// Shared channel-based thread pool for CSG parallelism.
//
// Workers are spawned once and reused across all CSG operations.
// Disabled entirely when the `threads` feature is off.

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
            let cpus = thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1);
            // Leave 1-2 cores free for the OS and other work
            let n = if cpus <= 4 {
                (cpus - 1).max(1)
            } else {
                cpus - 2
            };
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

    /// Run 8 closures in parallel on pool threads, returning all results.
    /// Falls back to sequential execution if fewer than 2 threads available.
    pub fn parallel_do8<R, F>(tasks: [F; 8]) -> [R; 8]
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        let pool = get_pool();
        if pool.size < 2 {
            return tasks.map(|f| f());
        }

        // Distribute 8 tasks across available pool threads
        let num_workers = pool.size.min(8);
        let mut receivers: Vec<mpsc::Receiver<(usize, R)>> = Vec::with_capacity(8);
        let mut txs: Vec<mpsc::Sender<(usize, R)>> = Vec::with_capacity(8);

        for _ in 0..8 {
            let (tx, rx) = mpsc::channel();
            txs.push(tx);
            receivers.push(rx);
        }

        for (i, task) in tasks.into_iter().enumerate() {
            let tx = txs[i].clone();
            let worker = i % num_workers;
            let _ = pool.senders[worker].send(Box::new(move || {
                let result = task();
                let _ = tx.send((i, result));
            }));
        }
        drop(txs);

        // Collect in order
        let mut results: [Option<R>; 8] = [(); 8].map(|_| None);
        for rx in receivers {
            if let Ok((_i, r)) = rx.recv() {
                results[_i] = Some(r);
            }
        }
        results.map(|r| r.expect("parallel_do8: a task failed"))
    }

    /// Run N independent tasks in parallel, returning results in order.
    /// Tasks are distributed round-robin across pool workers.
    /// No nesting — each task should run sequentially.
    pub fn parallel_for<R, F>(tasks: Vec<F>) -> Vec<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        let pool = get_pool();
        if pool.size < 2 || tasks.is_empty() {
            return tasks.into_iter().map(|f| f()).collect();
        }

        let n = tasks.len();
        let num_workers = pool.size.min(n);
        let mut receivers = Vec::with_capacity(n);

        for (i, task) in tasks.into_iter().enumerate() {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            let worker = i % num_workers;
            let _ = pool.senders[worker].send(Box::new(move || {
                let _ = tx.send(task());
            }));
        }

        receivers
            .into_iter()
            .map(|rx| rx.recv().expect("parallel_for: a task failed"))
            .collect()
    }

    /// Map a function over chunks of a slice in parallel, collecting results in order.
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

    pub fn parallel_do8<R, F>(tasks: [F; 8]) -> [R; 8]
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        tasks.map(|f| f())
    }

    pub fn parallel_for<R, F>(tasks: Vec<F>) -> Vec<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        tasks.into_iter().map(|f| f()).collect()
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

pub use inner::{parallel_do2, parallel_do8, parallel_for, parallel_map, thread_count};
