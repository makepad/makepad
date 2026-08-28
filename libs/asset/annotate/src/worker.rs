//! The annotation worker: drain the store's `annotate.asset` queue.
//!
//! The queue is the store's own job queue, so this is a worker like the
//! generation coordinator is a worker — claim under a lease, heartbeat with
//! real progress, succeed or fail with a reason. What makes it its own loop
//! is BATCHING: every executor invocation pays a one-off model load (~15 s
//! on the fleet box), so claiming one job at a time would spend most of the
//! GPU's day loading weights. It claims up to `batch` jobs, runs ONE
//! executor pass over all their sheets, and settles them individually.
//!
//! Characters are split off into their own pass inside the batch: they get
//! [`crate::PROMPT_PERSON`], which asks about a person rather than a piece
//! that snaps onto a grid, and a prompt file is per-invocation.
//!
//! Two hosts run exactly this loop — the CLI's `--worker` mode and the
//! Asset UI, which hosts it in-process beside its embedded server — so it
//! lives here rather than in either.

use crate::executor::{run_batch, ExecutorChoice, ExecutorLine};
use crate::pass::{self, SheetPrep};
use crate::plan::{Annotator, BaseAnnotation};
use crate::{needs_annotation, parse_record, plan_upload};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClaimedJobDto, ClientConfig, JobId,
};
use makepad_asset_data::AssetId;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The job kind this worker claims. Mirrors
/// `makepad_asset_store::host::annotate::JOB_KIND`; the two crates do not
/// depend on each other, and the string IS the contract between them.
pub const JOB_KIND: &str = "annotate.asset";

/// Lease taken per job and the ceiling the server allows. A 32-sheet batch
/// at ~4 s/sheet plus a model load is about 2.5 minutes; the lease is long
/// enough that a slow box does not lose its work, and heartbeats extend it.
const LEASE_MS: u64 = 10 * 60 * 1000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(20);
/// Sleep between claim attempts when the queue is empty.
const IDLE_SLEEP: Duration = Duration::from_secs(2);
/// Backoff after a transport error, so a stopped server is not hammered.
const ERROR_SLEEP: Duration = Duration::from_secs(5);
/// Delay before a failed job's next attempt.
const RETRY_DELAY_MS: u64 = 30_000;

/// Everything the loop needs.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub endpoints: ApiEndpoints,
    pub server_id: Option<[u8; 16]>,
    pub token: String,
    /// This worker's own client cache. An `AssetClient` cache is
    /// single-owner: two loops must never share one.
    pub cache_root: PathBuf,
    /// Scratch for sheets and batch files; cleared per batch.
    pub work: PathBuf,
    /// Data-plane address the turntable sheets are fetched from.
    pub data: SocketAddr,
    pub executor: ExecutorChoice,
    /// Sheets per executor invocation.
    pub batch: usize,
    /// Lease suffix / worker identity stem.
    pub suffix: String,
    pub version: u32,
    pub prep: SheetPrep,
    pub log: bool,
}

impl WorkerConfig {
    fn annotator(&self) -> Annotator {
        Annotator { version: self.version, model: self.executor.model_tag.clone() }
    }
}

fn log(cfg: &WorkerConfig, message: &str) {
    if cfg.log {
        eprintln!("[annotate-worker] {message}");
    }
}

/// Run until `stop`. Never panics on a transport failure: an Asset Server
/// that goes away is a reason to wait, not to end the worker.
pub fn run(cfg: &WorkerConfig, stop: &AtomicBool) {
    log(
        cfg,
        &format!(
            "executor {} · argv {:?} · batch {} · v{}",
            cfg.executor.label(),
            cfg.executor.argv,
            cfg.batch,
            cfg.version
        ),
    );
    let mut client_cfg = ClientConfig::new(cfg.cache_root.join("cache"));
    client_cfg.token = Some(cfg.token.clone());
    let client = match AssetClient::connect(client_cfg, cfg.endpoints, cfg.server_id) {
        Ok(c) => c,
        Err(e) => {
            log(cfg, &format!("connect failed: {e}"));
            return;
        }
    };
    while !stop.load(Ordering::Relaxed) {
        match claim_batch(&client, cfg, stop) {
            Ok(batch) if batch.is_empty() => sleep_until(stop, IDLE_SLEEP),
            Ok(batch) => {
                let n = batch.len();
                let started = Instant::now();
                let (published, failed) = run_claimed(&client, cfg, batch, stop);
                log(
                    cfg,
                    &format!(
                        "batch of {n}: {published} published, {failed} failed in {:.0}s",
                        started.elapsed().as_secs_f64()
                    ),
                );
            }
            Err(e) => {
                log(cfg, &format!("claim failed: {e}"));
                sleep_until(stop, ERROR_SLEEP);
            }
        }
    }
    log(cfg, "stopped");
}

/// Sleep in short slices so a stop is noticed promptly.
fn sleep_until(stop: &AtomicBool, total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One asset the worker holds a lease on.
struct Held {
    job: JobId,
    asset: AssetId,
    alias: String,
    /// Characters get the person prompt, which is a separate invocation.
    person: bool,
    ppm: PathBuf,
}

fn claim_batch(
    client: &AssetClient,
    cfg: &WorkerConfig,
    stop: &AtomicBool,
) -> Result<Vec<ClaimedJobDto>, String> {
    let mut out = Vec::new();
    while out.len() < cfg.batch.max(1) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let claimed = client
            .worker_claim_kinds(LEASE_MS, Some(&cfg.suffix), &[JOB_KIND])
            .map_err(|e| format!("{e}"))?;
        match claimed {
            Some(job) => out.push(job),
            // The queue is drained — run what we have rather than waiting
            // for a full batch that may never arrive.
            None => break,
        }
    }
    Ok(out)
}

/// Prepare, run and settle one claimed batch. Returns (published, failed).
fn run_claimed(
    client: &AssetClient,
    cfg: &WorkerConfig,
    claimed: Vec<ClaimedJobDto>,
    stop: &AtomicBool,
) -> (usize, usize) {
    let _ = std::fs::remove_dir_all(&cfg.work);
    let sheets = cfg.work.join("sheets");
    if let Err(e) = std::fs::create_dir_all(&sheets) {
        for job in &claimed {
            fail(client, cfg, &job.job, &format!("scratch dir: {e}"));
        }
        return (0, claimed.len());
    }
    let mut held: Vec<Held> = Vec::new();
    let mut failed = 0usize;
    for job in &claimed {
        match prepare(cfg, job, &sheets) {
            Ok(h) => held.push(h),
            Err(e) => {
                fail(client, cfg, &job.job, &e);
                failed += 1;
            }
        }
    }
    if held.is_empty() {
        return (0, failed);
    }
    // Two invocations at most: pieces and people never share a prompt file.
    let (people, pieces): (Vec<Held>, Vec<Held>) = held.into_iter().partition(|h| h.person);
    let mut published = 0usize;
    for (group, person) in [(pieces, false), (people, true)] {
        if group.is_empty() {
            continue;
        }
        let (ok, bad) = run_group(client, cfg, &group, person, stop);
        published += ok;
        failed += bad;
    }
    (published, failed)
}

/// Fetch one asset's sheet and write the image the executor will read.
fn prepare(
    cfg: &WorkerConfig,
    job: &ClaimedJobDto,
    sheets: &std::path::Path,
) -> Result<Held, String> {
    let alias = job
        .body
        .get("alias")
        .and_then(Value::as_str)
        .ok_or("job body has no alias")?
        .to_string();
    let asset = job
        .body
        .get("asset")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<AssetId>().ok())
        .ok_or("job body has no asset id")?;
    let person = job.body.get("kind").and_then(Value::as_str) == Some("character");
    let png = pass::thumbnail_sheet(cfg.data, &cfg.token, &alias)?;
    let ppm_bytes = pass::sheet_to_ppm(&png, person, &cfg.prep)?;
    let ppm = sheets.join(format!("{}.ppm", asset.to_string().replace(['/', ':'], "_")));
    std::fs::write(&ppm, ppm_bytes).map_err(|e| format!("write {}: {e}", ppm.display()))?;
    Ok(Held { job: job.job, asset, alias, person, ppm })
}

/// Run one prompt's worth of a batch and settle every job in it.
fn run_group(
    client: &AssetClient,
    cfg: &WorkerConfig,
    group: &[Held],
    person: bool,
    stop: &AtomicBool,
) -> (usize, usize) {
    let tag = if person { "person" } else { "kit" };
    let jobs_path = cfg.work.join(format!("jobs-{tag}.tsv"));
    let prompt_path = cfg.work.join(format!("prompt-{tag}.txt"));
    let out_path = cfg.work.join(format!("replies-{tag}.tsv"));
    let mut jobs = String::new();
    for h in group {
        jobs.push_str(&pass::job_line(
            &h.asset.to_string(),
            &h.ppm,
            &pass::context_line(&h.alias, person),
        ));
    }
    if let Err(e) = std::fs::write(&jobs_path, &jobs) {
        return settle_all_failed(client, cfg, group, &format!("write jobs: {e}"));
    }
    if let Err(e) = std::fs::write(&prompt_path, pass::prompt_for(person)) {
        return settle_all_failed(client, cfg, group, &format!("write prompt: {e}"));
    }

    // Progress: heartbeat every held lease with the batch's real position
    // and measured rate. A silent executor still ticks (run_batch delivers
    // an empty line on timeout), so a lease never expires under a live run.
    let started = Instant::now();
    let mut last_beat = Instant::now();
    let mut done = 0usize;
    let total = group.len();
    let mut on_line = |line: &str, parsed: ExecutorLine| {
        if let ExecutorLine::Progress { done: d, .. } = parsed {
            done = d.min(total);
        }
        if cfg.log && !line.trim().is_empty() {
            eprintln!("[annotate-worker] {}", line.trim());
        }
        if last_beat.elapsed() < HEARTBEAT_EVERY {
            return;
        }
        last_beat = Instant::now();
        let per = if done > 0 {
            started.elapsed().as_secs_f64() / done as f64
        } else {
            started.elapsed().as_secs_f64()
        };
        let note = format!(
            "sheet {done}/{total} · {per:.1} s/sheet · {}",
            cfg.executor.label()
        );
        let permille = ((done * 1000) / total.max(1)).min(1000) as u16;
        for h in group {
            let _ = client.worker_heartbeat(
                &h.job,
                LEASE_MS,
                Some(&cfg.suffix),
                Some((permille, &note)),
            );
        }
    };
    let replies = match run_batch(
        &cfg.executor.argv,
        &jobs_path,
        &prompt_path,
        &out_path,
        stop,
        &mut on_line,
    ) {
        Ok(r) => r,
        Err(e) => return settle_all_failed(client, cfg, group, &e),
    };

    let annotator = cfg.annotator();
    let mut published = 0usize;
    let mut failed = 0usize;
    for h in group {
        let id = h.asset.to_string();
        let Some(reply) = replies.ok.get(&id) else {
            let why = replies
                .err
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "executor returned no reply".to_string());
            fail(client, cfg, &h.job, &why);
            failed += 1;
            continue;
        };
        match publish_one(client, cfg, h, reply, &annotator) {
            Ok(description) => {
                let result = obj(vec![
                    ("asset_id", s(id.clone())),
                    ("alias", s(h.alias.clone())),
                    ("description", s(description)),
                    ("model", s(cfg.executor.model_tag.clone())),
                ]);
                if let Err(e) = client.worker_succeed(&h.job, Some(&cfg.suffix), Some(&result)) {
                    log(cfg, &format!("{}: succeed refused: {e}", h.alias));
                }
                published += 1;
            }
            Err(e) => {
                fail(client, cfg, &h.job, &e);
                failed += 1;
            }
        }
    }
    (published, failed)
}

/// Read the record, recompute the fields the pass owns, write it back.
fn publish_one(
    client: &AssetClient,
    cfg: &WorkerConfig,
    held: &Held,
    reply: &str,
    annotator: &Annotator,
) -> Result<String, String> {
    let rec = parse_record(reply);
    if !rec.is_useful() {
        return Err("unusable reply".to_string());
    }
    let current = client
        .get_annotation(&held.asset)
        .map_err(|e| format!("read annotation: {e}"))?;
    let base = BaseAnnotation {
        title: current.title,
        description: current.description,
        kind: current.kind.map(|k| makepad_asset_client::dto::kind_name(k).to_string()),
        categories: current.categories,
        tags: current.tags,
        creator: current.creator,
        generator: current.generator,
        backend: current.backend,
        model: current.model,
        prompt: current.prompt,
        provenance: current.provenance,
        private: current.private,
    };
    // A concurrent run already at this version means the work is done; do
    // not spend a write undoing someone else's identical answer.
    if !needs_annotation(&base.tags, annotator) && cfg.version == annotator.version {
        return Ok(base.description);
    }
    let up = plan_upload(&base, &rec, annotator);
    let upload = makepad_asset_client::AnnotationUpload {
        title: up.title.clone(),
        description: up.description.clone(),
        kind: up.kind.as_deref().and_then(makepad_asset_client::dto::kind_parse),
        categories: up.categories.clone(),
        tags: up.tags.clone(),
        creator: up.creator.clone(),
        generator: up.generator.clone(),
        backend: up.backend.clone(),
        model: up.model.clone(),
        prompt: up.prompt.clone(),
        provenance: up.provenance.clone(),
        private: up.private,
    };
    client
        .put_annotation(&held.asset, &upload)
        .map_err(|e| format!("put annotation: {e}"))?;
    Ok(up.description)
}

fn settle_all_failed(
    client: &AssetClient,
    cfg: &WorkerConfig,
    group: &[Held],
    why: &str,
) -> (usize, usize) {
    for h in group {
        fail(client, cfg, &h.job, why);
    }
    (0, group.len())
}

/// Report a failure with its reason. The reason is what an operator reads
/// in the RUNS list, so it is never swallowed.
fn fail(client: &AssetClient, cfg: &WorkerConfig, job: &JobId, why: &str) {
    let mut text = why.to_string();
    text.truncate(500);
    let doc = obj(vec![("error", s(text.clone()))]);
    if let Err(e) = client.worker_fail(job, Some(&cfg.suffix), RETRY_DELAY_MS, Some(&doc)) {
        log(cfg, &format!("fail report refused: {e}"));
    }
    log(cfg, &format!("job failed: {text}"));
}
