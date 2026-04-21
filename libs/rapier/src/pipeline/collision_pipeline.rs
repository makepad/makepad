//! Physics pipeline structures.

use crate::dynamics::{ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet};
use crate::geometry::{
    BroadPhaseBvh, BroadPhasePairEvent, ColliderChanges, ColliderHandle, ColliderPair,
    ModifiedColliders, NarrowPhase,
};
use crate::math::Real;
use crate::pipeline::{EventHandler, PhysicsHooks};
use crate::{dynamics::RigidBodySet, geometry::ColliderSet};

/// A collision detection pipeline that can be used without full physics simulation.
///
/// This runs only collision detection (broad-phase + narrow-phase) without dynamics/forces.
/// Use when you want to detect collisions but don't need physics simulation.
///
/// **For full physics**, use [`PhysicsPipeline`](crate::pipeline::PhysicsPipeline) instead which includes this internally.
///
/// ## Use cases
///
/// - Collision detection in a non-physics game
/// - Custom physics integration where you handle forces yourself
/// - Debugging collision detection separately from dynamics
///
/// Like PhysicsPipeline, this only holds temporary buffers. Reuse the same instance for performance.
// NOTE: this contains only workspace data, so there is no point in making this serializable.
pub struct CollisionPipeline {
    broadphase_collider_pairs: Vec<ColliderPair>,
    broad_phase_events: Vec<BroadPhasePairEvent>,
}

#[allow(dead_code)]
fn check_pipeline_send_sync() {
    fn do_test<T: Sync>() {}
    do_test::<CollisionPipeline>();
}

impl Default for CollisionPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl CollisionPipeline {
    /// Initializes a new physics pipeline.
    pub fn new() -> CollisionPipeline {
        CollisionPipeline {
            broadphase_collider_pairs: Vec::new(),
            broad_phase_events: Vec::new(),
        }
    }

    fn detect_collisions(
        &mut self,
        prediction_distance: Real,
        islands: &mut IslandManager,
        broad_phase: &mut BroadPhaseBvh,
        narrow_phase: &mut NarrowPhase,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        modified_colliders: &[ColliderHandle],
        removed_colliders: &[ColliderHandle],
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
        handle_user_changes: bool,
    ) {
        // Update broad-phase.
        self.broad_phase_events.clear();
        self.broadphase_collider_pairs.clear();

        let params = IntegrationParameters {
            normalized_prediction_distance: prediction_distance,
            dt: 0.0,
            ..Default::default()
        };

        broad_phase.update(
            &params,
            colliders,
            bodies,
            modified_colliders,
            removed_colliders,
            &mut self.broad_phase_events,
        );

        // Update narrow-phase.
        if handle_user_changes {
            narrow_phase.handle_user_changes(
                None,
                modified_colliders,
                removed_colliders,
                colliders,
                bodies,
                events,
            );
        }

        narrow_phase.register_pairs(None, colliders, bodies, &self.broad_phase_events, events);
        narrow_phase.compute_contacts(
            prediction_distance,
            0.0,
            islands,
            bodies,
            colliders,
            &ImpulseJointSet::new(),
            &MultibodyJointSet::new(),
            hooks,
            events,
        );
        narrow_phase.compute_intersections(bodies, colliders, hooks, events);
    }

    fn clear_modified_colliders(
        &mut self,
        colliders: &mut ColliderSet,
        modified_colliders: &mut ModifiedColliders,
    ) {
        for handle in modified_colliders.iter() {
            if let Some(co) = colliders.get_mut_internal(*handle) {
                co.changes = ColliderChanges::empty();
            }
        }

        modified_colliders.clear();
    }

    /// Executes one step of the collision detection.
    pub fn step(
        &mut self,
        prediction_distance: Real,
        islands: &mut IslandManager,
        broad_phase: &mut BroadPhaseBvh,
        narrow_phase: &mut NarrowPhase,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
    ) {
        let modified_bodies = bodies.take_modified();
        let mut modified_colliders = colliders.take_modified();
        let mut removed_colliders = colliders.take_removed();

        super::user_changes::handle_user_changes_to_colliders(
            bodies,
            colliders,
            &modified_colliders[..],
        );
        super::user_changes::handle_user_changes_to_rigid_bodies(
            None,
            bodies,
            colliders,
            &mut ImpulseJointSet::new(),
            &mut MultibodyJointSet::new(),
            &modified_bodies,
            &mut modified_colliders,
        );

        // Disabled colliders are treated as if they were removed.
        removed_colliders.extend(
            modified_colliders
                .iter()
                .copied()
                .filter(|h| colliders.get(*h).map(|c| !c.is_enabled()).unwrap_or(false)),
        );

        self.detect_collisions(
            prediction_distance,
            islands,
            broad_phase,
            narrow_phase,
            bodies,
            colliders,
            &modified_colliders[..],
            &removed_colliders,
            hooks,
            events,
            true,
        );

        self.clear_modified_colliders(colliders, &mut modified_colliders);
        removed_colliders.clear();
    }
}

#[cfg(test)]
mod tests {

    #[test]
    #[cfg(feature = "dim3")]
    pub fn test_no_rigid_bodies() {
        use crate::prelude::*;
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        /* Create the ground. */
        let collider_a = ColliderBuilder::cuboid(1.0, 1.0, 1.0)
            .active_collision_types(ActiveCollisionTypes::all())
            .sensor(true)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        let a_handle = collider_set.insert(collider_a);

        let collider_b = ColliderBuilder::cuboid(1.0, 1.0, 1.0)
            .active_collision_types(ActiveCollisionTypes::all())
            .sensor(true)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        let _ = collider_set.insert(collider_b);

        let integration_parameters = IntegrationParameters::default();
        let mut islands = IslandManager::new();
        let mut broad_phase = BroadPhaseBvh::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut collision_pipeline = CollisionPipeline::new();
        let physics_hooks = ();

        collision_pipeline.step(
            integration_parameters.prediction_distance(),
            &mut islands,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &physics_hooks,
            &(),
        );

        let mut hit = false;

        for (_, _, intersecting) in narrow_phase.intersection_pairs_with(a_handle) {
            if intersecting {
                hit = true;
            }
        }

        assert!(hit, "No hit found");
    }

    #[test]
    #[cfg(feature = "dim2")]
    pub fn test_no_rigid_bodies() {
        use crate::prelude::*;
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        /* Create the ground. */
        let collider_a = ColliderBuilder::cuboid(1.0, 1.0)
            .active_collision_types(ActiveCollisionTypes::all())
            .sensor(true)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        let a_handle = collider_set.insert(collider_a);

        let collider_b = ColliderBuilder::cuboid(1.0, 1.0)
            .active_collision_types(ActiveCollisionTypes::all())
            .sensor(true)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();

        let _ = collider_set.insert(collider_b);

        let integration_parameters = IntegrationParameters::default();
        let mut islands = IslandManager::new();
        let mut broad_phase = BroadPhaseBvh::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut collision_pipeline = CollisionPipeline::new();
        let physics_hooks = ();

        collision_pipeline.step(
            integration_parameters.prediction_distance(),
            &mut islands,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &physics_hooks,
            &(),
        );

        let mut hit = false;

        for (_, _, intersecting) in narrow_phase.intersection_pairs_with(a_handle) {
            if intersecting {
                hit = true;
            }
        }

        assert!(hit, "No hit found");
    }
}
