//! "Needs a look": the fleet's stuck things worst first (see [`crate::needs`]), one compact row
//! each: a severity stripe, its tag, who and where, for how long, and what is wrong in numbers.
//! A click picks one (the panel frames its lease on the timeline and opens its detail); the
//! wheel scrolls past the first rows. Hovering a row shows it whole in a floating tooltip.

use crate::float::{draw_float, Float};
use crate::fmt::dur_short;
use crate::needs::{Need, Sev};
use crate::timeline::{mix, moved_by, DrawBar};
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.NeedsListBase = #(NeedsList::register_widget(vm))
    mod.widgets.NeedsList = set_type_default() do mod.widgets.NeedsListBase{
        width: Fill height: Fit
        background: mix(theme.color_bg_app, theme.color_text, 0.025)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.58)
        accent: theme.color_focus
        good: #3fb56a
        warn: #e8a13c
        bad: #e5534b
        tip_bg: mix(theme.color_bg_app, theme.color_text, 0.9)
        tip_ink: theme.color_bg_app
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: theme.color_text text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {color: theme.color_text text_style: theme.font_regular{font_size: 6.8}}
        draw_tag +: {color: theme.color_text text_style: theme.font_bold{font_size: 6.2}}
        draw_tip_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
    }
}

const HEAD_H: f64 = 26.0;
const ROW_H: f64 = 30.0;
/// rows shown before the list scrolls
const MAX_ROWS: usize = 7;

#[derive(Clone, Debug, Default)]
pub enum NeedsListAction {
    Picked(Need),
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct NeedsList {
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
    draw_tag: DrawText,
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
    accent: Vec4f,
    #[live]
    good: Vec4f,
    #[live]
    warn: Vec4f,
    #[live]
    bad: Vec4f,
    #[live]
    tip_bg: Vec4f,
    #[live]
    tip_ink: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    view: Rect,
    #[rust]
    needs: Arc<Vec<Need>>,
    #[rust]
    live: bool,
    /// the picked need's key
    #[rust]
    selected: Option<String>,
    /// the row under the pointer, and where the pointer entered it
    #[rust]
    hover: Option<(usize, DVec2)>,
    #[rust]
    scroll: f64,
    /// the tooltip's overlay
    #[rust]
    float: Option<DrawList2d>,
}

impl NeedsList {
    /// The needs of the newest fleet (`live`: the panel has answers to judge by).
    pub fn set_needs(&mut self, cx: &mut Cx, needs: Arc<Vec<Need>>, live: bool) {
        self.needs = needs;
        self.live = live;
        self.area.redraw(cx);
    }

    pub fn select_need(&mut self, cx: &mut Cx, key: Option<String>) {
        if key != self.selected {
            self.selected = key;
            self.area.redraw(cx);
        }
    }

    fn sev_color(&self, s: Sev) -> Vec4f {
        match s {
            Sev::Red => self.bad,
            Sev::Amber => self.warn,
            Sev::Info => self.muted,
        }
    }

    fn width(d: &DrawText, cx: &mut Cx2d, s: &str) -> f64 {
        d.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64
    }

    /// `s` at (x, y), clipped to `max_w`; its width.
    fn text(d: &mut DrawText, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        if max_w < 6.0 || s.is_empty() {
            return 0.0;
        }
        d.color = color;
        let w = Self::width(d, cx, s);
        if w <= max_w {
            d.draw_abs(cx, dvec2(x, y), s);
            return w;
        }
        cx.push_clip_rect(Rect { pos: dvec2(x, y - 4.0), size: dvec2(max_w, 24.0) });
        d.draw_abs(cx, dvec2(x, y), s);
        cx.pop_clip_rect();
        max_w
    }

    fn rows_shown(&self) -> usize {
        self.needs.len().clamp(1, MAX_ROWS)
    }

    fn row_at(&self, p: DVec2) -> Option<usize> {
        let v = self.view;
        if !v.contains(p) || p.y < v.pos.y + HEAD_H {
            return None;
        }
        let i = ((p.y - v.pos.y - HEAD_H + self.scroll) / ROW_H) as usize;
        (i < self.needs.len()).then_some(i)
    }

    fn draw_list(&mut self, cx: &mut Cx2d) {
        let v = self.view;
        let (x, w) = (v.pos.x + 10.0, v.size.x - 20.0);
        let needs = self.needs.clone();
        let (ink, muted) = (self.ink, self.muted);
        let tw = Self::text(&mut self.draw_bold, cx, x, v.pos.y + 8.0, 140.0, "NEEDS A LOOK", ink);
        let red = needs.iter().filter(|n| n.sev == Sev::Red).count();
        let amber = needs.iter().filter(|n| n.sev == Sev::Amber).count();
        let summary = if !self.live {
            String::new()
        } else if needs.is_empty() {
            "nothing stuck".to_string()
        } else {
            let mut s = Vec::new();
            if red > 0 {
                s.push(format!("{red} red"));
            }
            if amber > 0 {
                s.push(format!("{amber} amber"));
            }
            if needs.len() > red + amber {
                s.push(format!("{} to know", needs.len() - red - amber));
            }
            s.join(" \u{b7} ")
        };
        let c = if red > 0 { self.bad } else if amber > 0 { self.warn } else { self.good };
        Self::text(&mut self.draw_text, cx, x + tw + 8.0, v.pos.y + 9.0, w - tw - 8.0, &summary, if needs.is_empty() { muted } else { c });
        let edge = self.edge;
        self.draw_shape.color = edge;
        self.draw_shape.draw_abs(cx, Rect { pos: dvec2(v.pos.x, v.pos.y + v.size.y - 1.0), size: dvec2(v.size.x, 1.0) });
        if needs.is_empty() {
            let line = if self.live { "every lease, queue, node and landing looks fine" } else { "waiting for the master\u{2026}" };
            Self::text(&mut self.draw_small, cx, x, v.pos.y + HEAD_H + 4.0, w, line, muted);
            return;
        }
        let clip = Rect { pos: dvec2(v.pos.x, v.pos.y + HEAD_H), size: dvec2(v.size.x, v.size.y - HEAD_H - 1.0) };
        self.scroll = self.scroll.clamp(0.0, (needs.len() as f64 * ROW_H - clip.size.y).max(0.0));
        cx.push_clip_rect(clip);
        for (i, n) in needs.iter().enumerate() {
            let y = clip.pos.y + i as f64 * ROW_H - self.scroll;
            if y + ROW_H < clip.pos.y || y > clip.pos.y + clip.size.y {
                continue;
            }
            let row = Rect { pos: dvec2(v.pos.x + 4.0, y + 1.0), size: dvec2(v.size.x - 8.0, ROW_H - 2.0) };
            let selected = self.selected.as_deref() == Some(n.key().as_str());
            let sc = self.sev_color(n.sev);
            if selected {
                let a = self.accent;
                self.draw_bar.plain(cx, row, vec4(a.x, a.y, a.z, 0.13), vec4(a.x, a.y, a.z, 0.8), 3.0);
            } else if self.hover.is_some_and(|(h, _)| h == i) {
                let a = self.accent;
                self.draw_bar.plain(cx, row, vec4(a.x, a.y, a.z, 0.07), vec4(0.0, 0.0, 0.0, 0.0), 3.0);
            }
            // the severity stripe (red glows)
            let stripe = Rect { pos: dvec2(v.pos.x + 7.0, y + 5.0), size: dvec2(3.0, ROW_H - 10.0) };
            if n.sev == Sev::Red {
                self.draw_bar.glowing(cx, stripe, sc, vec4(0.0, 0.0, 0.0, 0.0), vec4(sc.x, sc.y, sc.z, 0.55), 3.5, 1.5);
            } else {
                self.draw_bar.plain(cx, stripe, sc, vec4(0.0, 0.0, 0.0, 0.0), 1.5);
            }
            let tx = v.pos.x + 16.0;
            let tag = n.what.tag();
            let tag_w = Self::width(&self.draw_tag, cx, tag) + 8.0;
            self.draw_bar.plain(cx, Rect { pos: dvec2(tx, y + 4.0), size: dvec2(tag_w, 11.0) }, vec4(sc.x, sc.y, sc.z, 0.16), vec4(sc.x, sc.y, sc.z, 0.55), 2.5);
            Self::text(&mut self.draw_tag, cx, tx + 4.0, y + 5.0, tag_w, tag, sc);
            let right = v.pos.x + v.size.x - 10.0;
            let d = if n.for_ms > 0 { dur_short(n.for_ms) } else { String::new() };
            let dw = Self::width(&self.draw_small, cx, &d);
            Self::text(&mut self.draw_small, cx, right - dw, y + 5.0, dw + 1.0, &d, muted);
            Self::text(&mut self.draw_text, cx, tx + tag_w + 6.0, y + 4.0, right - dw - 8.0 - (tx + tag_w + 6.0), &n.title, ink);
            Self::text(&mut self.draw_small, cx, tx, y + 17.0, right - tx, &n.detail, muted);
        }
        cx.pop_clip_rect();
    }

    /// The hovered need's tooltip: under the pointer, its right edge on the column's (it
    /// reaches left over the timeline); its top left, width and lines.
    fn hover_tip(&self, cx: &mut Cx2d) -> Option<(DVec2, f64, Vec<(String, Vec4f)>)> {
        let (i, at) = self.hover?;
        let n = self.needs.get(i)?;
        let (fg, bg) = (self.tip_ink, self.tip_bg);
        let mut lines = vec![(format!("{} \u{b7} {}", n.what.tag(), n.title), fg)];
        if !n.detail.is_empty() {
            lines.push((n.detail.clone(), mix(fg, bg, 0.18)));
        }
        let hint = if n.lease.is_some() { "click: frame its lease and open its detail" } else { "click: open its detail" };
        lines.push((hint.into(), mix(fg, bg, 0.18)));
        let w = lines.iter().map(|(l, _)| Self::width(&self.draw_tip_text, cx, l)).fold(0.0, f64::max);
        let w = (w + 14.0).min(560.0);
        Some((dvec2(self.view.pos.x + self.view.size.x - w - 6.0, at.y + 14.0), w, lines))
    }
}

impl WidgetNode for NeedsList {
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

impl Widget for NeedsList {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // as tall as its rows, up to MAX_ROWS (then it scrolls)
        let h = HEAD_H + self.rows_shown() as f64 * ROW_H + 4.0;
        let view = cx.walk_turtle(Walk { height: Size::Fixed(h), ..walk });
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        self.draw_list(cx);
        let tip = self.hover_tip(cx);
        let float = tip.as_ref().map(|(at, w, lines)| Float { at: *at, w: *w, lines, line_h: 12.5, pad: 7.0, bg: self.tip_bg, shadow: 0.3 });
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
                if i.is_some() {
                    cx.set_cursor(MouseCursor::Hand);
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
            Hit::FingerUp(e) if e.is_over => {
                if let Some(n) = self.row_at(e.abs - shift).and_then(|i| self.needs.get(i)).cloned() {
                    self.selected = Some(n.key());
                    cx.widget_action(self.uid, NeedsListAction::Picked(n));
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }
}
