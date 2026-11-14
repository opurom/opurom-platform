//! Deterministic viewport state derived from resize and geometry wire events.
//!
//! `frame` remains the raw framebuffer size. `content` is the rectangle hosts
//! report for content placement once a hinge is known.

use crate::display_geometry::{
    resolve_layout, DisplayGeometry, GeometryError, LayoutDecision, Posture, Rect, WindowRole,
    DPI_UNIT,
};
use crate::Size;

/// Resolved layout metadata attached to one viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutPatch {
    pub geometry: DisplayGeometry,
    pub frame: Size,
    pub content: Rect,
    pub decision: LayoutDecision,
}

impl LayoutPatch {
    /// A plain resize has no hinge and keeps the whole frame as content.
    pub fn from_size(size: Size) -> Self {
        Self::from_geometry(DisplayGeometry {
            width: size.width,
            height: size.height,
            dpi_scale: DPI_UNIT,
            posture: Posture::Flat,
            window_role: WindowRole::Primary,
            fold: None,
        })
    }

    pub fn from_geometry(geometry: DisplayGeometry) -> Self {
        let decision = resolve_layout(geometry);
        Self {
            geometry,
            frame: Size {
                width: geometry.width,
                height: geometry.height,
            },
            content: content_viewport(geometry, decision.avoid_fold),
            decision,
        }
    }

    /// Reject patches whose fields do not match a deterministic resolution of
    /// their geometry, including patches built by struct literal.
    pub fn validate(self) -> Result<(), GeometryError> {
        self.geometry.validate()?;
        if self == Self::from_geometry(self.geometry) {
            Ok(())
        } else {
            Err(GeometryError::InvalidLayoutPatch)
        }
    }
}

/// Resolve and validate one geometry into a patch downstream adapters can read.
pub fn layout_patch(geometry: DisplayGeometry) -> Result<LayoutPatch, GeometryError> {
    geometry.validate()?;
    Ok(LayoutPatch::from_geometry(geometry))
}

fn content_viewport(geometry: DisplayGeometry, avoid_fold: Option<Rect>) -> Rect {
    let Some(avoid) = avoid_fold else {
        return Rect {
            x: 0,
            y: 0,
            width: geometry.width,
            height: geometry.height,
        };
    };
    let left_width = avoid.x.min(geometry.width);
    let right_start = avoid.x.saturating_add(avoid.width).min(geometry.width);
    let right_width = geometry.width.saturating_sub(right_start);
    // shortcut: one content pane on the larger side, upgrade when a host needs two panes at once.
    if left_width >= right_width {
        Rect {
            x: 0,
            y: 0,
            width: left_width,
            height: geometry.height,
        }
    } else {
        Rect {
            x: right_start,
            y: 0,
            width: right_width,
            height: geometry.height,
        }
    }
}
