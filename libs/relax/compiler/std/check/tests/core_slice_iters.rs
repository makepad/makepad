//! Differential coverage against core, including iterator state after skipped yields.
use relax_std_check::slice as hs;
use std::fmt::Debug;
use std::iter::FusedIterator;

fn forward<A, B>(mut a: A, mut b: B, mode: usize)
where
    A: Iterator + FusedIterator,
    B: Iterator,
    A::Item: AsRef<[i32]>,
    B::Item: AsRef<[i32]>,
{
    let mut step = 0;
    loop {
        assert_eq!(a.size_hint(), b.size_hint());
        if mode == 5 {
            assert_eq!(
                a.last().map(|x| x.as_ref().to_vec()),
                b.last().map(|x| x.as_ref().to_vec())
            );
            return;
        }
        if mode == 6 {
            assert_eq!(a.count(), b.count());
            return;
        }
        let n = if mode == 7 { usize::MAX } else { step % 4 };
        let x = if mode == 3 || mode == 7 {
            a.nth(n)
        } else {
            a.next()
        };
        let y = if mode == 3 || mode == 7 {
            b.nth(n)
        } else {
            b.next()
        };
        assert_eq!(
            x.as_ref().map(|v| v.as_ref()),
            y.as_ref().map(|v| v.as_ref())
        );
        if x.is_none() {
            break;
        }
        step += 1;
    }
    assert_eq!(a.size_hint(), b.size_hint());
    assert!(a.next().is_none());
    assert!(b.next().is_none());
}

fn double<A, B>(mut a: A, mut b: B, mode: usize)
where
    A: DoubleEndedIterator + FusedIterator,
    B: DoubleEndedIterator,
    A::Item: AsRef<[i32]>,
    B::Item: AsRef<[i32]>,
{
    if mode == 0 || mode == 3 || mode == 5 || mode == 6 || mode == 7 {
        forward(a, b, mode);
        return;
    }
    let mut step = 0;
    loop {
        assert_eq!(a.size_hint(), b.size_hint());
        let n = if mode == 8 { usize::MAX } else { step % 4 };
        let x = if mode == 4 || mode == 8 {
            a.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            a.next_back()
        } else {
            a.next()
        };
        let y = if mode == 4 || mode == 8 {
            b.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            b.next_back()
        } else {
            b.next()
        };
        assert_eq!(
            x.as_ref().map(|v| v.as_ref()),
            y.as_ref().map(|v| v.as_ref())
        );
        if x.is_none() {
            break;
        }
        step += 1;
    }
    assert_eq!(a.size_hint(), b.size_hint());
    assert!(a.next().is_none());
    assert!(a.next_back().is_none());
}

fn change(a: Option<&mut [i32]>, b: Option<&mut [i32]>, step: usize) -> bool {
    assert_eq!(a.as_deref(), b.as_deref());
    match (a, b) {
        (Some(a), Some(b)) => {
            for x in a {
                *x += 100 + step as i32;
            }
            for x in b {
                *x += 100 + step as i32;
            }
            true
        }
        (None, None) => false,
        _ => unreachable!(),
    }
}
fn mutable_forward<'a, 'b, A, B>(mut a: A, mut b: B, mode: usize)
where
    A: Iterator<Item = &'a mut [i32]> + FusedIterator,
    B: Iterator<Item = &'b mut [i32]>,
{
    let mut step = 0;
    loop {
        assert_eq!(a.size_hint(), b.size_hint());
        if mode == 5 {
            change(a.last(), b.last(), step);
            return;
        }
        if mode == 6 {
            assert_eq!(a.count(), b.count());
            return;
        }
        let n = if mode == 7 { usize::MAX } else { step % 4 };
        let x = if mode == 3 || mode == 7 {
            a.nth(n)
        } else {
            a.next()
        };
        let y = if mode == 3 || mode == 7 {
            b.nth(n)
        } else {
            b.next()
        };
        if !change(x, y, step) {
            break;
        }
        step += 1;
    }
    assert_eq!(a.size_hint(), b.size_hint());
    assert!(a.next().is_none());
}
fn mutable_double<'a, 'b, A, B>(mut a: A, mut b: B, mode: usize)
where
    A: DoubleEndedIterator<Item = &'a mut [i32]> + FusedIterator,
    B: DoubleEndedIterator<Item = &'b mut [i32]>,
{
    if mode == 0 || mode == 3 || mode == 5 || mode == 6 || mode == 7 {
        mutable_forward(a, b, mode);
        return;
    }
    let mut step = 0;
    loop {
        assert_eq!(a.size_hint(), b.size_hint());
        let n = if mode == 8 { usize::MAX } else { step % 4 };
        let x = if mode == 4 || mode == 8 {
            a.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            a.next_back()
        } else {
            a.next()
        };
        let y = if mode == 4 || mode == 8 {
            b.nth_back(n)
        } else if mode == 1 || step % 2 == 0 {
            b.next_back()
        } else {
            b.next()
        };
        if !change(x, y, step) {
            break;
        }
        step += 1;
    }
    assert_eq!(a.size_hint(), b.size_hint());
    assert!(a.next().is_none());
    assert!(a.next_back().is_none());
}
fn cloned<A, B>(a: A, b: B)
where
    A: Clone + DoubleEndedIterator + FusedIterator + relax_std_check::fmt::Debug,
    B: Clone + DoubleEndedIterator + Debug,
    A::Item: AsRef<[i32]>,
    B::Item: AsRef<[i32]>,
{
    assert_eq!(debug(&a), format!("{:?}", b));
    double(a.clone(), b.clone(), 2);
    double(a, b, 0);
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
fn windows_differential() {
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(hs::windows(&v, k), v.windows(k));
            assert_eq!(hs::windows(&v, k).len(), v.windows(k).len());
            for mode in 0..9 {
                double(hs::windows(&v, k), v.windows(k), mode);
            }
            for skip in 0..42 {
                let mut a = hs::windows(&v, k);
                let mut b = v.windows(k);
                assert_eq!(a.nth(skip), b.nth(skip));
                double(a, b, 2);
                let mut a = hs::windows(&v, k);
                let mut b = v.windows(k);
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                double(a, b, 2);
            }
        }
    }
}
#[test]
fn chunks_differential() {
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(hs::chunks(&v, k), v.chunks(k));
            assert_eq!(hs::chunks(&v, k).len(), v.chunks(k).len());
            for mode in 0..9 {
                double(hs::chunks(&v, k), v.chunks(k), mode);
            }
            for skip in 0..42 {
                let mut a = hs::chunks(&v, k);
                let mut b = v.chunks(k);
                assert_eq!(a.nth(skip), b.nth(skip));
                double(a, b, 2);
                let mut a = hs::chunks(&v, k);
                let mut b = v.chunks(k);
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                double(a, b, 2);
            }
        }
    }
}
#[test]
fn chunks_exact_differential() {
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(hs::chunks_exact(&v, k), v.chunks_exact(k));
            assert_eq!(hs::chunks_exact(&v, k).len(), v.chunks_exact(k).len());
            for mode in 0..9 {
                double(hs::chunks_exact(&v, k), v.chunks_exact(k), mode);
            }
            for skip in 0..42 {
                let mut a = hs::chunks_exact(&v, k);
                let mut b = v.chunks_exact(k);
                assert_eq!(a.nth(skip), b.nth(skip));
                double(a, b, 2);
                let mut a = hs::chunks_exact(&v, k);
                let mut b = v.chunks_exact(k);
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                double(a, b, 2);
            }
        }
    }
}
#[test]
fn rchunks_differential() {
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(hs::rchunks(&v, k), v.rchunks(k));
            assert_eq!(hs::rchunks(&v, k).len(), v.rchunks(k).len());
            for mode in 0..9 {
                double(hs::rchunks(&v, k), v.rchunks(k), mode);
            }
            for skip in 0..42 {
                let mut a = hs::rchunks(&v, k);
                let mut b = v.rchunks(k);
                assert_eq!(a.nth(skip), b.nth(skip));
                double(a, b, 2);
                let mut a = hs::rchunks(&v, k);
                let mut b = v.rchunks(k);
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                double(a, b, 2);
            }
        }
    }
}
#[test]
fn rchunks_exact_differential() {
    for n in 0..40 {
        let v = data(n);
        for k in (1..8).chain([usize::MAX]) {
            cloned(hs::rchunks_exact(&v, k), v.rchunks_exact(k));
            assert_eq!(hs::rchunks_exact(&v, k).len(), v.rchunks_exact(k).len());
            for mode in 0..9 {
                double(hs::rchunks_exact(&v, k), v.rchunks_exact(k), mode);
            }
            for skip in 0..42 {
                let mut a = hs::rchunks_exact(&v, k);
                let mut b = v.rchunks_exact(k);
                assert_eq!(a.nth(skip), b.nth(skip));
                double(a, b, 2);
                let mut a = hs::rchunks_exact(&v, k);
                let mut b = v.rchunks_exact(k);
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                double(a, b, 2);
            }
        }
    }
}
#[test]
fn chunks_mut_differential() {
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                assert_eq!(hs::chunks_mut(&mut a, k).len(), b.chunks_mut(k).len());
                mutable_double(hs::chunks_mut(&mut a, k), b.chunks_mut(k), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn chunks_exact_mut_differential() {
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                assert_eq!(
                    hs::chunks_exact_mut(&mut a, k).len(),
                    b.chunks_exact_mut(k).len()
                );
                mutable_double(hs::chunks_exact_mut(&mut a, k), b.chunks_exact_mut(k), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn rchunks_mut_differential() {
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                assert_eq!(hs::rchunks_mut(&mut a, k).len(), b.rchunks_mut(k).len());
                mutable_double(hs::rchunks_mut(&mut a, k), b.rchunks_mut(k), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn rchunks_exact_mut_differential() {
    for n in 0..40 {
        for k in (1..8).chain([usize::MAX]) {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                assert_eq!(
                    hs::rchunks_exact_mut(&mut a, k).len(),
                    b.rchunks_exact_mut(k).len()
                );
                mutable_double(
                    hs::rchunks_exact_mut(&mut a, k),
                    b.rchunks_exact_mut(k),
                    mode,
                );
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn split_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let a = data(n);
                let b = a.clone();
                double(hs::split(&a, pred), b.split(pred), mode);
                cloned(hs::split(&a, pred), b.split(pred));
            }
        }
    }
}
#[test]
fn split_inclusive_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let a = data(n);
                let b = a.clone();
                double(hs::split_inclusive(&a, pred), b.split_inclusive(pred), mode);
                cloned(hs::split_inclusive(&a, pred), b.split_inclusive(pred));
            }
        }
    }
}
#[test]
fn rsplit_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let a = data(n);
                let b = a.clone();
                double(hs::rsplit(&a, pred), b.rsplit(pred), mode);
                cloned(hs::rsplit(&a, pred), b.rsplit(pred));
            }
        }
    }
}
#[test]
fn splitn_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let a = data(n);
                    let b = a.clone();
                    forward(hs::splitn(&a, limit, pred), b.splitn(limit, pred), mode);
                }
            }
        }
    }
}
#[test]
fn rsplitn_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let a = data(n);
                    let b = a.clone();
                    forward(hs::rsplitn(&a, limit, pred), b.rsplitn(limit, pred), mode);
                }
            }
        }
    }
}
#[test]
fn split_mut_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                mutable_double(hs::split_mut(&mut a, pred), b.split_mut(pred), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn split_inclusive_mut_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                mutable_double(
                    hs::split_inclusive_mut(&mut a, pred),
                    b.split_inclusive_mut(pred),
                    mode,
                );
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn rsplit_mut_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                mutable_double(hs::rsplit_mut(&mut a, pred), b.rsplit_mut(pred), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn splitn_mut_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let mut a = data(n);
                    let mut b = a.clone();
                    mutable_forward(
                        hs::splitn_mut(&mut a, limit, pred),
                        b.splitn_mut(limit, pred),
                        mode,
                    );
                    assert_eq!(a, b);
                }
            }
        }
    }
}
#[test]
fn rsplitn_mut_differential() {
    for n in 0..40 {
        for pred in [no as fn(&i32) -> bool, yes, even, zero] {
            for limit in [0, 1, 2, 5, 40, usize::MAX] {
                for mode in 0..9 {
                    let mut a = data(n);
                    let mut b = a.clone();
                    mutable_forward(
                        hs::rsplitn_mut(&mut a, limit, pred),
                        b.rsplitn_mut(limit, pred),
                        mode,
                    );
                    assert_eq!(a, b);
                }
            }
        }
    }
}
#[test]
fn chunk_by_differential() {
    for n in 0..40 {
        for pred in [eq as fn(&i32, &i32) -> bool, le, all, none] {
            for mode in 0..9 {
                let a = data(n);
                let b = a.clone();
                double(hs::chunk_by(&a, pred), b.chunk_by(pred), mode);
                cloned(hs::chunk_by(&a, pred), b.chunk_by(pred));
            }
        }
    }
}
#[test]
fn chunk_by_mut_differential() {
    for n in 0..40 {
        for pred in [eq as fn(&i32, &i32) -> bool, le, all, none] {
            for mode in 0..9 {
                let mut a = data(n);
                let mut b = a.clone();
                mutable_double(hs::chunk_by_mut(&mut a, pred), b.chunk_by_mut(pred), mode);
                assert_eq!(a, b);
            }
        }
    }
}
#[test]
fn remainders() {
    for n in 0..40 {
        for k in 1..8 {
            for skip in 0..42 {
                let v = data(n);
                let mut a = hs::chunks_exact(&v, k);
                let mut b = v.chunks_exact(k);
                assert_eq!(a.remainder(), b.remainder());
                assert_eq!(a.nth(skip), b.nth(skip));
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                assert_eq!(a.remainder(), b.remainder());
                let mut a = hs::rchunks_exact(&v, k);
                let mut b = v.rchunks_exact(k);
                assert_eq!(a.remainder(), b.remainder());
                assert_eq!(a.nth(skip), b.nth(skip));
                assert_eq!(a.nth_back(skip), b.nth_back(skip));
                assert_eq!(a.remainder(), b.remainder());
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::chunks_exact_mut(&mut av, k);
                let mut b = bv.chunks_exact_mut(k);
                // Keep yielded borrows live together with the remainder: they must be disjoint.
                let front_a = a.nth(skip);
                let front_b = b.nth(skip);
                let back_a = a.nth_back(skip);
                let back_b = b.nth_back(skip);
                change(Some(a.into_remainder()), Some(b.into_remainder()), 1);
                change(front_a, front_b, 2);
                change(back_a, back_b, 3);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::rchunks_exact_mut(&mut av, k);
                let mut b = bv.rchunks_exact_mut(k);
                // Keep yielded borrows live together with the remainder: they must be disjoint.
                let front_a = a.nth(skip);
                let front_b = b.nth(skip);
                let back_a = a.nth_back(skip);
                let back_b = b.nth_back(skip);
                change(Some(a.into_remainder()), Some(b.into_remainder()), 1);
                change(front_a, front_b, 2);
                change(back_a, back_b, 3);
                assert_eq!(av, bv);
            }
        }
    }
}
#[test]
#[should_panic(expected = "window size must be non-zero")]
fn windows_zero() {
    let _ = hs::windows(&[1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn chunks_zero() {
    let _ = hs::chunks(&[1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn chunks_mut_zero() {
    let _ = hs::chunks_mut(&mut [1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn chunks_exact_zero() {
    let _ = hs::chunks_exact(&[1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn chunks_exact_mut_zero() {
    let _ = hs::chunks_exact_mut(&mut [1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn rchunks_zero() {
    let _ = hs::rchunks(&[1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn rchunks_mut_zero() {
    let _ = hs::rchunks_mut(&mut [1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn rchunks_exact_zero() {
    let _ = hs::rchunks_exact(&[1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn rchunks_exact_mut_zero() {
    let _ = hs::rchunks_exact_mut(&mut [1, 2], 0);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn as_chunks_zero() {
    let _ = hs::as_chunks::<_, 0>(&[1, 2]);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn as_chunks_mut_zero() {
    let _ = hs::as_chunks_mut::<_, 0>(&mut [1, 2]);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn as_rchunks_zero() {
    let _ = hs::as_rchunks::<_, 0>(&[1, 2]);
}
#[test]
#[should_panic(expected = "chunk size must be non-zero")]
fn as_rchunks_mut_zero() {
    let _ = hs::as_rchunks_mut::<_, 0>(&mut [1, 2]);
}
fn arrays<const N: usize>() {
    for n in 0..40 {
        let v = data(n);
        assert_eq!(hs::as_chunks::<_, N>(&v), v.as_chunks::<N>());
        assert_eq!(hs::as_rchunks::<_, N>(&v), v.as_rchunks::<N>());
        let mut a = v.clone();
        let mut b = v.clone();
        let (ac, ar) = hs::as_chunks_mut::<_, N>(&mut a);
        let (bc, br) = b.as_chunks_mut::<N>();
        assert_eq!(ac, bc);
        change(Some(ar), Some(br), 0);
        for (x, y) in ac.iter_mut().zip(bc) {
            change(Some(x), Some(y), 1);
        }
        assert_eq!(a, b);
        let (ar, ac) = hs::as_rchunks_mut::<_, N>(&mut a);
        let (br, bc) = b.as_rchunks_mut::<N>();
        assert_eq!(ac, bc);
        change(Some(ar), Some(br), 2);
        for (x, y) in ac.iter_mut().zip(bc) {
            change(Some(x), Some(y), 3);
        }
        assert_eq!(a, b);
    }
}
fn split_arrays<const N: usize>() {
    for n in 0..40 {
        let mut a = data(n);
        let mut b = a.clone();
        let x = hs::split_first_chunk_mut::<_, N>(&mut a);
        let y = b.split_first_chunk_mut::<N>();
        assert_eq!(x, y);
        if let (Some((x, xr)), Some((y, yr))) = (x, y) {
            change(Some(x), Some(y), 0);
            change(Some(xr), Some(yr), 1);
        }
        assert_eq!(a, b);
        let x = hs::split_last_chunk_mut::<_, N>(&mut a);
        let y = b.split_last_chunk_mut::<N>();
        assert_eq!(x, y);
        if let (Some((xr, x)), Some((yr, y))) = (x, y) {
            change(Some(x), Some(y), 2);
            change(Some(xr), Some(yr), 3);
        }
        assert_eq!(a, b);
    }
}
#[test]
fn array_views() {
    arrays::<1>();
    arrays::<2>();
    arrays::<3>();
    arrays::<4>();
    arrays::<5>();
    arrays::<6>();
    arrays::<7>();
    split_arrays::<0>();
    split_arrays::<1>();
    split_arrays::<2>();
    split_arrays::<7>();
    split_arrays::<41>();
    let mut z = [(); 17];
    assert_eq!(hs::as_chunks::<_, 3>(&z).0.len(), 5);
    assert_eq!(hs::as_rchunks_mut::<_, 3>(&mut z).0.len(), 2);
}
#[test]
fn predicate_order() {
    use std::cell::RefCell;
    let v = data(39);
    for mode in 0..9 {
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        double(
            hs::split(&v, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            v.split(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        double(
            hs::split_inclusive(&v, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            v.split_inclusive(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        double(
            hs::rsplit(&v, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            v.rsplit(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        forward(
            hs::splitn(&v, 5, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            v.splitn(5, |x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        forward(
            hs::rsplitn(&v, 5, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            v.rsplitn(5, |x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        double(
            hs::chunk_by(&v, |x, y| {
                let mut t = ta.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            v.chunk_by(|x, y| {
                let mut t = tb.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_double(
            hs::split_mut(&mut a, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            b.split_mut(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_double(
            hs::split_inclusive_mut(&mut a, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            b.split_inclusive_mut(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_double(
            hs::rsplit_mut(&mut a, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            b.rsplit_mut(|x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_forward(
            hs::splitn_mut(&mut a, 5, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            b.splitn_mut(5, |x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_forward(
            hs::rsplitn_mut(&mut a, 5, |x| {
                let mut t = ta.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            b.rsplitn_mut(5, |x| {
                let mut t = tb.borrow_mut();
                t.push(*x);
                t.len() % 3 == 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
        let ta = RefCell::new(Vec::new());
        let tb = RefCell::new(Vec::new());
        let mut a = v.clone();
        let mut b = v.clone();
        mutable_double(
            hs::chunk_by_mut(&mut a, |x, y| {
                let mut t = ta.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            b.chunk_by_mut(|x, y| {
                let mut t = tb.borrow_mut();
                t.push((*x, *y));
                t.len() % 3 != 0
            }),
            mode,
        );
        assert_eq!(ta, tb);
        assert_eq!(a, b);
    }
}

fn debug<T: relax_std_check::fmt::Debug>(value: &T) -> String {
    let mut out = relax_std_check::string::String::new();
    let mut f = relax_std_check::fmt::Formatter::new(&mut out);
    value.fmt(&mut f).unwrap();
    out.as_str().to_owned()
}

#[test]
fn skipped_mutations() {
    for n in 0..40 {
        for k in 1..8 {
            for skip in 0..42 {
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::chunks_mut(&mut av, k);
                let mut b = bv.chunks_mut(k);
                change(a.nth(skip), b.nth(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::chunks_mut(&mut av, k);
                let mut b = bv.chunks_mut(k);
                change(a.nth_back(skip), b.nth_back(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::chunks_exact_mut(&mut av, k);
                let mut b = bv.chunks_exact_mut(k);
                change(a.nth(skip), b.nth(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::chunks_exact_mut(&mut av, k);
                let mut b = bv.chunks_exact_mut(k);
                change(a.nth_back(skip), b.nth_back(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::rchunks_mut(&mut av, k);
                let mut b = bv.rchunks_mut(k);
                change(a.nth(skip), b.nth(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::rchunks_mut(&mut av, k);
                let mut b = bv.rchunks_mut(k);
                change(a.nth_back(skip), b.nth_back(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::rchunks_exact_mut(&mut av, k);
                let mut b = bv.rchunks_exact_mut(k);
                change(a.nth(skip), b.nth(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
                let mut av = data(n);
                let mut bv = av.clone();
                let mut a = hs::rchunks_exact_mut(&mut av, k);
                let mut b = bv.rchunks_exact_mut(k);
                change(a.nth_back(skip), b.nth_back(skip), 0);
                mutable_double(a, b, 2);
                assert_eq!(av, bv);
            }
        }
    }
}
#[test]
fn partial_last_and_count() {
    for n in 0..40 {
        for mode in [5, 6] {
            let v = data(n);
            let mut a = hs::windows(&v, 3);
            let mut b = v.windows(3);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::chunks(&v, 3);
            let mut b = v.chunks(3);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::chunks_exact(&v, 3);
            let mut b = v.chunks_exact(3);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::rchunks(&v, 3);
            let mut b = v.rchunks(3);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::rchunks_exact(&v, 3);
            let mut b = v.rchunks_exact(3);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::split(&v, even);
            let mut b = v.split(even);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::split_inclusive(&v, even);
            let mut b = v.split_inclusive(even);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::rsplit(&v, even);
            let mut b = v.rsplit(even);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut a = hs::chunk_by(&v, le);
            let mut b = v.chunk_by(le);
            assert_eq!(a.next(), b.next());
            assert_eq!(a.next_back(), b.next_back());
            double(a, b, mode);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::split_mut(&mut av, even);
            let mut b = bv.split_mut(even);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::split_inclusive_mut(&mut av, even);
            let mut b = bv.split_inclusive_mut(even);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::rsplit_mut(&mut av, even);
            let mut b = bv.rsplit_mut(even);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::chunk_by_mut(&mut av, le);
            let mut b = bv.chunk_by_mut(le);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::chunks_mut(&mut av, 3);
            let mut b = bv.chunks_mut(3);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::chunks_exact_mut(&mut av, 3);
            let mut b = bv.chunks_exact_mut(3);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::rchunks_mut(&mut av, 3);
            let mut b = bv.rchunks_mut(3);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
            let mut av = v.clone();
            let mut bv = v.clone();
            let mut a = hs::rchunks_exact_mut(&mut av, 3);
            let mut b = bv.rchunks_exact_mut(3);
            change(a.next(), b.next(), 0);
            change(a.next_back(), b.next_back(), 1);
            mutable_double(a, b, mode);
            assert_eq!(av, bv);
        }
    }
}
#[test]
fn huge_zero_sized_slices() {
    // A ZST slice can legitimately have usize::MAX elements without an allocation.
    let ptr = std::ptr::NonNull::<()>::dangling().as_ptr();
    let v = unsafe { std::slice::from_raw_parts(ptr, usize::MAX) };
    for k in [1, 2, 7, usize::MAX] {
        let mut a = hs::windows(v, k);
        let mut b = v.windows(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let mut a = hs::chunks(v, k);
        let mut b = v.chunks(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let mut a = hs::chunks_exact(v, k);
        let mut b = v.chunks_exact(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let mut a = hs::rchunks(v, k);
        let mut b = v.rchunks(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let mut a = hs::rchunks_exact(v, k);
        let mut b = v.rchunks_exact(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let bv = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = hs::chunks_mut(av, k);
        let mut b = bv.chunks_mut(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let bv = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = hs::chunks_exact_mut(av, k);
        let mut b = bv.chunks_exact_mut(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let bv = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = hs::rchunks_mut(av, k);
        let mut b = bv.rchunks_mut(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
        let av = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let bv = unsafe { std::slice::from_raw_parts_mut(ptr, usize::MAX) };
        let mut a = hs::rchunks_exact_mut(av, k);
        let mut b = bv.rchunks_exact_mut(k);
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(
            a.nth(usize::MAX / 2).map(|x| x.len()),
            b.nth(usize::MAX / 2).map(|x| x.len())
        );
        assert_eq!(
            a.nth_back(1).map(|x| x.len()),
            b.nth_back(1).map(|x| x.len())
        );
        assert_eq!(a.size_hint(), b.size_hint());
        assert_eq!(a.last().map(|x| x.len()), b.last().map(|x| x.len()));
    }
}

#[test]
fn debug_remaining_state() {
    let mut v = data(13);
    let a = {
        let mut it = hs::chunks_mut(&mut v, 3);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.chunks_mut(3);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::chunks_exact_mut(&mut v, 3);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.chunks_exact_mut(3);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::rchunks_mut(&mut v, 3);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.rchunks_mut(3);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::rchunks_exact_mut(&mut v, 3);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.rchunks_exact_mut(3);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::split_mut(&mut v, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.split_mut(even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::split_inclusive_mut(&mut v, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.split_inclusive_mut(even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::rsplit_mut(&mut v, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.rsplit_mut(even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::splitn(&v, 3, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.splitn(3, even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::splitn_mut(&mut v, 3, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.splitn_mut(3, even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::rsplitn(&v, 3, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.rsplitn(3, even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::rsplitn_mut(&mut v, 3, even);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.rsplitn_mut(3, even);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
    let a = {
        let mut it = hs::chunk_by_mut(&mut v, le);
        it.next();
        debug(&it)
    };
    let b = {
        let mut it = v.chunk_by_mut(le);
        it.next();
        format!("{:?}", it)
    };
    assert_eq!(a, b);
}
