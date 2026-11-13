// rejects: E0308
// A `$s:stmt` fragment of one token is a statement of its own too (rustc_parse parser/stmt.rs
// parse_full_stmt): `$a $b` with `v` and `-1` is the statement `v`, an `i32` that is not the
// block's value and not discarded (rustc: E0308, expected `()`, found `i32`), then the value
// `-1`. Relax kept no bounds for a fragment of one token and read `v - 1`.
macro_rules! both {
    ($a:stmt, $b:stmt) => {{ $a $b }};
}

pub fn f(v: i32) -> i32 {
    both!(v, -1)
}
