//! Delay lines, allocated once at construction and never resized.

/// A circular delay line with fractional (cubic Hermite) reads. Capacity is
/// rounded up to a power of two so wrapping is a mask.
#[derive(Clone)]
pub struct DelayLine {
    buf: Box<[f32]>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// A line that can delay by at least `max_samples`.
    pub fn new(max_samples: usize) -> Self {
        let len = (max_samples + 4).next_power_of_two();
        DelayLine { buf: vec![0.0; len].into_boxed_slice(), mask: len - 1, write: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len() - 4
    }

    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// The sample written `delay` samples ago (integer).
    #[inline]
    pub fn tap(&self, delay: usize) -> f32 {
        let d = delay.min(self.capacity()).max(1);
        self.buf[(self.write + self.buf.len() - d) & self.mask]
    }

    /// Fractional read, `delay` in samples (≥ 2 — the Hermite kernel needs
    /// one written sample on the near side), cubic Hermite.
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(2.0, (self.capacity() - 2) as f32);
        let di = delay.floor();
        let f = delay - di;
        let d = di as usize;
        let len = self.buf.len();
        let at = |k: usize| self.buf[(self.write + len * 2 - k) & self.mask];
        let xm1 = at(d - 1);
        let x0 = at(d);
        let x1 = at(d + 1);
        let x2 = at(d + 2);
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * f + c2) * f + c1) * f + x0
    }

    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
    }
}

/// Schroeder all-pass diffuser on a fixed-length line.
#[derive(Clone)]
pub struct Allpass {
    line: DelayLine,
    len: usize,
    pub g: f32,
}

impl Allpass {
    pub fn new(len: usize, g: f32) -> Self {
        Allpass { line: DelayLine::new(len), len: len.max(1), g }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let d = self.line.tap(self.len);
        let v = x + self.g * d;
        self.line.push(v);
        d - self.g * v
    }

    pub fn clear(&mut self) {
        self.line.clear();
    }
}

/// A feedback comb with a damping low-pass in the loop (a lossy tube): the
/// exhaust and intake resonators are built from these, tuned by length.
#[derive(Clone)]
pub struct Comb {
    line: DelayLine,
    pub delay: f32,
    pub feedback: f32,
    /// 0..1 loop low-pass (0 = bright, 0.9 = dark).
    pub damp: f32,
    z: f32,
}

impl Comb {
    pub fn new(max_samples: usize) -> Self {
        Comb { line: DelayLine::new(max_samples), delay: 100.0, feedback: 0.5, damp: 0.3, z: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.line.read(self.delay);
        self.z = y + (self.z - y) * self.damp;
        self.line.push(x + self.z * self.feedback);
        y
    }

    pub fn clear(&mut self) {
        self.line.clear();
        self.z = 0.0;
    }
}
