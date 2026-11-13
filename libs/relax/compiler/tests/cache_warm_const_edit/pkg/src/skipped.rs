// A module the boot skips (run.py sets RELAX_LAZY_DEFER to this file, as an app's lazy
// selection skips a module its boot does not need): the loader brings it in after the boot, and a
// restored image holds it past the boot's files.
pub const SKIPPED_K: u64 = 1;

#[inline(never)]
pub fn skipped_own() -> u64 {
    SKIPPED_K
}
