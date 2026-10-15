//! Two tiny status-bar charts drawn straight from data: the context heat
//! strip (share of the window by kind, the summarised part marked) and the
//! token-rate sparkline. Neither schedules frames: they redraw when the
//! window redraws for an event.

use makepad_code_agent::event::{ContextSegment, SegmentKind};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HeatStripBase = #(HeatStrip::register_widget(vm))
    mod.widgets.HeatStrip = set_type_default() do mod.widgets.HeatStripBase{
        width: 140
        height: 6
        draw_bg +: { color: mod.look.hair_hi }
    }

    mod.widgets.SparklineBase = #(Sparkline::register_widget(vm))
    mod.widgets.Sparkline = set_type_default() do mod.widgets.SparklineBase{
        width: 96
        height: 14
        draw_bg +: { color: mod.look.agent }
    }
}

/// The ink of a context segment kind.
pub fn segment_color(kind: SegmentKind) -> Vec4f {
    use super::look::{alpha, ink};
    match kind {
        SegmentKind::System => ink::DIM,
        SegmentKind::Project => alpha(ink::AGENT, 0.45),
        SegmentKind::Summary => ink::CHANGE,
        SegmentKind::User => ink::HUMAN,
        SegmentKind::Assistant => ink::AGENT,
        SegmentKind::ToolResult => ink::LANE,
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct HeatStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[rust]
    segments: Vec<ContextSegment>,
    #[rust]
    max: u32,
}

impl HeatStrip {
    /// Called while the status bar is being drawn, just before this widget.
    pub fn set(&mut self, segments: &[ContextSegment], max: u32) {
        self.segments.clear();
        self.segments.extend_from_slice(segments);
        self.max = max;
    }
}

impl Widget for HeatStrip {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let track = self.draw_bg.color;
        self.draw_bg.color = track;
        self.draw_bg.draw_abs(cx, rect);
        if self.max > 0 {
            let mut x = rect.pos.x;
            let per = rect.size.x / self.max as f64;
            for seg in &self.segments {
                let w = (seg.tokens as f64 * per).min(rect.pos.x + rect.size.x - x);
                if w <= 0.0 {
                    continue;
                }
                self.draw_bg.color = segment_color(seg.kind);
                let marked = seg.kind == SegmentKind::Summary;
                // The summarised region reaches above the strip as well.
                let (y, h) = if marked { (rect.pos.y - 2.0, rect.size.y + 2.0) } else { (rect.pos.y, rect.size.y) };
                self.draw_bg.draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2((w - 1.0).max(1.0), h) });
                x += w;
            }
        }
        self.draw_bg.color = track;
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Sparkline {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[rust]
    values: Vec<f32>,
}

impl Sparkline {
    pub fn set(&mut self, values: &[f32]) {
        if self.values != values {
            self.values.clear();
            self.values.extend_from_slice(values);
        }
    }
}

impl Widget for Sparkline {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let ink = self.draw_bg.color;
        let n = self.values.len();
        if n > 0 {
            let peak = self.values.iter().cloned().fold(10.0f32, f32::max) as f64;
            let slot = rect.size.x / n.max(24) as f64;
            let first = rect.pos.x + rect.size.x - slot * n as f64;
            for (i, v) in self.values.iter().enumerate() {
                let h = ((*v as f64 / peak) * rect.size.y).clamp(1.0, rect.size.y);
                let age = (i + 1) as f32 / n as f32;
                self.draw_bg.color = vec4(ink.x, ink.y, ink.z, 0.25 + 0.75 * age);
                self.draw_bg.draw_abs(
                    cx,
                    Rect { pos: dvec2(first + slot * i as f64, rect.pos.y + rect.size.y - h), size: dvec2((slot - 1.0).max(1.0), h) },
                );
            }
        }
        self.draw_bg.color = ink;
        DrawStep::done()
    }
}
