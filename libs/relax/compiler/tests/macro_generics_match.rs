// rustc_expand mbe/macro_rules.rs: try_match_macro gives each arm a fresh
// parser. A successful fragment followed by a failed literal cannot alter
// the glued tokens seen by the next arm.
macro_rules! fallback {
    (Wrap<$t:ty> ; reject) => { 0 };
    ($($t:tt)*) => { original_tokens!($($t)*) };
}
macro_rules! original_tokens {
    (Wrap < Option < u8 >> ; accept) => { 1 };
    (Wrap < Option < Option < u8 >>> ; accept) => { 2 };
}

macro_rules! equals_fallback {
    ($t:ty = reject) => { 0 };
    (Option < u8 >= accept) => { 3 };
    (Option < Option < u8 >>= accept) => { 4 };
}

#[test]
fn failed_arms_restore_split_tokens() {
    assert_eq!(fallback!(Wrap<Option<u8>>; accept), 1);
    assert_eq!(fallback!(Wrap<Option<Option<u8>>>; accept), 2);
    assert_eq!(equals_fallback!(Option<u8>=accept), 3);
    assert_eq!(equals_fallback!(Option<Option<u8>>=accept), 4);
}

// rustc_expand mbe/quoted.rs: parse_tree/parse_sep_and_kleene_op;
// mbe/macro_parser.rs: match_one handles nested * and ? repetitions.
// A typewit-style signature, without requiring the unavailable crate.
macro_rules! generics {
    (impl<$($lt:lifetime,)* $k:ident: $bound:ident $(+ ?$sized:ident)?>
        $arg:ident => <$q:ident as $tr:ident>::$assoc:ident<$t:ident>) => {
        (0 $(+ { let _ = stringify!($lt); 1 })*,
         0 $(+ { let _ = stringify!($sized); 1 })?)
    };
}
macro_rules! forward {
    ($($t:tt)*) => { generics!($($t)*) };
}

#[test]
fn lifetime_bounds_and_optional_unsized_bound() {
    assert_eq!(forward!(impl<K: Kind> K => <K as Kind>::To<T>), (0, 0));
    assert_eq!(forward!(impl<'a, 'b, K: Kind + ?Sized> K => <K as Kind>::To<T>), (2, 1));
}

// rustc_parse parser/mod.rs: parse_token_tree and rustc_expand
// mbe/macro_parser.rs: token_name_eq keep glued punctuation whole for tt/literals.
macro_rules! tokens {
    ($($t:tt)*) => { 0 $(+ { let _ = stringify!($t); 1 })* };
}
macro_rules! glued {
    (<<) => { 1 };
    (< <) => { 2 };
    (::) => { 3 };
    (: :) => { 4 };
}

#[test]
fn token_trees_and_literal_matchers_preserve_glue() {
    assert_eq!(tokens!(:: << >> >>= >= <- ?Sized 'a :), 10);
    assert_eq!(glued!(<<), 1);
    assert_eq!(glued!(< <), 2);
    assert_eq!(glued!(::), 3);
    assert_eq!(glued!(: :), 4);
}
