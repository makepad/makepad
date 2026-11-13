// Slice chunk, window and split iterators, including mutable state, arrays and ZSTs.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::fmt::Debug;
use std::iter::FusedIterator;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn obs<T: Obs + ?Sized>(&mut self, v: &T) {
        v.obs(self);
        self.1 += 1;
    }
    fn debug<T: Debug>(&mut self, v: &T) {
        self.obs(format!("{:?}", v).as_bytes());
    }
    /// Debug output with each `addr: 0x..` written as its byte offset from `base`: a pointer's
    /// address differs from run to run, its place in the slice does not.
    fn debug_from<T: Debug>(&mut self, v: &T, base: usize) {
        let s = format!("{:?}", v);
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(k) = rest.find("addr: 0x") {
            out.push_str(&rest[..k + 6]);
            let hex = &rest[k + 8..];
            let n = hex.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(hex.len());
            let a = usize::from_str_radix(&hex[..n], 16).unwrap();
            out.push_str(&format!("+{}", a.wrapping_sub(base)));
            rest = &hex[n..];
        }
        out.push_str(rest);
        self.obs(out.as_bytes());
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}

trait Obs {
    fn obs(&self, d: &mut D);
}
macro_rules! obs_int {
    ($($t:ty),*) => {$(
        impl Obs for $t {
            fn obs(&self, d: &mut D) {
                d.bytes(&self.to_le_bytes());
            }
        }
    )*};
}
obs_int!(u8, i32, usize);
impl<T: Obs> Obs for Option<T> {
    fn obs(&self, d: &mut D) {
        match self {
            None => d.bytes(&[0]),
            Some(v) => {
                d.bytes(&[1]);
                v.obs(d);
            }
        }
    }
}
impl<A: Obs, B: Obs> Obs for (A, B) {
    fn obs(&self, d: &mut D) {
        self.0.obs(d);
        self.1.obs(d);
    }
}
impl<T: Obs + ?Sized> Obs for &T {
    fn obs(&self, d: &mut D) {
        (**self).obs(d);
    }
}
impl<T: Obs + ?Sized> Obs for &mut T {
    fn obs(&self, d: &mut D) {
        (**self).obs(d);
    }
}
impl<T: Obs> Obs for [T] {
    fn obs(&self, d: &mut D) {
        self.len().obs(d);
        for v in self {
            v.obs(d);
        }
    }
}
impl<T: Obs, const N: usize> Obs for [T; N] {
    fn obs(&self, d: &mut D) {
        self.as_slice().obs(d);
    }
}
impl<T: Obs> Obs for Vec<T> {
    fn obs(&self, d: &mut D) {
        self.as_slice().obs(d);
    }
}

fn forward<A>(mut a: A, mode: usize, d: &mut D)
where
    A: Iterator + FusedIterator,
    A::Item: AsRef<[i32]>,
{
    let mut step = 0;
    loop {
        d.obs(&a.size_hint());
        if mode == 5 {
            d.obs(&a.last().map(|x| x.as_ref().to_vec()));
            return;
        }
        if mode == 6 {
            d.obs(&a.count());
            return;
        }
        let n = if mode == 7 { usize::MAX } else { step % 4 };
        let x = if mode == 3 || mode == 7 { a.nth(n) } else { a.next() };
        d.obs(&x.as_ref().map(|v| v.as_ref()));
        if x.is_none() {
            break;
        }
        step += 1;
    }
    d.obs(&a.size_hint());
    assert!(a.next().is_none());
}

fn double<A>(mut a: A, mode: usize, d: &mut D)
where
    A: DoubleEndedIterator + FusedIterator,
    A::Item: AsRef<[i32]>,
{
    if mode == 0 || mode == 3 || mode == 5 || mode == 6 || mode == 7 {
        forward(a, mode, d);
        return;
    }
    let mut step = 0;
    loop {
        d.obs(&a.size_hint());
        let n = if mode == 8 { usize::MAX } else { step % 4 };
        let x = if mode == 4 || mode == 8 {
            a.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            a.next_back()
        } else {
            a.next()
        };
        d.obs(&x.as_ref().map(|v| v.as_ref()));
        if x.is_none() {
            break;
        }
        step += 1;
    }
    d.obs(&a.size_hint());
    assert!(a.next().is_none());
    assert!(a.next_back().is_none());
}

fn change(a: Option<&mut [i32]>, step: usize, d: &mut D) -> bool {
    d.obs(&a.as_deref());
    match a {
        Some(a) => {
            for x in a {
                *x += 100 + step as i32;
            }
            true
        }
        None => false,
    }
}

fn mutable_forward<'a, A>(mut a: A, mode: usize, d: &mut D)
where
    A: Iterator<Item = &'a mut [i32]> + FusedIterator,
{
    let mut step = 0;
    loop {
        d.obs(&a.size_hint());
        if mode == 5 {
            change(a.last(), step, d);
            return;
        }
        if mode == 6 {
            d.obs(&a.count());
            return;
        }
        let n = if mode == 7 { usize::MAX } else { step % 4 };
        let x = if mode == 3 || mode == 7 { a.nth(n) } else { a.next() };
        if !change(x, step, d) {
            break;
        }
        step += 1;
    }
    d.obs(&a.size_hint());
    assert!(a.next().is_none());
}

fn mutable_double<'a, A>(mut a: A, mode: usize, d: &mut D)
where
    A: DoubleEndedIterator<Item = &'a mut [i32]> + FusedIterator,
{
    if mode == 0 || mode == 3 || mode == 5 || mode == 6 || mode == 7 {
        mutable_forward(a, mode, d);
        return;
    }
    let mut step = 0;
    loop {
        d.obs(&a.size_hint());
        let n = if mode == 8 { usize::MAX } else { step % 4 };
        let x = if mode == 4 || mode == 8 {
            a.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            a.next_back()
        } else {
            a.next()
        };
        if !change(x, step, d) {
            break;
        }
        step += 1;
    }
    d.obs(&a.size_hint());
    assert!(a.next().is_none());
    assert!(a.next_back().is_none());
}

fn cloned<A>(a: A, d: &mut D)
where
    A: Clone + DoubleEndedIterator + FusedIterator + Debug,
    A::Item: AsRef<[i32]>,
{
    d.debug(&a);
    double(a.clone(), 2, d);
    double(a, 0, d);
}

fn data(n: usize) -> Vec<i32> {
    (0..n).map(|i| (i * 7 % 11) as i32).collect()
}
fn no(_: &i32) -> bool {
    false
}
fn yes(_: &i32) -> bool {
    true
}
fn even(x: &i32) -> bool {
    x % 2 == 0
}
fn zero(x: &i32) -> bool {
    *x == 0
}
fn eq(a: &i32, b: &i32) -> bool {
    a == b
}
fn le(a: &i32, b: &i32) -> bool {
    a <= b
}
fn all(_: &i32, _: &i32) -> bool {
    true
}
fn none(_: &i32, _: &i32) -> bool {
    false
}

#[test]
fn a01_windows_differential() {
    let mut d = D::new();
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(v.windows(k), &mut d);
            d.obs(&v.windows(k).len());
            for mode in 0..9 {
                double(v.windows(k), mode, &mut d);
            }
            for skip in 0..42 {
                let mut a = v.windows(k);
                d.obs(&a.nth(skip));
                double(a, 2, &mut d);
                let mut a = v.windows(k);
                d.obs(&a.nth_back(skip));
                double(a, 2, &mut d);
            }
        }
    }
    d.line("windows_differential");
}

#[test]
fn a02_chunks_differential() {
    let mut d = D::new();
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(v.chunks(k), &mut d);
            d.obs(&v.chunks(k).len());
            for mode in 0..9 {
                double(v.chunks(k), mode, &mut d);
            }
            for skip in 0..42 {
                let mut a = v.chunks(k);
                d.obs(&a.nth(skip));
                double(a, 2, &mut d);
                let mut a = v.chunks(k);
                d.obs(&a.nth_back(skip));
                double(a, 2, &mut d);
            }
        }
    }
    d.line("chunks_differential");
}

#[test]
fn a03_chunks_exact_differential() {
    let mut d = D::new();
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(v.chunks_exact(k), &mut d);
            d.obs(&v.chunks_exact(k).len());
            for mode in 0..9 {
                double(v.chunks_exact(k), mode, &mut d);
            }
            for skip in 0..42 {
                let mut a = v.chunks_exact(k);
                d.obs(&a.nth(skip));
                double(a, 2, &mut d);
                let mut a = v.chunks_exact(k);
                d.obs(&a.nth_back(skip));
                double(a, 2, &mut d);
            }
        }
    }
    d.line("chunks_exact_differential");
}

#[test]
fn a04_rchunks_differential() {
    let mut d = D::new();
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(v.rchunks(k), &mut d);
            d.obs(&v.rchunks(k).len());
            for mode in 0..9 {
                double(v.rchunks(k), mode, &mut d);
            }
            for skip in 0..42 {
                let mut a = v.rchunks(k);
                d.obs(&a.nth(skip));
                double(a, 2, &mut d);
                let mut a = v.rchunks(k);
                d.obs(&a.nth_back(skip));
                double(a, 2, &mut d);
            }
        }
    }
    d.line("rchunks_differential");
}

#[test]
fn a05_rchunks_exact_differential() {
    let mut d = D::new();
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(v.rchunks_exact(k), &mut d);
            d.obs(&v.rchunks_exact(k).len());
            for mode in 0..9 {
                double(v.rchunks_exact(k), mode, &mut d);
            }
            for skip in 0..42 {
                let mut a = v.rchunks_exact(k);
                d.obs(&a.nth(skip));
                double(a, 2, &mut d);
                let mut a = v.rchunks_exact(k);
                d.obs(&a.nth_back(skip));
                double(a, 2, &mut d);
            }
        }
    }
    d.line("rchunks_exact_differential");
}

#[test]
fn a06_chunks_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut v = data(n);
                d.obs(&v.chunks_mut(k).len());
                mutable_double(v.chunks_mut(k), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("chunks_mut_differential");
}

#[test]
fn a07_chunks_exact_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut v = data(n);
                d.obs(&v.chunks_exact_mut(k).len());
                mutable_double(v.chunks_exact_mut(k), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("chunks_exact_mut_differential");
}

#[test]
fn a08_rchunks_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut v = data(n);
                d.obs(&v.rchunks_mut(k).len());
                mutable_double(v.rchunks_mut(k), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("rchunks_mut_differential");
}

#[test]
fn a09_rchunks_exact_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut v = data(n);
                d.obs(&v.rchunks_exact_mut(k).len());
                mutable_double(v.rchunks_exact_mut(k), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("rchunks_exact_mut_differential");
}

#[test]
fn a10_split_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let v = data(n);
                double(v.split(pred), mode, &mut d);
                cloned(v.split(pred), &mut d);
            }
        }
    }
    d.line("split_differential");
}

#[test]
fn a11_split_inclusive_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let v = data(n);
                double(v.split_inclusive(pred), mode, &mut d);
                cloned(v.split_inclusive(pred), &mut d);
            }
        }
    }
    d.line("split_inclusive_differential");
}

#[test]
fn a12_rsplit_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let v = data(n);
                double(v.rsplit(pred), mode, &mut d);
                cloned(v.rsplit(pred), &mut d);
            }
        }
    }
    d.line("rsplit_differential");
}

#[test]
fn a13_splitn_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let v = data(n);
                    forward(v.splitn(limit, pred), mode, &mut d);
                }
            }
        }
    }
    d.line("splitn_differential");
}

#[test]
fn a14_rsplitn_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let v = data(n);
                    forward(v.rsplitn(limit, pred), mode, &mut d);
                }
            }
        }
    }
    d.line("rsplitn_differential");
}

#[test]
fn a15_split_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut v = data(n);
                mutable_double(v.split_mut(pred), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("split_mut_differential");
}

#[test]
fn a16_split_inclusive_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut v = data(n);
                mutable_double(v.split_inclusive_mut(pred), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("split_inclusive_mut_differential");
}

#[test]
fn a17_rsplit_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut v = data(n);
                mutable_double(v.rsplit_mut(pred), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("rsplit_mut_differential");
}

#[test]
fn a18_splitn_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let mut v = data(n);
                    mutable_forward(v.splitn_mut(limit, pred), mode, &mut d);
                    d.obs(&v);
                }
            }
        }
    }
    d.line("splitn_mut_differential");
}

#[test]
fn a19_rsplitn_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let mut v = data(n);
                    mutable_forward(v.rsplitn_mut(limit, pred), mode, &mut d);
                    d.obs(&v);
                }
            }
        }
    }
    d.line("rsplitn_mut_differential");
}

#[test]
fn a20_chunk_by_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [eq as fn(&i32, &i32) -> bool, le, all, none] {
            for mode in 0..9 {
                let v = data(n);
                double(v.chunk_by(pred), mode, &mut d);
                cloned(v.chunk_by(pred), &mut d);
            }
        }
    }
    d.line("chunk_by_differential");
}

#[test]
fn a21_chunk_by_mut_differential() {
    let mut d = D::new();
    for n in 0..40 {
        for pred in [eq as fn(&i32, &i32) -> bool, le, all, none] {
            for mode in 0..9 {
                let mut v = data(n);
                mutable_double(v.chunk_by_mut(pred), mode, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("chunk_by_mut_differential");
}

#[test]
fn a22_remainders() {
    let mut d = D::new();
    for n in 0..40 {
        for k in 1..8 {
            for skip in 0..42 {
                let v = data(n);
                let mut a = v.chunks_exact(k);
                d.obs(&a.remainder());
                d.obs(&a.nth(skip));
                d.obs(&a.nth_back(skip));
                d.obs(&a.remainder());
                let mut a = v.rchunks_exact(k);
                d.obs(&a.remainder());
                d.obs(&a.nth(skip));
                d.obs(&a.nth_back(skip));
                d.obs(&a.remainder());
                let mut v = data(n);
                let mut a = v.chunks_exact_mut(k);
                let front = a.nth(skip);
                let back = a.nth_back(skip);
                // The yielded borrows remain live with the disjoint remainder.
                change(Some(a.into_remainder()), 1, &mut d);
                change(front, 2, &mut d);
                change(back, 3, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.rchunks_exact_mut(k);
                let front = a.nth(skip);
                let back = a.nth_back(skip);
                // The yielded borrows remain live with the disjoint remainder.
                change(Some(a.into_remainder()), 1, &mut d);
                change(front, 2, &mut d);
                change(back, 3, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("remainders");
}

fn zero_panics(tag: &str, expected: &str, f: impl FnOnce()) {
    let result = catch_unwind(AssertUnwindSafe(f));
    println!("| zero {} {}", tag, result.is_err());
    let payload = result.expect_err("expected zero-size panic");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(message.contains(expected), "unexpected panic message: {}", message);
}

#[test]
fn a23_windows_zero() {
    zero_panics("windows", "window size must be non-zero", || {
        let _ = [1, 2].windows(0);
    });
}

#[test]
fn a24_chunks_zero() {
    zero_panics("chunks", "chunk size must be non-zero", || {
        let _ = [1, 2].chunks(0);
    });
}

#[test]
fn a25_chunks_mut_zero() {
    zero_panics("chunks_mut", "chunk size must be non-zero", || {
        let _ = [1, 2].chunks_mut(0);
    });
}

#[test]
fn a26_chunks_exact_zero() {
    zero_panics("chunks_exact", "chunk size must be non-zero", || {
        let _ = [1, 2].chunks_exact(0);
    });
}

#[test]
fn a27_chunks_exact_mut_zero() {
    zero_panics("chunks_exact_mut", "chunk size must be non-zero", || {
        let _ = [1, 2].chunks_exact_mut(0);
    });
}

#[test]
fn a28_rchunks_zero() {
    zero_panics("rchunks", "chunk size must be non-zero", || {
        let _ = [1, 2].rchunks(0);
    });
}

#[test]
fn a29_rchunks_mut_zero() {
    zero_panics("rchunks_mut", "chunk size must be non-zero", || {
        let _ = [1, 2].rchunks_mut(0);
    });
}

#[test]
fn a30_rchunks_exact_zero() {
    zero_panics("rchunks_exact", "chunk size must be non-zero", || {
        let _ = [1, 2].rchunks_exact(0);
    });
}

#[test]
fn a31_rchunks_exact_mut_zero() {
    zero_panics("rchunks_exact_mut", "chunk size must be non-zero", || {
        let _ = [1, 2].rchunks_exact_mut(0);
    });
}

/// The array-chunk views of `[1, 2]` with N items each: generic, so rustc's deny-by-default
/// unconditional_panic lint does not reject a call with N = 0 at compile time (it panics at run time).
fn chunks_of<const N: usize>(which: u8) {
    let mut a = [1, 2];
    match which {
        0 => { let _ = a.as_chunks::<N>(); }
        1 => { let _ = a.as_chunks_mut::<N>(); }
        2 => { let _ = a.as_rchunks::<N>(); }
        _ => { let _ = a.as_rchunks_mut::<N>(); }
    }
}

#[test]
fn a32_as_chunks_zero() {
    zero_panics("as_chunks", "chunk size must be non-zero", || {
        chunks_of::<0>(0);
    });
}

#[test]
fn a33_as_chunks_mut_zero() {
    zero_panics("as_chunks_mut", "chunk size must be non-zero", || {
        chunks_of::<0>(1);
    });
}

#[test]
fn a34_as_rchunks_zero() {
    zero_panics("as_rchunks", "chunk size must be non-zero", || {
        chunks_of::<0>(2);
    });
}

#[test]
fn a35_as_rchunks_mut_zero() {
    zero_panics("as_rchunks_mut", "chunk size must be non-zero", || {
        chunks_of::<0>(3);
    });
}

fn arrays<const N: usize>(d: &mut D) {
    for n in 0..40 {
        let v = data(n);
        d.obs(&v.as_chunks::<N>());
        d.obs(&v.as_rchunks::<N>());
        let mut a = v.clone();
        let (ac, ar) = a.as_chunks_mut::<N>();
        d.obs(ac);
        change(Some(ar), 0, d);
        for x in ac {
            change(Some(x), 1, d);
        }
        d.obs(&a);
        let (ar, ac) = a.as_rchunks_mut::<N>();
        d.obs(ac);
        change(Some(ar), 2, d);
        for x in ac {
            change(Some(x), 3, d);
        }
        d.obs(&a);
    }
}

fn split_arrays<const N: usize>(d: &mut D) {
    for n in 0..40 {
        let mut a = data(n);
        let x = a.split_first_chunk_mut::<N>();
        d.obs(&x);
        if let Some((x, xr)) = x {
            change(Some(x), 0, d);
            change(Some(xr), 1, d);
        }
        d.obs(&a);
        let x = a.split_last_chunk_mut::<N>();
        d.obs(&x);
        if let Some((xr, x)) = x {
            change(Some(x), 2, d);
            change(Some(xr), 3, d);
        }
        d.obs(&a);
    }
}

#[test]
fn a36_array_views() {
    let mut d = D::new();
    arrays::<1>(&mut d);
    arrays::<2>(&mut d);
    arrays::<3>(&mut d);
    arrays::<4>(&mut d);
    arrays::<5>(&mut d);
    arrays::<6>(&mut d);
    arrays::<7>(&mut d);
    split_arrays::<0>(&mut d);
    split_arrays::<1>(&mut d);
    split_arrays::<2>(&mut d);
    split_arrays::<7>(&mut d);
    split_arrays::<41>(&mut d);
    let mut z = [(); 17];
    assert_eq!(z.as_chunks::<3>().0.len(), 5);
    assert_eq!(z.as_rchunks_mut::<3>().0.len(), 2);
    d.line("array_views");
}

#[test]
fn a37_predicate_order() {
    let mut d = D::new();
    use std::cell::RefCell;
    let v = data(39);
    for mode in 0..9 {
        let trace = RefCell::new(Vec::new());
        double(
            v.split(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        double(
            v.split_inclusive(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        double(
            v.rsplit(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        forward(
            v.splitn(5, |x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        forward(
            v.rsplitn(5, |x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        double(
            v.chunk_by(|x, y| {
                let mut t = trace.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_double(
            a.split_mut(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_double(
            a.split_inclusive_mut(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_double(
            a.rsplit_mut(|x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_forward(
            a.splitn_mut(5, |x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_forward(
            a.rsplitn_mut(5, |x| {
                let mut t = trace.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
        let trace = RefCell::new(Vec::new());
        let mut a = v.clone();
        mutable_double(
            a.chunk_by_mut(|x, y| {
                let mut t = trace.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            mode,
            &mut d,
        );
        d.obs(&*trace.borrow());
        d.obs(&a);
    }
    d.line("predicate_order");
}

#[test]
fn a38_skipped_mutations() {
    let mut d = D::new();
    for n in 0..40 {
        for k in 1..8 {
            for skip in 0..42 {
                let mut v = data(n);
                let mut a = v.chunks_mut(k);
                change(a.nth(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.chunks_mut(k);
                change(a.nth_back(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.chunks_exact_mut(k);
                change(a.nth(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.chunks_exact_mut(k);
                change(a.nth_back(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.rchunks_mut(k);
                change(a.nth(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.rchunks_mut(k);
                change(a.nth_back(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.rchunks_exact_mut(k);
                change(a.nth(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
                let mut v = data(n);
                let mut a = v.rchunks_exact_mut(k);
                change(a.nth_back(skip), 0, &mut d);
                mutable_double(a, 2, &mut d);
                d.obs(&v);
            }
        }
    }
    d.line("skipped_mutations");
}

#[test]
fn a39_partial_last_and_count() {
    let mut d = D::new();
    for n in 0..40 {
        for mode in [5, 6] {
            let v = data(n);
            let mut a = v.windows(3);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.chunks(3);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.chunks_exact(3);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.rchunks(3);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.rchunks_exact(3);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.split(even);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.split_inclusive(even);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.rsplit(even);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut a = v.chunk_by(le);
            d.obs(&a.next());
            d.obs(&a.next_back());
            double(a, mode, &mut d);
            let mut av = v.clone();
            let mut a = av.split_mut(even);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.split_inclusive_mut(even);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.rsplit_mut(even);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.chunk_by_mut(le);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.chunks_mut(3);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.chunks_exact_mut(3);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.rchunks_mut(3);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
            let mut av = v.clone();
            let mut a = av.rchunks_exact_mut(3);
            change(a.next(), 0, &mut d);
            change(a.next_back(), 1, &mut d);
            mutable_double(a, mode, &mut d);
            d.obs(&av);
        }
    }
    d.line("partial_last_and_count");
}

#[test]
fn a40_huge_zero_sized_slices() {
    let mut d = D::new();
    // Zero-sized elements occupy no bytes even at the maximum slice length.
    let ptr = std::ptr::NonNull::<()>::dangling().as_ptr();
    let v = unsafe { std::slice::from_raw_parts(ptr, usize::MAX) };
    for k in [1, 2, 7, usize::MAX] {
        let mut a = v.windows(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let mut a = v.chunks(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let mut a = v.chunks_exact(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let mut a = v.rchunks(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let mut a = v.rchunks_exact(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = av.chunks_mut(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = av.chunks_exact_mut(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = av.rchunks_mut(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = av.rchunks_exact_mut(k);
        d.obs(&a.size_hint());
        d.obs(&a.nth(usize::MAX / 2).map(|x| x.len()));
        d.obs(&a.nth_back(1).map(|x| x.len()));
        d.obs(&a.size_hint());
        d.obs(&a.last().map(|x| x.len()));
    }
    d.line("huge_zero_sized_slices");
}

#[test]
fn a41_debug_remaining_state() {
    let mut d = D::new();
    let mut v = data(13);
    let base = v.as_ptr() as usize;
    {
        let mut it = v.chunks_mut(3);
        let _ = it.next();
        d.debug_from(&it, base);
    }
    {
        let mut it = v.chunks_exact_mut(3);
        let _ = it.next();
        d.debug_from(&it, base);
    }
    {
        let mut it = v.rchunks_mut(3);
        let _ = it.next();
        d.debug_from(&it, base);
    }
    {
        let mut it = v.rchunks_exact_mut(3);
        let _ = it.next();
        d.debug_from(&it, base);
    }
    {
        let mut it = v.split_mut(even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.split_inclusive_mut(even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.rsplit_mut(even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.splitn(3, even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.splitn_mut(3, even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.rsplitn(3, even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.rsplitn_mut(3, even);
        let _ = it.next();
        d.debug(&it);
    }
    {
        let mut it = v.chunk_by_mut(le);
        let _ = it.next();
        d.debug(&it);
    }
    d.line("debug_remaining_state");
}
