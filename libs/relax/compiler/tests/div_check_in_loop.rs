// Signed division in a loop over a loop-invariant divisor (the slicer's grid cell of a point:
// `(x - x0).div_euclid(cell)`): the overflow check `b == -1 & a == MIN` has its `b == -1` hoisted
// out of the loop and the branch split into a test of that flag and, only when it holds, of
// `a == MIN` (backend split_branch_conditions). Called often enough to tier up; every result is
// compared with a reference that divides through black_box (no check shared with the loop), and
// MIN / -1 must still panic, at MIN's element, after the elements before it were summed.
use std::hint::black_box;

#[inline(never)]
fn cells(xs: &[i64], x0: i64, cell: i64, upto: &mut usize) -> i64 {
    let mut s = 0i64;
    for (k, &x) in xs.iter().enumerate() {
        *upto = k;
        let c = (x.wrapping_sub(x0)).div_euclid(cell);
        s = s.wrapping_mul(31).wrapping_add(c ^ (x / cell));
    }
    s
}

fn cells_ref(xs: &[i64], x0: i64, cell: i64) -> i64 {
    let mut s = 0i64;
    for &x in xs {
        let a = black_box(x.wrapping_sub(x0));
        let c = black_box(a).div_euclid(black_box(cell));
        s = s.wrapping_mul(31).wrapping_add(c ^ (black_box(x) / black_box(cell)));
    }
    s
}

#[test]
fn div_check_in_loop() {
    // (no element whose x - x0 reaches MIN for any x0 below: -1 is among the divisors)
    let xs: Vec<i64> = (-300i64..300).map(|i| i * 7919 + (i & 3) - 11).chain([i64::MAX - 2, i64::MIN + 3, 0, -1]).collect();
    let mut upto = 0;
    for round in 0..400 {
        for &cell in &[1i64, 3, -3, 7, -1, 1 << 40, i64::MAX, i64::MIN] {
            let x0 = (round % 5) as i64 - 2;
            assert_eq!(cells(&xs, x0, cell, &mut upto), cells_ref(&xs, x0, cell), "cell {cell} x0 {x0}");
        }
    }
    // MIN / -1 overflows: the panic comes at MIN's element (x0 = 0: x - x0 is MIN there)
    let mut with_min = xs.clone();
    with_min.insert(17, i64::MIN);
    for &cell in &[3i64, -1] {
        let mut at = usize::MAX;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cells(&with_min, 0, cell, &mut at)));
        if cell == -1 {
            assert!(r.is_err(), "MIN / -1 did not panic");
            assert_eq!(at, 17);
        } else {
            assert_eq!(r.ok(), Some(cells_ref(&with_min, 0, cell)));
        }
    }
    println!("div check in loop: ok");
}
