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
}

impl OccluderFade {
    pub(super) fn focus(&self) -> Option<Vec3f> {
        self.focus
    }
}

impl Renderer {
    /// The point a follow camera films (the player's chest), in TRUE world
    /// space, or `None` to fade nothing. Call every frame.
    pub fn set_occluder_focus(&mut self, focus: Option<Vec3f>) {
        self.occluder.focus = focus;
    }
}
