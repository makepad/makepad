//! `SynthKeys{node: "mono"}`: a keyboard that plays the instrument as it is
//! now (its held values too), to hear a change without the whole score.
//! A press plays the note until it is let go; dragging across plays each
//! key it reaches. What it plays goes where the desk sends previews (the
//! cue or, when asked, the main output), never into the score.

use crate::host as synth;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthKeysBase = #(SynthKeys::register_widget(vm))
    /** A keyboard playing an instrument as it is now. */
    mod.widgets.SynthKeys = set_type_default() do mod.widgets.SynthKeysBase{
        width: Fill height: 64 octave: 3 octaves: 2
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 7}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// Which of the twelve are black, and where a black key sits between its
/// white neighbours (in white-key widths from the octave's left edge).
const BLACK: [Option<f64>; 12] = [None, Some(0.65), None, Some(1.75), None, None, Some(3.6), None, Some(4.7), None, Some(5.75), None];

#[derive(Script, ScriptHook, Widget)]
pub struct SynthKeys {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_key: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    node: String,
    /// The lowest octave (C3 is MIDI 48).
    #[live]
    octave: f64,
    #[live]
    octaves: f64,
    /// Each key's note and rectangle, black keys first (they are on top).
    #[rust]
    keys: Vec<(f32, Rect)>,
    #[rust]
    down: Option<f32>,
}

impl SynthKeys {
    fn note_at(&self, abs: DVec2) -> Option<f32> {
        self.keys.iter().find(|(_, r)| r.contains(abs)).map(|(n, _)| *n)
    }

    fn press(&mut self, cx: &mut Cx, note: Option<f32>) {
        if note == self.down {
            return;
        }
        if let Some(old) = self.down.take() {
            synth::key(&self.node, old, false);
        }
        if let Some(note) = note {
            synth::key(&self.node, note, true);
            self.down = Some(note);
        }
        self.redraw(cx);
    }
}

impl Widget for SynthKeys {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                synth::select(&self.node);
                let note = self.note_at(fe.abs);
                self.press(cx, note);
            }
            Hit::FingerMove(fe) if self.down.is_some() => {
                let note = self.note_at(fe.abs).or(self.down);
                self.press(cx, note);
            }
            Hit::FingerUp(_) => self.press(cx, None),
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        self.draw_bg.color = color(p.well, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        let octaves = self.octaves.round().clamp(1.0, 6.0) as usize;
        let base = 12.0 * (self.octave.round().clamp(0.0, 8.0) as f32 + 1.0);
        let whites = octaves * 7 + 1;
        let w = rect.size.x / whites as f64;
        self.keys.clear();
        let mut whites_drawn = Vec::new();
        let mut k = 0usize;
        for o in 0..=octaves {
            for (i, black) in BLACK.iter().enumerate() {
                if o == octaves && i > 0 {
                    break;
                }
                let note = base + (o * 12 + i) as f32;
                match black {
                    Some(at) => {
                        let x = rect.pos.x + (o as f64 * 7.0 + at) * w;
                        self.keys.push((note, Rect { pos: dvec2(x, rect.pos.y), size: dvec2(w * 0.6, rect.size.y * 0.6) }));
                    }
                    None => {
                        whites_drawn.push((note, Rect { pos: dvec2(rect.pos.x + k as f64 * w, rect.pos.y), size: dvec2(w - 1.0, rect.size.y) }));
                        k += 1;
                    }
                }
            }
        }
        // Whites under blacks; hits look at blacks first.
        for (note, r) in &whites_drawn {
            self.draw_key.color = if self.down == Some(*note) { color(p.accent, 1.0) } else { color(p.text_hi, 0.88) };
            self.draw_key.draw_abs(cx, *r);
            if (*note as i32) % 12 == 0 {
                self.draw_text.color = color(p.panel, 0.8);
                self.draw_text.draw_abs(cx, dvec2(r.pos.x + 2.0, r.pos.y + r.size.y - 11.0), &format!("C{}", (*note as i32) / 12 - 1));
            }
        }
        for (note, r) in &self.keys {
            self.draw_key.color = if self.down == Some(*note) { color(p.accent, 1.0) } else { color(p.panel, 1.0) };
            self.draw_key.draw_abs(cx, *r);
        }
        self.keys.extend(whites_drawn);
        DrawStep::done()
    }
}
