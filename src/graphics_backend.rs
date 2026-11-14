//! Deterministic graphics-backend capability model for the platform adapter.
//!
//! This module models backend availability and capabilities; it performs no
//! driver calls and has no OS, clock or randomness dependency. ANGLE is the
//! OpenGL ES translation row, D3D11/D3D12 are Windows-only rows, and Software
//! is the headless fallback that never requires a live device.

use std::fmt;

/// Lowest texture size a capability row may report, in logical pixels.
pub const MIN_TEXTURE_SIZE: u32 = 256;
/// Largest texture size a capability row may report, in logical pixels.
pub const MAX_TEXTURE_SIZE: u32 = 16_384;
/// Largest integer MSAA sample count a capability row may report.
pub const MAX_MSAA_SAMPLES: u32 = 16;
/// Largest command-buffer count a capability row may report.
pub const MAX_COMMAND_BUFFERS: u32 = 8;

/// Supported effect bits. Every backend supports ordinary alpha blending.
pub const EFFECT_BLEND: u32 = 1 << 0;
pub const EFFECT_BLUR: u32 = 1 << 1;
pub const EFFECT_SHADOW: u32 = 1 << 2;
pub const EFFECT_COMPUTE: u32 = 1 << 3;
/// All defined effect bits.
pub const EFFECT_MASK: u32 = EFFECT_BLEND | EFFECT_BLUR | EFFECT_SHADOW | EFFECT_COMPUTE;
/// Wire tag base for a Software fallback reason.
pub const FALLBACK_REASON_BASE: u32 = 0x10;

/// Host operating-system class used by capability selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOs {
    MacOs,
    Windows,
    Linux,
    Other,
}

impl HostOs {
    /// Compile-time OS class for an adapter default; deterministic on every host.
    pub const fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub const fn tag(self) -> u32 {
        match self {
            Self::MacOs => 1,
            Self::Windows => 2,
            Self::Linux => 3,
            Self::Other => 4,
        }
    }

    pub fn from_tag(tag: u32) -> Result<Self, BackendError> {
        Ok(match tag {
            1 => Self::MacOs,
            2 => Self::Windows,
            3 => Self::Linux,
            4 => Self::Other,
            _ => return Err(BackendError::UnknownHostOs(tag)),
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }
}

/// The graphics backends named by the platform adapter capability model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsBackend {
    Metal,
    Vulkan,
    /// ANGLE / OpenGL ES translation.
    AngleGlEs,
    D3D11,
    D3D12,
    Software,
}

impl GraphicsBackend {
    pub const ALL: [Self; 6] = [
        Self::Metal,
        Self::Vulkan,
        Self::AngleGlEs,
        Self::D3D11,
        Self::D3D12,
        Self::Software,
    ];

    pub const fn tag(self) -> u32 {
        match self {
            Self::Metal => 1,
            Self::Vulkan => 2,
            Self::AngleGlEs => 3,
            Self::D3D11 => 4,
            Self::D3D12 => 5,
            Self::Software => 6,
        }
    }

    pub fn from_tag(tag: u32) -> Result<Self, BackendError> {
        Ok(match tag {
            1 => Self::Metal,
            2 => Self::Vulkan,
            3 => Self::AngleGlEs,
            4 => Self::D3D11,
            5 => Self::D3D12,
            6 => Self::Software,
            _ => return Err(BackendError::UnknownBackend(tag)),
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Vulkan => "vulkan",
            Self::AngleGlEs => "angle-gl-es",
            Self::D3D11 => "d3d11",
            Self::D3D12 => "d3d12",
            Self::Software => "software",
        }
    }

    pub const fn supports_os(self, os: HostOs) -> bool {
        match self {
            Self::Metal => match os {
                HostOs::MacOs => true,
                _ => false,
            },
            Self::D3D11 | Self::D3D12 => match os {
                HostOs::Windows => true,
                _ => false,
            },
            Self::Vulkan | Self::AngleGlEs | Self::Software => true,
        }
    }

    /// The one deterministic capability row for this backend.
    pub const fn capabilities(self) -> Capabilities {
        match self {
            Self::Metal => Capabilities {
                max_texture_size: 16_384,
                msaa_samples: 8,
                supported_effects: EFFECT_MASK,
                requires_live_device: true,
                max_command_buffers: 3,
            },
            Self::Vulkan => Capabilities {
                max_texture_size: 16_384,
                msaa_samples: 16,
                supported_effects: EFFECT_MASK,
                requires_live_device: true,
                max_command_buffers: MAX_COMMAND_BUFFERS,
            },
            Self::AngleGlEs => Capabilities {
                max_texture_size: 8_192,
                msaa_samples: 4,
                supported_effects: EFFECT_BLEND | EFFECT_BLUR | EFFECT_SHADOW,
                requires_live_device: true,
                max_command_buffers: 1,
            },
            Self::D3D11 => Capabilities {
                max_texture_size: 16_384,
                msaa_samples: 8,
                supported_effects: EFFECT_BLEND | EFFECT_BLUR | EFFECT_SHADOW,
                requires_live_device: true,
                max_command_buffers: 4,
            },
            Self::D3D12 => Capabilities {
                max_texture_size: 16_384,
                msaa_samples: 16,
                supported_effects: EFFECT_MASK,
                requires_live_device: true,
                max_command_buffers: MAX_COMMAND_BUFFERS,
            },
            Self::Software => Capabilities {
                max_texture_size: 4_096,
                msaa_samples: 1,
                supported_effects: EFFECT_BLEND,
                requires_live_device: false,
                max_command_buffers: 1,
            },
        }
    }
}

/// Integer-only capability row for one graphics backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub max_texture_size: u32,
    pub msaa_samples: u32,
    pub supported_effects: u32,
    pub requires_live_device: bool,
    pub max_command_buffers: u32,
}

impl Capabilities {
    /// Reject unknown effect bits and field-level inconsistencies as values.
    pub fn validate(self) -> Result<(), BackendError> {
        if !(MIN_TEXTURE_SIZE..=MAX_TEXTURE_SIZE).contains(&self.max_texture_size) {
            return Err(BackendError::InvalidTextureSize(self.max_texture_size));
        }
        if self.msaa_samples == 0
            || self.msaa_samples > MAX_MSAA_SAMPLES
            || !self.msaa_samples.is_power_of_two()
        {
            return Err(BackendError::InvalidMsaaSamples(self.msaa_samples));
        }
        if self.supported_effects & !EFFECT_MASK != 0 || self.supported_effects & EFFECT_BLEND == 0
        {
            return Err(BackendError::UnknownEffectMask(self.supported_effects));
        }
        if self.max_command_buffers == 0 || self.max_command_buffers > MAX_COMMAND_BUFFERS {
            return Err(BackendError::InvalidCommandBuffers(
                self.max_command_buffers,
            ));
        }
        if !self.requires_live_device
            && (self.supported_effects != EFFECT_BLEND || self.msaa_samples != 1)
        {
            return Err(BackendError::InconsistentCapabilitySet);
        }
        Ok(())
    }

    /// Reject capability rows that do not exactly match one backend default.
    pub fn validate_for(self, backend: GraphicsBackend) -> Result<(), BackendError> {
        self.validate()?;
        if self == backend.capabilities() {
            Ok(())
        } else {
            Err(BackendError::UnexpectedCapabilities(backend))
        }
    }
}

/// Concrete typed reason that forced the Software fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackReason {
    NoAvailableBackends,
    NoSupportedBackend,
    RequestedBackendUnavailable,
    RequestedBackendUnsupported,
}

impl FallbackReason {
    pub const fn tag(self) -> u32 {
        match self {
            Self::NoAvailableBackends => 0,
            Self::NoSupportedBackend => 1,
            Self::RequestedBackendUnavailable => 2,
            Self::RequestedBackendUnsupported => 3,
        }
    }

    pub fn from_tag(tag: u32) -> Result<Self, BackendError> {
        Ok(match tag {
            0 => Self::NoAvailableBackends,
            1 => Self::NoSupportedBackend,
            2 => Self::RequestedBackendUnavailable,
            3 => Self::RequestedBackendUnsupported,
            _ => return Err(BackendError::UnknownSelectionReason(tag)),
        })
    }
}

/// Why one backend was selected, including every Software fallback case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionReason {
    HostDefault,
    Requested,
    Preferred,
    Available,
    SoftwareFallback(FallbackReason),
}

impl SelectionReason {
    pub const fn tag(self) -> u32 {
        match self {
            Self::HostDefault => 1,
            Self::Requested => 2,
            Self::Preferred => 3,
            Self::Available => 4,
            Self::SoftwareFallback(reason) => FALLBACK_REASON_BASE + reason.tag(),
        }
    }

    pub fn from_tag(tag: u32) -> Result<Self, BackendError> {
        Ok(match tag {
            1 => Self::HostDefault,
            2 => Self::Requested,
            3 => Self::Preferred,
            4 => Self::Available,
            FALLBACK_REASON_BASE..=0x13 => {
                Self::SoftwareFallback(FallbackReason::from_tag(tag - FALLBACK_REASON_BASE)?)
            }
            _ => return Err(BackendError::UnknownSelectionReason(tag)),
        })
    }
}

/// Typed errors for unknown tags, invalid capability rows and malformed wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    UnknownBackend(u32),
    UnknownHostOs(u32),
    UnknownSelectionReason(u32),
    UnknownEffectMask(u32),
    InvalidTextureSize(u32),
    InvalidMsaaSamples(u32),
    InvalidCommandBuffers(u32),
    InconsistentCapabilitySet,
    UnexpectedCapabilities(GraphicsBackend),
    UnsupportedOnHost {
        os: HostOs,
        backend: GraphicsBackend,
    },
    InvalidWireHeader,
    InvalidWireObject,
    InvalidWireOpcode,
    InvalidWireLength,
    InvalidWireMagic,
    InvalidWireFlag,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnknownBackend(tag) => write!(f, "unknown graphics backend tag {tag}"),
            Self::UnknownHostOs(tag) => write!(f, "unknown host OS tag {tag}"),
            Self::UnknownSelectionReason(tag) => {
                write!(f, "unknown graphics selection reason tag {tag}")
            }
            Self::UnknownEffectMask(mask) => {
                write!(f, "unknown graphics effect mask 0x{mask:08x}")
            }
            Self::InvalidTextureSize(size) => {
                write!(f, "graphics max texture size {size} is out of range")
            }
            Self::InvalidMsaaSamples(samples) => {
                write!(f, "graphics MSAA sample count {samples} is invalid")
            }
            Self::InvalidCommandBuffers(count) => {
                write!(f, "graphics command-buffer count {count} is invalid")
            }
            Self::InconsistentCapabilitySet => {
                f.write_str("graphics capability set is internally inconsistent")
            }
            Self::UnexpectedCapabilities(backend) => write!(
                f,
                "graphics capability set does not match {} defaults",
                backend.as_str()
            ),
            Self::UnsupportedOnHost { os, backend } => write!(
                f,
                "{} is not supported on {}",
                backend.as_str(),
                os.as_str()
            ),
            Self::InvalidWireHeader => f.write_str("graphics capability event header is invalid"),
            Self::InvalidWireObject => {
                f.write_str("graphics capability event uses the wrong object ID")
            }
            Self::InvalidWireOpcode => {
                f.write_str("graphics capability event uses the wrong opcode")
            }
            Self::InvalidWireLength => {
                f.write_str("graphics capability event has the wrong length")
            }
            Self::InvalidWireMagic => f.write_str("graphics capability event has the wrong magic"),
            Self::InvalidWireFlag => {
                f.write_str("graphics capability event has an invalid live-device flag")
            }
        }
    }
}

impl std::error::Error for BackendError {}

/// Pure host descriptor: OS class, available hardware backends and preference order.
/// Software is the unconditional fallback and may be omitted from `available`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDescriptor {
    pub os: HostOs,
    pub available: Vec<GraphicsBackend>,
    pub preferences: Vec<GraphicsBackend>,
}

impl HostDescriptor {
    pub fn new(
        os: HostOs,
        available: impl AsRef<[GraphicsBackend]>,
        preferences: impl AsRef<[GraphicsBackend]>,
    ) -> Self {
        Self {
            os,
            available: available.as_ref().to_vec(),
            preferences: preferences.as_ref().to_vec(),
        }
    }

    /// Validate the canonical rows named by this descriptor before selection.
    pub fn validate(&self) -> Result<(), BackendError> {
        for &backend in self.available.iter().chain(self.preferences.iter()) {
            backend.capabilities().validate_for(backend)?;
        }
        Ok(())
    }
}

/// A selected backend plus the typed reason for that selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendSelection {
    pub backend: GraphicsBackend,
    pub reason: SelectionReason,
}

impl BackendSelection {
    pub const fn capabilities(self, os: HostOs) -> GraphicsBackendCapabilities {
        GraphicsBackendCapabilities::new(os, self.backend, self.reason)
    }
}

/// One backend capability row attached to a host OS and selection reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphicsBackendCapabilities {
    pub host_os: HostOs,
    pub backend: GraphicsBackend,
    pub capabilities: Capabilities,
    pub reason: SelectionReason,
}

impl GraphicsBackendCapabilities {
    pub const fn new(host_os: HostOs, backend: GraphicsBackend, reason: SelectionReason) -> Self {
        Self {
            host_os,
            backend,
            capabilities: backend.capabilities(),
            reason,
        }
    }

    /// Deterministic Software row for an adapter that has no selected device.
    pub const fn software_default() -> Self {
        Self::new(
            HostOs::host(),
            GraphicsBackend::Software,
            SelectionReason::HostDefault,
        )
    }

    pub fn validate(self) -> Result<(), BackendError> {
        if !self.backend.supports_os(self.host_os) {
            return Err(BackendError::UnsupportedOnHost {
                os: self.host_os,
                backend: self.backend,
            });
        }
        self.capabilities.validate_for(self.backend)
    }
}

fn is_candidate(host: &HostDescriptor, backend: GraphicsBackend) -> bool {
    backend != GraphicsBackend::Software
        && backend.supports_os(host.os)
        && host.available.contains(&backend)
}

fn fallback_reason(host: &HostDescriptor, requested: Option<GraphicsBackend>) -> FallbackReason {
    if let Some(backend) = requested {
        if !backend.supports_os(host.os) {
            return FallbackReason::RequestedBackendUnsupported;
        }
        if !host.available.contains(&backend) {
            return FallbackReason::RequestedBackendUnavailable;
        }
    }
    if host.available.is_empty()
        || host
            .available
            .iter()
            .all(|&backend| backend == GraphicsBackend::Software)
    {
        FallbackReason::NoAvailableBackends
    } else {
        FallbackReason::NoSupportedBackend
    }
}

/// Pure backend selection with its typed reason. The first supported candidate
/// is the explicit request, then the preference list, then the available list.
pub fn select_backend_with_reason(
    host: &HostDescriptor,
    requested: Option<GraphicsBackend>,
) -> Result<BackendSelection, BackendError> {
    host.validate()?;
    if let Some(backend) = requested {
        backend.capabilities().validate_for(backend)?;
        if backend == GraphicsBackend::Software || is_candidate(host, backend) {
            return Ok(BackendSelection {
                backend,
                reason: SelectionReason::Requested,
            });
        }
    }
    for &backend in &host.preferences {
        if is_candidate(host, backend) {
            return Ok(BackendSelection {
                backend,
                reason: SelectionReason::Preferred,
            });
        }
    }
    for &backend in &host.available {
        if is_candidate(host, backend) {
            return Ok(BackendSelection {
                backend,
                reason: SelectionReason::Available,
            });
        }
    }
    Ok(BackendSelection {
        backend: GraphicsBackend::Software,
        reason: SelectionReason::SoftwareFallback(fallback_reason(host, requested)),
    })
}

/// Pure backend selection: `select_backend_with_reason` without its reason.
pub fn select_backend(
    host: &HostDescriptor,
    requested: Option<GraphicsBackend>,
) -> Result<GraphicsBackend, BackendError> {
    Ok(select_backend_with_reason(host, requested)?.backend)
}
