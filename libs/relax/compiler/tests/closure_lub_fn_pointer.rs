struct C { a: f32, b: f32 }
fn pick(display: bool) -> f32 {
    let f = if display { |c: &C| c.a } else { |c: &C| c.b };
    f(&C { a: 1.0, b: 2.0 })
}
fn pick_match(k: u8) -> i32 {
    let f = match k { 0 => |x: i32| x + 1, 1 => |x: i32| x * 2, _ => |x: i32| -x };
    f(10)
}
#[test]
fn closure_lub_to_fn_pointer() {
    assert_eq!(pick(true), 1.0);
    assert_eq!(pick(false), 2.0);
    assert_eq!(pick_match(0), 11);
    assert_eq!(pick_match(1), 20);
    assert_eq!(pick_match(5), -10);
}

#[test]
fn closure_lub_adjusts_all_earlier_arms() {
    for k in 0..4 {
        let first = |x: i32| x + 1;
        let f = match k {
            0 => first,
            1 => first,
            2 => |x: i32| x * 2,
            _ => |x: i32| -x,
        };
        assert_eq!(f(10), [11, 11, 20, -10][k]);
    }
}

#[test]
fn same_capturing_closure_keeps_its_type() {
    let offset = 7;
    let captured = |x: i32| x + offset;
    for c in [true, false] {
        let f = if c { captured } else { captured };
        assert_eq!(f(10), 17);
    }
}
