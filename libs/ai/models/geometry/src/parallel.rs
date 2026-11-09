//! Spreading independent items over threads so that the results do not depend on the thread count: the items are
//! split into chunks by their count alone, and the chunks' results are folded in chunk order.

use std::{collections::BTreeMap, ops::Range, sync::Mutex};

/// Runs `f(i)` once for every `i < len`, on any threads and in any order, and returns when every call has returned.
/// An app passes its task pool's fan-out (`&|len, f| pool.fan_out(len, f)`), a tool its own threads, and a caller
/// without threads [`serial`].
pub type FanOut<'a> = &'a (dyn Fn(usize, &(dyn Fn(usize) + Sync)) + Sync);

/// The fan-out on the calling thread alone, in index order.
pub fn serial(len: usize, f: &(dyn Fn(usize) + Sync)) {
    (0..len).for_each(f);
}

/// Maps the chunks of `chunk` items of `0..len` (the last may be shorter) over `fan_out`, and folds their results
/// into `init` in chunk order as they arrive: the result is the same at any thread count, and only the chunks that
/// finish ahead of an earlier one wait in memory.
///
/// Panics if `chunk` is zero or `fan_out` did not run every chunk.
pub(crate) fn fold_chunks<T: Send, A: Send>(
    fan_out: FanOut, len: usize, chunk: usize, init: A, map: impl Fn(Range<usize>) -> T + Sync, fold: impl Fn(&mut A, T) + Sync,
) -> A {
    struct Folding<T, A> {
        next: usize,
        pending: BTreeMap<usize, T>,
        total: A,
    }
    assert!(chunk > 0, "chunk must be positive");
    let chunks = len.div_ceil(chunk);
    let state = Mutex::new(Folding { next: 0, pending: BTreeMap::new(), total: init });
    fan_out(chunks, &|c| {
        let result = map(c * chunk..(c * chunk + chunk).min(len));
        let mut guard = state.lock().unwrap();
        let folding = &mut *guard;
        folding.pending.insert(c, result);
        while let Some(result) = folding.pending.remove(&folding.next) {
            fold(&mut folding.total, result);
            folding.next += 1;
        }
    });
    let folding = state.into_inner().unwrap();
    assert!(folding.next == chunks, "the fan-out did not run every chunk");
    folding.total
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fan-out over `count` scoped threads taking indices in turn.
    pub(crate) fn threads(count: usize) -> impl Fn(usize, &(dyn Fn(usize) + Sync)) + Sync {
        move |len, f| {
            let next = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                for _ in 0..count {
                    scope.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= len {
                            break;
                        }
                        f(i);
                    });
                }
            });
        }
    }

    /// A fan-out on the calling thread in reverse index order: every chunk but the first arrives early.
    pub(crate) fn reversed(len: usize, f: &(dyn Fn(usize) + Sync)) {
        (0..len).rev().for_each(f);
    }

    #[test]
    fn chunks_fold_in_order_at_any_thread_count() {
        let fold = |fan_out: FanOut| {
            fold_chunks(fan_out, 1000, 7, Vec::new(), |range| range.map(|i| (i as f64).sqrt()).sum::<f64>(), |all: &mut Vec<f64>, s| all.push(s))
        };
        let expected = fold(&serial);
        assert_eq!(expected.len(), 143);
        assert_eq!(fold(&reversed), expected);
        assert_eq!(fold(&threads(5)), expected);
        assert!(fold_chunks(&serial, 0, 3, 0, |_| 1, |a: &mut i32, b| *a += b) == 0);
    }
}
