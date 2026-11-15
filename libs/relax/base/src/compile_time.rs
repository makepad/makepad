//! What third-party packages' compile-time code may do: one policy per process, chosen by the
//! entry point (relax-runner Host `compile_time_code`), read by the proc-macro host
//! (relax-compiler pmhost.rs) and by the build-script runs (relax-front unitgraph.rs).

/// What compile-time code of third-party packages may do (proc macros and build scripts).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    /// it runs as host code with the person's rights, as cargo runs it (bare `relax`)
    Open,
    /// code whose closure includes a registry or git package runs confined (`makepad run`)
    Sandboxed,
}

/// 0 = no entry point chose (read as Sandboxed), 1 = Sandboxed, 2 = Open
static POLICY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const SANDBOXED: u8 = 1;
const OPEN: u8 = 2;

/// The entry point's policy, before any crate graph loads. The first call stands, and a later
/// one only confines: a later `Open` leaves the policy as it is, so nothing after the entry point
/// opens what it closed.
pub fn set_policy(p: Policy) {
    use std::sync::atomic::Ordering::SeqCst;
    match p {
        Policy::Sandboxed => POLICY.store(SANDBOXED, SeqCst),
        Policy::Open => { let _ = POLICY.compare_exchange(0, OPEN, SeqCst, SeqCst); }
    }
}

/// A test that plays both entry points in one process (relax-compiler cache.rs, isolated_test):
/// the policy whatever was set before. No entry point calls it.
///
/// # Safety
/// The process runs no session, check or build script while the policy changes.
#[doc(hidden)]
pub unsafe fn replace_policy_in_test(p: Policy) {
    POLICY.store(if p == Policy::Open { OPEN } else { SANDBOXED }, std::sync::atomic::Ordering::SeqCst);
}

pub fn policy() -> Policy {
    if POLICY.load(std::sync::atomic::Ordering::SeqCst) == OPEN { Policy::Open } else { Policy::Sandboxed }
}

#[cfg(test)]
#[test]
fn the_first_policy_stands_and_a_later_one_only_confines() {
    assert_eq!(policy(), Policy::Sandboxed, "a process no entry point chose for is Sandboxed");
    set_policy(Policy::Open);
    set_policy(Policy::Open);
    assert_eq!(policy(), Policy::Open, "the entry point's Open stands");
    set_policy(Policy::Sandboxed);
    assert_eq!(policy(), Policy::Sandboxed, "a later call confines");
    set_policy(Policy::Open);
    assert_eq!(policy(), Policy::Sandboxed, "and a later Open does not open it again");
}
