//! The device-independent parts of an XR shell (see each module): head poses and their sources,
//! a headset display's optics, and the rotational reprojection of a finished eye pair. Shared by
//! every owner of a headset display: the WM (a tethered headset), the HAL (OpenXR), and remote
//! rendering, so none of this math exists twice.

pub mod optics;
pub mod pose;
pub mod warp;
