use crate::math::{Pose, Real, Vector};
use core::mem;

/// Geometric description of a contact between two shapes.
///
/// A contact represents the point(s) where two shapes touch or penetrate. This structure
/// contains all the information needed to resolve collisions: contact points, surface normals,
/// and penetration depth.
///
/// # Contact States
///
/// A `Contact` can represent different collision states:
///
/// - **Touching** (`dist ≈ 0.0`): Shapes are just barely in contact
/// - **Penetrating** (`dist < 0.0`): Shapes are overlapping (negative distance = penetration depth)
/// - **Separated** (`dist > 0.0`): Shapes are close but not touching (rarely used; see `closest_points` instead)
///
/// # Coordinate Systems
///
/// Contact data can be expressed in different coordinate systems:
///
/// - **World space**: Both shapes' transformations applied; `normal2 = -normal1`
/// - **Local space**: Relative to one shape's coordinate system
///
/// # Use Cases
///
/// - **Physics simulation**: Compute collision response forces
/// - **Collision resolution**: Push objects apart when penetrating
/// - **Trigger detection**: Detect when objects touch without resolving
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::contact;
/// use parry3d::shape::Ball;
/// use parry3d::math::{Pose, Vector};
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Overlapping balls (centers 1.5 units apart, combined radii = 2.0)
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(1.5, 0.0, 0.0);
///
/// if let Ok(Some(contact)) = contact(&pos1, &ball1, &pos2, &ball2, 0.0) {
///     // Penetration depth (negative distance)
///     assert!(contact.dist < 0.0);
///     println!("Penetration: {}", -contact.dist); // 0.5 units
///
///     // Normal points from shape 1 toward shape 2
///     println!("Normal: {:?}", contact.normal1);
///
///     // Contact points are on each shape's surface
///     println!("Vector on ball1: {:?}", contact.point1);
///     println!("Vector on ball2: {:?}", contact.point2);
/// }
/// # }
/// ```
#[derive(Debug, PartialEq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct Contact {
    /// Position of the contact point on the first shape's surface.
    ///
    /// This is the point on shape 1 that is closest to (or penetrating into) shape 2.
    /// Expressed in the same coordinate system as the contact (world or local space).
    pub point1: Vector,

    /// Position of the contact point on the second shape's surface.
    ///
    /// This is the point on shape 2 that is closest to (or penetrating into) shape 1.
    /// When shapes are penetrating, this point may be inside shape 1.
    pub point2: Vector,

    /// Contact normal pointing outward from the first shape.
    ///
    /// This unit vector points from shape 1 toward shape 2, perpendicular to the
    /// contact surface. Used to compute separation direction and collision response
    /// forces for shape 1.
    pub normal1: Vector,

    /// Contact normal pointing outward from the second shape.
    ///
    /// In world space, this is always equal to `-normal1`. In local space coordinates,
    /// it may differ due to different shape orientations.
    pub normal2: Vector,

    /// Signed distance between the two contact points.
    ///
    /// - **Positive**: Shapes are separated (distance between surfaces)
    /// - **Zero**: Shapes are exactly touching
    /// - **Negative**: Shapes are penetrating (absolute value = penetration depth)
    ///
    /// For collision resolution, use `-dist` as the penetration depth when `dist < 0.0`.
    pub dist: Real,
}

impl Contact {
    /// Creates a new contact with the given parameters.
    ///
    /// # Arguments
    ///
    /// * `point1` - Contact point on the first shape's surface
    /// * `point2` - Contact point on the second shape's surface
    /// * `normal1` - Unit normal pointing outward from shape 1
    /// * `normal2` - Unit normal pointing outward from shape 2
    /// * `dist` - Signed distance (negative = penetration depth)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Contact;
    /// use parry3d::math::Vector;
    ///
    /// // Create a contact representing two spheres touching
    /// let point1 = Vector::new(1.0, 0.0, 0.0);
    /// let point2 = Vector::new(2.0, 0.0, 0.0);
    /// let normal1 = Vector::new(1.0, 0.0, 0.0).normalize();
    /// let normal2 = Vector::new(-1.0, 0.0, 0.0).normalize();
    ///
    /// let contact = Contact::new(point1, point2, normal1, normal2, 0.0);
    /// assert_eq!(contact.dist, 0.0); // Touching, not penetrating
    /// # }
    /// ```
    #[inline]
    pub fn new(
        point1: Vector,
        point2: Vector,
        normal1: Vector,
        normal2: Vector,
        dist: Real,
    ) -> Self {
        Contact {
            point1,
            point2,
            normal1,
            normal2,
            dist,
        }
    }
}

impl Contact {
    /// Swaps the points and normals of this contact.
    #[inline]
    pub fn flip(&mut self) {
        mem::swap(&mut self.point1, &mut self.point2);
        mem::swap(&mut self.normal1, &mut self.normal2);
    }

    /// Returns a new contact containing the swapped points and normals of `self`.
    #[inline]
    pub fn flipped(mut self) -> Self {
        self.flip();
        self
    }

    /// Transform the points and normals from this contact by
    /// the given transformations.
    #[inline]
    pub fn transform_by_mut(&mut self, pos1: &Pose, pos2: &Pose) {
        self.point1 = pos1 * self.point1;
        self.point2 = pos2 * self.point2;
        self.normal1 = pos1.rotation * self.normal1;
        self.normal2 = pos2.rotation * self.normal2;
    }

    /// Transform `self.point1` and `self.normal1` by the `pos`.
    pub fn transform1_by_mut(&mut self, pos: &Pose) {
        self.point1 = pos * self.point1;
        self.normal1 = pos.rotation * self.normal1;
    }
}
