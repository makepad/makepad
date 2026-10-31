//! The activity HUD above the composer: what the agent is doing right now and
//! for how long, a progress bar when a command reports one, the tokens this
//! turn has produced, the files it touched, and one compact row per running
//! sub-agent. Between turns it holds a one-line summary of the last turn.
//!
//! Everything here is derived from the transcript, read-only; the only state
//! of its own is when each thing began (for the timers) and the short scan
//! that marks a change of activity. It asks for frames only while that scan
//! runs or a counter catches up (`animating`); busy turns already tick.

use super::look::{self, alpha, ink};
use super::model::{Entry, ToolEntry, ToolState, Transcript};
use makepad_code_agent::event::{LaneId, Touch};
use makepad_widgets::*;

const ROW: f64 = 22.0;
const BADGE: f64 = 62.0;
const BAR: f64 = 112.0;
/// The scan that marks a change of activity, in seconds.
const SCAN: f64 = 0.32;
const MAX_LANE_ROWS: usize = 3;
/// How long the HUD says what was copied, in seconds.
const FLASH: f64 = 1.2;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    set_type_default() do #(DrawHudFx::script_shader(vm)){
        ..mod.draw.DrawQuad
        // kind 0: the state badge, 1: a progress bar (val = done share),
        // 2: the scan across a row (t = 0..1 through it).
        pixel: fn() {
            let p = self.pos * self.rect_size
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let c = self.color
            if self.kind < 0.5 {
                // A wash of the state ink, a two-pixel bar on its left, and
                // while the scan runs a brief lift that fades from the left.
                var a = 0.13 + 0.22 * self.t * (1.0 - self.pos.x)
                if p.x < 2.0 {
                    a = 1.0
                }
                return vec4(c.rgb * a * c.a, a * c.a)
            }
            if self.kind < 1.5 {
                // Segmented bar: cells three points wide with a one-point
                // gap, the filled run in the ink with a hot leading cell.
                let cell = fract(p.x / 4.0)
                let gap = step(0.75, cell)
                let filled = step(p.x, self.val * self.rect_size.x)
                let head = filled * (1.0 - smoothstep(0.0, 10.0, self.val * self.rect_size.x - p.x))
                let track = vec4(0.14, 0.18, 0.23, 1.0) * 0.6
                let ink = vec4(mix(c.rgb, vec3(1.0, 1.0, 1.0), 0.45 * head), 1.0)
                let col = mix(track, ink, filled)
                let a = (1.0 - gap) * col.a
                return vec4(col.rgb * a, a)
            }
            // The scan: a soft vertical band of the ink sweeping right, with
            // a one-pixel bright line at its front and fine scanlines in it.
            let x = self.pos.x
            let front = self.t * 1.25
            let d = front - x
            let band = smoothstep(0.0, 0.02, d) * (1.0 - smoothstep(0.02, 0.32, d))
            let line = 1.0 - smoothstep(0.0, 1.5 * px / self.rect_size.x, abs(d))
            let lines = Finish.scan(p.y, px, 2.0, 0.5)
            let a = (0.10 * band * lines + 0.55 * line) * (1.0 - self.t) * c.a
            return vec4(c.rgb * a, a)
        }
    }

    mod.widgets.CodeHudBase = #(CodeHud::register_widget(vm))
    mod.widgets.CodeHud = set_type_default() do mod.widgets.CodeHudBase{
        width: Fill
        height: 22
        // The HUD's whole box (its hit area), drawn as one hairline on top.
        draw_bg +: {
            color: mod.look.hair
            pixel: fn() {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let line = 1.0 - step(px, self.pos.y * self.rect_size.y)
                return vec4(self.color.rgb * line, line)
            }
        }
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_bold +: {
            color: mod.look.text
            text_style: mod.look.mono_bold{font_size: 8.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawHudFx {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    kind: f32,
    #[live]
    t: f32,
    #[live]
    val: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Ready,
    Done,
    Think,
    Stream,
    Run,
    Edit,
    Scan,
    Wait,
    Ask,
    Failed,
    Copied,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Ready => "READY",
            Kind::Done => "DONE",
            Kind::Think => "THINK",
            Kind::Stream => "WRITE",
            Kind::Run => "RUN",
            Kind::Edit => "EDIT",
            Kind::Scan => "READ",
            Kind::Wait => "YOU",
            Kind::Ask => "ASK",
            Kind::Failed => "ERROR",
            Kind::Copied => "COPIED",
        }
    }

    fn ink(self) -> Vec4f {
        match self {
            Kind::Ready => ink::META,
            Kind::Done => ink::OK,
            Kind::Think => ink::LANE,
            Kind::Stream | Kind::Run | Kind::Scan => ink::AGENT,
            Kind::Edit => ink::CHANGE,
            Kind::Wait | Kind::Ask => ink::HUMAN,
            Kind::Failed => ink::BAD,
            Kind::Copied => ink::OK,
        }
    }

    fn live(self) -> bool {
        matches!(self, Kind::Think | Kind::Stream | Kind::Run | Kind::Edit | Kind::Scan)
    }
}

/// What one row says.
struct Line {
    kind: Kind,
    /// Changes when the activity changes (restarts its timer and the scan).
    key: u64,
    text: String,
    progress: Option<(u32, u32)>,
}

/// The last finished turn, for the summary between turns.
#[derive(Clone, Default)]
struct TurnSummary {
    secs: f64,
    tools: usize,
    changed: usize,
    tokens: u64,
    failed: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct CodeHud {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_fx: DrawHudFx,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[rust]
    clock: f64,
    #[rust]
    busy: bool,
    #[rust]
    turn_start: f64,
    #[rust]
    turn_tools_from: usize,
    #[rust]
    gen_at_start: u64,
    #[rust]
    last_total: u64,
    #[rust]
    touched_at_start: Vec<(String, Touch)>,
    #[rust]
    last: Option<TurnSummary>,
    #[rust]
    key: u64,
    #[rust]
    step_start: f64,
    #[rust]
    scan_start: f64,
    /// Tokens shown, gliding up to the real count.
    #[rust]
    shown_tokens: f64,
    #[rust]
    lines: Vec<(Line, f64, Option<String>)>,
    /// When each lane's current activity began: lane, key, start.
    #[rust]
    lane_steps: Vec<(LaneId, u64, f64)>,
    #[rust]
    turn_files: Vec<(String, Touch)>,
    #[rust]
    rate: f32,
    #[rust]
    route: String,
    #[rust]
    char_w: f64,
    /// Drawn texts and where, for copying.
    #[rust]
    hits: Vec<(Rect, String)>,
    #[rust]
    rect: Rect,
    /// A copy just happened: what, and when.
    #[rust]
    flash: Option<(String, f64)>,
}

impl CodeHud {
    /// Frames are wanted while the scan runs or the counter catches up.
    pub fn animating(&self) -> bool {
        self.flash.as_ref().is_some_and(|(_, at)| self.clock - at < FLASH)
            || self.clock - self.scan_start < SCAN || (self.busy && (self.target_tokens() - self.shown_tokens).abs() > 0.5)
    }

    fn target_tokens(&self) -> f64 {
        self.last_total.saturating_sub(self.gen_at_start) as f64
    }

    /// Fold the transcript into what the HUD shows. Call while drawing,
    /// before the HUD is drawn, with the view's clock in seconds.
    pub fn set(&mut self, t: &Transcript, clock: f64) {
        let dt = (clock - self.clock).clamp(0.0, 0.25);
        self.clock = clock;
        let tools_now = t.entries.iter().filter(|e| matches!(e, Entry::Tool(_))).count();
        if t.busy && !self.busy {
            self.turn_start = clock;
            self.turn_tools_from = tools_now;
            self.gen_at_start = t.usage.total_generated;
            self.touched_at_start = t.touched.clone();
            self.shown_tokens = 0.0;
            self.last = None;
        }
        self.last_total = t.usage.total_generated.max(self.gen_at_start);
        // This turn's files: new ones, and ones whose touch grew.
        self.turn_files.clear();
        if t.busy || self.busy || self.last.is_some() {
            for (path, how) in t.touched.iter().rev() {
                let before = self.touched_at_start.iter().find(|(p, _)| p == path).map(|(_, h)| *h);
                if before != Some(*how) {
                    self.turn_files.push((path.clone(), *how));
                }
            }
        }
        if !t.busy && self.busy {
            let tools = t.entries.iter().filter_map(|e| if let Entry::Tool(tool) = e { Some(tool) } else { None }).skip(self.turn_tools_from);
            let mut count = 0;
            let mut failed = false;
            for tool in tools {
                count += 1;
                failed |= tool.state == ToolState::Failed;
            }
            failed |= matches!(t.entries.last(), Some(Entry::Error(_)));
            self.last = Some(TurnSummary {
                secs: clock - self.turn_start,
                tools: count,
                changed: self.turn_files.iter().filter(|(_, h)| matches!(h, Touch::Edited | Touch::Created)).count(),
                tokens: self.last_total - self.gen_at_start,
                failed,
            });
            self.shown_tokens = self.target_tokens();
        }
        self.busy = t.busy;
        self.rate = if t.busy { t.usage.tokens_per_sec } else { 0.0 };
        self.route = t.status.as_ref().map(|s| s.route.clone()).unwrap_or_default();

        // The counter glides: a quarter of the gap per frame, at least a token.
        let target = self.target_tokens();
        if self.shown_tokens < target {
            let step = ((target - self.shown_tokens) * (1.0 - (-dt * 14.0).exp())).max(1.0);
            self.shown_tokens = (self.shown_tokens + step).min(target);
        } else {
            self.shown_tokens = target;
        }

        // The main row (or, for a moment, what was just copied).
        let mut main = self.main_line(t);
        if let Some((text, at)) = &self.flash {
            if clock - at < FLASH {
                main = Line { kind: Kind::Copied, key: 0x9000_0000, text: text.clone(), progress: None };
            } else {
                self.flash = None;
            }
        }
        if main.key != self.key {
            self.key = main.key;
            self.step_start = clock;
            if self.busy || main.kind == Kind::Done {
                self.scan_start = clock;
            }
        }
        let step_start = self.step_start;
        self.lines.clear();
        self.lines.push((main, step_start, None));

        // One row per running sub-agent.
        let mut steps = std::mem::take(&mut self.lane_steps);
        for lane in t.running_lanes().take(MAX_LANE_ROWS) {
            let mut line = lane_line(t, lane.id);
            line.text = format!("{}  ›  {}", clip(&lane.title, 40), line.text);
            let start = match steps.iter_mut().find(|(id, _, _)| *id == lane.id) {
                Some((_, key, start)) => {
                    if *key != line.key {
                        *key = line.key;
                        *start = clock;
                    }
                    *start
                }
                None => {
                    steps.push((lane.id, line.key, clock));
                    clock
                }
            };
            self.lines.push((line, start, Some(format!("L{}", lane.id))));
        }
        steps.retain(|(id, _, _)| t.running_lanes().any(|l| l.id == *id));
        self.lane_steps = steps;
    }

    fn main_line(&self, t: &Transcript) -> Line {
        if let Some(tool) = pending_tool(t) {
            let prompt = tool.approval.as_deref().unwrap_or("").lines().next().unwrap_or("").to_string();
            let text = if prompt.is_empty() { format!("approve  {}", tool.title) } else { format!("approve  {prompt}") };
            return Line { kind: Kind::Wait, key: 0x1000_0000 ^ tool.id, text, progress: None };
        }
        if t.question.is_some() {
            return Line { kind: Kind::Ask, key: 0x2000_0000, text: "the agent asked you something: answer in the box below".into(), progress: None };
        }
        if let Some(load) = t.status.as_ref().and_then(|s| s.load.as_ref()) {
            // The model is loading or being swapped in: its stage, a bar and the time left.
            return Line {
                kind: Kind::Think,
                key: 0x9000_0000,
                text: load.detail(),
                progress: load.fraction.map(|f| ((f.clamp(0.0, 1.0) * 1000.0).round() as u32, 0)),
            };
        }
        if !t.busy {
            return match &self.last {
                Some(s) => Line {
                    kind: if s.failed { Kind::Failed } else { Kind::Done },
                    key: 0x3000_0000 + s.tools as u64,
                    text: summary_text(s),
                    progress: None,
                },
                None => Line {
                    kind: Kind::Ready,
                    key: 0x4000_0000,
                    text: if self.route.is_empty() { "starting the model".into() } else { format!("{}  ·  ⌘K commands  @ file  ⌘T new chat", self.route) },
                    progress: None,
                },
            };
        }
        lane_line(t, 0)
    }

    fn mono_w(&mut self, cx: &mut Cx2d) -> f64 {
        if self.char_w <= 0.0 {
            let laid = self.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), "0000000000");
            self.char_w = laid.size_in_lpxs.width as f64 / 10.0;
        }
        self.char_w
    }

    /// Draw `text` with its left at `x`, vertically centred in the row at
    /// `y`; returns the x after it.
    fn put(&mut self, cx: &mut Cx2d, bold: bool, x: f64, y: f64, text: &str, color: Vec4f) -> f64 {
        if text.is_empty() {
            return x;
        }
        let dt = if bold { &mut self.draw_bold } else { &mut self.draw_text };
        dt.color = color;
        let laid = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let h = laid.size_in_lpxs.height as f64;
        let w = laid.size_in_lpxs.width as f64;
        dt.draw_abs(cx, dvec2(x.round(), (y + (ROW - h) * 0.5).round()), text);
        // Words are copyable (right-click); marks and spinners are not.
        if text.chars().any(|c| c.is_alphanumeric()) {
            self.hits.push((Rect { pos: dvec2(x, y), size: dvec2(w, ROW) }, text.to_string()));
        }
        x + w
    }

    /// Where the HUD finally sits minus where it was laid out: the layout
    /// may move it after drawing (rows aligned to the bottom).
    fn shift(&self, cx: &Cx) -> DVec2 {
        self.draw_bg.area().rect(cx).pos - self.rect.pos
    }

    /// The HUD is under `abs`.
    pub fn contains(&self, cx: &Cx, abs: DVec2) -> bool {
        self.draw_bg.area().clipped_rect(cx).contains(abs)
    }

    /// The text of the HUD item under `abs` (a value, a file, the activity).
    pub fn text_at(&self, cx: &Cx, abs: DVec2) -> Option<String> {
        let at = abs - self.shift(cx);
        self.hits.iter().find(|(r, _)| r.contains(at)).map(|(_, t)| t.clone())
    }

    /// Everything the HUD shows, one row per line.
    pub fn full_text(&self) -> String {
        let mut rows: Vec<(f64, Vec<String>)> = Vec::new();
        for (r, t) in &self.hits {
            match rows.iter_mut().find(|(y, _)| (*y - r.pos.y).abs() < 1.0) {
                Some((_, v)) => v.push(t.clone()),
                None => rows.push((r.pos.y, vec![t.clone()])),
            }
        }
        rows.into_iter().map(|(_, v)| v.join("  ")).collect::<Vec<_>>().join("\n")
    }

    /// Say on the HUD that `text` went to the clipboard, for a moment.
    pub fn flash_copied(&mut self, text: &str, clock: f64) {
        let one = text.lines().next().unwrap_or("");
        self.flash = Some((format!("{} chars  ·  {}", text.chars().count(), one), clock));
    }

    fn draw_row(&mut self, cx: &mut Cx2d, rect: Rect, i: usize) {
        let (line, start, lane) = {
            let (l, s, lane) = &self.lines[i];
            (Line { kind: l.kind, key: l.key, text: l.text.clone(), progress: l.progress }, *s, lane.clone())
        };
        let y = rect.pos.y;
        let x0 = rect.pos.x;
        let right = rect.pos.x + rect.size.x;
        let cw = self.mono_w(cx);
        let ink_k = line.kind.ink();
        let scan_t = ((self.clock - self.scan_start) / SCAN).clamp(0.0, 1.0) as f32;
        let scanning = i == 0 && scan_t < 1.0;

        // Badge (main row) or lane tag (sub-agent rows).
        let mut x = x0;
        if let Some(tag) = lane {
            self.put(cx, false, x + 12.0, y, "└", ink::DIM);
            let w = BADGE - 22.0;
            self.draw_fx.color = alpha(ink::LANE, 0.9);
            self.draw_fx.kind = 0.0;
            self.draw_fx.t = 0.0;
            self.draw_fx.draw_abs(cx, Rect { pos: dvec2(x + 22.0, y + 3.0), size: dvec2(w, ROW - 6.0) });
            self.put(cx, true, x + 28.0, y, &tag, ink::LANE);
        } else {
            self.draw_fx.color = ink_k;
            self.draw_fx.kind = 0.0;
            self.draw_fx.t = if scanning { 1.0 - scan_t } else { 0.0 };
            self.draw_fx.draw_abs(cx, Rect { pos: dvec2(x, y + 2.0), size: dvec2(BADGE, ROW - 4.0) });
            let glyph = if line.kind.live() {
                look::spinner(self.clock)
            } else {
                match line.kind {
                    Kind::Done | Kind::Copied => "✓",
                    Kind::Failed => "✗",
                    Kind::Wait | Kind::Ask => "◆",
                    _ => "◇",
                }
            };
            let gx = self.put(cx, false, x + 7.0, y, glyph, ink_k);
            self.put(cx, true, gx + cw * 0.6, y, line.kind.label(), ink_k);
        }
        x += BADGE + 10.0;

        // Right cluster, laid out from the right edge first so the activity
        // text knows how much room it has.
        let mut rx = right;
        let timer = if matches!(line.kind, Kind::Ready | Kind::Done | Kind::Failed | Kind::Copied) { String::new() } else { look::elapsed(self.clock - start) };
        let mut right_items: Vec<(String, Vec4f, bool)> = Vec::new();
        if i == 0 {
            if self.busy {
                let tokens = self.shown_tokens as u64;
                if self.rate > 0.5 {
                    right_items.push((format!("{:.0} t/s", self.rate), ink::META, false));
                }
                if tokens > 0 {
                    right_items.push((format!("↑{}", short(tokens)), ink::AGENT, true));
                }
                right_items.push((format!("T+{}", look::elapsed(self.clock - self.turn_start)), ink::META, false));
            }
            // Files this turn, newest first, at most three.
            let files: Vec<(String, Touch)> = self.turn_files.iter().take(3).cloned().collect();
            for (path, how) in files.iter().rev() {
                let name = path.rsplit('/').next().unwrap_or(path);
                let mark = match how {
                    Touch::Edited => "~",
                    Touch::Created => "+",
                    _ => "·",
                };
                right_items.push((format!("{mark}{name}"), look::touch(*how), false));
            }
            if self.turn_files.len() > 3 {
                right_items.push((format!("+{}", self.turn_files.len() - 3), ink::META, false));
            }
        }
        // Measure from the right: items are drawn left to right in reverse.
        let mut placed: Vec<(f64, String, Vec4f, bool)> = Vec::new();
        for (text, color, bold) in right_items.iter().rev() {
            let w = text.chars().count() as f64 * cw;
            rx -= w;
            placed.push((rx, text.clone(), *color, *bold));
            rx -= cw * 2.0;
        }
        let right_limit = rx - cw;

        // Progress bar before the right cluster.
        let mut text_limit = right_limit;
        if let Some((done, total)) = line.progress {
            // total 0: `done` is a share in thousandths (a model load), shown as a percentage.
            let (done, total, count) = if total == 0 { (done, 1000, format!("{}%", done / 10)) } else { (done, total, format!("{done}/{total}")) };
            let cw_count = count.chars().count() as f64 * cw;
            let bx = right_limit - cw_count - cw - BAR;
            if bx > x + 160.0 {
                self.draw_fx.color = ink_k;
                self.draw_fx.kind = 1.0;
                self.draw_fx.val = (done as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                self.draw_fx.draw_abs(cx, Rect { pos: dvec2(bx.round(), (y + ROW * 0.5 - 2.0).round()), size: dvec2(BAR, 4.0) });
                self.put(cx, false, bx + BAR + cw, y, &count, ink::TEXT);
                text_limit = bx - cw;
            }
        }

        // Activity text and its timer.
        let timer_w = if timer.is_empty() { 0.0 } else { (timer.chars().count() as f64 + 2.0) * cw };
        let room = ((text_limit - x - timer_w) / cw).floor().max(4.0) as usize;
        let text = clip(&line.text, room);
        let text_ink = match line.kind {
            Kind::Ready => ink::META,
            Kind::Done => ink::TEXT,
            _ => ink::TEXT_HI,
        };
        let after = self.put(cx, false, x, y, &text, text_ink);
        if !timer.is_empty() {
            self.put(cx, false, after + cw * 2.0, y, &timer, ink::META);
        }
        for (px_, text, color, bold) in placed {
            self.put(cx, bold, px_, y, &text, color);
        }

        // The scan over the main row when its activity changes.
        if scanning {
            self.draw_fx.color = ink_k;
            self.draw_fx.kind = 2.0;
            self.draw_fx.t = scan_t;
            self.draw_fx.draw_abs(cx, Rect { pos: dvec2(x0, y), size: dvec2(rect.size.x, ROW) });
        }
    }
}

impl Widget for CodeHud {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rows = self.lines.len().max(1);
        let mut walk = walk;
        walk.height = Size::Fixed(ROW * rows as f64 + 1.0);
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        self.hits.clear();
        // One hairline above (the shader draws only the top pixel row of the
        // box); the rows sit on the window's ground.
        self.draw_bg.draw_abs(cx, rect);
        for i in 0..self.lines.len() {
            let r = Rect { pos: dvec2(rect.pos.x, rect.pos.y + 1.0 + ROW * i as f64), size: dvec2(rect.size.x, ROW) };
            self.draw_row(cx, r, i);
        }
        DrawStep::done()
    }
}

/// The tool waiting for you, if any.
fn pending_tool(t: &Transcript) -> Option<&ToolEntry> {
    t.entries.iter().rev().find_map(|e| match e {
        Entry::Tool(tool) if tool.approval.is_some() => Some(tool),
        _ => None,
    })
}

/// What `lane` is doing: its running tool, else what it last produced.
fn lane_line(t: &Transcript, lane: LaneId) -> Line {
    for (i, e) in t.entries.iter().enumerate().rev() {
        match e {
            Entry::Tool(tool) if tool.lane == lane && tool.state == ToolState::Running => {
                let kind = if tool.is_edit() {
                    Kind::Edit
                } else if matches!(tool.name.as_str(), "read_file" | "grep" | "glob" | "list_dir" | "git_diff") {
                    Kind::Scan
                } else {
                    Kind::Run
                };
                let progress = progress_of(&tool.output);
                let built = if progress.is_none() { compiled(&tool.output) } else { 0 };
                let text = if built > 0 { format!("{}  ·  {built} built", tool.title) } else { tool.title.clone() };
                return Line { kind, key: 0x5000_0000 ^ tool.id, text, progress };
            }
            Entry::Assistant { lane: l, streaming: true, text } if *l == lane => {
                let words = text.split_whitespace().count();
                return Line { kind: Kind::Stream, key: 0x6000_0000 ^ i as u64, text: format!("answer  ·  {words} words"), progress: None };
            }
            Entry::Thinking { lane: l, text } if *l == lane => {
                let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                return Line { kind: Kind::Think, key: 0x7000_0000 ^ i as u64, text: last.to_string(), progress: None };
            }
            Entry::Tool(tool) if tool.lane == lane => break,
            Entry::User { .. } if lane == 0 => break,
            _ => {}
        }
    }
    Line { kind: Kind::Think, key: 0x8000_0000 ^ t.entries.len() as u64, text: "waiting for the model".into(), progress: None }
}

/// `n/m` progress a command prints (cargo, ninja, test runners) in its last
/// two lines, or `None`.
fn progress_of(output: &str) -> Option<(u32, u32)> {
    for line in output.trim_end().lines().rev().take(2) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'/' && i > 0 && bytes[i - 1].is_ascii_digit() && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                let mut a = i;
                while a > 0 && bytes[a - 1].is_ascii_digit() {
                    a -= 1;
                }
                let mut b = i + 1;
                while b < bytes.len() && bytes[b].is_ascii_digit() {
                    b += 1;
                }
                let done: u32 = line[a..i].parse().ok()?;
                let total: u32 = line[i + 1..b].parse().ok()?;
                if total > 1 && done <= total {
                    return Some((done, total));
                }
                i = b;
            } else {
                i += 1;
            }
        }
    }
    None
}

/// Crates (or units) a build reported as compiled so far: cargo prints no
/// total when its output is not a terminal, so the count stands alone.
fn compiled(output: &str) -> usize {
    output.lines().filter(|l| matches!(l.trim_start().split(' ').next(), Some("Compiling" | "Checking"))).count()
}

fn summary_text(s: &TurnSummary) -> String {
    let mut out = format!("turn took {}", look::elapsed(s.secs));
    if s.tools > 0 {
        out.push_str(&format!("  ·  {} tool{}", s.tools, if s.tools == 1 { "" } else { "s" }));
    }
    if s.changed > 0 {
        out.push_str(&format!("  ·  {} file{} changed", s.changed, if s.changed == 1 { "" } else { "s" }));
    }
    if s.tokens > 0 {
        out.push_str(&format!("  ·  ↑{} tokens", short(s.tokens)));
    }
    out
}

fn short(n: u64) -> String {
    if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn clip(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() <= max && !text.contains('\n') {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reads_counts_from_the_tail() {
        assert_eq!(progress_of("   Compiling a\n    Building [=====>    ] 34/120: foo"), Some((34, 120)));
        assert_eq!(progress_of("[ 7/9] Linking"), Some((7, 9)));
        assert_eq!(progress_of("src/a.rs:3: x"), None);
        assert_eq!(progress_of("ratio 9/3"), None);
        assert_eq!(progress_of(""), None);
    }

    #[test]
    fn compiled_counts_cargo_units() {
        assert_eq!(compiled("   Compiling a v1\n    Checking b v1\nwarning: x\n    Finished"), 2);
    }

    #[test]
    fn clip_keeps_one_line() {
        assert_eq!(clip("abc\ndef", 10), "abc…");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
    }
}
