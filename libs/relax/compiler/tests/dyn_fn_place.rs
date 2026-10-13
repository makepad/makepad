// F36: calls on any place of type dyn Fn* (through a raw-pointer deref too) go through the
// vtable; fn pointers are Clone (builtin impls)
struct S { k: u32 }
impl S {
    fn run(&self, g: *const dyn Fn(&S, u32) -> u32, x: u32) -> u32 { unsafe { (*g)(self, x) } }
}
#[test]
fn raw_dyn_fn_call() {
    let add = 5u32;
    let f: Box<dyn Fn(&S, u32) -> u32> = Box::new(move |s: &S, x: u32| s.k * x + add);
    let p: *const dyn Fn(&S, u32) -> u32 = &*f;
    let s = S { k: 3 };
    assert_eq!(s.run(p, 2), 11);
    assert_eq!((*f)(&s, 1), 8);
    let r: &dyn Fn(&S, u32) -> u32 = &*f;
    assert_eq!((*r)(&s, 0), 5);
    let bm: Box<dyn FnMut(u32) -> u32> = Box::new(move |x: u32| x + add);
    let m: *mut dyn FnMut(u32) -> u32 = Box::into_raw(bm);
    assert_eq!(unsafe { (*m)(1) }, 6);
    unsafe { drop(Box::from_raw(m)) };
}
fn tw(x: u32) -> u32 { x * 2 }
#[test]
fn fn_ptr_clone() {
    let f: fn(u32) -> u32 = tw;
    let g = f.clone();
    let v = vec![f, g];
    let mut w: Vec<fn(u32) -> u32> = Vec::new();
    w.extend_from_slice(&v);
    assert_eq!(w[1](4), 8);
}
