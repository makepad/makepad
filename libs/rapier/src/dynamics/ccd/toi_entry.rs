use crate::dynamics::{RigidBody, RigidBodyHandle};
use crate::geometry::{Collider, ColliderHandle};
use crate::math::Real;
use parry::query::{NonlinearRigidMotion, QueryDispatcher, ShapeCastOptions};

#[derive(Copy, Clone, Debug)]
pub struct TOIEntry {
    pub toi: Real,
    pub c1: ColliderHandle,
    pub b1: Option<RigidBodyHandle>,
    pub c2: ColliderHandle,
    pub b2: Option<RigidBodyHandle>,
    // We call this "pseudo" intersection because this also
    // includes colliders pairs with mismatching solver_groups.
    pub is_pseudo_intersection_test: bool,
}

impl TOIEntry {
    fn new(
        toi: Real,
        c1: ColliderHandle,
        b1: Option<RigidBodyHandle>,
        c2: ColliderHandle,
        b2: Option<RigidBodyHandle>,
        is_pseudo_intersection_test: bool,
    ) -> Self {
        Self {
            toi,
            c1,
            b1,
            c2,
            b2,
            is_pseudo_intersection_test,
        }
    }
    pub fn try_from_colliders<QD: ?Sized + QueryDispatcher>(
        query_dispatcher: &QD,
        ch1: ColliderHandle,
        ch2: ColliderHandle,
        co1: &Collider,
        co2: &Collider,
        rb1: Option<&RigidBody>,
        rb2: Option<&RigidBody>,
        frozen1: Option<Real>,
        frozen2: Option<Real>,
        start_time: Real,
        end_time: Real,
        smallest_contact_dist: Real,
    ) -> Option<Self> {
        assert!(start_time <= end_time);
        if rb1.is_none() && rb2.is_none() {
            return None;
        }

        let linvel1 =
            frozen1.is_none() as u32 as Real * rb1.map(|b| b.ccd_vels.linvel).unwrap_or_default();
        let linvel2 =
            frozen2.is_none() as u32 as Real * rb2.map(|b| b.ccd_vels.linvel).unwrap_or_default();
        let angvel1 =
            frozen1.is_none() as u32 as Real * rb1.map(|b| b.ccd_vels.angvel).unwrap_or_default();
        let angvel2 =
            frozen2.is_none() as u32 as Real * rb2.map(|b| b.ccd_vels.angvel).unwrap_or_default();

        #[cfg(feature = "dim2")]
        let vel12 = (linvel2 - linvel1).length()
            + angvel1.abs() * rb1.map(|b| b.ccd.ccd_max_dist).unwrap_or(0.0)
            + angvel2.abs() * rb2.map(|b| b.ccd.ccd_max_dist).unwrap_or(0.0);
        #[cfg(feature = "dim3")]
        let vel12 = (linvel2 - linvel1).length()
            + angvel1.length() * rb1.map(|b| b.ccd.ccd_max_dist).unwrap_or(0.0)
            + angvel2.length() * rb2.map(|b| b.ccd.ccd_max_dist).unwrap_or(0.0);

        // We may be slightly over-conservative by taking the `max(0.0)` here.
        // But removing the `max` doesn't really affect performances so let's
        // keep it since more conservatism is good at this stage.
        let thickness = (co1.shape.0.ccd_thickness() + co2.shape.0.ccd_thickness())
            + smallest_contact_dist.max(0.0);
        let is_pseudo_intersection_test = co1.is_sensor()
            || co2.is_sensor()
            || !co1.flags.solver_groups.test(co2.flags.solver_groups);

        if (end_time - start_time) * vel12 < thickness {
            return None;
        }

        // Compute the TOI.
        let identity = NonlinearRigidMotion::identity();
        let mut motion1 = rb1.map(Self::body_motion).unwrap_or(identity);
        let mut motion2 = rb2.map(Self::body_motion).unwrap_or(identity);

        if let Some(t) = frozen1 {
            motion1.freeze(t);
        }

        if let Some(t) = frozen2 {
            motion2.freeze(t);
        }

        let motion_c1 = motion1.prepend(co1.parent.map(|p| p.pos_wrt_parent).unwrap_or(co1.pos.0));
        let motion_c2 = motion2.prepend(co2.parent.map(|p| p.pos_wrt_parent).unwrap_or(co2.pos.0));

        // println!("start_time: {}", start_time);

        // If this is just an intersection test (i.e. with sensors)
        // then we can stop the TOI search immediately if it starts with
        // a penetration because we don't care about the whether the velocity
        // at the impact is a separating velocity or not.
        // If the TOI search involves two non-sensor colliders then
        // we don't want to stop the TOI search at the first penetration
        // because the colliders may be in a separating trajectory.
        let stop_at_penetration = is_pseudo_intersection_test;

        const USE_NONLINEAR_SHAPE_CAST: bool = true;

        let toi = if USE_NONLINEAR_SHAPE_CAST {
            query_dispatcher
                .cast_shapes_nonlinear(
                    &motion_c1,
                    co1.shape.as_ref(),
                    &motion_c2,
                    co2.shape.as_ref(),
                    start_time,
                    end_time,
                    stop_at_penetration,
                )
                .ok()??
        } else {
            let pos12 = motion_c1
                .position_at_time(start_time)
                .inv_mul(&motion_c2.position_at_time(start_time));
            let vel12 = linvel2 - linvel1;
            let options = ShapeCastOptions::with_max_time_of_impact(end_time - start_time);
            let mut hit = query_dispatcher
                .cast_shapes(
                    &pos12,
                    vel12,
                    co1.shape.as_ref(),
                    co2.shape.as_ref(),
                    options,
                )
                .ok()??;
            hit.time_of_impact += start_time;
            hit
        };

        Some(Self::new(
            toi.time_of_impact,
            ch1,
            co1.parent.map(|p| p.handle),
            ch2,
            co2.parent.map(|p| p.handle),
            is_pseudo_intersection_test,
        ))
    }

    fn body_motion(rb: &RigidBody) -> NonlinearRigidMotion {
        if rb.ccd.ccd_active {
            NonlinearRigidMotion::new(
                rb.pos.position,
                rb.mprops.local_mprops.local_com,
                rb.ccd_vels.linvel,
                rb.ccd_vels.angvel,
            )
        } else {
            NonlinearRigidMotion::constant_position(rb.pos.next_position)
        }
    }
}

impl PartialOrd for TOIEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TOIEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (-self.toi)
            .partial_cmp(&(-other.toi))
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialEq for TOIEntry {
    fn eq(&self, other: &Self) -> bool {
        self.toi == other.toi
    }
}

impl Eq for TOIEntry {}
