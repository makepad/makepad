use super::DebugColor;
use crate::dynamics::{
    ImpulseJoint, ImpulseJointHandle, Multibody, MultibodyLink, RigidBody, RigidBodyHandle,
};
use crate::geometry::{Aabb, Collider, ContactPair};
use crate::math::{Pose, Vector};
use crate::prelude::{ColliderHandle, MultibodyJointHandle};

/// The object currently being rendered by the debug-renderer.
#[derive(Copy, Clone)]
pub enum DebugRenderObject<'a> {
    /// A rigid-body is being rendered.
    RigidBody(RigidBodyHandle, &'a RigidBody),
    /// A collider is being rendered.
    Collider(ColliderHandle, &'a Collider),
    /// The AABB of a collider is being rendered.
    ColliderAabb(ColliderHandle, &'a Collider, &'a Aabb),
    /// An impulse-joint is being rendered.
    ImpulseJoint(ImpulseJointHandle, &'a ImpulseJoint),
    /// A multibody joint is being rendered.
    MultibodyJoint(MultibodyJointHandle, &'a Multibody, &'a MultibodyLink),
    /// The contacts of a contact-pair are being rendered.
    ContactPair(&'a ContactPair, &'a Collider, &'a Collider),
}

/// Trait implemented by graphics backends responsible for rendering the physics scene.
///
/// The only thing that is required from the graphics backend is to be able to render
/// a colored line. Note that the color is only a suggestion and is computed from the
/// `DebugRenderStyle`. The backend is free to apply its own style, for example based on
/// the `object` being rendered.
pub trait DebugRenderBackend {
    /// Predicate to filter-out some objects from the debug-rendering.
    fn filter_object(&self, _object: DebugRenderObject) -> bool {
        true
    }

    /// Draws a colored line.
    ///
    /// Note that this method can be called multiple time for the same `object`.
    fn draw_line(&mut self, object: DebugRenderObject, a: Vector, b: Vector, color: DebugColor);

    /// Draws a set of lines.
    fn draw_polyline(
        &mut self,
        object: DebugRenderObject,
        vertices: &[Vector],
        indices: &[[u32; 2]],
        transform: &Pose,
        scale: Vector,
        color: DebugColor,
    ) {
        for idx in indices {
            let a = *transform * (vertices[idx[0] as usize] * scale);
            let b = *transform * (vertices[idx[1] as usize] * scale);
            self.draw_line(object, a, b, color);
        }
    }

    /// Draws a chain of lines.
    fn draw_line_strip(
        &mut self,
        object: DebugRenderObject,
        vertices: &[Vector],
        transform: &Pose,
        scale: Vector,
        color: DebugColor,
        closed: bool,
    ) {
        for vtx in vertices.windows(2) {
            let a = *transform * (vtx[0] * scale);
            let b = *transform * (vtx[1] * scale);
            self.draw_line(object, a, b, color);
        }

        if closed && vertices.len() > 2 {
            let a = *transform * (vertices[0] * scale);
            let b = *transform * (*vertices.last().unwrap() * scale);
            self.draw_line(object, a, b, color);
        }
    }
}
