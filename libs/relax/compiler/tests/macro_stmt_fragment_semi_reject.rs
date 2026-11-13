// rejects: E0308
// A `$s:stmt` fragment ends its statement, so the `;` written after one is an empty statement of
// its own (rustc_parse parser/stmt.rs parse_full_stmt), not the end of an expression statement:
// the fragment's `v + 1` is no longer the block's value and is not discarded either, so it must be
// `()` (rustc: E0308, expected `()`, found `u32`). Relax took the `;` as the fragment's own.
macro_rules! discard {
    ($s:stmt) => {{ $s; }};
}

pub fn f(v: u32) {
    discard!(v + 1)
}
