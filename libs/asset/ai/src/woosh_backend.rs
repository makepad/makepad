//! The `woosh` backend: audio domain — Sony Woosh DFlow text-to-SFX through
//! the in-repo port in libs/diffusion (`woosh_pipeline`, CPU f32 today; the
//! CUDA device path rides the same modules when it lands). No python, no
//! external processes.
//!
//! Woosh is the third SFX voice beside sa3-sfx and moss-sfx, picked for
//! speed (4-NFE distilled student — ~0.06 s warm on the torch reference
//! 5090) and take variety. Output is a FIXED 5.0 s mono 48 kHz clip — the
//! model has no duration conditioning — duplicated to stereo wav; requested
//! `seconds`/`steps` are ignored by design.
//!
//! Layering (kokoro/sa3 pattern): request handling, zip extraction and WAV
//! encoding compile and test EVERYWHERE — generation is pluggable, so CI
//! exercises the whole audio job path with a stub. The real generator
//! (feature `audio`) tokenizes with the in-repo RoBERTa BPE port, runs
//! RoBERTa-large TE -> 12-layer MMDiT (4-step Euler + renoise, CFG embedded
//! in a single forward) -> VOCOS AE decode.
//!
//! Weights ship as GitHub release ZIPs (the official distribution — see the
//! registry entry): `ensure_loaded` extracts `checkpoints/<Name>/...` out of
//! each downloaded zip into the cache; the extracted safetensors are the
//! `converts_to` form, so boxes carrying extracted weights never re-download
//! the zips.
//!
//! Request: `{model: "woosh-sfx", prompt, seed}` -> one `audio/wav` artifact
//! (stereo 16-bit PCM, 48 kHz, both channels the mono output).
//!
//! Determinism note: seeds are deterministic per build but NOT
//! torch-compatible (same stance as sa3/moss; the port is validated against
//! oracle dumps by replaying the reference's captured noise instead).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::wav::encode_wav_pcm16_stereo;
use std::path::Path;

pub const SAMPLE_RATE: u32 = 48_000;
/// Fixed clip length: 501 latent frames x 480-sample hop.
pub const SECONDS: f64 = 5.0;

/// Job-fraction band for the pipeline's weight-load progress (labels — "load
/// woosh te/dflow/ae" — pass through as-is).
pub fn load_fraction(fraction: f64) -> f64 {
    0.01 + 0.03 * fraction.clamp(0.0, 1.0)
}

/// Job-fraction band for the pipeline's generate progress ("text-encode
/// k/23", "denoise k/4", "ae-decode k/10"); wav-encode follows at 0.95.
pub fn gen_fraction(fraction: f64) -> f64 {
    0.04 + 0.90 * fraction.clamp(0.0, 1.0)
}

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct AudioJob {
    pub prompt: String,
    pub seed: u64,
}

/// Pluggable generation: mono f32 samples at [`SAMPLE_RATE`].
pub type GenFn = Box<
    dyn FnMut(&AudioJob, ProgressSink, &CancelToken) -> Result<Vec<f32>, AssetAiError> + Send,
>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "audio")]
    Woosh(woosh_gen::WooshGen),
}

pub struct WooshBackend {
    model_id: String,
    gen: Gen,
}

impl WooshBackend {
    /// Test/CI constructor: generation is the given closure, no weights.
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "audio")]
    pub fn new_woosh(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Woosh(woosh_gen::WooshGen::new()),
        }
    }
}

impl ContentBackend for WooshBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Woosh(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "audio")]
            Gen::Woosh(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Woosh(gen) => gen.unload(),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let prompt = params.prompt.trim();
        if prompt.is_empty() {
            return Err(AssetAiError::Backend(
                "sound generation needs a non-empty `prompt`".to_string(),
            ));
        }
        // seconds/steps are intentionally ignored: the model generates a
        // fixed 5.0 s clip with a fixed 4-step schedule.
        let job = AudioJob {
            prompt: prompt.to_string(),
            seed: params.seed,
        };
        cancel.check()?;
        let mono = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut *progress, cancel)?,
            #[cfg(feature = "audio")]
            Gen::Woosh(gen) => gen.generate(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if mono.is_empty() {
            return Err(AssetAiError::Backend(
                "sound generation produced no audio".to_string(),
            ));
        }
        progress("wav-encode", 0.95);
        let wav = encode_wav_pcm16_stereo(&mono, &mono, SAMPLE_RATE);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes: wav,
        }])
    }
}

// ---------------------------------------------------------------------------
// Zip extraction (compiled everywhere; exercised by tests with a tiny
// ZipWriter archive and by the real backend on the release zips)
// ---------------------------------------------------------------------------

/// Extracts every file entry of `zip_path` under `dest_root`, preserving the
/// archive's relative paths ('/'-separated). Rejects zip-slip (absolute or
/// `..` components). Existing files are overwritten; directory entries are
/// skipped. Returns the extracted file paths.
pub fn extract_zip(zip_path: &Path, dest_root: &Path) -> Result<Vec<std::path::PathBuf>, AssetAiError> {
    use makepad_zip_file::zip_read_central_directory;
    let zerr = |what: &str, e: makepad_zip_file::ZipError| {
        AssetAiError::Backend(format!("{}: {what}: {e:?}", zip_path.display()))
    };
    let mut file = std::io::BufReader::new(
        std::fs::File::open(zip_path)
            .map_err(|e| AssetAiError::Io(format!("open {}: {e}", zip_path.display())))?,
    );
    let directory =
        zip_read_central_directory(&mut file).map_err(|e| zerr("central directory", e))?;
    let mut out = Vec::new();
    for header in &directory.file_headers {
        let name = header.file_name.trim_end_matches('/');
        if header.file_name.ends_with('/') || name.is_empty() {
            continue; // directory entry
        }
        if name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|part| part == ".." || part.is_empty())
        {
            return Err(AssetAiError::Backend(format!(
                "{}: refusing zip entry with unsafe path {:?}",
                zip_path.display(),
                header.file_name
            )));
        }
        let bytes = header.extract(&mut file).map_err(|e| zerr("extract", e))?;
        let mut dest = dest_root.to_path_buf();
        for part in name.split('/') {
            dest.push(part);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AssetAiError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        std::fs::write(&dest, &bytes)
            .map_err(|e| AssetAiError::Io(format!("write {}: {e}", dest.display())))?;
        out.push(dest);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Real generation through libs/diffusion (feature audio)
// ---------------------------------------------------------------------------

#[cfg(feature = "audio")]
mod woosh_gen {
    use super::AudioJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_diffusion::woosh_pipeline::WooshPipeline;
    use makepad_diffusion::DiffusionError;
    use std::path::PathBuf;

    /// DiffusionError -> AssetAiError, preserving cancellation.
    fn gen_err(context: &str, err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            err => AssetAiError::Backend(format!("{context}: {err:?}")),
        }
    }

    pub struct WooshGen {
        /// (te, dflow, ae, tokenizer.json), resolved by ensure_loaded.
        paths: Option<(PathBuf, PathBuf, PathBuf, PathBuf)>,
        loaded: Option<WooshPipeline>,
    }

    impl WooshGen {
        pub fn new() -> Self {
            Self {
                paths: None,
                loaded: None,
            }
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            // Extract any downloaded release zip whose converted form
            // (checkpoints/<Name>/weights.safetensors) is not in the cache
            // yet. Idempotent: extracted caches skip straight through.
            for (index, file) in ctx.spec.files.iter().enumerate() {
                let Some(converted) = file.converted_path(ctx.cache_dir) else {
                    continue;
                };
                if converted.is_file() {
                    continue;
                }
                let zip_path = file.dest_path(ctx.cache_dir);
                if !zip_path.is_file() {
                    return Err(AssetAiError::Backend(format!(
                        "model {}: neither {} nor its zip {} is in the cache",
                        ctx.spec.id,
                        converted.display(),
                        zip_path.display()
                    )));
                }
                let name = file.cache_as.rsplit('/').next().unwrap_or(&file.cache_as);
                (ctx.progress)(
                    &format!("extract {name}"),
                    index as f64 / ctx.spec.files.len() as f64,
                );
                // Entries are checkpoints/<Name>/...; converts_to is
                // audio/woosh/checkpoints/<Name>/weights.safetensors, so the
                // extraction root is the cache's audio/woosh dir.
                let mut root = ctx.cache_dir.to_path_buf();
                root.push("audio");
                root.push("woosh");
                super::extract_zip(&zip_path, &root)?;
                if !converted.is_file() {
                    return Err(AssetAiError::Backend(format!(
                        "model {}: {} did not contain {}",
                        ctx.spec.id,
                        zip_path.display(),
                        converted.display()
                    )));
                }
            }
            let file_at = |cache_as: &str| -> Result<PathBuf, AssetAiError> {
                ctx.spec
                    .files
                    .iter()
                    .find(|file| {
                        file.converts_to.as_deref() == Some(cache_as) || file.cache_as == cache_as
                    })
                    .map(|file| {
                        file.converts_to
                            .as_deref()
                            .filter(|_| file.cache_as != cache_as)
                            .map(|converted| {
                                let mut out = ctx.cache_dir.to_path_buf();
                                for part in converted.split('/') {
                                    out.push(part);
                                }
                                out
                            })
                            .unwrap_or_else(|| file.dest_path(ctx.cache_dir))
                    })
                    .ok_or_else(|| {
                        AssetAiError::Backend(format!(
                            "model {}: registry lists no {cache_as}",
                            ctx.spec.id
                        ))
                    })
            };
            let paths = (
                file_at("audio/woosh/checkpoints/TextConditionerA/weights.safetensors")?,
                file_at("audio/woosh/checkpoints/Woosh-DFlow/weights.safetensors")?,
                file_at("audio/woosh/checkpoints/Woosh-AE/weights.safetensors")?,
                file_at("audio/woosh/tokenizer.json")?,
            );
            if self.paths.as_ref() != Some(&paths) {
                self.unload()?;
                self.paths = Some(paths);
            }
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.loaded.is_some()
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            // Woosh is CPU f32 today; the pipeline owns every retained model
            // tensor and its cached negative conditioning.
            self.loaded = None;
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &AudioJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Vec<f32>, AssetAiError> {
            let (te, dflow, ae, tokenizer) = self
                .paths
                .clone()
                .ok_or_else(|| AssetAiError::Backend("woosh used before ensure_loaded".into()))?;
            if self.loaded.is_none() {
                progress("load weights", 0.01);
                let mut load_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, super::load_fraction(fraction));
                    Ok(())
                };
                let pipeline = WooshPipeline::load(&te, &dflow, &ae, &tokenizer, Some(&mut load_hook))
                    .map_err(|e| gen_err("woosh load", e))?;
                self.loaded = Some(pipeline);
            }
            cancel.check()?;
            let pipeline = self.loaded.as_ref().unwrap();
            // The pipeline ticks "text-encode k/23" (per TE layer),
            // "denoise k/4" (per step) and "ae-decode k/10" (per AE block);
            // labels forward as-is, fractions remap onto the job band.
            let mut gen_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(label, super::gen_fraction(fraction));
                Ok(())
            };
            let is_cancelled = || cancel.is_cancelled();
            pipeline
                .generate(&job.prompt, job.seed, Some(&mut gen_hook), Some(&is_cancelled))
                .map_err(|e| gen_err("woosh generate", e))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed generation + real zip extraction — what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;

    fn params(prompt: &str, seed: Option<u64>) -> GenerateParams {
        let request = GenerateRequestJson {
            model: "woosh-sfx".to_string(),
            prompt: Some(prompt.to_string()),
            seed,
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn job_fraction_bands_stay_ordered() {
        assert!((load_fraction(0.0) - 0.01).abs() < 1e-9);
        assert!((load_fraction(1.0) - 0.04).abs() < 1e-9);
        assert!((gen_fraction(0.0) - 0.04).abs() < 1e-9);
        assert!((gen_fraction(1.0) - 0.94).abs() < 1e-9);
        assert!((gen_fraction(7.0) - 0.94).abs() < 1e-9);
        assert!(gen_fraction(1.0) < 0.95);
    }

    #[test]
    fn stub_backend_is_never_reported_resident() {
        let mut backend = WooshBackend::with_stub(
            "woosh-test",
            Box::new(|_, _, _| unreachable!("lifecycle test never generates")),
        );
        assert!(!backend.is_resident());
        backend.unload().unwrap();
        backend.unload().unwrap();
        assert!(!backend.is_resident());
    }

    #[cfg(feature = "audio")]
    #[test]
    fn native_backend_is_send_and_starts_cold() {
        fn assert_send<T: Send>() {}
        assert_send::<WooshBackend>();
        let mut backend = WooshBackend::new_woosh("woosh-test");
        assert!(!backend.is_resident());
        backend.unload().unwrap();
    }

    #[test]
    fn stub_generation_to_stereo_wav() {
        let mut backend = WooshBackend::with_stub(
            "woosh-sfx",
            Box::new(|job: &AudioJob, progress: ProgressSink, _c: &CancelToken| {
                assert_eq!(job.prompt, "coin pickup");
                assert_eq!(job.seed, 42);
                progress("denoise 1/4", 0.3);
                let n = (SECONDS * SAMPLE_RATE as f64) as usize;
                Ok(vec![0.25f32; n])
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params("coin pickup", Some(42)), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        assert_eq!(artifacts[0].ext, "wav");
        let wav = &artifacts[0].bytes;
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2); // stereo
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            SAMPLE_RATE
        );
        // 5.0 s fixed: 240000 frames of 2ch x 16-bit.
        assert_eq!(wav.len(), 44 + 240_000 * 2 * 2);
    }

    #[test]
    fn pre_raised_cancel_unwinds_before_generation() {
        let mut backend = WooshBackend::with_stub(
            "woosh-sfx",
            Box::new(|_: &AudioJob, _p: ProgressSink, _c: &CancelToken| {
                panic!("generation must not run for a cancelled job");
            }),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params("boom", None), &mut sink, &cancel),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn empty_prompt_and_empty_audio_rejected() {
        let mut backend = WooshBackend::with_stub(
            "woosh-sfx",
            Box::new(|_: &AudioJob, _p: ProgressSink, _c: &CancelToken| Ok(Vec::new())),
        );
        let mut sink = |_: &str, _: f64| {};
        assert!(backend
            .generate(&params("  ", None), &mut sink, &CancelToken::new())
            .is_err());
        assert!(backend
            .generate(&params("boom", None), &mut sink, &CancelToken::new())
            .is_err());
    }

    #[test]
    fn zip_extraction_roundtrip_and_slip_guard() {
        use makepad_zip_file::{ZipMethod, ZipWriter};
        let dir = std::env::temp_dir().join(format!(
            "makepad-asset-ai-woosh-zip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build a tiny archive shaped like the release zips.
        let zip_path = dir.join("Woosh-AE.zip");
        {
            let mut writer = ZipWriter::new();
            writer
                .add(
                    "checkpoints/Woosh-AE/weights.safetensors",
                    b"not really weights",
                    ZipMethod::Deflate,
                )
                .unwrap();
            writer
                .add("checkpoints/Woosh-AE/config.yaml", b"z_dim: 128", ZipMethod::Store)
                .unwrap();
            std::fs::write(&zip_path, writer.finish().unwrap()).unwrap();
        }
        let root = dir.join("audio").join("woosh");
        let extracted = extract_zip(&zip_path, &root).unwrap();
        assert_eq!(extracted.len(), 2);
        let weights = root
            .join("checkpoints")
            .join("Woosh-AE")
            .join("weights.safetensors");
        assert_eq!(std::fs::read(&weights).unwrap(), b"not really weights");
        assert_eq!(
            std::fs::read(root.join("checkpoints").join("Woosh-AE").join("config.yaml")).unwrap(),
            b"z_dim: 128"
        );

        // Zip-slip attempts are rejected outright. ZipWriter refuses to
        // author such names, so binary-patch a benign one after the fact.
        let evil_path = dir.join("evil.zip");
        {
            let mut writer = ZipWriter::new();
            writer.add("xx/e.txt", b"nope", ZipMethod::Store).unwrap();
            let mut bytes = writer.finish().unwrap();
            let needle = b"xx/e.txt";
            for i in 0..bytes.len().saturating_sub(needle.len()) {
                if &bytes[i..i + needle.len()] == needle {
                    bytes[i..i + needle.len()].copy_from_slice(b"../e.txt");
                }
            }
            std::fs::write(&evil_path, bytes).unwrap();
        }
        assert!(extract_zip(&evil_path, &root).is_err());
        assert!(!dir.join("e.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Full extraction gate on the real release zips. Skips unless
    /// WOOSH_ZIP_DIR points at a dir with the three downloaded zips, e.g.
    ///   WOOSH_ZIP_DIR=local/models/woosh/zips cargo test -p makepad-asset-ai
    #[test]
    fn real_release_zips_extract_when_env_set() {
        let Ok(dir) = std::env::var("WOOSH_ZIP_DIR") else {
            return;
        };
        let out = std::env::temp_dir().join(format!(
            "makepad-asset-ai-woosh-realzip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&out);
        for (name, expect_len) in [
            ("TextConditionerA.zip", 1_425_689_504u64),
            ("Woosh-AE.zip", 884_664_420),
            ("Woosh-DFlow.zip", 1_378_890_620),
        ] {
            let zip = Path::new(&dir).join(name);
            let extracted = extract_zip(&zip, &out).unwrap();
            let weights = extracted
                .iter()
                .find(|p| p.file_name().is_some_and(|n| n == "weights.safetensors"))
                .unwrap_or_else(|| panic!("{name}: no weights.safetensors"));
            let len = std::fs::metadata(weights).unwrap().len();
            assert_eq!(len, expect_len, "{name}");
        }
        let _ = std::fs::remove_dir_all(&out);
    }
}
