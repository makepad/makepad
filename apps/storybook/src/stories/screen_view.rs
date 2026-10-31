//! The screen view pages under Containers: a view that is a display window,
//! with a bezel, a recessed face and glass over what it holds; and glass
//! that ripples like water where the pointer moves across it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let ScreenCaption = Label{
        draw_text +: {color: theme.color_screen_ink text_style: theme.font_regular{font_size: theme.font_size_p * 0.8}}
    }

    mod.stories.ScreenViewOverview = StoryPage{
        StoryNote{text: "A container that is a display: a slim bezel, a face set into it with the surround's shadow under its top edge, and a sheet of glass over whatever it holds. The children draw between the face and the glass, so the sheen lies over the digits, not only on the empty face around them."}
        StoryHeading{text: "One screen, under the controls"}
        StoryRow{
            subject := ScreenView{
                ScreenCaption{text: "OUTPUT"}
                Readout{text: "-6.0" cells: 5 digit_height: 30.}
                ScreenCaption{text: "dB"}
            }
        }

        StoryHeading{text: "What it holds"}
        StoryNote{text: "Anything a view holds: a readout with its caption, a level ladder, a line of text. Captions on a screen take the screen's own ink, a pale one on the dark glass of the dark theme and a dark one on the pale glass of the light theme."}
        StoryRow{
            ScreenView{
                flow: Right
                spacing: theme.space_2
                align: Align{y: 1.}
                Readout{text: "12:45" digit_height: 24.}
                ScreenCaption{text: "PM"}
            }
            ScreenView{
                width: 180.
                ScreenCaption{text: "INPUT L / R"}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.72 draw_bg.segment: 4.}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.58 draw_bg.segment: 4.}
            }
            ScreenView{
                width: 180.
                ScreenCaption{text: "READY"}
                ScreenCaption{text: "Take 3 of 12, 00:41 recorded"}
            }
        }

        StoryHeading{text: "Bezel, recess, sheen and scan lines"}
        StoryNote{text: "No bezel is a face cut straight into the panel; a wider one frames it. The recess is how far the surround's shadow reaches down the face. The sheen is quiet on purpose: a band of a few percent from the top left and a lighter line under the top edge. Scan lines are off unless a page or a sheet asks for them."}
        StoryRow{
            ScreenView{bezel: 0.0 Readout{text: "1.0" digit_height: 22.}}
            ScreenView{bezel: 5.0 Readout{text: "2.0" digit_height: 22.}}
            ScreenView{recess: 0.0 Readout{text: "3.0" digit_height: 22.}}
            ScreenView{recess: 9.0 Readout{text: "4.0" digit_height: 22.}}
            ScreenView{sheen: 0.0 Readout{text: "5.0" digit_height: 22.}}
            ScreenView{scan: 0.3 Readout{text: "6.0" digit_height: 22.}}
        }
    }

    // WATER UNDER THE POINTER: the page's own glass, so the ripples show
    // whatever sheet is on. Each drop of the pointer's trail sends a ring
    // out at 60 points a second, 20 points from crest to crest, landing
    // over its first 0.16 s and fading as it ages and grows. The glass reads
    // what lies under it displaced by up to about 4 points each way at a
    // fresh crest, and never folds the picture over itself: the rings are
    // trochoids, a broad crest that magnifies and a narrower trough that
    // squeezes, each ring's magnification is bounded, and where rings can
    // meet the newest takes its share of one budget first and the older
    // ones share the rest (shares that are the same over the whole glass).

    // A drop's strength: full a little after it lands, fading with what is
    // left of its life and as its ring grows (`d`: place and age, -1 none).
    let ripple_amp = fn(d: vec3, life: float) -> float {
        if d.z < 0.0 {
            return 0.0
        }
        let left = clamp(1.0 - d.z / max(life, 0.01), 0.0, 1.0)
        return 1.575 * left * sqrt(left) * smoothstep(0.0, 0.16, d.z) / (1.0 + d.z * 0.6667)
    }

    // One drop's ring at `p` as a displacement, for a drop of strength `a`:
    // a packet two crests deep round the front, its phase run through the
    // trochoid (Kepler's equation in three Newton steps), and nothing at
    // the drop's own centre.
    let ripple_ring = fn(p: vec2, d: vec3, a: float) -> vec2 {
        if a <= 0.0 {
            return vec2(0.0, 0.0)
        }
        let q = p - d.xy
        let r = max(length(q), 0.001)
        let u = r - d.z * 60.0
        let phi = u * 0.3141593
        var t = phi + 0.35 * sin(phi)
        t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
        t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
        t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
        let env = exp(0.0 - u * u / 196.0) * (1.0 - exp(0.0 - r * r / 36.0))
        return q * (a * env * sin(t) * 3.183099 / r)
    }

    // 1 where two rings' packets (31 points either side of their fronts)
    // can meet, falling to 0 over 6 points apart.
    let ripple_meet = fn(da: vec3, db: vec3) -> float {
        let dist = length(da.xy - db.xy)
        let fa = max(da.z, 0.0) * 60.0
        let fb = max(db.z, 0.0) * 60.0
        let gap = max(dist - (fa + fb + 61.6), max(max(fa - 30.8, 0.0) - fb - 30.8, max(fb - 30.8, 0.0) - fa - 30.8) - dist)
        return 1.0 - smoothstep(0.0, 6.0, gap)
    }

    // A drop's share of the magnification budget: what the newer drops it
    // can meet left of 0.88, over its own need `n`.
    let ripple_share = fn(used: float, n: float) -> float {
        return clamp((0.88 - used) / max(n, 0.00001), 0.0, 1.0)
    }

    // The water at `p`, the face's edge `face_d` away: `xy` the
    // displacement of what the glass reads, `zw` the slope the light
    // catches, both still over the last 12 points before the bezel.
    let ripple_sum = fn(p: vec2, face_d: float, life: float, d0: vec3, d1: vec3, d2: vec3, d3: vec3, d4: vec3, d5: vec3, d6: vec3, d7: vec3) -> vec4 {
        let a0 = ripple_amp(d0, life)
        let a1 = ripple_amp(d1, life)
        let a2 = ripple_amp(d2, life)
        let a3 = ripple_amp(d3, life)
        let a4 = ripple_amp(d4, life)
        let a5 = ripple_amp(d5, life)
        let a6 = ripple_amp(d6, life)
        let a7 = ripple_amp(d7, life)
        let s0 = ripple_share(0.0, a0 * 0.612)
        let t0 = s0 * a0 * 0.612
        let s1 = ripple_share(ripple_meet(d1, d0) * t0, a1 * 0.612)
        let t1 = s1 * a1 * 0.612
        let s2 = ripple_share(ripple_meet(d2, d0) * t0 + ripple_meet(d2, d1) * t1, a2 * 0.612)
        let t2 = s2 * a2 * 0.612
        let s3 = ripple_share(ripple_meet(d3, d0) * t0 + ripple_meet(d3, d1) * t1 + ripple_meet(d3, d2) * t2, a3 * 0.612)
        let t3 = s3 * a3 * 0.612
        let s4 = ripple_share(ripple_meet(d4, d0) * t0 + ripple_meet(d4, d1) * t1 + ripple_meet(d4, d2) * t2 + ripple_meet(d4, d3) * t3, a4 * 0.612)
        let t4 = s4 * a4 * 0.612
        let s5 = ripple_share(ripple_meet(d5, d0) * t0 + ripple_meet(d5, d1) * t1 + ripple_meet(d5, d2) * t2 + ripple_meet(d5, d3) * t3 + ripple_meet(d5, d4) * t4, a5 * 0.612)
        let t5 = s5 * a5 * 0.612
        let s6 = ripple_share(ripple_meet(d6, d0) * t0 + ripple_meet(d6, d1) * t1 + ripple_meet(d6, d2) * t2 + ripple_meet(d6, d3) * t3 + ripple_meet(d6, d4) * t4 + ripple_meet(d6, d5) * t5, a6 * 0.612)
        let t6 = s6 * a6 * 0.612
        let s7 = ripple_share(ripple_meet(d7, d0) * t0 + ripple_meet(d7, d1) * t1 + ripple_meet(d7, d2) * t2 + ripple_meet(d7, d3) * t3 + ripple_meet(d7, d4) * t4 + ripple_meet(d7, d5) * t5 + ripple_meet(d7, d6) * t6, a7 * 0.612)
        let w0 = ripple_ring(p, d0, a0)
        let w1 = ripple_ring(p, d1, a1)
        let w2 = ripple_ring(p, d2, a2)
        let w3 = ripple_ring(p, d3, a3)
        let w4 = ripple_ring(p, d4, a4)
        let w5 = ripple_ring(p, d5, a5)
        let w6 = ripple_ring(p, d6, a6)
        let w7 = ripple_ring(p, d7, a7)
        let disp = w0 * s0 + w1 * s1 + w2 * s2 + w3 * s3 + w4 * s4 + w5 * s5 + w6 * s6 + w7 * s7
        let slope = w0 + w1 + w2 + w3 + w4 + w5 + w6 + w7
        let keep = smoothstep(0.0, 12.0, 0.0 - face_d)
        return vec4(disp * keep, slope * keep)
    }

    // The glass over the water: `under` is what lies under it read along
    // the displacement (when `seen` is 1), the theme's own `sheen` over it,
    // light on the flanks that face the upper left and shade on the
    // others, from the `slope` (about 4 at a fresh crest); without the
    // scene, the sheen, light and shade alone, over what is drawn under the
    // glass.
    let ripple_lay = fn(under: vec3, seen: float, slope: vec2, sheen: float, cov: float) -> vec4 {
        let lit = dot(slope, vec2(-0.6, -0.8)) * 0.25
        let light = clamp(lit * 0.8, 0.0, 0.6)
        let shade = clamp(0.0 - lit * 0.5, 0.0, 0.45)
        if seen > 0.5 {
            var col = under * (1.0 - sheen) + vec3(sheen, sheen, sheen)
            col = mix(col, vec3(1.0, 1.0, 1.0), light)
            col = col * (1.0 - shade)
            return vec4(col * cov, cov)
        }
        var out = vec4(sheen, sheen, sheen, sheen)
        out = Finish.over(out, vec4(light, light, light, light))
        out = Finish.over(out, vec4(0.0, 0.0, 0.0, shade))
        return out * cov
    }

    let RippleScreen = ScreenView{
        field_reach: 2.0
        trail_life: 1.6
        glass_backdrop: true
        draw_glass +: {
            trail_0: instance(vec3(0.0, 0.0, -1.0))
            trail_1: instance(vec3(0.0, 0.0, -1.0))
            trail_2: instance(vec3(0.0, 0.0, -1.0))
            trail_3: instance(vec3(0.0, 0.0, -1.0))
            trail_4: instance(vec3(0.0, 0.0, -1.0))
            trail_5: instance(vec3(0.0, 0.0, -1.0))
            trail_6: instance(vec3(0.0, 0.0, -1.0))
            trail_7: instance(vec3(0.0, 0.0, -1.0))
            trail_life: uniform(1.6)
            scene_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)

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
                // The theme's quiet glass: the diagonal band and the line
                // under the top edge.
                let u = (p - (c - hf)) / max(hf * 2.0, vec2(1.0, 1.0))
                let diag = u.x + u.y * 0.6
                let band = smoothstep(0.0, 0.12, diag) * (1.0 - smoothstep(0.28, 0.5, diag))
                let top = Finish.cover(abs(p.y - (c.y - hf.y) - px * 1.5) - px * 0.5, px) * 0.6
                let sheen = clamp(self.sheen * (band + top), 0.0, 1.0)
                let w = ripple_sum(p, face_d, self.trail_life, self.trail_0, self.trail_1, self.trail_2, self.trail_3, self.trail_4, self.trail_5, self.trail_6, self.trail_7)
                var under = vec3(0.0, 0.0, 0.0)
                if self.has_gauss > 0.5 {
                    // The content under the glass, seen through the water:
                    // displaced, never from past the face.
                    let ps = clamp(p + w.xy, c - hf, c + hf)
                    let sq = (self.rect_pos + ps) / max(self.source_size, vec2(1.0, 1.0))
                    under = self.scene_texture.sample(vec2(sq.x, mix(sq.y, 1.0 - sq.y, self.source_y_flip))).rgb
                }
                return ripple_lay(under, self.has_gauss, w.zw, sheen, Finish.cover(face_d + px, px))
            }
        }
    }

    let RippleMeter = NeedleMeter{
        field_reach: 2.0
        trail_life: 1.6
        glass_backdrop: true
        draw_glass +: {
            trail_0: instance(vec3(0.0, 0.0, -1.0))
            trail_1: instance(vec3(0.0, 0.0, -1.0))
            trail_2: instance(vec3(0.0, 0.0, -1.0))
            trail_3: instance(vec3(0.0, 0.0, -1.0))
            trail_4: instance(vec3(0.0, 0.0, -1.0))
            trail_5: instance(vec3(0.0, 0.0, -1.0))
            trail_6: instance(vec3(0.0, 0.0, -1.0))
            trail_7: instance(vec3(0.0, 0.0, -1.0))
            trail_life: uniform(1.6)
            scene_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                // Two points inside the face, so its rim stays still
                // whatever sheet drew it.
                let hf = max(c - vec2(2.0, 2.0), vec2(0.5, 0.5))
                let face_d = Material.sd_box(p, c, hf, max(min(self.face_radius, min(c.x, c.y)) - 2.0, 0.0))
                let w = ripple_sum(p, face_d, self.trail_life, self.trail_0, self.trail_1, self.trail_2, self.trail_3, self.trail_4, self.trail_5, self.trail_6, self.trail_7)
                var under = vec3(0.0, 0.0, 0.0)
                if self.has_gauss > 0.5 {
                    // The content under the glass, seen through the water:
                    // displaced, never from past the face.
                    let ps = clamp(p + w.xy, c - hf, c + hf)
                    let sq = (self.rect_pos + ps) / max(self.source_size, vec2(1.0, 1.0))
                    under = self.scene_texture.sample(vec2(sq.x, mix(sq.y, 1.0 - sq.y, self.source_y_flip))).rgb
                }
                return ripple_lay(under, self.has_gauss, w.zw, 0.0, Finish.cover(face_d + px, px))
            }
        }
    }

    mod.stories.ScreenViewRipples = StoryPage{
        StoryNote{text: "Glass that answers the pointer the way water does. Move the pointer across a screen or the meter: wherever it passes it leaves drops, each drop sends a ring out across the glass, and what lies under the glass, the digits, the needle and the scale, bends through the rings and settles again. Stop moving and the water stills; nothing is drawn or asked for until the pointer moves again."}
        StoryHeading{text: "Move the pointer across the glass"}
        StoryRow{
            subject := RippleScreen{
                ScreenCaption{text: "OUTPUT"}
                Readout{text: "-6.0" cells: 5 digit_height: 30.}
                ScreenCaption{text: "dB"}
            }
            RippleMeter{width: 200. height: 120. value: 0.62 label: "VU"}
        }

        StoryHeading{text: "Small screens, side by side"}
        StoryNote{text: "Each screen keeps its own water: a ring stays inside the glass it was dropped in and dies out before the bezel, which stays still."}
        StoryRow{
            RippleScreen{flow: Right spacing: theme.space_2 align: Align{y: 1.} Readout{text: "12:45" digit_height: 24.} ScreenCaption{text: "PM"}}
            RippleScreen{flow: Right spacing: theme.space_2 align: Align{y: 1.} Readout{text: "440.0" cells: 6 digit_height: 24.} ScreenCaption{text: "Hz"}}
            RippleMeter{value: 0.35 label: "LOAD"}
        }

        StoryHeading{text: "Rings without the backdrop"}
        StoryNote{text: "With glass_backdrop off the glass cannot read what lies under it, so the rings are light and shade on the glass alone and the digits stay put. Nothing is captured for it."}
        StoryRow{
            RippleScreen{glass_backdrop: false Readout{text: "-12.0" cells: 5 digit_height: 30.}}
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/screenview/overview",
        category: "Containers",
        component: "ScreenView",
        also: &[],
        name: "Overview",
        dsl: "ScreenViewOverview",
        added: "2026-09-28",
        tags: &["instruments", "new"],
        doc: "# ScreenView\n\nA view that is a display window: a slim bezel, the screen face recessed into it with an inner shadow under its top edge, and glass over the children it holds. The children draw between the face and the glass, so the sheen lies over them.\n\n- `bezel` is the ring's width in points; `recess` how far the surround's shadow reaches down the face; `sheen` the glass's strength; `scan` the depth of scan lines, off by default.\n- The face is `color_screen`, the theme's display glass. Put `Readout`s, labels and meters inside; a caption on a screen reads best in `color_screen_ink`.\n- Everything else is a view's: `flow`, `spacing`, `padding`, `align`, `width`, `height`.\n\nThe face is the view's `draw_bg` and the glass `draw_glass`; both read `bezel` (and the face `recess` and `scan`, the glass `sheen`) as instances the widget keeps in step with its properties, so a sheet may replace either pixel function and still draw to the page's geometry. The glass is one quad drawn after the children, inside the widget's own turtle.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Bezel", target: "subject", kind: ControlKind::Number { prop: "bezel", min: 0., max: 12., step: 0.5, default: 2. } },
            Control { label: "Recess", target: "subject", kind: ControlKind::Number { prop: "recess", min: 0., max: 12., step: 0.5, default: 4. } },
            Control { label: "Sheen", target: "subject", kind: ControlKind::Number { prop: "sheen", min: 0., max: 0.12, step: 0.005, default: 0.03 } },
            Control { label: "Scan lines", target: "subject", kind: ControlKind::Number { prop: "scan", min: 0., max: 0.5, step: 0.01, default: 0. } },
            Control { label: "Corner", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2. } },
        ],
        on_actions: None,
    },
    Story {
        key: "containers/screenview/ripples",
        category: "Containers",
        component: "ScreenView",
        also: &["NeedleMeter"],
        name: "Ripples",
        dsl: "ScreenViewRipples",
        added: "2026-10-10",
        tags: &["instruments", "pointer", "water", "glass", "new"],
        doc: "# Ripples

Glass that rings like water where the pointer moves across it, and bends what lies under it.

- `field_reach` above zero gives a `ScreenView`'s or a `NeedleMeter`'s glass the pointer: its field and the trail of drops it leaves as it moves, handed to `draw_glass` as instances. `pointer_field` is the field's strength, the pointer's place and a clock; `trail_0` to `trail_7` are the last eight drops, newest first, each its place in points from the glass's top left, its age in seconds and what is left of its life, 1 down to 0.
- `trail_spacing` is how far the pointer moves between drops, `trail_life` how long a drop lives. A pointer that does not move drops nothing.
- `glass_backdrop` lifts the glass over the content while the trail is alive, with the window's scene of the frame bound (`scene_texture`, `source_size`, `source_y_flip`, `has_gauss`), so its shader can read the digits or the needle under it and draw them displaced.
- At rest nothing runs: no frames, no capture, and the glass is drawn where it always is. A meter draws its glass only while `field_reach` is above zero.

The shaders on this page are the page's own, so the ripples show under every sheet: each drop sends a ring outward, the slope of the water displaces what the glass reads, and the flanks facing the light catch it.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Reach", target: "subject", kind: ControlKind::Number { prop: "field_reach", min: 0., max: 24., step: 0.5, default: 2. } },
            Control { label: "Drop spacing", target: "subject", kind: ControlKind::Number { prop: "trail_spacing", min: 1., max: 40., step: 0.5, default: 6. } },
            Control { label: "Drop life (s)", target: "subject", kind: ControlKind::Number { prop: "trail_life", min: 0.2, max: 4., step: 0.05, default: 1.3 } },
        ],
        on_actions: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page builds, both of the screen's shaders compile, and the
    /// subject is a screen.
    #[test]
    fn the_page_builds_and_its_subject_is_a_screen() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.value(stories, LiveId::from_str(story.dsl));
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a shader on the page failed to compile");
        assert!(page.widget(&cx, ids!(subject)).borrow::<ScreenView>().is_some(), "no screen at subject");
    }
}
