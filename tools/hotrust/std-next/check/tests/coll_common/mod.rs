//! Shared helpers for the collections differential tests.
#![allow(dead_code)]

use hotrust_std_check::fmt as hf;
use hotrust_std_check::fmt::rt;
use std::cell::Cell;
use std::rc::Rc as StdRc;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Sink(String);

impl hf::Write for Sink {
    fn write_str(&mut self, s: &str) -> hf::Result {
        self.0.push_str(s);
        Ok(())
    }
}

/// `{:?}` (alternate = `{:#?}`) through HotRust's fmt.
pub fn hdebug<T: hf::Debug>(v: &T, alternate: bool) -> String {
    let args = [rt::Argument::new(v, <T as hf::Debug>::fmt)];
    let flags = if alternate { rt::FLAG_ALTERNATE } else { 0 };
    let ph = [rt::Placeholder::new(0, ' ', rt::ALIGN_UNKNOWN, flags, rt::Count::Implied, rt::Count::Implied)];
    let pieces: [&'static str; 1] = [""];
    let mut sink = Sink(String::new());
    hf::write(&mut sink, hf::Arguments::new_v1_formatted(&pieces, &args, &ph)).unwrap();
    sink.0
}

/// A value that counts its drops in a shared counter.
pub struct Counted {
    pub v: u32,
    pub drops: StdRc<Cell<usize>>,
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

/// Runs `f`, returns its panic message (None if it did not panic).
pub fn panic_msg<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> Option<String> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(f);
    std::panic::set_hook(prev);
    match r {
        Ok(()) => None,
        Err(e) => {
            if let Some(s) = e.downcast_ref::<String>() {
                Some(s.clone())
            } else if let Some(s) = e.downcast_ref::<&str>() {
                Some(s.to_string())
            } else {
                Some("<non-string panic>".to_string())
            }
        }
    }
}
