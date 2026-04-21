use crate::math::{Pose, Real, Rotation, Vector};
use crate::shape::{Segment, SupportMap};

#[cfg(feature = "alloc")]
use either::Either;

#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(feature = "encase", derive(encase::ShaderType))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
/// A capsule shape, also known as a pill or capped cylinder.
///
/// A capsule is defined by a line segment (its central axis) and a radius. It can be
/// visualized as a cylinder with hemispherical (2D: semicircular) caps on both ends.
/// This makes it perfect for representing elongated round objects.
///
/// # Structure
///
/// - **Segment**: The central axis from point `a` to point `b`
/// - **Radius**: The thickness around the segment
/// - **In 2D**: Looks like a rounded rectangle or "stadium" shape
/// - **In 3D**: Looks like a cylinder with spherical caps (a pill)
///
/// # Properties
///
/// - **Convex**: Yes, capsules are always convex
/// - **Smooth**: Completely smooth surface (no edges or corners)
/// - **Support mapping**: Efficient (constant time)
/// - **Rolling**: Excellent for objects that need to roll smoothly
///
/// # Use Cases
///
/// Capsules are ideal for:
/// - Characters and humanoid figures (torso, limbs)
/// - Pills, medicine capsules
/// - Elongated projectiles (missiles, torpedoes)
/// - Smooth rolling objects
/// - Any object that's "cylinder-like" but needs smooth collision at ends
///
/// # Advantages Over Cylinders
///
/// - **No sharp edges**: Smoother collision response
/// - **Better for characters**: More natural movement and rotation
/// - **Simpler collision detection**: Easier to compute contacts than cylinders
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Capsule;
/// use parry3d::math::Vector;
///
/// // Create a vertical capsule (aligned with Y axis)
/// // Half-height of 2.0 means the segment is 4.0 units long
/// let capsule = Capsule::new_y(2.0, 0.5);
/// assert_eq!(capsule.radius, 0.5);
/// assert_eq!(capsule.height(), 4.0);
///
/// // Create a custom capsule between two points
/// let a = Vector::ZERO;
/// let b = Vector::new(3.0, 4.0, 0.0);
/// let custom = Capsule::new(a, b, 1.0);
/// assert_eq!(custom.height(), 5.0); // Distance from a to b
/// # }
/// ```
pub struct Capsule {
    /// The line segment forming the capsule's central axis.
    ///
    /// The capsule extends from `segment.a` to `segment.b`, with hemispherical
    /// caps centered at each endpoint.
    pub segment: Segment,

    /// The radius of the capsule.
    ///
    /// This is the distance from the central axis to the surface. Must be positive.
    /// The total "thickness" of the capsule is `2 * radius`.
    pub radius: Real,
}

impl Capsule {
    /// Creates a new capsule aligned with the X axis.
    ///
    /// The capsule is centered at the origin and extends along the X axis.
    ///
    /// # Arguments
    ///
    /// * `half_height` - Half the length of the central segment (total length = `2 * half_height`)
    /// * `radius` - The radius of the capsule
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    ///
    /// // Create a capsule extending 6 units along X axis (3 units in each direction)
    /// // with radius 0.5
    /// let capsule = Capsule::new_x(3.0, 0.5);
    /// assert_eq!(capsule.height(), 6.0);
    /// assert_eq!(capsule.radius, 0.5);
    ///
    /// // The center is at the origin
    /// let center = capsule.center();
    /// assert!(center.length() < 1e-6);
    /// # }
    /// ```
    pub fn new_x(half_height: Real, radius: Real) -> Self {
        let b = Vector::X * half_height;
        Self::new(-b, b, radius)
    }

    /// Creates a new capsule aligned with the Y axis.
    ///
    /// The capsule is centered at the origin and extends along the Y axis.
    /// This is the most common orientation for character capsules (standing upright).
    ///
    /// # Arguments
    ///
    /// * `half_height` - Half the length of the central segment
    /// * `radius` - The radius of the capsule
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    ///
    /// // Create a typical character capsule: 2 units tall with 0.3 radius
    /// let character = Capsule::new_y(1.0, 0.3);
    /// assert_eq!(character.height(), 2.0);
    /// assert_eq!(character.radius, 0.3);
    ///
    /// // Total height including the spherical caps: 2.0 + 2 * 0.3 = 2.6
    /// # }
    /// ```
    pub fn new_y(half_height: Real, radius: Real) -> Self {
        let b = Vector::Y * half_height;
        Self::new(-b, b, radius)
    }

    /// Creates a new capsule aligned with the Z axis.
    ///
    /// The capsule is centered at the origin and extends along the Z axis.
    ///
    /// # Arguments
    ///
    /// * `half_height` - Half the length of the central segment
    /// * `radius` - The radius of the capsule
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    ///
    /// // Create a capsule for a torpedo extending along Z axis
    /// let torpedo = Capsule::new_z(5.0, 0.4);
    /// assert_eq!(torpedo.height(), 10.0);
    /// assert_eq!(torpedo.radius, 0.4);
    /// # }
    /// ```
    #[cfg(feature = "dim3")]
    pub fn new_z(half_height: Real, radius: Real) -> Self {
        let b = Vector::Z * half_height;
        Self::new(-b, b, radius)
    }

    /// Creates a new capsule with custom endpoints and radius.
    ///
    /// This is the most flexible constructor, allowing you to create a capsule
    /// with any orientation and position.
    ///
    /// # Arguments
    ///
    /// * `a` - The first endpoint of the central segment
    /// * `b` - The second endpoint of the central segment
    /// * `radius` - The radius of the capsule
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    /// use parry3d::math::Vector;
    ///
    /// // Create a diagonal capsule
    /// let a = Vector::ZERO;
    /// let b = Vector::new(3.0, 4.0, 0.0);
    /// let capsule = Capsule::new(a, b, 0.5);
    ///
    /// // Height is the distance between a and b
    /// assert_eq!(capsule.height(), 5.0); // 3-4-5 triangle
    ///
    /// // Center is the midpoint
    /// let center = capsule.center();
    /// assert_eq!(center, Vector::new(1.5, 2.0, 0.0));
    /// # }
    /// ```
    pub fn new(a: Vector, b: Vector, radius: Real) -> Self {
        let segment = Segment::new(a, b);
        Self { segment, radius }
    }

    /// Returns the length of the capsule's central segment.
    ///
    /// This is the distance between the two endpoints, **not** including the
    /// hemispherical caps. The total length of the capsule including caps is
    /// `height() + 2 * radius`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    ///
    /// let capsule = Capsule::new_y(3.0, 0.5);
    ///
    /// // Height of the central segment
    /// assert_eq!(capsule.height(), 6.0);
    ///
    /// // Total length including spherical caps
    /// let total_length = capsule.height() + 2.0 * capsule.radius;
    /// assert_eq!(total_length, 7.0);
    /// # }
    /// ```
    pub fn height(&self) -> Real {
        (self.segment.b - self.segment.a).length()
    }

    /// Returns half the length of the capsule's central segment.
    ///
    /// This is equivalent to `height() / 2.0`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    ///
    /// let capsule = Capsule::new_y(3.0, 0.5);
    /// assert_eq!(capsule.half_height(), 3.0);
    /// assert_eq!(capsule.half_height(), capsule.height() / 2.0);
    /// # }
    /// ```
    pub fn half_height(&self) -> Real {
        self.height() / 2.0
    }

    /// Returns the center point of the capsule.
    ///
    /// This is the midpoint between the two endpoints of the central segment.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    /// use parry3d::math::Vector;
    ///
    /// let a = Vector::new(-2.0, 0.0, 0.0);
    /// let b = Vector::new(4.0, 0.0, 0.0);
    /// let capsule = Capsule::new(a, b, 1.0);
    ///
    /// let center = capsule.center();
    /// assert_eq!(center, Vector::new(1.0, 0.0, 0.0));
    /// # }
    /// ```
    pub fn center(&self) -> Vector {
        self.segment.a.midpoint(self.segment.b)
    }

    /// Creates a new capsule equal to `self` with all its endpoints transformed by `pos`.
    ///
    /// This applies a rigid transformation (translation and rotation) to the capsule.
    ///
    /// # Arguments
    ///
    /// * `pos` - The isometry (rigid transformation) to apply
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Capsule;
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let capsule = Capsule::new_y(1.0, 0.5);
    ///
    /// // Translate the capsule 5 units along X axis
    /// let transform = Pose::translation(5.0, 0.0, 0.0);
    /// let transformed = capsule.transform_by(&transform);
    ///
    /// // Center moved by 5 units
    /// assert_eq!(transformed.center().x, 5.0);
    /// // Radius unchanged
    /// assert_eq!(transformed.radius, 0.5);
    /// # }
    /// ```
    pub fn transform_by(&self, pos: &Pose) -> Self {
        Self::new(pos * self.segment.a, pos * self.segment.b, self.radius)
    }

    /// The transformation such that `t * Y` is collinear with `b - a` and `t * origin` equals
    /// the capsule's center.
    pub fn canonical_transform(&self) -> Pose {
        let tra = self.center();
        let rot = self.rotation_wrt_y();
        Pose::from_parts(tra, rot)
    }

    /// The rotation `r` such that `r * Y` is collinear with `b - a`.
    pub fn rotation_wrt_y(&self) -> Rotation {
        let mut dir = self.segment.b - self.segment.a;
        if dir.y < 0.0 {
            dir = -dir;
        }
        let dir = dir.normalize_or(Vector::Y);
        Rotation::from_rotation_arc(Vector::Y, dir)
    }

    /// The transform `t` such that `t * Y` is collinear with `b - a` and such that `t * origin = (b + a) / 2.0`.
    pub fn transform_wrt_y(&self) -> Pose {
        let rot = self.rotation_wrt_y();
        Pose::from_parts(self.center(), rot)
    }

    /// Computes a scaled version of this capsule.
    ///
    /// If the scaling factor is non-uniform, then it can’t be represented as
    /// capsule. Instead, a convex polygon approximation (with `nsubdivs`
    /// subdivisions) is returned. Returns `None` if that approximation had degenerate
    /// normals (for example if the scaling factor along one axis is zero).
    #[cfg(all(feature = "dim2", feature = "alloc"))]
    pub fn scaled(
        self,
        scale: Vector,
        nsubdivs: u32,
    ) -> Option<Either<Self, super::ConvexPolygon>> {
        if scale.x != scale.y {
            // The scaled shape is not a capsule.
            let mut vtx = self.to_polyline(nsubdivs);
            vtx.iter_mut().for_each(|pt| *pt *= scale);
            Some(Either::Right(super::ConvexPolygon::from_convex_polyline(
                vtx,
            )?))
        } else {
            let uniform_scale = scale.x;
            Some(Either::Left(Self::new(
                self.segment.a * uniform_scale,
                self.segment.b * uniform_scale,
                self.radius * uniform_scale.abs(),
            )))
        }
    }

    /// Computes a scaled version of this capsule.
    ///
    /// If the scaling factor is non-uniform, then it can’t be represented as
    /// capsule. Instead, a convex polygon approximation (with `nsubdivs`
    /// subdivisions) is returned. Returns `None` if that approximation had degenerate
    /// normals (for example if the scaling factor along one axis is zero).
    #[cfg(all(feature = "dim3", feature = "alloc"))]
    pub fn scaled(
        self,
        scale: Vector,
        nsubdivs: u32,
    ) -> Option<Either<Self, super::ConvexPolyhedron>> {
        if scale.x != scale.y || scale.x != scale.z || scale.y != scale.z {
            // The scaled shape is not a capsule.
            let (mut vtx, idx) = self.to_trimesh(nsubdivs, nsubdivs);
            vtx.iter_mut().for_each(|pt| *pt *= scale);
            Some(Either::Right(super::ConvexPolyhedron::from_convex_mesh(
                vtx, &idx,
            )?))
        } else {
            let uniform_scale = scale.x;
            Some(Either::Left(Self::new(
                self.segment.a * uniform_scale,
                self.segment.b * uniform_scale,
                self.radius * uniform_scale.abs(),
            )))
        }
    }
}

impl SupportMap for Capsule {
    fn local_support_point(&self, dir: Vector) -> Vector {
        let dir = dir.normalize_or(Vector::Y);
        self.local_support_point_toward(dir)
    }

    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        if dir.dot(self.segment.a) > dir.dot(self.segment.b) {
            self.segment.a + dir * self.radius
        } else {
            self.segment.b + dir * self.radius
        }
    }
}
