//! The tween timeline inspector: every animation the app's [`TweenHost`]s
//! are playing, one at a time on a timeline you can scrub.
//!
//! The top lists each host's top-level animations (click one to inspect
//! it). Under it: Play / Pause, Restart and the speed, then a seconds ruler
//! with the timeline's labels, one lane per animation inside it (indented
//! by depth, a bar per iteration, lighter for repeats) and the playhead.
//! Press or drag on the ruler or the lanes to scrub: the animation pauses
//! and the playhead follows the pointer, with its callbacks suppressed; on
//! release it plays on if it was playing.
//!
//! While drawn it opens the inspector ([`set_tween_inspect`]); the host
//! that shows it closes it again. The data comes from the registry the
//! hosts publish into ([`TweenInspectRegistry`]); commands go back through
//! it and each host applies them on its next event.
//!
//! [`TweenHost`]: crate::tween::TweenHost

use crate::makepad_derive_widget::*;
use crate::makepad_draw::*;
use crate::tween::{InspectKind, InspectNode, Tag, TweenId, BIG};
use crate::tween_inspect::{set_tween_inspect, tween_inspect_on, InspectCommand, TweenInspectRegistry};
use crate::widget::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TweenInspectorBase = #(TweenInspector::register_widget(vm))

    /** The tween timeline inspector: what every TweenHost plays, scrubbable. */
    mod.widgets.TweenInspector = set_type_default() do mod.widgets.TweenInspectorBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: theme.color_inset
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 8}
        }
        color_ink: theme.color_text
        color_meta: theme.color_text_meta
        color_face: theme.color_outset
        color_face_on: theme.color_ctrl_selected
        color_ruler: theme.color_shadow
        color_label: theme.color_warning
        color_playhead: theme.color_error
    }
}

const PAD: f64 = 6.0;
const ROW: f64 = 16.0;
const LANE: f64 = 15.0;
/// The thinnest lane when there are more lanes than room.
const LANE_MIN: f64 = 7.0;
const RULER: f64 = 18.0;
/// One row of label names under the ruler (there are two).
const LABEL_ROW: f64 = 11.0;
const NAME_W: f64 = 118.0;
/// The most animations listed in the picker before "+N more".
const PICKER_ROWS: usize = 8;
const SPEEDS: [f64; 4] = [0.25, 0.5, 1.0, 2.0];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Chip {
    PlayPause,
    Restart,
    Speed(usize),
}

#[derive(Clone, Debug)]
struct PickRow {
    host: u64,
    id: TweenId,
    text: String,
}

#[derive(Clone, Copy, Debug)]
struct Scrub {
    host: u64,
    id: TweenId,
    was_playing: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TweenInspector {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    color_ink: Vec4f,
    /// Secondary text, ruler ticks and lanes nested below the top.
    #[live]
    color_meta: Vec4f,
    /// Picker rows and transport chips.
    #[live]
    color_face: Vec4f,
    /// The selected row and the current speed.
    #[live]
    color_face_on: Vec4f,
    #[live]
    color_ruler: Vec4f,
    /// Timeline labels on the ruler.
    #[live]
    color_label: Vec4f,
    #[live]
    color_playhead: Vec4f,

    /// The inspected animation: (host, top-level animation).
    #[rust]
    selected: Option<(u64, TweenId)>,
    #[rust]
    picker: Vec<(Rect, PickRow)>,
    #[rust]
    chips: Vec<(Rect, Chip)>,
    /// The ruler and the lanes: where a press scrubs.
    #[rust]
    track_rect: Rect,
    /// Where the timeline's 0 and its end are drawn (x), and its length.
    #[rust]
    span: (f64, f64, f64),
    #[rust]
    scrub: Option<Scrub>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_draw: f64,
    /// Copies of the inspected animation for drawing (reused buffers).
    #[rust]
    lanes: Vec<InspectNode>,
    #[rust]
    labels: Vec<(Tag, f64)>,
}

/// A readable name for a node: its tag, else what it animates, else its kind.
pub(crate) fn node_name(n: &InspectNode) -> String {
    if n.tag != Tag::NONE {
        return crate::widget_tree::live_id_token(LiveId(n.tag.0));
    }
    match n.kind {
        InspectKind::Tween => match n.first_track {
            Some((t, p)) => {
                let prop = crate::widget_tree::live_id_token(LiveId(p.0));
                if n.tracks > 1 {
                    format!("#{} {} +{}", t.0, prop, n.tracks - 1)
                } else {
                    format!("#{} {}", t.0, prop)
                }
            }
            None => "tween".to_string(),
        },
        InspectKind::Timeline => "timeline".to_string(),
        InspectKind::Group => "stagger".to_string(),
        InspectKind::Call => "call".to_string(),
        InspectKind::Pause => "pause".to_string(),
    }
}

/// A node's colour on the timeline, by kind.
fn kind_color(kind: InspectKind) -> Vec4f {
    match kind {
        InspectKind::Tween => vec4(0.33, 0.55, 0.90, 1.0),
        InspectKind::Timeline => vec4(0.55, 0.45, 0.85, 1.0),
        InspectKind::Group => vec4(0.30, 0.70, 0.60, 1.0),
        InspectKind::Call => vec4(0.95, 0.65, 0.25, 1.0),
        InspectKind::Pause => vec4(0.90, 0.40, 0.40, 1.0),
    }
}

/// The span a timeline view shows and where its playhead is: the whole
/// animation when it ends, one iteration when it repeats forever.
pub(crate) fn view_of(root: &InspectNode) -> (f64, f64) {
    if root.total_duration < BIG {
        (root.total_duration.max(0.001), root.total_time)
    } else {
        (root.duration.max(0.001), root.time)
    }
}

/// A ruler step near one tick per 60 px.
fn nice_step(span: f64, width: f64) -> f64 {
    let raw = span * 60.0 / width.max(1.0);
    let mag = 10f64.powf(raw.max(1e-6).log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * mag >= raw {
            return m * mag;
        }
    }
    10.0 * mag
}

impl TweenInspector {
    fn rect(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f) {
        self.draw_rect.color = color;
        self.draw_rect.draw_abs(cx, r);
    }

    fn text(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        self.draw_text.color = color;
        self.draw_text.draw_abs(cx, at, text);
    }

    /// `text` cut to `width` with an ellipsis.
    fn fit(&self, cx: &mut Cx2d, text: &str, width: f64) -> String {
        if crate::badge::measure(&self.draw_text, cx, text) <= width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut n = chars.len();
        while n > 0 {
            n -= 1;
            let cut: String = chars[..n].iter().collect::<String>() + "\u{2026}";
            if crate::badge::measure(&self.draw_text, cx, &cut) <= width {
                return cut;
            }
        }
        String::new()
    }

    fn command(&self, cx: &mut Cx, host: u64, cmd: InspectCommand) {
        cx.global::<TweenInspectRegistry>().command(host, cmd);
        // The host applies it on its next event: make sure one comes.
        cx.redraw_all();
    }

    /// The time under x on the timeline.
    fn time_at(&self, x: f64) -> f64 {
        let (x0, x1, len) = self.span;
        ((x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0) * len
    }

    fn scrub_to(&mut self, cx: &mut Cx, x: f64) {
        if let Some(s) = self.scrub {
            let time = self.time_at(x);
            self.command(cx, s.host, InspectCommand::Scrub { id: s.id, time });
        }
    }
}

impl Widget for TweenInspector {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            // Every frame reaches every host (they publish on it); redraw
            // only when one published since the last draw, or once a second
            // so stale hosts drop out.
            let now = cx.seconds_since_app_start();
            let fresh = cx.global::<TweenInspectRegistry>().hosts.iter().any(|h| h.at > self.last_draw);
            if fresh || now - self.last_draw > 1.0 {
                self.draw_bg.redraw(cx);
            } else {
                self.next_frame = cx.new_next_frame();
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                if let Some((_, row)) = self.picker.iter().find(|(r, _)| r.contains(fe.abs)) {
                    self.selected = Some((row.host, row.id));
                    self.draw_bg.redraw(cx);
                    return;
                }
                let Some((host, id)) = self.selected else {
                    return;
                };
                let paused = self.lanes.first().is_none_or(|n| n.paused);
                // A completed (detached) animation plays again from the start.
                let done = self.lanes.first().is_some_and(|n| !n.linked);
                if let Some((_, chip)) = self.chips.iter().find(|(r, _)| r.contains(fe.abs)) {
                    let cmd = match *chip {
                        Chip::PlayPause if done => InspectCommand::Restart(id),
                        Chip::PlayPause if paused => InspectCommand::Resume(id),
                        Chip::PlayPause => InspectCommand::Pause(id),
                        Chip::Restart => InspectCommand::Restart(id),
                        Chip::Speed(i) => InspectCommand::TimeScale { id, scale: SPEEDS[i] },
                    };
                    self.command(cx, host, cmd);
                    return;
                }
                if self.track_rect.contains(fe.abs) {
                    self.scrub = Some(Scrub { host, id, was_playing: !paused && !done });
                    self.scrub_to(cx, fe.abs.x);
                }
            }
            Hit::FingerMove(fe) => {
                self.scrub_to(cx, fe.abs.x);
            }
            Hit::FingerUp(_) => {
                if let Some(s) = self.scrub.take() {
                    if s.was_playing {
                        self.command(cx, s.host, InspectCommand::Resume(s.id));
                    }
                }
            }
            Hit::FingerHoverOver(fe) => {
                let hand = self.picker.iter().any(|(r, _)| r.contains(fe.abs))
                    || self.chips.iter().any(|(r, _)| r.contains(fe.abs));
                if hand {
                    cx.set_cursor(MouseCursor::Hand);
                } else if self.track_rect.contains(fe.abs) {
                    cx.set_cursor(MouseCursor::EwResize);
                } else {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !tween_inspect_on() {
            set_tween_inspect(cx, true);
        }
        let rect = cx.walk_turtle(walk);
        let now = cx.seconds_since_app_start();
        self.last_draw = now;
        // Keep a frame coming while shown: the hosts republish on it (about
        // 30 times a second) and the playhead follows.
        self.next_frame = cx.new_next_frame();

        // Copy what is drawn out of the registry (it lives in the Cx).
        self.lanes.clear();
        self.labels.clear();
        let mut rows: Vec<PickRow> = Vec::new();
        let mut more = 0usize;
        {
            let reg = cx.global::<TweenInspectRegistry>();
            reg.prune(now);
            for h in &reg.hosts {
                for root in h.roots() {
                    if rows.len() >= PICKER_ROWS {
                        more += 1;
                        continue;
                    }
                    let (len, _) = view_of(root);
                    let state = if !root.linked {
                        "done"
                    } else if root.paused {
                        "paused"
                    } else {
                        "playing"
                    };
                    rows.push(PickRow {
                        host: h.id,
                        id: root.id,
                        text: format!("{} \u{00b7} {}  {:.2}s  {}", h.name, node_name(root), len, state),
                    });
                }
            }
            let valid = self.selected.is_some_and(|(host, id)| {
                reg.hosts.iter().any(|h| h.id == host && h.nodes.iter().any(|n| n.id == id && n.depth == 0))
            });
            if !valid {
                self.selected = rows.first().map(|r| (r.host, r.id));
            }
            if let Some((host, id)) = self.selected {
                if let Some(h) = reg.hosts.iter().find(|h| h.id == host) {
                    self.lanes.extend_from_slice(h.subtree(id));
                    self.labels.extend(h.labels.iter().filter(|l| l.0 == id).map(|l| (l.1, l.2)));
                }
            }
        }

        self.draw_bg.draw_abs(cx, rect);
        let (ink, meta) = (self.color_ink, self.color_meta);
        let (face, face_on) = (self.color_face, self.color_face_on);
        let (label_ink, playhead) = (self.color_label, self.color_playhead);
        let mut y = rect.pos.y + PAD;
        let x0 = rect.pos.x + PAD;
        let w = (rect.size.x - PAD * 2.0).max(10.0);

        self.picker.clear();
        self.chips.clear();
        self.track_rect = Rect::default();
        if rows.is_empty() {
            self.text(cx, dvec2(x0, y), "No tweens are running.", ink);
            let hint = self.fit(cx, "Hosts appear while their widget gets events.", w);
            self.text(cx, dvec2(x0, y + ROW), &hint, meta);
            return DrawStep::done();
        }

        // The picker.
        for row in rows {
            let r = Rect { pos: dvec2(x0, y), size: dvec2(w, ROW - 1.0) };
            let on = self.selected == Some((row.host, row.id));
            self.rect(cx, r, if on { face_on } else { face });
            let text = self.fit(cx, &row.text, w - 8.0);
            self.text(cx, dvec2(x0 + 4.0, y + 2.0), &text, ink);
            self.picker.push((r, row));
            y += ROW;
        }
        if more > 0 {
            self.text(cx, dvec2(x0 + 4.0, y + 2.0), &format!("+{more} more"), meta);
            y += ROW;
        }
        y += 4.0;

        let Some(root) = self.lanes.first().copied() else {
            return DrawStep::done();
        };
        let (len, head) = view_of(&root);

        // Transport.
        let mut cx_x = x0;
        let chips: [(Chip, String); 6] = [
            (Chip::PlayPause, if root.paused || !root.linked { "Play".into() } else { "Pause".into() }),
            (Chip::Restart, "Restart".into()),
            (Chip::Speed(0), "0.25x".into()),
            (Chip::Speed(1), "0.5x".into()),
            (Chip::Speed(2), "1x".into()),
            (Chip::Speed(3), "2x".into()),
        ];
        for (chip, label) in chips {
            let tw = crate::badge::measure(&self.draw_text, cx, &label) + 10.0;
            let r = Rect { pos: dvec2(cx_x, y), size: dvec2(tw, ROW) };
            let on = matches!(chip, Chip::Speed(i) if (SPEEDS[i] - root.time_scale).abs() < 1e-6);
            self.rect(cx, r, if on { face_on } else { face });
            self.text(cx, dvec2(cx_x + 5.0, y + 2.0), &label, ink);
            self.chips.push((r, chip));
            cx_x += tw + 4.0;
        }
        let readout = format!("{:.3}s / {:.3}s", head, len);
        let rw = crate::badge::measure(&self.draw_text, cx, &readout);
        if cx_x + rw < x0 + w {
            self.text(cx, dvec2(x0 + w - rw, y + 2.0), &readout, meta);
        }
        y += ROW + 6.0;

        // The ruler, the lanes and the playhead.
        let bx0 = x0 + NAME_W;
        let bx1 = x0 + w - 4.0;
        self.span = (bx0, bx1, len);
        let to_x = |t: f64| bx0 + (t / len).clamp(0.0, 1.0) * (bx1 - bx0);
        let top = y;
        self.rect(cx, Rect { pos: dvec2(bx0, y), size: dvec2(bx1 - bx0, RULER) }, self.color_ruler);
        let step = nice_step(len, bx1 - bx0);
        let mut t = 0.0;
        let mut tick_end = f64::NEG_INFINITY;
        while t <= len + 1e-9 {
            let x = to_x(t);
            self.rect(cx, Rect { pos: dvec2(x, y + RULER - 5.0), size: dvec2(1.0, 5.0) }, meta);
            let text = format!("{:.2}", t);
            let tw = crate::badge::measure(&self.draw_text, cx, &text);
            // Inside the ruler, never over the previous number.
            let tx = (x + 2.0).min(bx1 - tw);
            if tx > tick_end + 4.0 {
                self.text(cx, dvec2(tx, y + 1.0), &text, meta);
                tick_end = tx + tw;
            }
            t += step;
        }
        // The labels: a mark on the ruler, the name on one of two rows
        // under it (the first row it fits on without touching a name
        // already there, kept inside the timeline; no room, no name).
        let mut row_end = [f64::NEG_INFINITY; 2];
        let names_y = y + RULER;
        for (tag, at) in self.labels.clone() {
            let x = to_x(at);
            self.rect(cx, Rect { pos: dvec2(x - 1.0, y), size: dvec2(2.0, RULER + LABEL_ROW * 2.0) }, label_ink);
            let name = crate::widget_tree::live_id_token(LiveId(tag.0));
            let tw = crate::badge::measure(&self.draw_text, cx, &name);
            let tx = (x + 3.0).min(bx1 - tw).max(bx0);
            if let Some(r) = (0..2).find(|&r| tx > row_end[r] + 4.0) {
                self.text(cx, dvec2(tx, names_y + LABEL_ROW * r as f64), &name, label_ink);
                row_end[r] = tx + tw;
            }
        }
        y += RULER + LABEL_ROW * 2.0 + 2.0;

        let bottom = rect.pos.y + rect.size.y - PAD;
        let lanes = self.lanes.clone();
        // Thinner lanes (names hidden below a readable height) before any
        // is left out.
        let room = (bottom - y).max(0.0);
        let count = lanes.len().max(1) as f64;
        // The "+N more" line takes a row only when lanes are left out.
        let reserve = if count * LANE_MIN > room { ROW } else { 0.0 };
        let lane = ((room - reserve) / count).clamp(LANE_MIN, LANE);
        let named = lane >= 11.0;
        let mut hidden = 0usize;
        for n in &lanes {
            if y + lane > bottom - reserve {
                hidden += 1;
                continue;
            }
            let indent = (8.0 * n.depth as f64).min(NAME_W - 24.0);
            if named {
                let name = self.fit(cx, &node_name(n), NAME_W - indent - 4.0);
                self.text(cx, dvec2(x0 + indent, y + (lane - 11.0) * 0.5), &name, if n.depth == 0 { ink } else { meta });
            }
            let base = kind_color(n.kind);
            let start = n.local_start;
            let one = n.duration.max(0.0);
            let total = if n.total_duration < BIG { n.total_duration } else { len * 4.0 };
            if one <= 1e-9 {
                // A zero-length node (a call, a pause, a set): a tick.
                let x = to_x(start);
                self.rect(cx, Rect { pos: dvec2(x - 1.0, y + 1.0), size: dvec2(3.0, (lane - 2.0).max(1.0)) }, base);
            } else {
                // One bar per iteration (repeat delays show as gaps).
                let cycle = if n.repeat != 0 && n.total_duration < BIG && n.repeat > 0 {
                    (total - one) / n.repeat as f64
                } else {
                    one
                };
                let mut k = 0;
                let mut s = start;
                while s < start + total - 1e-9 && k < 256 {
                    let e = (s + one).min(start + total);
                    let (xa, xb) = (to_x(s), to_x(e));
                    if xb > xa {
                        let mut c = base;
                        if k > 0 {
                            c.w = 0.55;
                        }
                        let inset = (lane * 0.2).min(3.0);
                        self.rect(cx, Rect { pos: dvec2(xa, y + inset), size: dvec2((xb - xa).max(1.0), lane - inset * 2.0) }, c);
                    }
                    s += cycle.max(one);
                    k += 1;
                }
            }
            y += lane;
        }
        if hidden > 0 {
            self.text(cx, dvec2(x0, y + 1.0), &format!("+{hidden} more lanes"), meta);
        }
        self.track_rect = Rect { pos: dvec2(bx0, top), size: dvec2(bx1 - bx0, (y - top).max(RULER)) };
        let hx = to_x(head);
        self.rect(cx, Rect { pos: dvec2(hx - 0.5, top), size: dvec2(1.5, y - top) }, playhead);
        DrawStep::done()
    }
}
