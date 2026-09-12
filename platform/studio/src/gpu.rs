//! Cross-platform serializable WM/hosted-app GPU transition vocabulary.
//!
//! These definitions do not implement coordination. Backend behavior is
//! native-Linux gated elsewhere. A nonzero `transition` identifies a single
//! transaction and must be validated by the controller.
//! `renderer_generation` differs from `transport_epoch`.

use crate::shared_framebuf::SharedSwapchain;
use makepad_micro_serde::*;

pub const GPU_CONTROL_VERSION: u32 = 1;

/// Physical GPU device identity, not a Vulkan process handle.
#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuDeviceIdentity {
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

/// Host → app GPU control. A nonzero `transition` is one transaction.
#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub enum HostToAppGpu {
    /// Close ordinary GPU frame submission. CPU application state remains.
    Prepare {
        transition: u64,
        renderer_generation: u64,
        target_device_uuid: [u8; 16],
        /// Host-announced windows needing a transport replacement. Dormant
        /// process-warm clients may have no hosted framebuffer yet.
        windows: Vec<usize>,
    },
    Poll {
        transition: u64,
    },
    Swapchain {
        transition: u64,
        transport_epoch: u64,
        swapchain: SharedSwapchain,
    },
    Commit {
        transition: u64,
    },
    Retire {
        transition: u64,
    },
    /// Pre-commit cancel only. The controller handles errors and stale
    /// transitions; these protocol definitions alone do not implement
    /// coordination.
    Cancel {
        transition: u64,
    },
}

impl HostToAppGpu {
    pub fn transition(&self) -> u64 {
        match self {
            Self::Prepare { transition, .. }
            | Self::Poll { transition }
            | Self::Swapchain { transition, .. }
            | Self::Commit { transition }
            | Self::Retire { transition }
            | Self::Cancel { transition } => *transition,
        }
    }
}

/// App → host GPU control. A nonzero `transition` is one transaction.
#[derive(SerBin, DeBin, SerJson, DeJson, Debug, Clone)]
pub enum AppToHostGpu {
    Hello {
        version: u32,
        renderer_generation: u64,
        device: GpuDeviceIdentity,
    },
    /// Old local submissions complete. Remote readers are not done; the host
    /// drains those separately.
    Quiesced {
        transition: u64,
    },
    /// Candidate resource migration complete, not yet committed.
    Prepared {
        transition: u64,
        renderer_generation: u64,
        device: GpuDeviceIdentity,
    },
    /// Import confirmed in the candidate. Old resources retained until Retire.
    SwapchainReady {
        transition: u64,
        transport_epoch: u64,
        window_id: usize,
    },
    Committed {
        transition: u64,
        renderer_generation: u64,
    },
    Cancelled {
        transition: u64,
    },
    Failed {
        transition: u64,
        message: String,
    },
    /// Old imports and the previous logical device have been released.
    Retired {
        transition: u64,
    },
}
