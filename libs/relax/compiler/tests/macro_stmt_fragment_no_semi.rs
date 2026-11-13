// A `$s:stmt` fragment is a whole statement where it is substituted, and no `;` is looked for after
// it (rustc_parse parser/stmt.rs parse_full_stmt): `$s next = ..;` is two statements, and `{ $s }`
// is a block whose value is the fragment. Relax read the fragment and what followed it as one
// expression and asked for a `;` between them, so a macro that interleaves the caller's statements
// with its own failed with "expected `;` or `}` after expression (found Ident `xor`)" (relax #92).

macro_rules! interleave {
    (@seed $seed:expr; $($s:stmt;)*) => {{
        let mut xor: u32 = $seed;
        $( $s xor = xor.rotate_left(5) ^ 0x9e37_79b9; )*
        xor
    }};
}

// each kind of statement: an expression, a `let`, an item, a block-like expression, a macro call
macro_rules! count {
    ($($s:stmt;)*) => {{
        let mut n = 0u32;
        $( $s n += 1; )*
        n
    }};
}

macro_rules! value {
    ($s:stmt) => {{ $s }};
}

macro_rules! both {
    ($a:stmt, $b:stmt) => {{ $a $b }};
}

const SEED: u64 = 0x1234_5678_9abc_def0;

#[test]
fn statements_interleave_with_the_macros_own() {
    let mut v = 1u32;
    let seed1 = 7u32;
    let x = interleave! {
        @seed (SEED ^ (SEED >> 32)) as u32;
        v = v.wrapping_add(seed1);
        v = v ^ 5;
    };
    assert_eq!((v, x), (13, 2063232168));
}

#[test]
fn each_statement_ends_with_its_fragment() {
    let mut log = Vec::new();
    let n = count! {
        log.push(1);
        let k = 2;
        fn three() -> u32 { 3 };
        if k == 2 { log.push(k) } else { log.push(0) };
        log.push(three());
        assert!(log.len() == 3);
    };
    assert_eq!((n, log), (6, vec![1, 2, 3]));
}

#[test]
fn a_fragment_is_the_value_of_its_block() {
    let v = 3u32;
    let a: u32 = value!(v + 1);
    let b: u32 = both!(let w = v * 2, w + 1);
    let c: u32 = value!(match v { 3 => 10, _ => 0 });
    assert_eq!((a, b, c), (4, 7, 10));
}
