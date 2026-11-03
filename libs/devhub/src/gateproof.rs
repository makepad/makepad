//! Passing gate units, keyed by the inputs of the tree under test.

use crate::orch::ci::{package_dirs, read_proofs, Digests, Os, Tree, Unit};
use crate::state::Pair;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

#[derive(Default)]
struct Data {
    keys: HashMap<String, (String, String, u64)>,
    units: BTreeMap<String, u64>,
    lines: usize,
}

impl Data {
    fn insert(&mut self, unit: String, key: String, src: String, ms: u64) {
        let newest = self.units.entry(unit.clone()).or_default();
        *newest = (*newest).max(ms);
        if self.keys.get(&key).is_none_or(|(_, _, old)| ms >= *old) {
            self.keys.insert(key, (unit, src, ms));
        }
    }
}

pub struct Store {
    path: PathBuf,
    data: Mutex<Data>,
}

fn line(unit: &str, key: &str, src: &str, ms: u64) -> String {
    let mut time = String::new();
    Value::Int(ms as i64).write_into(&mut time);
    format!("{{\"unit\":{},\"key\":{},\"src\":{},\"ms\":{time}}}\n", json_str(unit), json_str(key), json_str(src))
}

impl Store {
    pub fn open(home: &Path) -> Store {
        let path = home.join("gate-proofs.jsonl");
        let mut data = Data::default();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in text.lines() {
                    data.lines += 1;
                    let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
                    let get = |k| v.get(k).and_then(Value::as_str);
                    let (Some(unit), Some(key), Some(src), Some(ms)) = (get("unit"), get("key"), get("src"), v.get("ms").and_then(Value::as_u64)) else { continue };
                    data.insert(unit.into(), key.into(), src.into(), ms);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("gate proofs: {}: {e}", path.display()),
        }
        let store = Store { path, data: Mutex::new(data) };
        store.compact(&mut store.data.lock().unwrap());
        store
    }

    pub fn record(&self, unit: &str, key: &str, src: &str) -> bool {
        let mut data = self.data.lock().unwrap();
        let ms = crate::proto::wall_ms();
        let append = || -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() { std::fs::create_dir_all(parent)?; }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
            file.write_all(line(unit, key, src, ms).as_bytes())
        };
        if let Err(e) = append() {
            eprintln!("gate proofs: {}: {e}", self.path.display());
            return false;
        }
        data.lines += 1;
        data.insert(unit.into(), key.into(), src.into(), ms);
        self.compact(&mut data);
        true
    }

    fn compact(&self, data: &mut Data) {
        if data.lines <= 20_000 { return; }
        let mut rows: Vec<_> = data.keys.iter().collect();
        rows.sort_by(|a, b| b.1.2.cmp(&a.1.2).then_with(|| a.0.cmp(b.0)));
        rows.truncate(10_000);
        let mut kept = Data::default();
        let mut text = String::new();
        for (key, (unit, src, ms)) in rows {
            text.push_str(&line(unit, key, src, *ms));
            kept.insert(unit.clone(), key.clone(), src.clone(), *ms);
            kept.lines += 1;
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path)) {
            eprintln!("gate proofs: compact {}: {e}", self.path.display());
        } else {
            *data = kept;
        }
    }

    pub fn proven(&self, repo: &Path, pair: &Pair) -> (Vec<String>, Vec<String>) {
        let candidates: Vec<String> = self.data.lock().unwrap().units.keys().cloned().collect();
        let find = || -> Result<(Vec<String>, Vec<String>), String> {
            let tree = Tree::at(repo, pair)?;
            let dirs = package_dirs(&tree);
            let digests = digests(repo, &tree, &dirs, &candidates, true)?;
            let mut works = HashMap::new();
            let (mut units, mut why) = (Vec::new(), Vec::new());
            for unit in &candidates {
                let Some(app) = app_unit(&dirs, unit) else { continue };
                let d = digests.of(&app, Os::Linux);
                let Some(key) = key_digest(&tree, &app.dir, unit, d) else { continue };
                let src = self.data.lock().unwrap().keys.get(&key).map(|(_, src, _)| src.clone());
                if let Some(src) = src {
                    units.push(unit.clone());
                    why.push(format!("GATEPROOF proven {unit} ({src})"));
                    continue;
                }
                let section = if unit.starts_with("present ") { "app" }
                    else if unit == "devhub makepad-devhub" { "tests" }
                    else { continue };
                let path = if app.dir.is_empty() { "ci.toml".to_string() } else { format!("{}/ci.toml", app.dir) };
                let Some(proofs) = tree.read(&path).and_then(|s| read_proofs(&s).ok()) else { continue };
                let Some(proof) = proofs.get(&(section.to_string(), Os::Linux)) else { continue };
                let digest = digests.of(&Unit { dir: app.dir.clone(), section: section.into() }, Os::Linux);
                if digest.is_empty() || proof.digest != digest { continue; }
                let same = works.entry(proof.pair.work.clone()).or_insert_with(|| same_config(repo, &tree, &proof.pair.work));
                if !*same { continue; }
                units.push(unit.clone());
                why.push(format!("GATEPROOF proven {unit} (ci.toml {section}.linux {} {})", proof.pair.short(), proof.date));
            }
            Ok((units, why))
        };
        find().unwrap_or_else(|e| (Vec::new(), vec![format!("GATEPROOF none: {e}")]))
    }

    pub fn record_log(&self, repo: &Path, pair: &Pair, log: &str, src: &str) -> usize {
        let units: Vec<String> = log.lines().filter_map(|l| l.strip_prefix("GATEUNIT ok ").map(str::to_string)).collect();
        if units.is_empty() { return 0; }
        let mut count = 0;
        for (unit, key) in self.keys(repo, pair, &units) {
            let Some(key) = key else { continue };
            if self.record(&unit, &key, src) { count += 1; }
        }
        count
    }

    pub fn keys(&self, repo: &Path, pair: &Pair, units: &[String]) -> BTreeMap<String, Option<String>> {
        keys(repo, pair, units)
    }
}

/// Input keys do not need the proof ledger.
pub(crate) fn keys(repo: &Path, pair: &Pair, units: &[String]) -> BTreeMap<String, Option<String>> {
    let mut keys: BTreeMap<String, Option<String>> = units.iter().map(|u| (u.clone(), None)).collect();
    if units.is_empty() { return keys; }
    let Ok(tree) = Tree::at(repo, pair) else { return keys };
    let dirs = package_dirs(&tree);
    let Ok(digests) = digests(repo, &tree, &dirs, units, false) else { return keys };
    for (unit, key) in &mut keys {
        let Some(app) = app_unit(&dirs, unit) else { continue };
        *key = key_digest(&tree, &app.dir, unit, digests.of(&app, Os::Linux));
    }
    keys
}

fn app_unit(dirs: &BTreeMap<String, String>, unit: &str) -> Option<Unit> {
    let dir = dirs.get(unit.split_whitespace().nth(1)?)?;
    Some(Unit { dir: dir.clone(), section: "app".into() })
}

/// The lock is the checkout's (Cargo.lock is not tracked), as CI's `Digests::at` reads it.
fn digests(repo: &Path, tree: &Tree, dirs: &BTreeMap<String, String>, units: &[String], ci: bool) -> Result<Digests, String> {
    let mut apps: Vec<Unit> = units.iter().filter_map(|u| app_unit(dirs, u)).collect();
    if ci && units.iter().any(|u| u == "devhub makepad-devhub") {
        if let Some(dir) = dirs.get("makepad-devhub") { apps.push(Unit { dir: dir.clone(), section: "tests".into() }); }
    }
    let lock = std::fs::read_to_string(repo.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    Digests::with_lock(tree, &apps, lock)
}

fn key_digest(tree: &Tree, dir: &str, unit: &str, digest: &str) -> Option<String> {
    if digest.is_empty() { return None; }
    let mut text = format!("gate-proof-v1\n{unit}\n{digest}\nCargo.toml {}\n", tree.blob("Cargo.toml")?);
    for path in [".cargo/config.toml", "tools/run/fixture/land_gate.sh", "tools/run/fixture/affected.py", &format!("{dir}/Cargo.lock")] {
        text.push_str(&format!("{path} {}\n", tree.blob(path).unwrap_or("-")));
    }
    Some(makepad_ci_merge::sha256(text.as_bytes()))
}

fn same_config(repo: &Path, tree: &Tree, work: &str) -> bool {
    for path in ["Cargo.toml", ".cargo/config.toml"] {
        let Ok(out) = crate::git_exec::command().arg("-C").arg(repo).args(["rev-parse", "--verify", &format!("{work}:{path}")])
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null()).stderr(Stdio::null()).output() else { return false };
        if out.status.success() {
            if tree.blob(path) != Some(String::from_utf8_lossy(&out.stdout).trim()) { return false; }
        } else if tree.blob(path).is_some() || path == "Cargo.toml" {
            return false;
        }
    }
    true
}
