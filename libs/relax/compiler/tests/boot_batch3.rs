// Boot batch 3: impl chosen from trait args (B20), expected inputs from the expected output and
// closure signatures from `dyn Fn` (B22/B32), closure bodies as coercion sites, a supertrait's
// associated type through `Self::` (B31). rustc: all pass.
use std::ops::{Index, IndexMut};
use std::sync::Arc;

// B20: String implements Extend<char>, Extend<&char>, Extend<&str>, ...
#[test]
fn impl_by_trait_arg() {
    let mut s = String::from("a");
    let v = vec!["b", "c"];
    s.extend(v.iter().map(|x| *x));
    s.extend(['d'].iter());
    assert!(s == "abcd");
}

// B22: `Some(Arc::new(|| ..))` into `Option<Arc<dyn Fn()>>`
static mut HITS: u32 = 0;
fn hit() {
    unsafe { HITS += 1 };
}
fn call(f: Option<Arc<dyn Fn()>>) {
    if let Some(f) = f {
        f();
    }
}

#[test]
fn expected_inputs_through_ctor() {
    let g: Option<Arc<dyn Fn()>> = Some(Arc::new(|| {
        hit();
    }));
    call(g);
    let b: Box<dyn Fn() -> u32> = Box::new(|| 7);
    let h = unsafe { HITS };
    assert!(h + b() == 8);
}

// B32: closure parameters from `Box<dyn Fn(&mut C, u32) -> u32>`, through a closure body that
// coerces `Box<closure>` to the boxed trait object
struct C {
    v: u32,
}

#[test]
fn closure_sig_from_dyn() {
    let mut g: Vec<Box<dyn Fn(&mut C, u32) -> u32>> = Vec::new();
    g.resize_with(2, || Box::new(|c, x| c.v + x));
    let b: Box<dyn Fn(&C) -> u32> = Box::new(|c| c.v * 2);
    let mut c = C { v: 3 };
    assert!(g[1](&mut c, 4) + b(&c) == 13);
}

// B31: `Self::Output` in `impl IndexMut` is Index's associated type
struct V {
    d: Vec<u32>,
}
impl Index<usize> for V {
    type Output = u32;
    fn index(&self, i: usize) -> &u32 {
        &self.d[i]
    }
}
impl IndexMut<usize> for V {
    fn index_mut(&mut self, i: usize) -> &mut Self::Output {
        &mut self.d[i]
    }
}

#[test]
fn supertrait_assoc_through_self() {
    let mut v = V { d: vec![5, 6] };
    v[1] += 1;
    assert!(v[0] + v[1] == 12);
}
