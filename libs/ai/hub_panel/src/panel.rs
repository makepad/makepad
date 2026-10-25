//! The AI hub control panel: what the hub holds and does, drawn with the shell kit into a
//! body rect, plus the hits that turn clicks into [`HubCommand`]s. The WM's top-bar flyout
//! and the AI Hub app draw this very panel. The caller owns the window, the snapshot
//! source (a [`HubControl`](crate::hub_control::HubControl)) and what it does with a command.

use crate::hub_control::{compute_label, ComputeMode, HubCommand, HubSnapshot, KvPrecision, ModelKind, ModelState};
use crate::hub_glow::GlowTracker;
use makepad_shell_kit::ui::{cut_top, contains, rect, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;
use std::time::Instant;

/// A hit target inside the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubHit {
    /// A model row's Load / Unload button.
    Model(usize),
    /// The KV precision switch: true is q8.
    Kv(bool),
    /// The context size's step: true is the next size up.
    Context(bool),
    /// The hub child: stop and start it again.
    Restart,
    /// Open or close the list of chat models.
    PickOpen,
    /// A chat model of that list (index into the snapshot's `chat_models`).
    Pick(usize),
    /// A compute mode of the picker (index into `ComputeMode::ALL`).
    Compute(usize),
}

/// The panel's own state: the glow of working models, the hover, the hits of the last draw
/// and whether the chat model list is open.
#[derive(Default)]
pub struct HubPanel {
    /// Which hub models light up (see `hub_glow`).
    glow: GlowTracker,
    glow_clock: Option<Instant>,
    /// The frame loop that runs only while something glows.
    glow_frame: NextFrame,
    hot: Option<HubHit>,
    hits: Vec<(HubHit, Rect)>,
    /// The chat model list is open (the hub child: a pick restarts it).
    pick: bool,
}

impl HubPanel {
    /// Seconds on the glow clock (it starts with the first read).
    fn glow_now(&mut self) -> f64 {
        self.glow_clock.get_or_insert_with(Instant::now).elapsed().as_secs_f64()
    }

    /// A new hub snapshot: the glow follows it. True when the panel must redraw for the glow
    /// or a "used Ns ago" note (an idle hub asks for nothing); a changed snapshot is the
    /// caller's to see.
    pub fn update(&mut self, _cx: &mut Cx, snap: &HubSnapshot) -> bool {
        let now = self.glow_now();
        self.glow.update(snap, now);
        let notes = self.glow.notes_changed(now);
        notes || self.glow.animating(now)
    }

    /// True when `event` is the glow's next frame: the panel redraws.
    pub fn is_glow_frame(&self, event: &Event) -> bool {
        self.glow_frame.is_event(event).is_some()
    }

    /// The pointer moved: true when the hovered target changed (redraw).
    pub fn hover(&mut self, p: Vec2d) -> bool {
        let hot = self.hit_at(p);
        let changed = hot != self.hot;
        self.hot = hot;
        changed
    }

    /// The hovered target is gone (pointer left the panel).
    pub fn clear_hover(&mut self) -> bool {
        self.hot.take().is_some()
    }

    fn hit_at(&self, p: Vec2d) -> Option<HubHit> {
        self.hits.iter().find(|(_, r)| contains(*r, p)).map(|(h, _)| *h)
    }

    /// A press at `p`: the command it asks for, if any. The chat model list toggles here
    ///.
    pub fn click(&mut self, p: Vec2d, snap: &HubSnapshot) -> Option<HubCommand> {
        match self.hit_at(p)? {
            HubHit::Model(i) => {
                let m = snap.models.get(i)?;
                match m.state {
                    ModelState::Loaded => Some(HubCommand::Unload(m.id.clone())),
                    ModelState::Unloaded | ModelState::Error => Some(HubCommand::Load(m.id.clone())),
                    ModelState::Loading | ModelState::Downloading => None,
                }
            }
            HubHit::Context(up) => {
                let now = snap.context;
                let next = if up { hub_context_up(snap) } else { snap.context_options.iter().rev().copied().find(|c| *c < now) };
                next.map(HubCommand::SetContext)
            }
            HubHit::Restart => Some(HubCommand::Restart),
            HubHit::PickOpen => {
                self.pick = !self.pick;
                None
            }
            HubHit::Pick(i) => {
                let picked = hub_chat_models(snap).get(i).cloned();
                self.pick = false;
                picked.filter(|m| snap.process.as_ref().is_some_and(|p| p.model != *m)).map(HubCommand::SetModel)
            }
            HubHit::Compute(i) => {
                let mode = ComputeMode::ALL[i];
                snap.admission.as_ref().is_some_and(|a| a.mode != mode).then_some(HubCommand::SetCompute(mode))
            }
            HubHit::Kv(q8) => Some(HubCommand::SetKv(if q8 { KvPrecision::Q8 } else { KvPrecision::F16 })),
        }
    }

    /// Whether `p` is on one of the panel's targets as last drawn.
    pub fn hit_at_point(&self, p: Vec2d) -> bool {
        self.hit_at(p).is_some()
    }

    /// The hub child's Restart button at the right of `row`; its width.
    fn restart_button(&mut self, cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, row: Rect) -> f64 {
        let w = d.button_width(cx, tok, false, "Restart", tok.font.body_small);
        let r = rect(row.pos.x + row.size.x - w, row.pos.y + 3.0, w, row.size.y - 6.0);
        let state = if self.hot == Some(HubHit::Restart) { CtrlState::Hover } else { CtrlState::Normal };
        d.button(cx, r, tok, state, None, "Restart", tok.font.body_small, tok.popups.text, true);
        self.hits.push((HubHit::Restart, r));
        w
    }

    /// The panel's height for `snap`, hero included.
    pub fn height(&self, snap: &HubSnapshot) -> f64 {
        fn hub_compute_height(h: &HubSnapshot) -> f64 {
            match &h.admission {
                Some(a) => 22.0 + 28.0 + 18.0 + if a.mode == ComputeMode::AiFirst { 18.0 } else { 0.0 },
                None => 0.0,
            }
        }
        let h = snap;
        if !h.available {
            return match &h.process {
                Some(p) => 40.0 + 12.0 + p.phase.reason().map_or(0.0, |r| r.lines().take(HUB_REASON_LINES).count() as f64 * 16.0 + 6.0) + 34.0,
                None => 40.0 + 12.0 + 40.0,
            };
        }
        let policy = if h.policy.is_some() { 18.0 } else { 0.0 } + hub_compute_height(h);
        let process = if h.process.is_some() { 30.0 + if self.pick { hub_chat_models(h).len() as f64 * 26.0 } else { 0.0 } } else { 0.0 };
        let activity = h.activity.len().max(1) as f64 * 20.0;
        let recent = h.recent.len().min(3) as f64 * 16.0;
        let context = if h.models.iter().any(|m| Some(&m.id) == h.chat_model.as_ref()) { 30.0 } else { 0.0 };
        40.0 + 12.0 + 38.0 + policy + process + 22.0 + h.models.len() as f64 * 34.0 + context + 12.0 + 22.0 + activity + 20.0 + recent
    }

    /// Draw the panel into `body` (hero, separator, sections), the way the WM flyout does.
    pub fn draw(&mut self, cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, snap: &HubSnapshot, body: Rect) {
        self.hits.clear();
        let tok = *tok;
        let fg = tok.popups.text;
        let hub = snap;
        let now = self.glow_now();
        if self.glow.animating(now) {
            self.glow_frame = cx.new_next_frame();
        }
        let (hero, rest) = cut_top(body, 40.0);
        let status = hub.status();
        d.panel_hero(cx, hero, &tok, fg, Ico::Chip, "AI hub", &status, 0.0);
        let (sep, rest) = cut_top(rest, 12.0);
        d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        if !hub.available {
            match &hub.process {
                // The WM's own hub on its way up, or down: why, and Restart.
                Some(p) => {
                    let mut rest = rest;
                    if let Some(reason) = p.phase.reason() {
                        for line in reason.lines().take(HUB_REASON_LINES) {
                            let (row, r2) = cut_top(rest, 16.0);
                            rest = r2;
                            d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, line);
                        }
                        rest = cut_top(rest, 6.0).1;
                    }
                    let (row, _) = cut_top(rest, 34.0);
                    self.restart_button(cx, d, &tok, row);
                }
                None => unavailable(cx, d, &tok, rest, "Start the AI hub to see its models."),
            }
            return;
        }
        // GPU memory.
        let (row, rest) = cut_top(rest, 22.0);
        let gib = |mb: u32| mb as f64 / 1024.0;
        d.label(cx, row, true, tok.font.caption, darker(fg, 1.4), HAlign::Left, "GPU memory");
        d.label(cx, row, false, tok.font.caption, fg, HAlign::Right,
            &format!("{:.1} / {:.1} GiB, {:.1} free", gib(hub.gpu_used_mb), gib(hub.gpu_total_mb), gib(hub.gpu_total_mb.saturating_sub(hub.gpu_used_mb))));
        let (bar_row, rest) = cut_top(rest, 16.0);
        let track = rect(bar_row.pos.x, bar_row.pos.y + 4.0, bar_row.size.x, 8.0);
        d.solid(cx, track, alpha(fg, 0.12));
        let used = if hub.gpu_total_mb > 0 { (hub.gpu_used_mb as f64 / hub.gpu_total_mb as f64).clamp(0.0, 1.0) } else { 0.0 };
        d.solid(cx, rect(track.pos.x, track.pos.y, track.size.x * used, track.size.y), fg);
        // Models, grouped by what they do (the hub sorts them): a type icon, the name, one
        // line on what it does with its state and GPU footprint, Load / Unload. The chat
        // model's context setting sits under it, capped by what the GPU budget allows.
        // What the hub's memory policy did last: a variant run, a chat model unloaded for a job.
        let rest = match &hub.policy {
            Some(note) => {
                let (row, rest) = cut_top(rest, 18.0);
                d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, note);
                rest
            }
            None => rest,
        };
        // Compute jobs: the node's mode (saved in ai-hub.conf) and what it makes of now.
        let rest = match &hub.admission {
            Some(a) => {
                let (row, rest) = cut_top(rest, 22.0);
                d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Compute jobs");
                let (row, rest) = cut_top(rest, 28.0);
                let w = row.size.x / ComputeMode::ALL.len() as f64;
                for (i, mode) in ComputeMode::ALL.into_iter().enumerate() {
                    let r = rect(row.pos.x + i as f64 * w + 1.0, row.pos.y + 2.0, w - 2.0, row.size.y - 4.0);
                    let state = if a.mode == mode { CtrlState::Selected } else if self.hot == Some(HubHit::Compute(i)) { CtrlState::Hover } else { CtrlState::Normal };
                    d.button(cx, r, &tok, state, None, compute_label(mode), tok.font.body_small, fg, true);
                    self.hits.push((HubHit::Compute(i), r));
                }
                let (row, mut rest) = cut_top(rest, 18.0);
                d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, &a.line());
                if a.mode == ComputeMode::AiFirst {
                    let (row, r2) = cut_top(rest, 18.0);
                    rest = r2;
                    d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, AI_FIRST_WARNING);
                }
                rest
            }
            None => rest,
        };
        // The WM's own hub: the chat model it loads at start (a pick restarts it), and Restart.
        let rest = match &hub.process {
            Some(p) => {
                let (row, mut rest) = cut_top(rest, 30.0);
                let restart_w = self.restart_button(cx, d, &tok, row);
                let pick_w = d.button_width(cx, &tok, false, "Change", tok.font.body_small);
                let pick = rect(row.pos.x + row.size.x - restart_w - 6.0 - pick_w, row.pos.y + 3.0, pick_w, row.size.y - 6.0);
                let state = if self.pick { CtrlState::Selected } else if self.hot == Some(HubHit::PickOpen) { CtrlState::Hover } else { CtrlState::Normal };
                d.button(cx, pick, &tok, state, None, "Change", tok.font.body_small, fg, true);
                self.hits.push((HubHit::PickOpen, pick));
                d.label(cx, rect(row.pos.x, row.pos.y, 80.0, row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Chat model");
                let name = if p.model.is_empty() { "none at start" } else { p.model.as_str() };
                d.label_elided(cx, rect(row.pos.x + 80.0, row.pos.y, (pick.pos.x - 8.0 - row.pos.x - 80.0).max(0.0), row.size.y), false, tok.font.body_small, fg, HAlign::Left, name);
                if self.pick {
                    for (i, id) in hub_chat_models(hub).iter().enumerate() {
                        let (row, r2) = cut_top(rest, 26.0);
                        rest = r2;
                        let r = rect(row.pos.x + 80.0, row.pos.y + 2.0, row.size.x - 80.0, row.size.y - 4.0);
                        let state = if *id == p.model { CtrlState::Selected } else if self.hot == Some(HubHit::Pick(i)) { CtrlState::Hover } else { CtrlState::Normal };
                        d.button(cx, r, &tok, state, None, id, tok.font.body_small, fg, false);
                        self.hits.push((HubHit::Pick(i), r));
                    }
                }
                rest
            }
            None => rest,
        };
        let (head, mut rest) = cut_top(rest, 22.0);
        d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Models");
        for (i, m) in hub.models.iter().enumerate() {
            let (row, r2) = cut_top(rest, 34.0);
            rest = r2;
            let (label, state, disabled) = match m.state {
                ModelState::Loaded => ("Unload", CtrlState::Normal, false),
                ModelState::Unloaded | ModelState::Error => ("Load", CtrlState::Normal, false),
                ModelState::Loading => ("Loading", CtrlState::Disabled, true),
                ModelState::Downloading => ("Getting", CtrlState::Disabled, true),
            };
            // Three columns that never overlap: [icon + name / what it does] takes what is
            // left and elides, [state / GPU size] and [Load] have fixed widths on the right.
            const GAP: f64 = 10.0;
            const ICON: f64 = 18.0;
            let g = self.glow.row(&m.id, now);
            let k = (0.5 - 0.5 * (now * std::f64::consts::TAU / 1.4).cos()) as f32;
            let lit = g.strength * if g.working { 0.8 + 0.2 * k } else { 1.0 };
            if g.strength > 0.0 {
                d.solid(cx, rect(row.pos.x - 6.0, row.pos.y + 1.0, row.size.x + 12.0, row.size.y - 2.0), vec4(0.30, 0.78, 0.42, 0.95 * lit));
            }
            let w = d.button_width(cx, &tok, false, "Unload", tok.font.body_small);
            let button = rect(row.pos.x + row.size.x - w, row.pos.y + 4.0, w, row.size.y - 8.0);
            let state = if !disabled && self.hot == Some(HubHit::Model(i)) { CtrlState::Hover } else { state };
            let lit_now = self.glow.row(&m.id, now).strength;
            let label_fg = if lit_now > 0.0 { lerp_color(fg, vec4(0.02, 0.14, 0.06, 1.0), lit_now) } else { fg };
            if lit_now > 0.0 {
                // On the green the button is a darker pad with dark ink (the themed
                // button's own label colour is light).
                d.solid(cx, button, vec4(0.02, 0.14, 0.06, 0.14 * lit_now));
                d.label(cx, button, true, tok.font.body_small, label_fg, HAlign::Center, label);
            } else {
                d.button(cx, button, &tok, state, None, label, tok.font.body_small, label_fg, true);
            }
            if !disabled {
                self.hits.push((HubHit::Model(i), button));
            }
            let state_w = ["downloading", "on disk", "loaded", "loading", "failed", "99.9 GiB"]
                .iter()
                .map(|t| d.measure(cx, false, tok.font.caption, t))
                .fold(0.0f64, f64::max)
                .ceil();
            let state_col = rect(button.pos.x - GAP - state_w, row.pos.y, state_w, row.size.y);
            let dim = if m.state == ModelState::Loaded { fg } else { darker(fg, 1.4) };
            // A working model lights up: a soft wash and edge in the accent that breathes,
            // an accent icon and name, a live line. It fades out after the work.
            // Computing = a calm green row (brightness breathes), dark text on it.
            let ink = vec4(0.02, 0.14, 0.06, 1.0);
            let dim = if g.strength > 0.0 { lerp_color(dim, ink, g.strength) } else { dim };
            let sub = if g.strength > 0.0 { lerp_color(darker(fg, 1.4), ink, g.strength) } else { darker(fg, 1.4) };
            d.icon(cx, hub_icon(m.kind), rect(row.pos.x, row.pos.y + (row.size.y - ICON) * 0.5, ICON, ICON), dim);
            let text_x = row.pos.x + ICON + 8.0;
            let text = rect(text_x, row.pos.y, (state_col.pos.x - GAP - text_x).max(0.0), row.size.y);
            let (top, bottom) = (rect(text.pos.x, text.pos.y + 2.0, text.size.x, 15.0), rect(text.pos.x, text.pos.y + 17.0, text.size.x, 14.0));
            d.label_elided(cx, top, false, tok.font.body_small, dim, HAlign::Left, &m.name);
            if let Some(live) = &g.live {
                d.label_elided(cx, bottom, false, tok.font.caption, ink, HAlign::Left, live);
            } else if let Some(used) = &g.used {
                d.label_elided(cx, bottom, false, tok.font.caption, sub, HAlign::Left, &format!("{} \u{b7} {}", m.kind.what(), used));
            } else {
                d.label_elided(cx, bottom, false, tok.font.caption, sub, HAlign::Left, m.kind.what());
            }
            let state = match m.state {
                ModelState::Loaded => "loaded",
                ModelState::Loading => "loading",
                ModelState::Downloading => "downloading",
                ModelState::Error => "failed",
                ModelState::Unloaded => "on disk",
            };
            let (stop, sbottom) = (rect(state_col.pos.x, top.pos.y, state_col.size.x, top.size.y), rect(state_col.pos.x, bottom.pos.y, state_col.size.x, bottom.size.y));
            d.label(cx, stop, false, tok.font.caption, dim, HAlign::Right, state);
            if m.state == ModelState::Loaded && m.gpu_mb > 0 {
                d.label(cx, sbottom, false, tok.font.caption, sub, HAlign::Right, &format!("{:.1} GiB", gib(m.gpu_mb)));
            }
            if Some(&m.id) != hub.chat_model.as_ref() {
                continue;
            }
            // This model's context: a step down / up through the sizes the hub offers, up
            // to what fits next to the UI; then the KV precision.
            let (row, r2) = cut_top(rest, 30.0);
            rest = r2;
            let step = d.button_width(cx, &tok, false, "+", tok.font.body_small).max(24.0);
            let mut x = row.pos.x + 26.0;
            d.label_elided(cx, rect(x, row.pos.y, 52.0, row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Context");
            x += 56.0;
            let can_up = hub_context_up(hub).is_some();
            for (label, up) in [("-", false), ("+", true)] {
                let r = rect(x, row.pos.y + 3.0, step, row.size.y - 6.0);
                let enabled = if up { can_up } else { hub.context_options.iter().any(|c| *c < hub.context) };
                let state = if !enabled { CtrlState::Disabled } else if self.hot == Some(HubHit::Context(up)) { CtrlState::Hover } else { CtrlState::Normal };
                d.button(cx, r, &tok, state, None, label, tok.font.body_small, fg, true);
                if enabled {
                    self.hits.push((HubHit::Context(up), r));
                }
                x += step + 4.0;
                if !up {
                    d.label(cx, rect(x, row.pos.y, 48.0, row.size.y), true, tok.font.body_small, fg, HAlign::Center, &kilo(hub.context));
                    x += 48.0 + 4.0;
                }
            }
            let max = match m.context_fit {
                Some(0) => "no room next to the UI".to_string(),
                Some(fit) => format!("max {}", kilo(fit)),
                None => String::new(),
            };
            let kw = d.button_width(cx, &tok, false, "f16", tok.font.body_small);
            d.label_elided(cx, rect(x + 4.0, row.pos.y, ((row.pos.x + row.size.x - 2.0 * kw - 10.0) - (x + 4.0)).max(0.0), row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &max);
            for (k, q8) in [(KvPrecision::F16, false), (KvPrecision::Q8, true)] {
                let kx = row.pos.x + row.size.x - kw * (2.0 - q8 as u8 as f64);
                let r = rect(kx, row.pos.y + 3.0, kw, row.size.y - 6.0);
                let state = if hub.kv == k { CtrlState::Selected } else if self.hot == Some(HubHit::Kv(q8)) { CtrlState::Hover } else { CtrlState::Normal };
                d.button(cx, r, &tok, state, None, k.name(), tok.font.body_small, fg, true);
                self.hits.push((HubHit::Kv(q8), r));
            }
        }
        // Activity.
        let (sep, rest) = cut_top(rest, 12.0);
        d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (head, mut rest) = cut_top(rest, 22.0);
        d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Working now");
        if hub.activity.is_empty() {
            let (row, r2) = cut_top(rest, 20.0);
            rest = r2;
            d.label(cx, row, false, tok.font.body_small, darker(fg, 1.4), HAlign::Left, "Nothing");
        }
        for a in &hub.activity {
            let (row, r2) = cut_top(rest, 20.0);
            rest = r2;
            let rate = match (a.tokens_per_s, a.progress) {
                (Some(t), _) => format!("{:.0} tok/s", t),
                (None, Some(p)) => format!("{:.0}%", p * 100.0),
                _ => String::new(),
            };
            let rate_w = d.measure(cx, false, tok.font.body_small, &rate);
            let left = rect(row.pos.x, row.pos.y, (row.size.x - rate_w - 10.0).max(0.0), row.size.y);
            d.label_elided(cx, left, false, tok.font.body_small, fg, HAlign::Left, &format!("{} for {}", a.model, a.app));
            d.label(cx, row, false, tok.font.body_small, fg, HAlign::Right, &rate);
        }
        // Recent requests.
        let (head, mut rest) = cut_top(rest, 20.0);
        d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Recent");
        for q in hub.recent.iter().take(3) {
            let (row, r2) = cut_top(rest, 16.0);
            rest = r2;
            let r = if q.ok { format!("{} ms", q.millis) } else { format!("{} ms, failed", q.millis) };
            let r_w = d.measure(cx, false, tok.font.caption, &r);
            let left = rect(row.pos.x, row.pos.y, (row.size.x - r_w - 10.0).max(0.0), row.size.y);
            d.label_elided(cx, left, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &format!("{} for {}", q.model, q.app));
            d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Right, &r);
        }
    }

}

fn unavailable(cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, r: Rect, what: &str) {
    d.label(cx, r, false, tok.font.body, alpha(tok.popups.text, 0.6), HAlign::Left, what);
}

fn lerp_color(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    let t = t.clamp(0.0, 1.0);
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

/// The panel's icon for what a hub model does.
fn hub_icon(kind: ModelKind) -> Ico {
    match kind {
        ModelKind::Chat => Ico::Prompt,
        ModelKind::Vision => Ico::Photo,
        ModelKind::SpeechToText => Ico::Mic,
        ModelKind::TextToSpeech => Ico::Speaker,
        ModelKind::Image => Ico::Palette,
        ModelKind::Music => Ico::Pulse,
        ModelKind::Stems => Ico::Headphone,
        ModelKind::Video => Ico::Play,
        ModelKind::Mesh | ModelKind::Other => Ico::Chip,
    }
}

/// The next larger context the hub offers that still fits next to the UI (the chat
/// model's `context_fit`; unknown = not capped).
fn hub_context_up(hub: &HubSnapshot) -> Option<u32> {
    let fit = hub.models.iter().find(|m| Some(&m.id) == hub.chat_model.as_ref()).and_then(|m| m.context_fit);
    hub.context_options.iter().copied().find(|c| *c > hub.context && fit.is_none_or(|f| *c <= f))
}

/// Lines of a hub crash's or failed build's reason the panel shows.
const HUB_REASON_LINES: usize = 6;
/// Under the AI First option: it never gives GPU memory back to the desktop.
const AI_FIRST_WARNING: &str = "AI keeps its GPU memory: the desktop and games may slow down or fail.";

/// The chat models of the hub's registry, for the hub child's chat model pick.
fn hub_chat_models(hub: &HubSnapshot) -> Vec<String> {
    hub.chat_models.clone()
}

/// 49152 -> "48k".
fn kilo(tokens: u32) -> String {
    if tokens >= 1024 && tokens % 1024 == 0 { format!("{}k", tokens / 1024) } else { tokens.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_control::ModelInfo;

    fn hub(fit: Option<u32>) -> HubSnapshot {
        HubSnapshot {
            available: true,
            models: vec![ModelInfo { id: "q".into(), name: "q".into(), kind: ModelKind::Chat, state: ModelState::Loaded, context_fit: fit, gpu_mb: 0, device: String::new() }],
            context: 32768,
            chat_model: Some("q".into()),
            context_options: vec![16384, 32768, 49152, 65536],
            ..Default::default()
        }
    }

    #[test]
    fn the_context_steps_up_only_as_far_as_the_budget_allows() {
        assert_eq!(hub_context_up(&hub(Some(57344))), Some(49152));
        assert_eq!(hub_context_up(&hub(Some(40000))), None);
        assert_eq!(hub_context_up(&hub(None)), Some(49152), "not known yet: not capped");
        assert_eq!(kilo(49152), "48k");
        assert_eq!(hub_icon(ModelKind::SpeechToText), Ico::Mic);
    }

    #[test]
    fn a_press_becomes_the_command_of_its_target() {
        let mut p = HubPanel::default();
        let snap = hub(None);
        let at = dvec2(5.0, 5.0);
        p.hits.push((HubHit::Model(0), rect(0.0, 0.0, 10.0, 10.0)));
        assert_eq!(p.click(at, &snap), Some(HubCommand::Unload("q".into())));
        p.hits = vec![(HubHit::Context(true), rect(0.0, 0.0, 10.0, 10.0))];
        assert_eq!(p.click(at, &snap), Some(HubCommand::SetContext(49152)));
        p.hits = vec![(HubHit::PickOpen, rect(0.0, 0.0, 10.0, 10.0))];
        assert_eq!(p.click(at, &snap), None);
        assert!(p.pick, "the pick list opens");
        assert!(p.hover(at) && !p.hover(at), "hover reports a change once");
        assert_eq!(p.click(dvec2(50.0, 50.0), &snap), None, "outside every target");
    }
}
