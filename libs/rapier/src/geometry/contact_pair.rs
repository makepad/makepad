#[cfg(doc)]
use super::Collider;
use super::CollisionEvent;
use crate::dynamics::{RigidBodyHandle, RigidBodySet};
use crate::geometry::{ColliderHandle, ColliderSet, Contact, ContactManifold};
use crate::math::{Real, TangentImpulse, Vector};
use crate::pipeline::EventHandler;
use crate::prelude::CollisionEventFlags;
use crate::utils::ScalarType;
use parry::math::{SIMD_WIDTH, SimdReal};
use parry::query::ContactManifoldsWorkspace;

bitflags::bitflags! {
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    /// Flags affecting the behavior of the constraints solver for a given contact manifold.
    pub struct SolverFlags: u32 {
        /// The constraint solver will take this contact manifold into
        /// account for force computation.
        const COMPUTE_IMPULSES = 0b001;
    }
}

impl Default for SolverFlags {
    fn default() -> Self {
        SolverFlags::COMPUTE_IMPULSES
    }
}

#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A single contact between two collider.
pub struct ContactData {
    /// The impulse, along the contact normal, applied by this contact to the first collider's rigid-body.
    ///
    /// The impulse applied to the second collider's rigid-body is given by `-impulse`.
    pub impulse: Real,
    /// The friction impulse along the vector orthonormal to the contact normal, applied to the first
    /// collider's rigid-body.
    pub tangent_impulse: TangentImpulse<Real>,
    /// The impulse retained for warmstarting the next simulation step.
    pub warmstart_impulse: Real,
    /// The friction impulse retained for warmstarting the next simulation step.
    pub warmstart_tangent_impulse: TangentImpulse<Real>,
    /// The twist impulse retained for warmstarting the next simulation step.
    #[cfg(feature = "dim3")]
    pub warmstart_twist_impulse: Real,
}

impl Default for ContactData {
    fn default() -> Self {
        Self {
            impulse: 0.0,
            tangent_impulse: na::zero(),
            warmstart_impulse: 0.0,
            warmstart_tangent_impulse: na::zero(),
            #[cfg(feature = "dim3")]
            warmstart_twist_impulse: 0.0,
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug)]
/// The description of all the contacts between a pair of colliders.
pub struct IntersectionPair {
    /// Are the colliders intersecting?
    pub intersecting: bool,
    /// Was a `CollisionEvent::Started` emitted for this collider?
    pub(crate) start_event_emitted: bool,
}

impl IntersectionPair {
    pub(crate) fn new() -> Self {
        Self {
            intersecting: false,
            start_event_emitted: false,
        }
    }

    pub(crate) fn emit_start_event(
        &mut self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        collider1: ColliderHandle,
        collider2: ColliderHandle,
        events: &dyn EventHandler,
    ) {
        self.start_event_emitted = true;
        events.handle_collision_event(
            bodies,
            colliders,
            CollisionEvent::Started(collider1, collider2, CollisionEventFlags::SENSOR),
            None,
        );
    }

    pub(crate) fn emit_stop_event(
        &mut self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        collider1: ColliderHandle,
        collider2: ColliderHandle,
        events: &dyn EventHandler,
    ) {
        self.start_event_emitted = false;
        events.handle_collision_event(
            bodies,
            colliders,
            CollisionEvent::Stopped(collider1, collider2, CollisionEventFlags::SENSOR),
            None,
        );
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone)]
/// All contact information between two colliding colliders.
///
/// When two colliders are touching, a ContactPair stores all the contact points, normals,
/// and forces between them. You can access this through the narrow phase or in event handlers.
///
/// ## Contact manifolds
///
/// The contacts are organized into "manifolds" - groups of contact points that share similar
/// properties (like being on the same face). Most collider pairs have 1 manifold, but complex
/// shapes may have multiple.
///
/// ## Use cases
///
/// - Reading contact normals for custom physics
/// - Checking penetration depth
/// - Analyzing impact forces
/// - Implementing custom contact responses
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # use rapier3d::geometry::ContactPair;
/// # let contact_pair = ContactPair::default();
/// if let Some((manifold, contact)) = contact_pair.find_deepest_contact() {
///     println!("Deepest penetration: {}", -contact.dist);
///     println!("Contact normal: {:?}", manifold.data.normal);
/// }
/// ```
pub struct ContactPair {
    /// The first collider involved in the contact pair.
    pub collider1: ColliderHandle,
    /// The second collider involved in the contact pair.
    pub collider2: ColliderHandle,
    /// The set of contact manifolds between the two colliders.
    ///
    /// All contact manifold contain themselves contact points between the colliders.
    /// Note that contact points in the contact manifold do not take into account the
    /// [`Collider::contact_skin`] which only affects the constraint solver and the
    /// [`SolverContact`].
    pub manifolds: Vec<ContactManifold>,
    /// Was a `CollisionEvent::Started` emitted for this collider?
    pub(crate) start_event_emitted: bool,
    pub(crate) workspace: Option<ContactManifoldsWorkspace>,
}

impl Default for ContactPair {
    fn default() -> Self {
        Self::new(ColliderHandle::invalid(), ColliderHandle::invalid())
    }
}

impl ContactPair {
    pub(crate) fn new(collider1: ColliderHandle, collider2: ColliderHandle) -> Self {
        Self {
            collider1,
            collider2,
            manifolds: Vec::new(),
            start_event_emitted: false,
            workspace: None,
        }
    }

    /// Is there any active contact in this contact pair?
    pub fn has_any_active_contact(&self) -> bool {
        self.manifolds
            .iter()
            .any(|m| !m.data.solver_contacts.is_empty())
    }

    /// Clears all the contacts of this contact pair.
    pub fn clear(&mut self) {
        self.manifolds.clear();
        self.workspace = None;
    }

    /// The total impulse (force × time) applied by all contacts.
    ///
    /// This is the accumulated force that pushed the colliders apart.
    /// Useful for determining impact strength.
    pub fn total_impulse(&self) -> Vector {
        self.manifolds
            .iter()
            .map(|m| m.total_impulse() * m.data.normal)
            .sum()
    }

    /// The total magnitude of all contact impulses (sum of lengths, not length of sum).
    ///
    /// This is what's compared against `contact_force_event_threshold`.
    pub fn total_impulse_magnitude(&self) -> Real {
        self.manifolds
            .iter()
            .fold(0.0, |a, m| a + m.total_impulse())
    }

    /// Finds the strongest contact impulse and its direction.
    ///
    /// Returns `(magnitude, normal_direction)` of the strongest individual contact.
    pub fn max_impulse(&self) -> (Real, Vector) {
        let mut result = (0.0, Vector::ZERO);

        for m in &self.manifolds {
            let impulse = m.total_impulse();

            if impulse > result.0 {
                result = (impulse, m.data.normal);
            }
        }

        result
    }

    /// Finds the contact point with the deepest penetration.
    ///
    /// When objects overlap, this returns the contact point that's penetrating the most.
    /// Useful for:
    /// - Finding the "worst" overlap
    /// - Determining primary contact direction
    /// - Custom penetration resolution
    ///
    /// Returns both the contact point and its parent manifold.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # use rapier3d::geometry::ContactPair;
    /// # let pair = ContactPair::default();
    /// if let Some((manifold, contact)) = pair.find_deepest_contact() {
    ///     let penetration_depth = -contact.dist;  // Negative dist = penetration
    ///     println!("Deepest penetration: {} units", penetration_depth);
    /// }
    /// ```
    pub fn find_deepest_contact(&self) -> Option<(&ContactManifold, &Contact)> {
        let mut deepest = None;

        for m2 in &self.manifolds {
            let deepest_candidate = m2.find_deepest_contact();

            deepest = match (deepest, deepest_candidate) {
                (_, None) => deepest,
                (None, Some(c2)) => Some((m2, c2)),
                (Some((m1, c1)), Some(c2)) => {
                    if c1.dist <= c2.dist {
                        Some((m1, c1))
                    } else {
                        Some((m2, c2))
                    }
                }
            }
        }

        deepest
    }

    pub(crate) fn emit_start_event(
        &mut self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        events: &dyn EventHandler,
    ) {
        self.start_event_emitted = true;

        events.handle_collision_event(
            bodies,
            colliders,
            CollisionEvent::Started(self.collider1, self.collider2, CollisionEventFlags::empty()),
            Some(self),
        );
    }

    pub(crate) fn emit_stop_event(
        &mut self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        events: &dyn EventHandler,
    ) {
        self.start_event_emitted = false;

        events.handle_collision_event(
            bodies,
            colliders,
            CollisionEvent::Stopped(self.collider1, self.collider2, CollisionEventFlags::empty()),
            Some(self),
        );
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A contact manifold between two colliders.
///
/// A contact manifold describes a set of contacts between two colliders. All the contact
/// part of the same contact manifold share the same contact normal and contact kinematics.
pub struct ContactManifoldData {
    // The following are set by the narrow-phase.
    /// The first rigid-body involved in this contact manifold.
    pub rigid_body1: Option<RigidBodyHandle>,
    /// The second rigid-body involved in this contact manifold.
    pub rigid_body2: Option<RigidBodyHandle>,
    // We put the following fields here to avoids reading the colliders inside of the
    // contact preparation method.
    /// Flags used to control some aspects of the constraints solver for this contact manifold.
    pub solver_flags: SolverFlags,
    /// The world-space contact normal shared by all the contact in this contact manifold.
    // NOTE: read the comment of `solver_contacts` regarding serialization. It applies
    // to this field as well.
    pub normal: Vector,
    /// The contacts that will be seen by the constraints solver for computing forces.
    // NOTE: unfortunately, we can't ignore this field when serialize
    // the contact manifold data. The reason is that the solver contacts
    // won't be updated for sleeping bodies. So it means that for one
    // frame, we won't have any solver contacts when waking up an island
    // after a deserialization. Not only does this break post-snapshot
    // determinism, but it will also skip constraint resolution for these
    // contacts during one frame.
    //
    // An alternative would be to skip the serialization of `solver_contacts` and
    // find a way to recompute them right after the deserialization process completes.
    // However, this would be an expensive operation. And doing this efficiently as part
    // of the narrow-phase update or the contact manifold collect will likely lead to tricky
    // bugs too.
    //
    // So right now it is best to just serialize this field and keep it that way until it
    // is proven to be actually problematic in real applications (in terms of snapshot size for example).
    pub solver_contacts: Vec<SolverContact>,
    /// The relative dominance of the bodies involved in this contact manifold.
    pub relative_dominance: i16,
    /// A user-defined piece of data.
    pub user_data: u32,
}

/// A single solver contact.
pub type SolverContact = SolverContactGeneric<Real, 1>;
/// A group of `SIMD_WIDTH` solver contacts stored in SoA fashion for SIMD optimizations.
pub type SimdSolverContact = SolverContactGeneric<SimdReal, SIMD_WIDTH>;

/// A contact seen by the constraints solver for computing forces.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "serde-serialize",
    serde(bound(
        serialize = "N: serde::Serialize, N::Vector: serde::Serialize, [u32; LANES]: serde::Serialize"
    ))
)]
#[cfg_attr(
    feature = "serde-serialize",
    serde(bound(
        deserialize = "N: serde::Deserialize<'de>, N::Vector: serde::Deserialize<'de>, [u32; LANES]: serde::Deserialize<'de>"
    ))
)]
#[repr(C)]
#[repr(align(16))]
pub struct SolverContactGeneric<N: ScalarType, const LANES: usize> {
    // IMPORTANT: don’t change the fields unless `SimdSolverContactRepr` is also changed.
    //
    // TOTAL: 11/14 = 3*4/4*4-1
    /// The contact point in world-space.
    pub point: N::Vector, // 2/3
    /// The distance between the two original contacts points along the contact normal.
    /// If negative, this is measures the penetration depth.
    pub dist: N, // 1/1
    /// The effective friction coefficient at this contact point.
    pub friction: N, // 1/1
    /// The effective restitution coefficient at this contact point.
    pub restitution: N, // 1/1
    /// The desired tangent relative velocity at the contact point.
    ///
    /// This is set to zero by default. Set to a non-zero value to
    /// simulate, e.g., conveyor belts.
    pub tangent_velocity: N::Vector, // 2/3
    /// Impulse used to warmstart the solve for the normal constraint.
    pub warmstart_impulse: N, // 1/1
    /// Impulse used to warmstart the solve for the friction constraints.
    pub warmstart_tangent_impulse: TangentImpulse<N>, // 1/2
    /// Impulse used to warmstart the solve for the twist friction constraints.
    pub warmstart_twist_impulse: N, // 1/1
    /// Whether this contact existed during the last timestep.
    ///
    /// A value of 0.0 means `false` and `1.0` means `true`.
    /// This isn’t a bool for optimizations purpose with SIMD.
    pub is_new: N, // 1/1
    /// The index of the manifold contact used to generate this solver contact.
    pub contact_id: [u32; LANES], // 1/1
    #[cfg(feature = "dim3")]
    pub(crate) padding: [N; 1],
}

#[repr(C)]
#[repr(align(16))]
pub struct SimdSolverContactRepr {
    data0: SimdReal,
    data1: SimdReal,
    data2: SimdReal,
    #[cfg(feature = "dim3")]
    data3: SimdReal,
}

// NOTE: if these assertion fail with a weird "0 - 1 would overflow" error, it means the equality doesn’t hold.
static_assertions::const_assert_eq!(
    align_of::<SimdSolverContactRepr>(),
    align_of::<SolverContact>()
);
#[cfg(feature = "simd-is-enabled")]
static_assertions::assert_eq_size!(SimdSolverContactRepr, SolverContact);
static_assertions::const_assert_eq!(
    align_of::<SimdSolverContact>(),
    align_of::<[SolverContact; SIMD_WIDTH]>()
);
#[cfg(feature = "simd-is-enabled")]
static_assertions::assert_eq_size!(SimdSolverContact, [SolverContact; SIMD_WIDTH]);

impl SimdSolverContact {
    #[cfg(not(feature = "simd-is-enabled"))]
    pub unsafe fn gather_unchecked(contacts: &[&[SolverContact]; SIMD_WIDTH], k: usize) -> Self {
        contacts[0][k]
    }

    #[cfg(feature = "simd-is-enabled")]
    pub unsafe fn gather_unchecked(contacts: &[&[SolverContact]; SIMD_WIDTH], k: usize) -> Self {
        // TODO PERF: double-check that the compiler is using simd loads and
        //       isn’t generating useless copies.

        let data_repr: &[&[SimdSolverContactRepr]; SIMD_WIDTH] =
            unsafe { std::mem::transmute(contacts) };

        /* NOTE: this is a manual NEON implementation. To compare with what the compiler generates with `wide`.
        unsafe {
            use std::arch::aarch64::*;

            assert!(k < SIMD_WIDTH);

            // Fetch.
            let aos0_0 = vld1q_f32(&data_repr[0][k].data0.0 as *const _ as *const f32);
            let aos0_1 = vld1q_f32(&data_repr[1][k].data0.0 as *const _ as *const f32);
            let aos0_2 = vld1q_f32(&data_repr[2][k].data0.0 as *const _ as *const f32);
            let aos0_3 = vld1q_f32(&data_repr[3][k].data0.0 as *const _ as *const f32);

            let aos1_0 = vld1q_f32(&data_repr[0][k].data1.0 as *const _ as *const f32);
            let aos1_1 = vld1q_f32(&data_repr[1][k].data1.0 as *const _ as *const f32);
            let aos1_2 = vld1q_f32(&data_repr[2][k].data1.0 as *const _ as *const f32);
            let aos1_3 = vld1q_f32(&data_repr[3][k].data1.0 as *const _ as *const f32);

            let aos2_0 = vld1q_f32(&data_repr[0][k].data2.0 as *const _ as *const f32);
            let aos2_1 = vld1q_f32(&data_repr[1][k].data2.0 as *const _ as *const f32);
            let aos2_2 = vld1q_f32(&data_repr[2][k].data2.0 as *const _ as *const f32);
            let aos2_3 = vld1q_f32(&data_repr[3][k].data2.0 as *const _ as *const f32);

            // Transpose.
            let a = vzip1q_f32(aos0_0, aos0_2);
            let b = vzip1q_f32(aos0_1, aos0_3);
            let c = vzip2q_f32(aos0_0, aos0_2);
            let d = vzip2q_f32(aos0_1, aos0_3);
            let soa0_0 = vzip1q_f32(a, b);
            let soa0_1 = vzip2q_f32(a, b);
            let soa0_2 = vzip1q_f32(c, d);
            let soa0_3 = vzip2q_f32(c, d);

            let a = vzip1q_f32(aos1_0, aos1_2);
            let b = vzip1q_f32(aos1_1, aos1_3);
            let c = vzip2q_f32(aos1_0, aos1_2);
            let d = vzip2q_f32(aos1_1, aos1_3);
            let soa1_0 = vzip1q_f32(a, b);
            let soa1_1 = vzip2q_f32(a, b);
            let soa1_2 = vzip1q_f32(c, d);
            let soa1_3 = vzip2q_f32(c, d);

            let a = vzip1q_f32(aos2_0, aos2_2);
            let b = vzip1q_f32(aos2_1, aos2_3);
            let c = vzip2q_f32(aos2_0, aos2_2);
            let d = vzip2q_f32(aos2_1, aos2_3);
            let soa2_0 = vzip1q_f32(a, b);
            let soa2_1 = vzip2q_f32(a, b);
            let soa2_2 = vzip1q_f32(c, d);
            let soa2_3 = vzip2q_f32(c, d);

            // Return.
            std::mem::transmute([
                soa0_0, soa0_1, soa0_2, soa0_3, soa1_0, soa1_1, soa1_2, soa1_3, soa2_0, soa2_1,
                soa2_2, soa2_3,
            ])
        }
         */

        let aos0 = [
            unsafe { data_repr[0].get_unchecked(k).data0.0 },
            unsafe { data_repr[1].get_unchecked(k).data0.0 },
            unsafe { data_repr[2].get_unchecked(k).data0.0 },
            unsafe { data_repr[3].get_unchecked(k).data0.0 },
        ];
        let aos1 = [
            unsafe { data_repr[0].get_unchecked(k).data1.0 },
            unsafe { data_repr[1].get_unchecked(k).data1.0 },
            unsafe { data_repr[2].get_unchecked(k).data1.0 },
            unsafe { data_repr[3].get_unchecked(k).data1.0 },
        ];
        let aos2 = [
            unsafe { data_repr[0].get_unchecked(k).data2.0 },
            unsafe { data_repr[1].get_unchecked(k).data2.0 },
            unsafe { data_repr[2].get_unchecked(k).data2.0 },
            unsafe { data_repr[3].get_unchecked(k).data2.0 },
        ];
        #[cfg(feature = "dim3")]
        let aos3 = [
            unsafe { data_repr[0].get_unchecked(k).data3.0 },
            unsafe { data_repr[1].get_unchecked(k).data3.0 },
            unsafe { data_repr[2].get_unchecked(k).data3.0 },
            unsafe { data_repr[3].get_unchecked(k).data3.0 },
        ];

        use crate::utils::transmute_to_wide;
        let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
        let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
        let soa2 = wide::f32x4::transpose(transmute_to_wide(aos2));
        #[cfg(feature = "dim3")]
        let soa3 = wide::f32x4::transpose(transmute_to_wide(aos3));

        #[cfg(feature = "dim2")]
        return unsafe {
            std::mem::transmute::<[[wide::f32x4; 4]; 3], SolverContactGeneric<SimdReal, 4>>([
                soa0, soa1, soa2,
            ])
        };
        #[cfg(feature = "dim3")]
        return unsafe {
            std::mem::transmute::<[[wide::f32x4; 4]; 4], SolverContactGeneric<SimdReal, 4>>([
                soa0, soa1, soa2, soa3,
            ])
        };
    }
}

#[cfg(feature = "simd-is-enabled")]
impl SimdSolverContact {
    /// Should we treat this contact as a bouncy contact?
    /// If `true`, use [`Self::restitution`].
    pub fn is_bouncy(&self) -> SimdReal {
        use na::{SimdPartialOrd, SimdValue};

        let one = SimdReal::splat(1.0);
        let zero = SimdReal::splat(0.0);

        // Treat new collisions as bouncing at first, unless we have zero restitution.
        let if_new = one.select(self.restitution.simd_gt(zero), zero);

        // If the contact is still here one step later, it is now a resting contact.
        // The exception is very high restitutions, which can never rest
        let if_not_new = one.select(self.restitution.simd_ge(one), zero);

        if_new.select(self.is_new.simd_ne(zero), if_not_new)
    }
}

impl SolverContact {
    /// Should we treat this contact as a bouncy contact?
    /// If `true`, use [`Self::restitution`].
    pub fn is_bouncy(&self) -> Real {
        if self.is_new != 0.0 {
            // Treat new collisions as bouncing at first, unless we have zero restitution.
            (self.restitution > 0.0) as u32 as Real
        } else {
            // If the contact is still here one step later, it is now a resting contact.
            // The exception is very high restitutions, which can never rest
            (self.restitution >= 1.0) as u32 as Real
        }
    }
}

impl Default for ContactManifoldData {
    fn default() -> Self {
        Self::new(None, None, SolverFlags::empty())
    }
}

impl ContactManifoldData {
    pub(crate) fn new(
        rigid_body1: Option<RigidBodyHandle>,
        rigid_body2: Option<RigidBodyHandle>,
        solver_flags: SolverFlags,
    ) -> ContactManifoldData {
        Self {
            rigid_body1,
            rigid_body2,
            solver_flags,
            normal: Vector::ZERO,
            solver_contacts: Vec::new(),
            relative_dominance: 0,
            user_data: 0,
        }
    }

    /// Number of actives contacts, i.e., contacts that will be seen by
    /// the constraints solver.
    #[inline]
    pub fn num_active_contacts(&self) -> usize {
        self.solver_contacts.len()
    }
}

/// Additional methods for the contact manifold.
pub trait ContactManifoldExt {
    /// Computes the sum of all the impulses applied by contacts from this contact manifold.
    fn total_impulse(&self) -> Real;
}

impl ContactManifoldExt for ContactManifold {
    fn total_impulse(&self) -> Real {
        self.points.iter().map(|pt| pt.data.impulse).sum()
    }
}
