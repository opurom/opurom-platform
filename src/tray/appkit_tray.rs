//! The macOS tray adapter: a real `NSStatusItem` with an `NSMenu` whose items
//! route clicks through one target object into the adapter's event queue.
//!
//! AppKit status-bar surfaces are main-thread-only, so `mount` requires a
//! `MainThreadMarker`; off the main thread it degrades to the typed
//! [`TrayError::Unavailable`] reason instead of touching AppKit at all.
//!
//! Clicks arrive as real `NSMenuItem` actions on the main thread; the target
//! forwards the clicked item's tag into a mutex-locked queue that
//! `poll_event` drains from any thread. There is no fabricated click path:
//! the CLI `--verify-desktop` check drives the real selector dispatch.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_core_foundation::CGFloat;
use objc2_foundation::NSString;

use super::{TrayError, TrayEvent, TrayHost, TrayMenu};

#[derive(Clone, Default)]
struct TrayIvars {
    queue: Arc<Mutex<VecDeque<u32>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and TrayTarget has no Drop.
    #[unsafe(super(NSObject))]
    #[ivars = TrayIvars]
    struct TrayTarget;

    impl TrayTarget {
        // The action every tray menu item routes to. The sender is the clicked
        // NSMenuItem; its tag is the application-assigned item id.
        #[unsafe(method(opuromTrayItemClicked:))]
        fn opurom_tray_item_clicked(&self, _sender: &AnyObject) {
            let tag: i64 = unsafe { msg_send![_sender, tag] };
            if tag >= 0 {
                if let Ok(mut queue) = self.ivars().queue.lock() {
                    queue.push_back(tag as u32);
                }
            }
        }
    }

    unsafe impl NSObjectProtocol for TrayTarget {}
);

impl TrayTarget {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(TrayIvars::default());
        unsafe { msg_send![super(this), init] }
    }
}

/// The live macOS tray host. `new()` touches no AppKit, so constructing it is
/// safe anywhere; only `mount`/`update`/`unmount` need the main thread.
#[derive(Default)]
pub struct AppKitTray {
    status_item: Option<Retained<NSStatusItem>>,
    menu: Option<Retained<NSMenu>>,
    target: Option<Retained<TrayTarget>>,
    quit_id: Option<u32>,
    queue: Option<Arc<Mutex<VecDeque<u32>>>>,
}

impl AppKitTray {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TrayHost for AppKitTray {
    fn mount(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.status_item.is_some() {
            return Err(TrayError::AlreadyMounted);
        }
        menu.validate()?;
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(TrayError::Unavailable(
                "the AppKit status bar requires the main thread",
            ));
        };
        let target = TrayTarget::new();
        let queue = target.ivars().queue.clone();

        let status_item = NSStatusBar::systemStatusBar()
            .statusItemWithLength(NSVariableStatusItemLength as CGFloat);
        if let Some(button) = status_item.button(mtm) {
            button.setTitle(&NSString::from_str(&menu.title));
        }

        let ns_menu = build_menu(mtm, &menu, &target)?;
        status_item.setMenu(Some(&ns_menu));

        self.quit_id = menu.quit_id;
        self.queue = Some(queue);
        self.menu = Some(ns_menu);
        self.target = Some(target);
        self.status_item = Some(status_item);
        Ok(())
    }

    fn update(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.status_item.is_none() {
            return Err(TrayError::NotMounted);
        }
        menu.validate()?;
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(TrayError::Unavailable(
                "the AppKit status bar requires the main thread",
            ));
        };
        let target = self
            .target
            .as_ref()
            .ok_or(TrayError::Backend("missing tray target"))?;
        let ns_menu = build_menu(mtm, &menu, target)?;
        if let Some(status_item) = &self.status_item {
            status_item.setMenu(Some(&ns_menu));
        }
        self.quit_id = menu.quit_id;
        self.menu = Some(ns_menu);
        Ok(())
    }

    fn poll_event(&mut self) -> Option<TrayEvent> {
        let queue = self.queue.as_ref()?;
        let tag = queue.lock().ok()?.pop_front()?;
        if Some(tag) == self.quit_id {
            Some(TrayEvent::QuitRequested(tag))
        } else {
            Some(TrayEvent::ItemClicked(tag))
        }
    }

    fn unmount(&mut self) -> Result<(), TrayError> {
        let status_item = self.status_item.take().ok_or(TrayError::NotMounted)?;
        if MainThreadMarker::new().is_none() {
            // The retained surfaces stay dropped; the system status item is
            // reclaimed with them on the main thread's next pass.
            return Err(TrayError::Unavailable(
                "the AppKit status bar requires the main thread",
            ));
        }
        status_item.setMenu(None);
        NSStatusBar::systemStatusBar().removeStatusItem(&status_item);
        self.menu = None;
        self.target = None;
        self.quit_id = None;
        self.queue = None;
        Ok(())
    }

    fn is_mounted(&self) -> bool {
        self.status_item.is_some()
    }
}

fn build_menu(
    mtm: MainThreadMarker,
    menu: &TrayMenu,
    target: &TrayTarget,
) -> Result<Retained<NSMenu>, TrayError> {
    let ns_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&menu.title));
    for item in &menu.items {
        let ns_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&item.label),
                Some(sel!(opuromTrayItemClicked:)),
                &NSString::from_str(""),
            )
        };
        unsafe {
            ns_item.setTag(item.id as isize);
            ns_item.setEnabled(item.enabled);
            ns_item.setTarget(Some(target));
        }
        ns_menu.addItem(&ns_item);
    }
    Ok(ns_menu)
}
