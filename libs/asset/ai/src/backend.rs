//! The `ContentBackend` trait: one impl per runtime.
//!
//! Shipped impls:
//! - `testpattern` (src/testpattern.rs): procedural PNG from the prompt hash.
//!   No GPU, no downloads — it makes the whole service (queue, artifacts,
//!   endpoints) testable end-to-end on any machine and IS the CI test.
//! - `flux` (src/flux_backend.rs, cargo feature `flux`): real image
//!   generation through libs/diffusion's `FluxPipeline`.
//!
//! Adding a runtime = implementing this trait and matching its registry
//! `backend` string in `create_backend`.

use crate::download::{DownloadProgress, Downloader};
use crate::error::AssetAiError;
use crate::protocol::GenerateRequestJson;
use crate::registry::{FileSpec, ModelSpec};
use std::path::{Path, PathBuf};

/// Normalized generation parameters. Ranges are clamped here; `None` means
/// "not requested" so each backend can apply its own domain default (image
/// 512x512/4 steps, video 640x352/50 steps/124 frames, ...).
#[derive(Clone)]
pub struct GenerateParams {
    pub model: String,
    pub prompt: String,
    pub negative_prompt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub seed: u64,
    pub steps: Option<u32>,
    pub guidance: f32,
    /// Test hook (testpattern backend only): artificial generation time.
    pub delay_ms: u64,
    /// Download/verify files then stop before generating (pull job).
    pub pull_only: bool,

    // Binary input for chained stages (image->mesh, i2v, image->world, STT).
    /// Decoded `input_b64` bytes; empty when the request carried none.
    pub input_bytes: Vec<u8>,
    /// Content type of `input_bytes`, e.g. "image/png".
    pub input_content_type: String,
    /// Decoded named inputs (multi-input models); empty when none were sent.
    pub inputs: Vec<NamedInput>,

    // Video domain (h3 backend).
    /// Frame count at the model's native fps.
    pub frames: Option<u32>,
    /// "" = backend default (h265); "h264" for the compatibility codec.
    pub codec: String,

    // Text domain (llm backend).
    /// Domain the expanded prompt targets: "image" | "video" | "mesh".
    pub target_domain: String,
    /// Exact subject identity which the LLM must preserve verbatim.
    pub identity_anchor: String,
    pub style: String,
    pub max_tokens: u32,
    /// 0.0 = greedy/deterministic.
    pub temperature: f32,
    pub variants: u32,

    // Speech domain (kokoro + indextts backends).
    pub text: String,
    pub voice: String,
    pub speed: f32,
    /// 8-slot emotion vector (indextts), validated to length 8 and clamped
    /// per-slot to [0, 1.2]; `None` = neutral.
    pub emotion: Option<[f32; 8]>,

    // Audio domain (sa3 backend); also the music domain's song duration.
    /// Clip duration in seconds; `None` = backend default.
    pub seconds: Option<f64>,

    // Music domain (music3 backend).
    /// Song lyrics, optionally with `[Verse]`-style section tags on their
    /// own lines; empty = generation driven by the description alone.
    pub lyrics: String,

    // Mesh domain (trellis backend).
    /// FaithC retopo grid resolution for the output GLB; `None` = raw mesh.
    pub remesh_resolution: Option<u32>,
    /// Mesh texturing (tex SLAT flow + decode -> baked atlas / vertex
    /// colors); `None` = backend default (trellis: on).
    pub texture: Option<bool>,
    /// Face target for decimated mesh output; `None` = backend default.
    pub decimation_target: Option<u32>,
    /// Baked texture atlas size; `None` = backend default.
    pub texture_size: Option<u32>,

    // Peer-assisted model distribution (see crate::peer / crate::peer_fetch).
    /// Coordinator-selected source-box base URLs, tried before Hugging Face.
    pub peer_sources: Vec<String>,
    /// Coordinator-minted transfer tickets (self-describing scope).
    pub peer_tickets: Vec<String>,
}

/// One decoded named input (see `GenerateRequestJson::inputs`).
#[derive(Clone)]
pub struct NamedInput {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for GenerateParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerateParams")
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .field("pull_only", &self.pull_only)
            .field("input_bytes_len", &self.input_bytes.len())
            .field("inputs_len", &self.inputs.len())
            .field("peer_sources_len", &self.peer_sources.len())
            .field("peer_tickets", &format_args!("{} redacted", self.peer_tickets.len()))
            .finish_non_exhaustive()
    }
}

impl GenerateParams {
    /// Builds params from the wire request. `input_b64` that fails to decode
    /// is an error (a chained stage relaying an artifact must not silently
    /// generate from nothing).
    pub fn from_request(request: &GenerateRequestJson) -> Result<Self, AssetAiError> {
        let seed = request.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
        });
        let temperature = request.temperature.unwrap_or(0.7);
        let speed = request.speed.unwrap_or(1.0);
        let input_bytes = match request.input_b64.as_deref() {
            None | Some("") => Vec::new(),
            Some(b64) => makepad_base64::base64_decode(b64.as_bytes())
                .map_err(|e| AssetAiError::Params(format!("input_b64: bad base64: {e:?}")))?,
        };
        let inputs = match request.inputs.as_deref() {
            None => Vec::new(),
            Some(list) => {
                if list.len() > 4 {
                    return Err(AssetAiError::Params(format!(
                        "inputs: at most 4 named inputs, got {}",
                        list.len()
                    )));
                }
                let mut inputs: Vec<NamedInput> = Vec::with_capacity(list.len());
                for input in list {
                    if input.name.is_empty() || input.name.len() > 64 {
                        return Err(AssetAiError::Params(
                            "inputs: name must be 1..=64 chars".to_string(),
                        ));
                    }
                    if inputs.iter().any(|prev| prev.name == input.name) {
                        return Err(AssetAiError::Params(format!(
                            "inputs: duplicate name {:?}",
                            input.name
                        )));
                    }
                    if input.content_type.is_empty() || input.content_type.len() > 128 {
                        return Err(AssetAiError::Params(format!(
                            "inputs[{}]: content_type must be 1..=128 chars",
                            input.name
                        )));
                    }
                    // The in-repo decoder assumes 4-char groups; reject other
                    // lengths here so hostile wire input refuses instead of
                    // panicking the worker.
                    if input.data_b64.is_empty() || input.data_b64.len() % 4 != 0 {
                        return Err(AssetAiError::Params(format!(
                            "inputs[{}]: base64 length {} is not a non-empty multiple of 4",
                            input.name,
                            input.data_b64.len()
                        )));
                    }
                    let bytes = makepad_base64::base64_decode(input.data_b64.as_bytes())
                        .map_err(|e| {
                            AssetAiError::Params(format!(
                                "inputs[{}]: bad base64: {e:?}",
                                input.name
                            ))
                        })?;
                    if bytes.is_empty() || bytes.len() > 128 * 1024 * 1024 {
                        return Err(AssetAiError::Params(format!(
                            "inputs[{}]: decoded size {} outside 1..=134217728 bytes",
                            input.name,
                            bytes.len()
                        )));
                    }
                    inputs.push(NamedInput {
                        name: input.name.clone(),
                        content_type: input.content_type.clone(),
                        bytes,
                    });
                }
                inputs
            }
        };
        let is_chat = request.domain.as_deref() == Some("chat")
            || request
                .chat_messages
                .as_ref()
                .is_some_and(|messages| !messages.is_empty());
        let prompt = if is_chat {
            match request.chat_messages.as_deref() {
                Some(messages) if !messages.is_empty() => crate::protocol::assemble_chat_prompt(
                    request.chat_system.as_deref().unwrap_or(""),
                    messages,
                ),
                _ => request.prompt.clone().unwrap_or_default(),
            }
        } else {
            request.prompt.clone().unwrap_or_default()
        };
        let target_domain = if is_chat {
            "chat".to_string()
        } else {
            request
                .target_domain
                .clone()
                .unwrap_or_else(|| "image".to_string())
        };

        Ok(Self {
            model: request.model.clone(),
            prompt,
            negative_prompt: request.negative_prompt.clone().unwrap_or_default(),
            width: request.width.map(|v| v.clamp(16, 8192)),
            height: request.height.map(|v| v.clamp(16, 8192)),
            seed,
            steps: request.steps.map(|v| v.clamp(1, 200)),
            guidance: request.guidance.unwrap_or(3.5) as f32,
            delay_ms: request.delay_ms.unwrap_or(0).min(60_000),
            pull_only: request.pull_only.unwrap_or(false),

            input_bytes,
            input_content_type: request
                .input_content_type
                .clone()
                .unwrap_or_else(|| "image/png".to_string()),
            inputs,

            frames: request.frames.map(|v| v.clamp(1, 1024)),
            codec: request.codec.clone().unwrap_or_default(),

            target_domain,
            identity_anchor: request.identity_anchor.clone().unwrap_or_default(),
            style: request.style.clone().unwrap_or_default(),
            max_tokens: request.max_tokens.unwrap_or(512).clamp(16, 4096),
            temperature: if temperature.is_finite() {
                temperature.clamp(0.0, 2.0) as f32
            } else {
                0.7
            },
            variants: request.variants.unwrap_or(1).clamp(1, 8),

            text: request.text.clone().unwrap_or_default(),
            voice: request.voice.clone().unwrap_or_default(),
            speed: if speed.is_finite() && speed > 0.0 {
                speed.clamp(0.25, 4.0) as f32
            } else {
                1.0
            },
            emotion: match request.emotion.as_deref() {
                None | Some([]) => None,
                Some(values) => {
                    if values.len() != 8 {
                        return Err(AssetAiError::Params(format!(
                            "emotion: expected 8 floats [happy, angry, sad, afraid, \
                             disgusted, melancholic, surprised, calm], got {}",
                            values.len()
                        )));
                    }
                    let mut emotion = [0f32; 8];
                    for (slot, value) in emotion.iter_mut().zip(values) {
                        if !value.is_finite() {
                            return Err(AssetAiError::Params(
                                "emotion: non-finite value".to_string(),
                            ));
                        }
                        *slot = value.clamp(0.0, 1.2) as f32;
                    }
                    Some(emotion)
                }
            },

            // Wire-level sanity range only; each backend clamps to its own
            // supported span (sa3/moss/woosh <=120s clips, music3 <=300s
            // songs).
            seconds: request
                .seconds
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(0.5, 300.0)),

            lyrics: request.lyrics.clone().unwrap_or_default(),

            // 0 = raw decode mesh (escape hatch — must survive the clamp);
            // None = backend default (trellis: FaithC retopo at 256).
            remesh_resolution: request
                .remesh_resolution
                .map(|v| if v == 0 { 0 } else { v.clamp(16, 512) }),
            texture: request.texture,
            decimation_target: request
                .decimation_target
                .map(|v| v.clamp(1_000, 2_000_000)),
            texture_size: request.texture_size.map(|v| v.clamp(256, 4096)),

            peer_sources: {
                let sources = request.peer_sources.clone().unwrap_or_default();
                if sources.len() > 32 {
                    return Err(AssetAiError::Params(format!(
                        "peer_sources: at most 32 entries, got {}",
                        sources.len()
                    )));
                }
                if let Some(bad) = sources.iter().find(|s| s.len() > 256) {
                    return Err(AssetAiError::Params(format!(
                        "peer_sources: entry longer than 256 chars ({} chars)",
                        bad.len()
                    )));
                }
                sources
            },
            peer_tickets: {
                let tickets = request.peer_tickets.clone().unwrap_or_default();
                if tickets.len() > crate::peer::MAX_PLAN_TICKETS {
                    return Err(AssetAiError::Params(format!(
                        "peer_tickets: at most {} entries, got {}",
                        crate::peer::MAX_PLAN_TICKETS,
                        tickets.len()
                    )));
                }
                if tickets.iter().any(|t| t.len() > 512) {
                    return Err(AssetAiError::Params(
                        "peer_tickets: entry longer than 512 chars".to_string(),
                    ));
                }
                tickets
            },
        })
    }
}

/// One generated output. `content_type` drives the `/artifact` response;
/// `ext` names the file on disk.
pub struct ArtifactData {
    pub content_type: &'static str,
    pub ext: &'static str,
    pub bytes: Vec<u8>,
}

/// `(stage, progress 0..=1)` — forwarded into the job state so `/job/<id>`
/// shows e.g. running{stage:"denoise 3/50", progress:0.4}.
///
/// PROGRESS CONVENTION (every backend follows it): the stage string names
/// the phase and carries the within-stage count where one exists
/// ("denoise 3/50", "vae-clip 12/42", "load unet 8.2/23.8GB"); the f64 is
/// the OVERALL job fraction. Long phases (weight stream-in, per-step
/// denoise, per-tile decode) must report per component/step/tile — an
/// opaque multi-second stage is a bug.
pub type ProgressSink<'a> = &'a mut dyn FnMut(&str, f64);

/// Shared cancel flag for one job: raised by `POST /job/<id>/cancel` while
/// the job runs; backends check it at every natural boundary (between
/// denoise steps, VAE tiles/clips, weight-load components, pipeline stages)
/// and unwind with [`AssetAiError::Cancelled`] promptly — seconds, not
/// end-of-job. A single in-flight kernel/forward is the granularity floor.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise the flag (idempotent).
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `Err(AssetAiError::Cancelled)` once the flag is raised — the one-liner
    /// backends call between steps: `cancel.check()?;`
    pub fn check(&self) -> Result<(), AssetAiError> {
        if self.is_cancelled() {
            Err(AssetAiError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// What a backend gets to work with while loading: the registry entry, the
/// cache dir, and the downloader (with a progress hook the server uses to
/// surface per-model download state on `/models`).
pub struct BackendCtx<'a> {
    pub spec: &'a ModelSpec,
    pub cache_dir: &'a Path,
    pub downloader: &'a Downloader,
    pub download_progress: &'a mut dyn FnMut(DownloadProgress),
    /// The job cancellation token is available during artifact download,
    /// verification, conversion and resident loading — not only generation.
    pub cancel: &'a CancelToken,
    /// Load-phase progress, same convention as [`ProgressSink`]: any
    /// ensure_loaded work beyond a presence check (weight parse, worker
    /// spawn, subprocess warmup) names what it's doing so a cold load is
    /// never a silent "load 0%".
    pub progress: &'a mut dyn FnMut(&str, f64),
}

impl<'a> BackendCtx<'a> {
    /// Downloads any registry files missing from the cache; returns their
    /// resolved local paths in registry order.
    pub fn ensure_files(&mut self) -> Result<Vec<PathBuf>, AssetAiError> {
        let mut paths = Vec::new();
        for file in &self.spec.files {
            self.cancel.check()?;
            // Legacy converters treated a final converted path as sufficient
            // and often deleted/never retained the upstream. Preserve that
            // behavior. Structured conversions require their strict receipt;
            // otherwise preparation downloads/verifies the source so the
            // backend converter can run.
            if file.conversion.is_none() {
                if let Some(converted) = file.converted_path(self.cache_dir) {
                    if converted.is_file() {
                        paths.push(converted);
                        continue;
                    }
                }
            } else if crate::download::converted_file_is_verified(file, self.cache_dir) {
                paths.push(file.converted_path(self.cache_dir).unwrap());
                continue;
            }
            paths.push(self.downloader.ensure_file(
                file,
                self.cache_dir,
                self.download_progress,
                self.cancel,
            )?);
        }
        Ok(paths)
    }

    pub fn file_by_role(&self, role: &str) -> Result<&FileSpec, AssetAiError> {
        self.spec.file_by_role(role).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "model {}: registry has no artifact with role {:?}",
                self.spec.id, role
            ))
        })
    }

    /// Resolve a prepared runtime artifact by semantic role. Structured
    /// conversions resolve to their verified final output; legacy converted
    /// files resolve there when present, otherwise to their source.
    pub fn path_by_role(&self, role: &str) -> Result<PathBuf, AssetAiError> {
        let file = self.file_by_role(role)?;
        if let Some(converted) = file.converted_path(self.cache_dir) {
            if converted.is_file() {
                if file.conversion.is_none()
                    || crate::download::converted_file_is_verified(file, self.cache_dir)
                {
                    return Ok(converted);
                }
            }
        }
        let source = file.dest_path(self.cache_dir);
        if crate::download::source_file_is_verified(file, self.cache_dir) {
            Ok(source)
        } else {
            Err(AssetAiError::Backend(format!(
                "model {}: artifact role {:?} is not prepared/verified at {}",
                self.spec.id,
                role,
                source.display()
            )))
        }
    }
}

pub trait ContentBackend: Send {
    fn model_id(&self) -> &str;

    /// Download, verify and (for backend-specific formats) convert artifacts.
    /// Backend construction must be cheap: a pull job creates the object and
    /// calls this hook, but deliberately stops before `ensure_loaded`.
    fn prepare_artifacts(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files().map(|_| ())
    }

    /// Brings the model to a usable state; may trigger downloads through
    /// `ctx.ensure_files()`. Must be idempotent — the worker calls it before
    /// every job and keeps backend instances alive across jobs, so a loaded
    /// backend should return quickly.
    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError>;

    /// True only while this backend genuinely retains usable model runtime
    /// state (normally GPU/host weights or a live resident worker). Merely
    /// being callable as a subprocess is not residency.
    fn is_resident(&self) -> bool {
        false
    }

    /// Release resident runtime state. Default is appropriate for stateless
    /// and subprocess backends; native heavy backends override both hooks.
    fn unload(&mut self) -> Result<(), AssetAiError> {
        Ok(())
    }

    /// Whether a failed `generate` left resident runtime state safe to reuse.
    /// Cancellation is assumed clean by default because backends are required
    /// to unwind at natural boundaries; ordinary errors are conservative and
    /// retire residency unless a backend explicitly proves recovery.
    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        matches!(error, AssetAiError::Cancelled)
    }

    /// Runs one generation. `progress` reports fine-grained stages (see
    /// [`ProgressSink`] convention); `cancel` must be checked at every
    /// natural boundary and unwound promptly with
    /// [`AssetAiError::Cancelled`] (partial work discarded; GPU pools
    /// released; resident weights may stay — that's the warm cache).
    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError>;
}

/// True when this build contains an implementation for the given registry
/// `backend` string. `/models` reports models with missing backends as
/// unavailable instead of failing at generate time.
pub fn backend_compiled(name: &str) -> bool {
    match name {
        "testpattern" => true,
        "flux" | "flux2" => cfg!(feature = "flux"),
        "llm" => cfg!(feature = "llm"),
        "kokoro" => cfg!(feature = "tts"),
        "indextts" => cfg!(feature = "indextts"),
        "h3" => cfg!(feature = "video"),
        "sa3" => cfg!(feature = "audio"),
        "moss" => cfg!(feature = "audio"),
        "woosh" => cfg!(feature = "audio"),
        "ace" => cfg!(feature = "audio"),
        "trellis" => cfg!(feature = "mesh"),
        "paint" | "paint-test" => cfg!(feature = "paint"),
        "matte-native" => cfg!(feature = "matte-native"),
        "depth-native" => cfg!(feature = "depth-native"),
        "segment-native" => cfg!(feature = "segment-native"),
        "rig-native" => cfg!(feature = "rig-native"),
        "motion-native" => cfg!(feature = "motion-native"),
        // Native Music3 lives on the same CUDA stack as SA3 (`audio`).
        "music3" => cfg!(feature = "audio") || cfg!(feature = "python-backends"),
        // Official ModularPipeline reference — compiled even when the
        // other python-backends (FlashWorld / oracles) are off.
        "music3-python" => true,
        // Box-provisioned Python/Torch reference-tier backends are an
        // explicit opt-in. Native-only fleet builds must never advertise
        // them merely because their std-only wrappers compile everywhere.
        "flashworld" | "depth" | "rig-oracle" | "motion-oracle" => {
            cfg!(feature = "python-backends")
        }
        _ => false,
    }
}

/// True when this MACHINE can actually serve the backend. Compiled-in is not
/// enough for subprocess reference-tier backends — their python stacks are
/// provisioned per box, and `/models` must not advertise capabilities the
/// scheduler would route jobs to only to fail at generate time.
pub fn backend_provisioned(name: &str) -> bool {
    match name {
        // The canonical flux models are CUDA-FP8-only combined checkpoints:
        // a build with the `flux` feature on a machine without a CUDA device
        // (mac dev/CI, Metal) must not advertise them — there is no
        // CPU/Metal/BF16 fallback behind the ready state.
        #[cfg(feature = "flux")]
        "flux" => crate::flux_backend::flux_fp8_provisioned(),
        #[cfg(feature = "flux")]
        "flux2" => crate::flux2_backend::flux2_cuda_provisioned(),
        // The deterministic paint tier ("paint-test") runs anywhere; the real
        // Hunyuan backend ("paint") fails closed until its native CUDA
        // executor and pinned checkpoints are present on this machine.
        #[cfg(feature = "paint")]
        "paint" => crate::paint_backend::hunyuan_native_provisioned(),
        #[cfg(not(feature = "paint"))]
        "paint" => false,
        #[cfg(feature = "python-backends")]
        "flashworld" => crate::world_backend::flashworld_provisioned(),
        "music3" => crate::music3_backend::music3_provisioned(),
        "music3-python" => crate::music3_backend::music3_python_provisioned(),
        "depth-native" => cfg!(feature = "depth-native"),
        "segment-native" => cfg!(feature = "segment-native"),
        #[cfg(feature = "python-backends")]
        "depth" => crate::depth_backend::depth_provisioned(),
        #[cfg(feature = "python-backends")]
        "rig-oracle" => crate::rig_backend::rig_provisioned(),
        #[cfg(feature = "python-backends")]
        "motion-oracle" => crate::motion_backend::motion_provisioned(),
        #[cfg(not(feature = "python-backends"))]
        "flashworld" | "depth" | "rig-oracle" | "motion-oracle" => false,
        _ => true,
    }
}

/// Applies the registry's hard GPU requirements. Unlike the scheduling
/// estimate, declared minimums fail closed when the corresponding hardware
/// fact is unavailable: an architecture-specific checkpoint must never be
/// advertised, pulled, or loaded on a box we cannot prove is compatible.
pub fn check_gpu_requirements(
    model_id: &str,
    min_vram_gb: Option<f64>,
    min_compute_cap: Option<f64>,
    gpu: &crate::gpu::GpuInfo,
) -> Result<(), String> {
    let describe = || {
        format!(
            "GPU {:?} vram {:?}MB cc {:?}",
            gpu.name, gpu.vram_total_mb, gpu.compute_cap
        )
    };
    if let Some(min_vram_gb) = min_vram_gb {
        let Some(total_mb) = gpu.vram_total_mb else {
            return Err(format!(
                "model {model_id} requires >= {min_vram_gb} GB VRAM but the GPU is unknown ({}) — refusing (fail closed)",
                describe()
            ));
        };
        if (total_mb as f64) < min_vram_gb * 1024.0 {
            return Err(format!(
                "model {model_id} requires >= {min_vram_gb} GB VRAM; this box has {total_mb} MB ({})",
                describe()
            ));
        }
    }
    if let Some(min_cap) = min_compute_cap {
        let Some(cap) = gpu.compute_cap else {
            return Err(format!(
                "model {model_id} requires CUDA compute capability >= {min_cap} but the GPU is unknown ({}) — refusing (fail closed)",
                describe()
            ));
        };
        if cap + 1e-6 < min_cap {
            return Err(format!(
                "model {model_id} requires CUDA compute capability >= {min_cap}; this GPU reports {cap} ({})",
                describe()
            ));
        }
    }
    Ok(())
}

/// One authoritative answer to whether this service can execute `spec` on
/// this machine. Declared hard GPU requirements are fail-closed. In their
/// absence, a missing total-VRAM reading does not invent a limit (preserving
/// CPU/Metal development), while a known card that cannot hold the registry
/// estimate plus reserve is permanently unavailable.
pub fn model_availability(
    spec: &ModelSpec,
    gpu: &crate::gpu::GpuInfo,
    vram_reserve_mb: u64,
) -> Result<(), String> {
    if !spec.available {
        return Err(match &spec.note {
            Some(note) => format!("disabled in registry: {note}"),
            None => "disabled in registry".to_string(),
        });
    }
    if !backend_compiled(&spec.backend) {
        return Err(format!(
            "backend {:?} is not compiled into this build (cargo feature)",
            spec.backend
        ));
    }
    if !backend_provisioned(&spec.backend) {
        return Err(format!(
            "backend {:?} is not provisioned on this machine",
            spec.backend
        ));
    }

    check_gpu_requirements(
        &spec.id,
        spec.min_vram_gb,
        spec.min_compute_cap,
        gpu,
    )?;

    let estimate_mb = crate::residency::estimated_peak_mb(spec);
    if estimate_mb > 0 {
        if let Some(total_mb) = gpu.vram_total_mb {
            let required_mb = estimate_mb.saturating_add(vram_reserve_mb);
            if required_mb > total_mb {
                return Err(format!(
                    "requires {required_mb} MB total VRAM (estimate {estimate_mb} MB + reserve {vram_reserve_mb} MB), but this machine reports {total_mb} MB"
                ));
            }
        }
    }
    Ok(())
}

pub fn create_backend(spec: &ModelSpec) -> Result<Box<dyn ContentBackend>, AssetAiError> {
    match spec.backend.as_str() {
        #[cfg(feature = "paint")]
        "paint" | "paint-test" => Ok(Box::new(crate::paint_backend::PaintBackend::new(spec))),
        "testpattern" => Ok(Box::new(crate::testpattern::TestPatternBackend::new(
            &spec.id,
        ))),
        #[cfg(feature = "flux")]
        "flux" => Ok(Box::new(crate::flux_backend::FluxBackend::new(&spec.id))),
        #[cfg(not(feature = "flux"))]
        "flux" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'flux' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "flux")]
        "flux2" => Ok(Box::new(crate::flux2_backend::Flux2Backend::new(&spec.id))),
        #[cfg(not(feature = "flux"))]
        "flux2" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a CUDA build with the 'flux' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "llm")]
        "llm" => Ok(Box::new(crate::llm_backend::LlmBackend::new_llama(&spec.id))),
        #[cfg(not(feature = "llm"))]
        "llm" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'llm' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "tts")]
        "kokoro" => Ok(Box::new(crate::kokoro_backend::KokoroBackend::new_kokoro(
            &spec.id,
        ))),
        #[cfg(not(feature = "tts"))]
        "kokoro" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'tts' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "indextts")]
        "indextts" => Ok(Box::new(
            crate::indextts_backend::IndexTtsBackend::new_indextts(&spec.id),
        )),
        #[cfg(not(feature = "indextts"))]
        "indextts" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'indextts' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "video")]
        "h3" => Ok(Box::new(crate::h3_backend::H3Backend::new_h3(&spec.id))),
        #[cfg(not(feature = "video"))]
        "h3" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'video' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "audio")]
        "sa3" => Ok(Box::new(crate::sa3_backend::Sa3Backend::new_sa3(&spec.id))),
        #[cfg(feature = "audio")]
        "moss" => Ok(Box::new(crate::moss_backend::MossBackend::new_moss(&spec.id))),
        #[cfg(feature = "audio")]
        "woosh" => Ok(Box::new(crate::woosh_backend::WooshBackend::new_woosh(
            &spec.id,
        ))),
        #[cfg(feature = "audio")]
        "ace" => Ok(Box::new(crate::ace_backend::AceBackend::new_ace(&spec.id))),
        #[cfg(not(feature = "audio"))]
        "moss" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'audio' cargo feature",
            spec.id
        ))),
        #[cfg(not(feature = "audio"))]
        "sa3" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'audio' cargo feature",
            spec.id
        ))),
        #[cfg(not(feature = "audio"))]
        "woosh" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'audio' cargo feature",
            spec.id
        ))),
        #[cfg(not(feature = "audio"))]
        "ace" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'audio' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "mesh")]
        "trellis" => Ok(Box::new(crate::trellis_backend::TrellisBackend::new_trellis(
            &spec.id,
        ))),
        #[cfg(not(feature = "mesh"))]
        "trellis" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'mesh' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "python-backends")]
        "flashworld" => Ok(Box::new(
            crate::world_backend::WorldBackend::new_flashworld(&spec.id),
        )),
        #[cfg(any(feature = "audio", feature = "python-backends"))]
        "music3" => Ok(Box::new(
            crate::music3_backend::Music3Backend::new_music3(&spec.id),
        )),
        "music3-python" => Ok(Box::new(
            crate::music3_backend::Music3Backend::new_music3_python(&spec.id),
        )),
        #[cfg(feature = "matte-native")]
        "matte-native" => Ok(Box::new(crate::matte_backend::MatteBackend::new_native(
            &spec.id,
        ))),
        #[cfg(not(feature = "matte-native"))]
        "matte-native" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'matte-native' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "depth-native")]
        "depth-native" => Ok(Box::new(
            crate::depth_backend::DepthBackend::new_native(&spec.id),
        )),
        #[cfg(not(feature = "depth-native"))]
        "depth-native" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'depth-native' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "segment-native")]
        "segment-native" => Ok(Box::new(
            crate::segment_backend::SegmentBackend::new_native(&spec.id),
        )),
        #[cfg(not(feature = "segment-native"))]
        "segment-native" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'segment-native' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "python-backends")]
        "depth" => Ok(Box::new(
            crate::depth_backend::DepthBackend::new_subprocess(&spec.id),
        )),
        #[cfg(feature = "rig-native")]
        "rig-native" => Ok(Box::new(
            crate::rig_native_backend::RigNativeBackend::new(&spec.id),
        )),
        #[cfg(not(feature = "rig-native"))]
        "rig-native" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'rig-native' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "python-backends")]
        "rig-oracle" => Ok(Box::new(
            crate::rig_backend::RigBackend::new_subprocess(&spec.id),
        )),
        #[cfg(feature = "motion-native")]
        "motion-native" => Ok(Box::new(
            crate::motion_native_backend::MotionNativeBackend::new(&spec.id),
        )),
        #[cfg(not(feature = "motion-native"))]
        "motion-native" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'motion-native' cargo feature",
            spec.id
        ))),
        #[cfg(feature = "python-backends")]
        "motion-oracle" => Ok(Box::new(
            crate::motion_backend::MotionBackend::new_subprocess(&spec.id),
        )),
        #[cfg(not(any(feature = "audio", feature = "python-backends")))]
        "music3" => Err(AssetAiError::Unavailable(format!(
            "model {} needs a build with the 'audio' cargo feature",
            spec.id
        ))),
        #[cfg(not(feature = "python-backends"))]
        "flashworld" | "depth" | "rig-oracle" | "motion-oracle" => {
            Err(AssetAiError::Unavailable(format!(
                "model {} needs a build with the 'python-backends' cargo feature",
                spec.id
            )))
        }
        other => Err(AssetAiError::Unavailable(format!(
            "no backend {other:?} in this build"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{backend_compiled, create_backend, model_availability};
    #[cfg(not(feature = "python-backends"))]
    use super::backend_provisioned;
    #[cfg(not(feature = "python-backends"))]
    use crate::error::AssetAiError;
    use crate::gpu::GpuInfo;
    use crate::registry::{Domain, ModelSpec};

    fn gpu(vram_total_mb: Option<u64>, compute_cap: Option<f64>) -> GpuInfo {
        GpuInfo {
            name: Some("fixture".to_string()),
            vram_free_mb: vram_total_mb,
            vram_total_mb,
            compute_cap,
        }
    }

    fn spec(backend: &str, available: bool, vram_gb: Option<f64>) -> ModelSpec {
        ModelSpec {
            id: "fixture".into(),
            domain: Domain::Image,
            backend: backend.into(),
            available,
            gated: false,
            vram_gb,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            files: Vec::new(),
        }
    }

    #[cfg(feature = "indextts")]
    #[test]
    fn indextts_is_advertised_when_its_backend_is_compiled() {
        assert!(backend_compiled("indextts"));
    }

    #[cfg(not(feature = "indextts"))]
    #[test]
    fn indextts_is_not_advertised_without_its_backend_feature() {
        assert!(!backend_compiled("indextts"));
    }

    #[test]
    fn availability_applies_total_vram_and_reserve_at_the_exact_boundary() {
        let model = spec("testpattern", true, Some(10.0));
        assert!(model_availability(&model, &gpu(Some(12 * 1024), None), 2 * 1024).is_ok());

        let reason = model_availability(&model, &gpu(Some(12 * 1024 - 1), None), 2 * 1024)
            .expect_err("one MB below the required budget must be unavailable");
        assert!(reason.contains("12288 MB"), "got: {reason}");
        assert!(reason.contains("12287 MB"), "got: {reason}");

        // Development machines without a VRAM probe retain the previous
        // behavior: absence of evidence is not a fabricated hard limit.
        assert!(model_availability(&model, &GpuInfo::default(), 2 * 1024).is_ok());
    }

    #[test]
    fn declared_gpu_requirements_fail_closed_at_exact_boundaries() {
        let mut model = spec("testpattern", true, Some(20.0));
        model.min_vram_gb = Some(22.0);
        model.min_compute_cap = Some(8.9);

        assert!(model_availability(
            &model,
            &gpu(Some(22 * 1024), Some(8.9)),
            2 * 1024
        )
        .is_ok());

        let unknown = model_availability(&model, &GpuInfo::default(), 2 * 1024)
            .expect_err("declared requirements must reject unknown hardware");
        assert!(unknown.contains("GPU is unknown"), "got: {unknown}");

        let low_vram = model_availability(
            &model,
            &gpu(Some(22 * 1024 - 1), Some(8.9)),
            2 * 1024,
        )
        .expect_err("one MB below the hard VRAM minimum must reject");
        assert!(low_vram.contains("requires >= 22 GB VRAM"), "got: {low_vram}");

        let low_cap = model_availability(
            &model,
            &gpu(Some(22 * 1024), Some(8.899)),
            2 * 1024,
        )
        .expect_err("compute capability below the minimum must reject");
        assert!(low_cap.contains("capability >= 8.9"), "got: {low_cap}");

        let missing_cap = model_availability(
            &model,
            &gpu(Some(22 * 1024), None),
            2 * 1024,
        )
        .expect_err("an unknown required compute capability must reject");
        assert!(missing_cap.contains("GPU is unknown"), "got: {missing_cap}");
    }

    #[test]
    fn availability_reason_precedence_is_stable() {
        let disabled = spec("not-a-backend", false, Some(100.0));
        let reason = model_availability(&disabled, &gpu(Some(1), None), 2 * 1024).unwrap_err();
        assert_eq!(reason, "disabled in registry");

        let missing = spec("not-a-backend", true, Some(100.0));
        let reason = model_availability(&missing, &gpu(Some(1), None), 2 * 1024).unwrap_err();
        assert!(reason.contains("not compiled"), "got: {reason}");
    }

    #[cfg(not(feature = "python-backends"))]
    #[test]
    fn python_backends_are_neither_advertised_provisioned_nor_constructible() {
        for name in ["flashworld", "depth", "rig-oracle", "motion-oracle"] {
            assert!(!backend_compiled(name), "{name}");
            assert!(!backend_provisioned(name), "{name}");
            let error = match create_backend(&spec(name, true, None)) {
                Ok(_) => panic!("{name} unexpectedly constructed"),
                Err(error) => error,
            };
            assert!(
                matches!(error, AssetAiError::Unavailable(ref message) if message.contains("python-backends")),
                "{name}: {error}"
            );
        }
    }

    #[cfg(feature = "python-backends")]
    #[test]
    fn python_backends_are_compiled_and_constructible_when_opted_in() {
        for name in [
            "flashworld",
            "music3",
            "depth",
            "rig-oracle",
            "motion-oracle",
        ] {
            assert!(backend_compiled(name), "{name}");
            assert!(create_backend(&spec(name, true, None)).is_ok(), "{name}");
        }
    }
}
