//! Fold-aware display geometry and responsive layout decisions.
//!
//! All quantities are integer logical pixels or fixed-point integers with
//! `1024 == 1.0`; no floating-point term affects a decision. Hosts validate a
//! geometry with [`DisplayGeometry::new`] at the trust boundary, while
//! [`resolve_layout`] stays total even for a geometry built by struct literal.

use std::fmt;

/// Fixed-point unit for DPI scale: `1024 == 1.0`.
pub const DPI_UNIT: u32 = 1024;
/// Sane DPI bounds: 0.25x through 4.0x.
pub const MIN_DPI_SCALE: u32 = DPI_UNIT / 4;
pub const MAX_DPI_SCALE: u32 = DPI_UNIT * 4;
/// Fixed-point unit for [`LayoutDecision::split_ratio`]: `1024 == 1.0`.
pub const SPLIT_RATIO_UNIT: u32 = 1024;
/// Hinge angles are inclusive `0..=180` degrees.
pub const MAX_HINGE_ANGLE: i32 = 180;
/// Width boundaries of the five documented tiers, in logical pixels.
pub const COMPACT_LIMIT: u32 = 600;
pub const MEDIUM_LIMIT: u32 = 900;
pub const EXPANDED_LIMIT: u32 = 1200;
pub const WIDE_LIMIT: u32 = 1600;
/// A hinge widens the gutter by this many logical pixels.
pub const HINGE_GUTTER_BONUS: u32 = 16;
pub const MAX_GUTTER: u32 = 64;
/// A `Hinge` posture without an explicit fold gets a centered fold this wide.
pub const DEFAULT_FOLD_WIDTH: u32 = 24;
/// `0` means no split; a split ratio is otherwise clamped to 25%..75%.
pub const DEFAULT_SPLIT_RATIO: u32 = SPLIT_RATIO_UNIT / 2;
pub const MIN_SPLIT_RATIO: u32 = SPLIT_RATIO_UNIT / 4;
pub const MAX_SPLIT_RATIO: u32 = SPLIT_RATIO_UNIT * 3 / 4;

/// Physical device posture. Only `Hinge` carries an integer angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    Flat,
    HalfOpened,
    Book,
    Hinge { angle: i32 },
}

/// The window's role in a multi-window or split-screen arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRole {
    Primary,
    Secondary,
    Split,
}

/// Horizontal hinge region in logical pixels. The width is always positive and
/// `x + width` never exceeds the geometry width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    pub x: u32,
    pub width: u32,
}

/// A full-height rectangle content must avoid, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The five documented width tiers: compact phone, wide phone, tablet,
/// desktop narrow and desktop full.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayoutTier {
    Compact,
    Medium,
    Expanded,
    Wide,
    Ultra,
}

impl LayoutTier {
    pub fn from_width(width: u32) -> Self {
        if width < COMPACT_LIMIT {
            Self::Compact
        } else if width < MEDIUM_LIMIT {
            Self::Medium
        } else if width < EXPANDED_LIMIT {
            Self::Expanded
        } else if width < WIDE_LIMIT {
            Self::Wide
        } else {
            Self::Ultra
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Medium => "medium",
            Self::Expanded => "expanded",
            Self::Wide => "wide",
            Self::Ultra => "ultra",
        }
    }

    /// Columns before any hinge adjustment.
    pub fn columns(self) -> u32 {
        match self {
            Self::Compact => 1,
            Self::Medium => 2,
            Self::Expanded => 4,
            Self::Wide => 8,
            Self::Ultra => 12,
        }
    }

    /// Gutter before any hinge adjustment.
    pub fn gutter(self) -> u32 {
        match self {
            Self::Compact => 8,
            Self::Medium => 12,
            Self::Expanded => 16,
            Self::Wide => 20,
            Self::Ultra => 24,
        }
    }
}

/// Validation and wire-framing failures are values, never panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    NonPositiveDimension,
    DpiScaleOutOfRange,
    HingeAngleOutOfRange,
    EmptyFoldRegion,
    FoldOutsideWidth,
    InvalidWireHeader,
    InvalidWireObject,
    InvalidWireOpcode,
    InvalidWireLength,
    InvalidWireFlags,
    InvalidPostureTag,
    InvalidRoleTag,
    UnexpectedHingeAngle,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::NonPositiveDimension => "display width and height must be positive",
            Self::DpiScaleOutOfRange => "DPI scale is outside 0.25x..4.0x",
            Self::HingeAngleOutOfRange => "hinge angle is outside 0..180 degrees",
            Self::EmptyFoldRegion => "fold width must be positive",
            Self::FoldOutsideWidth => "fold region must lie inside the display width",
            Self::InvalidWireHeader => "geometry event header is invalid",
            Self::InvalidWireObject => "geometry event uses the wrong object ID",
            Self::InvalidWireOpcode => "geometry event uses the wrong opcode",
            Self::InvalidWireLength => "geometry event has the wrong length",
            Self::InvalidWireFlags => "geometry event flags are invalid",
            Self::InvalidPostureTag => "geometry event posture tag is unknown",
            Self::InvalidRoleTag => "geometry event window role tag is unknown",
            Self::UnexpectedHingeAngle => "hinge angle is set without a hinge posture",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for GeometryError {}

/// One display surface: logical size, DPI, posture, window role and hinge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayGeometry {
    pub width: u32,
    pub height: u32,
    /// Fixed-point DPI scale, [`DPI_UNIT`] == 1.0.
    pub dpi_scale: u32,
    pub posture: Posture,
    pub window_role: WindowRole,
    pub fold: Option<FoldRegion>,
}

impl DisplayGeometry {
    /// Construct a validated geometry. A fold may accompany any posture; its
    /// presence denotes a physical hinge just like [`Posture::Hinge`].
    pub fn new(
        width: u32,
        height: u32,
        dpi_scale: u32,
        posture: Posture,
        window_role: WindowRole,
        fold: Option<FoldRegion>,
    ) -> Result<Self, GeometryError> {
        let geometry = Self {
            width,
            height,
            dpi_scale,
            posture,
            window_role,
            fold,
        };
        geometry.validate()?;
        Ok(geometry)
    }

    pub fn validate(self) -> Result<(), GeometryError> {
        if self.width == 0 || self.height == 0 {
            return Err(GeometryError::NonPositiveDimension);
        }
        if !(MIN_DPI_SCALE..=MAX_DPI_SCALE).contains(&self.dpi_scale) {
            return Err(GeometryError::DpiScaleOutOfRange);
        }
        if let Posture::Hinge { angle } = self.posture {
            if !(0..=MAX_HINGE_ANGLE).contains(&angle) {
                return Err(GeometryError::HingeAngleOutOfRange);
            }
        }
        if let Some(fold) = self.fold {
            if fold.width == 0 {
                return Err(GeometryError::EmptyFoldRegion);
            }
            let end = fold
                .x
                .checked_add(fold.width)
                .ok_or(GeometryError::FoldOutsideWidth)?;
            if end > self.width {
                return Err(GeometryError::FoldOutsideWidth);
            }
        }
        Ok(())
    }

    /// The effective hinge region. A `Hinge` posture without an explicit fold
    /// gets a centered default so layout always has a region to avoid.
    pub fn hinge_region(self) -> Option<FoldRegion> {
        if let Some(fold) = self.fold {
            return Some(fold);
        }
        if !matches!(self.posture, Posture::Hinge { .. }) || self.width == 0 {
            return None;
        }
        let width = DEFAULT_FOLD_WIDTH.min(self.width);
        Some(FoldRegion {
            x: (self.width - width) / 2,
            width,
        })
    }

    pub fn has_hinge(self) -> bool {
        self.hinge_region().is_some()
    }
}

/// Pure, deterministic layout decision for one geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutDecision {
    pub tier: LayoutTier,
    pub columns: u32,
    /// Gutter in logical pixels; a hinge always increases it.
    pub gutter: u32,
    /// Full-height region content must avoid when a hinge is present.
    pub avoid_fold: Option<Rect>,
    /// Fixed-point primary-pane ratio, [`SPLIT_RATIO_UNIT`] == 1.0; 0 = no split.
    pub split_ratio: u32,
}

/// Resolve layout with integer-only arithmetic: tier, columns, gutter,
/// hinge avoidance and split ratio. The same geometry always yields the same
/// decision on every host.
pub fn resolve_layout(geometry: DisplayGeometry) -> LayoutDecision {
    let tier = LayoutTier::from_width(geometry.width);
    let hinge = geometry.hinge_region();
    let gutter = if hinge.is_some() {
        (tier.gutter() + HINGE_GUTTER_BONUS).min(MAX_GUTTER)
    } else {
        tier.gutter()
    };

    let avoid_fold = hinge.map(|fold| {
        let start = fold.x.saturating_sub(gutter);
        let end = fold
            .x
            .saturating_add(fold.width)
            .saturating_add(gutter)
            .min(geometry.width);
        Rect {
            x: start,
            y: 0,
            width: end.saturating_sub(start),
            height: geometry.height,
        }
    });

    let split_ratio = if !matches!(geometry.window_role, WindowRole::Split) {
        0
    } else {
        match (hinge, geometry.width) {
            (Some(fold), width) if width > 0 => {
                let center = u64::from(fold.x) + u64::from(fold.width) / 2;
                let raw = center * u64::from(SPLIT_RATIO_UNIT) / u64::from(width);
                let clamped = raw.clamp(u64::from(MIN_SPLIT_RATIO), u64::from(MAX_SPLIT_RATIO));
                u32::try_from(clamped).unwrap_or(MAX_SPLIT_RATIO)
            }
            _ => DEFAULT_SPLIT_RATIO,
        }
    };

    LayoutDecision {
        tier,
        columns: tier.columns(),
        gutter,
        avoid_fold,
        split_ratio,
    }
}
