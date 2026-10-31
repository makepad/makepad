//! Native content hosted by a window manager. All IDs are scoped to the sending app.
use makepad_micro_serde::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin, SerJson, DeJson)]
pub struct NativeAppId { pub session: u32, pub index: u32, pub generation: u32 }

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum NativeAppRequest {
    Create { id: NativeAppId, app: String },
    Destroy { id: NativeAppId },
    Op { id: NativeAppId, op: u32, arg: u64, data: Vec<u8> },
    /// Physical content size, including the owning pass's DPI.
    Configure { id: NativeAppId, width: u32, height: u32 },
    /// Last use has been submitted. A sync_file, if present, precedes this message on C2H.
    Release { id: NativeAppId, buffer: u64, fence: bool },
    /// The window frame sampling this commit was presented.
    Presented { id: NativeAppId, commit: u64 },
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct NativeAppBuffer {
    pub width: u32, pub height: u32, pub fourcc: u32, pub modifier: u64,
    pub planes: Vec<NativeAppPlane>,
}
#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct NativeAppPlane { pub offset: u32, pub stride: u32 }

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum NativeAppEvent {
    /// Plane descriptors precede this message on hosted_ipc, under `buffer`. Sent once per allocation.
    Buffer { id: NativeAppId, buffer: u64, desc: NativeAppBuffer },
    /// Acquire has already been observed by the WM's Wayland server. No CPU pixels on this wire.
    Commit { id: NativeAppId, buffer: u64, commit: u64 },
    Unmapped { id: NativeAppId },
    Forget { id: NativeAppId, buffer: u64 },
    State { id: NativeAppId, what: u32, a: u64, data: Vec<u8> },
}
