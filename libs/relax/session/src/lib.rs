//! Compilation inputs shared by every compiler layer.
pub mod clock;
pub mod options;
pub mod phase;
pub mod sip;
pub mod memo_local;
pub use relax_api_phase::sync;
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

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub unsafe fn reset_statics() {
    sip::reset_statics();
}

/// The statics `reset_static` restored, for the test that runs twice in one process: their padding
/// holds whatever the move left, so comparing their bytes says nothing. `None`: nobody asks.
pub static RESETS: std::sync::Mutex<Option<Vec<(usize, usize)>>> = std::sync::Mutex::new(None);

fn note_reset(at: usize, size: usize) {
    if let Some(list) = RESETS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        list.push((at, size));
    }
}

/// A static back to `value`, what it starts a process with: the old value is dropped. For a run's
/// end in place, when no thread of the run is left that could be using it.
///
/// # Safety
/// Nothing else reads or writes `slot` meanwhile.
pub unsafe fn reset_static<T>(slot: &'static T, value: T) {
    drop(std::ptr::replace(slot as *const T as *mut T, value));
    note_reset(slot as *const T as usize, std::mem::size_of::<T>());
}
