//! `SynthPicker{node: "mono"}`: the instrument switch. It shows what the
//! node is (a synth preset, an audio shader, a drum kit, a General MIDI
//! program, the piano …) and opens the list of everything it could be
//! instead, grouped and searchable (type while it is open). Resting on a row plays the node as
//! that instrument in place (a short phrase of its own notes, on the cue
//! or the main output as the desk routes previews); a click makes it so:
//! the node is rewritten, keeping its name, so every loop that plays it
//! keeps its notes; one undo step.

use crate::host as synth;
use crate::choices::{ChoiceList, Clicked};
use makepad_value_pickers::panel::control::{DrawPanelShape, R_GROUP};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthPickerBase = #(SynthPicker::register_widget(vm))
    /** The instrument a node is, and what it could be instead (previewed, switched). */
    mod.widgets.SynthPicker = set_type_default() do mod.widgets.SynthPickerBase{
        width: Fill height: Fit list_height: 360
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_bold +: {text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

const HEAD: f64 = 28.0;

#[derive(Script, ScriptHook, Widget)]
pub struct SynthPicker {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_row: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    node: String,
    #[live]
    label: String,
    #[live]
    list_height: f64,
    #[rust]
    open: bool,
    #[rust]
    list: ChoiceList,
    #[rust]
    head: Rect,
    #[rust]
    next_frame: NextFrame,
}

impl Widget for SynthPicker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.list.tick() {
            self.next_frame = cx.new_next_frame();
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerHoverOver(fe) => {
                let (changed, wake) = self.list.hover_at(fe.abs);
                if wake {
                    self.next_frame = cx.new_next_frame();
                }
                if changed {
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.list.stop_preview();
                self.redraw(cx);
            }
            Hit::TextInput(te) if self.open => {
                self.list.type_text(&te.input);
                self.redraw(cx);
            }
            Hit::KeyDown(ke) if self.open && ke.key_code == KeyCode::Backspace => {
                self.list.backspace();
                self.redraw(cx);
            }
            Hit::KeyDown(ke) if self.open && ke.key_code == KeyCode::Escape => {
                self.list.stop_preview();
                self.open = false;
                synth::set_searching(false);
                self.redraw(cx);
            }
            Hit::KeyFocus(_) => synth::set_searching(self.open),
            Hit::KeyFocusLost(_) => synth::set_searching(false),
            Hit::FingerScroll(fe) if self.open => {
                self.list.scroll_by(fe.scroll.y);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && fe.was_tap() => {
                synth::select(&self.node);
                if self.head.contains(fe.abs) {
                    self.open = !self.open;
                    if self.open {
                        self.list.load(&self.node);
                        cx.set_key_focus(self.draw_bg.area());
                    } else {
                        self.list.stop_preview();
                    }
                    synth::set_searching(self.open);
                } else if self.list.click(fe.abs) == Clicked::Switched {
                    self.open = false;
                    synth::set_searching(false);
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.list.loaded() {
            self.list.load(&self.node);
        }
        let p = makepad_look::palette::get(cx);
        let height = if self.open { HEAD + self.list_height } else { HEAD };
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(height), ..walk });
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_row.edged_round(cx, rect, color(p.row, 1.0), color(p.separator, 1.0), 1.0, R_GROUP);
        self.head = Rect { pos: rect.pos, size: dvec2(rect.size.x, HEAD) };
        let label = if self.label.is_empty() { "INSTRUMENT".to_string() } else { self.label.to_uppercase() };
        self.draw_small.color = color(p.secondary, 1.0);
        self.draw_small.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 9.0), &format!("{label}  {}", self.node));
        let current = format!("{}  {}", self.list.current_label(), if self.open { "close" } else { "switch" });
        self.draw_bold.color = color(p.text_hi, 1.0);
        let w = makepad_widgets::badge::advance(&self.draw_bold, cx, &current);
        self.draw_bold.draw_abs(cx, dvec2(rect.pos.x + rect.size.x - 8.0 - w, rect.pos.y + 8.0), &current);
        if self.open {
            let list = Rect { pos: dvec2(rect.pos.x, rect.pos.y + HEAD), size: dvec2(rect.size.x, self.list_height) };
            self.list.draw(cx, list, &mut self.draw_row, &mut self.draw_text, &mut self.draw_small);
        }
        DrawStep::done()
    }
}
