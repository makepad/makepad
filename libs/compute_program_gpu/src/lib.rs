//! The GPU device for compute programs (`makepad-compute-program`): a session's GPU ops run on a
//! Cx (an app's, or [`spawn`]'s own thread's) in unattached compute passes on the plain HAL
//! path, scheduled against what else that GPU draws ([`GpuDevice`]).

mod device;
mod session;
pub use device::{spawn, GpuDevice};
pub use session::GpuSession;
