// Simple channel-based thread pool for CSG parallelism.
//
// Workers are spawned once and reused across boolean operations.
// Disabled entirely when the `threads` feature is off.
//
// Usage:
//   let results = pool().map_slices(&data, chunk_size, |slice| { ... });

#[cfg(feature = "threads")]
mod inner {
    use std::sync::mpsc;
    use std::sync::{Arc, OnceLock};
    use std::thread;

    type Job = Box<dyn FnOnce() + Send>;

    struct Pool {
        senders: Vec<mpsc::Sender<Job>>,
        size: usize,
    }

    impl Pool {
        fn new(size: usize) -> Pool {
            let mut senders = Vec::with_capacity(size);
            for _ in 0..size {
                let (tx, rx) = mpsc::channel::<Job>();
                thread::spawn(move || {
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                });
                senders.push(tx);
            }
            Pool { senders, size }
        }
    }

    static POOL: OnceLock<Pool> = OnceLock::new();

    fn get_pool() -> &'static Pool {
        POOL.get_or_init(|| {
            let n = thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1)
                .max(1);
            Pool::new(n)
        })
    }

    /// Number of worker threads in the pool.
    pub fn thread_count() -> usize {
        get_pool().size
    }

    /// Run two closures in parallel on pool threads, returning both results.
    pub fn parallel_do2<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
    where
        A: Send + 'static,
        B: Send + 'static,
        FA: FnOnce() -> A + Send + 'static,
        FB: FnOnce() -> B + Send + 'static,
    {
        let pool = get_pool();
        if pool.size < 2 {
            return (fa(), fb());
        }

        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();

        let _ = pool.senders[0].send(Box::new(move || {
            let _ = tx_a.send(fa());
        }));
        let _ = pool.senders[1].send(Box::new(move || {
            let _ = tx_b.send(fb());
        }));

        let a = rx_a.recv().expect("parallel_do2: task A failed");
        let b = rx_b.recv().expect("parallel_do2: task B failed");
        (a, b)
    }

    /// Map a function over chunks of a slice in parallel, collecting results in order.
    ///
    /// `data` is split into `thread_count()` roughly-equal chunks.
    /// `f` is called on each chunk and must return a Vec of results.
    /// Results are concatenated in input order.
    pub fn parallel_map<T, R, F>(data: &[T], f: F) -> Vec<R>
    where
        T: Send + Sync + Copy + 'static,
        R: Send + 'static,
        F: Fn(&[T]) -> Vec<R> + Send + Clone + 'static,
    {
        let pool = get_pool();
        let n = data.len();
        if n == 0 {
            return Vec::new();
        }

        let num_chunks = pool.size.min(n);
        let chunk_size = (n + num_chunks - 1) / num_chunks;

        if num_chunks <= 1 {
            return f(data);
        }

        // Share data across threads via Arc.
        let shared_data: Arc<[T]> = data.to_vec().into();

        let mut receivers = Vec::with_capacity(num_chunks);

        for (i, sender) in pool.senders.iter().take(num_chunks).enumerate() {
            let start = i * chunk_size;
            let end = (start + chunk_size).min(n);
            if start >= end {
                break;
            }

            let (tx, rx) = mpsc::channel::<Vec<R>>();
            receivers.push(rx);

            let data_arc = Arc::clone(&shared_data);
            let f_clone = f.clone();
            let job = Box::new(move || {
                let chunk = &data_arc[start..end];
                let result = f_clone(chunk);
                let _ = tx.send(result);
            });
            let _ = sender.send(job);
        }

        let mut results = Vec::with_capacity(n);
        for rx in receivers {
            if let Ok(chunk_results) = rx.recv() {
                results.extend(chunk_results);
            }
        }
        results
    }
}

#[cfg(not(feature = "threads"))]
mod inner {
    pub fn thread_count() -> usize {
        1
    }

    pub fn parallel_do2<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
    where
        A: Send + 'static,
        B: Send + 'static,
        FA: FnOnce() -> A + Send + 'static,
        FB: FnOnce() -> B + Send + 'static,
    {
        (fa(), fb())
    }

    pub fn parallel_map<T, R, F>(data: &[T], f: F) -> Vec<R>
    where
        T: Send + Sync + Copy + 'static,
        R: Send + 'static,
        F: Fn(&[T]) -> Vec<R> + Send + Clone + 'static,
    {
        f(data)
    }
}

pub use inner::{parallel_do2, parallel_map, thread_count};
