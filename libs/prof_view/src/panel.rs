//! The profile view: the running makepad apps (makepad run and native builds with their remote
//! endpoint on) and the saved recordings on the left; for the attached app (or an opened
//! recording) the profiler's timeline over the answer of the selected query. The query bar asks
//! the profiler (summary, frames, hitches, top cpu / jit / gpu, lanes, why-slow, diff) and the
//! console sections ask the app itself over the same link (panics, threads, memory, code
//! versions; any `makepad inspect` command typed in the bar).

use crate::answer::{ProfAnswer, ProfAnswerAction};
use crate::session::{local_apps, AppEntry, Session, Status};
use crate::timeline::{ProfTimeline, ProfTimelineAction};
use makepad_prof_stream::ask::{self, Ans};
use makepad_prof_stream::Profile;
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let SideTitle = Label{
        padding: Inset{left: 10 top: 10 bottom: 4}
        draw_text +: {color: mix(theme.color_bg_app, theme.color_text, 0.55) text_style: theme.font_bold{font_size: 7.5}}
    }
    let Bar = ButtonFlatter{
        height: 24 margin: 0 padding: Inset{left: 8 right: 8 top: 3 bottom: 3}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5} color: theme.color_text}
    }
    let BarStatus = Label{
        padding: Inset{left: 6 right: 6}
        draw_text +: {color: mix(theme.color_bg_app, theme.color_text, 0.6) text_style: theme.font_regular{font_size: 8.5}}
    }

    mod.widgets.ProfListBase = #(ProfList::register_widget(vm))
    mod.widgets.ProfList = set_type_default() do mod.widgets.ProfListBase{
        width: Fill height: Fit
        max_rows: 8.0
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.06)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.55)
        accent: theme.color_focus
        draw_title +: {text_style: theme.font_regular{font_size: 9} color: theme.color_text}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5} color: theme.color_text}
    }

    mod.widgets.ProfViewBase = #(ProfView::register_widget(vm))
    mod.widgets.ProfView = set_type_default() do mod.widgets.ProfViewBase{
        width: Fill height: Fill flow: Right spacing: 0
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}
        live_side := View{
            width: 250 height: Fill flow: Down spacing: 0
            show_bg: true
            draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.025)}
            View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
                SideTitle{text: "RUNNING APPS"}
                View{width: Fill height: 1}
                live_refresh := Bar{text: "Refresh"}
            }
            live_apps := mod.widgets.ProfList{empty_text: "No makepad app is running (makepad run, or a native build with MAKEPAD_REMOTE=1)"}
            SideTitle{text: "RECORDINGS"}
            live_recordings := mod.widgets.ProfList{max_rows: 14.0 empty_text: "Record a range of a live app to keep it here"}
            View{width: Fill height: Fit flow: Right{wrap: true} padding: Inset{left: 6 top: 4}
                live_baseline := Bar{text: "Set as baseline"}
                live_clear_baseline := Bar{text: "Clear baseline"}
            }
            live_baseline_label := BarStatus{width: Fill text: "" draw_text +: {wrap: Words}}
        }
        live_main := View{
            width: Fill height: Fill flow: Down spacing: 0
            View{width: Fill height: 32 flow: Right spacing: 2 align: Align{y: 0.5} padding: Inset{left: 6 right: 6}
                live_title := Label{padding: Inset{left: 4 right: 8} text: "Live" draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 10}}}
                live_status := BarStatus{text: ""}
                View{width: Fill height: 1}
                live_analyze := Bar{text: "Analyze"}
                live_record := Bar{text: "Record"}
                live_follow := Bar{text: "Follow"}
                live_detach := Bar{text: "Detach"}
            }
            View{width: Fill height: 28 flow: Right spacing: 0 align: Align{y: 0.5} padding: Inset{left: 6 right: 6}
                q_summary := Bar{text: "Summary"}
                q_frames := Bar{text: "Frames"}
                q_hitches := Bar{text: "Hitches"}
                q_cpu := Bar{text: "Top CPU"}
                q_fn := Bar{text: "Hot fns"}
                q_jit := Bar{text: "JIT"}
                q_gpu := Bar{text: "GPU"}
                q_lanes := Bar{text: "Lanes"}
                q_why := Bar{text: "Why slow"}
                q_diff := Bar{text: "Diff"}
                View{width: 12 height: 1}
                c_panics := Bar{text: "Panics"}
                c_threads := Bar{text: "Threads"}
                c_memory := Bar{text: "Memory"}
                c_hung := Bar{text: "Why hung"}
                c_asm := Bar{text: "Asm vs rustc"}
                View{width: Fill height: 1}
                live_query := TextInput{width: 260 height: 24 empty_text: "query or inspect command (Enter)"}
            }
            live_timeline := ProfTimeline{height: Fill}
            View{width: Fill height: 1 show_bg: true draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.12)}}
            live_answer := ProfAnswer{height: 320}
        }
    }
}

// ------------------------------------------------------------------ a simple list

#[derive(Clone, Debug, Default)]
pub enum ProfListAction {
    Select(usize),
    #[default]
    None,
}

const ROW: f64 = 30.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListRow {
    pub title: String,
    pub sub: String,
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct ProfList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    max_rows: f64,
    #[live]
    empty_text: String,
    #[rust]
    area: Area,
    #[rust]
    viewport: Rect,
    #[rust]
    rows: Vec<ListRow>,
    #[rust]
    selected: Option<usize>,
    #[rust]
    hovered: Option<usize>,
    #[rust]
    scroll: f64,
}

impl ProfList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<ListRow>, selected: Option<usize>) {
        if rows != self.rows || selected != self.selected {
            self.rows = rows;
            self.selected = selected;
            self.area.redraw(cx);
        }
    }
    fn row_at(&self, abs: DVec2) -> Option<usize> {
        if !self.viewport.contains(abs) {
            return None;
        }
        let i = ((abs.y - self.viewport.pos.y + self.scroll) / ROW).floor();
        (i >= 0.0 && (i as usize) < self.rows.len()).then_some(i as usize)
    }
    fn fit(draw: &mut DrawText, cx: &mut Cx2d, pos: DVec2, max_w: f64, s: &str) {
        let n = ((max_w / 5.0) as usize).max(4);
        let shown: String = if s.chars().count() > n { s.chars().take(n - 1).chain(std::iter::once('…')).collect() } else { s.to_string() };
        draw.draw_abs(cx, pos, &shown);
    }
}

impl WidgetNode for ProfList {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        let mut w = self.walk;
        let n = if self.rows.is_empty() { 1.6 } else { (self.rows.len() as f64).min(self.max_rows.max(1.0)) };
        w.height = Size::Fixed(n * ROW + 4.0);
        w
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

impl Widget for ProfList {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut walk = walk;
        let n = if self.rows.is_empty() { 1.6 } else { (self.rows.len() as f64).min(self.max_rows.max(1.0)) };
        walk.height = Size::Fixed(n * ROW + 4.0);
        let view = cx.walk_turtle(walk);
        self.viewport = view;
        cx.add_rect_area(&mut self.area, view);
        cx.push_clip_rect(view);
        if self.rows.is_empty() {
            self.draw_small.color = self.muted;
            let t = self.empty_text.clone();
            let lines = crate::answer_wrap(&t, view.size.x - 20.0, 4.3);
            for (k, l) in lines.iter().enumerate().take(3) {
                self.draw_small.draw_abs(cx, dvec2(view.pos.x + 10.0, view.pos.y + 4.0 + k as f64 * 11.0), l);
            }
        }
        for i in 0..self.rows.len() {
            let y = view.pos.y + i as f64 * ROW - self.scroll;
            if y + ROW < view.pos.y || y > view.pos.y + view.size.y {
                continue;
            }
            let r = Rect { pos: dvec2(view.pos.x + 4.0, y + 1.0), size: dvec2(view.size.x - 8.0, ROW - 2.0) };
            if self.selected == Some(i) || self.hovered == Some(i) {
                self.draw_shape.color = if self.selected == Some(i) { vec4(self.accent.x, self.accent.y, self.accent.z, 0.22) } else { self.surface };
                self.draw_shape.draw_abs(cx, r);
            }
            let (title, sub) = (self.rows[i].title.clone(), self.rows[i].sub.clone());
            self.draw_title.color = self.ink;
            Self::fit(&mut self.draw_title, cx, dvec2(view.pos.x + 10.0, y + 3.0), view.size.x - 20.0, &title);
            self.draw_small.color = self.muted;
            Self::fit(&mut self.draw_small, cx, dvec2(view.pos.x + 10.0, y + 16.0), view.size.x - 20.0, &sub);
        }
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let placed = self.area.rect(cx);
        if placed.size.x > 0.0 {
            self.viewport = placed;
        }
        match event.hits(cx, self.area) {
            Hit::FingerUp(e) if e.is_over => {
                if let Some(i) = self.row_at(e.abs) {
                    self.selected = Some(i);
                    self.area.redraw(cx);
                    cx.widget_action(self.uid, ProfListAction::Select(i));
                }
            }
            Hit::FingerScroll(e) => {
                let content = self.rows.len() as f64 * ROW;
                self.scroll = (self.scroll + e.scroll.y).clamp(0.0, (content - self.viewport.size.y).max(0.0));
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let h = self.row_at(e.abs);
                if h != self.hovered {
                    self.hovered = h;
                    self.area.redraw(cx);
                }
                cx.set_cursor(if h.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ the view

#[derive(Clone, Debug, Default)]
pub enum ProfViewAction {
    /// show a fn's source: its file path, line and name (a JIT install's row)
    OpenSource { path: String, line: u32, name: String },
    /// the attached app asks for attention (its JIT ring was clicked): the window comes to the front
    Attention,
    #[default]
    None,
}

/// What the main area shows.
enum Source {
    None,
    Live(Session),
    /// an opened recording: its path and profile
    File(PathBuf, Arc<Mutex<Profile>>),
}

#[derive(Script, ScriptHook, Widget)]
pub struct ProfView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    apps: Vec<AppEntry>,
    #[rust]
    recordings: Vec<PathBuf>,
    #[rust]
    dir: Option<PathBuf>,
    #[rust(Source::None)]
    src: Source,
    #[rust]
    baseline: Option<(PathBuf, Arc<Profile>)>,
    /// the query shown (re-asked as a live profile grows)
    #[rust]
    query: String,
    /// the shown answer comes from the app (a console command): not re-asked
    #[rust]
    console: bool,
    #[rust]
    last_ask: f64,
    #[rust]
    note: String,
    /// the live profile's events looked at so far (an `attention` one brings the window forward)
    #[rust]
    events_seen: usize,
}

impl ProfView {
    /// Where recordings are kept.
    pub fn set_recordings_dir(&mut self, cx: &mut Cx, dir: PathBuf) {
        self.dir = Some(dir);
        self.scan(cx);
    }

    /// Lists the running apps and the recordings again.
    pub fn scan(&mut self, cx: &mut Cx) {
        self.apps = local_apps();
        let attached = match &self.src {
            Source::Live(s) => Some(s.entry.pid),
            _ => None,
        };
        let rows: Vec<ListRow> = self
            .apps
            .iter()
            .map(|a| ListRow { title: a.app.clone(), sub: format!("pid {} · {}{}", a.pid, if a.jit { "makepad run" } else { "native" }, if a.prof { "" } else { " · no profiler stream" }) })
            .collect();
        let sel = attached.and_then(|pid| self.apps.iter().position(|a| a.pid == pid));
        if let Some(mut l) = self.view.widget(cx, ids!(live_apps)).borrow_mut::<ProfList>() {
            l.set_rows(cx, rows, sel);
        }
        let mut recs: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        if let Some(dir) = &self.dir {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "mkpf") {
                        let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                        recs.push((t, p));
                    }
                }
            }
        }
        recs.sort_by(|a, b| b.0.cmp(&a.0));
        self.recordings = recs.into_iter().map(|x| x.1).collect();
        let open = match &self.src {
            Source::File(p, _) => self.recordings.iter().position(|r| r == p),
            _ => None,
        };
        let rows: Vec<ListRow> = self
            .recordings
            .iter()
            .map(|p| {
                let size = std::fs::metadata(p).map_or(0, |m| m.len());
                ListRow { title: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), sub: format!("{} KB", size / 1024) }
            })
            .collect();
        if let Some(mut l) = self.view.widget(cx, ids!(live_recordings)).borrow_mut::<ProfList>() {
            l.set_rows(cx, rows, open);
        }
        self.view.redraw(cx);
    }

    fn profile(&self) -> Option<Arc<Mutex<Profile>>> {
        match &self.src {
            Source::Live(s) => Some(s.profile.clone()),
            Source::File(_, p) => Some(p.clone()),
            Source::None => None,
        }
    }

    /// Attaches to the running app with this pid (devhub's profile window).
    pub fn attach_pid(&mut self, cx: &mut Cx, pid: u32) {
        self.scan(cx);
        match self.apps.iter().position(|a| a.pid == pid) {
            Some(i) => self.attach(cx, i),
            None => {
                self.note = format!("no running makepad app with pid {pid}");
                self.status(cx);
            }
        }
    }

    /// Lets go of the attached app (or opened recording).
    pub fn detach(&mut self, cx: &mut Cx) {
        self.src = Source::None;
        if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
            t.set_profile(cx, None);
        }
        self.show(cx, Ans::new("Detached"));
        self.scan(cx);
        self.status(cx);
    }

    fn attach(&mut self, cx: &mut Cx, i: usize) {
        let Some(entry) = self.apps.get(i).cloned() else { return };
        if let Source::Live(s) = &self.src {
            if s.entry.pid == entry.pid {
                return;
            }
        }
        match Session::attach(&cx.thread_spawner(), entry) {
            Ok(s) => {
                let p = s.profile.clone();
                self.src = Source::Live(s);
                self.events_seen = 0;
                if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
                    t.set_profile(cx, Some(p));
                }
                self.note.clear();
                self.ask(cx, "summary");
            }
            Err(e) => self.note = e,
        }
        self.scan(cx);
        self.status(cx);
    }

    fn open(&mut self, cx: &mut Cx, i: usize) {
        let Some(path) = self.recordings.get(i).cloned() else { return };
        self.open_path(cx, path);
    }

    /// Shows the answer of `query` (a query to show on open).
    pub fn ask_query(&mut self, cx: &mut Cx, query: &str) {
        self.ask(cx, query);
    }

    /// Opens a recording: a `.mkpf` profile, or a GPU recording `.mkgp` (its chunks become the
    /// profile's GPU records).
    pub fn open_path(&mut self, cx: &mut Cx, path: PathBuf) {
        let read = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| {
            if b.starts_with(b"MKGP") {
                let mut f = makepad_prof_stream::prelude().to_vec();
                f.extend_from_slice(&makepad_prof_stream::gpu_records(&b));
                Profile::decode(&f)
            } else {
                Profile::decode(&b)
            }
        });
        match read {
            Ok(p) => {
                let p = Arc::new(Mutex::new(p));
                self.src = Source::File(path, p.clone());
                if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
                    t.set_profile(cx, Some(p));
                    t.set_follow(cx, false);
                }
                self.ask(cx, "summary");
            }
            Err(e) => self.note = format!("cannot open {}: {e}", path.display()),
        }
        self.scan(cx);
        self.status(cx);
    }

    fn status(&mut self, cx: &mut Cx) {
        let (title, status) = match &self.src {
            Source::None => ("Live".to_string(), "attach to a running app or open a recording".to_string()),
            Source::Live(s) => {
                let st = match s.status() {
                    Status::Connecting => "connecting…".to_string(),
                    Status::Live => {
                        let p = s.profile.lock().unwrap();
                        format!("live · {} frames · {} lanes · {} KB{}", p.frames.len(), p.lanes.len(), s.bytes.load(std::sync::atomic::Ordering::Relaxed) / 1024, if s.recording() { " · recording" } else { "" })
                    }
                    Status::Ended(why) => format!("ended: {why}"),
                };
                (format!("{} · pid {}", s.entry.app, s.entry.pid), st)
            }
            Source::File(path, p) => {
                let p = p.lock().unwrap();
                (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), format!("recording · {} frames · {} lanes", p.frames.len(), p.lanes.len()))
            }
        };
        let status = if self.note.is_empty() { status } else { format!("{status} · {}", self.note) };
        self.view.label(cx, ids!(live_title)).set_text(cx, &title);
        self.view.label(cx, ids!(live_status)).set_text(cx, &status);
        let rec = matches!(&self.src, Source::Live(s) if s.recording());
        self.view.button(cx, ids!(live_record)).set_text(cx, if rec { "Stop & save" } else { "Record" });
        let base = match &self.baseline {
            Some((p, _)) => format!("baseline: {}", p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            None => "no baseline (Diff compares against one)".into(),
        };
        self.view.label(cx, ids!(live_baseline_label)).set_text(cx, &base);
    }

    /// Asks `query`: a profiler query on the shown profile, else a command to the attached app.
    fn ask(&mut self, cx: &mut Cx, query: &str) {
        let first = query.split_whitespace().next().unwrap_or("");
        // `source <path>:<line> <fn>`: a row that leads to code
        if first == "source" {
            let mut w = query.split_whitespace().skip(1);
            let at = w.next().unwrap_or("");
            let name = w.collect::<Vec<_>>().join(" ");
            let (path, line) = at.rsplit_once(':').map_or((at.to_string(), 0), |(p, l)| (p.to_string(), l.parse().unwrap_or(0)));
            cx.widget_action(self.view.widget_uid(), ProfViewAction::OpenSource { path, line, name });
            return;
        }
        const PF: [&str; 12] = ["summary", "frames", "frame", "top", "hitches", "timeline", "lanes", "why-slow", "diff", "gpu", "brief", "help"];
        self.query = query.to_string();
        if PF.contains(&first) {
            self.console = false;
            if first == "frame" {
                let w = query.split_whitespace().nth(1).unwrap_or("slowest");
                let id = self.profile().and_then(|p| {
                    let p = p.lock().unwrap();
                    match w {
                        "slowest" => p.frames.iter().max_by_key(|f| f.dur()).map(|f| f.id),
                        "last" => p.frames.last().map(|f| f.id),
                        _ => w.trim_start_matches('F').parse::<u64>().ok(),
                    }
                });
                if let Some(id) = id {
                    if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
                        t.show_frame(cx, id);
                    }
                }
            }
            self.answer_now(cx);
            return;
        }
        self.console = true;
        match &self.src {
            Source::Live(s) => {
                s.ask(query);
                self.show(cx, Ans::new(format!("asking the app: {query}…")));
            }
            _ => self.show(cx, Ans::from(format!("`{query}` is a command for a running app: attach to one first\nmore: summary  help"))),
        }
    }

    fn answer_now(&mut self, cx: &mut Cx) {
        let Some(p) = self.profile() else {
            self.show(cx, Ans::new("Attach to a running app, or open a recording"));
            return;
        };
        let base = self.baseline.as_ref().map(|b| b.1.clone());
        let a = {
            let p = p.lock().unwrap();
            ask::ask(&p, &self.query, base.as_deref())
        };
        if let Some(mut w) = self.view.widget(cx, ids!(live_answer)).borrow_mut::<ProfAnswer>() {
            w.refresh(cx, a);
        }
    }

    fn show(&mut self, cx: &mut Cx, a: Ans) {
        if let Some(mut w) = self.view.widget(cx, ids!(live_answer)).borrow_mut::<ProfAnswer>() {
            w.set_answer(cx, Some(a));
        }
    }

    fn record(&mut self, cx: &mut Cx) {
        let Source::Live(s) = &self.src else { return };
        if !s.recording() {
            s.record();
        } else if let Some(bytes) = s.stop_recording() {
            let name = format!("{}-{}.mkpf", s.entry.app, makepad_civil_stamp());
            match &self.dir {
                Some(dir) => {
                    let _ = std::fs::create_dir_all(dir);
                    let path = dir.join(name);
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => self.note = format!("saved {}", path.display()),
                        Err(e) => self.note = format!("cannot save: {e}"),
                    }
                }
                None => self.note = "no recordings folder".into(),
            }
            self.scan(cx);
        }
        self.status(cx);
    }

    fn console_answers(&mut self, cx: &mut Cx) {
        let Source::Live(s) = &self.src else { return };
        let answers = s.answers();
        if let Some(a) = answers.into_iter().last() {
            // the app's JSON lines as the text answer `makepad inspect` prints (one renderer)
            let req = makepad_remote::Req::parse(&a.line);
            let req = makepad_prof_stream::answer::Request { cmd: req.cmd, pos: req.pos, kv: req.kv };
            let mut ans = Ans::from(makepad_prof_stream::answer::render(&req, &a.lines));
            if let Some(e) = &a.error {
                ans.note(format!("the link: {e}"));
            }
            // a parked panic: what can be decided about it
            for l in &a.lines {
                if let Some(id) = l.split("\"id\":\"").nth(1).and_then(|r| r.split('"').next()).filter(|id| id.starts_with('P')) {
                    ans.more(format!("locals panic={id}  resume {id} retry  resume {id} restart  resume {id} panic"));
                }
            }
            self.show(cx, ans);
        }
    }
}

/// A file-name-safe local timestamp: 20261022-153012.
fn makepad_civil_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // days since the epoch to y-m-d (proleptic Gregorian)
    let days = (secs / 86400) as i64;
    let (y, m, d) = civil(days);
    let s = secs % 86400;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl ProfView {
    /// The attached app's JIT ring was clicked since the last look (an `attention` event of
    /// `makepad run`): it asks for attention.
    fn attention(&mut self) -> bool {
        let Source::Live(s) = &self.src else { return false };
        let p = s.profile.lock().unwrap();
        let new = p.events.get(self.events_seen..).unwrap_or(&[]);
        let asked = new.iter().any(|e| p.strings.get(e.cat as usize).is_some_and(|c| c == "attention"));
        self.events_seen = p.events.len();
        asked
    }
}

impl Widget for ProfView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Signal = event {
            if matches!(self.src, Source::Live(_)) {
                // a live profile grew: redraw the timeline, re-ask the shown query at most 4x a second
                if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
                    t.redraw(cx);
                }
                let now = Cx::time_now();
                if !self.console && now - self.last_ask > 0.25 && !self.query.is_empty() {
                    self.last_ask = now;
                    self.answer_now(cx);
                }
                self.console_answers(cx);
                self.status(cx);
                if self.attention() {
                    cx.push_unique_platform_op(CxOsOp::ActivateApp);
                    cx.widget_action(self.view.widget_uid(), ProfViewAction::Attention);
                }
            }
        }
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}

impl WidgetMatchEvent for ProfView {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        let apps_uid = self.view.widget(cx, ids!(live_apps)).widget_uid();
        let recs_uid = self.view.widget(cx, ids!(live_recordings)).widget_uid();
        let tl_uid = self.view.widget(cx, ids!(live_timeline)).widget_uid();
        let ans_uid = self.view.widget(cx, ids!(live_answer)).widget_uid();
        for action in actions.iter() {
            let Some(a) = action.as_widget_action() else { continue };
            if a.widget_uid == apps_uid {
                if let ProfListAction::Select(i) = a.cast() {
                    self.attach(cx, i);
                }
            } else if a.widget_uid == recs_uid {
                if let ProfListAction::Select(i) = a.cast() {
                    self.open(cx, i);
                }
            } else if a.widget_uid == tl_uid {
                match a.cast() {
                    ProfTimelineAction::SelectFrame(id) => {
                        self.query = format!("frame F{id}");
                        self.console = false;
                        self.answer_now(cx);
                    }
                    ProfTimelineAction::Unfollowed => {}
                    ProfTimelineAction::None => {}
                }
            } else if a.widget_uid == ans_uid {
                if let ProfAnswerAction::Query(q) = a.cast() {
                    self.ask(cx, &q);
                }
            }
        }
        // Analyze: the brief an agent starts from, shown here (each part is a query of its
        // own above) and copied for a lane
        if self.view.button(cx, ids!(live_analyze)).clicked(actions) {
            self.ask(cx, "brief");
            if let Some(p) = self.profile() {
                let text = ask::ask(&p.lock().unwrap(), "brief", None).text();
                cx.copy_to_clipboard(&text);
                self.note = "the brief is on the clipboard".into();
                self.status(cx);
            }
        }
        if self.view.button(cx, ids!(live_refresh)).clicked(actions) {
            self.scan(cx);
        }
        if self.view.button(cx, ids!(live_record)).clicked(actions) {
            self.record(cx);
        }
        if self.view.button(cx, ids!(live_follow)).clicked(actions) {
            if let Some(mut t) = self.view.widget(cx, ids!(live_timeline)).borrow_mut::<ProfTimeline>() {
                let on = !t.follow();
                t.set_follow(cx, on);
            }
        }
        if self.view.button(cx, ids!(live_detach)).clicked(actions) {
            self.detach(cx);
        }
        if self.view.button(cx, ids!(live_baseline)).clicked(actions) {
            if let Source::File(path, p) = &self.src {
                let copy = Profile::decode(&std::fs::read(path).unwrap_or_default()).ok();
                let _ = p;
                if let Some(b) = copy {
                    self.baseline = Some((path.clone(), Arc::new(b)));
                }
            } else {
                self.note = "open a recording to make it the baseline".into();
            }
            self.status(cx);
        }
        if self.view.button(cx, ids!(live_clear_baseline)).clicked(actions) {
            self.baseline = None;
            self.status(cx);
        }
        macro_rules! on_click {
            ($($id:ident => $q:expr),* $(,)?) => {
                $(if self.view.button(cx, ids!($id)).clicked(actions) {
                    self.ask(cx, $q);
                })*
            };
        }
        on_click! {
            q_summary => "summary",
            q_frames => "frames",
            q_hitches => "hitches",
            q_cpu => "top cpu lane=all",
            q_fn => "top fn lane=main",
            q_jit => "top jit",
            q_gpu => "gpu frames",
            q_lanes => "lanes",
            q_why => "why-slow",
            q_diff => "diff",
            c_panics => "panics",
            c_threads => "threads",
            c_memory => "memory",
            c_hung => "why-hung",
            c_asm => "asm top vs=rustc s=2",
        }
        if let Some((text, _)) = self.view.text_input(cx, ids!(live_query)).returned(actions) {
            let q = text.trim().to_string();
            if !q.is_empty() {
                self.ask(cx, &q);
            }
        }
    }
}
