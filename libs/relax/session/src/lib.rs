//! Compilation inputs shared by every compiler layer.
pub mod options;
pub mod phase;
pub mod sip;
pub mod memo_local;
pub mod sync;
/// The one Win32 declaration module of makepad run; every relax crate depends on relax-session.
#[cfg(windows)]
pub mod win32;
pub use options::{Options, RuntimeOptions};

/// Immutable compiler and execution policy for one driver invocation.
/// rustc: compiler/rustc_session/src/session.rs:90-93.
#[derive(Default)]
pub struct Session {
    pub options: Options,
    pub runtime: RuntimeOptions,
}

// This crate's source stamp (build.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));
