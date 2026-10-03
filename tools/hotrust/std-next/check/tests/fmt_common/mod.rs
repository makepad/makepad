//! Shared helpers for the fmt differential tests: format through HotRust's fmt machinery
//! (hand-built Arguments) and compare with real format!.
#![allow(dead_code)]

use hotrust_std_check::fmt as hf;
use hotrust_std_check::fmt::rt;

pub struct Sink(pub String);

impl hf::Write for Sink {
    fn write_str(&mut self, s: &str) -> hf::Result {
        self.0.push_str(s);
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct Spec {
    pub fill: char,
    pub align: u8,
    pub flags: u32,
    pub width: Option<usize>,
    pub prec: Option<usize>,
}

pub const DEF: Spec = Spec { fill: ' ', align: rt::ALIGN_UNKNOWN, flags: 0, width: None, prec: None };

fn count(c: Option<usize>) -> rt::Count {
    match c {
        Some(n) => rt::Count::Is(n),
        None => rt::Count::Implied,
    }
}

pub fn render<T>(v: &T, f: fn(&T, &mut hf::Formatter<'_>) -> hf::Result, s: &Spec) -> String {
    let args = [rt::Argument::new(v, f)];
    let ph = [rt::Placeholder::new(0, s.fill, s.align, s.flags, count(s.prec), count(s.width))];
    let pieces: [&'static str; 1] = [""];
    let mut sink = Sink(String::new());
    hf::write(&mut sink, hf::Arguments::new_v1_formatted(&pieces, &args, &ph)).unwrap();
    sink.0
}

/// Same through a width/precision *parameter* (`{:w$.p$}` with Count::Param).
pub fn render_param<T>(v: &T, f: fn(&T, &mut hf::Formatter<'_>) -> hf::Result, w: usize, p: usize) -> String {
    let args = [rt::Argument::new(v, f), rt::Argument::from_usize(&w), rt::Argument::from_usize(&p)];
    let ph = [rt::Placeholder::new(0, ' ', rt::ALIGN_UNKNOWN, 0, rt::Count::Param(2), rt::Count::Param(1))];
    let pieces: [&'static str; 2] = ["<", ">"];
    let mut sink = Sink(String::new());
    hf::write(&mut sink, hf::Arguments::new_v1_formatted(&pieces, &args, &ph)).unwrap();
    sink.0
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub fn spec(fill: char, align: u8, flags: u32, width: Option<usize>, prec: Option<usize>) -> Spec {
    Spec { fill, align, flags, width, prec }
}
