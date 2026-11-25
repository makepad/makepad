//! The self-update: when a landing changes a service's code, devhub updates it on every box by
//! itself, with no rollout script. Hub updates never stop lease placement.
//! A service is affected when the landing changes a file its package's binary is built from: the
//! crates of cargo's resolved graph ([`linked_dirs`]), minus what no build reads ([`builds`]).
//! A WM service's roll starts once its code has been still for [`SETTLE_MS`] (landings come in
//! bursts); the hubs and the master take each devhub build as it ends.
//! Each roll leaves two lines in `rolls.jsonl` (`devhub rolls`): its target and why, then how it
//! ended. A hub seen at a commit its record does not name, no roll of its own under way, gets a
//! `runs` line of its own (a roll a master restart cut short, a restart whose answer was lost).
//! - the node hubs (they run apps/devhub, libs/devhub with its panel; only libs/devhub's graph rolls
//!   them, and what changed in the panel's alone comes along with the next roll): their target is
//!   the newest devhub build the master completed (its
//!   per-landing CLI build, [`prebuild`]: never a tip that did not build, never older than a hub
//!   runs, the same for every box), so they never wait for the master's own restart (Mac hubs lagged
//!   the deployment for its 30 min uptime, 11-03). A hub newer than the master works with it; a
//!   request needing a feature its hub lacks waits for that feature by name. The deployment gets
//!   the code (the auto-sync, kicked; a push for the boxes outside it, which a timed run there makes
//!   wait: an update wait, said once) while leases keep using the work tree. Once `caps.deploy` has
//!   the code, the master takes the room of its build there (a lease of class `hub-build`, granted
//!   in its turn beside the box's work, held until the hub restarts) and asks for `hub-restart`; the hub
//!   builds its new self first (`waits:` meanwhile) and refuses a deployment it does not build at
//!   (said once per commit, loudly; the box is not asked again until its deployment moves), then
//!   restarts at once: a restart ends nothing (runs and live apps go on,
//!   their wrappers record exits, the new hub takes leases, runs and answered starts back from its
//!   ledger; a dev JIT store build's cargo goes on in its own scope and the new hub waits for it,
//!   lease/jitstore.rs; holders see a reconnect). Only a sync apply or an in-hub selftest makes it wait
//!   seconds (Windows: its runs, job objects that end with the hub). The hub exits and what keeps
//!   it (the WM's supervisor, the Mac's LaunchAgent) starts it again through `cargo run`. Its caps
//!   then name the commit it runs (`devhub nodes`: `hub <sha>`). A hub without `handover`
//!   restarts only between runs: its box takes no new lease until it has (once per box).
//!   Only requests needing a feature it lacks wait for it. A hub from
//!   before the self-update needs one restart by hand (it is said once).
//! - the services the WM supervises (the AI hub): `service-restart` once `caps.deploy` has the code
//!   and the node's apply has built it, only when the hub does not run that build already (a
//!   restart reloads its models; `current` answers)
//!   (SIGTERM: it hands its jobs back; the WM starts it again through `cargo run`), never while
//!   someone watches the box's screen ([`crate::lease::exec::user_watching`]) or an app holds
//!   unsaved work: the roll waits until neither holds. The hub itself restarts regardless (the
//!   user never sees it); a WM restart waits the same way on the node
//!   ([`crate::lease::exec::user_blockers`]).
//! - the master itself, never held up by the node rolls and ending nothing: what runs under a lease
//!   lives on the node, and the new master takes it back (jobs, live jobs, verify legs and CI
//!   shards go on from where their logs stand; queued ones stay queued). Only an allapps run is
//!   started over, so it is waited for up to [`MASTER_DEADLINE`]. It execs with its own args into
//!   each devhub build it lacks as that build ends ([`prebuild`]: devhub's own small binary,
//!   never the JIT, built apart from `target/host` ([`build_devhub`]) and swapped in at
//!   `target/host/makepad-devhub` as the master execs into it, so the lanes' CLI moves with it
//!   ([`swap_cli`]); a master that runs as the devhub app, the
//!   one with a menu-bar item, execs into the app's build: [`MasterBin`]), before the next landing's build starts: a
//!   landing during a build is the next one's, and the master takes it as that ends (the node rolls
//!   start over from the new master). The exec waits for none of the master's own work: its
//!   leases, jobs, live sessions, warm legs (orch/warm.rs) and the boxes' deployment builds
//!   ([`follow_rooms`]) go on under the new master. `devhub master restart` builds work's tip
//!   first: a build that fails keeps it running and is an inbox error (it waits for the build and
//!   answers with it; it never waits out the self-update's retry pause).
//! - the WM (with its window handover, never over unsaved work) and the dock apps (patched live,
//!   or a restart offered) are the auto-sync's apply (`restart=auto`) and live deploy.
//!
//! Every step is an `update` event.

use super::{git, Ctx};
use crate::lease::sched::{deploy_to, DeployTo};
use crate::proto::{wall_ms, Sev};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// User-facing service rolls started at once. Hub requests need no slot.
pub const AT_ONCE: usize = 1;
const TICK: Duration = Duration::from_secs(15);
/// How often a roll that waits for a box's user looks again.
const USER_WAIT: Duration = Duration::from_secs(30);
/// An update waits this long before it says so (it keeps waiting: no lease is ended).
const DRAIN_SAY: Duration = Duration::from_secs(1800);

/// A service roll waiting this long gives its slot back: one busy box cannot hold the fleet.
const SLOT_HOLD: Duration = Duration::from_secs(300);

/// One of the [`AT_ONCE`] service rolls, held until it ends (or [`SLOT_HOLD`]).
struct Slot {
    drains: Arc<AtomicUsize>,
    held: bool,
}

impl Slot {
    fn try_take(&mut self) -> bool {
        let mut n = self.drains.load(Ordering::Acquire);
        while !self.held && n < AT_ONCE {
            match self.drains.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => self.held = true,
                Err(now) => n = now,
            }
        }
        self.held
    }

    fn give_back(&mut self) {
        if std::mem::take(&mut self.held) {
            self.drains.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn take(&mut self) {
        while !self.try_take() {
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.held {
            self.drains.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// The services the self-update restarts on nodes: (name, the package whose graph rolls it, the
/// features a node builds it with). The WM builds the AI hub with `--features node` (its
/// NodeConf's default), whose graph holds crates the default one does not (libs/render, scene,
/// the model families): a landing in libs/render rebuilt every Linux box's AI hub, and the roll,
/// reading the default graph, never restarted one (11-22).
pub const SERVICES: [(&str, &str, &str); 2] = [("devhub", "libs/devhub", ""), ("ai-hub", "apps/aihub", "node")];

/// The repo dirs of the crates the binary of the package at `pkg` is built from: cargo's resolved
/// graph, for every target. Reading `path` lines is not enough for this: an optional dependency
/// no feature turns on reads as one (libs/ai/hub's took platform, draw and 100 more crates into
/// libs/devhub's closure, 157 crates to the 48 it builds).
fn linked_dirs(repo: &Path, pkg: &str, features: &str) -> Result<BTreeSet<String>, String> {
    // (no --locked: the lockfile is not tracked; each checkout's own goes stale with a landing's new
    // crate, and cargo refreshes it here as any build in this checkout would)
    let o = std::process::Command::new(cargo_bin()).current_dir(repo).args(["tree", "--offline", "-e", "normal,build", "--prefix", "none", "--target", "all", "--manifest-path"]).arg(format!("{pkg}/Cargo.toml")).args(if features.is_empty() { Vec::new() } else { vec!["--features", features] }).output().map_err(|e| format!("cargo: {e}"))?;
    if !o.status.success() {
        return Err(crate::proto::clean(&String::from_utf8_lossy(&o.stderr), 300));
    }
    let root = repo.canonicalize().map_err(|e| format!("{}: {e}", repo.display()))?;
    Ok(tree_dirs(&String::from_utf8_lossy(&o.stdout), &root.to_string_lossy(), std::path::MAIN_SEPARATOR))
}

/// The crates under `root` of a `cargo tree --prefix none` listing, as repo paths. `sep` is the
/// platform's: cargo prints a Windows path with `\`, where `root` canonicalizes to a verbatim
/// `\\?\C:\` path cargo never prints.
fn tree_dirs(listing: &str, root: &str, sep: char) -> BTreeSet<String> {
    let slashed = |s: &str| if sep == '/' { s.to_string() } else { s.replace(sep, "/") };
    let root = slashed(root);
    let root = root.strip_prefix("//?/").filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(&root);
    let marker = format!(" ({}/", root.trim_end_matches('/'));
    listing.lines().filter_map(|l| {
        let l = slashed(l);
        let rest = &l[l.find(&marker)? + marker.len()..];
        Some(rest[..rest.find(')')?].to_string())
    }).collect()
}

/// Whether a build reads this file of a crate (its path below the crate's dir): a CI proof's stamp,
/// the crate's tests and its notes are no code a service runs.
fn builds(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name != "ci.toml" && !name.ends_with(".md") && !["tests/", "benches/", "examples/"].iter().any(|d| file.starts_with(d))
}

/// What the commits from `from` to `to` change of what `dirs` build, as repo paths ("" in `dirs`:
/// the whole repo, [`service_dirs`]). None: a commit is not known here.
fn built_changes(repo: &Path, from: &str, to: &str, dirs: &BTreeSet<String>) -> Option<Vec<String>> {
    let changed = crate::gitread::files_differing_under(repo, from, to, dirs.iter().map(String::as_str))?;
    Some(changed.into_iter().filter(|(_, file)| builds(file)).map(|(dir, file)| if dir.is_empty() { file } else { format!("{dir}/{file}") }).collect())
}

/// Each service's crates at this landing, and why they are the whole repo. A landing whose graph
/// cargo cannot read rolls every service: a missed update is the worse error.
fn service_dirs(repo: &Path) -> (HashMap<String, BTreeSet<String>>, Option<String>) {
    match SERVICES.iter().map(|(n, d, f)| Ok((n.to_string(), linked_dirs(repo, d, f)?))).collect::<Result<HashMap<_, _>, String>>() {
        Ok(dirs) => (dirs, None),
        Err(e) => (SERVICES.iter().map(|(n, _, _)| (n.to_string(), BTreeSet::from([String::new()]))).collect(), Some(format!("cargo does not read the services' crates ({e}): the whole repo counts"))),
    }
}

/// `rolls.jsonl` in devhub's home.
fn rolls_path() -> PathBuf {
    crate::proto::home().join("rolls.jsonl")
}

/// Past this `rolls.jsonl` is cut to half of it when a master starts: over a week of rolls at
/// 100 a day on 10 boxes.
const ROLLS_KEEP: usize = 8 << 20;

/// The tail of a record that grew past [`ROLLS_KEEP`], from a whole line.
fn rolls_kept(text: &str) -> Option<&str> {
    if text.len() <= ROLLS_KEEP {
        return None;
    }
    let mut at = text.len() - ROLLS_KEEP / 2;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    Some(text[at..].split_once('\n').map_or("", |(_, whole)| whole))
}

/// What `rolls.jsonl` says of a change: its count and the first few files.
fn why_of(changed: &[String]) -> String {
    const NAMED: usize = 3;
    let mut why = format!("{} file{}: {}", changed.len(), if changed.len() == 1 { "" } else { "s" }, changed.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", "));
    if changed.len() > NAMED {
        why.push_str(&format!(" +{}", changed.len() - NAMED));
    }
    why
}

/// A line of the roll's record, kept when a master restarts in the middle of a roll or a box is
/// gone for good. `fields`: the line's own, by name.
fn note_roll(node: &str, svc: &str, fields: &[(&str, &str)]) {
    use std::io::Write;
    let rest: String = fields.iter().map(|(k, v)| format!(",{}:{}", makepad_remote::json_str(k), makepad_remote::json_str(v))).collect();
    let line = format!("{{\"ms\":{},\"node\":{},\"svc\":{}{rest}}}\n", wall_ms(), makepad_remote::json_str(node), makepad_remote::json_str(svc));
    let wrote = std::fs::OpenOptions::new().create(true).append(true).open(rolls_path()).and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = wrote {
        eprintln!("devhub: {}: {e}", rolls_path().display());
    }
}

/// A box's WM restart as its hub said it (`sync: WM restarted (<service> <state>): <why>, after
/// <how long> waiting on: <what>`, or `with nothing to wait for`): a `wm` line of the record, so
/// `devhub rolls` shows each WM restart and its wait. Only a wait's start was kept, an inbox line,
/// and a box's WM was read as 37 min behind while each restart there had waited 3-92 s (11-24).
pub fn on_record(r: &crate::proto::Record) {
    if let Some(ended) = wm_restarted(r) {
        note_roll(&r.machine, "wm", &[("ended", &ended)]);
    }
}

/// [`on_record`]'s line of a hub's WM restart record: what follows "WM restarted".
fn wm_restarted(r: &crate::proto::Record) -> Option<String> {
    (r.kind == "sync").then_some(())?;
    r.line.strip_prefix("sync: WM restarted ").map(|rest| format!("restarted {rest}"))
}

/// The commit each box's hub runs by the record: the short sha of its newest devhub `runs` line.
fn hubs_run(text: &str) -> HashMap<String, String> {
    let mut at = HashMap::new();
    for l in text.lines().filter(|l| l.contains("\"ended\":\"runs ")) {
        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
        let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if let Some(sha) = field("ended").strip_prefix("runs ").filter(|_| field("svc") == "devhub") {
            at.insert(field("node"), sha.to_string());
        }
    }
    at
}

/// Whether a hub at `rev` is one the record names `known` (a short sha, or nothing yet).
fn runs_known(known: &str, rev: &str) -> bool {
    rev.is_empty() || (!known.is_empty() && rev.starts_with(known))
}

/// `node`'s hub runs `rev` with no roll of its own under way: a `runs` line when the record
/// ([`hubs_run`]) has it elsewhere.
fn note_runs(runs: &Mutex<HashMap<String, String>>, node: &str, rev: &str) {
    let mut runs = runs.lock().unwrap();
    let known = runs.get(node).cloned().unwrap_or_default();
    if runs_known(&known, rev) {
        return;
    }
    note_roll(node, "devhub", &[("from", &known), ("to", rev), ("ended", &format!("runs {}", super::short(rev)))]);
    runs.insert(node.to_string(), super::short(rev).to_string());
}

/// `devhub rolls [--node N] [n]`.
pub fn rolls_cli(args: &[&str]) -> i32 {
    let at = args.iter().position(|a| *a == "--node");
    let node = at.and_then(|i| args.get(i + 1)).copied();
    let n = args.iter().enumerate().filter(|(i, _)| at.is_none_or(|a| *i != a + 1)).find_map(|(_, a)| a.parse().ok()).unwrap_or(30);
    match std::fs::read_to_string(rolls_path()) {
        Ok(text) => {
            println!("{}", rolls_text(&text, node, n, &super::stamp));
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no rolls yet");
            0
        }
        Err(e) => {
            eprintln!("makepad devhub rolls: {}: {e}", rolls_path().display());
            2
        }
    }
}

/// The newest `n` lines of the record, oldest first.
fn rolls_text(text: &str, node: Option<&str>, n: usize, clock: &dyn Fn(u64) -> String) -> String {
    let lines: Vec<String> = text.lines().filter_map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
        let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if node.is_some_and(|w| w != field("node")) {
            return None;
        }
        let short = |k: &str| { let f = field(k); f[..f.len().min(10)].to_string() };
        let at = clock(v.get("ms").and_then(|x| x.as_u64()).unwrap_or(0));
        Some(match (field("ended").as_str(), field("to").is_empty()) {
            ("", _) => format!("{at} {} {}: {} -> {}: {}", field("node"), field("svc"), short("from"), short("to"), field("why")),
            // a WM restart ([`on_record`]): no commit of its own
            (ended, true) => format!("{at} {} {}: {ended}", field("node"), field("svc")),
            (ended, false) => format!("{at} {} {}: {} ended: {ended}", field("node"), field("svc"), short("to")),
        })
    }).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The repo dirs of `pkg` and every crate it depends on by path (workspace deps resolved through
/// the root Cargo.toml), relative to `repo`.
pub fn crate_dirs(repo: &Path, pkg: &str) -> BTreeSet<String> {
    crate_dirs_from(pkg, &|p| std::fs::read_to_string(repo.join(p)).ok())
}

/// The same closure in a git tree, without moving a checkout.
pub fn crate_dirs_from(pkg: &str, read: &dyn Fn(&str) -> Option<String>) -> BTreeSet<String> {
    let root = read("Cargo.toml").unwrap_or_default();
    let mut ws: HashMap<String, String> = HashMap::new();
    let mut in_ws = false;
    for l in root.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_ws = t == "[workspace.dependencies]";
            continue;
        }
        if in_ws {
            if let (Some((name, _)), Some(p)) = (t.split_once('='), path_of(t)) {
                ws.insert(name.trim().to_string(), p);
            }
        }
    }
    let mut out = BTreeSet::new();
    let mut todo = vec![pkg.trim_end_matches('/').to_string()];
    while let Some(dir) = todo.pop() {
        if !out.insert(dir.clone()) {
            continue;
        }
        let text = read(&format!("{dir}/Cargo.toml")).unwrap_or_default();
        for l in text.lines() {
            let t = l.trim();
            let rel = if let Some(p) = path_of(t) {
                Some(normalize(&format!("{dir}/{p}")))
            } else if t.contains("workspace = true") || t.contains("workspace=true") {
                // `x = { workspace = true }`, or `x.workspace = true`
                t.split_once('=').and_then(|(n, _)| ws.get(n.trim().trim_end_matches(".workspace").trim_end()).cloned())
            } else {
                None
            };
            if let Some(r) = rel.filter(|r| read(&format!("{r}/Cargo.toml")).is_some()) {
                todo.push(r);
            }
        }
    }
    out
}

/// `path = "x"` in a toml line: the key, not a `path` in a dependency's name. The first `path`
/// of `makepad-pathkit = { path = "../pathkit" }` is the name's, so no crate that used it held it
/// among its inputs: their proofs held across a change to it, and a bucket named only an
/// unrelated landing for a call that change broke (11-07).
fn path_of(line: &str) -> Option<String> {
    line.match_indices("path").find_map(|(i, _)| {
        if !line[..i].chars().next_back().is_none_or(|c| c == '{' || c == ',' || c.is_whitespace()) { return None; }
        let rest = line[i + 4..].trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
        Some(rest[..rest.find('"')?].to_string())
    })
}

/// `a/b/../c` -> `a/c`.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.join("/")
}

/// Whether a tree at `at` lacks the code of `to` in `dirs`: false when `at` contains `to` (a later
/// landing moved it past) or differs from it nowhere in `dirs`.
/// Whether a hub at `hub_rev` lacks code of the deployment its box has (`deploy`): built there, it
/// runs that code while the deployment cannot move on ([`roll_node`]: held for the timed ask first
/// in line). A Mac's hub ran ee658cc475 for over an hour while its deployment stood at 2549ec0082,
/// held for a timed ask that waited 25 min to start (11-24). An older hub (no `hub_rev`) is not one.
fn hub_behind_deployment(repo: &Path, hub_rev: &str, deploy: &str, dirs: &BTreeSet<String>) -> bool {
    !hub_rev.is_empty() && !deploy.is_empty() && lacks(repo, hub_rev, deploy, dirs)
}

fn lacks(repo: &Path, at: &str, to: &str, dirs: &BTreeSet<String>) -> bool {
    if at.is_empty() {
        return true;
    }
    if crate::gitread::is_ancestor(repo, to, at) {
        return false;
    }
    touches(repo, at, to, dirs)
}

/// Whether the commits from `from` to `to` change anything `dirs` build.
fn touches(repo: &Path, from: &str, to: &str, dirs: &BTreeSet<String>) -> bool {
    if from == to {
        return false;
    }
    // (an unknown commit counts as a change)
    built_changes(repo, from, to, dirs).is_none_or(|c| !c.is_empty())
}

/// When `to`'s line (its first parents: the landings) last took a change to `dirs`, as wall ms: the
/// landing's commit time, not its lane commit's. A lane's commit is older than its landing by as
/// long as the lane took to hand it in (16 min for the AI hub's last change at ce27b321b7, 11-22),
/// and a service started in between ran without it while it counted as started after it, so the
/// roll never asked. The answer for one commit never changes: asked once per landing and dir set,
/// not per tick.
fn last_change_ms(repo: &Path, to: &str, dirs: &BTreeSet<String>) -> u64 {
    static KNOWN: Mutex<Option<(String, HashMap<BTreeSet<String>, u64>)>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref().is_none_or(|(at, _)| at != to) {
        *known = Some((to.to_string(), HashMap::new()));
    }
    let Some((_, by_dirs)) = known.as_mut() else { return 0 };
    if let Some(ms) = by_dirs.get(dirs) {
        return *ms;
    }
    let mut args = vec!["log", "-1", "--first-parent", "--format=%ct", to, "--"];
    args.extend(dirs.iter().map(|d| if d.is_empty() { "." } else { d.as_str() }));
    let ms = git(repo, &args).ok().and_then(|s| s.trim().parse::<u64>().ok()).map_or(0, |s| s * 1000);
    by_dirs.insert(dirs.clone(), ms);
    ms
}

/// Whether a WM-supervised service that started at `started` (wall ms) rolls: its code changed
/// after it started (at `at`, the last landing of its closure) and has been still for
/// [`SETTLE_MS`], or its node says it runs a build its checkout has since replaced (`rebuilt`,
/// [`crate::lease::Caps::services_stale`]), whenever it started; not once its node said it runs
/// the deployment's build for that change (`asked`). Started after the landing but before its
/// box's deployment had it, a Linux node's AI hub ran its old build 1 h: its start was later than the
/// change, so no roll came (11-23).
fn service_due(started: u64, at: u64, rebuilt: bool, asked: bool, now: u64) -> bool {
    !asked && (rebuilt || started < at && at + SETTLE_MS <= now)
}

/// An update event, also in the master's log: its feed keeps none past its readers, and a restart
/// that waited 6 and 8 min for its drain left nothing to read it by (11-15).
fn event(ctx: &Ctx, sev: Sev, line: String) {
    eprintln!("{line}");
    ctx.store.event("update", sev, line, None);
}

pub fn start(ctx: Arc<Ctx>) {
    follow_rooms(&ctx);
    let record = std::fs::read_to_string(rolls_path()).unwrap_or_default();
    if let Some(kept) = rolls_kept(&record) {
        if let Err(e) = std::fs::write(rolls_path(), kept) {
            event(&ctx, Sev::Warn, format!("update: {}: {e}", rolls_path().display()));
        }
    }
    // node -> the commit its hub runs by the record: one seen running another, with no roll of
    // its own under way, gets its `runs` line (a roll that a master restart cut short, a restart
    // whose answer was lost, its WM's: two hubs moved with no line, 11-10)
    let runs = Arc::new(Mutex::new(hubs_run(&record)));
    drop(record);
    // the hubs' target: the commit this master was built at, read now (lazily it would be
    // whatever the checkout moved to since)
    let _ = crate::lease::caps::hub_rev();
    let _ = std::thread::Builder::new().name("devhub-update".into()).spawn(move || {
        let busy: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        // node -> the deployment its new hub did not build at: no roll again until it moves
        let broken: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        // node -> the deployment its node refused to move from: no roll of any service there until
        // its tree changes (a refused apply is refused the same way every time)
        let refused: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        // (box, service) that said it runs the deployment's build already, at which last change and
        // deployment: asked again when one of them moves, not every tick
        let current: Arc<Mutex<HashMap<(String, String), (u64, String)>>> = Arc::new(Mutex::new(HashMap::new()));
        let drains = Arc::new(AtomicUsize::new(0));
        // a self-update whose exec failed is said loudly and tried again later, not every tick;
        // true: the exec follows in a moment
        let (mut master_next, mut master_wait) = (None::<Instant>, MASTER_RETRY);
        // what this master runs as, and so builds and execs into ([`MasterBin`])
        let bin = MasterBin::this();
        // the known bad build last passed over, said once
        let mut passed_over = String::new();
        let mut update_master = |good: &str| {
            if master_next.is_some_and(|at| Instant::now() < at) {
                return false;
            }
            // never back onto a build that panicked on its probation or that a rollback left
            if let Some(why) = bad_build(&ctx.home, good) {
                if passed_over != good {
                    passed_over = good.to_string();
                    event(&ctx, Sev::Info, format!("update: the master does not take {}, a build known bad ({why}): it takes the next landing's", super::short(good)));
                }
                return false;
            }
            if restart_master(&ctx, good, "its self-update", false).is_ok() {
                return true;
            }
            // said loudly (inbox); it keeps running and tries again later, not every tick
            event(&ctx, Sev::Info, format!("update: the master tries {} again in {} min (or `devhub master restart`)", super::short(good), master_wait.as_secs() / 60));
            master_next = Some(Instant::now() + master_wait);
            master_wait = (master_wait * 2).min(MASTER_RETRY_MAX);
            false
        };
        let mut said: BTreeSet<String> = BTreeSet::new();
        let mut closures: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut closures_at = String::new();
        // why this landing's closures are the whole repo
        let mut closures_why: Option<String> = None;
        let mut wait_since: Option<Instant> = None;
        // the last tip whose no-restart was said
        let mut quiet_at = String::new();
        let mut fds_said = 1000;
        // the commit the CLI binary was last built at (this master's build at first), and the
        // newest one that built
        let mine = crate::lease::caps::hub_rev().to_string();
        let mut cli_at = cli_built_at(bin, &mine, std::env::var_os(crate::lease::caps::BUILT_AT).is_some());
        *CLI_GOOD.lock().unwrap() = mine.clone();
        // the same for the app this master runs as, if it does
        let mut app_at = mine.clone();
        *APP_GOOD.lock().unwrap() = mine;
        let mut tips_at = String::new();
        // the boxes up at the last look (None: no look yet), for [`back_behind`]
        let mut was_up: Option<BTreeSet<String>> = None;
        // the private checkout's commit the land-rule binary was last looked at for, and its last build try
        let (mut gate_seen, mut gate_try) = (String::new(), None);
        loop {
            std::thread::sleep(TICK);
            note_fds(&ctx, &mut fds_said);
            if RESTART_ASKED.swap(false, std::sync::atomic::Ordering::AcqRel) && update_master(&master_good(bin)) {
                continue;
            }
            let Some(work) = crate::gitread::rev(&ctx.repo, "refs/heads/work") else { continue };
            // where a box catches up to, and how far behind one is
            if let Some(main) = crate::gitread::rev(&ctx.repo.join("apps/commercial"), "refs/heads/main") {
                // every followed repository's: a catch-up moves only what its pin names
                let tips = crate::nested::tips_pin(&ctx.repo, &work, &main).key();
                if tips != tips_at {
                    ctx.leases.set_tips(&tips, landed_since(&ctx, wall_ms().saturating_sub(LANDINGS_KNOWN.as_millis() as u64)));
                    tips_at = tips;
                }
            }
            if closures_at != work {
                (closures, closures_why) = service_dirs(&ctx.repo);
                closures_at = work.clone();
                if let Some(why) = &closures_why {
                    event(&ctx, Sev::Warn, format!("update: {} at {why}", super::short(&work)));
                }
            }
            // the master itself, before the next landing's build starts: it execs into each devhub
            // build it lacks as that build ends. It builds nothing of its own, and neither a settle
            // nor an uptime holds it (11-08: its restart build began a minute before a landing, and
            // the next one waited out 10 min of uptime): a landing during a build is the next
            // build's, taken as that one ends. The restart ends nothing, so only an allapps run
            // (for a while) and a lane sweep hold it. Never held up by node rolls either: one waits
            // for a long lease's run boundary, landings keep coming, and a master that waited for
            // them all never updated (18:18 → 19:05, 2026-10-30). A roll it cuts short starts over
            // from the new master.
            let mine = crate::lease::caps::hub_rev();
            let building = CLI_BUILDING.load(Ordering::Acquire);
            let good = master_good(bin);
            // (the master is libs/devhub's own binary, or the devhub app around it)
            let master_dirs = bin.dirs(&closures["devhub"]);
            if !mine.is_empty() && touches(&ctx.repo, mine, &good, &master_dirs) {
                let since = *wait_since.get_or_insert_with(Instant::now);
                let allapps = ctx.store.read(|d| d.allapps.running.is_some());
                match restart_blocker_of(building, super::lane::sweeping(), allapps, crate::serial::master_flashing(), since.elapsed()) {
                    Some(why) => {
                        if said.insert(format!("master {good} {}", why.split(' ').take(2).collect::<Vec<_>>().join(" "))) {
                            event(&ctx, Sev::Info, format!("update: the master's restart on {} waits (stalled: {why})", super::short(&good)));
                        }
                    }
                    None if update_master(&good) => {
                        wait_since = None;
                        continue;
                    }
                    None => {}
                }
            } else {
                wait_since = None;
                // a landing that changed nothing the master builds says so, once: no restart is
                // no stall (11-18: twelve landings of other crates read as a stalled self-update)
                // (looked at once per tip)
                if !mine.is_empty() && work != quiet_at {
                    quiet_at = work.clone();
                    if !work.starts_with(mine) && !touches(&ctx.repo, mine, &work, &master_dirs) {
                        event(&ctx, Sev::Info, format!("update: nothing devhub builds changed at {} since {}: no restart", super::short(&work), super::short(mine)));
                    }
                }
            }
            prebuild(&ctx, &work, &closures["devhub"], (bin == MasterBin::App).then_some(&master_dirs), &mut cli_at, &mut app_at);
            prebuild_gate(&ctx, &mut gate_seen, &mut gate_try);
            // the hubs' target: the newest devhub build the master completed
            let good = cli_good();
            // each service rolls once its own code settled: the hub's frequent landings never hold
            // back an AI hub whose code is still (11-01: the fleet's AI hubs ran 1 h old code)
            let changed: HashMap<&str, u64> = SERVICES.iter().skip(1).map(|(s, _, _)| (*s, last_change_ms(&ctx.repo, &work, &closures[*s]))).collect();
            push_back_behind(&ctx, &tips_at, &busy, &mut was_up);
            // never a tip that did not build (11-02: hubs rolled onto a landing that broke
            // makepad-devhub, which the master's build had refused minutes before)
            for n in ctx.leases.nodes().into_iter().filter(|n| n.up && !matches!(n.role(), crate::lease::Role::Device)) {
                let Some(c) = n.caps.clone() else { continue };
                if busy.lock().unwrap().contains(&n.name) {
                    continue;
                }
                note_runs(&runs, &n.name, &c.hub_rev);
                if refused.lock().unwrap().get(&n.name).map(String::as_str) == Some(c.deploy.split(',').next().unwrap_or("")) {
                    continue;
                }
                // the hub
                let hub = &closures["devhub"];
                let lags = !c.hub_rev.is_empty()
                    && !good.is_empty()
                    && !crate::gitread::is_ancestor(&ctx.repo, &good, &c.hub_rev)
                    && touches(&ctx.repo, &c.hub_rev, &good, hub)
                    && broken.lock().unwrap().get(&n.name).map(String::as_str) != Some(c.deploy.split(',').next().unwrap_or(""));
                let job = if c.hub_rev.is_empty() {
                    if said.insert(format!("old {}", n.name)) {
                        event(&ctx, Sev::Warn, format!("update: {}'s hub is from before the self-update: restart it once by hand (then devhub updates it by itself)", n.name));
                    }
                    None
                } else if lags {
                    Some(("devhub", hub.clone()))
                } else {
                    // the WM-supervised services that run there and started before their code's last change
                    SERVICES.iter().skip(1).find_map(|(s, _, _)| {
                        let started = c.services.iter().find(|(x, _)| x == s)?.1;
                        let dirs = &closures[*s];
                        let at = changed[*s];
                        let asked = current.lock().unwrap().get(&(n.name.clone(), s.to_string())) == Some(&(at, c.deploy.clone()));
                        let rebuilt = c.services_stale.iter().any(|x| x == s);
                        service_due(started, at, rebuilt, asked, wall_ms()).then(|| (*s, dirs.clone()))
                    })
                };
                let Some((svc, dirs)) = job else { continue };
                busy.lock().unwrap().insert(n.name.clone());
                let target = if svc == "devhub" { good.clone() } else { work.clone() };
                let from = if svc == "devhub" { c.hub_rev.clone() } else { String::new() };
                let why = if svc == "devhub" {
                    built_changes(&ctx.repo, &from, &target, &dirs).map_or("the commit it runs is not known here".to_string(), |c| why_of(&c))
                } else if c.services_stale.iter().any(|x| x == svc) {
                    "its checkout rebuilt it after it started".to_string()
                } else {
                    "its code changed after it started".to_string()
                };
                let why = closures_why.as_ref().map_or(why.clone(), |c| format!("{c}: {why}"));
                let (c2, b2, d2, node, br, rf, cur) = (ctx.clone(), busy.clone(), drains.clone(), n.name.clone(), broken.clone(), refused.clone(), current.clone());
                let (at, deploy, rn) = (changed.get(svc).copied().unwrap_or(0), c.deploy.clone(), runs.clone());
                let _ = std::thread::Builder::new().name("devhub-update-node".into()).spawn(move || {
                    let end = roll_node(&c2, &node, svc, &dirs, &target, d2, &|| note_roll(&node, svc, &[("from", &from), ("to", &target), ("why", &why)]));
                    let now = c2.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps).map_or(String::new(), |c| c.hub_rev);
                    let ended = match &end {
                        RollEnd::Broke(at) => Some(format!("its new hub does not build at {}; it runs on at {}", super::short(at), super::short(&now))),
                        RollEnd::Back if svc == "devhub" => Some(format!("runs {}", super::short(&now))),
                        RollEnd::Back => Some("restarted".to_string()),
                        RollEnd::Late => Some("did not come back in 25 min".to_string()),
                        RollEnd::Refused(_) => Some("its node refused the deployment".to_string()),
                        RollEnd::Current | RollEnd::NoRestart => None,
                    };
                    if let Some(ended) = ended {
                        note_roll(&node, svc, &[("to", &target), ("ended", &ended)]);
                    }
                    // (before the box leaves `busy`: its next look finds the record current)
                    if svc == "devhub" && matches!(end, RollEnd::Back) {
                        rn.lock().unwrap().insert(node.clone(), super::short(&now).to_string());
                    }
                    if matches!(end, RollEnd::Current) {
                        cur.lock().unwrap().insert((node.clone(), svc.to_string()), (at, deploy));
                    }
                    match &end {
                        RollEnd::Broke(at) => br.lock().unwrap().insert(node.clone(), at.clone()),
                        _ => br.lock().unwrap().remove(&node),
                    };
                    match end {
                        RollEnd::Refused(at) => rf.lock().unwrap().insert(node.clone(), at),
                        _ => rf.lock().unwrap().remove(&node),
                    };
                    c2.leases.set_updating(&node, None, false);
                    b2.lock().unwrap().remove(&node);
                });
            }
        }
    });
}

/// A devhub build runs now ([`prebuild`]: the CLI, and the app a master runs as).
static CLI_BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The newest commit whose devhub the master built (or whose devhub code is that of one it
/// built): the hubs' target.
static CLI_GOOD: Mutex<String> = Mutex::new(String::new());

fn cli_good() -> String {
    CLI_GOOD.lock().unwrap().clone()
}

/// What an app master's first look takes the CLI binary to be built at when nothing says
/// ([`cli_built_at`]): no commit, so that look builds it (cargo's no-op when it is current).
const CLI_UNKNOWN: &str = "unknown";

/// The commit the CLI binary at `target/host` was built at as a master starts on `mine`: its own,
/// when it is that binary or its own restart built it (`restarted`: the exec's [`BUILT_AT`]
/// says so, and [`prebuild`] builds the CLI before the app); the app's login-item wrapper builds
/// only the app, so a master it started does not know ([`CLI_UNKNOWN`]).
///
/// [`BUILT_AT`]: crate::lease::caps::BUILT_AT
fn cli_built_at(bin: MasterBin, mine: &str, restarted: bool) -> String {
    match bin {
        MasterBin::App if !restarted => CLI_UNKNOWN.to_string(),
        _ => mine.to_string(),
    }
}

/// The newest commit whose devhub app the master built ([`build_master_app`]), when it runs as
/// that app ([`MasterBin::App`]): its own target. (The hubs' stays [`CLI_GOOD`].)
static APP_GOOD: Mutex<String> = Mutex::new(String::new());

/// The newest build of what this master runs as: its target.
fn master_good(bin: MasterBin) -> String {
    match bin {
        MasterBin::Cli => cli_good(),
        MasterBin::App => APP_GOOD.lock().unwrap().clone(),
    }
}

/// A commit that changes none of the master's code since `from` keeps `from`'s build: when `from`
/// built, `to` did.
fn carry_good(good: &mut String, from: &str, to: &str) {
    if good == from {
        *good = to.to_string();
    }
}

/// Whether a build at `work` is due for what was last built at `at` from `dirs`: none when `work`
/// changes nothing of them (`at` moves on, and `good` with it when `at` built).
fn due(repo: &Path, work: &str, dirs: &BTreeSet<String>, at: &mut String, good: &Mutex<String>) -> bool {
    if *at == work {
        return false;
    }
    if at.is_empty() || !touches(repo, at, work, dirs) {
        carry_good(&mut good.lock().unwrap(), at, work);
        *at = work.to_string();
        return false;
    }
    *at = work.to_string();
    true
}

/// Lanes call the built binary (`target/host/makepad-devhub`, through `makepad devhub`): right
/// after a landing that changes its inputs the master builds it once, a background build on 4
/// jobs ([`build_devhub`]), and swaps it in as it execs into that build ([`restart_master`],
/// [`swap_cli`]): a landing's CLI goes live with its master, never ahead of it. Swapped in as its
/// build ended, the CLI of a landing asked the master that still ran the tip before it for a new
/// request (`stats route`) on every lane's `devhub test` for 8 min (11-15).
/// Landings during a build get one more after it. A build that succeeds is the hubs' target
/// ([`CLI_GOOD`]). A master that runs as the devhub app (`app_dirs`: what it rolls for, libs/devhub's
/// graph and the app's own crate; the app's other crates come along with the next one) builds the
/// app after it on the same thread, one cargo at a time: its own target ([`APP_GOOD`]).
fn prebuild(ctx: &Arc<Ctx>, work: &str, dirs: &BTreeSet<String>, app_dirs: Option<&BTreeSet<String>>, at: &mut String, app_at: &mut String) {
    if CLI_BUILDING.load(Ordering::Acquire) {
        return;
    }
    let cli = due(&ctx.repo, work, dirs, at, &CLI_GOOD);
    let app = app_dirs.is_some_and(|d| due(&ctx.repo, work, d, app_at, &APP_GOOD));
    if !cli && !app {
        return;
    }
    CLI_BUILDING.store(true, Ordering::Release);
    let (ctx, work) = (ctx.clone(), work.to_string());
    let spawned = std::thread::Builder::new().name("devhub-cli-build".into()).spawn(move || {
        if cli {
            let t = Instant::now();
            match build_devhub(&ctx.repo, Some(&work), rule_builds(&ctx), &|l| event(&ctx, Sev::Warn, l)) {
                Ok(()) => {
                    *CLI_GOOD.lock().unwrap() = work.clone();
                    event(&ctx, Sev::Info, format!("update: devhub's CLI is built at {} in {:.0} s; the hubs roll to it, the lanes' CLI moves with the master's restart onto it", super::short(&work), t.elapsed().as_secs_f64()));
                }
                Err(e) => event(&ctx, Sev::Warn, format!("update: devhub's CLI at {}: {e}; the hubs stay at {}", super::short(&work), super::short(&cli_good()))),
            }
        }
        if app {
            let t = Instant::now();
            match build_master_app(&ctx.repo, &|l| event(&ctx, Sev::Warn, l)) {
                Ok(()) => {
                    *APP_GOOD.lock().unwrap() = work.clone();
                    event(&ctx, Sev::Info, format!("update: the devhub app is built at {} in {:.0} s; the master takes it", super::short(&work), t.elapsed().as_secs_f64()));
                }
                Err(e) => event(&ctx, Sev::Warn, format!("update: the devhub app at {}: {e}; the master keeps running {}", super::short(&work), super::short(crate::lease::caps::hub_rev()))),
            }
        }
        CLI_BUILDING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        CLI_BUILDING.store(false, Ordering::Release);
    }
}

/// Why the master's update to the newest devhub build it lacks ([`CLI_GOOD`]) waits, if it does:
/// a build of a newer tip runs (it takes that one as it ends: an exec would orphan its cargo), a
/// lane sweep runs (its state is this process's), or an allapps run does (it is started over), for
/// at most [`MASTER_DEADLINE`] after the update was first seen (`pending`). Nothing the leases do
/// holds it: a restart ends no lease and no run, keeps the queue, starts leases in setup again and
/// holds a box whose cleanup is unconfirmed until its node confirms it (restart_tests, sched).
/// Waiting for a moment with no lease in setup or cleanup, it never came on a busy night: up 57
/// min with five landings pending (11-04); behind back-to-back timed leases it waited for hours
/// (11-03).
fn restart_blocker_of(building: bool, sweeping: bool, allapps: bool, flash: Option<String>, pending: Duration) -> Option<String> {
    // a flash through the master's claim socket: cut, the board's image is half written (it
    // takes seconds; a monitor holds nothing)
    if let Some(flash) = flash {
        return Some(format!("{flash}: it restarts as that ends"));
    }
    if building {
        return Some("devhub builds a newer tip: the master takes that build as it ends".into());
    }
    if sweeping {
        return Some("a lane sweep runs in the master".into());
    }
    if allapps && pending < MASTER_DEADLINE {
        return Some(format!("an allapps run: it restarts as that ends, {} min after the build at the latest", MASTER_DEADLINE.as_secs() / 60));
    }
    None
}

/// What a deployment push only waits for (a timed run on the box: `waits: timed lease L.. runs
/// here: the deployment moves after it`), from its answer lines (`<node> {"err":..}`).
fn push_waits(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|l| {
        let v = makepad_strict_json::parse(l[l.find('{')?..].as_bytes()).ok()?;
        v.get("err")?.as_str()?.strip_prefix("waits: ").map(str::to_string)
    })
}

/// More open fds than this: the master restarts itself (see [`note_fds`]).
const FDS_RESTART: usize = 8000;
const FDS_WARN: usize = 2000;
/// Set by a fd leak ([`note_fds`]): the master restarts itself at the next tick.
static RESTART_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn open_fds() -> usize {
    let dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}

/// A leak that fills the fd table stops every git and child the master spawns (14.8k unix sockets
/// on 10-30, git failing with EBADF, cause not found): it says so with the count early, and
/// restarts itself past the limit (leases and jobs go on or are placed again).
fn note_fds(ctx: &Ctx, said: &mut usize) {
    let n = open_fds();
    if n >= FDS_WARN && n >= *said * 2 {
        *said = n;
        event(ctx, Sev::Warn, format!("master: {n} open fds; {}", if n >= FDS_RESTART { "it restarts itself" } else { "a leak? `lsof -p` its pid and see who the peers are" }));
    }
    if n >= FDS_RESTART {
        RESTART_ASKED.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// A failed master update tries again after this, doubling up to [`MASTER_RETRY_MAX`].
const MASTER_RETRY: Duration = Duration::from_secs(10 * 60);
const MASTER_RETRY_MAX: Duration = Duration::from_secs(2 * 60 * 60);

/// How long the master's update waits for an allapps run.
const MASTER_DEADLINE: Duration = Duration::from_secs(3 * 60);

/// A service's code still this long: its roll starts (one per burst of landings).
const SETTLE_MS: u64 = 3 * 60 * 1000;

/// How far back the scheduler knows the landings: how far behind a box is, and whether its
/// deployment holds a queued timed ask's pin (`Sched::lacks_deployment`). A day: a timed ask
/// waited past four hours on a box whose deployment held its pin, and the four-hour list no
/// longer named the pin (11-18).
const LANDINGS_KNOWN: Duration = Duration::from_secs(24 * 3600);

/// The landings since `since` (ms), oldest first: the deployment each left and when.
fn landed_since(ctx: &Ctx, since: u64) -> Vec<(String, u64)> {
    use crate::land::{Entry, St};
    let history = ctx.store.history("land").unwrap_or_default();
    let mut landed: Vec<(String, u64)> = history.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed && e.changed_ms >= since)
        .filter_map(|e| { let pair = crate::bucket::entry_pair(&e.landed)?; Some((pair.pin_key(), e.changed_ms)) }).collect();
    landed.sort_by_key(|(_, ms)| *ms);
    landed
}

/// A node's hub that does not answer during its roll is restarting (its new build starts in
/// seconds): warned about only past this.
const HUB_BACK: Duration = Duration::from_secs(120);

/// An update (`what`: the deployment, the hub restart) that a timed run made wait
/// ([`crate::lease::master::Master::update_waits`]), said no more when its ask ends.
struct UpdateWaits<'a> {
    ctx: &'a Ctx,
    node: &'a str,
    what: &'static str,
    on: bool,
}

impl UpdateWaits<'_> {
    fn set(&mut self, why: Option<String>) {
        if why.is_some() || self.on {
            self.on = why.is_some();
            self.ctx.leases.update_waits(self.node, self.what, why);
        }
    }
}

impl Drop for UpdateWaits<'_> {
    fn drop(&mut self) {
        self.set(None);
    }
}

/// One node: wait for the deployment code, then ask for its hub's restart (which ends no run). Some: the
/// deployment its new hub does not build at (the hub runs on; said once per commit).
/// The boxes back on the network since the last look (`was_up`; up now, a device's tree aside)
/// whose deployment lacks the tips (`behind`, of its deployment): (name, deployment).
fn back_behind<'a>(was_up: &BTreeSet<String>, up: &'a [(String, String)], behind: impl Fn(&str) -> bool) -> Vec<&'a (String, String)> {
    up.iter().filter(|(name, deploy)| !was_up.contains(name) && !deploy.is_empty() && behind(deploy)).collect()
}

/// A box back on the network whose deployment lacks the tips (`tips`, the pin key of work and the
/// private main) gets them at once: the auto-sync's boxes by a kick, any other by a push with no
/// build. Off the auto-sync, a box's deployment moved only with a hub roll or a lease that named
/// it: the Windows box, away when its push failed, sat 81 h on that deployment until a lease
/// named it (11-17). Not a box a roll pushes itself (`busy`; the push holds it there too), one
/// whose deployment a timed ask's pin or a proof holds, a user's WM off the auto-sync (it moves
/// only to a proven tip), nor any at the first look (a master's start is no reconnect).
fn push_back_behind(ctx: &Arc<Ctx>, tips: &str, busy: &Arc<Mutex<BTreeSet<String>>>, was_up: &mut Option<BTreeSet<String>>) {
    let nodes = ctx.leases.nodes();
    let user_wm = |name: &str| nodes.iter().any(|n| n.name == name && n.role() == crate::lease::Role::UserWm);
    let up: Vec<(String, String)> = nodes.iter()
        .filter(|n| n.up && !matches!(n.role(), crate::lease::Role::Device))
        .filter_map(|n| n.caps.as_ref().map(|c| (n.name.clone(), c.deploy.clone())))
        .collect();
    let names: BTreeSet<String> = up.iter().map(|(n, _)| n.clone()).collect();
    let Some(before) = was_up.replace(names) else { return };
    if tips.is_empty() { return; }
    for (node, deploy) in back_behind(&before, &up, |d| !crate::sync::deploy_has(&ctx.repo, d, tips)) {
        if busy.lock().unwrap().contains(node) || ctx.leases.update_pin(node).is_some() || crate::sync::deploy_held(node).is_some() { continue; }
        let at = super::short(deploy.split(',').next().unwrap_or(""));
        if crate::sync::autosync_covers(&ctx.home, node) {
            event(ctx, Sev::Info, format!("update: {node} is back with its deployment at {at}: the auto-sync moves it to the tips"));
            crate::sync::kick_autosync(wall_ms());
            continue;
        }
        if user_wm(node) || !busy.lock().unwrap().insert(node.clone()) { continue; }
        let (ctx, held, node, at) = (ctx.clone(), busy.clone(), node.clone(), at.to_string());
        let name = node.clone();
        let busy_push = held.clone();
        let spawned = std::thread::Builder::new().name("devhub-push-back".into()).spawn(move || {
            let busy = busy_push;
            let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
            let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, &node, None, false);
            busy.lock().unwrap().remove(&node);
            if code == 0 {
                event(&ctx, Sev::Info, format!("update: {node} is back with its deployment at {at}: pushed to the tips"));
            } else {
                let err = lines.iter().rev().find(|l| l.contains("err")).cloned().unwrap_or_default();
                event(&ctx, Sev::Warn, format!("update: {node} is back with its deployment at {at}: its push to the tips failed: {err}"));
            }
        });
        if spawned.is_err() { held.lock().unwrap().remove(&name); }
    }
}

fn roll_node(ctx: &Ctx, node: &str, svc: &str, dirs: &BTreeSet<String>, work: &str, drains: Arc<AtomicUsize>, restarts: &dyn Fn()) -> RollEnd {
    let short = super::short(work);
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
    let caps = || ctx.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps);
    // Its hub has the target's code already (its WM's restart or a newer roll took it there): this
    // roll is done. Never a restart to where its hub was or behind it (a roll begun for an older
    // target kept a box "updating" to it while its hub ran a newer commit, 11-03).
    let has_it = || svc == "devhub" && caps().is_some_and(|c| !c.hub_rev.is_empty() && !lacks(&ctx.repo, &c.hub_rev, work, dirs));
    let done = |c: &crate::lease::Caps| event(ctx, Sev::Info, format!("update: {node} runs devhub at {} already (the roll was for {short})", super::short(&c.hub_rev)));
    // Deployment moves beside leases. Nodes outside autosync get a push here, without a restart:
    // to the tips, or to the pin of the timed ask first in line there (None: not pushed yet)
    let mut pushed: Option<Option<String>> = None;
    let mut kicked = false;
    // the pin holds the deployment until that ask starts (said once while it does)
    let mut held = false;
    let mut proof_said: Option<String> = None;
    let mut said = false;
    // a failed push is said once while it fails the same way
    let mut failed: Option<String> = None;
    let t = Instant::now();
    let mut slot = Slot { drains, held: false };
    let updating = |stall: Option<&str>| if svc == "devhub" {
        ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}{}", stall.map(|s| format!("; stalled: {s}")).unwrap_or_default())), false);
    };
    updating(None);
    // a push a timed run makes wait: an update wait (no new timed lease there until it moves), said
    // once, asked again at USER_WAIT (it was a warning every 5 s, 26 times for one run, 11-03)
    let mut push_wait = UpdateWaits { ctx, node, what: "deployment", on: false };
    // the deployment the hub builds at instead of the target, while that cannot move (None: the target)
    let mut builds_at: Option<String> = None;
    loop {
        let c = caps().unwrap_or_default();
        if has_it() {
            done(&c);
            return RollEnd::NoRestart;
        }
        // made a device while its roll ran: its tree is the person's, never a deploy target
        if c.role() == crate::lease::Role::Device {
            event(ctx, Sev::Info, format!("update: {node} is a device now: its tree is its own, and the roll to {short} leaves it"));
            return RollEnd::NoRestart;
        }
        // Older hubs share the sync checkout with leases: wait for apply without draining them.
        let leased_checkout = svc == "devhub" && c.deploy.is_empty();
        let at = if leased_checkout { &c.tree } else { &c.deploy };
        let at = at.split(',').next().unwrap_or("");
        if !lacks(&ctx.repo, at, work, dirs) {
            break;
        }
        let to = deploy_to(ctx.leases.update_pin(node), &c.deploy, |d, a| crate::sync::deploy_has(&ctx.repo, d, a));
        if let DeployTo::Held { lease, at: pin } = &to {
            // its hub behind the deployment its box has builds there meanwhile, not after that
            // ask's start and run ([`hub_behind_deployment`])
            if svc == "devhub" && !leased_checkout && hub_behind_deployment(&ctx.repo, &c.hub_rev, at, dirs) {
                event(ctx, Sev::Info, format!("update: {node}'s deployment is held at {} until {lease} (timed, first in line) starts: its hub, at {}, builds at that deployment meanwhile", super::short(at), super::short(&c.hub_rev)));
                builds_at = Some(at.to_string());
                break;
            }
            if !held {
                push_wait.set(None);
                updating(Some(&format!("held at {} until {lease} (timed, first in line) starts", super::short(pin))));
                held = true;
            }
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        if held {
            updating(None);
            held = false;
        }
        // the user's WM node takes only proven tips: its deployment holds for the proof (said as
        // the roll's stall while it does)
        let proof = crate::sync::deploy_held(node);
        if proof != proof_said {
            updating(proof.as_deref());
            proof_said = proof;
        }
        let pin = match to {
            DeployTo::Pin(at) => Some(at),
            _ => None,
        };
        if crate::sync::autosync_covers(&ctx.home, node) {
            // (the auto-sync follows the pin itself)
            if !kicked {
                crate::sync::kick_autosync(wall_ms());
                kicked = true;
            }
        } else if pushed.as_ref() != Some(&pin) {
            let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, node, pin.as_deref(), false);
            if code == 0 {
                pushed = Some(pin.clone());
            }
            match push_waits(&lines).filter(|_| code != 0) {
                Some(why) => {
                    if !push_wait.on {
                        updating(Some(&why));
                    }
                    push_wait.set(Some(format!("its deployment moves to {}", super::short(pin.as_deref().unwrap_or(work)))));
                    std::thread::sleep(USER_WAIT);
                    continue;
                }
                None if push_wait.on => {
                    push_wait.set(None);
                    updating(None);
                }
                None => {}
            }
            if code != 0 {
                let err = lines.iter().rev().find(|l| l.contains("err")).cloned().unwrap_or_default();
                // the node refused the order itself: the same push is refused the same way every
                // 5 s (a self-contained box whose tree is no fleet deployment: 90 warnings, 11-07)
                if err.contains("apply refused: ") {
                    event(ctx, Sev::Err, format!("update: {node}'s deployment cannot move to {short}: {err}; no roll moves it until its tree changes (a box whose tree is its own: `devhub node {node} role device` keeps it out of the rolls)"));
                    return RollEnd::Refused(c.deploy.split(',').next().unwrap_or("").to_string());
                }
                if failed.as_deref() != Some(err.as_str()) {
                    event(ctx, Sev::Warn, format!("update: pushing {node}'s deployment to {short} failed: {err}"));
                    failed = Some(err);
                }
            }
        }
        if !said && t.elapsed() > DRAIN_SAY {
            event(ctx, Sev::Warn, format!("update: {node}'s deployment waits for {short} (at {}{})", if at.is_empty() { "?" } else { super::short(at) }, proof_said.as_ref().map(|p| format!("; {p}")).unwrap_or_default()));
            said = true;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    // the hub builds itself on its box's cores like any build: in the room of a lease of its class
    // there, granted in its turn beside the work granted there, held until the hub answers that it
    // restarts on what it built (two Macs' hubs built beside their work at 12 and 14 of their 10
    // cores, 11-05)
    // The room is its build's alone: it is taken once the box's tree is free ([`tree_busy`]), and
    // given back while the hub waits on other work there (a sync's apply, a timed run), to be
    // taken again in line once that ends. The hub builds in its deployment's tree, which a sync's
    // apply builds in too (one cargo lock), and refuses its restart while one runs: a hub room
    // granted beside its box's deployment build held 4 cores past the box's for nothing, 36 of
    // 129 rooms in 6 h, and live test runs waited 11 min at 24 of 16 cores behind both (11-13).
    let take_room = || -> Result<super::leg::Held, RollEnd> {
        let mut said = None;
        while let Some(why) = tree_busy(ctx, node) {
            if has_it() {
                done(&caps().unwrap_or_default());
                return Err(RollEnd::NoRestart);
            }
            if said != Some(why) {
                updating(Some(&format!("its hub builds after {why} there (one tree)")));
                said = Some(why);
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        match hub_build_room(ctx, node, &has_it, &|line| updating(Some(&format!("its hub build waits for room ({line})")))) {
            Ok(held) => Ok(held),
            Err(_) if has_it() => {
                done(&caps().unwrap_or_default());
                Err(RollEnd::NoRestart)
            }
            Err(e) => {
                event(ctx, Sev::Warn, format!("update: {node}'s hub build got no room: {e}"));
                Err(RollEnd::NoRestart)
            }
        }
    };
    // A service its node keeps (the AI hub of a node no WM runs, started from its kept copy) rolls
    // onto its node's build of what the deployment holds: the node's prebuild runs first, at that
    // deployment (it moves nothing), in the box's deployment-build room, and replaces the kept
    // copy when it built other bytes. A timed run or ask there makes it wait, as a push does. A
    // build that fails leaves the service on its kept copy, said, until the next change. Only a
    // node off the auto-sync: the auto-sync's apply builds as its sync.conf says, and off it no
    // apply built, so a Mac's AI hub ran the installer's build for two weeks (11-24).
    if svc != "devhub" && !crate::sync::autosync_covers(&ctx.home, node) && caps().is_some_and(|c| c.has("deploy-builds")) {
        let mut waited = UpdateWaits { ctx, node, what: "build", on: false };
        loop {
            let at = caps().map(|c| c.deploy).filter(|d| !d.is_empty());
            let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, node, at.as_deref(), true);
            if code == 0 {
                waited.set(None);
                break;
            }
            if let Some(why) = push_waits(&lines) {
                waited.set(Some(format!("its {svc} build ({why})")));
                std::thread::sleep(USER_WAIT);
                continue;
            }
            waited.set(None);
            let err = lines.iter().rev().find(|l| l.contains("err")).cloned().unwrap_or_default();
            event(ctx, Sev::Warn, format!("update: {node}'s build of {svc} for {short} failed: {err}; {svc} runs on its kept copy, the last build that succeeded, until the next change"));
            return RollEnd::Current;
        }
    }
    let mut room = None;
    if svc == "devhub" {
        match take_room() {
            Ok(held) => room = Some(held),
            Err(end) => return end,
        }
        updating(None);
        // A hub without `handover` restarts only between runs: it drains, once.
        let old = caps().is_some_and(|c| !c.has("handover"));
        if old { ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}; an older hub: no new leases until it restarts")), true); }
        let short = builds_at.as_deref().map_or(short, super::short);
        event(ctx, Sev::Info, format!("update: {node} restarts its hub for {short}{}", if old { " once its runs end (an older hub: no new leases until then)" } else { " now (runs and leases go on)" }));
    } else {
        slot.take();
    }
    let word = if svc == "devhub" { "hub-restart" } else { "service-restart" };
    let before = caps();
    // the hub restarts at once (it says `waits:` only for seconds; an older one until its runs end);
    // a user-facing service (the AI hub) never under a present user or unsaved work: the roll
    // waits until the box is idle (the node refuses with `waits:` and says what for)
    let mut said = false;
    let t = Instant::now();
    // since when the node's hub has not answered: it restarts (for this roll, a parallel one or
    // its WM's), which is planned and said as info; a warning only past HUB_BACK
    let mut down: Option<Instant> = None;
    let mut said_down = false;
    // a timed run there makes the hub restart wait (its new hub builds after it): the next timed
    // lease waits for the restart, which gets the gap after the run (said until this ask ends)
    let mut waits = UpdateWaits { ctx, node, what: "hub restart", on: false };
    let mut stall = String::new();
    loop {
        let watched = svc != "devhub" && caps().is_some_and(|c| c.watched);
        // A newer apply may have moved the deployment since the master read its caps.
        let deploy = caps().map(|c| c.deploy).unwrap_or_default().split(',').next().unwrap_or("").to_string();
        // a hub that came back from a restart at the deployment already: never restarted twice
        if svc == "devhub" && down.is_some() && caps().is_some_and(|c| !deploy.is_empty() && c.hub_rev == deploy) {
            event(ctx, Sev::Info, format!("update: {node} runs devhub at {}", super::short(&deploy)));
            return RollEnd::NoRestart;
        }
        if has_it() {
            done(&caps().unwrap_or_default());
            return RollEnd::NoRestart;
        }
        if svc == "devhub" && room.is_none() {
            match take_room() {
                Ok(held) => { room = Some(held); updating(None); stall.clear(); }
                Err(end) => return end,
            }
        }
        // a hub build granted fewer cores than it measured (it started now on its box's free
        // ones) builds on that many jobs; an older hub compares the whole word with its
        // deployment, so it gets the sha alone
        let jobs = room.as_ref().filter(|_| caps().is_some_and(|c| c.has("hub-jobs"))).and_then(|held| ctx.leases.hub_build_jobs(&held.id));
        let line = if svc == "devhub" { format!("hub-restart {deploy}{}", jobs.map_or(String::new(), |j| format!(" jobs={j}"))) } else { format!("service-restart {svc}") };
        let answer = if watched { Ok(vec![format!("{{\"err\":\"waits: someone watches {node}'s screen\"}}")]) } else { crate::cli::ask(&ctl, Some(node), word, &line) };
        if answer.is_ok() { down = None; }
        match answer {
            Err(e) if crate::lease::master::unreachable(&e) && down.get_or_insert_with(Instant::now).elapsed() < HUB_BACK => {
                if !said_down {
                    let what = if crate::lease::master::host_gone(&e) { "is off the network" } else { "'s hub is restarting" };
                    event(ctx, Sev::Info, format!("update: {node}{what} ({e}); {svc} is asked again when it answers"));
                    said_down = true;
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":\"waits: ")) => {
                if svc == "devhub" && gives_room_back(&l[0]) {
                    room = None;
                }
                if svc == "devhub" && !waits.on && l[0].contains("waits: a timed lease runs") {
                    waits.set(Some(format!("its hub restarts at {}", super::short(&deploy))));
                }
                // what it waits for, on the box's line while it waits
                if let Some(why) = push_waits(&l).filter(|w| *w != stall) {
                    updating(Some(&why));
                    stall = why;
                }
                if t.elapsed() > SLOT_HOLD {
                    slot.give_back();
                }
                if !said && (svc != "devhub" || t.elapsed() > DRAIN_SAY) {
                    event(ctx, Sev::Info, format!("update: {node} {svc} waits: {}", l[0]));
                    said = true;
                }
                std::thread::sleep(if svc == "devhub" { Duration::from_secs(5) } else { USER_WAIT });
            }
            // a hub from before run-boundary restarts refuses while any lease is there: wait it out
            Ok(l) if svc == "devhub" && l.first().is_some_and(|x| x.contains("drain first")) => std::thread::sleep(Duration::from_secs(5)),
            Ok(l) if svc == "devhub" && l.first().is_some_and(|x| x.starts_with("{\"err\":\"the new hub does not build")) => {
                let why = makepad_strict_json::parse_depth(l[0].as_bytes(), 8).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_else(|| l[0].clone());
                if BROKEN_SAID.lock().unwrap().insert(deploy.clone()) {
                    event(ctx, Sev::Err, format!("update: the hub does not build at {} ({node}: {why}); the hubs keep running their code until a landing that builds", super::short(&deploy)));
                } else {
                    event(ctx, Sev::Info, format!("update: {node}: {why}"));
                }
                restarts();
                return RollEnd::Broke(deploy);
            }
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":")) => {
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {}", l[0]));
                return RollEnd::NoRestart;
            }
            Err(e) => {
                let gone = down.map(|d| format!(" (no answer for {} s)", d.elapsed().as_secs())).unwrap_or_default();
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {e}{gone}"));
                return RollEnd::NoRestart;
            }
            // its service already runs the deployment's build (the WM's restart took it there,
            // or the landing changed nothing it is built of): never restarted for nothing
            Ok(l) if svc != "devhub" && l.first().is_some_and(|x| x.starts_with("{\"current\":")) => {
                event(ctx, Sev::Info, format!("update: {node} runs {svc} at {short} already: no restart"));
                return RollEnd::Current;
            }
            Ok(l) => {
                // the hub build it restarts for, measured there: its class's measure in the ledger
                if let Some(b) = l.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok()).and_then(|v| v.get("built").and_then(crate::lease::HubMeasure::parse)) {
                    ctx.leases.ledger_hub_build(node, &b);
                }
                restarts();
                event(ctx, Sev::Info, format!("update: {node} restarts {svc} for {short}"));
                break;
            }
        }
    }
    drop(waits);
    // its build is done (or it will not build): the room is the box's again
    drop(room);
    // back: the hub reports the new commit; a service a newer start that answers again (its slot
    // is held till then: a new start takes seconds, its build and load minutes, and a roll that
    // went on at the new start had every box's AI hub down at once)
    let answers = || crate::cli::ask(&ctl, Some(node), "service-health", &format!("service-health {svc}")).is_ok_and(|l| l.first().is_some_and(|x| x.contains("\"answers\":true")));
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1500) {
        std::thread::sleep(Duration::from_secs(5));
        let Some(c) = caps() else { continue };
        let back = if svc == "devhub" {
            !c.hub_rev.is_empty() && Some(&c.hub_rev) != before.as_ref().map(|b| &b.hub_rev)
        } else {
            let old = before.as_ref().and_then(|b| b.services.iter().find(|(x, _)| x == svc).map(|s| s.1)).unwrap_or(0);
            c.services.iter().any(|(x, t)| x == svc && *t > old) && answers()
        };
        if back {
            let at = if svc == "devhub" { super::short(&c.hub_rev).to_string() } else { short.to_string() };
            event(ctx, Sev::Info, format!("update: {node} runs {svc} at {at}{}", if svc == "devhub" { "" } else { ", and it answers" }));
            return RollEnd::Back;
        }
    }
    event(ctx, Sev::Err, format!("update: {node} {svc} did not come back in 25 min (its log on the node)"));
    RollEnd::Late
}

/// What `node`'s deployment tree does now that its hub's build waits for: a sync's apply there
/// ([`crate::lease::Caps::applying`]; the hub refuses its restart under one), its deployment build
/// ([`deploy_build_room`], in the same tree and target), or a timed run (the hub builds after it).
/// None: the tree is free for the hub's build.
fn tree_busy(ctx: &Ctx, node: &str) -> Option<&'static str> {
    use crate::lease::sched::State;
    let applying = ctx.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps).is_some_and(|c| c.applying);
    let leases = ctx.leases.view();
    let running = leases.iter().filter(|l| matches!(l.state, State::Starting | State::Granted) && l.node.as_deref() == Some(node));
    let deploy_queued = leases.iter().any(|l| l.state == State::Queued && l.req.deploy_build() && l.req.node.as_deref() == Some(node));
    tree_busy_of(applying, deploy_queued, running.map(|l| (l.req.deploy_build(), l.req.kind == crate::lease::Kind::Timed)))
}

/// [`tree_busy`] of whether the box applies, whether its deployment's build waits in line for it,
/// and of each lease running there whether it is its deployment's build and whether it is timed.
/// A hub room asked beside a deployment build in line was granted with it, and given back at once
/// as the hub waited on that build's apply: the room held its cores for nothing, and the box's
/// background work waited on the update it announced (L1169056, 11-16).
fn tree_busy_of(applying: bool, deploy_queued: bool, running: impl Iterator<Item = (bool, bool)>) -> Option<&'static str> {
    if applying { return Some("a sync's apply"); }
    let (mut deploy, mut timed) = (false, false);
    for (d, t) in running { deploy |= d; timed |= t; }
    if deploy { return Some("its deployment's build"); }
    if deploy_queued { return Some("its deployment's build in line"); }
    timed.then_some("a timed run")
}

/// Whether a hub's `waits:` answer to `hub-restart` is a wait on other work on its box, not on its
/// own build: a sync's apply there, a timed run (`lease/exec.rs`). Its build room goes back until
/// that ends ([`tree_busy`]); its own build, a deployment that moved under the ask (asked again at
/// once at the new one) and the short ones keep it.
fn gives_room_back(answer: &str) -> bool {
    answer.contains("waits: a sync runs on ") || answer.contains("waits: a timed lease runs on ")
}

/// The room `node`'s hub builds itself in ([`crate::lease::sched::Req::hub_build`]): the master's
/// own lease of its class there, granted in its turn (its wait on the box's line). `gone`: the
/// hub has the code another way, and the ask is given up.
fn hub_build_room(ctx: &Ctx, node: &str, gone: &dyn Fn() -> bool, waits: &dyn Fn(&str)) -> Result<super::leg::Held, String> {
    use crate::lease::sched::{DEFAULT_PRIO, HUB_BUILD};
    let spec = super::leg::Spec { lane: format!("{HUB_BUILD}-{node}"), agent: "devhub-roll".into(), kind: crate::lease::Kind::Build, needs: String::new(), prio: DEFAULT_PRIO, rev: String::new(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: Some(node.to_string()), since_ms: None };
    super::leg::Held::acquire_with(&spec, &|r| r.class = HUB_BUILD.into(), &mut |line| waits(line), gone)
}

/// The room a box's deployment build takes (a `sync` apply's prebuild, [`crate::lease::sched::Req::deploy_build`]):
/// a lease of its hub build's class, so sized and queued like it, held by the apply until its node
/// answers. `repo` is the master's checkout. It waits in line like any build, but never behind the
/// box's timed work, which goes first there: a timed run on the box, or a timed ask first in line
/// for it (the box holds its deployment for that ask, sync/autosync.rs), gives it up (None) and
/// the apply waits for the gap after. A master restart does not wait for it: the apply goes on on
/// its node, and the next master holds the room until that apply ends ([`follow_rooms`]).
pub(crate) fn deploy_build_room(repo: &std::path::Path, node: &str, waits: &mut dyn FnMut(&str)) -> Result<Option<DeployRoom>, String> {
    use crate::lease::sched::{DEFAULT_PRIO, DEPLOY_BUILD, HUB_BUILD};
    let timed = || super::leases().is_some_and(|lm| lm.update_pin(node).is_some() || lm.view().iter().any(|l| matches!(l.state, crate::lease::sched::State::Starting | crate::lease::sched::State::Granted) && l.req.kind == crate::lease::Kind::Timed && l.node.as_deref() == Some(node)));
    if timed() { return Ok(None); }
    let spec = super::leg::Spec { lane: format!("{DEPLOY_BUILD}-{node}"), agent: "devhub-sync".into(), kind: crate::lease::Kind::Build, needs: String::new(), prio: DEFAULT_PRIO, rev: String::new(), repo_dir: repo.to_string_lossy().into_owned(), node: Some(node.to_string()), since_ms: None };
    match super::leg::Held::acquire_with(&spec, &|r| r.class = HUB_BUILD.into(), waits, &timed) {
        Ok(held) => Ok(Some(DeployRoom { held })),
        Err(_) if timed() => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `node` is a bench box: timed work is its job, so its deployment's tree moves only with
/// its build, in its room (sync/autosync.rs): a tree that moved alone handed its JIT apps a landing
/// of the JIT's own code, which they took by rebuilding `makepad run` at ~13 cores outside any
/// lease, while timed work waited there (11-13).
pub(crate) fn is_bench(node: &str) -> bool {
    super::leases().is_some_and(|lm| lm.nodes().iter().any(|n| n.name == node && n.role() == crate::lease::Role::Bench))
}

/// A box's deployment-build room ([`deploy_build_room`]), held until dropped.
pub(crate) struct DeployRoom {
    held: super::leg::Held,
}

impl DeployRoom {
    /// The jobs its build builds with ([`crate::lease::sched::Sched::hub_build_jobs`]), for a node
    /// whose hub reports `deploy-jobs`.
    pub(crate) fn jobs(&self) -> Option<u32> {
        let lm = super::leases()?;
        let takes = lm.nodes().into_iter().find(|n| n.name == self.held.node).and_then(|n| n.caps).is_some_and(|c| c.has("deploy-jobs"));
        takes.then(|| lm.hub_build_jobs(&self.held.id)).flatten()
    }
}

/// How a box's roll ended. Only a restart that was asked for goes on the record: a service that
/// runs that build already is not one (the AI hub said so to an ask every 15 s, read as 24 rolls a minute).
enum RollEnd {
    /// restarted, and back
    Back,
    /// restarted, and not back
    Late,
    /// its new hub does not build at this commit
    Broke(String),
    /// its node refused its deployment's apply, at this deployment: no roll until that changes
    Refused(String),
    /// the service runs the deployment's build already
    Current,
    /// nothing to do any more, or it would not: said in the update's events
    NoRestart,
}

/// The deployments a hub did not build at, said loudly once (each box refuses it on its own).
static BROKEN_SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Rustup counts its nested proxies here and refuses past 20 ("infinite recursion detected"): a
/// master started through `cargo run` carries it into every build it runs.
const RUSTUP_DEPTH: &str = "RUST_RECURSION_COUNT";

/// One master restart at a time: the self-update's and `devhub master restart`'s.
pub(super) static RESTARTING: Mutex<()> = Mutex::new(());

/// The deployment-build rooms a previous master granted whose apply still runs on its node: held
/// until it ends ([`follow_rooms`]).
static FOLLOWED_ROOMS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// How often a taken-back room asks its node whether the apply still runs.
const ROOM_LOOK: Duration = Duration::from_secs(5);

/// A master restart waits for none of its own work: a box's deployment build goes on on its node
/// (the sync apply that asked for it is the node's), and its room, the box's charge for that build,
/// is held by the new master until the node's apply ends, the node answering that no apply runs
/// (`sync status`), or until the lease ends (the box gone off the network, the user). A restart
/// that waited for them waited minutes (a box's deployment build: p90 230 s, up to 19 min, 11-15).
/// Called once at the start, before the master's own leases no record names end.
fn follow_rooms(ctx: &Arc<Ctx>) {
    use crate::lease::sched::State;
    for l in ctx.leases.view().into_iter().filter(|l| l.req.master && l.req.deploy_build() && l.state == State::Granted) {
        let Some(node) = l.node.clone() else { continue };
        FOLLOWED_ROOMS.lock().unwrap().insert(l.id.clone());
        let (c, id) = (ctx.clone(), l.id.clone());
        let followed = std::thread::Builder::new().name("devhub-room".into()).spawn(move || {
            let held = super::leg::Held::attach(&id, &node);
            event(&c, Sev::Info, format!("update: {node}'s deployment build goes on under this master: its room {id} is held until the box's apply ends"));
            let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
            while super::leg::lease_held(&c, &id) && !apply_ended(crate::cli::ask(&ctl, Some(&node), "sync", "status")) {
                std::thread::sleep(ROOM_LOOK);
            }
            held.release(None);
            FOLLOWED_ROOMS.lock().unwrap().remove(&id);
        });
        if followed.is_err() { FOLLOWED_ROOMS.lock().unwrap().remove(&l.id); }
    }
}

/// A node's `sync status` answer says no apply runs there. An unreachable node (its hub
/// restarting after the build) is asked again.
fn apply_ended(status: Result<Vec<String>, String>) -> bool {
    status.ok().and_then(|lines| lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok())).and_then(|v| v.get("busy").and_then(|b| b.as_bool())) == Some(false)
}

/// A room the master holds for a box's apply a previous master started ([`follow_rooms`]): a
/// record of its work, so it does not end with the unfollowed ones.
pub fn follows_room(id: &str) -> bool {
    FOLLOWED_ROOMS.lock().unwrap().contains(id)
}

/// What a master runs as, and so what its self-update builds and execs into: devhub's own binary
/// (`makepad-devhub serve`, at `target/host`, the CLI's callers' too), or the devhub app with its
/// menu-bar item (a master on a desktop, apps/devhub), built in its own target dir ([`APP_TARGET`])
/// and run from there: its login item's wrapper builds the same there before it starts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterBin {
    Cli,
    App,
}

/// The devhub app's package and binary.
pub const APP_NAME: &str = "makepad-app-devhub";
/// The target dir of the devhub app a master runs as (in the checkout).
pub const APP_TARGET: &str = "target/devhub-app";

impl MasterBin {
    /// This process's: the devhub app when it is that binary.
    pub fn this() -> MasterBin {
        Self::of(&std::env::current_exe().unwrap_or_default())
    }

    fn of(exe: &Path) -> MasterBin {
        if exe.file_stem().is_some_and(|s| s == APP_NAME) { MasterBin::App } else { MasterBin::Cli }
    }

    /// The binary it execs into once its build is done.
    pub fn live(self, repo: &Path) -> PathBuf {
        let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
        match self {
            MasterBin::Cli => repo.join("target/host").join(exe("makepad-devhub")),
            MasterBin::App => repo.join(APP_TARGET).join("host").join(exe(APP_NAME)),
        }
    }

    /// Where devhub's binary builds: the app's target dir under a master that runs as the app. The
    /// app builds on the same devhub lib, one compile (over 9 min on a Mac at background QoS, 11-09):
    /// in a target dir of its own it was compiled twice in each landing's cycle, and the master
    /// took a landing only after the second.
    fn cli_target(self, repo: &Path) -> PathBuf {
        match self {
            MasterBin::Cli => repo.join("target/devhub"),
            MasterBin::App => repo.join(APP_TARGET),
        }
    }

    /// The repo dirs whose change it rolls for: libs/devhub's graph (`devhub`), and the app's own
    /// crate for the app.
    fn dirs(self, devhub: &BTreeSet<String>) -> BTreeSet<String> {
        let mut dirs = devhub.clone();
        if self == MasterBin::App {
            dirs.insert("apps/devhub".to_string());
        }
        dirs
    }
}

/// The master execs itself into the landed code ([`build_devhub`], and [`build_master_app`] for the
/// app: [`MasterBin`]), with the same args.
/// `build`: `devhub master restart` builds work's tip (`work`) first, and a build that fails keeps
/// this master running and says so: an Err event (the inbox) and the answer to whoever asked
/// (`by`: it waits for it). Never a silent no-op (11-02: a hand restart after a failed self-update
/// was dropped by its retry pause, and clients met the old master's "unsupported service" for 15
/// min). The self-update builds nothing: `work` is the newest devhub build the master completed
/// ([`CLI_GOOD`]), swapped in at `target/host/makepad-devhub` as the master execs into it
/// ([`swap_cli`]: the lanes' CLI with it; a newer one built since runs under its older name, and
/// the new master takes the rest at once). Ok: the exec follows in a moment, after the answer went
/// out.
pub fn restart_master(ctx: &Arc<Ctx>, work: &str, by: &str, build: bool) -> Result<String, String> {
    let _one = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
    let mine = super::short(crate::lease::caps::hub_rev()).to_string();
    // the running build is kept before the new one is swapped in: `devhub master rollback` execs back into it
    keep_running_binary(&ctx.repo)?;
    let bin = MasterBin::this();
    let how = if build { format!("it builds in {} first", ctx.repo.display()) } else { "its build is done".to_string() };
    event(ctx, Sev::Info, format!("update: the master restarts on {} ({by}; {how})", super::short(work)));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // built first: a build that fails keeps this master running. Its own work waits for nothing:
    // its leases, jobs, live sessions, warm legs and deployment rooms go on under the next master
    let t = Instant::now();
    let built_at = if build { crate::lease::caps::head_sha(&ctx.repo) } else { Some(work.to_string()) };
    let built = if !build {
        Ok(())
    } else {
        let say = |l: String| event(ctx, Sev::Warn, l);
        build_devhub(&ctx.repo, built_at.as_deref(), rule_builds(ctx), &say).and_then(|()| if bin == MasterBin::App { build_master_app(&ctx.repo, &say) } else { Ok(()) })
    };
    if let Err(why) = built {
        let line = format!("update: the master's build of {} failed ({why}); it keeps running {mine} ({by})", super::short(work));
        event(ctx, Sev::Err, line.clone());
        return Err(line);
    }
    // the lanes' CLI moves with the master: swapped in now, as it execs into the same build
    if let Err(e) = swap_cli(&ctx.repo) {
        let line = format!("update: the master's restart on {}: {e}; it keeps running {mine} ({by})", super::short(work));
        event(ctx, Sev::Err, line.clone());
        return Err(line);
    }
    // the build just swapped in, exec'd as it is: a `cargo run` here would wait on the build lock
    // of every other cargo in `target/host` while no master beats
    let mut cmd = std::process::Command::new(exec_image(ctx, bin, &bin.live(&ctx.repo)));
    cmd.args(&args).current_dir(&ctx.repo);
    match &built_at {
        Some(rev) => cmd.env(crate::lease::caps::BUILT_AT, rev),
        None => cmd.env_remove(crate::lease::caps::BUILT_AT),
    };
    // the new build's probation starts now ([`guard_fresh_build`])
    cmd.env(RESTARTED_MS, wall_ms().to_string());
    #[cfg(unix)]
    {
        // the dials of the exec's gap queue for the new build instead of being refused
        crate::node::keep_control_over_exec(&mut cmd);
        let c2 = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-master-exec".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _held = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
            let e = crate::node::exec(&mut cmd);
            event(&c2, Sev::Err, format!("update: the master could not restart itself: {e}; it keeps running {}", super::short(crate::lease::caps::hub_rev())));
        });
        Ok(format!("built {} in {:.0} s; the master execs into it now (the same pid: leases, jobs, live sessions, warm legs and deployment builds go on)", super::short(work), t.elapsed().as_secs_f64()))
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, t);
        let line = "update: restart the master by hand (no exec on this OS)".to_string();
        event(ctx, Sev::Err, line.clone());
        Err(line)
    }
}

pub(super) fn cargo_bin() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| PathBuf::from("cargo"))
}

/// devhub's binary, built apart from `target/host` ([`MasterBin::cli_target`]: never the lock of
/// that one, where the apps and the JIT build), swapped in at `target/host/makepad-devhub` through a
/// rename as the master execs into it ([`swap_cli`]): a caller runs the old build or the new one,
/// never a missing or half-written file, and never one ahead of its master. A background
/// build ([`crate::os::background`]: background QoS on the person's Mac) on 4 jobs, the master's
/// own restart too: the running master beats on while it builds, and only the exec's gap counts.
/// One build at a time, and its swap apart from either: cargo on macOS copies the binary up out of `deps`
/// (no hard link), removing the old copy first even when nothing rebuilt, so the swap of a build
/// that ended while another one's cargo ran found no file (11-07: the CLI's rebuild and the
/// master's restart at one landing; the master stayed behind for its retry pause).
pub fn build_devhub(repo: &Path, rev: Option<&str>, rule: bool, say: &dyn Fn(String)) -> Result<(), String> {
    let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    let target = MasterBin::this().cli_target(repo);
    let mut cmd = crate::os::background(cargo_bin().as_os_str());
    cmd.args(["build", "-j4", "--profile", "host", "-p", "makepad-devhub", "--bin", "makepad-devhub"]);
    // the land rule's binary from the same tree ([`crate::gate`]), when the private checkout holds
    // it and the rule is not switched off (`rule`, [`rule_builds`])
    let gate = rule && crate::gate::present(repo);
    if gate {
        cmd.args(["-p", crate::gate::BIN, "--bin", crate::gate::BIN]);
    }
    let gate_at = gate_head(repo);
    cmd.arg("--target-dir").arg(&target).current_dir(repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null());
    // the commit it builds, which its CLI says beside a master's refusal (`caps::cli_build_rev`)
    match rev {
        Some(rev) => { cmd.env(crate::lease::caps::BUILD_REV, rev); }
        None => { cmd.env_remove(crate::lease::caps::BUILD_REV); }
    }
    match cmd.output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => return Err(format!("does not build: {}", build_failure(&o))),
        Err(e) => return Err(format!("cargo: {e}")),
    }
    let built = target.join("host").join(cli_name());
    sign_stable(repo, &built, CLI_SIGN_ID, say);
    if gate {
        built_gate(repo, &target, gate_at.as_deref(), say);
    }
    Ok(())
}

/// Whether the master builds the land rule's binary: the private checkout holds it, and the rule
/// is not switched off ([`crate::state::Doc::land_rule_off`]).
fn rule_builds(ctx: &Ctx) -> bool {
    crate::gate::present(&ctx.repo) && ctx.store.read(|d| d.land_rule_off.is_empty())
}

fn cli_name() -> String {
    format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX)
}

/// The private checkout's HEAD, the source a land-rule binary built now is of.
fn gate_head(repo: &Path) -> Option<String> {
    crate::gitread::rev(&repo.join("apps/commercial"), "HEAD")
}

/// The file beside a land-rule binary naming the private checkout's commit it was built at.
fn gate_stamp(bin: &Path) -> PathBuf {
    bin.with_extension("rev")
}

/// A land-rule binary just built in `target`: signed, and stamped with the commit it is of.
fn built_gate(repo: &Path, target: &Path, at: Option<&str>, say: &dyn Fn(String)) {
    let bin = target.join("host").join(crate::gate::bin(repo).file_name().unwrap_or_default());
    sign_stable(repo, &bin, GATE_SIGN_ID, say);
    let _ = std::fs::write(gate_stamp(&bin), at.unwrap_or_default());
}

/// Whether the land-rule binary is due a build of its own: none is live (`last_try`: when a build
/// was last tried, so a failing one is tried again only every [`GATE_RETRY`]), or the private
/// checkout moved under its package since the live one was built (`touched`).
fn gate_due(live: bool, built_at: &str, head: &str, touched: impl FnOnce() -> bool, last_try: Option<Instant>) -> bool {
    if !live {
        return last_try.is_none_or(|t| t.elapsed() >= GATE_RETRY);
    }
    !built_at.is_empty() && built_at != head && touched()
}

const GATE_RETRY: Duration = Duration::from_secs(600);

/// The land rule's binary alone ([`crate::gate`]): built and swapped in at once when none is live
/// (a master's first start with the private checkout, or one whose build failed) or when the
/// private checkout moved under its package (a landing there changes nothing of devhub's own, so
/// no devhub build takes it along). The binary runs per call, so it goes live as it is swapped
/// in. On its own thread, one cargo at a time ([`BUILDING`]); a landing waits for it. Its flag is
/// its own ([`GATE_BUILDING`]): devhub's CLI build and the master's self-update never wait for it.
/// A land-rule build whose signing waited 36 min on the Mac's keychain held the CLI's flag, and
/// the master took none of three devhub landings (11-16).
fn prebuild_gate(ctx: &Arc<Ctx>, seen: &mut String, last_try: &mut Option<Instant>) {
    if !rule_builds(ctx) || CLI_BUILDING.load(Ordering::Acquire) || GATE_BUILDING.load(Ordering::Acquire) {
        return;
    }
    let Some(head) = gate_head(&ctx.repo) else { return };
    let live = crate::gate::bin(&ctx.repo);
    if live.is_file() && *seen == head {
        return;
    }
    *seen = head.clone();
    let built_at = std::fs::read_to_string(gate_stamp(&live)).unwrap_or_default().trim().to_string();
    let commercial = ctx.repo.join("apps/commercial");
    let dir = BTreeSet::from([crate::gate::CRATE_DIR.trim_start_matches("apps/commercial/").to_string()]);
    if !gate_due(live.is_file(), &built_at, &head, || touches(&commercial, &built_at, &head, &dir), *last_try) {
        return;
    }
    *last_try = Some(Instant::now());
    GATE_BUILDING.store(true, Ordering::Release);
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("devhub-gate-build".into()).spawn(move || {
        let t = Instant::now();
        match build_gate(&ctx.repo, &head, &|l| event(&ctx, Sev::Warn, l)) {
            Ok(()) => event(&ctx, Sev::Info, format!("update: the land rule's binary is built at {} in {:.0} s and live", super::short(&head), t.elapsed().as_secs_f64())),
            Err(e) => event(&ctx, Sev::Warn, format!("update: the land rule's binary at {}: {e}", super::short(&head))),
        }
        GATE_BUILDING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        GATE_BUILDING.store(false, Ordering::Release);
    }
}

/// A land-rule build runs now ([`prebuild_gate`]).
static GATE_BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`prebuild_gate`]'s build: the binary alone, into devhub's build dir, then swapped in.
fn build_gate(repo: &Path, head: &str, say: &dyn Fn(String)) -> Result<(), String> {
    let target = MasterBin::this().cli_target(repo);
    {
        let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut cmd = crate::os::background(cargo_bin().as_os_str());
        cmd.args(["build", "-j4", "--profile", "host", "-p", crate::gate::BIN, "--bin", crate::gate::BIN, "--target-dir"]).arg(&target).current_dir(repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null());
        match cmd.output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => return Err(format!("does not build: {}", build_failure(&o))),
            Err(e) => return Err(format!("cargo: {e}")),
        }
    }
    finish_gate(repo, &target, head, |staged| sign_stable(repo, staged, GATE_SIGN_ID, say))
}

/// A built land-rule binary goes live signed: a copy of it is signed outside the build lock
/// ([`BUILDING`]), so a signing that waits (the keychain) holds no devhub build, then it takes the
/// built one's place, with its stamp, and goes live ([`swap_gate`]).
fn finish_gate(repo: &Path, target: &Path, head: &str, sign: impl FnOnce(&Path)) -> Result<(), String> {
    let built = target.join("host").join(crate::gate::bin(repo).file_name().unwrap_or_default());
    let staged = built.with_extension("signing");
    {
        let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::copy(&built, &staged).map_err(|e| format!("{}: {e}", staged.display()))?;
    }
    sign(&staged);
    let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::rename(&staged, &built).map_err(|e| format!("{}: {e}", built.display()))?;
    std::fs::write(gate_stamp(&built), head).map_err(|e| format!("{}: {e}", gate_stamp(&built).display()))?;
    swap_gate(repo, target)
}

/// The land-rule binary of devhub's build dir goes live, with its stamp (none built: nothing).
fn swap_gate(repo: &Path, target: &Path) -> Result<(), String> {
    let live = crate::gate::bin(repo);
    let built = target.join("host").join(live.file_name().unwrap_or_default());
    if !built.is_file() {
        return Ok(());
    }
    swap_in(&built, &live)?;
    let _ = std::fs::copy(gate_stamp(&built), gate_stamp(&live));
    Ok(())
}

/// The lanes' CLI takes the newest devhub build ([`build_devhub`]), as the master execs into it
/// ([`restart_master`]): one build at a time, so never a binary cargo is writing. None built yet:
/// the CLI stays as it is. The CLI it replaces is kept ([`prev_cli`]) for a rollback to put back.
fn swap_cli(repo: &Path) -> Result<(), String> {
    let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    let target = MasterBin::this().cli_target(repo);
    let built = target.join("host").join(cli_name());
    if !built.is_file() { return Ok(()); }
    let live = repo.join("target/host").join(cli_name());
    if live.is_file() {
        swap_in(&live, &prev_cli(repo))?;
    }
    swap_in(&built, &live)?;
    // the land rule's binary built with it goes live with it
    swap_gate(repo, &target)
}

/// The fixed code identifiers of devhub's binaries on a Mac: with the local signing identity their
/// designated requirement is identifier + certificate, the same for every build, so macOS privacy
/// grants (Local Network, keychain access) stay across self-updates. The master app's is the
/// bundle id of its Dock wrapper (`~/Applications/devhub.app`).
pub const APP_SIGN_ID: &str = "nl.makepad.devhub";
pub const CLI_SIGN_ID: &str = "nl.makepad.devhub-cli";
pub const GATE_SIGN_ID: &str = "nl.makepad.devhub-gate";

/// Signs a just-built devhub binary on macOS (tools/macos/sign-app.sh: the stable identity
/// "makepad local signing", else ad-hoc with a one-line hint to run tools/macos/make-signing-identity.sh),
/// before it is swapped in and started. Never fails a build or an update: an error is logged and the
/// binary starts as the linker left it (ad-hoc). Nothing on other OSes.
/// A signing that runs this long is said once ([`run_said`]): it is not stopped.
const SIGN_SAY: Duration = Duration::from_secs(180);

fn sign_stable(repo: &Path, bin: &Path, id: &str, say: &dyn Fn(String)) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let script = repo.join("tools/macos/sign-app.sh");
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg(&script).arg(bin).arg(id);
    let what = format!("signing {} as {id}", bin.display());
    let said = run_said(cmd, &what, SIGN_SAY, say);
    match said {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            eprintln!("update: {}", out.lines().last().unwrap_or("signed"));
        }
        Ok(o) => eprintln!("update: signing {} as {id} failed ({}); it starts as built: {}", bin.display(), o.status, String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")),
        Err(e) => eprintln!("update: signing {} as {id}: {e}; it starts as built", bin.display()),
    }
}

/// Runs `cmd` to its end (stdin null, its output kept) and says, once, when it runs past `after`:
/// what waits for it waits on. Nothing stops it (no wall-clock kills): a signing waited 36 min on
/// the Mac's keychain with nothing said, and the master took no devhub build all that time (11-16).
fn run_said(mut cmd: std::process::Command, what: &str, after: Duration, say: &dyn Fn(String)) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let started = Instant::now();
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut said = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if !said && started.elapsed() >= after {
            said = true;
            say(format!("update: {what} has run {} s (pid {}): most likely it waits on the Mac's keychain, locked or a prompt on its screen; the build waits until it ends", started.elapsed().as_secs(), child.id()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    if let Some(mut out) = child.stdout.take() { let _ = out.read_to_end(&mut stdout); }
    if let Some(mut err) = child.stderr.take() { let _ = err.read_to_end(&mut stderr); }
    if said {
        say(format!("update: {what} ended after {} s ({status})", started.elapsed().as_secs()));
    }
    Ok(std::process::Output { status, stdout, stderr })
}

/// One devhub build at a time ([`build_devhub`], [`build_master_app`]).
static BUILDING: Mutex<()> = Mutex::new(());

/// The devhub app a master runs as ([`MasterBin::App`]), built where it runs from
/// ([`APP_TARGET`]): a background build on 4 jobs, as [`build_devhub`], with its login item's
/// wrapper's exact command (the same fingerprint: neither rebuilds the other's). Cargo replaces
/// the binary in place; the running master's image is not that file.
pub fn build_master_app(repo: &Path, say: &dyn Fn(String)) -> Result<(), String> {
    let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut cmd = crate::os::background(cargo_bin().as_os_str());
    cmd.args(["build", "-j4", "--profile", "host", "-p", APP_NAME, "--bin", APP_NAME, "--target-dir"]).arg(repo.join(APP_TARGET)).current_dir(repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null());
    match cmd.output() {
        Ok(o) if o.status.success() => {
            sign_stable(repo, &repo.join(APP_TARGET).join("host").join(APP_NAME), APP_SIGN_ID, say);
            Ok(())
        }
        Ok(o) => Err(format!("does not build: {}", build_failure(&o))),
        Err(e) => Err(format!("cargo: {e}")),
    }
}

/// Copies `built` beside `live` and renames it over: `live` is always a whole binary (one at a
/// time: [`build_devhub`] holds its build to one).
fn swap_in(built: &Path, live: &Path) -> Result<(), String> {
    let next = live.with_extension("next");
    std::fs::create_dir_all(live.parent().unwrap_or(Path::new("."))).map_err(|e| format!("{}: {e}", live.display()))?;
    std::fs::copy(built, &next).and_then(|_| std::fs::rename(&next, live)).map(|_| ()).map_err(|e| format!("swapping {} in at {}: {e}", built.display(), live.display()))
}

/// The executables' dir of the Dock wrapper's bundle the master app runs from on macOS
/// (`~/Applications/devhub.app`, which its LaunchAgent starts), when it is there.
fn app_bundle_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let dir = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join("Applications/devhub.app/Contents/MacOS");
    dir.is_dir().then_some(dir)
}

/// Where the master app's `build` is placed to run from: a copy inside its wrapper's bundle
/// (`bundle`, [`app_bundle_dir`]), else `build` where it is. A process whose image runs from
/// outside a bundle has no app identity (LaunchServices: no bundle id), so the menu bar showed
/// the master's status item as an empty pill without its glyph, and the bundle's Local Network
/// grant was not its own. That happened from 11-17, once the master started through its
/// LaunchAgent and no Dock launch lent it the wrapper's identity.
fn bundled(bin: MasterBin, build: &Path, bundle: Option<&Path>) -> Result<PathBuf, String> {
    match bundle.filter(|_| bin == MasterBin::App) {
        Some(dir) => {
            let at = dir.join(format!("{APP_NAME}{}", std::env::consts::EXE_SUFFIX));
            swap_in(build, &at)?;
            Ok(at)
        }
        None => Ok(build.to_path_buf()),
    }
}

/// The image a master's restart or rollback execs: [`bundled`], or `build` where it is when the
/// copy fails (said: it starts, without the bundle's identity).
fn exec_image(ctx: &Arc<Ctx>, bin: MasterBin, build: &Path) -> PathBuf {
    bundled(bin, build, app_bundle_dir().as_deref()).unwrap_or_else(|e| {
        event(ctx, Sev::Warn, format!("update: {e}; the master runs {} where it is, without its bundle's identity (its menu-bar item shows no glyph)", build.display()));
        build.to_path_buf()
    })
}

/// The master build a restart replaced, kept for `devhub master rollback` (beside the binary this
/// master runs as, [`MasterBin::live`]).
fn prev_binary(repo: &Path) -> PathBuf {
    let live = MasterBin::this().live(repo);
    let name = live.file_name().map(|n| format!("{}.prev", n.to_string_lossy())).unwrap_or_default();
    live.with_file_name(name)
}

/// The lanes' CLI a restart replaced ([`swap_cli`]), the one that went with the master build it
/// left ([`prev_binary`]): a rollback puts it back ([`cli_back`]).
fn prev_cli(repo: &Path) -> PathBuf {
    // (not `.prev`: a master that runs as the CLI keeps its own previous build under that name)
    repo.join("target/host").join(format!("{}.prev-cli", cli_name()))
}

/// At a rollback: the lanes' CLI back to the build kept with the master's previous one, as a
/// restart moves it forward ([`swap_cli`]). A rollback left the newer CLI in place, and a CLI ahead
/// of its master sends words the master does not know: a rollback below the hand-in's `pin=` word
/// would have refused every hand-in (#8853, 11-24). Ok(false): none kept (a master restarted
/// before CLIs were kept), the CLI stays.
fn cli_back(repo: &Path) -> Result<bool, String> {
    let _one = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    let kept = prev_cli(repo);
    if !kept.is_file() { return Ok(false); }
    swap_in(&kept, &repo.join("target/host").join(cli_name()))?;
    Ok(true)
}

/// The build this master started from, copied as it starts: a CLI rebuild swaps a new file in at
/// the path it ran from, and a rollback needs the build that ran.
fn started_binary(repo: &Path) -> PathBuf {
    prev_binary(repo).with_extension("running")
}

/// At the master's start: its own binary, kept as [`started_binary`] (a master that runs the
/// rollback copy keeps none: [`prev_binary`] is that build already).
pub fn keep_started_binary(repo: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("update: the master's binary: {e}"))?;
    let started = started_binary(repo);
    if exe.canonicalize().ok() == prev_binary(repo).canonicalize().ok() {
        let _ = std::fs::remove_file(&started);
        return Ok(());
    }
    std::fs::copy(&exe, &started).map(|_| ()).map_err(|e| format!("update: keeping {} as {}: {e}", exe.display(), started.display()))
}

/// Before a restart: the build that runs becomes [`prev_binary`] (what it started from, or its
/// file now when it kept none; a master that runs that copy already, after a rollback, keeps it).
fn keep_running_binary(repo: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("update: the running master's binary: {e}"))?;
    let prev = prev_binary(repo);
    if exe.canonicalize().ok() == prev.canonicalize().ok() {
        return Ok(());
    }
    let started = started_binary(repo);
    let kept = std::fs::copy(if started.is_file() { &started } else { &exe }, &prev).map(|_| ());
    kept.map_err(|e| format!("update: keeping {} as {}: {e} (no restart without a build to roll back to)", exe.display(), prev.display()))?;
    // the commit it is of: the build this master runs ([`rev_stamp`])
    let stamp = rev_stamp(&prev);
    let _ = std::fs::remove_file(&stamp);
    if let Err(e) = std::fs::write(&stamp, crate::lease::caps::hub_rev()) {
        eprintln!("update: {}: {e} (a rollback to it will not know its commit)", stamp.display());
    }
    Ok(())
}

/// The file beside a kept master build naming the commit it is of: a master a rollback execs into
/// it says that commit as its own (11-17: a rolled-back master took the checkout's HEAD for its
/// own, and `devhub master restart` refused it as "already on work's tip" while it ran a build two
/// landings older that panicked).
fn rev_stamp(bin: &Path) -> PathBuf {
    let mut name = bin.file_name().unwrap_or_default().to_os_string();
    name.push(".rev");
    bin.with_file_name(name)
}

/// The commit [`rev_stamp`] names for a kept build; empty when it names none.
fn stamped_rev(bin: &Path) -> String {
    std::fs::read_to_string(rev_stamp(bin)).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// `master-bad.tsv` in devhub's home: the master builds known bad, a line each (`commit`, wall ms,
/// why): one that panicked on its probation ([`guard_fresh_build`]) or that a rollback left. A
/// self-update never takes one again, nor does a probation's way out ([`remedy`]); the next
/// landing's build is taken. A hand `devhub master restart` may.
fn bad_path(home: &Path) -> PathBuf {
    home.join("master-bad.tsv")
}

fn mark_bad(home: &Path, rev: &str, why: &str) {
    if rev.is_empty() || bad_build(home, rev).is_some() {
        return;
    }
    let line = format!("{rev}\t{}\t{}\n", wall_ms(), why.replace(['\t', '\n', '\r'], " "));
    let wrote = std::fs::OpenOptions::new().create(true).append(true).open(bad_path(home)).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    if let Err(e) = wrote {
        eprintln!("update: {}: {e}", bad_path(home).display());
    }
}

/// Why the master build of `rev` is known bad, when it is ([`bad_path`]).
pub(super) fn bad_build(home: &Path, rev: &str) -> Option<String> {
    if rev.is_empty() {
        return None;
    }
    let all = std::fs::read_to_string(bad_path(home)).ok()?;
    all.lines().find_map(|l| {
        let mut f = l.splitn(3, '\t');
        (f.next()? == rev).then(|| f.nth(1).unwrap_or_default().to_string())
    })
}

/// Execs the master back into the build its last restart replaced ([`prev_binary`]), the same
/// args and pid: a deploy rollback when a new build misbehaves (memory, CPU), not a code revert.
/// The next `devhub master restart` builds work's tip again.
pub fn rollback_master(ctx: &Arc<Ctx>, by: &str) -> Result<String, String> {
    let _one = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
    let prev = prev_binary(&ctx.repo);
    if !prev.is_file() {
        return Err(format!("update: no previous master build at {} to roll back to", prev.display()));
    }
    let (from, back_to) = (crate::lease::caps::hub_rev(), stamped_rev(&prev));
    if !back_to.is_empty() && back_to == from {
        return Err(format!("update: the master runs its previous build {} already: there is none older to roll back to (`devhub master restart` builds work's tip)", super::short(from)));
    }
    // the build it leaves is not taken again by a self-update: the next landing's is
    mark_bad(&ctx.home, from, &format!("rolled back from ({by})"));
    let to = if back_to.is_empty() { "of a commit it does not know".to_string() } else { super::short(&back_to).to_string() };
    event(ctx, Sev::Warn, format!("update: the master rolls back from {} to its previous build {to} ({by})", super::short(from)));
    // the lanes' CLI goes back with it; one that cannot is said, and the rollback goes on: a master
    // that panics is the worse of the two
    match cli_back(&ctx.repo) {
        Ok(true) => {}
        Ok(false) => event(ctx, Sev::Warn, format!("update: no CLI was kept with the previous build: the lanes' CLI stays at {}'s; `devhub master restart` brings both onto work's tip", super::short(from))),
        Err(e) => event(ctx, Sev::Err, format!("update: the lanes' CLI could not go back with the master ({e}): it stays at {}'s until `devhub master restart`", super::short(from))),
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = std::process::Command::new(exec_image(ctx, MasterBin::this(), &prev));
    // it says the commit it runs (none when unknown: then it takes no self-update until a hand
    // restart), and is on probation too: a build that panics now as well is left for work's tip
    cmd.args(&args).current_dir(&ctx.repo).env(crate::lease::caps::BUILT_AT, &back_to).env(RESTARTED_MS, wall_ms().to_string());
    #[cfg(unix)]
    {
        crate::node::keep_control_over_exec(&mut cmd);
        let c2 = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-master-exec".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _held = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
            let e = crate::node::exec(&mut cmd);
            event(&c2, Sev::Err, format!("update: the master could not roll back: {e}; it keeps running {}", super::short(crate::lease::caps::hub_rev())));
        });
        Ok(format!("the master execs into its previous build {} now (the same pid)", prev.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        Err("update: roll the master back by hand (no exec on this OS)".to_string())
    }
}

/// How long a build a restart exec'd into is on probation ([`guard_fresh_build`]).
const PROBATION: Duration = Duration::from_secs(5 * 60);

/// How often a master on probation that found no build to go to looks at work's tip again.
const PROBATION_RETRY: Duration = Duration::from_secs(30);

/// The wall ms of the exec that put this master's image on a build: set by [`restart_master`] and
/// [`rollback_master`].
const RESTARTED_MS: &str = "DEVHUB_RESTARTED_MS";

/// A master a restart or a rollback exec'd into a build: for [`PROBATION`], a panic on any of its
/// threads marks that build bad ([`bad_path`]) and moves the master on by itself ([`remedy`]): back
/// to the build its restart replaced, else onto work's tip built fresh, else onto the next commit
/// of work once there is one. Each step is said in its log, as an Err event (the inbox) and in a
/// note for main. 11-17: a self-updated build panicked 5 s in under the scheduler's lock, and the
/// poisoned lock failed every beat and lease request; the build before it panicked the same way
/// once restored leases named a box not yet back, and only a build of the tip with the fix (main
/// stopped the master for launchd's wrapper to build) brought it back.
pub fn guard_fresh_build(ctx: &Arc<Ctx>) {
    let Some(at) = std::env::var(RESTARTED_MS).ok().and_then(|v| v.parse::<u64>().ok()) else { return };
    let Some(left) = PROBATION.checked_sub(Duration::from_millis(wall_ms().saturating_sub(at))) else { return };
    let c = ctx.clone();
    on_first_panic(left, move |why| {
        let me = crate::lease::caps::hub_rev();
        let by = format!("its build {} panicked {} s after it started: {why}", super::short(me), wall_ms().saturating_sub(at) / 1000);
        mark_bad(&c.home, me, &by);
        let tell = |line: String| {
            if let Err(e) = crate::notes::leave(&c.home, "main", &line) {
                eprintln!("update: a note for main: {e}");
            }
            event(&c, Sev::Err, line);
        };
        // a tip whose build failed here is not built again until work moves
        let mut failed: Option<String> = None;
        let mut waits = false;
        loop {
            let prev = prev_binary(&c.repo);
            let prev_rev = prev.is_file().then(|| stamped_rev(&prev));
            let work = crate::gitread::rev(&c.repo, "refs/heads/work");
            let bad = |rev: &str| failed.as_deref() == Some(rev) || bad_build(&c.home, rev).is_some();
            match remedy(me, prev_rev.as_deref(), work.as_deref(), &bad) {
                Remedy::Rollback => {
                    tell(format!("update: the master rolls back by itself: {by}"));
                    match rollback_master(&c, &by) {
                        Ok(_) => return,
                        Err(e) => tell(format!("update: the master's rollback after its build panicked failed: {e}")),
                    }
                }
                Remedy::Build(work) => {
                    tell(format!("update: the master builds work's tip {} and restarts on it by itself: {by}", super::short(&work)));
                    // (a failed build says why itself)
                    if restart_master(&c, &work, "its probation", true).is_ok() {
                        return;
                    }
                    failed = Some(work);
                }
                Remedy::Wait if !waits => {
                    waits = true;
                    tell(format!("update: the master stays on its build {}, which panicked ({by}): its previous build and work's tip are known bad or did not build; it restarts by itself on the next commit of work, or `devhub master restart`", super::short(me)));
                }
                Remedy::Wait => {}
            }
            std::thread::sleep(PROBATION_RETRY);
        }
    });
}

/// Where a master on probation goes once its build `me` panicked ([`guard_fresh_build`]).
#[derive(Debug, PartialEq)]
enum Remedy {
    /// back to the build its restart replaced ([`rollback_master`])
    Rollback,
    /// onto work's tip, built first ([`restart_master`])
    Build(String),
    /// nowhere yet: both are known bad (`bad`), or the same build
    Wait,
}

/// The build the restart replaced (`prev`: its commit, empty when unknown; none when there is no
/// such build) while that is another one not known bad; else work's tip, when that is another
/// commit not known bad; else none yet.
fn remedy(me: &str, prev: Option<&str>, work: Option<&str>, bad: &dyn Fn(&str) -> bool) -> Remedy {
    if prev.is_some_and(|p| p != me && !bad(p)) {
        return Remedy::Rollback;
    }
    match work {
        Some(w) if !w.is_empty() && w != me && !bad(w) => Remedy::Build(w.to_string()),
        _ => Remedy::Wait,
    }
}

/// Runs `act` once, on a thread of its own, with the first panic of any thread of this process
/// within `within` (`thread 'name' panicked at file:line: message`); the panic is said as before.
fn on_first_panic(within: Duration, act: impl FnOnce(String) + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(Some(tx));
    let until = Instant::now() + within;
    let said = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        said(info);
        if Instant::now() >= until {
            return;
        }
        let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let _ = tx.send(format!("thread '{thread}' panicked at {at}: {message}"));
    }));
    // (the act runs apart from the panicking thread, after it let go of what it held)
    let _ = std::thread::Builder::new().name("devhub-probation".into()).spawn(move || {
        if let Ok(why) = rx.recv_timeout(within) {
            act(why);
        }
    });
}

/// Why a cargo build failed: its first error and where it is (`error[E0425]: ... at
/// libs/x.rs:12:5`), else its last stderr lines, and always its exit status (a build killed by a
/// signal may print nothing: 11-03 a node said only "no output").
pub fn build_failure(o: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&o.stderr);
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let why = match lines.iter().position(|l| l.starts_with("error")) {
        Some(i) => {
            let at = lines[i + 1..].iter().take(3).find_map(|l| l.strip_prefix("--> ")).map(|a| format!(" at {a}")).unwrap_or_default();
            format!("{}{at}", lines[i])
        }
        None if lines.is_empty() => "no output".to_string(),
        None => lines[lines.len().saturating_sub(3)..].join(" | "),
    };
    crate::proto::clean(&format!("{why} ({})", o.status), 400)
}

#[cfg(test)]
mod tests {
    const PANIC_OUT: &str = "DEVHUB_TEST_PANIC_OUT";

    /// The probation role: a thread panics twice within the window; the act writes what it heard.
    #[test]
    fn probation_role() {
        let Some(out) = std::env::var_os(PANIC_OUT) else { return };
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        super::on_first_panic(std::time::Duration::from_secs(60), move |why| { let _ = tx.send(why); });
        for n in 1..=2 {
            let _ = std::thread::Builder::new().name("devhub-panicky".into()).spawn(move || panic!("boom {n}")).unwrap().join();
        }
        let first = rx.recv().unwrap();
        let again = rx.recv_timeout(std::time::Duration::from_millis(300)).is_ok();
        std::fs::write(out, format!("{first}\nagain={again}\n")).unwrap();
    }

    /// A master on probation after its restart hears the first panic of any of its threads, once,
    /// with the thread and where it panicked: what it rolls back by (11-17: a self-updated build
    /// panicked under the scheduler's lock, and every lease request failed until a hand rollback).
    #[test]
    fn a_build_on_probation_acts_on_its_first_panic_of_any_thread() {
        let dir = std::env::temp_dir().join(format!("devhub-probation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("heard");
        let status = crate::testenv::role("orch::roll::tests::probation_role", &dir).env(PANIC_OUT, &out)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
        let heard = std::fs::read_to_string(&out).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(status.success(), "{status}: {heard}");
        assert!(heard.starts_with("thread 'devhub-panicky' panicked at ") && heard.contains("roll.rs:") && heard.contains(": boom 1\n"), "{heard}");
        assert!(heard.ends_with("again=false\n"), "once: {heard}");
    }

    /// A box back on the network is pushed to the tips when its deployment lacks them; one that
    /// stayed up, one already there, and one with no deployment yet are not.
    #[test]
    fn a_box_back_on_the_network_behind_the_tips_is_pushed() {
        let was_up: std::collections::BTreeSet<String> = ["stayed".to_string()].into();
        let up: Vec<(String, String)> = [("stayed", "old"), ("back", "old"), ("current", "tip"), ("fresh", "")].iter().map(|(n, d)| (n.to_string(), d.to_string())).collect();
        let pushed: Vec<&str> = super::back_behind(&was_up, &up, |d| d != "tip").iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(pushed, vec!["back"]);
    }

    /// A land-rule binary is signed outside the build lock: a signing that waits on the keychain
    /// holds no devhub build (one waited 36 min, and the master took none of three landings, 11-16).
    /// Signed, it takes the built one's place with its stamp and goes live.
    #[test]
    fn a_land_rule_binary_is_signed_outside_the_build_lock() {
        let d = std::env::temp_dir().join(format!("devhub-gate-sign-{}-{}", std::process::id(), crate::proto::wall_ms()));
        let target = super::MasterBin::this().cli_target(&d);
        let name = crate::gate::bin(&d).file_name().unwrap().to_owned();
        std::fs::create_dir_all(target.join("host")).unwrap();
        std::fs::write(target.join("host").join(&name), b"built").unwrap();
        super::finish_gate(&d, &target, "abc", |staged| {
            assert!(super::BUILDING.try_lock().is_ok(), "the build lock is free while it signs");
            std::fs::write(staged, b"signed").unwrap();
        }).unwrap();
        assert_eq!(std::fs::read(crate::gate::bin(&d)).unwrap(), b"signed", "the signed binary is live");
        assert_eq!(std::fs::read(target.join("host").join(&name)).unwrap(), b"signed", "and in the build dir, for the next swap");
        assert_eq!(std::fs::read_to_string(super::gate_stamp(&crate::gate::bin(&d))).unwrap(), "abc", "with its stamp");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A run past its time is said once, and its end too; it is never stopped. A quick one says
    /// nothing.
    #[cfg(unix)]
    #[test]
    fn a_long_signing_is_said_once_and_left_to_end() {
        let said = std::cell::RefCell::new(Vec::new());
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 0.3; echo signed"]);
        let out = super::run_said(cmd, "signing x", std::time::Duration::from_millis(50), &|l| said.borrow_mut().push(l)).unwrap();
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "signed", "it runs to its end: {out:?}");
        let said = said.into_inner();
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("update: signing x has run 0 s (pid ") && said[0].contains("keychain"), "{said:?}");
        assert!(said[1].starts_with("update: signing x ended after 0 s"), "{said:?}");
        let quick = super::run_said(std::process::Command::new("true"), "signing y", std::time::Duration::from_secs(60), &|l| panic!("a quick run says nothing: {l}"));
        assert!(quick.unwrap().status.success());
    }

    /// A restart keeps the running build beside the new one; a master that runs that copy keeps it.
    /// The lanes' CLI it replaces is kept with it, and a rollback puts that CLI back: a newer CLI
    /// left with a rolled-back master sends words it does not know (#8853's `pin=`, 11-24).
    #[test]
    fn the_previous_master_build_is_kept_for_a_rollback() {
        let d = std::env::temp_dir().join(format!("devhub-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target/host")).unwrap();
        super::keep_running_binary(&d).unwrap();
        let prev = super::prev_binary(&d);
        assert_eq!(std::fs::read(&prev).unwrap(), std::fs::read(std::env::current_exe().unwrap()).unwrap());
        // the build it started from wins over whatever replaced its file since
        std::fs::write(super::started_binary(&d), b"the build that ran").unwrap();
        super::keep_running_binary(&d).unwrap();
        assert_eq!(std::fs::read(&prev).unwrap(), b"the build that ran");
        assert_eq!(super::stamped_rev(&prev), crate::lease::caps::hub_rev(), "it names the commit of the build that runs, for a rollback to say");
        super::keep_started_binary(&d).unwrap();
        assert_eq!(std::fs::read(super::started_binary(&d)).unwrap(), std::fs::read(std::env::current_exe().unwrap()).unwrap());
        assert!(super::keep_running_binary(std::path::Path::new("/nonexistent-devhub-repo")).is_err(), "no restart without a build to roll back to");
        let cli = d.join("target/host").join(super::cli_name());
        std::fs::write(&cli, b"old cli").unwrap();
        assert_eq!(super::cli_back(&d), Ok(false), "none kept: the CLI stays");
        assert_eq!(std::fs::read(&cli).unwrap(), b"old cli");
        let built = super::MasterBin::this().cli_target(&d).join("host").join(super::cli_name());
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"new cli").unwrap();
        super::swap_cli(&d).unwrap();
        assert_eq!((std::fs::read(&cli).unwrap(), std::fs::read(super::prev_cli(&d)).unwrap()), (b"new cli".to_vec(), b"old cli".to_vec()), "a restart keeps the CLI it replaces");
        assert_eq!(super::cli_back(&d), Ok(true));
        assert_eq!(std::fs::read(&cli).unwrap(), b"old cli", "a rollback puts the CLI of the previous build back");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 11-17: a self-updated build panicked under the scheduler's lock, the build before it the
    /// same way: each is marked bad, the master goes back once, then onto work's tip built fresh,
    /// and with nowhere left to go it waits for the next commit of work. A self-update never takes
    /// a marked build again.
    #[test]
    fn a_master_build_that_panics_on_probation_moves_on_and_is_never_taken_again() {
        use super::Remedy;
        let home = std::env::temp_dir().join(format!("devhub-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let bad = |rev: &str| super::bad_build(&home, rev).is_some();
        // the self-update's build "new" panics: back to "old", which is not known bad
        super::mark_bad(&home, "new", "thread 'a' panicked at\tx.rs:1: boom");
        assert_eq!(super::bad_build(&home, "new").as_deref(), Some("thread 'a' panicked at x.rs:1: boom"), "the why, kept on one line");
        assert_eq!(super::remedy("new", Some("old"), Some("new"), &bad), Remedy::Rollback);
        // "old" panics as well: work's tip "new" is bad, so it waits; a landing moves work to "fix"
        super::mark_bad(&home, "old", "boom");
        super::mark_bad(&home, "old", "again");
        assert_eq!(std::fs::read_to_string(super::bad_path(&home)).unwrap().lines().count(), 2, "a build is marked once");
        assert_eq!(super::remedy("old", Some("old"), Some("new"), &bad), Remedy::Wait, "never back onto itself or onto a bad tip");
        assert_eq!(super::remedy("old", Some("old"), Some("fix"), &bad), Remedy::Build("fix".into()));
        assert_eq!(super::remedy("new", Some("old"), Some("fix"), &bad), Remedy::Build("fix".into()), "a bad previous build is passed over");
        assert_eq!(super::remedy("fix", Some("old"), Some("fix"), &bad), Remedy::Wait, "a tip that panics too waits for the next");
        // commits nobody knows are no build of their own and nothing to mark
        assert_eq!(super::remedy("", Some(""), None, &bad), Remedy::Wait, "an unknown build is not rolled back onto itself");
        assert_eq!(super::remedy("new", Some(""), None, &bad), Remedy::Rollback, "a previous build of unknown commit is not known bad");
        assert_eq!(super::remedy("new", None, Some(""), &bad), Remedy::Wait);
        super::mark_bad(&home, "", "boom");
        assert_eq!(super::bad_build(&home, ""), None);
        assert_eq!(super::bad_build(&home, "fix"), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The master app runs from a copy inside its wrapper's bundle (an app identity for its menu-bar
    /// item, 11-17), a whole binary each time; the CLI, and a Mac without the wrapper, run the build
    /// where it is.
    #[test]
    fn the_master_app_runs_from_inside_its_bundle() {
        use super::MasterBin;
        let d = std::env::temp_dir().join(format!("devhub-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let macos = d.join("devhub.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let build = d.join("makepad-app-devhub.prev");
        std::fs::write(&build, b"build 1").unwrap();
        let at = super::bundled(MasterBin::App, &build, Some(&macos)).unwrap();
        assert_eq!(at, macos.join(format!("makepad-app-devhub{}", std::env::consts::EXE_SUFFIX)), "the app's own name, whatever the build's");
        assert_eq!(std::fs::read(&at).unwrap(), b"build 1");
        std::fs::write(&build, b"build 2").unwrap();
        assert_eq!(std::fs::read(super::bundled(MasterBin::App, &build, Some(&macos)).unwrap()).unwrap(), b"build 2", "each restart replaces it");
        assert_eq!(std::fs::read_dir(&macos).unwrap().count(), 1, "no copy left beside it");
        assert_eq!(super::bundled(MasterBin::Cli, &build, Some(&macos)).unwrap(), build, "the CLI runs where it is built");
        assert_eq!(super::bundled(MasterBin::App, &build, None).unwrap(), build, "no wrapper: where it is built");
        assert!(super::bundled(MasterBin::App, &d.join("missing"), Some(&macos)).is_err(), "a failed copy is said");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A master execs into what it runs as: devhub's own binary at `target/host`, or the devhub app
    /// where its build puts it; the app rolls for its own crate besides libs/devhub's graph.
    #[test]
    fn a_master_execs_into_the_build_of_what_it_runs_as() {
        use super::MasterBin;
        use std::path::Path;
        let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let repo = Path::new("/r");
        assert_eq!(MasterBin::of(&repo.join("target/host").join(exe("makepad-devhub"))), MasterBin::Cli);
        assert_eq!(MasterBin::of(&repo.join("target/devhub-app/host").join(exe("makepad-app-devhub"))), MasterBin::App);
        assert_eq!(MasterBin::Cli.live(repo), repo.join("target/host").join(exe("makepad-devhub")));
        assert_eq!(MasterBin::App.live(repo), repo.join("target/devhub-app/host").join(exe("makepad-app-devhub")));
        // the CLI builds beside the app, which is built on the same lib
        assert_eq!(MasterBin::App.cli_target(repo), repo.join("target/devhub-app"));
        assert_eq!(MasterBin::Cli.cli_target(repo), repo.join("target/devhub"));
        let devhub = std::collections::BTreeSet::from(["libs/devhub".to_string(), "libs/devhub_conf".to_string()]);
        assert_eq!(MasterBin::Cli.dirs(&devhub), devhub);
        // the CLI binary: the master's own build, or its restart's; one its wrapper started builds it
        assert_eq!(super::cli_built_at(MasterBin::Cli, "c1", false), "c1");
        assert_eq!(super::cli_built_at(MasterBin::App, "c1", true), "c1");
        assert_eq!(super::cli_built_at(MasterBin::App, "c1", false), super::CLI_UNKNOWN);
        assert!(super::touches(repo, super::CLI_UNKNOWN, "c1", &devhub), "an unknown build is one to make");
        let app = MasterBin::App.dirs(&devhub);
        assert!(app.contains("apps/devhub") && devhub.iter().all(|d| app.contains(d)), "{app:?}");
    }

    /// The lanes' CLI takes the master's newest build only as the master execs into it: a build
    /// leaves `target/host` alone, the swap puts it there, and with none built the CLI stays. Swapped
    /// in as its build ended, a landing's CLI asked the old master for `stats route` for 8 min (11-15).
    #[test]
    fn the_cli_moves_with_the_master_s_restart() {
        let d = std::env::temp_dir().join(format!("devhub-cli-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let live = d.join("target/host").join(super::cli_name());
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        std::fs::write(&live, b"old build").unwrap();
        super::swap_cli(&d).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"old build", "nothing built: the CLI stays");
        let built = super::MasterBin::this().cli_target(&d).join("host").join(super::cli_name());
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"new build").unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"old build", "a build alone leaves the CLI as it is");
        super::swap_cli(&d).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new build", "the restart's swap puts the build in place");
        // the land rule's binary built with it goes live with it, with the commit it is of
        let gate = built.with_file_name(crate::gate::bin(&d).file_name().unwrap());
        std::fs::write(&gate, b"rule build").unwrap();
        std::fs::write(super::gate_stamp(&gate), b"c7").unwrap();
        assert!(!crate::gate::bin(&d).exists(), "a build alone leaves the rule as it is");
        super::swap_cli(&d).unwrap();
        assert_eq!(std::fs::read(crate::gate::bin(&d)).unwrap(), b"rule build", "the rule moves with the CLI");
        assert_eq!(std::fs::read(super::gate_stamp(&crate::gate::bin(&d))).unwrap(), b"c7", "and its stamp");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The land rule's binary is built on its own when none is live (tried again only after a pause
    /// when its build fails), and when the private checkout moved under its package.
    #[test]
    fn the_land_rule_binary_is_built_when_missing_or_its_source_moved() {
        use std::time::{Duration, Instant};
        let never = || panic!("not asked");
        assert!(super::gate_due(false, "", "h", never, None), "none live, never tried: build");
        assert!(!super::gate_due(false, "", "h", never, Some(Instant::now())), "a failed build is not tried every tick");
        let long_ago = Instant::now().checked_sub(super::GATE_RETRY + Duration::from_secs(1));
        assert!(long_ago.is_none() || super::gate_due(false, "", "h", never, long_ago), "but again after the pause");
        assert!(!super::gate_due(true, "h", "h", never, None), "live at the checkout's commit: nothing");
        assert!(!super::gate_due(true, "", "h", never, None), "live from devhub's own build without a stamp: nothing");
        assert!(super::gate_due(true, "g", "h", || true, None), "the checkout moved under the package: build");
        assert!(!super::gate_due(true, "g", "h", || false, None), "it moved elsewhere: nothing");
    }

    /// The live binary is replaced whole: the old one or the new one, never a partial file.
    #[test]
    fn a_built_binary_is_swapped_in_whole() {
        let d = std::env::temp_dir().join(format!("devhub-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("own/host")).unwrap();
        let (built, live) = (d.join("own/host/bin"), d.join("host/bin"));
        std::fs::write(&built, b"new build").unwrap();
        std::fs::create_dir_all(d.join("host")).unwrap();
        std::fs::write(&live, b"old build").unwrap();
        let held = std::fs::File::open(&live).unwrap();
        super::swap_in(&built, &live).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new build");
        // a caller that had the old file open keeps reading the old build
        let mut old = String::new();
        std::io::Read::read_to_string(&mut &held, &mut old).unwrap();
        assert_eq!(old, "old build");
        assert!(!live.with_extension("next").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A failed build says its first error and where, else its last lines, with its exit status.
    #[cfg(unix)]
    #[test]
    fn a_failed_build_says_its_first_error_and_how_it_ended() {
        use std::os::unix::process::ExitStatusExt;
        let out = |stderr: &str, raw: i32| std::process::Output { status: std::process::ExitStatus::from_raw(raw), stdout: Vec::new(), stderr: stderr.as_bytes().to_vec() };
        let err = "   Compiling makepad-devhub v0.1.0\nerror[E0425]: cannot find value `X` in module `super`\n   --> libs/devhub/src/lease/native.rs:122:40\n    |\nerror: could not compile `makepad-devhub`";
        assert_eq!(super::build_failure(&out(err, 101 << 8)), "error[E0425]: cannot find value `X` in module `super` at libs/devhub/src/lease/native.rs:122:40 (exit status: 101)");
        assert_eq!(super::build_failure(&out("   Compiling a\n   Compiling b\n   Compiling c\nlinker went away\n\n", 1 << 8)), "Compiling b | Compiling c | linker went away (exit status: 1)");
        let killed = super::build_failure(&out("", 9));
        assert!(killed.starts_with("no output (signal: 9"), "{killed}");
    }

    #[test]
    fn a_taken_back_room_holds_until_its_nodes_apply_ends() {
        let status = |busy: &str| Ok(vec![format!("{{\"machine\":\"box-a\",\"busy\":{busy},\"prewarm\":false}}")]);
        assert!(super::apply_ended(status("false")), "the node says no apply runs: the room goes");
        assert!(!super::apply_ended(status("true")), "its apply still runs: the room stays");
        assert!(!super::apply_ended(Err("box-a: unreachable".into())), "a node away (its hub restarting after the build) is asked again");
        assert!(!super::apply_ended(Ok(Vec::new())) && !super::apply_ended(Ok(vec!["{}".into()])), "an answer that says nothing keeps the room");
    }

    #[test]
    fn drains_stay_at_most_at_once_and_a_roll_gives_its_back() {
        let drains = std::sync::Arc::new(super::AtomicUsize::new(0));
        let slot = || super::Slot { drains: drains.clone(), held: false };
        // (one box at a time)
        let (mut a, mut b) = (slot(), slot());
        assert!(a.try_take() && a.try_take());
        assert!(!b.try_take());
        drop(a);
        assert!(b.try_take());
    }

    /// The master takes each devhub build it lacks as that build ends: no settle, no uptime (11-08:
    /// its restart build began a minute before a landing, and the next one waited out 10 min of
    /// uptime while the scheduler ran without it).
    #[test]
    fn the_master_takes_each_devhub_build_as_it_ends() {
        use std::time::Duration;
        let min = Duration::from_secs(60);
        assert_eq!(super::restart_blocker_of(false, false, false, None, Duration::ZERO), None, "a build that ended: at once");
        // a build of a newer tip runs: that one, as it ends
        assert!(super::restart_blocker_of(true, false, false, None, 9 * min).is_some_and(|w| w.contains("newer tip")));
        assert!(super::restart_blocker_of(false, true, false, None, 9 * min).is_some_and(|w| w.contains("sweep")));
        // an allapps run holds it for a while only
        let waits: Vec<bool> = (0..5).map(|m| super::restart_blocker_of(false, false, true, None, m * min).is_some()).collect();
        assert_eq!(waits, [true, true, true, false, false]);
        // a flash through it holds it until the flash ends, however long the wait
        assert!(super::restart_blocker_of(false, false, false, Some("a flash of board X runs through it".into()), 9 * min).is_some_and(|w| w.contains("flash of board X")));
    }

    /// A hub's build room is held only while the hub builds: an answer that waits on a sync's
    /// apply or a timed run gives it back, and it is taken again once the box's tree is free of
    /// both and of its deployment's build (both build in one tree). Its own build, and a
    /// deployment that moved under the ask, keep it (a hub room beside its box's deployment build
    /// held 4 cores for nothing, 11-13).
    #[test]
    fn a_hub_build_room_is_held_only_while_the_hub_builds() {
        let answer = |w: &str| format!("{{\"err\":\"waits: {w}\"}}");
        for (w, back) in [
            ("a sync runs on node-a (the hub restarts after it)", true),
            ("a timed lease runs on node-a: its new hub at 0123456789 builds after it", true),
            ("node-a's new hub builds at 0123456789 first", false),
            ("the deployment of node-a moved to 0123456789 (the master looked at 9876543210)", false),
            ("the overlay selftest of L1 runs in node-a's hub", false),
        ] {
            assert_eq!(super::gives_room_back(&answer(w)), back, "{w}");
        }
        assert_eq!(super::tree_busy_of(true, false, std::iter::empty()), Some("a sync's apply"));
        assert_eq!(super::tree_busy_of(false, false, [(false, false), (true, false)].into_iter()), Some("its deployment's build"));
        assert_eq!(super::tree_busy_of(false, true, [(false, false)].into_iter()), Some("its deployment's build in line"), "its room is not asked beside a deployment build in line");
        assert_eq!(super::tree_busy_of(false, false, [(false, true)].into_iter()), Some("a timed run"));
        assert_eq!(super::tree_busy_of(false, false, [(false, false)].into_iter()), None, "other work beside it: the hub builds");
    }

    /// The hubs follow the newest devhub build, not the master's restart (Mac hubs lagged the
    /// deployment for the master's 30 min uptime, 11-03); a hub with newer features works with it.
    #[test]
    fn hubs_roll_to_the_newest_build() {
        // a landing that leaves the master's code alone keeps the build's verdict; one after a
        // failed build does not
        let mut good = "a".to_string();
        super::carry_good(&mut good, "a", "b");
        assert_eq!(good, "b");
        super::carry_good(&mut good, "c", "d");
        assert_eq!(good, "b", "c never built: d is not known good");
    }

    /// A push a timed run makes wait is an update wait, not a failure.
    #[test]
    fn a_push_refused_for_a_timed_run_only_waits() {
        let refused = vec!["node-a {\"err\":\"waits: timed lease L287466 runs here: the deployment moves after it\"}".to_string()];
        assert_eq!(super::push_waits(&refused).as_deref(), Some("timed lease L287466 runs here: the deployment moves after it"));
        assert_eq!(super::push_waits(&["node-a {\"err\":\"push: connection reset\"}".to_string()]), None);
        assert_eq!(super::push_waits(&[r#"{"err":"waits: a timed lease runs on node-a: its new hub builds after it"}"#.to_string()]).as_deref(), Some("a timed lease runs on node-a: its new hub builds after it"));
    }

    #[test]
    fn a_service_rolls_for_what_cargo_builds_of_it() {
        let listing = "makepad-devhub v0.1.0 (/repo/libs/devhub)\nmakepad-remote v0.1.0 (/repo/libs/remote) (*)\nmacro-thing v0.1.0 (proc-macro) (/repo/libs/macro/impl)\nlibc v0.2.1\nother v0.1.0 (/elsewhere/other)\n";
        let dirs: Vec<String> = super::tree_dirs(listing, "/repo", '/').into_iter().collect();
        assert_eq!(dirs, ["libs/devhub", "libs/macro/impl", "libs/remote"]);
        // on Windows: backslashes, and a verbatim root cargo does not print
        let listing = "makepad-devhub v0.1.0 (C:\\wt\\repo\\libs\\devhub)\nmacro-thing v0.1.0 (proc-macro) (C:\\wt\\repo\\libs\\macro\\impl)\nother v0.1.0 (C:\\wt\\other)\n";
        let dirs: Vec<String> = super::tree_dirs(listing, "\\\\?\\C:\\wt\\repo", '\\').into_iter().collect();
        assert_eq!(dirs, ["libs/devhub", "libs/macro/impl"]);
        for file in ["src/lib.rs", "src/tests/a.rs", "Cargo.toml", "build.rs", ""] {
            assert!(super::builds(file), "{file}");
        }
        for file in ["ci.toml", "README.md", "src/notes.md", "tests/a.rs", "benches/b.rs", "examples/c/main.rs"] {
            assert!(!super::builds(file), "{file}");
        }
        // a service rolls once its code changed after it started and settled, or once its node
        // says its checkout rebuilt it, however late it started; never twice for one change
        let (at, settled) = (1_000_000u64, 1_000_000 + super::SETTLE_MS);
        assert!(super::service_due(at - 1, at, false, false, settled), "started before the change");
        assert!(!super::service_due(at - 1, at, false, false, settled - 1), "the change not settled yet");
        assert!(!super::service_due(at + 1, at, false, false, settled), "started after the change, on its build");
        assert!(super::service_due(at + 1, at, true, false, at + 2), "started after the change but before its box built it: rebuilt, it rolls at once");
        assert!(!super::service_due(at - 1, at, true, true, settled), "its node said it runs the deployment's build");
    }

    /// A landing cargo cannot read rolls, for whatever changed, and says why.
    #[test]
    fn a_landing_cargo_cannot_read_rolls_for_any_file() {
        let d = std::env::temp_dir().join(format!("devhub-roll-unread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("libs/a/tests")).unwrap();
        let g = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(&d).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}");
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let save = |from: &str| {
            for f in ["top.rs", "libs/a/x.rs", "libs/a/ci.toml", "libs/a/tests/t.rs"] {
                std::fs::write(d.join(f), from).unwrap();
            }
            g(&["add", "."]);
            g(&["commit", "-q", "-m", from]);
            g(&["rev-parse", "HEAD"])
        };
        g(&["init", "-q", "-b", "work"]);
        let (one, two) = (save("1"), save("2"));
        let (dirs, why) = super::service_dirs(&d);
        assert!(why.as_deref().is_some_and(|w| w.contains("the whole repo counts")), "{why:?}");
        for (svc, _, _) in super::SERVICES {
            let mut changed = super::built_changes(&d, &one, &two, &dirs[svc]).unwrap();
            changed.sort();
            assert_eq!(changed, ["libs/a/tests/t.rs", "libs/a/x.rs", "top.rs"], "{svc}: the whole repo, less what no crate reads");
            assert!(super::touches(&d, &one, &two, &dirs[svc]) && !super::touches(&d, &two, &two, &dirs[svc]));
            assert!(super::last_change_ms(&d, &two, &dirs[svc]) > 0, "{svc}");
        }
        // a lane's change counts from its landing (the merge into work), not from its own commit
        g(&["checkout", "-q", "-b", "lane"]);
        std::fs::write(d.join("libs/a/x.rs"), "3").unwrap();
        g(&["add", "."]);
        let o = std::process::Command::new("git").arg("-C").arg(&d).args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "lane"]).env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z").env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z").output().unwrap();
        assert!(o.status.success());
        g(&["checkout", "-q", "work"]);
        g(&["merge", "-q", "--no-ff", "-m", "land", "lane"]);
        let landed = g(&["rev-parse", "HEAD"]);
        let at: u64 = g(&["log", "-1", "--format=%ct", &landed]).parse().unwrap();
        for (svc, _, _) in super::SERVICES {
            assert_eq!(super::last_change_ms(&d, &landed, &dirs[svc]), at * 1000, "{svc}: the landing's time, past the lane commit's");
        }
        // a hub behind the deployment its box has builds there while that cannot move; one at it or
        // past it, or an older hub that says no commit, does not
        let hub = &dirs["devhub"];
        assert!(super::hub_behind_deployment(&d, &one, &two, hub), "the hub at one, its box deployed at two");
        assert!(!super::hub_behind_deployment(&d, &two, &two, hub) && !super::hub_behind_deployment(&d, &landed, &two, hub), "at its deployment or past it");
        assert!(!super::hub_behind_deployment(&d, "", &two, hub) && !super::hub_behind_deployment(&d, &one, "", hub));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The rule's point: what a hub builds of devhub is far from its panel's widgets.
    #[test]
    fn devhub_rolls_for_its_own_graph_and_not_its_panels() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let d = super::linked_dirs(&repo, "libs/devhub", "").unwrap();
        assert!(d.contains("libs/devhub") && d.contains("libs/remote") && d.contains("libs/strict_json"), "{d:?}");
        for not_linked in ["libs/ai/hub", "libs/ci_judge"] {
            assert!(!d.contains(not_linked), "{not_linked}: the hubs would roll for AI hub edits: {d:?}");
        }
        for not_built in ["apps/devhub", "widgets", "draw", "platform"] {
            assert!(!d.contains(not_built), "{not_built}: {d:?}");
        }
        // the AI hub's graph as the WM builds it (`--features node`): a landing in a crate only
        // that feature builds rebuilt every box's AI hub, and the roll never restarted one (11-22)
        let (_, dir, features) = super::SERVICES[1];
        let hub = super::linked_dirs(&repo, dir, features).unwrap();
        assert!(hub.contains("apps/aihub") && hub.contains("libs/render") && hub.contains("libs/scene"), "the node build's crates: {hub:?}");
    }

    #[test]
    fn a_roll_can_be_read_back_with_its_target_and_why() {
        assert_eq!(super::rolls_kept("short\n"), None);
        let long = format!("{}\nlast\n", "x".repeat(super::ROLLS_KEEP));
        assert_eq!(super::rolls_kept(&long), Some("last\n"));
        assert_eq!(super::why_of(&["a".into()]), "1 file: a");
        assert_eq!(super::why_of(&["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]), "5 files: a, b, c +2");
        let text = "{\"ms\":1,\"node\":\"n1\",\"svc\":\"devhub\",\"from\":\"aaaaaaaaaaaaaa\",\"to\":\"bbbbbbbbbbbbbb\",\"why\":\"1 file: a\"}\n{\"ms\":2,\"node\":\"n2\",\"svc\":\"devhub\",\"from\":\"a\",\"to\":\"b\",\"why\":\"w\"}\n{\"ms\":3,\"node\":\"n1\",\"svc\":\"devhub\",\"to\":\"bbbbbbbbbbbbbb\",\"ended\":\"runs bbbbbbbbbb\"}\nnot a whole li";
        let clock = |ms: u64| format!("t{ms}");
        assert_eq!(super::rolls_text(text, Some("n1"), 10, &clock), "t1 n1 devhub: aaaaaaaaaa -> bbbbbbbbbb: 1 file: a\nt3 n1 devhub: bbbbbbbbbb ended: runs bbbbbbbbbb");
        // a box's WM restart, as its hub said it, is a `wm` line with its wait (11-24)
        let rec = |kind: &str, line: &str| crate::proto::hub_record("n1", "", 0, kind, crate::proto::Sev::Info, line.into(), String::new());
        let said = "sync: WM restarted (makepad-wm.service active): the build replaced the WM binary, after 45 s waiting on: test lease L1 of lane-h (its windows)";
        let ended = super::wm_restarted(&rec("sync", said)).unwrap();
        assert_eq!(ended, "restarted (makepad-wm.service active): the build replaced the WM binary, after 45 s waiting on: test lease L1 of lane-h (its windows)");
        assert_eq!(super::wm_restarted(&rec("sync", "sync: the WM restart (the build replaced the WM binary) waits for: test lease L1 of lane-h (its windows)")), None, "a wait's start is no restart");
        assert_eq!(super::wm_restarted(&rec("hub", said)), None, "only the hub's sync says it");
        let wm = format!("{{\"ms\":4,\"node\":\"n1\",\"svc\":\"wm\",\"ended\":{}}}\n", makepad_remote::json_str(&ended));
        assert_eq!(super::rolls_text(&format!("{}\n{wm}", text.trim_end()), Some("n1"), 1, &clock), format!("t4 n1 wm: {ended}"));
        assert_eq!(super::rolls_text(text, None, 1, &clock), "t3 n1 devhub: bbbbbbbbbb ended: runs bbbbbbbbbb");
        // the record says what each hub runs: one seen elsewhere gets its own line, once
        let runs = super::hubs_run(text);
        assert_eq!((runs.get("n1").map(String::as_str), runs.get("n2")), (Some("bbbbbbbbbb"), None), "{runs:?}");
        assert!(super::runs_known("bbbbbbbbbb", "bbbbbbbbbbbbbb"), "the hub its last line names");
        assert!(!super::runs_known("bbbbbbbbbb", "cccccccccccccc"), "moved with no line");
        assert!(!super::runs_known("", "cccccccccccccc"), "a box the record never named");
        assert!(super::runs_known("", ""), "a hub that names no commit");
    }

    #[test]
    fn closures_follow_path_and_workspace_deps() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let d = super::crate_dirs(&repo, "apps/devhub");
        assert!(d.contains("apps/devhub") && d.contains("libs/devhub") && d.contains("libs/remote") && d.contains("libs/strict_json"), "{d:?}");
        assert!(!d.contains("apps/wm"));
        assert_eq!(super::normalize("apps/devhub/../../libs/devhub"), "libs/devhub");
        assert_eq!(super::path_of(r#"makepad-remote = { path = "../remote", optional = true }"#).as_deref(), Some("../remote"));
    }

    /// A dependency's path is its `path` key's, wherever else "path" (or another key's name)
    /// stands in the line: in the dependency's name, a feature or a value. The name's "path" was
    /// read as the key, and no crate that used such a dependency held it among its inputs (11-07).
    #[test]
    fn a_dependency_path_is_its_key_not_a_word_of_its_name() {
        for (line, want) in [
            (r#"makepad-pathkit = { path = "../pathkit" }"#, Some("../pathkit")),
            (r#"pathfinder = { path="../pathfinder", version = "0.1" }"#, Some("../pathfinder")),
            (r#"version-path = { version = "1", features = ["path"], path = "../vp" }"#, Some("../vp")),
            (r#"features-xpath = { features = ["xpath", "version"], optional = true, path = "../fx" }"#, Some("../fx")),
            (r#"path-features-version = {path = "../pfv",features = ["path"]}"#, Some("../pfv")),
            (r#"path = "src/lib.rs""#, Some("src/lib.rs")),
            (r#"makepad-pathkit.workspace = true"#, None),
            (r#"makepad-pathkit = { workspace = true }"#, None),
            (r#"xpath = { version = "1", features = ["path"] }"#, None),
            (r#"classpath = "x""#, None),
        ] {
            assert_eq!(super::path_of(line).as_deref(), want, "{line}");
        }
        // the closure follows each of them
        let dep = |name: &str, dir: &str| format!("[package]\nname = \"{name}\"\n[dependencies]\n{name}-dep = {{ version = \"1\", features = [\"path\"], path = \"../{dir}\" }}\n");
        let files = std::collections::BTreeMap::from([
            ("Cargo.toml".to_string(), "[workspace.dependencies]\nmakepad-pathkit = { path = \"pathkit\" }\n".to_string()),
            ("a/Cargo.toml".to_string(), "[package]\nname = \"a\"\n[dependencies]\nmakepad-pathkit = { path = \"../pathkit\" }\nversion-path = { version = \"1\", path = \"../vp\" }\n".to_string()),
            ("pathkit/Cargo.toml".to_string(), dep("makepad-pathkit", "features-path")),
            ("vp/Cargo.toml".to_string(), "[package]\nname = \"version-path\"\n".to_string()),
            ("features-path/Cargo.toml".to_string(), "[package]\nname = \"features-path\"\n".to_string()),
            ("b/Cargo.toml".to_string(), "[package]\nname = \"b\"\n[dependencies]\nmakepad-pathkit.workspace = true\n".to_string()),
        ]);
        let read = |p: &str| files.get(p).cloned();
        let dirs = |pkg: &str| super::crate_dirs_from(pkg, &read).into_iter().collect::<Vec<_>>();
        assert_eq!(dirs("a"), ["a", "features-path", "pathkit", "vp"]);
        assert_eq!(dirs("b"), ["b", "features-path", "pathkit"]);
    }
}
