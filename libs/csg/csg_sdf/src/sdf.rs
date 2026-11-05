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
    /// `hint`, when present, is an exact signed sample of this same immutable
    /// field; implementations may use its surface-free ball to reuse the sign.
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, _hint: Option<(Vec3d, f64)>) -> f64 {
        self.distance(p).clamp(lower, upper)
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

/// Blanket impl so `&dyn Sdf3` and `Box<dyn Sdf3>` work.
impl<T: Sdf3 + ?Sized> Sdf3 for &T {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hint: Option<(Vec3d, f64)>) -> f64 {
        (**self).distance_bounded(p, lower, upper, hint)
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
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hint: Option<(Vec3d, f64)>) -> f64 {
        (**self).distance_bounded(p, lower, upper, hint)
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
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hint: Option<(Vec3d, f64)>) -> f64 {
        (**self).distance_bounded(p, lower, upper, hint)
    }
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        (**self).distance_many(ps, out)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}
