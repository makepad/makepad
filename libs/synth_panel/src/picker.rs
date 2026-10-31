//! `SynthPicker{node: "mono"}`: the instrument switch. It shows what the
//! node is (a synth preset, an audio shader, a drum kit, a General MIDI
//! program, the piano …) and opens the instrument browser under it
//! ([`crate::browser`], the one every picker uses): every engine's
//! presets by category, searchable; resting on a row plays the node as
//! that instrument in place, a click makes it so (the node rewritten,
//! keeping its name, so every loop that plays it keeps its notes; one undo
//! step) and the browser stays open for the next.

use crate::host as synth;
use crate::browser::Browser;
use makepad_value_pickers::panel::control::{DrawPanelShape, R_GROUP};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthPickerBase = #(SynthPicker::register_widget(vm))
    /** The instrument a node is, and what it could be instead (previewed, switched). */
    mod.widgets.SynthPicker = set_type_default() do mod.widgets.SynthPickerBase{
        width: Fill height: Fit list_height: 420
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
    #[live]
    browser: Browser,
    #[rust]
    head: Rect,
    #[rust]
    next_frame: NextFrame,
}

impl Widget for SynthPicker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.browser.tick() {
            self.next_frame = cx.new_next_frame();
        }
        // The open browser has its own events (the keys while it has them).
        if self.open {
            let keys = matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_));
            if !keys || cx.has_key_focus(self.draw_bg.area()) {
                let out = self.browser.handle_event(cx, event);
                if out.close {
                    self.open = false;
                    synth::set_searching(false);
                }
                if out.redraw {
                    self.redraw(cx);
                    self.next_frame = cx.new_next_frame();
                }
                if out.consumed {
                    if matches!(event, Event::MouseDown(_)) {
                        cx.set_key_focus(self.draw_bg.area());
                    }
                    return;
                }
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::KeyFocus(_) => synth::set_searching(self.open),
            Hit::KeyFocusLost(_) => synth::set_searching(false),
            Hit::FingerUp(fe) if fe.is_over && fe.was_tap() && self.head.contains(fe.abs) => {
                synth::select(&self.node);
                self.open = !self.open;
                if self.open {
                    self.browser.inline = true;
                    self.browser.open(&self.node);
                    cx.set_key_focus(self.draw_bg.area());
                } else {
                    self.browser.close();
                }
                synth::set_searching(self.open);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
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
        let now = synth::current(&self.node).map(|k| k.split_once(':').map_or(k.clone(), |(t, f)| format!("{t} {}", f.split(',').filter_map(|x| x.split_once('=')).map(|(_, v)| v.trim_start_matches('@').trim_matches('"').replace('_', " ")).collect::<Vec<_>>().join(" ")))).unwrap_or_default();
        let current = format!("{now}  {}", if self.open { "close" } else { "browse" });
        self.draw_bold.color = color(p.text_hi, 1.0);
        let w = makepad_widgets::badge::advance(&self.draw_bold, cx, &current);
        self.draw_bold.draw_abs(cx, dvec2(rect.pos.x + rect.size.x - 8.0 - w, rect.pos.y + 8.0), &current);
        if self.open {
            let list = Rect { pos: dvec2(rect.pos.x, rect.pos.y + HEAD), size: dvec2(rect.size.x, self.list_height) };
            if self.browser.draw(cx, list) {
                self.next_frame = cx.new_next_frame();
            }
        }
        DrawStep::done()
    }
}
