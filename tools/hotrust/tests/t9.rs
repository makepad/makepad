// HotRust trait objects: &dyn / Box<dyn> method calls through vtables, default methods,
// supertrait methods, dyn Fn callables, drops through the vtable.

trait Named {
    fn name_len(&self) -> usize;
}

trait Shape: Named {
    fn area(&self) -> f64;
    fn scale(&mut self, k: f64);
    fn double_area(&self) -> f64 {
        self.area() * 2.0
    }
}

struct Rect {
    w: f64,
    h: f64,
}

struct Circle {
    r: f64,
}

impl Named for Rect {
    fn name_len(&self) -> usize {
        4
    }
}

impl Named for Circle {
    fn name_len(&self) -> usize {
        6
    }
}

impl Shape for Rect {
    fn area(&self) -> f64 {
        self.w * self.h
    }
    fn scale(&mut self, k: f64) {
        self.w *= k;
        self.h *= k;
    }
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        3.0 * self.r * self.r
    }
    fn scale(&mut self, k: f64) {
        self.r *= k;
    }
    fn double_area(&self) -> f64 {
        -1.0
    }
}

fn total(shapes: &[&dyn Shape]) -> f64 {
    let mut t = 0.0;
    let mut i = 0;
    while i < shapes.len() {
        t += shapes[i].area();
        i += 1;
    }
    t
}

static mut DROPS: i64 = 0;

struct Noisy {
    v: i64,
}

impl Drop for Noisy {
    fn drop(&mut self) {
        unsafe { DROPS += self.v };
    }
}

impl Named for Noisy {
    fn name_len(&self) -> usize {
        self.v as usize
    }
}

fn call_twice(f: &dyn Fn(i64) -> i64, x: i64) -> i64 {
    f(f(x))
}

fn sq(x: i64) -> i64 {
    x * x
}

#[test]
fn trait_objects() {
    let r = Rect { w: 2.0, h: 3.0 };
    let c = Circle { r: 1.0 };
    assert_eq!(total(&[&r, &c]), 9.0);
    let s: &dyn Shape = &r;
    assert_eq!(s.double_area(), 12.0);
    assert_eq!(s.name_len(), 4);
    let s2: &dyn Shape = &c;
    assert_eq!(s2.double_area(), -1.0);
    assert_eq!(s2.name_len(), 6);
    let mut m = Rect { w: 1.0, h: 1.0 };
    {
        let ms: &mut dyn Shape = &mut m;
        ms.scale(3.0);
        assert_eq!(ms.area(), 9.0);
    }
    assert_eq!(m.w, 3.0);
}

#[test]
fn boxed() {
    let mut b: Box<dyn Shape> = Box::new(Rect { w: 4.0, h: 5.0 });
    assert_eq!(b.area(), 20.0);
    b.scale(0.5);
    assert_eq!(b.area(), 5.0);
    {
        let n: Box<dyn Named> = Box::new(Noisy { v: 7 });
        assert_eq!(n.name_len(), 7);
    }
    assert_eq!(unsafe { DROPS }, 7);
}

#[test]
fn dyn_fn() {
    let k = 3;
    let add = |x: i64| x + k;
    assert_eq!(call_twice(&add, 1), 7);
    assert_eq!(call_twice(&sq, 3), 81);
    let f: Box<dyn Fn(i64) -> i64> = Box::new(move |x: i64| x * k);
    assert_eq!(f(5), 15);
    let g: Box<dyn Fn(i64) -> i64> = Box::new(sq);
    assert_eq!(g(6), 36);
    let mut acc = 0;
    {
        let mut h: Box<dyn FnMut(i64)> = Box::new(|x: i64| acc += x);
        h(4);
        h(5);
    }
    assert_eq!(acc, 9);
}
