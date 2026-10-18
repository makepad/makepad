//! The city: a voxel-grid city seen from the street, ray-marched block by block (the page's
//! `city3` and `shadeCity`) once per view into two RGBA16F targets in one pass (colour + alpha,
//! emission + the depth of the nearest surface in metres), then drawn into the scene list with a
//! cheap blit instance between the props behind it and the props in front of it. Props hidden by
//! the city (`hideC`) read its depth target.
//!
//! Owner after the foundation: stage A. Everything here is a stub: no city is drawn yet.
use crate::render::SceneCtx;
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The march: fb0 premultiplied body colour + alpha, fb1 window / beacon light + depth (m).
    mod.draw.DrawSkyCity = mod.std.set_type_default() do #(DrawSkyCity::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        fragment: fn(){
            self.fb0 = vec4(0.0, 0.0, 0.0, 0.0)
            self.fb1 = vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    // The march's result in the scene list (and mirrored in the reflection run).
    mod.draw.DrawSkyCityBlit = mod.std.set_type_default() do #(DrawSkyCityBlit::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        fragment: fn(){
            self.fb0 = vec4(0.0, 0.0, 0.0, 0.0)
            self.fb1 = vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.draw.SkyCityBase = #(City::script_component(vm))
    mod.draw.SkyCity = mod.std.set_type_default() do mod.draw.SkyCityBase{
        draw_city: mod.draw.DrawSkyCity{}
        draw_blit: mod.draw.DrawSkyCityBlit{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyCity { #[deref] pub draw_super: DrawQuad }

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyCityBlit { #[deref] pub draw_super: DrawQuad }

#[derive(Script, ScriptHook)]
pub struct City { #[live] pub draw_city: DrawSkyCity, #[live] pub draw_blit: DrawSkyCityBlit }

impl City {
    /// Marches the city into its own two targets, a pass the scene pass `consumer` samples (attach
    /// it with `cx.attach_child_pass(city, consumer, None)`). Called only when the view's picture
    /// changes; the city exists only where `p[P::CtW] >= 0.001`.
    pub fn draw_pass(&mut self, _cx: &mut Cx2d, _sc: &SceneCtx, _consumer: DrawPassId) {}

    /// The city in the scene list: its march blitted over what lies behind it (`sc.mirror`: the
    /// reflection run).
    pub fn draw_blit(&mut self, _cx: &mut Cx2d, _sc: &SceneCtx) {}

    /// The march's targets: colour + alpha, light + depth (m), once there are any.
    pub fn textures(&self) -> Option<(Texture, Texture)> { None }
}
