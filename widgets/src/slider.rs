use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInput, TextInputAction},
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.SliderBase = #(Slider::register_widget(vm))
    mod.widgets.DragAxis =  set_type_default() do #(DragAxis::script_api(vm))
    mod.widgets.splat(mod.widgets.DragAxis)
    mod.widgets.SliderTaper =  set_type_default() do #(SliderTaper::script_api(vm))
    mod.widgets.splat(mod.widgets.SliderTaper)

    use mod.widgets.*

    set_type_default() do #(DrawSlider::script_shader(vm)){
        ..mod.draw.DrawQuad // splat in draw quad
    }

    /** The minimal slider: a flat two-band track with a value fill and a hover-grown handle. */
    mod.widgets.SliderMinimal = set_type_default() do mod.widgets.SliderBase{
        /** lowest value the slider reports */
        min: 0.0
        /** highest value the slider reports */
        max: 1.0
        /** value quantization; 0 is continuous 0..1 step 0.01 */
        step: 0.0
        label_align: Align{x: 0., y: 0.}
        margin: theme.mspace_1{top: theme.space_2}
        /** decimals shown in the value field 0..6 step 1 */
        precision: 2.
        height: 25
        hover_actions_enabled: false

        /** The minimal track material: a shadow band, a highlight band and the value fill. */
        draw_bg +: {
            /** pointer-hover mix; also grows the handle 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** track band thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)

            color: uniform(theme.color_inset_1)
            color_hover: uniform(theme.color_inset_1_hover)
            color_focus: uniform(theme.color_inset_1_focus)
            color_disabled: uniform(theme.color_inset_1_disabled)
            color_drag: uniform(theme.color_inset_1_drag)

            color_2: uniform(theme.color_inset_2)
            color_2_hover: uniform(theme.color_inset_2_hover)
            color_2_focus: uniform(theme.color_inset_2_focus)
            color_2_disabled: uniform(theme.color_inset_2_disabled)
            color_2_drag: uniform(theme.color_inset_2_drag)

            border_color: uniform(theme.color_bevel_outset_1)
            border_color_hover: uniform(theme.color_bevel_outset_1)
            border_color_focus: uniform(theme.color_bevel_outset_1)
            border_color_drag: uniform(theme.color_bevel_outset_1)
            border_color_disabled: uniform(theme.color_bevel_outset_1_disabled)

            border_color_2: uniform(theme.color_bevel_outset_2)
            border_color_2_hover: uniform(theme.color_bevel_outset_2)
            border_color_2_focus: uniform(theme.color_bevel_outset_2)
            border_color_2_drag: uniform(theme.color_bevel_outset_2)
            border_color_2_disabled: uniform(theme.color_bevel_outset_2_disabled)

            /** track top offset, leaving room for the label in pixels 0..40 step 1 */
            offset_y: uniform(20.)
            /** handle width at full hover in pixels 0..40 step 1 */
            handle_size: uniform(20.)

            /** the filled-amount color, left of the handle */
            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val_hover)
            val_color_focus: uniform(theme.color_val_focus)
            val_color_drag: uniform(theme.color_val_drag)
            val_color_disabled: uniform(theme.color_val_disabled)

            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            handle_color_focus: uniform(theme.color_handle_focus)
            handle_color_drag: uniform(theme.color_handle_drag)
            handle_color_disabled: uniform(theme.color_handle_disabled)

            pixel: fn() {
                let slider_height = self.rect_size.y - self.offset_y

                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let track_height = self.rect_size.y - self.offset_y

                let handle_sz = mix(0., self.handle_size, self.hover)

                // Track shadow
                sdf.rect(
                    0.
                    self.offset_y
                    self.rect_size.x
                    track_height * 0.5 + 1
                )

                sdf.fill(
                    self.border_color_2
                        .mix(self.border_color_2_focus, self.focus)
                        .mix(self.border_color_2_hover.mix(self.border_color_2_drag, self.drag), self.hover)
                        .mix(self.border_color_2_disabled, self.disabled)
                )

                // Track highlight
                sdf.rect(
                    0
                    self.offset_y + track_height * 0.5
                    self.rect_size.x
                    track_height * 0.5
                )

                sdf.fill(
                    self.border_color
                        .mix(self.border_color_focus, self.focus)
                        .mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover)
                        .mix(self.border_color_disabled, self.disabled)
                )

                // Amount. From the stop, or from the default's position
                // when the slider asks for it -- a bipolar control wants
                // its bar to grow out of the middle in whichever
                // direction it was moved, not to fill from one end.
                let lo = min(self.origin_pos, self.slide_pos)
                let hi = max(self.origin_pos, self.slide_pos)
                let from = mix(0.0, lo, self.arc_origin)
                let to = mix(self.slide_pos, hi, self.arc_origin)
                sdf.rect(
                    from * self.rect_size.x
                    self.offset_y
                    max(1.0, (to - from) * self.rect_size.x)
                    slider_height
                )
                sdf.fill(
                    self.val_color
                        .mix(self.val_color_focus, self.focus)
                        .mix(self.val_color_hover.mix(self.val_color_drag, self.drag), self.hover)
                        .mix(self.val_color_disabled, self.disabled)
                )

                // Handle
                let handle_bg_size = mix(0, 10, self.hover)
                let handle_bg_x = self.slide_pos * self.rect_size.x

                sdf.rect(
                    handle_bg_x - handle_sz * 0.5
                    self.offset_y
                    handle_sz
                    slider_height * /** handle height scale 1..3 step 0.1 */ 2.
                )

                sdf.fill_keep(
                    self.handle_color
                        .mix(self.handle_color_focus, self.focus)
                        .mix(self.handle_color_hover.mix(self.handle_color_drag, self.drag), self.hover)
                        .mix(self.handle_color_disabled, self.disabled)
                )

                return sdf.result
            }
        }

        /** The slider label ink, state-mixed with the track. */
        draw_text +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** placeholder-showing mix 0..1 step 0.01 */
            empty: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_drag: uniform(theme.color_label_outer_drag)
            color_focus: uniform(theme.color_label_outer_focus)
            color_disabled: uniform(theme.color_label_outer_disabled)
            color_empty: uniform(theme.color_text_placeholder)

            text_style: theme.font_regular{
                line_spacing: theme.font_wdgt_line_spacing
                font_size: theme.font_size_p
            }

            get_color: fn() {
                return self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_empty, self.empty)
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_disabled, self.disabled)
            }
        }

        label_walk: Walk{
            width: Fill
            height: Fit
            margin: Inset{top: 0., bottom: theme.space_1}
        }

        text_input: TextInput{
            empty_text: "0"
            is_numeric_only: true
            is_read_only: false

            width: Fit
            label_align: Align{y: 0.}
            margin: 0.
            padding: 0.

            draw_text +: {
                color: theme.color_text_val
                color_hover: theme.color_text_hover
                color_focus: theme.color_text_focus
                color_down: theme.color_text_down
                color_disabled: theme.color_text_disabled
                color_empty: theme.color_text_placeholder
                color_empty_hover: theme.color_text_placeholder_hover
                color_empty_focus: theme.color_text_focus
            }

            draw_bg +: {
                border_radius: 0.
                border_size: 0.

                color: theme.color_u_hidden
                color_hover: theme.color_u_hidden
                color_focus: theme.color_u_hidden
                color_disabled: theme.color_u_hidden
                color_empty: theme.color_u_hidden

                border_color: theme.color_u_hidden
                border_color_hover: theme.color_u_hidden
                border_color_empty: theme.color_u_hidden
                border_color_disabled: theme.color_u_hidden
                border_color_focus: theme.color_u_hidden

                border_color_2: theme.color_u_hidden
                border_color_2_hover: theme.color_u_hidden
                border_color_2_empty: theme.color_u_hidden
                border_color_2_disabled: theme.color_u_hidden
                border_color_2_focus: theme.color_u_hidden
            }

            draw_cursor +: {color: theme.color_text_cursor}

            draw_selection +: {
                border_radius: theme.textselection_corner_radius

                color: theme.color_d_hidden
                color_hover: theme.color_d_hidden
                color_focus: theme.color_d_hidden
                color_empty: theme.color_u_hidden
                color_disabled: theme.color_u_hidden
            }
        }

        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
            drag: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {drag: 0.0}
                        draw_text: {drag: 0.0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Arrow
                    from: {all: Snap}
                    apply: {
                        draw_bg: {drag: 1.0}
                        draw_text: {drag: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.SliderMinimalFlat = mod.widgets.SliderMinimal{
        draw_bg +: {
            border_color: theme.color_bevel_outset_2
            border_color_hover: theme.color_bevel_outset_2
            border_color_focus: theme.color_bevel_outset_2
            border_color_drag: theme.color_bevel_outset_2
            border_color_disabled: theme.color_bevel_outset_2_disabled

            border_color_2: theme.color_bevel_outset_2
            border_color_2_hover: theme.color_bevel_outset_2
            border_color_2_focus: theme.color_bevel_outset_2
            border_color_2_drag: theme.color_bevel_outset_2
            border_color_2_disabled: theme.color_bevel_outset_2_disabled
        }
    }

    /** The flat slider: a boxed track with a centre ridge, a value line and a drag handle. */
    mod.widgets.SliderFlat = mod.widgets.SliderMinimal{
        height: 36

        /** The slider face: an inset SDF box with a ridge, a value line and a handle box. */
        draw_bg +: {
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** bevel gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_border_horizontal: uniform(0.0)
            /** fill gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_fill_horizontal: uniform(0.0)

            /** dither the gradient fill to hide banding 0..1 step 1 */
            color_dither: uniform(1.0)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_drag: uniform(theme.color_inset_drag)
            color_disabled: uniform(theme.color_inset_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_inset_2_hover)
            color_2_focus: uniform(theme.color_inset_2_focus)
            color_2_drag: uniform(theme.color_inset_2_drag)
            color_2_disabled: uniform(theme.color_inset_2_disabled)

            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            handle_color_focus: uniform(theme.color_handle_focus)
            handle_color_drag: uniform(theme.color_handle_drag)
            handle_color_disabled: uniform(theme.color_handle_disabled)

            handle_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            handle_color_2_hover: uniform(theme.color_handle_2_hover)
            handle_color_2_focus: uniform(theme.color_handle_2_focus)
            handle_color_2_drag: uniform(theme.color_handle_2_drag)
            handle_color_2_disabled: uniform(theme.color_handle_2_disabled)

            border_color: uniform(theme.color_bevel_inset_1)
            border_color_hover: uniform(theme.color_bevel_inset_1_hover)
            border_color_focus: uniform(theme.color_bevel_inset_1_focus)
            border_color_drag: uniform(theme.color_bevel_inset_1_drag)
            border_color_disabled: uniform(theme.color_bevel_inset_1_disabled)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_bevel_inset_2_hover)
            border_color_2_focus: uniform(theme.color_bevel_inset_2_focus)
            border_color_2_drag: uniform(theme.color_bevel_inset_2_drag)
            border_color_2_disabled: uniform(theme.color_bevel_inset_2_disabled)

            /** value line inset from the track edge in pixels 0..20 step 0.5 */
            val_padding: uniform(5.)

            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val_hover)
            val_color_focus: uniform(theme.color_val_focus)
            val_color_drag: uniform(theme.color_val_drag)
            val_color_disabled: uniform(theme.color_val_disabled)

            /** handle width in pixels 4..60 step 1 */
            handle_size: uniform(20.)

            // THE MATERIAL, from the theme, packed as `ReliefView` and
            // `RoundedView` pack theirs. Zero in every stock theme: at
            // `material` 0 this shader draws what it always drew.
            /** surface material tier: 0 flat, 1 relief, 2 relief with rim, gloss and specular 0..2 step 1 */
            material: uniform(theme.material_level)
            /** key light: direction (x right, y down, z out) and intensity */
            material_light: uniform(vec4(theme.material_light_x, theme.material_light_y, theme.material_light_z, theme.material_light_intensity))
            /** bevel width, profile curve, raise, specular */
            material_relief: uniform(vec4(theme.material_bevel_width, theme.material_bevel_curve, theme.material_raise, theme.material_specular))
            /** occlusion, rim, gloss, roughness */
            material_finish: uniform(vec4(theme.material_ao, theme.material_rim, theme.material_gloss, theme.material_roughness))
            /** face gradient, hairline, occlusion reach, sink */
            material_tune: uniform(vec4(theme.material_face_gradient, theme.material_hairline, theme.material_ao_reach, theme.material_sink))
            /** cast shadow strength, blur, falloff (0 linear 1 expo), contact occlusion */
            material_shadow: uniform(vec4(theme.material_shadow, theme.material_shadow_blur, theme.material_shadow_falloff, theme.material_contact_ao))
            /** inner shadow, inner blur, ground lip, glow */
            material_inner: uniform(vec4(theme.material_inner_shadow, theme.material_inner_radius, theme.material_ground_lip, theme.material_glow))
            /** the ink a lit shoulder is tinted toward */
            material_light_ink: uniform(theme.color_material_light)
            /** the ink a shaded shoulder and the occlusion are tinted toward */
            material_shadow_ink: uniform(theme.color_material_shadow)

            /** the outward gradient of a rounded box centred on c with half size h and corner k, by central differences */
            material_grad: fn(p: vec2, c: vec2, h: vec2, k: float) -> vec2 {
                let e = 0.5
                let g = vec2(
                    Material.sd_box(p + vec2(e, 0.0), c, h, k) - Material.sd_box(p - vec2(e, 0.0), c, h, k),
                    Material.sd_box(p + vec2(0.0, e), c, h, k) - Material.sd_box(p - vec2(0.0, e), c, h, k)
                )
                if length(g) > 0.00001 {
                    return normalize(g)
                }
                return vec2(0.0, 1.0)
            }

            /** the box centred on c (half size h, corner k, distance d at p)
             * lit as one face at elevation `elev`: a groove below zero, with
             * the surround's inner shadow over it, a cap above. Tier 1 is
             * the relief alone. */
            material_box: fn(fill: vec4, p: vec2, d: float, c: vec2, h: vec2, k: float, elev: float) -> vec4 {
                let g = self.material_grad(p, c, h, k)
                var insh = 0.0
                if self.material_inner.x > 0.001 && elev < 0.0 {
                    let ioff = Material.shadow_dir(self.material_light) * abs(elev) * 1.6
                    insh = 1.0 - Material.box_cov(c - h + ioff, c + h + ioff, p, max(self.material_inner.y * 0.5, 0.35), k)
                }
                let t2 = step(1.5, self.material)
                let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                let rel = vec4(min(self.material_relief.x, min(h.x, h.y)), self.material_relief.y, self.material_relief.z, self.material_relief.w * t2)
                let uv = (p - c) / (2.0 * h) + vec2(0.5, 0.5)
                let o = Material.face(
                    fill.rgb, d, g, uv, elev, elev, 0.0, insh, 0.0,
                    self.material_light, rel, fin, self.material_tune, self.material_inner.x,
                    self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                )
                return vec4(o, fill.a)
            }

            /** what the raised handle box (centred on c, half size h, corner
             * k) throws on the groove at p, premultiplied: its cast shadow,
             * contact and lip. Laid into the groove's fill, since the
             * handle travels inside it. */
            material_handle_under: fn(p: vec2, c: vec2, h: vec2, k: float) -> vec4 {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let d = Material.sd_box(p, c, h, k)
                let g = self.material_grad(p, c, h, k)
                let raise = self.material_relief.z * (1.0 - self.disabled)
                let off = Material.cast_offset(raise, self.material_light)
                // The shadow falls inside the groove: its blur stays in scale
                // with the handle.
                let sh = vec4(self.material_shadow.x, min(self.material_shadow.y, min(h.x, h.y)), self.material_shadow.z, self.material_shadow.w)
                return Material.cast(
                    d, Material.sd_box(p - off, c, h, k), Material.sd_box(p + off, c, h, k), g, px, raise, self.material_relief.z,
                    self.material_light, sh, self.material_inner.z,
                    self.material_shadow_ink.rgb, self.material_light_ink.rgb
                ) * (1.0 - self.disabled)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let handle_sz = self.handle_size

                let offset_px = vec2(0., /** label reserve above track 0..40 step 1 */ 20.)

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let sz_px = vec2(
                    self.rect_size.x
                    self.rect_size.y - offset_px.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x
                    self.rect_size.y / sz_px.y
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2. - offset_px.y
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                let slider_top = offset_px.y + self.border_size
                let slider_width = self.rect_size.x - self.border_size * 2.
                let slider_bottom = self.rect_size.y - offset_px.y - self.border_size * 2.
                let slider_height = (self.rect_size.y - offset_px.y) * 0.5 - self.val_padding

                // Setup fill colors
                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_focus = self.color_focus
                let mut color_fill_drag = self.color_drag
                let mut color_fill_disabled = self.color_disabled

                // Compute adjusted y position for gradients
                let pos_y_adj = self.pos.y - offset_uv.y

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let gfx = self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    let gfy = pos_y_adj * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    let gradient_fill = vec2(gfx, gfy)
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    color_fill = mix(self.color, self.color_2, dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, dir)
                    color_fill_focus = mix(self.color_focus, self.color_2_focus, dir)
                    color_fill_drag = mix(self.color_drag, self.color_2_drag, dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, dir)
                }

                // Setup border colors
                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_focus = self.border_color_focus
                let mut color_stroke_drag = self.border_color_drag
                let mut color_stroke_disabled = self.border_color_disabled

                let mut border_color_2 = self.border_color
                let mut border_color_2_hover = self.border_color_hover
                let mut border_color_2_focus = self.border_color_focus
                let mut border_color_2_drag = self.border_color_drag
                let mut border_color_2_disabled = self.border_color_disabled

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let gbx = self.pos.x * scale_factor_border.x + dither
                    let gby = pos_y_adj * scale_factor_border.y + dither
                    let gradient_border = vec2(gbx, gby)
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, dir)
                    color_stroke_focus = mix(self.border_color_focus, self.border_color_2_focus, dir)
                    color_stroke_drag = mix(self.border_color_drag, self.border_color_2_drag, dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, dir)
                    border_color_2 = self.border_color_2
                    border_color_2_hover = self.border_color_2_hover
                    border_color_2_focus = self.border_color_2_focus
                    border_color_2_drag = self.border_color_2_drag
                    border_color_2_disabled = self.border_color_2_disabled
                }

                // Setup handle colors
                let mut handle_fill = self.handle_color
                let mut handle_fill_hover = self.handle_color_hover
                let mut handle_fill_focus = self.handle_color_focus
                let mut handle_fill_drag = self.handle_color_drag
                let mut handle_fill_disabled = self.handle_color_disabled

                let mut handle_stroke = self.border_color
                let mut handle_stroke_hover = self.border_color_hover
                let mut handle_stroke_drag = self.border_color_drag
                let mut handle_stroke_disabled = self.border_color_disabled

                if self.handle_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let gfx = self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    let gfy = pos_y_adj * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    let gradient_fill = vec2(gfx, gfy)
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    handle_fill = mix(self.handle_color, self.handle_color_2, dir)
                    handle_fill_hover = mix(self.handle_color_hover, self.handle_color_2_hover, dir)
                    handle_fill_focus = mix(self.handle_color_focus, self.handle_color_2_focus, dir)
                    handle_fill_drag = mix(self.handle_color_drag, self.handle_color_2_drag, dir)
                    handle_fill_disabled = mix(self.handle_color_disabled, self.handle_color_2_disabled, dir)
                }

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let gbx = self.pos.x * scale_factor_border.x + dither
                    let gby = pos_y_adj * scale_factor_border.y + dither
                    let gradient_border = vec2(gbx, gby)
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    handle_stroke = mix(self.border_color_2, self.border_color, dir)
                    handle_stroke_hover = mix(self.border_color_2_hover, self.border_color_hover, dir)
                    handle_stroke_drag = mix(self.border_color_2_drag, self.border_color_drag, dir)
                    handle_stroke_disabled = mix(self.border_color_2_disabled, self.border_color_disabled, dir)
                }

                // The handle's geometry, before the groove is filled, because
                // under a material its shadow is laid INTO the groove's fill.
                let ctrl_height = self.rect_size.y - offset_px.y
                let offset_sides = self.border_size + /** track side inset 0..20 step 0.5 */ 6.
                let handle_x = self.slide_pos * (self.rect_size.x - handle_sz - offset_sides) - 3
                let handle_padding = /** handle vertical inset 0..6 step 0.5 */ 1.5
                let handle_box = vec4(
                    handle_x + offset_sides + self.border_size
                    offset_px.y + self.border_size + handle_padding
                    self.handle_size - self.border_size * 2.
                    ctrl_height - self.border_size * 2. - handle_padding * 2.
                )

                // Draw main box
                sdf.box(
                    self.border_size
                    slider_top
                    slider_width
                    slider_bottom
                    self.border_radius
                )

                var fill = color_fill
                    .mix(color_fill_focus, self.focus)
                    .mix(color_fill_hover.mix(color_fill_drag, self.drag), self.hover)
                    .mix(color_fill_disabled, self.disabled)

                // THE MATERIAL: the track is a groove cut into the housing,
                // the handle a cap standing in it and throwing its shadow on
                // it. Nothing changes at 0.
                let p = self.pos * self.rect_size
                // `sdf.box` draws a corner of TWICE its argument, clamped.
                let hc = handle_box.xy + handle_box.zw * 0.5
                let hh = max(handle_box.zw * 0.5, vec2(0.5, 0.5))
                let hk = min(2.0 * self.border_radius, min(hh.x, hh.y))
                if self.material > 0.5 {
                    let c = vec2(self.border_size + slider_width * 0.5, slider_top + slider_bottom * 0.5)
                    let h = max(vec2(slider_width * 0.5, slider_bottom * 0.5), vec2(0.5, 0.5))
                    let k = min(2.0 * self.border_radius, min(h.x, h.y))
                    fill = self.material_box(fill, p, sdf.shape, c, h, k, -self.material_tune.w * (1.0 - self.disabled))
                    let under = self.material_handle_under(p, hc, hh, hk)
                    fill = vec4(mix(fill.rgb, under.rgb / max(under.a, 0.0001), under.a), fill.a)
                }

                sdf.fill_keep(fill)

                let stroke = color_stroke
                    .mix(color_stroke_focus.mix(color_stroke_hover.mix(color_stroke_drag, self.drag), self.hover), self.focus)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.stroke(stroke, self.border_size)

                // Ridge
                sdf.rect(
                    self.border_size + offset_sides
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5 - self.border_size - 0.5
                    self.rect_size.x - 2. * offset_sides - self.border_size * 2.
                    self.border_size * 2. + 1.
                )

                sdf.fill(
                    self.border_color
                        .mix(self.border_color_focus.mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover), self.focus)
                        .mix(self.border_color_disabled, self.disabled)
                )

                sdf.rect(
                    self.border_size + offset_sides
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5
                    self.rect_size.x - 2. * offset_sides - self.border_size * 2. + 0.5
                    self.border_size * 2.
                )

                sdf.fill(
                    border_color_2
                        .mix(border_color_2_hover, self.hover)
                        .mix(border_color_2_hover.mix(border_color_2_drag, self.drag), self.hover)
                        .mix(border_color_2_disabled, self.disabled)
                )

                // Value line. From the stop, or from where the DEFAULT
                // sits when the slider asks for it -- which is not the
                // same as the middle of the track: a knob whose unity is
                // at a quarter of its travel grows from the quarter.
                let track_length = self.rect_size.x - offset_sides * 4.
                let val_x = self.slide_pos * track_length + offset_sides * 2.
                let origin_x = self.origin_pos * track_length + offset_sides * 2.
                let offset_top = self.rect_size.y - (self.rect_size.y - offset_px.y) * 0.5
                let move_x = mix(offset_sides, origin_x, self.arc_origin)

                sdf.move_to(move_x, offset_top)
                sdf.line_to(val_x, offset_top)

                sdf.stroke(
                    self.val_color
                        .mix(self.val_color_hover, self.hover)
                        .mix(self.val_color_focus.mix(self.val_color_hover.mix(self.val_color_drag, self.drag), self.hover), self.focus)
                        .mix(self.val_color_disabled, self.disabled),
                    slider_height
                )

                // Handle
                sdf.box(
                    handle_box.x
                    handle_box.y
                    handle_box.z
                    handle_box.w
                    self.border_radius
                )

                var hfill = handle_fill
                    .mix(handle_fill_hover, self.hover)
                    .mix(handle_fill_focus.mix(handle_fill_hover.mix(handle_fill_drag, self.drag), self.hover), self.focus)
                    .mix(handle_fill_disabled, self.disabled)

                if self.material > 0.5 {
                    // The cap: raised, lifted a quarter more under the hand.
                    let raise = self.material_relief.z * (1.0 + 0.25 * max(self.hover, self.drag)) * (1.0 - self.disabled)
                    hfill = self.material_box(hfill, p, sdf.shape, hc, hh, hk, raise)
                }

                sdf.fill_keep(hfill)

                let hstroke = handle_stroke
                    .mix(handle_stroke_hover.mix(handle_stroke_drag, self.drag), self.hover)
                    .mix(handle_stroke_disabled, self.disabled)

                sdf.stroke(hstroke, self.border_size)

                return sdf.result
            }
        }
    }

    /** The standard slider: the flat face plus the theme's inset bevel and handle gradient. */
    mod.widgets.Slider = mod.widgets.SliderFlat{
        draw_bg +: {
            handle_color: theme.color_handle_1
            handle_color_hover: theme.color_handle_1_hover
            handle_color_focus: theme.color_handle_1_focus
            handle_color_disabled: theme.color_handle_1_disabled
            handle_color_drag: theme.color_handle_1_drag

            handle_color_2: theme.color_handle_2

            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
            border_color_drag: theme.color_bevel_inset_1_drag

            border_color_2: theme.color_bevel_inset_2
        }
    }

    mod.widgets.SliderGradientY = mod.widgets.Slider{
        draw_bg +: {
            color: theme.color_inset_1
            color_hover: theme.color_inset_1_hover
            color_focus: theme.color_inset_1_focus
            color_disabled: theme.color_inset_1_disabled
            color_drag: theme.color_inset_1_drag

            color_2: theme.color_inset_2
        }
    }

    mod.widgets.SliderGradientX = mod.widgets.SliderGradientY{
        draw_bg +: {
            gradient_border_horizontal: 1.0
            gradient_fill_horizontal: 1.0
        }
    }

    /** The fader: a desk cap running a slotted track, along whichever axis the slider is dragged. */
    mod.widgets.SliderFader = mod.widgets.SliderMinimal{
        width: Fill
        height: 40.

        // The cap's length and the track's end inset belong to the WIDGET
        // rather than to the material, because the DRAG divides by the
        // distance the cap actually travels: a finger that has crossed the
        // whole track must leave the cap on the stop, not short of it or
        // hard against it half way down. The shader is handed the same two
        // numbers every draw, so what is drawn is what is dragged.
        /** the cap's length along the track, in pixels 8..60 step 1 */
        cap_size: 18.
        /** how far the track's two ends are held off the edges, in pixels 0..40 step 1 */
        track_inset: 8.

        // NO INLINE LABEL, and that is the design rather than an omission.
        // A fader stood on end is forty points across -- a desk strip is
        // narrower still -- and this material fills the whole quad, so a
        // word or a readout drawn here lands ON the track. The legend is a
        // sibling's job: a Label over the column and another under it, the
        // way a desk prints them. The readout is sized away as RotaryKnob
        // sizes it away, the field still taking key focus so a value can be
        // typed; the label turtle draws nothing while `text` is empty, and
        // the tap-the-label reset goes with it, so a DOUBLE TAP is what
        // puts a fader back to its default.
        text_input +: {
            width: 0.
            height: 0.
        }

        /** The fader material: a slot, a bar out of the origin, and the cap. */
        draw_bg +: {
            /** corner rounding of the slot and the cap 0..12 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** the slot's width across the track, in pixels 2..24 step 0.5 */
            track_size: uniform(7.)
            /** the cap's width across the track, as a share of it 0.2..1 step 0.05 */
            cap_cross: uniform(0.72)
            /** the grip line cut across the cap, in pixels 0..4 step 0.5 */
            cap_line: uniform(2.)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_drag: uniform(theme.color_inset_drag)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel_inset_1)
            border_color_hover: uniform(theme.color_bevel_inset_1_hover)
            border_color_focus: uniform(theme.color_bevel_inset_1_focus)
            border_color_drag: uniform(theme.color_bevel_inset_1_drag)
            border_color_disabled: uniform(theme.color_bevel_inset_1_disabled)

            handle_color: uniform(theme.color_handle_1)
            handle_color_hover: uniform(theme.color_handle_1_hover)
            handle_color_focus: uniform(theme.color_handle_1_focus)
            handle_color_drag: uniform(theme.color_handle_1_drag)
            handle_color_disabled: uniform(theme.color_handle_1_disabled)

            // Inherited and not read by this material: offset_y and
            // handle_size, which reserve a label strip and grow a hover
            // handle on the flat faces, and the whole _2 colour family.
            // They still take uniform slots and still show in the tweaker
            // doing nothing. The cap's size is the WIDGET's `cap_size`, not
            // draw_bg's `handle_size`, which is the one trap this material
            // inherits: the drag has to divide by the same number, and only
            // Rust can hold both ends of that.

            // TRACK SPACE: `p` runs along the track from the value's own
            // zero -- the left end lying down, the BOTTOM standing up,
            // which is where a desk fader's zero is -- and `q` across it.
            // This is the one place a track rect becomes a screen rect, and
            // it is what lets the two orientations be one material:
            // vertical only means the track runs up. Lifted from
            // LevelMeter, which reads its scale the same way.
            rect_of: fn(p: float, q: float, plen: float, qlen: float) -> vec4 {
                let up = self.rect_size.y - p - plen
                return vec4(
                    mix(p, q, self.is_vertical),
                    mix(q, up, self.is_vertical),
                    mix(plen, qlen, self.is_vertical),
                    mix(qlen, plen, self.is_vertical)
                )
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let along = mix(self.rect_size.x, self.rect_size.y, self.is_vertical)
                let across = mix(self.rect_size.y, self.rect_size.x, self.is_vertical)

                // THE TRAVEL LAW, and it is RangeSlider's: the cap's CENTRE
                // is what the value means, so it stops half a cap in from
                // either inset end and never hangs off its own track. Rust
                // divides the drag by this same distance.
                let travel = max(along - self.inset_px * 2. - self.cap_px, 1.)
                let foot = self.inset_px + self.cap_px * 0.5

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover)
                    .mix(self.border_color_disabled, self.disabled)
                let val = self.val_color
                    .mix(self.val_color_focus, self.focus)
                    .mix(self.val_color_hover.mix(self.val_color_drag, self.drag), self.hover)
                    .mix(self.val_color_disabled, self.disabled)
                let grip = self.handle_color
                    .mix(self.handle_color_focus, self.focus)
                    .mix(self.handle_color_hover.mix(self.handle_color_drag, self.drag), self.hover)
                    .mix(self.handle_color_disabled, self.disabled)

                // The slot, the whole length of the track.
                let slot_w = min(self.track_size, across)
                let slot_q = (across - slot_w) * 0.5
                let slot = self.rect_of(
                    self.inset_px,
                    slot_q,
                    max(along - self.inset_px * 2., 1.),
                    slot_w
                )
                sdf.box(slot.x, slot.y, slot.z, slot.w, self.border_radius)
                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                // The bar, inside the slot. Both of its ends come from the
                // widget -- from the stop, or out of the default's own
                // place when the slider asks for it -- so a gain fader
                // grows its bar out of unity the way it was moved, and a
                // cut cannot look like a boost.
                let lo = foot + self.fill_lo * travel
                let hi = foot + self.fill_hi * travel
                let bar = self.rect_of(
                    lo,
                    slot_q + self.border_size,
                    max(hi - lo, 1.),
                    max(slot_w - self.border_size * 2., 1.)
                )
                sdf.box(bar.x, bar.y, bar.z, bar.w, self.border_radius)
                sdf.fill(val)

                // The cap, and the grip line across its middle -- the line
                // the value is read off, and what makes a cap a cap rather
                // than a block.
                let cap_w = min(max(across * self.cap_cross, 4.), across)
                let cap_q = (across - cap_w) * 0.5
                let cap_at = foot + self.slide_pos * travel
                let body = self.rect_of(cap_at - self.cap_px * 0.5, cap_q, self.cap_px, cap_w)
                sdf.box(body.x, body.y, body.z, body.w, self.border_radius)
                sdf.fill_keep(grip)
                sdf.stroke(stroke, self.border_size)

                let line = self.rect_of(
                    cap_at - self.cap_line * 0.5,
                    cap_q + self.border_size,
                    self.cap_line,
                    max(cap_w - self.border_size * 2., 1.)
                )
                sdf.rect(line.x, line.y, line.z, line.w)
                sdf.fill(stroke)

                return sdf.result
            }
        }
    }

    /** The fader stood on end: the same material and the same law read up the y axis, for a channel strip. */
    mod.widgets.SliderFaderY = mod.widgets.SliderFader{
        // One property moves both halves. `axis` is what the drag already
        // read; the material reads it too now, so a fader cannot end up
        // drawn one way and dragged the other.
        axis: Vertical
        width: 40.
        height: 160.
    }

    /** The round slider: a label column beside a pill track with a capsule value fill. */
    mod.widgets.SliderRoundFlat = mod.widgets.SliderMinimal{
        height: 18.
        margin: theme.mspace_1{top: theme.space_2}

        /** The pill track material: a rounded box with a capped value bar and a dot handle. */
        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            drag: instance(0.0)
            instance_val: instance(0.0)

            /** label column width, left of the track, in pixels 0..200 step 1 */
            label_size: 75.

            /** bevel gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_border_horizontal: uniform(0.0)
            /** fill gradient axis: 0 vertical, 1 horizontal 0..1 step 1 */
            gradient_fill_horizontal: uniform(0.0)

            /** exponent biasing the fill gradient along the track 1..20 step 0.5 */
            val_heat: uniform(10.)

            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius; the pill wants a large one 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius * 2.)

            /** dither the gradient fill to hide banding 0..1 step 1 */
            color_dither: uniform(1.0)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)
            color_drag: uniform(theme.color_inset_drag)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_inset_2_hover)
            color_2_focus: uniform(theme.color_inset_2_focus)
            color_2_disabled: uniform(theme.color_inset_2_disabled)
            color_2_drag: uniform(theme.color_inset_2_drag)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)
            border_color_drag: uniform(theme.color_bevel_drag)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_bevel_inset_2_hover)
            border_color_2_focus: uniform(theme.color_bevel_inset_2_focus)
            border_color_2_disabled: uniform(theme.color_bevel_inset_2_disabled)
            border_color_2_drag: uniform(theme.color_bevel_inset_2_drag)

            /** value bar inset from the track edge in pixels 0..20 step 0.5 */
            val_padding: uniform(2.5)

            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val_hover)
            val_color_focus: uniform(theme.color_val_focus)
            val_color_disabled: uniform(theme.color_val_disabled)
            val_color_drag: uniform(theme.color_val_drag)

            val_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            val_color_2_hover: uniform(theme.color_val_2_hover)
            val_color_2_focus: uniform(theme.color_val_2_focus)
            val_color_2_disabled: uniform(theme.color_val_2_disabled)
            val_color_2_drag: uniform(theme.color_val_2_drag)

            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            handle_color_focus: uniform(theme.color_handle_focus)
            handle_color_disabled: uniform(theme.color_handle_disabled)
            handle_color_drag: uniform(theme.color_handle_drag)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                let label_sz_uv = self.label_size / self.rect_size.x

                let handle_size = /** round handle radius 1..20 step 0.5 */ 4.0
                let padding = self.val_padding

                let track_length_bg = self.rect_size.x - self.label_size
                let padding_full = padding * 2.
                let min_size = padding_full + handle_size * 2.
                let track_length_val = self.rect_size.x - self.label_size - padding_full - min_size

                // Setup fill colors
                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_focus = self.color_focus
                let mut color_fill_drag = self.color_drag
                let mut color_fill_disabled = self.color_disabled

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let pos_x_heat = pow(self.pos.x, self.val_heat) - label_sz_uv
                    let gfx = pos_x_heat * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    let gfy = self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    let gradient_fill = vec2(gfx, gfy)
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    color_fill = mix(self.color, self.color_2, dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, dir)
                    color_fill_focus = mix(self.color_focus, self.color_2_focus, dir)
                    color_fill_drag = mix(self.color_drag, self.color_2_drag, dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, dir)
                }

                // Setup border colors
                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_focus = self.border_color_focus
                let mut color_stroke_drag = self.border_color_drag
                let mut color_stroke_disabled = self.border_color_disabled

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let gbx = self.pos.x + dither
                    let gby = self.pos.y + dither
                    let gradient_border = vec2(gbx, gby)
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, dir)
                    color_stroke_focus = mix(self.border_color_focus, self.border_color_2_focus, dir)
                    color_stroke_drag = mix(self.border_color_drag, self.border_color_2_drag, dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, dir)
                }

                // Setup val colors
                let mut val_fill = self.val_color
                let mut val_fill_hover = self.val_color_hover
                let mut val_fill_focus = self.val_color_focus
                let mut val_fill_drag = self.val_color_drag
                let mut val_fill_disabled = self.val_color_disabled

                if self.val_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let pos_x_heat = pow(self.pos.x, self.val_heat) - label_sz_uv
                    let dir = pos_x_heat * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    val_fill = mix(self.val_color, self.val_color_2, dir)
                    val_fill_hover = mix(self.val_color_hover, self.val_color_2_hover, dir)
                    val_fill_focus = mix(self.val_color_focus, self.val_color_2_focus, dir)
                    val_fill_drag = mix(self.val_color_drag, self.val_color_2_drag, dir)
                    val_fill_disabled = mix(self.val_color_disabled, self.val_color_2_disabled, dir)
                }

                // Background
                sdf.box(
                    self.label_size + self.border_size
                    self.border_size
                    track_length_bg - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                let bg_fill = color_fill
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_focus.mix(color_fill_hover.mix(color_fill_drag, self.drag), self.hover), self.focus)
                    .mix(color_fill_disabled, self.disabled)

                sdf.fill_keep(bg_fill)

                let bg_stroke = color_stroke
                    .mix(color_stroke_focus.mix(color_stroke_hover.mix(color_stroke_drag, self.drag), self.hover), self.focus)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.stroke(bg_stroke, self.border_size)

                // Amount bar
                let handle_shift = self.label_size + padding_full + handle_size
                let val_height = self.rect_size.y - padding_full - self.border_size * 2.
                let val_offset_x = self.label_size + padding + self.border_size + val_height * 0.5
                let val_target_x = track_length_val * self.slide_pos + min_size - self.border_size * 2. - val_height

                sdf.circle(
                    val_offset_x
                    self.rect_size.y * 0.5
                    val_height * 0.5
                )

                sdf.box(
                    val_offset_x
                    padding + self.border_size
                    val_target_x
                    self.rect_size.y - padding_full - self.border_size * 2.
                    /** value bar corner radius 0..8 step 0.5 */ 1.
                )

                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift
                    self.rect_size.y * 0.5
                    val_height * 0.5
                )

                let vfill = val_fill
                    .mix(val_fill_hover, self.hover)
                    .mix(val_fill_focus.mix(val_fill_hover.mix(val_fill_drag, self.drag), self.hover), self.focus)
                    .mix(val_fill_disabled, self.disabled)

                sdf.fill(vfill)

                // Handle
                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift
                    self.rect_size.y * 0.5
                    mix(0., handle_size, self.hover)
                )

                sdf.fill_keep(
                    self.handle_color
                        .mix(self.handle_color_hover, self.hover)
                        .mix(self.handle_color_focus.mix(self.handle_color_hover.mix(self.handle_color_drag, self.drag), self.hover), self.focus)
                        .mix(self.handle_color_disabled, self.disabled)
                )

                return sdf.result
            }
        }

        text_input: TextInput{
            width: Fit
            padding: 0.
            margin: Inset{right: 7.5, top: 1.0}

            draw_text +: {
                hover: instance(0.0)
                focus: instance(0.0)
                empty: instance(0.0)
                drag: instance(0.0)
                disabled: instance(0.0)

                color: theme.color_text_val
                color_hover: uniform(theme.color_text_hover)
                color_focus: uniform(theme.color_text_focus)
                color_drag: uniform(theme.color_text_down)
                color_disabled: uniform(theme.color_text_disabled)
                color_empty: uniform(theme.color_text_placeholder)
                color_empty_hover: uniform(theme.color_text_placeholder_hover)
                color_empty_focus: uniform(theme.color_text_focus)

                text_style: theme.font_regular{
                    font_size: theme.font_size_base
                }

                get_color: fn() {
                    return self.color
                        .mix(self.color_empty, self.empty)
                        .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                        .mix(self.color_focus.mix(self.color_hover, self.hover), self.focus)
                        .mix(self.color_disabled, self.disabled)
                }
            }

            draw_bg +: {
                border_size: 0.

                color: theme.color_u_hidden
                color_hover: theme.color_u_hidden
                color_focus: theme.color_u_hidden
                color_disabled: theme.color_u_hidden
                color_empty: theme.color_u_hidden
            }

            draw_selection +: {
                border_radius: theme.textselection_corner_radius

                color: theme.color_d_hidden
                color_hover: theme.color_d_hidden
                color_focus: theme.color_bg_highlight_inline
            }

        }

    }

    /** The standard round slider: the pill track plus the theme's inset bevel. */
    mod.widgets.SliderRound = mod.widgets.SliderRoundFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
            border_color_drag: theme.color_bevel_inset_1_drag

            border_color_2: theme.color_bevel_inset_2

            val_color: theme.color_val_1
            val_color_hover: theme.color_val_1_hover
            val_color_focus: theme.color_val_1_focus
            val_color_disabled: theme.color_val_1_disabled
            val_color_drag: theme.color_val_1_drag

            val_color_2: theme.color_val_2
        }
    }


    mod.widgets.SliderRoundGradientY = mod.widgets.SliderRound{
        draw_bg +: {
            color: theme.color_inset_1
            color_hover: theme.color_inset_1_hover
            color_focus: theme.color_inset_1_focus
            color_disabled: theme.color_inset_1_disabled
            color_drag: theme.color_inset_1_drag

            color_2: theme.color_inset_2
        }

    }

    mod.widgets.SliderRoundGradientX = mod.widgets.SliderRoundGradientY{
        draw_bg +: {
            gradient_border_horizontal: 1.0
            gradient_fill_horizontal: 1.0
        }
    }

    /** The rotary knob: the slider value drawn as an arc with the label below. */
    mod.widgets.RotaryFlat = mod.widgets.SliderMinimal{
        height: 95.
        width: 65.
        axis: Vertical
        flow: Right
        align: Align{x: 0., y: 0.0}
        label_walk: Walk{
            margin.top: 0
            width: Fill
        }
        text_input: TextInput{
            width: Fit
        }
        /** The knob material: an arc track with a bottom gap and a bevelled rim. */
        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            drag: instance(0.0)

            /** opening at the bottom of the arc in degrees 0..180 step 5 */
            gap: uniform(90.)
            /** rim bevel thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)

            /** arc thickness in pixels 1..30 step 0.5 */
            val_size: uniform(10.)
            /** inner arc inset in pixels 0..20 step 0.5 */
            val_padding: uniform(5.)

            /** dither the gradient fill to hide banding 0..1 step 1 */
            color_dither: uniform(1.)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)
            color_drag: uniform(theme.color_inset_drag)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_inset_2_hover)
            color_2_focus: uniform(theme.color_inset_2_focus)
            color_2_disabled: uniform(theme.color_inset_2_disabled)
            color_2_drag: uniform(theme.color_inset_2_drag)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_drag: uniform(theme.color_bevel_drag)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_bevel_inset_2_hover)
            border_color_2_drag: uniform(theme.color_bevel_inset_2_drag)
            border_color_2_focus: uniform(theme.color_bevel_inset_2_focus)
            border_color_2_disabled: uniform(theme.color_bevel_inset_2_disabled)

            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            handle_color_focus: uniform(theme.color_handle_focus)
            handle_color_disabled: uniform(theme.color_handle_disabled)
            handle_color_drag: uniform(theme.color_handle_drag)

            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val)
            val_color_focus: uniform(theme.color_val)
            val_color_disabled: uniform(theme.color_val_disabled)
            val_color_drag: uniform(theme.color_val_drag)

            val_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            val_color_2_hover: uniform(theme.color_val_2)
            val_color_2_focus: uniform(theme.color_val_2)
            val_color_2_disabled: uniform(theme.color_val_2_disabled)
            val_color_2_drag: uniform(theme.color_val_2_drag)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let one_deg = PI / 180
                let threesixty_deg = 2. * PI
                let gap_size = self.gap * one_deg
                let val_length = threesixty_deg - (one_deg * self.gap)
                let start = gap_size * 0.5
                let outer_end = start + val_length
                let val_end = start + val_length * self.slide_pos
                // Where the arc BEGINS. Off the stop by default; out of
                // the default's own angle when the slider asks, so a cut
                // and a boost point opposite ways round the rim and the
                // knob reads at a glance without its number.
                let origin_end = start + val_length * self.origin_pos
                let val_start = mix(start, min(origin_end, val_end), self.arc_origin)
                let val_stop = mix(val_end, max(origin_end, val_end), self.arc_origin)

                let label_offset_px = /** label reserve below knob 0..40 step 1 */ 20.
                let label_offset_uv = self.rect_size.y
                let scale_px = min(self.rect_size.x, self.rect_size.y - /** knob fit inset 0..8 step 0.5 */ 2. - theme.beveling)

                let scale_factor = scale_px * /** knob metric scale per px 0.005..0.05 step 0.002 */ 0.02

                let outer_width = self.val_size * scale_factor
                let radius_px = (scale_px - outer_width) * 0.5

                let center_px = vec2(
                    self.rect_size.x * 0.5
                    radius_px + outer_width * 0.5 + label_offset_px
                )

                let offset_px = vec2(
                    center_px.x - radius_px
                    label_offset_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x
                    offset_px.y / self.rect_size.y
                )

                let border_sz_px = vec2(
                    radius_px * 2.
                    radius_px * 2.
                )

                let gap_deg = self.gap * 0.25
                let gap_rad = gap_deg * PI / 180
                let arc_height_n = cos(gap_rad)
                let diam_px = radius_px * 2.
                let arc_height_px = diam_px * arc_height_n

                let scale_border = vec2(
                    self.rect_size.x / border_sz_px.x
                    self.rect_size.y / arc_height_px
                )

                let border_sz = self.border_size * scale_factor

                // Setup fill colors - use gradient_border.y as mix factor
                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_focus = self.color_focus
                let mut color_fill_drag = self.color_drag
                let mut color_fill_disabled = self.color_disabled

                let mut gradient_y = self.pos.y
                let mut gradient_down = pow(self.pos.y, /** bevel shade curve 0.5..4 step 0.1 */ 2.)
                let mut gradient_up = pow(self.pos.y, /** rim shade curve 0.1..2 step 0.05 */ 0.5)

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * /** dither grain 0..0.5 step 0.01 */ 0.04 * self.color_dither
                    let pos_y_adj = self.pos.y - offset_uv.y
                    let gbx = self.pos.x * scale_border.x + dither
                    let gby = pos_y_adj * scale_border.y + dither
                    gradient_y = gby
                    gradient_down = pow(gby, /** bevel shade curve 0.5..4 step 0.1 */ 2.)
                    gradient_up = pow(gby, /** rim shade curve 0.1..2 step 0.05 */ 0.5)
                    color_fill = mix(self.color, self.color_2, gby)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, gby)
                    color_fill_focus = mix(self.color_focus, self.color_2_focus, gby)
                    color_fill_drag = mix(self.color_drag, self.color_2_drag, gby)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, gby)
                }

                // Setup border colors
                let mut border_color_2 = self.border_color
                let mut border_color_2_hover = self.border_color_hover
                let mut border_color_2_focus = self.border_color_focus
                let mut border_color_2_drag = self.border_color_drag
                let mut border_color_2_disabled = self.border_color_disabled

                if self.border_color_2.x > -0.5 {
                    border_color_2 = self.border_color_2
                    border_color_2_hover = self.border_color_2_hover
                    border_color_2_focus = self.border_color_2_focus
                    border_color_2_drag = self.border_color_2_drag
                    border_color_2_disabled = self.border_color_2_disabled
                }

                // Setup val colors
                let mut val_color_2 = self.val_color
                let mut val_color_2_hover = self.val_color_hover
                let mut val_color_2_focus = self.val_color_focus
                let mut val_color_2_drag = self.val_color_drag
                let mut val_color_2_disabled = self.val_color_disabled

                if self.val_color_2.x > -0.5 {
                    val_color_2 = self.val_color_2
                    val_color_2_hover = self.val_color_2_hover
                    val_color_2_focus = self.val_color_2_focus
                    val_color_2_drag = self.val_color_2_drag
                    val_color_2_disabled = self.val_color_2_disabled
                }

                // Background
                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px
                    start
                    outer_end
                    outer_width
                )

                sdf.fill(
                    color_fill
                        .mix(color_fill_hover, self.hover)
                        .mix(color_fill_focus.mix(color_fill_hover.mix(color_fill_drag, self.drag), self.hover), self.focus)
                        .mix(color_fill_disabled, self.disabled)
                )

                sdf.arc_round_caps(
                    center_px.x
                    center_px.y - border_sz
                    radius_px
                    start
                    outer_end
                    border_sz * /** rim shadow width scale 1..8 step 0.5 */ 4.
                )

                sdf.fill(
                    mix(self.border_color, theme.color_d_hidden, gradient_up)
                        .mix(mix(self.border_color_hover, theme.color_d_hidden, gradient_up), self.hover)
                        .mix(mix(self.border_color_focus, theme.color_d_hidden, gradient_up).mix(mix(self.border_color_hover, theme.color_d_hidden, gradient_up).mix(mix(self.border_color_drag, theme.color_d_hidden, gradient_up), self.drag), self.hover), self.focus)
                        .mix(mix(self.border_color_disabled, theme.color_d_hidden, gradient_up), self.disabled)
                )

                // Track ridge
                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px
                    start
                    outer_end
                    border_sz * /** track ridge width scale 1..8 step 0.5 */ 4.
                )

                sdf.fill(
                    self.border_color
                        .mix(self.border_color_hover, self.hover)
                        .mix(self.border_color.mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover), self.focus)
                        .mix(self.border_color_disabled, self.disabled)
                )

                let inner_width = outer_width - self.val_padding * scale_factor

                // Value
                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px
                    val_start
                    val_stop
                    inner_width
                )

                sdf.fill(
                    mix(self.val_color, val_color_2, self.slide_pos)
                        .mix(mix(self.val_color_hover, val_color_2_hover, self.slide_pos), self.hover)
                        .mix(mix(self.val_color_focus, val_color_2_focus, self.slide_pos).mix(mix(self.val_color_focus, val_color_2_hover, self.slide_pos).mix(mix(self.val_color_drag, val_color_2_drag, self.slide_pos), self.drag), self.hover), self.focus)
                        .mix(mix(self.val_color_disabled, val_color_2_disabled, self.slide_pos), self.disabled)
                )

                // Handle
                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px
                    val_end
                    val_end
                    mix(
                        mix(0., inner_width, self.focus)
                        inner_width
                        self.hover
                    )
                )

                sdf.fill(
                    self.handle_color
                        .mix(self.handle_color_hover, self.hover)
                        .mix(self.handle_color_focus.mix(self.handle_color_hover.mix(self.handle_color_drag, self.drag), self.hover), self.focus)
                        .mix(self.handle_color_disabled, self.disabled)
                )

                // Bevel Outer
                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px + outer_width * 0.5 - border_sz * 0.5
                    start
                    outer_end
                    border_sz
                )

                sdf.fill(
                    mix(self.border_color, border_color_2, gradient_down)
                        .mix(mix(self.border_color_hover, border_color_2_hover, gradient_down), self.hover)
                        .mix(mix(self.border_color, border_color_2, gradient_down).mix(mix(self.border_color_hover, border_color_2_hover, gradient_down).mix(mix(self.border_color_drag, border_color_2_drag, gradient_down), self.drag), self.hover), self.focus)
                        .mix(mix(self.border_color_disabled, theme.color_d_hidden, gradient_down), self.disabled)
                )

                sdf.arc_round_caps(
                    center_px.x
                    center_px.y
                    radius_px - outer_width * 0.5 - border_sz * 0.5
                    start
                    outer_end
                    border_sz
                )

                sdf.fill(
                    mix(self.border_color, theme.color_u_hidden, gradient_up)
                        .mix(mix(self.border_color_hover, theme.color_u_hidden, gradient_up), self.hover)
                        .mix(mix(self.border_color_focus, theme.color_u_hidden, gradient_up).mix(mix(self.border_color_hover, theme.color_u_hidden, gradient_up).mix(mix(self.border_color_drag, theme.color_u_hidden, gradient_up), self.drag), self.hover), self.focus)
                        .mix(mix(self.border_color_disabled, theme.color_u_hidden, gradient_up), self.disabled)
                )

                return sdf.result
            }
        }
    }

    /** The standard rotary: the flat knob plus the theme's inset bevel and value gradient. */
    mod.widgets.Rotary = mod.widgets.RotaryFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_drag: theme.color_bevel_inset_1_drag
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled

            border_color_2: theme.color_bevel_inset_2

            val_color: theme.color_val_1
            val_color_hover: theme.color_val_1
            val_color_focus: theme.color_val_1
            val_color_disabled: theme.color_val_1_disabled
            val_color_drag: theme.color_val_1_drag

            val_color_2: theme.color_val_2
        }
    }

    mod.widgets.RotaryGradientY = mod.widgets.Rotary{
        draw_bg +: {
            color: theme.color_inset_1
            color_hover: theme.color_inset_1_hover
            color_focus: theme.color_inset_1_focus
            color_disabled: theme.color_inset_1_disabled
            color_drag: theme.color_inset_1_drag

            color_2: theme.color_inset_2
        }
    }

    /** The panel knob: one dark disc with the value cut into a groove near its edge, drawn to stay legible at 24 pixels square. */
    mod.widgets.RotaryKnob = mod.widgets.SliderMinimal{
        // 44 across, 64 down: the knob is the 44 and the 20 left over is
        // the legend row -- the same twenty pixels the rest of the family
        // reserves for a one-line label, in SliderMinimal's `offset_y`
        // uniform and RotaryFlat's `label_offset_px`. The shader
        // takes the biggest circle the box holds and pins it to the
        // bottom, so height over width IS the reserve; a caller who wants
        // the compact form writes the two the same and leaves the text
        // empty. The label row itself is font_size_p 10 by
        // font_wdgt_line_spacing 1.2 plus space_1 3, which is 15 in ALL
        // THREE themes -- the skeleton raises space_factor to 10 but then
        // writes space_1 as a literal 3 and font_size_p as a literal 10,
        // so nothing about that row moves. Twenty clears it everywhere.
        width: 44.
        height: 64.
        axis: Vertical

        // Written out although Layout's own default is already this,
        // because draw_walk_slider only takes the deferred-walk branch --
        // the branch that draws the label and the readout at all -- under
        // Flow::Right{wrap: false}. RotaryFlat states it for the same
        // reason. Nothing is logged if it is ever missed.
        flow: Right

        margin: theme.mspace_1
        align: Align{x: 0., y: 0.}
        label_align: Align{x: 0.5, y: 0.}

        // The whole range in 160 points of travel, whatever the knob is
        // drawn at. Left at the family default of 0 a drag divides by the
        // control's own height, which is 95 on a stock Rotary and would be
        // 24 here -- four percent of the range per pixel, on the size this
        // is built for. Shift does slow a drag down, but a rate a hand has
        // to hold a key to get is not the rate a knob should sit at. The
        // wheel is not the answer either: nothing marks a scroll consumed,
        // so a knob that took the wheel inside a scrolling panel would move
        // its value AND scroll the panel with the same gesture. `scroll_step`
        // stays off, here as everywhere else in the library.
        drag_travel: 160.

        // SliderMinimal's `label_walk` is INHERITED, not replaced, and both
        // of its sizes are load-bearing. Fill width is what makes
        // cx.defer_walk_turtle hand the row back at all: without it neither
        // the label nor the readout is drawn, and nothing says so. Fit
        // height is what keeps `label_area` on the legend row -- Size's own
        // default is Fill, so a replaced walk that does not say Fit gets a
        // label turtle the height of the whole control, and the family's
        // tap-the-label reset then fires anywhere on the face.

        // No readout. Four digits do not fit beside a 24 pixel knob, and on
        // a square one there is nowhere to put them: the label and the
        // readout are drawn INSIDE the same quad the knob fills, so a
        // readout with a size would land on the knob's own face. The field
        // still takes key focus on a press, so a value can be typed --
        // blind, which is the price. Give it a size back to see it. Merged
        // rather than replaced, so `empty_text`, `is_numeric_only`,
        // `is_read_only` and the hidden-background block survive; the
        // shape is `editor +:` on CodeView's editor and `scroll_bar_y +:`
        // on ScrollBars', both of them #[live] widget fields the base set
        // with a fresh instance, exactly as this one is.
        text_input +: {
            width: 0.
            height: 0.
        }

        /** The knob material: one dark disc, a groove carrying the value, and a needle. */
        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** opening at the bottom of the groove in degrees 20..180 step 5 */
            gap: uniform(90.)
            /** groove thickness as a fraction of the knob's radius 0.04..0.3 step 0.01 */
            ring_size: uniform(0.12)
            /** how much of the face the needle covers, from the groove inward 0.1..1 step 0.05 */
            pointer_length: uniform(0.55)
            /** the bezel hairline in pixels; it does not scale with the knob 0..4 step 0.25 */
            border_size: uniform(theme.beveling)

            // WHAT IS BEHIND WHAT, because every number below depends on
            // it. The ground is theme.color_bg_app -- window.rs:137 clears
            // every window to it and a storybook page does not cover it:
            // #4C4C4C dark, #D9D9D9 light, #DDDDDD skeleton. The only
            // thing this widget puts ON that ground is the disc and the
            // bezel stroked round it. Everything else -- the groove, the
            // lit arc, the needle -- is drawn INSIDE the disc, so the
            // material is their ground, in every theme, at every value.
            //
            // That is a geometry decision made by arithmetic. Float the
            // groove outside the disc, as a ring with page showing between
            // it and the face, and its ground becomes the page -- and in
            // the dark theme the page is a mid grey that nothing dark can
            // stand off: PURE BLACK on #4C4C4C is 2.44:1, so no dark
            // groove reaches the 3:1 a boundary wants, while a light one
            // has to reach #999999 before it does, which leaves under
            // 2.9:1 above it for the lit arc to live in. One ground, and
            // the numbers below are all there is to check.

            // THE MATERIAL. theme.color_opaque_d_5 is the darkest opaque
            // step every theme carries -- #171717, #353535, #3C3C3C, and
            // the skeleton literal is held to the generator by the drift
            // test in theme_tokens.rs. It does not step with state: one
            // rung up that ladder, theme.color_opaque_d_2, is #454545
            // against that #4C4C4C page, so a body that lifted under the
            // hand dissolved into the panel at the moment it was being
            // turned, and in the light themes it took the needle's own
            // contrast down with it. Against the page the material reads
            // 2.09:1, 8.69:1, 8.12:1.
            color: uniform(theme.color_opaque_d_5)

            // THE VALUE INK, for the lit arc and for the needle: two names
            // on one token so a caller can pull them apart. Not an accent
            // role -- theme.color_primary is a pale salmon in the dark
            // theme and a dark brick in the light one, both picked to read
            // on their own theme's PAGE, and both under 2:1 on this
            // material. The top of the opaque-up ladder is defined off
            // color_fg_app and is the brightest step every theme carries:
            // #DEDEDE, #F6F6F6, #FCFCFC, at 13.3:1, 11.4:1 and 10.8:1 on
            // the material, in every state, because the material never
            // moves.
            //
            // It holds that one colour through hover, focus and drag: it
            // is saying the value, and a lamp that also brightened under
            // the pointer would be saying two things with one colour.
            val_color: uniform(theme.color_opaque_u_6)
            handle_color: uniform(theme.color_opaque_u_6)

            // THE BEZEL, and it is the only element that meets the page.
            // theme.color_inverse_surface is the one token whose whole job
            // is to stand against the surface, and the only one that
            // changes SIDES with the theme: #DEDEDE in the dark theme,
            // #353535 and #3C3C3C in the light ones, which is 6.38:1,
            // 8.69:1 and 8.12:1 against the page. In the light themes it
            // resolves to the material itself, so there is no visible
            // bezel there -- and none is wanted, the material already
            // being 8:1 off its own page. In the dark theme it is the
            // whole reason a 24-square knob has an edge at all, the
            // material managing 2.09:1 there and nothing darker able to do
            // better.
            //
            // Disabled it takes the material and goes away. That is read
            // from `self.color` in the shader rather than written as a
            // token here, so it follows a caller who re-colours the
            // material instead of promising to and not doing it.
            border_color: uniform(theme.color_inverse_surface)

            // Inherited and not read by this shader: color_hover, _focus,
            // _drag and _disabled and their border and val siblings; the
            // whole _2 colour family; offset_y and handle_size. They still
            // take uniform slots and still appear in the tweaker sidebar
            // doing nothing. A RotaryKnobGradientY rung is where the _2
            // family would earn its place.

            // THE MATERIAL, from the theme, packed as `ReliefView` and
            // `RoundedView` pack theirs. Zero in every stock theme: at
            // `material` 0 this shader draws what it always drew.
            /** surface material tier: 0 flat, 1 relief, 2 relief with rim, gloss and specular 0..2 step 1 */
            material: uniform(theme.material_level)
            /** key light: direction (x right, y down, z out) and intensity */
            material_light: uniform(vec4(theme.material_light_x, theme.material_light_y, theme.material_light_z, theme.material_light_intensity))
            /** bevel width, profile curve, raise, specular */
            material_relief: uniform(vec4(theme.material_bevel_width, theme.material_bevel_curve, theme.material_raise, theme.material_specular))
            /** occlusion, rim, gloss, roughness */
            material_finish: uniform(vec4(theme.material_ao, theme.material_rim, theme.material_gloss, theme.material_roughness))
            /** face gradient, hairline, occlusion reach, sink */
            material_tune: uniform(vec4(theme.material_face_gradient, theme.material_hairline, theme.material_ao_reach, theme.material_sink))
            /** cast shadow strength, blur, falloff (0 linear 1 expo), contact occlusion */
            material_shadow: uniform(vec4(theme.material_shadow, theme.material_shadow_blur, theme.material_shadow_falloff, theme.material_contact_ao))
            /** inner shadow, inner blur, ground lip, glow */
            material_inner: uniform(vec4(theme.material_inner_shadow, theme.material_inner_radius, theme.material_ground_lip, theme.material_glow))
            /** the margin the disc keeps back from its box under a material, where its cast shadow falls, in points; held to a third of the radius 0..32 step 0.5 */
            material_margin: uniform(theme.material_margin)
            /** how far the dome rises: the slope of the cap at its rim, 0 a flat top 0..1.5 step 0.05 */
            material_dome: uniform(0.55)
            /** the ink a lit shoulder is tinted toward */
            material_light_ink: uniform(theme.color_material_light)
            /** the ink a shaded shoulder and the occlusion are tinted toward */
            material_shadow_ink: uniform(theme.color_material_shadow)
            /** the emissive ink the lit arc's halo takes */
            material_glow_ink: uniform(theme.color_material_glow)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                // The knob is the biggest circle the box holds, pinned to
                // the BOTTOM, so whatever height is left over is the legend
                // row: draw_walk_slider stacks the label and the readout
                // from the top. A square box is therefore all knob at any
                // size, and a box wider than it is tall has no legend row
                // at all. RotaryFlat instead reserves a fixed 20 pixels
                // inside its own shader, which is why it only fits when the
                // box is 20 taller than it is wide, and why at 24 square
                // its dial's centre lands below the bottom edge.
                let dia = min(self.rect_size.x, self.rect_size.y)
                let radius = dia * 0.5
                let center = vec2(self.rect_size.x * 0.5, self.rect_size.y - radius)

                // Every thickness below is a fraction of the radius under a
                // floor in pixels, and the fractions are chosen so that at
                // 24 across -- the size this is built for -- every one of
                // them is still the fraction: 0.6, 1.44, 1.08, 0.72, 0.547.
                // The first floor to bite is the material either side of
                // the groove, at 22.2 across; the rest follow at 21.9 (the
                // needle's width), 20.8 (the groove), 20.0 (the clearance)
                // and 16.7 (the needle's inset), and below that the drawing
                // grows a shade heavier than proportion asks. Those
                // crossovers are for the default groove: a caller who thins
                // it meets its floor much sooner, and at ring_size 0.04 the
                // groove draws its floor below about 62 across. The floors
                // themselves are half a pixel and a pixel because that is
                // where a stroke stops being a stroke and becomes a grey
                // smear, not because anything about the drawing wants them.
                // Only the bezel is absolute at every size, because a bezel
                // is a hairline and scaling it would make it a second
                // groove.
                let clearance = /** clearance between the bezel and the box, as a fraction of the radius 0..0.15 step 0.01 */ 0.05
                let bed = /** material either side of the groove, as a fraction of the radius 0..0.3 step 0.01 */ 0.09
                let ring_gap = max(radius * bed, 1.)
                // Under a material the disc keeps back from its box by the
                // margin its cast shadow falls into, held to a third of the
                // radius so a 24-square knob keeps a face. Zero without one.
                let margin = min(self.material_margin, radius / 3.0) * step(0.5, self.material)
                let disc_r = radius - max(radius * clearance, 0.5) - self.border_size - margin
                let ring_w = max(radius * self.ring_size, /** thinnest the groove may draw, in pixels 0.5..3 step 0.25 */ 1.25)
                let ring_r = disc_r - self.border_size - ring_gap - ring_w * 0.5
                let field_r = ring_r - ring_w * 0.5 - ring_gap

                // Sdf2d measures an arc from straight DOWN and turns
                // clockwise, so half the gap either side of the bottom
                // leaves the opening where a hand expects it and puts the
                // two stops on the lower corners. At a gap of 0 both stops
                // land on the SAME angle and the needle cannot tell the
                // ends apart, which is why the annotation on `gap` starts
                // at 20 rather than at 0.
                let one_deg = PI / 180
                let sweep = 2. * PI - self.gap * one_deg
                let start = self.gap * one_deg * 0.5
                let val_end = start + sweep * self.slide_pos
                let origin_end = start + sweep * self.origin_pos

                // Where the lit arc BEGINS: at the stop, or at the
                // default's own angle when the slider asks for it, so a cut
                // and a boost point opposite ways round the groove. Lifted
                // from RotaryFlat so the two materials agree exactly on
                // what arc_from_origin means.
                let val_start = mix(start, min(origin_end, val_end), self.arc_origin)
                let val_stop = mix(val_end, max(origin_end, val_end), self.arc_origin)

                // THE THREE INKS, and the arithmetic that places them. The
                // material is the ground for all of them.
                //
                //                     dark     light   skeleton
                //   material         #171717   #353535   #3C3C3C
                //   unlit track      #676767   #828282   #898989
                //   value ink        #DEDEDE   #F6F6F6   #FCFCFC
                //
                //   track on material   3.17      3.19      3.15
                //   ink on track        4.20      3.56      3.41
                //   ink on material    13.33     11.35     10.75
                //
                // The track is not a token, it is a RULE: the material
                // carried four tenths of the way to the ink. That is what
                // holds both steps over 3:1 in three themes whose ladders
                // are nothing like each other -- the light themes' opaque-up
                // rungs are all bunched within 1.4:1 of white and could not
                // have supplied the middle from a token. It also means the
                // track follows a caller who re-colours either end, instead
                // of being a fixed grey that suits only the default
                // material.
                let material = self.color
                let live_ink = self.val_color
                let track_mix = /** how far the unlit groove is carried from the material toward the ink 0.2..0.7 step 0.05 */ 0.4

                // WHAT SAYS DEAD: the groove goes out, all of it. The ink
                // falls to the material and the track is derived from the
                // ink, so both ends of the groove land on the material and
                // the knob becomes a plain disc. No live value can imitate
                // that -- value 0 still shows a full track with a lit cap
                // on the stop, and value 1 a fully lit groove. The needle
                // stays, at the track's own brightness: 13.33 down to 3.17
                // in the dark theme, 11.35 to 3.19 and 10.75 to 3.15 in the
                // other two. A dead knob still points.
                let ink = live_ink.mix(material, self.disabled)
                let track = material.mix(ink, track_mix)
                let needle = live_ink.mix(material.mix(live_ink, track_mix), self.disabled)
                let bezel = self.border_color.mix(material, self.disabled)

                // Hover, focus and drag are one thing here, and they are
                // said by SIZE rather than by colour. One thing because the
                // theme says so: three of the four bevel slots resolve to
                // the same two colours in every theme in the tree, and the
                // fourth is a key the skeleton does not define. Size
                // because colour has nowhere left to go -- that bevel pair
                // composited over this material steps 3.09:1 in the dark
                // theme but only 1.85:1 and 1.48:1 in the light ones, so a
                // colour step that reads in one theme is invisible in the
                // other two. A handle that grows out of nothing is the
                // family's own idiom (SliderMinimal's `handle_size`,
                // RotaryFlat's hover-grown dot) and it is the same step in
                // every theme, being no contrast at all. Here it is an
                // ADDITION to a pointer that is always drawn rather than a
                // substitute for one, which is what made it wrong on a knob
                // when RotaryFlat does it alone.
                let lifted = max(self.hover, max(self.focus, self.drag)) * (1. - self.disabled)

                // THE MATERIAL: the disc is a domed cap standing off the
                // page, throwing its shadow into the margin round it, the
                // shoulder rolling into a shallow dome so the whole cap
                // catches the light. Nothing changes at 0.
                var cap = material
                if self.material > 0.5 {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let q = p - center
                    let rr = length(q)
                    let d = rr - disc_r
                    var g = vec2(0.0, 1.0)
                    if rr > 0.00001 {
                        g = q / rr
                    }
                    let raise = self.material_relief.z * (1.0 + 0.25 * lifted) * (1.0 - self.disabled)
                    let off = Material.cast_offset(raise, self.material_light)
                    let sh = vec4(self.material_shadow.x, min(self.material_shadow.y, max(margin, 1.0) * 1.2), self.material_shadow.z, self.material_shadow.w)
                    var under = Material.cast(
                        d, length(q - off) - disc_r, length(q + off) - disc_r, g, px, raise, self.material_relief.z,
                        self.material_light, sh, self.material_inner.z,
                        self.material_shadow_ink.rgb, self.material_light_ink.rgb
                    ) * (1.0 - self.disabled)
                    // Faded out before the box edge, so it ends round.
                    under = under * smoothstep(0.0, max(margin, 1.0), radius - rr)
                    // `clear` premultiplies what it is given.
                    sdf.clear(vec4(under.rgb / max(under.a, 0.0001), under.a))
                    let t2 = step(1.5, self.material)
                    let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                    let rel = vec4(min(self.material_relief.x, disc_r * 0.5), self.material_relief.y, self.material_relief.z, self.material_relief.w * t2)
                    // No face gradient: a dome's normal carries its own.
                    let tune = vec4(0.0, self.material_tune.y, self.material_tune.z, self.material_tune.w)
                    // The dome: a paraboloid, its slope growing with the
                    // radius, over a raise that the normal multiplies back in.
                    let dome = self.material_dome * clamp(rr / max(disc_r, 0.001), 0.0, 1.0) / max(raise, 0.001)
                    let uv = q / (2.0 * disc_r) + vec2(0.5, 0.5)
                    cap = vec4(Material.face(
                        material.rgb, d, g, uv, raise, raise, dome, 0.0, 0.0,
                        self.material_light, rel, fin, tune, self.material_inner.x,
                        self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                    ), material.a)
                }

                // The disc, and the bezel on the same shape: fill_keep
                // hands the circle straight to the stroke, which lays the
                // width EITHER SIDE of it -- half on the material, half on
                // the page.
                sdf.circle(center.x, center.y, disc_r)
                sdf.fill_keep(cap)
                sdf.stroke(bezel, self.border_size)

                // The unlit groove, whole.
                sdf.arc_round_caps(
                    center.x
                    center.y
                    ring_r
                    start
                    start + sweep
                    ring_w
                )

                sdf.fill(track)

                // The lit arc, at the same radius and the same thickness,
                // so it fills the groove rather than sitting beside it.
                //
                // At rest the two angles are equal, and arc_round_caps with
                // a zero half-angle takes the cap branch for every pixel:
                // a single round cap sitting ON that angle. So a knob
                // filling from the stop shows a lamp at the stop rather
                // than nothing, and a bipolar one shows a lamp at its
                // resting angle. THAT is the centre mark. Once the value
                // moves off, one end of the arc is still at the resting
                // angle, so home is drawn by the arc itself at every value
                // and needs no mark of its own -- which is just as well,
                // because a mark would have to be painted rather than cut
                // (an SDF fill composites, it cannot erase) and at rest it
                // would land on the very cap it exists to explain.
                sdf.arc_round_caps(
                    center.x
                    center.y
                    ring_r
                    val_start
                    val_stop
                    ring_w
                )

                sdf.fill(ink)

                // The direction the value points, and the convention the
                // whole drawing shares: arc_round_caps rotates the pixel by
                // -start_angle and reads its cap at (0, radius), which
                // un-rotates through Math.rotate_2d to
                // center + radius * (-sin, cos). So this vector lands the
                // handle exactly on the arc's own end.
                let dir = vec2(-sin(val_end), cos(val_end))

                // The handle: nothing at rest, a dot as wide as the groove
                // is thick once the control has the pointer or the key
                // focus, sitting on the value's end of the arc and drawn
                // after it so it reads as the arc's head. Radius AND alpha
                // follow `lifted`, so at rest there is no sub-pixel speck
                // left where a zero-radius circle would still be evaluated.
                let handle_r = ring_w * /** handle size at full hover, as a multiple of the groove's thickness 0.5..2 step 0.1 */ 1.
                sdf.circle(
                    center.x + dir.x * ring_r
                    center.y + dir.y * ring_r
                    handle_r * lifted
                )
                sdf.fill(vec4(ink.rgb, ink.a * lifted))

                // The needle, always drawn, and pointing at the VALUE's own
                // angle rather than at either end of the arc.
                let tip = field_r - max(radius * /** needle inset from the groove, as a fraction of the radius 0..0.2 step 0.01 */ 0.06, 0.5)
                let heel = tip - tip * self.pointer_length
                sdf.move_to(center.x + dir.x * heel, center.y + dir.y * heel)
                sdf.line_to(center.x + dir.x * tip, center.y + dir.y * tip)

                // Sdf2d strokes this width EITHER SIDE of the line, so the
                // needle lands a shade under the groove's own thickness.
                sdf.stroke(
                    needle
                    max(ring_w * /** needle width as a fraction of the groove's 0.1..1 step 0.02 */ 0.38, 0.5)
                )

                return sdf.result
            }
        }
    }

}

/// The rate the modifiers held ask for: Shift fine (x0.2), plain (x1),
/// Ctrl fast (x4), Ctrl and Shift together faster still (x10).
///
/// The wheel and the drag read the SAME ladder, so a knob answers a held
/// Shift the same whichever gesture is moving it, and a hand that learns
/// the rates on one has them on the other.
pub(crate) fn modifier_ladder(modifiers: &KeyModifiers) -> f64 {
    match (modifiers.control, modifiers.shift) {
        (true, true) => 10.0,
        (true, false) => 4.0,
        (false, true) => 0.2,
        (false, false) => 1.0,
    }
}

/// Value delta for one scroll event: notch count (Windows wheels send 120
/// units per notch; trackpads send smaller deltas that accumulate over the
/// gesture) times the step fraction, scaled by `modifier_ladder`.
/// Scroll up (negative y) raises the value; a zero step disables wheel input.
pub(crate) fn wheel_value_delta(
    scroll: Vec2d,
    modifiers: &KeyModifiers,
    scroll_step: f64,
) -> f64 {
    if scroll_step == 0.0 {
        return 0.0;
    }
    let axis = if scroll.y != 0.0 { -scroll.y } else { -scroll.x };
    (axis / 120.0) * scroll_step * modifier_ladder(modifiers)
}

/// The pointer distance that covers a slider's whole range: the travel the
/// slider names, or the control's own extent along the drag axis when it
/// names none.
///
/// The fallback is the expression the drag divided by before there was a
/// number here, so a slider that names no travel behaves to the pixel as it
/// always did. A zero or a negative named travel is not a travel -- a zero
/// would divide into infinity and pin the value to a stop on the first pixel
/// of movement, a negative would run the drag backwards -- so both fall back
/// to it as well. Nothing here guards the fallback itself: a control drawn
/// to nothing still divides by nothing, exactly as it always has.
pub(crate) fn drag_span(drag_travel: f64, own: f64) -> f64 {
    if drag_travel > 0.0 {
        drag_travel
    } else {
        own
    }
}

/// How far ONE move of a drag takes the value: the pointer's movement since
/// the move before it, read along an axis, at the rate the modifiers held
/// ask for, over the span that covers the whole range.
///
/// Measured from the last move and not from the press, so the modifiers are
/// read afresh every move: press Shift halfway through and the pointer moves
/// the value five times finer FROM THERE ON, with the value staying exactly
/// where it was rather than jumping to where a whole drag at the finer rate
/// would have left it. Hold nothing and the moves add back up to the
/// press-to-now distance over the span, which is the number the drag was
/// before it was incremental.
///
/// `alt` swaps which way the pointer is read: a vertical knob or fader takes
/// the pointer's horizontal movement, a horizontal slider its vertical, and
/// right and up raise the value either way. The span does not swap with it --
/// the same pointer distance still crosses the whole range, only measured
/// across.
///
/// A span of nothing is not a span. Dividing by it gave an infinity, which
/// pinned the value to a stop; a running total cannot hold an infinity, so
/// the move takes the whole range instead, which leaves the value in the
/// same place.
pub(crate) fn drag_value_delta(
    delta_px: Vec2d,
    axis: DragAxis,
    alt: bool,
    modifiers: &KeyModifiers,
    span: f64,
) -> f64 {
    let along = match (axis, alt) {
        (DragAxis::Horizontal, false) => delta_px.x,
        (DragAxis::Horizontal, true) => -delta_px.y,
        (DragAxis::Vertical, false) => -delta_px.y,
        (DragAxis::Vertical, true) => delta_px.x,
    };
    if along == 0.0 {
        return 0.0;
    }
    let moved = along * modifier_ladder(modifiers) / span;
    if moved.is_nan() {
        0.0
    } else if moved.is_infinite() {
        moved.signum()
    } else {
        moved
    }
}

/// Where a fader's cap sits along its track, in points from the track's own
/// zero end, for a cap `cap` long on a control `length` long whose track is
/// held `inset` off both ends.
///
/// This is `RangeSlider`'s law, and it is here for its reason: the cap's
/// CENTRE is what the value means, so the centre stops half a cap in from
/// either end and never hangs off the track. A control that names neither a
/// cap nor an inset -- every slider in the tree but the faders -- is the
/// whole of its own length, to the point, which is what the drag divided by
/// before there were faders. The floor under the travel is RangeSlider's as
/// well: a cap with nowhere to go still has a distance to divide by rather
/// than a zero.
pub(crate) fn fader_cap_center(pos: f64, length: f64, cap: f64, inset: f64) -> f64 {
    inset + cap * 0.5 + pos * (length - inset * 2.0 - cap).max(1.0)
}

/// The value bar's two ends along the track, 0..1: from the low stop, or
/// out of the ORIGIN -- where the default sits on the track -- when the
/// slider asks for it.
///
/// Decided here rather than in a shader so that one law answers for the
/// drawing and for a test alike. A bipolar control grows its bar out of
/// unity the way it was moved, so a cut and a boost point opposite ways and
/// rest shows as nothing at all rather than as a half-filled track that
/// looks like a setting.
pub(crate) fn fill_span(origin: f64, pos: f64, from_origin: bool) -> (f64, f64) {
    if from_origin {
        (origin.min(pos), origin.max(pos))
    } else {
        (0.0, pos)
    }
}

#[derive(Copy, Clone, Debug, Script, ScriptHook)]
pub enum DragAxis {
    #[pick]
    Horizontal,
    Vertical,
}

/// How a slider's travel -- the 0..1 fraction of its track -- becomes the
/// value it reports, and back.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook)]
pub enum SliderTaper {
    /// The value is proportional to the travel; `step` floors it.
    #[pick]
    Linear,
    /// A gain knob's law: the default sits at half travel, the boost half
    /// runs straight to `max`, and the cut half is a square law that
    /// reaches `min` only at the stop -- fine near unity, fast near the
    /// kill. Falls back to Linear when the default is not strictly inside
    /// the range, because then there is no half to be either side of.
    Audio,
    /// Equal travel is equal ratio: a frequency knob. Falls back to Linear
    /// when the range admits no ratio (`min <= 0` or `max <= min`).
    Log,
    /// Detents: the NEAREST multiple of `step`, where Linear floors.
    /// Falls back to Linear without a step.
    Stepped,
    /// Two positions: `min` below half travel, `max` from half.
    Binary,
}

/// Travel to value. `Linear` is the map every slider had before there
/// were tapers, expression for expression, so a slider that names none
/// reports exactly what it did.
pub fn taper_to_value(
    taper: SliderTaper,
    travel: f64,
    min: f64,
    max: f64,
    default: f64,
    step: f64,
) -> f64 {
    let linear = |travel: f64| {
        let val = travel * (max - min);
        if step != 0.0 {
            (val / step).floor() * step + min
        } else {
            val + min
        }
    };
    let travel_in = if travel.is_finite() { travel.clamp(0.0, 1.0) } else { 0.0 };
    match taper {
        SliderTaper::Linear => linear(travel),
        SliderTaper::Audio => {
            if !(default > min && default < max) {
                return linear(travel);
            }
            if travel_in >= 0.5 {
                default + (max - default) * ((travel_in - 0.5) / 0.5)
            } else {
                let t = travel_in / 0.5;
                min + (default - min) * t * t
            }
        }
        SliderTaper::Log => {
            if min <= 0.0 || max <= min {
                return linear(travel);
            }
            min * (max / min).powf(travel_in)
        }
        SliderTaper::Stepped => {
            if step <= 0.0 || max <= min {
                return linear(travel);
            }
            (min + (travel_in * (max - min) / step).round() * step).min(max)
        }
        SliderTaper::Binary => {
            if travel_in >= 0.5 {
                max
            } else {
                min
            }
        }
    }
}

/// Value to travel: the inverse of [`taper_to_value`] on every taper,
/// so a value pushed in from outside lands the pointer where a hand
/// would have had to put it.
pub fn taper_to_travel(
    taper: SliderTaper,
    value: f64,
    min: f64,
    max: f64,
    default: f64,
    step: f64,
) -> f64 {
    let linear = |value: f64| (value - min) / (max - min);
    let _ = step;
    match taper {
        SliderTaper::Linear => linear(value),
        SliderTaper::Audio => {
            if !(default > min && default < max) {
                return linear(value);
            }
            let travel = if value >= default {
                0.5 + 0.5 * (value - default) / (max - default)
            } else {
                0.5 * ((value - min) / (default - min)).max(0.0).sqrt()
            };
            if travel.is_finite() { travel.clamp(0.0, 1.0) } else { 0.0 }
        }
        SliderTaper::Log => {
            if min <= 0.0 || max <= min {
                return linear(value);
            }
            if value <= 0.0 {
                return 0.0;
            }
            ((value / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
        }
        SliderTaper::Stepped => {
            if max <= min {
                return linear(value);
            }
            linear(value).clamp(0.0, 1.0)
        }
        SliderTaper::Binary => {
            if value >= (min + max) * 0.5 {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// The number a slider shows for its value, with its unit after a space
/// when it has one. A slider with no unit shows the bare number it
/// always did.
pub fn format_readout(value: f64, precision: usize, unit: &str) -> String {
    let number = match precision {
        0 => format!("{:.0}", value),
        1 => format!("{:.1}", value),
        2 => format!("{:.2}", value),
        3 => format!("{:.3}", value),
        4 => format!("{:.4}", value),
        5 => format!("{:.5}", value),
        6 => format!("{:.6}", value),
        7 => format!("{:.7}", value),
        _ => format!("{}", value),
    };
    if unit.is_empty() {
        number
    } else {
        format!("{number} {unit}")
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSlider {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    label_size: f32,
    #[live]
    slide_pos: f32,
    /// Where the default sits on the track, for a material that draws
    /// its value arc from there rather than from the stop.
    #[live]
    origin_pos: f32,
    /// 1.0 when the slider asks for that arc, else 0.0.
    #[live]
    arc_origin: f32,
    /// The value bar's two ends along the track, 0..1, worked out once in
    /// Rust by [`fill_span`]. The faces that predate it read `origin_pos`
    /// and `arc_origin` and work the same span out for themselves.
    #[live]
    fill_lo: f32,
    #[live]
    fill_hi: f32,
    /// 1.0 when the control lies along y, read off the widget's own `axis`,
    /// so a material drawn both ways cannot disagree with the drag about
    /// which way it lies.
    #[live]
    is_vertical: f32,
    /// The cap and the track inset the drag divides by, in points, handed
    /// to the shader so the two cannot drift apart.
    #[live]
    cap_px: f32,
    #[live]
    inset_px: f32,
}

#[derive(Script, Widget, Animator)]
pub struct Slider {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawSlider,

    #[walk]
    walk: Walk,

    #[live(DragAxis::Horizontal)]
    pub axis: DragAxis,

    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[rust]
    label_area: Area,
    #[live]
    label_walk: Walk,
    #[live]
    label_align: Align,
    #[live]
    draw_text: DrawText,
    #[live]
    text: String,

    #[live]
    text_input: TextInput,

    #[live]
    precision: usize,

    #[live]
    min: f64,
    #[live]
    max: f64,
    #[live]
    step: f64,
    #[live]
    default: f64,

    /// Fraction of the value range one scroll-wheel notch moves while the
    /// pointer hovers this slider. 0.0 (the default) disables wheel input.
    #[live]
    scroll_step: f64,

    /// How far the pointer must travel, in layout points, to cross the whole
    /// range. 0.0 -- the default -- means the control's own extent along the
    /// drag axis, which is what every slider did before there was a number
    /// here: the taller the box, the finer the drag.
    ///
    /// That is fine for a control drawn 95 points tall and coarse on one
    /// drawn 24 square, where it puts four percent of the range in a pixel.
    /// A held Shift divides that rate by five and a held Ctrl multiplies it
    /// by four -- `drag_value_delta` reads the same ladder on the drag as
    /// `wheel_value_delta` does on the wheel -- but a control that names a
    /// travel keeps its resolution at any size with no key held at all.
    #[live]
    drag_travel: f64,

    /// How the travel becomes the value. Linear, the plain map, unless
    /// the slider says otherwise.
    #[live(SliderTaper::Linear)]
    pub taper: SliderTaper,
    /// A unit the readout carries after the number -- "dB", "Hz" -- or
    /// nothing. Typed back with or without it, the number still lands.
    #[live]
    pub unit: String,
    /// What the readout multiplies the value by before showing it, so a
    /// parameter the engine keeps as 0..1 can read as 0..100 with a "%"
    /// beside it. The number typed back is divided by the same factor,
    /// so the readout and the box agree on what they mean. One by
    /// default: the value shown is the value held.
    #[live(1.0)]
    pub display_scale: f64,
    /// Draw the value arc from the default's position rather than from
    /// the stop, so a cut and a boost point different ways. A material
    /// reads it as `arc_origin` beside `origin_pos`; one that does not
    /// draws as before.
    #[live(false)]
    pub arc_from_origin: bool,

    /// The cap's length along the track, and how far the track is held off
    /// the control's two ends, in points.
    ///
    /// Rust owns them because the DRAG divides by the distance between the
    /// cap's stops, and the material is handed the same two numbers every
    /// draw; see [`fader_cap_center`]. Zero and zero -- every slider here
    /// but the faders -- leaves the drag dividing by the control's own
    /// extent exactly as it always did.
    #[live]
    cap_size: f64,
    #[live]
    track_inset: f64,

    #[live]
    bind: String,

    // Indicates if the label of the slider responds to hover events
    // The primary use case for this kind of emitted actions is for tooltips displaying
    // and it is turned on by default, since this component already consumes finger events
    #[live(true)]
    hover_actions_enabled: bool,

    #[rust]
    pub relative_value: f64,
    #[rust]
    pub dragging: Option<f64>,

    /// Where the pointer stood at the previous move of the drag in hand, so
    /// that every move is measured from the one before it and the modifiers
    /// held can be read afresh each time. Unset until the first move of a
    /// drag, which measures from the press itself.
    #[rust]
    drag_from: Option<Vec2d>,
    /// What the drag has added up to BEFORE the 0..1 clamp. A drag that runs
    /// past a stop has to come back the same distance before the value
    /// leaves that stop, which is what a drag measured from the press always
    /// did.
    #[rust]
    drag_travelled: f64,
}

impl ScriptHook for Slider {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.set_internal(self.default);
        vm.with_cx_mut(|cx| {
            self.update_text_input(cx);
        });
    }
}

#[derive(Clone, Debug, Default)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide(f64),
    /// The label was tapped and the control went back to its DSL
    /// `default:`. Carried by `slided` and `end_slide` alike, so a host
    /// that already listens for either needs no wiring for it.
    Reset(f64),
    LabelHoverIn(Rect),
    LabelHoverOut,
    #[default]
    None,
}

impl Slider {
    fn to_external(&self) -> f64 {
        taper_to_value(self.taper, self.relative_value, self.min, self.max, self.default, self.step)
    }

    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.relative_value;
        self.relative_value =
            taper_to_travel(self.taper, external, self.min, self.max, self.default, self.step);
        old != self.relative_value
    }

    pub fn update_text_input(&mut self, cx: &mut Cx) {
        let e = self.to_external() * self.display_scale;
        let text = format_readout(e, self.precision, &self.unit);
        // A unit is not a number, and the value box filters out
        // everything that is not one -- `is_numeric_only` resolves to
        // `InputMode::Decimal`, which strips on `set_text` as well as
        // on a keystroke. That is why a unit set in the DSL never
        // reached the screen. A slider carrying one takes the filter
        // off; typing is still checked, because the Returned handler
        // strips the unit, and anything that will not parse is
        // reverted by the next call to this function.
        let numeric_only = self.unit.is_empty();
        if self.text_input.is_numeric_only() != numeric_only {
            self.text_input.set_is_numeric_only(cx, numeric_only);
        }
        self.text_input.set_text(cx, &text);
        self.text_input.select_all(cx);
    }

    /// The pointer distance that covers the whole range on a control whose
    /// cap is drawn on its track: the journey the CAP makes between its two
    /// stops, so the finger and the cap arrive together. Without a cap and
    /// an inset it is the extent itself, which is what every slider here
    /// divided by before there were faders.
    fn cap_travel(&self, extent: f64) -> f64 {
        fader_cap_center(1.0, extent, self.cap_size, self.track_inset)
            - fader_cap_center(0.0, extent, self.cap_size, self.track_inset)
    }

    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.slide_pos = self.relative_value as f32;
        let origin =
            taper_to_travel(self.taper, self.default, self.min, self.max, self.default, self.step);
        self.draw_bg.origin_pos = origin as f32;
        self.draw_bg.arc_origin = if self.arc_from_origin { 1.0 } else { 0.0 };
        let (lo, hi) = fill_span(origin, self.relative_value, self.arc_from_origin);
        self.draw_bg.fill_lo = lo as f32;
        self.draw_bg.fill_hi = hi as f32;
        // The face and the drag read the same `axis`, so a fader cannot be
        // drawn one way and dragged the other.
        self.draw_bg.is_vertical = match self.axis {
            DragAxis::Vertical => 1.0,
            DragAxis::Horizontal => 0.0,
        };
        self.draw_bg.cap_px = self.cap_size as f32;
        self.draw_bg.inset_px = self.track_inset as f32;
        self.draw_bg.begin(cx, walk, self.layout);

        if let Flow::Right { wrap: false, .. } = self.layout.flow {
            if let Some(mut dw) = cx.defer_walk_turtle(self.label_walk) {
                //, (self.value*100.0) as usize);
                let walk = self.text_input.walk(cx);
                let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), walk);

                let label_walk = dw.resolve(cx);
                cx.begin_turtle(label_walk, Layout::default());
                self.draw_text
                    .draw_walk(cx, label_walk, self.label_align, &self.text);
                cx.end_turtle_with_area(&mut self.label_area);
            }
        } else {
            let walk = self.text_input.walk(cx);
            let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), walk);
            self.draw_text
                .draw_walk(cx, self.label_walk, self.label_align, &self.text);
        }

        self.draw_bg.end(cx);
    }

    pub fn value(&self) -> f64 {
        self.to_external()
    }

    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        // `set_internal` already answers the only question worth asking --
        // whether the TRAVEL moved. Comparing the requested value against
        // the old one instead said yes for a value that quantises or clamps
        // onto the travel already in effect, and then repainted for nothing.
        if self.set_internal(v) {
            self.update_text_input(cx);
            // And draw the material again. `update_text_input` only dirties
            // the readout's own area, which was the whole repaint a caller
            // pushing a value in ever got -- so a slider whose readout is
            // sized away, as a panel knob's is, got none at all and simply
            // did not move on screen. `reset_to_default` just below has
            // always done this; this one was missing it.
            self.draw_bg.redraw(cx);
        }
    }

    /// Snap back to the DSL `default:` value, as a title-click reset does.
    pub fn reset_to_default(&mut self, cx: &mut Cx) {
        self.set_internal(self.default);
        self.update_text_input(cx);
        self.draw_bg.redraw(cx);
    }
}

impl Widget for Slider {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(value) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.value()));
        }
        if method == live_id!(set_value) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if let Some(v) = value.as_f64() {
                    vm.with_cx_mut(|cx| {
                        self.set_value(cx, v);
                    });
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);

        for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::KeyFocus => {
                    self.animator_play(cx, ids!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animator_play(cx, ids!(focus.off));
                }
                TextInputAction::Returned(value, _modifiers) => {
                    // The readout is typed back the way it was shown, unit
                    // and all; the unit is not part of the number.
                    let typed = value.trim();
                    let typed = typed
                        .strip_suffix(self.unit.as_str())
                        .map(str::trim_end)
                        .unwrap_or(typed);
                    if let Ok(v) = typed.parse::<f64>() {
                        // Shown scaled, so read back scaled: a percent
                        // box hands back 0..100 for a 0..1 parameter.
                        let v = if self.display_scale != 0.0 {
                            v / self.display_scale
                        } else {
                            v
                        };
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    cx.widget_action(uid, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escaped => {
                    self.update_text_input(cx);
                }
                _ => (),
            }
        }

        // Where the pointer is over the WORD, for a tooltip to hang off. A
        // report, not a claim: the drag below is what owns this pointer.
        //
        // This asked with `capture_overload`, which buys a hover nothing --
        // `hits` reads that flag only on a press -- and cost the label a
        // co-capture of every press landing on it. The label sits across the
        // top of the slider's own face, so that co-capture ran first, marked
        // the press `handled` as the LABEL's, and the drag's own `hits` two
        // lines below then refused it: with hover actions on, a press that
        // started on the word could not slide the control at all.
        //
        // Hovers leave `hits` on these two events and no others, so asking on
        // them alone reports what it always reported and takes nothing.
        if self.hover_actions_enabled
            && matches!(event, Event::MouseMove(_) | Event::MouseLeave(_))
        {
            match event.hits(cx, self.label_area) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, SliderAction::LabelHoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, SliderAction::LabelHoverOut);
                }
                _ => (),
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                if self.animator_in_state(cx, ids!(disabled.on)) {
                    return ();
                }
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerScroll(e) => {
                if self.scroll_step > 0.0 && !self.animator_in_state(cx, ids!(disabled.on)) {
                    let delta = wheel_value_delta(e.scroll, &e.modifiers, self.scroll_step);
                    if delta != 0.0 && self.dragging.is_none() {
                        self.relative_value = (self.relative_value + delta).max(0.0).min(1.0);
                        self.set_internal(self.to_external());
                        self.draw_bg.redraw(cx);
                        self.update_text_input(cx);
                        cx.widget_action(uid, SliderAction::Slide(self.to_external()));
                        cx.widget_action(uid, SliderAction::EndSlide(self.to_external()));
                    }
                    // The wheel a slider took is spent: a list or a
                    // panel scrolling by the same wheel behind it -- an
                    // effect rack's rows -- reads the mark and stays
                    // where it is, so one notch moves the value and
                    // never also slides the slider out from under the
                    // pointer. Marked whenever the slider is the one
                    // wearing a step, not only when the notch moved it,
                    // so a slider at its end still holds the panel.
                    event.set_scroll_handled(Vec2Index::X);
                    event.set_scroll_handled(Vec2Index::Y);
                }
            }
            Hit::FingerDown(FingerDownEvent {
                // abs,
                // rect,
                device,
                tap_count,
                ..
            }) if device.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.on)) {
                    return ();
                }
                if tap_count == 2 {
                    self.reset_to_default(cx);
                    cx.widget_action(uid, SliderAction::Slide(self.to_external()));
                    return ();
                }
                // cx.set_key_focus(self.slider.area());
                // self.relative_value = ((abs.x - rect.pos.x) / rect.size.x ).max(0.0).min(1.0);
                self.update_text_input(cx);

                self.text_input.set_is_read_only(cx, true);
                self.text_input.set_key_focus(cx);
                self.text_input.select_all(cx);
                self.text_input.redraw(cx);

                self.animator_play(cx, ids!(drag.on));
                self.dragging = Some(self.relative_value);
                self.drag_from = None;
                self.drag_travelled = self.relative_value;
                cx.widget_action(uid, SliderAction::StartSlide);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.on)) {
                    return ();
                }

                self.text_input.set_is_read_only(cx, false);
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.force_new_edit_group();
                self.animator_play(cx, ids!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                self.dragging = None;
                self.drag_from = None;
                // A TAP on the label puts the control back to its DSL
                // default.
                //
                // The label was the one part of a slider that did
                // nothing at all: a drag here is RELATIVE, so a press
                // that lands on the text and does not move changes
                // nothing, and there was no other gesture on it. Yet the
                // default is exactly the value that is hard to get back
                // to by hand and exactly the one wanted back -- an EQ's
                // crossover corners, a width at unity, any bipolar
                // control at rest.
                //
                // `was_tap` and not a bare `is_over`, so a drag that
                // happened to BEGIN on the label ends as the drag it
                // was. And reset before the EndSlide below, so that
                // carries the new value rather than the old one.
                //
                // The empty check is not a nicety. The label's turtle is
                // walked whether or not there is text in it, so a slider
                // with no `text:` still has a `label_area` across its top,
                // and on a square knob that is half the face: a stationary
                // click to focus it threw the value away. No text, no
                // label, no reset.
                if fe.was_tap()
                    && !self.text.is_empty()
                    && self.label_area.rect(cx).contains(fe.abs_start)
                {
                    self.reset_to_default(cx);
                    cx.widget_action(uid, SliderAction::Reset(self.to_external()));
                }
                cx.widget_action(uid, SliderAction::EndSlide(self.to_external()));
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerMove(fe) => {
                if self.animator_in_state(cx, ids!(disabled.on)) {
                    return ();
                }

                if self.dragging.is_some() {
                    // The span is the one the control's OWN axis names, held
                    // even when Alt is turning the drag across it: what Alt
                    // changes is which way the pointer is read, not how far
                    // it has to go. Naming none, it is the journey the CAP
                    // makes between its stops, which on a control with no cap
                    // to hold it off the ends is the control's own extent --
                    // so a fader's cap and the finger arrive together.
                    let span = if let DragAxis::Horizontal = self.axis {
                        drag_span(
                            self.drag_travel,
                            self.cap_travel(
                                fe.rect.size.x - self.draw_bg.label_size as f64,
                            ),
                        )
                    } else {
                        drag_span(self.drag_travel, self.cap_travel(fe.rect.size.y))
                    };
                    // From the move before, or from the press on the first
                    // move, so the modifiers that count are the ones down
                    // NOW and a key taken or let go mid-drag changes the
                    // rate from here without moving the value.
                    let from = self.drag_from.unwrap_or(fe.abs_start);
                    self.drag_from = Some(fe.abs);
                    self.drag_travelled += drag_value_delta(
                        fe.abs - from,
                        self.axis,
                        fe.modifiers.alt,
                        &fe.modifiers,
                        span,
                    );
                    self.relative_value = self.drag_travelled.max(0.0).min(1.0);
                    self.set_internal(self.to_external());
                    self.draw_bg.redraw(cx);
                    self.update_text_input(cx);
                    cx.widget_action(uid, SliderAction::Slide(self.to_external()));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_slider(cx, walk);
        DrawStep::done()
    }

    fn text(&self) -> String {
        format!("{}", self.to_external())
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        // The number the readout shows, unit and all, so a test waits on
        // what a person reads rather than on a raw f64.
        Some(format_readout(
            self.to_external() * self.display_scale,
            self.precision,
            &self.unit,
        ))
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Ok(v) = v.parse::<f64>() {
            self.set_internal(v);
            self.update_text_input(cx);
            // The same repaint `set_value` was missing, for the same reason:
            // this is the other door into the drawn value, and a readout
            // sized to nothing swallows the only dirtying it did.
            // Unconditional, because unlike `set_value` this one always
            // re-normalises the readout -- a typed "5" on a 0..1 slider has
            // to come back as "1.00" even when the travel did not move.
            self.draw_bg.redraw(cx);
        }
    }
}

impl SliderRef {
    pub fn value(&self) -> Option<f64> {
        if let Some(inner) = self.borrow() {
            return Some(inner.value());
        }

        return None;
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v)
        }
    }

    /// Reset to the DSL default and return the value now in effect, so the
    /// caller can push it into whatever the slider is bound to.
    pub fn reset_to_default(&self, cx: &mut Cx) -> Option<f64> {
        let mut inner = self.borrow_mut()?;
        inner.reset_to_default(cx);
        Some(inner.to_external())
    }

    pub fn slided(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::TextSlide(v) | SliderAction::Slide(v) | SliderAction::Reset(v) => {
                    return Some(v)
                }
                _ => (),
            }
        }
        None
    }

    /// The hand has landed: a drag is starting, before it has moved. The
    /// wheel and a typed value never say this; they show up as `slided`.
    pub fn start_slide(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let SliderAction::StartSlide = item.cast() {
                return true;
            }
        }
        false
    }

    pub fn end_slide(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::EndSlide(v)
                | SliderAction::TextSlide(v)
                | SliderAction::Reset(v) => return Some(v),
                _ => (),
            }
        }
        None
    }

    pub fn label_hover_in(&self, actions: &Actions) -> Option<Rect> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::LabelHoverIn(rect) => Some(rect),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn label_hover_out(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::LabelHoverOut => true,
                _ => false,
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod taper_tests {
    use super::*;

    /// Linear is the old map, expression for expression: the floor on a
    /// stepped slider included.
    #[test]
    fn linear_taper_is_the_old_map() {
        let v = taper_to_value(SliderTaper::Linear, 0.35, 0.0, 2.0, 1.0, 0.0);
        assert_eq!(v, 0.35 * 2.0);
        let stepped = taper_to_value(SliderTaper::Linear, 0.35, 0.0, 2.0, 1.0, 0.25);
        assert_eq!(stepped, 0.5, "floors, as it always did");
        assert_eq!(taper_to_travel(SliderTaper::Linear, 0.7, 0.0, 2.0, 1.0, 0.0), 0.35);
        // Out of range stays out of range: nothing clamps what a caller
        // pushed past the stops, exactly as before.
        assert_eq!(taper_to_value(SliderTaper::Linear, 1.5, 0.0, 2.0, 1.0, 0.0), 3.0);
    }

    #[test]
    fn audio_taper_rests_at_default_mid_travel_and_squares_the_cut() {
        let at = |t| taper_to_value(SliderTaper::Audio, t, 0.0, 2.0, 1.0, 0.0);
        assert_eq!(at(0.5), 1.0);
        assert_eq!(at(1.0), 2.0);
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.75), 1.5);
        assert_eq!(at(0.25), 0.25, "a square law on the cut side");
        for i in 0..=100 {
            let t = i as f64 / 100.0;
            let back = taper_to_travel(SliderTaper::Audio, at(t), 0.0, 2.0, 1.0, 0.0);
            assert!((back - t).abs() < 1e-12, "{t} came back as {back}");
        }
        // The default lands at half travel wherever it sits in the range.
        assert_eq!(taper_to_value(SliderTaper::Audio, 0.5, 0.0, 1.5, 1.0, 0.0), 1.0);
        assert_eq!(taper_to_travel(SliderTaper::Audio, 1.0, 0.0, 1.5, 1.0, 0.0), 0.5);
        // A default on a stop has no two halves: Linear, and no NaN from
        // the widget's own zeroed defaults on construction.
        assert_eq!(taper_to_value(SliderTaper::Audio, 0.25, 0.0, 2.0, 0.0, 0.0), 0.5);
        assert_eq!(taper_to_travel(SliderTaper::Audio, 0.5, 0.0, 2.0, 2.0, 0.0), 0.25);
        assert_eq!(taper_to_travel(SliderTaper::Audio, 1.0, 0.0, 2.0, 0.0, 0.0), 0.5);
    }

    #[test]
    fn log_taper_is_straight_in_octaves_and_refuses_a_zero_floor() {
        let at = |t| taper_to_value(SliderTaper::Log, t, 20.0, 20_000.0, 1_000.0, 0.0);
        assert!((at(0.5) - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-6);
        let ratio = at(0.4) / at(0.3);
        assert!((at(0.8) / at(0.7) - ratio).abs() < 1e-9, "equal travel, equal ratio");
        let back = taper_to_travel(SliderTaper::Log, at(0.3), 20.0, 20_000.0, 1_000.0, 0.0);
        assert!((back - 0.3).abs() < 1e-12);
        assert_eq!(
            taper_to_value(SliderTaper::Log, 0.5, 0.0, 100.0, 50.0, 0.0),
            taper_to_value(SliderTaper::Linear, 0.5, 0.0, 100.0, 50.0, 0.0),
            "a floor of zero has no ratio to anything"
        );
    }

    #[test]
    fn stepped_rounds_to_the_nearest_detent_and_toggle_snaps() {
        let at = |t| taper_to_value(SliderTaper::Stepped, t, 0.0, 3.0, 0.0, 1.0);
        assert_eq!(at(0.6), 2.0);
        assert_eq!(at(0.49), 1.0);
        assert_eq!(at(1.0), 3.0, "the top detent is reachable");
        assert_eq!(taper_to_travel(SliderTaper::Stepped, 2.0, 0.0, 3.0, 0.0, 1.0), 2.0 / 3.0);
        let toggle = |t| taper_to_value(SliderTaper::Binary, t, 0.0, 1.0, 0.0, 0.0);
        assert_eq!(toggle(0.49), 0.0);
        assert_eq!(toggle(0.5), 1.0);
        assert_eq!(taper_to_travel(SliderTaper::Binary, 1.0, 0.0, 1.0, 0.0, 0.0), 1.0);
        assert_eq!(taper_to_travel(SliderTaper::Binary, 0.0, 0.0, 1.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn the_readout_carries_its_unit() {
        assert_eq!(format_readout(1.5, 2, "dB"), "1.50 dB");
        assert_eq!(format_readout(1.5, 2, ""), "1.50");
        assert_eq!(format_readout(1.5, 0, "%"), "2 %");
    }
}

#[cfg(test)]
mod style_reapply_tests {
    use super::*;
    use crate::desktop_style::{install, DesktopStyle, StyleSheet};

    #[test]
    fn style_reapply_preserves_slider_value() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let original = crate::script_eval!(vm, {use mod.widgets.* Slider{min: 0.0 max: 100.0 default: 25.0}});
            let mut slider = Slider::script_from_value(vm, original);
            assert!((slider.value() - 25.0).abs() < 1e-9);
            vm.with_cx_mut(|cx| slider.set_value(cx, 70.0));
            assert_eq!(slider.text_input.text(), "70.00");
            for (style, dark) in [(DesktopStyle::Macos, true), (DesktopStyle::Windows2000, false), (DesktopStyle::Omarchy, false)] {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(crate::script_mod);
                slider.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
                assert!(
                    (slider.value() - 70.0).abs() < 1e-9,
                    "{}: a slider's value must survive a style change, got {}",
                    style.id(),
                    slider.value()
                );
                assert_eq!(slider.text_input.text(), "70.00", "{}", style.id());
                assert!((slider.default - 25.0).abs() < 1e-9);
            }
        });
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    fn mods(control: bool, shift: bool) -> KeyModifiers {
        KeyModifiers { control, shift, alt: false, logo: false }
    }

    #[test]
    fn wheel_ladder() {
        // One Windows notch is scroll.y = -120 (wheel up) -> value moves UP.
        let up = Vec2d { x: 0.0, y: -120.0 };
        assert!((wheel_value_delta(up, &mods(false, false), 0.025) - 0.025).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(false, true), 0.025) - 0.005).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(true, false), 0.025) - 0.10).abs() < 1e-12);
        assert!((wheel_value_delta(up, &mods(true, true), 0.025) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn wheel_down_decreases() {
        let down = Vec2d { x: 0.0, y: 120.0 };
        assert!((wheel_value_delta(down, &mods(false, false), 0.025) + 0.025).abs() < 1e-12);
    }

    #[test]
    fn horizontal_axis_fallback() {
        // Tilt wheels / horizontal trackpad gestures land on x when y is 0.
        let tilt = Vec2d { x: -120.0, y: 0.0 };
        assert!((wheel_value_delta(tilt, &mods(false, false), 0.025) - 0.025).abs() < 1e-12);
    }

    #[test]
    fn zero_step_disables() {
        let up = Vec2d { x: 0.0, y: -120.0 };
        assert_eq!(wheel_value_delta(up, &mods(true, true), 0.0), 0.0);
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;

    #[test]
    fn a_named_travel_replaces_the_control_size() {
        // No number -- the family default -- is the control's own extent,
        // which is what the drag divided by before this existed. Every
        // slider in the tree is on this line.
        assert_eq!(drag_span(0.0, 95.0), 95.0);
        assert_eq!(drag_span(0.0, 24.0), 24.0);
        assert_eq!(drag_span(0.0, 0.0), 0.0);
        // A number: that distance, whatever the control is drawn at.
        assert_eq!(drag_span(160.0, 24.0), 160.0);
        assert_eq!(drag_span(160.0, 300.0), 160.0);
        // Neither zero nor a negative is a travel.
        assert_eq!(drag_span(-10.0, 24.0), 24.0);
    }

    fn mods(control: bool, shift: bool, alt: bool) -> KeyModifiers {
        KeyModifiers { control, shift, alt, logo: false }
    }

    #[test]
    fn a_drag_moves_as_far_as_before_with_no_modifier_held() {
        // The drag used to be one sum, (pointer - press) over the span. It is
        // a running total of moves now, and over the same pointer path with
        // no key held the total is the same number: every slider in the
        // library that nobody holds a key on drags exactly as it did.
        let span = 160.0;
        let path = [
            Vec2d { x: 0.0, y: 0.0 },
            Vec2d { x: 7.0, y: -13.0 },
            Vec2d { x: 3.0, y: -40.0 },
            Vec2d { x: -11.0, y: -12.5 },
        ];
        let whole = path[path.len() - 1] - path[0];
        let plain = mods(false, false, false);

        let mut summed = 0.0;
        for step in path.windows(2) {
            summed += drag_value_delta(step[1] - step[0], DragAxis::Vertical, false, &plain, span);
        }
        assert!((summed - (-whole.y / span)).abs() < 1e-12, "{}", summed);

        let mut summed = 0.0;
        for step in path.windows(2) {
            summed += drag_value_delta(step[1] - step[0], DragAxis::Horizontal, false, &plain, span);
        }
        assert!((summed - (whole.x / span)).abs() < 1e-12, "{}", summed);
    }

    #[test]
    fn shift_makes_a_drag_five_times_finer_and_ctrl_four_times_faster() {
        let span = 160.0;
        let up = Vec2d { x: 0.0, y: -16.0 };
        let plain = drag_value_delta(up, DragAxis::Vertical, false, &mods(false, false, false), span);
        assert!((plain - 0.1).abs() < 1e-12, "{}", plain);

        let shift = drag_value_delta(up, DragAxis::Vertical, false, &mods(false, true, false), span);
        assert!((shift - plain * 0.2).abs() < 1e-12, "{}", shift);
        let ctrl = drag_value_delta(up, DragAxis::Vertical, false, &mods(true, false, false), span);
        assert!((ctrl - plain * 4.0).abs() < 1e-12, "{}", ctrl);
        let both = drag_value_delta(up, DragAxis::Vertical, false, &mods(true, true, false), span);
        assert!((both - plain * 10.0).abs() < 1e-12, "{}", both);

        // The wheel's ladder and the drag's are one ladder.
        let notch = Vec2d { x: 0.0, y: -120.0 };
        let wheel_plain = wheel_value_delta(notch, &mods(false, false, false), 0.025);
        for held in [mods(false, true, false), mods(true, false, false), mods(true, true, false)] {
            let by_wheel = wheel_value_delta(notch, &held, 0.025) / wheel_plain;
            let by_drag = drag_value_delta(up, DragAxis::Vertical, false, &held, span) / plain;
            assert!((by_wheel - by_drag).abs() < 1e-12, "{} vs {}", by_wheel, by_drag);
        }
    }

    #[test]
    fn alt_reads_a_vertical_sliders_drag_from_the_horizontal() {
        let span = 100.0;
        let right = Vec2d { x: 25.0, y: 0.0 };
        let up = Vec2d { x: 0.0, y: -25.0 };
        let plain = mods(false, false, false);

        // Held on a knob or a fader: the pointer's across movement moves the
        // value, right raising it as up does, and up does nothing.
        assert!((drag_value_delta(right, DragAxis::Vertical, true, &plain, span) - 0.25).abs() < 1e-12);
        assert_eq!(drag_value_delta(up, DragAxis::Vertical, true, &plain, span), 0.0);
        // and on a horizontal slider the other way about.
        assert!((drag_value_delta(up, DragAxis::Horizontal, true, &plain, span) - 0.25).abs() < 1e-12);
        assert_eq!(drag_value_delta(right, DragAxis::Horizontal, true, &plain, span), 0.0);
        // Not held, each reads its own way, up and right raising the value.
        assert!((drag_value_delta(up, DragAxis::Vertical, false, &plain, span) - 0.25).abs() < 1e-12);
        assert!((drag_value_delta(right, DragAxis::Horizontal, false, &plain, span) - 0.25).abs() < 1e-12);
        // The rates go with it: Alt says which way, the ladder says how fast.
        let fine = drag_value_delta(right, DragAxis::Vertical, true, &mods(false, true, false), span);
        assert!((fine - 0.05).abs() < 1e-12, "{}", fine);
    }

    #[test]
    fn changing_a_modifier_mid_drag_changes_the_rate_without_a_jump() {
        let span = 100.0;
        let step = Vec2d { x: 0.0, y: -10.0 };
        let mut value = 0.0;
        for _ in 0..3 {
            value += drag_value_delta(step, DragAxis::Vertical, false, &mods(false, false, false), span);
        }
        assert!((value - 0.3).abs() < 1e-12, "{}", value);

        // Shift goes down with the drag still in hand: the value stays where
        // it stands and only the rate from here on is finer. It does not
        // jump to the 0.06 a whole drag at the fine rate would have reached.
        for _ in 0..3 {
            value += drag_value_delta(step, DragAxis::Vertical, false, &mods(false, true, false), span);
        }
        assert!((value - 0.36).abs() < 1e-12, "{}", value);

        // And let go again: the plain rate from that point, no jump back.
        value += drag_value_delta(step, DragAxis::Vertical, false, &mods(false, false, false), span);
        assert!((value - 0.46).abs() < 1e-12, "{}", value);
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to a slider.
///
/// The drag is the control and it owns the pointer from the press. What is
/// tested here is that nothing on the slider quietly takes that pointer first:
/// the label's hover report used to co-capture every press that landed on the
/// word, which marked the press as the LABEL's and left the drag refusing it.
#[cfg(test)]
mod pointer_capture_tests {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    pub(super) const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    pub(super) const WINDOW: WindowId = WindowId(1, 1);

    pub(super) struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        pub(super) fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        pub(super) fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    pub(super) fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    pub(super) fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    pub(super) fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    gain := Slider{
                        width: 300.
                        flow: Right
                        text: "gain"
                        hover_actions_enabled: true
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let gain = root.widget(cx, ids!(gain));
        (root, gain)
    }

    /// Where the WORD is, which is the part of the face the hover report
    /// covers.
    fn on_the_word(cx: &Cx, gain: &WidgetRef) -> Vec2d {
        let inner = gain.borrow::<Slider>().unwrap();
        let rect = inner.label_area.rect(cx);
        assert!(
            rect.size.x > 0.0 && rect.size.y > 0.0,
            "the label was drawn, or this test is pressing nothing"
        );
        rect.pos + rect.size * 0.5
    }

    fn started_sliding(actions: &ActionsBuf, gain: &WidgetRef) -> bool {
        actions.iter().filter_map(|a| a.as_widget_action()).any(|a| {
            a.widget_uid == gain.widget_uid()
                && matches!(a.cast::<SliderAction>(), SliderAction::StartSlide)
        })
    }

    /// A press on the word is still a press on the slider. With hover actions
    /// on, the label co-captured it and marked it handled, and the drag's own
    /// `hits` two lines later refused a press that was already spoken for --
    /// so the control could not be slid from its own legend at all.
    #[test]
    fn a_press_on_the_label_still_starts_the_slide() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, gain) = start(&mut cx);
        let at = on_the_word(&cx, &gain);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at));
        assert!(
            started_sliding(&actions, &gain),
            "the press on the word reached the drag"
        );
        cx.fingers.first_mouse_button = None;
    }

    /// And it is the FACE that holds the pointer, not the legend: one press,
    /// one owner. A second, stray capture is what makes every host around a
    /// slider stand its own gesture down for the wrong reason.
    #[test]
    fn the_face_is_the_only_thing_holding_that_press() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, gain) = start(&mut cx);
        let at = on_the_word(&cx, &gain);
        let face = gain.borrow::<Slider>().unwrap().draw_bg.area();
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at));
        assert!(
            cx.fingers.is_area_captured(face),
            "the drag took the pointer"
        );
        assert!(
            !cx.fingers.is_mouse_held_outside(&[face]),
            "and nothing else on the slider took a second hold of it"
        );
        cx.fingers.first_mouse_button = None;
    }
}

/// THE FADER, and the three things it has to get right: where the cap sits
/// on its track, which side of the origin the bar grows, and that standing
/// the control on end moves the HIT TEST with the drawing.
#[cfg(test)]
mod fader_tests {
    use super::pointer_capture_tests::{middle, press, send, Target, WINDOW};
    use super::*;
    use std::cell::Cell;

    /// The cap's two stops are half a cap in from the track's ends, and the
    /// middle of its travel is the middle of the control: an 18 point cap on
    /// a 200 point control held 6 off either end travels 170, from 15 to 185.
    #[test]
    fn the_cap_stops_half_a_cap_in_from_either_end() {
        let at = |pos| fader_cap_center(pos, 200.0, 18.0, 6.0);
        assert_eq!(at(0.0), 15.0);
        assert_eq!(at(1.0), 185.0);
        assert_eq!(at(0.5), 100.0, "the middle of the travel is the middle of the track");
        // Stood on end the same law is read back from the far edge, since a
        // desk fader's zero is at the BOTTOM: full value sits fifteen points
        // down from the top, and rest fifteen up from the bottom.
        assert_eq!(200.0 - at(1.0), 15.0);
        assert_eq!(200.0 - at(0.0), 185.0);
        // A control that names neither a cap nor an inset is the whole of
        // its own length, which is what the drag divided by before there
        // were faders.
        let span = |length, cap, inset| {
            fader_cap_center(1.0, length, cap, inset) - fader_cap_center(0.0, length, cap, inset)
        };
        assert_eq!(span(300.0, 0.0, 0.0), 300.0);
        assert_eq!(span(200.0, 18.0, 6.0), 170.0);
        // And a cap with nowhere left to go still leaves a distance to
        // divide by rather than a zero.
        assert_eq!(span(10.0, 18.0, 6.0), 1.0);
    }

    /// The bar grows out of the origin, on the side the value fell.
    #[test]
    fn the_centre_origin_bar_draws_on_the_side_the_value_fell() {
        assert_eq!(fill_span(0.5, 0.8, true), (0.5, 0.8), "a boost fills above unity");
        assert_eq!(fill_span(0.5, 0.2, true), (0.2, 0.5), "and a cut below it");
        assert_eq!(fill_span(0.5, 0.5, true), (0.5, 0.5), "at rest it is a line on the origin");
        // An origin that is not the middle is still the origin: a fader
        // whose unity sits at a quarter of its travel grows from the
        // quarter.
        assert_eq!(fill_span(0.25, 0.9, true), (0.25, 0.9));
        // Off, which is the family's default: from the stop, wherever the
        // default happens to sit.
        assert_eq!(fill_span(0.5, 0.2, false), (0.0, 0.2));
        assert_eq!(fill_span(0.5, 0.8, false), (0.0, 0.8));
    }

    /// The two presets and every property they are made of raise nothing.
    /// A theme token that does not exist, or a property the base has not
    /// got, is only a line in the running app's log; evaluated with that
    /// log held, both faces come up clean.
    #[test]
    fn the_presets_and_their_properties_raise_no_script_error() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let _ = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            let _ = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    level := SliderFader{cap_size: 24. track_inset: 4.}
                    gain := SliderFaderY{min: -1. max: 1. default: 0. arc_from_origin: true}
                }
            });
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:#?}");
        });
    }

    fn motion(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: Cell::new(Area::Empty),
        })
    }

    fn release(abs: Vec2d) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// One fader lying down and one standing up, the same length and the
    /// same cap, so the same gesture can be asked of both.
    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    flat := SliderFader{width: 300. height: 40. default: 0.5}
                    upright := SliderFaderY{width: 40. height: 300. default: 0.5}
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let flat = root.widget(cx, ids!(flat));
        let upright = root.widget(cx, ids!(upright));
        (root, flat, upright)
    }

    fn value_of(fader: &WidgetRef) -> f64 {
        fader.borrow::<Slider>().unwrap().value()
    }

    /// A press, a move and a release, the button recorded as held in
    /// between the way the platform records it.
    fn drag(cx: &mut Cx, root: &WidgetRef, from: Vec2d, by: Vec2d) {
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(cx, root, &press(from));
        send(cx, root, &motion(from + by));
        send(cx, root, &release(from + by));
        cx.fingers.first_mouse_button = None;
    }

    /// One gesture on one fader, and the value it left behind.
    ///
    /// A scene of its own for each, because a capture outlives its gesture
    /// here: releasing the mouse digit is the event loop's job -- the
    /// platform calls `Fingers::mouse_up` after dispatching the up -- and a
    /// unit test has no event loop. A second gesture on the same Cx would
    /// be handed to whatever held the first one, which is the very
    /// confusion this test exists to rule out.
    fn dragged(upright: bool, by: Vec2d) -> f64 {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, flat, standing) = start(&mut cx);
        let fader = if upright { standing } else { flat };
        let at = middle(&cx, &fader);
        drag(&mut cx, &root, at, by);
        value_of(&fader)
    }

    /// The axis flips the HIT TEST and not merely the drawing. The same
    /// gesture -- sixty points right and sixty points up -- is read off x by
    /// the fader lying down and off y by the one standing up, and since the
    /// two are the same length with the same cap they land on the same
    /// value. Each ignores the other's axis entirely.
    #[test]
    fn the_axis_flip_moves_the_hit_test_with_the_face() {
        let by = Vec2d { x: 60.0, y: -60.0 };
        let (a, b) = (dragged(false, by), dragged(true, by));
        assert!(a > 0.5, "the gesture moved the flat fader up its own axis: {a}");
        assert!((a - b).abs() < 1e-12, "the same gesture, read on each axis: {a} and {b}");
        // Sixty points of a 266 point travel: three hundred long, less an
        // eighteen point cap and eight off either end.
        assert!((a - (0.5 + 60.0 / 266.0)).abs() < 1e-12, "{a}");

        // And the cross axis does nothing at all to either.
        assert_eq!(
            dragged(false, Vec2d { x: 0.0, y: -60.0 }),
            0.5,
            "an upright gesture moved the flat fader"
        );
        assert_eq!(
            dragged(true, Vec2d { x: 60.0, y: 0.0 }),
            0.5,
            "a flat gesture moved the upright fader"
        );
    }

    /// The cap keeps the pointer for the whole drag, and nothing else takes
    /// a second hold of it. A strip of these lives inside a drag-scrolling
    /// column, and the column stands down for exactly as long as this
    /// capture lasts -- so a fader that let go of the pointer mid-drag would
    /// scroll the rack it sits in.
    #[test]
    fn the_cap_holds_the_pointer_for_the_whole_drag() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _flat, upright) = start(&mut cx);
        let face = upright.borrow::<Slider>().unwrap().draw_bg.area();
        let at = middle(&cx, &upright);

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at));
        assert!(cx.fingers.is_area_captured(face), "the press took the pointer");
        send(&mut cx, &root, &motion(at + Vec2d { x: 0.0, y: -40.0 }));
        assert!(cx.fingers.is_area_captured(face), "and still held it mid-drag");
        assert!(
            !cx.fingers.is_mouse_held_outside(&[face]),
            "something else took a second hold of the same press"
        );
        send(&mut cx, &root, &release(at + Vec2d { x: 0.0, y: -40.0 }));
        cx.fingers.first_mouse_button = None;
        assert!(value_of(&upright) > 0.5, "the drag moved the value");
    }
}
