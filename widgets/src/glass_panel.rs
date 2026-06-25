use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    gauss_view::{request_window_gauss, GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.glass = {}

    mod.widgets.glass.LayerBase = #(GlassLayer::register_widget(vm))
    mod.widgets.glass.GlassRadioBase = #(GlassRadio::register_widget(vm))

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
        width: 78
        height: 44
        flow: Overlay

        draw_slot +: {
            active: instance(0.0)
            hover: instance(0.0)
            down: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let active = self.active
                let w = self.rect_size.x
                let h = self.rect_size.y
                let pad = 2.0
                // Visual corner radius is 2*r, so (h-2*pad)/4 gives a clean capsule.
                let r = (h - pad * 2.0) * 0.25
                let top = smoothstep(0.0, 0.32, 1.0 - self.pos.y)
                let bottom = smoothstep(0.60, 1.0, self.pos.y)

                // Capsule track: dark translucent when off, the whole track turns
                // accent-green when on, just like an Apple toggle.
                sdf.box(pad, pad, w - pad * 2.0, h - pad * 2.0, r)
                let off_color = vec4(0.02, 0.05, 0.06, 0.34 + self.hover * 0.05)
                let on_color = vec4(0.17, 0.79, 0.37, 0.95)
                sdf.fill_keep(off_color.mix(on_color, active))
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, 0.05 + active * 0.05) * top)
                sdf.fill_keep(vec4(0.0, 0.0, 0.0, 0.10 + self.down * 0.08) * bottom)
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.08 + self.hover * 0.05), 1.0)

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
            active: instance(0.0)
            hover: instance(0.0)
            down: instance(0.0)

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
                let knob_d = h - 8.0
                let knob_x = mix(4.0, w - knob_d - 4.0, active)
                let knob_y = (h - knob_d) * 0.5
                // True circle (sdf.box visual corner radius is 2*r, so d/4 = circle) -
                // a round contour keeps the lens ring round instead of a square.
                let knob_r = knob_d * 0.25
                sdf.box(knob_x, knob_y, knob_d, knob_d, knob_r)

                let shape = sdf.shape
                let screen_pos = self.rect_pos + self.pos * self.rect_size
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let gradient = vec2(dFdx(shape), dFdy(shape))
                let normal = mix(vec2(0.0, 1.0), normalize(gradient), step(0.00001, length(gradient)))

                // Spherical-dome refraction. `radial` is 0 at the centre and 1 at the rim;
                // we both bend along the surface normal (rim) AND magnify the whole disc by
                // pulling the sample toward the knob centre, so the centre is never a flat
                // un-refracted hole that shows the dark backing as a black square.
                let knob_c = vec2(knob_x + knob_d * 0.5, knob_y + knob_d * 0.5)
                let local = (self.pos * self.rect_size - knob_c) / (knob_d * 0.5)
                let radial = clamp(length(local), 0.0, 1.0)
                let dome = sqrt(max(1.0 - radial * radial, 0.0))
                let mag_off = -local * (0.32 * (1.0 - dome)) * (knob_d * 0.5) / max(self.source_size, vec2(1.0, 1.0))
                let lens = pow(clamp(1.0 - abs(shape) / 18.0, 0.0, 1.0), 1.2)
                let base_off = normal * (lens * 22.0) / max(self.source_size, vec2(1.0, 1.0)) + mag_off
                let col_off = normal * (lens * 7.0) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + base_off, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_r = clamp(uv_g + col_off, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_b = clamp(uv_g - col_off, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let s_r = self.sample_blur(uv_r)
                let s_g = self.sample_blur(uv_g)
                let s_b = self.sample_blur(uv_b)
                let refracted = vec3(s_r.r, s_g.g, s_b.b)
                let fallback = vec3(0.80, 0.92, 0.86)
                let base = fallback.mix(refracted, self.has_gauss)

                // Frosted glass body: lift the refraction onto a soft light base, brighter
                // toward the top, so a dark backdrop reads as pale glass rather than black.
                let top = smoothstep(0.0, 1.0, 1.0 - self.pos.y)
                let body = vec3(0.86, 0.92, 0.90) + top * 0.10
                let material = base.mix(body, 0.42 + dome * 0.14)
                sdf.fill_keep(vec4(material, 1.0))

                // Bright specular crescent on the light-facing (upper-right) rim.
                let light_dir = normalize(vec2(0.55, -0.83))
                let facing = clamp(dot(normal, light_dir), 0.0, 1.0)
                let rim = pow(clamp(1.0 - abs(shape) / 3.0, 0.0, 1.0), 1.0)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, facing * rim * (0.80 + self.hover * 0.10)))
                // Faint full edge to seal the glass.
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.14), 0.8)
                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.14}}
                    apply: {
                        draw_slot: {down: snap(0.0), hover: 0.0}
                        draw_knob: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_slot: {down: snap(0.0), hover: 1.0}
                        draw_knob: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward {duration: 0.08}}
                    apply: {
                        draw_slot: {down: 1.0, hover: 1.0}
                        draw_knob: {down: 1.0, hover: 1.0}
                    }
                }
            }
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.18}}
                    apply: {
                        draw_slot: {active: 0.0}
                        draw_knob: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.24}}
                    apply: {
                        draw_slot: {active: 1.0}
                        draw_knob: {active: 1.0}
                    }
                }
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
        height: 44
        margin: 0
        padding: Inset{left: 16, right: 16, top: 0, bottom: 0}
        empty_text: "Text"
        draw_bg +: {
            border_radius: 14.0
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
            text_style: theme.font_regular{font_size: 12}
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

#[derive(Script, Widget, Animator)]
pub struct GlassRadio {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    draw_slot: DrawQuad,
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

    pub fn active(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }

    pub fn set_active(&mut self, cx: &mut Cx, value: bool, animate: Animate) {
        self.animator_toggle(cx, value, animate, ids!(active.on), ids!(active.off));
        self.redraw(cx);
    }
}

impl Widget for GlassRadio {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }

        match event.hits(cx, self.draw_slot.area()) {
            Hit::KeyFocus(_) => {}
            Hit::KeyFocusLost(_) => {}
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.animator_play(cx, ids!(hover.down));
                self.set_key_focus(cx);
            }
            Hit::FingerUp(fe) => {
                self.animator_play(cx, ids!(hover.on));
                // Checkbox semantics: a click toggles this control on or off
                // independently (with the sliding animation), it is not a radio.
                if fe.is_over {
                    if self.animator_in_state(cx, ids!(active.on)) {
                        self.animator_play(cx, ids!(active.off));
                    } else {
                        self.animator_play(cx, ids!(active.on));
                    }
                    cx.widget_action_with_data(&self.action_data, uid, GlassRadioAction::Clicked);
                }
            }
            Hit::FingerMove(_fe) => {}
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
        let rect = self.draw_slot.draw_walk(cx, walk);
        cx.add_nav_stop(self.draw_slot.area(), NavRole::TextInput, Inset::default());

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
