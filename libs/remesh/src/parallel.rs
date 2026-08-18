//! Parallel helpers on the shared csg_math thread pool (no rayon).

use makepad_csg_math::thread_pool;

/// Run `f(start, end)` over chunked ranges of `0..n`, results in range order.
pub fn par_ranges<R, F>(n: usize, chunk: usize, f: F) -> Vec<R>
where
    R: Send + 'static,
    F: Fn(usize, usize) -> R + Send + Sync + Clone + 'static,
{
    let mut tasks: Vec<Box<dyn FnOnce() -> R + Send>> = Vec::new();
    let mut s = 0;
    while s < n {
        let e = (s + chunk.max(1)).min(n);
        let fc = f.clone();
        tasks.push(Box::new(move || fc(s, e)));
        s = e;
    }
    thread_pool::parallel_for(tasks)
}

/// Parallel sort (+ optional dedup) by a total i128 key, bucketing on the key
/// range. Result identical to `sort_unstable_by_key(key)` (+ `dedup_by_key`).
pub fn par_sort_by_key<T, K>(mut v: Vec<T>, key: K, dedup: bool) -> Vec<T>
where
    T: Send + Sync + Copy + 'static,
    K: Fn(&T) -> i128 + Send + Sync + Clone + 'static,
{
    let n = v.len();
    let threads = thread_pool::thread_count().max(1);
    if n < 262_144 || threads < 2 {
        v.sort_unstable_by_key(|x| key(x));
        if dedup {
            v.dedup_by_key(|x| key(x));
        }
        return v;
    }
    let mut mn = i128::MAX;
    let mut mx = i128::MIN;
    for x in &v {
        let k = key(x);
        mn = mn.min(k);
        mx = mx.max(k);
    }
    let nb = (threads * 4).min(256) as i128;
    let width = ((mx - mn) / nb + 1).max(1);
    let v = std::sync::Arc::new(v);
    // per-chunk partition into buckets
    let chunk = n.div_ceil(threads);
    let parts: Vec<Vec<Vec<T>>> = {
        let v = v.clone();
        let key = key.clone();
        par_ranges(n, chunk, move |s, e| {
            let mut buckets: Vec<Vec<T>> = vec![Vec::new(); nb as usize];
            for i in s..e {
                let b = ((key(&v[i]) - mn) / width) as usize;
                buckets[b].push(v[i]);
            }
            buckets
        })
    };
    let parts = std::sync::Arc::new(parts);
    // per-bucket: concatenate chunk pieces (chunk order), sort, dedup
    let sorted: Vec<Vec<T>> = {
        let parts = parts.clone();
        par_ranges(nb as usize, 1, move |s, _e| {
            let mut bucket: Vec<T> = Vec::new();
            for p in parts.iter() {
                bucket.extend_from_slice(&p[s]);
            }
            bucket.sort_unstable_by_key(|x| key(x));
            if dedup {
                bucket.dedup_by_key(|x| key(x));
            }
            bucket
        })
    };
    let total: usize = sorted.iter().map(|b| b.len()).sum();
    let mut out = Vec::with_capacity(total);
    for b in sorted {
        out.extend_from_slice(&b);
    }
    out
}
