//! Small f32 vector helpers and boxes. f32 everywhere: the reference has to
//! give the GPU bake's numbers, so it uses the same width and plain
//! (unfused) arithmetic.

pub type V3 = [f32; 3];

#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
#[inline]
pub fn mix3(a: V3, b: V3, t: f32) -> V3 {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}
#[inline]
pub fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// An axis-aligned box. `Aabb::EMPTY` has lo > hi and overlaps nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub lo: V3,
    pub hi: V3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { lo: [f32::MAX; 3], hi: [f32::MIN; 3] };

    pub fn new(lo: V3, hi: V3) -> Aabb {
        Aabb { lo, hi }
    }
    pub fn point(p: V3) -> Aabb {
        Aabb { lo: p, hi: p }
    }
    pub fn sphere(c: V3, r: f32) -> Aabb {
        Aabb::point(c).expand(r)
    }
    pub fn is_empty(&self) -> bool {
        self.lo[0] > self.hi[0] || self.lo[1] > self.hi[1] || self.lo[2] > self.hi[2]
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            lo: [self.lo[0].min(o.lo[0]), self.lo[1].min(o.lo[1]), self.lo[2].min(o.lo[2])],
            hi: [self.hi[0].max(o.hi[0]), self.hi[1].max(o.hi[1]), self.hi[2].max(o.hi[2])],
        }
    }
    pub fn expand(&self, r: f32) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        Aabb { lo: sub(self.lo, [r; 3]), hi: add(self.hi, [r; 3]) }
    }
    pub fn translate(&self, t: V3) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        Aabb { lo: add(self.lo, t), hi: add(self.hi, t) }
    }
    pub fn contains(&self, p: V3) -> bool {
        (0..3).all(|a| p[a] >= self.lo[a] && p[a] <= self.hi[a])
    }
    pub fn overlaps(&self, o: &Aabb) -> bool {
        (0..3).all(|a| self.lo[a] <= o.hi[a] && o.lo[a] <= self.hi[a])
    }
    pub fn corners(&self) -> [V3; 8] {
        let (l, h) = (self.lo, self.hi);
        [
            [l[0], l[1], l[2]],
            [h[0], l[1], l[2]],
            [l[0], h[1], l[2]],
            [h[0], h[1], l[2]],
            [l[0], l[1], h[2]],
            [h[0], l[1], h[2]],
            [l[0], h[1], h[2]],
            [h[0], h[1], h[2]],
        ]
    }
    /// The distance from `p` to the box (0 inside).
    pub fn distance(&self, p: V3) -> f32 {
        let mut s = 0.0;
        for a in 0..3 {
            let d = (self.lo[a] - p[a]).max(p[a] - self.hi[a]).max(0.0);
            s += d * d;
        }
        s.sqrt()
    }
}

/// The nearest lattice index: floor(x + 0.5), exact on CPU and in shaders
/// (shader `round` is implementation-defined at .5).
#[inline]
pub fn nearest(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}
