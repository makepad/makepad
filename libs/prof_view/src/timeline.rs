//! The profiler's timeline: every lane of a profile on one clock. A ruler, the frames band (one
//! bar per frame, coloured by its time against the refresh budget), then per lane its spans by
//! nesting depth (waits as thin grey bars, compile jobs and JIT installs marked), the GPU lanes
//! last. The wheel zooms around the pointer, a drag pans, a click on the frames band selects a
//! frame (its window lit and its critical path outlined on the lanes), a double click zooms to
//! it. While live and following, the view keeps the newest time at its right edge.
//!
//! Cost per frame: per visible lane row, a binary search to the window and one quad per span
//! wider than a pixel; narrower spans fold into the pixel column they fall in.

use makepad_prof_stream::{model::CpStep, role, Profile};
use makepad_widgets::*;
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ProfTimelineBase = #(ProfTimeline::register_widget(vm))
    mod.widgets.ProfTimeline = set_type_default() do mod.widgets.ProfTimelineBase{
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.04)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.55)
        accent: theme.color_focus
        good: #4dc46b
        warn: #f2a64d
        bad: #e5534b
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 8}}
        draw_small +: {color: theme.color_text text_style: theme.font_regular{font_size: 7}}
    }
}

#[derive(Clone, Debug, Default)]
pub enum ProfTimelineAction {
    /// a frame was clicked (its id)
    SelectFrame(u64),
    /// the view was zoomed / panned away from the newest time
    Unfollowed,
    #[default]
    None,
}

const RULER: f64 = 18.0;
const FRAMES: f64 = 26.0;
const GUTTER: f64 = 150.0;
const ROW_H: f64 = 13.0;
const LANE_GAP: f64 = 5.0;
/// rows of a lane shown at most (deeper nesting is cut)
const MAX_ROWS: usize = 6;

#[derive(Clone, Copy, Debug)]
enum Drag {
    Pan { start_x: f64, start_t0: f64, moved: bool },
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct ProfTimeline {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
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
    #[rust]
    area: Area,
    #[rust]
    view: Rect,
    #[rust]
    profile: Option<Arc<Mutex<Profile>>>,
    /// the view's left edge (stream ns) and scale
    #[rust]
    t0: f64,
    #[rust]
    ns_per_px: f64,
    /// keep the newest time at the right edge
    #[rust(true)]
    follow: bool,
    #[rust]
    selected: Option<u64>,
    /// a frame to zoom to at the next draw
    #[rust]
    zoom_to: Option<u64>,
    #[rust]
    pointer: DVec2,
    #[rust]
    hover_text: String,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    lane_scroll: f64,
    /// lane tops of the last draw: (lane index, y, height)
    #[rust]
    lane_rects: Vec<(usize, f64, f64)>,
    /// the frames band's bars of the last draw: (x0, x1, frame id)
    #[rust]
    frame_bars: Vec<(f64, f64, u64)>,
}

impl ProfTimeline {
    pub fn set_profile(&mut self, cx: &mut Cx, p: Option<Arc<Mutex<Profile>>>) {
        self.profile = p;
        self.selected = None;
        self.ns_per_px = 0.0;
        self.follow = true;
        self.area.redraw(cx);
    }

    pub fn set_follow(&mut self, cx: &mut Cx, on: bool) {
        self.follow = on;
        self.area.redraw(cx);
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Selects a frame and shows it (zooms to it with a margin, at the next draw: the view's
    /// width is known then).
    pub fn show_frame(&mut self, cx: &mut Cx, id: u64) {
        self.selected = Some(id);
        self.zoom_to = Some(id);
        self.follow = false;
        self.area.redraw(cx);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn plot_x(&self) -> f64 {
        self.view.pos.x + GUTTER
    }

    fn x_of(&self, t: u64) -> f64 {
        self.plot_x() + (t as f64 - self.t0) / self.ns_per_px.max(1e-9)
    }

    fn t_of(&self, x: f64) -> f64 {
        self.t0 + (x - self.plot_x()) * self.ns_per_px
    }

    fn color_of(&self, name: u32, wait: bool, label: &str) -> Vec4f {
        if wait {
            return mix_color(self.background, self.ink, 0.22);
        }
        if label.starts_with("compile") {
            return vec4(0.93, 0.55, 0.22, 1.0);
        }
        // a stable hue per name, kept away from the frames band's red / green
        let h = ((name as u64).wrapping_mul(2654435761) % 360) as f32;
        let (r, g, b) = hsv(h, 0.45, 0.78);
        vec4(r, g, b, 1.0)
    }

    fn rect(&mut self, cx: &mut Cx2d, x0: f64, y: f64, x1: f64, h: f64, color: Vec4f) {
        self.draw_shape.color = color;
        self.draw_shape.draw_abs(cx, Rect { pos: dvec2(x0, y), size: dvec2((x1 - x0).max(1.0), h) });
    }

    fn text_fit(&mut self, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f, small: bool) {
        if max_w < 12.0 {
            return;
        }
        let draw = if small { &mut self.draw_small } else { &mut self.draw_text };
        let per = if small { 4.2 } else { 4.8 };
        let n = ((max_w / per) as usize).min(s.chars().count());
        if n < 2 {
            return;
        }
        let shown: String = if n < s.chars().count() { s.chars().take(n.saturating_sub(1)).chain(std::iter::once('…')).collect() } else { s.to_string() };
        draw.color = color;
        draw.draw_abs(cx, dvec2(x, y), &shown);
    }

    fn draw_profile(&mut self, cx: &mut Cx2d, p: &Profile) {
        let view = self.view;
        let plot_w = (view.size.x - GUTTER).max(10.0);
        if p.t_max == 0 {
            self.draw_text.color = self.muted;
            self.draw_text.draw_abs(cx, dvec2(view.pos.x + 12.0, view.pos.y + 12.0), "Waiting for the app's first records… (it records while it draws)");
            return;
        }
        if let Some(id) = self.zoom_to.take() {
            if let Some(f) = p.frame(id).copied() {
                let span = (f.dur() as f64 * 1.6).max(1e6);
                self.ns_per_px = span / plot_w;
                self.t0 = f.t0 as f64 - f.dur() as f64 * 0.3;
            }
        }
        if self.ns_per_px <= 0.0 {
            // the first view: 2 s (a live profile fills it from the right)
            let span = if self.follow { 2e9 } else { (p.t_max - p.t_min).clamp(50_000_000, 2_000_000_000) as f64 };
            self.ns_per_px = span / plot_w;
        }
        if self.follow {
            self.t0 = p.t_max as f64 - self.ns_per_px * plot_w;
        }
        let t_lo = self.t0.max(0.0) as u64;
        let t_hi = (self.t0 + self.ns_per_px * plot_w).max(0.0) as u64;
        // ruler
        let span_ms = (t_hi - t_lo) as f64 / 1e6;
        let step = nice_step(span_ms / (plot_w / 90.0));
        let first = ((t_lo.saturating_sub(p.t_min)) as f64 / 1e6 / step).ceil() * step;
        let mut ms_at = first;
        let top = view.pos.y;
        self.rect(cx, view.pos.x, top + RULER - 1.0, view.pos.x + view.size.x, 1.0, self.edge);
        while ms_at <= (t_hi - p.t_min) as f64 / 1e6 {
            let x = self.x_of(p.t_min + (ms_at * 1e6) as u64);
            if x >= self.plot_x() {
                self.rect(cx, x, top + RULER - 5.0, x + 1.0, view.size.y - RULER + 5.0, mix_color(self.background, self.ink, 0.06));
                let label = if step >= 1000.0 { format!("{:.0} s", ms_at / 1000.0) } else if step >= 1.0 { format!("{} ms", trim(ms_at)) } else { format!("{} ms", trim3(ms_at)) };
                self.draw_small.color = self.muted;
                self.draw_small.draw_abs(cx, dvec2(x + 3.0, top + 3.0), &label);
            }
            ms_at += step;
        }
        // frames band
        let fy = top + RULER + 2.0;
        self.draw_text.color = self.muted;
        self.draw_text.draw_abs(cx, dvec2(view.pos.x + 8.0, fy + 7.0), "frames");
        let budget = if p.info.refresh_mhz > 0 { 1e12 / p.info.refresh_mhz as f64 } else { 16.67e6 };
        self.frame_bars.clear();
        let a = p.frames.partition_point(|f| f.t1 < t_lo);
        let max_dur = p.frames.iter().map(|f| f.dur()).max().unwrap_or(1).max(1) as f64;
        for f in p.frames[a..].iter().take_while(|f| f.t0 < t_hi) {
            let x0 = self.x_of(f.t0).max(self.plot_x());
            let x1 = self.x_of(f.t1).min(view.pos.x + view.size.x);
            if x1 <= x0 {
                continue;
            }
            let d = f.dur() as f64;
            let c = if d <= budget * 1.05 { self.good } else if d <= budget * 2.1 { self.warn } else { self.bad };
            let h = (FRAMES - 6.0) * (0.3 + 0.7 * (d / max_dur).sqrt());
            let sel = self.selected == Some(f.id);
            if sel {
                self.rect(cx, x0, fy, x1, view.size.y - RULER - 2.0, mix_color(self.background, self.accent, 0.10));
            }
            self.rect(cx, x0 + 0.5, fy + FRAMES - 4.0 - h, x1 - 0.5, h, if sel { self.accent } else { c });
            self.frame_bars.push((x0, x1, f.id));
            if x1 - x0 > 46.0 {
                let s = format!("F{} {}", f.id, makepad_prof_stream::text::ms(f.dur()));
                self.text_fit(cx, x0 + 3.0, fy + 1.0, x1 - x0 - 4.0, &s, self.ink, true);
            }
        }
        // the lanes: main first, then by role (owner, compiler, program, other), GPU last
        // lanes with spans (threads that only woke others have no row of their own)
        let mut order: Vec<usize> = (0..p.lanes.len()).filter(|&i| !p.lanes[i].rows.is_empty()).collect();
        let rank = |r: u8| match r {
            role::MAIN => 0,
            role::RENDER => 1,
            role::OWNER => 2,
            role::COMPILER => 3,
            role::PROGRAM => 4,
            role::GPU => 9,
            role::INSPECT => 8,
            _ => 5,
        };
        order.sort_by_key(|&i| (rank(p.lanes[i].thread.role), p.lanes[i].thread.tid));
        let mut y = fy + FRAMES + 4.0 - self.lane_scroll;
        self.lane_rects.clear();
        let cp: Vec<CpStep> = match (self.selected.and_then(|id| p.frame(id).copied()), p.main_lane()) {
            (Some(f), Some(m)) => p.critical_path(m, f.t0, f.t1),
            _ => Vec::new(),
        };
        let clip_top = fy + FRAMES + 2.0;
        cx.push_clip_rect(Rect { pos: dvec2(view.pos.x, clip_top), size: dvec2(view.size.x, (view.pos.y + view.size.y - clip_top).max(0.0)) });
        for li in order {
            let lane = &p.lanes[li];
            let rows = lane.rows.len().clamp(1, MAX_ROWS);
            let h = rows as f64 * ROW_H;
            self.lane_rects.push((li, y, h));
            if y + h < clip_top || y > view.pos.y + view.size.y {
                y += h + LANE_GAP;
                continue;
            }
            // gutter: the lane's name and role
            self.rect(cx, view.pos.x, y - 2.0, view.pos.x + view.size.x, 1.0, mix_color(self.background, self.ink, 0.05));
            let name = p.lane_name(li);
            let r = role::NAMES.get(lane.thread.role as usize).copied().unwrap_or("");
            self.text_fit(cx, view.pos.x + 8.0, y, GUTTER - 14.0, &name, self.ink, false);
            if h > ROW_H {
                self.text_fit(cx, view.pos.x + 8.0, y + ROW_H, GUTTER - 14.0, &format!("{r} T{}", lane.thread.tid), self.muted, true);
            }
            for d in 0..rows {
                let ry = y + d as f64 * ROW_H;
                let mut last_px = f64::MIN;
                for s in lane.segs_in(d, t_lo, t_hi) {
                    let x0 = self.x_of(s.t0).max(self.plot_x());
                    let x1 = self.x_of(s.t1).min(view.pos.x + view.size.x);
                    // spans narrower than a pixel fold into the column already drawn
                    if x1 < last_px + 1.0 && x1 - x0 < 1.0 {
                        continue;
                    }
                    let label = p.s(s.name);
                    let c = self.color_of(s.name, s.wait, label);
                    if s.wait {
                        self.rect(cx, x0, ry + ROW_H * 0.5 - 1.5, x1, 3.0, c);
                    } else {
                        self.rect(cx, x0, ry + 1.0, x1, ROW_H - 2.0, c);
                        if x1 - x0 > 30.0 {
                            let t = format!("{} {}", label, makepad_prof_stream::text::dur(s.t1 - s.t0));
                            self.text_fit(cx, x0 + 2.0, ry + 1.5, x1 - x0 - 3.0, &t, vec4(0.08, 0.08, 0.1, 1.0), true);
                        }
                    }
                    last_px = x1;
                }
            }
            // JIT installs on this lane: a tick per install, its height by tier
            let tid = lane.thread.tid;
            for e in p.jit_in(t_lo, t_hi).iter().filter(|e| e.tid == tid && e.ev == makepad_prof_stream::jit::INSTALL) {
                let x = self.x_of(e.t);
                let c = tier_color(e.tier);
                self.rect(cx, x, y + h - 4.0 - e.tier.min(4) as f64, x + 1.0, 4.0 + e.tier.min(4) as f64, c);
            }
            // the selected frame's critical path on this lane
            for st in cp.iter().filter(|st| st.lane == li) {
                let x0 = self.x_of(st.t0).max(self.plot_x());
                let x1 = self.x_of(st.t1).min(view.pos.x + view.size.x);
                if x1 > x0 {
                    let c = if st.wait.is_some() { self.warn } else { self.accent };
                    self.rect(cx, x0, y - 1.0, x1, 2.0, c);
                    self.rect(cx, x0, y + h - 1.0, x1, 2.0, c);
                }
            }
            y += h + LANE_GAP;
        }
        cx.pop_clip_rect();
        // the gutter's edge
        self.rect(cx, view.pos.x + GUTTER - 1.0, top + RULER, view.pos.x + GUTTER, view.size.y - RULER, self.edge);
        // hover: what is under the pointer
        self.hover_text = self.hover_at(p, self.pointer);
        if !self.hover_text.is_empty() && view.contains(self.pointer) {
            let s = self.hover_text.clone();
            let w = (s.chars().count() as f64 * 5.2 + 12.0).min(520.0);
            let mut x = self.pointer.x + 12.0;
            if x + w > view.pos.x + view.size.x {
                x = self.pointer.x - w - 8.0;
            }
            let yy = self.pointer.y + 14.0;
            self.rect(cx, x, yy, x + w, 18.0, mix_color(self.background, self.ink, 0.88));
            self.draw_text.color = self.background;
            self.draw_text.draw_abs(cx, dvec2(x + 6.0, yy + 3.0), &s);
        }
    }

    /// The span or frame under `at`: one line.
    fn hover_at(&self, p: &Profile, at: DVec2) -> String {
        if at.x < self.plot_x() {
            return String::new();
        }
        let fy = self.view.pos.y + RULER + 2.0;
        if at.y >= fy && at.y < fy + FRAMES {
            if let Some(&(_, _, id)) = self.frame_bars.iter().find(|b| at.x >= b.0 && at.x < b.1) {
                if let Some(f) = p.frame(id) {
                    return format!("F{} · {} ms · click: select, double click: zoom", id, makepad_prof_stream::text::ms(f.dur()));
                }
            }
            return String::new();
        }
        let t = self.t_of(at.x).max(0.0) as u64;
        for &(li, y, h) in &self.lane_rects {
            if at.y < y || at.y >= y + h {
                continue;
            }
            let d = ((at.y - y) / ROW_H) as usize;
            let slack = (self.ns_per_px * 2.0) as u64;
            let lane = &p.lanes[li];
            if let Some(s) = lane.segs_in(d, t.saturating_sub(slack), t + slack).iter().min_by_key(|s| if t >= s.t0 && t <= s.t1 { 0 } else { s.t0.abs_diff(t).min(s.t1.abs_diff(t)) }) {
                let what = if s.wait { format!("wait {}", p.s(s.name)) } else { p.s(s.name).to_string() };
                let mut line = format!("{} · {} · {}", what, makepad_prof_stream::text::dur(s.t1 - s.t0), p.lane_name(li));
                if s.wait {
                    if let Some((by, _)) = p.woke_by(li, s) {
                        line.push_str(&format!(" · ended by {}", p.lane_name(by)));
                    }
                }
                return line;
            }
            // a JIT install near the pointer
            let near: Vec<_> = p.jit_in(t.saturating_sub(slack * 2), t + slack * 2).iter().filter(|e| e.tid == lane.thread.tid && e.ev == makepad_prof_stream::jit::INSTALL).collect();
            if let Some(e) = near.first() {
                return format!("install {} · {} · {} B{}", p.s(e.name), Profile::tier_name(e.tier), e.a, if near.len() > 1 { format!(" (+{} more here)", near.len() - 1) } else { String::new() });
            }
            return p.lane_name(li);
        }
        String::new()
    }

    fn zoom(&mut self, factor: f64, x: f64) {
        let t = self.t_of(x);
        self.ns_per_px = (self.ns_per_px * factor).clamp(200.0, 2e9);
        self.t0 = t - (x - self.plot_x()) * self.ns_per_px;
    }
}

fn tier_color(t: u8) -> Vec4f {
    match t {
        1 => vec4(0.6, 0.6, 0.65, 1.0),
        2 => vec4(0.45, 0.65, 0.95, 1.0),
        3 => vec4(0.3, 0.8, 0.8, 1.0),
        4 | 5 => vec4(0.95, 0.4, 0.85, 1.0),
        _ => vec4(0.9, 0.9, 0.3, 1.0),
    }
}

fn hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

fn mix_color(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

fn nice_step(raw: f64) -> f64 {
    let raw = raw.max(1e-4);
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * mag >= raw {
            return m * mag;
        }
    }
    10.0 * mag
}

fn trim(v: f64) -> String {
    let s = format!("{:.1}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn trim3(v: f64) -> String {
    let s = format!("{:.3}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl WidgetNode for ProfTimeline {
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

impl Widget for ProfTimeline {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        cx.push_clip_rect(view);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        match self.profile.clone() {
            Some(p) => {
                let p = p.lock().unwrap();
                self.draw_profile(cx, &p);
            }
            None => {
                self.draw_text.color = self.muted;
                self.draw_text.draw_abs(cx, dvec2(view.pos.x + 12.0, view.pos.y + 12.0), "Attach to a running app, or open a recording");
            }
        }
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let placed = self.area.rect(cx);
        if placed.size.x > 0.0 {
            self.view = placed;
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                self.pointer = e.abs;
                self.area.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.pointer = dvec2(-1e9, -1e9);
                self.area.redraw(cx);
            }
            Hit::FingerScroll(e) => {
                if e.abs.x < self.plot_x() {
                    self.lane_scroll = (self.lane_scroll + e.scroll.y).max(0.0);
                } else if e.scroll.x.abs() > e.scroll.y.abs() {
                    self.t0 += e.scroll.x * self.ns_per_px;
                    if self.follow {
                        self.follow = false;
                        cx.widget_action(self.uid, ProfTimelineAction::Unfollowed);
                    }
                } else {
                    self.zoom((e.scroll.y * 0.004).exp(), e.abs.x);
                    if self.follow && e.scroll.y < 0.0 {
                        // zooming in while following keeps the right edge on the newest time
                        let w = (self.view.size.x - GUTTER).max(10.0);
                        let _ = w;
                    }
                }
                self.area.redraw(cx);
            }
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                self.drag = Some(Drag::Pan { start_x: e.abs.x, start_t0: self.t0, moved: false });
            }
            Hit::FingerMove(e) => {
                if let Some(Drag::Pan { start_x, start_t0, moved }) = self.drag {
                    let dx = e.abs.x - start_x;
                    if moved || dx.abs() > 3.0 {
                        self.drag = Some(Drag::Pan { start_x, start_t0, moved: true });
                        self.t0 = start_t0 - dx * self.ns_per_px;
                        if self.follow {
                            self.follow = false;
                            cx.widget_action(self.uid, ProfTimelineAction::Unfollowed);
                        }
                        cx.set_cursor(MouseCursor::Grabbing);
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(e) => {
                let moved = matches!(self.drag.take(), Some(Drag::Pan { moved: true, .. }));
                if !moved && e.is_over {
                    let fy = self.view.pos.y + RULER + 2.0;
                    if e.abs.y >= fy && e.abs.y < fy + FRAMES {
                        if let Some(&(_, _, id)) = self.frame_bars.iter().find(|b| e.abs.x >= b.0 && e.abs.x < b.1) {
                            if e.tap_count >= 2 {
                                self.show_frame(cx, id);
                            } else {
                                self.selected = Some(id);
                            }
                            cx.widget_action(self.uid, ProfTimelineAction::SelectFrame(id));
                            self.area.redraw(cx);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
