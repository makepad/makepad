// A stack slot keeps its type's alignment whatever the function saves: an odd number of
// callee-saved registers below the frame's base left an align(16) local 8 bytes off on x64
// (GetThreadContext refused such a CONTEXT on Windows, movaps would fault). And an alignment
// above the frame base's 16: the slot's offset from the base was aligned, its address only to 16
// (an aligned 32-byte AVX load of it faults).
use std::hint::black_box;

#[repr(align(16))]
struct Aligned([u64; 4]);

#[inline(never)]
fn off(a: &mut Aligned) -> usize {
    a.0[0] += 1;
    a as *mut Aligned as usize % 16
}

// `live` values stay in callee-saved registers across the calls
macro_rules! holding {
    ($name:ident, $($v:ident),*) => {
        #[inline(never)]
        fn $name(x: u64) -> (usize, u64) {
            $(let $v = black_box(x + 1);)*
            let mut a = Aligned([0; 4]);
            let o = off(&mut a);
            let o2 = off(&mut a);
            (o + o2, $($v +)* a.0[0])
        }
    };
}
holding!(one, a);
holding!(two, a, b);
holding!(three, a, b, c);
holding!(four, a, b, c, d);
holding!(five, a, b, c, d, e);

#[test]
fn an_aligned_local_stays_aligned() {
    for f in [one, two, three, four, five] {
        assert_eq!(f(1).0, 0);
    }
}

#[repr(C, align(32))]
struct A32([u64; 4]);
#[repr(C, align(64))]
struct A64([u64; 8]);
#[repr(C, align(256))]
struct A256([u8; 300]);

fn off_of<T>(p: &T, align: usize) -> usize {
    p as *const T as usize % align
}

/// An over-aligned aggregate passed and returned by value: the callee's view of its argument,
/// and the result its caller receives
#[inline(never)]
fn by_value(b: A64, a: A32) -> (usize, A64) {
    let off = off_of(&b, 64) + off_of(&a, 32);
    (off, A64([b.0[7] + a.0[3]; 8]))
}

/// An over-aligned parameter the callee writes to: its address, and the write kept
#[inline(never)]
fn written_param(mut b: A64) -> usize {
    b.0[7] += 1;
    off_of(black_box(&mut b), 64) + (b.0[7] != 2) as usize
}

#[repr(align(256))]
struct Empty;

// over-aligned locals (a zero-sized one too), an array of them, and one across calls, with
// `live` values in callee-saved registers (odd and even saves)
macro_rules! over {
    ($name:ident, $($v:ident),*) => {
        #[inline(never)]
        fn $name(x: u64) -> (usize, u64) {
            $(let $v = black_box(x + 1);)*
            let a = black_box(A32([x; 4]));
            let b = black_box(A64([x; 8]));
            let c = black_box(A256([x as u8; 300]));
            let arr = black_box([A64([x; 8]), A64([x + 1; 8]), A64([x + 2; 8])]);
            let mut off = off_of(&a, 32) + off_of(&b, 64) + off_of(&c, 256);
            off += arr.iter().map(|e| off_of(e, 64)).sum::<usize>();
            let (callee, r) = by_value(black_box(A64([x; 8])), black_box(A32([x; 4])));
            off += callee + off_of(&r, 64) + off_of(&b, 64) + written_param(black_box(A64([1; 8])));
            let z = Empty;
            off += off_of(black_box(&z), 256);
            (off, $($v +)* a.0[3] + b.0[7] + c.0[299] as u64 + arr[2].0[0] + r.0[0])
        }
    };
}
over!(o1, a1);
over!(o2, a1, a2);
over!(o3, a1, a2, a3);
over!(o4, a1, a2, a3, a4);
over!(o5, a1, a2, a3, a4, a5);

/// `f` called `depth` frames down, through frames whose sizes differ by 16 bytes: the frame base
/// takes every 16-byte offset an over-aligned slot could start at
#[inline(never)]
fn down(depth: u32, f: fn(u64) -> (usize, u64)) -> (usize, u64) {
    let pad = [depth as u8; 16];
    black_box(&pad);
    if depth == 0 { f(1) } else { let r = down_wide(depth - 1, f); (r.0, r.1 + pad[0] as u64) }
}

#[inline(never)]
fn down_wide(depth: u32, f: fn(u64) -> (usize, u64)) -> (usize, u64) {
    let pad = [depth as u8; 32];
    black_box(&pad);
    if depth == 0 { f(1) } else { let r = down(depth - 1, f); (r.0, r.1 + pad[0] as u64) }
}

#[test]
fn an_over_aligned_local_keeps_its_alignment() {
    for f in [o1, o2, o3, o4, o5] {
        for depth in 0..32 {
            let (off, _) = down(depth, f);
            assert_eq!(off, 0, "an over-aligned slot off its alignment ({depth} frames down)");
        }
    }
    // the values came through: 1 + live values, the locals' and the call's
    assert_eq!(o2(1).1, 2 + 2 + 1 + 1 + 1 + 3 + 2);
}
