//! ScreenView -- a view that is a display window.
//!
//! A slim bezel, the screen face set into it with an inner shadow under its
//! top edge, and a sheet of glass over whatever the view holds: a readout,
//! a label, a meter. The children are drawn between the face and the glass,
//! so the sheen lies over the digits as it does on a real display, and not
//! only on the empty face around them.
//!
//! The glass is a second quad drawn after the children, inside one turtle
//! of the widget's own so the parent sees a single walk (the same order of
//! drawing `CornerCapView` uses for its caps). It costs one quad per screen.
//!
//! The face is the view's own `draw_bg` and the glass `draw_glass`; both
//! read `bezel`, `recess`, `sheen` and `scan` as instances, which the widget
//! keeps in step with its properties, so a sheet replaces either pixel
//! function and still sees the geometry the page asked for. The glass takes
//! the face's `border_radius` the same way, as its `face_radius`.
//!
//! The bezel is a shade and the lip a light laid over the ground, so the
//! window reads on a dark ground and a light one alike: a ring a little
//! darker than whatever it is set in, a device pixel of dark at its edge,
//! and a device pixel of light on the ground under its bottom edge.
use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ScreenViewBase = #(ScreenView::register_widget(vm))
    /** A display window: a slim bezel, a recessed screen face with an inner
     * shadow at the top, and a glass sheen over the children it holds. */
    mod.widgets.ScreenView = set_type_default() do mod.widgets.ScreenViewBase{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        padding: Inset{top: 7. right: 9. bottom: 7. left: 9.}
        show_bg: true

        /** the bezel's width, in points 0..12 step 0.5 */
        bezel: 2.0
        /** how deep the face sits: the inner shadow's reach under the top edge, in points 0..12 step 0.5 */
        recess: 4.0
        /** the glass sheen's strength over the face and its content 0..0.12 step 0.005 */
        sheen: 0.03
        /** scan line depth; 0 draws none 0..0.5 step 0.01 */
        scan: 0.0

        draw_bg +: {
            bezel: instance(2.0)
            recess: instance(4.0)
            scan: instance(0.0)
            /** the corner of the bezel's outer edge, in points 0..16 step 0.5 */
            border_radius: uniform(theme.corner_radius * 0.5)
            /** the screen face */
            color: uniform(theme.color_screen)
            /** the bezel ring: a shade laid over the ground, so it reads on any ground */
            color_bezel: uniform(theme.color_d_05)
            /** the lip catching the light under the bottom edge: light laid over the ground */
            color_lip: uniform(theme.color_u_15)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                // The edges on whole device pixels, so the one-pixel lines
                // on them are one pixel and not two half-lit ones. The last
                // row of the box is the lip's, under the outline.
                let lo = vec2(Finish.snap(self.rect_pos.x, px), Finish.snap(self.rect_pos.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + self.rect_size.x, px), Finish.snap(self.rect_pos.y + self.rect_size.y, px) - px) - self.rect_pos
                let c = (lo + hi) * 0.5
                let ho = (hi - lo) * 0.5
                let r_out = min(self.border_radius, min(ho.x, ho.y))
                let outer = Material.sd_box(p, c, ho, r_out)
                let b = Finish.snap(max(self.bezel, 0.0), px)
                let hf = max(ho - vec2(b, b), vec2(0.5, 0.5))
                let face_d = Material.sd_box(p, c, hf, max(r_out - b, 0.0))

                // The bezel: a flat ring, a shade of whatever ground it is
                // set in, with a device pixel of dark at its outer edge.
                // Premultiplied from here on, so a translucent bezel stays
                // one over the ground.
                let bz = self.color_bezel
                var col = vec4(bz.rgb * bz.a, bz.a)
                col = Finish.over(col, vec4(0.0, 0.0, 0.0, 0.45 * Finish.band(outer, 0.0, px, px)))

                // The face, with the surround's shadow falling across it
                // from the top: dark at the edge, gone at `recess`.
                var face = self.color.rgb
                if self.recess > 0.0 {
                    let insh = Finish.inset(p, c, hf, max(r_out - b, 0.0), vec2(0.0, self.recess * 0.5), max(self.recess * 0.45, 0.3))
                    face = face * (1.0 - 0.55 * insh)
                }
                if self.scan > 0.0 {
                    face = face * Finish.scan(p.y - (c.y - hf.y), px, 3.0 * px, self.scan)
                }
                // One device pixel of black where the glass meets the bezel.
                face = mix(face, face * 0.4, Finish.band(face_d, 0.0, px, px))
                col = mix(col, vec4(face, 1.0), Finish.cover(face_d, px))
                col = col * Finish.cover(outer, px)
                // The lip: a device pixel of light on the ground under the
                // bottom edge only, and not up the sides.
                let lip = Finish.ring_out(outer, 0.0, px, px) * step(hi.y, p.y)
                let la = self.color_lip.a * lip
                return Finish.over(vec4(self.color_lip.rgb * la, la), col)
            }
        }

        draw_glass +: {
            bezel: instance(2.0)
            sheen: instance(0.03)
            // The face's `border_radius`, which the widget copies here
            // before each draw so the glass always has the face's corner.
            face_radius: instance(2.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let lo = vec2(Finish.snap(self.rect_pos.x, px), Finish.snap(self.rect_pos.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + self.rect_size.x, px), Finish.snap(self.rect_pos.y + self.rect_size.y, px) - px) - self.rect_pos
                let c = (lo + hi) * 0.5
                let ho = (hi - lo) * 0.5
                let r_out = min(self.face_radius, min(ho.x, ho.y))
                let b = Finish.snap(max(self.bezel, 0.0), px)
                let hf = max(ho - vec2(b, b), vec2(0.5, 0.5))
                let face_d = Material.sd_box(p, c, hf, max(r_out - b, 0.0))
                // A straight diagonal band from the top left to under half
                // way across, and a lighter line one device pixel inside the
                // top edge. Nothing more: the glass is quiet.
                let u = (p - (c - hf)) / max(hf * 2.0, vec2(1.0, 1.0))
                let diag = u.x + u.y * 0.6
                let band = smoothstep(0.0, 0.12, diag) * (1.0 - smoothstep(0.28, 0.5, diag))
                let top = Finish.cover(abs(p.y - (c.y - hf.y) - px * 1.5) - px * 0.5, px) * 0.6
                let a = clamp(self.sheen * (band + top), 0.0, 1.0) * Finish.cover(face_d + px, px)
                return vec4(a, a, a, a)
            }
        }
    }
}

/// A view that is a display window.
#[derive(Script, ScriptHook, Widget)]
pub struct ScreenView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_glass: DrawQuad,
    /// The bezel's width, in points.
    #[live(2.0)]
    pub bezel: f64,
    /// The inner shadow's reach under the face's top edge, in points.
    #[live(4.0)]
    pub recess: f64,
    /// The glass sheen's strength.
    #[live(0.03)]
    pub sheen: f64,
    /// Scan line depth; zero draws none.
    #[live]
    pub scan: f64,
    #[rust]
    draw_state: DrawStateWrap<ScreenDraw>,
    #[rust]
    #[area]
    area: Area,
}

#[derive(Clone)]
enum ScreenDraw {
    Content,
    Glass,
}

impl Widget for ScreenView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ScreenDraw::Content) {
            if !self.view.visible() {
                self.draw_state.end();
                return DrawStep::done();
            }
            cx.begin_turtle(walk, Layout::default());
            let bezel = self.bezel.max(0.0) as f32;
            self.view.draw_bg.draw_vars.set_dyn_instance(cx, id!(bezel), &[bezel]);
            self.view.draw_bg.draw_vars.set_dyn_instance(cx, id!(recess), &[self.recess.max(0.0) as f32]);
            self.view.draw_bg.draw_vars.set_dyn_instance(cx, id!(scan), &[self.scan.clamp(0.0, 1.0) as f32]);
            self.draw_glass.draw_vars.set_dyn_instance(cx, id!(bezel), &[bezel]);
            self.draw_glass.draw_vars.set_dyn_instance(cx, id!(sheen), &[self.sheen.clamp(0.0, 1.0) as f32]);
            let mut radius = [0.0f32];
            self.view.draw_bg.get_uniform(cx, id!(border_radius), &mut radius);
            self.draw_glass.draw_vars.set_dyn_instance(cx, id!(face_radius), &radius);
        }
        if let Some(ScreenDraw::Content) = self.draw_state.get() {
            // The inner view fills the turtle, or measures it when the walk
            // asked to fit: a Fill inside a Fit collapses to nothing.
            let inner = Walk {
                width: if walk.width.is_fit() { Size::fit() } else { Size::fill() },
                height: if walk.height.is_fit() { Size::fit() } else { Size::fill() },
                ..Walk::default()
            };
            self.view.draw_walk(cx, scope, inner)?;
            self.draw_state.set(ScreenDraw::Glass);
        }
        if let Some(ScreenDraw::Glass) = self.draw_state.get() {
            // Drawn at any sheen: a sheet may have given the glass a pixel
            // function of its own that does not read it.
            let rect = self.view.area().rect(cx);
            self.draw_glass.draw_abs(cx, rect);
            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[cfg(test)]
mod tests {
    use crate::makepad_draw::*;

    /// The source the shader compiler writes for one draw object on one
    /// backend, or its errors.
    fn compile(vm: &mut ScriptVm, draw: ScriptValue, backend: &str) -> String {
        let value = match backend {
            "metal" => crate::script_eval!(vm, {mod.shader.test_compile_draw_source(#(draw), "metal", false)}),
            "hlsl" => crate::script_eval!(vm, {mod.shader.test_compile_draw_source(#(draw), "hlsl", false)}),
            "glsl" => crate::script_eval!(vm, {mod.shader.test_compile_draw_source(#(draw), "glsl", false)}),
            _ => crate::script_eval!(vm, {mod.shader.test_compile_draw_source(#(draw), "wgsl", false)}),
        };
        vm.bx
            .heap
            .string_with(value, |_heap, text| text.to_string())
            .expect("the compiler answers with source")
    }

    /// Every face the instruments add, and the window's ground, compiles on
    /// every backend the shader compiler writes: a face that does not is
    /// drawn as nothing at all, with no error anywhere on screen. Each is
    /// checked for a name only its own pixel function reads, so a face that
    /// fell back to a default does not pass.
    #[test]
    fn every_instrument_face_compiles_on_every_backend() {
        crate::on_test_cx(|| {
            let mut cx = crate::checkout_test_cx();
            cx.with_vm(|vm| {
                let faces = [
                    ("Readout", "glyph_width", crate::script_eval!(vm, {mod.widgets.Readout.draw_bg})),
                    ("Lamp", "lens_tint", crate::script_eval!(vm, {mod.widgets.Lamp.draw_bg})),
                    ("LampBar", "lens_tint", crate::script_eval!(vm, {mod.widgets.LampBar.draw_bg})),
                    ("NeedleMeter", "pivot_size", crate::script_eval!(vm, {mod.widgets.NeedleMeter.draw_bg})),
                    ("ScreenView face", "color_bezel", crate::script_eval!(vm, {mod.widgets.ScreenView.draw_bg})),
                    ("ScreenView glass", "face_radius", crate::script_eval!(vm, {mod.widgets.ScreenView.draw_glass})),
                    ("ToggleRocker", "rocker_color", crate::script_eval!(vm, {mod.widgets.ToggleRocker.draw_bg})),
                    ("ToggleSlide", "grip_pitch", crate::script_eval!(vm, {mod.widgets.ToggleSlide.draw_bg})),
                ];
                let ground = crate::script_eval!(vm, {mod.widgets.Window.draw_bg});
                for backend in ["metal", "hlsl", "glsl", "wgsl"] {
                    for (name, own, draw) in faces {
                        let text = compile(vm, draw, backend);
                        assert!(!text.starts_with("ERRORS"), "{name} did not compile for {backend}: {text}");
                        assert!(text.contains(own), "{name} for {backend} is not its own shader");
                    }
                    // The window's ground has no name of its own to look
                    // for: it is the view's face without `get_color`, which
                    // every view face calls and the ground does not.
                    let text = compile(vm, ground, backend);
                    assert!(!text.starts_with("ERRORS"), "the window ground did not compile for {backend}: {text}");
                    assert!(text.contains("premul") && !text.contains("get_color"), "the window ground for {backend} is not its own shader");
                }
            });
        });
    }
}
