//! Support mapping based Cylinder shape.

use crate::math::{Real, Vector};
use crate::shape::SupportMap;

#[cfg(feature = "alloc")]
use either::Either;

/// A 3D cylinder shape with axis aligned along the Y axis.
///
/// A cylinder is a shape with circular cross-sections perpendicular to its axis.
/// In Parry, cylinders are always aligned with the Y axis in their local coordinate
/// system and centered at the origin.
///
/// # Structure
///
/// - **Axis**: Always aligned with Y axis (up/down)
/// - **half_height**: Half the length along the Y axis
/// - **radius**: The radius of the circular cross-section
/// - **Height**: Total height = `2 * half_height`
///
/// # Properties
///
/// - **3D only**: Only available with the `dim3` feature
/// - **Convex**: Yes, cylinders are convex shapes
/// - **Flat caps**: The top and bottom are flat circles (not rounded)
/// - **Sharp edges**: The rim where cap meets side is a sharp edge
///
/// # vs Capsule
///
/// If you need rounded ends instead of flat caps, use [`Capsule`](super::Capsule):
/// - **Cylinder**: Flat circular caps, sharp edges at rims
/// - **Capsule**: Hemispherical caps, completely smooth (no edges)
/// - **Capsule**: Better for characters and rolling objects
/// - **Cylinder**: Better for columns, cans, pipes
///
/// # Use Cases
///
/// - Pillars and columns
/// - Cans and barrels
/// - Wheels and disks
/// - Pipes and tubes
/// - Any object with flat circular ends
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Cylinder;
///
/// // Create a cylinder: radius 2.0, total height 10.0
/// let cylinder = Cylinder::new(5.0, 2.0);
///
/// assert_eq!(cylinder.half_height, 5.0);
/// assert_eq!(cylinder.radius, 2.0);
///
/// // Total height is 2 * half_height
/// let total_height = cylinder.half_height * 2.0;
/// assert_eq!(total_height, 10.0);
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(feature = "encase", derive(encase::ShaderType))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(C)]
pub struct Cylinder {
    /// Half the length of the cylinder along the Y axis.
    ///
    /// The cylinder extends from `-half_height` to `+half_height` along Y.
    /// Total height = `2 * half_height`. Must be positive.
    pub half_height: Real,

    /// The radius of the circular cross-section.
    ///
    /// All points on the cylindrical surface are at this distance from the Y axis.
    /// Must be positive.
    pub radius: Real,
}

impl Cylinder {
    /// Creates a new cylinder aligned with the Y axis.
    ///
    /// # Arguments
    ///
    /// * `half_height` - Half the total height along the Y axis
    /// * `radius` - The radius of the circular cross-section
    ///
    /// # Panics
    ///
    /// Panics if `half_height` or `radius` is not positive.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Cylinder;
    ///
    /// // Create a cylinder with radius 3.0 and height 8.0
    /// let cylinder = Cylinder::new(4.0, 3.0);
    ///
    /// assert_eq!(cylinder.half_height, 4.0);
    /// assert_eq!(cylinder.radius, 3.0);
    ///
    /// // The cylinder:
    /// // - Extends from y = -4.0 to y = 4.0 (total height 8.0)
    /// // - Has circular cross-section with radius 3.0 in the XZ plane
    /// # }
    /// ```
    pub fn new(half_height: Real, radius: Real) -> Cylinder {
        assert!(half_height.is_sign_positive() && radius.is_sign_positive());

        Cylinder {
            half_height,
            radius,
        }
    }

    /// Computes a scaled version of this cylinder.
    ///
    /// Scaling a cylinder can produce different results depending on the scale factors:
    ///
    /// - **Uniform scaling** (all axes equal): Produces another cylinder
    /// - **Y different from X/Z**: Produces another cylinder (if X == Z)
    /// - **Non-uniform X/Z**: Produces an elliptical cylinder approximated as a convex mesh
    ///
    /// # Arguments
    ///
    /// * `scale` - Scaling factors for X, Y, Z axes
    /// * `nsubdivs` - Number of subdivisions for mesh approximation (if needed)
    ///
    /// # Returns
    ///
    /// * `Some(Either::Left(Cylinder))` - If X and Z scales are equal
    /// * `Some(Either::Right(ConvexPolyhedron))` - If X and Z scales differ (elliptical)
    /// * `None` - If mesh approximation failed (e.g., zero scale on an axis)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32", feature = "alloc"))] {
    /// use parry3d::shape::Cylinder;
    /// use parry3d::math::Vector;
    /// use either::Either;
    ///
    /// let cylinder = Cylinder::new(2.0, 1.0);
    ///
    /// // Uniform scaling: produces a larger cylinder
    /// let scale1 = Vector::splat(2.0);
    /// if let Some(Either::Left(scaled)) = cylinder.scaled(scale1, 20) {
    ///     assert_eq!(scaled.radius, 2.0);      // 1.0 * 2.0
    ///     assert_eq!(scaled.half_height, 4.0); // 2.0 * 2.0
    /// }
    ///
    /// // Different Y scale: still a cylinder
    /// let scale2 = Vector::new(1.5, 3.0, 1.5);
    /// if let Some(Either::Left(scaled)) = cylinder.scaled(scale2, 20) {
    ///     assert_eq!(scaled.radius, 1.5);      // 1.0 * 1.5
    ///     assert_eq!(scaled.half_height, 6.0); // 2.0 * 3.0
    /// }
    ///
    /// // Non-uniform X/Z: produces elliptical cylinder (mesh approximation)
    /// let scale3 = Vector::new(2.0, 1.0, 1.0);
    /// if let Some(Either::Right(polyhedron)) = cylinder.scaled(scale3, 20) {
    ///     // Result is a convex mesh approximating an elliptical cylinder
    ///     assert!(polyhedron.points().len() > 0);
    /// }
    /// # }
    /// ```
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn scaled(
        self,
        scale: Vector,
        nsubdivs: u32,
    ) -> Option<Either<Self, super::ConvexPolyhedron>> {
        if scale.x != scale.z {
            // The scaled shape isn't a cylinder.
            let (mut vtx, idx) = self.to_trimesh(nsubdivs);
            vtx.iter_mut().for_each(|pt| *pt *= scale);
            Some(Either::Right(super::ConvexPolyhedron::from_convex_mesh(
                vtx, &idx,
            )?))
        } else {
            Some(Either::Left(Self::new(
                self.half_height * scale.y,
                self.radius * scale.x,
            )))
        }
    }
}

impl SupportMap for Cylinder {
    fn local_support_point(&self, dir: Vector) -> Vector {
        let mut vres = dir;
        vres[1] = 0.0;
        vres = vres.normalize_or_zero() * self.radius;
        vres[1] = self.half_height.copysign(dir[1]);
        vres
    }
}
