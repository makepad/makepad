//! Shadow samples of `--live` verdicts: one live verdict in `EVERY` is checked against a real
//! checkout. The lane's files, as the verdict saw them, run through `cargo test --release` as a
//! low-priority background job; `lease::shadow` compares the two and journals the evidence.
//! Nothing waits for it: the lane has its live verdict already.

use super::Ctx;
use crate::state::{Job, Live, LiveTests};
use makepad_remote::json_str;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// One live verdict in this many is sampled.
const EVERY: u64 = 4;
/// Samples in flight or waiting at once (a lane's edits must not queue a heap of oracle runs).
const MAX_OPEN: usize = 3;

static COUNT: AtomicU64 = AtomicU64::new(0);
/// The newest verdict group seen per live job: a verdict is counted once however often it is saved.
static SEEN: Mutex<Option<HashMap<String, (u64, String)>>> = Mutex::new(None);

/// Called with the live job after a verdict was saved. Samples it, or not; never fails the lane.
pub fn sample(ctx: &Ctx, x: &Live) {
    let Some(t) = x.patch.as_ref().and_then(|p| p.tests.as_ref()).filter(|t| !t.restarted && x.test) else { return };
    if !ctx.store.read(|d| d.devhub_shadow) { return; }
    {
        let mut seen = SEEN.lock().unwrap();
        let seen = seen.get_or_insert_with(HashMap::new);
        let key = (t.group, t.edit.clone());
        if seen.get(&x.id) == Some(&key) { return; }
        seen.insert(x.id.clone(), key);
    }
    if COUNT.fetch_add(1, Ordering::Relaxed) % EVERY != 0 { return; }
    if ctx.store.read(|d| d.jobs.iter().filter(|j| !j.live_sample.is_empty() && !j.shadow_recorded).count()) >= MAX_OPEN { return; }
    if let Err(e) = submit(ctx, x, t) {
        ctx.store.event("shadow", crate::proto::Sev::Info, format!("live shadow {}: not sampled: {e}", x.id), None);
    }
}

fn submit(ctx: &Ctx, x: &Live, t: &LiveTests) -> Result<(), String> {
    let tree = Path::new(&x.tree);
    let (rev, snap, files) = super::job::snapshot(ctx, tree, &x.lane)?;
    // the lane may have saved again since the verdict: only the exact bytes it saw are an oracle
    if !snapshot_matches(ctx, &snap, &x.sources) {
        return Err("the lane's files moved on since the verdict".into());
    }
    let cmd: Vec<String> = ["cargo", "test", "--release", "-p", &x.app].iter().map(|s| s.to_string()).chain(x.test_args.iter().cloned()).collect();
    let sample = format!("{{\"live\":{},\"group\":{},\"ran\":{},\"passed\":{},\"failed\":{},\"skipped\":{},\"ignored\":{},\"selected\":{},\"failures\":[{}]}}",
        json_str(&x.id), t.group, t.ran, t.passed, t.failed, t.skipped, t.ignored, json_str(&t.selected), t.failures.iter().map(|f| json_str(f)).collect::<Vec<_>>().join(","));
    let needs = ctx.leases.nodes().into_iter().find(|n| n.name == x.node).and_then(|n| n.caps.map(|c| format!("arch={},os={}", c.arch, c.os))).unwrap_or_default();
    ctx.store.update(|d| {
        d.jobs_next += 1;
        d.jobs.push(Job {
            id: format!("J{}", d.jobs_next), lane: x.lane.clone(), src: "tree".into(), priority: "background".into(), prio: super::leg::BACKGROUND_PRIO,
            agent: "devhub-shadow".into(), kind: "test".into(), cost: "medium".into(), class: format!("shadow-live:{}", x.app), needs,
            rev, snap, snap_files: files, repo_dir: x.tree.clone(), cmd, live_sample: sample, state: "queued".into(), submitted_ms: crate::proto::wall_ms(), ..Default::default()
        });
    });
    Ok(())
}

/// The snapshot holds the files `sources` names, with the same bytes (live hashes are `link:exec:sha256`).
fn snapshot_matches(ctx: &Ctx, snap: &str, sources: &std::collections::BTreeMap<String, String>) -> bool {
    let Ok(text) = std::fs::read_to_string(super::job::snap_dir(ctx, snap).join("manifest.json")) else { return snap.is_empty() && sources.is_empty() };
    let Ok(v) = makepad_strict_json::parse(text.as_bytes()) else { return false };
    let mut changed = std::collections::BTreeMap::new();
    for f in v.get("changed").and_then(|a| a.as_arr()).unwrap_or(&[]) {
        let (Some(p), Some(h)) = (f.get("path").and_then(|p| p.as_str()), f.get("hash").and_then(|h| h.as_str())) else { return false };
        changed.insert(p.to_string(), h.to_string());
    }
    let deleted: Vec<String> = v.get("deleted").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
    let live_files = sources.iter().filter(|(_, h)| h.as_str() != "-").count();
    live_files == changed.len()
        && sources.iter().filter(|(_, h)| h.as_str() == "-").count() == deleted.len()
        && sources.iter().all(|(p, h)| if h == "-" { deleted.contains(p) } else { changed.get(p).is_some_and(|sha| h.rsplit(':').next() == Some(sha.as_str())) })
}

/// What the libtest output of the oracle says: (passed, failed, failing test names).
pub fn native_result(log: &str) -> (u64, u64, Vec<String>) {
    let (mut passed, mut failed) = (0, 0);
    let mut names = Vec::new();
    for line in log.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("test result:") {
            let num = |word: &str| rest.split(';').find_map(|p| p.trim().trim_start_matches("ok.").trim_start_matches("FAILED.").trim().strip_suffix(word).and_then(|n| n.trim().parse::<u64>().ok())).unwrap_or(0);
            passed += num(" passed");
            failed += num(" failed");
        } else if let Some(name) = line.strip_prefix("test ").and_then(|l| l.strip_suffix(" ... FAILED")) {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    (passed, failed, names)
}

/// Agreement of a live verdict with the oracle: Some(reason) is a disagreement, None agreed;
/// the bool says the sample cannot decide (an affected-only verdict names no unaffected failure).
pub fn compare(sample: &makepad_strict_json::Value, exit: Option<i32>, log: &str) -> (Option<String>, bool) {
    let num = |k: &str| sample.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let live: Vec<String> = sample.get("failures").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|f| f.as_str().map(str::to_string)).collect();
    let (passed, failed, native) = native_result(log);
    if exit.is_none() || !log.lines().any(|l| l.starts_with("test result:")) {
        return (Some(format!("the oracle run has no verdict (exit {exit:?})")), false);
    }
    let all = sample.get("selected").and_then(|s| s.as_str()) == Some("all");
    let norm = |n: &str| n.to_string();
    let false_failures: Vec<&String> = live.iter().filter(|f| !native.iter().any(|n| norm(n) == **f)).collect();
    if !false_failures.is_empty() {
        return (Some(format!("live failed {false_failures:?}, the real checkout passed them")), false);
    }
    let missed: Vec<&String> = native.iter().filter(|n| !live.contains(n)).collect();
    if all {
        if !missed.is_empty() { return (Some(format!("the real checkout fails {missed:?}, live (all tests) did not")), false); }
        if num("passed") != passed { return (Some(format!("passed: live {} vs real {passed}", num("passed"))), false); }
        return (None, false);
    }
    if num("passed") > passed { return (Some(format!("live passed {} tests, the real checkout only {passed}", num("passed"))), false); }
    let _ = failed;
    (None, !missed.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(selected: &str, passed: u64, failures: &[&str]) -> makepad_strict_json::Value {
        let f = failures.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",");
        makepad_strict_json::parse(format!("{{\"selected\":\"{selected}\",\"passed\":{passed},\"failures\":[{f}]}}").as_bytes()).unwrap()
    }

    const LOG: &str = "running 3 tests\ntest a::ok ... ok\ntest a::bad ... FAILED\ntest a::fine ... ok\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n";

    #[test]
    fn the_oracle_log_is_read() {
        assert_eq!(native_result(LOG), (2, 1, vec!["a::bad".to_string()]));
    }

    #[test]
    fn a_full_verdict_must_equal_the_real_run() {
        assert_eq!(compare(&sample("all", 2, &["a::bad"]), Some(101), LOG), (None, false));
        assert!(compare(&sample("all", 2, &[]), Some(101), LOG).0.is_some(), "a missed failure");
        assert!(compare(&sample("all", 3, &["a::bad"]), Some(101), LOG).0.is_some(), "a passed count that differs");
        assert!(compare(&sample("all", 2, &["a::bad", "a::ok"]), Some(101), LOG).0.is_some(), "a false failure");
    }

    #[test]
    fn an_affected_verdict_cannot_name_unaffected_failures() {
        assert_eq!(compare(&sample("affected", 1, &[]), Some(101), LOG), (None, true));
        assert!(compare(&sample("affected", 1, &["a::fine"]), Some(101), LOG).0.is_some());
        assert!(compare(&sample("affected", 5, &[]), Some(101), LOG).0.is_some());
        assert!(compare(&sample("affected", 1, &[]), None, "").0.is_some(), "no verdict from the oracle");
    }
}
