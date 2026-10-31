// Web target layout corpus: size_of / align_of / offset_of of ~250 types, which must equal
// rustc's for wasm32-unknown-unknown (pointer width 32) and wasm64-unknown-unknown (64; its data
// layout's integer, float and pointer alignments are x86_64's, so rustc x86_64 is the reference).
// Included by wasm_ref.rs (rustc, run in node: the reference table) and check.rs (Relax under the
// web target: one const assertion per row of the table). Driver: run.sh.
use std::borrow::Cow;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::cmp::{Ordering, Reverse};
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::num::{NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8, NonZeroUsize, Wrapping};
use std::ops::{Range, RangeInclusive};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

struct S1(u8, u32, u8);
struct S2 { a: u8, b: u64, c: u16, d: u8 }
struct S3 { a: bool, b: [u8; 16] }
struct S4(u8, [u8; 16]);
struct S5 { a: u32, b: (bool, u8), c: u16 }
struct S6((u8, u8, u8, bool), (u8, bool, u8));
struct S7 { x: char, y: u8 }
struct S8 { r: &'static u8, n: u8 }
struct S9 { a: u16, b: u8, c: u32, d: u8, e: u64 }
struct S10 { a: [u16; 3], b: u8 }
struct S11<T: ?Sized> { a: u8, b: u32, t: T }
struct S12 { a: u8, b: Option<bool>, c: u32 }
struct S13 { a: (), b: u8, c: [u32; 0], d: u16 }
#[repr(C)]
struct C1(u8, u32, u8);
#[repr(C)]
struct C2 { a: u8, b: u64, c: u16 }
#[repr(packed)]
struct P1(u8, u32, u16);
#[repr(packed(2))]
struct P2 { a: u8, b: u64, c: u8 }
#[repr(align(16))]
struct A1(u8, u16);
#[repr(transparent)]
struct T1(u32);
struct Nz(NonZeroU32, u8);
struct Fp(fn(), u8);
struct Bx(Box<u8>, bool);

#[derive(Clone, Copy)]
enum E1 { A, B, C }
enum E2 { A = -1, B = 0, C = 1 }
enum E3 { A = 300, B }
enum E4 { A(u32), B(u8), C }
enum E5 { A(bool), B }
enum E6 { A(bool), B, C, D }
enum E7 { A(char), B, C }
enum E8 { A(u64, bool), B(u8) }
enum E9 { A(u8, bool), B(u32) }
enum E10 { A(&'static u8), B(u8) }
enum E11 { A(E1), B }
enum E12 { A(u32, E1), B(u16) }
enum E13 { A { x: u8, y: u64 }, B { z: u16 } }
enum E14 { Only(u32, u8) }
enum E15 { Only(u8), Never(std::convert::Infallible) }
enum E16 { A(Option<bool>), B(bool), C }
enum E17 { A([u8; 3], bool), B(u16) }
enum E18 { A(u8), B(u8), C(u8), D(u8) }
enum E19 { A(NonNull<u8>), B, C }
enum E20 { A(Box<u32>), B(u32) }
#[repr(u8)]
enum R1 { A(u32), B }
#[repr(C)]
enum R2 { A(u8), B(u64) }
#[repr(C)]
enum R3 { A, B }
#[repr(i16)]
enum R4 { A = -5, B = 9 }
#[repr(align(8))]
enum R5 { A, B }
enum E21 { A(u16, u8), B(u8, u8, u8) }
enum E22 { A(u32, bool), B(u32), C }
enum E23 { A((bool, bool)), B }
enum E24 { A(Option<char>), B, C }

// ---- pointer-width sensitive types (the web targets: wasm32 4-byte pointers, wasm64 8)
struct W1 { a: usize, b: u8 }
struct W2 { a: u8, b: usize, c: u64, d: u16 }
struct W3 { p: *const u8, len: usize, f: f64 }
struct W4 { s: &'static str, n: u32 }
struct W5 { d: &'static dyn Fn(), b: bool }
struct W6 { v: Vec<u8>, s: String, k: u64 }
struct W7 { a: u128, b: u8 }
struct W8 { a: i64, b: u32 }
struct W9 { f: fn(u32) -> u32, x: u8, y: Option<Box<u8>> }
#[repr(C)]
struct WC1 { a: u8, p: *const u8, b: u16, n: usize, c: u64 }
#[repr(C)]
struct WC2 { a: u32, b: f64, c: u8 }
#[repr(C)]
struct WC3 { a: u8, b: u128 }
#[repr(C)]
union WU1 { a: u8, p: *const u8, n: usize }
#[repr(C)]
union WU2 { a: u64, p: *const u8 }
enum WE1 { A(usize), B(u8) }
enum WE2 { A(&'static [u8]), B }
enum WE3 { A(Vec<u8>), B(String), C }
enum WE4 { A(Box<dyn Fn()>), B(u32) }
enum WE5 { A(*const u8, u8), B(u16) }
enum WE6 { A(u64), B(usize, u8) }
#[repr(usize)]
enum WR1 { A = 1, B = 2 }
#[repr(isize)]
enum WR2 { A = -1, B }
// the wasm_bridge message shapes (platform/src/os/web/from_wasm.rs, to_wasm.rs): ids, pointers
// handed to the page as u32, strings and byte vectors, f64 timestamps
struct LiveIdLike(u64);
struct MsgTextureLike { id: usize, width: usize, height: usize, data: Vec<u32>, live: LiveIdLike }
struct MsgWindowLike { inner_width: f64, inner_height: f64, dpi: f64, is_fullscreen: bool, title: String }
struct MsgPtrLike { ptr: u32, len: u32, cap: u32, owner: Option<Box<[u8]>> }
struct MsgEventLike { time: f64, id: u32, modifiers: u32, text: Option<String>, abs: (f64, f64) }

macro_rules! web_layout_rows {
    () => {
        sz!(bool, char, u8, u16, u32, u64, u128, i128, usize, isize, f32, f64, (), &u8, &[u8], &str, &dyn Fn(), fn(), *const u8, Box<u8>, Box<[u8]>);
        sz!(Option<bool>, Option<char>, Option<u8>, Option<u32>, Option<u64>, Option<&u8>, Option<&[u8]>, Option<fn()>, Option<Box<u8>>, Option<NonNull<u8>>);
        sz!(Option<NonZeroU8>, Option<NonZeroU16>, Option<NonZeroU32>, Option<NonZeroU64>, Option<Option<bool>>, Option<Option<Option<bool>>>, Option<(u8, bool)>, Option<(bool, u32)>, Option<[bool; 3]>, Option<()>);
        sz!(Option<Option<char>>, Option<Option<&u8>>, Option<E1>, Option<Option<E1>>, Option<E2>, Option<E3>, Option<std::cmp::Ordering>, Result<u32, ()>, Result<bool, ()>, Result<&u8, u8>);
        sz!(Result<u8, bool>, Result<u32, u32>, Result<(), Box<u8>>, Result<u64, std::convert::Infallible>, Cell<bool>, Option<Cell<bool>>, UnsafeCell<bool>, Option<UnsafeCell<bool>>, RefCell<u8>, Option<RefCell<u8>>);
        sz!((u8, u32, u8), (u8, u16), (u32, u8, u16, u8), (bool, u64), ((u8, bool), u32), [u8; 0], [bool; 4], [(u8, u16); 2], (u8, [u32; 0]), (char, u8));
        sz!(E1, E2, E3, E4, E5, E6, E7, E8, E9, E10, E11, E12, E13, E14, E15, E16, E17, E18, E19, E20, E21, E22, E23, E24);
        sz!(R1, R2, R3, R4, R5, Option<R1>, Option<R3>, Option<E5>, Option<E6>, Option<E9>, Option<E14>, Option<E15>, Option<E17>);
        sz!(S1, S2, S3, S4, S5, S6, S7, S8, S9, S10, S12, S13, C1, C2, P1, P2, A1, T1, Nz, Fp, Bx);
        sz!(Option<S1>, Option<S3>, Option<S7>, Option<S8>, Option<Nz>, Option<Fp>, Option<Bx>, Option<T1>, Option<P1>, Option<S11<u8>>);
        // pointer-width sensitive
        sz!(Option<usize>, Option<isize>, [usize; 3], (usize, u8), (u8, usize, u8), (u64, usize), (u128, usize), Option<NonZeroUsize>, Option<Option<NonZeroUsize>>, &&[u8]);
        sz!(Vec<u8>, Vec<u64>, String, Box<str>, Box<dyn Fn()>, Box<dyn Fn() + Send>, Rc<u8>, Arc<u8>, Rc<[u8]>, Arc<str>, Weak<u8>);
        sz!(Option<Vec<u8>>, Option<String>, Option<Box<dyn Fn()>>, Option<&dyn Fn()>, Option<Rc<u8>>, Option<Arc<str>>, Result<Vec<u8>, String>, Result<usize, Box<str>>, Option<Option<Vec<u8>>>);
        sz!(Cell<usize>, RefCell<Vec<u8>>, Option<RefCell<usize>>, Range<usize>, Range<u64>, RangeInclusive<usize>, Duration, Option<Duration>, Cow<'static, str>, Option<Cow<'static, [u8]>>);
        sz!(PhantomData<u64>, ManuallyDrop<String>, MaybeUninit<usize>, Wrapping<usize>, Reverse<usize>, Ordering, Pin<Box<u8>>, Option<Pin<Box<dyn Fn()>>>);
        // (core::str::Chars is left out: Relax's core keeps the str and two indices, rustc's a byte iterator)
        sz!(core::slice::Iter<'static, u8>, core::ops::Bound<usize>, AtomicUsize, AtomicU64, AtomicPtr<u8>, AtomicBool, Option<AtomicUsize>);
        sz!(W1, W2, W3, W4, W5, W6, W7, W8, W9, WC1, WC2, WC3, WU1, WU2, WE1, WE2, WE3, WE4, WE5, WE6, WR1, WR2);
        sz!(Option<W1>, Option<W4>, Option<W5>, Option<W6>, Option<W9>, Option<WE1>, Option<WE2>, Option<WE3>, Option<WE5>, Option<WR1>, Option<Option<WE2>>);
        sz!(LiveIdLike, MsgTextureLike, MsgWindowLike, MsgPtrLike, MsgEventLike, Option<MsgEventLike>, Vec<MsgEventLike>);
        offs!(S1: 0, 1, 2);
        offs!(S2: a, b, c, d);
        offs!(S3: a, b);
        offs!(S4: 0, 1);
        offs!(S5: a, b, c);
        offs!(S6: 0, 1);
        offs!(S7: x, y);
        offs!(S8: r, n);
        offs!(S9: a, b, c, d, e);
        offs!(S10: a, b);
        offs!(S11<u64>: a, b, t);
        offs!(S11<u8>: a, b, t);
        offs!(S11<[u8; 3]>: a, b, t);
        offs!(S11<usize>: a, b, t);
        offs!(S12: a, b, c);
        offs!(S13: a, b, c, d);
        offs!(C1: 0, 1, 2);
        offs!(C2: a, b, c);
        offs!(P1: 0, 1, 2);
        offs!(P2: a, b, c);
        offs!(A1: 0, 1);
        offs!(Nz: 0, 1);
        offs!(Fp: 0, 1);
        offs!(Bx: 0, 1);
        offs!((u8, u32, u8): 0, 1, 2);
        offs!((u32, u8, u16, u8): 0, 1, 2, 3);
        offs!((bool, u64): 0, 1);
        offs!((u8, usize, u8): 0, 1, 2);
        offs!((u64, usize, u16): 0, 1, 2);
        offs!(W1: a, b);
        offs!(W2: a, b, c, d);
        offs!(W3: p, len, f);
        offs!(W4: s, n);
        offs!(W5: d, b);
        offs!(W6: v, s, k);
        offs!(W7: a, b);
        offs!(W8: a, b);
        offs!(W9: f, x, y);
        offs!(WC1: a, p, b, n, c);
        offs!(WC2: a, b, c);
        offs!(WC3: a, b);
        offs!(MsgTextureLike: id, width, height, data, live);
        offs!(MsgWindowLike: inner_width, inner_height, dpi, is_fullscreen, title);
        offs!(MsgPtrLike: ptr, len, cap, owner);
        offs!(MsgEventLike: time, id, modifiers, text, abs);
    };
}
