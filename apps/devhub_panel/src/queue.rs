//! The queue panel: the waiting requests in queue order, each with why it waits (held by a
//! lease, the lane cap, a budget), how long it has waited and when it is expected to start.
//! A wait over 10 minutes stands out. The wheel scrolls; hovering a row shows its whole reason
//! in a floating tooltip (redrawn only when the row under the pointer changes).

use crate::fmt::{clock, dur, local_offset_s};
use crate::model::{Fleet, LONG_WAIT_MS};
use crate::float::{draw_float, Float};
use crate::timeline::{mix, moved_by, DrawBar};
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FleetQueueBase = #(FleetQueue::register_widget(vm))
    mod.widgets.FleetQueue = set_type_default() do mod.widgets.FleetQueueBase{
        width: Fill height: Fill
        background: mix(theme.color_bg_app, theme.color_text, 0.025)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.58)
        bad: #e5534b
        accent: theme.color_focus
        tip_bg: mix(theme.color_bg_app, theme.color_text, 0.9)
        tip_ink: theme.color_bg_app
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: theme.color_text text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {color: theme.color_text text_style: theme.font_regular{font_size: 6.8}}
        draw_tip_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
    }
}

const HEAD_H: f64 = 28.0;
const ROW_H: f64 = 38.0;

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct FleetQueue {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_bar: DrawBar,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_tip: DrawBar,
    #[live]
    draw_tip_text: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    bad: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    tip_bg: Vec4f,
    #[live]
    tip_ink: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    view: Rect,
    #[rust]
    fleet: Option<Arc<Fleet>>,
    #[rust]
    scroll: f64,
    /// the row under the pointer, and where the pointer entered it
    #[rust]
    hover: Option<(usize, DVec2)>,
    /// the tooltip's overlay
    #[rust]
    float: Option<DrawList2d>,
}

impl FleetQueue {
    pub fn set_fleet(&mut self, cx: &mut Cx, f: Arc<Fleet>) {
        self.fleet = Some(f);
        self.area.redraw(cx);
    }

    fn text(draw: &mut DrawText, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        if max_w < 10.0 || s.is_empty() {
            return 0.0;
        }
        draw.color = color;
        let width = |d: &DrawText, cx: &mut Cx2d, s: &str| d.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64;
        let w = width(draw, cx, s);
        if w <= max_w {
            draw.draw_abs(cx, dvec2(x, y), s);
            return w;
        }
        // clipped, not cut: a label's layout is made once
        cx.push_clip_rect(Rect { pos: dvec2(x, y - 4.0), size: dvec2(max_w, 24.0) });
        draw.draw_abs(cx, dvec2(x, y), s);
        cx.pop_clip_rect();
        max_w
    }

    fn right_text(draw: &mut DrawText, cx: &mut Cx2d, right: f64, y: f64, s: &str, color: Vec4f) -> f64 {
        draw.color = color;
        let w = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64;
        draw.draw_abs(cx, dvec2(right - w, y), s);
        w
    }

    fn rect(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f) {
        self.draw_shape.color = color;
        self.draw_shape.draw_abs(cx, r);
    }

    /// Draws the queue; the hovered row's tooltip lines and where the pointer is.
    fn draw_queue(&mut self, cx: &mut Cx2d, f: &Fleet) -> Option<(Vec<String>, DVec2)> {
        let v = self.view;
        let (x, w) = (v.pos.x + 10.0, v.size.x - 20.0);
        let q = f.queued();
        let long = q.iter().filter(|r| f.wait_ms(r) >= LONG_WAIT_MS).count();
        let ink = self.ink;
        let tw = Self::text(&mut self.draw_bold, cx, x, v.pos.y + 8.0, 80.0, "QUEUE", ink);
        let oldest = q.iter().map(|r| f.wait_ms(r)).max();
        let line = match (q.len(), oldest) {
            (0, _) => "nothing waits".to_string(),
            (n, Some(o)) => format!("{n} waiting \u{b7} oldest {}{}", dur(o), if long > 0 { format!(" \u{b7} {long} over 10 min") } else { String::new() }),
            (n, None) => format!("{n} waiting"),
        };
        let c = if long > 0 { self.bad } else { self.muted };
        Self::text(&mut self.draw_text, cx, x + tw + 8.0, v.pos.y + 9.0, w - tw - 8.0, &line, c);
        let edge = self.edge;
        self.rect(cx, Rect { pos: dvec2(v.pos.x, v.pos.y + HEAD_H - 1.0), size: dvec2(v.size.x, 1.0) }, edge);
        let clip = Rect { pos: dvec2(v.pos.x, v.pos.y + HEAD_H), size: dvec2(v.size.x, (v.size.y - HEAD_H).max(0.0)) };
        let room = clip.size.y;
        self.scroll = self.scroll.clamp(0.0, (q.len() as f64 * ROW_H - room + 6.0).max(0.0));
        cx.push_clip_rect(clip);
        let offset = local_offset_s(f.now_ms);
        let mut hover: Option<(Vec<String>, DVec2)> = None;
        for (i, r) in q.iter().enumerate() {
            let y = clip.pos.y + i as f64 * ROW_H - self.scroll;
            if y + ROW_H < clip.pos.y || y > clip.pos.y + room {
                continue;
            }
            let wait = f.wait_ms(r);
            let over = wait >= LONG_WAIT_MS;
            let row = Rect { pos: dvec2(v.pos.x + 4.0, y + 2.0), size: dvec2(v.size.x - 8.0, ROW_H - 4.0) };
            let hovered = self.hover.filter(|(h, _)| *h == i);
            if over {
                let b = self.bad;
                self.draw_bar.plain(cx, row, vec4(b.x, b.y, b.z, 0.1), vec4(b.x, b.y, b.z, 0.45), 3.0);
            } else if hovered.is_some() {
                let a = self.accent;
                self.draw_bar.plain(cx, row, vec4(a.x, a.y, a.z, 0.08), vec4(0.0, 0.0, 0.0, 0.0), 3.0);
            }
            let (ink, muted) = (self.ink, self.muted);
            let wc = if over { self.bad } else { ink };
            let ww = Self::right_text(&mut self.draw_bold, cx, x + w, y + 5.0, &dur(wait), wc);
            let head = format!("{} \u{b7} {} {}{}", r.lane, r.kind, r.id, if r.priority == "interactive" { " \u{b7} interactive" } else { "" });
            Self::text(&mut self.draw_text, cx, x, y + 6.0, w - ww - 8.0, &head, ink);
            let reason = if r.reason.is_empty() { "waiting".to_string() } else { r.reason.clone() };
            Self::text(&mut self.draw_small, cx, x, y + 17.0, w, &reason, muted);
            let mut tail = format!("asked {}", clock(r.asked_ms, offset, false));
            if !r.agent.is_empty() {
                tail.push_str(&format!(" \u{b7} {}", r.agent));
            }
            tail.push_str(&format!(" \u{b7} {} {}", r.cost, r.kind));
            tail.push_str(&if r.eta_ms > 0 { format!(" \u{b7} expected in {}", dur(r.eta_ms)) } else { " \u{b7} start not known".to_string() });
            Self::text(&mut self.draw_small, cx, x, y + 27.0, w, &tail, muted);
            if let Some((_, at)) = hovered {
                let mut lines = vec![format!("{} \u{b7} {} \u{b7} {}", r.id, r.kind, r.lane)];
                lines.push(format!("waiting {} \u{b7} {}", dur(wait), reason));
                let on = f.waited_on(r);
                if !on.is_empty() {
                    lines.push(format!("waited on {on}"));
                }
                lines.push(format!("cost {} ({}) \u{b7} {:.0} cores \u{b7} {:.0} GB", r.cost, r.cost_source, r.cores, r.ram_gb));
                hover = Some((lines, at));
            }
        }
        cx.pop_clip_rect();
        hover
    }

    fn row_at(&self, p: DVec2) -> Option<usize> {
        let v = self.view;
        if !v.contains(p) || p.y < v.pos.y + HEAD_H {
            return None;
        }
        Some(((p.y - v.pos.y - HEAD_H + self.scroll) / ROW_H) as usize)
    }
}

impl WidgetNode for FleetQueue {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn children(&self, _visit: &mut dyn FnMut(LiveId, WidgetRef)) {}
    fn find_widgets_from_point(&self, _cx: &Cx, _p: DVec2, _found: &mut dyn FnMut(&WidgetRef)) {}
}

impl Widget for FleetQueue {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        let hover = self.fleet.clone().and_then(|f| self.draw_queue(cx, &f));
        // the hovered row's tooltip: under the pointer, its right edge on the column's (it
        // reaches left over the timeline)
        let (fg, bg) = (self.tip_ink, self.tip_bg);
        let tip = hover.map(|(lines, at)| {
            let lines: Vec<(String, Vec4f)> = lines.into_iter().enumerate().map(|(i, l)| (l, if i == 0 { fg } else { mix(fg, bg, 0.18) })).collect();
            let w = lines.iter().map(|(l, _)| self.draw_tip_text.layout(cx, 0.0, 0.0, None, false, Align::default(), l).size_in_lpxs.width as f64).fold(0.0, f64::max);
            let w = (w + 14.0).min(560.0);
            (dvec2(view.pos.x + view.size.x - w - 6.0, at.y + 14.0), w, lines)
        });
        let float = tip.as_ref().map(|(at, w, lines)| Float { at: *at, w: *w, lines, line_h: 12.5, pad: 7.0, bg, shadow: 0.3 });
        draw_float(cx, &mut self.float, self.area, self.view, float, &mut self.draw_tip, &mut self.draw_tip_text);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let shift = moved_by(cx, self.area, self.view);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let i = self.row_at(e.abs - shift);
                if i != self.hover.map(|(h, _)| h) {
                    self.hover = i.map(|i| (i, e.abs - shift));
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(e) => {
                self.scroll = (self.scroll + e.scroll.y).max(0.0);
                self.hover = None;
                self.area.redraw(cx);
            }
            _ => {}
        }
    }
}
