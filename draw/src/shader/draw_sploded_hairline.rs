use crate::{cx_2d::*, makepad_platform::*};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    // The container outline for the exploded z-layer view: a closed frame
    // strip marking that a nesting level exists, drawn only while the mode is
    // up. Its job is the level you most want to click and cannot otherwise
    // see — a parent its children cover completely.
    //
    // It is NOT a DrawQuad. A quad would submit a full-plane polygon and carve
    // the middle away in the pixel shader, which costs fill-rate over the whole
    // container and, at any visible alpha, fogs the layers behind it. The mode
    // doubles as an overdraw instrument: a covered pixel must mean the APP
    // painted it. So the only geometry submitted here is the frame ring itself
    // (`geom.OutlineGeom`, eight triangles around the loop) and the middle of
    // the container is never rasterized at all.
    //
    // Depth needs no instance field: the outline is emitted at the end_turtle
    // funnel, so the draw call it lands in carries the closing scope's own
    // nesting depth (`CxDrawCall::turtle_depth`) and the ordinary
    // `world.z = draw_depth + zbias` puts it on the right plane.
    mod.draw.DrawSplodedHairline = mod.std.set_type_default() do #(DrawSplodedHairline::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.OutlineVertex, geom.OutlineGeom)

        // 0 at the strip's outer edge, 1 at its inner edge — the only
        // coordinate the pixel stage needs, for the AA falloff.
        across: varying(float)
        world: varying(vec4f)

        vertex: fn() {
            // The stroke thins on small containers so a frame never closes up
            // into a solid patch, and gains half a pixel on each side for the
            // antialiased falloff.
            let w = min(self.stroke, min(self.rect_size.x, self.rect_size.y) * 0.25)
            let band = w + 1.0
            // Outward from the rect centre for this corner: (+1,+1) at (0,0),
            // (-1,+1) at (1,0), and so on.
            let dir = sign(self.geom.pos - vec2(0.5, 0.5))
            let corner = self.rect_pos + self.geom.pos * self.rect_size
            // Outer ring sits half the band outside the border, inner ring the
            // same distance inside, so the stroke straddles the rect edge.
            let offset = mix(0.5, -0.5, self.geom.inner) * band
            let p = corner + dir * offset

            // Honour the same clip the app's own draws honour, so a container
            // inside a scrolled viewport outlines only its VISIBLE part and
            // one scrolled fully out of view collapses to nothing. Clamping
            // the ring's vertices cuts the frame straight at the viewport
            // edge, which is what a cut container should look like.
            let clipped = clamp(
                clamp(p, self.draw_clip.xy, self.draw_clip.zw)
                + self.draw_list.view_shift
                self.draw_list.view_clip.xy
                self.draw_list.view_clip.zw
            )

            self.across = self.geom.inner
            self.world = self.draw_list.view_transform * vec4(
                clipped.x
                clipped.y
                self.draw_depth + self.draw_call.zbias
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        fragment: fn(){
            self.fb0 = self.pixel()
        }

        pixel: fn(){
            // Antialias in SCREEN space, not band space. The strip is drawn
            // through the explode camera, so its on-screen width varies with
            // the rotation and the fit scale; a fixed falloff in band units
            // goes crunchy exactly where the rotation is strongest. The
            // screen derivative of `across` says how much of the band one
            // pixel covers here, which makes the falloff one real pixel wide
            // everywhere.
            let across_per_px = max(
                length(vec2(dFdx(self.across), dFdy(self.across)))
                0.00001
            )
            let edge = min(self.across, 1.0 - self.across)
            let aa = clamp(edge / across_per_px, 0.0, 1.0)
            // Deepest levels read warmest, so the eye can rank planes without
            // counting them. 12 is a nesting depth no real UI exceeds by much.
            let t = clamp(self.level / 12.0, 0.0, 1.0)
            let depth_tint = vec3(0.45, 0.72, 1.0).mix(vec3(1.0, 0.62, 0.18), t)
            // The tweaker's marks: the SAME colours its flat hover and pinned
            // outlines use, so lighting up reads identically in both modes.
            // emphasis 1 = hover (cyan), 2 = pinned (orange), both full alpha.
            let hover = clamp(self.emphasis, 0.0, 1.0)
            let pinned = clamp(self.emphasis - 1.0, 0.0, 1.0)
            let tint = depth_tint
                .mix(vec3(0.19, 0.78, 1.0), hover)
                .mix(vec3(1.0, 0.62, 0.13), pinned)
            let alpha = mix(0.8, 1.0, hover)
            // Premultiplied, so the blend does not fringe over varied
            // backgrounds.
            let a = alpha * aa
            return vec4(tint * a, a)
        }
    }
}

/// One container's frame in the exploded view.
///
/// Field-ordering law (CLAUDE.md item 16): only `#[live]` instance fields may
/// follow `#[deref]`, because `DrawVars::as_slice` reads straight past the end
/// of the base struct into them. There is no `#[deref]` here — this draw class
/// owns its own geometry rather than inheriting DrawQuad's — so the whole
/// struct after `draw_vars` is instance data.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSplodedHairline {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    /// Filled in by the align pass from the live clip stack — the SAME clip
    /// the app's own draws get. Named `draw_clip` so `CxDrawShaderMapping`
    /// finds its slot; do not rename.
    #[live]
    pub draw_clip: Vec4f,
    #[live(0.0)]
    pub draw_depth: f32,
    /// Nesting level, for the depth tint.
    #[live]
    pub level: f32,
    /// Stroke width in logical pixels.
    #[live(1.5)]
    pub stroke: f32,
    /// 0 = a scope frame; 1 = the tweaker's hover outline; 2 = its pinned
    /// selection. Drives the tint and full alpha.
    #[live(0.0)]
    pub emphasis: f32,
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
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        // The returned area is deliberately dropped: one shared outline emits
        // every scope in the frame, so holding on to the last one's area would
        // leave a stale reference behind on the next redraw.
        cx.add_aligned_instance(&self.draw_vars);
    }

    /// Emit one of the tweaker's marks — a hover (`emphasis` 1) or pinned
    /// (`emphasis` 2) outline — at an already-clipped screen rect. Unaligned:
    /// the mark list sits directly under the pass root, so `rect` IS the
    /// instance's position and no alignment pass moves it. The caller sets
    /// `cx.nesting_depth` to the mark's level first, which is what lands the
    /// draw call on the marked widget's own plane.
    pub fn draw_mark(&mut self, cx: &mut Cx2d, rect: Rect, level: f32, emphasis: f32, stroke: f32) {
        if self.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.level = level;
        self.emphasis = emphasis;
        self.stroke = stroke;
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw_clip = vec4(-1.0e6, -1.0e6, 1.0e6, 1.0e6);
        // In-plane, but above everything the widget itself painted there:
        // a selected container's outline must not vanish under its own
        // background. 100 stays well inside the per-level headroom.
        self.draw_depth = 100.0;
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        cx.add_instance(&self.draw_vars);
        self.emphasis = 0.0;
        self.draw_depth = 0.0;
    }
}
