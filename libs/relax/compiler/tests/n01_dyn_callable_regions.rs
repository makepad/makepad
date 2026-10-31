trait Apply {
    fn apply(&self, f: &dyn for<'a> Fn(&'a u32) -> &'a u32) -> u32;
    fn outer<'a>(&self, x: &'a u32, f: &dyn Fn(&'a u32) -> &'a u32) -> u32;
}
struct Receiver;
impl Apply for Receiver {
    fn apply(&self, f: &dyn Fn(&u32) -> &u32) -> u32 {
        let value = 17;
        *f(&value)
    }
    fn outer<'a>(&self, x: &'a u32, f: &dyn Fn(&'a u32) -> &'a u32) -> u32 {
        *f(x)
    }
}
fn identity(x: &u32) -> &u32 { x }
fn call<T: Apply>(receiver: &T) -> u32 { receiver.apply(&identity) }

#[test]
fn higher_ranked_dyn_callable_in_impl_signature() {
    assert_eq!(call(&Receiver), 17);
    let object: &dyn Apply = &Receiver;
    assert_eq!(object.apply(&identity), 17);
}

#[test]
fn outer_lifetime_in_dyn_callable_impl_signature() {
    let value = 29;
    assert_eq!(Receiver.outer(&value, &identity), 29);
    let object: &dyn Apply = &Receiver;
    assert_eq!(object.outer(&value, &identity), 29);
}

#[test]
fn closure_deduces_higher_ranked_object_signature() {
    let callback: &dyn for<'a> Fn(&'a u32) -> &'a u32 = &|value: &_| value;
    assert_eq!(Receiver.apply(callback), 17);
}

trait View<'a> { fn view(&self, value: &'a u32) -> &'a u32; }
trait SubView<'a>: View<'a> {}
trait Marker { fn marker(&self) -> u32; }
trait Both<'a>: Marker + View<'a> {}
impl<'a> View<'a> for Receiver {
    fn view(&self, value: &'a u32) -> &'a u32 { value }
}
impl<'a> SubView<'a> for Receiver {}
impl Marker for Receiver { fn marker(&self) -> u32 { 3 } }
impl<'a> Both<'a> for Receiver {}

#[test]
fn higher_ranked_object_upcast_prefix() {
    let sub: &dyn for<'a> SubView<'a> = &Receiver;
    let view: &dyn for<'a> View<'a> = sub;
    let value = 41;
    assert_eq!(*view.view(&value), 41);
}

#[test]
fn higher_ranked_object_upcast_vptr() {
    let both: &dyn for<'a> Both<'a> = &Receiver;
    let view: &dyn for<'a> View<'a> = both;
    let value = 43;
    assert_eq!(*view.view(&value), 43);
}

fn boxed_callback<F: FnMut(&u32) -> u32 + 'static>(f: F) -> Box<dyn FnMut(&u32) -> u32> {
    Box::new(f)
}
fn borrowed_callback<F: Fn(&u32) -> &u32>(f: &F) -> &dyn Fn(&u32) -> &u32 { f }
fn nested_callback<F: FnMut(&mut dyn FnMut(u32))>(f: &mut F) -> &mut dyn FnMut(&mut dyn FnMut(u32)) { f }

#[test]
fn generic_callable_bounds_keep_elided_binders() {
    let mut callback = boxed_callback(|x| *x + 2);
    assert_eq!(callback(&7), 9);
    assert_eq!(*borrowed_callback(&identity)(&11), 11);
    let mut total = 0;
    let mut visit = |f: &mut dyn FnMut(u32)| f(13);
    nested_callback(&mut visit)(&mut |x| total += x);
    assert_eq!(total, 13);
}
