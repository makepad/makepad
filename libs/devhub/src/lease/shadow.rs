//! Master-owned tree/overlay pairs. The requested run never waits for its shadow or comparison.

use super::overlay_compare as compare;
use crate::orch::Ctx;
use crate::state::Job;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, Seek, Write};
use std::path::Path;
use std::sync::Arc;

/// One worker keeps journal I/O out of job dispatch and the record/UI path.
pub fn start(ctx: Arc<Ctx>) {
    let _ = std::thread::Builder::new().name("devhub-shadow".into()).spawn(move || {
        let mut recorder = None;
        loop {
            let r = (|| {
                if recorder.is_none() { recorder = Some(Recorder::open(&ctx)?); }
                recorder.as_mut().unwrap().tick(&ctx)
            })();
            if let Err(e) = r { recorder = None; ctx.store.event("shadow", crate::proto::Sev::Warn, format!("shadow: {e}"), None); }
            ctx.store.wait(std::time::Duration::from_secs(2));
        }
    });
}

fn journal(path: &Path, repair: bool) -> Result<Vec<Value>, String> {
    let mut f = match std::fs::OpenOptions::new().read(true).write(repair).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    (if repair { f.lock() } else { f.lock_shared() }).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(&f);
    let (mut rows, mut end) = (Vec::new(), 0u64);
    let mut tail = None;
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).map_err(|e| e.to_string())? == 0 { break; }
        let parsed = makepad_strict_json::parse(&line);
        if !line.ends_with(b"\n") {
            if let Ok(v) = parsed { rows.push(v); tail = Some(true); } else { tail = Some(false); }
            break;
        }
        rows.push(parsed.map_err(|e| format!("shadow.jsonl: {e:?}"))?);
        end += line.len() as u64;
    }
    drop(reader);
    if repair {
        match tail {
            Some(true) => { f.seek(std::io::SeekFrom::End(0)).and_then(|_| f.write_all(b"\n")).map_err(|e| e.to_string())?; }
            Some(false) => f.set_len(end).map_err(|e| e.to_string())?,
            None => {}
        }
        if tail.is_some() { f.sync_data().map_err(|e| e.to_string())?; }
    }
    Ok(rows)
}

pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let word = line.strip_prefix("overlay-shadow ").unwrap_or(line).trim();
    match word {
        "on" | "off" => {
            ctx.store.update(|d| d.devhub_shadow = word == "on");
            Ok(format!("{{\"text\":\"DEVHUB_SHADOW {word}\\n\"}}"))
        }
        "enabled" => Ok(format!("{{\"on\":{}}}", ctx.store.read(|d| d.devhub_shadow))),
        "status" => {
            let now = crate::proto::wall_ms();
            let mut counts = [0, 0, 0, 0];
            let mut entries = Vec::new();
            for v in journal(&ctx.home.join("shadow.jsonl"), false)? {
                let ended = v.get("ended_ms").and_then(Value::as_u64).unwrap_or(0);
                if ended > now || now - ended > 86_400_000 { continue; }
                if v.get("jit_error").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) { counts[2] += 1; }
                else if v.get("equal").and_then(Value::as_bool) == Some(true) { counts[0] += 1; }
                else if v.get("inconclusive").and_then(Value::as_bool) == Some(true) { counts[3] += 1; }
                else { counts[1] += 1; entries.push(v.to_json()); }
            }
            let on = ctx.store.read(|d| d.devhub_shadow);
            let mut text = format!("DEVHUB_SHADOW {}: last 24 h: {} agreed, {} disagreed, {} JIT errors, {} inconclusive (live samples)\n", if on { "on" } else { "off" }, counts[0], counts[1], counts[2], counts[3]);
            for entry in entries { text.push_str(&entry); text.push('\n'); }
            Ok(format!("{{\"text\":{}}}", json_str(&text)))
        }
        _ => Err("overlay shadow on|off|status".into()),
    }
}

/// The journal is read once at startup; receipts make append-before-state crash recovery idempotent.
struct Recorder { recorded: HashSet<String> }

impl Recorder {
    fn open(ctx: &Ctx) -> Result<Self, String> {
        let recorded = journal(&ctx.home.join("shadow.jsonl"), true)?.iter().filter_map(|v| v.get("overlay_job").and_then(Value::as_str).map(str::to_string)).collect();
        Ok(Self { recorded })
    }

    fn tick(&mut self, ctx: &Ctx) -> Result<(), String> {
        let jobs: Vec<Job> = ctx.store.read(|d| d.jobs.iter().filter(|j| (j.shadow || !j.shadow_of.is_empty() || !j.live_sample.is_empty()) && !j.shadow_recorded).cloned().collect());
        self.recorded.retain(|id| jobs.iter().any(|j| &j.id == id));
        if jobs.is_empty() { return Ok(()); }
        let nodes = ctx.leases.nodes();
        for primary in jobs.iter().filter(|j| j.shadow && j.shadow_job.is_empty() && j.granted_ms > 0) {
            // Class follows the granted box, even when the user's soft pin fell to a twin.
            let mut needs = super::sched::parse_needs(&primary.needs);
            if let Some(node) = nodes.iter().find(|n| n.name == primary.node) {
                if let Some(caps) = &node.caps {
                    needs.retain(|(k, _)| !matches!(k.as_str(), "arch" | "os" | "role"));
                    needs.extend([("arch".into(), caps.arch.clone()), ("os".into(), caps.os.clone()), ("role".into(), node.role().as_str().into())]);
                }
            }
            if ["arch", "os", "role"].iter().any(|key| !needs.iter().any(|(k, v)| k.as_str() == *key && !v.is_empty())) { continue; }
            ctx.store.update(|d| {
                if !d.jobs.iter().any(|j| j.id == primary.id && j.shadow_job.is_empty()) { return; }
                d.jobs_next += 1;
                let id = format!("J{}", d.jobs_next);
                let copy = Job {
                    id: id.clone(), lane: primary.lane.clone(), src: if primary.src == "overlay" { "tree" } else { "overlay" }.into(), priority: "background".into(), prio: crate::orch::leg::BACKGROUND_PRIO,
                    node_pref: primary.node.clone(),
                    agent: "devhub-shadow".into(), kind: "test".into(), cost: primary.cost.clone(), est_min: primary.est_min, cores: primary.cores, ram_gb: primary.ram_gb, whole: primary.whole,
                    class: format!("shadow:{}", primary.class), loader: primary.loader.clone(), needs: needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(","),
                    rev: primary.rev.clone(), repo_dir: primary.repo_dir.clone(), snap: primary.snap.clone(), snap_files: primary.snap_files,
                    cmd: primary.cmd.clone(), cwd: primary.cwd.clone(), shadow_of: primary.id.clone(), state: "queued".into(), submitted_ms: crate::proto::wall_ms(), ..Default::default()
                };
                d.jobs.iter_mut().find(|j| j.id == primary.id).unwrap().shadow_job = id;
                d.jobs.push(copy);
            });
        }
        for sample in jobs.iter().filter(|j| !j.live_sample.is_empty() && !j.shadow_recorded && j.done()) {
            if !self.recorded.contains(&sample.id) {
                // a sample that never ran (cancelled, lost) says nothing
                if sample.run_n > 0 {
                    let entry = live_entry(ctx, sample)?;
                    let mut file = std::fs::OpenOptions::new().create(true).read(true).append(true).open(ctx.home.join("shadow.jsonl")).map_err(|e| e.to_string())?;
                    file.lock().map_err(|e| e.to_string())?;
                    file.write_all(format!("{entry}\n").as_bytes()).and_then(|_| file.sync_data()).map_err(|e| e.to_string())?;
                }
                self.recorded.insert(sample.id.clone());
            }
            ctx.store.update(|d| { if let Some(j) = d.jobs.iter_mut().find(|j| j.id == sample.id) { j.shadow_recorded = true; } });
        }
        for primary in jobs.iter().filter(|j| j.shadow && !j.shadow_recorded && j.done()) {
            if primary.granted_ms == 0 && primary.shadow_job.is_empty() {
                ctx.store.update(|d| { if let Some(j) = d.jobs.iter_mut().find(|j| j.id == primary.id) { j.shadow_recorded = true; } });
                continue;
            }
            let Some(shadow) = jobs.iter().find(|j| j.id == primary.shadow_job && j.done()) else { continue };
            let (tree, overlay) = if primary.src == "overlay" { (shadow, primary) } else { (primary, shadow) };
            if !self.recorded.contains(&overlay.id) {
                let entry = entry(ctx, tree, overlay)?;
                let mut file = std::fs::OpenOptions::new().create(true).read(true).append(true).open(ctx.home.join("shadow.jsonl")).map_err(|e| e.to_string())?;
                file.lock().map_err(|e| e.to_string())?;
                file.write_all(format!("{entry}\n").as_bytes()).and_then(|_| file.sync_data()).map_err(|e| e.to_string())?;
                self.recorded.insert(overlay.id.clone());
            }
            ctx.store.update(|d| { for j in d.jobs.iter_mut().filter(|j| j.id == primary.id || j.id == shadow.id) { j.shadow_recorded = true; } });
        }
        Ok(())
    }
}

/// A sampled `--live` verdict against the real checkout's cargo run of the same files.
fn live_entry(ctx: &Ctx, oracle: &Job) -> Result<String, String> {
    let log = std::fs::read_to_string(ctx.logs("jobs").join(format!("{}.log", oracle.id))).map_err(|e| format!("{} log: {e}", oracle.id))?;
    let sample = makepad_strict_json::parse(oracle.live_sample.as_bytes()).map_err(|_| "bad live sample".to_string())?;
    let (diff, inconclusive) = crate::orch::live_shadow::compare(&sample, oracle.exit, &log);
    let text = |k: &str| sample.get(k).and_then(Value::as_str).unwrap_or("");
    let pkg = oracle.cmd.get(4).map_or("", String::as_str);
    Ok(format!("{{\"lane\":{},\"pkg\":{},\"target\":\"live\",\"live\":{},\"tree_exit\":{},\"overlay_exit\":null,\"equal\":{},\"inconclusive\":{},\"first_diff\":{},\"tree_job\":{},\"overlay_job\":{},\"ended_ms\":{},\"jit_error\":\"\"}}",
        json_str(&oracle.lane), json_str(pkg), json_str(text("live")), oracle.exit.map_or("null".into(), |n| n.to_string()), diff.is_none() && !inconclusive, inconclusive, diff.as_deref().map_or("null".into(), json_str), json_str(&oracle.id), json_str(&oracle.id), crate::proto::wall_ms()))
}

fn entry(ctx: &Ctx, tree: &Job, overlay: &Job) -> Result<String, String> {
    let read = |j: &Job| -> Result<String, String> {
        if j.run_n == 0 { return Ok(String::new()); }
        std::fs::read_to_string(ctx.logs("jobs").join(format!("{}.log", j.id))).map_err(|e| format!("{} log: {e}", j.id))
    };
    let (a, b) = (read(tree)?, read(overlay)?);
    let mut jit_error = String::new();
    for (side, j, text) in [("tree", tree, &a), ("overlay", overlay, &b)] {
        if let Some(e) = compare::jit_error(text).or_else(|| (!j.jit_error.is_empty()).then(|| j.jit_error.clone())).or_else(|| j.panic.as_ref().map(|p| format!("JIT panic: {}", p.message))) {
            jit_error = format!("{side}: {e}");
            break;
        }
    }
    let first = if !jit_error.is_empty() { Some(jit_error.clone()) }
        else if tree.exit.is_none() || overlay.exit.is_none() { Some(format!("missing verdict: tree={:?} {}; overlay={:?} {}", tree.exit, tree.note, overlay.exit, overlay.note)) }
        else if !a.lines().any(|l| l.starts_with("test result:")) || !b.lines().any(|l| l.starts_with("test result:")) { Some("missing test result: tree or overlay did not reach libtest".into()) }
        else { compare::first_diff(&compare::summary(tree.exit.unwrap(), &a), &compare::summary(overlay.exit.unwrap(), &b)) };
    let args = super::loaders::cargo_run_args(&tree.cmd).ok_or("shadow tree has no JIT command")?;
    let pkg = args.get(1).map_or("", String::as_str);
    let target = args.iter().take_while(|s| s.as_str() != "--").enumerate().find_map(|(i, a)| match a.as_str() {
        "--lib" => Some("lib".to_string()), "--tests" => Some("tests".to_string()), "--test" => Some(format!("test:{}", args.get(i + 1).map_or("", String::as_str))), _ => None,
    }).unwrap_or_else(|| "default".into());
    Ok(format!("{{\"lane\":{},\"pkg\":{},\"target\":{},\"tree_exit\":{},\"overlay_exit\":{},\"equal\":{},\"first_diff\":{},\"tree_job\":{},\"overlay_job\":{},\"ended_ms\":{},\"jit_error\":{}}}",
        json_str(&tree.lane), json_str(pkg), json_str(&target), tree.exit.map_or("null".into(), |n| n.to_string()), overlay.exit.map_or("null".into(), |n| n.to_string()), first.is_none(), first.as_deref().map_or("null".into(), json_str), json_str(&tree.id), json_str(&overlay.id), crate::proto::wall_ms(), json_str(&jit_error)))
}
