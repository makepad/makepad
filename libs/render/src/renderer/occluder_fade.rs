//! Occluder fade: model pixels between a follow camera and the body it
//! films, or right against the lens, draw screen-door dithered (the Mario 3D
//! grammar), so a pillar, a tree or a tower never hides the player.
//!
//! Per pixel, in both model shaders (DrawSceneSkinned and the DrawScenePbr
//! sibling, `occ_eye` / `occ_focus`). A pixel is dithered when it lies
//! inside a cone around the eye→focus line (0.55 m wide at the body, 0.22 m
//! wider per metre toward the lens, so the window matches the body on
//! screen) and short of the body (the last 0.8 m stays solid, so the body
//! and what it leans on do not dissolve), or within 1.2 m of the lens. Up to
//! 80% of a screen-space noise pattern is discarded there. The skinned
//! character batch never binds the uniforms, so the player is never
//! dithered. A host opts in each frame with [`Renderer::set_occluder_focus`];
//! `None` (every host that does not call it) writes the switch off.

use super::*;

#[derive(Default)]
pub(super) struct OccluderFade {
    focus: Option<Vec3f>,
    push: Option<(Vec3f, f32)>,
}

impl OccluderFade {
    pub(super) fn focus(&self) -> Option<Vec3f> {
        self.focus
    }

    /// The lanes' `occ_eye.w` and `occ_focus` from the fade and the foliage
    /// push: (push radius, focus xyz, clear length). The push rides the
    /// focus point (about 1 m over the feet); with no fade, the point is
    /// the pushing feet lifted 1 m and the clear length 0 (fade off).
    pub(super) fn uniforms(&self) -> (f32, Vec3f, f32) {
        match (self.focus, self.push) {
            (Some(f), push) => (push.map_or(0.0, |p| p.1), f, 0.8),
            (None, Some((feet, r))) => (r, feet + vec3f(0.0, 1.0, 0.0), 0.0),
            (None, None) => (0.0, Vec3f::default(), 0.0),
        }
    }
}

impl Renderer {
    /// The point a follow camera films (the player's chest), in TRUE world
    /// space, or `None` to fade nothing. Call every frame.
    pub fn set_occluder_focus(&mut self, focus: Option<Vec3f>) {
        self.occluder.focus = focus;
    }

    /// The followed body's feet in TRUE world space and a radius (m):
    /// swaying foliage (a `wind` layer: grass tufts, flowers, bushes) within
    /// it bends away and dips, so a run through a meadow parts it. Device
    /// local and per frame, like the fade; `None` bends nothing.
    pub fn set_foliage_push(&mut self, feet: Option<Vec3f>, radius: f32) {
        self.occluder.push = feet.filter(|_| radius > 0.0).map(|f| (f, radius));
    }
}
