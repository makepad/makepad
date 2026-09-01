//! FlashWorld image/text -> 3D gaussian-splat WORLD backend (`world` domain).
//!
//! Reference-tier: drives the pinned FlashWorld python reference
//! (github.com/imlixinyang/FlashWorld @ 424db4487, provisioned on the box —
//! venv, repo clone, model.ckpt, Wan2.2-TI2V-5B base) through a PERSISTENT
//! line-protocol worker subprocess (`python/fw_worker.py`, embedded in this
//! binary and staged to the tmp dir at spawn). The own-stack port replaces
//! this backend later; the request/artifact contract stays.
//!
//! Protocol: worker stdout lines prefixed `@EV ` carry one JSON event each —
//! `{"ev":"stage","stage":...}` / `{"ev":"ready"}` / `{"ev":"done","ply":..}`
//! / `{"ev":"error","message":..}`. Everything else on stdout (reference
//! prints, tqdm spill) is ignored. Worker stderr inherits the service's, so
//! python tracebacks land in svc.log.
//!
//! VRAM policy: the loaded reference holds ~55GB. With no cross-backend
//! eviction API in the service yet, the worker subprocess is KILLED after
//! each job by default so a following video/other-domain job on the same box
//! sees a clean GPU; set MAKEPAD_FLASHWORLD_KEEP_WARM=1 on a dedicated box to
//! keep it resident (warm jobs ~6s vs ~70s cold).
//!
//! Box provisioning knobs (all have .169 defaults):
//!   MAKEPAD_FLASHWORLD_PYTHON  venv python     (C:\ai\fwvenv\Scripts\python.exe)
//!   MAKEPAD_FLASHWORLD_DIR     repo clone      (C:\ai\flashworld)
//!   MAKEPAD_FLASHWORLD_CKPT    model.ckpt      (C:\ai\models\flashworld\model.ckpt)

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::child_process;
use crate::error::AssetAiError;
use makepad_micro_serde::*;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The persistent job worker script, staged into the tmp dir at spawn.
const WORKER_PY: &str = include_str!("../python/fw_worker.py");
/// Default camera trajectory: the FlashWorld examples/7.json forward-push
/// walk (24 cameras, ~5.4 units of travel, conditioning image at index 10),
/// proven on the coastal-world validation run.
const CAMERAS_FORWARD_JSON: &str = include_str!("../python/fw_cameras_forward.json");

/// Cold-load budget: model construction + 21GB ckpt load + fp8 quantize.
const READY_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Per-job budget (warm generation is ~6s; leave room for a slow disk).
const JOB_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(DeJson)]
struct WorkerEvent {
    ev: String,
    stage: Option<String>,
    k: Option<u32>,
    n: Option<u32>,
    ply: Option<String>,
    message: Option<String>,
}

/// stdout line stream of the worker child; the reader thread ends (and sends
/// `Err`) when the child closes stdout, i.e. exits or crashes.
struct Worker {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Worker {
    fn kill(mut self) {
        let _ = child_process::kill_tree(&mut self.child);
        let _ = self.child.wait();
    }
}

pub struct WorldBackend {
    model_id: String,
    python: PathBuf,
    repo_dir: PathBuf,
    ckpt: PathBuf,
    keep_warm: bool,
    tmp_dir: Option<PathBuf>,
    worker: Option<Worker>,
    job_counter: u64,
}

fn env_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn flashworld_paths() -> (PathBuf, PathBuf, PathBuf) {
    (
        env_path(
            "MAKEPAD_FLASHWORLD_PYTHON",
            r"C:\ai\fwvenv\Scripts\python.exe",
        ),
        env_path("MAKEPAD_FLASHWORLD_DIR", r"C:\ai\flashworld"),
        env_path(
            "MAKEPAD_FLASHWORLD_CKPT",
            r"C:\ai\models\flashworld\model.ckpt",
        ),
    )
}

/// True only where the reference stack is actually provisioned. The backend
/// compiles everywhere (pure std subprocess driver), so without this probe
/// every box advertises flashworld "ready" and the fleet scheduler routes
/// world jobs to machines that fail them at generate time.
pub fn flashworld_provisioned() -> bool {
    let (python, repo_dir, ckpt) = flashworld_paths();
    python.exists() && repo_dir.exists() && ckpt.exists()
}

impl WorldBackend {
    pub fn new_flashworld(model_id: &str) -> Self {
        let (python, repo_dir, ckpt) = flashworld_paths();
        Self {
            model_id: model_id.to_string(),
            python,
            repo_dir,
            ckpt,
            keep_warm: std::env::var("MAKEPAD_FLASHWORLD_KEEP_WARM").is_ok_and(|v| v == "1"),
            tmp_dir: None,
            worker: None,
            job_counter: 0,
        }
    }

    fn tmp_dir(&self) -> Result<&Path, AssetAiError> {
        self.tmp_dir
            .as_deref()
            .ok_or_else(|| AssetAiError::Backend("world backend not loaded".into()))
    }

    fn spawn_worker(&mut self) -> Result<(), AssetAiError> {
        let tmp = self.tmp_dir()?.to_path_buf();
        std::fs::create_dir_all(&tmp)
            .map_err(|e| AssetAiError::Io(format!("tmp dir {}: {e}", tmp.display())))?;
        let worker_py = tmp.join("fw_worker.py");
        let cameras = tmp.join("fw_cameras_forward.json");
        std::fs::write(&worker_py, WORKER_PY)
            .map_err(|e| AssetAiError::Io(format!("stage fw_worker.py: {e}")))?;
        std::fs::write(&cameras, CAMERAS_FORWARD_JSON)
            .map_err(|e| AssetAiError::Io(format!("stage cameras json: {e}")))?;

        let mut child = child_process::spawn(
            Command::new(&self.python)
            .arg(&worker_py)
            .arg("--ckpt")
            .arg(&self.ckpt)
            .arg("--cameras")
            .arg(&cameras)
            .current_dir(&self.repo_dir)
            .env("PYTHONUNBUFFERED", "1")
            .env("GRADIO_ANALYTICS_ENABLED", "False")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr inherits the service's stream -> tracebacks in svc.log.
        )
            .map_err(|e| {
                AssetAiError::Backend(format!(
                    "spawn {} failed: {e} (is the FlashWorld venv provisioned on this box?)",
                    self.python.display()
                ))
            })?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });

        self.worker = Some(Worker {
            child,
            stdin,
            lines: rx,
        });
        Ok(())
    }

    fn kill_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.kill();
        }
    }

    /// Pumps worker stdout until `until(ev)` returns Some, forwarding stage
    /// events to `progress`; kills the worker and errors out on cancel,
    /// worker exit, or deadline.
    fn pump_events<T>(
        &mut self,
        cold: bool,
        deadline: Duration,
        progress: &mut dyn FnMut(&str, f64),
        cancel: &CancelToken,
        mut until: impl FnMut(&WorkerEvent) -> Option<Result<T, AssetAiError>>,
    ) -> Result<T, AssetAiError> {
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                self.kill_worker();
                return Err(AssetAiError::Cancelled);
            }
            if started.elapsed() > deadline {
                self.kill_worker();
                return Err(AssetAiError::Backend(format!(
                    "flashworld worker timed out after {:?}",
                    deadline
                )));
            }
            let worker = match self.worker.as_ref() {
                Some(worker) => worker,
                None => return Err(AssetAiError::Backend("flashworld worker gone".into())),
            };
            let line = match worker.lines.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.kill_worker();
                    return Err(AssetAiError::Backend(
                        "flashworld worker exited unexpectedly (see service log for the python traceback)"
                            .into(),
                    ));
                }
            };
            let Some(json) = line.strip_prefix("@EV ") else {
                continue; // reference prints / tqdm spill
            };
            let event = match WorkerEvent::deserialize_json(json) {
                Ok(event) => event,
                Err(_) => continue,
            };
            if event.ev == "stage" {
                let stage = event.stage.as_deref().unwrap_or("working");
                progress(stage, stage_fraction(stage, event.k, event.n, cold));
            }
            if let Some(result) = until(&event) {
                return result;
            }
        }
    }
}

/// Maps worker stage names to overall job fractions. Cold jobs spend the
/// first ~70s loading the model (fractions 0..0.70); warm jobs jump straight
/// to generation.
fn stage_fraction(stage: &str, k: Option<u32>, n: Option<u32>, cold: bool) -> f64 {
    let load = |frac: f64| if cold { frac * 0.70 } else { 0.02 };
    let name = stage.split_whitespace().next().unwrap_or(stage);
    match name {
        "boot" => load(0.02),
        "load-libs" => load(0.06),
        "load-vae" => load(0.12),
        "load-tokenizer" => load(0.20),
        "load-text-encoder" => load(0.24),
        "load-transformer" => load(0.45),
        "load-ckpt" => load(0.62),
        "fp8-quantize" => load(0.88),
        "to-gpu" => load(0.96),
        "generate" => {
            if cold {
                0.72
            } else {
                0.06
            }
        }
        "denoise" => {
            let (base, span) = if cold { (0.74, 0.22) } else { (0.10, 0.80) };
            let (k, n) = (k.unwrap_or(0) as f64, n.unwrap_or(4).max(1) as f64);
            base + span * (k / n)
        }
        "export" => {
            if cold {
                0.97
            } else {
                0.92
            }
        }
        _ => if cold { 0.5 } else { 0.5 },
    }
}

impl ContentBackend for WorldBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // No registry-managed downloads: the reference stack (venv + repo +
        // ckpt + Wan base) is provisioned on the box. Validate the paths here
        // so a mis-provisioned box fails with a message naming the knob.
        ctx.ensure_files()?; // no-op for the empty file list; keeps the pattern
        self.tmp_dir = Some(ctx.cache_dir.join("tmp").join("flashworld"));
        for (what, path, var) in [
            ("python", &self.python, "MAKEPAD_FLASHWORLD_PYTHON"),
            ("repo", &self.repo_dir, "MAKEPAD_FLASHWORLD_DIR"),
            ("ckpt", &self.ckpt, "MAKEPAD_FLASHWORLD_CKPT"),
        ] {
            if !path.exists() {
                return Err(AssetAiError::Unavailable(format!(
                    "flashworld {what} not found at {} (set {var})",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.prompt.is_empty() && params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "world generation needs a prompt and/or an input image".into(),
            ));
        }
        if !params.input_bytes.is_empty() && !params.input_bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(AssetAiError::Params(format!(
                "world input must be PNG (got content type {:?})",
                params.input_content_type
            )));
        }
        progress("starting", 0.0);
        cancel.check()?;

        let tmp = self.tmp_dir()?.to_path_buf();
        std::fs::create_dir_all(&tmp)
            .map_err(|e| AssetAiError::Io(format!("tmp dir {}: {e}", tmp.display())))?;

        // Dead child from a previous cancel/crash? Drop it so we respawn.
        if let Some(worker) = self.worker.as_mut() {
            if let Ok(Some(_)) = worker.child.try_wait() {
                self.kill_worker();
            }
        }

        // Cold path: spawn the worker and pump its load stages until ready.
        let cold = self.worker.is_none();
        if cold {
            progress("spawn worker", 0.005);
            self.spawn_worker()?;
            self.pump_events(true, READY_TIMEOUT, progress, cancel, |event| {
                match event.ev.as_str() {
                    "ready" => Some(Ok(())),
                    "error" => Some(Err(AssetAiError::Backend(format!(
                        "flashworld load failed: {}",
                        event.message.as_deref().unwrap_or("unknown")
                    )))),
                    _ => None,
                }
            })?;
        }

        // Stage the job inputs.
        self.job_counter += 1;
        let job_tag = format!(
            "{}_{}",
            std::process::id(),
            self.job_counter
        );
        let out_dir = tmp.join(format!("job_{job_tag}"));
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| AssetAiError::Io(format!("job dir {}: {e}", out_dir.display())))?;
        let image_path = if params.input_bytes.is_empty() {
            None
        } else {
            let path = out_dir.join("input.png");
            std::fs::write(&path, &params.input_bytes)
                .map_err(|e| AssetAiError::Io(format!("write input png: {e}")))?;
            Some(path)
        };

        #[derive(SerJson)]
        struct JobLine {
            prompt: String,
            image_path: Option<String>,
            seed: u64,
            out_dir: String,
        }
        let job = JobLine {
            prompt: params.prompt.clone(),
            image_path: image_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            seed: params.seed,
            out_dir: out_dir.to_string_lossy().into_owned(),
        };
        let mut line = job.serialize_json();
        line.push('\n');
        {
            let worker = self.worker.as_mut().expect("worker alive");
            if let Err(e) = worker.stdin.write_all(line.as_bytes()) {
                self.kill_worker();
                return Err(AssetAiError::Backend(format!(
                    "flashworld worker stdin write failed: {e}"
                )));
            }
        }

        // Pump generation events until done.
        let ply_path = self.pump_events(cold, JOB_TIMEOUT, progress, cancel, |event| {
            match event.ev.as_str() {
                "done" => Some(match event.ply.clone() {
                    Some(ply) => Ok(ply),
                    None => Err(AssetAiError::Backend("worker done without ply path".into())),
                }),
                "error" => Some(Err(AssetAiError::Backend(format!(
                    "flashworld generation failed: {}",
                    event.message.as_deref().unwrap_or("unknown")
                )))),
                _ => None,
            }
        });

        // VRAM policy: free the GPU for other domains on this box unless
        // explicitly pinned warm.
        if !self.keep_warm {
            if let Some(worker) = self.worker.as_mut() {
                // Polite exit lets CUDA teardown run; the reaper below is the
                // backstop.
                let _ = worker.stdin.write_all(b"{\"exit\":true}\n");
            }
            std::thread::sleep(Duration::from_millis(300));
            self.kill_worker();
        }

        let ply_path = ply_path?;
        progress("read ply", 0.98);
        let bytes = std::fs::read(&ply_path)
            .map_err(|e| AssetAiError::Backend(format!("read {ply_path}: {e}")))?;
        let _ = std::fs::remove_dir_all(&out_dir);
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "application/x-ply",
            ext: "ply",
            bytes,
        }])
    }
}

impl Drop for WorldBackend {
    fn drop(&mut self) {
        self.kill_worker();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_event_lines_parse() {
        let ev = WorkerEvent::deserialize_json(
            r#"{"ev":"stage","stage":"denoise 2/4","k":2,"n":4}"#,
        )
        .unwrap();
        assert_eq!(ev.ev, "stage");
        assert_eq!(ev.stage.as_deref(), Some("denoise 2/4"));
        assert_eq!(ev.k, Some(2));
        let done = WorkerEvent::deserialize_json(r#"{"ev":"done","ply":"C:/x/gaussians.ply"}"#)
            .unwrap();
        assert_eq!(done.ply.as_deref(), Some("C:/x/gaussians.ply"));
    }

    #[test]
    fn stage_fractions_monotonic_cold() {
        let seq = [
            stage_fraction("boot", None, None, true),
            stage_fraction("load-vae", None, None, true),
            stage_fraction("load-transformer", None, None, true),
            stage_fraction("load-ckpt", None, None, true),
            stage_fraction("fp8-quantize", None, None, true),
            stage_fraction("generate", None, None, true),
            stage_fraction("denoise 1/4", Some(1), Some(4), true),
            stage_fraction("denoise 4/4", Some(4), Some(4), true),
            stage_fraction("export", None, None, true),
        ];
        for pair in seq.windows(2) {
            assert!(pair[0] < pair[1], "not monotonic: {seq:?}");
        }
        assert!(seq[8] < 1.0);
    }

    #[test]
    fn stage_fractions_monotonic_warm() {
        let seq = [
            stage_fraction("generate", None, None, false),
            stage_fraction("denoise 1/4", Some(1), Some(4), false),
            stage_fraction("denoise 3/4", Some(3), Some(4), false),
            stage_fraction("export", None, None, false),
        ];
        for pair in seq.windows(2) {
            assert!(pair[0] < pair[1], "not monotonic: {seq:?}");
        }
    }

    #[test]
    fn embedded_cameras_parse_as_json() {
        // Keep the embedded preset valid: 24 cameras, resolution 24x480x704.
        assert!(CAMERAS_FORWARD_JSON.contains("\"cameras\""));
        assert!(CAMERAS_FORWARD_JSON.contains("\"resolution\""));
        assert!(CAMERAS_FORWARD_JSON.contains("\"image_index\""));
    }
}
