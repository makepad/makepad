//! The detail of what was picked: a lease (from the timeline or a need about it) or a need
//! about a node, a landing, a bucket or the inbox. For a lease: its lane, agent and node, what
//! it runs, its run against its measured run as a small graph, its holder's last beat, what it
//! waited on, its job's output tail (`job output` of its log's end, asked only while this is
//! open), and the devhub commands that act on it, each with a copy button. The panel stays
//! read-only: it shows the commands, a person or an agent runs them.

use crate::fmt::{clock, dur, local_offset_s};
use crate::model::{Fleet, JobBrief};
use crate::needs::{Need, Sev};
use crate::timeline::{mix, moved_by, DrawBar};
use makepad_devhub::lease::panel::Row;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DetailPaneBase = #(DetailPane::register_widget(vm))
    mod.widgets.DetailPane = set_type_default() do mod.widgets.DetailPaneBase{
        width: Fill height: Fit
        background: mix(theme.color_bg_app, theme.color_text, 0.045)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        code_bg: mix(theme.color_bg_app, theme.color_text, 0.08)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.58)
        accent: theme.color_focus
        good: #3fb56a
        warn: #e8a13c
        bad: #e5534b
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: theme.color_text text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {color: theme.color_text text_style: theme.font_regular{font_size: 6.8}}
        draw_code +: {color: theme.color_text text_style: theme.font_code{font_size: 6.6}}
    }
}

/// The output box's height, for a subject with a job.
const OUTPUT_H: f64 = 124.0;
const LINE: f64 = 13.0;
const LABEL_W: f64 = 58.0;

#[derive(Clone, Debug, Default)]
pub enum DetailPaneAction {
    Closed,
    #[default]
    None,
}

/// What the pane shows: a lease, a need, or a need about a lease.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subject {
    pub lease: Option<String>,
    pub need: Option<Need>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Btn {
    Close,
    Copy(usize),
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct DetailPane {
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
    draw_code: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    code_bg: Vec4f,
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
    fleet: Option<Arc<Fleet>>,
    #[rust]
    subject: Option<Subject>,
    /// the commands of the last draw
    #[rust]
    acts: Vec<String>,
    #[rust]
    buttons: Vec<(Rect, Btn)>,
    #[rust]
    hover: Option<Btn>,
    /// the command just copied: its button says so until `copied_timer` ends
    #[rust]
    copied: Option<usize>,
    #[rust]
    copied_timer: Timer,
}

impl DetailPane {
    pub fn set_fleet(&mut self, cx: &mut Cx, f: Arc<Fleet>) {
        self.fleet = Some(f);
        if self.subject.is_some() {
            self.area.redraw(cx);
        }
    }

    pub fn show_subject(&mut self, cx: &mut Cx, s: Option<Subject>) {
        if s != self.subject {
            self.subject = s;
            self.copied = None;
            self.area.redraw(cx);
        }
    }

    pub fn subject(&self) -> Option<&Subject> {
        self.subject.as_ref()
    }

    fn width(d: &DrawText, cx: &mut Cx2d, s: &str) -> f64 {
        d.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64
    }

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

    fn sev_color(&self, s: Sev) -> Vec4f {
        match s {
            Sev::Red => self.bad,
            Sev::Amber => self.warn,
            Sev::Info => self.muted,
        }
    }

    /// A labelled fact; the next line's y.
    fn fact(&mut self, cx: &mut Cx2d, x: f64, y: f64, w: f64, label: &str, value: &str, color: Vec4f) -> f64 {
        let muted = self.muted;
        Self::text(&mut self.draw_small, cx, x, y + 1.0, LABEL_W - 4.0, label, muted);
        Self::text(&mut self.draw_text, cx, x + LABEL_W, y, w - LABEL_W, value, color);
        y + LINE
    }

    fn draw_pane(&mut self, cx: &mut Cx2d, f: &Fleet, s: &Subject) {
        self.buttons.clear();
        let v = self.view;
        let (x, w) = (v.pos.x + 10.0, v.size.x - 20.0);
        let (ink, muted) = (self.ink, self.muted);
        let offset = local_offset_s(f.now_ms);
        let row: Option<&Row> = s.lease.as_deref().and_then(|id| f.row(id));
        // the title and the close button
        let title = match (row, &s.need) {
            (Some(r), _) => format!("{} \u{b7} {} \u{b7} {}", r.id, r.kind, r.lane),
            (None, Some(n)) => n.title.clone(),
            (None, None) => s.lease.clone().map_or(String::new(), |id| format!("{id} \u{b7} no longer in the window")),
        };
        Self::text(&mut self.draw_bold, cx, x, v.pos.y + 7.0, w - 24.0, &title, ink);
        let close = Rect { pos: dvec2(v.pos.x + v.size.x - 24.0, v.pos.y + 4.0), size: dvec2(18.0, 18.0) };
        if self.hover == Some(Btn::Close) {
            let a = self.accent;
            self.draw_bar.plain(cx, close, vec4(a.x, a.y, a.z, 0.15), vec4(0.0, 0.0, 0.0, 0.0), 4.0);
        }
        Self::text(&mut self.draw_text, cx, close.pos.x + 5.5, close.pos.y + 3.0, 12.0, "\u{d7}", muted);
        self.buttons.push((close, Btn::Close));
        let mut y = v.pos.y + 24.0;
        // what is wrong, when a need says so
        if let Some(n) = &s.need {
            let sc = self.sev_color(n.sev);
            let tag = n.what.tag();
            let tw = Self::width(&self.draw_small, cx, tag) + 8.0;
            self.draw_bar.plain(cx, Rect { pos: dvec2(x, y), size: dvec2(tw, 12.0) }, vec4(sc.x, sc.y, sc.z, 0.16), vec4(sc.x, sc.y, sc.z, 0.6), 2.5);
            Self::text(&mut self.draw_small, cx, x + 4.0, y + 1.5, tw, tag, sc);
            Self::text(&mut self.draw_text, cx, x + tw + 6.0, y + 0.5, w - tw - 6.0, &n.detail, sc);
            y += LINE + 3.0;
        }
        let job = job_for(f, s);
        if let Some(r) = row {
            let who = if r.agent.is_empty() { r.lane.clone() } else { format!("{} \u{b7} agent {}", r.lane, r.agent) };
            y = self.fact(cx, x, y, w, "lane", &who, ink);
            let node = if r.node.is_empty() { "not placed yet".to_string() } else { r.node.clone() };
            y = self.fact(cx, x, y, w, "node", &format!("{node} \u{b7} {} \u{b7} {} ({})", r.priority, r.cost, r.cost_source), ink);
            let runs = match job {
                Some(j) if !j.cmd.is_empty() => j.cmd.clone(),
                _ if !r.running.is_empty() => r.running.clone(),
                _ => r.class.clone(),
            };
            y = self.fact(cx, x, y, w, "runs", &runs, ink);
            if let Some(j) = job {
                let (state, c) = match &j.panic {
                    Some(p) if j.state == "paused" => (format!("{} paused on {} at {}", j.id, p.message, p.location), self.bad),
                    _ => (format!("{} \u{b7} {}", j.id, j.state), ink),
                };
                y = self.fact(cx, x, y, w, "job", &state, c);
            }
            // its run against its measured run
            y = self.draw_run(cx, f, r, x, y, w);
            let beat = match (r.beat_ms, r.ended_ms) {
                (_, Some(e)) => (format!("ended {} ({})", clock(e, offset, true), if r.how.is_empty() { "ended" } else { &r.how }), muted),
                (Some(b), None) => {
                    let age = f.now_ms.saturating_sub(b);
                    (format!("{} ago (it expires at 10 s without one)", dur(age)), if age >= crate::needs::STALE_BEAT_MS { self.bad } else { ink })
                }
                (None, None) if r.granted_ms.is_some() => ("watched by its holder's process".to_string(), muted),
                (None, None) => ("-".to_string(), muted),
            };
            y = self.fact(cx, x, y, w, "beat", &beat.0, beat.1);
            let wait = if r.state == "queued" {
                let mut s = format!("waiting {} \u{b7} #{} \u{b7} {}", dur(f.wait_ms(r)), r.pos.max(1), if r.reason.is_empty() { "no reason given" } else { &r.reason });
                if r.eta_ms > 0 {
                    s.push_str(&format!(" \u{b7} expected in {}", dur(r.eta_ms)));
                }
                s
            } else {
                let on = f.waited_on(r);
                format!("waited {}{}", dur(f.wait_ms(r)), if on.is_empty() { String::new() } else { format!(" on {on}") })
            };
            y = self.fact(cx, x, y, w, "wait", &wait, ink);
        }
        // the commands, at the bottom: what acts on it
        let acts = acts_for(f, s);
        let acts_y = v.pos.y + v.size.y - acts_h(acts.len()) - 4.0;
        // the output tail fills what is left
        y += 4.0;
        if acts_y - y >= 12.0 {
            self.draw_output(cx, f, job.map(|j| j.id.as_str()), row.is_some(), x, y, w, acts_y - y - 6.0);
        }
        if !acts.is_empty() {
            Self::text(&mut self.draw_small, cx, x, acts_y, w, "ACT (read-only here: copy and run)", muted);
            for (i, a) in acts.iter().enumerate() {
                let ay = acts_y + 13.0 + i as f64 * 18.0;
                let copied = self.copied == Some(i);
                let bw = 44.0;
                let code = Rect { pos: dvec2(x, ay), size: dvec2(w - bw - 4.0, 15.0) };
                let cb = self.code_bg;
                self.draw_bar.plain(cx, code, cb, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
                Self::text(&mut self.draw_code, cx, x + 5.0, ay + 3.0, code.size.x - 8.0, a, ink);
                let btn = Rect { pos: dvec2(x + w - bw, ay), size: dvec2(bw, 15.0) };
                let a_c = if copied { self.good } else { self.accent };
                let fill = if self.hover == Some(Btn::Copy(i)) { 0.3 } else { 0.14 };
                self.draw_bar.plain(cx, btn, vec4(a_c.x, a_c.y, a_c.z, fill), vec4(a_c.x, a_c.y, a_c.z, 0.7), 3.0);
                let label = if copied { "copied" } else { "copy" };
                let lw = Self::width(&self.draw_small, cx, label);
                Self::text(&mut self.draw_small, cx, btn.pos.x + (bw - lw) * 0.5, ay + 2.5, bw, label, a_c);
                self.buttons.push((btn, Btn::Copy(i)));
            }
        }
        self.acts = acts;
    }

    /// Its run as a bar against its measured run: green within it, amber past it, red past twice.
    fn draw_run(&mut self, cx: &mut Cx2d, f: &Fleet, r: &Row, x: f64, y: f64, w: f64) -> f64 {
        let Some(g) = r.granted_ms else {
            return y;
        };
        let ran = f.end_ms(r).saturating_sub(g);
        let muted = self.muted;
        Self::text(&mut self.draw_small, cx, x, y + 3.0, LABEL_W - 4.0, "run", muted);
        let tx = x + LABEL_W;
        let tw = w - LABEL_W;
        let label = match r.measured_ms {
            Some(m) if m > 0 => format!("{} of its measured {} ({:.1}\u{d7})", dur(ran), dur(m), ran as f64 / m as f64),
            _ if r.live => format!("{} (a live session: open-ended)", dur(ran)),
            _ => format!("{} (not measured yet)", dur(ran)),
        };
        let measured = r.measured_ms.filter(|m| *m > 0);
        let scale = measured.map_or(ran.max(1), |m| ran.max(2 * m)).max(1) as f64 * 1.08;
        let track = Rect { pos: dvec2(tx, y + 13.0), size: dvec2(tw, 6.0) };
        let cb = self.code_bg;
        self.draw_bar.plain(cx, track, cb, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
        let share = |ms: u64| ms as f64 / scale;
        let c = match measured {
            Some(m) if ran > 2 * m => self.bad,
            Some(m) if ran > m => self.warn,
            Some(_) => self.good,
            None => self.accent,
        };
        let fw = (tw * share(ran)).max(3.0);
        let fill = Rect { pos: track.pos, size: dvec2(fw, 6.0) };
        if ran > measured.map_or(u64::MAX, |m| 2 * m) {
            self.draw_bar.glowing(cx, fill, c, vec4(0.0, 0.0, 0.0, 0.0), vec4(c.x, c.y, c.z, 0.5), 3.5, 3.0);
        } else {
            self.draw_bar.plain(cx, fill, c, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
        }
        if let Some(m) = measured {
            // its measured run and twice it, as ticks
            for (k, at) in [(1.0, m), (0.55, 2 * m)] {
                let mx = tx + tw * share(at);
                let ink = self.ink;
                self.draw_shape.color = mix(self.background, ink, k * 0.8);
                self.draw_shape.draw_abs(cx, Rect { pos: dvec2(mx - 0.5, y + 10.0), size: dvec2(1.5, 12.0) });
            }
        }
        let ink = self.ink;
        Self::text(&mut self.draw_text, cx, tx, y, tw, &label, ink);
        y + 24.0
    }

    /// The job's output tail (its error lines first), or why there is none.
    #[allow(clippy::too_many_arguments)]
    fn draw_output(&mut self, cx: &mut Cx2d, f: &Fleet, job: Option<&str>, lease: bool, x: f64, y: f64, w: f64, h: f64) {
        let muted = self.muted;
        let detail = job.and_then(|j| f.detail.as_ref().filter(|d| d.job.id == j));
        let head = match (job, detail) {
            (Some(j), Some(_)) => format!("OUTPUT \u{b7} {j} (job result {j} --tail)"),
            (Some(j), None) => format!("OUTPUT \u{b7} {j}: asking the master\u{2026}"),
            (None, _) if lease => "OUTPUT \u{b7} no job runs this lease: its output is its holder's".to_string(),
            (None, _) => return,
        };
        Self::text(&mut self.draw_small, cx, x, y, w, &head, muted);
        let Some(d) = detail else { return };
        let box_r = Rect { pos: dvec2(x, y + 12.0), size: dvec2(w, h - 12.0) };
        let cb = self.code_bg;
        self.draw_bar.plain(cx, box_r, cb, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
        let lh = 10.5;
        let fit = ((box_r.size.y - 6.0) / lh).max(0.0) as usize;
        // the error lines first (at most a third), then the newest lines
        let errs: Vec<&String> = d.errors.iter().rev().take(fit / 3).collect::<Vec<_>>().into_iter().rev().collect();
        let rest = fit.saturating_sub(errs.len());
        let tail: Vec<&String> = d.tail.iter().rev().take(rest).collect::<Vec<_>>().into_iter().rev().collect();
        let (bad, ink) = (self.bad, self.ink);
        let mut ly = box_r.pos.y + 3.0;
        for (l, c) in errs.iter().map(|l| (l, bad)).chain(tail.iter().map(|l| (l, ink))) {
            Self::text(&mut self.draw_code, cx, x + 5.0, ly, w - 10.0, l, c);
            ly += lh;
        }
        if !d.err.is_empty() {
            Self::text(&mut self.draw_code, cx, x + 5.0, ly, w - 10.0, &format!("the master gave no output: {}", d.err), bad);
        } else if errs.is_empty() && tail.is_empty() {
            Self::text(&mut self.draw_code, cx, x + 5.0, ly, w - 10.0, "(no output yet)", muted);
        }
    }
}

/// The job a subject's output comes from: its lease's, or the one its need names.
fn job_for<'a>(f: &'a Fleet, s: &Subject) -> Option<&'a JobBrief> {
    let row = s.lease.as_deref().and_then(|id| f.row(id));
    row.and_then(|r| f.job_of(&r.id)).or_else(|| s.need.as_ref().and_then(|n| n.job.as_deref()).and_then(|j| f.jobs.as_ref()?.iter().find(|x| x.id == j)))
}

/// The devhub commands that act on a subject (at most four): its need's, then its lease's.
fn acts_for(f: &Fleet, s: &Subject) -> Vec<String> {
    let mut acts: Vec<String> = s.need.as_ref().map(|n| n.act.clone()).unwrap_or_default();
    if let Some(r) = s.lease.as_deref().and_then(|id| f.row(id)) {
        let mut add = |c: String| {
            if !acts.contains(&c) {
                acts.push(c);
            }
        };
        if let Some(j) = job_for(f, s) {
            add(format!("makepad devhub job result {} --tail", j.id));
        }
        if r.state == "queued" {
            add(format!("makepad devhub why-queued {}", r.id));
            add(format!("makepad devhub job bump {}", r.id));
        } else if r.ended_ms.is_none() {
            add(format!("makepad devhub cancel {}", r.id));
        }
    }
    acts.truncate(4);
    acts
}

fn acts_h(n: usize) -> f64 {
    if n == 0 { 0.0 } else { n as f64 * 18.0 + 16.0 }
}

/// The pane's height for a subject: as tall as what it shows (draw_pane's lines).
fn height_for(f: &Fleet, s: &Subject) -> f64 {
    let row = s.lease.as_deref().and_then(|id| f.row(id));
    let job = job_for(f, s);
    let mut h = 24.0;
    if s.need.is_some() {
        h += LINE + 3.0;
    }
    if let Some(r) = row {
        // lane, node, runs, beat, wait; job; the run bar
        h += LINE * 5.0 + if job.is_some() { LINE } else { 0.0 } + if r.granted_ms.is_some() { 24.0 } else { 0.0 };
    }
    h += 4.0 + match (job, row) {
        (Some(_), _) => OUTPUT_H,
        (None, Some(_)) => 18.0,
        (None, None) => 0.0,
    };
    h + acts_h(acts_for(f, s).len()) + 8.0
}

impl WidgetNode for DetailPane {
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

impl Widget for DetailPane {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let h = match (&self.subject, &self.fleet) {
            (Some(s), Some(f)) => height_for(f, s),
            _ => 0.0,
        };
        let view = cx.walk_turtle(Walk { height: Size::Fixed(h), ..walk });
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        let (Some(s), Some(f)) = (self.subject.clone(), self.fleet.clone()) else {
            self.buttons.clear();
            return DrawStep::done();
        };
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        let edge = self.edge;
        self.draw_shape.color = edge;
        self.draw_shape.draw_abs(cx, Rect { pos: dvec2(view.pos.x, view.pos.y + view.size.y - 1.0), size: dvec2(view.size.x, 1.0) });
        cx.push_clip_rect(view);
        self.draw_pane(cx, &f, &s);
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let shift = moved_by(cx, self.area, self.view);
        if self.copied_timer.is_event(event).is_some() {
            self.copied = None;
            self.area.redraw(cx);
        }
        let at = |p: DVec2, b: &[(Rect, Btn)]| b.iter().find(|(r, _)| r.contains(p)).map(|(_, b)| *b);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let h = at(e.abs - shift, &self.buttons);
                if h != self.hover {
                    self.hover = h;
                    self.area.redraw(cx);
                }
                if h.is_some() {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_over => match at(e.abs - shift, &self.buttons) {
                Some(Btn::Close) => {
                    self.subject = None;
                    cx.widget_action(self.uid, DetailPaneAction::Closed);
                    self.area.redraw(cx);
                }
                Some(Btn::Copy(i)) => {
                    if let Some(c) = self.acts.get(i).cloned() {
                        cx.copy_to_clipboard(&c);
                        self.copied = Some(i);
                        self.copied_timer = cx.start_timeout(2.0);
                        self.area.redraw(cx);
                    }
                }
                None => {}
            },
            _ => {}
        }
    }
}
