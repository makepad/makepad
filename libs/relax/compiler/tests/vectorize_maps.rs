// Element maps that O3 vectorises (codegen loops.rs vectorize_stream_maps): f32 and 32-bit
// integer loops over iterator zips and raw-pointer streams with a loop-invariant scalar, every
// length 0..=40 at several start offsets (unaligned vector loads, remainders of every size), and
// destination streams that overlap the source streams at every distance from -9 to 9 elements
// (the runtime distance check must keep the scalar order); each result is compared with a
// reference whose per-element step is a call (never widened); called often enough to tier up.
use std::hint::black_box;

#[inline(never)]
fn mul_add(y: &mut [f32], x: &[f32], k: f32) {
    for (o, v) in y.iter_mut().zip(x.iter()) {
        *o += *v * k;
    }
}
#[inline(never)]
fn mul_add_ref(y: &mut [f32], x: &[f32], k: f32) {
    for i in 0..y.len().min(x.len()) {
        let t = black_box(x[i] * k);
        y[i] = y[i] + t;
    }
}
#[inline(never)]
fn scale_neg(y: &mut [f32], x: &[f32], k: f32) {
    for (o, v) in y.iter_mut().zip(x.iter()) {
        *o = -(*v) * k - 1.5;
    }
}
#[inline(never)]
fn scale_neg_ref(y: &mut [f32], x: &[f32], k: f32) {
    for i in 0..y.len().min(x.len()) {
        y[i] = black_box(-x[i] * k) - 1.5;
    }
}
#[inline(never)]
fn mix_i32(a: &mut [i32], b: &[i32], k: i32) {
    for (o, v) in a.iter_mut().zip(b.iter()) {
        *o = o.wrapping_add(v.wrapping_mul(k)) ^ 0x55;
    }
}
#[inline(never)]
fn mix_i32_ref(a: &mut [i32], b: &[i32], k: i32) {
    for i in 0..a.len().min(b.len()) {
        a[i] = a[i].wrapping_add(black_box(b[i].wrapping_mul(k))) ^ 0x55;
    }
}
/// a raw-pointer stream: dst[i] = src[i] * 2 + 1 for n elements, the two streams may overlap
#[inline(never)]
unsafe fn stream(dst: *mut f32, src: *const f32, n: usize) {
    let mut d = dst;
    let mut s = src;
    let end = src.add(n);
    while s != end {
        *d = *s * 2.0 + 1.0;
        d = d.add(1);
        s = s.add(1);
    }
}
#[inline(never)]
unsafe fn stream_ref(dst: *mut f32, src: *const f32, n: usize) {
    for i in 0..n {
        let v = black_box(*src.add(i) * 2.0);
        *dst.add(i) = v + 1.0;
    }
}

/// index loops: `a[i] += b[i] * c[i]`, displaced reads, and the in-place shifted forms
#[inline(never)]
fn idx_mad(a: &mut [f32], b: &[f32], c: &[f32]) {
    let n = a.len().min(b.len()).min(c.len());
    let (a, b, c) = (&mut a[..n], &b[..n], &c[..n]);
    for i in 0..n {
        a[i] += b[i] * c[i];
    }
}
#[inline(never)]
fn idx_mad_ref(a: &mut [f32], b: &[f32], c: &[f32]) {
    let n = a.len().min(b.len()).min(c.len());
    for i in 0..n {
        let t = black_box(b[i] * c[i]);
        a[i] = a[i] + t;
    }
}
#[inline(never)]
fn idx_disp(a: &mut [f32], b: &[f32], c: &[f32], k: f32) {
    let n = a.len().min(b.len()).min(c.len());
    for i in 1..n.saturating_sub(1) {
        a[i] = b[i - 1] + c[i + 1] * k;
    }
}
#[inline(never)]
fn idx_disp_ref(a: &mut [f32], b: &[f32], c: &[f32], k: f32) {
    let n = a.len().min(b.len()).min(c.len());
    for i in 1..n.saturating_sub(1) {
        a[i] = b[i - 1] + black_box(c[i + 1] * k);
    }
}
/// `v[i] = v[i + 1] * 2` reads ahead of its writes (vector-safe); `v[i + 1] = v[i] * 2 + 1` feeds
/// each result into the next element (must stay in order)
#[inline(never)]
fn shift_down(v: &mut [i32]) {
    for i in 0..v.len().saturating_sub(1) {
        v[i] = v[i + 1].wrapping_mul(3);
    }
}
#[inline(never)]
fn shift_down_ref(v: &mut [i32]) {
    for i in 0..v.len().saturating_sub(1) {
        v[i] = black_box(v[i + 1].wrapping_mul(3));
    }
}
#[inline(never)]
fn carry_up(v: &mut [i32]) {
    for i in 0..v.len().saturating_sub(1) {
        v[i + 1] = v[i].wrapping_mul(3) ^ 1;
    }
}
#[inline(never)]
fn carry_up_ref(v: &mut [i32]) {
    for i in 0..v.len().saturating_sub(1) {
        v[i + 1] = black_box(v[i].wrapping_mul(3)) ^ 1;
    }
}

/// the PNG "up" defilter: three zipped byte streams
#[inline(never)]
fn up_defilter(prev_row: &[u8], raw: &[u8], current: &mut [u8]) {
    for ((filt, recon), up) in raw.iter().zip(current).zip(prev_row) {
        *recon = (*filt).wrapping_add(*up)
    }
}
#[inline(never)]
fn up_defilter_ref(prev_row: &[u8], raw: &[u8], current: &mut [u8]) {
    for i in 0..prev_row.len().min(raw.len()).min(current.len()) {
        current[i] = black_box(raw[i].wrapping_add(prev_row[i]));
    }
}

/// eight lane accumulators over chunks (the array version of a SIMD dot product)
#[inline(never)]
fn dot_lanes(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for l in 0..8 {
            acc[l] += x[l] * y[l];
        }
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))
}
#[inline(never)]
fn dot_lanes_ref(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let n = a.len().min(b.len()) / 8 * 8;
    let mut i = 0;
    while i < n {
        for l in 0..8 {
            let t = black_box(a[i + l] * b[i + l]);
            acc[l] = acc[l] + t;
        }
        i += 8;
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))
}
/// lane accumulators with an early exit and a subtract
#[inline(never)]
fn diff_lanes(a: &[f32], b: &[f32], limit: f32) -> (f32, f32) {
    let mut acc = [1.0f32; 4];
    let mut rounds = 0.0f32;
    for (x, y) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for l in 0..4 {
            acc[l] -= x[l] * y[l];
        }
        rounds += 1.0;
        if acc[0] < limit {
            break;
        }
    }
    (acc[0] + acc[1] + acc[2] + acc[3], rounds)
}
#[inline(never)]
fn diff_lanes_ref(a: &[f32], b: &[f32], limit: f32) -> (f32, f32) {
    let mut acc = [1.0f32; 4];
    let mut rounds = 0.0f32;
    let n = a.len().min(b.len()) / 4 * 4;
    let mut i = 0;
    while i < n {
        for l in 0..4 {
            let t = black_box(a[i + l] * b[i + l]);
            acc[l] = acc[l] - t;
        }
        rounds += 1.0;
        i += 4;
        if acc[0] < limit {
            break;
        }
    }
    (acc[0] + acc[1] + acc[2] + acc[3], rounds)
}

/// the index bounded by `min` of two lengths: no slicing first
#[inline(never)]
fn idx_sq(out: &mut [f32], a: &[f32]) {
    for i in 0..out.len().min(a.len()) {
        out[i] = a[i] * a[i];
    }
}
#[inline(never)]
fn idx_sq_ref(out: &mut [f32], a: &[f32]) {
    for i in 0..out.len().min(a.len()) {
        out[i] = black_box(a[i] * a[i]);
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn vectorized_maps() {
    let src: Vec<f32> = (0..64).map(|i| (i as f32) * 0.37 - 5.0).collect();
    let isrc: Vec<i32> = (0..64).map(|i| (i as i32).wrapping_mul(0x9E37_79B1u32 as i32)).collect();
    let mut fails = 0;
    for rep in 0..4000u32 {
        let k = 0.25 + (rep % 7) as f32;
        let ki = 3 + (rep % 5) as i32;
        for n in 0..41usize {
            for lo in 0..3usize {
                let hi = (lo + n).min(64);
                let mut y1 = src.clone();
                let mut y2 = src.clone();
                mul_add(&mut y1[lo..hi], &src[lo..hi], black_box(k));
                mul_add_ref(&mut y2[lo..hi], &src[lo..hi], k);
                fails += (bits(&y1) != bits(&y2)) as u32;
                let mut y1 = vec![0.0f32; 64];
                let mut y2 = vec![0.0f32; 64];
                scale_neg(&mut y1[lo..hi], &src[lo..hi], black_box(k));
                scale_neg_ref(&mut y2[lo..hi], &src[lo..hi], k);
                fails += (bits(&y1) != bits(&y2)) as u32;
                let mut a1 = isrc.clone();
                let mut a2 = isrc.clone();
                mix_i32(&mut a1[lo..hi], &isrc[lo..hi], black_box(ki));
                mix_i32_ref(&mut a2[lo..hi], &isrc[lo..hi], ki);
                fails += (a1 != a2) as u32;
            }
            for lo in 0..3usize {
                let mut y1 = vec![3.0f32; 64];
                let mut y2 = vec![3.0f32; 64];
                idx_sq(&mut y1[lo..], &src[..n]);
                idx_sq_ref(&mut y2[lo..], &src[..n]);
                fails += (bits(&y1) != bits(&y2)) as u32;
                let mut y1 = vec![3.0f32; 64];
                let mut y2 = vec![3.0f32; 64];
                idx_sq(&mut y1[..n], &src[lo..]);
                idx_sq_ref(&mut y2[..n], &src[lo..]);
                fails += (bits(&y1) != bits(&y2)) as u32;
            }
            for lo in 0..3usize {
                let hi = (lo + n).min(64);
                let mut y1 = src.clone();
                let mut y2 = src.clone();
                idx_mad(&mut y1[lo..hi], &src[lo..hi], &src[3..]);
                idx_mad_ref(&mut y2[lo..hi], &src[lo..hi], &src[3..]);
                fails += (bits(&y1) != bits(&y2)) as u32;
                let mut y1 = vec![1.0f32; 64];
                let mut y2 = vec![1.0f32; 64];
                idx_disp(&mut y1[lo..hi], &src[lo..hi], &src[1 + lo..], black_box(k));
                idx_disp_ref(&mut y2[lo..hi], &src[lo..hi], &src[1 + lo..], k);
                fails += (bits(&y1) != bits(&y2)) as u32;
                let mut a1 = isrc.clone();
                let mut a2 = isrc.clone();
                shift_down(&mut a1[lo..hi]);
                shift_down_ref(&mut a2[lo..hi]);
                fails += (a1 != a2) as u32;
                let mut a1 = isrc.clone();
                let mut a2 = isrc.clone();
                carry_up(&mut a1[lo..hi]);
                carry_up_ref(&mut a2[lo..hi]);
                fails += (a1 != a2) as u32;
            }
            {
                let mut fa: Vec<f32> = (0..64).map(|i| ((i * 37 + rep as usize) % 101) as f32 * 0.31 - 12.0).collect();
                let fb: Vec<f32> = (0..64).map(|i| ((i * 11 + 5) % 89) as f32 * 0.17 - 7.0).collect();
                if rep % 7 == 0 {
                    fa[(rep as usize) % 64] = f32::NAN;
                }
                if rep % 11 == 0 {
                    fa[(rep as usize * 3) % 64] = f32::INFINITY;
                }
                fails += (dot_lanes(&fa[..n], &fb).to_bits() != dot_lanes_ref(&fa[..n], &fb).to_bits()) as u32;
                let lim = -((rep % 50) as f32) * 3.0;
                let (s1, r1) = diff_lanes(&fa[..n], &fb, lim);
                let (s2, r2) = diff_lanes_ref(&fa[..n], &fb, lim);
                fails += (s1.to_bits() != s2.to_bits() || r1 != r2) as u32;
            }
            {
                let a: Vec<u8> = (0..64u32).map(|i| (i * 37 + rep) as u8).collect();
                let b: Vec<u8> = (0..64u32).map(|i| (i * 11 + 5) as u8).collect();
                let mut o1 = vec![7u8; 64];
                let mut o2 = vec![7u8; 64];
                up_defilter(&a[..n], &b[1..n.max(1)], &mut o1[2..]);
                up_defilter_ref(&a[..n], &b[1..n.max(1)], &mut o2[2..]);
                fails += (o1 != o2) as u32;
            }
            // overlapping streams inside one buffer
            for shift in -9i32..=9 {
                let base = 12usize;
                let mut b1 = src.clone();
                let mut b2 = src.clone();
                unsafe {
                    let s1 = b1.as_ptr().add(base);
                    let d1 = b1.as_mut_ptr().add((base as i32 + shift) as usize);
                    stream(d1, s1, n);
                    let s2 = b2.as_ptr().add(base);
                    let d2 = b2.as_mut_ptr().add((base as i32 + shift) as usize);
                    // the reference is the same loop in element order
                    for i in 0..n {
                        let v = black_box(*s2.add(i) * 2.0);
                        *d2.add(i) = v + 1.0;
                    }
                }
                fails += (bits(&b1) != bits(&b2)) as u32;
            }
            let mut d = vec![0.0f32; 64];
            let mut e = vec![0.0f32; 64];
            unsafe {
                stream(d.as_mut_ptr(), src.as_ptr(), n);
                stream_ref(e.as_mut_ptr(), src.as_ptr(), n);
            }
            fails += (bits(&d) != bits(&e)) as u32;
        }
    }
    println!("| fails {}", fails);
    assert_eq!(fails, 0);
}
