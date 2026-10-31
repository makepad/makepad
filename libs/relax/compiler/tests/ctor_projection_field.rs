// A tuple struct whose field is declared through an associated type, built with its constructor
// from an argument whose type is that projection (`W(p.into_searcher(h))` in a generic fn):
// the constructor's signature is instantiated AND normalized like any fn's, so the argument is
// passed as the concrete type (core::str::MatchIndices(P::Searcher) -- `s.replace(|c| .., "_")`
// read a garbage haystack and crashed under `makepad run`).
pub trait Pat<'a> { type S: Srch<'a>; fn into_s(self, h: &'a str) -> Self::S; }
pub trait Srch<'a> { fn hay(&self) -> &'a str; fn pos(&self) -> usize; }
pub struct PredS<'a, F> { f: F, h: &'a str, pos: usize }
impl<'a, F: FnMut(char) -> bool> Srch<'a> for PredS<'a, F> { fn hay(&self) -> &'a str { self.h } fn pos(&self) -> usize { self.pos } }
impl<'a, F: FnMut(char) -> bool> Pat<'a> for F { type S = PredS<'a, F>; fn into_s(self, h: &'a str) -> PredS<'a, F> { PredS { f: self, h, pos: 3 } } }
pub struct W<'a, P: Pat<'a>>(P::S);
pub enum E<'a, P: Pat<'a>> { One(P::S), Zero }
fn ctor<'a, P: Pat<'a>>(p: P, h: &'a str) -> W<'a, P> { W(p.into_s(h)) }
fn reified<'a, P: Pat<'a>>(p: P, h: &'a str) -> W<'a, P> { let c = W::<'a, P>; c(p.into_s(h)) }
fn variant<'a, P: Pat<'a>>(p: P, h: &'a str) -> E<'a, P> { E::One(p.into_s(h)) }

#[test]
fn ctor_with_projection_arg() {
    let w = ctor(|c: char| c == '.', "a-b.c d");
    assert_eq!(w.0.hay(), "a-b.c d");
    assert_eq!(w.0.pos(), 3);
}

#[test]
fn reified_ctor_with_projection_arg() {
    let w = reified(|c: char| c == 'b', "xyz");
    assert_eq!(w.0.hay(), "xyz");
    assert_eq!(w.0.pos(), 3);
}

#[test]
fn variant_ctor_with_projection_arg() {
    match variant(|c: char| c == 'b', "hello") {
        E::One(s) => assert_eq!((s.hay(), s.pos()), ("hello", 3)),
        E::Zero => panic!("wrong variant"),
    }
}

#[test]
fn str_replace_and_match_indices_with_closure() {
    let key = "a-b.c d";
    assert_eq!(key.replace(|c: char| !c.is_ascii_alphanumeric(), "_"), "a_b_c_d");
    assert_eq!(key.replace(char::is_whitespace, "_"), "a-b.c_d");
    let v: Vec<(usize, &str)> = key.match_indices(|c: char| c == '.').collect();
    assert_eq!(v, vec![(3, ".")]);
}
