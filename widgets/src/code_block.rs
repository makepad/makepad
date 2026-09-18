//! CodeBlock — a run of code shown as a block, with its lines counted and a
//! control that takes a copy.
//!
//! A block of code in a page is read, not edited. This one lays its lines
//! out itself: a gutter of numbers that can be turned off, a header carrying
//! the language's name and the copy control, and a band behind the lines the
//! writer wants looked at. `prompt` puts a mark before a line in place of a
//! number, which is what turns a listing into a session.
//!
//! # What it deliberately does not do
//!
//! **It does not highlight syntax.** Every line is one ink. Highlighting
//! means a parser per language and this library already has one, in the
//! editor: `CodeView` is that editor wearing a read-only preset, and code
//! that wants colour, selection, or a line too long to fit belongs there
//! rather than here. Running prose with a few words of code in it is served
//! already too — `Html` and `Markdown` draw `<pre>` and `<code>` through the
//! text flow — and this is not a replacement for either.
//!
//! It does not wrap, scroll or edit. A line wider than the block is cut off
//! at the trailing padding, because a wrapped line stops lining up with the
//! one above it and indentation is how code is read. Give a block
//! `width: Fit` where nothing may be cut.
//!
//! # What is drawn and what is copied
//!
//! The numbers and the prompt marks are DRAWN. They are not part of `text`,
//! and that is the whole reason they are here: a copy gives back the code
//! exactly as it was handed in, so pasting it does not first mean deleting a
//! column of numbers or a mark from the front of every line.
//!
//! Tabs are the one place the two disagree. A tab is expanded to the next
//! stop for drawing — the text engine has no tab stop, so an indented block
//! would otherwise come out flush left with nothing to say it had lost
//! anything — and a copy hands the tabs back untouched, because what is
//! pasted has to compile.
use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    badge::{measure, sized},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CodeBlockBase = #(CodeBlock::register_widget(vm))

    set_type_default() do #(DrawCodeBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A run of code as a block: counted lines, a language label, and a
     * control that takes a copy. */
    mod.widgets.CodeBlock = set_type_default() do mod.widgets.CodeBlockBase{
        width: Fill
        height: Fit
        margin: theme.mspace_v_1

        /** the code; every newline starts a line */
        text: ""
        /** the name at the leading edge of the header; empty draws none */
        language: ""
        /** count the lines down the gutter */
        show_line_numbers: true
        /** the number the first line carries 1..9999 step 1 */
        first_line: 1
        /** lines drawn with a band behind them, as shown numbers: "3, 7-9" */
        highlight: ""
        /** a mark drawn before a line in place of a number: "$" */
        prompt: ""
        /** which lines carry that mark; empty is every line */
        prompt_lines: ""
        /** offer the copy control */
        show_copy: true
        /** the copy control's label */
        copy_text: "Copy"
        /** the label it wears for a moment after a copy */
        copied_text: "Copied"
        /** how long that label stays, in seconds 0.2..10 step 0.1 */
        copied_secs: 1.5
        /** columns a drawn tab advances to 1..16 step 1 */
        tab_size: 4

        /** one line's height in pixels 8..48 step 0.5 */
        line_height: 15.
        /** the header's height in pixels 14..48 step 1 */
        header_height: 22.
        /** padding at the leading and trailing edges in pixels 0..40 step 1 */
        pad_x: 8.
        /** padding above and below the lines in pixels 0..40 step 1 */
        pad_y: 6.
        /** room between the gutter's parts and the code in pixels 0..24 step 1 */
        gutter_gap: 6.

        /** the band behind a line named by highlight */
        color_highlight: theme.color_bg_highlight
        /** the mark down that line's leading edge */
        color_highlight_mark: theme.color_primary

        draw_code +: {
            color: theme.color_on_surface
            // Every line is placed by hand, so a line spacing other than 1
            // would only shift the ink inside a box nothing else reads.
            text_style: theme.font_code{line_spacing: 1.0}
        }
        draw_gutter +: {
            color: theme.color_text_meta
            text_style: theme.font_code{line_spacing: 1.0}
        }
        draw_prompt +: {
            color: theme.color_primary
            text_style: theme.font_code{line_spacing: 1.0}
        }
        draw_meta +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.type_label_s_size line_spacing: 1.0}
        }

        draw_bg +: {
            /** the copied moment 0..1 step 0.01 */
            copied: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)

            /** border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_surface_container_low)
            border_color: uniform(theme.color_outline_variant)
            /** the hairlines under the header and beside the gutter */
            rule_color: uniform(theme.color_outline_variant)
            /** the copy control's face at rest */
            copy_color: uniform(theme.color_surface_container_high)
            /** its face under the pointer */
            copy_color_hover: uniform(theme.color_surface_container_highest)
            /** its face for the moment after a copy */
            copy_color_copied: uniform(theme.color_success_container)
            /** its edge while the keyboard is on it */
            copy_color_focus: uniform(theme.color_primary)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let bs = self.border_size

                sdf.box(
                    bs
                    bs
                    self.rect_size.x - bs * 2.
                    self.rect_size.y - bs * 2.
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, bs)

                // Both rules are bars laid where the block's own sides are
                // already straight, so neither can square off a rounded
                // corner by running into it.
                if self.header_px > 0.5 {
                    sdf.rect(bs, self.header_px, max(self.rect_size.x - bs * 2., 1.0), 1.0)
                    sdf.fill(self.rule_color)
                }
                if self.gutter_px > 0.5 {
                    sdf.rect(
                        self.gutter_px
                        self.header_px + bs
                        1.0
                        max(self.rect_size.y - self.header_px - bs * 2., 1.0)
                    )
                    sdf.fill(self.rule_color)
                }

                // The control is placed from the TRAILING edge, because the
                // width it is placed in is not known until the block has
                // been laid out and the instance is already emitted by then.
                if self.copy_w > 0.5 {
                    let bx = self.rect_size.x - self.copy_inset - self.copy_w
                    let by = (self.header_px - self.copy_h) * 0.5
                    let face = self.copy_color
                        .mix(self.copy_color_hover, self.hot_copy)
                        .mix(self.copy_color_copied, self.copied)
                    let edge = self.rule_color.mix(self.copy_color_focus, self.focus)
                    sdf.box(bx, by, self.copy_w, self.copy_h, self.border_radius)
                    sdf.fill_keep(face)
                    sdf.stroke(edge, 1.0 + self.focus)
                }
                return sdf.result
            }
        }

        animator: Animator{
            copied: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.25}}
                    apply: {draw_bg: {copied: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {copied: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
        }
    }

    /** A session rather than a listing: a mark before each line in place of
     * a number, on a darker face. */
    mod.widgets.CodeTerminal = mod.widgets.CodeBlock{
        show_line_numbers: false
        prompt: "$"
        draw_bg +: {color: theme.color_surface_container_lowest}
        draw_prompt +: {color: theme.color_success}
    }

    /** A few words of code inside a sentence: no header, no gutter, and only
     * as wide as the words. */
    mod.widgets.CodeInline = mod.widgets.CodeBlock{
        width: Fit
        height: Fit
        margin: 0.
        show_line_numbers: false
        show_copy: false
        line_height: 13.
        pad_x: 4.
        pad_y: 1.
        draw_bg +: {
            border_radius: theme.radius_s
            color: theme.color_bg_highlight
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCodeBlock {
    #[deref]
    draw_super: DrawQuad,
    /// The header's height, zero when there is no header, and where the
    /// hairline beside the gutter stands. Rust measures both, so Rust owns
    /// them and the shader is handed the same numbers the text was drawn at.
    #[live]
    header_px: f32,
    #[live]
    gutter_px: f32,
    /// The copy control's box, given as a size and a trailing inset rather
    /// than as a position: see the note in the shader.
    #[live]
    copy_w: f32,
    #[live]
    copy_h: f32,
    #[live]
    copy_inset: f32,
    /// The pointer is over the copy control.
    #[live]
    hot_copy: f32,
}

/// Where the parts of a block go, worked out from the numbers the widget
/// holds. It is a separate type for two reasons — it can be tested without a
/// script heap, and the hit test and the drawing take the copy control's box
/// from the same place, so what is drawn is what is pressed.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Frame {
    lines: usize,
    line_h: f64,
    /// Zero when there is no header at all.
    header_h: f64,
    /// The numbers and the prompt together, with the room after them.
    gutter_w: f64,
    pad_x: f64,
    pad_y: f64,
    /// Zero when there is no copy control.
    copy_w: f64,
    copy_h: f64,
}

impl Frame {
    /// The top of the first line: past the header, then the padding.
    fn body_top(&self) -> f64 {
        self.header_h + self.pad_y
    }

    fn height(&self) -> f64 {
        self.body_top() + self.lines as f64 * self.line_h + self.pad_y
    }

    fn line_top(&self, index: usize) -> f64 {
        self.body_top() + index as f64 * self.line_h
    }

    /// Where the code starts, from the block's leading edge.
    fn code_x(&self) -> f64 {
        self.pad_x + self.gutter_w
    }

    /// The width a block asks for when nothing else decides: its longest
    /// line, whole.
    fn width_for(&self, longest: f64) -> f64 {
        self.code_x() + longest + self.pad_x
    }

    /// The copy control's box, in the block's own frame.
    fn copy_rect(&self, width: f64) -> Rect {
        Rect {
            pos: dvec2(width - self.pad_x - self.copy_w, (self.header_h - self.copy_h) * 0.5),
            size: dvec2(self.copy_w, self.copy_h),
        }
    }

    /// Whether a press at this point in the block takes the copy control.
    ///
    /// The box is grown a little all round. The control is small, it sits at
    /// the very corner of the block, and a press that misses it by a point
    /// does nothing whatever rather than something else.
    fn on_copy(&self, x: f64, y: f64, width: f64) -> bool {
        if self.copy_w <= 0.0 || self.header_h <= 0.0 {
            return false;
        }
        let r = self.copy_rect(width);
        x >= r.pos.x - GRAB_SLACK
            && x <= r.pos.x + r.size.x + GRAB_SLACK
            && y >= r.pos.y - GRAB_SLACK
            && y <= r.pos.y + r.size.y + GRAB_SLACK
    }
}

/// How far below the top of its line box a glyph's ink starts, as a share of
/// the font size. `draw_abs` takes the line box, not the ink.
const INK_DROP: f64 = 0.30;

/// Room left above and below the copy control inside the header.
const COPY_PAD_Y: f64 = 4.0;

/// How far outside the copy control a press still takes it.
const GRAB_SLACK: f64 = 3.0;

/// How far a highlight band stays off the block's own edge, so the band
/// cannot draw over the border it sits inside.
const BAND_INSET: f64 = 1.0;

/// The width of the mark down a highlighted line's leading edge.
const BAND_MARK: f64 = 2.0;

/// A tab expanded to the next multiple of `tab` columns.
///
/// For DRAWING only: the text engine has no tab stop, so a tab draws as
/// nothing and code indented with tabs would come out flush left with no
/// sign that anything had been lost. A copy hands back the original text.
fn expand_tabs(line: &str, tab: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let tab = tab.max(1);
    let mut out = String::with_capacity(line.len() + tab);
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let stop = (column / tab + 1) * tab;
            out.push_str(&" ".repeat(stop - column));
            column = stop;
        } else {
            out.push(c);
            column += 1;
        }
    }
    out
}

/// How many columns the gutter needs for the last number it will show.
fn number_columns(first: usize, lines: usize) -> usize {
    if lines == 0 {
        0
    } else {
        (first + lines - 1).to_string().len()
    }
}

/// Which of `lines` lines a spec such as "3, 7-9" names, given the number the
/// first line carries.
///
/// A part that is not a number or a range of them is skipped rather than
/// rejected. The spec is written by hand in a source file, and a block that
/// drew no bands at all because of one stray comma would be harder to put
/// right than one that draws the bands it understood.
fn marked(spec: &str, lines: usize, first: usize) -> Vec<bool> {
    let mut out = vec![false; lines];
    if lines == 0 {
        return out;
    }
    let last = first + lines - 1;
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a.trim().parse::<usize>(), b.trim().parse::<usize>()),
            None => (part.parse::<usize>(), part.parse::<usize>()),
        };
        let (Ok(from), Ok(to)) = (from, to) else {
            continue;
        };
        // Written the other way round is still a range.
        let (from, to) = (from.min(to), from.max(to));
        // Clamped before the loop rather than inside it: a spec may name a
        // range far past the end of the block, and counting up to it would
        // be the only slow thing in the draw.
        if to < first || from > last {
            continue;
        }
        for number in from.max(first)..=to.min(last) {
            out[number - first] = true;
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub enum CodeBlockAction {
    /// The code is on the clipboard.
    Copied,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct CodeBlock {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawCodeBlock,
    /// The band behind a highlighted line, and the mark down its leading
    /// edge. One layer draws both, in two colours.
    #[live]
    draw_mark: DrawColor,
    #[live]
    draw_code: DrawText,
    #[live]
    draw_gutter: DrawText,
    #[live]
    draw_prompt: DrawText,
    /// The header's ink: the language's name and the copy control's label.
    #[live]
    draw_meta: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The code, exactly as it will be copied.
    #[live]
    pub text: String,
    /// The name shown at the leading edge of the header. It is a label and
    /// nothing more: no part of this widget reads it.
    #[live]
    pub language: String,
    #[live(true)]
    pub show_line_numbers: bool,
    /// The number the first line carries, for an excerpt out of the middle
    /// of a file.
    #[live(1u32)]
    pub first_line: u32,
    /// The lines drawn with a band, as the numbers the gutter shows.
    #[live]
    pub highlight: String,
    /// The mark drawn before a line in place of a number.
    #[live]
    pub prompt: String,
    /// Which lines carry the mark. Empty is every line, which is what a
    /// block of commands wants; a transcript with output in it names the
    /// command lines here so the output is left plain.
    #[live]
    pub prompt_lines: String,

    #[live(true)]
    pub show_copy: bool,
    #[live("Copy".to_string())]
    pub copy_text: String,
    #[live("Copied".to_string())]
    pub copied_text: String,
    #[live(1.5)]
    pub copied_secs: f64,
    #[live(4u32)]
    pub tab_size: u32,

    #[live(15.0)]
    pub line_height: f64,
    #[live(22.0)]
    pub header_height: f64,
    #[live(8.0)]
    pub pad_x: f64,
    #[live(6.0)]
    pub pad_y: f64,
    #[live(6.0)]
    pub gutter_gap: f64,

    #[live]
    pub color_highlight: Vec4f,
    #[live]
    pub color_highlight_mark: Vec4f,

    #[rust]
    copied: bool,
    #[rust]
    copied_timer: Timer,
    /// What the last draw measured. The hit test has no text engine to ask,
    /// and the line count is what turns a y into a line.
    #[rust]
    line_count: usize,
    #[rust]
    number_w: f64,
    #[rust]
    prompt_w: f64,
    #[rust]
    copy_w: f64,
}

impl CodeBlock {
    /// A header exists only if something would be in it.
    fn has_header(&self) -> bool {
        !self.language.is_empty() || self.has_copy()
    }

    /// There is nothing to copy from an empty block, so it is not offered.
    fn has_copy(&self) -> bool {
        self.show_copy && !self.text.is_empty()
    }

    fn header_h(&self) -> f64 {
        if self.has_header() {
            self.header_height.max(0.0)
        } else {
            0.0
        }
    }

    fn copy_h(&self) -> f64 {
        (self.header_h() - COPY_PAD_Y * 2.0).max(10.0)
    }

    /// The numbers, the mark, and the room after each of them.
    fn gutter_w(&self) -> f64 {
        let numbers = if self.number_w > 0.0 { self.number_w + self.gutter_gap } else { 0.0 };
        let prompt = if self.prompt_w > 0.0 { self.prompt_w + self.gutter_gap } else { 0.0 };
        numbers + prompt
    }

    /// The block as it stands, built fresh each time: every number in it is
    /// a live property and the tweaker may have moved any of them since the
    /// last draw.
    fn frame(&self) -> Frame {
        Frame {
            lines: self.line_count,
            line_h: self.line_height.max(1.0),
            header_h: self.header_h(),
            gutter_w: self.gutter_w(),
            pad_x: self.pad_x,
            pad_y: self.pad_y,
            copy_w: self.copy_w,
            copy_h: self.copy_h(),
        }
    }

    fn copy_label(&self) -> String {
        if self.copied && !self.copied_text.is_empty() {
            self.copied_text.clone()
        } else {
            self.copy_text.clone()
        }
    }

    /// Put the code on the clipboard and start the copied moment.
    pub fn copy(&mut self, cx: &mut Cx) {
        cx.copy_to_clipboard(&self.text);
        self.mark_copied(cx);
    }

    /// The copied moment on its own, for the platform's own copy chord: the
    /// clipboard is the platform's business there, and writing to it as well
    /// would put the same text on it twice.
    fn mark_copied(&mut self, cx: &mut Cx) {
        self.copied = true;
        cx.stop_timer(self.copied_timer);
        self.copied_timer = cx.start_timeout(self.copied_secs.max(0.1));
        self.animator_play(cx, ids!(copied.on));
        cx.widget_action(self.uid, CodeBlockAction::Copied);
        self.draw_bg.redraw(cx);
    }
}

impl Widget for CodeBlock {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(copy) {
            vm.with_cx_mut(|cx| self.copy(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);

        if self.copied_timer.is_event(event).is_some() {
            self.copied = false;
            self.animator_play(cx, ids!(copied.off));
            self.draw_bg.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverOut(_) => {
                if self.draw_bg.hot_copy != 0.0 {
                    self.draw_bg.hot_copy = 0.0;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOver(fe) => {
                let over = self.frame().on_copy(
                    fe.abs.x - fe.rect.pos.x,
                    fe.abs.y - fe.rect.pos.y,
                    fe.rect.size.x,
                );
                let hot = if over { 1.0 } else { 0.0 };
                if self.draw_bg.hot_copy != hot {
                    self.draw_bg.hot_copy = hot;
                    self.draw_bg.redraw(cx);
                }
                if over {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                // The whole block takes focus, not just the control on it:
                // the copy chord is the reason to have focus here, and it
                // has to work after a press anywhere on the code.
                cx.set_key_focus(self.draw_bg.area());
                let local = fe.abs - fe.rect.pos;
                if self.frame().on_copy(local.x, local.y, fe.rect.size.x) {
                    self.copy(cx);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) => {
                if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) && self.has_copy() {
                    self.copy(cx);
                }
            }
            // The platform's own copy chord, which reaches the block
            // because it has key focus. The response is the clipboard here,
            // so the moment is shown without writing to it again.
            Hit::TextCopy(te) => {
                if !self.text.is_empty() {
                    *te.response.borrow_mut() = Some(self.text.clone());
                    self.mark_copied(cx);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let tab = self.tab_size as usize;
        let lines: Vec<String> = self.text.lines().map(|line| expand_tabs(line, tab)).collect();
        let first = self.first_line.max(1) as usize;
        self.line_count = lines.len();

        // The gutter's two parts. The numbers are right-aligned by padding
        // them out to the widest one rather than by measuring each: the
        // gutter is set in the code face, whose digits all share a width,
        // and measuring a run per line per frame to learn that would be
        // work done to reach the answer already in hand.
        let columns = if self.show_line_numbers { number_columns(first, lines.len()) } else { 0 };
        let number_w = if columns == 0 {
            0.0
        } else {
            measure(&self.draw_gutter, cx, &"0".repeat(columns))
        };
        let prompt_w = if self.prompt.is_empty() {
            0.0
        } else {
            measure(&self.draw_prompt, cx, &self.prompt)
        };
        // The control keeps the width of the longer of its two labels, so it
        // does not change size under the pointer the instant it is pressed.
        let copy_w = if self.has_copy() && self.has_header() {
            let rest = measure(&self.draw_meta, cx, &self.copy_text);
            let done = measure(&self.draw_meta, cx, &self.copied_text);
            rest.max(done) + self.pad_x * 2.0
        } else {
            0.0
        };
        self.number_w = number_w;
        self.prompt_w = prompt_w;
        self.copy_w = copy_w;

        let frame = self.frame();
        // Only a block sized to its content needs to know how wide its
        // longest line is, and measuring every line is the one thing here
        // that costs anything worth avoiding.
        let longest = if matches!(walk.width, Size::Fit { .. }) {
            let mut widest = 0.0f64;
            for line in &lines {
                widest = widest.max(measure(&self.draw_code, cx, line));
            }
            widest
        } else {
            0.0
        };
        let walk = sized(walk, frame.width_for(longest), frame.height());

        self.draw_bg.header_px = frame.header_h as f32;
        // The hairline stands halfway through the room after the gutter, so
        // it belongs to neither side.
        self.draw_bg.gutter_px = if frame.gutter_w > 0.0 {
            (self.pad_x + frame.gutter_w - self.gutter_gap * 0.5) as f32
        } else {
            0.0
        };
        self.draw_bg.copy_w = frame.copy_w as f32;
        self.draw_bg.copy_h = frame.copy_h as f32;
        self.draw_bg.copy_inset = self.pad_x as f32;

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();

        let bands = marked(&self.highlight, lines.len(), first);
        let band_w = (rect.size.x - BAND_INSET * 2.0).max(0.0);
        for (index, on) in bands.iter().enumerate() {
            if !*on {
                continue;
            }
            let top = rect.pos.y + frame.line_top(index);
            let x = rect.pos.x + BAND_INSET;
            self.draw_mark.color = self.color_highlight;
            self.draw_mark
                .draw_abs(cx, Rect { pos: dvec2(x, top), size: dvec2(band_w, frame.line_h) });
            self.draw_mark.color = self.color_highlight_mark;
            self.draw_mark
                .draw_abs(cx, Rect { pos: dvec2(x, top), size: dvec2(BAND_MARK, frame.line_h) });
        }

        let code_x = rect.pos.x + frame.code_x();
        let code_font = self.draw_code.text_style.font_size as f64;
        // Long lines are cut at the trailing padding rather than wrapped: a
        // wrapped line stops lining up with the one above it, and
        // indentation is how code is read.
        cx.push_clip_rect(Rect {
            pos: dvec2(code_x, rect.pos.y),
            size: dvec2((rect.pos.x + rect.size.x - self.pad_x - code_x).max(0.0), rect.size.y),
        });
        for (index, line) in lines.iter().enumerate() {
            let top = rect.pos.y + frame.line_top(index);
            let y = top + (frame.line_h - code_font) * 0.5 - code_font * INK_DROP;
            self.draw_code.draw_abs(cx, dvec2(code_x, y), line);
        }
        cx.pop_clip_rect();

        // The gutter sits outside that clip: it is to the LEFT of where the
        // code starts, and clipping the code to its own column would take
        // the numbers with it.
        let prompted = if self.prompt.is_empty() {
            Vec::new()
        } else if self.prompt_lines.is_empty() {
            vec![true; lines.len()]
        } else {
            marked(&self.prompt_lines, lines.len(), first)
        };
        let number_x = rect.pos.x + self.pad_x;
        let prompt_x = number_x + if self.number_w > 0.0 { self.number_w + self.gutter_gap } else { 0.0 };
        let gutter_font = self.draw_gutter.text_style.font_size as f64;
        for index in 0..lines.len() {
            let top = rect.pos.y + frame.line_top(index);
            let y = top + (frame.line_h - gutter_font) * 0.5 - gutter_font * INK_DROP;
            if columns > 0 {
                let number = format!("{:>width$}", first + index, width = columns);
                self.draw_gutter.draw_abs(cx, dvec2(number_x, y), &number);
            }
            if prompted.get(index).copied().unwrap_or(false) {
                let mark = self.prompt.clone();
                self.draw_prompt.draw_abs(cx, dvec2(prompt_x, y), &mark);
            }
        }

        if frame.header_h > 0.0 {
            let meta_font = self.draw_meta.text_style.font_size as f64;
            let y = rect.pos.y + (frame.header_h - meta_font) * 0.5 - meta_font * INK_DROP;
            if !self.language.is_empty() {
                let name = self.language.clone();
                self.draw_meta.draw_abs(cx, dvec2(rect.pos.x + self.pad_x, y), &name);
            }
            if frame.copy_w > 0.0 {
                let label = self.copy_label();
                let width = measure(&self.draw_meta, cx, &label);
                let box_ = frame.copy_rect(rect.size.x);
                let x = rect.pos.x + box_.pos.x + (box_.size.x - width) * 0.5;
                self.draw_meta.draw_abs(cx, dvec2(x, y), &label);
            }
        }
        self.draw_bg.end(cx);

        // A block with no copy control has nothing the keyboard could do,
        // and a stop that lights up and then answers nothing is worse than
        // no stop. The role list has no entry for a block, and TextInput is
        // the one that plainly stops.
        if frame.copy_w > 0.0 {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.draw_bg.redraw(cx);
        }
    }
}

impl CodeBlockRef {
    /// Put the code on the clipboard, as pressing the control would.
    pub fn copy(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.copy(cx);
        }
    }

    /// Whether this block's code has just gone onto the clipboard.
    pub fn copied(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), CodeBlockAction::Copied))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block with the geometry the preset gives it: a header, a gutter
    /// two digits wide, and a copy control.
    fn block(lines: usize) -> Frame {
        Frame {
            lines,
            line_h: 15.0,
            header_h: 22.0,
            gutter_w: 20.0,
            pad_x: 8.0,
            pad_y: 6.0,
            copy_w: 40.0,
            copy_h: 14.0,
        }
    }

    #[test]
    fn a_block_is_as_tall_as_its_lines_its_header_and_its_padding() {
        let f = block(4);
        assert_eq!(f.height(), 22.0 + 6.0 + 4.0 * 15.0 + 6.0);
        // An empty block is still a header and the room round nothing.
        assert_eq!(block(0).height(), 22.0 + 6.0 + 6.0);
    }

    #[test]
    fn a_block_without_a_header_starts_at_its_own_padding() {
        let f = Frame { header_h: 0.0, copy_w: 0.0, ..block(2) };
        assert_eq!(f.body_top(), 6.0);
        assert_eq!(f.line_top(0), 6.0);
        assert_eq!(f.line_top(1), 21.0);
    }

    #[test]
    fn lines_stack_below_the_header() {
        let f = block(3);
        assert_eq!(f.line_top(0), 28.0);
        assert_eq!(f.line_top(2), 58.0);
    }

    #[test]
    fn the_natural_width_is_the_longest_line_whole() {
        let f = block(3);
        assert_eq!(f.code_x(), 28.0);
        assert_eq!(f.width_for(200.0), 28.0 + 200.0 + 8.0);
    }

    #[test]
    fn the_copy_control_hangs_off_the_trailing_edge() {
        let f = block(3);
        let r = f.copy_rect(300.0);
        assert_eq!(r.pos.x, 300.0 - 8.0 - 40.0);
        assert_eq!(r.pos.y, 4.0, "centred in the header");
        // The same box at a different width, because the block is laid out
        // before anyone knows how wide it is.
        assert_eq!(f.copy_rect(500.0).pos.x, 500.0 - 8.0 - 40.0);
    }

    #[test]
    fn a_press_on_the_copy_control_is_told_from_one_beside_it() {
        let f = block(3);
        assert!(f.on_copy(280.0, 11.0, 300.0));
        assert!(!f.on_copy(20.0, 11.0, 300.0), "the language label is not the control");
        assert!(!f.on_copy(280.0, 40.0, 300.0), "nor is the code below it");
    }

    #[test]
    fn a_press_that_misses_the_control_by_a_point_still_takes_it() {
        let f = block(3);
        let r = f.copy_rect(300.0);
        assert!(f.on_copy(r.pos.x - 2.0, 11.0, 300.0));
        assert!(!f.on_copy(r.pos.x - 6.0, 11.0, 300.0));
    }

    #[test]
    fn a_block_with_no_control_takes_no_press_at_all() {
        let f = Frame { copy_w: 0.0, ..block(3) };
        assert!(!f.on_copy(280.0, 11.0, 300.0));
        let f = Frame { header_h: 0.0, ..block(3) };
        assert!(!f.on_copy(280.0, 11.0, 300.0));
    }

    #[test]
    fn a_tab_goes_to_the_next_stop_not_a_fixed_number_of_spaces() {
        assert_eq!(expand_tabs("\tlet x", 4), "    let x");
        assert_eq!(expand_tabs("ab\tc", 4), "ab  c", "two columns in, two to go");
        assert_eq!(expand_tabs("abcd\te", 4), "abcd    e", "on a stop, a whole one");
        assert_eq!(expand_tabs("\t\tx", 4), "        x");
    }

    #[test]
    fn a_line_without_tabs_comes_back_as_it_was() {
        assert_eq!(expand_tabs("let x = 1;", 4), "let x = 1;");
        assert_eq!(expand_tabs("", 4), "");
    }

    #[test]
    fn a_tab_size_of_zero_still_advances() {
        // The property is live and the tweaker can wind it to nothing; an
        // expansion that advanced by zero would never leave the character.
        assert_eq!(expand_tabs("a\tb", 0), "a b");
    }

    #[test]
    fn the_gutter_is_as_wide_as_the_last_number_it_shows() {
        assert_eq!(number_columns(1, 9), 1);
        assert_eq!(number_columns(1, 10), 2);
        assert_eq!(number_columns(98, 5), 3, "an excerpt counts from where it starts");
        assert_eq!(number_columns(1, 0), 0);
    }

    #[test]
    fn a_spec_names_single_lines_and_ranges() {
        assert_eq!(marked("2", 4, 1), vec![false, true, false, false]);
        assert_eq!(marked("2-3", 4, 1), vec![false, true, true, false]);
        assert_eq!(marked("1, 4", 4, 1), vec![true, false, false, true]);
    }

    #[test]
    fn a_range_written_backwards_is_still_a_range() {
        assert_eq!(marked("3-2", 4, 1), vec![false, true, true, false]);
    }

    #[test]
    fn a_spec_is_read_in_the_numbers_the_gutter_shows() {
        // A block that starts at 40 is highlighted by what is on screen,
        // not by how far down the block a line happens to be.
        assert_eq!(marked("41", 3, 40), vec![false, true, false]);
        assert_eq!(marked("1", 3, 40), vec![false, false, false]);
    }

    #[test]
    fn a_stray_part_costs_only_itself() {
        assert_eq!(marked("2, oops, 4", 4, 1), vec![false, true, false, true]);
        assert_eq!(marked(",,", 4, 1), vec![false; 4]);
        assert_eq!(marked("", 4, 1), vec![false; 4]);
    }

    #[test]
    fn a_range_past_the_end_of_the_block_marks_what_it_reaches() {
        assert_eq!(marked("3-999999", 4, 1), vec![false, false, true, true]);
        assert_eq!(marked("100-200", 4, 1), vec![false; 4]);
    }

    #[test]
    fn a_spec_against_an_empty_block_names_nothing() {
        assert!(marked("1-3", 0, 1).is_empty());
    }
}
