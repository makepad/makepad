// A macro call ending a block is an expression (rustc_parse parse_stmt_mac, rustc_expand
// flat_map_stmt `is_trailing_mac`): a `;` ending its expansion is ignored (the
// semicolon_in_expressions_from_macros lint) and the block keeps the expression's value, in a run
// as in a check (alloc-no-stdlib's `unsafe { define_allocator_memory_pool!(..) }`).
#![allow(semicolon_in_expressions_from_macros, unused_unsafe)]

struct Pool {
    data: Vec<u8>,
}

macro_rules! pool {
    ($n:expr) => {
        Pool { data: vec![0u8; $n] };
    };
}

macro_rules! sum {
    ($a:expr, $b:expr) => {
        $a + $b;
    };
}

#[test]
fn tail_macro_with_semicolon_keeps_its_value() {
    let p = unsafe { pool!(16) };
    assert_eq!(p.data.len(), 16);
    let q = { pool!(4) };
    assert_eq!(q.data.len(), 4);
    let n: u32 = { sum!(2, 3) };
    assert_eq!(n, 5);
}

fn tail_of_fn() -> u32 {
    sum!(40, 2)
}

#[test]
fn tail_macro_of_a_fn_body() {
    assert_eq!(tail_of_fn(), 42);
}
