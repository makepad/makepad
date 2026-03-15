use crate::math::Vector;
use crate::query::Ray;

/// A structure representing 4 rays in an SIMD SoA fashion.
///
/// Note: When SIMD is not enabled, this is equivalent to a single Ray.
#[derive(Debug, Copy, Clone)]
pub struct SimdRay {
    /// The origin of the rays represented as a single SIMD point.
    pub origin: Vector,
    /// The direction of the rays represented as a single SIMD vector.
    pub dir: Vector,
}

impl SimdRay {
    /// Creates a new SIMD ray with all its lanes filled with the same ray.
    pub fn splat(ray: Ray) -> Self {
        Self {
            origin: ray.origin,
            dir: ray.dir,
        }
    }
}
