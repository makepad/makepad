use crate::{cx_2d::*, makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.sdf.*
    use mod.draw
    use mod.geom

    // A one-pixel wireframe frame around a turtle scope, drawn only while the
    // exploded z-layer view is up. Its whole job is to make a nesting level
    // visible even when a child completely covers its parent — the level you
    // most want to click and cannot otherwise see or reach.
    //
    // Depth needs no instance field of its own: the hairline is emitted at the
    // `end_turtle` funnel, so the draw call it lands in is stamped with the
    // closing scope's own nesting depth (`CxDrawCall::turtle_depth`) and the
    // ordinary `world.z = draw_depth + zbias` already puts it on the right
    // plane. Inheriting DrawQuad's transform keeps it in the explode with
    // every other quad.
    mod.draw.DrawSplodedHairline = mod.std.set_type_default() do #(DrawSplodedHairline::script_shader(vm)){
        ..mod.draw.DrawQuad

        // `level` is a Rust instance field (below) — declaring it here too
        // would clash with it, the way rect_pos/rect_size are left undeclared.
        pixel: fn(){
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // These stay HOLLOW. `stroke` covers |distance| < width, so on a
            // small container a wide stroke reaches the middle from both sides
            // and the frame reads as a solid slab — hence the clamp to a
            // fraction of the shorter side.
            // 2.0 = twice the original one-pixel hairline. A script-level
            // `let` is NOT in scope inside shader code, so this stays a
            // literal — bind one and the shader silently fails to compile and
            // nothing draws at all.
            let w = min(2.0, min(self.rect_size.x, self.rect_size.y) * 0.2)
            // The radius is load-bearing, not decoration. Sdf2d.box clamps its
            // interior to a FLAT plateau at -k, where k = min(2 * r, half the
            // shorter side) — it is not a true signed distance inside. `stroke`
            // paints wherever |shape| < width, so with 2 * r <= width the
            // plateau is inside the stroke band and the whole box fills solid.
            // r = w gives k = 2w > w, which keeps the interior outside the band.
            sdf.box(w, w, self.rect_size.x - 2.0 * w, self.rect_size.y - 2.0 * w, w)
            // Deepest levels read warmest, so the eye can rank planes without
            // counting them. 12 is a nesting depth no real UI exceeds by much.
            let t = clamp(self.level / 12.0, 0.0, 1.0)
            let tint = vec3(0.45, 0.72, 1.0).mix(vec3(1.0, 0.62, 0.18), t)
            // Stroke only — never fill. The exploded view doubles as an
            // overdraw instrument: a solid plane must always mean the APP
            // painted those pixels. A frame the mode adds marks existence
            // only, so it stays a hollow outline over a transparent plane.
            let a = 0.8
            sdf.stroke(vec4(tint * a, a), w)
            return sdf.result
        }
    }
}

/// The wireframe frame for one turtle scope in the exploded view.
///
/// Field-ordering law (CLAUDE.md item 16): only `#[live]` instance fields may
/// follow `#[deref]`, because `DrawVars::as_slice` reads straight past the end
/// of the base struct into them.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSplodedHairline {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub level: f32,
}

impl DrawSplodedHairline {
    /// Emit one frame at `rect` as an ALIGNED instance, so it rides every
    /// deferred alignment shift the way real content does. A CPU-side rect log
    /// would go stale the moment `move_align_list` shifted the instances it
    /// was describing — which is exactly the trap this avoids.
    pub fn draw_scope(&mut self, cx: &mut Cx2d, rect: Rect, level: f32) {
        if self.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.level = level;
        self.draw_super.rect_pos = rect.pos.into();
        self.draw_super.rect_size = rect.size.into();
        // Unclipped: the frame of a scrolled-away parent is still structure.
        self.draw_super.draw_clip = vec4(-1.0e6, -1.0e6, 1.0e6, 1.0e6);
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        // The returned area is deliberately dropped: one shared wireframe
        // emits every scope in the frame, so holding on to the last one's
        // area would leave a stale reference behind on the next redraw.
        cx.add_aligned_instance(&self.draw_vars);
    }
}
