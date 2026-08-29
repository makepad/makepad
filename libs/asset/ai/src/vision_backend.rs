//! The `vision` domain: one image + one prompt in, one text answer out.
//!
//! This is a chat-shaped service, not an image-generation one. The weights —
//! the same Qwen3.8-27B language GGUF the `text` domain serves, plus the
//! mmproj vision tower — stay resident on a dedicated thread between
//! requests, exactly like [`crate::llm_backend`]'s session; a request costs
//! one image encode plus a short greedy decode, not a model load. That is the
//! whole reason a "vision worker" belongs on the same node service as chat
//! rather than in a batch CLI: the answer is interactive once the box is warm.
//!
//! Wire contract (what a client codes against):
//!
//! ```text
//! POST /generate {"model":"qwen3.8-27b-vision","domain":"vision",
//!                 "prompt":"<full prompt>","input_b64":"<PNG or JPEG>",
//!                 "max_tokens":220}                       -> {"job_id":...}
//! GET  /job/<id>  -> running{stage,progress,partial_text}
//!                 -> done{text, artifacts:[text/plain]}
//!                 -> error{error}
//! ```
//!
//! The answer lands in `JobStatusJson::text` on completion and grows in
//! `partial_text` while the turn decodes (the same streaming field chat uses),
//! and it is also persisted as the job's `text/plain` artifact so the ordinary
//! `/artifact/<id>` handoff works for a caller that wants bytes.
//!
//! SHEET TIER. A vision turn's cost is dominated by how many image tokens the
//! tower produces: an input downscaled to 512 px on its longest edge becomes a
//! 32x32 patch grid = 1024 patches = 256 image tokens, and 256 px becomes 64
//! image tokens. Measured peak with the Q4_K_M language model resident:
//! ~24.6 GB at 512 px / max_context 4096, ~21.0 GB at 256 px / max_context
//! 2048. So the tier is chosen ONCE, at load, from the free VRAM the box
//! actually reports (see [`tier_for_free_vram`]) and logged — a request never
//! silently gets a different tier than the one the box was sized for.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;

/// Largest request image payload accepted, in decoded bytes. A turntable
/// sheet is a few hundred KB; this is the "somebody posted a video file"
/// guard, refused with a 400-class error before any decode happens.
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;

/// Largest decoded image accepted, in pixels. A 32 MiB PNG can expand to
/// gigabytes; the tower only ever sees a few hundred pixels a side, so
/// anything past this is refused rather than decoded and thrown away.
pub const MAX_INPUT_PIXELS: usize = 32 * 1024 * 1024;

/// Hard cap on generated answer tokens, whatever the request asks for.
pub const MAX_NEW_TOKENS: u32 = 512;

/// ChatML prefix that opens the user turn and the image.
pub const VISION_PREFIX: &str = "<|im_start|>user\n<|vision_start|>";

/// Assembles the text that follows the image embeddings: the caller's whole
/// prompt, then the assistant turn with an empty think block (this model runs
/// an open think block by default, and a vision answer wants the visible
/// text — the same closed-think prefill the annotate CLI uses).
pub fn vision_suffix(prompt: &str) -> String {
    format!(
        "<|vision_end|>{prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

/// How big an image the tower is fed, and how much context the session
/// reserves for it. One decision per load, never per request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetTier {
    /// Longest edge the input is downscaled to before preprocessing.
    pub sheet_px: u32,
    /// Session context window.
    pub max_context: u32,
}

/// The tier a 32 GB+ box runs: 256 image tokens, room for a long prompt.
pub const LARGE_SHEET: SheetTier = SheetTier {
    sheet_px: 512,
    max_context: 4096,
};

/// The tier a 24 GB box runs: 64 image tokens, halved context. Measured to
/// fit a 4090 alongside the same Q4_K_M weights.
pub const SMALL_SHEET: SheetTier = SheetTier {
    sheet_px: 256,
    max_context: 2048,
};

/// Free VRAM at load time from which the large sheet is affordable.
pub const LARGE_SHEET_FREE_MB: u64 = 30 * 1024;

/// Picks the sheet tier from the box's free VRAM at load time.
///
/// `None` — no NVML on this machine — is a Metal/unified-memory box, not a
/// small one: there is no CUDA VRAM ceiling to protect and the large sheet is
/// the shape this tower was measured on there (24.6 GB peak). Guessing small
/// would quietly halve the answer quality on the machine most likely to be
/// serving a single interactive user.
pub fn tier_for_free_vram(free_mb: Option<u64>) -> SheetTier {
    match free_mb {
        Some(mb) if mb < LARGE_SHEET_FREE_MB => SMALL_SHEET,
        _ => LARGE_SHEET,
    }
}

/// Request-shape validation for the vision domain, applied at `POST
/// /generate` so a caller gets a 400 instead of a queued job that fails on
/// the worker minutes later.
///
/// It deliberately does NOT decode the image: base64 decoding already
/// happened (bad base64 is refused by `GenerateParams::from_request`), and
/// pixel-level checks belong with the decode itself
/// ([`decode_image_rgb8`]).
pub fn validate_vision_params(params: &GenerateParams) -> Result<(), AssetAiError> {
    if params.pull_only {
        // A pull job carries no image and never reaches generation.
        return Ok(());
    }
    if params.prompt.trim().is_empty() {
        return Err(AssetAiError::Params(
            "vision: prompt is required (the question to ask about the image)".to_string(),
        ));
    }
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params(
            "vision: input_b64 is required (a PNG or JPEG image)".to_string(),
        ));
    }
    if params.input_bytes.len() > MAX_INPUT_BYTES {
        return Err(AssetAiError::Params(format!(
            "vision: input image is {} bytes, over the {MAX_INPUT_BYTES} byte limit",
            params.input_bytes.len()
        )));
    }
    // Eight bytes of magic, checked here rather than only at decode time: a
    // payload that is not an image at all would otherwise be admitted, wait
    // in the queue, and cost a cold box a full model load before failing.
    // What CANNOT be decided here is whether a real PNG decodes — that is the
    // decoder's answer, and it needs the weights' worker anyway.
    image_kind(&params.input_bytes).map(|_| ())
}

/// The container formats the vision domain accepts, identified by magic
/// bytes rather than by the caller's `input_content_type` (a client that
/// mislabels its own PNG should still get an answer; one that labels a zip
/// "image/png" must still be refused). `Clip` is an mp4/mov whose FIRST
/// frame is the image: a client whose cache already holds hardware-encoded
/// intra frames sends its stored bytes untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Clip,
}

/// `Err` (a 400-class `Params`) for anything that is neither.
pub fn image_kind(bytes: &[u8]) -> Result<ImageKind, AssetAiError> {
    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(PNG_MAGIC) {
        return Ok(ImageKind::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return Ok(ImageKind::Jpeg);
    }
    // ISO-BMFF (mp4/mov): a size-prefixed `ftyp` box leads the file.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Ok(ImageKind::Clip);
    }
    Err(AssetAiError::Params(
        "vision: input_b64 is not a PNG or JPEG image or an mp4/mov clip".to_string(),
    ))
}

/// Refuses a decoded image whose pixel count would be pointless to hold: the
/// tower sees a few hundred pixels a side no matter what arrives.
pub fn check_image_dimensions(width: usize, height: usize) -> Result<(), AssetAiError> {
    if width == 0 || height == 0 {
        return Err(AssetAiError::Params(
            "vision: input image has a zero dimension".to_string(),
        ));
    }
    let pixels = width.saturating_mul(height);
    if pixels > MAX_INPUT_PIXELS {
        return Err(AssetAiError::Params(format!(
            "vision: input image is {width}x{height} ({pixels} pixels), over the \
             {MAX_INPUT_PIXELS} pixel limit"
        )));
    }
    Ok(())
}

/// Decodes a PNG or JPEG request payload into tightly packed RGB8.
pub fn decode_image_rgb8(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    match image_kind(bytes)? {
        ImageKind::Png => decode_png(bytes),
        ImageKind::Jpeg => decode_jpeg(bytes),
        ImageKind::Clip => decode_clip_first_frame(bytes),
    }
}

/// First frame of an mp4/mov payload via the platform's hardware decoder.
/// The decoder only opens paths, so the bytes take one round trip through
/// the service tmp dir; the temp file dies with the call either way.
#[cfg(feature = "video")]
fn decode_clip_first_frame(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STAMP: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("asset-ai-keyframes");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AssetAiError::Io(format!("vision: clip tmp dir: {e}")))?;
    let path = dir.join(format!(
        "vkf-{}-{}.mov",
        std::process::id(),
        STAMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        std::fs::write(&path, bytes)
            .map_err(|e| AssetAiError::Io(format!("vision: clip tmp write: {e}")))?;
        let path_str = path
            .to_str()
            .ok_or_else(|| AssetAiError::Io("vision: non-utf8 tmp path".to_string()))?;
        let mut decoder = makepad_video::VideoFileDecoder::open(path_str)
            .map_err(|e| AssetAiError::Params(format!("vision: clip open: {e}")))?;
        let frame = decoder
            .next_frame()
            .map_err(|e| AssetAiError::Params(format!("vision: clip decode: {e}")))?
            .ok_or_else(|| AssetAiError::Params("vision: clip has no frames".to_string()))?;
        let (w, h) = (frame.width as usize, frame.height as usize);
        check_image_dimensions(w, h)?;
        Ok((frame.to_rgb8(), w, h))
    })();
    let _ = std::fs::remove_file(&path);
    result
}

#[cfg(not(feature = "video"))]
fn decode_clip_first_frame(_bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    Err(AssetAiError::Params(
        "vision: clip inputs need a build with the `video` feature".to_string(),
    ))
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    use makepad_zune_core::options::DecoderOptions;
    use makepad_zune_png::PngDecoder;
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| AssetAiError::Params(format!("vision: png decode: {e:?}")))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| AssetAiError::Params("vision: png decode: no header info".into()))?;
    let (width, height) = (info.width, info.height);
    check_image_dimensions(width, height)?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| AssetAiError::Params("vision: png decode: no colorspace".into()))?;
    let components = colorspace.num_components();
    if components == 0 {
        return Err(AssetAiError::Params(
            "vision: png decode: zero color channels".into(),
        ));
    }
    let pixels = decoder
        .decode_raw()
        .map_err(|e| AssetAiError::Params(format!("vision: png decode: {e:?}")))?;
    Ok((to_rgb8(&pixels, width * height, components), width, height))
}

fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    use makepad_zune_jpeg::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_jpeg::makepad_zune_core::colorspace::ColorSpace;
    use makepad_zune_jpeg::makepad_zune_core::options::DecoderOptions;
    use makepad_zune_jpeg::JpegDecoder;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| AssetAiError::Params(format!("vision: jpeg decode: {e:?}")))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| AssetAiError::Params("vision: jpeg decode: no dimensions".into()))?;
    check_image_dimensions(width, height)?;
    let pixels = decoder
        .decode()
        .map_err(|e| AssetAiError::Params(format!("vision: jpeg decode: {e:?}")))?;
    let components = if width * height != 0 {
        (pixels.len() / (width * height)).max(1)
    } else {
        1
    };
    Ok((to_rgb8(&pixels, width * height, components), width, height))
}

/// Any component count -> tightly packed RGB8 (alpha dropped, luma
/// replicated). Short input is zero-filled rather than panicking: a truncated
/// decode is a bad request, not a crash.
fn to_rgb8(src: &[u8], pixel_count: usize, components: usize) -> Vec<u8> {
    let mut out = vec![0u8; pixel_count * 3];
    for i in 0..pixel_count {
        let p = i * components;
        if p + components > src.len() {
            break;
        }
        if components >= 3 {
            out[i * 3..i * 3 + 3].copy_from_slice(&src[p..p + 3]);
        } else {
            let luma = src[p];
            out[i * 3] = luma;
            out[i * 3 + 1] = luma;
            out[i * 3 + 2] = luma;
        }
    }
    out
}

/// Box-average downscale so the longest edge is at most `max_edge`, aspect
/// preserved. Images already inside the budget pass through untouched —
/// upscaling would only spend image tokens on invented pixels.
pub fn downscale_to_fit(
    rgb: &[u8],
    width: usize,
    height: usize,
    max_edge: usize,
) -> (Vec<u8>, usize, usize) {
    let longest = width.max(height);
    if longest <= max_edge || max_edge == 0 || width == 0 || height == 0 {
        return (rgb.to_vec(), width, height);
    }
    let dst_w = ((width * max_edge) / longest).max(1);
    let dst_h = ((height * max_edge) / longest).max(1);
    let mut out = vec![0u8; dst_w * dst_h * 3];
    for y in 0..dst_h {
        let sy0 = y * height / dst_h;
        let sy1 = (((y + 1) * height) / dst_h).max(sy0 + 1).min(height);
        for x in 0..dst_w {
            let sx0 = x * width / dst_w;
            let sx1 = (((x + 1) * width) / dst_w).max(sx0 + 1).min(width);
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for sy in sy0..sy1 {
                let row = sy * width;
                for sx in sx0..sx1 {
                    let p = (row + sx) * 3;
                    if p + 3 > rgb.len() {
                        continue;
                    }
                    acc[0] += rgb[p] as u32;
                    acc[1] += rgb[p + 1] as u32;
                    acc[2] += rgb[p + 2] as u32;
                    n += 1;
                }
            }
            let d = (y * dst_w + x) * 3;
            if n > 0 {
                out[d] = (acc[0] / n) as u8;
                out[d + 1] = (acc[1] / n) as u8;
                out[d + 2] = (acc[2] / n) as u8;
            }
        }
    }
    (out, dst_w, dst_h)
}

/// The vision domain's registry contract: what [`VisionBackend::ensure_loaded`]
/// will look for. Checked by the crate tests so a hand-edited registry cannot
/// ship an entry this backend has no way to load.
pub fn spec_is_servable(spec: &crate::registry::ModelSpec) -> Result<(), String> {
    if spec.domain != crate::registry::Domain::Vision {
        return Err(format!("model {} is not in the vision domain", spec.id));
    }
    for role in ["llm-gguf", "mmproj"] {
        if spec.file_by_role(role).is_none() {
            return Err(format!("model {} has no {role:?} artifact", spec.id));
        }
    }
    Ok(())
}

/// True when this machine has a device the vision tower and the language
/// session can actually run on (Metal on macOS, CUDA on Windows/Linux).
///
/// Probed ONCE and memoised: `/models` and `/health` ask per model per
/// request, and binding a device is not free.
///
/// File presence is deliberately NOT part of this: `backend_provisioned` sees
/// only a backend name, and a missing GGUF is already the honest
/// `absent`/`downloading` model state that `ensure_loaded` resolves by
/// pulling it.
pub fn vision_provisioned() -> bool {
    #[cfg(feature = "llm")]
    {
        static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *PROBE.get_or_init(|| makepad_ai_llm::ExecRuntime::new().is_ok())
    }
    #[cfg(not(feature = "llm"))]
    {
        false
    }
}

#[cfg(feature = "llm")]
pub use resident::VisionBackend;

#[cfg(feature = "llm")]
mod resident {
    use super::*;
    use std::path::PathBuf;

    /// The resident vision model: a tower + a session on ONE thread, kept
    /// warm across requests. A second `VisionBackend` for the same model id
    /// is never created (the service holds one backend object per model), and
    /// `unload` is the only thing that drops the weights.
    pub struct VisionBackend {
        model_id: String,
        worker: Option<worker::VisionWorker>,
        /// The (language gguf, mmproj) pair the live worker was spawned from;
        /// a registry change to either respawns instead of answering from
        /// weights nobody asked for.
        loaded: Option<(PathBuf, PathBuf)>,
    }

    impl VisionBackend {
        pub fn new(model_id: &str) -> Self {
            Self {
                model_id: model_id.to_string(),
                worker: None,
                loaded: None,
            }
        }

        /// Brings the tower + session up from an explicit file pair at an
        /// explicit tier. `ensure_loaded` is this plus the registry/cache
        /// resolution in front of it; it is public because loading a local
        /// pair is a real thing to do (the smoke test, a box serving weights
        /// that never came from Hugging Face) and because the alternative
        /// would be a private copy of the same three lines.
        ///
        /// Idempotent for an unchanged pair; a changed one drops the old
        /// session BEFORE loading the new (two 15 GiB residents of the same
        /// model would be an OOM, not a handover).
        pub fn load_from_paths(
            &mut self,
            gguf: PathBuf,
            mmproj: PathBuf,
            tier: SheetTier,
            progress: &mut dyn FnMut(&str, f64),
        ) -> Result<(), AssetAiError> {
            let want = (gguf.clone(), mmproj.clone());
            if self.worker.is_some() && self.loaded.as_ref() == Some(&want) {
                return Ok(());
            }
            self.worker = None;
            self.loaded = None;
            let worker = worker::VisionWorker::spawn(
                gguf,
                mmproj,
                tier,
                self.model_id.clone(),
                progress,
            )
            .map_err(|e| AssetAiError::Backend(format!("vision load: {e}")))?;
            self.worker = Some(worker);
            self.loaded = Some(want);
            Ok(())
        }
    }

    impl ContentBackend for VisionBackend {
        fn model_id(&self) -> &str {
            &self.model_id
        }

        fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            let gguf = ctx.path_by_role("llm-gguf")?;
            let mmproj = ctx.path_by_role("mmproj")?;
            if self.worker.is_some()
                && self.loaded.as_ref() == Some(&(gguf.clone(), mmproj.clone()))
            {
                return Ok(());
            }
            // The tier is a load-time decision, taken against the VRAM the
            // box has free RIGHT NOW rather than against its card's spec: a
            // box already holding the text model has less to give.
            let tier = tier_for_free_vram(crate::residency::fresh_free_mb());
            let gb = std::fs::metadata(&gguf)
                .map(|m| m.len() as f64 / 1e9)
                .unwrap_or(0.0);
            (ctx.progress)(
                &format!(
                    "load vision gguf ({gb:.1}GB) + tower, {}px sheet",
                    tier.sheet_px
                ),
                0.1,
            );
            self.load_from_paths(gguf, mmproj, tier, ctx.progress)?;
            (ctx.progress)("vision session ready", 0.9);
            Ok(())
        }

        fn is_resident(&self) -> bool {
            self.worker.is_some()
        }

        fn unload(&mut self) -> Result<(), AssetAiError> {
            // Dropping the worker closes its thread; the tower's context and
            // the session's weights are released there.
            self.worker = None;
            self.loaded = None;
            Ok(())
        }

        fn generate(
            &mut self,
            params: &GenerateParams,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Vec<ArtifactData>, AssetAiError> {
            self.generate_streamed(params, progress, &mut |_| {}, cancel)
        }

        fn generate_streamed(
            &mut self,
            params: &GenerateParams,
            progress: ProgressSink,
            on_text: &mut dyn FnMut(&str),
            cancel: &CancelToken,
        ) -> Result<Vec<ArtifactData>, AssetAiError> {
            if params.pull_only {
                return Ok(Vec::new());
            }
            validate_vision_params(params)?;
            let worker = self.worker.as_ref().ok_or_else(|| {
                AssetAiError::Backend("vision backend used before ensure_loaded".to_string())
            })?;
            let tier = worker.tier();

            progress("decode image", 0.02);
            let (rgb, width, height) = decode_image_rgb8(&params.input_bytes)?;
            cancel.check()?;
            let (rgb, width, height) =
                downscale_to_fit(&rgb, width, height, tier.sheet_px as usize);

            let answer = worker
                .ask(
                    worker::VisionAsk {
                        prompt: params.prompt.clone(),
                        rgb,
                        width,
                        height,
                        max_new_tokens: params.max_tokens.min(MAX_NEW_TOKENS).max(1),
                    },
                    cancel,
                    progress,
                    on_text,
                )
                .map_err(|e| {
                    if e == "cancelled" {
                        AssetAiError::Cancelled
                    } else {
                        AssetAiError::Backend(format!("vision: {e}"))
                    }
                })?;

            progress("done", 1.0);
            Ok(vec![ArtifactData {
                content_type: "text/plain; charset=utf-8",
                ext: "txt",
                bytes: answer.into_bytes(),
            }])
        }
    }

    mod worker {
        use super::*;
        use makepad_ai_llm::{
            preprocess_rgb8, GgufFile, LlamaSession, LlamaSessionConfig, VisionConfig, VisionTower,
        };
        use std::sync::mpsc;

        /// One question about one image.
        pub struct VisionAsk {
            pub prompt: String,
            /// Already downscaled to the tier's sheet size.
            pub rgb: Vec<u8>,
            pub width: usize,
            pub height: usize,
            pub max_new_tokens: u32,
        }

        pub enum WorkerEvent {
            Stage(String, f64),
            /// Full answer text so far — prefix-stable snapshots, never
            /// deltas, the same convention the chat lane publishes as
            /// `partial_text`.
            Text(String),
            Done(Result<String, String>),
        }

        enum WorkerMsg {
            Ask(VisionAsk, CancelToken, mpsc::Sender<WorkerEvent>),
        }

        /// Handle to the thread that owns the resident tower + session.
        /// `VisionTower` and `LlamaSession` are `!Send`, so they are built and
        /// used only there; this handle is Send and lives exactly as long as
        /// the backend object the service keeps across jobs.
        pub struct VisionWorker {
            tx: mpsc::Sender<WorkerMsg>,
            tier: SheetTier,
        }

        impl VisionWorker {
            pub fn tier(&self) -> SheetTier {
                self.tier
            }

            pub fn spawn(
                gguf: PathBuf,
                mmproj: PathBuf,
                tier: SheetTier,
                model_id: String,
                progress: &mut dyn FnMut(&str, f64),
            ) -> Result<Self, String> {
                enum BootEvt {
                    Progress(String, f64),
                    /// Ok carries the device description, for the one load line.
                    Ready(Result<String, String>),
                }
                let (tx, rx) = mpsc::channel::<WorkerMsg>();
                let (boot_tx, boot_rx) = mpsc::channel::<BootEvt>();
                std::thread::Builder::new()
                    .name("vision-worker".to_string())
                    .spawn(move || {
                        let loaded = (|| -> Result<
                            (VisionTower, LlamaSession, VisionConfig, String),
                            String,
                        > {
                            let mmproj_path = mmproj.to_string_lossy().to_string();
                            let file = GgufFile::open(&mmproj_path)
                                .map_err(|e| format!("open mmproj: {e:?}"))?;
                            let config = VisionConfig::from_gguf(&file)
                                .map_err(|e| format!("mmproj vision config: {e:?}"))?;
                            // The arena is sized for the largest grid this
                            // tier can produce, which is exactly the tier's
                            // sheet: every request is downscaled to it, so one
                            // vision graph is compiled and then reused.
                            let grid = (tier.sheet_px as usize) / config.patch_size.max(1);
                            let max_patches = (grid * grid).max(4);
                            let _ = boot_tx
                                .send(BootEvt::Progress("load vision tower".into(), 0.15));
                            let tower = VisionTower::load(&mmproj_path, max_patches)
                                .map_err(|e| format!("load vision tower: {e:?}"))?;
                            let device = tower.device_description();
                            let session = LlamaSession::load_with_progress(
                                &gguf,
                                LlamaSessionConfig {
                                    max_context: Some(tier.max_context),
                                    ..LlamaSessionConfig::default()
                                },
                                &mut |stage, frac| {
                                    let _ = boot_tx.send(BootEvt::Progress(
                                        stage.to_string(),
                                        0.2 + frac * 0.7,
                                    ));
                                },
                            )
                            .map_err(|e| format!("load llm session: {e:?}"))?;
                            Ok((tower, session, config, device))
                        })();
                        let (mut tower, mut session, config) = match loaded {
                            Ok((tower, session, config, device)) => {
                                // ONE line, and it names every fact the next
                                // question needs: which model, which device,
                                // which tier, and how many image tokens that
                                // tier costs per request.
                                let grid = (tier.sheet_px as usize) / config.patch_size.max(1);
                                eprintln!(
                                    "[vision] {model_id} resident on {device}: {}px sheet, \
                                     {} image tokens, max_context {}",
                                    tier.sheet_px,
                                    (grid * grid) / 4,
                                    tier.max_context,
                                );
                                let _ = boot_tx.send(BootEvt::Ready(Ok(device)));
                                (tower, session, config)
                            }
                            Err(err) => {
                                let _ = boot_tx.send(BootEvt::Ready(Err(err)));
                                return;
                            }
                        };
                        while let Ok(WorkerMsg::Ask(ask, cancel, events)) = rx.recv() {
                            let result = run_ask(
                                &mut tower,
                                &mut session,
                                &config,
                                tier,
                                &ask,
                                &cancel,
                                &events,
                            );
                            let _ = events.send(WorkerEvent::Done(result));
                        }
                        // Sender dropped -> backend dropped: weights unmap here.
                    })
                    .map_err(|e| format!("spawn vision worker: {e}"))?;
                loop {
                    match boot_rx.recv() {
                        Ok(BootEvt::Progress(stage, frac)) => progress(&stage, frac),
                        Ok(BootEvt::Ready(Ok(_))) => break,
                        Ok(BootEvt::Ready(Err(err))) => return Err(err),
                        Err(_) => return Err("vision worker died during load".to_string()),
                    }
                }
                Ok(Self { tx, tier })
            }

            /// Blocks until the answer is complete, forwarding stage and text
            /// snapshots. Cancellation is checked on the worker thread between
            /// generated tokens and reports `Err("cancelled")`.
            pub fn ask(
                &self,
                ask: VisionAsk,
                cancel: &CancelToken,
                on_stage: &mut dyn FnMut(&str, f64),
                on_text: &mut dyn FnMut(&str),
            ) -> Result<String, String> {
                let (event_tx, event_rx) = mpsc::channel();
                self.tx
                    .send(WorkerMsg::Ask(ask, cancel.clone(), event_tx))
                    .map_err(|_| "vision worker thread is gone".to_string())?;
                loop {
                    match event_rx.recv() {
                        Ok(WorkerEvent::Stage(stage, frac)) => on_stage(&stage, frac),
                        Ok(WorkerEvent::Text(text)) => on_text(&text),
                        Ok(WorkerEvent::Done(result)) => return result,
                        Err(_) => return Err("vision worker dropped the reply".to_string()),
                    }
                }
            }
        }

        /// How often a growing answer is published as `partial_text`. Every
        /// token would be a lock per token for no readable gain.
        const TEXT_SNAPSHOT_EVERY: usize = 8;

        fn run_ask(
            tower: &mut VisionTower,
            session: &mut LlamaSession,
            config: &VisionConfig,
            tier: SheetTier,
            ask: &VisionAsk,
            cancel: &CancelToken,
            events: &mpsc::Sender<WorkerEvent>,
        ) -> Result<String, String> {
            let cancelled = || "cancelled".to_string();
            let stage = |name: &str, frac: f64| {
                let _ = events.send(WorkerEvent::Stage(name.to_string(), frac));
            };
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            stage("preprocess image", 0.05);
            let prepared = preprocess_rgb8(&ask.rgb, ask.width, ask.height, config)
                .map_err(|e| format!("preprocess: {e:?}"))?;
            stage("encode image", 0.1);
            let embeddings = tower
                .encode(&prepared)
                .map_err(|e| format!("encode: {e:?}"))?;
            if cancel.is_cancelled() {
                return Err(cancelled());
            }

            // One request must never see another's image or question: clear
            // KV + recurrent state first. State clear only, not a reload.
            session.reset().map_err(|e| format!("reset: {e:?}"))?;
            let prefix_ids = session
                .vocab()
                .tokenize(VISION_PREFIX, false, true)
                .map_err(|e| format!("tokenize prefix: {e:?}"))?;
            let suffix = vision_suffix(&ask.prompt);
            let suffix_ids = session
                .vocab()
                .tokenize(&suffix, false, true)
                .map_err(|e| format!("tokenize prompt: {e:?}"))?;
            let image_tokens = prepared.tokens_w() * prepared.tokens_h();
            let needed =
                prefix_ids.len() + image_tokens + suffix_ids.len() + ask.max_new_tokens as usize;
            if needed > tier.max_context as usize {
                // An honest refusal beats a truncated answer: the caller can
                // shorten the prompt or ask for fewer tokens.
                return Err(format!(
                    "prompt needs {needed} tokens ({} prompt + {image_tokens} image + {} \
                     answer) but this box serves the {}px tier with max_context {}",
                    prefix_ids.len() + suffix_ids.len(),
                    ask.max_new_tokens,
                    tier.sheet_px,
                    tier.max_context
                ));
            }
            stage("prefill", 0.15);
            session
                .append_tokens(&prefix_ids)
                .map_err(|e| format!("prefill prefix: {e:?}"))?;
            session
                .append_image_embeddings(&embeddings, prepared.tokens_w(), prepared.tokens_h())
                .map_err(|e| format!("prefill image: {e:?}"))?;
            session
                .append_tokens(&suffix_ids)
                .map_err(|e| format!("prefill prompt: {e:?}"))?;
            if cancel.is_cancelled() {
                return Err(cancelled());
            }

            let max_new = ask.max_new_tokens as usize;
            let mut text = String::new();
            let mut decoder = session.vocab().text_decoder();
            let mut generated = 0usize;
            stage(&format!("decode 0/{max_new}"), 0.2);
            while generated < max_new {
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }
                let Some(token) = session
                    .next_greedy_token()
                    .map_err(|e| format!("generate: {e:?}"))?
                else {
                    break;
                };
                generated += 1;
                if let Some(chunk) = decoder.push_token(session.vocab(), token) {
                    text.push_str(&chunk);
                }
                if generated % TEXT_SNAPSHOT_EVERY == 0 {
                    let _ = events.send(WorkerEvent::Text(text.clone()));
                    stage(
                        &format!("decode {generated}/{max_new}"),
                        0.2 + 0.75 * (generated as f64 / max_new as f64),
                    );
                }
            }
            let text = text.trim().to_string();
            let _ = events.send(WorkerEvent::Text(text.clone()));
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Domain, Registry};

    fn params(prompt: &str, input: Vec<u8>) -> GenerateParams {
        let request = crate::protocol::GenerateRequestJson {
            model: "qwen3.8-27b-vision".to_string(),
            domain: Some("vision".to_string()),
            prompt: Some(prompt.to_string()),
            input_b64: Some(
                String::from_utf8(makepad_base64::base64_encode(
                    &input,
                    &makepad_base64::BASE64_STANDARD,
                ))
                .expect("base64 is ascii"),
            ),
            max_tokens: Some(220),
            ..Default::default()
        };
        GenerateParams::from_request(&request).expect("params")
    }

    #[test]
    fn registry_carries_a_servable_vision_entry_sharing_the_text_gguf() {
        let registry = Registry::embedded().unwrap();
        let vision = registry.find("qwen3.8-27b-vision").expect("vision entry");
        assert_eq!(vision.domain, Domain::Vision);
        assert_eq!(vision.backend, "vision");
        assert!(vision.available && !vision.gated);
        // Honest VRAM: the small-sheet peak is what a 24 GB card must fit,
        // and the hard floor keeps this off a card that cannot hold it.
        assert_eq!(vision.vram_gb, Some(21.0));
        assert_eq!(vision.min_vram_gb, Some(22.0));
        assert_eq!(vision.files.len(), 2);

        // The language GGUF is byte-for-byte the text domain's file, at the
        // same cache path: a box that already serves qwen3.8-27b downloads
        // only the tower. (Registry::parse also refuses two entries that
        // claim one cache path with conflicting identities, so this is the
        // sharing contract, not a coincidence.)
        let text = registry.find("qwen3.8-27b").expect("text entry");
        let text_gguf = text.file_by_role("llm-gguf").unwrap();
        let vision_gguf = vision.file_by_role("llm-gguf").unwrap();
        assert_eq!(vision_gguf.cache_as, text_gguf.cache_as);
        assert_eq!(vision_gguf.repo, text_gguf.repo);
        assert_eq!(vision_gguf.path, text_gguf.path);
        assert_eq!(vision_gguf.revision, text_gguf.revision);
        assert_eq!(vision_gguf.size, text_gguf.size);
        assert_eq!(vision_gguf.sha256, text_gguf.sha256);

        let mmproj = vision.file_by_role("mmproj").expect("mmproj role");
        assert_eq!(mmproj.repo, "unsloth/Qwen3.8-27B-GGUF");
        assert_eq!(mmproj.path, "mmproj-F16.gguf");
        assert_eq!(mmproj.cache_as, "llm/Qwen3.8-27B-mmproj-F16.gguf");
        assert_eq!(mmproj.size, Some(927_607_488));
        assert_eq!(
            mmproj.sha256.as_deref(),
            Some("cbb841a9ee0636b2ec172f5bb8df2ea8dfeb01e90fe7c6126581d662a0b4e43e")
        );
        assert!(!mmproj.local);
        assert_eq!(mmproj.revision, vision_gguf.revision);

        spec_is_servable(vision).unwrap();
        let text_spec_refused = spec_is_servable(text).unwrap_err();
        assert!(text_spec_refused.contains("not in the vision domain"));
    }

    #[test]
    fn vision_domain_round_trips_through_the_wire_name() {
        assert_eq!(Domain::parse("vision"), Some(Domain::Vision));
        assert_eq!(Domain::Vision.as_str(), "vision");
        assert_eq!(Domain::parse("visionn"), None);
    }

    #[test]
    fn a_request_without_an_image_is_refused() {
        let missing = params("what is this?", Vec::new());
        let err = validate_vision_params(&missing).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)), "{err}");
        assert!(err.to_string().contains("input_b64 is required"), "{err}");
    }

    #[test]
    fn a_request_without_a_prompt_is_refused() {
        let png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let err = validate_vision_params(&params("   ", png)).unwrap_err();
        assert!(err.to_string().contains("prompt is required"), "{err}");
    }

    #[test]
    fn a_payload_that_is_not_an_image_is_refused_at_admission() {
        // A cold box must not spend a 16 GB model load discovering this.
        let err = validate_vision_params(&params("describe", b"PK\x03\x04zip".to_vec()))
            .unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)), "{err}");
        assert!(err.to_string().contains("not a PNG or JPEG"), "{err}");
        assert_eq!(image_kind(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10]), Ok(ImageKind::Png));
        assert_eq!(image_kind(&[0xff, 0xd8, 0xff, 0xe0]), Ok(ImageKind::Jpeg));
        assert!(image_kind(&[0x89, b'P', b'N']).is_err());
    }

    #[test]
    fn an_oversized_payload_is_refused_before_any_decode() {
        // A real PNG header, so this tests the size cap and not the sniff.
        let mut big = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        big.resize(MAX_INPUT_BYTES + 1, 0);
        let err = validate_vision_params(&params("describe", big.clone())).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)), "{err}");
        assert!(err.to_string().contains("byte limit"), "{err}");
        // Exactly at the limit is fine.
        big.truncate(MAX_INPUT_BYTES);
        assert!(validate_vision_params(&params("describe", big)).is_ok());
    }

    #[test]
    fn bad_base64_never_reaches_the_backend() {
        let request = crate::protocol::GenerateRequestJson {
            model: "qwen3.8-27b-vision".to_string(),
            domain: Some("vision".to_string()),
            prompt: Some("describe".to_string()),
            input_b64: Some("not base64 at all!!".to_string()),
            ..Default::default()
        };
        let err = GenerateParams::from_request(&request).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)), "{err}");
    }

    #[test]
    fn a_pull_job_needs_neither_prompt_nor_image() {
        let request = crate::protocol::GenerateRequestJson {
            model: "qwen3.8-27b-vision".to_string(),
            domain: Some("vision".to_string()),
            pull_only: Some(true),
            ..Default::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        validate_vision_params(&params).unwrap();
    }

    #[test]
    fn tier_follows_free_vram_and_treats_no_nvml_as_unified_memory() {
        assert_eq!(tier_for_free_vram(Some(80 * 1024)), LARGE_SHEET);
        assert_eq!(tier_for_free_vram(Some(LARGE_SHEET_FREE_MB)), LARGE_SHEET);
        assert_eq!(
            tier_for_free_vram(Some(LARGE_SHEET_FREE_MB - 1)),
            SMALL_SHEET
        );
        // A 4090 with the box otherwise idle: the small sheet.
        assert_eq!(tier_for_free_vram(Some(24_000)), SMALL_SHEET);
        assert_eq!(tier_for_free_vram(Some(0)), SMALL_SHEET);
        // No nvidia-smi (mac/Metal): the large sheet, which is what that box
        // class was measured on.
        assert_eq!(tier_for_free_vram(None), LARGE_SHEET);
        // The tiers themselves are the measured pair, not adjustable prose.
        assert_eq!(LARGE_SHEET.sheet_px, 512);
        assert_eq!(LARGE_SHEET.max_context, 4096);
        assert_eq!(SMALL_SHEET.sheet_px, 256);
        assert_eq!(SMALL_SHEET.max_context, 2048);
    }

    #[test]
    fn oversized_dimensions_are_refused() {
        assert!(check_image_dimensions(1024, 1024).is_ok());
        assert!(check_image_dimensions(0, 16).is_err());
        assert!(check_image_dimensions(16, 0).is_err());
        let err = check_image_dimensions(MAX_INPUT_PIXELS, 2).unwrap_err();
        assert!(err.to_string().contains("pixel limit"), "{err}");
    }

    #[test]
    fn png_and_jpeg_decode_to_rgb_and_anything_else_is_refused() {
        // A 4x2 RGB PNG encoded by the same encoder the service ships.
        let mut rgb = Vec::new();
        for i in 0..8u8 {
            rgb.extend_from_slice(&[i * 30, 255 - i * 30, 128]);
        }
        let png = crate::testpattern::encode_png_rgb8(&rgb, 4, 2).expect("encode");
        let (decoded, w, h) = decode_image_rgb8(&png).expect("decode png");
        assert_eq!((w, h), (4, 2));
        assert_eq!(decoded, rgb);

        let err = decode_image_rgb8(b"PK\x03\x04 not an image").unwrap_err();
        assert!(err.to_string().contains("not a PNG or JPEG"), "{err}");
        // A JPEG magic with no frame behind it is a decode error, not a panic.
        assert!(decode_image_rgb8(&[0xff, 0xd8, 0x00, 0x01]).is_err());
    }

    #[test]
    fn downscale_hits_the_tier_edge_and_averages_instead_of_dropping_pixels() {
        // 1024x512 sheet -> 512x256 at the large tier: aspect preserved.
        let src = vec![0u8; 1024 * 512 * 3];
        let (_, w, h) = downscale_to_fit(&src, 1024, 512, 512);
        assert_eq!((w, h), (512, 256));

        // 2x1 of pure black and pure white must average to mid grey, not
        // whichever pixel a nearest-neighbour sample happened to land on.
        let pair = vec![0, 0, 0, 255, 255, 255];
        let (out, w, h) = downscale_to_fit(&pair, 2, 1, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![127, 127, 127]);

        // Already inside the budget: untouched, never upscaled.
        let small = vec![9u8; 8 * 8 * 3];
        let (out, w, h) = downscale_to_fit(&small, 8, 8, 512);
        assert_eq!((w, h), (8, 8));
        assert_eq!(out, small);
    }

    #[test]
    fn the_prompt_wraps_the_caller_text_in_the_vision_turn() {
        let suffix = vision_suffix("What piece is this?");
        assert!(suffix.starts_with("<|vision_end|>What piece is this?<|im_end|>"));
        assert!(suffix.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
        assert!(VISION_PREFIX.ends_with("<|vision_start|>"));
    }

    /// Loads a real model and answers a question about a synthetic image
    /// through the SAME path a request takes (validate -> decode -> downscale
    /// -> resident worker -> text/plain artifact).
    ///
    /// Skipped unless `MAKEPAD_VISION_SMOKE=1`; the weights come from
    /// `MAKEPAD_VISION_SMOKE_MODEL` / `MAKEPAD_VISION_SMOKE_MMPROJ` (the local
    /// 9B pair is the cheap one). Run it under `local/tools/gpu-guard`.
    #[test]
    #[cfg(feature = "llm")]
    fn vision_smoke_answers_about_a_synthetic_image() {
        if std::env::var("MAKEPAD_VISION_SMOKE").as_deref() != Ok("1") {
            return;
        }
        let model = std::env::var("MAKEPAD_VISION_SMOKE_MODEL")
            .expect("MAKEPAD_VISION_SMOKE_MODEL=<path to a language gguf>");
        let mmproj = std::env::var("MAKEPAD_VISION_SMOKE_MMPROJ")
            .expect("MAKEPAD_VISION_SMOKE_MMPROJ=<path to the matching mmproj gguf>");
        // 64x64: left half red, right half blue.
        let mut rgb = vec![0u8; 64 * 64 * 3];
        for y in 0..64 {
            for x in 0..64 {
                let p = (y * 64 + x) * 3;
                if x < 32 {
                    rgb[p] = 220;
                } else {
                    rgb[p + 2] = 220;
                }
            }
        }
        let png = crate::testpattern::encode_png_rgb8(&rgb, 64, 64).expect("encode");

        let mut backend = VisionBackend::new("vision-smoke");
        let load = std::time::Instant::now();
        backend
            .load_from_paths(
                model.into(),
                mmproj.into(),
                tier_for_free_vram(crate::residency::fresh_free_mb()),
                &mut |stage, frac| eprintln!("[vision-smoke] load {stage} {:.0}%", frac * 100.0),
            )
            .expect("load");
        eprintln!("[vision-smoke] loaded in {:.1}s", load.elapsed().as_secs_f64());
        assert!(backend.is_resident());

        let params = params(
            "Name the two colours in this image, left half first. Answer in one short sentence.",
            png,
        );
        let asked = std::time::Instant::now();
        let mut streamed = String::new();
        let artifacts = backend
            .generate_streamed(
                &params,
                &mut |stage, frac| eprintln!("[vision-smoke] {stage} {:.0}%", frac * 100.0),
                &mut |text| streamed = text.to_string(),
                &CancelToken::new(),
            )
            .expect("answer");
        let answer = String::from_utf8(artifacts[0].bytes.clone()).expect("utf8 answer");
        eprintln!(
            "[vision-smoke] {:.1}s answer: {answer}",
            asked.elapsed().as_secs_f64()
        );
        assert_eq!(artifacts.len(), 1);
        assert!(artifacts[0].content_type.starts_with("text/plain"));
        assert!(!answer.trim().is_empty(), "empty answer");
        // The streamed snapshots are prefixes of the final answer.
        assert!(answer.starts_with(streamed.trim_end()) || streamed.is_empty());
        // It looked at the image rather than answering from the prompt alone.
        let lower = answer.to_lowercase();
        assert!(lower.contains("red") && lower.contains("blue"), "{answer}");

        backend.unload().unwrap();
        assert!(!backend.is_resident());
    }
}
