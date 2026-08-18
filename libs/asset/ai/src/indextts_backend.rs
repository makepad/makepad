//! The `indextts` backend: character-voice TTS (speech domain) through the
//! in-repo IndexTTS-2.5 port in libs/diffusion (`indextts_pipeline`).
//!
//! What it adds over the `kokoro` backend: zero-shot VOICE CLONING from a
//! reference clip, and an explicit 8-dim EMOTION VECTOR — programmatic NPC
//! states ([happy, angry, sad, afraid, disgusted, melancholic, surprised,
//! calm], each 0..=1.2, biased + sum-capped exactly like the reference
//! implementation before conditioning).
//!
//! Reference-voice resolution, in order:
//! 1. `input_b64` with content type `audio/wav` — the cloning path proper:
//!    any PCM WAV (16/24/32-bit int or f32, mono/stereo) becomes the voice.
//! 2. `voice` = a wav name in the model cache under `indextts/voices/`
//!    (e.g. `voice: "narrator"` -> `indextts/voices/narrator.wav`). Boxes
//!    drop reference clips there once and reuse them by name; clips are
//!    trimmed to the reference's 15 s cap downstream.
//! 3. no voice given -> `indextts/voices/default.wav` if present, else a
//!    helpful error (this backend cannot invent a voice — cloning IS the
//!    model).
//!
//! Request: `{model: "indextts-2.5", text, voice?, emotion?, speed?, seed?,
//! input_b64?}` -> one `audio/wav` artifact (mono 16-bit PCM, 22.05 kHz).
//!
//! Like every heavy backend this file keeps the REQUEST/artifact logic
//! testable without weights: `with_stub` plugs a synthesis closure; the real
//! path (cargo feature `indextts`) rides the resident worker in
//! `indextts_synth`.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::wav::{decode_wav_to_mono_f32, encode_wav_pcm16_mono};
use std::path::PathBuf;

/// One synthesis request handed to the synth.
#[derive(Clone, Debug)]
pub struct IndexTtsJob {
    pub text: String,
    /// Resolved reference-voice audio: mono f32 samples + sample rate.
    pub reference: (Vec<f32>, u32),
    /// Normalized-by-caller? No — RAW user emotion vector (already clamped
    /// per-slot to 0..=1.2 by params parsing); the pipeline applies the
    /// reference's bias + sum cap. `None` = neutral.
    pub emotion: Option<[f32; 8]>,
    /// Speaking-rate multiplier (>1 faster; mapped to the reference's
    /// duration_factor = 1/speed downstream).
    pub speed: f32,
    pub seed: u64,
}

/// Pluggable synthesis: `(samples, sample_rate)` out.
pub type SynthFn = Box<dyn FnMut(&IndexTtsJob) -> Result<(Vec<f32>, u32), AssetAiError> + Send>;

enum Synth {
    Stub(SynthFn),
    #[cfg(feature = "indextts")]
    Real(indextts_synth::IndexTtsSynth),
}

pub struct IndexTtsBackend {
    model_id: String,
    cache_dir: Option<PathBuf>,
    synth: Synth,
}

impl IndexTtsBackend {
    /// Test/CI constructor: synthesis is the given closure, no weights needed.
    pub fn with_stub(model_id: &str, synth: SynthFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            cache_dir: None,
            synth: Synth::Stub(synth),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "indextts")]
    pub fn new_indextts(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            cache_dir: None,
            synth: Synth::Real(indextts_synth::IndexTtsSynth::new()),
        }
    }

    /// Resolves the reference voice per the priority order in the module doc.
    fn resolve_reference(
        &self,
        params: &GenerateParams,
    ) -> Result<(Vec<f32>, u32), AssetAiError> {
        if !params.input_bytes.is_empty() {
            if params.input_content_type != "audio/wav" {
                return Err(AssetAiError::Params(format!(
                    "indextts reference input must be audio/wav, got {:?}",
                    params.input_content_type
                )));
            }
            return decode_wav_to_mono_f32(&params.input_bytes)
                .map_err(|e| AssetAiError::Params(format!("reference wav: {e}")));
        }
        let name = normalize_voice_name(&params.voice)?;
        let Some(cache_dir) = &self.cache_dir else {
            return Err(AssetAiError::Backend(
                "indextts used before ensure_loaded".to_string(),
            ));
        };
        let path = cache_dir.join("indextts").join("voices").join(format!("{name}.wav"));
        let bytes = std::fs::read(&path).map_err(|_| {
            AssetAiError::Params(format!(
                "reference voice {:?} not found (expected {}); supply `input_b64` \
                 audio/wav or drop reference clips into indextts/voices/",
                name,
                path.display()
            ))
        })?;
        decode_wav_to_mono_f32(&bytes)
            .map_err(|e| AssetAiError::Backend(format!("{}: {e}", path.display())))
    }
}

/// Voice name -> bare file stem ("narrator"), default applied, path traversal
/// rejected.
pub fn normalize_voice_name(voice: &str) -> Result<String, AssetAiError> {
    let voice = voice.trim();
    let voice = voice.strip_suffix(".wav").unwrap_or(voice);
    let voice = if voice.is_empty() { "default" } else { voice };
    if !voice
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AssetAiError::Backend(format!(
            "bad voice name {voice:?} (expected e.g. \"narrator\")"
        )));
    }
    Ok(voice.to_string())
}

impl ContentBackend for IndexTtsBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        self.cache_dir = Some(ctx.cache_dir.to_path_buf());
        match &mut self.synth {
            Synth::Stub(_) => Ok(()),
            #[cfg(feature = "indextts")]
            Synth::Real(synth) => synth.ensure_loaded(ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.synth {
            Synth::Stub(_) => false,
            #[cfg(feature = "indextts")]
            Synth::Real(synth) => synth.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.synth {
            Synth::Stub(_) => Ok(()),
            #[cfg(feature = "indextts")]
            Synth::Real(synth) => synth.unload(),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let text = if params.text.trim().is_empty() {
            params.prompt.trim()
        } else {
            params.text.trim()
        };
        if text.is_empty() {
            return Err(AssetAiError::Backend(
                "speech synthesis needs non-empty `text`".to_string(),
            ));
        }
        cancel.check()?;
        progress("reference voice", 0.02);
        let reference = self.resolve_reference(params)?;
        let job = IndexTtsJob {
            text: text.to_string(),
            reference,
            emotion: params.emotion,
            speed: params.speed,
            seed: params.seed,
        };
        cancel.check()?;
        let (samples, sample_rate) = match &mut self.synth {
            Synth::Stub(synth) => synth(&job)?,
            #[cfg(feature = "indextts")]
            Synth::Real(synth) => synth.synthesize(job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if samples.is_empty() {
            return Err(AssetAiError::Backend(
                "synthesis produced no audio".to_string(),
            ));
        }
        progress("encode", 0.95);
        let wav = encode_wav_pcm16_mono(&samples, sample_rate);
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes: wav,
        }])
    }
}

// ---------------------------------------------------------------------------
// Real synthesis through libs/diffusion (feature indextts). The resident
// pipeline and all of its CUDA sessions live on the owned worker below.
// ---------------------------------------------------------------------------

#[cfg(feature = "indextts")]
mod indextts_synth {
    use super::IndexTtsJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_diffusion::indextts::INDEXTTS_SAMPLE_RATE;
    use makepad_diffusion::indextts_pipeline::{
        IndexTtsPipeline, IndexTtsSynthesisParams, IndexTtsVoice, IndexTtsWeightPaths,
    };
    use makepad_diffusion::DiffusionError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};

    /// Prepared voices kept warm per backend instance (one entry ~1.2 MB).
    const VOICE_CACHE_CAP: usize = 8;

    /// The CUDA sessions inside `IndexTtsPipeline` intentionally are not
    /// `Send`: their device tensors and TLS caches must be created, used and
    /// destroyed on one thread. The backend therefore owns only this channel
    /// handle; the pipeline never crosses a thread boundary and no unsafe
    /// `Send` promise is needed.
    pub struct IndexTtsSynth {
        weights_dir: Option<PathBuf>,
        worker: Option<IndexTtsWorker>,
    }

    enum WorkerCommand {
        Synthesize {
            weights_dir: PathBuf,
            job: IndexTtsJob,
            cancel: CancelToken,
            events: mpsc::Sender<WorkerEvent>,
        },
        Shutdown,
    }

    enum WorkerEvent {
        Progress(String, f64),
        Done(Result<(Vec<f32>, u32), AssetAiError>),
    }

    struct IndexTtsWorker {
        commands: mpsc::Sender<WorkerCommand>,
        join: Option<JoinHandle<()>>,
        resident: Arc<AtomicBool>,
    }

    fn diffusion_err(err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            other => AssetAiError::Backend(format!("indextts: {other}")),
        }
    }

    /// FNV-1a over the reference samples + rate: the voice-cache key.
    fn reference_key(samples: &[f32], rate: u32) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |byte: u8| {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for b in rate.to_le_bytes() {
            eat(b);
        }
        for s in samples {
            for b in s.to_bits().to_le_bytes() {
                eat(b);
            }
        }
        hash
    }

    impl IndexTtsWorker {
        fn spawn() -> Result<Self, AssetAiError> {
            let (commands, receiver) = mpsc::channel();
            let resident = Arc::new(AtomicBool::new(false));
            let thread_resident = resident.clone();
            let join = thread::Builder::new()
                .name("indextts-runtime".to_string())
                .spawn(move || worker_main(receiver, thread_resident))
                .map_err(|error| {
                    AssetAiError::Backend(format!("indextts worker spawn: {error}"))
                })?;
            Ok(Self {
                commands,
                join: Some(join),
                resident,
            })
        }

        fn is_resident(&self) -> bool {
            self.resident.load(Ordering::Acquire)
        }

        fn synthesize(
            &mut self,
            weights_dir: PathBuf,
            job: IndexTtsJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<(Vec<f32>, u32), AssetAiError> {
            let (events, receiver) = mpsc::channel();
            self.commands
                .send(WorkerCommand::Synthesize {
                    weights_dir,
                    job,
                    cancel: cancel.clone(),
                    events,
                })
                .map_err(|_| {
                    self.resident.store(false, Ordering::Release);
                    AssetAiError::Backend("indextts worker stopped before synthesis".into())
                })?;
            loop {
                match receiver.recv() {
                    Ok(WorkerEvent::Progress(stage, fraction)) => progress(&stage, fraction),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => {
                        self.resident.store(false, Ordering::Release);
                        return Err(AssetAiError::Backend(
                            "indextts worker stopped during synthesis".into(),
                        ));
                    }
                }
            }
        }

        /// Stop and join the owned thread. Joining is the release barrier:
        /// the pipeline/voice cache drops first, then the thread-local CUDA
        /// weight cache, activation pool, stream and handles are destroyed.
        fn shutdown(&mut self) -> Result<(), AssetAiError> {
            let Some(join) = self.join.take() else {
                self.resident.store(false, Ordering::Release);
                return Ok(());
            };
            let _ = self.commands.send(WorkerCommand::Shutdown);
            let result = join.join().map_err(|_| {
                AssetAiError::Backend("indextts worker panicked during unload".into())
            });
            self.resident.store(false, Ordering::Release);
            result
        }
    }

    impl Drop for IndexTtsWorker {
        fn drop(&mut self) {
            let _ = self.shutdown();
        }
    }

    fn send_progress(
        events: &mpsc::Sender<WorkerEvent>,
        cancel: &CancelToken,
        stage: String,
        fraction: f64,
    ) -> Result<(), DiffusionError> {
        if cancel.is_cancelled() {
            return Err(DiffusionError::Cancelled);
        }
        events
            .send(WorkerEvent::Progress(stage, fraction))
            .map_err(|_| DiffusionError::Cancelled)
    }

    fn worker_main(receiver: mpsc::Receiver<WorkerCommand>, resident: Arc<AtomicBool>) {
        // These values are born and die on this thread. In particular, never
        // return `pipeline` through a channel even on an error path.
        let mut pipeline: Option<IndexTtsPipeline> = None;
        let mut voices: Vec<(u64, IndexTtsVoice)> = Vec::new();
        while let Ok(command) = receiver.recv() {
            match command {
                WorkerCommand::Synthesize {
                    weights_dir,
                    job,
                    cancel,
                    events,
                } => {
                    let (result, retire_thread) = worker_synthesize(
                        &mut pipeline,
                        &mut voices,
                        &resident,
                        &weights_dir,
                        &job,
                        &cancel,
                        &events,
                    );
                    let _ = events.send(WorkerEvent::Done(result));
                    if retire_thread {
                        break;
                    }
                }
                WorkerCommand::Shutdown => break,
            }
        }
        // Explicit ordering makes teardown auditable: live session tensors
        // return to the TLS pool, then thread exit destroys that pool/cache.
        voices.clear();
        drop(pipeline.take());
        resident.store(false, Ordering::Release);
    }

    #[allow(clippy::too_many_arguments)]
    fn worker_synthesize(
        pipeline: &mut Option<IndexTtsPipeline>,
        voices: &mut Vec<(u64, IndexTtsVoice)>,
        resident: &AtomicBool,
        weights_dir: &PathBuf,
        job: &IndexTtsJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> (Result<(Vec<f32>, u32), AssetAiError>, bool) {
        let mut retire_thread = false;
        let result = (|| {
            // One hook maps the pipeline's phase-local [0,1] into this job's
            // band and observes the shared cancellation flag at every model
            // boundary.
            let cold = pipeline.is_none();
            if cold {
                let paths = IndexTtsWeightPaths::service_layout(weights_dir);
                let mut hook = |label: &str, fraction: f64| {
                    send_progress(
                        events,
                        cancel,
                        format!("weights: {label}"),
                        0.03 + fraction * 0.40,
                    )
                };
                match IndexTtsPipeline::load(&paths, Some(&mut hook)) {
                    Ok(loaded) => {
                        *pipeline = Some(loaded);
                        resident.store(true, Ordering::Release);
                    }
                    Err(error) => {
                        // A cancelled/failed cold load can have populated
                        // global CUDA namespaces before the owning pipeline
                        // exists. Clean them on this same thread. If cleanup
                        // itself fails, retire the thread so TLS destruction
                        // remains the unconditional fallback.
                        let cleanup = makepad_diffusion::backend::release_gpu_runtime_namespaces(
                            &["indextts_"],
                        );
                        resident.store(false, Ordering::Release);
                        return match cleanup {
                            Ok(_) => Err(diffusion_err(error)),
                            Err(cleanup) => {
                                retire_thread = true;
                                Err(AssetAiError::Backend(format!(
                                    "indextts load failed ({error}); CUDA cleanup failed ({cleanup})"
                                )))
                            }
                        };
                    }
                }
            }
            let pipeline = pipeline.as_ref().expect("pipeline resident");
            let (voice_base, synth_base) = if cold { (0.43, 0.50) } else { (0.03, 0.12) };

            let (ref_samples, ref_rate) = &job.reference;
            let key = reference_key(ref_samples, *ref_rate);
            if let Some(pos) = voices.iter().position(|(cached, _)| *cached == key) {
                let hit = voices.remove(pos);
                voices.insert(0, hit);
            } else {
                let mut hook = |label: &str, fraction: f64| {
                    send_progress(
                        events,
                        cancel,
                        label.to_string(),
                        voice_base + fraction * (synth_base - voice_base),
                    )
                };
                let voice = pipeline
                    .prepare_voice(ref_samples, *ref_rate, Some(&mut hook))
                    .map_err(diffusion_err)?;
                voices.insert(0, (key, voice));
                voices.truncate(VOICE_CACHE_CAP);
            }
            let voice = &voices[0].1;

            let mut params = IndexTtsSynthesisParams::default();
            params.emotion = job.emotion;
            params.speed = if job.speed > 0.0 { job.speed } else { 1.0 };
            params.sampling.seed = job.seed;
            let span = 0.94 - synth_base;
            let mut hook = |label: &str, fraction: f64| {
                send_progress(
                    events,
                    cancel,
                    label.to_string(),
                    synth_base + fraction * span,
                )
            };
            let samples = pipeline
                .synthesize(voice, &job.text, &params, Some(&mut hook))
                .map_err(diffusion_err)?;
            Ok((samples, INDEXTTS_SAMPLE_RATE))
        })();
        // Only a failed cold load can require retiring the thread. Once the
        // pipeline exists, cancellation/error unwinds leave it owned here;
        // the backend's standard error policy decides whether to unload it.
        (result, retire_thread)
    }

    impl IndexTtsSynth {
        pub fn new() -> Self {
            Self {
                weights_dir: None,
                worker: None,
            }
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            // Download/verify all registry files into the cache (no-ops when
            // present), then remember the weights root for the worker. The
            // ~7 GB host-RAM pipeline load itself stays lazy (first job).
            for file in ctx.spec.files.iter() {
                ctx.downloader
                    .ensure_file(file, ctx.cache_dir, ctx.download_progress, ctx.cancel)?;
            }
            let weights_dir = ctx.cache_dir.join("indextts");
            if self.weights_dir.as_ref() != Some(&weights_dir) {
                self.unload()?;
                self.weights_dir = Some(weights_dir);
            }
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.worker
                .as_ref()
                .is_some_and(IndexTtsWorker::is_resident)
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            if let Some(mut worker) = self.worker.take() {
                worker.shutdown()?;
            }
            Ok(())
        }

        pub fn synthesize(
            &mut self,
            job: IndexTtsJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<(Vec<f32>, u32), AssetAiError> {
            let weights_dir = self.weights_dir.clone().ok_or_else(|| {
                AssetAiError::Backend("indextts used before ensure_loaded".into())
            })?;
            if self.worker.is_none() {
                self.worker = Some(IndexTtsWorker::spawn()?);
            }
            self.worker
                .as_mut()
                .expect("worker just created")
                .synthesize(weights_dir, job, progress, cancel)
        }

        #[cfg(test)]
        pub fn start_idle_worker_for_test(&mut self) -> Result<(), AssetAiError> {
            if self.worker.is_none() {
                self.worker = Some(IndexTtsWorker::spawn()?);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed synthesis — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::GenerateParams;
    use crate::protocol::GenerateRequestJson;

    fn params_json(text: &str) -> GenerateRequestJson {
        GenerateRequestJson {
            model: "indextts-2.5".to_string(),
            text: Some(text.to_string()),
            ..GenerateRequestJson::default()
        }
    }

    fn reference_wav_b64() -> String {
        let wav = encode_wav_pcm16_mono(&vec![0.1f32; 2400], 24_000);
        String::from_utf8(makepad_base64::base64_encode(
            &wav,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    #[test]
    fn voice_name_normalization() {
        assert_eq!(normalize_voice_name("").unwrap(), "default");
        assert_eq!(normalize_voice_name("narrator").unwrap(), "narrator");
        assert_eq!(normalize_voice_name("guard_02.wav").unwrap(), "guard_02");
        assert!(normalize_voice_name("../evil").is_err());
        assert!(normalize_voice_name("a b").is_err());
    }

    #[test]
    fn stub_backend_is_never_reported_resident() {
        let mut backend = IndexTtsBackend::with_stub(
            "indextts-test",
            Box::new(|_| unreachable!("lifecycle test never synthesizes")),
        );
        assert!(!backend.is_resident());
        backend.unload().unwrap();
        backend.unload().unwrap();
        assert!(!backend.is_resident());
    }

    #[cfg(feature = "indextts")]
    #[test]
    fn native_worker_handle_is_send_and_idle_shutdown_joins() {
        fn assert_send<T: Send>() {}
        assert_send::<IndexTtsBackend>();

        let mut synth = indextts_synth::IndexTtsSynth::new();
        synth.start_idle_worker_for_test().unwrap();
        // A live control thread with no pipeline is not model residency.
        assert!(!synth.is_resident());
        synth.unload().unwrap();
        synth.unload().unwrap();
        assert!(!synth.is_resident());
    }

    #[test]
    fn emotion_param_validated_through_protocol() {
        // 8 floats pass through, clamped.
        let mut request = params_json("hi");
        request.emotion = Some(vec![0.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, -1.0]);
        let params = GenerateParams::from_request(&request).unwrap();
        let emotion = params.emotion.unwrap();
        assert_eq!(emotion[1], 1.2); // clamped high
        assert_eq!(emotion[2], 0.5);
        assert_eq!(emotion[7], 0.0); // clamped low
        // Wrong arity is a parameter error.
        let mut request = params_json("hi");
        request.emotion = Some(vec![1.0, 2.0]);
        assert!(GenerateParams::from_request(&request).is_err());
        // Empty array = neutral.
        let mut request = params_json("hi");
        request.emotion = Some(vec![]);
        assert!(GenerateParams::from_request(&request).unwrap().emotion.is_none());
    }

    #[test]
    fn input_b64_reference_reaches_the_job() {
        let mut backend = IndexTtsBackend::with_stub(
            "indextts-2.5",
            Box::new(|job: &IndexTtsJob| {
                assert_eq!(job.text, "hello there");
                let (samples, rate) = &job.reference;
                assert_eq!(*rate, 24_000);
                assert_eq!(samples.len(), 2400);
                assert!(job.emotion.is_some());
                assert_eq!(job.emotion.unwrap()[2], 0.8);
                Ok((vec![0.0f32; 220], 22_050))
            }),
        );
        let mut request = params_json("hello there");
        request.input_b64 = Some(reference_wav_b64());
        request.input_content_type = Some("audio/wav".to_string());
        request.emotion = Some(vec![0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let params = GenerateParams::from_request(&request).unwrap();
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        // 22.05 kHz mono PCM out.
        assert_eq!(
            u32::from_le_bytes([
                artifacts[0].bytes[24],
                artifacts[0].bytes[25],
                artifacts[0].bytes[26],
                artifacts[0].bytes[27]
            ]),
            22_050
        );
    }

    #[test]
    fn named_voice_resolves_from_cache_dir() {
        let dir = std::env::temp_dir().join(format!(
            "indextts_backend_test_{}",
            std::process::id()
        ));
        let voices = dir.join("indextts").join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        std::fs::write(
            voices.join("narrator.wav"),
            encode_wav_pcm16_mono(&vec![0.2f32; 160], 16_000),
        )
        .unwrap();

        let mut backend = IndexTtsBackend::with_stub(
            "indextts-2.5",
            Box::new(|job: &IndexTtsJob| {
                assert_eq!(job.reference.1, 16_000);
                assert_eq!(job.reference.0.len(), 160);
                Ok((vec![0.1f32; 10], 22_050))
            }),
        );
        backend.cache_dir = Some(dir.clone());
        let mut request = params_json("hi");
        request.voice = Some("narrator".to_string());
        let params = GenerateParams::from_request(&request).unwrap();
        let mut sink = |_: &str, _: f64| {};
        assert_eq!(
            backend
                .generate(&params, &mut sink, &CancelToken::new())
                .unwrap()
                .len(),
            1
        );
        // Missing voice = a parameter error that names the expected path.
        let mut request = params_json("hi");
        request.voice = Some("ghost".to_string());
        let params = GenerateParams::from_request(&request).unwrap();
        let err = match backend.generate(&params, &mut sink, &CancelToken::new()) {
            Err(err) => err,
            Ok(_) => panic!("missing voice must error"),
        };
        assert!(format!("{err:?}").contains("ghost"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrong_input_content_type_rejected() {
        let mut backend = IndexTtsBackend::with_stub(
            "indextts-2.5",
            Box::new(|_: &IndexTtsJob| panic!("must not synthesize")),
        );
        backend.cache_dir = Some(std::env::temp_dir());
        let mut request = params_json("hi");
        request.input_b64 = Some(reference_wav_b64());
        request.input_content_type = Some("image/png".to_string());
        let params = GenerateParams::from_request(&request).unwrap();
        let mut sink = |_: &str, _: f64| {};
        assert!(backend
            .generate(&params, &mut sink, &CancelToken::new())
            .is_err());
    }

    #[test]
    fn pre_raised_cancel_unwinds_before_synthesis() {
        let mut backend = IndexTtsBackend::with_stub(
            "indextts-2.5",
            Box::new(|_: &IndexTtsJob| panic!("synthesis must not run for a cancelled job")),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_: &str, _: f64| {};
        let params = GenerateParams::from_request(&params_json("hi")).unwrap();
        assert!(matches!(
            backend.generate(&params, &mut sink, &cancel),
            Err(AssetAiError::Cancelled)
        ));
    }
}
