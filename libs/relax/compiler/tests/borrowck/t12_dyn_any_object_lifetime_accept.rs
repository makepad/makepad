// T12 (conform): pre-existing borrowck false positive, rustc accepts; found by N01 Gandalf audit
// (round 52 OB, arch-n01-oB.REPORT.md). Known failure: not listed in run_live_regressions.sh;
// `makepad check` with MAKEPAD_RUN_BORROWCK=full (that script's settings) reports the errors named below,
// as does the pre-N01 tip (which does not check derived impl bodies, so it misses those). Do not
// list it until conform fixes the cause.
// Cause: the makepad-draw rr reflection shape (shader_access.rs 9 sites, reflect.rs 6 sites incl.
// E0499): `ShaderBorrow<'a, T: ?Sized = dyn Any>` (the default's object lifetime is 'static),
// `fn write(self) -> Option<&'a mut dyn Any>` and nodes rebuilt from a reborrow are reported as
// "'a must outlive 'static" / "'1 must outlive 'static". rustc accepts the code (cargo check
// -p makepad-draw --features tweaker is clean).
use std::any::{Any, TypeId};
pub enum ShaderBorrow<'a, T: ?Sized = dyn Any> { Read(&'a T), Write(&'a mut T) }
impl<'a> ShaderBorrow<'a> {
    pub fn type_id(&self) -> TypeId { match self { Self::Read(v) => (*v).type_id(), Self::Write(v) => (**v).type_id() } }
    pub fn read(self) -> Option<&'a dyn Any> { match self { Self::Read(v) => Some(v), Self::Write(_) => None } }
    pub fn write(self) -> Option<&'a mut dyn Any> { match self { Self::Write(v) => Some(v), Self::Read(_) => None } }
}
pub struct ShaderAccess<'a> { pub value: ShaderBorrow<'a>, pub copy: Option<fn(&mut dyn Any, &dyn Any) -> bool> }
pub struct ShaderInfo { pub base: Option<for<'a> fn(ShaderBorrow<'a>) -> ShaderNode<'a>> }
pub struct ShaderNode<'a> { pub value: ShaderBorrow<'a>, pub info: &'static ShaderInfo }
impl<'a> ShaderNode<'a> {
    pub fn access(self, _path: &str) -> Option<ShaderAccess<'a>> { Some(ShaderAccess { value: self.value, copy: None }) }
    pub fn field_mut(self, path: &str) -> Option<&'a mut dyn Any> { self.access(path)?.value.write() }
    pub fn copy_field(self, path: &str, source: &dyn Any) -> bool {
        let Some(field) = self.access(path) else { return false; };
        let Some(copy) = field.copy else { return false; };
        field.value.write().is_some_and(|target| copy(target, source))
    }
    fn copy_from(self, source: ShaderNode<'_>) {
        let Some(target) = self.value.write() else { return; };
        let Some(value) = source.value.read() else { return; };
        if let (Some(tb), Some(sb)) = (self.info.base, source.info.base) {
            tb(ShaderBorrow::Write(&mut *target)).copy_from(sb(ShaderBorrow::Read(value)));
        }
        let Some(field) = (ShaderNode { value: ShaderBorrow::Write(&mut *target), info: self.info }).access("x") else { return; };
        if let Some(copy) = field.copy { if let Some(t) = field.value.write() { copy(t, value); } }
    }
}

static INFO: ShaderInfo = ShaderInfo { base: None };
fn run() {
    let mut x = 1u32;
    let y = 2u32;
    ShaderNode { value: ShaderBorrow::Write(&mut x), info: &INFO }.copy_from(ShaderNode { value: ShaderBorrow::Read(&y), info: &INFO });
    let node = ShaderNode { value: ShaderBorrow::Write(&mut x), info: &INFO };
    assert_eq!(node.value.type_id(), TypeId::of::<u32>());
    let field = ShaderNode { value: ShaderBorrow::Write(&mut x), info: &INFO }.field_mut("x").unwrap();
    *field.downcast_mut::<u32>().unwrap() = 5;
    assert!(!ShaderNode { value: ShaderBorrow::Write(&mut x), info: &INFO }.copy_field("x", &y));
    assert_eq!(x, 5);
}
#[test]
#[ignore = "T12 conform: dyn Any default object lifetime through ShaderBorrow"]
fn dyn_any_default_object_lifetime() { run() }
fn main() { run() }
