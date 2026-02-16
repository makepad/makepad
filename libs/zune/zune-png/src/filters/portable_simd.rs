#![cfg(feature = "portable-simd")]
use core::simd::prelude::*;
#[allow(unused_assignments)]
pub fn defilter_sub_generic<const SIZE: usize>(raw: &[u8], current: &mut [u8]) {
    let mut zero = [0; 16];
    let (mut a, mut d) = (u8x16::splat(0), u8x16::splat(0));

    for (raw, out) in raw.chunks_exact(SIZE).zip(current.chunks_exact_mut(SIZE)) {
        zero[0..SIZE].copy_from_slice(raw);

        a = d;
        d = u8x16::from_slice(&zero);
        d += a;
        d.copy_to_slice(&mut zero);

        out.copy_from_slice(&zero[0..SIZE]);
    }
}

pub fn defilter_avg_generic<const SIZE: usize>(prev_row: &[u8], raw: &[u8], current: &mut [u8]) {
    let (mut x, mut y) = ([0; 16], [0; 16]);
    let (mut a, mut b);

    let mut d = u8x16::splat(0);

    let one = u8x16::splat(1);

    for ((prev, raw), current_row) in prev_row
        .chunks_exact(SIZE)
        .zip(raw.chunks_exact(SIZE))
        .zip(current.chunks_exact_mut(SIZE))
    {
        x[0..SIZE].copy_from_slice(raw);
        y[0..SIZE].copy_from_slice(prev);

        b = u8x16::from_slice(&y);
        a = d;
        d = u8x16::from_slice(&x);
        // find average with overflow handling
        // from stanford bit-hacks
        //
        // forgot the link i got it from :(
        let avg = (a & b) + ((a ^ b) >> one);
        d += avg;
        d.copy_to_slice(&mut x);

        current_row.copy_from_slice(&x[0..SIZE]);
    }
}

#[allow(unused_assignments)]
pub fn defilter_paeth_generic<const SIZE: usize>(prev_row: &[u8], raw: &[u8], current: &mut [u8]) {
    // https://rust.godbolt.org/z/MKnPP19Mr
    let (mut f, mut g) = ([0; 16], [0; 16]);
    let zero = i16x8::splat(0);

    let (mut c, mut b, mut a, mut d) = (zero, zero, zero, zero);

    for ((prev, raw), current_row) in prev_row
        .chunks_exact(SIZE)
        .zip(raw.chunks_exact(SIZE))
        .zip(current.chunks_exact_mut(SIZE))
    {
        f[0..SIZE].copy_from_slice(prev);
        g[0..SIZE].copy_from_slice(raw);

        c = b;
        b = u8x8::from_slice(&f).cast::<i16>();
        a = d;
        d = u8x8::from_slice(&g).cast::<i16>();

        let pa = b - c;
        let pb = a - c;
        let pc = pa + pb;

        let pa = pa.abs();
        let pb = pb.abs();
        let pc = pc.abs();

        let smallest = pa.simd_min(pb).simd_min(pc);

        let p = smallest.simd_eq(pb).select(b, c);
        let q = smallest.simd_eq(pa).select(a, p);

        let cw = q.cast::<u8>() + d.cast::<u8>();
        d = cw.cast::<i16>();

        cw.copy_to_slice(&mut f);

        current_row.copy_from_slice(&f[0..SIZE]);
    }
}
