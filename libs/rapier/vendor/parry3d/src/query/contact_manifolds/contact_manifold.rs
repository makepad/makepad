use crate::math::{Pose, Real, Vector};
use crate::shape::PackedFeatureId;
#[cfg(feature = "dim3")]
use alloc::vec::Vec;

#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
/// A single contact point between two shapes.
///
/// A `TrackedContact` represents a single point of contact between two shapes, with enough
/// information to track the contact across multiple frames and identify which geometric
/// features (vertices, edges, faces) are in contact.
///
/// # Understanding Contact Points
///
/// Each contact point consists of:
/// - Two contact positions (one on each shape, in local coordinates)
/// - A distance value (negative = penetrating, positive = separated)
/// - Feature IDs that identify which part of each shape is in contact
/// - Optional user data for tracking contact-specific information
///
/// # Local vs World Space
///
/// Contact points are stored in **local space** (the coordinate system of each shape).
/// This is important because:
/// - Shapes can move and rotate, but local coordinates remain constant
/// - Contact tracking works by comparing feature IDs and local positions
/// - To get world-space positions, transform the local points by the shape's position
///
/// # Distance Convention
///
/// The `dist` field uses the following convention:
/// - `dist < 0.0`: Shapes are penetrating (overlapping). The absolute value is the penetration depth.
/// - `dist == 0.0`: Shapes are exactly touching.
/// - `dist > 0.0`: Shapes are separated. This happens when using contact prediction.
///
/// # Example: Basic Contact Query
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{ContactManifold, TrackedContact};
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// // Two balls, one slightly overlapping the other
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(1.5, 0.0, 0.0); // Overlapping by 0.5
///
/// let mut manifold = ContactManifold::<(), ()>::new();
/// contact_manifold_ball_ball(&pos12, &ball1, &ball2, 0.0, &mut manifold);
///
/// if let Some(contact) = manifold.points.first() {
///     println!("Penetration depth: {}", -contact.dist);
///     println!("Contact on ball1 (local): {:?}", contact.local_p1);
///     println!("Contact on ball2 (local): {:?}", contact.local_p2);
/// }
/// # }
/// ```
///
/// # Example: Converting to World Space
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{ContactManifold, TrackedContact};
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Position shapes in world space
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(1.5, 0.0, 0.0);
/// let pos12 = pos1.inverse() * pos2; // Relative position
///
/// let mut manifold = ContactManifold::<(), ()>::new();
/// contact_manifold_ball_ball(&pos12, &ball1, &ball2, 0.0, &mut manifold);
///
/// if let Some(contact) = manifold.points.first() {
///     // Convert local positions to world space
///     let world_p1 = pos1 * contact.local_p1;
///     let world_p2 = pos2 * contact.local_p2;
///
///     println!("Contact in world space:");
///     println!("  On ball1: {:?}", world_p1);
///     println!("  On ball2: {:?}", world_p2);
/// }
/// # }
/// ```
///
/// # Feature IDs
///
/// The `fid1` and `fid2` fields identify which geometric features are in contact:
/// - For a ball: Always the face (surface)
/// - For a box: Could be a vertex, edge, or face
/// - For a triangle: Could be a vertex, edge, or the face
///
/// These IDs are used to track contacts across frames. If the same feature IDs appear
/// in consecutive frames, it's likely the same physical contact point.
pub struct TrackedContact<Data> {
    /// The contact point in the local-space of the first shape.
    ///
    /// This is the point on the first shape's surface (or interior if penetrating)
    /// that is closest to or in contact with the second shape.
    pub local_p1: Vector,

    /// The contact point in the local-space of the second shape.
    ///
    /// This is the point on the second shape's surface (or interior if penetrating)
    /// that is closest to or in contact with the first shape.
    pub local_p2: Vector,

    /// The signed distance between the two contact points.
    ///
    /// - Negative values indicate penetration (shapes are overlapping)
    /// - Positive values indicate separation (used with contact prediction)
    /// - Zero means the shapes are exactly touching
    ///
    /// The magnitude represents the distance along the contact normal.
    pub dist: Real,

    /// The feature ID of the first shape involved in the contact.
    ///
    /// This identifies which geometric feature (vertex, edge, or face) of the first
    /// shape is involved in this contact. Used for contact tracking across frames.
    pub fid1: PackedFeatureId,

    /// The feature ID of the second shape involved in the contact.
    ///
    /// This identifies which geometric feature (vertex, edge, or face) of the second
    /// shape is involved in this contact. Used for contact tracking across frames.
    pub fid2: PackedFeatureId,

    /// User-data associated to this contact.
    ///
    /// This can be used to store any additional information you need to track
    /// per-contact, such as:
    /// - Accumulated impulses for warm-starting in physics solvers
    /// - Contact age or lifetime
    /// - Material properties or friction state
    /// - Custom identifiers or flags
    pub data: Data,
}

impl<Data: Default + Copy> TrackedContact<Data> {
    /// Creates a new tracked contact.
    ///
    /// # Arguments
    ///
    /// * `local_p1` - Contact point on the first shape (in its local space)
    /// * `local_p2` - Contact point on the second shape (in its local space)
    /// * `fid1` - Feature ID of the first shape (which part is in contact)
    /// * `fid2` - Feature ID of the second shape (which part is in contact)
    /// * `dist` - Signed distance between the contact points (negative = penetrating)
    ///
    /// The contact data is initialized to its default value.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::TrackedContact;
    /// use parry3d::shape::PackedFeatureId;
    /// use parry3d::math::Vector;
    ///
    /// let contact = TrackedContact::<()>::new(
    ///     Vector::new(1.0, 0.0, 0.0),  // Point on shape 1
    ///     Vector::new(-1.0, 0.0, 0.0), // Point on shape 2
    ///     PackedFeatureId::face(0),    // Face 0 of shape 1
    ///     PackedFeatureId::face(0),    // Face 0 of shape 2
    ///     -0.1,                         // Penetration depth of 0.1
    /// );
    ///
    /// assert_eq!(contact.dist, -0.1);
    /// # }
    /// ```
    pub fn new(
        local_p1: Vector,
        local_p2: Vector,
        fid1: PackedFeatureId,
        fid2: PackedFeatureId,
        dist: Real,
    ) -> Self {
        Self {
            local_p1,
            local_p2,
            fid1,
            fid2,
            dist,
            data: Data::default(),
        }
    }

    /// Creates a new tracked contact where its input may need to be flipped.
    pub fn flipped(
        local_p1: Vector,
        local_p2: Vector,
        fid1: PackedFeatureId,
        fid2: PackedFeatureId,
        dist: Real,
        flipped: bool,
    ) -> Self {
        if !flipped {
            Self::new(local_p1, local_p2, fid1, fid2, dist)
        } else {
            Self::new(local_p2, local_p1, fid2, fid1, dist)
        }
    }

    /// Copy to `self` the geometric information from `contact`.
    pub fn copy_geometry_from(&mut self, contact: Self) {
        self.local_p1 = contact.local_p1;
        self.local_p2 = contact.local_p2;
        self.fid1 = contact.fid1;
        self.fid2 = contact.fid2;
        self.dist = contact.dist;
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A contact manifold between two shapes.
///
/// A `ContactManifold` describes a collection of contact points between two shapes that share
/// the same contact normal and contact kinematics. This is a fundamental data structure for
/// physics simulation, providing stable and persistent contact information across multiple frames.
///
/// # Key Concepts
///
/// ## What is a Contact Manifold?
///
/// Instead of treating each contact point independently, a contact manifold groups together
/// all contact points that share the same properties:
/// - **Same contact normal**: All contacts push the shapes apart in the same direction
/// - **Same contact kinematics**: All contacts describe the same type of interaction
/// - **Coherent geometry**: All contacts belong to the same collision feature pair
///
/// For example, when a box sits on a plane, you get a manifold with 4 contact points (one
/// for each corner of the box touching the plane), all sharing the same upward normal.
///
/// ## Why Use Manifolds?
///
/// Contact manifolds are essential for stable physics simulation:
/// 1. **Stability**: Multiple contact points prevent rotation and provide stable support
/// 2. **Performance**: Grouped contacts can be processed more efficiently
/// 3. **Persistence**: Contact tracking across frames enables warm-starting and reduces jitter
/// 4. **Natural representation**: Matches the physical reality of contact patches
///
/// # Generic Parameters
///
/// - `ManifoldData`: User-defined data associated with the entire manifold
/// - `ContactData`: User-defined data associated with each individual contact point
///
/// Both can be `()` if you don't need to store additional data.
///
/// # Examples
///
/// ## Basic Usage: Two Balls Colliding
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{ContactManifold, TrackedContact};
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// // Create two balls
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Position them so they overlap
/// let pos12 = Pose::translation(1.5, 0.0, 0.0); // Overlapping by 0.5
///
/// // Create an empty manifold
/// let mut manifold = ContactManifold::<(), ()>::new();
///
/// // Compute contacts (no prediction distance)
/// contact_manifold_ball_ball(&pos12, &ball1, &ball2, 0.0, &mut manifold);
///
/// // Check the results
/// assert!(!manifold.points.is_empty());
/// println!("Number of contacts: {}", manifold.points.len());
/// println!("Contact normal (local): {:?}", manifold.local_n1);
///
/// if let Some(contact) = manifold.points.first() {
///     println!("Penetration depth: {}", -contact.dist);
/// }
/// # }
/// ```
///
/// ## Contact Prediction
///
/// Contact prediction allows detecting contacts before shapes actually touch,
/// which is useful for continuous collision detection:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::ContactManifold;
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Balls are separated by 0.1
/// let pos12 = Pose::translation(2.1, 0.0, 0.0);
///
/// let mut manifold = ContactManifold::<(), ()>::new();
///
/// // With prediction distance of 0.2, we can detect the near-contact
/// let prediction = 0.2;
/// contact_manifold_ball_ball(&pos12, &ball1, &ball2, prediction, &mut manifold);
///
/// if !manifold.points.is_empty() {
///     let contact = &manifold.points[0];
///     println!("Predicted contact distance: {}", contact.dist);
///     assert!(contact.dist > 0.0); // Positive = separated but predicted
/// }
/// # }
/// ```
///
/// ## Efficient Contact Updates with Spatial Coherence
///
/// One of the main benefits of contact manifolds is efficient updates:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::ContactManifold;
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let mut manifold = ContactManifold::<(), ()>::new();
///
/// // Frame 1: Initial contact
/// let pos12_frame1 = Pose::translation(1.9, 0.0, 0.0);
/// contact_manifold_ball_ball(&pos12_frame1, &ball1, &ball2, 0.1, &mut manifold);
/// println!("Frame 1: {} contacts", manifold.points.len());
///
/// // Frame 2: Small movement - try to update efficiently
/// let pos12_frame2 = Pose::translation(1.85, 0.0, 0.0);
///
/// if manifold.try_update_contacts(&pos12_frame2) {
///     println!("Successfully updated contacts using spatial coherence");
/// } else {
///     println!("Shapes moved too much, recomputing from scratch");
///     contact_manifold_ball_ball(&pos12_frame2, &ball1, &ball2, 0.1, &mut manifold);
/// }
/// # }
/// ```
///
/// ## Working with Multiple Contacts
///
/// Some shape pairs produce multiple contact points:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::ContactManifold;
/// use parry3d::query::details::contact_manifold_cuboid_cuboid;
/// use parry3d::shape::Cuboid;
/// use parry3d::math::{Pose, Vector};
///
/// // Two boxes
/// let cuboid1 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let cuboid2 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // One box sitting on top of another
/// let pos12 = Pose::translation(0.0, 1.9, 0.0); // Slight overlap
///
/// let mut manifold = ContactManifold::<(), ()>::new();
/// contact_manifold_cuboid_cuboid(&pos12, &cuboid1, &cuboid2, 0.0, &mut manifold);
///
/// println!("Number of contact points: {}", manifold.points.len());
///
/// // Find the deepest penetration
/// if let Some(deepest) = manifold.find_deepest_contact() {
///     println!("Deepest penetration: {}", -deepest.dist);
/// }
///
/// // Iterate over all contacts
/// for (i, contact) in manifold.points.iter().enumerate() {
///     println!("Contact {}: dist={}, fid1={:?}, fid2={:?}",
///              i, contact.dist, contact.fid1, contact.fid2);
/// }
/// # }
/// ```
///
/// ## Storing Custom Data
///
/// You can attach custom data to both the manifold and individual contacts:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::ContactManifold;
/// use parry3d::query::details::contact_manifold_ball_ball;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// // Custom data structures
/// #[derive(Clone, Default, Copy)]
/// struct MyManifoldData {
///     collision_id: u32,
///     first_contact_frame: u32,
/// }
///
/// #[derive(Clone, Default, Copy)]
/// struct MyContactData {
///     accumulated_impulse: f32,
///     contact_age: u32,
/// }
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(1.8, 0.0, 0.0);
///
/// // Create manifold with custom data
/// let manifold_data = MyManifoldData {
///     collision_id: 42,
///     first_contact_frame: 100,
/// };
/// let mut manifold: ContactManifold<MyManifoldData, MyContactData> =
///     ContactManifold::with_data(0, 0, manifold_data);
///
/// contact_manifold_ball_ball(&pos12, &ball1, &ball2, 0.0, &mut manifold);
///
/// // Access manifold data
/// println!("Collision ID: {}", manifold.data.collision_id);
///
/// // Set contact-specific data
/// if let Some(contact) = manifold.points.first_mut() {
///     contact.data.accumulated_impulse = 10.0;
///     contact.data.contact_age = 5;
/// }
/// # }
/// ```
///
/// # Contact Normal Convention
///
/// The contact normal (`local_n1` and `local_n2`) points from the first shape toward the
/// second shape. To separate the shapes:
/// - Move shape 1 in the direction of `-local_n1`
/// - Move shape 2 in the direction of `local_n2` (which equals `-local_n1` in world space)
///
/// # Working with Composite Shapes
///
/// When dealing with composite shapes (like triangle meshes or compounds), the manifold
/// tracks which subshapes are involved:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::ContactManifold;
///
/// // For composite shapes, the manifold tracks subshape indices
/// let manifold = ContactManifold::<(), ()>::with_data(
///     5,  // subshape1: 5th subshape of first shape
///     12, // subshape2: 12th subshape of second shape
///     (), // manifold data
/// );
///
/// println!("Contact between subshape {} and {}",
///          manifold.subshape1, manifold.subshape2);
/// # }
/// ```
pub struct ContactManifold<ManifoldData, ContactData> {
    // NOTE: use a SmallVec instead?
    // And for 2D use an ArrayVec since there will never be more than 2 contacts anyways.
    /// The contacts points.
    #[cfg(feature = "dim2")]
    pub points: arrayvec::ArrayVec<TrackedContact<ContactData>, 2>,
    /// The contacts points.
    #[cfg(feature = "dim3")]
    pub points: Vec<TrackedContact<ContactData>>,
    /// The contact normal of all the contacts of this manifold, expressed in the local space of the first shape.
    pub local_n1: Vector,
    /// The contact normal of all the contacts of this manifold, expressed in the local space of the second shape.
    pub local_n2: Vector,
    /// The first subshape involved in this contact manifold.
    ///
    /// This is zero if the first shape is not a composite shape.
    pub subshape1: u32,
    /// The second subshape involved in this contact manifold.
    ///
    /// This is zero if the second shape is not a composite shape.
    pub subshape2: u32,
    /// If the first shape involved is a composite shape, this contains the position of its subshape
    /// involved in this contact.
    pub subshape_pos1: Option<Pose>,
    /// If the second shape involved is a composite shape, this contains the position of its subshape
    /// involved in this contact.
    pub subshape_pos2: Option<Pose>,
    /// Additional tracked data associated to this contact manifold.
    pub data: ManifoldData,
}

impl<ManifoldData, ContactData: Default + Copy> ContactManifold<ManifoldData, ContactData> {
    /// Create a new empty contact-manifold.
    pub fn new() -> Self
    where
        ManifoldData: Default,
    {
        Self::default()
    }

    /// Create a new empty contact-manifold with the given associated data.
    pub fn with_data(subshape1: u32, subshape2: u32, data: ManifoldData) -> Self {
        Self {
            #[cfg(feature = "dim2")]
            points: arrayvec::ArrayVec::new(),
            #[cfg(feature = "dim3")]
            points: Vec::new(),
            local_n1: Vector::ZERO,
            local_n2: Vector::ZERO,
            subshape1,
            subshape2,
            subshape_pos1: None,
            subshape_pos2: None,
            data,
        }
    }

    /// Clones `self` and then remove all contact points from `self`.
    pub fn take(&mut self) -> Self
    where
        ManifoldData: Clone,
    {
        #[cfg(feature = "dim2")]
        let points = self.points.clone();
        #[cfg(feature = "dim3")]
        let points = core::mem::take(&mut self.points);
        self.points.clear();

        ContactManifold {
            points,
            local_n1: self.local_n1,
            local_n2: self.local_n2,
            subshape1: self.subshape1,
            subshape2: self.subshape2,
            subshape_pos1: self.subshape_pos1,
            subshape_pos2: self.subshape_pos2,
            data: self.data.clone(),
        }
    }

    /*
    pub(crate) fn single_manifold<'a, 'b>(
        manifolds: &mut Vec<Self>,
        data: &dyn Fn() -> ManifoldData,
    ) -> &'a mut Self {
        if manifolds.is_empty() {
            let manifold_data = data();
            manifolds.push(ContactManifold::with_data((0, 0), manifold_data));
        }

        &mut manifolds[0]
    }
    */

    /// Returns a slice of all the contact points in this manifold.
    ///
    /// This provides read-only access to all contact points. The contacts are stored
    /// in the order they were added during manifold computation.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::ContactManifold;
    /// use parry3d::query::details::contact_manifold_ball_ball;
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    /// let pos12 = Pose::translation(1.8, 0.0, 0.0);
    ///
    /// let mut manifold = ContactManifold::<(), ()>::new();
    /// contact_manifold_ball_ball(&pos12, &ball1, &ball2, 0.0, &mut manifold);
    ///
    /// // Access all contacts
    /// for (i, contact) in manifold.contacts().iter().enumerate() {
    ///     println!("Contact {}: distance = {}", i, contact.dist);
    /// }
    /// # }
    /// ```
    #[inline]
    pub fn contacts(&self) -> &[TrackedContact<ContactData>] {
        &self.points
    }

    /// Attempts to efficiently update contact points using spatial coherence.
    ///
    /// This method tries to update the contact points based on the new relative position
    /// of the shapes (`pos12`) without recomputing the entire contact manifold. This is
    /// much faster than full recomputation but only works when:
    /// - The shapes haven't moved or rotated too much
    /// - The contact normal hasn't changed significantly
    /// - The contact configuration is still valid
    ///
    /// Returns `true` if the update succeeded, `false` if full recomputation is needed.
    ///
    /// # When to Use This
    ///
    /// Use this method every frame after the initial contact computation. It exploits
    /// temporal coherence in physics simulation where shapes typically move smoothly.
    /// When it returns `false`, fall back to full contact manifold recomputation.
    ///
    /// # Thresholds
    ///
    /// This method uses default thresholds for angle and distance changes. For custom
    /// thresholds, use [`try_update_contacts_eps`](Self::try_update_contacts_eps).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::ContactManifold;
    /// use parry3d::query::details::contact_manifold_ball_ball;
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    /// let mut manifold = ContactManifold::<(), ()>::new();
    ///
    /// // Initial computation
    /// let pos12_old = Pose::translation(1.9, 0.0, 0.0);
    /// contact_manifold_ball_ball(&pos12_old, &ball1, &ball2, 0.1, &mut manifold);
    ///
    /// // Next frame: shapes moved slightly
    /// let pos12_new = Pose::translation(1.85, 0.05, 0.0);
    ///
    /// if manifold.try_update_contacts(&pos12_new) {
    ///     println!("Updated contacts efficiently!");
    /// } else {
    ///     println!("Need to recompute from scratch");
    ///     contact_manifold_ball_ball(&pos12_new, &ball1, &ball2, 0.1, &mut manifold);
    /// }
    /// # }
    /// ```
    #[inline]
    pub fn try_update_contacts(&mut self, pos12: &Pose) -> bool {
        // const DOT_THRESHOLD: Real = 0.crate::COS_10_DEGREES;
        // const DOT_THRESHOLD: Real = crate::utils::COS_5_DEGREES;
        const DOT_THRESHOLD: Real = crate::utils::COS_1_DEGREES;
        const DIST_SQ_THRESHOLD: Real = 1.0e-6; // TODO: this should not be hard-coded.
        self.try_update_contacts_eps(pos12, DOT_THRESHOLD, DIST_SQ_THRESHOLD)
    }

    /// Attempts to use spatial coherence to update contacts points, using user-defined tolerances.
    #[inline]
    pub fn try_update_contacts_eps(
        &mut self,
        pos12: &Pose,
        angle_dot_threshold: Real,
        dist_sq_threshold: Real,
    ) -> bool {
        if self.points.is_empty() {
            return false;
        }

        let local_n2 = pos12.rotation * self.local_n2;

        if -self.local_n1.dot(local_n2) < angle_dot_threshold {
            return false;
        }

        for pt in &mut self.points {
            let local_p2 = pos12 * pt.local_p2;
            let dpt = local_p2 - pt.local_p1;
            let dist = dpt.dot(self.local_n1);

            if dist * pt.dist < 0.0 {
                // We switched between penetrating/non-penetrating.
                // The may result in other contacts to appear.
                return false;
            }
            let new_local_p1 = local_p2 - self.local_n1 * dist;

            if pt.local_p1.distance_squared(new_local_p1) > dist_sq_threshold {
                return false;
            }

            pt.dist = dist;
            pt.local_p1 = new_local_p1;
        }

        true
    }

    /// Transfers contact data from previous frame's contacts to current contacts based on feature IDs.
    ///
    /// This method is crucial for maintaining persistent contact information across frames.
    /// It matches contacts between the old and new manifolds by comparing their feature IDs
    /// (which geometric features are in contact). When a match is found, the user data is
    /// transferred from the old contact to the new one.
    ///
    /// This enables important physics features like:
    /// - **Warm-starting**: Reusing accumulated impulses speeds up constraint solving
    /// - **Contact aging**: Tracking how long a contact has existed
    /// - **Friction state**: Maintaining tangential impulse information
    ///
    /// # When to Use
    ///
    /// Call this method after recomputing the contact manifold, passing the old contact
    /// points from the previous frame. This preserves contact-specific solver state.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::ContactManifold;
    /// use parry3d::query::details::contact_manifold_ball_ball;
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// #[derive(Clone, Default, Copy)]
    /// struct MyContactData {
    ///     accumulated_impulse: f32,
    ///     age: u32,
    /// }
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    /// let mut manifold = ContactManifold::<(), MyContactData>::new();
    ///
    /// // Frame 1: Compute contacts
    /// let pos12_frame1 = Pose::translation(1.9, 0.0, 0.0);
    /// contact_manifold_ball_ball(&pos12_frame1, &ball1, &ball2, 0.0, &mut manifold);
    ///
    /// // Simulate physics, accumulate impulse
    /// if let Some(contact) = manifold.points.first_mut() {
    ///     contact.data.accumulated_impulse = 42.0;
    ///     contact.data.age = 1;
    /// }
    ///
    /// // Frame 2: Save old contacts, recompute
    /// let old_contacts = manifold.points.clone();
    /// let pos12_frame2 = Pose::translation(1.85, 0.0, 0.0);
    /// contact_manifold_ball_ball(&pos12_frame2, &ball1, &ball2, 0.0, &mut manifold);
    ///
    /// // Transfer data from old to new based on feature ID matching
    /// manifold.match_contacts(&old_contacts);
    ///
    /// // Data is preserved!
    /// if let Some(contact) = manifold.points.first() {
    ///     assert_eq!(contact.data.accumulated_impulse, 42.0);
    /// }
    /// # }
    /// ```
    pub fn match_contacts(&mut self, old_contacts: &[TrackedContact<ContactData>]) {
        for contact in &mut self.points {
            for old_contact in old_contacts {
                if contact.fid1 == old_contact.fid1 && contact.fid2 == old_contact.fid2 {
                    // Transfer the tracked data.
                    contact.data = old_contact.data;
                }
            }
        }
    }

    /// Copy data associated to contacts from `old_contacts` to the new contacts in `self`
    /// based on matching the contact positions.
    pub fn match_contacts_using_positions(
        &mut self,
        old_contacts: &[TrackedContact<ContactData>],
        dist_threshold: Real,
    ) {
        let sq_threshold = dist_threshold * dist_threshold;
        for contact in &mut self.points {
            for old_contact in old_contacts {
                if contact.local_p1.distance_squared(old_contact.local_p1) < sq_threshold
                    && contact.local_p2.distance_squared(old_contact.local_p2) < sq_threshold
                {
                    // Transfer the tracked data.
                    contact.data = old_contact.data;
                }
            }
        }
    }

    /// Removes all the contacts from `self`.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Finds and returns the contact with the deepest penetration.
    ///
    /// This returns the contact with the smallest (most negative) distance value,
    /// which corresponds to the largest penetration depth. Returns `None` if the
    /// manifold has no contact points.
    ///
    /// # Use Cases
    ///
    /// - Finding the primary contact for simplified physics resolution
    /// - Determining the severity of an overlap for collision response
    /// - Prioritizing contacts in contact reduction algorithms
    /// - Debug visualization of the most significant contact
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::ContactManifold;
    /// use parry3d::query::details::contact_manifold_cuboid_cuboid;
    /// use parry3d::shape::Cuboid;
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let cuboid1 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
    /// let cuboid2 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
    ///
    /// // Position with some penetration
    /// let pos12 = Pose::translation(0.0, 1.8, 0.0);
    ///
    /// let mut manifold = ContactManifold::<(), ()>::new();
    /// contact_manifold_cuboid_cuboid(&pos12, &cuboid1, &cuboid2, 0.0, &mut manifold);
    ///
    /// if let Some(deepest) = manifold.find_deepest_contact() {
    ///     let penetration_depth = -deepest.dist;
    ///     println!("Maximum penetration: {}", penetration_depth);
    ///     println!("Deepest contact point (shape 1): {:?}", deepest.local_p1);
    /// }
    /// # }
    /// ```
    pub fn find_deepest_contact(&self) -> Option<&TrackedContact<ContactData>> {
        let mut deepest = self.points.first()?;

        for pt in &self.points {
            if pt.dist < deepest.dist {
                deepest = pt;
            }
        }

        Some(deepest)
    }
}
