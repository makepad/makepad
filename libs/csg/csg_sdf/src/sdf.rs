use makepad_csg_math::Vec3d;

/// Signed distance field trait.
///
/// `distance(p)` returns the signed distance from point `p` to the surface:
///   - negative inside the shape
///   - positive outside the shape
///   - zero on the surface
pub trait Sdf3 {
    fn distance(&self, p: Vec3d) -> f64;

    /// `distance(p)` clamped to `lower..=upper`. A consumer that only needs
    /// this interval lets a compound field stop once an operand decides it.
    /// `hints` is the query stream's own state between nearby queries of this
    /// same immutable field: a field may take a sign from a ball it left there,
    /// and leave one.
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, _hints: &mut Hints) -> f64 {
        self.distance(p).clamp(lower, upper)
    }

    /// [`Sdf3::distance_bounded`] at many points, one stream of queries in order.
    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        for (p, o) in ps.iter().zip(out.iter_mut()) {
            *o = self.distance_bounded(*p, lower, upper, hints);
        }
    }

    /// The distances at many points (a field that runs as a kernel answers them in one call).
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        for (p, o) in ps.iter().zip(out.iter_mut()) {
            *o = self.distance(*p);
        }
    }

    /// Surface normal via central finite differences.
    fn normal(&self, p: Vec3d) -> Vec3d {
        const H: f64 = 1e-4;
        let dx = self.distance(Vec3d::new(p.x + H, p.y, p.z))
            - self.distance(Vec3d::new(p.x - H, p.y, p.z));
        let dy = self.distance(Vec3d::new(p.x, p.y + H, p.z))
            - self.distance(Vec3d::new(p.x, p.y - H, p.z));
        let dz = self.distance(Vec3d::new(p.x, p.y, p.z + H))
            - self.distance(Vec3d::new(p.x, p.y, p.z - H));
        Vec3d::new(dx, dy, dz).normalize()
    }
}

/// How many balls a query stream keeps per field: a walk often comes back near where it was a
/// few queries before (a split block's next child, the next angle's cell).
pub const HINT_BALLS: usize = 8;

/// What one stream of nearby queries of a field (one walk over nearby points, on one thread)
/// carries from each query to the next: the last few balls free of the field's surface whose
/// signs it took, where its last answer came from, and a compound field's per part. Each stream
/// owns its own. It only ever shortcuts a query to the answer the query would give without it.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    balls: [Option<Ball>; HINT_BALLS],
    next: usize,
    /// The field's own note of where its last distance came from, where the next search starts
    /// (a mesh: its nearest triangle plus one; zero: none).
    pub near: u32,
    /// A compound field's parts' hints, by part.
    pub parts: Vec<Hints>,
}

impl Hints {
    /// Part `i`'s hints.
    pub fn part(&mut self, i: usize) -> &mut Hints {
        if self.parts.len() <= i {
            self.parts.resize_with(i + 1, Hints::default);
        }
        &mut self.parts[i]
    }

    /// The balls kept, the newest first.
    pub fn balls(&self) -> impl Iterator<Item = &Ball> {
        (1..=HINT_BALLS).filter_map(move |k| self.balls[(self.next + HINT_BALLS - k) % HINT_BALLS].as_ref())
    }

    /// Keep `ball`, in place of the oldest once there are [`HINT_BALLS`].
    pub fn leave(&mut self, ball: Ball) {
        self.balls[self.next] = Some(ball);
        self.next = (self.next + 1) % HINT_BALLS;
    }
}

/// A ball free of a field's surface, so that the whole ball has the sign at its centre: its
/// radius is a proven lower bound on the distance from the centre to the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub center: Vec3d,
    pub radius: f64,
    pub inside: bool,
}

/// Blanket impl so `&dyn Sdf3` and `Box<dyn Sdf3>` work.
impl<T: Sdf3 + ?Sized> Sdf3 for &T {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        (**self).distance_bounded(p, lower, upper, hints)
    }
    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        (**self).distance_many_bounded(ps, lower, upper, out, hints)
    }
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        (**self).distance_many(ps, out)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}

impl Sdf3 for Box<dyn Sdf3> {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        (**self).distance_bounded(p, lower, upper, hints)
    }
    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        (**self).distance_many_bounded(ps, lower, upper, out, hints)
    }
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        (**self).distance_many(ps, out)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}

impl Sdf3 for Box<dyn Sdf3 + Send + Sync> {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        (**self).distance_bounded(p, lower, upper, hints)
    }
    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        (**self).distance_many_bounded(ps, lower, upper, out, hints)
    }
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        (**self).distance_many(ps, out)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}
