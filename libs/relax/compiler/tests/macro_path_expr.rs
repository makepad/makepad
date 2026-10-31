// rustc_expand mbe/transcribe.rs: transcribe_pnr keeps MetaVarKind::Path.
// rustc_parse parser/expr.rs: parse_expr_bottom reparses it with PathStyle::Type.
fn parse<T>(parser: fn() -> T, attr: u8) -> (T, u8) {
    (parser(), attr)
}

macro_rules! m {
    ($parser:path) => { parse($parser, 7) };
}

#[test]
fn generic_parser_is_one_call_argument() {
    let (v, attr) = m!(Vec<u8>::new);
    assert!(v.is_empty());
    assert_eq!(attr, 7);
    let (map, attr) = m!(std::collections::HashMap<u8, Vec<u8>>::new);
    assert!(map.is_empty());
    assert_eq!(attr, 7);
}

macro_rules! relay {
    ($parser:tt) => { parse($parser, 9) };
}

macro_rules! forward {
    ($parser:path) => { relay!($parser) };
}

macro_rules! recapture {
    ($parser:ty) => { m!($parser) };
}

macro_rules! make_fn {
    ($name:ident, $parser:path) => {
        fn $name() -> Vec<u8> { $parser() }
    };
}

make_fn!(empty_vec, Vec<u8>::new);

#[test]
fn forwarded_paths_keep_their_kind() {
    let (v, attr) = forward!(Vec<u8>::new);
    assert!(v.is_empty());
    assert_eq!(attr, 9);
    let (map, attr) = forward!(std::collections::HashMap<u8, Vec<u8>>::new);
    assert!(map.is_empty());
    assert_eq!(attr, 9);
    // rustc_parse parser/path.rs: parse_path_inner accepts a path-shaped Ty metavar.
    let (v, attr) = recapture!(Vec<u8>::new);
    assert!(v.is_empty());
    assert_eq!(attr, 7);
    assert!(empty_vec().is_empty());
}

struct Pair<T, U>(T, U);
struct Record<T> { value: T }

macro_rules! call {
    ($p:path, $a:expr, $b:expr) => { $p($a, $b) };
}

#[test]
fn call_parentheses_follow_the_fragment() {
    let pair = call!(Pair<u8, u16>, 3, 5);
    assert_eq!(pair.0, 3);
    assert_eq!(pair.1, 5);
    let pair = call!(Pair, 8u8, 13u16);
    assert_eq!(pair.0, 8);
    assert_eq!(pair.1, 13);
}

// rustc_parse parser/pat.rs: parse_pat_with_range_pat accepts a captured path
// before tuple/struct fields; can_be_ident_pat does not treat it as a binding.
macro_rules! tuple_pat {
    ($p:path, $value:expr) => {{
        let $p(a, b) = $value;
        (a, b)
    }};
}

macro_rules! struct_pat {
    ($p:path, $value:expr) => {{
        let $p { value } = $value;
        value
    }};
}

macro_rules! path_pat {
    ($p:path, $value:expr) => {
        match $value { $p => true, _ => false }
    };
}

#[test]
fn paths_in_patterns_keep_generic_arguments() {
    assert_eq!(tuple_pat!(Pair<u8, u16>, Pair(3, 5)), (3, 5));
    assert_eq!(struct_pat!(Record<u8>, Record { value: 8 }), 8);
    assert!(path_pat!(Option<u8>::None, None));
    assert!(!path_pat!(Option<u8>::None, Some(1)));
    const ZERO: u8 = 0;
    assert!(path_pat!(ZERO, 0));
    assert!(!path_pat!(ZERO, 1));
}

macro_rules! ty_new {
    ($t:ty) => { <$t>::new() };
}

macro_rules! path_new {
    ($p:path) => { <$p>::new() };
}

#[test]
fn qualified_type_and_path_prefixes() {
    let v = ty_new!(Vec<u8>);
    assert!(v.is_empty());
    let map = ty_new!(std::collections::HashMap<u8, Vec<u8>>);
    assert!(map.is_empty());
    let v = path_new!(Vec<u8>);
    assert!(v.is_empty());
}

// rustc_parse parser/expr.rs: parse_expr_bottom returns the captured path
// before parse_expr_path_start can consume a struct literal or a `::` suffix.
// Probe matching so the negative cases remain rustc --test compatible.
macro_rules! whole_expr {
    ($e:expr) => { true };
    ($($t:tt)*) => { false };
}

macro_rules! path_suffixes {
    ($p:path) => { (whole_expr!($p()), whole_expr!($p {}), whole_expr!($p::new())) };
}

macro_rules! ty_suffixes {
    ($t:ty) => { (whole_expr!(<$t>::new()), whole_expr!($t::new())) };
}

#[test]
fn only_expression_suffixes_extend_path_expressions() {
    assert_eq!(path_suffixes!(Vec<u8>), (true, false, false));
    assert_eq!(path_suffixes!(Record<u8>), (true, false, false));
    assert_eq!(ty_suffixes!(Vec<u8>), (true, false));
}
