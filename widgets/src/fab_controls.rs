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
//! * `mod.widgets.FabColorWheel` — hue ring around a saturation/value
//!   square, pointer-captured drags, arrow-key nudges.
//! * `mod.widgets.FabColorPick` — a swatch that opens a self-managed
//!   popover (wheel + RGB rows + hex entry) anchored at the swatch;
//!   outside-click commits, Escape reverts. Publishes `Changed` live and
//!   `Ended` on commit, plus `Opened`/`Closed` for hosts that need to know.
//! * `mod.widgets.FabLabel` / `FabLabelDim` / `FabLabelSmall` /
//!   `FabHeaderLabel`, `mod.widgets.FabSearch` (input well),
//!   `mod.widgets.FabPropRow` (label-left / value-right row),
//!   `mod.widgets.FabSection` (clickable section header) — the DSL shapes
//!   panels are assembled from (the tweaker's sidebar is the first tenant).

use crate::button::ButtonAction;
use crate::widget_tree::CxWidgetExt;
use crate::{
    animator::*, makepad_derive_widget::*, makepad_draw::ime::TextInputConfig, makepad_draw::*,
    text_input::*, view::View, widget::*,
};
use crate::makepad_script::script;

pub fn script_mod(vm: &mut ScriptVm) {
    // Phase 1: the token table and a prelude carrying the `fab` alias, so
    // the ported DSL below reads exactly like it does in the fab app.
    let block = script! {
        use mod.prelude.widgets_internal.*

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

            // ---- type (points) ----
            font_size_ui: 8.5
            font_size_small: 7.5
            font_size_header: 9.0

            // ---- motion ----
            anim_fast: 0.10
            anim_normal: 0.15
        }

        mod.prelude.fab_internal = {
            ..mod.prelude.widgets_internal,
            fab: mod.fab
        }
    };
    vm.eval(block);

    // Phase 2: the controls, in the fab visual language.
    let block = script! {
        use mod.prelude.fab_internal.*
        use mod.widgets.*

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

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, fab.radius)
                let reveal = mix(1.0, max(self.hover, max(self.down, self.focus)), self.flat)
                let mut base = fab.color_num.mix(fab.color_num_hover, self.hover).mix(fab.color_input_active, self.down)
                base = vec4(base.xyz, base.w * reveal)
                sdf.fill_keep(base)
                let mut border = fab.color_border.mix(fab.color_focus_ring, self.focus)
                border = vec4(border.xyz, border.w * reveal)
                sdf.stroke(border, 1.0)
                if self.fill >= 0.0 {
                    sdf.box(1.0, 1.0, max(2.0, (w - 2.0) * self.fill), h - 2.0, fab.radius)
                    sdf.fill(vec4(fab.color_num_fill.xyz, 0.85))
                }
                // Hover arrows in the end zones; they retire while the field
                // is a text editor (focus carries the editing state).
                if self.hover > 0.01 {
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

            draw_text +: {
                ink_centered: true
                color: fab.color_text_dim
                text_overflow: TextOverflow.Ellipsis
                text_style: theme.font_regular{
                    font_size: fab.font_size_ui
                }
            }
            text_input: TextInput{
                width: Fill
                height: Fill
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
                    text_style: theme.font_regular{
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
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill_keep(vec4(self.swatch.xyz, 1.0))
                let ring = fab.color_border.mix(fab.color_focus_ring, max(self.hover, self.open))
                sdf.stroke(ring, 1.0)
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
                text_style: theme.font_regular{
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
                text_style: theme.font_regular{
                    font_size: fab.font_size_small
                }
            }
        }
        mod.widgets.FabHeaderLabel = mod.widgets.FabLabel{
            draw_text +: {
                color: fab.color_text_header
                text_style: theme.font_regular{
                    font_size: fab.font_size_header
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
                }
                draw_text +: {
                    ink_centered: true
                    color: fab.color_text
                    color_empty: fab.color_text_muted
                    color_empty_hover: fab.color_text_dim
                    color_empty_focus: fab.color_text_dim
                    text_style: theme.font_regular{
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

        mod.widgets.FabColorPickBase = #(FabColorPick::register_widget(vm))
        mod.widgets.FabColorPick = set_type_default() do mod.widgets.FabColorPickBase{
            width: fab.swatch_width
            height: 16
            with_alpha: true
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
                num_r := mod.widgets.FabValueInput{ label: "R" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                num_g := mod.widgets.FabValueInput{ label: "G" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                num_b := mod.widgets.FabValueInput{ label: "B" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                num_a := mod.widgets.FabValueInput{ label: "A" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                hex_row := View{
                    width: Fill
                    height: fab.row_height
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    spacing: 6
                    mod.widgets.FabLabelDim{ width: 30 text: "Hex" }
                    pick := mod.widgets.Button{
                        width: Fit
                        height: Fill
                        padding: Inset{left: 6 right: 6 top: 2 bottom: 2}
                        text: "pick"
                    }
                    hex := TextInput{
                        width: Fill
                        height: Fill
                        empty_text: ""
                        draw_bg +: {
                            color: fab.color_input
                            border_radius: fab.radius
                        }
                        draw_text +: {
                            color: fab.color_text
                            ink_centered: true
                            text_style: theme.font_regular{ font_size: fab.font_size_ui }
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

/// Which zone a pointer at `rel` (widget-local, origin top-left) lands in,
/// for a wheel drawn at `size` (its smaller dimension).
pub fn wheel_zone(rel: DVec2, size: f64) -> WheelZone {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let half = SQUARE_HALF * size;
    if dx.abs() <= half && dy.abs() <= half {
        return WheelZone::Square;
    }
    let r = (dx * dx + dy * dy).sqrt();
    if r <= RING_OUTER * size + 4.0 && r >= RING_INNER * size - 4.0 {
        return WheelZone::Ring;
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
pub fn drag_rate(p: &DragParams, width: f64, shift: bool) -> f64 {
    if p.bounded && p.has_range() {
        let rate = p.range() / width.max(1.0);
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

    #[rust]
    drag: Option<DragState>,
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

    fn sync_text(&mut self, cx: &mut Cx) {
        let t = self.format();
        self.text_input.set_text(cx, &t);
    }

    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        if self.editing || self.drag.is_some() {
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
        self.draw_bg.begin(cx, walk, self.layout);
        if !self.label.is_empty() {
            // The label spans exactly the space the value does not need:
            // the value lands right-anchored; a tight row elides the label,
            // never the number.
            let row = cx.turtle().rect().size.x;
            let pad = self.layout.padding.left + self.layout.padding.right;
            let fs = self.draw_text.text_style.font_size as f64;
            let value_reserve = (self.format().chars().count() as f64 + 0.5) * fs * 0.72 + 6.0;
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
        let iw = self.text_input.walk(cx);
        let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), iw);
        // The 3D-suite convention: stepper chevrons reveal on hover at the
        // field's edges — their zones (field_zone) exist regardless; the
        // glyphs only while the pointer is here and nothing is in flight.
        if self.hovered && !self.editing && self.drag.is_none() {
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

        // Double-click = RESET, detected on the raw press so it works in
        // every state (the second press of a double-click lands while the
        // first click's text editor is already open — the editor claims
        // hits, so hit-testing can't see it). Coexistence: a single click
        // still opens the editor instantly (snappy); the second click
        // within the window converts that into end-edit + reset.
        if let Event::MouseDown(me) = event {
            if me.button.is_primary()
                && self.draw_bg.area().clipped_rect(cx).contains(me.abs)
            {
                if me.time - self.last_press_time < 0.4 {
                    self.last_press_time = 0.0;
                    if self.editing {
                        self.end_edit(cx);
                        cx.revert_key_focus();
                    }
                    self.drag = None;
                    cx.widget_action(uid, FabValueInputAction::Reset);
                    return;
                }
                self.last_press_time = me.time;
            }
        }

        // Ctrl+Wheel nudges by one step; a plain wheel keeps scrolling the
        // panel underneath.
        if let Event::Scroll(e) = event {
            if e.modifiers.control || e.modifiers.logo {
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

        // Escape or a right-button press cancels an in-flight drag and
        // restores the pressed value.
        if self.drag.is_some() {
            match event {
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
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
                        if self.editing {
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
                let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                cx.set_cursor(match zone {
                    FieldZone::Middle => MouseCursor::EwResize,
                    _ => MouseCursor::Default,
                });
                self.hovered = true;
                self.draw_bg.redraw(cx);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOver(fe) => {
                if self.drag.is_none() && !self.editing {
                    let rect = self.draw_bg.area().rect(cx);
                    let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                    cx.set_cursor(match zone {
                        FieldZone::Middle => MouseCursor::EwResize,
                        _ => MouseCursor::Default,
                    });
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
                self.animator_play(cx, ids!(hover.off));
            }
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
                } else {
                    // A click. The zone at release decides: arrows step,
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
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabValueInputAction::Ended(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.value())
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
                let zone = wheel_zone(fe.abs - rect.pos, size);
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
                    self.apply_pointer(cx, uid, fe.abs, true);
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
    #[live]
    with_alpha: bool,
    /// The popover panel (wheel + rows + hex), from the type default.
    #[live]
    popover: View,
    #[rust]
    overlay_list: Option<DrawList2d>,
    #[rust]
    open: bool,
    #[rust]
    hsv: [f32; 3],
    #[rust(1.0)]
    alpha: f32,
    /// The colour when the popover opened — restored by Escape.
    #[rust]
    opened_value: [f32; 4],
    #[rust]
    panel_rect: Rect,
    #[rust]
    sync_pending: bool,
    /// Names of the palette entries, in strip order.
    #[rust]
    palette_names: Vec<String>,
}

impl ScriptHook for FabColorPick {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay_list = Some(DrawList2d::script_new(vm));
    }
}

impl FabColorPick {
    pub fn rgba(&self) -> [f32; 4] {
        let [h, s, v] = self.hsv;
        let [r, g, b] = hsv_to_rgb(h, s, v);
        [r, g, b, self.alpha]
    }

    pub fn set_rgba(&mut self, cx: &mut Cx, rgba: [f32; 4]) {
        self.hsv = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        self.alpha = rgba[3];
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
        self.open = false;
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
        let nums = [
            (live_id!(num_r), rgba[0]),
            (live_id!(num_g), rgba[1]),
            (live_id!(num_b), rgba[2]),
            (live_id!(num_a), rgba[3]),
        ];
        for (id, channel) in nums {
            if let Some(mut num) = self.popover.child(id).borrow_mut::<FabValueInput>() {
                num.set_value(cx, (channel * 255.0).round() as f64);
            }
        }
        let hex = self.popover.child(live_id!(hex_row)).child(live_id!(hex));
        if !hex.is_empty() {
            // Don't stomp the hex text while the person is typing in it.
            if hex.area() == Area::Empty || !cx.has_key_focus(hex.area()) {
                hex.set_text(cx, &format_hex(rgba, self.with_alpha));
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
        self.opened_value = self.rgba();
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
        let uid = self.widget_uid();
        if revert {
            let original = self.opened_value;
            self.hsv = rgb_to_hsv(original[0], original[1], original[2]);
            self.alpha = original[3];
            self.publish(cx, uid, true);
        } else {
            self.publish(cx, uid, true);
        }
        self.open = false;
        self.draw_swatch.open = 0.0;
        cx.widget_action(uid, FabColorPickAction::Closed);
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
        self.draw_swatch.redraw(cx);
        cx.redraw_all();
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
            let est_height = 360.0_f64 + if strip_height > 0.0 { strip_height + 26.0 } else { 0.0 };
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
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        if self.open {
            // Escape reverts and closes, from anywhere.
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Escape {
                    self.close_popover(cx, true);
                    return;
                }
            }
            // A press outside the panel and the swatch commits and closes.
            if let Event::MouseDown(me) = event {
                let swatch_rect = self.draw_swatch.area().rect(cx);
                if !self.panel_rect.contains(me.abs) && !swatch_rect.contains(me.abs) {
                    self.close_popover(cx, false);
                    // Do not return: the press still belongs to whatever is
                    // underneath.
                }
            }
            let mut changed = false;
            let mut ended = false;
            for action in cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope)) {
                let Some(widget_action) = action.as_widget_action() else {
                    continue;
                };
                let wheel_uid = self.popover.child(live_id!(wheel)).widget_uid();
                let r_uid = self.popover.child(live_id!(num_r)).widget_uid();
                let g_uid = self.popover.child(live_id!(num_g)).widget_uid();
                let b_uid = self.popover.child(live_id!(num_b)).widget_uid();
                let a_uid = self.popover.child(live_id!(num_a)).widget_uid();
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
                                    self.hsv = rgb_to_hsv(c[0], c[1], c[2]);
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
                        let mut rgba = self.rgba();
                        if widget_action.widget_uid == r_uid {
                            rgba[0] = channel;
                        } else if widget_action.widget_uid == g_uid {
                            rgba[1] = channel;
                        } else if widget_action.widget_uid == b_uid {
                            rgba[2] = channel;
                        } else {
                            rgba[3] = channel;
                        }
                        self.hsv = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                        self.alpha = rgba[3];
                        changed = true;
                        ended |= is_ended;
                    }
                } else if widget_action.widget_uid == hex_uid {
                    if let TextInputAction::Returned(text, _) =
                        widget_action.cast::<TextInputAction>()
                    {
                        if let Some((rgba, had_alpha)) = parse_hex(&text) {
                            self.hsv = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                            if had_alpha {
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

        match event.hits(cx, self.draw_swatch.area()) {
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

    #[test]
    fn a_bounded_field_sweeps_its_range_across_its_width() {
        let p = bounded(0.0, 24.0, false);
        let a = DragAnchor { x: 0.0, value: 12.0 };
        let (v, _) = drag_map(&p, a, 100.0, 200.0, false, false);
        assert!((v - 24.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 50.0, 200.0, false, false);
        assert!((v - 18.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 50.0, 200.0, true, false);
        assert!((v - 12.3).abs() < 1e-9, "{v}");
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
        let (v, _) = drag_map(&p, a2, 299.0, 100.0, false, false);
        assert!((v - 0.99).abs() < 1e-9, "{v}");
    }

    #[test]
    fn cyclic_fields_wrap_at_their_ends() {
        let p = bounded(0.0, 24.0, true);
        let a = DragAnchor { x: 0.0, value: 23.0 };
        let (v, _) = drag_map(&p, a, 100.0, 1200.0, false, false);
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

    #[test]
    fn the_zones_split_arrows_from_the_drag_surface() {
        assert_eq!(field_zone(5.0, 200.0, 20.0), FieldZone::Decrement);
        assert_eq!(field_zone(100.0, 200.0, 20.0), FieldZone::Middle);
        assert_eq!(field_zone(195.0, 200.0, 20.0), FieldZone::Increment);
    }
}
