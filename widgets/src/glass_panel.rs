use crate::{
    animator::Animate,
    gauss_view::{arm_gauss_capture, request_window_gauss, GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    overlay_place::span_inboard,
    view::View,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncResult},
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.glass = {}

    mod.widgets.glass.LayerBase = #(GlassLayer::register_widget(vm))
    mod.widgets.glass.GlassRadioBase = #(GlassRadio::register_widget(vm))
    mod.widgets.glass.GlassButtonBase = #(GlassButton::register_widget(vm))
    mod.widgets.glass.GlassSliderBase = #(GlassSlider::register_widget(vm))
    mod.widgets.glass.GlassSegmentedBase = #(GlassSegmented::register_widget(vm))
    mod.widgets.glass.FloatingSurfaceBase = #(GlassFloatingSurface::register_widget(vm))

    mod.widgets.glass.Layer = mod.widgets.glass.LayerBase{
        width: Fill
        height: Fill
        flow: Overlay
        align: Align{x: 0.0 y: 0.0}

        draw_bg +: {
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }
    }

    mod.widgets.glass.GlassRadio = set_type_default() do mod.widgets.glass.GlassRadioBase{
        width: 70
        height: 34
        flow: Overlay

        draw_slot +: {
            active: uniform(0.0)
            hover: uniform(0.0)
            down: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let active = self.active
                let w = self.rect_size.x
                let h = self.rect_size.y
                // Track + nub are SHORTER than the full switch height (centred vertically), so more
                // of the glass lens shows around them. Visual corner radius is 2*r -> r = height/4.
                let track_h = h * 0.66
                let track_y = (h - track_h) * 0.5
                let r = track_h * 0.25

                // Nub metrics first, so the track can be inset to match the nub's narrowed sides.
                // `travel_w` (the original full nub width) is the basis the LENS travels on; the
                // nub's CENTRE rides that same basis so the lens stays aligned over it, while
                // `knob_w` is sized so its left/right glass gap matches the top/bottom gap.
                let kpad = 3.0
                let knob_h = track_h - 4.0
                let travel_w = (h - kpad * 2.0) * 1.35
                let knob_w = travel_w + 6.0 - h + knob_h
                // Inset the track to the nub's outer travel edge so the green/gray track sides line
                // up with the narrowed nub instead of running to the full switch width.
                let track_xpad = kpad + (travel_w - knob_w) * 0.5

                // Track capsule: medium gray when off, accent green when on. Each element
                // ends with `sdf.fill` (not fill_keep) which RESETS the shape, otherwise the
                // following boxes union into it and the knob fill would paint everything.
                sdf.box(track_xpad, track_y, w - track_xpad * 2.0, track_h, r)
                let off_color = vec4(0.46, 0.48, 0.51, 1.0)
                let on_color = vec4(0.27, 0.80, 0.33, 1.0)
                sdf.fill(off_color.mix(on_color, active))

                // White rounded nub - short and narrower (even glass padding on all sides).
                let knob_cx = mix(kpad + travel_w * 0.5, w - kpad - travel_w * 0.5, active)
                let knob_x = knob_cx - knob_w * 0.5
                let knob_y = (h - knob_h) * 0.5
                let knob_r = knob_h * 0.22

                // Soft drop shadow under the knob.
                sdf.box(knob_x - 0.5, knob_y + 1.5, knob_w + 1.0, knob_h + 1.0, knob_r)
                sdf.fill(vec4(0.0, 0.0, 0.0, 0.18))

                // Clean white knob with a gentle top-down shade.
                sdf.box(knob_x, knob_y, knob_w, knob_h, knob_r)
                let ky = smoothstep(0.0, 1.0, self.pos.y)
                let knob_col = vec3(1.0, 1.0, 1.0).mix(vec3(0.90, 0.91, 0.94), ky * 0.28)
                sdf.fill(vec4(knob_col, 1.0))
                return sdf.result
            }
        }

        draw_knob +: {
            scene_texture: texture_2d(float)
            mip0_texture: texture_2d(float)
            mip1_texture: texture_2d(float)
            mip2_texture: texture_2d(float)
            mip3_texture: texture_2d(float)
            mip4_texture: texture_2d(float)
            mip5_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)
            active: uniform(0.0)
            hover: uniform(0.0)
            down: uniform(0.0)

            sample_blur: fn(uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                // Keep it fairly sharp so the magnified green reads as glass, not frost.
                return self.scene_texture.sample_as_bgra(safe_uv) * 0.55
                    + self.mip0_texture.sample_as_bgra(safe_uv) * 0.30
                    + self.mip1_texture.sample_as_bgra(safe_uv) * 0.15
            }

            // Re-create the switch underneath (track capsule + sliding knob) so the lens can refract
            // the ACTUAL switch in-shader, not just the blurred window behind it. `p` is in the
            // switch's local pixel space. This mirrors `draw_slot`'s geometry exactly.
            slot_color: fn(p: vec2) -> vec3 {
                let w = self.rect_size.x
                let h = self.rect_size.y
                // Must mirror draw_slot's SHORTER + inset track and narrower nub exactly.
                let track_h = h * 0.66
                let r = track_h * 0.25
                let kpad = 3.0
                let knob_h = track_h - 4.0
                let travel_w = (h - kpad * 2.0) * 1.35
                let knob_w = travel_w + 6.0 - h + knob_h
                let track_xpad = kpad + (travel_w - knob_w) * 0.5
                // rounded-box distance to the (short, inset) track capsule (inlined; no nested fns)
                let tq = abs(p - vec2(w * 0.5, h * 0.5)) - vec2((w - track_xpad * 2.0) * 0.5, track_h * 0.5) + vec2(r, r)
                let d_track = min(max(tq.x, tq.y), 0.0) + length(max(tq, vec2(0.0, 0.0))) - r
                let off_color = vec3(0.46, 0.48, 0.51)
                let on_color = vec3(0.27, 0.80, 0.33)
                let behind = vec3(0.16, 0.20, 0.30)
                var col = behind.mix(off_color.mix(on_color, self.active), smoothstep(0.7, -0.7, d_track))
                // sliding white knob (short, narrower, centred on the full-width travel basis)
                let knob_cx = mix(kpad + travel_w * 0.5, w - kpad - travel_w * 0.5, self.active)
                let knob_x = knob_cx - knob_w * 0.5
                let knob_y = (h - knob_h) * 0.5
                let knob_r = knob_h * 0.22
                let kq = abs(p - vec2(knob_x + knob_w * 0.5, knob_y + knob_h * 0.5)) - vec2(knob_w * 0.5, knob_h * 0.5) + vec2(knob_r, knob_r)
                let d_knob = min(max(kq.x, kq.y), 0.0) + length(max(kq, vec2(0.0, 0.0))) - knob_r
                let ky = clamp(p.y / max(h, 1.0), 0.0, 1.0)
                let knob_col = vec3(1.0, 1.0, 1.0).mix(vec3(0.90, 0.91, 0.94), ky * 0.28)
                return col.mix(knob_col, smoothstep(0.7, -0.7, d_knob))
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let active = self.active
                let w = self.rect_size.x
                let h = self.rect_size.y

                // Full-switch-size overlay: the lens nub position is driven entirely by
                // `active` in-shader, so it glides (and gloops) from one side to the other in
                // lockstep with the knob underneath.
                let kpad = 3.0
                let knob_h = h - kpad * 2.0
                let knob_w = knob_h * 1.35
                // Gloop: stretch the blob horizontally during the crossing (max at active=0.5),
                // round at the ends - a liquid squash/stretch.
                let gloop = sin(active * 3.14159265)
                let lens_h = knob_h + 6.0 - gloop * 2.0
                let lens_w = knob_w + 6.0 + gloop * knob_w * 0.55
                let lens_cx = mix(kpad + knob_w * 0.5, w - kpad - knob_w * 0.5, active)
                let lens_x = lens_cx - lens_w * 0.5
                let lens_y = (h - lens_h) * 0.5
                sdf.box(lens_x, lens_y, lens_w, lens_h, lens_h * 0.25)

                let shape = sdf.shape
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let glen = length(gradient)
                var normal = vec2(0.0, 1.0)
                if glen > 0.0001 {
                    normal = gradient / glen
                }

                // Refract the switch DIRECTLY in-shader. The interior gently magnifies what's under
                // the lens (so the knob reads as enlarged through glass) and the rim bends the track
                // in along the normal - then we read the recreated switch at that warped position.
                let rim = clamp(1.0 - abs(shape) / 9.0, 0.0, 1.0)
                let bend = rim * rim
                let local_p = self.pos * self.rect_size
                let lens_center = vec2(lens_cx, h * 0.5)
                let warped = local_p + (lens_center - local_p) * (0.20 * (1.0 - bend)) + normal * (bend * 9.0)
                let chroma = normal * (bend * 3.0)
                let switch_col = vec3(
                    self.slot_color(warped + chroma).x,
                    self.slot_color(warped).y,
                    self.slot_color(warped - chroma).z
                )

                // Faint window-backdrop blend so it still reads as real glass (and keeps the gauss
                // textures live), but the switch underneath stays clearly visible.
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let disp = normal * (bend * 11.0) / max(self.source_size, vec2(1.0, 1.0))
                let win = self.sample_blur(clamp(uv + disp, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let base = switch_col.mix(vec3(win.x, win.y, win.z), self.has_gauss * 0.14)

                // Light frost so the switch stays visible through the glass.
                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let material = base.mix(vec3(1.0, 1.0, 1.0), 0.12 + top * 0.10)
                sdf.fill_keep(vec4(material, 1.0))

                // Bright specular crescent on the light-facing (upper-right) rim.
                let light_dir = normalize(vec2(0.50, -0.86))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let edgeband = clamp(1.0 - abs(shape) / 2.6, 0.0, 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * edgeband * (0.45 + self.hover * 0.10)))
                // Faint full edge to seal the glass.
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.16), 0.8)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.GlassButton = set_type_default() do mod.widgets.glass.GlassButtonBase{
        width: Fit
        height: 44
        padding: Inset{left: 22, right: 22, top: 0, bottom: 0}
        align: Align{x: 0.5, y: 0.5}
        spacing: 7
        label_walk: Walk{width: Fit, height: Fit}
        // No icon unless one is given; then it sizes to its own drawing.
        icon_walk: Walk{width: Fit, height: Fit}

        draw_text +: {
            color: #xffffffff
            text_style: theme.font_bold{font_size: 13}
        }

        // Transparent base: nothing is captured here, so the glass overlay refracts the
        // real background (clean glass) instead of muddying a semi-transparent fill.
        draw_bg +: {
            hover: uniform(0.0)
            down: uniform(0.0)
            press: uniform(0.0)
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }

        draw_glass +: {
            /** The face it shows before there is a scene to refract. The
             * surfaces it stands next to fall back to the same colour, and
             * they used to disagree: pale white here, dark slate there, side
             * by side on the same unready frame. Declared per shader because
             * these three are hand-written quads rather than derivatives of
             * the rounded surface, so there is no shared place to put it. */
            fallback_color: uniform(mix(theme.color_bg_app, theme.color_text, 0.30))
            scene_texture: texture_2d(float)
            mip0_texture: texture_2d(float)
            mip1_texture: texture_2d(float)
            mip2_texture: texture_2d(float)
            mip3_texture: texture_2d(float)
            mip4_texture: texture_2d(float)
            mip5_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)
            hover: uniform(0.0)
            down: uniform(0.0)
            press: uniform(0.0)
            tint: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            // Press response (ported from the examples/splash focus lens). `press_flatten` is the
            // smoothstep flatten amount (>0 while pressing, <0 = un-flatten/restore on release).
            // ripple_age / ripple_strength drive a single flattening WAVE that sweeps the lens, and
            // the refraction is RGB-split for chromatic diffraction. `press` is a separate smooth
            // shrink amount (the primary press indicator).
            press_flatten: uniform(0.0)
            ripple_age: uniform(1000.0)
            ripple_strength: uniform(0.0)
            diffraction_strength: uniform(4.0)
            // Lens corner radius (visual radius is 2x this, Sdf2d convention).
            corner_radius: uniform(9.0)

            // Frosted sample (weight the blurred mips) so the button reads as glass and a
            // hard background line doesn't show as a sharp dark bar behind the label.
            sample_blur: fn(uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                return self.mip1_texture.sample_as_bgra(safe_uv) * 0.46
                    + self.mip2_texture.sample_as_bgra(safe_uv) * 0.34
                    + self.mip0_texture.sample_as_bgra(safe_uv) * 0.20
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                // NOTE: Sdf2d.box renders a VISUAL corner radius of 2*r.
                let r = self.corner_radius
                // Smooth shrink is the primary press indicator (eased in Rust, no jiggle).
                let shrink = clamp(self.press, 0.0, 1.0)
                let ins = 2.0 + shrink * 2.6
                sdf.box(ins, ins, w - ins * 2.0, h - ins * 2.0, r)

                let shape = sdf.shape
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let src = max(self.source_size, vec2(1.0, 1.0))
                // Rounded box has a flat interior (gradient 0 -> normalize = NaN); branch explicitly.
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let glen = length(gradient)
                var normal = vec2(0.0, 1.0)
                if glen > 0.0001 {
                    normal = gradient / glen
                }

                // Flattening WAVE (examples/splash focus lens). An expanding Gaussian ring whose
                // crest both ripples the surface and FLATTENS the lens as it passes - on release
                // (press_flatten < 0) it un-flattens the same way. No spring, so no jiggle.
                let lens_pos = self.pos * 2.0 - 1.0
                let ripple_dist = length(lens_pos)
                let ripple_age = max(self.ripple_age, 0.0)
                let ripple_life = clamp(1.0 - ripple_age / 1.30, 0.0, 1.0)
                let wave_t = clamp(ripple_age / 1.05, 0.0, 1.0)
                let wave_center = mix(0.0, 1.25, wave_t * wave_t * (3.0 - 2.0 * wave_t))
                let wave_width = 0.24
                let wave_delta = ripple_dist - wave_center
                let wave = exp(-(wave_delta * wave_delta) / (wave_width * wave_width))
                let wave_mask = smoothstep(0.0, 0.10, ripple_age) * (1.0 - smoothstep(1.16, 1.44, ripple_dist))
                let ripple_wave = wave * ripple_life * ripple_life * self.ripple_strength * wave_mask
                let ripple_slope = (-wave_delta / wave_width) * ripple_wave
                let ripple_dir = lens_pos / max(ripple_dist, 0.001)
                let press = clamp(self.press_flatten, 0.0, 1.0)
                let restore = clamp(-self.press_flatten, 0.0, 1.0)
                let wave_flatten = smoothstep(ripple_dist - 0.14, ripple_dist + 0.26, wave_center)
                let flatten = clamp(press * wave_flatten + restore * (1.0 - wave_flatten), 0.0, 1.0)
                let lift = restore * wave_flatten * (1.0 - wave_t) * 0.45
                let ripple_surface = ripple_slope * 1.35 + ripple_wave * 0.34
                let lens_depth = clamp(1.0 - flatten * 0.90 + lift * 0.55 + ripple_wave * 0.45, 0.0, 1.85)
                let diffraction_depth = clamp(1.0 - flatten * 0.76 + lift * 0.70 + (abs(ripple_surface) + ripple_wave) * 1.6, 0.0, 2.60)

                // Edge lens + RGB-split diffraction. The base offset bends the background at the rim
                // (scaled by the flattening wave); the colour offset samples R/G/B at slightly
                // different positions for the chromatic splice. The click-ripple displacement is
                // cranked WAY up here to try out a much stronger lensing pulse.
                let rim = clamp(1.0 - abs(shape) / 13.0, 0.0, 1.0)
                let lens = rim * rim * lens_depth
                let water_offset = ripple_dir * (ripple_surface * 85.0) / src
                let base_offset = normal * (lens * 18.0) / src + water_offset
                let color_offset = normal * (lens * self.diffraction_strength * diffraction_depth) / src
                    + ripple_dir * ((ripple_surface + ripple_wave * 0.65) * self.diffraction_strength * 14.0) / src
                let uv_g = clamp(uv + base_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_r = clamp(uv_g + color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_b = clamp(uv_g - color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(uv_r)
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(uv_b)
                let refracted = vec3(s_r.x, s_g.y, s_b.z)
                let fallback = self.fallback_color.rgb
                let base = fallback.mix(refracted, self.has_gauss)

                // Fully OPAQUE glass - the "transparent" look comes only from the refraction lookup.
                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let frost = base.mix(vec3(1.0, 1.0, 1.0), 0.06 + top * 0.08 + self.hover * 0.04)
                let material = frost.mix(self.tint.rgb, self.tint.a)
                sdf.fill_keep(vec4(material, 1.0))

                // Bright specular crescent on the upper-right rim + a crest highlight on the wave.
                let light_dir = normalize(vec2(0.5, -0.86))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let edgeband = clamp(1.0 - abs(shape) / 2.6, 0.0, 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * edgeband * (0.50 + self.hover * 0.12) + ripple_wave * 0.11))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.18 + self.hover * 0.10), 0.9)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.GlassButtonProminent = mod.widgets.glass.GlassButton{
        draw_glass +: {
            tint: uniform(vec4(0.16, 0.46, 0.92, 0.34))
        }
    }

    mod.widgets.glass.GlassSlider = set_type_default() do mod.widgets.glass.GlassSliderBase{
        width: Fill
        height: 32
        value: 0.4

        draw_track +: {
            value: uniform(0.0)
            hover: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let th = 6.0
                let ty = (h - th) * 0.5
                let r = th * 0.25
                sdf.box(2.0, ty, w - 4.0, th, r)
                sdf.fill(vec4(0.42, 0.45, 0.48, 1.0))
                let fw = (w - 4.0) * self.value
                sdf.box(2.0, ty, fw, th, r)
                sdf.fill(vec4(0.20, 0.80, 0.34, 1.0))
                return sdf.result
            }
        }

        draw_knob +: {
            /** The face it shows before there is a scene to refract. The
             * surfaces it stands next to fall back to the same colour, and
             * they used to disagree: pale white here, dark slate there, side
             * by side on the same unready frame. Declared per shader because
             * these three are hand-written quads rather than derivatives of
             * the rounded surface, so there is no shared place to put it. */
            fallback_color: uniform(mix(theme.color_bg_app, theme.color_text, 0.30))
            scene_texture: texture_2d(float)
            mip0_texture: texture_2d(float)
            mip1_texture: texture_2d(float)
            mip2_texture: texture_2d(float)
            mip3_texture: texture_2d(float)
            mip4_texture: texture_2d(float)
            mip5_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)
            value: uniform(0.0)
            hover: uniform(0.0)

            sample_blur: fn(uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                return self.mip1_texture.sample_as_bgra(safe_uv) * 0.46
                    + self.mip2_texture.sample_as_bgra(safe_uv) * 0.34
                    + self.mip0_texture.sample_as_bgra(safe_uv) * 0.20
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let kd = h - 4.0
                let kx = (w - kd) * self.value
                let ky = (h - kd) * 0.5
                sdf.box(kx, ky, kd, kd, kd * 0.25)

                let shape = sdf.shape
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let glen = length(gradient)
                var normal = vec2(0.0, 1.0)
                if glen > 0.0001 {
                    normal = gradient / glen
                }

                let rim = clamp(1.0 - abs(shape) / 11.0, 0.0, 1.0)
                let bend = rim * rim
                let disp = normal * (bend * 12.0) / max(self.source_size, vec2(1.0, 1.0))
                let chroma = normal * (bend * 3.5) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + disp, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(clamp(uv_g + chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(clamp(uv_g - chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let refracted = vec3(s_r.r, s_g.g, s_b.b)
                let fallback = self.fallback_color.rgb
                let base = fallback.mix(refracted, self.has_gauss)

                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let material = base.mix(vec3(1.0, 1.0, 1.0), 0.30 + top * 0.14)
                sdf.fill_keep(vec4(material, 1.0))

                let light_dir = normalize(vec2(0.5, -0.86))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let edgeband = clamp(1.0 - abs(shape) / 2.6, 0.0, 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * edgeband * (0.5 + self.hover * 0.1)))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.2), 0.9)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.GlassSegmented = set_type_default() do mod.widgets.glass.GlassSegmentedBase{
        width: Fill
        height: 38
        flow: Right
        align: Align{x: 0.5, y: 0.5}
        labels: ["One", "Two", "Three"]

        draw_text +: {
            color: #xffffffff
            text_style: theme.font_bold{font_size: 12, line_spacing: 1.0}
        }

        draw_bg +: {
            sel_pos: uniform(0.0)
            count: uniform(1.0)
            hover: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let r = h * 0.22
                sdf.box(1.0, 1.0, w - 2.0, h - 2.0, r)
                sdf.fill(vec4(0.10, 0.13, 0.18, 0.60))
                return sdf.result
            }
        }

        draw_sel +: {
            /** The face it shows before there is a scene to refract. The
             * surfaces it stands next to fall back to the same colour, and
             * they used to disagree: pale white here, dark slate there, side
             * by side on the same unready frame. Declared per shader because
             * these three are hand-written quads rather than derivatives of
             * the rounded surface, so there is no shared place to put it. */
            fallback_color: uniform(mix(theme.color_bg_app, theme.color_text, 0.30))
            scene_texture: texture_2d(float)
            mip0_texture: texture_2d(float)
            mip1_texture: texture_2d(float)
            mip2_texture: texture_2d(float)
            mip3_texture: texture_2d(float)
            mip4_texture: texture_2d(float)
            mip5_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)
            sel_pos: uniform(0.0)
            count: uniform(1.0)
            hover: uniform(0.0)
            pill_x: uniform(0.0)
            pill_w: uniform(0.0)

            sample_blur: fn(uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                return self.mip1_texture.sample_as_bgra(safe_uv) * 0.46
                    + self.mip2_texture.sample_as_bgra(safe_uv) * 0.34
                    + self.mip0_texture.sample_as_bgra(safe_uv) * 0.20
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let pad = 3.0
                // The pill's x/width come from Rust: segments are sized to their
                // own labels, so they can't be derived from a segment count here.
                // The travelling "gloop" stretch is already folded in.
                let pill_x = self.pill_x
                let pill_w = self.pill_w
                let pill_y = pad
                let pill_h = h - pad * 2.0
                let r = pill_h * 0.25
                sdf.box(pill_x, pill_y, pill_w, pill_h, r)

                let shape = sdf.shape
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let glen = length(gradient)
                var normal = vec2(0.0, 1.0)
                if glen > 0.0001 {
                    normal = gradient / glen
                }

                let rim = clamp(1.0 - abs(shape) / 12.0, 0.0, 1.0)
                let bend = rim * rim
                let disp = normal * (bend * 12.0) / max(self.source_size, vec2(1.0, 1.0))
                let chroma = normal * (bend * 3.5) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + disp, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(clamp(uv_g + chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(clamp(uv_g - chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let refracted = vec3(s_r.r, s_g.g, s_b.b)
                let fallback = self.fallback_color.rgb
                let base = fallback.mix(refracted, self.has_gauss)

                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let material = base.mix(vec3(1.0, 1.0, 1.0), 0.16 + top * 0.10)
                sdf.fill_keep(vec4(material, 1.0))

                let light_dir = normalize(vec2(0.5, -0.86))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let edgeband = clamp(1.0 - abs(shape) / 2.6, 0.0, 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * edgeband * (0.45 + self.hover * 0.1)))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.18), 0.9)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.Panel = mod.widgets.LensedRoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 12
        padding: 16
        clip_x: false
        clip_y: false
        draw_bg +: {
            blur_level: 5.2
            lensing_effect: 0.94
            lensing_strength: 28.0
            lensing_width: 20.0
            corner_radius: 10.0
            tint_color: #xf8fbff
            // Settled by eye in all three themes: below this the panel reads
            // as a smear of its ground rather than a surface on it; from
            // about 0.15 it goes milky under white text.
            tint_alpha: 0.08
            surface_alpha: 1.0
            border_alpha: 0.72
            border_width: 1.0
            specular_strength: 0.22
            noise_strength: 0.004
            shadow_color: #x0007
            shadow_radius: 13.0
            shadow_offset: vec2(0.0, 5.0)
            diffraction_strength: 4.4
        }
    }

    mod.widgets.glass.ClearPanel = mod.widgets.glass.Panel{
        draw_bg +: {
            blur_level: 5.4
            lensing_effect: 1.0
            lensing_strength: 34.0
            lensing_width: 18.0
            tint_alpha: 0.004
            surface_alpha: 1.0
            border_alpha: 0.84
            specular_strength: 0.28
            shadow_color: #x0005
            diffraction_strength: 5.4
        }
    }

    mod.widgets.glass.NavBar = mod.widgets.glass.ClearPanel{
        height: 58
        flow: Right
        spacing: 8
        padding: Inset{left: 8, right: 8, top: 8, bottom: 8}
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            corner_radius: 12.0
            blur_level: 5.2
            lensing_effect: 1.0
            lensing_strength: 36.0
            lensing_width: 18.0
            shadow_radius: 12.0
            shadow_offset: vec2(0.0, 4.0)
        }
    }

    mod.widgets.glass.TabBar = mod.widgets.glass.NavBar{
        height: 66
        spacing: 4
        padding: Inset{left: 8, right: 8, top: 8, bottom: 8}
        draw_bg +: {
            corner_radius: 12.0
        }
    }

    mod.widgets.glass.Group = mod.widgets.glass.Panel{
        spacing: 10
        padding: 12
        draw_bg +: {
            corner_radius: 10.0
            blur_level: 5.0
            lensing_effect: 0.88
            lensing_strength: 26.0
            lensing_width: 18.0
            tint_alpha: 0.004
            surface_alpha: 1.0
            border_alpha: 0.60
            shadow_radius: 10.0
            shadow_offset: vec2(0.0, 3.0)
        }
    }

    mod.widgets.glass.Card = mod.widgets.glass.Group{
        padding: 14
        draw_bg +: {
            // A step above Panel's 0.08, as it was a step above Panel before:
            // the card is the family's more solid member.
            tint_alpha: 0.10
            lensing_effect: 0.34
        }
    }

    /** A sheet of glass that floats over the page: moved by its body, sized
     * by its edges and its corners. */
    mod.widgets.glass.FloatingSurface = set_type_default() do mod.widgets.glass.FloatingSurfaceBase{
        flow: Overlay
        /** where it opens, in window points */
        pos: vec2(120., 120.)
        /** how big it opens */
        size: vec2(320., 220.)
        /** it never gets smaller than this */
        min_size: vec2(140., 96.)
        /** nor bigger; a zero side means the window is the only ceiling */
        max_size: vec2(0., 0.)
        /** how far either side of an edge a press still takes hold of it,
         * in points; also the width of the unmarked ring of page outside
         * the glass that answers to this surface, which is why it stops
         * at 16 2..16 step 1 */
        grab_margin: 8.
        /** the corner mark's side, in points; 0 draws none 0..48 step 1 */
        grip_size: 24.
        /** the body moves it 0..1 step 1 */
        movable: true
        /** the edges and the corners size it 0..1 step 1 */
        resizable: true
        /** whether it is up; one that is not draws nothing at all 0..1 step 1 */
        shown: false

        content := mod.widgets.glass.Panel{
            width: Fill
            height: Fill
            flow: Down
            spacing: 12
            padding: 16
            // Scrolled, not spilled: the surface can be dragged smaller than
            // whatever was put on it, and content that ran out past the glass
            // would sit on the page with no sheet under it.
            body := View{
                width: Fill
                height: Fill
                flow: Down
                spacing: 12
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }
        }

        draw_grip +: {
            /** pointer-on-the-frame mix 0..1 step 0.01 */
            hover: uniform(0.0)
            grip_color: uniform(#xffffffcc)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let c = self.grip_color * (0.55 + self.hover * 0.45)
                // Six dots stepped down the corner's diagonal. Dots and not
                // hatch lines: a mark this small drawn as a path does not
                // paint reliably. They start 7 points in from each edge so
                // they clear the panel's rounded corner rather than sitting
                // on the unpainted bite it takes out of the square.
                let r = 1.4
                let s = 5.5
                let b = 7.0
                sdf.circle(w - b, h - b, r)
                sdf.fill(c)
                sdf.circle(w - b - s, h - b, r)
                sdf.fill(c)
                sdf.circle(w - b, h - b - s, r)
                sdf.fill(c)
                sdf.circle(w - b - s * 2.0, h - b, r)
                sdf.fill(c)
                sdf.circle(w - b - s, h - b - s, r)
                sdf.fill(c)
                sdf.circle(w - b, h - b - s * 2.0, r)
                sdf.fill(c)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.LensSurface = mod.widgets.LensedRoundedView{
        width: Fit
        height: 42
        flow: Overlay
        clip_x: false
        clip_y: false
        draw_bg +: {
            blur_level: 0.36
            lensing_effect: 1.0
            lensing_strength: 42.0
            lensing_width: 12.0
            corner_radius: 10.0
            tint_color: #xf8fbff
            tint_alpha: 0.014
            surface_alpha: 1.0
            border_alpha: 0.86
            border_width: 1.0
            specular_strength: 0.28
            noise_strength: 0.004
            shadow_color: #x0000
            shadow_radius: 0.0
            shadow_offset: vec2(0.0, 0.0)
            diffraction_strength: 5.6
        }
    }

    mod.widgets.glass.ButtonSurface = mod.widgets.glass.LensSurface{}

    mod.widgets.glass.ProminentButtonSurface = mod.widgets.glass.LensSurface{
        draw_bg +: {
            tint_alpha: 0.024
            surface_alpha: 1.0
            border_alpha: 0.94
            /** The prominent variant's face when there is nothing to
             * refract. It has to differ from the plain surface's - a
             * prominent button that loses the only thing marking it out is
             * worse than a grey one - so the shared theme-derived base is
             * carried toward this family's blue rather than replaced by a
             * fixed colour that a light theme would put a hole in. */
            fallback_color: mix(mix(theme.color_bg_app, theme.color_text, 0.30), #x2a6fd6, 0.45)
            diffraction_strength: 6.2
        }
    }

    mod.widgets.glass.IconSurface = mod.widgets.glass.LensSurface{
        width: 42
        height: 42
    }

    mod.widgets.glass.ChipSurface = mod.widgets.glass.LensSurface{
        height: 34
        draw_bg +: {
            corner_radius: 8.0
            lensing_strength: 36.0
            lensing_width: 10.0
            shadow_radius: 0.0
            shadow_offset: vec2(0.0, 0.0)
        }
    }

    mod.widgets.glass.RadioSurface = mod.widgets.glass.LensSurface{
        height: 40
        draw_bg +: {
            corner_radius: 10.0
            lensing_strength: 40.0
            lensing_width: 11.0
            tint_alpha: 0.010
            surface_alpha: 1.0
        }
    }

    mod.widgets.glass.InputSurface = mod.widgets.glass.LensSurface{
        height: 44
        draw_bg +: {
            corner_radius: 14.0
            blur_level: 0.48
            lensing_strength: 36.0
            lensing_width: 13.0
            tint_alpha: 0.008
            surface_alpha: 1.0
        }
    }

    mod.widgets.glass.CutButton = mod.widgets.ButtonFlat{
        width: Fit
        height: 42
        margin: 0
        padding: Inset{left: 16, right: 16, top: 0, bottom: 0}
        align: Center
        draw_text +: {
            color: #xf8fbff
            color_hover: #xffffffff
            color_down: #xd8ecff
            color_focus: #xffffffff
            text_style: theme.font_bold{font_size: 12}
        }
        draw_bg +: {
            border_size: uniform(1.0)
            border_radius: uniform(10.0)
            color: uniform(#x00081400)
            color_hover: uniform(#xffffff08)
            color_down: uniform(#x00081424)
            color_focus: uniform(#xffffff06)
            border_color: uniform(#xffffff10)
            border_color_hover: uniform(#xffffff5c)
            border_color_down: uniform(#xffffffff)
            border_color_focus: uniform(#x9dccff9a)
            inner_shadow: uniform(#x00000012)
            top_glint: uniform(#xffffff54)
            cut_depth: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let border = self.border_size
                let radius = self.border_radius

                sdf.box(
                    border
                    border
                    self.rect_size.x - border * 2.0
                    self.rect_size.y - border * 2.0
                    radius
                )

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                sdf.fill_keep(fill)

                let edge_light = smoothstep(0.0, 0.22, 1.0 - self.pos.y)
                let edge_dark = smoothstep(0.62, 1.0, self.pos.y)
                sdf.fill_keep(self.top_glint * edge_light * 0.16)
                sdf.fill_keep(self.inner_shadow * edge_dark * (0.32 + self.down * 0.26))

                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_down, self.down)
                sdf.stroke(stroke, border)
                return sdf.result
            }
        }
    }

    mod.widgets.glass.Button = mod.widgets.glass.CutButton{}

    mod.widgets.glass.ProminentButton = mod.widgets.glass.CutButton{
        draw_text +: {
            color: #xffffffff
            color_hover: #xffffffff
            color_down: #xffffffff
        }
        draw_bg +: {
            color: uniform(#x2f8fff08)
            color_hover: uniform(#x54a7ff18)
            color_down: uniform(#x1b5da83a)
            border_color: uniform(#x9dccff38)
            border_color_hover: uniform(#xd7e8ffff)
            border_color_down: uniform(#xffffffff)
            top_glint: uniform(#xffffffff)
        }
    }

    mod.widgets.glass.IconButton = mod.widgets.glass.CutButton{
        width: 42
        height: 42
        padding: 0
        draw_bg +: {
            border_radius: uniform(10.0)
        }
    }

    mod.widgets.glass.Chip = mod.widgets.glass.CutButton{
        height: 34
        padding: Inset{left: 13, right: 13, top: 0, bottom: 0}
        draw_text +: {
            text_style: theme.font_bold{font_size: 10}
        }
        draw_bg +: {
            border_radius: uniform(8.0)
            color: uniform(#x00081400)
            color_hover: uniform(#xffffff08)
            color_down: uniform(#x00081420)
            border_color: uniform(#xffffff0e)
        }
    }

    mod.widgets.glass.TextInput = mod.widgets.TextInputFlat{
        height: 38
        margin: 0
        // Single-line TextInput centres its glyphs vertically on its own; the
        // vertical padding only bounds the multiline/scroll clip.
        padding: Inset{left: 14, right: 14, top: 11, bottom: 11}
        empty_text: "Text"
        draw_bg +: {
            border_radius: 9.0
            border_size: 1.0
            color: #x00081418
            color_hover: #xffffff10
            color_focus: #x0008142c
            color_empty: #x00081418
            border_color: #xffffff36
            border_color_hover: #xffffff58
            border_color_focus: #x9dccffff
            border_color_empty: #xffffff36
        }
        draw_text +: {
            color: #xffffffff
            color_hover: #xffffffff
            color_focus: #xffffffff
            color_empty: #xd9e2f0aa
            color_empty_hover: #xffffffff
            color_empty_focus: #xffffffff
            text_style: theme.font_regular{font_size: 12, line_spacing: 1.0}
        }
    }

    mod.widgets.glass.SearchField = mod.widgets.glass.TextInput{
        empty_text: "Search"
    }

    mod.widgets.glass.Slider = mod.widgets.SliderFlat{
        width: Fill
        height: 40
        margin: 0
        draw_text +: {
            color: #xeef4ff
            color_hover: #xffffffff
            color_focus: #xffffffff
            text_style: theme.font_regular{font_size: 10}
        }
        draw_bg +: {
            border_radius: 14.0
            border_size: 1.0
            color: #x00081418
            color_hover: #xffffff10
            color_focus: #x0008142c
            color_drag: #x00081442
            border_color: #xffffff36
            border_color_hover: #xffffff58
            border_color_focus: #x9dccffff
            border_color_drag: #xffffffff
            handle_color: #xf9fbffff
            handle_color_hover: #xffffffff
            handle_color_focus: #xffffffff
            handle_color_drag: #xffffffff
            val_color: #x78b9ffff
            val_color_hover: #x95ccffff
            val_color_focus: #xb6ddffff
            val_color_drag: #xffffffff
        }
    }

    mod.widgets.glass.Toggle = mod.widgets.Toggle{
        margin: 0
        draw_text +: {
            color: #xeef4ff
            color_hover: #xffffffff
            color_active: #xffffffff
            text_style: theme.font_regular{font_size: 11}
        }
        draw_bg +: {
            color: #x00081418
            color_hover: #xffffff16
            color_active: #x65d6a6aa
            border_color: #xffffff40
            border_color_hover: #xffffff70
            border_color_active: #xb9ffe3ff
            mark_color: #xf6f8ffff
            mark_color_active: #xffffffff
        }
    }

    mod.widgets.glass.RadioButton = mod.widgets.RadioButton{
        width: 96
        height: 40
        margin: 0
        padding: 0
        align: Align{x: 0.0 y: 0.5}
        icon_walk: Walk{width: 0, height: Fit}
        label_walk: Walk{
            width: Fit
            height: Fit
            margin: 0
        }
        label_align: Align{x: 0.0 y: 0.5}
        draw_text +: {
            color: #xeef4ff
            color_hover: #xffffffff
            color_down: #xffffffff
            color_active: #xffffffff
            text_style: theme.font_bold{font_size: 13}
        }
        draw_bg +: {
            size: 40.0
            border_size: 1.0
            border_radius: 18.0
            color: #x00081400
            color_hover: #xffffff00
            color_down: #xffffff00
            color_active: #xffffff00
            border_color: #xffffff00
            border_color_hover: #xffffff00
            border_color_down: #xffffff00
            border_color_active: #xffffff00
            mark_color: #x00000000
            mark_color_active: #xffffffff
            inner_shadow: #x00140a44
            top_glint: #xffffffff
            active_glow: #x7dffaeff
            drop_shadow: #x00140855
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let active = self.active
                let w = self.rect_size.x
                let h = self.rect_size.y
                let pad = 2.0
                let track_x = pad
                let track_y = pad
                let track_w = w - pad * 2.0
                let track_h = h - pad * 2.0
                let track_r = track_h * 0.5
                let top = smoothstep(0.0, 0.26, 1.0 - self.pos.y)
                let bottom = smoothstep(0.58, 1.0, self.pos.y)
                let press = self.down * 0.08

                sdf.box(track_x, track_y, track_w, track_h, track_r)
                sdf.fill_keep(vec4(0.94, 1.0, 0.98, 0.10 + self.hover * 0.04))
                sdf.fill_keep(self.inner_shadow * bottom * (0.20 + self.down * 0.25))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.24 + self.hover * 0.18), 0.85)

                let green_x = track_x + 5.0 + press
                let green_y = track_y + 5.0 + press
                let green_w = 56.0 - press * 2.0
                let green_h = track_h - 10.0 - press * 2.0
                let green_r = green_h * 0.5
                sdf.box(green_x, green_y, green_w, green_h, green_r)
                sdf.fill_keep(vec4(0.02, 0.78, 0.31, active))
                sdf.fill_keep(vec4(0.36, 1.0, 0.58, active * 0.34) * top)
                sdf.fill_keep(vec4(0.0, 0.35, 0.12, active * 0.20) * bottom)
                sdf.stroke(vec4(0.74, 1.0, 0.78, active * 0.26), 0.70)

                let lens_x = track_x + 37.0 - press
                let lens_y = track_y - 1.0 + press
                let lens_w = track_w - 36.0
                let lens_h = track_h + 2.0 - press * 2.0
                let lens_r = lens_h * 0.5
                sdf.box(lens_x, lens_y, lens_w, lens_h, lens_r)
                sdf.fill_keep(vec4(0.93, 1.0, 0.98, 0.13 + active * 0.12 + self.hover * 0.04))
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, 0.11 + active * 0.08) * top)
                sdf.stroke(vec4(0.94, 1.0, 0.98, 0.62 + active * 0.20 + self.hover * 0.10), 1.20)

                sdf.box(lens_x + 8.0, lens_y + 4.0, lens_w - 18.0, 2.2, 1.1)
                sdf.fill_keep(self.top_glint * (0.22 + active * 0.18))

                sdf.circle(lens_x + lens_w - 12.5, lens_y + 10.5, 2.6)
                sdf.fill_keep(self.top_glint * (0.42 + active * 0.16))
                sdf.circle(lens_x + lens_w - 17.0, lens_y + lens_h - 10.0, 1.5)
                sdf.fill(self.active_glow * active * 0.30)
                return sdf.result
            }
        }
        animator +: {
            active: {
                off: AnimatorState{
                    from: {all: Forward {duration: 0.18}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.22}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.glass.List = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: 1
        padding: 4
        show_bg: true
        draw_bg +: {
            color: #x0f172560
            border_radius: 14.0
            border_color: #xffffff20
            border_size: 1.0
        }
    }

    mod.widgets.glass.ListRow = View{
        width: Fill
        height: 54
        flow: Right
        spacing: 10
        padding: Inset{left: 12, right: 12, top: 7, bottom: 7}
        align: Align{x: 0.0 y: 0.5}
    }

    mod.widgets.glass.Badge = View{
        width: Fit
        height: 24
        flow: Right
        align: Center
        padding: Inset{left: 9, right: 9, top: 0, bottom: 0}
        show_bg: true
        draw_bg +: {
            color: #x1dce8a66
            border_color: #xb8ffddaa
            border_size: 1.0
            border_radius: 9.0
        }
    }

    // Typography for glass UIs - kept in the library so generated app code can just
    // reference `glass.H1`, `glass.Caption`, etc. instead of restyling labels inline.
    mod.widgets.glass.H1 = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 30}
    }
    mod.widgets.glass.H2 = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 18}
    }
    mod.widgets.glass.Body = Label{
        width: Fill
        height: Fit
        draw_text.color: #xd9e8ffcc
        draw_text.text_style: theme.font_regular{font_size: 13}
    }
    mod.widgets.glass.Caption = Label{
        width: Fit
        height: Fit
        draw_text.color: #x8fa6c8ff
        draw_text.text_style: theme.font_bold{font_size: 11}
    }
    mod.widgets.glass.OptionLabel = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 16}
    }

    mod.widgets.glass.ButtonLabel = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 13}
    }

    // Lensing glass buttons: a refracting surface with a centered label slot.
    mod.widgets.glass.LensButton = mod.widgets.glass.ButtonSurface{
        height: 44
        align: Align{x: 0.5, y: 0.5}
        padding: Inset{left: 20, right: 20, top: 0, bottom: 0}
    }
    mod.widgets.glass.LensButtonProminent = mod.widgets.glass.ProminentButtonSurface{
        height: 44
        align: Align{x: 0.5, y: 0.5}
        padding: Inset{left: 20, right: 20, top: 0, bottom: 0}
    }
    mod.widgets.glass.LensChip = mod.widgets.glass.ChipSurface{
        height: 32
        align: Align{x: 0.5, y: 0.5}
        padding: Inset{left: 14, right: 14, top: 0, bottom: 0}
    }

    mod.widgets.GlassPanel = mod.widgets.glass.Panel{}
    mod.widgets.GlassFloatingSurface = mod.widgets.glass.FloatingSurface{}
}

#[derive(Script, Widget)]
pub struct GlassLayer {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[live]
    draw_bg: DrawQuad,
}

impl ScriptHook for GlassLayer {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for GlassLayer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, self.view.layout);
        self.draw_bg.begin(cx, self.view.walk, self.view.layout);
        self.view.draw_all(cx, scope);
        self.draw_bg.end(cx);
        cx.end_pass_sized_turtle();

        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

#[derive(Clone, Debug, Default)]
pub enum GlassRadioAction {
    Clicked,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct GlassRadio {
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
    draw_slot: DrawQuad,
    #[redraw]
    #[live]
    draw_knob: DrawQuad,

    #[live]
    on_click: ScriptFnRef,

    #[visible]
    #[live(true)]
    pub visible: bool,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,

    #[rust]
    draw_list: Option<DrawList2d>,

    // State is driven directly from Rust (the script animator does not bind to these
    // script-declared draw uniforms reliably). `active` eases toward `active_target`.
    #[rust]
    active: f32,
    #[rust]
    active_target: f32,
    #[rust]
    hover: f32,
    #[rust]
    down: f32,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for GlassRadio {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        vm.with_cx_mut(|cx| {
            // Built (or rebuilt by a live reload) and not drawn yet: ask for the scene to
            // be captured on the frame this first paints in, rather than the one after it.
            arm_gauss_capture(cx);
            self.redraw(cx);
        });
    }
}

impl GlassRadio {
    fn bind_knob_snapshot(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw_knob = &mut self.draw_knob.draw_vars;
        if let Some(snapshot) = snapshot {
            draw_knob.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw_knob.set_texture(slot, texture);
                } else {
                    draw_knob.empty_texture(slot);
                }
            }
            draw_knob.set_uniform(
                cx,
                live_id!(source_size),
                &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
            );
            draw_knob.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw_knob.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw_knob.empty_texture(slot);
            }
            draw_knob.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
            draw_knob.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
            draw_knob.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_slot.redraw(cx);
        self.draw_knob.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), GlassRadioAction::Clicked)
        } else {
            false
        }
    }

    pub fn active(&self, _cx: &Cx) -> bool {
        self.active_target > 0.5
    }

    pub fn set_active(&mut self, cx: &mut Cx, value: bool, animate: Animate) {
        self.active_target = if value { 1.0 } else { 0.0 };
        if let Animate::No = animate {
            self.active = self.active_target;
        } else {
            self.next_frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    fn push_state(&mut self, cx: &mut Cx) {
        for draw in [&mut self.draw_slot, &mut self.draw_knob] {
            draw.draw_vars.set_uniform(cx, live_id!(active), &[self.active]);
            draw.draw_vars.set_uniform(cx, live_id!(hover), &[self.hover]);
            draw.draw_vars.set_uniform(cx, live_id!(down), &[self.down]);
        }
    }
}

impl Widget for GlassRadio {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();

        // Ease `active` toward its target each frame for the sliding animation.
        if self.next_frame.is_event(event).is_some() {
            let delta = self.active_target - self.active;
            if delta.abs() <= 0.004 {
                self.active = self.active_target;
            } else {
                self.active += delta * 0.18;
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }

        match event.hits(cx, self.draw_slot.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hover = 1.0;
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.hover = 0.0;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.down = 1.0;
                self.set_key_focus(cx);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.down = 0.0;
                // Checkbox semantics: a click toggles this control on or off
                // independently (with the sliding animation), it is not a radio.
                if fe.is_over {
                    self.active_target = if self.active_target > 0.5 { 0.0 } else { 1.0 };
                    self.next_frame = cx.new_next_frame();
                    cx.widget_action_with_data(&self.action_data, uid, GlassRadioAction::Clicked);
                    cx.widget_to_script_call(uid, NIL, self.source.clone(), self.on_click.clone(), &[]);
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        // The slot is laid out in the normal flow, so its final on-screen position is only
        // known once the ancestor turtles apply their deferred alignment (e.g. the column is
        // centered in the pass, the row is centered vertically). Those shifts are recorded in
        // the turtle align list against every instance drawn inside the current turtle's range.
        //
        // We therefore draw the glass knob into the overlay draw list *while the current
        // (row) turtle is still active* and at the slot's pre-align rect. Because the knob
        // instance is registered in the same align range as the slot, `move_align_list` shifts
        // both by the exact same amount - so the knob (and its lensing `rect_pos`) tracks the
        // slot precisely. Wrapping it in a `begin_root_turtle`/`end_pass_sized_turtle` pair
        // instead turns that range into a `SkipTurtle`, which the parent's shift skips over -
        // that is what left the knob detached from the background.
        self.push_state(cx);
        let rect = self.draw_slot.draw_walk(cx, walk);
        cx.add_nav_stop(self.draw_slot.area(), NavRole::TextInput, Inset::default());

        // The glass lens overlay: a full-switch-size rect drawn on top into the overlay draw
        // list (so it can sample the blurred scene). Its nub position and gloop are computed
        // from `active` entirely in the shader.
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let snapshot = request_window_gauss(cx);
        self.bind_knob_snapshot(cx, snapshot);
        self.draw_knob.draw_abs(cx, rect);
        self.draw_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }
}

impl GlassRadioRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.clicked(actions))
    }

    pub fn active(&self, cx: &Cx) -> bool {
        self.borrow().is_some_and(|inner| inner.active(cx))
    }

    pub fn set_active(&self, cx: &mut Cx, value: bool, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, value, animate);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum GlassButtonAction {
    Clicked,
    #[default]
    None,
}

/// A clickable glass button that draws its solid base in the background pass and a
/// self-managed lensing glass overlay on top (same approach as GlassRadio), so it
/// refracts the scene and composes anywhere in normal flow.
#[derive(Script, Widget)]
pub struct GlassButton {
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
    draw_bg: DrawQuad,
    #[redraw]
    #[live]
    draw_glass: DrawQuad,
    #[live]
    draw_text: DrawText,
    /// An optional glyph beside (or instead of) the label. It has to be the
    /// button's own: the button draws itself into a self-managed overlay, so
    /// anything a caller stacks on top of it from outside is painted over by
    /// the glass.
    #[live]
    pub draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[live]
    pub text: ArcStringMut,
    #[live]
    on_click: ScriptFnRef,

    #[visible]
    #[live(true)]
    pub visible: bool,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,

    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    hover: f32,
    #[rust]
    down: f32,
    #[rust]
    press: f32,
    #[rust]
    pressing: bool,
    #[rust]
    press_started_at: f64,
    #[rust]
    release_started_at: f64,
    #[rust]
    release_flatten: f32,
    #[rust]
    press_flatten: f32,
    #[rust]
    ripple_age: f32,
    #[rust]
    ripple_strength: f32,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for GlassButton {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        vm.with_cx_mut(|cx| {
            // Built (or rebuilt by a live reload) and not drawn yet: ask for the scene to
            // be captured on the frame this first paints in, rather than the one after it.
            arm_gauss_capture(cx);
            self.redraw(cx);
        });
    }
}

impl GlassButton {
    fn bind_glass(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw = &mut self.draw_glass.draw_vars;
        if let Some(snapshot) = snapshot {
            draw.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw.set_texture(slot, texture);
                } else {
                    draw.empty_texture(slot);
                }
            }
            draw.set_uniform(
                cx,
                live_id!(source_size),
                &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
            );
            draw.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw.empty_texture(slot);
            }
            draw.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
            draw.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
            draw.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
        self.draw_glass.redraw(cx);
        self.draw_text.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn push_state(&mut self, cx: &mut Cx) {
        for draw in [&mut self.draw_bg, &mut self.draw_glass] {
            draw.draw_vars.set_uniform(cx, live_id!(hover), &[self.hover]);
            draw.draw_vars.set_uniform(cx, live_id!(down), &[self.down]);
            draw.draw_vars.set_uniform(cx, live_id!(press), &[self.press]);
        }
        // The press response lives on the lens only. `press` (set above) is the smooth shrink; the
        // flattening wave is driven by press_flatten + ripple_age + ripple_strength.
        let glass = &mut self.draw_glass.draw_vars;
        glass.set_uniform(cx, live_id!(press_flatten), &[self.press_flatten]);
        glass.set_uniform(cx, live_id!(ripple_age), &[self.ripple_age]);
        glass.set_uniform(cx, live_id!(ripple_strength), &[self.ripple_strength]);
    }

    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), GlassButtonAction::Clicked)
        } else {
            false
        }
    }
}

impl Widget for GlassButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();

        // Drive the press response (ported from examples/splash). `press` smoothly eases toward
        // `down` for the shrink (critically damped -> no jiggle). A flattening WAVE sweeps the lens
        // on press (press_flatten 0 -> 1, smoothstep over ~0.95s) and inverts on release; the wave
        // age/strength fade out over ~1.35s.
        if let Some(time) = self.next_frame.is_event(event).map(|ne| ne.time) {
            let d = self.down - self.press;
            let mut keep = false;
            if d.abs() > 0.002 {
                self.press += d * 0.28;
                keep = true;
            } else {
                self.press = self.down;
            }

            if self.pressing {
                if self.press_started_at <= 0.0 {
                    self.press_started_at = time;
                }
                let age = (time - self.press_started_at).max(0.0);
                let t = (age / 0.95).min(1.0) as f32;
                let flatten = t * t * (3.0 - 2.0 * t);
                self.press_flatten = flatten;
                self.ripple_age = age as f32;
                self.ripple_strength = (1.0 - age / 1.30).max(0.0) as f32 * (1.0 - flatten * 0.10);
                if age < 1.35 {
                    keep = true;
                }
            } else if self.press_started_at > 0.0 {
                if self.release_started_at <= 0.0 {
                    self.release_started_at = time;
                }
                let age = (time - self.release_started_at).max(0.0);
                // Hold the captured flatten as a negative value -> the wave un-flattens the lens.
                self.press_flatten = -self.release_flatten.clamp(0.0, 1.0);
                self.ripple_age = age as f32;
                self.ripple_strength = (1.0 - age / 1.30).max(0.0) as f32 * 0.62;
                if age < 1.35 {
                    keep = true;
                } else {
                    self.press_started_at = 0.0;
                    self.release_started_at = 0.0;
                    self.press_flatten = 0.0;
                    self.ripple_strength = 0.0;
                }
            }

            self.push_state(cx);
            if keep {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hover = 1.0;
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.hover = 0.0;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.down = 1.0;
                // Start the press flattening wave.
                self.pressing = true;
                self.press_started_at = 0.0;
                self.release_started_at = 0.0;
                self.ripple_strength = 0.0;
                self.next_frame = cx.new_next_frame();
                self.set_key_focus(cx);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.down = 0.0;
                // Release: capture the current flatten and run the inverse (un-flatten) wave.
                self.pressing = false;
                self.release_flatten = self.press_flatten.max(0.0);
                self.release_started_at = 0.0;
                self.next_frame = cx.new_next_frame();
                if fe.is_over {
                    cx.widget_action_with_data(&self.action_data, uid, GlassButtonAction::Clicked);
                    // Fire the splash `on_click: || ...` handler so lensing glass buttons are
                    // interactive in runsplash blocks (not just visual flourish).
                    cx.widget_to_script_call(uid, NIL, self.source.clone(), self.on_click.clone(), &[]);
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        // Draw the WHOLE button into a self-managed overlay so nothing (especially the label)
        // is captured by the gauss scene - a captured label refracts into a dark bar. The
        // transparent base just establishes layout + hit area, the opaque glass refracts the
        // real background beneath, and the crisp label is drawn last on top.
        self.push_state(cx);
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let snapshot = request_window_gauss(cx);
        self.bind_glass(cx, snapshot);

        // Measure the label once. We RESERVE this size in the bg turtle (so Fit-width buttons
        // still size to their text) but do NOT draw the glyphs there - drawing them in the bg
        // turtle as well would batch with the crisp label below and render the text TWICE at
        // slightly different positions (a ghost). The only visible label is the draw_abs pass on
        // top of the glass.
        let text = self.text.as_ref();
        let laid = self
            .draw_text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let text_size = dvec2(
            laid.size_in_lpxs.width as f64 * self.draw_text.font_scale as f64,
            laid.size_in_lpxs.height as f64 * self.draw_text.font_scale as f64,
        );

        // The icon is measured with the label and reserved with it, so a
        // Fit-width button sizes to both. With no icon set this is exactly
        // the label on its own, as it always was.
        let icon_walk = self.icon_walk;
        let icon_size = self.draw_icon.measure(cx, icon_walk);
        let icon_gap = match icon_size {
            Some(_) if !text.is_empty() => self.layout.spacing,
            _ => 0.0,
        };
        let content = dvec2(
            icon_size.map(|s| s.x).unwrap_or(0.0) + icon_gap + text_size.x,
            text_size.y.max(icon_size.map(|s| s.y).unwrap_or(0.0)),
        );

        self.draw_bg.begin(cx, walk, self.layout);
        cx.walk_turtle(Walk {
            abs_pos: None,
            margin: Inset::default(),
            width: Size::Fixed(content.x),
            height: Size::Fixed(content.y),
            ..Default::default()
        });
        self.draw_bg.end(cx);
        let rect = self.draw_bg.area().rect(cx);

        self.draw_glass.draw_abs(cx, rect);

        // Crisp label on top of the glass, as plain aligned glyph instances (draw_abs) registered
        // in the CURRENT turtle's align range - exactly like the glass quad above - NOT inside a
        // nested `begin_turtle(abs_pos: ...)`. An abs_pos walk records `deferred_before_count: 0`
        // (turtle.rs walk_turtle_internal), so after a `Fill` sibling the parent's deferred-fill
        // shift would skip the label and detach it. draw_abs keeps it in the shifted align range.
        let align = self.layout.align;
        let pos = dvec2(
            rect.pos.x + (rect.size.x - content.x) * align.x,
            rect.pos.y + (rect.size.y - content.y) * align.y,
        );
        // Icon first, then the label beside it — both by draw_abs on top of
        // the glass, for the same reason the label alone always was.
        if let Some(size) = icon_size {
            self.draw_icon.draw_abs(
                cx,
                Rect { pos: dvec2(pos.x, pos.y + (content.y - size.y) * 0.5), size },
            );
        }
        let text_pos = dvec2(
            pos.x + icon_size.map(|s| s.x).unwrap_or(0.0) + icon_gap,
            pos.y + (content.y - text_size.y) * 0.5,
        );
        self.draw_text.draw_abs(cx, text_pos, text);
        self.draw_list.as_mut().unwrap().end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());

        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text.set(v);
        self.redraw(cx);
    }

    // Scripts can read and change the button label (e.g. a play/pause toggle),
    // mirroring Label's script surface.
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.bx.heap.new_string_from_str(self.text.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(new_text) = vm
                        .bx
                        .heap
                        .cast_to_owned_string(value, "copying glass button text")
                    {
                        vm.with_cx_mut(|cx| {
                            self.set_text(cx, &new_text);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }
}

impl GlassButtonRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.clicked(actions))
    }
}

#[derive(Clone, Debug, Default)]
pub enum GlassSliderAction {
    Changed,
    #[default]
    None,
}

/// A slider with a draggable lensing glass knob. The track (with its filled portion) is drawn
/// in the background pass; the glass knob refracts it in a self-managed overlay (like
/// GlassRadio), so it composes in normal flow.
#[derive(Script, Widget)]
pub struct GlassSlider {
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
    draw_track: DrawQuad,
    #[redraw]
    #[live]
    draw_knob: DrawQuad,

    #[visible]
    #[live(true)]
    pub visible: bool,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,

    #[rust]
    draw_list: Option<DrawList2d>,
    #[live(0.4)]
    pub value: f32,
    #[rust]
    hover: f32,
    #[rust]
    dragging: bool,
}

impl ScriptHook for GlassSlider {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        vm.with_cx_mut(|cx| {
            // Built (or rebuilt by a live reload) and not drawn yet: ask for the scene to
            // be captured on the frame this first paints in, rather than the one after it.
            arm_gauss_capture(cx);
            self.redraw(cx);
        });
    }
}

impl GlassSlider {
    fn bind_knob(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw = &mut self.draw_knob.draw_vars;
        if let Some(snapshot) = snapshot {
            draw.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw.set_texture(slot, texture);
                } else {
                    draw.empty_texture(slot);
                }
            }
            draw.set_uniform(
                cx,
                live_id!(source_size),
                &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
            );
            draw.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw.empty_texture(slot);
            }
            draw.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
            draw.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
            draw.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_track.redraw(cx);
        self.draw_knob.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn push_state(&mut self, cx: &mut Cx) {
        for draw in [&mut self.draw_track, &mut self.draw_knob] {
            draw.draw_vars.set_uniform(cx, live_id!(value), &[self.value]);
            draw.draw_vars.set_uniform(cx, live_id!(hover), &[self.hover]);
        }
    }

    fn set_value_from_x(&mut self, cx: &mut Cx, abs_x: f64) -> bool {
        let rect = self.draw_track.area().rect(cx);
        let v = (((abs_x - rect.pos.x) / rect.size.x.max(1.0)) as f32).clamp(0.0, 1.0);
        if (v - self.value).abs() > 0.0001 {
            self.value = v;
            true
        } else {
            false
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn changed(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), GlassSliderAction::Changed)
        } else {
            false
        }
    }
}

impl Widget for GlassSlider {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_track.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hover = 1.0;
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.hover = 0.0;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.dragging = true;
                self.set_key_focus(cx);
                if self.set_value_from_x(cx, fe.abs.x) {
                    cx.widget_action_with_data(&self.action_data, uid, GlassSliderAction::Changed);
                }
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                if self.dragging && self.set_value_from_x(cx, fe.abs.x) {
                    cx.widget_action_with_data(&self.action_data, uid, GlassSliderAction::Changed);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.dragging = false;
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.push_state(cx);
        let rect = self.draw_track.draw_walk(cx, walk);
        cx.add_nav_stop(self.draw_track.area(), NavRole::TextInput, Inset::default());

        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let snapshot = request_window_gauss(cx);
        self.bind_knob(cx, snapshot);
        self.draw_knob.draw_abs(cx, rect);
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

impl GlassSliderRef {
    pub fn value(&self) -> f32 {
        self.borrow().map_or(0.0, |inner| inner.value())
    }

    pub fn changed(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.changed(actions))
    }
}

#[derive(Clone, Debug, Default)]
pub enum GlassSegmentedAction {
    Selected,
    #[default]
    None,
}

/// A segmented control: N text segments with a lensing glass selection pill that gloops to
/// the selected segment. Container + labels in the background pass; the pill refracts them in
/// a self-managed overlay.
#[derive(Script, Widget)]
pub struct GlassSegmented {
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
    draw_bg: DrawQuad,
    #[redraw]
    #[live]
    draw_sel: DrawQuad,
    #[live]
    draw_text: DrawText,

    #[live]
    labels: Vec<String>,
    #[rust]
    pub selected: usize,

    #[visible]
    #[live(true)]
    pub visible: bool,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,

    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    sel_pos: f32,
    /// Per-segment (x offset from the control's left edge, width), sized to
    /// each label rather than `width / count`: "Default" and "Max" are very
    /// different words and equal thirds crowd the long one while stranding the
    /// short one. Filled during draw; hit-testing reads it back.
    #[rust]
    seg_geom: Vec<(f64, f64)>,
    #[rust]
    hover: f32,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for GlassSegmented {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        self.sel_pos = self.selected as f32;
        vm.with_cx_mut(|cx| {
            // Built (or rebuilt by a live reload) and not drawn yet: ask for the scene to
            // be captured on the frame this first paints in, rather than the one after it.
            arm_gauss_capture(cx);
            self.redraw(cx);
        });
    }
}

/// Space either side of a label inside its segment. The whole point of
/// measuring is that the pill never crowds the word.
const SEG_PAD: f64 = 13.0;
/// Never squeeze below this, however many segments there are — past it the
/// text starts touching the pill's edge and looks broken rather than tight.
const SEG_PAD_MIN: f64 = 5.0;

impl GlassSegmented {
    /// Sizes each segment to its own label, then spends whatever width is left
    /// over evenly. If the labels don't fit at full padding the padding
    /// shrinks (never the text) down to `SEG_PAD_MIN`.
    fn measure_segments(&mut self, cx: &mut Cx2d, total: f64) {
        let widths: Vec<f64> = self
            .labels
            .clone()
            .iter()
            .map(|label| {
                self.draw_text
                    .layout(cx.cx, 0.0, 0.0, None, false, Align::default(), label)
                    .size_in_lpxs
                    .width as f64
            })
            .collect();
        let n = widths.len().max(1) as f64;
        let text_total: f64 = widths.iter().sum();
        let pad = (((total - text_total) / (2.0 * n)).min(SEG_PAD)).max(SEG_PAD_MIN);
        let natural: f64 = text_total + pad * 2.0 * n;
        // Slack is shared equally so every segment keeps the same margin; a
        // proportional share would give the longest word the most air, which
        // is the opposite of what crowding needs.
        let slack = ((total - natural) / n).max(0.0);
        let scale = if natural > total && natural > 0.0 { total / natural } else { 1.0 };
        self.seg_geom.clear();
        let mut x = 0.0;
        for width in &widths {
            let w = (width + pad * 2.0) * scale + slack;
            self.seg_geom.push((x, w));
            x += w;
        }
    }

    /// Where the selection pill sits right now, in pixels from the control's
    /// left edge, including the travelling squash-and-stretch.
    fn pill_geometry(&self) -> (f64, f64) {
        if self.seg_geom.is_empty() {
            return (0.0, 0.0);
        }
        let last = self.seg_geom.len() - 1;
        let pos = (self.sel_pos as f64).clamp(0.0, last as f64);
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(last);
        let t = pos - i0 as f64;
        let (x0, w0) = self.seg_geom[i0];
        let (x1, w1) = self.seg_geom[i1];
        let x = x0 + (x1 - x0) * t;
        let w = w0 + (w1 - w0) * t;
        // 0 at rest, 1 at the midpoint of a move.
        let gloop = (t - t.round()).abs() * 2.0;
        const PAD: f64 = 3.0;
        (x + PAD - gloop * w * 0.22, w - PAD * 2.0 + gloop * w * 0.44)
    }

    fn bind_sel(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw = &mut self.draw_sel.draw_vars;
        if let Some(snapshot) = snapshot {
            draw.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw.set_texture(slot, texture);
                } else {
                    draw.empty_texture(slot);
                }
            }
            draw.set_uniform(
                cx,
                live_id!(source_size),
                &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
            );
            draw.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw.empty_texture(slot);
            }
            draw.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
            draw.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
            draw.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
        self.draw_sel.redraw(cx);
        self.draw_text.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn count(&self) -> f32 {
        self.labels.len().max(1) as f32
    }

    fn push_state(&mut self, cx: &mut Cx) {
        let count = self.count();
        for draw in [&mut self.draw_bg, &mut self.draw_sel] {
            draw.draw_vars.set_uniform(cx, live_id!(sel_pos), &[self.sel_pos]);
            draw.draw_vars.set_uniform(cx, live_id!(count), &[count]);
            draw.draw_vars.set_uniform(cx, live_id!(hover), &[self.hover]);
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Selects `index` from code, keeping the DRAWN pill in step.
    ///
    /// `selected` is public but the pill is positioned from `sel_pos`, which
    /// only follows it through `on_after_apply` or the click animation —
    /// writing the field directly leaves the control showing one segment while
    /// reporting another, and a click on the segment it is really on then does
    /// nothing (the handler ignores a click on the current selection). Always
    /// restore a saved value through here.
    pub fn set_selected(&mut self, cx: &mut Cx, index: usize) {
        let index = index.min(self.labels.len().saturating_sub(1));
        self.selected = index;
        self.sel_pos = index as f32;
        self.redraw(cx);
    }

    pub fn changed(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), GlassSegmentedAction::Selected)
        } else {
            false
        }
    }
}

impl Widget for GlassSegmented {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();

        if self.next_frame.is_event(event).is_some() {
            let target = self.selected as f32;
            let delta = target - self.sel_pos;
            if delta.abs() <= 0.004 {
                self.sel_pos = target;
            } else {
                // Deliberately unhurried: the pill is the only thing that
                // confirms the tap, and at 0.30 it arrived before the eye
                // could follow it.
                self.sel_pos += delta * 0.16;
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hover = 1.0;
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.hover = 0.0;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let rect = self.draw_bg.area().rect(cx);
                let n = self.labels.len().max(1);
                let local = fe.abs.x - rect.pos.x;
                // Segments have their own widths, so the index is a lookup, not
                // a division. Falls back to even thirds only before the first
                // draw has measured anything.
                let idx = if self.seg_geom.is_empty() {
                    let frac = (local / rect.size.x.max(1.0)).clamp(0.0, 0.999);
                    (frac * n as f64) as usize
                } else {
                    self.seg_geom
                        .iter()
                        .position(|(x, w)| local < x + w)
                        .unwrap_or(n - 1)
                };
                if idx != self.selected {
                    self.selected = idx;
                    self.next_frame = cx.new_next_frame();
                    cx.widget_action_with_data(&self.action_data, uid, GlassSegmentedAction::Selected);
                }
                self.set_key_focus(cx);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.push_state(cx);
        // Only the container is drawn into the background pass; the labels are drawn on TOP
        // of the glass pill below so the text stays sharp (not refracted through the blurry
        // gauss capture).
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        let rect = self.draw_bg.area().rect(cx);

        // Lensing selection pill + crisp labels, both in a self-managed overlay.
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.measure_segments(cx, rect.size.x);
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let snapshot = request_window_gauss(cx);
        self.bind_sel(cx, snapshot);
        let (pill_x, pill_w) = self.pill_geometry();
        self.draw_sel
            .draw_vars
            .set_uniform(cx, live_id!(pill_x), &[pill_x as f32]);
        self.draw_sel
            .draw_vars
            .set_uniform(cx, live_id!(pill_w), &[pill_w as f32]);
        self.draw_sel.draw_abs(cx, rect);
        // Place each label at the centre of ITS OWN segment. Segments are
        // sized to their labels, so the pill always surrounds a word with the
        // same breathing room whether it says "Max" or "Default".
        let labels = self.labels.clone();
        for (i, label) in labels.iter().enumerate() {
            let (x, w) = self.seg_geom.get(i).copied().unwrap_or((0.0, 0.0));
            cx.begin_turtle(
                Walk {
                    abs_pos: Some(Vec2d { x: rect.pos.x + x, y: rect.pos.y }),
                    width: Size::Fixed(w),
                    height: Size::Fixed(rect.size.y),
                    ..Default::default()
                },
                Layout {
                    align: Align { x: 0.5, y: 0.5 },
                    ..Layout::default()
                },
            );
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align::default(), label);
            cx.end_turtle();
        }
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

impl GlassSegmentedRef {
    pub fn selected(&self) -> usize {
        self.borrow().map_or(0, |inner| inner.selected())
    }

    pub fn changed(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.changed(actions))
    }

    /// See [`GlassSegmented::set_selected`].
    pub fn set_selected(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, index);
        }
    }
}

/// What a floating surface reports.
///
/// A resize from a top or a left edge moves the surface as well as sizing
/// it, so every one of these carries the whole frame. `Sizing` and `Moving`
/// arrive on every frame of a drag; `Placed` once, when the hand comes off.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum GlassFloatingSurfaceAction {
    /// A resize is under way, and the frame it is passing through.
    Sizing { pos: Vec2d, size: Vec2d },
    /// A move is under way.
    Moving { pos: Vec2d, size: Vec2d },
    /// Where the drag left it.
    Placed { pos: Vec2d, size: Vec2d },
    #[default]
    None,
}

/// Which sides of a surface a press took hold of. Two sides is a corner,
/// one is an edge, and none of them is the body.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Grip {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Grip {
    fn is_empty(self) -> bool {
        !(self.left || self.right || self.top || self.bottom)
    }

    /// The pointer that says what this grip will do before it is dragged.
    fn cursor(self) -> MouseCursor {
        match (self.left || self.right, self.top || self.bottom) {
            // The two diagonals are different pointers, and a corner that
            // shows the wrong one is a corner the hand distrusts. Top-left
            // and bottom-right lean one way (both flags agree), top-right
            // and bottom-left the other.
            (true, true) if self.left == self.top => MouseCursor::NwseResize,
            (true, true) => MouseCursor::NeswResize,
            (true, false) => MouseCursor::EwResize,
            (false, true) => MouseCursor::NsResize,
            (false, false) => MouseCursor::Arrow,
        }
    }
}

/// A floating surface's frame, and the limits a drag on it must respect.
///
/// It is a plain struct apart from the widget for two reasons: the hit test
/// and the resize are then the same arithmetic — what is grabbable is what
/// moves — and it can be checked without a window.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Frame {
    pos: Vec2d,
    size: Vec2d,
    min: Vec2d,
    max: Vec2d,
}

impl Frame {
    /// What a press at `at` takes hold of.
    ///
    /// The band reaches `grab` points BOTH ways from each edge. Inward
    /// alone would not do: the surface is a rounded rectangle, so its last
    /// few points at every corner are unpainted, and a band that stopped at
    /// the edge would ask for a press on glass that is not there. Outward
    /// alone would take presses meant for the page behind it.
    fn grip_at(self, at: Vec2d, grab: f64) -> Grip {
        let grab = grab.max(0.0);
        let lo = self.pos;
        let hi = self.pos + self.size;
        if at.x < lo.x - grab || at.x > hi.x + grab || at.y < lo.y - grab || at.y > hi.y + grab {
            return Grip::default();
        }
        let mut grip = Grip {
            left: (at.x - lo.x).abs() <= grab,
            right: (at.x - hi.x).abs() <= grab,
            top: (at.y - lo.y).abs() <= grab,
            bottom: (at.y - hi.y).abs() <= grab,
        };
        // A surface narrower than two bands has them overlapping down the
        // middle. Holding both would size it from both ends at once under
        // one finger, which is not a gesture anybody makes on purpose, so
        // the nearer edge wins.
        if grip.left && grip.right {
            if at.x - lo.x <= hi.x - at.x {
                grip.right = false;
            } else {
                grip.left = false;
            }
        }
        if grip.top && grip.bottom {
            if at.y - lo.y <= hi.y - at.y {
                grip.bottom = false;
            } else {
                grip.top = false;
            }
        }
        grip
    }

    /// Where a drag of `delta` on `grip` leaves this frame.
    fn resized(self, grip: Grip, delta: Vec2d) -> (Vec2d, Vec2d) {
        let (x, w) = resize_axis(
            self.pos.x,
            self.size.x,
            delta.x,
            grip.left,
            grip.right,
            self.min.x,
            self.max.x,
        );
        let (y, h) = resize_axis(
            self.pos.y,
            self.size.y,
            delta.y,
            grip.top,
            grip.bottom,
            self.min.y,
            self.max.y,
        );
        (dvec2(x, y), dvec2(w, h))
    }

    /// The frame pulled inside a window of `room`, so a surface dragged at
    /// the edge cannot be left somewhere it can never be dragged back from.
    fn settled(self, room: Vec2d) -> (Vec2d, Vec2d) {
        let w = fit_axis(self.size.x, self.min.x, self.max.x, room.x);
        let h = fit_axis(self.size.y, self.min.y, self.max.y, room.y);
        let pos = dvec2(
            span_inboard(self.pos.x, w, 0.0, room.x),
            span_inboard(self.pos.y, h, 0.0, room.y),
        );
        (pos, dvec2(w, h))
    }
}

/// The ceiling for one axis. A `max` of zero — or any max below the floor —
/// means unbounded, which is also what keeps `clamp` out of the one state it
/// panics in: a floor above its own ceiling.
fn ceiling(min: f64, max: f64) -> f64 {
    if max > 0.0 {
        max.max(min)
    } else {
        f64::INFINITY
    }
}

/// Resize one axis: `lo` moves the near edge, `hi` the far one, and with
/// neither of them set the axis is left exactly as it was.
///
/// The clamp is applied to the SIZE and the position is derived from it,
/// never the other way about. Dragging the near edge inward past the floor
/// has to pin that edge and leave the far one exactly where it was; a
/// position moved first and clamped afterwards drags the far edge along
/// with it, which is the resize bug everyone writes once.
fn resize_axis(
    start: f64,
    extent: f64,
    delta: f64,
    lo: bool,
    hi: bool,
    min: f64,
    max: f64,
) -> (f64, f64) {
    // Never zero, whatever the caller declared: a surface with no size left
    // is invisible AND has no edge to grab, so it could not be brought back.
    let min = min.max(1.0);
    let max = ceiling(min, max);
    if lo {
        let e = (extent - delta).clamp(min, max);
        (start + extent - e, e)
    } else if hi {
        (start, (extent + delta).clamp(min, max))
    } else {
        (start, extent)
    }
}

/// One axis of a size held between its floor, its ceiling and the room there
/// is. The room beats the ceiling and the floor beats the room: a surface
/// squeezed under its own floor by a small window is still usable, and one
/// squeezed to nothing is not.
fn fit_axis(extent: f64, min: f64, max: f64, room: f64) -> f64 {
    let min = min.max(1.0);
    let max = ceiling(min, max).min(room.max(min));
    extent.clamp(min, max)
}

/// What a press does to a surface, decided before its contents are asked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Press {
    /// Someone asked earlier already answered it.
    Taken,
    /// On the frame: size it, and the contents never see the press.
    Size(Grip),
    /// On the sheet: the contents get first refusal, and if none of them
    /// wants it the surface claims it - moving if it may move, and standing
    /// still but still claiming if it may not.
    Sheet { moves: bool },
    /// Not on this surface at all.
    Elsewhere,
}

/// Whose press it is.
///
/// `handled` is the protocol every widget that goes through `event.hits`
/// keeps: a press carries the area of whoever took it, `hits` hands it to
/// nobody else, and it stamps the area on the way out. This surface cannot
/// go through `hits`, because its two gestures are claimed at opposite ends
/// of its own subtree's dispatch and `hits` claims at one point only - so
/// the protocol has to be kept here by hand, and that is what the first
/// arm is. It is not a nicety: this widget floats in window points over
/// panes that are asked about a press before the pane it lives in, and
/// without this one press both starts a drag here and does whatever that
/// pane does with it - resizes the split, or opens a different page and
/// takes the surface down with it.
///
/// The `Sheet` arm claims whether or not either switch is on, which is the
/// other half. A sheet of painted glass that let a press through would be
/// answering with whatever is behind it, which nobody can see.
///
/// The same decision answers a HOVER. A sheet that claimed the press but
/// left the hover open would light up a field under the glass and paint its
/// I-beam over painted glass, and then answer the click itself; the pointer
/// and the press have to agree about who owns a place.
fn press_on(
    frame: Frame,
    at: Vec2d,
    grab: f64,
    handled: bool,
    movable: bool,
    resizable: bool,
) -> Press {
    if handled {
        return Press::Taken;
    }
    if resizable {
        let grip = frame.grip_at(at, grab);
        if !grip.is_empty() {
            return Press::Size(grip);
        }
    }
    let rect = Rect {
        pos: frame.pos,
        size: frame.size,
    };
    if rect.contains(at) {
        return Press::Sheet { moves: movable };
    }
    Press::Elsewhere
}

/// What the pointer is doing to the surface.
#[derive(Copy, Clone, Debug)]
enum Drag {
    /// Moving it: where the press was, and where the surface was.
    Move { held_at: Vec2d, from: Vec2d },
    /// Sizing it: which sides, where the press was, and the frame it
    /// started from. The frame is captured at the press so every step of
    /// the drag is measured from the same place — accumulating deltas
    /// instead lets a clamped edge lose track of the finger.
    Size { grip: Grip, held_at: Vec2d, from: Frame },
}

/// A sheet of glass that floats over the page, moved by its body and sized
/// by its edges and its corners.
///
/// The material is what makes this different from `FloatingPanel`, and the
/// difference is not decoration. The lens reads the scene BEHIND the
/// surface, so a drag has to leave the glass looking at where it is now:
/// every frame of a move or a resize redraws the surface's whole subtree,
/// which is what makes it ask the window for a fresh capture. A repaint that
/// reuses the drawn content asks for nothing, and the lens then carries a
/// photograph of the part of the screen the drag started on.
///
/// **It has no title bar.** A bar would be the obvious handle and it is the
/// wrong one here: the surface is one sheet, and a strip of chrome across
/// the top of it is exactly what this family exists not to draw. The body
/// moves it instead, and the move is claimed AFTER the contents have had the
/// press — so a button on the surface still answers a click, and only what
/// nothing inside wanted moves the surface.
///
/// **The frame is claimed the other way round**, before the contents see the
/// press: the band is a few points wide and lies over whatever the caller
/// put against the edge, and a resize that begins by dropping a caret into a
/// field is a resize the person then has to undo.
///
/// **A press another widget already answered is not this surface's.** It
/// floats in window points and can therefore lie over panes that are asked
/// about a press before the pane it lives in; the press belongs to whoever
/// took it first, so a drag cannot be STARTED on the part of the sheet that
/// overlaps one — and on a shell where that pane answers a press by opening
/// a different page, the attempt does not merely fail, it takes the page the
/// surface is standing on with it. Once a drag has begun it carries on over
/// anything, because nothing else holds the pointer.
///
/// **The pointer, the press and the wheel are claimed together.** A hover
/// over the sheet is marked handled exactly as a press is — before the
/// contents for the frame band, after them for the body — so a control under
/// the glass does not light up and offer a caret for a click it will never
/// get. A wheel over the sheet is stopped after the contents have had it, so
/// the surface's own body still scrolls and the page underneath does not
/// slide out from under a sheet that stays put.
///
/// **And the keystrokes go with the press.** Every press the surface answers
/// takes the key focus, the resize band included, so a search box elsewhere
/// stops eating keys the moment the sheet is worked. Closing asks where the
/// caret IS, not who put it there: one anywhere the surface draws goes back
/// to whatever the surface took it from, or is dropped where the surface
/// never took it (a `TextInput` a caller put on the glass took it directly,
/// and there is nothing to go back to); one that has since moved OUT of the
/// surface is left exactly where it is, because it belongs to whatever took
/// it — on a page with a "hide it" button, that button.
///
/// **The grab band costs a ring of page.** It reaches `grab_margin` points
/// outward as well as inward, so presses that far outside the painted glass
/// belong to the surface, with nothing drawn there to say so. The outward
/// half is not optional — a rounded rectangle's corners are unpainted — but
/// it is a reason to keep `grab_margin` small.
///
/// **It is not reachable by touch.** The gesture is written against
/// `Event::MouseDown`/`MouseMove`/`MouseUp` and not `Event::TouchUpdate`. A
/// touch path is expressible — `TouchPoint` carries its own `handled`, so
/// the before-and-after reading works the same way — but it needs a second
/// state machine to track which touch owns the drag, and nothing in this
/// tree exercises it. Until that is written, this widget is desktop only.
///
/// What it deliberately does NOT do: snap to anything, settle anywhere,
/// remember where it was, or paint a scrim. It reports its frame and the
/// caller keeps the value if it wants it back next run — a widget that
/// writes files has learned something it has no business knowing.
#[derive(Script, Widget)]
pub struct GlassFloatingSurface {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    /// The corner mark, drawn last and INSIDE the same overlay list as the
    /// glass. A quad the parent draws after a glass child is painted over
    /// by the lens (the draw-order rule in `gauss_view`); one drawn into the
    /// overlay after it is not.
    #[live]
    draw_grip: DrawQuad,

    /// Where it sits and how big, in window points.
    #[live(Vec2d { x: 120., y: 120. })]
    pub pos: Vec2d,
    #[live(Vec2d { x: 320., y: 220. })]
    pub size: Vec2d,
    #[live(Vec2d { x: 140., y: 96. })]
    pub min_size: Vec2d,
    /// A zero side means the window is the only ceiling.
    #[live]
    pub max_size: Vec2d,
    /// How far either side of an edge a press still takes hold of it.
    ///
    /// The band reaches this far OUTSIDE the painted glass as well as
    /// inside, so it is also the width of a ring of page around the surface
    /// that answers to the surface, with nothing drawn there to say so. The
    /// outward half is not optional — a rounded rectangle's corners are
    /// unpainted, and a band that stopped at the boundary would ask for a
    /// press on glass that is not there — but it is the reason to keep this
    /// number small, and the reason the exposed range stops at 16.
    #[live(8.0)]
    pub grab_margin: f64,
    /// The corner mark's side. Zero draws none; the corner still grabs.
    #[live(24.0)]
    pub grip_size: f64,
    #[live(true)]
    pub movable: bool,
    #[live(true)]
    pub resizable: bool,

    /// Whether it is up.
    ///
    /// Live rather than runtime state, so a page can put one on screen by
    /// declaring it and a reload brings it back the way the page asked for
    /// it. The consequence is that a live edit or a theme switch closes one
    /// a person opened by hand and reopens one the page declared — the same
    /// price every other live property pays. Named `shown` and not `open`
    /// because `open` is already a method this widget answers from script.
    #[live]
    pub shown: bool,
    #[rust]
    drag: Option<Drag>,
    /// Whether the pointer showing now is one we set, and so ours to put
    /// back when it leaves the frame.
    #[rust]
    holds_cursor: bool,
    /// Whether the keystrokes showing here are ones this surface took, and
    /// so ours to hand back when it goes away.
    #[rust]
    holds_key_focus: bool,
    /// What had the keystrokes at the moment this surface first took them,
    /// so they can go back there. Empty means "nothing worth going back
    /// to", which is also what a caret already inside the surface counts as.
    #[rust]
    focus_before: Area,
    /// The corner mark's hover mix.
    #[rust]
    hot: f32,
}

impl ScriptHook for GlassFloatingSurface {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        // The surface takes NO room in the layout that holds it: it paints
        // on its own overlay against a root turtle for the pass. Reporting
        // `Fill` upward would make it a deferred fill and hand it a share of
        // its parent's spare height for a surface that may not even be up.
        self.view.walk = Walk::empty();
        vm.with_cx_mut(|cx| {
            if self.shown {
                // The same route `open` takes, and for the same reason. A
                // whole-tree apply re-applies the glass inside, which
                // announces itself; an EVAL apply does not — it touches this
                // object's own properties and stops — so `{shown: true}`
                // written by the design overlay or by a controls panel
                // reaches here with nothing else having said a word.
                self.appear(cx);
            } else {
                self.drop_key_focus(cx);
                self.redraw(cx);
            }
        });
    }
}

impl GlassFloatingSurface {
    /// The frame as it stands, built fresh each time: every number in it is
    /// a live property and the tweaker may have moved any of them since the
    /// last draw.
    fn frame(&self) -> Frame {
        Frame {
            pos: self.pos,
            size: self.size,
            min: self.min_size,
            max: self.max_size,
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.view.redraw(cx);
    }

    /// Redraw after a drag moved or sized the surface.
    ///
    /// The whole subtree, not just this widget's own list. The glass paints
    /// what is behind it from a capture the window takes only when a draw
    /// asks for one, and the ask happens inside the surface's own draw. A
    /// repaint that reuses the content's draw list never asks, the window
    /// stops capturing, and the lens goes on showing the scene as it was
    /// when the drag began — the surface then carries a picture of the wrong
    /// part of the screen around with it, which is worse than no glass at
    /// all because it looks deliberate.
    fn redraw_over_the_scene(&mut self, cx: &mut Cx, area: Area) {
        cx.redraw_area_and_children(area);
        self.redraw(cx);
    }

    /// Take the keystrokes.
    ///
    /// Any press the surface answers takes them, not only the one that
    /// starts a move. A person working a sheet of glass has stopped typing
    /// into whatever had the caret, and a search box that goes on eating
    /// keys while the surface is being dragged is what follows from taking
    /// focus on the move alone.
    ///
    /// Where they came from is remembered here rather than left to
    /// `cx.revert_key_focus`. That reverts to whatever held the focus one
    /// step ago, and after (press the sheet, click a control on the glass,
    /// press the sheet again) one step ago is a widget INSIDE the surface —
    /// precisely the one that stops being drawn. What is worth going back
    /// to is the last holder that was NOT the surface's own, so a focus
    /// already inside it does not overwrite the answer.
    fn take_key_focus(&mut self, cx: &mut Cx, area: Area) {
        if !self.focus_is_inside(cx) {
            self.focus_before = cx.key_focus();
        }
        self.holds_key_focus = true;
        cx.set_key_focus(area);
    }

    /// Whether the caret is somewhere this surface draws.
    ///
    /// Not `cx.has_key_focus(content_area)`: a `TextInput` a caller put ON
    /// the glass takes the focus for ITSELF, and that focus is just as much
    /// this surface's to clean up — it sits on a widget that stops being
    /// drawn the moment the surface goes away. Everything the surface paints
    /// goes into its own overlay draw list or into a list nested under it
    /// (a glass child opens its own overlay, whose codeflow parent is this
    /// one), so walking that chain is the question "is this mine" asked
    /// exactly.
    fn focus_is_inside(&self, cx: &Cx) -> bool {
        let Some(draw_list) = &self.draw_list else {
            return false;
        };
        let mine = draw_list.id();
        let mut next = cx.key_focus().draw_list_id();
        while let Some(id) = next {
            if id == mine {
                return true;
            }
            next = cx
                .draw_lists
                .checked_index(id)
                .and_then(|list| list.codeflow_parent_id);
        }
        false
    }

    /// Give the keystrokes back on the way out. One that goes away with the
    /// caret still on it leaves the keystrokes going to an area nothing
    /// draws any more.
    ///
    /// The question is where the caret IS, not who put it there. A caret
    /// outside the surface is not the surface's to move — by the time this
    /// runs from an action handler the page's own "hide it" button has
    /// already taken the focus for itself, because key focus is promoted
    /// before actions are handled, and snatching it back off the widget the
    /// person just pressed would be worse than doing nothing.
    ///
    /// A caret inside goes back to whatever the surface took it from, and
    /// where that is not known — a `TextInput` a caller put on the glass
    /// took it directly, so the surface never took anything — it is
    /// dropped. Nothing is a better answer than a widget that is not drawn.
    fn drop_key_focus(&mut self, cx: &mut Cx) {
        let took_it = self.holds_key_focus;
        let back = self.focus_before;
        self.holds_key_focus = false;
        self.focus_before = Area::Empty;
        if !self.focus_is_inside(cx) {
            return;
        }
        cx.set_key_focus(if took_it { back } else { Area::Empty });
    }

    /// Come up, or arrive somewhere new, with the lens looking at where it
    /// is now.
    ///
    /// The announcement is the part a redraw cannot do. A window decides
    /// whether to capture the scene a glass surface refracts BEFORE any
    /// widget draws, on the evidence of what asked last frame and what
    /// announced itself since. A surface built closed announced itself once,
    /// at build time, and that arm was spent on the next frame with nothing
    /// drawn; so the frame it is finally shown on has no capture behind it,
    /// it paints its flat face, and the window then redraws the whole UI to
    /// correct itself.
    ///
    /// The subtree is redrawn rather than this widget's own list because
    /// that is what a drag does, and a surface that arrives somewhere new is
    /// in the same position as one dragged there. Two routes for one
    /// situation is how they drift apart.
    fn appear(&mut self, cx: &mut Cx) {
        arm_gauss_capture(cx);
        let area = self.view.widget(cx, ids!(content)).area();
        self.redraw_over_the_scene(cx, area);
    }

    pub fn open(&mut self, cx: &mut Cx) {
        if !self.shown {
            self.shown = true;
            self.drag = None;
            self.appear(cx);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if self.shown {
            self.shown = false;
            self.drag = None;
            self.drop_key_focus(cx);
            // Nothing to announce: it is going away.
            self.redraw(cx);
        }
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.shown {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.shown
    }

    /// Where it is and how big, in window points.
    pub fn placement(&self) -> (Vec2d, Vec2d) {
        (self.pos, self.size)
    }

    /// Put it somewhere. The next draw settles the frame against the window
    /// and the floor, so a caller may hand this whatever it saved.
    pub fn place(&mut self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        self.pos = pos;
        self.size = size;
        if self.shown {
            self.appear(cx);
        } else {
            // Nothing to announce for a surface that is not up: the arm is
            // taken on the next frame whether or not any glass drew, so an
            // announcement made here is spent on a frame with nothing to
            // capture for, and the frame it is finally shown on has none.
            self.redraw(cx);
        }
    }

    /// The corner mark's square, inside the bottom-right corner.
    fn grip_rect(&self) -> Rect {
        let side = self.grip_size.min(self.size.x).min(self.size.y).max(0.0);
        Rect {
            pos: dvec2(
                self.pos.x + self.size.x - side,
                self.pos.y + self.size.y - side,
            ),
            size: dvec2(side, side),
        }
    }

    fn say(&mut self, cx: &mut Cx, action: GlassFloatingSurfaceAction) {
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
    }
}

impl Widget for GlassFloatingSurface {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.shown {
            return;
        }
        let content = self.view.widget(cx, ids!(content));
        let area = content.area();
        // What the page BENEATH the surface had already claimed before this
        // widget was reached. It is read again once the contents have run,
        // and the only handler between the two reads is the surface's own
        // subtree — so a change means something INSIDE wanted the event.
        // Nothing else can tell us that: a press on a control behind the
        // surface has marked the event handled long before this widget is
        // reached at all, and reading `handled` on its own confuses the two.
        let claimed_before = match event {
            Event::MouseDown(me) => Some(me.handled.get()),
            Event::MouseMove(me) => Some(me.handled.get()),
            _ => None,
        };
        // A press on the sheet, waiting to see whether the contents want it:
        // where it landed, where the surface was, and whether it may move.
        let mut offered_sheet: Option<(Vec2d, Vec2d, bool)> = None;
        // A HOVER over the sheet, waiting on the same question. The surface
        // claims it if nothing inside wanted it, so that the widgets behind
        // the glass stop lighting up for a press they will never get.
        let mut offered_hover = false;
        // What the frame made of a pointer move that started no drag. It
        // decides the pointer AND whether the pointer showing is still ours
        // to put back — the two have to come from one answer, or a move from
        // the edge band onto the body leaves a resize arrow over the sheet.
        let mut hover_verdict = None;
        // The pointer the FRAME wants, decided before the contents are given
        // the event and applied after them: a control under the grab band
        // would otherwise set its own pointer last and win the argument.
        let mut frame_cursor = None;

        match event {
            Event::MouseDown(me) if self.drag.is_none() => {
                let frame = self.frame();
                match press_on(
                    frame,
                    me.abs,
                    self.grab_margin,
                    !me.handled.get().is_empty(),
                    self.movable,
                    self.resizable,
                ) {
                    // Not ours. Fall through so the contents still see it —
                    // they will refuse it for the same reason — rather than
                    // returning and cutting the dispatch short.
                    Press::Taken | Press::Elsewhere => {}
                    Press::Size(grip) if me.button.is_primary() => {
                        // The frame claims a press BEFORE the contents see
                        // it. The band is a few points wide and lies over
                        // whatever the caller put against the edge, and a
                        // resize that begins by dropping a caret into a
                        // field is a resize the person then has to undo.
                        self.drag = Some(Drag::Size {
                            grip,
                            held_at: me.abs,
                            from: frame,
                        });
                        // Nobody after us answers this press either. The page
                        // under the surface keeps working — it just does not
                        // get to act on a press aimed at the surface's edge.
                        me.handled.set(area);
                        self.take_key_focus(cx, area);
                        cx.set_cursor(grip.cursor());
                        self.hot = 1.0;
                        self.redraw(cx);
                        return;
                    }
                    // A secondary press sizes nothing, so the band is only
                    // more sheet to it: the contents get first refusal and
                    // the surface claims whatever is left, exactly as on the
                    // body. Claiming it matters even though nothing in this
                    // tree acts on a right-press — a press that fell through
                    // painted glass would be answered by a widget nobody can
                    // see, whichever button made it.
                    Press::Size(_) => {
                        offered_sheet = Some((me.abs, self.pos, false));
                    }
                    Press::Sheet { moves } => {
                        // The contents have first refusal; settled below.
                        offered_sheet = Some((me.abs, self.pos, moves && me.button.is_primary()));
                    }
                }
            }
            Event::MouseMove(me) => match self.drag {
                Some(Drag::Size {
                    grip,
                    held_at,
                    from,
                }) => {
                    let (pos, size) = from.resized(grip, me.abs - held_at);
                    let moved = (pos, size) != (self.pos, self.size);
                    self.pos = pos;
                    self.size = size;
                    cx.set_cursor(grip.cursor());
                    if moved {
                        self.say(cx, GlassFloatingSurfaceAction::Sizing { pos, size });
                        self.redraw_over_the_scene(cx, area);
                    }
                    return;
                }
                Some(Drag::Move { held_at, from }) => {
                    let pos = from + (me.abs - held_at);
                    if pos != self.pos {
                        self.pos = pos;
                        let size = self.size;
                        self.say(cx, GlassFloatingSurfaceAction::Moving { pos, size });
                        // Moving changes what is behind the glass every bit
                        // as much as sizing does, so it needs the same
                        // refresh.
                        self.redraw_over_the_scene(cx, area);
                    }
                    cx.set_cursor(MouseCursor::Move);
                    return;
                }
                None => {
                    // The same decision a press takes, taken again for the
                    // pointer. A hover marks a move handled exactly as a
                    // press marks a press, so this is where the surface says
                    // "this place is mine" to the widgets it floats over —
                    // and a move that arrives here ALREADY claimed is a move
                    // over a widget that will answer the press as well, so
                    // the frame promises nothing there.
                    //
                    // The two drag branches above are deliberately NOT gated
                    // this way: a drag that has begun owns the pointer and
                    // has to keep hearing about it wherever it goes.
                    let verdict = press_on(
                        self.frame(),
                        me.abs,
                        self.grab_margin,
                        !me.handled.get().is_empty(),
                        self.movable,
                        self.resizable,
                    );
                    hover_verdict = Some(verdict);
                    let mut hot = 0.0;
                    match verdict {
                        Press::Size(grip) => {
                            // Before the contents, as the press is: a control
                            // under the band cannot answer the press, so it
                            // must not answer the hover either.
                            me.handled.set(area);
                            frame_cursor = Some(grip.cursor());
                            hot = 1.0;
                        }
                        // After the contents, as the press is — settled once
                        // they have had their turn.
                        Press::Sheet { .. } => offered_hover = true,
                        Press::Taken | Press::Elsewhere => {}
                    }
                    if hot != self.hot {
                        self.hot = hot;
                        self.redraw(cx);
                    }
                }
            },
            Event::MouseUp(_) => {
                if self.drag.take().is_some() {
                    self.hot = 0.0;
                    let (pos, size) = (self.pos, self.size);
                    self.say(cx, GlassFloatingSurfaceAction::Placed { pos, size });
                    self.redraw(cx);
                    return;
                }
            }
            _ => {}
        }

        content.handle_event(cx, event, scope);

        // The sheet is claimed AFTER the contents, which is the whole reason
        // it can hold controls: a press a button on the glass took has
        // changed `handled` by now, and the surface leaves it alone.
        if let (Some((held_at, from, moves)), Event::MouseDown(me)) = (offered_sheet, event) {
            if me.handled.get() == claimed_before.unwrap_or_default() {
                me.handled.set(area);
                self.take_key_focus(cx, area);
                if moves {
                    self.drag = Some(Drag::Move { held_at, from });
                }
            }
        }

        // And the hover with it, on the same test.
        if let (true, Event::MouseMove(me)) = (offered_hover, event) {
            if me.handled.get() == claimed_before.unwrap_or_default() {
                me.handled.set(area);
                // A sheet of glass has no pointer of its own — but the one
                // showing may be an I-beam a field under it set before the
                // surface arrived over it, and that field will not clear it.
                frame_cursor = Some(MouseCursor::Arrow);
            }
        }

        // The wheel, claimed after the contents for the same reason and with
        // the same effect. The surface's own body scrolls first; a wheel it
        // had no use for stops here rather than scrolling the page out from
        // under a sheet that stays put.
        if let Event::Scroll(se) = event {
            let rect = Rect {
                pos: self.pos,
                size: self.size,
            };
            if rect.contains(se.abs) {
                se.handled_x.set(true);
                se.handled_y.set(true);
            }
        }

        if let Event::MouseMove(_) = event {
            if let Some(cursor) = frame_cursor {
                cx.set_cursor(cursor);
                self.holds_cursor = true;
            } else if self.holds_cursor && hover_verdict == Some(Press::Elsewhere) {
                // The pointer has left the surface altogether, so the one
                // showing is still the one we set and ours to put back. The
                // other three verdicts must NOT restore: `Taken` means a pane
                // dispatched before us owns this move and set its own
                // pointer, and a `Sheet` that reached here without a cursor
                // is one a control on the glass took.
                cx.set_cursor(MouseCursor::Arrow);
                self.holds_cursor = false;
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);

        if self.shown {
            // Never bigger than the window and never off it: a surface whose
            // frame has gone past an edge cannot be dragged back, which
            // would make losing it permanent.
            let (pos, size) = self.frame().settled(cx.current_pass_size());
            self.pos = pos;
            self.size = size;

            let content = self.view.widget(cx.cx.cx, ids!(content));
            let mut walk = Walk::new(Size::Fixed(size.x), Size::Fixed(size.y));
            walk.abs_pos = Some(pos);
            content.draw_walk_all(cx, scope, walk);

            // The corner mark goes last, in the same overlay list, so it
            // sits ON the glass rather than under it.
            if self.resizable && self.grip_size > 0.0 {
                let grip_rect = self.grip_rect();
                self.draw_grip
                    .draw_vars
                    .set_uniform(cx, live_id!(hover), &[self.hot]);
                self.draw_grip.draw_abs(cx, grip_rect);
            }
        }

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(open) {
            vm.with_cx_mut(|cx| self.open(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(close) {
            vm.with_cx_mut(|cx| self.close(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(toggle) {
            vm.with_cx_mut(|cx| self.toggle(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }
}

impl GlassFloatingSurfaceRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// Where it is and how big, for a caller that wants to put it back.
    pub fn placement(&self) -> Option<(Vec2d, Vec2d)> {
        self.borrow().map(|inner| inner.placement())
    }

    pub fn place(&self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.place(cx, pos, size);
        }
    }

    /// The new size, while a drag on an edge or a corner is making it.
    pub fn sizing(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<GlassFloatingSurfaceAction>() {
            GlassFloatingSurfaceAction::Sizing { pos, size } => Some((pos, size)),
            _ => None,
        }
    }

    /// Where it is, while a drag on the body is moving it.
    pub fn moving(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<GlassFloatingSurfaceAction>() {
            GlassFloatingSurfaceAction::Moving { pos, size } => Some((pos, size)),
            _ => None,
        }
    }

    /// Where a drag just left it, if one did.
    pub fn placed(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<GlassFloatingSurfaceAction>() {
            GlassFloatingSurfaceAction::Placed { pos, size } => Some((pos, size)),
            _ => None,
        }
    }

    /// The frame this surface reports this pass, whichever kind of drag
    /// reported it — for a host that only wants to follow the frame and does
    /// not care which handle is doing it.
    pub fn framed(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<GlassFloatingSurfaceAction>() {
            GlassFloatingSurfaceAction::Sizing { pos, size }
            | GlassFloatingSurfaceAction::Moving { pos, size }
            | GlassFloatingSurfaceAction::Placed { pos, size } => Some((pos, size)),
            GlassFloatingSurfaceAction::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A surface at 100,100 sized 200x150, with the preset's floor and no
    /// ceiling of its own.
    fn surface() -> Frame {
        Frame {
            pos: dvec2(100.0, 100.0),
            size: dvec2(200.0, 150.0),
            min: dvec2(140.0, 96.0),
            max: dvec2(0.0, 0.0),
        }
    }

    fn grip(left: bool, right: bool, top: bool, bottom: bool) -> Grip {
        Grip {
            left,
            right,
            top,
            bottom,
        }
    }

    /// The `script_mod!` block is invisible to the Rust compiler: a mistake
    /// in it shows up only in a running app's log, and a shader that fails
    /// to compile is not an error anywhere — the draw is simply skipped and
    /// the widget paints nothing. This family now takes its no-capture face
    /// from two theme tokens rather than from a literal, in four different
    /// shaders, and multiplies a new input into the shadow of three more.
    /// Evaluating the crate's whole script module here, building one of
    /// each preset from it and reading the shader-error slot back is what
    /// turns a mistake in either into a failed build.
    #[test]
    fn the_glass_presets_build_and_their_shaders_compile() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (surface, button, slider, segmented) = cx.with_vm(|vm| {
            crate::script_mod(vm);
            // Registering type defaults compiles nothing; making an
            // instance out of one does. Clearing here keeps any other
            // module's complaint out of this test's answer.
            let _ = crate::makepad_draw::makepad_platform::shader_error::take();
            (
                GlassFloatingSurface::script_new_with_default(vm),
                GlassButton::script_new_with_default(vm),
                GlassSlider::script_new_with_default(vm),
                GlassSegmented::script_new_with_default(vm),
            )
        });
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // Numbers only the DSL sets: the Rust defaults for these are false
        // and zero.
        assert!(!surface.shown, "the preset ships closed, so a page has to say so");
        assert!(surface.movable && surface.resizable);
        assert_eq!(surface.grab_margin, 8.0);
        assert_eq!(surface.grip_size, 24.0);
        assert_eq!(surface.size, dvec2(320.0, 220.0));
        // And the three hand-written quads reached their own presets.
        assert_eq!(button.walk.height.to_fixed(), Some(44.0));
        assert_eq!(slider.walk.height.to_fixed(), Some(32.0));
        assert_eq!(segmented.walk.height.to_fixed(), Some(38.0));
    }

    /// A press someone answered before this widget was reached is not this
    /// surface's, whatever part of it the press landed on. The surface
    /// floats in window points over panes that are asked first; without
    /// this, one press both starts a drag here and does whatever that pane
    /// does with it.
    #[test]
    fn a_press_someone_answered_already_is_not_the_surfaces() {
        let f = surface();
        assert_eq!(press_on(f, dvec2(100.0, 175.0), 8.0, true, true, true), Press::Taken);
        assert_eq!(press_on(f, dvec2(200.0, 175.0), 8.0, true, true, true), Press::Taken);
        // The same two presses, with nobody having taken them first.
        assert_eq!(
            press_on(f, dvec2(100.0, 175.0), 8.0, false, true, true),
            Press::Size(grip(true, false, false, false)),
            "the left edge is the surface's when it is free"
        );
        assert_eq!(
            press_on(f, dvec2(200.0, 175.0), 8.0, false, true, true),
            Press::Sheet { moves: true },
            "and so is the body"
        );
    }

    /// A sheet that can neither be moved nor sized still answers a press on
    /// itself. It is painted glass over the page: a press that fell through
    /// would be answered by something nobody can see.
    #[test]
    fn a_sheet_that_cannot_move_or_size_still_claims_its_own_press() {
        let f = surface();
        assert_eq!(
            press_on(f, dvec2(200.0, 175.0), 8.0, false, false, false),
            Press::Sheet { moves: false }
        );
        assert_eq!(
            press_on(f, dvec2(100.0, 175.0), 8.0, false, false, false),
            Press::Sheet { moves: false },
            "with sizing switched off the edge is just more sheet"
        );
        assert_eq!(
            press_on(f, dvec2(500.0, 500.0), 8.0, false, false, false),
            Press::Elsewhere,
            "and the page beyond it is still the page's"
        );
    }

    /// The frame is decided before the sheet, and the outward half of the
    /// band belongs to the surface while the page a few points further out
    /// does not.
    #[test]
    fn the_frame_is_taken_before_the_sheet_and_the_page_after_it() {
        let f = surface();
        assert_eq!(
            press_on(f, dvec2(94.0, 175.0), 8.0, false, true, true),
            Press::Size(grip(true, false, false, false)),
            "outside the painted glass but inside the band"
        );
        assert_eq!(
            press_on(f, dvec2(91.0, 175.0), 8.0, false, true, true),
            Press::Elsewhere,
            "three points further out and it is the page's"
        );
        assert_eq!(
            press_on(f, dvec2(100.0, 175.0), 8.0, false, true, false),
            Press::Sheet { moves: true },
            "with sizing off, a press on the edge moves it instead"
        );
    }

    /// Each edge moves its own side and leaves the other three alone. The
    /// far edge staying put is the whole of what "dragging an edge" means,
    /// and it is the half a naive resize gets wrong.
    #[test]
    fn each_edge_moves_only_its_own_side() {
        let f = surface();
        assert_eq!(
            f.resized(grip(false, true, false, false), dvec2(40.0, 0.0)),
            (dvec2(100.0, 100.0), dvec2(240.0, 150.0)),
            "the right edge grows the width and does not move the surface"
        );
        assert_eq!(
            f.resized(grip(true, false, false, false), dvec2(40.0, 0.0)),
            (dvec2(140.0, 100.0), dvec2(160.0, 150.0)),
            "the left edge moves in and the right edge stays at 300"
        );
        assert_eq!(
            f.resized(grip(false, false, false, true), dvec2(0.0, 30.0)),
            (dvec2(100.0, 100.0), dvec2(200.0, 180.0)),
            "the bottom edge grows the height"
        );
        assert_eq!(
            f.resized(grip(false, false, true, false), dvec2(0.0, -20.0)),
            (dvec2(100.0, 80.0), dvec2(200.0, 170.0)),
            "the top edge moves up and the bottom stays at 250"
        );
    }

    /// A corner is both of its edges at once, and neither of the other two.
    #[test]
    fn each_corner_moves_both_of_its_sides() {
        let f = surface();
        assert_eq!(
            f.resized(grip(false, true, false, true), dvec2(50.0, 40.0)),
            (dvec2(100.0, 100.0), dvec2(250.0, 190.0)),
            "bottom-right grows both and moves nothing"
        );
        assert_eq!(
            f.resized(grip(true, false, true, false), dvec2(-20.0, -10.0)),
            (dvec2(80.0, 90.0), dvec2(220.0, 160.0)),
            "top-left moves the surface and grows it by the same amount"
        );
        assert_eq!(
            f.resized(grip(true, false, false, true), dvec2(20.0, 25.0)),
            (dvec2(120.0, 100.0), dvec2(180.0, 175.0)),
            "bottom-left moves in on x only"
        );
        assert_eq!(
            f.resized(grip(false, true, true, false), dvec2(30.0, 15.0)),
            (dvec2(100.0, 115.0), dvec2(230.0, 135.0)),
            "top-right moves down on y only"
        );
    }

    /// Dragged past the floor, the edge under the finger stops and the far
    /// edge does not budge. Clamping the position instead of the size would
    /// shove the far edge along, quietly moving a surface the person was
    /// only trying to make smaller.
    #[test]
    fn the_floor_pins_the_dragged_edge_and_spares_the_far_one() {
        let f = surface();
        let (pos, size) = f.resized(grip(true, false, false, false), dvec2(400.0, 0.0));
        assert_eq!(size.x, 140.0, "stopped at the floor");
        assert_eq!(pos.x + size.x, 300.0, "and the right edge never moved");

        let (pos, size) = f.resized(grip(false, false, true, false), dvec2(400.0, 400.0));
        assert_eq!(size.y, 96.0);
        assert_eq!(pos.y + size.y, 250.0, "the bottom edge never moved");

        // The far edges do the same thing, without moving the surface.
        let (pos, size) = f.resized(grip(false, true, false, true), dvec2(-500.0, -500.0));
        assert_eq!((pos, size), (dvec2(100.0, 100.0), dvec2(140.0, 96.0)));
    }

    /// A ceiling stops a drag the same way a floor does, and a zero ceiling
    /// is no ceiling — which is what the preset ships, so it must not be
    /// mistaken for "may not be wider than nothing".
    #[test]
    fn a_ceiling_stops_the_drag_and_a_zero_one_does_not_exist() {
        let mut f = surface();
        f.max = dvec2(260.0, 0.0);
        let (pos, size) = f.resized(grip(false, true, false, true), dvec2(900.0, 900.0));
        assert_eq!(size.x, 260.0, "the ceiling held");
        assert_eq!(size.y, 1050.0, "and a zero ceiling did not");
        assert_eq!(pos, dvec2(100.0, 100.0));

        // A ceiling under the floor loses to the floor rather than
        // inverting the clamp, which would panic.
        f.max = dvec2(10.0, 10.0);
        let (_, size) = f.resized(grip(false, true, false, true), dvec2(900.0, 900.0));
        assert_eq!(size, dvec2(140.0, 96.0));
    }

    /// The hit test finds each edge and each corner where they are drawn,
    /// and finds nothing in the body — which is what leaves the body free to
    /// move the surface.
    #[test]
    fn the_hit_test_finds_each_edge_and_each_corner() {
        let f = surface();
        let grab = 8.0;
        assert_eq!(f.grip_at(dvec2(100.0, 175.0), grab), grip(true, false, false, false));
        assert_eq!(f.grip_at(dvec2(300.0, 175.0), grab), grip(false, true, false, false));
        assert_eq!(f.grip_at(dvec2(200.0, 100.0), grab), grip(false, false, true, false));
        assert_eq!(f.grip_at(dvec2(200.0, 250.0), grab), grip(false, false, false, true));
        assert_eq!(f.grip_at(dvec2(100.0, 100.0), grab), grip(true, false, true, false));
        assert_eq!(f.grip_at(dvec2(300.0, 100.0), grab), grip(false, true, true, false));
        assert_eq!(f.grip_at(dvec2(100.0, 250.0), grab), grip(true, false, false, true));
        assert_eq!(f.grip_at(dvec2(300.0, 250.0), grab), grip(false, true, false, true));
        assert!(f.grip_at(dvec2(200.0, 175.0), grab).is_empty(), "the body");
        assert!(f.grip_at(dvec2(500.0, 500.0), grab).is_empty(), "the page");
    }

    /// The band reaches both ways from the edge, so the unpainted bite a
    /// rounded corner takes out of the square is still grabbable, and a
    /// press further out than that belongs to the page.
    #[test]
    fn the_band_reaches_both_sides_of_an_edge_and_no_further() {
        let f = surface();
        assert_eq!(f.grip_at(dvec2(94.0, 175.0), 8.0), grip(true, false, false, false));
        assert_eq!(f.grip_at(dvec2(106.0, 175.0), 8.0), grip(true, false, false, false));
        assert!(f.grip_at(dvec2(91.0, 175.0), 8.0).is_empty());
        assert!(f.grip_at(dvec2(110.0, 175.0), 8.0).is_empty());
    }

    /// On a surface narrower than two bands the two overlap; one finger then
    /// gets one edge — the nearer — rather than both ends at once.
    #[test]
    fn overlapping_bands_give_the_nearer_edge_only() {
        let f = Frame {
            pos: dvec2(0.0, 0.0),
            size: dvec2(20.0, 20.0),
            min: dvec2(10.0, 10.0),
            max: dvec2(0.0, 0.0),
        };
        assert_eq!(f.grip_at(dvec2(9.0, 9.0), 12.0), grip(true, false, true, false));
        assert_eq!(f.grip_at(dvec2(11.0, 11.0), 12.0), grip(false, true, false, true));
    }

    /// A surface is pulled back inside the window, and cut down to it if it
    /// is bigger — but never below its own floor, because a surface with no
    /// size left cannot be dragged anywhere at all.
    #[test]
    fn a_surface_is_settled_inside_the_window() {
        let f = surface();
        assert_eq!(
            f.settled(dvec2(1000.0, 800.0)),
            (dvec2(100.0, 100.0), dvec2(200.0, 150.0)),
            "already inside: untouched"
        );

        let mut off = surface();
        off.pos = dvec2(950.0, 780.0);
        assert_eq!(
            off.settled(dvec2(1000.0, 800.0)),
            (dvec2(800.0, 650.0), dvec2(200.0, 150.0)),
            "pulled back so its far edges sit on the window's"
        );

        let mut huge = surface();
        huge.size = dvec2(2000.0, 2000.0);
        assert_eq!(
            huge.settled(dvec2(1000.0, 800.0)),
            (dvec2(0.0, 0.0), dvec2(1000.0, 800.0)),
            "cut down to the window"
        );

        let mut tiny_window = surface();
        tiny_window.size = dvec2(2000.0, 2000.0);
        assert_eq!(
            tiny_window.settled(dvec2(40.0, 40.0)).1,
            dvec2(140.0, 96.0),
            "a window smaller than the floor loses to the floor"
        );
    }

    /// The two diagonals are different pointers. A corner that shows the
    /// wrong one tells the hand it will do something it will not.
    #[test]
    fn each_corner_shows_its_own_diagonal_pointer() {
        assert_eq!(grip(true, false, true, false).cursor(), MouseCursor::NwseResize);
        assert_eq!(grip(false, true, false, true).cursor(), MouseCursor::NwseResize);
        assert_eq!(grip(false, true, true, false).cursor(), MouseCursor::NeswResize);
        assert_eq!(grip(true, false, false, true).cursor(), MouseCursor::NeswResize);
        assert_eq!(grip(true, false, false, false).cursor(), MouseCursor::EwResize);
        assert_eq!(grip(false, false, false, true).cursor(), MouseCursor::NsResize);
        assert_eq!(Grip::default().cursor(), MouseCursor::Arrow);
    }

    /// A grab margin of zero leaves the edges exactly on the boundary rather
    /// than making the whole surface a grip or none of it one.
    #[test]
    fn a_zero_grab_margin_is_the_edge_itself() {
        let f = surface();
        assert_eq!(f.grip_at(dvec2(100.0, 175.0), 0.0), grip(true, false, false, false));
        assert!(f.grip_at(dvec2(101.0, 175.0), 0.0).is_empty());
    }
}
