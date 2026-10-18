//! After the scene: the blur of the lights (Sharpness), the water reflections and the final
//! composite: colour plus light, then the display tone shoulder (an export keeps the raw HDR).
//!
//! Owner after the foundation: stage D (blur, reflections, export). The foundation's final pass is
//! here: sharp, no reflections. The blur (`DrawSkyBlur`) and the reflection decision are stubs.
use crate::params::Params;
use crate::render::{SceneCtx, SceneTarget, ViewSpec};
use crate::zones::Zones;
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The final composite of one view: the scene's colour plus its light (premultiplied, so the
    // light also shows over the sky), then the tone shoulder above 1.0 unless `tone` is 0.
    mod.draw.DrawSkyFinal = mod.std.set_type_default() do #(DrawSkyFinal::script_shader(vm)){
        ..mod.draw.DrawQuad,
        scene_color: texture_2d(float)
        scene_light: texture_2d(float)
        tone: uniform(1.0)
        pixel: fn(){
            let c = self.scene_color.sample_nearest(self.pos)
            let e = self.scene_light.sample_nearest(self.pos)
            var o = c.xyz + e.xyz
            if self.tone > 0.5 {
                let hi = max(o - vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0))
                o = min(o, vec3(1.0, 1.0, 1.0)) + (vec3(1.0, 1.0, 1.0) - exp(vec3(0.0, 0.0, 0.0) - hi)) * 0.4
            }
            return vec4(o, c.w)
        }
    }

    // One separable gaussian step of the lights (stage D).
    mod.draw.DrawSkyBlur = mod.std.set_type_default() do #(DrawSkyBlur::script_shader(vm)){
        ..mod.draw.DrawQuad,
        pixel: fn(){
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.draw.SkyPostBase = #(Post::script_component(vm))
    mod.draw.SkyPost = mod.std.set_type_default() do mod.draw.SkyPostBase{
        draw_final: mod.draw.DrawSkyFinal{}
        draw_blur: mod.draw.DrawSkyBlur{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyFinal {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyBlur {
    #[deref]
    pub draw_super: DrawQuad,
}

/// The scene's targets as post sees them.
#[allow(dead_code)] // the blur and the reflections read the view and the mirror
pub struct SceneOut<'a> {
    pub view: &'a ViewSpec,
    pub scene: &'a SceneTarget,
    /// The mirrored scene, when [`Post::wants_mirror`] asked for it.
    pub mirror: Option<&'a SceneTarget>,
}

#[derive(Script, ScriptHook)]
pub struct Post {
    #[live]
    pub draw_final: DrawSkyFinal,
    #[live]
    pub draw_blur: DrawSkyBlur,
}

impl Post {
    /// Whether the scene list also runs mirrored into a second target (water reflections: some
    /// reflection strength and some sea in view). Stub: never.
    pub fn wants_mirror(&self, _p: &Params, _z: &Zones) -> bool {
        false
    }

    /// The passes between the scene and the final (the blur of the light target). Called only
    /// when the view's picture changes; their pass chain must end in a pass `draw_final` attaches.
    /// Stub: none (the view is sharp).
    pub fn draw_passes(&mut self, _cx: &mut Cx2d, _sc: &SceneCtx, _out: &SceneOut) {}

    /// The final composite into `rect` of the current pass, every draw. It attaches the passes it
    /// samples to the current pass (so they paint first and stay alive).
    pub fn draw_final(&mut self, cx: &mut Cx2d, rect: Rect, _p: &Params, out: &SceneOut, tone: bool) {
        cx.make_child_pass(&out.scene.pass);
        let vars = &mut self.draw_final.draw_vars;
        crate::render::bind_texture(cx, vars, live_id!(scene_color), &out.scene.color);
        crate::render::bind_texture(cx, vars, live_id!(scene_light), &out.scene.light);
        vars.set_uniform(cx, live_id!(tone), &[if tone { 1.0 } else { 0.0 }]);
        self.draw_final.draw_abs(cx, rect);
    }
}
