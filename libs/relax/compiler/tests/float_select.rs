// Bit-exact selects: unordered comparisons and signed-zero ties must keep the chosen operand.
#[inline(never)]
fn select_f32(a: f32, b: f32, kind: u32) -> f32 {
    match kind {
        0 => {
            if a > b {
                a
            } else {
                b
            }
        }
        1 => {
            if a > b {
                b
            } else {
                a
            }
        }
        2 => {
            if b > a {
                a
            } else {
                b
            }
        }
        3 => {
            if b > a {
                b
            } else {
                a
            }
        }
        4 => {
            if a < b {
                a
            } else {
                b
            }
        }
        5 => {
            if a < b {
                b
            } else {
                a
            }
        }
        6 => {
            if b < a {
                a
            } else {
                b
            }
        }
        7 => {
            if b < a {
                b
            } else {
                a
            }
        }
        8 => {
            if a >= b {
                a
            } else {
                b
            }
        }
        9 => {
            if a >= b {
                b
            } else {
                a
            }
        }
        10 => {
            if b >= a {
                a
            } else {
                b
            }
        }
        11 => {
            if b >= a {
                b
            } else {
                a
            }
        }
        12 => {
            if a <= b {
                a
            } else {
                b
            }
        }
        13 => {
            if a <= b {
                b
            } else {
                a
            }
        }
        14 => {
            if b <= a {
                a
            } else {
                b
            }
        }
        15 => {
            if b <= a {
                b
            } else {
                a
            }
        }
        16 => {
            if a == b {
                a
            } else {
                b
            }
        }
        17 => {
            if a == b {
                b
            } else {
                a
            }
        }
        18 => {
            if b == a {
                a
            } else {
                b
            }
        }
        19 => {
            if b == a {
                b
            } else {
                a
            }
        }
        20 => {
            if a != b {
                a
            } else {
                b
            }
        }
        21 => {
            if a != b {
                b
            } else {
                a
            }
        }
        22 => {
            if b != a {
                a
            } else {
                b
            }
        }
        23 => {
            if b != a {
                b
            } else {
                a
            }
        }
        _ => {
            if kind & 1 != 0 {
                a
            } else {
                b
            }
        }
    }
}
#[test]
fn bits_f32() {
    let values = [0.0f32, -0.0, 1.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0x7fc01234), -f32::NAN];
    for &a in &values {
        for &b in &values {
            for kind in 0..26u32 {
                let (x, y) = if (kind / 2) & 1 != 0 { (b, a) } else { (a, b) };
                let c = match kind / 4 {
                    0 => x > y,
                    1 => x < y,
                    2 => x >= y,
                    3 => x <= y,
                    4 => x == y,
                    _ => x != y,
                };
                let expected = if kind >= 24 {
                    if kind & 1 != 0 {
                        a
                    } else {
                        b
                    }
                } else if c != (kind & 1 != 0) {
                    a
                } else {
                    b
                };
                assert_eq!(select_f32(a, b, kind).to_bits(), expected.to_bits());
            }
        }
    }
}
#[inline(never)]
fn select_f64(a: f64, b: f64, kind: u32) -> f64 {
    match kind {
        0 => {
            if a > b {
                a
            } else {
                b
            }
        }
        1 => {
            if a > b {
                b
            } else {
                a
            }
        }
        2 => {
            if b > a {
                a
            } else {
                b
            }
        }
        3 => {
            if b > a {
                b
            } else {
                a
            }
        }
        4 => {
            if a < b {
                a
            } else {
                b
            }
        }
        5 => {
            if a < b {
                b
            } else {
                a
            }
        }
        6 => {
            if b < a {
                a
            } else {
                b
            }
        }
        7 => {
            if b < a {
                b
            } else {
                a
            }
        }
        8 => {
            if a >= b {
                a
            } else {
                b
            }
        }
        9 => {
            if a >= b {
                b
            } else {
                a
            }
        }
        10 => {
            if b >= a {
                a
            } else {
                b
            }
        }
        11 => {
            if b >= a {
                b
            } else {
                a
            }
        }
        12 => {
            if a <= b {
                a
            } else {
                b
            }
        }
        13 => {
            if a <= b {
                b
            } else {
                a
            }
        }
        14 => {
            if b <= a {
                a
            } else {
                b
            }
        }
        15 => {
            if b <= a {
                b
            } else {
                a
            }
        }
        16 => {
            if a == b {
                a
            } else {
                b
            }
        }
        17 => {
            if a == b {
                b
            } else {
                a
            }
        }
        18 => {
            if b == a {
                a
            } else {
                b
            }
        }
        19 => {
            if b == a {
                b
            } else {
                a
            }
        }
        20 => {
            if a != b {
                a
            } else {
                b
            }
        }
        21 => {
            if a != b {
                b
            } else {
                a
            }
        }
        22 => {
            if b != a {
                a
            } else {
                b
            }
        }
        23 => {
            if b != a {
                b
            } else {
                a
            }
        }
        _ => {
            if kind & 1 != 0 {
                a
            } else {
                b
            }
        }
    }
}
#[test]
fn bits_f64() {
    let values = [0.0f64, -0.0, 1.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::from_bits(0x7ff8000000001234), -f64::NAN];
    for &a in &values {
        for &b in &values {
            for kind in 0..26u32 {
                let (x, y) = if (kind / 2) & 1 != 0 { (b, a) } else { (a, b) };
                let c = match kind / 4 {
                    0 => x > y,
                    1 => x < y,
                    2 => x >= y,
                    3 => x <= y,
                    4 => x == y,
                    _ => x != y,
                };
                let expected = if kind >= 24 {
                    if kind & 1 != 0 {
                        a
                    } else {
                        b
                    }
                } else if c != (kind & 1 != 0) {
                    a
                } else {
                    b
                };
                assert_eq!(select_f64(a, b, kind).to_bits(), expected.to_bits());
            }
        }
    }
}
#[inline(never)]
fn argmax(logits: &[f32]) -> (f32, usize) {
    let mut best = logits[0];
    let mut idx = 0;
    for (i, &v) in logits.iter().enumerate() {
        if v > best {
            best = v;
            idx = i;
        }
    }
    (best, idx)
}
#[test]
fn argmax_shared_compare() {
    for _ in 0..1000 {
        let (best, idx) = argmax(&[0.0, -0.0, f32::NAN, 4.0, 4.0, 2.0]);
        assert_eq!(best.to_bits(), 4.0f32.to_bits());
        assert_eq!(idx, 3);
    }
}

#[test]
fn truncate_without_drop() {
    let mut v = vec![1u32, 2, 3, 4];
    let capacity = v.capacity();
    v.truncate(10);
    assert_eq!(v.len(), 4);
    v.truncate(2);
    assert_eq!(v.as_slice(), &[1, 2]);
    assert_eq!(v.capacity(), capacity);
    v.push(9);
    assert_eq!(v.as_slice(), &[1, 2, 9]);
    v.truncate(0);
    assert_eq!(v.len(), 0);
    let mut z = vec![(); 12];
    z.truncate(3);
    assert_eq!(z.len(), 3);
    z.truncate(20);
    assert_eq!(z.len(), 3);
}

#[test]
fn truncate_with_drop() {
    use std::cell::Cell;
    struct Item<'a>(&'a Cell<usize>);
    impl Drop for Item<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Cell::new(0);
    {
        let mut v = Vec::new();
        for _ in 0..4 {
            v.push(Item(&drops));
        }
        v.truncate(8);
        assert_eq!(drops.get(), 0);
        v.truncate(2);
        assert_eq!(drops.get(), 2);
        v.truncate(0);
        assert_eq!(drops.get(), 4);
    }
    assert_eq!(drops.get(), 4);
}
