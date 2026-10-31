#![allow(dead_code, non_upper_case_globals)]
struct Flag<const V: bool = true>;
struct Byte<const V: u8 = 7>;
struct Signed<const V: i32 = -7>;
struct Letter<const V: char = 'x'>;
fn flag(_: Flag<true>) {}
fn byte(_: Byte<7>) {}
fn signed(_: Signed<{-7}>) {}
fn letter(_: Letter<'x'>) {}
fn unwrap<const N: usize>(x: [u8; N]) -> [u8; { N }] { x }
trait Value<const N: u8> { const V: u8; }
struct X;
impl Value<1> for X { const V: u8 = 13; }
impl Value<2> for X { const V: u8 = 29; }
const FIRST: u8 = <X as Value<1>>::V;
const SECOND: u8 = <X as Value<2>>::V;
trait Val { const N: usize; }
struct Three;
struct Twice<T>(T);
impl Val for Three { const N: usize = 3; }
impl<T: Val> Val for Twice<T> { const N: usize = T::N * 2; }
const fn add(a: usize, b: usize) -> usize { a + b }
const A: usize = <Twice<Three> as Val>::N;
const B: usize = <Twice<Twice<Three>> as Val>::N;
const C: usize = add(add(1, 2), add(3, 4));
#[test] fn bool_default() { let f: Flag = Flag; flag(f); }
#[test] fn byte_default() { let b: Byte = Byte; byte(b); }
#[test] fn signed_default() { let s: Signed = Signed; signed(s); }
#[test] fn char_default() { let c: Letter = Letter; letter(c); }
#[test] fn one_block_const_param() { assert_eq!(unwrap([1, 2]), [1, 2]); }
#[test] fn assoc_const_selection() { assert_eq!(FIRST, 13); assert_eq!(SECOND, 29); }
#[test] fn nested_const_frames() { assert_eq!(A, 6); assert_eq!(B, 12); assert_eq!(C, 10); }
