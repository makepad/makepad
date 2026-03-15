use crate::geometry::{ColliderHandle, ContactManifold, Shape, ShapeCastHit};
use crate::math::{Pose, Real, Vector};
use crate::pipeline::{QueryFilterFlags, QueryPipeline, QueryPipelineMut};
use crate::utils;
use na::{RealField, Vector2};
use parry::bounding_volume::BoundingVolume;
use parry::query::details::ShapeCastOptions;
use parry::query::{DefaultQueryDispatcher, PersistentQueryDispatcher};

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
/// A length measure used for various options of a character controller.
pub enum CharacterLength {
    /// The length is specified relative to some of the character shape’s size.
    ///
    /// For example setting `CharacterAutostep::max_height` to `CharacterLength::Relative(0.1)`
    /// for a shape with a height equal to 20.0 will result in a maximum step height
    /// of `0.1 * 20.0 = 2.0`.
    Relative(Real),
    /// The length is specified as an absolute value, independent from the character shape’s size.
    ///
    /// For example setting `CharacterAutostep::max_height` to `CharacterLength::Relative(0.1)`
    /// for a shape with a height equal to 20.0 will result in a maximum step height
    /// of `0.1` (the shape height is ignored in for this value).
    Absolute(Real),
}

impl CharacterLength {
    /// Returns `self` with its value changed by the closure `f` if `self` is the `Self::Absolute`
    /// variant.
    pub fn map_absolute(self, f: impl FnOnce(Real) -> Real) -> Self {
        if let Self::Absolute(value) = self {
            Self::Absolute(f(value))
        } else {
            self
        }
    }

    /// Returns `self` with its value changed by the closure `f` if `self` is the `Self::Relative`
    /// variant.
    pub fn map_relative(self, f: impl FnOnce(Real) -> Real) -> Self {
        if let Self::Relative(value) = self {
            Self::Relative(f(value))
        } else {
            self
        }
    }

    fn eval(self, value: Real) -> Real {
        match self {
            Self::Relative(x) => value * x,
            Self::Absolute(x) => x,
        }
    }
}

#[derive(Debug)]
struct HitInfo {
    toi: ShapeCastHit,
    is_wall: bool,
    is_nonslip_slope: bool,
}

/// Configuration for the auto-stepping character controller feature.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CharacterAutostep {
    /// The maximum step height a character can automatically step over.
    pub max_height: CharacterLength,
    /// The minimum width of free space that must be available after stepping on a stair.
    pub min_width: CharacterLength,
    /// Can the character automatically step over dynamic bodies too?
    pub include_dynamic_bodies: bool,
}

impl Default for CharacterAutostep {
    fn default() -> Self {
        Self {
            max_height: CharacterLength::Relative(0.25),
            min_width: CharacterLength::Relative(0.5),
            include_dynamic_bodies: true,
        }
    }
}

#[derive(Debug)]
struct HitDecomposition {
    normal_part: Vector,
    horizontal_tangent: Vector,
    vertical_tangent: Vector,
    // NOTE: we don’t store the penetration part since we don’t really need it
    //       for anything.
}

impl HitDecomposition {
    pub fn unconstrained_slide_part(&self) -> Vector {
        self.normal_part + self.horizontal_tangent + self.vertical_tangent
    }
}

/// A collision between the character and its environment during its movement.
#[derive(Copy, Clone, Debug)]
pub struct CharacterCollision {
    /// The collider hit by the character.
    pub handle: ColliderHandle,
    /// The position of the character when the collider was hit.
    pub character_pos: Pose,
    /// The translation that was already applied to the character when the hit happens.
    pub translation_applied: Vector,
    /// The translations that was still waiting to be applied to the character when the hit happens.
    pub translation_remaining: Vector,
    /// Geometric information about the hit.
    pub hit: ShapeCastHit,
}

/// A kinematic character controller for player/NPC movement (walking, climbing, sliding).
///
/// This provides classic game character movement: walking on floors, sliding on slopes,
/// climbing stairs, and snapping to ground. It's kinematic (not physics-based), meaning
/// you control movement directly rather than applying forces.
///
/// **Not suitable for:** Ragdolls, vehicles, or physics-driven movement (use dynamic bodies instead).
///
/// # How it works
///
/// 1. You provide desired movement (e.g., "move forward 5 units")
/// 2. Controller casts the character shape through the world
/// 3. It handles collisions: sliding along walls, stepping up stairs, snapping to ground
/// 4. Returns the final movement to apply
///
/// # Example
///
/// ```
/// # use rapier3d::prelude::*;
/// # use rapier3d::control::{CharacterAutostep, KinematicCharacterController};
/// # let mut bodies = RigidBodySet::new();
/// # let mut colliders = ColliderSet::new();
/// # let broad_phase = BroadPhaseBvh::new();
/// # let narrow_phase = NarrowPhase::new();
/// # let dt = 1.0 / 60.0;
/// # let speed = 5.0;
/// # let (input_x, input_z) = (1.0, 0.0);
/// # let character_shape = Ball::new(0.5);
/// # let mut character_pos = Pose::IDENTITY;
/// # let query_pipeline = broad_phase.as_query_pipeline(
/// #     narrow_phase.query_dispatcher(),
/// #     &bodies,
/// #     &colliders,
/// #     QueryFilter::default(),
/// # );
/// let controller = KinematicCharacterController {
///     slide: true,  // Slide along walls instead of stopping
///     autostep: Some(CharacterAutostep::default()),  // Auto-climb stairs
///     max_slope_climb_angle: 45.0_f32.to_radians(),  // Max climbable slope
///     ..Default::default()
/// };
///
/// // In your game loop:
/// let desired_movement = Vector::new(input_x, 0.0, input_z) * speed * dt;
/// let movement = controller.move_shape(
///     dt,
///     &query_pipeline,
///     &character_shape,
///     &character_pos,
///     desired_movement,
///     |_| {}  // Collision event callback
/// );
/// character_pos.translation += movement.translation;
/// ```
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug)]
pub struct KinematicCharacterController {
    /// The direction that goes "up". Used to determine where the floor is, and the floor’s angle.
    pub up: Vector,
    /// A small gap to preserve between the character and its surroundings.
    ///
    /// This value should not be too large to avoid visual artifacts, but shouldn’t be too small
    /// (must not be zero) to improve numerical stability of the character controller.
    pub offset: CharacterLength,
    /// Should the character try to slide against the floor if it hits it?
    pub slide: bool,
    /// Should the character automatically step over small obstacles? (disabled by default)
    ///
    /// Note that autostepping is currently a very computationally expensive feature, so it
    /// is disabled by default.
    pub autostep: Option<CharacterAutostep>,
    /// The maximum angle (radians) between the floor’s normal and the `up` vector that the
    /// character is able to climb.
    pub max_slope_climb_angle: Real,
    /// The minimum angle (radians) between the floor’s normal and the `up` vector before the
    /// character starts to slide down automatically.
    pub min_slope_slide_angle: Real,
    /// Should the character be automatically snapped to the ground if the distance between
    /// the ground and its feed are smaller than the specified threshold?
    pub snap_to_ground: Option<CharacterLength>,
    /// Increase this number if your character appears to get stuck when sliding against surfaces.
    ///
    /// This is a small distance applied to the movement toward the contact normals of shapes hit
    /// by the character controller. This helps shape-casting not getting stuck in an always-penetrating
    /// state during the sliding calculation.
    ///
    /// This value should remain fairly small since it can introduce artificial "bumps" when sliding
    /// along a flat surface.
    pub normal_nudge_factor: Real,
}

impl Default for KinematicCharacterController {
    fn default() -> Self {
        Self {
            up: Vector::Y,
            offset: CharacterLength::Relative(0.01),
            slide: true,
            autostep: None,
            max_slope_climb_angle: Real::frac_pi_4(),
            min_slope_slide_angle: Real::frac_pi_4(),
            snap_to_ground: Some(CharacterLength::Relative(0.2)),
            normal_nudge_factor: 1.0e-4,
        }
    }
}

/// The effective movement computed by the character controller.
#[derive(Debug)]
pub struct EffectiveCharacterMovement {
    /// The movement to apply.
    pub translation: Vector,
    /// Is the character touching the ground after applying `EffectiveKineamticMovement::translation`?
    pub grounded: bool,
    /// Is the character sliding down a slope due to slope angle being larger than `min_slope_slide_angle`?
    pub is_sliding_down_slope: bool,
}

impl KinematicCharacterController {
    fn check_and_fix_penetrations(&self) {
        /*
        // 1/ Check if the body is grounded and if there are penetrations.
        let mut grounded = false;
        let mut penetrating = false;

        let mut contacts = vec![];

        let aabb = shape
            .compute_aabb(shape_pos)
            .loosened(self.offset);
        queries.colliders_with_aabb_intersecting_aabb(&aabb, |handle| {
            // TODO: apply the filter.
            if let Some(collider) = colliders.get(*handle) {
                if let Ok(Some(contact)) = parry::query::contact(
                    &shape_pos,
                    shape,
                    collider.position(),
                    collider.shape(),
                    self.offset,
                ) {
                    contacts.push((contact, collider));
                }
            }

            true
        });
         */
    }

    /// Computes the possible movement for a shape.
    pub fn move_shape(
        &self,
        dt: Real,
        queries: &QueryPipeline,
        character_shape: &dyn Shape,
        character_pos: &Pose,
        desired_translation: Vector,
        mut events: impl FnMut(CharacterCollision),
    ) -> EffectiveCharacterMovement {
        let mut result = EffectiveCharacterMovement {
            translation: Vector::ZERO,
            grounded: false,
            is_sliding_down_slope: false,
        };
        let dims = self.compute_dims(character_shape);

        // 1. Check and fix penetrations.
        self.check_and_fix_penetrations();

        let mut translation_remaining = desired_translation;

        let grounded_at_starting_pos = self.detect_grounded_status_and_apply_friction(
            dt,
            queries,
            character_shape,
            character_pos,
            dims,
            None,
            None,
        );

        let mut max_iters = 20;
        let mut kinematic_friction_translation = Vector::ZERO;
        let offset = self.offset.eval(dims.y);
        let mut is_moving = false;

        while let Some((translation_dir, translation_dist)) =
            utils::try_normalize_and_get_length(translation_remaining, 1.0e-5)
        {
            if max_iters == 0 {
                break;
            } else {
                max_iters -= 1;
            }
            is_moving = true;

            // 2. Cast towards the movement direction.
            if let Some((handle, hit)) = queries.cast_shape(
                &(Pose::from_translation(result.translation) * *character_pos),
                translation_dir,
                character_shape,
                ShapeCastOptions {
                    target_distance: offset,
                    stop_at_penetration: false,
                    max_time_of_impact: translation_dist,
                    compute_impact_geometry_on_penetration: true,
                },
            ) {
                // We hit something, compute and apply the allowed interference-free translation.
                let allowed_dist = hit.time_of_impact;
                let allowed_translation = translation_dir * allowed_dist;
                result.translation += allowed_translation;
                translation_remaining -= allowed_translation;

                events(CharacterCollision {
                    handle,
                    character_pos: Pose::from_translation(result.translation) * *character_pos,
                    translation_applied: result.translation,
                    translation_remaining,
                    hit,
                });

                let hit_info = self.compute_hit_info(hit);

                // Try to go upstairs.
                if !self.handle_stairs(
                    *queries,
                    character_shape,
                    &(Pose::from_translation(result.translation) * *character_pos),
                    dims,
                    handle,
                    &hit_info,
                    &mut translation_remaining,
                    &mut result,
                ) {
                    // No stairs, try to move along slopes.
                    translation_remaining = self.handle_slopes(
                        &hit_info,
                        desired_translation,
                        translation_remaining,
                        self.normal_nudge_factor,
                        &mut result,
                    );
                }
            } else {
                // No interference along the path.
                result.translation += translation_remaining;
                result.grounded = self.detect_grounded_status_and_apply_friction(
                    dt,
                    queries,
                    character_shape,
                    &(Pose::from_translation(result.translation) * *character_pos),
                    dims,
                    None,
                    None,
                );
                break;
            }
            result.grounded = self.detect_grounded_status_and_apply_friction(
                dt,
                queries,
                character_shape,
                &(Pose::from_translation(result.translation) * *character_pos),
                dims,
                Some(&mut kinematic_friction_translation),
                Some(&mut translation_remaining),
            );

            if !self.slide {
                break;
            }
        }
        // When not moving, `detect_grounded_status_and_apply_friction` is not reached
        // so we call it explicitly here.
        if !is_moving {
            result.grounded = self.detect_grounded_status_and_apply_friction(
                dt,
                queries,
                character_shape,
                &(Pose::from_translation(result.translation) * *character_pos),
                dims,
                None,
                None,
            );
        }
        // If needed, and if we are not already grounded, snap to the ground.
        if grounded_at_starting_pos {
            self.snap_to_ground(
                queries,
                character_shape,
                &(Pose::from_translation(result.translation) * *character_pos),
                dims,
                &mut result,
            );
        }

        // Return the result.
        result
    }

    fn snap_to_ground(
        &self,
        queries: &QueryPipeline,
        character_shape: &dyn Shape,
        character_pos: &Pose,
        dims: Vector2<Real>,
        result: &mut EffectiveCharacterMovement,
    ) -> Option<(ColliderHandle, ShapeCastHit)> {
        if let Some(snap_distance) = self.snap_to_ground {
            if result.translation.dot(self.up) < -1.0e-5 {
                let snap_distance = snap_distance.eval(dims.y);
                let offset = self.offset.eval(dims.y);
                if let Some((hit_handle, hit)) = queries.cast_shape(
                    character_pos,
                    -self.up,
                    character_shape,
                    ShapeCastOptions {
                        target_distance: offset,
                        stop_at_penetration: false,
                        max_time_of_impact: snap_distance,
                        compute_impact_geometry_on_penetration: true,
                    },
                ) {
                    // Apply the snap.
                    result.translation -= self.up * hit.time_of_impact;
                    result.grounded = true;
                    return Some((hit_handle, hit));
                }
            }
        }

        None
    }

    fn predict_ground(&self, up_extends: Real) -> Real {
        self.offset.eval(up_extends) + 0.05
    }
    fn detect_grounded_status_and_apply_friction(
        &self,
        dt: Real,
        queries: &QueryPipeline,
        character_shape: &dyn Shape,
        character_pos: &Pose,
        dims: Vector2<Real>,
        mut kinematic_friction_translation: Option<&mut Vector>,
        mut translation_remaining: Option<&mut Vector>,
    ) -> bool {
        let prediction = self.predict_ground(dims.y);

        // TODO: allow custom dispatchers.
        let dispatcher = DefaultQueryDispatcher;

        let mut manifolds: Vec<ContactManifold> = vec![];
        let character_aabb = character_shape
            .compute_aabb(character_pos)
            .loosened(prediction);

        let mut grounded = false;

        'outer: for (_, collider) in queries.intersect_aabb_conservative(character_aabb) {
            manifolds.clear();
            let pos12 = character_pos.inv_mul(collider.position());
            let _ = dispatcher.contact_manifolds(
                &pos12,
                character_shape,
                collider.shape(),
                prediction,
                &mut manifolds,
                &mut None,
            );

            if let (Some(kinematic_friction_translation), Some(translation_remaining)) = (
                kinematic_friction_translation.as_deref_mut(),
                translation_remaining.as_deref_mut(),
            ) {
                let init_kinematic_friction_translation = *kinematic_friction_translation;
                let kinematic_parent = collider
                    .parent
                    .and_then(|p| queries.bodies.get(p.handle))
                    .filter(|rb| rb.is_kinematic());

                for m in &manifolds {
                    if self.is_grounded_at_contact_manifold(m, character_pos, dims) {
                        grounded = true;
                    }

                    if let Some(kinematic_parent) = kinematic_parent {
                        let mut num_active_contacts = 0;
                        let mut manifold_center = Vector::ZERO;
                        let normal = -(character_pos.rotation * m.local_n1);

                        for contact in &m.points {
                            if contact.dist <= prediction {
                                num_active_contacts += 1;
                                let contact_point = collider.position() * contact.local_p2;
                                let target_vel = kinematic_parent.velocity_at_point(contact_point);

                                let normal_target_mvt = target_vel.dot(normal) * dt;
                                let normal_current_mvt = translation_remaining.dot(normal);

                                manifold_center += contact_point;
                                *translation_remaining +=
                                    normal * (normal_target_mvt - normal_current_mvt);
                            }
                        }

                        if num_active_contacts > 0 {
                            let target_vel = kinematic_parent
                                .velocity_at_point(manifold_center / num_active_contacts as Real);
                            let tangent_platform_mvt =
                                (target_vel - normal * target_vel.dot(normal)) * dt;
                            // Apply larger-absolute-value-wins component-wise
                            if tangent_platform_mvt.x.abs() > kinematic_friction_translation.x.abs()
                            {
                                kinematic_friction_translation.x = tangent_platform_mvt.x;
                            }
                            if tangent_platform_mvt.y.abs() > kinematic_friction_translation.y.abs()
                            {
                                kinematic_friction_translation.y = tangent_platform_mvt.y;
                            }
                            #[cfg(feature = "dim3")]
                            if tangent_platform_mvt.z.abs() > kinematic_friction_translation.z.abs()
                            {
                                kinematic_friction_translation.z = tangent_platform_mvt.z;
                            }
                        }
                    }
                }

                *translation_remaining +=
                    *kinematic_friction_translation - init_kinematic_friction_translation;
            } else {
                for m in &manifolds {
                    if self.is_grounded_at_contact_manifold(m, character_pos, dims) {
                        grounded = true;
                        break 'outer; // We can stop the search early.
                    }
                }
            }
        }
        grounded
    }

    fn is_grounded_at_contact_manifold(
        &self,
        manifold: &ContactManifold,
        character_pos: &Pose,
        dims: Vector2<Real>,
    ) -> bool {
        let normal = -(character_pos.rotation * manifold.local_n1);

        // For the controller to be grounded, the angle between the contact normal and the up vector
        // has to be smaller than acos(1.0e-3) = 89.94 degrees.
        if normal.dot(self.up) >= 1.0e-3 {
            let prediction = self.predict_ground(dims.y);
            for contact in &manifold.points {
                if contact.dist <= prediction {
                    return true;
                }
            }
        }
        false
    }

    fn handle_slopes(
        &self,
        hit: &HitInfo,
        movement_input: Vector,
        translation_remaining: Vector,
        normal_nudge_factor: Real,
        result: &mut EffectiveCharacterMovement,
    ) -> Vector {
        let [_vertical_input, horizontal_input] = self.split_into_components(movement_input);
        let horiz_input_decomp = self.decompose_hit(horizontal_input, &hit.toi);
        let decomp = self.decompose_hit(translation_remaining, &hit.toi);

        // An object is trying to slip if the tangential movement induced by its vertical movement
        // points downward.
        let slipping_intent = self.up.dot(horiz_input_decomp.vertical_tangent) < 0.0;
        // An object is slipping if its vertical movement points downward.
        let slipping = self.up.dot(decomp.vertical_tangent) < 0.0;

        // An object is trying to climb if its vertical input motion points upward.
        let climbing_intent = self.up.dot(_vertical_input) > 0.0;
        // An object is climbing if the tangential movement induced by its vertical movement points upward.
        let climbing = self.up.dot(decomp.vertical_tangent) > 0.0;

        let allowed_movement = if hit.is_wall && climbing && !climbing_intent {
            // Can’t climb the slope, remove the vertical tangent motion induced by the forward motion.
            decomp.horizontal_tangent + decomp.normal_part
        } else if hit.is_nonslip_slope && slipping && !slipping_intent {
            // Prevent the vertical movement from sliding down.
            decomp.horizontal_tangent + decomp.normal_part
        } else {
            // Let it slide (including climbing the slope).
            result.is_sliding_down_slope = true;
            decomp.unconstrained_slide_part()
        };

        allowed_movement + hit.toi.normal1 * normal_nudge_factor
    }

    fn split_into_components(&self, translation: Vector) -> [Vector; 2] {
        let vertical_translation = self.up * (self.up.dot(translation));
        let horizontal_translation = translation - vertical_translation;
        [vertical_translation, horizontal_translation]
    }

    fn compute_hit_info(&self, toi: ShapeCastHit) -> HitInfo {
        #[cfg(feature = "dim2")]
        let angle_with_floor = self.up.angle_to(toi.normal1);
        #[cfg(feature = "dim3")]
        let angle_with_floor = self.up.angle_between(toi.normal1);
        let is_ceiling = self.up.dot(toi.normal1) < 0.0;
        let is_wall = angle_with_floor >= self.max_slope_climb_angle && !is_ceiling;
        let is_nonslip_slope = angle_with_floor <= self.min_slope_slide_angle;

        HitInfo {
            toi,
            is_wall,
            is_nonslip_slope,
        }
    }

    fn decompose_hit(&self, translation: Vector, hit: &ShapeCastHit) -> HitDecomposition {
        let dist_to_surface = translation.dot(hit.normal1);
        let normal_part;
        let penetration_part;

        if dist_to_surface < 0.0 {
            normal_part = Vector::ZERO;
            penetration_part = dist_to_surface * hit.normal1;
        } else {
            penetration_part = Vector::ZERO;
            normal_part = dist_to_surface * hit.normal1;
        }

        let tangent = translation - normal_part - penetration_part;
        #[cfg(feature = "dim3")]
        let horizontal_tangent_dir = hit.normal1.cross(self.up);
        #[cfg(feature = "dim2")]
        let horizontal_tangent_dir = Vector::ZERO;

        let horizontal_tangent_dir = horizontal_tangent_dir.try_normalize().unwrap_or_default();
        let horizontal_tangent = tangent.dot(horizontal_tangent_dir) * horizontal_tangent_dir;
        let vertical_tangent = tangent - horizontal_tangent;

        HitDecomposition {
            normal_part,
            horizontal_tangent,
            vertical_tangent,
        }
    }

    fn compute_dims(&self, character_shape: &dyn Shape) -> Vector2<Real> {
        let extents = character_shape.compute_local_aabb().extents();
        let up_extent = extents.dot(self.up.abs());
        let side_extent = (extents - (self.up).abs() * up_extent).length();
        Vector2::new(side_extent, up_extent)
    }
    fn handle_stairs(
        &self,
        mut queries: QueryPipeline,
        character_shape: &dyn Shape,
        character_pos: &Pose,
        dims: Vector2<Real>,
        stair_handle: ColliderHandle,
        hit: &HitInfo,
        translation_remaining: &mut Vector,
        result: &mut EffectiveCharacterMovement,
    ) -> bool {
        let Some(autostep) = self.autostep else {
            return false;
        };

        // Only try to autostep on walls.
        if !hit.is_wall {
            return false;
        }

        let offset = self.offset.eval(dims.y);
        let min_width = autostep.min_width.eval(dims.x) + offset;
        let max_height = autostep.max_height.eval(dims.y) + offset;

        if !autostep.include_dynamic_bodies {
            if queries
                .colliders
                .get(stair_handle)
                .and_then(|co| co.parent)
                .and_then(|p| queries.bodies.get(p.handle))
                .map(|b| b.is_dynamic())
                == Some(true)
            {
                // The "stair" is a dynamic body, which the user wants to ignore.
                return false;
            }

            queries.filter.flags |= QueryFilterFlags::EXCLUDE_DYNAMIC;
        }

        let shifted_character_pos = Pose::from_parts(
            character_pos.translation + self.up * max_height,
            character_pos.rotation,
        );

        let Some(horizontal_dir) =
            (*translation_remaining - self.up * translation_remaining.dot(self.up)).try_normalize()
        else {
            return false;
        };

        if queries
            .cast_shape(
                character_pos,
                self.up,
                character_shape,
                ShapeCastOptions {
                    target_distance: offset,
                    stop_at_penetration: false,
                    max_time_of_impact: max_height,
                    compute_impact_geometry_on_penetration: true,
                },
            )
            .is_some()
        {
            // We can’t go up.
            return false;
        }

        if queries
            .cast_shape(
                &shifted_character_pos,
                horizontal_dir,
                character_shape,
                ShapeCastOptions {
                    target_distance: offset,
                    stop_at_penetration: false,
                    max_time_of_impact: min_width,
                    compute_impact_geometry_on_penetration: true,
                },
            )
            .is_some()
        {
            // We don’t have enough room on the stair to stay on it.
            return false;
        }

        // Check that we are not getting into a ramp that is too steep
        // after stepping.
        if let Some((_, hit)) = queries.cast_shape(
            &Pose::from_parts(
                shifted_character_pos.translation + horizontal_dir * min_width,
                shifted_character_pos.rotation,
            ),
            -self.up,
            character_shape,
            ShapeCastOptions {
                target_distance: offset,
                stop_at_penetration: false,
                max_time_of_impact: max_height,
                compute_impact_geometry_on_penetration: true,
            },
        ) {
            let [vertical_slope_translation, horizontal_slope_translation] = self
                .split_into_components(*translation_remaining)
                .map(|remaining| subtract_hit(remaining, &hit));

            let slope_translation = horizontal_slope_translation + vertical_slope_translation;

            #[cfg(feature = "dim2")]
            let angle_with_floor = self.up.angle_to(hit.normal1);
            #[cfg(feature = "dim3")]
            let angle_with_floor = self.up.angle_between(hit.normal1);
            let climbing = self.up.dot(slope_translation) >= 0.0;

            if climbing && angle_with_floor > self.max_slope_climb_angle {
                return false; // The target ramp is too steep.
            }
        }

        // We can step, we need to find the actual step height.
        let step_height = max_height
            - queries
                .cast_shape(
                    &Pose::from_parts(
                        shifted_character_pos.translation + horizontal_dir * min_width,
                        shifted_character_pos.rotation,
                    ),
                    -self.up,
                    character_shape,
                    ShapeCastOptions {
                        target_distance: offset,
                        stop_at_penetration: false,
                        max_time_of_impact: max_height,
                        compute_impact_geometry_on_penetration: true,
                    },
                )
                .map(|hit| hit.1.time_of_impact)
                .unwrap_or(max_height);

        // Remove the step height from the vertical part of the self.
        let step = self.up * step_height;
        *translation_remaining -= step;

        // Advance the collider on the step horizontally, to make sure further
        // movement won’t just get stuck on its edge.
        let horizontal_nudge =
            horizontal_dir * horizontal_dir.dot(*translation_remaining).min(min_width);
        *translation_remaining -= horizontal_nudge;

        result.translation += step + horizontal_nudge;
        true
    }

    /// For the given collisions between a character and its environment, this method will apply
    /// impulses to the rigid-bodies surrounding the character shape at the time of the collisions.
    /// Note that the impulse calculation is only approximate as it is not based on a global
    /// constraints resolution scheme.
    pub fn solve_character_collision_impulses<'a>(
        &self,
        dt: Real,
        queries: &mut QueryPipelineMut,
        character_shape: &dyn Shape,
        character_mass: Real,
        collisions: impl IntoIterator<Item = &'a CharacterCollision>,
    ) {
        for collision in collisions {
            self.solve_single_character_collision_impulse(
                dt,
                queries,
                character_shape,
                character_mass,
                collision,
            );
        }
    }

    /// For the given collision between a character and its environment, this method will apply
    /// impulses to the rigid-bodies surrounding the character shape at the time of the collision.
    /// Note that the impulse calculation is only approximate as it is not based on a global
    /// constraints resolution scheme.
    fn solve_single_character_collision_impulse(
        &self,
        dt: Real,
        queries: &mut QueryPipelineMut,
        character_shape: &dyn Shape,
        character_mass: Real,
        collision: &CharacterCollision,
    ) {
        let extents = character_shape.compute_local_aabb().extents();
        let up_extent = extents.dot(self.up.abs());
        let movement_to_transfer =
            collision.hit.normal1 * collision.translation_remaining.dot(collision.hit.normal1);
        let prediction = self.predict_ground(up_extent);

        // TODO: allow custom dispatchers.
        let dispatcher = DefaultQueryDispatcher;

        let mut manifolds: Vec<ContactManifold> = vec![];
        let character_aabb = character_shape
            .compute_aabb(&collision.character_pos)
            .loosened(prediction);

        for (_, collider) in queries.as_ref().intersect_aabb_conservative(character_aabb) {
            if let Some(parent) = collider.parent {
                if let Some(body) = queries.bodies.get(parent.handle) {
                    if body.is_dynamic() {
                        manifolds.clear();
                        let pos12 = collision.character_pos.inv_mul(collider.position());
                        let prev_manifolds_len = manifolds.len();
                        let _ = dispatcher.contact_manifolds(
                            &pos12,
                            character_shape,
                            collider.shape(),
                            prediction,
                            &mut manifolds,
                            &mut None,
                        );

                        for m in &mut manifolds[prev_manifolds_len..] {
                            m.data.rigid_body2 = Some(parent.handle);
                            m.data.normal = collision.character_pos.rotation * m.local_n1;
                        }
                    }
                }
            }
        }

        let velocity_to_transfer = movement_to_transfer * utils::inv(dt);

        for manifold in &manifolds {
            let body_handle = manifold.data.rigid_body2.unwrap();
            let body = &mut queries.bodies[body_handle];

            for pt in &manifold.points {
                if pt.dist <= prediction {
                    let body_mass = body.mass();
                    let contact_point = body.position() * pt.local_p2;
                    let delta_vel_per_contact = (velocity_to_transfer
                        - body.velocity_at_point(contact_point))
                    .dot(manifold.data.normal);
                    let mass_ratio = body_mass * character_mass / (body_mass + character_mass);

                    body.apply_impulse_at_point(
                        manifold.data.normal * delta_vel_per_contact.max(0.0) * mass_ratio,
                        contact_point,
                        true,
                    );
                }
            }
        }
    }
}

fn subtract_hit(translation: Vector, hit: &ShapeCastHit) -> Vector {
    let surface_correction = (-translation).dot(hit.normal1).max(0.0);
    // This fixes some instances of moving through walls
    let surface_correction = surface_correction * (1.0 + 1.0e-5);
    translation + hit.normal1 * surface_correction
}

#[cfg(all(feature = "dim3", feature = "f32"))]
#[cfg(test)]
mod test {
    use crate::{
        control::{CharacterLength, KinematicCharacterController},
        prelude::*,
    };

    #[test]
    fn character_controller_climb_test() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();

        let gravity = Vector::Y * -9.81;

        let ground_size = 100.0;
        let ground_height = 0.1;
        /*
         * Create a flat ground
         */
        let rigid_body =
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -ground_height, 0.0));
        let floor_handle = bodies.insert(rigid_body);
        let collider = ColliderBuilder::cuboid(ground_size, ground_height, ground_size);
        colliders.insert_with_parent(collider, floor_handle, &mut bodies);

        /*
         * Create a slope we can climb.
         */
        let slope_angle = 0.2;
        let slope_size = 2.0;
        let collider = ColliderBuilder::cuboid(slope_size, ground_height, slope_size)
            .translation(Vector::new(0.1 + slope_size, -ground_height + 0.4, 0.0))
            .rotation(Vector::Z * slope_angle);
        colliders.insert(collider);

        /*
         * Create a slope we can't climb.
         */
        let impossible_slope_angle = 0.6;
        let impossible_slope_size = 2.0;
        let collider = ColliderBuilder::cuboid(slope_size, ground_height, ground_size)
            .translation(Vector::new(
                0.1 + slope_size * 2.0 + impossible_slope_size - 0.9,
                -ground_height + 1.7,
                0.0,
            ))
            .rotation(Vector::Z * impossible_slope_angle);
        colliders.insert(collider);

        let integration_parameters = IntegrationParameters::default();

        // Initialize character which can climb
        let mut character_body_can_climb = RigidBodyBuilder::kinematic_position_based()
            .additional_mass(1.0)
            .build();
        character_body_can_climb.set_translation(Vector::new(0.6, 0.5, 0.0), false);
        let character_handle_can_climb = bodies.insert(character_body_can_climb);

        let collider = ColliderBuilder::ball(0.5).build();
        colliders.insert_with_parent(collider.clone(), character_handle_can_climb, &mut bodies);

        // Initialize character which cannot climb
        let mut character_body_cannot_climb = RigidBodyBuilder::kinematic_position_based()
            .additional_mass(1.0)
            .build();
        character_body_cannot_climb.set_translation(Vector::new(-0.6, 0.5, 0.0), false);
        let character_handle_cannot_climb = bodies.insert(character_body_cannot_climb);

        let collider = ColliderBuilder::ball(0.5).build();
        let character_shape = collider.shape();
        colliders.insert_with_parent(collider.clone(), character_handle_cannot_climb, &mut bodies);

        for i in 0..200 {
            // Step once
            pipeline.step(
                gravity,
                &integration_parameters,
                &mut islands,
                &mut bf,
                &mut nf,
                &mut bodies,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                &mut CCDSolver::new(),
                &(),
                &(),
            );

            let mut update_character_controller =
                |controller: KinematicCharacterController, handle: RigidBodyHandle| {
                    let character_body = bodies.get(handle).unwrap();
                    // Use a closure to handle or collect the collisions while
                    // the character is being moved.
                    let mut collisions = vec![];
                    let filter_character_controller = QueryFilter::new().exclude_rigid_body(handle);
                    let query_pipeline = bf.as_query_pipeline(
                        nf.query_dispatcher(),
                        &bodies,
                        &colliders,
                        filter_character_controller,
                    );
                    let effective_movement = controller.move_shape(
                        integration_parameters.dt,
                        &query_pipeline,
                        character_shape,
                        character_body.position(),
                        Vector::new(0.1, -0.1, 0.0),
                        |collision| collisions.push(collision),
                    );
                    let character_body = bodies.get_mut(handle).unwrap();
                    let translation = character_body.translation();
                    assert!(
                        effective_movement.grounded,
                        "movement should be grounded at all times for current setup (iter: {}), pos: {}.",
                        i,
                        translation + effective_movement.translation
                    );
                    character_body.set_next_kinematic_translation(
                        translation + effective_movement.translation,
                    );
                };

            let character_controller_cannot_climb = KinematicCharacterController {
                max_slope_climb_angle: impossible_slope_angle - 0.001,
                ..Default::default()
            };
            let character_controller_can_climb = KinematicCharacterController {
                max_slope_climb_angle: impossible_slope_angle + 0.001,
                ..Default::default()
            };
            update_character_controller(
                character_controller_cannot_climb,
                character_handle_cannot_climb,
            );
            update_character_controller(character_controller_can_climb, character_handle_can_climb);
        }
        let character_body = bodies.get(character_handle_can_climb).unwrap();
        assert!(character_body.translation().x > 6.0);
        assert!(character_body.translation().y > 3.0);
        let character_body = bodies.get(character_handle_cannot_climb).unwrap();
        assert!(character_body.translation().x < 4.0);
        assert!(dbg!(character_body.translation().y) < 2.0);
    }

    #[test]
    fn character_controller_ground_detection() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();

        let gravity = Vector::Y * -9.81;

        let ground_size = 1001.0;
        let ground_height = 1.0;
        /*
         * Create a flat ground
         */
        let rigid_body =
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -ground_height / 2.0, 0.0));
        let floor_handle = bodies.insert(rigid_body);
        let collider = ColliderBuilder::cuboid(ground_size, ground_height, ground_size);
        colliders.insert_with_parent(collider, floor_handle, &mut bodies);

        let integration_parameters = IntegrationParameters::default();

        // Initialize character with snap to ground
        let character_controller_snap = KinematicCharacterController {
            snap_to_ground: Some(CharacterLength::Relative(0.2)),
            ..Default::default()
        };
        let mut character_body_snap = RigidBodyBuilder::kinematic_position_based()
            .additional_mass(1.0)
            .build();
        character_body_snap.set_translation(Vector::new(0.6, 0.5, 0.0), false);
        let character_handle_snap = bodies.insert(character_body_snap);

        let collider = ColliderBuilder::ball(0.5).build();
        colliders.insert_with_parent(collider.clone(), character_handle_snap, &mut bodies);

        // Initialize character without snap to ground
        let character_controller_no_snap = KinematicCharacterController {
            snap_to_ground: None,
            ..Default::default()
        };
        let mut character_body_no_snap = RigidBodyBuilder::kinematic_position_based()
            .additional_mass(1.0)
            .build();
        character_body_no_snap.set_translation(Vector::new(-0.6, 0.5, 0.0), false);
        let character_handle_no_snap = bodies.insert(character_body_no_snap);

        let collider = ColliderBuilder::ball(0.5).build();
        let character_shape = collider.shape();
        colliders.insert_with_parent(collider.clone(), character_handle_no_snap, &mut bodies);

        for i in 0..10000 {
            // Step once
            pipeline.step(
                gravity,
                &integration_parameters,
                &mut islands,
                &mut bf,
                &mut nf,
                &mut bodies,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                &mut CCDSolver::new(),
                &(),
                &(),
            );

            let mut update_character_controller =
                |controller: KinematicCharacterController, handle: RigidBodyHandle| {
                    let character_body = bodies.get(handle).unwrap();
                    // Use a closure to handle or collect the collisions while
                    // the character is being moved.
                    let mut collisions = vec![];
                    let filter_character_controller = QueryFilter::new().exclude_rigid_body(handle);
                    let query_pipeline = bf.as_query_pipeline(
                        nf.query_dispatcher(),
                        &bodies,
                        &colliders,
                        filter_character_controller,
                    );
                    let effective_movement = controller.move_shape(
                        integration_parameters.dt,
                        &query_pipeline,
                        character_shape,
                        character_body.position(),
                        Vector::new(0.1, -0.1, 0.1),
                        |collision| collisions.push(collision),
                    );
                    let character_body = bodies.get_mut(handle).unwrap();
                    let translation = character_body.translation();
                    assert!(
                        effective_movement.grounded,
                        "movement should be grounded at all times for current setup (iter: {}), pos: {}.",
                        i,
                        translation + effective_movement.translation
                    );
                    character_body.set_next_kinematic_translation(
                        translation + effective_movement.translation,
                    );
                };

            update_character_controller(character_controller_no_snap, character_handle_no_snap);
            update_character_controller(character_controller_snap, character_handle_snap);
        }

        let character_body = bodies.get_mut(character_handle_no_snap).unwrap();
        let translation = character_body.translation();

        // accumulated numerical errors make the test go less far than it should,
        // but it's expected.
        assert!(
            translation.x >= 940.0,
            "actual translation.x:{}",
            translation.x
        );
        assert!(
            translation.z >= 940.0,
            "actual translation.z:{}",
            translation.z
        );

        let character_body = bodies.get_mut(character_handle_snap).unwrap();
        let translation = character_body.translation();
        assert!(
            translation.x >= 960.0,
            "actual translation.x:{}",
            translation.x
        );
        assert!(
            translation.z >= 960.0,
            "actual translation.z:{}",
            translation.z
        );
    }
}
