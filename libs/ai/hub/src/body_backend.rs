//! SAM 3D Body worker backend for the `body` domain.
//!
//! The worker returns one JSON object per input frame. Its pose packet schema
//! is `{"n_people":N,"people":[{"mhr":[204 f32],"global_rot":[3],
//! "cam_t":[3],"shape":[45],"expr":[72],"focal":f32,"bbox":[4],
//! "joints":[[x,y,z] x127]?}]}`. Rust validates only that the line is JSON
//! with a top-level `n_people` field and otherwise forwards it opaquely.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, LiveFrameIn,
    LiveFrameOut, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_strict_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const BODY_WORKER_ENV: &str = "MAKEPAD_SAM3DBODY_WORKER";
pub const BODY_TIMEOUT_ENV: &str = "MAKEPAD_SAM3DBODY_TIMEOUT_S";
pub const BODY_SPAWN_TIMEOUT_ENV: &str = "MAKEPAD_SAM3DBODY_SPAWN_TIMEOUT_S";
const DEFAULT_TIMEOUT_S: f64 = 10.0;
// The real worker loads its model at spawn (~12s reference, more on a cold
// disk) and only then emits its `{"ready":true}` line — the per-frame
// timeout must not start until that handshake, or the first frame kills a
// still-loading worker and the restart loop reloads it forever.
const DEFAULT_SPAWN_TIMEOUT_S: f64 = 120.0;
const MAX_RESTARTS: u8 = 3;

pub fn body_provisioned() -> bool {
    std::env::var(BODY_WORKER_ENV)
        .ok()
        .is_some_and(|command| !command.trim().is_empty())
}

fn configured_command() -> Result<Vec<String>, AssetAiError> {
    let command = std::env::var(BODY_WORKER_ENV).map_err(|_| {
        AssetAiError::Unavailable(format!(
            "sam3dbody worker is not configured; set {BODY_WORKER_ENV}"
        ))
    })?;
    let parts: Vec<String> = command
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        return Err(AssetAiError::Unavailable(format!(
            "sam3dbody worker command in {BODY_WORKER_ENV} is empty"
        )));
    }
    Ok(parts)
}

fn positive_seconds_env(env: &str, default_s: f64) -> Result<Duration, AssetAiError> {
    let Some(text) = std::env::var(env).ok() else {
        return Ok(Duration::from_secs_f64(default_s));
    };
    let seconds = text.parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0);
    match seconds {
        Some(seconds) => Ok(Duration::from_secs_f64(seconds)),
        None => Err(AssetAiError::Unavailable(format!(
            "{env} must be a positive number of seconds, got {text:?}"
        ))),
    }
}

fn configured_timeout() -> Result<Duration, AssetAiError> {
    positive_seconds_env(BODY_TIMEOUT_ENV, DEFAULT_TIMEOUT_S)
}

fn configured_spawn_timeout() -> Result<Duration, AssetAiError> {
    positive_seconds_env(BODY_SPAWN_TIMEOUT_ENV, DEFAULT_SPAWN_TIMEOUT_S)
}

enum WorkerRead {
    Line(String),
    Eof,
    Error(String),
}

struct WorkerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<WorkerRead>,
    reader: Option<JoinHandle<()>>,
    // The worker's first stdout line must be its ready handshake (a JSON
    // object with a `ready` field), emitted after its model finishes
    // loading; frames sent before it are only buffered by the OS pipe.
    ready_seen: bool,
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = crate::child_process::kill_tree(&mut self.child);
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Persistent length-prefixed PNG / JSON-lines worker connection.
pub struct BodyWorker {
    command: Vec<String>,
    timeout: Duration,
    spawn_timeout: Duration,
    process: Option<WorkerProcess>,
    restarts: u8,
}

impl BodyWorker {
    pub fn new() -> Result<Self, AssetAiError> {
        let mut worker = Self {
            command: configured_command()?,
            timeout: configured_timeout()?,
            spawn_timeout: configured_spawn_timeout()?,
            process: None,
            restarts: 0,
        };
        worker.spawn_process()?;
        Ok(worker)
    }

    fn spawn_process(&mut self) -> Result<(), AssetAiError> {
        let mut command = Command::new(&self.command[0]);
        command
            .args(&self.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = crate::child_process::spawn(&mut command).map_err(|error| {
            AssetAiError::Unavailable(format!(
                "spawn sam3dbody worker {:?}: {error}",
                self.command
            ))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AssetAiError::Backend("sam3dbody worker has no piped stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AssetAiError::Backend("sam3dbody worker has no piped stdout".to_string())
        })?;
        let (line_tx, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => {
                        let _ = line_tx.send(WorkerRead::Eof);
                        return;
                    }
                    Ok(_) => {
                        while line.ends_with('\n') || line.ends_with('\r') {
                            line.pop();
                        }
                        if line_tx.send(WorkerRead::Line(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = line_tx.send(WorkerRead::Error(error.to_string()));
                        return;
                    }
                }
            }
        });
        self.process = Some(WorkerProcess {
            child,
            stdin: Some(stdin),
            lines,
            reader: Some(reader),
            ready_seen: false,
        });
        Ok(())
    }

    /// Waits for the worker's `{"ready":true}` handshake line under the
    /// spawn timeout. Returns Ok(true) when ready, Ok(false) after a
    /// restart (caller re-enters its loop), Err on cancel/timeout/limit.
    fn await_ready(&mut self, cancel: &CancelToken) -> Result<bool, AssetAiError> {
        if self.process.as_ref().unwrap().ready_seen {
            return Ok(true);
        }
        let deadline = Instant::now() + self.spawn_timeout;
        loop {
            if cancel.is_cancelled() {
                self.stop_process();
                return Err(AssetAiError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                self.stop_process();
                return Err(AssetAiError::Backend(format!(
                    "sam3dbody worker not ready after {:.0} seconds",
                    self.spawn_timeout.as_secs_f64()
                )));
            }
            let wait = (deadline - now).min(Duration::from_millis(50));
            match self.process.as_ref().unwrap().lines.recv_timeout(wait) {
                Ok(WorkerRead::Line(line)) => {
                    let is_ready = makepad_strict_json::parse(line.as_bytes())
                        .ok()
                        .is_some_and(|value| value.get("ready").is_some());
                    if is_ready {
                        self.process.as_mut().unwrap().ready_seen = true;
                        return Ok(true);
                    }
                    self.restart_after_death(&format!(
                        "first line was not the ready handshake: {line:.120}"
                    ))?;
                    return Ok(false);
                }
                Ok(WorkerRead::Eof) => {
                    self.restart_after_death("exited before ready handshake")?;
                    return Ok(false);
                }
                Ok(WorkerRead::Error(error)) => {
                    self.restart_after_death(&format!("stdout read failed: {error}"))?;
                    return Ok(false);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.restart_after_death("stdout reader stopped")?;
                    return Ok(false);
                }
            }
        }
    }

    fn stop_process(&mut self) {
        self.process.take();
    }

    fn restart_after_death(&mut self, reason: &str) -> Result<(), AssetAiError> {
        self.stop_process();
        if self.restarts >= MAX_RESTARTS {
            return Err(AssetAiError::Backend(format!(
                "sam3dbody worker died after {MAX_RESTARTS} restarts: {reason}"
            )));
        }
        self.restarts += 1;
        self.spawn_process()
    }

    pub fn ensure_started(&mut self) -> Result<(), AssetAiError> {
        if self.process.is_none() {
            self.spawn_process()?;
        }
        Ok(())
    }

    pub fn is_started(&self) -> bool {
        self.process.is_some()
    }

    pub fn restart_count(&self) -> u8 {
        self.restarts
    }

    fn begin_session(&mut self) {
        self.restarts = 0;
    }

    /// Sends one PNG and waits for the matching pose JSON line.
    pub fn process_png(
        &mut self,
        png: &[u8],
        cancel: &CancelToken,
    ) -> Result<String, AssetAiError> {
        cancel.check()?;
        let length = u32::try_from(png.len()).map_err(|_| {
            AssetAiError::Params("sam3dbody input png exceeds 4 GiB".to_string())
        })?;

        loop {
            self.ensure_started()?;
            let exited = self
                .process
                .as_mut()
                .unwrap()
                .child
                .try_wait()
                .map_err(|error| {
                    AssetAiError::Backend(format!(
                        "sam3dbody worker status check failed: {error}"
                    ))
                })?;
            if let Some(status) = exited {
                self.restart_after_death(&format!("exited with {status}"))?;
                continue;
            }
            if !self.await_ready(cancel)? {
                continue;
            }

            // KNOWN GAP (P2): this write has no deadline — a worker that
            // wedges mid-frame-read can block us in write_all. The lock-step
            // protocol (one frame in flight) makes that window small; the
            // full fix is a writer thread symmetrical to the reader.
            let write_result = {
                let process = self.process.as_mut().unwrap();
                let stdin = process.stdin.as_mut().unwrap();
                stdin
                    .write_all(&length.to_le_bytes())
                    .and_then(|_| stdin.write_all(png))
                    .and_then(|_| stdin.flush())
            };
            if let Err(error) = write_result {
                self.restart_after_death(&format!("stdin write failed: {error}"))?;
                continue;
            }

            let deadline = Instant::now() + self.timeout;
            loop {
                if cancel.is_cancelled() {
                    self.stop_process();
                    return Err(AssetAiError::Cancelled);
                }
                let now = Instant::now();
                if now >= deadline {
                    self.stop_process();
                    return Err(AssetAiError::Backend(format!(
                        "sam3dbody worker timed out after {:.3} seconds",
                        self.timeout.as_secs_f64()
                    )));
                }
                let wait = (deadline - now).min(Duration::from_millis(50));
                let event = self.process.as_ref().unwrap().lines.recv_timeout(wait);
                match event {
                    Ok(WorkerRead::Line(line)) => {
                        validate_pose_packet(&line)?;
                        return Ok(line);
                    }
                    Ok(WorkerRead::Eof) => {
                        self.restart_after_death("stdout closed")?;
                        break;
                    }
                    Ok(WorkerRead::Error(error)) => {
                        self.restart_after_death(&format!("stdout read failed: {error}"))?;
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        self.restart_after_death("stdout reader stopped")?;
                        break;
                    }
                }
            }
        }
    }
}

pub fn validate_pose_packet(line: &str) -> Result<(), AssetAiError> {
    let value = makepad_strict_json::parse(line.as_bytes()).map_err(|error| {
        AssetAiError::Backend(format!("sam3dbody worker returned invalid json: {error}"))
    })?;
    if !matches!(&value, Value::Obj(_)) || value.get("n_people").is_none() {
        return Err(AssetAiError::Backend(
            "sam3dbody worker json is missing top-level n_people".to_string(),
        ));
    }
    Ok(())
}

pub struct BodyBackend {
    model_id: String,
    worker: Option<BodyWorker>,
}

impl BodyBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            worker: None,
        }
    }

    pub fn with_worker(model_id: &str, worker: BodyWorker) -> Self {
        Self {
            model_id: model_id.to_string(),
            worker: Some(worker),
        }
    }

    fn worker_mut(&mut self) -> Result<&mut BodyWorker, AssetAiError> {
        self.worker.as_mut().ok_or_else(|| {
            AssetAiError::Backend("sam3dbody backend used before ensure_loaded".to_string())
        })
    }
}

impl ContentBackend for BodyBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        ctx.cancel.check()?;
        (ctx.progress)("body: worker", 0.5);
        match self.worker.as_mut() {
            Some(worker) => worker.ensure_started()?,
            None => self.worker = Some(BodyWorker::new()?),
        }
        (ctx.progress)("body: ready", 1.0);
        Ok(())
    }

    fn is_resident(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(BodyWorker::is_started)
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.worker = None;
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs an input image (input_b64 png)",
                self.model_id
            )));
        }
        if crate::subproc_img::png_header(&params.input_bytes).is_none() {
            return Err(AssetAiError::Params(
                "sam3dbody input_b64 is not a png".to_string(),
            ));
        }
        cancel.check()?;
        progress("body: infer", 0.05);
        let worker = self.worker_mut()?;
        worker.begin_session();
        let pose = worker.process_png(&params.input_bytes, cancel)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "application/json",
            ext: "json",
            bytes: pose.into_bytes(),
        }])
    }

    fn live_supported(&self) -> bool {
        true
    }

    fn live_step(
        &mut self,
        frame: LiveFrameIn<'_>,
        cancel: &CancelToken,
    ) -> Result<LiveFrameOut, AssetAiError> {
        cancel.check()?;
        let start = Instant::now();
        let init = frame.init.ok_or_else(|| {
            AssetAiError::Params("sam3dbody live step requires an input frame".to_string())
        })?;
        let png = crate::testpattern::encode_png_rgb8(
            &init.data,
            init.width as usize,
            init.height as usize,
        )?;
        let worker = self.worker_mut()?;
        if frame.frame_index == 0 {
            worker.begin_session();
        }
        let pose = worker.process_png(&png, cancel)?;
        cancel.check()?;
        Ok(LiveFrameOut {
            image: init.clone(),
            aux_json: Some(pose),
            model_ms: start.elapsed().as_secs_f64() * 1000.0,
            text_encode_ms: 0.0,
        })
    }
}
