// rustc_hir_analysis/collect/resolve_bound_vars.rs + rustc_resolve/late.rs @8d1a764:
// object lifetime defaults through fn pointers and type-relative paths, `Self`
// elision only for type constructors, inferred expression/pattern path lifetimes.
trait Tr { fn value(&self) -> u8; }
impl Tr for u8 { fn value(&self) -> u8 { *self } }

// resolve_bound_vars.rs:720-749: `dyn Tr` inside `&'a fn(..)` defaults to 'a.
fn through_pointer<'a>(f: &'a fn(*const dyn Tr) -> u8, p: *const (dyn Tr + 'a)) -> u8 { f(p) }
fn read<'r>(p: *const (dyn Tr + 'r)) -> u8 { unsafe { (*p).value() } }

// late.rs:2730-2745: a type alias is not `Self` for elision; the output takes x's lifetime.
struct Holder(u8);
type Alias = Holder;
impl Alias {
    fn pick(self: &Alias, x: &u8) -> &u8 { let _ = self.0; x }
}

// resolve_bound_vars.rs:904-944: `Wrap::<'a, dyn Tr>::new` keeps Wrap's `T: 'x` default.
struct Wrap<'x, T: ?Sized + 'x>(&'x T);
impl<'x, T: ?Sized + 'x> Wrap<'x, T> {
    fn new(value: &'x T) -> Self { Wrap(value) }
}

// late.rs:2271-2296: pattern paths infer their implicit lifetimes (closure parameters too).
struct W<'a>(&'a u8);

#[test]
fn fn_pointer_object_default() {
    let value = 9u8;
    let f: fn(*const dyn Tr) -> u8 = read;
    assert_eq!(through_pointer(&f, &value as &dyn Tr as *const dyn Tr), 9);
}
#[test]
fn alias_self_elision() {
    let holder = Holder(1);
    let result;
    { let x = 4u8; result = *holder.pick(&x); }
    assert_eq!(result, 4);
}
#[test]
fn type_relative_prefix_default() {
    let value = 3u8;
    let wrap = Wrap::<'_, dyn Tr>::new(&value);
    assert_eq!(wrap.0.value(), 3);
}
#[test]
fn closure_pattern_path() {
    let value = 6u8;
    let read = |W(x): W<'_>| *x;
    assert_eq!(read(W(&value)), 6);
}
