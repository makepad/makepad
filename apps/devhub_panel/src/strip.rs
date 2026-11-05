//! The landing strip: the land queue as chips, newest last (queued and landing in blue, landed
//! in green with its work commit, failed and conflicted in red, waiting in amber), the last
//! run of each post-land bucket, and the inbox's unseen counts. Hovering a chip shows its
//! detail in a floating tooltip above the strip; a click on the inbox chip opens its unseen
//! entries in a floating list above it (a click anywhere on the strip closes it).

use crate::float::{draw_float, Float};
use crate::fmt::{clock, dur, local_offset_s, short};
use crate::model::Fleet;
use crate::timeline::{mix, moved_by, DrawBar};
use makepad_devhub::land::St;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LandStripBase = #(LandStrip::register_widget(vm))
    mod.widgets.LandStrip = set_type_default() do mod.widgets.LandStripBase{
        width: Fill height: 40
        background: mix(theme.color_bg_app, theme.color_text, 0.035)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.58)
        good: #3fb56a
        warn: #e8a13c
        bad: #e5534b
        accent: theme.color_focus
        tip_bg: mix(theme.color_bg_app, theme.color_text, 0.9)
        tip_ink: theme.color_bg_app
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: theme.color_text text_style: theme.font_bold{font_size: 8.0}}
        draw_tip_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
    }
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct LandStrip {
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
    good: Vec4f,
    #[live]
    warn: Vec4f,
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
    /// the chips of the last draw (the inbox's first, when there is one)
    #[rust]
    chips: Vec<Rect>,
    #[rust]
    inbox_chip: Option<Rect>,
    /// the chip under the pointer, and where the pointer entered it
    #[rust]
    hover: Option<(usize, DVec2)>,
    #[rust]
    inbox_open: bool,
    /// the overlay of the tooltip or the inbox list
    #[rust]
    float: Option<DrawList2d>,
}

/// A box floating above the strip: its top left, width, lines, line height and padding.
type Floating = (DVec2, f64, Vec<(String, Vec4f)>, f64, f64);

impl LandStrip {
    pub fn set_fleet(&mut self, cx: &mut Cx, f: Arc<Fleet>) {
        self.fleet = Some(f);
        self.area.redraw(cx);
    }

    fn width(d: &DrawText, cx: &mut Cx2d, s: &str) -> f64 {
        d.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64
    }

    /// A chip: a tinted rounded box with its text; its width.
    fn chip(&mut self, cx: &mut Cx2d, x: f64, y: f64, s: &str, c: Vec4f, bold: bool) -> (Rect, f64) {
        let d = if bold { &self.draw_bold } else { &self.draw_text };
        let w = Self::width(d, cx, s) + 12.0;
        let r = Rect { pos: dvec2(x, y), size: dvec2(w, 18.0) };
        self.draw_bar.plain(cx, r, vec4(c.x, c.y, c.z, 0.16), vec4(c.x, c.y, c.z, 0.55), 3.0);
        let ink = self.ink;
        let d = if bold { &mut self.draw_bold } else { &mut self.draw_text };
        d.color = ink;
        d.draw_abs(cx, dvec2(x + 6.0, y + 3.5), s);
        (r, w)
    }

    /// Draws the strip; the box floating above it (the inbox list, or a chip's tooltip).
    fn draw_strip(&mut self, cx: &mut Cx2d, f: &Fleet) -> Option<Floating> {
        let v = self.view;
        let edge = self.edge;
        self.draw_shape.color = edge;
        self.draw_shape.draw_abs(cx, Rect { pos: v.pos, size: dvec2(v.size.x, 1.0) });
        let y = v.pos.y + (v.size.y - 18.0) * 0.5;
        let offset = local_offset_s(f.now_ms);
        let mut tips: Vec<(Rect, Vec<String>)> = Vec::new();
        self.inbox_chip = None;
        // right side first: the inbox and the buckets keep their place, the landings take the rest
        let mut rx = v.pos.x + v.size.x - 8.0;
        if let Some(ib) = f.inbox {
            let s = format!("inbox {} err \u{b7} {} warn \u{b7} {} req {}", ib.err, ib.warn, ib.req, if self.inbox_open { "\u{25be}" } else { "\u{25b4}" });
            let c = if ib.err > 0 { self.bad } else if ib.warn > 0 { self.warn } else { self.good };
            let w = Self::width(&self.draw_text, cx, &s) + 12.0;
            rx -= w;
            let (r, _) = self.chip(cx, rx, y, &s, c, false);
            self.inbox_chip = Some(r);
            tips.push((r, vec![format!("{} unseen in the inbox", ib.unseen), "click: its newest entries".into()]));
            rx -= 12.0;
        }
        // the last run of each bucket, in name order
        let mut names: Vec<&str> = f.buckets.iter().map(|b| b.name.as_str()).collect();
        names.sort();
        names.dedup();
        for name in names.iter().rev() {
            let Some(b) = f.buckets.iter().rev().find(|b| b.name == *name) else { continue };
            let s = format!("{} {} {}", b.name, if b.ok { "ok" } else { "FAILED" }, short(&b.tip, 8));
            let c = if b.ok { self.good } else { self.bad };
            let w = Self::width(&self.draw_text, cx, &s) + 12.0;
            rx -= w;
            let (r, _) = self.chip(cx, rx, y, &s, c, false);
            let mut lines = vec![format!("bucket {} \u{b7} {} \u{b7} {}", b.name, if b.ok { "ok" } else { "FAILED" }, b.tip)];
            lines.push(format!("ran {} \u{2192} {} ({})", clock(b.started_ms, offset, true), clock(b.ended_ms, offset, true), dur(b.ended_ms.saturating_sub(b.started_ms))));
            if !b.failing.is_empty() {
                lines.push(format!("failing: {}", b.failing.join(", ")));
            }
            tips.push((r, lines));
            rx -= 6.0;
        }
        let muted = self.muted;
        if !f.buckets.is_empty() {
            let w = Self::width(&self.draw_text, cx, "buckets");
            rx -= w + 4.0;
            self.draw_text.color = muted;
            self.draw_text.draw_abs(cx, dvec2(rx, y + 3.5), "buckets");
            rx -= 14.0;
        }
        // the landings, newest at the right, as many as fit
        let x0 = v.pos.x + 10.0;
        let ink = self.ink;
        self.draw_bold.color = ink;
        self.draw_bold.draw_abs(cx, dvec2(x0, y + 3.0), "LANDING");
        let lx = x0 + Self::width(&self.draw_bold, cx, "LANDING") + 10.0;
        let mut entries: Vec<_> = f.land.iter().collect();
        entries.sort_by_key(|e| e.id);
        let label = |e: &makepad_devhub::land::Entry, now: u64| -> String {
            let sha = e.landed.split_whitespace().nth(1).map(|s| short(s, 7).to_string()).unwrap_or_default();
            let state = match (e.state, entries_head(f) == Some(e.id)) {
                (St::Queued, true) => "landing",
                (s, _) => s.as_str(),
            };
            let age = dur_ago(now.saturating_sub(e.changed_ms));
            if sha.is_empty() { format!("#{} {} {state} {age}", e.id, e.label()) } else { format!("#{} {} {sha} {age}", e.id, e.label()) }
        };
        let mut chips: Vec<(String, Vec4f, usize)> = Vec::new();
        let mut used = 0.0;
        for (i, e) in entries.iter().enumerate().rev() {
            let s = label(e, f.now_ms);
            let w = Self::width(&self.draw_text, cx, &s) + 12.0 + 5.0;
            if lx + used + w > rx {
                break;
            }
            used += w;
            let c = match e.state {
                St::Landed => self.good,
                St::Failed | St::Conflict => self.bad,
                St::Waiting => self.warn,
                St::Queued => self.accent,
            };
            chips.push((s, c, i));
        }
        let mut x = lx;
        for (s, c, i) in chips.iter().rev() {
            let (r, w) = self.chip(cx, x, y, s, *c, false);
            let e = entries[*i];
            let mut lines = vec![format!("#{} {} \u{b7} {}", e.id, e.label(), e.state.as_str())];
            if !e.agent.is_empty() {
                lines.push(format!("agent {}", e.agent));
            }
            if !e.landed.is_empty() {
                lines.push(e.landed.clone());
            }
            if !e.held.is_empty() {
                lines.push(format!("held: {}", e.held));
            }
            lines.push(format!("handed in {} \u{b7} changed {}", clock(e.asked_ms, offset, true), clock(e.changed_ms, offset, true)));
            lines.extend(e.detail.iter().take(6).cloned());
            tips.push((r, lines));
            x += w + 5.0;
        }
        if entries.is_empty() {
            self.draw_text.color = muted;
            self.draw_text.draw_abs(cx, dvec2(lx, y + 3.5), "no hand-ins yet");
        }
        self.chips = tips.iter().map(|(r, _)| *r).collect();
        if self.inbox_open {
            return self.inbox_box(f);
        }
        let (i, at) = self.hover?;
        let (_, lines) = tips.get(i)?;
        Some(self.tip_box(cx, at, lines))
    }

    /// The inbox's unseen entries, newest first, in a floating list above the inbox chip.
    fn inbox_box(&self, f: &Fleet) -> Option<Floating> {
        let chip = self.inbox_chip?;
        let pad = 8.0;
        let lh = 13.0;
        let n = f.inbox_list.len().min(16);
        let mut lines: Vec<(String, Vec4f)> = Vec::with_capacity(n + 2);
        let head = match f.inbox.map(|i| i.unseen) {
            Some(u) if u as usize > n => format!("INBOX \u{b7} the newest {n} of {u} unseen"),
            _ => format!("INBOX \u{b7} {} unseen", f.inbox_list.len()),
        };
        lines.push((head, self.tip_ink));
        for e in f.inbox_list.iter().take(n) {
            let c = match e.sev() {
                "ERR" => self.bad,
                "WARN" => self.warn,
                _ => mix(self.tip_ink, self.tip_bg, 0.2),
            };
            let count = if e.count > 1 { format!(" (\u{d7}{})", e.count) } else { String::new() };
            let text = e.line.splitn(2, ' ').nth(1).unwrap_or(&e.line);
            lines.push((format!("{} ago \u{b7} {text}{count}", dur_ago(f.now_ms.saturating_sub(e.last_ms))), c));
        }
        if f.inbox_list.is_empty() {
            lines.push(("nothing unseen".into(), mix(self.tip_ink, self.tip_bg, 0.3)));
        }
        lines.push(("`makepad devhub show <id>` prints one whole; `seen <id>` clears it".into(), mix(self.tip_ink, self.tip_bg, 0.35)));
        let w = 640.0f64.min(self.view.size.x - 8.0);
        let h = Float::height(lines.len(), lh, pad);
        let x = (chip.pos.x + chip.size.x - w).max(self.view.pos.x + 4.0);
        Some((dvec2(x, self.view.pos.y - h - 6.0), w, lines, lh, pad))
    }

    /// A chip's tooltip, above the strip at the pointer.
    fn tip_box(&self, cx: &mut Cx2d, at: DVec2, lines: &[String]) -> Floating {
        let (pad, lh) = (7.0, 12.5);
        let w = lines.iter().map(|l| Self::width(&self.draw_tip_text, cx, l)).fold(0.0, f64::max);
        let w = (w + pad * 2.0).min(640.0);
        let h = Float::height(lines.len(), lh, pad);
        let (l, r) = (self.view.pos.x + 4.0, self.view.pos.x + self.view.size.x - w - 4.0);
        let x = (at.x - w * 0.5).clamp(l, r.max(l));
        let fg = self.tip_ink;
        (dvec2(x, self.view.pos.y - h - 6.0), w, lines.iter().map(|l| (l.clone(), fg)).collect(), lh, pad)
    }
}

/// The hand-in the train works on now: the first queued one.
fn entries_head(f: &Fleet) -> Option<u64> {
    f.land.iter().filter(|e| e.state == St::Queued).map(|e| e.id).min()
}

fn dur_ago(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h", s / 3600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

impl WidgetNode for LandStrip {
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

impl Widget for LandStrip {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        let float = self.fleet.clone().and_then(|f| self.draw_strip(cx, &f));
        let (bg, shadow) = (self.tip_bg, if self.inbox_open { 0.35 } else { 0.3 });
        let float = float.as_ref().map(|(at, w, lines, line_h, pad)| Float { at: *at, w: *w, lines, line_h: *line_h, pad: *pad, bg, shadow });
        draw_float(cx, &mut self.float, self.area, self.view, float, &mut self.draw_tip, &mut self.draw_tip_text);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let shift = moved_by(cx, self.area, self.view);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let i = self.chips.iter().position(|r| r.contains(e.abs - shift));
                if i != self.hover.map(|(h, _)| h) {
                    self.hover = i.map(|i| (i, e.abs - shift));
                    self.area.redraw(cx);
                }
                if self.inbox_chip.is_some_and(|r| r.contains(e.abs - shift)) {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_over => {
                // the inbox chip opens and closes its list; anywhere else on the strip closes it
                self.inbox_open = !self.inbox_open && self.inbox_chip.is_some_and(|r| r.contains(e.abs - shift));
                self.area.redraw(cx);
            }
            _ => {}
        }
    }
}
