//! MiniMax-Music3 lyrics + description -> full-song wav backend (`music`
//! domain).
//!
//! Reference-tier: drives the OFFICIAL diffusers ModularPipeline integration
//! of MiniMaxAI/MiniMax-Music3 (model card "🧨 Diffusers" path, diffusers
//! pinned to PR #14456 commit dafe3733fcfdbf3c48915fe77be3aef65b5d6a2d)
//! through a persistent line-protocol worker subprocess
//! (`python/music3_worker.py`, embedded in this binary and staged to the tmp
//! dir at spawn) — the same pattern as `world_backend`. A native own-stack
//! port can replace the worker while preserving this request/artifact
//! contract.
//!
//! Model contract (per the official model card): inputs are `lyrics` (with
//! optional `[Verse]`/`[Chorus]`-style section tags on their own lines) and a
//! music description in `prompt` (genre, BPM, key, vocal + arrangement
//! detail; Structured Captions recommended), plus `seconds` (5..=300,
//! default 60) and `seed`. Output is one stereo 16-bit wav artifact at the
//! pipeline's native sample rate. Weights are registry-managed (entry
//! `minimax-music3`, ~28.5 GB pinned at revision bd348f9c) and download to
//! `music/MiniMax-Music3/` in the service cache via the normal pull-job flow;
//! only the python venv is box-provisioned.
//!
//! VRAM policy: full bf16 residency is ~29 GB, so the worker is KILLED after
//! each job by default (the .169 box shares its GPU with the 90 GB H3 video
//! pipeline); set MAKEPAD_MUSIC3_KEEP_WARM=1 on a dedicated box to keep it
//! resident. MAKEPAD_MUSIC3_OFFLOAD=1 enables the model card's auto CPU
//! offload path (~22 GB peak) for 24/32 GB-class boxes.
//!
//! Box provisioning knob:
//!   MAKEPAD_MUSIC3_PYTHON  venv python  (C:\ai\music3venv\Scripts\python.exe)
//!
use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_micro_serde::*;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The persistent job worker script, staged into the tmp dir at spawn.
const WORKER_PY: &str = include_str!("../python/music3_worker.py");

/// Cache-relative model root; must match the registry entry's `cache_as`
/// prefix for `minimax-music3`.
const MODEL_CACHE_SUBDIR: &str = "music/MiniMax-Music3";

/// Cache-relative root of the audio.cpp GGUF pack; must match the registry
/// entry's `cache_as` prefix for `minimax-music3-q4`.
const MODEL_CACHE_SUBDIR_Q4: &str = "music/MiniMax-Music3-Q4";

/// The registry id whose weights are the audio.cpp GGUF pack. Native engine
/// only — the Python ModularPipeline cannot serve a GGUF pack.
const MODEL_ID_Q4: &str = "minimax-music3-q4";

/// Song duration bounds per the official model card: full songs up to five
/// minutes (generation stops earlier on the model's end-of-audio token).
pub const MIN_SECONDS: f64 = 5.0;
pub const MAX_SECONDS: f64 = 300.0;
pub const DEFAULT_SECONDS: f64 = 60.0;

/// Cold-load budget: ~28.5 GB of shards streamed from disk + bf16 load.
const READY_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Per-job budget: the 8B LM generates 25 frames/s of song autoregressively,
/// so a 5-minute song is 7500 forwards plus the flow-matching + vocoder tail.
const JOB_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(DeJson)]
struct WorkerEvent {
    ev: String,
    stage: Option<String>,
    k: Option<u32>,
    n: Option<u32>,
    wav: Option<String>,
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Music3Backend {
    model_id: String,
    python: PathBuf,
    keep_warm: bool,
    /// Official ModularPipeline only — never the native CUDA path.
    force_python: bool,
    model_dir: Option<PathBuf>,
    tmp_dir: Option<PathBuf>,
    worker: Option<Worker>,
    job_counter: u64,
}

fn music3_python() -> PathBuf {
    std::env::var_os("MAKEPAD_MUSIC3_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ai\music3venv\Scripts\python.exe"))
}

/// Native path is the product path (`audio` + CUDA). Python venv remains a
/// fallback only on `python-backends` boxes that have no CUDA device.
pub fn music3_provisioned() -> bool {
    #[cfg(feature = "audio")]
    {
        if makepad_diffusion::backend::gpu_device_available() {
            return true;
        }
    }
    #[cfg(feature = "python-backends")]
    {
        return music3_python().exists();
    }
    #[cfg(not(feature = "python-backends"))]
    {
        false
    }
}

/// Official Python worker is provisioned when the box venv exists.
pub fn music3_python_provisioned() -> bool {
    music3_python().exists()
}

impl Music3Backend {
    pub fn new_music3(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            python: music3_python(),
            keep_warm: std::env::var("MAKEPAD_MUSIC3_KEEP_WARM").is_ok_and(|v| v == "1"),
            force_python: false,
            model_dir: None,
            tmp_dir: None,
            worker: None,
            job_counter: 0,
        }
    }

    /// Always the official ModularPipeline worker (`minimax-music3-python`).
    pub fn new_music3_python(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            python: music3_python(),
            keep_warm: std::env::var("MAKEPAD_MUSIC3_KEEP_WARM").is_ok_and(|v| v == "1"),
            force_python: true,
            model_dir: None,
            tmp_dir: None,
            worker: None,
            job_counter: 0,
        }
    }

    fn tmp_dir(&self) -> Result<&Path, AssetAiError> {
        self.tmp_dir
            .as_deref()
            .ok_or_else(|| AssetAiError::Backend("music3 backend not loaded".into()))
    }

    fn spawn_worker(&mut self) -> Result<(), AssetAiError> {
        let tmp = self.tmp_dir()?.to_path_buf();
        let model_dir = self
            .model_dir
            .clone()
            .ok_or_else(|| AssetAiError::Backend("music3 backend not loaded".into()))?;
        std::fs::create_dir_all(&tmp)
            .map_err(|e| AssetAiError::Io(format!("tmp dir {}: {e}", tmp.display())))?;
        let worker_py = tmp.join("music3_worker.py");
        std::fs::write(&worker_py, WORKER_PY)
            .map_err(|e| AssetAiError::Io(format!("stage music3_worker.py: {e}")))?;

        let mut child = Command::new(&self.python)
            .arg(&worker_py)
            .arg("--model-dir")
            .arg(&model_dir)
            .arg("--view-dir")
            .arg(tmp.join("model_view"))
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr inherits the service's stream -> tracebacks in svc.log.
            .spawn()
            .map_err(|e| {
                AssetAiError::Backend(format!(
                    "spawn {} failed: {e} (is the Music3 diffusers venv provisioned on this box?)",
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
                    "music3 worker timed out after {:?}",
                    deadline
                )));
            }
            let worker = match self.worker.as_ref() {
                Some(worker) => worker,
                None => return Err(AssetAiError::Backend("music3 worker gone".into())),
            };
            let line = match worker.lines.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.kill_worker();
                    return Err(AssetAiError::Backend(
                        "music3 worker exited unexpectedly (see service log for the python traceback)"
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

impl Music3Backend {
    fn uses_native(&self) -> bool {
        if self.force_python {
            return false;
        }
        #[cfg(feature = "audio")]
        {
            std::env::var("MAKEPAD_MUSIC3_FORCE_PYTHON").ok().as_deref() != Some("1")
                && makepad_diffusion::backend::gpu_device_available()
        }
        #[cfg(not(feature = "audio"))]
        {
            false
        }
    }
}

#[cfg(feature = "audio")]
fn generate_native(
    model_dir: &Path,
    prompt: &str,
    lyrics: &str,
    duration_s: f64,
    seed: u64,
    progress: ProgressSink,
    cancel: &CancelToken,
) -> Result<Vec<ArtifactData>, AssetAiError> {
    use makepad_diffusion::music3_pipeline::{
        music3_generate_with_progress, music3_planar_stereo, Music3Generate,
    };
    cancel.check()?;
    let req = Music3Generate {
        caption: prompt.to_string(),
        lyrics: lyrics.to_string(),
        seconds: duration_s,
        seed,
    };
    let mut last = 0.0f64;
    let audio = music3_generate_with_progress(
        model_dir,
        &req,
        &mut |stage, frac| {
            last = frac;
            progress(stage, frac);
        },
        &|| cancel.is_cancelled(),
    )
    .map_err(|e| match e {
        makepad_diffusion::DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("music3 native: {other}")),
    })?;
    cancel.check()?;
    let (left, right) = music3_planar_stereo(&audio)
        .map_err(|e| AssetAiError::Backend(format!("music3 stereo: {e}")))?;
    progress("wav-encode", last.max(0.98));
    let bytes = crate::wav::encode_wav_pcm16_stereo(
        &left,
        &right,
        makepad_diffusion::music3::MUSIC3_SAMPLE_RATE as u32,
    );
    progress("done", 1.0);
    Ok(vec![ArtifactData {
        content_type: "audio/wav",
        ext: "wav",
        bytes,
    }])
}

#[cfg(not(feature = "audio"))]
fn generate_native(
    _model_dir: &Path,
    _prompt: &str,
    _lyrics: &str,
    _duration_s: f64,
    _seed: u64,
    _progress: ProgressSink,
    _cancel: &CancelToken,
) -> Result<Vec<ArtifactData>, AssetAiError> {
    Err(AssetAiError::Unavailable(
        "music3 native needs the 'audio' cargo feature".into(),
    ))
}

fn official_inputs(params: &GenerateParams) -> Result<(String, String), AssetAiError> {
    let prompt = params.prompt.trim();
    if prompt.is_empty() {
        return Err(AssetAiError::Params(
            "music generation needs a non-empty music description in `prompt`".into(),
        ));
    }
    let lyrics = params.lyrics.trim();
    Ok((
        prompt.to_string(),
        if lyrics.is_empty() {
            "[Instrumental]".to_string()
        } else {
            // Official `_normalize_lyrics` keeps only leading [tags] on a
            // tag line and drops the rest. LLM expand often writes
            // `[Verse] the words` on one line; split that before normalize
            // so the words survive as their own line.
            split_inline_lyric_tags(lyrics)
        },
    ))
}

/// `[Verse] hello` → `[Verse]\nhello`. Bare `[Verse]` is unchanged.
fn split_inline_lyric_tags(lyrics: &str) -> String {
    let mut out = Vec::new();
    for line in lyrics.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if let Some(end) = trimmed.find(']') {
                let tags_end = {
                    let mut i = end + 1;
                    while i < trimmed.len() {
                        let rest = &trimmed[i..];
                        if rest.starts_with(' ') || rest.starts_with('\t') {
                            i += 1;
                            continue;
                        }
                        if rest.starts_with('[') {
                            if let Some(more) = rest.find(']') {
                                i += more + 1;
                                continue;
                            }
                        }
                        break;
                    }
                    i
                };
                let tags = trimmed[..tags_end].trim();
                let rest = trimmed[tags_end..].trim();
                if tags.starts_with('[') && tags.ends_with(']') {
                    out.push(tags.to_string());
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    }
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Maps worker stage names to overall job fractions. Cold jobs spend the
/// first minutes streaming ~28.5 GB of shards (fractions 0..0.30); the
/// dominant span either way is the autoregressive `lm k/n` frame loop. The
/// flow transformer and vocoder also report completed-forward counters, so a
/// client continues moving after semantic generation instead of parking on a
/// phase marker.
fn stage_fraction(stage: &str, k: Option<u32>, n: Option<u32>, cold: bool) -> f64 {
    let load = |frac: f64| if cold { frac * 0.30 } else { 0.02 };
    let name = stage.split_whitespace().next().unwrap_or(stage);
    match name {
        "boot" => load(0.02),
        "build-view" => load(0.06),
        "load-libs" => load(0.15),
        "load-components" => load(0.55),
        "to-gpu" => load(0.90),
        "generate" => {
            if cold {
                0.31
            } else {
                0.04
            }
        }
        // The AR frame loop: k counted LM forwards of n = duration_s * 25.
        "lm" => {
            let (base, span) = if cold { (0.32, 0.55) } else { (0.05, 0.80) };
            let (k, n) = (k.unwrap_or(0) as f64, n.unwrap_or(1).max(1) as f64);
            base + span * (k / n).min(1.0)
        }
        // Flow-matching DiT forwards after the LM loop. The worker derives the
        // total from actual semantic work: chunks * 30 scheduler steps * two
        // classifier-free-guidance forwards.
        "dit" => {
            let (k, n) = (k.unwrap_or(0) as f64, n.unwrap_or(1).max(1) as f64);
            0.88 + 0.08 * (k / n).min(1.0)
        }
        // The vocoder runs once per generated chunk. Keep a small visible
        // band for it: long songs can have many chunks and decoding is real
        // GPU work rather than an indeterminate post-processing pause.
        "vocoder" => {
            let (k, n) = (k.unwrap_or(0) as f64, n.unwrap_or(1).max(1) as f64);
            0.965 + 0.015 * (k / n).min(1.0)
        }
        "write" => 0.985,
        _ => 0.5,
    }
}

impl ContentBackend for Music3Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // Registry-managed weights: downloads/verifies the pinned
        // MiniMaxAI/MiniMax-Music3 diffusers file set (or the audio.cpp GGUF
        // pack for `minimax-music3-q4`) on first use (or via a pull job); a
        // box with verified files skips straight through.
        ctx.ensure_files()?;
        let subdir = if self.model_id == MODEL_ID_Q4 {
            MODEL_CACHE_SUBDIR_Q4
        } else {
            MODEL_CACHE_SUBDIR
        };
        self.model_dir = Some(ctx.cache_dir.join(subdir.split('/').collect::<PathBuf>()));
        self.tmp_dir = Some(ctx.cache_dir.join("tmp").join("music3"));
        if self.uses_native() {
            return Ok(());
        }
        if self.model_id == MODEL_ID_Q4 {
            return Err(AssetAiError::Unavailable(
                "minimax-music3-q4 is a GGUF pack: it needs the native engine \
                 ('audio' feature + GPU device); the Python worker cannot serve it"
                    .into(),
            ));
        }
        if !self.python.exists() {
            return Err(AssetAiError::Unavailable(format!(
                "music3 python not found at {} (set MAKEPAD_MUSIC3_PYTHON)",
                self.python.display()
            )));
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let (prompt, lyrics) = official_inputs(params)?;
        let duration_s = params
            .seconds
            .unwrap_or(DEFAULT_SECONDS)
            .clamp(MIN_SECONDS, MAX_SECONDS);
        progress("starting", 0.0);
        cancel.check()?;
        if self.uses_native() {
            return generate_native(
                self.model_dir.as_deref().ok_or_else(|| {
                    AssetAiError::Backend("music3 backend not loaded".into())
                })?,
                &prompt,
                &lyrics,
                duration_s,
                params.seed,
                progress,
                cancel,
            );
        }
        if self.model_id == MODEL_ID_Q4 {
            return Err(AssetAiError::Unavailable(
                "minimax-music3-q4 is a GGUF pack: it needs the native engine \
                 ('audio' feature + GPU device); the Python worker cannot serve it"
                    .into(),
            ));
        }

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
                        "music3 load failed: {}",
                        event.message.as_deref().unwrap_or("unknown")
                    )))),
                    _ => None,
                }
            })?;
        }

        self.job_counter += 1;
        let out_dir = tmp.join(format!("job_{}_{}", std::process::id(), self.job_counter));
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| AssetAiError::Io(format!("job dir {}: {e}", out_dir.display())))?;

        let job = JobLine {
            prompt,
            lyrics,
            duration_s,
            seed: params.seed,
            out_wav: out_dir.join("song.wav").to_string_lossy().into_owned(),
        };
        let mut line = job.serialize_json();
        line.push('\n');
        {
            let worker = self.worker.as_mut().expect("worker alive");
            if let Err(e) = worker.stdin.write_all(line.as_bytes()) {
                self.kill_worker();
                return Err(AssetAiError::Backend(format!(
                    "music3 worker stdin write failed: {e}"
                )));
            }
        }

        // Pump generation events until done.
        let wav_path = self.pump_events(cold, JOB_TIMEOUT, progress, cancel, |event| {
            match event.ev.as_str() {
                "done" => Some(match event.wav.clone() {
                    Some(wav) => Ok(wav),
                    None => Err(AssetAiError::Backend("worker done without wav path".into())),
                }),
                "error" => Some(Err(AssetAiError::Backend(format!(
                    "music3 generation failed: {}",
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

        let wav_path = wav_path?;
        progress("read wav", 0.995);
        let bytes = std::fs::read(&wav_path)
            .map_err(|e| AssetAiError::Backend(format!("read {wav_path}: {e}")))?;
        let _ = std::fs::remove_dir_all(&out_dir);
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes,
        }])
    }
}

#[derive(SerJson)]
struct JobLine {
    prompt: String,
    lyrics: String,
    duration_s: f64,
    seed: u64,
    out_wav: String,
}

impl Drop for Music3Backend {
    fn drop(&mut self) {
        self.kill_worker();
    }
}

/// Splits a single prompt-box text into (music description, lyrics) at the
/// first lyrics marker. Accepts a line that is `lyrics:` (case-insensitive)
/// after stripping wrapping `*`/`#`, or a line that *starts* with `lyrics:`
/// (so `Lyrics: [Verse]` still splits). Same-line remainder becomes the
/// first lyrics line. Programmatic requests should set `lyrics` directly.
pub fn split_music_prompt(text: &str) -> (String, String) {
    let mut description = Vec::new();
    let mut lyrics = Vec::new();
    let mut in_lyrics = false;
    for line in text.lines() {
        if !in_lyrics {
            if let Some(rest) = lyrics_marker_rest(line) {
                in_lyrics = true;
                if !rest.is_empty() {
                    lyrics.push(rest);
                }
                continue;
            }
            description.push(line);
        } else {
            lyrics.push(line);
        }
    }
    (
        description.join("\n").trim().to_string(),
        lyrics.join("\n").trim().to_string(),
    )
}

fn lyrics_marker_rest(line: &str) -> Option<&str> {
    let t = line.trim().trim_matches('*').trim_matches('#').trim();
    if t.len() >= 7 && t[..7].eq_ignore_ascii_case("lyrics:") {
        Some(t[7..].trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::GenerateParams;
    use crate::protocol::GenerateRequestJson;

    #[test]
    fn worker_event_lines_parse() {
        let ev =
            WorkerEvent::deserialize_json(r#"{"ev":"stage","stage":"lm 250/1500","k":250,"n":1500}"#)
                .unwrap();
        assert_eq!(ev.ev, "stage");
        assert_eq!(ev.stage.as_deref(), Some("lm 250/1500"));
        assert_eq!(ev.k, Some(250));
        assert_eq!(ev.n, Some(1500));
        let done =
            WorkerEvent::deserialize_json(r#"{"ev":"done","wav":"C:/x/song.wav"}"#).unwrap();
        assert_eq!(done.wav.as_deref(), Some("C:/x/song.wav"));
        let err = WorkerEvent::deserialize_json(r#"{"ev":"error","message":"boom"}"#).unwrap();
        assert_eq!(err.message.as_deref(), Some("boom"));
    }

    #[test]
    fn stage_fractions_monotonic_cold() {
        let seq = [
            stage_fraction("boot", None, None, true),
            stage_fraction("build-view", None, None, true),
            stage_fraction("load-libs", None, None, true),
            stage_fraction("load-components", None, None, true),
            stage_fraction("to-gpu", None, None, true),
            stage_fraction("generate", None, None, true),
            stage_fraction("lm 25/1500", Some(25), Some(1500), true),
            stage_fraction("lm 1500/1500", Some(1500), Some(1500), true),
            stage_fraction("dit 5/120", Some(5), Some(120), true),
            stage_fraction("dit 120/120", Some(120), Some(120), true),
            stage_fraction("vocoder 1/3", Some(1), Some(3), true),
            stage_fraction("vocoder 3/3", Some(3), Some(3), true),
            stage_fraction("write", None, None, true),
        ];
        for pair in seq.windows(2) {
            assert!(pair[0] < pair[1], "not monotonic: {seq:?}");
        }
        assert!(seq.last().copied().unwrap() < 1.0);
    }

    #[test]
    fn stage_fractions_monotonic_warm() {
        let seq = [
            stage_fraction("generate", None, None, false),
            stage_fraction("lm 25/1500", Some(25), Some(1500), false),
            stage_fraction("lm 1400/1500", Some(1400), Some(1500), false),
            stage_fraction("dit 5/120", Some(5), Some(120), false),
            stage_fraction("dit 120/120", Some(120), Some(120), false),
            stage_fraction("vocoder 1/2", Some(1), Some(2), false),
            stage_fraction("vocoder 2/2", Some(2), Some(2), false),
            stage_fraction("write", None, None, false),
        ];
        for pair in seq.windows(2) {
            assert!(pair[0] < pair[1], "not monotonic: {seq:?}");
        }
    }

    #[test]
    fn semantic_and_flow_counters_move_the_reported_fraction() {
        let semantic: Vec<_> = [5, 25, 250, 750, 1500]
            .into_iter()
            .map(|k| stage_fraction("lm", Some(k), Some(1500), true))
            .collect();
        assert!(semantic.windows(2).all(|pair| pair[0] < pair[1]));

        let flow: Vec<_> = [5, 30, 60, 90, 120]
            .into_iter()
            .map(|k| stage_fraction("dit", Some(k), Some(120), true))
            .collect();
        assert!(flow.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(semantic.last().unwrap() < flow.first().unwrap());
    }

    #[test]
    fn worker_hooks_the_inner_qwen_model_used_by_official_pipeline() {
        assert!(WORKER_PY.contains("getattr(lm, \"model\", lm)"));
        assert!(WORKER_PY.contains("n_key=\"lm_n\""));
        assert!(WORKER_PY.contains("n_key=\"dit_n\""));
        assert!(WORKER_PY.contains("n_key=\"vocoder_n\""));
    }

    #[test]
    fn job_line_escapes_multiline_lyrics() {
        let job = JobLine {
            prompt: "warm acoustic pop".into(),
            lyrics: "[Verse]\nMorning \"light\"\n[Chorus]\nSoftly".into(),
            duration_s: 60.0,
            seed: 7,
            out_wav: r"C:\cache\tmp\music3\job_1_1\song.wav".into(),
        };
        let line = job.serialize_json();
        assert!(!line.contains('\n'), "job line must stay a single line");
        // Round-trip through the same serde the worker's json.loads mirrors.
        #[derive(DeJson)]
        struct JobBack {
            prompt: String,
            lyrics: String,
            duration_s: f64,
            seed: u64,
            out_wav: String,
        }
        let back = JobBack::deserialize_json(&line).unwrap();
        assert_eq!(back.prompt, "warm acoustic pop");
        assert_eq!(back.lyrics, "[Verse]\nMorning \"light\"\n[Chorus]\nSoftly");
        assert!((back.duration_s - 60.0).abs() < 1e-9);
        assert_eq!(back.seed, 7);
        assert!(back.out_wav.ends_with("song.wav"));
    }

    #[test]
    fn lyrics_field_rides_the_wire_and_duration_clamps() {
        let request = GenerateRequestJson {
            model: "minimax-music3".to_string(),
            prompt: Some("Genre: blues rock. BPM: 92.".to_string()),
            lyrics: Some("[Verse]\nDust on the highway".to_string()),
            seconds: Some(1e9),
            ..GenerateRequestJson::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        assert_eq!(params.lyrics, "[Verse]\nDust on the highway");
        // Wire clamp caps at the music maximum; the backend re-clamps to
        // [MIN_SECONDS, MAX_SECONDS].
        assert!((params.seconds.unwrap() - MAX_SECONDS).abs() < 1e-9);
        let low = params.seconds.unwrap().clamp(MIN_SECONDS, MAX_SECONDS);
        assert!(low <= MAX_SECONDS && low >= MIN_SECONDS);
    }

    #[test]
    fn official_inputs_require_description_and_normalize_instrumental() {
        let mut backend = Music3Backend::new_music3("minimax-music3");
        let request = GenerateRequestJson {
            model: "minimax-music3".to_string(),
            ..GenerateRequestJson::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        let mut sink = |_: &str, _: f64| {};
        match backend.generate(&params, &mut sink, &CancelToken::new()) {
            Err(AssetAiError::Params(_)) => {}
            Err(other) => panic!("expected Params error, got {other:?}"),
            Ok(_) => panic!("expected Params error, got artifacts"),
        }

        let request = GenerateRequestJson {
            model: "minimax-music3".to_string(),
            prompt: Some("cinematic ambient score, 72 BPM".to_string()),
            ..GenerateRequestJson::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        let (prompt, lyrics) = official_inputs(&params).unwrap();
        assert_eq!(prompt, "cinematic ambient score, 72 BPM");
        assert_eq!(lyrics, "[Instrumental]");
    }

    #[test]
    fn split_music_prompt_convention() {
        let (desc, lyrics) = split_music_prompt(
            "Genre: acoustic pop. BPM: 96.\nWarm and intimate.\nLyrics:\n[Verse]\nMorning light\n[Chorus]\nSoftly",
        );
        assert_eq!(desc, "Genre: acoustic pop. BPM: 96.\nWarm and intimate.");
        assert_eq!(lyrics, "[Verse]\nMorning light\n[Chorus]\nSoftly");
        // No marker = everything is description, lyrics empty.
        let (desc, lyrics) = split_music_prompt("just a vibe");
        assert_eq!(desc, "just a vibe");
        assert!(lyrics.is_empty());
        // Marker on the first line = pure-lyrics request.
        let (desc, lyrics) = split_music_prompt("lyrics:\n[Verse]\nHello");
        assert!(desc.is_empty());
        assert_eq!(lyrics, "[Verse]\nHello");
        // LLM expand often wraps the label or puts the first tag on the same line.
        let (desc, lyrics) = split_music_prompt(
            "Global Metadata: folk.\n**Lyrics:**\n[Verse]\nHello",
        );
        assert_eq!(desc, "Global Metadata: folk.");
        assert_eq!(lyrics, "[Verse]\nHello");
        let (desc, lyrics) = split_music_prompt("pads and bass\nLyrics: [Verse] city lights");
        assert_eq!(desc, "pads and bass");
        assert_eq!(lyrics, "[Verse] city lights");
    }

    #[test]
    fn official_inputs_split_inline_section_tags() {
        let request = GenerateRequestJson {
            model: "minimax-music3".to_string(),
            prompt: Some("indie folk, 102 BPM".to_string()),
            lyrics: Some("[Verse] chasing the sun\n[Chorus] hold on".to_string()),
            ..GenerateRequestJson::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        let (prompt, lyrics) = official_inputs(&params).unwrap();
        assert_eq!(prompt, "indie folk, 102 BPM");
        assert_eq!(lyrics, "[Verse]\nchasing the sun\n[Chorus]\nhold on");
    }
}
