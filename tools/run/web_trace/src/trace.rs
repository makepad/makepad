//! The coverage file of a traced run (`makepad-web-trace/1`), and the merge of several runs.
//!
//! ```text
//! {
//!   "format": "makepad-web-trace/1",
//!   "package": "makepad-app-calculator", "target": "wasm32-unknown-unknown", "build": "<relax provenance>",
//!   "runs": [{"name", "script", "window": [w, h, dpi], "frames", "virtual_ms", "panics": [..]}],
//!   "functions": [{"key", "kind", "crate", "file", "line", "col", "blocks", "hit": [[line, col], ..]}],
//!   "exports": [{"name", "calls"}],             // the program's wasm exports the page called
//!   "imports": [{"module": "env", "name", "calls"}],
//!   "from_wasm": [{"name", "count"}],           // messages the program sent the page (web.js handlers)
//!   "to_wasm": [{"name", "count"}],             // messages the page sent the program
//!   "assets": [{"url", "path", "size", "ranges": [[lo, hi), ..], "fetches"}],
//!   "glyphs": [{"url", "glyphs": [id, ..]}],    // per font file, the glyph ids the program outlined
//!   "shaders": {"compiled", "programs"},
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
