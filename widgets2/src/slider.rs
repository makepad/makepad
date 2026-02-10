use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl},
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInput, TextInputAction},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.SliderBase = #(Slider::register_widget(vm))
    mod.widgets.DragAxis =  set_type_default() do #(DragAxis::script_api(vm))
    mod.widgets.splat(mod.widgets.DragAxis)

    use mod.widgets.*

    set_type_default() do #(DrawSlider::script_shader(vm)){
        ..mod.draw.DrawQuad // splat in draw quad
    }

    mod.widgets.SliderMinimal = set_type_default() do mod.widgets.SliderBase{
        min: 0.0
        max: 1.0
        step: 0.0
        label_align: Align{x: 0., y: 0.}
        margin: theme.mspace_1{top: theme.space_2}
        precision: 2.
        height: 25
        hover_actions_enabled: false

        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            drag: instance(0.0)
            disabled: instance(0.0)

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

            offset_y: uniform(20.)
            handle_size: uniform(20.)

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

                // Amount
                sdf.rect(
                    0
                    self.offset_y
                    self.slide_pos * self.rect_size.x
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
                    slider_height * 2.
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

        draw_text +: {
            hover: instance(0.0)
            focus: instance(0.0)
            empty: instance(0.0)
            drag: instance(0.0)
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

    mod.widgets.SliderFlat = mod.widgets.SliderMinimal{
        height: 36

        draw_bg +: {
            disabled: instance(0.0)

            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

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

            val_padding: uniform(5.)

            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val_hover)
            val_color_focus: uniform(theme.color_val_focus)
            val_color_drag: uniform(theme.color_val_drag)
            val_color_disabled: uniform(theme.color_val_disabled)

            handle_size: uniform(20.)
            bipolar: uniform(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let handle_sz = self.handle_size

                let offset_px = vec2(0., 20.)

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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                let offset_sides = self.border_size + 6.
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

                // Value line
                let track_length = self.rect_size.x - offset_sides * 4.
                let val_x = self.slide_pos * track_length + offset_sides * 2.
                let offset_top = self.rect_size.y - (self.rect_size.y - offset_px.y) * 0.5
                let move_x = mix(offset_sides, self.rect_size.x * 0.5, self.bipolar)

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
                let handle_padding = 1.5
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

    mod.widgets.SliderRoundFlat = mod.widgets.SliderMinimal{
        height: 18.
        margin: theme.mspace_1{top: theme.space_2}

        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            drag: instance(0.0)
            instance_val: instance(0.0)

            label_size: 75.

            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            val_heat: uniform(10.)

            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius * 2.)

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

                let handle_size = 4.0
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
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
                    1.
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
        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            drag: instance(0.0)

            gap: uniform(90.)
            border_size: uniform(theme.beveling)

            val_size: uniform(10.)
            val_padding: uniform(5.)

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

                let label_offset_px = 20.
                let label_offset_uv = self.rect_size.y
                let scale_px = min(self.rect_size.x, self.rect_size.y - 2. - theme.beveling)

                let scale_factor = scale_px * 0.02

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
                let mut gradient_down = pow(self.pos.y, 2.)
                let mut gradient_up = pow(self.pos.y, 0.5)

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let pos_y_adj = self.pos.y - offset_uv.y
                    let gbx = self.pos.x * scale_border.x + dither
                    let gby = pos_y_adj * scale_border.y + dither
                    gradient_y = gby
                    gradient_down = pow(gby, 2.)
                    gradient_up = pow(gby, 0.5)
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
                    border_sz * 4.
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
                    border_sz * 4.
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
                    start
                    val_end
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

#[derive(Copy, Clone, Debug, Script, ScriptHook)]
pub enum DragAxis {
    #[pick]
    Horizontal,
    Vertical,
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
}

#[derive(Script, Widget, Animator)]
pub struct Slider {
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
    LabelHoverIn(Rect),
    LabelHoverOut,
    #[default]
    None,
}

impl Slider {
    fn to_external(&self) -> f64 {
        let val = self.relative_value * (self.max - self.min);
        if self.step != 0.0 {
            return (val / self.step).floor() * self.step + self.min;
        } else {
            val + self.min
        }
    }

    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.relative_value;
        self.relative_value = (external - self.min) / (self.max - self.min);
        old != self.relative_value
    }

    pub fn update_text_input(&mut self, cx: &mut Cx) {
        let e = self.to_external();
        self.text_input.set_text(
            cx,
            &match self.precision {
                0 => format!("{:.0}", e),
                1 => format!("{:.1}", e),
                2 => format!("{:.2}", e),
                3 => format!("{:.3}", e),
                4 => format!("{:.4}", e),
                5 => format!("{:.5}", e),
                6 => format!("{:.6}", e),
                7 => format!("{:.7}", e),
                _ => format!("{}", e),
            },
        );
        self.text_input.select_all(cx);
    }

    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.slide_pos = self.relative_value as f32;
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

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);

        // alright lets match our designer against the slider backgdrop
        match event.hit_designer(cx, self.draw_bg.area()) {
            HitDesigner::DesignerPick(_e) => {
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _ => (),
        }

        for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::KeyFocus => {
                    self.animator_play(cx, ids!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animator_play(cx, ids!(focus.off));
                }
                TextInputAction::Returned(value, _modifiers) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    cx.widget_action(
                        uid,
                        &scope.path,
                        SliderAction::TextSlide(self.to_external()),
                    );
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
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverOut);
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
                cx.widget_action(uid, &scope.path, SliderAction::StartSlide);
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
                cx.widget_action(uid, &scope.path, SliderAction::EndSlide(self.to_external()));
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
                    cx.widget_action(uid, &scope.path, SliderAction::Slide(self.to_external()));
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

    pub fn slided(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::TextSlide(v) | SliderAction::Slide(v) => return Some(v),
                _ => (),
            }
        }
        None
    }

    pub fn end_slide(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                SliderAction::EndSlide(v) | SliderAction::TextSlide(v) => return Some(v),
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
