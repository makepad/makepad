//! Buckets check landed tips on their own timers.
//! One run covers all landings since that bucket's last tip.
//! Failures name the lanes whose inputs changed; they never control landing.
//! A unit failing the way it failed at the bucket's last look at it is a break already on the
//! branch: it names who broke it then, not the later landings that changed its inputs.
//! A failed unit stays red until it passes: the next run of its bucket looks at it again whatever
//! that run's diff reaches, and a run that did not get to it is no pass.
//! A restarted master adopts its bucket's unfinished run instead of starting it again.
//! When a bucket is due comes from what outlives the master: its last start (its ledger, or the
//! runs of its kind it knows of) and the landings since. A master restart never moves it: CI was
//! due again at each of them (every ~30 min) and never ran for 8 h (11-02).
//! The CI bucket runs once a night ([`Timing::daily`]): a run every ~3 h of a moving tip proved
//! little a night's run of the day's tip does not, and took the fleet's day (user, 11-05).
//! A run whose lease was lost under it (expired, its node lost it) has no verdict: it runs once
//! more at once, and lost again it is no verdict on the landings, only a warn: the bucket takes the
//! tip again at its next turn.
//! The full bucket ([`full`]) runs every package's tests with no proof reused, once a day before
//! the daily push review, early on a quiet lease queue ([`Timing::quiet_queue`]) and on demand
//! (`devhub bucket run full`): the tests bucket runs what the
//! landings reach and takes passes as proofs, and a red no landing's reach caught stayed unseen
//! (makepad-script's, 11-07). It reports; it gates nothing.
//! `devhub bucket run <bucket>` starts a bucket's run at once, of the tip then ([`kick`]).
//! A tests or full run's shards take the pool's slots in turn ([`BOARD`]), shard by shard: the
//! next run starts on the first box that frees. A waiting shard says why in the master's log as
//! it starts to wait and every 10 min, `devhub bucket` shows each bucket running on how many slots
//! since when, or queued and why, and a master restart gives each slot back to its shard.

use crate::land::{Entry, St};
use crate::lease::sched::NIGHT_END_HOUR;
use crate::lease::Pin;
use crate::proto::{hub_record, wall_ms, Sev};
use crate::state::{Pair, Store};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::land::Publish;

pub struct Timing {
    /// Least time between starts.
    pub every: Duration,
    /// Wait for the tip to stay unchanged this long.
    pub quiet: Duration,
    /// Start anyway after being due this long.
    pub cap: Duration,
    /// Once a day at this hour of the local clock, after a landing it has not checked: in place
    /// of `every`, `quiet` and `cap`.
    pub daily: Option<u32>,
    /// Also once the lease queue stayed quiet this long (no lane's ask queued a minute,
    /// [`crate::lease::sched::Sched::queue_busy_ms`]), [`QUIET_RUN_GAP_MS`] after its last start at
    /// the soonest ([`quiet_queue_due`]).
    pub quiet_queue: Option<Duration>,
}

/// The least time between a bucket's starts on a quiet queue ([`Timing::quiet_queue`]).
const QUIET_RUN_GAP_MS: u64 = 12 * 3_600_000;

/// Whether a bucket starts now on a quiet queue ([`Timing::quiet_queue`]): `busy`, when a lane's
/// ask last queued a minute (None: no lease master to ask).
fn quiet_queue_due(t: &Timing, last_start: u64, now: u64, busy: Option<u64>) -> bool {
    let (Some(quiet), Some(busy)) = (t.quiet_queue, busy) else { return false };
    now >= last_start.saturating_add(QUIET_RUN_GAP_MS) && now.saturating_sub(busy) >= quiet.as_millis() as u64
}

impl Timing {
    /// Due at once after the last landing, waiting `quiet` for the tip to settle, at most `cap`.
    pub fn after(quiet: Duration, cap: Duration) -> Timing {
        Timing { every: Duration::ZERO, quiet, cap, daily: None, quiet_queue: None }
    }

    /// The fast bucket's: pipelined, a run of the newest tip at the first landing it has not
    /// checked, and the next one as a run ends. It waited for the landings to pause a minute (at
    /// most 10): at 11 landings in 90 min a quiet minute was rare, and a Mac show held for it
    /// (11-07).
    pub fn pipelined() -> Timing {
        Timing::after(Duration::ZERO, Duration::ZERO)
    }
}

/// When a bucket starts its next run: due at `every` after its last start and not before the
/// first landing it has not checked; then once the tip stayed `quiet` since it last changed, and
/// at the latest `cap` after it was due, quiet or not. All of them outlive the master.
fn starts_at(t: &Timing, last_start: u64, first_landed: u64, last_changed: u64) -> u64 {
    if let Some(hour) = t.daily {
        return daily_after(last_start.max(first_landed), hour, &|secs| makepad_civil_time::local_or_utc(secs).offset);
    }
    let ms = |d: Duration| d.as_millis() as u64;
    let due = last_start.saturating_add(ms(t.every)).max(first_landed);
    due.max(last_changed.saturating_add(ms(t.quiet))).min(due.saturating_add(ms(t.cap)))
}

/// The first `hour`:00 on the local clock after `after_ms` (`offset`: seconds east of UTC at an
/// instant), with the offset at that hour (a daylight saving change in between).
fn daily_after(after_ms: u64, hour: u32, offset: &dyn Fn(i64) -> i32) -> u64 {
    let after = (after_ms / 1000) as i64;
    let local = after + offset(after) as i64;
    let mut at = local.div_euclid(86_400) * 86_400 + hour as i64 * 3600;
    if at <= local {
        at += 86_400;
    }
    let utc = at - offset(at - offset(after) as i64) as i64;
    utc.max(0) as u64 * 1000
}

/// A bucket's last run that its ledger may not have: the start and tip of the newest run of its
/// kind (CI's runs survive a master restart; the bucket that asked for one did not).
pub type Prior = fn(&crate::state::Doc) -> Option<(u64, Pair)>;

/// When each landing landed.
fn landed_ms(store: &Store) -> Vec<u64> {
    store.read(|d| d.land.iter().filter(|v| v.get("state").and_then(Value::as_str) == Some(St::Landed.as_str()))
        .filter_map(|v| v.get("changed_ms")?.as_u64()).collect())
}

/// The start a bucket run's log is named after (`<bucket>-<ms>.log`).
pub(crate) fn log_started(log: &Path) -> Option<u64> {
    log.file_stem()?.to_str()?.rsplit_once('-')?.1.parse().ok()
}

/// Notes a bucket run beside its log (`.log.run`), for a restarted master to adopt.
pub(crate) fn save_run(log: &Path, pid: u32, started: u64, base: &Pin, tip: &Pin) -> std::io::Result<()> {
    let sidecar = log.with_extension("log.run");
    let value = obj(vec![
        ("pid", Value::Int(pid as i64)), ("started_ms", Value::Int(started as i64)),
        ("base", tip_value(base)), ("tip", tip_value(tip)),
    ]);
    let temp = sidecar.with_extension("run.tmp");
    std::fs::write(&temp, value.to_json()).and_then(|_| std::fs::rename(&temp, &sidecar))
}

pub struct Outcome {
    pub ok: bool,
    pub failing: Vec<String>,
    pub log: String,
    pub summary: String,
    /// why this run says nothing about the tip (its lease lost twice, a CI run that stopped before
    /// its legs ran): nothing here is a failure of the landings, and it runs again at its next turn
    pub no_verdict: Option<String>,
}

/// The log's line before a bucket runs its gate once more after a lost lease; an adopted run
/// whose log has it runs no third time.
const RERUN: &str = "bucket: its lease was lost under the gate (the fleet, not the code): running it once more";

/// A gate's log line where it said its start (`GATE <mode>: `) and exited 0 without its verdict
/// line, and runs once more as a lost lease's does ([`again`]): it said nothing of the landings. A
/// box's disk reclaim took the run log of a full run's libraries' sweep under it; the sweep ended 0
/// four seconds into its checks, and the run had no verdict beside 691 judged units (L1561802,
/// 11-22).
const RERUN_UNSAID: &str = "bucket: the gate exited 0 without its verdict line (it said nothing of the landings): running it once more";

/// The lines after which a gate's log holds its run once more.
const RERUN_LINES: [&str; 2] = [RERUN, RERUN_UNSAID];

/// A log's text before its last [`RERUN_LINES`] line, and after it.
fn rerun_cut(text: &str) -> Option<(&str, &str)> {
    RERUN_LINES.iter().filter_map(|m| text.rfind(&format!("{m}\n")).map(|i| (i, i + m.len() + 1))).max().map(|(i, j)| (&text[..i], &text[j..]))
}

/// The log's line of an adopted run whose holder ended with no exit and no verdict while no master
/// watched it: the master's box restarted under it, its holder and lease went with it, and the run
/// is a lost lease's ([`adopted_end`]).
const UNWATCHED: &str = "bucket: the gate's holder ended without its exit or a verdict while no master watched it (the master's box restarted under it): its lease went with it";

/// The status of an adopted run whose holder has ended (`file`: the exit it wrote, `code`: the one
/// the master saw), and whether to say [`UNWATCHED`]: one that ended unwatched, with no exit and no
/// verdict, is a lost lease's ([`crate::lease::client::LOST_EXIT`]) and runs once more ([`again`]).
/// The fast run in flight at the admin Mac's power cut read as no verdict and waited for the next
/// landing (11-17).
fn adopted_end(file: Option<i32>, code: Option<i32>, stopped_before: bool, said: bool) -> (Result<Option<i32>, String>, bool) {
    match file {
        Some(code) => (Ok(Some(code)), false),
        None if stopped_before => (Ok(Some(0)), false),
        None if code.is_none() && !said => (Ok(Some(crate::lease::client::LOST_EXIT)), true),
        None => (Ok(code), false),
    }
}

/// A bucket's run of `tip` (every followed repository's commit, [`crate::orch::snapshot_pin`])
/// since `base`, into its log.
pub type Run = Arc<dyn Fn(Option<&Pin>, &Pin, &Path) -> Outcome + Send + Sync>;

/// A bisect's run of its red unit (the first argument) at one pin against its last pass, into its log.
pub type Probe = Arc<dyn Fn(&str, Option<&Pin>, &Pin, &Path) -> Outcome + Send + Sync>;

const GATE_NEEDS: &str = "arch=x64,os=linux";

/// The fast and tests buckets' boxes: [`GATE_NEEDS`] on the master's rustc. agree.sh and Relax's
/// own suites compare with that rustc's std, so a box on another toolchain disagrees with the rest
/// (a box mid-update waits until it reports the master's release).
fn gate_needs() -> String {
    match crate::lease::caps::rustc_version() {
        v if v.is_empty() => GATE_NEEDS.to_string(),
        v => format!("{GATE_NEEDS},rustc_eq={v}"),
    }
}
static LEDGER: Mutex<()> = Mutex::new(());

/// The tests bucket's last [`TIMES_ROWS`] run walls in ms, newest first: seeded from the ledger
/// when its bucket starts ([`spawn`]) and given each run's as its row is written ([`append`]), so
/// the measure a full run's floor reads ([`tests_run_ms`]) is a lookup. None: not seeded yet.
static TESTS_WALLS: Mutex<Option<VecDeque<u64>>> = Mutex::new(None);
static BISECTS: Mutex<()> = Mutex::new(());
/// The pool's slots, one per tests shard its boxes take ([`pool_shape`]), passed shard by shard: a
/// run's shard takes one as its lease is asked and frees it as its lease ends, so the next run
/// starts on the first box that frees. The shards in flight, of every run, are never more than the
/// pool takes: a full run's three shards beside a tests run's took every pool box, and landing
/// gates waited behind the six (11-07). The whole pool per run for its whole length left two of
/// three boxes with no full shard for 65 min, while a tests run asked at 17:29 waited past 19:40
/// (11-12). A waiting shard says why ([`Board::slot`]), and `devhub bucket` shows each run on how
/// many slots since when.
static BOARD: Board = Board::new(wall_ms, pool_slots);

/// How often a shard waiting for a pool slot says it still waits.
const SLOT_SAID: Duration = Duration::from_secs(10 * 60);
/// How long the slots a restarted master read back ([`SLOTS_FILE`]) are kept for their shards:
/// a run's bucket adopts it as its thread starts, and each shard takes its slot back at once.
const SLOT_ADOPT: Duration = Duration::from_secs(2 * 60);
/// How often a waiting shard looks again without a release: whether its run still needs it.
const SLOT_LOOK: Duration = Duration::from_secs(5);
/// How often a waiting shard reads again how many shards the pool takes ([`pool_slots`]).
const POOL_LOOK: Duration = Duration::from_secs(60);
/// The held slots on disk beside the logs, a line each ([`Claim::line`]), written before a shard's
/// lease is asked and after it ends: a restarted master gives each back to its shard, whose lease
/// ran on, before a run that only waited takes it. Beside them each full shard's wait
/// (`wait <log> <shard> <since>`, [`Slots::waits`]) and each full run whose floor rose
/// (`starved <log>`): a restart forgot a floor 3 s after it rose and began every wait again, and
/// the floor came back 17 min later (12:50, 11-22). And how many shards the pool took at its last
/// measure (`pool <n>`): a restarted master's first look comes before any box said it is up, and
/// with no count a raised floor fell to one and no shard took a slot ("4 of ? pool slots held"
/// after each restart, 11-22). (A master before them reads no such line.)
const SLOTS_FILE: &str = "shards.slots";
/// The whole pool's turn before slots: removed as the board reads its slots.
const TURN_FILE: &str = "shards.turn";
/// The shard of a claim its run keeps for its next shard, none of its own ([`Slot`]'s drop):
/// shards count from 1.
const RUN_KEPT: usize = 0;

/// A shard's pool slot: a line of [`SLOTS_FILE`].
#[derive(Clone, Debug, PartialEq)]
struct Claim {
    /// its run's log
    log: PathBuf,
    shard: usize,
    tip: String,
    /// since when its run holds pool slots: its first slot's take
    since: u64,
    /// since when this shard holds this one
    taken: u64,
}

impl Claim {
    fn line(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}\n", self.log.display(), self.shard, self.since, self.taken, self.tip)
    }

    fn parse(line: &str) -> Option<Claim> {
        let mut field = line.split('\t');
        let log = PathBuf::from(field.next().filter(|l| !l.is_empty())?);
        Some(Claim { log, shard: field.next()?.parse().ok()?, since: field.next()?.parse().ok()?, taken: field.next()?.parse().ok()?, tip: field.next()?.to_string() })
    }

    fn of(&self, log: &Path, shard: usize) -> bool {
        self.log == log && self.shard == shard
    }
}

/// What a shard's lease needs of the pool's slots ([`lease_run`]).
#[derive(Clone, Copy, Debug)]
enum Need {
    /// a lease to ask: a slot, in turn
    Lease,
    /// its lease from before a master restart runs on: its slot back at once, or one beyond the
    /// pool's count with none on file (since its sidecar was written, its run's start)
    Running(Option<u64>),
    /// its lease from before a master restart has ended: no slot, and its slot on file is free
    Ended,
    /// its lease gave its slot to a run that goes first ([`Dealer::line`]): the slot is free, and
    /// it asks for one again in turn
    Gave,
}

/// A run on the board ([`Board::enter`]).
struct OnRun {
    tip: String,
    /// since when it holds pool slots: its first slot's take
    since: Option<u64>,
    /// why its shards wait, while they do
    why: Option<String>,
    /// a full run one of whose shards waited the tests bucket's measured run behind post-land tests
    /// runs ([`Slots::starve_ms`]): its floor is half the pool from then on ([`Slots::floor_n`])
    starved: bool,
}

/// A shard waiting for a pool slot.
struct Waiter {
    log: PathBuf,
    /// when its run was asked: the earliest goes first
    asked: u64,
    shard: usize,
    /// since when it waits: a full run's shard past [`Slots::starve_ms`] of it starves its run
    /// ([`Slots::floor_n`])
    since: u64,
}

/// Who holds the pool's slots and who waits for them ([`Board`]).
struct Slots {
    /// [`SLOTS_FILE`], once read ([`Slots::load`])
    file: Option<PathBuf>,
    held: Vec<Claim>,
    /// read back after a master restart and not taken up yet, since `kept_at`
    kept: Vec<Claim>,
    kept_at: u64,
    waiting: Vec<Waiter>,
    /// how many shards the pool takes, when that was read, and why it is not known
    pool: Option<usize>,
    pool_read: Option<u64>,
    pool_err: String,
    /// the tests bucket's measured run ([`tests_run_ms`]), read with the pool's count: how long a
    /// full run's shard waits behind post-land tests runs before its run keeps half the pool
    /// ([`Slots::floor_n`])
    starve_ms: Option<u64>,
    runs: BTreeMap<PathBuf, OnRun>,
    /// since when each full run's shard waits for a slot to run on, by (its run's log, shard): from
    /// its first ask, through a slot it took and gave up before its lease was granted
    /// ([`Board::give_queued`]), until its holder lets its slot go (its part ran) or its run leaves
    /// the board; on disk with the slots, so a master restart keeps it ([`SLOTS_FILE`])
    waits: BTreeMap<(PathBuf, usize), u64>,
    /// the full runs whose floor rose ([`OnRun::starved`]) as a restarted master read them back,
    /// until each run is on the board again
    kept_starved: BTreeSet<PathBuf>,
    /// what was said of a run's slots, by its log, until its bucket's next run: a slot taken beyond
    /// the pool's count, one freed after its window ([`SLOT_ADOPT`])
    notes: Vec<(PathBuf, String)>,
}

/// The pool's slots ([`BOARD`]) and what each bucket's thread is at.
struct Board {
    slots: Mutex<Slots>,
    freed: std::sync::Condvar,
    /// each bucket's state now (`running`, `queued`, ..), since when, and what it is at
    doing: Mutex<BTreeMap<String, (&'static str, u64, String)>>,
    /// the clock its windows and times read ([`wall_ms`]; a test's own)
    clock: fn() -> u64,
    /// how many shards the pool takes ([`pool_slots`]; a test's own)
    pool: fn() -> Result<usize, String>,
    /// how long a full run's shard waits before it is starved ([`tests_run_ms`]; a test's own)
    starve: fn() -> Option<u64>,
    /// how far a bucket's run is, for a bucket whose log is written as its run ends ([`progress`])
    progress: Mutex<BTreeMap<String, fn() -> Option<String>>>,
}

/// Bucket `name`'s run says how far it is on the board with `of` (None: nothing going): a run that
/// writes its log as it ends otherwise names only that log, not there yet (a daily CI run read as
/// wedged for hours, 11-22).
pub fn progress(name: &str, of: fn() -> Option<String>) {
    BOARD.progress.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), of);
}

/// A shard's pool slot ([`Board::slot`]): free again when it drops, or its run's ([`Slot::ended`]).
struct Slot<'a> {
    board: &'a Board,
    log: PathBuf,
    shard: usize,
    /// its run keeps it as it drops ([`Slot::ended`])
    run_keeps: bool,
}

impl Slot<'_> {
    /// Its shard ended. A full run's last slot so let go, while none of its shards waits for one,
    /// stays the run's ([`RUN_KEPT`]) until its next shard takes it, its run finds nothing to deal
    /// again ([`Board::let_go_kept`]) or leaves the board: its run deals what its shards left
    /// without verdicts to one more shard once they all end, and that shard asked 0.3 s after the
    /// slots freed, as two tests shards that waited took them; it then waited behind four tests
    /// slots while its run's floor was two (14:47, 11-22). (A slot given up to a run that goes
    /// first is dropped, and frees at once: its shard asks again in turn.)
    fn ended(mut self) {
        self.run_keeps = true;
    }
}

/// A run on the board ([`Board::enter`]) until this drops; its slots kept from before a master
/// restart and not taken up go with it.
struct OnBoard<'a> {
    board: &'a Board,
    log: PathBuf,
}

/// How many shards the pool takes now ([`test_boxes`]).
fn pool_slots() -> Result<usize, String> {
    test_boxes().map(|(shards, _)| shards)
}

/// The pool slots shards of runs that go before a full run wait for and none is free for
/// ([`Board::short_before_full`]).
fn short_before_full(s: &Slots) -> usize {
    let Some(n) = s.pool else { return 0 };
    let free = n.saturating_sub(s.held.len() + s.kept.len());
    s.waiting.iter().filter(|w| bucket_of(&w.log) != FULL).count().saturating_sub(free)
}

/// The tests bucket's measured run: the median wall of its last [`TIMES_ROWS`] runs
/// ([`TESTS_WALLS`]). None: none measured. A full run whose shard waited that long behind post-land
/// tests runs keeps half the pool ([`Slots::floor_n`]).
fn tests_run_ms() -> Option<u64> {
    let mut walls: Vec<u64> = TESTS_WALLS.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.iter().copied().collect();
    walls.sort_unstable();
    walls.get(walls.len() / 2).copied()
}

/// A bucket row's run wall in ms.
fn wall_of(row: &Value) -> Option<u64> {
    row.get("ended_ms")?.as_u64()?.checked_sub(row.get("started_ms")?.as_u64()?)
}

/// The bucket a run's log is of (`tests-<ms>.log`: `tests`).
fn bucket_of(log: &Path) -> String {
    log.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit_once('-')).map_or_else(String::new, |(b, _)| b.to_string())
}

/// `HH:MM:SS` on the local clock.
fn clock(ms: u64) -> String {
    makepad_civil_time::local_or_utc((ms / 1000) as i64).hms()
}

/// When `path` was last written, in ms.
fn modified_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64)
}

/// The run of `log` is over: its sidecar retired (no verdict) or its row in its bucket's ledger.
fn run_over(home: &Path, log: &Path) -> bool {
    let path = log.display().to_string();
    !log.with_extension("log.run").exists() || rows_of(home, &bucket_of(log)).iter().any(|r| r.get("log").and_then(Value::as_str) == Some(path.as_str()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl Slots {
    /// Reads [`SLOTS_FILE`] beside `log` once: each slot of a run that is not over is kept for its
    /// shard until [`SLOT_ADOPT`]; one of a run that is over is free, said so.
    fn load(&mut self, home: &Path, log: &Path, now: u64, say: &dyn Fn(String)) {
        if self.file.is_some() { return; }
        let dir = log.parent().unwrap_or(Path::new("."));
        let file = dir.join(SLOTS_FILE);
        let _ = std::fs::remove_file(dir.join(TURN_FILE));
        let mut freed = false;
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for line in text.lines() {
            if let Some(wait) = line.strip_prefix("wait\t") {
                let mut field = wait.split('\t');
                let (Some(log), Some(Ok(shard)), Some(Ok(since))) = (field.next(), field.next().map(str::parse), field.next().map(str::parse)) else { continue };
                if !run_over(home, Path::new(log)) { self.waits.insert((PathBuf::from(log), shard), since); }
                continue;
            }
            if let Some(log) = line.strip_prefix("starved\t") {
                if !run_over(home, Path::new(log)) { self.kept_starved.insert(PathBuf::from(log)); }
                continue;
            }
            // (the count goes until a look reads it again, [`Board::try_take`])
            if let Some(n) = line.strip_prefix("pool\t") {
                if let (None, Ok(n)) = (self.pool, n.trim().parse()) { self.pool = Some(n); }
                continue;
            }
            let Some(c) = Claim::parse(line) else { continue };
            if run_over(home, &c.log) {
                say(format!("bucket {}: the pool slot of shard {} of its run of {} is free: the run is over (log {})", bucket_of(&c.log), c.shard, c.tip, c.log.display()));
                freed = true;
            } else {
                self.kept.push(c);
            }
        }
        self.kept_at = now;
        self.file = Some(file);
        if freed { self.write(say); }
    }

    /// The held and kept slots to [`SLOTS_FILE`], none: no file.
    fn write(&self, say: &dyn Fn(String)) {
        let Some(file) = &self.file else { return };
        let mut text: String = self.held.iter().chain(&self.kept).map(Claim::line).collect();
        for ((log, shard), since) in &self.waits { text.push_str(&format!("wait\t{}\t{shard}\t{since}\n", log.display())); }
        for log in self.runs.iter().filter(|(_, on)| on.starved).map(|(log, _)| log).chain(&self.kept_starved) {
            text.push_str(&format!("starved\t{}\n", log.display()));
        }
        // (with nothing held or waiting, no count is needed)
        if let Some(n) = self.pool.filter(|_| !text.is_empty()) { text.push_str(&format!("pool\t{n}\n")); }
        let done = if text.is_empty() {
            std::fs::remove_file(file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
        } else {
            let temp = file.with_extension("slots.tmp");
            std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, file))
        };
        if let Err(e) = done { say(format!("bucket: {}: {e} (a restarted master gives these slots back to no shard)", file.display())); }
    }

    /// Frees the slots kept past [`SLOT_ADOPT`], a line each, and forgets the waits and raised
    /// floors read back of runs that did not come back on the board within it.
    fn expire(&mut self, now: u64, say: &dyn Fn(String)) {
        if now.saturating_sub(self.kept_at) < SLOT_ADOPT.as_millis() as u64 { return; }
        if !self.kept_starved.is_empty() || self.waits.keys().any(|(log, _)| !self.runs.contains_key(log)) {
            let runs = &self.runs;
            self.waits.retain(|(log, _), _| runs.contains_key(log));
            self.kept_starved.clear();
            if self.kept.is_empty() { self.write(say); }
        }
        if self.kept.is_empty() { return; }
        for c in std::mem::take(&mut self.kept) {
            self.note(&c.log, format!("the pool slot of shard {} of its run of {} held before the master restart is free again: its bucket did not take it up within {} (log {})",
                c.shard, c.tip, took(SLOT_ADOPT.as_secs()), c.log.display()), say);
        }
        self.write(say);
    }

    /// Says `line` of the run of `log`, and keeps it for `devhub bucket`.
    fn note(&mut self, log: &Path, line: String, say: &dyn Fn(String)) {
        say(format!("bucket {}: {line}", bucket_of(log)));
        self.notes.push((log.to_path_buf(), line));
    }

    /// Since when the run of `log` holds pool slots: its first's take.
    fn since(&self, log: &Path) -> Option<u64> {
        let on = self.runs.get(log).and_then(|r| r.since);
        on.into_iter().chain(self.held.iter().chain(&self.kept).filter(|c| c.log == log).map(|c| c.since)).min()
    }

    fn tip(&self, log: &Path) -> String {
        let on = self.runs.get(log).map(|r| r.tip.clone());
        on.or_else(|| self.held.iter().chain(&self.kept).find(|c| c.log == log).map(|c| c.tip.clone())).unwrap_or_default()
    }

    /// Who holds the slots: each run, how many and since when; then the slots kept for the runs a
    /// restarted master adopts.
    fn holding(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (kept, list) in [(false, &self.held), (true, &self.kept)] {
            let mut by: BTreeMap<&Path, (usize, u64, &str)> = BTreeMap::new();
            for c in list.iter() {
                let at = by.entry(c.log.as_path()).or_insert((0, c.since, c.tip.as_str()));
                at.0 += 1;
                at.1 = at.1.min(c.since);
            }
            for (log, (n, since, tip)) in by {
                out.push(match kept {
                    false => format!("the {} run of {tip} holds {n} since {} (log {})", bucket_of(log), clock(since), log.display()),
                    true => format!("the {} run of {tip} held {n} before the master restart, and its bucket takes them up again (log {})", bucket_of(log), log.display()),
                });
            }
        }
        out
    }

    fn pool_count(&self) -> String {
        self.pool.map_or_else(|| "?".to_string(), |n| n.to_string())
    }

    /// The slots the run of `log` holds, kept ones too.
    fn of_run(&self, log: &Path) -> usize {
        self.held.iter().chain(&self.kept).filter(|c| c.log == log).count()
    }

    /// Whether shard `shard` of the run of `log` waits for its run's floor: a full run holds one
    /// slot whatever waits, so its lowest waiting shard, while its run holds none, goes in its
    /// run's ask order and not after every other run's ([`turn`]); its run's last slot is never
    /// given up ([`Board::give_queued`], [`Dealer::gives`]). With every other run first and its last
    /// shard giving too, the full run asked at 17:00 took nine units of 693 in 80 min: the tests
    /// runs, asked back to back from 11:58 on, held all four slots but for a moment between two
    /// (11-19). One slot of four leaves a tests run three, well within its hour.
    /// Its floor is [`Slots::floor_n`] slots: its lowest waiting shards up to that many, less the
    /// slots it holds, go in its ask's order.
    fn floor(&self, log: &Path, shard: usize) -> bool {
        let floor = self.floor_n(log);
        bucket_of(log) == FULL && self.of_run(log) < floor && self.waiting.iter().filter(|w| w.log == log && w.shard < shard).count() < floor - self.of_run(log)
    }

    /// The slots a full run keeps and takes in its ask's order ([`Slots::floor`]): one, and half the
    /// pool once one of its shards waited the tests bucket's measured run behind post-land tests
    /// runs ([`Slots::starve_ms`]). With only one, a full run asked at 11:57 ran on one slot of four
    /// for an hour behind tests runs asked back to back, each of which took the other three
    /// within seconds of its ask, and full runs took 2.5-6.7 h (11-22). With half, a full run runs
    /// on about half the pool beside them, and a tests run waits for a slot about one tests run at
    /// most.
    fn floor_n(&self, log: &Path) -> usize {
        match self.pool {
            Some(n) if bucket_of(log) == FULL && self.runs.get(log).is_some_and(|on| on.starved) => (n / 2).max(1),
            _ => 1,
        }
    }

    /// Whether waiting full shard `w` waited the tests bucket's measured run ([`Slots::starve_ms`]).
    fn starved(&self, w: &Waiter, now: u64) -> bool {
        bucket_of(&w.log) == FULL && self.starve_ms.is_some_and(|b| now.saturating_sub(w.since) >= b)
    }
}

/// A waiting shard's place in the pool's queue: a full run's after every other run's but its floor
/// ([`Slots::floor`]), then by its run's ask ([`Board::try_take`]).
fn turn(log: &Path, asked: u64, shard: usize, floor: bool) -> (bool, u64, &Path, usize) {
    (bucket_of(log) == FULL && !floor, asked, log, shard)
}

impl Board {
    /// The pool slots shards of runs that go before a full run ([`turn`]) wait for and none is
    /// free for: how many of a full run's shards give theirs up as their units end
    /// ([`Dealer::line`]). A slot let go counts as free at once, before its waiter takes it.
    fn short_before_full(&self) -> usize {
        short_before_full(&self.slots())
    }

    /// The slots the full run of `log` keeps ([`Slots::floor_n`]): its dealer gives none below them.
    fn floor_n(&self, log: &Path) -> usize {
        self.slots().floor_n(log)
    }

    /// Shard `shard` of the full run of `log`, its lease not granted yet, gives its slot up at once
    /// to a run that goes first and waits for one ([`lease_run`]'s `withdraw`): true, its claim
    /// gone, when one waits and its run holds another (its floor, [`Slots::floor`]); one slot per
    /// waiter, under the board's lock. Held from a free moment
    /// between two tests runs, a full shard's slot went to a lease that queued for a box, set up on
    /// it and gave the slot straight back at its first unit, the next tests run's shard waiting
    /// all the while (L1385862, 11-18).
    fn give_queued(&self, log: &Path, shard: usize, say: &dyn Fn(String)) -> bool {
        let mut s = self.slots();
        if bucket_of(log) != FULL || short_before_full(&s) == 0 || !s.held.iter().any(|c| c.of(log, shard)) || s.of_run(log) <= s.floor_n(log) { return false; }
        // (its wait goes on: [`Slots::waits`] keeps it)
        s.held.retain(|c| !c.of(log, shard));
        s.write(say);
        drop(s);
        self.freed.notify_all();
        true
    }

    const fn new(clock: fn() -> u64, pool: fn() -> Result<usize, String>) -> Board {
        let slots = Slots {
            file: None, held: Vec::new(), kept: Vec::new(), kept_at: 0, waiting: Vec::new(),
            pool: None, pool_read: None, pool_err: String::new(), starve_ms: None, runs: BTreeMap::new(), waits: BTreeMap::new(), kept_starved: BTreeSet::new(), notes: Vec::new(),
        };
        Board { slots: Mutex::new(slots), freed: std::sync::Condvar::new(), doing: Mutex::new(BTreeMap::new()), clock, pool, starve: tests_run_ms, progress: Mutex::new(BTreeMap::new()) }
    }

    /// The board with its starve bound read by `starve` ([`tests_run_ms`] by default).
    #[cfg(test)]
    const fn starving_after(mut self, starve: fn() -> Option<u64>) -> Board {
        self.starve = starve;
        self
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bucket `name` is at `what` since `since`; None: at nothing.
    fn doing(&self, name: &str, now: Option<(&'static str, u64, String)>) {
        let mut doing = self.doing.lock().unwrap_or_else(|e| e.into_inner());
        match now {
            Some(now) => { doing.insert(name.to_string(), now); }
            None => { doing.remove(name); }
        }
    }

    /// The run of `log` (its tip `tip`) is on the board until the guard drops. What was said of
    /// its bucket's runs before goes.
    fn enter(&self, home: &Path, log: &Path, tip: &str, say: &dyn Fn(String)) -> OnBoard<'_> {
        let mut s = self.slots();
        s.load(home, log, (self.clock)(), say);
        let bucket = bucket_of(log);
        s.notes.retain(|(l, _)| l == log || bucket_of(l) != bucket);
        // (a floor raised before a master restart comes back with the run, [`SLOTS_FILE`])
        let starved = s.kept_starved.remove(log);
        s.runs.insert(log.to_path_buf(), OnRun { tip: tip.to_string(), since: None, why: None, starved });
        OnBoard { board: self, log: log.to_path_buf() }
    }

    /// One look: shard `shard` of the run of `log` takes a pool slot as `need` lets it (None: it
    /// needs none), or why it waits. A slot kept from before a master restart goes back to its
    /// shard, a shard whose lease runs on never waits, and the rest go in the order their runs were
    /// asked as slots free.
    fn try_take(&self, home: &Path, log: &Path, shard: usize, need: Need, say: &dyn Fn(String)) -> Result<Option<Slot<'_>>, String> {
        let now = (self.clock)();
        let stale = !matches!(need, Need::Ended | Need::Gave) && self.slots().pool_read.is_none_or(|at| now.saturating_sub(at) >= POOL_LOOK.as_millis() as u64);
        let read = if stale { Some(((self.pool)(), (self.starve)())) } else { None };
        let mut s = self.slots();
        if let Some((_, starve)) = &read { s.starve_ms = *starve; }
        match read.map(|(pool, _)| pool) {
            Some(Ok(n)) => (s.pool, s.pool_read) = (Some(n), Some(now)),
            // read again at the next look until it is known, then a minute on
            Some(Err(e)) => (s.pool_err, s.pool_read) = (e, s.pool.map(|_| now)),
            None => {}
        }
        s.load(home, log, now, say);
        let mine = |c: &Claim| c.of(log, shard);
        let running = match need {
            Need::Ended | Need::Gave => {
                if let Some(i) = s.kept.iter().position(mine) {
                    s.kept.remove(i);
                    s.waits.remove(&(log.to_path_buf(), shard));
                    s.write(say);
                    drop(s);
                    self.freed.notify_all();
                }
                return Ok(None);
            }
            Need::Running(started) => Some(started),
            Need::Lease => None,
        };
        s.expire(now, say);
        if let Some(i) = s.kept.iter().position(|c| mine(c) || c.of(log, RUN_KEPT)) {
            let mut back = s.kept.remove(i);
            back.shard = shard;
            return Ok(Some(self.hold(s, back, say)));
        }
        // the slot its run's last shard to end left it ([`Slot`]'s drop)
        if let Some(i) = s.held.iter().position(|c| c.of(log, RUN_KEPT)) {
            let mut back = s.held.remove(i);
            (back.shard, back.taken) = (shard, now);
            return Ok(Some(self.hold(s, back, say)));
        }
        let (tip, used) = (s.tip(log), s.held.len() + s.kept.len());
        if let Some(started) = running {
            let claim = Claim { log: log.to_path_buf(), shard, tip: tip.clone(), since: s.since(log).or(started).unwrap_or(now), taken: now };
            if s.pool.is_some_and(|n| used >= n) {
                let line = format!("shard {shard} of its run of {tip} takes a pool slot beyond the {} the pool takes: its lease ran on through a master restart with no slot on file (log {})", s.pool_count(), log.display());
                s.note(log, line, say);
            }
            return Ok(Some(self.hold(s, claim, say)));
        }
        let asked = log_started(log).unwrap_or(0);
        // in the order their runs were asked, a post-land tests run's shards before a full run's
        // ([`turn`]): a scheduled full run of an older tip held all three slots 40 min while the
        // tests run of the landings since waited, and those went unchecked (11-13). The full run
        // yields at its next shard's end; no shard is cut short.
        // A full run's floor goes in its ask's order ([`Slots::floor`]).
        // A full shard that waited the tests bucket's measured run raises its run's floor to half
        // the pool ([`Slots::floor_n`]).
        // A full shard's wait goes on through a slot it took and gave up before its lease was
        // granted ([`Board::give_queued`]): taken the moment one freed between two tests runs and
        // given to the next one's shard, each piece of its wait began again, none reached the
        // measured run, and a full run asked at 11:57 was still on one slot of four at 13:58
        // (11-22). The board keeps it ([`Slots::waits`]), on disk through a master restart; one it
        // has no record of (a slots file from a master before them) whose log names no grant has
        // waited since its run was asked.
        if !s.waiting.iter().any(|w| w.log == log && w.shard == shard) {
            let key = (log.to_path_buf(), shard);
            let known = s.waits.get(&key).copied();
            let since = match known {
                Some(since) => since,
                None if bucket_of(log) == FULL => {
                    let since = if asked > 0 && !ever_granted(&shard_log(log, shard)) { asked.min(now) } else { now };
                    s.waits.insert(key, since);
                    s.write(say);
                    since
                }
                None => now,
            };
            s.waiting.push(Waiter { log: log.to_path_buf(), asked, shard, since });
        }
        if s.waiting.iter().any(|w| w.log == log && s.starved(w, now)) && s.runs.get(log).is_some_and(|on| !on.starved) {
            if let Some(on) = s.runs.get_mut(log) { on.starved = true; }
            s.write(say);
            let line = format!("bucket full: a shard of its run waited the tests bucket's measured run ({}) behind post-land tests runs: the run keeps {} of the pool's {} slots from now on (log {})",
                crate::lease::sched::dur(s.starve_ms.unwrap_or(0)), s.floor_n(log), s.pool_count(), log.display());
            say(line);
        }
        let me = turn(log, asked, shard, s.floor(log, shard));
        let ahead = s.waiting.iter().filter(|w| turn(&w.log, w.asked, w.shard, s.floor(&w.log, w.shard)) < me).count();
        let jumped = s.waiting.iter().filter(|w| turn(&w.log, w.asked, w.shard, s.floor(&w.log, w.shard)) < me && (w.asked, w.log.as_path(), w.shard) > (asked, log, shard)).count();
        if s.pool.is_some_and(|n| n.saturating_sub(used) > ahead) {
            s.waiting.retain(|w| !(w.log == log && w.shard == shard));
            let claim = Claim { log: log.to_path_buf(), shard, tip, since: s.since(log).unwrap_or(now), taken: now };
            return Ok(Some(self.hold(s, claim, say)));
        }
        let mut whys = s.holding();
        if s.pool.is_none() {
            whys.push(format!("how many shards the pool takes is not known: {}", s.pool_err));
        } else if jumped > 0 {
            whys.push(format!("{ahead} shard{} go{} first ({jumped} of a post-land tests run, which goes before a full run)", plural(ahead), if ahead == 1 { "es" } else { "" }));
        } else if ahead > 0 {
            whys.push(format!("{ahead} shard{} asked before it go{} first", plural(ahead), if ahead == 1 { "es" } else { "" }));
        }
        if let Some(bound) = s.starve_ms.filter(|_| bucket_of(log) == FULL && !s.floor(log, shard)) {
            if s.runs.get(log).is_some_and(|on| on.starved) {
                whys.push(format!("past its run's floor of {} slots (a shard of its run waited the tests bucket's measured run, {})", s.floor_n(log), crate::lease::sched::dur(bound)));
            } else {
                whys.push(format!("its run keeps half the pool once one of its shards has waited the tests bucket's measured run ({})", crate::lease::sched::dur(bound)));
            }
        }
        if bucket_of(log) != FULL {
            let full: BTreeSet<&Path> = s.held.iter().map(|c| c.log.as_path()).filter(|l| bucket_of(l) == FULL).collect();
            if full.iter().any(|l| s.of_run(l) > s.floor_n(l)) {
                whys.push("a full run's shard gives its slot up as its next unit ends".to_string());
            } else if !full.is_empty() {
                whys.push(if full.iter().any(|l| s.floor_n(l) > 1) { "a full run keeps half the pool, its floor, until its run ends".to_string() } else { "a full run keeps one slot, its floor, until its run ends".to_string() });
            }
        }
        let why = format!("{used} of {} pool slots held: {}", s.pool_count(), whys.join("; "));
        if let Some(on) = s.runs.get_mut(log) { on.why = Some(why.clone()); }
        Err(why)
    }

    /// The slot the run of `log` keeps for its next shard ([`RUN_KEPT`]) frees: it deals no more
    /// (one a restarted master read back too).
    fn let_go_kept(&self, log: &Path) {
        let mut s = self.slots();
        let had = s.held.len() + s.kept.len();
        s.held.retain(|c| !c.of(log, RUN_KEPT));
        s.kept.retain(|c| !c.of(log, RUN_KEPT));
        if s.held.len() + s.kept.len() == had { return; }
        s.write(&|l| eprintln!("{l}"));
        drop(s);
        self.freed.notify_all();
    }

    /// Holds `claim` for its shard, on disk before its lease is asked.
    fn hold<'a>(&'a self, mut s: std::sync::MutexGuard<'_, Slots>, claim: Claim, say: &dyn Fn(String)) -> Slot<'a> {
        let (log, shard) = (claim.log.clone(), claim.shard);
        if let Some(on) = s.runs.get_mut(&log) {
            on.since.get_or_insert(claim.since);
            on.why = None;
        }
        s.held.push(claim);
        s.write(say);
        Slot { board: self, log, shard, run_keeps: false }
    }

    /// Shard `shard` of the run of `log` takes a pool slot as `need` lets it ([`Board::try_take`]),
    /// waiting its turn for a lease: None when it needs none, or when its run's queue was dealt out
    /// while it waited (`needed`). It says why it waits as it starts to, and every [`SLOT_SAID`].
    fn slot(&self, home: &Path, log: &Path, shard: usize, need: Need, needed: &dyn Fn() -> bool, say: &dyn Fn(String)) -> Option<Slot<'_>> {
        let (name, start) = (bucket_of(log), (self.clock)());
        let mut said = None::<u64>;
        loop {
            let why = match self.try_take(home, log, shard, need, say) {
                Ok(slot) => {
                    if said.is_some() {
                        say(format!("bucket {name}: shard {shard} of its run takes a pool slot after {} of waiting (log {})", took((self.clock)().saturating_sub(start) / 1000), log.display()));
                    }
                    return slot;
                }
                Err(why) => why,
            };
            if !needed() {
                self.slots().waiting.retain(|w| !(w.log == log && w.shard == shard));
                self.freed.notify_all();
                return None;
            }
            let now = (self.clock)();
            if said.is_none_or(|at| now.saturating_sub(at) >= SLOT_SAID.as_millis() as u64) {
                say(match said {
                    None => format!("bucket {name}: shard {shard} of its run (asked {}, log {}) waits for a pool slot: {why}", clock(log_started(log).unwrap_or(start)), log.display()),
                    Some(_) => format!("bucket {name}: shard {shard} of its run still waits for a pool slot after {}: {why}", took(now.saturating_sub(start) / 1000)),
                });
                said = Some(now);
            }
            drop(self.freed.wait_timeout(self.slots(), SLOT_LOOK).unwrap_or_else(|e| e.into_inner()).0);
        }
    }

    /// `<bucket>: <state> since <HH:MM:SS> (<for>); <what>` of each bucket at something: a run on
    /// the pool's slots on how many, since its first; one whose shards wait, why; what was said of
    /// its slots; and of each asked to run (its kick file) whose run has not started.
    fn now_lines(&self, home: &Path, now: u64) -> Vec<String> {
        let doing = self.doing.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let s = self.slots();
        let mut out = Vec::new();
        for name in KICKED {
            let on = s.runs.iter().filter(|(log, _)| bucket_of(log) == name).find_map(|(log, on)| {
                let (held, waits, asked) = (s.held.iter().filter(|c| &c.log == log).count(), s.waiting.iter().filter(|w| &w.log == log).count(), log_started(log).unwrap_or(now));
                if held > 0 {
                    let since = s.since(log).unwrap_or(now);
                    let waits = if waits > 0 { format!(", {waits} shard{} wait{} for one", plural(waits), if waits == 1 { "s" } else { "" }) } else { String::new() };
                    // the run's ask, beside its start: after the waiting shard it read as that shard's (11-12)
                    let asked = if since > asked + 60_000 { format!(", asked {}", clock(asked)) } else { String::new() };
                    return Some(format!("{name}: running since {} ({}{asked}); its run of {} on {held} of {} pool slots{waits}; {}",
                        clock(since), took(now.saturating_sub(since) / 1000), on.tip, s.pool_count(), run_logs(log)));
                }
                let why = on.why.as_ref().filter(|_| waits > 0)?;
                Some(format!("{name}: queued since {} ({}); its run of {} waits for the pool slots: {why}; log {}", clock(asked), took(now.saturating_sub(asked) / 1000), on.tip, log.display()))
            });
            let at = doing.get(name);
            match (&on, at) {
                (Some(line), _) => out.push(line.clone()),
                (None, Some((state, since, what))) => {
                    let far = self.progress.lock().unwrap_or_else(|e| e.into_inner()).get(name).and_then(|of| of()).map_or(String::new(), |p| format!("; {p}"));
                    out.push(format!("{name}: {state} since {} ({}); {what}{far}", clock(*since), took(now.saturating_sub(*since) / 1000)))
                }
                (None, None) => {}
            }
            out.extend(s.notes.iter().filter(|(log, _)| bucket_of(log) == name).map(|(_, note)| format!("{name}: {note}")));
            if let Some(asked) = modified_ms(&kick_file(home, name)) {
                out.push(format!("{name}: asked since {} ({}); `devhub bucket run`: its run starts {}", clock(asked), took(now.saturating_sub(asked) / 1000),
                    if at.is_some() || on.is_some() { "as the one above ends" } else { "at its next look" }));
            }
        }
        out
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut s = self.board.slots();
        let ours = s.held.iter().find(|c| c.of(&self.log, self.shard)).cloned();
        s.held.retain(|c| !c.of(&self.log, self.shard));
        if let Some(mut c) = ours.clone().filter(|_| self.run_keeps && bucket_of(&self.log) == FULL && s.runs.contains_key(&self.log) && s.of_run(&self.log) == 0 && !s.waiting.iter().any(|w| w.log == self.log)) {
            c.shard = RUN_KEPT;
            s.held.push(c);
        }
        // its holder let it go (its part ran, or its lease ended): its wait is over; one it gave up
        // before its lease's grant was gone already, and its wait goes on
        if ours.is_some() { s.waits.remove(&(self.log.clone(), self.shard)); }
        s.write(&|l| eprintln!("{l}"));
        drop(s);
        self.board.freed.notify_all();
    }
}

impl Drop for OnBoard<'_> {
    fn drop(&mut self) {
        let mut s = self.board.slots();
        let starved = s.runs.remove(&self.log).is_some_and(|on| on.starved);
        s.waiting.retain(|w| w.log != self.log);
        let (kept, waits, held) = (s.kept.len(), s.waits.len(), s.held.len());
        s.held.retain(|c| !c.of(&self.log, RUN_KEPT));
        s.kept.retain(|c| c.log != self.log);
        s.waits.retain(|(log, _), _| *log != self.log);
        if s.kept.len() != kept || s.waits.len() != waits || s.held.len() != held || starved { s.write(&|l| eprintln!("{l}")); }
        drop(s);
        self.board.freed.notify_all();
    }
}

/// A bucket row's tip: work, main, and each other nested repository's commit (`nested`, by name).
fn tip_value(tip: &Pin) -> Value {
    let pair = tip.pair();
    let others: Vec<(String, Value)> = tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED).map(|(n, sha)| (n.clone(), s(sha))).collect();
    let mut fields = vec![("work", s(&pair.work)), ("main", s(&pair.main))];
    if !others.is_empty() { fields.push(("nested", Value::Obj(others.into_iter().collect()))); }
    obj(fields)
}

/// [`tip_value`] read back; a row with no `nested` looked at no other nested repository.
fn read_tip(v: &Value) -> Option<Pin> {
    let pair = Pair::new(v.get("work")?.as_str()?, v.get("main")?.as_str()?);
    if !pair.valid() { return None; }
    let mut pin = pin_of(&pair);
    if let Some(Value::Obj(nested)) = v.get("nested") {
        for (name, sha) in nested.iter() {
            let sha = sha.as_str().filter(|s| crate::state::hex40(s))?;
            if !crate::nested::valid(name) { return None; }
            pin.nested.insert(name.clone(), sha.to_string());
        }
    }
    Some(pin)
}

/// The pin of a pair: makepad and the paired repository.
pub(crate) fn pin_of(pair: &Pair) -> Pin {
    Pin::at(&pair.work).with(crate::state::PAIRED, &pair.main)
}

/// The lease `--rev` of a bucket's tip: work, commercial main, and each other nested repository
/// at the branch its checkout in `repo` is on ([`crate::nested::branches`]).
fn tip_rev(repo: &Path, tip: &Pin) -> Result<String, String> {
    let branches = crate::nested::branches(repo);
    let mut rev = tip.pair().rev();
    for (name, sha) in tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED) {
        let branch = branches.iter().find(|(n, _)| n == name).map(|(_, b)| b).ok_or_else(|| format!("{name} is on no branch in {}: the run cannot pin it", repo.display()))?;
        rev.push_str(&format!(",{name}={branch}@{sha}"));
    }
    Ok(rev)
}

/// The gate's bases of the other nested repositories (`GATE_NESTED_BASES=<name>=<sha|->,..`):
/// each one `tip` names at its `base` commit, `-` where the base had none of it (every file of it
/// is new to the bucket).
fn nested_bases(base: &Pin, tip: &Pin) -> String {
    let sides: Vec<String> = tip.nested.keys().filter(|n| *n != crate::state::PAIRED).map(|n| format!("{n}={}", base.nested.get(n).map_or("-", String::as_str))).collect();
    format!("GATE_NESTED_BASES={}", sides.join(","))
}

/// `<work 10>,<main 10>[,<name>=<sha 10>]..`: a bucket tip as its lines say it.
fn tip_short(tip: &Pin) -> String {
    tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED).fold(tip.pair().short(), |k, (n, sha)| format!("{k},{n}={}", crate::state::short(sha)))
}

/// An outcome that says nothing of the tip.
fn no_verdict(why: String) -> Outcome {
    Outcome { ok: false, failing: Vec::new(), log: String::new(), summary: String::new(), no_verdict: Some(why) }
}

fn read_run(path: &Path) -> Option<(u32, u64, Pin, Pin)> {
    let v = makepad_strict_json::parse(&std::fs::read(path).ok()?).ok()?;
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    if pid == 0 { return None; }
    Some((pid, v.get("started_ms")?.as_u64()?, read_tip(v.get("base")?)?, read_tip(v.get("tip")?)?))
}

pub(crate) fn entry_pair(text: &str) -> Option<Pair> {
    let words: Vec<_> = text.split_whitespace().collect();
    let ["work", work, "main", main] = words.as_slice() else { return None };
    let pair = Pair::new(work, main);
    pair.valid().then_some(pair)
}

/// Newest first, across all buckets.
pub fn ledger(home: &Path, n: usize) -> Vec<String> {
    let _lock = LEDGER.lock().unwrap();
    let file = match std::fs::File::open(home.join("buckets.jsonl")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => { eprintln!("buckets ledger: {e}"); return Vec::new(); }
    };
    let mut lines = VecDeque::new();
    for line in BufReader::new(file).lines() {
        match line {
            Ok(line) if !line.is_empty() => {
                lines.push_back(line);
                if lines.len() > n { lines.pop_front(); }
            }
            Ok(_) => {}
            Err(e) => { eprintln!("buckets ledger: {e}"); break; }
        }
    }
    lines.into_iter().rev().collect()
}

fn append(home: &Path, value: Value) -> std::io::Result<()> {
    let _lock = LEDGER.lock().unwrap();
    std::fs::create_dir_all(home)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(home.join("buckets.jsonl"))?;
    let mut line = value.to_json();
    line.push('\n');
    file.write_all(line.as_bytes())?;
    if value.get("bucket").and_then(Value::as_str) == Some("tests") {
        if let (Some(wall), Some(walls)) = (wall_of(&value), TESTS_WALLS.lock().unwrap_or_else(|e| e.into_inner()).as_mut()) {
            walls.push_front(wall);
            walls.truncate(TIMES_ROWS);
        }
    }
    Ok(())
}

/// A CI leg (`smoke:linux:apps/pdf`, `live:kinds`): `<kind>:<rest>`, one word, its own key. Its
/// row says it passed or failed (orch/ci.rs writes its GATEUNIT lines), so its red stays open until
/// a run passes it and `devhub bucket` says it STILL FAILING; a landing hears of it only by the
/// files of its errors ([`file_blame`]), never as one of every landing of the night's range (each
/// CI red went to nobody: none of its legs was a unit, 11-17).
fn is_leg(unit: &str) -> bool {
    !unit.contains(char::is_whitespace) && unit.split_once(':').is_some_and(|(kind, rest)| !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase()) && !rest.is_empty())
}

fn is_unit(unit: &str) -> bool {
    let mut words = unit.split_whitespace();
    match (words.next(), words.next()) {
        // `tests <pkg>`: its test targets' one cargo check (land_gate.sh step_testcheck); `test
        // <pkg>`: its tests run under the JIT (the tests bucket, land_gate.sh testrun); `guard
        // <pkg>`: its GPU guard of a user-visible effect (the hot bucket, step_guards); `wasm <pkg>`:
        // its wasm32 check (the fast bucket, step_wasmcheck); `windows <pkg>`: its cargo check on a
        // Windows box (the windows bucket); `wintest <pkg>/<target>`: its tests on a Windows box
        // (the windows-tests bucket); `boundary <app>`: its boundary audit (the fast bucket,
        // qboundary_one)
        (Some("check" | "present" | "cargo" | "tests" | "test" | "guard" | "wasm" | "windows" | "wintest" | "boundary"), Some(_)) => true,
        (Some("suite"), Some(_)) => words.next().is_none(),
        (Some("devhub"), Some("makepad-devhub")) => words.next().is_none(),
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn range(repo: &Path, base: &str, tip: &str) -> Result<std::sync::Arc<std::collections::HashSet<String>>, String> {
    crate::gitread::Pass::default().range(repo, base, tip)
}

/// The landings between two tips: a commercial-only landing moves main, not work. One that
/// moved nothing (its branch was in work already: just before is what it landed) is none. A side
/// a landing did not move is no sign of it: the commercial-only landings after a work landing
/// left work at its commit, and the bucket that ran at that work landing named them too (#2543
/// and #2544 in the range of #2542, 11-07).
fn landings(repo: &Path, base: &Pair, tip: &Pair, entries: &[Entry]) -> Result<Vec<Entry>, String> {
    let mut pass = crate::gitread::Pass::default();
    let work = pass.range(repo, &base.work, &tip.work)?;
    let main = pass.range(&repo.join("apps/commercial"), &base.main, &tip.main)?;
    Ok(entries.iter().filter(|e| e.pre != e.landed).filter(|e| {
        let Some(landed) = entry_pair(&e.landed) else { return false };
        let pre = entry_pair(&e.pre);
        let moved = |side: fn(&Pair) -> &String| pre.as_ref().is_none_or(|p| side(p) != side(&landed));
        (moved(|p| &p.work) && work.contains(&landed.work)) || (moved(|p| &p.main) && main.contains(&landed.main))
    }).cloned().collect())
}

/// The landings a search from `from` to `tip` walks ([`run_bisect`]), in their chains' order
/// ([`order_landings`]), with what it left out and why, and the chains of the other nested
/// repositories `tip` pins, which it walked too.
fn chain_landings(repo: &Path, history: &[Entry], from: &Pin, tip: &Pin) -> Result<(Vec<Entry>, Vec<String>, Vec<(String, Vec<String>)>), String> {
    let (a, b) = (from.pair(), tip.pair());
    let work = bisect_chain(repo, &a.work, &b.work)?;
    let main = bisect_chain(&repo.join("apps/commercial"), &a.main, &b.main)?;
    // every other nested repository the red tip pinned is walked too, as the bucket names its tip
    // (`apps=..`): a landing of its halves alone is a candidate, and each probe pins it where that
    // landing left it, where every probe took the red tip's (11-23)
    let (nested, unread) = nested_chains(repo, from, tip);
    let mut range = landings(repo, &a, &b, history)?;
    let alone: Vec<Entry> = history.iter().filter(|e| !nested_moves(e, &nested).is_empty() && !range.iter().any(|r| r.id == e.id)).cloned().collect();
    range.extend(alone);
    let (ordered, mut left_out) = order_landings(&work, &main, &nested, &range);
    left_out.extend(unread);
    Ok((ordered, left_out, nested))
}

/// A tip's first-parent chain, oldest first, including `from` at position zero (11-15).
fn bisect_chain(dir: &Path, from: &str, tip: &str) -> Result<Vec<String>, String> {
    if from == tip { return Ok(vec![from.to_string()]); }
    crate::gitread::with(dir, |p, repo| {
        let base = crate::gitread::resolve(p, repo, from).ok_or_else(|| format!("bisect: cannot read {from}"))?;
        let tip = crate::gitread::resolve(p, repo, tip).ok_or_else(|| format!("bisect: cannot read {tip}"))?;
        let mut chain = Vec::new();
        for c in repo.first_parents(tip) {
            let (oid, _) = c.map_err(|e| format!("bisect first-parent chain: {e}"))?;
            chain.push(oid.to_hex());
            if oid == base { chain.reverse(); return Ok(chain); }
        }
        Err(format!("bisect: {from} is not on {tip}'s first-parent chain"))
    }).ok_or_else(|| format!("bisect: cannot read repository {}", dir.display()))?
}

/// Serial landings grow along both chains; an unchanged side stays at its pre's position.
/// Record times and hand-in numbers do not order the commits they landed (11-15). A landing of
/// other nested repositories' halves alone ([`nested_moves`]: its pair the one before it left)
/// comes after the landing that made its pair, and such landings by their own chains' positions.
/// They were none: their pair did not move, and a red an apps landing caused was laid to a
/// makepad landing or to nobody (11-23).
fn order_landings(work: &[String], main: &[String], nested: &[(String, Vec<String>)], entries: &[Entry]) -> (Vec<Entry>, Vec<String>) {
    let positions = |chain: &[String]| chain.iter().enumerate().map(|(i, sha)| (sha.clone(), i)).collect::<HashMap<_, _>>();
    let (work, main) = (positions(work), positions(main));
    let (mut ordered, mut left_out) = (Vec::new(), Vec::new());
    for e in entries {
        let (Some(pre), Some(landed)) = (entry_pair(&e.pre), entry_pair(&e.landed)) else {
            left_out.push(format!("#{} has no commit pair: left out", e.id));
            continue;
        };
        let moves = nested_moves(e, nested);
        if pre == landed && moves.is_empty() { continue; }
        let w = work.get(if pre.work == landed.work { &pre.work } else { &landed.work });
        let m = main.get(if pre.main == landed.main { &pre.main } else { &landed.main });
        if w.is_none() { left_out.push(format!("#{} is not on work's first-parent chain: left out", e.id)); }
        if m.is_none() { left_out.push(format!("#{} is not on main's first-parent chain: left out", e.id)); }
        let alone = usize::from(pre == landed);
        if let (Some(w), Some(m)) = (w, m) { ordered.push(((*w, *m, alone, moves, e.changed_ms), e.clone())); }
    }
    ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
    (ordered.into_iter().map(|(_, e)| e).collect(), left_out)
}

/// The positions a landing moved the other nested repositories to along their chains (`nested`:
/// each walked one's, [`nested_chains`]), by chain: none at a chain's base (its last pass's
/// commit) or off it.
fn nested_moves(e: &Entry, nested: &[(String, Vec<String>)]) -> Vec<(usize, usize)> {
    let mut moves: Vec<(usize, usize)> = e.nested.iter().filter_map(|side| {
        let (k, (_, chain)) = nested.iter().enumerate().find(|(_, (name, _))| *name == side.name)?;
        chain.iter().position(|c| *c == side.landed).filter(|at| *at > 0).map(|at| (k, at))
    }).collect();
    moves.sort();
    moves
}

/// The first-parent chains of the other nested repositories a bisect's red tip pinned (11-23),
/// each from its last pass's commit (its red tip's alone when that named none: nothing of it to
/// walk). One that does not read stays at its red tip's commit, said.
fn nested_chains(repo: &Path, from: &Pin, tip: &Pin) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let (mut chains, mut unread) = (Vec::new(), Vec::new());
    for (name, at) in tip.nested.iter().filter(|(name, _)| *name != crate::state::PAIRED) {
        let base = from.nested.get(name).unwrap_or(at);
        match bisect_chain(&repo.join(crate::nested::dir(name)), base, at) {
            Ok(chain) => chains.push((name.clone(), chain)),
            Err(e) => unread.push(format!("{name} not walked, at its red tip's commit: {e}")),
        }
    }
    (chains, unread)
}

/// Each candidate's pin: its landed pair, each walked nested repository at the last candidate up
/// to it that moved it (its last pass's commit before one did), and never behind where its chain
/// stood when the candidate's pair was made (`times`: each chain's commit times; `when`: each
/// candidate's pair's, [`pair_time`]), any other at the red tip's. A probe pins commits that
/// existed together, whether or not a landing moved them (main, 11-24).
fn candidate_pins(candidates: &[Entry], tip: &Pin, nested: &[(String, Vec<String>)], times: &[Vec<i64>], when: &[Option<i64>]) -> Vec<Pin> {
    let mut at = vec![0usize; nested.len()];
    candidates.iter().enumerate().map(|(c, e)| {
        for (k, i) in nested_moves(e, nested) { at[k] = at[k].max(i); }
        if let Some(t) = when.get(c).copied().flatten() {
            for (k, ts) in times.iter().enumerate().take(nested.len()) {
                at[k] = at[k].max(ts.iter().position(|x| *x > t).map_or(ts.len().saturating_sub(1), |p| p.saturating_sub(1)));
            }
        }
        let mut pin = bisect_pin(pin_of(&entry_pair(&e.landed).expect("landed pair")), tip);
        for (k, (name, chain)) in nested.iter().enumerate() { pin.nested.insert(name.clone(), chain[at[k]].clone()); }
        pin
    }).collect()
}

/// The last pass's pin of each other nested repository the red tip pins and the pass did not (one
/// the bucket followed only after it): that repository's first-parent commit at the pass's time,
/// the newest no later than its pair's commits ([`pair_time`]), never the red tip's. A CI red's
/// bisect pinned apps at its red tip (Fab moved in) beside makepad and commercial from before Fab
/// moved out of commercial, and every probe, its last pass too, failed on two `fab-ai` packages,
/// a pin set that never existed (main, 11-24). One with no commit by then stays at the red tip's,
/// said.
fn place_from(repo: &Path, from: &Pin, tip: &Pin) -> (Pin, Vec<String>) {
    let (mut from, mut said) = (from.clone(), Vec::new());
    let at = pair_time(repo, &from.pair());
    for (name, sha) in tip.nested.iter().filter(|(n, _)| *n != crate::state::PAIRED) {
        if from.nested.contains_key(name) { continue; }
        let then = at.and_then(|t| chain_at(&repo.join(crate::nested::dir(name)), sha, t));
        if then.is_none() { said.push(format!("{name} had no commit at its last pass's time to read: at its red tip's")); }
        from.nested.insert(name.clone(), then.unwrap_or_else(|| sha.clone()));
    }
    (from, said)
}

/// When a pair was made: its newer commit's committer time (s), each side read in its repository.
fn pair_time(repo: &Path, pair: &Pair) -> Option<i64> {
    [commit_time(repo, &pair.work), commit_time(&repo.join("apps/commercial"), &pair.main)].into_iter().flatten().max()
}

fn commit_time(dir: &Path, sha: &str) -> Option<i64> {
    if sha.is_empty() { return None; }
    crate::gitread::with(dir, |p, repo| {
        let oid = crate::gitread::resolve(p, repo, sha)?;
        repo.read_commit(&oid).ok().map(|c| c.committer.timestamp)
    }).flatten()
}

/// Each commit's committer time along a chain (i64::MAX where one does not read: never chosen by
/// its time).
fn chain_times(dir: &Path, chain: &[String]) -> Vec<i64> {
    chain.iter().map(|sha| commit_time(dir, sha).unwrap_or(i64::MAX)).collect()
}

/// The newest commit of `tip`'s first-parent chain in `dir` made no later than `t` (s).
fn chain_at(dir: &Path, tip: &str, t: i64) -> Option<String> {
    crate::gitread::with(dir, |p, repo| {
        let tip = crate::gitread::resolve(p, repo, tip)?;
        repo.first_parents(tip).map_while(Result::ok).find(|(_, c)| c.committer.timestamp <= t).map(|(oid, _)| oid.to_hex())
    }).flatten()
}

/// The lane a landing was: its makepad branch, else its commercial one's, else its first other
/// nested half's (a landing of those alone).
fn lane_of(e: &Entry) -> &str {
    if e.branch != "-" { return &e.branch; }
    if !e.cbranch.is_empty() { return &e.cbranch; }
    e.nested.first().map_or(e.branch.as_str(), |side| side.branch.as_str())
}

fn attribute(repo: &Path, entries: &[Entry], failing: &[String]) -> (Vec<Vec<String>>, bool) {
    let changed = changed_by(repo, entries, failing);
    let mut attributed = vec![Vec::new(); entries.len()];
    let mut ambiguous = false;
    for unit in failing {
        let by = changed.get(unit).map_or(&[][..], Vec::as_slice);
        if by.is_empty() {
            ambiguous = true;
            for units in &mut attributed { units.push(unit.clone()); }
        } else {
            for &i in by { attributed[i].push(unit.clone()); }
        }
    }
    (attributed, ambiguous)
}

/// The landings among `entries` whose change moved each unit's inputs (its gateproof keys), by
/// index: none for a unit no landing changed, or for what is no unit ([`named_changers`]).
fn changed_by(repo: &Path, entries: &[Entry], failing: &[String]) -> BTreeMap<String, Vec<usize>> {
    let units: Vec<String> = failing.iter().filter(|u| is_unit(u)).cloned().collect();
    let mut keys = HashMap::new();
    for e in entries {
        for pair in [entry_pair(&e.pre), entry_pair(&e.landed)].into_iter().flatten() {
            keys.entry(pair.clone()).or_insert_with(|| crate::gateproof::blame_keys(repo, &pair, &units));
        }
    }
    failing.iter().map(|unit| (unit.clone(), if is_unit(unit) {
        named_changers(&entries.iter().enumerate().filter_map(|(i, e)| {
            let pre = entry_pair(&e.pre)?;
            let landed = entry_pair(&e.landed)?;
            let before = keys.get(&pre)?.get(unit)?;
            let after = keys.get(&landed)?.get(unit)?;
            (before != after).then(|| (i, before.as_ref().map(|(_, own)| own) != after.as_ref().map(|(_, own)| own)))
        }).collect::<Vec<_>>())
    } else { Vec::new() })).collect()
}

/// Of the landings that moved a unit's key (index, and whether it moved the unit's own inputs and
/// not only the gate's scripts), the ones a row names: those that changed the unit's package or
/// its dependencies. One that changed only the gate's scripts moved every unit's key, and was
/// named for each red of its range beside the landing whose change the red's own crate shows
/// (#2542, a gate script, for a Printability call the toolpath change broke, 11-07); it is never
/// named, and a red only such landings reach names nobody (its bisect finds the landing).
fn named_changers(moved: &[(usize, bool)]) -> Vec<usize> {
    moved.iter().filter(|(_, own)| *own).map(|(i, _)| *i).collect()
}

/// A landing as a bucket names it: `rc-x+rc-x (AGENT) #12`.
fn who_of(e: &Entry) -> String {
    format!("{} ({}) #{}", e.label(), e.agent, e.id)
}

/// The agents a run's new reds name a landing of, each with those landings: every failing unit's
/// who (`broke`) whose last look before this run (`before`, [`still_failing`]) was no red (a unit
/// red already was told at its first red), as [`who_of`] names a landing in it (`#12` is not `#123`).
fn told_agents(broke: &BTreeMap<String, String>, before: &BTreeMap<String, (String, String, String)>, history: &[Entry]) -> BTreeMap<String, Vec<u64>> {
    let new: Vec<&str> = broke.iter().filter(|(key, _)| !before.contains_key(*key)).map(|(_, who)| who.as_str()).collect();
    let mut told: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for e in history.iter().filter(|e| makepad_devhub_conf::valid_name(&e.agent)) {
        let named = who_of(e);
        let names = |who: &str| who.match_indices(&named).any(|(at, _)| !who[at + named.len()..].starts_with(|c: char| c.is_ascii_digit()));
        if new.iter().any(|who| names(who)) {
            let ids = told.entry(e.agent.clone()).or_default();
            if !ids.contains(&e.id) { ids.push(e.id); }
        }
    }
    told
}

/// A unit's package: `check <pkg>` without its `--also=` flips (one break shows under both).
pub fn unit_key(unit: &str) -> String {
    unit.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// A compile error line as `<file>: <message>`: rustc's `<path>:<line>:<col>: ` gives way to the
/// file's name (the node's tree path, or an edit shifting the line, is no new error). The message
/// ends at a tab: a present's summary line carries the error as a tab-separated record whose next
/// field changes from run to run, and each run after a break named the landing of its own bucket
/// as the breaker (a missing field broke Fab's build at #1926, then #1927-#1931 were named, 11-06).
pub(crate) fn error_key(line: &str) -> Option<String> {
    let (path, said) = location(line)?;
    Some(format!("{}: {}", path.rsplit('/').next().unwrap_or(path), message(said)))
}

/// rustc's `<path>:<line>:<col>: <message>`, as its path and message.
pub(crate) fn location(line: &str) -> Option<(&str, &str)> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let mut from = 0;
    while let Some(c) = line[from..].find(':') {
        let at = from + c;
        let rest = &line[at + 1..];
        let row = digits(rest);
        if row > 0 && rest[row..].starts_with(':') {
            let col = &rest[row + 1..];
            let n = digits(col);
            if n > 0 && col[n..].starts_with(": ") {
                return Some((&line[..at], &col[n + 2..]));
            }
        }
        from = at + 1;
    }
    None
}

/// An error's message as [`error_key`] keeps it: up to a tab. Rows from before kept what followed.
fn message(text: &str) -> &str {
    text.split('\t').next().unwrap_or(text).trim_end()
}

/// A run's failing units with their compile errors (each unit's own output: the lines after the
/// unit before it), or the tests a hung unit still ran (land_gate.sh's `HUNG: ` line), and the
/// units that passed, by package (a proven one passed at these inputs).
/// A build that could not compile (cargo's and the JIT's `error: could not compile `<crate>`
/// (<target>)`) is kept after its unit's errors, so its first line is still an error
/// ([`NOT_BUILT`], [`compile_red`]).
fn unit_lines(text: &str) -> (BTreeMap<String, Vec<String>>, BTreeSet<String>) {
    let (mut errors, mut passed, mut own, mut built) = (BTreeMap::new(), BTreeSet::new(), Vec::new(), Vec::<String>::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            match rest.split_once(' ') {
                Some(("fail", unit)) if !own.is_empty() || !built.is_empty() => {
                    let mut e = std::mem::take(&mut own);
                    e.sort();
                    e.dedup();
                    for b in built.drain(..) { if !e.contains(&b) { e.push(b); } }
                    errors.insert(unit.to_string(), e);
                }
                // (`gone`: its package left the tree, nothing is left to fail)
                Some(("ok" | "proven" | "gone", unit)) => { passed.insert(unit_key(unit)); }
                _ => {}
            }
            own.clear();
            built.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
            built.clear();
        } else if let Some(build) = line.strip_prefix("error: could not compile ").filter(|b| b.starts_with('`')) {
            built.push(format!("{NOT_BUILT}{}", crate::proto::clean(build.split(" due to ").next().unwrap_or(build).trim_end(), 200)));
        } else if line.starts_with("HUNG: ") {
            own.push(crate::proto::clean(line, 400));
        } else if let Some(said) = line.strip_prefix("EXIT: ") {
            // (a run's end with no error of its own: orch/warm.rs exit_said)
            own.push(crate::proto::clean(said, 400));
        } else if let Some(e) = error_key(line) {
            own.push(e);
        }
    }
    (errors, passed)
}

/// The files a run's failing units and steps say their box's tree lacked: cargo's `failed to read
/// `<path>`` and relax's `relax: <path>: .. no such file`, each as its path in the tree (what
/// follows the run's root: the JIT's `dev/src/`, a native run's `tree/`), by the unit
/// (`GATEUNIT fail`) or step (`GATESTEP fail`) whose lines they are.
fn lacked_files(text: &str) -> BTreeMap<String, Vec<String>> {
    let (mut by, mut own) = (BTreeMap::new(), Vec::<String>::new());
    for line in text.lines().map(str::trim) {
        if let Some(item) = line.strip_prefix("GATEUNIT fail ").or_else(|| line.strip_prefix("GATESTEP fail ")) {
            if !own.is_empty() { by.insert(item.trim().to_string(), std::mem::take(&mut own)); }
        } else if line.starts_with("GATEUNIT ") || line.starts_with("GATESTEP ") || line.starts_with("STEP ") {
            own.clear();
        } else if let Some(path) = lacked_path(line) {
            let path = path.replace('\\', "/");
            for root in ["/dev/src/", "/dev/tree/", "/tree/"] {
                let Some((_, at)) = path.split_once(root) else { continue };
                if !at.is_empty() && !own.iter().any(|o| o == at) { own.push(at.to_string()); }
            }
        }
    }
    by
}

/// The path of a line that says its run could not read a file: cargo's `failed to read `<path>``
/// (a manifest), relax's `relax: <path>: <path>: no such file`.
fn lacked_path(line: &str) -> Option<&str> {
    if let Some((_, rest)) = line.split_once("failed to read `") { return rest.split_once('`').map(|(p, _)| p); }
    let rest = line.strip_prefix("relax: ").filter(|_| line.to_ascii_lowercase().ends_with("no such file"))?;
    rest.split_once(": ").map(|(p, _)| p)
}

/// The failing units and steps whose box's tree lacked a file the pinned tree holds
/// ([`lacked_files`]), each with that file: the box's fault, no landing's. The pinned tree is
/// `tip`'s, with each followed repository it does not name at its tip in `repo`.
fn lacked_in_tree(repo: &Path, tip: &Pin, text: &str, failing: &[String]) -> BTreeMap<String, String> {
    let lacked: BTreeMap<String, Vec<String>> = lacked_files(text).into_iter().filter(|(u, _)| failing.contains(u)).collect();
    if lacked.is_empty() { return BTreeMap::new(); }
    let mut pin = tip.clone();
    for (name, _, sha) in crate::nested::tips(repo) { pin.nested.entry(name).or_insert(sha); }
    let Ok(tree) = crate::orch::ci::Tree::at_pin(repo, &pin) else { return BTreeMap::new() };
    lacked.into_iter().filter_map(|(u, paths)| Some((u, paths.into_iter().find(|p| tree.has(p))?))).collect()
}

/// A run's failing tests by unit, as `makepad test` names each (`FAILED <test>`: the unit's own
/// lines, those after the unit before it), and the units it ran that passed, by name (a proven one
/// passed at these inputs; a gone one has nothing left to fail).
fn test_lines(text: &str) -> (BTreeMap<String, Vec<String>>, BTreeSet<String>) {
    let (mut failed, mut ok, mut own) = (BTreeMap::new(), BTreeSet::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            match rest.split_once(' ') {
                Some(("fail", unit)) if !own.is_empty() => {
                    let mut tests = std::mem::take(&mut own);
                    tests.sort();
                    tests.dedup();
                    failed.insert(unit.to_string(), tests);
                }
                Some(("ok" | "proven" | "gone", unit)) => { ok.insert(unit.to_string()); }
                _ => {}
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if let Some(test) = line.strip_prefix("FAILED ").filter(|t| !t.is_empty() && !t.contains(char::is_whitespace)) {
            own.push(test.to_string());
        }
    }
    (failed, ok)
}

/// The panic lines a failing unit's tests printed (`panicked at <file>:<line>:<col> -- <message>`,
/// as `makepad test` says each under its `FAILED <test>`), the first [`PANICS_KEPT`], by unit. A
/// shard log holds them in the unit's result block, a hundred lines after its GATESTART/GATEEND,
/// and `devhub bucket` named only the test, so its red read as one without output (11-17).
fn panic_lines(text: &str) -> BTreeMap<String, Vec<String>> {
    let (mut out, mut own) = (BTreeMap::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            if let Some(("fail", unit)) = rest.split_once(' ') {
                if !own.is_empty() { out.insert(unit.to_string(), std::mem::take(&mut own)); }
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else if line.trim_start().starts_with("panicked at ") && own.len() < PANICS_KEPT {
            own.push(crate::proto::clean(line.trim_start(), 300));
        }
    }
    out
}

/// The lines of the tree each failing test's message names, by unit and test: `<path>:<line>` (a
/// column after it or not) after `panicked at <where> -- ` and in the indented lines under it.
/// Where it panicked is no such line: a harness's assert stands over the line its check found
/// (`relax check --boundary options` names the line that needs an Option entry, 11-25).
fn said_lines(text: &str) -> BTreeMap<(String, String), Vec<(String, usize)>> {
    let (mut out, mut own, mut said) = (BTreeMap::new(), Vec::<(String, Vec<(String, usize)>)>::new(), false);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            if let Some(("fail", unit)) = rest.split_once(' ') {
                for (test, mut lines) in own.drain(..).filter(|(_, l)| !l.is_empty()) {
                    lines.sort();
                    lines.dedup();
                    out.insert((unit.to_string(), test), lines);
                }
            }
            own.clear();
            said = false;
        } else if line.starts_with("STEP ") {
            own.clear();
            said = false;
        } else if let Some(test) = line.strip_prefix("FAILED ").filter(|t| !t.is_empty() && !t.contains(char::is_whitespace)) {
            own.push((test.to_string(), Vec::new()));
            said = false;
        } else {
            let msg = line.trim_start().strip_prefix("panicked at ").and_then(|r| r.split_once(" -- ")).map(|(_, m)| m);
            let more = msg.or_else(|| (said && line.starts_with("    ")).then_some(line));
            match (own.last_mut(), more) {
                (Some((_, lines)), Some(more)) => {
                    lines.extend(tree_lines(more));
                    said = true;
                }
                _ => said = false,
            }
        }
    }
    out
}

/// The `<path>:<line>` words of `text` that can name a file of the tree: a relative path with a
/// directory and a file name with an extension, its line a number (a column after it dropped).
fn tree_lines(text: &str) -> Vec<(String, usize)> {
    text.split(|c: char| c.is_whitespace() || "()[]{}<>,;'\"`".contains(c)).filter_map(|w| {
        let mut parts = w.split(':');
        let (path, n) = (parts.next()?, parts.next()?.parse::<usize>().ok()?);
        let file = path.rsplit_once('/')?.1;
        (!path.starts_with('/') && file.contains('.') && n > 0).then(|| (path.to_string(), n))
    }).collect()
}

/// How a failed listing's end with no error of its own starts when it had listed its targets whole
/// (orch/warm.rs exit_said): its red is no test of its lane's.
pub(crate) const EXIT_AFTER_LISTING: &str = "exit after listing";

/// The panic lines kept per failing unit ([`panic_lines`]).
const PANICS_KEPT: usize = 3;

/// What a unit's failing tests' blame starts with ([`tests_blame`]).
pub const BY_TEST: &str = "by test: ";

/// The tests a blame names at most per breaker (`a, b, c and 2 more since X`).
const TESTS_NAMED: usize = 3;

/// What a bucket row saw of a test.
#[derive(Debug, PartialEq)]
enum Look {
    /// it failed there, in that unit
    Failed(String),
    /// it passed: its unit or its whole package passed, or its unit failed naming other tests
    Passed,
    /// no unit that holds it ran, or one failed naming none (a build that stopped before its tests,
    /// or a whole package's run that failed elsewhere, which may have stopped before its target)
    Not,
}

/// A row as [`test_blame`] reads it: one from before rows named their tests (11-07) with what its
/// kept log says ([`test_lines`]), and who changed each failing unit by its landings' units (none
/// for a unit its ambiguous run spread over every landing). As it is with its log gone.
fn named_row(row: &Value) -> Value {
    if row.get("ran_ok").is_some() { return row.clone(); }
    let Some(text) = row.get("log").and_then(Value::as_str).and_then(|l| std::fs::read_to_string(l).ok()) else { return row.clone() };
    let (failed, ok) = test_lines(last_run(&text));
    let landings = row.get("landings").and_then(Value::as_arr).unwrap_or_default();
    let spread = row.get("ambiguous").and_then(Value::as_bool) == Some(true);
    let said = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let changed: Vec<(String, Value)> = failed.keys().filter_map(|unit| {
        let by: Vec<String> = landings.iter().filter(|e| e.get("units").and_then(Value::as_arr).is_some_and(|us| us.iter().any(|u| u.as_str() == Some(unit.as_str()))))
            .map(|e| format!("{} ({}) #{}", said(e, "label"), said(e, "agent"), e.get("id").and_then(Value::as_u64).unwrap_or(0))).collect();
        (!by.is_empty() && !(spread && by.len() == landings.len())).then(|| (unit.clone(), s(&by.join(", "))))
    }).collect();
    let mut row = row.clone();
    if let Value::Obj(fields) = &mut row {
        fields.push(("tests_failed".into(), Value::Obj(failed.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(s).collect()))).collect())));
        fields.push(("ran_ok".into(), Value::Arr(ok.iter().map(s).collect())));
        fields.push(("changed".into(), Value::Obj(changed)));
    }
    row
}

/// What `row` saw of `test` of `unit`. None: a row from before rows named their tests (11-07)
/// whose log is gone ([`named_row`]).
fn looked(row: &Value, unit: &str, test: &str) -> Option<Look> {
    let ok = row.get("ran_ok").and_then(Value::as_arr)?;
    let Some(Value::Obj(failed)) = row.get("tests_failed") else { return None };
    let mut seen = false;
    for (ran, tests) in failed.iter().filter(|(ran, _)| ran == unit || *ran == unit_key(unit)) {
        let tests = tests.as_arr().unwrap_or_default();
        if tests.iter().any(|t| t.as_str() == Some(test)) { return Some(Look::Failed(ran.clone())); }
        seen |= ran == unit && !tests.is_empty();
    }
    seen |= ok.iter().filter_map(Value::as_str).any(|ran| ran == unit || ran == unit_key(unit));
    Some(if seen { Look::Passed } else { Look::Not })
}

/// A row's log, by its file name.
fn log_name(row: &Value) -> Option<String> {
    row.get("log").and_then(Value::as_str).and_then(|l| Path::new(l).file_name()).map(|l| l.to_string_lossy().into_owned())
}

/// Who broke `test` of `unit`: the landings that changed its unit in the first run where it
/// failed after a run where it passed. With no such pair in the ledger nobody knows, and no
/// landing is named: why (no green baseline: it failed in every run back to the first log, and
/// the ledger ends there or the run before it has no log to read). All nine red tests of the
/// first run that named its tests said only `origin unknown` (11-07). A unit's breaker is no test's: the
/// landings that changed a package were named for every test failing in it, and a pass of any of
/// its targets ended the look back, so whatever landed next was named (one package's tests for a
/// devhub-only landing, another's for two that changed only its tests' support, 11-07).
/// A test that failed before that pass too fails off and on: it passed at a tip as new as its
/// earlier red's or newer, so the landings since that pass are no more why than the ones before
/// ([`INTERMITTENT`]). A Fab test's E2BIG, red in 2 of 5 runs, named two emulator landings of the
/// third red's range (11-19). Unless both reds kept their panics and these share none
/// ([`breaks_differ`]): then that pass ended one break and the newer red is another, laid to its
/// run's landings as any first failure is. An added lease field's round trip failed for `untimed`
/// on 11-24 and for `lost_on` on 11-25, passing between, and read as a test failing off and on.
/// `rows`: newest first, from a run where it failed.
fn test_blame(rows: &[Value], unit: &str, test: &str) -> String {
    let mut first: Option<(&Value, String)> = None;
    for (i, row) in rows.iter().enumerate() {
        match looked(row, unit, test) {
            Some(Look::Failed(ran)) => first = Some((row, ran)),
            Some(Look::Not) => {}
            Some(Look::Passed) => {
                let Some((failed, ran)) = first else { return ORIGIN_UNKNOWN.to_string() };
                let before = rows[i + 1..].iter().find_map(|r| match looked(r, unit, test) { Some(Look::Failed(was)) => Some((r, was)), _ => None });
                if let Some((before, _)) = before.filter(|(before, was)| !breaks_differ(before, was, failed, &ran)) {
                    return format!("{INTERMITTENT}: it failed in {}, passed in the later {}, and fails again", log_name(before).unwrap_or_default(), log_name(row).unwrap_or_default());
                }
                let row = failed;
                return match row.get("changed").and_then(|c| c.get(&ran)).and_then(Value::as_str).filter(|w| !w.is_empty()) {
                    Some(who) => who.to_string(),
                    None => format!("{ORIGIN_UNKNOWN}: no landing of its first failing run ({}) changed {ran}", log_name(row).unwrap_or_default()),
                };
            }
            None => {
                let before = log_name(row).map_or("its ledger row names none".to_string(), |l| format!("{l} is gone"));
                return match first.and_then(|(row, _)| log_name(row)) {
                    Some(log) => format!("{ORIGIN_UNKNOWN}: no green baseline, it failed in every run back to {log}, and the log of the run before ({before}) is not there to read its tests from"),
                    None => ORIGIN_UNKNOWN.to_string(),
                };
            }
        }
    }
    match first.and_then(|(row, _)| log_name(row)) {
        Some(log) => format!("{ORIGIN_UNKNOWN}: no green baseline, it failed in every run the ledger keeps, the first {log}"),
        None => ORIGIN_UNKNOWN.to_string(),
    }
}

/// Whether unit `ua`'s red in row `a` and unit `ub`'s in row `b` are two breaks: both rows kept
/// their panics ([`panic_lines`]) and no message is in both, each said without where it panicked
/// and with each run of digits as one `#` (a flaky test's timings differ from run to run; two
/// breaks of one assertion name different things). False when either kept none: nothing tells.
fn breaks_differ(a: &Value, ua: &str, b: &Value, ub: &str) -> bool {
    let said = |row: &Value, unit: &str| -> BTreeSet<String> {
        let Some(Value::Obj(p)) = row.get("panics") else { return BTreeSet::new() };
        p.iter().filter(|(u, _)| u.as_str() == unit).flat_map(|(_, l)| l.as_arr().unwrap_or_default()).filter_map(Value::as_str).map(|l| {
            let mut out = String::new();
            for c in l.split_once(" -- ").map_or(l, |(_, m)| m).chars() {
                if !c.is_ascii_digit() { out.push(c) } else if !out.ends_with('#') { out.push('#') }
            }
            out
        }).collect()
    };
    let (x, y) = (said(a, ua), said(b, ub));
    !x.is_empty() && !y.is_empty() && x.is_disjoint(&y)
}

/// Who broke each failing test of one package's units ([`test_blame`]), those with one breaker
/// together: `by test: a, b since X; c origin unknown, first failed in L`. `by_line`: who added a
/// line of the tree a test's message names, which comes first ([`said_lines`]).
fn tests_blame(rows: &[Value], named: &[(String, Vec<String>)], by_line: &dyn Fn(&str, &str) -> Option<String>) -> String {
    let mut by: Vec<(String, Vec<&str>)> = Vec::new();
    for (unit, tests) in named {
        for test in tests {
            let who = by_line(unit, test).unwrap_or_else(|| test_blame(rows, unit, test));
            match by.iter_mut().find(|(w, _)| *w == who) {
                Some((_, them)) => them.push(test.as_str()),
                None => by.push((who, vec![test.as_str()])),
            }
        }
    }
    let said: Vec<String> = by.iter().map(|(who, them)| {
        let more = them.len().saturating_sub(TESTS_NAMED);
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        format!("{}{more} {}", them[..them.len().min(TESTS_NAMED)].join(", "), still_since(who))
    }).collect();
    format!("{BY_TEST}{}", said.join("; "))
}

/// A row's failing units of package `key` with the tests each named.
fn named_tests(row: &Value, key: &str) -> Vec<(String, Vec<String>)> {
    let Some(Value::Obj(failed)) = row.get("tests_failed") else { return Vec::new() };
    failed.iter().filter(|(unit, _)| unit_key(unit) == key)
        .map(|(unit, tests)| (unit.clone(), tests.as_arr().unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()))
        .filter(|(_, tests)| !tests.is_empty()).collect()
}

/// A run's proven unit keys, by package, read from `GATEUNIT proven <unit>` lines.
fn proven_lines(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(|l| l.strip_prefix("GATEUNIT proven ")).map(unit_key).collect()
}

/// The command each unit of a run said shows it alone (`GATEREPRO <unit> :: <command>`: a unit
/// that failed), by package, for the lane that owns it to run: a unit's name is no command.
fn repro_lines(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|l| l.strip_prefix("GATEREPRO ")?.split_once(" :: ")).map(|(unit, cmd)| (unit_key(unit), cmd.trim().to_string())).collect()
}

/// A unit's command, as `bucket status` and the bucket's FAILED lines say it. A present's (the
/// gate ran the app attended and hidden, its stderr in the run's out.log, and fails it on a panic
/// or a JIT compile error there) is said with its live session beside it: the app on its box's
/// JIT as a lane looks at one, where the run's own stderr says why it presents no frame.
pub fn repro_said(unit: &str, cmd: &str) -> String {
    match unit.strip_prefix("present ") {
        Some(pkg) => format!("{unit}: shown alone by `{cmd}` (its stderr says why), or from a lane by its live session: `makepad devhub live start {pkg}`, then `makepad devhub live look <id>`"),
        None => format!("{unit}: shown alone by `{cmd}`"),
    }
}

/// The failing units already broken at this bucket's last look at them, with who broke them: a
/// unit whose every error now was in its last failure, with no run of the bucket passing it
/// since. A later landing that changed such a unit's inputs did not break it (one landing broke a
/// phone host check, and each later devhub landing was named for it, 11-03). A unit with no
/// compile error (a present, a test run) failing again is the same break, and so is a failure in
/// a row that kept no errors to compare with: rechecked a day later, one was named for whatever
/// had just landed (11-04). `rows`: the bucket's earlier rows, newest first.
fn carried_since(rows: &[Value], failing: &[String], errors: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for unit in failing.iter().filter(|u| is_unit(u)) {
        let key = unit_key(unit);
        let now = errors.get(unit).map_or(&[][..], Vec::as_slice);
        for row in rows {
            if row.get("passed").and_then(Value::as_arr).is_some_and(|p| p.iter().any(|k| k.as_str() == Some(key.as_str()))) { break; }
            let failed = match row.get("errors") {
                Some(Value::Obj(failed)) => failed.iter().find(|(u, _)| unit_key(u) == key),
                _ => None,
            };
            let listed = row.get("failing").and_then(Value::as_arr).is_some_and(|f| f.iter().any(|u| u.as_str().is_some_and(|u| unit_key(u) == key)));
            if failed.is_none() && !listed { continue; }
            let before: Vec<&str> = failed.and_then(|(_, b)| b.as_arr()).unwrap_or_default().iter().filter_map(Value::as_str).map(message).collect();
            // (its build's line frames its errors: a row from before it was kept is the same break)
            if row.get("errors").is_none() || now.iter().filter(|e| !e.starts_with(NOT_BUILT)).all(|e| before.contains(&e.as_str())) {
                out.insert(unit.clone(), origin(rows, &key));
            }
            break;
        }
    }
    out
}

/// Who broke unit `key`, failing with no pass since: what the first run to fail it said, else that
/// nobody knows, with that run's log and where its output is kept. A later run's name is no
/// better: it named who changed the unit then. `rows`: the bucket's earlier rows, newest first.
fn origin(rows: &[Value], key: &str) -> String {
    let lists = |list: &str, row: &Value| row.get(list).and_then(Value::as_arr).is_some_and(|l| l.iter().any(|u| u.as_str().is_some_and(|u| unit_key(u) == key)));
    let failed = |row: &Value| lists("failing", row) || matches!(row.get("errors"), Some(Value::Obj(e)) if e.iter().any(|(u, _)| unit_key(u) == key));
    // a unit whose last failing run named its tests: who broke each of them
    if let Some(i) = rows.iter().position(|row| failed(row)) {
        let named = named_tests(&rows[i], key);
        // as its run said it, from the rows and logs it read ([`named_row`])
        if let Some(who) = rows[i].get("broke").and_then(|b| b.get(key)).and_then(Value::as_str).filter(|w| w.starts_with(BY_TEST)).filter(|_| !named.is_empty()) {
            return who.to_string();
        }
        if !named.is_empty() { return tests_blame(&rows[i..], &named, &|_, _| None); }
        // one that could not build named no test: that build is its red
        if let Some(red) = compile_red(rows, i, key) { return red; }
    }
    let first = rows.iter().take_while(|row| !lists("passed", row)).filter(|row| failed(row)).last();
    if let Some(who) = first.and_then(|row| row.get("broke")).and_then(|b| b.get(key)).and_then(Value::as_str) {
        return who.to_string();
    }
    let log = first.and_then(|row| row.get("log")).and_then(Value::as_str).and_then(|l| Path::new(l).file_name()).map(|l| l.to_string_lossy().into_owned());
    // a CI leg's red read "origin unknown, first failed in ci-...log" for 21 h, and its own log,
    // which said why, was gone (11-24)
    let kept = first.and_then(|row| row.get("kept")).and_then(|k| k.get(key)).and_then(Value::as_str).map_or(String::new(), |at| format!(" (its output: {at})"));
    log.map_or(ORIGIN_UNKNOWN.to_string(), |log| format!("{ORIGIN_UNKNOWN}, first failed in {log}{kept}"))
}

/// Unit `key`'s red in `rows[i]`, its newest failing run, when it could not build there: that
/// build, its first error and who broke it (as the oldest run with those errors since, by
/// [`carried_since`]'s rule, said), never a test an older run failed. A run that stops at a
/// build says nothing of that test: a test build's E0433 read "by test: requests_round_trip
/// since .." for a test the same run passed (11-24). None: it built.
fn compile_red(rows: &[Value], i: usize, key: &str) -> Option<String> {
    let lines = |row: &Value| -> Vec<String> {
        match row.get("errors") {
            Some(Value::Obj(e)) => e.iter().filter(|(u, _)| unit_key(u) == key).flat_map(|(_, l)| l.as_arr().unwrap_or_default()).filter_map(Value::as_str).map(|l| message(l).to_string()).collect(),
            _ => Vec::new(),
        }
    };
    let now = lines(&rows[i]);
    let built: Vec<&str> = now.iter().filter_map(|l| l.strip_prefix(NOT_BUILT)).collect();
    if built.is_empty() { return None; }
    let errors: Vec<&String> = now.iter().filter(|l| !l.starts_with(NOT_BUILT)).collect();
    let mut from = i;
    for (j, row) in rows.iter().enumerate().skip(i + 1) {
        let before = lines(row);
        if row_lists(row, "passed", key) || before.is_empty() || !errors.iter().all(|e| before.contains(e)) { break; }
        from = j;
    }
    let who = rows[from].get("broke").and_then(|b| b.get(key)).and_then(Value::as_str).filter(|w| !w.is_empty() && !w.starts_with(BY_TEST));
    if let Some(who) = who.filter(|w| w.starts_with(COMPILE_RED)) { return Some(who.to_string()); }
    let first = errors.first().map_or(String::new(), |e| format!(": {e}"));
    let since = who.map_or_else(|| format!(", {ORIGIN_UNKNOWN}"), |who| format!(" {}", still_since(who)));
    Some(format!("{COMPILE_RED}{}{first}{since}", built.join(" and ")))
}

/// What a red that could not build is laid to ([`compile_red`]).
pub const COMPILE_RED: &str = "compile red: ";

/// How a build that could not compile starts among its unit's errors ([`unit_lines`]).
const NOT_BUILT: &str = "could not compile ";

/// A failure older than what the ledger can name a landing for.
pub const ORIGIN_UNKNOWN: &str = "origin unknown";

/// What a test that fails off and on is laid to ([`test_blame`]): no landing.
pub const INTERMITTENT: &str = "intermittent, origin unknown";

/// What a unit red only in test listings its bucket never had a result for is laid to: its
/// target's owner ([`first_run_owner`]), not the landings under its first run.
pub const FIRST_RUN: &str = "first run here, red at its first run";

/// Each unit's test listings in a run's log, as the prebuild bucket writes them (orch/warm.rs
/// judge): the `<pkg>:<target> on <box>: exit <code>` line that heads each listing's lines before
/// its `GATEUNIT <verdict> suite <pkg>` line, with whether it passed.
fn listing_heads(text: &str) -> BTreeMap<String, Vec<(String, bool)>> {
    let head = |line: &str| -> Option<(String, bool)> {
        let (said, code) = line.rsplit_once(": exit ")?;
        let code: i64 = code.parse().ok()?;
        let (label, node) = said.split_once(" on ")?;
        (!label.is_empty() && !label.contains(' ') && !node.is_empty() && !node.contains(' ')).then(|| (label.to_string(), code == 0))
    };
    let (mut out, mut own): (BTreeMap<String, Vec<(String, bool)>>, Option<(String, bool)>) = (BTreeMap::new(), None);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            let unit = rest.split_once(' ').map(|(_, unit)| unit);
            // (a listing of the unit's own package: another bucket's line of that shape is none)
            let pkg = unit.and_then(|u| u.strip_prefix("suite "));
            if let (Some(unit), Some(pkg), Some(listed)) = (unit, pkg, own.take()) {
                if listed.0.split(':').next() == Some(pkg) { out.entry(unit.to_string()).or_default().push(listed); }
            }
        } else if own.is_none() {
            own = head(line);
        }
    }
    out
}

/// The test listings of `unit` that the bucket's `earlier` rows got a result for: a row's
/// `targets`, or the listing heads of the log of a row from before rows kept them. None: a row
/// that ran the unit keeps neither, so whether a listing is new there is not known.
fn listed_before(earlier: &[Value], unit: &str) -> Option<BTreeSet<String>> {
    let mut seen = BTreeSet::new();
    let lists = |row: &Value, field: &str| row.get(field).and_then(Value::as_arr).unwrap_or_default().iter().any(|u| u.as_str() == Some(unit));
    for row in earlier.iter().filter(|row| lists(row, "passed") || lists(row, "failing")) {
        match row.get("targets") {
            Some(targets) => seen.extend(targets.get(unit).and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string)),
            None => {
                let text = std::fs::read_to_string(row.get("log").and_then(Value::as_str)?).ok()?;
                seen.extend(listing_heads(last_run(&text)).remove(unit).unwrap_or_default().into_iter().map(|(label, _)| label));
            }
        }
    }
    Some(seen)
}

/// Who a red at its first run here ([`FIRST_RUN`]) of the listings `labels` goes to: the landing
/// that last changed an integration target's own file at `tip`; none found, main.
fn first_run_owner(repo: &Path, history: &[Entry], tip: &Pair, labels: &[&str]) -> String {
    let said = format!("{FIRST_RUN} ({})", labels.join(", "));
    let tree = crate::orch::ci::Tree::at(repo, tip).ok();
    let files: Vec<String> = labels.iter().filter_map(|label| label.split_once(':'))
        .filter(|(_, target)| !matches!(*target, "lib" | "default") && !target.contains(':'))
        .filter_map(|(pkg, target)| crate::orch::ci::target_files(tree.as_ref()?, pkg, Some(target))?.into_iter().next()).collect();
    match files.iter().find_map(|file| owner_of(repo, history, tip, file).map(|e| (e, file))) {
        Some((e, file)) => format!("{said}: its owner {}, which last changed {file}", who_of(e)),
        None if files.is_empty() => format!("{said}: its owner unknown: no test file found for it"),
        None => format!("{said}: its owner unknown: no landing found for {}", files.join(", ")),
    }
}

/// The files landing `e` changed, the nested commercial checkout's under `apps/commercial/`.
/// None: a moved side's commits are not in the repository.
fn landing_files(repo: &Path, e: &Entry) -> Option<Vec<String>> {
    let (pre, landed) = (entry_pair(&e.pre)?, entry_pair(&e.landed)?);
    let mut files = if pre.work == landed.work { Vec::new() } else { crate::gitread::files_differing(repo, &pre.work, &landed.work)? };
    if pre.main != landed.main {
        files.extend(crate::gitread::files_differing(&repo.join("apps/commercial"), &pre.main, &landed.main)?.into_iter().map(|f| format!("apps/commercial/{f}")));
    }
    // its other nested halves: each landed commit against its first parent, the tip it landed on
    for side in e.nested.iter().filter(|side| side.name != crate::state::PAIRED && !side.landed.is_empty()) {
        let dir = repo.join(crate::nested::dir(&side.name));
        let parent = crate::gitread::with(&dir, |p, r| {
            let oid = crate::gitread::resolve(p, r, &side.landed)?;
            r.first_parents(oid).nth(1).and_then(Result::ok).map(|(o, _)| o.to_hex())
        }).flatten()?;
        files.extend(crate::gitread::files_differing(&dir, &parent, &side.landed)?.into_iter().map(|f| format!("{}{f}", crate::nested::prefix(&side.name))));
    }
    Some(files)
}

/// The commit that last touched the file rustc named `file` at `at`: its longest tail that is a
/// file of the tree, the nested commercial checkout's under `apps/commercial/`.
fn last_touch(repo: &Path, at: &Pair, file: &str) -> Option<String> {
    let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty()).collect();
    (0..parts.len()).find_map(|i| {
        let rel = parts[i..].join("/");
        match rel.strip_prefix("apps/commercial/") {
            Some(p) => crate::gitread::last_touch(&repo.join("apps/commercial"), &at.main, p),
            None => crate::gitread::last_touch(repo, &at.work, &rel),
        }
    })
}

/// The source lines the statics step names (restart_statics.py: a static with no reset and no
/// reason, `<key> (<path>:<line>)`), as the bucket's tip has them, with the raw failing line.
fn statics_lines(text: &str) -> Vec<(String, usize, String)> {
    text.lines().filter_map(|l| {
        let rest = l.strip_prefix("no reset and no reason: ")?;
        let (path, n) = rest.rsplit_once(" (")?.1.strip_suffix(')')?.rsplit_once(':')?;
        Some((path.to_string(), n.parse().ok()?, l.to_string()))
    }).collect()
}

/// The text of `path` at `at`, the nested commercial checkout's under `apps/commercial/`.
fn file_text(repo: &Path, at: &Pair, path: &str) -> Option<String> {
    let (dir, commit, rel) = match path.strip_prefix("apps/commercial/") {
        Some(rel) => (repo.join("apps/commercial"), at.main.as_str(), rel),
        None => (repo.to_path_buf(), at.work.as_str(), path),
    };
    let oid = crate::gitread::entries_at(&dir, commit, &[rel])?.pop().filter(|o| !o.is_empty())?;
    crate::gitread::blob(&dir, &oid).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Of `since` (oldest first), the landing whose diff added `line`, the text of `path`'s line `n`
/// at `tip`: the newest one that lacked it before and had it after. None when none did (an older
/// line a landing broke otherwise, or one from before the range).
fn line_blame<'a>(since: &'a [Entry], tip: &Pair, path: &str, n: usize, text: &dyn Fn(&Pair, &str) -> Option<String>) -> Option<&'a Entry> {
    let line = text(tip, path)?.lines().nth(n.checked_sub(1)?)?.trim().to_string();
    if line.is_empty() { return None; }
    let has = |at: &Pair| text(at, path).is_some_and(|t| t.lines().any(|l| l.trim() == line));
    since.iter().rev().find(|e| entry_pair(&e.landed).is_some_and(|at| has(&at)) && entry_pair(&e.pre).is_some_and(|at| !has(&at)))
}

/// The statics step's who (`<landing> (it added <file>:<line>)`, comma-separated) less the landings
/// `last`, its who at its previous red run, named already (`#12` is not `#123`).
fn statics_new(who: &str, last: &str) -> String {
    let named = |w: &str| last.match_indices(w).any(|(at, _)| !last[at + w.len()..].starts_with(|c: char| c.is_ascii_digit()));
    who.split(", ").filter(|w| !named(w.split(" (it added ").next().unwrap_or(w))).collect::<Vec<_>>().join(", ")
}

/// Whether `file`, as rustc or the JIT names it (the tree's path, or one under a node's tree), is
/// `p`, a file of the tree.
fn touches(file: &str, p: &str) -> bool {
    file == p || file.strip_suffix(p).is_some_and(|head| head.ends_with('/'))
}

/// Each failing unit's error files, as its errors name them, in the unit's own lines as
/// [`unit_lines`] reads them. A present's summary names a file after the text before it: its path
/// is the last word. A warning's file is one only for a unit a warning turns red (a JIT check of a
/// package's program build, a depcheck, a test target's check, a wasm check: zero warnings) when
/// no error names a file; a present's or a test run's warning is not why it failed. A red on one
/// unused method was named for all 8 landings of its range, the one that added it among them (11-12).
fn error_files(text: &str) -> BTreeMap<String, Vec<String>> {
    let (mut files, mut own, mut warned) = (BTreeMap::new(), Vec::new(), Vec::new());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            if let Some(("fail", unit)) = rest.split_once(' ') {
                let warns_red = ["check ", "cargo ", "tests ", "wasm ", "windows "].iter().any(|p| unit.starts_with(p));
                let mut f = if own.is_empty() && warns_red { std::mem::take(&mut warned) } else { std::mem::take(&mut own) };
                f.sort();
                f.dedup();
                if !f.is_empty() { files.insert(unit.to_string(), f); }
            }
            own.clear();
            warned.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
            warned.clear();
        } else if let Some((path, said)) = location(line) {
            let path = path.rsplit(char::is_whitespace).next().unwrap_or(path).to_string();
            if said.starts_with("warning") { warned.push(path) } else { own.push(path) }
        }
    }
    files
}

/// The files a run's own failing steps name: each located error or warning line no unit claims
/// (a step's lines end at its `STEP` line; a unit's at its `GATEUNIT` line). A step's warning is
/// one: the JIT's build fails on any. A JIT that would not build on one unused function was named
/// for every landing of two ranges, and the landing that added it was in neither (#4024's
/// check_reuse.rs, 11-12).
fn step_files(text: &str) -> Vec<String> {
    let (mut files, mut own) = (Vec::new(), Vec::new());
    for line in text.lines() {
        if line.starts_with("GATEUNIT ") {
            own.clear();
        } else if line.starts_with("STEP ") {
            files.append(&mut own);
        } else if let Some((path, _)) = location(line) {
            own.push(path.to_string());
        }
    }
    files.append(&mut own);
    files.sort();
    files.dedup();
    files
}

/// The tip of the last of `rows` (the bucket's earlier rows, newest first) that passed unit `key`;
/// a step (no unit) passed in each row that does not list it failing.
fn last_pass(rows: &[Value], key: &str) -> Option<Pin> {
    let has = |row: &Value, list: &str| row.get(list).and_then(Value::as_arr).is_some_and(|p| p.iter().any(|k| k.as_str() == Some(key)));
    rows.iter().find(|row| if is_unit(key) || is_leg(key) { has(row, "passed") } else { row.get("failing").is_some() && !has(row, "failing") })
        .and_then(|row| row.get("tip")).and_then(read_tip)
}

/// A new red whose attribution names nobody or several landings, or a failing test's: one search
/// since its last pass, kept across master restarts (11-15). A test's first probe is its red tip
/// ([`run_bisect`]); `closed`: a later run passed it already, and only that probe runs.
#[derive(Clone)]
pub(crate) struct Bisect {
    bucket: String, unit: String, key: String, from: Pin, tip: Pin, line: String, log: String, closed: bool,
    /// a CI leg's: the landings its error lines' files name ([`file_blame`]), told should it fail
    /// again at its tip
    named: Vec<u64>,
    /// a CI leg's red tip probe, once it came back red (its `tip-red` ledger row): a resumed job
    /// bisects without probing it again
    tip_probe: Option<Value>,
    /// a test's: every target of its package the red run failed (`unit` the first), probed
    /// together ([`Bisect::targets`])
    units: Vec<String>,
}

impl Bisect {
    fn value(&self, state: &str, extra: Vec<(&str, Value)>) -> Value {
        let mut fields = vec![("bucket", s(&self.bucket)), ("key", s(&self.key)), ("unit", s(&self.unit)),
            ("from", tip_value(&self.from)), ("tip", tip_value(&self.tip)), ("line", s(&self.line)), ("log", s(&self.log)), ("closed", Value::Bool(self.closed)),
            ("named", Value::Arr(self.named.iter().map(|id| Value::Int(*id as i64)).collect())), ("state", s(state))];
        if self.units.len() > 1 { fields.push(("units", Value::Arr(self.units.iter().map(|u| s(u)).collect()))); }
        fields.extend(extra);
        obj(fields)
    }

    fn read(v: &Value) -> Option<Bisect> {
        let text = |k| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Bisect { bucket: text("bucket")?, unit: text("unit")?, key: text("key")?, from: read_tip(v.get("from")?)?,
            tip: read_tip(v.get("tip")?)?, line: text("line")?, log: text("log")?, closed: v.get("closed").and_then(Value::as_bool).unwrap_or(false),
            named: v.get("named").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_u64).collect(),
            tip_probe: v.get("probe").cloned(),
            units: v.get("units").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect() })
    }

    /// What its probes run: a test's every failing target of its package, else its unit. One run
    /// failed 13 makepad-fab targets, most of them every time (#7560 removed ids their selectors
    /// name); its one bisect probed chrome alone, which passed, and the run read as flaky (11-20).
    fn targets(&self) -> Vec<String> {
        let mut targets = vec![self.unit.clone()];
        targets.extend(self.units.iter().filter(|u| **u != self.unit).cloned());
        targets
    }

    /// A test's red: a run of the unit can pass where another failed (a hang guard racing a cold
    /// JIT), so its tip is probed before any landing is.
    fn of_a_test(&self) -> bool {
        self.key.starts_with("test ")
    }

    /// A CI leg's red: its tip is probed first ([`leg_at_tip`]), and a red one its error lines do
    /// not pin on one landing is bisected at night ([`wait_for_night`]).
    fn of_a_leg(&self) -> bool {
        is_leg(&self.key)
    }
}

fn named_ids(who: &str) -> BTreeSet<u64> {
    who.split('#').skip(1).filter_map(|rest| {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        rest[..n].parse().ok()
    }).collect()
}

/// The pair's pin with the other repositories the red run pinned (11-15).
fn bisect_pin(mut pin: Pin, tip: &Pin) -> Pin {
    for (name, sha) in &tip.nested { pin.nested.entry(name.clone()).or_insert_with(|| sha.clone()); }
    pin
}

fn bisect_key(v: &Value) -> Option<(&str, &str, &str)> {
    Some((v.get("bucket")?.as_str()?, v.get("key")?.as_str()?, v.get("from")?.get("work")?.as_str()?))
}

// The callers hold BISECTS across the read and write (11-15).
fn bisect_rows(home: &Path) -> std::io::Result<Vec<Value>> {
    let file = match std::fs::File::open(home.join("bisects.jsonl")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut rows = Vec::new();
    for line in BufReader::new(file).lines() {
        if let Ok(v) = makepad_strict_json::parse(line?.as_bytes()) { rows.push(v); }
    }
    Ok(rows)
}

fn append_bisect(home: &Path, row: Value) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(home.join("bisects.jsonl"))?;
    let mut line = row.to_json();
    line.push('\n');
    file.write_all(line.as_bytes())
}

fn queue_bisect(home: &Path, job: &Bisect) -> std::io::Result<bool> {
    let _lock = BISECTS.lock().unwrap();
    let key = (job.bucket.as_str(), job.key.as_str(), job.from.makepad.as_str());
    if bisect_rows(home)?.iter().any(|v| bisect_key(v) == Some(key)) { return Ok(false); }
    append_bisect(home, job.value("queued", Vec::new()))?;
    Ok(true)
}

fn pending_bisects(home: &Path, name: &str) -> std::io::Result<Vec<Bisect>> {
    let _lock = BISECTS.lock().unwrap();
    let rows = bisect_rows(home)?;
    let state = |v: &Value| v.get("state").and_then(Value::as_str).map(str::to_string);
    let done: BTreeSet<_> = rows.iter().filter(|v| state(v).as_deref() == Some("done")).filter_map(bisect_key).collect();
    // a leg's red tip probe, recorded: the job resumes at its bisect
    let tipped: BTreeMap<_, &Value> = rows.iter().filter(|v| state(v).as_deref() == Some("tip-red")).filter_map(|v| Some((bisect_key(v)?, v))).collect();
    let mut seen = BTreeSet::new();
    Ok(rows.iter().filter(|v| v.get("bucket").and_then(Value::as_str) == Some(name) && state(v).as_deref() == Some("queued"))
        .filter(|v| bisect_key(*v).is_some_and(|key| !done.contains(&key) && seen.insert(key)))
        .filter_map(|v| bisect_key(v).and_then(|key| tipped.get(&key).copied()).or(Some(v))).filter_map(Bisect::read).collect())
}

fn done_bisect(home: &Path, job: &Bisect, extra: Vec<(&str, Value)>) -> std::io::Result<()> {
    let _lock = BISECTS.lock().unwrap();
    append_bisect(home, job.value("done", extra))
}

/// The last candidate is red already; only the earlier ones need probing (11-15).
fn first_red(n: usize, probe: &mut dyn FnMut(usize) -> Option<bool>) -> Result<(usize, Vec<(usize, bool)>), (usize, Vec<(usize, bool)>)> {
    assert!(n > 0, "bisect needs a red landing");
    let (mut lo, mut hi, mut probes) = (0, n - 1, Vec::new());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let Some(red) = probe(mid) else { return Err((mid, probes)) };
        probes.push((mid, red));
        if red { hi = mid; } else { lo = mid + 1; }
    }
    Ok((lo, probes))
}

/// A unit the gate skipped as proven at the probe's inputs passed there: a landing that did not
/// touch it writes no GATEUNIT line for it, only the proof (#5223's probe had no answer, 11-15).
/// A passing run says a test's package passed only by its line or its proof: a probe that ran
/// none of it (it selected nothing) has no answer. One that selected nothing ended OK, and read as
/// green at its red tip while makepad-fab failed there for every run (11-20).
fn probe_says(key: &str, outcome: &Outcome, text: &str) -> Option<bool> {
    if outcome.no_verdict.is_some() { return None; }
    if outcome.failing.iter().any(|u| unit_key(u) == key) { return Some(true); }
    if unit_lines(last_run(text)).1.contains(key) || proven_units(text).contains(key) { return Some(false); }
    (outcome.ok && !key.starts_with("test ")).then_some(false)
}

/// The units a log says were proven at its inputs, by package: gateproof's `GATEPROOF proven <unit>
/// (<source>)` lines, written once above the first run, so they hold for a rerun too.
fn proven_units(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(|l| l.strip_prefix("GATEPROOF proven ")?.split_once(" (")).map(|(unit, _)| unit_key(unit)).collect()
}

/// A cancelled probe has no verdict, even if its partial log names a red or green unit.
fn probe_verdict(status: Result<Option<i32>, String>, text: &str, log: &Path, rev: &str) -> Outcome {
    if matches!(&status, Ok(Some(code)) if *code == crate::lease::client::CANCELLED_EXIT) {
        return Outcome { log: log.display().to_string(), ..no_verdict(format!("probe {rev} cancelled before it completed ({})", log.display())) };
    }
    verdict(status, text, log)
}

/// Files in the red's rustc locations and restart_statics.py's `(path:row)` locations (11-15).
fn bisect_files(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in text.lines() {
        if let Some((path, _)) = location(line) { files.push(path.split_whitespace().last().unwrap_or(path).to_string()); }
        for rest in line.split('(').skip(1) {
            let Some((path, row)) = rest.split_once(')').and_then(|(at, _)| at.rsplit_once(':')) else { continue };
            if !path.is_empty() && !row.is_empty() && row.bytes().all(|b| b.is_ascii_digit()) { files.push(path.to_string()); }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// What a bisect names: the landing it proved, a test's owner when the test passed where it had
/// failed (flaky or not reproduced; None: no owner found), or nobody.
enum Named<'a> {
    Landing(&'a Entry),
    Owner(Option<&'a Entry>),
    Nobody,
}

/// Only a proven landing gets a lane note, and a test's owner when the test did not fail again;
/// doubtful attributions go to main (11-15).
fn bisect_result(home: &Path, job: &Bisect, emit: &dyn Fn(Sev, String, String), named: Named, probes: Vec<Value>, line: String) -> Result<(), String> {
    // (a flake's logs: its red run's, then its probes')
    let flaky = matches!(named, Named::Owner(_)).then(|| std::iter::once(job.log.clone()).chain(probes.iter().filter_map(|p| p.get("log")?.as_str().map(str::to_string))).collect::<Vec<_>>());
    let mut extra = vec![("result", s(&line)), ("probes", Value::Arr(probes))];
    let agent = match named {
        Named::Landing(e) => {
            extra.push(("landing", Value::Int(e.id as i64)));
            Some(e.agent.as_str())
        }
        Named::Owner(owner) => {
            extra.push(("flaky", Value::Bool(true)));
            if let Some(e) = owner { extra.push(("owner", Value::Int(e.id as i64))); }
            owner.map(|e| e.agent.as_str())
        }
        Named::Nobody => None,
    };
    done_bisect(home, job, extra).map_err(|e| format!("bisect ledger: {e}"))?;
    emit(if matches!(named, Named::Landing(_)) { Sev::Err } else { Sev::Warn }, line.clone(), String::new());
    for agent in agent.into_iter().filter(|a| *a != "main").chain(std::iter::once("main")) {
        if let Err(e) = crate::notes::leave(home, agent, &line) { eprintln!("devhub bucket {}: agent notes: {e}", job.bucket); }
    }
    // (its red run's failing tests of the unit: the flake list's by test)
    if let Some(logs) = flaky {
        let tests: Vec<String> = test_lines(last_run(&std::fs::read_to_string(&job.log).unwrap_or_default())).0.into_iter()
            .filter(|(u, _)| unit_key(u) == job.key).flat_map(|(_, t)| t).collect::<BTreeSet<_>>().into_iter().collect();
        flake_said(home, &job.bucket, &job.unit, &tests, &job.tip, &logs, wall_ms(), emit);
    }
    Ok(())
}

/// The flake list, in the hub's home: one row per red that ran again at its red tip and passed
/// there (a bucket's rerun on its box, mac_tests.sh `GATEFLAKE`; a bisect's red-tip probe), by its
/// unit's key and failing tests.
const FLAKES: &str = "flakes.jsonl";

/// A test that flakes twice within this span is a bug, and main hears it with both logs (main, 11-24).
const FLAKE_SPAN_MS: u64 = 7 * 24 * 3600 * 1000;

static FLAKE_LIST: Mutex<()> = Mutex::new(());

/// A run's flakes (`GATEFLAKE <unit> :: <its first run's failing tests>`): red, then green on its
/// rerun on its box at its tip.
fn flake_lines(text: &str) -> Vec<(String, Vec<String>)> {
    text.lines().filter_map(|l| l.strip_prefix("GATEFLAKE ")?.split_once(" ::"))
        .map(|(unit, tests)| (unit.trim().to_string(), tests.split_whitespace().map(str::to_string).collect())).collect()
}

/// Adds a flake to the flake list: `unit` red at `tip` with `tests` failing (none named: the unit
/// as a whole), then green there, its `logs` kept. Main's note when the same test (the same unit
/// key, a test in common, or neither naming one) flaked before within [`FLAKE_SPAN_MS`] and main
/// heard of none of those: a test that flakes twice in seven days is a bug, with both logs, and
/// not a landing's red (main, 11-24). A read of the list per flake: flakes are rare.
fn record_flake(home: &Path, bucket: &str, unit: &str, tests: &[String], tip: &Pin, logs: &[String], at_ms: u64) -> std::io::Result<Option<String>> {
    let _lock = FLAKE_LIST.lock().unwrap_or_else(|e| e.into_inner());
    let path = home.join(FLAKES);
    let key = unit_key(unit);
    let named = |v: &Value| -> Vec<String> { v.get("tests").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect() };
    let earlier: Vec<Value> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("key").and_then(Value::as_str) == Some(key.as_str()) && v.get("ms").and_then(Value::as_u64).is_some_and(|ms| at_ms.saturating_sub(ms) <= FLAKE_SPAN_MS))
        .filter(|v| { let them = named(v); them.is_empty() && tests.is_empty() || them.iter().any(|t| tests.contains(t)) }).collect();
    let tell = !earlier.is_empty() && !earlier.iter().any(|v| v.get("told").and_then(Value::as_bool) == Some(true));
    let row = obj(vec![("ms", Value::Int(at_ms as i64)), ("bucket", s(bucket)), ("unit", s(unit)), ("key", s(&key)), ("tests", Value::Arr(tests.iter().map(s).collect())),
        ("tip", tip_value(tip)), ("at", s(&tip_short(tip))), ("logs", Value::Arr(logs.iter().map(s).collect())), ("told", Value::Bool(tell))]);
    std::fs::create_dir_all(home)?;
    let mut line = row.to_json();
    line.push('\n');
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(line.as_bytes())?;
    if !tell { return Ok(None); }
    let what = if tests.is_empty() { unit.to_string() } else { format!("{unit} {}", tests.join(", ")) };
    let said: Vec<String> = earlier.iter().chain(std::iter::once(&row)).map(|v| {
        let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
        let logs: Vec<String> = v.get("logs").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect();
        format!("bucket {} at {}: {}", field("bucket"), field("at"), logs.join(", "))
    }).collect();
    Ok(Some(format!("flaky {} times in 7 days: {what}: a bug for its owner, not a landing's red; red, then green at the same tip: {}", said.len(), said.join("; "))))
}

/// A test a bucket run measured at or past this is slow ([`slow_notes`]): unit tests run well under
/// a second and scenario tests a few seconds (AGENTS.md); the rest is room for a JIT's warm-up and a
/// box's noise (main, 11-24). The tests and full buckets measure each test (land_gate.sh
/// test_times, GATETESTS).
const SLOW_TEST_S: f64 = 10.0;

/// Main hears the slowest tests of the buckets' runs once a week, under the line too (main, 11-24).
const SLOW_WEEK_MS: u64 = 7 * 24 * 3600 * 1000;

/// How many tests that weekly list names (main, 11-24).
const SLOW_WEEK_TOP: usize = 10;

/// When main last heard the weekly list (wall ms), in the hub's home.
const SLOW_WEEK: &str = "slow-tests-week";

/// Each slow test is said at most once in this span (main, 11-23: once a day).
const SLOW_SAID_SPAN_MS: u64 = 24 * 3600 * 1000;

/// The slow tests said, in the hub's home: one row per test said (`key`, `ms`, `agent`).
const SLOW_SAID: &str = "slow-tests.jsonl";

static SLOW_LIST: Mutex<()> = Mutex::new(());

/// The notes, `(agent, line)`, for the tests of `tests` (each unit's GATETESTS) at or past
/// [`SLOW_TEST_S`] and not said within [`SLOW_SAID_SPAN_MS`], recorded as said: each to the agent
/// of the newest landing of `history` that touched its package, while `live` says its lane is,
/// else main's. Its time and the run's log are said; the bucket's verdict is not changed by it
/// (main, 11-23). A read of the list only on a run with a slow test.
fn slow_notes(home: &Path, bucket: &str, tip: &Pin, log: &str, tests: &BTreeMap<String, Vec<(String, f64, u64)>>, history: &[Entry], live: &dyn Fn(&Entry) -> bool, now: u64) -> std::io::Result<Vec<(String, String)>> {
    let slow: Vec<(&String, &String, f64)> = tests.iter().flat_map(|(unit, ts)| ts.iter().filter(|(_, secs, _)| *secs >= SLOW_TEST_S).map(move |(test, secs, _)| (unit, test, *secs))).collect();
    if slow.is_empty() { return Ok(Vec::new()); }
    let _lock = SLOW_LIST.lock().unwrap_or_else(|e| e.into_inner());
    let path = home.join(SLOW_SAID);
    let said: Vec<(String, u64)> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
        Some((v.get("key")?.as_str()?.to_string(), v.get("ms")?.as_u64()?))
    }).collect();
    let (mut out, mut rows): (Vec<(String, String)>, Vec<String>) = (Vec::new(), Vec::new());
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for (unit, test, secs) in slow {
        let key = format!("{} {test}", unit_key(unit));
        if said.iter().any(|(k, ms)| *k == key && now.saturating_sub(*ms) < SLOW_SAID_SPAN_MS) || !keys.insert(key.clone()) { continue; }
        let pkg = unit.split_whitespace().nth(1).unwrap_or_default();
        let (agent, whose) = match history.iter().filter(|e| e.touched.iter().any(|p| p == pkg)).max_by_key(|e| (e.changed_ms, e.id)) {
            Some(e) if live(e) => (e.agent.clone(), format!("{} last touched {pkg}", who_of(e))),
            Some(e) => ("main".to_string(), format!("{} last touched {pkg}, and its lane has ended", who_of(e))),
            None => ("main".to_string(), format!("no landing devhub keeps touched {pkg}")),
        };
        out.push((agent.clone(), format!("bucket {bucket} {}: slow test {test} of {unit}: {secs:.0} s, past the {SLOW_TEST_S:.0} s line (unit tests run well under a second, scenario tests a few seconds); {whose}; log {log}", tip_short(tip))));
        rows.push(obj(vec![("key", s(&key)), ("ms", Value::Int(now as i64)), ("agent", s(&agent))]).to_json());
    }
    if !rows.is_empty() {
        std::fs::create_dir_all(home)?;
        std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(format!("{}\n", rows.join("\n")).as_bytes())?;
    }
    Ok(out)
}

/// Main's weekly list, once [`SLOW_WEEK_MS`] passed since the last (or none was said): the
/// [`SLOW_WEEK_TOP`] slowest tests the buckets' runs of that span measured, each at its slowest, with
/// its unit and bucket, under the line too. A read of the ledger once a week; one small file's read
/// a run otherwise.
fn weekly_slowest(home: &Path, now: u64) -> std::io::Result<Option<String>> {
    let _lock = SLOW_LIST.lock().unwrap_or_else(|e| e.into_inner());
    let mark = home.join(SLOW_WEEK);
    if std::fs::read_to_string(&mark).ok().and_then(|t| t.trim().parse::<u64>().ok()).is_some_and(|ms| now.saturating_sub(ms) < SLOW_WEEK_MS) { return Ok(None); }
    let since = now.saturating_sub(SLOW_WEEK_MS);
    let mut slowest: BTreeMap<String, (f64, String)> = BTreeMap::new();
    for row in ledger(home, usize::MAX).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        if row.get("started_ms").and_then(Value::as_u64).is_none_or(|ms| ms < since) { continue; }
        let bucket = row.get("bucket").and_then(Value::as_str).unwrap_or("?").to_string();
        let Some(Value::Obj(units)) = row.get("unit_tests") else { continue };
        for (unit, tests) in units {
            for t in tests.as_arr().unwrap_or_default() {
                let t = t.as_arr().unwrap_or_default();
                let (Some(name), Some(secs)) = (t.first().and_then(Value::as_str), t.get(1).and_then(crate::lease::num)) else { continue };
                let at = slowest.entry(format!("{name} of {}", unit_key(unit))).or_insert((secs, bucket.clone()));
                if secs > at.0 { *at = (secs, bucket.clone()); }
            }
        }
    }
    std::fs::create_dir_all(home)?;
    std::fs::write(&mark, now.to_string())?;
    let mut top: Vec<(String, f64, String)> = slowest.into_iter().map(|(test, (secs, bucket))| (test, secs, bucket)).collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(SLOW_WEEK_TOP);
    if top.is_empty() { return Ok(None); }
    let said: Vec<String> = top.iter().map(|(test, secs, bucket)| format!("{test} {secs:.1} s ({bucket})")).collect();
    Ok(Some(format!("the week's {} slowest tests the buckets measured (each at its slowest; the line is {SLOW_TEST_S:.0} s): {}", top.len(), said.join("; "))))
}

/// [`record_flake`], with main's note said and left when it has one.
fn flake_said(home: &Path, bucket: &str, unit: &str, tests: &[String], tip: &Pin, logs: &[String], at_ms: u64, emit: &dyn Fn(Sev, String, String)) {
    match record_flake(home, bucket, unit, tests, tip, logs, at_ms) {
        Ok(Some(note)) => {
            emit(Sev::Warn, format!("bucket {bucket} {}: {note}", tip_short(tip)), String::new());
            if let Err(e) = crate::notes::leave(home, "main", &note) { eprintln!("devhub bucket {bucket}: agent notes: {e}"); }
        }
        Ok(None) => {}
        Err(e) => eprintln!("devhub bucket {bucket}: flake list: {e}"),
    }
}

/// Retire a failed search too; a ledger failure must not hide its reason from the event feed.
fn bisect_job(repo: &Path, home: &Path, store: &Store, emit: &dyn Fn(Sev, String, String), probe: &Probe, job: Bisect) {
    let failed = job.clone();
    if let Err(e) = run_bisect(repo, home, store, emit, probe, &wait_for_night, job) {
        let line = format!("bucket {} {}: bisect unresolved: {e}", failed.bucket, tip_short(&failed.tip));
        if let Err(record) = bisect_result(home, &failed, emit, Named::Nobody, Vec::new(), line.clone()) {
            emit(Sev::Warn, format!("{line}; {record}"), String::new());
        }
    }
}

/// Returns in the master's night: a CI leg's bisect probes boot an app on a box each, so they run
/// from midnight to [`NIGHT_END_HOUR`] on its clock and pause at day until the next (main, 11-18).
fn wait_for_night() {
    loop {
        let secs = (wall_ms() / 1000) as i64;
        let Some(wait) = till_night(secs, makepad_civil_time::local_or_utc(secs).offset as i64) else { return };
        std::thread::sleep(Duration::from_secs(wait.min(600)));
    }
}

/// Seconds from `secs` (at `offset` seconds from UTC) to the next local midnight; None in the night.
fn till_night(secs: i64, offset: i64) -> Option<u64> {
    let day = (secs + offset).rem_euclid(86_400);
    (day / 3600 >= NIGHT_END_HOUR).then(|| (86_400 - day) as u64)
}

/// A bisect's probe log at `at` (`from`, `tip` or a candidate's landing id), named by its bucket,
/// its key and `started`, its red run's start ([`bisect_start`]): the same names in every image of
/// the master. A restart is an exec that keeps the probes' lease holders, and its bisect starts its
/// job again: by its logs' `.run` it adopts the probe in flight and reads the ones that ended
/// ([`lease_run`]). Named by the time each search started, every restart on 11-24 asked for the
/// probes once more beside the ones still held, all in the bisect lane's tree, one lease at a time
/// per box: six of one test's red tip, four of one fast probe, two of them hung for hours, and the
/// bisect queue starved (L1687102). The start stays last, where the lease runner reads it.
fn bisect_log(logs: &Path, job: &Bisect, started: u64, at: &str) -> PathBuf {
    let key: String = job.key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    logs.join(format!("{}-{key}-{at}-{started}.log", job.bucket))
}

/// The start its probes' logs carry ([`bisect_log`]): its red run's, as that run's log names it
/// (`<bucket>-<start>.log`, each a key of its own); one not named so, the search's own.
fn bisect_start(job: &Bisect) -> u64 {
    log_started(Path::new(&job.log)).unwrap_or_else(wall_ms)
}

/// A later run that passed a red ([`passed_since_red`]): its bucket, its tip (short, and whole
/// where its row says it) and its start.
struct PassedSince {
    bucket: String,
    at: String,
    tip: Option<Pin>,
    started: u64,
}

/// The run that passed `job`'s red after its red run started, the newest: its own bucket's next
/// runs too, the hot gate's aside (a hand-in's merge, not a landed tip). Its search stops there
/// ([`run_bisect`]): the bisects of a red #8817 had fixed went on probing the landings before it
/// while every bucket was green (11-24). None for a red log not named by its start.
fn passed_since_red(home: &Path, job: &Bisect) -> Option<PassedSince> {
    let started = log_started(Path::new(&job.log))?;
    let all: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let row = all.iter().filter(|r| r.get("bucket").and_then(Value::as_str).is_some_and(|b| b != "hot"))
        .filter(|r| r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at > started))
        .find(|r| row_lists(r, "passed", &job.key))?;
    let tip = row.get("tip").and_then(read_tip);
    Some(PassedSince { bucket: row.get("bucket")?.as_str()?.to_string(), at: tip.as_ref().map_or_else(|| "?".to_string(), tip_short), tip,
        started: row.get("started_ms")?.as_u64()? })
}

/// What fixed a red that was red twice at its red tip `red` and passed at `passed`, a later run's
/// tip ([`run_bisect`]): the one landing between them, or how many and which; nothing when none
/// is read.
fn fixed_by(repo: &Path, history: &[Entry], red: &Pin, passed: &Pin) -> String {
    // (the landings a search between them would walk: off its first-parent chains, none)
    let between = chain_landings(repo, history, red, passed).map(|(between, _, _)| between).unwrap_or_default();
    match between.as_slice() {
        [] => String::new(),
        [e] => format!(", by #{} {} ({}) lane {}", e.id, e.label(), e.agent, lane_of(e)),
        all => format!("; {} landings between: {}{}", all.len(), all.iter().take(8).map(|e| format!("#{}", e.id)).collect::<Vec<_>>().join(", "), if all.len() > 8 { ", …" } else { "" }),
    }
}

/// The intermittent reds, in the hub's home: a test red again at its red tip whose search a later
/// run's pass stopped ([`run_bisect`]), a row each; a `cleared` row (`devhub bucket clear <key>`)
/// closes its key's before it. A lucky pass had closed such a search as fixed since, and a real
/// intermittent regression went unseen (main, 11-24): `devhub bucket` says each open one
/// ([`open_intermittent`]), with no probe queued behind it.
const INTERMITTENTS: &str = "intermittent.jsonl";

/// The runs of its bucket after its pass, each that ran it green, that close an intermittent red
/// (main, 11-24).
const INTERMITTENT_GREEN: usize = 3;

static INTERMITTENT_LIST: Mutex<()> = Mutex::new(());

/// Adds `job`'s red to the intermittent list: red again at its red tip `tip_log`, passed by
/// `passed` (None: a run the ledger no longer holds, or a red log not named by its start). What
/// its bisect's line says of it.
fn record_intermittent(home: &Path, job: &Bisect, passed: Option<&PassedSince>, tip_log: &Path, at_ms: u64) -> String {
    let by = passed.map_or_else(|| "a later run".to_string(), |p| format!("{} {}", p.bucket, p.at));
    let row = obj(vec![("ms", Value::Int(at_ms as i64)), ("bucket", s(&job.bucket)), ("key", s(&job.key)), ("unit", s(&job.unit)), ("at", s(&tip_short(&job.tip))),
        ("passed", s(&by)), ("since_ms", Value::Int(passed.map_or(at_ms, |p| p.started) as i64)), ("log", s(tip_log.display().to_string())), ("red_log", s(&job.log))]);
    let _lock = INTERMITTENT_LIST.lock().unwrap_or_else(|e| e.into_inner());
    let mut line = row.to_json();
    line.push('\n');
    let kept = std::fs::create_dir_all(home).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(home.join(INTERMITTENTS))?.write_all(line.as_bytes()));
    let said = format!("intermittent: reproduced at {}, passed at {by}", tip_short(&job.tip));
    match kept {
        Ok(()) => format!("{said}; `devhub bucket` says it until it is cleared (`devhub bucket clear {}`) or green in its bucket's next {INTERMITTENT_GREEN} runs of it", job.key),
        Err(e) => format!("{said}; the intermittent list did not take it: {e}"),
    }
}

/// The intermittent reds still open ([`INTERMITTENTS`]), each its newest row: not cleared since, and
/// not green in each of its bucket's last [`INTERMITTENT_GREEN`] runs that ran it after the run
/// that passed it (`rows`: every bucket's, newest first). A run red again since keeps it open; that
/// red is its bucket's STILL FAILING too.
fn open_intermittent(home: &Path, rows: &[Value]) -> Vec<Value> {
    let text = {
        let _lock = INTERMITTENT_LIST.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::read_to_string(home.join(INTERMITTENTS)).unwrap_or_default()
    };
    let mut open: BTreeMap<(String, String), Value> = BTreeMap::new();
    for v in text.lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        if let Some(key) = v.get("cleared").and_then(Value::as_str) {
            open.retain(|(_, k), _| k != key);
            continue;
        }
        let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        if let (Some(bucket), Some(key)) = (field("bucket"), field("key")) { open.insert((bucket, key), v); }
    }
    open.into_iter().filter(|((bucket, key), v)| {
        let since = v.get("since_ms").and_then(Value::as_u64).unwrap_or(0);
        let ran: Vec<bool> = rows.iter().filter(|r| r.get("bucket").and_then(Value::as_str) == Some(bucket.as_str()))
            .filter(|r| r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at > since))
            .filter_map(|r| if row_lists(r, "failing", key) { Some(false) } else if row_lists(r, "passed", key) { Some(true) } else { None })
            .take(INTERMITTENT_GREEN).collect();
        !(ran.len() == INTERMITTENT_GREEN && ran.iter().all(|green| *green))
    }).map(|(_, v)| v).collect()
}

/// `devhub bucket clear <key>`: a human or a lane closes the open intermittent reds of `key`
/// ([`open_intermittent`]). The service's answer: `{"said":..}` or `{"error":..}`.
pub fn clear_intermittent(home: &Path, key: &str) -> String {
    let rows: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let n = open_intermittent(home, &rows).iter().filter(|v| v.get("key").and_then(Value::as_str) == Some(key)).count();
    if n == 0 { return obj(vec![("error", s(&format!("no open intermittent red of `{}`", crate::proto::clean(key, 120))))]).to_json(); }
    let mut line = obj(vec![("cleared", s(key)), ("ms", Value::Int(wall_ms() as i64))]).to_json();
    line.push('\n');
    let _lock = INTERMITTENT_LIST.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::OpenOptions::new().create(true).append(true).open(home.join(INTERMITTENTS)).and_then(|mut f| f.write_all(line.as_bytes())) {
        Ok(()) => obj(vec![("said", s(&format!("cleared {key}: {n} intermittent red{}", if n == 1 { "" } else { "s" })))]).to_json(),
        Err(e) => obj(vec![("error", s(&format!("{INTERMITTENTS}: {e}")))]).to_json(),
    }
}

/// Where a bisect probes: its last pass, its red tip (a test's, first), or a landing of its range.
#[derive(Clone, Copy)]
enum At {
    Base,
    Tip,
    Landing(usize),
}

/// `night`: returns when a CI leg's bisect may probe ([`wait_for_night`]).
fn run_bisect(repo: &Path, home: &Path, store: &Store, emit: &dyn Fn(Sev, String, String), probe: &Probe, night: &dyn Fn(), job: Bisect) -> Result<(), String> {
    let history = store.history("land")?.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
    // its last pass's other nested repositories where they stood then, the ones it did not pin
    // too: every probe is gated against it, its red tip's too
    let (placed, unplaced) = place_from(repo, &job.from, &job.tip);
    let job = Bisect { from: placed, ..job };
    // a leg's red tip first, once: red, and its error lines pin it on no one landing, it is bisected
    let tip_red = match (job.of_a_leg(), job.tip_probe.clone()) {
        (false, _) => None,
        (true, Some(probe)) => Some(probe),
        (true, None) => {
            let Some(probe) = leg_at_tip(repo, home, emit, probe, &history, &job)? else { return Ok(()) };
            let _lock = BISECTS.lock().unwrap();
            append_bisect(home, job.value("tip-red", vec![("probe", probe.clone())])).map_err(|e| format!("bisect ledger: {e}"))?;
            Some(probe)
        }
    };
    let (candidates, mut left_out, nested) = chain_landings(repo, &history, &job.from, &job.tip)?;
    left_out.extend(unplaced);
    let times: Vec<Vec<i64>> = nested.iter().map(|(name, chain)| chain_times(&repo.join(crate::nested::dir(name)), chain)).collect();
    let when: Vec<Option<i64>> = candidates.iter().map(|e| entry_pair(&e.landed).and_then(|p| pair_time(repo, &p))).collect();
    let pins = candidate_pins(&candidates, &job.tip, &nested, &times, &when);
    let heading = format!("bucket {} {}: bisect", job.bucket, tip_short(&job.tip));
    // A test's every failing target of its package is probed (the key's verdict is any of them
    // failing); at its red tip the search narrows to those red there again, and the result says
    // which were green there: one run failed 13 makepad-fab targets, its bisect probed chrome
    // alone, and chrome's pass hid the 12 that #7560 broke (11-20).
    let targets = std::cell::RefCell::new(job.targets());
    let unreproduced = std::cell::RefCell::new(Vec::<String>::new());
    let red_line = std::cell::RefCell::new(job.line.clone());
    let failed_at = std::cell::RefCell::new(Vec::<String>::new());
    // each red probe's way: its failing targets and their failing tests
    let ways = std::cell::RefCell::new(Vec::<Vec<(String, Vec<String>)>>::new());
    let what = || said_targets(&targets.borrow());
    let logs = home.join("bucket-logs/bisect");
    let started = bisect_start(&job);
    // a test red again at its red tip that a later run passed: a lucky pass of a real intermittent
    // red looks the same as a fix, and it stays said until it is cleared or green again (main, 11-24)
    let intermittent = |probes, passed: Option<&PassedSince>, note: Option<String>| {
        let said = record_intermittent(home, &job, passed, &bisect_log(&logs, &job, started, "tip"), wall_ms());
        let note = note.map_or_else(String::new, |n| format!(" ({n})"));
        let mut line = format!("{heading} of {}: {said}{note}; tip log {}", what(), bisect_log(&logs, &job, started, "tip").display());
        for why in &left_out { line.push_str(&format!("; {why}")); }
        bisect_result(home, &job, emit, Named::Nobody, probes, line)
    };
    // ... and before it is said so, its red tip is probed once more (main, 11-24): red again, it
    // is deterministic there, fixed since, between its red tip and the passing run's tip, the
    // landing between named when there is one; green, the same revision went both ways, and it is
    // intermittent. RELAX263's red, red at its tip every time and fixed by a later landing, was
    // listed intermittent. A pass at its red tip itself went both ways already: no probe.
    let settle = |mut probes: Vec<Value>, passed: Option<&PassedSince>| {
        if passed.is_some_and(|p| p.at == tip_short(&job.tip)) {
            return intermittent(probes, passed, None);
        }
        let (first, again) = (bisect_log(&logs, &job, started, "tip"), bisect_log(&logs, &job, started, "tip-again"));
        let outcome = probe(&targets.borrow().join(";"), Some(&job.from), &job.tip, &again);
        let answer = probe_says(&job.key, &outcome, &std::fs::read_to_string(&again).unwrap_or_default());
        let why = outcome.no_verdict.clone().unwrap_or_else(|| outcome.summary.clone());
        let mut fields = vec![("tip", tip_value(&job.tip)), ("red", answer.map_or(Value::Null, Value::Bool)), ("log", s(again.display().to_string())), ("red_tip", Value::Bool(true)), ("again", Value::Bool(true))];
        if answer.is_none() { fields.push(("why", s(&why))); }
        probes.push(obj(fields));
        match answer {
            Some(true) => {
                let tip = tip_short(&job.tip);
                let fixed = match passed {
                    Some(p) => format!("a later {} run at {} passed it: fixed since: between {tip} and {}{}", p.bucket, p.at, p.at, p.tip.as_ref().map_or_else(String::new, |t| fixed_by(repo, &history, &job.tip, t))),
                    None => format!("a later run passed it: fixed since: after {tip}"),
                };
                let mut line = format!("{heading} of {}: red at its red tip {tip} on both probes, and {fixed}; tip logs {}, {}", what(), first.display(), again.display());
                for why in &left_out { line.push_str(&format!("; {why}")); }
                bisect_result(home, &job, emit, Named::Nobody, probes, line)
            }
            Some(false) => intermittent(probes, passed, None),
            None => intermittent(probes, passed, Some(format!("its red tip's second probe gave no answer: {why}"))),
        }
    };
    // the later run that passed its red, once one has ([`passed_since_red`]): its search stops
    // before its next probe, whatever answer it ends on
    let fixed = std::cell::RefCell::new(None::<PassedSince>);
    let finish = |named, probes, line: String| {
        let (named, mut line) = match fixed.borrow().as_ref() {
            // (a test's search goes on only from its red tip red again)
            Some(by) if job.of_a_test() => return settle(probes, Some(by)),
            Some(by) => (Named::Nobody, format!("{heading} of {}: a later {} run passed it: fixed since, nobody told; its search stopped there", what(), by.bucket)),
            None => (named, line),
        };
        let not = unreproduced.borrow();
        if !not.is_empty() { line.push_str(&format!("; green at its red tip, not reproduced: {}", said_targets(&not))); }
        for why in &left_out { line.push_str(&format!("; {why}")); }
        bisect_result(home, &job, emit, named, probes, line)
    };
    if candidates.is_empty() && !job.of_a_test() {
        let still = if tip_red.is_some() { "; red at its red tip, it stays STILL FAILING" } else { "" };
        return finish(Named::Nobody, tip_red.into_iter().collect(), format!("{heading} of {}: no landings on its first-parent chains{still}", job.unit));
    }
    std::fs::create_dir_all(&logs).map_err(|e| format!("bisect logs: {e}"))?;
    let log_at = |at: At| match at {
        At::Base => bisect_log(&logs, &job, started, "from"),
        At::Tip => bisect_log(&logs, &job, started, "tip"),
        At::Landing(i) => bisect_log(&logs, &job, started, &candidates[i].id.to_string()),
    };
    let pin_at = |i: usize| pins[i].clone();
    let (mut why, mut probes, mut said) = (String::new(), Vec::new(), Vec::new());
    if let Some(p) = tip_red {
        probes.push(p);
        said.push(format!("red tip {} red", tip_short(&job.tip)));
    }
    let mut check = |at: At| {
        if job.of_a_leg() { night(); }
        // a red a later run passed needs no landing's probe; a test's red tip still tells a flaky
        // test from one fixed since
        if !matches!(at, At::Tip) {
            if let Some(by) = passed_since_red(home, &job) {
                *fixed.borrow_mut() = Some(by);
                return None;
            }
        }
        let (pin, mark, name) = match at {
            At::Base => (job.from.clone(), ("base", Value::Bool(true)), format!("last pass {}", tip_short(&job.from))),
            At::Tip => (job.tip.clone(), ("red_tip", Value::Bool(true)), format!("red tip {}", tip_short(&job.tip))),
            At::Landing(i) => (pin_at(i), ("id", Value::Int(candidates[i].id as i64)), format!("#{}", candidates[i].id)),
        };
        // against the last pass, not the candidate before it: the gate runs only what its base
        // changed, and a candidate whose own landing missed the unit ran nothing for it (11-15)
        let log = log_at(at);
        let outcome = probe(&targets.borrow().join(";"), Some(&job.from), &pin, &log);
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let answer = probe_says(&job.key, &outcome, &text);
        if answer.is_none() {
            let stopped = if outcome.summary.is_empty() { outcome.failing.join(", ") } else { outcome.summary.clone() };
            why = outcome.no_verdict.clone().unwrap_or_else(|| format!("no verdict for {}{}", what(), if stopped.is_empty() { String::new() } else { format!("; {stopped}") }));
        }
        *failed_at.borrow_mut() = outcome.failing.clone();
        if answer == Some(true) {
            let tests = test_lines(last_run(&text)).0;
            let mut way: Vec<(String, Vec<String>)> = outcome.failing.iter().map(|u| (u.clone(), tests.get(u).cloned().unwrap_or_default())).collect();
            way.sort();
            ways.borrow_mut().push(way);
        }
        let mut fields = vec![("tip", tip_value(&pin)), ("red", answer.map_or(Value::Null, Value::Bool)), ("log", s(log.display().to_string())), mark];
        if answer.is_none() { fields.push(("why", s(&why))); }
        probes.push(obj(fields));
        said.push(format!("{name} {}", match answer { Some(true) => "red", Some(false) => "green", None => "no answer" }));
        answer
    };
    // A test that passes where it failed is flaky, or its failure was the box's: its owner hears
    // it, with what the JIT said of the app as it failed (a hang guard racing a cold JIT's lazy
    // compiles), and no landing of the range does (11-17).
    let flaky = |probes, at: String| {
        let owner = test_owner(repo, &history, &job);
        let who = owner.as_ref().map_or_else(|| "its owner unknown: no landing found for its file".to_string(), |(e, file)| format!("its owner {}, which last changed {file}", who_of(e)));
        let note = jit_note(&std::fs::read_to_string(&job.log).unwrap_or_default(), &job.unit)
            .map_or_else(|| "the red run's JIT said nothing of it".to_string(), |n| format!("the JIT at its failure: {n}"));
        finish(Named::Owner(owner.map(|(e, _)| e)), probes, format!("{heading} of {}: flaky or not reproduced: {at}; {note}; {who}; red: {}; red log {}",
            what(), crate::proto::clean(&job.line, 200), job.log))
    };
    // a test's red tip first: a red it does not reproduce names no landing
    if job.of_a_test() {
        match check(At::Tip) {
            Some(true) => {
                // the targets red there again carry the search (their own red line): a target
                // green there is not reproduced, and no landing of the range is laid to it
                let red: Vec<String> = targets.borrow().iter().filter(|u| failed_at.borrow().contains(u)).cloned().collect();
                if !red.is_empty() && red.len() < targets.borrow().len() {
                    *unreproduced.borrow_mut() = targets.borrow().iter().filter(|u| !red.contains(u)).cloned().collect();
                    if !red.contains(&job.unit) {
                        let lines = test_lines(last_run(&std::fs::read_to_string(&job.log).unwrap_or_default())).0;
                        if let Some(line) = red.iter().find_map(|u| lines.get(u).and_then(|t| t.first())) { *red_line.borrow_mut() = line.clone(); }
                    }
                    *targets.borrow_mut() = red;
                }
            }
            Some(false) => return flaky(probes, format!("green at its red tip {} (log {})", tip_short(&job.tip), log_at(At::Tip).display())),
            None => return finish(Named::Nobody, probes, format!("{heading} of {} stopped at its red tip: {why}; log {}", what(), log_at(At::Tip).display())),
        }
        if job.closed {
            return settle(probes, passed_since_red(home, &job).as_ref());
        }
        if candidates.is_empty() { return finish(Named::Nobody, probes, format!("{heading} of {}: no landings on its first-parent chains", what())); }
    }
    // Leaving a landing out can leave the last candidate short of the known red tip (11-15).
    let last = candidates.len() - 1;
    if pin_at(last) != job.tip {
        match check(At::Landing(last)) {
            Some(true) => {}
            Some(false) => return finish(Named::Nobody, probes, format!("{heading} of {}: attribution doubtful: last candidate #{} is green; log {}", what(), candidates[last].id, log_at(At::Landing(last)).display())),
            None => return finish(Named::Nobody, probes, format!("{heading} of {} stopped at #{}: {why}; log {}", what(), candidates[last].id, log_at(At::Landing(last)).display())),
        }
    }
    let i = match first_red(candidates.len(), &mut |i| check(At::Landing(i))) {
        Ok((i, _)) => i,
        Err((i, _)) => {
            let id = candidates[i].id;
            return finish(Named::Nobody, probes, format!("{heading} of {} stopped at #{id}: no answer: {why}; log {}", what(), log_at(At::Landing(i)).display()));
        }
    };
    // A step's last pass only omitted its failure; prove the base before naming candidate zero.
    // A unit's last pass explicitly listed it passed and remains a green proof (11-15); a test's
    // may have passed by luck, and one red at its own last pass is flaky.
    if i == 0 && (!is_unit(&job.key) || job.of_a_test()) {
        match check(At::Base) {
            Some(false) => {}
            // red the same way at every probe, its last pass too: deterministic, whatever turned
            // it red is outside the range (a box, a tool, a repository it does not walk), not a
            // flaky test's owner's to hear
            Some(true) if job.of_a_test() && ways.borrow().windows(2).all(|w| w[0] == w[1]) => {
                return finish(Named::Nobody, probes, format!("{heading} of {}: deterministic, not flaky: red the same way at every probe, its last pass {} too, where it had passed: no landing of this range is its cause; red: {}; log {}",
                    what(), tip_short(&job.from), crate::proto::clean(&red_line.borrow(), 200), log_at(At::Base).display()));
            }
            Some(true) if job.of_a_test() => return flaky(probes, format!("red at its last pass {}, where it had passed, and not the same way (log {})", tip_short(&job.from), log_at(At::Base).display())),
            answer => {
                let result = if answer == Some(true) { format!("red at its last pass {} already; no landing of this range broke it", tip_short(&job.from)) }
                    else { format!("attribution doubtful: its last pass {} did not answer: {why}", tip_short(&job.from)) };
                return finish(Named::Nobody, probes, format!("{heading} of {}: {result}; log {}", what(), log_at(At::Base).display()));
            }
        }
    }
    let e = &candidates[i];
    let log = if probes.iter().any(|p| p.get("id").and_then(Value::as_u64) == Some(e.id)) { log_at(At::Landing(i)).display().to_string() } else { job.log.clone() };
    if SOURCE_STEPS.contains(&job.key.as_str()) {
        let files = bisect_files(&job.line);
        let touched = landing_files(repo, e);
        if !touched.as_ref().is_some_and(|t| t.iter().any(|p| files.iter().any(|f| touches(f, p)))) {
            let reason = if files.is_empty() { "changed none of the reported files (the red line names no file)".to_string() }
                else if touched.is_none() { format!("could not read its diff for {}", files.join(", ")) }
                else { format!("changed none of {}", files.join(", ")) };
            return finish(Named::Nobody, probes, format!("{heading} of {}: attribution doubtful: #{} ({}) {reason}; log {log}", what(), e.id, e.label()));
        }
    }
    let said = said.join(", ");
    finish(Named::Landing(e), probes, format!("{heading}: #{} {} ({}) lane {} turned {} red: {}; first red of {} landings since it last passed (probes: {said}); log {log}",
        e.id, e.label(), e.agent, lane_of(e), what(), crate::proto::clean(&red_line.borrow(), 300), candidates.len()))
}

/// A bisect's targets as its lines say them: one unit as it is; several of one package as the
/// package and each one's target (`test makepad-fab --test chrome, --test monitor`).
fn said_targets(units: &[String]) -> String {
    let Some(first) = units.first() else { return String::new() };
    let key = unit_key(first);
    let rest: Vec<&str> = units[1..].iter().map(|u| u.strip_prefix(&key).map_or(u.as_str(), str::trim_start)).collect();
    if rest.is_empty() { first.clone() } else { format!("{first}, {}", rest.join(", ")) }
}

/// A failing test's owner: the landing that last changed the file holding the test (its name as
/// the red's line has it, in its target's files at the red tip), with that file.
fn test_owner<'a>(repo: &Path, history: &'a [Entry], job: &Bisect) -> Option<(&'a Entry, String)> {
    let tip = job.tip.pair();
    let tree = crate::orch::ci::Tree::at(repo, &tip).ok()?;
    let file = test_file(&tree, &job.unit, &job.line)?;
    Some((owner_of(repo, history, &tip, &file)?, file))
}

/// The note of a line that holds makepad_test's `JIT state at the failure: ..` (to the end of the
/// quoted message or of the line).
fn jit_note_of(line: &str) -> Option<String> {
    const MARK: &str = "JIT state at the failure: ";
    let said = &line[line.find(MARK)? + MARK.len()..];
    let end = [said.find("\\n"), said.find('"'), said.find(" -- see ")].into_iter().flatten().min().unwrap_or(said.len());
    Some(crate::proto::clean(said[..end].trim(), 300)).filter(|n| !n.is_empty())
}

/// A CI leg's red tip, probed once ([`Bisect::of_a_leg`]). Green: its owner hears it is flaky or
/// not reproduced, with what the JIT said as it failed. Red again: the landing its error lines'
/// files name ([`Bisect::named`]); with none, or several, its probe (`Some`), and [`run_bisect`]
/// bisects its range at night: from its last recorded pass, else the CI run before (main, 11-18:
/// legs run under the box's JIT now, not as native app builds; its red tip named nobody before).
fn leg_at_tip(repo: &Path, home: &Path, emit: &dyn Fn(Sev, String, String), probe: &Probe, history: &[Entry], job: &Bisect) -> Result<Option<Value>, String> {
    let logs = home.join("bucket-logs/bisect");
    std::fs::create_dir_all(&logs).map_err(|e| format!("bisect logs: {e}"))?;
    let log = bisect_log(&logs, job, bisect_start(job), "tip");
    let outcome = probe(&job.unit, Some(&job.from), &job.tip, &log);
    let answer = probe_says(&job.key, &outcome, &std::fs::read_to_string(&log).unwrap_or_default());
    let why = outcome.no_verdict.clone().unwrap_or_else(|| outcome.summary.clone());
    let mut fields = vec![("tip", tip_value(&job.tip)), ("red", answer.map_or(Value::Null, Value::Bool)), ("log", s(log.display().to_string())), ("red_tip", Value::Bool(true))];
    if answer.is_none() { fields.push(("why", s(&why))); }
    let heading = format!("bucket {} {}: tip probe of {}", job.bucket, tip_short(&job.tip), job.unit);
    let red = format!("red: {}; red log {}", crate::proto::clean(&job.line, 200), job.log);
    let (named, line) = match answer {
        Some(false) => {
            let (failed, owner) = (leg_line(&job.log, &job.unit), leg_owner(repo, history, job));
            let note = failed.and_then(|(_, shard_log)| std::fs::read_to_string(shard_log).ok()).and_then(|text| text.lines().rev().find_map(jit_note_of))
                .map_or_else(|| "the red run's JIT said nothing of it".to_string(), |n| format!("the JIT at its failure: {n}"));
            let who = owner.as_ref().map_or_else(|| "its owner unknown: no landing found for its file".to_string(), |(e, file)| format!("its owner {}, which last changed {file}", who_of(e)));
            (Named::Owner(owner.map(|(e, _)| e)), format!("{heading}: flaky or not reproduced: green at its red tip (log {}); {note}; {who}; {red}", log.display()))
        }
        Some(true) if job.closed => (Named::Nobody, format!("{heading}: red again at its tip, and a later run passed it: fixed since, nobody told; log {}", log.display())),
        // devhub ended it on two boxes (orch/ci.rs `ended_by_devhub`): a hang, which only its owner
        // can read from its stacks, never a landing's by its error lines or a night's bisect
        Some(true) if leg_line(&job.log, &job.unit).is_some_and(|(summary, _)| summary.starts_with(crate::orch::ci::ENDED_BY_DEVHUB)) => {
            let owner = leg_owner(repo, history, job);
            let who = owner.as_ref().map_or_else(|| "its owner unknown: no landing found for its file".to_string(), |(e, file)| format!("its owner {}, which last changed {file}", who_of(e)));
            (Named::Owner(owner.map(|(e, _)| e)), format!("{heading}: red again at its red tip, where devhub had ended it on two boxes: a hang for its owner to read from its stacks; {who}; {red}; tip log {}", log.display()))
        }
        Some(true) => match history.iter().filter(|e| job.named.contains(&e.id)).collect::<Vec<_>>()[..] {
            [e] => (Named::Landing(e), format!("{heading}: #{} {} ({}) lane {} turned it red: its error lines name a file it changed; {red}; tip log {}", e.id, e.label(), e.agent, e.branch, log.display())),
            _ => return Ok(Some(obj(fields))),
        },
        None => (Named::Nobody, format!("{heading}: no answer at its red tip: {why}; log {}", log.display())),
    };
    bisect_result(home, job, emit, named, vec![obj(fields)], line).map(|_| None)
}

/// A failed leg's line in its CI bucket log `log` (orch/ci.rs bucket_log: `fail <leg>: <summary>
/// (<its own log>)`): its summary and its own log.
fn leg_line(log: &str, leg: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(log).ok()?;
    let rest = text.lines().find_map(|l| l.strip_prefix(&format!("fail {leg}: ")))?.strip_suffix(')')?;
    let (summary, own) = rest.rsplit_once(" (")?;
    Some((summary.to_string(), own.to_string()))
}

/// A flaky CI leg's owner: the landing that last changed its file ([`leg_file`]) at its red tip.
fn leg_owner<'a>(repo: &Path, history: &'a [Entry], job: &Bisect) -> Option<(&'a Entry, String)> {
    let tip = job.tip.pair();
    let tree = crate::orch::ci::Tree::at(repo, &tip).ok()?;
    let summary = leg_line(&job.log, &job.unit).map(|(summary, _)| summary).unwrap_or_default();
    let file = leg_file(&tree, &job.unit, &summary)?;
    Some((owner_of(repo, history, &tip, &file)?, file))
}

/// The file a CI leg's red is its owner's by: a failing test's (its step's summary names its tests:
/// `test <pkg> [args]: exit N: <name>, ..`, orch/ci.rs judge), a smoke's script or else its app's
/// folder, else the first file its steps run (a live replay's script).
fn leg_file(tree: &crate::orch::ci::Tree, leg: &str, summary: &str) -> Option<String> {
    let failed = summary.split_once(": exit ").and_then(|(step, rest)| Some((step, rest.split_once(": ")?.1)));
    if let Some((step, names)) = failed.filter(|(step, _)| step.starts_with("test ")) {
        if let Some(file) = test_file(tree, step, names.split(", ").next().unwrap_or("")) { return Some(file); }
    }
    if let Some((_, dir)) = leg.strip_prefix("smoke:").and_then(|rest| rest.split_once(':')) {
        let script = format!("{dir}/ci.splash");
        return Some(if tree.has(&script) { script } else { dir.to_string() });
    }
    let shard = crate::orch::ci::leg_shard(tree, leg, &|_| None, 0)?;
    shard.steps.iter().flatten().find(|w| w.contains('/') && tree.has(w)).cloned()
}

/// The file of `tree` that holds test `test` (`module::tests::name`, or `name`) of `unit` (`test
/// <pkg> [--test T]`): of its target's files the one that defines `fn <name>`, else an integration
/// target's own file.
fn test_file(tree: &crate::orch::ci::Tree, unit: &str, test: &str) -> Option<String> {
    let words: Vec<&str> = unit.split_whitespace().collect();
    let target = words.windows(2).find(|w| w[0] == "--test").map(|w| w[1]);
    let files = crate::orch::ci::target_files(tree, words.get(1).copied()?, target)?;
    let name = test.split_whitespace().next().unwrap_or("").rsplit("::").next().unwrap_or("");
    let defines = |f: &&String| !name.is_empty() && tree.read(f).is_some_and(|t| [format!("fn {name}("), format!("fn {name}<")].iter().any(|d| t.contains(d.as_str())));
    files.iter().find(defines).or_else(|| target.and(files.first())).cloned()
}

/// The landing of `history` that brought the last change of `file` at `at` (the nested commercial
/// checkout's under `apps/commercial/`): the one whose landed side has that commit and whose pre
/// side has not, found by its landing order. None: no landing the history keeps brought it.
fn owner_of<'a>(repo: &Path, history: &'a [Entry], at: &Pair, file: &str) -> Option<&'a Entry> {
    let commercial = file.strip_prefix("apps/commercial/");
    let (dir, rel) = match commercial { Some(rel) => (repo.join("apps/commercial"), rel), None => (repo.to_path_buf(), file) };
    let side = |p: Pair| if commercial.is_some() { p.main } else { p.work };
    let commit = crate::gitread::last_commit_of(&dir, &side(at.clone()), rel)?;
    let has = |pair: &str| entry_pair(pair).is_some_and(|p| crate::gitread::is_ancestor(&dir, &commit, &side(p)));
    let mut ordered: Vec<&Entry> = history.iter().collect();
    ordered.sort_by_key(|e| (e.changed_ms, e.id));
    // the first landing whose landed side has it; record times order landings but loosely (11-15),
    // so the one that brought it is looked for around there (none found: main hears the flake)
    let first = ordered.partition_point(|e| !has(&e.landed));
    let near = first.saturating_sub(8)..(first + 8).min(ordered.len());
    ordered[near].iter().copied().find(|e| has(&e.landed) && !has(&e.pre))
}

/// What the JIT said of the app as test unit `unit` failed, in the unit's own lines of a run's log:
/// makepad_test's `JIT state at the failure: ..`, in a panic's quoted message or a line of its own
/// (the newest look's). A hang guard racing a cold JIT reads `main is not stuck: it waited on its
/// lazy compiles 99% of the 200 ms probe`.
fn jit_note(text: &str, unit: &str) -> Option<String> {
    let (mut own, mut note) = (Vec::new(), None);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("GATEUNIT ") {
            if rest.strip_prefix("fail ") == Some(unit) {
                note = own.iter().copied().find_map(jit_note_of).or(note);
            }
            own.clear();
        } else if line.starts_with("STEP ") {
            own.clear();
        } else {
            own.push(line);
        }
    }
    note
}

/// Who broke a failing unit whose errors sit in `files`: of `since`, the landings since the unit
/// last passed, each one whose change touched one of those files (`touched`, each landing's
/// files; None: its diff could not be read). Each is named with the commit of it that changed the
/// file (`commit`, from the file's history at the landing): `rc-x (X) #12 (0123456789 changed vm.rs)`.
/// None of them touched a file: no one, and the landings that changed the unit's inputs answer for
/// it (a change elsewhere, a callee's, broke a caller's file). A run's own landings were named
/// before. A JIT break in vm.rs from #2593's 572669339f was named for whoever landed next with a
/// change to the apps' inputs: two other lanes (11-07).
fn file_blame(files: &[String], since: &[Entry], touched: &[Option<&[String]>], commit: &dyn Fn(&Entry, &str) -> Option<String>) -> Vec<(u64, String)> {
    since.iter().zip(touched).filter_map(|(e, t)| {
        let hit = t.as_ref()?.iter().find(|p| files.iter().any(|f| touches(f, p)))?;
        let name = hit.rsplit('/').next().unwrap_or(hit);
        Some((e.id, match commit(e, hit).as_deref().and_then(|c| c.split_whitespace().next()) {
            Some(c) => format!("{} ({c} changed {name})", who_of(e)),
            None => format!("{} (it changed {name})", who_of(e)),
        }))
    }).collect()
}

/// `<unit> still failing since <who>`, for a `who` from [`origin`] or a row.
pub fn still_since(who: &str) -> String {
    if who.starts_with(ORIGIN_UNKNOWN) || who.starts_with(INTERMITTENT) || who.starts_with(BY_TEST) || who.starts_with(FIRST_RUN) || who.starts_with(COMPILE_RED) { who.to_string() } else { format!("since {who}") }
}

/// A `, `-joined list cut to its first `n` items and how many more: a red laid to the dozens of
/// landings since its unit last passed filled `devhub bucket`'s line (11-17).
pub fn few(list: &str, n: usize) -> String {
    let items: Vec<&str> = list.split(", ").collect();
    if items.len() <= n + 1 { return list.to_string(); }
    format!("{} and {} more", items[..n].join(", "), items.len() - n)
}

/// The fast bucket's statics step (land_gate.sh join_statics: restart_statics.py).
const STATICS_STEP: &str = "statics";

/// Source failures need a matching file in the named landing's own diff (11-15).
const SOURCE_STEPS: [&str; 3] = [STATICS_STEP, "private", "rustfmt"];

/// The buckets whose next run looks again at what failed (land_gate.sh's GATE_RETRY). CI needs no
/// telling: a shard with no proof at its inputs runs; prebuild lists its legs under each new JIT;
/// the full bucket runs everything.
const RERUNS: [&str; 6] = ["fast", "hot", "tests", "mac", WINDOWS, WINDOWS_TESTS];

/// The full bucket's name ([`full`]).
pub const FULL: &str = "full";

/// The CI bucket's name (orch/ci.rs `regression`): its reds are its legs ([`is_leg`]).
const CI: &str = "ci";

/// The buckets `devhub bucket run` starts ([`kick`]).
const KICKED: [&str; 6] = ["fast", "tests", "hot", "mac", CI, FULL];

/// The file that starts a bucket's run at its next look ([`spawn`]).
fn kick_file(home: &Path, name: &str) -> PathBuf {
    home.join("bucket-logs").join(format!("{name}.now"))
}

/// `devhub bucket run <bucket>`: its run starts at once, of the tip then, whether or not a landing
/// came since its last. The service's answer: `{"said":..}` or `{"error":..}`, which says what
/// the bucket is at now, and what the run waits for when it cannot start at once.
pub fn kick(home: &Path, name: &str) -> String {
    kick_on(&BOARD, home, name, wall_ms())
}

fn kick_on(board: &Board, home: &Path, name: &str, now: u64) -> String {
    let file = kick_file(home, name);
    let done = if !KICKED.contains(&name) { Err(format!("no bucket `{}`: {}", crate::proto::clean(name, 64), KICKED.join(", "))) }
        else { std::fs::create_dir_all(home.join("bucket-logs")).and_then(|_| std::fs::write(&file, "")).map_err(|e| format!("{}: {e}", file.display())) };
    if let Err(e) = done {
        return obj(vec![("error", s(&e))]).to_json();
    }
    let at = board.doing.lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned();
    let holding = board.slots().holding();
    let then = match at {
        Some((state, since, what)) => format!("; it is {state} since {} ({}): {what}; the asked run starts as that one ends", clock(since), took(now.saturating_sub(since) / 1000)),
        None if SHARDED.contains(&name) && !holding.is_empty() => format!("; the pool slots: {}: its shards wait for them, and `devhub bucket` says so", holding.join("; ")),
        None => String::new(),
    };
    obj(vec![("said", s(&format!("bucket {name}: its run starts at its next look{then} (`devhub bucket` says its state and verdict)")))]).to_json()
}

/// The buckets whose runs take the pool boxes in turn ([`BOARD`]).
const SHARDED: [&str; 2] = ["tests", FULL];

/// Bucket `name`'s rows, newest first.
fn rows_of(home: &Path, name: &str) -> Vec<Value> {
    ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("bucket").and_then(Value::as_str) == Some(name)).collect()
}

/// The tests bucket's last look at `unit` (`test <pkg>[ --test T]`: a package's tests under the
/// JIT, land_gate.sh testrun): whether it failed, when that run started, its wall in s. None: no
/// run of the bucket's last ones looked at it.
pub fn tests_last(home: &Path, unit: &str) -> Option<(bool, u64, Option<u64>)> {
    rows_of(home, "tests").into_iter().find_map(|row| {
        let has = |field: &str| row.get(field).and_then(Value::as_arr).is_some_and(|a| a.iter().any(|u| u.as_str() == Some(unit)));
        let failed = has("failing");
        (failed || has("passed") || has("ran_ok")).then(|| (failed, row.get("started_ms").and_then(Value::as_u64).unwrap_or(0), row.get("unit_s").and_then(|t| t.get(unit)).and_then(Value::as_u64)))
    })
}

/// The units whose last look ended in a failure, by package, with who broke them and the log of
/// that look. A run that never looked at a unit says nothing about it: one whose diff touched
/// nothing said ok and a real failure looked fixed (11-04). `rows`: the bucket's rows, newest first.
fn still_failing(rows: &[Value]) -> BTreeMap<String, (String, String, String)> {
    let (mut settled, mut out) = (BTreeSet::new(), BTreeMap::new());
    for (i, row) in rows.iter().enumerate() {
        for unit in row.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).filter(|u| is_unit(u) || is_leg(u)) {
            let key = unit_key(unit);
            if settled.contains(&key) { continue; }
            let who = origin(&rows[i..], &key);
            let log = row.get("log").and_then(Value::as_str).unwrap_or("").to_string();
            out.insert(key, (unit.to_string(), who, log));
        }
        settled.extend(out.keys().cloned());
        settled.extend(row.get("passed").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string));
    }
    out
}

/// Whether row `r`'s list `list` (`failing`, `passed`) holds a unit of `key` ([`unit_key`]).
fn row_lists(r: &Value, list: &str, key: &str) -> bool {
    r.get(list).and_then(Value::as_arr).is_some_and(|l| l.iter().filter_map(Value::as_str).any(|u| unit_key(u) == key))
}

/// The bucket of a run that started after `started` and passed `key`, other than bucket `name`'s
/// own and the hot gate's (a hand-in's merge, not a landed tip): the unit passes at a newer tip than
/// the run that saw it fail. `all`: every bucket's rows, newest first.
fn passed_after(all: &[Value], name: &str, key: &str, started: u64) -> Option<String> {
    passed_after_row(all, name, key, started).and_then(|r| r.get("bucket")).and_then(Value::as_str).map(str::to_string)
}

/// The row of that run ([`passed_after`]).
fn passed_after_row<'a>(all: &'a [Value], name: &str, key: &str, started: u64) -> Option<&'a Value> {
    all.iter().filter(|r| r.get("bucket").and_then(Value::as_str).is_some_and(|b| b != name && b != "hot"))
        .filter(|r| r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at > started))
        .find(|r| row_lists(r, "passed", key))
}

/// Red `unit`'s package as landings of `landed` touched it after the red's run started at
/// `failed`: the first of them and how many (`package`, `first`, `first_ms`, `n`); None when none
/// did. That run predates the change, and the next look at the unit is the first to judge it.
fn changed_since(landed: &[Entry], unit: &str, failed: u64) -> Option<Value> {
    let package = unit.split_whitespace().nth(1)?;
    let since: Vec<&Entry> = landed.iter().filter(|e| e.changed_ms > failed && e.touched.iter().any(|p| p == package)).collect();
    let first = since.iter().min_by_key(|e| e.changed_ms)?;
    Some(obj(vec![("package", s(package)), ("first", s(&format!("#{} {}", first.id, first.branch))),
        ("first_ms", Value::Int(first.changed_ms as i64)), ("n", Value::Int(since.len() as i64))]))
}

/// The ledger's newest rows as they are written, newest first: what [`reds_passed_since`] reads.
pub fn recent_rows(home: &Path) -> Vec<String> {
    ledger(home, 2_000)
}

/// Whether each of `keys`, red in a run that started at `after`, has passed since: of the runs that
/// started later (any bucket's but the hot gate's, which runs a hand-in's merge), the newest that
/// looked at it passed it. A note of that red is not said any more (notes.rs `take_open`): lanes
/// heard of a red an hour after a later landing had made it pass (11-17). `rows`: [`recent_rows`].
pub(crate) fn reds_passed_since(rows: &[String], after: u64, keys: &[String]) -> bool {
    let mut open: Vec<&String> = keys.iter().collect();
    for line in rows {
        let Ok(r) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        // (rows are written as their runs end: one that ended before the red's start started before it)
        if r.get("ended_ms").and_then(Value::as_u64).is_some_and(|at| at < after) {
            break;
        }
        if r.get("bucket").and_then(Value::as_str) == Some("hot") || !r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at > after) {
            continue;
        }
        if open.iter().any(|key| row_lists(&r, "failing", key)) {
            return false;
        }
        open.retain(|key| !row_lists(&r, "passed", key));
        if open.is_empty() {
            return true;
        }
    }
    false
}

/// Bucket `name`'s reds ([`still_failing`]) less those a later run of another bucket passed since
/// the red's last failing run ([`passed_after`]). The full bucket's red of a package stayed open,
/// and was laid to landings, hours after its fix, while the tests bucket passed it run after run
/// (11-17). `all`: every bucket's rows, newest first.
fn open_reds(all: &[Value], name: &str) -> BTreeMap<String, (String, String, String)> {
    let started = |r: &Value| r.get("started_ms").and_then(Value::as_u64).unwrap_or(0);
    let rows: Vec<Value> = all.iter().filter(|r| r.get("bucket").and_then(Value::as_str) == Some(name)).cloned().collect();
    let mut red = still_failing(&rows);
    red.retain(|key, _| {
        let last = rows.iter().find(|r| row_lists(r, "failing", key)).map_or(0, started);
        passed_after(all, name, key, last).is_none()
    });
    red
}

/// A bucket's reds ([`open_reds`]); CI's less the legs that passed since ([`ci_reds`]), where the
/// repository is at hand.
fn reds(repo: Option<&Path>, all: &[Value], name: &str) -> BTreeMap<String, (String, String, String)> {
    match repo.filter(|_| name == CI) {
        Some(repo) => ci_reds(repo, all),
        None => open_reds(all, name),
    }
}

/// CI's reds ([`open_reds`]) less the legs that passed since at a later tip. A CI run takes an app
/// on one OS a night, so a red leg's app can pass on its other OSes for days before its turn on
/// that one comes, and a lane's own `ci run` that proves a leg fixed runs outside the bucket: three
/// smoke reds of 11-15 to 11-17 stayed STILL FAILING after each had passed again, in runs whose rows
/// listed no passes (11-18). Each unit of the leg needs a proof landed on work (the landed tip's
/// ci.toml files hold the newest) from a run whose tip holds the red's and is not it
/// ([`crate::orch::ci::leg_proofs`], [`proven_since`]).
fn ci_reds(repo: &Path, all: &[Value]) -> BTreeMap<String, (String, String, String)> {
    let mut red = open_reds(all, CI);
    let legs: Vec<&str> = red.keys().map(String::as_str).filter(|k| is_leg(k)).collect();
    let at = (!legs.is_empty()).then(|| crate::orch::snapshot(repo)).flatten();
    let Some(tree) = at.and_then(|at| crate::orch::ci::Tree::at(repo, &at).ok()) else { return red };
    let proofs = crate::orch::ci::leg_proofs(&tree, &legs);
    let commercial = repo.join("apps/commercial");
    let after = |red: &Pair, proof: &Pair| proof != red && crate::gitread::is_ancestor(repo, &red.work, &proof.work) && crate::gitread::is_ancestor(&commercial, &red.main, &proof.main);
    red.retain(|key, _| !proven_since(all, key, proofs.get(key), &after));
    red
}

/// Whether CI leg `key` passed since its newest red in `all` (rows newest first): each of its units
/// (`units`: each unit's proofs) has a proof from a run at a tip `after` the red's.
fn proven_since(all: &[Value], key: &str, units: Option<&Vec<Vec<crate::orch::ci::Proof>>>, after: &dyn Fn(&Pair, &Pair) -> bool) -> bool {
    let Some(units) = units.filter(|u| !u.is_empty()) else { return false };
    let red = all.iter().filter(|r| r.get("bucket").and_then(Value::as_str) == Some(CI)).find(|r| row_lists(r, "failing", key));
    let Some(red) = red.and_then(|r| r.get("tip")).and_then(read_tip).map(|t| t.pair()) else { return false };
    units.iter().all(|proofs| proofs.iter().any(|p| after(&red, &p.pair)))
}

/// Where a failing unit's breakers can start: the tip of the newest run of any bucket that passed
/// it and started before the failing run (`started`), the hot gate's aside (a hand-in's merge). Only the landings after it can have broken
/// it: the full bucket's range since its last run named 187 landings for one red the tests bucket
/// had passed within it (11-17). `all`: every bucket's rows, newest first.
fn passed_before(all: &[Value], key: &str, started: u64) -> Option<Pin> {
    all.iter().filter(|r| r.get("bucket").and_then(Value::as_str) != Some("hot") && r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at < started))
        .find(|r| row_lists(r, "passed", key)).and_then(|r| r.get("tip")).and_then(read_tip)
}

/// [`passed_before`] of a test unit itself (`test <pkg> --test T`), as a run's `ran_ok` names it,
/// else of its package: a pass of another target of the package is no pass of this one.
fn unit_passed_before(all: &[Value], unit: &str, started: u64) -> Option<Pin> {
    let ran = |r: &&Value| r.get("ran_ok").and_then(Value::as_arr).is_some_and(|l| l.iter().any(|u| u.as_str() == Some(unit)));
    all.iter().filter(|r| r.get("bucket").and_then(Value::as_str) != Some("hot") && r.get("started_ms").and_then(Value::as_u64).is_some_and(|at| at < started))
        .find(ran).and_then(|r| r.get("tip")).and_then(read_tip)
        .or_else(|| passed_before(all, &unit_key(unit), started))
}

/// What the next run of a bucket looks at whatever its diff reaches, as land_gate.sh's GATE_RETRY:
/// the units still failing and, after a step that failed with no unit to name (the JIT that did not
/// build), `steps`: the gate takes the JIT even if every unit is proven.
fn retry_env(rows: &[Value]) -> String {
    let mut units: Vec<String> = still_failing(rows).into_values().map(|(unit, _, _)| unit).collect();
    let step = rows.first().and_then(|r| r.get("failing")).and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).any(|u| !is_unit(u));
    if step { units.push("steps".to_string()); }
    format!("GATE_RETRY={}", units.join(";"))
}

/// The landings the store's live view keeps (its newest couple of hundred): what [`status`] reads
/// for the packages changed since a red, never the whole land history.
pub fn landed(store: &Store) -> Vec<Entry> {
    store.read(|d| d.land.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect())
}

/// `devhub bucket`'s lines: what each bucket is at now (`{"now":..}`: running or queued since when,
/// what it waits for), what each still owes a pass ([`open_reds`]: a later run of another bucket
/// that passed one closed it; `changed`: those whose package a landing of `landed`
/// touched after the red's run started, the first such landing and how many), when each bucket's
/// last run ended and whether it passed (`last_runs`), then the last runs, newest first. The full
/// bucket's red of a unit the tests bucket passed hours later read as a current break, and a
/// bucket's red a day old as today's (11-16); a red the full bucket's 08:51 run found still read
/// as one at 17:00, though its fix landed at 09:06 (11-17).
pub fn status(home: &Path, landed: &[Entry]) -> Vec<String> {
    let mut out: Vec<String> = BOARD.now_lines(home, wall_ms()).iter().map(|l| obj(vec![("now", s(l))]).to_json()).collect();
    // each running shard's unit far past its measure, with its box: an oversubscribed box beside it
    if let Some(ctx) = crate::orch::ctx() {
        out.extend(crate::orch::lane::slow_now(ctx, wall_ms()).iter().map(|l| obj(vec![("now", s(l))]).to_json()));
    }
    // a run that never got its verdict and still holds its shards' JITs, a day on
    out.extend(stale_holds(home, wall_ms()).iter().map(|l| obj(vec![("now", s(l))]).to_json()));
    let rows: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let bucket = |r: &Value| r.get("bucket").and_then(Value::as_str).unwrap_or("").to_string();
    let started = |r: &Value| r.get("started_ms").and_then(Value::as_u64).unwrap_or(0);
    for name in RERUNS.into_iter().chain([FULL, CI]) {
        // (a red a later run of another bucket passed is closed: [`open_reds`]; a leg a later
        // proof passed: [`ci_reds`])
        let still = reds(crate::orch::ctx().map(|c| c.repo.as_path()), &rows, name);
        if still.is_empty() { continue; }
        // a red whose package changed after its run started: that run predates the change, and
        // the bucket's next run is the first to look again
        let changed: Vec<(String, Value)> = still.values().filter_map(|(unit, _, log)| {
            Some((unit.clone(), changed_since(landed, unit, log_started(Path::new(log))?)?))
        }).collect();
        // and where each failed last: a green kept log beside a red bucket read as a miscount (11-05)
        let mut line = vec![("still_failing", s(name)), ("units", Value::Obj(still.values().map(|(unit, who, _)| (unit.clone(), s(who))).collect())),
            ("logs", Value::Obj(still.into_values().map(|(unit, _, log)| (unit, s(&log))).collect()))];
        if !changed.is_empty() { line.push(("changed", Value::Obj(changed))); }
        out.push(obj(line).to_json());
    }
    // a test red again at its red tip that a later run passed, until cleared or green again
    out.extend(open_intermittent(home, &rows).into_iter().map(|v| {
        let field = |k: &str| s(v.get(k).and_then(Value::as_str).unwrap_or(""));
        obj(vec![("intermittent", field("bucket")), ("key", field("key")), ("at", field("at")), ("passed", field("passed")), ("log", field("log")),
            ("ms", v.get("ms").cloned().unwrap_or(Value::Int(0)))]).to_json()
    }));
    // each bucket's last run: when it ended, and whether it passed; and a run after it that had no
    // verdict, which writes no row ([`note_no_verdict`])
    let last: Vec<(String, Value)> = KICKED.into_iter().filter_map(|name| {
        let r = rows.iter().find(|r| bucket(r) == name);
        let none = no_verdict_since(home, name, r.map_or(0, started)).map(|(ended, why)| obj(vec![("ended_ms", Value::Int(ended as i64)), ("why", s(&why))]));
        let mut fields = Vec::new();
        if let Some(r) = r {
            let ended = r.get("ended_ms").and_then(Value::as_u64).unwrap_or_else(|| started(r));
            fields.extend([("ended_ms", Value::Int(ended as i64)), ("ok", Value::Bool(r.get("ok").and_then(Value::as_bool) == Some(true)))]);
            // a failed run: what became of each of its reds since, as a STILL FAILING line says it
            // of an open one. The full bucket runs once a day while the others pass its reds: its
            // mundane-suite red, fixed at 19:39 and passed by the tests bucket at 20:23, read as
            // "full 4h ago FAILED" all evening (11-21).
            let failed = r.get("log").and_then(Value::as_str).and_then(|l| log_started(Path::new(l))).unwrap_or_else(|| started(r));
            let reds: Vec<(String, Value)> = r.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).filter(|u| is_unit(u)).filter_map(|unit| {
                let key = unit_key(unit);
                let mut marks = Vec::new();
                if let Some(pass) = passed_after_row(&rows, name, &key, started(r)) {
                    let at = pass.get("ended_ms").and_then(Value::as_u64).unwrap_or_else(|| started(pass));
                    marks.extend([("passed", s(&bucket(pass))), ("passed_ms", Value::Int(at as i64))]);
                }
                if let Some(changed) = changed_since(landed, unit, failed) {
                    marks.push(("changed", changed));
                }
                (!marks.is_empty()).then(|| (key, obj(marks)))
            }).collect();
            if !reds.is_empty() { fields.push(("reds", Value::Obj(reds))); }
        }
        if let Some(none) = none { fields.push(("no_verdict", none)); }
        (!fields.is_empty()).then(|| (name.to_string(), obj(fields)))
    }).collect();
    if !last.is_empty() { out.push(obj(vec![("last_runs", Value::Obj(last))]).to_json()); }
    out.extend(ledger(home, 10));
    out
}

/// What the buckets know of the checkout's tip, for the shows that take only proven tips
/// (orch/show.rs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TipState {
    /// the fast bucket's newest run: the tip it looked at and its log
    pub fast: Option<(Pair, String)>,
    /// a step that run stopped at (the JIT that did not build): its units did not run, so it says
    /// nothing of them
    pub stopped: Option<String>,
    /// the units red after that run, by package (`check <pkg>`), with who broke each
    pub red: BTreeMap<String, String>,
    /// the fast bucket's run in flight: when it started and the tip it looks at (the next run
    /// takes the newest tip as it ends)
    pub running: Option<(u64, Pair)>,
    /// the units the hot bucket's newest run of the tip failed
    pub hot_failing: Vec<String>,
}

/// The buckets' word on `tip` (one read of the ledger).
pub fn tip_state(home: &Path, tip: &Pair) -> TipState {
    let rows: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let of = |name: &str| rows.iter().filter(|v| v.get("bucket").and_then(Value::as_str) == Some(name)).cloned().collect::<Vec<_>>();
    let (fast, hot) = (of("fast"), of("hot"));
    let newest = fast.first();
    let failing = |row: &Value| row.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    let started = newest.and_then(|r| r.get("started_ms")?.as_u64()).unwrap_or(0);
    let prefix = "fast-";
    let running = std::fs::read_dir(home.join("bucket-logs")).ok().into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        let ms: u64 = entry.file_name().to_str()?.strip_prefix(prefix)?.strip_suffix(".log.run")?.parse().ok()?;
        let (_, at, _, run_tip) = read_run(&entry.path())?;
        (ms > started && at == ms).then_some((ms, run_tip.pair()))
    }).max_by_key(|(ms, _)| *ms);
    TipState {
        fast: newest.and_then(|r| Some((read_tip(r.get("tip")?)?.pair(), r.get("log").and_then(Value::as_str).unwrap_or("").to_string()))),
        stopped: newest.and_then(|r| failing(r).into_iter().find(|u| !is_unit(u))),
        red: still_failing(&fast).into_iter().map(|(key, (_, who, _))| (key, who)).collect(),
        running,
        hot_failing: hot.iter().find(|r| r.get("tip").and_then(read_tip).map(|t| t.pair()).as_ref() == Some(tip)).map(failing).unwrap_or_default(),
    }
}

/// Where a box that takes only proven tips goes ([`proven_tip`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Proven {
    /// it moves to this tip (every repository the fast bucket's run of it looked at)
    Move(Pin),
    /// it stays where it is: why
    Stay(String),
}

/// Where a box that takes only proven tips moves (the user's WM node, sync/autosync.rs), by the
/// fast bucket's runs: to the newest tip a run looked at that did not stop at a step and has no
/// check, cargo, present or boundary unit red after it that was not red after the run of the newest
/// tip the box holds (`holds(tip)`: its deployment has it; never back). A boundary red is an app
/// that would stop at a JIT boundary when it runs there (`check --boundary all`, qboundary_one). A box that holds none of the tips
/// (it took every tip unproven until then) starts at the newest run that did not stop, red and all.
pub fn proven_tip(home: &Path, holds: &dyn Fn(&Pin) -> bool) -> Proven {
    let fast = rows_of(home, "fast");
    let tip = |r: &Value| r.get("tip").and_then(read_tip);
    let stopped = |r: &Value| r.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).find(|u| !is_unit(u)).map(str::to_string);
    let log = |r: &Value| r.get("log").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(base) = fast.iter().position(|r| tip(r).is_some_and(|t| holds(&t))) else {
        return match fast.iter().filter(|r| stopped(r).is_none()).find_map(tip) {
            Some(t) => Proven::Move(t),
            None => Proven::Stay("no run of the fast bucket looked at a tip yet".to_string()),
        };
    };
    let took = still_failing(&fast[base..]);
    let mut why = None;
    for (i, row) in fast[..base].iter().enumerate() {
        let Some(t) = tip(row) else { continue };
        if let Some(step) = stopped(row) {
            why.get_or_insert_with(|| format!("tip {} is not proven: the fast bucket's run of it stopped at its {step} step (log {})", tip_short(&t), log(row)));
            continue;
        }
        let red = still_failing(&fast[i..]);
        let new = red.iter().find(|(key, _)| matches!(key.split_whitespace().next(), Some("check" | "cargo" | "present" | "boundary")) && !took.contains_key(*key));
        match new {
            Some((_, (unit, who, log))) => {
                why.get_or_insert_with(|| format!("tip {} has {unit} failing {} (log {log})", tip_short(&t), still_since(who)));
            }
            None => return Proven::Move(t),
        }
    }
    Proven::Stay(why.unwrap_or_else(|| "waits for the fast bucket to look at a newer tip".to_string()))
}

/// No verdict finishes the attempt without a ledger row; other errors leave it adoptable.
#[derive(Debug)]
enum JudgeError {
    NoVerdict(String),
    Other(String),
}

impl From<String> for JudgeError {
    fn from(error: String) -> Self { Self::Other(error) }
}

impl std::fmt::Display for JudgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::NoVerdict(why) | Self::Other(why) => f.write_str(why) }
    }
}

/// The step a run's gate stopped at before it reached any unit: a `GATESTEP fail <step>` line, and
/// no unit started or said (`GATESTART`, `GATEUNIT`). A JIT its box did not have, or a diff's reach
/// it could not read: it said nothing of the landings' units, so its bucket's next run reads their
/// reach from this run's base ([`next_base`]). The mac gate stopped at its JIT on two Macs, and
/// the next run diffed from the stopped run's tip, found 0 units and said ok: the menu-bar item's
/// restart guard had not run in the bucket once (11-17).
fn stopped_short(run: &str) -> Option<String> {
    if run.lines().any(|l| l.starts_with("GATESTART ") || l.starts_with("GATEUNIT ")) { return None; }
    run.lines().find_map(|l| l.strip_prefix("GATESTEP fail ")).map(|step| step.trim().to_string()).filter(|step| !step.is_empty())
}

/// What a bucket's next run diffs from after its ledger row `row`: the row's tip, or the row's own
/// base when its gate stopped before any unit ([`stopped_short`]), so the next run covers the units
/// that run never reached. Read so at a master's start too.
fn next_base(row: &Value) -> Option<Pin> {
    let stopped = row.get("stopped").and_then(Value::as_str).is_some_and(|step| !step.is_empty());
    stopped.then(|| row.get("base").and_then(read_tip)).flatten().or_else(|| read_tip(row.get("tip")?))
}

/// Both a newly run and an adopted attempt retire their sidecar when no verdict is possible.
fn finish_run(log: &Path, result: Result<(), JudgeError>) -> Result<(), JudgeError> {
    if matches!(&result, Err(JudgeError::NoVerdict(_))) {
        let sidecar = log.with_extension("log.run");
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(JudgeError::Other(format!("removing bucket run {}: {e}", sidecar.display()))),
        }
    }
    result
}

/// Where bucket `name`'s newest run with no verdict is noted ([`note_no_verdict`]).
fn no_verdict_file(home: &Path, name: &str) -> PathBuf {
    home.join("bucket-logs").join(format!("{name}.noverdict"))
}

/// Notes bucket `name`'s run that started at `started` and ended now with no verdict (`why`, its
/// log): it writes no ledger row, so its bucket's last run read as the one before. The CI run of
/// 11-20 02:00 sat 7 h on a hung Windows shard and ended with none, and `devhub bucket` said
/// "ci 27h ago": read as CI not running for a day ([`status`]'s `last_runs`).
fn note_no_verdict(home: &Path, name: &str, started: u64, why: &str, log: &str) {
    let line = obj(vec![("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(wall_ms() as i64)), ("why", s(why)), ("log", s(log))]).to_json();
    let file = no_verdict_file(home, name);
    if let Err(e) = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&file, line)) { eprintln!("devhub bucket {name}: its no-verdict note: {e}"); }
}

/// Bucket `name`'s newest run with no verdict ([`note_no_verdict`]) when it started after
/// `after_ms` (its newest ledger row's start): when it ended and why.
fn no_verdict_since(home: &Path, name: &str, after_ms: u64) -> Option<(u64, String)> {
    let v = makepad_strict_json::parse(std::fs::read_to_string(no_verdict_file(home, name)).ok()?.as_bytes()).ok()?;
    let at = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    (at("started_ms") > after_ms).then(|| (at("ended_ms"), v.get("why").and_then(Value::as_str).unwrap_or("").to_string()))
}

/// Bucket `name`'s verdict on `tip`, from the outcome `run` reaches from the base (the last tip it
/// judged, else the oldest base of the last hour's landings): the landings since the base named
/// for the units they changed, a ledger row and a feed line (`emit`). Err: no verdict.
#[allow(clippy::too_many_arguments)]
fn judge(name: &str, home: &Path, repo: &Path, store: &Store, emit: &dyn Fn(Sev, String, String), started: u64, base: Option<Pin>, tip: &Pin,
    bisect: Option<&dyn Fn(Bisect)>, run: impl FnOnce(Option<&Pin>) -> Result<Outcome, String>) -> Result<(), JudgeError> {
    let history = store.history("land")?.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed).collect::<Vec<_>>();
    let entries = || history.clone();
    let base = base.or_else(|| {
        let recent = Duration::from_secs(60 * 60).as_millis();
        let mut entries = entries();
        entries.sort_by_key(|e| (e.changed_ms, e.id));
        entries.iter().filter(|e| u128::from(started.saturating_sub(e.changed_ms)) <= recent).find_map(|e| entry_pair(&e.pre)).map(|p| pin_of(&p))
    });
    let mut outcome = run(base.as_ref())?;
    // a unit or step that failed on a file its box's tree lacked, which the pinned tree holds, is
    // the box's: no lane's. A warm leg's tree without makepad/apps failed makepad-game-blocks on
    // its missing manifest, and the bucket laid it to four landings of the lane that changed the
    // package last (11-22)
    if !outcome.ok && outcome.no_verdict.is_none() {
        let infra = lacked_in_tree(repo, tip, last_run(&std::fs::read_to_string(&outcome.log).unwrap_or_default()), &outcome.failing);
        if !infra.is_empty() {
            let said = format!("infra, no lane's: {}", infra.iter().map(|(u, path)| format!("{u}: its box's tree lacked {path}, which the pinned tree holds")).collect::<Vec<_>>().join("; "));
            outcome.failing.retain(|u| !infra.contains_key(u));
            if outcome.failing.is_empty() {
                outcome.no_verdict = Some(said);
            } else {
                outcome.summary.push_str(&format!("{}{said}", if outcome.summary.is_empty() { "" } else { "; " }));
            }
        }
    }
    // the landings' ledger and attribution read work and main
    let base_pin = base.clone();
    let (base, tip_pin, tip) = (base.map(|b| b.pair()), tip, &tip.pair());
    if let Some(why) = &outcome.no_verdict {
        note_no_verdict(home, name, started, why, &outcome.log);
        emit(Sev::Warn, format!("bucket {name} {}: no verdict: {why} ({}); it runs again at its next turn", tip_short(tip_pin), outcome.log), String::new());
        return Err(JudgeError::NoVerdict(format!("no verdict on {}: {why}", tip_short(tip_pin))));
    }
    let ended = wall_ms();
    let (entries, attribution_error) = match &base {
        Some(base) => match landings(repo, base, tip, &entries()) {
            Ok(entries) => (entries, None),
            Err(e) => (Vec::new(), Some(format!("landing attribution: {e}"))),
        },
        None => (Vec::new(), None),
    };
    if let Some(e) = &attribution_error {
        outcome.summary.push_str(&format!("; {e}"));
    }
    let text = std::fs::read_to_string(&outcome.log).unwrap_or_default();
    let stopped = stopped_short(last_run(&text));
    if let Some(step) = &stopped {
        outcome.summary.push_str(&format!("{}its gate stopped at its {step} step before any unit: its next run reads the landings' reach from this run's base", if outcome.summary.is_empty() { "" } else { "; " }));
    }
    let (errors, passed) = unit_lines(last_run(&text));
    let (tests_failed, ran_ok) = test_lines(last_run(&text));
    let panics = panic_lines(last_run(&text));
    let kept = kept_outputs(last_run(&text));
    let proven = proven_lines(last_run(&text));
    let repro = repro_lines(last_run(&text));
    let secs = unit_secs(last_run(&text));
    let mb = unit_mb(last_run(&text));
    let threads = unit_jobs(last_run(&text));
    let (tests_s, peak_tests) = (unit_tests(last_run(&text)), unit_peak_tests(last_run(&text)));
    let all: Vec<Value> = ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let earlier: Vec<Value> = all.iter().filter(|r| r.get("bucket").and_then(Value::as_str) == Some(name)).cloned().collect();
    // a failing unit that names its failing tests is blamed by test ([`test_blame`]), the rest by unit
    let unnamed: Vec<String> = outcome.failing.iter().filter(|u| !tests_failed.contains_key(*u)).cloned().collect();
    let carried = carried_since(&earlier, &unnamed, &errors);
    // what this run did not get to (a gate that stopped at the JIT): red still, whatever else it says
    let still: BTreeMap<String, (String, String)> = if RERUNS.contains(&name) {
        open_reds(&all, name).into_iter().filter(|(key, _)| !passed.contains(key) && !outcome.failing.iter().any(|u| unit_key(u) == *key)).map(|(_, (unit, who, log))| (unit, (who, log))).collect()
    } else { BTreeMap::new() };
    outcome.ok &= still.is_empty();
    let blame: Vec<String> = unnamed.iter().filter(|u| !carried.contains_key(*u)).cloned().collect();
    // a unit whose errors sit in a file a landing changed since the unit last passed: that landing,
    // in this run's range or before it ([`file_blame`]); the rest, the landings that changed its inputs.
    // A failing step (no unit: the JIT's build) by the files its own lines name ([`step_files`])
    let mut located = error_files(last_run(&text));
    let own = step_files(last_run(&text));
    for step in blame.iter().filter(|u| !is_unit(u) && !is_leg(u) && !own.is_empty()) { located.entry(step.clone()).or_insert_with(|| own.clone()); }
    let (mut since, mut diffs): (HashMap<Option<Pair>, Vec<Entry>>, HashMap<u64, Option<Vec<String>>>) = (HashMap::new(), HashMap::new());
    let mut by_file: BTreeMap<String, Vec<(u64, String)>> = BTreeMap::new();
    let mut statics_errors: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    // the statics step names each static's line: the landing that added it, alone (the fast
    // bucket named all four lanes of its range "(ambiguous)" for two statics one landing added, 11-14)
    if blame.iter().any(|u| u.as_str() == STATICS_STEP) {
        let from = last_pass(&earlier, STATICS_STEP).map(|p| p.pair()).or_else(|| base.clone());
        let landed = since.entry(from.clone()).or_insert_with(|| from.as_ref().and_then(|from| landings(repo, from, tip, &history).ok()).unwrap_or_default());
        let mut who: Vec<(u64, String)> = Vec::new();
        for (path, n, line) in statics_lines(last_run(&text)) {
            let Some(e) = line_blame(landed, tip, &path, n, &|at, p| file_text(repo, at, p)) else { continue };
            statics_errors.entry(e.id).or_default().push(line);
            let name = path.rsplit('/').next().unwrap_or(&path);
            if !who.iter().any(|(id, _)| *id == e.id) { who.push((e.id, format!("{} (it added {name}:{n})", who_of(e)))); }
        }
        if !who.is_empty() { by_file.insert(STATICS_STEP.to_string(), who); }
    }
    let by_files: Vec<String> = blame.iter().filter(|u| !by_file.contains_key(*u)).cloned().collect();
    for unit in &by_files {
        let Some(files) = located.get(unit) else { continue };
        let from = last_pass(&earlier, &unit_key(unit)).map(|p| p.pair()).or_else(|| base.clone());
        let landed = since.entry(from.clone()).or_insert_with(|| from.as_ref().and_then(|from| landings(repo, from, tip, &history).ok()).unwrap_or_default());
        for e in landed.iter() { diffs.entry(e.id).or_insert_with(|| landing_files(repo, e)); }
        let touched: Vec<Option<&[String]>> = landed.iter().map(|e| diffs[&e.id].as_deref()).collect();
        let who = file_blame(files, landed, &touched, &|e, p| entry_pair(&e.landed).and_then(|at| last_touch(repo, &at, p)));
        if !who.is_empty() { by_file.insert(unit.clone(), who); }
    }
    let blame: Vec<String> = blame.into_iter().filter(|u| !by_file.contains_key(u)).collect();
    let by_file: BTreeMap<String, (Vec<u64>, String)> = by_file.into_iter()
        .map(|(u, who)| (u, (who.iter().map(|(id, _)| *id).collect(), who.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(", ")))).collect();
    // (a CI leg no file names is laid to no landing: every landing of the night would be)
    let blame: Vec<String> = blame.into_iter().filter(|u| !is_leg(u)).collect();
    // a unit red only in test listings this bucket never had a result for: red at their first run
    // here, the owner's of their target, not the range's (a Mac's first listing of makepad-hal's
    // quit_later, months old, was laid to the landing under it, #8152, 11-22)
    let heads = listing_heads(last_run(&text));
    let first: BTreeMap<String, String> = blame.iter().filter_map(|unit| {
        let red: Vec<&str> = heads.get(unit)?.iter().filter(|(_, ok)| !ok).map(|(label, _)| label.as_str()).collect();
        let seen = (!red.is_empty()).then(|| listed_before(&earlier, unit)).flatten()?;
        red.iter().all(|label| !seen.contains(*label)).then(|| (unit.clone(), first_run_owner(repo, &history, tip, &red)))
    }).collect();
    let blame: Vec<String> = blame.into_iter().filter(|u| !first.contains_key(u)).collect();
    let (units, mut ambiguous) = if blame.is_empty() { (vec![Vec::new(); entries.len()], false) }
        else { attribute(repo, &entries, &blame) };
    ambiguous |= attribution_error.is_some();
    // the landings that changed each unit with named tests, kept for the runs after to name should
    // a test first fail here ([`test_blame`]); and who broke each of those tests
    let named: Vec<String> = outcome.failing.iter().filter(|u| tests_failed.contains_key(*u)).cloned().collect();
    // (of the landings after the newest pass of the unit by any bucket within this run's range:
    // [`passed_before`]; units with the same start share one look)
    let mut ranges: BTreeMap<Option<String>, (Option<Vec<Entry>>, Vec<String>)> = BTreeMap::new();
    for unit in &named {
        let from = passed_before(&all, &unit_key(unit), started).map(|p| p.pair());
        let slot = ranges.entry(from.as_ref().map(|p| format!("{}:{}", p.work, p.main))).or_insert_with(|| {
            (from.as_ref().and_then(|from| landings(repo, from, tip, &history).ok()).filter(|narrow| narrow.len() < entries.len()), Vec::new())
        });
        slot.1.push(unit.clone());
    }
    let changed: BTreeMap<String, String> = ranges.into_values().flat_map(|(narrow, units)| {
        let range = narrow.unwrap_or_else(|| entries.clone());
        changed_by(repo, &range, &units).into_iter().map(|(u, by)| (u, by.iter().map(|&i| who_of(&range[i])).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>()
    }).filter(|(_, who)| !who.is_empty()).collect();
    let tests_value = Value::Obj(tests_failed.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(s).collect()))).collect());
    let ran_value = Value::Arr(ran_ok.iter().map(s).collect());
    let changed_value = Value::Obj(changed.iter().map(|(u, who)| (u.clone(), s(who))).collect());
    // (its panics too: a test's first red after a pass is this run's, and without them its earlier
    // red's other break read as the same failure off and on, 11-25)
    let panics_value = Value::Obj(panics.iter().map(|(u, p)| (u.clone(), Value::Arr(p.iter().map(s).collect()))).collect());
    let looked_back: Vec<Value> = std::iter::once(obj(vec![("log", s(&outcome.log)), ("tests_failed", tests_value.clone()), ("panics", panics_value.clone()), ("ran_ok", ran_value.clone()), ("changed", changed_value.clone())]))
        .chain(earlier.iter().take(TIMES_ROWS).map(named_row)).collect();
    let mut by_key: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for unit in &named { by_key.entry(unit_key(unit)).or_default().push((unit.clone(), tests_failed[unit].clone())); }
    // a failing test whose message names a line of the tree: the landing since the test last
    // passed that added that line ([`said_lines`], [`line_blame`]), ahead of who changed its unit (a
    // missing Option entry the boundary check named at its line, under the harness's assert, read
    // "intermittent, origin unknown", and one of the three landings that changed the unit added it, 11-25)
    let mut by_line: BTreeMap<(String, String), String> = BTreeMap::new();
    for ((unit, test), lines) in said_lines(last_run(&text)) {
        let from = looked_back[1..].iter().find(|r| matches!(looked(r, &unit, &test), Some(Look::Passed))).and_then(|r| r.get("tip")).and_then(read_tip).map(|p| p.pair()).or_else(|| base.clone());
        let landed = since.entry(from.clone()).or_insert_with(|| from.as_ref().and_then(|from| landings(repo, from, tip, &history).ok()).unwrap_or_default());
        let mut who: Vec<(u64, String)> = Vec::new();
        for (path, n) in &lines {
            let Some(e) = line_blame(landed, tip, path, *n, &|at, p| file_text(repo, at, p)) else { continue };
            let name = path.rsplit('/').next().unwrap_or(path);
            if !who.iter().any(|(id, _)| *id == e.id) { who.push((e.id, format!("{} (it added {name}:{n})", who_of(e)))); }
        }
        if !who.is_empty() { by_line.insert((unit, test), who.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(", ")); }
    }
    let by_test: BTreeMap<String, String> = by_key.iter().map(|(key, named)| (key.clone(), tests_blame(&looked_back, named, &|unit, test| by_line.get(&(unit.to_string(), test.to_string())).cloned()))).collect();
    // who broke each failing unit: for the next run of the bucket to name, should it still fail
    let mut broke: BTreeMap<String, String> = by_test.clone();
    // (a bucket's first run has no green baseline: what its range changed is no more why than what
    // came before it, and a red no landing's file names is nobody's: `origin unknown, first failed
    // in <log>`. The windows bucket's first red, a commercial crate's Unix-only calls, was laid to
    // the eight landings of its first hour, 11-19)
    let first_run = earlier.is_empty();
    for unit in unnamed.iter().filter(|u| is_unit(u) || is_leg(u)) {
        let who = carried.get(unit).or_else(|| by_file.get(unit).map(|(_, who)| who)).or_else(|| first.get(unit)).cloned().unwrap_or_else(|| if first_run { String::new() } else {
            entries.iter().zip(&units).filter(|(_, us)| us.contains(unit)).map(|(e, _)| who_of(e)).collect::<Vec<_>>().join(", ")
        });
        if !who.is_empty() { broke.entry(unit_key(unit)).or_insert(who); }
    }
    // a step its own lines name the landing of (the statics step by each static's line): that
    // landing's agent is told at once, as for a unit
    if let Some((_, who)) = by_file.get(STATICS_STEP) { broke.entry(STATICS_STEP.to_string()).or_insert_with(|| who.clone()); }
    // the command that shows each failing unit's own lines alone
    let repro: BTreeMap<String, String> = repro.into_iter().filter(|(u, _)| outcome.failing.iter().any(|f| unit_key(f) == *u)).collect();
    // the agents of the landings a new red names: told at their next devhub call ([`crate::notes`])
    // (the statics step stays red until its statics are fixed: a landing it names is told at the
    // first red run that names it, not at each run after)
    let mut tell = broke.clone();
    if let Some(who) = tell.get_mut(STATICS_STEP) {
        let last = earlier.first().filter(|r| r.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().any(|u| u.as_str() == Some(STATICS_STEP)))
            .and_then(|r| r.get("broke")).and_then(|b| b.get(STATICS_STEP)).and_then(Value::as_str).unwrap_or("");
        *who = statics_new(who, last);
    }
    // a red a run of another bucket that started after this one has passed: it passes at a newer
    // tip, and no landing is told of it (the full bucket's three-hour run told 187 landings of a
    // red the tests bucket had passed in the meantime, 11-17)
    let passed_since: BTreeMap<String, String> = outcome.failing.iter().map(|u| unit_key(u))
        .filter_map(|key| passed_after(&all, name, &key, started).map(|by| (key, by))).collect();
    tell.retain(|key, who| !who.is_empty() && !passed_since.contains_key(key));
    let before = reds(Some(repo), &all, name);
    // each new red's one bisect, where the bucket has a probe: a failing test's always (its red tip
    // first: a test can fail once and pass again, and one a later run passed is probed there
    // alone), a check's when its attribution names nobody or several landings. A red a bisect
    // takes tells no landing now: one red is one bisect and one charged lane (a key-shadow red
    // that passed at the next tip was laid to 14 landings of 8 lanes, each of which spent time
    // disproving it, 11-17). The statics step's landings each added a line of their own, and are
    // told as they are.
    let mut jobs: Vec<Bisect> = Vec::new();
    if bisect.is_some() && !outcome.ok {
        for unit in &outcome.failing {
            let key = unit_key(unit);
            let (test, leg, closed) = (key.starts_with("test "), is_leg(unit), passed_since.contains_key(&key));
            if before.contains_key(&key) || jobs.iter().any(|j| j.key == key) || first.contains_key(unit) || !(is_unit(unit) || leg || unit == STATICS_STEP) { continue; }
            if !test && !leg && (closed || broke.get(&key).is_some_and(|who| named_ids(who).len() == 1)) { continue; }
            let pass = if test { unit_passed_before(&all, unit, started).or_else(|| last_pass(&earlier, &key)) } else { last_pass(&earlier, &key) };
            // (a leg's from: its last pass, else the run before; its tip alone, with neither; its
            // other nested repositories where that pass ran them; the bisect places the ones it
            // named none of where they stood at its time, never at the red tip's ([`place_from`]))
            let Some(from) = pass.or_else(|| base_pin.clone()).or_else(|| leg.then(|| tip_pin.clone())) else { continue };
            if !test && !leg {
                let landed = since.entry(Some(from.pair())).or_insert_with(|| landings(repo, &from.pair(), tip, &history).unwrap_or_default());
                if landed.len() < 2 { continue; }
            }
            let lines: [&BTreeMap<String, Vec<String>>; 2] = if test { [&tests_failed, &errors] } else { [&errors, &tests_failed] };
            let line = if unit == STATICS_STEP { statics_lines(last_run(&text)).into_iter().next().map(|(_, _, line)| line) } else { None }
                .or_else(|| lines.into_iter().find_map(|lines| lines.iter().filter(|(u, _)| unit_key(u) == key).find_map(|(_, lines)| lines.first()).cloned()))
                .unwrap_or_else(|| key.clone());
            let named = if leg { broke.get(&key).map(|who| named_ids(who).into_iter().collect()).unwrap_or_default() } else { Vec::new() };
            let units = if test { outcome.failing.iter().filter(|u| unit_key(u) == key).cloned().collect() } else { Vec::new() };
            jobs.push(Bisect { bucket: name.to_string(), unit: unit.clone(), key, from, tip: tip_pin.clone(), line, log: outcome.log.clone(), closed, named, tip_probe: None, units });
        }
    }
    let bisected = |key: &str| key != STATICS_STEP && jobs.iter().any(|j| j.key == key);
    tell.retain(|key, _| !bisected(key));
    let told = if outcome.ok { BTreeMap::new() } else { told_agents(&tell, &before, &history) };
    // One event per newly named landing, with its own failing lines; main hears it too (11-14).
    let mut broken: Vec<(String, String, Vec<String>)> = Vec::new();
    let red_of = |key: &str, id: u64| -> String {
        let failing = statics_errors.get(&id).filter(|_| key == STATICS_STEP).and_then(|lines| lines.first())
            .or_else(|| [&errors, &panics, &tests_failed].into_iter().find_map(|lines| lines.iter().filter(|(u, _)| unit_key(u) == key).find_map(|(_, lines)| lines.first())))
            .map(String::as_str).unwrap_or(key);
        format!("{}{}", crate::proto::clean(failing, 300), kept.get(key).map_or(String::new(), |at| format!(" (its output: {at})")))
    };
    // a red at its first run here, as its owner and main hear it
    let first_keys: BTreeMap<String, &String> = first.iter().map(|(u, who)| (unit_key(u), who)).collect();
    let first_line = |key: &str, who: &str| format!("bucket {name} {}: {key} {who}: {}; log {}", tip_short(tip_pin), red_of(key, 0), outcome.log);
    for (agent, ids) in &told {
        for id in ids {
            let Some(e) = history.iter().find(|e| e.id == *id) else { continue };
            let named = who_of(e);
            let keys: Vec<String> = tell.iter().filter(|(key, who)| !before.contains_key(*key) && who.match_indices(&named)
                .any(|(at, _)| !who[at + named.len()..].starts_with(|c: char| c.is_ascii_digit()))).map(|(key, _)| key.clone()).collect();
            let (firsts, keys): (Vec<String>, Vec<String>) = keys.into_iter().partition(|key| first_keys.contains_key(key));
            for key in firsts { broken.push((agent.clone(), first_line(&key, first_keys[&key]), vec![key])); }
            if keys.is_empty() { continue; }
            let reds: Vec<String> = keys.iter().map(|key| format!("{key} red: {}", red_of(key, *id))).collect();
            broken.push((agent.clone(), format!("bucket {name} {}: #{} {} ({}) lane {} turned {}; log {}", tip_short(tip_pin), e.id, e.label(), e.agent, e.branch, reds.join("; "), outcome.log), keys));
        }
    }
    // (one no landing owns: main's)
    for (key, who) in &first_keys {
        if !outcome.ok && tell.contains_key(key) && !before.contains_key(key) && !broken.iter().any(|(_, _, keys)| keys.contains(key)) {
            broken.push(("main".to_string(), first_line(key, who), vec![key.clone()]));
        }
    }
    let rows = entries.iter().zip(&units).map(|(e, units)| obj(vec![
        ("id", Value::Int(e.id as i64)), ("label", s(&e.label())), ("agent", s(&e.agent)),
        ("land_to_verdict_ms", Value::Int(ended.saturating_sub(e.changed_ms) as i64)),
        ("units", Value::Arr(units.iter().chain(by_file.iter().filter(|(_, (ids, _))| ids.contains(&e.id)).map(|(u, _)| u)).map(|u| s(u)).collect())),
    ])).collect();
    append(home, obj(vec![
        ("bucket", s(name)), ("base", base_pin.as_ref().map_or(Value::Null, tip_value)), ("tip", tip_value(tip_pin)),
        ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(ended as i64)),
        ("ok", Value::Bool(outcome.ok)), ("failing", Value::Arr(outcome.failing.iter().map(|u| s(u)).collect())),
        // the step its gate stopped at before any unit ([`stopped_short`]): its next run's base is this one's
        ("stopped", stopped.as_deref().map_or(Value::Null, |step| s(step))),
        ("summary", s(&outcome.summary)), ("log", s(&outcome.log)), ("landings", Value::Arr(rows)),
        ("ambiguous", Value::Bool(ambiguous)),
        ("errors", Value::Obj(errors.iter().map(|(u, e)| (u.clone(), Value::Arr(e.iter().map(s).collect()))).collect())),
        ("passed", Value::Arr(passed.iter().map(s).collect())),
        // each unit's test listings that got a result, which tell a later red whether it is a first run ([`listed_before`])
        ("targets", Value::Obj(heads.iter().map(|(u, ls)| (u.clone(), Value::Arr(ls.iter().map(|(l, _)| l.as_str()).collect::<BTreeSet<_>>().into_iter().map(s).collect()))).collect())),
        ("carried", Value::Obj(carried.iter().map(|(u, who)| (u.clone(), s(who))).collect())),
        // each failing unit's failing tests, the units that passed and who changed each named unit:
        // what a test's blame reads back ([`test_blame`])
        ("tests_failed", tests_value),
        ("panics", panics_value),
        ("ran_ok", ran_value),
        ("changed", changed_value),
        ("still", Value::Obj(still.iter().map(|(u, (who, _))| (u.clone(), s(who))).collect())),
        ("broke", Value::Obj(broke.iter().map(|(k, who)| (k.clone(), s(who))).collect())),
        // where each failed unit's output is kept, which a red that stays open names ([`origin`])
        ("kept", Value::Obj(kept.iter().map(|(k, at)| (k.clone(), s(at))).collect())),
        ("proven", Value::Arr(proven.iter().map(s).collect())),
        ("repro", Value::Obj(repro.iter().map(|(u, cmd)| (u.clone(), s(cmd))).collect())),
        // each unit's seconds, which the next runs deal the slowest out by first (tests)
        ("unit_s", Value::Obj(secs.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect())),
        ("unit_mb", Value::Obj(mb.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect())),
        // the threads each ran on, which its seconds and peak were measured at
        ("unit_jobs", Value::Obj(threads.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect())),
        // a part's slowest tests, `[name, s, MB]` slowest first, and the tests that ran at its
        // highest look (land_gate.sh test_times)
        ("unit_tests", Value::Obj(tests_s.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(|(n, secs, mb)| Value::Arr(vec![s(n), Value::F64(*secs), Value::Int(*mb as i64)])).collect()))).collect())),
        ("unit_peak_tests", Value::Obj(peak_tests.iter().map(|(u, p)| (u.clone(), obj(vec![("mb", Value::Int(p.mb as i64)), ("at_s", Value::Int(p.at_s as i64)), ("tests", Value::Arr(p.tests.iter().map(s).collect()))]))).collect())),
    ])).map_err(|e| format!("bucket ledger: {e}"))?;
    // each red its gate ran once more on its box at its tip and that passed there: a flake, on the
    // flake list with both runs' outputs, and nobody named (main, 11-24)
    for (unit, tests) in flake_lines(last_run(&text)) {
        let logs: Vec<String> = std::iter::once(outcome.log.clone()).chain(kept.get(&unit_key(&unit)).map(|at| format!("both runs' output: {at}"))).collect();
        flake_said(home, name, &unit, &tests, tip_pin, &logs, started, emit);
    }
    // each test it measured past libtest's own warning: the lane of the newest landing that touched
    // its package hears it, once a day (main, 11-23)
    let live: BTreeSet<String> = store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| l.branch.clone()).collect());
    match slow_notes(home, name, tip_pin, &outcome.log, &tests_s, &history, &|e| live.contains(&e.branch), started) {
        Ok(notes) => for (agent, line) in notes {
            if let Err(e) = crate::notes::leave(home, &agent, &line) { eprintln!("devhub bucket {name}: agent notes: {e}"); }
        },
        Err(e) => eprintln!("devhub bucket {name}: slow tests: {e}"),
    }
    if !tests_s.is_empty() {
        match weekly_slowest(home, started) {
            Ok(Some(line)) => if let Err(e) = crate::notes::leave(home, "main", &line) { eprintln!("devhub bucket {name}: agent notes: {e}"); },
            Ok(None) => {}
            Err(e) => eprintln!("devhub bucket {name}: weekly slow tests: {e}"),
        }
    }
    // The red row must precede its probes: GATE_RETRY then includes the red unit (11-15).
    if let Some(bisect) = bisect {
        for job in &jobs { bisect(job.clone()); }
    }
    let latency = entries.iter().map(|e| e.changed_ms).max().map(|landed| {
        format!("land->verdict {} s; ", ended.saturating_sub(landed) / 1000)
    }).unwrap_or_default();
    let (sev, line, detail) = if outcome.ok {
        (Sev::Info, format!("bucket {name} {} ok ({latency}{})", tip_short(tip_pin), outcome.summary), String::new())
    } else {
        let blamed = entries.iter().zip(&units).flat_map(|(e, units)| units.iter().filter(|u| !bisected(&unit_key(u))).map(move |u| {
            format!("{} ({}) changed {u}", e.label(), e.agent)
        // each with the log of its last look: this run's when it failed here, else the run that last looked
        })).chain(carried.iter().map(|(u, who)| (u, who, &outcome.log)).chain(still.iter().map(|(u, (who, log))| (u, who, log)))
            .map(|(u, who, log)| format!("{u} still failing {} (log {log})", still_since(who))))
            .chain(by_file.iter().filter(|(u, _)| !bisected(&unit_key(u))).map(|(u, (_, who))| format!("{u} broken by {who}")))
            .chain(first.iter().map(|(u, who)| format!("{u} {who}")))
            .chain(by_test.iter().filter(|(key, _)| !bisected(key)).map(|(key, who)| format!("{key} failing {who} (log {})", outcome.log)))
            .chain(passed_since.iter().map(|(key, by)| format!("{key} passed since in a later run of the {by} bucket: closed, nobody told")))
            .chain(jobs.iter().map(|j| if j.closed { format!("{} runs once more at its red tip: its owner is told should it pass there", j.unit) }
                else if j.of_a_leg() { format!("{} runs once more at its red tip: its owner is told should it pass there, else {}", j.unit,
                    if j.named.is_empty() { "nobody: it stays STILL FAILING".to_string() } else { format!("the landing its errors name ({})", j.named.iter().map(|id| format!("#{id}")).collect::<Vec<_>>().join(", ")) }) }
                else { format!("{} bisected since {}: one landing is told, or the test's owner should it pass at its red tip", j.unit, tip_short(&j.from)) })).collect::<Vec<_>>().join("; ");
        let mut detail = crate::land::failure_lines(&text);
        detail.extend(repro.iter().map(|(u, cmd)| repro_said(u, cmd)));
        if let Some(e) = attribution_error { detail.push(e); }
        // each red unit with its label (a run's exit after its listing) and where its output is kept
        let failing = outcome.failing.iter().map(|u| {
            let label = errors.get(u).and_then(|e| e.first()).filter(|e| e.starts_with(EXIT_AFTER_LISTING)).map(|_| EXIT_AFTER_LISTING.to_string());
            let said: Vec<String> = label.into_iter().chain(kept.get(&unit_key(u)).map(|at| format!("its output: {at}"))).collect();
            if said.is_empty() { u.clone() } else { format!("{u} ({})", said.join("; ")) }
        }).collect::<Vec<_>>().join(", ");
        (Sev::Err, format!("bucket {name} {} FAILED {failing}: {blamed}{} ({latency}{})", tip_short(tip_pin), if ambiguous { " (ambiguous)" } else { "" }, outcome.summary), detail.join("\n"))
    };
    // each red's note for its landing's agent and for main, unsaid should a later run pass its
    // units before their next call ([`reds_passed_since`])
    for (agent, line, keys) in broken {
        emit(Sev::Err, line.clone(), String::new());
        let to = if agent == "main" { vec!["main"] } else { vec![agent.as_str(), "main"] };
        for to in to {
            if let Err(e) = crate::notes::leave_red(home, to, &line, started, &keys) {
                eprintln!("devhub bucket {name}: agent notes: {e}");
            }
        }
    }
    emit(sev, line, detail);
    Ok(())
}

/// The prebuild legs' test listings of a landed tip under the JIT it brought (orch/warm.rs), as
/// bucket `prebuild`'s verdict: a listing that fails there is a break on the branch, named for the
/// landings that changed its package. `outcome.log` holds each
/// listing's output and its `GATEUNIT` line; `base` is the previous prebuild's tip.
pub fn prebuild(home: &Path, repo: &Path, store: &Store, started: u64, base: Option<Pair>, tip: &Pair, outcome: Outcome) -> Result<(), String> {
    let emit = |sev: Sev, line: String, detail: String| store.event("bucket", sev, line, Some(detail));
    judge("prebuild", home, repo, store, &emit, started, base.as_ref().map(pin_of), &pin_of(tip), None, |_| Ok(outcome)).map_err(|e| e.to_string())
}

/// Bucket `name` at a stage of its run ([`Board::doing`]) until this drops.
struct Stage<'a>(&'a Board, &'a str);

impl<'a> Stage<'a> {
    fn at(board: &'a Board, name: &'a str, now: (&'static str, u64, String)) -> Stage<'a> {
        board.doing(name, Some(now));
        Stage(board, name)
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        self.0.doing(self.1, None);
    }
}

/// The newest run of bucket `name` its ledger has no row of: its sidecar `<name>-<ms>.log.run`
/// newer than `last_start` (its newest row's start; None: it has none), which a restarted master
/// adopts, a run that ran or one that waited for the pool boxes. A sidecar stays after its row,
/// and a shard's (`<name>-s<i>-<ms>`) is no run of the bucket's.
fn unfinished(logs: &Path, name: &str, last_start: Option<u64>) -> Option<(u64, PathBuf)> {
    let prefix = format!("{name}-");
    std::fs::read_dir(logs).ok().into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        let file = entry.file_name();
        let started: u64 = file.to_str()?.strip_prefix(&prefix)?.strip_suffix(".log.run")?.parse().ok()?;
        last_start.is_none_or(|last| started > last).then_some((started, entry.path()))
    }).max_by_key(|(started, _)| *started)
}

/// Each bucket owns one timer and runs synchronously on its own thread.
pub fn spawn(name: &str, timing: Timing, repo: PathBuf, home: PathBuf, publish: Publish, run: Run, prior: Prior, probe: Option<Probe>) {
    let name = name.to_string();
    let thread = std::thread::Builder::new().name(format!("devhub-bucket-{name}"));
    if let Err(e) = thread.spawn(move || {
        let machine = makepad_devhub_conf::default_machine_name();
        let store = match Store::open_quiet(&home, "") { Ok(s) => Arc::new(s), Err(e) => { eprintln!("devhub bucket: {e}"); return; } };
        let mut mine = ledger(&home, usize::MAX).into_iter().filter_map(|line| {
            let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
            (v.get("bucket")?.as_str()? == name).then_some(v)
        });
        let latest = mine.next();
        if name == "tests" {
            let earlier: Vec<Value> = mine.take(TIMES_ROWS - 1).collect();
            *TESTS_WALLS.lock().unwrap_or_else(|e| e.into_inner()) = Some(latest.iter().chain(&earlier).filter_map(wall_of).collect());
        }
        let mut last = latest.as_ref().and_then(next_base);
        let mut last_start = latest.as_ref().and_then(|v| v.get("started_ms")?.as_u64()).unwrap_or(0);
        let logs = home.join("bucket-logs");
        let kick = kick_file(&home, &name);
        let emit = |sev: Sev, line: String, detail: String| publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
        let tx = probe.and_then(|probe| {
            let (tx, rx) = std::sync::mpsc::channel::<Bisect>();
            match pending_bisects(&home, &name) {
                Ok(jobs) => for job in jobs { let _ = tx.send(job); },
                Err(e) => eprintln!("devhub bucket {name}: bisect restart: {e}"),
            }
            let (name, repo, home, store, publish, machine) = (name.clone(), repo.clone(), home.clone(), store.clone(), publish.clone(), machine.clone());
            match std::thread::Builder::new().name(format!("devhub-bisect-{name}")).spawn(move || {
                let emit = |sev: Sev, line: String, detail: String| publish(hub_record(&machine, "", 0, "bucket", sev, line, detail));
                for job in rx {
                    bisect_job(&repo, &home, &store, &emit, &probe, job);
                }
            }) {
                Ok(_) => Some(tx),
                Err(e) => { eprintln!("bucket bisect thread: {e}"); None }
            }
        });
        let bisect = |job: Bisect| {
            let Some(tx) = &tx else { return };
            match queue_bisect(&home, &job) {
                Ok(true) => { let _ = tx.send(job); }
                Ok(false) => {}
                Err(e) => eprintln!("devhub bucket {name}: bisect queue: {e}"),
            }
        };
        let ask_node = ask_box;
        let record = |started: u64, base: Option<Pin>, tip: Pin, log: PathBuf| -> Result<Pin, JudgeError> {
            let _at = Stage::at(&BOARD, &name, ("running", started, format!("its run of {}; log {}", tip_short(&tip), log.display())));
            let result = judge(&name, &home, &repo, &store, &emit, started, base, &tip, tx.as_ref().map(|_| &bisect as &dyn Fn(Bisect)), |base| match base {
                Some(base) => {
                    std::fs::create_dir_all(&logs).map_err(|e| format!("bucket logs: {e}"))?;
                    Ok(run(Some(base), &tip, &log))
                }
                None => Ok(Outcome { ok: true, failing: Vec::new(), log: String::new(), summary: "baseline".into(), no_verdict: None }),
            });
            finish_run(&log, result)?;
            BOARD.doing(&name, Some(("keeping its failed run's JITs", wall_ms(), format!("asks the boxes that ran them to hold them; log {}", log.display()))));
            keep_failed_jits(&name, &home, &log, &ask_node, &away_nodes(), &emit);
            // (its row's tip, or its base when its gate stopped short of its units)
            Ok(rows_of(&home, &name).first().and_then(next_base).unwrap_or(tip))
        };
        if let Some((ms, sidecar)) = unfinished(&logs, &name, latest.as_ref().map(|_| last_start)) {
            if let Some((pid, started, base, tip)) = read_run(&sidecar).filter(|(_, started, _, _)| *started == ms) {
                publish(hub_record(&machine, "", 0, "bucket", Sev::Info,
                    format!("bucket {name} adopts its run of {} from before the master restart (pid {pid})", tip_short(&tip)), String::new()));
                last = Some(match record(started, Some(base), tip.clone(), sidecar.with_extension("")) {
                    Ok(next) => next,
                    Err(e) => { eprintln!("bucket {name}: {e}"); tip }
                });
                last_start = started;
            }
        }
        // a newer run of its kind than its ledger has (its asker died with a master) is its last
        if let Some((started, tip)) = store.read(prior).filter(|(started, _)| *started > last_start) {
            last_start = started;
            last = Some(pin_of(&tip));
        }
        // the tip at this thread's first look is no change; one it sees change later is (a push
        // that is no landing)
        let (mut seen, mut changed, mut unlanded) = (None::<Pin>, 0, None::<u64>);
        let poll = Duration::from_secs(if timing.quiet < Duration::from_secs(5) { 1 } else { 5 });
        loop {
            std::thread::sleep(poll);
            let Some(tip) = crate::orch::snapshot_pin(&repo) else { continue };
            let now = wall_ms();
            if seen.as_ref().is_some_and(|s| *s != tip) { changed = now; }
            seen = Some(tip.clone());
            // asked for (`devhub bucket run`): now, of this tip, whatever its timing says
            if std::fs::remove_file(&kick).is_err() {
                if last.as_ref() == Some(&tip) {
                    unlanded = None;
                    continue;
                }
                let landed = landed_ms(&store);
                let first = landed.iter().copied().filter(|ms| *ms > last_start).min().unwrap_or_else(|| *unlanded.get_or_insert(now));
                let last_changed = landed.iter().copied().max().unwrap_or(0).max(changed);
                if now < starts_at(&timing, last_start, first, last_changed) && !quiet_queue_due(&timing, last_start, now, crate::orch::leases().map(|lm| lm.queue_busy_ms())) { continue; }
            }
            let started = now;
            last_start = started;
            unlanded = None;
            match record(started, last.clone(), tip, logs.join(format!("{name}-{started}.log"))) {
                Ok(tip) => last = Some(tip),
                Err(e) => eprintln!("bucket {name}: {e}"),
            }
        }
    }) { eprintln!("bucket thread: {e}"); }
}

pub fn fast(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    fast_gate(repo, home, proofs, ["gate", "landq-fast", "interactive"], Some("0"))
}

/// The fast gate at one landing of a bisect: a background test lease, never a gate one, so it
/// takes no box's gate slot and no landing waits behind it (a `landq` gate lease is a landing
/// gate to the scheduler, 11-15).
pub fn fast_probe(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Probe {
    let gate = fast_gate(repo, home, proofs, ["test", BISECT_LANE, "background"], None);
    Arc::new(move |_, base, tip, log| gate(base, tip, log))
}

/// The lane of a bisect's probe leases.
const BISECT_LANE: &str = "landq-bisect";

/// The tests and full buckets' probe at one pin of a bisect: the red unit alone (`GATE_ONLY`;
/// `GATE_RETRY` puts its package in the run whatever the pin's diff reaches, and no proof skips
/// it) on a background test lease, never a shard's pool slot. Neither bucket had one: a red was
/// laid to every landing that moved its package's inputs, which for Fab is nearly every landing
/// (one key-shadow red told 14 landings of 8 lanes, and it passed at the next tip, 11-17).
pub fn tests_probe(repo: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Probe {
    Arc::new(move |unit, base, tip, log| {
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let base = base.expect("a bisect probes against its last pass");
        let needs = gate_needs();
        let lease = ["lease", "test", "--needs", &needs, "--cores", "8", "--priority", "background", "--lane", BISECT_LANE, "--rev", &rev];
        let (only, retry) = (format!("GATE_ONLY={unit}"), format!("GATE_RETRY={unit}"));
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &only, &retry, &nested, "bash", "tools/run/fixture/land_gate.sh", "testrun", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
        let outcome = probe_verdict(status, &text, log, &rev);
        if outcome.no_verdict.is_none() {
            proofs.record_log(&repo, &tip.pair(), &text, &format!("bisect {unit} {}", tip_short(tip)));
        }
        outcome
    })
}

fn fast_gate(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>, [kind, lane, priority]: [&'static str; 3], prio: Option<&'static str>) -> Run {
    Arc::new(move |base, tip, log| {
        let (units, why) = proofs.proven(&repo, &tip.pair());
        let retry = retry_env(&rows_of(&home, "fast"));
        let base = base.expect("fast bucket needs a base");
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        // its presents share these cores (land_gate.sh step_qpresent): what the box sets aside for them.
        // Interactive, as the hot gate: every hand-in waits on its verdict. At background priority
        // a lane's whole asks held two pool boxes against it while its 8 cores waited on a tests
        // shard on the third, every landing behind it 15 min, and it ran at a background CPU
        // weight beside live calls (L657290, 11-08).
        let needs = gate_needs();
        let mut lease = vec!["lease", kind, "--needs", &needs, "--priority", priority, "--lane", lane];
        // a bisect's probe asks no count: its class's measure charges it, as devhub's other legs.
        // It used 7.3 cores for a minute on an aged start of 3 against the gate's 8, while its
        // runs average 3-5 (L1632210, 11-23)
        if lane != BISECT_LANE { lease.extend(["--cores", "8"]); }
        if let Some(prio) = prio { lease.extend(["--prio", prio]); }
        lease.extend(["--rev", &rev]);
        let proven = format!("GATE_PROVEN={}", units.join(";"));
        let input = match QueueInput::write(log, &[&proven]) { Ok(i) => i, Err(e) => return no_verdict(e) };
        lease.extend(["--input", &input.arg]);
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &retry, &nested, "bash", "tools/run/fixture/land_gate.sh", "fast", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &why, log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
        let probing = lane == BISECT_LANE;
        let outcome = if probing { probe_verdict(status, &text, log, &rev) } else { verdict(status, &text, log) };
        if !probing || outcome.no_verdict.is_none() {
            proofs.record_log(&repo, &tip.pair(), &text, &format!("bucket fast {}", tip_short(tip)));
        }
        outcome
    })
}

/// Runs the tests of the changed packages and the gate packages the landings reach, the smoke set
/// and JIT checks too when they reach the JIT (affected.py --runs: the fast bucket's selection, both
/// repos; every user of a JIT or platform change was 547 units and held most of the pool's boxes for
/// 78-106 min a run, 11-08; the full bucket runs every package's daily) under the tree's JIT on the
/// Linux pool boxes, after landing and at background priority: it gates nothing. One unit `test <pkg>` each, skipped while its inputs are proven, run
/// again while red until it passes (land_gate.sh testrun). The units are one queue, slowest first
/// by their last measures ([`times_env`]), dealt a unit at a time to one shard per box
/// ([`test_boxes`]) as each has a slot free ([`Dealer`]), each shard its own lease and log (a full
/// reach, 363 packages two at a time on one box, ran 45 min; 11-07), and the run's log is theirs,
/// one after the other.
pub fn tests(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    test_run(repo, home, proofs, false)
}

/// The full bucket's run: every package's tests as the tests bucket runs them, no proof reused
/// and none of its retries needed (`GATE_FULL=1`), dealt by the units' measures of both buckets.
pub fn full(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>) -> Run {
    test_run(repo, home, proofs, true)
}

/// The tests and full buckets' rows, newest first: the units' measures of either deal the next.
fn test_rows(home: &Path) -> Vec<Value> {
    ledger(home, 2_000).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("bucket").and_then(Value::as_str).is_some_and(|b| b == "tests" || b == FULL)).collect()
}

fn test_run(repo: PathBuf, home: PathBuf, proofs: Arc<crate::gateproof::Store>, full: bool) -> Run {
    Arc::new(move |base, tip, log| {
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let base = base.expect("tests bucket needs a base");
        // a run a restarted master adopts is dealt as it was (its shards' leases run on, each
        // with its sidecar beside its log); a new one over the pool boxes there are now. Its
        // sidecar comes before its wait for the pool boxes: an asked run that waited took its
        // kick file, and a master restart in the wait lost it (11-12)
        let adopted = log.with_extension("log.run").exists().then(|| run_shape(log)).flatten();
        if adopted.is_none() {
            if let Some(Err(e)) = log_started(log).map(|started| save_run(log, std::process::id(), started, base, tip)) {
                eprintln!("bucket {}: {}: {e} (a master restart before it ends loses it)", bucket_of(log), log.display());
            }
        }
        let say = |line: String| eprintln!("{line}");
        let _on = BOARD.enter(&home, log, &tip_short(tip), &say);
        // a new run's first shard takes its slot before what it runs is read: a run that waited
        // starts on what the run before it proved. An adopted run's shards take theirs back as
        // their leases run on ([`Need::Running`]).
        let first = Mutex::new(if adopted.is_none() { BOARD.slot(&home, log, 1, Need::Lease, &|| true, &say) } else { None });
        let QueueMade { why, proven, retry, times, peaks, jobs, tests, floor } = queue_made(log, adopted.is_some(), || {
            let (units, why) = if full { (Vec::new(), vec![format!("full: every package's tests at {}, no proof reused", tip_short(tip))]) } else { proofs.proven(&repo, &tip.pair()) };
            let rows = test_rows(&home);
            QueueMade {
                why,
                proven: format!("GATE_PROVEN={}", units.join(";")),
                retry: if full { "GATE_RETRY=".to_string() } else { retry_env(&rows_of(&home, "tests")) },
                times: times_env(&rows),
                peaks: peaks_env(&rows),
                jobs: jobs_env(&rows),
                tests: test_times_env(&rows),
                floor: ram_floor(&rows),
            }
        });
        // (made again from the saved queue by a master that adopts the run: the same bytes)
        let input = match QueueInput::write(log, &[&proven, &times, &peaks, &jobs, &tests]) { Ok(i) => i, Err(e) => return verdict(Err(e), "", log) };
        let reach = format!("GATE_FULL={}", if full { "1" } else { "" });
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let (shards, cores) = match adopted {
            Some(shape) => shape,
            None => {
                let shape = pool_boxes(&bucket_of(log));
                if let Err(e) = std::fs::write(log, format!("{SHARDS}{} {}\n", shape.0, shape.1)) {
                    return verdict(Err(format!("bucket log {}: {e}", log.display())), "", log);
                }
                shape
            }
        };
        let (ask, class) = (cores.to_string(), shard_class(cores, full));
        // the run's shards and the cores each asks, by which a shard gives a long unit more of its
        // own (land_gate.sh unit_slots)
        let sharded = format!("GATE_SHARDS={shards} {cores}");
        // the run's one queue, dealt as the shards' slots free ([`Dealer`])
        let dealer = Mutex::new(Dealer::new(shards));
        // when it took its tip: by day its shards hold a box only once it is old (`--run-since`)
        let since = log_started(log).unwrap_or_else(wall_ms).to_string();
        let ended: Vec<AtomicBool> = (0..shards).map(|_| AtomicBool::new(false)).collect();
        let sweep = sweeps(full, crate::orch::ci::Inputs::between(&repo, &bases, &tip.pair()).map(|i| i.jit_changed), gate_at(&repo, &tip.pair().work).as_deref());
        let (mut ran, unsaid, mut covered, swept) = std::thread::scope(|scope| {
            scope.spawn(|| deal(&dealer, &ended, log, &home));
            let swept = sweep.then(|| scope.spawn(|| libs_sweep(&repo, log, &rev, &since, (base, tip))));
            let shard = |i: usize| {
                let (repo, home, first, say, retry, input, reach, ask, class, dealer, ended, rev, bases, nested, floor, sharded) = (&repo, &home, &first, &say, &retry, &input, &reach, &ask, &class, &dealer, &ended, &rev, &bases, &nested, &floor, &sharded);
                let since = since.as_str();
                move || {
                    let _ended = Ends(&ended[i - 1]);
                    let shard_log = shard_log(log, i);
                    // at most a run per 4 of its cores (land_gate.sh TEST_CORES); a shape not
                    // measured yet is a large lease (20-40 min on its cores), not its kind's
                    // small: its three shards ran small until their class had measures (11-07)
                    // never resized below its largest unit's last peak: a unit runs alone at most
                    // (land_gate.sh ram_room) and starts within the RAM its lease got
                    let needs = gate_needs();
                    // (`--yields`: its gate reads $DEVHUB_YIELD before each unit it asks for, so a
                    // bench box is lent it whatever its length and has it back at a unit's end)
                    let mut lease = vec!["lease", "test", "--needs", needs.as_str(), "--cores", ask, "--cost", "large", "--class", class, "--priority", "background", "--lane", crate::lease::sched::TESTS_LANE, "--rev", rev, "--run-since", since, "--yields"];
                    if let Some(gb) = floor { lease.extend(["--ram-floor-gb", gb.as_str()]); }
                    lease.extend(["--input", input.arg.as_str()]);
                    let gate = ["env", retry, sharded, reach, nested, "GATE_QUEUE=1", "bash", "tools/run/fixture/land_gate.sh", "testrun", &bases.work, &bases.main];
                    let needed = || dealer.lock().unwrap_or_else(|e| e.into_inner()).needed();
                    // its pool slot for as long as its lease (a lost one's second, too)
                    let mut held = if i == 1 { first.lock().unwrap_or_else(|e| e.into_inner()).take() } else { None };
                    let mut slot = |need: Need| {
                        if matches!(need, Need::Gave) {
                            held = None;
                            return false;
                        }
                        if held.is_none() { held = BOARD.slot(home, log, i, need, &needed, say); }
                        held.is_some()
                    };
                    let more = || dealer.lock().unwrap_or_else(|e| e.into_inner()).take_yield(i - 1);
                    // a full run's shard whose lease waits for a box gives its slot to a run that goes first
                    let withdraw = || BOARD.give_queued(log, i, say);
                    let run = lease_run(repo, &lease, &gate, &[], &shard_log, (base, tip), &needed, &mut slot, &more, &withdraw);
                    // (one more shard may follow: [`Slot::ended`])
                    if let Some(slot) = held.take() { slot.ended(); }
                    hold_run_jits(log, &run.1, &ask_box);
                    (shard_log, run)
                }
            };
            let handles: Vec<_> = (1..=shards).map(|i| scope.spawn(shard(i))).collect();
            let mut ran: Vec<(PathBuf, (Result<Option<i32>, String>, String))> = handles.into_iter().enumerate().map(|(i, h)| h.join().unwrap_or_else(|_| (shard_log(log, i + 1), (Err("its shard's thread panicked".to_string()), String::new())))).collect();
            // a shard that ended without its units' verdicts (cancelled, lost, killed): those units run
            // once more on another box than that shard's, and what that run leaves unjudged too is said
            // as no verdict: a full run's shard on a box whose hub restarted under it ended cancelled
            // with 158 of its units unjudged, and its row named one red of the run's 614 (11-12). It
            // runs as its shards end, beside the sweep, on the slot its last shard left the run
            // ([`Slot::ended`])
            let left: Vec<(usize, Unjudged)> = ran.iter().enumerate().filter_map(|(i, (_, (status, part)))| unjudged(status, part).map(|u| (i, u))).collect();
            let (mut unsaid, mut covered): (Vec<String>, Vec<usize>) = (Vec::new(), Vec::new());
            if !left.is_empty() {
                let units: Vec<&str> = left.iter().flat_map(|(_, u)| u.units.iter().map(String::as_str)).collect();
                let avoid = left.iter().map(|(_, u)| u.node.as_str()).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(",");
                say(format!("bucket {}: {} unit(s) its shards left without verdicts run once more{}: {}", bucket_of(log), units.len(), if avoid.is_empty() { String::new() } else { format!(", not on {avoid}") }, left.iter().map(|(_, u)| format!("{} ({})", u.lease, u.why)).collect::<Vec<_>>().join("; ")));
                let only = format!("GATE_ONLY={}", units.join(";"));
                let (ask, class) = (cores.to_string(), shard_class(cores, full));
                let needs = gate_needs();
                let mut lease = vec!["lease", "test", "--needs", needs.as_str(), "--cores", ask.as_str(), "--cost", "large", "--class", class.as_str(), "--priority", "background", "--lane", crate::lease::sched::TESTS_LANE, "--rev", rev.as_str(), "--run-since", since.as_str()];
                if let Some(gb) = &floor { lease.extend(["--ram-floor-gb", gb.as_str()]); }
                if !avoid.is_empty() { lease.extend(["--avoid", avoid.as_str()]); }
                lease.extend(["--input", input.arg.as_str()]);
                // (one shard's: its units are what the shards left, and a long one takes its cores)
                let sharded = format!("GATE_SHARDS=1 {cores}");
                let gate = ["env", retry.as_str(), sharded.as_str(), reach.as_str(), nested.as_str(), only.as_str(), "bash", "tools/run/fixture/land_gate.sh", "testrun", bases.work.as_str(), bases.main.as_str()];
                let i = shards + 1;
                let mut held = None;
                let mut slot = |need: Need| {
                    if held.is_none() { held = BOARD.slot(&home, log, i, need, &|| true, &say); }
                    held.is_some()
                };
                let again = shard_log(log, i);
                let rerun = lease_run(&repo, &lease, &gate, &[], &again, (base, tip), &|| true, &mut slot, &|| false, &|| false);
                hold_run_jits(log, &rerun.1, &ask_box);
                unsaid = no_verdicts(&left.iter().map(|(_, u)| u).collect::<Vec<_>>(), &rerun.1);
                // a shard whose units the rerun judged has its verdicts there: its own end is no verdict's
                if unsaid.is_empty() { covered = left.iter().map(|(i, _)| *i).collect(); }
                ran.push((again, rerun));
            } else {
                // nothing for one more shard: the slot kept for it frees now, not after the sweep
                BOARD.let_go_kept(log);
            }
            (ran, unsaid, covered, swept.map(|h| h.join().unwrap_or_else(|_| (libs_log(log), (Err("the libraries' sweep's thread panicked".to_string()), String::new())))))
        });
        // a shard its box ended for memory whose units were dealt again and judged elsewhere: its own
        // end is no verdict ([`redealt_ends`])
        covered.extend(redealt_ends(&ran.iter().map(|(_, (status, part))| (status, part.as_str())).collect::<Vec<_>>()));
        // (after the rerun of what shards left unjudged: that reruns test units, the sweep has none)
        ran.extend(swept);
        let text = merged(&why, (shards, cores), &ran);
        if let Err(e) = std::fs::write(log, &text) {
            return verdict(Err(format!("bucket log {}: {e}", log.display())), &text, log);
        }
        proofs.record_log(&repo, &tip.pair(), &text, &format!("bucket tests {}", tip_short(tip)));
        let judged: Vec<_> = ran.iter().enumerate().filter(|(i, _)| !covered.contains(i)).map(|(_, r)| r.clone()).collect();
        let status = worst(&judged);
        let mut outcome = verdict(status.clone(), &text, log);
        // a part that said nothing of the landings is named; the rest's verdicts stand: a red on
        // its failing units, and with none, no verdict that names it
        let named = unsaid_parts(log, shards, &judged);
        if !named.is_empty() {
            match &mut outcome.no_verdict {
                // (beside a part that ended without an exit status, which keeps its own words)
                Some(why) if status == Ok(None) => *why = if judged.iter().any(|(_, (s, _))| *s == Ok(None)) { format!("{why}; {}", named.join("; ")) } else { named.join("; ") },
                _ if !outcome.ok => outcome.summary = [outcome.summary.as_str(), &named.join("; ")].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", "),
                _ => {}
            }
        }
        if !unsaid.is_empty() {
            outcome.ok = false;
            outcome.failing.extend(unsaid);
        }
        // units of the queue no shard was dealt said nothing: a run its shards all ended before
        // (each lost twice, or told none of the run's queue) is no verdict, and its tip runs again;
        // a red stands on its failing units
        if let Some((never, units)) = dealer.lock().unwrap_or_else(|e| e.into_inner()).undealt() {
            say(format!("bucket {}: {never} of its queue's {units} units were never dealt: its shards ended first", bucket_of(log)));
            if outcome.ok {
                outcome.ok = false;
                outcome.no_verdict = Some(format!("{never} of its queue's {units} units were never dealt (its shards ended first)"));
            }
        }
        outcome
    })
}

/// What a shard's part of a run left without a verdict ([`unjudged`]).
#[derive(Debug, PartialEq)]
struct Unjudged {
    lease: String,
    node: String,
    /// its client's `LEASE CANCELLED`/`LEASE LOST` line, else its exit
    why: String,
    units: Vec<String>,
}

/// The units a shard's part of a run started (`GATESTART`) and said no verdict of (`GATEUNIT`, or
/// its end for memory, [`memory_ends`]): its lease cancelled, lost or killed before its units'
/// lines. None: it judged every unit it started.
fn unjudged(status: &Result<Option<i32>, String>, part: &str) -> Option<Unjudged> {
    // the toolchain changed under the shard (its client's `TOOLCHAIN CHANGED` line): a unit that
    // failed has no verdict, as one it never judged (a rustup update beside a shard failed its
    // cargo with rustup's "binary not applicable", 11-13)
    let toolchain = part.lines().any(|l| l.starts_with("TOOLCHAIN CHANGED"));
    let said: BTreeSet<&str> = part.lines().filter_map(|l| l.strip_prefix("GATEUNIT ")?.split_once(' ').filter(|(status, _)| !(toolchain && *status == "fail")).map(|(_, u)| u.trim())).collect();
    // (a unit its box ended for memory, or one its lease was cancelled under while it hung, is
    // judged by that end: [`verdict`])
    let ended: BTreeSet<String> = memory_ends(part).into_iter().chain(hung_ends(part)).map(|(u, _)| u).collect();
    let mut units: Vec<String> = Vec::new();
    for line in part.lines().filter_map(|l| l.strip_prefix("GATESTART ")) {
        let line = line.trim_end();
        let unit = line.rsplit_once(' ').filter(|(_, at)| at.bytes().all(|b| b.is_ascii_digit())).map_or(line, |(u, _)| u);
        if !said.contains(unit) && !ended.contains(unit) && !units.iter().any(|u| u == unit) { units.push(unit.to_string()); }
    }
    if units.is_empty() { return None; }
    let (lease, node) = part_box(part);
    let why = part.lines().rev().find(|l| l.starts_with("LEASE CANCELLED") || l.starts_with("LEASE LOST") || l.starts_with("TOOLCHAIN CHANGED")).map(|l| crate::proto::clean(l, 160)).unwrap_or_else(|| match status {
        Err(e) => e.clone(),
        Ok(Some(code)) => format!("exit {code}"),
        Ok(None) => "it ended without an exit status".into(),
    });
    Some(Unjudged { lease, node, why, units })
}

/// The lease and box of a shard's part of a run (its `L1 box-a: sources at ..` line), else empty.
fn part_box(part: &str) -> (String, String) {
    part.lines().find_map(|l| {
        let (lease, node) = l.split_once(": sources at ")?.0.split_once(' ')?;
        (lease.starts_with('L') && !node.contains(' ')).then(|| (lease.to_string(), node.to_string()))
    }).unwrap_or_default()
}

/// The parts of the run of `log` (its `shards`, the rerun of what they left, the libraries' sweep)
/// that exited 0 without their verdict line, each named with its lease and box: `no verdict,
/// twice` when it ran once more ([`RERUN_UNSAID`]) and said nothing again. The other parts'
/// verdicts stand.
fn unsaid_parts(log: &Path, shards: usize, ran: &[(PathBuf, (Result<Option<i32>, String>, String))]) -> Vec<String> {
    ran.iter().filter(|(_, (status, part))| matches!(status, Ok(Some(0))) && !part.lines().any(|l| l.starts_with("NODEGATE-OK") || l.starts_with("NODEGATE-FAIL") || l == NOT_NEEDED)).map(|(at, (_, part))| {
        let name = if *at == libs_log(log) { "the libraries' sweep".to_string() } else {
            (1..=shards + 1).find(|i| shard_log(log, *i) == *at).map_or_else(|| at.display().to_string(), |i| if i > shards { "the rerun of what its shards left".to_string() } else { format!("shard {i}") })
        };
        let raw = std::fs::read_to_string(at).unwrap_or_default();
        let twice = raw.rsplit_once(&format!("{YIELDED}\n")).map_or(raw.as_str(), |(_, p)| p).lines().any(|l| l == RERUN_UNSAID);
        let (lease, node) = part_box(part);
        let at = if lease.is_empty() { String::new() } else { format!(" ({lease} on {node})") };
        if twice { format!("{name}{at}: no verdict, twice (it exited 0 without its verdict line, and again as it ran once more)") } else { format!("{name}{at}: no verdict (it exited 0 without its verdict line)") }
    }).collect()
}

/// The row's line for each shard whose units its rerun (`again`, its part) left without verdicts
/// too: `no verdict: N units of shard Lx (<why>)`.
fn no_verdicts(left: &[&Unjudged], again: &str) -> Vec<String> {
    let said: BTreeSet<&str> = again.lines().filter_map(|l| l.strip_prefix("GATEUNIT ")?.split_once(' ').map(|(_, u)| u.trim())).collect();
    left.iter().filter_map(|u| {
        let n = u.units.iter().filter(|x| !said.contains(x.as_str())).count();
        (n > 0).then(|| format!("no verdict: {n} unit(s) of shard {} ({}), its rerun on another box left them unjudged too", if u.lease.is_empty() { "?" } else { &u.lease }, u.why))
    }).collect()
}

/// How many of its bucket's newest runs a failed run's JIT outlives its failure in: once that many
/// ran without any of its units failing, the failure is fixed or a flake gone, and its JIT goes.
const KEEP_RUNS: usize = 10;

/// The JITs a run's failed leases ran, as `(node, key, binary)`. A lease's part of the log runs
/// from its line `L1 box-a: sources at ...` (a queue line names other boxes) to the next lease's;
/// its JIT is the gate's `jit: kept <key>` or the hub's `devhub: kept the JIT binary <key>` there,
/// and its binary the path a line there names (`GATEPEAKBY ... /x/makepad.jit-<key> run ...`). It
/// failed when it says `GATEUNIT fail <unit>` of a unit in `failing`. A log that says so of none
/// keeps every JIT it ran. A tests run's three shards ran three JITs on three boxes, and keeping
/// the passing shards' too asked a box for a loader its gate tree had already let go (11-10).
fn run_jits(text: &str, failing: &BTreeSet<String>) -> Vec<(String, String, Option<String>)> {
    // (node, keys, binaries, failed) of each lease's part
    let mut parts: Vec<(String, Vec<String>, Vec<String>, bool)> = Vec::new();
    for line in text.lines() {
        if let Some((lease, box_)) = line.split_once(": sources at ").and_then(|(head, _)| head.split_once(' ')) {
            if lease.starts_with('L') && lease[1..].bytes().all(|b| b.is_ascii_digit()) && !box_.is_empty() && !box_.contains(' ') {
                parts.push((box_.to_string(), Vec::new(), Vec::new(), false));
            }
        }
        let Some((_, keys, bins, failed)) = parts.last_mut() else { continue };
        let key = line.strip_prefix("jit: kept ").or_else(|| line.strip_prefix("devhub: kept the JIT binary ")).map(str::trim).filter(|k| k.len() == 24 && k.bytes().all(|b| b.is_ascii_hexdigit()));
        if let Some(k) = key.filter(|k| !keys.iter().any(|o| o == k)) { keys.push(k.to_string()); }
        bins.extend(line.split_whitespace().filter(|w| w.starts_with('/') && w.rsplit('/').next().is_some_and(|f| f.starts_with("makepad.jit-"))).map(str::to_string));
        *failed |= line.strip_prefix("GATEUNIT fail ").is_some_and(|u| failing.contains(&unit_key(u.trim())));
    }
    let any_failed = parts.iter().any(|p| p.3);
    let mut out: Vec<(String, String, Option<String>)> = Vec::new();
    for (node, keys, bins, _) in parts.into_iter().filter(|p| p.3 || !any_failed) {
        for key in keys {
            let bin = bins.iter().find(|b| b.rsplit('/').next().and_then(|f| f.strip_prefix("makepad.jit-")) == Some(key.as_str())).cloned();
            if !out.iter().any(|(n, k, _)| *n == node && *k == key) { out.push((node.clone(), key, bin)); }
        }
    }
    out
}

/// What keeps a failed run's JITs: `{bucket, log, node, key, hold, units}` a line, in devhub's home.
fn keeps_file(home: &Path) -> PathBuf {
    home.join("jit-keeps.jsonl")
}

/// A node's answer to a verb, through the master (`jit-keep`, `jit-unkeep`): its `err` an error.
fn ask_box(node: &str, line: &str) -> Result<(), String> {
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-bucket");
    let lines = crate::cli::ask(&c, Some(node), line.split(' ').next().unwrap_or(""), line)?;
    match lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        Some(v) => v.get("err").and_then(Value::as_str).map_or(Ok(()), |e| Err(e.to_string())),
        None => Err(format!("no answer: {lines:?}")),
    }
}

/// The holds a run's shards took on their JITs as each ended ([`hold_run_jits`]), `<node> <hold>`
/// a line, beside its log: its verdict lets them go ([`keep_failed_jits`]).
fn run_holds(log: &Path) -> PathBuf {
    log.with_extension("log.holds")
}

/// A shard's JIT stays on its box from the shard's end to its run's verdict, held in the box's dev
/// store as `<log stem>-run-<key>`: the verdict comes when the run's last shard ends, and a pool
/// box's store evicted a red shard's JIT in the 18 min between (71466bd26fdc, 11-13). `part`: the
/// shard's part of the log.
fn hold_run_jits(log: &Path, part: &str, ask: &dyn Fn(&str, &str) -> Result<(), String>) {
    let stem = log.file_name().map(|f| f.to_string_lossy().trim_end_matches(".log").to_string()).unwrap_or_default();
    for (node, key, bin) in run_jits(part, &BTreeSet::new()) {
        let hold = format!("{stem}-run-{}", &key[..8]);
        match ask(&node, &format!("jit-keep {hold} {key}{}", bin.map_or(String::new(), |b| format!(" {b}")))) {
            Ok(()) => {
                // one write of the whole line: the run's shards end on their own threads, and a
                // `writeln!` to the file wrote the node, the space, the hold and the newline apart,
                // so two shards ending at once ran their lines together (two boxes' names as one, 11-14)
                let line = format!("{node} {hold}\n");
                if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(run_holds(log)).and_then(|mut f| f.write_all(line.as_bytes())) {
                    eprintln!("bucket: {}: {e} (the hold of JIT {key} on {node} stays until released by hand)", run_holds(log).display());
                }
            }
            Err(e) => eprintln!("bucket: the JIT {key} a shard of {} ran on {node} is not held to its verdict: {e}", log.display()),
        }
    }
}

/// The holds listed in a run's holds file ([`run_holds`]) go, and the file with them.
fn release_holds(name: &str, file: &Path, ask: &dyn Fn(&str, &str) -> Result<(), String>, emit: &dyn Fn(Sev, String, String)) {
    let Ok(held) = std::fs::read_to_string(file) else { return };
    for (node, hold) in held.lines().filter_map(|l| l.split_once(' ')) {
        if let Err(e) = ask(node, &format!("jit-unkeep {hold}")) {
            emit(Sev::Warn, format!("bucket {name}: the hold {hold} on {node} of its run's JIT stays: {e}"), String::new());
        }
    }
    let _ = std::fs::remove_file(file);
}

/// A run's start in ms from its log's stem (`tests-1794551798465`).
fn run_ms(stem: &str) -> Option<u64> {
    stem.trim_end_matches(".log").rsplit_once('-')?.1.parse().ok()
}

/// The runs of bucket `name` in `logs` whose shards' JIT holds are still listed: no verdict let
/// them go yet ([`hold_run_jits`]), as (its start in ms, its holds file).
fn unjudged_holds(logs: &Path, name: &str) -> Vec<(u64, PathBuf)> {
    let prefix = format!("{name}-");
    let mut out: Vec<(u64, PathBuf)> = std::fs::read_dir(logs).into_iter().flatten().flatten().filter_map(|e| {
        let file = e.file_name().to_string_lossy().into_owned();
        let ms = file.strip_prefix(&prefix)?.strip_suffix(".log.holds")?.parse().ok()?;
        Some((ms, e.path()))
    }).collect();
    out.sort();
    out
}

/// How old a run without a verdict whose JIT holds stay is before `devhub bucket` names it: a
/// later verdict of its bucket lets them go, and a run that old had none since.
const STALE_HOLDS_MS: u64 = 24 * 3600 * 1000;

/// `devhub bucket`'s lines for the runs of the sharded buckets that got no verdict, are over
/// [`STALE_HOLDS_MS`] old and still hold their shards' JITs on their boxes.
fn stale_holds(home: &Path, now_ms: u64) -> Vec<String> {
    let logs = home.join("bucket-logs");
    SHARDED.iter().flat_map(|name| unjudged_holds(&logs, name).into_iter().filter(|(ms, _)| now_ms.saturating_sub(*ms) > STALE_HOLDS_MS).map(move |(ms, file)| {
        let holds: Vec<String> = std::fs::read_to_string(&file).unwrap_or_default().lines().filter_map(|l| l.split_once(' ')).map(|(node, hold)| format!("{hold} on {node}")).collect();
        format!("bucket {name}: its run {name}-{ms} ({} h ago) got no verdict and still holds its shards' JITs: {}; its bucket's next verdict lets them go", now_ms.saturating_sub(ms) / 3_600_000, holds.join(", "))
    })).collect()
}

/// The nodes no verb reaches now: down, or dev off (the master's `nodes`); none when it does not
/// answer.
fn away_nodes() -> BTreeSet<String> {
    crate::cli::control().and_then(|c| crate::cli::ask(&c, None, "nodes", "nodes")).map(|lines| away_of(&lines)).unwrap_or_default()
}

/// [`away_nodes`] from the `nodes` answer's lines.
fn away_of(lines: &[String]) -> BTreeSet<String> {
    lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter_map(|v| {
        let yes = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        let node = v.get("node").and_then(Value::as_str)?.to_string();
        (!yes("up") || !yes("dev")).then_some(node)
    }).collect()
}

/// A failed bucket run's JITs stay on the boxes that ran them, held in their dev stores
/// (`jit-keep`, where `--jit <key>` finds them), until its failure is gone: [`KEEP_RUNS`] newest
/// runs of its bucket without any of its units failing (fixed, or a flake that did not come back).
/// One kept on a box that is `away` (down, or dev off) waits for it without a word, and goes once
/// it is back: a box unplugged for days had each fast verdict warn again that its JITs stay
/// (11-16), and a hold its store keeps has no end but its release.
/// Its run dir is the run's kept files beside its log (`<log>.keep`), which stay with the log.
/// Every box evicted the JIT under which the fast bucket's csg-brep aborted in each run within the
/// hour, a lane's build of the same source never aborted, and `--jit` reached it nowhere (11-10).
/// `ask(node, line)`: a node's answer to a verb, its `err` an error.
fn keep_failed_jits(name: &str, home: &Path, log: &Path, ask: &dyn Fn(&str, &str) -> Result<(), String>, away: &BTreeSet<String>, emit: &dyn Fn(Sev, String, String)) {
    let rows = rows_of(home, name);
    let units = |r: &Value| -> BTreeSet<String> { r.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(unit_key).collect() };
    let mut keeps: Vec<Value> = std::fs::read_to_string(keeps_file(home)).unwrap_or_default().lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let before = keeps.len();
    let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let stem = log.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(failed) = rows.first().filter(|r| log_name(r).as_deref() == Some(stem.as_str())).map(units).filter(|u| !u.is_empty()) {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        for (node, key, bin) in run_jits(&text, &failed) {
            let hold = format!("{}-{}", stem.trim_end_matches(".log"), &key[..8]);
            match ask(&node, &format!("jit-keep {hold} {key}{}", bin.map_or(String::new(), |b| format!(" {b}")))) {
                Ok(()) => keeps.push(obj(vec![("bucket", s(name)), ("log", s(&stem)), ("node", s(&node)), ("key", s(&key)), ("hold", s(&hold)), ("units", Value::Arr(failed.iter().map(s).collect()))])),
                Err(e) => emit(Sev::Warn, format!("bucket {name}: the JIT {key} its failed run ran on {node} is not kept: {e}"), String::new()),
            }
        }
    }
    // the holds its shards took to its verdict go, once a red one's is kept above; and an older
    // run of this bucket that never got its verdict is superseded by this one: its holds go too
    release_holds(name, &run_holds(log), ask, emit);
    if let (Some(dir), Some(this)) = (log.parent(), run_ms(&stem)) {
        for (ms, file) in unjudged_holds(dir, name) {
            if ms < this { release_holds(name, &file, ask, emit); }
        }
    }
    keeps.retain(|k| {
        if field(k, "bucket") != name { return true; }
        let kept: BTreeSet<String> = k.get("units").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect();
        // (a run gone from the ledger is far older than its newest runs)
        let at = rows.iter().position(|r| log_name(r).as_deref() == Some(field(k, "log").as_str())).unwrap_or(usize::MAX);
        let gone = at >= KEEP_RUNS && rows.len() >= KEEP_RUNS && rows[..KEEP_RUNS].iter().all(|r| units(r).is_disjoint(&kept));
        if !gone || away.contains(&field(k, "node")) { return true; }
        match ask(&field(k, "node"), &format!("jit-unkeep {}", field(k, "hold"))) {
            Ok(()) => false,
            Err(e) => {
                emit(Sev::Warn, format!("bucket {name}: the JIT {} kept on {} for its failed run {} stays: {e}", field(k, "key"), field(k, "node"), field(k, "log")), String::new());
                true
            }
        }
    });
    if keeps.len() != before || rows.first().is_some_and(|r| log_name(r).as_deref() == Some(stem.as_str())) {
        let text: String = keeps.iter().map(|k| k.to_json() + "\n").collect();
        let tmp = keeps_file(home).with_extension("part");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, keeps_file(home))) {
            emit(Sev::Warn, format!("bucket {name}: {}: {e}", keeps_file(home).display()), String::new());
        }
    }
}

/// A tests run's log's first line: its shards and the cores each asks (`SHARDS <n> <cores>`), how
/// a restarted master that adopts the run deals it again (a master just restarted may not know
/// every box yet).
const SHARDS: &str = "SHARDS ";

/// The shards and cores the run's log says ([`SHARDS`]).
fn run_shape(log: &Path) -> Option<(usize, u32)> {
    let text = std::fs::read_to_string(log).ok()?;
    let (n, cores) = text.lines().find_map(|l| l.strip_prefix(SHARDS))?.split_once(' ')?;
    Some((n.parse().ok().filter(|n| *n > 0)?, cores.trim().parse().ok()?))
}

/// The run's variable `land_gate.sh` reads its queue's values from (queue_in): a folder of one file
/// each, named as the variable, which a gate lease gets as its input (`--input`).
const QUEUE_INPUT: &str = "GATE_QUEUE_DIR";

/// A gate run's queue values (`GATE_PROVEN`, `GATE_TIMES`, `GATE_PEAKS`, `GATE_JOBS`,
/// `GATE_TEST_TIMES`) as its lease's input archive beside its log ([`QUEUE_INPUT`]), never its
/// environment: as `env` values of its command every process of its units inherited them, 88-140
/// KB and growing with the queue, and a JIT that packed its run's environment into one string
/// failed its exec past 128 KiB: one Fab target's part was red in every tests run on one box
/// (11-19). A box takes the archive once by its hash; it goes from beside the log when the run
/// ends (the saved queue makes the same bytes again, [`queue_tar`]).
struct QueueInput {
    file: PathBuf,
    /// `--input`'s `GATE_QUEUE_DIR=<archive>`
    arg: String,
}

impl QueueInput {
    fn write(log: &Path, vars: &[&str]) -> Result<QueueInput, String> {
        let file = log.with_extension("log.queue.tar");
        // (whole or not at all: a restarted master's run writes it again under a holder it adopts)
        let temp = log.with_extension("log.queue.tar.tmp");
        std::fs::write(&temp, queue_tar(vars)).and_then(|_| std::fs::rename(&temp, &file)).map_err(|e| format!("bucket queue input {}: {e}", file.display()))?;
        Ok(QueueInput { arg: format!("{QUEUE_INPUT}={}", file.display()), file })
    }
}

impl Drop for QueueInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// `vars` (`NAME=value` each) as a tar archive of one file per name holding its value: the same
/// values make the same bytes (one time, no owner), so a box holds a queue once.
fn queue_tar(vars: &[&str]) -> Vec<u8> {
    // (a fixed time: one of 1970 reads as implausibly old to some tars)
    const MTIME: u64 = 1_790_000_000;
    let mut out = Vec::new();
    for var in vars {
        let (name, value) = var.split_once('=').unwrap_or((var, ""));
        let mut h = [0u8; 512];
        let n = name.len().min(100);
        h[..n].copy_from_slice(&name.as_bytes()[..n]);
        let mut octal = |at: usize, len: usize, v: u64| h[at..at + len].copy_from_slice(format!("{v:0w$o}\0", w = len - 1).as_bytes());
        octal(100, 8, 0o644);
        octal(108, 8, 0);
        octal(116, 8, 0);
        octal(124, 12, value.len() as u64);
        octal(136, 12, MTIME);
        h[148..156].fill(b' ');
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(value.as_bytes());
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

/// What a tests run's one queue is made from, fixed at its start beside its log (`.log.queue`): its
/// proven units and why (`GATE_PROVEN`), its retries, its units' last measures (`GATE_TIMES`, whose
/// order the queue is, the slowest first, and `GATE_PEAKS`), its split targets' tests' (which their
/// parts are dealt by, `GATE_TEST_TIMES`) and its RAM floor. A restarted master
/// that adopts the run deals the same queue: one that read the measures again made another, and
/// each lease asked after the restart made that one and was told units of the first (a full run
/// across two restarts read red with no test failing, 11-17).
#[derive(Debug, Clone, PartialEq)]
struct QueueMade {
    why: Vec<String>,
    proven: String,
    retry: String,
    times: String,
    peaks: String,
    jobs: String,
    tests: String,
    floor: Option<String>,
}

const WHY: &str = "WHY ";
const FLOOR: &str = "RAM_FLOOR_GB=";

impl QueueMade {
    fn file(log: &Path) -> PathBuf {
        log.with_extension("log.queue")
    }

    fn read(log: &Path) -> Option<QueueMade> {
        let text = std::fs::read_to_string(Self::file(log)).ok()?;
        let env = |name: &str| text.lines().find(|l| l.strip_prefix(name).is_some_and(|v| v.starts_with('='))).map(str::to_string);
        Some(QueueMade {
            why: text.lines().filter_map(|l| l.strip_prefix(WHY)).map(str::to_string).collect(),
            proven: env("GATE_PROVEN")?,
            retry: env("GATE_RETRY")?,
            times: env("GATE_TIMES")?,
            peaks: env("GATE_PEAKS")?,
            // (a run a master before these measures started: none, each unit on its slot's threads)
            jobs: env("GATE_JOBS").unwrap_or_else(|| "GATE_JOBS=".into()),
            // (one before the tests' measures: its parts dealt by sorted name, as then)
            tests: env("GATE_TEST_TIMES").unwrap_or_else(|| "GATE_TEST_TIMES=".into()),
            floor: text.lines().find_map(|l| l.strip_prefix(FLOOR)).map(str::to_string),
        })
    }

    fn write(&self, log: &Path) -> std::io::Result<()> {
        let mut text: String = self.why.iter().flat_map(|w| w.lines()).map(|w| format!("{WHY}{w}\n")).collect();
        for env in [&self.proven, &self.retry, &self.times, &self.peaks, &self.jobs, &self.tests] { text.push_str(&format!("{env}\n")); }
        if let Some(gb) = &self.floor { text.push_str(&format!("{FLOOR}{gb}\n")); }
        let temp = Self::file(log).with_extension("queue.tmp");
        std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, Self::file(log)))
    }
}

/// The run's [`QueueMade`]: an adopted run's as its start saved it, else `make`'s, saved for a
/// master that adopts it (one that adopts a run saved before it read its measures makes it now).
fn queue_made(log: &Path, adopted: bool, make: impl FnOnce() -> QueueMade) -> QueueMade {
    if let Some(made) = adopted.then(|| QueueMade::read(log)).flatten() { return made; }
    let made = make();
    if let Err(e) = made.write(log) {
        eprintln!("bucket {}: {}: {e} (a master restart before it ends deals its units by newer measures: no verdict)", bucket_of(log), QueueMade::file(log).display());
    }
    made
}

/// [`test_boxes`], once there is one: a run waits for a pool box as a lease waits for a node.
fn pool_boxes(name: &str) -> (usize, u32) {
    let mut said = false;
    loop {
        match test_boxes() {
            Ok(shape) => return shape,
            Err(e) if !said => {
                eprintln!("bucket {name}: waits for its boxes: {e}");
                said = true;
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}

/// The boxes the tests bucket deals its units over: the up pool boxes, dev on, that meet
/// [`gate_needs`] (the master's rustc), one shard each (a box with room takes a second beside it, in the one tree
/// they share at the run's pin); and the cores each shard takes, the most a test lease gets on
/// the smallest of them.
fn test_boxes() -> Result<(usize, u32), String> {
    let c = crate::cli::control()?;
    let nodes = crate::cli::ask(&c, None, "nodes", "nodes")?;
    // a shard's RAM by its class's measure, as the scheduler charges it ([`pool_shape`])
    let measured = |cores: u32| shard_ram_of(&crate::cli::ask(&c, None, "estimate", &shard_ram_ask(cores)).ok()?);
    pool_shape(&nodes, &crate::lease::caps::rustc_version(), &measured)
}

/// The master's `estimate` ask of a tests shard's class at `cores`: `<kind> <class>`. Asked as
/// `estimate test <class>`, its kind read `estimate`, no class matched, and the pool stayed sized
/// by the fixed 40 GB (11-18).
fn shard_ram_ask(cores: u32) -> String {
    format!("test {}", shard_class(cores, false))
}

/// The RAM charge in an `estimate` answer ([`crate::lease::master::estimate_answer`]).
fn shard_ram_of(answer: &[String]) -> Option<f64> {
    answer.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("ram_gb").and_then(crate::lease::num))
}

/// The most cores a tests shard asks for (three units at once, land_gate.sh TEST_CORES): a whole
/// box's 16, asked at background priority, waited behind lane work on every box (L564088 queued
/// 6 min and more, 11-07). 8 ran two at once; 12, three, is one measured run's try (11-07): back
/// to 8 when contention eats the third unit's gain.
const SHARD_CORES: u32 = 12;

/// The RAM a box needs per tests shard while its class has no measure ([`pool_shape`]): a 12-core
/// shard's three units at once peaked at 27-29 GB (11-07), charged that and a fifth
/// ([`shard_class`]), up to its box's whole budget. A 30 GB pool box never took one, and a 60 GB
/// box takes one at a time: a shard dealt beyond that ran only after a sibling's box freed, two
/// rounds, 40-60 min of wall.
const SHARD_RAM_GB: f64 = 40.0;

/// A tests shard's measured class, its shape (its cores) in it: a shard's wall and peaks come from
/// what it runs at once, and another shape's are wrong data for it. An 8-core shard's 37.9 GB peak
/// charged the 12-core shards 45.5 GB each, where they peaked at 27-29 GB, and one waited for RAM
/// on a box that held it (11-07). The full bucket's shards ([`FULL`]: every package's tests, no
/// proof reused) are a class of their own: theirs ran 26-39 min against the tests bucket's 9
/// (L767799-801), and in one class the tests bucket's next shards asked large, measured 27 min,
/// with 18-30 GB each, and waited 10 min for RAM (11-10).
fn shard_class(cores: u32, full: bool) -> String {
    format!("test bash land_gate.sh testrun{} {cores} cores @{}", if full { " full" } else { "" }, crate::lease::sched::TESTS_LANE)
}

/// [`test_boxes`] of the master's `nodes` answer: how many shards, and the cores each asks: their
/// boxes' least test cores (a box's shared CPUs when its lease.conf pins them, else its cores), at
/// most [`SHARD_CORES`]. A box takes as many shards as it has those cores and RAM for (nine
/// tenths of it, the scheduler's budget less its margin): a shard's RAM as the scheduler charges
/// it (`measured`: its class's measure at those cores), else [`SHARD_RAM_GB`]. Sized by the
/// fixed 40 GB while shards were charged about 20, the pool was three boxes, a 30 GB pool box
/// took none, and a tests run's third shard waited 30 min for the slot a full run held (11-18).
/// With `rustc` (the master's release), a box on another one takes none: its leases would wait.
fn pool_shape(nodes: &[String], rustc: &str, measured: &dyn Fn(u32) -> Option<f64>) -> Result<(usize, u32), String> {
    // each box: its test cores and its RAM
    let mut boxes: Vec<(u32, Option<f64>)> = Vec::new();
    for v in nodes.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        let yes = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        let Some(caps) = v.get("caps").filter(|_| yes("up") && yes("dev") && !yes("draining") && v.get("role").and_then(Value::as_str) == Some("pool")) else { continue };
        let text = |k: &str| caps.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        if text("os") != "linux" || !matches!(text("arch").as_str(), "x86_64" | "x64") { continue; }
        if !rustc.is_empty() && text("rustc") != rustc { continue; }
        let ram = caps.get("ram_gb").and_then(crate::lease::num);
        let cores = caps.get("cpus_shared").and_then(Value::as_u64).filter(|n| *n > 0).or_else(|| caps.get("cores").and_then(Value::as_u64)).unwrap_or(0) as u32;
        if cores == 0 { continue; }
        boxes.push((cores, ram));
    }
    let ask = boxes.iter().map(|(c, _)| *c).min().map(|least| least.min(SHARD_CORES)).ok_or_else(|| format!("no up pool box takes {GATE_NEEDS} leases"))?;
    let shard_gb = measured(ask).filter(|gb| *gb > 0.0).unwrap_or(SHARD_RAM_GB);
    // (a box that says no RAM takes one, as before it was measured)
    let n: usize = boxes.iter().map(|(c, ram)| (c / ask).min(ram.map_or(1, |g| (g * 0.9 / shard_gb) as u32)) as usize).sum();
    if n == 0 { return Err(format!("no up pool box has the RAM for a tests shard ({shard_gb:.1} GB)")); }
    Ok((n, ask))
}

/// Shard `i`'s log beside the run's (`tests-<ms>.log`: `tests-s<i>-<ms>.log`): named after the
/// run's start as a bucket log is, and no bucket run of its own to a restarted master.
fn shard_log(log: &Path, i: usize) -> PathBuf {
    let stem = log.file_stem().and_then(|s| s.to_str()).unwrap_or("tests");
    let (name, ms) = stem.rsplit_once('-').unwrap_or((stem, "0"));
    log.with_file_name(format!("{name}-s{i}-{ms}.log"))
}

/// Where a running run's output is: its shards' logs and its libraries sweep's as they grow. Its
/// own log holds only its shape until it ends and gathers them: a red's output read as missing
/// from a 12-byte run log (11-17).
fn run_logs(log: &Path) -> String {
    let shards = run_shape(log).map_or(0, |(n, _)| n);
    let live: Vec<String> = (1..=shards).map(|i| shard_log(log, i)).chain([libs_log(log)]).filter(|p| p.exists()).map(|p| p.display().to_string()).collect();
    if live.is_empty() {
        format!("log {}", log.display())
    } else {
        format!("logs {} (all in {} once it ends)", live.join(" "), log.display())
    }
}

/// Whether a tests run sweeps the libraries ([`libs_sweep`]): a full run always; a tests run when
/// its landings changed what the JIT reads (CI's own rule, [`crate::orch::ci::Inputs`]), or when
/// git cannot say. Two such landings in one run share its one sweep. And only when the run's own
/// land_gate.sh (`gate`, at its tip) has the sweep's mode ([`LIBS_MODE`]): a full run of a tip from
/// before it, started by a master after it, ran its sweep, and that tree's gate failed it
/// `bad-mode`, a red no landing caused (11-17).
fn sweeps(full: bool, jit_changed: Option<bool>, gate: Option<&str>) -> bool {
    gate.is_some_and(|g| g.contains(LIBS_MODE)) && (full || jit_changed.unwrap_or(true))
}

/// land_gate.sh's line that starts its libraries' sweep ([`sweeps`]).
const LIBS_MODE: &str = "if [ \"$mode\" = libs ]; then";

/// The run's land_gate.sh at its tip `work` commit, None when git cannot read it.
fn gate_at(repo: &Path, work: &str) -> Option<String> {
    crate::orch::git(repo, &["show", &format!("{work}:tools/run/fixture/land_gate.sh")]).ok()
}

/// The libraries' sweep's log beside the run's (`tests-<ms>.log`: `tests-libs-<ms>.log`), no bucket
/// run of its own to a restarted master, as a shard's ([`shard_log`]).
fn libs_log(log: &Path) -> PathBuf {
    let stem = log.file_stem().and_then(|s| s.to_str()).unwrap_or("tests");
    let (name, ms) = stem.rsplit_once('-').unwrap_or((stem, "0"));
    log.with_file_name(format!("{name}-libs-{ms}.log"))
}

/// The libraries' sweep's class: measured apart from the shards' ([`shard_class`]), one for both
/// buckets (the same checks).
fn libs_class() -> String {
    format!("test bash land_gate.sh libs @{}", crate::lease::sched::TESTS_LANE)
}

/// The libraries' sweep of a tests or full run ([`sweeps`]; land_gate.sh libs): `makepad check` of
/// every library of the root workspace, both repos, each its unit `check <pkg>`. `makepad check`
/// borrow-checks only the package it names, and the gate packages stand in for the rest: a relax
/// change that passed a 317-package corpus broke makepad-csg-sdf, outside it, and held Fab's and
/// Sandbox's suites until it was reverted (11-17). A lease of its own beside the run's shards, so
/// the run's wall stays its longest unit's (about 10 min cold on 12 cores: 317 of the libraries
/// took 368 s on 13, 11-16), and no pool slot: the shards hold them all, and one more would wait out
/// a shard's run. A tests-lane lease at the run's rev: it passes holds as the run's shards do, and
/// starts on what its box has free once it waited as long as a shard does.
fn libs_sweep(repo: &Path, log: &Path, rev: &str, since: &str, pins: (&Pin, &Pin)) -> (PathBuf, (Result<Option<i32>, String>, String)) {
    let (needs, cores, class) = (gate_needs(), SHARD_CORES.to_string(), libs_class());
    let lease = ["lease", "test", "--needs", needs.as_str(), "--cores", cores.as_str(), "--cost", "large", "--class", class.as_str(), "--priority", "background", "--lane", crate::lease::sched::TESTS_LANE, "--rev", rev, "--run-since", since];
    let gate = ["bash", "tools/run/fixture/land_gate.sh", "libs"];
    let at = libs_log(log);
    let run = lease_run(repo, &lease, &gate, &[], &at, pins, &|| true, &mut |_: Need| true, &|| false, &|| false);
    hold_run_jits(log, &run.1, &ask_box);
    (at, run)
}

/// The run's log: its shape ([`SHARDS`]), its why lines, then each shard's run.
fn merged(why: &[String], (shards, cores): (usize, u32), ran: &[(PathBuf, (Result<Option<i32>, String>, String))]) -> String {
    let mut text = format!("{SHARDS}{shards} {cores}\n");
    text.extend(why.iter().map(|l| format!("{l}\n")));
    for (i, (shard, (_, part))) in ran.iter().enumerate() {
        // a shard the toolchain changed under: its failures are no verdicts ([`unjudged`]), and
        // its units run once more
        let part = if part.lines().any(|l| l.starts_with("TOOLCHAIN CHANGED")) {
            part.lines().map(|l| match (l.strip_prefix("GATEUNIT fail "), l.strip_prefix("GATESTEP fail ")) {
                (Some(unit), _) => format!("GATEUNIT voided {unit}\n"),
                (_, Some(step)) => format!("GATESTEP voided {step}\n"),
                _ => format!("{l}\n"),
            }).collect::<String>()
        } else {
            part.clone()
        };
        text.push_str(&format!("== shard {}/{}: {}\n{part}", i + 1, ran.len(), shard.display()));
        if !part.ends_with('\n') { text.push('\n'); }
    }
    text
}

/// The run's status from its shards': a lease error, else a lost lease, else a failing exit,
/// else a run ended without one (a shard that exited 0 without its NODEGATE-OK too), else 0. A
/// shard the queue no longer needed ([`NOT_NEEDED`]) passes.
fn worst(ran: &[(PathBuf, (Result<Option<i32>, String>, String))]) -> Result<Option<i32>, String> {
    let said = |(status, part): &(Result<Option<i32>, String>, String)| match status {
        Ok(Some(0)) if !part.lines().any(|l| l.starts_with("NODEGATE-OK") || l == NOT_NEEDED) => Ok(None),
        other => other.clone(),
    };
    let all: Vec<Result<Option<i32>, String>> = ran.iter().map(|(_, r)| said(r)).collect();
    all.iter().find(|s| s.is_err()).cloned()
        .or_else(|| all.iter().find(|s| lost(s)).cloned())
        .or_else(|| all.iter().find(|s| matches!(s, Ok(Some(c)) if *c != 0)).cloned())
        .or_else(|| all.iter().find(|s| matches!(s, Ok(None))).cloned())
        .unwrap_or(Ok(Some(0)))
}

/// A tests run's shard's log line once it ended unneeded: the run's queue was dealt out before its
/// gate started (queued for its box, or in its setup), and the run's verdict waits for no box.
const NOT_NEEDED: &str = "bucket: the run's queue was dealt out before this shard's gate started: not needed";

/// How often the dealer looks at its shards' logs: each unit waits for its look once (474 units
/// over nine slots, a look of 0.25 s: 13 s of a run's 30 min).
const DEAL_LOOK: Duration = Duration::from_millis(250);

/// A tests run's one queue of units, dealt to its shards a unit at a time as each has a slot free
/// (land_gate.sh GATE_QUEUE: `GATEQUEUE <lease> <units> <digest>` once its queue is made, every
/// shard's the same, the slowest unit first; then `GATEWANT <lease> <n>` per free slot, answered in
/// the lease's `$DEVHUB_TOLD`, `<n> <index> <digest>` or `<n> -`: dealt out). A shard on a slow or
/// busy box takes fewer units: a deal fixed at the start left the run's wall to the shard beside
/// lane work, 1.20x its share (11-07). Each answer is in the run's `.log.dealt` before it is told,
/// so a restarted master deals on from it and the shards' logs.
#[derive(Default)]
struct Dealer {
    /// the queue's size and digest, as the first shard to make it said them
    queue: Option<(usize, String)>,
    /// each lease's queue digest (its GATEQUEUE): one not the run's is told none of the run's
    made: BTreeMap<String, String>,
    /// the queue's first unit never dealt
    next: usize,
    /// units of a lease that ended without its verdict, dealt again first (the slowest first)
    back: BTreeSet<usize>,
    /// each want's answer, by its lease and number: a unit's index, or none (dealt out)
    dealt: BTreeMap<(String, u32), Option<usize>>,
    /// the lease each dealt unit last went to
    owner: BTreeMap<usize, String>,
    /// each shard's lease now (its log's last GATEQUEUE) and whether it said its verdict
    shards: Vec<(Option<String>, bool)>,
    /// a full run's leases told to give their pool slots to a run that goes first, and of those
    /// the ones whose shard let it go ([`Dealer::take_yield`])
    yielding: BTreeSet<String>,
    gave: BTreeSet<String>,
}

/// An answer to a want: the lease, the line its run is told, and its `.log.dealt` record.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    lease: String,
    line: String,
    record: String,
}

impl Dealer {
    fn new(shards: usize) -> Dealer {
        Dealer { shards: vec![(None, false); shards], ..Dealer::default() }
    }

    /// The run's answers before a master's restart (its `.log.dealt`, `<lease> <n> <index|->`).
    fn replay(&mut self, text: &str) {
        for l in text.lines() {
            let [lease, n, what] = l.split_whitespace().collect::<Vec<_>>()[..] else { continue };
            let Ok(n) = n.parse::<u32>() else { continue };
            let index = what.parse::<usize>().ok();
            if let Some(i) = index { self.took(i, lease); }
            if what == YIELD { self.yielding.insert(lease.to_string()); }
            self.dealt.insert((lease.to_string(), n), index);
        }
    }

    fn took(&mut self, i: usize, lease: &str) {
        self.back.remove(&i);
        self.next = self.next.max(i + 1);
        self.owner.insert(i, lease.to_string());
    }

    /// The answer to `lease`'s `n`-th want: the queue's unit `index`, or none (`<n> -`; one of
    /// another queue hears the run's digest, `<n> - <digest>`).
    fn told(&self, lease: &str, n: u32, index: Option<usize>) -> Told {
        let digest = self.queue.as_ref().map_or("", |(_, d)| d.as_str());
        let (line, record) = match index {
            Some(i) => (format!("{n} {i} {digest}"), format!("{lease} {n} {i}")),
            None if self.other(lease) => (format!("{n} - {digest}"), format!("{lease} {n} -")),
            None => (format!("{n} -"), format!("{lease} {n} -")),
        };
        Told { lease: lease.to_string(), line, record }
    }

    /// Whether `lease` made another queue than the run's.
    fn other(&self, lease: &str) -> bool {
        matches!((&self.queue, self.made.get(lease)), (Some((_, run)), Some(its)) if its != run)
    }

    /// The units of a known queue never dealt and its size: none once it is dealt out.
    fn undealt(&self) -> Option<(usize, usize)> {
        let (units, _) = self.queue.as_ref()?;
        (self.next < *units).then(|| (*units - self.next, *units))
    }

    /// A line of shard `shard`'s log: a want not answered yet is answered. `waiting`: the pool
    /// slots runs that go first wait for ([`Board::short_before_full`], 0 but in a full run's): a
    /// unit to deal goes to no lease of a full run's while more of those wait than its shards are
    /// giving up already, and the lease is told it is dealt out (`<n> -`, recorded `<n> yield`).
    /// Its gate runs its last units and ends, and its shard asks for a slot again behind them; its
    /// queue goes on once a slot is free for it. A full run of an older tip held all three pool
    /// slots 2 h, and the tests run of the landings since, which catches their reds within the
    /// hour, waited 63 min of it (11-17). Its last working shard keeps its slot, the run's floor
    /// ([`Slots::floor`]): giving that too, as from 11-18 (a full run's one shard of 680 units had
    /// held its slot 2 h 39 min beside a tests run's two), starved the full run behind tests runs
    /// asked back to back (11-19).
    fn line(&mut self, shard: usize, line: &str, waiting: usize, floor: usize) -> Option<Told> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["GATEQUEUE", lease, units, digest] => {
                let units = units.parse().ok()?;
                if self.queue.is_none() { self.queue = Some((units, digest.to_string())); }
                self.made.insert(lease.to_string(), digest.to_string());
                *self.shards.get_mut(shard)? = (Some(lease.to_string()), false);
                None
            }
            ["GATEWANT", lease, n] => {
                let n: u32 = n.parse().ok()?;
                let units = self.queue.as_ref()?.0;
                if self.dealt.contains_key(&(lease.to_string(), n)) { return None; }
                // a lease that made another queue (its measures read after a master restart, the
                // run's before it) runs none of the run's: its index would name another unit
                if self.other(lease) {
                    self.dealt.insert((lease.to_string(), n), None);
                    return Some(self.told(lease, n, None));
                }
                if self.gives(lease, units, waiting, floor) {
                    self.yielding.insert(lease.to_string());
                    self.dealt.insert((lease.to_string(), n), None);
                    return Some(Told { lease: lease.to_string(), line: format!("{n} -"), record: format!("{lease} {n} {YIELD}") });
                }
                let index = self.back.pop_first().or_else(|| (self.next < units).then_some(self.next));
                if let Some(i) = index { self.took(i, lease); }
                self.dealt.insert((lease.to_string(), n), index);
                Some(self.told(lease, n, index))
            }
            // its box is wanted for timed work (a bench box lent between timed series: its
            // $DEVHUB_YIELD file): its gate takes no more units, and once its last ones end its
            // shard gives its slot up and asks for a lease again, as one told to give it up does
            ["GATEYIELD", lease] => {
                self.yielding.insert(lease.to_string());
                None
            }
            _ if line.starts_with("NODEGATE-OK") || line.starts_with("NODEGATE-FAIL") => {
                if let Some((_, verdict)) = self.shards.get_mut(shard) { *verdict = true; }
                None
            }
            // (a part that gave its slot up ended as a lost one does: its units back unless judged)
            _ if RERUN_LINES.contains(&line) || line == YIELDED => {
                self.over(shard);
                None
            }
            _ => None,
        }
    }

    /// Shard `shard`'s lease ended (its run did, or its log runs it again): one that said no verdict
    /// ran its units for nothing (their lines come at its end), so the ones it was the last to get
    /// are dealt again first.
    fn over(&mut self, shard: usize) {
        let Some((Some(lease), verdict)) = self.shards.get_mut(shard).map(std::mem::take) else { return };
        if !verdict {
            self.back.extend(self.owner.iter().filter(|(_, l)| **l == lease).map(|(i, _)| *i));
        }
    }

    /// Units left to deal, or none known yet: a shard whose gate has not started is still needed.
    fn needed(&self) -> bool {
        self.queue.as_ref().is_none_or(|(units, _)| self.next < *units) || !self.back.is_empty()
    }

    /// Whether `lease` gives its slot up rather than take a unit ([`Dealer::line`]): one told to
    /// already does; else while units are left and more slots are waited for than the run's shards
    /// are giving up (told to, their slot not let go yet), but never its last working shard: the
    /// run's floor ([`Slots::floor`]).
    fn gives(&self, lease: &str, units: usize, waiting: usize, floor: usize) -> bool {
        if self.yielding.contains(lease) { return true; }
        let current = |l: &Option<String>| l.as_ref().is_some_and(|l| l == lease);
        if waiting == 0 || !self.shards.iter().any(|(l, _)| current(l)) || !(self.next < units || !self.back.is_empty()) { return false; }
        let giving = self.shards.iter().filter_map(|(l, _)| l.as_ref()).filter(|l| self.yielding.contains(*l) && !self.gave.contains(*l)).count();
        let working = self.shards.iter().filter(|(l, said)| !said && l.as_ref().is_some_and(|l| !self.yielding.contains(l))).count();
        waiting > giving && working > floor
    }

    /// Whether shard `shard`'s lease now was told to give its slot up and its shard has not let it
    /// go yet: true once, as it does ([`lease_run`]'s `more`).
    fn take_yield(&mut self, shard: usize) -> bool {
        let Some(Some(lease)) = self.shards.get(shard).map(|(l, _)| l.clone()) else { return false };
        self.yielding.contains(&lease) && self.gave.insert(lease)
    }

    /// The answers of the shards' leases now: told again by a restarted master (a run reads its
    /// first answer to a want, so one told twice is harmless), the old master's last unsent too.
    fn answers_now(&self) -> Vec<Told> {
        let live: BTreeSet<&str> = self.shards.iter().filter_map(|(l, _)| l.as_deref()).collect();
        self.dealt.iter().filter(|((lease, _), _)| live.contains(lease.as_str())).map(|((lease, n), index)| self.told(lease, *n, *index)).collect()
    }

    fn live(&self, lease: &str) -> bool {
        self.shards.iter().any(|(l, _)| l.as_deref() == Some(lease))
    }
}

/// The lines a log grew by since `seen` (its offset and its unfinished last line); a log that
/// shrank was made again, read from its start.
fn grown(path: &Path, seen: &mut (u64, String)) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    if len < seen.0 { *seen = (0, String::new()); }
    let mut more = Vec::new();
    if f.seek(SeekFrom::Start(seen.0)).is_err() || f.read_to_end(&mut more).is_err() { return Vec::new(); }
    seen.0 += more.len() as u64;
    seen.1.push_str(&String::from_utf8_lossy(&more));
    let Some(cut) = seen.1.rfind('\n') else { return Vec::new() };
    let rest = seen.1.split_off(cut + 1);
    std::mem::replace(&mut seen.1, rest).lines().map(str::to_string).collect()
}

/// Says a shard's run ended when its thread does, a panic too: the dealer waits for them all.
struct Ends<'a>(&'a AtomicBool);

impl Drop for Ends<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The run's dealer ([`Dealer`]) while its shards run: each shard's log read as it grows, each want
/// answered through its lease's node ([`crate::lease::client::tell`]), its record in the run's
/// `.log.dealt` first. An answer not recorded or not told goes again at the next look, until its
/// lease ends. A unit its watch says stalled is a note for main as it is said ([`stall_note`]), once
/// per stall (its key in the run's `.log.stalls`, which a master that adopts the run reads).
fn deal(dealer: &Mutex<Dealer>, ended: &[AtomicBool], log: &Path, home: &Path) {
    let lock = || dealer.lock().unwrap_or_else(|e| e.into_inner());
    let dealt = log.with_extension("log.dealt");
    let earlier = std::fs::read_to_string(&dealt).unwrap_or_default();
    lock().replay(&earlier);
    let mut seen: Vec<(u64, String)> = vec![(0, String::new()); ended.len()];
    let stalls = log.with_extension("log.stalls");
    let mut noted: BTreeSet<String> = std::fs::read_to_string(&stalls).unwrap_or_default().lines().map(str::to_string).collect();
    let mut leases: Vec<Option<String>> = vec![None; ended.len()];
    // answers not told yet, and whether each is recorded
    let mut unsent: Vec<(Told, bool)> = Vec::new();
    let mut adopted = !earlier.is_empty();
    let mut client = None;
    let mut said = BTreeSet::new();
    let full = bucket_of(log) == FULL;
    loop {
        let last = ended.iter().all(|e| e.load(Ordering::Acquire));
        for (i, end) in ended.iter().enumerate() {
            let over = end.load(Ordering::Acquire);
            let lines = grown(&shard_log(log, i + 1), &mut seen[i]);
            for l in &lines {
                if let Some(id) = l.strip_prefix("GATEQUEUE ").and_then(|r| r.split_whitespace().next()).or_else(|| granted(l)) {
                    leases[i] = Some(id.to_string());
                }
                let Some((key, note)) = stall_note(log, i + 1, leases[i].as_deref(), l, wall_ms() / 1000) else { continue };
                if !noted.insert(key.clone()) { continue; }
                let kept = std::fs::OpenOptions::new().create(true).append(true).open(&stalls).and_then(|mut f| writeln!(f, "{key}"));
                if let Err(e) = kept.and_then(|_| crate::notes::leave(home, "main", &note)) {
                    if said.insert(format!("stall {e}")) { eprintln!("bucket tests: a stall's note for main: {e}"); }
                }
            }
            let (waiting, floor) = if full && !lines.is_empty() { (BOARD.short_before_full(), BOARD.floor_n(log)) } else { (0, 1) };
            let mut d = lock();
            unsent.extend(lines.iter().filter_map(|l| d.line(i, l, waiting, floor)).map(|t| (t, false)));
            if over { d.over(i); }
        }
        if std::mem::take(&mut adopted) {
            unsent.extend(lock().answers_now().into_iter().map(|t| (t, true)));
        }
        unsent.retain_mut(|(t, recorded)| {
            if !lock().live(&t.lease) { return false; }
            if !*recorded {
                let wrote = std::fs::OpenOptions::new().create(true).append(true).open(&dealt).and_then(|mut f| writeln!(f, "{}", t.record));
                if let Err(e) = wrote {
                    if said.insert(format!("record {e}")) { eprintln!("bucket tests: its queue's record {}: {e}", dealt.display()); }
                    return true;
                }
                *recorded = true;
            }
            let c = match client.take() {
                Some(c) => c,
                None => match crate::cli::control() {
                    Ok(c) => c,
                    Err(e) => {
                        if said.insert(format!("control {e}")) { eprintln!("bucket tests: its queue's dealer: {e}"); }
                        return true;
                    }
                },
            };
            match crate::lease::client::tell(&c, &t.lease, &t.line) {
                Ok(()) => {
                    client = Some(c);
                    false
                }
                Err(e) => {
                    if said.insert(format!("{} {e}", t.lease)) { eprintln!("bucket tests: telling {} its unit: {e}", t.lease); }
                    true
                }
            }
        });
        if last { return; }
        std::thread::sleep(DEAL_LOOK);
    }
}

/// The runs the tests bucket's units' last measures come from (land_gate.sh's GATETIME and
/// GATEPEAK lines, kept in each row's `unit_s` and `unit_mb`).
const TIMES_ROWS: usize = 30;

/// A run's units' measures by land_gate.sh's `<what> <unit> <n>` lines.
fn unit_measures(text: &str, what: &str) -> BTreeMap<String, u64> {
    text.lines().filter_map(|l| {
        let (unit, n) = l.strip_prefix(what)?.strip_prefix(' ')?.rsplit_once(' ')?;
        Some((unit.to_string(), n.trim().parse().ok()?))
    }).collect()
}

/// A run's units' seconds (land_gate.sh's `GATETIME <unit> <s>`).
fn unit_secs(text: &str) -> BTreeMap<String, u64> {
    unit_measures(text, "GATETIME")
}

/// A run's units' peak RSS in MB (land_gate.sh's `GATEPEAK <unit> <MB>`: the most its processes
/// held at once, their RSS summed at its watch's looks or one's own peak between them).
fn unit_mb(text: &str) -> BTreeMap<String, u64> {
    unit_measures(text, "GATEPEAK")
}

/// A run's parts' slowest tests (land_gate.sh's `GATETESTS <unit> :: <name>=<s>/<MB> ..`, its
/// test_times): each test's time and the MB its part held while it ran, slowest first.
fn unit_tests(text: &str) -> BTreeMap<String, Vec<(String, f64, u64)>> {
    text.lines().filter_map(|l| {
        let (unit, tests) = l.strip_prefix("GATETESTS ")?.split_once(" :: ")?;
        let tests = tests.split_whitespace().filter_map(|t| {
            let (name, at) = t.rsplit_once('=')?;
            let (secs, mb) = at.split_once('/')?;
            Some((name.to_string(), secs.parse().ok()?, mb.parse().ok()?))
        }).collect();
        Some((unit.to_string(), tests))
    }).collect()
}

/// The tests that ran at a part's highest look ([`unit_peak_tests`]).
#[derive(Clone, Debug, PartialEq)]
struct PeakTests {
    /// what its processes held then, in MB, and when, in s into the part
    mb: u64,
    at_s: u64,
    tests: Vec<String>,
}

/// A run's parts' highest looks and the tests that ran then (land_gate.sh's `GATEPEAKTESTS <unit>
/// :: <MB> <s in> <name>..`).
fn unit_peak_tests(text: &str) -> BTreeMap<String, PeakTests> {
    text.lines().filter_map(|l| {
        let (unit, rest) = l.strip_prefix("GATEPEAKTESTS ")?.split_once(" :: ")?;
        let mut words = rest.split_whitespace();
        let (mb, at_s) = (words.next()?.parse().ok()?, words.next()?.parse().ok()?);
        Some((unit.to_string(), PeakTests { mb, at_s, tests: words.map(str::to_string).collect() }))
    }).collect()
}

/// A row's slowest unit with timed tests, as `bucket status` says it: `<unit> (<s> s, <GB> GB):
/// its slowest a 312.4 s, b 120.1 s, c 90.0 s; at its highest look (13.2 GB) ran a, b`. None when no
/// unit of it timed its tests.
pub fn slowest_tests(row: &Value) -> Option<String> {
    let Some(Value::Obj(units)) = row.get("unit_tests") else { return None };
    let secs = |u: &str| row.get("unit_s").and_then(|t| t.get(u)).and_then(Value::as_u64).unwrap_or(0);
    let (unit, tests) = units.iter().filter(|(_, t)| t.as_arr().is_some_and(|a| !a.is_empty())).max_by_key(|(u, _)| secs(u))?;
    let said: Vec<String> = tests.as_arr().unwrap_or_default().iter().take(3).filter_map(|t| {
        let t = t.as_arr()?;
        Some(format!("{} {:.1} s", t.first()?.as_str()?, t.get(1).and_then(crate::lease::num)?))
    }).collect();
    let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
    let peak = row.get("unit_peak_tests").and_then(|p| p.get(unit)).map(|p| {
        let ran: Vec<&str> = p.get("tests").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).collect();
        format!("; at its highest look ({}) ran {}", gb(p.get("mb").and_then(Value::as_u64).unwrap_or(0)), if ran.is_empty() { "no timed test".to_string() } else { ran.join(", ") })
    }).unwrap_or_default();
    let mb = row.get("unit_mb").and_then(|t| t.get(unit)).and_then(Value::as_u64).unwrap_or(0);
    Some(format!("{unit} ({} s, {}): its slowest {}{peak}", secs(unit), gb(mb), said.join(", ")))
}

/// Each unit's last measured seconds in the bucket's last [`TIMES_ROWS`] runs, as land_gate.sh's
/// GATE_TIMES (`<unit>=<s>` joined by `;`), which its shards deal the slowest out by first.
/// `rows`: newest first.
fn times_env(rows: &[Value]) -> String {
    format!("GATE_TIMES={}", last_measures(rows, "unit_s").iter().map(|(u, s)| format!("{u}={s}")).collect::<Vec<_>>().join(";"))
}

/// Each unit's last measured peak RSS (MB) in the bucket's last [`TIMES_ROWS`] runs, as
/// land_gate.sh's GATE_PEAKS (`<unit>=<MB>` joined by `;`): a shard starts a unit it was dealt
/// once its running units' peaks and that one's fit the RAM its lease got (`DEVHUB_RAM_MB`).
fn peaks_env(rows: &[Value]) -> String {
    format!("GATE_PEAKS={}", last_measures(rows, "unit_mb").iter().map(|(u, mb)| format!("{u}={mb}")).collect::<Vec<_>>().join(";"))
}

/// A run's units' threads (land_gate.sh's `GATEJOBS <unit> <n>`: its RUST_TEST_THREADS, RELAX_JOBS
/// and MAKEPAD_CPU_CORES), which its seconds and peak in the same run were measured at.
fn unit_jobs(text: &str) -> BTreeMap<String, u64> {
    unit_measures(text, "GATEJOBS")
}

/// Each unit's threads in the bucket's last [`TIMES_ROWS`] runs, as land_gate.sh's GATE_JOBS
/// (`<unit>=<n>` joined by `;`): a shard reads a unit's measured seconds and peak by them (its
/// work in thread-seconds, its RAM a thread) and gives a long unit more threads within its RAM
/// (unit_slots). A unit's are its newest measure's: each run that timed it said its threads.
fn jobs_env(rows: &[Value]) -> String {
    format!("GATE_JOBS={}", last_measures(rows, "unit_jobs").iter().map(|(u, n)| format!("{u}={n}")).collect::<Vec<_>>().join(";"))
}

/// Each split target's tests' newest measures in the bucket's last [`TIMES_ROWS`] runs (its parts'
/// GATETESTS, [`unit_tests`]), as land_gate.sh's GATE_TEST_TIMES (`<target> :: <name>=<s> ..`
/// joined by `;`): its deal_part deals the target's parts by them, the slowest test first to the
/// part with the fewest seconds. Dealt by sorted name, part 1 of the Fab bin's five held its
/// slowest tests and ran 17 min beside parts of 3-4 min (11-18).
fn test_times_env(rows: &[Value]) -> String {
    let mut targets: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for row in rows.iter().take(TIMES_ROWS) {
        let Some(Value::Obj(units)) = row.get("unit_tests") else { continue };
        for (unit, tests) in units {
            let target = unit.rsplit_once(" part ").map_or(unit.as_str(), |(target, _)| target);
            let measured = targets.entry(target.to_string()).or_default();
            for t in tests.as_arr().unwrap_or_default().iter().filter_map(Value::as_arr) {
                let (Some(name), Some(secs)) = (t.first().and_then(Value::as_str), t.get(1).and_then(crate::lease::num)) else { continue };
                measured.entry(name.to_string()).or_insert(secs);
            }
        }
    }
    let groups: Vec<String> = targets.iter().filter(|(_, tests)| !tests.is_empty())
        .map(|(target, tests)| format!("{target} :: {}", tests.iter().map(|(name, secs)| format!("{name}={secs:.1}")).collect::<Vec<_>>().join(" ")))
        .collect();
    format!("GATE_TEST_TIMES={}", groups.join(";"))
}

/// A tests shard's RAM floor (`--ram-floor-gb`, GB): the largest of its units' last peaks
/// ([`peaks_env`]), the most one unit dealt to it alone needs; none before any is measured.
fn ram_floor(rows: &[Value]) -> Option<String> {
    last_measures(rows, "unit_mb").values().max().map(|mb| format!("{:.1}", *mb as f64 / 1024.0))
}

/// Each unit's newest measure of a row's `field` in the bucket's last [`TIMES_ROWS`] runs
/// (`rows`: newest first).
fn last_measures(rows: &[Value], field: &str) -> BTreeMap<String, u64> {
    let mut measures: BTreeMap<String, u64> = BTreeMap::new();
    for row in rows.iter().take(TIMES_ROWS) {
        if let Some(Value::Obj(units)) = row.get(field) {
            for (unit, n) in units {
                if let Some(n) = n.as_u64() { measures.entry(unit.clone()).or_insert(n); }
            }
        }
    }
    measures
}

/// How many times its last measure a tests unit runs before the check-in lists it ([`slow_units`]).
const SLOW_TIMES: u64 = 3;
/// And how long at least: a unit measured at seconds is no news at three times them.
const SLOW_FLOOR_S: u64 = 10 * 60;
/// On a box whose load is over [`OVER_LOAD`] times its cores, a unit past this many times its
/// measure is listed whatever its seconds, as the box oversubscribed. Its cores: a full run's
/// `makepad-ai-vulkan` ran 14 min against its 37 s on a box at load 23 of 16 cores, and its line
/// blamed its watch, the box said nowhere (two times its cores, 11-12).
const OVER_TIMES: u64 = 5;
const OVER_LOAD: f64 = 1.0;

/// A tests shard's box as the check-in sees it ([`slow_units`]): its node, whether its hub
/// answers, its load and cores.
pub struct ShardBox {
    pub node: String,
    pub up: bool,
    pub load: f64,
    pub cores: u32,
}

/// For the manager's check-in (`lane quiet`): the units of the tests run in flight that run past
/// [`SLOW_TIMES`] their last measure (and [`SLOW_FLOOR_S`]), by the live `GATESTART` and `GATEEND`
/// lines land_gate.sh's at_once says in each shard's log, each with what its box says
/// (`shard_box`: by the shard's lease, None once it ended): a hub that does not answer, a load
/// past [`OVER_LOAD`] times its cores (a unit past [`OVER_TIMES`] its measure there is listed
/// whatever its seconds), else where to look for a hang. A unit that hangs busy is no HUNG
/// (land_gate.sh's watch stops a unit only when its tests make no progress for 9 min, or for its
/// measure once past twice it, never one at work), so this list is what says it: two render tests
/// polled stalled GStreamer pipelines at 4 % of a core for 73 min and held their shard at 98 min
/// while the others took 21 to 33, and nothing said so (11-07). Three units said 27x their measure on a box whose hub
/// had stopped answering, read as the box oversubscribed, while the other shards' units ran at
/// 1.0x theirs (11-07).
/// A unit whose tests gave no new result far past its measure (the watch's `GATEBUSY`: a spin,
/// which its watch never stops) is said as that instead, with the stack sample the watch took
/// then: six threads in `while !go` and a test thread spinning in a budget callback would have
/// held a shard and its box for ever with nothing said (11-09). It is a report, not a cap: main
/// or the unit's owner decides. The full bucket's shards are said as the tests bucket's are: a
/// test of makepad-xr stalled busy in one at 18:12, held the run's last pool slot for 3 h, and
/// the next tests run's third shard waited behind it, with nothing said (11-12).
pub fn slow_units(home: &Path, now_ms: u64, shard_box: &dyn Fn(&str) -> Option<ShardBox>) -> Vec<String> {
    let logs = home.join("bucket-logs");
    let tests = last_measures(&rows_of(home, "tests"), "unit_s");
    let mut out = Vec::new();
    for name in SHARDED {
        let rows = rows_of(home, name);
        // the newest run with no row yet: the bucket runs one at a time, and a run's sidecar stays
        // after its verdict
        let Some(ms) = std::fs::read_dir(&logs).ok().into_iter().flatten().filter_map(|entry| {
            entry.ok()?.file_name().to_str()?.strip_prefix(&format!("{name}-"))?.strip_suffix(".log.run")?.parse::<u64>().ok()
        }).max().filter(|ms| !rows.iter().any(|r| r.get("started_ms").and_then(Value::as_u64) == Some(*ms))) else { continue };
        // the full bucket runs once a day: a unit it has no measure of takes the tests bucket's
        let mut measures = last_measures(&rows, "unit_s");
        for (unit, m) in &tests { measures.entry(unit.clone()).or_insert(*m); }
        out.extend(slow_in_run(&logs.join(format!("{name}-{ms}.log")), name, ms, &measures, now_ms, shard_box));
    }
    out
}

/// [`slow_units`] of one run of bucket `name`, by its shards' logs.
fn slow_in_run(log: &Path, name: &str, ms: u64, measures: &BTreeMap<String, u64>, now_ms: u64, shard_box: &dyn Fn(&str) -> Option<ShardBox>) -> Vec<String> {
    let mut out = Vec::new();
    for i in 1.. {
        let Ok(text) = std::fs::read_to_string(shard_log(log, i)) else { break };
        let (lease, running, busy) = running_in(&text);
        let at = match lease {
            Some(lease) => match shard_box(lease) { Some(b) => Some((lease, b)), None => continue },
            None => None,
        };
        let over = at.as_ref().is_some_and(|(_, b)| b.up && b.load > OVER_LOAD * f64::from(b.cores));
        let who = at.as_ref().map_or("its lease".to_string(), |(lease, b)| format!("{lease} on {}", b.node));
        for (unit, b) in &busy {
            let measured = measures.get(*unit).map_or("not measured yet".to_string(), |m| format!("measured {}", took(*m)));
            let keep = shard_log(log, i).with_extension("keep");
            let get = at.as_ref().map_or(String::new(), |(lease, _)| format!("`devhub get {} tmp/{}/{} <dir> --lease {lease}` now, ", crate::lease::sched::TESTS_LANE, crate::lease::KEEP_DIR, b.sample));
            let says = if b.says.is_empty() { String::new() } else { format!(": {}", b.says) };
            out.push(format!("{name} run {ms} shard {i} ({who}): `{unit}` busy, no new result for {} ({measured}); nothing stops it: main or its owner decides. Its stack sample: {get}{} once its lease ends{says}", took((now_ms / 1000).saturating_sub(b.since)), keep.join(b.sample).display()));
        }
        for (unit, ran, measure) in slow_in(&running, measures, now_ms / 1000, over) {
            if busy.contains_key(unit.as_str()) { continue; }
            let why = match &at {
                Some((_, b)) if !b.up => format!("{} does not answer: the seconds are its silence, not the unit's (its lease is lost if it stays so, and the shard runs again)", b.node),
                Some((_, b)) if over => format!("{} is oversubscribed: load {:.0} on {} cores", b.node, b.load, b.cores),
                _ => {
                    let node = at.as_ref().map_or("<node>", |(_, b)| b.node.as_str());
                    format!("its watch fails it HUNG, its state kept, once its tests make no progress for its measure past twice it, else for 9 min (no result, no I/O, and no CPU but a spin in a driver; never while it builds or works): `devhub top {node}` shows its processes, `devhub inspect {node}/<pid> why-hung` where they wait")
                }
            };
            out.push(format!("{name} run {ms} shard {i} ({who}): `{unit}` running {} against its measured {} ({}x); {why}", took(ran), took(measure), ran / measure.max(1)));
        }
    }
    out
}

/// A unit its watch said stalled (`line`: unit_watch_body.sh's `GATEBUSY <unit> :: <since> <sample>
/// <says>`, since > 0) on shard `shard` of run `log`, held by `lease`, as a note for main: the
/// stall's key (shard, unit, since) and its line. The check-in lists such a unit with its sample
/// ([`slow_units`]), but nobody was told: a full run's `ui_semantics` (measured 30 s) sat 48 min in
/// a JIT whose owner waited on its own build, its sample taken at 10 min, its shard and pool slot
/// held, until main read the board (11-19). Nothing stops it here either: main or its owner decides.
fn stall_note(log: &Path, shard: usize, lease: Option<&str>, line: &str, now_s: u64) -> Option<(String, String)> {
    let (unit, said) = line.strip_prefix("GATEBUSY ")?.split_once(" :: ")?;
    let mut w = said.splitn(3, ' ');
    let (since, sample) = (w.next()?.parse::<u64>().ok().filter(|s| *s > 0)?, w.next()?);
    let says = w.next().map(str::trim).filter(|s| !s.is_empty()).map_or(String::new(), |s| format!(": {s}"));
    let keep = shard_log(log, shard).with_extension("keep").join(sample);
    let (get, who) = match lease {
        Some(lease) => (format!("`devhub get {} tmp/{}/{sample} <dir> --lease {lease}` now, ", crate::lease::sched::TESTS_LANE, crate::lease::KEEP_DIR), lease),
        None => (String::new(), "its lease"),
    };
    let run = log_started(log).map_or_else(String::new, |ms| format!(" run {ms}"));
    Some((format!("{shard} {unit} {since}"), format!("bucket {}{run} shard {shard} ({who}): `{unit}` stalled, no new test result for {}; nothing stops it: you or its owner decide. Its state: {get}{} once its lease ends{says}",
        bucket_of(log), took(now_s.saturating_sub(since)), keep.display())))
}

/// A running unit its watch said stalled busy (unit_watch_body.sh's `GATEBUSY <unit> :: <since>
/// <sample> <says>`): since when (epoch s) its tests gave no new result, its stack sample's path
/// in the shard's keep dir, and what the sample says (the JIT's cause, the busiest thread).
#[derive(Debug, PartialEq)]
struct Busy<'a> {
    since: u64,
    sample: &'a str,
    says: &'a str,
}

/// A shard's log's lease (its last `GATEQUEUE <lease> ..`, or its lease's grant line
/// `L<n> <kind>@<node>: granted` for a shard that deals no queue: a shard that lost its lease runs
/// again on a new one, and the lost one's units ended with it, as they do with a `LEASE LOST` or
/// `LEASE CANCELLED`), the units it started since (`GATESTART <unit> <epoch s>`) and did not end
/// (`GATEEND <unit>`), by when each started, and those of them its watch said stalled busy
/// ([`Busy`]; `GATEBUSY <unit> :: 0`: a new result since). Read by its queue line alone, a
/// run's one shard (no dealt queue, so no `GATEQUEUE`) was said "shard 4 (its lease)": its lease
/// went unnamed and unchecked, and the lease cancelled for it was another run's shard its log
/// named as holding a tree (L1375670, L1369106, 11-18).
/// Whether a shard's log names a grant of its lease ([`granted`]): read to the first one.
fn ever_granted(log: &Path) -> bool {
    use std::io::BufRead;
    std::fs::File::open(log).is_ok_and(|f| std::io::BufReader::new(f).lines().map_while(Result::ok).any(|l| granted(&l).is_some()))
}

/// The lease a log's grant line names (`L7 test@box-a: granted, setting up`).
fn granted(l: &str) -> Option<&str> {
    let (id, rest) = l.split_once(' ')?;
    let (at, said) = rest.split_once(": ")?;
    (id.len() > 1 && id.starts_with('L') && id[1..].bytes().all(|b| b.is_ascii_digit()) && at.contains('@') && !at.contains(' ') && said.starts_with("granted")).then_some(id)
}

fn running_in(text: &str) -> (Option<&str>, BTreeMap<&str, u64>, BTreeMap<&str, Busy<'_>>) {
    let (mut lease, mut running, mut busy) = (None, BTreeMap::new(), BTreeMap::new());
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("GATEQUEUE ") {
            lease = rest.split_whitespace().next();
            running.clear();
            busy.clear();
        } else if let Some(id) = granted(l) {
            lease = Some(id);
            running.clear();
            busy.clear();
        } else if l.starts_with("LEASE LOST") || l.starts_with("LEASE CANCELLED") {
            lease = None;
            running.clear();
            busy.clear();
        } else if let Some((unit, at)) = l.strip_prefix("GATESTART ").and_then(|r| r.rsplit_once(' ')) {
            if let Ok(at) = at.trim().parse() { running.insert(unit, at); }
        } else if let Some(unit) = l.strip_prefix("GATEEND ") {
            running.remove(unit.trim_end());
            busy.remove(unit.trim_end());
        } else if let Some((unit, said)) = l.strip_prefix("GATEBUSY ").and_then(|r| r.split_once(" :: ")) {
            let mut w = said.splitn(3, ' ');
            match (w.next().and_then(|n| n.parse::<u64>().ok()), w.next()) {
                (Some(since), Some(sample)) if since > 0 && running.contains_key(unit) => {
                    busy.insert(unit, Busy { since, sample, says: w.next().unwrap_or("").trim() });
                }
                _ => { busy.remove(unit); }
            }
        }
    }
    (lease, running, busy)
}

/// The running units ([`running_in`]) the check-in lists by `now_s`: past [`SLOW_TIMES`] their
/// measure and [`SLOW_FLOOR_S`], or on a box oversubscribed (`over`) past [`OVER_TIMES`] it: each
/// unit, its seconds, its measure. A unit not measured yet is not judged.
fn slow_in(running: &BTreeMap<&str, u64>, measures: &BTreeMap<String, u64>, now_s: u64, over: bool) -> Vec<(String, u64, u64)> {
    running.iter().filter_map(|(unit, at)| {
        let (ran, measure) = (now_s.saturating_sub(*at), *measures.get(*unit)?);
        let slow = ran >= SLOW_FLOOR_S && ran >= SLOW_TIMES * measure;
        (slow || (over && ran >= OVER_TIMES * measure.max(1))).then(|| (unit.to_string(), ran, measure))
    }).collect()
}

/// Seconds as a check-in says them.
fn took(s: u64) -> String {
    if s >= 120 { format!("{} min", s / 60) } else { format!("{s} s") }
}

/// Boots the apps the landings touched on a Linux node's warm JIT (one that presents), right
/// after each landing (Linux only, by its needs: its runs compare with the Linux ones before).
pub fn hot(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let base = base.expect("hot bucket needs a base");
        let retry = retry_env(&rows_of(&home, "hot"));
        // a warm boot of one or two apps needs far less than a test lease's default 6 cores; its
        // RAM is its class's measured peak (a fixed 5 GB ask was a cap its guards outgrew, 11-13)
        let needs = format!("{GATE_NEEDS},gui=1");
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let lease = ["lease", "test", "--needs", &needs, "--cost", "small", "--cores", "3", "--priority", "interactive", "--lane", "landq-hot", "--rev", &rev];
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &retry, &nested, "bash", "tools/run/fixture/land_gate.sh", "hot", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
        verdict(status, &text, log)
    })
}

/// Runs the test targets only a Mac can fail ([package.metadata.makepad.macos-tests]) that the
/// landings reach, on a Mac's lease under its own JIT (tools/run/fixture/mac_tests.sh: the Mac's
/// bash runs no land_gate.sh). The Linux buckets build them empty or never reach what they guard: a
/// menu-bar item's restart guard (JITPERF) had no post-land run (11-17). Then the boundary audits
/// (`boundary <app>` units, as the fast bucket's) of the apps the landings reach: only a Mac lowers
/// their Apple-only provided code (26 violations no Linux audit saw, 11-24). A small background
/// lease beside the Mac's other work; with nothing reached it ends at once, before any test.
pub fn mac(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        // (a first run with no landing in the last hour to diff from: nothing to say yet)
        let Some(base) = base else { return no_verdict("no base to read the landings' reach from".into()) };
        let retry = retry_env(&rows_of(&home, "mac"));
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let lease = ["lease", "test", "--needs", MAC_NEEDS, "--cost", "small", "--cores", "2", "--priority", "background", "--lane", "landq-mac", "--class", MAC_CLASS, "--rev", &rev];
        let (bases, nested) = (base.pair(), nested_bases(base, tip));
        let gate = ["env", &retry, &nested, "bash", "tools/run/fixture/mac_tests.sh", &bases.work, &bases.main];
        let (status, text) = lease_run(&repo, &lease, &gate, &[], log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
        verdict(status, &text, log)
    })
}

/// The mac bucket's box: a Mac with its display (a menu-bar item or a Metal device needs its session).
const MAC_NEEDS: &str = "os=macos,gui=1";

/// The mac bucket's class, one for all its runs: by its command's words each run's base sha named a
/// class of its own (`test bash mac_tests.sh <sha>`), so none was measured, its RAM nor the JIT build
/// a Mac without the tip's JIT runs first (11-17).
const MAC_CLASS: &str = "test bash mac_tests.sh @landq-mac";

/// The windows bucket's name ([`windows`]).
pub const WINDOWS: &str = "windows";

/// The windows bucket's class, one for all its runs (its command names its packages).
const WINDOWS_CLASS: &str = "build cargo check @landq-windows";

/// Checks the packages a Windows box checks ([`windows_reach`]) that the landings reach, and those
/// still red there, with one plain `cargo check` on a Windows box's lease (argv: a Windows run is
/// never a shell's), each package red on an error or a warning of its own ([`windows_units`]). No
/// other run compiles their `cfg(windows)` code: the hand-in's dependents check runs on its host,
/// and the CI's Windows checks had not run for two weeks. A `()` where a serial line belonged did
/// not build on Windows for about 12 hours, and the Windows box kept its old hub (11-19). With
/// nothing reached it takes no lease.
pub fn windows(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let Some(base) = base else { return no_verdict("no base to read the landings' reach from".into()) };
        let again: BTreeSet<String> = still_failing(&rows_of(&home, WINDOWS)).into_values().filter_map(|(unit, _, _)| unit.strip_prefix("windows ").map(str::to_string)).collect();
        let (from, to) = (base.pair(), tip.pair());
        let Some(paths) = crate::gitread::files_differing(&repo, &from.work, &to.work) else {
            return no_verdict(format!("the files between work {} and {} are not readable here", crate::state::short(&from.work), crate::state::short(&to.work)));
        };
        let tree = match crate::orch::ci::Tree::at(&repo, &to) { Ok(t) => t, Err(e) => return no_verdict(e) };
        let (pkgs, gone) = windows_reach(&tree, &paths, &again);
        // (a red package that has a build gap there now, or left the tree: nothing is left to fail)
        let gone: String = gone.iter().map(|p| format!("windows: {p} is checked on Windows no more (a build gap there, or it left the tree)\nGATEUNIT gone windows {p}\n")).collect();
        if pkgs.is_empty() {
            let said = format!("windows: the landings reach no package checked on Windows, and none is red there\n{gone}NODEGATE-OK\n");
            if let Err(e) = std::fs::write(log, &said) { return no_verdict(format!("{}: {e}", log.display())); }
            return verdict(Ok(Some(0)), &said, log);
        }
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let lease = ["lease", "build", "--needs", "os=windows", "--cost", "small", "--priority", "background", "--lane", "landq-windows", "--class", WINDOWS_CLASS, "--rev", &rev];
        let mut gate = vec!["cargo", "check", "--message-format", "short"];
        for (pkg, _) in &pkgs { gate.extend(["-p", pkg.as_str()]); }
        let why = [format!("windows: cargo check of {} package(s) the landings reach or still red there: {}", pkgs.len(), pkgs.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(" "))];
        let (status, text) = lease_run(&repo, &lease, &gate, &why, log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
        let said = format!("{gone}{}", windows_units(&status, &text, &pkgs));
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = file.write_all(said.as_bytes()); }
        verdict(status, &format!("{text}{said}"), log)
    })
}

/// The workspace-wide files every package builds by: a change to one reaches every package.
const WORKSPACE_WIDE: [&str; 4] = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"];

/// The packages a Windows box checks, each with its directory: those whose Cargo.toml's
/// platform-gaps has a `windows` table with no `build` gap ([`crate::lease::native::on_windows`]),
/// that `paths` reach (a crate they touch, or one depending on it: [`crate::orch::ci::in_set`]; a
/// workspace-wide file or the cargo config reaches every one), and those of `again` (still red
/// there) whatever they reach; then those of `again` checked there no more (a build gap there
/// now, or gone from the tree). A package with no `windows` table is not checked there: nothing
/// says it builds there ([`crate::lease::native::on_windows`] keeps it off Windows too).
fn windows_reach(tree: &crate::orch::ci::Tree, paths: &[String], again: &BTreeSet<String>) -> (Vec<(String, String)>, Vec<String>) {
    use crate::orch::ci::{in_set, krate, touched_crates};
    let all = paths.iter().any(|p| WORKSPACE_WIDE.contains(&p.as_str()) || p.starts_with(".cargo/"));
    let touched = touched_crates(tree, paths);
    let reached = in_set(tree, &touched, &touched);
    let checked: Vec<(String, String)> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(|m| krate(tree, m))
        .filter(|k| k.platform_gaps.get("windows").is_some_and(|gaps| !gaps.contains_key("build")))
        .map(|k| (k.package, k.dir)).collect();
    let gone: Vec<String> = again.iter().filter(|p| !checked.iter().any(|(c, _)| c == *p)).cloned().collect();
    let mut out: Vec<(String, String)> = checked.into_iter().filter(|(p, _)| all || reached.contains(p) || again.contains(p)).collect();
    out.sort();
    (out, gone)
}

/// The windows bucket's unit lines for its cargo check's output (`text`, the lease's log) of
/// `pkgs` (package, directory), as land_gate.sh's cargo_units says a cargo check's: each package's
/// own short-format error and warning lines (the package whose directory holds the line's file most
/// closely, its path with `/`), then `GATEUNIT fail windows <pkg>` for one that could not compile or
/// warned (zero warnings), `GATEUNIT ok` for the rest once cargo passed, and the gate's
/// NODEGATE line. A cargo check that failed and named none of them fails them all (one check: the
/// crate that stopped it stops those below). A lease that ran no cargo says nothing: its verdict is
/// the lease's.
fn windows_units(status: &Result<Option<i32>, String>, text: &str, pkgs: &[(String, String)]) -> String {
    let located: Vec<(String, String)> = text.lines().filter_map(|line| {
        let (path, said) = location(line)?;
        // (`<path>:<line>:<col>: `, its path's `\` as `/`)
        let head = line[..line.len() - said.len()].replace('\\', "/");
        (said.starts_with("error") || said.starts_with("warning")).then(|| (path.replace('\\', "/"), format!("{head}{said}")))
    }).collect();
    let owner = |file: &str| pkgs.iter().filter(|(_, dir)| dir.is_empty() || file.starts_with(&format!("{dir}/"))).max_by_key(|(_, dir)| dir.len()).map(|(p, _)| p.as_str());
    let failed_build = |pkg: &str| text.lines().any(|l| l.starts_with(&format!("error: could not compile `{pkg}`")));
    // (cargo's own failure exits 101; a lease that ended before it says its own exit)
    let ran = matches!(status, Ok(Some(101))) || !located.is_empty() || text.lines().any(|l| l.trim_start().starts_with("Finished ") || l.starts_with("error: could not compile"));
    let code = match status { Ok(Some(code)) if ran => *code, _ => return String::new() };
    let (mut out, mut named) = (String::new(), Vec::new());
    for (pkg, _) in pkgs {
        let own: Vec<&String> = located.iter().filter(|(file, _)| owner(file) == Some(pkg.as_str())).map(|(_, line)| line).collect();
        if failed_build(pkg) || !own.is_empty() {
            let mut seen = BTreeSet::new();
            for line in own.into_iter().filter(|l| seen.insert(l.as_str())).take(40) { out.push_str(&format!("{line}\n")); }
            out.push_str(&format!("{pkg}: cargo check failed on Windows\nGATEREPRO windows {pkg} :: makepad devhub run <lane> build --needs os=windows -- cargo check -p {pkg}\nGATEUNIT fail windows {pkg}\n"));
            named.push(pkg.as_str());
        } else if code == 0 {
            out.push_str(&format!("GATEUNIT ok windows {pkg}\n"));
        }
    }
    if code != 0 && named.is_empty() {
        out.push_str(&format!("windows: the cargo check exited {code} and named none of its packages: it fails them all\n"));
        for (pkg, _) in pkgs { out.push_str(&format!("GATEUNIT fail windows {pkg}\n")); }
        named.extend(pkgs.iter().map(|(p, _)| p.as_str()));
    }
    out.push_str(&if named.is_empty() { "NODEGATE-OK\n".to_string() } else { format!("NODEGATE-FAIL: windows {}\n", named.join(" ")) });
    out
}

/// The windows-tests bucket's name ([`windows_tests`]).
pub const WINDOWS_TESTS: &str = "windows-tests";

/// The windows-tests bucket's class, one for all its runs (its command names its packages).
const WINDOWS_TESTS_CLASS: &str = "test cargo test @landq-windows-tests";

/// Its corpus leg's class ([`corpus_argv`]: the JIT's build and its files under it).
const WINDOWS_CORPUS_CLASS: &str = "test makepad corpus @landq-windows-tests";

/// Runs the test targets a Windows box runs ([`windows_tests_reach`]: each package's
/// `[package.metadata.makepad.windows-tests]`) that the landings reach, and those still red there,
/// with one plain `cargo test` on a Windows box's lease (argv: a Windows run is never a shell's,
/// and the lease has no path to the box's JIT or devhub, so cargo runs them, as the windows
/// bucket's check). Each target is a unit (`wintest <package>/<target>`); its known reds (its
/// declaration's `skip`, until the queue item it names lands their fixes) stay out by their exact
/// names. A unit red on its run runs once more, its failing tests alone, in a second lease: green
/// then is a flake, red again names the landings. Its known reds then run alone, in a lease of
/// their own, judging nothing: one that passes is said, and main hears it once, to take it off
/// the list. The list kept itself honest only as long as someone relayed each fix (main, 11-24). No other run tests `cfg(windows)` code: the
/// tests bucket and the CI run on Linux and the Mac, and 13 of one lib's tests and one of another's
/// had gone red on Windows unseen (11-24). A declared corpus file (`corpus:<file>`) runs under
/// the JIT there, `makepad test <file>`, in a lease of its own ([`corpus_argv`]): no other run tests
/// what the JIT does on Windows. With nothing reached it takes no lease.
pub fn windows_tests(repo: PathBuf, home: PathBuf) -> Run {
    Arc::new(move |base, tip, log| {
        let Some(base) = base else { return no_verdict("no base to read the landings' reach from".into()) };
        let again: BTreeSet<String> = still_failing(&rows_of(&home, WINDOWS_TESTS)).into_values().filter_map(|(unit, _, _)| unit.strip_prefix("wintest ").map(str::to_string)).collect();
        let (from, to) = (base.pair(), tip.pair());
        let Some(paths) = crate::gitread::files_differing(&repo, &from.work, &to.work) else {
            return no_verdict(format!("the files between work {} and {} are not readable here", crate::state::short(&from.work), crate::state::short(&to.work)));
        };
        let tree = match crate::orch::ci::Tree::at(&repo, &to) { Ok(t) => t, Err(e) => return no_verdict(e) };
        let (units, gone) = windows_tests_reach(&tree, &paths, &again);
        // (a red unit no longer declared, or gone from the tree: nothing is left to fail)
        let gone: String = gone.iter().map(|u| format!("{WINDOWS_TESTS}: wintest {u} is run on Windows no more (no longer declared, or it left the tree)\nGATEUNIT gone wintest {u}\n")).collect();
        if units.is_empty() {
            let said = format!("{WINDOWS_TESTS}: the landings reach no test target run on Windows, and none is red there\n{gone}NODEGATE-OK\n");
            if let Err(e) = std::fs::write(log, &said) { return no_verdict(format!("{}: {e}", log.display())); }
            return verdict(Ok(Some(0)), &said, log);
        }
        let rev = match tip_rev(&repo, tip) { Ok(r) => r, Err(e) => return no_verdict(e) };
        let lease = ["lease", "test", "--needs", "os=windows", "--cost", "medium", "--priority", "background", "--lane", "landq-windows-tests", "--class", WINDOWS_TESTS_CLASS, "--rev", &rev];
        // each unit's first run and its rerun, by the units' order: the corpus files' leg, then the
        // cargo targets' (the main log is the cargo leg's, or the corpus leg's when it is alone)
        let (cargo, corpus): (Vec<WinUnit>, Vec<WinUnit>) = units.iter().cloned().partition(|u| u.corpus().is_none());
        let at = |u: &WinUnit| units.iter().position(|v| v == u).unwrap_or(0);
        let mut first: Vec<WinRun> = vec![WinRun::NotRun; units.len()];
        let mut rerun: Vec<Option<WinRun>> = vec![None; units.len()];
        let mut text = String::new();
        if !corpus.is_empty() {
            let corpus_log = if cargo.is_empty() { log.to_path_buf() } else { shard_log(log, 3) };
            let (runs, faulty): (Vec<WinUnit>, Vec<WinUnit>) = corpus.into_iter().partition(|u| u.fault.is_none());
            for u in &faulty { first[at(u)] = WinRun::Broke(u.fault.clone().unwrap_or_default(), Vec::new()); }
            let mut leg = String::new();
            if !runs.is_empty() {
                let lease = ["lease", "test", "--needs", "os=windows", "--cost", "medium", "--priority", "background", "--lane", "landq-windows-tests", "--class", WINDOWS_CORPUS_CLASS, "--rev", &rev];
                let argv = corpus_argv(&runs);
                let gate: Vec<&str> = argv.iter().map(String::as_str).collect();
                let why = [format!("{WINDOWS_TESTS}: {} corpus file(s) under the JIT, which the landings reach or still red there, a red one once more: {}", runs.len(), runs.iter().map(|u| format!("{} ({})", u.name(), u.why)).collect::<Vec<_>>().join(", "))];
                let (status, out) = lease_run(&repo, &lease, &gate, &why, &corpus_log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
                let Some(ran) = corpus_runs(&status, &out, &runs) else { return verdict(status, &out, &corpus_log) };
                for (u, (one, again)) in runs.iter().zip(ran) {
                    first[at(u)] = one;
                    rerun[at(u)] = again;
                }
                leg = out;
            } else if cargo.is_empty() {
                leg = format!("{WINDOWS_TESTS}: no corpus file of them can run: {}\n", faulty.iter().map(WinUnit::name).collect::<Vec<_>>().join(", "));
                if let Err(e) = std::fs::write(log, &leg) { return no_verdict(format!("{}: {e}", log.display())); }
            }
            if cargo.is_empty() { text = leg; }
        }
        // the cargo targets' leg
        if !cargo.is_empty() {
            let argv = windows_tests_argv(&cargo, &[]);
            let gate: Vec<&str> = argv.iter().map(String::as_str).collect();
            let why = [format!("{WINDOWS_TESTS}: cargo test of {} target(s) the landings reach or still red there: {}", cargo.len(), cargo.iter().map(|u| format!("{} ({})", u.name(), u.why)).collect::<Vec<_>>().join(", "))];
            let (status, out) = lease_run(&repo, &lease, &gate, &why, log, (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
            let Some(ran) = win_runs(&status, &out, &cargo) else { return verdict(status, &out, log) };
            text = out;
            for (u, run) in cargo.iter().zip(ran) { first[at(u)] = run; }
            // the red ones once more, their failing tests alone (whole where one broke), in a log of
            // their own beside this run's
            let red: Vec<WinUnit> = cargo.iter().filter(|u| matches!(first[at(u)], WinRun::Failed(..) | WinRun::Broke(..))).cloned().collect();
            if !red.is_empty() {
                let only: Vec<String> = match red.iter().any(|u| matches!(first[at(u)], WinRun::Broke(..))) {
                    true => Vec::new(),
                    false => red.iter().flat_map(|u| match &first[at(u)] { WinRun::Failed(tests, _) => tests.clone(), _ => Vec::new() }).collect(),
                };
                let argv = windows_tests_argv(&red, &only);
                let gate: Vec<&str> = argv.iter().map(String::as_str).collect();
                let why = [format!("{WINDOWS_TESTS}: the red target(s) once more at this tip before anyone is named: {}", red.iter().map(WinUnit::name).collect::<Vec<_>>().join(", "))];
                let (again_status, again_text) = lease_run(&repo, &lease, &gate, &why, &shard_log(log, 1), (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
                if let Some(runs) = win_runs(&again_status, &again_text, &red) {
                    for (unit, run) in red.iter().zip(runs) { rerun[at(unit)] = Some(run); }
                }
            }
        }
        let (said, ok) = windows_tests_said(&units, &first, &rerun);
        let said = format!("{gone}{said}");
        // the known reds alone, after the verdict's runs, in a log of their own: none gates
        let known: Vec<WinUnit> = units.iter().zip(&first).filter(|(u, r)| !u.skip.is_empty() && u.corpus().is_none() && !matches!(r, WinRun::NotRun)).map(|(u, _)| u.clone()).collect();
        let known_said = match known.is_empty() {
            true => String::new(),
            false => {
                let argv = known_argv(&known);
                let gate: Vec<&str> = argv.iter().map(String::as_str).collect();
                let why = [format!("{WINDOWS_TESTS}: the known reds alone, to see which pass now (they judge nothing): {}", known.iter().map(WinUnit::name).collect::<Vec<_>>().join(", "))];
                let (known_status, known_text) = lease_run(&repo, &lease, &gate, &why, &shard_log(log, 2), (base, tip), &|| true, &mut |_| true, &|| false, &|| false);
                let ran = matches!(known_status, Ok(Some(0 | 101))).then(|| test_outcomes(&known_text));
                let (lines, passes) = known_reds_said(&known, ran.as_ref());
                tell_known_passes(&home, &known, &passes);
                lines
            }
        };
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = file.write_all(format!("{said}{known_said}").as_bytes()); }
        verdict(Ok(Some(if ok { 0 } else { 1 })), &format!("{text}{said}"), log)
    })
}

/// A test target the windows-tests bucket runs: its package and directory, its target (`lib`,
/// `test:<name>`, or a corpus file `corpus:<file>`), its test binary's name, why it runs on
/// Windows, and the known reds it leaves out by their exact names until the queue item `known`
/// lands their fixes; what keeps a corpus entry from running (`fault`, [`corpus_fault`]).
#[derive(Clone, Debug, PartialEq)]
struct WinUnit {
    package: String,
    dir: String,
    target: String,
    binary: String,
    why: String,
    known: String,
    skip: Vec<String>,
    fault: Option<String>,
}

impl WinUnit {
    /// Its unit's name: `wintest <package>/<target>`, two words, so each target is a unit of its own.
    fn name(&self) -> String {
        format!("wintest {}/{}", self.package, self.target)
    }

    /// Its target as cargo selects it.
    fn select(&self) -> Vec<String> {
        match self.target.strip_prefix("test:") {
            Some(t) => vec!["--test".into(), t.into()],
            None => vec!["--lib".into()],
        }
    }

    /// Its corpus file from the tree's root (`corpus:<file>`, the file from its package's
    /// directory), which runs under the JIT alone: `makepad test <file>`. None: a cargo target.
    fn corpus(&self) -> Option<String> {
        let file = self.target.strip_prefix("corpus:")?;
        Some(if self.dir.is_empty() { file.to_string() } else { format!("{}/{file}", self.dir) })
    }

    /// The command that runs it alone on a Windows box (`exact`: a cargo target's failing tests).
    fn repro(&self, exact: &[String]) -> String {
        if let Some(file) = self.corpus() {
            return format!("makepad devhub run <lane> test --needs os=windows -- cargo run --profile host -p makepad-run --bin makepad -- test {file}");
        }
        let exact = if exact.is_empty() { String::new() } else { format!(" -- --exact{}", exact.iter().map(|t| format!(" {t}")).collect::<String>()) };
        format!("makepad devhub test <lane> {} {} --needs os=windows --native \"its Windows red\"{exact}", self.package, self.select().join(" "))
    }
}

/// What keeps corpus entry `u` from running in `tree` (None: it runs): its file is a plain relative
/// `.rs` path in the tree (no `..`, nothing a PowerShell literal would read: [`corpus_argv`]), and it
/// has no skip list (`makepad test` runs a file whole).
fn corpus_fault(tree: &crate::orch::ci::Tree, u: &WinUnit) -> Option<String> {
    let file = u.corpus()?;
    let plain = file.ends_with(".rs") && file.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)));
    if !plain { return Some(format!("its corpus entry {} is no plain relative .rs path", u.target)); }
    if !tree.files.iter().any(|f| *f == file) { return Some(format!("its corpus file {file} is not in the tree")); }
    if !u.skip.is_empty() { return Some("a corpus file runs whole under `makepad test`: its skip list cannot apply".into()); }
    None
}

/// How a unit's run went ([`win_runs`]): passed; tests failed (their names, and the panics their
/// output gave); its binary broke (did not build, ended before its result, or never ran: why, and
/// the error lines that say it); or it did not run, another package's build having stopped the
/// cargo test (no verdict on it: that package is the red one).
#[derive(Clone, Debug, PartialEq)]
enum WinRun {
    Passed,
    Failed(Vec<String>, Vec<String>),
    Broke(String, Vec<String>),
    NotRun,
}

/// The test targets a Windows box runs: each `[package.metadata.makepad.windows-tests]` target of
/// a package that `paths` reach (a crate they touch, or one depending on it; a workspace-wide file
/// or the cargo config reaches every one), and those of `again` (still red there, by
/// `<package>/<target>`) whatever they reach; then those of `again` declared no more.
fn windows_tests_reach(tree: &crate::orch::ci::Tree, paths: &[String], again: &BTreeSet<String>) -> (Vec<WinUnit>, Vec<String>) {
    use crate::orch::ci::{in_set, krate, touched_crates};
    let all = paths.iter().any(|p| WORKSPACE_WIDE.contains(&p.as_str()) || p.starts_with(".cargo/"));
    let touched = touched_crates(tree, paths);
    let reached = in_set(tree, &touched, &touched);
    let declared: Vec<WinUnit> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(|m| krate(tree, m)).flat_map(|k| {
        k.windows_tests.iter().map(|(target, decl)| WinUnit {
            package: k.package.clone(),
            dir: k.dir.clone(),
            target: target.clone(),
            binary: match target.strip_prefix("test:") { Some(t) => t.replace('-', "_"), None if target.starts_with("corpus:") => String::new(), None => k.lib_name.clone() },
            why: decl.why.clone(),
            known: decl.known.clone(),
            skip: decl.skip.clone(),
            fault: None,
        }).collect::<Vec<_>>()
    }).map(|u| WinUnit { fault: corpus_fault(tree, &u), ..u }).collect();
    let key = |u: &WinUnit| format!("{}/{}", u.package, u.target);
    let gone: Vec<String> = again.iter().filter(|a| !declared.iter().any(|u| key(u) == **a)).cloned().collect();
    let mut out: Vec<WinUnit> = declared.into_iter().filter(|u| all || reached.contains(&u.package) || again.contains(&key(u))).collect();
    out.sort_by_key(key);
    (out, gone)
}

/// The cargo test of `units`: every package's targets and, past `--`, the known reds left out by
/// their exact names; `only`: these tests alone (by their exact names), else all of them.
fn windows_tests_argv(units: &[WinUnit], only: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = ["cargo", "test", "--no-fail-fast", "--message-format", "short"].map(String::from).to_vec();
    let mut selected = BTreeSet::new();
    for u in units {
        if selected.insert(("-p".to_string(), u.package.clone())) { argv.extend(["-p".to_string(), u.package.clone()]); }
    }
    let mut targets: Vec<Vec<String>> = units.iter().map(WinUnit::select).collect();
    targets.sort();
    targets.dedup();
    argv.extend(targets.into_iter().flatten());
    argv.extend(["--".to_string(), "--exact".to_string()]);
    argv.extend(only.iter().cloned());
    let mut skips: Vec<&String> = units.iter().flat_map(|u| &u.skip).collect();
    skips.sort();
    skips.dedup();
    for s in skips { argv.extend(["--skip".to_string(), s.clone()]); }
    argv
}

/// The cargo test of `units`' known reds alone ([`WinUnit::skip`]), by their exact names.
fn known_argv(units: &[WinUnit]) -> Vec<String> {
    let names: Vec<String> = units.iter().flat_map(|u| u.skip.iter().cloned()).collect();
    let all: Vec<WinUnit> = units.iter().map(|u| WinUnit { skip: Vec::new(), ..u.clone() }).collect();
    windows_tests_argv(&all, &names)
}

/// The corpus leg of `units` (corpus files whose entries can run, [`corpus_fault`]): one Windows
/// PowerShell script (a Windows run is plain argv; the node runs a `-Command` script from a .ps1
/// file, lease/argv.rs windows_plain), as `devhub test` runs a file under the JIT: the JIT built once
/// in the lease's target (`cargo build --profile host -p makepad-run --bin makepad`), then each file
/// under it (`makepad test <file>`, one file a run), a red one once more at once. Its markers, which
/// [`corpus_runs`] reads: `CORPUSBUILD ok|fail <exit>`, `CORPUSRUN <file>` (`CORPUSAGAIN` for the
/// second run) and `CORPUSEXIT <file> <exit>` around each run, `CORPUSDONE` at its end. The files
/// are single-quoted literals: [`corpus_fault`] lets none through a quote could end.
fn corpus_argv(units: &[WinUnit]) -> Vec<String> {
    let files: Vec<String> = units.iter().filter_map(WinUnit::corpus).map(|f| format!("'{f}'")).collect();
    let script = format!(r#"$TD = if ($env:CARGO_TARGET_DIR) {{ $env:CARGO_TARGET_DIR }} else {{ Join-Path (Get-Location) 'target' }}
cargo build --profile host -q -p makepad-run --bin makepad --message-format short 2>&1 | ForEach-Object {{ "$_" }}
if ($LASTEXITCODE -ne 0) {{ "CORPUSBUILD fail $LASTEXITCODE"; exit 1 }}
'CORPUSBUILD ok'
$J = Join-Path $TD 'host/makepad.exe'
$red = 0
foreach ($T in @({})) {{
  foreach ($M in @('CORPUSRUN', 'CORPUSAGAIN')) {{
    "$M $T"
    & $J test $T 2>&1 | ForEach-Object {{ "$_" }}
    $code = $LASTEXITCODE
    "CORPUSEXIT $T $code"
    if ($code -eq 0) {{ break }}
    if ($M -eq 'CORPUSAGAIN') {{ $red = 1 }}
  }}
}}
'CORPUSDONE'
exit $red"#, files.join(", "));
    ["powershell", "-NoProfile", "-NonInteractive", "-Command"].iter().map(|w| w.to_string()).chain([script]).collect()
}

/// Each of `units`' runs in the corpus leg's output (`text`, [`corpus_argv`]'s markers): its first
/// and, when that was red, its second. A run passed by its exit 0 and its `test result: <n> passed;
/// 0 failed` line; its failing tests (`test <name> ... FAILED (<ms>)`) with the reason lines under
/// each (`  report: ...`, as the JIT's harness prints them); or it broke: the file did not compile
/// under the JIT (its error lines, the JIT's `could not compile` last), it ended before its result,
/// or its output names no test. The JIT not built breaks every unit, its build's error lines said.
/// None: the script did not run to its end (its lease's verdict stands).
fn corpus_runs(status: &Result<Option<i32>, String>, text: &str, units: &[WinUnit]) -> Option<Vec<(WinRun, Option<WinRun>)>> {
    #[derive(Default)]
    struct Run { file: String, failed: Vec<String>, why: Vec<String>, errors: Vec<String>, result: Option<bool>, exit: Option<i64> }
    if !matches!(status, Ok(Some(_))) { return None; }
    let (mut runs, mut build, mut built, mut done, mut reasons) = (Vec::<Run>::new(), Vec::<String>::new(), None::<bool>, false, false);
    let error = |line: &str| -> Option<String> {
        match location(line).filter(|(_, said)| said.starts_with("error")) {
            Some((_, said)) => Some(crate::proto::clean(&format!("{}{said}", line[..line.len() - said.len()].replace('\\', "/")), 300)),
            None => line.trim().starts_with("error").then(|| crate::proto::clean(line.trim(), 300)),
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("CORPUSBUILD ") {
            built = Some(rest == "ok");
        } else if let Some(file) = t.strip_prefix("CORPUSRUN ").or_else(|| t.strip_prefix("CORPUSAGAIN ")) {
            runs.push(Run { file: file.to_string(), ..Default::default() });
            reasons = false;
        } else if let Some(rest) = t.strip_prefix("CORPUSEXIT ") {
            if let Some(r) = runs.last_mut().filter(|r| r.exit.is_none()) { r.exit = rest.rsplit_once(' ').and_then(|(_, c)| c.parse().ok()); }
        } else if t == "CORPUSDONE" {
            done = true;
        } else if let Some(r) = runs.last_mut().filter(|r| r.exit.is_none()) {
            if let Some(rest) = t.strip_prefix("test ").filter(|r| r.contains(" ... ")) {
                reasons = false;
                if let Some((name, _)) = rest.split_once(" ... FAILED") {
                    r.failed.push(name.to_string());
                    reasons = true;
                }
            } else if let Some(result) = t.strip_prefix("test result: ") {
                r.result = Some(result.contains("; 0 failed"));
                reasons = false;
            } else if reasons && line.starts_with("  ") && !t.is_empty() {
                if r.why.len() < PANICS_KEPT { r.why.push(crate::proto::clean(t, 300)); }
            } else if let Some(e) = error(line) {
                r.errors.push(e);
            }
        } else if built.is_none() {
            build.extend(error(line));
        }
    }
    if built == Some(false) {
        return Some(units.iter().map(|_| (WinRun::Broke("the JIT did not build on Windows".into(), build.clone()), None)).collect());
    }
    if !done { return None; }
    let of = |r: &Run| -> WinRun {
        if r.exit == Some(0) && r.result == Some(true) { return WinRun::Passed; }
        if !r.failed.is_empty() {
            let mut failed = r.failed.clone();
            failed.sort();
            failed.dedup();
            return WinRun::Failed(failed, r.why.clone());
        }
        let exit = r.exit.map_or("none".to_string(), |c| c.to_string());
        match r.result {
            None if r.errors.iter().any(|e| e.contains("could not compile")) => WinRun::Broke("it did not compile under the JIT on Windows".into(), r.errors.clone()),
            None => WinRun::Broke(format!("the JIT ended before its result (exit {exit})"), r.errors.clone()),
            Some(_) => WinRun::Broke(format!("its run failed (exit {exit}), and its output names no test"), r.errors.clone()),
        }
    };
    Some(units.iter().map(|u| {
        let file = u.corpus().unwrap_or_default();
        let mut mine = runs.iter().filter(|r| r.file == file);
        match mine.next() {
            Some(one) => (of(one), mine.next().map(of)),
            None => (WinRun::Broke("the corpus leg ran no run of it".into(), Vec::new()), None),
        }
    }).collect())
}

/// Each test's outcome in a cargo test's output: passed (`test <name> ... ok`) or not (`...
/// FAILED`), its last line deciding (a test that runs a child prints the child's line first).
fn test_outcomes(text: &str) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for t in text.lines().map(str::trim) {
        let Some(rest) = t.strip_prefix("test ") else { continue };
        if let Some(name) = rest.strip_suffix(" ... ok") { out.insert(name.to_string(), true); }
        else if let Some(name) = rest.strip_suffix(" ... FAILED") { out.insert(name.to_string(), false); }
    }
    out
}

/// The known reds' run's lines (`ran`: each test's outcome, None when the lease ran no cargo
/// test), none a verdict's (no `FAILED`, panic or unit line): each that passes now, and how many
/// are still red or did not finish (a crash ends its binary's run). With the passes, by unit.
fn known_reds_said(units: &[WinUnit], ran: Option<&BTreeMap<String, bool>>) -> (String, Vec<(String, String)>) {
    let (mut out, mut passes) = (String::new(), Vec::new());
    for u in units {
        let name = u.name();
        let Some(ran) = ran else {
            out.push_str(&format!("{name}: its {} known red(s) did not run: the lease ran no cargo test\n", u.skip.len()));
            continue;
        };
        let (mut red, mut unfinished) = (0, 0);
        for test in &u.skip {
            match ran.get(test) {
                Some(true) => {
                    out.push_str(&format!("{name}: known red now passes on Windows: take it off its windows-tests skip list: {test}\n"));
                    passes.push((name.clone(), test.clone()));
                }
                Some(false) => red += 1,
                None => unfinished += 1,
            }
        }
        if red + unfinished > 0 {
            out.push_str(&format!("{name}: of its known reds, {red} still red and {unfinished} did not finish (left out until {} lands their fixes)\n", if u.known.is_empty() { "the item that fixes them" } else { &u.known }));
        }
    }
    (out, passes)
}

/// Main hears each known red that passes once (`passes`: unit, test), kept in the hub's home
/// ([`KNOWN_TOLD`]); a test that left its unit's list (of `units`, this run's) leaves the record,
/// so one listed again is told again.
fn tell_known_passes(home: &Path, units: &[WinUnit], passes: &[(String, String)]) {
    let path = home.join(KNOWN_TOLD);
    let listed = |unit: &str, test: &str| units.iter().find(|u| u.name() == unit).is_none_or(|u| u.skip.iter().any(|t| t == test));
    let mut told: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines()
        .filter(|l| l.split_once('\t').is_some_and(|(unit, test)| listed(unit, test))).map(str::to_string).collect();
    for (unit, test) in passes {
        let line = format!("{unit}\t{test}");
        if told.contains(&line) { continue; }
        let note = format!("{WINDOWS_TESTS}: {unit}'s known red {test} now passes on Windows: take it off the skip list in its Cargo.toml's [package.metadata.makepad.windows-tests]");
        if let Err(e) = crate::notes::leave(home, "main", &note) { eprintln!("devhub bucket {WINDOWS_TESTS}: agent notes: {e}"); }
        told.push(line);
    }
    if let Err(e) = std::fs::write(&path, told.iter().map(|l| format!("{l}\n")).collect::<String>()) { eprintln!("devhub bucket {WINDOWS_TESTS}: {}: {e}", path.display()); }
}

/// The known reds main heard pass ([`tell_known_passes`]): `<unit>\t<test>` a line.
const KNOWN_TOLD: &str = "windows-tests-known-told";

/// Each of `units`' run in a cargo test's output (`text`), by its test binary (cargo's `Running
/// … (<dir>/<binary>-<hash>[.exe])` line): its failing tests (`test <name> ... FAILED`) and panics,
/// or why it broke: its package did not build (a compile error in its directory, or cargo's
/// `could not compile` of it), its binary ended before its result (cargo's exit line for it with
/// an exit other than a test failure's 101), or cargo ran no such binary (cargo's own errors
/// beside it). A binary passed by its last `test result` line and no exit line of cargo's: a test
/// that runs the binary again for a child prints the child's results in its section too (some of
/// relax-compiler's lib tests do). None: the lease ran no cargo test (cargo exits 0 or 101; its
/// verdict is then the lease's).
fn win_runs(status: &Result<Option<i32>, String>, text: &str, units: &[WinUnit]) -> Option<Vec<WinRun>> {
    struct Bin { name: String, failed: Vec<String>, panics: Vec<String>, result: Option<bool> }
    if !matches!(status, Ok(Some(0 | 101))) { return None; }
    let (mut bins, mut panic_at) = (Vec::<Bin>::new(), None::<String>);
    // compile errors by file (its path with `/`), and cargo's own error lines
    let (mut compile, mut cargo) = (Vec::<(String, String)>::new(), Vec::<String>::new());
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Running ") {
            let exe = rest.rsplit_once('(').map_or(rest, |(_, e)| e.trim_end_matches(')'));
            let file = exe.rsplit(['/', '\\']).next().unwrap_or(exe).trim_end_matches(".exe");
            let name = file.rsplit_once('-').filter(|(_, hash)| hash.chars().all(|c| c.is_ascii_hexdigit())).map_or(file, |(n, _)| n);
            bins.push(Bin { name: name.to_string(), failed: Vec::new(), panics: Vec::new(), result: None });
            panic_at = None;
        } else if t.starts_with("Doc-tests ") {
            bins.push(Bin { name: String::new(), failed: Vec::new(), panics: Vec::new(), result: None });
        } else if let (Some(at), Some(bin)) = (panic_at.take(), bins.last_mut()) {
            // (the line after a panic's place is its message, whatever it starts with)
            if bin.panics.len() < PANICS_KEPT { bin.panics.push(crate::proto::clean(&format!("panicked at {at}: {t}"), 300)); }
        } else if let Some((path, said)) = location(line).filter(|(_, said)| said.starts_with("error")) {
            compile.push((path.replace('\\', "/"), crate::proto::clean(&format!("{}{said}", line[..line.len() - said.len()].replace('\\', "/")), 300)));
        } else if t.starts_with("error") || t.contains("process didn't exit successfully") {
            cargo.push(crate::proto::clean(t, 300));
        } else if let Some(bin) = bins.last_mut() {
            if let Some(test) = t.strip_prefix("test ").and_then(|r| r.strip_suffix(" ... FAILED")) {
                bin.failed.push(test.to_string());
            } else if let Some(result) = t.strip_prefix("test result: ") {
                bin.result = Some(result.starts_with("ok"));
            } else if let Some((_, at)) = t.split_once(" panicked at ") {
                panic_at = Some(at.trim_end_matches(':').to_string());
            }
        }
    }
    let own = |u: &WinUnit| -> Vec<String> {
        compile.iter().filter(|(file, _)| file.starts_with(&format!("{}/", u.dir))).map(|(_, l)| l.clone())
            .chain(cargo.iter().filter(|l| l.starts_with(&format!("error: could not compile `{}`", u.package))).cloned()).collect()
    };
    let unbuilt = units.iter().any(|u| !own(u).is_empty());
    Some(units.iter().map(|u| {
        let own = own(u);
        // cargo's line for a binary that exited other than 0, and whether that was a test failure's
        let exited: Vec<String> = cargo.iter().filter(|l| l.contains("process didn't exit successfully") && l.contains(&u.binary)).cloned().collect();
        let failed_tests = exited.iter().all(|l| l.contains("exit code: 101)") || l.contains("exit status: 101)"));
        match bins.iter().find(|b| b.name == u.binary) {
            Some(_) if !failed_tests => WinRun::Broke("its test binary ended before its result".into(), exited),
            Some(b) if exited.is_empty() && b.result == Some(true) => WinRun::Passed,
            Some(b) if !b.failed.is_empty() => {
                let mut failed = b.failed.clone();
                failed.sort();
                failed.dedup();
                WinRun::Failed(failed, b.panics.clone())
            }
            Some(b) if b.result.is_some() || !exited.is_empty() => WinRun::Broke("its tests failed, and its output names none".into(), exited),
            Some(_) => WinRun::Broke("its test binary ended before its result".into(), exited),
            None if !own.is_empty() => WinRun::Broke("it did not build on Windows".into(), own),
            None if unbuilt => WinRun::NotRun,
            None => WinRun::Broke("cargo ran no test binary of it".into(), compile.iter().map(|(_, l)| l.clone()).chain(cargo.iter().cloned()).collect()),
        }
    }).collect())
}

/// The windows-tests bucket's unit lines, after its `STEP` line (which keeps the cargo output's own
/// FAILED and panic lines from any unit): each unit's known reds left out; a unit green, or green
/// on its rerun (`rerun`: a flake, `GATEFLAKE` with the tests that failed first); a unit red on both
/// runs, with its failing tests, its panics, its repro and `GATEUNIT fail`; then the NODEGATE line.
/// With whether every unit passed.
fn windows_tests_said(units: &[WinUnit], first: &[WinRun], rerun: &[Option<WinRun>]) -> (String, bool) {
    let mut out = format!("STEP {WINDOWS_TESTS}: {} target(s) on Windows\n", units.len());
    let mut red = Vec::new();
    for ((u, run), again) in units.iter().zip(first).zip(rerun) {
        let name = u.name();
        if !u.skip.is_empty() && u.corpus().is_none() {
            out.push_str(&format!("{name}: leaves out its {} known red(s) on Windows by their names, until {} lands their fixes\n", u.skip.len(), if u.known.is_empty() { "the item that fixes them" } else { &u.known }));
        }
        // (a rerun that did not get to it leaves its first run's red standing)
        let last = match again { Some(WinRun::NotRun) | None => run, Some(r) => r };
        match (run, last) {
            (WinRun::Passed, _) => out.push_str(&format!("GATEUNIT ok {name}\n")),
            (WinRun::NotRun, _) | (_, WinRun::NotRun) => out.push_str(&format!("{name}: not run: another package's build stopped the cargo test\n")),
            (_, WinRun::Passed) => {
                let tests = match run { WinRun::Failed(t, _) => t.join(" "), _ => String::new() };
                out.push_str(&format!("{name}: a flake: red, then green on its rerun\nGATEFLAKE {name} :: {tests}\nGATEUNIT ok {name}\n"));
            }
            (_, WinRun::Failed(tests, panics)) => {
                for t in tests { out.push_str(&format!("FAILED {t}\n")); }
                for p in panics { out.push_str(&format!("{p}\n")); }
                out.push_str(&format!("{name}: {} test(s) failed on Windows{}\nGATEREPRO {name} :: {}\nGATEUNIT fail {name}\n",
                    tests.len(), if matches!(again, Some(WinRun::Failed(..) | WinRun::Broke(..))) { ", on its rerun too" } else { "" }, u.repro(tests)));
                red.push(name);
            }
            (_, WinRun::Broke(why, errors)) => {
                for e in errors.iter().take(40) { out.push_str(&format!("{e}\n")); }
                out.push_str(&format!("{name}: {why}\nGATEREPRO {name} :: {}\nGATEUNIT fail {name}\n", u.repro(&[])));
                red.push(name);
            }
        }
    }
    out.push_str(&if red.is_empty() { "NODEGATE-OK\n".to_string() } else { format!("NODEGATE-FAIL: {WINDOWS_TESTS} {}\n", red.join(", ")) });
    (out, red.is_empty())
}

/// Runs land_gate.sh under a devhub lease, its output in `log` after the `why` lines; a failed
/// step's logs, which go with the node's scratch at the lease's end, in `<log>.keep` beside it
/// (`lease --keep`). A lost lease ([`crate::lease::client::LOST_EXIT`]) runs it once more in the
/// same log, after [`RERUN`]; the text is the last run's. Once `needed` says no, a lease whose
/// gate has not started (its log's run has no GATEQUEUE line yet: queued, or in its setup) ends,
/// its holder stopped by its pid, and the run passes [`NOT_NEEDED`]: a tests run's verdict waited
/// for a box the queue no longer needed. A tests shard's lease holds a pool slot (`slot`,
/// [`Need`]): it asks for one before its lease, and one that waited until its run's queue was dealt
/// out needs no lease and passes [`NOT_NEEDED`]; one adopted takes its slot back as its lease runs
/// on, and frees it if its lease has ended. A lease told to give its slot to a run that goes
/// first (`more`, [`Dealer::take_yield`]) lets it go once its gate ends ([`Need::Gave`]), and the
/// gate runs again in the same log after [`YIELDED`], its lease asked in turn: the text is every
/// part's, a lost lease's run before a [`RERUN`] cut ([`kept`]), and the status the worst part's.
///
/// The holder writes its own exit beside the log as it ends (`lease --exit-to`, `<log>.exit`), so
/// a master that adopts the run after a restart has it whether or not it is the holder's parent: an
/// exec keeps the children, a crash or a switch-over does not, and a shard that had ended before
/// the restart is no process to wait for at all. Its exit is the file's; one stopped as not needed
/// before the restart is said so in its log. A tests run adopted at 16:51Z (11-08) read a shard the
/// old master had stopped as not needed as one with no exit status: no verdict on the run.
#[allow(clippy::too_many_arguments)]
fn lease_run(repo: &Path, lease: &[&str], gate: &[&str], why: &[String], log: &Path, (base, tip): (&Pin, &Pin), needed: &dyn Fn() -> bool, slot: &mut dyn FnMut(Need) -> bool, more: &dyn Fn() -> bool, withdraw: &dyn Fn() -> bool) -> (Result<Option<i32>, String>, String) {
    let sidecar = log.with_extension("log.run");
    let exit = log.with_extension("log.exit");
    let note = |line: &str| if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) { let _ = writeln!(file, "{line}"); };
    let unneeded = || !needed() && !this_part(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| l.starts_with("GATEQUEUE "));
    let not_needed = || -> Result<Option<i32>, String> {
        note(NOT_NEEDED);
        Ok(Some(0))
    };
    // its ask withdrawn before its grant ([`Board::give_queued`]): it asks for a slot again
    let withdrew = std::cell::Cell::new(false);
    let ungranted = || !this_part(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| granted(l).is_some());
    let mut execute = |again: bool, gave: bool| -> Result<Option<i32>, String> {
        if gave { slot(Need::Gave); }
        if sidecar.exists() {
            let (pid, _, _, _) = read_run(&sidecar).ok_or_else(|| format!("invalid bucket run {}", sidecar.display()))?;
            // its holder's exit written names its end, whatever process its pid is since: a
            // restarted bisect adopts probes that ended hours before ([`bisect_log`])
            let ended = || crate::procinfo::exited(pid).or_else(|| crate::lease::client::read_exit(&exit, pid).map(|_| None));
            slot(match ended() { Some(_) => Need::Ended, None => Need::Running(modified_ms(&sidecar)) });
            let mut stopped = false;
            let stopped_before = || this_part(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| l == NOT_NEEDED);
            loop {
                // (the holder writes its file before it exits: read once it is gone)
                if let Some(code) = ended() {
                    if stopped { return not_needed(); }
                    let (status, unwatched) = adopted_end(crate::lease::client::read_exit(&exit, pid), code, stopped_before(), verdict_said(log));
                    if unwatched { note(UNWATCHED); }
                    return status;
                }
                if !stopped && unneeded() {
                    crate::os::signal_one(pid, true);
                    stopped = true;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let started = log_started(log).ok_or_else(|| format!("invalid bucket log {}", log.display()))?;
        if !slot(Need::Lease) {
            if !again { std::fs::write(log, why.iter().map(|l| format!("{l}\n")).collect::<String>()).map_err(|e| e.to_string())?; }
            return not_needed();
        }
        let own = crate::cli::own_argv(repo);
        let mut file = if again { std::fs::OpenOptions::new().append(true).open(log) } else { std::fs::File::create(log) }.map_err(|e| e.to_string())?;
        if !again { for line in why { writeln!(file, "{line}").map_err(|e| e.to_string())?; } }
        let err = file.try_clone().map_err(|e| e.to_string())?;
        // (a run again in the same log: the last holder's exit is not this one's)
        match std::fs::remove_file(&exit) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", exit.display())),
            _ => {}
        }
        let mut child = std::process::Command::new(&own[0]).args(&own[1..]).args(lease).arg("--keep").arg(log.with_extension("keep")).arg("--exit-to").arg(&exit).arg("--").args(gate)
            .current_dir(repo).env("DEVHUB_AGENT", crate::lease::sched::BUCKET_AGENT)
            .stdin(std::process::Stdio::null()).stdout(file).stderr(err).spawn().map_err(|e| e.to_string())?;
        let saved = save_run(log, child.id(), started, base, tip);
        let mut stopped = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(_)) if withdrew.get() => break Ok(None),
                Ok(Some(_)) if stopped => break not_needed(),
                Ok(Some(s)) => break Ok(s.code()),
                Ok(None) if !stopped && unneeded() => {
                    let _ = child.kill();
                    stopped = true;
                }
                // (a queued ask: its holder's end drops it, no box set up for it)
                Ok(None) if !stopped && ungranted() && withdraw() => {
                    let _ = child.kill();
                    stopped = true;
                    withdrew.set(true);
                }
                Ok(None) => std::thread::sleep(Duration::from_secs(1)),
                Err(e) => break Err(e.to_string()),
            }
        };
        saved.map_err(|e| format!("bucket run: {e}"))?;
        status
    };
    // a shard that gave its slot up and waited for one again when the master restarted: its log
    // holds its earlier parts, and goes on after them
    let resumed = std::fs::read_to_string(log).is_ok_and(|t| t.lines().any(|l| l == YIELDED || l == WITHDREW)) && !sidecar.exists();
    let mut status = execute(resumed, false);
    // a part that gave its slot up and failed: the run's status, whatever its later parts end in
    let mut failed = None;
    loop {
        if withdrew.replace(false) {
            note(WITHDREW);
            let _ = std::fs::remove_file(&sidecar);
            status = execute(true, true);
        } else if let Some(why) = rerun_why(&status, &std::fs::read_to_string(log).unwrap_or_default()) {
            note(why);
            let _ = std::fs::remove_file(&sidecar);
            status = execute(true, false);
        } else if matches!(status, Ok(Some(_))) && verdict_said(log) && more() {
            if status != Ok(Some(0)) { failed.get_or_insert(status.clone()); }
            note(YIELDED);
            let _ = std::fs::remove_file(&sidecar);
            status = execute(true, true);
        } else {
            break;
        }
    }
    if let (Some(f), Ok(Some(0))) = (failed, &status) { status = f; }
    if let Err(e) = &status { note(&format!("gate lease: {e}")); }
    let text = kept(&std::fs::read_to_string(log).unwrap_or_default());
    if let Err(e) = keep_failed_units(&text, &log.with_extension("keep"), &crate::proto::home().join("lease-logs")) {
        note(&format!("bucket: a failed unit's output is not kept under its lease: {e}"));
    }
    (status, text)
}

/// A failed unit's output outlives its run under the lease's id, where `job result <lease>` reads
/// it, as a failed lane run's does ([`crate::lease::master`]): the last 64 KB of each file the gate
/// names for it (`GATEOUT <unit> :: <file>..`) and where the whole file is. The lease's `--keep`
/// brought them back (`<log>.keep/<its run dir>/<file>`; the newest, where a lost lease's run left
/// one too), and a lease's part of the log starts at its line `L1 box-a: sources at ..`. A file
/// already kept is not added again (a master that adopted the run after a restart ends it once
/// more). The tests bucket's L817327 said "kept no output" of a `makepad-fab` bin that aborted on a
/// corrupt heap: its message and stacks were only in the shard's keep (11-11).
fn keep_failed_units(text: &str, keep: &Path, logs: &Path) -> std::io::Result<()> {
    let mut lease = None;
    for line in text.lines() {
        if let Some(id) = part_lease(line) { lease = Some(id); }
        let (Some(id), Some((unit, files))) = (lease, line.strip_prefix("GATEOUT ").and_then(|r| r.split_once(" :: "))) else { continue };
        let path = logs.join(format!("{id}.log"));
        let held = std::fs::read_to_string(&path).unwrap_or_default();
        let mut add = String::new();
        for file in files.split_whitespace() {
            let found = std::fs::read_dir(keep).into_iter().flatten().flatten().map(|d| d.path().join(file)).filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p))).max();
            let head = match &found {
                Some((_, p)) => format!("== {unit} failed: its {file}, the last 64 KB ({})", p.display()),
                None => format!("== {unit} failed: its {file} was not kept in {}", keep.display()),
            };
            if held.lines().any(|l| l == head) { continue; }
            let body = found.map_or(String::new(), |(_, p)| crate::lease::exec::log_tail(&p, usize::MAX));
            add.push_str(&format!("{head}\n{body}\n"));
        }
        if !add.is_empty() {
            std::fs::create_dir_all(logs)?;
            std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(add.as_bytes())?;
        }
    }
    Ok(())
}

/// The lease a gate's part of a log runs under, from its first line (`L1 box-a: sources at ..`).
fn part_lease(line: &str) -> Option<&str> {
    let (id, _) = line.split_once(": sources at ").and_then(|(head, _)| head.split_once(' '))?;
    (id.len() > 1 && id.starts_with('L') && id[1..].bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

/// Where each failed unit's output is kept, by unit key, as it is read: the lease `devhub job
/// result <lease>` shows it under, or the file. A gate's unit that named its output files
/// (`GATEOUT <unit> :: <file>..`), kept under the lease of the part it ran in
/// ([`keep_failed_units`]); a prebuild listing's whole stream (`GATEKEPT <unit> :: <lease>`,
/// orch/warm.rs); a CI leg's log, kept beside the bucket's (`GATEKEPT <leg> :: <file>`, orch/ci.rs).
/// A red named only its unit, and its kept output was found by reading the bucket's code (11-18).
fn kept_outputs(text: &str) -> BTreeMap<String, String> {
    let (mut out, mut lease) = (BTreeMap::new(), None);
    for line in text.lines() {
        if let Some(id) = part_lease(line) { lease = Some(id); }
        if let Some((unit, at)) = line.strip_prefix("GATEKEPT ").and_then(|r| r.split_once(" :: ")) {
            let at = at.trim();
            out.insert(unit_key(unit), if Path::new(at).is_absolute() { at.to_string() } else { format!("devhub job result {at}") });
        } else if let (Some(id), Some((unit, _))) = (lease, line.strip_prefix("GATEOUT ").and_then(|r| r.split_once(" :: "))) {
            out.insert(unit_key(unit), format!("devhub job result {id}"));
        }
    }
    out
}

fn lost(status: &Result<Option<i32>, String>) -> bool {
    matches!(status, Ok(Some(code)) if *code == crate::lease::client::LOST_EXIT)
}

/// A lost lease's gate runs once more, and so does one the kernel's OOM killer ended (its box ran
/// out, or a scope's memory), and one that exited 0 without its verdict line ([`unsaid`]): none is
/// a verdict on the landings. Not when its log says it already ran again.
fn again(status: &Result<Option<i32>, String>, log: &str) -> bool {
    let oom = this_part(log).lines().any(|l| l.contains(" ended without its summary: ") && l.contains("the kernel's OOM killer killed"));
    let since = log.rsplit_once(&format!("{YIELDED}\n")).map_or(log, |(_, part)| part);
    (lost(status) || oom || unsaid(status, log)) && !since.lines().any(|l| RERUN_LINES.contains(&l))
}

/// A gate that said its start (land_gate.sh's `GATE <mode>: ` line) in its part of the log and
/// exited 0 without a verdict line, not stopped as not needed: it said nothing of the landings
/// ([`RERUN_UNSAID`]). A cargo check says no verdict line, nor its start.
fn unsaid(status: &Result<Option<i32>, String>, log: &str) -> bool {
    let part = this_part(log);
    matches!(status, Ok(Some(0))) && part.lines().any(|l| l.starts_with("GATE ") && l.contains(": "))
        && !part.lines().any(|l| l.starts_with("NODEGATE-OK") || l.starts_with("NODEGATE-FAIL") || l == NOT_NEEDED)
}

/// The line a gate that runs once more ([`again`]) writes before it: why.
fn rerun_why(status: &Result<Option<i32>, String>, log: &str) -> Option<&'static str> {
    again(status, log).then(|| if unsaid(status, log) { RERUN_UNSAID } else { RERUN })
}

/// The last run's part of a log.
fn last_run(text: &str) -> &str {
    rerun_cut(text).map_or(text, |(_, last)| last)
}

/// A shard's log line where its lease gave its pool slot to a run that goes first, and its gate
/// runs again ([`lease_run`]).
const YIELDED: &str = "bucket: this shard gave its pool slot to a run that goes first as its units ended; its gate runs again on the next slot";

/// The log's line of a full run's shard that gave its slot up before its lease was granted
/// ([`Board::give_queued`]): its ask withdrawn, it asks for a slot again in turn.
const WITHDREW: &str = "bucket: this shard gave its pool slot to a run that goes first before its lease was granted; it asks for a slot again in turn";

/// The `.log.dealt` record of a want a full run's lease was told dealt out to give its slot up
/// ([`Dealer::line`]).
const YIELD: &str = "yield";

/// Whether the gate's part of `log` now said its verdict (NODEGATE-OK or NODEGATE-FAIL): a part
/// that gave its slot up and ended without one (killed, cancelled) ends its shard as before, its
/// units dealt to the run's other shards ([`lease_run`]).
fn verdict_said(log: &Path) -> bool {
    this_part(&std::fs::read_to_string(log).unwrap_or_default()).lines().any(|l| l.starts_with("NODEGATE-OK") || l.starts_with("NODEGATE-FAIL"))
}

/// The gate's part of a log now: after its last [`RERUN`] or [`YIELDED`] line.
fn this_part(text: &str) -> &str {
    let at = [RERUN, RERUN_UNSAID, YIELDED, WITHDREW].iter().filter_map(|m| text.rfind(&format!("{m}\n")).map(|i| i + m.len() + 1)).max().unwrap_or(0);
    &text[at..]
}

/// What a log says of its run: each part a yield ended ([`YIELDED`]) and the last part's last run,
/// a lost lease's run before its [`RERUN`] cut ([`last_run`]).
fn kept(text: &str) -> String {
    let Some((before, after)) = rerun_cut(text) else { return text.to_string() };
    let done = before.rfind(&format!("{YIELDED}\n")).map_or("", |at| &before[..at + YIELDED.len() + 1]);
    format!("{}{after}", kept(done))
}

/// A run passes on exit 0 with NODEGATE-OK and no failing unit. One that ended at exit 0 or
/// without an exit status and says neither NODEGATE-OK nor NODEGATE-FAIL nor a failing unit is no
/// verdict on the landings: what is missing and where its log ended are said, and the bucket
/// takes the tip again (a gate the restarted master adopted had been killed by the Mac's reboot,
/// still queued for its lease, and read as a failed run of #2259, 11-06).
fn verdict(status: Result<Option<i32>, String>, text: &str, log: &Path) -> Outcome {
    let lost_run = lost(&status);
    // the failing units, and the failing steps no unit stands for (land_gate.sh step_fail)
    let mut failing: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("GATEUNIT fail ").or_else(|| l.strip_prefix("GATESTEP fail ")).map(|u| u.trim().to_string())).collect();
    // a unit a memory end stopped is a verdict on it, unless the run dealt it again and another
    // shard judged it: that verdict stands (a tests shard ended on a box with 3 GB free while three
    // Fab tests ran; they passed on the shards they went to next, and the run named the two
    // landings of its batch for them, 11-18)
    // (and so is one its lease was cancelled under while its watch said it hung, [`hung_ends`])
    let judged = judged_units(text);
    let memory: Vec<(String, String)> = memory_ends(text).into_iter().chain(hung_ends(text))
        .map(|(unit, said)| if judged.contains(&unit) { (String::new(), format!("{said}, dealt again and judged on another shard")) } else { (unit, said) })
        .collect();
    failing.extend(memory.iter().filter(|(unit, _)| !unit.is_empty()).map(|(unit, _)| unit.clone()));
    let ok = failing.is_empty() && status.as_ref().is_ok_and(|s| *s == Some(0)) && text.lines().any(|l| l.starts_with("NODEGATE-OK"));
    let mut unsaid = None;
    // NODEGATE-FAIL's words, only when the log marks nothing: a gate from before step_fail (a
    // probe of an older revision). Beside failing units its words are mostly theirs (`devhub-tests`
    // is `devhub makepad-devhub`), and the steps among them were lost: three runs red at statics
    // beside failing units read as its passes, and the next red named its seven landings, not
    // #5890 three runs before, which added the statics (11-16).
    if !ok && failing.is_empty() {
        for line in text.lines().filter_map(|l| l.strip_prefix("NODEGATE-FAIL")) {
            let steps = line.trim_start_matches(':').split(" [").next().unwrap_or("");
            failing.extend(steps.split_whitespace().map(str::to_string));
        }
        if failing.is_empty() {
            let end = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map_or("nothing".to_string(), |l| format!("`{}`", crate::proto::clean(l, 160)));
            match status {
                Ok(Some(0)) => unsaid = Some(format!("the gate exited 0 without a NODEGATE-OK or NODEGATE-FAIL line; its log ends at {end}")),
                Ok(None) => unsaid = Some(format!("the gate's process ended without an exit status (a signal, or a run adopted after a master restart that ended unwatched: a reboot) before its verdict; its log ends at {end}")),
                Ok(code) => failing.push(format!("no verdict (exit {code:?})")),
                Err(e) => failing.push(format!("gate lease: {e}")),
            }
        }
    }
    failing.sort();
    failing.dedup();
    let flakes = flake_lines(last_run(text)).into_iter().map(|(unit, tests)| format!("{unit}: a flake, red{}, then green on its rerun at this tip",
        if tests.is_empty() { String::new() } else { format!(" ({})", tests.join(", ")) }));
    let summary = text.lines().filter(|l| l.starts_with("STEP ")).map(str::to_string).chain(memory.into_iter().map(|(_, said)| said)).chain(flakes).collect::<Vec<_>>().join(", ");
    let no_verdict = if lost_run { Some("its lease was lost under the gate twice (the fleet, not the landings)".to_string()) } else { unsaid };
    Outcome { ok, failing, log: log.display().to_string(), summary, no_verdict }
}

/// The units a run's text judged (`GATEUNIT <ok|fail|proven> <unit>`).
fn judged_units(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(|l| l.strip_prefix("GATEUNIT ")?.split_once(' ').map(|(_, unit)| unit.trim().to_string())).collect()
}

/// The shards (by index in `parts`, each its status and log) that their boxes ended for memory
/// and whose every unit was judged: the units it was running then were dealt again (the dealer's
/// `back`) and judged on other shards, the rest by itself. Its exit is then no verdict on the
/// run: those units' verdicts are.
fn redealt_ends(parts: &[(&Result<Option<i32>, String>, &str)]) -> Vec<usize> {
    let judged: BTreeSet<String> = parts.iter().flat_map(|(_, part)| judged_units(part)).collect();
    parts.iter().enumerate().filter(|(_, (status, part))| {
        let ended = memory_ends(part);
        !ended.is_empty() && ended.iter().all(|(unit, _)| judged.contains(unit)) && unjudged(status, part).is_none()
    }).map(|(i, _)| i).collect()
}

/// The units a shard's lease was cancelled under while its watch said them stalled busy: a
/// `GATEBUSY <unit> :: <since> <sample> <says>` (since > 0) with no `GATEEND` or new result after
/// it, then its client's `LEASE CANCELLED`. Each with its line for the run's summary: since when
/// it gave no new result, and its watch's sample (what it said, and where its shard's keep dir
/// holds it, if it took it). The hang main or its holder ended is a verdict on the unit, as a
/// memory end is ([`memory_ends`]), not a unit left unjudged to run once more ([`unjudged`]): a
/// tests run's `plate_loading`, blocked 10 min when main cancelled its shard (L1459906), hung
/// 20 min more in the run's rerun of what its shards left (L1460075) before main cancelled that
/// too (11-20).
fn hung_ends(text: &str) -> Vec<(String, String)> {
    let (mut out, mut keep) = (Vec::new(), None);
    let mut busy: BTreeMap<&str, (u64, &str, &str)> = BTreeMap::new();
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("== shard ") {
            keep = rest.split_once(": ").map(|(_, log)| Path::new(log).with_extension("keep"));
            busy.clear();
        } else if l.starts_with("GATEQUEUE ") || granted(l).is_some() || l.starts_with("LEASE LOST") {
            busy.clear();
        } else if l.starts_with("LEASE CANCELLED") {
            let by = l.split_once(": ").map_or(l, |(by, _)| by).trim_start_matches("LEASE CANCELLED").trim();
            for (unit, (since, sample, says)) in std::mem::take(&mut busy) {
                let at = keep.as_ref().map(|k: &PathBuf| k.join(sample)).filter(|p| p.exists()).map_or(String::new(), |p| format!(" (kept in {})", p.display()));
                let says = if says.is_empty() { String::new() } else { format!(": {says}") };
                out.push((unit.to_string(), format!("{unit} hung: no new test result since {} when its lease was cancelled ({by}); its watch's sample {sample}{at}{says}", crate::logstamp::stamp(since * 1000))));
            }
        } else if let Some(unit) = l.strip_prefix("GATEEND ") {
            busy.remove(unit.trim_end());
        } else if let Some((unit, said)) = l.strip_prefix("GATEBUSY ").and_then(|r| r.split_once(" :: ")) {
            let mut w = said.splitn(3, ' ');
            match (w.next().and_then(|n| n.parse::<u64>().ok()), w.next()) {
                (Some(since), Some(sample)) if since > 0 => { busy.insert(unit, (since, sample, w.next().unwrap_or("").trim())); }
                _ => { busy.remove(unit); }
            }
        }
    }
    out
}

/// The units a gate's lease ran when its box ended it for memory (its client's `ended (memory)`
/// line; the units its shard had started and not ended, `GATESTART`/`GATEEND`), each with what
/// to say of it: a verdict on that unit, blamed on the landings that changed it. One with no
/// unit running is the gate's own (`its lease`). A tests shard ended at 45 GB in `test
/// makepad-run` read "no verdict (exit Some(1))" and named every lane of its batch (11-13).
fn memory_ends(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut running: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.starts_with("== shard ") {
            running.clear();
        } else if let Some(rest) = line.strip_prefix("GATESTART ") {
            running.push(rest.rsplit_once(' ').map_or(rest, |(unit, _)| unit));
        } else if let Some(unit) = line.strip_prefix("GATEEND ") {
            running.retain(|u| *u != unit);
        } else if line.contains(" ended (memory): ") {
            let held = line.split_once("it held ").and_then(|(_, r)| r.split([',', ';']).next()).map_or(String::new(), |gb| format!(" at {}", gb.trim()));
            if running.is_empty() { out.push(("its lease".to_string(), format!("its lease ended for memory{held} with no unit running"))); }
            for unit in running.drain(..) { out.push((unit.to_string(), format!("{unit} ended for memory{held}"))); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bisect_finds_each_first_red_and_stops_without_an_answer() {
        for (n, max) in [(1, 0), (2, 1), (3, 2), (5, 3), (8, 3)] {
            for red in 0..n {
                let (found, probes) = first_red(n, &mut |i| Some(i >= red)).unwrap();
                assert_eq!(found, red, "first red of {n}");
                assert!(probes.len() <= max, "{n} landings, first red {red}: {probes:?}");
                assert!(probes.iter().all(|(i, answer)| *i < n - 1 && *answer == (*i >= red)), "the tip's red is known: {probes:?}");
                assert!(found == 0 || probes.contains(&(found - 1, false)), "a later first red has a proven green parent: {probes:?}");
                assert!(found == n - 1 || probes.contains(&(found, true)), "a first red short of the tip is probed red: {probes:?}");
            }
        }
        let mut calls = Vec::new();
        let result = first_red(8, &mut |i| { calls.push(i); if i == 3 { Some(true) } else { None } });
        assert_eq!(result, Err((1, vec![(3, true)])));
        assert_eq!(calls, [3, 1], "no probes after the one with no answer");
    }

    fn bisect_fixture(d: &Path) -> (Vec<Pair>, Vec<Entry>, String) {
        use makepad_git::{Commit, ObjectId, Signature, Tree, TreeEntry};
        let _ = std::fs::remove_dir_all(d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        for (dir, branch) in [(d, "work"), (c.as_path(), "main")] {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["init", "-q", "--template=", "-b", branch])
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().unwrap();
            assert!(o.status.success(), "the fixture repository initializes: {}", String::from_utf8_lossy(&o.stderr));
        }
        // Objects in process keep the fixture small; only repository initialization starts git.
        let commit = |dir: &Path, parents: &[&str], text: &str| crate::gitread::with(dir, |_, repo| {
            let blob = repo.write_blob(text.as_bytes()).unwrap();
            let tree = repo.write_tree(&Tree { entries: vec![TreeEntry { name: "a.rs".into(), mode: 0o100644, oid: blob }] }).unwrap();
            let sig = Signature { name: String::new(), email: String::new(), timestamp: 0, tz_offset: "+0000".into() };
            repo.write_commit(&Commit { tree, parents: parents.iter().map(|s| ObjectId::from_hex(s).unwrap()).collect(), author: sig.clone(), committer: sig, message: text.into() }).unwrap().to_hex()
        }).unwrap();
        let (w0, m0) = (commit(d, &[], "base\n"), commit(&c, &[], "base\n"));
        let w1 = commit(d, &[&w0], "first\n");
        let ws = commit(d, &[&w0], "work side\n");
        let ms = commit(&c, &[&m0], "main side\n");
        let m1 = commit(&c, &[&m0, &ms], "second\n");
        let w2 = commit(d, &[&w1, &ws], "third\n");
        let pins = vec![Pair::new(&w0, &m0), Pair::new(&w1, &m0), Pair::new(&w1, &m1), Pair::new(&w2, &m1)];
        let entry = |id: u64, pre: &Pair, landed: &Pair, changed_ms| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: format!("work {} main {}", landed.work, landed.main), pre: format!("work {} main {}", pre.work, pre.main), detail: Vec::new(),
            agent: format!("bisect-{id}"), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms };
        let entries = vec![entry(10, &pins[1], &pins[2], 3), entry(11, &pins[2], &pins[3], 1), entry(12, &pins[0], &pins[1], 2),
            entry(98, &pins[0], &Pair::new(&w0, &ms), 4), entry(99, &pins[0], &Pair::new(&ws, &m0), 5)];
        (pins, entries, ws)
    }

    #[test]
    fn bisect_orders_landings_and_requires_proof_before_naming_a_lane() {
        let d = std::env::temp_dir().join(format!("devhub-bucket-bisect-proof-{}", std::process::id()));
        let (pins, entries, ws) = bisect_fixture(&d);
        let c = d.join("apps/commercial");
        let (w0, w1, w2) = (&pins[0].work, &pins[1].work, &pins[3].work);
        let (m0, m1) = (&pins[0].main, &pins[3].main);
        let store = Store::open_quiet(&d.join("store"), "").unwrap();
        store.update(|doc| doc.land = entries.iter().map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        let work = bisect_chain(&d, w0, w2).unwrap();
        let main = bisect_chain(&c, m0, m1).unwrap();
        assert_eq!(work, [w0.clone(), w1.clone(), w2.clone()], "work's base is position zero and second parents stay out");
        assert_eq!(main, [m0.clone(), m1.clone()], "main's base is position zero and second parents stay out");
        let (ordered, left_out) = order_landings(&work, &main, &[], &entries);
        assert_eq!(ordered.iter().map(|e| e.id).collect::<Vec<_>>(), [12, 10, 11], "paired chain positions order work-only and commercial-only landings, not ids or record times");
        assert_eq!(left_out, ["#98 is not on main's first-parent chain: left out", "#99 is not on work's first-parent chain: left out"], "each off-chain landing names the side that excluded it");
        assert!(bisect_chain(&d, &ws, w2).is_err(), "a base reachable only through a second parent is no first-parent base");
        let run = |case: &str, key: &str, line: &str, answers: [Option<bool>; 4], expected: Option<u64>| {
            let home = d.join(case);
            let job = Bisect { bucket: "fast".into(), unit: key.into(), key: key.into(), from: pin_of(&pins[0]), tip: pin_of(&pins[3]), line: line.into(), log: "known-red.log".into(), closed: case.starts_with("closed"), named: Vec::new(), tip_probe: None, units: Vec::new() };
            let calls = Arc::new(Mutex::new(Vec::new()));
            let (called, at, unit) = (calls.clone(), pins.clone(), key.to_string());
            let probe: Probe = Arc::new(move |_, base, tip, log| {
                let i = at.iter().position(|p| *p == tip.pair()).expect("only a chain candidate or the base is probed");
                assert_eq!(base.map(Pin::pair), Some(at[0].clone()), "{unit}: every probe is gated against the last pass, the chain's from");
                called.lock().unwrap().push(i);
                // (a passing run's line of its unit: a test's pass is said by it)
                std::fs::write(log, if answers[i] == Some(false) { format!("GATEUNIT ok {unit}\n") } else { String::new() }).unwrap();
                Outcome { ok: answers[i] == Some(false), failing: if answers[i] == Some(true) { vec![unit.clone()] } else { Vec::new() }, log: log.display().to_string(), summary: String::new(),
                    no_verdict: answers[i].is_none().then(|| "probe did not answer".into()) }
            });
            let feed = Mutex::new(Vec::new());
            run_bisect(&d, &home, &store, &|sev, line, detail| feed.lock().unwrap().push((sev, line, detail)), &probe, &|| panic!("{case}: only a leg waits for the night"), job).unwrap();
            let feed = feed.into_inner().unwrap();
            let rows = bisect_rows(&home).unwrap();
            assert_eq!(rows.len(), 1, "{case}: one completed ledger row");
            let row = rows.into_iter().next().unwrap();
            assert_eq!(row.get("state").and_then(Value::as_str), Some("done"), "{case}: the bisect is retired");
            assert_eq!(row.get("landing").and_then(Value::as_u64), expected, "{case}: only a proven landing is named");
            assert_eq!(feed.len(), 1, "{case}: one result event");
            assert_eq!(feed[0].0, if expected.is_some() { Sev::Err } else { Sev::Warn }, "{case}: doubtful attribution is a warning");
            assert_eq!(row.get("result").and_then(Value::as_str), Some(feed[0].1.as_str()), "{case}: the ledger keeps the result line");
            assert!(feed[0].1.contains("#99 is not on work's first-parent chain: left out") && feed[0].1.contains("#98 is not on main's first-parent chain: left out"), "{case}: the result explains both excluded landings");
            if let Some(id) = expected {
                assert!(feed[0].1.contains(&format!("#{} rc-l{} (bisect-{}) lane rc-l{} turned {key} red:", id, id, id, id)), "{case}: proven attribution keeps its wording");
            } else {
                assert!(!feed[0].1.contains(" lane ") && !feed[0].1.contains(" turned "), "{case}: a doubtful result blames no lane");
            }
            for agent in entries.iter().map(|e| (e.agent.as_str(), Some(e.id))).chain(std::iter::once(("main", None))) {
                let notes = crate::notes::take(&home, agent.0);
                let named = agent.0 == "main" || agent.1 == expected;
                assert_eq!(notes.len(), usize::from(named), "{case}: only main and a proven landing's agent get a note ({})", agent.0);
                if named { assert_eq!(notes[0].1, feed[0].1, "{case}: the note carries the result"); }
            }
            let calls = calls.lock().unwrap().clone();
            assert_eq!(row.get("probes").and_then(Value::as_arr).unwrap().len(), calls.len(), "{case}: all probes, including unanswered ones, survive in the ledger");
            (row, calls)
        };
        let (row, calls) = run("ordered", "check pkg", "error: failed", [Some(false), Some(false), Some(true), Some(true)], Some(10));
        assert_eq!(calls, [2, 1], "the first red is #10, with #12 probed green, despite both conflicting record orders");
        let probes = row.get("probes").and_then(Value::as_arr).unwrap();
        assert_eq!(probes.iter().map(|p| (p.get("id").and_then(Value::as_u64), p.get("red").and_then(Value::as_bool))).collect::<Vec<_>>(), [(Some(10), Some(true)), (Some(12), Some(false))], "the ledger proves the named landing's red and its parent's green");
        for (case, answer, text) in [("base-red", Some(true), "red at its last pass"), ("base-unknown", None, "attribution doubtful: its last pass")] {
            let (row, calls) = run(case, "build", "build failed", [answer, Some(true), Some(true), Some(true)], None);
            assert_eq!(calls, [2, 1, 0], "{case}: candidate zero needs a base probe");
            let result = row.get("result").and_then(Value::as_str).unwrap();
            assert!(result.contains(&format!("{text} {}", pins[0].short())), "{case}: the result identifies the doubtful last pass");
            assert!(result.contains(if answer.is_some() { "already; no landing of this range broke it" } else { "did not answer" }), "{case}: the result explains why nobody is blamed");
            let base = row.get("probes").and_then(Value::as_arr).unwrap().last().unwrap();
            assert_eq!(base.get("base").and_then(Value::as_bool), Some(true), "{case}: the base probe is distinguished from a landing");
            assert_eq!(base.get("red").and_then(Value::as_bool), answer, "{case}: the ledger retains the base's actual answer");
        }
        let (_, calls) = run("unit-base", "check pkg", "error: failed", [None, Some(true), Some(true), Some(true)], Some(12));
        assert_eq!(calls, [2, 1], "a unit trusts its explicit last pass without probing from");
        let (_, calls) = run("step-base", "statics", "no reset and no reason: crate::STATE (a.rs:1)", [Some(false), Some(true), Some(true), Some(true)], Some(12));
        assert_eq!(calls, [2, 1, 0], "a step names candidate zero only after its base is green");
        for key in SOURCE_STEPS {
            let (row, _) = run(&format!("{key}-miss"), key, "no reset and no reason: crate::STATE (untouched.rs:1)", [Some(false), Some(false), Some(true), Some(true)], None);
            assert!(row.get("result").and_then(Value::as_str).unwrap().contains("attribution doubtful: #10 (rc-l10) changed none of untouched.rs"), "{key}: an unrelated source change names no lane");
        }
        let (row, _) = run("source-hit", "statics", "no reset and no reason: crate::STATE (apps/commercial/a.rs:1)", [Some(false), Some(false), Some(true), Some(true)], Some(10));
        assert_eq!(row.get("landing").and_then(Value::as_u64), Some(10), "a commercial diff uses the apps/commercial prefix");
        let (row, _) = run("source-unnamed", "statics", "source failed", [Some(false), Some(false), Some(true), Some(true)], None);
        assert!(row.get("result").and_then(Value::as_str).unwrap().contains("attribution doubtful: #10 (rc-l10) changed none of the reported files"), "a source failure without a file cannot blame a lane");
        let (row, calls) = run("probe-unknown", "check pkg", "error: failed", [Some(false), None, Some(true), Some(true)], None);
        assert_eq!(calls, [2, 1], "an unanswered candidate stops the search");
        assert_eq!(row.get("probes").and_then(Value::as_arr).unwrap()[0].get("red").and_then(Value::as_bool), Some(true), "a stopped search keeps its earlier red probe");
        store.update(|doc| doc.land.retain(|v| v.get("id").and_then(Value::as_u64) != Some(11))).unwrap();
        let (_, calls) = run("tip-not-candidate", "check pkg", "error: failed", [Some(false), Some(false), Some(false), Some(true)], None);
        assert_eq!(calls, [2], "a last candidate short of the red tip must itself prove red before searching");
        store.update(|doc| doc.land = entries.iter().map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        // A test's red tip is probed first: green there, it is flaky or not reproduced and names no
        // landing (the owner's lookup finds no package here: main hears it); red, the search names
        // one; red down to candidate zero, its last pass is probed, and red there the same way it is
        // deterministic, no flake (11-23); one
        // a later run passed stops at its tip (11-17).
        let test = "test pkg";
        let (row, calls) = run("test-flaky", test, "the_test", [Some(false), Some(false), Some(true), Some(false)], None);
        assert_eq!(calls, [3], "only the red tip is probed");
        assert_eq!(row.get("flaky").and_then(Value::as_bool), Some(true), "the ledger says flaky");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains(&format!("flaky or not reproduced: green at its red tip {}", pins[3].short())) && result.contains("its owner unknown"), "{result}");
        let listed = std::fs::read_to_string(d.join("test-flaky").join(FLAKES)).unwrap_or_default();
        assert!(listed.lines().count() == 1 && listed.contains("known-red.log") && listed.contains("\"key\":\"test pkg\""), "the flake list keeps a bisect's flake with its red log: {listed}");
        let (row, calls) = run("test-red", test, "the_test", [Some(false), Some(false), Some(true), Some(true)], Some(10));
        assert_eq!(calls, [3, 2, 1], "the red tip, then the search");
        assert_eq!(row.get("probes").and_then(Value::as_arr).unwrap()[0].get("red_tip").and_then(Value::as_bool), Some(true), "the tip's probe is marked");
        let (row, calls) = run("test-red-at-its-pass", test, "the_test", [Some(true), Some(true), Some(true), Some(true)], None);
        assert_eq!(calls, [3, 2, 1, 0], "candidate zero of a test needs its last pass probed");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains(&format!("deterministic, not flaky: red the same way at every probe, its last pass {} too", pins[0].short())) && row.get("flaky").is_none(), "red the same way at every probe: {result}");
        assert!(!d.join("test-red-at-its-pass").join(FLAKES).exists(), "no flake");
        // red again there, and passed by a later run, its red tip is probed once more: red again, it
        // is deterministic there and fixed since (RELAX263, red at its tip every time and fixed by a
        // later landing, was listed intermittent, 11-24); green, the same revision went both ways:
        // intermittent, and `devhub bucket` says it until it is cleared or green again (main, 11-24)
        let (row, calls) = run("closed-test", test, "the_test", [Some(false), Some(false), Some(true), Some(true)], None);
        assert_eq!(calls, [3, 3], "a red a later run passed is probed at its tip, twice");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains(&format!("red at its red tip {0} on both probes, and a later run passed it: fixed since: after {0}", pins[3].short())) && !result.contains("intermittent"), "{result}");
        let again = row.get("probes").and_then(Value::as_arr).unwrap().last().unwrap();
        assert_eq!((again.get("again").and_then(Value::as_bool), again.get("red").and_then(Value::as_bool)), (Some(true), Some(true)), "the ledger keeps the second probe");
        assert!(row.get("flaky").is_none() && open_intermittent(&d.join("closed-test"), &[]).is_empty(), "deterministic: no flake, not intermittent");
        // with the passing run's tip in its row: red twice, fixed between the two tips, by the one
        // landing between them, which is told nothing (it fixed, it broke nothing); red, then green
        // there, intermittent and listed
        for (case, again) in [("fixed-between", true), ("red-then-green", false)] {
            let home = d.join(case);
            std::fs::create_dir_all(&home).unwrap();
            let red_started = 1795521038382u64;
            let passed = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(red_started as i64 + 1)), ("passed", Value::Arr(vec![s(test)])), ("tip", tip_value(&pin_of(&pins[3])))]);
            std::fs::write(home.join("buckets.jsonl"), format!("{}\n", passed.to_json())).unwrap();
            let job = Bisect { bucket: "tests".into(), unit: test.into(), key: test.into(), from: pin_of(&pins[0]), tip: pin_of(&pins[2]), line: "the_test".into(),
                log: home.join(format!("tests-{red_started}.log")).display().to_string(), closed: true, named: Vec::new(), tip_probe: None, units: Vec::new() };
            let asked = Arc::new(Mutex::new(Vec::new()));
            let (said, at) = (asked.clone(), pins.clone());
            let probe: Probe = Arc::new(move |unit, _, tip, log| {
                let mut said = said.lock().unwrap();
                said.push(at.iter().position(|p| *p == tip.pair()).unwrap());
                let red = said.len() == 1 || again;
                std::fs::write(log, if red { format!("FAILED the_test\nGATEUNIT fail {unit}\n") } else { format!("GATEUNIT ok {unit}\n") }).unwrap();
                Outcome { ok: !red, failing: if red { vec![unit.to_string()] } else { Vec::new() }, log: log.display().to_string(), summary: String::new(), no_verdict: None }
            });
            run_bisect(&d, &home, &store, &|_, _, _| {}, &probe, &|| panic!("{case}: no night"), job).unwrap();
            let row = bisect_rows(&home).unwrap().pop().unwrap();
            assert_eq!(*asked.lock().unwrap(), [2, 2], "{case}: its red tip, twice");
            let result = row.get("result").and_then(Value::as_str).unwrap();
            let listed = open_intermittent(&home, &[]);
            if again {
                assert!(result.contains(&format!("fixed since: between {} and {}, by #11 rc-l11 (bisect-11) lane rc-l11", pins[2].short(), pins[3].short())) && listed.is_empty(), "{case}: {result}");
                assert!(row.get("landing").is_none() && entries.iter().all(|e| crate::notes::take(&home, &e.agent).is_empty()), "{case}: a fix blames no landing");
            } else {
                assert!(result.contains(&format!("intermittent: reproduced at {}, passed at tests {}", pins[2].short(), pins[3].short())) && listed.len() == 1, "{case}: {result}");
            }
        }
        // A master's restart starts its bisect again: its probes' logs are its red run's, the same
        // again, and its lease runner adopts the probes the last image left by them; named by each
        // search's start, every restart asked for them once more beside the ones still held, and
        // the bisect lane starved (11-24). A red a later run passed, its own bucket's next one too,
        // stops its search before its next probe: fixed since, no landing told.
        let home = d.join("restarted");
        std::fs::create_dir_all(&home).unwrap();
        let red_started = 1795521038382u64;
        let job = Bisect { bucket: "tests".into(), unit: test.into(), key: test.into(), from: pin_of(&pins[0]), tip: pin_of(&pins[3]), line: "the_test".into(),
            log: home.join(format!("tests-{red_started}.log")).display().to_string(), closed: false, named: Vec::new(), tip_probe: None, units: Vec::new() };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (said, at, ledger_home) = (asked.clone(), pins.clone(), home.clone());
        let probe: Probe = Arc::new(move |unit, _, tip, log| {
            let i = at.iter().position(|p| *p == tip.pair()).unwrap();
            let mut said = said.lock().unwrap();
            said.push((i, log.to_path_buf()));
            // the tests bucket's next run passes it as the search probes its first landing
            if said.len() == 2 {
                let row = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(red_started as i64 + 1)), ("passed", Value::Arr(vec![s(unit)])), ("tip", tip_value(&pin_of(&at[3])))]);
                std::fs::write(ledger_home.join("buckets.jsonl"), format!("{}\n", row.to_json())).unwrap();
            }
            std::fs::write(log, format!("FAILED the_test\nGATEUNIT fail {unit}\n")).unwrap();
            Outcome { ok: false, failing: vec![unit.to_string()], log: log.display().to_string(), summary: String::new(), no_verdict: None }
        });
        let search = |job: Bisect| {
            run_bisect(&d, &home, &store, &|_, _, _| {}, &probe, &|| panic!("no night"), job).unwrap();
            bisect_rows(&home).unwrap().pop().unwrap()
        };
        let row = search(job.clone());
        let first: Vec<(usize, PathBuf)> = asked.lock().unwrap().drain(..).collect();
        assert_eq!(first.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [3, 2], "its red tip, then #10, as which a later run passed it: no more");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        let said = format!("intermittent: reproduced at {0}, passed at tests {0}", pins[3].short());
        assert!(result.contains(&said) && row.get("landing").is_none(), "red again at its red tip, then passed: intermittent ({result})");
        assert!(entries.iter().all(|e| crate::notes::take(&home, &e.agent).is_empty()), "no landing hears of it");
        assert_eq!(log_started(&first[0].1), Some(red_started), "its logs carry its red run's start: {}", first[0].1.display());
        // `devhub bucket` says it until a human or a lane clears it
        let ledger_rows = || ledger(&home, 100).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect::<Vec<Value>>();
        let said_open = || status(&home, &[]).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter(|v| v.get("intermittent").is_some()).collect::<Vec<Value>>();
        let open = said_open();
        assert!(open.len() == 1 && open[0].get("key").and_then(Value::as_str) == Some(test) && open[0].get("log").and_then(Value::as_str) == first[0].1.to_str(), "said with its tip log: {open:?}");
        assert!(clear_intermittent(&home, test).contains("cleared test pkg: 1 intermittent red"), "a lane clears it");
        assert!(said_open().is_empty() && clear_intermittent(&home, test).contains("no open intermittent red"), "cleared, it is said no more");
        let row = search(job);
        assert_eq!(*asked.lock().unwrap(), first[..1], "restarted, its red tip's probe has the same log, and no landing is probed");
        assert!(row.get("result").and_then(Value::as_str).unwrap().contains(&said));
        // or until its bucket's next three runs of it are green
        let ran = |at: u64, list: &str| {
            let row = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(at as i64)), (list, Value::Arr(vec![s(test)]))]);
            let mut file = std::fs::OpenOptions::new().append(true).open(home.join("buckets.jsonl")).unwrap();
            writeln!(file, "{}", row.to_json()).unwrap();
        };
        ran(red_started + 2, "passed");
        ran(red_started + 3, "failing");
        ran(red_started + 4, "passed");
        ran(red_started + 5, "passed");
        assert_eq!(open_intermittent(&home, &ledger_rows()).len(), 1, "red again in one of its last three runs: still said");
        ran(red_started + 6, "passed");
        assert!(open_intermittent(&home, &ledger_rows()).is_empty() && said_open().is_empty(), "green in its bucket's next three runs: said no more");
        // A test whose run failed several targets of its package probes them all: at its red tip
        // the search narrows to the ones red there again, and names their landing; one green there
        // is said as not reproduced. A probe that ran none of them has no answer: one that selected
        // nothing read as green at its red tip, and 12 makepad-fab targets #7560 broke went
        // unattributed as flaky (11-20).
        let multi = |case: &str, ran: bool| {
            let home = d.join(case);
            let (chrome, monitor) = ("test pkg --test chrome".to_string(), "test pkg --test monitor".to_string());
            std::fs::create_dir_all(&home).unwrap();
            let red_log = home.join("tests-1.log");
            std::fs::write(&red_log, "FAILED the_chrome_test\nGATEUNIT fail test pkg --test chrome\nFAILED the_monitor_test\nGATEUNIT fail test pkg --test monitor\n").unwrap();
            let job = Bisect { bucket: "tests".into(), unit: chrome.clone(), key: "test pkg".into(), from: pin_of(&pins[0]), tip: pin_of(&pins[3]), line: "the_chrome_test".into(),
                log: red_log.display().to_string(), closed: false, named: Vec::new(), tip_probe: None, units: vec![chrome.clone(), monitor.clone()] };
            let asked = Arc::new(Mutex::new(Vec::new()));
            let (said, at) = (asked.clone(), pins.clone());
            let probe: Probe = Arc::new(move |units, _, tip, log| {
                let i = at.iter().position(|p| *p == tip.pair()).unwrap();
                said.lock().unwrap().push((i, units.to_string()));
                // monitor red from #10 on (pins 2 and 3), chrome green everywhere
                let failing: Vec<String> = units.split(';').filter(|u| ran && u.ends_with("monitor") && i >= 2).map(str::to_string).collect();
                let passed: String = units.split(';').filter(|u| ran && !failing.iter().any(|f| f == u)).map(|u| format!("GATEUNIT ok {u}\n")).collect();
                std::fs::write(log, passed).unwrap();
                Outcome { ok: failing.is_empty(), failing, log: log.display().to_string(), summary: String::new(), no_verdict: None }
            });
            run_bisect(&d, &home, &store, &|_, _, _| {}, &probe, &|| panic!("{case}: no night"), job).unwrap();
            let row = bisect_rows(&home).unwrap().into_iter().next().unwrap();
            let asked = asked.lock().unwrap().clone();
            (row, asked)
        };
        let (row, asked) = multi("test-targets", true);
        let both = "test pkg --test chrome;test pkg --test monitor".to_string();
        assert_eq!(asked, [(3, both), (2, "test pkg --test monitor".into()), (1, "test pkg --test monitor".into())], "every target at the red tip, then the ones red there");
        assert_eq!(row.get("landing").and_then(Value::as_u64), Some(10), "the landing the red targets bisect to is named, not a test file's owner");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains("turned test pkg --test monitor red: the_monitor_test") && result.contains("green at its red tip, not reproduced: test pkg --test chrome"), "{result}");
        assert!(row.get("flaky").is_none(), "{result}");
        let (row, asked) = multi("test-targets-ran-nothing", false);
        assert_eq!(asked.len(), 1, "an empty probe stops at the red tip");
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(row.get("flaky").is_none() && row.get("landing").is_none() && result.contains("of test pkg --test chrome, --test monitor stopped at its red tip"), "{result}");
        // A red only an apps move causes: the landing of its apps half alone (its pair the one the
        // landing before it left) is a candidate, each probe pins apps where the landings up to it
        // left it, and the search names it, a test's or a unit's. Each probe took the red tip's
        // apps: every one was red, and the red went to a makepad landing or to nobody (11-23). A
        // test red at every probe, its last pass too, is flaky only when not the same way.
        let apps = d.join("apps/apps");
        std::fs::create_dir_all(&apps).unwrap();
        let o = std::process::Command::new("git").arg("-C").arg(&apps).args(["init", "-q", "--template=", "-b", "main"])
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().unwrap();
        assert!(o.status.success(), "the apps repository initializes: {}", String::from_utf8_lossy(&o.stderr));
        // (the fixture's pairs were made at time 0; the apps move after them, at 1)
        let commit = |parents: &[&str], text: &str, at: i64| crate::gitread::with(&apps, |_, repo| {
            use makepad_git::{Commit, ObjectId, Signature, Tree, TreeEntry};
            let blob = repo.write_blob(text.as_bytes()).unwrap();
            let tree = repo.write_tree(&Tree { entries: vec![TreeEntry { name: "fab.rs".into(), mode: 0o100644, oid: blob }] }).unwrap();
            let sig = Signature { name: String::new(), email: String::new(), timestamp: at, tz_offset: "+0000".into() };
            repo.write_commit(&Commit { tree, parents: parents.iter().map(|s| ObjectId::from_hex(s).unwrap()).collect(), author: sig.clone(), committer: sig, message: text.into() }).unwrap().to_hex()
        }).unwrap();
        let a0 = commit(&[], "fab\n", 0);
        let a1 = commit(&[&a0], "fab moved\n", 1);
        // #13 lands apps alone after #10, at #10's pair
        let mut alone = entries.iter().find(|e| e.id == 10).unwrap().clone();
        (alone.id, alone.branch, alone.agent, alone.pre) = (13, "-".into(), "bisect-13".into(), alone.landed.clone());
        alone.nested = vec![crate::land::Side { name: "apps".into(), branch: "rc-appsfix".into(), sha: a1.clone(), landed: a1.clone() }];
        store.update(|doc| doc.land = entries.iter().chain(std::iter::once(&alone)).map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        let moved_from = |case: &str, key: &str, from: Pin, red_at: &dyn Fn(&Pin) -> Option<&'static str>| {
            let home = d.join(case);
            let job = Bisect { bucket: "fast".into(), unit: key.into(), key: key.into(), from, tip: pin_of(&pins[3]).with("apps", &a1), line: "the_test".into(),
                log: "known-red.log".into(), closed: false, named: Vec::new(), tip_probe: None, units: Vec::new() };
            let asked = Arc::new(Mutex::new(Vec::new()));
            let (said, at, unit) = (asked.clone(), pins.clone(), key.to_string());
            let red: Vec<(Pin, Option<&'static str>)> = [pin_of(&pins[0]).with("apps", &a0), pin_of(&pins[1]).with("apps", &a0), pin_of(&pins[2]).with("apps", &a0), pin_of(&pins[2]).with("apps", &a1), pin_of(&pins[3]).with("apps", &a1)]
                .into_iter().map(|p| { let r = red_at(&p); (p, r) }).collect();
            let probe: Probe = Arc::new(move |_, _, tip, log| {
                let i = at.iter().position(|p| *p == tip.pair()).expect("a chain candidate or the base");
                let apps = tip.nested.get("apps").cloned().unwrap_or_default();
                said.lock().unwrap().push((i, apps));
                let way = red.iter().find(|(p, _)| p == tip).unwrap_or_else(|| panic!("{unit}: a probe at no landing's pin: {tip:?}")).1;
                std::fs::write(log, match way { Some(test) => format!("FAILED {test}\nGATEUNIT fail {unit}\n"), None => format!("GATEUNIT ok {unit}\n") }).unwrap();
                Outcome { ok: way.is_none(), failing: way.map(|_| vec![unit.clone()]).unwrap_or_default(), log: log.display().to_string(), summary: String::new(), no_verdict: None }
            });
            run_bisect(&d, &home, &store, &|_, _, _| {}, &probe, &|| panic!("{case}: no night"), job).unwrap();
            let row = bisect_rows(&home).unwrap().into_iter().next().unwrap();
            let asked = asked.lock().unwrap().clone();
            (row, asked)
        };
        let moved = |case: &str, key: &str, red_at: &dyn Fn(&Pin) -> Option<&'static str>| moved_from(case, key, pin_of(&pins[0]).with("apps", &a0), red_at);
        let apps_red = |p: &Pin| (p.nested.get("apps").map(String::as_str) == Some(a1.as_str())).then_some("the_test");
        for key in ["check pkg", "test pkg"] {
            let (row, asked) = moved(&format!("apps-{key}"), key, &apps_red);
            let result = row.get("result").and_then(Value::as_str).unwrap();
            assert_eq!(row.get("landing").and_then(Value::as_u64), Some(13), "{key}: the apps landing is named: {result}");
            assert!(result.contains(&format!("#13 -+apps=rc-appsfix (bisect-13) lane rc-appsfix turned {key} red")), "{key}: by its lane: {result}");
            assert!(asked.contains(&(2, a0.clone())) && asked.contains(&(2, a1.clone())), "{key}: #10 probed at its apps (green), #13 at its own (red): {asked:?}");
        }
        // a last pass that pinned no apps (the bucket followed it only later): its apps is where
        // apps stood at its time, never the red tip's. A CI red's every probe pinned apps at its
        // red tip beside the commercial of before Fab left it, and failed on two `fab-ai`
        // packages, its last pass too ("red at its last pass already", 11-24)
        let (row, asked) = moved_from("apps-unpinned", "check pkg", pin_of(&pins[0]), &apps_red);
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert_eq!(row.get("landing").and_then(Value::as_u64), Some(13), "the apps landing is named: {result}");
        assert_eq!(row.get("from").and_then(|f| f.get("nested")).and_then(|n| n.get("apps")).and_then(Value::as_str), Some(a0.as_str()), "its last pass at the apps of its time: {row:?}");
        assert_eq!(asked, [(2, a0.clone()), (2, a1.clone())], "#10 at the apps of its time (green), #13 at its own (red)");
        let (row, _) = moved("apps-every-probe", "test pkg", &|_| Some("the_test"));
        assert!(row.get("result").and_then(Value::as_str).unwrap().contains("deterministic, not flaky") && row.get("flaky").is_none(), "red the same way everywhere");
        let (row, _) = moved("apps-other-way", "test pkg", &|p| Some(if *p == pin_of(&pins[0]).with("apps", &a0) { "another_test" } else { "the_test" }));
        let result = row.get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains("flaky or not reproduced: red at its last pass") && result.contains("not the same way") && row.get("flaky").and_then(Value::as_bool) == Some(true), "red another way at its last pass: {result}");
        store.update(|doc| doc.land = entries.iter().map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        // A CI leg's red tip is probed first: green, its owner hears it is flaky with the JIT's note
        // from the leg's own log; red, the one landing its error lines name, else its range is
        // bisected, each probe after the night's wait, and only the landing it proves is told; a
        // resumed job bisects without probing its tip again (11-18)
        const LEG: &str = "smoke:linux:a.rs";
        let leg_run = |case: &str, named: Vec<u64>, answers: [Option<bool>; 4], tip_probe: Option<Value>| {
            let home = d.join(case);
            std::fs::create_dir_all(&home).unwrap();
            let (ci_log, own) = (home.join("ci-1.log"), home.join("smoke.log"));
            std::fs::write(&own, "boot\nJIT state at the failure: main waited on lazy compiles 99% of the probe\n").unwrap();
            let summary = if case == "leg-devhub" { "ended by devhub on a and on b: cargo build: hung, every process idle" } else { "boot failed" };
            std::fs::write(&ci_log, format!("fail {LEG}: {summary} ({})\nGATEUNIT fail {LEG}\n", own.display())).unwrap();
            let job = Bisect { bucket: CI.into(), unit: LEG.into(), key: LEG.into(), from: pin_of(&pins[0]), tip: pin_of(&pins[3]), line: "boot failed".into(), log: ci_log.display().to_string(), closed: case.starts_with("closed"), named, tip_probe, units: Vec::new() };
            queue_bisect(&home, &job).unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let waits = Arc::new(Mutex::new(Vec::new()));
            let (called, waited, at) = (calls.clone(), waits.clone(), pins.clone());
            let probe: Probe = Arc::new(move |unit, _, tip, log| {
                assert_eq!(unit, LEG);
                let i = at.iter().position(|p| *p == tip.pair()).expect("the red tip, a chain candidate or the base");
                called.lock().unwrap().push(i);
                std::fs::write(log, "").unwrap();
                Outcome { ok: answers[i] == Some(false), failing: if answers[i] == Some(true) { vec![unit.to_string()] } else { Vec::new() }, log: log.display().to_string(), summary: String::new(),
                    no_verdict: answers[i].is_none().then(|| "no box".into()) }
            });
            let night = || { let n = calls.lock().unwrap().len(); waited.lock().unwrap().push(n); };
            run_bisect(&d, &home, &store, &|_, _, _| {}, &probe, &night, job).unwrap();
            let rows = bisect_rows(&home).unwrap();
            let row = rows.last().unwrap().clone();
            assert_eq!(row.get("state").and_then(Value::as_str), Some("done"), "{case}: retired");
            let told: Vec<u64> = entries.iter().filter(|e| !crate::notes::take(&home, &e.agent).is_empty()).map(|e| e.id).collect();
            assert_eq!(crate::notes::take(&home, "main").len(), 1, "{case}: main hears every end");
            let (calls, waits) = (calls.lock().unwrap().clone(), waits.lock().unwrap().clone());
            // each bisect probe after a wait: the waits are the probe counts just before each
            let first = usize::from(!calls.is_empty() && calls[0] == 3);
            assert_eq!(waits, (first..calls.len()).collect::<Vec<_>>(), "{case}: every probe but the tip's waits for the night");
            let tipped = rows.iter().filter(|r| r.get("state").and_then(Value::as_str) == Some("tip-red")).count();
            (row.get("result").and_then(Value::as_str).unwrap().to_string(), row.get("landing").and_then(Value::as_u64), row.get("flaky").is_some(), told, calls, tipped)
        };
        let red_tip = [Some(false), Some(false), Some(true), Some(true)];
        let (line, landing, flaky, _, calls, _) = leg_run("leg-flaky", vec![10], [Some(false); 4], None);
        assert!(flaky && landing.is_none() && calls == [3] && line.contains("flaky or not reproduced: green at its red tip") && line.contains("the JIT at its failure: main waited on lazy compiles 99% of the probe"), "{line}");
        let (line, landing, _, told, calls, _) = leg_run("leg-named", vec![10], red_tip, None);
        assert!(landing == Some(10) && told == [10] && calls == [3] && line.contains("turned it red: its error lines name a file it changed"), "{line} {told:?}");
        // ended by devhub on two boxes and red again at its tip: a hang its owner reads, no landing
        // its error lines name, no night's bisect (11-22)
        let (line, landing, _, _, calls, _) = leg_run("leg-devhub", vec![10], red_tip, None);
        assert!(landing.is_none() && calls == [3] && line.contains("where devhub had ended it on two boxes"), "{line} {calls:?}");
        let (line, landing, _, told, calls, tipped) = leg_run("leg-unnamed", vec![], red_tip, None);
        assert!(landing == Some(10) && told == [10] && calls == [3, 2, 1] && tipped == 1 && line.contains("lane rc-l10 turned smoke:linux:a.rs red") && line.contains("probes: red tip"), "no error line names a landing: bisected to #10 ({line} {told:?} {calls:?})");
        let (line, landing, _, told, calls, _) = leg_run("leg-two", vec![10, 12], [Some(false), Some(true), Some(true), Some(true)], None);
        assert!(landing == Some(12) && told == [12] && calls == [3, 2, 1, 0], "two landings named: bisected to #12, its base proved green ({line} {told:?} {calls:?})");
        let (line, landing, _, told, calls, _) = leg_run("leg-base-red", vec![], [Some(true); 4], None);
        assert!(landing.is_none() && told.is_empty() && calls == [3, 2, 1, 0] && line.contains("red at its last pass") && line.contains("no landing of this range broke it"), "{line} {told:?}");
        let tip = obj(vec![("tip", tip_value(&pin_of(&pins[3]))), ("red", Value::Bool(true)), ("red_tip", Value::Bool(true))]);
        let (line, landing, _, told, calls, tipped) = leg_run("leg-resumed", vec![], red_tip, Some(tip));
        assert!(landing == Some(10) && told == [10] && calls == [2, 1] && tipped == 0, "a resumed job's tip is not probed again ({line} {calls:?})");
        let (line, landing, _, told, calls, _) = leg_run("closed-leg", vec![10], red_tip, None);
        assert!(landing.is_none() && told.is_empty() && calls == [3] && line.contains("fixed since, nobody told"), "{line} {told:?}");
        let (line, landing, _, told, _, _) = leg_run("leg-no-answer", vec![10], [None; 4], None);
        assert!(landing.is_none() && told.is_empty() && line.contains("no answer at its red tip: no box"), "{line}");
        // a test's owner: the landing that brought its file's last change, each repository's side,
        // found by the landings' record times (here in their chain's order, the side ones after)
        let timed: Vec<Entry> = entries.iter().cloned().map(|mut e| { e.changed_ms = [(12, 1), (10, 2), (11, 3), (98, 4), (99, 5)].iter().find(|(id, _)| *id == e.id).unwrap().1; e }).collect();
        assert_eq!(owner_of(&d, &timed, &pins[3], "a.rs").map(|e| e.id), Some(11), "work's a.rs last changed in #11's merge");
        assert_eq!(owner_of(&d, &timed, &pins[3], "apps/commercial/a.rs").map(|e| e.id), Some(10), "commercial's in #10's");
        assert!(owner_of(&d, &timed, &pins[3], "b.rs").is_none(), "no file, no owner");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A CI leg's red closes once each of its units has a committed proof on the leg's OS from a run
    /// at a tip after the red's: three smoke reds stayed STILL FAILING for days after each had passed
    /// again (11-18). A proof at the red tip itself, an older one, another OS's, or a red newer than
    /// the proof keeps it open.
    #[test]
    fn a_leg_red_closes_on_a_proof_from_a_later_tip() {
        let (a, b, c) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let proof = |os: &str, work: &str| format!("[app.{os}]\ncapture = \"\"\ndate = \"2026-11-17\"\ndigest = \"{}\"\nmain = \"\"\nwork = \"{work}\"\n", "d".repeat(64));
        let toml = format!("{}{}", proof("macos", &b), proof("windows", &c));
        let tree = crate::orch::ci::Tree::mem(vec![("tools/ci_leg/Cargo.toml", ""), ("apps/x/ci.splash", ""), ("apps/x/ci.toml", &toml)]);
        let proofs = crate::orch::ci::leg_proofs(&tree, &["smoke:macos:apps/x", "smoke:linux:apps/x", "smoke:linux:apps/gone"]);
        assert_eq!(proofs["smoke:macos:apps/x"].iter().map(|u| u.iter().map(|p| p.pair.work.clone()).collect::<Vec<_>>()).collect::<Vec<_>>(), [[b.clone()]], "one unit, its macOS proof alone");
        assert_eq!(proofs["smoke:linux:apps/x"], [Vec::<crate::orch::ci::Proof>::new()], "no Linux proof: a unit with none");
        assert!(!proofs.contains_key("smoke:linux:apps/gone"), "a leg no plan has is left out");
        // the commits in order a, b, c: a proof is after a red when its tip comes later
        let order = |sha: &str| [&a, &b, &c].iter().position(|x| x.as_str() == sha);
        let after = |red: &Pair, p: &Pair| order(&p.work) > order(&red.work);
        let row = |failing: &str, work: &str| obj(vec![("bucket", s(CI)), ("failing", Value::Arr(vec![s(failing)])), ("tip", obj(vec![("work", s(work)), ("main", s(&"e".repeat(40)))]))]);
        let leg = "smoke:macos:apps/x";
        assert!(proven_since(&[row(leg, &a)], leg, proofs.get(leg), &after), "red at a, proved at b: closed");
        assert!(!proven_since(&[row(leg, &b)], leg, proofs.get(leg), &after), "proved at its own red tip: open");
        assert!(!proven_since(&[row(leg, &c), row(leg, &a)], leg, proofs.get(leg), &after), "red again at c, after the proof: open");
        assert!(!proven_since(&[row("smoke:linux:apps/x", &a)], "smoke:linux:apps/x", proofs.get("smoke:linux:apps/x"), &after), "no proof on its OS (Windows' is no Linux pass): open");
        assert!(!proven_since(&[row("smoke:linux:apps/gone", &a)], "smoke:linux:apps/gone", proofs.get("smoke:linux:apps/gone"), &after), "no plan: open");
    }

    /// A flaky test's owner is told what the JIT said as it failed, and which file is its: the red
    /// run's note from the unit's own lines (a quoted panic's too), and the file that defines the
    /// failing test in its target's files (11-17).
    #[test]
    fn a_flaky_tests_owner_reads_its_jit_note_and_its_file() {
        let note = "compiling: main is not stuck: it waited on its lazy compiles 99% of the 200 ms probe (335 first compiles finished in it)";
        let log = format!("GATEUNIT ok test a --lib\nFAILED the_test\n  panicked at t.rs:1:1 -- TestError {{ message: \"guard\\n  JIT state at the failure: {note} -- see `jit` [report r.txt]\" }}\n\
            GATEUNIT fail test p --test t\nJIT state at the failure: another unit's\nGATEUNIT fail test q --lib\n");
        assert_eq!(jit_note(&log, "test p --test t").as_deref(), Some(note), "its own lines' note, up to the quote's end");
        assert_eq!(jit_note(&log, "test q --lib").as_deref(), Some("another unit's"), "a line of its own");
        assert_eq!(jit_note(&log, "test a --lib"), None, "a unit that passed has none");
        let tree = crate::orch::ci::Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"app\"]\n"),
            ("app/Cargo.toml", "[package]\nname = \"pkg\"\n"),
            ("app/src/lib.rs", "mod a;\n"),
            ("app/src/a.rs", "mod tests {\n    fn a_lib_test() {}\n}\n"),
            ("app/tests/t.rs", "mod util;\nfn other() {}\n"),
            ("app/tests/t/util.rs", "fn the_test() {}\n"),
        ]);
        assert_eq!(test_file(&tree, "test pkg --test t", "the_test").as_deref(), Some("app/tests/t/util.rs"), "the target's file that defines it");
        assert_eq!(test_file(&tree, "test pkg --test t", "gone").as_deref(), Some("app/tests/t.rs"), "else the target's own file");
        assert_eq!(test_file(&tree, "test pkg --lib", "a::tests::a_lib_test").as_deref(), Some("app/src/a.rs"), "a unit test by its last segment");
        assert_eq!(test_file(&tree, "test pkg", "gone"), None, "a lib's test it cannot find has no file");
        assert_eq!(test_file(&tree, "test nopkg --test t", "the_test"), None);
        // a CI leg's: its failing test's, a smoke's script or app folder
        assert_eq!(leg_file(&tree, "test:pkg", "test pkg --release: exit 101: a::tests::a_lib_test, b").as_deref(), Some("app/src/a.rs"), "the step's first failing test");
        assert_eq!(leg_file(&tree, "smoke:linux:app", "boot failed").as_deref(), Some("app"), "an app with no script of its own: its folder");
        let scripted = crate::orch::ci::Tree::mem(vec![("app/ci.splash", "")]);
        assert_eq!(leg_file(&scripted, "smoke:macos:app", "").as_deref(), Some("app/ci.splash"), "its own script");
        assert_eq!(jit_note_of("x JIT state at the failure: main waited \\n more").as_deref(), Some("main waited"), "a note ends at the quoted newline");
    }

    #[test]
    fn a_bisect_ends_with_a_landing_or_a_reason() {
        let d = std::env::temp_dir().join(format!("devhub-bucket-bisect-terminal-{}", std::process::id()));
        let (pins, entries, ws) = bisect_fixture(&d);
        let home = d.join("home");
        let started = 1794010109906;
        let store = Store::open_quiet(&d.join("store"), "").unwrap();
        store.update(|doc| doc.land = entries.iter().map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        let job = Bisect { bucket: "fast".into(), unit: "check pkg".into(), key: "check pkg".into(), from: pin_of(&Pair::new(&ws, &pins[0].main)),
            tip: pin_of(&pins[3]), line: "error: failed".into(), log: format!("fast-{started}.log"), closed: false, named: Vec::new(), tip_probe: None, units: Vec::new() };
        // each probe's log is its bucket's, its key's and its red run's: the same in a restarted
        // master, which adopts the probes by them (11-24)
        let logs = ["from", "12", "10", "11", "tip"].map(|at| bisect_log(&home.join("bucket-logs/bisect"), &job, bisect_start(&job), at));
        assert_eq!(logs.iter().collect::<BTreeSet<_>>().len(), 5, "the base, three candidates and the red tip have distinct logs");
        for log in &logs {
            assert_eq!(log_started(log), Some(started), "each probe's lease reads the red run's start from {}", log.display());
        }
        let other = Bisect { key: "check other".into(), ..job.clone() };
        assert_ne!(bisect_log(&home, &other, started, "tip"), bisect_log(&home, &job, started, "tip"), "another red of the same run has logs of its own");
        assert!(queue_bisect(&home, &job).unwrap(), "the off-chain job starts queued");
        let probe: Probe = Arc::new(|_, _, _, _| panic!("an off-chain base must stop before probing"));
        let feed = Mutex::new(Vec::new());
        let emit = |sev, line, detail| feed.lock().unwrap().push((sev, line, detail));
        bisect_job(&d, &home, &store, &emit, &probe, job.clone());
        let rows = bisect_rows(&home).unwrap();
        let done: Vec<_> = rows.iter().filter(|v| v.get("state").and_then(Value::as_str) == Some("done")).collect();
        assert_eq!(done.len(), 1, "a setup error writes exactly one terminal record");
        let result = done[0].get("result").and_then(Value::as_str).unwrap();
        assert!(result.contains("bisect unresolved") && result.contains("is not on") && result.contains("first-parent chain"), "the terminal result names the first-parent reason: {result}");
        assert!(done[0].get("landing").is_none(), "an unresolved search names no landing");
        assert!(done[0].get("probes").and_then(Value::as_arr).unwrap().is_empty(), "a setup failure made no probes");
        let events = feed.lock().unwrap();
        assert_eq!(events.len(), 1, "a setup error emits exactly one event");
        assert_eq!((events[0].0, events[0].1.as_str()), (Sev::Warn, result), "the warning carries the terminal result");
        drop(events);
        let notes = crate::notes::take(&home, "main");
        assert_eq!(notes.len(), 1, "main gets one unresolved note");
        assert_eq!(notes[0].1, result, "the note keeps the reason");
        assert!(pending_bisects(&home, "fast").unwrap().is_empty(), "a restarted worker does not retry the finished job");

        let blocked = d.join("blocked-ledger");
        std::fs::create_dir_all(blocked.join("bisects.jsonl")).unwrap();
        feed.lock().unwrap().clear();
        bisect_job(&d, &blocked, &store, &emit, &probe, job.clone());
        let events = feed.lock().unwrap();
        assert_eq!(events.len(), 1, "a failed terminal write still emits one event");
        assert_eq!(events[0].0, Sev::Warn, "a ledger failure is a warning");
        assert!(events[0].1.contains("bisect unresolved") && events[0].1.contains("first-parent chain") && events[0].1.contains("bisect ledger:"), "the event keeps both the setup and ledger errors: {}", events[0].1);
        drop(events);

        let rev = format!("work@{}", pins[1].work);
        for text in ["GATEUNIT fail check pkg\n", "GATEUNIT ok check pkg\nNODEGATE-OK\n"] {
            let outcome = probe_verdict(Ok(Some(crate::lease::client::CANCELLED_EXIT)), text, &logs[1], &rev);
            assert_eq!(probe_says(&job.key, &outcome, text), None, "a cancelled probe's partial evidence gives no answer");
            assert!(!outcome.ok && outcome.failing.is_empty(), "cancellation is classified before red or green evidence");
            assert_eq!(outcome.no_verdict.as_deref(), Some(format!("probe {rev} cancelled before it completed ({})", logs[1].display()).as_str()), "the reason names the cancelled revision and log");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bisect_reads_both_source_location_forms() {
        let files = bisect_files("error: libs/example/src/lib.rs:12:4: missing value\n\
            no reset and no reason: relax_compiler::jit::TEST_PROGRESS (libs/relax/compiler/src/jit.rs:1643)\n\
            no reset and no reason: example::STATE (libs/example/src/lib.rs:12)\n\
            no reset and no reason: example::OTHER (apps/commercial/example/src/lib.rs:7)\n\
            no file here (not a path:row)\n");
        assert_eq!(files, ["apps/commercial/example/src/lib.rs", "libs/example/src/lib.rs", "libs/relax/compiler/src/jit.rs"], "both location forms yield distinct paths, without the diagnostic prefix");
        assert!(bisect_files("error: no location (file.rs:) (file.rs:bad)").is_empty(), "missing or nonnumeric rows are not source locations");
        assert!(touches("checkout/libs/example/src/lib.rs", &files[1]), "a diagnostic's checkout prefix still matches the landing's relative path");
        assert!(!touches("checkout/notlibs/example/src/lib.rs", &files[1]), "a suffix inside a path component is no match");
    }

    /// The windows bucket checks the packages checked on Windows that the landings reach, and its
    /// red ones again; its units read a Windows cargo check's own lines, `\` paths and all, as the
    /// readers of a run's reds take them.
    #[test]
    fn the_windows_bucket_checks_what_the_landings_reach_and_reads_its_cargo_check() {
        let tree = crate::orch::ci::Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"libs/base\", \"libs/hub\", \"libs/unix\", \"apps/app\", \"libs/jit\"]\n"),
            ("libs/base/Cargo.toml", "[package]\nname = \"base\"\n[package.metadata.makepad.platform-gaps]\nwindows = {}\n[package.metadata.makepad.windows-tests]\nlib = \"its Win32 clipboard\"\n"),
            ("libs/base/src/lib.rs", ""),
            ("libs/hub/Cargo.toml", "[package]\nname = \"hub\"\n[lib]\nname = \"hub_core\"\n[dependencies]\nbase = { path = \"../base\" }\n[package.metadata.makepad.platform-gaps]\nwindows = { \"lib\" = \"2 of 9 fail there\" }\n[package.metadata.makepad.windows-tests]\n\"test:ui-hang\" = \"its watchdog\"\n[package.metadata.makepad.windows-tests.lib]\nwhy = \"its Win32 drag\"\nknown = \"fixq\"\nskip = [\n  \"a::b\",\n  \"a::c\",\n]\n"),
            ("libs/hub/src/lib.rs", ""),
            ("libs/unix/Cargo.toml", "[package]\nname = \"unix\"\n[package.metadata.makepad.platform-gaps]\nwindows = { \"build\" = \"its Unix-only deps\" }\n"),
            ("libs/unix/src/lib.rs", ""),
            ("apps/app/Cargo.toml", "[package]\nname = \"app\"\n[dependencies]\nhub = { path = \"../../libs/hub\" }\n"),
            ("apps/app/src/main.rs", ""),
            ("libs/jit/Cargo.toml", "[package]\nname = \"jit\"\n[package.metadata.makepad.windows-tests]\n\"corpus:tests/name_cut.rs\" = \"its thread names\"\n\"corpus:tests/tls.rs\" = \"its thread-local keys\"\n\"corpus:tests/gone.rs\" = \"a file it lost\"\n\"corpus:../x.rs\" = \"a path out of it\"\n\"corpus:tests/skips.rs\" = { why = \"a skip list\", skip = [\"a\"] }\n"),
            ("libs/jit/src/lib.rs", ""),
            ("libs/jit/tests/name_cut.rs", ""),
            ("libs/jit/tests/tls.rs", ""),
            ("libs/jit/tests/skips.rs", ""),
        ]);
        let reach = |paths: &[&str], again: &[&str]| -> Vec<String> {
            let again: BTreeSet<String> = again.iter().map(|p| p.to_string()).collect();
            windows_reach(&tree, &paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(), &again).0.into_iter().map(|(p, _)| p).collect()
        };
        assert_eq!(reach(&["libs/base/src/lib.rs"], &[]), ["base", "hub"], "a crate it touches, and one checked there that depends on it");
        assert_eq!(reach(&["libs/hub/src/lib.rs"], &[]), ["hub"], "a lib gap is its tests': it is checked there");
        assert!(reach(&["apps/app/src/main.rs", "libs/unix/src/lib.rs"], &[]).is_empty(), "a package not checked there, and one with a build gap, are none");
        assert_eq!(reach(&["apps/app/src/main.rs"], &["base"]), ["base"], "a red one again, whatever the landings reach");
        assert_eq!(reach(&["Cargo.lock"], &[]), ["base", "hub"], "a workspace-wide file reaches every one");
        let again: BTreeSet<String> = ["base", "unix", "left"].iter().map(|p| p.to_string()).collect();
        assert_eq!(windows_reach(&tree, &[], &again).1, ["left", "unix"], "a red one with a build gap now, or gone from the tree, is checked there no more");
        let (_, passed) = unit_lines("GATEUNIT gone windows unix\n");
        assert!(passed.contains("windows unix"), "and its red closes");
        let pkgs = vec![("base".to_string(), "libs/base".to_string()), ("hub".to_string(), "libs/hub".to_string())];
        let units = |code: i32, text: &str| windows_units(&Ok(Some(code)), text, &pkgs);
        // a package that could not compile: its own lines with `/` paths, its unit red, its error's file
        let broke = "    Checking base v0.1.0\nlibs\\hub\\src\\serial.rs:805:22: error[E0599]: no method named `ping` found for mutable reference `&mut ()`\nerror: could not compile `hub` (lib) due to 1 previous error\n";
        let said = units(101, broke);
        assert!(said.starts_with("libs/hub/src/serial.rs:805:22: error[E0599]: no method named `ping`") && said.contains("GATEUNIT fail windows hub\n") && !said.contains("windows base\n"), "the failing package alone, its line with `/`: {said}");
        assert!(said.ends_with("NODEGATE-FAIL: windows hub\n"), "{said}");
        let v = verdict(Ok(Some(101)), &format!("{broke}{said}"), Path::new("windows.log"));
        assert_eq!((v.ok, v.failing.as_slice()), (false, &["windows hub".to_string()][..]), "its red unit, as a run's verdict reads it");
        assert!(is_unit("windows hub") && unit_key("windows hub") != unit_key("check hub"), "a unit of its own, apart from the Linux checks' passes");
        assert_eq!(error_files(&said).get("windows hub"), Some(&vec!["libs/hub/src/serial.rs".to_string()]), "blamed by its error's file");
        // a warning of its own turns a package red (zero warnings); one in a dependency outside them does not
        let warned = "libs\\hub\\src\\a.rs:3:9: warning: unused variable: `x`\nC:\\cargo\\registry\\dep\\src\\lib.rs:1:1: warning: use of deprecated item\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.09s\n";
        let said = units(0, warned);
        assert!(said.contains("GATEUNIT ok windows base\n") && said.contains("GATEUNIT fail windows hub\n") && !said.contains("registry"), "{said}");
        assert_eq!(error_files(&said).get("windows hub"), Some(&vec!["libs/hub/src/a.rs".to_string()]), "a warning's file, as a check's red takes it");
        let said = units(0, "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.09s\n");
        assert_eq!(said, "GATEUNIT ok windows base\nGATEUNIT ok windows hub\nNODEGATE-OK\n");
        assert!(verdict(Ok(Some(0)), &said, Path::new("windows.log")).ok, "a clean check passes");
        // a cargo check that failed naming none of them fails them all; a lease that ran no cargo says nothing
        let said = units(101, "error: failed to load manifest for workspace member `libs/hub`\n");
        assert!(said.contains("GATEUNIT fail windows base\n") && said.contains("GATEUNIT fail windows hub\n") && said.ends_with("NODEGATE-FAIL: windows base hub\n"), "{said}");
        assert_eq!(units(1, "devhub lease: no box of os=windows is up\n"), "");
        assert_eq!(windows_units(&Err("lost".into()), "", &pkgs), "");

        // the windows-tests bucket: the test targets a package declares for Windows, each a unit,
        // run where the landings reach them or still red there; no other run tests `cfg(windows)`
        // code, and 14 tests of two libs had gone red there unseen (11-24)
        let wreach = |paths: &[&str], again: &[&str]| -> Vec<String> {
            let again: BTreeSet<String> = again.iter().map(|p| p.to_string()).collect();
            windows_tests_reach(&tree, &paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(), &again).0.iter().map(WinUnit::name).collect()
        };
        assert_eq!(wreach(&["libs/base/src/lib.rs"], &[]), ["wintest base/lib", "wintest hub/lib", "wintest hub/test:ui-hang"], "a crate it touches, and one that depends on it");
        assert_eq!(wreach(&["libs/hub/src/lib.rs"], &[]), ["wintest hub/lib", "wintest hub/test:ui-hang"]);
        assert!(wreach(&["apps/app/src/main.rs"], &[]).is_empty(), "a package that declares none");
        assert_eq!(wreach(&["apps/app/src/main.rs"], &["base/lib"]), ["wintest base/lib"], "a red one again, whatever the landings reach");
        let again: BTreeSet<String> = ["hub/lib", "left/lib"].iter().map(|p| p.to_string()).collect();
        assert_eq!(windows_tests_reach(&tree, &[], &again).1, ["left/lib"], "a red one declared no more");
        // (its cargo targets: its corpus files run in a leg of their own, below)
        let (all, corpus): (Vec<WinUnit>, Vec<WinUnit>) = windows_tests_reach(&tree, &["Cargo.lock".to_string()], &BTreeSet::new()).0.into_iter().partition(|u| u.corpus().is_none());
        assert_eq!(corpus.len(), 5, "a workspace-wide file reaches the corpus files too");
        assert_eq!((all[1].binary.as_str(), all[1].skip.as_slice(), all[2].binary.as_str()), ("hub_core", &["a::b".to_string(), "a::c".to_string()][..], "ui_hang"), "its binary by its lib's name, a test's with `_`; its known reds");
        assert!(is_unit("wintest hub/lib") && unit_key("wintest hub/lib") != unit_key("wintest hub/test:ui-hang"), "each target a unit of its own");
        // one plain cargo test (argv: a Windows run is never a shell's), the known reds left out by name
        let argv = windows_tests_argv(&all, &[]);
        assert_eq!(argv.join(" "), "cargo test --no-fail-fast --message-format short -p base -p hub --lib --test ui-hang -- --exact --skip a::b --skip a::c");
        assert_eq!(windows_tests_argv(&all[1..2], &["a::d".into()]).join(" "), "cargo test --no-fail-fast --message-format short -p hub --lib -- --exact a::d --skip a::b --skip a::c", "a rerun: its failing tests alone");
        // its output, by test binary: passed, tests failed (with their panics), or a binary that ended
        // before its result
        let out = "     Running unittests src\\lib.rs (target\\debug\\build\\base/0a1b2c3d4e5f6071\\out\\base-0a1b2c3d4e5f6071.exe)\nrunning 2 tests\n\
            test inner::child ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 517 filtered out\ntest x::one ... ok\ntest result: ok. 2 passed; 0 failed\n\
            Running unittests src\\lib.rs (target\\debug\\deps\\hub_core-1234abcd.exe)\ntest a::d ... FAILED\ntest a::e ... ok\n---- a::d stdout ----\n\
            thread 'a::d' panicked at libs\\hub\\src\\lib.rs:10:5:\nerror of its own: assertion failed\ntest result: FAILED. 1 passed; 1 failed\nerror: test failed, to rerun pass `-p hub --lib`\n\
            Caused by:\n  process didn't exit successfully: `C:\\t\\deps\\hub_core-1234abcd.exe --exact` (exit code: 101)\n\
            Running tests\\ui-hang.rs (target\\debug\\deps\\ui_hang-99aa.exe)\nerror: test failed, to rerun pass `-p hub --test ui-hang`\nCaused by:\n\
            process didn't exit successfully: `C:\\t\\deps\\ui_hang-99aa.exe --exact` (exit code: 0xc0000005, STATUS_ACCESS_VIOLATION)\n";
        let first = win_runs(&Ok(Some(101)), out, &all).unwrap();
        assert_eq!(first[0], WinRun::Passed, "by its last result: a test's child prints its own in the binary's section");
        assert_eq!(first[1], WinRun::Failed(vec!["a::d".into()], vec!["panicked at libs\\hub\\src\\lib.rs:10:5: error of its own: assertion failed".into()]), "its panic's message, whatever it starts with");
        assert!(matches!(&first[2], WinRun::Broke(why, lines) if why == "its test binary ended before its result" && lines.len() == 1 && lines[0].contains("STATUS_ACCESS_VIOLATION")), "{:?}", first[2]);
        assert_eq!((win_runs(&Ok(Some(1)), "devhub lease: no box of os=windows is up\n", &all), win_runs(&Err("lost".into()), "", &all)), (None, None), "a lease that ran no cargo: its own verdict");
        // a package that did not build is its red; the others it stopped get no verdict
        let unbuilt = win_runs(&Ok(Some(101)), "libs\\hub\\src\\lib.rs:3:9: error[E0425]: cannot find value `x`\nerror: could not compile `hub` (lib test) due to 1 previous error\n", &all).unwrap();
        assert!(matches!(&unbuilt[1], WinRun::Broke(why, lines) if why == "it did not build on Windows" && lines[0].starts_with("libs/hub/src/lib.rs:3:9: error[E0425]")), "{:?}", unbuilt[1]);
        assert_eq!(unbuilt[0], WinRun::NotRun);
        // the units' lines: red on its rerun too names the landings, green on its rerun is a flake
        let (said, ok) = windows_tests_said(&all, &first, &[None, Some(first[1].clone()), Some(WinRun::Passed)]);
        assert!(!ok && said.starts_with("STEP windows-tests: 3 target(s) on Windows\nGATEUNIT ok wintest base/lib\n"), "{said}");
        assert!(said.contains("wintest hub/lib: leaves out its 2 known red(s) on Windows by their names, until fixq lands their fixes\n") && said.contains("GATEREPRO wintest hub/lib :: makepad devhub test <lane> hub --lib --needs os=windows --native \"its Windows red\" -- --exact a::d\n"), "{said}");
        assert!(said.contains("GATEFLAKE wintest hub/test:ui-hang :: \nGATEUNIT ok wintest hub/test:ui-hang\n") && said.ends_with("NODEGATE-FAIL: windows-tests wintest hub/lib\n"), "{said}");
        let v = verdict(Ok(Some(1)), &format!("{out}{said}"), Path::new("windows-tests.log"));
        assert_eq!((v.ok, v.failing.as_slice()), (false, &["wintest hub/lib".to_string()][..]), "its red unit, as a run's verdict reads it");
        let (tests, _) = test_lines(&format!("{out}{said}"));
        assert_eq!(tests.get("wintest hub/lib"), Some(&vec!["a::d".to_string()]), "its failing test, none of the cargo output's own lines before its STEP");
        assert_eq!(panic_lines(&format!("{out}{said}")).get("wintest hub/lib").map(Vec::len), Some(1));
        let (said, ok) = windows_tests_said(&all[..1], &first[..1], &[None]);
        assert!(ok && said.ends_with("GATEUNIT ok wintest base/lib\nNODEGATE-OK\n") && verdict(Ok(Some(0)), &said, Path::new("windows-tests.log")).ok, "{said}");
        // its known reds run alone, judging nothing: one that passes now is said, and main hears it
        // once, to take it off the list; the list kept itself honest only while someone relayed each
        // fix (main, 11-24)
        assert_eq!(known_argv(&all[1..2]).join(" "), "cargo test --no-fail-fast --message-format short -p hub --lib -- --exact a::b a::c", "the known reds alone, none left out");
        let ran = test_outcomes("test a::b ... ok\ntest a::c ... ok\ntest a::c ... FAILED\n");
        assert_eq!(ran, BTreeMap::from([("a::b".to_string(), true), ("a::c".to_string(), false)]), "its last line decides: a child's comes first");
        let (lines, passes) = known_reds_said(&all[1..2], Some(&ran));
        assert_eq!(lines, "wintest hub/lib: known red now passes on Windows: take it off its windows-tests skip list: a::b\nwintest hub/lib: of its known reds, 1 still red and 0 did not finish (left out until fixq lands their fixes)\n");
        assert_eq!(passes, [("wintest hub/lib".to_string(), "a::b".to_string())]);
        let v = verdict(Ok(Some(0)), &format!("GATEUNIT ok wintest hub/lib\n{lines}NODEGATE-OK\n"), Path::new("windows-tests.log"));
        assert!(v.ok && test_lines(&lines).0.is_empty() && panic_lines(&lines).is_empty(), "its lines judge nothing");
        assert_eq!(known_reds_said(&all[1..2], None).0, "wintest hub/lib: its 2 known red(s) did not run: the lease ran no cargo test\n");
        // a corpus file (`corpus:<file>`, from its package's directory) runs under the JIT there,
        // `makepad test <file>`, as its package's unit; an entry that cannot run is its red unrun
        assert_eq!(wreach(&["libs/jit/tests/tls.rs"], &[]), ["wintest jit/corpus:../x.rs", "wintest jit/corpus:tests/gone.rs", "wintest jit/corpus:tests/name_cut.rs", "wintest jit/corpus:tests/skips.rs", "wintest jit/corpus:tests/tls.rs"], "reached as its package is");
        assert!(is_unit("wintest jit/corpus:tests/tls.rs") && unit_key("wintest jit/corpus:tests/tls.rs") != unit_key("wintest jit/corpus:tests/name_cut.rs"), "each file a unit of its own");
        let faults: Vec<Option<&str>> = corpus.iter().map(|u| u.fault.as_deref()).collect();
        assert_eq!(faults, [Some("its corpus entry corpus:../x.rs is no plain relative .rs path"), Some("its corpus file libs/jit/tests/gone.rs is not in the tree"), None, Some("a corpus file runs whole under `makepad test`: its skip list cannot apply"), None]);
        let runs: Vec<WinUnit> = corpus.iter().filter(|u| u.fault.is_none()).cloned().collect();
        assert_eq!(runs.iter().map(|u| u.corpus().unwrap()).collect::<Vec<_>>(), ["libs/jit/tests/name_cut.rs", "libs/jit/tests/tls.rs"]);
        // one PowerShell script (a Windows run is plain argv): the JIT built once, each file under it,
        // a red one once more at once, its markers around each run
        let argv = corpus_argv(&runs);
        assert_eq!(argv[..4], ["powershell", "-NoProfile", "-NonInteractive", "-Command"]);
        assert!(argv[4].contains("cargo build --profile host -q -p makepad-run --bin makepad") && argv[4].contains("foreach ($T in @('libs/jit/tests/name_cut.rs', 'libs/jit/tests/tls.rs'))") && argv[4].contains("& $J test $T"), "{}", argv[4]);
        assert!(crate::lease::argv::windows_plain(&argv, Path::new("corpus")).unwrap().is_some_and(|r| r.text.contains("'CORPUSDONE'")), "the node runs it from a .ps1 file");
        // its output by file: a JIT that ended before its result, then passed (a flake); tests that
        // failed on both runs, with the reason lines under each, in the per-test blame as a cargo
        // target's are
        let out = "CORPUSBUILD ok\nCORPUSRUN libs/jit/tests/name_cut.rs\nCORPUSEXIT libs/jit/tests/name_cut.rs -1073741819\nCORPUSAGAIN libs/jit/tests/name_cut.rs\n\
            test test_crate::cut ... ok (2.00 ms)\ntest result: 1 passed; 0 failed. compiled 9 fns\nCORPUSEXIT libs/jit/tests/name_cut.rs 0\n\
            CORPUSRUN libs/jit/tests/tls.rs\ntest test_crate::keys ... FAILED (3.00 ms)\n  report: panic at libs/jit/tests/tls.rs:9:5: left == right\ntest test_crate::grow ... ok (1.00 ms)\ntest result: 1 passed; 1 failed. compiled 9 fns\nCORPUSEXIT libs/jit/tests/tls.rs 101\n\
            CORPUSAGAIN libs/jit/tests/tls.rs\ntest test_crate::keys ... FAILED (3.00 ms)\n  report: panic at libs/jit/tests/tls.rs:9:5: left == right\ntest result: 1 passed; 1 failed. compiled 9 fns\nCORPUSEXIT libs/jit/tests/tls.rs 101\nCORPUSDONE\n";
        let ran = corpus_runs(&Ok(Some(1)), out, &runs).unwrap();
        let keys = WinRun::Failed(vec!["test_crate::keys".into()], vec!["report: panic at libs/jit/tests/tls.rs:9:5: left == right".into()]);
        assert_eq!(ran, [(WinRun::Broke("the JIT ended before its result (exit -1073741819)".into(), Vec::new()), Some(WinRun::Passed)), (keys.clone(), Some(keys))]);
        let (first, rerun): (Vec<WinRun>, Vec<Option<WinRun>>) = ran.into_iter().unzip();
        let (said, ok) = windows_tests_said(&runs, &first, &rerun);
        assert!(!ok && said.contains("GATEFLAKE wintest jit/corpus:tests/name_cut.rs :: \nGATEUNIT ok wintest jit/corpus:tests/name_cut.rs\n"), "{said}");
        assert!(said.contains("FAILED test_crate::keys\nreport: panic at libs/jit/tests/tls.rs:9:5: left == right\nwintest jit/corpus:tests/tls.rs: 1 test(s) failed on Windows, on its rerun too\nGATEREPRO wintest jit/corpus:tests/tls.rs :: makepad devhub run <lane> test --needs os=windows -- cargo run --profile host -p makepad-run --bin makepad -- test libs/jit/tests/tls.rs\nGATEUNIT fail wintest jit/corpus:tests/tls.rs\n"), "{said}");
        let v = verdict(Ok(Some(1)), &format!("{out}{said}"), Path::new("windows-tests.log"));
        assert_eq!((v.ok, v.failing.as_slice()), (false, &["wintest jit/corpus:tests/tls.rs".to_string()][..]));
        assert_eq!(test_lines(&format!("{out}{said}")).0.get("wintest jit/corpus:tests/tls.rs"), Some(&vec!["test_crate::keys".to_string()]), "its failing test, as the per-test blame reads it");
        // a file that did not compile under the JIT; the JIT not built breaks them all; a script cut
        // short is no verdict (its lease's)
        let unbuilt = corpus_runs(&Ok(Some(1)), "CORPUSBUILD ok\nCORPUSRUN libs/jit/tests/tls.rs\nerror[E0425]: cannot find value `k` in this scope\nerror: could not compile `libs/jit/tests/tls.rs` (test) due to 1 previous error: no test ran\nCORPUSEXIT libs/jit/tests/tls.rs 1\nCORPUSDONE\n", &runs[1..]).unwrap();
        assert!(matches!(&unbuilt[0].0, WinRun::Broke(why, lines) if why == "it did not compile under the JIT on Windows" && lines.len() == 2 && lines[0].starts_with("error[E0425]")), "{unbuilt:?}");
        let nojit = corpus_runs(&Ok(Some(1)), "libs\\relax\\x.rs:3:9: error[E0425]: cannot find value `x`\nCORPUSBUILD fail 101\n", &runs).unwrap();
        assert!(nojit.iter().all(|(r, again)| again.is_none() && matches!(r, WinRun::Broke(why, l) if why == "the JIT did not build on Windows" && l[0].starts_with("libs/relax/x.rs:3:9: error[E0425]"))), "{nojit:?}");
        assert_eq!((corpus_runs(&Ok(Some(1)), "CORPUSBUILD ok\nCORPUSRUN libs/jit/tests/tls.rs\n", &runs), corpus_runs(&Err("lost".into()), "", &runs)), (None, None));
        let (said, _) = windows_tests_said(&corpus[3..4], &[WinRun::Broke(corpus[3].fault.clone().unwrap(), Vec::new())], &[None]);
        assert!(said.contains("wintest jit/corpus:tests/skips.rs: a corpus file runs whole under `makepad test`: its skip list cannot apply\nGATEREPRO") && !said.contains("leaves out"), "an entry that cannot run, red unrun: {said}");
        let home = std::env::temp_dir().join(format!("devhub-bucket-known-reds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        tell_known_passes(&home, &all[1..2], &passes);
        tell_known_passes(&home, &all[1..2], &passes);
        let told = crate::notes::take(&home, "main");
        assert_eq!(told.len(), 1, "main hears it once: {told:?}");
        assert!(told[0].1.contains("wintest hub/lib's known red a::b now passes on Windows"), "{told:?}");
        // off the list, its record goes: listed again, it is told again
        let off = vec![WinUnit { skip: vec!["a::c".into()], ..all[1].clone() }];
        tell_known_passes(&home, &off, &[]);
        tell_known_passes(&home, &all[1..2], &passes);
        assert_eq!(crate::notes::take(&home, "main").len(), 1, "told again once listed again");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn named_landings_are_distinct_whole_numbers() {
        assert_eq!(named_ids("rc-a (A) #12 (it added x.rs:1), rc-b (B) #3"), BTreeSet::from([3, 12]));
        assert!(named_ids("").is_empty());
        assert!(!named_ids("rc-a (A) #12").contains(&1), "#1 is not inside #12");
        assert_eq!(named_ids("#1, #12, #1, #12"), BTreeSet::from([1, 12]));
    }

    #[test]
    fn a_probe_answers_only_for_the_unit_it_judged() {
        let key = "test pkg";
        let mut outcome = Outcome { ok: false, failing: vec!["test pkg --lib".into()], log: String::new(), summary: String::new(), no_verdict: None };
        assert_eq!(probe_says(key, &outcome, "GATEUNIT ok test pkg --test other\n"), Some(true), "a red target wins over a passed target");
        outcome.failing.clear();
        outcome.ok = true;
        assert_eq!(probe_says("rustfmt", &outcome, ""), Some(false), "a step's whole gate passed");
        assert_eq!(probe_says(key, &outcome, ""), None, "a test's run that passed with no line of it ran none of it (11-20)");
        assert_eq!(probe_says(key, &outcome, "GATEUNIT ok test pkg --test chrome\n"), Some(false), "and one with its line passed it");
        outcome.ok = false;
        outcome.failing.push("test other".into());
        assert_eq!(probe_says(key, &outcome, "GATEUNIT ok test pkg --lib\n"), Some(false), "this unit passed while another failed");
        assert_eq!(probe_says(key, &outcome, ""), None, "an unjudged unit has no answer");
        assert_eq!(probe_says(key, &outcome, &format!("GATEUNIT ok test pkg --lib\n{RERUN}\n")), None, "only the last run counts");
        let proof = "GATEPROOF proven test pkg --lib (bucket fast 4d334b18a6,1f614334b6)\n";
        assert_eq!(probe_says(key, &outcome, proof), Some(false), "a unit proven at the probe's inputs passed there");
        assert_eq!(probe_says(key, &outcome, &format!("{proof}{RERUN}\n")), Some(false), "a proof of the inputs holds for the rerun too");
        assert_eq!(probe_says(key, &outcome, "GATEPROOF proven test pkgs (bucket fast a,b)\nGATEPROOF none: no store\n"), None, "another unit's proof is no answer");
        outcome.failing.push("test pkg --lib".into());
        assert_eq!(probe_says(key, &outcome, proof), Some(true), "a red run wins over a proof");
        outcome.failing.pop();
        outcome.no_verdict = Some("lease lost".into());
        assert_eq!(probe_says(key, &outcome, "GATEUNIT ok test pkg --lib\n"), None);
        outcome.failing.push("test pkg --lib".into());
        assert_eq!(probe_says(key, &outcome, ""), None, "no verdict also voids a red");
    }

    #[test]
    fn a_bisect_range_is_queued_once_and_resumes_until_done() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-bisect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let from = pin_of(&Pair::new(&"a".repeat(40), &"b".repeat(40))).with("repo-b", &"c".repeat(40));
        let tip = pin_of(&Pair::new(&"d".repeat(40), &"e".repeat(40))).with("repo-b", &"f".repeat(40));
        let mut job = Bisect { bucket: "fast".into(), unit: "check pkg".into(), key: "check pkg".into(), from: from.clone(), tip: tip.clone(), line: "error: missing value".into(), log: "fast-1.log".into(), closed: false, named: Vec::new(), tip_probe: None, units: Vec::new() };
        assert_eq!(bisect_pin(pin_of(&from.pair()), &tip).nested.get("repo-b"), tip.nested.get("repo-b"), "the pair takes the red run's other nested pins");
        assert_eq!(bisect_pin(from.clone(), &tip), from, "a pass's own nested pins stay its own");
        assert_eq!(bisect_pin(pin_of(&from.pair()), &tip).pair(), from.pair(), "its paired side stays at the candidate");
        assert!(queue_bisect(&home, &job).unwrap());
        assert!(!queue_bisect(&home, &job).unwrap());
        job.tip = from.clone();
        job.from.nested.insert(crate::state::PAIRED.to_string(), "e".repeat(40));
        assert!(!queue_bisect(&home, &job).unwrap(), "bucket, key and from work identify the range");
        assert_eq!(std::fs::read_to_string(home.join("bisects.jsonl")).unwrap().lines().count(), 1, "one queued row");
        assert!(pending_bisects(&home, "tests").unwrap().is_empty());
        let mut pending = pending_bisects(&home, "fast").unwrap();
        assert_eq!(pending.len(), 1, "a restarted master finds the queued job");
        let saved = pending.pop().unwrap();
        assert_eq!((&saved.from, &saved.tip), (&from, &tip), "both pins keep their other nested repositories");
        assert_eq!((&saved.unit, &saved.key, &saved.line, &saved.log), (&job.unit, &job.key, &job.line, &job.log));
        done_bisect(&home, &saved, vec![("landing", Value::Int(12)), ("probes", Value::Arr(Vec::new()))]).unwrap();
        assert!(pending_bisects(&home, "fast").unwrap().is_empty(), "done jobs do not resume");
        assert!(!queue_bisect(&home, &saved).unwrap(), "done ranges never queue again");
        job.from.makepad = "b".repeat(40);
        assert!(queue_bisect(&home, &job).unwrap(), "a later last pass is a new range");
        // a CI leg's red tip probe, recorded before its bisect waits for the night: a restarted
        // master resumes the job at its bisect, with no second tip probe
        let mut leg = job.clone();
        (leg.bucket, leg.key, leg.unit) = (CI.into(), "smoke:linux:apps/x".into(), "smoke:linux:apps/x".into());
        assert!(queue_bisect(&home, &leg).unwrap());
        append_bisect(&home, leg.value("tip-red", vec![("probe", obj(vec![("red", Value::Bool(true))]))])).unwrap();
        let resumed = pending_bisects(&home, CI).unwrap();
        assert_eq!(resumed.len(), 1, "its tip-red row stands for its queued job, not a second one");
        assert_eq!(resumed[0].tip_probe.as_ref().and_then(|p| p.get("red")).and_then(Value::as_bool), Some(true), "the resumed leg keeps its tip probe");
        assert!(pending_bisects(&home, "fast").unwrap().iter().all(|j| j.tip_probe.is_none()), "a queued job without one has none");
        // the night: midnight until 06:00 on the master's clock
        let hour = 3600;
        assert_eq!(till_night(2 * hour, 0), None, "02:00 UTC is night");
        assert_eq!(till_night(5 * hour, 2 * hour), Some(17 * hour as u64), "05:00 UTC at +2 is 07:00: day, 17 hours to midnight");
        assert_eq!(till_night(23 * hour, 2 * hour), None, "23:00 UTC at +2 is 01:00: night");
        assert_eq!(till_night(6 * hour, 0), Some(18 * hour as u64), "06:00 is day");
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A CI leg's red stays open until a run passes it, and `devhub bucket` says it STILL FAILING
    /// meanwhile: its legs' verdicts are units' (each CI red went to nobody and no line, 11-17).
    #[test]
    fn a_ci_legs_red_stays_open_until_a_run_passes_it() {
        assert!(is_leg("smoke:linux:apps/pdf") && is_leg("live:kinds"), "a leg is <kind>:<rest>");
        assert!(!is_leg("check pkg") && !is_leg(STATICS_STEP) && !is_leg("no verdict: 2 unit(s) of shard L1") && !is_leg(":x") && !is_leg("smoke:"), "no unit, step or note is one");
        let home = std::env::temp_dir().join(format!("devhub-bucket-ci-legs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open_quiet(&home, "").unwrap();
        let tip = Pair::new(&"a".repeat(40), "");
        let judged = |n: u64, text: &str| {
            let log = home.join(format!("ci-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let failing: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("GATEUNIT fail ")).map(str::to_string).collect();
            let outcome = Outcome { ok: failing.is_empty(), failing, log: log.display().to_string(), summary: String::new(), no_verdict: None };
            judge(CI, &home, &home, &store, &|_, _, _| {}, n, None, &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
        };
        let still = || status(&home, &[]).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find(|v| v.get("still_failing").and_then(Value::as_str) == Some(CI))
            .and_then(|v| v.get("units").cloned());
        judged(1, "fail smoke:linux:apps/pdf: 1 step failed (x.log)\nGATEKEPT smoke:linux:apps/pdf :: /kept/ci-1.keep/x.log\nGATEUNIT fail smoke:linux:apps/pdf\nok live:kinds: 1 step ok (y.log)\nGATEUNIT ok live:kinds\n");
        let row = rows_of(&home, CI).remove(0);
        assert_eq!(row.get("passed").and_then(Value::as_arr).unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>(), ["live:kinds"], "its passed legs");
        let units = still().expect("a STILL FAILING line for ci");
        // and where its own log is kept: a live:replay red said only its bucket's log for 21 h, and
        // its own was gone (11-24)
        assert_eq!(units.get("smoke:linux:apps/pdf").and_then(Value::as_str), Some("origin unknown, first failed in ci-1.log (its output: /kept/ci-1.keep/x.log)"), "no file named: nobody");
        assert!(units.get("live:kinds").is_none());
        judged(2, "ok live:kinds: 1 step ok (y.log)\nGATEUNIT ok live:kinds\n");
        assert!(still().is_some(), "a run that did not look at it leaves it open");
        judged(3, "ok smoke:linux:apps/pdf: 1 step ok (x.log)\nGATEUNIT ok smoke:linux:apps/pdf\n");
        assert!(still().is_none(), "a run that passed it closes it");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_no_verdict_run_retires_its_sidecar_but_other_errors_keep_it() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-no-verdict-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let log = home.join("ci-1.log");
        let sidecar = log.with_extension("log.run");
        let store = Store::open_quiet(&home, "").unwrap();
        let tip = Pair::new(&"a".repeat(40), "");
        save_run(&log, 1, 1, &pin_of(&tip), &pin_of(&tip)).unwrap();
        let error = "no verdict on a: unavailable".to_string();
        assert!(finish_run(&log, Err(JudgeError::Other(error))).is_err());
        assert!(sidecar.exists(), "an ordinary error keeps the run even with no-verdict text");
        let outcome = Outcome { ok: false, failing: Vec::new(), log: log.display().to_string(), summary: "incomplete".into(), no_verdict: Some("unavailable".into()) };
        let result = judge("ci", &home, &home, &store, &|_, _, _| {}, 1, None, &pin_of(&tip), None, |_| Ok(outcome));
        assert!(matches!(finish_run(&log, result), Err(JudgeError::NoVerdict(_))));
        assert!(!sidecar.exists(), "a no-verdict end must never be adopted again");
        assert!(rows_of(&home, "ci").is_empty(), "no verdict writes no ledger row");
        assert!(matches!(finish_run(&log, Err(JudgeError::NoVerdict("unavailable".into()))), Err(JudgeError::NoVerdict(_))), "an absent sidecar is already retired");
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// Every step the gate fails with no unit says so on a line of its own (land_gate.sh step_fail:
    /// GATESTEP), and beside failing units it still fails: no pass of it. Read off NODEGATE-FAIL
    /// only when no unit failed, three runs red at statics beside failing units read as its passes,
    /// and the next red named seven landings for #5890's statics (11-16).
    #[test]
    fn every_step_failing_beside_failing_units_still_fails() {
        let gate = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/land_gate.sh")).unwrap();
        let mut steps: Vec<String> = gate.lines().filter(|l| !l.trim_start().starts_with('#'))
            .flat_map(|l| l.split("step_fail ").skip(1).chain(l.split("GATESTEP fail ").skip(1)).map(|rest| rest.trim_start_matches('"').split(['"', ';', ' ', '}']).next().unwrap_or("").to_string()).collect::<Vec<_>>())
            .filter(|w| !w.is_empty() && !w.starts_with('$')).collect();
        // check_pkg's and present's own words, run as steps (as_step)
        steps.extend(["makepad-widgets", "makepad-widgets-warnings", "stage", "calc-present", "stage-settle"].map(str::to_string));
        steps.sort();
        steps.dedup();
        for step in [STATICS_STEP, "private", "depcheck", "queue", "affected", "check-reject", "x64-tests", "rt-lock-copies", "jit-suite:$p", "bad-mode"] {
            assert!(steps.iter().any(|w| w == step), "the gate marks {step}: {steps:?}");
        }
        let units = ["check makepad-fab", "test makepad-run"];
        let text = format!("{}{}NODEGATE-FAIL: makepad-fab test:makepad-run {} [lease L1 lane landq-fast at abc in /tree]\n",
            units.map(|u| format!("GATEUNIT fail {u}\n")).concat(), steps.iter().map(|w| format!("GATESTEP fail {w}\n")).collect::<String>(), steps.join(" "));
        let o = verdict(Ok(Some(1)), &text, Path::new("x.log"));
        let (red, green) = (Pair::new(&"a".repeat(40), &"b".repeat(40)), Pair::new(&"c".repeat(40), &"b".repeat(40)));
        let rows = [obj(vec![("tip", tip_value(&pin_of(&red))), ("failing", Value::Arr(o.failing.iter().map(|u| s(u)).collect()))]),
            obj(vec![("tip", tip_value(&pin_of(&green))), ("failing", Value::Arr(vec![s("check example")]))])];
        for step in &steps {
            assert!(step.split_whitespace().count() == 1 && !is_unit(step), "{step}: a step's key is one word, no unit");
            assert!(o.failing.contains(step), "{step} fails beside failing units: {:?}", o.failing);
            assert_eq!(last_pass(&rows, step).map(|p| p.pair()).as_ref(), Some(&green), "{step}: red beside units is no pass of it");
        }
        assert_eq!(o.failing.len(), steps.len() + units.len(), "the units and the steps, not the units' own words: {:?}", o.failing);
        // a gate from before the marks: its words, when no unit failed
        assert_eq!(verdict(Ok(Some(1)), "NODEGATE-FAIL: statics private [lease L1]\n", Path::new("x.log")).failing, ["private", STATICS_STEP]);
        // a shard the toolchain changed under: its steps' failures are no verdict, as its units'
        let ran = [(PathBuf::from("s1.log"), (Ok(Some(1)), "TOOLCHAIN CHANGED under it\nGATEUNIT fail check x\nGATESTEP fail statics\nNODEGATE-FAIL: x statics\n".to_string()))];
        let text = merged(&[], (1, 1), &ran);
        assert!(text.contains("GATEUNIT voided check x\nGATESTEP voided statics\n") && !text.contains(" fail "), "{text}");
    }

    #[test]
    fn a_lost_lease_runs_the_gate_once_more_and_lost_again_is_no_verdict() {
        let lost_exit = Ok(Some(crate::lease::client::LOST_EXIT));
        assert!(again(&lost_exit, "why\nL1 test@node-a: granted\n"));
        assert!(!again(&lost_exit, &format!("why\n{RERUN}\nL2 test@node-b: granted\n")), "only once, also for a run adopted after a master restart");
        assert!(!again(&Ok(Some(1)), "GATEUNIT fail check x\n"), "a failed gate is a verdict");
        // an adopted run whose holder went with the master's box (no exit, no verdict) is a lost
        // lease's: once more; one that said its verdict or wrote its exit is as it said
        let (cut, said) = adopted_end(None, None, false, false);
        assert!(said && again(&cut, &format!("why\nL1 test@node-a: granted\nSTEP jit 9 s\n{UNWATCHED}\n")), "a run the reboot cut runs once more");
        assert_eq!(adopted_end(None, None, false, true), (Ok(None), false), "its verdict said: as it said");
        assert_eq!(adopted_end(Some(1), None, false, false), (Ok(Some(1)), false), "its exit written: that exit");
        assert_eq!(adopted_end(None, None, true, false), (Ok(Some(0)), false), "stopped as not needed before the restart");
        // an OOM kill is no verdict on the landings either: once more
        let oom = "L9 run 1 ended without its summary: killed by signal 15 (SIGTERM); the kernel's OOM killer killed 5 (makepad.jit-3), 4.5 GB resident, at the lease's memory cap of 7.5 GB\n";
        assert!(again(&Ok(Some(143)), &format!("why\n{oom}")));
        assert!(!again(&Ok(Some(143)), &format!("why\n{oom}{RERUN}\n{oom}")), "only once");
        assert!(!again(&Ok(Some(143)), "why\nL9 run 1 ended without its summary: killed by signal 15 (SIGTERM); no OOM kill\n"));
        // the verdict reads the last run only: the lost run's lines are gone with it
        let text = format!("why\nSTEP jit 9 s\n{RERUN}\nSTEP jit 3 s\nNODEGATE-OK\n");
        assert_eq!(last_run(&text), "STEP jit 3 s\nNODEGATE-OK\n");
        let o = verdict(Ok(Some(0)), last_run(&text), Path::new("x.log"));
        assert!(o.ok && o.no_verdict.is_none() && o.summary == "STEP jit 3 s", "{:?} {}", o.failing, o.summary);
        // a gate that said its start and exited 0 without its verdict line said nothing of the
        // landings: once more, then no verdict naming it, twice (a box's disk reclaim took a sweep's
        // run log under it, L1561802, 11-22). A cargo check says no verdict line; one stopped as not
        // needed, or that said its verdict, is as it said
        let started = "why\nL7 box-a: sources at work@abc\nGATE libs: steps before\nGATESTART check a 1\n";
        assert_eq!((again(&Ok(Some(0)), started), rerun_why(&Ok(Some(0)), started)), (true, Some(RERUN_UNSAID)));
        assert_eq!(rerun_why(&lost_exit, "why\nL1 test@node-a: granted\n"), Some(RERUN), "a lost lease says so");
        let twice = format!("{started}{RERUN_UNSAID}\nL8 box-b: sources at work@abc\nGATE libs: steps before\n");
        assert!(!again(&Ok(Some(0)), &twice), "only once");
        assert!(!again(&lost_exit, &twice), "once in all: a lost lease after it is no verdict");
        assert!(!again(&Ok(Some(0)), "why\n    Finished `dev` profile\n"), "a cargo check says no verdict line");
        assert!(!again(&Ok(Some(0)), &format!("{started}NODEGATE-OK libs 3 checks\n")) && !again(&Ok(Some(0)), &format!("{started}{NOT_NEEDED}\n")));
        assert_eq!(last_run(&twice), "L8 box-b: sources at work@abc\nGATE libs: steps before\n", "the verdict reads the last run only");
        let dir = std::env::temp_dir().join(format!("devhub-bucket-unsaid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = dir.join(format!("{FULL}-5.log"));
        std::fs::write(libs_log(&run), &twice).unwrap();
        std::fs::write(shard_log(&run, 1), started).unwrap();
        let ran = [(libs_log(&run), (Ok(Some(0)), kept(&twice))), (shard_log(&run, 1), (Ok(Some(0)), started.to_string())), (shard_log(&run, 2), (Ok(Some(0)), format!("{started}NODEGATE-OK testrun 3 packages\n")))];
        assert_eq!(unsaid_parts(&run, 2, &ran), ["the libraries' sweep (L8 on box-b): no verdict, twice (it exited 0 without its verdict line, and again as it ran once more)", "shard 1 (L7 on box-a): no verdict (it exited 0 without its verdict line)"], "each named, its last lease and box; the shard that said its verdict stands");
        let _ = std::fs::remove_dir_all(&dir);
        // both gates say the start the rule reads (land_gate.sh for every mode, mac_tests.sh)
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture");
        for (gate, start) in [("land_gate.sh", "echo \"GATE $mode: "), ("mac_tests.sh", "echo \"GATE mac: ")] {
            assert!(std::fs::read_to_string(fixture.join(gate)).unwrap().lines().any(|l| l.starts_with(start)), "{gate} says its start");
        }
        // lost twice: no verdict, nothing for the landings to answer for
        assert!(verdict(lost_exit, "STEP affected 0 s\n", Path::new("x.log")).no_verdict.is_some());
        assert!(verdict(Ok(Some(1)), "", Path::new("x.log")).no_verdict.is_none());
        // ended at exit 0, or with no exit status (adopted, killed with the Mac), before any verdict
        // line: no verdict on the landings, and what is missing is said
        let queued = "why\nL5 gate: 1st in queue (node-a: waits for room)\n";
        for status in [Ok(Some(0)), Ok(None)] {
            let o = verdict(status, queued, Path::new("x.log"));
            assert!(!o.ok && o.failing.is_empty(), "{:?}", o.failing);
            let why = o.no_verdict.unwrap();
            assert!(why.contains("its log ends at `L5 gate: 1st in queue (node-a: waits for room)`"), "{why}");
        }
        assert!(verdict(Ok(Some(0)), "", Path::new("x.log")).no_verdict.unwrap().contains("NODEGATE-OK or NODEGATE-FAIL"));
        // a shard its box ended for memory: a verdict on the unit it ran then, said so, and no
        // "no verdict" naming the batch
        let ended = "makepad devhub: L1 on node-a ended (memory): L1 (test of landq-tests) ended on node-a for memory: it held 45.0 GB, past the 28.9 GB it was weighed at\n";
        let text = format!("== shard 1/2: s1.log\nGATESTART test a 1\nGATEEND test a\nGATESTART test b 2\n{ended}== shard 2/2: s2.log\nGATESTART test c 3\nGATEEND test c\nNODEGATE-OK\n");
        let o = verdict(Ok(Some(1)), &text, Path::new("x.log"));
        assert!(!o.ok && o.no_verdict.is_none(), "{:?}", o.no_verdict);
        assert_eq!(o.failing, vec!["test b".to_string()], "the unit running at the end, not the shard's ended ones nor the other shard's");
        assert!(o.summary.contains("test b ended for memory at 45.0 GB"), "{}", o.summary);
        // one the run dealt again: the shard that judged it says its verdict, and the ended
        // shard's exit is none ([`redealt_ends`])
        let again = format!("== shard 1/2: s1.log\nGATESTART test a 1\nGATEEND test a\nGATEUNIT ok test a\nGATESTART test b 2\n{ended}== shard 2/2: s2.log\nGATESTART test b 3\nGATEEND test b\nGATEUNIT ok test b\nNODEGATE-OK\n");
        let o = verdict(Ok(Some(0)), &again, Path::new("x.log"));
        assert!(o.ok && o.failing.is_empty(), "{:?}", o.failing);
        assert!(o.summary.contains("test b ended for memory at 45.0 GB, dealt again and judged on another shard"), "{}", o.summary);
        let (s1, s2) = again.split_once("== shard 2/2").unwrap();
        assert_eq!(redealt_ends(&[(&Ok(Some(1)), s1), (&Ok(Some(0)), s2)]), vec![0], "the ended shard, its units judged");
        assert!(redealt_ends(&[(&Ok(Some(1)), s1)]).is_empty(), "not while the unit it ran then has no verdict");
    }

    #[test]
    fn a_break_already_on_the_branch_names_who_broke_it_not_the_next_lander() {
        let err = |at: &str| format!("/n/tree/tools/x/src/boot.rs:{at}: E0433: failed to resolve: use of undeclared type `crate::logcat`");
        // the landing that broke it: its run's errors and who it named
        let first = format!("STEP qcheck 1 s\nm: 0 errors\nGATEUNIT ok check m\n{}\n{}\nx: 2 errors\nGATEUNIT fail check x\nSTEP qpresent 1 s\n", err("102:9"), err("110:5"));
        let (errors, passed) = unit_lines(&first);
        assert_eq!(errors["check x"], vec!["boot.rs: E0433: failed to resolve: use of undeclared type `crate::logcat`".to_string()], "path and line give way to the file");
        assert!(passed.contains("check m") && !passed.contains("check x"));
        let row = |errors: &BTreeMap<String, Vec<String>>, passed: &[&str], broke: &str| obj(vec![
            ("errors", Value::Obj(errors.iter().map(|(u, e)| (u.clone(), Value::Arr(e.iter().map(s).collect()))).collect())),
            ("passed", Value::Arr(passed.iter().map(|p| s(*p)).collect())),
            ("broke", obj(vec![("check x", s(broke))])),
        ]);
        let broke = row(&errors, &["check m"], "rc-a (A) #970");
        // a later run checks it as a flip of another lib's build, an edit moved the line: the same break
        let later = format!("{}\nx: 1 errors\nGATEUNIT fail check x --also=y\n", err("130:9"));
        let (now, _) = unit_lines(&later);
        let failing = |e: &BTreeMap<String, Vec<String>>| e.keys().cloned().collect::<Vec<_>>();
        let carried = carried_since(&[obj(vec![]), broke.clone()], &failing(&now), &now);
        assert_eq!(carried.get("check x --also=y").map(String::as_str), Some("rc-a (A) #970"), "rows that never looked at it are skipped");
        // a run that passed it since: a failure now is new
        assert!(carried_since(&[row(&BTreeMap::new(), &["check x"], ""), broke.clone()], &failing(&now), &now).is_empty());
        // an error it did not have: new, the landings that changed its inputs answer for it
        let more = format!("{}\n/n/tree/tools/x/src/main.rs:3:1: E0425: cannot find value `z`\nGATEUNIT fail check x\n", err("102:9"));
        let more = unit_lines(&more).0;
        assert!(carried_since(&[broke], &failing(&more), &more).is_empty());
        assert_eq!(error_key("no position: here"), None);
        assert_eq!(error_key("C:/t/a.rs:1:2: warning: unused").as_deref(), Some("a.rs: warning: unused"));
        // a present's summary carries the error in a tab-separated record whose next field
        // changes per run: the same break, run after run, and against a row that kept the field
        let present = |n: u32| format!("app: no present (failed before its first frame: error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t{n} )error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t{n} ; its last log lines:\n/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\nGATEUNIT fail present app\n");
        let (first, _) = unit_lines(&present(7435));
        assert_eq!(first["present app"], vec!["sla.rs: E0063: missing field `stack`".to_string()]);
        let old = obj(vec![("errors", obj(vec![("present app", Value::Arr(vec![s("sla.rs: E0063: missing field `stack`\t7435 )error\t/n/tree/app/src/sla.rs:690:23: E0063: missing field `stack`\t7435 ; its last log lines:")]))])), ("broke", obj(vec![("present app", s("rc-b (B) #1926"))]))]);
        let (next, _) = unit_lines(&present(1030));
        assert_eq!(carried_since(&[old], &failing(&next), &next).get("present app").map(String::as_str), Some("rc-b (B) #1926"), "the breaker, not the next lander");
    }

    /// The same present failure keys the same in two runs. The runs differ in the JIT's [t=N ms],
    /// the pid, the thread id, an address, the tree, the lease's tmp, and which log line comes
    /// first. The gate's settle_errors.sh drops what varies before it cuts each record. While the
    /// cut fell elsewhere in each run, the same JIT break in vm.rs was named for the next lander (11-07).
    #[test]
    fn a_present_failure_keys_the_same_in_every_run() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/settle_errors.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run = |tree: &str, t: u32, pid: u32, lease: &str, parked_first: bool| -> String {
            let (tree, tmp) = (root.join(tree), root.join(lease));
            let s = tmp.join("landgate.x").join("hrun-app");
            std::fs::create_dir_all(&tree).unwrap();
            std::fs::create_dir_all(&s).unwrap();
            let (tree, tmp) = (tree.canonicalize().unwrap(), tmp.canonicalize().unwrap());
            let fun = "makepad_script_std::vm::ScriptVmCall::resume_if_panicked";
            let at = format!("{}/platform/script/std/src/vm.rs:157:36", tree.display());
            let compile = format!("relax: [t={t} ms] compile error in {fun}: {at}: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls `Box::from_raw` of a provided crate at 0x7f{pid:x}0038: the JIT cannot take its frame");
            let parked = format!("relax: compile error parked on thread main ({pid}) at its call of code that did not compile, or stop: makepad inspect {pid} panics (log {}/relax-session-{pid}-52951.log)", tmp.display());
            let log = if parked_first { format!("{parked}\n{compile}\n") } else { format!("{compile}\n{parked}\n") };
            std::fs::write(s.join("out.log"), log).unwrap();
            std::fs::write(s.join("report"), format!("present 0\nerror\tcompile error in {fun}: {at}: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls\t{pid}\n")).unwrap();
            let o = std::process::Command::new("bash").arg(&script).arg(&s).current_dir(&tree).env("PWD", &tree).env("TMPDIR", &tmp).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            // as land_gate.sh's present says it
            format!("app: no present (failed before its first frame: {}); its last log lines:\nGATEUNIT fail present app\n", String::from_utf8_lossy(&o.stdout))
        };
        let first = run("ci-tree", 12657, 649131, "L618033", false);
        let next = run("native-lane", 8122, 2955436, "L7", true);
        let (a, b) = (unit_lines(&first).0, unit_lines(&next).0);
        assert_eq!(a["present app"], b["present app"], "{first}\n{next}");
        let key = &a["present app"][0];
        assert!(key.starts_with("vm.rs: `<makepad_script_std::vm::ScriptVmCall>::resume_if_panicked` calls `Box::"), "{key}");
        assert!(!key.contains("649131") && !key.contains("2955436") && !key.contains("0x7f"), "{key}");
        // the summary names the tree's file, not the box's path to it
        assert!(first.contains(" platform/script/std/src/vm.rs:157:36: ") && !first.contains("ci-tree") && !first.contains("L618033"), "{first}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A watch beside one unit (unit_watch_body.sh, as unit_watch.sh runs it) whose looks are
    /// counted, not timed: its waits move its clock on by what they wait (`sleeps`: on the box's
    /// clock, sleeping it as unit_watch.sh's do; else at once), and the wait after its `looks`th
    /// look stops it as a gate's TERM does, its peak kept (a look is a wait of the watch's own
    /// shell: its capture's CPU sample waits in a subshell). A test waits for its exit, what it
    /// found by its looks, never for a wall-clock window: a 4 s one failed a parked compile error on
    /// a loaded box before the watch's look and capture were done (11-11).
    #[cfg(target_os = "linux")]
    fn counted_watch(dir: &Path, looks: u32, sleeps: bool) -> PathBuf {
        let body = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/unit_watch_body.sh");
        let (start, wait) = match sleeps {
            true => ("t0=$SECONDS", "sleep \"$1\" & nap=$!; wait \"$nap\"; nap=\"\"; now=$((SECONDS - t0))"),
            false => (":", "now=$((now + $1))"),
        };
        let script = dir.join("counted_watch.sh");
        std::fs::write(&script, format!(
            "{start}; now=0 proc=/proc nap=\"\" looks=0\nwait_s() {{\n  if [ \"$BASH_SUBSHELL\" = 0 ]; then looks=$((looks + 1)); [ \"$looks\" -le {looks} ] || {{ stop; exit 0; }}; fi\n  {wait}\n}}\nread_gpu() {{ :; }}\nwall() {{ date +%s; }}\n. {}\n",
            body.display())).unwrap();
        script
    }

    /// A unit's process, once its watch has exited: whether the watch stopped it (its KILL, sent
    /// before the watch exited, ends it however a TERM from here falls) or it ran on.
    #[cfg(target_os = "linux")]
    fn stopped_by_its_watch(unit: &mut std::process::Child) -> bool {
        use std::os::unix::process::ExitStatusExt;
        let _ = std::process::Command::new("kill").args(["-TERM", &unit.id().to_string()]).status();
        unit.wait().unwrap().signal() == Some(9)
    }

    /// A unit whose process parks fails HUNG at the gate's next look, with the park's text and the
    /// compile error it parked at, and its processes are stopped. Nobody resumes a park in a gate:
    /// an app a test started parked at a compile error and held its unit 64 min (11-07). A panic
    /// that parks for less than a look goes on by itself and is no hang.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_whose_process_parks_is_hung_at_the_next_look() {
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        // two looks, on a clock that takes no time: a brief park is past by then
        let script = counted_watch(&root, 2, false);
        // (the unit's output, the session log of a JIT app of it) -> its .hung, and whether its process was stopped
        let watch = |own: &str, app: &str| -> (Option<String>, bool) {
            // the unit in a session of its own, as the gate's `watched` starts it
            let mut unit = std::process::Command::new("setsid").args(["sleep", "300"]).spawn().unwrap();
            let sid = unit.id();
            // (once setsid is in place: the spawn returns before the child's exec)
            let leads = || std::fs::read_to_string(format!("/proc/{sid}/stat")).ok()
                .is_some_and(|stat| stat.rsplit(") ").next().and_then(|s| s.split_whitespace().nth(3)) == Some(sid.to_string().as_str()));
            assert!((0..50).any(|_| leads() || { std::thread::sleep(Duration::from_millis(20)); false }), "the unit leads its session");
            let out = root.join(format!("unit-{sid}"));
            std::fs::write(&out, own).unwrap();
            std::fs::write(root.join("tmp").join(format!("relax-session-{sid}-1.log")), app).unwrap();
            let watched = std::process::Command::new("bash").arg(&script).arg(sid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", "600").env("TMPDIR", root.join("tmp")).env("B", "true").status().unwrap();
            assert!(watched.success(), "{watched}");
            let stopped = stopped_by_its_watch(&mut unit);
            assert!(std::fs::metadata(format!("{}.peak", out.display())).is_ok(), "its peak, hung or not");
            (std::fs::read_to_string(format!("{}.hung", out.display())).ok(), stopped)
        };
        let compile = "relax: compile error in lazy_typeck_dep::broken: libs/dep/src/lib.rs:3:5: mismatched types\n\
            relax: compile error parked on thread main at its call of code that did not compile: edit the code (a live edit that makes it compile resumes the call), or stop: makepad inspect 7 panics (resume P1 stop)\n";
        let (hung, stopped) = watch("running 1 test\n", compile);
        let hung = hung.expect("a parked compile error is a hang");
        assert!(hung.starts_with("== a process of it parked, which nobody resumes in a gate: relax: compile error in lazy_typeck_dep::broken: libs/dep/src/lib.rs:3:5: mismatched types; relax: compile error parked on thread main"), "{hung}");
        assert!(stopped, "its processes are stopped");
        // a panic parked as watched, in the unit's own output
        let (hung, _) = watch("running 1 test\nrelax: panic parked on thread main (index out of bounds) for 600 s -- attach: makepad inspect 7 panics (resume P1 panic)\n", "");
        assert!(hung.is_some_and(|h| h.contains("panic parked on thread main (index out of bounds) for 600 s")));
        // a brief park goes on by itself
        let (hung, stopped) = watch("running 1 test\n", "relax: panic parked on thread main (boom) for 1 s -- attach: makepad inspect 7 panics (resume P1 panic)\n");
        assert!(hung.is_none() && !stopped, "{hung:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit whose processes only move bytes between themselves (a test polling its app's remote:
    /// socket traffic, under 1% of a core) does nothing, and is hung after its quiet. A test polled
    /// its parked app every 50 ms, read as busy, and held its unit 64 min (11-07). A unit that
    /// works on a core is busy.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_that_only_talks_to_itself_is_hung_after_its_quiet() {
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // six looks of a second each, on the box's clock
        let script = counted_watch(&root, 6, true);
        // whether the watch said HUNG within 6 looks of a quiet of 3 s
        let hung = |program: &str| -> bool {
            let mut unit = std::process::Command::new("setsid").args(["python3", "-I", "-c", program]).spawn().unwrap();
            let sid = unit.id();
            let leads = || std::fs::read_to_string(format!("/proc/{sid}/stat")).ok()
                .is_some_and(|stat| stat.rsplit(") ").next().and_then(|s| s.split_whitespace().nth(3)) == Some(sid.to_string().as_str()));
            assert!((0..50).any(|_| leads() || { std::thread::sleep(Duration::from_millis(20)); false }), "the unit leads its session");
            let out = root.join(format!("unit-{sid}"));
            std::fs::write(&out, "running 1 test\n").unwrap();
            let watched = std::process::Command::new("bash").arg(&script).arg(sid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", "3").env("TMPDIR", &root).env("B", "true").status().unwrap();
            assert!(watched.success(), "{watched}");
            let _ = unit.kill();
            let _ = unit.wait();
            std::fs::read_to_string(format!("{}.hung", out.display())).is_ok_and(|h| h.starts_with("== its processes did nothing"))
        };
        // 2.6 MB/s through a socket pair, back and forth in one process
        let chatter = "import socket, time\na, b = socket.socketpair()\nbuf = b'x' * 32768\nwhile True:\n    for _ in range(8):\n        a.sendall(buf)\n        n = 0\n        while n < len(buf):\n            n += len(b.recv(65536))\n    time.sleep(0.1)\n";
        assert!(hung(chatter), "socket traffic is no work");
        assert!(!hung("while True:\n    pass\n"), "a core's work is");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit hung as `test relax-runner` was (L939783, 11-13): its harness reads a test's child's
    /// output, and that child's own child blocks on a lock the child holds: no CPU, no I/O, no new
    /// result, no process gone. Its tests run (its `running 2 tests` has no `test result:`), so past
    /// twice its measure its quiet is that measure: it fails HUNG naming that, its state kept and its
    /// processes stopped, long before the 9 min (here 600 s). The unit sat 9 min against its 27 s.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_whose_test_waits_on_its_own_childs_lock_fails_past_twice_its_measure() {
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // twelve looks of a second each, on the box's clock
        let script = counted_watch(&root, 12, true);
        let (lock, out) = (root.join("lock"), root.join("unit"));
        let grandchild = "import fcntl, sys\nf = open(sys.argv[1], 'w')\nfcntl.flock(f, fcntl.LOCK_EX)\n";
        let child = format!("import fcntl, subprocess, sys\nf = open(sys.argv[1], 'w')\nfcntl.flock(f, fcntl.LOCK_EX)\nsubprocess.run([sys.executable, '-I', '-c', {grandchild:?}, sys.argv[1]], capture_output=True)\n");
        let harness = format!("import subprocess, sys\nprint('running 2 tests', flush=True)\nprint('test a ... ok', flush=True)\nsubprocess.run([sys.executable, '-I', '-c', {child:?}, sys.argv[1]], capture_output=True)\n");
        let mut unit = std::process::Command::new("setsid").args(["python3", "-I", "-c", &harness]).arg(&lock)
            .stdout(std::fs::File::create(&out).unwrap()).spawn().unwrap();
        let sid = unit.id();
        let leads = || std::fs::read_to_string(format!("/proc/{sid}/stat")).ok()
            .is_some_and(|stat| stat.rsplit(") ").next().and_then(|s| s.split_whitespace().nth(3)) == Some(sid.to_string().as_str()));
        assert!((0..50).any(|_| leads() || { std::thread::sleep(Duration::from_millis(20)); false }), "the unit leads its session");
        let watched = std::process::Command::new("bash").arg(&script).arg(sid.to_string()).arg(&out)
            .env("HUNG_LOOK", "1").env("HUNG_QUIET", "600").env("MEASURE_S", "2").env("TMPDIR", &root).env("B", "true").status().unwrap();
        assert!(watched.success(), "{watched}");
        let stopped = stopped_by_its_watch(&mut unit);
        // (its child and grandchild too, should the watch have left them: they lead no session of their own)
        let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{sid}")]).status();
        let hung = std::fs::read_to_string(format!("{}.hung", out.display())).unwrap_or_default();
        let first = hung.lines().next().unwrap_or("");
        assert!(first.starts_with("== its processes did nothing (no test result, under 5% of a core, no I/O) for ") && first.contains(", past twice its measured 2 s;"), "read as its tests running with no progress: {first:?} (its output: {:?})", std::fs::read_to_string(&out).unwrap_or_default());
        assert!(hung.contains("-- threads"), "its state kept: {hung}");
        assert!(stopped, "its processes are stopped");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A unit is hung only when its tests make no progress for the quiet: no new test result and its
    /// processes doing nothing, counted while a target's tests run. A build is never a hang (a cold
    /// JIT's compile, its wait for the box's JIT store), nor is a unit at work however long it takes:
    /// a bound from its warm measure failed a cold JIT's `makepad-fab --bin` HUNG at 672 s against a
    /// warm 66 s, its first frame at 471 s and a test thread at a core in JIT code (11-09). The watch's
    /// body reads a fake /proc and a fake output on a fake clock here: a real spinning unit on a
    /// loaded box's tests got under a tenth of a core (11-08). Its peak (GATEPEAK) is the most it held
    /// at once, never its processes' own peaks summed: those read a force_mode unit's 12 GB as 30 GB
    /// (11-10).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_is_hung_only_when_its_tests_make_no_progress() {
        let body = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/unit_watch_body.sh");
        let root = std::env::temp_dir().join(format!("devhub-bucket-unit-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // its session's one process, a pid past Linux's limit (2^22): none the watch's kill reaches
        let pid = 4_194_401u32;
        // the watch on a fake clock, a wait taking no time: second t's /proc is <case>/proc/<t>, its
        // processes `mem(t)`'s (pid, command line, VmHWM and VmRSS in KB), the session's own at `cpu(t)`
        // ticks; its output is `outs`' text from the second each is given on; a stall is said after 5 s
        // with no new result; a watch that has not ended by 40 s is stopped as the gate's TERM does,
        // not hung; the JIT's why-hung of its process answers `why_hung` (none: "").
        // What it kept: the first lines of <out>.hung and <out>.busy, <out>.peak,
        // what it said live (its fd 4) and <out>.peak.by. `quiet`: its HUNG_QUIET (2 s in `watch`), `measure`
        // its MEASURE_S (none in `watch`)
        let watch_held = |case: &str, why_hung: &str, cpu: &dyn Fn(u64) -> u64, outs: &[(u64, &str)], mem: &dyn Fn(u64) -> Vec<(u32, &'static str, u64, u64)>, quiet: u64, measure: u64| -> (String, String, String, String, String) {
            let dir = root.join(case);
            let proc = dir.join("proc");
            std::fs::create_dir_all(&dir).unwrap();
            let b = if why_hung.is_empty() { "true".to_string() } else {
                std::fs::write(dir.join("why-hung"), why_hung).unwrap();
                std::fs::write(dir.join("inspect.sh"), format!("[ \"$3\" != why-hung ] || cat {}\n", dir.join("why-hung").display())).unwrap();
                format!("bash {}", dir.join("inspect.sh").display())
            };
            for t in 0..=40u64 {
                for (p, cmd, hwm, rss) in mem(t) {
                    let task = proc.join(t.to_string()).join(p.to_string());
                    let thread = task.join("task").join(p.to_string());
                    std::fs::create_dir_all(&thread).unwrap();
                    let comm = cmd.split(' ').next().unwrap();
                    let stat = format!("{p} ({comm}) R 1 {p} {pid} 0 -1 4194304 100 0 0 0 {} 0 0 0 20 0 1 0\n", if p == pid { cpu(t) } else { 0 });
                    for dir in [&task, &thread] {
                        std::fs::write(dir.join("stat"), &stat).unwrap();
                        std::fs::write(dir.join("wchan"), "0").unwrap();
                    }
                    std::fs::write(task.join("io"), "read_bytes: 0\nwrite_bytes: 0\n").unwrap();
                    std::fs::write(task.join("status"), format!("VmPeak:\t{hwm} kB\nVmHWM:\t{hwm} kB\nVmRSS:\t{rss} kB\nVmData:\t{rss} kB\n")).unwrap();
                    std::fs::write(task.join("cmdline"), cmd.split(' ').map(|a| format!("{a}\0")).collect::<String>()).unwrap();
                }
            }
            let out = dir.join("unit");
            std::fs::write(&out, outs.iter().find(|(t, _)| *t == 0).map_or("", |(_, text)| *text)).unwrap();
            for (t, text) in outs {
                std::fs::write(format!("{}.at{t}", out.display()), text).unwrap();
            }
            let script = dir.join("fake_watch.sh");
            std::fs::write(&script, format!(
                "now=0 nap=\"\" proc={0}/0\nwait_s() {{ now=$((now + $1)); [ \"$now\" -le 40 ] || {{ stop; exit 0; }}; proc={0}/$now; [ ! -f \"$out.at$now\" ] || cp \"$out.at$now\" \"$out\"; }}\nread_gpu() {{ :; }}\nwall() {{ echo $((1794000000 + now)); }}\nexec 4> \"$2.live\"\n. {1}\n",
                proc.display(), body.display())).unwrap();
            assert!(std::process::Command::new("bash").arg(&script).arg(pid.to_string()).arg(&out)
                .env("HUNG_LOOK", "1").env("HUNG_QUIET", quiet.to_string()).env("MEASURE_S", measure.to_string()).env("BUSY_S", "5").env("BUSY_UNIT", "test p").env("DEVHUB_KEEP", dir.join("keep"))
                .env("TMPDIR", &dir).env("B", &b).status().unwrap().success());
            let kept = |ext: &str| std::fs::read_to_string(format!("{}.{ext}", out.display())).unwrap_or_default();
            let first = |ext: &str| kept(ext).lines().next().unwrap_or("").to_string();
            if !kept("busy").is_empty() {
                assert_eq!(std::fs::read_to_string(dir.join("keep").join(case).join("unit.busy")).unwrap(), kept("busy"), "{case}: its sample in the lease's keep dir");
            }
            (first("hung"), kept("peak"), first("busy"), kept("live"), kept("peak.by"))
        };
        let watch = |case: &str, why_hung: &str, cpu: &dyn Fn(u64) -> u64, outs: &[(u64, &str)], mem: &dyn Fn(u64) -> Vec<(u32, &'static str, u64, u64)>| watch_held(case, why_hung, cpu, outs, mem, 2, 0);
        let one = |_: u64| vec![(pid, "python3", 10_240u64, 10_240u64)];
        const GB: u64 = 1 << 20;
        // the test's process at 4 GB from 2 s to 5 s, a build it set off at half a GB and then at 3 GB from
        // 5 s, gone at 8 s: 4.5 GB at once at most, against 7 GB of their own peaks summed
        let two = |t: u64| {
            let mut ps = vec![(pid, "python3 -I -c spin", if t >= 2 { 4 * GB } else { GB }, if (2..5).contains(&t) { 4 * GB } else { GB })];
            if t < 8 { ps.push((pid + 1, "rustc --crate-name dep", if t >= 5 { 3 * GB } else { GB / 2 }, if t >= 5 { 3 * GB } else { GB / 2 })); }
            ps
        };
        let hung_at = |s: u64| format!("== its processes did nothing (no test result, under 5% of a core, no I/O) for 2 s while its tests ran, {s} s into the unit; their state");
        // a core's work each second for 20 s, far past any measure, one result at 8 s, then nothing
        let (hung, peak, busy, live, by) = watch("busy", "", &|t| 100 * t.min(20), &[(0, "running 2 tests\n"), (8, "running 2 tests\ntest a ... ok\n")], &two);
        assert_eq!(hung, hung_at(22), "busy: a unit at work is no hang, however long it works");
        assert_eq!(peak.trim(), "4608", "busy: its peak in MB is the most its processes held at once, not their own peaks summed (7168)");
        assert_eq!(by.trim(), "4.0 GB python3 -I -c spin; 0.5 GB rustc --crate-name dep", "busy: its processes at its peak, largest first, by command");
        let top = "the busiest thread: 4194401/task/4194401 python3 R 100% wchan 0";
        assert_eq!(busy, format!("== busy: no new test result for 5 s while its tests ran, 13 s into the unit; nothing stops it; {top}; their state"), "busy: its last stall's sample");
        assert_eq!(live, format!("GATEBUSY test p :: 1794000000 busy/unit.busy {top}\nGATEBUSY test p :: 0\nGATEBUSY test p :: 1794000008 busy/unit.busy {top}\n"),
            "busy: each stall said once with its last result's time, a new result clearing it");
        // nothing for 10 s before its target's tests start (its build waits), then nothing
        // its process at 2 GB for a moment between two looks, held at 0.1 GB at each
        let spike = |t: u64| vec![(pid, "python3", if t >= 3 { 2 * GB } else { GB / 10 }, GB / 10)];
        let (hung, peak, busy, live, by) = watch("build", "", &|_| 0, &[(0, ""), (10, "running 1 test\n")], &spike);
        assert_eq!(hung, hung_at(11), "build: a build is never a hang; the quiet counts from its tests' start");
        assert_eq!((busy.as_str(), live.as_str()), ("", ""), "build: a build is no stall");
        assert_eq!((peak.trim(), by.trim()), ("2048", "2.0 GB python3 (its own peak, between looks)"), "build: one process's own peak between looks counts");
        // its processes idle, a test result every 2 s
        let (hung, peak, busy, live, _) = watch("results", "", &|_| 0, &[(0, "running 3 tests\n"), (2, "running 3 tests\ntest a ... ok\n"),
            (4, "running 3 tests\ntest a ... ok\ntest b ... ok\n"), (6, "running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n")], &one);
        assert_eq!(peak.trim(), "10", "results: its peak in MB");
        assert_eq!(hung, hung_at(8), "results: a new test result is progress");
        assert_eq!((busy.as_str(), live.as_str()), ("", ""), "results: no stall");
        assert!(!root.join("results").join("unit.ended").exists(), "results: a unit whose process lives has not ended");
        // its tests done, then no process of its session left: the unit ended, which the watch says and ends
        // with, never a build going on for ever (a gate whose wait never returned for a unit that had ended sat
        // on its watch 10 min, L1483898, 11-21)
        let done = "running 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n";
        let (hung, ..) = watch("ended", "", &|_| 0, &[(0, done)], &|t| if t < 4 { one(t) } else { Vec::new() });
        assert_eq!(hung, "", "ended: no hang");
        assert_eq!(std::fs::read_to_string(root.join("ended").join("unit.ended")).unwrap_or_default(), "its session has no process left, 4 s into the unit: the unit ended\n", "ended: said at the first look with nothing left");
        // a target's tests done and nothing while the next one builds; then that target's tests, one of
        // them run again in a child of its own whose output is the unit's (the harness's self-exec: its
        // own `running 1 test` to `test result:` inside the target's), then nothing: the target's tests
        // still run. Each such child's result read as its target's end, and relax-compiler's tests,
        // idle in a later test, were a build to the watch: they held their shards' boxes 28 min to 1h38m,
        // and the runs ended with no verdict, three times (11-11, 11-12)
        let first = "running 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n";
        let next = format!("{first}\nrunning 3 tests\ntest b ... ok\n");
        let child = format!("{next}     Running unittests src/lib.rs\n\nrunning 1 test\ntest c ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out\n\ntest c ... ok\n");
        let (hung, _, busy, live, _) = watch("nested", "", &|_| 0, &[(0, first), (6, &next), (7, &child)], &one);
        assert_eq!(hung, hung_at(9), "nested: a child's own tests inside its target's do not end them; between targets is a build");
        assert_eq!((busy.as_str(), live.as_str()), ("", ""), "nested: no stall");
        // a core's CPU and no result, its main idle and the busy thread in a GPU driver alone (a fence
        // wait spinning: a full run's `test makepad-ai-vulkan`, measured 37 s, sat 35 min, 11-12): no
        // work of its own, hung after the quiet; the same thread with a frame of its own code is work
        let spin = |frames: &str| format!("{{\"verdict\":\"idle\",\"why\":\"main is not stuck: 0% CPU\"}}\n\
            {{\"thread\":\"main\",\"id\":\"T1\",\"state\":\"waiting\",\"cpu_pct\":0,\"frames\":[{{\"fn\":\"libc.so.6+0x7c952\",\"in\":\"libc.so.6\"}}]}}\n\
            {{\"thread\":\"t\",\"id\":\"T28\",\"state\":\"running\",\"cpu_pct\":94,\"frames\":[{frames}]}}\n\
            {{\"other_threads\":[\"[vkcf] Analysis T25 waiting 0%: libc.so.6+0x7c952\"]}}\n");
        let (hung, _, busy, _, _) = watch("driver", &spin("{\"fn\":\"libnvidia-eglcore.so.1+0x7643d6\",\"in\":\"libnvidia-eglcore.so.1\"}"), &|t| 100 * t, &[(0, "running 1 test\n")], &one);
        assert_eq!(hung, format!("== its processes did no work of their own (no test result, no I/O; their CPU a spin outside their code: pid {pid}: main idle, T28 94% in libnvidia-eglcore.so.1) for 2 s while its tests ran, 2 s into the unit; the busiest thread: {pid}/task/{pid} python3 R 100% wchan 0; their state"), "driver: a spin in a driver alone is hung");
        assert_eq!(busy, "", "driver: hung before its stall is said");
        let (hung, _, busy, _, _) = watch("own", &spin("{\"fn\":\"lib.so+0x10\",\"in\":\"lib.so\"},{\"fn\":\"m::t::wait_go\",\"in\":\"makepad.jit-1\"}"), &|t| 100 * t, &[(0, "running 1 test\n")], &one);
        assert_eq!(hung, "", "own: a busy thread in its own code is work, however long");
        assert!(busy.starts_with("== busy: no new test result for 5 s"), "own: said as a stall: {busy}");
        // a core's CPU and no result, its JIT saying main has waited on the owner past the quiet and the owner
        // on a build it holds itself (a full run's `ui_semantics`, measured 30 s, sat 48 min, the owner
        // spinning, 11-19): hung, its line naming the build and its holder. Main blocked on the JIT for less
        // than the quiet is a compile at work, and main blocked on its own program's wait is the program's
        let blocked = |why: &str| format!("{{\"verdict\":\"blocked\",\"why\":\"main is blocked: {why}\",\"findings\":[]}}\n\
            {{\"thread\":\"run-owner\",\"id\":\"T37\",\"state\":\"running\",\"cpu_pct\":61,\"frames\":[{{\"fn\":\"relax_compiler::jit::wait_inflight\",\"in\":\"makepad.jit-1\"}}]}}\n");
        let chain = |s: &str| blocked(&format!("domain-1-makepa (T47) waits: owner (program request) for {s} s; owner serves program for thread `run-jit-1` in jit 0x7f0e2340009d -> run-owner (T37) waits: build #292211 core::mem::drop held by run-owner (T37) for 609.2 s in syscall"));
        let (hung, _, busy, _, _) = watch("jit", &chain("608.8"), &|t| 100 * t, &[(0, "running 1 test\n")], &one);
        assert_eq!(hung, format!("== its JIT held it (no test result, no I/O; pid {pid}: main blocked on its JIT for 608.8 s, on build #292211 core::mem::drop, whose recorded holder is run-owner (T37)) for 2 s while its tests ran, 2 s into the unit; the busiest thread: {pid}/task/{pid} python3 R 100% wchan 0; their state"), "jit: a deadlock its JIT reports is a hang, named by its build and holder");
        assert_eq!(busy, "", "jit: hung before its stall is said");
        let (hung, ..) = watch("compiling", &chain("1.5"), &|t| 100 * t, &[(0, "running 1 test\n")], &one);
        assert_eq!(hung, "", "compiling: main on its JIT for less than the quiet is work");
        let (hung, ..) = watch("parked", &blocked("makepad-fab (T1) waits: park 0x558aad18f268 for 613.5 s in std::sys::os::futex_wait"), &|t| 100 * t, &[(0, "running 1 test\n")], &one);
        assert_eq!(hung, "", "parked: main waiting on its own program is the program's, not its JIT's");
        // held to its measure: its tests run from the start and its processes do nothing. Measured 3 s, past
        // twice that (6 s) a quiet of its measure fails it, with its capture, long before the 30 s quiet;
        // measured 20 s it is still within twice its measure at 30 s; at work past twice its measure, it
        // runs on. `test relax-runner`, measured 27 s, sat 9 min parked on its child's output (L939783, 11-13)
        let idle = [(0, "running 1 test\n")];
        let (hung, ..) = watch_held("measured", "", &|_| 0, &idle, &one, 30, 3);
        assert_eq!(hung, "== its processes did nothing (no test result, under 5% of a core, no I/O) for 6 s while its tests ran, 6 s into the unit, past twice its measured 3 s; their state", "measured: past twice its measure, its measure's quiet");
        let (hung, ..) = watch_held("within", "", &|_| 0, &idle, &one, 30, 20);
        assert_eq!(hung, "== its processes did nothing (no test result, under 5% of a core, no I/O) for 30 s while its tests ran, 30 s into the unit; their state", "within: under twice its measure, the long quiet");
        let (hung, _, busy, ..) = watch_held("working", "", &|t| 100 * t, &idle, &one, 30, 3);
        assert_eq!(hung, "", "working: a unit at work past twice its measure runs on");
        assert!(busy.starts_with("== busy: no new test result for 5 s"), "working: said as a stall: {busy}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A shard's class names its shape and its bucket: another shape's or the full bucket's
    /// measures are not its own.
    #[test]
    fn a_shards_class_is_its_shape_and_its_bucket() {
        assert_eq!(shard_class(12, false), format!("test bash land_gate.sh testrun 12 cores @{}", crate::lease::sched::TESTS_LANE));
        assert_ne!(shard_class(8, false), shard_class(12, false), "another shape measures apart");
        assert_eq!(shard_class(12, true), format!("test bash land_gate.sh testrun full 12 cores @{}", crate::lease::sched::TESTS_LANE), "the full bucket's measure apart");
    }

    /// The tests bucket takes a shard per SHARD_CORES of test cores on each up Linux x64 pool box
    /// with dev on and the RAM for one (its shared CPUs when pinned), each shard with the test
    /// cores of the smallest, at most SHARD_CORES; a bench, a user's box, a Mac, a down, draining
    /// or small-RAM box takes none, nor one on another rustc than the master's.
    #[test]
    fn the_tests_bucket_takes_a_shard_per_pool_box() {
        let node_ram = |name: &str, role: &str, up: bool, dev: bool, draining: bool, os: &str, arch: &str, cores: u32, shared: u32, ram: f64| format!(
            r#"{{"node":"{name}","up":{up},"dev":{dev},"role":"{role}","draining":{draining},"caps":{{"os":"{os}","arch":"{arch}","cores":{cores},"cpus_shared":{shared},"ram_gb":{ram}}}}}"#);
        let node = |name: &str, role: &str, up: bool, dev: bool, draining: bool, os: &str, arch: &str, cores: u32, shared: u32| node_ram(name, role, up, dev, draining, os, arch, cores, shared, 62.0);
        let nodes = vec![
            node("boxa", "pool", true, true, false, "linux", "x86_64", 16, 0),
            node("boxb", "pool", true, true, false, "linux", "x86_64", 32, 16),
            node("boxc", "pool", true, true, false, "linux", "x86_64", 32, 0),
            node("bench", "bench", true, true, false, "linux", "x86_64", 8, 0),
            node("userbox", "user-wm", true, true, false, "linux", "x86_64", 8, 0),
            node("mac", "pool", true, true, false, "macos", "aarch64", 8, 0),
            node("win", "pool", true, true, false, "windows", "x86_64", 8, 0),
            node("down", "pool", false, true, false, "linux", "x86_64", 8, 0),
            node("off", "pool", true, false, false, "linux", "x86_64", 8, 0),
            node("leaving", "pool", true, true, true, "linux", "x86_64", 8, 0),
        ];
        // boxa 1, boxb (pinned to 16 of 32) 1, boxc (32 cores, RAM for one) 1
        let unmeasured = |_: u32| None;
        assert_eq!(pool_shape(&nodes, "", &unmeasured), Ok((3, SHARD_CORES)));
        let lean = node_ram("lean", "pool", true, true, false, "linux", "x86_64", 16, 0, 30.0);
        assert_eq!(pool_shape(&[nodes[0].clone(), lean.clone()], "", &unmeasured), Ok((1, SHARD_CORES)), "a box short of a shard's RAM takes none");
        let big = node_ram("big", "pool", true, true, false, "linux", "x86_64", 32, 0, 96.0);
        assert_eq!(pool_shape(&[nodes[0].clone(), big.clone()], "", &unmeasured), Ok((3, SHARD_CORES)), "the cores and RAM for two: two");
        assert_eq!(pool_shape(&[node("small", "pool", true, true, false, "linux", "x86_64", 4, 0)], "", &unmeasured), Ok((1, 4)), "a smaller box: its cores");
        assert!(pool_shape(&nodes[3..], "", &unmeasured).is_err(), "no box: no run");
        let on = |v: &str| nodes[0].replace(r#""os":"linux""#, &format!(r#""os":"linux","rustc":"{v}""#));
        assert_eq!(pool_shape(&[on("1.100.0"), on("1.99.1")], "1.100.0", &unmeasured), Ok((1, SHARD_CORES)), "a box on another rustc than the master's takes none");
        // the shard's class measured at 20 GB, as the scheduler charges it: the 30 GB box takes
        // one, the 96 GB box's 32 cores two; at 12-core shards' cores, asked for that class
        let asked = std::cell::Cell::new(0);
        let twenty = |cores: u32| { asked.set(cores); Some(20.0) };
        assert_eq!(pool_shape(&[nodes[0].clone(), lean.clone(), big], "", &twenty), Ok((4, SHARD_CORES)), "measured: the RAM the scheduler charges");
        assert_eq!(asked.get(), SHARD_CORES);
        assert_eq!(pool_shape(&[lean.clone()], "", &|_| Some(30.0)), Err("no up pool box has the RAM for a tests shard (30.0 GB)".to_string()), "a measure past a box's budget: none there");
        // the measure as the master answers the bucket's ask: its class's p90 peak and a fifth
        let mut sched = crate::lease::sched::Sched::new("master", 1);
        let ask = |s: &crate::lease::sched::Sched| shard_ram_of(&[crate::lease::master::estimate_answer(s, &shard_ram_ask(SHARD_CORES))]);
        assert_eq!(ask(&sched), None, "no measure: none");
        sched.record_resources(&shard_class(SHARD_CORES, false), "test", Some(16.5), None);
        assert_eq!(ask(&sched), Some(19.8), "the shard class's peak, measured, reaches the bucket");
    }

    /// The run's one queue: each want gets the next unit, the slowest first; a lease that ends
    /// without its verdict gives its units back, dealt first; dealt out, a want hears none and a
    /// shard not started is no longer needed. A restarted master deals on from its record and the
    /// shards' logs: no want answered twice, the answers of the leases now told again.
    #[test]
    fn a_tests_runs_queue_deals_a_unit_per_free_slot_and_a_lost_leases_again() {
        let mut d = Dealer::new(3);
        assert!(d.needed(), "no queue known yet: every shard is needed");
        assert_eq!(d.line(0, "GATEWANT L1 1", 0, 1), None, "no want before its queue");
        let mut record = Vec::new();
        let mut want = |d: &mut Dealer, shard: usize, lease: &str, n: u32| {
            let t = d.line(shard, &format!("GATEWANT {lease} {n}"), 0, 1).expect(lease);
            record.push(t.record);
            t.line
        };
        assert_eq!(d.line(0, "GATEQUEUE L1 4 3f2a", 0, 1), None);
        assert_eq!(want(&mut d, 0, "L1", 1), "1 0 3f2a");
        assert_eq!(want(&mut d, 0, "L1", 2), "2 1 3f2a");
        d.line(1, "GATEQUEUE L2 4 3f2a", 0, 1);
        assert_eq!(want(&mut d, 1, "L2", 1), "1 2 3f2a");
        assert_eq!(d.line(1, "GATEWANT L2 1", 0, 1), None, "a want is answered once");
        // L1 is lost before its verdict: its two units go back, dealt first, the slowest first
        d.line(0, RERUN, 0, 1);
        assert_eq!(want(&mut d, 1, "L2", 2), "2 0 3f2a");
        d.line(0, "GATEQUEUE L3 4 3f2a", 0, 1);
        assert_eq!(want(&mut d, 0, "L3", 1), "1 1 3f2a");
        assert!(d.needed());
        assert_eq!(want(&mut d, 0, "L3", 2), "2 3 3f2a");
        assert!(!d.needed(), "dealt out: a shard not started is not needed");
        assert_eq!(want(&mut d, 1, "L2", 3), "3 -");
        // a lease that said its verdict gives nothing back
        d.line(1, "NODEGATE-OK testrun 2 packages", 0, 1);
        d.over(1);
        assert!(!d.needed());
        assert_eq!(record, ["L1 1 0", "L1 2 1", "L2 1 2", "L2 2 0", "L3 1 1", "L3 2 3", "L2 3 -"]);
        let mut again = Dealer::new(3);
        again.replay(&record.join("\n"));
        for (shard, l) in [(0, "GATEQUEUE L1 4 3f2a"), (0, "GATEWANT L1 1"), (0, "GATEWANT L1 2"), (1, "GATEQUEUE L2 4 3f2a"), (1, "GATEWANT L2 1"), (0, RERUN), (1, "GATEWANT L2 2"), (0, "GATEQUEUE L3 4 3f2a"), (0, "GATEWANT L3 1"), (0, "GATEWANT L3 2"), (1, "GATEWANT L2 3")] {
            assert_eq!(again.line(shard, l, 0, 1), None, "{l}");
        }
        assert!(!again.needed(), "L1's units were dealt again before the restart: none back");
        assert_eq!(again.answers_now().iter().map(|t| t.line.as_str()).collect::<Vec<_>>(), ["1 2 3f2a", "2 0 3f2a", "3 -", "1 1 3f2a", "2 3 3f2a"]);
        assert_eq!(again.line(1, "GATEWANT L2 4", 0, 1).map(|t| t.line).as_deref(), Some("4 -"));
        // lost and not dealt again before the restart: back after it too
        let mut d = Dealer::new(2);
        d.replay("L5 1 0\nL5 2 1\n");
        for l in ["GATEQUEUE L5 2 aa", "GATEWANT L5 1", "GATEWANT L5 2", RERUN] {
            assert_eq!(d.line(0, l, 0, 1), None, "{l}");
        }
        assert!(d.needed());
        d.line(1, "GATEQUEUE L6 2 aa", 0, 1);
        assert_eq!(d.line(1, "GATEWANT L6 1", 0, 1).map(|t| t.line).as_deref(), Some("1 0 aa"));
        assert!(!d.live("L5") && d.live("L6"));
    }

    /// A full run's lease gives its pool slot to runs that go first ([`Board::short_before_full`])
    /// as its next unit would be dealt: told dealt out, recorded `yield`, its shard lets the slot
    /// go once and asks again. No more of its leases give than slots are waited for, never its last
    /// working one (its floor), and a dealt-out want is no yield. A restarted master knows who was told.
    /// A yielded shard's log keeps every part, a lost lease's run cut ([`kept`]).
    #[test]
    fn a_full_runs_shard_gives_its_slot_to_a_tests_run_as_its_unit_ends() {
        let mut d = Dealer::new(3);
        let mut record = Vec::new();
        let mut want = |d: &mut Dealer, shard: usize, lease: &str, n: u32, waiting: usize| {
            let t = d.line(shard, &format!("GATEWANT {lease} {n}"), waiting, 1).expect(lease);
            record.push(t.record);
            t.line
        };
        for (i, l) in ["A", "B", "C"].iter().enumerate() { d.line(i, &format!("GATEQUEUE {l} 9 dd"), 0, 1); }
        assert_eq!([want(&mut d, 0, "A", 1, 0), want(&mut d, 1, "B", 1, 0), want(&mut d, 2, "C", 1, 0)], ["1 0 dd", "1 1 dd", "1 2 dd"]);
        // a tests run's shard waits: A's next want gives its slot up, and B's takes a unit (one
        // slot is given already); A, once told, gives on
        assert_eq!(want(&mut d, 0, "A", 2, 1), "2 -");
        assert_eq!(want(&mut d, 1, "B", 2, 1), "2 3 dd");
        assert_eq!(want(&mut d, 0, "A", 3, 0), "3 -");
        // two wait: B gives too, and C, the last shard working, works on while A and B give theirs
        assert_eq!(want(&mut d, 1, "B", 3, 3), "3 -");
        assert_eq!(want(&mut d, 2, "C", 2, 2), "2 4 dd");
        assert!(d.take_yield(0) && !d.take_yield(0) && !d.take_yield(2), "A's shard lets its slot go, once");
        d.line(0, "NODEGATE-OK testrun", 0, 1);
        d.line(0, YIELDED, 0, 1);
        assert_eq!(want(&mut d, 2, "C", 3, 1), "3 5 dd", "A judged its units: none back");
        // A's shard is back on a new lease: with one slot waited for and B giving it, A2 works
        d.line(0, "GATEQUEUE A2 9 dd", 0, 1);
        assert_eq!(want(&mut d, 0, "A2", 1, 1), "1 6 dd");
        assert!(d.take_yield(1), "B lets its slot go");
        d.line(1, YIELDED, 0, 1);
        assert_eq!(want(&mut d, 0, "A2", 2, 1), "2 -", "one more is waited for: A2 gives, C works on");
        assert_eq!(want(&mut d, 2, "C", 4, 0), "4 1 dd", "B's part ended without its verdict: its units are dealt again first");
        assert!(d.needed(), "the run's queue is dealt on");
        assert_eq!(record, ["A 1 0", "B 1 1", "C 1 2", "A 2 yield", "B 2 3", "A 3 yield", "B 3 yield", "C 2 4", "C 3 5", "A2 1 6", "A2 2 yield", "C 4 1"]);
        // a restarted master: A2 was told, and its shard lets its slot go
        let mut restarted = Dealer::new(3);
        restarted.replay(&record.join("\n"));
        for (shard, l) in [(0, "GATEQUEUE A 9 dd"), (1, "GATEQUEUE B 9 dd"), (2, "GATEQUEUE C 9 dd"), (0, "GATEQUEUE A2 9 dd")] { restarted.line(shard, l, 0, 1); }
        assert!(restarted.take_yield(0) && !restarted.take_yield(2));
        // its last working shard keeps its slot, the run's floor, whatever waits: giving it too
        // starved a full run 80 min behind tests runs asked back to back (11-19)
        let mut last = Dealer::new(2);
        last.line(0, "GATEQUEUE L 680 aa", 0, 1);
        last.line(1, "GATEQUEUE M 680 aa", 0, 1);
        assert_eq!(last.line(0, "GATEWANT L 1", 0, 1).map(|t| t.line).as_deref(), Some("1 0 aa"));
        assert_eq!(last.line(1, "GATEWANT M 1", 0, 1).map(|t| t.line).as_deref(), Some("1 1 aa"));
        assert_eq!(last.line(0, "GATEWANT L 2", 4, 1).map(|t| t.record).as_deref(), Some("L 2 yield"));
        assert_eq!(last.line(1, "GATEWANT M 2", 4, 1).map(|t| t.line).as_deref(), Some("2 2 aa"), "four wait, but M is the run's last working shard");
        assert!(last.take_yield(0) && !last.take_yield(1), "L lets its slot go; M keeps its own");
        last.line(0, YIELDED, 0, 1);
        last.line(0, "GATEQUEUE L2 680 aa", 0, 1);
        assert_eq!(last.line(0, "GATEWANT L2 1", 4, 1).map(|t| t.record).as_deref(), Some("L2 1 yield"), "back on a lease while M works, L's shard gives again");
        assert_eq!(last.line(1, "GATEWANT M 3", 4, 1).map(|t| t.line).as_deref(), Some("3 0 aa"), "M works on, L's unit first");
        // dealt out, a want hears none and gives nothing: its shard just ends
        let mut out = Dealer::new(2);
        out.line(0, "GATEQUEUE X 1 ee", 0, 1);
        out.line(1, "GATEQUEUE Y 1 ee", 0, 1);
        assert_eq!(out.line(0, "GATEWANT X 1", 0, 1).map(|t| t.line).as_deref(), Some("1 0 ee"));
        assert_eq!(out.line(1, "GATEWANT Y 1", 5, 1).map(|t| t.record).as_deref(), Some("Y 1 -"));
        assert!(!out.take_yield(1));
        // its box wanted for timed work (a lent bench box's $DEVHUB_YIELD): its gate asks for no
        // more units, and its shard lets its slot go once they end and asks for a lease again
        let mut lent = Dealer::new(2);
        lent.line(0, "GATEQUEUE P 4 ff", 0, 1);
        lent.line(1, "GATEQUEUE Q 4 ff", 0, 1);
        assert_eq!(lent.line(0, "GATEWANT P 1", 0, 1).map(|t| t.line).as_deref(), Some("1 0 ff"));
        assert!(lent.line(0, "GATEYIELD P", 0, 1).is_none());
        assert!(lent.take_yield(0) && !lent.take_yield(0) && !lent.take_yield(1), "P's shard lets its slot go, once");
        assert_eq!(lent.line(1, "GATEWANT Q 1", 0, 1).map(|t| t.line).as_deref(), Some("1 1 ff"), "the rest goes to the run's other shards");
        assert!(lent.needed(), "and to P's shard's next lease");

        // the shard's log: the part that gave its slot, and the next part's last run
        let lost = format!("why\nGATEQUEUE A 9 dd\nGATESTART u0 1\nGATEUNIT ok u0\nNODEGATE-OK testrun\n{YIELDED}\nGATEQUEUE A2 9 dd\nGATESTART u6 2\n");
        let lost_status = Ok(Some(crate::lease::client::LOST_EXIT));
        assert!(again(&lost_status, &lost), "a lease lost after a yield runs again");
        let log = format!("{lost}{RERUN}\nGATEQUEUE A3 9 dd\nGATESTART u6 3\nGATEUNIT ok u6\nNODEGATE-OK testrun\n");
        assert!(!again(&lost_status, &log), "once a part");
        assert_eq!(kept(&log), format!("why\nGATEQUEUE A 9 dd\nGATESTART u0 1\nGATEUNIT ok u0\nNODEGATE-OK testrun\n{YIELDED}\nGATEQUEUE A3 9 dd\nGATESTART u6 3\nGATEUNIT ok u6\nNODEGATE-OK testrun\n"));
        assert_eq!(this_part(&log), "GATEQUEUE A3 9 dd\nGATESTART u6 3\nGATEUNIT ok u6\nNODEGATE-OK testrun\n");
        assert!(unjudged(&Ok(Some(0)), &kept(&log)).is_none(), "every unit judged: the lost run's start is cut");
        // a part goes on after a yield only once it said its verdict
        let at = std::env::temp_dir().join(format!("devhub-yield-{}.log", std::process::id()));
        std::fs::write(&at, &log).unwrap();
        assert!(verdict_said(&at));
        std::fs::write(&at, &lost).unwrap();
        assert!(!verdict_said(&at), "the part since the yield said none");
        let _ = std::fs::remove_file(&at);
        let twice = format!("GATEQUEUE A 9 dd\nGATESTART u0 1\n{RERUN}\nGATEQUEUE A1 9 dd\nNODEGATE-OK testrun\n{YIELDED}\nGATEQUEUE A2 9 dd\nGATESTART u1 2\n{RERUN}\nGATEQUEUE A3 9 dd\nNODEGATE-OK testrun\n");
        assert_eq!(kept(&twice), format!("GATEQUEUE A1 9 dd\nNODEGATE-OK testrun\n{YIELDED}\nGATEQUEUE A3 9 dd\nNODEGATE-OK testrun\n"), "each lost run is cut");
        assert!(again(&lost_status, &twice[..twice.rfind(RERUN).unwrap()]), "a part after a yield runs again though an earlier part ran again");
    }

    /// A master restart mid-run deals on the run's one queue: the adopted run reads the inputs its
    /// start saved, not the measures since (whose order is another queue), and a lease that made
    /// another queue all the same is told none of the run's, never a unit by another queue's index;
    /// a queue its shards ended before dealing out is no verdict. A full run across two restarts
    /// had three shards fail in 0 s on "the dealer's queue is not this shard's" and read red with
    /// no test failing (11-17).
    #[test]
    fn a_master_restart_mid_run_deals_on_the_runs_own_queue() {
        let dir = std::env::temp_dir().join(format!("devhub-requeue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("full-1000.log");
        let measured = |a: u64| QueueMade {
            why: vec!["full: every package's tests, no proof reused".into()],
            proven: "GATE_PROVEN=".into(), retry: "GATE_RETRY=".into(),
            times: format!("GATE_TIMES=test a={a};test b=5"), peaks: "GATE_PEAKS=test a=900".into(), jobs: "GATE_JOBS=test a=4".into(),
            tests: "GATE_TEST_TIMES=test a :: t::slow=300.0 t::quick=1.0".into(), floor: Some("0.9".into()),
        };
        let first = queue_made(&log, false, || measured(20));
        assert_eq!(first, measured(20), "a new run's queue is made from the measures now");
        // a run a master before the threads' measures saved: adopted all the same, its units on their slots' threads
        let saved = std::fs::read_to_string(QueueMade::file(&log)).unwrap();
        std::fs::write(QueueMade::file(&log), saved.lines().filter(|l| !l.starts_with("GATE_JOBS=") && !l.starts_with("GATE_TEST_TIMES=")).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        assert_eq!(QueueMade::read(&log).map(|q| (q.jobs, q.tests)), Some(("GATE_JOBS=".to_string(), "GATE_TEST_TIMES=".to_string())));
        std::fs::write(QueueMade::file(&log), saved).unwrap();
        let want = |d: &mut Dealer, shard: usize, lease: &str, n: u32| d.line(shard, &format!("GATEWANT {lease} {n}"), 0, 1).expect(lease);
        let mut d = Dealer::new(3);
        d.line(0, "GATEQUEUE L1 4 aa", 0, 1);
        d.line(1, "GATEQUEUE L2 4 aa", 0, 1);
        let told = [want(&mut d, 0, "L1", 1), want(&mut d, 0, "L1", 2), want(&mut d, 1, "L2", 1)];
        assert_eq!(told.iter().map(|t| t.line.as_str()).collect::<Vec<_>>(), ["1 0 aa", "2 1 aa", "1 2 aa"]);
        let record = told.iter().map(|t| t.record.as_str()).collect::<Vec<_>>().join("\n");

        // the master restarts: the measures moved on, and the run deals by its start's
        assert_eq!(queue_made(&log, true, || measured(3)), first, "an adopted run's queue is its start's");
        let mut restarted = Dealer::new(3);
        restarted.replay(&record);
        for (shard, l) in [(0, "GATEQUEUE L1 4 aa"), (0, "GATEWANT L1 1"), (0, "GATEWANT L1 2"), (1, "GATEQUEUE L2 4 aa"), (1, "GATEWANT L2 1"), (1, RERUN), (1, "GATEQUEUE L3 4 aa")] {
            assert_eq!(restarted.line(shard, l, 0, 1), None, "{l}");
        }
        assert_eq!(want(&mut restarted, 1, "L3", 1).line, "1 2 aa", "L2 was lost: its unit is dealt again first, by the run's queue");
        // a lease that made another queue (a run adopted from a master that saved none) hears
        // none of the run's, and again after another restart
        restarted.line(2, "GATEQUEUE L4 4 bb", 0, 1);
        let told = want(&mut restarted, 2, "L4", 1);
        assert_eq!((told.line.as_str(), told.record.as_str()), ("1 - aa", "L4 1 -"), "another queue's lease takes none of the run's");
        assert!(restarted.answers_now().iter().any(|t| t.lease == "L4" && t.line == "1 - aa"));
        assert_eq!(restarted.undealt(), Some((1, 4)), "its shards ending now would leave a unit never dealt: no verdict");
        assert_eq!(want(&mut restarted, 0, "L1", 3).line, "3 3 aa");
        assert_eq!(restarted.undealt(), None, "dealt out");
        // a unit told under another queue all the same: the gate ends as a lost lease's run
        // (land_gate.sh `exit 4`, no verdict line), which runs once more, never a red
        let other = "GATEQUEUE L5 4 bb\nGATEWANT L5 1\ntestrun: the dealer's queue aa is not this shard's bb: no verdict from this shard\nSTEP testrun 0 s\ntestrun: no verdict: told a unit of another queue [lease L5]\n";
        assert_eq!(crate::lease::client::LOST_EXIT, 4, "land_gate.sh's exit for a unit of another queue");
        assert!(again(&Ok(Some(4)), other), "it runs once more");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A gate's queue values ride as its lease's input, one file each by its variable's name, never
    /// in its command's environment, which every process of its units inherited ([`QueueInput`]):
    /// `tar` reads the archive back without a word, the same values make the same bytes (an adopted
    /// run's shards find it held), and the run's end takes it from beside its log.
    #[test]
    fn a_gates_queue_values_ride_as_its_leases_input_files() {
        let dir = std::env::temp_dir().join(format!("devhub-queue-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let log = dir.join("tests-1.log");
        let times = format!("GATE_TIMES={}", (0..3000).map(|i| format!("test p{i}=12.5")).collect::<Vec<_>>().join(";"));
        let vars = ["GATE_PROVEN=test a;test b", times.as_str(), "GATE_PEAKS=", "GATE_TEST_TIMES=test a :: t::x=1.0"];
        let input = QueueInput::write(&log, &vars).unwrap();
        assert_eq!(input.arg, format!("GATE_QUEUE_DIR={}", dir.join("tests-1.log.queue.tar").display()));
        assert_eq!(std::fs::read(&input.file).unwrap(), queue_tar(&vars), "the same values, the same bytes");
        let o = std::process::Command::new(crate::os::tool("tar")).arg("-xf").arg(&input.file).arg("-C").arg(dir.join("out")).output().unwrap();
        assert!(o.status.success() && o.stderr.is_empty(), "tar reads it back without a word: {}", String::from_utf8_lossy(&o.stderr));
        for var in vars {
            let (name, value) = var.split_once('=').unwrap();
            assert_eq!(std::fs::read_to_string(dir.join("out").join(name)).unwrap(), value, "{name} is its file");
        }
        let file = input.file.clone();
        drop(input);
        assert!(!file.exists(), "it goes with its run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dealer reads a shard's log as it grows: whole lines once, a log made again from its start.
    #[test]
    fn a_growing_log_is_read_a_whole_line_once() {
        let path = std::env::temp_dir().join(format!("devhub-grown-{}.log", std::process::id()));
        let mut seen = (0, String::new());
        assert!(grown(&path, &mut seen).is_empty(), "no log yet");
        std::fs::write(&path, "GATEQUEUE L1 3 aa\nGATEWA").unwrap();
        assert_eq!(grown(&path, &mut seen), ["GATEQUEUE L1 3 aa"]);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"NT L1 1\nGATEWANT L1 2\n").unwrap();
        assert_eq!(grown(&path, &mut seen), ["GATEWANT L1 1", "GATEWANT L1 2"]);
        assert!(grown(&path, &mut seen).is_empty());
        std::fs::write(&path, "L2 test\n").unwrap();
        assert_eq!(grown(&path, &mut seen), ["L2 test"]);
        let _ = std::fs::remove_file(&path);
    }

    /// Shards log beside their run, named so a restarted master adopts the run, not them; the
    /// run's log is its why lines and each shard's; its status the worst of theirs.
    #[test]
    fn a_sharded_run_logs_each_shard_and_takes_the_worst_status() {
        let log = Path::new("/n/home/bucket-logs/tests-1794010109906.log");
        assert_eq!(shard_log(log, 2), Path::new("/n/home/bucket-logs/tests-s2-1794010109906.log"));
        assert_eq!(log_started(&shard_log(log, 2)), Some(1794010109906), "its lease_run reads its start");
        let ok = |n: u32| (shard_log(log, n as usize), (Ok(Some(0)), format!("GATEUNIT ok test p{n}\nNODEGATE-OK testrun 1 packages\n")));
        let failed = (shard_log(log, 3), (Ok(Some(1)), "GATEUNIT fail test p3\nNODEGATE-FAIL: test:p3".to_string()));
        let text = merged(&["why".to_string()], (2, 16), &[ok(1), failed.clone()]);
        assert!(text.starts_with("SHARDS 2 16\nwhy\n== shard 1/2: /n/home/bucket-logs/tests-s1-1794010109906.log\nGATEUNIT ok test p1\n"), "{text}");
        assert!(text.ends_with("NODEGATE-FAIL: test:p3\n"), "{text}");
        assert_eq!(worst(&[ok(1), ok(2)]), Ok(Some(0)));
        assert_eq!(worst(&[ok(1), failed.clone()]), Ok(Some(1)));
        let silent = (shard_log(log, 4), (Ok(Some(0)), "GATEUNIT ok test p4\n".to_string()));
        assert_eq!(worst(&[ok(1), silent]), Ok(None), "a shard that exited 0 without its NODEGATE-OK is no pass");
        let lost_one = (shard_log(log, 5), (Ok(Some(crate::lease::client::LOST_EXIT)), String::new()));
        assert_eq!(worst(&[failed.clone(), lost_one]), Ok(Some(crate::lease::client::LOST_EXIT)));
        let error = (shard_log(log, 6), (Err("no lease".to_string()), String::new()));
        assert_eq!(worst(&[ok(1), failed, error]), Err("no lease".to_string()));
        let unneeded = (shard_log(log, 7), (Ok(Some(0)), format!("L9 test: 1st in queue (a: busy)\n{NOT_NEEDED}\n")));
        assert_eq!(worst(&[ok(1), unneeded]), Ok(Some(0)), "a shard the queue no longer needed passes");
        let outcome = verdict(worst(&[ok(1), ok(2)]), &merged(&[], (2, 16), &[ok(1), ok(2)]), log);
        assert!(outcome.ok && outcome.failing.is_empty());
        // the libraries' sweep logs beside them, no run of its own either, and a library it failed
        // fails the run by name; it runs on a full run, a JIT change and a range git cannot read
        assert_eq!(libs_log(log), Path::new("/n/home/bucket-logs/tests-libs-1794010109906.log"));
        assert_eq!(log_started(&libs_log(log)), Some(1794010109906), "its lease_run reads its start");
        let swept = (libs_log(log), (Ok(Some(1)), "GATEUNIT ok check makepad-draw\nGATEUNIT fail check makepad-csg-sdf\nNODEGATE-FAIL: makepad-csg-sdf\n".to_string()));
        let outcome = verdict(worst(&[ok(1), swept.clone()]), &merged(&[], (1, 16), &[ok(1), swept]), log);
        assert!(!outcome.ok && outcome.failing == ["check makepad-csg-sdf"], "{:?}", outcome.failing);
        let gate = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/run/fixture/land_gate.sh")).unwrap();
        let has = Some(gate.as_str());
        assert!(sweeps(true, Some(false), has) && sweeps(false, Some(true), has) && sweeps(false, None, has), "a full run, a JIT change, an unread range");
        assert!(!sweeps(false, Some(false), has), "a tests run whose landings left the JIT alone");
        // a run of a tree whose gate has no sweep runs none: that gate fails it `bad-mode`
        let older = gate.replace(LIBS_MODE, "");
        assert!(!sweeps(true, Some(true), Some(&older)) && !sweeps(true, Some(true), None), "a gate from before the sweep, or one git cannot read");
    }

    /// A run a restarted master adopts gives its verdict whoever reaped its holder: the holder's own
    /// exit (`lease --exit-to`), written as it ended, before the restart (the old image reaped it)
    /// or after it (a holder this process never started); a shard the old master stopped as not
    /// needed passes; one with neither ended by a signal, its exit not known (16:51Z, 11-08).
    #[cfg(unix)]
    #[test]
    fn an_adopted_run_takes_its_holders_own_exit_whoever_reaped_it() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-adopt-exit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"f".repeat(40)));
        let log = dir.join("tests-s1-5.log");
        let exit = log.with_extension("log.exit");
        let adopt = |pid: u32, text: &str| {
            std::fs::write(&log, text).unwrap();
            save_run(&log, pid, 5, &tip, &tip).unwrap();
            lease_run(&dir, &[], &[], &[], &log, (&tip, &tip), &|| true, &mut |_| true, &|| false, &|| false).0
        };
        // ended before the restart: the old master reaped it; nobody can now
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        let gone = ended.id();
        ended.wait().unwrap();
        std::fs::write(&exit, format!("{gone} 3\n")).unwrap();
        assert_eq!(adopt(gone, "L1 test@a: granted\n"), Ok(Some(3)), "its own exit, as it ended");
        std::fs::write(&exit, format!("{} 3\n", gone + 1)).unwrap();
        // (with its verdict said: one without is a lost lease's and runs again, [`adopted_end`])
        assert_eq!(adopt(gone, "L1 test@a: granted\nNODEGATE-FAIL: x\n"), Ok(None), "another holder's exit is not its own: not known");
        std::fs::remove_file(&exit).unwrap();
        assert_eq!(adopt(gone, &format!("L1 test: 1st in queue (a: busy)\n{NOT_NEEDED}\n")), Ok(Some(0)), "stopped as not needed before the restart");
        // its pid another process's since (a restarted bisect adopts probes that ended hours
        // before): its exit written is its end
        let alive = std::process::id();
        std::fs::write(&exit, format!("{alive} 5\n")).unwrap();
        assert_eq!(adopt(alive, "L3 test@a: granted\n"), Ok(Some(5)), "its own exit file, though its pid runs on");
        // runs on after the restart and is not this process's child (a crash's or a switch-over's
        // master started it): its file once it ends
        let script = format!("sleep 1; echo \"$$ 7\" > '{0}.tmp' && mv '{0}.tmp' '{0}'", exit.display());
        let out = std::process::Command::new("sh").args(["-c", &format!("sh -c '{}' >/dev/null 2>&1 </dev/null & echo $!", script.replace('\'', "'\\''"))]).output().unwrap();
        let orphan: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(adopt(orphan, "L2 test@a: granted\n"), Ok(Some(7)), "a holder it did not start: its exit when it ends");
        assert_eq!(crate::lease::client::read_exit(&exit, orphan), Some(7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restarted master deals a run it adopts as the run was dealt (its log's SHARDS line), not
    /// over the boxes it knows just after its start; a log without one is no shape.
    #[test]
    fn an_adopted_tests_run_keeps_its_shards() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-shape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("tests-1794010109906.log");
        std::fs::write(&log, merged(&["why".to_string()], (4, 16), &[])).unwrap();
        assert_eq!(run_shape(&log), Some((4, 16)));
        std::fs::write(&log, "L1: granted\n").unwrap();
        assert_eq!(run_shape(&log), None);
        std::fs::write(&log, "SHARDS 0 16\n").unwrap();
        assert_eq!(run_shape(&log), None, "no shards is no run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `devhub bucket run` starts a bucket's run at its next look (its kick file, which the run
    /// takes); an unknown bucket is said. The full bucket's measures deal the tests bucket's next
    /// run as its own do, newest first; another bucket's do not.
    #[test]
    fn a_bucket_runs_on_demand_and_the_full_buckets_measures_deal_the_tests_runs() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-kick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kick(&dir, FULL).starts_with(r#"{"said":"bucket full: its run starts"#), "{}", kick(&dir, FULL));
        assert!(kick_file(&dir, FULL).exists());
        assert_eq!(kick(&dir, "nope"), r#"{"error":"no bucket `nope`: fast, tests, hot, mac, ci, full"}"#);
        assert!(!kick_file(&dir, "nope").exists());
        let row = |bucket: &str, units: &[(&str, i64)]| obj(vec![("bucket", s(bucket)), ("unit_s", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        for r in [row("tests", &[("test a", 10)]), row(FULL, &[("test a", 20), ("test b", 5)]), row("fast", &[("test c", 1)])] {
            append(&dir, r).unwrap();
        }
        assert_eq!(times_env(&test_rows(&dir)), "GATE_TIMES=test a=20;test b=5", "the full run is the newest measure of `test a`");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A full run's shard whose lease is not granted yet gives its slot up at once to a tests run's
    /// waiting shard ([`Board::give_queued`]), one slot per waiter: the next full shard keeps its
    /// own, a tests shard never gives, and none gives while nothing waits.
    #[test]
    fn a_full_shard_not_granted_yet_gives_its_slot_to_a_waiting_tests_shard() {
        fn now() -> u64 { 1_794_000_000_000 }
        fn pool() -> Result<usize, String> { Ok(2) }
        let home = std::env::temp_dir().join(format!("devhub-bucket-give-queued-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bucket-logs")).unwrap();
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"f".repeat(40)));
        let (full, tests) = (home.join("bucket-logs").join(format!("{FULL}-{}.log", now() - 60_000)), home.join("bucket-logs").join(format!("tests-{}.log", now())));
        for log in [&full, &tests] { save_run(log, 1, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        let say = |_: String| {};
        let board = Board::new(now, pool);
        let (_on_full, _on_tests) = (board.enter(&home, &full, "tipA", &say), board.enter(&home, &tests, "tipB", &say));
        let _held: Vec<Slot> = (1..=2).map(|i| board.try_take(&home, &full, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        assert!(!board.give_queued(&full, 1, &say), "nothing waits: it keeps its slot");
        assert!(board.try_take(&home, &tests, 1, Need::Lease, &say).is_err(), "the tests run's shard waits");
        assert!(board.give_queued(&full, 1, &say), "its lease waits for a box: it gives its slot up");
        assert!(!board.give_queued(&full, 2, &say), "the slot it let go is the waiter's: the next full shard keeps its own");
        assert!(!board.give_queued(&tests, 1, &say), "a tests run's shard never gives");
        let taken = board.try_take(&home, &tests, 1, Need::Lease, &say).unwrap();
        assert!(taken.is_some(), "the tests run's shard takes it");
        let why = board.try_take(&home, &tests, 2, Need::Lease, &say).err().expect("its second shard waits");
        assert!(why.ends_with("a full run keeps one slot, its floor, until its run ends"), "{why}");
        assert!(!board.give_queued(&full, 2, &say), "a tests shard waits, but the full run's last slot is its floor");
        // a lease its ask heard granted reads as granted whether or not a poll saw it start: one set
        // up between two polls said no grant, and its shard ended its running gate as an ask not
        // granted yet, 32 units in (L1561926, 11-22); one said as it started is not said again
        use crate::lease::{client::grant_line, Kind};
        assert_eq!(grant_line("L9", Kind::Test, "box-a", "L9 test: 1st in queue (box-a: busy)", "").as_deref().and_then(granted), Some("L9"));
        assert_eq!(grant_line("L9", Kind::Test, "box-a", "L9 test@box-a: granted, setting up", ""), None);
        assert_eq!(grant_line("L9", Kind::Test, "box-a", "L9 test@box-a: granted, setting up", "; class x").as_deref(), Some("L9 test@box-a: granted; class x"), "a class note is said as before");
        // (its slots let go before their files' dir goes)
        drop((taken, _held, _on_full, _on_tests));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A full run that holds no slot takes its floor in its ask's order ([`Slots::floor`]): its
    /// lowest waiting shard before a tests run asked after it, its others after that run's shards,
    /// and its one slot is never given up. With every other run first, a full run asked at 17:00
    /// waited 80 min behind tests runs asked back to back (11-19).
    #[test]
    fn a_full_run_takes_its_floor_slot_in_its_asks_order() {
        fn now() -> u64 { 1_794_000_000_000 }
        fn pool() -> Result<usize, String> { Ok(2) }
        let home = std::env::temp_dir().join(format!("devhub-bucket-floor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bucket-logs")).unwrap();
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"f".repeat(40)));
        let named = |bucket: &str, ms: u64| home.join("bucket-logs").join(format!("{bucket}-{ms}.log"));
        let (before, full, after) = (named("tests", now() - 120_000), named(FULL, now() - 60_000), named("tests", now()));
        for log in [&before, &full, &after] { save_run(log, 1, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        let say = |_: String| {};
        let board = Board::new(now, pool);
        let _on: Vec<OnBoard> = [(&before, "tipA"), (&full, "tipB"), (&after, "tipC")].into_iter().map(|(log, tip)| board.enter(&home, log, tip, &say)).collect();
        let mut first: Vec<Slot> = (1..=2).map(|i| board.try_take(&home, &before, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        for (log, shard) in [(&full, 1), (&full, 2), (&after, 1), (&after, 2)] {
            assert!(board.try_take(&home, log, shard, Need::Lease, &say).is_err(), "{} {shard} waits", log.display());
        }
        drop(first.remove(0));
        let why = board.try_take(&home, &after, 1, Need::Lease, &say).err().expect("the full run's floor goes first");
        assert!(why.ends_with("1 shard asked before it goes first"), "{why}");
        let floor = board.try_take(&home, &full, 1, Need::Lease, &say).unwrap().expect("its floor, asked before the later tests run");
        drop(first);
        let why = board.try_take(&home, &full, 2, Need::Lease, &say).err().expect("past its floor, the full run goes after");
        assert!(why.ends_with("2 shards go first (2 of a post-land tests run, which goes before a full run)"), "{why}");
        let later = board.try_take(&home, &after, 1, Need::Lease, &say).unwrap().expect("the tests run's shard");
        assert_eq!(board.short_before_full(), 1, "the tests run's second shard waits");
        assert!(!board.give_queued(&full, 1, &say), "the full run's one slot is its floor");
        drop((floor, later, _on));

        // A full run one of whose shards waited the tests bucket's measured run behind tests runs
        // keeps half the pool from then on: its next shard takes the next free slot before a tests
        // run asked after it, and it gives none below that; past it, the tests run goes first. A
        // full run asked at 11:57 ran on one slot of four for an hour while tests runs asked back
        // to back took the other three within seconds each (11-22).
        static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_794_000_000_000);
        fn clock() -> u64 { CLOCK.load(Ordering::Relaxed) }
        fn four() -> Result<usize, String> { Ok(4) }
        const RUN_MS: u64 = 25 * 60_000;
        fn measured() -> Option<u64> { Some(RUN_MS) }
        let board = Board::new(clock, four).starving_after(measured);
        let said = std::cell::RefCell::new(Vec::new());
        let say = |line: String| said.borrow_mut().push(line);
        let (full, tests) = (named(FULL, clock() - 60_000), named("tests", clock()));
        for log in [&full, &tests] { save_run(log, 4, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        let _on: Vec<OnBoard> = [(&full, "tipF"), (&tests, "tipT")].into_iter().map(|(log, tip)| board.enter(&home, log, tip, &say)).collect();
        let mut theirs: Vec<Slot> = (1..=3).map(|i| board.try_take(&home, &tests, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        let first = board.try_take(&home, &full, 1, Need::Lease, &say).unwrap().expect("the full run's floor");
        for (log, shard) in [(&full, 2), (&full, 3), (&tests, 4)] {
            assert!(board.try_take(&home, log, shard, Need::Lease, &say).is_err(), "{} {shard} waits", log.display());
        }
        assert_eq!(board.floor_n(&full), 1, "one slot until a shard of it waited the tests bucket's run");
        CLOCK.fetch_add(RUN_MS, Ordering::Relaxed);
        assert!(board.try_take(&home, &full, 2, Need::Lease, &say).is_err(), "no slot is free yet");
        assert_eq!(board.floor_n(&full), 2, "half the pool once its shard waited the tests bucket's run");
        assert!(said.borrow().iter().any(|l| l.contains("waited the tests bucket's measured run (25m00s) behind post-land tests runs: the run keeps 2 of the pool's 4 slots from now on")), "{:?}", said.borrow());
        drop(theirs.remove(0));
        let why = board.try_take(&home, &tests, 4, Need::Lease, &say).err().expect("the full run's floor goes first");
        assert!(why.ends_with("1 shard asked before it goes first; a full run keeps half the pool, its floor, until its run ends"), "{why}");
        let second = board.try_take(&home, &full, 2, Need::Lease, &say).unwrap().expect("its second floor slot, before the tests run asked after it");
        assert!(!board.give_queued(&full, 2, &say), "it gives none below its floor of two");
        let why = board.try_take(&home, &full, 3, Need::Lease, &say).err().expect("past its floor it goes after");
        assert!(why.contains("past its run's floor of 2 slots (a shard of its run waited the tests bucket's measured run, 25m00s)"), "{why}");
        drop(theirs.remove(0));
        let fourth = board.try_take(&home, &tests, 4, Need::Lease, &say).unwrap().expect("past the full run's floor, the tests run goes first");
        drop((first, second, fourth, theirs, _on));

        // A full shard that takes a slot as one frees between two tests runs and gives it to the
        // next one's shard before its lease is granted waits on from its ask: given up between
        // tests runs, each piece of its wait began again and none reached the measured run (still
        // one slot of four at 13:58, asked 11:57, 11-22). Its shard 2 ran a part before (its log
        // names a grant), so its wait is the board's to keep. After a master restart a shard whose
        // log names no grant has waited since its run was asked.
        let board = Board::new(clock, four).starving_after(measured);
        let (full, tests) = (named(FULL, clock() - 30_000), named("tests", clock() - 20_000));
        for log in [&full, &tests] { save_run(log, 4, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        std::fs::write(shard_log(&full, 2), "L7 test@box-a: granted, setting up\nbucket: this shard gave its pool slot to a run that goes first as its units ended; its gate runs again on the next slot\n").unwrap();
        let _on: Vec<OnBoard> = [(&full, "tipG"), (&tests, "tipU")].into_iter().map(|(log, tip)| board.enter(&home, log, tip, &say)).collect();
        let mut theirs: Vec<Slot> = (1..=3).map(|i| board.try_take(&home, &tests, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        let first = board.try_take(&home, &full, 1, Need::Lease, &say).unwrap().expect("the full run's floor");
        assert!(board.try_take(&home, &full, 2, Need::Lease, &say).is_err(), "its second shard waits");
        CLOCK.fetch_add(RUN_MS / 2, Ordering::Relaxed);
        drop(theirs.remove(0));
        let taken = board.try_take(&home, &full, 2, Need::Lease, &say).unwrap().expect("a slot freed between two tests runs");
        let next = named("tests", clock());
        save_run(&next, 4, log_started(&next).unwrap(), &tip, &tip).unwrap();
        let on_next = board.enter(&home, &next, "tipV", &say);
        assert!(board.try_take(&home, &next, 1, Need::Lease, &say).is_err(), "the next tests run's shard waits");
        assert!(board.give_queued(&full, 2, &say), "its lease waits for a box: it gives its slot up");
        drop(taken);
        let given = board.try_take(&home, &next, 1, Need::Lease, &say).unwrap().expect("the slot it gave up");
        // The master restarts here (a new board over the same files, the old one gone without a
        // word): shard 2's wait comes back from the slots file and goes on, and once its run's floor
        // rose, the next restart brings that back with the run.
        let restarted = Board::new(clock, four).starving_after(measured);
        let on_restarted: Vec<OnBoard> = [(&full, "tipG"), (&tests, "tipU"), (&next, "tipV")].into_iter().map(|(log, tip)| restarted.enter(&home, log, tip, &say)).collect();
        assert_eq!(restarted.floor_n(&full), 1, "no shard of it has waited the measured run yet");
        CLOCK.fetch_add(RUN_MS / 2, Ordering::Relaxed);
        drop(restarted.try_take(&home, &full, 2, Need::Lease, &say));
        assert_eq!(restarted.floor_n(&full), 2, "its wait went on through the slot it gave up and the master's restart: half the pool");
        let again = Board::new(clock, four).starving_after(measured);
        let on_again = again.enter(&home, &full, "tipG", &say);
        // (shard 2's wait ended as its holder let its slot go: its next one begins now)
        drop(again.try_take(&home, &full, 2, Need::Lease, &say));
        assert_eq!(again.floor_n(&full), 2, "a raised floor comes back with its run after a restart");
        drop((on_again, on_restarted, first, theirs, given, on_next, _on));
        let board = Board::new(clock, four).starving_after(measured);
        let _on: Vec<OnBoard> = [(&full, "tipG"), (&tests, "tipU")].into_iter().map(|(log, tip)| board.enter(&home, log, tip, &say)).collect();
        let held: Vec<Slot> = (1..=4).map(|i| board.try_take(&home, &tests, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        assert!(board.try_take(&home, &full, 3, Need::Lease, &say).is_err(), "no slot is free");
        assert_eq!(board.floor_n(&full), 2, "after a restart its shard, never granted, has waited since its run's ask");
        // The master restarts before any box said it is up: the pool's count comes back with the
        // slots, and the raised floor holds; with none, the floor fell to one and no shard took a
        // slot ("4 of ? pool slots held" after each restart, 11-22).
        fn unknown() -> Result<usize, String> { Err("no up pool box takes arch=x64,os=linux leases".into()) }
        let lost = Board::new(clock, unknown).starving_after(measured);
        let on_lost: Vec<OnBoard> = [(&full, "tipG"), (&tests, "tipU")].into_iter().map(|(log, tip)| lost.enter(&home, log, tip, &say)).collect();
        let why = lost.try_take(&home, &full, 3, Need::Lease, &say).err().expect("no slot is free");
        assert!(why.starts_with("4 of 4 pool slots held"), "the count on file: {why}");
        assert_eq!(lost.floor_n(&full), 2, "its floor stays half the pool by the count on file");
        drop((on_lost, held, _on));

        // A full run's shards end and its run deals what they left without verdicts to one more
        // shard a moment later: the slot of its last shard to end stays the run's for that shard,
        // and frees once the run has nothing to deal again or leaves the board; one that ends while
        // another of its shards runs frees at once. Two tests shards that waited took the run's slots
        // in that moment, and its next shard waited behind four tests slots (14:47, 11-22).
        let board = Board::new(clock, four).starving_after(measured);
        let (full, tests) = (named(FULL, clock() - 10_000), named("tests", clock() - 5_000));
        for log in [&full, &tests] { save_run(log, 4, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        let (on_full, on_tests) = (board.enter(&home, &full, "tipH", &say), board.enter(&home, &tests, "tipW", &say));
        let mut slots: Vec<Slot> = (1..=2).map(|i| board.try_take(&home, &tests, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        let [one, two] = [1, 2].map(|i| board.try_take(&home, &full, i, Need::Lease, &say).unwrap().expect("a free slot"));
        assert!(board.try_take(&home, &tests, 3, Need::Lease, &say).is_err(), "the pool is full");
        one.ended();
        slots.push(board.try_take(&home, &tests, 3, Need::Lease, &say).unwrap().expect("a shard that ends while another of its run runs frees its slot"));
        assert!(board.try_take(&home, &tests, 4, Need::Lease, &say).is_err(), "the pool is full");
        two.ended();
        let why = board.try_take(&home, &tests, 4, Need::Lease, &say).err().expect("its last shard's slot stays the run's");
        assert!(why.starts_with("4 of 4 pool slots held") && why.contains("the full run of tipH holds 1"), "{why}");
        let on_file = std::fs::read_to_string(home.join("bucket-logs").join(SLOTS_FILE)).unwrap_or_default();
        assert!(on_file.lines().any(|l| Claim::parse(l).is_some_and(|c| c.of(&full, RUN_KEPT))), "on file for a restart: {on_file}");
        let rerun = board.try_take(&home, &full, 5, Need::Lease, &say).unwrap().expect("its next shard takes it at once");
        drop(rerun);
        slots.push(board.try_take(&home, &tests, 4, Need::Lease, &say).unwrap().expect("the run's last shard's own end frees it"));
        // the run asks once more, and its shard that ends leaves it the slot; the run finds nothing
        // to deal again, then leaves the board
        drop(slots.remove(0));
        board.try_take(&home, &full, 6, Need::Lease, &say).unwrap().expect("a free slot").ended();
        assert!(board.try_take(&home, &tests, 5, Need::Lease, &say).is_err(), "kept for the run");
        let restarted = Board::new(clock, four).starving_after(measured);
        let on_restarted = restarted.enter(&home, &full, "tipH", &say);
        restarted.let_go_kept(&full);
        let on_file = std::fs::read_to_string(home.join("bucket-logs").join(SLOTS_FILE)).unwrap_or_default();
        assert!(!on_file.lines().any(|l| Claim::parse(l).is_some_and(|c| c.of(&full, RUN_KEPT))), "a restarted master lets it go too: {on_file}");
        drop(on_restarted);
        board.let_go_kept(&full);
        slots.push(board.try_take(&home, &tests, 5, Need::Lease, &say).unwrap().expect("free once the run has nothing to deal"));
        drop(slots.remove(0));
        board.try_take(&home, &full, 7, Need::Lease, &say).unwrap().expect("a free slot").ended();
        assert!(board.try_take(&home, &tests, 6, Need::Lease, &say).is_err(), "kept for the run");
        drop(on_full);
        slots.push(board.try_take(&home, &tests, 6, Need::Lease, &say).unwrap().expect("free once the run left the board"));
        drop((slots, on_tests));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The pool's slots pass shard by shard: a run's shard takes the first that frees, a waiting
    /// one says why, `devhub bucket` shows each run on how many since its first, and a kick's answer
    /// names who holds them. After a master restart every slot goes back to its shard, whose lease
    /// ran on, with its times (each restart reset the running run's start, 17:29 to 19:30, 11-12),
    /// before a run that only waited; a shard asked earlier still goes first. A slot of a run that
    /// is over is free at once, one not taken up within its window after it, and a shard that ran
    /// on with no slot on file takes one beyond the pool's count, each said with its run and shard.
    /// The pool's count is read again a minute on. One clock, the test's; no waits.
    #[test]
    fn the_pool_slots_pass_shard_by_shard_and_a_restart_keeps_every_holder() {
        use std::sync::atomic::{AtomicU64, AtomicUsize};
        static NOW: AtomicU64 = AtomicU64::new(0);
        static POOL: AtomicUsize = AtomicUsize::new(3);
        fn now() -> u64 { NOW.load(Ordering::Relaxed) }
        fn pool() -> Result<usize, String> { Ok(POOL.load(Ordering::Relaxed)) }
        let on = |minutes: u64| NOW.fetch_add(minutes * 60_000, Ordering::Relaxed) + minutes * 60_000;
        let t0 = 1_794_000_000_000;
        // both from the start: a rerun in the same warm test process found the pool the last run
        // left (one slot, 11-13)
        NOW.store(t0, Ordering::Relaxed);
        POOL.store(3, Ordering::Relaxed);
        let home = std::env::temp_dir().join(format!("devhub-bucket-slots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let logs = home.join("bucket-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"f".repeat(40)));
        let said = std::cell::RefCell::new(Vec::<String>::new());
        let say = |l: String| said.borrow_mut().push(l);
        let heard = |what: &str| said.borrow().iter().any(|l| l.contains(what));
        let lines = |board: &Board| board.now_lines(&home, now());
        let has = |board: &Board, what: &str| lines(board).iter().any(|l| l.contains(what));
        let file_lines = || { let mut l: Vec<String> = std::fs::read_to_string(logs.join(SLOTS_FILE)).unwrap_or_default().lines().map(str::to_string).collect(); l.sort(); l };
        // (the held slots; a full shard's wait is on file beside them, [`Slots::waits`])
        let on_file = || file_lines().into_iter().filter(|l| Claim::parse(l).is_some()).collect::<Vec<_>>();
        let named = |bucket: &str, ms: u64| logs.join(format!("{bucket}-{ms}.log"));
        let (full, tests, next) = (named(FULL, t0 - 30_000), named("tests", t0 + 30_000), named(FULL, t0 + 5 * 60_000));
        for log in [&full, &tests, &next] { save_run(log, 1, log_started(log).unwrap(), &tip, &tip).unwrap(); }
        std::fs::write(logs.join(TURN_FILE), full.display().to_string()).unwrap();

        // a full run takes the three slots; the tests run's first shard waits and says why
        let board = Board::new(now, pool);
        let on_full = board.enter(&home, &full, "tipA", &say);
        let mut a: Vec<Slot> = (1..=3).map(|i| board.try_take(&home, &full, i, Need::Lease, &say).unwrap().expect("a free slot")).collect();
        assert_eq!(on_file().len(), 3, "each held slot is on disk before its lease is asked");
        assert_eq!(file_lines().iter().filter(|l| l.starts_with(&format!("wait\t{}\t", full.display()))).count(), 3, "each full shard's wait is on disk while its slot waits for its lease: {:?}", file_lines());
        assert!(!logs.join(TURN_FILE).exists(), "the whole pool's turn file goes");
        let on_tests = board.enter(&home, &tests, "tipB", &say);
        on(1);
        let why = board.try_take(&home, &tests, 1, Need::Lease, &say).err().expect("no slot is free");
        assert!(why.starts_with(&format!("3 of 3 pool slots held: the full run of tipA holds 3 since {}", clock(t0))), "{why}");
        assert!(why.ends_with("a full run's shard gives its slot up as its next unit ends"), "{why}");
        assert_eq!(board.short_before_full(), 1, "the full run's dealer gives one slot up");
        assert!(has(&board, &format!("full: running since {} (60 s); its run of tipA on 3 of 3 pool slots; log {}", clock(t0), full.display())), "{:?}", lines(&board));
        // a run that writes its log as it ends says how far it is (a CI run read as wedged, 11-22)
        board.doing(CI, Some(("running", t0, "its run of tipC; log ci-1.log".into())));
        board.progress.lock().unwrap().insert(CI.into(), || Some("53 of 77 shards ended".into()));
        assert!(has(&board, &format!("ci: running since {} (60 s); its run of tipC; log ci-1.log; 53 of 77 shards ended", clock(t0))), "{:?}", lines(&board));
        board.progress.lock().unwrap().insert(CI.into(), || None);
        assert!(has(&board, "its run of tipC; log ci-1.log") && !has(&board, "ci-1.log;"), "nothing going: nothing more: {:?}", lines(&board));
        board.doing(CI, None);
        // its output while it runs: the shards' own logs, which the run's log gathers at its end
        std::fs::write(&full, format!("{SHARDS}3 12\n")).unwrap();
        std::fs::write(shard_log(&full, 1), "GATESTART test a 1\n").unwrap();
        assert!(has(&board, &format!("pool slots; logs {} (all in {} once it ends)", shard_log(&full, 1).display(), full.display())), "{:?}", lines(&board));
        for f in [full.clone(), shard_log(&full, 1)] { std::fs::remove_file(f).unwrap(); }
        assert!(has(&board, "tests: queued since") && has(&board, "its run of tipB waits for the pool slots: 3 of 3 pool slots held"), "{:?}", lines(&board));
        let kicked = kick_on(&board, &home, "tests", now());
        assert!(kicked.contains(&format!("the pool slots: the full run of tipA holds 3 since {}", clock(t0))), "{kicked}");
        std::fs::remove_file(kick_file(&home, "tests")).unwrap();

        // the full run's fourth shard waits too, its run asked before the tests run
        let why = board.try_take(&home, &full, 4, Need::Lease, &say).err().expect("no slot is free");
        assert!(why.ends_with("1 shard goes first (1 of a post-land tests run, which goes before a full run)"), "{why}");
        assert_eq!(board.short_before_full(), 1, "a full run's own waiting shard asks it for nothing");

        // the full run's second shard ends: the slot is the tests run's, not the full run's next
        // shard's (a full run yields at its next shard's end)
        let t1 = on(1);
        drop(a.remove(1));
        assert_eq!(board.short_before_full(), 0, "a slot let go is free at once, before its waiter takes it");
        assert!(board.try_take(&home, &full, 4, Need::Lease, &say).is_err(), "the full run's next shard waits for the tests run's");
        let b1 = board.try_take(&home, &tests, 1, Need::Lease, &say).unwrap().expect("the freed slot");
        assert!(board.try_take(&home, &tests, 2, Need::Lease, &say).is_err(), "no other slot is free");
        assert_eq!(board.short_before_full(), 1, "the tests run's second shard waits");
        assert!(has(&board, &format!("tests: running since {}", clock(t1))) && has(&board, "its run of tipB on 1 of 3 pool slots, 1 shard waits for one"), "{:?}", lines(&board));
        assert!(has(&board, "its run of tipA on 2 of 3 pool slots"), "{:?}", lines(&board));

        // the master restarts: its slots go with its process, on disk
        let before = on_file();
        std::mem::forget((a, b1, on_full, on_tests));
        on(10);
        let board = Board::new(now, pool);
        let _next = board.enter(&home, &next, "tipC", &say);
        let why = board.try_take(&home, &next, 1, Need::Lease, &say).err().expect("every slot is kept for its shard");
        assert!(why.contains("the full run of tipA held 2 before the master restart") && why.contains("the tests run of tipB held 1 before the master restart"), "{why}");
        // its buckets adopt both runs: each shard whose lease runs on takes its slot back at once
        let (_full, _tests) = (board.enter(&home, &full, "tipA", &say), board.enter(&home, &tests, "tipB", &say));
        let a1 = board.try_take(&home, &full, 1, Need::Running(Some(t0)), &say).unwrap().expect("its slot back");
        let a3 = board.try_take(&home, &full, 3, Need::Running(Some(t0)), &say).unwrap().expect("its slot back");
        let b1 = board.try_take(&home, &tests, 1, Need::Running(Some(t1)), &say).unwrap().expect("its slot back");
        assert_eq!(on_file(), before, "every slot keeps its holder and its times through the restart");
        assert!(has(&board, &format!("full: running since {} (12 min); its run of tipA on 2 of 3", clock(t0))), "the restart does not move a run's start: {:?}", lines(&board));
        assert!(has(&board, &format!("tests: running since {} (10 min, asked {}); its run of tipB", clock(t1), clock(t0 + 30_000))), "the run's ask beside its start: {:?}", lines(&board));
        assert!(!heard("beyond the"), "a slot given back is within the count: {:?}", said.borrow());
        assert!(board.try_take(&home, &full, 2, Need::Ended, &say).unwrap().is_none(), "a shard that ended before the restart needs none");
        // the tests run's second shard was asked before the next run: it goes first as a slot frees
        assert!(board.try_take(&home, &tests, 2, Need::Lease, &say).is_err());
        drop(a3);
        let why = board.try_take(&home, &next, 1, Need::Lease, &say).err().expect("a shard asked earlier goes first");
        assert!(why.ends_with("1 shard asked before it goes first"), "{why}");
        let b2 = board.try_take(&home, &tests, 2, Need::Lease, &say).unwrap().expect("its turn");

        // the pool's count is read again a minute on, not before
        POOL.store(2, Ordering::Relaxed);
        on(1);
        drop(a1);
        let why = board.try_take(&home, &next, 1, Need::Lease, &say).err().expect("the pool takes two now");
        assert!(why.starts_with("2 of 2 pool slots held"), "{why}");
        POOL.store(3, Ordering::Relaxed);
        assert!(board.try_take(&home, &next, 1, Need::Lease, &say).is_err(), "read a minute on");
        on(1);
        let c1 = board.try_take(&home, &next, 1, Need::Lease, &say).unwrap().expect("three again");
        // a shard that ran on through a restart with no slot on file takes one beyond the count
        let b3 = board.try_take(&home, &tests, 3, Need::Running(None), &say).unwrap().expect("never waits");
        let beyond = "shard 3 of its run of tipB takes a pool slot beyond the 3 the pool takes";
        assert!(heard(&format!("bucket tests: {beyond}")) && has(&board, &format!("tests: {beyond}")), "{:?} {:?}", said.borrow(), lines(&board));

        // another restart: the tests run is over (its sidecar retired), the next run's bucket never
        // takes its slot up again
        std::mem::forget((b1, b2, b3, c1, _full, _tests, _next));
        std::fs::remove_file(tests.with_extension("log.run")).unwrap();
        POOL.store(1, Ordering::Relaxed);
        on(5);
        let board = Board::new(now, pool);
        let later = named(FULL, t0 + 15 * 60_000);
        save_run(&later, 1, log_started(&later).unwrap(), &tip, &tip).unwrap();
        let _later = board.enter(&home, &later, "tipD", &say);
        assert!(heard(&format!("bucket tests: the pool slot of shard 2 of its run of tipB is free: the run is over (log {})", tests.display())), "{:?}", said.borrow());
        assert!(on_file().iter().all(|l| l.starts_with(&next.display().to_string())), "{:?}", on_file());
        let why = board.try_take(&home, &later, 1, Need::Lease, &say).err().expect("kept for its shard within the window");
        assert!(why.contains("the full run of tipC held 1 before the master restart"), "{why}");
        on(2);
        let d1 = board.try_take(&home, &later, 1, Need::Lease, &say).unwrap().expect("free after the window");
        let freed = format!("the pool slot of shard 1 of its run of tipC held before the master restart is free again: its bucket did not take it up within 2 min (log {})", next.display());
        assert!(heard(&format!("bucket full: {freed}")) && has(&board, &format!("full: {freed}")), "{:?} {:?}", said.borrow(), lines(&board));
        drop(d1);
        assert!(on_file().is_empty() && !logs.join(SLOTS_FILE).exists(), "no slot held, and no wait of a run that came back: no file: {:?}", file_lines());

        // a waiting run's sidecar is a run to adopt, newer than its bucket's last row; a shard's is not
        let ms = |log: &Path| log_started(log).unwrap();
        save_run(&logs.join(format!("full-s3-{}.log", ms(&later) + 1)), 1, ms(&later) + 1, &tip, &tip).unwrap();
        assert_eq!(unfinished(&logs, FULL, Some(ms(&next))), Some((ms(&later), later.with_extension("log.run"))));
        assert_eq!(unfinished(&logs, FULL, Some(ms(&later))), None, "a run with a row is no run to adopt");
        assert_eq!(unfinished(&logs, FULL, None).map(|(ms, _)| ms), Some(ms(&later)));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Each unit's seconds come from its run's GATETIME line, and the next run deals by the
    /// newest measure of each.
    #[test]
    fn the_tests_bucket_deals_by_each_units_last_measure() {
        let secs = unit_secs("GATETIME test a 12\ntest a: ok\nGATETIME test b 140\nGATETIME junk\n");
        assert_eq!(secs.into_iter().collect::<Vec<_>>(), [("test a".to_string(), 12), ("test b".to_string(), 140)]);
        let row = |units: &[(&str, i64)]| obj(vec![("unit_s", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        let rows = [row(&[("test a", 9)]), row(&[("test a", 12), ("test b", 140)]), obj(vec![])];
        assert_eq!(times_env(&rows), "GATE_TIMES=test a=9;test b=140", "the newest measure of each");
        assert_eq!(times_env(&[]), "GATE_TIMES=");
    }

    /// Each unit's peak RSS comes from its run's GATEPEAK line, and the next run's shards start
    /// the units they are dealt by the newest peak of each.
    #[test]
    fn the_tests_bucket_tells_its_shards_each_units_last_peak() {
        let mb = unit_mb("GATEPEAK test a --lib 2100\nGATETIME test a --lib 12\nGATEPEAK test b 30500\nGATEPEAK junk\nGATEPEAKS x 1\n");
        assert_eq!(mb.into_iter().collect::<Vec<_>>(), [("test a --lib".to_string(), 2100), ("test b".to_string(), 30500)]);
        let row = |units: &[(&str, i64)]| obj(vec![("unit_mb", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        let rows = [row(&[("test a --lib", 1800)]), row(&[("test a --lib", 2100), ("test b", 30500)])];
        assert_eq!(peaks_env(&rows), "GATE_PEAKS=test a --lib=1800;test b=30500", "the newest measure of each");
        assert_eq!(ram_floor(&rows).as_deref(), Some("29.8"), "a shard's floor: the largest unit's last peak, GB");
        assert_eq!(ram_floor(&[]), None, "none measured: no floor");
        assert_eq!(times_env(&rows), "GATE_TIMES=", "a peak is no time");
        let jobs = |units: &[(&str, i64)]| obj(vec![("unit_jobs", Value::Obj(units.iter().map(|(u, n)| (u.to_string(), Value::Int(*n))).collect()))]);
        assert_eq!(unit_jobs("GATEJOBS test b 12\nGATETIME test b 600\n").into_iter().collect::<Vec<_>>(), [("test b".to_string(), 12)]);
        assert_eq!(jobs_env(&[jobs(&[("test b", 12)]), jobs(&[("test a --lib", 4), ("test b", 4)])]), "GATE_JOBS=test a --lib=4;test b=12", "the threads of each unit's newest measure");
    }

    /// A part's tests' times and the tests at its highest look (land_gate.sh test_times) go into
    /// its run's row, and `bucket status` names its slowest part's slowest tests; a row with none
    /// says nothing.
    #[test]
    fn a_runs_row_keeps_its_parts_slowest_tests_and_those_at_its_peak() {
        let text = "GATETESTS test f --bin f part 1/2 :: print::slow=312.4/13100 xr::grip=120.0/9000 a::b=1.5/2000\nGATETESTS test g --lib part 1/2 :: g::t=2.0/100\nGATETESTS junk\nGATEPEAKTESTS test f --bin f part 1/2 :: 13700 812 print::slow xr::grip\nGATEPEAKTESTS test g --lib part 1/2 :: 300 4\n";
        let tests = unit_tests(text);
        assert_eq!(tests["test f --bin f part 1/2"], [("print::slow".to_string(), 312.4, 13100), ("xr::grip".to_string(), 120.0, 9000), ("a::b".to_string(), 1.5, 2000)]);
        let peaks = unit_peak_tests(text);
        assert_eq!(peaks["test f --bin f part 1/2"], PeakTests { mb: 13700, at_s: 812, tests: vec!["print::slow".to_string(), "xr::grip".to_string()] });
        assert!(peaks["test g --lib part 1/2"].tests.is_empty(), "no timed test ran at its highest look");
        let row = obj(vec![
            ("unit_s", obj(vec![("test f --bin f part 1/2", Value::Int(1045)), ("test g --lib part 1/2", Value::Int(20))])),
            ("unit_mb", obj(vec![("test f --bin f part 1/2", Value::Int(14005))])),
            ("unit_tests", Value::Obj(tests.iter().map(|(u, t)| (u.clone(), Value::Arr(t.iter().map(|(n, secs, mb)| Value::Arr(vec![s(n), Value::F64(*secs), Value::Int(*mb as i64)])).collect()))).collect())),
            ("unit_peak_tests", Value::Obj(peaks.iter().map(|(u, p)| (u.clone(), obj(vec![("mb", Value::Int(p.mb as i64)), ("at_s", Value::Int(p.at_s as i64)), ("tests", Value::Arr(p.tests.iter().map(s).collect()))]))).collect())),
        ]);
        let row = makepad_strict_json::parse(row.to_json().as_bytes()).unwrap();
        assert_eq!(slowest_tests(&row).as_deref(), Some("test f --bin f part 1/2 (1045 s, 13.7 GB): its slowest print::slow 312.4 s, xr::grip 120.0 s, a::b 1.5 s; at its highest look (13.4 GB) ran print::slow, xr::grip"));
        assert_eq!(slowest_tests(&obj(vec![("unit_s", obj(vec![("test x", Value::Int(9))]))])), None, "no timed tests: nothing said");
        // the next run deals each split target's parts by its tests' newest measures, by target
        let older = obj(vec![("unit_tests", obj(vec![("test f --bin f part 2/3", Value::Arr(vec![Value::Arr(vec![s("print::slow"), Value::F64(9.0), Value::Int(1)]), Value::Arr(vec![s("old::one"), Value::Int(4), Value::Int(1)])]))]))]);
        assert_eq!(test_times_env(&[row, older]), "GATE_TEST_TIMES=test f --bin f :: a::b=1.5 old::one=4.0 print::slow=312.4 xr::grip=120.0;test g --lib :: g::t=2.0");
        assert_eq!(test_times_env(&[]), "GATE_TEST_TIMES=");
        // A test past the 10 s line goes to the lane of the newest landing that touched its package,
        // with its time and the run's log, once a day; to main once that lane has ended (main,
        // 11-24). print::slow and xr::grip are; a::b and g::t are not.
        let home = std::env::temp_dir().join(format!("devhub-bucket-slow-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let landing = |id: u64, pkg: &str, changed_ms: u64| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), checked: String::new(), touched: vec![pkg.to_string()], wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0,
            landed: String::new(), pre: String::new(), detail: Vec::new(), agent: format!("a{id}"), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms };
        let history = [landing(1, "f", 10), landing(2, "f", 20), landing(3, "g", 30)];
        let tip = pin_of(&Pair::new(&"a".repeat(40), &"b".repeat(40)));
        let said = |live: &dyn Fn(&Entry) -> bool, now: u64| slow_notes(&home, "tests", &tip, "tests-1.log", &tests, &history, live, now).unwrap();
        let notes = said(&|_| true, 1_000);
        assert_eq!(notes.iter().map(|(agent, _)| agent.as_str()).collect::<Vec<_>>(), ["a2", "a2"], "print::slow and xr::grip, to the newest landing that touched f: {notes:?}");
        assert!(notes[0].1.contains("slow test print::slow of test f --bin f part 1/2: 312 s, past the 10 s line") && notes[0].1.contains("rc-l2 (a2) #2 last touched f; log tests-1.log"), "{notes:?}");
        assert!(said(&|_| true, 1_000 + 3_600_000).is_empty(), "once a day");
        let notes = said(&|e| e.id != 2, 1_000 + 24 * 3_600_000);
        assert!(notes.len() == 2 && notes.iter().all(|(agent, line)| agent == "main" && line.contains("its lane has ended")), "a day on, its lane ended: main hears it: {notes:?}");
        // main's weekly list: the slowest tests of the week's runs, each at its slowest, under the line too
        let ran = |started: u64, unit: &str, tests: &[(&str, f64)]| append(&home, obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(started as i64)),
            ("unit_tests", obj(vec![(unit, Value::Arr(tests.iter().map(|(n, secs)| Value::Arr(vec![s(*n), Value::F64(*secs), Value::Int(1)])).collect()))]))])).unwrap();
        let week = 7 * 24 * 3_600_000;
        ran(1, "test old", &[("old::t", 900.0)]);
        ran(week + 10, "test f --bin f part 1/2", &[("print::slow", 300.0), ("a::b", 1.5)]);
        ran(week + 20, "test f --bin f part 2/2", &[("print::slow", 312.4)]);
        ran(week + 30, "test g --lib", &[("g::t", 2.0)]);
        let list = weekly_slowest(&home, week + 40).unwrap().unwrap();
        assert_eq!(list, "the week's 3 slowest tests the buckets measured (each at its slowest; the line is 10 s): print::slow of test f 312.4 s (tests); g::t of test g 2.0 s (tests); a::b of test f 1.5 s (tests)");
        assert_eq!(weekly_slowest(&home, week + 50).unwrap(), None, "once a week");
        assert!(weekly_slowest(&home, 2 * week + 40).unwrap().is_none(), "a week on, no run in that week: nothing to say");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A unit that runs far over its measure is said while it runs: one still running past three
    /// times its measure and 10 min, not one that ended, one within its measure's reach, one
    /// measured at seconds and not yet running 10 min, or one never measured. On a box past twice
    /// its cores in load, one past five times its measure is said too, the box oversubscribed; on
    /// a box whose hub does not answer, the seconds are said as its silence; a shard's lease that
    /// ended took its units with it, and a shard that runs again on a new lease starts afresh.
    #[test]
    fn a_unit_running_far_over_its_measure_is_said_while_it_runs() {
        let measures: BTreeMap<String, u64> = [("test slow", 540), ("test done", 60), ("test near", 600), ("test quick", 5)].iter().map(|(u, s)| (u.to_string(), *s)).collect();
        let now = 10_000;
        let shard = "testrun: 4 packages\nGATEQUEUE L7 5 0123456789ab\nGATESTART test slow 5000\nGATESTART test done 5000\nGATEEND test done\nGATESTART test near 9000\nGATESTART test quick 9500\nGATESTART test new 1000\nGATESTART junk\n";
        let (lease, running, busy) = running_in(shard);
        assert_eq!(lease, Some("L7"));
        assert!(busy.is_empty());
        assert_eq!(slow_in(&running, &measures, now, false), [("test slow".to_string(), 5000, 540)]);
        assert_eq!(slow_in(&running, &measures, 5100, false), [], "not yet 10 min");
        assert_eq!(slow_in(&running, &measures, now, true), [("test quick".to_string(), 500, 5), ("test slow".to_string(), 5000, 540)], "oversubscribed: past five times its measure");
        let again = format!("{shard}LEASE LOST on box-a (L7): link lost\nGATEQUEUE L8 5 0123456789ab\nGATESTART test quick 9900\n");
        assert_eq!(running_in(&again), (Some("L8"), [("test quick", 9900)].into_iter().collect(), BTreeMap::new()), "the lost lease's units ended with it");
        let dir = std::env::temp_dir().join(format!("devhub-slow-units-{}", std::process::id()));
        let logs = dir.join("bucket-logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("tests-1794013801204.log.run"), "{}").unwrap();
        std::fs::write(logs.join("tests-s1-1794013801204.log"), shard).unwrap();
        let row = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(1)), ("unit_s", Value::Obj(measures.iter().map(|(u, n)| (u.clone(), Value::Int(*n as i64))).collect()))]);
        std::fs::write(dir.join("buckets.jsonl"), format!("{}\n", row.to_json())).unwrap();
        let at = |up: bool, load: f64| move |lease: &str| (lease == "L7").then(|| ShardBox { node: "box-a".into(), up, load, cores: 16 });
        let said = slow_units(&dir, now * 1000, &at(true, 12.0));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("tests run 1794013801204 shard 1 (L7 on box-a): `test slow` running 83 min against its measured 9 min (9x); its watch fails it HUNG"), "{said:?}");
        assert!(said[0].contains("`devhub top box-a`"), "{said:?}");
        let said = slow_units(&dir, now * 1000, &at(true, 40.0));
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("tests run 1794013801204 shard 1 (L7 on box-a): `test quick` running 8 min against its measured 5 s (100x); box-a is oversubscribed: load 40 on 16 cores"), "{said:?}");
        // past its cores is oversubscribed, not only past twice them: the box shows beside the unit
        let said = slow_units(&dir, now * 1000, &at(true, 20.0));
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said.iter().all(|l| l.ends_with("box-a is oversubscribed: load 20 on 16 cores")), "{said:?}");
        let said = slow_units(&dir, now * 1000, &at(false, 40.0));
        assert_eq!(said.len(), 1, "a box that does not answer is not judged oversubscribed by its last load: {said:?}");
        assert!(said[0].ends_with("(9x); box-a does not answer: the seconds are its silence, not the unit's (its lease is lost if it stays so, and the shard runs again)"), "{said:?}");
        assert_eq!(slow_units(&dir, now * 1000, &|_| None), Vec::<String>::new(), "its lease ended");
        // a unit its watch said stalled busy is said as that with its sample, not as slow; one with a
        // new result since is slow again; one never started is nothing
        let stalled = "GATEQUEUE L9 3 0123456789ab\nGATESTART test spin 1000\nGATEBUSY test spin :: 3000 landgate.x/testrun-spin.busy the busiest thread: 7/task/8 worker R 99% wchan 0\nGATESTART test slow 5000\nGATEBUSY test slow :: 4000 landgate.x/testrun-slow.busy\nGATEBUSY test slow :: 0\nGATEBUSY test gone :: 4000 landgate.x/testrun-gone.busy\n";
        std::fs::write(logs.join("tests-s2-1794013801204.log"), stalled).unwrap();
        // each new stall is a note for main as its line comes, keyed so it is said once
        let run = logs.join("tests-1794013801204.log");
        let notes: Vec<(String, String)> = stalled.lines().filter_map(|l| stall_note(&run, 2, Some("L9"), l, 3600)).collect();
        assert_eq!(notes.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["2 test spin 3000", "2 test slow 4000", "2 test gone 4000"], "a stall's key; `:: 0` is no stall");
        assert_eq!(notes[0].1, format!("bucket tests run 1794013801204 shard 2 (L9): `test spin` stalled, no new test result for 10 min; nothing stops it: you or its owner decide. Its state: `devhub get landq-tests tmp/keep/landgate.x/testrun-spin.busy <dir> --lease L9` now, {} once its lease ends: the busiest thread: 7/task/8 worker R 99% wchan 0",
            logs.join("tests-s2-1794013801204.keep/landgate.x/testrun-spin.busy").display()), "the note: run, shard, lease, unit, how long, where its state is and what it says");
        assert!(stall_note(&run, 2, None, "GATEBUSY test slow :: 4000 landgate.x/testrun-slow.busy", 4060).is_some_and(|(_, l)| l.contains("shard 2 (its lease): `test slow` stalled, no new test result for 60 s; ") && l.ends_with("testrun-slow.busy once its lease ends")), "no lease known yet, nothing said by its sample");
        let both = |lease: &str| ["L7", "L9"].contains(&lease).then(|| ShardBox { node: "box-a".into(), up: true, load: 12.0, cores: 16 });
        let said = slow_units(&dir, now * 1000, &both);
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(said[1], format!("tests run 1794013801204 shard 2 (L9 on box-a): `test spin` busy, no new result for 116 min (not measured yet); nothing stops it: main or its owner decides. Its stack sample: `devhub get landq-tests tmp/keep/landgate.x/testrun-spin.busy <dir> --lease L9` now, {} once its lease ends: the busiest thread: 7/task/8 worker R 99% wchan 0", logs.join("tests-s2-1794013801204.keep/landgate.x/testrun-spin.busy").display()));
        assert!(said[2].starts_with("tests run 1794013801204 shard 2 (L9 on box-a): `test slow` running 83 min against its measured 9 min (9x)"), "{said:?}");
        std::fs::remove_file(logs.join("tests-s2-1794013801204.log")).unwrap();
        // a shard that deals no queue is said by its lease's grant line, and checked by it: its
        // lease named, it drops out once that lease ended; a cancelled lease's units end with it
        let unqueued = stalled.replacen("GATEQUEUE L9 3 0123456789ab\n", "L9 test: 1st in queue (box-b: held by L4 (test of landq-tests))\nL9 test@box-a: granted, setting up\n", 1);
        assert_eq!(running_in(&unqueued).0, Some("L9"), "not L4, which its queue line named");
        std::fs::write(logs.join("tests-s2-1794013801204.log"), &unqueued).unwrap();
        let said = slow_units(&dir, now * 1000, &both);
        assert!(said.iter().any(|l| l.starts_with("tests run 1794013801204 shard 2 (L9 on box-a): `test spin` busy")), "{said:?}");
        let l7 = |lease: &str| (lease == "L7").then(|| ShardBox { node: "box-a".into(), up: true, load: 12.0, cores: 16 });
        assert!(slow_units(&dir, now * 1000, &l7).iter().all(|l| !l.contains("shard 2")), "its lease ended: the shard is not said");
        let cancelled = format!("{unqueued}LEASE CANCELLED by user on box-a (L9): pick another node\n");
        assert_eq!(running_in(&cancelled), (None, BTreeMap::new(), BTreeMap::new()), "its units ended with its lease");
        std::fs::remove_file(logs.join("tests-s2-1794013801204.log")).unwrap();
        // a full run's shard is said as a tests run's: its stalled unit held the pool slot it was on
        std::fs::write(logs.join("full-1794013000000.log.run"), "{}").unwrap();
        std::fs::write(logs.join("full-s1-1794013000000.log.run"), "{}").unwrap();
        std::fs::write(logs.join("full-s1-1794013000000.log"), stalled).unwrap();
        let said = slow_units(&dir, now * 1000, &both);
        assert!(said.iter().any(|l| l.starts_with("full run 1794013000000 shard 1 (L9 on box-a): `test spin` busy, no new result for 116 min")), "{said:?}");
        assert!(said.iter().any(|l| l.starts_with("full run 1794013000000 shard 1 (L9 on box-a): `test slow` running 83 min against its measured 9 min")), "a unit full never measured takes the tests bucket's measure: {said:?}");
        for f in ["full-1794013000000.log.run", "full-s1-1794013000000.log.run", "full-s1-1794013000000.log"] { std::fs::remove_file(logs.join(f)).unwrap(); }
        // a run with its row is over: its sidecar stays
        let ended = obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(1794013801204))]);
        std::fs::write(dir.join("buckets.jsonl"), format!("{}\n{}\n", ended.to_json(), row.to_json())).unwrap();
        assert_eq!(slow_units(&dir, now * 1000, &at(true, 12.0)), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hung test unit (land_gate.sh testrun stopped it) fails with the tests it still ran, which
    /// `bucket status` says under the row, and the command that runs them alone.
    #[test]
    fn a_hung_unit_fails_with_the_tests_it_still_ran() {
        let log = "test a: ok (1 passed, 0 failed, 1 targets)\nGATEUNIT ok test a\ntest b: HUNG (its processes did nothing (under 5% of a core, no I/O) for 540 s: stopped; 3 passed, 0 failed before it)\n\
            HUNG: m::t::waits_on_eos; its stacks: testrun-b.hung in the run's kept logs\n{\"verdict\":\"blocked\",\"why\":\"main is blocked\"}\n\
            GATEREPRO test b :: makepad test b -- m::t::waits_on_eos --exact\nGATEUNIT fail test b\nNODEGATE-FAIL: test:b\n";
        let (errors, passed) = unit_lines(log);
        assert_eq!(errors["test b"], ["HUNG: m::t::waits_on_eos; its stacks: testrun-b.hung in the run's kept logs"]);
        assert!(passed.contains("test a") && !errors.contains_key("test a"));
        assert_eq!(repro_lines(log)["test b"], "makepad test b -- m::t::waits_on_eos --exact");
    }

    /// Proven units and failing-unit repro commands keep their package keys.
    #[test]
    fn proven_units_and_repro_commands_are_parsed() {
        let warn = |at: &str, what: &str| format!("/n/tree/apps/f/tests/common/p.rs:{at}: warning: {what}");
        let log = format!("STEP depcheck 1 s\n{}\n{}\nGATEUNIT ok tests f\nGATEUNIT proven tests g\nGATEUNIT ok tests h\n",
            warn("5:11", "constant `MACHINE` is never used"), warn("5:11", "constant `MACHINE` is never used"));
        let proven = proven_lines(&log);
        assert!(proven.contains("tests g") && !proven.contains("tests f") && !proven.contains("tests h"), "only the proven unit: {proven:?}");
        let (errors, passed) = unit_lines(&log);
        assert!(errors.is_empty() && passed.contains("tests f"), "a warning is no failure");
        // a failing unit's, by its package (its flips aside)
        let repro = repro_lines("GATEREPRO check w --also=x :: cd libs/w && makepad check --also=x w\nGATEUNIT fail check w --also=x\nGATEREPRO test t :: makepad test t\n");
        assert_eq!(repro.get("check w").map(String::as_str), Some("cd libs/w && makepad check --also=x w"));
        assert_eq!(repro.get("test t").map(String::as_str), Some("makepad test t"));
        // a boundary audit's red: a unit of its own, apart from its app's check, and blamed by the provided
        // fn's file its violation names (its way in, the `reached` row, names none)
        let audit = "violation\tFAIL\tmakepad_s\tmakepad_s::Heap::get\t/n/tree/platform/s/src/heap.rs:108:1: a field of `Heap` (`objects: Vec<Obj>`) is reached from JIT-compiled code\n  reached\tgame::camera -> makepad_s::Heap::get\nGATEREPRO boundary makepad-x :: cd apps/x && makepad check --boundary all makepad-x\nGATEUNIT fail boundary makepad-x\n";
        assert!(is_unit("boundary makepad-x") && unit_key("boundary makepad-x") != unit_key("check makepad-x"), "a unit of its own");
        assert_eq!(repro_lines(audit).get("boundary makepad-x").map(String::as_str), Some("cd apps/x && makepad check --boundary all makepad-x"));
        assert_eq!(error_files(audit).get("boundary makepad-x"), Some(&vec!["/n/tree/platform/s/src/heap.rs".to_string()]), "its violation's file");
        // as said: a present with the live session a lane runs it in
        assert_eq!(repro_said("test t", "makepad test t"), "test t: shown alone by `makepad test t`");
        let present = repro_said("present makepad-x", "cd apps/x && makepad run makepad-x");
        assert!(present.starts_with("present makepad-x: shown alone by `cd apps/x && makepad run makepad-x`") && present.contains("`makepad devhub live start makepad-x`, then `makepad devhub live look <id>`"), "{present}");
    }

    /// A unit that fails with errors in a file is named for the landing that changed that file
    /// since the unit last passed, with that landing's commit that changed it. This holds even when
    /// the landing came before this run's range. When no landing since the pass touched the file,
    /// the unit's inputs answer for it as before.
    #[test]
    fn a_failure_is_named_for_the_landing_that_changed_its_errors_file() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-file-blame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(d.join("platform/script/std/src")).unwrap();
        std::fs::create_dir_all(d.join("apps/example/src")).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        std::fs::write(c.join("m"), "m\n").unwrap();
        g(&c, &["add", "-A"]);
        g(&c, &["commit", "-q", "-m", "m"]);
        let m = g(&c, &["rev-parse", "HEAD"]);
        let commit = |file: &str, text: &str, msg: &str| -> String {
            std::fs::write(d.join(file), text).unwrap();
            g(&d, &["add", "-A"]);
            g(&d, &["commit", "-q", "-m", msg]);
            g(&d, &["rev-parse", "HEAD"])
        };
        // the example's package and a test target older than the ledger's landings
        std::fs::create_dir_all(d.join("apps/example/tests")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[workspace]\nmembers = [\"apps/example\"]\n").unwrap();
        std::fs::write(d.join("apps/example/Cargo.toml"), "[package]\nname = \"example\"\n").unwrap();
        std::fs::write(d.join("apps/example/tests/old.rs"), "fn main() {}\n").unwrap();
        commit("platform/script/std/src/vm.rs", "fn with_vm() {}\n", "script std: the vm");
        let w0 = commit("apps/example/src/main.rs", "fn main() {}\n", "example");
        // the break: a lane's two commits, the second changing the function the JIT cannot take
        commit("platform/script/std/src/lib.rs", "\n", "script std: a module");
        let w1 = commit("platform/script/std/src/vm.rs", "fn with_vm() {}\nfn resume_if_panicked() {}\n", "script std: a panic in a with_vm closure no longer loses the VM");
        // what landed after it, each changing the apps' inputs elsewhere
        // (#2 brings a test target too)
        std::fs::write(d.join("apps/example/tests/fresh.rs"), "fn main() {}\n").unwrap();
        let w2 = commit("apps/example/src/main.rs", "fn main() { tree() }\n", "example: trees");
        let w3 = commit("platform/script/std/src/lib.rs", "\n\n", "script std: more");
        let entry = |id: u64, pre: &str, landed: &str| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: format!("work {landed} main {m}"), pre: format!("work {pre} main {m}"), detail: Vec::new(),
            agent: format!("L{id}"), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms: 0 };
        let history = vec![entry(1, &w0, &w1), entry(2, &w1, &w2), entry(3, &w2, &w3)];
        // the present's summary, under a node's tree, and a rustc error in a test file of its own
        let log = "app: no present (failed before its first frame: relax: compile error in makepad_script_std::vm::ScriptVmCall::resume_if_panicked: platform/script/std/src/vm.rs:2:4: `resume_if_panicked` calls\terror\tcompile error)\n\
            /n/tree/apps/example/src/main.rs:1:13: warning: unused\nGATEUNIT fail present example\n/n/tree/apps/example/src/main.rs:1:13: E0425: cannot find function `tree`\nGATEUNIT fail check example\n";
        let files = error_files(log);
        assert_eq!(files["present example"], ["platform/script/std/src/vm.rs"], "the summary's file, not the warning's");
        assert_eq!(files["check example"], ["/n/tree/apps/example/src/main.rs"]);
        // the present last passed before the break; this run's range holds only the landings after it
        let rows = [obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w2, &m)))), ("passed", Value::Arr(vec![s("check example")]))]),
            obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w0, &m)))), ("passed", Value::Arr(vec![s("present example")]))])];
        let tip = Pair::new(&w3, &m);
        let from = last_pass(&rows, "present example").unwrap().pair();
        assert_eq!(from, Pair::new(&w0, &m));
        let since = landings(&d, &from, &tip, &history).unwrap();
        assert_eq!(since.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2, 3]);
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        let commit_of = |e: &Entry, p: &str| entry_pair(&e.landed).and_then(|at| last_touch(&d, &at, p));
        assert_eq!(file_blame(&files["present example"], &since, &touched, &commit_of), [(1, format!("rc-l1 (L1) #1 ({} changed vm.rs)", &w1[..10]))]);
        // the same with the box's path to the tree
        assert_eq!(file_blame(&["/n/tree/platform/script/std/src/vm.rs".to_string()], &since, &touched, &commit_of).len(), 1);
        // a landing whose diff cannot be read is not named for it
        assert!(file_blame(&files["present example"], &since, &[None, None, None], &commit_of).is_empty());
        // the statics step names each static's line: the landing whose diff added that line, alone,
        // not the others of the range; a line from before the range names nobody
        let statics = "STEP statics 1 s [0-1]\nno reset and no reason: makepad_script_std::vm::PANICKED (platform/script/std/src/vm.rs:2)\n\
            no reset and no reason: example::TREES (apps/example/src/main.rs:1)\nno reset and no reason: makepad_script_std::vm::VMS (platform/script/std/src/vm.rs:1)\nNODEGATE-FAIL: statics\n";
        let lines = statics_lines(statics);
        assert_eq!(lines.iter().map(|(p, n, _)| (p.as_str(), *n)).collect::<Vec<_>>(), [("platform/script/std/src/vm.rs", 2), ("apps/example/src/main.rs", 1), ("platform/script/std/src/vm.rs", 1)]);
        let text = |at: &Pair, p: &str| file_text(&d, at, p);
        let named: Vec<Option<u64>> = lines.iter().map(|(p, n, _)| line_blame(&since, &tip, p, *n, &text).map(|e| e.id)).collect();
        assert_eq!(named, [Some(1), Some(2), None], "the adder of each static's line, and nobody for one older than the range");
        assert!(line_blame(&since, &tip, "platform/script/std/src/vm.rs", 9, &text).is_none(), "a line the file does not have");
        // its landing is told at the first red run that names it, not again while the step stays red
        let who = format!("{} (it added vm.rs:2), {} (it added main.rs:1)", who_of(&since[0]), who_of(&since[1]));
        assert_eq!(statics_new(&who, ""), who);
        assert_eq!(statics_new(&who, &format!("{} (it added vm.rs:2)", who_of(&since[0]))), format!("{} (it added main.rs:1)", who_of(&since[1])));
        assert_eq!(statics_new(&who, &format!("rc-l1 (L1) #12 (it added x.rs:1), {} (it added main.rs:1)", who_of(&since[1]))), format!("{} (it added vm.rs:2)", who_of(&since[0])), "#1 is not #12");
        // The first red names its landing to the event stream, its agent and main, once (11-14).
        let home = d.join("bucket-notes");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open_quiet(&home, "").unwrap();
        store.update(|doc| doc.land = history.iter().map(|e| makepad_strict_json::parse(e.to_json().as_bytes()).unwrap()).collect()).unwrap();
        let raw = "no reset and no reason: makepad_script_std::vm::PANICKED (platform/script/std/src/vm.rs:2)";
        let judged = || {
            let log = home.join("fast.log");
            let text = format!("STEP statics 1 s [0-1]\n{raw}\nNODEGATE-FAIL: statics\n");
            std::fs::write(&log, &text).unwrap();
            let outcome = verdict(Ok(Some(1)), &text, &log);
            let fed = Mutex::new(Vec::new());
            judge("fast", &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1,
                Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
            fed.into_inner().unwrap()
        };
        let first = judged();
        let broke: Vec<&String> = first.iter().filter(|(sev, line, _)| *sev == Sev::Err && line.contains(" turned ")).map(|(_, line, _)| line).collect();
        assert_eq!(broke.len(), 1, "one event for the landing: {first:?}");
        assert!(broke[0].contains("#1 rc-l1 (L1) lane rc-l1 turned statics red:") && broke[0].contains(raw), "{}", broke[0]);
        assert_eq!(first.iter().filter(|(_, line, _)| line.contains(" FAILED ")).count(), 1, "the run still has its FAILED event");
        for agent in ["L1", "main"] {
            let notes = crate::notes::take(&home, agent);
            assert_eq!(notes.iter().map(|(_, line)| line).collect::<Vec<_>>(), broke, "{agent} gets the landing's message once");
        }
        let next = judged();
        assert_eq!(next.len(), 1, "only the run's FAILED event on the next red: {next:?}");
        assert!(next[0].1.contains(" FAILED ") && !next[0].1.contains(" turned "), "{next:?}");
        for agent in ["L1", "main"] { assert!(crate::notes::take(&home, agent).is_empty(), "no repeated note to {agent}"); }
        // A test's red waits for its bisect, its red tip first: no landing hears of it before, not
        // even the one landing that touched its file, which a bucket without a probe tells at once;
        // one a later run of another bucket passed is probed at its tip alone (a key-shadow red was
        // laid to 14 landings of 8 lanes, and it passed at the next tip, 11-17).
        let test_red = |bucket: &str, bisect: Option<&dyn Fn(Bisect)>| {
            let log = home.join(format!("{bucket}-t.log"));
            let text = "/n/tree/apps/example/src/main.rs:1:13: E0425: cannot find function `tree`\nGATEUNIT fail test example --test t\nNODEGATE-FAIL: tests\n";
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(1)), text, &log);
            let fed = Mutex::new(Vec::new());
            judge(bucket, &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), bisect, |_| Ok(outcome)).unwrap();
            fed.into_inner().unwrap()
        };
        let told = test_red("mac", None);
        assert!(told.iter().any(|(_, line, _)| line.contains("#2 rc-l2 (L2) lane rc-l2 turned test example red")), "without a probe its landing is told: {told:?}");
        assert_eq!(crate::notes::take(&home, "L2").len(), 1);
        crate::notes::take(&home, "main");
        // a bucket's first run has no green baseline: a red whose errors' files no landing changed
        // is laid to nobody, not to every landing of its range
        let log = home.join("windows-first.log");
        let text = "libs/other/src/x.rs:1:1: error[E0433]: cannot find `unix` in `os`\nGATEUNIT fail windows other\nNODEGATE-FAIL: windows other\n";
        std::fs::write(&log, text).unwrap();
        let outcome = verdict(Ok(Some(101)), text, &log);
        let fed = Mutex::new(Vec::new());
        judge(WINDOWS, &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
        let fed = fed.into_inner().unwrap();
        assert!(!fed.iter().any(|(_, line, _)| line.contains(" turned ")), "no landing is told: {fed:?}");
        assert_eq!(still_failing(&rows_of(&home, WINDOWS))["windows other"].1, "origin unknown, first failed in windows-first.log");
        for agent in ["L1", "L2"] { assert!(crate::notes::take(&home, agent).is_empty(), "{agent} hears nothing"); }
        let queued: Mutex<Vec<Bisect>> = Mutex::new(Vec::new());
        let queue = |job: Bisect| queued.lock().unwrap().push(job);
        let held = test_red("tests", Some(&queue));
        assert!(!held.iter().any(|(_, line, _)| line.contains(" turned ")), "{held:?}");
        assert!(held.iter().any(|(_, line, _)| line.contains(&format!("test example --test t bisected since {}", Pair::new(&w0, &m).short()))), "{held:?}");
        for agent in ["L2", "main"] { assert!(crate::notes::take(&home, agent).is_empty(), "{agent} hears nothing before the bisect ends"); }
        let jobs = std::mem::take(&mut *queued.lock().unwrap());
        assert_eq!(jobs.iter().map(|j| (j.unit.as_str(), j.key.as_str(), j.closed)).collect::<Vec<_>>(), [("test example --test t", "test example", false)], "one bisect of the red");
        assert_eq!(jobs[0].from.pair(), Pair::new(&w0, &m), "since its run's base: nothing passed it before");
        append(&home, obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(5)), ("passed", Value::Arr(vec![s("test example")])), ("tip", tip_value(&pin_of(&tip)))])).unwrap();
        let held = test_red(FULL, Some(&queue));
        assert!(held.iter().any(|(_, line, _)| line.contains("test example --test t runs once more at its red tip")), "{held:?}");
        assert_eq!(queued.lock().unwrap().iter().map(|j| j.closed).collect::<Vec<_>>(), [true], "a red a later run passed is closed: its tip is probed alone");
        // its range starts where its last pass ran each nested repository, as the bucket's row
        // names it, not at the red tip's: each probe took the red tip's apps, every one was red
        // the same way, and the red of an apps landing went to no landing (11-23)
        let apps_at = "a".repeat(40);
        append(&home, obj(vec![("bucket", s("tests")), ("started_ms", Value::Int(0)), ("passed", Value::Arr(vec![s("test example")])), ("tip", tip_value(&pin_of(&Pair::new(&w0, &m)).with("apps", &apps_at)))])).unwrap();
        queued.lock().unwrap().clear();
        test_red("fast", Some(&queue));
        let jobs = std::mem::take(&mut *queued.lock().unwrap());
        assert_eq!(jobs.iter().map(|j| j.from.nested.get("apps")).collect::<Vec<_>>(), [Some(&apps_at)], "the last pass's own apps commit starts the range");
        // a CI leg's new red runs once more at its tip first, the landing its errors name kept for then
        let log = home.join("ci-p.log");
        std::fs::write(&log, "fail check:macos:example: 1 step failed (c.log)\n/n/tree/apps/example/src/main.rs:1:13: E0425: cannot find function `tree`\nGATEUNIT fail check:macos:example\n").unwrap();
        let outcome = Outcome { ok: false, failing: vec!["check:macos:example".into()], log: log.display().to_string(), summary: String::new(), no_verdict: None };
        let fed = Mutex::new(Vec::new());
        judge(CI, &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), Some(&queue), |_| Ok(outcome)).unwrap();
        let fed = fed.into_inner().unwrap();
        assert!(!fed.iter().any(|(_, line, _)| line.contains(" turned ")), "nobody before its tip's probe: {fed:?}");
        assert!(fed.iter().any(|(_, line, _)| line.contains("check:macos:example runs once more at its red tip: its owner is told should it pass there, else the landing its errors name (#2)")), "{fed:?}");
        for agent in ["L2", "main"] { assert!(crate::notes::take(&home, agent).is_empty(), "{agent} hears nothing before the probe ends"); }
        let job = queued.lock().unwrap().pop().unwrap();
        assert_eq!((job.unit.as_str(), job.key.as_str(), job.named.as_slice(), job.of_a_leg()), ("check:macos:example", "check:macos:example", &[2u64][..], true), "one probe of the leg");
        // a CI leg's errors name the landing that touched their file, and that landing alone hears it
        let log = home.join("ci-t.log");
        let text = "fail check:linux:example: 1 step failed (c.log)\n/n/tree/apps/example/src/main.rs:1:13: E0425: cannot find function `tree`\nGATEUNIT fail check:linux:example\n";
        std::fs::write(&log, text).unwrap();
        let outcome = Outcome { ok: false, failing: vec!["check:linux:example".into()], log: log.display().to_string(), summary: String::new(), no_verdict: None };
        let fed = Mutex::new(Vec::new());
        judge(CI, &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
        let fed = fed.into_inner().unwrap();
        assert!(fed.iter().any(|(_, line, _)| line.contains("#2 rc-l2 (L2) lane rc-l2 turned check:linux:example red")), "{fed:?}");
        assert!(!fed.iter().any(|(_, line, _)| line.contains("rc-l1 (L1)") || line.contains("rc-l3 (L3)")), "no other landing of the range: {fed:?}");
        for agent in ["L2", "main"] { assert_eq!(crate::notes::take(&home, agent).len(), 1, "{agent}"); }
        // the check last passed at w2: since then only #3, which did not touch main.rs (the callee changed elsewhere)
        let from = last_pass(&rows, "check example").unwrap().pair();
        let since = landings(&d, &from, &tip, &history).unwrap();
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        assert!(file_blame(&files["check example"], &since, &touched, &commit_of).is_empty(), "the landings that changed its inputs answer for it");
        assert!(last_pass(&rows, "guard x").is_none());
        // a failing step (the JIT's build, no unit) by the files its own lines name, a warning's too;
        // a unit's lines (a test target's warning) are the unit's. The build last got through at w0:
        // its two red runs since are one range, and only #1 touched vm.rs
        let log = "/n/tree/platform/script/std/src/vm.rs:2:4: warning: function `resume_if_panicked` is never used\n\
            warning: `makepad-script-std` (lib) generated 1 warning\nLOADER-FAIL warnings\nSTEP jit 1 s [0-1]\n\
            /n/tree/apps/example/src/main.rs:1:13: warning: unused\nGATEUNIT ok tests example\nSTEP testcheck 1 s [1-2]\nNODEGATE-FAIL: build\n";
        assert_eq!(step_files(log), ["/n/tree/platform/script/std/src/vm.rs"]);
        let rows = [obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w2, &m)))), ("failing", Value::Arr(vec![s("build")]))]),
            obj(vec![("tip", tip_value(&pin_of(&Pair::new(&w0, &m)))), ("failing", Value::Arr(vec![s("check example")]))])];
        let from = last_pass(&rows, "build").unwrap().pair();
        assert_eq!(from, Pair::new(&w0, &m), "a row without the step failing got through it");
        let since = landings(&d, &from, &tip, &history).unwrap();
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        assert_eq!(file_blame(&step_files(log), &since, &touched, &commit_of), [(1, format!("rc-l1 (L1) #1 ({} changed vm.rs)", &w1[..10]))]);
        // A unit red only in a test listing its bucket never had a result for is red at its first
        // run here: its target's owner hears it (the landing that last changed its file), not the
        // landings under it, and one no landing owns goes to main. A listing an earlier row's log
        // has (a row from before rows kept their listings) is no first run (11-22).
        for agent in ["L1", "L2", "L3", "main"] { crate::notes::take(&home, agent); }
        let earlier = |bucket: &str, kept: bool| {
            let log = home.join(format!("{bucket}-0.log"));
            std::fs::write(&log, "prebuild w: 2 test listings\nexample:lib on b1: exit 0\nGATEUNIT ok suite example\nexample:old on b1: exit 0\nGATEUNIT ok suite example\n").unwrap();
            let mut row = vec![("bucket", s(bucket)), ("started_ms", Value::Int(0)), ("passed", Value::Arr(vec![s("suite example")])), ("log", s(log.display().to_string())), ("tip", tip_value(&pin_of(&Pair::new(&w0, &m))))];
            if kept { row.push(("targets", obj(vec![("suite example", Value::Arr(vec![s("example:lib")]))]))); }
            append(&home, obj(row)).unwrap();
        };
        let listings = |bucket: &str, red: &str| {
            let log = home.join(format!("{bucket}-1.log"));
            let text = format!("prebuild x: 2 test listings\nexample:lib on b1: exit 0\nGATEUNIT ok suite example\n{red} on b2: exit 101\nEXIT: exit 101 with no compile error or panic\nGATEUNIT fail suite example\n");
            std::fs::write(&log, &text).unwrap();
            let outcome = Outcome { ok: false, failing: vec!["suite example".into()], log: log.display().to_string(), summary: String::new(), no_verdict: None };
            let fed = Mutex::new(Vec::new());
            judge(bucket, &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
            (fed.into_inner().unwrap(), rows_of(&home, bucket).into_iter().next().unwrap())
        };
        let broke_of = |row: &Value| row.get("broke").and_then(|b| b.get("suite example")).and_then(Value::as_str).unwrap_or("").to_string();
        earlier("prebuild", true);
        let (fed, row) = listings("prebuild", "example:fresh");
        let said = format!("bucket prebuild {}: suite example {FIRST_RUN} (example:fresh): its owner rc-l2 (L2) #2, which last changed apps/example/tests/fresh.rs: ", tip_short(&pin_of(&tip)));
        assert!(fed.iter().any(|(_, line, _)| line.starts_with(&said)), "its owner hears it: {fed:?}");
        assert!(!fed.iter().any(|(_, line, _)| line.contains(" turned ")), "no landing under it is told it turned the unit red: {fed:?}");
        for (agent, n) in [("L2", 1), ("main", 1), ("L1", 0), ("L3", 0)] { assert_eq!(crate::notes::take(&home, agent).len(), n, "{agent}"); }
        assert!(broke_of(&row).starts_with(FIRST_RUN), "{row:?}");
        assert!(!row.get("landings").and_then(Value::as_arr).unwrap_or_default().iter().any(|l| l.get("units").and_then(Value::as_arr).unwrap_or_default().iter().any(|u| u.as_str() == Some("suite example"))), "no landing's units: {row:?}");
        assert_eq!(row.get("targets").and_then(|t| t.get("suite example")).and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).collect::<Vec<_>>(), ["example:fresh", "example:lib"], "the row keeps its listings");
        earlier("unowned", true);
        let (fed, row) = listings("unowned", "example:old");
        assert!(fed.iter().any(|(_, line, _)| line.contains(&format!("suite example {FIRST_RUN} (example:old): its owner unknown: no landing found for apps/example/tests/old.rs: "))), "{fed:?}");
        for (agent, n) in [("main", 1), ("L1", 0), ("L2", 0), ("L3", 0)] { assert_eq!(crate::notes::take(&home, agent).len(), n, "{agent}: one no landing owns is main's"); }
        assert!(broke_of(&row).starts_with(FIRST_RUN));
        earlier("listed", false);
        let (fed, row) = listings("listed", "example:old");
        assert!(!fed.iter().any(|(_, line, _)| line.contains(FIRST_RUN)) && !broke_of(&row).starts_with(FIRST_RUN), "its listing ran here before (an earlier row's log): {fed:?}");
        // a unit that failed on a file its box's tree lacked, which the pinned tree holds, is the
        // box's: no verdict, no landing told (a warm leg's tree without makepad/apps, 11-22); one
        // whose manifest names a file the tree lacks too is the code's, a red
        let lacked = |text: &str, failing: &[&str]| {
            let log = home.join("infra.log");
            std::fs::write(&log, text).unwrap();
            let outcome = Outcome { ok: false, failing: failing.iter().map(|u| u.to_string()).collect(), log: log.display().to_string(), summary: String::new(), no_verdict: None };
            let fed = Mutex::new(Vec::new());
            let r = judge("infra", &home, &d, &store, &|sev, line, detail| fed.lock().unwrap().push((sev, line, detail)), 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&tip), None, |_| Ok(outcome));
            (r.err().map(|e| e.to_string()), fed.into_inner().unwrap())
        };
        let relax = "warming example:lib\nrelax: /n/dev/src/apps/example/src/main.rs: /n/dev/src/apps/example/src/main.rs: no such file\nrelax test: the run of the lib's unit tests exited with status 1\nGATEUNIT fail suite example\n";
        let cargo = "error: failed to load manifest for workspace member `/n/tree/apps/example`\nCaused by:\n  failed to read `/n/tree/apps/example/Cargo.toml`\nCaused by:\n  No such file or directory (os error 2)\nGATESTEP fail check\n";
        let (none, told) = lacked(&format!("{relax}{cargo}"), &["suite example", "check"]);
        let none = none.unwrap_or_default();
        assert!(none.contains("infra, no lane's: check: its box's tree lacked apps/example/Cargo.toml, which the pinned tree holds; suite example: its box's tree lacked apps/example/src/main.rs"), "{none}");
        assert!(!told.iter().any(|(sev, _, _)| *sev == Sev::Err) && rows_of(&home, "infra").is_empty(), "no red, no landing told: {told:?}");
        let gone = "  failed to read `/n/tree/apps/example/gone/Cargo.toml`\nGATEUNIT fail check gone\n";
        let (verdict, _) = lacked(&format!("{relax}{gone}"), &["suite example", "check gone"]);
        let row = rows_of(&home, "infra").into_iter().next().unwrap();
        assert!(verdict.is_none() && row.get("failing").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).eq(["check gone"]), "a file the tree lacks too is the code's red, the box's is not: {row:?}");
        assert!(row.get("summary").and_then(Value::as_str).unwrap_or("").contains("infra, no lane's: suite example"), "{row:?}");
        // A failing test is blamed by its own message (11-25). Its first red after a pass keeps its
        // panics in the look back, so a new break of a test that broke before for another reason is
        // no failure off and on; and a line of the tree its message names is laid to the landing
        // since the test last passed that added it, alone (a missing Option entry the boundary
        // check named at its line, under the harness's assert, read "intermittent, origin unknown")
        let unit = "test example --lib";
        let run = |log: &str, at: &str, said: Option<&str>| -> String {
            let path = home.join(log);
            let text = match said {
                Some(said) => format!("FAILED live::t\n  panicked at apps/example/tests/fresh.rs:1:1 -- {said}\nGATEUNIT fail {unit}\nNODEGATE-FAIL: tests\n"),
                None => format!("GATEUNIT ok {unit}\nNODEGATE-OK\n"),
            };
            std::fs::write(&path, &text).unwrap();
            // (the run's verdict: this test's `verdict` above is a refusal's text)
            let outcome = super::verdict(Ok(Some(if said.is_some() { 1 } else { 0 })), &text, &path);
            judge("lines", &home, &d, &store, &|_, _, _| {}, 1, Some(pin_of(&Pair::new(&w0, &m))), &pin_of(&Pair::new(at, &m)), None, |_| Ok(outcome)).unwrap();
            rows_of(&home, "lines").first().and_then(|r| r.get("broke")).and_then(|b| b.get("test example")).and_then(Value::as_str).unwrap_or("").to_string()
        };
        run("lines-1.log", &w0, Some("kept the old value"));
        run("lines-2.log", &w0, None);
        let again = run("lines-3.log", &w3, Some("lost the new value"));
        assert!(again.starts_with("by test: live::t ") && !again.contains(INTERMITTENT), "another break than its first red's: {again}");
        let named = run("lines-4.log", &w3, Some("1 missing entry:\n    entry\tFAIL\tplatform/script/std/src/vm.rs:2 (makepad_script_std::vm)\tadd it"));
        assert_eq!(named, format!("by test: live::t since {} (it added vm.rs:2)", who_of(&history[0])), "the landing that added the line it names, not the three since its pass");
        let said = said_lines(&std::fs::read_to_string(home.join("lines-4.log")).unwrap());
        assert_eq!(said, BTreeMap::from([((unit.to_string(), "live::t".to_string()), vec![("platform/script/std/src/vm.rs".to_string(), 2)])]), "where it panicked is no line it names");
        for agent in ["L1", "L2", "L3"] { let _ = crate::notes::take(&home, agent); }
        let _ = crate::notes::take(&home, "main");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A unit red on a warning alone (the JIT check of an app's program build) is blamed by the
    /// warning's file: of a range of 8 landings, only the one that touched that file.
    #[test]
    fn a_unit_red_on_a_warning_alone_is_blamed_on_the_landing_that_touched_its_file() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-red-warning-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(c.join("demo/src")).unwrap();
        std::fs::create_dir_all(d.join("platform/src")).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        let commit = |repo: &Path, file: &str, text: &str| -> String {
            std::fs::write(repo.join(file), text).unwrap();
            g(repo, &["add", "-A"]);
            g(repo, &["commit", "-q", "-m", file]);
            g(repo, &["rev-parse", "HEAD"])
        };
        let (mut w, mut m) = (commit(&d, "platform/src/lib.rs", "0\n"), commit(&c, "demo/src/x.rs", "0\n"));
        let base = Pair::new(&w, &m);
        // eight landings, each changing the app's inputs (the platform, its commercial crates);
        // only #4051 touches x.rs
        let mut history = Vec::new();
        for id in 4045..=4052u64 {
            let pre = format!("work {w} main {m}");
            match id {
                4051 => m = commit(&c, "demo/src/x.rs", "fn unused_helper() {}\n"),
                _ if id % 2 == 0 => w = commit(&d, "platform/src/lib.rs", &format!("{id}\n")),
                _ => m = commit(&c, "demo/src/y.rs", &format!("{id}\n")),
            }
            history.push(Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
                ms: String::new(), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: format!("work {w} main {m}"), pre, detail: Vec::new(), agent: format!("L{id}"), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms: 0 });
        }
        let log = "makepad-demo: 0 errors\n/n/ci-tree/apps/commercial/demo/src/x.rs:12:4: warning: function `unused_helper` is never used [dead_code]\n\
            makepad-demo: 1 warnings\nGATEREPRO check makepad-demo :: cd apps/commercial/demo && makepad check makepad-demo\nGATEUNIT fail check makepad-demo\n";
        let files = error_files(log);
        assert_eq!(files["check makepad-demo"], ["/n/ci-tree/apps/commercial/demo/src/x.rs"], "the warning's file, as no error names one");
        assert!(!error_files(&log.replace("GATEUNIT fail check", "GATEUNIT fail present")).contains_key("present makepad-demo"), "a present's warning is not why it failed");
        assert_eq!(error_files(&log.replace("GATEUNIT fail check", "GATEUNIT fail tests"))["tests makepad-demo"], ["/n/ci-tree/apps/commercial/demo/src/x.rs"], "a test target's check fails on its warning: its file");
        let since = landings(&d, &base, &Pair::new(&w, &m), &history).unwrap();
        assert_eq!(since.len(), 8);
        let diffs: Vec<Option<Vec<String>>> = since.iter().map(|e| landing_files(&d, e)).collect();
        let touched: Vec<Option<&[String]>> = diffs.iter().map(Option::as_deref).collect();
        let who = file_blame(&files["check makepad-demo"], &since, &touched, &|e, p| entry_pair(&e.landed).and_then(|at| last_touch(&d, &at, p)));
        assert_eq!(who.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [4051], "{who:?}");
        assert!(who[0].1.starts_with("rc-l4051 (L4051) #4051 (") && who[0].1.ends_with(" changed x.rs)"), "{}", who[0].1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A red names only the landings that changed its unit's package or dependencies: never one
    /// that changed only the gate's scripts.
    #[test]
    fn a_red_names_only_landings_that_changed_its_package_or_dependencies() {
        assert_eq!(named_changers(&[(0, false), (1, true), (3, true)]), [1, 3]);
        assert!(named_changers(&[(0, false), (2, false)]).is_empty(), "only the gate's scripts: nobody");
        assert!(named_changers(&[]).is_empty());
    }

    /// A bucket's range holds the landings whose moved side its tip brought: a commercial-only
    /// landing after a work landing left work at that landing's commit, and is no landing of the
    /// range that ends there.
    #[test]
    fn a_commercial_landing_is_not_in_the_range_of_the_work_landing_before_it() {
        let g = |dir: &Path, args: &[&str]| -> String {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let d = std::env::temp_dir().join(format!("devhub-bucket-landings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        g(&c, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "apps/commercial/\n").unwrap();
        let commit = |dir: &Path, file: &str| -> String {
            std::fs::write(dir.join(file), format!("{file}\n")).unwrap();
            g(dir, &["add", "-A"]);
            g(dir, &["commit", "-q", "-m", file]);
            g(dir, &["rev-parse", "HEAD"])
        };
        let (w0, m0) = (commit(&d, "w0"), commit(&c, "m0"));
        let w1 = commit(&d, "w1");
        let m1 = commit(&c, "m1");
        let m2 = commit(&c, "m2");
        let entry = |id: u64, pre: (&str, &str), landed: (&str, &str)| Entry { id, branch: format!("rc-l{id}"), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: format!("work {} main {}", landed.0, landed.1), pre: format!("work {} main {}", pre.0, pre.1), detail: Vec::new(),
            agent: format!("L{id}"), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms: 0 };
        let entries = [entry(1, (&w0, &m0), (&w1, &m0)), entry(2, (&w1, &m0), (&w1, &m1)), entry(3, (&w1, &m1), (&w1, &m2))];
        let ids = |base: Pair, tip: Pair| landings(&d, &base, &tip, &entries).unwrap().iter().map(|e| e.id).collect::<Vec<_>>();
        // the run at the work landing: its tip has work at w1, main where it was
        assert_eq!(ids(Pair::new(&w0, &m0), Pair::new(&w1, &m0)), [1]);
        // the next run brought both commercial landings
        assert_eq!(ids(Pair::new(&w1, &m0), Pair::new(&w1, &m2)), [2, 3]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A run's new red is told to the agents of the landings its who names ([`told_agents`]): `#12`
    /// is no `#123` of the same branch, a landing without an agent tells nobody, and a unit red
    /// already at its last look was told then.
    #[test]
    fn a_new_red_tells_the_agents_of_the_landings_it_names() {
        let entry = |id: u64, branch: &str, agent: &str| Entry { id, branch: branch.to_string(), cwt: String::new(), cbranch: String::new(), nested: Vec::new(), state: St::Landed, held: String::new(),
            ms: String::new(), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: String::new(), pre: String::new(), detail: Vec::new(),
            agent: agent.to_string(), proof: false, wire: false, witness: String::new(), asked_ms: 0, changed_ms: 0 };
        let history = [entry(12, "rc-x", "LANE-A"), entry(123, "rc-x", "LANE-A"), entry(124, "rc-y", "LANE-B"), entry(125, "rc-z", ""), entry(126, "rc-w", "LANE-C")];
        let broke: BTreeMap<String, String> = [
            ("check x", format!("{}, {}", who_of(&history[1]), who_of(&history[3]))),
            ("check y", format!("{} (abc1234 changed s.rs)", who_of(&history[2]))),
            ("test z", format!("{BY_TEST}t::a since {}", who_of(&history[4]))),
        ].into_iter().map(|(k, who)| (k.to_string(), who)).collect();
        let before: BTreeMap<String, (String, String, String)> = [("test z".to_string(), ("test z --lib".to_string(), who_of(&history[4]), "tests-1.log".to_string()))].into();
        let told = told_agents(&broke, &before, &history);
        assert_eq!(told.into_iter().collect::<Vec<_>>(), [("LANE-A".to_string(), vec![123]), ("LANE-B".to_string(), vec![124])]);
    }

    #[test]
    fn a_failed_unit_stays_red_until_it_passes() {
        let row = |failing: &[&str], passed: &[&str], broke: &[(&str, &str)]| obj(vec![
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())),
            ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())),
            ("broke", Value::Obj(broke.iter().map(|(k, who)| (k.to_string(), s(*who))).collect())),
        ]);
        let failed = row(&["check x --also=y", "present p", "devhub makepad-devhub", "tests t"], &["check m"], &[("check x", "rc-a (A) #970")]);
        // a later run that only looked at Calculator: its diff touched nothing, and it is no pass
        let quiet = row(&[], &["present makepad-app-calculator"], &[]);
        let still = still_failing(&[quiet.clone(), failed.clone()]);
        assert_eq!(still.keys().map(String::as_str).collect::<Vec<_>>(), ["check x", "devhub makepad-devhub", "present p", "tests t"]);
        assert_eq!((still["check x"].0.as_str(), still["check x"].1.as_str()), ("check x --also=y", "rc-a (A) #970"), "the unit as it failed, with its flips");
        assert_eq!(still["present p"].1, ORIGIN_UNKNOWN);
        assert_eq!(retry_env(&[quiet.clone(), failed.clone()]), "GATE_RETRY=check x --also=y;devhub makepad-devhub;present p;tests t");
        // a pass settles it, also under other flips; an older failure stays settled
        let fixed = row(&[], &["check x", "present p", "tests t"], &[]);
        assert_eq!(still_failing(&[fixed.clone(), quiet.clone(), failed.clone()]).keys().map(String::as_str).collect::<Vec<_>>(), ["devhub makepad-devhub"]);
        // failing again after a pass: open again
        assert!(still_failing(&[row(&["present p"], &[], &[]), fixed, failed.clone()]).contains_key("present p"));
        // a package that left the tree has nothing left to fail
        let (_, passed) = unit_lines("GATEUNIT gone check x --also=y\n");
        assert!(passed.contains("check x"));
        // a step's failure names no unit: the gate takes the JIT once more, and any verdict settles it
        assert_eq!(retry_env(&[row(&["build"], &[], &[])]), "GATE_RETRY=steps");
        assert_eq!(retry_env(&[quiet.clone(), row(&["build"], &[], &[])]), "GATE_RETRY=");
        // the tests bucket's run of a package's tests is a unit of its own, red until it passes
        let tests = row(&["test t", "tests t"], &[], &[]);
        assert_eq!(retry_env(&[quiet.clone(), tests.clone()]), "GATE_RETRY=test t;tests t");
        assert_eq!(still_failing(&[row(&[], &["test t"], &[]), tests]).keys().map(String::as_str).collect::<Vec<_>>(), ["tests t"]);
        // and the hot bucket's GPU guard of a package: a unit, so its break is named, and red until it passes
        assert_eq!(retry_env(&[quiet.clone(), row(&["guard g"], &[], &[])]), "GATE_RETRY=guard g");
        // and the fast bucket's wasm32 check of a package
        assert_eq!(retry_env(&[quiet, row(&["wasm w"], &[], &[])]), "GATE_RETRY=wasm w");
        // a present failing again is the same break: who broke it, not the landings since
        let now = ["present p".to_string()];
        assert_eq!(carried_since(&[failed], &now, &BTreeMap::new()).get("present p").map(String::as_str), Some(ORIGIN_UNKNOWN));
    }

    /// The first recheck of a failure from before rows kept errors named whatever had just landed,
    /// and each later run carried that name. Its first run's word stands, and it had none.
    #[test]
    fn a_failure_older_than_the_ledgers_names_is_of_unknown_origin() {
        let old = obj(vec![("failing", Value::Arr(vec![s("check z")])), ("log", s("/h/bucket-logs/fast-1.log"))]);
        let quiet = obj(vec![("failing", Value::Arr(vec![])), ("errors", obj(vec![])), ("passed", Value::Arr(vec![s("check m")]))]);
        let unknown = "origin unknown, first failed in fast-1.log";
        let now = BTreeMap::from([("check z".to_string(), vec!["a.rs: warning: unused".to_string()])]);
        let failing = ["check z".to_string()];
        assert_eq!(carried_since(&[quiet.clone(), old.clone()], &failing, &now).get("check z").map(String::as_str), Some(unknown), "no errors to compare with: the same break");
        // a wrong name a later run wrote does not stand either
        let named = obj(vec![("failing", Value::Arr(vec![s("check z")])), ("errors", Value::Obj([("check z".to_string(), Value::Arr(vec![s("a.rs: warning: unused")]))].into_iter().collect())), ("broke", obj(vec![("check z", s("rc-late (L) #9"))]))]);
        assert_eq!(carried_since(&[named.clone(), quiet.clone(), old.clone()], &failing, &now).get("check z").map(String::as_str), Some(unknown));
        assert_eq!(still_failing(&[named.clone(), quiet, old])["check z"].1, unknown);
        // where the first failure has a name, that is who
        assert_eq!(still_failing(&[named])["check z"].1, "rc-late (L) #9");
        assert_eq!(still_since(unknown), unknown);
        assert_eq!(still_since("rc-a (A) #9"), "since rc-a (A) #9");
    }

    /// A failing unit is blamed by test: each failing test names who changed its unit in the first
    /// run where it failed after a run where it passed, or that nobody knows, naming no landing. A
    /// pass of another target of its package is no pass of the test (one package's tests, failing
    /// since before, named a devhub-only landing; another's two landings that changed only its
    /// tests' support, 11-07).
    #[test]
    fn a_failing_test_names_who_changed_its_unit_where_it_first_failed() {
        let log = "GATEUNIT ok test p --test other\ntest p --lib: FAILED (exit 101; 7 passed, 2 failed)\nFAILED live::tests::a\n  panicked at p/src/live.rs:1:2 -- no\nFAILED live::tests::b\nrelax test: 0 warnings\ntest result: FAILED. 7 passed; 2 failed\nGATEUNIT fail test p --lib\nerror: could not compile\nGATEUNIT fail test q\n";
        let (failed, ok) = test_lines(log);
        assert_eq!(failed, BTreeMap::from([("test p --lib".to_string(), vec!["live::tests::a".to_string(), "live::tests::b".to_string()])]), "a unit that ran no test names none");
        assert_eq!(panic_lines(log), BTreeMap::from([("test p --lib".to_string(), vec!["panicked at p/src/live.rs:1:2 -- no".to_string()])]), "a failing unit keeps its tests' panic lines");
        assert_eq!(ok, BTreeSet::from(["test p --test other".to_string()]));
        let row = |log: &str, failed: &[(&str, &[&str])], ok: &[&str], changed: &[(&str, &str)]| obj(vec![
            ("failing", Value::Arr(failed.iter().map(|(u, _)| s(*u)).collect())),
            ("log", s(&format!("/h/bucket-logs/{log}"))),
            ("tests_failed", Value::Obj(failed.iter().map(|(u, t)| (u.to_string(), Value::Arr(t.iter().map(|t| s(*t)).collect()))).collect())),
            ("ran_ok", Value::Arr(ok.iter().map(|u| s(*u)).collect())),
            ("changed", Value::Obj(changed.iter().map(|(u, w)| (u.to_string(), s(*w))).collect())),
        ]);
        let unit = "test p --lib";
        let (a, b) = ("live::tests::a", "live::tests::b");
        let now = row("tests-4.log", &[(unit, &[a, b][..])], &["test p --test other"], &[(unit, "rc-late (L) #4")]);
        // b failed first in run 3, where a passed; run 2's pass of a sibling target looked at neither
        let three = row("tests-3.log", &[(unit, &[b][..])], &[], &[(unit, "rc-b (B) #3")]);
        let two = row("tests-2.log", &[], &["test p --test other"], &[]);
        let one = row("tests-1.log", &[], &[unit], &[]);
        let rows = [now.clone(), three.clone(), two, one.clone()];
        assert_eq!(test_blame(&rows, unit, b), "rc-b (B) #3");
        assert_eq!(test_blame(&rows, unit, a), "rc-late (L) #4", "a failed first now");
        assert_eq!(tests_blame(&rows, &[(unit.to_string(), vec![a.to_string(), b.to_string()])], &|_, _| None), "by test: live::tests::a since rc-late (L) #4; live::tests::b since rc-b (B) #3");
        // a whole package's pass is a pass of each of its targets' tests
        assert_eq!(test_blame(&[three.clone(), row("tests-2.log", &[], &["test p"], &[])], unit, b), "rc-b (B) #3");
        // no pass among the runs that named their tests: nobody knows
        let old = obj(vec![("failing", Value::Arr(vec![s(unit)])), ("log", s("/h/bucket-logs/tests-0.log"))]);
        assert_eq!(test_blame(&[now.clone(), three.clone(), old], unit, b), "origin unknown: no green baseline, it failed in every run back to tests-3.log, and the log of the run before (tests-0.log is gone) is not there to read its tests from");
        assert_eq!(test_blame(&[now.clone(), three.clone()], unit, b), "origin unknown: no green baseline, it failed in every run the ledger keeps, the first tests-3.log");
        // its first failing run changed nothing of its unit: no landing is named
        let quiet = row("tests-3.log", &[(unit, &[b][..])], &[], &[]);
        assert_eq!(test_blame(&[now.clone(), quiet, one.clone()], unit, b), "origin unknown: no landing of its first failing run (tests-3.log) changed test p --lib");
        // red, then a pass at a later tip, then red again: off and on, laid to no landing; a test
        // that never failed before that pass names its landing still
        let flaps = [now.clone(), row("tests-5.log", &[], &[unit], &[]), three.clone(), one.clone()];
        let off_and_on = "intermittent, origin unknown: it failed in tests-3.log, passed in the later tests-5.log, and fails again";
        assert_eq!(test_blame(&flaps, unit, b), off_and_on);
        assert_eq!(test_blame(&flaps, unit, a), "rc-late (L) #4");
        assert_eq!(tests_blame(&flaps, &[(unit.to_string(), vec![a.to_string(), b.to_string()])], &|_, _| None), format!("by test: live::tests::a since rc-late (L) #4; live::tests::b {off_and_on}"));
        // two breaks of one test with a pass between are no flapping: their panics share nothing,
        // so the newer is laid to its run's landings (an added lease field's round trip failed for
        // `untimed`, passed for a day, then failed for `lost_on`, and read as intermittent, 11-25);
        // the same panic, its numbers aside, is the same failure off and on
        let panicked = |r: &Value, line: &str| {
            let mut r = r.clone();
            if let Value::Obj(f) = &mut r { f.push(("panics".into(), obj(vec![(unit, Value::Arr(vec![s(line)]))]))); }
            r
        };
        let passed = row("tests-5.log", &[], &[unit], &[]);
        let two_breaks = [panicked(&now, "panicked at p/src/live.rs:90:2 -- field lost_on is written"), passed.clone(), panicked(&three, "panicked at p/src/live.rs:60:2 -- field untimed is written"), one.clone()];
        assert_eq!(test_blame(&two_breaks, unit, b), "rc-late (L) #4");
        let timings = [panicked(&now, "panicked at p/src/live.rs:90:2 -- took 812 ms"), passed.clone(), panicked(&three, "panicked at p/src/live.rs:60:2 -- took 95 ms"), one.clone()];
        assert_eq!(test_blame(&timings, unit, b), off_and_on);
        assert_eq!(test_blame(&[panicked(&now, "panicked at p/src/live.rs:90:2 -- field lost_on is written"), passed, three.clone(), one.clone()], unit, b), off_and_on, "one red kept no panics: nothing tells them apart");
        // `devhub bucket` says the same, past three tests counted
        let many = ["t0", "t1", "t2", "t3", "t4"];
        let wide = row("tests-5.log", &[(unit, &many[..])], &[], &[]);
        let said = "by test: t0, t1, t2 and 2 more origin unknown: no landing of its first failing run (tests-5.log) changed test p --lib";
        assert_eq!(still_failing(&[wide, one.clone()])["test p"].1, said);
        assert_eq!(still_since(said), said);
        // a newer run that could not build its tests names no test: that build is the red, with its
        // first error and who broke it, not a test an older run failed (a test build's E0433 read
        // "by test: requests_round_trip since .." for a test that run passed, 11-24)
        let (errors, _) = unit_lines("test result: ok. 9 passed\nlibs/p/src/disk.rs:12:3: E0433: cannot find `tree` in `super`\nerror: could not compile `p` (test) due to 1 previous error: no test ran\nGATEUNIT fail test p --lib\n");
        assert_eq!(errors[unit], ["disk.rs: E0433: cannot find `tree` in `super`", "could not compile `p` (test)"], "its build after its errors");
        let built = |log: &str, broke: &str| obj(vec![("failing", Value::Arr(vec![s(unit)])), ("log", s(&format!("/h/bucket-logs/{log}"))),
            ("errors", Value::Obj(vec![(unit.to_string(), Value::Arr(errors[unit].iter().map(|e| s(e)).collect()))])), ("broke", obj(vec![("test p", s(broke))]))]);
        let red = "compile red: `p` (test): disk.rs: E0433: cannot find `tree` in `super` since rc-c (C) #5";
        assert_eq!(still_failing(&[built("tests-5.log", "rc-c (C) #5"), now.clone(), three.clone(), one.clone()])["test p"].1, red);
        // still red at the next run, which carried that name: the build's first breaker
        assert_eq!(still_failing(&[built("tests-6.log", red), built("tests-5.log", "rc-c (C) #5"), now.clone()])["test p"].1, red);
        assert_eq!(still_since(red), red);
        // its errors the same as a row's from before builds were kept: the same break, carried
        let older = obj(vec![("failing", Value::Arr(vec![s(unit)])), ("errors", Value::Obj(vec![(unit.to_string(), Value::Arr(vec![s(&errors[unit][0])]))])), ("broke", obj(vec![("test p", s("rc-c (C) #5"))]))]);
        assert_eq!(carried_since(&[older], &[unit.to_string()], &errors).get(unit).map(String::as_str), Some("rc-c (C) #5"));
    }

    /// A row from before rows named their tests reads its kept log for them, and its landings'
    /// units for who changed each (none where its ambiguous run spread a unit over every landing):
    /// the first run that named its tests blamed all nine red ones `origin unknown` (11-07).
    #[test]
    fn a_row_from_before_tests_were_named_reads_its_kept_log() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-test-named-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let unit = "test p --lib";
        let log = |name: &str, text: &str| { let at = dir.join(name); std::fs::write(&at, text).unwrap(); at.display().to_string() };
        let landing = |id: i64, units: &[&str]| obj(vec![("id", Value::Int(id)), ("label", s("rc-x+rc-x")), ("agent", s("X")), ("units", Value::Arr(units.iter().map(|u| s(*u)).collect()))]);
        let old = |log: String, landings: Vec<Value>, ambiguous: bool| obj(vec![("failing", Value::Arr(vec![s(unit)])), ("log", s(&log)), ("landings", Value::Arr(landings)), ("ambiguous", Value::Bool(ambiguous))]);
        let failed = log("tests-2.log", "test p --lib: FAILED (exit 101)\nFAILED t::b\nGATEUNIT fail test p --lib\n");
        let passed = log("tests-1.log", "GATEUNIT ok test p --lib\n");
        let two = named_row(&old(failed.clone(), vec![landing(7, &[unit]), landing(8, &[])], false));
        assert_eq!(looked(&two, unit, "t::b"), Some(Look::Failed(unit.to_string())));
        assert_eq!(two.get("changed").and_then(|c| c.get(unit)).and_then(Value::as_str), Some("rc-x+rc-x (X) #7"));
        let one = named_row(&obj(vec![("log", s(&passed)), ("landings", Value::Arr(Vec::new()))]));
        assert_eq!(looked(&one, unit, "t::b"), Some(Look::Passed));
        let now = obj(vec![("log", s("/h/tests-3.log")), ("tests_failed", Value::Obj(vec![(unit.to_string(), Value::Arr(vec![s("t::b")]))])), ("ran_ok", Value::Arr(Vec::new())), ("changed", Value::Obj(Vec::new()))]);
        assert_eq!(test_blame(&[now.clone(), two, one.clone()], unit, "t::b"), "rc-x+rc-x (X) #7");
        // spread over every landing by an ambiguous run: no landing changed it
        let spread = named_row(&old(failed, vec![landing(7, &[unit]), landing(8, &[unit])], true));
        assert!(spread.get("changed").and_then(|c| c.get(unit)).is_none());
        assert_eq!(test_blame(&[now, spread, one], unit, "t::b"), "origin unknown: no landing of its first failing run (tests-2.log) changed test p --lib");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tests run's FAILED line blames a unit that names its failing tests by test, and its row
    /// keeps what the next runs read back.
    #[test]
    fn a_unit_that_names_its_failing_tests_is_blamed_by_test() {
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-bytest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let store = Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let tip = Pair::new("a1", "b1");
        let judged = |n: u32, text: &str| {
            let log = home.join(format!("tests-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(if text.contains("NODEGATE-OK") { 0 } else { 1 })), text, &log);
            judge("tests", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
            let red: Vec<String> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Err).map(|r| r.line).collect();
            (log.display().to_string(), red)
        };
        let (_, green) = judged(1, "GATEUNIT ok test p --lib\nNODEGATE-OK\n");
        assert!(green.is_empty(), "{green:?}");
        let (log, red) = judged(2, "test p --lib: FAILED (exit 101; 1 passed, 1 failed)\nFAILED t::a\nGATEUNIT fail test p --lib\nNODEGATE-FAIL: tests\n");
        assert_eq!(red.len(), 1, "{red:?}");
        assert!(red[0].contains(&format!("FAILED test p --lib: test p failing by test: t::a origin unknown: no landing of its first failing run (tests-2.log) changed test p --lib (log {log})")), "{}", red[0]);
        let row = rows_of(&home, "tests").into_iter().next().unwrap();
        assert_eq!(looked(&row, "test p --lib", "t::a"), Some(Look::Failed("test p --lib".to_string())));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A run whose gate stopped before any unit (the mac gate on a box with no JIT) is red and keeps
    /// its bucket's base: the next run reads the landings' reach from that base and covers the units
    /// the stopped run never reached. A run that reached its units moves the base to its tip, red or
    /// green. Two Macs' runs stopped at their JIT, the next diffed from their tip, found 0 units and
    /// said ok (11-17).
    #[test]
    fn a_run_stopped_before_its_units_keeps_the_base_for_the_next() {
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-stopped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open(&home, "mac", Arc::new(|_: Record| 0)).unwrap();
        let pin = |c: &str| pin_of(&Pair::new(&c.repeat(40), &"f".repeat(40)));
        let (b0, t1, t2, t3) = (pin("a"), pin("b"), pin("c"), pin("d"));
        let judged = |n: u32, base: &Pin, tip: &Pin, text: &str| -> (Pin, Value) {
            let log = home.join(format!("mac-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(if text.contains("NODEGATE-OK") { 0 } else { 1 })), text, &log);
            let mut ran_from = None;
            judge("mac", &home, &home, &store, &|_, _, _| {}, 1, Some(base.clone()), tip, None, |from| { ran_from = from.cloned(); Ok(outcome) }).unwrap();
            assert_eq!(ran_from.as_ref(), Some(base), "run {n} reads the landings' reach from its base");
            let row = rows_of(&home, "mac").into_iter().next().unwrap();
            (next_base(&row).unwrap(), row)
        };
        let unit = |end: &str| format!("mac gate: the JIT /j\nmac gate: 1 unit(s) the landings reach\nGATESTART test p 1\nGATEEND test p\n{end}");
        // its gate stopped at the JIT: red, and the base stays
        let (next, row) = judged(1, &b0, &t1, "mac gate: this box named no JIT ($MAKEPAD_LOADER '')\nGATESTEP fail jit\nNODEGATE-FAIL: jit [lease L1]\n");
        assert_eq!((next, row.get("stopped").and_then(Value::as_str), row.get("ok").and_then(Value::as_bool)), (b0.clone(), Some("jit"), Some(false)));
        // the next run covers the stopped run's landings, and moves the base to its tip
        let (next, row) = judged(2, &b0, &t2, &unit("test p: ok (1 passed, 0 failed, 3 s)\nGATEUNIT ok test p\nNODEGATE-OK mac 1 unit(s) [lease L2]\n"));
        assert_eq!((next, row.get("stopped").cloned(), row.get("ok").and_then(Value::as_bool)), (t2.clone(), Some(Value::Null), Some(true)));
        // a red run that reached its units moves it too
        let (next, _) = judged(3, &t2, &t3, &unit("test p: FAILED (exit 101, 0 passed, 1 failed)\nGATEUNIT fail test p\nNODEGATE-FAIL: test p [lease L3]\n"));
        assert_eq!(next, t3);
        // A red its gate ran once more on its box at its tip, green there, is a flake: the run is
        // ok and names nobody, and the flake list keeps it with both runs' outputs. The same test's
        // second flake within seven days goes to main as a bug with both logs, once in the span. A
        // Fab test red once was laid to the last landing of its batch, which could not reach it
        // (11-23); main's rule (11-24).
        let flake = |n: u32| format!("L{n} box-a: sources at work@abc\n{}", unit(&format!("test p: red (exit 101; 0 passed, 1 failed): it runs once more on this box at this tip before anyone is named\n  its first run: FAILED t::a\n\
            test p: a flake: red (exit 101), then green on its rerun (1 passed, 0 failed)\nGATEOUT test p :: mac-unit-1.log mac-unit-1-rerun.log\nGATEFLAKE test p :: t::a \n\
            test p: ok (1 passed, 0 failed, 3 s)\nGATEUNIT ok test p\nNODEGATE-OK mac 1 unit(s) [lease L{n}]\n")));
        let main_heard = || crate::notes::take(&home, "main").into_iter().map(|(_, line)| line).filter(|l| l.contains("flaky")).collect::<Vec<_>>();
        let (t4, t5, t6) = (pin("e"), pin("1"), pin("2"));
        let (_, row) = judged(4, &t3, &t4, &flake(4));
        assert_eq!((row.get("ok").and_then(Value::as_bool), row.get("failing").and_then(Value::as_arr).map(|f| f.len())), (Some(true), Some(0)), "a flake is no red: {row:?}");
        assert!(row.get("summary").and_then(Value::as_str).unwrap().contains("test p: a flake, red (t::a), then green on its rerun at this tip"), "{row:?}");
        let listed: Vec<Value> = std::fs::read_to_string(home.join(FLAKES)).unwrap().lines().map(|l| makepad_strict_json::parse(l.as_bytes()).unwrap()).collect();
        assert_eq!(listed.len(), 1, "the flake list keeps it");
        assert_eq!(listed[0].get("tests").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).collect::<Vec<_>>(), ["t::a"], "by its failing test");
        let logs = listed[0].get("logs").and_then(Value::as_arr).unwrap_or_default().iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        assert!(logs.len() == 2 && logs[0].ends_with("mac-4.log") && logs[1] == "both runs' output: devhub job result L4", "its run's log and both runs' kept output: {logs:?}");
        assert!(main_heard().is_empty(), "one flake is a row on the list");
        judged(5, &t4, &t5, &flake(5));
        let heard = main_heard();
        assert!(heard.len() == 1 && heard[0].starts_with("flaky 2 times in 7 days: test p t::a: a bug for its owner, not a landing's red")
            && ["mac-4.log", "mac-5.log", "devhub job result L4", "devhub job result L5"].iter().all(|l| heard[0].contains(l)), "twice in seven days: main hears it with both logs: {heard:?}");
        judged(6, &t5, &t6, &flake(6));
        assert!(main_heard().is_empty(), "once in the span");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A unit still failing names the log of its last look, in the bucket's FAILED line and in
    /// `devhub bucket`'s STILL FAILING: a kept log holding only a green present of the unit, beside
    /// a red bucket, read as a miscount (11-05).
    #[test]
    fn a_still_failing_unit_names_the_log_of_its_last_look() {
        use crate::proto::Record;
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let store = Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let tip = Pair::new("a1", "b1");
        let judged = |n: u32, text: &str| {
            let log = home.join(format!("fast-{n}.log"));
            std::fs::write(&log, text).unwrap();
            let outcome = verdict(Ok(Some(if text.contains("NODEGATE-OK") { 0 } else { 1 })), text, &log);
            judge("fast", &home, &home, &store, &|sev, line, detail| store.event("bucket", sev, line, Some(detail)), 1, None, &pin_of(&tip), None, |_| Ok(outcome)).unwrap();
            let red: Vec<String> = fed.lock().unwrap().drain(..).filter(|r| r.kind == "bucket" && r.sev == Sev::Err).map(|r| r.line).collect();
            (log.display().to_string(), red)
        };
        let failed = "/n/tree/a/src/s.rs:134:12: warning: method `s` is never used [dead_code]\nf: 1 warnings\nGATEUNIT fail check f\nNODEGATE-FAIL: f-warnings\n";
        let (first, _) = judged(1, failed);
        // a run that did not look at it: red, with the log of the run that did
        let (_, quiet) = judged(2, "GATEUNIT ok check c\nNODEGATE-OK\n");
        assert_eq!(quiet.len(), 1, "{quiet:?}");
        assert!(quiet[0].contains(&format!("check f still failing origin unknown, first failed in fast-1.log (log {first})")), "{}", quiet[0]);
        // failing again: this run's log
        let (third, again) = judged(3, failed);
        assert!(again[0].contains(&format!("check f still failing origin unknown, first failed in fast-1.log (log {third})")), "{}", again[0]);
        let status = status(&home, &[]);
        let still = status.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find(|v| v.get("still_failing").is_some()).expect("a STILL FAILING line");
        assert_eq!(still.get("logs").and_then(|l| l.get("check f")).and_then(Value::as_str), Some(third.as_str()));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The full bucket starts early once no lane's ask queued a minute for its quiet time, 12 h
    /// after its last start at the soonest; with no lease master to ask, only by its hour.
    #[test]
    fn a_quiet_queue_starts_the_full_bucket_early_twice_a_day_at_most() {
        let (min, h) = (60_000u64, 3_600_000u64);
        let full = Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO, daily: Some(17), quiet_queue: Some(Duration::from_secs(20 * 60)) };
        let now = 30 * h;
        assert!(quiet_queue_due(&full, now - 13 * h, now, Some(now - 25 * min)), "quiet 25 min, its last run 13 h ago");
        assert!(!quiet_queue_due(&full, now - 13 * h, now, Some(now - 5 * min)), "an ask queued a minute 5 min ago");
        assert!(!quiet_queue_due(&full, now - 6 * h, now, Some(now - 3 * h)), "its last run 6 h ago");
        assert!(!quiet_queue_due(&full, now - 13 * h, now, None), "no lease master");
        assert!(!quiet_queue_due(&Timing::pipelined(), 0, now, Some(0)), "a bucket with no quiet_queue");
    }

    /// `devhub bucket` says when each bucket's last run ended and how, and a red closes when a later
    /// run of another bucket passed that unit: the full bucket's red of a unit the tests bucket
    /// passed hours later read as a current break (11-16), and stayed open past its fix (11-17).
    /// A red's note waiting for a lane is not said once a later run passed its units (11-17).
    #[test]
    fn a_red_another_bucket_passed_since_is_closed_and_each_bucket_says_its_last_run() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-test-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let row = |bucket: &str, started: u64, failing: &[&str], passed: &[&str]| {
            let log = home.join(format!("{bucket}-{started}.log")).display().to_string();
            let units = |u: &[&str]| Value::Arr(u.iter().map(|u| s(*u)).collect());
            append(&home, obj(vec![("bucket", s(bucket)), ("started_ms", Value::Int(started as i64)), ("ended_ms", Value::Int(started as i64 + 5)), ("ok", Value::Bool(failing.is_empty())),
                ("failing", units(failing)), ("passed", units(passed)), ("log", s(&log))])).unwrap();
        };
        row("full", 100, &["tests pkg-a", "tests pkg-b"], &[]);
        row("tests", 50, &[], &["tests pkg-b"]);
        row("tests", 200, &[], &["tests pkg-a"]);
        row("tests", 300, &[], &["tests pkg-a"]);
        // landings: pkg-b's before the red's run, then after it (twice), and one of another package
        let landing = |id: u64, at: u64, touched: &[&str]| {
            let mut e = Entry::parse(&format!("{{\"id\":{id},\"branch\":\"rc-l{id}\",\"state\":\"landed\",\"changed_ms\":{at}}}")).expect("a landing");
            e.touched = touched.iter().map(|p| p.to_string()).collect();
            e
        };
        let landed = [landing(1, 90, &["pkg-b"]), landing(2, 120, &["pkg-c", "pkg-b"]), landing(3, 150, &["pkg-b"]), landing(4, 160, &["pkg-c"])];
        let lines: Vec<Value> = status(&home, &landed).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
        let still = lines.iter().find(|v| v.get("still_failing").and_then(Value::as_str) == Some(FULL)).expect("the full bucket's reds");
        let units = still.get("units").expect("its open reds");
        assert!(units.get("tests pkg-a").is_none(), "the tests runs after it passed it: closed");
        assert!(units.get("tests pkg-b").is_some(), "a pass before the red is no pass of it: open");
        assert!(still.get("stale").is_none());
        let all: Vec<Value> = ledger(&home, 100).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
        assert_eq!(super::passed_after(&all, FULL, "tests pkg-a", 100).as_deref(), Some("tests"), "a later run of another bucket");
        assert_eq!(super::passed_after(&all, "tests", "tests pkg-a", 100), None, "never the bucket's own");
        assert_eq!(super::passed_after(&all, FULL, "tests pkg-b", 100), None, "a pass before the red's run");
        // the breakers of a red start after the newest pass of any bucket before the failing run
        assert_eq!(super::passed_before(&all, "tests pkg-a", 250).map(|p| p.makepad), None, "these rows name no tip: none to start from");
        let sha = |c: char| c.to_string().repeat(40);
        let tipped = |bucket: &str, started: u64, passed: &[&str], work: char| obj(vec![("bucket", s(bucket)), ("started_ms", Value::Int(started as i64)),
            ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())), ("tip", obj(vec![("work", s(&sha(work))), ("main", s(&sha('e')))]))]);
        let rows = [tipped("full", 400, &[], '4'), tipped("hot", 350, &["tests pkg-a"], '5'), tipped("tests", 300, &["tests pkg-a"], '3'), tipped("fast", 200, &["tests pkg-a"], '2'), tipped("tests", 100, &["tests pkg-b"], '1')];
        assert_eq!(super::passed_after(&rows, FULL, "tests pkg-a", 320), None, "the hot gate's run is a hand-in's merge, no landed tip");
        assert_eq!(super::passed_before(&rows, "tests pkg-a", 400).map(|p| p.makepad), Some(sha('3')), "the newest pass before the run, of any bucket but the hot gate");
        assert_eq!(super::passed_before(&rows, "tests pkg-a", 250).map(|p| p.makepad), Some(sha('2')), "never a run that started after it");
        assert_eq!(super::passed_before(&rows, "tests pkg-c", 400), None, "a unit no run passed");
        // a red's waiting note is unsaid once later runs passed each of its units (notes.rs take_open)
        let keys = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<String>>();
        let recent = recent_rows(&home);
        assert!(super::reds_passed_since(&recent, 100, &keys(&["tests pkg-a"])), "the tests runs after the red passed it");
        assert!(!super::reds_passed_since(&recent, 100, &keys(&["tests pkg-a", "tests pkg-b"])), "pkg-b passed only before the red: still said");
        let mut tipped_lines: Vec<String> = rows.iter().map(|r| r.to_json()).collect();
        assert!(super::reds_passed_since(&tipped_lines, 250, &keys(&["tests pkg-a"])), "a later run of any bucket");
        assert!(!super::reds_passed_since(&tipped_lines, 320, &keys(&["tests pkg-a"])), "the hot gate's pass is a hand-in's merge");
        tipped_lines.insert(0, obj(vec![("bucket", s("fast")), ("started_ms", Value::Int(500)), ("failing", Value::Arr(vec![s("tests pkg-a")]))]).to_json());
        assert!(!super::reds_passed_since(&tipped_lines, 250, &keys(&["tests pkg-a"])), "the newest look at it failed it again: still said");
        let changed = still.get("changed").expect("a red whose package changed since");
        let b = changed.get("tests pkg-b").expect("pkg-b changed after the red's run");
        assert_eq!((b.get("first").and_then(Value::as_str), b.get("first_ms").and_then(Value::as_u64), b.get("n").and_then(Value::as_u64)), (Some("#2 rc-l2"), Some(120), Some(2)), "the first landing after the run, and the count: not the one before it");
        assert!(changed.get("tests pkg-a").is_none(), "no landing touched pkg-a");
        assert_eq!((few("a, b, c, d, e", 3), few("a, b, c, d", 3)), ("a, b, c and 2 more".to_string(), "a, b, c, d".to_string()), "a long origin is cut; one item more is kept");
        let last = lines.iter().find_map(|v| v.get("last_runs")).expect("the last runs");
        assert_eq!((last.get(FULL).and_then(|r| r.get("ended_ms")).and_then(Value::as_u64), last.get(FULL).and_then(|r| r.get("ok")).and_then(Value::as_bool)), (Some(105), Some(false)));
        assert_eq!(last.get("tests").and_then(|r| r.get("ended_ms")).and_then(Value::as_u64), Some(305), "its newest run");
        assert!(last.get("ci").is_none(), "a bucket with no run says none");
        // a failed last run says what became of each of its reds since, closed ones too: the full
        // bucket's red the tests bucket passed after its fix read as a bare "FAILED" (11-21)
        let reds = last.get(FULL).and_then(|r| r.get("reds")).expect("the failed run's reds since");
        let a = reds.get("tests pkg-a").expect("pkg-a passed since");
        assert_eq!((a.get("passed").and_then(Value::as_str), a.get("passed_ms").and_then(Value::as_u64)), (Some("tests"), Some(305)), "the later pass of another bucket, and when it ended");
        assert!(a.get("changed").is_none(), "no landing touched pkg-a");
        let b = reds.get("tests pkg-b").expect("pkg-b changed since");
        assert!(b.get("passed").is_none(), "a pass before the red is no pass of it");
        assert_eq!(b.get("changed").and_then(|c| c.get("first")).and_then(Value::as_str), Some("#2 rc-l2"), "the first landing after the run");
        assert!(last.get("tests").and_then(|r| r.get("reds")).is_none(), "a run that passed has none");
        // a run with no verdict writes no row: its bucket's last run says it beside its last verdict
        // (CI's 02:00 run had none, and `devhub bucket` said "ci 27h ago", 11-20); one older than
        // the bucket's newest row says nothing
        note_no_verdict(&home, "tests", 400, "1 of 3 shards have no verdict", "tests-400.log");
        note_no_verdict(&home, FULL, 50, "an older one", "full-50.log");
        note_no_verdict(&home, "ci", 10, "its legs did not run", "ci-10.log");
        let lines: Vec<Value> = status(&home, &landed).iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
        let last = lines.iter().find_map(|v| v.get("last_runs")).expect("the last runs");
        let none = |name: &str| last.get(name).and_then(|r| r.get("no_verdict")).and_then(|n| n.get("why")).and_then(Value::as_str).map(str::to_string);
        assert_eq!((none("tests").as_deref(), last.get("tests").and_then(|r| r.get("ended_ms")).and_then(Value::as_u64)), (Some("1 of 3 shards have no verdict"), Some(305)), "and its last verdict's run");
        assert_eq!(none(FULL), None, "older than its newest row");
        assert!(none("ci").is_some() && last.get("ci").and_then(|r| r.get("ok")).is_none(), "a bucket whose only run had no verdict says that one: {last:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bucket_starts_from_its_last_start_and_landings_and_the_cap_is_a_cap() {
        let (min, h) = (60_000u64, 3_600_000u64);
        let ci = Timing::after(Duration::from_secs(15 * 60), Duration::from_secs(3 * 3600));
        // last run at 0, a landing every 10 min from 10 min on: never 15 min quiet, so it starts
        // 3 h after that first landing (a master restart changes none of the inputs)
        let first_start = (10 * min..5 * h).step_by(min as usize).find(|now| *now >= starts_at(&ci, 0, 10 * min, now / (10 * min) * (10 * min)));
        assert_eq!(first_start, Some(10 * min + 3 * h));
        // quiet for 15 min: then
        assert_eq!(starts_at(&ci, 0, 10 * min, 40 * min), 55 * min);
        // a run with no ledger row and no run of its kind known: due since its first landing, long
        // past, so now
        assert!(starts_at(&ci, 0, 1, 1_000 * min) <= 3 * h + 1);
        let hot = Timing::after(Duration::ZERO, Duration::ZERO);
        assert_eq!(starts_at(&hot, 5, 9, 9), 9, "hot: at the landing");
        let tests = Timing::after(Duration::from_secs(60), Duration::from_secs(600));
        assert_eq!(starts_at(&tests, 0, 100, 100), 100 + min, "a minute after the last landing");
        assert_eq!(starts_at(&tests, 0, 100, 100 + 20 * min), 100 + 10 * min, "at most 10 min after the first");
        let fast = Timing::pipelined();
        assert_eq!(starts_at(&fast, 0, 100, 100 + 20 * min), 100, "at the first landing it has not checked, quiet or not");
        let every = Timing { every: Duration::from_secs(3600), quiet: Duration::ZERO, cap: Duration::ZERO, daily: None, quiet_queue: None };
        assert_eq!(starts_at(&every, 1_000, 2_000, 2_000), 1_000 + h, "not before every after its last start");
        assert_eq!(log_started(Path::new("/x/bucket-logs/ci-1793673952236.log")), Some(1793673952236));
    }

    /// A steady stream of landings 30 s apart never starves the pipelined fast bucket: each run
    /// starts as the one before ends, of the newest tip, so a landing is looked at by the run
    /// after the one in flight when it landed. Waiting for a quiet minute (at most 10) left the
    /// box idle 8 min a run under such a stream, a Mac show held all the while (11-07).
    #[test]
    fn a_pipelined_bucket_runs_back_to_back_through_a_landing_stream() {
        let (sec, run) = (1_000u64, 150_000u64);
        let landed: Vec<u64> = (1..=40).map(|i| i * 30 * sec).collect();
        // the spawn loop's decision each poll (1 s), a run taking `run`: (start, end, landings looked at)
        let runs_of = |timing: &Timing| {
            let (mut last_start, mut busy_until, mut looked) = (0u64, 0u64, 0usize);
            let mut runs = Vec::new();
            for now in (0..landed[39] + 3 * run).step_by(sec as usize) {
                let tip = landed.iter().filter(|ms| **ms <= now).count();
                if now < busy_until || tip == looked {
                    continue;
                }
                let first = landed.iter().copied().filter(|ms| *ms > last_start).min().unwrap_or(now);
                if now < starts_at(timing, last_start, first, landed[tip - 1]) {
                    continue;
                }
                (last_start, busy_until, looked) = (now, now + run, tip);
                runs.push((now, now + run, tip));
            }
            runs
        };
        let runs = runs_of(&Timing::pipelined());
        assert_eq!(runs[0].0, landed[0], "the first landing starts a run at once");
        for w in runs.windows(2) {
            assert_eq!(w[1].0, w[0].1, "the next run starts as one ends");
        }
        for (i, ms) in landed.iter().enumerate() {
            let end = runs.iter().find(|r| r.2 > i).map(|r| r.1).unwrap();
            assert!(end <= ms + 2 * run, "landing {i} at {ms} is looked at by {end}");
        }
        assert_eq!(runs.last().map(|r| r.2), Some(landed.len()), "the stream's last tip");
        let quiet = runs_of(&Timing::after(Duration::from_secs(60), Duration::from_secs(600)));
        assert!(quiet.windows(2).all(|w| w[1].0 - w[0].1 >= 7 * 60 * sec), "the quiet minute never came: {quiet:?}");
    }

    /// The night's run: at 02:00 local after the first landing it has not checked, once a day
    /// however many landings follow, whatever the zone and its daylight saving.
    #[test]
    fn the_nightly_run_starts_at_two_after_a_landing_once_a_day() {
        let (min, h, day) = (60_000u64, 3_600_000u64, 86_400_000u64);
        let cet = |_: i64| 3600;
        // 2026-11-05 00:00 UTC is 01:00 CET
        let midnight = 1_793_836_800_000u64;
        assert_eq!(daily_after(midnight, 2, &cet), midnight + h, "02:00 CET the same night");
        assert_eq!(daily_after(midnight + h, 2, &cet), midnight + h + day, "at 02:00 itself: the next night");
        assert_eq!(daily_after(midnight + 9 * h, 2, &cet), midnight + h + day, "a landing at 10:00: the next night");
        assert_eq!(daily_after(midnight + 23 * h, 2, &|_| 0), midnight + day + 2 * h, "in UTC");
        // a zone that leaves summer time before the night's 02:00: UTC+2 at the landing, UTC+1 then
        let dst = |secs: i64| if secs < (midnight / 1000) as i64 - 3600 { 7200 } else { 3600 };
        assert_eq!(daily_after(midnight - 6 * h, 2, &dst), midnight + h, "02:00 on the clock after the change");
        let nightly = Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO, daily: Some(2), quiet_queue: None };
        let at = starts_at(&nightly, midnight - day, midnight + 10 * min, midnight + 50 * min);
        assert_eq!(at % (h / 60), 0, "on the minute");
        assert!(at > midnight + 10 * min && at <= midnight + 10 * min + day, "within a day of the landing");
        assert_eq!(starts_at(&nightly, at, at + min, at + min), daily_after(at + min, 2, &|s| makepad_civil_time::local_or_utc(s).offset), "the next night, not again at once");
        assert_eq!(log_started(Path::new("/x/ci.log")), None);
    }

    /// A failed unit's output is kept under the id of the lease whose part of the log failed it, from
    /// the newest of the keep's run dirs, once however often the run ends; a file the keep lacks is
    /// said so, not skipped.
    #[test]
    fn a_failed_units_output_is_kept_under_its_lease() {
        let dir = std::env::temp_dir().join(format!("devhub-bucket-keep-units-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (keep, logs) = (dir.join("tests-s1-5.keep"), dir.join("lease-logs"));
        let put = |run: &str, file: &str, text: &str, secs: u64| {
            std::fs::create_dir_all(keep.join(run)).unwrap();
            std::fs::write(keep.join(run).join(file), text).unwrap();
            std::fs::File::options().write(true).open(keep.join(run).join(file)).unwrap().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
        };
        put("landgate.lost", "testrun-x---bin-x", "the lost lease's run\n", 100);
        put("landgate.last", "testrun-x---bin-x", "running 3 tests\nmalloc(): unaligned tcache chunk detected\n", 200);
        put("landgate.last", "testrun-y", "test y::a ... FAILED\n", 200);
        let text = "L7 test: 1st in queue (box-c: budget cores 16)\nL7 box-a: sources at work@0123456789\nGATEOUT test x --bin x :: testrun-x---bin-x\nGATEUNIT fail test x --bin x\nL8 box-b: sources at work@0123456789\nGATEOUT test y :: testrun-y testrun-y.hung\nGATEUNIT fail test y\n";
        keep_failed_units(text, &keep, &logs).unwrap();
        keep_failed_units(text, &keep, &logs).unwrap();
        let x = keep.join("landgate.last").join("testrun-x---bin-x");
        assert_eq!(std::fs::read_to_string(logs.join("L7.log")).unwrap(), format!("== test x --bin x failed: its testrun-x---bin-x, the last 64 KB ({})\nrunning 3 tests\nmalloc(): unaligned tcache chunk detected\n", x.display()), "the unit's output, from the newest run dir, under the lease that failed it, once");
        let l8 = std::fs::read_to_string(logs.join("L8.log")).unwrap();
        assert!(l8.contains("test y::a ... FAILED") && !l8.contains("malloc"), "each lease keeps its own units' output: {l8}");
        assert!(l8.contains(&format!("== test y failed: its testrun-y.hung was not kept in {}\n", keep.display())), "a file the keep lacks is said: {l8}");
        assert_eq!(l8.matches("== ").count(), 2, "the second end adds nothing: {l8}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed run's JITs are kept on the boxes its log says ran them: of a run of several leases
    /// (a tests run's shards) only the failed leases' JITs, each with the binary its part names. They
    /// stay while the failure shows in any of the bucket's newest [`KEEP_RUNS`] runs; then they go. A
    /// box that cannot release one keeps it on the list, and the next run asks again.
    #[test]
    fn a_failed_runs_jit_stays_until_its_failure_is_gone_from_the_newest_runs() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-jit-keeps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bucket-logs")).unwrap();
        let (k1, k2) = ("c0da2f549bbc38d03e1c375b", "0123456789abcdef01234567");
        let text = format!("L7 test: 1st in queue (box-c: budget cores 16)\nL7 box-a: sources at work@0123456789\njit: kept {k1}\njit: kept {k1}\nGATEPEAKBY test x :: 1.0 GB /t/host/makepad.jit-{k1} run --harness\nL8 box-b: sources at work@0123456789\nbuilt\ndevhub: kept the JIT binary {k2}\nGATEUNIT fail test makepad-fab --bin makepad-fab\n");
        let units = |u: &[&str]| u.iter().map(|u| u.to_string()).collect::<BTreeSet<String>>();
        assert_eq!(run_jits(&text, &units(&["test makepad-fab"])), vec![("box-b".to_string(), k2.to_string(), None)], "the failed lease's JIT alone, on the box it ran on");
        assert_eq!(run_jits(&text, &units(&["suite makepad-csg-brep"])), vec![("box-a".to_string(), k1.to_string(), Some(format!("/t/host/makepad.jit-{k1}"))), ("box-b".to_string(), k2.to_string(), None)], "a log naming none of the failed units: every JIT, once, with its binary where named");
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let refuse = std::cell::Cell::new(false);
        let ask = |node: &str, line: &str| { asked.borrow_mut().push(format!("{node}: {line}")); if refuse.get() { Err("no link".to_string()) } else { Ok(()) } };
        let warned = std::cell::RefCell::new(Vec::<String>::new());
        let emit = |_: Sev, line: String, _: String| warned.borrow_mut().push(line);
        let away = std::cell::RefCell::new(BTreeSet::<String>::new());
        let run = |n: usize, failing: &[&str]| {
            let log = home.join("bucket-logs").join(format!("fast-{n}.log"));
            std::fs::write(&log, format!("L{n} box-a: sources at work@0123456789\njit: kept {}\n", if n == 1 { k1 } else { k2 })).unwrap();
            append(&home, obj(vec![("bucket", s("fast")), ("started_ms", Value::Int(n as i64)), ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("log", s(&log.display().to_string()))])).unwrap();
            asked.borrow_mut().clear();
            keep_failed_jits("fast", &home, &log, &ask, &away.borrow(), &emit);
            asked.borrow().clone()
        };
        assert_eq!(run(1, &["jit-suite:makepad-csg-brep"]), vec![format!("box-a: jit-keep fast-1-c0da2f54 {k1}")], "a failed run's JIT is kept");
        assert!(run(2, &[]).is_empty(), "a green run keeps nothing");
        assert_eq!(run(3, &["jit-suite:makepad-csg-brep"]), vec![format!("box-a: jit-keep fast-3-01234567 {k2}")], "it failed again: that run's JIT too");
        for n in 4..=12 { assert!(run(n, &[]).is_empty(), "run {n}: the failure is among the newest {KEEP_RUNS} runs, so even the first run's JIT stays"); }
        let both = vec!["box-a: jit-unkeep fast-1-c0da2f54".to_string(), "box-a: jit-unkeep fast-3-01234567".to_string()];
        // their box is down (or dev off): they wait for it without a word, and go once it is back
        let nodes = [r#"{"node":"box-a","up":false,"dev":true}"#, r#"{"node":"box-b","up":true,"dev":false}"#, r#"{"node":"box-c","up":true,"dev":true}"#].map(String::from);
        assert_eq!(away_of(&nodes), ["box-a", "box-b"].map(String::from).into_iter().collect::<BTreeSet<_>>(), "a box down or dev off is away");
        away.borrow_mut().insert("box-a".into());
        assert!(run(13, &[]).is_empty() && warned.borrow().is_empty(), "an away box is asked nothing and warned of nothing: {:?}", warned.borrow());
        away.borrow_mut().clear();
        refuse.set(true);
        assert_eq!(run(14, &[]), both, "both go once the newest runs are clean of their failure");
        assert!(warned.borrow().len() == 2 && warned.borrow().iter().all(|w| w.contains("stays: no link")), "{:?}", warned.borrow());
        refuse.set(false);
        assert_eq!(run(15, &[]), both, "what a box did not release is asked again at the next run");
        assert!(std::fs::read_to_string(keeps_file(&home)).unwrap().is_empty(), "nothing kept now");
        // a shard's JIT is held from its end to its run's verdict, and that hold goes after a red
        // run's JIT is kept: never a moment unheld (11-13)
        let log = home.join("bucket-logs").join("fast-16.log");
        std::fs::write(&log, format!("L16 box-a: sources at work@0123456789\njit: kept {k1}\n")).unwrap();
        asked.borrow_mut().clear();
        hold_run_jits(&log, &std::fs::read_to_string(&log).unwrap(), &ask);
        assert_eq!(asked.borrow().clone(), vec![format!("box-a: jit-keep fast-16-run-c0da2f54 {k1}")], "held at its shard's end");
        append(&home, obj(vec![("bucket", s("fast")), ("started_ms", Value::Int(16)), ("failing", Value::Arr(vec![s("jit-suite:makepad-csg-brep")])), ("log", s(&log.display().to_string()))])).unwrap();
        // an older run that never got its verdict still holds its JIT; a newer one runs on
        let logs = home.join("bucket-logs");
        std::fs::write(logs.join("fast-9.log.holds"), "box-b fast-9-run-01234567\n").unwrap();
        std::fs::write(logs.join("fast-20.log.holds"), "box-b fast-20-run-01234567\n").unwrap();
        // a tests run that got no verdict and still holds its JIT is named once it is a day old
        std::fs::write(logs.join("tests-5.log.holds"), "box-c tests-5-run-abcdef01\n").unwrap();
        let day = STALE_HOLDS_MS;
        assert!(stale_holds(&home, 5 + day).is_empty(), "a day old at most: not named");
        let said = stale_holds(&home, 6 + day);
        assert!(said.len() == 1 && said[0].contains("its run tests-5 (24 h ago) got no verdict") && said[0].contains("tests-5-run-abcdef01 on box-c"), "{said:?}");
        asked.borrow_mut().clear();
        keep_failed_jits("fast", &home, &log, &ask, &BTreeSet::new(), &emit);
        assert_eq!(asked.borrow().clone(), vec![format!("box-a: jit-keep fast-16-c0da2f54 {k1}"), "box-a: jit-unkeep fast-16-run-c0da2f54".to_string(), "box-b: jit-unkeep fast-9-run-01234567".to_string()], "kept for its red, then its run's hold goes, and the superseded run's");
        assert!(!run_holds(&log).exists() && !logs.join("fast-9.log.holds").exists(), "the run's holds and the older unjudged run's are let go once");
        assert!(logs.join("fast-20.log.holds").exists(), "a newer run's stay: no verdict superseded it");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Shards that end at once on two boxes each list their JIT's hold whole, and the verdict lets
    /// each go on its own box: a holds line written in pieces ran two shards' together, and its
    /// release asked for two holds at once on a "box" named by two boxes' names run together (11-14).
    #[test]
    fn shards_ending_at_once_on_two_boxes_hold_and_release_each_on_its_box() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-twoholds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let log = home.join("tests-7.log");
        let (ka, kb) = ("0123456789abcdef01234567", "fedcba9876543210fedcba98");
        let parts = [format!("L1 box-a: sources at work@0123456789\njit: kept {ka}\n"), format!("L2 box-b: sources at work@0123456789\njit: kept {kb}\n")];
        let rounds = 200;
        std::thread::scope(|scope| {
            for part in &parts {
                let log = &log;
                scope.spawn(move || {
                    for _ in 0..rounds { hold_run_jits(log, part, &|_, _| Ok(())); }
                });
            }
        });
        let asked = std::cell::RefCell::new(Vec::new());
        let ask = |node: &str, line: &str| { asked.borrow_mut().push(format!("{node}: {line}")); Ok(()) };
        let warned = std::cell::RefCell::new(Vec::new());
        release_holds("tests", &run_holds(&log), &ask, &|_, line, _| warned.borrow_mut().push(line));
        let asked = asked.into_inner();
        let (a, b) = ("box-a: jit-unkeep tests-7-run-01234567", "box-b: jit-unkeep tests-7-run-fedcba98");
        assert_eq!(asked.len(), 2 * rounds, "one release per hold taken: {:?}", &asked[..asked.len().min(4)]);
        assert!(asked.iter().all(|l| l == a || l == b), "each hold is released on its own box by its own name: {:?}", asked.iter().find(|l| *l != a && *l != b));
        assert_eq!(asked.iter().filter(|l| *l == a).count(), rounds, "box-a's holds, every one");
        assert!(warned.into_inner().is_empty() && !run_holds(&log).exists(), "all released, the holds file gone");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A shard that ended without its units' verdicts (its lease cancelled, lost, killed) names
    /// those units, its lease, its box and why; one that judged all it started names none; what
    /// its rerun on another box judged is no "no verdict", what it did not is (a full run's
    /// cancelled shard left 158 of its units unjudged and its row named one red, 11-12).
    #[test]
    fn a_shard_ended_without_its_verdicts_names_its_units_and_its_rerun_says_what_stays_unjudged() {
        let part = "L5 box-b: sources at work@abc\nGATESTART test a 1794\nGATEUNIT ok test a\nGATESTART test b 1795\nGATESTART test c --test x 1796\nGATEEND test c --test x\nLEASE CANCELLED by user on box-b (L5): pick another node\n";
        let u = unjudged(&Ok(Some(3)), part).expect("two units unjudged");
        assert_eq!(u, Unjudged { lease: "L5".into(), node: "box-b".into(), why: "LEASE CANCELLED by user on box-b (L5): pick another node".into(), units: vec!["test b".into(), "test c --test x".into()] });
        assert_eq!(unjudged(&Ok(Some(0)), "L6 box-a: sources at work@abc\nGATESTART test a 1\nGATEUNIT fail test a\n"), None, "a shard that judged every unit it started");
        assert_eq!(unjudged(&Err("gate lease: gone".into()), "GATESTART test d 2\n").map(|u| u.why), Some("gate lease: gone".to_string()), "no client line: its status says why");
        assert!(no_verdicts(&[&u], "GATEUNIT ok test b\nGATEUNIT fail test c --test x\n").is_empty(), "its rerun judged both");
        assert_eq!(no_verdicts(&[&u], "GATEUNIT ok test b\n"), vec!["no verdict: 1 unit(s) of shard L5 (LEASE CANCELLED by user on box-b (L5): pick another node), its rerun on another box left them unjudged too".to_string()]);
        // the toolchain changed under a shard: what failed there has no verdict and runs once more,
        // and the run's log names no failure of it
        let swapped = "L7 box-c: sources at work@abc\nGATESTART test a 1\nGATEUNIT ok test a\nGATESTART test b 2\nGATEUNIT fail test b\nTOOLCHAIN CHANGED under L7 on box-c (rustc 1.99.1 -> rustc 1.100.0): toolchain changed under you: rerun.\n";
        let u = unjudged(&Ok(Some(4)), swapped).expect("its failed unit has no verdict");
        assert_eq!((u.units, u.why.starts_with("TOOLCHAIN CHANGED")), (vec!["test b".to_string()], true));
        let text = merged(&[], (1, 8), &[(PathBuf::from("s1.log"), (Ok(Some(4)), swapped.to_string()))]);
        assert!(!text.contains("GATEUNIT fail") && text.contains("GATEUNIT voided test b"), "{text}");
        // a unit its lease was cancelled under while its watch said it hung is no unit left
        // unjudged: it is the run's failure, with its sample, and does not run once more; one the
        // watch said moved again, or one that ended, is unjudged as before
        let hung = "L8 box-d: sources at work@abc\nGATESTART test h 1794000000\nGATESTART test m 1794000001\nGATESTART test e 1794000002\n\
                    GATEBUSY test h :: 1794000010 landgate.x/testrun-h.busy the JIT says: blocked: main waits\nGATEBUSY test m :: 1794000011 landgate.x/testrun-m.busy\n\
                    GATEBUSY test e :: 1794000012 landgate.x/testrun-e.busy\nGATEBUSY test m :: 0\nGATEEND test e\n\
                    LEASE CANCELLED by user on box-d (L8): pick another node (devhub nodes) and retry. Exit code 3.\n";
        assert_eq!(unjudged(&Ok(Some(3)), hung).map(|u| u.units), Some(vec!["test m".to_string(), "test e".to_string()]), "the hung unit does not run once more");
        let ends = hung_ends(hung);
        assert_eq!(ends.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["test h"], "{ends:?}");
        assert!(ends[0].1.contains("test h hung: no new test result since") && ends[0].1.contains("cancelled (by user on box-d (L8))") && ends[0].1.ends_with("landgate.x/testrun-h.busy: the JIT says: blocked: main waits"), "{}", ends[0].1);
        let lost = hung.replace("LEASE CANCELLED by user on box-d (L8)", "LEASE LOST on box-d (L8)");
        assert!(hung_ends(&lost).is_empty() && unjudged(&Ok(Some(4)), &lost).is_some_and(|u| u.units.len() == 3), "a lost lease's busy unit runs once more");
        let text = merged(&[], (1, 8), &[(PathBuf::from("s1.log"), (Ok(Some(3)), hung.to_string()))]);
        let outcome = verdict(Ok(Some(3)), &text, Path::new("tests-1.log"));
        assert!(!outcome.ok && outcome.failing == ["test h"] && outcome.summary.contains("test h hung") && outcome.no_verdict.is_none(), "{:?} {} {:?}", outcome.failing, outcome.summary, outcome.no_verdict);
    }

    #[test]
    fn a_tips_state_is_the_newest_fast_run_with_what_stays_red_and_the_hot_runs_of_that_tip() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-tip-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bucket-logs")).unwrap();
        let pair = |c: char| Pair::new(&c.to_string().repeat(40), &"f".repeat(40));
        let (t0, t1, t2) = (pair('a'), pair('b'), pair('c'));
        let row = |bucket: &str, tip: &Pair, started: i64, failing: &[&str], passed: &[&str], log: &str| obj(vec![
            ("bucket", s(bucket)), ("tip", tip_value(&pin_of(tip))), ("started_ms", Value::Int(started)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())), ("log", s(log)),
        ]);
        append(&home, row("fast", &t0, 10, &["check pkg-a"], &[], "l0")).unwrap();
        append(&home, row("hot", &t1, 15, &["present makepad-foo"], &[], "h1")).unwrap();
        // its JIT did not build: its units did not run, and pkg-a is red still
        append(&home, row("fast", &t1, 20, &["jit"], &["check pkg-b"], "l1")).unwrap();
        append(&home, row("hot", &t2, 25, &[], &[], "h2")).unwrap();
        save_run(&home.join("bucket-logs/fast-30.log"), 7, 30, &pin_of(&t1), &pin_of(&t2)).unwrap();
        save_run(&home.join("bucket-logs/fast-5.log"), 7, 5, &pin_of(&t0), &pin_of(&t0)).unwrap();
        let at = tip_state(&home, &t1);
        assert_eq!(at.fast, Some((t1.clone(), "l1".to_string())));
        assert_eq!(at.stopped.as_deref(), Some("jit"));
        assert_eq!(at.red.keys().cloned().collect::<Vec<_>>(), vec!["check pkg-a".to_string()]);
        assert_eq!(at.hot_failing, vec!["present makepad-foo".to_string()]);
        assert_eq!(at.running, Some((30, t2.clone())), "the run going, of another tip");
        let at = tip_state(&home, &t2);
        assert_eq!((at.running, at.hot_failing.len()), (Some((30, t2.clone())), 0));
        // a sidecar older than the newest row is a finished run
        std::fs::remove_file(home.join("bucket-logs/fast-30.log.run")).unwrap();
        assert_eq!(tip_state(&home, &t0).running, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_box_that_takes_only_proven_tips_moves_to_the_newest_tip_with_nothing_new_red() {
        let home = std::env::temp_dir().join(format!("devhub-bucket-proven-tip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let pair = |c: char| pin_of(&Pair::new(&c.to_string().repeat(40), &"f".repeat(40)));
        let tips: Vec<Pin> = "abcdef9".chars().map(pair).collect();
        let row = |tip: &Pin, failing: &[&str], passed: &[&str], log: &str| obj(vec![
            ("bucket", s("fast")), ("tip", tip_value(tip)), ("started_ms", Value::Int(1)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(passed.iter().map(|u| s(*u)).collect())), ("log", s(log)),
        ]);
        // a box holds the tips up to the one it is at (each tip has the ones before it)
        let at = |n: usize| {
            let held = tips[..=n].to_vec();
            move |t: &Pin| held.contains(t)
        };
        assert_eq!(proven_tip(&home, &|_: &Pin| false), Proven::Stay("no run of the fast bucket looked at a tip yet".into()));
        // a: pkg-a red already
        append(&home, row(&tips[0], &["check pkg-a"], &[], "l0")).unwrap();
        // b: its JIT did not build
        append(&home, row(&tips[1], &["jit"], &[], "l1")).unwrap();
        assert_eq!(proven_tip(&home, &|_: &Pin| false), Proven::Move(tips[0].clone()), "new to the rule: the newest run that did not stop, red and all");
        let stay = proven_tip(&home, &at(0));
        assert!(matches!(&stay, Proven::Stay(why) if why.contains("stopped at its jit step (log l1)")), "{stay:?}");
        // c: pkg-a still red, pkg-b green: taken (pkg-a was red where the box is)
        append(&home, row(&tips[2], &["check pkg-a"], &["check pkg-b"], "l2")).unwrap();
        assert_eq!(proven_tip(&home, &at(0)), Proven::Move(tips[2].clone()));
        // d: pkg-b breaks, and a test, which no show runs
        append(&home, row(&tips[3], &["check pkg-b", "tests pkg-c"], &[], "l3")).unwrap();
        assert_eq!(proven_tip(&home, &at(0)), Proven::Move(tips[2].clone()), "the newest tip with nothing new red");
        let stay = proven_tip(&home, &at(2));
        assert!(matches!(&stay, Proven::Stay(why) if why.starts_with(&format!("tip {} has check pkg-b failing", tip_short(&tips[3]))) && why.ends_with("(log l3)")), "{stay:?}");
        assert_eq!(proven_tip(&home, &at(3)), Proven::Stay("waits for the fast bucket to look at a newer tip".into()), "never back");
        // e: pkg-b fixed, pkg-a too
        append(&home, row(&tips[4], &[], &["check pkg-a", "check pkg-b"], "l4")).unwrap();
        assert_eq!(proven_tip(&home, &at(2)), Proven::Move(tips[4].clone()));
        // f: a present breaks
        append(&home, row(&tips[5], &["present makepad-foo"], &[], "l5")).unwrap();
        let stay = proven_tip(&home, &at(4));
        assert!(matches!(&stay, Proven::Stay(why) if why.contains("has present makepad-foo failing")), "{stay:?}");
        // g: the present fixed, its app's boundary audit red: the app would stop at a JIT boundary there
        append(&home, row(&tips[6], &["boundary makepad-foo"], &["present makepad-foo"], "l6")).unwrap();
        let stay = proven_tip(&home, &at(5));
        assert!(matches!(&stay, Proven::Stay(why) if why.contains("has boundary makepad-foo failing")), "{stay:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A bucket's tip pins each other nested repository it follows: its row says it, its gate
    /// diffs it from the base's commit (all of it where the base had none), and a red unit of it
    /// holds a box that takes only proven tips like any other.
    #[test]
    fn a_tip_carries_the_other_nested_repositories_and_their_red_units_hold() {
        let (a, b, f, e, e2) = ("a".repeat(40), "b".repeat(40), "f".repeat(40), "e".repeat(40), "d".repeat(40));
        let tip = pin_of(&Pair::new(&b, &f)).with("repo-b", &e);
        let row = tip_value(&tip);
        assert_eq!(read_tip(&row), Some(tip.clone()), "round-trips");
        assert_eq!(read_tip(&tip_value(&pin_of(&Pair::new(&a, &f)))), Some(pin_of(&Pair::new(&a, &f))), "a row with no other nested repository");
        assert_eq!(tip_short(&tip), format!("{},{},repo-b={}", &b[..10], &f[..10], &e[..10]));
        assert_eq!(nested_bases(&pin_of(&Pair::new(&a, &f)), &tip), "GATE_NESTED_BASES=repo-b=-", "new to the bucket: all of it");
        assert_eq!(nested_bases(&tip, &pin_of(&Pair::new(&a, &f)).with("repo-b", &e2)), format!("GATE_NESTED_BASES=repo-b={e}"));
        assert!(tip_rev(Path::new("/nonexistent-devhub-repo"), &tip).is_err_and(|e| e.contains("repo-b is on no branch")), "a repository the run cannot pin: no verdict");
        assert_eq!(tip_rev(Path::new("/nonexistent-devhub-repo"), &pin_of(&Pair::new(&a, &f))), Ok(format!("work@{a},commercial=main@{f}")));
        let home = std::env::temp_dir().join(format!("devhub-bucket-nested-tip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fast = |tip: &Pin, failing: &[&str]| obj(vec![("bucket", s("fast")), ("tip", tip_value(tip)), ("started_ms", Value::Int(1)),
            ("failing", Value::Arr(failing.iter().map(|u| s(*u)).collect())), ("passed", Value::Arr(Vec::new())), ("log", s("l"))]);
        let base = pin_of(&Pair::new(&a, &f)).with("repo-b", &e2);
        append(&home, fast(&base, &[])).unwrap();
        append(&home, fast(&tip, &["check repo-b-app"])).unwrap();
        let stay = proven_tip(&home, &|t: &Pin| *t == base);
        assert!(matches!(&stay, Proven::Stay(why) if why.contains(&format!("tip {} has check repo-b-app failing", tip_short(&tip)))), "{stay:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `devhub test` says how the tests bucket last looked at its unit: the newest tests run that
    /// did, red or passed, with its wall; another bucket's rows and a run that never looked at
    /// the unit say nothing of it.
    #[test]
    fn the_tests_buckets_last_look_at_a_unit_is_its_newest() {
        let home = std::env::temp_dir().join(format!("devhub-tests-last-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let units = |v: &[&str]| Value::Arr(v.iter().map(|u| s(*u)).collect());
        let row = |bucket: &str, at: i64, failing: &[&str], passed: &[&str], secs: i64| obj(vec![("bucket", s(bucket)), ("started_ms", Value::Int(at)), ("failing", units(failing)), ("passed", units(passed)), ("unit_s", obj(vec![("test x", Value::Int(secs))]))]).to_json();
        let rows = [row("tests", 1, &[], &["test x", "test y"], 40), row("tests", 2, &["test x"], &[], 45), row("hot", 3, &[], &["test x"], 1), row("tests", 4, &[], &[], 0)];
        std::fs::write(home.join("buckets.jsonl"), rows.join("\n") + "\n").unwrap();
        assert_eq!(tests_last(&home, "test x"), Some((true, 2, Some(45))));
        assert_eq!(tests_last(&home, "test y"), Some((false, 1, None)));
        assert_eq!(tests_last(&home, "test z"), None);
        let _ = std::fs::remove_dir_all(&home);
    }
}
