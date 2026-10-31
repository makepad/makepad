// Operators on references to primitives (F29): std implements the operator traits for `&prim`
// (`impl Not for &bool`, `impl Add<i32> for &i32`, `impl Neg for &i64`, ...), so the operand
// auto-dereferences. rustc: all pass.
fn first_false(v: &[bool]) -> usize {
    let mut i = 0;
    for o in v.iter() {
        if !o {
            return i;
        }
        i += 1;
    }
    v.len()
}

#[test]
fn not_on_ref_bool() {
    assert!(first_false(&[true, false]) == 1);
    let b = true;
    let r = &b;
    assert!(!r == false);
}

#[test]
fn unary_on_refs() {
    let x: i64 = 5;
    let y: u32 = 0b1010;
    let f: f32 = 1.5;
    assert!(-&x == -5);
    assert!(!&y == !0b1010u32);
    assert!(-&f == -1.5);
}

#[test]
fn binary_on_refs() {
    let a: i32 = 7;
    let b: i32 = 3;
    let f: f32 = 2.0;
    let g: f32 = 4.0;
    assert!(&a + b == 10);
    assert!(a - &b == 4);
    assert!(&a * &b == 21);
    assert!(&f * &g == 8.0);
    assert!((&a & 1) == 1);
    let v = [1u32, 2, 3];
    let s: u32 = v.iter().fold(0, |acc, x| acc + x);
    assert!(s == 6);
}
