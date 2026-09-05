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

                // Draw main box
                sdf.box(
                    self.border_size
                    slider_top
                    slider_width
                    slider_bottom
                    self.border_radius
                )

                let fill = color_fill
                    .mix(color_fill_focus, self.focus)
                    .mix(color_fill_hover.mix(color_fill_drag, self.drag), self.hover)
                    .mix(color_fill_disabled, self.disabled)

                sdf.fill_keep(fill)

                let stroke = color_stroke
                    .mix(color_stroke_focus.mix(color_stroke_hover.mix(color_stroke_drag, self.drag), self.hover), self.focus)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.stroke(stroke, self.border_size)

                // Ridge
                let offset_sides = self.border_size + /** track side inset 0..20 step 0.5 */ 6.
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
                let ctrl_height = self.rect_size.y - offset_px.y
                let handle_x = self.slide_pos * (self.rect_size.x - handle_sz - offset_sides) - 3
                let handle_padding = /** handle vertical inset 0..6 step 0.5 */ 1.5
                sdf.box(
                    handle_x + offset_sides + self.border_size
                    offset_px.y + self.border_size + handle_padding
                    self.handle_size - self.border_size * 2.
                    ctrl_height - self.border_size * 2. - handle_padding * 2.
                    self.border_radius
                )

                let hfill = handle_fill
                    .mix(handle_fill_hover, self.hover)
                    .mix(handle_fill_focus.mix(handle_fill_hover.mix(handle_fill_drag, self.drag), self.hover), self.focus)
                    .mix(handle_fill_disabled, self.disabled)

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

}

/// Value delta for one scroll event: notch count (Windows wheels send 120
/// units per notch; trackpads send smaller deltas that accumulate over the
/// gesture) times the step fraction, scaled by the modifier ladder —
/// Shift fine (x0.2), plain (x1), Ctrl coarse (x4), Ctrl+Shift (x10).
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
    let ladder = match (modifiers.control, modifiers.shift) {
        (true, true) => 10.0,
        (true, false) => 4.0,
        (false, true) => 0.2,
        (false, false) => 1.0,
    };
    (axis / 120.0) * scroll_step * ladder
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
    Toggle,
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
        SliderTaper::Toggle => {
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
        SliderTaper::Toggle => {
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

    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.slide_pos = self.relative_value as f32;
        self.draw_bg.origin_pos =
            taper_to_travel(self.taper, self.default, self.min, self.max, self.default, self.step)
                as f32;
        self.draw_bg.arc_origin = if self.arc_from_origin { 1.0 } else { 0.0 };
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
        let prev_value = self.value();
        self.set_internal(v);
        if v != prev_value {
            self.update_text_input(cx);
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

        if self.hover_actions_enabled {
            match event.hits_with_capture_overload(cx, self.label_area, true) {
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
                }
            }
            Hit::FingerDown(FingerDownEvent {
                // abs,
                // rect,
                device,
                ..
            }) if device.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.on)) {
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
                if fe.was_tap() && self.label_area.rect(cx).contains(fe.abs_start) {
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

                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    if let DragAxis::Horizontal = self.axis {
                        self.relative_value = (start_pos
                            + rel.x / (fe.rect.size.x - self.draw_bg.label_size as f64))
                            .max(0.0)
                            .min(1.0);
                    } else {
                        self.relative_value = (start_pos - rel.y / fe.rect.size.y as f64)
                            .max(0.0)
                            .min(1.0);
                    }
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

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Ok(v) = v.parse::<f64>() {
            self.set_internal(v);
            self.update_text_input(cx);
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
        let toggle = |t| taper_to_value(SliderTaper::Toggle, t, 0.0, 1.0, 0.0, 0.0);
        assert_eq!(toggle(0.49), 0.0);
        assert_eq!(toggle(0.5), 1.0);
        assert_eq!(taper_to_travel(SliderTaper::Toggle, 1.0, 0.0, 1.0, 0.0, 0.0), 1.0);
        assert_eq!(taper_to_travel(SliderTaper::Toggle, 0.0, 0.0, 1.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn the_readout_carries_its_unit() {
        assert_eq!(format_readout(1.5, 2, "dB"), "1.50 dB");
        assert_eq!(format_readout(1.5, 2, ""), "1.50");
        assert_eq!(format_readout(1.5, 0, "%"), "2 %");
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
