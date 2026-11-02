//! JIT placement hints and fleet build counts. Binaries stay on their own boxes.

use super::sched::NodeView;
use makepad_strict_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::Path;

/// Cheap content hint from a box's reported compiler and .inputs list. Dirty inputs have no hint.
pub fn head_id(root: &Path, nodes: &[NodeView]) -> Option<String> {
    let store = nodes.iter().filter_map(|n| n.caps.as_ref()?.jit_store.as_ref()).find(|s| !s.inputs.is_empty() && !s.compiler.is_empty())?;
    let inputs: Vec<&str> = store.inputs.iter().map(String::as_str).collect();
    if inputs.iter().any(|p| p.is_empty() || Path::new(p).components().any(|c| !matches!(c, std::path::Component::Normal(_)))) { return None; }
    let run = |args: &[&str], paths: &[&str]| makepad_remote::group::command("git").arg("-C").arg(root).arg("--literal-pathspecs").args(args).args(paths).stdin(std::process::Stdio::null()).output().ok();
    // The index names HEAD's blobs only while these inputs have no staged or unstaged edits.
    // Bound argv just like the monitor: a JIT can have thousands of input paths.
    let mut listing = String::new();
    for paths in inputs.chunks(500) {
        if !run(&["diff", "--quiet", "HEAD", "--"], paths)?.status.success() { return None; }
        let out = run(&["ls-files", "-s", "-z", "--"], paths)?;
        if !out.status.success() { return None; }
        listing.push_str(&String::from_utf8(out.stdout).ok()?);
    }
    let blobs: HashMap<&str, &str> = listing.split('\0').filter_map(|e| {
        let (meta, path) = e.split_once('\t')?;
        let mut words = meta.split_whitespace();
        words.next()?;
        let blob = words.next()?;
        (words.next()? == "0").then_some((path, blob))
    }).collect();
    let mut text = store.compiler.clone();
    for p in inputs {
        text.push('\n');
        text.push_str(p);
        text.push(' ');
        text.push_str(blobs.get(p)?);
    }
    Some(makepad_network::digest::sha256_hash(text.as_bytes())[..12].iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Default)]
struct Tally { n: u64, secs: f64 }
impl Tally { fn add(&mut self, n: u64, secs: f64) { self.n += n; self.secs += secs; } }

/// UTC day buckets; --day limits build records to today. Caps counters are lifetime totals.
pub fn stats(path: &Path, nodes: &[NodeView], today: bool, now: u64) -> Result<String, String> {
    let mut builds: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut days: BTreeMap<(u64, String), Tally> = BTreeMap::new();
    let mut incomplete = 0;
    let file = match std::fs::File::open(path) {
        Ok(f) => Some(f),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(file) = file {
        for line in std::io::BufReader::new(file).lines() {
            let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            if v.get("class").and_then(Value::as_str) != Some("jit-build") { continue; }
            let s = |k| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
            let (Some(arch), Some(node), Some(ended)) = (s("arch"), s("node"), v.get("ended_ms").and_then(Value::as_u64)) else { incomplete += 1; continue };
            let day = ended / 86_400_000;
            if today && day != now / 86_400_000 { continue; }
            // the master's records carry the builds a node's store reported since the last report
            let n = v.get("builds").and_then(Value::as_u64).unwrap_or(1);
            let secs = v.get("wall_ms").and_then(Value::as_u64).unwrap_or(0) as f64 / 1000.0 * n as f64;
            builds.entry((node.to_string(), arch.to_string())).or_default().add(n, secs);
            days.entry((day, node.to_string())).or_default().add(n, secs);
        }
    }
    let mut out = format!("JIT builds {}\n", if today { "today (UTC)" } else { "all days (UTC)" });
    for ((node, arch), t) in &builds { out += &format!("  {node} {arch}: {} builds, {:.0} s\n", t.n, t.secs); }
    if builds.is_empty() { out += "  no jit-build records\n"; }
    if incomplete > 0 { out += &format!("  {incomplete} records missing arch/node/ended_ms\n"); }
    out += "store hits/misses per box (lifetime caps counters)\n";
    for n in nodes {
        let store = n.caps.as_ref().and_then(|c| c.jit_store.as_ref());
        let hits = store.map_or("unknown".into(), |s| s.hits.to_string());
        let misses = store.map_or("unknown".into(), |s| s.misses.to_string());
        out += &format!("  {}: {hits} hits, {misses} misses{}\n", n.name, if n.up { "" } else { " (last reported; offline)" });
    }
    out += "builds per box per day (UTC)\n";
    for ((day, node), t) in days {
        let (y, m, d) = makepad_civil_time::to_ymd(day as i32);
        out += &format!("  {y:04}-{m:02}-{d:02} {node}: {} builds, {:.0} s\n", t.n, t.secs);
    }
    Ok(out)
}
