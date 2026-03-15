use super::{DebugColor, DebugRenderBackend, outlines};
use crate::dynamics::{
    GenericJoint, ImpulseJointSet, MultibodyJointSet, RigidBodySet, RigidBodyType,
};
use crate::geometry::{Ball, ColliderSet, Cuboid, NarrowPhase, Shape, TypedShape};
#[cfg(feature = "dim3")]
use crate::geometry::{Cone, Cylinder};
use crate::math::{DIM, Matrix, Pose, Vector};
use crate::pipeline::debug_render_pipeline::DebugRenderStyle;
use crate::pipeline::debug_render_pipeline::debug_render_backend::DebugRenderObject;
use crate::utils::OrthonormalBasis;
use parry::utils::PoseOpt;
use std::any::TypeId;
use std::collections::HashMap;

bitflags::bitflags! {
    /// Flags indicating what part of the physics engine should be rendered
    /// by the debug-renderer.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub struct DebugRenderMode: u32 {
        /// If this flag is set, the collider shapes will be rendered.
        const COLLIDER_SHAPES = 1 << 0;
        /// If this flag is set, the local coordinate axes of rigid-bodies will be rendered.
        const RIGID_BODY_AXES = 1 << 1;
        /// If this flag is set, the multibody joints will be rendered.
        const MULTIBODY_JOINTS = 1 << 2;
        /// If this flag is set, the impulse joints will be rendered.
        const IMPULSE_JOINTS = 1 << 3;
        /// If this flag is set, all the joints will be rendered.
        const JOINTS = Self::MULTIBODY_JOINTS.bits() | Self::IMPULSE_JOINTS.bits();
        /// If this flag is set, the solver contacts will be rendered.
        const SOLVER_CONTACTS = 1 << 4;
        /// If this flag is set, the geometric contacts will be rendered.
        const CONTACTS = 1 << 5;
        /// If this flag is set, the Aabbs of colliders will be rendered.
        const COLLIDER_AABBS = 1 << 6;
    }
}

impl Default for DebugRenderMode {
    fn default() -> Self {
        Self::COLLIDER_SHAPES | Self::JOINTS | Self::RIGID_BODY_AXES
    }
}

#[cfg(feature = "dim2")]
type InstancesMap = HashMap<TypeId, Vec<Vector>>;
#[cfg(feature = "dim3")]
type InstancesMap = HashMap<TypeId, (Vec<Vector>, Vec<[u32; 2]>)>;

/// Pipeline responsible for rendering the state of the physics engine for debugging purpose.
pub struct DebugRenderPipeline {
    #[cfg(feature = "dim2")]
    instances: InstancesMap,
    #[cfg(feature = "dim3")]
    instances: InstancesMap,
    /// The style used to compute the line colors for each element
    /// to render.
    pub style: DebugRenderStyle,
    /// Flags controlling what part of the physics engine need to
    /// be rendered.
    pub mode: DebugRenderMode,
}

impl Default for DebugRenderPipeline {
    fn default() -> Self {
        Self::new(DebugRenderStyle::default(), DebugRenderMode::default())
    }
}

impl DebugRenderPipeline {
    /// Creates a new debug-render pipeline from a given style and flags.
    pub fn new(style: DebugRenderStyle, mode: DebugRenderMode) -> Self {
        Self {
            instances: outlines::instances(style.subdivisions),
            style,
            mode,
        }
    }

    /// Creates a new debug-render pipeline that renders everything
    /// it can from the physics state.
    pub fn render_all(style: DebugRenderStyle) -> Self {
        Self::new(style, DebugRenderMode::all())
    }

    /// Render the scene.
    pub fn render(
        &mut self,
        backend: &mut impl DebugRenderBackend,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        narrow_phase: &NarrowPhase,
    ) {
        self.render_rigid_bodies(backend, bodies);
        self.render_colliders(backend, bodies, colliders);
        self.render_joints(backend, bodies, impulse_joints, multibody_joints);
        self.render_contacts(backend, colliders, narrow_phase);
    }

    /// Render contact.
    pub fn render_contacts(
        &mut self,
        backend: &mut impl DebugRenderBackend,
        colliders: &ColliderSet,
        narrow_phase: &NarrowPhase,
    ) {
        if self.mode.contains(DebugRenderMode::CONTACTS) {
            for pair in narrow_phase.contact_pairs() {
                if let (Some(co1), Some(co2)) =
                    (colliders.get(pair.collider1), colliders.get(pair.collider2))
                {
                    let object = DebugRenderObject::ContactPair(pair, co1, co2);

                    if backend.filter_object(object) {
                        for manifold in &pair.manifolds {
                            for contact in manifold.contacts() {
                                let world_subshape_pos1 =
                                    manifold.subshape_pos1.prepend_to(co1.position());
                                backend.draw_line(
                                    object,
                                    world_subshape_pos1 * contact.local_p1,
                                    manifold.subshape_pos2.prepend_to(co2.position())
                                        * contact.local_p2,
                                    self.style.contact_depth_color,
                                );
                                backend.draw_line(
                                    object,
                                    world_subshape_pos1 * contact.local_p1,
                                    world_subshape_pos1
                                        * (contact.local_p1
                                            + manifold.local_n1 * self.style.contact_normal_length),
                                    self.style.contact_normal_color,
                                );
                            }
                        }
                    }
                }
            }
        }

        if self.mode.contains(DebugRenderMode::SOLVER_CONTACTS) {
            for pair in narrow_phase.contact_pairs() {
                if let (Some(co1), Some(co2)) =
                    (colliders.get(pair.collider1), colliders.get(pair.collider2))
                {
                    let object = DebugRenderObject::ContactPair(pair, co1, co2);

                    if backend.filter_object(object) {
                        for manifold in &pair.manifolds {
                            for contact in &manifold.data.solver_contacts {
                                let point = contact.point;
                                backend.draw_line(
                                    object,
                                    point,
                                    point + manifold.data.normal * self.style.contact_normal_length,
                                    self.style.contact_normal_color,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Render only the joints from the scene.
    pub fn render_joints(
        &mut self,
        backend: &mut impl DebugRenderBackend,
        bodies: &RigidBodySet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
    ) {
        let mut render_joint = |body1,
                                body2,
                                data: &GenericJoint,
                                mut anchor_color: DebugColor,
                                mut separation_color: DebugColor,
                                object| {
            if !backend.filter_object(object) {
                return;
            }

            if let (Some(rb1), Some(rb2)) = (bodies.get(body1), bodies.get(body2)) {
                let coeff = if !data.is_enabled() || !rb1.is_enabled() || !rb2.is_enabled() {
                    self.style.disabled_color_multiplier
                } else if (rb1.is_fixed() || rb1.is_sleeping())
                    && (rb2.is_fixed() || rb2.is_sleeping())
                {
                    self.style.sleep_color_multiplier
                } else {
                    [1.0; 4]
                };

                let frame1 = rb1.position() * data.local_frame1;
                let frame2 = rb2.position() * data.local_frame2;

                let a = rb1.translation();
                let b = frame1.translation;
                let c = frame2.translation;
                let d = rb2.translation();

                for k in 0..4 {
                    anchor_color[k] *= coeff[k];
                    separation_color[k] *= coeff[k];
                }

                backend.draw_line(object, a, b, anchor_color);
                backend.draw_line(object, b, c, separation_color);
                backend.draw_line(object, c, d, anchor_color);
            }
        };

        if self.mode.contains(DebugRenderMode::IMPULSE_JOINTS) {
            for (handle, joint) in impulse_joints.iter() {
                let anc_color = self.style.impulse_joint_anchor_color;
                let sep_color = self.style.impulse_joint_separation_color;
                let object = DebugRenderObject::ImpulseJoint(handle, joint);
                render_joint(
                    joint.body1,
                    joint.body2,
                    &joint.data,
                    anc_color,
                    sep_color,
                    object,
                );
            }
        }

        if self.mode.contains(DebugRenderMode::MULTIBODY_JOINTS) {
            for (handle, _, multibody, link) in multibody_joints.iter() {
                let anc_color = self.style.multibody_joint_anchor_color;
                let sep_color = self.style.multibody_joint_separation_color;
                let parent = multibody.link(link.parent_id().unwrap()).unwrap();
                let object = DebugRenderObject::MultibodyJoint(handle, multibody, link);
                render_joint(
                    parent.rigid_body_handle(),
                    link.rigid_body_handle(),
                    &link.joint.data,
                    anc_color,
                    sep_color,
                    object,
                );
            }
        }
    }

    /// Render only the rigid-bodies from the scene.
    pub fn render_rigid_bodies(
        &mut self,
        backend: &mut impl DebugRenderBackend,
        bodies: &RigidBodySet,
    ) {
        for (handle, rb) in bodies.iter() {
            let object = DebugRenderObject::RigidBody(handle, rb);

            if self.style.rigid_body_axes_length != 0.0
                && self.mode.contains(DebugRenderMode::RIGID_BODY_AXES)
                && backend.filter_object(object)
            {
                #[cfg(feature = "dim2")]
                let basis = Matrix::from_angle(rb.rotation().angle());
                #[cfg(feature = "dim3")]
                let basis = Matrix::from_quat(*rb.rotation());
                let coeff = if !rb.is_enabled() {
                    self.style.disabled_color_multiplier
                } else if rb.is_sleeping() {
                    self.style.sleep_color_multiplier
                } else {
                    [1.0; 4]
                };
                let colors = [
                    [0.0 * coeff[0], 1.0 * coeff[1], 0.25 * coeff[2], coeff[3]],
                    [120.0 * coeff[0], 1.0 * coeff[1], 0.1 * coeff[2], coeff[3]],
                    [240.0 * coeff[0], 1.0 * coeff[1], 0.2 * coeff[2], coeff[3]],
                ];

                let com = rb.position() * rb.mprops.local_mprops.local_com;

                for k in 0..DIM {
                    let axis = basis.col(k) * self.style.rigid_body_axes_length;
                    backend.draw_line(object, com, com + axis, colors[k]);
                }
            }
        }
    }

    /// Render only the colliders from the scene.
    pub fn render_colliders(
        &mut self,
        backend: &mut impl DebugRenderBackend,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
    ) {
        if self.mode.contains(DebugRenderMode::COLLIDER_SHAPES) {
            for (h, co) in colliders.iter() {
                let object = DebugRenderObject::Collider(h, co);

                if backend.filter_object(object) {
                    let color = if let Some(parent) = co.parent().and_then(|p| bodies.get(p)) {
                        let coeff = if !parent.is_enabled() || !co.is_enabled() {
                            self.style.disabled_color_multiplier
                        } else if parent.is_sleeping() {
                            self.style.sleep_color_multiplier
                        } else {
                            [1.0; 4]
                        };
                        let c = match parent.body_type {
                            RigidBodyType::Fixed => self.style.collider_fixed_color,
                            RigidBodyType::Dynamic => self.style.collider_dynamic_color,
                            RigidBodyType::KinematicPositionBased
                            | RigidBodyType::KinematicVelocityBased => {
                                self.style.collider_kinematic_color
                            }
                        };

                        [
                            c[0] * coeff[0],
                            c[1] * coeff[1],
                            c[2] * coeff[2],
                            c[3] * coeff[3],
                        ]
                    } else if !co.is_enabled() {
                        self.style.disabled_color_multiplier
                    } else {
                        self.style.collider_parentless_color
                    };

                    self.render_shape(object, backend, co.shape(), co.position(), color)
                }
            }
        }

        if self.mode.contains(DebugRenderMode::COLLIDER_AABBS) {
            for (h, co) in colliders.iter() {
                let aabb = co.compute_aabb();
                let cuboid = Cuboid::new(aabb.half_extents());
                let object = DebugRenderObject::ColliderAabb(h, co, &aabb);

                if backend.filter_object(object) {
                    self.render_shape(
                        object,
                        backend,
                        &cuboid,
                        &Pose::from_translation(aabb.center()),
                        self.style.collider_aabb_color,
                    );
                }
            }
        }
    }

    #[cfg(feature = "dim2")]
    fn render_shape(
        &mut self,
        object: DebugRenderObject,
        backend: &mut impl DebugRenderBackend,
        shape: &dyn Shape,
        pos: &Pose,
        color: DebugColor,
    ) {
        match shape.as_typed_shape() {
            TypedShape::Ball(s) => {
                let vtx = &self.instances[&TypeId::of::<Ball>()];
                backend.draw_line_strip(
                    object,
                    vtx,
                    pos,
                    Vector::splat(s.radius * 2.0),
                    color,
                    true,
                );
                // Draw a radius line to visualize rotation
                backend.draw_line(
                    object,
                    pos * Vector::new(s.radius * 0.2, 0.0),
                    pos * Vector::new(s.radius * 0.8, 0.0),
                    color,
                )
            }
            TypedShape::Cuboid(s) => {
                let vtx = &self.instances[&TypeId::of::<Cuboid>()];
                backend.draw_line_strip(object, vtx, pos, s.half_extents * 2.0, color, true)
            }
            TypedShape::Capsule(s) => {
                let vtx = s.to_polyline(self.style.subdivisions);
                backend.draw_line_strip(object, &vtx, pos, Vector::splat(1.0), color, true)
            }
            TypedShape::Segment(s) => {
                backend.draw_line_strip(object, &[s.a, s.b], pos, Vector::splat(1.0), color, false)
            }
            TypedShape::Triangle(s) => backend.draw_line_strip(
                object,
                &[s.a, s.b, s.c],
                pos,
                Vector::splat(1.0),
                color,
                true,
            ),
            TypedShape::TriMesh(s) => {
                for tri in s.triangles() {
                    self.render_shape(object, backend, &tri, pos, color)
                }
            }
            TypedShape::Polyline(s) => backend.draw_polyline(
                object,
                s.vertices(),
                s.indices(),
                pos,
                Vector::splat(1.0),
                color,
            ),
            TypedShape::HalfSpace(s) => {
                let basis = s.normal.orthonormal_basis()[0];
                let a = Vector::from(basis) * 10_000.0;
                let b = Vector::from(basis) * -10_000.0;
                backend.draw_line_strip(object, &[a, b], pos, Vector::splat(1.0), color, false)
            }
            TypedShape::HeightField(s) => {
                for seg in s.segments() {
                    self.render_shape(object, backend, &seg, pos, color)
                }
            }
            TypedShape::Compound(s) => {
                for (sub_pos, shape) in s.shapes() {
                    self.render_shape(object, backend, &**shape, &(pos * sub_pos), color)
                }
            }
            TypedShape::ConvexPolygon(s) => {
                backend.draw_line_strip(object, s.points(), pos, Vector::splat(1.0), color, true)
            }
            /*
             * Round shapes.
             */
            TypedShape::RoundCuboid(s) => {
                let vtx = s.to_polyline(self.style.border_subdivisions);
                backend.draw_line_strip(object, &vtx, pos, Vector::splat(1.0), color, true)
            }
            TypedShape::RoundTriangle(s) => {
                // TODO: take roundness into account.
                self.render_shape(object, backend, &s.inner_shape, pos, color)
            }
            // TypedShape::RoundTriMesh(s) => self.render_shape(backend, &s.inner_shape, pos, color),
            // TypedShape::RoundHeightField(s) => {
            //     self.render_shape(backend, &s.inner_shape, pos, color)
            // }
            TypedShape::RoundConvexPolygon(s) => {
                let vtx = s.to_polyline(self.style.border_subdivisions);
                backend.draw_line_strip(object, &vtx, pos, Vector::splat(1.0), color, true)
            }
            TypedShape::Voxels(s) => {
                let (vtx, idx) = s.to_polyline();
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::Custom(_) => {}
        }
    }

    #[cfg(feature = "dim3")]
    fn render_shape(
        &mut self,
        object: DebugRenderObject,
        backend: &mut impl DebugRenderBackend,
        shape: &dyn Shape,
        pos: &Pose,
        color: DebugColor,
    ) {
        match shape.as_typed_shape() {
            TypedShape::Ball(s) => {
                let (vtx, idx) = &self.instances[&TypeId::of::<Ball>()];
                backend.draw_polyline(object, vtx, idx, pos, Vector::splat(s.radius * 2.0), color)
            }
            TypedShape::Cuboid(s) => {
                let (vtx, idx) = &self.instances[&TypeId::of::<Cuboid>()];
                backend.draw_polyline(object, vtx, idx, pos, s.half_extents * 2.0, color)
            }
            TypedShape::Capsule(s) => {
                let (vtx, idx) = s.to_outline(self.style.subdivisions);
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::Segment(s) => backend.draw_polyline(
                object,
                &[s.a, s.b],
                &[[0, 1]],
                pos,
                Vector::splat(1.0),
                color,
            ),
            TypedShape::Triangle(s) => backend.draw_line_strip(
                object,
                &[s.a, s.b, s.c],
                pos,
                Vector::splat(1.0),
                color,
                true,
            ),
            TypedShape::TriMesh(s) => {
                for tri in s.triangles() {
                    self.render_shape(object, backend, &tri, pos, color)
                }
            }
            TypedShape::Polyline(s) => backend.draw_polyline(
                object,
                s.vertices(),
                s.indices(),
                pos,
                Vector::splat(1.0),
                color,
            ),
            TypedShape::HalfSpace(s) => {
                let basis = s.normal.orthonormal_basis();
                let a = Vector::from(basis[0]) * 10_000.0;
                let b = Vector::from(basis[0]) * -10_000.0;
                let c = Vector::from(basis[1]) * 10_000.0;
                let d = Vector::from(basis[1]) * -10_000.0;
                backend.draw_polyline(
                    object,
                    &[a, b, c, d],
                    &[[0, 1], [2, 3]],
                    pos,
                    Vector::splat(1.0),
                    color,
                )
            }
            TypedShape::HeightField(s) => {
                for tri in s.triangles() {
                    self.render_shape(object, backend, &tri, pos, color)
                }
            }
            TypedShape::Compound(s) => {
                for (sub_pos, shape) in s.shapes() {
                    self.render_shape(object, backend, &**shape, &(pos * sub_pos), color)
                }
            }
            TypedShape::ConvexPolyhedron(s) => {
                let indices: Vec<_> = s
                    .edges()
                    .iter()
                    .map(|e| [e.vertices[0], e.vertices[1]])
                    .collect();
                backend.draw_polyline(object, s.points(), &indices, pos, Vector::splat(1.0), color)
            }
            TypedShape::Cylinder(s) => {
                let (vtx, idx) = &self.instances[&TypeId::of::<Cylinder>()];
                backend.draw_polyline(
                    object,
                    vtx,
                    idx,
                    pos,
                    Vector::new(s.radius, s.half_height, s.radius) * 2.0,
                    color,
                )
            }
            TypedShape::Cone(s) => {
                let (vtx, idx) = &self.instances[&TypeId::of::<Cone>()];
                backend.draw_polyline(
                    object,
                    vtx,
                    idx,
                    pos,
                    Vector::new(s.radius, s.half_height, s.radius) * 2.0,
                    color,
                )
            }
            /*
             * Round shapes.
             */
            TypedShape::RoundCuboid(s) => {
                let (vtx, idx) = s.to_outline(self.style.border_subdivisions);
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::RoundTriangle(s) => {
                // TODO: take roundness into account.
                self.render_shape(object, backend, &s.inner_shape, pos, color)
            }
            // TypedShape::RoundTriMesh(s) => self.render_shape(object, backend, &s.inner_shape, pos, color),
            // TypedShape::RoundHeightField(s) => {
            //     self.render_shape(object, backend, &s.inner_shape, pos, color)
            // }
            TypedShape::RoundCylinder(s) => {
                let (vtx, idx) =
                    s.to_outline(self.style.subdivisions, self.style.border_subdivisions);
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::RoundCone(s) => {
                let (vtx, idx) =
                    s.to_outline(self.style.subdivisions, self.style.border_subdivisions);
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::RoundConvexPolyhedron(s) => {
                let (vtx, idx) = s.to_outline(self.style.border_subdivisions);
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::Voxels(s) => {
                let (vtx, idx) = s.to_outline();
                backend.draw_polyline(object, &vtx, &idx, pos, Vector::splat(1.0), color)
            }
            TypedShape::Custom(_) => {}
        }
    }
}
