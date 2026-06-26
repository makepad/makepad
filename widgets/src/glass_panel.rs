use crate::{
    animator::Animate,
    gauss_view::{request_window_gauss, GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    view::View,
    widget::*,
    widget_async::CxWidgetToScriptCallExt,
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
                let pad = 2.0
                // Visual corner radius is 2*r, so (h-2*pad)/4 gives a clean capsule.
                let r = (h - pad * 2.0) * 0.25

                // Track capsule: medium gray when off, accent green when on. Each element
                // ends with `sdf.fill` (not fill_keep) which RESETS the shape, otherwise the
                // following boxes union into it and the knob fill would paint everything.
                sdf.box(pad, pad, w - pad * 2.0, h - pad * 2.0, r)
                let off_color = vec4(0.46, 0.48, 0.51, 1.0)
                let on_color = vec4(0.27, 0.80, 0.33, 1.0)
                sdf.fill(off_color.mix(on_color, active))

                // White rounded-box knob (wider than tall), with the track still visible.
                let kpad = 3.0
                let knob_h = h - kpad * 2.0
                let knob_w = knob_h * 1.35
                let knob_x = mix(kpad, w - knob_w - kpad, active)
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
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let normal = mix(vec2(0.0, 1.0), normalize(gradient), step(0.00001, length(gradient)))

                // Edge-only refraction: the rim bends the switch underneath, the centre looks
                // straight through. No centre-pull magnification (that produced a dark box).
                let rim = clamp(1.0 - abs(shape) / 9.0, 0.0, 1.0)
                let bend = rim * rim
                let disp = normal * (bend * 11.0) / max(self.source_size, vec2(1.0, 1.0))
                let chroma = normal * (bend * 3.5) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + disp, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(clamp(uv_g + chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(clamp(uv_g - chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let refracted = vec3(s_r.r, s_g.g, s_b.b)
                let fallback = vec3(0.86, 0.92, 0.90)
                let base = fallback.mix(refracted, self.has_gauss)

                // Frosted-glass body so it always reads as bright glass, never a dark hole.
                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let material = base.mix(vec3(1.0, 1.0, 1.0), 0.20 + top * 0.12)
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
        label_walk: Walk{width: Fit, height: Fit}

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
                let r = 9.0
                let ins = 2.0 + self.press * 1.5
                sdf.box(ins, ins, w - ins * 2.0, h - ins * 2.0, r)

                let shape = sdf.shape
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                // The rounded box has a FLAT interior, so the gradient is exactly zero there
                // and `normalize` would return NaN. `mix` does NOT guard it (NaN*0 = NaN),
                // which propagated into the uv lookup as a black bar. Branch explicitly.
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let glen = length(gradient)
                var normal = vec2(0.0, 1.0)
                if glen > 0.0001 {
                    normal = gradient / glen
                }

                // Edge-only refraction so the centre passes the background straight through
                // (clean glass) and the rim bends it.
                let rim = clamp(1.0 - abs(shape) / 13.0, 0.0, 1.0)
                let bend = rim * rim
                let disp = normal * (bend * 14.0) / max(self.source_size, vec2(1.0, 1.0))
                let chroma = normal * (bend * 4.0) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + disp, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(clamp(uv_g + chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(clamp(uv_g - chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                let refracted = vec3(s_r.r, s_g.g, s_b.b)
                let fallback = vec3(0.80, 0.88, 0.95)
                let base = fallback.mix(refracted, self.has_gauss)

                // Fully OPAQUE glass - the "transparent" look comes only from the refraction
                // lookup (like the radio knob), never from alpha. The label is drawn crisply
                // on top in the overlay, so it is not refracted into a dark smear.
                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let frost = base.mix(vec3(1.0, 1.0, 1.0), 0.06 + top * 0.08 + self.hover * 0.04)
                let material = frost.mix(self.tint.rgb, self.tint.a)
                sdf.fill_keep(vec4(material, 1.0))

                // Bright specular crescent on the upper-right rim.
                let light_dir = normalize(vec2(0.5, -0.86))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let edgeband = clamp(1.0 - abs(shape) / 2.6, 0.0, 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * edgeband * (0.50 + self.hover * 0.12)))
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
                let fallback = vec3(0.85, 0.92, 0.90)
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
                let seg_w = w / self.count
                let pad = 3.0
                // Gloop: stretch the pill horizontally as it travels (0 at rest, max at the
                // midpoint between two segments), like the radio knob's squash/stretch.
                let g = abs(self.sel_pos - floor(self.sel_pos + 0.5)) * 2.0
                let pill_x = self.sel_pos * seg_w + pad - g * seg_w * 0.22
                let pill_w = seg_w - pad * 2.0 + g * seg_w * 0.44
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
                let fallback = vec3(0.85, 0.92, 0.90)
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

    mod.widgets.glass.Panel = mod.widgets.AppleGlassRoundedView{
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
            tint_alpha: 0.006
            surface_alpha: 1.0
            border_alpha: 0.72
            border_width: 1.0
            specular_strength: 0.22
            noise_strength: 0.004
            fallback_color: #x334156
            shadow_color: #x0007
            shadow_radius: 24.0
            shadow_offset: vec2(0.0, 9.0)
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
            fallback_color: #x263242
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
            shadow_radius: 20.0
            shadow_offset: vec2(0.0, 7.0)
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
            shadow_radius: 16.0
            shadow_offset: vec2(0.0, 6.0)
        }
    }

    mod.widgets.glass.Card = mod.widgets.glass.Group{
        padding: 14
        draw_bg +: {
            tint_alpha: 0.030
            lensing_effect: 0.34
        }
    }

    mod.widgets.glass.LensSurface = mod.widgets.AppleGlassRoundedView{
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
            fallback_color: #x314052
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
            fallback_color: #x234e74
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
        // TextInput pins its text to padding.top (layout align does not move it), so the
        // vertical padding is what centres the glyphs in the 38px field.
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
            // Tight line spacing so `align: y:0.5` centres the glyphs, not a tall line box.
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
        vm.with_cx_mut(|cx| self.redraw(cx));
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
        vm.with_cx_mut(|cx| self.redraw(cx));
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

        // Ease the gloopy press toward the current down state.
        if self.next_frame.is_event(event).is_some() {
            let delta = self.down - self.press;
            if delta.abs() <= 0.01 {
                self.press = self.down;
            } else {
                self.press += delta * 0.32;
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
                self.next_frame = cx.new_next_frame();
                self.set_key_focus(cx);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.down = 0.0;
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

        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_bg.end(cx);
        let rect = self.draw_bg.area().rect(cx);

        self.draw_glass.draw_abs(cx, rect);

        // Crisp label on top of the glass. It MUST be drawn as plain aligned glyph
        // instances (draw_abs) registered in the CURRENT turtle's align range - exactly
        // like the glass quad above - NOT inside a nested `begin_turtle(abs_pos: ...)`.
        // An abs_pos walk records `deferred_before_count: 0` (see turtle.rs
        // walk_turtle_internal), so when this button is laid out after a `Fill` sibling
        // (e.g. the centered month title between the `<` / `>` buttons) the parent row's
        // deferred-fill shift (`total_resolved_length_to`) never reaches the label: the
        // glass quad slides to its final x while the label stays at the pre-shift x,
        // leaving the glyph detached far to the left. Drawing the label as draw_abs glyph
        // instances puts it in the same align range as the glass, so both ride the shift.
        let text = self.text.as_ref();
        let laid = self
            .draw_text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let text_size = dvec2(
            laid.size_in_lpxs.width as f64 * self.draw_text.font_scale as f64,
            laid.size_in_lpxs.height as f64 * self.draw_text.font_scale as f64,
        );
        let align = self.layout.align;
        let pos = dvec2(
            rect.pos.x + (rect.size.x - text_size.x) * align.x,
            rect.pos.y + (rect.size.y - text_size.y) * align.y,
        );
        self.draw_text.draw_abs(cx, pos, text);
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
        vm.with_cx_mut(|cx| self.redraw(cx));
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
        vm.with_cx_mut(|cx| self.redraw(cx));
    }
}

impl GlassSegmented {
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
                self.sel_pos += delta * 0.30;
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
                let frac = ((fe.abs.x - rect.pos.x) / rect.size.x.max(1.0)).clamp(0.0, 0.999);
                let idx = (frac * n as f64) as usize;
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
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let snapshot = request_window_gauss(cx);
        self.bind_sel(cx, snapshot);
        self.draw_sel.draw_abs(cx, rect);
        // Place each label explicitly at the centre of its segment so it lines up exactly
        // with the pill (both divide the width by the same segment count).
        let n = self.labels.len().max(1) as f64;
        let seg_w = rect.size.x / n;
        for (i, label) in self.labels.clone().iter().enumerate() {
            let seg_pos = Vec2d {
                x: rect.pos.x + i as f64 * seg_w,
                y: rect.pos.y,
            };
            cx.begin_turtle(
                Walk {
                    abs_pos: Some(seg_pos),
                    width: Size::Fixed(seg_w),
                    height: Size::Fixed(rect.size.y),
                    margin: Inset::default(),
                    metrics: Metrics::default(),
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
}
