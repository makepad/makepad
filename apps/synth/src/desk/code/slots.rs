//! The Loop lens's hit-pattern resolution (SYNTHTRACK owns this behaviour;
//! the card's look is the lens design's): ÷ and × beside the step pills
//! split the pattern into fewer or more slots over the same length, through
//! `tracker_model::resample_hits` (every hit keeps its time; a coarser
//! split that would move a hit is refused and says why). One splice, one
//! undo step.

use super::{Hot, ScoreEditor, ScoreEditorAction};
use crate::desk::tracker_model as tm;
use makepad_look::palette::Palette;
use makepad_widgets::*;

impl ScoreEditor {
    /// The slot control at the right of a pattern's pill strip; the strip
    /// left for the pills.
    pub(super) fn paint_slots(&mut self, cx: &mut Cx2d, p: &Palette, node: &str, slots: usize, strip: Rect) -> Rect {
        let w = 64.0;
        let x = strip.pos.x + strip.size.x - w;
        let mid = strip.pos.y + strip.size.y * 0.5;
        let fewer = tm::SLOTS.iter().rev().find(|s| **s < slots).copied();
        let more = tm::SLOTS.iter().find(|s| **s > slots).copied();
        let minus = self.chip(cx, p, dvec2(x, mid + 1.0), "÷", false, p.accent);
        if let Some(s) = fewer {
            self.hots.push((minus, Hot::Slots { node: node.to_string(), slots: s }));
        }
        let plus = self.chip(cx, p, dvec2(x + minus.size.x + 2.0, mid + 1.0), "×", false, p.accent);
        if let Some(s) = more {
            self.hots.push((plus, Hot::Slots { node: node.to_string(), slots: s }));
        }
        let text = format!("{slots} slots");
        self.draw_label.color = Vec4f::from_u32(p.text_soft);
        self.draw_label.draw_abs(cx, dvec2(x + 1.0, mid - 15.0), &text);
        Rect { pos: strip.pos, size: dvec2((strip.size.x - w - 4.0).max(40.0), strip.size.y) }
    }

    /// Split `node`'s pattern into `slots`.
    pub(super) fn set_slots(&mut self, cx: &mut Cx, node: &str, slots: usize, gesture: u64) {
        match tm::resample_hits(&self.source, node, slots) {
            Ok(s) => self.splice(cx, s.start, s.end, s.text, gesture, true),
            Err(e) => cx.widget_action(self.widget_uid(), ScoreEditorAction::Status(e)),
        }
    }
}
