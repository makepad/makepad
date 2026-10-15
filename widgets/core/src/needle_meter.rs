//! NeedleMeter -- a needle on a printed scale, for a value read by angle.
//!
//! A needle is read differently from a bar: by its angle against a scale the
//! eye already knows, and by how it moves. So the scale is part of the face
//! -- major and minor ticks on an arc, and an optional red zone at the loud
//! end -- and the needle eases to a new value rather than jumping, the way a
//! needle with mass does. The ease stops the moment it lands, so a meter
//! that is not being written costs nothing.
//!
//! `needle_angle` is the one mapping from a value to an angle; the shader
//! runs the same arithmetic, and a test holds it.
//!
//! The face can have glass over it that answers the pointer (`field_reach`):
//! `draw_glass`, drawn over the face and its caption only while the reach
//! is above zero, carries the pointer's field and the trail of drops the
//! pointer leaves as it moves across (see `PointerGlass`), and with
//! `glass_backdrop` reads the face under it while the trail is alive, so a
//! sheet's glass can bend the needle and the scale. Off by default.
use crate::{
    lamp::{amount_of, Glide},
    makepad_derive_widget::*,
    makepad_draw::*,
    pointer_field::{PointerGlass, PointerGlassSetup},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawNeedleMeterBase = #(DrawNeedleMeter::script_component(vm))
    set_type_default() do #(DrawNeedleMeter::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.NeedleMeterBase = #(NeedleMeter::register_widget(vm))
    /** A needle meter: a face, a scale of major and minor ticks with an
     * optional red zone, and a needle for a value from 0 to 1 that eases
     * to where it is sent, under a pivot cover. */
    mod.widgets.NeedleMeter = set_type_default() do mod.widgets.NeedleMeterBase{
        width: theme.size_control_l * 4.
        height: theme.size_control_l * 2.5
        flow: Down
        // The caption stands in the lower left, clear of the needle's
        // travel and the pivot cover.
        align: Align{x: 0.0 y: 1.0}
        padding: Inset{left: 7. bottom: 5. right: 7. top: 5.}
        /** where the needle points, 0 at the left end of the scale and 1 at the right 0..1 step 0.01 */
        value: 0.0
        /** seconds the needle takes to reach a new value 0..2 step 0.05 */
        ease_secs: 0.3
        /** a caption printed on the face, under the scale */
        label: ""
        /** dimmed, for a meter that is not live 0..1 step 1 */
        disabled: false
        /** how far past the face the pointer's field and trail reach, in points; 0 keeps them and the glass off 0..24 step 0.5 */
        field_reach: 0.0
        /** how far the pointer moves between the trail's drops, in points 1..40 step 0.5 */
        trail_spacing: 6.0
        /** how long a drop of the trail lives, in seconds 0.2..4 step 0.05 */
        trail_life: 1.3
        /** while the trail is alive, the glass reads the face under it from the window's scene */
        glass_backdrop: false

        // The glass over the face, drawn only while `field_reach` is above
        // zero; clear until a sheet gives it a pixel function. It gets the
        // face's corner as `face_radius`.
        draw_glass +: {
            face_radius: instance(4.0)
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }

        draw_label +: {
            color: theme.color_screen_ink
            text_style: theme.font_regular{font_size: theme.font_size_p * 0.8}
        }

        // `value` and `opacity` are the instances: they ride in the draw
        // struct, so every other property here is a uniform or the
        // instance slots stop lining up with the struct.
        draw_bg +: {
            value: 0.0
            opacity: 1.0
            /** the scale's sweep, in degrees 30..300 step 1 */
            sweep: uniform(96.0)
            /** major divisions along the scale 1..20 step 1 */
            majors: uniform(5.0)
            /** minor divisions inside each major one 1..10 step 1 */
            minors: uniform(4.0)
            /** where the red zone starts on the scale; 1 draws none 0..1 step 0.01 */
            zone: uniform(0.8)
            /** a major tick's length, in points 1..16 step 0.5 */
            tick_major: uniform(6.0)
            /** a minor tick's length, in points 1..16 step 0.5 */
            tick_minor: uniform(3.0)
            /** the pivot cover's radius, in points 1..12 step 0.5 */
            pivot_size: uniform(4.0)
            /** the face's corner, in points 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** the strength of the glass sheen over the face 0..0.1 step 0.005 */
            sheen: uniform(0.025)
            /** the face */
            color: uniform(theme.color_screen)
            /** the scale: its arc and its ticks */
            color_scale: uniform(theme.color_screen_ink)
            /** the needle and its pivot cover */
            color_needle: uniform(theme.color_screen_ink)
            /** the red zone */
            color_zone: uniform(theme.color_error)

            // The pivot and the scale radius for the face's size, so every
            // part is drawn around the same point: the pivot sits low in
            // the face, and the scale is as large as the face allows.
            pivot: fn() -> vec2 {
                return vec2(self.rect_size.x * 0.5, self.rect_size.y - self.pivot_size - 4.0)
            }

            scale_radius: fn() -> float {
                let half = self.sweep * 0.5 * 0.017453293
                let pv = self.pivot()
                let by_width = (self.rect_size.x * 0.5 - 8.0) / max(sin(min(half, 1.5707963)), 0.1)
                return max(min(pv.y - 7.0, by_width), 4.0)
            }

            // The needle's angle for a value, in radians from straight up,
            // clockwise.
            needle_angle: fn(v: float) -> float {
                return (clamp(v, 0.0, 1.0) - 0.5) * self.sweep * 0.017453293
            }

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                // The face's edges on whole device pixels, so its rim is one
                // pixel wide wherever the layout put the meter.
                let lo = vec2(Finish.snap(self.rect_pos.x, px), Finish.snap(self.rect_pos.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + self.rect_size.x, px), Finish.snap(self.rect_pos.y + self.rect_size.y, px)) - self.rect_pos
                let c = (lo + hi) * 0.5
                let hc = (hi - lo) * 0.5
                let face_d = Material.sd_box(p, c, hc, min(self.border_radius, min(hc.x, hc.y)))
                let pv = self.pivot()
                let rr = self.scale_radius()
                let q = p - pv
                let r = length(q)
                let ang = atan2(q.x, 0.0 - q.y)
                let sweep = self.sweep * 0.017453293
                let t = ang / sweep + 0.5
                let on_scale = step(-0.0001, t) * step(t, 1.0001)

                var col = self.color.rgb
                // A quiet diagonal sheen across the upper face, as glass.
                let band = clamp(1.0 - (p.x / max(self.rect_size.x, 1.0) + p.y / max(self.rect_size.y, 1.0) * 1.6), 0.0, 1.0)
                col = col + vec3(1.0, 1.0, 1.0) * self.sheen * smoothstep(0.35, 0.9, band)

                // The scale: a hairline arc and a tick at every division.
                let arc = Finish.cover(abs(r - rr) - px * 0.5, px) * on_scale
                let n = max(floor(self.majors + 0.5), 1.0) * max(floor(self.minors + 0.5), 1.0)
                let k = floor(clamp(t, 0.0, 1.0) * n + 0.5)
                let ak = (k / n - 0.5) * sweep
                let perp = abs(q.x * cos(ak) + q.y * sin(ak))
                let m = max(floor(self.minors + 0.5), 1.0)
                let major = 1.0 - step(0.5, k - m * floor(k / m + 0.0001))
                let len = mix(self.tick_minor, self.tick_major, major)
                let radial = max(rr - len - r, r - rr)
                // A minor tick one device pixel wide, a major one two.
                let tick = Finish.cover(perp - px * mix(0.5, 1.0, major), px) * Finish.cover(radial, px) * step(0.0, -q.y * cos(ak) + q.x * sin(ak))
                col = mix(col, self.color_scale.rgb, max(arc, tick))

                // The red zone: a band just outside the arc from `zone` on.
                if self.zone < 0.999 {
                    let zone_t = Finish.cover((self.zone - t) * sweep * r, px) * Finish.cover((t - 1.0) * sweep * r, px)
                    let zone_r = Finish.cover(max(rr + px - r, r - rr - 2.5), px)
                    col = mix(col, self.color_zone.rgb, zone_t * zone_r)
                }

                // The needle, one device pixel wide, from under the pivot
                // cover to just past the scale. Its tail stops a device
                // pixel short of the cover's edge, so no sliver of its
                // antialiasing shows beside the cover.
                let na = self.needle_angle(self.value)
                let nd = vec2(sin(na), -cos(na))
                let along = dot(q, nd)
                let across = abs(q.x * nd.y - q.y * nd.x)
                let needle = Finish.cover(across - px * 0.5, px) * Finish.cover(along - (rr + 3.0), px) * Finish.cover(-along - max(self.pivot_size - px, 0.0), px)
                col = mix(col, self.color_needle.rgb, needle)

                // The pivot cover over the needle's root, with a dark rim.
                let pd = r - self.pivot_size
                let cover_col = mix(self.color_needle.rgb, self.color.rgb, 0.35)
                col = mix(col, cover_col, Finish.cover(pd, px))
                col = mix(col, col * 0.55, Finish.band(pd, 0.0, px, px))

                // The face's own rim.
                col = mix(col, col * 0.6, Finish.band(face_d, 0.0, px, px))
                let a = Finish.cover(face_d, px) * self.color.a
                return vec4(col * a, a) * self.opacity
            }
        }
    }
}

/// The needle's angle for a value on a scale that sweeps `sweep_deg`
/// degrees: radians from straight up, clockwise, with the value held to the
/// scale. What the shader draws.
pub fn needle_angle(value: f64, sweep_deg: f64) -> f64 {
    (value.clamp(0.0, 1.0) - 0.5) * sweep_deg.to_radians()
}

/// The meter's shader: where the needle stands is the one instance that
/// moves, so meters batch.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNeedleMeter {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    value: f32,
    #[live]
    opacity: f32,
}

/// A needle meter.
#[derive(Script, ScriptHook, Widget)]
pub struct NeedleMeter {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawNeedleMeter,
    #[live]
    draw_label: DrawText,
    /// The glass over the face and its caption, drawn only while
    /// `field_reach` is above zero.
    #[live]
    draw_glass: DrawQuad,
    /// Where the needle is sent, 0 to 1 along the scale.
    #[live]
    pub value: f64,
    #[live(0.3)]
    pub ease_secs: f64,
    #[live]
    pub label: String,
    #[live]
    pub disabled: bool,
    /// How far past the face, in points, the pointer is in reach of the
    /// glass: above zero, `draw_glass` is drawn over the face and its
    /// caption and gets the pointer's field and the trail of drops the
    /// pointer leaves as it moves (`pointer_field`, `trail_0` to
    /// `trail_7` and `trail_life`, see [`PointerGlass`]). Zero, the
    /// default, draws no glass.
    #[live]
    pub field_reach: f64,
    /// How far the pointer moves between the trail's drops, in points.
    #[live(6.0)]
    pub trail_spacing: f64,
    /// How long a drop of the trail lives, in seconds: the frames stop when
    /// the newest drop is this old.
    #[live(1.3)]
    pub trail_life: f64,
    /// While the trail is alive, the glass is drawn over the face on an
    /// overlay with the window's scene of the frame bound (`has_gauss`,
    /// slot 0 `scene_texture`, `source_size`, `source_y_flip`), so its
    /// shader can read the face under it and bend it. At rest it is drawn
    /// in place with nothing bound. False, the default, never lifts it.
    #[live]
    pub glass_backdrop: bool,
    #[rust]
    pointer_glass: PointerGlass,
    #[rust]
    glide: Glide,
    #[rust]
    seeded: Option<f64>,
    #[rust]
    last_tick: f64,
    #[rust]
    running: bool,
    #[rust]
    next_frame: NextFrame,
}

impl NeedleMeter {
    /// Send the needle to `value`, 0 to 1 along the scale; it eases there.
    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        self.value = amount_of(value);
        self.head_for(cx);
    }

    /// Where the needle was sent.
    pub fn value(&self) -> f64 {
        amount_of(self.value)
    }

    /// Where the needle stands now, on its way there.
    pub fn shown(&self) -> f64 {
        self.glide.value() as f64
    }

    fn glass_setup(&self) -> PointerGlassSetup {
        PointerGlassSetup {
            reach: self.field_reach.max(0.0),
            spacing: self.trail_spacing,
            life: self.trail_life,
            backdrop: self.glass_backdrop,
        }
    }

    fn head_for(&mut self, cx: &mut Cx) {
        self.value = amount_of(self.value);
        self.seeded = Some(self.value);
        self.glide.set(self.value as f32);
        if !self.glide.is_settled() && !self.running {
            self.running = true;
            self.last_tick = cx.seconds_since_app_start();
            self.next_frame = cx.new_next_frame();
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for NeedleMeter {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // A script can write any number here; one that is no number is 0,
        // or it would never compare equal to itself and redraw for ever.
        self.value = amount_of(self.value);
        match self.seeded {
            None => {
                self.seeded = Some(self.value);
                self.glide.jump(self.value as f32);
            }
            Some(seen) if seen != self.value => self.head_for(cx),
            _ => {}
        }
        self.draw_bg.value = self.glide.value();
        let opacity = if self.disabled { 0.55 } else { 1.0 };
        self.draw_bg.opacity = opacity;
        self.draw_bg.begin(cx, walk, self.layout);
        if !self.label.is_empty() {
            // The caption dims with the face: set, draw, restore.
            let rest = self.draw_label.color;
            self.draw_label.color = Vec4f { w: rest.w * opacity, ..rest };
            self.draw_label.draw_walk(cx, Walk::fit(), Align::default(), &self.label);
            self.draw_label.color = rest;
        }
        self.draw_bg.end(cx);
        if self.field_reach > 0.0 {
            let rect = self.draw_bg.area().rect(cx);
            let mut radius = [0.0f32];
            self.draw_bg.get_uniform(cx, id!(border_radius), &mut radius);
            self.draw_glass.draw_vars.set_dyn_instance(cx, id!(face_radius), &radius);
            let setup = self.glass_setup();
            self.pointer_glass.draw(cx, &mut self.draw_glass, rect, &setup);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.field_reach > 0.0 {
            let setup = self.glass_setup();
            if self.pointer_glass.handle_event(cx, event, &mut self.draw_glass, &setup) {
                self.draw_bg.redraw(cx);
            }
        }
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = (ne.time - self.last_tick).max(0.0) as f32;
            self.last_tick = ne.time;
            self.glide.tick(dt, self.ease_secs as f32);
            self.draw_bg.redraw(cx);
            if self.glide.is_settled() {
                self.running = false;
            } else {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("{:.2}", self.value()))
    }
}

impl NeedleMeterRef {
    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ends of the scale are half the sweep either side of straight up,
    /// the middle is straight up, and a value off the scale is held to it.
    #[test]
    fn a_value_maps_to_its_needle_angle() {
        let sweep = 96.0;
        assert!((needle_angle(0.0, sweep) + 48f64.to_radians()).abs() < 1e-12);
        assert!((needle_angle(1.0, sweep) - 48f64.to_radians()).abs() < 1e-12);
        assert_eq!(needle_angle(0.5, sweep), 0.0);
        assert!((needle_angle(0.75, sweep) - 24f64.to_radians()).abs() < 1e-12);
        assert_eq!(needle_angle(1.4, sweep), needle_angle(1.0, sweep), "past the end is the end");
        assert_eq!(needle_angle(-3.0, sweep), needle_angle(0.0, sweep));
    }

    /// Set, the needle heads for the value and is not there yet; the value
    /// the meter reports is where it was sent.
    #[test]
    fn the_needle_eases_to_where_it_is_sent() {
        crate::on_test_cx(|| {
            let mut cx = crate::checkout_test_cx();
            let meter = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, { mod.widgets.NeedleMeter{value: 0.2} });
                WidgetRef::script_from_value(vm, value)
            });
            let mut inner = meter.borrow_mut::<NeedleMeter>().expect("a meter");
            inner.set_value(&mut cx, 0.9);
            assert_eq!(inner.value(), 0.9);
            inner.set_value(&mut cx, 7.0);
            assert_eq!(inner.value(), 1.0, "held to the scale");
            inner.glide.tick(0.01, 0.3);
            assert!(inner.shown() < 1.0, "it moves toward the value rather than jumping");
            for _ in 0..40 {
                inner.glide.tick(0.05, 0.3);
            }
            assert_eq!(inner.shown(), 1.0, "and lands");
            // A value that is no number is the left end, which the needle
            // can reach and stop at.
            inner.set_value(&mut cx, f64::NAN);
            assert_eq!(inner.value(), 0.0);
            for _ in 0..40 {
                inner.glide.tick(0.05, 0.3);
            }
            assert!(inner.glide.is_settled(), "the needle settles");
        });
    }
}
