use crate::math::Real;

/// A color for debug-rendering.
///
/// The default colors are provided in HSLA (Hue Saturation Lightness Alpha) format.
pub type DebugColor = [f32; 4];

/// Style used for computing colors when rendering the scene.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DebugRenderStyle {
    /// The number of subdivisions used to approximate the curved
    /// parts of a shape with smooth faces.
    pub subdivisions: u32,
    /// The number of subdivisions used to approximate the curved
    /// borders of round shapes.
    pub border_subdivisions: u32,
    /// The color of colliders attached to dynamic rigid-bodies.
    pub collider_dynamic_color: DebugColor,
    /// The color of colliders attached to fixed rigid-bodies.
    pub collider_fixed_color: DebugColor,
    /// The color of colliders attached to kinematic rigid-bodies.
    pub collider_kinematic_color: DebugColor,
    /// The color of colliders not attached to any rigid-body.
    pub collider_parentless_color: DebugColor,
    /// The color of the line between a rigid-body’s center-of-mass and the
    /// anchors of its attached impulse joints.
    pub impulse_joint_anchor_color: DebugColor,
    /// The color of the line between the two anchors of an impulse joint.
    pub impulse_joint_separation_color: DebugColor,
    /// The color of the line between a rigid-body’s center-of-mass and the
    /// anchors of its attached multibody joints.
    pub multibody_joint_anchor_color: DebugColor,
    /// The color of the line between the two anchors of a multibody joint.
    pub multibody_joint_separation_color: DebugColor,
    /// If a rigid-body is sleeping, its attached entities will have their colors
    /// multiplied by this array. (For a joint, both attached rigid-bodies must be sleeping
    /// or non-dynamic for this multiplier to be applied).
    pub sleep_color_multiplier: DebugColor,
    /// If a rigid-body is disabled, its attached entities will have their colors
    /// multiplied by this array. (For a joint, both attached rigid-bodies must be disabled
    /// for this multiplier to be applied).
    pub disabled_color_multiplier: DebugColor,
    /// The length of the local coordinate axes rendered for a rigid-body.
    pub rigid_body_axes_length: Real,
    /// The color for the segments joining the two contact points.
    pub contact_depth_color: DebugColor,
    /// The color of the contact normals.
    pub contact_normal_color: DebugColor,
    /// The length of the contact normals.
    pub contact_normal_length: Real,
    /// The color of the colliders' [`Aabb`](crate::geometry::Aabb)s.
    pub collider_aabb_color: DebugColor,
}

impl Default for DebugRenderStyle {
    fn default() -> Self {
        Self {
            subdivisions: 20,
            border_subdivisions: 5,
            collider_dynamic_color: [340.0, 1.0, 0.3, 1.0],
            collider_kinematic_color: [20.0, 1.0, 0.3, 1.0],
            collider_fixed_color: [30.0, 1.0, 0.4, 1.0],
            collider_parentless_color: [30.0, 1.0, 0.4, 1.0],
            impulse_joint_anchor_color: [240.0, 0.5, 0.4, 1.0],
            impulse_joint_separation_color: [0.0, 0.5, 0.4, 1.0],
            multibody_joint_anchor_color: [300.0, 1.0, 0.4, 1.0],
            multibody_joint_separation_color: [0.0, 1.0, 0.4, 1.0],
            sleep_color_multiplier: [1.0, 1.0, 0.2, 1.0],
            disabled_color_multiplier: [0.0, 0.0, 1.0, 1.0],
            rigid_body_axes_length: 0.5,
            contact_depth_color: [120.0, 1.0, 0.4, 1.0],
            contact_normal_color: [0.0, 1.0, 1.0, 1.0],
            contact_normal_length: 0.3,
            collider_aabb_color: [124.0, 1.0, 0.4, 1.0],
        }
    }
}
