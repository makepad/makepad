//! Automation scripts and their runner (HAL.md 7): a plain-text list of steps that drives an app
//! through the host API of the HAL (`inject`, the clock, `await_frames`, `query`) and reports
//! every step as one JSON line, then a summary.
//!
//! One step per line, `#` starts a comment, arguments are separated by blanks, `"..."` quotes an
//! argument (`\"`, `\\`, `\n` are escapes):
//!
//! ```text
//! wait_frame [n]                        wait until the app answered n more frames (default 1); steps
//!                                       wait 10 s, the run's first wait_frame MAKEPAD_HAL_FIRST_FRAME_S (300)
//! click <widget path | x y>             press and release the primary button at the widget's
//!                                       centre (the path suffix `/snap` reports) or at the point
//! click <widget path> @ <dx> <dy>       the same at (dx, dy) from the widget's top-left corner
//! key <name> [mods]                     key down + up; mods: shift ctrl alt logo, joined by + or blanks
//! text "<s>"                            typed text
//! scroll dx dy x y                      wheel scroll at the point
//! resize w h                            the window takes the size (points)
//! clock advance <ms>                    the HAL clock stops following real time and moves on
//! query <request>                       ask the app, keep the reply for `expect`
//! expect <substring>                    the last query's reply contains it (asked again until it does);
//!                                       a reply line ends with `\n`, so "\t15\n" means the text is exactly 15
//! expect_text <widget path> "<text>"    the visible widget's text is exactly this (asked again until it is)
//! snapshot <name>                       the widget tree now, written to `<snapshot_dir>/<name>.txt` (a backend
//!                                       with a raster also writes the window as `<name>.png`)
//! budget max_frame_ms <n>               longest CPU time Cx spent building one frame
//! budget ledger_bytes <= <n>            most GPU bytes the HAL ledger held at any step
//! budget stale_ids == <n>               ids the HAL refused as stale
//! wait_gpu                              wait until no pipeline is compiling (backends with a GPU)
//! sleep_frames <n>                      n frames (1/60 s each) pass, on the manual clock too; the answer
//!                                       says how many frames the app presented meanwhile
//! domain <app> | domain -               `wait_frame`, `query`, `expect` and `snapshot` go to the domain this
//!                                       app started (waited for until it runs and has a frame), or back
//!                                       to the app itself; input steps always go to the app itself
//! ```
//!
//! The app answers the request `widgets` (generic: every app with a widget tree): one line per
//! widget, tab separated: `path type visible x y w h window text`, positions window-local. The
//! runner stops at the first failing step.

use crate::{input, ledger::Kind, session::World, Hal, Id, RawEvent};
use makepad_key_code::{KeyCode, KEYCODE_VARIANTS};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ the script

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Path(String),
    /// a widget's top-left corner plus an offset
    PathAt(String, f64, f64),
    Point(f64, f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Budget {
    MaxFrameMs(f64),
    LedgerBytes(u64),
    StaleIds(u64),
}

impl Budget {
    pub fn name(&self) -> &'static str {
        match self {
            Budget::MaxFrameMs(_) => "max_frame_ms",
            Budget::LedgerBytes(_) => "ledger_bytes",
            Budget::StaleIds(_) => "stale_ids",
        }
    }

    pub fn limit(&self) -> f64 {
        match *self {
            Budget::MaxFrameMs(v) => v,
            Budget::LedgerBytes(v) | Budget::StaleIds(v) => v as f64,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    WaitFrame(u32),
    Click(Target),
    Key { name: String, key: u32, mods: u32 },
    Text(String),
    Scroll { dx: f64, dy: f64, x: f64, y: f64 },
    Resize { w: f64, h: f64 },
    ClockAdvance(u64),
    Query(String),
    Expect(String),
    /// a visible widget's text, exactly
    ExpectText(String, String),
    Snapshot(String),
    Budget(Budget),
    SleepFrames(u32),
    /// the domain (by app) later queries go to; None: the app itself
    Domain(Option<String>),
    WaitGpu,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// 1-based line in the script file
    pub no: usize,
    /// the line as written, comment cut
    pub text: String,
    pub step: Step,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub lines: Vec<Line>,
}

/// Splits a line into arguments; `#` outside quotes starts a comment.
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '#' {
            break;
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            loop {
                match chars.next() {
                    None => return Err("unterminated quote".into()),
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some(c @ ('"' | '\\')) => s.push(c),
                        _ => return Err("unknown escape (use \\\" \\\\ \\n \\t)".into()),
                    },
                    Some(c) => s.push(c),
                }
            }
            out.push(s);
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                s.push(c);
                chars.next();
            }
            out.push(s);
        }
    }
    Ok(out)
}

fn number<T: std::str::FromStr>(what: &str, s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("{what}: `{s}` is not a number"))
}

fn key_index(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "enter" | "return" => "returnkey".to_string(),
        "esc" => "escape".to_string(),
        "del" => "delete".to_string(),
        l if l.len() == 1 && l.as_bytes()[0].is_ascii_lowercase() => format!("key{l}"),
        l if l.len() == 1 && l.as_bytes()[0].is_ascii_digit() => format!("key{l}"),
        l => l.to_string(),
    };
    KEYCODE_VARIANTS.iter().position(|k| *k != KeyCode::Unknown && format!("{k:?}").to_ascii_lowercase() == alias).map(|i| i as u32)
}

fn modifiers(words: &[String]) -> Result<u32, String> {
    let mut bits = 0;
    for w in words.iter().flat_map(|w| w.split('+')).filter(|w| !w.is_empty()) {
        bits |= match w.to_ascii_lowercase().as_str() {
            "shift" => input::MOD_SHIFT,
            "ctrl" | "control" => input::MOD_CONTROL,
            "alt" | "option" => input::MOD_ALT,
            "logo" | "cmd" | "super" | "meta" => input::MOD_LOGO,
            other => return Err(format!("unknown modifier `{other}` (shift ctrl alt logo)")),
        };
    }
    Ok(bits)
}

fn parse_step(args: &[String]) -> Result<Step, String> {
    let rest = &args[1..];
    let need = |n: usize, usage: &str| if rest.len() == n { Ok(()) } else { Err(format!("usage: {usage}")) };
    let joined = || rest.join(" ");
    Ok(match args[0].as_str() {
        "wait_frame" => match rest {
            [] => Step::WaitFrame(1),
            [n] => Step::WaitFrame(number("wait_frame", n)?),
            _ => return Err("usage: wait_frame [n]".into()),
        },
        "click" => match rest {
            [path] => Step::Click(Target::Path(path.clone())),
            [x, y] => Step::Click(Target::Point(number("click x", x)?, number("click y", y)?)),
            [path, at, x, y] if at == "@" => Step::Click(Target::PathAt(path.clone(), number("click dx", x)?, number("click dy", y)?)),
            _ => return Err("usage: click <widget path | x y | widget path @ dx dy>".into()),
        },
        "key" => {
            let Some(name) = rest.first() else { return Err("usage: key <name> [mods]".into()) };
            let key = key_index(name).ok_or_else(|| format!("unknown key `{name}`"))?;
            Step::Key { name: name.clone(), key, mods: modifiers(&rest[1..])? }
        }
        "text" => {
            need(1, "text \"<s>\"")?;
            Step::Text(rest[0].clone())
        }
        "scroll" => {
            need(4, "scroll dx dy x y")?;
            Step::Scroll { dx: number("dx", &rest[0])?, dy: number("dy", &rest[1])?, x: number("x", &rest[2])?, y: number("y", &rest[3])? }
        }
        "resize" => {
            need(2, "resize w h")?;
            Step::Resize { w: number("w", &rest[0])?, h: number("h", &rest[1])? }
        }
        "clock" => match rest {
            [word, ms] if word == "advance" => Step::ClockAdvance(number("clock advance", ms)?),
            _ => return Err("usage: clock advance <ms>".into()),
        },
        "query" | "expect" if rest.is_empty() => return Err(format!("usage: {} <{}>", args[0], if args[0] == "query" { "request" } else { "substring" })),
        "query" => Step::Query(joined()),
        "expect" => Step::Expect(joined()),
        "expect_text" => {
            need(2, "expect_text <widget path> \"<text>\"")?;
            Step::ExpectText(rest[0].clone(), rest[1].clone())
        }
        "snapshot" => {
            need(1, "snapshot <name>")?;
            if rest[0].is_empty() || rest[0].contains(['/', '\\']) || rest[0].starts_with('.') {
                return Err("a snapshot name is a plain file stem".into());
            }
            Step::Snapshot(rest[0].clone())
        }
        "budget" => {
            let Some(what) = rest.first() else { return Err("usage: budget <max_frame_ms|ledger_bytes|stale_ids> [<= | ==] <n>".into()) };
            let value = match &rest[1..] {
                [v] => v,
                [op, v] if op == "<=" || op == "==" => v,
                _ => return Err("usage: budget <max_frame_ms|ledger_bytes|stale_ids> [<= | ==] <n>".into()),
            };
            Step::Budget(match what.as_str() {
                "max_frame_ms" => Budget::MaxFrameMs(number("max_frame_ms", value)?),
                "ledger_bytes" => Budget::LedgerBytes(number("ledger_bytes", value)?),
                "stale_ids" => Budget::StaleIds(number("stale_ids", value)?),
                other => return Err(format!("unknown budget `{other}` (max_frame_ms ledger_bytes stale_ids)")),
            })
        }
        "wait_gpu" => {
            need(0, "wait_gpu")?;
            Step::WaitGpu
        }
        "sleep_frames" => {
            need(1, "sleep_frames <n>")?;
            Step::SleepFrames(number("sleep_frames", &rest[0])?)
        }
        "domain" => {
            need(1, "domain <app> | domain -")?;
            Step::Domain((rest[0] != "-").then(|| rest[0].clone()))
        }
        other => return Err(format!("unknown step `{other}`")),
    })
}

impl Script {
    pub fn parse(text: &str) -> Result<Script, ParseError> {
        let mut lines = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let no = i + 1;
            let err = |message: String| ParseError { line: no, message };
            let args = tokenize(raw).map_err(err)?;
            if args.is_empty() {
                continue;
            }
            let step = parse_step(&args).map_err(|m| ParseError { line: no, message: m })?;
            let text = raw.split('#').next().unwrap_or(raw).trim().to_string();
            lines.push(Line { no, text, step });
        }
        Ok(Script { lines })
    }
}

impl Script {
    /// Reads and parses a script file; the error names the file and the line.
    pub fn load(path: &std::path::Path) -> Result<Script, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Script::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

// ------------------------------------------------------------------ the widget reply

/// One widget of the `widgets` reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: String,
    pub kind: String,
    pub visible: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// the HAL window the widget is in
    pub window: Option<Id>,
    pub text: String,
}

impl Row {
    pub fn parse(line: &str) -> Option<Row> {
        let mut f = line.splitn(9, '\t');
        let path = f.next()?.to_string();
        let kind = f.next()?.to_string();
        let visible = f.next()? == "1";
        let (x, y, w, h) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        let window = f.next()?.split_once(':').and_then(|(i, g)| Some(Id::new(i.parse().ok()?, g.parse().ok()?)));
        Some(Row { path, kind, visible, x, y, w, h, window, text: f.next().unwrap_or("").to_string() })
    }

    /// The reply line of a widget (the app side writes this).
    pub fn encode(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let window = self.window.map_or("-".to_string(), |w| format!("{}:{}", w.index, w.gen));
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            clean(&self.path),
            clean(&self.kind),
            self.visible as u8,
            self.x,
            self.y,
            self.w,
            self.h,
            window,
            clean(&self.text)
        )
    }
}

/// A widget path matches a query when it is equal or ends with it after a `.`.
pub fn path_matches(path: &str, query: &str) -> bool {
    let query = query.trim_matches('.');
    !query.is_empty() && (path == query || path.strip_suffix(query).is_some_and(|head| head.ends_with('.')))
}

fn parse_rows(reply: &str) -> Vec<Row> {
    reply.lines().filter_map(Row::parse).collect()
}

// ------------------------------------------------------------------ the runner

pub struct Config {
    pub session: u32,
    pub dpi: f32,
    /// longest a step waits for the app (frames, widgets, replies)
    pub step_timeout: Duration,
    /// longest the first `wait_frame` of a run waits (a cold JIT run takes longer than any later
    /// step): `MAKEPAD_HAL_FIRST_FRAME_S`, default 300 s
    pub first_frame_timeout: Duration,
    pub snapshot_dir: Option<PathBuf>,
}

impl Config {
    pub fn new(session: u32, dpi: f32) -> Config {
        // (a cold compile of the app takes longer than a step normally waits)
        let ms = std::env::var("MAKEPAD_HAL_STEP_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()).filter(|ms| *ms > 0).unwrap_or(10_000);
        let first = std::env::var("MAKEPAD_HAL_FIRST_FRAME_S").ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(300);
        Config { session, dpi, step_timeout: Duration::from_millis(ms), first_frame_timeout: Duration::from_secs(first), snapshot_dir: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub step: usize,
    pub line: usize,
    pub cmd: String,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BudgetResult {
    pub budget: Budget,
    pub value: f64,
    pub ok: bool,
}

/// What a run saw, taken before the session closes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub steps: usize,
    pub ran: usize,
    pub failure: Option<Failure>,
    pub budgets: Vec<BudgetResult>,
    pub frames: u64,
    pub draws: u64,
    /// frames answered with a GPU time, and the mean / longest GPU time of them (ns)
    pub gpu_frames: u64,
    pub gpu_ns_mean: u64,
    pub gpu_ns_max: u64,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.failure.is_none()
    }

    /// The summary line, written after the session closed: it reports what the HAL still holds.
    pub fn summary_line(&self, hal: &Hal) -> String {
        let (t, b, p, w) = hal.with_world(|w| (w.count(Kind::Texture), w.count(Kind::Buffer), w.count(Kind::Pipeline), w.count(Kind::Window)));
        let left = hal.report_all(0);
        let failure = match &self.failure {
            Some(f) => format!("{{\"step\":{},\"line\":{},\"cmd\":{},\"why\":{}}}", f.step, f.line, json(&f.cmd), json(&f.why)),
            None => "null".to_string(),
        };
        let budgets: Vec<String> = self
            .budgets
            .iter()
            .map(|b| format!("{{\"name\":\"{}\",\"limit\":{},\"value\":{},\"ok\":{}}}", b.budget.name(), b.budget.limit(), b.value, b.ok))
            .collect();
        format!(
            "{{\"summary\":true,\"ok\":{},\"steps\":{},\"ran\":{},\"first_failure\":{},\"budgets\":[{}],\"frames\":{},\"draws\":{},\"gpu_frames\":{},\"gpu_ns_mean\":{},\"gpu_ns_max\":{},\"ledger_after_close\":{{\"textures\":{t},\"buffers\":{b},\"pipelines\":{p},\"windows\":{w},\"entries\":{},\"bytes\":{}}}}}",
            self.ok(),
            self.steps,
            self.ran,
            failure,
            budgets.join(","),
            self.frames,
            self.draws,
            self.gpu_frames,
            self.gpu_ns_mean,
            self.gpu_ns_max,
            left.items.len(),
            left.total_bytes
        )
    }
}

/// An RGBA8 PNG (stored deflate blocks: no compression, no dependency).
pub fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for b in data {
            c ^= *b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// JSON string literal.
pub fn json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

pub struct Runner {
    script: Script,
    cfg: Config,
    out: Box<dyn Write + Send>,
    /// the request of the last `query` step and what it answered
    last_query: Option<(String, String)>,
    budgets: Vec<Budget>,
    max_ledger_bytes: u64,
    /// frames the session had answered when the last `wait_frame` was satisfied (or the runner was made):
    /// a `wait_frame n` is satisfied by n frames since then, including those that came between
    /// steps (a click's answer can arrive before the next step starts)
    frames_seen: u64,
    /// the session `wait_frame`, `query`, `expect` and `snapshot` go to: the app's own, or a domain it started
    target: u32,
    /// the first `wait_frame` has run (it has the long first-frame limit)
    first_frame_waited: bool,
}

impl Runner {
    /// The run counts the session's frames from here: a frame the app presents before `run`
    /// gets to its first step (the app's thread races the runner's) is one the first `wait_frame` waits for.
    pub fn new(hal: &Hal, script: Script, cfg: Config, out: Box<dyn Write + Send>) -> Runner {
        let target = cfg.session;
        let frames_seen = Self::frames(hal, cfg.session);
        Runner { script, cfg, out, last_query: None, budgets: Vec::new(), max_ledger_bytes: 0, frames_seen, target, first_frame_waited: false }
    }

    /// Runs the steps against the session, one JSON line each; stops at the first failure.
    pub fn run(&mut self, hal: &Hal) -> Outcome {
        let mut outcome = Outcome { steps: self.script.lines.len(), ..Default::default() };
        let lines = self.script.lines.clone();
        for (i, line) in lines.iter().enumerate() {
            let t0 = Instant::now();
            let mut result = self.step(hal, &line.step);
            let violated = self.check_budgets(hal);
            if result.is_ok() {
                if let Some(v) = violated.iter().find(|b| !b.ok) {
                    result = Err(format!("budget {} exceeded: {} > {}", v.budget.name(), v.value, v.budget.limit()));
                }
            }
            outcome.ran = i + 1;
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            let (ok, details) = match &result {
                Ok(d) => (true, d.as_str()),
                Err(d) => (false, d.as_str()),
            };
            let _ = writeln!(self.out, "{{\"step\":{},\"line\":{},\"cmd\":{},\"ok\":{},\"ms\":{:.1},\"details\":{}}}", i + 1, line.no, json(&line.text), ok, ms, json(details));
            let _ = self.out.flush();
            if let Err(why) = result {
                outcome.failure = Some(Failure { step: i + 1, line: line.no, cmd: line.text.clone(), why });
                break;
            }
        }
        outcome.budgets = self.check_budgets(hal);
        let stats = hal.with_world(|w| w.session(self.cfg.session).map(|s| s.stats).unwrap_or_default());
        outcome.frames = stats.frames;
        outcome.draws = stats.draws;
        outcome.gpu_frames = stats.gpu_frames;
        outcome.gpu_ns_mean = stats.gpu_ns_total.checked_div(stats.gpu_frames).unwrap_or(0);
        outcome.gpu_ns_max = stats.gpu_ns_max;
        outcome
    }

    fn check_budgets(&mut self, hal: &Hal) -> Vec<BudgetResult> {
        if let Some(r) = hal.report(self.cfg.session, 0) {
            self.max_ledger_bytes = self.max_ledger_bytes.max(r.total_bytes);
        }
        let stats = hal.with_world(|w| w.session(self.cfg.session).map(|s| s.stats).unwrap_or_default());
        self.budgets
            .iter()
            .map(|b| {
                let value = match b {
                    Budget::MaxFrameMs(_) => stats.max_frame_ns as f64 / 1e6,
                    Budget::LedgerBytes(_) => self.max_ledger_bytes as f64,
                    Budget::StaleIds(_) => stats.stale_ids as f64,
                };
                BudgetResult { budget: *b, value, ok: value <= b.limit() }
            })
            .collect()
    }

    fn ask(&self, hal: &Hal, request: &str) -> Option<String> {
        self.ask_in(hal, self.target, request)
    }

    fn ask_in(&self, hal: &Hal, session: u32, request: &str) -> Option<String> {
        hal.query(session, request.as_bytes(), self.cfg.step_timeout.min(Duration::from_secs(2))).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The window a point-addressed step goes to: the first one the session has.
    fn first_window(&self, hal: &Hal) -> Result<Id, String> {
        let session = self.cfg.session;
        let found = |w: &World| w.session(session).and_then(|s| s.windows.keys_of(session).first().copied());
        let mut window = None;
        hal.wait_until(self.cfg.step_timeout, |w| {
            window = found(w);
            window.is_some()
        });
        window.ok_or_else(|| "the session has no window".to_string())
    }

    fn window_geom(&self, hal: &Hal, window: Id) -> Option<[f32; 4]> {
        hal.with_world(|w| w.session(self.cfg.session).and_then(|s| s.windows.get(self.cfg.session, window).ok().map(|e| e.val.geom)))
    }

    /// Asks for the widget tree until `pick` finds a row (the app needs a few frames to have one).
    fn find_row(&self, hal: &Hal, what: &str, pick: impl Fn(&Row) -> bool) -> Result<Row, String> {
        let deadline = Instant::now() + self.cfg.step_timeout;
        let mut last = Vec::new();
        loop {
            // (the app's own widgets: input steps go to the app itself)
            if let Some(reply) = self.ask_in(hal, self.cfg.session, "widgets") {
                last = parse_rows(&reply);
                if let Some(row) = last.iter().find(|r| r.visible && r.w > 0.0 && r.h > 0.0 && pick(r)) {
                    return Ok(row.clone());
                }
            }
            if Instant::now() >= deadline {
                let seen: Vec<String> = last.iter().filter(|r| r.visible).take(40).map(|r| format!("{}={:?}", r.path, r.text)).collect();
                return Err(format!("no visible widget {what}; widgets: {}", seen.join(" ")));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn frames(hal: &Hal, session: u32) -> u64 {
        hal.with_world(|w| w.session(session).map_or(0, |s| s.stats.frames))
    }

    /// The session of the domain the app started from `app`, once it runs and has a frame.
    fn domain_session(&self, hal: &Hal, app: &str) -> Result<u32, String> {
        let parent = self.cfg.session;
        let mut found = None;
        let ready = hal.wait_until(self.cfg.step_timeout, |w| {
            found = w.domains_of(parent).into_iter().rev().find(|d| d.app == app || d.app.ends_with(app)).map(|d| d.domain);
            found.is_some_and(|d| w.session(d).is_some_and(|s| s.stats.frames > 0))
        });
        match (ready, found) {
            (true, Some(d)) => Ok(d),
            (_, Some(d)) => Err(format!("domain {d} ({app}) has no frame within {} s", self.cfg.step_timeout.as_secs())),
            _ => Err(format!("no domain started from `{app}` within {} s", self.cfg.step_timeout.as_secs())),
        }
    }

    fn step(&mut self, hal: &Hal, step: &Step) -> Result<String, String> {
        let session = self.cfg.session;
        match step {
            Step::Domain(app) => {
                self.target = match app {
                    Some(app) => self.domain_session(hal, app)?,
                    None => session,
                };
                self.frames_seen = Self::frames(hal, self.target);
                Ok(format!("session {}", self.target))
            }
            Step::WaitFrame(n) => {
                let (session, target) = (self.target, self.frames_seen + *n as u64);
                let limit = if std::mem::replace(&mut self.first_frame_waited, true) { self.cfg.step_timeout } else { self.cfg.first_frame_timeout };
                if hal.wait_until(limit, |w| w.session(session).is_some_and(|s| s.stats.frames >= target)) {
                    self.frames_seen = Self::frames(hal, session);
                    Ok(format!("{n} frame(s)"))
                } else {
                    Err(format!("no {n} frame(s) within {} s", limit.as_secs()))
                }
            }
            Step::WaitGpu => {
                // the backend's asynchronous work (pipeline compiles) has finished
                if hal.wait_until(self.cfg.step_timeout, |w| w.session(session).is_some_and(|s| s.pipelines_pending == 0)) {
                    Ok("no pipeline compiling".into())
                } else {
                    Err(format!("pipelines still compiling after {} s", self.cfg.step_timeout.as_secs()))
                }
            }
            Step::Click(target) => {
                let (win, x, y, what) = match target {
                    Target::Point(x, y) => (self.first_window(hal)?, *x, *y, format!("({x}, {y})")),
                    Target::PathAt(path, dx, dy) => {
                        let row = self.find_row(hal, &format!("`{path}`"), |r| path_matches(&r.path, path))?;
                        let win = match row.window {
                            Some(w) => w,
                            None => self.first_window(hal)?,
                        };
                        (win, row.x + dx, row.y + dy, format!("{} at ({}, {})", row.path, row.x + dx, row.y + dy))
                    }
                    Target::Path(path) => {
                        let row = self.find_row(hal, &format!("`{path}`"), |r| path_matches(&r.path, path))?;
                        let win = match row.window {
                            Some(w) => w,
                            None => self.first_window(hal)?,
                        };
                        (win, row.x + row.w / 2.0, row.y + row.h / 2.0, format!("{} at ({}, {})", row.path, row.x + row.w / 2.0, row.y + row.h / 2.0))
                    }
                };
                hal.inject(session, RawEvent::mouse_move(win, x, y, 0.0, 0.0, 0));
                hal.inject(session, RawEvent::mouse_down(win, x, y, 1, 0));
                hal.inject(session, RawEvent::mouse_up(win, x, y, 1, 0));
                Ok(format!("clicked {what}"))
            }
            Step::Key { name, key, mods } => {
                let win = self.first_window(hal)?;
                hal.inject(session, RawEvent::key_down(win, *key, false, *mods));
                hal.inject(session, RawEvent::key_up(win, *key, *mods));
                Ok(format!("key {name}"))
            }
            Step::Text(text) => {
                let win = self.first_window(hal)?;
                hal.with_events(session, |q| q.text(win, input::text::TYPED, text.as_bytes()));
                Ok(format!("{} bytes", text.len()))
            }
            Step::Scroll { dx, dy, x, y } => {
                let win = self.first_window(hal)?;
                hal.inject(session, RawEvent::mouse_move(win, *x, *y, 0.0, 0.0, 0));
                hal.inject(session, RawEvent::scroll(win, *x, *y, *dx, *dy, 0, false, input::scroll::NONE));
                Ok(format!("scrolled ({dx}, {dy}) at ({x}, {y})"))
            }
            Step::Resize { w, h } => {
                let win = self.first_window(hal)?;
                let geom = self.window_geom(hal, win).unwrap_or([0.0; 4]);
                let state = hal.with_world(|wd| wd.session(session).and_then(|s| s.windows.get(session, win).ok().map(|e| e.val.state))).unwrap_or(crate::window_state::RESTORE);
                hal.inject(session, RawEvent::geom(win, geom[0] as f64, geom[1] as f64, *w, *h, self.cfg.dpi, state));
                Ok(format!("{w} x {h}"))
            }
            Step::ClockAdvance(ms) => {
                hal.clock_advance(ms.saturating_mul(1_000_000));
                Ok(format!("clock {} ns", hal.now()))
            }
            Step::Query(request) => {
                let reply = self.ask(hal, request).ok_or_else(|| format!("no answer to `{request}`"))?;
                if let Some(e) = reply.strip_prefix("error:") {
                    return Err(format!("the app refused `{request}`:{e}"));
                }
                let details = format!("{} line(s), {} bytes", reply.lines().count(), reply.len());
                self.last_query = Some((request.clone(), reply));
                Ok(details)
            }
            Step::Expect(needle) => {
                let Some((request, mut reply)) = self.last_query.clone() else { return Err("expect without an earlier query".into()) };
                let deadline = Instant::now() + self.cfg.step_timeout;
                loop {
                    if let Some(at) = reply.find(needle.as_str()) {
                        let start = reply[..at].rfind('\n').map_or(0, |i| i + 1);
                        let end = reply[at..].find('\n').map_or(reply.len(), |i| at + i);
                        return Ok(shorten(&reply[start..end], 200));
                    }
                    if Instant::now() >= deadline {
                        return Err(format!("`{needle}` not in the reply to `{request}`: {}", shorten(&reply.replace('\n', " | "), 400)));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                    if let Some(again) = self.ask(hal, &request) {
                        reply = again;
                        self.last_query = Some((request.clone(), reply.clone()));
                    }
                }
            }
            Step::ExpectText(path, text) => {
                let deadline = Instant::now() + self.cfg.step_timeout;
                let mut seen = Vec::new();
                loop {
                    if let Some(reply) = self.ask(hal, "widgets") {
                        let rows: Vec<Row> = parse_rows(&reply).into_iter().filter(|r| r.visible && path_matches(&r.path, path)).collect();
                        if rows.iter().any(|r| r.text == *text) {
                            return Ok(format!("{path} = {}", shorten(text, 80)));
                        }
                        seen = rows.iter().map(|r| format!("{:?}", r.text)).collect();
                    }
                    if Instant::now() >= deadline {
                        return Err(format!("`{path}` is not {text:?}: visible texts {}", if seen.is_empty() { "(no visible widget)".to_string() } else { seen.join(" ") }));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            Step::Snapshot(name) => {
                let reply = self.ask(hal, "widgets").ok_or("no answer to the widget query")?;
                let rows = reply.lines().count();
                match &self.cfg.snapshot_dir {
                    Some(dir) => {
                        let path = dir.join(format!("{name}.txt"));
                        std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, &reply)).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                        // a backend with a raster also gives the window's pixels
                        let mut shown = format!("{rows} widget(s) -> {}", path.display());
                        if let Some(px) = hal.window_pixels(self.cfg.session, None) {
                            let png = dir.join(format!("{name}.png"));
                            std::fs::write(&png, png_rgba(px.w, px.h, &px.rgba)).map_err(|e| format!("cannot write {}: {e}", png.display()))?;
                            shown.push_str(&format!(", {}x{} pixels -> {}", px.w, px.h, png.display()));
                        }
                        Ok(shown)
                    }
                    None => Ok(format!("{rows} widget(s), {} bytes (no snapshot dir: not saved)", reply.len())),
                }
            }
            Step::Budget(b) => {
                self.budgets.retain(|o| o.name() != b.name());
                self.budgets.push(*b);
                Ok(format!("{} <= {}", b.name(), b.limit()))
            }
            Step::SleepFrames(n) => {
                let ns = *n as u64 * 16_666_667;
                let frames = |hal: &Hal| hal.with_world(|w| w.session(session).map_or(0, |s| s.stats.frames));
                let before = frames(hal);
                if hal.with_world(|w| w.clock.manual.is_some()) {
                    hal.clock_advance(ns);
                }
                std::thread::sleep(Duration::from_nanos(ns));
                // an idle app presents none
                Ok(format!("{n} frame(s), {} presented", frames(hal) - before))
            }
        }
    }
}

/// A run on its own thread next to the app's event loop (the native client's mode): the thread
/// drives the host API, then asks the app to quit; the loop's thread joins it after closing the
/// session.
pub struct Running {
    handle: std::thread::JoinHandle<Outcome>,
}

impl Running {
    /// Starts the steps, one JSON line each on stdout.
    pub fn start(hal: &'static Hal, script: Script, cfg: Config) -> Running {
        let session = cfg.session;
        let mut runner = Runner::new(hal, script, cfg, Box::new(std::io::stdout()));
        let handle = std::thread::spawn(move || {
            let outcome = runner.run(hal);
            hal.with_events(session, |q| q.lifecycle(crate::lifecycle::QUIT_REQUESTED));
            outcome
        });
        Running { handle }
    }

    /// Waits for the run, prints the summary (call after the session closed: it reports what the
    /// HAL still holds) and says whether every step and budget passed.
    pub fn finish(self, hal: &Hal) -> bool {
        match self.handle.join() {
            Ok(outcome) => {
                println!("{}", outcome.summary_line(hal));
                outcome.ok()
            }
            Err(_) => {
                println!("{{\"summary\":true,\"ok\":false,\"error\":\"the automation runner panicked\"}}");
                false
            }
        }
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<Step> {
        Script::parse(text).unwrap().lines.into_iter().map(|l| l.step).collect()
    }

    fn fails(text: &str) -> ParseError {
        Script::parse(text).unwrap_err()
    }

    #[test]
    fn every_step_parses() {
        let steps = parse(
            "# calculator\n\nwait_frame\nwait_frame 3\nclick k_add   # comment\nclick 12.5 30\nkey enter\nkey a shift+ctrl\nkey ArrowLeft alt logo\n\
             text \"1 + \\\"2\\\"\"\nscroll 0 -40 10 20\nresize 800 600\nclock advance 250\nquery widgets\nexpect \"a b\"\nsnapshot first\n\
             budget max_frame_ms 16.5\nbudget ledger_bytes <= 1000\nbudget stale_ids == 0\nsleep_frames 2\nclick tile @ 4 5.5\ndomain counter\ndomain -\nexpect_text display.result 15\n",
        );
        let key = |n| key_index(n).unwrap();
        assert_eq!(
            steps,
            vec![
                Step::WaitFrame(1),
                Step::WaitFrame(3),
                Step::Click(Target::Path("k_add".into())),
                Step::Click(Target::Point(12.5, 30.0)),
                Step::Key { name: "enter".into(), key: key("ReturnKey"), mods: 0 },
                Step::Key { name: "a".into(), key: key("KeyA"), mods: input::MOD_SHIFT | input::MOD_CONTROL },
                Step::Key { name: "ArrowLeft".into(), key: key("ArrowLeft"), mods: input::MOD_ALT | input::MOD_LOGO },
                Step::Text("1 + \"2\"".into()),
                Step::Scroll { dx: 0.0, dy: -40.0, x: 10.0, y: 20.0 },
                Step::Resize { w: 800.0, h: 600.0 },
                Step::ClockAdvance(250),
                Step::Query("widgets".into()),
                Step::Expect("a b".into()),
                Step::Snapshot("first".into()),
                Step::Budget(Budget::MaxFrameMs(16.5)),
                Step::Budget(Budget::LedgerBytes(1000)),
                Step::Budget(Budget::StaleIds(0)),
                Step::SleepFrames(2),
                Step::Click(Target::PathAt("tile".into(), 4.0, 5.5)),
                Step::Domain(Some("counter".into())),
                Step::Domain(None),
                Step::ExpectText("display.result".into(), "15".into()),
            ]
        );
    }

    #[test]
    fn lines_keep_their_numbers_and_text() {
        let s = Script::parse("# c\n\nclick k_add # go\n").unwrap();
        assert_eq!(s.lines.len(), 1);
        assert_eq!((s.lines[0].no, s.lines[0].text.as_str()), (3, "click k_add"));
    }

    #[test]
    fn a_hash_inside_quotes_is_text() {
        assert_eq!(parse("text \"a # b\""), vec![Step::Text("a # b".into())]);
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(fails("wait_frame\nfly 3").line, 2);
        assert!(fails("fly").message.contains("unknown step"));
        assert!(fails("text \"open").message.contains("unterminated"));
        assert!(fails("click").message.contains("usage"));
        assert!(fails("click a b").message.contains("not a number"));
        assert!(fails("key").message.contains("usage"));
        assert!(fails("key nonsense").message.contains("unknown key"));
        assert!(fails("key a hyper").message.contains("unknown modifier"));
        assert!(fails("budget speed 3").message.contains("unknown budget"));
        assert!(fails("budget stale_ids >= 3").message.contains("usage"));
        assert!(fails("clock back 3").message.contains("usage"));
        assert!(fails("snapshot ../x").message.contains("file stem"));
        assert!(fails("query").message.contains("usage"));
        assert!(fails("expect").message.contains("usage"));
        assert!(fails("text \"a\\q\"").message.contains("escape"));
    }

    #[test]
    fn rows_round_trip() {
        let row = Row { path: "root.k_add".into(), kind: "Button".into(), visible: true, x: 10.5, y: 20.0, w: 40.0, h: 30.0, window: Some(Id::new(2, 1)), text: "a\tb".into() };
        let back = Row::parse(&row.encode()).unwrap();
        assert_eq!(back, Row { text: "a b".into(), ..row });
        assert_eq!(Row::parse("x\ty").map(|r| r.path), None);
        let none = Row::parse("p\tT\t0\t1\t2\t3\t4\t-\t").unwrap();
        assert_eq!((none.window, none.visible, none.text.as_str()), (None, false, ""));
    }

    #[test]
    fn paths_match_by_suffix() {
        assert!(path_matches("ui.keys.k_add", "k_add"));
        assert!(path_matches("ui.keys.k_add", "keys.k_add"));
        assert!(!path_matches("ui.keys.xk_add", "k_add"));
        assert!(!path_matches("ui.keys.k_add", ""));
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }
}
