//! The queries (`gpu prof frames | frame | top | util | diff`) answered on a `Recording`.
//! Answers are JSON lines plus findings sentences; the caller prints them.

use crate::{intersect_ns, partition, union, Recording};
use std::collections::HashMap;

/// A request's words and `k=v` arguments.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub pos: Vec<String>,
    pub kv: Vec<(String, String)>,
}

impl Args {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str())
    }
    pub fn num(&self, k: &str, d: u64) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
    /// `a b k=v` words.
    pub fn parse(words: &[String]) -> Args {
        let mut a = Args::default();
        for w in words {
            match w.split_once('=') {
                Some((k, v)) => a.kv.push((k.trim_start_matches("--").to_string(), v.to_string())),
                None => a.pos.push(w.trim_start_matches("--").to_string()),
            }
        }
        a
    }
}

pub struct Answer {
    pub measured: String,
    pub lines: Vec<String>,
    pub findings: Vec<String>,
}

impl Answer {
    fn new(measured: impl Into<String>) -> Answer {
        Answer { measured: measured.into(), lines: Vec::new(), findings: Vec::new() }
    }
}

pub fn json_str(s: &str) -> String {
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

pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("gpu prof start", "[s=60] [split=16] [hz=<display Hz>] [ablate=<shader> every=8]", "record every session's frames (ring of s seconds); every split-th frame cuts its passes per shader, for shader shares only (0: none; split encoders lose hidden-surface removal, so a shader hidden under opaque draws reads too high: confirm with ablate=)"),
    ("gpu prof stop", "[save=<path>] [sid=]", "stop; save= writes the busiest (or sid=) session's .mkgp"),
    ("gpu prof status", "", "recording state, sessions and frames held, what the device can sample"),
    ("gpu prof shader", "<name> [save=<path>]", "a shader's cost hints and (MAKEPAD_GPU_PROF=1 at launch) its generated source"),
    ("gpu prof frames", "[n=10]", "frame time stats, hitch zones with their class (gpu/cpu/present-bound) and the dominant shader or CPU phase"),
    ("gpu prof frame", "F<n>", "one frame: CPU phases, GPU passes, shaders drawn, nearest split-frame attribution"),
    ("gpu prof top", "by=shader|owner|list|pass|draw [frames=a-b] [n=15]", "GPU time (measured per pass; shared out to shaders / owners / draw lists by split-frame shares), draws, instances, triangles, bytes read"),
    ("gpu prof ablate", "", "the result of `gpu prof start ablate=<shader>`: GPU time per pass with that shader's draws left out vs neighbouring normal frames (the true cost, hidden-surface removal and bandwidth included)"),
    ("gpu prof runs", "[F<n>|last] [pass=<name>] [n=20]", "a split frame's runs of draws (one shader each), slowest first, with their draws' lists and owners: the per-draw drill-down"),
    ("gpu prof lanes", "F<n> [width=100]", "one frame's GPU timeline: vertex / fragment / compute / blit lanes per encoder, overlap and idle gaps"),
    ("gpu prof util", "[frames=a-b]", "GPU busy / idle / overlap with the CPU / bubbles, frames by class"),
    ("gpu prof diff", "<a.mkgp> <b.mkgp|live>", "frame stats and shaders / owners / passes that grew between two recordings"),
];

/// Answers `frames | frame | top | util | diff`. `load` resolves a recording name for `diff`
/// (a path, or `live`).
pub fn run(rec: &Recording, args: &Args, load: &dyn Fn(&str) -> Result<Recording, String>) -> Result<Answer, String> {
    let sub = args.pos.first().map(String::as_str).unwrap_or("");
    match sub {
        "frames" => Ok(frames(rec, args)),
        "frame" => frame(rec, args),
        "top" => top(rec, args),
        "util" => Ok(util(rec, args)),
        "lanes" => lanes(rec, args),
        "runs" => runs(rec, args),
        "ablate" => Ok(ablate(rec)),
        "diff" => {
            let (Some(a), Some(b)) = (args.pos.get(1), args.pos.get(2)) else { return Err("gpu prof diff <a.mkgp> <b.mkgp|live>".into()) };
            Ok(diff(&load(a)?, &load(b)?))
        }
        s => Err(format!("gpu prof {s}: frames | frame F<n> | lanes F<n> | top by=shader|owner|list|pass|draw | util | diff <a> <b>")),
    }
}

// ------------------------------------------------------------------ text rendering

/// One JSON-object answer line as (key path, value text) pairs; nested objects flatten to
/// `outer.inner`. `None` for a line that is not an object of scalars.
fn flat(line: &str) -> Option<Vec<(String, String)>> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    fn ws(b: &[u8], i: &mut usize) {
        while *i < b.len() && (b[*i] as char).is_whitespace() {
            *i += 1;
        }
    }
    fn string(b: &[u8], i: &mut usize) -> Option<String> {
        if b.get(*i) != Some(&b'"') {
            return None;
        }
        *i += 1;
        let mut o = Vec::new();
        while *i < b.len() {
            match b[*i] {
                b'"' => {
                    *i += 1;
                    return String::from_utf8(o).ok();
                }
                b'\\' => {
                    *i += 1;
                    match b.get(*i)? {
                        b'n' => o.push(b' '),
                        b't' => o.push(b' '),
                        c => o.push(*c),
                    }
                }
                c => o.push(c),
            }
            *i += 1;
        }
        None
    }
    fn object(b: &[u8], i: &mut usize, prefix: &str, out: &mut Vec<(String, String)>) -> Option<()> {
        ws(b, i);
        if b.get(*i) != Some(&b'{') {
            return None;
        }
        *i += 1;
        loop {
            ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Some(());
            }
            let k = string(b, i)?;
            ws(b, i);
            if b.get(*i) != Some(&b':') {
                return None;
            }
            *i += 1;
            ws(b, i);
            let key = if prefix.is_empty() { k } else { format!("{prefix}.{k}") };
            match b.get(*i)? {
                b'{' => object(b, i, &key, out)?,
                b'"' => {
                    let v = string(b, i)?;
                    out.push((key, v));
                }
                _ => {
                    let st = *i;
                    while *i < b.len() && !matches!(b[*i], b',' | b'}') {
                        *i += 1;
                    }
                    out.push((key, String::from_utf8_lossy(&b[st..*i]).trim().to_string()));
                }
            }
            ws(b, i);
            if b.get(*i) == Some(&b',') {
                *i += 1;
            }
        }
    }
    object(b, &mut i, "", &mut out)?;
    Some(out)
}

/// An answer as markdown: runs of lines with the same keys become one table (header once, then
/// rows), other lines stay as they are, findings become a list.
pub fn markdown(a: &Answer) -> Vec<String> {
    let mut out = vec![format!("## {}", a.measured)];
    let mut i = 0;
    while i < a.lines.len() {
        let Some(row) = flat(&a.lines[i]) else {
            out.push(a.lines[i].clone());
            i += 1;
            continue;
        };
        let keys: Vec<String> = row.iter().map(|(k, _)| k.clone()).collect();
        if keys.len() == 1 && keys[0] == "text" {
            // pre-rendered text (a lanes chart): a code block
            out.push("```".into());
            while let Some(r) = a.lines.get(i).and_then(|l| flat(l)).filter(|r| r.len() == 1 && r[0].0 == "text") {
                out.push(r[0].1.clone());
                i += 1;
            }
            out.push("```".into());
            continue;
        }
        out.push(format!("| {} |", keys.join(" | ")));
        out.push(format!("|{}", "---|".repeat(keys.len())));
        while let Some(r) = a.lines.get(i).and_then(|l| flat(l)).filter(|r| r.iter().map(|(k, _)| k).eq(keys.iter())) {
            out.push(format!("| {} |", r.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(" | ")));
            i += 1;
        }
    }
    if !a.findings.is_empty() {
        out.push(String::new());
        for f in &a.findings {
            out.push(format!("- {f}"));
        }
    }
    out
}

// ------------------------------------------------------------------ analysis

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Class {
    Gpu,
    Cpu,
    Present,
    Ok,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Gpu => "gpu-bound",
            Class::Cpu => "cpu-bound",
            Class::Present => "present-bound",
            Class::Ok => "ok",
        }
    }
}

struct Stat {
    /// index into rec.frames
    idx: usize,
    interval: u64,
    busy: u64,
    /// owned ns per pass (partition of the union)
    owned: Vec<u64>,
    cpu_busy: u64,
    gpu_wait: u64,
    bubbles: u64,
    bubble_count: u32,
    class: Class,
    hitch: bool,
}

struct Analysis {
    stats: Vec<Stat>,
    median: u64,
    p95: u64,
    max: u64,
    budget: u64,
    /// (pass name, width, height) -> (shader -> share of the pass's split-frame time)
    shares: HashMap<(u32, u32, u32), HashMap<u32, f64>>,
    /// the same by pass name alone, for a pass size not split yet
    shares_by_name: HashMap<u32, HashMap<u32, f64>>,
    /// some frame has CPU phases
    has_cpu: bool,
    split_frames: usize,
}

const BUBBLE_NS: u64 = 500_000;

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn f3(v: f64) -> String {
    format!("{v:.3}")
}

/// Three significant digits (no exponent): 0.0123, 1.23, 123, 12300.
pub fn g3(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let digits = (2 - v.abs().log10().floor() as i32).max(0) as usize;
    let s = format!("{v:.digits$}");
    if digits >= 3 && s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn analyze(rec: &Recording) -> Analysis {
    // 0: the display refresh is unknown, so a hitch is judged against the median alone
    let budget = if rec.header.refresh_hz > 1.0 { (1e9 / rec.header.refresh_hz as f64) as u64 } else { 0 };
    let mut stats = Vec::with_capacity(rec.frames.len());
    let (mut prev_present, mut floor) = (0u64, 0u64);
    for (idx, f) in rec.frames.iter().enumerate() {
        let interval = if f.present != 0 && prev_present != 0 { f.present.saturating_sub(prev_present) } else { 0 };
        if f.present != 0 {
            prev_present = f.present;
        }
        let ok: Vec<usize> = (0..f.passes.len()).filter(|i| f.passes[*i].gpu_ok()).collect();
        let spans: Vec<(u64, u64)> = ok.iter().map(|i| (f.passes[*i].start, f.passes[*i].end)).collect();
        let (own, busy) = partition(&spans, floor);
        let mut owned = vec![0u64; f.passes.len()];
        for (k, i) in ok.iter().enumerate() {
            owned[*i] = own[k];
        }
        // bubbles: gaps inside the frame's GPU span
        let un = union(spans.iter().map(|s| (s.0.max(floor).min(s.1), s.1)).collect());
        let (mut bubbles, mut bubble_count) = (0u64, 0u32);
        for w in un.windows(2) {
            let gap = w[1].0 - w[0].1;
            if gap > BUBBLE_NS {
                bubbles += gap;
                bubble_count += 1;
            }
        }
        if let Some(l) = un.last() {
            floor = floor.max(l.1);
        }
        let gpu_wait_name = |n: u32| rec.s(n) == "gpu-wait";
        let cpu_busy = union(f.cpu.iter().filter(|c| !gpu_wait_name(c.name)).map(|c| (c.start, c.end)).collect()).iter().map(|s| s.1 - s.0).sum();
        let gpu_wait = union(f.cpu.iter().filter(|c| gpu_wait_name(c.name)).map(|c| (c.start, c.end)).collect()).iter().map(|s| s.1 - s.0).sum();
        stats.push(Stat { idx, interval, busy, owned, cpu_busy, gpu_wait, bubbles, bubble_count, class: Class::Ok, hitch: false });
    }
    let mut ivs: Vec<u64> = stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).map(|s| s.interval).collect();
    ivs.sort();
    let median = percentile(&ivs, 0.5);
    let p95 = percentile(&ivs, 0.95);
    let max = ivs.last().copied().unwrap_or(0);
    for s in stats.iter_mut() {
        if rec.frames[s.idx].special() || s.interval == 0 {
            continue;
        }
        let iv = s.interval as f64;
        let (busy, cpu) = (s.busy as f64, s.cpu_busy as f64);
        s.class = if busy >= 0.85 * iv || (s.gpu_wait as f64 >= 0.3 * iv && busy >= 0.5 * iv) {
            Class::Gpu
        } else if cpu >= 0.85 * iv && busy < 0.7 * iv {
            Class::Cpu
        } else if busy < 0.5 * iv && cpu < 0.5 * iv && iv > 1.25 * (median.max(budget)) as f64 {
            Class::Present
        } else {
            Class::Ok
        };
        s.hitch = s.interval as f64 > 1.5 * median as f64 && s.interval > budget;
    }
    // a hitch is classed by what grew: the part of its excess over the median frame that the
    // CPU or the GPU took longer than in a median frame
    let (mut cpus, mut busys): (Vec<u64>, Vec<u64>) = stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).map(|s| (s.cpu_busy, s.busy)).unzip();
    cpus.sort();
    busys.sort();
    let (cpu_med, busy_med) = (percentile(&cpus, 0.5), percentile(&busys, 0.5));
    for s in stats.iter_mut().filter(|s| s.hitch) {
        let excess = s.interval.saturating_sub(median) as f64;
        let cpu_grew = s.cpu_busy.saturating_sub(cpu_med) as f64;
        let gpu_grew = s.busy.saturating_sub(busy_med) as f64;
        s.class = if cpu_grew >= 0.5 * excess && cpu_grew >= gpu_grew {
            Class::Cpu
        } else if gpu_grew >= 0.5 * excess {
            Class::Gpu
        } else {
            Class::Present
        };
    }
    // shader shares of each pass from the split frames
    let mut sums: HashMap<(u32, u32, u32), HashMap<u32, u64>> = HashMap::new();
    let mut sums_name: HashMap<u32, HashMap<u32, u64>> = HashMap::new();
    let mut split_frames = 0;
    for f in rec.frames.iter().filter(|f| f.split) {
        split_frames += 1;
        for g in &f.groups {
            let Some(p) = f.passes.get(g.pass as usize) else { continue };
            let ns = g.ns.saturating_sub(p.baseline);
            *sums.entry((p.name, p.w, p.h)).or_default().entry(g.shader).or_default() += ns;
            *sums_name.entry(p.name).or_default().entry(g.shader).or_default() += ns;
        }
    }
    fn normalise<K: std::hash::Hash + Eq>(sums: HashMap<K, HashMap<u32, u64>>) -> HashMap<K, HashMap<u32, f64>> {
        sums.into_iter()
            .filter_map(|(pass, m)| {
                let total: u64 = m.values().sum();
                (total > 0).then(|| (pass, m.into_iter().map(|(s, v)| (s, v as f64 / total as f64)).collect()))
            })
            .collect()
    }
    let has_cpu = rec.frames.iter().any(|f| !f.cpu.is_empty());
    Analysis { stats, median, p95, max, budget, shares: normalise(sums), shares_by_name: normalise(sums_name), has_cpu, split_frames }
}

/// Exclusive ns per phase name of one frame's nested CPU spans.
fn exclusive(rec: &Recording, f: &crate::Frame) -> Vec<(String, u64)> {
    fn close(stack: &mut Vec<(usize, u64)>, out: &mut HashMap<u32, u64>, v: &[&crate::CpuSpan]) {
        let (i, kids) = stack.pop().unwrap();
        let dur = v[i].end - v[i].start;
        *out.entry(v[i].name).or_default() += dur.saturating_sub(kids);
        if let Some(p) = stack.last_mut() {
            p.1 += dur;
        }
    }
    let mut v: Vec<&crate::CpuSpan> = f.cpu.iter().collect();
    v.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut stack: Vec<(usize, u64)> = Vec::new(); // (index in v, children ns)
    let mut out: HashMap<u32, u64> = HashMap::new();
    for i in 0..v.len() {
        while stack.last().is_some_and(|(t, _)| v[*t].end <= v[i].start) {
            close(&mut stack, &mut out, &v);
        }
        stack.push((i, 0));
    }
    while !stack.is_empty() {
        close(&mut stack, &mut out, &v);
    }
    let mut r: Vec<(String, u64)> = out.into_iter().map(|(k, v)| (rec.s(k).to_string(), v)).collect();
    r.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    r
}

/// How one frame's GPU time divides among shaders: (shader, ms ns, method, pass index).
struct Attr {
    pass: usize,
    shader: u32,
    ns: f64,
    method: &'static str,
}

/// Name for the time of a pass that has no split data to divide it.
const UNSPLIT: u32 = u32::MAX;

fn attribute(rec: &Recording, an: &Analysis, st: &Stat) -> Vec<Attr> {
    let f = &rec.frames[st.idx];
    let mut out = Vec::new();
    for (pi, p) in f.passes.iter().enumerate() {
        let t = st.owned[pi] as f64;
        if t <= 0.0 {
            continue;
        }
        let mut present: Vec<u32> = Vec::new();
        for d in f.draws.iter().filter(|d| d.pass as usize == pi) {
            if !present.contains(&d.shader) {
                present.push(d.shader);
            }
        }
        if present.len() == 1 {
            out.push(Attr { pass: pi, shader: present[0], ns: t, method: "direct" });
            continue;
        }
        let Some(sh) = an.shares.get(&(p.name, p.w, p.h)).or_else(|| an.shares_by_name.get(&p.name)).filter(|_| !present.is_empty()) else {
            out.push(Attr { pass: pi, shader: UNSPLIT, ns: t, method: "none" });
            continue;
        };
        let known: f64 = present.iter().filter_map(|s| sh.get(s)).sum();
        let unknown = present.iter().filter(|s| !sh.contains_key(s)).count();
        let scale = if unknown == 0 && known > 0.0 { 1.0 / known } else { 1.0 };
        for s in &present {
            if let Some(w) = sh.get(s) {
                out.push(Attr { pass: pi, shader: *s, ns: t * w * scale, method: "split" });
            }
        }
        if unknown > 0 && known < 1.0 {
            out.push(Attr { pass: pi, shader: UNSPLIT, ns: t * (1.0 - known), method: "none" });
        }
    }
    out
}

fn range_of(args: &Args) -> Result<(u64, u64), String> {
    match args.get("frames") {
        None => Ok((0, u64::MAX)),
        Some(r) => {
            let (a, b) = r.split_once('-').unwrap_or((r, r));
            let p = |s: &str| s.trim_start_matches('F').parse::<u64>().map_err(|_| format!("frames={r}: want frames=a-b"));
            Ok((p(a)?, p(b)?))
        }
    }
}

fn in_range(rec: &Recording, st: &Stat, r: (u64, u64)) -> bool {
    let id = rec.frames[st.idx].id;
    id >= r.0 && id <= r.1
}

#[derive(Default)]
struct Agg {
    ns: f64,
    draws: u64,
    instances: u64,
    verts: u64,
    tris: u64,
    geo_bytes: u64,
    tex_bytes: u64,
    methods: Vec<&'static str>,
}

impl Agg {
    fn method(&mut self, m: &'static str) {
        if !self.methods.contains(&m) {
            self.methods.push(m);
        }
    }
    fn method_str(&self) -> String {
        if self.methods.is_empty() {
            return "counts".into();
        }
        self.methods.join("+")
    }
}

/// Aggregates over the normal (non-split) frames in `r`: by key -> Agg.
fn aggregate(rec: &Recording, an: &Analysis, r: (u64, u64), by: &str) -> (Vec<(String, Agg)>, usize, f64) {
    let mut m: HashMap<String, Agg> = HashMap::new();
    let (mut frames, mut total) = (0usize, 0.0f64);
    for st in an.stats.iter().filter(|s| in_range(rec, s, r)) {
        let f = &rec.frames[st.idx];
        // split frames only lend their shader shares: no time and no counts from them
        if f.special() {
            continue;
        }
        let counted = true;
        frames += 1;
        let name_shader = |s: u32| if s == UNSPLIT { "(unsplit passes)".to_string() } else { rec.s(s).to_string() };
        for d in &f.draws {
            let key = match by {
                "shader" => rec.s(d.shader).to_string(),
                "owner" => rec.s(d.owner).to_string(),
                "pass" => f.passes.get(d.pass as usize).map_or("?", |p| rec.s(p.name)).to_string(),
                "list" => rec.s(d.list).to_string(),
                _ => format!("{} / {} / {}", f.passes.get(d.pass as usize).map_or("?", |p| rec.s(p.name)), rec.s(d.shader), rec.s(d.owner)),
            };
            let a = m.entry(key).or_default();
            a.draws += 1;
            a.instances += d.instances as u64;
            a.verts += d.instances as u64 * d.indices as u64;
            a.tris += d.instances as u64 * (d.indices / 3) as u64;
            a.geo_bytes += d.geo_bytes;
            a.tex_bytes += d.tex_bytes;
        }
        if !counted {
            continue;
        }
        total += st.busy as f64;
        if by == "pass" {
            for (pi, p) in f.passes.iter().enumerate() {
                if st.owned[pi] > 0 {
                    let a = m.entry(rec.s(p.name).to_string()).or_default();
                    a.ns += st.owned[pi] as f64;
                    a.method("measured");
                }
            }
            continue;
        }
        for at in attribute(rec, an, st) {
            if by == "shader" {
                let a = m.entry(name_shader(at.shader)).or_default();
                a.ns += at.ns;
                a.method(at.method);
                continue;
            }
            // owners: the shader's time divided by each owner's instances of it in this pass
            let ds: Vec<&crate::Draw> = f.draws.iter().filter(|d| d.pass as usize == at.pass && (at.shader == UNSPLIT || d.shader == at.shader)).collect();
            let weight = |d: &crate::Draw| d.instances.max(1) as f64;
            let sum: f64 = ds.iter().map(|d| weight(d)).sum();
            if ds.is_empty() || sum <= 0.0 {
                continue;
            }
            for d in ds {
                let key = if by == "owner" {
                    rec.s(d.owner).to_string()
                } else if by == "list" {
                    rec.s(d.list).to_string()
                } else {
                    format!("{} / {} / {}", f.passes.get(d.pass as usize).map_or("?", |p| rec.s(p.name)), rec.s(d.shader), rec.s(d.owner))
                };
                let a = m.entry(key).or_default();
                a.ns += at.ns * weight(d) / sum;
                a.method(at.method);
            }
        }
    }
    let mut v: Vec<(String, Agg)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.ns.total_cmp(&a.1.ns).then(b.1.instances.cmp(&a.1.instances)).then(a.0.cmp(&b.0)));
    (v, frames, total)
}

// ------------------------------------------------------------------ frames

fn frames(rec: &Recording, args: &Args) -> Answer {
    let n = args.num("n", 10) as usize;
    let an = analyze(rec);
    let mut a = Answer::new("GPU frame profile");
    let normal: Vec<&Stat> = an.stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).collect();
    let mut busy: Vec<u64> = normal.iter().map(|s| s.busy).collect();
    let mut cpu: Vec<u64> = normal.iter().map(|s| s.cpu_busy).collect();
    busy.sort();
    cpu.sort();
    let hitches: Vec<&&Stat> = normal.iter().filter(|s| s.hitch).collect();
    let mut classes: HashMap<Class, usize> = HashMap::new();
    for s in &normal {
        *classes.entry(s.class).or_default() += 1;
    }
    let cl = |c: Class| classes.get(&c).copied().unwrap_or(0);
    a.lines.push(format!(
        "{{\"frames\":{},\"split_frames\":{},\"unpresented_frames\":{},\"interval_ms\":{{\"median\":{},\"p95\":{},\"max\":{}}},\"fps_median\":{:.1},\"gpu_busy_ms\":{{\"median\":{},\"p95\":{}}},\"cpu_busy_ms\":{{\"median\":{},\"p95\":{}}},\"hitches\":{},\"budget_ms\":{},\"class\":{{\"gpu-bound\":{},\"cpu-bound\":{},\"present-bound\":{},\"ok\":{}}}}}",
        normal.len(),
        an.split_frames,
        rec.frames.iter().filter(|f| f.present == 0).count(),
        f3(ms(an.median)),
        f3(ms(an.p95)),
        f3(ms(an.max)),
        if an.median > 0 { 1e9 / an.median as f64 } else { 0.0 },
        f3(ms(percentile(&busy, 0.5))),
        f3(ms(percentile(&busy, 0.95))),
        f3(ms(percentile(&cpu, 0.5))),
        f3(ms(percentile(&cpu, 0.95))),
        hitches.len(),
        f3(ms(an.budget)),
        cl(Class::Gpu),
        cl(Class::Cpu),
        cl(Class::Present),
        cl(Class::Ok)
    ));
    // hitch zones: hitch frames within 20 frames of each other
    let mut zones: Vec<Vec<&Stat>> = Vec::new();
    for s in hitches.iter().map(|s| **s) {
        let id = rec.frames[s.idx].id;
        match zones.last_mut() {
            Some(z) if id - rec.frames[z.last().unwrap().idx].id <= 20 => z.push(s),
            _ => zones.push(vec![s]),
        }
    }
    if normal.is_empty() {
        a.findings.push("no presented frames in the recording".into());
    } else {
        if !an.has_cpu {
            a.findings.push("no CPU phases were recorded: frames are classified from GPU time alone".into());
        }
        a.findings.push(format!(
            "{} frames, median {:.1} ms ({:.0} fps), p95 {:.1} ms, max {:.1} ms; GPU busy {:.1} ms median, CPU busy {:.1} ms median",
            normal.len(),
            ms(an.median),
            1e9 / an.median.max(1) as f64,
            ms(an.p95),
            ms(an.max),
            ms(percentile(&busy, 0.5)),
            ms(percentile(&cpu, 0.5))
        ));
    }
    for z in zones.iter().take(n) {
        let (first, last) = (rec.frames[z[0].idx].id, rec.frames[z.last().unwrap().idx].id);
        let mut votes: HashMap<Class, usize> = HashMap::new();
        for s in z {
            *votes.entry(s.class).or_default() += 1;
        }
        let class = votes.iter().max_by_key(|(c, v)| (**v, c.name())).map(|(c, _)| *c).unwrap_or(Class::Ok);
        let worst = z.iter().map(|s| s.interval).max().unwrap_or(0);
        let why = zone_reason(rec, &an, z, class);
        a.lines.push(format!(
            "{{\"zone\":{{\"first\":{first},\"last\":{last},\"hitches\":{},\"class\":{},\"max_ms\":{},\"why\":{}}}}}",
            z.len(),
            json_str(class.name()),
            f3(ms(worst)),
            json_str(&why)
        ));
        let range = if first == last { format!("frame {first}") } else { format!("frames {first}-{last}") };
        a.findings.push(format!(
            "{range}: {} hitch{} (worst {:.1} ms vs {:.1} median), {}{}",
            z.len(),
            if z.len() == 1 { "" } else { "es" },
            ms(worst),
            ms(an.median),
            match class {
                Class::Gpu => "GPU-bound",
                Class::Cpu => "CPU-bound",
                Class::Present => "present-bound (neither CPU nor GPU work grew: a wait on the display, the compositor or another process)",
                Class::Ok => "not clearly bound",
            },
            if why.is_empty() { String::new() } else { format!(", {why}") }
        ));
    }
    if zones.len() > n {
        a.findings.push(format!("{} more hitch zones (n={})", zones.len() - n, zones.len()));
    }
    if hitches.is_empty() && !normal.is_empty() {
        a.findings.push("no hitches: every frame interval stayed under 1.5x the median or the display budget".into());
    }
    // the slowest frames
    let mut slow: Vec<&&Stat> = normal.iter().collect();
    slow.sort_by(|x, y| y.interval.cmp(&x.interval));
    for s in slow.into_iter().take(n.min(5)) {
        a.lines.push(format!(
            "{{\"slow_frame\":{},\"interval_ms\":{},\"gpu_busy_ms\":{},\"cpu_busy_ms\":{},\"class\":{}}}",
            rec.frames[s.idx].id,
            f3(ms(s.interval)),
            f3(ms(s.busy)),
            f3(ms(s.cpu_busy)),
            json_str(s.class.name())
        ));
    }
    if an.split_frames > 0 {
        // what cutting the passes cost: each split frame's GPU busy against the median of the
        // normal frames around it (the split frames' times are only used as shares)
        let mut ratios: Vec<f64> = Vec::new();
        for (i, st) in an.stats.iter().enumerate().filter(|(_, s)| rec.frames[s.idx].split && s.busy > 0) {
            let mut near: Vec<u64> = an.stats[i.saturating_sub(8)..(i + 9).min(an.stats.len())].iter().filter(|s| !rec.frames[s.idx].special() && s.busy > 0).map(|s| s.busy).collect();
            near.sort();
            if !near.is_empty() {
                ratios.push(st.busy as f64 / percentile(&near, 0.5).max(1) as f64);
            }
        }
        ratios.sort_by(|x, y| x.total_cmp(y));
        let mid = ratios.get(ratios.len() / 2).copied();
        a.lines.push(format!("{{\"split_frames\":{},\"split_gpu_busy_vs_neighbours\":{}}}", an.split_frames, mid.map_or("null".into(), |m| format!("{m:.2}"))));
        a.findings.push(format!(
            "{} split frames are left out of these statistics; they only give each shader's share of its pass{}",
            an.split_frames,
            mid.map_or(String::new(), |m| format!(" (cutting the passes cost {:+.0}% GPU time against neighbouring frames)", (m - 1.0) * 100.0))
        ));
    }
    // on glass: the display's own times, when the backend gets them
    let mut shown: Vec<u64> = Vec::new();
    let mut prev = 0u64;
    for f in rec.frames.iter().filter(|f| f.shown != 0) {
        if prev != 0 && f.shown > prev {
            shown.push(f.shown - prev);
        }
        prev = f.shown;
    }
    if !shown.is_empty() {
        let mut sorted = shown.clone();
        sorted.sort();
        let vsync = if rec.header.refresh_hz > 1.0 { (1e9 / rec.header.refresh_hz as f64) as u64 } else { sorted[0].max(1) };
        let missed = shown.iter().filter(|v| **v as f64 > 1.5 * vsync as f64).count();
        a.lines.push(format!(
            "{{\"on_glass_interval_ms\":{{\"median\":{},\"p95\":{},\"max\":{}}},\"vsync_ms\":{},\"missed_vsyncs\":{}}}",
            f3(ms(percentile(&sorted, 0.5))),
            f3(ms(percentile(&sorted, 0.95))),
            f3(ms(*sorted.last().unwrap())),
            f3(ms(vsync)),
            missed
        ));
        a.findings.push(format!(
            "on glass: median {:.1} ms between frames shown ({:.0} fps), {} of {} intervals missed a vsync of {:.2} ms",
            ms(percentile(&sorted, 0.5)),
            1e9 / percentile(&sorted, 0.5).max(1) as f64,
            missed,
            shown.len(),
            ms(vsync)
        ));
    }
    for note in &rec.header.notes {
        a.findings.push(format!("note: {note}"));
    }
    a
}

/// The dominant shader or CPU phase of a hitch zone, as a clause.
fn zone_reason(rec: &Recording, an: &Analysis, z: &[&Stat], class: Class) -> String {
    if class == Class::Cpu {
        let mut by: HashMap<String, u64> = HashMap::new();
        for s in z {
            for (n, ns) in exclusive(rec, &rec.frames[s.idx]) {
                if n != "gpu-wait" {
                    *by.entry(n).or_default() += ns;
                }
            }
        }
        let total: u64 = by.values().sum();
        return by
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            .filter(|_| total > 0)
            .map(|(n, ns)| format!("{:.0}% of the CPU time in phase {n}", 100.0 * ns as f64 / total as f64))
            .unwrap_or_default();
    }
    if class != Class::Gpu {
        return String::new();
    }
    let mut by: HashMap<u32, f64> = HashMap::new();
    let mut total = 0.0;
    for s in z {
        for at in attribute(rec, an, s) {
            *by.entry(at.shader).or_default() += at.ns;
            total += at.ns;
        }
    }
    let Some((&shader, &ns)) = by.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))) else { return String::new() };
    if total <= 0.0 {
        return String::new();
    }
    let pct = 100.0 * ns / total;
    if shader == UNSPLIT {
        return format!("{pct:.0}% in passes without split-frame data");
    }
    let mut s = format!("{pct:.0}% in shader {}", rec.s(shader));
    if let Some(shape) = quad_shape(rec, z, shader) {
        s.push_str(&format!(" ({shape})"));
    }
    s
}

/// "single quad in a WxH pass" when the shader's draws in these frames are one instance of <= 6
/// indices each (a full-target pass quad is how feedback and post effects draw).
fn quad_shape(rec: &Recording, z: &[&Stat], shader: u32) -> Option<String> {
    let mut size = (0u32, 0u32);
    for s in z {
        let f = &rec.frames[s.idx];
        for d in f.draws.iter().filter(|d| d.shader == shader) {
            if d.instances != 1 || d.indices > 6 {
                return None;
            }
            if let Some(p) = f.passes.get(d.pass as usize) {
                if (p.w as u64 * p.h as u64) > (size.0 as u64 * size.1 as u64) {
                    size = (p.w, p.h);
                }
            }
        }
    }
    (size.0 > 0).then(|| format!("single quad draw in a {}x{} pass", size.0, size.1))
}

// ------------------------------------------------------------------ frame

/// `F<n>` / `<n>`, or a normal frame by rank: `last`, `median` (by interval), `slowest`.
fn pick_frame(rec: &Recording, want: &str) -> Option<u64> {
    if let Ok(id) = want.trim_start_matches('F').parse() {
        return Some(id);
    }
    let an = analyze(rec);
    let mut normal: Vec<&Stat> = an.stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).collect();
    let st = match want {
        "last" => normal.last().copied(),
        "slowest" => normal.iter().max_by_key(|s| s.interval).copied(),
        "median" => {
            normal.sort_by_key(|s| s.interval);
            normal.get(normal.len() / 2).copied()
        }
        _ => None,
    }?;
    Some(rec.frames[st.idx].id)
}

fn frame(rec: &Recording, args: &Args) -> Result<Answer, String> {
    let want = args.pos.get(1).ok_or("gpu prof frame F<n>")?;
    let id = pick_frame(rec, want).ok_or_else(|| format!("gpu prof frame {want}: want F<n>, last, median or slowest"))?;
    let an = analyze(rec);
    let Some(st) = an.stats.iter().find(|s| rec.frames[s.idx].id == id) else {
        let (lo, hi) = (rec.frames.first().map_or(0, |f| f.id), rec.frames.last().map_or(0, |f| f.id));
        return Err(format!("no frame {id} in the recording (it holds {lo}..{hi})"));
    };
    let f = &rec.frames[st.idx];
    let mut a = Answer::new(format!("frame {id}"));
    a.lines.push(format!(
        "{{\"frame\":{id},\"split\":{},\"presented\":{},\"interval_ms\":{},\"gpu_busy_ms\":{},\"cpu_busy_ms\":{},\"gpu_wait_ms\":{},\"bubbles\":{},\"bubble_ms\":{},\"class\":{}}}",
        f.split,
        f.present != 0,
        f3(ms(st.interval)),
        f3(ms(st.busy)),
        f3(ms(st.cpu_busy)),
        f3(ms(st.gpu_wait)),
        st.bubble_count,
        f3(ms(st.bubbles)),
        json_str(st.class.name())
    ));
    for (n, ns) in exclusive(rec, f).into_iter().take(8) {
        a.lines.push(format!("{{\"cpu_phase\":{},\"exclusive_ms\":{}}}", json_str(&n), f3(ms(ns))));
    }
    for (pi, p) in f.passes.iter().enumerate() {
        let draws = f.draws.iter().filter(|d| d.pass as usize == pi).count();
        let kind = match p.kind {
            crate::kind::COMPUTE => "compute",
            crate::kind::BLIT => "blit",
            _ => "render",
        };
        a.lines.push(format!(
            "{{\"pass\":{},\"kind\":\"{kind}\",\"size\":\"{}x{}\",\"gpu_ms\":{},\"vertex_ms\":{},\"fragment_ms\":{},\"draws\":{draws},\"attach_mb\":{},\"attachments\":{}}}",
            json_str(rec.s(p.name)),
            p.w,
            p.h,
            g3(ms(st.owned[pi])),
            g3(ms(p.vs.1.saturating_sub(p.vs.0))),
            g3(ms(p.fs.1.saturating_sub(p.fs.0))),
            g3(p.bytes as f64 / 1e6),
            json_str(rec.s(p.attach))
        ));
    }
    let mut by: HashMap<u32, (u64, u64)> = HashMap::new();
    for d in &f.draws {
        let e = by.entry(d.shader).or_default();
        e.0 += 1;
        e.1 += d.instances as u64;
    }
    let attr = attribute(rec, &an, st);
    let mut shaders: Vec<(u32, (u64, u64))> = by.into_iter().collect();
    shaders.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then(a.0.cmp(&b.0)));
    for (s, (n, inst)) in shaders.iter().take(12) {
        let est: f64 = attr.iter().filter(|x| x.shader == *s).fold(0.0, |a, x| a + x.ns);
        a.lines.push(format!("{{\"shader\":{},\"draws\":{n},\"instances\":{inst},\"ms_est\":{}}}", json_str(rec.s(*s)), f3(est / 1e6)));
    }
    if !f.split {
        // the nearest split frame's measured groups
        let near = rec.frames.iter().filter(|g| g.split && !g.groups.is_empty()).min_by_key(|g| g.id.abs_diff(id));
        if let Some(g) = near {
            let mut tot: HashMap<u32, u64> = HashMap::new();
            for gr in &g.groups {
                let base = g.passes.get(gr.pass as usize).map_or(0, |p| p.baseline);
                *tot.entry(gr.shader).or_default() += gr.ns.saturating_sub(base);
            }
            let mut t: Vec<(u32, u64)> = tot.into_iter().collect();
            t.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (s, ns) in t.into_iter().take(5) {
                a.lines.push(format!("{{\"split_frame\":{},\"shader\":{},\"ms\":{}}}", g.id, json_str(rec.s(s)), f3(ms(ns))));
            }
        }
    }
    let top_pass = f.passes.iter().enumerate().max_by_key(|(i, _)| st.owned[*i]);
    let mut finding = format!("frame {id}: {:.1} ms interval, GPU busy {:.1} ms, CPU busy {:.1} ms, {}", ms(st.interval), ms(st.busy), ms(st.cpu_busy), st.class.name());
    if let Some((i, p)) = top_pass.filter(|(i, _)| st.owned[*i] > 0) {
        finding.push_str(&format!("; pass {} took {:.1} ms", rec.s(p.name), ms(st.owned[i])));
    }
    if let Some(top) = attr.iter().max_by(|a, b| a.ns.total_cmp(&b.ns)).filter(|a| a.shader != UNSPLIT && st.busy > 0) {
        finding.push_str(&format!(", {:.0}% of the frame's GPU time in shader {}", 100.0 * top.ns / st.busy as f64, rec.s(top.shader)));
    }
    a.findings.push(finding);
    if f.split {
        a.findings.push("this is a split frame: its encoders were cut per shader group, so its times are not a normal frame's".into());
    }
    Ok(a)
}

// ------------------------------------------------------------------ top

fn top(rec: &Recording, args: &Args) -> Result<Answer, String> {
    let by = match args.get("by").unwrap_or("shader") {
        // a model is drawn as its own draw list
        "model" => "list",
        b => b,
    };
    if !matches!(by, "shader" | "owner" | "list" | "pass" | "draw") {
        return Err(format!("gpu prof top by={by}: shader | owner | list (model) | pass | draw"));
    }
    let n = args.num("n", 15) as usize;
    let r = range_of(args)?;
    let an = analyze(rec);
    let (rows, frames, total) = aggregate(rec, &an, r, by);
    let mut a = Answer::new(format!("GPU time by {by}, {frames} frames"));
    let total_ms = ms(total as u64);
    let per_frame = |ns: f64| if frames > 0 { ns / 1e6 / frames as f64 } else { 0.0 };
    // per frame, 3 significant digits: what one frame costs and carries
    let pf = |v: f64| if frames > 0 { v / frames as f64 } else { 0.0 };
    for (k, g) in rows.iter().take(n) {
        a.lines.push(format!(
            "{{\"{by}\":{},\"gpu_ms\":{},\"share_pct\":{},\"draws\":{},\"instances\":{},\"tris_k\":{},\"geo_mb\":{},\"tex_bound_mb\":{},\"method\":{}{}}}",
            json_str(k),
            g3(per_frame(g.ns)),
            g3(if total > 0.0 { 100.0 * g.ns / total } else { 0.0 }),
            g3(pf(g.draws as f64)),
            g3(pf(g.instances as f64)),
            g3(pf(g.tris as f64) / 1e3),
            g3(pf(g.geo_bytes as f64) / 1e6),
            g3(pf(g.tex_bytes as f64) / 1e6),
            json_str(&g.method_str()),
            if by == "shader" { shader_info(rec, k).map_or(String::new(), |i| format!(",\"cost\":{}", json_str(i))) } else { String::new() }
        ));
    }
    if rows.len() > n {
        let rest: f64 = rows[n..].iter().map(|(_, g)| g.ns).sum();
        a.lines.push(format!("{{\"{by}\":{},\"gpu_ms\":{},\"share_pct\":{},\"draws\":\"\",\"instances\":\"\",\"tris_k\":\"\",\"geo_mb\":\"\",\"tex_bound_mb\":\"\",\"method\":\"\"{}}}",
            json_str(&format!("({} more)", rows.len() - n)), g3(per_frame(rest)), g3(if total > 0.0 { 100.0 * rest / total } else { 0.0 }),
            if by == "shader" { ",\"cost\":\"\"" } else { "" }));
    }
    if frames == 0 {
        a.findings.push("no normal (non-split) frames in that range".into());
        return Ok(a);
    }
    a.findings.push(format!("{frames} frames, GPU busy {total_ms:.1} ms total ({:.2} ms/frame)", total_ms / frames as f64));
    if let Some((k, g)) = rows.first().filter(|(_, g)| g.ns > 0.0) {
        let mut s = format!("{by} {k}: {:.0}% of the GPU time ({:.2} ms/frame", 100.0 * g.ns / total.max(1.0), per_frame(g.ns));
        if g.draws > 0 {
            s.push_str(&format!(", {} draws, {} instances", g.draws, g.instances));
        }
        s.push_str(&format!(", {})", match g.method_str().as_str() {
            "measured" => "measured per pass".to_string(),
            m => format!("method {m}"),
        }));
        a.findings.push(s);
    }
    let unsplit = rows.iter().find(|(k, _)| k == "(unsplit passes)").map(|(_, g)| g.ns).unwrap_or(0.0);
    if by != "pass" && unsplit > 0.1 * total {
        // which passes they are
        let mut passes: HashMap<String, f64> = HashMap::new();
        for st in an.stats.iter().filter(|s| !rec.frames[s.idx].special() && in_range(rec, s, r)) {
            let f = &rec.frames[st.idx];
            for at in attribute(rec, &an, st).iter().filter(|x| x.shader == UNSPLIT) {
                *passes.entry(format!("{} {}x{}", rec.s(f.passes[at.pass].name), f.passes[at.pass].w, f.passes[at.pass].h)).or_default() += at.ns;
            }
        }
        let mut v: Vec<(String, f64)> = passes.into_iter().collect();
        v.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        let names: Vec<String> = v.iter().take(3).map(|(n, ns)| format!("{n} ({:.2} ms/frame)", per_frame(*ns))).collect();
        a.findings.push(format!(
            "{:.0}% of the GPU time is in passes with several shaders and no split frame to divide it, mostly {} ({} split frames recorded; record longer or lower split=)",
            100.0 * unsplit / total,
            names.join(", "),
            an.split_frames
        ));
    }
    Ok(a)
}

/// The compiler's cost hints of the shader named `name`.
fn shader_info<'a>(rec: &'a Recording, name: &str) -> Option<&'a str> {
    rec.header.shader_info.iter().find(|(id, _)| rec.s(*id) == name).map(|(_, i)| i.as_str())
}

// ------------------------------------------------------------------ ablation

fn ablate(rec: &Recording) -> Answer {
    let an = analyze(rec);
    let mut a = Answer::new("ablation: GPU ms per frame with a shader's draws left out, vs the normal frames around them");
    let mut shaders: Vec<u32> = rec.frames.iter().map(|f| f.ablated).filter(|s| *s != 0).collect();
    shaders.sort();
    shaders.dedup();
    if shaders.is_empty() {
        a.findings.push("no ablation frames: `gpu prof start ablate=<shader> [every=8]` (names from `gpu prof top by=shader`)".into());
        return a;
    }
    for sh in shaders {
        // per pass name: (ablated own times, normal own times near them)
        let mut per: Vec<(u32, Vec<u64>, Vec<u64>)> = Vec::new();
        let (mut busy_a, mut busy_n) = (Vec::new(), Vec::new());
        for (i, st) in an.stats.iter().enumerate().filter(|(_, s)| rec.frames[s.idx].ablated == sh) {
            busy_a.push(st.busy);
            let f = &rec.frames[st.idx];
            let near: Vec<&Stat> = an.stats[i.saturating_sub(6)..(i + 7).min(an.stats.len())].iter().filter(|s| !rec.frames[s.idx].special()).collect();
            busy_n.extend(near.iter().map(|s| s.busy));
            for (pi, p) in f.passes.iter().enumerate() {
                let e = match per.iter_mut().position(|x| x.0 == p.name) {
                    Some(k) => &mut per[k],
                    None => {
                        per.push((p.name, Vec::new(), Vec::new()));
                        per.last_mut().unwrap()
                    }
                };
                e.1.push(st.owned[pi]);
                for n in &near {
                    let g = &rec.frames[n.idx];
                    let t: u64 = g.passes.iter().enumerate().filter(|(_, q)| q.name == p.name).map(|(k, _)| n.owned[k]).sum();
                    e.2.push(t);
                }
            }
        }
        let med = |mut v: Vec<u64>| {
            v.sort();
            percentile(&v, 0.5)
        };
        let (ba, bn) = (med(busy_a.clone()), med(busy_n));
        a.lines.push(format!(
            "{{\"shader\":{},\"frames\":{},\"gpu_ms_normal\":{},\"gpu_ms_without\":{},\"saved_ms\":{}}}",
            json_str(rec.s(sh)),
            busy_a.len(),
            g3(ms(bn)),
            g3(ms(ba)),
            g3(ms(bn) - ms(ba))
        ));
        let mut rows: Vec<(u32, f64, f64)> = per.into_iter().map(|(n, x, y)| (n, ms(med(y)), ms(med(x)))).filter(|r| (r.1 - r.2).abs() > 0.005).collect();
        rows.sort_by(|x, y| (y.1 - y.2).total_cmp(&(x.1 - x.2)));
        for (n, normal, without) in rows.iter().take(6) {
            a.lines.push(format!("{{\"pass\":{},\"ms_normal\":{},\"ms_without\":{},\"saved_ms\":{}}}", json_str(rec.s(*n)), g3(*normal), g3(*without), g3(normal - without)));
        }
        a.findings.push(format!(
            "leaving out shader {} saves {:.2} ms of GPU time per frame ({:.1} -> {:.1} ms; medians of {} ablation frames vs their neighbours)",
            rec.s(sh),
            ms(bn) - ms(ba),
            ms(bn),
            ms(ba),
            busy_a.len()
        ));
    }
    a
}

// ------------------------------------------------------------------ runs

fn runs(rec: &Recording, args: &Args) -> Result<Answer, String> {
    let want = args.pos.get(1).map(String::as_str).unwrap_or("last");
    let f = if want == "last" {
        rec.frames.iter().rev().find(|f| f.split && !f.groups.is_empty())
    } else {
        let id: u64 = want.trim_start_matches('F').parse().map_err(|_| format!("gpu prof runs {want}: want F<n> or last"))?;
        rec.frames.iter().find(|f| f.id == id)
    }
    .ok_or("no split frame with runs in the recording (gpu prof start split=16)")?;
    let pass_filter = args.get("pass");
    let n = args.num("n", 20) as usize;
    let mut rows: Vec<(&crate::Group, u64)> = f
        .groups
        .iter()
        .filter(|g| pass_filter.map_or(true, |p| f.passes.get(g.pass as usize).is_some_and(|x| rec.s(x.name) == p)))
        .map(|g| (g, g.ns.saturating_sub(f.passes.get(g.pass as usize).map_or(0, |p| p.baseline))))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let total: u64 = rows.iter().map(|r| r.1).sum();
    let mut a = Answer::new(format!("runs of split frame {} ({} runs, times minus each pass's empty-encoder baseline; shares, not frame times)", f.id, rows.len()));
    for (g, ns) in rows.iter().take(n) {
        let ds = f.draws.get(g.first as usize..(g.first + g.count) as usize).unwrap_or(&[]);
        let inst: u64 = ds.iter().map(|d| d.instances as u64).sum();
        let tris: u64 = ds.iter().map(|d| d.instances as u64 * (d.indices / 3) as u64).sum();
        // the lists and owners of the run, most draws first
        let mut names: Vec<(String, usize)> = Vec::new();
        for d in ds {
            let k = match (rec.s(d.owner), rec.s(d.list)) {
                ("", l) => l.to_string(),
                (o, "") => o.to_string(),
                (o, l) => format!("{o}/{l}"),
            };
            match names.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += 1,
                None => names.push((k, 1)),
            }
        }
        names.sort_by(|x, y| y.1.cmp(&x.1));
        let mut who: Vec<String> = names.iter().take(3).map(|(k, c)| if *c > 1 { format!("{k} x{c}") } else { k.clone() }).collect();
        if names.len() > 3 {
            who.push(format!("+{} more", names.len() - 3));
        }
        let p = f.passes.get(g.pass as usize);
        let fl = ds.iter().fold(0u8, |a, d| a | d.flags);
        let mode = format!(
            "{}{}",
            if fl & crate::draw_flags::BLEND != 0 { "blend" } else { "opaque" },
            if fl & crate::draw_flags::DEPTH_WRITE != 0 { "+zwrite" } else { "" }
        );
        a.lines.push(format!(
            "{{\"pass\":{},\"shader\":{},\"ms\":{},\"share_pct\":{},\"draws\":{},\"instances\":{},\"tris_k\":{},\"mode\":\"{mode}\",\"lists\":{}}}",
            json_str(p.map_or("?", |p| rec.s(p.name))),
            json_str(rec.s(g.shader)),
            g3(ms(*ns)),
            g3(if total > 0 { 100.0 * *ns as f64 / total as f64 } else { 0.0 }),
            g.count,
            inst,
            g3(tris as f64 / 1e3),
            json_str(&who.join(", "))
        ));
    }
    if rows.len() > n {
        a.findings.push(format!("{} more runs (n=)", rows.len() - n));
    }
    Ok(a)
}

// ------------------------------------------------------------------ lanes

const LANES: [&str; 4] = ["vertex", "fragment", "compute", "blit"];

/// The stage spans of one encoder: (lane index, start, end).
fn stage_spans(p: &crate::Pass) -> Vec<(usize, u64, u64)> {
    let mut v = Vec::new();
    match p.kind {
        crate::kind::COMPUTE if p.gpu_ok() => v.push((2, p.start, p.end)),
        crate::kind::BLIT if p.gpu_ok() => v.push((3, p.start, p.end)),
        crate::kind::RENDER => {
            if p.vs.1 > p.vs.0 {
                v.push((0, p.vs.0, p.vs.1));
            }
            if p.fs.1 > p.fs.0 {
                v.push((1, p.fs.0, p.fs.1));
            }
            if v.is_empty() && p.gpu_ok() {
                // encoder samples only (no stage boundaries): the whole pass on the fragment lane
                v.push((1, p.start, p.end));
            }
        }
        _ => {}
    }
    v
}

fn lanes(rec: &Recording, args: &Args) -> Result<Answer, String> {
    let want = args.pos.get(1).ok_or("gpu prof lanes F<n>")?;
    let id = pick_frame(rec, want).ok_or_else(|| format!("gpu prof lanes {want}: want F<n>, last, median or slowest"))?;
    let f = rec.frames.iter().find(|f| f.id == id).ok_or_else(|| format!("no frame {id} in the recording"))?;
    let width = args.num("width", 100).clamp(20, 400) as usize;
    let spans: Vec<(usize, usize, u64, u64)> = f.passes.iter().enumerate().flat_map(|(i, p)| stage_spans(p).into_iter().map(move |(l, a, b)| (i, l, a, b))).collect();
    let mut a = Answer::new(format!("GPU lanes of frame {id}"));
    let (Some(t0), Some(t1)) = (spans.iter().map(|s| s.2).min(), spans.iter().map(|s| s.3).max()) else {
        a.findings.push("no GPU stage times in this frame".into());
        return Ok(a);
    };
    let total = (t1 - t0).max(1);
    let col = |t: u64| (((t - t0) as f64 / total as f64) * width as f64).floor().min(width as f64 - 1.0) as usize;
    for (i, l, s, e) in &spans {
        let p = &f.passes[*i];
        a.lines.push(format!(
            "{{\"pass\":{},\"index\":{i},\"lane\":\"{}\",\"start_ms\":{},\"end_ms\":{},\"ms\":{}}}",
            json_str(rec.s(p.name)),
            LANES[*l],
            f3(ms(s - t0)),
            f3(ms(e - t0)),
            f3(ms(e - s))
        ));
    }
    // one text row per encoder and stage: the span drawn over the frame's GPU time
    let mut text = Vec::new();
    text.push(format!("0 ms {:>w$} {:.2} ms", "", ms(total), w = width.saturating_sub(5)));
    for (i, l, s, e) in &spans {
        let mut row = vec![b'.'; width];
        for c in col(*s)..=col((*e).max(*s)) {
            row[c] = if *l == 0 { b'v' } else if *l == 1 { b'#' } else if *l == 2 { b'c' } else { b'b' };
        }
        text.push(format!("{} {:<8} {:>3} {}", String::from_utf8(row).unwrap_or_default(), LANES[*l], i, rec.s(f.passes[*i].name)));
    }
    for t in text {
        a.lines.push(format!("{{\"text\":{}}}", json_str(&t)));
    }
    // overlap: time two encoders' stages run at once; gaps: no stage running
    let mut edges: Vec<(u64, i32)> = spans.iter().flat_map(|s| [(s.2, 1), (s.3, -1)]).collect();
    edges.sort();
    let (mut depth, mut last, mut overlap, mut idle) = (0i32, t0, 0u64, 0u64);
    for (t, d) in edges {
        if depth >= 2 {
            overlap += t - last;
        } else if depth == 0 {
            idle += t - last;
        }
        depth += d;
        last = t;
    }
    a.findings.push(format!(
        "frame {id}: GPU from first to last stage {:.2} ms; {:.2} ms with two or more stages running at once, {:.2} ms with nothing running",
        ms(total),
        ms(overlap),
        ms(idle)
    ));
    Ok(a)
}

// ------------------------------------------------------------------ util

fn util(rec: &Recording, args: &Args) -> Answer {
    let (r, _) = match range_of(args) {
        Ok(r) => (r, ()),
        Err(e) => {
            let mut a = Answer::new("error");
            a.findings.push(e);
            return a;
        }
    };
    let an = analyze(rec);
    let sel: Vec<&Stat> = an.stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0 && in_range(rec, s, r)).collect();
    let mut a = Answer::new(format!("GPU utilisation, {} frames", sel.len()));
    if sel.is_empty() {
        a.findings.push("no normal frames in that range".into());
        return a;
    }
    let interval: u64 = sel.iter().map(|s| s.interval).sum();
    let busy: u64 = sel.iter().map(|s| s.busy.min(s.interval.max(s.busy))).sum();
    let cpu: u64 = sel.iter().map(|s| s.cpu_busy).sum();
    let bubbles: u64 = sel.iter().map(|s| s.bubbles).sum();
    let bubble_n: u32 = sel.iter().map(|s| s.bubble_count).sum();
    // GPU executing one frame while the CPU encodes the next
    let gpu_un = union(sel.iter().flat_map(|s| rec.frames[s.idx].passes.iter().filter(|p| p.gpu_ok()).map(|p| (p.start, p.end))).collect());
    let cpu_un = union(sel.iter().flat_map(|s| rec.frames[s.idx].cpu.iter().filter(|c| rec.s(c.name) != "gpu-wait").map(|c| (c.start, c.end))).collect());
    let overlap = intersect_ns(&gpu_un, &cpu_un);
    let idle = interval.saturating_sub(busy);
    let mut classes: HashMap<Class, usize> = HashMap::new();
    for s in &sel {
        *classes.entry(s.class).or_default() += 1;
    }
    let cl = |c: Class| classes.get(&c).copied().unwrap_or(0);
    a.lines.push(format!(
        "{{\"frames\":{},\"interval_ms\":{},\"gpu_busy_ms\":{},\"gpu_idle_ms\":{},\"gpu_busy_pct\":{:.1},\"cpu_busy_ms\":{},\"cpu_busy_pct\":{:.1},\"overlap_ms\":{},\"bubble_ms\":{},\"bubbles\":{},\"class\":{{\"gpu-bound\":{},\"cpu-bound\":{},\"present-bound\":{},\"ok\":{}}}}}",
        sel.len(),
        f3(ms(interval)),
        f3(ms(busy)),
        f3(ms(idle)),
        100.0 * busy as f64 / interval as f64,
        f3(ms(cpu)),
        100.0 * cpu as f64 / interval as f64,
        f3(ms(overlap)),
        f3(ms(bubbles)),
        bubble_n,
        cl(Class::Gpu),
        cl(Class::Cpu),
        cl(Class::Present),
        cl(Class::Ok)
    ));
    let pct = |v: u64| 100.0 * v as f64 / interval as f64;
    a.findings.push(format!(
        "GPU busy {:.0}% of the time, idle {:.0}%; CPU busy {:.0}%; GPU and CPU overlap {:.0}%",
        pct(busy),
        pct(idle),
        pct(cpu),
        pct(overlap)
    ));
    let dom = [Class::Gpu, Class::Cpu, Class::Present, Class::Ok].into_iter().max_by_key(|c| cl(*c)).unwrap();
    a.findings.push(format!(
        "{} of {} frames are {} (gpu {}, cpu {}, present {}, ok {})",
        cl(dom),
        sel.len(),
        dom.name(),
        cl(Class::Gpu),
        cl(Class::Cpu),
        cl(Class::Present),
        cl(Class::Ok)
    ));
    if bubble_n > 0 {
        a.findings.push(format!("{bubble_n} GPU idle gaps over 0.5 ms inside frames ({:.1} ms): the GPU waited for work being encoded or for a dependency", ms(bubbles)));
    }
    if busy as f64 > 0.85 * interval as f64 {
        a.findings.push("the GPU is the limit: busy more than 85% of every frame interval".into());
    } else if (cpu as f64) > 0.85 * interval as f64 {
        a.findings.push("the CPU is the limit: main-thread phases fill the frame interval".into());
    }
    a
}

// ------------------------------------------------------------------ diff

fn diff(a: &Recording, b: &Recording) -> Answer {
    let (aa, ab) = (analyze(a), analyze(b));
    let mut out = Answer::new("difference between two recordings");
    let summary = |an: &Analysis, rec: &Recording| {
        let n = an.stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).count();
        let mut busy: Vec<u64> = an.stats.iter().filter(|s| !rec.frames[s.idx].special() && s.interval > 0).map(|s| s.busy).collect();
        busy.sort();
        (n, an.median, an.p95, percentile(&busy, 0.5), an.stats.iter().filter(|s| s.hitch).count())
    };
    let (sa, sb) = (summary(&aa, a), summary(&ab, b));
    out.lines.push(format!(
        "{{\"a\":{{\"frames\":{},\"median_ms\":{},\"p95_ms\":{},\"gpu_busy_ms\":{},\"hitches\":{}}},\"b\":{{\"frames\":{},\"median_ms\":{},\"p95_ms\":{},\"gpu_busy_ms\":{},\"hitches\":{}}}}}",
        sa.0, f3(ms(sa.1)), f3(ms(sa.2)), f3(ms(sa.3)), sa.4, sb.0, f3(ms(sb.1)), f3(ms(sb.2)), f3(ms(sb.3)), sb.4
    ));
    out.findings.push(format!(
        "frame median {:.1} -> {:.1} ms, p95 {:.1} -> {:.1} ms, GPU busy median {:.2} -> {:.2} ms, hitches {} -> {}",
        ms(sa.1), ms(sb.1), ms(sa.2), ms(sb.2), ms(sa.3), ms(sb.3), sa.4, sb.4
    ));
    for by in ["shader", "owner", "pass"] {
        let (ra, fa, _) = aggregate(a, &aa, (0, u64::MAX), by);
        let (rb, fb, _) = aggregate(b, &ab, (0, u64::MAX), by);
        let pf = |ns: f64, f: usize| if f > 0 { ns / 1e6 / f as f64 } else { 0.0 };
        let ma: HashMap<&str, f64> = ra.iter().map(|(k, g)| (k.as_str(), pf(g.ns, fa))).collect();
        let mb: HashMap<&str, f64> = rb.iter().map(|(k, g)| (k.as_str(), pf(g.ns, fb))).collect();
        let mut keys: Vec<&str> = ma.keys().chain(mb.keys()).copied().collect();
        keys.sort();
        keys.dedup();
        let mut d: Vec<(&str, f64, f64)> = keys.into_iter().map(|k| (k, *ma.get(k).unwrap_or(&0.0), *mb.get(k).unwrap_or(&0.0))).filter(|x| (x.2 - x.1).abs() > 0.02).collect();
        d.sort_by(|x, y| (y.2 - y.1).abs().total_cmp(&(x.2 - x.1).abs()).then(x.0.cmp(y.0)));
        for (k, x, y) in d.iter().take(8) {
            out.lines.push(format!("{{\"{by}\":{},\"a_ms_per_frame\":{},\"b_ms_per_frame\":{},\"delta\":{}}}", json_str(k), f3(*x), f3(*y), f3(y - x)));
        }
        if let Some((k, x, y)) = d.iter().filter(|x| x.2 > x.1).max_by(|p, q| (p.2 - p.1).total_cmp(&(q.2 - q.1))) {
            out.findings.push(format!("{by} {k} grew from {x:.2} to {y:.2} ms/frame (+{:.2})", y - x));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    const MS: u64 = 1_000_000;

    /// A synthetic recording: `n` frames at 16.7 ms; frames 30..33 are slow because the pass
    /// "scene" takes 28 ms there, mostly in shader `feedback`; every 8th frame is a split frame.
    fn synth() -> Recording {
        let mut i = Interner::default();
        let (scene, window, feedback, ui, owner_a, owner_b, phase_draw, phase_wait) =
            (i.id("scene"), i.id("window"), i.id("feedback#4"), i.id("DrawQuad#1"), i.id("vj/fx"), i.id("ui/main"), i.id("draw-list"), i.id("gpu-wait"));
        let mut rec = Recording::default();
        rec.header.app = "synth".into();
        rec.header.refresh_hz = 60.0;
        let mut t = 1_000 * MS;
        for n in 0..60u64 {
            let slow = (30..33).contains(&n);
            let gpu_scene = if slow { 28 * MS } else { 3 * MS };
            let interval = if slow { 30 * MS } else { 16_700_000 };
            let split = n % 8 == 4;
            t += interval;
            let start = t - interval + 2 * MS;
            let mut f = Frame { id: 100 + n, present: t, split, ..Default::default() };
            f.cpu.push(CpuSpan { name: phase_draw, start: start - 2 * MS, end: start - MS });
            if slow {
                f.cpu.push(CpuSpan { name: phase_wait, start, end: start + 2 * MS });
            }
            f.passes.push(Pass { name: scene, w: 3840, h: 2160, start, end: start + gpu_scene, ..Default::default() });
            f.passes.push(Pass { name: window, w: 1920, h: 1080, start: start + gpu_scene, end: start + gpu_scene + MS, ..Default::default() });
            f.draws.push(Draw { pass: 0, shader: feedback, owner: owner_a, instances: 1, indices: 6, ..Default::default() });
            f.draws.push(Draw { pass: 0, shader: ui, owner: owner_a, instances: 40, indices: 6, ..Default::default() });
            f.draws.push(Draw { pass: 1, shader: ui, owner: owner_b, instances: 200, indices: 6, ..Default::default() });
            if split {
                f.passes[0].baseline = MS / 10;
                f.groups.push(Group { pass: 0, shader: feedback, ns: 9 * MS, first: 0, count: 1 });
                f.groups.push(Group { pass: 0, shader: ui, ns: MS, first: 1, count: 1 });
            }
            rec.frames.push(f);
        }
        rec.strings = i.strings;
        rec
    }

    fn run_q(rec: &Recording, words: &str) -> Answer {
        let w: Vec<String> = words.split_whitespace().map(String::from).collect();
        run(rec, &Args::parse(&w), &|_| Err("no".into())).unwrap()
    }

    #[test]
    fn roundtrip_is_exact() {
        let mut rec = synth();
        rec.header.counter_sets = vec!["timestamp".into()];
        rec.header.notes = vec!["stage boundary only".into()];
        let bytes = rec.encode();
        let back = Recording::decode(&bytes).unwrap();
        assert_eq!(rec, back);
        assert!(bytes.len() < rec.approx_bytes());
        assert!(Recording::decode(b"nope").is_err());
        // a file cut short reads as far as its last whole chunk
        let cut = Recording::decode(&bytes[..bytes.len() - 5]).unwrap();
        assert!(cut.header.notes.iter().any(|n| n.contains("cut short")));
        assert_eq!(cut.header.app, "synth");
        // a chunk of an unknown tag is skipped
        let mut extra = rec.encode_prelude();
        extra.extend([99u8, 3, 1, 2, 3]);
        extra.extend(rec.encode_header());
        extra.extend(rec.encode_strings(0));
        extra.extend(Recording::encode_frames(&rec.frames));
        assert_eq!(Recording::decode(&extra).unwrap().frames, rec.frames);
    }

    #[test]
    fn frames_finds_the_hitch_zone_and_its_shader() {
        let a = run_q(&synth(), "frames");
        let zone = a.findings.iter().find(|f| f.contains("hitch")).expect("a hitch zone");
        assert!(zone.starts_with("frames 130-132: 3 hitches"), "{zone}");
        assert!(zone.contains("GPU-bound"), "{zone}");
        assert!(zone.contains("in shader feedback#4"), "{zone}");
        assert!(zone.contains("single quad draw in a 3840x2160 pass"), "{zone}");
        assert!(a.findings.iter().any(|f| f.contains("split frames are left out")));
    }

    #[test]
    fn top_divides_a_pass_by_split_shares() {
        let a = run_q(&synth(), "top by=shader");
        // feedback holds 9/10 of scene's split time; scene is ~3 ms normally, 28 ms in the zone
        let first = &a.lines[0];
        assert!(first.contains("\"shader\":\"feedback#4\""), "{first}");
        assert!(first.contains("\"method\":\"split\""), "{first}");
        let a = run_q(&synth(), "top by=pass frames=100-110");
        assert!(a.lines[0].contains("\"pass\":\"scene\""));
        let a = run_q(&synth(), "top by=owner");
        assert!(a.lines.iter().any(|l| l.contains("vj/fx")));
        let a = run_q(&synth(), "top by=draw n=3");
        assert_eq!(a.lines.len(), 3);
    }

    #[test]
    fn frame_and_util_answer() {
        let rec = synth();
        let a = run_q(&rec, "frame F131");
        assert!(a.lines.iter().any(|l| l.contains("\"pass\":\"scene\"")));
        assert!(a.findings[0].contains("gpu-bound"), "{:?}", a.findings);
        assert!(run(&rec, &Args::parse(&["frame".into(), "F9999".into()]), &|_| Err("".into())).is_err());
        let u = run_q(&rec, "util");
        assert!(u.findings[0].contains("GPU busy"), "{:?}", u.findings);
    }

    #[test]
    fn markdown_tables_and_digits() {
        assert_eq!(g3(1234.6), "1235");
        assert_eq!(g3(12.345), "12.3");
        assert_eq!(g3(0.012345), "0.0123");
        assert_eq!(g3(0.0), "0");
        let md = markdown(&run_q(&synth(), "top by=pass"));
        assert!(md[1].starts_with("| pass | gpu_ms |"), "{md:?}");
        assert!(md[2].starts_with("|---|"));
        assert!(md.iter().any(|l| l.starts_with("| scene |")));
    }

    #[test]
    fn ablation_measures_the_left_out_shader() {
        let mut rec = synth();
        let feedback = rec.strings.iter().position(|s| s == "feedback#4").unwrap() as u32;
        // every 5th normal frame leaves feedback out: its scene pass shrinks by 2 ms
        for (i, f) in rec.frames.iter_mut().enumerate().filter(|(_, f)| !f.split) {
            if i % 5 == 0 && !(30..33).contains(&(f.id - 100)) {
                f.ablated = feedback;
                f.passes[0].end -= 2 * MS;
                f.passes[1].start -= 2 * MS;
                f.passes[1].end -= 2 * MS;
            }
        }
        let a = run_q(&rec, "ablate");
        assert!(a.lines[0].contains("\"shader\":\"feedback#4\""), "{:?}", a.lines);
        assert!(a.lines[0].contains("\"saved_ms\":\"2.00\"") || a.lines[0].contains("\"saved_ms\":2.00"), "{:?}", a.lines);
        // ablation frames are not frame-time samples
        let f = run_q(&rec, "top by=pass");
        assert!(f.findings[0].starts_with("4"), "{:?}", f.findings);
    }

    #[test]
    fn diff_names_what_grew() {
        let (a, mut b) = (synth(), synth());
        for f in b.frames.iter_mut().filter(|f| !f.split) {
            f.passes[0].end += 5 * MS;
            f.passes[1].start += 5 * MS;
            f.passes[1].end += 5 * MS;
        }
        let words: Vec<String> = ["diff", "a", "b"].iter().map(|s| s.to_string()).collect();
        let (ca, cb) = (a.clone(), b.clone());
        let d = run(&a, &Args::parse(&words), &move |n| Ok(if n == "a" { ca.clone() } else { cb.clone() })).unwrap();
        assert!(d.findings.iter().any(|f| f.contains("grew from")), "{:?}", d.findings);
    }
}
