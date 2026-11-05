// Small local arrays built, mapped and counted per iteration of a hot loop (a marching-squares
// cell: the four corners as an array of index pairs, `map` to "inside" flags, `iter().filter()
// .count()`). Every result is compared with a reference that reads the field through black_box;
// called often enough to tier up.
use std::hint::black_box;

#[inline(never)]
fn classify(f: &[f64], nx: usize, ny: usize) -> (usize, u64) {
    let val = |i: usize, j: usize| f[j * nx + i];
    let (mut n, mut h) = (0usize, 0u64);
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let inside = c.map(|(a, b)| val(a, b) < 0.0);
            let count = inside.iter().filter(|x| **x).count();
            if count == 0 || count == 4 {
                continue;
            }
            n += 1;
            let bits = inside.iter().enumerate().fold(0u64, |s, (k, &x)| s | ((x as u64) << k));
            h = h.rotate_left(5) ^ (count as u64) ^ (bits << 3) ^ (c[(i + j) % 4].0 as u64) << 8;
        }
    }
    (n, h)
}

fn classify_ref(f: &[f64], nx: usize, ny: usize) -> (usize, u64) {
    let (mut n, mut h) = (0usize, 0u64);
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let mut inside = [false; 4];
            let mut count = 0;
            for k in 0..4 {
                let (a, b) = black_box(c[k]);
                inside[k] = black_box(f[b * nx + a]) < 0.0;
                count += inside[k] as usize;
            }
            if count == 0 || count == 4 {
                continue;
            }
            n += 1;
            let mut bits = 0u64;
            for k in 0..4 {
                bits |= (black_box(inside[k]) as u64) << k;
            }
            h = h.rotate_left(5) ^ (count as u64) ^ (bits << 3) ^ (c[(i + j) % 4].0 as u64) << 8;
        }
    }
    (n, h)
}

#[test]
fn small_array_map() {
    let (nx, ny) = (37usize, 23usize);
    // a field with a wavy zero set, NaN-free
    let mut f = vec![0.0f64; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let (x, y) = (i as f64 - 18.0, j as f64 - 11.0);
            f[j * nx + i] = (x * x + y * y).sqrt() - 7.0 - 2.0 * (x * 0.7).sin() * (y * 0.4).cos();
        }
    }
    let want = classify_ref(&f, nx, ny);
    assert!(want.0 > 20, "{want:?}");
    for round in 0..3000 {
        let k = round % (nx * ny);
        f[k] = -f[k];
        let w = classify_ref(&f, nx, ny);
        assert_eq!(classify(&f, nx, ny), w, "round {round}");
    }
    println!("small array map: ok");
}
