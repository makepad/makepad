use std::borrow::Borrow;
use std::collections::HashSet;
#[derive(PartialEq, Eq, Hash)]
struct Frame(u64);
impl Frame { fn present(&self, frames: &HashSet<Frame>) -> bool { if frames.contains(&self) { true } else { false } } }
#[test]
fn expected_bool_retains_borrow_constraint() {
    let mut frames = HashSet::new(); frames.insert(Frame(3));
    assert!(Frame(3).present(&frames));
}
struct Shader { active: bool }
fn any_active(shaders: &[impl Borrow<Shader>]) -> bool { shaders.iter().map(Borrow::borrow).any(|shader| shader.active) }
#[test]
fn field_lookup_resolves_callable_obligations() { assert!(any_active(&[Shader { active: true }])); }
fn read<T>(value: &T) -> usize { std::mem::size_of_val(value) }
fn borrow_in_closure<'a>(values: &'a [String]) -> usize {
    values.iter().map(|value| read(&value)).sum()
}
#[test]
fn summary_body_has_its_own_region_context() {
    assert_eq!(borrow_in_closure(&[String::from("a")]), std::mem::size_of::<&String>());
}
#[test]
fn instantiated_body_retains_declared_borrow_target() {
    let shaders = [std::sync::Arc::new(Shader { active: true })];
    assert!(any_active(&shaders));
}
