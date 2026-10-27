//! The OS layer. Everything above this module is portable std code; everything OS-specific is
//! here: posix.rs (libSystem / glibc declarations and the portable sys API in its POSIX form)
//! with macos.rs / linux.rs (constants, layouts, futex, errno), windows.rs (kernel32 & co. and
//! the same portable API), web.rs (the web targets, wasm32/wasm64-unknown-unknown: rustc's std for
//! them, `cfg(target_family = "wasm")`), rt.rs (the Relax runtime hooks). Only this module tree
//! declares foreign functions (and the Windows-only and web-only modules std selects for fs /
//! process / env / pipes).

#![allow(non_camel_case_types)]

#[cfg(unix)]
#[path = "posix.rs"]
mod posix;
#[cfg(unix)]
pub use posix::*;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
pub mod os;
#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "linux.rs"]
pub mod os;
#[cfg(windows)]
#[path = "windows.rs"]
pub mod os;
#[cfg(windows)]
pub use os::portable::*;
#[cfg(target_family = "wasm")]
#[path = "web.rs"]
pub mod os;
#[cfg(target_family = "wasm")]
pub use os::portable::*;

pub mod rt;

pub mod net;
pub mod time;
