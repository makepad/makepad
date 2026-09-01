//! The `flux` backend: real image generation through libs/diffusion's
//! `FluxPipeline`. Compiled only with the `flux` cargo feature so the service
//! scaffold (and CI) never needs the ggml/mlx toolchain.
//!
//! Integration notes (the CUDA port work lands in libs/diffusion, not here):
//! - libs/diffusion is its own cargo workspace; a plain path dependency from
//!   this crate works fine across the workspace boundary.
//! - The canonical models are combined single-file FP8 checkpoints (DiT FP8
//!   + T5XXL FP8 + CLIP-L FP16 + VAE F32 in one safetensors), cached under
//!   the ComfyUI-style `checkpoints/` root that `ComfyModelRoots::new`
//!   resolves against our cache dir. There is no split-file or BF16 tier
//!   behind these IDs and no CPU/Metal fallback: availability is gated on
//!   real CUDA FP8 execution capability (`flux_fp8_provisioned`).
//! - Warm residency: the loaded `FluxPipeline` lives on THE process-shared
//!   keep-alive worker thread (`FluxWorker::shared`; the pipeline is not
//!   `Send`: Metal runtime types on macOS) that survives across jobs and
//!   across backend instances. The server keeps one backend instance per
//!   model id (flux1-schnell AND flux1-dev), but all of them funnel into
//!   the one worker: the CUDA buffer pool and device weight cache are
//!   thread-local, so flux residency must sit on a single thread for a
//!   model switch to be able to evict what it replaces. Same model file +
//!   same image size reuse the pipeline outright — an unchanged prompt
//!   skips every load phase, and a changed prompt re-encodes only the text
//!   conditioning against the RESIDENT text-encoder weights
//!   (`ensure_prompts_with_hooks`; zero disk reads, warm device caches).
//!   A size change rebuilds the pipeline but reuses the device weight
//!   caches; a MODEL switch (schnell<->dev) additionally evicts ALL of the
//!   outgoing checkpoint's device caches — every component keys on the
//!   combined checkpoint path, and a 24GB card cannot hold two ~16GB
//!   resident FP8 stacks.
//! - Progress + cancellation ride `FluxRunHooks`: the pipeline emits labeled
//!   phases (tokenize / load+encode clip_l+t5 / load unet / "denoise k/N" /
//!   vae-decode) with phase-local fractions this file remaps onto the
//!   overall job, and polls the job's `CancelToken` between phases/steps —
//!   a raised flag unwinds as job state "cancelled" within a step boundary.

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::registry::ModelSpec;
use makepad_ai_flux::comfy::{FluxGenerationConfig, FluxPrompts};
use makepad_ai_flux::flux::{ComfyModelRoots, FluxPromptToImagePlan, FluxResolvedBundle};
use makepad_ai_flux::flux_lora::{FluxLoraRef, FluxLoraStack};
use makepad_ai_flux::flux_pipeline::encode_png_rgb;
use makepad_ai_common::DiffusionError;
use std::path::PathBuf;

pub struct FluxBackend {
    model_id: String,
    ready: Option<ReadyFiles>,
    /// Handle to THE process-shared flux worker thread (plus the shared
    /// handle's generation, for respawn-after-panic bookkeeping). The server
    /// keeps one backend instance per MODEL id, but every flux model must
    /// run on one shared worker thread: the CUDA buffer pool and the device
    /// weight cache are thread-local, so per-model worker threads would
    /// stack a full unet (~24GB device + ~22GB host) PER MODEL with no way
    /// to evict across threads — a schnell/dev switch has to happen on the
    /// thread that holds the old model's residency. The worker keys warm
    /// reuse by model files (`serves_plan`) and evicts the outgoing unet's
    /// device cache on a model switch, so switching costs one cold rebuild
    /// while shared files (vae/text encoders) stay device-warm.
    worker: Option<(u64, flux_worker::FluxWorker)>,
}

/// File name (relative to the ComfyUI-style `checkpoints/` dir) resolved
/// from the registry entry once `ensure_loaded` has the file on disk. The
/// canonical flux models are combined single-file FP8 checkpoints carrying
/// all four components (DiT FP8 + T5XXL FP8 + CLIP-L FP16 + VAE F32).
struct ReadyFiles {
    cache_dir: PathBuf,
    checkpoint_name: String,
}

/// True when this machine can actually execute the canonical FP8 flux tier:
/// a CUDA device for the raw-F8 resident dense path. Metal/CPU builds (mac
/// dev machines, CI) must not advertise flux as available merely because the
/// cargo `flux` feature compiled — there is deliberately no BF16/CPU service
/// fallback behind the ready state. The fleet is sm89/sm120; an exotic
/// pre-Ampere CUDA card would pass this gate but fail the first generate
/// explicitly (fail closed at the job, never a silent degrade).
pub fn flux_fp8_provisioned() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(makepad_ai_common::backend::gpu_device_available)
}

impl FluxBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            ready: None,
            worker: None,
        }
    }
}

fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        // A raised cancel flag unwinds the pipeline as DiffusionError::
        // Cancelled — job state "cancelled", not an error.
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        err => AssetAiError::Backend(format!("flux: {err}")),
    }
}

/// Finds the registry file cached under `<dir_prefix>/...` (optionally
/// requiring the name to contain `name_hint` — text_encoders holds both
/// clip_l and t5xxl) and returns its file name, which is what
/// `FluxWorkflowFiles` wants: `ComfyModelRoots` re-adds the directory.
fn name_under(
    spec: &ModelSpec,
    dir_prefix: &str,
    name_hint: Option<&str>,
) -> Result<String, AssetAiError> {
    spec.files
        .iter()
        .find_map(|file| {
            file.cache_as
                .strip_prefix(&format!("{dir_prefix}/"))
                .filter(|rest| name_hint.map_or(true, |hint| rest.contains(hint)))
                .map(|rest| rest.to_string())
        })
        .ok_or_else(|| {
            AssetAiError::Backend(format!(
                "model {}: registry has no file cached under {}/ (hint {:?})",
                spec.id, dir_prefix, name_hint
            ))
        })
}

impl ContentBackend for FluxBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Truthful residency: holding a live handle to the shared flux worker
    /// keeps that thread — and its thread-local device weight cache + CUDA
    /// pool plus the ~22GB host arena — alive. Without this, the service's
    /// VRAM admission gate cannot see (or evict) ~30GB after an image job,
    /// and a subsequent 90GB-class video load is refused with "nothing left
    /// to evict".
    fn is_resident(&self) -> bool {
        self.worker.is_some()
    }

    /// Serialized teardown: drop the shared registration AND this handle so
    /// the worker thread exits (its TLS teardown releases the device cache
    /// and pool). The server verifies the freed VRAM via fresh NVML reads
    /// before admitting the next heavy load.
    fn unload(&mut self) -> Result<(), AssetAiError> {
        let Some((generation, worker)) = self.worker.take() else {
            return Ok(());
        };
        // Ask the pipeline thread to free its own residency and WAIT for the
        // acknowledgement before reporting the model retired.
        //
        // Dropping the handle is not a release. The device weight cache and
        // the CUDA idle pool are thread-locals of the pipeline thread, so the
        // only thing a drop does is ask that thread to exit eventually — and
        // on the fleet it returned nothing at all: a retired flux1-schnell
        // left 16.6 GB of a 24 GB card allocated while `/models` reported
        // `models_loaded: []`, the server's own `vram-release` on the WRONG
        // thread said "evicted 0 cached weight buffers", and every heavy job
        // after it died on "insufficient VRAM ... only 6901 MB free".
        let released = worker.release();
        flux_worker::FluxWorker::retire_shared(generation);
        drop(worker);
        match released {
            Ok(line) => {
                eprintln!("[asset-ai] {line}");
                Ok(())
            }
            // Already dead: its own exit path ran the same release.
            Err(flux_worker::FluxWorkerError::WorkerGone(message)) => {
                eprintln!("[asset-ai] flux release skipped: {message}");
                Ok(())
            }
            Err(flux_worker::FluxWorkerError::Cancelled) => Ok(()),
            // A release that did not happen must fail the retire loudly: the
            // admission gate is about to promise the VRAM to another model.
            Err(flux_worker::FluxWorkerError::Other(message)) => {
                Err(AssetAiError::Backend(message))
            }
        }
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // Fail closed before touching files: without real FP8 execution
        // capability this backend must never look loadable.
        if !flux_fp8_provisioned() {
            return Err(AssetAiError::Unavailable(format!(
                "model {}: the canonical FLUX FP8 tier requires a CUDA device (no CPU/Metal fallback)",
                self.model_id
            )));
        }
        // Idempotent: after the first success only the presence check runs.
        ctx.ensure_files()?;
        self.ready = Some(ReadyFiles {
            cache_dir: ctx.cache_dir.to_path_buf(),
            checkpoint_name: name_under(ctx.spec, "checkpoints", None)?,
        });
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let ready = self.ready.as_ref().ok_or_else(|| {
            AssetAiError::Backend("flux backend used before ensure_loaded".to_string())
        })?;
        let width = params.width.unwrap_or(512);
        let height = params.height.unwrap_or(512);
        // Model-aware step default: schnell is 4-step distilled; dev-class
        // models want ~20 (a 4-step dev render is mush).
        let default_steps = if self.model_id.contains("schnell") { 4 } else { 20 };
        let steps = params.steps.unwrap_or(default_steps);
        progress("prepare", 0.01);
        cancel.check()?;
        let roots = ComfyModelRoots::new(&ready.cache_dir);
        let bundle = FluxResolvedBundle::from_checkpoint(
            roots.checkpoints_dir.join(&ready.checkpoint_name),
        )
        .map_err(diffusion_err)?;
        // LoRA adapters live in the operator drop-box next to the model
        // cache; a missing name fails the job loudly with the available
        // list rather than rendering an un-adapted image.
        let loras = FluxLoraStack::new(
            crate::backend::resolve_loras(
                &crate::backend::lora_dir(&ready.cache_dir),
                &params.loras,
            )?
            .into_iter()
            .map(|(name, path, strength)| FluxLoraRef {
                name,
                path,
                strength,
            })
            .collect(),
        );
        let plan = FluxPromptToImagePlan::from_files_with_loras(
            bundle,
            FluxPrompts {
                clip_l: params.prompt.clone(),
                t5xxl: params.prompt.clone(),
                negative: params.negative_prompt.clone(),
            },
            FluxGenerationConfig {
                width,
                height,
                batch_size: 1,
                seed: params.seed,
                steps,
                cfg: 1.0,
                denoise: 1.0,
                guidance: params.guidance.unwrap_or(3.5),
                sampler_name: "euler".to_string(),
                scheduler: "simple".to_string(),
            },
            loras,
        )
        .map_err(diffusion_err)?;

        // All GPU work runs on the keep-alive flux worker thread: it owns
        // the resident FluxPipeline (warm across jobs) and keeps every flux
        // job on one thread, which the CUDA backend wants anyway (buffer
        // pool, device weight cache and step graph are thread-local).
        // Progress/cancel semantics are unchanged: the worker forwards the
        // pipeline's labeled phases already remapped onto the overall job,
        // and polls this job's CancelToken between phases/steps.
        if self.worker.is_none() {
            self.worker = Some(flux_worker::FluxWorker::shared()?);
        }
        let job = flux_worker::GenerateJob {
            plan,
            width,
            height,
            seed: params.seed,
            steps: steps as usize,
            guidance: params.guidance.unwrap_or(3.5),
        };
        let (generation, worker) = self.worker.as_ref().expect("flux worker just acquired");
        let run = match worker.generate(job, cancel.clone(), progress) {
            Ok(run) => run,
            Err(flux_worker::FluxWorkerError::Cancelled) => return Err(AssetAiError::Cancelled),
            Err(flux_worker::FluxWorkerError::Other(message)) => {
                return Err(AssetAiError::Backend(format!("flux: {message}")));
            }
            Err(flux_worker::FluxWorkerError::WorkerGone(message)) => {
                // The worker thread died (panic in the pipeline). Retire the
                // shared worker for OUR generation only — another backend
                // instance may already have triggered a respawn — and drop
                // the handle so the next job reacquires fresh + cold-loads.
                flux_worker::FluxWorker::retire_shared(*generation);
                self.worker = None;
                return Err(AssetAiError::Backend(format!("flux: {message}")));
            }
        };
        cancel.check()?;
        progress("png-encode", 0.96);
        let png = encode_png_rgb(&run.image.image, run.image.width, run.image.height)
            .map_err(diffusion_err)?;
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes: png,
        }])
    }
}

// ---------------------------------------------------------------------------
// FluxWorker: the resident pipeline on a keep-alive worker thread
// ---------------------------------------------------------------------------

mod flux_worker {
    use crate::backend::{CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_flux::flux::FluxPromptToImagePlan;
    use makepad_ai_flux::flux_pipeline::{
        FluxPipeline, FluxPipelineGenerateRun, FluxRunHooks,
    };
    use makepad_ai_common::DiffusionError;
    use std::sync::mpsc;

    /// One generation, fully resolved by the backend (paths in the plan,
    /// size/seed/steps/guidance from the request). Everything here is Send;
    /// the pipeline itself never leaves the worker thread.
    pub struct GenerateJob {
        pub plan: FluxPromptToImagePlan,
        pub width: u32,
        pub height: u32,
        pub seed: u64,
        pub steps: usize,
        pub guidance: f32,
    }

    /// Worker-side result errors, pre-mapped to a Send-safe shape (the
    /// backend only distinguishes cancellation from a message anyway).
    pub enum FluxWorkerError {
        Cancelled,
        Other(String),
        /// The worker thread or its channel is gone (a panic inside the
        /// pipeline) — the backend drops its handle and respawns cold.
        WorkerGone(String),
    }

    /// Streamed back to the blocked caller while a job runs on the worker
    /// thread: forwarded progress (label + OVERALL job fraction), then
    /// exactly one Done.
    enum WorkerEvent {
        Progress(String, f64),
        Done(Result<FluxPipelineGenerateRun, FluxWorkerError>),
    }

    struct WorkerMsg {
        job: GenerateJob,
        cancel: CancelToken,
        events: mpsc::Sender<WorkerEvent>,
    }

    /// What the backend may ask the pipeline thread to do.
    ///
    /// `Release` is the whole point of this enum: the device weight cache and
    /// the CUDA idle block pool are THREAD-LOCALS of the pipeline thread, so
    /// nothing on the server's worker thread can free them. Retiring the model
    /// used to consist of dropping this handle and hoping the thread noticed;
    /// that returned no VRAM the admission gate could see.
    enum WorkerCmd {
        Run(WorkerMsg),
        /// Free every device allocation this thread owns and acknowledge with
        /// a one-line summary. Sent by `FluxBackend::unload`, which BLOCKS on
        /// the acknowledgement.
        Release(mpsc::Sender<String>),
    }

    /// Handle to the dedicated pipeline thread. `FluxPipeline` is not `Send`
    /// (Metal runtime types on macOS), so it is built and used only on that
    /// thread; this handle is Send. There is ONE such thread per process,
    /// shared by every flux model's backend instance through
    /// [`FluxWorker::shared`]: the CUDA buffer pool and device weight cache
    /// are thread-local, so all flux GPU residency must live on a single
    /// thread for model switches to be able to evict what they replace.
    /// The shared registration keeps a clone of the sender, so the thread —
    /// and its resident weights — live for the rest of the process.
    #[derive(Clone)]
    pub struct FluxWorker {
        tx: mpsc::Sender<WorkerCmd>,
    }

    /// The process-shared worker: `generation` counts respawns so a backend
    /// that saw its worker die retires exactly the instance it used (not a
    /// successor another backend already respawned).
    static SHARED_WORKER: std::sync::Mutex<(u64, Option<FluxWorker>)> =
        std::sync::Mutex::new((0, None));

    /// Upper bound on a release acknowledgement. Generous because the release
    /// queues behind whatever the thread is doing; a real free of a resident
    /// FP8 stack is well under a second.
    const RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    /// Frees EVERY device allocation this pipeline thread owns — on this
    /// thread, the only place they can be freed from.
    ///
    /// Three separate owners, and the old teardown reached none of them from
    /// the outside: the resident pipeline's four checkpoint weight-cache
    /// namespaces (`evict_device_caches`), any other dense-linear weight
    /// buffer left on this thread, and the idle CUDA block pool that every
    /// activation is recycled through. Dropping the pipeline alone releases
    /// only its HOST arenas by design.
    ///
    /// Safe to call at any point where nothing is generating: the caches are
    /// re-uploaded on the next miss, which is what they are for. Returns the
    /// one-line summary that goes into the service log.
    fn release_device_residency(warm: &mut Option<FluxPipeline>) -> String {
        let checkpoint = match warm.take() {
            Some(pipeline) => {
                let freed = pipeline.evict_device_caches();
                // Host weight arenas (~16 GB raw payload) go with the drop.
                drop(pipeline);
                freed
            }
            None => 0,
        };
        // `_if_loaded` semantics: a thread that never touched CUDA must not
        // initialize a device merely to discover it has nothing to release.
        let (residual, trouble) =
            match makepad_ai_common::backend::release_gpu_runtime_namespaces(&[""]) {
                Ok(count) => (count, String::new()),
                Err(error) => (0, format!(", release error: {error}")),
            };
        makepad_ai_common::backend::gpu_pool_clear();
        format!(
            "flux vram-release on the pipeline thread: {checkpoint} checkpoint weight buffers,              {residual} residual weight buffers, idle pool cleared{trouble}"
        )
    }

    impl FluxWorker {
        /// Returns (generation, handle) of the process-shared worker,
        /// spawning it if this is the first use (or the first after a
        /// retirement). Jobs are strictly sequential at the server level, so
        /// the lock is uncontended bookkeeping, not a scheduling point.
        pub fn shared() -> Result<(u64, Self), AssetAiError> {
            let mut shared = SHARED_WORKER.lock().unwrap();
            if shared.1.is_none() {
                shared.0 += 1;
                shared.1 = Some(Self::spawn()?);
            }
            Ok((shared.0, shared.1.clone().expect("shared flux worker set")))
        }

        /// Drops the shared registration IF it still is `generation` — the
        /// caller's worker thread died, and the next [`Self::shared`] call
        /// respawns. A stale generation is a no-op (someone else already
        /// retired it and a fresh worker may be live).
        pub fn retire_shared(generation: u64) {
            let mut shared = SHARED_WORKER.lock().unwrap();
            if shared.0 == generation {
                shared.1 = None;
            }
        }

        fn spawn() -> Result<Self, AssetAiError> {
            let (tx, rx) = mpsc::channel::<WorkerCmd>();
            std::thread::Builder::new()
                .name("flux-pipeline".to_string())
                .spawn(move || {
                    // Warm state: survives between jobs for the life of the
                    // backend instance. Dropped early only on a model-file/
                    // size change (rebuild) or a non-cancel run failure.
                    let mut warm: Option<FluxPipeline> = None;
                    while let Ok(cmd) = rx.recv() {
                        match cmd {
                            WorkerCmd::Run(WorkerMsg { job, cancel, events }) => {
                                let result = run_generate(&mut warm, job, &cancel, &events);
                                let _ = events.send(WorkerEvent::Done(result));
                            }
                            WorkerCmd::Release(ack) => {
                                let _ = ack.send(release_device_residency(&mut warm));
                            }
                        }
                    }
                    // Every sender dropped -> the backend retired without
                    // waiting for a release (a panic path, or a process
                    // teardown). Free on the way out too, and SAY so: a thread
                    // that exits silently here is how 16.6 GB stayed on a 24 GB
                    // card with `models_loaded: []`.
                    eprintln!(
                        "[asset-ai] flux worker thread exiting: {}",
                        release_device_residency(&mut warm)
                    );
                })
                .map_err(|e| AssetAiError::Backend(format!("spawn flux worker: {e}")))?;
            Ok(Self { tx })
        }

        /// Frees the thread's device residency and BLOCKS for the
        /// acknowledgement, so the caller can honestly report the VRAM back.
        /// Queues behind an in-flight generation like any other command.
        pub fn release(&self) -> Result<String, FluxWorkerError> {
            let (ack_tx, ack_rx) = mpsc::channel();
            self.tx.send(WorkerCmd::Release(ack_tx)).map_err(|_| {
                FluxWorkerError::WorkerGone("flux worker thread is gone".to_string())
            })?;
            ack_rx
                .recv_timeout(RELEASE_TIMEOUT)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Disconnected => FluxWorkerError::WorkerGone(
                        "flux worker dropped the release reply".to_string(),
                    ),
                    mpsc::RecvTimeoutError::Timeout => FluxWorkerError::Other(format!(
                        "flux worker did not acknowledge the device release within {}s",
                        RELEASE_TIMEOUT.as_secs()
                    )),
                })
        }

        /// Blocks until the generation finishes, forwarding progress events
        /// into `progress`. Cancellation rides the shared token, polled on
        /// the worker thread between phases/steps.
        pub fn generate(
            &self,
            job: GenerateJob,
            cancel: CancelToken,
            progress: ProgressSink,
        ) -> Result<FluxPipelineGenerateRun, FluxWorkerError> {
            let (event_tx, event_rx) = mpsc::channel();
            self.tx
                .send(WorkerCmd::Run(WorkerMsg {
                    job,
                    cancel,
                    events: event_tx,
                }))
                .map_err(|_| {
                    FluxWorkerError::WorkerGone("flux worker thread is gone".to_string())
                })?;
            loop {
                match event_rx.recv() {
                    Ok(WorkerEvent::Progress(name, fraction)) => progress(&name, fraction),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => {
                        return Err(FluxWorkerError::WorkerGone(
                            "flux worker dropped the reply".to_string(),
                        ))
                    }
                }
            }
        }
    }

    fn worker_err(err: DiffusionError) -> FluxWorkerError {
        match err {
            DiffusionError::Cancelled => FluxWorkerError::Cancelled,
            err => FluxWorkerError::Other(err.to_string()),
        }
    }

    /// The warm-residency core, on the worker thread. The pipeline emits
    /// phase-local fractions 0..=1 with labeled stages ("text-encode t5",
    /// "denoise 2/4", "vae-decode"); remapped onto the overall job here:
    /// load+text 0.02..0.35, denoise+vae 0.35..0.95 (the backend adds
    /// png-encode at 0.96).
    ///
    /// A resident pipeline serving the same model files at the same image
    /// size skips the load phases entirely (unchanged prompts skip text
    /// encoding too — the bench-grade warm path); a prompt change
    /// re-encodes only the text conditioning. Anything else rebuilds —
    /// dropping the old pipeline FIRST so its host weight arena (~22GB) is
    /// freed before the new load allocates.
    fn run_generate(
        warm: &mut Option<FluxPipeline>,
        job: GenerateJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> Result<FluxPipelineGenerateRun, FluxWorkerError> {
        let progress = |name: &str, fraction: f64| {
            let _ = events.send(WorkerEvent::Progress(name.to_string(), fraction));
        };
        let is_cancelled = || cancel.is_cancelled();

        if !warm.as_ref().is_some_and(|pipeline| {
            pipeline.serves_plan(&job.plan, Some(job.width), Some(job.height))
        }) {
            if let Some(old) = warm.take() {
                // A rebuild on the shared worker. On a MODEL switch
                // (schnell<->dev) the outgoing checkpoint's device weight
                // caches must go first: every component (DiT/T5/CLIP/VAE)
                // keys on the combined checkpoint path, never evicts on its
                // own, and a 24GB card cannot hold two resident FP8 stacks
                // (~16GB each). A size-only rebuild of the SAME model skips
                // the evict and re-uses the device weights outright.
                // Dropping the pipeline then frees its host weight arenas
                // (~16GB raw payload) before the new load allocates.
                //
                // A LoRA change counts as a model switch: merged weights get
                // their own cache namespace, so without the evict the
                // outgoing (pristine or previously-adapted) unet would stay
                // resident next to the incoming one.
                if old.diffusion_model_path() != job.plan.bundle.diffusion_model_path
                    || old.lora_fingerprint() != job.plan.loras.fingerprint()
                {
                    old.evict_device_caches();
                }
                drop(old);
            }
        }
        let load_result = {
            let mut load_progress =
                |name: &str, fraction: f64| progress(name, 0.02 + fraction * 0.33);
            let mut hooks = FluxRunHooks {
                progress: &mut load_progress,
                cancel: &is_cancelled,
            };
            match warm.as_mut() {
                Some(pipeline) => pipeline
                    .ensure_prompts_with_hooks(&job.plan.prompts, Some(&mut hooks))
                    .map(|_| None),
                None => FluxPipeline::load_with_hooks(
                    job.plan,
                    Some(job.width),
                    Some(job.height),
                    Some(&mut hooks),
                )
                .map(|(pipeline, _load_timing)| Some(pipeline)),
            }
        };
        match load_result {
            Ok(Some(pipeline)) => *warm = Some(pipeline),
            Ok(None) => {}
            Err(err) => {
                // A cancel unwind is designed-clean — keep the warm pipeline
                // (its state only mutates after a successful encode). Any
                // other failure: drop residency, reload cold next job.
                if !matches!(err, DiffusionError::Cancelled) {
                    *warm = None;
                }
                return Err(worker_err(err));
            }
        }
        if cancel.is_cancelled() {
            return Err(FluxWorkerError::Cancelled);
        }
        let run = {
            let pipeline = warm.as_ref().expect("warm flux pipeline just ensured");
            let mut gen_progress =
                |name: &str, fraction: f64| progress(name, 0.35 + fraction * 0.60);
            let mut hooks = FluxRunHooks {
                progress: &mut gen_progress,
                cancel: &is_cancelled,
            };
            pipeline.generate_with_hooks(job.seed, job.steps, job.guidance, Some(&mut hooks))
        };
        match run {
            Ok(run) => Ok(run),
            Err(err) => {
                // Same policy as the load: cancels keep the warm pipeline,
                // real mid-run failures drop residency rather than trust
                // half-finished state.
                if !matches!(err, DiffusionError::Cancelled) {
                    *warm = None;
                }
                Err(worker_err(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::flux_worker::{FluxWorker, FluxWorkerError};

    /// The retire path's contract, and the whole bug in one assertion:
    /// `release()` runs ON the pipeline thread and BLOCKS for its
    /// acknowledgement. Dropping the handle — all the old `unload` did — is
    /// not a release, because the device weight cache and the CUDA idle block
    /// pool are that thread's thread-locals and nothing outside it can free
    /// them.
    ///
    /// Runs everywhere: on a machine with no CUDA the release is a no-op, and
    /// it must not initialize a device merely to discover that.
    #[test]
    fn release_round_trips_on_the_pipeline_thread() {
        let (_generation, worker) = FluxWorker::shared().expect("spawn flux worker");
        let line = match worker.release() {
            Ok(line) => line,
            Err(FluxWorkerError::Other(message))
            | Err(FluxWorkerError::WorkerGone(message)) => {
                panic!("release was not acknowledged: {message}")
            }
            Err(FluxWorkerError::Cancelled) => panic!("release reported cancellation"),
        };
        assert!(
            line.contains("vram-release on the pipeline thread"),
            "release must say what it freed and where: {line}"
        );
        assert!(
            line.contains("idle pool cleared"),
            "the CUDA idle block pool is part of the residency: {line}"
        );
    }

    /// Two releases in a row are safe: an idle worker holds nothing and says
    /// so, so an eviction that races the idle sweep cannot fail a retire.
    #[test]
    fn release_is_idempotent() {
        let (_generation, worker) = FluxWorker::shared().expect("spawn flux worker");
        assert!(worker.release().is_ok());
        assert!(worker.release().is_ok());
    }
}
