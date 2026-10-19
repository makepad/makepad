// Fields of a `&mut` parameter that a loop loads and stores each iteration (a cursor index, a bit
// buffer) are kept in registers for the loop and written back on every way out: the normal exit,
// each early exit, the panic that unwinds out of the loop. Output matches rustc.
use std::hint::black_box;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Cursor<'a> {
    chars: &'a [u32],
    index: usize,
    seen: u64,
    wide: u8,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> u32 {
        self.chars.get(self.index).copied().unwrap_or(0)
    }
    fn skip_while(&mut self, limit: u32) -> usize {
        let start = self.index;
        while self.peek() != 0 && self.peek() < limit {
            self.seen = self.seen.wrapping_mul(31).wrapping_add(self.peek() as u64);
            self.index += 1;
        }
        self.index - start
    }
    // an early exit and a narrow field
    fn skip_until(&mut self, stop: u32) -> bool {
        loop {
            let c = self.peek();
            if c == 0 {
                return false;
            }
            self.wide = self.wide.wrapping_add(c as u8);
            if c == stop {
                return true;
            }
            self.index += 1;
        }
    }
    // a loop that panics in the middle: the index must be current when it unwinds
    fn count_to_nine(&mut self) {
        loop {
            let c = self.chars[self.index];
            assert!(c != 9, "nine at {}", self.index);
            self.index += 1;
        }
    }
    // an access through a pointer based on the parameter: no promotion, still right
    fn through_pointer(&mut self, n: usize) {
        let p: *mut usize = &mut self.index;
        for _ in 0..n {
            unsafe {
                *p += 2;
            }
            self.index += 1;
        }
    }
}

struct Pair {
    a: f32,
    n: u32,
}

fn accumulate(p: &mut Pair, xs: &[f32]) {
    for x in xs {
        p.a += *x * 0.5;
        p.n += 1;
    }
}

fn run() {
    let text: Vec<u32> = (0..64).map(|i| if i == 63 { 0 } else { 1 + (i * 7) % 11 }).collect();
    let mut c = Cursor { chars: &text, index: 0, seen: 0, wide: 0 };
    let a = c.skip_while(9);
    println!("{} {} {}", a, c.index, c.seen);
    let b = c.skip_until(10);
    println!("{} {} {}", b, c.index, c.wide);
    let b = c.skip_until(77);
    println!("{} {} {}", b, c.index, c.wide);
    let mut c = Cursor { chars: &text, index: 0, seen: 0, wide: 0 };
    let r = catch_unwind(AssertUnwindSafe(|| c.count_to_nine()));
    println!("{} {}", r.is_err(), c.index);
    let mut c = Cursor { chars: &text, index: 3, seen: 0, wide: 0 };
    c.through_pointer(black_box(5));
    println!("{}", c.index);
    let mut p = Pair { a: 1.0, n: 0 };
    accumulate(&mut p, &[1.0, 2.0, 3.5, black_box(4.0)]);
    println!("{} {}", p.a, p.n);
    // enough calls to be compiled by the optimiser
    let mut total = 0u64;
    for round in 0..3000 {
        let mut c = Cursor { chars: &text, index: black_box(round % 5), seen: 0, wide: 0 };
        total += c.skip_while(black_box(10)) as u64;
        c.skip_until(black_box(4));
        total += c.index as u64 + c.wide as u64;
        let mut p = Pair { a: 0.0, n: 0 };
        accumulate(&mut p, &[1.0, 2.0]);
        total += p.n as u64;
    }
    println!("{}", total);
}

#[test]
fn loop_fields_stay_correct() {
    run();
}

fn main() {
    run();
}
