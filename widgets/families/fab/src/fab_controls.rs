//! The "fab" control set — the fab app's control styling
//! (libs/fab/src/ui: the drag-numeric field, the color picker, the row /
//! panel / search visual language) ported into the widget library as a
//! named, reusable set. Nothing here depends on libs/fab: the code and the
//! token table are carried over and adapted; the fab app itself migrates to
//! these later.
//!
//! Registered names:
//! * `mod.fab` — the token table (surfaces, text, accents, density, type,
//!   motion), same token names the fab app uses.
//! * `mod.widgets.FabValueInput` — the drag-numeric field. Press arms, 3 px
//!   engages a drag (one step per pixel, Shift fine, Ctrl snaps, clamping
//!   shifts the anchor), a plain click opens text entry, the end zones step.
//!   `enabled: false` dims it and makes it inert. `track: true` turns the
//!   row into a slider instead: the press lands the value under the pointer
//!   and the drag keeps it there, the name resets, the number types.
//! * `mod.widgets.FabSlider` — a horizontal track with the name on its
//!   left and the number on its right. The press lands the thumb where the
//!   pointer is and the drag keeps it there; a click on the name takes the
//!   row back to zero.
//! * `mod.widgets.FabKnob` — the slider's number on a dial, for a cell of a
//!   matrix: 44 by 64 by default and sized by whatever box it is given, down
//!   to 28 across. Press and pull up for more (the whole range in 150 points,
//!   Shift a tenth of the speed), the wheel and the arrows step it once it
//!   has the keyboard, a double click takes it back to zero. Same fields,
//!   same actions and same ref helpers as the slider.
//! * `mod.widgets.FabDiagonalLabel` — the name over a column too narrow to
//!   hold it: a matrix header, turned 45 degrees and let out over its
//!   neighbours. `lean` picks which side it hangs over. It draws OUTSIDE its
//!   own box on purpose, so the row it stands in wants `clip_x: false`.
//! * `mod.widgets.FabColorWheel` — hue ring around a saturation/value
//!   square, pointer-captured drags, arrow-key nudges.
//! * `mod.widgets.FabColorPick` — a swatch that opens a self-managed
//!   popover (wheel + R G B A and H S V track rows + hex entry) anchored at
//!   the swatch; one colour seen four ways, each following the others;
//!   outside-click commits, Escape reverts. Publishes `Changed` live and
//!   `Ended` on commit, plus `Opened`/`Closed` for hosts that need to know.
//! * `mod.widgets.FabPaletteCarousel` — every palette on offer in one row
//!   that scrolls sideways, a chip of four stacked colours each; drag, wheel
//!   or arrows along it, press and let go on a chip to pick it. It glides:
//!   an arrow, a wheel's momentum and a flick off a drag all close the
//!   distance over a tenth of a second or so rather than jumping it (see
//!   [`ChipGlide`]).
//! * `mod.widgets.FabLabel` / `FabLabelDim` / `FabLabelSmall` /
//!   `FabHeaderLabel`, `mod.widgets.FabSearch` (input well),
//!   `mod.widgets.FabPropRow` (label-left / value-right row),
//!   `mod.widgets.FabSection` (clickable section header) — the DSL shapes
//!   panels are assembled from (the tweaker's sidebar is the first tenant).

use crate::button::ButtonAction;
// `diagonal_run` is named only by the tests down the file, which check this
// control's arithmetic where it now lives — and the attribute rather than a
// test-only import because one of those tests reads this file down to where
// the tests begin, and a second such marker up here would cut it short.
#[allow(unused_imports)]
use crate::diagonal_text::{
    diagonal_row_height, diagonal_run, draw_diagonal_name, DiagonalLean, DiagonalRun,
};
use crate::widget_tree::CxWidgetExt;
use crate::{
    animator::*, makepad_derive_widget::*, makepad_draw::ime::TextInputConfig,
    makepad_draw::*, text_input::*, view::View, widget::*,
};
use crate::makepad_script::script;
use crate::scroll_motion::{
    estimate_release_velocity, push_sample, FrameClock, ScrollSample,
    FLING_DECEL_RATE_PER_MS, FLING_MIN_TOTAL_DELTA, FLING_SAMPLE_MAX_AGE,
};

pub fn script_mod(vm: &mut ScriptVm) {
    // Phase 1: the token table and a prelude carrying the `fab` alias, so
    // the ported DSL below reads exactly like it does in the fab app. The
    // table reads the theme as it stands without a sheet
    // (`desktop_style::keep_stock`): the kit's face is built on the app's
    // font family, and a sheet's own family must not come through it.
    let block = script! {
        use mod.prelude.stock_internal.*

        mod.fab = {
            // ---- surfaces (fab default-dark grade) ----
            color_area: #x303030
            color_editor: #x232323
            color_editor_alt: #x282828
            color_header: #x3d3d3d
            color_panel: #x3d3d3d
            color_panel_sub: #x353535
            color_popover: #x1a1a1a
            color_popover_border: #x545454
            color_border: #x161616
            color_border_light: #x4a4a4a
            color_row_hover: #x3a3a3a
            color_input: #x1d1d1d
            color_input_hover: #x232323
            color_input_active: #x161616
            color_button: #x545454
            color_button_hover: #x656565
            color_button_down: #x4a4a4a
            color_button_active: #x5680c2

            // ---- text ----
            color_text: #xe6e6e6
            color_text_dim: #x9a9a9a
            color_text_muted: #x707070
            color_text_active: #xffffff
            color_text_header: #xd0d0d0
            color_text_on_accent: #xffffff

            // ---- accents ----
            color_accent: #x5680c2
            color_accent_hover: #x6b93d4
            color_accent_dim: #x3c5a8a
            color_selection_bg: #x334d80
            color_focus_ring: #x7aa2e8
            color_warning: #xe0a020
            color_error: #xe04040
            color_ok: #x5cb85c

            // ---- the drag-numeric field's inset well ----
            color_num: #x1d1d1d
            color_num_hover: #x2a2a2a
            color_num_fill: #x3c5a8a
            color_num_arrow: #xb0b0b0

            // ---- density ----
            row_height: 24.0
            row_height_sm: 20.0
            header_height: 26.0
            prop_label_width: 92.0
            pad_1: 4.0
            pad_2: 6.0
            pad_3: 10.0
            // Sdf2d.box arguments — the drawn corner reads as twice these.
            radius: 2.0
            radius_lg: 3.0
            border: 1.0
            swatch_width: 46.0

            // ---- type ----
            // The kit's own face, here for the reason the palette above is
            // here. A sheet MOVES `theme.font_regular` -- `android` takes
            // Roboto, `ios` Inter -- and installing a blend takes the sheet
            // off and puts it back on, so a kit whose words came from the
            // theme changed typeface on the applied frame and changed back
            // on leave. The row heights are fab and held; the text metrics
            // did not, and every label and field on the panel reflowed under
            // the hand of whoever was dragging a weight. The Theme tab's
            // picker already carries a face of its own against exactly this
            // (`PanelFont`, tweaker.rs); the kit carries the same one, so
            // the panel and the controls in it read as one surface.
            //
            // The LATIN member only. What follows it in the family -- the
            // symbol face, and the CJK and emoji members the policy keeps
            // lazy -- is whatever was resolved for the app, because the
            // search well and the hex field take TYPED text: a panel that
            // reflows is a nuisance, a panel that cannot spell what was
            // typed into it is a dead end.
            font: theme.font_regular{
                font_family: theme.font_regular.font_family{
                    latin := FontMember{
                        res: crate_resource("makepad_widgets:resources/IBMPlexSans-Text.ttf")
                        asc: -0.1
                        desc: 0.0
                    }
                }
                line_spacing: 1.2
            }

            // ---- type sizes (points) ----
            font_size_ui: 8.5
            font_size_small: 7.5
            font_size_header: 9.0

            // ---- motion ----
            anim_fast: 0.10
            anim_normal: 0.15
        }
        true
    };
    vm.eval(block);

    // Phase 1b: the prelude carrying the `fab` alias, built from the table
    // above once it is final. The alias holds the table it was built from, so
    // re-pointing `mod.fab` after this would leave every control reading the
    // palette that is no longer there.
    let block = script! {
        use mod.prelude.widgets_internal.*

        mod.prelude.fab_internal = {
            ..mod.prelude.widgets_internal,
            fab: mod.fab
        }
    };
    vm.eval(block);

    // Phase 2: the controls, in the fab visual language. Every stock
    // template they nest, and the theme, are the library's as it stands
    // without a sheet (`desktop_style::keep_stock`): a sheet's tokens and its
    // writes onto `mod.widgets` reach none of them
    // (`nothing_a_sheet_sets_reaches_a_fab_control`). The prelude comes
    // after the templates: the kit registers halfway through the library,
    // and a name several enums spread into `mod.widgets` (`Right`, `Linear`)
    // is bound by whichever registered last, which is not the same half way
    // through a run and after it; the prelude's own are the ones meant here.
    let block = script! {
        use mod.prelude.fab_internal.*
        use mod.stock_widgets.*
        use mod.prelude.stock_internal.*

        set_type_default() do #(DrawDragNum::script_shader(vm)){
            ..mod.draw.DrawQuad

            // These are `#[live]` fields on DrawDragNum, so they are already
            // instances; `instance(..)` here would hand the f32 an object.
            hover: 0.0
            down: 0.0
            focus: 0.0
            disabled: 0.0
            fill: -1.0
            flat: 0.0
            stepper: 1.0
            fill_pad_l: 0.0
            fill_pad_r: 0.0
            num_box: 0.0
            num_box_r: 0.0

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, fab.radius)
                let reveal = mix(1.0, max(self.hover, max(self.down, self.focus)), self.flat)
                // Off: the well sinks most of the way into the row, so a
                // field that answers nothing does not look like one that will.
                let dim = 1.0 - 0.6 * self.disabled
                let mut base = fab.color_num.mix(fab.color_num_hover, self.hover).mix(fab.color_input_active, self.down)
                base = vec4(base.xyz, base.w * reveal * dim)
                sdf.fill_keep(base)
                let mut border = fab.color_border.mix(fab.color_focus_ring, self.focus)
                border = vec4(border.xyz, border.w * reveal * dim)
                sdf.stroke(border, 1.0)
                if self.fill >= 0.0 {
                    let x0 = 1.0 + self.fill_pad_l
                    let x1 = max(x0 + 2.0, w - 1.0 - self.fill_pad_r)
                    sdf.box(x0, 1.0, max(2.0, (x1 - x0) * self.fill), h - 2.0, fab.radius)
                    sdf.fill(vec4(fab.color_num_fill.xyz, 0.85))
                }
                // The readout's own well, at the end the number is drawn in.
                // It is the track's end too, so the fill above stops at its
                // edge and the two never overlap; what the eye reads is a
                // slider that ends in a field, which is exactly what a press
                // in either half does.
                if self.num_box > 2.0 {
                    let bx = w - self.num_box_r - self.num_box
                    sdf.box(bx, 1.5, self.num_box, h - 3.0, fab.radius)
                    sdf.fill_keep(vec4(fab.color_input_active.xyz, dim))
                    let edge = fab.color_border.mix(fab.color_focus_ring, self.focus)
                    sdf.stroke(vec4(edge.xyz, edge.w * dim), 1.0)
                }
                // Hover arrows in the end zones; they retire while the field
                // is a text editor (focus carries the editing state), while
                // it is off, and on a row that is a track — there the ends
                // are the ends of the track, not two little buttons.
                if self.hover * (1.0 - self.disabled) * self.stepper > 0.01 {
                    if self.focus < 0.5 {
                        let cy = h * 0.5
                        let a = vec4(fab.color_num_arrow.xyz, self.hover)
                        sdf.move_to(9.0, cy - 3.5)
                        sdf.line_to(5.5, cy)
                        sdf.line_to(9.0, cy + 3.5)
                        sdf.stroke(a, 1.25)
                        sdf.move_to(w - 9.0, cy - 3.5)
                        sdf.line_to(w - 5.5, cy)
                        sdf.line_to(w - 9.0, cy + 3.5)
                        sdf.stroke(a, 1.25)
                    }
                }
                return sdf.result
            }
        }

        mod.widgets.FabValueInputBase = #(FabValueInput::register_widget(vm))
        /** The drag-numeric field: press arms, 3 px of travel starts the
         * scrub, release without travel opens keyboard editing. */
        mod.widgets.FabValueInput = set_type_default() do mod.widgets.FabValueInputBase{
            width: Fill
            height: fab.row_height
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
            margin: Inset{top: 0 bottom: 0 left: 0 right: 0}

            label: ""
            min: 0.0
            max: 0.0
            /** scrub granularity per pixel of travel 0.001..1 step 0.001 */
            step: 0.01
            snap: 0.0
            precision: 2
            suffix: ""
            value: 0.0
            wrap: false
            show_fill: false
            quantize: false
            track: false

            draw_text +: {
                ink_centered: true
                color: fab.color_text_dim
                text_overflow: TextOverflow.Ellipsis
                text_style: fab.font{
                    font_size: fab.font_size_ui
                }
            }
            text_input: TextInput{
                width: Fill
                height: Fill
                // The same trap FabSearch's `input` documents: `android`
                // and `ios` set `mod.widgets.TextInput.min_height`, a walk
                // applies it whatever height was asked for, and the value
                // would be drawn below the 18px field it belongs to. Zero
                // is what the default theme resolves to, so nothing moves.
                min_height: 0
                // Read-only display may carry a unit suffix. Editing
                // switches this back to numeric-only in Rust.
                is_numeric_only: false
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
                label_align: Align{x: 1.0 y: 0.5}
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                    border_radius: 0.0
                }
                draw_text +: {
                    ink_centered: true
                    color: fab.color_text
                    text_style: fab.font{
                        font_size: fab.font_size_ui
                    }
                }
            }
            animator: Animator{
                hover: {
                    default: @off
                    off: AnimatorState{
                        from: {all: Forward {duration: fab.anim_fast}}
                        apply: { draw_bg: {hover: 0.0, down: 0.0} }
                    }
                    on: AnimatorState{
                        from: {all: Snap}
                        apply: { draw_bg: {hover: 1.0, down: 0.0} }
                    }
                    down: AnimatorState{
                        from: {all: Snap}
                        apply: { draw_bg: {hover: 1.0, down: 1.0} }
                    }
                }
                focus: {
                    default: @off
                    off: AnimatorState{
                        from: {all: Forward {duration: fab.anim_fast}}
                        apply: { draw_bg: {focus: 0.0} }
                    }
                    on: AnimatorState{
                        from: {all: Snap}
                        apply: { draw_bg: {focus: 1.0} }
                    }
                }
            }
        }

        set_type_default() do #(DrawFabSlider::script_shader(vm)){
            ..mod.draw.DrawQuad

            // `#[live]` fields on DrawFabSlider, so they are already
            // instances — see DrawDragNum above.
            hover: 0.0
            down: 0.0
            focus: 0.0
            disabled: 0.0
            travel: 0.0
            label_px: 0.0
            readout_px: 0.0
            thumb_px: 12.0
            inset_px: 2.0

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let h = self.rect_size.y
                // The track is whatever the name and the number leave of the
                // row. Rust hands over the same two column widths its hit
                // test measures from, so the thumb is drawn where it can be
                // taken hold of.
                let x0 = self.label_px
                let x1 = max(x0 + 2.0, self.rect_size.x - self.readout_px)
                let w = x1 - x0
                let dim = 1.0 - 0.6 * self.disabled
                let cy = h * 0.5
                let well = max(4.0, h - 10.0)

                sdf.box(x0 + 0.5, cy - well * 0.5, w - 1.0, well, fab.radius)
                let mut base = fab.color_num.mix(fab.color_num_hover, self.hover)
                base = vec4(base.xyz, base.w * dim)
                sdf.fill_keep(base)
                let mut border = fab.color_border.mix(fab.color_focus_ring, self.focus)
                border = vec4(border.xyz, border.w * dim)
                sdf.stroke(border, 1.0)

                // The thumb's centre never leaves the well, so what it runs
                // over is the well less the inset at both ends and its own
                // width — the three numbers `SliderTravel` measures with.
                let span = max(1.0, w - self.inset_px * 2.0 - self.thumb_px)
                let tc = x0 + self.inset_px + self.thumb_px * 0.5 + self.travel * span
                sdf.box(x0 + 1.0, cy - well * 0.5 + 1.0, max(1.0, tc - x0 - 1.0), well - 2.0, fab.radius)
                sdf.fill(vec4(fab.color_num_fill.xyz, 0.85 * dim))

                sdf.box(tc - self.thumb_px * 0.5, 2.0, self.thumb_px, h - 4.0, fab.radius)
                let mut face = fab.color_button.mix(fab.color_button_hover, self.hover).mix(fab.color_button_down, self.down)
                face = vec4(face.xyz, face.w * dim)
                sdf.fill_keep(face)
                sdf.stroke(vec4(fab.color_border.xyz, fab.color_border.w * dim), 1.0)
                return sdf.result
            }
        }

        mod.widgets.FabSliderBase = #(FabSlider::register_widget(vm))
        /** The track: a press anywhere on it lands the thumb under the
         * pointer and the drag keeps it there, a click on the name takes the
         * row back to zero. */
        mod.widgets.FabSlider = set_type_default() do mod.widgets.FabSliderBase{
            width: Fill
            height: fab.row_height
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
            margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
            // No spacing: the three columns are measured rather than spaced.
            // The hit test derives the track from the two column widths, and
            // a gap the turtle put in is a gap it cannot see.
            spacing: 0

            label: ""
            label_width: fab.prop_label_width
            readout_width: 34.0
            min: 0.0
            max: 100.0
            /** the arrow-key increment, and the detent a drag lands on 0..25 step 0.5 */
            step: 1.0
            /** the shift+arrow increment 0..50 step 0.5 */
            big_step: 10.0
            precision: 0
            unit: "%"
            value: 0.0
            thumb_size: 12.0
            track_inset: 2.0
            enabled: true

            draw_label +: {
                ink_centered: true
                color: fab.color_text_dim
                text_overflow: TextOverflow.Ellipsis
                text_style: fab.font{
                    font_size: fab.font_size_ui
                }
            }
            draw_value +: {
                ink_centered: true
                color: fab.color_text
                text_style: fab.font{
                    font_size: fab.font_size_ui
                }
            }
        }

        set_type_default() do #(DrawFabKnob::script_shader(vm)){
            ..mod.draw.DrawQuad

            // `#[live]` fields on DrawFabKnob, so they are already
            // instances — see DrawDragNum above.
            hover: 0.0
            down: 0.0
            focus: 0.0
            disabled: 0.0
            travel: 0.0
            label_px: 0.0
            readout_px: 0.0

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // Where the face stands in the box: the three lines of
                // `knob_face`, to the letter. Rust lays the name and the
                // number out round the same arithmetic, so the words land
                // above and below the dial and never on it.
                let rows = self.label_px + self.readout_px
                let d = max(min(self.rect_size.x, self.rect_size.y - rows), 0.0)
                let top = max((self.rect_size.y - rows - d) * 0.5, 0.0)
                let c = vec2(self.rect_size.x * 0.5, top + self.label_px + d * 0.5)
                let r = d * 0.5
                let dim = 1.0 - 0.6 * self.disabled

                // Every thickness is a share of the radius over a floor in
                // pixels. The shares are what a 44 face is drawn with; the
                // floors are what a 28 face is drawn with, which is the size
                // a matrix cell comes down to, and under them a groove stops
                // being a groove and becomes a grey smear.
                let groove = max(r * 0.18, 2.5)
                let ring_r = max(r - 1.0 - groove * 0.5, 1.0)
                let cap_r = max(ring_r - groove * 0.5 - max(r * 0.1, 1.5), 1.0)

                // Sdf2d measures an arc from straight DOWN and turns
                // clockwise, so an eighth of a turn in is half past seven
                // and three quarters of a turn on from there is half past
                // four: the opening sits at the bottom, where a hand expects
                // the two stops of a dial to be.
                let start = PI * 0.25
                let sweep = PI * 1.5
                let at = start + sweep * self.travel

                // NOUGHT IS OFF, and it has to read as off from across a
                // matrix in which most of the cells are at nought. So the
                // lit arc is not drawn at all there -- an arc of no length
                // still draws its round cap, which is a lamp on the stop and
                // says "a little" rather than "nothing" -- and the tick goes
                // down to the muted ink. Anything above nought is on, however
                // little of it there is: the lamp on the stop is its first
                // sign.
                let lit = step(0.0005, self.travel)
                let lifted = max(self.hover, self.down)

                // The groove does NOT lift under the pointer, where the
                // slider's well does. A well is a slab with a border round
                // it; a groove is a line on the panel's own ground, and the
                // well's hover tone is within a shade of that ground -- the
                // unlit half of the dial went out at the moment a hand
                // arrived to turn it. The cap and the arc say hover instead.
                sdf.arc_round_caps(c.x, c.y, ring_r, start, start + sweep, groove)
                sdf.fill(vec4(fab.color_num.xyz, fab.color_num.w * dim))

                if lit > 0.5 {
                    sdf.arc_round_caps(c.x, c.y, ring_r, start, at, groove)
                    let accent = fab.color_accent.mix(fab.color_accent_hover, lifted)
                    sdf.fill(vec4(accent.xyz, accent.w * dim))
                }

                // The cap is the button face of the rest of the kit, and it
                // answers the hand the way the slider's thumb does. Its edge
                // is where the keyboard shows: the same ring the wells wear.
                sdf.circle(c.x, c.y, cap_r)
                let mut face = fab.color_button.mix(fab.color_button_hover, self.hover).mix(fab.color_button_down, self.down)
                face = vec4(face.xyz, face.w * dim)
                sdf.fill_keep(face)
                let mut edge = fab.color_border.mix(fab.color_focus_ring, self.focus)
                edge = vec4(edge.xyz, edge.w * dim)
                sdf.stroke(edge, 1.0 + 0.5 * self.focus)

                // The tick points at the value's own angle, from a third of
                // the way out to just short of the cap's edge. The arc's
                // round cap un-rotates to centre + radius * (-sin, cos), so
                // the tick and the head of the lit arc share one bearing.
                let dir = vec2(0.0 - sin(at), cos(at))
                let heel = cap_r * 0.3
                let tip = max(cap_r - 2.0, heel + 1.0)
                sdf.move_to(c.x + dir.x * heel, c.y + dir.y * heel)
                sdf.line_to(c.x + dir.x * tip, c.y + dir.y * tip)
                let mut ink = fab.color_text_muted.mix(fab.color_text, lit)
                ink = ink.mix(fab.color_text_active, lifted * lit)
                sdf.stroke(vec4(ink.xyz, ink.w * dim), max(r * 0.07, 1.0))
                return sdf.result
            }
        }

        mod.widgets.FabKnobBase = #(FabKnob::register_widget(vm))
        /** The dial: press it and pull up for more or down for less, the
         * whole range in a hand's width of travel; a double click takes it
         * back to nought. Sized by the cell it is put in, down to 28 wide. */
        mod.widgets.FabKnob = set_type_default() do mod.widgets.FabKnobBase{
            // 44 across and 64 down: a 44 face, the number under it, and the
            // slack shared above and below. Both are only a default. The face
            // is the biggest circle the box holds once the two text rows are
            // taken off its height, so a cell hands over whatever it has --
            // fixed or Fill, either way -- and the dial fits itself to it.
            width: 44
            height: 64
            flow: Down
            // Written out, and nought: the face is measured off the WHOLE
            // box, by the shader and by the layout alike, so padding here
            // would move the words and leave the dial where it was. A cell
            // that wants air round its knob asks for it with a margin.
            padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
            margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
            spacing: 0

            // No name by default, and then no row for one either: in a
            // matrix the column and the row headers carry the names, and
            // twelve pixels of nothing over every face is a row of knobs
            // fewer on the panel.
            label: ""
            label_height: 12.0
            show_readout: true
            readout_height: 12.0
            min: 0.0
            max: 100.0
            /** the arrow-key and wheel increment, and the detent a drag lands on 0..25 step 0.5 */
            step: 1.0
            /** the shift+arrow and shift+wheel increment 0..50 step 0.5 */
            big_step: 10.0
            precision: 0
            unit: "%"
            value: 0.0
            /** how far the pointer travels for the whole range, in points 40..400 step 10 */
            drag_travel: 150.0
            wheel_on_hover: false
            enabled: true

            draw_label +: {
                ink_centered: true
                color: fab.color_text_dim
                text_overflow: TextOverflow.Ellipsis
                text_style: fab.font{
                    font_size: fab.font_size_small
                }
            }
            draw_value +: {
                ink_centered: true
                color: fab.color_text
                text_style: fab.font{
                    font_size: fab.font_size_small
                }
            }
            // The number under a knob standing at nought. A second ink and
            // not a second size, so a column of cells keeps one baseline and
            // only the cells that count for something are lit.
            draw_value_off +: {
                ink_centered: true
                color: fab.color_text_muted
                text_style: fab.font{
                    font_size: fab.font_size_small
                }
            }
        }

        set_type_default() do #(DrawColorWheel::script_shader(vm)){
            ..mod.draw.DrawQuad

            hue: 0.0
            sat: 0.0
            val: 0.0

            pixel: fn() {
                let size = min(self.rect_size.x, self.rect_size.y)
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let dx = self.pos.x * self.rect_size.x - c.x
                let dy = self.pos.y * self.rect_size.y - c.y

                let outer = size * 0.48
                let inner = size * 0.385
                let half = size * 0.255

                // Hue ring: 0 at twelve o'clock, clockwise, red at the top.
                sdf.circle(c.x, c.y, outer)
                sdf.circle(c.x, c.y, inner)
                sdf.subtract()
                let ang = atan2(dx, 0.0 - dy)
                let hue_at = fract(ang / 6.2831853 + 1.0)
                sdf.fill(Pal.hsv2rgb(vec4(hue_at, 1.0, 1.0, 1.0)))

                // Saturation/value square at the current hue.
                let sq_s = clamp((dx + half) / (2.0 * half), 0.0, 1.0)
                let sq_v = 1.0 - clamp((dy + half) / (2.0 * half), 0.0, 1.0)
                sdf.rect(c.x - half, c.y - half, half * 2.0, half * 2.0)
                sdf.fill(Pal.hsv2rgb(vec4(self.hue, sq_s, sq_v, 1.0)))

                // Pucks: a dark outline with a light ring inside stays
                // visible over any colour underneath.
                let mid = (outer + inner) * 0.5
                let pa = self.hue * 6.2831853
                let rp = vec2(c.x + sin(pa) * mid, c.y - cos(pa) * mid)
                sdf.circle(rp.x, rp.y, 6.5)
                sdf.stroke(vec4(0.04, 0.04, 0.04, 0.9), 1.4)
                sdf.circle(rp.x, rp.y, 5.0)
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 1.6)

                let sp = vec2(
                    c.x - half + self.sat * 2.0 * half,
                    c.y - half + (1.0 - self.val) * 2.0 * half
                )
                sdf.circle(sp.x, sp.y, 6.0)
                sdf.stroke(vec4(0.04, 0.04, 0.04, 0.9), 1.4)
                sdf.circle(sp.x, sp.y, 4.5)
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 1.6)

                return sdf.result
            }
        }

        mod.widgets.FabColorWheelBase = #(FabColorWheel::register_widget(vm))
        mod.widgets.FabColorWheel = set_type_default() do mod.widgets.FabColorWheelBase{
            width: 220
            height: 220
        }

        set_type_default() do #(DrawFabSwatch::script_shader(vm)){
            ..mod.draw.DrawQuad
            hover: 0.0
            open: 0.0
            lifted: 0.0
            target: 0.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                // A colour carried off its square leaves the square most of
                // the way to an empty field, so the hand can see where it
                // came from and that it is no longer there.
                sdf.fill_keep(vec4(self.swatch.xyz, 1.0).mix(fab.color_input, self.lifted * 0.75))
                // The square a colour left is not the one in the hand, so it
                // gives up its hover ring while it is empty.
                let lit = max(max(self.hover, self.open) * (1.0 - self.lifted), self.target)
                let ring = fab.color_border.mix(fab.color_focus_ring, lit)
                sdf.stroke(ring, 1.0 + self.target)
                return sdf.result
            }
        }

        // ---- type ----
        // The stock `Label` carries padding that overflows a 20 px fab row;
        // zero padding and centred ink keep every label on the row's line.
        mod.widgets.FabLabel = Label{
            width: Fit
            height: Fit
            padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
            draw_text +: {
                ink_centered: true
                color: fab.color_text
                text_style: fab.font{
                    font_size: fab.font_size_ui
                }
            }
        }
        mod.widgets.FabLabelDim = mod.widgets.FabLabel{
            draw_text +: {
                color: fab.color_text_dim
            }
        }
        mod.widgets.FabLabelSmall = mod.widgets.FabLabel{
            draw_text +: {
                color: fab.color_text_dim
                text_style: fab.font{
                    font_size: fab.font_size_small
                }
            }
        }
        mod.widgets.FabHeaderLabel = mod.widgets.FabLabel{
            draw_text +: {
                color: fab.color_text_header
                text_style: fab.font{
                    font_size: fab.font_size_header
                }
            }
        }

        // The name over a column that cannot hold it flat. Kept here among
        // the labels because that is what it is; what makes it its own
        // control is that the ink is MEANT to leave the box.
        // The lean itself is declared with the arithmetic, in
        // diagonal_text.rs, because the library's tables turn a heading the
        // same way and there can only be one of it.
        let DiagonalLean = mod.widgets.DiagonalLean
        mod.widgets.FabDiagonalLabelBase = #(FabDiagonalLabel::register_widget(vm))
        /** A name written across the corner of the box it names, for a
         * column too narrow to hold it flat: a matrix header. The ink
         * overflows its own box on purpose, so the row these stand in wants
         * `clip_x: false` and room at the end they lean over. */
        mod.widgets.FabDiagonalLabel = set_type_default() do mod.widgets.FabDiagonalLabelBase{
            // Fill so a header row divides itself between its columns the
            // way the grid under it does. 60 down is measured and not
            // guessed: the longest theme name the library ships takes 56.5
            // points of height at 45 degrees in the panel's small face, and
            // a host that knows its own longest name should fix this for
            // itself with `diagonal_row_height`.
            width: Fill
            height: 60
            text: ""
            /** how far from the horizontal the name is turned, in degrees 0..90 step 5 */
            angle: 45.0
            /** Fall hangs the name over the LEFT, Rise over the right */
            lean: DiagonalLean.Fall

            draw_text +: {
                color: fab.color_text_dim
                text_style: fab.font{
                    font_size: fab.font_size_small
                }
            }
        }

        // ---- the search well ----
        mod.widgets.FabSearch = View{
            width: Fill
            height: fab.row_height
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 6 right: 4 top: 0 bottom: 0}
            show_bg: true
            draw_bg +: {
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                    sdf.fill_keep(fab.color_input)
                    sdf.stroke(fab.color_border, 1.0)
                    return sdf.result
                }
            }
            input := TextInput{
                width: Fill
                height: Fill
                // The field fills a row of a FIXED height, so nothing here
                // may impose a height of its own. `android` and `ios` set
                // `mod.widgets.TextInput.min_height` to 48 and 44, and a
                // walk applies a min height UNCONDITIONALLY -- the height
                // asked for cannot escape it (draw/src/turtle.rs, where
                // `walk.min_height` is resolved). The field's content box
                // then stood 48 tall inside a 24 tall well, and a
                // single-line input CENTRES its line box in that box
                // (`TextInput::scroll_to_cursor`), so the word "Filter" was
                // drawn 12px below where the well ends: sunk to the bottom
                // of the box, straddling the lower border. Zero is what the
                // default theme resolves to anyway, so this moves nothing
                // that is on screen today.
                min_height: 0
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
                empty_text: "Filter"
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                    color_hover: vec4(0.0, 0.0, 0.0, 0.0)
                    color_focus: vec4(0.0, 0.0, 0.0, 0.0)
                    color_down: vec4(0.0, 0.0, 0.0, 0.0)
                    color_empty: vec4(0.0, 0.0, 0.0, 0.0)
                    border_size: 0.0
                    border_radius: 0.0
                    // The field has no ground of its own: the well around it
                    // is this FabSearch View's `draw_bg`, drawn in the fab
                    // palette. Written out because `windows-2000` and
                    // `nextstep` REPLACE `mod.widgets.TextInput.draw_bg.pixel`
                    // outright with a hard-coded opaque white Win95 field,
                    // which ignores every colour declared above and would
                    // paint a white slab over the well -- leaving the
                    // panel's own light grey text on white. With the colours
                    // above all transparent and no border, this is exactly
                    // what the stock face already resolves to today.
                    pixel: fn() {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                }
                draw_text +: {
                    ink_centered: true
                    color: fab.color_text
                    // Every state, not just the resting one: the stock field takes the
                    // app theme's ink for hover, focus and down, and under a light theme
                    // that ink is dark -- so the word being typed went black on this
                    // panel's dark well the moment the box took focus.
                    color_hover: fab.color_text_active
                    color_focus: fab.color_text_active
                    color_down: fab.color_text_active
                    color_disabled: fab.color_text_muted
                    color_empty: fab.color_text_muted
                    color_empty_hover: fab.color_text_dim
                    color_empty_focus: fab.color_text_dim
                    text_style: fab.font{
                        font_size: fab.font_size_ui
                    }
                }
            }
        }

        // ---- label-left / value-right row ----
        mod.widgets.FabPropRow = View{
            width: Fill
            height: fab.row_height
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
            spacing: 6
            name := mod.widgets.FabLabelDim{
                width: fab.prop_label_width
                text: "Name"
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
            }
        }

        // ---- clickable section header (text chevron; icons stay SVG-only
        // elsewhere, a fold glyph is text) ----
        mod.widgets.FabSection = View{
            width: Fill
            height: 22
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
            spacing: 4
            cursor: MouseCursor.Hand
            show_bg: true
            draw_bg +: {
                hover: instance(0.0)
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                    sdf.fill(fab.color_panel.mix(fab.color_button_hover, self.hover * 0.5))
                    return sdf.result
                }
            }
            title := mod.widgets.FabHeaderLabel{ text: "Section" }
        }

        // ---- theme palette strip (in the colour popover) ----
        set_type_default() do #(DrawFabPaletteCell::script_shader(vm)){
            ..mod.draw.DrawQuad
            // `#[live]` fields on DrawFabPaletteCell, so they are already
            // instances — see DrawDragNum above: `instance(..)` here hands a
            // Vec4f (and two f32s) an object, and every app that loads these
            // widgets says so in three lines at startup.
            cell: vec4(0.0, 0.0, 0.0, 1.0)
            hot: 0.0
            cur: 0.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 2.0)
                // A checker under the colour so translucent entries read as such.
                let cx = floor(self.pos.x * self.rect_size.x / 4.0)
                let cy = floor(self.pos.y * self.rect_size.y / 4.0)
                let ch = modf(cx + cy, 2.0)
                let back = vec3(0.22, 0.22, 0.22).mix(vec3(0.34, 0.34, 0.34), ch)
                let rgb = back.mix(self.cell.xyz, self.cell.w)
                sdf.fill_keep(vec4(rgb, 1.0))
                let ring = fab.color_border.mix(fab.color_focus_ring, max(self.hot, self.cur))
                sdf.stroke(ring, 1.0)
                return sdf.result
            }
        }
        mod.widgets.FabPaletteStripBase = #(FabPaletteStrip::register_widget(vm))
        mod.widgets.FabPaletteStrip = set_type_default() do mod.widgets.FabPaletteStripBase{
            width: Fill
            height: Fit
            cell_size: 12.0
            gap: 2.0
        }

        // ---- every palette on offer, in a row that scrolls ----
        set_type_default() do #(DrawFabPaletteChip::script_shader(vm)){
            ..mod.draw.DrawQuad
            band_0: vec4(0.0, 0.0, 0.0, 1.0)
            band_1: vec4(0.0, 0.0, 0.0, 1.0)
            band_2: vec4(0.0, 0.0, 0.0, 1.0)
            band_3: vec4(0.0, 0.0, 0.0, 1.0)
            bands: 4.0
            hover: 0.0
            cur: 0.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 2.0)
                // The band under this fragment, chosen by three steps rather
                // than by branching: a chip is small and every fragment of it
                // takes this path. A step at or past the chip's foot never
                // fires, which is how a chip of fewer bands shows fewer.
                let y = self.pos.y
                let mut band = self.band_0
                band = band.mix(self.band_1, step(1.0 / self.bands, y))
                band = band.mix(self.band_2, step(2.0 / self.bands, y))
                band = band.mix(self.band_3, step(3.0 / self.bands, y))
                sdf.fill_keep(vec4(band.xyz, 1.0))
                // The ring is how the chip in force is told from the rest,
                // so it thickens as well as lights: a row of chips at this
                // size is mostly colour, and a hue alone does not carry a
                // one-pixel difference in an edge.
                let ring = fab.color_border.mix(fab.color_focus_ring, max(self.hover, self.cur))
                sdf.stroke(ring, 1.0 + self.cur)
                return sdf.result
            }
        }
        mod.widgets.FabPaletteCarouselBase = #(FabPaletteCarousel::register_widget(vm))
        /** Every palette on offer in one row that scrolls sideways: a chip
         * each, four colours stacked with the first on top, the one in force
         * outlined. A chip is four times as tall as it is wide, so each of
         * its colours is a square; the row is one chip high and as wide as
         * it is given. A host showing palettes of fewer colours says how
         * many, and the chips and the row come down to that many squares. */
        mod.widgets.FabPaletteCarousel = set_type_default() do mod.widgets.FabPaletteCarouselBase{
            width: Fill
            height: 88
            chip_width: 22.0
            chip_height: 88.0
            gap: 3.0
        }

        mod.widgets.FabColorPickBase = #(FabColorPick::register_widget(vm))
        mod.widgets.FabColorPick = set_type_default() do mod.widgets.FabColorPickBase{
            width: fab.swatch_width
            height: 16
            with_alpha: true
            // The ring colour a square lights with as a swap target, so the
            // two marks of one carry read as one family, and a round-ended
            // bar so it reads as a mark and not as a stray line of the row.
            draw_insert +: {
                color: fab.color_focus_ring
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.rect_size.x * 0.5)
                    sdf.fill(self.color)
                    return sdf.result
                }
            }
            popover: View{
                width: 244
                height: Fit
                flow: Down
                padding: 8
                spacing: 6
                show_bg: true
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                        sdf.fill_keep(fab.color_popover)
                        sdf.stroke(fab.color_popover_border, 1.0)
                        return sdf.result
                    }
                }
                wheel := mod.widgets.FabColorWheel{
                    width: 228
                    height: 228
                }
                // Seven tracks, not seven scrubs: a filled row reads as a
                // slider and is one — press it where the value should be.
                num_r := mod.widgets.FabValueInput{ label: "R" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                num_g := mod.widgets.FabValueInput{ label: "G" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                num_b := mod.widgets.FabValueInput{ label: "B" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                num_a := mod.widgets.FabValueInput{ label: "A" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                // The same colour said the other way. Hue is a clamped
                // 0..360 here and not a cyclic one: a track carries a fill,
                // a fill has two ends, and a row that jumped from 360 back
                // to 0 under the hand would fight what the eye is reading.
                // The ring above is where hue comes round.
                num_h := mod.widgets.FabValueInput{ label: "H" min: 0.0 max: 360.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                num_s := mod.widgets.FabValueInput{ label: "S" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                num_v := mod.widgets.FabValueInput{ label: "V" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                hex_row := View{
                    width: Fill
                    height: fab.row_height
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    spacing: 6
                    mod.widgets.FabLabelDim{ width: 30 text: "Hex" }
                    pick := Button{
                        width: Fit
                        height: Fill
                        // `android` and `ios` set `mod.widgets.Button.min_height`
                        // to 48 and 44; a walk applies it whatever height the
                        // instance asked for, so this row would stand twice
                        // its height inside a popover sized for one.
                        min_height: 0
                        padding: Inset{left: 6 right: 6 top: 2 bottom: 2}
                        text: "pick"
                    }
                    hex := TextInput{
                        width: Fill
                        height: Fill
                        min_height: 0
                        empty_text: ""
                        draw_bg +: {
                            color: fab.color_input
                            border_radius: fab.radius
                        }
                        draw_text +: {
                            color: fab.color_text
                            ink_centered: true
                            text_style: fab.font{ font_size: fab.font_size_ui }
                        }
                    }
                }
                // The host's theme palette: hover names (and pulses) a
                // colour, a click binds the property to it by reference.
                palette_name := mod.widgets.FabLabelDim{ width: Fill text: "" }
                palette := mod.widgets.FabPaletteStrip{}
            }
        }
    };
    vm.eval(block);
}

// ===========================================================================
// Shared pure helpers (color space, hex, wheel geometry)
// ===========================================================================

/// HSV → RGB, all channels 0..1. `h` wraps.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// RGB → HSV, all channels 0..1. A grey keeps hue 0 and sat 0.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, s, v]
}

/// Accepts `#rgb`, `#rrggbb`, `#rrggbbaa`, each with or without the hash.
/// Returns the colour and whether the string carried alpha.
pub fn parse_hex(text: &str) -> Option<([f32; 4], bool)> {
    let t = text.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nib = |c: u8| -> f32 {
        let d = (c as char).to_digit(16).unwrap_or(0) as f32;
        d / 15.0
    };
    let byte = |hi: u8, lo: u8| -> f32 {
        let h = (hi as char).to_digit(16).unwrap_or(0);
        let l = (lo as char).to_digit(16).unwrap_or(0);
        ((h * 16 + l) as f32) / 255.0
    };
    let b = t.as_bytes();
    match b.len() {
        3 => Some(([nib(b[0]), nib(b[1]), nib(b[2]), 1.0], false)),
        6 => Some((
            [byte(b[0], b[1]), byte(b[2], b[3]), byte(b[4], b[5]), 1.0],
            false,
        )),
        8 => Some((
            [
                byte(b[0], b[1]),
                byte(b[2], b[3]),
                byte(b[4], b[5]),
                byte(b[6], b[7]),
            ],
            true,
        )),
        _ => None,
    }
}

/// `#rrggbb`, or `#rrggbbaa` when `with_alpha`.
pub fn format_hex(rgba: [f32; 4], with_alpha: bool) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    if with_alpha {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            b(rgba[0]),
            b(rgba[1]),
            b(rgba[2]),
            b(rgba[3])
        )
    } else {
        format!("#{:02x}{:02x}{:02x}", b(rgba[0]), b(rgba[1]), b(rgba[2]))
    }
}

/// Air before the first figure inside a track row's readout well, in points.
/// The editor drawn in the well starts here, and a single-line field draws
/// its text from its own left edge, so this is what keeps the figures off the
/// well's border.
pub const NUM_BOX_PAD_L: f64 = 5.0;
/// Air after the last figure inside that well.
pub const NUM_BOX_PAD_R: f64 = 6.0;

/// Ring outer radius as a fraction of the widget size (the shader uses the
/// same constants, so hit testing and pixels never disagree).
pub const RING_OUTER: f64 = 0.48;
/// Ring inner radius as a fraction of the widget size.
pub const RING_INNER: f64 = 0.385;
/// Half-side of the SV square as a fraction of the widget size.
pub const SQUARE_HALF: f64 = 0.255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelZone {
    Ring,
    Square,
    None,
}

/// How far from a puck's centre a press still grabs it: the drawn puck's
/// outer ring (6.5 points, stroked 1.4 wide in the shader) and a little more
/// for the hand.
pub const PUCK_GRAB: f64 = 8.0;

/// Which zone a pointer at `rel` (widget-local, origin top-left) lands in,
/// for a wheel drawn at `size` (its smaller dimension) showing `hsv`.
///
/// A press on a puck grabs that puck, wherever the point falls. The colours
/// people pick live on the square's edge -- every grey at saturation 0, the
/// full colours at 1, black and the brights at the bottom and the top -- and
/// a puck there hangs half outside the square: a press on its outer half
/// used to fall in the gap before the ring and do nothing, or at the top
/// right corner, where the full colours are, reach the ring and move the
/// hue instead. Off the pucks, the gap between the square and the ring goes
/// to whichever of the two is nearer rather than to nothing.
pub fn wheel_zone(rel: DVec2, size: f64, hsv: [f32; 3]) -> WheelZone {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let half = SQUARE_HALF * size;
    let [h, s, v] = hsv.map(|c| c as f64);
    // The pucks where the shader draws them.
    let square_puck = dvec2(-half + s * 2.0 * half, -half + (1.0 - v) * 2.0 * half);
    let mid = (RING_OUTER + RING_INNER) * 0.5 * size;
    let angle = h * std::f64::consts::TAU;
    let ring_puck = dvec2(angle.sin() * mid, -angle.cos() * mid);
    let at = dvec2(dx, dy);
    let to_square_puck = (at - square_puck).length();
    let to_ring_puck = (at - ring_puck).length();
    if to_square_puck.min(to_ring_puck) <= PUCK_GRAB {
        return if to_square_puck <= to_ring_puck { WheelZone::Square } else { WheelZone::Ring };
    }
    if dx.abs() <= half && dy.abs() <= half {
        return WheelZone::Square;
    }
    let r = (dx * dx + dy * dy).sqrt();
    let inner = RING_INNER * size - 4.0;
    if r <= RING_OUTER * size + 4.0 && r >= inner {
        return WheelZone::Ring;
    }
    if r < inner {
        // The gap: how far outside the square, against how far inside the ring.
        let ox = (dx.abs() - half).max(0.0);
        let oy = (dy.abs() - half).max(0.0);
        let off_square = (ox * ox + oy * oy).sqrt();
        return if off_square <= inner - r { WheelZone::Square } else { WheelZone::Ring };
    }
    WheelZone::None
}

/// Hue (0..1) for a pointer on the ring: 0 at twelve o'clock, increasing
/// clockwise, red at the top.
pub fn ring_hue(rel: DVec2, size: f64) -> f32 {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let ang = dx.atan2(-dy);
    ((ang / std::f64::consts::TAU).rem_euclid(1.0)) as f32
}

/// (saturation, value) for a pointer over the SV square, clamped so a drag
/// that leaves the square keeps tracking the nearest edge.
pub fn square_sv(rel: DVec2, size: f64) -> (f32, f32) {
    let half = SQUARE_HALF * size;
    let cx = size * 0.5;
    let s = ((rel.x - (cx - half)) / (half * 2.0)).clamp(0.0, 1.0);
    let v = 1.0 - ((rel.y - (cx - half)) / (half * 2.0)).clamp(0.0, 1.0);
    (s as f32, v as f32)
}

// ===========================================================================
// FabValueInput — the drag-numeric field. The pure drag core carries every
// mapping decision, no Cx anywhere.
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDragNum {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    fill: f32,
    /// Hide the idle chip; hover/down/focus still reveal the editor surface.
    #[live]
    flat: f32,
    /// Draw the ‹ › stepper chevrons on hover. Off on a row that is a
    /// track: their end zones are the track's ends there.
    #[live(1.0)]
    stepper: f32,
    /// How far the fill is held off each end of the row, in points. Nothing
    /// by default; a track row holds it clear of the name and the number, so
    /// that the two ends of the fill are the two ends of the range and a
    /// press on either lands there. In points rather than a fraction because
    /// the columns are measured before the row knows how wide it is.
    #[live]
    fill_pad_l: f32,
    #[live]
    fill_pad_r: f32,
    /// The readout's WELL: its width in points, or zero for no well. A box
    /// the number sits in, so that a place a number can be typed looks like
    /// one. Off on a scrub field, whose whole face is the well.
    #[live]
    num_box: f32,
    /// How far the well's right edge is held off the row's, in points — the
    /// row's own right padding, so the well and the editor drawn in it line
    /// up exactly. Both are in points because the row's width is not known
    /// when the instance is written.
    #[live]
    num_box_r: f32,
}

#[derive(Clone, Debug, Default)]
pub enum FabValueInputAction {
    /// Live while dragging or after a typed entry.
    Changed(f64),
    /// The gesture finished (mouse up / Enter) — commit points.
    Ended(f64),
    /// Double-click: the host should reset this field's prop to its
    /// baseline and drop it from any change ledger.
    Reset,
    #[default]
    None,
}

/// The numeric contract one field carries into a drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragParams {
    pub min: f64,
    pub max: f64,
    /// One arrow-click / wheel-step increment.
    pub step: f64,
    /// Cyclic: the value comes round at the ends instead of clamping.
    pub wrap: bool,
    /// Bounded mapping: the field's width sweeps the whole range.
    pub bounded: bool,
    /// Explicit Ctrl-snap increment; `0` picks a rung from the range.
    pub snap_override: f64,
}

impl DragParams {
    pub fn range(&self) -> f64 {
        self.max - self.min
    }
    fn has_range(&self) -> bool {
        self.max > self.min
    }
}

/// Where a drag measures from. Clamping and modifier changes move the
/// anchor rather than the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragAnchor {
    pub x: f64,
    pub value: f64,
}

/// A press engages into a drag only past this much horizontal travel;
/// below it, the release is a click.
pub const DRAG_THRESHOLD: f64 = 3.0;

/// Value change per pixel for the current mapping and modifiers.
/// Bounded: the range across `width`, ×0.05 fine. Unbounded: one step per
/// pixel — the drag is the coarse gesture, Shift (×0.1) the fine one.
/// How many field-widths of travel a bounded scrub takes to cross its whole
/// range.
///
/// One was the obvious mapping and the wrong one: the pointer moving with
/// the value 1:1 across a 48-point field means the entire range passes under
/// a thumb's width of movement, and nothing in between can be landed on.
/// Four gives the hand somewhere to go.
const DRAG_RANGE_TRAVEL: f64 = 4.0;

pub fn drag_rate(p: &DragParams, width: f64, shift: bool) -> f64 {
    if p.bounded && p.has_range() {
        let rate = p.range() / (width.max(1.0) * DRAG_RANGE_TRAVEL);
        if shift {
            rate * 0.05
        } else {
            rate
        }
    } else {
        let rate = p.step;
        if shift {
            rate * 0.1
        } else {
            rate
        }
    }
}

/// The Ctrl-snap increment: an explicit override wins, otherwise a rung
/// sized to the range, and Ctrl+Shift takes the next finer rung.
pub fn snap_increment(p: &DragParams, fine: bool) -> f64 {
    let base = if p.snap_override > 0.0 {
        p.snap_override
    } else {
        let range = if p.has_range() { p.range() } else { 21.0 };
        if range < 2.1 {
            0.1
        } else if range < 21.0 {
            1.0
        } else {
            10.0
        }
    };
    if fine {
        base * 0.1
    } else {
        base
    }
}

/// One step of the drag mapping: pointer at `x`, modifiers as held right
/// now. Returns the value to publish and the anchor to carry forward
/// (shifted when a limit was hit). Both ends stay reachable under snap.
pub fn drag_map(
    p: &DragParams,
    anchor: DragAnchor,
    x: f64,
    width: f64,
    shift: bool,
    ctrl: bool,
) -> (f64, DragAnchor) {
    let rate = drag_rate(p, width, shift);
    let raw = anchor.value + (x - anchor.x) * rate;

    let (ranged, anchor) = if p.has_range() {
        if p.wrap {
            let wrapped = p.min + (raw - p.min).rem_euclid(p.range());
            if (wrapped - raw).abs() > f64::EPSILON {
                (wrapped, DragAnchor { x, value: wrapped })
            } else {
                (raw, anchor)
            }
        } else {
            let clamped = raw.clamp(p.min, p.max);
            if (clamped - raw).abs() > f64::EPSILON {
                // Anchor shift: measure the rest of the drag from the limit.
                (clamped, DragAnchor { x, value: clamped })
            } else {
                (raw, anchor)
            }
        }
    } else {
        (raw, anchor)
    };

    // Snap the published value only; the anchor stays on the unsnapped
    // track so releasing Ctrl lands back on the pointer's own value.
    let mut publish = ranged;
    if ctrl {
        let inc = snap_increment(p, shift);
        if inc > 0.0 {
            publish = (ranged / inc).round() * inc;
            if p.has_range() && !p.wrap {
                publish = publish.clamp(p.min, p.max);
                if ranged <= p.min {
                    publish = p.min;
                } else if ranged >= p.max {
                    publish = p.max;
                }
            }
        }
    }
    (publish, anchor)
}

/// Re-anchor for a modifier change: the value stays put at the current
/// pointer position, only the rate changes from here on.
pub fn reanchor(current_value: f64, x: f64) -> DragAnchor {
    DragAnchor {
        x,
        value: current_value,
    }
}

/// The value a press at `x` (from the row's left edge) names on a row that
/// is a TRACK, whose fill runs from `lo` to `hi` in the same frame.
///
/// The arithmetic is the fill's, read backwards, and that is the whole point
/// of it: the two ends of the fill are the two ends of the range, so a hand
/// reaching for a zero or a maximum gets one by pressing where it can see the
/// fill ends. A mapping over the row's whole width would put both of those a
/// name's width and a number's width out of reach.
pub fn track_value_at(p: &DragParams, x: f64, lo: f64, hi: f64) -> f64 {
    let span = (hi - lo).max(1.0);
    let t = ((x - lo) / span).clamp(0.0, 1.0);
    p.min + t * p.range()
}

/// The three zones of the row: the stepping arrows at the ends and the
/// drag/edit surface between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldZone {
    Decrement,
    Middle,
    Increment,
}

/// Zone for a pointer at `x` within a row of `width`×`height`.
pub fn field_zone(x: f64, width: f64, height: f64) -> FieldZone {
    let zone = (width / 3.0).min(height * 0.7);
    if x < zone {
        FieldZone::Decrement
    } else if x > width - zone {
        FieldZone::Increment
    } else {
        FieldZone::Middle
    }
}

#[derive(Clone, Copy, Debug)]
struct DragState {
    press_x: f64,
    press_value: f64,
    width: f64,
    engaged: bool,
    anchor: DragAnchor,
    shift: bool,
    raw_value: f64,
}

#[derive(Script, Widget, Animator)]
pub struct FabValueInput {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,
    #[redraw]
    #[live]
    draw_bg: DrawDragNum,
    #[live]
    draw_text: DrawText,
    #[live]
    text_input: TextInput,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// A host can swap the field for another control in the same slot.
    #[live(true)]
    #[visible]
    visible: bool,

    /// Held during either a drag or text editing, so cancel never also
    /// dismisses the surrounding popup or modal.
    #[rust]
    cancel_scope: Option<CancelScope>,

    #[live]
    label: String,
    #[live]
    min: f64,
    #[live]
    max: f64,
    #[live(0.01)]
    step: f64,
    /// Explicit Ctrl-snap increment; `0` derives one from the range.
    #[live]
    snap: f64,
    #[live(2)]
    precision: usize,
    #[live]
    suffix: String,
    #[live]
    value: f64,
    #[live]
    wrap: bool,
    /// The range came from a `/**name min..max step s*/` doc-channel hint:
    /// a hint, never a clamp — a typed value outside it EXPANDS the range.
    #[live]
    hint_bounds: bool,
    /// Bounded: the fill bar shows the value's place in the range and a
    /// drag sweeps the range across the row's width.
    #[live]
    show_fill: bool,
    #[live]
    quantize: bool,
    /// The row is a TRACK, not a scrub.
    ///
    /// A filled field reads as a slider, and a hand treats it as one: it
    /// presses where it wants the value and expects the value to be there.
    /// The scrub cannot answer that — it arms on the press, waits three
    /// pixels, then moves the number by a fraction of the range per pixel
    /// with the pointer pinned out of sight, so a press changes nothing and
    /// a short pull inside a 228-point popover moves a 0..255 channel by
    /// five. Switched on, the press lands the value under the pointer at
    /// once and the drag keeps it there, the way the panel's own sliders
    /// behave; the name still resets, the number still types, and the wheel
    /// and the arrows still step.
    ///
    /// Off by default: the fields the property panel scrubs are unbounded
    /// or nearly so, and a press that jumped them to wherever the pointer
    /// happened to be would be a disaster there.
    #[live]
    track: bool,
    /// Off: the value shows dimmed and nothing answers — no press, scrub,
    /// wheel step or click into text entry. A host switches a field off
    /// when what it drives is not there to be driven.
    #[live(true)]
    enabled: bool,

    #[rust]
    drag: Option<DragState>,
    /// A track row's press is live: the value follows the pointer until the
    /// release. Kept apart from `drag` so the scrub's own state machine —
    /// its threshold, its anchor, its pinned pointer — stays exactly as it
    /// is for the fields that still scrub.
    #[rust]
    tracking: bool,
    /// What a track row held when the press landed, for a cancel to put back.
    #[rust]
    track_press_value: f64,
    /// A track row's outer two columns, in points, as the row last drew
    /// them: the name on the left, the number on the right. Measured at
    /// draw time so the zones a press is read against are the columns the
    /// eye is looking at.
    #[rust]
    track_columns: (f64, f64),
    /// The readout column's width as the font last measured it, and the
    /// (figures, font size) it was measured for. Kept because the measure is
    /// a text layout and the answer only moves when one of those two does.
    #[rust]
    number_px: Option<(usize, f64, f64)>,
    /// Pointer over the field: the ‹ › stepper chevrons reveal.
    #[rust]
    hovered: bool,
    /// Time of the last primary press inside the field: two presses within
    /// the double-click window make a RESET gesture.
    #[rust]
    last_press_time: f64,
    /// Live-path measurement: FingerMoves delivered to this owner and
    /// publishes made during the current drag. Logged at drag end so a
    /// physical pass measures against the platform's PIN stats line.
    #[rust]
    drag_moves: u64,
    #[rust]
    drag_publishes: u64,
    #[rust]
    editing: bool,
}

impl ScriptHook for FabValueInput {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        let text = self.format();
        vm.with_cx_mut(|cx| {
            self.text_input.set_is_numeric_only(cx, false);
            self.text_input.set_text(cx, &text);
            self.text_input.set_is_read_only(cx, true);
        });
    }
}

impl FabValueInput {
    fn params(&self) -> DragParams {
        DragParams {
            min: self.min,
            max: self.max,
            step: self.step,
            wrap: self.wrap,
            bounded: self.show_fill,
            snap_override: self.snap,
        }
    }

    fn format(&self) -> String {
        let v = match self.precision {
            0 => format!("{:.0}", self.value),
            1 => format!("{:.1}", self.value),
            2 => format!("{:.2}", self.value),
            3 => format!("{:.3}", self.value),
            _ => format!("{}", self.value),
        };
        if self.suffix.is_empty() {
            v
        } else {
            format!("{v}{}", self.suffix)
        }
    }

    /// How many characters the widest number this row can print takes.
    ///
    /// The readout's column is measured from this and not from the number
    /// standing there now: a boundary that moved as "9" became "255" would
    /// put the end of the track somewhere different on every frame, and a
    /// press aimed at the maximum would sometimes open the editor instead.
    fn widest_number(&self) -> usize {
        let width_of = |v: f64| -> usize {
            let mut s = match self.precision {
                0 => format!("{v:.0}"),
                1 => format!("{v:.1}"),
                2 => format!("{v:.2}"),
                3 => format!("{v:.3}"),
                _ => format!("{v}"),
            };
            s.push_str(&self.suffix);
            s.chars().count()
        };
        width_of(self.min)
            .max(width_of(self.max))
            .max(self.format().chars().count())
    }

    /// How wide the readout column has to be, MEASURED rather than guessed.
    ///
    /// The number is drawn in the row's own face, and the column it stands in
    /// is also the end of the track: get it wrong and either the figures clip
    /// or the tracks of a stack of rows end in different places. It used to be
    /// `(figures + 0.5) * font_size * 0.72`, a ratio picked to be safe for the
    /// one font the panel ships and wrong for any other.
    ///
    /// So it is asked of the font instead: lay out each of the ten digits at
    /// the size the row draws at, take the WIDEST — proportional faces do not
    /// give "1" and "8" the same advance, and a column measured on the number
    /// standing there now would move as 9 became 255 — and give the well that
    /// many of them plus [`NUM_BOX_PAD_L`] and [`NUM_BOX_PAD_R`] of air, so
    /// the figures do not touch its edges. The COLUMN is the well plus the
    /// row's own right padding, which is what holds the well off the row's
    /// edge.
    ///
    /// [`FabValueInput::widest_number`] says how many figures, which for the
    /// colour popover's seven rows is three (255, 360, 100) on every one of
    /// them: that is why they come out one width without being told to.
    ///
    /// The row's label text and the embedded editor's are the same family at
    /// the same size, so one of them can answer for both.
    fn number_column(&mut self, cx: &mut Cx2d) -> f64 {
        // `widest_number` counts the suffix in with the figures; here the two
        // are measured apart, because a suffix is letters and not digits.
        let figures = self
            .widest_number()
            .saturating_sub(self.suffix.chars().count());
        let fs = self.draw_text.text_style.font_size as f64;
        if let Some((cached_figures, cached_fs, px)) = self.number_px {
            if cached_figures == figures && (cached_fs - fs).abs() < f64::EPSILON {
                return px;
            }
        }
        let mut widest_digit = 0.0_f64;
        for digit in 0..10u32 {
            let text = digit.to_string();
            let laidout =
                self.draw_text
                    .layout(cx, 0.0, 0.0, None, false, Align::default(), &text);
            if let Some(row) = laidout.rows.first() {
                widest_digit = widest_digit.max(row.width_in_lpxs as f64);
            }
        }
        // No font loaded yet (the first frame, or a headless test): fall back
        // to the old estimate rather than collapsing the column to nothing.
        if widest_digit <= 0.0 {
            widest_digit = fs * 0.72;
        }
        let suffix = if self.suffix.is_empty() {
            0.0
        } else {
            self.draw_text
                .layout(cx, 0.0, 0.0, None, false, Align::default(), &self.suffix)
                .rows
                .first()
                .map_or(0.0, |row| row.width_in_lpxs as f64)
        };
        let px = self.layout.padding.right
            + NUM_BOX_PAD_L
            + figures as f64 * widest_digit
            + suffix
            + NUM_BOX_PAD_R;
        self.number_px = Some((figures, fs, px));
        px
    }

    /// The string offered for editing: full precision, trailing zeros
    /// trimmed, so opening and committing an edit can never silently round
    /// the stored value.
    fn format_full(&self) -> String {
        let mut v = format!("{:.6}", self.value);
        if v.contains('.') {
            while v.ends_with('0') {
                v.pop();
            }
            if v.ends_with('.') {
                v.pop();
            }
        }
        v
    }

    fn normalize(&self, mut value: f64) -> f64 {
        if self.quantize && self.step > 0.0 {
            value = self.min + ((value - self.min) / self.step).round() * self.step;
        }
        if self.max <= self.min {
            return value;
        }
        if self.wrap {
            self.min + (value - self.min).rem_euclid(self.max - self.min)
        } else {
            value.clamp(self.min, self.max)
        }
    }

    fn parse(&self, text: &str) -> Option<f64> {
        let cleaned: String = text
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        cleaned.parse::<f64>().ok()
    }

    /// `TextInput::set_text` re-filters the string, clears the marks and the
    /// undo history, refloors the selection and throws the laid-out text away
    /// so the whole run is shaped again — all of it whether or not the string
    /// moved. Down a colour drag that is five text runs re-shaped per pointer
    /// move for numbers that mostly did not change, so the field is only
    /// written when what it says is not what it should say. (Guarded here and
    /// at the hex field rather than inside `set_text`, where a caller that
    /// sets the same string deliberately to clear the history would quietly
    /// stop working.)
    fn sync_text(&mut self, cx: &mut Cx) {
        let t = self.format();
        if self.text_input.text() != t {
            self.text_input.set_text(cx, &t);
        }
    }

    /// What the pointer says it can do here. A scrub's middle is a
    /// sideways pull; a track's is a place to put the value, and the two
    /// columns beside it are a word and a number to click.
    ///
    /// The track's arrows are the same sideways pair the scrub shows, and for
    /// the same reason: what the hand is about to do is move a value left and
    /// right. A hand was pointing at the fact that the row answers a press at
    /// all, which every row does.
    fn cursor_at(&self, abs_x: f64, face: Rect) -> MouseCursor {
        if self.track {
            let (label_px, readout_px) = self.track_columns;
            match slider_zone(abs_x - face.pos.x, face.size.x, label_px, readout_px) {
                SliderZone::Track => MouseCursor::EwResize,
                SliderZone::Readout => MouseCursor::Text,
                SliderZone::Label => MouseCursor::Hand,
            }
        } else {
            match field_zone(abs_x - face.pos.x, face.size.x, face.size.y) {
                FieldZone::Middle => MouseCursor::EwResize,
                _ => MouseCursor::Default,
            }
        }
    }

    /// Which of a track row's three columns a press at `abs_x` landed in.
    fn track_zone(&self, cx: &Cx, abs_x: f64) -> SliderZone {
        let face = self.draw_bg.area().rect(cx);
        let (label_px, readout_px) = self.track_columns;
        slider_zone(abs_x - face.pos.x, face.size.x, label_px, readout_px)
    }

    /// Where a track row's fill begins and ends, in the face's own frame.
    fn track_span(&self, width: f64) -> (f64, f64) {
        let (name, number) = self.track_columns;
        (1.0 + name, (width - number - 1.0).max(2.0 + name))
    }

    /// The value a track row's pointer is naming right now.
    fn track_value(&self, cx: &Cx, abs_x: f64) -> f64 {
        let face = self.draw_bg.area().rect(cx);
        let (lo, hi) = self.track_span(face.size.x);
        self.normalize(track_value_at(
            &self.params(),
            abs_x - face.pos.x,
            lo,
            hi,
        ))
    }

    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        if self.editing || self.drag.is_some() || self.tracking {
            return;
        }
        let v = self.normalize(v);
        if (v - self.value).abs() > f64::EPSILON {
            self.value = v;
            self.sync_text(cx);
            self.draw_bg.redraw(cx);
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Whether the number at the row's right hand end is open for typing.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// The middle of that number's box, in window points, for a host that
    /// wants to put a pointer there. Meaningless before the row has drawn,
    /// and on a row that is not a track, which has no box.
    pub fn number_box_middle(&self, cx: &Cx) -> Vec2d {
        let face = self.draw_bg.area().rect(cx);
        let well = self.draw_bg.num_box as f64;
        dvec2(
            face.pos.x + face.size.x - self.layout.padding.right - well * 0.5,
            face.pos.y + face.size.y * 0.5,
        )
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switching off mid-gesture ends the gesture first: an open editor
    /// closes without committing, an engaged scrub lets the pointer go.
    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            let uid = self.widget_uid();
            if self.editing {
                self.end_edit(cx);
                cx.revert_key_focus();
            }
            self.cancel_drag(cx, uid);
            self.hovered = false;
            self.animator_play(cx, ids!(hover.off));
        }
        self.draw_bg.redraw(cx);
    }

    /// Focus/IME state of the private text editor used while a scrub field is
    /// being typed. Canvas hosts cannot discover this child through the
    /// public widget tree because it is embedded directly, not a WidgetRef.
    pub fn text_ime_anchor(&self, cx: &Cx) -> Option<(Area, Rect, TextInputConfig)> {
        let area = self.text_input.area();
        if !self.editing || area.is_empty() || !cx.has_key_focus(area) {
            return None;
        }
        Some((
            area,
            self.text_input.cursor_rect_in_absolute(cx)?,
            self.text_input.ime_config(),
        ))
    }

    fn publish(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, ended: bool) {
        if (v - self.value).abs() > f64::EPSILON {
            self.value = v;
            self.sync_text(cx);
            self.draw_bg.redraw(cx);
            cx.widget_action(uid, FabValueInputAction::Changed(self.value));
        }
        if ended {
            cx.widget_action(uid, FabValueInputAction::Ended(self.value));
        }
    }

    fn step_once(&mut self, cx: &mut Cx, uid: WidgetUid, direction: f64, shift: bool) {
        let step = if shift { self.step * 0.1 } else { self.step };
        let v = self.normalize(self.value + direction * step.max(f64::EPSILON));
        if (v - self.value).abs() > f64::EPSILON {
            self.publish(cx, uid, v, true);
        }
    }

    pub fn begin_edit(&mut self, cx: &mut Cx) {
        self.drag = None;
        if self.cancel_scope.is_none() {
            self.cancel_scope = Some(self.begin_cancel_scope(cx));
        }
        self.editing = true;
        let full = self.format_full();
        self.text_input.set_is_numeric_only(cx, true);
        self.text_input.set_text(cx, &full);
        self.text_input.set_is_read_only(cx, false);
        self.text_input.set_key_focus(cx);
        self.text_input.select_all(cx);
        self.animator_play(cx, ids!(focus.on));
        self.draw_bg.redraw(cx);
    }

    fn end_edit(&mut self, cx: &mut Cx) {
        self.editing = false;
        self.cancel_scope = None;
        self.text_input.set_is_read_only(cx, true);
        self.text_input.set_is_numeric_only(cx, false);
        self.sync_text(cx);
        self.animator_play(cx, ids!(focus.off));
        self.draw_bg.redraw(cx);
    }

    fn commit_edit_text(&mut self, cx: &mut Cx, uid: WidgetUid, text: &str) {
        if let Some(parsed) = self.parse(text) {
            if self.hint_bounds && self.max > self.min {
                // Hint semantics: typing past a bound expands the range.
                self.min = self.min.min(parsed);
                self.max = self.max.max(parsed);
            }
            let v = self.normalize(parsed);
            self.publish(cx, uid, v, true);
        }
        self.end_edit(cx);
    }

    /// Apply a `name min..max step s` doc-channel hint to the scrubber:
    /// bounds show the fill bar and set the drag sweep, step sets the
    /// granularity. A hint, not a schema — typing past a bound expands it.
    pub fn set_hint(&mut self, min: Option<f64>, max: Option<f64>, step: Option<f64>) {
        if let (Some(a), Some(b)) = (min, max) {
            if b > a {
                self.min = a;
                self.max = b;
                self.show_fill = true;
                self.hint_bounds = true;
            }
        }
        if let Some(step) = step {
            if step > 0.0 {
                self.step = step;
            }
        }
    }

    fn cancel_drag(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.cancel_scope = None;
        if self.tracking {
            // The track holds no pointer of its own — the press is where it
            // always was — so putting the value back is the whole of it.
            self.tracking = false;
            let back = self.track_press_value;
            self.publish(cx, uid, back, false);
            self.animator_play(cx, ids!(hover.off));
        }
        if let Some(drag) = self.drag.take() {
            if drag.engaged {
                // Early cancel (Escape / right-click): the button is still
                // held, so the pin must be lifted explicitly.
                cx.unpin_pointer_capture();
                self.publish(cx, uid, drag.press_value, false);
            }
            self.animator_play(cx, ids!(hover.off));
        }
    }
}

impl Widget for FabValueInput {
    // The generic switch and the bridge's /snap `enabled` column both go
    // through these, so what they say is what the field does.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.set_enabled(cx, !disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        !self.enabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        // The fill claims "this range means something": only bounded fields
        // paint one.
        self.draw_bg.fill = if self.show_fill && self.max > self.min {
            (((self.value - self.min) / (self.max - self.min)) as f32).clamp(0.0, 1.0)
        } else {
            -1.0
        };
        self.draw_bg.disabled = if self.enabled { 0.0 } else { 1.0 };
        self.draw_bg.stepper = if self.track { 0.0 } else { 1.0 };
        if self.track {
            // The two outer columns, measured with the metrics the draw
            // below uses, so the zones a press is read against are the
            // columns the eye sees. A row with no name has no name column,
            // and the track begins at the row's own edge. Measured BEFORE
            // the quad is begun, because that is when its instance is
            // written — and it needs no width, which is the other reason
            // the fill's inset is carried in points.
            let fs = self.draw_text.text_style.font_size as f64;
            let name = if self.label.is_empty() {
                0.0
            } else {
                self.layout.padding.left + self.label.chars().count() as f64 * fs * 0.62 + 2.0
            };
            let number = self.number_column(cx);
            self.track_columns = (name, number);
            self.draw_bg.fill_pad_l = name as f32;
            self.draw_bg.fill_pad_r = number as f32;
            // The well is the column minus the row's own right padding, and
            // it sits exactly where the editor below will be drawn.
            self.draw_bg.num_box = (number - self.layout.padding.right) as f32;
            self.draw_bg.num_box_r = self.layout.padding.right as f32;
        } else {
            self.draw_bg.fill_pad_l = 0.0;
            self.draw_bg.fill_pad_r = 0.0;
            self.draw_bg.num_box = 0.0;
            self.draw_bg.num_box_r = 0.0;
        }
        self.draw_bg.begin(cx, walk, self.layout);
        if !self.label.is_empty() {
            // The label spans exactly the space the value does not need:
            // the value lands right-anchored; a tight row elides the label,
            // never the number.
            let row = cx.turtle().rect().size.x;
            let pad = self.layout.padding.left + self.layout.padding.right;
            let fs = self.draw_text.text_style.font_size as f64;
            // A track's readout is a fixed column, so the label takes the
            // whole of the rest: that is what puts the box hard against the
            // row's right padding and every row's track end on one line. A
            // scrub's value is drawn where it falls, so there the label
            // reserves what this number needs and no more.
            let value_reserve = if self.track {
                self.track_columns.1 - self.layout.padding.right - NUM_BOX_PAD_L
            } else {
                (self.format().chars().count() as f64 + 0.5) * fs * 0.72 + 6.0
            };
            let label_w = (row - pad - value_reserve).max(0.0);
            // A label that cannot fit is not drawn at all: a crushed "w"
            // renders as a stray dot beside the number.
            let needed = self.label.chars().count() as f64 * fs * 0.62 + 2.0;
            if label_w >= needed {
                let mut label_walk = Walk::fit();
                label_walk.width = Size::Fixed(label_w);
                self.draw_text
                    .draw_walk(cx, label_walk, Align::default(), &self.label);
            }
        }
        let mut iw = self.text_input.walk(cx);
        if self.track {
            // The editor sits inside the well, [`NUM_BOX_PAD_L`] in from its
            // left: a single-line field lays its text out at its natural
            // width and draws it from the left, so that inset is what gives
            // the first figure its air. Fixed, because a `Fill` editor would
            // stretch back over the track and swallow presses meant for it.
            let (_, number) = self.track_columns;
            iw.width = Size::Fixed(
                (number - self.layout.padding.right - NUM_BOX_PAD_L).max(2.0),
            );
        }
        if self.enabled {
            let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), iw);
        } else {
            // Off: the value is still there to read, in the label's ink at
            // half strength, where the editor would have put it.
            let text = self.format();
            let old = self.draw_text.color;
            self.draw_text.color = vec4(old.x, old.y, old.z, old.w * 0.5);
            self.draw_text.draw_walk(cx, iw, Align { x: 1.0, y: 0.5 }, &text);
            self.draw_text.color = old;
        }
        // The 3D-suite convention: stepper chevrons reveal on hover at the
        // field's edges — their zones (field_zone) exist regardless; the
        // glyphs only while the pointer is here and nothing is in flight.
        if self.enabled && self.hovered && !self.editing && self.drag.is_none() && !self.track {
            let rect = cx.turtle().rect();
            let fs = self.draw_text.text_style.font_size as f64;
            let y = rect.pos.y + (rect.size.y - fs * 1.5).max(0.0) * 0.5;
            let old = self.draw_text.color;
            self.draw_text.color = vec4(0.69, 0.69, 0.69, 0.9);
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + 3.0, y), "\u{2039}");
            self.draw_text.draw_abs(
                cx,
                dvec2(rect.pos.x + rect.size.x - 9.0, y),
                "\u{203a}",
            );
            self.draw_text.color = old;
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        // Off: nothing below answers. The animator still settles whatever
        // was in flight when the field went off.
        if !self.enabled {
            return;
        }
        if (self.editing || self.drag.is_some() || self.tracking)
            && crate::family_api::is_modal_dismissal(event)
        {
            self.cancel_drag(cx, uid);
            if self.editing {
                self.end_edit(cx);
            }
            return;
        }

        // Double-click = RESET, detected on the raw press so it works in
        // every state (the second press of a double-click lands while the
        // first click's text editor is already open — the editor claims
        // hits, so hit-testing can't see it). Coexistence: a single click
        // still opens the editor instantly (snappy); the second click
        // within the window converts that into end-edit + reset.
        //
        // A track row has none of this: its reset is a tap on the name, the
        // way the panel's sliders read it, and a double press on the track
        // is two presses that each landed a value — the second must not undo
        // the first.
        if let (Event::MouseDown(me), false) = (event, self.track) {
            // ...but only in the MIDDLE. The stepper arrows exist to be
            // clicked repeatedly, and two of those inside the double-click
            // window were being read as the reset gesture — nudge a value up
            // three times and it snapped back to its default on the way.
            let face = self.draw_bg.area().rect(cx);
            let on_middle = face.size.x > 0.0
                && matches!(
                    field_zone(me.abs.x - face.pos.x, face.size.x, face.size.y),
                    FieldZone::Middle
                );
            if me.button.is_primary()
                && on_middle
                && self.draw_bg.area().clipped_rect(cx).contains(me.abs)
            {
                if me.time - self.last_press_time < 0.4 {
                    self.last_press_time = 0.0;
                    if self.editing {
                        self.end_edit(cx);
                        cx.revert_key_focus();
                    }
                    self.drag = None;
                    self.cancel_scope = None;
                    cx.widget_action(uid, FabValueInputAction::Reset);
                    return;
                }
                self.last_press_time = me.time;
            }
        }

        // Ctrl+Wheel nudges by one step; a plain wheel keeps scrolling the
        // panel underneath. A track row takes the plain wheel too: it lives
        // in a popover with nothing behind it to scroll, and the wheel over
        // a row is how a hand asks for one unit.
        if let Event::Scroll(e) = event {
            if e.modifiers.control || e.modifiers.logo || self.track {
                if !e.handled_y.get()
                    && e.scroll.y.abs() > f64::EPSILON
                    && self.draw_bg.area().rect(cx).contains(e.abs)
                {
                    let direction = if e.scroll.y < 0.0 { 1.0 } else { -1.0 };
                    self.step_once(cx, uid, direction, e.modifiers.shift);
                    e.handled_y.set(true);
                }
            }
        }

        if self.editing
            && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
            && (matches!(event, Event::KeyDown(ke) if ke.key_code == KeyCode::Escape)
                || event.back_pressed())
        {
            self.end_edit(cx);
            cx.revert_key_focus();
            return;
        }

        // Escape, Back or a right-button press cancels an in-flight drag and
        // restores the pressed value.
        if self.drag.is_some() || self.tracking {
            match event {
                Event::KeyDown(ke)
                    if ke.key_code == KeyCode::Escape
                        && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s)) =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::BackPressed { .. }
                    if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                        && event.back_pressed() =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::MouseDown(me) if me.button.is_secondary() => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::WindowLostFocus(_) => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                _ => {}
            }
        }

        // The embedded input is a display until a click opens it: while it
        // is not editing it receives no events at all — otherwise it claims
        // the press for text selection and the drag never sees a move.
        if self.editing {
            // Focus ownership is the state boundary: if focus moved away
            // while an action was consumed elsewhere, commit and return to
            // the read-only display.
            let input_area = self.text_input.area();
            if input_area != Area::Empty && !cx.has_key_focus(input_area) {
                let text = self.text_input.text().to_string();
                self.commit_edit_text(cx, uid, &text);
                return;
            }
            for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    TextInputAction::KeyFocus => {
                        self.animator_play(cx, ids!(focus.on));
                    }
                    TextInputAction::KeyFocusLost => {
                        if self.editing {
                            let text = self.text_input.text().to_string();
                            self.commit_edit_text(cx, uid, &text);
                        }
                    }
                    TextInputAction::Returned(v, _) => {
                        if self.editing {
                            self.commit_edit_text(cx, uid, &v);
                            cx.revert_key_focus();
                        }
                    }
                    TextInputAction::Escaped => {
                        if self.editing
                            && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                        {
                            self.end_edit(cx);
                            cx.revert_key_focus();
                        }
                    }
                    _ => {}
                }
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) => {
                let rect = self.draw_bg.area().rect(cx);
                cx.set_cursor(self.cursor_at(fe.abs.x, rect));
                self.hovered = true;
                self.draw_bg.redraw(cx);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOver(fe) => {
                if self.drag.is_none() && !self.tracking && !self.editing {
                    let rect = self.draw_bg.area().rect(cx);
                    cx.set_cursor(self.cursor_at(fe.abs.x, rect));
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() && !self.editing && self.track => {
                // The track's press is the whole gesture's beginning AND its
                // first move: the value is where the pointer is, at once.
                // The two outer columns are not track — the name is the
                // reset and the number is the way in to typing — and a press
                // on either changes nothing until the release says what it
                // was.
                self.track_press_value = self.value;
                self.cancel_scope = Some(self.begin_cancel_scope(cx));
                cx.set_key_focus(self.draw_bg.area());
                self.animator_play(cx, ids!(hover.down));
                if self.track_zone(cx, fe.abs.x) == SliderZone::Track {
                    self.tracking = true;
                    self.drag_moves = 0;
                    self.drag_publishes = 0;
                    let v = self.track_value(cx, fe.abs.x);
                    self.publish(cx, uid, v, false);
                }
            }
            Hit::FingerMove(fe) if self.tracking => {
                // The arrows stay for the whole pull. A captured area is sent
                // moves and not hovers, so nothing else would set a cursor
                // here and the one from the hover would simply go stale.
                cx.set_cursor(MouseCursor::EwResize);
                self.drag_moves += 1;
                let v = self.track_value(cx, fe.abs.x);
                let before = self.value;
                self.publish(cx, uid, v, false);
                if (self.value - before).abs() > f64::EPSILON {
                    self.drag_publishes += 1;
                }
            }
            Hit::FingerUp(fe) if self.track => {
                self.cancel_scope = None;
                if self.tracking {
                    self.tracking = false;
                    log!(
                        "TRACK stats: finger_moves={} publishes={}",
                        self.drag_moves,
                        self.drag_publishes
                    );
                    cx.widget_action(uid, FabValueInputAction::Ended(self.value));
                } else {
                    // Nothing moved, so the column the release is over says
                    // what the press meant.
                    match self.track_zone(cx, fe.abs.x) {
                        SliderZone::Label => {
                            cx.widget_action(uid, FabValueInputAction::Reset);
                        }
                        SliderZone::Readout => self.begin_edit(cx),
                        SliderZone::Track => {}
                    }
                }
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            // The keyboard, once a press has left the focus here: one step
            // an arrow, a tenth of one with Shift, and Return opens the
            // editor without reaching for the number with the pointer.
            Hit::KeyDown(ke) if self.track && !self.editing => match ke.key_code {
                KeyCode::ArrowLeft | KeyCode::ArrowDown => {
                    self.step_once(cx, uid, -1.0, ke.modifiers.shift)
                }
                KeyCode::ArrowRight | KeyCode::ArrowUp => {
                    self.step_once(cx, uid, 1.0, ke.modifiers.shift)
                }
                KeyCode::ReturnKey => self.begin_edit(cx),
                _ => {}
            },
            Hit::FingerDown(fe) if fe.device.is_primary_hit() && !self.editing => {
                let rect = self.draw_bg.area().rect(cx);
                // Press changes nothing: it only arms.
                self.drag = Some(DragState {
                    press_x: fe.abs.x,
                    press_value: self.value,
                    width: rect.size.x,
                    engaged: false,
                    anchor: DragAnchor {
                        x: fe.abs.x,
                        value: self.value,
                    },
                    shift: fe.modifiers.shift,
                    raw_value: self.value,
                });
                // The next event may already be Escape; ownership is captured
                // before dispatch, so the scope must exist before this returns.
                self.cancel_scope = Some(self.begin_cancel_scope(cx));
                self.animator_play(cx, ids!(hover.down));
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag else {
                    return;
                };
                if !drag.engaged {
                    if (fe.abs.x - drag.press_x).abs() < DRAG_THRESHOLD {
                        return;
                    }
                    // Engage at the pointer, discarding the threshold
                    // distance: the first dragged pixel is a small change.
                    drag.engaged = true;
                    drag.anchor = reanchor(self.value, fe.abs.x);
                    drag.raw_value = self.value;
                    // The pointer pins where the press happened: hidden,
                    // infinite drag range, restored in place on release.
                    // Engaged only now, at the threshold — a plain click
                    // never touches the cursor. The pin rides on this
                    // widget's finger capture; the hardware button-up
                    // releases both automatically.
                    cx.pin_pointer_capture();
                    self.drag_moves = 0;
                    self.drag_publishes = 0;
                }
                self.drag_moves += 1;
                let mods = cx.keyboard.modifiers();
                if mods.shift != drag.shift {
                    // A modifier change re-anchors: the value holds still,
                    // only the rate changes from here.
                    drag.shift = mods.shift;
                    drag.anchor = reanchor(drag.raw_value, fe.abs.x);
                }
                let params = self.params();
                let (publish, anchor) = drag_map(
                    &params,
                    drag.anchor,
                    fe.abs.x,
                    drag.width,
                    mods.shift,
                    mods.control | mods.logo,
                );
                drag.raw_value = anchor.value
                    + (fe.abs.x - anchor.x) * drag_rate(&params, drag.width, mods.shift);
                if params.has_range() && !params.wrap {
                    drag.raw_value = drag.raw_value.clamp(params.min, params.max);
                }
                drag.anchor = anchor;
                self.drag = Some(drag);
                let v = self.normalize(publish);
                self.drag_publishes += 1;
                self.publish(cx, uid, v, false);
                // Hold the pin against quiet OS re-association drops.
                cx.repin_mouse_pointer();
            }
            Hit::FingerUp(fe) => {
                self.cancel_scope = None;
                let Some(drag) = self.drag.take() else {
                    return;
                };
                if drag.engaged {
                    // The pin released with the capture on the way in; the
                    // action is all that is left to send.
                    log!(
                        "SCRUB stats: finger_moves={} publishes={}",
                        self.drag_moves,
                        self.drag_publishes
                    );
                    cx.widget_action(uid, FabValueInputAction::Ended(self.value));
                } else if !fe.cancelled {
                    // A click (a press taken away is none). The zone at release decides: arrows step,
                    // the middle opens text entry with the value selected.
                    let rect = self.draw_bg.area().rect(cx);
                    let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                    match zone {
                        FieldZone::Decrement => self.step_once(cx, uid, -1.0, fe.modifiers.shift),
                        FieldZone::Increment => self.step_once(cx, uid, 1.0, fe.modifiers.shift),
                        FieldZone::Middle => self.begin_edit(cx),
                    }
                }
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => {}
        }
    }
}

impl FabValueInputRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabValueInputAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        ended_value(actions, self.widget_uid())
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.value())
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, enabled);
        }
    }

    pub fn enabled(&self) -> bool {
        self.borrow().map_or(true, |i| i.enabled())
    }
}

fn ended_value(actions: &Actions, uid: WidgetUid) -> Option<f64> {
    for action in actions.filter_widget_actions_cast::<FabValueInputAction>(uid) {
        if let FabValueInputAction::Ended(v) = action {
            return Some(v);
        }
    }
    None
}

// ===========================================================================
// FabSlider — a horizontal track whose thumb goes where the pointer is. The
// travel law is pure and the shader is handed the same numbers the hit test
// measures with, so what is drawn is what is grabbable.
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabSlider {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    /// Where the value sits along the travel, 0..1.
    #[live]
    travel: f32,
    /// The name's column and the number's column, in pixels. Rust measures
    /// the track between them and the shader draws it between the same two.
    #[live]
    label_px: f32,
    #[live]
    readout_px: f32,
    #[live]
    thumb_px: f32,
    #[live]
    inset_px: f32,
}

#[derive(Clone, Debug, Default)]
pub enum FabSliderAction {
    /// The value moved. Live under a drag, and under every arrow key
    /// including the repeats the keyboard sends while one is held down.
    Changed(f64),
    /// The gesture that was moving the value is over, and this is the value
    /// it came to rest on. A commit: a host that rate-limits `Changed` is
    /// meant to spend this one at once.
    ///
    /// At most ONE per gesture, and never none. A mouse release ends a drag
    /// or a tap on the name; a deliberate key press ends itself, so a single
    /// arrow and a jump to a stop both land without waiting; and the release
    /// of a HELD key ends the run of repeats it sent, which is what keeps a
    /// second of held arrow down to two of these instead of thirty. See
    /// `key_step`.
    ///
    /// Where a run ends without its release -- the keyboard moving on, the
    /// row being switched off, the window losing the focus mid-key -- what
    /// the run owes is paid there instead. A host that rate-limits `Changed`
    /// and spends this one can hold it to that.
    Ended(f64),
    /// A click on the name: the row is back at zero and the host should take
    /// it out of whatever it feeds.
    Reset,
    #[default]
    None,
}

/// The travel one slider carries into a press: the range it spans, the
/// detent it lands on, and the two pixel sizes the shader is handed.
///
/// The mapping is ABSOLUTE — the value is where the pointer is, not how far
/// it has come — which is the whole difference between this and the number
/// field's scrub above. Kept here, entire and with no `Cx` anywhere, so the
/// law the hit test uses is the law the tests read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderTravel {
    pub min: f64,
    pub max: f64,
    /// The detent a value lands on; `0` is continuous.
    pub step: f64,
    /// Thumb width and the track's inset, in pixels.
    pub thumb: f64,
    pub inset: f64,
}

impl SliderTravel {
    /// The two stops in order, whichever way round they were written.
    pub fn stops(&self) -> (f64, f64) {
        (self.min.min(self.max), self.min.max(self.max))
    }

    /// A value onto the detent and inside the stops. Quantising walks a
    /// value off the end of a range that is not a whole number of steps, so
    /// the clamp comes second and not first.
    pub fn settle(&self, v: f64) -> f64 {
        let (lo, hi) = self.stops();
        let v = if self.step > 0.0 {
            lo + ((v - lo) / self.step).round() * self.step
        } else {
            v
        };
        v.clamp(lo, hi)
    }

    /// Inside the stops, and nowhere near the detent.
    ///
    /// The detent belongs to the hand: it is where a drag lands and how far
    /// an arrow key carries. A number that arrives from a caller is somebody
    /// else's arithmetic and is none of its business -- a row of weights
    /// sharing a hundred parts stops adding up the moment three of them are
    /// rounded on the way in.
    pub fn contain(&self, v: f64) -> f64 {
        let (lo, hi) = self.stops();
        v.clamp(lo, hi)
    }

    /// Where a value sits along the travel, 0..1.
    pub fn travel(&self, v: f64) -> f64 {
        let (lo, hi) = self.stops();
        let span = hi - lo;
        if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((v - lo) / span).clamp(0.0, 1.0)
        }
    }

    /// What the thumb's CENTRE runs over: the track less its inset at both
    /// ends and the thumb's own width, so neither stop hangs off the end.
    pub fn travel_px(&self, width: f64) -> f64 {
        (width - self.inset * 2.0 - self.thumb).max(1.0)
    }

    /// The value at an x offset from the left edge of the TRACK.
    pub fn value_at(&self, x: f64, width: f64) -> f64 {
        let t = ((x - self.inset - self.thumb * 0.5) / self.travel_px(width)).clamp(0.0, 1.0);
        let (lo, hi) = self.stops();
        self.settle(lo + t * (hi - lo))
    }

    /// The x of the thumb's centre, in the same frame as `value_at`.
    pub fn thumb_x(&self, v: f64, width: f64) -> f64 {
        self.inset + self.thumb * 0.5 + self.travel(v) * self.travel_px(width)
    }
}

/// The three columns of the row. Only the track answers a press with a
/// value: the number is there to be read, and the name is the reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderZone {
    Label,
    Track,
    Readout,
}

/// Which column a pointer at `x` (from the row's left edge) is over, for a
/// row of `width` whose outer columns are `label_px` and `readout_px` wide.
pub fn slider_zone(x: f64, width: f64, label_px: f64, readout_px: f64) -> SliderZone {
    if x < label_px {
        SliderZone::Label
    } else if x > width - readout_px {
        SliderZone::Readout
    } else {
        SliderZone::Track
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabSlider {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawFabSlider,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_value: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    label: String,
    /// The name's column and the number's, in points. Fixed rather than
    /// fitted: an equalizer is a stack of these, and the tracks have to
    /// begin in the same place all the way down the column.
    #[live(92.0)]
    label_width: f64,
    #[live(34.0)]
    readout_width: f64,
    #[live]
    min: f64,
    #[live(100.0)]
    max: f64,
    /// The arrow-key increment, and the detent a drag lands on.
    #[live(1.0)]
    step: f64,
    /// Shift+arrow. Coarse, where Shift on the number field above is fine:
    /// one arrow on a 0..100 track is already the small gesture, so Shift
    /// has nowhere to go but up.
    #[live(10.0)]
    big_step: f64,
    #[live(0)]
    precision: usize,
    #[live]
    unit: String,
    #[live]
    value: f64,
    #[live(12.0)]
    thumb_size: f64,
    #[live(2.0)]
    track_inset: f64,
    /// Off: the row shows dimmed and nothing answers.
    #[live(true)]
    enabled: bool,

    /// Held for the length of a gesture, so Escape and a modal's dismissal
    /// reach this control rather than whatever it is sitting in.
    #[rust]
    cancel_scope: Option<CancelScope>,
    #[rust]
    dragging: bool,
    /// What the value was when the press landed, for a cancel to put back.
    #[rust]
    press_value: f64,
    /// The number this row PRINTS, where that is not the number it holds.
    ///
    /// A column that is read as a whole -- shares of a total, rounded over
    /// the column rather than a row at a time -- has a number for the
    /// readout that is nobody's own value, and can be a part or two from it.
    /// Kept apart so that it stops at the text: the thumb stands on `value`,
    /// an arrow steps from `value`, and what a gesture reports is `value`.
    /// Cleared by anything that moves the row, because the number a hand has
    /// just set is the row's own, and a share worked out for the value
    /// before it would print as a lie under a moving thumb.
    #[rust]
    readout: Option<f64>,
    #[rust]
    hovered: bool,
    /// A keyboard run owes a commit: an arrow has moved the row since the
    /// last one went out, and the release that will end the run has not
    /// arrived yet.
    #[rust]
    key_commit_due: bool,
    /// The name's own box. The face is one area and it takes the press; this
    /// is only ever asked whether a tap FINISHED on the word, and never
    /// asked for a hit of its own — a second area over the same press is
    /// what once left the stock Slider unable to be dragged from its legend.
    #[rust]
    label_area: Area,
}

impl FabSlider {
    fn travel(&self) -> SliderTravel {
        SliderTravel {
            min: self.min,
            max: self.max,
            step: self.step,
            thumb: self.thumb_size,
            inset: self.track_inset,
        }
    }

    /// The row's outer two columns, padding included, in the face's frame.
    fn columns(&self) -> (f64, f64) {
        (
            self.layout.padding.left + self.label_width,
            self.layout.padding.right + self.readout_width,
        )
    }

    /// The value the pointer is naming right now.
    fn value_at_pointer(&self, cx: &Cx, abs_x: f64) -> f64 {
        let face = self.draw_bg.area().rect(cx);
        let (label_px, readout_px) = self.columns();
        let width = (face.size.x - label_px - readout_px).max(1.0);
        self.travel().value_at(abs_x - face.pos.x - label_px, width)
    }

    /// Which column a press at `abs_x` landed in.
    fn zone_at(&self, cx: &Cx, abs_x: f64) -> SliderZone {
        let face = self.draw_bg.area().rect(cx);
        let (label_px, readout_px) = self.columns();
        slider_zone(abs_x - face.pos.x, face.size.x, label_px, readout_px)
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// A value pushed in from outside, held EXACTLY as it was handed over.
    ///
    /// What is guaranteed: the number a host sets is the number `value()`
    /// reads back, the number the next arrow key steps from, and the number
    /// the thumb stands on -- clamped to the stops and to nothing else. The
    /// detent is the HAND's grid, the thing a drag lands on and the thing an
    /// arrow steps by; it is never a filter on the host's own arithmetic. A
    /// `step` of 1 is what a weight row wants under a finger, and it used to
    /// round what the host had worked out as well: four rows splitting a
    /// hundred parts come down to 0 / 37.5 / 37.5 / 25, and rows that stored
    /// 38 read back 101% under a legend promising a hundred -- and then
    /// handed the next arrow key an origin nobody had set.
    ///
    /// What is NOT guaranteed: that the number on screen is the number held.
    /// The readout is `precision` places wide and rounds to fit, so a row
    /// holding 37.5 at `precision: 0` prints 38. A host that needs the column
    /// to READ as a hundred as well as sum to one is not to round before it
    /// sets -- that stores the rounded number, with every consequence in the
    /// paragraph above -- but to say both numbers at once. See
    /// [`FabSlider::set_value_and_readout`].
    ///
    /// Refused mid-drag: a host answering late must not argue with the hand
    /// that is on the thumb.
    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        if self.dragging {
            return;
        }
        self.hold(cx, v, None);
    }

    /// The number the row HOLDS and the number it PRINTS, handed over
    /// together.
    ///
    /// For the host whose readout is not its own arithmetic. A column of
    /// shares of a total is rounded over the whole column, so what one row
    /// shows is a part or two off what it carries; a host with nowhere to
    /// put that but the value ended up storing it, and then the row stepped
    /// from it -- an arrow on the largest share of a mix walked the weight
    /// DOWN, because the largest share is the one the column takes its
    /// rounding out of.
    ///
    /// So they arrive together and part company here: `v` is the whole of
    /// what the row holds, steps from and reports, and `readout` reaches
    /// nothing but the text. They are one call because they are one fact --
    /// a row left printing the share of a mix it no longer holds is the same
    /// fault the other way round.
    ///
    /// Refused mid-drag, for the reason above it.
    pub fn set_value_and_readout(&mut self, cx: &mut Cx, v: f64, readout: f64) {
        if self.dragging {
            return;
        }
        self.hold(cx, v, Some(readout));
    }

    /// What the row prints: what a host said to print, or what the row holds.
    pub fn readout(&self) -> f64 {
        self.readout.unwrap_or(self.value)
    }

    /// The one door a host's number comes in by. The redraw hangs off the
    /// PAIR, because a column can be re-rounded by a move on another row
    /// without this one's value changing by anything at all.
    fn hold(&mut self, cx: &mut Cx, v: f64, readout: Option<f64>) {
        let v = self.travel().contain(v);
        if (v - self.value).abs() > f64::EPSILON || readout != self.readout {
            self.value = v;
            self.readout = readout;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// The row's name, for a host filling a list of them.
    pub fn set_label(&mut self, cx: &mut Cx, text: &str) {
        if self.label != text {
            self.label = text.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    /// The two stops, for a row whose range is not a constant: a setting
    /// whose limits are worked out from other settings, where a track drawn
    /// over a fixed range would offer numbers the host can only clamp away.
    /// What the row holds is brought inside the new stops at once, so the
    /// thumb never stands off the end of its own track.
    ///
    /// Refused mid-drag, for [`FabSlider::set_value`]'s reason: the travel
    /// under a moving hand is not moved out from under it.
    pub fn set_range(&mut self, cx: &mut Cx, min: f64, max: f64) {
        if self.dragging || !(max > min) {
            return;
        }
        if (self.min - min).abs() > f64::EPSILON || (self.max - max).abs() > f64::EPSILON {
            self.min = min;
            self.max = max;
            self.value = self.travel().contain(self.value);
            self.draw_bg.redraw(cx);
        }
    }

    /// The two stops as they stand.
    pub fn range(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    /// Where the row's NAME was drawn. The face is one area and the name is
    /// a box inside it, so a host lining a column of rows up on their names
    /// -- a panel that puts a control of its own on a row beside sliders --
    /// has something to line up on. Never hit-tested: see `label_area` for
    /// why a second area over the same press is a slider nobody can drag
    /// from its legend.
    pub fn label_rect(&self, cx: &Cx) -> Rect {
        self.label_area.rect(cx)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switching off mid-gesture ends the gesture first: the pointer is let
    /// go, the value the press found is put back, and a keyboard run that has
    /// not committed yet pays up. A row nothing can reach will never see the
    /// release that would otherwise have ended it.
    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            let uid = self.widget_uid();
            self.cancel_drag(cx, uid);
            self.end_key_run(cx, uid);
            self.hovered = false;
        }
        self.draw_bg.redraw(cx);
    }

    /// Answers whether the value actually moved, which is what tells a key
    /// press whether it has anything left to commit.
    fn publish(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, ended: bool) -> bool {
        let moved = (v - self.value).abs() > f64::EPSILON;
        if moved {
            self.value = v;
            // The hand's number is the row's own, so whatever a host had it
            // printing instead goes here: a share worked out for the weight
            // before this one would sit under a thumb that has left it.
            self.readout = None;
            self.draw_bg.redraw(cx);
            cx.widget_action(uid, FabSliderAction::Changed(self.value));
        }
        if ended {
            cx.widget_action(uid, FabSliderAction::Ended(self.value));
        }
        moved
    }

    /// One key's worth of movement, and whether it ends anything.
    ///
    /// A fresh press is a deliberate act and commits where it lands, so one
    /// arrow and a jump to a stop both land at once. What the keyboard sends
    /// AFTER it is not a second gesture, it is the first one still running:
    /// a repeat says only that the value moved, and the single commit the run
    /// owes is paid at the release. An arrow held for a second is therefore
    /// two commits rather than thirty -- which is the difference that matters
    /// on the other side, where a commit is a host dropping everything to
    /// install what it was handed. A drag is bounded there already, by the
    /// interval behind its moves; a held key had been going round it.
    fn key_step(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, repeat: bool) {
        let moved = self.publish(cx, uid, v, !repeat);
        if repeat {
            // A repeat that landed nowhere new -- an arrow held against a
            // stop -- owes nothing of its own, and cancels nothing already
            // owed by the repeats before it.
            self.key_commit_due |= moved;
        } else {
            self.key_commit_due = false;
        }
    }

    /// The commit a keyboard run still owes, paid at the release -- or at
    /// whatever ends the run before one arrives. Unpaid, the last value a
    /// hand nudged sits on the row and reaches nobody.
    fn end_key_run(&mut self, cx: &mut Cx, uid: WidgetUid) {
        if self.key_commit_due {
            self.key_commit_due = false;
            cx.widget_action(uid, FabSliderAction::Ended(self.value));
        }
    }

    fn nudge(&mut self, cx: &mut Cx, uid: WidgetUid, direction: f64, big: bool, repeat: bool) {
        let step = if big { self.big_step } else { self.step };
        // A continuous track still has to move by something; a hundredth of
        // the range is the arrow-key equivalent of one percent.
        let step = if step > 0.0 {
            step
        } else {
            (self.max - self.min).abs() * 0.01
        };
        // One step from where the row actually stands. Settling the sum onto
        // the detent's grid reads the origin off the grid first, so an arrow
        // pressed on a row set to 37.5 published 39 -- a step and a half the
        // hand never asked for.
        let v = self.travel().contain(self.value + direction * step);
        self.key_step(cx, uid, v, repeat);
    }

    /// ZERO, and not the range's floor nor a `default:` the way the stock
    /// Slider's title click goes. "This one counts for nothing" is the
    /// gesture a row of these needs most, and it is the same number whichever
    /// row it is asked of; a range that never reaches zero takes its nearest
    /// stop instead.
    fn reset(&mut self, cx: &mut Cx, uid: WidgetUid) {
        let v = self.travel().settle(0.0);
        self.publish(cx, uid, v, false);
        cx.widget_action(uid, FabSliderAction::Reset);
    }

    fn cancel_drag(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.cancel_scope = None;
        if self.dragging {
            self.dragging = false;
            let back = self.press_value;
            self.publish(cx, uid, back, false);
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for FabSlider {
    // The generic switch and the bridge's `enabled` column both come through
    // here, so what they say is what the row does.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.set_enabled(cx, !disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        !self.enabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let (label_px, readout_px) = self.columns();
        self.draw_bg.label_px = label_px as f32;
        self.draw_bg.readout_px = readout_px as f32;
        self.draw_bg.thumb_px = self.thumb_size as f32;
        self.draw_bg.inset_px = self.track_inset as f32;
        self.draw_bg.travel = self.travel().travel(self.value) as f32;
        self.draw_bg.hover = if self.hovered && self.enabled { 1.0 } else { 0.0 };
        self.draw_bg.down = if self.dragging { 1.0 } else { 0.0 };
        self.draw_bg.focus = if cx.cx.cx.has_key_focus(self.draw_bg.area()) {
            1.0
        } else {
            0.0
        };
        self.draw_bg.disabled = if self.enabled { 0.0 } else { 1.0 };
        self.draw_bg.begin(cx, walk, self.layout);

        // The name gets a turtle of its own, because the reset gesture needs
        // a box to ask about at the release. It is a measurement and not a
        // hit target: the face above is the only thing that takes a press.
        let label_walk = Walk::new(Size::Fixed(self.label_width), Size::fill());
        cx.begin_turtle(label_walk, Layout::default());
        if !self.label.is_empty() {
            self.draw_label
                .draw_walk(cx, label_walk, Align { x: 0.0, y: 0.5 }, &self.label);
        }
        cx.end_turtle_with_area(&mut self.label_area);

        // The track itself is painted by the face underneath; this only
        // claims the width, and claims exactly the width the hit test and
        // the shader both measure, so the number lands where it belongs.
        let row = cx.turtle().rect().size.x;
        let track_w = (row - label_px - readout_px).max(1.0);
        let _ = cx.walk_turtle(Walk::new(Size::Fixed(track_w), Size::fill()));

        let text = crate::slider::format_readout(self.readout(), self.precision, &self.unit);
        let value_walk = Walk::new(Size::Fixed(self.readout_width), Size::fill());
        self.draw_value
            .draw_walk(cx, value_walk, Align { x: 1.0, y: 0.5 }, &text);

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        // Off: nothing below answers.
        if !self.enabled {
            return;
        }
        if self.dragging && crate::family_api::is_modal_dismissal(event) {
            self.cancel_drag(cx, uid);
            return;
        }
        // The window going away ends whatever this row was in the middle of,
        // by either hand: the press is let go and the value it found put
        // back, and a keyboard run pays the commit it owes. Whatever would
        // have ended either one -- the release, the key coming up -- is
        // going to the app that took the focus. `Hit::KeyFocusLost` does not
        // stand in for it: the focus INSIDE this app has not moved, so a held
        // arrow and an alt-tab left the last value a hand nudged sitting on
        // the row with nothing having been told about it. Ordered as
        // `set_enabled` orders the same pair, so that what is committed is
        // the value the row is left standing on.
        if let Event::WindowLostFocus(_) = event {
            self.cancel_drag(cx, uid);
            self.end_key_run(cx, uid);
            return;
        }
        // Escape, Back or the right button puts back the value the press
        // found.
        if self.dragging {
            match event {
                Event::KeyDown(ke)
                    if ke.key_code == KeyCode::Escape
                        && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s)) =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::BackPressed { .. }
                    if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                        && event.back_pressed() =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::MouseDown(me) if me.button.is_secondary() => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                _ => {}
            }
        }

        // One area, asked plainly: no sweep area and no capture overload,
        // so the press this takes is the press nothing else is holding. And
        // no wheel arm below, deliberately — the panel these sit in scrolls,
        // and a row that ate the wheel would be a row you could not get past.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(match self.zone_at(cx, fe.abs.x) {
                    SliderZone::Track => MouseCursor::Grab,
                    SliderZone::Label => MouseCursor::Hand,
                    SliderZone::Readout => MouseCursor::Default,
                });
                if !self.hovered {
                    self.hovered = true;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.press_value = self.value;
                // `hits` took the mouse on the way in and holds it until the
                // release, which is what a scroller around this control asks
                // the capture list about before it drags its own content.
                self.cancel_scope = Some(self.begin_cancel_scope(cx));
                if self.zone_at(cx, fe.abs.x) == SliderZone::Track {
                    // A track is not a scrub: the thumb goes where the
                    // finger is, on the press itself, and stays under it.
                    self.dragging = true;
                    let v = self.value_at_pointer(cx, fe.abs.x);
                    self.publish(cx, uid, v, false);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    let v = self.value_at_pointer(cx, fe.abs.x);
                    self.publish(cx, uid, v, false);
                }
            }
            Hit::FingerUp(fe) => {
                self.cancel_scope = None;
                // A tap that began AND stayed on the name. `was_tap` rather
                // than `is_over`, so a drag that merely started there ends as
                // the drag it was; and it is the press that is asked about,
                // so the box is tested against `abs_start`.
                let tapped_label = fe.was_tap()
                    && !self.label.is_empty()
                    && self.label_area.rect(cx).contains(fe.abs_start);
                if tapped_label {
                    self.reset(cx, uid);
                }
                if self.dragging || tapped_label {
                    // After the reset and never before it: the commit is what
                    // a host writes down, and it has to carry the zero.
                    cx.widget_action(uid, FabSliderAction::Ended(self.value));
                }
                self.dragging = false;
                self.draw_bg.redraw(cx);
            }
            // Ctrl and Cmd are the accelerator space, and that space belongs
            // to whatever this row is sitting in: Ctrl+Home is the panel going
            // to its top, Cmd+Arrow is the window manager's. A focused row
            // that nudged on either would break those shortcuts silently, and
            // only while the keyboard happened to be resting on it. Shift is
            // this control's own -- the coarse step -- and Alt is left
            // unclaimed, which is where a fine step would go.
            Hit::KeyDown(ke) if !ke.modifiers.control && !ke.modifiers.logo => {
                match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => {
                        self.nudge(cx, uid, -1.0, ke.modifiers.shift, ke.is_repeat)
                    }
                    KeyCode::ArrowRight | KeyCode::ArrowUp => {
                        self.nudge(cx, uid, 1.0, ke.modifiers.shift, ke.is_repeat)
                    }
                    // Absolute, both of them: the first press names the stop
                    // and every repeat after it names the same stop, so a key
                    // held against the end of its own travel goes quiet.
                    KeyCode::Home => {
                        let v = self.travel().stops().0;
                        self.key_step(cx, uid, v, ke.is_repeat);
                    }
                    KeyCode::End => {
                        let v = self.travel().stops().1;
                        self.key_step(cx, uid, v, ke.is_repeat);
                    }
                    _ => {}
                }
            }
            // Letting go of the key that was driving the value ends the
            // gesture, the way letting go of the mouse button does.
            Hit::KeyUp(ke)
                if matches!(
                    ke.key_code,
                    KeyCode::ArrowLeft
                        | KeyCode::ArrowRight
                        | KeyCode::ArrowUp
                        | KeyCode::ArrowDown
                        | KeyCode::Home
                        | KeyCode::End
                ) =>
            {
                self.end_key_run(cx, uid);
            }
            Hit::KeyFocus(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                // The keyboard has gone elsewhere and the release will go with
                // it, so what the run owes is paid here or never.
                self.end_key_run(cx, uid);
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }
}

impl FabSliderRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabSliderAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        slider_ended_value(actions, self.widget_uid())
    }

    /// Was the name clicked? The value is already back at zero; this is the
    /// host's cue to drop the row from whatever ledger it keeps.
    pub fn was_reset(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions_cast::<FabSliderAction>(self.widget_uid())
            .any(|action| matches!(action, FabSliderAction::Reset))
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    /// See [`FabSlider::set_value_and_readout`]: what the row holds and what
    /// it prints, for a host whose column is rounded over the column.
    pub fn set_value_and_readout(&self, cx: &mut Cx, v: f64, readout: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value_and_readout(cx, v, readout);
        }
    }

    /// See [`FabSlider::set_range`].
    pub fn set_range(&self, cx: &mut Cx, min: f64, max: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_range(cx, min, max);
        }
    }

    pub fn range(&self) -> (f64, f64) {
        self.borrow().map_or((0.0, 0.0), |i| i.range())
    }

    pub fn value(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.value())
    }

    /// What the row prints, which is what it holds unless a host said
    /// otherwise.
    pub fn readout(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.readout())
    }

    pub fn set_label(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_label(cx, text);
        }
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, enabled);
        }
    }

    pub fn enabled(&self) -> bool {
        self.borrow().map_or(true, |i| i.enabled())
    }
}

/// `Changed` comes before `Ended` in the same buffer and
/// `find_widget_action` answers with the first of them, so the commit has to
/// be looked for rather than found.
fn slider_ended_value(actions: &Actions, uid: WidgetUid) -> Option<f64> {
    for action in actions.filter_widget_actions_cast::<FabSliderAction>(uid) {
        if let FabSliderAction::Ended(v) = action {
            return Some(v);
        }
    }
    None
}

// ===========================================================================
// FabKnob — the slider's number on a dial, for where a row is too much room:
// a cell of a matrix. It holds, steps and reports exactly as the slider does,
// so a panel treats the two alike; what differs is the gesture. A dial has no
// track to press on, so the value is how far the pointer has COME and not
// where it is, and the law of that is pure for the same reason the slider's
// is.
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabKnob {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    /// Where the value sits along the sweep, 0..1. Nought is drawn as OFF.
    #[live]
    travel: f32,
    /// The name's row and the number's, in pixels, either of them nought
    /// where there is no such row. The shader finds the face from these two
    /// and its own box, by the arithmetic of [`knob_face`].
    #[live]
    label_px: f32,
    #[live]
    readout_px: f32,
}

#[derive(Clone, Debug, Default)]
pub enum FabKnobAction {
    /// The value moved. Live under a drag, under the wheel, and under every
    /// arrow key including the repeats the keyboard sends while one is held.
    Changed(f64),
    /// The gesture that was moving the value is over, and this is the value
    /// it came to rest on. A commit, as [`FabSliderAction::Ended`] is one: a
    /// host that rate-limits `Changed` is meant to spend this one at once.
    ///
    /// At most ONE per gesture. A release ends a drag; a deliberate key press
    /// ends itself and the release of a HELD key ends the run of repeats it
    /// sent; a spin of the wheel ends when the wheel has been still for
    /// [`KNOB_WHEEL_SETTLE`]; a double click ends itself. Where a run ends
    /// without its ending -- the keyboard moving on, the knob being switched
    /// off, the window losing the focus -- what it owes is paid there.
    ///
    /// And NONE for a press that moved nothing. A knob is pressed to be given
    /// the keyboard far more often than a track is, there are a hundred of
    /// them on the panel this was built for, and a commit is a host dropping
    /// everything to install what it was handed: a click that changed nothing
    /// has nothing to install.
    Ended(f64),
    /// A double click: the knob is back at nought and the host should take it
    /// out of whatever it feeds. The slider's reset is a click on its name,
    /// and a knob in a matrix has no name to click.
    Reset,
    #[default]
    None,
}

/// How long the wheel has to have been still before a spin of it counts as
/// over, in seconds. One notch is not a gesture: a hand spins a wheel through
/// five or ten of them, and a commit for each is the held arrow's thirty
/// commits by another door.
pub const KNOB_WHEEL_SETTLE: f64 = 0.35;

/// How far a press has to travel before it is a drag, in points. The number
/// field's threshold and for the number field's reason: a knob is clicked to
/// be given the keyboard, and a careless click must not nudge the value it
/// was only meant to select.
pub const KNOB_DRAG_SLOP: f64 = 3.0;

/// The turn one knob carries into a gesture: the range it spans, the detent
/// it lands on, and how far a pointer travels to cross the whole of it.
///
/// The mapping is RELATIVE -- the value is how far the pointer has come since
/// the press, not where it is -- which is the whole difference between this
/// and [`SliderTravel`]. A dial drawn 28 across has no room for an absolute
/// law: a quarter of a turn would be eleven pixels. So the travel is a number
/// of its own and the same for every size of knob, and a hand that has
/// learned the rate on one has it on all of them.
///
/// Kept here, entire and with no `Cx` anywhere, so the law the drag uses is
/// the law the tests read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnobTurn {
    pub min: f64,
    pub max: f64,
    /// The detent a value lands on; `0` is continuous.
    pub step: f64,
    /// The pointer travel that covers the whole range, in points.
    pub travel: f64,
}

impl KnobTurn {
    /// The stops, the detent and the containment are the SLIDER's, borrowed
    /// whole rather than written again: a number means the same thing in a
    /// cell of a matrix as in a row of sliders, and a host that offers both
    /// must not have the two disagree about what a hand asked for.
    fn law(&self) -> SliderTravel {
        SliderTravel {
            min: self.min,
            max: self.max,
            step: self.step,
            thumb: 0.0,
            inset: 0.0,
        }
    }

    /// The two stops in order, whichever way round they were written.
    pub fn stops(&self) -> (f64, f64) {
        self.law().stops()
    }

    /// A value onto the detent and inside the stops: where a HAND lands.
    pub fn settle(&self, v: f64) -> f64 {
        self.law().settle(v)
    }

    /// Inside the stops and nowhere near the detent: where a HOST's number
    /// is held. See [`SliderTravel::contain`].
    pub fn contain(&self, v: f64) -> f64 {
        self.law().contain(v)
    }

    /// Where a value sits along the sweep, 0..1.
    pub fn travel(&self, v: f64) -> f64 {
        self.law().travel(v)
    }

    /// The value a drag is carrying, after the pointer has risen `up` points
    /// (down is negative) from where it last was.
    ///
    /// What goes in and comes out is the RAW value -- what the hand has asked
    /// for, before the detent -- because a drag that settled on every move
    /// would round each pixel's worth back to where it started, and a fine
    /// drag over a coarse detent would never leave it. The caller settles
    /// what it publishes and keeps this.
    ///
    /// Held to the stops, though, and at once: a pointer that has gone a
    /// hand's width past the top must not have to come all the way back down
    /// before the value starts to fall. Clamping here moves the anchor with
    /// the hand, as the number field's scrub does.
    ///
    /// `fine` is Shift: a tenth of the speed, so the same travel covers a
    /// tenth of the range.
    pub fn carry(&self, raw: f64, up: f64, fine: bool) -> f64 {
        let (lo, hi) = self.stops();
        // A travel of nought or less is not a travel. It would divide into
        // infinity and pin the value to a stop on the first pixel, or run
        // the drag backwards.
        let travel = if self.travel > 0.0 { self.travel } else { 150.0 };
        let rate = if fine { 0.1 } else { 1.0 };
        (raw + up / travel * (hi - lo) * rate).clamp(lo, hi)
    }
}

/// Where the dial stands in its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnobFace {
    /// The face's diameter: the biggest circle the box holds once the text
    /// rows are taken off its height.
    pub diameter: f64,
    /// The top of the stack -- name, face, number -- from the top of the
    /// box. The slack is shared above and below, so a short knob in a tall
    /// cell sits in the middle of it.
    pub top: f64,
}

/// The face a box of `width` by `height` has room for, with `label_px` of
/// name over it and `readout_px` of number under it (nought for a row that
/// is not there).
///
/// Three lines, and the shader has the same three: this is what lays the
/// words out and that is what paints the dial, and they agree because they
/// are one piece of arithmetic written down twice and tested once.
pub fn knob_face(width: f64, height: f64, label_px: f64, readout_px: f64) -> KnobFace {
    let rows = label_px + readout_px;
    let diameter = width.min(height - rows).max(0.0);
    let top = ((height - rows - diameter) * 0.5).max(0.0);
    KnobFace { diameter, top }
}

/// The number under a knob: the slider's readout with the unit hard against
/// the number. A row has room for the space between them and a cell does not
/// -- `100 %` in the panel's small face is a point wider than a 28 point
/// cell, and it is the unit that falls off the end.
pub fn knob_readout(value: f64, precision: usize, unit: &str) -> String {
    let mut text = crate::slider::format_readout(value, precision, "");
    text.push_str(unit);
    text
}

/// Is a press at `time` and `abs` the second half of a double click whose
/// first half was the press in `first`?
///
/// Measured press to press, against the platform's own two numbers, and by
/// the knob itself rather than read off the event's tap count: that count is
/// kept by the platform's event loop, which a headless run does not have, so
/// a reset hung off it could be shipped and never once tested.
pub fn is_double_press(first: Option<(f64, Vec2d)>, time: f64, abs: Vec2d) -> bool {
    use crate::event::{TAP_COUNT_DISTANCE, TAP_COUNT_TIME};
    first.is_some_and(|(t, p)| {
        time - t < TAP_COUNT_TIME && (abs - p).length() < TAP_COUNT_DISTANCE
    })
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabKnob {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawFabKnob,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_value: DrawText,
    /// The number's ink while the knob stands at nought.
    #[live]
    draw_value_off: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The name over the face. Empty is no name AND no row for one.
    #[live]
    label: String,
    /// The name's row and the number's, in points. Fixed rather than fitted,
    /// for the slider's reason: a matrix is a grid of these, and the faces
    /// have to stand on one line all the way along a row.
    #[live(12.0)]
    label_height: f64,
    /// Whether the number is printed under the face. Off, its row goes too
    /// and the face takes the room.
    #[live(true)]
    show_readout: bool,
    #[live(12.0)]
    readout_height: f64,
    #[live]
    min: f64,
    #[live(100.0)]
    max: f64,
    /// The arrow-key and wheel increment, and the detent a drag lands on.
    #[live(1.0)]
    step: f64,
    /// Shift+arrow and Shift+wheel. Coarse, as it is on the slider, and for
    /// the slider's reason: one step is already the small gesture. Under a
    /// DRAG Shift is fine instead, because a drag is continuous and has
    /// somewhere smaller to go.
    #[live(10.0)]
    big_step: f64,
    #[live(0)]
    precision: usize,
    #[live]
    unit: String,
    #[live]
    value: f64,
    /// The pointer travel that covers the whole range, in points.
    #[live(150.0)]
    drag_travel: f64,
    /// Whether the wheel turns a knob the pointer is merely OVER.
    ///
    /// Off, the wheel turns only the knob that has the keyboard -- the one
    /// that was last pressed. The panel these sit in scrolls, and it is a
    /// wall of them: a knob that took every wheel that crossed it would be a
    /// panel that cannot be scrolled and a column of weights moved by
    /// accident on the way past. On is for a host whose knobs stand somewhere
    /// that does not scroll.
    #[live(false)]
    wheel_on_hover: bool,
    /// Off: the knob shows dimmed and nothing answers.
    #[live(true)]
    enabled: bool,

    /// Held for the length of a gesture, so Escape and a modal's dismissal
    /// reach this control rather than whatever it is sitting in.
    #[rust]
    cancel_scope: Option<CancelScope>,
    /// A press is down and is this knob's to turn.
    #[rust]
    dragging: bool,
    /// The press has travelled [`KNOB_DRAG_SLOP`] and is turning the knob.
    #[rust]
    engaged: bool,
    /// The drag has said `Changed` at least once, so its release owes a
    /// commit.
    #[rust]
    drag_said: bool,
    /// What the drag has asked for, before the detent. See
    /// [`KnobTurn::carry`].
    #[rust]
    drag_raw: f64,
    /// Where the pointer was at the last move. The drag is summed move by
    /// move rather than measured from the press, so that Shift can come and
    /// go in the middle of one without the value jumping to where the other
    /// rate would have had it.
    #[rust]
    drag_last_y: f64,
    /// What the value was when the press landed, for a cancel to put back.
    #[rust]
    press_value: f64,
    /// When and where the last press landed, while it could still be the
    /// first half of a double click.
    #[rust]
    first_press: Option<(f64, Vec2d)>,
    /// The number this knob PRINTS, where that is not the number it holds.
    /// See `FabSlider::readout`: a column rounded over the column.
    #[rust]
    readout: Option<f64>,
    #[rust]
    hovered: bool,
    /// A keyboard run owes a commit. See `FabSlider::key_commit_due`.
    #[rust]
    key_commit_due: bool,
    /// Wheel travel that has not yet made a whole notch. A trackpad sends a
    /// notch as a dozen small deltas.
    #[rust]
    wheel_carry: f64,
    /// A spin of the wheel owes a commit, and this is the clock it is paid
    /// on.
    #[rust]
    wheel_commit_due: bool,
    #[rust]
    wheel_timer: Timer,
}

impl FabKnob {
    fn turn(&self) -> KnobTurn {
        KnobTurn {
            min: self.min,
            max: self.max,
            step: self.step,
            travel: self.drag_travel,
        }
    }

    /// The two text rows as they stand: nought for the one that is not there.
    fn text_rows(&self) -> (f64, f64) {
        (
            if self.label.is_empty() { 0.0 } else { self.label_height.max(0.0) },
            if self.show_readout { self.readout_height.max(0.0) } else { 0.0 },
        )
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// A value pushed in from outside, held EXACTLY as it was handed over and
    /// clamped to the stops and to nothing else. Emits nothing. Everything
    /// [`FabSlider::set_value`] says about the detent being the hand's grid
    /// and never a filter on the host's arithmetic is true here, and for the
    /// same rows of weights.
    ///
    /// Refused mid-drag: a host answering late must not argue with the hand
    /// that is on the knob.
    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        if self.dragging {
            return;
        }
        self.hold(cx, v, None);
    }

    /// The number the knob HOLDS and the number it PRINTS, handed over
    /// together, for the host whose readout is rounded over a whole column.
    /// See [`FabSlider::set_value_and_readout`]. Emits nothing; refused
    /// mid-drag.
    pub fn set_value_and_readout(&mut self, cx: &mut Cx, v: f64, readout: f64) {
        if self.dragging {
            return;
        }
        self.hold(cx, v, Some(readout));
    }

    /// What the knob prints: what a host said to print, or what it holds.
    pub fn readout(&self) -> f64 {
        self.readout.unwrap_or(self.value)
    }

    /// The one door a host's number comes in by. The redraw hangs off the
    /// PAIR, as the slider's does.
    fn hold(&mut self, cx: &mut Cx, v: f64, readout: Option<f64>) {
        let v = self.turn().contain(v);
        if (v - self.value).abs() > f64::EPSILON || readout != self.readout {
            self.value = v;
            self.readout = readout;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// The knob's name. Empty takes the name's row away with it.
    pub fn set_label(&mut self, cx: &mut Cx, text: &str) {
        if self.label != text {
            self.label = text.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switching off mid-gesture ends the gesture first, by every hand: the
    /// pointer is let go and the value the press found put back, and a run
    /// of the keyboard or of the wheel that has not committed yet pays up. A
    /// knob nothing can reach will never see what would have ended them.
    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            let uid = self.widget_uid();
            self.cancel_drag(cx, uid);
            self.end_runs(cx, uid);
            self.hovered = false;
        }
        self.draw_bg.redraw(cx);
    }

    /// Answers whether the value actually moved.
    fn publish(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, ended: bool) -> bool {
        let moved = (v - self.value).abs() > f64::EPSILON;
        if moved {
            self.value = v;
            // The hand's number is the knob's own; see `FabSlider::publish`.
            self.readout = None;
            self.draw_bg.redraw(cx);
            cx.widget_action(uid, FabKnobAction::Changed(self.value));
        }
        if ended {
            cx.widget_action(uid, FabKnobAction::Ended(self.value));
        }
        moved
    }

    /// One key's worth of movement, and whether it ends anything: a fresh
    /// press commits where it lands, a repeat says only that the value moved
    /// and leaves the commit to the release. `FabSlider::key_step`, whole.
    fn key_step(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, repeat: bool) {
        let moved = self.publish(cx, uid, v, !repeat);
        if repeat {
            self.key_commit_due |= moved;
        } else {
            self.key_commit_due = false;
        }
    }

    /// The commit a keyboard run still owes.
    fn end_key_run(&mut self, cx: &mut Cx, uid: WidgetUid) {
        if self.key_commit_due {
            self.key_commit_due = false;
            cx.widget_action(uid, FabKnobAction::Ended(self.value));
        }
    }

    /// The commit a spin of the wheel still owes, paid when the wheel has
    /// been still long enough -- or at whatever ends the spin before that.
    fn end_wheel_run(&mut self, cx: &mut Cx, uid: WidgetUid) {
        cx.stop_timer(self.wheel_timer);
        self.wheel_timer = Timer::default();
        self.wheel_carry = 0.0;
        if self.wheel_commit_due {
            self.wheel_commit_due = false;
            cx.widget_action(uid, FabKnobAction::Ended(self.value));
        }
    }

    /// Both of the runs that end on something other than a release.
    fn end_runs(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.end_key_run(cx, uid);
        self.end_wheel_run(cx, uid);
    }

    /// One step's worth, or `big_step`'s; a hundredth of the range where the
    /// knob is continuous, which is the arrow-key equivalent of one percent.
    fn increment(&self, big: bool) -> f64 {
        let step = if big { self.big_step } else { self.step };
        if step > 0.0 {
            step
        } else {
            (self.max - self.min).abs() * 0.01
        }
    }

    /// One step from where the knob actually STANDS, which may be off the
    /// detent because a host put it there. See `FabSlider::nudge`.
    fn nudge(&mut self, cx: &mut Cx, uid: WidgetUid, direction: f64, big: bool, repeat: bool) {
        let v = self.turn().contain(self.value + direction * self.increment(big));
        self.key_step(cx, uid, v, repeat);
    }

    /// The wheel, a notch at a time. Up is more. What is left of a notch is
    /// kept for the next event, so a trackpad's dozen small deltas add up to
    /// the step a wheel's one click is.
    fn wheel(&mut self, cx: &mut Cx, uid: WidgetUid, scroll: Vec2d, big: bool) {
        // Shift turns a wheel sideways on some platforms; it is the same
        // wheel.
        let axis = if scroll.y != 0.0 { -scroll.y } else { -scroll.x };
        self.wheel_carry += axis / 120.0;
        let notches = self.wheel_carry.trunc();
        if notches == 0.0 {
            return;
        }
        self.wheel_carry -= notches;
        let v = self.turn().contain(self.value + notches * self.increment(big));
        if self.publish(cx, uid, v, false) {
            self.wheel_commit_due = true;
        }
        if self.wheel_commit_due {
            cx.stop_timer(self.wheel_timer);
            self.wheel_timer = cx.start_timeout(KNOB_WHEEL_SETTLE);
        }
    }

    /// NOUGHT, as the slider's reset is and for its reason: "this one counts
    /// for nothing" is the same number whichever cell it is asked of. A range
    /// that never reaches nought takes its nearest stop instead. The commit
    /// goes out with it, because a double click is over when it lands.
    fn reset(&mut self, cx: &mut Cx, uid: WidgetUid) {
        let v = self.turn().settle(0.0);
        self.publish(cx, uid, v, false);
        cx.widget_action(uid, FabKnobAction::Reset);
        cx.widget_action(uid, FabKnobAction::Ended(self.value));
    }

    fn cancel_drag(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.cancel_scope = None;
        if self.dragging {
            self.dragging = false;
            self.engaged = false;
            self.drag_said = false;
            let back = self.press_value;
            self.publish(cx, uid, back, false);
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for FabKnob {
    // The generic switch and the bridge's `enabled` column both come through
    // here, so what they say is what the knob does.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.set_enabled(cx, !disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        !self.enabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let (label_px, readout_px) = self.text_rows();
        self.draw_bg.label_px = label_px as f32;
        self.draw_bg.readout_px = readout_px as f32;
        self.draw_bg.travel = self.turn().travel(self.value) as f32;
        self.draw_bg.hover = if self.hovered && self.enabled { 1.0 } else { 0.0 };
        self.draw_bg.down = if self.dragging { 1.0 } else { 0.0 };
        self.draw_bg.focus = if cx.cx.cx.has_key_focus(self.draw_bg.area()) {
            1.0
        } else {
            0.0
        };
        self.draw_bg.disabled = if self.enabled { 0.0 } else { 1.0 };
        // The face is measured off the whole box, so the box is laid out
        // with no padding whatever the caller wrote: see the template.
        let layout = Layout {
            padding: Inset::default(),
            spacing: 0.0,
            ..self.layout
        };
        self.draw_bg.begin(cx, walk, layout);

        // A box that is Fit on an axis has no size yet. A Fit height comes
        // out as a square face with its rows round it, which is what the
        // arithmetic gives for a box exactly as tall as its stack; a Fit
        // width has nothing to be measured against and takes the default.
        let size = cx.turtle().rect().size;
        let width = if size.x.is_finite() { size.x } else { 44.0 };
        let height = if size.y.is_finite() {
            size.y
        } else {
            width + label_px + readout_px
        };
        let face = knob_face(width, height, label_px, readout_px);
        let row = |h: f64| Walk::new(Size::Fixed(width), Size::Fixed(h));

        if face.top > 0.0 {
            let _ = cx.walk_turtle(row(face.top));
        }
        if label_px > 0.0 {
            self.draw_label
                .draw_walk(cx, row(label_px), Align { x: 0.5, y: 0.5 }, &self.label);
        }
        // The dial itself is painted by the face underneath; this only
        // claims the height the shader gives it, so the number lands under
        // the dial and not on it.
        let _ = cx.walk_turtle(row(face.diameter));
        if readout_px > 0.0 {
            let text = knob_readout(self.readout(), self.precision, &self.unit);
            let lit = self.turn().travel(self.value) > 0.0005;
            let ink = if lit {
                &mut self.draw_value
            } else {
                &mut self.draw_value_off
            };
            ink.draw_walk(cx, row(readout_px), Align { x: 0.5, y: 0.5 }, &text);
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        // Off: nothing below answers.
        if !self.enabled {
            return;
        }
        if self.dragging && crate::family_api::is_modal_dismissal(event) {
            self.cancel_drag(cx, uid);
            return;
        }
        // The window going away ends whatever this knob was in the middle
        // of, by any hand. Ordered as `set_enabled` orders the same things,
        // so that what is committed is the value the knob is left on. See
        // the same arm of `FabSlider::handle_event` for why `KeyFocusLost`
        // does not stand in for this.
        if let Event::WindowLostFocus(_) = event {
            self.cancel_drag(cx, uid);
            self.end_runs(cx, uid);
            return;
        }
        // The wheel has been still long enough: the spin is over.
        if self.wheel_timer.is_event(event).is_some() {
            self.end_wheel_run(cx, uid);
            return;
        }
        // Escape, Back or the right button puts back the value the press
        // found.
        if self.dragging {
            match event {
                Event::KeyDown(ke)
                    if ke.key_code == KeyCode::Escape
                        && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s)) =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::BackPressed { .. }
                    if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                        && event.back_pressed() =>
                {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::MouseDown(me) if me.button.is_secondary() => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                _ => {}
            }
        }

        // THE POINTER-CAPTURE RULE. One area, asked plainly: no sweep area
        // and no capture overload, so the press this takes is the press
        // nothing else is holding, and `hits` holds it until the release. A
        // drag that leaves the cell -- and every drag on a 28 point knob
        // leaves the cell -- goes on turning this knob and lights up nothing
        // it passes over.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::NsResize);
                if !self.hovered {
                    self.hovered = true;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                // A press is the end of whatever the wheel was doing.
                self.end_wheel_run(cx, uid);
                self.cancel_scope = Some(self.begin_cancel_scope(cx));
                if is_double_press(self.first_press, fe.time, fe.abs) {
                    // The second half of a double click is a command and
                    // not a grip: it turns nothing, and a third press starts
                    // the count again.
                    self.first_press = None;
                    self.reset(cx, uid);
                } else {
                    self.first_press = Some((fe.time, fe.abs));
                    self.dragging = true;
                    self.engaged = false;
                    self.drag_said = false;
                    self.press_value = self.value;
                    self.drag_raw = self.value;
                    self.drag_last_y = fe.abs.y;
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                if !self.dragging {
                    return;
                }
                // Up and down only. A hand pulling a knob up wanders
                // sideways, and a sideways inch that counted for anything
                // would make every drag a slightly different one.
                if !self.engaged {
                    let come = fe.abs.y - self.drag_last_y;
                    if come.abs() < KNOB_DRAG_SLOP {
                        return;
                    }
                    // Measured from the EDGE of the slop, so the slop is
                    // travel the value never sees rather than a jump it takes
                    // on engaging -- and so whatever the pointer has come
                    // beyond it counts, however few moves it came in.
                    self.engaged = true;
                    self.drag_last_y += come.signum() * KNOB_DRAG_SLOP;
                    // A press that has travelled is a drag, and a drag is
                    // not the first half of a double click.
                    self.first_press = None;
                }
                let up = self.drag_last_y - fe.abs.y;
                self.drag_last_y = fe.abs.y;
                if up == 0.0 {
                    return;
                }
                let turn = self.turn();
                self.drag_raw = turn.carry(self.drag_raw, up, fe.modifiers.shift);
                let v = turn.settle(self.drag_raw);
                if self.publish(cx, uid, v, false) {
                    self.drag_said = true;
                }
            }
            Hit::FingerUp(fe) => {
                self.cancel_scope = None;
                if self.dragging && self.drag_said {
                    cx.widget_action(uid, FabKnobAction::Ended(self.value));
                }
                self.dragging = false;
                self.engaged = false;
                self.drag_said = false;
                // A drag on a knob ends somewhere else more often than not,
                // and a release off the knob is the last this area hears of
                // that pointer: no hover-out follows it, because the hover
                // was never handed back. Unanswered, the knob stays lit
                // until the pointer happens to cross it again.
                self.hovered = fe.is_over && fe.device.has_hovers();
                self.draw_bg.redraw(cx);
            }
            Hit::FingerScroll(fe)
                if self.wheel_on_hover || cx.has_key_focus(self.draw_bg.area()) =>
            {
                // A hand that is on the knob already has it.
                if !self.dragging {
                    self.wheel(cx, uid, fe.scroll, fe.modifiers.shift);
                }
                // The wheel a knob took is spent, whether or not the notch
                // moved it: a knob at its stop still holds the panel, or the
                // last notch of a spin would turn into a scroll.
                event.set_scroll_handled(Vec2Index::X);
                event.set_scroll_handled(Vec2Index::Y);
            }
            // Ctrl and Cmd are the accelerator space and belong to whatever
            // this knob is sitting in; see the same arm of the slider.
            Hit::KeyDown(ke) if !ke.modifiers.control && !ke.modifiers.logo => {
                match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => {
                        self.nudge(cx, uid, -1.0, ke.modifiers.shift, ke.is_repeat)
                    }
                    KeyCode::ArrowRight | KeyCode::ArrowUp => {
                        self.nudge(cx, uid, 1.0, ke.modifiers.shift, ke.is_repeat)
                    }
                    // Absolute, both of them, so a key held against the end
                    // of its own travel goes quiet.
                    KeyCode::Home => {
                        let v = self.turn().stops().0;
                        self.key_step(cx, uid, v, ke.is_repeat);
                    }
                    KeyCode::End => {
                        let v = self.turn().stops().1;
                        self.key_step(cx, uid, v, ke.is_repeat);
                    }
                    _ => {}
                }
            }
            // Letting go of the key that was driving the value ends the
            // gesture, the way letting go of the mouse button does.
            Hit::KeyUp(ke)
                if matches!(
                    ke.key_code,
                    KeyCode::ArrowLeft
                        | KeyCode::ArrowRight
                        | KeyCode::ArrowUp
                        | KeyCode::ArrowDown
                        | KeyCode::Home
                        | KeyCode::End
                ) =>
            {
                self.end_key_run(cx, uid);
            }
            Hit::KeyFocus(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                // The keyboard has gone elsewhere, and with it both the
                // release that would have ended a run of arrows and the
                // wheel's claim on this knob: what either owes is paid here
                // or never.
                self.end_runs(cx, uid);
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }
}

impl FabKnobRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabKnobAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        knob_ended_value(actions, self.widget_uid())
    }

    /// Was the knob double clicked? The value is already back at nought and
    /// the commit for it is in the same buffer; this is the host's cue to
    /// drop the cell from whatever ledger it keeps.
    pub fn was_reset(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions_cast::<FabKnobAction>(self.widget_uid())
            .any(|action| matches!(action, FabKnobAction::Reset))
    }

    /// Emits nothing. See [`FabKnob::set_value`].
    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    /// Emits nothing. See [`FabKnob::set_value_and_readout`].
    pub fn set_value_and_readout(&self, cx: &mut Cx, v: f64, readout: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value_and_readout(cx, v, readout);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.value())
    }

    /// What the knob prints, which is what it holds unless a host said
    /// otherwise.
    pub fn readout(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.readout())
    }

    pub fn set_label(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_label(cx, text);
        }
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, enabled);
        }
    }

    pub fn enabled(&self) -> bool {
        self.borrow().map_or(true, |i| i.enabled())
    }
}

/// `Changed` comes before `Ended` in the same buffer, so the commit has to
/// be looked for rather than found. See `slider_ended_value`.
fn knob_ended_value(actions: &Actions, uid: WidgetUid) -> Option<f64> {
    for action in actions.filter_widget_actions_cast::<FabKnobAction>(uid) {
        if let FabKnobAction::Ended(v) = action {
            return Some(v);
        }
    }
    None
}

// ===========================================================================
// FabDiagonalLabel — the name over a column too narrow to hold it.
//
// Where the name goes, and the glyph walk that puts it there, are in
// diagonal_text.rs: the library's tables turn their headings by the same
// arithmetic, and a name standing over the wrong column names the wrong
// column, so there is one copy of that sum and not three. What stays here is
// this control's own face — the panel's palette, the panel's small type.
//
// A matrix of knobs ten rows by eight columns, in a sidebar 280 wide, comes
// out around 26 points a column, and the names a panel has to write over
// such columns -- a theme's, a family's -- run to two and a half times that.
// So the name is turned on its side and let out over its neighbours,
// which is safe
// for the reason parallel lines are safe: at 45 degrees a pitch of 26 puts
// 26 * sin(45) = 18 points between one name and the next ACROSS the line,
// and a line of the panel's small face is ten, so however long the names get
// they never touch — only the empty ground beside them is crossed.
//
// The widget therefore draws OUTSIDE its own box on purpose. The box is an
// anchor and not a frame, and nothing here opens a turtle, because a turtle
// is exactly what would cut the name off: a begun turtle pushes its rect
// onto the clip stack, the align pass stamps that rect onto every instance
// drawn inside it, and `DrawText` discards the pixels outside it
// (`draw/src/turtle.rs`, `clip_and_shift_align_list`; the clip note at the
// top of `draw/src/shader/draw_text.rs`). The host owes the other half of
// that bargain: the row these stand in carries `clip_x: false`, and the
// ground the ink spills onto has to be inside whatever DOES clip.
//
// Where the baseline starts is the one thing that has to be exact — a name
// standing over the wrong column names the wrong column — so that is pure
// arithmetic, settled and tested without a window.
// ===========================================================================

/// A name written across the corner of the box it names, for a column too
/// narrow to hold it flat. No gesture, no focus, no actions: it is a label.
#[derive(Script, ScriptHook, Widget)]
pub struct FabDiagonalLabel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_text: DrawRotatedText,
    #[walk]
    walk: Walk,

    /// The name.
    #[live]
    text: String,
    /// How far from the horizontal the name is turned, in degrees, 0 to 90.
    #[live(45.0)]
    angle: f64,
    /// Which way it leans, and so which side it hangs over.
    #[live]
    lean: DiagonalLean,

    /// The claimed box, kept so a set of the name can ask for the frame that
    /// shows it. The glyphs are not it: a label with nothing in it draws no
    /// glyphs and would then have no area to redraw from, which is exactly
    /// the moment a host fills the header in.
    #[redraw]
    #[rust]
    area: Area,
    /// Scratch for one name's glyphs, kept so a wall of these does not
    /// allocate once a frame each.
    #[rust]
    glyphs: Vec<PathGlyphInstance>,
    /// Where the last draw actually put the ink. A host that wants to know
    /// whether its row is tall enough can read it back rather than guess.
    #[rust]
    last_run: Option<DiagonalRun>,
}

impl FabDiagonalLabel {
    /// Where the last draw put the name, or `None` before it has drawn one.
    pub fn last_run(&self) -> Option<DiagonalRun> {
        self.last_run
    }

    /// How tall a header row has to be for `longest` at this label's angle
    /// and face, in points.
    ///
    /// The host asks once, with the longest name it will ever write, and
    /// fixes the row at the answer; every shorter name then hangs from the
    /// same bottom line. See [`diagonal_row_height`] for the sum itself,
    /// which a DSL can do without a widget.
    pub fn row_height_for(&self, cx: &mut Cx2d, longest: &str) -> f64 {
        match self.draw_text.prepare_single_line_run(cx, longest) {
            Some(run) => diagonal_row_height(
                run.width_in_lpxs as f64,
                (run.ascender_in_lpxs - run.descender_in_lpxs) as f64,
                self.angle,
            ),
            None => 0.0,
        }
    }
}

impl Widget for FabDiagonalLabel {
    fn text(&self) -> String {
        self.text.clone()
    }

    /// Emits nothing, and says nothing when the name has not changed: the
    /// host writes every header into a fixed slot on every draw, and a
    /// setter that dirtied the draw list each time would redraw the panel
    /// forever.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text == v {
            return;
        }
        self.text.clear();
        self.text.push_str(v);
        self.redraw(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The box is CLAIMED and then drawn over, never drawn in: no turtle
        // is begun here, so this widget pushes no clip of its own and the
        // name is free to cross its neighbours. See the note above.
        let rect = cx.walk_turtle(walk);
        cx.add_aligned_rect_area(&mut self.area, rect);
        // A Fill on an axis nothing has sized is a NaN box; there is no
        // centre to stand a name on, so the shared placement draws nothing
        // rather than a name at nowhere.
        let mut glyphs = std::mem::take(&mut self.glyphs);
        self.last_run = draw_diagonal_name(
            &mut self.draw_text,
            cx,
            &mut glyphs,
            rect,
            &self.text,
            self.angle,
            self.lean,
        );
        self.glyphs = glyphs;
        DrawStep::done()
    }
}

impl FabDiagonalLabelRef {
    pub fn text(&self) -> String {
        self.borrow().map_or_else(String::new, |inner| inner.text())
    }

    /// Emits nothing. See [`FabDiagonalLabel::set_text`].
    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    /// Where the last draw put the name. See
    /// [`FabDiagonalLabel::last_run`].
    pub fn last_run(&self) -> Option<DiagonalRun> {
        self.borrow().and_then(|inner| inner.last_run())
    }
}

// ===========================================================================
// FabColorWheel
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawColorWheel {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hue: f32,
    #[live]
    sat: f32,
    #[live]
    val: f32,
}

#[derive(Clone, Debug, Default)]
pub enum ColorWheelAction {
    /// Live while dragging or nudging: (hue, sat, val), all 0..1.
    Changed([f32; 3]),
    /// The gesture finished (mouse up).
    Ended([f32; 3]),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabColorWheel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_wheel: DrawColorWheel,
    #[walk]
    walk: Walk,
    #[rust]
    drag: Option<WheelZone>,
}

impl FabColorWheel {
    pub fn set_hsv(&mut self, cx: &mut Cx, h: f32, s: f32, v: f32) {
        if (h - self.draw_wheel.hue).abs() > f32::EPSILON
            || (s - self.draw_wheel.sat).abs() > f32::EPSILON
            || (v - self.draw_wheel.val).abs() > f32::EPSILON
        {
            self.draw_wheel.hue = h;
            self.draw_wheel.sat = s;
            self.draw_wheel.val = v;
            self.draw_wheel.redraw(cx);
        }
    }

    pub fn hsv(&self) -> [f32; 3] {
        [
            self.draw_wheel.hue,
            self.draw_wheel.sat,
            self.draw_wheel.val,
        ]
    }

    fn apply_pointer(&mut self, cx: &mut Cx, uid: WidgetUid, abs: DVec2, ended: bool) {
        let rect = self.draw_wheel.area().rect(cx);
        let size = rect.size.x.min(rect.size.y);
        let rel = abs - rect.pos;
        match self.drag {
            Some(WheelZone::Ring) => {
                self.draw_wheel.hue = ring_hue(rel, size);
            }
            Some(WheelZone::Square) => {
                let (s, v) = square_sv(rel, size);
                self.draw_wheel.sat = s;
                self.draw_wheel.val = v;
            }
            _ => return,
        }
        self.draw_wheel.redraw(cx);
        let hsv = self.hsv();
        cx.widget_action(uid, ColorWheelAction::Changed(hsv));
        if ended {
            cx.widget_action(uid, ColorWheelAction::Ended(hsv));
        }
    }

    fn nudge(&mut self, cx: &mut Cx, uid: WidgetUid, dh: f32, dv: f32) {
        self.draw_wheel.hue = (self.draw_wheel.hue + dh).rem_euclid(1.0);
        self.draw_wheel.val = (self.draw_wheel.val + dv).clamp(0.0, 1.0);
        self.draw_wheel.redraw(cx);
        let hsv = self.hsv();
        cx.widget_action(uid, ColorWheelAction::Changed(hsv));
        cx.widget_action(uid, ColorWheelAction::Ended(hsv));
    }
}

impl Widget for FabColorWheel {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = self.draw_wheel.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_wheel.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Crosshair);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_wheel.area());
                let rect = self.draw_wheel.area().rect(cx);
                let size = rect.size.x.min(rect.size.y);
                let zone = wheel_zone(fe.abs - rect.pos, size, self.hsv());
                if zone != WheelZone::None {
                    self.drag = Some(zone);
                    self.apply_pointer(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerMove(fe) => {
                if self.drag.is_some() {
                    self.apply_pointer(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerUp(fe) => {
                if self.drag.is_some() {
                    if fe.cancelled {
                        // Taken away: the colour stays the last one the drag
                        // set, and the edit ends there.
                        cx.widget_action(uid, ColorWheelAction::Ended(self.hsv()));
                    } else {
                        self.apply_pointer(cx, uid, fe.abs, true);
                    }
                    self.drag = None;
                }
            }
            Hit::KeyDown(ke) => {
                let fine = if ke.modifiers.shift { 0.1 } else { 1.0 };
                match ke.key_code {
                    KeyCode::ArrowLeft => self.nudge(cx, uid, -fine / 360.0, 0.0),
                    KeyCode::ArrowRight => self.nudge(cx, uid, fine / 360.0, 0.0),
                    KeyCode::ArrowUp => self.nudge(cx, uid, 0.0, fine / 100.0),
                    KeyCode::ArrowDown => self.nudge(cx, uid, 0.0, -fine / 100.0),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl FabColorWheelRef {
    pub fn changed(&self, actions: &Actions) -> Option<[f32; 3]> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ColorWheelAction::Changed(hsv) = item.cast() {
                return Some(hsv);
            }
        }
        None
    }

    pub fn set_hsv(&self, cx: &mut Cx, h: f32, s: f32, v: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_hsv(cx, h, s, v);
        }
    }
}

// ===========================================================================
// FabPaletteStrip — a wrapped grid of small colour cells (one draw call),
// hit-tested by rect math. The host fills it (a theme palette); hover and
// click come back as indices.
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabPaletteCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub cell: Vec4f,
    #[live]
    pub hot: f32,
    #[live]
    pub cur: f32,
}

#[derive(Clone, Debug, Default)]
pub enum FabPaletteAction {
    /// The pointer rests on a cell (None: it left the strip).
    Hover(Option<usize>),
    Pick(usize),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabPaletteStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_cell: DrawFabPaletteCell,
    #[walk]
    walk: Walk,
    #[live]
    cell_size: f64,
    #[live]
    gap: f64,
    #[rust]
    colors: Vec<[f32; 4]>,
    #[rust]
    current: Option<usize>,
    #[rust]
    hot: Option<usize>,
    #[rust]
    cols: usize,
    #[rust]
    area: Area,
}

impl FabPaletteStrip {
    pub fn set_colors(&mut self, cx: &mut Cx, colors: Vec<[f32; 4]>) {
        self.colors = colors;
        self.hot = None;
        self.draw_cell.redraw(cx);
    }

    /// Mark the cell equal to the host's current colour.
    pub fn set_current(&mut self, cx: &mut Cx, current: Option<usize>) {
        if self.current != current {
            self.current = current;
            self.draw_cell.redraw(cx);
        }
    }

    fn pitch(&self) -> f64 {
        self.cell_size + self.gap
    }

    fn cols_for(&self, width: f64) -> usize {
        (((width + self.gap) / self.pitch()).floor() as usize).max(1)
    }

    /// The strip's height at a width (the popover sizes itself with it).
    pub fn height_for(&self, width: f64) -> f64 {
        if self.colors.is_empty() {
            return 0.0;
        }
        let rows = self.colors.len().div_ceil(self.cols_for(width));
        rows as f64 * self.pitch() - self.gap
    }

    fn cell_at(&self, rect: Rect, abs: DVec2) -> Option<usize> {
        if !rect.contains(abs) || self.cols == 0 {
            return None;
        }
        let rel = abs - rect.pos;
        let col = (rel.x / self.pitch()).floor() as usize;
        let row = (rel.y / self.pitch()).floor() as usize;
        if col >= self.cols {
            return None;
        }
        // The gap between cells belongs to nobody.
        if rel.x - col as f64 * self.pitch() > self.cell_size || rel.y - row as f64 * self.pitch() > self.cell_size {
            return None;
        }
        let index = row * self.cols + col;
        (index < self.colors.len()).then_some(index)
    }
}

impl Widget for FabPaletteStrip {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::flow_down());
        let width = cx.turtle().rect().size.x;
        self.cols = self.cols_for(width);
        let height = self.height_for(width);
        // Claim the grid's height, then paint the cells over it.
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(height)));
        let (pitch, cell_size) = (self.pitch(), self.cell_size);
        for (i, c) in self.colors.iter().enumerate() {
            let col = (i % self.cols) as f64;
            let row = (i / self.cols) as f64;
            self.draw_cell.cell = vec4(c[0], c[1], c[2], c[3]);
            self.draw_cell.hot = if self.hot == Some(i) { 1.0 } else { 0.0 };
            self.draw_cell.cur = if self.current == Some(i) { 1.0 } else { 0.0 };
            self.draw_cell.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + col * pitch, rect.pos.y + row * pitch),
                    size: dvec2(cell_size, cell_size),
                },
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        let rect = self.area.rect(cx);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.cell_at(rect, fe.abs);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_cell.redraw(cx);
                    cx.widget_action(uid, FabPaletteAction::Hover(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot.is_some() {
                    self.hot = None;
                    self.draw_cell.redraw(cx);
                    cx.widget_action(uid, FabPaletteAction::Hover(None));
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if let Some(i) = self.cell_at(rect, fe.abs) {
                    cx.widget_action(uid, FabPaletteAction::Pick(i));
                }
            }
            _ => {}
        }
    }
}

// ===========================================================================
// FabPaletteCarousel — every palette on offer, in one row that scrolls
// sideways. One draw call of chips, hit-tested by rect math, the way the
// palette strip is; the host hands it the list and says which is in force,
// and a pick comes back as an index.
// ===========================================================================

/// Four colours stacked in one quad, rounded as a whole: one chip of the
/// carousel. Or fewer: `bands` says how many, and each is still a square.
///
/// One quad and not four: a chip is one thing to a hand -- it is pressed, it
/// is outlined, it is the palette -- and four boxes with a corner each would
/// have to be rounded outside-only and kept in step. The bands are chosen in
/// the shader off the fragment's own height, a quarter each, so a chip four
/// times as tall as it is wide is four exact squares: a band wider than it is
/// tall reads as a stripe of the one above it, where a square reads as a
/// colour of its own.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabPaletteChip {
    #[deref]
    draw_super: DrawQuad,
    /// Top band first, the way the colours are read down the chip.
    #[live]
    pub band_0: Vec4f,
    #[live]
    pub band_1: Vec4f,
    #[live]
    pub band_2: Vec4f,
    #[live]
    pub band_3: Vec4f,
    /// How many of the four bands the chip shows, one to four: a palette of
    /// fewer colours is a shorter chip of the same squares.
    #[live]
    pub bands: f32,
    #[live]
    pub hover: f32,
    /// The chip the host is wearing: a ring that stays on without a pointer.
    #[live]
    pub cur: f32,
}

#[derive(Clone, Debug, Default)]
pub enum FabPaletteCarouselAction {
    /// A chip was pressed and let go without the row moving under it.
    Pick(usize),
    /// The pointer rests on a chip (None: it left the row, or a drag took it).
    Hover(Option<usize>),
    #[default]
    None,
}

/// How far a press may wander before it is a drag. A hand pressing a chip
/// 22 points wide is not still to the point, and a press that is let go a
/// pixel or two from where it landed is still a press on that chip.
const CAROUSEL_DRAG_SLOP: f64 = 4.0;

/// The arithmetic of a row of equal chips seen through a window: where each
/// one stands, which is under a point, and how far the row may scroll.
/// Apart from the widget so that it is checked on its own, without a draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipTrack {
    pub count: usize,
    pub chip_width: f64,
    pub gap: f64,
    /// The window's width, the part of the row that is on the screen.
    pub view: f64,
}

impl ChipTrack {
    pub fn pitch(&self) -> f64 {
        self.chip_width + self.gap
    }

    /// The whole row, first chip's left edge to the last one's right.
    pub fn span(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.count as f64 * self.pitch() - self.gap
    }

    /// The furthest the row scrolls: the last chip flush with the right
    /// edge, and nought for a row the window holds whole.
    pub fn max_scroll(&self) -> f64 {
        (self.span() - self.view).max(0.0)
    }

    pub fn clamp(&self, scroll: f64) -> f64 {
        scroll.clamp(0.0, self.max_scroll())
    }

    /// The least scroll that puts the whole of chip `index` in the window:
    /// a chip already in it leaves the row where it is, one off to the left
    /// brings its left edge to the window's, one off to the right its right
    /// edge. Least, because a row that jumped to centre every chip it was
    /// asked to show would move under a hand that had only just found it.
    pub fn reveal(&self, scroll: f64, index: usize) -> f64 {
        if index >= self.count {
            return self.clamp(scroll);
        }
        let left = index as f64 * self.pitch();
        let right = left + self.chip_width;
        let scroll = if left < scroll {
            left
        } else if right > scroll + self.view {
            right - self.view
        } else {
            scroll
        };
        self.clamp(scroll)
    }

    /// The chip under a point `x` along the window, if there is one. The gap
    /// between two chips belongs to neither: a press there is aimed at no
    /// palette, and choosing its neighbour would be a guess.
    pub fn index_at(&self, scroll: f64, x: f64) -> Option<usize> {
        if x < 0.0 || x > self.view {
            return None;
        }
        let along = x + scroll;
        if along < 0.0 {
            return None;
        }
        let index = (along / self.pitch()).floor() as usize;
        let into = along - index as f64 * self.pitch();
        (index < self.count && into <= self.chip_width).then_some(index)
    }

    /// How far one press of an arrow moves the row: as many whole chips as
    /// the window holds, and never fewer than one.
    ///
    /// Whole chips, and not the window's own width: a page of exactly the
    /// window would leave the chip that straddles the far edge cut in half on
    /// the other side, and the row is for comparing colours side by side. A
    /// window too narrow to hold one chip still moves on by one, or the arrow
    /// would be a button that does nothing.
    pub fn page(&self) -> f64 {
        // The last chip of a window needs no gap after it, so the gap is
        // added back before the count: at ten chips' pitch less one gap the
        // window holds ten, not nine.
        ((self.view + self.gap) / self.pitch()).floor().max(1.0) * self.pitch()
    }

    /// The chips any part of which is in the window, first to last.
    pub fn in_view(&self, scroll: f64) -> std::ops::Range<usize> {
        if self.count == 0 {
            return 0..0;
        }
        let first = ((scroll / self.pitch()).floor().max(0.0)) as usize;
        let last = (((scroll + self.view) / self.pitch()).ceil().max(0.0)) as usize;
        first.min(self.count)..last.min(self.count)
    }
}

/// What a scroll event moves the row, in points.
///
/// A sideways delta always: it is the row's own. A plain vertical wheel as
/// well, where a carousel standing in a page would leave it to the page:
/// this one is a single row a chip high, so there is nothing vertical over
/// it for the wheel to do, and a person with a one-wheel mouse has no other
/// way along it than a drag. Whichever of the two is the larger wins, so a
/// trackpad's diagonal drift does not scroll a row it was not moving along.
pub fn carousel_wheel(scroll_x: f64, scroll_y: f64) -> f64 {
    if scroll_x.abs() >= scroll_y.abs() {
        scroll_x
    } else {
        scroll_y
    }
}

/// The shortest and longest a glide may take, in seconds. These are the
/// app-facing carousel's own numbers (`carousel.rs`, which shares them with
/// the drum picker): two rows out of the same kit that settled at different
/// speeds would read as two different pieces of machinery.
const CHIP_GLIDE_SECS: (f64, f64) = (0.14, 0.75);

/// A glide is over once the drawing is this close to the row's place, in
/// points. An exponential approach never actually arrives, and a twentieth
/// of a point is inside a pixel at any scale the panel is drawn at.
const CHIP_GLIDE_DONE: f64 = 0.05;

/// A turn of the wheel is worth at most one chip of momentum after it. The
/// operating system's notches run from a few points to most of a screen, so
/// the speed read off them is not a hand's: a row that shot half its
/// palettes past because a notch was reported large cannot be aimed. The
/// app-facing carousel caps what one notch MOVES its strip for that same
/// reason; this caps what the notches throw it.
const CHIP_WHEEL_CARRY: f64 = 1.0;

/// The time constant of a glide closing `distance` points, opened at
/// `velocity` points a second.
///
/// Matching the opening speed to the hand is what makes the hand-off from a
/// drag to its flick invisible: the picture leaves the finger at the speed
/// the finger had. A move nobody threw -- an arrow, the chip in force
/// changing -- opens at the short end and is over in a seventh of a second.
///
/// Spelled again here rather than shared with the app-facing carousel,
/// which keeps its copy private: it is three lines, and the number that
/// matters ([`CHIP_GLIDE_SECS`]) is named in both places.
pub fn chip_glide_secs(distance: f64, velocity: f64) -> f64 {
    let v = velocity.abs();
    if v <= f64::EPSILON {
        return CHIP_GLIDE_SECS.0;
    }
    (distance.abs() / v).clamp(CHIP_GLIDE_SECS.0, CHIP_GLIDE_SECS.1)
}

/// How far a row let go at `velocity` points a second travels on, its speed
/// decaying by `decay_per_ms` every millisecond: the whole remaining travel
/// of `v(t) = v0 * decay^t`, which is `v0 / lambda` with
/// `lambda = -ln(decay) * 1000`.
///
/// The library's scrollers integrate that same decay frame by frame. A row
/// that only has to end up somewhere needs the end of it, because where it
/// lands is what decides the whole motion.
pub fn chip_spin_travel(velocity: f64, decay_per_ms: f64) -> f64 {
    let lambda = -decay_per_ms.ln() * 1000.0;
    if lambda <= 0.0 || !lambda.is_finite() {
        0.0
    } else {
        velocity / lambda
    }
}

/// What a release carries the row on by, in points: the flick.
///
/// `velocity` is the FINGER's, in points a second, and `travel` how far it
/// came; the row runs the other way to the hand, so the carry is negated. A
/// press that barely moved is a press and not a throw. A hand that rested
/// on the row before it let go has no speed either, because the samples
/// that old are dropped before the release is measured -- which is the rule
/// that stops a careful drag from drifting on after the hand stops.
pub fn carousel_flick(velocity: f64, travel: f64) -> f64 {
    if travel.abs() <= FLING_MIN_TOTAL_DELTA {
        return 0.0;
    }
    chip_spin_travel(-velocity, FLING_DECEL_RATE_PER_MS)
}

/// How far behind the row's own place its drawing still is, and how fast
/// that is closing: the glide.
///
/// The row's place is settled the moment anything moves it -- an arrow, a
/// wheel, the chip in force changing -- and it is the DRAWING that takes a
/// tenth of a second to get there. That way round, because everything else
/// asks the row where it stands: the arrows at its two ends, the chip under
/// a point, a host reading the scroll back. Answered off a picture still in
/// flight, those would answer the same question differently twice in a
/// frame, and the panel's arrows would flicker on and off through every
/// glide.
///
/// The lag closes the way the app-facing carousel's glide does, because it
/// IS that glide written from the other end: `to + (from - to) * exp(-t/tau)`
/// is `to - lag * exp(-t/tau)`. A strip of cards and a row of chips settle
/// alike, off the same two numbers and the same jitter-clamped clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChipGlide {
    /// The lag this decay started from, kept so that a frame is measured
    /// from the opening of the glide rather than from the frame before it.
    from: f64,
    /// Points the drawing is behind the row's place; nought at rest.
    lag: f64,
    tau: f64,
    /// The jitter-clamped frame clock the library's scrollers use, so a
    /// late frame becomes a slightly uneven step and not a visible jerk.
    clock: FrameClock,
}

impl ChipGlide {
    /// The row's place moved on by `moved` points: the drawing stays where
    /// it is and closes that from here, opening at `velocity` points a
    /// second.
    ///
    /// Re-aimed rather than restarted -- what is still outstanding is added
    /// to what is new -- so a second arrow press while the first is still
    /// running carries on from where the picture is, instead of jumping it
    /// to where the row now stands and gliding from there.
    pub fn moved(&mut self, moved: f64, velocity: f64) {
        let lag = self.lag + moved;
        if lag.abs() <= CHIP_GLIDE_DONE {
            self.land();
            return;
        }
        self.from = lag;
        self.lag = lag;
        self.tau = chip_glide_secs(lag, velocity);
        self.clock = FrameClock::default();
    }

    /// There is no lag: the picture and the place are one number again.
    ///
    /// A hand landing on the row calls this with the row's place already
    /// taken back to where the picture had got to -- so the row stops dead
    /// under the finger and the chip drawn there is the chip pressed. A
    /// width that clamps the row calls it as it is, since that place was
    /// decided by the layout rather than by a gesture.
    pub fn land(&mut self) {
        *self = Self::default();
    }

    /// Whether the drawing is still catching up, which is whether another
    /// frame is owed. At rest it is false and nothing asks for frames.
    pub fn running(&self) -> bool {
        self.lag != 0.0
    }

    /// One frame of it, at wall-clock `now`; the answer is [`ChipGlide::running`]
    /// again, so a caller asks for the next frame only while there is one to
    /// draw.
    pub fn tick(&mut self, now: f64) -> bool {
        if !self.running() {
            return false;
        }
        let t = self.clock.advance(now);
        let lag = self.from * (-t / self.tau.max(f64::EPSILON)).exp();
        if lag.abs() <= CHIP_GLIDE_DONE {
            self.land();
            false
        } else {
            self.lag = lag;
            true
        }
    }

    /// Where a row whose place is `scroll` is drawn this frame.
    pub fn at(&self, scroll: f64) -> f64 {
        scroll - self.lag
    }
}

/// A press held on the carousel: where it landed and where the row stood,
/// so a drag moves the row with the hand rather than jumping it.
#[derive(Clone, Copy, Debug)]
struct CarouselPress {
    abs_x: f64,
    scroll: f64,
    dragged: bool,
    /// The press landed a glide that was still running. Landing it is what
    /// the hand asked for, so the release that follows chooses nothing: a
    /// hand put out to stop a moving row is not pointing at a palette.
    caught: bool,
}

/// Every palette on offer, in one row that scrolls sideways.
///
/// Its own control rather than a View of chip slots: a View has no way to
/// grow a child at run time, and a list of palettes is as long as the colour
/// it is grown from makes it -- two dozen for a grey, sixty for a colour out
/// of the book. So the chips are drawn, not built: one quad each off the
/// list the host hands it, the ones in the window only, clipped to it.
///
/// The edge says there is more by cutting the chip that stands across it,
/// and nothing louder: a row of colour is busy enough, and a fade would lay
/// the panel's ground over the very colours somebody is comparing.
///
/// It scrolls by a drag, by a sideways delta, by a plain wheel (see
/// [`carousel_wheel`]) and by the arrow keys once it has the keyboard, and
/// it is clamped at both ends. The press holds the pointer until it is let
/// go, and nothing else in the panel reacts meanwhile; a press that moves
/// more than a few points is a drag and picks nothing.
#[derive(Script, ScriptHook, Widget)]
pub struct FabPaletteCarousel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_chip: DrawFabPaletteChip,
    #[walk]
    walk: Walk,
    #[live(22.0)]
    chip_width: f64,
    #[live(88.0)]
    chip_height: f64,
    #[live(3.0)]
    gap: f64,
    /// A host with nothing to offer hides the whole row: hidden, it draws
    /// nothing, takes no room in its parent and answers no input.
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    chips: Vec<[Vec4f; 4]>,
    /// How many of each chip's four colours are shown, the top ones: see
    /// [`FabPaletteCarousel::set_bands`].
    #[rust(4)]
    bands: usize,
    #[rust]
    chosen: Option<usize>,
    #[rust]
    hot: Option<usize>,
    /// How far along the row the window stands, in points from its start.
    #[rust]
    scroll: f64,
    /// How wide the window was when it was last drawn.
    ///
    /// Kept rather than asked of the area, because the two questions an arrow
    /// asks -- whether there is anywhere left to go that way, and how far a
    /// page is -- are asked while the host is drawing, when the area is last
    /// frame's and the draw list under it may already be gone. A number the
    /// row wrote itself is the same number whenever it is read.
    #[rust]
    window: f64,
    /// The chosen chip is owed a place in the window on the next draw,
    /// which is the first moment the window's width is known.
    #[rust]
    reveal_due: bool,
    #[rust]
    press: Option<CarouselPress>,
    /// The drawing's lag behind `scroll`, and the frames that close it.
    #[rust]
    glide: ChipGlide,
    #[rust]
    next_frame: NextFrame,
    /// Where the hand (or the wheel's own stream) has been lately, for the
    /// speed a release is thrown at.
    #[rust]
    samples: Vec<ScrollSample>,
    /// Where the notches of the wheel now turning have put the row, with
    /// none of the momentum they were given: what the next notch's momentum
    /// is measured from.
    #[rust]
    notched: f64,
    /// The whole row, the window the chips are seen through. Marked
    /// `#[area]` so `Widget::area()` reports it: without that the derive
    /// reports the last chip drawn, which is a different one every scroll.
    #[rust]
    #[area]
    area: Area,
}

impl FabPaletteCarousel {
    /// The palettes on offer, four colours each with the top band first, and
    /// which of them is in force.
    ///
    /// Silent when nothing moved, for [`FabDiagonalLabel::set_text`]'s
    /// reason: the host writes this on every draw, and a setter that dirtied
    /// the draw list each time would redraw the panel forever. When either
    /// DID move -- a new list, or another chip in force because the host
    /// changed its mind from outside -- the chip in force is brought into the
    /// window, since its outline is the one thing that says which palette is
    /// in force, and an outline off the edge of the window says nothing.
    pub fn set_chips(&mut self, cx: &mut Cx, chips: &[[Vec4f; 4]], chosen: Option<usize>) {
        let chosen = chosen.filter(|index| *index < chips.len());
        if self.chips.as_slice() == chips && self.chosen == chosen {
            return;
        }
        if self.chips.as_slice() != chips {
            self.chips = chips.to_vec();
            self.hot = None;
        }
        self.chosen = chosen;
        self.reveal_due = chosen.is_some();
        self.repaint(cx);
    }

    /// How many colours each chip shows, one to four, the top ones of the
    /// four it was handed. `chip_height` is a chip of all four, so a row of
    /// palettes of two is half as tall and every colour is still a square:
    /// the row takes the height of what it shows, and the host's layout
    /// closes up under it rather than keeping room for colours nobody chose.
    /// Silent when it is already that, like [`FabPaletteCarousel::set_chips`].
    pub fn set_bands(&mut self, cx: &mut Cx, bands: usize) {
        let bands = bands.clamp(1, 4);
        if bands != self.bands {
            self.bands = bands;
            self.repaint(cx);
        }
    }

    pub fn bands(&self) -> usize {
        self.bands
    }

    /// The height of a chip, and of the row: `chip_height` shared over the
    /// four and given to the bands shown.
    fn shown_height(&self) -> f64 {
        self.chip_height * self.bands as f64 / 4.0
    }

    /// Back to the first chip, for a list that is a new one rather than the
    /// old one grown again: somebody who changed the colour it is grown from
    /// is looking at a different row, and the middle of it is nowhere.
    pub fn rewind(&mut self, cx: &mut Cx) {
        self.scroll = 0.0;
        // No glide: there is nothing to follow from the old row to the new
        // one, and a picture sliding back through palettes that are already
        // gone would say the two rows were one.
        self.glide.land();
        self.repaint(cx);
    }

    pub fn chosen(&self) -> Option<usize> {
        self.chosen
    }

    pub fn len(&self) -> usize {
        self.chips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chips.is_empty()
    }

    /// The four colours chip `index` is showing, for a test that has to read
    /// the bands back off the control rather than off the state that wrote
    /// them.
    pub fn chip_colors(&self, index: usize) -> Option<[Vec4f; 4]> {
        self.chips.get(index).copied()
    }

    /// Where the row stands -- or where it is on its way to, while the
    /// drawing is still catching up with it (see [`ChipGlide`]). The number
    /// everything but the draw answers off.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Where the row is DRAWN this frame, which lags [`FabPaletteCarousel::scroll`]
    /// by a tenth of a second or so after every move. What a test of the
    /// motion reads, and what the chips are painted at.
    pub fn drawn_scroll(&self) -> f64 {
        self.glide.at(self.scroll)
    }

    /// Whether the drawing is still closing on the row's place, which is
    /// whether the row is asking for frames.
    pub fn is_gliding(&self) -> bool {
        self.glide.running()
    }

    /// Scroll to `scroll`, clamped to the row's ends, with no motion to
    /// watch: a host putting the row somewhere is not a gesture, and a row
    /// that glided every time its host placed it would be sliding about
    /// while somebody was reading it. The gestures --
    /// [`FabPaletteCarousel::scroll_page`], the wheel, a flick off a drag,
    /// the chip in force coming into view -- are the ones that glide.
    pub fn set_scroll(&mut self, cx: &mut Cx, scroll: f64) {
        let rect = self.area.rect(cx);
        let scroll = self.track(rect.size.x).clamp(scroll);
        if scroll != self.scroll || self.glide.running() {
            self.scroll = scroll;
            self.glide.land();
            self.repaint(cx);
        }
    }

    /// The row moved on by a page, or back by one: as many whole chips as
    /// the window holds ([`ChipTrack::page`]), clamped at the ends like every
    /// other way of moving it. What a host's arrow presses.
    ///
    /// It glides there. A second press while the first is still running is
    /// a page further on, measured from where the row is going rather than
    /// from where the picture has got to: two presses are two pages,
    /// whatever the hand's timing, and the row never doubles back.
    pub fn scroll_page(&mut self, cx: &mut Cx, forward: bool) {
        let track = self.track(self.window);
        let page = track.page();
        let scroll = track.clamp(self.scroll + if forward { page } else { -page });
        self.glide_to(cx, scroll, 0.0);
    }

    /// Whether the row stands at one of its ends, which is what an arrow
    /// pointing that way asks to know whether it has anything left to do. A
    /// row the window holds whole stands at both at once, so both arrows are
    /// off together.
    ///
    /// Off the window as last drawn, so that these answer the same arithmetic
    /// [`FabPaletteCarousel::scroll_page`] obeys: an arrow that is live and a
    /// press that moves nothing cannot both be right.
    ///
    /// And off where the row is GOING, not off the picture mid-glide: an
    /// arrow that answered for a moving picture would go out somewhere in
    /// the middle of its own glide and come back on at the end of it.
    pub fn at_start(&self) -> bool {
        self.scroll <= 0.0
    }

    pub fn at_end(&self) -> bool {
        // A hair's width of slack: the far end is reached by arithmetic on
        // the window's width, and an arrow left live over a row that cannot
        // move is a press that does nothing.
        self.scroll >= self.track(self.window).max_scroll() - 0.01
    }

    /// Bring chip `index` whole into the window, moving the row as little as
    /// that takes -- at once, like [`FabPaletteCarousel::set_scroll`], which
    /// is what it is: a host placing the row. The row bringing the chip in
    /// force into view OFF ITS OWN CHANGE glides instead; that one is the
    /// row answering something the person did, and they should see it
    /// happen.
    pub fn show_chip(&mut self, cx: &mut Cx, index: usize) {
        let rect = self.area.rect(cx);
        let scroll = self.track(rect.size.x).reveal(self.scroll, index);
        self.set_scroll(cx, scroll);
    }

    /// The furthest the row scrolls at the width it was last drawn.
    pub fn max_scroll(&self, cx: &Cx) -> f64 {
        self.track(self.area.rect(cx).size.x).max_scroll()
    }

    /// Where chip `index` stands on the screen, the part of it the window
    /// shows; None for a chip wholly outside the window, which is not drawn.
    pub fn chip_rect(&self, cx: &Cx, index: usize) -> Option<Rect> {
        let rect = self.area.rect(cx);
        if index >= self.chips.len() || rect.size.x <= 0.0 {
            return None;
        }
        let left = rect.pos.x + index as f64 * (self.chip_width + self.gap) - self.scroll;
        let from = left.max(rect.pos.x);
        let to = (left + self.chip_width).min(rect.pos.x + rect.size.x);
        (to > from).then(|| Rect {
            pos: dvec2(from, rect.pos.y),
            size: dvec2(to - from, self.shown_height().min(rect.size.y)),
        })
    }

    /// The chip under a point on the screen, and the part of it that shows.
    pub fn chip_at(&self, cx: &Cx, abs: Vec2d) -> Option<(usize, Rect)> {
        let rect = self.area.rect(cx);
        if !rect.contains(abs) {
            return None;
        }
        let index = self.track(rect.size.x).index_at(self.scroll, abs.x - rect.pos.x)?;
        Some((index, self.chip_rect(cx, index)?))
    }

    fn track(&self, view: f64) -> ChipTrack {
        ChipTrack {
            count: self.chips.len(),
            chip_width: self.chip_width,
            gap: self.gap,
            view,
        }
    }

    /// Move the row's place to `scroll` and leave the drawing to close the
    /// distance, opening at `velocity` points a second -- nought for a move
    /// nobody threw.
    fn glide_to(&mut self, cx: &mut Cx, scroll: f64, velocity: f64) {
        let moved = scroll - self.scroll;
        if moved == 0.0 {
            return;
        }
        self.scroll = scroll;
        self.glide.moved(moved, velocity);
        if self.glide.running() {
            self.next_frame = cx.new_next_frame();
        }
        self.repaint(cx);
    }

    /// Put the row at `scroll` with the picture on it: what a hand dragging
    /// it does, and what a wheel's own delta does, both of which are already
    /// as continuous as the hand moving them.
    fn place(&mut self, cx: &mut Cx, scroll: f64) {
        if scroll != self.scroll {
            self.scroll = scroll;
            self.repaint(cx);
        }
    }

    /// One frame of a glide in flight. Another is asked for only while
    /// there is more of it to draw, so a row at rest costs nothing: the
    /// last frame of a glide is the last frame the row asks for at all.
    fn tick(&mut self, cx: &mut Cx, time: f64) {
        if !self.glide.running() {
            return;
        }
        if self.glide.tick(time) {
            self.next_frame = cx.new_next_frame();
        }
        self.repaint(cx);
    }

    /// The row and every chip on it. The chips' own area alone is not
    /// enough: a list that has just emptied has no chip to redraw by.
    fn repaint(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.draw_chip.redraw(cx);
    }

    fn set_hot(&mut self, cx: &mut Cx, hot: Option<usize>) {
        if hot != self.hot {
            self.hot = hot;
            self.repaint(cx);
            cx.widget_action(self.uid, FabPaletteCarouselAction::Hover(hot));
        }
    }
}

impl Widget for FabPaletteCarousel {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        // The row is as tall as its chips, whatever height it was declared
        // at: a declared height is the height of a chip of four.
        let height = self.shown_height();
        let walk = match walk.height {
            Size::Fixed(_) => Walk { height: Size::Fixed(height), ..walk },
            _ => walk,
        };
        cx.begin_turtle(walk, Layout::flow_down());
        let width = cx.turtle().rect().size.x;
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(height)));
        self.window = rect.size.x.max(width);
        let track = self.track(self.window);
        // The width is new on every draw -- the sidebar is dragged wider, a
        // list comes in shorter -- so the scroll is clamped to what it is
        // now, and a chip owed a place in the window is given it here.
        let clamped = track.clamp(self.scroll);
        if clamped != self.scroll {
            // A place the layout decided, not a gesture: the picture goes
            // there with it rather than sliding in from where the row used
            // to be able to stand.
            self.scroll = clamped;
            self.glide.land();
        }
        if self.reveal_due {
            self.reveal_due = false;
            if let Some(index) = self.chosen {
                let to = track.reveal(self.scroll, index);
                let moved = to - self.scroll;
                if moved != 0.0 {
                    self.scroll = to;
                    // This one glides: the chip in force has changed under
                    // somebody's hand, and a row that jumped would leave
                    // them looking for which chip moved where.
                    self.glide.moved(moved, 0.0);
                }
            }
        }
        // The frames that carry a glide are asked for here as well as from
        // the tick, because the reveal above opens one mid-draw; and only
        // while one is running, so a row at rest is a row that has stopped
        // asking.
        if self.glide.running() {
            self.next_frame = cx.new_next_frame();
        }
        let at = self.glide.at(self.scroll);
        cx.push_clip_rect(rect);
        let pitch = track.pitch();
        for index in track.in_view(at) {
            let bands = self.chips[index];
            self.draw_chip.band_0 = bands[0];
            self.draw_chip.band_1 = bands[1];
            self.draw_chip.band_2 = bands[2];
            self.draw_chip.band_3 = bands[3];
            self.draw_chip.bands = self.bands as f32;
            self.draw_chip.hover = if self.hot == Some(index) { 1.0 } else { 0.0 };
            self.draw_chip.cur = if self.chosen == Some(index) { 1.0 } else { 0.0 };
            self.draw_chip.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + index as f64 * pitch - at, rect.pos.y),
                    size: dvec2(self.chip_width, height),
                },
            );
        }
        cx.pop_clip_rect();
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    /// Through `hits`, so that a press anywhere else in the host holds the
    /// pointer and this row neither lights nor answers while it does; and
    /// the press it does take holds the pointer against everything else
    /// until it is let go.
    ///
    /// The choice is made on the release, over the chip the press landed on,
    /// the way every button is -- and only if the row did not move: a hand
    /// that dragged the row along was looking for a palette, not choosing
    /// the one that happened to end up under it.
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        if let Some(ne) = self.next_frame.is_event(event) {
            self.tick(cx, ne.time);
        }
        let uid = self.widget_uid();
        let rect = self.area.rect(cx);
        let track = self.track(rect.size.x);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = track.index_at(self.scroll, fe.abs.x - rect.pos.x);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                self.set_hot(cx, hot);
            }
            Hit::FingerHoverOut(_) => {
                if self.press.is_none() {
                    self.set_hot(cx, None);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.area);
                // A hand on a moving row stops it where it stands: the
                // row's place comes back to what was drawn, so from the
                // touch on the picture and the place are one number and the
                // chip under the finger is the chip drawn there.
                let caught = self.glide.running();
                if caught {
                    self.scroll = self.glide.at(self.scroll);
                    self.glide.land();
                    self.repaint(cx);
                }
                self.samples.clear();
                push_sample(&mut self.samples, fe.abs.x, fe.time);
                self.press = Some(CarouselPress {
                    abs_x: fe.abs.x,
                    scroll: self.scroll,
                    dragged: false,
                    caught,
                });
            }
            Hit::FingerMove(fe) => {
                let Some(mut press) = self.press else {
                    return;
                };
                push_sample(&mut self.samples, fe.abs.x, fe.time);
                let moved = fe.abs.x - press.abs_x;
                if !press.dragged && moved.abs() > CAROUSEL_DRAG_SLOP {
                    press.dragged = true;
                    cx.set_cursor(MouseCursor::Grabbing);
                    self.set_hot(cx, None);
                }
                if press.dragged {
                    // The row goes with the hand, one point for one:
                    // dragging right brings the earlier chips back into the
                    // window.
                    self.place(cx, track.clamp(press.scroll - moved));
                }
                self.press = Some(press);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                let Some(press) = self.press.take() else {
                    return;
                };
                if press.dragged {
                    // The flick: the row leaves the finger at the speed the
                    // finger had and runs down to rest, clamped at the ends
                    // like every other way of moving it. A hand that came to
                    // a stop before it let go leaves no speed behind it (see
                    // [`carousel_flick`]), so a careful drag stays put.
                    push_sample(&mut self.samples, fe.abs.x, fe.time);
                    let (velocity, travel) = estimate_release_velocity(&self.samples);
                    let carry = carousel_flick(velocity, travel);
                    if carry != 0.0 {
                        self.glide_to(cx, track.clamp(self.scroll + carry), -velocity);
                    }
                    return;
                }
                if press.caught || !fe.is_over {
                    return;
                }
                if let Some(index) = track.index_at(self.scroll, fe.abs.x - rect.pos.x) {
                    cx.widget_action(uid, FabPaletteCarouselAction::Pick(index));
                }
            }
            Hit::FingerScroll(fs) => {
                // A stream that has gone quiet is over; what comes after it
                // is a new turn of the wheel and is measured on its own.
                if self.samples.last().map_or(true, |last| fs.time - last.time > FLING_SAMPLE_MAX_AGE) {
                    self.samples.clear();
                    self.notched = self.scroll;
                }
                let delta = carousel_wheel(fs.scroll.x, fs.scroll.y);
                let notched = track.clamp(self.notched + delta);
                let moved = notched - self.notched;
                self.notched = notched;
                if moved != 0.0 {
                    // The delta itself lands whole: a wheel and a trackpad
                    // are already as continuous as the hand on them, and a
                    // row that eased into every delta would trail the pad.
                    self.place(cx, track.clamp(self.scroll + moved));
                    // What is under the pointer moved; the tooltip and the
                    // ring follow it.
                    let hot = track.index_at(self.scroll, fs.abs.x - rect.pos.x);
                    self.set_hot(cx, hot);
                }
                // What the wheel is doing, taken off the row's own travel:
                // a notch or two rolled slowly leaves nothing behind it, and
                // a spin runs on a little and settles instead of stopping
                // dead where the last notch left it.
                //
                // Measured from where the NOTCHES have put the row, so the
                // momentum is one carry and not one per notch, and taken
                // only while the spin is still gathering speed: a wheel
                // slowing down must not pull the row back towards itself.
                push_sample(&mut self.samples, self.notched, fs.time);
                let (velocity, _) = estimate_release_velocity(&self.samples);
                let pitch = track.pitch() * CHIP_WHEEL_CARRY;
                let carry =
                    chip_spin_travel(velocity, FLING_DECEL_RATE_PER_MS).clamp(-pitch, pitch);
                let target = track.clamp(self.notched + carry);
                if (target - self.scroll) * carry > 0.0 {
                    self.glide_to(cx, target, velocity);
                }
            }
            Hit::KeyDown(ke) => {
                let scroll = match ke.key_code {
                    KeyCode::ArrowLeft => self.scroll - track.pitch(),
                    KeyCode::ArrowRight => self.scroll + track.pitch(),
                    KeyCode::Home => 0.0,
                    KeyCode::End => track.max_scroll(),
                    _ => return,
                };
                self.glide_to(cx, track.clamp(scroll), 0.0);
            }
            _ => {}
        }
    }
}

// ===========================================================================
// FabColorPick — a swatch that opens a self-managed popover (wheel + RGBA
// rows + hex). No shell bus: the popover draws in an overlay draw list
// anchored at the swatch, outside-click commits, Escape reverts.
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFabSwatch {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub hover: f32,
    #[live]
    pub open: f32,
    /// The colour has been carried off this square (see
    /// [`FabColorPick::draggable`]).
    #[live]
    pub lifted: f32,
    /// A carried colour would land on this square if let go now.
    #[live]
    pub target: f32,
    #[live]
    pub swatch: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub enum FabColorPickAction {
    /// Live: the bound value should follow immediately (rgba 0..1).
    Changed(Vec4f),
    /// Commit (release / Enter / outside-click close). Escape publishes
    /// `Changed(original)` then `Ended(original)`.
    Ended(Vec4f),
    Opened,
    Closed,
    /// The popover's `pick` button: sample a colour from the app — the
    /// host owns the eyedropper (it knows the window), the popover closes.
    Eyedropper,
    /// The pointer rests on a palette cell (its name) or left the strip.
    PaletteHover(Option<String>),
    /// A palette cell was clicked: the host binds the property to the
    /// named colour; the popover has closed without publishing a value.
    PalettePick(String),
    /// A press on a [`FabColorPick::draggable`] swatch travelled past the
    /// slop: the colour is being carried, and no popover will open.
    DragStarted,
    /// The carried colour's pointer, window-local.
    DragMoved(DVec2),
    /// Let go here. What lands where is the host's to decide.
    DragDropped(DVec2),
    /// Escape, Back, the other button or the window going away: the carry
    /// is off and nothing is to change.
    DragCancelled,
    #[default]
    None,
}

// Hand-written `WidgetNode` (the `Widget` derive owns that impl): the open
// popover's controls surface as children so the remote bridge and the
// design tweaker's walks can reach the `pick` button and the fields.
#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct FabColorPick {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_swatch: DrawFabSwatch,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// Whether the colour has a transparency at all.
    ///
    /// Off, the A row is not drawn, the colour is held fully opaque however it
    /// arrives — dragged onto the square, sampled with the eyedropper, typed
    /// as an eight-figure hex — and the hex field reads and writes six
    /// figures. A theme colour has no transparency: a square that quietly
    /// carried an alpha of 20 was showing `#92755014` to somebody who never
    /// asked for an alpha and had no row to put it back with.
    #[live(true)]
    with_alpha: bool,
    /// The popover panel (wheel + rows + hex), from the type default.
    #[live]
    popover: View,
    #[rust]
    overlay_list: Option<DrawList2d>,
    #[rust]
    open: bool,
    /// Held while the popover is open, so `Escape` reverts this picker rather
    /// than dismissing whatever it was opened in front of.
    #[rust]
    cancel_scope: Option<CancelScope>,
    #[rust]
    hsv: [f32; 3],
    #[rust(1.0)]
    alpha: f32,
    /// The colour when the popover opened — restored by Escape. Held twice:
    /// as the RGBA that went out, and as the HSV that was behind it, because
    /// the hue of a grey is not in the RGBA.
    #[rust]
    opened_value: [f32; 4],
    #[rust]
    opened_hsv: [f32; 3],
    #[rust]
    panel_rect: Rect,
    #[rust]
    sync_pending: bool,
    /// Names of the palette entries, in strip order.
    #[rust]
    palette_names: Vec<String>,
    /// The popover holds the sweep lock (see [`FabColorPick::lock`]).
    #[rust]
    locked: bool,
    /// The area the lock was taken with: the swatch's, as it was then.
    #[rust]
    lock_area: Area,
    /// The colour can be carried off the swatch to somewhere else. Off by
    /// default, because a swatch that carries has to open its popover on
    /// the release instead of the press -- it cannot know before the
    /// pointer has moved or not which of the two the press was -- and every
    /// picker that never carries keeps the press it has always had.
    ///
    /// Only the gesture lives here: a press that travels past
    /// [`KNOB_DRAG_SLOP`] reports `DragStarted`, the pointer while it moves
    /// and where it was let go, and a lifted copy of the swatch follows the
    /// pointer sideways. What the drop means is the host's, which knows
    /// where the other squares are.
    #[live]
    draggable: bool,
    /// The copy of the swatch that rides under the pointer.
    #[live]
    draw_lifted: DrawFabSwatch,
    /// The bar that marks a gap the carried colour would be slotted into
    /// (see [`FabColorPick::set_insert_bar`]).
    #[live]
    draw_insert: DrawColor,
    /// Where that bar stands, window-local, while the host says the colour
    /// would go in between rather than onto another square.
    #[rust]
    insert_bar: Option<Rect>,
    /// A press on a draggable swatch that has not been let go: where it
    /// landed, and whether the popover was up when it did (a press that shut
    /// the popover does not open it again on the release).
    #[rust]
    press: Option<(DVec2, bool)>,
    /// The press has travelled past the slop and is carrying the colour.
    #[rust]
    carrying: bool,
    /// Where the pointer is while it carries.
    #[rust]
    carry_at: DVec2,
    /// Where on the swatch the press took hold, so the copy hangs from the
    /// pointer at that spot rather than jumping to centre on it.
    #[rust]
    grab: DVec2,
    /// Held while the colour is carried, so Escape calls the carry off
    /// rather than whatever the swatch sits in front of.
    #[rust]
    carry_scope: Option<CancelScope>,
}

/// A picker dropped while it is open (its page rebuilt on a theme or story
/// switch) cannot let go of the pointer itself: a drop has no `Cx`, and a
/// lock nobody holds turns every hit in the window away. The next event
/// lets go of it.
impl Drop for FabColorPick {
    fn drop(&mut self) {
        if self.locked {
            crate::family_api::orphan_sweep_locks(&[self.lock_area, self.draw_swatch.area()]);
        }
    }
}

impl ScriptHook for FabColorPick {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay_list = Some(DrawList2d::script_new(vm));
    }
}

impl FabColorPick {
    /// The colour the square is DRAWN in: the quad's own instance, which is
    /// what a test asking what is on the screen has to read. It is written
    /// by [`FabColorPick::set_rgba`] and by the popover, so a control the
    /// host forgot to write answers with what it last drew and not with what
    /// the host meant.
    pub fn drawn_swatch(&self) -> Vec4f {
        self.draw_swatch.swatch
    }

    pub fn rgba(&self) -> [f32; 4] {
        let [h, s, v] = self.hsv;
        let [r, g, b] = hsv_to_rgb(h, s, v);
        [r, g, b, self.opacity()]
    }

    /// The alpha this picker reports. Without the A row there is no such
    /// thing as a partly transparent colour here, so one never leaves: a
    /// value that arrived carrying an alpha is made opaque rather than kept
    /// out of sight where nothing can put it right.
    fn opacity(&self) -> f32 {
        if self.with_alpha {
            self.alpha
        } else {
            1.0
        }
    }

    /// Take a colour that arrived as RGB, KEEPING the hue and saturation the
    /// person set where the new colour has none of its own.
    ///
    /// HSV plus alpha is the one thing the picker holds; the wheel, the seven
    /// rows and the hex field are all views of it. That is deliberate: a
    /// round trip through RGB throws away the hue of every grey and the hue
    /// and saturation of every black, because `rgb_to_hsv` has nowhere to put
    /// them and answers 0. Pulled S down to nothing and back up, a colour
    /// that came back through RGB would come back red; pulled V down to
    /// black and back up, the same. So the undefined channels are not read
    /// out of the new colour, they are left where the hand had them.
    fn adopt_rgb(&mut self, rgb: [f32; 3]) {
        let [h, s, v] = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
        self.hsv = [
            if s <= 0.0 || v <= 0.0 { self.hsv[0] } else { h },
            if v <= 0.0 { self.hsv[1] } else { s },
            v,
        ];
    }

    pub fn set_rgba(&mut self, cx: &mut Cx, rgba: [f32; 4]) {
        self.adopt_rgb([rgba[0], rgba[1], rgba[2]]);
        self.alpha = rgba[3];
        let rgba = self.rgba();
        self.draw_swatch.swatch = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
        self.draw_swatch.redraw(cx);
        if self.open {
            self.sync_pending = true;
            self.sync_widgets(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the colour is being carried off the swatch.
    pub fn is_carrying(&self) -> bool {
        self.carrying
    }

    /// Light the swatch as the place a carried colour would land. The host
    /// says so, because only the host knows which square is under the
    /// pointer.
    pub fn set_drop_target(&mut self, cx: &mut Cx, on: bool) {
        let target = if on { 1.0 } else { 0.0 };
        if self.draw_swatch.target != target {
            self.draw_swatch.target = target;
            self.draw_swatch.redraw(cx);
        }
    }

    pub fn is_drop_target(&self) -> bool {
        self.draw_swatch.target > 0.0
    }

    /// Stand an insertion bar at `bar` (window-local) while this swatch's
    /// colour is carried, or take it down with `None`. The host says where,
    /// because the gaps are between squares only it knows of; the carried
    /// swatch draws it, because the carry's overlay floats over the row and
    /// the gap it marks belongs to no square.
    pub fn set_insert_bar(&mut self, cx: &mut Cx, bar: Option<Rect>) {
        if self.insert_bar != bar {
            self.insert_bar = bar;
            self.redraw_carry(cx);
        }
    }

    pub fn insert_bar(&self) -> Option<Rect> {
        self.insert_bar
    }

    /// The press has travelled: the colour is off the swatch.
    fn start_carry(&mut self, cx: &mut Cx, at: DVec2) {
        self.carrying = true;
        self.carry_at = at;
        self.carry_scope = Some(self.begin_cancel_scope(cx));
        self.draw_swatch.lifted = 1.0;
        let uid = self.widget_uid();
        cx.widget_action(uid, FabColorPickAction::DragStarted);
        cx.widget_action(uid, FabColorPickAction::DragMoved(at));
        self.redraw_carry(cx);
    }

    /// The carry over, however it ended: the press forgotten, the swatch
    /// back, the copy gone. Says nothing; the caller names the ending.
    fn end_carry(&mut self, cx: &mut Cx) {
        self.press = None;
        self.carrying = false;
        self.carry_scope = None;
        self.draw_swatch.lifted = 0.0;
        // A bar outliving the carry would mark a gap nothing is going into.
        self.insert_bar = None;
        self.redraw_carry(cx);
    }

    /// Called off: nothing is to change.
    fn cancel_carry(&mut self, cx: &mut Cx) {
        let was = self.carrying;
        self.end_carry(cx);
        if was {
            cx.widget_action(self.widget_uid(), FabColorPickAction::DragCancelled);
        }
    }

    fn redraw_carry(&mut self, cx: &mut Cx) {
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
        self.draw_swatch.redraw(cx);
        // The copy floats in the window's overlay, over whatever the pointer
        // crosses, and that has to be drawn again where the copy has left.
        cx.redraw_all();
    }

    /// Take the pointer for the open popover.
    ///
    /// The popover floats over widgets that are walked before it, the app's
    /// buttons and the panel's rows, and with nothing to stop them they took
    /// hovers and presses through it: a drag on the wheel that began over a
    /// button under it pressed the button and never reached the wheel. The
    /// lock turns away every hit test that does not name this picker, the
    /// way a drop-down's list does; the popover's own controls are let
    /// through while it hands them the event (see `handle_event`).
    ///
    /// Taken with the swatch's area, which exists from the moment the
    /// swatch has been drawn once. The popover's own area does not exist
    /// until the popover has drawn, and a lock on an empty area stops
    /// nothing. A swatch that was never drawn takes it on its first draw.
    fn lock(&mut self, cx: &mut Cx) {
        let area = self.draw_swatch.area();
        if self.locked || area.is_empty() {
            return;
        }
        crate::family_api::release_orphaned_sweep_locks(cx);
        cx.sweep_lock(area);
        self.lock_area = area;
        self.locked = true;
    }

    /// Let go of the pointer, and only of the lock this picker took. Both
    /// areas: a redraw moves the lock to the swatch's newer area, and a
    /// swatch drawn into another slot since keeps the old one in the stack.
    fn unlock(&mut self, cx: &mut Cx) {
        if self.locked {
            cx.sweep_unlock(self.lock_area);
            cx.sweep_unlock(self.draw_swatch.area());
            self.locked = false;
        }
    }

    /// Whether this picker's lock is the innermost one: an overlay opened
    /// above the popover takes the pointer from it, and this one must not
    /// take it back.
    fn lock_on_top(&self, cx: &Cx) -> bool {
        let top = cx.sweep_lock_area();
        top == Some(self.lock_area) || top == Some(self.draw_swatch.area())
    }

    /// The popover's palette strip: named colours in display order.
    pub fn set_palette(&mut self, cx: &mut Cx, entries: Vec<(String, [f32; 4])>) {
        let colors = entries.iter().map(|(_, c)| *c).collect();
        self.palette_names = entries.into_iter().map(|(n, _)| n).collect();
        if let Some(mut strip) = self.popover.child(live_id!(palette)).borrow_mut::<FabPaletteStrip>() {
            strip.set_colors(cx, colors);
        }
        self.sync_pending = true;
    }

    fn palette_label(&self, cx: &mut Cx, hot: Option<usize>) {
        let text = match hot.and_then(|i| self.palette_names.get(i)) {
            Some(name) => format!("theme.{name}"),
            None if self.palette_names.is_empty() => String::new(),
            None => format!("theme colours ({})", self.palette_names.len()),
        };
        self.popover.child(live_id!(palette_name)).set_text(cx, &text);
    }

    /// Close without publishing: the host is about to bind the property
    /// to a palette reference, and a `Changed` would ledger a hex first.
    fn close_quiet(&mut self, cx: &mut Cx) {
        if !self.open {
            return;
        }
        let uid = self.widget_uid();
        self.popover.handle_event(cx,
            &Event::Actions(vec![Box::new(crate::modal::ModalAction::Dismissed)]),
            &mut Scope::empty());
        self.open = false;
        self.unlock(cx);
        self.cancel_scope = None;
        self.draw_swatch.open = 0.0;
        cx.widget_action(uid, FabColorPickAction::Closed);
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
        self.draw_swatch.redraw(cx);
        cx.redraw_all();
    }

    fn publish(&mut self, cx: &mut Cx, uid: WidgetUid, ended: bool) {
        let rgba = self.rgba();
        self.draw_swatch.swatch = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
        self.draw_swatch.redraw(cx);
        let value = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
        cx.widget_action(uid, FabColorPickAction::Changed(value));
        if ended {
            cx.widget_action(uid, FabColorPickAction::Ended(value));
        }
    }

    /// Push the state into every control (wheel, rows, hex).
    fn sync_widgets(&mut self, cx: &mut Cx) {
        let [h, s, v] = self.hsv;
        let rgba = self.rgba();
        if let Some(mut wheel) = self.popover.child(live_id!(wheel)).borrow_mut::<FabColorWheel>()
        {
            wheel.set_hsv(cx, h, s, v);
        }
        // Every row from the one truth, never from another row: the four
        // bytes are derived, the three HSV numbers are the truth in the
        // units the rows print it in. A row that is being dragged or typed
        // into refuses the write itself, so nothing stamps on the hand.
        let nums = [
            (live_id!(num_r), (rgba[0] * 255.0) as f64),
            (live_id!(num_g), (rgba[1] * 255.0) as f64),
            (live_id!(num_b), (rgba[2] * 255.0) as f64),
            (live_id!(num_a), (rgba[3] * 255.0) as f64),
            (live_id!(num_h), (h * 360.0) as f64),
            (live_id!(num_s), (s * 100.0) as f64),
            (live_id!(num_v), (v * 100.0) as f64),
        ];
        for (id, channel) in nums {
            if id == live_id!(num_a) && !self.with_alpha {
                continue;
            }
            if let Some(mut num) = self.popover.child(id).borrow_mut::<FabValueInput>() {
                num.set_value(cx, channel);
            }
        }
        let hex = self.popover.child(live_id!(hex_row)).child(live_id!(hex));
        if !hex.is_empty() {
            // Don't stomp the hex text while the person is typing in it, and
            // don't re-shape the run for a string it is already showing (see
            // `FabValueInput::sync_text` for what a `set_text` costs).
            if hex.area() == Area::Empty || !cx.has_key_focus(hex.area()) {
                let text = format_hex(rgba, self.with_alpha);
                if hex.text() != text {
                    hex.set_text(cx, &text);
                }
            }
        }
        if let Some(mut strip) = self.popover.child(live_id!(palette)).borrow_mut::<FabPaletteStrip>() {
            let byte = |v: f32| (v * 255.0).round() as i32;
            let current = strip
                .colors
                .iter()
                .position(|c| (0..4).all(|k| byte(c[k]) == byte(rgba[k])));
            strip.set_current(cx, current);
            let hot = strip.hot;
            drop(strip);
            self.palette_label(cx, hot);
        }
    }

    pub fn open_popover(&mut self, cx: &mut Cx) {
        if self.open {
            return;
        }
        self.open = true;
        self.lock(cx);
        self.cancel_scope = Some(self.begin_cancel_scope(cx));
        self.opened_value = self.rgba();
        self.opened_hsv = self.hsv;
        self.draw_swatch.open = 1.0;
        self.sync_pending = true;
        let uid = self.widget_uid();
        cx.widget_action(uid, FabColorPickAction::Opened);
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
        self.draw_swatch.redraw(cx);
        // The popover's controls join the widget tree under this swatch
        // while it is open, so the remote bridge (/snap) and the tweaker's
        // tree walks can reach the `pick` button and the fields.
        let uid = self.uid;
        let mut kids = Vec::new();
        self.popover.children(&mut |id, w| kids.push((id, w)));
        for (id, w) in kids {
            cx.widget_tree_insert_child_deep(uid, id, w);
        }
    }

    pub fn close_popover(&mut self, cx: &mut Cx, revert: bool) {
        if !self.open {
            return;
        }
        self.popover.handle_event(cx,
            &Event::Actions(vec![Box::new(crate::modal::ModalAction::Dismissed)]),
            &mut Scope::empty());
        let uid = self.widget_uid();
        if revert {
            // Put back the colour AS IT WAS HELD, hue and all: a revert
            // through RGB would hand a grey back with its hue lost.
            self.hsv = self.opened_hsv;
            self.alpha = self.opened_value[3];
            self.publish(cx, uid, true);
        } else {
            self.publish(cx, uid, true);
        }
        self.open = false;
        self.unlock(cx);
        self.cancel_scope = None;
        self.draw_swatch.open = 0.0;
        cx.widget_action(uid, FabColorPickAction::Closed);
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
        self.draw_swatch.redraw(cx);
        cx.redraw_all();
    }

    /// One of the popover's channel rows by name -- `num_r`, `num_h` and the
    /// rest. Empty before the popover has been built. For a host that has to
    /// reach past the control to the row itself, which is what a test of how
    /// a press reaches a row needs.
    pub fn channel_row(&self, id: LiveId) -> WidgetRef {
        self.popover.child(id)
    }

    /// The open popover's window-local rect (zero when closed). The panel
    /// host uses it to give the popup input priority over its scroll list.
    pub fn popover_rect(&self) -> Rect {
        if self.open {
            self.panel_rect
        } else {
            Rect::default()
        }
    }
}

impl WidgetNode for FabColorPick {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn set_action_data(&mut self, _action_data: std::sync::Arc<dyn ActionTrait>) {}

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        None
    }

    fn area(&self) -> Area {
        self.draw_swatch.area()
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_swatch.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if self.open {
            self.popover.children(visit);
        }
    }
}

impl Widget for FabColorPick {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rgba = self.rgba();
        self.draw_swatch.swatch = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
        self.draw_swatch.draw_walk(cx, walk);
        if self.open {
            // Opened before the swatch had ever drawn: no area to lock with
            // until now.
            self.lock(cx);
            let anchor = self.draw_swatch.area().rect(cx);
            let overlay_list = self.overlay_list.as_mut().unwrap();
            overlay_list.begin_overlay_reuse(cx);
            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_down());
            // Anchor under the swatch; clamp into the pass, flip above when
            // the bottom would overflow.
            let width = 244.0_f64;
            let strip_height = self
                .popover
                .child(live_id!(palette))
                .borrow::<FabPaletteStrip>()
                .map_or(0.0, |s| s.height_for(width - 16.0));
            // The A row is not drawn when this picker has no alpha, so it is
            // counted rather than assumed: the estimate decides which side of
            // the swatch the popover hangs from, and one row too many near
            // the foot of the window flips it over for nothing.
            let a_row = self.popover.child(live_id!(num_a));
            if a_row.visible() != self.with_alpha {
                a_row.set_visible(cx, self.with_alpha);
            }
            let rows = if self.with_alpha { 8.0 } else { 7.0 };
            // Tall enough to decide which side of the swatch to hang from,
            // counted rather than guessed: 8 of padding at each end, the
            // wheel, then the channel rows and the hex line at `row_height`,
            // with 6 of spacing between every child.
            let est_height = 16.0
                + 228.0
                + rows * 24.0
                + rows * 6.0
                + if strip_height > 0.0 { strip_height + 26.0 } else { 0.0 };
            let mut pos = dvec2(anchor.pos.x + anchor.size.x - width, anchor.pos.y + anchor.size.y + 2.0);
            if pos.y + est_height > pass_size.y {
                pos.y = (anchor.pos.y - est_height - 2.0).max(0.0);
            }
            pos.x = pos.x.clamp(0.0, (pass_size.x - width).max(0.0));
            let mut panel_walk = Walk::fit();
            panel_walk.abs_pos = Some(pos);
            panel_walk.width = Size::Fixed(width);
            // Push state into the controls BEFORE they draw: a redraw
            // requested during the draw event is dropped, so a sync after
            // the draw only showed on the next unrelated redraw.
            if self.sync_pending {
                self.sync_pending = false;
                self.sync_widgets(cx);
            }
            let _ = self.popover.draw_walk(cx, scope, panel_walk);
            // The UNCLIPPED rect: the popover draws in an overlay above
            // every clip, but `clipped_rect` intersects the host row's
            // clip stack and came back zero inside a scroll list — which
            // made the first outside-press logic close the popover on ANY
            // press ("the popup cannot be manipulated").
            self.panel_rect = self.popover.area().rect(cx);
            cx.end_pass_sized_turtle();
            self.overlay_list.as_mut().unwrap().end(cx);
        } else if self.carrying {
            // The carried copy, in the same overlay the popover would use
            // (the two are never up together: a carry shuts the popover).
            // It follows the pointer sideways and keeps the swatch's line,
            // because the squares it can land on are a row.
            let anchor = self.draw_swatch.area().rect(cx);
            let overlay_list = self.overlay_list.as_mut().unwrap();
            overlay_list.begin_overlay_reuse(cx);
            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_down());
            let rgba = self.rgba();
            self.draw_lifted.swatch = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
            self.draw_lifted.hover = 1.0;
            // Lifted a little off the row and a little narrower than the
            // squares, so the ring of the square it would land on shows round
            // it rather than under it.
            let inset = 4.0_f64.min(anchor.size.x * 0.1);
            let mut lifted = Walk::fixed(anchor.size.x - 2.0 * inset, anchor.size.y);
            lifted.abs_pos = Some(dvec2(self.carry_at.x - self.grab.x + inset, anchor.pos.y - 6.0));
            self.draw_lifted.draw_walk(cx, lifted);
            // After the copy, because the copy hangs over the very gap the
            // pointer is in, and a bar under it would be hidden by it.
            if let Some(bar) = self.insert_bar {
                self.draw_insert.draw_abs(cx, bar);
            }
            cx.end_pass_sized_turtle();
            self.overlay_list.as_mut().unwrap().end(cx);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.open && crate::family_api::is_modal_dismissal(event) {
            self.close_popover(cx, true);
            return;
        }

        if self.open {
            // Only the owner can consume Back or act on Escape.
            if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                && (matches!(event, Event::KeyDown(ke) if ke.key_code == KeyCode::Escape)
                    || event.back_pressed())
            {
                self.close_popover(cx, true);
                return;
            }
            // A press outside the panel and the swatch commits and closes,
            // and the press is the popover's: what was walked before this
            // saw the lock and nothing else, and what is walked after would
            // see no lock, now it is released, and take the press as its
            // own. Only one of the two halves of the page would have heard
            // it, depending on the order it is walked in. Read as it came in
            // rather than through a hit test, because the lock turns this
            // picker's own hit tests away everywhere but on the swatch.
            if let Event::MouseDown(me) = event {
                let swatch_rect = self.draw_swatch.area().rect(cx);
                if !self.panel_rect.contains(me.abs) && !swatch_rect.contains(me.abs) {
                    let on_top = !self.locked || self.lock_on_top(cx);
                    self.close_popover(cx, false);
                    if on_top && me.handled.get().is_empty() {
                        me.handled.set(self.draw_swatch.area());
                    }
                    return;
                }
            }
            // The popover's own controls hit-test with no sweep area of their
            // own, so the lock would turn them away with everything else. It
            // is lifted while they are handed the event and taken again after,
            // with the swatch's area as it is now; and only when it is the
            // innermost lock, because under an overlay opened above this one
            // they are to be turned away.
            let held = self.locked && self.lock_on_top(cx);
            if held {
                cx.sweep_unlock(self.lock_area);
                cx.sweep_unlock(self.draw_swatch.area());
            }
            let popover_actions = cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope));
            if held && self.locked {
                let area = self.draw_swatch.area();
                cx.sweep_lock(area);
                self.lock_area = area;
            }
            let mut changed = false;
            let mut ended = false;
            for action in popover_actions {
                let Some(widget_action) = action.as_widget_action() else {
                    continue;
                };
                let wheel_uid = self.popover.child(live_id!(wheel)).widget_uid();
                let r_uid = self.popover.child(live_id!(num_r)).widget_uid();
                let g_uid = self.popover.child(live_id!(num_g)).widget_uid();
                let b_uid = self.popover.child(live_id!(num_b)).widget_uid();
                let a_uid = self.popover.child(live_id!(num_a)).widget_uid();
                let h_uid = self.popover.child(live_id!(num_h)).widget_uid();
                let s_uid = self.popover.child(live_id!(num_s)).widget_uid();
                let v_uid = self.popover.child(live_id!(num_v)).widget_uid();
                let hex_uid = self
                    .popover
                    .child(live_id!(hex_row))
                    .child(live_id!(hex))
                    .widget_uid();
                let pick_uid = self
                    .popover
                    .child(live_id!(hex_row))
                    .child(live_id!(pick))
                    .widget_uid();
                if widget_action.widget_uid == pick_uid {
                    if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                        let uid = self.widget_uid();
                        self.close_popover(cx, false);
                        cx.widget_action(uid, FabColorPickAction::Eyedropper);
                    }
                    continue;
                }
                let strip_uid = self.popover.child(live_id!(palette)).widget_uid();
                if widget_action.widget_uid == strip_uid {
                    match widget_action.cast::<FabPaletteAction>() {
                        FabPaletteAction::Hover(hot) => {
                            self.palette_label(cx, hot);
                            let name = hot.and_then(|i| self.palette_names.get(i).cloned());
                            cx.widget_action(uid, FabColorPickAction::PaletteHover(name));
                        }
                        FabPaletteAction::Pick(i) => {
                            if let Some(name) = self.palette_names.get(i).cloned() {
                                let color = self
                                    .popover
                                    .child(live_id!(palette))
                                    .borrow::<FabPaletteStrip>()
                                    .and_then(|s| s.colors.get(i).copied());
                                if let Some(c) = color {
                                    self.adopt_rgb([c[0], c[1], c[2]]);
                                    self.alpha = c[3];
                                    self.draw_swatch.swatch = vec4(c[0], c[1], c[2], c[3]);
                                }
                                self.close_quiet(cx);
                                cx.widget_action(uid, FabColorPickAction::PaletteHover(None));
                                cx.widget_action(uid, FabColorPickAction::PalettePick(name));
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if widget_action.widget_uid == wheel_uid {
                    match widget_action.cast::<ColorWheelAction>() {
                        ColorWheelAction::Changed(hsv) => {
                            self.hsv = hsv;
                            changed = true;
                        }
                        ColorWheelAction::Ended(hsv) => {
                            self.hsv = hsv;
                            changed = true;
                            ended = true;
                        }
                        _ => {}
                    }
                } else if widget_action.widget_uid == r_uid
                    || widget_action.widget_uid == g_uid
                    || widget_action.widget_uid == b_uid
                    || widget_action.widget_uid == a_uid
                {
                    let (value, is_ended) = match widget_action.cast::<FabValueInputAction>() {
                        FabValueInputAction::Changed(v) => (Some(v), false),
                        FabValueInputAction::Ended(v) => (Some(v), true),
                        _ => (None, false),
                    };
                    if let Some(v) = value {
                        let channel = (v / 255.0).clamp(0.0, 1.0) as f32;
                        if widget_action.widget_uid == a_uid {
                            self.alpha = channel;
                        } else {
                            // One byte moved; the other two come out of the
                            // colour the picker holds, and the whole goes
                            // back in through `adopt_rgb` so a grey keeps
                            // the hue the hand gave it.
                            let rgba = self.rgba();
                            let mut rgb = [rgba[0], rgba[1], rgba[2]];
                            if widget_action.widget_uid == r_uid {
                                rgb[0] = channel;
                            } else if widget_action.widget_uid == g_uid {
                                rgb[1] = channel;
                            } else {
                                rgb[2] = channel;
                            }
                            self.adopt_rgb(rgb);
                        }
                        changed = true;
                        ended |= is_ended;
                    }
                } else if widget_action.widget_uid == h_uid
                    || widget_action.widget_uid == s_uid
                    || widget_action.widget_uid == v_uid
                {
                    // Straight into the truth, with no colour space crossed
                    // on the way: this is why S and V may be taken to zero
                    // and brought back without the hue moving.
                    let (value, is_ended) = match widget_action.cast::<FabValueInputAction>() {
                        FabValueInputAction::Changed(v) => (Some(v), false),
                        FabValueInputAction::Ended(v) => (Some(v), true),
                        _ => (None, false),
                    };
                    if let Some(v) = value {
                        if widget_action.widget_uid == h_uid {
                            self.hsv[0] = (v / 360.0).clamp(0.0, 1.0) as f32;
                        } else if widget_action.widget_uid == s_uid {
                            self.hsv[1] = (v / 100.0).clamp(0.0, 1.0) as f32;
                        } else {
                            self.hsv[2] = (v / 100.0).clamp(0.0, 1.0) as f32;
                        }
                        changed = true;
                        ended |= is_ended;
                    }
                } else if widget_action.widget_uid == hex_uid {
                    if let TextInputAction::Returned(text, _) =
                        widget_action.cast::<TextInputAction>()
                    {
                        if let Some((rgba, had_alpha)) = parse_hex(&text) {
                            self.adopt_rgb([rgba[0], rgba[1], rgba[2]]);
                            // An eight-figure hex typed into a picker with no
                            // alpha keeps its colour and loses its
                            // transparency, rather than being refused: what
                            // was pasted is nearly always the colour.
                            if had_alpha && self.with_alpha {
                                self.alpha = rgba[3];
                            }
                            changed = true;
                            ended = true;
                        }
                        self.sync_pending = true;
                    }
                }
            }
            if changed {
                self.publish(cx, uid, ended);
                self.sync_widgets(cx);
            }
        }

        // A carry is called off the way a knob's turn is: Escape or Back
        // when it is this carry's to take, the other button, or the window
        // going away under the hand. The press is forgotten with it, so the
        // release that follows opens nothing.
        if self.press.is_some() {
            // A key nothing claimed is this carry's as well: the press holds
            // the pointer, so no other gesture is under way to take it.
            let ours = |cx: &Cx, scope: &Option<CancelScope>| {
                scope.as_ref().is_some_and(|s| cx.owns_cancel(s)) || (scope.is_some() && !cx.has_cancel_owner())
            };
            let off = match event {
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => ours(cx, &self.carry_scope),
                Event::BackPressed { .. } => ours(cx, &self.carry_scope) && event.back_pressed(),
                Event::MouseDown(me) => me.button.is_secondary(),
                Event::WindowLostFocus(_) => true,
                _ => false,
            };
            if off {
                self.cancel_carry(cx);
                return;
            }
        }

        // Named as the lock's own, so the swatch still hears the press that
        // shuts its popover while the popover holds the pointer.
        let swatch = self.draw_swatch.area();
        if self.draggable {
            // THE POINTER-CAPTURE RULE: the press holds the pointer until the
            // release, so a carry across the other squares lights none of
            // them by itself and presses nothing on the way. A plain hit and
            // not a sweep: a sweep lets go of the pointer the moment it
            // leaves the swatch, which is where every carry goes. Only the
            // press on the swatch of an open popover has to name the lock
            // to be heard; the popover shuts on it, and every event after it
            // is asked plainly, which finds the capture by its area alone.
            let hit = if self.open {
                event.hits_with_sweep_area(cx, swatch, swatch)
            } else {
                event.hits(cx, swatch)
            };
            match hit {
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                    self.draw_swatch.hover = 1.0;
                    self.draw_swatch.redraw(cx);
                }
                Hit::FingerHoverOut(_) => {
                    self.draw_swatch.hover = 0.0;
                    self.draw_swatch.redraw(cx);
                }
                Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                    // A press on the swatch of an open popover shuts it now,
                    // as it always has: whether it then carries or not, the
                    // popover is not wanted over the row, and a carry must
                    // not leave the pointer locked behind it.
                    let was_open = self.open;
                    if self.open {
                        self.close_popover(cx, false);
                    }
                    self.press = Some((fe.abs, was_open));
                    self.carrying = false;
                    self.grab = fe.abs - self.draw_swatch.area().rect(cx).pos;
                }
                Hit::FingerMove(fe) => {
                    let Some((at, _)) = self.press else {
                        return;
                    };
                    if !self.carrying {
                        // The kit's slop, measured either way: a hand that
                        // only meant to click wobbles, and a wobble that
                        // carried would make every click a gamble.
                        if (fe.abs - at).length() < KNOB_DRAG_SLOP {
                            return;
                        }
                        self.start_carry(cx, fe.abs);
                        return;
                    }
                    self.carry_at = fe.abs;
                    cx.widget_action(uid, FabColorPickAction::DragMoved(fe.abs));
                    self.redraw_carry(cx);
                }
                Hit::FingerUp(fe) => {
                    let Some((_, was_open)) = self.press else {
                        return;
                    };
                    if self.carrying {
                        self.end_carry(cx);
                        cx.widget_action(uid, FabColorPickAction::DragDropped(fe.abs));
                    } else {
                        self.press = None;
                        if !was_open {
                            self.open_popover(cx);
                        }
                    }
                    self.draw_swatch.hover = if fe.is_over && fe.device.has_hovers() { 1.0 } else { 0.0 };
                    self.draw_swatch.redraw(cx);
                }
                _ => {}
            }
            return;
        }
        match event.hits_with_sweep_area(cx, swatch, swatch) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.draw_swatch.hover = 1.0;
                self.draw_swatch.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_swatch.hover = 0.0;
                self.draw_swatch.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if self.open {
                    self.close_popover(cx, false);
                } else {
                    self.open_popover(cx);
                }
            }
            _ => {}
        }
    }
}

impl FabColorPickRef {
    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabColorPickAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn set_rgba(&self, cx: &mut Cx, rgba: [f32; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rgba(cx, rgba);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |i| i.is_open())
    }
}

// ===========================================================================
// Tests — the pure core, ported with the control
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn bounded(min: f64, max: f64, wrap: bool) -> DragParams {
        DragParams {
            min,
            max,
            step: 0.25,
            wrap,
            bounded: true,
            snap_override: 0.0,
        }
    }

    fn unbounded(step: f64) -> DragParams {
        DragParams {
            min: 0.0,
            max: 0.0,
            step,
            wrap: false,
            bounded: false,
            snap_override: 0.0,
        }
    }

    /// The range takes FOUR widths of travel, not one — see
    /// [`DRAG_RANGE_TRAVEL`]. A field's own width is a thumb's movement, and
    /// spending the whole range across it leaves nothing landable in
    /// between.
    #[test]
    fn a_bounded_field_takes_four_widths_to_sweep_its_range() {
        let p = bounded(0.0, 24.0, false);
        let a = DragAnchor { x: 0.0, value: 12.0 };
        // 200 wide, so 800 of travel is the full 24; 100 is an eighth of it.
        let (v, _) = drag_map(&p, a, 100.0, 200.0, false, false);
        assert!((v - 15.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 800.0, 200.0, false, false);
        assert!((v - 24.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 50.0, 200.0, false, false);
        assert!((v - 13.5).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 50.0, 200.0, true, false);
        assert!((v - 12.075).abs() < 1e-9, "{v}");
    }

    #[test]
    fn an_unbounded_field_moves_by_pixels_times_step() {
        let p = unbounded(1.0);
        let a = DragAnchor { x: 0.0, value: 2000.0 };
        let (v, _) = drag_map(&p, a, 100.0, 400.0, false, false);
        assert!((v - 2100.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 100.0, 400.0, true, false);
        assert!((v - 2010.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn clamping_shifts_the_anchor_so_reversal_moves_immediately() {
        let p = bounded(0.0, 1.0, false);
        let a = DragAnchor { x: 0.0, value: 0.5 };
        let (v, a2) = drag_map(&p, a, 300.0, 100.0, false, false);
        assert!((v - 1.0).abs() < 1e-9);
        assert_eq!(a2.x, 300.0);
        assert_eq!(a2.value, 1.0);
        // One pixel back off the limit moves by one pixel's worth: the whole
        // range is 400 of travel here, so that is 1/400.
        let (v, _) = drag_map(&p, a2, 299.0, 100.0, false, false);
        assert!((v - 0.9975).abs() < 1e-9, "{v}");
    }

    #[test]
    fn cyclic_fields_wrap_at_their_ends() {
        let p = bounded(0.0, 24.0, true);
        let a = DragAnchor { x: 0.0, value: 23.0 };
        // 1200 wide is 4800 of travel for 24, so 400 pixels is 2.
        let (v, _) = drag_map(&p, a, 400.0, 1200.0, false, false);
        assert!((v - 1.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn hex_parses_and_formats_round_trip() {
        let (rgba, had_alpha) = parse_hex("#ff8000").unwrap();
        assert!(!had_alpha);
        assert!((rgba[0] - 1.0).abs() < 1e-6);
        assert!((rgba[1] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(format_hex(rgba, false), "#ff8000");
        let (rgba, had_alpha) = parse_hex("40E0D080").unwrap();
        assert!(had_alpha);
        assert_eq!(format_hex(rgba, true), "#40e0d080");
        assert!(parse_hex("#12345").is_none());
        assert!(parse_hex("nope").is_none());
    }

    #[test]
    fn hsv_rgb_round_trips() {
        for rgb in [[1.0f32, 0.0, 0.0], [0.2, 0.7, 0.4], [0.5, 0.5, 0.5]] {
            let [h, s, v] = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1e-5, "{rgb:?} -> {back:?}");
            }
        }
    }

    /// Where the popover's wheel puts its square puck, widget-local.
    fn square_puck_at(size: f64, s: f64, v: f64) -> DVec2 {
        let half = SQUARE_HALF * size;
        dvec2(size * 0.5 - half + s * 2.0 * half, size * 0.5 - half + (1.0 - v) * 2.0 * half)
    }

    /// A press anywhere on a puck's ink grabs that puck.
    ///
    /// The square's zone ended exactly at its edge, and the colours people
    /// pick live on that edge: every grey at saturation 0, every full colour
    /// at 1, black and the brights at the bottom and top. A puck there is
    /// half outside the square, and a press on its outer half landed in the
    /// gap before the ring and did nothing, or, at the top-right corner where
    /// the full colours are, on the ring, and changed the hue instead.
    #[test]
    fn a_press_on_a_puck_grabs_it_where_it_hangs_off_its_zone() {
        let size = 228.0;
        let zone = |at: DVec2, hsv: [f32; 3]| wheel_zone(at, size, hsv);
        let edges = [
            (1.0, 0.5, dvec2(5.0, 0.0)),
            (0.0, 0.5, dvec2(-5.0, 0.0)),
            (0.5, 1.0, dvec2(0.0, -5.0)),
            (0.5, 0.0, dvec2(0.0, 5.0)),
            (1.0, 1.0, dvec2(4.0, -4.0)),
            (1.0, 0.0, dvec2(4.0, 4.0)),
            (0.0, 1.0, dvec2(-4.0, -4.0)),
            (0.0, 0.0, dvec2(-4.0, 4.0)),
        ];
        for (s, v, out) in edges {
            let at = square_puck_at(size, s, v) + out;
            assert_eq!(
                zone(at, [0.3, s as f32, v as f32]),
                WheelZone::Square,
                "a press on the outer half of the puck at s {s} v {v} missed it"
            );
        }
        // The ring puck lies well inside the ring's own zone, but it is
        // held to the same rule.
        let mid = (RING_OUTER + RING_INNER) * 0.5 * size;
        for out in [-7.0, 7.0] {
            let at = dvec2(size * 0.5, size * 0.5 - mid - out);
            assert_eq!(zone(at, [0.0, 0.5, 0.5]), WheelZone::Ring, "a press {out} off the ring puck missed it");
        }
        // Off the pucks, the gap between the square and the ring goes to
        // whichever is nearer, instead of to nothing.
        let c = size * 0.5;
        let half = SQUARE_HALF * size;
        let hsv = [0.3, 0.5, 0.5];
        assert_eq!(zone(dvec2(c + half + 8.0, c), hsv), WheelZone::Square, "the gap beside the square went to nothing");
        assert_eq!(zone(dvec2(c + RING_INNER * size - 6.0, c), hsv), WheelZone::Ring, "the gap inside the ring went to nothing");
        // Outside the ring is outside the wheel.
        assert_eq!(zone(dvec2(c + RING_OUTER * size + 8.0, c), hsv), WheelZone::None);
        assert_eq!(zone(dvec2(1.0, 1.0), hsv), WheelZone::None);
    }

    #[test]
    fn the_zones_split_arrows_from_the_drag_surface() {
        assert_eq!(field_zone(5.0, 200.0, 20.0), FieldZone::Decrement);
        assert_eq!(field_zone(100.0, 200.0, 20.0), FieldZone::Middle);
        assert_eq!(field_zone(195.0, 200.0, 20.0), FieldZone::Increment);
    }

    /// The box every stock widget nested in a fab template resolves to,
    /// under whatever sheet is installed. One string per control, compared
    /// against the same reading with no sheet at all.
    fn fab_geometry(cx: &mut Cx) -> Vec<(&'static str, String)> {
        fn built(cx: &mut Cx, name: &str) -> WidgetRef {
            let widget = cx.with_vm(|vm| {
                let widgets = vm.module(id!(widgets));
                let value = vm
                    .bx
                    .heap
                    .value(widgets, LiveId::from_str(name).into(), NoTrap);
                WidgetRef::script_from_value(vm, value)
            });
            assert!(!widget.is_empty(), "{name} built no widget");
            widget
        }
        // Everything a walk can impose a size with. `margin` is in here too:
        // the stock field takes it from a theme token every sheet moves.
        fn shape(w: Walk) -> String {
            format!(
                "w={:?} h={:?} min={:?}/{:?} max={:?}/{:?} aspect={:?} margin={:?}",
                w.width,
                w.height,
                w.min_width,
                w.min_height,
                w.max_width,
                w.max_height,
                w.aspect,
                w.margin,
            )
        }
        let mut out = Vec::new();

        // The panel's filter field: the well, and the field inside it.
        let search = built(cx, "FabSearch");
        let input = search.widget(&*cx, &[live_id!(input)]);
        assert!(!input.is_empty(), "FabSearch no longer has an `input`");
        out.push(("FabSearch", shape(search.walk(cx))));
        out.push(("FabSearch/input", shape(input.walk(cx))));

        // The drag-numeric field's editor -- every property row on the panel.
        let value_input = built(cx, "FabValueInput");
        out.push(("FabValueInput", shape(value_input.walk(cx))));
        {
            let mut field = value_input
                .borrow_mut::<FabValueInput>()
                .expect("FabValueInput is a FabValueInput");
            out.push(("FabValueInput/text_input", shape(field.text_input.walk(cx))));
        }

        // The colour popover's hex row: a stock Button beside a stock field.
        let color_pick = built(cx, "FabColorPick");
        let (hex, pick) = {
            let popover = color_pick
                .borrow::<FabColorPick>()
                .expect("FabColorPick is a FabColorPick");
            (
                popover.popover.widget(&*cx, &[live_id!(hex)]),
                popover.popover.widget(&*cx, &[live_id!(pick)]),
            )
        };
        assert!(!hex.is_empty(), "the colour popover no longer has a `hex`");
        assert!(!pick.is_empty(), "the colour popover no longer has a `pick`");
        out.push(("FabColorPick/hex", shape(hex.walk(cx))));
        out.push(("FabColorPick/pick", shape(pick.walk(cx))));

        // The knob nests nothing a sheet can reach, and that is the claim:
        // its box is its own two numbers, under every sheet.
        let knob = built(cx, "FabKnob");
        out.push(("FabKnob", shape(knob.walk(cx))));

        // The matrix's column header, which is a box like any other even
        // though its ink is not.
        let header = built(cx, "FabDiagonalLabel");
        out.push(("FabDiagonalLabel", shape(header.walk(cx))));
        out
    }

    /// Every control the dev panel is built from resolves to the SAME box
    /// under every sheet the library ships as it does under none.
    ///
    /// The panel paints its own palette on purpose and is meant to be immune
    /// to whatever the app is wearing -- but immunity is not automatic. A fab
    /// template NESTS stock widgets, and a sheet reaches those through
    /// `mod.widgets.TextInput` and `mod.widgets.Button`, taking everything
    /// the template did not write out for itself.
    ///
    /// That is how the filter field came to sink. `android` and `ios` set
    /// `mod.widgets.TextInput.min_height` to 48 and 44; a walk applies a min
    /// height UNCONDITIONALLY (`draw/src/turtle.rs`, where `walk.min_height`
    /// is resolved), so FabSearch's `height: Fill` field stood 48 tall inside
    /// a 24 tall well -- and a single-line input CENTRES its line box in its
    /// own content box (`TextInput::scroll_to_cursor`), which put the word
    /// "Filter" a dozen pixels below the well's floor, straddling its border.
    ///
    /// Read off `DesktopStyle::ALL` rather than written out, so a sheet added
    /// later -- or an existing one that starts overriding `max_height`, the
    /// margin or the padding -- fails HERE and not on somebody's screen.
    #[test]
    fn the_fab_controls_resolve_the_same_box_under_every_sheet() {
        use crate::desktop_style::{install, uninstall, DesktopStyle, StyleSheet};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let plain = fab_geometry(&mut cx);
        assert_eq!(plain.len(), 8, "a control was dropped from the reading");
        for style in DesktopStyle::ALL {
            for dark in [false, true] {
                if dark && !style.supports_dark() {
                    continue;
                }
                cx.with_vm(|vm| {
                    install(vm, StyleSheet::load_with_appearance(style, dark));
                    vm.with_reload(crate::script_mod);
                });
                let under = fab_geometry(&mut cx);
                for ((name, want), (_, got)) in plain.iter().zip(under.iter()) {
                    assert_eq!(
                        want,
                        got,
                        "`{name}` resolves to a different box under `{}`{}",
                        style.id(),
                        if dark { " dark" } else { "" }
                    );
                }
            }
        }
        // ...and taking the sheet off puts the panel back where it started.
        cx.with_vm(|vm| {
            uninstall(vm);
            vm.with_reload(crate::script_mod);
        });
        assert_eq!(fab_geometry(&mut cx), plain);
    }

    /// Nothing a sheet sets reaches a fab control.
    ///
    /// The kit nests stock fields, buttons and scroll bars, and builds its
    /// face on the app's font family, and a sheet moves every one of those:
    /// its token half before the kit registers (a spacing rung reaches the
    /// margin of a nested field, a font the kit's words), its widget half
    /// onto the templates the kit nests (a face, a padding, an animator
    /// state's timing). So the kit is built from the library as it stands
    /// without a sheet (`desktop_style::keep_stock`), and this holds it
    /// there: under a sheet that sets everything a sheet may on every stock
    /// template (`desktop_style::everything_sheet`), the kit's table and every
    /// `mod.widgets.Fab*` the file registers -- read off the source, so a
    /// control added later is read too -- resolve line for line as they do
    /// with no sheet: every box, face, colour, font and timing. The sheet
    /// leaves the kit's own templates alone; no sheet writes to them by name.
    ///
    /// It stands where two narrower guards stood: one read the leaves the
    /// shipped sheets set on a `TextInput` and asked the filter field to
    /// declare each of them, the other walked the faces alone. Neither said
    /// anything about the next sheet.
    #[test]
    fn nothing_a_sheet_sets_reaches_a_fab_control() {
        use crate::desktop_style::{everything_sheet, install, resolution, resolution_diff, uninstall};
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let mut names: Vec<&str> = src
            .split("mod.widgets.")
            .skip(1)
            .filter_map(|rest| rest.split_once(" = ").map(|(name, _)| name))
            .filter(|name| name.starts_with("Fab") && name.chars().all(|c| c.is_ascii_alphanumeric()))
            .collect();
        names.sort();
        names.dedup();
        assert!(names.len() > 10, "only {} fab controls were read off the file", names.len());
        let read = |vm: &mut ScriptVm| -> Vec<String> {
            let fab = vm.module(id!(fab));
            let mut out = resolution(vm, fab.into(), "fab");
            let widgets = vm.module(id!(widgets));
            for name in &names {
                let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
                assert!(value.as_object().is_some(), "`{name}` did not resolve");
                out.extend(resolution(vm, value, name));
            }
            out
        };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let plain = read(vm);
            let sheet = everything_sheet(vm, &|name| names.contains(&name));
            install(vm, sheet);
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_reload(crate::script_mod);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "the sheet does not evaluate: {errors:?}");
            let moved = resolution_diff(&plain, &read(vm), 20);
            assert!(moved.is_empty(), "a sheet reaches the fab controls at:\n{}", moved.join("\n"));
            uninstall(vm);
        });
    }

    /// One entry of a runtime table, as the VM has it right now.
    fn table_color(cx: &mut Cx, table: LiveId, key: &str) -> u32 {
        cx.with_vm(|vm| {
            let module = vm.module(table);
            vm.bx
                .heap
                .value(module, LiveId::from_str(key).into(), NoTrap)
                .as_color()
                .unwrap_or_else(|| panic!("`{key}` is not a colour in this table"))
        })
    }

    /// Every colour the fab palette declares, with the literal it is written
    /// as. Read off the source, so an entry added to the table joins the
    /// tests below without anybody remembering to come back for it.
    fn declared_fab_colors() -> Vec<(String, u32)> {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let table = src
            .split("mod.fab = {")
            .nth(1)
            .expect("the file declares `mod.fab`");
        // The colours only: the density, type and motion entries after them
        // are numbers, and are deliberately NOT part of any palette swap.
        let table = &table[..table
            .find("// ---- density ----")
            .expect("the table still has a density block after the colours")];
        let mut out = Vec::new();
        for line in table.lines() {
            let line = line.trim();
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let Some(hex) = value.trim().strip_prefix("#x") else {
                continue;
            };
            if !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                continue;
            }
            let rgba = u32::from_str_radix(hex, 16).expect("a hex colour");
            // The table writes six digits and means opaque.
            out.push((name.to_string(), if hex.len() == 6 { rgba << 8 | 0xFF } else { rgba }));
        }
        assert!(out.len() > 30, "only {} colours were read off the table", out.len());
        out
    }

    /// The panel's own palette, under every sheet the library ships.
    ///
    /// This is the immunity the panel exists for, stated as a value rather
    /// than as an intention: every colour the panel's chrome is drawn in is
    /// the literal written in the table above, whatever the app is wearing.
    /// Not a default with something on the other side of it -- there is no
    /// other side, and this is the reading that keeps it that way.
    #[test]
    fn the_panels_palette_is_untouched_under_every_sheet() {
        use crate::desktop_style::{install, uninstall, DesktopStyle, StyleSheet};
        let declared = declared_fab_colors();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        for style in [None].into_iter().chain(DesktopStyle::ALL.map(Some)) {
            cx.with_vm(|vm| {
                match style {
                    Some(style) => install(vm, StyleSheet::load(style)),
                    None => uninstall(vm),
                }
                vm.with_reload(crate::script_mod);
            });
            for (name, want) in &declared {
                assert_eq!(
                    table_color(&mut cx, id!(fab), name),
                    *want,
                    "`fab.{name}` moved under `{}`",
                    style.map(|s| s.id()).unwrap_or("no sheet")
                );
            }
        }
    }

    /// The panel's palette can be READ, under every sheet the library ships.
    ///
    /// Immunity says the colours do not move. It does not say they were ever
    /// legible, and the two are worth holding apart: the pairs below are the
    /// panel's load-bearing ones -- the ground it draws its words on, the
    /// well, the button face, the popover, and the ink on the accent.
    ///
    /// Under every sheet rather than under none, because `mod.tweak_panel`'s
    /// three ink grades belong to the tweaker's table and so are not in the
    /// reading the test above takes.
    #[test]
    fn the_panels_palette_reads_against_itself_under_every_sheet() {
        use crate::desktop_style::{install, uninstall, DesktopStyle, StyleSheet};
        use crate::theme_tokens::{reads_on, LEGIBLE, READABLE};
        // (ground, ink, how far apart they have to stand)
        const PAIRS: &[(&str, &str, f64)] = &[
            ("color_area", "color_text", READABLE),
            ("color_area", "color_text_dim", LEGIBLE),
            ("color_panel", "color_text_header", READABLE),
            ("color_header", "color_text", READABLE),
            ("color_button", "color_text", READABLE),
            ("color_button_hover", "color_text_active", READABLE),
            // LEGIBLE, not READABLE: the panel's accent (#x5680c2 under
            // white) stands 3.99 apart and always has. This records where it
            // is rather than claiming it was ever a 4.5.
            ("color_button_active", "color_text_on_accent", LEGIBLE),
            ("color_input", "color_text", READABLE),
            ("color_popover", "color_text", READABLE),
            ("color_row_hover", "color_text", READABLE),
            // The two faces a panel SWITCH takes (`set_button_fill`): the
            // accent's container while it is on, the well tone while it is
            // off. Both carry the button's own word, which is `color_text`.
            ("color_accent_dim", "color_text", READABLE),
            ("color_input_hover", "color_text", READABLE),
        ];
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        for style in [None].into_iter().chain(DesktopStyle::ALL.map(Some)) {
            cx.with_vm(|vm| {
                match style {
                    Some(style) => install(vm, StyleSheet::load(style)),
                    None => uninstall(vm),
                }
                vm.with_reload(crate::script_mod);
            });
            let where_ = style.map(|s| s.id()).unwrap_or("no sheet");
            for (ground, ink, need) in PAIRS {
                let g = table_color(&mut cx, id!(fab), ground);
                let i = table_color(&mut cx, id!(fab), ink);
                let apart = reads_on(g, i);
                assert!(
                    apart >= *need,
                    "{where_}: `{ink}` on `{ground}` stands {apart:.2} apart, under the {need} it needs"
                );
            }
            // The panel's own ink grades on the same grounds are the
            // tweaker's table, and are read in the tweaker family.
        }
    }

    /// Every text style the kit declares, in template order, read back as
    /// what it RESOLVES to rather than as what the source says it is.
    fn kit_text_styles(cx: &mut Cx) -> Vec<TextStyle> {
        cx.with_vm(|vm| {
            let values = vec![
                crate::script_eval!(vm, {mod.widgets.FabValueInput.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabValueInput.text_input.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabSlider.draw_label.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabSlider.draw_value.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabKnob.draw_label.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabKnob.draw_value.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabKnob.draw_value_off.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabLabel.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabLabelSmall.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabHeaderLabel.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabDiagonalLabel.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabSearch.input.draw_text.text_style}),
                crate::script_eval!(vm, {mod.widgets.FabColorPick.popover.hex_row.hex.draw_text.text_style}),
            ];
            values
                .into_iter()
                .map(|value| TextStyle::script_from_value(vm, value))
                .collect()
        })
    }

    /// The kit's words keep ONE face, under every sheet the library ships.
    ///
    /// `mod.fab` is there so the panel does not restyle itself while the
    /// theme underneath it is being changed, and the text was the half of
    /// that which leaked: each `text_style` below named `theme.font_regular`
    /// and took only its SIZE from the table. Installing a blend takes the
    /// sheet off on every apply, so on the applied frame `android`'s Roboto
    /// came through every label and field on the panel at once and went back
    /// on leave -- the row heights are fab and held, the text metrics moved,
    /// and the labels reflowed under the hand that was dragging a weight.
    ///
    /// Two things make this hard to fake. The face is the one the family
    /// RESOLVES to, so a site re-pointed at the theme by hand fails here
    /// however it is spelled; and each entry is checked against the size its
    /// own template declares, which is what proves the path found a real fab
    /// text style rather than an empty object wearing the 10pt default.
    /// The filter box, focused with a word in it, under either base theme:
    /// the ink it types with has to be the panel's, never the theme's. Only
    /// the resting ink was; hover, focus and down fell through to the stock
    /// field's `theme.color_text_focus` and friends, which under a light
    /// theme are dark, on a well that is not.
    #[test]
    fn the_filter_box_keeps_its_own_ink_in_every_state() {
        for base in [crate::BaseTheme::Light, crate::BaseTheme::Dark] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            crate::set_base_theme(&mut cx, base);
            let (inks, own) = cx.with_vm(|vm| {
                crate::script_mod(vm);
                let widgets = vm.module(id!(widgets));
                let search = vm.bx.heap.value(widgets, LiveId::from_str("FabSearch").into(), NoTrap);
                let input = vm.bx.heap.value(search.as_object().expect("FabSearch"), LiveId::from_str("input").into(), NoTrap);
                let draw_text = vm.bx.heap.value(input.as_object().expect("input"), LiveId::from_str("draw_text").into(), NoTrap);
                let dt = draw_text.as_object().expect("draw_text");
                let keys = ["color", "color_hover", "color_focus", "color_down", "color_disabled", "color_empty", "color_empty_hover", "color_empty_focus"];
                let inks: Vec<(&str, Option<u32>)> = keys
                    .into_iter()
                    .map(|k| (k, vm.bx.heap.value(dt, LiveId::from_str(k).into(), NoTrap).as_color()))
                    .collect();
                let fab = vm.module(id!(fab));
                let own: Vec<u32> = ["color_text", "color_text_active", "color_text_dim", "color_text_muted"]
                    .into_iter()
                    .filter_map(|k| vm.bx.heap.value(fab, LiveId::from_str(k).into(), NoTrap).as_color())
                    .collect();
                (inks, own)
            });
            assert_eq!(own.len(), 4, "the panel's palette is missing an ink this test relies on");
            for (key, ink) in inks {
                let ink = ink.unwrap_or_else(|| panic!("{key} is not a colour on the filter box"));
                assert!(
                    own.contains(&ink),
                    "under {base:?}, {key} is #{ink:08X}, not one of the panel's own inks: it fell through to the app theme"
                );
            }
        }
    }

    #[test]
    fn the_kits_words_keep_one_face_under_every_sheet() {
        use crate::desktop_style::{install, uninstall, DesktopStyle, StyleSheet};
        // (where it is written, the size that template asks for)
        const SITES: &[(&str, f32)] = &[
            ("FabValueInput.draw_text", 8.5),
            ("FabValueInput.text_input.draw_text", 8.5),
            ("FabSlider.draw_label", 8.5),
            ("FabSlider.draw_value", 8.5),
            ("FabKnob.draw_label", 7.5),
            ("FabKnob.draw_value", 7.5),
            ("FabKnob.draw_value_off", 7.5),
            ("FabLabel.draw_text", 8.5),
            ("FabLabelSmall.draw_text", 7.5),
            ("FabHeaderLabel.draw_text", 9.0),
            ("FabDiagonalLabel.draw_text", 7.5),
            ("FabSearch.input.draw_text", 8.5),
            ("FabColorPick.popover.hex_row.hex.draw_text", 8.5),
        ];
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        for style in [None].into_iter().chain(DesktopStyle::ALL.map(Some)) {
            cx.with_vm(|vm| {
                match style {
                    Some(style) => install(vm, StyleSheet::load(style)),
                    None => uninstall(vm),
                }
                vm.with_reload(crate::script_mod);
            });
            let where_ = style.map(|s| s.id()).unwrap_or("no sheet");
            let read = kit_text_styles(&mut cx);
            assert_eq!(read.len(), SITES.len());
            for (style, (site, size)) in read.into_iter().zip(SITES) {
                assert_eq!(
                    style.font_size, *size,
                    "{where_}: `{site}` did not resolve to a fab text style"
                );
                let family = format!("{:?}", style.font_family);
                // The FIRST member is the one the latin metrics come from.
                let first = family
                    .split("resource_path: \"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("nothing at all");
                assert!(
                    first.ends_with("IBMPlexSans-Text.ttf"),
                    "{where_}: `{site}` leads with `{first}`"
                );
                assert!(
                    !family.contains("RobotoFlex.ttf") && !family.contains("Inter.ttf"),
                    "{where_}: `{site}` took the sheet's typeface: {family}"
                );
                // ...and what follows it is still the app's fallback chain,
                // so a filter field can spell what was typed into it.
                assert!(
                    family.contains("NotoColorEmoji.ttf"),
                    "{where_}: `{site}` lost the fallbacks: {family}"
                );
            }
        }
    }

    /// The panel's density and type are not a sheet's to move.
    ///
    /// The density and type entries are what make the panel an inspector: a
    /// 24px row, a 20px small row, 8.5pt words. A sheet reaching these would
    /// put `android`'s 48px controls back into the panel through the front
    /// door -- the very thing the sunken filter field was.
    #[test]
    fn the_panels_density_and_type_never_move_under_a_sheet() {
        use crate::desktop_style::{install, DesktopStyle, StyleSheet};
        const NUMBERS: &[(&str, f64)] = &[
            ("row_height", 24.0),
            ("row_height_sm", 20.0),
            ("header_height", 26.0),
            ("prop_label_width", 92.0),
            ("font_size_ui", 8.5),
            ("font_size_small", 7.5),
            ("font_size_header", 9.0),
        ];
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        for style in DesktopStyle::ALL {
            cx.with_vm(|vm| {
                install(vm, StyleSheet::load(style));
                vm.with_reload(crate::script_mod);
            });
            for (name, want) in NUMBERS {
                let got = cx.with_vm(|vm| {
                    let fab = vm.module(id!(fab));
                    vm.bx
                        .heap
                        .value(fab, LiveId::from_str(name).into(), NoTrap)
                        .as_f64()
                });
                assert_eq!(got, Some(*want), "`fab.{name}` moved under `{}`", style.id());
            }
        }
    }

    #[test]
    fn ended_finds_commit_after_changed_action() {
        let uid = WidgetUid(17);
        let actions: ActionsBuf = vec![
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabValueInputAction::Changed(72.0)),
                widget_uid: uid,
                group: None,
            }),
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabValueInputAction::Ended(73.0)),
                widget_uid: uid,
                group: None,
            }),
        ];
        assert_eq!(ended_value(&actions, uid), Some(73.0));
    }

    fn equalizer_row() -> SliderTravel {
        SliderTravel {
            min: 0.0,
            max: 100.0,
            step: 0.0,
            thumb: 12.0,
            inset: 2.0,
        }
    }

    /// The whole point of a track: the value is where the pointer IS, not
    /// how far it has travelled since the press. Both stops and the middle,
    /// measured from the track's left edge — 160 of track, 12 of thumb and 2
    /// of inset at each end leave 144 of travel, starting 8 in.
    #[test]
    fn the_track_reads_the_value_under_the_pointer() {
        let t = equalizer_row();
        let v = t.value_at(8.0, 160.0);
        assert!(v.abs() < 1e-9, "{v}");
        let v = t.value_at(80.0, 160.0);
        assert!((v - 50.0).abs() < 1e-9, "{v}");
        let v = t.value_at(152.0, 160.0);
        assert!((v - 100.0).abs() < 1e-9, "{v}");
    }

    /// ...and the thumb is drawn where a press will read it back. The shader
    /// is handed these same three numbers every draw, so this is also what
    /// keeps the pixels and the hit test from disagreeing.
    #[test]
    fn the_thumb_stands_where_a_press_reads_it_back() {
        let t = equalizer_row();
        for want in [0.0, 12.5, 33.0, 50.0, 99.0, 100.0] {
            let x = t.thumb_x(want, 160.0);
            let got = t.value_at(x, 160.0);
            assert!((got - want).abs() < 1e-9, "{want} came back as {got}");
        }
    }

    /// Past either stop is the stop. A track has nowhere else to go, and a
    /// detent that does not divide the range must not walk off the end of
    /// it either.
    #[test]
    fn the_value_can_never_leave_the_track() {
        let t = equalizer_row();
        assert!(t.value_at(-400.0, 160.0).abs() < 1e-9);
        assert!((t.value_at(4000.0, 160.0) - 100.0).abs() < 1e-9);
        assert!(t.settle(-1.0).abs() < 1e-9);
        assert!((t.settle(1e9) - 100.0).abs() < 1e-9);
        let detented = SliderTravel {
            step: 7.0,
            ..equalizer_row()
        };
        let v = detented.value_at(4000.0, 160.0);
        assert!((v - 98.0).abs() < 1e-9, "{v}");
        assert!(detented.settle(1e9) <= 100.0);
        assert!(detented.settle(-1e9) >= 0.0);
    }

    #[test]
    fn the_columns_split_the_name_and_the_number_off_the_track() {
        assert_eq!(slider_zone(4.0, 300.0, 100.0, 40.0), SliderZone::Label);
        assert_eq!(slider_zone(150.0, 300.0, 100.0, 40.0), SliderZone::Track);
        assert_eq!(slider_zone(280.0, 300.0, 100.0, 40.0), SliderZone::Readout);
    }

    #[test]
    fn a_sliders_commit_is_found_after_the_change_it_follows() {
        let uid = WidgetUid(19);
        let actions: ActionsBuf = vec![
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabSliderAction::Changed(41.0)),
                widget_uid: uid,
                group: None,
            }),
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabSliderAction::Ended(42.0)),
                widget_uid: uid,
                group: None,
            }),
        ];
        assert_eq!(slider_ended_value(&actions, uid), Some(42.0));
    }

    /// The new control is painted out of the panel's own table and nothing
    /// else. A face that writes a colour of its own is a face that stays
    /// dark under a light theme; a face that reads the app's theme is the
    /// white slab the whole palette was written to avoid.
    #[test]
    fn the_sliders_face_names_only_the_panels_own_palette() {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let face = src
            .split("do #(DrawFabSlider::script_shader(vm)){")
            .nth(1)
            .expect("the file declares the slider's shader");
        let face = &face[..face
            .find("mod.widgets.FabSliderBase")
            .expect("the shader is followed by the registration")];
        assert!(
            !face.contains("#x"),
            "the slider's face writes a colour of its own"
        );
        assert!(
            !face.contains("theme."),
            "the slider's face reads the app's theme"
        );
        assert!(
            face.contains("fab.color_num"),
            "the slider's face is drawn from the fab table"
        );
    }

    /// The knob is held to the same table, and ALL of it is: the shader, and
    /// the template after it with its three inks. The slider's reading stops
    /// at the shader; this one runs on to the next control's, because a knob
    /// is put on the panel a hundred at a time and a word of it that took the
    /// app's ink would be a hundred words changing colour under the hand that
    /// is blending the app's theme.
    #[test]
    fn the_knob_names_only_the_panels_own_palette() {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let knob = src
            .split("do #(DrawFabKnob::script_shader(vm)){")
            .nth(1)
            .expect("the file declares the knob's shader");
        let knob = &knob[..knob
            .find("do #(DrawColorWheel::script_shader(vm)){")
            .expect("the knob is followed by the colour wheel")];
        assert!(
            knob.contains("mod.widgets.FabKnob = "),
            "the reading stops short of the knob's template"
        );
        assert!(!knob.contains("#x"), "the knob writes a colour of its own");
        assert!(!knob.contains("vec4(0.") && !knob.contains("vec4(1."), "the knob writes a colour of its own");
        assert!(
            !knob.contains("theme.") && !knob.contains("mod.theme"),
            "the knob reads the app's theme"
        );
        for ink in ["fab.color_accent", "fab.color_num", "fab.color_button", "fab.color_focus_ring", "fab.color_text_muted"] {
            assert!(knob.contains(ink), "the knob's face no longer draws from `{ink}`");
        }
        // Every text style in it is the kit's own face.
        assert_eq!(
            knob.matches("text_style:").count(),
            knob.matches("text_style: fab.font{").count(),
            "a text style on the knob is not built from `fab.font`"
        );
    }

    /// A 26 point column of a knob matrix, 48 down.
    fn column() -> Rect {
        Rect {
            pos: dvec2(100.0, 40.0),
            size: dvec2(26.0, 48.0),
        }
    }

    /// The anchor, which is the whole control: one end of the name stands on
    /// the middle of its own column's bottom edge, so a name is always over
    /// the thing it names however long it is. A `Fall` ENDS there, a `Rise`
    /// STARTS there.
    #[test]
    fn a_name_stands_on_the_bottom_centre_of_its_own_column() {
        let b = column();
        let centre = dvec2(b.pos.x + 13.0, b.pos.y + 48.0);
        for name_width in [10.0, 50.0, 120.0] {
            let fall = diagonal_run(b, name_width, 10.0, 45.0, DiagonalLean::Fall);
            assert!(
                (fall.end - centre).length() < 1e-9,
                "a fall of {name_width} ends at {:?}, not {centre:?}",
                fall.end
            );
            let rise = diagonal_run(b, name_width, 10.0, 45.0, DiagonalLean::Rise);
            assert!(
                (rise.start - centre).length() < 1e-9,
                "a rise of {name_width} starts at {:?}, not {centre:?}",
                rise.start
            );
            // ...and the other end is one name away along the slope.
            assert!(((fall.end - fall.start).length() - name_width).abs() < 1e-9);
            assert!(((rise.end - rise.start).length() - name_width).abs() < 1e-9);
        }
    }

    /// The two leans hang over OPPOSITE sides, which is the reason both
    /// exist: over a matrix, `Fall` spills into the empty corner above the
    /// row names and `Rise` spills past the last column into the panel's
    /// edge.
    #[test]
    fn the_two_leans_hang_over_opposite_sides_of_the_column() {
        let b = column();
        let fall = diagonal_run(b, 50.0, 10.0, 45.0, DiagonalLean::Fall);
        let rise = diagonal_run(b, 50.0, 10.0, 45.0, DiagonalLean::Rise);
        assert!(fall.bounds.pos.x < b.pos.x, "a fall hangs over the left");
        assert!(
            fall.bounds.pos.x + fall.bounds.size.x <= b.pos.x + b.size.x + 1e-9,
            "a fall stays off the right"
        );
        assert!(
            rise.bounds.pos.x + rise.bounds.size.x > b.pos.x + b.size.x,
            "a rise hangs over the right"
        );
        assert!(rise.bounds.pos.x >= b.pos.x - 10.0, "a rise barely hangs left");
        // Both sit ON the column's bottom edge, whichever way they lean.
        for run in [fall, rise] {
            let bottom = run.bounds.pos.y + run.bounds.size.y;
            assert!((bottom - (b.pos.y + b.size.y)).abs() < 1e-9);
        }
        // ...and a name too long for the row it is in leaves the top of it
        // rather than shrinking or being cut: the row's height is the host's
        // to get right, and `diagonal_row_height` is how.
        let over = diagonal_run(b, 90.0, 10.0, 45.0, DiagonalLean::Fall);
        assert!(over.bounds.size.y > b.size.y);
        assert!(over.bounds.pos.y < b.pos.y, "the name stayed inside a box too short for it");
    }

    /// Nought degrees is a plain horizontal label on the box's bottom edge,
    /// not a special case: a column wide enough not to need the trick does
    /// not need a different control.
    #[test]
    fn a_turn_of_nothing_is_a_plain_horizontal_label() {
        let b = column();
        for lean in [DiagonalLean::Fall, DiagonalLean::Rise] {
            let run = diagonal_run(b, 50.0, 10.0, 0.0, lean);
            assert_eq!(run.angle, 0.0);
            assert!((run.start.y - run.end.y).abs() < 1e-9, "the baseline is level");
            assert!((run.start.y - (b.pos.y + b.size.y)).abs() < 1e-9, "on the bottom edge");
            assert!((run.bounds.size.y - 10.0).abs() < 1e-9, "one line tall");
        }
        assert!(
            (diagonal_row_height(50.0, 10.0, 0.0) - 10.0).abs() < 1e-9,
            "a level row is a line tall whatever the name is"
        );
    }

    /// A longer name needs a taller row, and the widget's bounds and the
    /// number a host fixes its row with are the same number.
    #[test]
    fn a_longer_name_needs_a_taller_header_row() {
        let short = diagonal_row_height(20.0, 10.0, 45.0);
        let long = diagonal_row_height(60.0, 10.0, 45.0);
        assert!(long > short + 20.0, "{short} -> {long}");
        // The sum itself: the name along the hypotenuse, the line across it.
        let want = 60.0 * std::f64::consts::FRAC_1_SQRT_2 + 10.0 * std::f64::consts::FRAC_1_SQRT_2;
        assert!((long - want).abs() < 1e-9, "{long} is not {want}");
        // ...and a steeper turn needs more room still.
        assert!(diagonal_row_height(60.0, 10.0, 60.0) > long);
        // What the helper says and what the run takes are one number.
        for lean in [DiagonalLean::Fall, DiagonalLean::Rise] {
            let run = diagonal_run(column(), 60.0, 10.0, 45.0, lean);
            assert!((run.bounds.size.y - long).abs() < 1e-9);
        }
    }

    /// The panel's own palette and the panel's own face, on the header as on
    /// everything else the kit draws. Read off the SOURCE, so a colour token
    /// borrowed from the app's theme fails here rather than on the day
    /// somebody installs a sheet. The same reading the knob gets, for the
    /// same reason.
    #[test]
    fn the_diagonal_header_names_only_the_panels_own_palette() {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let label = src
            .split("mod.widgets.FabDiagonalLabelBase = ")
            .nth(1)
            .expect("the file declares the diagonal header");
        let label = &label[..label
            .find("// ---- the search well ----")
            .expect("the diagonal header is followed by the search well")];
        assert!(
            label.contains("mod.widgets.FabDiagonalLabel = "),
            "the reading stops short of the header's template"
        );
        assert!(!label.contains("#x"), "the header writes a colour of its own");
        assert!(
            !label.contains("theme.") && !label.contains("mod.theme"),
            "the header reads the app's theme"
        );
        for token in ["fab.color_text_dim", "fab.font_size_small"] {
            assert!(label.contains(token), "the header no longer draws from `{token}`");
        }
        assert_eq!(
            label.matches("text_style:").count(),
            label.matches("text_style: fab.font{").count(),
            "a text style on the header is not built from `fab.font`"
        );
    }

    /// The carousel's chip is the one face in the kit that is MEANT to be a
    /// colour the panel knows nothing about: its four bands are the theme
    /// being offered, and a chip drawn from the panel's table would show the
    /// panel instead of the palette. So the reading is the other way round --
    /// the bands come off the host, and everything the panel owns, which is
    /// the ring that says which chip is hovered and which is in force, comes
    /// off the fab table like everything else.
    #[test]
    fn the_palette_carousel_shows_the_hosts_colours_and_wears_the_panels_ring() {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        let chip = src
            .split("do #(DrawFabPaletteChip::script_shader(vm)){")
            .nth(1)
            .expect("the file declares the chip's shader");
        let chip = &chip[..chip
            .find("mod.widgets.FabColorPickBase")
            .expect("the chip is followed by the colour picker")];
        assert!(
            chip.contains("mod.widgets.FabPaletteCarousel = "),
            "the reading stops short of the carousel's template"
        );
        assert!(!chip.contains("#x"), "the chip writes a colour of its own");
        assert!(
            !chip.contains("theme.") && !chip.contains("mod.theme"),
            "the chip reads the app's theme"
        );
        for band in ["self.band_0", "self.band_1", "self.band_2", "self.band_3"] {
            assert!(chip.contains(band), "the chip no longer paints `{band}`");
        }
        // And how many of them: a palette of fewer colours is a chip of
        // fewer squares, and the steps between bands are read off it.
        assert!(chip.contains("step(1.0 / self.bands, y)"), "the chip no longer shows as many bands as it is told");
        for ink in ["fab.color_border", "fab.color_focus_ring"] {
            assert!(chip.contains(ink), "the chip's ring no longer comes from `{ink}`");
        }
        // The one colour written out is the band's own opacity: a palette
        // holding a translucent colour must still show as a solid block, or
        // a chip becomes a reading of the panel's ground through it.
        assert_eq!(
            chip.matches("vec4(").count(),
            chip.matches("vec4(0.0, 0.0, 0.0, 1.0)").count() + chip.matches("vec4(band.xyz, 1.0)").count(),
            "the chip's face writes a colour that is neither a band nor a band's opacity"
        );
    }

    fn a_row_of(count: usize, view: f64) -> ChipTrack {
        ChipTrack { count, chip_width: 22.0, gap: 3.0, view }
    }

    /// The row scrolls from its first chip flush left to its last chip
    /// flush right and no further, and a row the window holds whole does not
    /// scroll at all.
    #[test]
    fn a_row_of_chips_scrolls_between_its_two_ends() {
        let row = a_row_of(40, 260.0);
        assert_eq!(row.span(), 40.0 * 25.0 - 3.0);
        assert_eq!(row.max_scroll(), row.span() - 260.0);
        assert_eq!(row.clamp(-50.0), 0.0, "the row scrolled back past its first chip");
        assert_eq!(row.clamp(1e6), row.max_scroll(), "the row scrolled on past its last chip");
        let short = a_row_of(6, 260.0);
        assert_eq!(short.max_scroll(), 0.0, "a row the window holds whole still scrolls");
        assert_eq!(a_row_of(0, 260.0).max_scroll(), 0.0);
    }

    /// Which chip is under a point, the gap between two belonging to
    /// neither, and the scroll moving what is under the same point.
    #[test]
    fn the_chip_under_a_point_moves_with_the_scroll() {
        let row = a_row_of(40, 260.0);
        assert_eq!(row.index_at(0.0, 10.0), Some(0));
        assert_eq!(row.index_at(0.0, 23.5), None, "a press in the gap chose a palette");
        assert_eq!(row.index_at(0.0, 26.0), Some(1));
        assert_eq!(row.index_at(250.0, 10.0), Some(10), "the scroll did not move what is under the point");
        assert_eq!(row.index_at(0.0, 261.0), None, "a point past the window found a chip");
        assert_eq!(a_row_of(3, 260.0).index_at(0.0, 200.0), None, "a point past the last chip found one");
    }

    /// A chip is brought into the window by as little as that takes, and a
    /// chip already in it leaves the row where it stood.
    #[test]
    fn revealing_a_chip_moves_the_row_as_little_as_it_can() {
        let row = a_row_of(40, 260.0);
        assert_eq!(row.reveal(0.0, 3), 0.0, "a chip in the window moved the row");
        // Off to the right: its right edge to the window's.
        let at = row.reveal(0.0, 20);
        assert_eq!(at, 20.0 * 25.0 + 22.0 - 260.0);
        assert_eq!(row.index_at(at, 259.0), Some(20), "the chip revealed is not at the right edge");
        // Off to the left: its left edge to the window's.
        assert_eq!(row.reveal(600.0, 5), 125.0);
        // The last chip, which is also the row's far end.
        assert_eq!(row.reveal(0.0, 39), row.max_scroll());
        // The chips drawn are the ones any part of which shows.
        assert_eq!(row.in_view(0.0), 0..11, "the chip cut by the edge was not drawn");
        assert_eq!(row.in_view(row.max_scroll()).end, 40);
    }

    /// A page is the whole chips the window holds, never nought, and paging
    /// from either end walks the row and stops there.
    #[test]
    fn a_page_is_the_whole_chips_the_window_holds() {
        let row = a_row_of(40, 260.0);
        // Ten chips stand whole in 260 points at a pitch of 25.
        assert_eq!(row.page(), 250.0);
        assert_eq!(row.index_at(row.page(), 1.0), Some(10), "a page did not land on the eleventh chip");
        // A window narrower than a chip still moves on by one.
        assert_eq!(a_row_of(40, 10.0).page(), 25.0);
        assert_eq!(a_row_of(40, 25.0).page(), 25.0, "a window holding one chip paged by none");
        // From either end, and no further than the end.
        assert_eq!(row.clamp(0.0 - row.page()), 0.0);
        let mut at = 0.0;
        for _ in 0..40 {
            at = row.clamp(at + row.page());
        }
        assert_eq!(at, row.max_scroll(), "paging on did not reach the row's far end");
        assert_eq!(a_row_of(6, 260.0).clamp(a_row_of(6, 260.0).page()), 0.0, "a row the window holds whole paged anyway");
    }

    /// A sideways delta and a plain wheel both scroll the row, the larger of
    /// the two where a trackpad sends both.
    #[test]
    fn the_wheel_and_a_sideways_delta_both_move_the_row() {
        assert_eq!(carousel_wheel(0.0, 30.0), 30.0, "a plain wheel does not move the row");
        assert_eq!(carousel_wheel(-12.0, 0.0), -12.0, "a sideways delta does not move the row");
        assert_eq!(carousel_wheel(12.0, 2.0), 12.0, "a trackpad's drift beat the way it was moving");
        assert_eq!(carousel_wheel(1.0, -9.0), -9.0);
    }

    /// A glide closes the distance over frames rather than in the one the
    /// press landed in, and it slows down into its target rather than
    /// stopping dead at it.
    #[test]
    fn a_glide_closes_the_gap_over_frames_and_slows_into_it() {
        let mut glide = ChipGlide::default();
        glide.moved(250.0, 0.0);
        assert_eq!(glide.at(250.0), 0.0, "the picture jumped to where the row now stands");
        assert!(glide.running());
        // The frames a display sends, and the ground each covers.
        let mut at = 0.0;
        let mut steps = Vec::new();
        for frame in 1..12 {
            glide.tick(frame as f64 / 60.0);
            let now = glide.at(250.0);
            steps.push(now - at);
            at = now;
        }
        assert!(steps[0] > 1.0, "the first frame moved nothing: {steps:?}");
        assert!(at < 250.0, "a tenth of a second in, the row is already there");
        assert!(
            steps[0] > steps[10] * 3.0,
            "the row is not slowing into its target: {steps:?}"
        );
        // And it does arrive, and stops asking for frames when it does.
        let mut frame = 12;
        while glide.tick(frame as f64 / 60.0) {
            frame += 1;
            assert!(frame < 600, "the glide never came to rest");
        }
        assert!(!glide.running());
        assert_eq!(glide.at(250.0), 250.0, "the row came to rest short of its place");
        assert!(!glide.tick(20.0), "a row at rest asked for another frame");
    }

    /// A second move while the first is still running is added to what is
    /// outstanding: the picture carries on from where it is, and the row
    /// never doubles back.
    #[test]
    fn a_second_move_is_added_to_what_is_still_outstanding() {
        let mut glide = ChipGlide::default();
        glide.moved(250.0, 0.0);
        for frame in 1..6 {
            glide.tick(frame as f64 / 60.0);
        }
        let midway = glide.at(250.0);
        assert!(midway > 0.0 && midway < 250.0, "{midway}");
        glide.moved(250.0, 0.0);
        assert!(
            (glide.at(500.0) - midway).abs() < 1e-9,
            "the second page jumped the picture to {}",
            glide.at(500.0)
        );
        let mut at = midway;
        let mut frame = 6;
        while glide.tick(frame as f64 / 60.0) {
            let now = glide.at(500.0);
            assert!(now >= at - 1e-9, "the row went backwards: {at} then {now}");
            at = now;
            frame += 1;
            assert!(frame < 600, "the glide never came to rest");
        }
        assert_eq!(glide.at(500.0), 500.0);
    }

    /// A move small enough to be over already is over: nothing to draw, no
    /// frames asked for.
    #[test]
    fn a_move_of_nothing_is_not_a_glide() {
        let mut glide = ChipGlide::default();
        glide.moved(0.01, 0.0);
        assert!(!glide.running(), "a hundredth of a point is a glide");
        assert_eq!(glide.at(7.0), 7.0);
    }

    /// The flick: a release carries the row on by what its speed would run
    /// out in, the other way to the hand. A press that went nowhere, and a
    /// hand that came to a stop before it let go, both leave it standing.
    #[test]
    fn a_flick_carries_the_row_on_and_a_hand_that_stopped_does_not() {
        let slow = carousel_flick(-200.0, -100.0);
        let fast = carousel_flick(-2000.0, -100.0);
        assert!(slow > 0.0, "a flick to the left did not carry the row on");
        assert!(fast > slow * 9.0, "the carry is not the speed's: {slow} then {fast}");
        assert_eq!(carousel_flick(2000.0, 100.0), -fast, "and it is signed");
        assert_eq!(carousel_flick(-2000.0, -2.0), 0.0, "a press that wobbled threw the row");
        assert_eq!(carousel_flick(0.0, -100.0), 0.0, "a drag that ended standing still drifted on");
        // A decay that never decays must not carry the row an infinity.
        assert_eq!(chip_spin_travel(500.0, 1.0), 0.0);
    }

    /// A glide opens at the speed the hand let go with, and however far or
    /// slow the throw, it is over between a seventh of a second and
    /// three quarters of one.
    #[test]
    fn a_glide_opens_at_the_speed_the_hand_left_it_with() {
        // The opening speed of an exponential approach is distance / tau.
        let tau = chip_glide_secs(300.0, 1200.0);
        assert!((300.0 / tau - 1200.0).abs() < 1.0);
        assert_eq!(chip_glide_secs(2.0, 4000.0), CHIP_GLIDE_SECS.0, "a small correction is instant");
        assert_eq!(chip_glide_secs(900.0, 20.0), CHIP_GLIDE_SECS.1, "a slow crawl lasts all day");
        assert_eq!(chip_glide_secs(250.0, 0.0), CHIP_GLIDE_SECS.0, "a move nobody threw takes the short glide");
    }

    fn cell() -> KnobTurn {
        KnobTurn {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            travel: 150.0,
        }
    }

    /// The whole range in the travel the knob names, up for more -- and the
    /// same travel whatever size the knob is drawn at, which is the point of
    /// the number being its own.
    #[test]
    fn a_knob_crosses_its_range_in_the_travel_it_names() {
        let t = cell();
        assert!((t.carry(0.0, 150.0, false) - 100.0).abs() < 1e-9);
        assert!((t.carry(0.0, 75.0, false) - 50.0).abs() < 1e-9);
        assert!((t.carry(50.0, -75.0, false) - 0.0).abs() < 1e-9, "down is less");
        // Shift is a tenth of the speed: the same travel, a tenth of the way.
        assert!((t.carry(0.0, 150.0, true) - 10.0).abs() < 1e-9);
        // A travel that is not one falls back rather than dividing by it.
        let broken = KnobTurn { travel: 0.0, ..cell() };
        assert!((broken.carry(0.0, 75.0, false) - 50.0).abs() < 1e-9);
    }

    /// Past either stop is the stop, AND the anchor goes with the hand: a
    /// pointer that overshot the top by a mile starts bringing the value down
    /// on the first pixel of its way back.
    #[test]
    fn a_knob_can_never_leave_its_range_and_does_not_wind_up_past_it() {
        let t = cell();
        let top = t.carry(90.0, 4000.0, false);
        assert_eq!(top, 100.0);
        let back = t.carry(top, -15.0, false);
        assert!((back - 90.0).abs() < 1e-9, "the overshoot had to be unwound first: {back}");
        assert_eq!(t.carry(5.0, -4000.0, false), 0.0);
        assert_eq!(t.settle(1e9), 100.0);
        assert_eq!(t.contain(-3.0), 0.0);
        // The host's number keeps its fraction; the hand's lands on the
        // detent.
        assert_eq!(t.contain(37.5), 37.5);
        assert_eq!(t.settle(37.4), 37.0);
    }

    /// A fine drag over a coarse detent still gets somewhere, because what is
    /// carried from move to move is what the hand asked for and not what the
    /// detent made of it.
    #[test]
    fn a_fine_drag_is_not_rounded_back_to_where_it_started() {
        let t = KnobTurn { step: 5.0, ..cell() };
        let mut raw = 50.0;
        let mut said = Vec::new();
        for _ in 0..60 {
            raw = t.carry(raw, 1.0, true);
            said.push(t.settle(raw));
        }
        // Sixty points at a tenth of the speed is four parts of the range.
        assert!((raw - 54.0).abs() < 1e-9, "{raw}");
        assert_eq!(*said.last().unwrap(), 55.0, "the detent swallowed the drag");
        assert!(said.iter().all(|v| *v == 50.0 || *v == 55.0));
    }

    /// The face is the biggest circle the box holds once the words have had
    /// their rows, and the slack is shared above and below.
    #[test]
    fn the_face_is_what_the_words_leave_of_the_box() {
        // The default: 44 by 64 with the number and no name.
        assert_eq!(knob_face(44.0, 64.0, 0.0, 12.0), KnobFace { diameter: 44.0, top: 4.0 });
        // A name as well takes the face down to what is left of the height.
        assert_eq!(knob_face(44.0, 64.0, 12.0, 12.0), KnobFace { diameter: 40.0, top: 0.0 });
        // The cell the matrix comes down to: the width is what binds.
        assert_eq!(knob_face(28.0, 64.0, 0.0, 12.0), KnobFace { diameter: 28.0, top: 12.0 });
        // No words at all, in a square: all face.
        assert_eq!(knob_face(28.0, 28.0, 0.0, 0.0), KnobFace { diameter: 28.0, top: 0.0 });
        // A box with no room is a face of nothing, never a negative one.
        assert_eq!(knob_face(28.0, 10.0, 12.0, 12.0), KnobFace { diameter: 0.0, top: 0.0 });
    }

    /// ...and the shader finds the face by the same three lines. It cannot be
    /// run from here, so it is READ: the day somebody changes one copy of the
    /// arithmetic, this is what says there is another.
    #[test]
    fn the_shader_finds_the_face_by_the_same_arithmetic() {
        let src = include_str!("fab_controls.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a first half");
        for line in [
            "let rows = self.label_px + self.readout_px",
            "let d = max(min(self.rect_size.x, self.rect_size.y - rows), 0.0)",
            "let top = max((self.rect_size.y - rows - d) * 0.5, 0.0)",
            "let rows = label_px + readout_px;",
            "let diameter = width.min(height - rows).max(0.0);",
            "let top = ((height - rows - diameter) * 0.5).max(0.0);",
        ] {
            assert!(src.contains(line), "`{line}` is gone: the face is measured two ways now");
        }
    }

    /// The unit stands hard against the number, because the cell is 28 wide.
    #[test]
    fn a_knobs_number_carries_its_unit_without_a_space() {
        assert_eq!(knob_readout(100.0, 0, "%"), "100%");
        assert_eq!(knob_readout(37.5, 0, "%"), "38%");
        assert_eq!(knob_readout(0.25, 2, ""), "0.25");
    }

    #[test]
    fn a_double_press_is_two_presses_close_in_time_and_place() {
        let first = Some((10.0, dvec2(100.0, 100.0)));
        assert!(is_double_press(first, 10.2, dvec2(101.0, 99.0)));
        assert!(!is_double_press(first, 10.7, dvec2(100.0, 100.0)), "too late");
        assert!(!is_double_press(first, 10.2, dvec2(100.0, 120.0)), "too far");
        assert!(!is_double_press(None, 10.2, dvec2(100.0, 100.0)), "there was no first");
    }

    #[test]
    fn a_knobs_commit_is_found_after_the_change_it_follows() {
        let uid = WidgetUid(23);
        let actions: ActionsBuf = vec![
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabKnobAction::Changed(41.0)),
                widget_uid: uid,
                group: None,
            }),
            Box::new(WidgetAction {
                data: None,
                action: Box::new(FabKnobAction::Ended(42.0)),
                widget_uid: uid,
                group: None,
            }),
        ];
        assert_eq!(knob_ended_value(&actions, uid), Some(42.0));
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to the track.
///
/// One press, one owner: the face takes the mouse on the way in through
/// `hits` and holds it until the release, which is what lets a scroller
/// around this control stand its own drag down while a thumb is being
/// moved. The name is a box the release is MEASURED against, never a second
/// area asking for a hit of its own — that second ask is what once left the
/// stock Slider unable to be dragged from its own legend.
#[cfg(test)]
mod fab_slider_gestures {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    pub(super) struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        pub(super) fn new(cx: &mut Cx) -> Self {
            Target {
                pass: DrawPass::new(cx),
                draw_list: DrawList2d::new(cx),
            }
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

    pub(super) fn press(abs: Vec2d, time: f64) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    pub(super) fn moved(abs: Vec2d, time: f64) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    pub(super) fn release(abs: Vec2d, time: f64) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time,
        })
    }

    pub(super) fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
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
                    weight := FabSlider{
                        width: 300.
                        label: "one"
                        value: 50.0
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
        let weight = root.widget(cx, ids!(weight));
        assert!(!weight.is_empty(), "the scene has a slider in it");
        (root, weight)
    }

    /// The window x of a point `travel` (0..1) along the track, taken the
    /// way the control takes it.
    fn on_the_track(cx: &Cx, weight: &WidgetRef, travel: f64) -> Vec2d {
        let inner = weight.borrow::<FabSlider>().unwrap();
        let face = inner.draw_bg.area().rect(cx);
        assert!(face.size.x > 0.0, "the slider was drawn");
        let (label_px, readout_px) = inner.columns();
        let width = face.size.x - label_px - readout_px;
        let t = inner.travel();
        let (lo, hi) = t.stops();
        dvec2(
            face.pos.x + label_px + t.thumb_x(lo + travel * (hi - lo), width),
            face.pos.y + face.size.y * 0.5,
        )
    }

    fn on_the_word(cx: &Cx, weight: &WidgetRef) -> Vec2d {
        let rect = weight.borrow::<FabSlider>().unwrap().label_area.rect(cx);
        assert!(
            rect.size.x > 0.0 && rect.size.y > 0.0,
            "the name was drawn, or this test is pressing nothing"
        );
        rect.pos + rect.size * 0.5
    }

    fn changed(actions: &ActionsBuf, weight: &WidgetRef) -> Option<f64> {
        weight.as_fab_slider().changed(actions)
    }

    fn ended(actions: &ActionsBuf, weight: &WidgetRef) -> Option<f64> {
        weight.as_fab_slider().ended(actions)
    }

    fn was_reset(actions: &ActionsBuf, weight: &WidgetRef) -> bool {
        weight.as_fab_slider().was_reset(actions)
    }

    /// A press on the track lands the value under the pointer at once — no
    /// threshold, no anchor, nothing to travel through first — and the move
    /// keeps it there.
    #[test]
    fn a_press_on_the_track_jumps_the_thumb_and_then_tracks_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        let at = on_the_track(&cx, &weight, 0.25);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at, 0.0));
        assert_eq!(
            changed(&actions, &weight),
            Some(25.0),
            "the press landed the thumb"
        );
        let to = on_the_track(&cx, &weight, 0.75);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(
            changed(&actions, &weight),
            Some(75.0),
            "the drag followed the pointer"
        );
        let actions = send(&mut cx, &root, &release(to, 0.2));
        assert_eq!(ended(&actions, &weight), Some(75.0));
        cx.fingers.first_mouse_button = None;
    }

    /// The name is the reset, and the commit that follows it carries the
    /// zero rather than the value the press found.
    #[test]
    fn a_tap_on_the_name_resets_the_row_before_it_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        let at = on_the_word(&cx, &weight);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at, 0.0));
        assert_eq!(
            changed(&actions, &weight),
            None,
            "a press on the name moves nothing"
        );
        let actions = send(&mut cx, &root, &release(at, 0.0));
        assert!(was_reset(&actions, &weight), "the tap read as a reset");
        assert_eq!(
            ended(&actions, &weight),
            Some(0.0),
            "and the commit carries the zero"
        );
        cx.fingers.first_mouse_button = None;
    }

    pub(super) fn key(key_code: KeyCode, shift: bool) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers {
                shift,
                ..KeyModifiers::default()
            },
            time: 0.0,
        })
    }

    /// What a caller SETS is what the row holds.
    ///
    /// `step: 1.0` and `precision: 0` are what a weight row wants under a
    /// finger. Neither is a licence to round the host's own arithmetic on the
    /// way in: four rows splitting a hundred parts come down to 0 / 37.5 /
    /// 37.5 / 25, and rows that stored 38 put 101% on screen under a legend
    /// promising a hundred.
    #[test]
    fn a_value_set_from_outside_is_held_exactly() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (_root, weight) = start(&mut cx);
        let row = weight.as_fab_slider();
        row.set_value(&mut cx, 37.5);
        assert_eq!(
            row.value(),
            37.5,
            "the detent rounded a number nobody dragged"
        );
        // The stops are not the detent, and they still hold.
        row.set_value(&mut cx, 120.0);
        assert_eq!(row.value(), 100.0);
        row.set_value(&mut cx, -3.0);
        assert_eq!(row.value(), 0.0);
    }

    /// An arrow moves one step from where the row STANDS.
    ///
    /// Focused the way a hand focuses it -- a press on the track and a
    /// release -- and only then is the off-detent value pushed in, which is
    /// the panel's own order: the host writes the weights, the hand nudges
    /// one of them. Settling the sum onto the grid read the origin off the
    /// grid first and published 39 from a row standing at 37.5, a jump of a
    /// step and a half that the ledger behind the panel then had to absorb.
    #[test]
    fn an_arrow_moves_one_step_from_where_the_row_stands() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        let at = on_the_track(&cx, &weight, 0.5);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        // Dispatched rather than captured: `set_key_focus` only records the
        // request, and the focus moves on the cycle that runs once the
        // press's actions have gone out.
        root.handle_event(&mut cx, &press(at, 0.0), &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        cx.fingers.first_mouse_button = None;
        let face = weight.borrow::<FabSlider>().unwrap().draw_bg.area();
        assert!(
            cx.has_key_focus(face),
            "the press left the keyboard elsewhere, so the arrows below reach nothing"
        );
        weight.as_fab_slider().set_value(&mut cx, 37.5);

        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, false));
        assert_eq!(
            changed(&actions, &weight),
            Some(38.5),
            "the arrow started from the rounded number"
        );
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowLeft, false));
        assert_eq!(changed(&actions, &weight), Some(37.5), "and back again");
        // Shift is the coarse step, taken from the same true origin.
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, true));
        assert_eq!(changed(&actions, &weight), Some(47.5));
    }

    /// The detent is still the detent for the hand that is on the thumb: a
    /// drag lands on whole parts however the row was set.
    #[test]
    fn a_drag_still_lands_on_the_detent() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        weight.as_fab_slider().set_value(&mut cx, 37.5);
        let at = on_the_track(&cx, &weight, 0.617);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at, 0.0));
        let landed = changed(&actions, &weight).expect("the press moved the thumb");
        assert_eq!(landed, landed.round(), "the drag came to rest off the detent");
        send(&mut cx, &root, &release(at, 0.1));
        cx.fingers.first_mouse_button = None;
    }

    pub(super) fn key_repeat(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: true,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    pub(super) fn key_up(key_code: KeyCode) -> Event {
        Event::KeyUp(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    pub(super) fn key_with(key_code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers,
            time: 0.0,
        })
    }

    /// How many commits one event produced. A commit is the expensive word in
    /// this control's vocabulary -- the panel rebuilds a module on each one --
    /// so the COUNT is the thing under test, not whether one arrived.
    fn commits(actions: &ActionsBuf, weight: &WidgetRef) -> usize {
        actions
            .filter_widget_actions_cast::<FabSliderAction>(weight.widget_uid())
            .filter(|action| matches!(action, FabSliderAction::Ended(_)))
            .count()
    }

    /// The keyboard, taken the way a hand has to take it: there is no tab
    /// order into this row, so a press on the track and a release is the only
    /// door in. Dispatched rather than captured -- `set_key_focus` only
    /// records the request, and the focus moves on the cycle that runs once
    /// the press's actions have gone out.
    fn give_it_the_keyboard(cx: &mut Cx, root: &WidgetRef, weight: &WidgetRef) {
        let at = on_the_track(cx, weight, 0.5);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        root.handle_event(cx, &press(at, 0.0), &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        cx.fingers.first_mouse_button = None;
        let face = weight.borrow::<FabSlider>().unwrap().draw_bg.area();
        assert!(
            cx.has_key_focus(face),
            "the press left the keyboard elsewhere, so the keys below reach nothing"
        );
    }

    /// A HELD arrow is ONE gesture, not thirty.
    ///
    /// The keyboard sends a repeat every thirty-odd milliseconds. A host that
    /// reads each of them as the end of a gesture does its end-of-gesture work
    /// thirty times for one second of a held key, and the panel these rows sit
    /// in spends a module rebuild on every one of them -- a second of held
    /// arrow for a second of main thread nobody can draw on. The same second
    /// spent dragging costs a handful, because a drag reports moves and the
    /// settle behind them bounds it. This is the test that says the arrows are
    /// bounded the same way.
    #[test]
    fn a_held_arrow_commits_for_the_run_and_not_once_per_repeat() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        weight.as_fab_slider().set_value(&mut cx, 10.0);

        // The deliberate press commits where it lands: one tap of an arrow is
        // a whole gesture, and it must not wait for a release to be seen.
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, false));
        assert_eq!(changed(&actions, &weight), Some(11.0));
        assert_eq!(
            commits(&actions, &weight),
            1,
            "a single deliberate press has to land at once"
        );

        // ...and then the key is HELD. Every repeat moves the row; not one of
        // them ends a gesture the hand has not let go of.
        let mut moved = 0;
        for beat in 0..29 {
            let actions = send(&mut cx, &root, &key_repeat(KeyCode::ArrowRight));
            moved += changed(&actions, &weight).is_some() as usize;
            assert_eq!(
                commits(&actions, &weight),
                0,
                "repeat {beat} ended a gesture the hand has not let go of"
            );
        }
        assert_eq!(moved, 29, "the repeats stopped moving the row");

        // The release is the end, and it carries what the run stopped on.
        let actions = send(&mut cx, &root, &key_up(KeyCode::ArrowRight));
        assert_eq!(
            commits(&actions, &weight),
            1,
            "the run ended without a commit, so its last value never left the row"
        );
        assert_eq!(ended(&actions, &weight), Some(40.0));
        assert_eq!(weight.as_fab_slider().value(), 40.0);
    }

    /// The value a run stops on is the value the host has to end up holding.
    ///
    /// The repeats say only "moved", so the commit rides on the release -- and
    /// a release that never arrives, because the keyboard went somewhere else
    /// mid-run, must still pay what the run owes. Otherwise the last thing the
    /// hand did sits on the row and reaches nothing.
    #[test]
    fn a_run_the_keyboard_walks_out_on_still_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        weight.as_fab_slider().set_value(&mut cx, 10.0);
        send(&mut cx, &root, &key(KeyCode::ArrowRight, false));
        let actions = send(&mut cx, &root, &key_repeat(KeyCode::ArrowRight));
        assert_eq!(changed(&actions, &weight), Some(12.0));
        assert_eq!(commits(&actions, &weight), 0, "the repeat is mid-run");

        let face = weight.borrow::<FabSlider>().unwrap().draw_bg.area();
        let actions = send(
            &mut cx,
            &root,
            &Event::KeyFocus(KeyFocusEvent {
                prev: face,
                focus: Area::Empty,
            }),
        );
        assert_eq!(
            ended(&actions, &weight),
            Some(12.0),
            "the run was abandoned with a value the host had never been told to keep"
        );
        // ...and once only. What is owed is owed once.
        let actions = send(
            &mut cx,
            &root,
            &Event::KeyFocus(KeyFocusEvent {
                prev: face,
                focus: Area::Empty,
            }),
        );
        assert_eq!(commits(&actions, &weight), 0);
    }

    /// An arrow steps from what the row HOLDS, not from what it PRINTS.
    ///
    /// A host whose column is read as a whole rounds over the whole column,
    /// and the leftover parts come off the largest share -- so the biggest
    /// row of a mix prints several parts below the weight it carries.
    /// Written into the value, which was once the only way to say it, that
    /// printed number became the one the next arrow started from: the key
    /// that means MORE asked for less than the row already had, and walked
    /// the biggest theme in the mix down four parts a press. Where the gap
    /// was narrower than one step the row could not move at all, and every
    /// press still cost the host its end-of-gesture work.
    #[test]
    fn an_arrow_steps_from_what_the_row_holds_and_not_what_it_prints() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        let row = weight.as_fab_slider();
        // The shape a mix with a tail leaves: a dominant row, and four parts
        // of the column's rounding taken off it.
        row.set_value_and_readout(&mut cx, 85.5, 81.0);
        assert_eq!(row.value(), 85.5, "the weight the row stands for");
        assert_eq!(row.readout(), 81.0, "the share the column prints");

        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, false));
        assert_eq!(
            changed(&actions, &weight),
            Some(86.5),
            "the arrow stepped from the printed share, so the key that means more asked for \
             less than the row already held"
        );
        assert_eq!(commits(&actions, &weight), 1, "a deliberate press lands at once");
        assert_eq!(ended(&actions, &weight), Some(86.5));
        // ...and the row prints its own number again. A share worked out for
        // the weight before this one is a share this row no longer has.
        assert_eq!(
            row.readout(),
            86.5,
            "the row went on printing a share the hand has moved it off"
        );

        // A plain value takes the readout back with it, so a host cannot
        // leave one standing over a number it was never worked out for.
        row.set_value(&mut cx, 40.0);
        assert_eq!(row.readout(), 40.0);
    }

    /// A run the WINDOW walks out on still commits.
    ///
    /// `Hit::KeyFocusLost` does not fire when the whole app is deactivated --
    /// the focus INSIDE it has not moved -- so an arrow held while the hand
    /// alt-tabs away ended the run with nothing told about where it stopped.
    /// The repeats say only that the value moved, so a host that rate-limits
    /// those and acts on the commit was left holding the value from before
    /// the run.
    #[test]
    fn a_run_the_window_walks_out_on_still_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        weight.as_fab_slider().set_value(&mut cx, 10.0);
        send(&mut cx, &root, &key(KeyCode::ArrowRight, false));
        let actions = send(&mut cx, &root, &key_repeat(KeyCode::ArrowRight));
        assert_eq!(changed(&actions, &weight), Some(12.0));
        assert_eq!(commits(&actions, &weight), 0, "the repeat is mid-run");

        let actions = send(&mut cx, &root, &Event::WindowLostFocus(WINDOW));
        assert_eq!(
            ended(&actions, &weight),
            Some(12.0),
            "the window took the keyboard away and the run went uncommitted"
        );
        // ...and once only. What is owed is owed once.
        let actions = send(&mut cx, &root, &Event::WindowLostFocus(WINDOW));
        assert_eq!(commits(&actions, &weight), 0);
        assert_eq!(
            weight.as_fab_slider().value(),
            12.0,
            "the deactivation moved the row the hand had stopped on"
        );
    }

    /// End is ABSOLUTE: the first press names the stop, and every repeat after
    /// it names the same stop. A key held against the end of its own travel is
    /// a key this row has already answered, and answering it again is an
    /// end-of-gesture for a value that did not move.
    #[test]
    fn a_held_stop_key_commits_once_and_then_has_nothing_to_say() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        weight.as_fab_slider().set_value(&mut cx, 10.0);

        let actions = send(&mut cx, &root, &key(KeyCode::End, false));
        assert_eq!(
            changed(&actions, &weight),
            Some(100.0),
            "End is the far stop, and it goes there on the press"
        );
        assert_eq!(
            commits(&actions, &weight),
            1,
            "a stop key has to feel immediate"
        );
        for beat in 0..10 {
            let actions = send(&mut cx, &root, &key_repeat(KeyCode::End));
            assert_eq!(changed(&actions, &weight), None);
            assert_eq!(
                commits(&actions, &weight),
                0,
                "repeat {beat} commits the stop the row is already standing on"
            );
        }
        let actions = send(&mut cx, &root, &key_up(KeyCode::End));
        assert_eq!(
            commits(&actions, &weight),
            0,
            "the press paid for itself; the release owes nothing"
        );
    }

    /// Ctrl and Cmd belong to whatever the row is sitting IN.
    ///
    /// Ctrl+Home is the panel going to its top and Cmd+Arrow is the window
    /// manager's; a focused row that nudges on either is a row that broke a
    /// shortcut the rest of the app still honours -- silently, and only while
    /// the keyboard happens to be on it. Shift is this control's own.
    #[test]
    fn the_accelerator_modifiers_are_not_this_rows_to_take() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        give_it_the_keyboard(&mut cx, &root, &weight);
        let row = weight.as_fab_slider();
        row.set_value(&mut cx, 10.0);

        let held = [
            (
                "ctrl",
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            ),
            (
                "cmd",
                KeyModifiers {
                    logo: true,
                    ..KeyModifiers::default()
                },
            ),
        ];
        for (name, modifiers) in held {
            for key_code in [KeyCode::ArrowRight, KeyCode::ArrowDown, KeyCode::End] {
                let actions = send(&mut cx, &root, &key_with(key_code, modifiers));
                assert_eq!(
                    changed(&actions, &weight),
                    None,
                    "{name}+{key_code:?} moved the row"
                );
                assert_eq!(commits(&actions, &weight), 0);
            }
        }
        assert_eq!(row.value(), 10.0, "the row answered an accelerator");

        // ...and Shift, which IS the row's own, still takes the coarse step.
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, true));
        assert_eq!(changed(&actions, &weight), Some(20.0));
    }

    /// One press, one owner. This is the reading a scroller takes before it
    /// starts dragging its content, so a second hold anywhere on the row
    /// would be the panel scrolling out from under a thumb.
    #[test]
    fn the_face_is_the_only_thing_holding_that_press() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, weight) = start(&mut cx);
        let at = on_the_track(&cx, &weight, 0.5);
        let face = weight.borrow::<FabSlider>().unwrap().draw_bg.area();
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        assert!(
            cx.fingers.is_area_captured(face),
            "the track took the pointer"
        );
        assert!(
            !cx.fingers.is_mouse_held_outside(&[face]),
            "and nothing else on the row took a second hold of it"
        );
        cx.fingers.first_mouse_button = None;
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to the dial, and the rest of what
/// a hand can do to one.
///
/// The slider's harness, borrowed: a real knob, drawn once so it has an area
/// a press can land on, taking real events. What a headless run cannot do is
/// let a digit go -- the release is the platform loop's -- so the capture
/// outlives each press here, which costs these tests nothing: the knob keeps
/// its own account of where a press landed and when.
#[cfg(test)]
mod fab_knob_gestures {
    #![allow(dead_code)]
    use super::fab_slider_gestures::{key, key_repeat, key_up, press, release, send, Target};
    use super::*;
    use crate::event::{ScrollEvent, ScrollPhase};
    use std::cell::Cell;

    const WINDOW: WindowId = WindowId(1, 1);

    /// Four knobs in a row, the way a row of the matrix is: the default cell,
    /// its neighbour, the narrowest cell there is, and one with a name.
    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Right
                    cell := FabKnob{value: 50.0}
                    other := FabKnob{value: 20.0}
                    small := FabKnob{width: 28. height: 28. show_readout: false}
                    named := FabKnob{label: "mix"}
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
        let cell = root.widget(cx, ids!(cell));
        assert!(!cell.is_empty(), "the scene has a knob in it");
        (root, cell)
    }

    fn face_of(knob: &WidgetRef) -> Area {
        knob.borrow::<FabKnob>().unwrap().draw_bg.area()
    }

    /// The middle of a knob's box, which is where a hand takes hold of one.
    fn middle(cx: &Cx, knob: &WidgetRef) -> Vec2d {
        let rect = face_of(knob).rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "the knob was drawn");
        rect.pos + rect.size * 0.5
    }

    fn moved_with(abs: Vec2d, time: f64, shift: bool) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers {
                shift,
                ..KeyModifiers::default()
            },
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    /// `notches` of a real wheel over `abs`, up for positive.
    fn wheel(abs: Vec2d, notches: f64, shift: bool) -> Event {
        Event::Scroll(ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, -120.0 * notches),
            abs,
            modifiers: KeyModifiers {
                shift,
                ..KeyModifiers::default()
            },
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::Changed,
        })
    }

    fn changed(actions: &ActionsBuf, knob: &WidgetRef) -> Option<f64> {
        knob.as_fab_knob().changed(actions)
    }

    fn ended(actions: &ActionsBuf, knob: &WidgetRef) -> Option<f64> {
        knob.as_fab_knob().ended(actions)
    }

    /// Everything one knob said in one buffer, in the order it said it.
    fn said(actions: &ActionsBuf, knob: &WidgetRef) -> Vec<String> {
        actions
            .filter_widget_actions_cast::<FabKnobAction>(knob.widget_uid())
            .map(|action| format!("{action:?}"))
            .collect()
    }

    fn commits(actions: &ActionsBuf, knob: &WidgetRef) -> usize {
        actions
            .filter_widget_actions_cast::<FabKnobAction>(knob.widget_uid())
            .filter(|action| matches!(action, FabKnobAction::Ended(_)))
            .count()
    }

    /// Something for `handle_actions` to carry, and nothing else.
    #[derive(Debug)]
    struct Tick;

    /// The keyboard, taken the only way there is to take it: a press on the
    /// knob and a release that moved nothing. Dispatched rather than
    /// captured, because `set_key_focus` only records the request.
    ///
    /// The focus moves on the cycle behind an event, and the only such cycle
    /// a headless run can reach is the one `handle_actions` runs when it has
    /// an action to carry. The slider's press always has one, because a
    /// press on a track moves the thumb. A press on a knob turns nothing and
    /// says nothing, so the cycle is given a tick of its own to go round on.
    fn give_it_the_keyboard(cx: &mut Cx, root: &WidgetRef, knob: &WidgetRef) {
        let at = middle(cx, knob);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        root.handle_event(cx, &press(at, 0.0), &mut Scope::empty());
        cx.action(Tick);
        cx.handle_actions();
        root.handle_event(cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        cx.fingers.first_mouse_button = None;
        assert!(
            cx.has_key_focus(face_of(knob)),
            "the press left the keyboard elsewhere, so the keys below reach nothing"
        );
    }

    /// Up is more, the whole range in 150 points, and the gesture says
    /// `Changed` while it runs and `Ended` once when it is let go. The three
    /// points of slop are travel the value never sees.
    #[test]
    fn a_press_and_a_pull_up_raises_the_value_and_then_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at, 0.0));
        assert!(said(&actions, &cell).is_empty(), "a press turns nothing: {:?}", said(&actions, &cell));

        let up = dvec2(at.x, at.y - KNOB_DRAG_SLOP - 30.0);
        let actions = send(&mut cx, &root, &moved_with(up, 0.1, false));
        assert_eq!(changed(&actions, &cell), Some(70.0), "thirty points up is a fifth of the range");
        assert_eq!(commits(&actions, &cell), 0, "the hand is still on it");

        let down = dvec2(at.x, up.y + 15.0);
        let actions = send(&mut cx, &root, &moved_with(down, 0.2, false));
        assert_eq!(changed(&actions, &cell), Some(60.0), "and down is less");

        let actions = send(&mut cx, &root, &release(down, 0.3));
        assert_eq!(said(&actions, &cell), vec!["Ended(60.0)".to_string()]);
        assert_eq!(cell.as_fab_knob().value(), 60.0);
        cx.fingers.first_mouse_button = None;
    }

    /// Shift is a tenth of the speed -- and it can come and go in the middle
    /// of a drag without the value jumping to where the other rate would have
    /// had it, because the drag is summed move by move.
    #[test]
    fn shift_is_a_finer_drag_and_can_change_its_mind() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        let a = dvec2(at.x, at.y - KNOB_DRAG_SLOP - 30.0);
        let actions = send(&mut cx, &root, &moved_with(a, 0.1, true));
        assert_eq!(
            changed(&actions, &cell),
            Some(52.0),
            "thirty points with Shift down is a tenth of thirty points without"
        );
        // Shift let go: the next thirty points are worth the full twenty,
        // from where the fine drag left off.
        let b = dvec2(at.x, a.y - 30.0);
        let actions = send(&mut cx, &root, &moved_with(b, 0.2, false));
        assert_eq!(changed(&actions, &cell), Some(72.0));
        // A fine drag arrives a point at a time, each worth a fifteenth of
        // the detent. Summed before the detent they add up; rounded on every
        // move they would each round back to where they started.
        for point in 1..=45 {
            send(&mut cx, &root, &moved_with(dvec2(at.x, b.y - point as f64), 0.2, true));
        }
        assert_eq!(cell.as_fab_knob().value(), 75.0, "the detent swallowed a fine drag");
        let b = dvec2(at.x, b.y - 45.0);
        send(&mut cx, &root, &release(b, 0.3));
        cx.fingers.first_mouse_button = None;
    }

    /// Sideways is nothing. A hand pulling up wanders, and a drag that
    /// counted the wander would be a slightly different drag every time.
    #[test]
    fn sideways_travel_turns_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x + 200.0, at.y), 0.1, false));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x - 200.0, at.y + 1.0), 0.2, false));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        // ...and a press that moved nothing has nothing to commit.
        let actions = send(&mut cx, &root, &release(dvec2(at.x - 200.0, at.y + 1.0), 0.3));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        assert_eq!(cell.as_fab_knob().value(), 50.0);
        cx.fingers.first_mouse_button = None;
    }

    /// A careless click does not nudge the value it was only meant to select:
    /// under the slop nothing moves, and nothing is committed.
    #[test]
    fn a_wobble_under_the_slop_moves_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        // Two and a half points, written out: measured against the constant
        // this would follow it down to nothing and go on passing. Without a
        // slop they are a part and two thirds, which the detent calls two.
        let wobble = dvec2(at.x + 1.0, at.y - 2.5);
        let actions = send(&mut cx, &root, &moved_with(wobble, 0.05, false));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        let actions = send(&mut cx, &root, &release(wobble, 0.1));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        assert_eq!(cell.as_fab_knob().value(), 50.0);
        cx.fingers.first_mouse_button = None;
    }

    /// Past the stop is the stop, under the hand as under the host.
    #[test]
    fn the_value_clamps_at_both_ends() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 4000.0), 0.1, false));
        assert_eq!(changed(&actions, &cell), Some(100.0));
        // The overshoot is not wound up: the first points of the way back
        // already count.
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 3985.0), 0.15, false));
        assert_eq!(changed(&actions, &cell), Some(90.0), "the pointer had a mile to unwind first");
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x, at.y + 4000.0), 0.2, false));
        assert_eq!(changed(&actions, &cell), Some(0.0));
        let actions = send(&mut cx, &root, &release(dvec2(at.x, at.y + 4000.0), 0.3));
        assert_eq!(ended(&actions, &cell), Some(0.0));
        cx.fingers.first_mouse_button = None;

        let knob = cell.as_fab_knob();
        knob.set_value(&mut cx, 120.0);
        assert_eq!(knob.value(), 100.0);
        knob.set_value(&mut cx, -3.0);
        assert_eq!(knob.value(), 0.0);
    }

    /// What a host SETS is what the knob holds -- the detent is the hand's --
    /// and setting it says nothing to anybody. A panel that pushed a hundred
    /// weights into a matrix and heard a hundred `Changed` back would install
    /// the mix it had just installed.
    #[test]
    fn a_value_set_from_outside_is_held_exactly_and_emits_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (_root, cell) = start(&mut cx);
        let knob = cell.as_fab_knob();
        let actions = cx.capture_actions(|cx| {
            knob.set_value(cx, 37.5);
        });
        assert!(actions.is_empty(), "set_value spoke");
        assert_eq!(knob.value(), 37.5, "the detent rounded a number nobody dragged");
        assert_eq!(knob.readout(), 37.5);

        let actions = cx.capture_actions(|cx| {
            knob.set_value_and_readout(cx, 85.5, 81.0);
        });
        assert!(actions.is_empty(), "set_value_and_readout spoke");
        assert_eq!(knob.value(), 85.5, "the weight the knob stands for");
        assert_eq!(knob.readout(), 81.0, "the share the column prints");
        // A plain value takes the readout back with it.
        knob.set_value(&mut cx, 40.0);
        assert_eq!(knob.readout(), 40.0);
    }

    /// A double click is the reset: nought, said as a change, a reset and a
    /// commit, in that order and on the second PRESS. The first press of the
    /// pair turned nothing and so committed nothing.
    #[test]
    fn a_double_click_resets_the_knob_and_commits_the_nought() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at, 1.0));
        assert!(said(&actions, &cell).is_empty());
        let actions = send(&mut cx, &root, &release(at, 1.05));
        assert!(said(&actions, &cell).is_empty(), "the first click of the pair spoke: {:?}", said(&actions, &cell));

        let actions = send(&mut cx, &root, &press(at, 1.2));
        assert_eq!(
            said(&actions, &cell),
            vec!["Changed(0.0)".to_string(), "Reset".to_string(), "Ended(0.0)".to_string()]
        );
        assert!(cell.as_fab_knob().was_reset(&actions));
        assert_eq!(cell.as_fab_knob().value(), 0.0);
        // The second press is a command, not a grip: it turns nothing.
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 60.0), 1.25, false));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        let actions = send(&mut cx, &root, &release(at, 1.3));
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        cx.fingers.first_mouse_button = None;
    }

    /// Two clicks that are NOT a double click: too far apart in time, and a
    /// drag followed at once by a press.
    #[test]
    fn two_separate_presses_are_not_a_reset() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 1.0));
        send(&mut cx, &root, &release(at, 1.05));
        let actions = send(&mut cx, &root, &press(at, 2.0));
        assert!(!cell.as_fab_knob().was_reset(&actions), "a slow second click reset the knob");
        // That press becomes a drag that comes back to where it began...
        send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 40.0), 2.05, false));
        send(&mut cx, &root, &moved_with(at, 2.1, false));
        send(&mut cx, &root, &release(at, 2.15));
        // ...and a press straight after it, on the same spot, is a new grip.
        let actions = send(&mut cx, &root, &press(at, 2.2));
        assert!(!cell.as_fab_knob().was_reset(&actions), "a drag counted as half a double click");
        send(&mut cx, &root, &release(at, 2.25));
        cx.fingers.first_mouse_button = None;
    }

    /// One press, one owner, and nothing else reacts meanwhile: the drag
    /// leaves the cell on its way up -- on a 28 point knob every drag does --
    /// and the knob it passes over neither lights up nor moves.
    #[test]
    fn the_knob_holds_its_press_and_the_neighbour_stays_out_of_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let other = root.widget(&cx, ids!(other));
        let at = middle(&cx, &cell);
        let over_there = middle(&cx, &other);
        let face = face_of(&cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        assert!(cx.fingers.is_area_captured(face), "the knob took the pointer");
        assert!(
            !cx.fingers.is_mouse_held_outside(&[face]),
            "and nothing else took a second hold of it"
        );
        let actions = send(&mut cx, &root, &moved_with(dvec2(over_there.x, over_there.y - 30.0), 0.1, false));
        assert!(changed(&actions, &cell).is_some(), "the drag went on turning the knob it began on");
        assert!(said(&actions, &other).is_empty(), "the neighbour answered a drag that is not its own");
        assert!(!other.borrow::<FabKnob>().unwrap().hovered, "the neighbour lit up under a held pointer");
        assert_eq!(other.as_fab_knob().value(), 20.0);
        cx.fingers.first_mouse_button = None;
    }

    /// The arrows, once the knob has the keyboard: one step from where it
    /// stands, Shift the coarse step, and the commit rules of the slider --
    /// a press commits where it lands, a held key commits at its release.
    #[test]
    fn the_arrows_step_a_knob_that_has_the_keyboard() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        // Before it has the keyboard, the keys are somebody else's.
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowUp, false));
        assert!(said(&actions, &cell).is_empty());

        give_it_the_keyboard(&mut cx, &root, &cell);
        cell.as_fab_knob().set_value(&mut cx, 37.5);
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowUp, false));
        assert_eq!(said(&actions, &cell), vec!["Changed(38.5)".to_string(), "Ended(38.5)".to_string()]);
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowDown, false));
        assert_eq!(changed(&actions, &cell), Some(37.5));
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight, true));
        assert_eq!(changed(&actions, &cell), Some(47.5), "Shift is the coarse step");

        let mut run = 0;
        for _ in 0..9 {
            let actions = send(&mut cx, &root, &key_repeat(KeyCode::ArrowUp));
            run += commits(&actions, &cell);
        }
        assert_eq!(run, 0, "a held arrow committed on its repeats");
        let actions = send(&mut cx, &root, &key_up(KeyCode::ArrowUp));
        assert_eq!(said(&actions, &cell), vec!["Ended(56.5)".to_string()]);

        let actions = send(&mut cx, &root, &key(KeyCode::Home, false));
        assert_eq!(ended(&actions, &cell), Some(0.0));
        let actions = send(&mut cx, &root, &key(KeyCode::End, false));
        assert_eq!(ended(&actions, &cell), Some(100.0));
    }

    /// The wheel steps the knob that has the keyboard and leaves every other
    /// knob alone, so a wheel crossing the matrix scrolls the panel instead
    /// of walking a column of weights. A spin is one gesture: every notch
    /// says `Changed`, and the one commit waits for the wheel to go still.
    #[test]
    fn the_wheel_steps_the_knob_that_has_the_keyboard_and_commits_once() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);

        // Merely under the pointer: not this knob's wheel, and said so by
        // leaving the event unspent for the scroller around it.
        let event = wheel(at, 1.0, false);
        let actions = send(&mut cx, &root, &event);
        assert!(said(&actions, &cell).is_empty(), "{:?}", said(&actions, &cell));
        assert!(!event.scroll_handled(Vec2Index::Y), "an idle knob ate the panel's wheel");

        give_it_the_keyboard(&mut cx, &root, &cell);
        let mut heard = Vec::new();
        for _ in 0..3 {
            let event = wheel(at, 1.0, false);
            let actions = send(&mut cx, &root, &event);
            heard.extend(said(&actions, &cell));
            assert!(event.scroll_handled(Vec2Index::Y), "the knob's wheel also scrolled the panel");
        }
        assert_eq!(heard, vec!["Changed(51.0)", "Changed(52.0)", "Changed(53.0)"]);
        let actions = send(&mut cx, &root, &wheel(at, -1.0, true));
        assert_eq!(said(&actions, &cell), vec!["Changed(43.0)".to_string()], "Shift is the coarse step");

        // A trackpad's notch arrives in pieces, and the pieces add up.
        let mut heard = Vec::new();
        for _ in 0..4 {
            let actions = send(&mut cx, &root, &wheel(at, 0.25, false));
            heard.extend(said(&actions, &cell));
        }
        assert_eq!(heard, vec!["Changed(44.0)"]);

        // The wheel goes still, and the spin is over.
        let timer = cell.borrow::<FabKnob>().unwrap().wheel_timer;
        assert_ne!(timer.0, 0, "nothing is waiting to commit the spin");
        let settle = Event::Timer(TimerEvent {
            time: None,
            timer_id: timer.0,
        });
        let actions = send(&mut cx, &root, &settle);
        assert_eq!(said(&actions, &cell), vec!["Ended(44.0)".to_string()]);
        // ...once. What is owed is owed once.
        let actions = send(&mut cx, &root, &settle);
        assert!(said(&actions, &cell).is_empty());

        // At the stop the wheel still belongs to the knob, and says nothing.
        cell.as_fab_knob().set_value(&mut cx, 100.0);
        let event = wheel(at, 1.0, false);
        let actions = send(&mut cx, &root, &event);
        assert!(said(&actions, &cell).is_empty());
        assert!(event.scroll_handled(Vec2Index::Y));
    }

    /// A spin the keyboard walks out on still commits, as a run of arrows
    /// does: the timer it was waiting on belongs to a knob that no longer
    /// has the wheel.
    #[test]
    fn a_spin_the_keyboard_walks_out_on_still_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        give_it_the_keyboard(&mut cx, &root, &cell);
        send(&mut cx, &root, &wheel(at, 2.0, false));
        let actions = send(
            &mut cx,
            &root,
            &Event::KeyFocus(KeyFocusEvent {
                prev: face_of(&cell),
                focus: Area::Empty,
            }),
        );
        assert_eq!(said(&actions, &cell), vec!["Ended(52.0)".to_string()]);
        let actions = send(&mut cx, &root, &Event::WindowLostFocus(WINDOW));
        assert!(said(&actions, &cell).is_empty(), "the spin was paid for twice");
    }

    /// `wheel_on_hover` is the host saying its knobs stand somewhere that
    /// does not scroll.
    #[test]
    fn a_knob_told_to_takes_the_wheel_it_is_merely_under() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        cell.borrow_mut::<FabKnob>().unwrap().wheel_on_hover = true;
        let event = wheel(middle(&cx, &cell), 1.0, false);
        let actions = send(&mut cx, &root, &event);
        assert_eq!(changed(&actions, &cell), Some(51.0));
        assert!(event.scroll_handled(Vec2Index::Y));
    }

    /// Switched off, a knob answers nothing -- and switching it off in the
    /// middle of a drag puts back what the press found.
    #[test]
    fn a_knob_switched_off_lets_go_and_goes_quiet() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.0));
        send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 63.0), 0.1, false));
        assert_eq!(cell.as_fab_knob().value(), 90.0);
        let actions = cx.capture_actions(|cx| cell.as_fab_knob().set_enabled(cx, false));
        assert_eq!(
            cell.as_fab_knob().changed(&actions),
            Some(50.0),
            "the drag that was cut short left its value behind"
        );
        let actions = send(&mut cx, &root, &moved_with(dvec2(at.x, at.y - 120.0), 0.2, false));
        assert!(said(&actions, &cell).is_empty());
        assert_eq!(cell.as_fab_knob().value(), 50.0);
        cx.fingers.first_mouse_button = None;
    }

    /// A drag that is let go somewhere else leaves the knob unlit. No
    /// hover-out follows a release off the area, so a knob that waited for
    /// one stayed lit until the pointer happened to cross it again -- seen in
    /// a running window, on the first drag that was ever tried on one.
    #[test]
    fn a_drag_let_go_off_the_knob_does_not_leave_it_lit() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let at = middle(&cx, &cell);
        send(&mut cx, &root, &moved_with(at, 0.0, false));
        assert!(cell.borrow::<FabKnob>().unwrap().hovered, "the pointer arrived and nothing lit");
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 0.1));
        let away = dvec2(at.x, at.y - 200.0);
        send(&mut cx, &root, &moved_with(away, 0.2, false));
        send(&mut cx, &root, &release(away, 0.3));
        cx.fingers.first_mouse_button = None;
        assert!(!cell.borrow::<FabKnob>().unwrap().hovered, "the knob is still lit with the pointer gone");

        // ...and one let go ON the knob keeps the light the pointer earns it.
        send(&mut cx, &root, &moved_with(at, 0.4, false));
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(at, 1.5));
        send(&mut cx, &root, &release(at, 1.6));
        cx.fingers.first_mouse_button = None;
        assert!(cell.borrow::<FabKnob>().unwrap().hovered);
    }

    /// The box is whatever the cell says, the face is what the words leave of
    /// it, and a knob with no name has no row for one.
    #[test]
    fn the_knob_takes_the_size_of_its_cell_and_drops_the_rows_it_has_no_words_for() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, cell) = start(&mut cx);
        let small = root.widget(&cx, ids!(small));
        let named = root.widget(&cx, ids!(named));
        assert_eq!(face_of(&cell).rect(&cx).size, dvec2(44.0, 64.0), "the default box");
        assert_eq!(face_of(&small).rect(&cx).size, dvec2(28.0, 28.0), "the cell's box");
        assert_eq!(cell.borrow::<FabKnob>().unwrap().text_rows(), (0.0, 12.0));
        assert_eq!(small.borrow::<FabKnob>().unwrap().text_rows(), (0.0, 0.0));
        assert_eq!(named.borrow::<FabKnob>().unwrap().text_rows(), (12.0, 12.0));
        // What the shader was handed is what the layout measured with.
        let inner = named.borrow::<FabKnob>().unwrap();
        assert_eq!((inner.draw_bg.label_px, inner.draw_bg.readout_px), (12.0, 12.0));
        // Nought is drawn as off, and the travel is what says so.
        assert_eq!(inner.draw_bg.travel, 0.0);
        assert_eq!(cell.borrow::<FabKnob>().unwrap().draw_bg.travel, 0.5);
    }
}

/// The header, drawn. What the arithmetic says about where a name goes is
/// only worth something if that is where the widget actually put it, and a
/// name written in by a host has to reach the screen.
#[cfg(test)]
mod fab_diagonal_label_draw {
    #![allow(dead_code)]
    use super::fab_slider_gestures::Target;
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };

    /// A header row the way a matrix wants one: columns at the pitch a 280
    /// wide sidebar comes down to, leaning both ways, and one with no name
    /// behind it.
    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    View{
                        width: Fit
                        height: Fit
                        flow: Right
                        clip_x: false
                        clip_y: false
                        first := FabDiagonalLabel{width: 26. text: "Windows 2000"}
                        second := FabDiagonalLabel{width: 26. text: "Dark"}
                        rising := FabDiagonalLabel{width: 26. text: "Windows 2000" lean: DiagonalLean.Rise}
                        blank := FabDiagonalLabel{width: 26.}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, Target) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        (root, target)
    }

    fn run_of(widget: &WidgetRef) -> DiagonalRun {
        widget
            .borrow::<FabDiagonalLabel>()
            .expect("it is a FabDiagonalLabel")
            .last_run()
            .expect("it drew a name")
    }

    fn box_of(widget: &WidgetRef, cx: &Cx) -> Rect {
        let area = widget
            .borrow::<FabDiagonalLabel>()
            .expect("it is a FabDiagonalLabel")
            .area;
        area.rect(cx)
    }

    /// `row_height_for` shapes the name, and shaping wants a live `Cx2d`
    /// even though nothing is drawn with it.
    fn measure(cx: &mut Cx, widget: &WidgetRef, text: &str) -> f64 {
        let pass = DrawPass::new(cx);
        let mut draw_list = DrawList2d::new(cx);
        pass.set_size(cx, SIZE);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(SIZE, Layout::flow_down());
        let height = widget
            .borrow::<FabDiagonalLabel>()
            .expect("it is a FabDiagonalLabel")
            .row_height_for(&mut cx2d, text);
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
        height
    }

    /// The drawn name lands where the pure placement says it does, its ink
    /// leaves its own 26 point box, and the box it claimed is still only 26
    /// wide — which together is the whole claim: the widget takes a column
    /// and paints across the panel.
    #[test]
    fn the_drawn_name_stands_on_its_own_column_and_leaves_its_box() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = start(&mut cx);
        let first = root.widget(&cx, ids!(first));
        let box_ = box_of(&first, &cx);
        assert_eq!(box_.size.x, 26.0, "the claimed box is one column");
        let run = run_of(&first);
        // The baseline ends on the middle of the claimed box's bottom edge.
        let centre = dvec2(box_.pos.x + box_.size.x * 0.5, box_.pos.y + box_.size.y);
        assert!((run.end - centre).length() < 1e-6, "{:?} is not {centre:?}", run.end);
        assert!(run.angle > 0.0, "a fall turns clockwise on screen");
        // The ink is wider than the column, which is the reason to have it.
        assert!(run.bounds.size.x > 26.0, "the name stayed inside its box");
        assert!(run.bounds.pos.x < box_.pos.x, "the name did not hang left");
        // ...and no taller than the row the template ships: the default
        // height holds the longest theme name the library carries, which is
        // the name in this scene.
        assert!(
            run.bounds.size.y <= box_.size.y,
            "the template's {} is too short for {}",
            box_.size.y,
            run.bounds.size.y
        );
    }

    /// The rise leans the other way: the same anchor, the opposite side.
    #[test]
    fn a_rising_name_starts_where_a_falling_one_ends() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = start(&mut cx);
        let rising = root.widget(&cx, ids!(rising));
        let box_ = box_of(&rising, &cx);
        let run = run_of(&rising);
        let centre = dvec2(box_.pos.x + box_.size.x * 0.5, box_.pos.y + box_.size.y);
        assert!((run.start - centre).length() < 1e-6);
        assert!(run.angle < 0.0, "a rise turns anticlockwise on screen");
        assert!(
            run.bounds.pos.x + run.bounds.size.x > box_.pos.x + box_.size.x,
            "the name did not hang right"
        );
    }

    /// A shorter name takes less room, and one with nothing in it draws
    /// nothing — a matrix leaves columns it has nothing to name blank, and
    /// they still have to keep their place in the row.
    #[test]
    fn a_shorter_name_takes_less_room_and_an_empty_one_takes_none() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = start(&mut cx);
        let long = run_of(&root.widget(&cx, ids!(first)));
        let short = run_of(&root.widget(&cx, ids!(second)));
        assert!(
            short.bounds.size.y < long.bounds.size.y,
            "a short name wants less height: {} vs {}",
            short.bounds.size.y,
            long.bounds.size.y
        );
        let blank = root.widget(&cx, ids!(blank));
        assert!(
            blank
                .borrow::<FabDiagonalLabel>()
                .unwrap()
                .last_run()
                .is_none(),
            "an empty header drew something"
        );
        assert_eq!(box_of(&blank, &cx).size.x, 26.0, "it still claimed its column");
    }

    /// The name a host writes in is the name that is drawn, and asking for
    /// it asks for the frame that shows it. Writing the same name again is
    /// silent: a matrix fills every header on every draw, and a setter
    /// that dirtied the list each time would redraw the panel forever.
    #[test]
    fn a_written_name_is_held_and_asks_for_the_frame_that_shows_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let first = root.widget(&cx, ids!(first));
        assert_eq!(
            first.borrow::<FabDiagonalLabel>().unwrap().text(),
            "Windows 2000"
        );
        let was = run_of(&first).bounds.size.y;

        cx.new_draw_event = Default::default();
        first
            .borrow_mut::<FabDiagonalLabel>()
            .unwrap()
            .set_text(&mut cx, "Windows 2000");
        assert!(
            !cx.new_draw_event.will_redraw(),
            "the same name asked for a frame"
        );

        first
            .borrow_mut::<FabDiagonalLabel>()
            .unwrap()
            .set_text(&mut cx, "NeXTSTEP");
        assert!(cx.new_draw_event.will_redraw(), "a new name asked for no frame");
        assert_eq!(first.borrow::<FabDiagonalLabel>().unwrap().text(), "NeXTSTEP");

        // ...and the next frame draws it, shorter than what it replaced.
        target.draw(&mut cx, &root);
        assert!(run_of(&first).bounds.size.y < was);
    }

    /// The row height a host fixes its header at is the height the longest
    /// name it will write actually needs.
    #[test]
    fn the_widget_can_say_how_tall_the_header_row_has_to_be() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = start(&mut cx);
        let first = root.widget(&cx, ids!(first));
        let drawn = run_of(&first).bounds.size.y;
        let asked = measure(&mut cx, &first, "Windows 2000");
        assert!((asked - drawn).abs() < 1e-6, "asked {asked}, drew {drawn}");
        // A longer name than any theme carries wants a taller row.
        assert!(measure(&mut cx, &first, "Windows 2000 dark") > asked);
    }
}

/// A row that LOOKS like a slider, made to behave like one.
///
/// The filled rows in the colour popover were scrubs wearing a fill: the
/// press only armed, three pixels of travel engaged a relative drag at a
/// quarter of the range per row-width, and the pointer was pinned out of
/// sight while it ran. Pressing the fill at three quarters and pulling left
/// therefore did nothing visible at all, which is what "I cannot move the
/// sliders" meant. These say what a track row does instead, and the first of
/// them fails on the scrub.
#[cfg(test)]
mod fab_value_input_track {
    use super::fab_slider_gestures::{moved, press, release, send, Target};
    use super::*;
    use crate::event::{ScrollEvent, ScrollPhase};
    use std::cell::Cell;

    const WINDOW: WindowId = WindowId(1, 1);

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    band := FabValueInput{
                        width: 228.
                        label: "R"
                        min: 0.0
                        max: 255.0
                        step: 1.0
                        precision: 0
                        show_fill: true
                        quantize: true
                        track: true
                    }
                    scrub := FabValueInput{
                        width: 228.
                        label: "R"
                        min: 0.0
                        max: 255.0
                        step: 1.0
                        precision: 0
                        show_fill: true
                        quantize: true
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let band = root.widget(cx, ids!(band));
        let scrub = root.widget(cx, ids!(scrub));
        assert!(!band.is_empty() && !scrub.is_empty(), "the scene has both rows");
        (root, band, scrub)
    }

    fn face(cx: &Cx, row: &WidgetRef) -> Rect {
        let rect = row.borrow::<FabValueInput>().unwrap().draw_bg.area().rect(cx);
        assert!(rect.size.x > 0.0, "the row was drawn");
        rect
    }

    /// A window point `t` (0..1) along the row's fill — the span the row
    /// itself paints the fill across and reads a press against.
    fn along(cx: &Cx, row: &WidgetRef, t: f64) -> Vec2d {
        let f = face(cx, row);
        let (lo, hi) = row.borrow::<FabValueInput>().unwrap().track_span(f.size.x);
        assert!(hi > lo + 10.0, "the track has no room left between its columns");
        dvec2(f.pos.x + lo + t * (hi - lo), f.pos.y + f.size.y * 0.5)
    }

    /// A window point inside one of the two outer columns.
    fn on_the_name(cx: &Cx, row: &WidgetRef) -> Vec2d {
        let f = face(cx, row);
        let (name, _) = row.borrow::<FabValueInput>().unwrap().track_columns;
        assert!(name > 2.0, "the name column was never measured");
        dvec2(f.pos.x + name * 0.5, f.pos.y + f.size.y * 0.5)
    }

    fn on_the_number(cx: &Cx, row: &WidgetRef) -> Vec2d {
        let f = face(cx, row);
        let (_, number) = row.borrow::<FabValueInput>().unwrap().track_columns;
        assert!(number > 2.0, "the number column was never measured");
        dvec2(
            f.pos.x + f.size.x - number * 0.5,
            f.pos.y + f.size.y * 0.5,
        )
    }

    fn value(row: &WidgetRef) -> f64 {
        row.borrow::<FabValueInput>().unwrap().value()
    }

    /// Who claimed a press on its way through the page. With no event loop
    /// here to end a capture on the release, the area a press captured is
    /// let go by hand afterwards, or it would take every later press.
    fn claimed(event: &Event) -> Area {
        match event {
            Event::MouseDown(e) => e.handled.get(),
            Event::MouseMove(e) => e.handled.get(),
            _ => Area::Empty,
        }
    }

    fn said(actions: &ActionsBuf, row: &WidgetRef) -> Vec<FabValueInputAction> {
        actions
            .filter_widget_actions_cast::<FabValueInputAction>(row.widget_uid())
            .collect()
    }

    fn changed(actions: &ActionsBuf, row: &WidgetRef) -> Option<f64> {
        said(actions, row).into_iter().find_map(|a| match a {
            FabValueInputAction::Changed(v) => Some(v),
            _ => None,
        })
    }

    fn ended(actions: &ActionsBuf, row: &WidgetRef) -> Option<f64> {
        said(actions, row).into_iter().find_map(|a| match a {
            FabValueInputAction::Ended(v) => Some(v),
            _ => None,
        })
    }

    fn wheel(abs: Vec2d, notches: f64) -> Event {
        Event::Scroll(ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, -120.0 * notches),
            abs,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::Changed,
        })
    }

    fn key(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// The mapping, with no window anywhere near it: a point along the fill
    /// names the value that point of the fill stands for.
    #[test]
    fn a_point_on_the_track_names_the_value_that_point_means() {
        let p = DragParams {
            min: 0.0,
            max: 255.0,
            step: 1.0,
            wrap: false,
            bounded: true,
            snap_override: 0.0,
        };
        // A fill running from 16 to 192: its two ends are the two ends of
        // the range, and its middle is the middle.
        assert_eq!(track_value_at(&p, 16.0, 16.0, 192.0), 0.0);
        assert_eq!(track_value_at(&p, 192.0, 16.0, 192.0), 255.0);
        assert!((track_value_at(&p, 104.0, 16.0, 192.0) - 127.5).abs() < 1e-9);
        // Off either end is the end, not a number past it.
        assert_eq!(track_value_at(&p, -40.0, 16.0, 192.0), 0.0);
        assert_eq!(track_value_at(&p, 900.0, 16.0, 192.0), 255.0);
    }

    /// The press lands the value under the pointer at once, and the drag
    /// keeps it there. This is the one the scrub fails.
    #[test]
    fn a_press_on_the_track_lands_the_value_and_the_drag_keeps_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(
            changed(&actions, &band),
            Some(128.0),
            "the press did not land the value it was pointing at"
        );
        let to = along(&cx, &band, 0.75);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(
            changed(&actions, &band),
            Some(191.0),
            "the drag did not follow the pointer"
        );
        let actions = send(&mut cx, &root, &release(to, 0.2));
        assert_eq!(ended(&actions, &band), Some(191.0), "the release did not commit");
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// And the field that did NOT ask for a track still scrubs: it arms on
    /// the press and changes nothing until the travel threshold is past.
    #[test]
    fn a_field_that_did_not_ask_for_a_track_still_scrubs() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _band, scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &scrub, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(changed(&actions, &scrub), None, "the press moved a scrub field");
        assert_eq!(value(&scrub), 0.0);
        send(&mut cx, &root, &release(at, 0.1));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// The number is still the way in to typing, and Return still commits.
    #[test]
    fn a_click_on_the_number_still_opens_typing_and_return_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = on_the_number(&cx, &band);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(
            changed(&actions, &band),
            None,
            "the press on the number moved the value"
        );
        send(&mut cx, &root, &release(at, 0.1));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(
            band.borrow::<FabValueInput>().unwrap().editing,
            "a click on the number did not open the editor"
        );
        // What Return does with the typed text, taken at the seam the
        // editor's own action arrives at.
        let uid = band.widget_uid();
        let actions = cx.capture_actions(|cx| {
            band.borrow_mut::<FabValueInput>()
                .unwrap()
                .commit_edit_text(cx, uid, "200");
        });
        assert_eq!(ended(&actions, &band), Some(200.0), "the typed value never committed");
        assert_eq!(value(&band), 200.0);
        assert!(!band.borrow::<FabValueInput>().unwrap().editing);
    }

    /// The name is the reset, the way the panel's sliders read it.
    #[test]
    fn a_tap_on_the_name_asks_for_a_reset_and_moves_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        band.borrow_mut::<FabValueInput>().unwrap().set_value(&mut cx, 100.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = on_the_name(&cx, &band);
        let down = press(at, 0.0);
        let mut actions = send(&mut cx, &root, &down);
        assert_eq!(changed(&actions, &band), None, "the press on the name moved the value");
        actions.extend(send(&mut cx, &root, &release(at, 0.1)));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(
            said(&actions, &band)
                .iter()
                .any(|a| matches!(a, FabValueInputAction::Reset)),
            "the tap on the name did not read as a reset"
        );
        assert_eq!(value(&band), 100.0, "the reset moved the row itself");
    }

    /// The wheel over the row steps it, and the arrows step it once a press
    /// has left the keyboard here.
    #[test]
    fn the_wheel_and_the_arrows_still_step_the_row() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        let over = along(&cx, &band, 0.5);
        let actions = send(&mut cx, &root, &wheel(over, 1.0));
        assert_eq!(changed(&actions, &band), Some(1.0), "the wheel did not step the row");
        let actions = send(&mut cx, &root, &wheel(over, -1.0));
        assert_eq!(changed(&actions, &band), Some(0.0), "and back again");

        // The keyboard the only way a hand gets it: a press and a release,
        // dispatched, because `set_key_focus` only records the request.
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        root.handle_event(&mut cx, &down, &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        let area = band.borrow::<FabValueInput>().unwrap().draw_bg.area();
        assert!(cx.has_key_focus(area), "the press left the keyboard elsewhere");
        let stood = value(&band);
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight));
        assert_eq!(changed(&actions, &band), Some(stood + 1.0), "the arrow did not step");
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowLeft));
        assert_eq!(changed(&actions, &band), Some(stood), "and back again");
    }

    /// The seven rows of the colour popover, as the popover declares them.
    fn seven(cx: &mut Cx) -> (WidgetRef, Vec<WidgetRef>) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 244.
                    height: Fill
                    flow: Down
                    num_r := FabValueInput{ label: "R" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_g := FabValueInput{ label: "G" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_b := FabValueInput{ label: "B" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_a := FabValueInput{ label: "A" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_h := FabValueInput{ label: "H" min: 0.0 max: 360.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_s := FabValueInput{ label: "S" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                    num_v := FabValueInput{ label: "V" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true quantize: true track: true }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let rows = [
            ids!(num_r), ids!(num_g), ids!(num_b), ids!(num_a),
            ids!(num_h), ids!(num_s), ids!(num_v),
        ]
        .into_iter()
        .map(|id| root.widget(cx, id))
        .collect::<Vec<_>>();
        (root, rows)
    }

    /// Every box one width, that width holding three figures, and every
    /// track therefore ending at the same x.
    ///
    /// The widths are not set to a number anywhere: each row measures the
    /// widest digit of its own font and multiplies by how many figures its
    /// range can print, which is three on all seven (255, 360, 100). That is
    /// why they agree, and why they would still agree in another font.
    #[test]
    fn the_seven_boxes_are_one_column_of_one_width_fitting_three_figures() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (_root, rows) = seven(&mut cx);
        let widths = rows
            .iter()
            .map(|row| row.borrow::<FabValueInput>().unwrap().track_columns.1)
            .collect::<Vec<_>>();
        let first = widths[0];
        assert!(first > 2.0, "the readout column was never measured");
        // The well is the column less the row's own right padding, and the
        // shader draws one at all only above two points: a row whose readout
        // is not a box says nothing about being typed in.
        let well = rows[0].borrow::<FabValueInput>().unwrap().draw_bg.num_box as f64;
        let pad = rows[0].borrow::<FabValueInput>().unwrap().layout.padding.right;
        assert!(
            well > 2.0,
            "the readout is not drawn as a box, so nothing says it can be typed in"
        );
        assert!(
            (well + pad - first).abs() < 0.01,
            "the box drawn and the column measured are different widths: {well} + {pad} against {first}"
        );
        for (row, width) in rows.iter().zip(&widths) {
            assert!(
                (width - first).abs() < 0.01,
                "the boxes are not one width: {widths:?}"
            );
            // The track ends where the box begins, so one column means one
            // end for every track.
            let face = row.borrow::<FabValueInput>().unwrap().draw_bg.area().rect(&cx);
            let (_, hi) = row.borrow::<FabValueInput>().unwrap().track_span(face.size.x);
            assert!(
                ((face.pos.x + hi) - (rows[0].borrow::<FabValueInput>().unwrap().draw_bg.area().rect(&cx).pos.x
                    + rows[0].borrow::<FabValueInput>().unwrap().track_span(face.size.x).1))
                    .abs()
                    < 0.01,
                "the tracks end in different places"
            );
        }

        // And three figures really do fit: the widest three-digit string the
        // font can draw, measured in the font, against the box the row drew.
        let padding = rows[0].borrow::<FabValueInput>().unwrap().layout.padding.right;
        let measure = |cx: &mut Cx, text: &str| {
            let row = rows[0].borrow::<FabValueInput>().unwrap();
            row.draw_text
                .layout(cx, 0.0, 0.0, None, false, Align::default(), text)
                .rows
                .first()
                .map_or(0.0, |r| r.width_in_lpxs as f64)
        };
        let mut widest = String::new();
        for _ in 0..3 {
            let mut worst = ('0', 0.0_f64);
            for digit in "0123456789".chars() {
                let w = measure(&mut cx, &digit.to_string());
                if w > worst.1 {
                    worst = (digit, w);
                }
            }
            widest.push(worst.0);
        }
        let drawn = measure(&mut cx, &widest);
        assert!(drawn > 0.0, "the font measured nothing, so this proves nothing");
        assert!(
            first - padding >= drawn,
            "three figures ({widest}, {drawn:.2} points) do not fit the {first:.2}-point box"
        );
    }

    /// The box at the row's right-hand end is a place a number is typed, and
    /// it is its own target: a press in it opens the editor and moves
    /// nothing, and a press on the track moves the value and opens nothing.
    ///
    /// The box is not a separate widget — it is the row's own embedded editor
    /// drawn in the row's own face — so which of the three it is is decided
    /// by the column the press lands in, and the columns are the ones the
    /// draw measured.
    #[test]
    fn the_box_types_and_the_track_drags_and_neither_does_the_others_job() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        // Hoisted: taken inside a `borrow_mut` it would be asked of a
        // widget already borrowed and come back as nothing.
        let uid = band.widget_uid();
        let editing = || band.borrow::<FabValueInput>().unwrap().editing;
        let tracking = || band.borrow::<FabValueInput>().unwrap().tracking;

        // A press in the box: nothing moves, and the release opens it.
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = on_the_number(&cx, &band);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(changed(&actions, &band), None, "a press in the box moved the value");
        assert!(!tracking(), "a press in the box started a drag of the track");
        send(&mut cx, &root, &release(at, 0.1));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(editing(), "a press in the box did not open it for typing");

        // A number typed there commits on Return, and the fill follows it.
        let before = band.borrow::<FabValueInput>().unwrap().draw_bg.fill;
        let actions = cx.capture_actions(|cx| {
            band.borrow_mut::<FabValueInput>()
                .unwrap()
                .commit_edit_text(cx, uid, "200");
        });
        assert_eq!(ended(&actions, &band), Some(200.0), "Return did not commit what was typed");
        assert_eq!(value(&band), 200.0, "the row is not holding what was typed");
        assert!(!editing(), "the editor stayed open after Return");
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        assert!(
            band.borrow::<FabValueInput>().unwrap().draw_bg.fill > before,
            "the fill did not follow what was typed"
        );

        // Out of range is held to the row's own ends, not refused.
        cx.capture_actions(|cx| {
            band.borrow_mut::<FabValueInput>()
                .unwrap()
                .commit_edit_text(cx, uid, "900");
        });
        assert_eq!(value(&band), 255.0, "a number past the end was refused instead of held");

        // Escape puts back what was there.
        let stood = value(&band);
        cx.capture_actions(|cx| band.borrow_mut::<FabValueInput>().unwrap().begin_edit(cx));
        assert!(editing(), "the editor did not open");
        cx.capture_actions(|cx| band.borrow_mut::<FabValueInput>().unwrap().end_edit(cx));
        assert_eq!(value(&band), stood, "Escape did not put back what was there");

        // And a press on the track drags and opens nothing.
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.25);
        let down = press(at, 1.0);
        let actions = send(&mut cx, &root, &down);
        assert!(changed(&actions, &band).is_some(), "a press on the track landed nothing");
        assert!(tracking(), "a press on the track did not start the drag");
        assert!(!editing(), "a press on the track opened the box");
        send(&mut cx, &root, &release(at, 1.1));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(!editing(), "the release of a track drag opened the box");
    }

    /// The pointer over a track says what a track is for, and keeps saying
    /// it for the whole pull. The horizontal arrows were what he was seeing
    /// by accident, over the splitter that was crossing the popover.
    #[test]
    fn a_track_points_with_the_horizontal_arrows_and_holds_them() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        let face = face(&cx, &band);
        let on_track = along(&cx, &band, 0.5);
        assert_eq!(
            band.borrow::<FabValueInput>().unwrap().cursor_at(on_track.x, face),
            MouseCursor::EwResize,
            "hovering a track did not show the horizontal arrows"
        );

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(on_track, 0.0);
        send(&mut cx, &root, &down);
        let to = on_track + dvec2(20.0, 0.0);
        send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(
            cx.mouse_cursor(),
            MouseCursor::EwResize,
            "the arrows went during the drag"
        );
        send(&mut cx, &root, &release(to, 0.2));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }
}

#[cfg(test)]
mod fab_color_pick_shield {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    /// A window's pass with an overlay in it, which is what the popover
    /// draws into.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        })
    }

    fn release(abs: Vec2d) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 1.1,
        })
    }

    fn moved(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 1.2,
        })
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    /// Who claimed a pointer event on its way through the page.
    fn claimed(event: &Event) -> Area {
        match event {
            Event::MouseDown(e) => e.handled.get(),
            Event::MouseMove(e) => e.handled.get(),
            _ => Area::Empty,
        }
    }

    /// A press and its release, and the actions of both. With no event loop
    /// here to end a capture on release, the area the press captured is let
    /// go by hand afterwards, or it would take every later press.
    fn click(cx: &mut Cx, root: &WidgetRef, at: Vec2d) -> (ActionsBuf, Area) {
        let down = press(at);
        let mut actions = send(cx, root, &down);
        let taken = claimed(&down);
        actions.extend(send(cx, root, &release(at)));
        down.unhandle(cx, &taken);
        (actions, taken)
    }

    fn pressed(actions: &ActionsBuf, button: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .any(|action| {
                action.widget_uid == button.widget_uid()
                    && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_))
            })
    }

    fn middle(cx: &Cx, area: Area) -> Vec2d {
        let rect = area.rect(cx);
        assert!(rect.size.x > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    /// Default event order, the last child first: the button under the
    /// popover is walked BEFORE the picker whose popover lies over it, which
    /// is the order a page is in beside the panel walked after it.
    fn start(cx: &mut Cx) -> (WidgetRef, Target) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    pick := FabColorPick{width: 300. height: 20.}
                    under := Button{width: 300. height: 200. text: "under"}
                    beside := Button{width: 100. height: 40. margin: Inset{left: 500.} text: "beside"}
                    other := FabColorPick{width: 60. height: 20. margin: Inset{left: 600.}}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        (root, target)
    }

    fn is_open(pick: &WidgetRef) -> bool {
        pick.borrow::<FabColorPick>().expect("a colour picker").is_open()
    }

    /// Open by a press on the swatch, and drawn, as a person opens it.
    fn open_by_hand(cx: &mut Cx, root: &WidgetRef, target: &mut Target, pick: &WidgetRef) {
        let at = middle(cx, pick.area());
        click(cx, root, at);
        assert!(is_open(pick), "a press on the swatch did not open the popover");
        target.draw(cx, root);
    }

    /// Nothing under the open popover hears the pointer, and the popover
    /// does.
    ///
    /// The popover is drawn over the page but it is not in the page's way:
    /// a widget walked before the picker saw a press or a hover through the
    /// popover as its own, took it, and marked it handled, so the wheel that
    /// was drawn on top of it never heard of it. That was the operator's
    /// finding: a button under the picker still took hover and clicks, and
    /// a drag on the wheel that happened to start over one never moved the
    /// puck.
    #[test]
    fn a_pointer_on_the_popover_reaches_nothing_under_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        let under = root.widget(&cx, ids!(under));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        let at = middle(&cx, under.area());
        let panel = pick.borrow::<FabColorPick>().unwrap().popover_rect();
        assert!(panel.contains(at), "the button is not under the popover, so this tests nothing");

        let hover = moved(at);
        send(&mut cx, &root, &hover);
        assert_ne!(claimed(&hover), under.area(), "the button under the popover took the hover");

        let (actions, taken) = click(&mut cx, &root, at);
        assert!(!pressed(&actions, &under), "the button under the popover heard the press");
        assert_ne!(taken, under.area(), "the button under the popover took the press");
        assert!(
            pick.as_fab_color_pick().changed(&actions).is_some(),
            "the wheel under the pointer never heard the press"
        );
        assert!(is_open(&pick), "a press on the popover shut it");
    }

    /// A press outside the popover shuts it and goes no further, and the
    /// pointer is the page's again afterwards.
    ///
    /// What was walked before the picker saw the lock and nothing else; what
    /// is walked after would find the lock released by then, and take the
    /// press that shut the popover as its own, as a drop-down's list does
    /// not let it.
    #[test]
    fn a_press_outside_shuts_the_popover_and_gives_the_pointer_back() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        let beside = root.widget(&cx, ids!(beside));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        assert!(cx.sweep_lock_area().is_some(), "the open popover never took the pointer");

        let at = middle(&cx, beside.area());
        let (actions, _) = click(&mut cx, &root, at);
        assert!(!is_open(&pick), "a press outside did not shut the popover");
        assert!(!pressed(&actions, &beside), "the press that shut the popover pressed a button too");
        assert_eq!(cx.sweep_lock_area(), None, "the shut popover still holds the pointer");
        target.draw(&mut cx, &root);
        let (actions, _) = click(&mut cx, &root, at);
        assert!(pressed(&actions, &beside), "with the popover shut the button hears its press");
    }

    /// Every way the popover shuts lets go of the pointer.
    #[test]
    fn every_way_the_popover_shuts_lets_go_of_the_pointer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        let open = |cx: &mut Cx, target: &mut Target| {
            pick.borrow_mut::<FabColorPick>().unwrap().open_popover(cx);
            target.draw(cx, &root);
            assert!(cx.sweep_lock_area().is_some(), "the open popover never took the pointer");
        };
        let shut = |cx: &Cx, how: &str| {
            assert!(!is_open(&pick), "{how} did not shut the popover");
            assert_eq!(cx.sweep_lock_area(), None, "{how} left the pointer locked");
        };

        // Escape and Back shut it reverting, once the event loop has handed
        // the press to its cancel scope, which a bare `Cx` never does; the
        // revert is the call they make.
        open(&mut cx, &mut target);
        pick.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, true);
        shut(&cx, "Escape or Back");

        open(&mut cx, &mut target);
        send(&mut cx, &root, &Event::Actions(vec![Box::new(crate::modal::ModalAction::Dismissed)]));
        shut(&cx, "a modal's dismissal");

        open(&mut cx, &mut target);
        pick.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);
        shut(&cx, "the host");

        open(&mut cx, &mut target);
        let eyedropper = pick.borrow::<FabColorPick>().unwrap().popover.child(live_id!(hex_row)).child(live_id!(pick)).area();
        let at = middle(&cx, eyedropper);
        let (actions, _) = click(&mut cx, &root, at);
        assert!(
            actions
                .iter()
                .filter_map(|a| a.as_widget_action())
                .any(|a| a.widget_uid == pick.widget_uid()
                    && matches!(a.cast::<FabColorPickAction>(), FabColorPickAction::Eyedropper)),
            "the pick button never armed the eyedropper"
        );
        shut(&cx, "the eyedropper");

        pick.borrow_mut::<FabColorPick>()
            .unwrap()
            .set_palette(&mut cx, vec![("accent".to_string(), [0.9, 0.2, 0.1, 1.0])]);
        open(&mut cx, &mut target);
        let cell = {
            let inner = pick.borrow::<FabColorPick>().unwrap();
            let strip = inner.popover.child(live_id!(palette));
            let strip = strip.borrow::<FabPaletteStrip>().unwrap();
            let rect = strip.area.rect(&cx);
            assert!(rect.size.x > 0.0, "the palette strip never drew");
            rect.pos + dvec2(strip.cell_size * 0.5, strip.cell_size * 0.5)
        };
        let (actions, _) = click(&mut cx, &root, cell);
        assert!(
            actions
                .iter()
                .filter_map(|a| a.as_widget_action())
                .any(|a| a.widget_uid == pick.widget_uid()
                    && matches!(a.cast::<FabColorPickAction>(), FabColorPickAction::PalettePick(_))),
            "the palette cell was never picked"
        );
        shut(&cx, "a pick from the palette strip");
    }

    /// Two pickers on one page never both hold the pointer: a press on the
    /// second one's swatch while the first is up shuts the first and opens
    /// nothing, and the second, opened by the next press, holds it alone.
    #[test]
    fn two_pickers_never_both_hold_the_pointer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        let other = root.widget(&cx, ids!(other));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        let at = middle(&cx, other.area());
        click(&mut cx, &root, at);
        assert!(!is_open(&pick), "a press on the other swatch left the first popover up");
        assert!(!is_open(&other), "the press that shut one popover opened another");
        assert_eq!(cx.sweep_lock_area(), None);
        target.draw(&mut cx, &root);

        open_by_hand(&mut cx, &root, &mut target, &other);
        assert!(cx.sweep_lock_area().is_some(), "the second popover never took the pointer");
        other.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);
        assert_eq!(cx.sweep_lock_area(), None, "the first popover's lock outlived it");
    }

    // ---- one colour, four views of it ----

    fn row(pick: &WidgetRef, id: LiveId) -> WidgetRef {
        pick.borrow::<FabColorPick>().unwrap().popover.child(id)
    }

    fn row_value(pick: &WidgetRef, id: LiveId) -> f64 {
        row(pick, id)
            .borrow::<FabValueInput>()
            .expect("the popover has that row")
            .value()
    }

    /// A window point `t` (0..1) along a row's fill — the span the row
    /// itself paints the fill across and reads a press against.
    fn along(cx: &Cx, pick: &WidgetRef, id: LiveId, t: f64) -> Vec2d {
        let r = row(pick, id);
        let inner = r.borrow::<FabValueInput>().expect("the popover has that row");
        let f = inner.draw_bg.area().rect(cx);
        assert!(f.size.x > 0.0, "the row was drawn");
        let (lo, hi) = inner.track_span(f.size.x);
        dvec2(f.pos.x + lo + t * (hi - lo), f.pos.y + f.size.y * 0.5)
    }

    fn wheel_hsv(pick: &WidgetRef) -> [f32; 3] {
        row(pick, live_id!(wheel))
            .borrow::<FabColorWheel>()
            .expect("the popover has a wheel")
            .hsv()
    }

    fn hex_text(pick: &WidgetRef) -> String {
        pick.borrow::<FabColorPick>()
            .unwrap()
            .popover
            .child(live_id!(hex_row))
            .child(live_id!(hex))
            .text()
    }

    /// One press on a row, and its release, with the capture let go by hand.
    fn drag_row(cx: &mut Cx, root: &WidgetRef, pick: &WidgetRef, id: LiveId, t: f64) -> ActionsBuf {
        let at = along(cx, pick, id, t);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let mut actions = send(cx, root, &down);
        actions.extend(send(cx, root, &release(at)));
        down.unhandle(cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        actions
    }

    fn opened_on(cx: &mut Cx, root: &WidgetRef, target: &mut Target, pick: &WidgetRef, rgba: [f32; 4]) {
        pick.borrow_mut::<FabColorPick>().unwrap().set_rgba(cx, rgba);
        open_by_hand(cx, root, target, pick);
    }

    /// A move on the H row reaches the bytes, the hex and the wheel, and
    /// does it on the same frame.
    #[test]
    fn moving_the_hue_row_carries_the_bytes_the_hex_and_the_wheel_with_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        opened_on(&mut cx, &root, &mut target, &pick, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(row_value(&pick, live_id!(num_h)), 0.0);
        // A third of the way along 0..360 is green.
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 1.0 / 3.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 120.0);
        assert_eq!(row_value(&pick, live_id!(num_r)), 0.0, "R never heard the hue move");
        assert_eq!(row_value(&pick, live_id!(num_g)), 255.0);
        assert_eq!(row_value(&pick, live_id!(num_b)), 0.0);
        assert_eq!(hex_text(&pick), "#00ff00ff", "the hex field never heard it");
        assert!((wheel_hsv(&pick)[0] - 1.0 / 3.0).abs() < 1e-4, "the wheel never heard it");
    }

    /// And the other way: a byte reaches H S V, the hex and the wheel.
    #[test]
    fn moving_a_byte_row_carries_the_hsv_rows_the_hex_and_the_wheel_with_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        opened_on(&mut cx, &root, &mut target, &pick, [1.0, 0.0, 0.0, 1.0]);
        // Half way along G: 128 of 255, which is orange at 30 degrees.
        drag_row(&mut cx, &root, &pick, live_id!(num_g), 0.5);
        assert_eq!(row_value(&pick, live_id!(num_g)), 128.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 30.0, "H never heard the byte move");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 100.0);
        assert_eq!(hex_text(&pick), "#ff8000ff", "the hex field never heard it");
        let hue = wheel_hsv(&pick)[0];
        assert!((hue - 30.0 / 360.0).abs() < 1e-3, "the wheel never heard it: {hue}");
    }

    /// Saturation down to nothing and back up again keeps the hue.
    ///
    /// The picker holds HSV and derives the rest; a grey has no hue to read
    /// back out of RGB, so a state kept as bytes would answer 0 — red — the
    /// moment S touched the floor, and the colour would come back red.
    #[test]
    fn saturation_to_zero_and_back_keeps_the_hue() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        opened_on(&mut cx, &root, &mut target, &pick, [1.0, 0.0, 0.0, 1.0]);
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 200.0 / 360.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0);
        drag_row(&mut cx, &root, &pick, live_id!(num_s), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_s)), 0.0);
        assert_eq!(
            row_value(&pick, live_id!(num_h)),
            200.0,
            "the hue jumped when the colour went grey"
        );
        drag_row(&mut cx, &root, &pick, live_id!(num_s), 1.0);
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "the hue did not come back");
    }

    /// And value down to black and back up again keeps both.
    #[test]
    fn value_to_zero_and_back_keeps_the_hue_and_the_saturation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        opened_on(&mut cx, &root, &mut target, &pick, [1.0, 0.0, 0.0, 1.0]);
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 200.0 / 360.0);
        drag_row(&mut cx, &root, &pick, live_id!(num_v), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "black lost the hue");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0, "black lost the saturation");
        drag_row(&mut cx, &root, &pick, live_id!(num_v), 1.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "the hue did not come back");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0, "the saturation did not come back");
    }

    /// A colour that went in as a hex comes back out of the hex field
    /// unchanged, however far round the HSV state it travelled.
    #[test]
    fn a_hex_comes_back_out_of_the_field_unchanged() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        for text in [
            "#3a7bd5ff",
            "#ff8000ff",
            "#808080ff",
            "#000000ff",
            "#ffffffff",
            "#01020380",
        ] {
            let (rgba, _) = parse_hex(text).expect("a hex this test wrote");
            pick.borrow_mut::<FabColorPick>().unwrap().set_rgba(&mut cx, rgba);
            assert_eq!(hex_text(&pick), text, "the colour did not survive the round trip");
        }
    }

    /// One move, one change: the popover says its colour once however many
    /// views of it followed along.
    #[test]
    fn the_popover_says_its_colour_once_per_move() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        opened_on(&mut cx, &root, &mut target, &pick, [1.0, 0.0, 0.0, 1.0]);
        let at = along(&cx, &pick, live_id!(num_h), 0.25);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let actions = send(&mut cx, &root, &down);
        let count = |actions: &ActionsBuf| {
            actions
                .filter_widget_actions_cast::<FabColorPickAction>(pick.widget_uid())
                .filter(|a| matches!(a, FabColorPickAction::Changed(_)))
                .count()
        };
        assert_eq!(count(&actions), 1, "the press said its colour more than once");
        let to = along(&cx, &pick, live_id!(num_h), 0.5);
        let actions = send(&mut cx, &root, &moved(to));
        assert_eq!(count(&actions), 1, "the move said its colour more than once");
        send(&mut cx, &root, &release(to));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// Every row in the popover is a track, and the popover still fits: the
    /// seven channels, the hex line and the strip inside one panel that is
    /// on screen.
    #[test]
    fn the_seven_rows_are_all_tracks_and_the_popover_still_fits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        for id in [
            live_id!(num_r),
            live_id!(num_g),
            live_id!(num_b),
            live_id!(num_a),
            live_id!(num_h),
            live_id!(num_s),
            live_id!(num_v),
        ] {
            let r = row(&pick, id);
            let inner = r.borrow::<FabValueInput>().expect("the popover has that row");
            assert!(inner.track, "a row of the popover is not a track");
            assert!(inner.show_fill, "a row of the popover shows no fill");
        }
        let panel = pick.borrow::<FabColorPick>().unwrap().popover_rect();
        assert!(panel.size.x > 0.0 && panel.size.y > 0.0, "the popover never drew");
        assert!(panel.pos.y >= 0.0, "the popover hangs off the top of the window");
        assert!(
            panel.pos.y + panel.size.y <= SIZE.y,
            "the popover is {} tall and runs off the bottom of a {} window",
            panel.size.y,
            SIZE.y
        );
    }

    // ---- carrying a colour off a draggable swatch ----

    fn carrier(cx: &mut Cx) -> (WidgetRef, Target) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    pick := FabColorPick{width: 60. height: 20. draggable: true}
                    plain := FabColorPick{width: 60. height: 20. margin: Inset{top: 40.}}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        (root, target)
    }

    fn carry_said(actions: &ActionsBuf, pick: &WidgetRef) -> Vec<&'static str> {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == pick.widget_uid())
            .filter_map(|action| match action.cast::<FabColorPickAction>() {
                FabColorPickAction::DragStarted => Some("started"),
                FabColorPickAction::DragMoved(_) => Some("moved"),
                FabColorPickAction::DragDropped(_) => Some("dropped"),
                FabColorPickAction::DragCancelled => Some("cancelled"),
                FabColorPickAction::Opened => Some("opened"),
                _ => None,
            })
            .collect()
    }

    fn escape() -> Event {
        Event::KeyDown(KeyEvent {
            key_code: KeyCode::Escape,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 1.15,
        })
    }

    /// A draggable swatch opens on the release, because only the release
    /// says the press did not travel; one that carried opens nothing and
    /// reports the carry from start to drop.
    #[test]
    fn a_draggable_swatch_opens_on_the_release_and_carries_past_the_slop() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = carrier(&mut cx);
        let pick = root.child(live_id!(pick));
        let at = middle(&cx, pick.area());
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));

        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        assert!(!is_open(&pick), "a draggable swatch opened on the press, before it could know");
        all.extend(send(&mut cx, &root, &release(at)));
        down.unhandle(&mut cx, &claimed(&down));
        assert!(is_open(&pick), "a press and release on a draggable swatch did not open it");
        assert_eq!(carry_said(&all, &pick), vec!["opened"]);
        pick.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);

        // A wobble under the slop is still a click.
        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        all.extend(send(&mut cx, &root, &moved(at + dvec2(KNOB_DRAG_SLOP - 1.0, 1.0))));
        all.extend(send(&mut cx, &root, &release(at + dvec2(KNOB_DRAG_SLOP - 1.0, 1.0))));
        down.unhandle(&mut cx, &claimed(&down));
        assert!(is_open(&pick), "a wobble under the slop carried instead of opening");
        assert_eq!(carry_said(&all, &pick), vec!["opened"]);
        pick.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);

        // Past the slop: a carry, and no popover.
        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        all.extend(send(&mut cx, &root, &moved(at + dvec2(20.0, 0.0))));
        assert!(pick.borrow::<FabColorPick>().unwrap().is_carrying(), "a press that travelled is not carrying");
        all.extend(send(&mut cx, &root, &moved(at + dvec2(80.0, 0.0))));
        all.extend(send(&mut cx, &root, &release(at + dvec2(80.0, 0.0))));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(!is_open(&pick), "a carried colour opened its popover on the drop");
        assert!(!pick.borrow::<FabColorPick>().unwrap().is_carrying(), "the drop left the colour carried");
        assert_eq!(carry_said(&all, &pick), vec!["started", "moved", "moved", "dropped"]);
    }

    /// Escape calls a carry off, and the release after it drops nothing and
    /// opens nothing.
    #[test]
    fn escape_calls_a_carry_off_and_the_release_after_it_says_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = carrier(&mut cx);
        let pick = root.child(live_id!(pick));
        let at = middle(&cx, pick.area());
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        all.extend(send(&mut cx, &root, &moved(at + dvec2(30.0, 0.0))));
        all.extend(send(&mut cx, &root, &escape()));
        all.extend(send(&mut cx, &root, &moved(at + dvec2(60.0, 0.0))));
        all.extend(send(&mut cx, &root, &release(at + dvec2(60.0, 0.0))));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert_eq!(carry_said(&all, &pick), vec!["started", "moved", "cancelled"]);
        assert!(!is_open(&pick), "the release after Escape opened the popover");
    }

    /// A carry that starts on the swatch of an open popover shuts it first,
    /// and lets go of the pointer the popover held.
    #[test]
    fn a_carry_from_an_open_popover_shuts_it_and_leaves_no_lock() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = carrier(&mut cx);
        let pick = root.child(live_id!(pick));
        pick.borrow_mut::<FabColorPick>().unwrap().open_popover(&mut cx);
        target.draw(&mut cx, &root);
        assert!(cx.sweep_lock_area().is_some(), "the open popover never took the pointer");
        let at = middle(&cx, pick.area());
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        all.extend(send(&mut cx, &root, &moved(at + dvec2(30.0, 0.0))));
        assert!(!is_open(&pick), "the popover is still up under a carry");
        assert_eq!(cx.sweep_lock_area(), None, "the carry left the popover's lock held");
        target.draw(&mut cx, &root);
        // Off the swatch, where every carry goes: the press that shut the
        // popover had to name its lock to be heard, and a capture left
        // naming it lets go of the pointer at the swatch's edge.
        all.extend(send(&mut cx, &root, &moved(at + dvec2(120.0, 0.0))));
        assert!(pick.borrow::<FabColorPick>().unwrap().is_carrying(), "the carry dropped at the swatch's edge");
        all.extend(send(&mut cx, &root, &release(at + dvec2(120.0, 0.0))));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(!is_open(&pick), "the drop opened the popover again");
        assert_eq!(cx.sweep_lock_area(), None);
        assert_eq!(carry_said(&all, &pick), vec!["started", "moved", "moved", "dropped"]);
    }

    /// A picker that did not ask to carry keeps the press it always had: it
    /// opens on the way down, and a press that travels carries nothing.
    #[test]
    fn a_picker_that_did_not_ask_to_carry_never_does() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = carrier(&mut cx);
        let plain = root.child(live_id!(plain));
        assert!(!plain.borrow::<FabColorPick>().unwrap().draggable, "the type default carries");
        let at = middle(&cx, plain.area());
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let mut all = send(&mut cx, &root, &down);
        assert!(is_open(&plain), "a plain picker stopped opening on the press");
        all.extend(send(&mut cx, &root, &moved(at + dvec2(40.0, 0.0))));
        all.extend(send(&mut cx, &root, &release(at + dvec2(40.0, 0.0))));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert_eq!(carry_said(&all, &plain), vec!["opened"], "a plain picker carried");
        plain.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);
    }

    /// The panel's splitter, standing in for every gesture that reads raw
    /// events: what it asks the platform while a popover control is held.
    ///
    /// The bar is drawn across the whole window height at the panel's left
    /// edge, and the popover hangs LEFT of its swatch, so the two cross. The
    /// bar used to take its press off `Event::MouseDown` by x-coordinate
    /// alone: one press armed both, and every move after it resized the
    /// sidebar under the hand — dragging the popover sideways out from under
    /// the very row being pulled. Asked through `hits`, as it is now, it
    /// hears nothing at all while the popover holds the pointer, which is
    /// what these check.
    fn splitter_asks(cx: &mut Cx, event: &Event, bar: Area) -> Hit {
        event.hits_with_options(
            cx,
            bar,
            HitOptions::new()
                .with_margin(Inset { left: 3.0, right: 3.0, top: 0.0, bottom: 0.0 })
                .with_capture_overload(true),
        )
    }

    /// A point inside the open popover, `t` of the way across its width, on
    /// the row `rows` rows up from its foot.
    fn in_popover(cx: &Cx, pick: &WidgetRef, id: LiveId) -> Vec2d {
        let child = pick.borrow::<FabColorPick>().unwrap().popover.child(id);
        let rect = child.area().rect(cx);
        assert!(rect.size.x > 0.0, "{id:?} was never drawn in the popover");
        rect.pos + rect.size * 0.5
    }

    /// Held on a channel row, the bar across the popover is deaf — it takes
    /// neither the press, nor a hover, nor any of the moves that follow.
    #[test]
    fn a_held_channel_row_leaves_the_splitter_nothing_to_hear() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, mut target) = start(&mut cx);
        let pick = root.widget(&cx, ids!(pick));
        let under = root.widget(&cx, ids!(under));
        open_by_hand(&mut cx, &root, &mut target, &pick);
        let bar = under.area();
        let at = in_popover(&cx, &pick, live_id!(num_r));
        assert!(
            pick.borrow::<FabColorPick>().unwrap().popover_rect().contains(at),
            "the row is not inside the popover, so this tests nothing"
        );

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        send(&mut cx, &root, &down);
        assert!(
            matches!(splitter_asks(&mut cx, &down, bar), Hit::Nothing),
            "the splitter took the press that belongs to the channel row"
        );
        for step in 1..4 {
            let to = at + dvec2(step as f64 * 12.0, 0.0);
            let move_event = moved(to);
            send(&mut cx, &root, &move_event);
            assert!(
                matches!(splitter_asks(&mut cx, &move_event, bar), Hit::Nothing),
                "the splitter took a move of the row's drag"
            );
        }
        send(&mut cx, &root, &release(at + dvec2(36.0, 0.0)));
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// And the same for the other three things a press in the popover can
    /// land on. The rule is the popover's, not any one control's, so all of
    /// them are covered by the one lock — which is the point of fixing this
    /// where ownership is decided rather than teaching the bar about rows.
    #[test]
    fn the_wheel_the_square_and_the_strip_hold_the_pointer_too() {
        for which in ["ring", "square", "strip"] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let (root, mut target) = start(&mut cx);
            let pick = root.widget(&cx, ids!(pick));
            let under = root.widget(&cx, ids!(under));
            pick.borrow_mut::<FabColorPick>().unwrap().set_palette(
                &mut cx,
                vec![
                    ("one".to_string(), [1.0, 0.0, 0.0, 1.0]),
                    ("two".to_string(), [0.0, 1.0, 0.0, 1.0]),
                ],
            );
            open_by_hand(&mut cx, &root, &mut target, &pick);
            target.draw(&mut cx, &root);
            let bar = under.area();
            let wheel = in_popover(&cx, &pick, live_id!(wheel));
            let size = {
                let child = pick.borrow::<FabColorPick>().unwrap().popover.child(live_id!(wheel));
                child.area().rect(&cx).size.x
            };
            let at = match which {
                // Straight up from the middle is the hue ring; the middle
                // itself is the saturation square.
                "ring" => wheel - dvec2(0.0, size * (RING_OUTER + RING_INNER) * 0.5),
                "square" => wheel,
                _ => in_popover(&cx, &pick, live_id!(palette)),
            };
            cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
            let down = press(at);
            send(&mut cx, &root, &down);
            assert!(
                matches!(splitter_asks(&mut cx, &down, bar), Hit::Nothing),
                "the splitter took the press meant for the {which}"
            );
            let to = at + dvec2(18.0, 6.0);
            let move_event = moved(to);
            send(&mut cx, &root, &move_event);
            assert!(
                matches!(splitter_asks(&mut cx, &move_event, bar), Hit::Nothing),
                "the splitter took a move of the {which}'s drag"
            );
            send(&mut cx, &root, &release(to));
            down.unhandle(&mut cx, &claimed(&down));
            cx.fingers.first_mouse_button = None;
        }
    }

    /// With the popover shut the bar is the bar again: it takes a press on
    /// itself, so the fix bought the rule and not a dead splitter.
    #[test]
    fn a_shut_popover_gives_the_splitter_its_press_back() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _target) = start(&mut cx);
        let under = root.widget(&cx, ids!(under));
        let bar = under.area();
        let at = middle(&cx, bar);
        assert_eq!(cx.sweep_lock_area(), None, "nothing should hold the pointer here");
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        assert!(
            matches!(splitter_asks(&mut cx, &down, bar), Hit::FingerDown(_)),
            "the splitter cannot be grabbed at all any more"
        );
        let up = release(at);
        assert!(
            matches!(splitter_asks(&mut cx, &up, bar), Hit::FingerUp(_)),
            "the splitter never heard its own release"
        );
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// The A row can be left out, and then there is no alpha anywhere: not
    /// in the rows, not in what the picker reports, not in the hex. A colour
    /// that arrives carrying one is made opaque rather than quietly keeping
    /// it where nothing can put it right — his square was reading #92755014,
    /// an alpha of 20 that nothing used and nobody asked for.
    ///
    /// Two scenes rather than two pickers in one, because the keyboard has
    /// one focus and the typed hex needs it.
    #[test]
    fn a_picker_without_alpha_holds_its_colour_opaque() {
        let mut columns = Vec::new();
        for with_alpha in [false, true] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            cx.with_vm(crate::script_mod);
            let root = cx.with_vm(|vm| {
                let value = if with_alpha {
                    crate::script_eval!(vm, {
                        use mod.prelude.widgets.*
                        use mod.widgets.*
                        View{ width: Fill height: Fill flow: Down
                            pick := FabColorPick{width: 300. height: 20.}
                        }
                    })
                } else {
                    crate::script_eval!(vm, {
                        use mod.prelude.widgets.*
                        use mod.widgets.*
                        View{ width: Fill height: Fill flow: Down
                            pick := FabColorPick{width: 300. height: 20. with_alpha: false}
                        }
                    })
                };
                WidgetRef::script_from_value(vm, value)
            });
            let mut target = Target::new(&mut cx);
            target.draw(&mut cx, &root);
            let pick = root.widget(&cx, ids!(pick));
            let rgba = || pick.borrow::<FabColorPick>().unwrap().rgba();

            // A colour with an alpha of 20/255, the way one arrives from a
            // drag or the eyedropper.
            let carried = [0.573_f32, 0.459, 0.314, 20.0 / 255.0];
            pick.borrow_mut::<FabColorPick>().unwrap().set_rgba(&mut cx, carried);
            let expected = if with_alpha { 20.0 / 255.0 } else { 1.0 };
            assert!(
                (rgba()[3] - expected).abs() < 1e-6,
                "with_alpha={with_alpha}: the picker reported alpha {}",
                rgba()[3]
            );
            assert_eq!(
                pick.borrow::<FabColorPick>().unwrap().drawn_swatch().w,
                expected,
                "with_alpha={with_alpha}: the square is drawn with the wrong alpha"
            );

            pick.borrow_mut::<FabColorPick>().unwrap().open_popover(&mut cx);
            target.draw(&mut cx, &root);
            let hex = pick
                .borrow::<FabColorPick>()
                .unwrap()
                .popover
                .child(live_id!(hex_row))
                .child(live_id!(hex));
            let figures = if with_alpha { 9 } else { 7 };
            assert_eq!(
                hex.text().len(),
                figures,
                "with_alpha={with_alpha}: the hex reads {}",
                hex.text()
            );
            assert_eq!(
                pick.borrow::<FabColorPick>().unwrap().popover.child(live_id!(num_a)).visible(),
                with_alpha,
                "with_alpha={with_alpha}: the A row is drawn the wrong way round"
            );
            // And the column does not move when the row goes: the boxes are
            // measured from the font and the figures, not from the stack.
            let column = pick
                .borrow::<FabColorPick>()
                .unwrap()
                .popover
                .child(live_id!(num_r))
                .borrow::<FabValueInput>()
                .unwrap()
                .track_columns
                .1;
            assert!(column > 2.0, "the R row's box was never measured");
            columns.push(column);

            // And an eight-figure hex typed in: the colour is taken, the
            // transparency only where there is a row to put it back with.
            hex.set_text(&mut cx, "#11223344");
            cx.set_key_focus(hex.area());
            cx.handle_actions();
            assert!(cx.has_key_focus(hex.area()), "the hex field never took the keyboard");
            let returned = Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                is_repeat: false,
                modifiers: KeyModifiers::default(),
                time: 2.0,
            });
            send(&mut cx, &root, &returned);
            assert!(
                (rgba()[0] - 0x11 as f32 / 255.0).abs() < 0.01,
                "with_alpha={with_alpha}: the typed hex was not taken"
            );
            let expected = if with_alpha { 0x44 as f32 / 255.0 } else { 1.0 };
            assert!(
                (rgba()[3] - expected).abs() < 1e-6,
                "with_alpha={with_alpha}: a typed eight-figure hex left alpha {}",
                rgba()[3]
            );
            pick.borrow_mut::<FabColorPick>().unwrap().close_popover(&mut cx, false);
        }
        assert!(
            (columns[0] - columns[1]).abs() < 0.01,
            "the readout column moved when the A row went: {columns:?}"
        );
    }
}

#[cfg(test)]
mod fab_carousel_motion {
    //! The carousel's motion, driven the way the platform drives it: real
    //! presses and real `NextFrame` events, so that what these say about the
    //! glide is what the panel does with it.
    use super::fab_slider_gestures::{moved, press, release, send, Target};
    use super::*;
    use std::cell::Cell;

    const WINDOW: WindowId = WindowId(1, 1);
    /// The window the row is seen through, and the row that is too long for
    /// it: forty chips at a pitch of 25 is a thousand points of palettes.
    const VIEW: f64 = 260.0;
    const COUNT: usize = 40;
    const PITCH: f64 = 25.0;
    /// A display's frame, which is what the tests step by.
    const FRAME: f64 = 1.0 / 60.0;

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    row := FabPaletteCarousel{
                        width: 260.
                        height: 88.
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The scene drawn, with forty palettes in the row and nothing chosen,
    /// so that nothing is owed a place in the window before a test starts.
    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, Target) {
        // No `init_cx_os`: this draws into a pass of its own, the way the
        // panel's own tests do, and a graphics device per test is both
        // unnecessary and a lot to ask of a machine running the suite
        // several tests at a time.
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let row = root.widget(cx, ids!(row));
        assert!(!row.is_empty(), "the scene has a carousel in it");
        let chips: Vec<[Vec4f; 4]> = (0..COUNT)
            .map(|index| {
                let shade = index as f32 / COUNT as f32;
                [vec4(shade, 0.3, 0.6, 1.0); 4]
            })
            .collect();
        row.borrow_mut::<FabPaletteCarousel>().expect("a carousel").set_chips(cx, &chips, None);
        target.draw(cx, &root);
        let seen = row.area().rect(cx);
        assert!((seen.size.x - VIEW).abs() < 0.5, "the window is {} wide", seen.size.x);
        (root, row, target)
    }

    fn scroll(row: &WidgetRef) -> f64 {
        row.borrow::<FabPaletteCarousel>().expect("a carousel").scroll()
    }

    fn drawn(row: &WidgetRef) -> f64 {
        row.borrow::<FabPaletteCarousel>().expect("a carousel").drawn_scroll()
    }

    fn gliding(row: &WidgetRef) -> bool {
        row.borrow::<FabPaletteCarousel>().expect("a carousel").is_gliding()
    }

    fn far_end() -> f64 {
        ChipTrack { count: COUNT, chip_width: 22.0, gap: 3.0, view: VIEW }.max_scroll()
    }

    /// One animation frame, as the platform sends it: the row's own
    /// next-frame id in the set, the way the animator's own tests do it.
    fn frame(cx: &mut Cx, root: &WidgetRef, row: &WidgetRef, at: u64, time: f64) {
        let next = row.borrow::<FabPaletteCarousel>().expect("a carousel").next_frame;
        let event = Event::NextFrame(NextFrameEvent {
            frame: at,
            time,
            set: [next].into_iter().collect(),
        });
        root.handle_event(cx, &event, &mut Scope::empty());
    }

    /// Frames until the row is at rest, at sixty a second from `from`. The
    /// positions it was drawn at on the way, so a test can say the row went
    /// one way and stayed inside its ends.
    fn settle(cx: &mut Cx, root: &WidgetRef, row: &WidgetRef, from: f64) -> Vec<f64> {
        let mut seen = Vec::new();
        let mut time = from;
        while gliding(row) {
            time += FRAME;
            frame(cx, root, row, seen.len() as u64 + 1, time);
            seen.push(drawn(row));
            assert!(seen.len() < 600, "the row never came to rest");
        }
        seen
    }

    fn a_press_on_the_arrow(cx: &mut Cx, row: &WidgetRef, forward: bool) {
        row.borrow_mut::<FabPaletteCarousel>().expect("a carousel").scroll_page(cx, forward);
    }

    /// An arrow does NOT land the row a page along in the frame it was
    /// pressed: the row's place is a page along, and the picture takes the
    /// frames after it to get there.
    #[test]
    fn an_arrow_glides_the_row_rather_than_stepping_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        assert_eq!(scroll(&row), 0.0);
        a_press_on_the_arrow(&mut cx, &row, true);
        let page = scroll(&row);
        assert!(page >= 10.0 * PITCH, "a page of the window is {page}");
        assert_eq!(drawn(&row), 0.0, "the row jumped the whole page in the frame the arrow was pressed");
        assert!(gliding(&row), "the arrow left nothing to draw");
        // The arrows read the row's place, not the picture: one that went
        // out halfway through its own glide and came back on at the end of
        // it would flicker on every press.
        assert!(!row.borrow::<FabPaletteCarousel>().expect("a carousel").at_start());

        frame(&mut cx, &root, &row, 1, FRAME);
        let first = drawn(&row);
        assert!(first > 0.0 && first < page, "one frame carried the row {first} of {page}");

        let seen = settle(&mut cx, &root, &row, FRAME);
        assert!(seen.len() > 6, "the row arrived in {} frames, which is a jump", seen.len());
        assert_eq!(drawn(&row), page, "the row came to rest short of its place");
        assert!(!gliding(&row));
        // At rest it asks for nothing more, and a stray frame moves nothing.
        frame(&mut cx, &root, &row, 999, 30.0);
        assert_eq!(drawn(&row), page);
        assert!(!gliding(&row));
    }

    /// A second press while the first is still running: the row's place is
    /// two pages along, the picture carries on from where it had got to,
    /// and it never goes backwards on its way there.
    #[test]
    fn a_second_arrow_press_retargets_without_a_jump() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        a_press_on_the_arrow(&mut cx, &row, true);
        let page = scroll(&row);
        for at in 1..5 {
            frame(&mut cx, &root, &row, at, at as f64 * FRAME);
        }
        let midway = drawn(&row);
        assert!(midway > 0.0 && midway < page, "{midway}");

        a_press_on_the_arrow(&mut cx, &row, true);
        assert_eq!(scroll(&row), page * 2.0, "two presses are not two pages");
        assert_eq!(drawn(&row), midway, "the second press jumped the picture");

        let mut was = midway;
        for at in settle(&mut cx, &root, &row, 5.0 * FRAME) {
            assert!(at >= was - 1e-9, "the row went backwards: {was} then {at}");
            was = at;
        }
        assert_eq!(drawn(&row), page * 2.0);
    }

    /// The ends hold whatever is moving the row: it comes to rest exactly
    /// on them and never passes them on the way.
    #[test]
    fn the_row_rests_exactly_at_both_of_its_ends() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        let end = far_end();
        assert!(end > 0.0);
        for _ in 0..COUNT {
            a_press_on_the_arrow(&mut cx, &row, true);
        }
        assert_eq!(scroll(&row), end, "the arrows ran the row past its last chip");
        for at in settle(&mut cx, &root, &row, 0.0) {
            assert!(at <= end + 1e-9, "the row was drawn {at} points along, past its end at {end}");
        }
        assert_eq!(drawn(&row), end, "the row came to rest off its far end");
        assert!(row.borrow::<FabPaletteCarousel>().expect("a carousel").at_end());

        for _ in 0..COUNT {
            a_press_on_the_arrow(&mut cx, &row, false);
        }
        assert_eq!(scroll(&row), 0.0);
        for at in settle(&mut cx, &root, &row, 0.0) {
            assert!(at >= -1e-9, "the row was drawn {at} points along, behind its first chip");
        }
        assert_eq!(drawn(&row), 0.0);
        assert!(!gliding(&row), "a row resting on its end is still asking for frames");
    }

    /// A drag follows the hand one point for one, and the release throws
    /// the row on: further than the hand itself came, and then to rest.
    #[test]
    fn a_flick_off_a_drag_carries_the_row_further_than_the_hand_did() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        let seen = row.area().rect(&cx);
        let from = seen.pos + dvec2(200.0, 40.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(from, 0.0));
        // Six frames of hand, twenty points each, leftwards: the row comes
        // with it, point for point.
        let mut at = from;
        for step in 1..7 {
            at = from - dvec2(20.0 * step as f64, 0.0);
            send(&mut cx, &root, &moved(at, step as f64 * FRAME));
            assert!(!gliding(&row), "the row glided under the hand holding it");
        }
        let dragged = scroll(&row);
        assert!((dragged - 120.0).abs() < 0.5, "the drag moved the row {dragged}, not with the hand");
        // Let go while it is still moving.
        send(&mut cx, &root, &release(at, 7.0 * FRAME));
        cx.fingers.first_mouse_button = None;
        let thrown = scroll(&row);
        assert!(thrown > dragged + 10.0, "the flick carried the row {thrown}, barely past the drag's {dragged}");
        assert!(gliding(&row), "the row stopped dead where the hand let go");
        assert_eq!(drawn(&row), dragged, "the flick jumped the picture on");

        let end = far_end();
        for was in settle(&mut cx, &root, &row, 7.0 * FRAME) {
            assert!(was <= end + 1e-9 && was >= -1e-9, "the flick carried the row out of its ends: {was}");
        }
        assert_eq!(drawn(&row), thrown, "the flick did not come to rest where it was going");
        assert!(!gliding(&row));
    }

    /// A drag that comes to a stop before the hand lets go leaves the row
    /// where the hand left it. Without that, every careful drag ends with
    /// the palettes sliding out from under the pointer.
    #[test]
    fn a_drag_that_ends_standing_still_does_not_drift() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        let seen = row.area().rect(&cx);
        let from = seen.pos + dvec2(200.0, 40.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(from, 0.0));
        let mut at = from;
        for step in 1..7 {
            at = from - dvec2(20.0 * step as f64, 0.0);
            send(&mut cx, &root, &moved(at, step as f64 * FRAME));
        }
        // The hand rests on the row for a third of a second and then lifts.
        send(&mut cx, &root, &release(at, 7.0 * FRAME + 0.33));
        cx.fingers.first_mouse_button = None;
        assert!((scroll(&row) - 120.0).abs() < 0.5, "the row drifted to {}", scroll(&row));
        assert!(!gliding(&row), "a drag that ended standing still threw the row");
    }

    fn a_wheel_over(cx: &mut Cx, root: &WidgetRef, at: Vec2d, delta: f64, time: f64) {
        let event = Event::Scroll(crate::event::ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, delta),
            abs: at,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time,
            phase: crate::event::ScrollPhase::Changed,
        });
        root.handle_event(cx, &event, &mut Scope::empty());
    }

    /// One notch lands whole and leaves nothing behind it; a spin of them
    /// carries a little momentum and settles, rather than stopping dead on
    /// the last notch. Either way the row is where the notches put it, to
    /// within the chip the momentum is capped at.
    #[test]
    fn a_wheel_spin_settles_and_a_single_notch_does_not() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        let seen = row.area().rect(&cx);
        let over = seen.pos + seen.size * 0.5;

        a_wheel_over(&mut cx, &root, over, 40.0, 0.0);
        assert_eq!(scroll(&row), 40.0, "the notch did not land whole");
        assert!(!gliding(&row), "one notch rolled on by itself");

        // A spin: notches a frame apart, the way a wheel turned hard
        // reports them.
        let mut time = 1.0;
        for _ in 0..6 {
            time += FRAME;
            a_wheel_over(&mut cx, &root, over, 40.0, time);
        }
        let spun = scroll(&row);
        assert!(spun > 40.0 + 6.0 * 40.0, "the spin stopped dead at {spun}");
        assert!(
            spun <= 40.0 + 6.0 * 40.0 + PITCH + 1e-9,
            "the spin carried the row {spun}, more than a chip past the last notch"
        );
        assert!(gliding(&row));
        assert!(drawn(&row) < spun, "the momentum jumped the picture on");
        settle(&mut cx, &root, &row, time);
        assert_eq!(drawn(&row), spun);
    }

    /// A hand put out to stop a moving row lands it: the picture and the
    /// place are one number again from the touch on, so the chip under the
    /// finger is the one that is drawn there -- and that press chooses
    /// nothing, because it was put out to stop the row.
    #[test]
    fn a_press_lands_a_moving_row_and_chooses_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, row, _target) = start(&mut cx);
        let seen = row.area().rect(&cx);
        let over = seen.pos + dvec2(50.0, 40.0);
        a_press_on_the_arrow(&mut cx, &row, true);
        for at in 1..4 {
            frame(&mut cx, &root, &row, at, at as f64 * FRAME);
        }
        let midway = drawn(&row);
        assert!(gliding(&row));

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, &press(over, 1.0));
        assert!(!gliding(&row), "the row went on gliding under the hand");
        assert_eq!(scroll(&row), midway, "the row landed somewhere other than where it was drawn");
        assert_eq!(drawn(&row), midway);
        let actions = send(&mut cx, &root, &release(over, 1.05));
        cx.fingers.first_mouse_button = None;
        assert!(
            !actions.iter().any(|action| matches!(
                action.as_widget_action().map(|a| a.cast::<FabPaletteCarouselAction>()),
                Some(FabPaletteCarouselAction::Pick(_))
            )),
            "the hand that stopped the row also chose a palette"
        );

        // And with the row standing still, the same press is a choice again.
        let actions = cx.capture_actions(|cx| {
            cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
            root.handle_event(cx, &press(over, 2.0), &mut Scope::empty());
            root.handle_event(cx, &release(over, 2.05), &mut Scope::empty());
            cx.fingers.first_mouse_button = None;
        });
        assert!(
            actions.iter().any(|action| matches!(
                action.as_widget_action().map(|a| a.cast::<FabPaletteCarouselAction>()),
                Some(FabPaletteCarouselAction::Pick(_))
            )),
            "a press on a row at rest no longer chooses the chip under it"
        );
    }
}
