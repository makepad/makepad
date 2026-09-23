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

                        let color_fill = self.color
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

                        sdf.fill_keep(color_fill)
                        sdf.stroke(color_stroke, self.border_size)

                        // Draw checkmark
                        let mark_padding = /** check inset frac 0.1..0.45 step 0.005 */ 0.275 * self.size
                        sdf.move_to(offset_px.x + mark_padding, center_px.y)
                        sdf.line_to(offset_px.x + center_px.x, center_px.y + sz_px * 0.5 - mark_padding)
                        sdf.line_to(offset_px.x + sz_px - mark_padding, offset_px.y + mark_padding)

                        let mark_color = self.mark_color
                            .mix(self.mark_color_hover, self.hover)
                            .mix(self.mark_color_active, self.active)
                            .mix(self.mark_color_error, self.error * self.active)
                            .mix(self.mark_color_disabled, self.disabled)

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

                let color_fill = self.color
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

                // Draw toggle mark. While dragging the knob follows drag_pos
                // instead of the active mix, and it grows while pressed.
                let knob_t = mix(self.active, self.drag_pos, self.drag)
                let mark_size = (sz_px.y * 0.5 - self.border_size - self.knob_inset) * (1.0 + self.pressed * self.knob_grow)
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + self.knob_inset
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * knob_t

                // Draw ring when off, filled circle when on
                sdf.circle(offset_px.x + mark_pos_y, center_px.y, mark_size)
                sdf.circle(offset_px.x + mark_pos_y, center_px.y, mark_size * /** knob ring hole frac 0.1..0.9 step 0.05 */ 0.45)
                sdf.subtract()

                sdf.circle(offset_px.x + mark_pos_y, center_px.y, mark_size)
                sdf.blend(self.active)

                let mark_color = self.mark_color
                    .mix(self.mark_color_hover, self.hover)
                    .mix(self.mark_color_active, self.active)
                    .mix(self.mark_color_error, self.error)
                    .mix(self.mark_color_disabled, self.disabled)

                sdf.fill(mark_color)
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

    /// The icon drawn on the toggle's knob while it is on; leave the svg
    /// unset for none.
    #[live]
    pub draw_icon_on: DrawSvg,
    /// The icon drawn on the toggle's knob while it is off.
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
        let text: &str = if on && !self.text_on.is_empty() {
            &self.text_on
        } else if !on && !self.text_off.is_empty() {
            &self.text_off
        } else {
            self.text.as_ref()
        };
        if self.label_before {
            // The label's outer margin clears the mark box; mirrored, it
            // clears a box at the end of the row instead.
            let margin = self.label_walk.margin;
            let walk = Walk {
                margin: Inset {
                    left: margin.right,
                    right: margin.left,
                    ..margin
                },
                ..self.label_walk
            };
            self.draw_text
                .draw_walk(cx, walk, self.label_align, text);
            self.draw_icon.draw_walk(cx, self.icon_walk);
        } else {
            self.draw_icon.draw_walk(cx, self.icon_walk);

            self.draw_text
                .draw_walk(cx, self.label_walk, self.label_align, text);
        }
        self.draw_bg.end(cx);
        self.draw_knob_icons(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
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
        let travel = pill.y - pill.x + border + inset;
        let along = pill.y * 0.5 + border - travel * knob_t as f64;
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
        (size * aspect[0] as f64 - size - border[0] as f64 - inset[0] as f64).max(1.0)
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
