// rustc forwards `$s:stmt` as an opaque NtStmt, including through `$t:tt` and into
// generated function bodies. Its extent is retained even when it is a macro call.
macro_rules! finish { ($s:stmt) => {{ $s 7u32 }}; }
macro_rules! through_tt { ($t:tt) => { finish!($t) }; }
macro_rules! through_stmt { ($s:stmt) => { through_tt!($s) }; }
macro_rules! erase { () => {}; }
macro_rules! discard_empty { ($s:stmt) => {{ $s; }}; }

#[test]
fn forwarding_through_tt_keeps_the_statement_boundary() {
    let mut n = 0;
    assert_eq!(through_stmt!(n += 1), 7);
    assert_eq!(n, 1);
}

#[test]
fn an_empty_statement_macro_keeps_the_following_tail() {
    assert_eq!(finish!(erase!()), 7);
    let unit: () = discard_empty!(erase!());
    assert_eq!(unit, ());
}

#[test]
fn a_statement_macro_can_expand_to_several_statements() {
    macro_rules! add_twice { ($n:ident) => { $n += 1; $n += 2; }; }
    let mut n = 0;
    assert_eq!(finish!(add_twice!(n)), 7);
    assert_eq!(n, 3);
}

#[test]
fn statement_attributes_remain_inside_the_boundary() {
    let mut n = 0;
    assert_eq!(finish!(#[cfg(any())] let _ = { n += 1; }), 7);
    assert_eq!(n, 0);
    n += 1;
    assert_eq!(n, 1);
}

#[test]
fn a_forwarded_statement_does_not_match_a_written_literal() {
    macro_rules! classify { (7) => { false }; ($s:stmt) => { true }; }
    macro_rules! forward { ($s:stmt) => { classify!($s) }; }
    assert!(forward!(7));
}

#[test]
fn a_forwarded_statement_is_not_an_expression_or_type_fragment() {
    macro_rules! classify { ($e:expr) => { 0 }; ($t:ty) => { 1 }; ($s:stmt) => { 2 }; }
    macro_rules! forward { ($s:stmt) => { classify!($s) }; }
    assert_eq!(forward!(7), 2);
}

macro_rules! generated_function {
    ($name:ident, $value:ident, $s:stmt) => {
        fn $name() -> u32 {
            let mut $value = 0u32;
            $s
            $value
        }
    };
}
generated_function!(generated, value, value += 3);

#[test]
fn generated_function_bodies_retain_the_statement_boundary() {
    assert_eq!(generated(), 3);
}
