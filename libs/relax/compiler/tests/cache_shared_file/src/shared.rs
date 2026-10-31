// The library and binary both compile this file. In a live run the binary's
// enum has an explicit tag, while the library's enum can use the Box niche.
// A cached closure must retain the file's owning crate when it calls finish.
enum Work {
    Value(Box<u64>),
    Empty,
}

impl Work {
    #[inline(never)]
    fn finish(self) -> u64 {
        match self {
            Self::Value(value) => *value,
            Self::Empty => 0,
        }
    }
}

#[inline(never)]
pub fn run() -> (u64, usize) {
    let mut sum = 0;
    let mut size = 0;
    for work in [Work::Value(Box::new(42)), Work::Empty] {
        // the closure reads the layout of the crate that compiled it
        sum += std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            size = std::mem::size_of::<Work>();
            work.finish()
        }))
        .unwrap();
    }
    // a boxed job run on another thread, as the sandbox's retirement worker does
    let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() -> (u64, usize) + Send>>();
    let worker = std::thread::spawn(move || {
        let mut got = (0, 0);
        while let Ok(job) = rx.recv() {
            got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap();
        }
        got
    });
    let work = Work::Value(Box::new(0));
    tx.send(Box::new(move || (work.finish(), std::mem::size_of::<Work>()))).unwrap();
    drop(tx);
    let (extra, size2) = worker.join().unwrap();
    assert_eq!(size, size2);
    (sum + extra, size)
}
