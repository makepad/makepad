//! Relax's middle end on RIR (relax-backend owns RIR, register allocation and machine code).
//! [`opt`] is the O1/O3 optimiser, [`loops`] and [`ranges`] its loop and range passes,
//! [`interp`] the threaded interpreter tier, [`native`] Relax's entry into the back end, and
//! [`fncost`], [`warm`] and [`exec_census`] the compile profilers.

pub mod fncost;
pub mod interp;
pub mod exec_census;
pub mod warm;
pub mod loops;
pub mod native;
pub mod opt;
pub mod ranges;

/// This crate has no movable compiler statics; opt::T remains a process-wide OnceLock.
pub struct ZoneTaken;

/// Takes this crate's compiler statics while every compiler thread is quiesced.
pub fn zone_take() -> ZoneTaken {
    // opt::T is a OnceLock: shared statics cannot reset it. Its string is leaked in the kept
    // heap, so it needs no reset.
    ZoneTaken
}

pub fn zone_put_back(_: ZoneTaken) {}

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
