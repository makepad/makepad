//! Named points projected to the screen each frame: what 2D boxes, labels and
//! leader lines attach to.
use makepad_math::*;
use crate::view::Camera;

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// The authoring node's name (a document path, an entity label).
    pub name: String,
    pub pos: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnchorProjection {
    /// Pixels from the viewport's top-left.
    pub screen: Vec2f,
    /// View-space distance along the camera's forward axis.
    pub depth: f32,
    /// In front of the near plane (a point behind the camera still gets a
    /// mirrored screen position a caller should not draw).
    pub in_front: bool,
}

impl Anchor {
    pub fn project(&self, camera: &Camera, viewport: Vec2f) -> AnchorProjection {
        project_point(self.pos, camera, viewport)
    }
}

pub fn project_point(pos: Vec3f, camera: &Camera, viewport: Vec2f) -> AnchorProjection {
    let aspect = if viewport.y > 0.0 { viewport.x / viewport.y } else { 1.0 };
    let view = camera.view_matrix().transform_vec4(vec4(pos.x, pos.y, pos.z, 1.0));
    let clip = camera.projection_matrix(aspect).transform_vec4(view);
    let w = if clip.w.abs() > 1.0e-6 { clip.w } else { 1.0e-6 };
    let ndc = vec2f(clip.x / w, clip.y / w);
    AnchorProjection {
        screen: vec2f((ndc.x * 0.5 + 0.5) * viewport.x, (0.5 - ndc.y * 0.5) * viewport.y),
        depth: -view.z,
        in_front: -view.z >= camera.near,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_look_at_target_lands_in_the_middle_and_behind_is_flagged() {
        let cam = Camera::look_at(vec3f(0.0, 0.0, 10.0), vec3f(0.0, 0.0, 0.0), 40.0);
        let a = Anchor { name: "hero".into(), pos: vec3f(0.0, 0.0, 0.0) }.project(&cam, vec2f(1920.0, 1080.0));
        assert!((a.screen.x - 960.0).abs() < 0.01 && (a.screen.y - 540.0).abs() < 0.01);
        assert!((a.depth - 10.0).abs() < 1.0e-4 && a.in_front);
        let up = project_point(vec3f(0.0, 1.0, 0.0), &cam, vec2f(1920.0, 1080.0));
        assert!(up.screen.y < 540.0, "+Y is up the screen");
        assert!(!project_point(vec3f(0.0, 0.0, 20.0), &cam, vec2f(100.0, 100.0)).in_front);
    }
}
