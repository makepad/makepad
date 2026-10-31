//! The body of a stream node's card (`MidiIn`, `Arp`, `Chord`, `Transpose`,
//! `Velocity`): read from the schema-driven [`StreamCard`] the lens built,
//! and the live state its loops publish (`feed::midi_in::activity`,
//! `feed::streams`), read as it paints.
//!
//! - MidiIn: a device chip (a click steps to the next input: any, then each
//!   port the platform lists), the channel, an activity light and the last
//!   notes in.
//! - Arp: a pattern chip (a click steps to the next pattern) and the step
//!   view: the steps of the chord held (the one playing lit), else the
//!   pattern on a C major triad.

use super::lens::{Lens, StreamCard};
use super::paint::{GAP, HEAD, KNOB_W};
use super::{Hot, ScoreEditor};
use makepad_audio_engine::feed::{midi_in, streams};
use makepad_audio_synth::kit::arp::PATTERNS;
use makepad_look::palette::Palette;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// How long the activity light stays lit after a message (s).
const LIT: f64 = 0.15;

fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

fn alpha(c: u32, a: f32) -> Vec4f {
    let mut v = Vec4f::from_u32(c);
    v.w = a;
    v
}

fn note_name(n: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[n as usize % 12], n as i32 / 12 - 1)
}

impl ScoreEditor {
    /// A chip that writes `value` into `field` when clicked; its rect.
    fn set_chip(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, at: DVec2, text: &str, field: &'static str, value: String) -> Rect {
        let w = advance(&self.draw_label, cx, text) + 14.0;
        let rect = Rect { pos: at, size: dvec2(w, 16.0) };
        let under = self.mouse.is_some_and(|m| rect.contains(m));
        self.box_(cx, rect, rgba(if under { p.control_hover } else { p.control }), rgba(p.line), 3.0);
        self.draw_label.color = rgba(p.text);
        self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 7.0, rect.pos.y + 2.5), text);
        self.hots.push((rect, Hot::Set { node: lens.node.clone(), field, value }));
        rect
    }

    /// The card's body: what it shows, then its knobs; true while it
    /// animates (an activity light, an arp stepping).
    pub(super) fn paint_stream(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, card: &StreamCard, body: Rect) -> bool {
        let knobs_w = lens.knobs.len() as f64 * KNOB_W;
        let kx = body.pos.x + body.size.x - knobs_w;
        let w = (kx - GAP - body.pos.x).max(20.0);
        self.module_head(cx, p, &lens.ty.to_uppercase(), body.pos.x, body.pos.y, w, body.size.y, true, "");
        let animates = self.paint_stream_view(cx, p, lens, card, Rect { pos: dvec2(body.pos.x, body.pos.y + HEAD), size: dvec2(w, body.size.y - HEAD) });
        if !lens.knobs.is_empty() {
            self.module_head(cx, p, "SETTINGS", kx, body.pos.y, knobs_w, body.size.y, false, "");
            for (i, knob) in lens.knobs.iter().enumerate() {
                self.paint_knob(cx, p, &lens.node, lens.hue, knob, dvec2(kx + i as f64 * KNOB_W, body.pos.y + HEAD), body.size.y - HEAD, &lens.knobs);
            }
        }
        animates
    }

    /// What a card shows besides its knobs, in `body`.
    fn paint_stream_view(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, card: &StreamCard, body: Rect) -> bool {
        let x = body.pos.x;
        let mid = body.pos.y + body.size.y * 0.5;
        match card {
            StreamCard::MidiIn { device, channel } => {
                let ports = midi_in::ports();
                // The next device a click picks: any, then each input.
                let choices: Vec<String> = std::iter::once("any".to_string()).chain(ports.iter().cloned()).collect();
                let at = choices.iter().position(|c| c.eq_ignore_ascii_case(device)).unwrap_or(0);
                let next = choices[(at + 1) % choices.len()].clone();
                let shown: String = device.chars().take(18).collect();
                let chip = self.set_chip(cx, p, lens, dvec2(x, mid - 18.0), &shown, "device", format!("\"{next}\""));
                let found = midi_in::matches(device, "") || ports.iter().any(|n| midi_in::matches(device, n));
                let activity = midi_in::activity(device, *channel);
                let now = makepad_audio_engine::clock::now();
                let lit = now - activity.last < LIT;
                let light = dvec2(chip.pos.x + chip.size.x + 10.0, mid - 10.0);
                self.draw_shape.fill = if lit { rgba(p.live) } else if found { rgba(p.edge_lo) } else { alpha(p.hot, 0.6) };
                self.draw_shape.shape = 2.0;
                self.draw_shape.draw_abs(cx, Rect { pos: light - dvec2(4.0, 4.0), size: dvec2(8.0, 8.0) });
                let ch = if *channel == 0 { "all channels".to_string() } else { format!("ch {channel}") };
                self.draw_label.color = rgba(if found { p.muted } else { p.hot });
                let status = if found { ch } else { "not connected".to_string() };
                self.draw_label.draw_abs(cx, dvec2(light.x + 10.0, mid - 16.0), &status);
                // The last notes in, newest last; the held ones bright.
                let mut nx = x;
                for (note, _) in activity.recent.iter().rev().take(5).rev() {
                    let held = activity.down.contains(note);
                    let text = note_name(*note);
                    self.draw_label.color = rgba(if held { p.text_bright } else { p.text_soft });
                    self.draw_label.draw_abs(cx, dvec2(nx, mid + 4.0), &text);
                    nx += advance(&self.draw_label, cx, &text) + 6.0;
                    if nx > body.pos.x + body.size.x - 20.0 {
                        break;
                    }
                }
                lit || !activity.down.is_empty()
            }
            StreamCard::Arp { pattern, preview, views } => {
                let name = PATTERNS.get(*pattern).copied().unwrap_or("up");
                let next = PATTERNS[(*pattern + 1) % PATTERNS.len()];
                self.set_chip(cx, p, lens, dvec2(x, mid - 18.0), name, "pattern", format!("@{next}"));
                // The held chord's steps on the first loop playing it.
                let live = views.iter().map(|v| streams::view(v).snapshot()).find(|s| !s.steps.is_empty());
                let (steps, step) = match &live {
                    Some(s) => (s.steps.clone(), Some(s.step)),
                    None => (preview.clone(), None),
                };
                let (lo, hi) = (steps.iter().copied().min().unwrap_or(60), steps.iter().copied().max().unwrap_or(72));
                let span = (hi as f64 - lo as f64).max(1.0);
                let n = steps.len().max(1);
                let w = ((body.size.x - 4.0) / n as f64).clamp(3.0, 14.0);
                let (top, h) = (mid + 0.0, 16.0);
                for (i, note) in steps.iter().enumerate().take(((body.size.x - 4.0) / w) as usize) {
                    let level = (*note as f64 - lo as f64) / span;
                    let bar = 4.0 + level * (h - 4.0);
                    let on = step == Some(i);
                    let rect = Rect { pos: dvec2(x + i as f64 * w, top + h - bar), size: dvec2((w - 2.0).max(1.0), bar) };
                    self.box_(cx, rect, if on { rgba(p.live) } else { alpha(lens.hue, if live.is_some() { 0.7 } else { 0.35 }) }, alpha(0, 0.0), 1.0);
                }
                live.is_some()
            }
            StreamCard::Shape { label } => {
                self.draw_label.color = rgba(p.text_soft);
                self.draw_label.draw_abs(cx, dvec2(x, mid - 6.0), label);
                false
            }
        }
    }
}
