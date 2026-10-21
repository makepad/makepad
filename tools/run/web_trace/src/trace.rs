//! The coverage file of a traced run (`makepad-web-trace/1`), and the merge of several runs.
//!
//! ```text
//! {
//!   "format": "makepad-web-trace/1",
//!   "package": "makepad-app-calculator", "target": "wasm32-unknown-unknown", "build": "<relax provenance>",
//!   "runs": [{"name", "script", "query", "stress", "window": [w, h, dpi], "frames", "virtual_ms", "panics": [..]}],
//!   "functions": [{"key", "kind", "crate", "file", "line", "col", "blocks", "hit": [[line, col], ..]}],
//!   "exports": [{"name", "calls"}],             // the program's wasm exports the page called
//!   "imports": [{"module": "env", "name", "calls"}],
//!   "from_wasm": [{"name", "count"}],           // messages the program sent the page (web.js handlers)
//!   "to_wasm": [{"name", "count"}],             // messages the page sent the program
//!   "assets": [{"url", "path", "size", "ranges": [[lo, hi), ..], "fetches"}],
//!   "glyphs": [{"url", "glyphs": [id, ..]}],    // per font file, the glyph ids the program outlined
//!   "shaders": {"compiled", "programs"},
//!   "pack_misses": ["<shader> (<where>, key <hex>)", ..],   // with --pack: shaders the pack lacked
//!   "splash": {                                 // the Splash the run used (platform splash_trace.rs)
//!     "modules_registered": [..], "modules_used": {module: by},
//!     "crates": [{"crate", "bytes", "used_bytes", "items", "used_items"}],   // used: kept (used or not an object)
//!     "items": [{"id", "module", "crate", "path", "file", "line", "column", "block_hash", "start", "end",
//!                "bytes", "used": true|false|null}]   // start/end: byte range in the block's text (whole statement)
//!   },
//!   "frames": [{"draw_calls", "passes", "upload_bytes", "digest": "<hex>"}]
//! }
//! ```
//! Functions are keyed by `jit::slot_key` (def path with type arguments), blocks by the source
//! position of their first instruction: both name the same code in another lowering of it (the
//! wasm back end's). A merge is the union of functions, blocks, imports, exports, messages,
//! assets (ranges joined) and glyphs; counts add up; runs and frames are listed per run.

use relax_compiler::json::Json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct Func {
    pub kind: String,
    pub krate: String,
    pub file: String,
    pub line: u64,
    pub col: u64,
    pub blocks: u64,
    pub hit: BTreeSet<(u64, u64)>,
}

#[derive(Default)]
pub struct Asset {
    pub path: String,
    pub size: u64,
    pub ranges: Vec<(u64, u64)>,
    pub fetches: u64,
}

#[derive(Default)]
pub struct Coverage {
    pub package: String,
    pub build: String,
    pub runs: Vec<String>,
    pub functions: BTreeMap<String, Func>,
    pub exports: BTreeMap<String, u64>,
    pub imports: BTreeMap<String, u64>,
    pub from_wasm: BTreeMap<String, u64>,
    pub to_wasm: BTreeMap<String, u64>,
    pub assets: BTreeMap<String, Asset>,
    pub glyphs: BTreeMap<String, BTreeSet<u64>>,
    pub shaders_compiled: u64,
    pub frames: Vec<String>,
    pub splash: Splash,
    /// Draw shaders not in the installed shader pack (`not in pack:` lines).
    pub pack_misses: BTreeSet<String>,
}

/// A `script_mod!` top-level statement: where it is registered, its size, and its use (None:
/// defines no object, kept).
#[derive(Default, Clone)]
pub struct SplashItem {
    pub module: String,
    pub krate: String,
    pub path: String,
    /// The block: its file, place and text hash (FNV-1a 64, hex); the item's byte range in it.
    pub file: String,
    pub line: u64,
    pub column: u64,
    pub block_hash: String,
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
    pub used: Option<bool>,
}

#[derive(Default)]
pub struct Splash {
    pub registered: BTreeSet<String>,
    pub used_modules: BTreeMap<String, String>,
    pub items: BTreeMap<String, SplashItem>,
}

impl Splash {
    /// One item seen by a run: used when any run used it.
    pub fn add(&mut self, id: String, item: SplashItem) {
        let e = self.items.entry(id).or_insert_with(|| SplashItem { used: Some(false), ..item.clone() });
        e.used = match (e.used, item.used) {
            (None, _) | (_, None) => None,
            (Some(a), Some(b)) => Some(a || b),
        };
    }

    /// The program's record (`wasm_take_splash_use`).
    pub fn add_record(&mut self, j: &Json) {
        if let Some(m) = j.get("modules") {
            for r in m.get("registered").map_or(&[][..], |r| r.arr()) {
                self.registered.insert(r.str().to_string());
            }
            if let Some(Json::Obj(used)) = m.get("used") {
                for (k, v) in used {
                    self.used_modules.entry(k.clone()).or_insert_with(|| v.str().to_string());
                }
            }
        }
        for b in j.get("blocks").map_or(&[][..], |r| r.arr()) {
            let s = |n: &str| b.get(n).map_or("", |k| k.str()).to_string();
            let bn = |n: &str| b.get(n).map_or(0.0, |k| k.num()) as u64;
            for i in b.get("items").map_or(&[][..], |r| r.arr()) {
                let used = match i.get("used") {
                    Some(Json::Bool(u)) => Some(*u),
                    _ => None,
                };
                let n = |k: &str| i.get(k).map_or(0.0, |v| v.num()) as u64;
                self.add(i.get("id").map_or("", |k| k.str()).to_string(), SplashItem {
                    module: s("module"), krate: s("crate"), path: i.get("path").map_or("", |k| k.str()).to_string(),
                    file: s("file"), line: bn("line"), column: bn("column"), block_hash: s("hash"),
                    start: n("start"), end: n("end"), bytes: n("bytes"), used,
                });
            }
        }
    }

    /// Per crate: (bytes, kept bytes, items, kept items).
    pub fn crates(&self) -> BTreeMap<String, (u64, u64, u64, u64)> {
        let mut out: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
        for i in self.items.values() {
            let e = out.entry(i.krate.clone()).or_default();
            let kept = i.used != Some(false);
            e.0 += i.bytes;
            e.2 += 1;
            if kept {
                e.1 += i.bytes;
                e.3 += 1;
            }
        }
        out
    }

    fn to_json(&self, o: &mut String) {
        let reg: Vec<String> = self.registered.iter().map(|s| esc(s)).collect();
        let used: Vec<String> = self.used_modules.iter().map(|(k, v)| format!("{}: {}", esc(k), esc(v))).collect();
        o.push_str(&format!(",\n  \"splash\": {{\n    \"modules_registered\": [{}],\n    \"modules_used\": {{{}}},\n    \"crates\": [", reg.join(", "), used.join(", ")));
        for (i, (k, (b, ub, n, un))) in self.crates().iter().enumerate() {
            o.push_str(if i == 0 { "\n      " } else { ",\n      " });
            o.push_str(&format!("{{\"crate\": {}, \"bytes\": {}, \"used_bytes\": {}, \"items\": {}, \"used_items\": {}}}", esc(k), b, ub, n, un));
        }
        o.push_str("\n    ],\n    \"items\": [");
        for (i, (id, it)) in self.items.iter().enumerate() {
            o.push_str(if i == 0 { "\n      " } else { ",\n      " });
            let used = match it.used { None => "null", Some(true) => "true", Some(false) => "false" };
            o.push_str(&format!("{{\"id\": {}, \"module\": {}, \"crate\": {}, \"path\": {}, \"file\": {}, \"line\": {}, \"column\": {}, \"block_hash\": {}, \"start\": {}, \"end\": {}, \"bytes\": {}, \"used\": {}}}", esc(id), esc(&it.module), esc(&it.krate), esc(&it.path), esc(&it.file), it.line, it.column, esc(&it.block_hash), it.start, it.end, it.bytes, used));
        }
        o.push_str("\n    ]\n  }");
    }
}

impl Asset {
    pub fn add_range(&mut self, lo: u64, hi: u64) {
        self.ranges.push((lo, hi));
        self.ranges.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::new();
        for (a, b) in self.ranges.drain(..) {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        self.ranges = out;
    }
}

pub fn esc(s: &str) -> String {
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

fn counts(out: &mut String, key: &str, m: &BTreeMap<String, u64>, field: &str, extra: &str) {
    out.push_str(&format!(",\n  {}: [", esc(key)));
    for (i, (k, v)) in m.iter().enumerate() {
        out.push_str(if i == 0 { "\n    " } else { ",\n    " });
        out.push_str(&format!("{{{}\"name\": {}, {}: {}}}", extra, esc(k), esc(field), v));
    }
    out.push_str("\n  ]");
}

impl Coverage {
    pub fn to_json(&self) -> String {
        let mut o = String::new();
        o.push_str(&format!("{{\n  \"format\": \"makepad-web-trace/1\",\n  \"package\": {},\n  \"target\": \"wasm32-unknown-unknown\",\n  \"build\": {}", esc(&self.package), esc(&self.build)));
        o.push_str(",\n  \"runs\": [");
        for (i, r) in self.runs.iter().enumerate() {
            o.push_str(if i == 0 { "\n    " } else { ",\n    " });
            o.push_str(r);
        }
        o.push_str("\n  ],\n  \"functions\": [");
        for (i, (k, f)) in self.functions.iter().enumerate() {
            o.push_str(if i == 0 { "\n    " } else { ",\n    " });
            let hit: Vec<String> = f.hit.iter().map(|(l, c)| format!("[{},{}]", l, c)).collect();
            o.push_str(&format!(
                "{{\"key\": {}, \"kind\": {}, \"crate\": {}, \"file\": {}, \"line\": {}, \"col\": {}, \"blocks\": {}, \"hit\": [{}]}}",
                esc(k), esc(&f.kind), esc(&f.krate), esc(&f.file), f.line, f.col, f.blocks, hit.join(",")
            ));
        }
        o.push_str("\n  ]");
        counts(&mut o, "exports", &self.exports, "calls", "");
        counts(&mut o, "imports", &self.imports, "calls", "\"module\": \"env\", ");
        counts(&mut o, "from_wasm", &self.from_wasm, "count", "");
        counts(&mut o, "to_wasm", &self.to_wasm, "count", "");
        o.push_str(",\n  \"assets\": [");
        for (i, (url, a)) in self.assets.iter().enumerate() {
            o.push_str(if i == 0 { "\n    " } else { ",\n    " });
            let r: Vec<String> = a.ranges.iter().map(|(lo, hi)| format!("[{},{}]", lo, hi)).collect();
            o.push_str(&format!("{{\"url\": {}, \"path\": {}, \"size\": {}, \"ranges\": [{}], \"fetches\": {}}}", esc(url), esc(&a.path), a.size, r.join(","), a.fetches));
        }
        o.push_str("\n  ],\n  \"glyphs\": [");
        for (i, (url, g)) in self.glyphs.iter().enumerate() {
            o.push_str(if i == 0 { "\n    " } else { ",\n    " });
            let ids: Vec<String> = g.iter().map(|x| x.to_string()).collect();
            o.push_str(&format!("{{\"url\": {}, \"glyphs\": [{}]}}", esc(url), ids.join(",")));
        }
        o.push_str(&format!("\n  ],\n  \"shaders\": {{\"compiled\": {}}}", self.shaders_compiled));
        self.splash.to_json(&mut o);
        let misses: Vec<String> = self.pack_misses.iter().map(|m| esc(m)).collect();
        o.push_str(&format!(",\n  \"pack_misses\": [{}]", misses.join(", ")));
        o.push_str(",\n  \"frames\": [");
        for (i, f) in self.frames.iter().enumerate() {
            o.push_str(if i == 0 { "\n    " } else { ",\n    " });
            o.push_str(f);
        }
        o.push_str("\n  ]\n}\n");
        o
    }

    /// Adds a coverage file (merge).
    pub fn add(&mut self, j: &Json) -> Result<(), String> {
        if j.get("format").map(|f| f.str()) != Some("makepad-web-trace/1") {
            return Err("not a makepad-web-trace/1 file".into());
        }
        let pkg = j.get("package").map_or("", |p| p.str());
        if self.package.is_empty() {
            self.package = pkg.to_string();
            self.build = j.get("build").map_or("", |p| p.str()).to_string();
        } else if self.package != pkg {
            return Err(format!("package {} is not {}", pkg, self.package));
        }
        for r in j.get("runs").map_or(&[][..], |r| r.arr()) {
            self.runs.push(write_json(r));
        }
        for f in j.get("functions").map_or(&[][..], |r| r.arr()) {
            let key = f.get("key").map_or("", |k| k.str()).to_string();
            let e = self.functions.entry(key).or_default();
            let s = |n: &str| f.get(n).map_or("", |k| k.str()).to_string();
            let n = |n: &str| f.get(n).map_or(0.0, |k| k.num()) as u64;
            if e.kind.is_empty() {
                (e.kind, e.krate, e.file, e.line, e.col) = (s("kind"), s("crate"), s("file"), n("line"), n("col"));
            }
            e.blocks = e.blocks.max(n("blocks"));
            for h in f.get("hit").map_or(&[][..], |h| h.arr()) {
                let a = h.arr();
                if a.len() == 2 {
                    e.hit.insert((a[0].num() as u64, a[1].num() as u64));
                }
            }
        }
        for (key, field, map) in [("exports", "calls", &mut self.exports), ("imports", "calls", &mut self.imports), ("from_wasm", "count", &mut self.from_wasm), ("to_wasm", "count", &mut self.to_wasm)] {
            for x in j.get(key).map_or(&[][..], |r| r.arr()) {
                *map.entry(x.get("name").map_or("", |k| k.str()).to_string()).or_default() += x.get(field).map_or(0.0, |k| k.num()) as u64;
            }
        }
        for a in j.get("assets").map_or(&[][..], |r| r.arr()) {
            let e = self.assets.entry(a.get("url").map_or("", |k| k.str()).to_string()).or_default();
            e.path = a.get("path").map_or("", |k| k.str()).to_string();
            e.size = a.get("size").map_or(0.0, |k| k.num()) as u64;
            e.fetches += a.get("fetches").map_or(0.0, |k| k.num()) as u64;
            for r in a.get("ranges").map_or(&[][..], |r| r.arr()) {
                let r = r.arr();
                if r.len() == 2 {
                    e.add_range(r[0].num() as u64, r[1].num() as u64);
                }
            }
        }
        for g in j.get("glyphs").map_or(&[][..], |r| r.arr()) {
            let e = self.glyphs.entry(g.get("url").map_or("", |k| k.str()).to_string()).or_default();
            for id in g.get("glyphs").map_or(&[][..], |r| r.arr()) {
                e.insert(id.num() as u64);
            }
        }
        for m in j.get("pack_misses").map_or(&[][..], |r| r.arr()) {
            self.pack_misses.insert(m.str().to_string());
        }
        if let Some(s) = j.get("splash") {
            for r in s.get("modules_registered").map_or(&[][..], |r| r.arr()) {
                self.splash.registered.insert(r.str().to_string());
            }
            if let Some(Json::Obj(used)) = s.get("modules_used") {
                for (k, v) in used {
                    self.splash.used_modules.entry(k.clone()).or_insert_with(|| v.str().to_string());
                }
            }
            for i in s.get("items").map_or(&[][..], |r| r.arr()) {
                let t = |n: &str| i.get(n).map_or("", |k| k.str()).to_string();
                let used = match i.get("used") {
                    Some(Json::Bool(u)) => Some(*u),
                    _ => None,
                };
                let n = |k: &str| i.get(k).map_or(0.0, |v| v.num()) as u64;
                self.splash.add(t("id"), SplashItem { module: t("module"), krate: t("crate"), path: t("path"), file: t("file"), line: n("line"), column: n("column"), block_hash: t("block_hash"), start: n("start"), end: n("end"), bytes: n("bytes"), used });
            }
        }
        self.shaders_compiled += j.get("shaders").and_then(|s| s.get("compiled")).map_or(0.0, |k| k.num()) as u64;
        for f in j.get("frames").map_or(&[][..], |r| r.arr()) {
            self.frames.push(write_json(f));
        }
        Ok(())
    }
}

/// A JSON value written back (merge copies runs and frames verbatim).
pub fn write_json(j: &Json) -> String {
    match j {
        Json::Null => "null".into(),
        Json::Bool(b) => b.to_string(),
        Json::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", *n as i64) } else { format!("{}", n) }
        }
        Json::Str(s) => esc(s),
        Json::Arr(v) => format!("[{}]", v.iter().map(write_json).collect::<Vec<_>>().join(", ")),
        Json::Obj(v) => format!("{{{}}}", v.iter().map(|(k, x)| format!("{}: {}", esc(k), write_json(x))).collect::<Vec<_>>().join(", ")),
    }
}

/// `makepad-web-trace merge <out> <in>...`
pub fn merge(out: &str, inputs: &[String]) -> Result<(), String> {
    let mut c = Coverage::default();
    for p in inputs {
        let d = std::fs::read(p).map_err(|e| format!("{}: {}", p, e))?;
        let j = relax_compiler::json::parse(&d).map_err(|e| format!("{}: {}", p, e))?;
        c.add(&j).map_err(|e| format!("{}: {}", p, e))?;
    }
    std::fs::write(out, c.to_json()).map_err(|e| format!("{}: {}", out, e))
}

/// `makepad-web-trace gate <A.json> <B.json>`: the replay-B gate (DESIGN.md "Rare-but-required
/// paths"). A build from trace A ships only what A ran; run B (other sizes, full length, another
/// seek order) is what a viewer may do. Everything B used that A did not is a stub hit in that
/// build: functions, blocks, assets (bytes outside A's ranges), glyphs and Splash definitions A
/// dropped. Prints each kind's misses (the first few by name) and fails on any.
pub fn gate(a_path: &str, b_path: &str) -> Result<bool, String> {
    let load = |p: &str| -> Result<Coverage, String> {
        let d = std::fs::read(p).map_err(|e| format!("{}: {}", p, e))?;
        let j = relax_compiler::json::parse(&d).map_err(|e| format!("{}: {}", p, e))?;
        let mut c = Coverage::default();
        c.add(&j).map_err(|e| format!("{}: {}", p, e))?;
        Ok(c)
    };
    let (a, b) = (load(a_path)?, load(b_path)?);
    let mut misses: Vec<(&str, Vec<String>)> = Vec::new();
    let mut fns = Vec::new();
    let mut blocks = Vec::new();
    for (k, f) in &b.functions {
        match a.functions.get(k) {
            None => fns.push(k.clone()),
            Some(af) => {
                for h in f.hit.difference(&af.hit) {
                    blocks.push(format!("{} @{}:{}", k, h.0, h.1));
                }
            }
        }
    }
    misses.push(("functions", fns));
    misses.push(("blocks", blocks));
    let mut assets = Vec::new();
    for (url, x) in &b.assets {
        let Some(ax) = a.assets.get(url) else {
            assets.push(url.clone());
            continue;
        };
        for (lo, hi) in &x.ranges {
            if !ax.ranges.iter().any(|(alo, ahi)| alo <= lo && hi <= ahi) {
                assets.push(format!("{} [{}, {})", url, lo, hi));
            }
        }
    }
    misses.push(("asset bytes", assets));
    let mut glyphs = Vec::new();
    for (url, g) in &b.glyphs {
        let empty = BTreeSet::new();
        let ag = a.glyphs.get(url).unwrap_or(&empty);
        let extra: Vec<String> = g.difference(ag).map(|x| x.to_string()).collect();
        if !extra.is_empty() {
            glyphs.push(format!("{}: {}", url, extra.join(",")));
        }
    }
    misses.push(("glyphs", glyphs));
    let mut splash = Vec::new();
    for (id, it) in &b.splash.items {
        if it.used == Some(true) && a.splash.items.get(id).is_some_and(|x| x.used == Some(false)) {
            splash.push(id.clone());
        }
    }
    misses.push(("Splash definitions", splash));
    // a shader A's run applied and never drew is not in the pack in A's run too: only new ones count
    misses.push(("draw shaders not in the pack", b.pack_misses.difference(&a.pack_misses).cloned().collect()));
    // GPU texture growth (the old film gate failed on a leak, 83 -> 2061 MB): B, longer, may not
    // hold more than twice A's most live texture bytes
    let peak = |c: &Coverage| c.frames.iter().filter_map(|f| relax_compiler::json::parse(f.as_bytes()).ok()).filter_map(|j| j.get("texture_bytes").map(|v| v.num() as u64)).max().unwrap_or(0);
    let (pa, pb) = (peak(&a), peak(&b));
    eprintln!("gate: live GPU textures peak: A {} MB, B {} MB", pa >> 20, pb >> 20);
    let mut ok = true;
    if pa > 0 && pb > 2 * pa {
        ok = false;
        eprintln!("gate: GPU textures grew: B holds {} MB at its peak, more than twice A's {} MB", pb >> 20, pa >> 20);
    }
    for (kind, list) in &misses {
        if list.is_empty() {
            eprintln!("gate: {}: 0 stub hits", kind);
        } else {
            ok = false;
            eprintln!("gate: {}: {} stub hits, e.g.", kind, list.len());
            for m in list.iter().take(12) {
                eprintln!("  {}", m);
            }
        }
    }
    eprintln!("gate: {}", if ok { "PASS (B ran nothing A did not)" } else { "FAIL" });
    Ok(ok)
}
