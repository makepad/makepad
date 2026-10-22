//! The query engine agents (and Scope's analysis panel, and other lanes' tools) ask a profile:
//! compact text answers per profproto.md §5. Line 1 is the finding; evidence follows as tables
//! with units in the header; ids (F812, T3) are accepted by later queries; every list says what
//! it left out; the last line offers drill-downs (`more:`).
//!
//! Queries: `summary` | `frames [n=10] [sort=slow|time]` | `frame F<n>` | `top [cpu|jit|gpu]
//! [lane=T<n>|main|all] [range=R<a>-<b>] [n=12]` | `hitches [ms=<n>]` | `timeline F<n>|R<a>-<b>
//! [width=100]` | `lanes` | `why-slow [F<n>|R<a>-<b>]` | `diff` (with a baseline) | `gpu <gpu
//! prof query>` | `help`.

use crate::model::{Frame, Profile};
use crate::text::{self, dur, ms, pct, sig, Legend, Table};
use crate::{jit, role};
use std::collections::HashMap;

/// An answer: the finding (line 1), then blocks of evidence, then drill-down queries. The text
/// rendering ([`Ans::text`]) is what an agent reads; Scope draws the same blocks.
#[derive(Clone, Debug, Default)]
pub struct Ans {
    pub finding: String,
    pub blocks: Vec<Block>,
    /// follow-up queries (`frame F812`, `top jit`): Scope shows them as buttons
    pub more: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Block {
    /// a table's title line
    Title(String),
    Table(Table),
    /// one line of evidence or a note (`+41 smaller, 0.3 ms total`)
    Note(String),
    /// preformatted text (a Unicode timeline)
    Pre(String),
}

impl Ans {
    pub fn new(finding: impl Into<String>) -> Ans {
        Ans { finding: finding.into(), ..Default::default() }
    }
    pub fn table(&mut self, t: Table) {
        self.blocks.push(Block::Table(t));
    }
    pub fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        let s = s.trim_end_matches('\n');
        if !s.is_empty() {
            self.blocks.push(Block::Note(s.to_string()));
        }
    }
    pub fn title(&mut self, s: impl Into<String>) {
        self.blocks.push(Block::Title(s.into()));
    }
    pub fn pre(&mut self, s: impl Into<String>) {
        self.blocks.push(Block::Pre(s.into()));
    }
    /// Drill-downs: queries separated by two spaces.
    pub fn more(&mut self, s: impl Into<String>) {
        for q in s.into().split("  ").map(str::trim).filter(|q| !q.is_empty()) {
            if !self.more.iter().any(|m| m == q) {
                self.more.push(q.to_string());
            }
        }
    }
    /// The text an agent reads.
    pub fn text(&self) -> String {
        let mut out = self.finding.clone();
        for b in &self.blocks {
            out.push('\n');
            match b {
                Block::Title(s) | Block::Note(s) | Block::Pre(s) => out.push_str(s),
                Block::Table(t) => out.push_str(&t.render()),
            }
        }
        if !self.more.is_empty() {
            out.push_str("\nmore: ");
            out.push_str(&self.more.join("  "));
        }
        out
    }
}

/// An answer from its text (a short message, or a rendered [`crate::answer`]): line 1 the
/// finding, `more: ` the drill-downs, markdown tables (a header row, then a `|---|` rule) as
/// tables, other lines notes.
impl From<String> for Ans {
    fn from(s: String) -> Ans {
        let mut a = Ans::default();
        let lines: Vec<&str> = s.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            if i == 0 {
                a.finding = l.to_string();
            } else if let Some(m) = l.strip_prefix("more: ") {
                // queries separated by two spaces (ask) or ` · ` (inspect's answers)
                m.split(" · ").for_each(|q| a.more(q));
            } else if let (Some(head), Some(true)) = (md_cells(l), lines.get(i + 1).map(|r| is_md_rule(r))) {
                let mut t = Table::new(&head.iter().map(String::as_str).collect::<Vec<_>>());
                i += 2;
                while let Some(cells) = lines.get(i).and_then(|r| md_cells(r)) {
                    t.row(cells);
                    i += 1;
                }
                a.table(t);
                continue;
            } else {
                a.note(l);
            }
            i += 1;
        }
        a
    }
}

/// The cells of a markdown table row (`| a | b\|c |`), unescaped and trimmed.
fn md_cells(l: &str) -> Option<Vec<String>> {
    let inner = l.strip_prefix('|')?.strip_suffix('|')?;
    let mut cells = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => cells.last_mut()?.push(chars.next()?),
            '|' => cells.push(String::new()),
            c => cells.last_mut()?.push(c),
        }
    }
    Some(cells.into_iter().map(|c| c.trim().to_string()).collect())
}

fn is_md_rule(l: &str) -> bool {
    l.starts_with('|') && l.contains('-') && l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

impl From<&str> for Ans {
    fn from(s: &str) -> Ans {
        Ans::from(s.to_string())
    }
}

/// A query's words: positional and `k=v`.
pub struct Q {
    pub pos: Vec<String>,
    pub kv: Vec<(String, String)>,
}

impl Q {
    pub fn parse(s: &str) -> Q {
        let mut q = Q { pos: Vec::new(), kv: Vec::new() };
        for w in s.split_whitespace() {
            match w.split_once('=') {
                Some((k, v)) => q.kv.push((k.trim_start_matches("--").to_string(), v.to_string())),
                None => q.pos.push(w.to_string()),
            }
        }
        q
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str())
    }
    pub fn num(&self, k: &str, d: u64) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

pub const HELP: &str = "compare two recordings: `diff` with base=<A.mkpf> (B is the one asked). `brief`: the starting point of an analysis (findings, slowest frame, JIT, how to test a fix). queries: summary | frames [n=10] [sort=slow|time] | frame F<n> | top [cpu|jit|gpu] [lane=main|all|T<n>] [range=R<a>-<b>] [n=12] | hitches [ms=<n>] | timeline F<n>|R<a>-<b> [width=100] | lanes | why-slow [F<n>|R<a>-<b>] | diff (needs a baseline: B) | gpu <frames|frame F<n>|top by=shader|owner|pass|util> | stall [F<n>|slowest] (a hitch from both sides: what the UI waited on and what that lane did) | events [cat=<prefix>] [app=1] [text=] [a=] [range=R<a>-<b>] [n=30] | log | marks (the app's own events) | input [n=] | at <s of the run>|R<ms>|F<n> (every lane at one moment) | memory (resident size and the app heap over time)  (ids: F<n> frame, T<n> lane, R<a>-<b> ms range from the start)";

/// Answers `query` on `p` (`base`: the baseline recording of a diff).
pub fn ask(p: &Profile, query: &str, base: Option<&Profile>) -> Ans {
    let q = Q::parse(query);
    let cmd = q.pos.first().map(String::as_str).unwrap_or("summary");
    if p.lanes.is_empty() && p.gpu.frames.is_empty() && p.events.is_empty() && cmd != "help" {
        return "empty profile: nothing was recorded (is the app drawing? under makepad run the main lane needs the platform's phase markers)\nmore: help".into();
    }
    match cmd {
        "summary" => summary(p),
        "frames" => frames(p, &q),
        "frame" => match q.pos.get(1).map(String::as_str) {
            Some(id) => frame(p, id),
            None => "which frame? frame F<n> or frame slowest\nmore: frames".into(),
        },
        "top" => top(p, &q),
        "hitches" => hitches(p, &q),
        "timeline" => timeline(p, &q),
        "lanes" => lanes(p),
        "why-slow" => why_slow(p, &q),
        "diff" => match base {
            Some(b) => diff(b, p),
            None => "diff needs a baseline recording: ask with A=<baseline.mkpf> (the CLI: `pf diff base=<file>`)\nmore: summary".into(),
        },
        "gpu" => gpu(p, &q),
        "brief" => brief(p),
        "stall" => stall(p, &q),
        "events" => events(p, &q, None),
        "log" => events(p, &q, Some("log")),
        "marks" => events(p, &Q::parse(&format!("{query} app=1")), None),
        "input" => input(p, &q),
        "at" => at_time(p, &q),
        "memory" => memory(p),
        "help" => Ans::new(HELP),
        other => format!("unknown query `{other}`\n{HELP}").into(),
    }
}

// ------------------------------------------------------------------ shared pieces

fn median_p95_max(v: &[u64]) -> (u64, u64, u64) {
    let mut s = v.to_vec();
    s.sort_unstable();
    let at = |q: f64| s.get(((s.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0);
    (at(0.5), at(0.95), at(1.0))
}

/// The frame id named by `F<n>` or `slowest` / `last`.
fn frame_id(p: &Profile, w: &str) -> Result<u64, String> {
    let have = || match (p.frames.first(), p.frames.last()) {
        (Some(a), Some(b)) => format!("have F{}..F{}", a.id, b.id),
        _ => "no frames".into(),
    };
    match w {
        "slowest" => p.frames.iter().max_by_key(|f| f.dur()).map(|f| f.id).ok_or_else(|| format!("no frames recorded ({})", have())),
        "last" => p.frames.last().map(|f| f.id).ok_or_else(|| "no frames recorded".into()),
        _ => {
            let n: u64 = w.trim_start_matches(['F', 'f', 'H']).parse().map_err(|_| format!("`{w}` is not a frame id ({}); try: frames", have()))?;
            p.frame(n).map(|f| f.id).ok_or_else(|| format!("no frame F{n} ({}); try: frames", have()))
        }
    }
}

/// `R<a>-<b>` (ms from the start) or a frame id: the window [t0, t1).
fn window(p: &Profile, w: &str) -> Result<(u64, u64, String), String> {
    if let Some(r) = w.strip_prefix('R') {
        let (a, b) = r.split_once('-').ok_or("a range is R<from ms>-<to ms>")?;
        let a: f64 = a.parse().map_err(|_| "a range is R<from ms>-<to ms>")?;
        let b: f64 = b.parse().map_err(|_| "a range is R<from ms>-<to ms>")?;
        let t0 = p.t_min + (a * 1e6) as u64;
        let t1 = p.t_min + (b * 1e6) as u64;
        if t1 <= t0 {
            return Err("empty range".into());
        }
        return Ok((t0, t1, w.to_string()));
    }
    let id = frame_id(p, w)?;
    let f = p.frame(id).unwrap();
    Ok((f.t0, f.t1, format!("F{id}")))
}

/// ms from the recording's start.
fn at(p: &Profile, t: u64) -> String {
    sig(t.saturating_sub(p.t_min) as f64 / 1e6)
}

/// The lanes of a `lane=` selection.
fn pick_lanes(p: &Profile, sel: Option<&str>) -> Result<Vec<usize>, String> {
    match sel.unwrap_or("main") {
        "all" => Ok((0..p.lanes.len()).collect()),
        "main" => p.main_lane().map(|i| vec![i]).ok_or_else(|| "no main lane; try: lanes".into()),
        "gpu" => Ok((0..p.lanes.len()).filter(|&i| p.lanes[i].thread.role == role::GPU).collect()),
        "compiler" => Ok((0..p.lanes.len()).filter(|&i| p.lanes[i].thread.role == role::COMPILER || p.lanes[i].thread.role == role::OWNER).collect()),
        w => {
            let n: u32 = w.trim_start_matches('T').parse().map_err(|_| format!("`{w}`: a lane is main, all, gpu, compiler or T<n>; try: lanes"))?;
            (0..p.lanes.len()).find(|&i| p.lanes[i].thread.tid == n).map(|i| vec![i]).ok_or_else(|| format!("no lane T{n}; try: lanes"))
        }
    }
}

/// What a lane did in [t0, t1) of a critical path's running step: self time by span, and the
/// time no span covers (on the main lane: idle, waiting for the next event or frame tick; on
/// another lane: work without a marker).
fn run_parts(p: &Profile, lane: usize, t0: u64, t1: u64) -> Vec<(String, u64)> {
    let main = p.main_lane() == Some(lane);
    let mut v: Vec<(String, u64)> = p.exclusive(lane, t0, t1).into_iter().filter(|x| !x.1).map(|(n, _, ns)| (p.s(n).to_string(), ns)).collect();
    let covered: u64 = p.lanes[lane].segs_in(0, t0, t1).iter().filter(|s| !s.wait).map(|s| s.t1.min(t1).saturating_sub(s.t0.max(t0))).sum();
    let gap = (t1 - t0).saturating_sub(covered);
    if gap > 0 {
        let what = if main {
            "idle"
        } else if p.lanes[lane].thread.role == role::OWNER {
            // the owner's own loop: taking the next request, waking the requester
            "between requests"
        } else {
            "busy outside its marked spans"
        };
        v.push((what.to_string(), gap));
    }
    if !main {
        let ln = p.lane_name(lane);
        for x in v.iter_mut() {
            x.0 = format!("{} on {}", x.0, ln);
        }
    }
    v
}

/// What a frame's time went to, in one phrase: the largest critical-path pieces.
fn cause(p: &Profile, f: &Frame) -> String {
    let Some(main) = p.main_lane() else { return String::new() };
    let mut by: Vec<(String, u64)> = Vec::new();
    for st in p.critical_path(main, f.t0, f.t1) {
        let parts = match (st.wait, st.ended_by) {
            (Some(w), Some(by)) => vec![(format!("wait {} (until {})", p.s(w.name), p.lane_name(by)), st.t1 - st.t0)],
            (Some(w), None) => vec![(format!("wait {}", p.s(w.name)), st.t1 - st.t0)],
            (None, _) => run_parts(p, st.lane, st.t0, st.t1),
        };
        for (what, ns) in parts {
            match by.iter_mut().find(|x| x.0 == what) {
                Some(x) => x.1 += ns,
                None => by.push((what, ns)),
            }
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let mut lg = Legend::default();
    by.iter().take(2).filter(|x| x.1 > 0).map(|(w, ns)| format!("{} {}", lg.name(w), dur(*ns))).collect::<Vec<_>>().join(", ")
}

fn jit_counts(p: &Profile, t0: u64, t1: u64) -> (usize, u64, HashMap<u8, usize>) {
    let ev = p.jit_in(t0, t1);
    let mut tiers: HashMap<u8, usize> = HashMap::new();
    let mut compile_ns = 0;
    let mut open: HashMap<(u32, u32), u64> = HashMap::new();
    let mut installs = 0;
    for e in ev {
        match e.ev {
            jit::INSTALL => {
                installs += 1;
                *tiers.entry(e.tier).or_default() += 1;
            }
            jit::COMPILE_BEGIN => {
                open.insert((e.tid, e.name), e.t);
            }
            jit::COMPILE_END => {
                if let Some(b) = open.remove(&(e.tid, e.name)) {
                    compile_ns += e.t - b;
                }
            }
            _ => {}
        }
    }
    // compile jobs are `compile` spans on the lanes that ran them (the main lane's lazy compiles
    // are counted apart)
    let main = p.main_lane();
    for li in 0..p.lanes.len() {
        if Some(li) != main {
            compile_ns += p.compile_ns_in(li, t0, t1);
        }
    }
    (installs, compile_ns, tiers)
}

fn tiers_text(t: &HashMap<u8, usize>) -> String {
    let mut v: Vec<(&u8, &usize)> = t.iter().collect();
    v.sort();
    v.iter().map(|(k, n)| format!("{} {}", n, Profile::tier_name(**k))).collect::<Vec<_>>().join(", ")
}

// ------------------------------------------------------------------ queries

fn summary(p: &Profile) -> Ans {
    let span = p.t_max.saturating_sub(p.t_min);
    let durs = p.frame_durs();
    let mut out = Ans::default();
    if durs.is_empty() {
        out.finding = format!("{} s recorded, no frames (no Draw on the main lane): {} lanes, {} JIT events", sig(span as f64 / 1e9), p.lanes.len(), p.jit.len());
    } else {
        let (med, p95, max) = median_p95_max(&durs);
        let worst = p.frames.iter().max_by_key(|f| f.dur()).unwrap();
        out.finding = format!(
            "{} frames in {} s: median {} ms, p95 {} ms, worst F{} {} ms ({})",
            durs.len(),
            sig(span as f64 / 1e9),
            ms(med),
            ms(p95),
            worst.id,
            ms(worst.dur()),
            cause(p, worst)
        );
        out.note(format!("frame ms  {}  (max {})", text::sparkline(&durs, 100), ms(max)));
    }
    // lanes: busy share of the whole recording
    let mut t = Table::new(&["lane", "role", "busy ms", "busy %", "wait ms"]);
    let mut rows: Vec<(usize, u64, u64)> = (0..p.lanes.len()).filter(|&i| !p.lanes[i].rows.is_empty()).map(|i| {
        let (b, w) = p.busy_in(i, p.t_min, p.t_max + 1);
        (i, b, w)
    }).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let shown = rows.len().min(8);
    for &(i, b, w) in rows.iter().take(shown) {
        let l = &p.lanes[i];
        t.row(vec![format!("T{} {}", l.thread.tid, p.lane_name(i)), role::NAMES.get(l.thread.role as usize).unwrap_or(&"?").to_string(), ms(b), pct(b, span), ms(w)]);
    }
    out.table(t);
    if rows.len() > shown {
        let rest: u64 = rows[shown..].iter().map(|x| x.1).sum();
        out.note(format!("+{} lanes, {} ms busy total", rows.len() - shown, ms(rest)));
    }
    let quiet = p.lanes.iter().filter(|l| l.rows.is_empty()).count();
    if quiet > 0 {
        out.note(format!("+{} threads with no spans (they only woke others or stayed idle)", quiet));
    }
    let (installs, compile_ns, tiers) = jit_counts(p, 0, u64::MAX);
    if installs > 0 || compile_ns > 0 {
        out.note(format!("jit: {} installs ({}), {} ms in compiles", installs, tiers_text(&tiers), ms(compile_ns)));
    }
    if !p.gpu.frames.is_empty() {
        out.note(format!("gpu: {} timed frames ({})", p.gpu.frames.len(), p.gpu.header.device));
    } else if p.lanes.iter().any(|l| l.thread.role == role::GPU) {
        let (_, busy) = p.gpu_lane_busy(p.t_min, p.t_max + 1);
        out.note(format!("gpu: busy {} ms ({}% of the recording)", ms(busy), pct(busy, span)));
    }
    let lost: u64 = p.dropped.iter().map(|d| d.1).sum();
    if lost > 0 {
        out.note(format!("note: {} records dropped by the producer (viewer too slow); totals undercount", lost));
    }
    out.more("frames  hitches  frame slowest  top cpu  top jit  lanes");
    out
}

fn frames(p: &Profile, q: &Q) -> Ans {
    if p.frames.is_empty() {
        return "no frames (no Draw on the main lane)\nmore: lanes  top cpu lane=all".into();
    }
    let n = q.num("n", 10) as usize;
    let mut idx: Vec<usize> = (0..p.frames.len()).collect();
    let by_time = q.get("sort") == Some("time");
    if !by_time {
        idx.sort_by(|a, b| p.frames[*b].dur().cmp(&p.frames[*a].dur()).then(a.cmp(b)));
    }
    if by_time {
        let skip = idx.len().saturating_sub(n);
        idx.drain(..skip);
    }
    let main = p.main_lane();
    let durs = p.frame_durs();
    let (med, p95, _) = median_p95_max(&durs);
    let worst = p.frames[idx[0]];
    let mut out = Ans::new(format!("{} frames, median {} ms, p95 {} ms; {} {}: F{} {} ms",
        p.frames.len(),
        ms(med),
        ms(p95),
        if by_time { "last" } else { "slowest" },
        n.min(idx.len()),
        worst.id,
        ms(worst.dur())
    ));
    // a native app has no JIT column; one lane has no critical path of its own
    let jit = !p.jit.is_empty();
    let multi = p.lanes.iter().filter(|l| !l.rows.is_empty()).count() > 1;
    let mut head = vec!["frame", "at ms", "ms", "main busy ms", "main wait ms"];
    if multi {
        head.push("crit path ms");
    }
    if jit {
        head.push("jit installs");
    }
    head.push("cause");
    let mut t = Table::new(&head);
    for &i in idx.iter().take(n) {
        let f = &p.frames[i];
        let (b, w) = main.map_or((0, 0), |m| p.busy_in(m, f.t0, f.t1));
        let mut row = vec![format!("F{}", f.id), at(p, f.t0), ms(f.dur()), ms(b), ms(w)];
        if multi {
            let cp: u64 = main.map_or(0, |m| p.critical_path(m, f.t0, f.t1).iter().filter(|s| s.wait.is_none()).map(|s| s.t1 - s.t0).sum());
            row.push(ms(cp));
        }
        if jit {
            row.push(jit_counts(p, f.t0, f.t1).0.to_string());
        }
        row.push(cause(p, f));
        t.row(row);
    }
    out.table(t);
    if idx.len() > n {
        out.note(format!("+{} frames not shown", idx.len() - n));
    }
    out.more(format!("frame F{}  hitches  timeline F{}", worst.id, worst.id));
    out
}

fn frame(p: &Profile, w: &str) -> Ans {
    let id = match frame_id(p, w) {
        Ok(id) => id,
        Err(e) => return e.into(),
    };
    let f = *p.frame(id).unwrap();
    let Some(main) = p.main_lane() else { return "no main lane".into() };
    let durs = p.frame_durs();
    let (med, _, _) = median_p95_max(&durs);
    let mut out = Ans::new(format!("F{} took {} ms ({}x the median {} ms): {}", id, ms(f.dur()), sig(f.dur() as f64 / med.max(1) as f64), ms(med), cause(p, &f)));
    let mut lg = Legend::default();
    // critical path, oldest first, merged by lane + what
    let cp = p.critical_path(main, f.t0, f.t1);
    let mut steps: Vec<(usize, String, u64)> = Vec::new();
    for st in &cp {
        let parts = match (st.wait, st.ended_by) {
            (Some(s), Some(by)) => vec![(format!("wait {} → ended by T{}", p.s(s.name), p.lanes[by].thread.tid), st.t1 - st.t0)],
            (Some(s), None) => vec![(format!("wait {}", p.s(s.name)), st.t1 - st.t0)],
            // the lane's own time on the path: its largest part names the step
            (None, _) => {
                let mut v = run_parts(p, st.lane, st.t0, st.t1);
                v.sort_by(|a, b| b.1.cmp(&a.1));
                let ln = format!(" on {}", p.lane_name(st.lane));
                let mut name = v.first().map_or("idle".to_string(), |x| x.0.trim_end_matches(ln.as_str()).to_string());
                if v.len() > 1 {
                    name.push_str(&format!(" (+{} more)", v.len() - 1));
                }
                vec![(name, st.t1 - st.t0)]
            }
        };
        for (what, ns) in parts {
            match steps.last_mut() {
                Some(l) if l.0 == st.lane && l.1 == what => l.2 += ns,
                _ => steps.push((st.lane, what, ns)),
            }
        }
    }
    let mut t = Table::new(&["#", "lane", "ms", "critical path step"]);
    let shown = steps.len().min(12);
    for (k, (l, what, ns)) in steps.iter().enumerate().take(shown) {
        t.row(vec![(k + 1).to_string(), format!("T{}", p.lanes[*l].thread.tid), ms(*ns), lg.name(what)]);
    }
    out.title("critical path (oldest first)");
    out.table(t);
    if steps.len() > shown {
        let rest: u64 = steps[shown..].iter().map(|x| x.2).sum();
        out.note(format!("+{} steps, {} ms", steps.len() - shown, ms(rest)));
    }
    // the main lane's phases, exclusive
    let ex = p.exclusive(main, f.t0, f.t1);
    let mut t = Table::new(&["main phase", "self ms", "%"]);
    let shown = ex.len().min(8);
    for (n, wait, ns) in ex.iter().take(shown) {
        t.row(vec![if *wait { format!("wait {}", lg.name(p.s(*n))) } else { lg.name(p.s(*n)) }, ms(*ns), pct(*ns, f.dur())]);
    }
    out.table(t);
    if ex.len() > shown {
        let rest: u64 = ex[shown..].iter().map(|x| x.2).sum();
        out.note(format!("+{} smaller, {} ms total", ex.len() - shown, ms(rest)));
    }
    // other lanes busy in the frame
    let mut others: Vec<(usize, u64)> = (0..p.lanes.len()).filter(|&i| i != main).map(|i| (i, p.busy_in(i, f.t0, f.t1).0)).filter(|x| x.1 > 0).collect();
    others.sort_by(|a, b| b.1.cmp(&a.1));
    if !others.is_empty() {
        let v: Vec<String> = others.iter().take(6).map(|(i, ns)| format!("T{} {} {}", p.lanes[*i].thread.tid, p.lane_name(*i), dur(*ns))).collect();
        out.note(format!("other lanes busy: {}{}", v.join(", "), if others.len() > 6 { format!(" (+{})", others.len() - 6) } else { String::new() }));
    }
    let (inst, cns, tiers) = jit_counts(p, f.t0, f.t1);
    let main_compile = p.compile_ns_in(main, f.t0, f.t1);
    if inst > 0 || main_compile > 0 {
        out.note(format!("jit in frame: {} installs ({}), main lane compiling {} ms, worker compiles {} ms", inst, tiers_text(&tiers), ms(main_compile), ms(cns)));
    }
    let (gp, gbusy) = p.gpu_lane_busy(f.t0, f.t1);
    if gbusy > 0 {
        let v: Vec<String> = gp.iter().take(4).map(|(n, ns)| format!("{} {}", lg.name(p.s(*n)), dur(*ns))).collect();
        out.note(format!("gpu busy {} ms ({}% of the frame): {}", ms(gbusy), pct(gbusy, f.dur()), v.join(", ")));
    }
    out.note(lg.render());
    out.more(format!("timeline F{id}  top cpu range=F{id}  frame F{}  frame F{}", id.saturating_sub(1), id + 1));
    out
}

fn top(p: &Profile, q: &Q) -> Ans {
    let what = q.pos.get(1).map(String::as_str).unwrap_or("cpu");
    let n = q.num("n", 12) as usize;
    let (t0, t1, wname) = match q.get("range") {
        Some(r) => match window(p, r) {
            Ok(w) => w,
            Err(e) => return e.into(),
        },
        None => (p.t_min, p.t_max + 1, "the recording".to_string()),
    };
    match what {
        "cpu" => {
            let lanes = match pick_lanes(p, q.get("lane")) {
                Ok(l) => l,
                Err(e) => return e.into(),
            };
            let mut acc: HashMap<(u32, bool), (u64, usize)> = HashMap::new();
            let mut total = 0u64;
            for &li in &lanes {
                for (name, wait, ns) in p.exclusive(li, t0, t1) {
                    let e = acc.entry((name, wait)).or_default();
                    e.0 += ns;
                    e.1 += 1;
                    total += ns;
                }
            }
            let mut v: Vec<((u32, bool), (u64, usize))> = acc.into_iter().collect();
            v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            if v.is_empty() {
                return format!("no spans on {} in {}\nmore: lanes", q.get("lane").unwrap_or("main"), wname).into();
            }
            let nframes = p.frames.iter().filter(|f| f.t0 >= t0 && f.t1 <= t1).count().max(1);
            let mut lg = Legend::default();
            let first = &v[0];
            let mut out = Ans::new(format!("top self time on {} in {}: {} {} ms ({}%)",
                q.get("lane").unwrap_or("main"),
                wname,
                lg.name(&format!("{}{}", if first.0 .1 { "wait " } else { "" }, p.s(first.0 .0))),
                ms(first.1 .0),
                pct(first.1 .0, total)
            ));
            let mut t = Table::new(&["span", "self ms", "%", "per frame ms"]);
            for ((name, wait), (ns, _)) in v.iter().take(n) {
                t.row(vec![lg.name(&format!("{}{}", if *wait { "wait " } else { "" }, p.s(*name))), ms(*ns), pct(*ns, total), ms(*ns / nframes as u64)]);
            }
            out.table(t);
            if v.len() > n {
                let rest: u64 = v[n..].iter().map(|x| x.1 .0).sum();
                out.note(format!("+{} smaller, {} ms total", v.len() - n, ms(rest)));
            }
            out.note(lg.render());
            out.more("top cpu lane=all  top jit  frames");
            out
        }
        "jit" => top_jit(p, t0, t1, &wname, n),
        "fn" | "samples" => {
            let tids: Option<Vec<u32>> = match q.get("lane") {
                Some(l) => match pick_lanes(p, Some(l)) {
                    Ok(v) => Some(v.iter().map(|&i| p.lanes[i].thread.tid).collect()),
                    Err(e) => return e.into(),
                },
                None => None,
            };
            top_fn(p, t0, t1, &wname, n, tids.as_deref(), q.get("lane").unwrap_or("all lanes"))
        }
        "gpu" => gpu(p, &Q::parse(&format!("gpu top {}", q.kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")))),
        other => format!("top {other}? top cpu | top fn | top jit | top gpu").into(),
    }
}

/// `top fn`: the CPU stack samples (SAMPLES, the producer's `hz=` sampler): fns by self and
/// inclusive weight, each with the tier it ran at (JIT inline levels are frames of their own).
fn top_fn(p: &Profile, t0: u64, t1: u64, wname: &str, n: usize, tids: Option<&[u32]>, lname: &str) -> Ans {
    let v: Vec<&(u32, u8, crate::SampleRec)> = p.samples.iter().filter(|s| s.2.t >= t0 && s.2.t < t1 && tids.map_or(true, |t| t.contains(&s.0))).collect();
    if v.is_empty() {
        return format!("no CPU samples in {wname}: the stream samples stacks only when opened with hz=N (`pf ... hz=1000`)\nmore: top cpu  summary").into();
    }
    let tier = |t: u8| match t {
        crate::tier::NATIVE => "native",
        crate::tier::BASELINE => "baseline",
        crate::tier::O1 => "O1",
        crate::tier::O2 => "O2",
        crate::tier::O3 => "O3",
        crate::tier::INLINED => "inl",
        _ => "other",
    };
    let total: u64 = v.iter().map(|s| s.2.weight.max(1)).sum();
    let mut selfw: HashMap<(u32, u8), u64> = HashMap::new();
    let mut incl: HashMap<u32, u64> = HashMap::new();
    for s in &v {
        let w = s.2.weight.max(1);
        if let Some(&(f, t)) = s.2.frames.last() {
            *selfw.entry((f, t)).or_default() += w;
        }
        let mut seen: Vec<u32> = Vec::new();
        for &(f, _) in &s.2.frames {
            if !seen.contains(&f) {
                seen.push(f);
                *incl.entry(f).or_default() += w;
            }
        }
    }
    let mut rows: Vec<((u32, u8), u64)> = selfw.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut lg = Legend::default();
    let first = &rows[0];
    let ev = match v[0].1 {
        0 => "CPU",
        1 => "cycles",
        2 => "instructions",
        3 => "branch-miss",
        _ => "cache-miss",
    };
    let mut out = Ans::new(format!("hottest fn on {} in {} ({} samples, {} weighted): {} [{}] {}% self", lname, wname, v.len(), ev, lg.name(p.s(first.0 .0)), tier(first.0 .1), pct(first.1, total)));
    let mut t = Table::new(&["fn", "tier", "self %", "incl %"]);
    for ((f, tr), w) in rows.iter().take(n) {
        t.row(vec![lg.name(p.s(*f)), tier(*tr).to_string(), pct(*w, total), pct(incl.get(f).copied().unwrap_or(0), total)]);
    }
    out.table(t);
    if rows.len() > n {
        let rest: u64 = rows[n..].iter().map(|x| x.1).sum();
        out.note(format!("+{} smaller, {}% self", rows.len() - n, pct(rest, total)));
    }
    out.note(lg.render());
    out.more("top fn lane=main  top cpu  top jit  summary");
    out
}

fn top_jit(p: &Profile, t0: u64, t1: u64, wname: &str, n: usize) -> Ans {
    let ev = p.jit_in(t0, t1);
    if ev.is_empty() {
        return format!("no JIT events in {wname} (a native app, or nothing compiled)\nmore: summary").into();
    }
    // per fn: compile ns, installs, last tier, code bytes; its source
    let mut per: HashMap<u32, (u64, usize, u8, u64)> = HashMap::new();
    let mut src: HashMap<u32, u32> = HashMap::new();
    let mut open: HashMap<(u32, u32), u64> = HashMap::new();
    for e in ev {
        match e.ev {
            jit::COMPILE_BEGIN => {
                open.insert((e.tid, e.name), e.t);
            }
            jit::COMPILE_END => {
                if let Some(b) = open.remove(&(e.tid, e.name)) {
                    per.entry(e.name).or_default().0 += e.t - b;
                }
            }
            jit::INSTALL => {
                let x = per.entry(e.name).or_default();
                x.1 += 1;
                x.2 = e.tier;
                x.3 += e.a;
                if e.src != 0 {
                    src.insert(e.name, e.src);
                }
            }
            _ => {}
        }
    }
    let (installs, cns, tiers) = jit_counts(p, t0, t1);
    let mut v: Vec<(u32, (u64, usize, u8, u64))> = per.into_iter().collect();
    v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.cmp(&a.1 .1)));
    let mut lg = Legend::default();
    let mut out = Ans::new(format!("{} installs in {} ({}), {} ms in compile jobs off the main lane", installs, wname, tiers_text(&tiers), ms(cns)));
    // per-fn compile time only where the producer sent compile begin / end events
    let timed = v.iter().any(|x| x.1 .0 > 0);
    let mut head = vec!["fn"];
    if timed {
        head.push("compile ms");
    }
    head.extend(["installs", "tier now", "code KB", "source"]);
    let mut t = Table::new(&head);
    for (name, (ns, k, tier, bytes)) in v.iter().take(n) {
        let mut row = vec![lg.name(p.s(*name))];
        if timed {
            row.push(ms(*ns));
        }
        // the file's name and line (the full path is the row's link)
        let full = src.get(name).map(|s| p.s(*s)).unwrap_or("");
        let parts: Vec<&str> = full.rsplit('/').take(2).collect();
        let shown = parts.into_iter().rev().collect::<Vec<_>>().join("/");
        row.extend([k.to_string(), Profile::tier_name(*tier).to_string(), sig(*bytes as f64 / 1024.0), shown]);
        let link = (!full.is_empty()).then(|| format!("source {} {}", full, p.s(*name)));
        t.row_link(row, link);
    }
    out.table(t);
    if v.len() > n {
        out.note(format!("+{} fns not shown", v.len() - n));
    }
    out.note(lg.render());
    out.more("top cpu lane=compiler  frames");
    out
}

fn hitches(p: &Profile, q: &Q) -> Ans {
    let durs = p.frame_durs();
    if durs.is_empty() {
        return "no frames\nmore: lanes".into();
    }
    let (med, _, _) = median_p95_max(&durs);
    let refresh_ns = if p.info.refresh_mhz > 0 { 1_000_000_000_000 / p.info.refresh_mhz } else { 16_666_667 };
    let th = match q.get("ms") {
        Some(v) => (v.parse::<f64>().unwrap_or(50.0) * 1e6) as u64,
        None => (med * 2).max(refresh_ns + refresh_ns / 2),
    };
    let hs: Vec<&Frame> = p.frames.iter().filter(|f| f.dur() >= th).collect();
    if hs.is_empty() {
        return format!("no hitches: no frame of {} took {} ms or more (median {} ms)\nmore: frames", p.frames.len(), ms(th), ms(med)).into();
    }
    let lost: u64 = hs.iter().map(|f| f.dur().saturating_sub(med)).sum();
    let mut out = Ans::new(format!("{} hitches (frames >= {} ms) cost {} ms over the median; worst F{} {} ms", hs.len(), ms(th), ms(lost), hs.iter().max_by_key(|f| f.dur()).unwrap().id, ms(hs.iter().map(|f| f.dur()).max().unwrap())));
    let mut t = Table::new(&["frame", "at ms", "ms", "cause"]);
    let n = q.num("n", 12) as usize;
    let mut sorted = hs.clone();
    sorted.sort_by(|a, b| b.dur().cmp(&a.dur()));
    for f in sorted.iter().take(n) {
        t.row(vec![format!("F{}", f.id), at(p, f.t0), ms(f.dur()), cause(p, f)]);
    }
    out.table(t);
    if sorted.len() > n {
        out.note(format!("+{} hitches not shown", sorted.len() - n));
    }
    // the worst hitches read from both sides (what the UI waited on, what that lane did)
    for f in sorted.iter().take(3) {
        if let Some((s, _)) = hitch_sides(p, f) {
            out.note(format!("F{}: {}", f.id, s));
        }
    }
    out.more(format!("stall F{}  frame F{}  why-slow", sorted[0].id, sorted[0].id));
    out
}

fn timeline(p: &Profile, q: &Q) -> Ans {
    let w = q.pos.get(1).map(String::as_str).unwrap_or("slowest");
    let (t0, t1, name) = match window(p, w) {
        Ok(x) => x,
        Err(e) => return e.into(),
    };
    let width = q.num("width", 100) as usize;
    let mut lanes: Vec<usize> = (0..p.lanes.len()).collect();
    // main first, then by busy time in the window
    let main = p.main_lane();
    lanes.sort_by_key(|&i| (Some(i) != main, std::cmp::Reverse(p.busy_in(i, t0, t1).0 + p.busy_in(i, t0, t1).1)));
    let max_lanes = q.num("lanes", 10) as usize;
    let cut = lanes.len().saturating_sub(max_lanes);
    lanes.truncate(max_lanes);
    let mut out = Ans::new(format!("timeline {} ({} ms, {} lanes)", name, ms(t1 - t0), lanes.len()));
    out.pre(text::timeline(p, &lanes, t0, t1, width));
    if cut > 0 {
        out.note(format!("+{} quieter lanes not shown (lanes=N)", cut));
    }
    out.more(format!("frame {name}  top cpu range={name} lane=all"));
    out
}

fn lanes(p: &Profile) -> Ans {
    let span = p.t_max.saturating_sub(p.t_min).max(1);
    let mut out = Ans::new(format!("{} lanes with spans over {} s", p.lanes.iter().filter(|l| !l.rows.is_empty()).count(), sig(span as f64 / 1e9)));
    let mut t = Table::new(&["lane", "name", "role", "os id", "busy %", "spans"]);
    // busiest first, at most 12
    let mut v: Vec<(usize, u64, usize)> = p.lanes.iter().enumerate().filter(|x| !x.1.rows.is_empty()).map(|(i, l)| (i, p.busy_in(i, p.t_min, p.t_max + 1).0, l.rows.iter().map(|r| r.len()).sum())).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    for &(i, b, n) in v.iter().take(12) {
        let l = &p.lanes[i];
        t.row(vec![format!("T{}", l.thread.tid), p.lane_name(i), role::NAMES.get(l.thread.role as usize).unwrap_or(&"?").to_string(), l.thread.os_id.to_string(), pct(b, span), n.to_string()]);
    }
    out.table(t);
    if v.len() > 12 {
        let rest: u64 = v[12..].iter().map(|x| x.1).sum();
        out.note(format!("+{} quieter lanes, {} ms busy in all", v.len() - 12, ms(rest)));
    }
    let quiet = p.lanes.iter().filter(|l| l.rows.is_empty()).count();
    if quiet > 0 {
        out.note(format!("+{} threads with no spans", quiet));
    }
    out.more("top cpu lane=T<n>  timeline slowest");
    out
}

fn why_slow(p: &Profile, q: &Q) -> Ans {
    let (t0, t1, name) = match q.pos.get(1) {
        Some(w) => match window(p, w) {
            Ok(x) => x,
            Err(e) => return e.into(),
        },
        None => (p.t_min, p.t_max + 1, "the recording".to_string()),
    };
    let Some(main) = p.main_lane() else { return "no main lane".into() };
    let frames: Vec<&Frame> = p.frames.iter().filter(|f| f.t1 > t0 && f.t0 < t1).collect();
    // findings, ranked by the ms they explain
    let mut findings: Vec<(u64, String, String)> = Vec::new();
    let durs: Vec<u64> = frames.iter().map(|f| f.dur()).collect();
    let (med, _, _) = median_p95_max(&durs);
    // 1. waits on the critical path, by what
    let mut waits: HashMap<String, (u64, usize, u64)> = HashMap::new();
    let mut runs: HashMap<String, u64> = HashMap::new();
    for f in &frames {
        for st in p.critical_path(main, f.t0, f.t1) {
            match st.wait {
                Some(w) => {
                    let by = st.ended_by.map(|b| format!(" (ended by {})", p.lane_name(b))).unwrap_or_default();
                    let e = waits.entry(format!("wait {}{}", p.s(w.name), by)).or_default();
                    e.0 += st.t1 - st.t0;
                    e.1 += 1;
                    e.2 = f.id;
                }
                None => {
                    for (key, ns) in run_parts(p, st.lane, st.t0, st.t1) {
                        *runs.entry(key).or_default() += ns;
                    }
                }
            }
        }
    }
    let mut lg = Legend::default();
    for (w, (ns, k, last)) in &waits {
        findings.push((*ns, format!("{} blocks the main lane: {} ms over {} frames", lg.name(w), ms(*ns), k), format!("frame F{last}")));
    }
    let mut rv: Vec<(String, u64)> = runs.into_iter().collect();
    rv.sort_by(|a, b| b.1.cmp(&a.1));
    // the main lane idle inside frames is latency (waiting for the next event or frame tick),
    // not work: a finding of its own, ranked below work
    let idle = rv.iter().find(|x| x.0 == "idle").map_or(0, |x| x.1);
    rv.retain(|x| x.0 != "idle");
    if idle > 0 {
        findings.push((idle / 4, format!("the main lane sat idle {} ms inside frames (between an event and the frame's draw: latency, not work)", ms(idle)), "frames".into()));
    }
    for (w, ns) in rv.iter().take(3) {
        let look = if q.pos.get(1).is_some() { format!("top cpu lane=all range={name}") } else { "top cpu lane=all".to_string() };
        findings.push((*ns, format!("{} on the critical path: {} ms ({} ms per frame)", lg.name(w), ms(*ns), ms(*ns / frames.len().max(1) as u64)), look));
    }
    // 2. JIT compiles on the main lane
    let mc = p.compile_ns_in(main, t0, t1);
    if mc > 0 {
        findings.push((mc, format!("lazy compiles on the main lane: {} ms (code compiled on first call)", ms(mc)), "top jit".into()));
    }
    // 3. hitches over the median
    let slow: Vec<&&Frame> = frames.iter().filter(|f| med > 0 && f.dur() > med * 2).collect();
    if !slow.is_empty() {
        let lost: u64 = slow.iter().map(|f| f.dur() - med).sum();
        let w = slow.iter().max_by_key(|f| f.dur()).unwrap();
        findings.push((lost, format!("{} frames over 2x the median ({} ms) cost {} ms; worst F{} ({})", slow.len(), ms(med), ms(lost), w.id, cause(p, w)), format!("frame F{}", w.id)));
    }
    findings.sort_by(|a, b| b.0.cmp(&a.0));
    if findings.is_empty() {
        return format!("nothing stands out in {name}: no waits or compiles on the critical path\nmore: frames  top cpu").into();
    }
    let mut out = Ans::new(format!("{}", findings[0].1));
    let mut t = Table::new(&["#", "ms", "finding", "look"]);
    for (k, (ns, f, look)) in findings.iter().take(8).enumerate() {
        t.row(vec![(k + 1).to_string(), ms(*ns), f.clone(), look.clone()]);
    }
    out.table(t);
    if findings.len() > 8 {
        out.note(format!("+{} smaller findings", findings.len() - 8));
    }
    out.note(lg.render());
    out.more("hitches  frames  top jit");
    out
}

fn diff(a: &Profile, b: &Profile) -> Ans {
    let (am, ap, _) = median_p95_max(&a.frame_durs());
    let (bm, bp, _) = median_p95_max(&b.frame_durs());
    let ch = |x: u64, y: u64| if x == 0 { "-".to_string() } else { format!("{:+.0}%", (y as f64 - x as f64) * 100.0 / x as f64) };
    // -1 faster, 0 within 3%, 1 slower
    let dir = |x: u64, y: u64| if y < x * 97 / 100 { -1 } else if y > x * 103 / 100 { 1 } else { 0 };
    let verdict = match (dir(am, bm), dir(ap, bp)) {
        (-1, d) if d <= 0 => "B is faster than A",
        (1, d) if d >= 0 => "B is slower than A",
        (0, 0) => "B and A are about the same",
        (0, -1) => "B has fewer slow frames than A",
        (0, 1) => "B has more slow frames than A",
        _ => "mixed: B's typical frame and its slow frames moved in opposite directions",
    };
    let mut out = Ans::new(format!("{}: median frame {} → {} ms ({}), p95 {} → {} ms ({})", verdict, ms(am), ms(bm), ch(am, bm), ms(ap), ms(bp), ch(ap, bp)));
    // main lane self time per frame by span
    let per_frame = |p: &Profile| -> HashMap<String, u64> {
        let mut m = HashMap::new();
        if let Some(main) = p.main_lane() {
            let n = p.frames.len().max(1) as u64;
            for (name, wait, ns) in p.exclusive(main, p.t_min, p.t_max + 1) {
                *m.entry(format!("{}{}", if wait { "wait " } else { "" }, p.s(name))).or_default() += ns / n;
            }
        }
        m
    };
    let (ma, mb) = (per_frame(a), per_frame(b));
    let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut rows: Vec<(String, u64, u64)> = keys.iter().map(|k| ((*k).clone(), *ma.get(*k).unwrap_or(&0), *mb.get(*k).unwrap_or(&0))).collect();
    rows.sort_by_key(|r| std::cmp::Reverse((r.2 as i64 - r.1 as i64).unsigned_abs()));
    let mut lg = Legend::default();
    let mut t = Table::new(&["main span", "A ms/frame", "B ms/frame", "change"]);
    let n = 10;
    for (k, x, y) in rows.iter().take(n) {
        t.row(vec![lg.name(k), ms(*x), ms(*y), ch(*x, *y)]);
    }
    out.table(t);
    if rows.len() > n {
        out.note(format!("+{} spans with smaller changes", rows.len() - n));
    }
    let (ai, ac, _) = jit_counts(a, 0, u64::MAX);
    let (bi, bc, _) = jit_counts(b, 0, u64::MAX);
    if ai + bi > 0 {
        out.note(format!("jit: installs {} → {}, compile {} → {} ms", ai, bi, ms(ac), ms(bc)));
    }
    out.note(lg.render());
    out.more("(on B) frames  hitches");
    out
}

/// GPU queries go to gpu_trace's engine (GPUPROF's) on the embedded recording.
fn gpu(p: &Profile, q: &Q) -> Ans {
    if p.gpu.frames.is_empty() {
        let (per, busy) = p.gpu_lane_busy(p.t_min, p.t_max + 1);
        if busy == 0 {
            return "no GPU timing in this profile (open the stream with gpu=1 on a GPU-timed build)\nmore: summary".into();
        }
        let span = p.t_max.saturating_sub(p.t_min);
        let mut lg = Legend::default();
        let mut out = Ans::new(format!("gpu busy {} ms ({}% of the recording); per-pass spans only (no gpu_trace frames)", ms(busy), pct(busy, span)));
        let mut t = Table::new(&["pass", "ms", "%"]);
        for (n, ns) in per.iter().take(10) {
            t.row(vec![lg.name(p.s(*n)), ms(*ns), pct(*ns, busy)]);
        }
        out.table(t);
        if per.len() > 10 {
            out.note(format!("+{} passes", per.len() - 10));
        }
        return out;
    }
    let words: Vec<String> = q.pos.iter().skip(1).cloned().chain(q.kv.iter().map(|(k, v)| format!("{k}={v}"))).collect();
    let args = makepad_gpu_trace::query::Args::parse(&words);
    let own = p.gpu.clone();
    match makepad_gpu_trace::query::run(&p.gpu, &args, &move |_| Ok(own.clone())) {
        Ok(a) => {
            let mut out = Ans::new(a.measured.clone());
            for f in &a.findings {
                out.note(f.clone());
            }
            for l in &a.lines {
                out.note(l.clone());
            }
            out
        }
        Err(e) => format!("{e}\nmore: gpu frames").into(),
    }
}

/// The starting point of an analysis for an agent: the ranked findings, the slowest frame, the
/// JIT's work, and how to test a hypothesis (each a command the agent can run).
fn brief(p: &Profile) -> Ans {
    let ws = why_slow(p, &Q::parse("why-slow"));
    let mut out = Ans::new(format!("{} (pid {}): {}", if p.info.app.is_empty() { "the app" } else { &p.info.app }, p.info.pid, ws.finding));
    // the shape of the recording in two lines: who works most, what the main lane does per frame
    let span = p.t_max.saturating_sub(p.t_min).max(1);
    let mut busy: Vec<(usize, u64)> = (0..p.lanes.len()).filter(|&i| !p.lanes[i].rows.is_empty()).map(|i| (i, p.busy_in(i, p.t_min, p.t_max + 1).0)).collect();
    busy.sort_by(|a, b| b.1.cmp(&a.1));
    let lanes: Vec<String> = busy.iter().take(4).map(|(i, ns)| format!("T{} {} ({}) {}%", p.lanes[*i].thread.tid, p.lane_name(*i), role::NAMES.get(p.lanes[*i].thread.role as usize).unwrap_or(&"?"), pct(*ns, span))).collect();
    out.note(format!("busiest lanes (busy % of {} s): {}", sig(span as f64 / 1e9), lanes.join(", ")));
    if let Some(main) = p.main_lane() {
        let n = p.frames.len().max(1) as u64;
        let ex: Vec<String> = p.exclusive(main, p.t_min, p.t_max + 1).into_iter().filter(|x| !x.1).take(4).map(|(nm, _, ns)| format!("{} {}", p.s(nm), ms(ns / n))).collect();
        out.note(format!("main lane self time per frame (ms, {} frames): {}", p.frames.len(), ex.join(", ")));
    }
    out.blocks.extend(ws.blocks);
    if out.text().contains("busy outside its marked spans") {
        out.note("busy outside its marked spans = a lane working where it has no profiler span (no phase marker covers it); name it by sampling that lane's CPU (see the commands below)");
    }
    if let Some(f) = p.frames.iter().max_by_key(|f| f.dur()) {
        if let Some((s, _)) = hitch_sides(p, f) {
            out.note(format!("slowest frame F{} ({} ms): {}", f.id, ms(f.dur()), s));
        }
    }
    if !p.events.is_empty() {
        let mut by: Vec<(&str, usize)> = Vec::new();
        for e in &p.events {
            let c = p.s(e.cat);
            match by.iter_mut().find(|x| x.0 == c) {
                Some(x) => x.1 += 1,
                None => by.push((c, 1)),
            }
        }
        by.sort_by(|a, b| b.1.cmp(&a.1));
        out.note(format!("events: {} ({}); input: {} events", p.events.len(), by.iter().take(8).map(|(c, n)| format!("{c} {n}")).collect::<Vec<_>>().join(", "), p.inputs.len()));
        for e in p.events.iter().filter(|e| matches!(p.s(e.cat), "panic" | "fault" | "log.error" | "scripterr")).rev().take(3) {
            out.note(format!("{} at {} s: {}", p.s(e.cat), run_s(p, e.t), cut(&e.text, 140)));
        }
    }
    if let Some(id) = p.frames.iter().max_by_key(|f| f.dur()).map(|f| f.id) {
        let fr = frame(p, &format!("F{id}"));
        out.title(format!("slowest frame: {}", fr.finding));
        out.blocks.extend(fr.blocks.into_iter().filter(|b| matches!(b, Block::Table(_))).take(1));
    }
    if !p.jit.is_empty() {
        let j = top_jit(p, p.t_min, p.t_max + 1, "the recording", 5);
        out.title(j.finding.clone());
        out.blocks.extend(j.blocks);
    }
    let app = if p.info.pid != 0 { p.info.pid.to_string() } else { "<pid>".into() };
    out.title("to test a hypothesis (makepad inspect <app> ...)".to_string());
    let mut t = Table::new(&["what", "command"]);
    t.row(vec!["record a baseline".into(), format!("makepad inspect {app} pf summary s=5 save=a.mkpf")]);
    t.row(vec!["after a change: compare".into(), format!("makepad inspect {app} pf diff s=5 base=a.mkpf")]);
    t.row(vec!["pin a fn's tier (miscompile / tier cost)".into(), format!("makepad inspect {app} tier <fn> o1|o3|auto")]);
    t.row(vec!["count calls and time of a fn".into(), format!("makepad inspect {app} count <fn> time=1")]);
    t.row(vec!["probe a fn (log args, histograms)".into(), format!("makepad inspect {app} probe add <fn> <code>")]);
    t.row(vec!["code versions of a fn".into(), format!("makepad inspect {app} versions <fn>")]);
    t.row(vec!["sample a lane's CPU (unmarked work)".into(), format!("makepad inspect {app} prof s=3 threads=<owner|main|compiler>")]);
    out.table(t);
    out.more("frames  hitches  stall slowest  events  top cpu lane=all  top jit  timeline slowest");
    out
}

// ------------------------------------------------------------------ flight recorder queries

/// `t` as seconds of the run (the process's start; else of the recording), 3 digits.
fn run_s(p: &Profile, t: u64) -> String {
    let base = if p.info.start_mono > 0 && p.info.start_mono <= t { p.info.start_mono } else { p.t_min };
    sig(t.saturating_sub(base) as f64 / 1e9)
}

/// A time a query names: `<s>` (seconds of the run), `<s>s`, `R<ms>` (ms from the recording's
/// start), `F<n>` (the frame's end).
fn time_arg(p: &Profile, w: &str) -> Result<u64, String> {
    if let Some(r) = w.strip_prefix('R') {
        let v: f64 = r.parse().map_err(|_| format!("`{w}`: R<ms from the start>"))?;
        return Ok(p.t_min + (v * 1e6) as u64);
    }
    if w.starts_with('F') {
        let id = frame_id(p, w)?;
        return Ok(p.frame(id).map_or(p.t_max, |f| f.t1));
    }
    let v: f64 = w.trim_end_matches('s').parse().map_err(|_| format!("`{w}`: a time is <s of the run>, R<ms from the start> or F<n>"))?;
    let base = if p.info.start_mono > 0 { p.info.start_mono } else { p.t_min };
    Ok(base + (v * 1e9) as u64)
}

/// The host's and the platform's own event categories (everything else is the app's).
fn is_host_cat(c: &str) -> bool {
    const PREFIXES: &[&str] = &["run.", "log", "panic", "window.", "seed.", "replay.", "zone.", "cache.", "fault", "script."];
    const KINDS: &[&str] = &["hitch", "mark", "hang", "deadlock", "mem", "spike", "rtstall", "scripterr", "patchrefused", "patchverified", "patchreverted"];
    PREFIXES.iter().any(|p| c.starts_with(p)) || KINDS.contains(&c)
}

/// What lane `other` did during the waits `ws` (intervals) of another lane: the one sentence
/// that names the other side of a UI stall.
fn other_side(p: &Profile, other: usize, ws: &[(u64, u64)]) -> String {
    let lane = &p.lanes[other];
    let total: u64 = ws.iter().map(|w| w.1.saturating_sub(w.0)).sum();
    // its root spans in the waits, by overlap
    let mut roots: Vec<(u32, u64)> = Vec::new();
    let mut inner: HashMap<u32, u64> = HashMap::new();
    let mut busy = 0u64;
    let mut waiting = 0u64;
    for &(t0, t1) in ws {
        for s in lane.segs_in(0, t0, t1).iter().filter(|s| !s.wait) {
            let o = s.t1.min(t1).saturating_sub(s.t0.max(t0));
            match roots.iter_mut().find(|x| x.0 == s.name) {
                Some(x) => x.1 += o,
                None => roots.push((s.name, o)),
            }
        }
        for (n, w, ns) in p.exclusive(other, t0, t1) {
            if !w {
                *inner.entry(n).or_default() += ns;
            }
        }
        let (b, w) = p.busy_in(other, t0, t1);
        busy += b;
        waiting += w;
    }
    roots.sort_by(|a, b| b.1.cmp(&a.1));
    let covered: u64 = roots.iter().map(|r| r.1).sum();
    let mut parts: Vec<String> = roots.iter().take(2).map(|(nm, ns)| format!("`{}` {} ms", p.s(*nm), ms(*ns))).collect();
    if roots.len() > 2 {
        parts.push(format!("+{} other requests {} ms", roots.len() - 2, ms(roots.iter().skip(2).map(|r| r.1).sum())));
    }
    let gap = total.saturating_sub(covered);
    let mut doing = if parts.is_empty() {
        if waiting > total / 2 {
            "itself waiting".to_string()
        } else if busy == 0 {
            "outside its marked spans (no span covers it)".to_string()
        } else {
            format!("busy {} ms", ms(busy))
        }
    } else {
        let own = if roots.first().is_some_and(|r| p.s(r.0).contains(" for main at ")) { " (the UI's own requests: its lazy compiles and links)" } else { "" };
        format!("serving {}{}", parts.join(", "), own)
    };
    if !parts.is_empty() && gap > total / 10 {
        let what = if lane.thread.role == role::OWNER { "between requests (taking the next one, wakes)" } else { "outside its marked spans" };
        doing.push_str(&format!("; {} ms {}", ms(gap), what));
    }
    let mut inn: Vec<(u32, u64)> = inner.into_iter().filter(|x| !roots.iter().take(2).any(|r| r.0 == x.0)).collect();
    inn.sort_by(|a, b| b.1.cmp(&a.1));
    let inner_s: Vec<String> = inn.iter().take(2).filter(|x| x.1 > total / 50).map(|(n, ns)| format!("{} {} ms", p.s(*n), ms(*ns))).collect();
    // stall samples of that lane in the waits (the recorder samples both sides of a long stall)
    let tid = lane.thread.tid;
    let mut by_fn: HashMap<u32, u64> = HashMap::new();
    let mut sw = 0u64;
    for (_, _, s) in p.samples.iter().filter(|x| x.0 == tid && ws.iter().any(|w| x.2.t >= w.0 && x.2.t <= w.1)) {
        if let Some(&(f, _)) = s.frames.last() {
            *by_fn.entry(f).or_default() += s.weight;
            sw += s.weight;
        }
    }
    let mut hot: Vec<(u32, u64)> = by_fn.into_iter().collect();
    hot.sort_by(|a, b| b.1.cmp(&a.1));
    let mut lg = Legend::default();
    let hot_s: Vec<String> = hot.iter().take(3).map(|(f, w)| format!("{} {}%", lg.name(p.s(*f)), pct(*w, sw))).collect();
    let mut s = format!("{} was {}", p.lane_name(other), doing);
    if !inner_s.is_empty() {
        s.push_str(&format!("; inside: {}", inner_s.join(", ")));
    }
    if !hot_s.is_empty() {
        s.push_str(&format!("; its stack samples: {}", hot_s.join(", ")));
    }
    s
}

/// The main lane's waits on the critical path of frame `f`, grouped by the lane that ended them,
/// the largest group with both sides: (sentence, wait ns).
fn hitch_sides(p: &Profile, f: &Frame) -> Option<(String, u64)> {
    let main = p.main_lane()?;
    let owner = p.lanes.iter().position(|l| l.thread.role == role::OWNER);
    // the main lane's waits in the frame by (the lane that ended them, wait name)
    let mut groups: Vec<((usize, u32), Vec<(u64, u64)>)> = Vec::new();
    let lane = &p.lanes[main];
    for d in 0..lane.rows.len() {
        for w in lane.segs_in(d, f.t0, f.t1).iter().filter(|s| s.wait) {
            // a wait no wake ended (cut, or its wake not recorded): an owner wait reads the owner
            let other = match p.woke_by(main, w).map(|x| x.0) {
                Some(o) => o,
                None if p.s(w.name).starts_with("owner") => match owner {
                    Some(o) => o,
                    None => continue,
                },
                None => continue,
            };
            let iv = (w.t0.max(f.t0), w.t1.min(f.t1));
            match groups.iter_mut().find(|g| g.0 == (other, w.name)) {
                Some(g) => g.1.push(iv),
                None => groups.push(((other, w.name), vec![iv])),
            }
        }
    }
    let sum = |v: &Vec<(u64, u64)>| v.iter().map(|w| w.1.saturating_sub(w.0)).sum::<u64>();
    let ((other, wname), mut ws) = groups.into_iter().max_by_key(|g| sum(&g.1))?;
    ws.sort_unstable();
    let wait_ns = sum(&ws);
    if wait_ns < f.dur() / 4 {
        return None;
    }
    let longest = ws.iter().max_by_key(|w| w.1 - w.0).copied().unwrap_or((0, 0));
    let how = if ws.len() == 1 { String::new() } else { format!(" in {} waits (longest {} ms at {} s)", ws.len(), ms(longest.1 - longest.0), run_s(p, longest.0)) };
    Some((format!("at {} s of the run the UI waited {} ms on {}{}: {}", run_s(p, ws[0].0), ms(wait_ns), p.s(wname), how, other_side(p, other, &ws)), wait_ns))
}

/// `stall [F<n>|slowest]`: a hitch read from both sides.
fn stall(p: &Profile, q: &Q) -> Ans {
    let w = q.pos.get(1).map(String::as_str).unwrap_or("slowest");
    let id = match frame_id(p, w) {
        Ok(id) => id,
        Err(e) => return e.into(),
    };
    let f = *p.frame(id).unwrap();
    let mut out = match hitch_sides(p, &f) {
        Some((s, _)) => Ans::new(format!("F{} ({} ms): {}", id, ms(f.dur()), s)),
        None => Ans::new(format!("F{} ({} ms) waited on no other lane for most of its time: {}", id, ms(f.dur()), cause(p, &f))),
    };
    if f.waits > 0 || f.compiles > 0 {
        out.note(format!("frame summary: {} waits {} ms (on the owner {} ms), {} lazy compiles {} ms, idle {} ms", f.waits, ms(f.wait_ns), ms(f.owner_ns), f.compiles, ms(f.compile_ns), ms(f.idle)));
    }
    // events in and around the frame
    let evs: Vec<&crate::model::Event> = p.events.iter().filter(|e| e.t + 500_000_000 >= f.t0 && e.t <= f.t1 + 100_000_000).collect();
    if !evs.is_empty() {
        let mut t = Table::new(&["at s", "lane", "event", "what"]);
        for e in evs.iter().take(12) {
            t.row(vec![run_s(p, e.t), lane_label(p, e.tid), p.s(e.cat).to_string(), cut(&e.text.replace('\u{1f}', " "), 100)]);
        }
        out.title("events around it".to_string());
        out.table(t);
    }
    out.more(format!("frame F{id}  timeline F{id}  at F{id}  events range=R{}-{}", sig(f.t0.saturating_sub(p.t_min) as f64 / 1e6 - 2000.0).trim_start_matches('-'), sig(f.t1.saturating_sub(p.t_min) as f64 / 1e6 + 500.0)));
    out
}

fn lane_label(p: &Profile, tid: u32) -> String {
    match p.lane_by_tid(tid) {
        Some(l) => {
            let i = p.lanes.iter().position(|x| x.thread.tid == l.thread.tid).unwrap_or(0);
            if p.main_lane() == Some(i) { "main".into() } else { cut(&p.lane_name(i), 24) }
        }
        None => "-".into(),
    }
}

/// `events [cat=<prefix>] [app=1] [text=<word>] [a=<n>] [range=R<a>-<b>] [n=30]`.
fn events(p: &Profile, q: &Q, cat_default: Option<&str>) -> Ans {
    if p.events.is_empty() {
        return "no events recorded (app marks, logs, panics, hitches: the flight recorder's; a live stream only has those since it attached)\nmore: summary".into();
    }
    let cat = q.get("cat").or(cat_default);
    let app_only = q.get("app").is_some_and(|v| v == "1");
    let needle = q.get("text");
    let a = q.get("a").and_then(|v| v.parse::<u64>().ok());
    let (t0, t1) = match q.get("range") {
        Some(r) => match window(p, r) {
            Ok(w) => (w.0, w.1),
            Err(e) => return e.into(),
        },
        None => (0, u64::MAX),
    };
    let sel: Vec<&crate::model::Event> = p
        .events
        .iter()
        .filter(|e| e.t >= t0 && e.t < t1)
        .filter(|e| cat.map_or(true, |c| p.s(e.cat).starts_with(c)))
        .filter(|e| !app_only || !is_host_cat(p.s(e.cat)))
        .filter(|e| needle.map_or(true, |n| e.text.contains(n)))
        .filter(|e| a.map_or(true, |a| e.a == a))
        .collect();
    let mut by: Vec<(&str, usize)> = Vec::new();
    for e in &p.events {
        let c = p.s(e.cat);
        match by.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += 1,
            None => by.push((c, 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let cats: Vec<String> = by.iter().take(10).map(|(c, n)| format!("{c} {n}")).collect();
    if sel.is_empty() {
        return format!("no events match (have {}: {})\nmore: events", p.events.len(), cats.join(", ")).into();
    }
    let n = q.num("n", 30) as usize;
    let mut by_sel: Vec<(&str, usize)> = Vec::new();
    for e in &sel {
        let c = p.s(e.cat);
        match by_sel.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += 1,
            None => by_sel.push((c, 1)),
        }
    }
    by_sel.sort_by(|a, b| b.1.cmp(&a.1));
    let cats: Vec<String> = by_sel.iter().take(10).map(|(c, n)| format!("{c} {n}")).collect();
    let mut out = Ans::new(format!(
        "{} events{} from {} s to {} s of the run; by kind: {}",
        sel.len(),
        cat.map_or(String::new(), |c| format!(" of `{c}`")),
        run_s(p, sel[0].t),
        run_s(p, sel[sel.len() - 1].t),
        cats.join(", ")
    ));
    let mut t = Table::new(&["at s", "lane", "event", "a", "what"]);
    // the last n (the moments before a bug are the interesting ones)
    let from = sel.len().saturating_sub(n);
    for e in &sel[from..] {
        t.row(vec![run_s(p, e.t), lane_label(p, e.tid), p.s(e.cat).to_string(), if e.a == 0 { String::new() } else { e.a.to_string() }, cut(&e.text.replace('\u{1f}', " "), 110)]);
    }
    out.table(t);
    if from > 0 {
        out.note(format!("+{} earlier events not shown (n=, range=R<a>-<b>, cat=, text=, a=)", from));
    }
    out.more("events app=1  events cat=log.error  events cat=panic  stall slowest  input");
    out
}

/// `input [n=20]`: the recorded input (what a replay feeds back).
fn input(p: &Profile, q: &Q) -> Ans {
    if p.inputs.is_empty() {
        return "no input recorded (the program records its input under makepad run with the flight recorder on)\nmore: events".into();
    }
    let mut by: Vec<(String, usize)> = Vec::new();
    for i in &p.inputs {
        let mut w = i.command.split_whitespace();
        let k = format!("{} {}", w.next().unwrap_or(""), w.next().unwrap_or(""));
        match by.iter_mut().find(|x| x.0 == k) {
            Some(x) => x.1 += 1,
            None => by.push((k, 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let first = &p.inputs[0];
    let last = &p.inputs[p.inputs.len() - 1];
    let mut out = Ans::new(format!(
        "{} input events from {} s (frame {}) to {} s (frame {}): {}",
        p.inputs.len(),
        run_s(p, first.t),
        first.frame,
        run_s(p, last.t),
        last.frame,
        by.iter().take(8).map(|(k, n)| format!("{} {}", k.trim(), n)).collect::<Vec<_>>().join(", ")
    ));
    let n = q.num("n", 20) as usize;
    let mut t = Table::new(&["at s", "frame", "command"]);
    let from = p.inputs.len().saturating_sub(n);
    for i in &p.inputs[from..] {
        t.row(vec![run_s(p, i.t), i.frame.to_string(), cut(&i.command, 100)]);
    }
    out.table(t);
    if from > 0 {
        out.note(format!("+{} earlier input events not shown (n=)", from));
    }
    out.more("rec replay (makepad inspect file=<this file> rec replay [until=<s>])  events");
    out
}

/// `at <s>|R<ms>|F<n>`: what every lane was doing at one moment, the frame it was in and the
/// events just before.
fn at_time(p: &Profile, q: &Q) -> Ans {
    let Some(w) = q.pos.get(1) else { return "at <s of the run> | R<ms> | F<n>\nmore: events".into() };
    let t = match time_arg(p, w) {
        Ok(t) => t,
        Err(e) => return e.into(),
    };
    let frame = p.frames.iter().find(|f| f.t0 <= t && t <= f.t1);
    let mut out = Ans::new(format!(
        "at {} s of the run{}",
        run_s(p, t),
        match frame {
            Some(f) => format!(" (inside F{}, {} ms)", f.id, ms(f.dur())),
            None => String::new(),
        }
    ));
    let mut tab = Table::new(&["lane", "role", "doing", "since s"]);
    for (i, l) in p.lanes.iter().enumerate() {
        // the innermost open record at t, waits included
        let mut found: Option<crate::model::Seg> = None;
        for row in &l.rows {
            let k = row.partition_point(|s| s.t0 <= t);
            if k > 0 && row[k - 1].t1 >= t {
                found = Some(row[k - 1]);
            }
        }
        let Some(s) = found else { continue };
        tab.row(vec![p.lane_name(i), role::NAMES.get(l.thread.role as usize).unwrap_or(&"?").to_string(), format!("{}{}", if s.wait { "wait " } else { "" }, cut(p.s(s.name), 80)), run_s(p, s.t0)]);
    }
    if tab.rows.is_empty() {
        out.note("no lane had an open span then (idle)");
    } else {
        out.table(tab);
    }
    let before: Vec<&crate::model::Event> = p.events.iter().filter(|e| e.t <= t && e.t + 5_000_000_000 >= t).collect();
    if !before.is_empty() {
        let mut et = Table::new(&["at s", "event", "what"]);
        for e in before.iter().rev().take(8).rev() {
            et.row(vec![run_s(p, e.t), p.s(e.cat).to_string(), cut(&e.text, 100)]);
        }
        out.title("events in the 5 s before".to_string());
        out.table(et);
    }
    if let Some(i) = p.inputs.iter().rposition(|x| x.t <= t) {
        out.note(format!("last input before: {} s frame {}: {}", run_s(p, p.inputs[i].t), p.inputs[i].frame, p.inputs[i].command));
    }
    out.more("events  stall slowest  timeline slowest");
    out
}

/// `s` cut to `n` chars (an ellipsis says so).
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut o: String = s.chars().take(n.saturating_sub(1)).collect();
    o.push('…');
    o
}

/// `memory`: the recorder's memory counters over time (resident size, the app's heap): first,
/// last, peak, growth and a sparkline each.
fn memory(p: &Profile) -> Ans {
    let mut names: Vec<u32> = Vec::new();
    for c in &p.counters {
        if c.tid == 0 && !names.contains(&c.counter) {
            names.push(c.counter);
        }
    }
    if names.is_empty() {
        return "no memory counters recorded (the flight recorder writes rss and app_heap once a second)\nmore: summary".into();
    }
    let mb = |v: i64| sig(v as f64 / (1 << 20) as f64);
    let mut t = Table::new(&["counter", "first MB", "last MB", "peak MB", "growth MB", "over s", "trend"]);
    let mut finding = String::new();
    for n in names {
        let v: Vec<&crate::CounterRec> = p.counters.iter().filter(|c| c.counter == n && c.tid == 0).collect();
        let (first, last) = (v[0], v[v.len() - 1]);
        let peak = v.iter().map(|c| c.value).max().unwrap_or(0);
        let vals: Vec<u64> = v.iter().map(|c| c.value.max(0) as u64).collect();
        let secs = last.t.saturating_sub(first.t) as f64 / 1e9;
        if finding.is_empty() {
            finding = format!("{} {} MB -> {} MB over {} s (peak {} MB)", p.s(n), mb(first.value), mb(last.value), sig(secs), mb(peak));
        }
        t.row(vec![p.s(n).to_string(), mb(first.value), mb(last.value), mb(peak), mb(last.value - first.value), sig(secs), text::sparkline(&vals, 40)]);
    }
    let mut out = Ans::new(finding);
    out.table(t);
    out.note("counters: rss = the process's resident size; app_heap = live bytes of the app domain's heap (app_heap_committed: its pages)");
    out.more("events cat=script.gc  events cat=zone.swap  summary");
    out
}
