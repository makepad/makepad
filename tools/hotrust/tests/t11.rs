// HotRust: `?`, compound assignment through op traits, deref coercions, sub-slice places,
// diverging tails.
use std::ops::{AddAssign, MulAssign};

#[derive(Clone, Copy)]
struct V2 {
    x: f64,
    y: f64,
}

impl MulAssign<f64> for V2 {
    fn mul_assign(&mut self, k: f64) {
        self.x *= k;
        self.y *= k;
    }
}

impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        self.x += o.x;
        self.y += o.y;
    }
}

struct R {
    pos: V2,
}

fn half(x: u32) -> Option<u32> {
    if x % 2 == 0 {
        Some(x / 2)
    } else {
        None
    }
}

fn quarter(x: u32) -> Option<u32> {
    let h = half(x)?;
    let q = half(h)?;
    Some(q)
}

fn checked(x: u32) -> Result<u32, u32> {
    if x > 10 {
        return Err(x);
    }
    Ok(x + 1)
}

fn twice(x: u32) -> Result<u32, u32> {
    let a = checked(x)?;
    Ok(checked(a)? * 2)
}

fn unwrap_or_zero(o: Option<u32>) -> u32 {
    match o {
        Some(v) => v,
        None => 0,
    }
}

fn is_err(r: Result<u32, u32>) -> bool {
    match r {
        Ok(_) => false,
        Err(_) => true,
    }
}

struct Holder {
    v: Vec<u32>,
    s: String,
}

impl Holder {
    fn items(&self) -> &[u32] {
        &self.v
    }
    fn text(&self) -> &str {
        &self.s
    }
}

fn never_returns() -> u32 {
    panic!("never")
}

#[test]
fn question_mark() {
    assert!(unwrap_or_zero(quarter(8)) == 2);
    assert!(unwrap_or_zero(quarter(6)) == 0);
    assert!(match twice(3) { Ok(v) => v == 10, Err(_) => false });
    assert!(is_err(twice(10)));
    assert!(is_err(twice(11)));
}

#[test]
fn op_assign_traits() {
    let mut r = R { pos: V2 { x: 1.0, y: 2.0 } };
    r.pos *= 3.0;
    r.pos += V2 { x: 0.5, y: 0.5 };
    assert!(r.pos.x == 3.5 && r.pos.y == 6.5);
}

#[test]
fn coercions_and_slices() {
    let mut v = Vec::new();
    v.push(1u32);
    v.push(2);
    v.push(3);
    let h = Holder { v, s: String::new() };
    assert!(h.items().len() == 3);
    assert!(h.text().len() == 0);
    let mut a = [1u32, 2, 3, 4];
    for x in &mut a[1..3] {
        *x += 10;
    }
    assert!(a[1] == 12 && a[2] == 13 && a[3] == 4);
    let s: &[u32] = &h.v;
    assert!(s[2] == 3);
    if false {
        never_returns();
    }
}
