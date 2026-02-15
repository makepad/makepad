use makepad_csg_math::Vec3d;

/// Signed distance field trait.
///
/// `distance(p)` returns the signed distance from point `p` to the surface:
///   - negative inside the shape
///   - positive outside the shape
///   - zero on the surface
pub trait Sdf3 {
    fn distance(&self, p: Vec3d) -> f64;

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
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}

impl Sdf3 for Box<dyn Sdf3> {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}

impl Sdf3 for Box<dyn Sdf3 + Send + Sync> {
    fn distance(&self, p: Vec3d) -> f64 {
        (**self).distance(p)
    }
    fn normal(&self, p: Vec3d) -> Vec3d {
        (**self).normal(p)
    }
}
