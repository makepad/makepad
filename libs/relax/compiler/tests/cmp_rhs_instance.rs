// `a == b` with a non-primitive left operand: the right operand is checked against a fresh
// type (rustc op.rs check_overloaded_binop), so an instance does not make it the left type.
// `x == &W(y.as_ref())` with x: &W<Name> compares W<Name> with W<&str> through
// `impl<S1, S2> PartialEq<W<S2>> for W<S1>` (unicase's Unicode / Ascii comparison, reached by
// Scope through pulldown-cmark's UniCase<CowStr>).
struct W<S>(S);
impl<S1: AsRef<str>, S2: AsRef<str>> PartialEq<W<S2>> for W<S1> {
    fn eq(&self, other: &W<S2>) -> bool { self.0.as_ref() == other.0.as_ref() }
}
struct Name(String);
impl AsRef<str> for Name { fn as_ref(&self) -> &str { &self.0 } }
fn cmp_ref<S: AsRef<str>>(x: &W<S>, y: &S) -> bool { x == &W(y.as_ref()) }
fn cmp_val<S: AsRef<str>>(x: W<S>, y: &S) -> bool { x == W(y.as_ref()) }
#[test]
fn rhs_is_not_the_left_type() {
    assert!(cmp_ref(&W(Name("a".into())), &Name("a".into())));
    assert!(cmp_val(W(Name("a".into())), &Name("a".into())));
    assert!(!cmp_ref(&W(Name("a".into())), &Name("b".into())));
}
#[test]
fn single_impl_still_types_literals() {
    let o: Option<u64> = Some(10);
    assert!(o == Some(10));
    let v: Vec<u16> = vec![5, 6];
    assert!(v == [5, 6]);
    let s = String::from("x");
    assert!(s == "x");
}
