// rustc_builtin_macros util.rs: get_exprs_from_tts / expr_to_spanned_string expand each
// argument fully before concat.rs: expand_concat folds its literal.
macro_rules! stringify_outputs {
    (@inner $first:ty) => { concat!("or `", stringify!($first), "`") };
    (@inner $first:ty, $($t:ty),+) => {
        concat!(stringify_outputs!($first), ", ", stringify_outputs!(@inner $($t),+))
    };
    ($first:ty) => { concat!("`", stringify!($first), "`") };
    ($($t:ty),+) => { stringify_outputs!(@inner $($t),+) };
}

macro_rules! relay {
    ($($t:tt)*) => { leaf!($($t)*) };
}
macro_rules! leaf {
    () => { "literal" };
    (concat) => { concat!("nested ", relay!()) };
    (env) => { "PATH" };
    (source) => { "macro_concat_nested.rs" };
    (format) => { "value: {}" };
}

const OUTPUTS: &str = concat!("returns ", stringify_outputs!(u8, u16, u32, f32, f64));
const CHAIN: &str = concat!("a ", relay!());
const NESTED: &str = concat!("a ", relay!(concat));

#[doc = concat!("Returns ", stringify_outputs!(u8, u16, f32), ".")]
fn documented() -> &'static str { OUTPUTS }

#[doc = relay!(concat)]
fn documented_chain() -> &'static str { NESTED }

macro_rules! unit {
    ($t:ty, $larger:ty, [$($int_output:ty),+], [$($float_output:ty),+]) => {
        #[doc = concat!("  - `", stringify!($t), "::per(", stringify!($larger),
            ")` (returns ", stringify_outputs!($($int_output),+, $($float_output),+), ")")]
        fn generated_doc() -> &'static str { stringify_outputs!($($int_output),+, $($float_output),+) }
    };
}
unit!(u8, u16, [u8, u16], [f32, f64]);

#[test]
fn recursive_outputs_are_literals() {
    assert_eq!(OUTPUTS, "returns `u8`, `u16`, `u32`, `f32`, or `f64`");
}

#[test]
fn nested_attribute_values_expand() {
    assert_eq!(documented(), OUTPUTS);
    assert_eq!(documented_chain(), "a nested literal");
    assert_eq!(generated_doc(), "`u8`, `u16`, `f32`, or `f64`");
}

#[test]
fn macro_to_macro_to_literal() {
    assert_eq!(CHAIN, "a literal");
    assert_eq!(concat!(relay!(), relay!(),), "literalliteral");
}

#[test]
fn macro_to_concat_to_literal() {
    assert_eq!(NESTED, "a nested literal");
    assert_eq!(concat!(concat!(relay!()), "!"), "literal!");
}

#[test]
fn other_eager_arguments() {
    assert_eq!(env!(concat!(relay!(env)), relay!()), env!("PATH"));
    assert_eq!(option_env!(relay!(env)), option_env!("PATH"));
    assert_eq!(include_str!(relay!(source)), include_str!("macro_concat_nested.rs"));
    assert_eq!(&include_bytes!(relay!(source))[..], include_str!(relay!(source)).as_bytes());
    assert_eq!(format!(concat!(relay!(format)), 7), "value: 7");
}

#[test]
fn stringify_keeps_its_tokens() {
    assert_eq!(concat!(stringify!(undefined!())), "undefined!()");
}
