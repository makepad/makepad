use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;

/// Hard union: min(a, b).
pub struct SdfUnion<A: Sdf3, B: Sdf3>(pub A, pub B);

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfUnion<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.0.distance(p).min(self.1.distance(p))
    }
}

/// Hard difference: max(a, -b).
pub struct SdfDifference<A: Sdf3, B: Sdf3>(pub A, pub B);

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfDifference<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.0.distance(p).max(-self.1.distance(p))
    }
}

/// Hard intersection: max(a, b).
pub struct SdfIntersection<A: Sdf3, B: Sdf3>(pub A, pub B);

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfIntersection<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.0.distance(p).max(self.1.distance(p))
    }
}

/// Smooth (blobby) union using polynomial smooth-min.
///
/// `k` is the "gloopiness" factor — higher values produce a wider, blobby
/// blend between the two shapes. Typical values: 0.1 (subtle) to 2.0 (very gloopy).
/// When k = 0.0, this degenerates to a hard union.
pub struct SdfSmoothUnion<A: Sdf3, B: Sdf3> {
    pub a: A,
    pub b: B,
    pub k: f64,
}

impl<A: Sdf3, B: Sdf3> SdfSmoothUnion<A, B> {
    pub fn new(a: A, b: B, gloopiness: f64) -> Self {
        Self {
            a,
            b,
            k: gloopiness,
        }
    }
}

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfSmoothUnion<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        let d1 = self.a.distance(p);
        let d2 = self.b.distance(p);
        if self.k <= 0.0 {
            return d1.min(d2);
        }
        let h = (0.5 + 0.5 * (d2 - d1) / self.k).clamp(0.0, 1.0);
        d2 + (d1 - d2) * h - self.k * h * (1.0 - h)
    }
}

/// Smooth (blobby) difference.
///
/// `k` is the gloopiness factor for the blend at the cut boundary.
pub struct SdfSmoothDifference<A: Sdf3, B: Sdf3> {
    pub a: A,
    pub b: B,
    pub k: f64,
}

impl<A: Sdf3, B: Sdf3> SdfSmoothDifference<A, B> {
    pub fn new(a: A, b: B, gloopiness: f64) -> Self {
        Self {
            a,
            b,
            k: gloopiness,
        }
    }
}

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfSmoothDifference<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        let d1 = self.a.distance(p);
        let d2 = -self.b.distance(p);
        if self.k <= 0.0 {
            return d1.max(d2);
        }
        let h = (0.5 - 0.5 * (d2 + d1) / self.k).clamp(0.0, 1.0);
        d1 + (d2 - d1) * h + self.k * h * (1.0 - h)
    }
}

/// Smooth intersection.
pub struct SdfSmoothIntersection<A: Sdf3, B: Sdf3> {
    pub a: A,
    pub b: B,
    pub k: f64,
}

impl<A: Sdf3, B: Sdf3> SdfSmoothIntersection<A, B> {
    pub fn new(a: A, b: B, gloopiness: f64) -> Self {
        Self {
            a,
            b,
            k: gloopiness,
        }
    }
}

impl<A: Sdf3, B: Sdf3> Sdf3 for SdfSmoothIntersection<A, B> {
    fn distance(&self, p: Vec3d) -> f64 {
        let d1 = self.a.distance(p);
        let d2 = self.b.distance(p);
        if self.k <= 0.0 {
            return d1.max(d2);
        }
        let h = (0.5 - 0.5 * (d2 - d1) / self.k).clamp(0.0, 1.0);
        d2 + (d1 - d2) * h + self.k * h * (1.0 - h)
    }
}

/// Chain multiple SDFs into one smooth-unioned blob.
///
/// Usage:
/// ```ignore
/// let blob = SdfBlobChain::new(gloopiness)
///     .add(sphere1)
///     .add(sphere2)
///     .add(capsule1);
/// ```
pub struct SdfBlobChain {
    pub sdfs: Vec<Box<dyn Sdf3 + Send + Sync>>,
    pub k: f64,
}

impl SdfBlobChain {
    pub fn new(gloopiness: f64) -> Self {
        Self {
            sdfs: Vec::new(),
            k: gloopiness,
        }
    }

    pub fn add(mut self, sdf: impl Sdf3 + Send + Sync + 'static) -> Self {
        self.sdfs.push(Box::new(sdf));
        self
    }
}

impl Sdf3 for SdfBlobChain {
    fn distance(&self, p: Vec3d) -> f64 {
        if self.sdfs.is_empty() {
            return f64::MAX;
        }
        let mut d = self.sdfs[0].distance(p);
        for sdf in &self.sdfs[1..] {
            let d2 = sdf.distance(p);
            if self.k <= 0.0 {
                d = d.min(d2);
            } else {
                let h = (0.5 + 0.5 * (d2 - d) / self.k).clamp(0.0, 1.0);
                d = d2 + (d - d2) * h - self.k * h * (1.0 - h);
            }
        }
        d
    }
}
