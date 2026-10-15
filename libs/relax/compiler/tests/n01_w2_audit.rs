// rustc_mir_transform/src/shim.rs:662-702; rustc_hir_typeck/src/callee.rs:240.
use std::sync::atomic::{AtomicUsize, Ordering};
static DROPS: AtomicUsize = AtomicUsize::new(0);
struct Field(bool);
impl Clone for Field {
    fn clone(&self) -> Self {
        if self.0 { panic!("second clone"); }
        Field(false)
    }
}
impl Drop for Field {
    fn drop(&mut self) { DROPS.fetch_add(1, Ordering::SeqCst); }
}
#[test]
fn clone_shim_unwinds_initialized_fields() {
    DROPS.store(0, Ordering::SeqCst);
    let fields = (Field(false), Field(true));
    let result = std::panic::catch_unwind(|| fields.clone());
    assert!(result.is_err());
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    drop(fields);
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);
    DROPS.store(0, Ordering::SeqCst);
    let fields = (Field(true), Field(false));
    assert!(std::panic::catch_unwind(|| fields.clone()).is_err());
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    drop(fields);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    DROPS.store(0, Ordering::SeqCst);
    let fields = (Field(false), Field(false));
    let cloned = fields.clone();
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    drop(cloned);
    drop(fields);
    assert_eq!(DROPS.load(Ordering::SeqCst), 4);
}
#[test]
fn callable_obligations_before_coercion() {
    fn invoke<F: Fn(&str) -> usize>(f: F) -> usize { f("abc") }
    let ptr: fn(&str) -> usize = str::len;
    assert_eq!(invoke(ptr), 3);
    assert_eq!(invoke(|s| s.len()), 3);
}

#[test]
fn higher_ranked_pointer_reborrow_uses_subtyping() {
    fn ignore(_: &u8) {}
    let mut f: for<'a> fn(&'a u8) = ignore;
    let r: &fn(&'static u8) = &mut f;
    r(&1);
    let raw: *const fn(&'static u8) = &mut f;
    unsafe { (*raw)(&2); }
    f(&3);
}
#[test]
fn comparison_obligations_infer_rhs() {
    assert_eq!(Some(5u64), Some(5));
    let x = vec![5u16, 6];
    assert!(x == [5, 6]);
}
trait Project { type Item: AsRef<str>; fn item(&self) -> Self::Item; }
impl Project for () { type Item = String; fn item(&self) -> String { String::from("native") } }
fn projected<T: Project>(value: T) -> usize { value.item().as_ref().len() }
#[test]
fn rigid_projection_uses_item_bounds() { assert_eq!(projected(()), 6); }
#[test]
fn opaque_deref_uses_native_selection() {
    fn value() -> impl std::ops::Deref<Target = str> { String::from("opaque") }
    assert_eq!((&*value()).len(), 6);
}

#[track_caller]
fn tracked_location() -> &'static std::panic::Location<'static> { std::panic::Location::caller() }
fn through_fn<F: Fn() -> &'static std::panic::Location<'static>>(f: F) -> &'static std::panic::Location<'static> { f() }
#[test]
fn fn_item_shim_uses_definition_location() {
    let location = through_fn(tracked_location);
    assert!(location.line() > 1);
    // The shim uses the Fn trait definition span (shim.rs:845), whose std
    // source filename is not part of the contract. Neither this call site nor
    // tracked_location's definition is that span.
    assert_ne!(location.file(), file!());
    let again = through_fn(tracked_location);
    assert_eq!(location.file(), again.file());
    assert_eq!(location.line(), again.line());
    assert_eq!(location.column(), again.column());
}

trait DefaultBody {
    fn produce(&self) -> impl std::fmt::Display { 5u8 }
    fn borrowed<'a>(&self, value: &'a str) -> impl std::fmt::Display + 'a { value }
}
impl DefaultBody for () {}
#[test]
fn default_trait_body_defines_its_opaque() {
    assert_eq!(format!("{}", ().produce()), "5");
    assert_eq!(format!("{}", ().borrowed("borrowed")), "borrowed");
}
