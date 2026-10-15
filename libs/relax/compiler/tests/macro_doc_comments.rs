// Doc comments in a macro_rules call's input are matched as rustc desugars them:
// `/// x` is `#[doc = r"x"]`, `//! x` is `#![doc = r"x"]` (rustc_ast tokenstream.rs:698
// `desugar_doc_comments`, called by rustc_expand mbe/macro_rules.rs:1808 `parser_from_cx`),
// the raw string with as many `#` as the text needs (tokenstream.rs:736-753).
// syn's token.rs:521 `define_punctuation_structs!` matches `#[doc = $usage:literal]`.

macro_rules! doc_literal {
    ($(#[doc = $x:literal])* $name:ident) => {
        const $name: &[&str] = &[$($x),*];
    };
}
doc_literal! {
    /// a
    /// b "quoted"
    /// c "#hash
    DOCS
}

macro_rules! doc_meta {
    ($(#[$m:meta])* struct $name:ident;) => {
        $(#[$m])* struct $name;
        impl $name {
            fn count() -> usize { [$(stringify!($m)),*].len() }
        }
    };
}
doc_meta! {
    /// one
    /// two
    struct WithDocs;
}

// tt passthrough: an inner doc is `# ! [doc = ..]` and is emitted as the attribute
macro_rules! pass {
    ($($t:tt)*) => { $($t)* };
}
macro_rules! module {
    ($name:ident { $($t:tt)* }) => { mod $name { $($t)* } };
}
module! {
    inside {
        //! inner doc, forwarded as `#![doc = r" inner doc..."]`
        pub fn after_inner() -> i32 { 7 }
    }
}
pass! {
    /// outer doc through tt
    fn after_outer() -> i32 { 8 }
}

macro_rules! single_tt {
    (#[doc = $d:literal] fn $f:ident) => { fn $f() -> &'static str { $d } };
}
single_tt! {
    /// plain
    fn plain_doc
}
single_tt! {
    /** block "doc" */
    fn block_doc
}

#[test]
fn doc_comments_desugar() {
    assert_eq!(DOCS, [" a", " b \"quoted\"", " c \"#hash"]);
    assert_eq!(WithDocs::count(), 2);
    assert_eq!(inside::after_inner(), 7);
    assert_eq!(after_outer(), 8);
    assert_eq!(plain_doc(), " plain");
    assert_eq!(block_doc(), " block \"doc\" ");
}
