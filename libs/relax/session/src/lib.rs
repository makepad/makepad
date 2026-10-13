//! Compilation inputs shared by every compiler layer.
pub mod options;
pub mod phase;
pub mod sip;
pub mod gen_local;
pub mod sync;
/// The one Win32 declaration module of makepad run; every relax crate depends on relax-session.
#[cfg(windows)]
pub mod win32;
pub use options::{Options, RuntimeOptions};

/// Compiler statics held for rollback; forget them before destroying their generations.
pub struct ZoneTaken {
    sip: sip::ZoneTaken,
}

/// Takes this crate's compiler statics while every compiler thread is quiesced.
pub fn zone_take() -> ZoneTaken {
    ZoneTaken { sip: sip::zone_take() }
}

/// Restores the statics after an aborted swap, forgetting the new generation's values.
pub fn zone_put_back(taken: ZoneTaken) {
    sip::zone_put_back(taken.sip);
}

#[test]
fn zone_reset_fresh_process() {
    const CHILD: &str = "MAKEPAD_RUN_ZONE_RESET_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        std::mem::forget(zone_take());
        std::mem::forget(zone_take());
        return;
    }
    // Other tests use these statics; the reset and its empty-state assertions need isolation.
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "zone_reset_fresh_process"])
        .env(CHILD, "1")
        .status().unwrap();
    assert!(status.success());
}

/// Immutable compiler and execution policy for one driver invocation.
/// rustc: compiler/rustc_session/src/session.rs:90-93.
#[derive(Default)]
pub struct Session {
    pub options: Options,
    pub runtime: RuntimeOptions,
}
