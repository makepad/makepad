//! The AI hub control panel: what the hub holds and does, drawn with the shell kit into a
//! body rect, plus the hits that turn clicks into [`HubCommand`]s. The AI Hub app's window
//! draws it (on a Mac, on Windows and as a makepad WM node's hub). The caller owns the window,
//! the snapshot (`hub_control::parse_snapshot` of the hub's `GET /control`) and what it does
//! with a command.

use crate::hub_control::{compute_label, CatalogueModel, ComputeMode, Get, HubCommand, HubSnapshot, KvPrecision, LicenceGate, ModelInfo, ModelKind, ModelState};
use crate::hub_glow::GlowTracker;
use makepad_shell_kit::ui::{cut_top, contains, rect, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;
use std::time::Instant;

/// A hit target inside the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubHit {
    /// A model row's button (Load / Unload / Download / Cancel / Retry), by row.
    Model(usize),
    /// A model row's Delete button.
    Delete(usize),
    /// The open confirm's yes (Download anyway / Delete) and no.
    ConfirmYes,
    ConfirmNo,
    /// Show or hide the models that are not for this machine.
    Elsewhere,
    /// The KV precision switch: true is q8.
    Kv(bool),
    /// The context size's step: true is the next size up.
    Context(bool),
    /// A compute mode of the picker (index into `ComputeMode::ALL`).
    Compute(usize),
}

/// The panel's own state: the glow of working models, the hover, the hits of the last draw
/// and the open licence or delete question.
#[derive(Default)]
pub struct HubPanel {
    /// Which hub models light up (see `hub_glow`).
    glow: GlowTracker,
    glow_clock: Option<Instant>,
    /// The frame loop that runs only while something glows.
    glow_frame: NextFrame,
    hot: Option<HubHit>,
    hits: Vec<(HubHit, Rect)>,
    /// The licence or delete question open under a model row.
    confirm: Option<Confirm>,
    /// The models that are not for this machine are listed.
    elsewhere: bool,
}

/// A question the panel asks before a command.
#[derive(Clone, Debug, PartialEq)]
enum Confirm {
    /// Download a model whose licence needs a yes.
    Licence { id: String, class: String },
    /// Delete a model's files, freeing `bytes`.
    Delete { id: String, bytes: u64 },
}

impl Confirm {
    fn id(&self) -> &str {
        match self {
            Confirm::Licence { id, .. } | Confirm::Delete { id, .. } => id,
        }
    }
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

    /// A press at `p`: the command it asks for, if any.
    pub fn click(&mut self, p: Vec2d, snap: &HubSnapshot) -> Option<HubCommand> {
        match self.hit_at(p)? {
            HubHit::Model(i) => {
                let rows = model_rows(snap);
                let r = rows.get(i)?;
                let id = r.id.to_string();
                match primary(r).1 {
                    Act::Unload => Some(HubCommand::Unload(id)),
                    Act::Load => Some(HubCommand::Load(id)),
                    Act::Cancel => Some(HubCommand::CancelDownload(id)),
                    Act::Download => match &r.cat?.gate {
                        LicenceGate::Open => Some(HubCommand::Download { id, confirmed: false }),
                        LicenceGate::Confirm(class) => {
                            self.confirm = Some(Confirm::Licence { id, class: class.clone() });
                            None
                        }
                        LicenceGate::NeedsToken => None,
                    },
                    Act::Busy | Act::None => None,
                }
            }
            HubHit::Delete(i) => {
                let rows = model_rows(snap);
                let r = rows.get(i).filter(|r| deletable(r, snap))?;
                self.confirm = Some(Confirm::Delete { id: r.id.to_string(), bytes: r.cat?.disk_bytes });
                None
            }
            HubHit::ConfirmYes => match self.confirm.take()? {
                Confirm::Licence { id, .. } => Some(HubCommand::Download { id, confirmed: true }),
                Confirm::Delete { id, .. } => Some(HubCommand::Delete(id)),
            },
            HubHit::ConfirmNo => {
                self.confirm = None;
                None
            }
            HubHit::Elsewhere => {
                self.elsewhere = !self.elsewhere;
                None
            }
            HubHit::Context(up) => {
                let now = snap.context;
                let next = if up { hub_context_up(snap) } else { snap.context_options.iter().rev().copied().find(|c| *c < now) };
                next.map(HubCommand::SetContext)
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

    /// The Models section into `rest`: what the hub serves and, with a catalogue (the AI
    /// Hub app), every local model this machine can run. Grouped by what they do: a type
    /// icon, the name, one line on what it does (licence, size), its state, and its
    /// buttons (Load / Unload, Download / Cancel / Retry, Delete). The chat model's context
    /// setting sits under it; a licence or delete confirm under its row.
    fn draw_models(&mut self, cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, hub: &HubSnapshot, rest: Rect, now: f64) -> Rect {
        let tok = *tok;
        let fg = tok.popups.text;
        let gib = |mb: u32| mb as f64 / 1024.0;
        let (head, mut rest) = cut_top(rest, 22.0);
        d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Models");
        if let Some(note) = hub.catalogue.as_ref().and_then(|c| c.note.as_ref()) {
            let (row, r2) = cut_top(rest, 18.0);
            rest = r2;
            d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, note);
        }
        let with_catalogue = hub.catalogue.is_some();
        let rows = model_rows(hub);
        const GAP: f64 = 10.0;
        const ICON: f64 = 18.0;
        let primary_w = if with_catalogue {
            ["Download", "Unload", "Loading", "Cancel", "Retry"].iter().map(|l| d.button_width(cx, &tok, false, l, tok.font.body_small)).fold(0.0f64, f64::max)
        } else {
            d.button_width(cx, &tok, false, "Unload", tok.font.body_small)
        };
        let delete_w = if with_catalogue { d.button_width(cx, &tok, false, "Delete", tok.font.body_small) } else { 0.0 };
        let state_texts: &[&str] = if with_catalogue {
            &["downloading", "on disk", "loaded", "loading", "failed", "99.9 GiB", "not downloaded", "999.9 GB"]
        } else {
            &["downloading", "on disk", "loaded", "loading", "failed", "99.9 GiB"]
        };
        let state_w = state_texts.iter().map(|t| d.measure(cx, false, tok.font.caption, t)).fold(0.0f64, f64::max).ceil();
        for (i, r) in rows.iter().enumerate() {
            let (row, r2) = cut_top(rest, 34.0);
            rest = r2;
            let (label, act) = primary(r);
            let disabled = matches!(act, Act::Busy | Act::None);
            // Columns that never overlap: [icon + name / what it does] takes what is left and
            // elides; [state / size], [Delete] and [the primary button] have fixed widths.
            let g = self.glow.row(r.id, now);
            let k = (0.5 - 0.5 * (now * std::f64::consts::TAU / 1.4).cos()) as f32;
            let lit = g.strength * if g.working { 0.8 + 0.2 * k } else { 1.0 };
            if g.strength > 0.0 {
                d.solid(cx, rect(row.pos.x - 6.0, row.pos.y + 1.0, row.size.x + 12.0, row.size.y - 2.0), vec4(0.30, 0.78, 0.42, 0.95 * lit));
            }
            let button = rect(row.pos.x + row.size.x - primary_w, row.pos.y + 4.0, primary_w, row.size.y - 8.0);
            let lit_now = g.strength;
            let label_fg = if lit_now > 0.0 { lerp_color(fg, vec4(0.02, 0.14, 0.06, 1.0), lit_now) } else { fg };
            if act != Act::None {
                let state = if disabled { CtrlState::Disabled } else if self.hot == Some(HubHit::Model(i)) { CtrlState::Hover } else { CtrlState::Normal };
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
            }
            let mut right = button.pos.x;
            if with_catalogue {
                let del = rect(button.pos.x - 6.0 - delete_w, row.pos.y + 4.0, delete_w, row.size.y - 8.0);
                right = del.pos.x;
                if deletable(r, hub) {
                    let state = if self.hot == Some(HubHit::Delete(i)) { CtrlState::Hover } else { CtrlState::Normal };
                    d.button(cx, del, &tok, state, None, "Delete", tok.font.body_small, fg, true);
                    self.hits.push((HubHit::Delete(i), del));
                }
            }
            let state_col = rect(right - GAP - state_w, row.pos.y, state_w, row.size.y);
            let loaded = r.served.is_some_and(|m| m.state == ModelState::Loaded);
            let dim = if loaded { fg } else { darker(fg, 1.4) };
            // A working model lights up: a calm green row (brightness breathes), dark text on it.
            let ink = vec4(0.02, 0.14, 0.06, 1.0);
            let dim = if g.strength > 0.0 { lerp_color(dim, ink, g.strength) } else { dim };
            let sub = if g.strength > 0.0 { lerp_color(darker(fg, 1.4), ink, g.strength) } else { darker(fg, 1.4) };
            d.icon(cx, hub_icon(r.kind), rect(row.pos.x, row.pos.y + (row.size.y - ICON) * 0.5, ICON, ICON), dim);
            let text_x = row.pos.x + ICON + 8.0;
            let text = rect(text_x, row.pos.y, (state_col.pos.x - GAP - text_x).max(0.0), row.size.y);
            let (top, bottom) = (rect(text.pos.x, text.pos.y + 2.0, text.size.x, 15.0), rect(text.pos.x, text.pos.y + 17.0, text.size.x, 14.0));
            d.label_elided(cx, top, false, tok.font.body_small, dim, HAlign::Left, r.served.map_or(r.id, |m| m.name.as_str()));
            if let Some(live) = &g.live {
                d.label_elided(cx, bottom, false, tok.font.caption, ink, HAlign::Left, live);
            } else {
                d.label_elided(cx, bottom, false, tok.font.caption, sub, HAlign::Left, &sub_line(r, g.used.as_deref()));
            }
            let (state, size) = state_text(r, gib);
            let (stop, sbottom) = (rect(state_col.pos.x, top.pos.y, state_col.size.x, top.size.y), rect(state_col.pos.x, bottom.pos.y, state_col.size.x, bottom.size.y));
            d.label(cx, stop, false, tok.font.caption, dim, HAlign::Right, &state);
            if !size.is_empty() {
                d.label(cx, sbottom, false, tok.font.caption, sub, HAlign::Right, &size);
            }
            // A download's progress: a thin bar under the text.
            if let Some(Get::Downloading { done, total }) = r.cat.filter(|_| r.served.is_none()).map(|c| &c.get) {
                let f = if *total > 0 { (*done as f64 / *total as f64).clamp(0.0, 1.0) } else { 0.0 };
                let track = rect(text.pos.x, row.pos.y + row.size.y - 3.0, (right - GAP - text.pos.x).max(0.0), 2.0);
                d.solid(cx, track, alpha(fg, 0.12));
                d.solid(cx, rect(track.pos.x, track.pos.y, track.size.x * f, track.size.y), fg);
            }
            // The licence or delete confirm of this row.
            if let Some(c) = self.confirm.as_ref().filter(|c| c.id() == r.id) {
                let (row, r2) = cut_top(rest, 30.0);
                rest = r2;
                let (question, yes) = match c {
                    Confirm::Licence { class, .. } => (format!("This model's licence is {class}: Download anyway?"), "Download"),
                    Confirm::Delete { id, bytes } => (format!("Delete {id} and free {}?", gb(*bytes)), "Delete"),
                };
                let no = "Cancel";
                let yes_w = d.button_width(cx, &tok, false, yes, tok.font.body_small);
                let no_w = d.button_width(cx, &tok, false, no, tok.font.body_small);
                let no_r = rect(row.pos.x + row.size.x - no_w, row.pos.y + 3.0, no_w, row.size.y - 6.0);
                let yes_r = rect(no_r.pos.x - 6.0 - yes_w, row.pos.y + 3.0, yes_w, row.size.y - 6.0);
                for (hit, r, l) in [(HubHit::ConfirmYes, yes_r, yes), (HubHit::ConfirmNo, no_r, no)] {
                    let state = if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
                    d.button(cx, r, &tok, state, None, l, tok.font.body_small, fg, true);
                    self.hits.push((hit, r));
                }
                d.label_elided(cx, rect(row.pos.x + 26.0, row.pos.y, (yes_r.pos.x - 8.0 - row.pos.x - 26.0).max(0.0), row.size.y), false, tok.font.caption, fg, HAlign::Left, &question);
            }
            let Some(m) = r.served.filter(|m| Some(&m.id) == hub.chat_model.as_ref()) else { continue };
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
        // The registry models this machine cannot run: a count, the list on request.
        if let Some(c) = hub.catalogue.as_ref().filter(|c| !c.elsewhere.is_empty()) {
            let (row, r2) = cut_top(rest, 26.0);
            rest = r2;
            let label = if self.elsewhere { "Hide" } else { "Show" };
            let w = d.button_width(cx, &tok, false, label, tok.font.body_small);
            let b = rect(row.pos.x + row.size.x - w, row.pos.y + 3.0, w, row.size.y - 6.0);
            let state = if self.hot == Some(HubHit::Elsewhere) { CtrlState::Hover } else { CtrlState::Normal };
            d.button(cx, b, &tok, state, None, label, tok.font.body_small, fg, true);
            self.hits.push((HubHit::Elsewhere, b));
            let n = c.elsewhere.len();
            d.label_elided(cx, rect(row.pos.x, row.pos.y, (b.pos.x - 8.0 - row.pos.x).max(0.0), row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &format!("{n} more in the registry {} not for this machine", if n == 1 { "is" } else { "are" }));
            if self.elsewhere {
                for (id, why) in &c.elsewhere {
                    let (row, r2) = cut_top(rest, 16.0);
                    rest = r2;
                    d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &format!("{id}: {}", why.lines().next().unwrap_or("")));
                }
            }
        }
        rest
    }

    /// The Models section's height for `hub` (see [`Self::draw_models`]).
    fn models_height(&self, hub: &HubSnapshot) -> f64 {
        let rows = model_rows(hub);
        let note = if hub.catalogue.as_ref().is_some_and(|c| c.note.is_some()) { 18.0 } else { 0.0 };
        let context = if rows.iter().any(|r| r.served.is_some_and(|m| Some(&m.id) == hub.chat_model.as_ref())) { 30.0 } else { 0.0 };
        let confirm = if self.confirm.as_ref().is_some_and(|c| rows.iter().any(|r| r.id == c.id())) { 30.0 } else { 0.0 };
        let elsewhere = match hub.catalogue.as_ref().filter(|c| !c.elsewhere.is_empty()) {
            Some(c) => 26.0 + if self.elsewhere { c.elsewhere.len() as f64 * 16.0 } else { 0.0 },
            None => 0.0,
        };
        22.0 + note + rows.len() as f64 * 34.0 + context + confirm + elsewhere
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
            return if h.catalogue.is_some() { 40.0 + 12.0 + 22.0 + self.models_height(h) } else { 40.0 + 12.0 + 40.0 };
        }
        let policy = if h.policy.is_some() { 18.0 } else { 0.0 } + hub_compute_height(h);
        let activity = h.activity.len().max(1) as f64 * 20.0;
        let recent = h.recent.len().min(3) as f64 * 16.0;
        40.0 + 12.0 + 38.0 + policy + self.models_height(h) + 12.0 + 22.0 + activity + 20.0 + recent
    }

    /// Draw the panel into `body` (hero, separator, sections).
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
            // The AI Hub app: the hub is off, the models on this machine still show.
            if hub.catalogue.is_some() {
                let (row, rest) = cut_top(rest, 22.0);
                unavailable(cx, d, &tok, row, "Not serving now. The models this machine can run:");
                self.draw_models(cx, d, &tok, hub, rest, now);
            } else {
                unavailable(cx, d, &tok, rest, "Start the AI hub to see its models.");
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
        let rest = self.draw_models(cx, d, &tok, hub, rest, now);
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

/// One row of the Models section: a model the hub serves, a catalogue model, or both.
struct Row<'a> {
    id: &'a str,
    kind: ModelKind,
    served: Option<&'a ModelInfo>,
    cat: Option<&'a CatalogueModel>,
}

/// The Models section's rows: what the hub serves, then (merged by id) the catalogue;
/// grouped by what they do, by name within a group.
fn model_rows(snap: &HubSnapshot) -> Vec<Row<'_>> {
    let mut rows: Vec<Row> = if snap.available { snap.models.iter().map(|m| Row { id: &m.id, kind: m.kind, served: Some(m), cat: None }).collect() } else { Vec::new() };
    if let Some(c) = &snap.catalogue {
        for cm in &c.models {
            match rows.iter_mut().find(|r| r.id == cm.id) {
                Some(r) => r.cat = Some(cm),
                None => rows.push(Row { id: &cm.id, kind: cm.kind, served: None, cat: Some(cm) }),
            }
        }
        rows.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(b.id)));
    }
    rows
}

/// What a row's main button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Load,
    Unload,
    Download,
    Cancel,
    /// Shown, disabled (loading, getting, a gated model without a token).
    Busy,
    /// No button.
    None,
}

fn primary(r: &Row) -> (&'static str, Act) {
    if let Some(m) = r.served {
        return match m.state {
            ModelState::Loaded => ("Unload", Act::Unload),
            ModelState::Unloaded | ModelState::Error => ("Load", Act::Load),
            ModelState::Loading => ("Loading", Act::Busy),
            ModelState::Downloading => ("Getting", Act::Busy),
        };
    }
    let Some(c) = r.cat else { return ("", Act::None) };
    match &c.get {
        Get::NotDownloaded if c.gate == LicenceGate::NeedsToken => ("Download", Act::Busy),
        Get::NotDownloaded => ("Download", Act::Download),
        Get::Queued | Get::Downloading { .. } => ("Cancel", Act::Cancel),
        Get::Failed(_) => ("Retry", Act::Download),
        Get::OnDisk | Get::NotDownloadable(_) => ("", Act::None),
    }
}

/// Whether a row's files may be deleted now: something of it is on disk, it is not
/// downloading, and it is not loaded, loading or working.
fn deletable(r: &Row, snap: &HubSnapshot) -> bool {
    let Some(c) = r.cat else { return false };
    let idle = r.served.is_none_or(|m| matches!(m.state, ModelState::Unloaded | ModelState::Error));
    let working = snap.activity.iter().any(|a| a.model == r.id);
    c.disk_bytes > 0 && !matches!(c.get, Get::Queued | Get::Downloading { .. }) && idle && !working
}

/// "6.9 GB", "230 MB", "0.2 MB".
fn gb(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e7 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.1} MB", b / 1e6)
    }
}

/// The line under a row's name: what it does, its licence when it is not plain, and
/// what keeps it from downloading.
fn sub_line(r: &Row, used: Option<&str>) -> String {
    let mut line = r.cat.map_or(r.kind.what(), |c| c.what).to_string();
    if let Some(c) = r.cat {
        match &c.get {
            Get::Failed(why) => return format!("Download failed: {}", why.lines().next().unwrap_or("")),
            Get::NotDownloadable(why) => return why.clone(),
            Get::Downloading { done, total } if r.served.is_none() => line = format!("Downloading {} of {}", gb(*done), gb(*total)),
            Get::Queued if r.served.is_none() => line = "Waits for the download before it".to_string(),
            Get::NotDownloaded if c.disk_bytes > 0 => line.push_str(&format!(" \u{b7} {} of {} here, Download resumes", gb(c.disk_bytes), gb(c.download_bytes))),
            _ => {}
        }
        if c.licence != "none" {
            line.push_str(&format!(" \u{b7} {} licence", c.licence));
        }
        if c.gate == LicenceGate::NeedsToken {
            line.push_str(" \u{b7} gated: needs HF_TOKEN");
        }
    }
    if let Some(used) = used {
        line.push_str(&format!(" \u{b7} {used}"));
    }
    line
}

/// A row's state and the size under it.
fn state_text(r: &Row, gib: impl Fn(u32) -> f64) -> (String, String) {
    if let Some(m) = r.served {
        let state = match m.state {
            ModelState::Loaded => "loaded",
            ModelState::Loading => "loading",
            ModelState::Downloading => "downloading",
            ModelState::Error => "failed",
            ModelState::Unloaded => "on disk",
        };
        let size = if m.state == ModelState::Loaded && m.gpu_mb > 0 {
            format!("{:.1} GiB", gib(m.gpu_mb))
        } else {
            r.cat.map(|c| gb(c.disk_bytes)).unwrap_or_default()
        };
        return (state.to_string(), size);
    }
    let Some(c) = r.cat else { return (String::new(), String::new()) };
    match &c.get {
        Get::NotDownloaded => ("not downloaded".into(), gb(c.download_bytes)),
        Get::Queued => ("queued".into(), gb(c.download_bytes)),
        Get::Downloading { done, total } => (format!("{:.0}%", if *total > 0 { *done as f64 * 100.0 / *total as f64 } else { 0.0 }), gb(*total)),
        Get::OnDisk => ("on disk".into(), gb(c.disk_bytes)),
        Get::Failed(_) => ("failed".into(), gb(c.download_bytes)),
        Get::NotDownloadable(_) => ("by hand".into(), String::new()),
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

/// Under the AI First option: it never gives GPU memory back to the desktop.
const AI_FIRST_WARNING: &str = "AI keeps its GPU memory: the desktop and games may slow down or fail.";

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

    fn catalogue_snap() -> HubSnapshot {
        use crate::hub_control::{Catalogue, CatalogueModel, Get};
        let m = |id: &str, kind, gate, get, disk| CatalogueModel { id: id.into(), kind, what: "x", download_bytes: 3_100_000_000, disk_bytes: disk, licence: "none".into(), gate, get };
        HubSnapshot {
            available: true,
            // Served: the chat model on disk and loaded.
            models: vec![ModelInfo { id: "chat-on".into(), name: "chat-on".into(), kind: ModelKind::Chat, state: ModelState::Loaded, context_fit: None, gpu_mb: 4000, device: String::new() }],
            catalogue: Some(Catalogue {
                models: vec![
                    m("song-nc", ModelKind::Music, LicenceGate::Confirm("non-commercial".into()), Get::NotDownloaded, 0),
                    m("chat-on", ModelKind::Chat, LicenceGate::Open, Get::OnDisk, 3_000_000_000),
                    m("chat-new", ModelKind::Chat, LicenceGate::Open, Get::NotDownloaded, 0),
                    m("img-gated", ModelKind::Image, LicenceGate::NeedsToken, Get::NotDownloaded, 0),
                    m("stt-dl", ModelKind::SpeechToText, LicenceGate::Open, Get::Downloading { done: 1_550_000_000, total: 3_100_000_000 }, 1_550_000_000),
                    m("tts-disk", ModelKind::TextToSpeech, LicenceGate::Open, Get::OnDisk, 350_000_000),
                ],
                elsewhere: vec![("flux2-dev".into(), "requires 30 GB".into())],
                note: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_catalogue_merges_with_what_is_served_grouped_by_kind() {
        let snap = catalogue_snap();
        let rows = model_rows(&snap);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), ["chat-new", "chat-on", "stt-dl", "tts-disk", "img-gated", "song-nc"]);
        assert!(rows[1].served.is_some() && rows[1].cat.is_some(), "a served model keeps its catalogue entry");
        assert_eq!(primary(&rows[0]), ("Download", Act::Download));
        assert_eq!(primary(&rows[1]), ("Unload", Act::Unload));
        assert_eq!(primary(&rows[2]), ("Cancel", Act::Cancel));
        assert_eq!(primary(&rows[3]).1, Act::None, "on disk, not served: Delete only");
        assert_eq!(primary(&rows[4]), ("Download", Act::Busy), "gated without a token: shown, disabled");
        assert!(!deletable(&rows[1], &snap), "loaded: never deleted");
        assert!(!deletable(&rows[2], &snap), "downloading: cancel first");
        assert!(deletable(&rows[3], &snap));
        assert!(!deletable(&rows[0], &snap), "nothing on disk");
        assert_eq!(state_text(&rows[2], |mb| mb as f64), ("50%".to_string(), "3.1 GB".to_string()));
        assert_eq!(state_text(&rows[0], |mb| mb as f64).0, "not downloaded");
        assert!(sub_line(&rows[4], None).contains("HF_TOKEN"));
        assert_eq!(gb(230_444), "0.2 MB");
        // Without a catalogue (the WM) the rows are exactly what the hub serves.
        let wm = HubSnapshot { catalogue: None, ..snap };
        assert_eq!(model_rows(&wm).len(), 1);
    }

    #[test]
    fn licences_and_deletes_ask_first() {
        let snap = catalogue_snap();
        let mut p = HubPanel::default();
        let at = dvec2(5.0, 5.0);
        let only = |h| vec![(h, rect(0.0, 0.0, 10.0, 10.0))];
        p.hits = only(HubHit::Model(0));
        assert_eq!(p.click(at, &snap), Some(HubCommand::Download { id: "chat-new".into(), confirmed: false }), "none: on click");
        p.hits = only(HubHit::Model(5));
        assert_eq!(p.click(at, &snap), None, "non-commercial: a question first");
        assert_eq!(p.confirm, Some(Confirm::Licence { id: "song-nc".into(), class: "non-commercial".into() }));
        p.hits = only(HubHit::ConfirmYes);
        assert_eq!(p.click(at, &snap), Some(HubCommand::Download { id: "song-nc".into(), confirmed: true }));
        assert_eq!(p.confirm, None);
        p.hits = only(HubHit::Model(4));
        assert_eq!(p.click(at, &snap), None, "gated without a token: nothing");
        p.hits = only(HubHit::Delete(3));
        assert_eq!(p.click(at, &snap), None);
        assert_eq!(p.confirm, Some(Confirm::Delete { id: "tts-disk".into(), bytes: 350_000_000 }));
        p.hits = only(HubHit::ConfirmNo);
        assert_eq!(p.click(at, &snap), None);
        assert_eq!(p.confirm, None, "no: nothing deleted");
        p.hits = only(HubHit::Delete(3));
        p.click(at, &snap);
        p.hits = only(HubHit::ConfirmYes);
        assert_eq!(p.click(at, &snap), Some(HubCommand::Delete("tts-disk".into())));
        p.hits = only(HubHit::Delete(1));
        assert_eq!(p.click(at, &snap), None);
        assert_eq!(p.confirm, None, "a loaded model asks nothing: it is not deletable");
        p.hits = only(HubHit::Model(2));
        assert_eq!(p.click(at, &snap), Some(HubCommand::CancelDownload("stt-dl".into())));
        p.hits = only(HubHit::Elsewhere);
        let closed = p.models_height(&snap);
        p.click(at, &snap);
        assert_eq!(p.models_height(&snap), closed + 16.0, "the list of models not for this machine opens");
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
        assert!(p.hover(at) && !p.hover(at), "hover reports a change once");
        assert_eq!(p.click(dvec2(50.0, 50.0), &snap), None, "outside every target");
    }
}
