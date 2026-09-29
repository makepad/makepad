use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncResult},
};

use crate::makepad_draw::DrawSvg;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CheckBoxBase = #(CheckBox::register_widget(vm))
    /** The three states a checkbox carries: Off, On, or Mixed for "some of the group". */
    mod.widgets.CheckState = #(CheckState::script_api(vm))

    /** The flat checkbox: an inset mark box with a stroked check, plus its label. */
    mod.widgets.CheckBoxFlat = set_type_default() do mod.widgets.CheckBoxBase{

        // The compact face this wears when its row runs out of width. Declared
        // with `:=` so it lands in the vec: a widget proto is frozen VALIDATED,
        // so a key its props do not list is a hard error at construction -- but
        // the checked path looks in the vec first, and declaring it once here
        // makes `tight: {...}` legal on every instance and every preset below.
        /** the face this wears when its row runs out of width */
        tight := {}
        width: Fit
        height: Fit
        padding: theme.mspace_2
        align: Align{x: 0., y: 0.}

        label_walk: Walk{
            width: Fit
            height: Fit
            margin: theme.mspace_h_1{left: 13.}
        }

        /** The mark box material: an SDF box with a stroked checkmark on top,
         * a dash while mixed, and the error ink over both when the intent says so. */
        draw_bg +: {
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** checked mix 0..1 step 0.01 */
            active: instance(0.0)
            /** mixed (partially checked) mix 0..1 step 0.01 */
            mixed: instance(0.0)
            /** error intent mix 0..1 step 0.01 */
            error: instance(0.0)
            /** pointer held down mix, for the toggle's knob growth 0..1 step 0.01 */
            pressed: instance(0.0)

            /** mark box side length in pixels 8..32 step 1 */
            size: uniform(15.0)
            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius, halved for the mark box 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** mark box at the end of the row instead of the start 0..1 step 1 */
            mark_at_end: uniform(0.0)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_down: uniform(theme.color_inset_down)
            color_active: uniform(theme.color_inset_active)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_down: uniform(theme.color_bevel_down)
            border_color_active: uniform(theme.color_bevel_active)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)
            /** bevel stroke under the error intent */
            border_color_error: uniform(theme.color_error)

            /** checkmark size as a fraction of the mark box 0..1 step 0.05 */
            mark_size: uniform(0.65)
            /** the check/knob ink, hidden while unchecked */
            mark_color: uniform(theme.color_u_hidden)
            mark_color_hover: uniform(theme.color_u_hidden)
            mark_color_down: uniform(theme.color_u_hidden)
            mark_color_active: uniform(theme.color_mark_active)
            mark_color_active_hover: uniform(theme.color_mark_active_hover)
            mark_color_focus: uniform(theme.color_mark_focus)
            mark_color_disabled: uniform(theme.color_mark_disabled)
            /** the dash ink while mixed */
            mark_color_mixed: uniform(theme.color_mark_active)
            /** the check, dash or knob ink under the error intent */
            mark_color_error: uniform(theme.color_error)

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
            /** how far the mark lifts toward the glow ink while active, and how far past full brightness 0..1 step 0.01 */
            material_ink: uniform(vec2(theme.material_ink_glow, theme.material_ink_lift))
            /** the ink a lit shoulder is tinted toward */
            material_light_ink: uniform(theme.color_material_light)
            /** the ink a shaded shoulder and the occlusion are tinted toward */
            material_shadow_ink: uniform(theme.color_material_shadow)
            /** the emissive ink an active well, its knob and its mark take */
            material_glow_ink: uniform(theme.color_material_glow)

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

            /** a well cut into the housing: the box centred on c (half size
             * h, corner k, distance d at p) lit as a sunken face, with the
             * surround's inner shadow over it -- the outline shifted
             * down-light and blurred -- and lit toward the glow ink by
             * `lit` under an illuminating material. Tier 1 is the relief
             * alone. */
            material_well: fn(fill: vec4, p: vec2, d: float, c: vec2, h: vec2, k: float, lit: float) -> vec4 {
                let g = self.material_grad(p, c, h, k)
                let elev = -self.material_tune.w * (1.0 - self.disabled)
                var insh = 0.0
                if self.material_inner.x > 0.001 && elev < 0.0 {
                    let ioff = Material.shadow_dir(self.material_light) * abs(elev) * 1.6
                    insh = 1.0 - Material.box_cov(c - h + ioff, c + h + ioff, p, max(self.material_inner.y * 0.5, 0.35), k)
                }
                let t2 = step(1.5, self.material)
                let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                let rel = vec4(self.material_relief.x, self.material_relief.y, self.material_relief.z, self.material_relief.w * t2)
                let uv = (p - c) / (2.0 * h) + vec2(0.5, 0.5)
                var o = Material.face(
                    fill.rgb, d, g, uv, elev, elev, 0.0, insh, 0.0,
                    self.material_light, rel, fin, self.material_tune, self.material_inner.x,
                    self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                )
                let glow = self.material_inner.w
                if glow > 0.001 {
                    o = mix(o, self.material_glow_ink.rgb, min(glow * 1.6, 1.0) * 0.72 * lit * (1.0 - self.disabled))
                }
                return vec4(o, fill.a)
            }

            /** a raised domed knob of radius r centred on kc, lit as one
             * object: the shoulder rolls into a shallow dome so the whole
             * cap catches the light. */
            material_knob: fn(fill: vec4, p: vec2, kc: vec2, r: float) -> vec4 {
                let q = p - kc
                let rr = length(q)
                let d = rr - r
                var g = vec2(0.0, 1.0)
                if rr > 0.00001 {
                    g = q / rr
                }
                let raise = self.material_relief.z * (1.0 - self.disabled)
                let t2 = step(1.5, self.material)
                let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                let rel = vec4(min(self.material_relief.x, r * 0.5), self.material_relief.y, self.material_relief.z, self.material_relief.w * t2)
                // No face gradient: a dome's normal carries its own.
                let tune = vec4(0.0, self.material_tune.y, self.material_tune.z, self.material_tune.w)
                // The dome: a paraboloid, its slope growing with the radius,
                // over a raise that m_normal multiplies back in.
                let dome = 0.55 * clamp(rr / max(r, 0.001), 0.0, 1.0) / max(raise, 0.001)
                let uv = q / (2.0 * r) + vec2(0.5, 0.5)
                let o = Material.face(
                    fill.rgb, d, g, uv, raise, raise, dome, 0.0, 0.0,
                    self.material_light, rel, fin, tune, self.material_inner.x,
                    self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                )
                return vec4(o, fill.a)
            }

            /** what a knob of radius r centred on kc throws on the ground at
             * p, premultiplied: its cast shadow, contact and lip, and its
             * glow by `lit`. Laid into the well's fill, since the knob
             * travels inside it. */
            material_knob_under: fn(p: vec2, kc: vec2, r: float, lit: float) -> vec4 {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let q = p - kc
                let rr = length(q)
                let d = rr - r
                var g = vec2(0.0, 1.0)
                if rr > 0.00001 {
                    g = q / rr
                }
                let raise = self.material_relief.z * (1.0 - self.disabled)
                let off = Material.cast_offset(raise, self.material_light)
                // The shadow falls inside the well: its blur stays in scale
                // with the knob.
                let sh = vec4(self.material_shadow.x, min(self.material_shadow.y, r), self.material_shadow.z, self.material_shadow.w)
                var under = Material.cast(
                    d, length(q - off) - r, length(q + off) - r, g, px, raise, self.material_relief.z,
                    self.material_light, sh, self.material_inner.z,
                    self.material_shadow_ink.rgb, self.material_light_ink.rgb
                ) * (1.0 - self.disabled)
                let glow = self.material_inner.w
                if glow > 0.001 {
                    let a3 = clamp(Material.tail(d, glow * 26.0, sh.z) * glow, 0.0, 1.0) * 0.85 * lit * (1.0 - self.disabled)
                    under = vec4(self.material_glow_ink.rgb * a3, a3) + under * (1.0 - a3)
                }
                return under
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let sz_px = self.size
                let center_px = vec2(sz_px * 0.5, self.rect_size.y * 0.5)
                // The mark box sits at the start of the row, or at its end
                // when the label goes first.
                let offset_px = vec2((self.rect_size.x - sz_px) * self.mark_at_end, center_px.y - sz_px * 0.5)

                //match self.check_type {
                //    CheckType.Check => {
                        // Draw background box
                        sdf.box(
                            offset_px.x + self.border_size
                            offset_px.y + self.border_size
                            sz_px - self.border_size * 2.
                            sz_px - self.border_size * 2.
                            self.border_radius * /** mark box corner scale 0..1 step 0.05 */ 0.5
                        )

                        var color_fill = self.color
                            .mix(self.color_focus, self.focus)
                            .mix(self.color_active, self.active)
                            .mix(self.color_hover, self.hover)
                            .mix(self.color_down, self.down)
                            .mix(self.color_disabled, self.disabled)

                        let color_stroke = self.border_color
                            .mix(self.border_color_focus, self.focus)
                            .mix(self.border_color_active, self.active)
                            .mix(self.border_color_hover, self.hover)
                            .mix(self.border_color_down, self.down)
                            .mix(self.border_color_error, self.error)
                            .mix(self.border_color_disabled, self.disabled)

                        // THE MATERIAL: the mark box is a well cut into the
                        // housing, lit up by an active mark under an
                        // illuminating material. Nothing changes at 0.
                        if self.material > 0.5 {
                            let p = self.pos * self.rect_size
                            let c = offset_px + vec2(sz_px * 0.5, sz_px * 0.5)
                            let h = max(vec2(sz_px * 0.5 - self.border_size, sz_px * 0.5 - self.border_size), vec2(0.5, 0.5))
                            // `sdf.box` draws a corner of TWICE its argument, clamped.
                            let k = min(self.border_radius, min(h.x, h.y))
                            color_fill = self.material_well(color_fill, p, sdf.shape, c, h, k, self.active)
                        }

                        sdf.fill_keep(color_fill)
                        sdf.stroke(color_stroke, self.border_size)

                        // Draw checkmark
                        let mark_padding = /** check inset frac 0.1..0.45 step 0.005 */ 0.275 * self.size
                        sdf.move_to(offset_px.x + mark_padding, center_px.y)
                        sdf.line_to(offset_px.x + center_px.x, center_px.y + sz_px * 0.5 - mark_padding)
                        sdf.line_to(offset_px.x + sz_px - mark_padding, offset_px.y + mark_padding)

                        var mark_color = self.mark_color
                            .mix(self.mark_color_hover, self.hover)
                            .mix(self.mark_color_active, self.active)
                            .mix(self.mark_color_error, self.error * self.active)
                            .mix(self.mark_color_disabled, self.disabled)

                        // Lit ink: an active mark toward the glow ink and
                        // past full brightness, one mix on a value already
                        // computed.
                        if self.material > 0.5 {
                            let lit = self.material_ink.x * self.active * (1.0 - self.disabled)
                            mark_color = vec4(mix(mark_color.rgb, self.material_glow_ink.rgb * self.material_ink.y, lit), mark_color.a)
                        }

                        sdf.stroke(mark_color, self.size * /** check stroke frac 0.02..0.2 step 0.005 */ 0.09)

                        // Draw the mixed dash: a bar across the box, faded in by the
                        // mixed instance. A zero-alpha stroke leaves the pixel alone.
                        let dash_inset = /** dash inset frac 0.1..0.45 step 0.005 */ 0.25 * self.size
                        sdf.move_to(offset_px.x + dash_inset, center_px.y)
                        sdf.line_to(offset_px.x + sz_px - dash_inset, center_px.y)
                        let dash_color = vec4(0., 0., 0., 0.)
                            .mix(self.mark_color_mixed.mix(self.mark_color_error, self.error).mix(self.mark_color_disabled, self.disabled), self.mixed)
                        sdf.stroke(dash_color, self.size * /** dash stroke frac 0.02..0.2 step 0.005 */ 0.1)
                //    }

                //    CheckType.None => {
                //        sdf.fill(vec4(0., 0., 0., 0.))
                //    }
                //}
                return sdf.result
            }
        }

        /** The checkbox label ink, state-mixed with the mark box. */
        draw_text +: {
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** pressed mix 0..1 step 0.01 */
            down: instance(0.0)
            /** checked mix 0..1 step 0.01 */
            active: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)
            /** error intent mix 0..1 step 0.01 */
            error: instance(0.0)

            ink_centered: true

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_down: uniform(theme.color_label_outer_down)
            color_focus: uniform(theme.color_label_outer_focus)
            color_active: uniform(theme.color_label_outer_active)
            color_disabled: uniform(theme.color_label_outer_disabled)
            /** label ink under the error intent */
            color_error: uniform(theme.color_error)

            get_color: fn() {
                return self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_error, self.error)
                    .mix(self.color_disabled, self.disabled)
            }
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
        }

        icon_walk: Walk{width: 14.0, height: Fit}

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
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
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
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
            /** mixed track: the dash, on while the state is Mixed */
            mixed: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {mixed: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {mixed: 1.0}
                    }
                }
            }
            /** press track: 1 while the pointer is held on the box */
            press: {
                default: @off
                off: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {pressed: 0.0}
                    }
                }
                on: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 1.0}
                    }
                }
            }
            /** error track: recolours the box, mark and label with the error ink */
            error: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {error: 0.0}
                        draw_text: {error: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {error: 1.0}
                        draw_text: {error: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.CheckBox = mod.widgets.CheckBoxFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_down: theme.color_bevel_inset_1_down
            border_color_active: theme.color_bevel_inset_1_active
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }

    /** The round checkbox: the standard mark box with its corners rounded
     * all the way, for pick-lists and avatars. */
    mod.widgets.CheckBoxCircle = mod.widgets.CheckBox{
        draw_bg +: {
            /** corner rounding radius; the box clamps it to a circle 0..999 step 0.5 */
            border_radius: theme.radius_full
        }
    }

    /** The flat toggle: the checkbox retuned as a pill with a sliding knob.
     * The knob drags along the track, grows while pressed by knob_grow, can
     * carry an icon per state, and the pill takes an outline while off. */
    mod.widgets.ToggleFlat = mod.widgets.CheckBoxFlat{
        label_walk +: {
            margin: theme.mspace_h_1{left: 27.}
        }

        /** the knob follows the pointer, and a release past the middle commits */
        draggable: true

        /** The pill material: a 1.6:1 box whose knob slides and fills on active. */
        draw_bg +: {
            mark_color: theme.color_label_outer
            mark_color_hover: theme.color_label_outer_active
            mark_color_down: theme.color_label_outer_down
            mark_color_active: theme.color_mark_active
            mark_color_active_hover: theme.color_mark_active_hover

            /** pill width as a multiple of its height 1..2.5 step 0.05 */
            pill_aspect: uniform(1.6)
            /** knob inset from the pill edge in pixels 0..6 step 0.5 */
            knob_inset: uniform(1.5)
            /** knob growth while pressed, as a fraction of its radius 0..1 step 0.05 */
            knob_grow: uniform(0.0)
            /** outline stroke while off, in pixels; 0 draws none 0..4 step 0.5 */
            outline_size: uniform(0.0)
            /** the outline ink while off */
            outline_color: uniform(theme.color_outline)
            /** 1 while the knob follows the pointer, driven by the widget 0..1 step 1 */
            drag: uniform(0.0)
            /** knob position while dragging, driven by the widget 0..1 step 0.01 */
            drag_pos: uniform(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let sz_px = vec2(self.size * self.pill_aspect, self.size)
                let center_px = vec2(sz_px.x * 0.5, self.rect_size.y * 0.5)
                // The pill sits at the start of the row, or at its end when
                // the label goes first.
                let offset_px = vec2((self.rect_size.x - sz_px.x) * self.mark_at_end, center_px.y - sz_px.y * 0.5)

                // Draw background pill
                sdf.box(
                    offset_px.x + self.border_size
                    offset_px.y + self.border_size
                    sz_px.x - self.border_size * 2.
                    sz_px.y - self.border_size * 2.
                    self.border_radius * self.size * /** pill corner scale 0..0.3 step 0.01 */ 0.1
                )

                var color_fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)

                let color_stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_active, self.active)
                    .mix(self.border_color_down, self.down)
                    .mix(self.border_color_error, self.error)
                    .mix(self.border_color_disabled, self.disabled)

                // The knob's geometry, before the pill is filled, because
                // under a material its shadow is laid INTO the pill's fill.
                // While dragging the knob follows drag_pos instead of the
                // active mix, and it grows while pressed.
                let knob_t = mix(self.active, self.drag_pos, self.drag)
                let mark_size = (sz_px.y * 0.5 - self.border_size - self.knob_inset) * (1.0 + self.pressed * self.knob_grow)
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + self.knob_inset
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * knob_t
                let kc = vec2(offset_px.x + mark_pos_y, center_px.y)

                // THE MATERIAL: the pill is a sunken track, the knob a raised
                // dome travelling in it, throwing its shadow on the track.
                // Nothing changes at 0.
                let p = self.pos * self.rect_size
                if self.material > 0.5 {
                    let c = offset_px + sz_px * 0.5
                    let h = max(sz_px * 0.5 - vec2(self.border_size, self.border_size), vec2(0.5, 0.5))
                    // `sdf.box` draws a corner of TWICE its argument, clamped.
                    let k = min(2.0 * self.border_radius * self.size * 0.1, min(h.x, h.y))
                    color_fill = self.material_well(color_fill, p, sdf.shape, c, h, k, 0.0)
                    let under = self.material_knob_under(p, kc, mark_size, self.active)
                    color_fill = vec4(mix(color_fill.rgb, under.rgb / max(under.a, 0.0001), under.a), color_fill.a)
                }

                sdf.fill_keep(color_fill)
                sdf.stroke(color_stroke, self.border_size)

                // The off outline: a second stroke on the pill edge that fades
                // out as the toggle turns on.
                if self.outline_size > 0.0 {
                    sdf.box(
                        offset_px.x + self.border_size
                        offset_px.y + self.border_size
                        sz_px.x - self.border_size * 2.
                        sz_px.y - self.border_size * 2.
                        self.border_radius * self.size * 0.1
                    )
                    sdf.stroke(self.outline_color.mix(vec4(0., 0., 0., 0.), self.active), self.outline_size)
                }

                let mark_color = self.mark_color
                    .mix(self.mark_color_hover, self.hover)
                    .mix(self.mark_color_active, self.active)
                    .mix(self.mark_color_error, self.error)
                    .mix(self.mark_color_disabled, self.disabled)

                if self.material > 0.5 {
                    // One solid knob, the same substance as the housing,
                    // carried toward the active ink as it turns on and lit
                    // toward the glow ink under an illuminating material.
                    var knob = self.color
                        .mix(self.mark_color_active, self.active)
                        .mix(self.mark_color_error, self.error)
                        .mix(self.mark_color_disabled, self.disabled)
                    let glow = self.material_inner.w
                    if glow > 0.001 {
                        knob = vec4(mix(knob.rgb, self.material_glow_ink.rgb, min(glow * 1.6, 1.0) * 0.72 * self.active * (1.0 - self.disabled)), knob.a)
                    }
                    sdf.circle(kc.x, kc.y, mark_size)
                    sdf.fill(self.material_knob(knob, p, kc, mark_size))
                } else {
                    // Draw ring when off, filled circle when on
                    sdf.circle(kc.x, kc.y, mark_size)
                    sdf.circle(kc.x, kc.y, mark_size * /** knob ring hole frac 0.1..0.9 step 0.05 */ 0.45)
                    sdf.subtract()

                    sdf.circle(kc.x, kc.y, mark_size)
                    sdf.blend(self.active)

                    sdf.fill(mark_color)
                }
                return sdf.result
            }
        }

        animator +: {
            active: {
                default: @off
                off: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.Toggle = mod.widgets.ToggleFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_down: theme.color_bevel_inset_1_down
            border_color_active: theme.color_bevel_inset_1_active
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }

    /** The rocker switch: the toggle's state drawn as a housing with two
     * halves, the one pressed in darker and lower. On presses the "I" half
     * at the end of the travel, off the "O" half. It flips on a click and
     * does not drag: a rocker has no knob to carry. */
    mod.widgets.ToggleRocker = mod.widgets.ToggleFlat{
        label_walk +: {
            margin: theme.mspace_h_1{left: 38.}
        }
        draggable: false

        draw_bg +: {
            /** housing width as a multiple of its height 1.2..3 step 0.05 */
            pill_aspect: 2.0
            /** the rocker's inset from the housing edge in points 0..4 step 0.25 */
            knob_inset: 1.5
            /** the raised half */
            rocker_color: uniform(theme.color_surface_bright)
            /** the half pressed in */
            rocker_color_pressed: uniform(theme.color_surface_dim)
            /** the legends: an "O" on the off half and an "I" on the on half */
            legend_color: uniform(theme.color_label_outer)
            /** the "I" legend while on */
            legend_on_color: uniform(theme.color_primary)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let sz = vec2(self.size * self.pill_aspect, self.size)
                // The housing's edges on whole device pixels, so its outline
                // is one pixel wide wherever the row put the switch.
                let o0 = vec2((self.rect_size.x - sz.x) * self.mark_at_end, self.rect_size.y * 0.5 - sz.y * 0.5)
                let lo = vec2(Finish.snap(self.rect_pos.x + o0.x, px), Finish.snap(self.rect_pos.y + o0.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + o0.x + sz.x, px), Finish.snap(self.rect_pos.y + o0.y + sz.y, px)) - self.rect_pos
                let o = lo
                let c = (lo + hi) * 0.5
                let h = (hi - lo) * 0.5
                let k = min(self.border_radius * 0.5, h.y)
                let housing = Material.sd_box(p, c, h, k)

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_error, self.error)
                    .mix(self.border_color_disabled, self.disabled)

                // The rocker inside the housing, split at the pivot. How far
                // each half is pressed in follows `active`, so the two trade
                // places as the state animates.
                // The inset on whole device pixels, so the rocker's edges
                // and the light line along its top land on pixel rows.
                let ins = Finish.snap(self.border_size + self.knob_inset, px)
                let hr = max(h - vec2(ins, ins), vec2(0.5, 0.5))
                let rocker = Material.sd_box(p, c, hr, max(k - ins, 0.0))
                let on_side = step(c.x, p.x)
                let pressed = mix(1.0 - on_side, on_side, self.active)
                // Each half is its colour laid over the housing's fill.
                let raised_col = vec4(mix(fill.rgb, self.rocker_color.rgb, self.rocker_color.a), max(fill.a, self.rocker_color.a))
                let pressed_col = vec4(mix(fill.rgb, self.rocker_color_pressed.rgb, self.rocker_color_pressed.a), max(fill.a, self.rocker_color_pressed.a))
                var cap = mix(raised_col, pressed_col, pressed)
                // The raised half tilts toward its outer end, so it is a
                // shade lighter there than at the pivot.
                let tilt = clamp(abs(p.x - c.x) / max(hr.x, 0.5), 0.0, 1.0)
                cap = vec4(cap.rgb * mix(1.0, 0.9 + 0.14 * tilt, 1.0 - pressed), cap.a)
                // The raised half catches the light along its top, the
                // pressed half has the housing's shadow across its top and
                // sits a device pixel lower.
                let top_edge = Finish.band(p.y - (c.y - hr.y), -px, 0.0, px)
                cap = vec4(mix(cap.rgb, Finish.lift(cap.rgb, 0.18), top_edge * (1.0 - pressed)), cap.a)
                let shade = 1.0 - smoothstep(0.0, hr.y * 0.9, p.y - (c.y - hr.y))
                cap = vec4(cap.rgb * (1.0 - 0.28 * shade * pressed), cap.a)
                // The break where the rocker pivots.
                let pivot = Finish.cover(abs(p.x - c.x) - px * 0.5, px)
                cap = vec4(mix(cap.rgb, cap.rgb * 0.45, pivot), cap.a)

                // The legends, a device pixel lower on the pressed half.
                let drop = px * pressed
                let at_o = vec2(c.x - hr.x * 0.5, c.y + drop)
                let at_i = vec2(c.x + hr.x * 0.5, c.y + drop)
                let ring = Finish.cover(abs(length(p - at_o) - hr.y * 0.34) - px * 0.6, px) * (1.0 - on_side)
                // The "I" is one device pixel wide, centred on a pixel column.
                let ix = Finish.snap(self.rect_pos.x + at_i.x, px) - self.rect_pos.x + px * 0.5
                let bar = Finish.cover(max(abs(p.x - ix) - px * 0.5, abs(p.y - at_i.y) - hr.y * 0.38), px) * on_side
                let legend = self.legend_color.mix(self.legend_on_color, self.active * on_side)
                cap = vec4(mix(cap.rgb, legend.rgb, (ring + bar) * legend.a), cap.a)

                var col = vec4(fill.rgb * fill.a, fill.a)
                col = mix(col, vec4(stroke.rgb * stroke.a, stroke.a), Finish.band(housing, 0.0, max(self.border_size, px), px))
                col = mix(col, vec4(cap.rgb * cap.a, cap.a), Finish.cover(rocker, px))
                // Disabled fades the whole switch rather than recolouring it.
                return col * Finish.cover(housing, px) * (1.0 - 0.55 * self.disabled)
            }
        }
    }

    /** The slide switch: a short slot with a knob that carries grip lines,
     * and a small mark in the free end of the slot, filled while on. The
     * knob follows a drag, as the toggle's does. */
    mod.widgets.ToggleSlide = mod.widgets.ToggleFlat{
        label_walk +: {
            margin: theme.mspace_h_1{left: 38.}
        }

        draw_bg +: {
            /** slot width as a multiple of its height 1.5..3 step 0.05 */
            pill_aspect: 2.0
            /** the knob's inset from the slot edge in points 0..4 step 0.25 */
            knob_inset: 1.0
            /** the knob */
            knob_color: uniform(theme.color_surface_bright)
            /** the mark in the free end of the slot while on */
            mark_on_color: uniform(theme.color_primary)
            /** the knob's grip lines, pitch in points 1..4 step 0.25 */
            grip_pitch: uniform(2.0)
            // The knob stands centred in the slot's height at both ends of
            // its travel; the drag and the knob icons read this to agree.
            knob_centred: uniform(1.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let sz = vec2(self.size * self.pill_aspect, self.size)
                // The housing's edges on whole device pixels, so its outline
                // is one pixel wide wherever the row put the switch.
                let o0 = vec2((self.rect_size.x - sz.x) * self.mark_at_end, self.rect_size.y * 0.5 - sz.y * 0.5)
                let lo = vec2(Finish.snap(self.rect_pos.x + o0.x, px), Finish.snap(self.rect_pos.y + o0.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + o0.x + sz.x, px), Finish.snap(self.rect_pos.y + o0.y + sz.y, px)) - self.rect_pos
                let o = lo
                let c = (lo + hi) * 0.5
                let h = (hi - lo) * 0.5
                let k = min(self.border_radius * 0.5, h.y)
                let slot = Material.sd_box(p, c, h, k)

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_error, self.error)
                    .mix(self.border_color_disabled, self.disabled)

                // The knob's place. A square knob stands as far clear of
                // the slot on every side at both ends, so it travels the
                // slot's length less its height; `knob_centred` tells the
                // drag and the knob icons to work it out the same way. Its
                // top and bottom sit on whole device pixels, so its rim and
                // the light line under it are one row each.
                let knob_t = mix(self.active, self.drag_pos, self.drag)
                // A whole number of device pixels, and as many fewer than
                // the slot as leaves the same clearance above and below.
                let slot_px = floor((hi.y - lo.y) / px + 0.5)
                var knob_px = max(floor(2.0 * (h.y - self.border_size - self.knob_inset) / px), 2.0)
                knob_px = knob_px - (slot_px - knob_px - 2.0 * floor((slot_px - knob_px) * 0.5))
                let ks = knob_px * 0.5 * px
                let k_top = lo.y + (slot_px - knob_px) * 0.5 * px
                let kc = vec2(lo.x + h.y + 2.0 * (h.x - h.y) * knob_t, k_top + ks)
                let kh = vec2(ks, ks)

                // The mark in the free end of the slot: a filled dot while
                // on, an open ring while off, on the side the knob left.
                let free = vec2(mix(lo.x + 2.0 * h.x - h.y, lo.x + h.y, knob_t), c.y)
                let mr = sz.y * 0.16
                let dot = Finish.cover(length(p - free) - mr, px)
                let ring = Finish.cover(abs(length(p - free) - mr) - px * 0.6, px)
                let on_ink = self.mark_on_color
                let off_ink = self.mark_color
                let mark = mix(ring, dot, self.active)
                let mark_ink = off_ink.mix(on_ink, self.active)

                var col = vec4(fill.rgb * fill.a, fill.a)
                col = Finish.over(col, vec4(mark_ink.rgb * mark_ink.a * mark, mark_ink.a * mark))
                col = mix(col, vec4(stroke.rgb * stroke.a, stroke.a), Finish.band(slot, 0.0, max(self.border_size, px), px))

                // The knob: a square block with rounded corners, a light
                // line along its top under the rim, and grip lines across
                // it, each line on a whole device pixel column.
                let kd = Material.sd_box(p, kc, kh, min(k, ks * 0.35))
                var knob = self.knob_color.rgb
                knob = mix(knob, Finish.lift(knob, 0.25), Finish.band(p.y - k_top, -px * 2.0, -px, px))
                let pitch = max(self.grip_pitch, px * 2.0)
                let gi = floor((p.x - kc.x) / pitch + 0.5)
                let gx = Finish.snap(self.rect_pos.x + kc.x + gi * pitch, px) - self.rect_pos.x
                let in_grip = step(abs(gi), 1.0) * step(abs(p.y - kc.y), ks * 0.5)
                let dark = Finish.cover(abs(p.x - (gx - px * 0.5)) - px * 0.5, px)
                let light = Finish.cover(abs(p.x - (gx + px * 0.5)) - px * 0.5, px)
                knob = mix(knob, knob * 0.55, dark * in_grip)
                knob = mix(knob, Finish.lift(knob, 0.3), light * in_grip)
                knob = mix(knob, knob * 0.6, Finish.band(kd, 0.0, px, px))
                col = mix(col, vec4(knob, 1.0), Finish.cover(kd, px))
                return col * Finish.cover(slot, px) * (1.0 - 0.55 * self.disabled)
            }
        }
    }

    /** The custom checkbox: no mark box drawn, for a caller-supplied icon. */
    mod.widgets.CheckBoxCustom = mod.widgets.CheckBox{
        width: Fit
        height: Fit
        padding: theme.mspace_2
        align: Align{x: 0., y: 0.5}

        label_walk +: {
            margin: theme.mspace_h_2
        }

        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.fill(vec4(0., 0., 0., 0.))
                return sdf.result
            }
        }
    }

    /** The icon-only checkbox: the custom face carrying a mark of its own
     * for each state and nothing else in the row. Give it a `draw_icon_off`
     * and a `draw_icon_on`, each with its svg and its ink; the one for the
     * state it is in is what it draws, in the middle of the face, with no
     * word beside it.
     *
     * The two states are handed different inks here, and a caller that
     * retunes them should keep them apart. A mark drawn at this size moves
     * two or three pixels between one shape and the other, so the colour is
     * what a person reads across a column of them.
     *
     * What the two states MEAN is the caller's: a padlock in front of a row,
     * an eye over a layer, a pin on a panel. The control itself only turns
     * over, says which side it is on, and reports the change. */
    mod.widgets.CheckBoxIcon = mod.widgets.CheckBoxCustom{
        /** icon only: the label is empty */
        text: ""
        align: Align{x: 0.5, y: 0.5}
        padding: theme.mspace_1

        /** no gap: an empty label still takes the margin that clears a
         * mark box, and that margin is what pushes the icon off centre */
        label_walk: Walk{
            width: Fit
            height: Fit
            margin: Inset{top: 0., right: 0., bottom: 0., left: 0.}
        }

        icon_walk: Walk{width: 14.0, height: 14.0}

        /** the mark while off: the quieter of the two inks */
        draw_icon_off +: {
            color: theme.color_label_outer_off
        }
        /** the mark while on: the label's own ink, a step brighter */
        draw_icon_on +: {
            color: theme.color_label_outer
        }

        // The mark is the whole control, so the mark is what has to go
        // dim. On every other face the disabled track works through the
        // box and the label, and this face has neither: left alone, a dead
        // lock was drawn exactly like a live one.
        //
        // It is the mark's opacity that falls, not its colour. A track that
        // restored two inks would also hand them back on the way out, and
        // the pair is exactly what a caller is expected to retune -- one
        // press of a disabled switch would have thrown their colours away.
        animator +: {
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon_off: {opacity: 1.0}
                        draw_icon_on: {opacity: 1.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        /** how much of a dead mark is left 0..1 step 0.05 */
                        draw_icon_off: {opacity: 0.35}
                        draw_icon_on: {opacity: 0.35}
                    }
                }
            }
        }
    }
}

/// The three states a checkbox carries. `Mixed` is the group's "some of
/// them" state: it draws a dash, reports as not checked, and a click on it
/// resolves to `On`, the way a select-all box behaves.
#[derive(Copy, Clone, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum CheckState {
    #[pick]
    #[default]
    Off,
    On,
    Mixed,
}

#[derive(Script, Widget, Animator)]
pub struct CheckBox {
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

    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[live]
    label_align: Align,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,

    #[live]
    draw_icon: DrawSvg,

    #[live]
    text: ArcStringMut,

    #[visible]
    #[live(true)]
    #[apply_state]
    pub visible: bool,

    #[live(None)]
    #[apply_state]
    pub active: Option<bool>,

    /// The state the box starts in; `active`, when given, wins over it.
    /// Kept in step with the animator afterwards so reflection reads true.
    #[live]
    pub state: CheckState,

    /// The error intent: the box, mark and label take the error ink.
    #[live]
    pub error: bool,

    /// Draw the label first and the mark box after it, at the end of the row.
    #[live]
    pub label_before: bool,

    /// The knob follows the pointer: a drag along the track moves it and a
    /// release commits the side it is on. A plain click still flips on press.
    #[live]
    pub draggable: bool,

    /// The icon drawn while the box is on; leave the svg unset for none.
    ///
    /// On the toggle it rides the knob. On a face with no knob -- the
    /// icon-only checkbox -- it stands in the row, in `draw_icon`'s place,
    /// and it is then the whole of what the control shows: its ink is what
    /// says which state the box is in.
    #[live]
    pub draw_icon_on: DrawSvg,
    /// The icon drawn while the box is off, in the same place as its
    /// opposite. Its ink is the other half of the pair and belongs apart
    /// from the on one.
    #[live]
    pub draw_icon_off: DrawSvg,
    /// Knob icon side as a fraction of the knob's diameter.
    #[live(0.6)]
    pub knob_icon_size: f64,

    /// The label shown while on; empty keeps `text`.
    #[live]
    pub text_on: String,
    /// The label shown while off; empty keeps `text`.
    #[live]
    pub text_off: String,

    #[live]
    on_click: ScriptFnRef,

    #[live]
    bind: String,
    #[action_data]
    #[rust]
    action_data: WidgetActionData,

    #[rust]
    drag: Option<KnobDrag>,
}

/// A drag in progress on the toggle's knob.
#[derive(Clone, Copy, Debug)]
struct KnobDrag {
    /// Where the pointer was when the knob started following it.
    start_x: f64,
    /// The knob's position along the track at that moment.
    start_pos: f32,
    /// The knob's position now, 0 (off) to 1 (on).
    pos: f32,
    /// The pointer travelled past the slop, so the knob follows it.
    moved: bool,
}

/// Pointer travel, in layout points, before a press becomes a drag.
const KNOB_DRAG_SLOP: f64 = 3.0;

impl ScriptHook for CheckBox {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        let initial = match self.active.take() {
            Some(true) => CheckState::On,
            Some(false) => CheckState::Off,
            None => self.state,
        };
        let error = self.error;
        vm.with_cx_mut(|cx| {
            if initial != CheckState::Off {
                self.set_state(cx, initial, Animate::No);
            }
            if error {
                self.animator_toggle(cx, true, Animate::No, ids!(error.on), ids!(error.off));
            }
        });
    }
}

#[derive(Clone, Debug, Default)]
pub enum CheckBoxAction {
    Change(bool),
    #[default]
    None,
}

impl CheckBox {
    pub fn draw_check_box(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        // The shader places the mark box from this flag; push it before the
        // instance is emitted so a label-first box draws its mark at the end.
        self.draw_bg.set_uniform(
            cx,
            live_id!(mark_at_end),
            &[if self.label_before { 1.0 } else { 0.0 }],
        );
        // The toggle's knob follows the drag through two uniforms the
        // animator never touches; a plain checkbox has neither and the
        // writes fall through.
        let (drag, drag_pos) = match self.drag {
            Some(drag) if drag.moved => (1.0, drag.pos),
            _ => (0.0, 0.0),
        };
        self.draw_bg.set_uniform(cx, live_id!(drag), &[drag]);
        self.draw_bg.set_uniform(cx, live_id!(drag_pos), &[drag_pos]);
        self.draw_bg.begin(cx, walk, self.layout);

        let on = self.animator_in_state(cx, ids!(active.on));
        // Asked before the label is borrowed, because the answer decides
        // which of three icons the row draws and all three are fields of
        // the same struct the label is read out of.
        let knob_carries_the_pair = self.state_icons_ride_the_knob(cx);
        let icon_walk = self.icon_walk;
        let text: &str = if on && !self.text_on.is_empty() {
            &self.text_on
        } else if !on && !self.text_off.is_empty() {
            &self.text_off
        } else {
            self.text.as_ref()
        };
        // The label's outer margin clears the mark box; mirrored, it
        // clears a box at the end of the row instead.
        let margin = self.label_walk.margin;
        let label_walk = if self.label_before {
            Walk {
                margin: Inset {
                    left: margin.right,
                    right: margin.left,
                    ..margin
                },
                ..self.label_walk
            }
        } else {
            self.label_walk
        };
        // An empty label is no label: walked all the same it still takes
        // its own line height and the margin that clears the mark box, and
        // on a face that is nothing but an icon that phantom is what pushes
        // the mark off the middle.
        let worded = !text.is_empty();
        if worded && self.label_before {
            self.draw_text
                .draw_walk(cx, label_walk, self.label_align, text);
        }
        // The mark in the row: the face for the state it is in when the
        // pair is the row's own, and the single icon otherwise.
        if knob_carries_the_pair {
            self.draw_icon.draw_walk(cx, icon_walk);
        } else if on {
            self.draw_icon_on.draw_walk(cx, icon_walk);
        } else {
            self.draw_icon_off.draw_walk(cx, icon_walk);
        }
        if worded && !self.label_before {
            self.draw_text
                .draw_walk(cx, label_walk, self.label_align, text);
        }
        self.draw_bg.end(cx);
        self.draw_knob_icons(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    /// Whether the state pair belongs on a knob rather than in the row:
    /// either there is no pair at all, in which case the row draws the
    /// single `draw_icon` as it always has, or the mark is a pill and the
    /// knob is where a pair rides -- drawn over the pill, after the row is
    /// closed, so it must not also stand in the row.
    ///
    /// `pill_aspect` is the toggle's own uniform and reads back zero
    /// through a shader that never declared one, which is what tells a
    /// pill from a mark box without either of them saying so.
    fn state_icons_ride_the_knob(&mut self, cx: &mut Cx) -> bool {
        if self.draw_icon_on.svg.is_none() && self.draw_icon_off.svg.is_none() {
            return true;
        }
        let mut aspect = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(pill_aspect), &mut aspect);
        aspect[0] > 0.0
    }

    /// The toggle's knob icons, drawn over the pill after it: the on icon
    /// past the middle of the travel, the off icon before it. The knob's
    /// place is recomputed from the same uniforms the shader reads, plus the
    /// active mix as the animator has it now, so the icon rides the knob
    /// through its slide.
    fn draw_knob_icons(&mut self, cx: &mut Cx2d) {
        if self.draw_icon_on.svg.is_none() && self.draw_icon_off.svg.is_none() {
            return;
        }
        let Some((center, diameter, knob_t)) = self.knob_geometry(cx) else {
            return;
        };
        let side = diameter * self.knob_icon_size;
        let rect = Rect {
            pos: center - dvec2(side * 0.5, side * 0.5),
            size: dvec2(side, side),
        };
        if knob_t > 0.5 {
            self.draw_icon_on.draw_abs(cx, rect);
        } else {
            self.draw_icon_off.draw_abs(cx, rect);
        }
    }

    /// Where the toggle's knob is: its centre in window points, its
    /// diameter, and its position along the track. `None` for a checkbox
    /// whose shader has no pill.
    fn knob_geometry(&mut self, cx: &mut Cx) -> Option<(DVec2, f64, f32)> {
        let mut aspect = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(pill_aspect), &mut aspect);
        let mut size = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(size), &mut size);
        if aspect[0] <= 0.0 || size[0] <= 0.0 {
            return None;
        }
        let mut border = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(border_size), &mut border);
        let mut inset = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(knob_inset), &mut inset);
        let mut active = [0.0f32];
        self.draw_bg.get_instance(cx, live_id!(active), &mut active);
        let knob_t = match self.drag {
            Some(drag) if drag.moved => drag.pos,
            _ => active[0],
        };
        let rect = self.draw_bg.area().rect(cx);
        let (size, border, inset) = (size[0] as f64, border[0] as f64, inset[0] as f64);
        let pill = dvec2(size * aspect[0] as f64, size);
        let radius = pill.y * 0.5 - border - inset;
        let along = if self.knob_centred(cx) {
            pill.y * 0.5 + (pill.x - pill.y) * knob_t as f64
        } else {
            let travel = pill.y - pill.x + border + inset;
            pill.y * 0.5 + border - travel * knob_t as f64
        };
        let offset_x = if self.label_before { rect.size.x - pill.x } else { 0.0 };
        let center = dvec2(rect.pos.x + offset_x + along, rect.pos.y + rect.size.y * 0.5);
        Some((center, radius * 2.0, knob_t))
    }

    /// How far the knob travels between off and on, in layout points.
    fn knob_travel(&mut self, cx: &mut Cx) -> f64 {
        let mut aspect = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(pill_aspect), &mut aspect);
        let mut size = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(size), &mut size);
        let mut border = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(border_size), &mut border);
        let mut inset = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(knob_inset), &mut inset);
        let size = size[0] as f64;
        if self.knob_centred(cx) {
            return (size * aspect[0] as f64 - size).max(1.0);
        }
        (size * aspect[0] as f64 - size - border[0] as f64 - inset[0] as f64).max(1.0)
    }

    /// Whether the face centres its knob in the slot's height at both ends
    /// (`knob_centred`, a uniform that reads back zero through a face that
    /// never declared it), so its travel is the slot's length less its
    /// height rather than the round toggle's.
    fn knob_centred(&mut self, cx: &mut Cx) -> bool {
        let mut centred = [0.0f32];
        self.draw_bg.get_uniform(cx, live_id!(knob_centred), &mut centred);
        centred[0] > 0.5
    }

    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let CheckBoxAction::Change(b) = item.cast() {
                return Some(b);
            }
        }
        None
    }

    /// True only while the box is `On`; a mixed box is not active.
    pub fn active(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }

    /// Sets the active state.
    ///
    /// Pass `Animate::No` for programmatic state restoration (e.g. after an
    /// `Event::ScriptReapply` reloads the widget tree) — `Animate::Yes`
    /// routes through `animator_play`, which early-outs when the animator's
    /// cached `current_state` already matches the target, leaving the
    /// shader uniform stale. `Animate::No` uses `animator_cut`, which
    /// always re-merges the state's values and re-applies them.
    pub fn set_active(&mut self, cx: &mut Cx, value: bool, animate: Animate) {
        self.set_state(cx, if value { CheckState::On } else { CheckState::Off }, animate);
    }

    /// The state the box is in, read from the animator.
    pub fn state(&self, cx: &Cx) -> CheckState {
        if self.animator_in_state(cx, ids!(mixed.on)) {
            CheckState::Mixed
        } else if self.animator_in_state(cx, ids!(active.on)) {
            CheckState::On
        } else {
            CheckState::Off
        }
    }

    /// Sets the state. `Mixed` shows the dash and reports as not active;
    /// see [`CheckBox::set_active()`] for what `animate` means.
    pub fn set_state(&mut self, cx: &mut Cx, state: CheckState, animate: Animate) {
        self.state = state;
        self.animator_toggle(
            cx,
            state == CheckState::On,
            animate,
            ids!(active.on),
            ids!(active.off),
        );
        self.animator_toggle(
            cx,
            state == CheckState::Mixed,
            animate,
            ids!(mixed.on),
            ids!(mixed.off),
        );
    }

    /// Whether the box shows the error intent.
    pub fn error(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(error.on))
    }

    /// Turns the error intent on or off.
    pub fn set_error(&mut self, cx: &mut Cx, error: bool) {
        self.error = error;
        self.animator_toggle(cx, error, Animate::Yes, ids!(error.on), ids!(error.off));
    }

    pub fn debug_dump_animator(&self, heap: &ScriptHeap) -> String {
        self.animator.debug_dump(heap)
    }
}

impl Widget for CheckBox {
    /// What this would be worth on a row, with `over` in force: the mark box,
    /// the label beside it, and the gap the label's own margin keeps between
    /// them.
    fn measure_width(
        &mut self,
        cx: &mut Cx2d,
        over: Option<&crate::width_override::WidthOverride>,
    ) -> Option<f64> {
        if over.is_some_and(|o| o.opaque) {
            return None;
        }
        if !over.and_then(|o| o.visible).unwrap_or(self.visible) {
            return Some(0.0);
        }

        let margin = over.and_then(|o| o.margin).unwrap_or(self.walk.margin);
        let width = over.and_then(|o| o.width).unwrap_or(self.walk.width);
        if let Size::Fixed(w) = width {
            return Some(w + margin.width());
        }
        if let Size::Fill { min, .. } = width {
            return Some(min.unwrap_or(0.0) + margin.width());
        }

        let pad = over.and_then(|o| o.padding).unwrap_or(self.layout.padding);
        let gap = over.and_then(|o| o.spacing).unwrap_or(self.layout.spacing);

        // The face it is wearing is the one it would draw: a toggle with an
        // `on` word says that word while it is on.
        let on = self.animator_in_state(cx.cx, ids!(active.on));
        let label: &str = match over.and_then(|o| o.text.as_deref()) {
            Some(t) => t,
            None if on && !self.text_on.is_empty() => self.text_on.as_str(),
            None if !on && !self.text_off.is_empty() => self.text_off.as_str(),
            None => self.text.as_ref(),
        };
        let text_w = if label.is_empty() {
            0.0
        } else {
            crate::badge::advance(&self.draw_text, cx, label) + self.label_walk.margin.width()
        };

        let mark = crate::badge::icon_extent(&mut self.draw_icon, cx, self.icon_walk)?;
        let parts = if text_w > 0.0 { 2 } else { 1 };
        Some(pad.width() + text_w + mark + gap * (parts as f64 - 1.0) + margin.width())
    }

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

    fn snapshot_checked(&self, cx: &Cx) -> Option<bool> {
        Some(self.active(cx))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(checked) {
            let is_active = vm.with_cx(|cx| self.animator_in_state(cx, ids!(active.on)));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(is_active));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.set_key_focus(cx);
                // A mixed box resolves to On: it is not active, so the
                // flip below lands there; only the dash needs clearing.
                if self.animator_in_state(cx, ids!(mixed.on)) {
                    self.animator_play(cx, ids!(mixed.off));
                }
                let new_active = if self.animator_in_state(cx, ids!(active.on)) {
                    self.animator_play(cx, ids!(active.off));
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        CheckBoxAction::Change(false),
                    );
                    false
                } else {
                    self.animator_play(cx, ids!(active.on));
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        CheckBoxAction::Change(true),
                    );
                    true
                };
                self.state = if new_active { CheckState::On } else { CheckState::Off };
                cx.widget_to_script_call(
                    uid,
                    NIL,
                    self.source.clone(),
                    self.on_click.clone(),
                    &[ScriptValue::from_bool(new_active)],
                );
                self.animator_play(cx, ids!(press.on));
                if self.draggable {
                    self.drag = Some(KnobDrag {
                        start_x: fe.abs.x,
                        start_pos: 0.0,
                        pos: 0.0,
                        moved: false,
                    });
                }
            }
            // A focused box flips on Space (and Return) the way a click does,
            // as a focused Button and RadioButton answer those keys. An app
            // with its own use for Space (a player's play/pause) takes it
            // before the focused control does.
            Hit::KeyDown(ke)
                if !ke.is_repeat && matches!(ke.key_code, KeyCode::Space | KeyCode::ReturnKey | KeyCode::NumpadEnter) =>
            {
                if self.animator_in_state(cx, ids!(mixed.on)) {
                    self.animator_play(cx, ids!(mixed.off));
                }
                let on = !self.animator_in_state(cx, ids!(active.on));
                self.animator_play(cx, if on { ids!(active.on) } else { ids!(active.off) });
                self.state = if on { CheckState::On } else { CheckState::Off };
                cx.widget_action_with_data(&self.action_data, uid, CheckBoxAction::Change(on));
                cx.widget_to_script_call(
                    uid,
                    NIL,
                    self.source.clone(),
                    self.on_click.clone(),
                    &[ScriptValue::from_bool(on)],
                );
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.animator_play(cx, ids!(press.off));
                if let Some(drag) = self.drag.take() {
                    // A cancelled press leaves the switch where the press put it.
                    if drag.moved && !fe.cancelled {
                        // The side the knob was released on wins, which may
                        // undo the flip the press made.
                        let on = drag.pos > 0.5;
                        if on != self.animator_in_state(cx, ids!(active.on)) {
                            self.animator_play(cx, if on { ids!(active.on) } else { ids!(active.off) });
                            self.state = if on { CheckState::On } else { CheckState::Off };
                            cx.widget_action_with_data(
                                &self.action_data,
                                uid,
                                CheckBoxAction::Change(on),
                            );
                            cx.widget_to_script_call(
                                uid,
                                NIL,
                                self.source.clone(),
                                self.on_click.clone(),
                                &[ScriptValue::from_bool(on)],
                            );
                        }
                    }
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(mut drag) = self.drag {
                    if !drag.moved {
                        if (fe.abs.x - drag.start_x).abs() < KNOB_DRAG_SLOP {
                            return;
                        }
                        // The knob picks up from where the slide has taken it
                        // so far, anchored to the pointer from here on.
                        let mut active = [0.0f32];
                        self.draw_bg.get_instance(cx, live_id!(active), &mut active);
                        drag.moved = true;
                        drag.start_pos = active[0];
                        drag.start_x = fe.abs.x;
                    }
                    let travel = self.knob_travel(cx);
                    let along = drag.start_pos as f64 + (fe.abs.x - drag.start_x) / travel;
                    drag.pos = along.clamp(0.0, 1.0) as f32;
                    self.drag = Some(drag);
                    self.draw_bg.redraw(cx);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_check_box(cx, walk)
    }

    /// The label as shown: the on or off text when one is set for the
    /// current state, else `text`.
    fn text(&self) -> String {
        match self.state {
            CheckState::On if !self.text_on.is_empty() => self.text_on.clone(),
            CheckState::Off if !self.text_off.is_empty() => self.text_off.clone(),
            _ => self.text.as_ref().to_string(),
        }
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text.as_ref() == v { return }
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl CheckBoxRef {
    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        self.borrow().and_then(|inner| inner.changed(actions))
    }

    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }

    pub fn active(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.active(cx)
        } else {
            false
        }
    }

    /// See [`CheckBox::set_active()`].
    pub fn set_active(&self, cx: &mut Cx, value: bool, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, value, animate);
        }
    }

    /// See [`CheckBox::state()`].
    pub fn state(&self, cx: &Cx) -> CheckState {
        self.borrow().map_or(CheckState::Off, |inner| inner.state(cx))
    }

    /// See [`CheckBox::set_state()`].
    pub fn set_state(&self, cx: &mut Cx, state: CheckState, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_state(cx, state, animate);
        }
    }

    /// See [`CheckBox::error()`].
    pub fn error(&self, cx: &Cx) -> bool {
        self.borrow().is_some_and(|inner| inner.error(cx))
    }

    /// See [`CheckBox::set_error()`].
    pub fn set_error(&self, cx: &mut Cx, error: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_error(cx, error);
        }
    }

    pub fn debug_dump_animator(&self, heap: &ScriptHeap) -> String {
        if let Some(inner) = self.borrow() {
            inner.debug_dump_animator(heap)
        } else {
            "no borrow".to_string()
        }
    }
}

#[cfg(test)]
mod icon_face_tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    /// One icon-only checkbox wearing the two padlocks, built from the
    /// library's own declaration so that a retuning there is a retuning
    /// here. `side` is the side the mark is asked to take.
    fn a_lock(cx: &mut Cx, side: f64) -> WidgetRef {
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets_internal.*
                use mod.widgets.*
                CheckBoxIcon{
                    icon_walk: Walk{width: #(side), height: #(side)}
                    draw_icon_off +: {
                        svg: crate_resource("makepad_widgets:resources/icons/icon_lock_open.svg")
                    }
                    draw_icon_on +: {
                        svg: crate_resource("makepad_widgets:resources/icons/icon_lock_shut.svg")
                    }
                }
            });
            assert!(vm.take_errors().is_empty(), "the icon-only checkbox did not build");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// One layout pass over the widget, which is what gives every part of
    /// it a rectangle. A control that was never drawn has none, and a press
    /// at its middle lands nowhere.
    fn drawn(cx: &mut Cx, widget: &WidgetRef) {
        let size = Vec2d { x: 120.0, y: 60.0 };
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        let mut draw_list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        widget.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    /// A press taken the whole way a hand takes it, down and up on the
    /// control, and everything that came out of it.
    fn a_press_on(cx: &mut Cx, widget: &WidgetRef) -> Vec<Action> {
        const WINDOW: WindowId = WindowId(1, 1);
        let face = widget.area().rect(cx);
        assert!(face.size.x > 0.0, "the control was never drawn, so the press lands nowhere");
        let at = face.pos + face.size * 0.5;
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = Event::MouseDown(MouseDownEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        });
        // Both halves are captured. A checkbox turns over on the press, not
        // on the release, so a test that watched only the release would
        // find the box flipped and nothing said about it.
        let mut actions = cx.capture_actions(|cx| {
            widget.handle_event(cx, &down, &mut Scope::empty());
        });
        let up = Event::MouseUp(MouseUpEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 1.1,
        });
        actions.extend(cx.capture_actions(|cx| {
            widget.handle_event(cx, &up, &mut Scope::empty());
        }));
        cx.fingers.first_mouse_button = None;
        actions
    }

    /// How many changes the widget reported in these actions, and the last
    /// of them. Two changes off one press is as wrong as none: whoever
    /// reads a lock acts on every change it reports.
    fn changes(actions: &[Action], uid: WidgetUid) -> (usize, Option<bool>) {
        let mut count = 0;
        let mut last = None;
        for action in actions.iter() {
            let Some(action) = action.downcast_ref::<WidgetAction>() else {
                continue;
            };
            if action.widget_uid != uid {
                continue;
            }
            if let CheckBoxAction::Change(value) = action.cast() {
                count += 1;
                last = Some(value);
            }
        }
        (count, last)
    }

    /// A press turns the face over and says so once, and the next press
    /// turns it back. Seen failing with the state pair drawn only on a
    /// knob: an icon-only face has none, so it showed nothing at all and
    /// there was no mark to press.
    #[test]
    fn a_press_turns_the_face_over_and_reports_it_once() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let widget = a_lock(&mut cx, 14.0);
        let uid = widget.widget_uid();
        drawn(&mut cx, &widget);
        assert!(!widget.as_check_box().active(&cx), "it opened already on");

        let actions = a_press_on(&mut cx, &widget);
        assert_eq!(changes(&actions, uid), (1, Some(true)), "the first press");
        assert!(widget.as_check_box().active(&cx), "the press did not turn it over");

        drawn(&mut cx, &widget);
        let actions = a_press_on(&mut cx, &widget);
        assert_eq!(changes(&actions, uid), (1, Some(false)), "the second press");
        assert!(!widget.as_check_box().active(&cx), "the second press did not turn it back");
    }

    /// The mark sits in the middle of the face and nothing else is in the
    /// row with it, at every size the catalogue shows one at.
    ///
    /// Seen failing on the custom face this is built from, whose label walk
    /// carries the margin that clears a mark box: the word was empty and
    /// the margin was not, so the face came out wider than it was tall and
    /// the padlock stood left of its centre by half of that.
    #[test]
    fn the_face_draws_its_mark_in_the_middle_with_no_word_beside_it() {
        for side in [12.0, 14.0, 20.0, 28.0] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let widget = a_lock(&mut cx, side);
            drawn(&mut cx, &widget);
            let face = widget.area().rect(&cx);
            assert!(face.size.x > 0.0 && face.size.y > 0.0, "the face at {side} drew nothing");
            assert!(
                (face.size.x - face.size.y).abs() < 0.5,
                "the face at {side} is {} by {}, so something stands beside the mark",
                face.size.x,
                face.size.y
            );
            let check = widget.borrow::<CheckBox>().expect("a CheckBox");
            assert_eq!(check.text.as_ref(), "", "the icon-only face carries a word");
            let icon = check.draw_icon_off.area().rect(&cx);
            assert!(icon.size.x > 0.0 && icon.size.y > 0.0, "the mark at {side} drew nothing");
            assert!((icon.size.x - side).abs() < 0.5, "the mark at {side} is {} wide", icon.size.x);
            let (mark, middle) = (icon.pos + icon.size * 0.5, face.pos + face.size * 0.5);
            assert!(
                (mark.x - middle.x).abs() <= 0.5 && (mark.y - middle.y).abs() <= 0.5,
                "the mark at {side} sits at {mark:?}, off the face's centre {middle:?}"
            );
        }
    }

    /// A dead one is drawn dead, and its two inks come back untouched. The
    /// mark is the whole of this face, so being disabled has to reach the
    /// mark: the box and the label the other faces dim are not there to do
    /// it. What falls is the mark's opacity, so a caller's own colours
    /// survive the round trip.
    ///
    /// Seen failing before the face carried a disabled track of its own,
    /// where a lock told to go dim was drawn exactly as it had been.
    #[test]
    fn a_disabled_face_goes_dim_without_losing_its_inks() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let widget = a_lock(&mut cx, 14.0);
        // A caller's own pair, to be found again on the way back.
        let mine = Vec4f { x: 1.0, y: 0.25, z: 0.0, w: 1.0 };
        let yours = Vec4f { x: 0.0, y: 0.5, z: 1.0, w: 1.0 };
        {
            let mut lock = widget.borrow_mut::<CheckBox>().expect("a CheckBox");
            lock.draw_icon_off.color = mine;
            lock.draw_icon_on.color = yours;
        }
        drawn(&mut cx, &widget);
        let live = {
            let lock = widget.borrow::<CheckBox>().expect("a CheckBox");
            (lock.draw_icon_off.opacity, lock.draw_icon_on.opacity)
        };
        // Cut to each state rather than played to it: nothing here pumps
        // the frames a 0.2 second fade would need, and the question is what
        // the state HOLDS, not how long it takes to get there.
        let cut = |cx: &mut Cx, disabled: bool| {
            widget
                .borrow_mut::<CheckBox>()
                .expect("a CheckBox")
                .animator_toggle(cx, disabled, Animate::No, ids!(disabled.on), ids!(disabled.off));
        };
        cut(&mut cx, true);
        drawn(&mut cx, &widget);
        let dead = {
            let lock = widget.borrow::<CheckBox>().expect("a CheckBox");
            assert_eq!(lock.draw_icon_off.color, mine, "going dim repainted the open mark");
            assert_eq!(lock.draw_icon_on.color, yours, "going dim repainted the shut mark");
            (lock.draw_icon_off.opacity, lock.draw_icon_on.opacity)
        };
        assert!(dead.0 < live.0, "the open mark is drawn the same dead as alive");
        assert!(dead.1 < live.1, "the shut mark is drawn the same dead as alive");

        cut(&mut cx, false);
        drawn(&mut cx, &widget);
        let lock = widget.borrow::<CheckBox>().expect("a CheckBox");
        assert_eq!((lock.draw_icon_off.opacity, lock.draw_icon_on.opacity), live, "it came back dim");
        assert_eq!(lock.draw_icon_off.color, mine, "coming back took the open mark's colour");
        assert_eq!(lock.draw_icon_on.color, yours, "coming back took the shut mark's colour");
    }

    /// The two states are not drawn in one ink, in either appearance the
    /// library ships. A mark this size moves two or three pixels between
    /// the shapes, so a pair handed one colour says nothing whatever about
    /// which state it is in.
    ///
    /// Seen failing on the first pair chosen for it, the label ink and the
    /// checked mark's: the dark sheet resolves both of those to the same
    /// token, and the two padlocks came out identical.
    #[test]
    fn the_two_states_are_not_drawn_in_the_same_ink() {
        for base in [crate::BaseTheme::Dark, crate::BaseTheme::Light] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            crate::set_base_theme(&mut cx, base);
            cx.with_vm(|vm| vm.with_reload(crate::script_mod));
            let widget = a_lock(&mut cx, 14.0);
            let check = widget.borrow::<CheckBox>().expect("a CheckBox");
            let (off, on) = (check.draw_icon_off.color, check.draw_icon_on.color);
            assert_ne!(off, on, "{base:?}: the open mark and the shut one are one colour");
        }
    }
}
