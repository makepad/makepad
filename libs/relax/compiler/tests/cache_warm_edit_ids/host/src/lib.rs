// The crate the edits leave alone between edits of wf-edit (the HAL of the script VM / HAL
// series): it does not depend on wf-edit, so its slots stay prelinked while wf-edit changes. It
// runs wf-edit's closures through `dyn FnMut` (vtables in data) and generic calls.
use std::sync::{Mutex, MutexGuard};

pub struct Host {
    cbs: Vec<Box<dyn FnMut(u64) -> u64>>,
}

impl Host {
    pub fn new() -> Self {
        Host { cbs: Vec::new() }
    }

    pub fn add(&mut self, f: Box<dyn FnMut(u64) -> u64>) {
        self.cbs.push(f);
    }

    #[inline(never)]
    pub fn run(&mut self, n: u64) -> u64 {
        let mut acc = 0u64;
        for i in 0..n {
            for f in self.cbs.iter_mut() {
                acc = acc.wrapping_add(f(i));
            }
        }
        acc
    }
}

#[inline(never)]
pub fn apply<F: Fn(u64) -> u64>(f: F, v: u64) -> u64 {
    f(v) + 1
}

// A closure of this file named by the cached code of a caller above the edits (the HAL's
// `lock`): the closure's key names this file's text, so an edit below makes the image's key of it
// stop decoding while the caller's crate state is the image's again after the next edit elsewhere.
static STATE: Mutex<u64> = Mutex::new(0);

#[inline(never)]
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[inline(never)]
pub fn bump(n: u64) -> u64 {
    let mut g = lock(&STATE);
    *g += n;
    *g
}

// A closure called directly and also taken as a fn pointer (a thunk symbol: the startup image
// makes a slot for every thunk key, used or not). After an edit below it, the cached code of
// `tick` -- unchanged, so its entry is taken again -- names the closure by its old file version;
// that key must never resolve to a slot the image made but cannot name.
#[inline(never)]
pub fn tick(v: u64) -> u64 {
    let c = |x: u64| x * 3 + 1;
    let p: fn(u64) -> u64 = c;
    std::hint::black_box(p)(v) + c(v)
}

#[inline(never)]
pub fn host_open(x: u64) -> u64 {
    // HOST-EDIT
    x * 7 + 1
}
