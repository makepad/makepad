//! Self-contained Wayland server core. Std plus a few direct libc calls; no libwayland.
//! The wire codec, schema tables and object map are portable; sockets, fd passing and the
//! epoll loop are Linux only.
pub mod proto;
pub mod schema;
pub mod wire;
#[cfg(target_os = "linux")]
pub mod sys;
#[cfg(target_os = "linux")]
pub mod transport;
#[cfg(target_os = "linux")]
pub mod core;
#[cfg(target_os = "linux")]
pub mod server;
#[cfg(target_os = "linux")]
pub mod shm;
#[cfg(target_os = "linux")]
pub mod compositor;
#[cfg(target_os = "linux")]
pub mod host;
