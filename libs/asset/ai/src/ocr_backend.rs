//! The `ocr` domain: one scanned page in, its transcription out, as HTML.
//!
//! Same machinery as the `vision` domain — the mmproj vision tower plus a
//! resident language session on one thread — but with the model, the page
//! fit and the decode discipline of a document-OCR fine-tune. The registry
//! entry is Chandra 2 (`datalab-to/chandra-ocr-2`, a Qwen3.5-4B fine-tune),
//! and everything model-specific below is a verbatim port of its reference
//! pipeline (`chandra/prompts.py`, `chandra/model/util.py`,
//! `chandra/settings.py`): the prompt text the weights were trained against,
//! the page-fit rule, greedy decoding, and the repeat detector that turns a
//! looping generation into a hotter retry instead of twelve thousand tokens
//! of the same line.
//!
//! Wire contract:
//!
//! ```text
//! POST /generate {"model":"chandra-ocr-2","domain":"ocr",
//!                 "input_b64":"<PNG, JPEG or mp4/mov keyframe>",
//!                 "prompt":"" | "layout" | "<custom prompt>",
//!                 "max_tokens":12384}                        -> {"job_id":...}
//! GET  /job/<id>  -> running{stage,progress,partial_text}
//!                 -> done{text, artifacts:[text/html]}
//! ```
//!
//! `prompt` empty = plain transcription (`OCR_PROMPT`); `"layout"` = the
//! layout-block variant with `data-bbox` boxes (`OCR_LAYOUT_PROMPT`);
//! anything else is sent verbatim for callers that know what they want.
//!
//! PAGE FIT. A page is fed at document resolution, not at the vision
//! domain's caption sheet: the short side is raised to at least
//! [`MIN_IMAGE_DIM`] px and the area capped at [`MAX_FIT`] (3072x2048), so a
//! 2000x2800 scan goes in untouched at ~5.5k image tokens. That only fits a
//! 24 GB card because the tower's activations are liveness-planned (see
//! `VisionTower::load`), not arena-resident.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::vision_backend::{decode_image_rgb8_within, image_kind};

/// Largest request payload accepted, in bytes. A full-resolution page PNG
/// runs 5-20 MB; this is the "somebody posted a video" guard.
pub const MAX_INPUT_BYTES: usize = 96 * 1024 * 1024;

/// Largest decoded page accepted, in pixels. The largest scans in the
/// library cache are 5.2k x 6.4k (33 MP); an 8192-square frame is 67 MP.
pub const MAX_INPUT_PIXELS: usize = 96 * 1024 * 1024;

/// Chandra's `MAX_OUTPUT_TOKENS`: what a request gets when it does not say.
pub const DEFAULT_NEW_TOKENS: u32 = 12_384;

/// Hard cap on generated tokens, whatever the request asks for.
pub const MAX_NEW_TOKENS: u32 = 16_384;

/// Chandra `settings.MIN_IMAGE_DIM`: a page whose short side is under this
/// is upscaled so the short side reaches it before the area fit.
pub const MIN_IMAGE_DIM: usize = 1536;

/// Chandra `scale_to_fit` `max_size` (w, h): the page area is capped at
/// this product, aspect preserved.
pub const MAX_FIT: (usize, usize) = (3072, 2048);

/// Chandra `scale_to_fit` `min_size`: a page below this area is upscaled.
pub const MIN_FIT: (usize, usize) = (1792, 28);

/// Chandra `scale_to_fit` `grid_size`: the fit works in 28 px blocks (the
/// Qwen2-VL processor convention its pipeline was written for); the tower
/// then rounds to its own 32 px token grid.
pub const FIT_GRID: usize = 28;

/// The session context reserved for one page: the largest fit (6144 image
/// tokens) + the prompt (~400) + the full output budget, rounded up.
pub const MAX_CONTEXT: u32 = 20_480;

/// Chandra `MAX_VLLM_RETRIES` is 6; each retry is a full re-decode, so the
/// service default is tighter and `OcrRequest::retries` can raise it.
pub const DEFAULT_RETRIES: u32 = 3;

/// Decode context a lane must keep free for the answer after the page and
/// the prompt are in. Under this a page is refused rather than half-read:
/// truncating at 200 tokens produces a plausible fragment of a transcript,
/// which is worse than an error because nothing downstream can tell.
pub const MIN_OUTPUT_ROOM: usize = 256;

/// Per-lane context for `lanes` lanes out of the [`MAX_CONTEXT`] budget.
///
/// The lanes DIVIDE the arena; they do not multiply it. The attention arena
/// is `lanes * per-lane`, so two lanes of the full budget would ask for twice
/// the KV the box was sized for and fail at load — after the weights are
/// already resident.
pub fn context_for_lanes(total: u32, lanes: usize) -> u32 {
    (total / lanes.max(1) as u32).max(1)
}

/// The tags Chandra's prompt allows, in the order and Python list rendering
/// (`['math', 'br', ...]`) the weights were trained against.
const ALLOWED_TAGS: &str = "['math', 'br', 'i', 'b', 'u', 'del', 'sup', 'sub', 'table', 'tr', 'td', 'p', 'th', 'div', 'pre', 'h1', 'h2', 'h3', 'h4', 'h5', 'ul', 'ol', 'li', 'input', 'a', 'span', 'img', 'hr', 'tbody', 'small', 'caption', 'strong', 'thead', 'big', 'code', 'chem']";

const ALLOWED_ATTRIBUTES: &str = "['class', 'colspan', 'rowspan', 'display', 'checked', 'type', 'border', 'value', 'style', 'href', 'alt', 'align', 'data-bbox', 'data-label']";

fn prompt_ending() -> String {
    format!(
        "Only use these tags {ALLOWED_TAGS}, and these attributes {ALLOWED_ATTRIBUTES}.\n\
\n\
Guidelines:\n\
* Inline math: Surround math with <math>...</math> tags. Math expressions should be rendered in KaTeX-compatible LaTeX. Use display for block math.\n\
* Tables: Use colspan and rowspan attributes to match table structure.\n\
* Formatting: Maintain consistent formatting with the image, including spacing, indentation, subscripts/superscripts, and special characters.\n\
* Images: Include a description of any images in the alt attribute of an <img> tag. Do not fill out the src property. Describe in detail inside the div tag. Also convert charts to high fidelity data, and convert diagrams to mermaid.\n\
* Forms: Mark checkboxes and radio buttons properly.\n\
* Text: join lines together properly into paragraphs using <p>...</p> tags.  Use <br> tags for line breaks within paragraphs, but only when absolutely necessary to maintain meaning.\n\
* Chemistry: Use <chem>...</chem> tags for chemical formulas with reactive SMILES.\n\
* Lists: Preserve indents and proper list markers.\n\
* Use the simplest possible HTML structure that accurately represents the content of the block.\n\
* Make sure the text is accurate and easy for a human to read and interpret.  Reading order should be correct and natural."
    )
}

/// Chandra `OCR_PROMPT`: plain transcription to HTML.
pub fn ocr_prompt() -> String {
    format!("OCR this image to HTML.\n\n{}", prompt_ending())
}

/// Chandra `OCR_LAYOUT_PROMPT`: transcription as labelled layout blocks
/// with normalised (0-1000) bounding boxes.
pub fn ocr_layout_prompt() -> String {
    format!(
        "OCR this image to HTML, arranged as layout blocks.  Each layout block should be a div with the data-bbox attribute representing the bounding box of the block in x0 y0 x1 y1 format.  Bboxes are normalized 0-1000. The data-label attribute is the label for the block.\n\
\n\
Use the following labels:\n\
- Caption\n\
- Footnote\n\
- Equation-Block\n\
- List-Group\n\
- Page-Header\n\
- Page-Footer\n\
- Image\n\
- Section-Header\n\
- Table\n\
- Text\n\
- Complex-Block\n\
- Code-Block\n\
- Form\n\
- Table-Of-Contents\n\
- Figure\n\
- Chemical-Block\n\
- Diagram\n\
- Bibliography\n\
- Blank-Page\n\
\n\
{}",
        prompt_ending()
    )
}

/// Which transcription a request asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OcrPrompt {
    /// Plain HTML transcription.
    Text,
    /// Layout blocks with bounding boxes.
    Layout,
    /// A caller-written prompt, sent verbatim.
    Custom(String),
}

impl OcrPrompt {
    /// The wire rule: empty = text, `layout` = layout, anything else custom.
    pub fn from_wire(prompt: &str) -> Self {
        match prompt.trim() {
            "" => OcrPrompt::Text,
            "layout" => OcrPrompt::Layout,
            other => OcrPrompt::Custom(other.to_string()),
        }
    }

    pub fn text(&self) -> String {
        match self {
            OcrPrompt::Text => ocr_prompt(),
            OcrPrompt::Layout => ocr_layout_prompt(),
            OcrPrompt::Custom(custom) => custom.clone(),
        }
    }
}

/// ChatML prefix that opens the user turn and the image.
pub const OCR_PREFIX: &str = "<|im_start|>user\n<|vision_start|>";

/// The text after the image embeddings: the prompt, then the assistant turn
/// with an empty think block — exactly what Chandra's chat template renders
/// with `add_generation_prompt=True`.
pub fn ocr_suffix(prompt: &str) -> String {
    format!("<|vision_end|>{prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

/// Port of Chandra `scale_to_fit`: the (width, height) a page is resized
/// to, in whole `FIT_GRID` blocks, area within [`MIN_FIT`, `MAX_FIT`],
/// aspect preserved as closely as the block grid allows. `(0, 0)` for an
/// empty image.
pub fn scale_to_fit(width: usize, height: usize) -> (usize, usize) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let grid = FIT_GRID;
    let original_ar = width as f64 / height as f64;
    let current_pixels = (width * height) as f64;
    let max_pixels = (MAX_FIT.0 * MAX_FIT.1) as f64;
    let min_pixels = (MIN_FIT.0 * MIN_FIT.1) as f64;

    let mut scale = 1.0;
    if current_pixels > max_pixels {
        scale = (max_pixels / current_pixels).sqrt();
    } else if current_pixels < min_pixels {
        scale = (min_pixels / current_pixels).sqrt();
    }

    // Python's round() is banker's rounding; the reference rounds block
    // counts with it, so match it rather than round-half-away.
    let mut w_blocks = round_half_even(width as f64 * scale / grid as f64).max(1);
    let mut h_blocks = round_half_even(height as f64 * scale / grid as f64).max(1);

    while (w_blocks * h_blocks * grid * grid) as f64 > max_pixels {
        if w_blocks == 1 && h_blocks == 1 {
            break;
        }
        if w_blocks == 1 {
            h_blocks -= 1;
            continue;
        }
        if h_blocks == 1 {
            w_blocks -= 1;
            continue;
        }
        let ar_w_loss = ((w_blocks - 1) as f64 / h_blocks as f64 - original_ar).abs();
        let ar_h_loss = (w_blocks as f64 / (h_blocks - 1) as f64 - original_ar).abs();
        if ar_w_loss < ar_h_loss {
            w_blocks -= 1;
        } else {
            h_blocks -= 1;
        }
    }
    (w_blocks * grid, h_blocks * grid)
}

fn round_half_even(x: f64) -> usize {
    let floor = x.floor();
    let diff = x - floor;
    let rounded = if diff > 0.5 {
        floor + 1.0
    } else if diff < 0.5 {
        floor
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    };
    rounded.max(0.0) as usize
}

/// The full Chandra page fit: `load_image`'s short-side floor, then
/// `scale_to_fit`, then the tower's own 32 px token grid (nearest multiple,
/// as the Qwen3.5 processor's smart resize does for an image already inside
/// its pixel limits). Returns the (width, height) the page is resampled to.
pub fn page_fit(width: usize, height: usize, align: usize) -> (usize, usize) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let (mut w, mut h) = (width as f64, height as f64);
    let short = w.min(h);
    if (short as usize) < MIN_IMAGE_DIM {
        let scale = MIN_IMAGE_DIM as f64 / short;
        w = (w * scale).floor();
        h = (h * scale).floor();
    }
    let (fw, fh) = scale_to_fit(w as usize, h as usize);
    let align = align.max(1);
    let round = |v: usize| (((v as f64) / align as f64).round() as usize).max(1) * align;
    (round(fw), round(fh))
}

/// Resample tightly packed RGB8 to an exact size: area-averaging when
/// shrinking, bilinear when growing (per axis). A page is either a large
/// scan coming down or a small one going up; the same page never does both.
pub fn resample_rgb8(
    src: &[u8],
    sw: usize,
    sh: usize,
    tw: usize,
    th: usize,
) -> Vec<u8> {
    if sw == tw && sh == th {
        return src.to_vec();
    }
    if tw == 0 || th == 0 || sw == 0 || sh == 0 {
        return Vec::new();
    }
    if tw <= sw && th <= sh {
        return area_average_rgb8(src, sw, sh, tw, th);
    }
    bilinear_rgb8(src, sw, sh, tw, th)
}

fn area_average_rgb8(src: &[u8], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<u8> {
    let mut out = vec![0u8; tw * th * 3];
    let x_scale = sw as f64 / tw as f64;
    let y_scale = sh as f64 / th as f64;
    for y in 0..th {
        let sy0 = (y as f64 * y_scale).floor() as usize;
        let sy1 = (((y + 1) as f64 * y_scale).ceil() as usize).min(sh).max(sy0 + 1);
        for x in 0..tw {
            let sx0 = (x as f64 * x_scale).floor() as usize;
            let sx1 = (((x + 1) as f64 * x_scale).ceil() as usize).min(sw).max(sx0 + 1);
            let mut acc = [0u64; 3];
            let mut n = 0u64;
            for sy in sy0..sy1 {
                let row = sy * sw;
                for sx in sx0..sx1 {
                    let p = (row + sx) * 3;
                    acc[0] += src[p] as u64;
                    acc[1] += src[p + 1] as u64;
                    acc[2] += src[p + 2] as u64;
                    n += 1;
                }
            }
            let d = (y * tw + x) * 3;
            out[d] = ((acc[0] + n / 2) / n) as u8;
            out[d + 1] = ((acc[1] + n / 2) / n) as u8;
            out[d + 2] = ((acc[2] + n / 2) / n) as u8;
        }
    }
    out
}

fn bilinear_rgb8(src: &[u8], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<u8> {
    let mut out = vec![0u8; tw * th * 3];
    let x_ratio = if tw > 1 { (sw - 1) as f64 / (tw - 1) as f64 } else { 0.0 };
    let y_ratio = if th > 1 { (sh - 1) as f64 / (th - 1) as f64 } else { 0.0 };
    for y in 0..th {
        let py = y as f64 * y_ratio;
        let y0 = (py.floor() as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let fy = py - y0 as f64;
        for x in 0..tw {
            let px = x as f64 * x_ratio;
            let x0 = (px.floor() as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let fx = px - x0 as f64;
            let d = (y * tw + x) * 3;
            for c in 0..3 {
                let s00 = src[(y0 * sw + x0) * 3 + c] as f64;
                let s01 = src[(y0 * sw + x1) * 3 + c] as f64;
                let s10 = src[(y1 * sw + x0) * 3 + c] as f64;
                let s11 = src[(y1 * sw + x1) * 3 + c] as f64;
                let top = s00 + (s01 - s00) * fx;
                let bottom = s10 + (s11 - s10) * fx;
                out[d + c] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Port of Chandra `detect_repeat_token`: true when the END of `text`
/// consists of one sequence (1..=250 chars) repeated more than a budget
/// that shrinks with the sequence length — a generation caught in a loop.
/// `cut_from_end` drops that many trailing chars first (the reference
/// checks both the full text and the text minus its last 50 chars).
pub fn detect_repeat(text: &str, cut_from_end: usize) -> bool {
    const BASE_MAX_REPEATS: f64 = 4.0;
    const WINDOW_SIZE: usize = 500;
    const SCALING_FACTOR: f64 = 3.0;
    let chars: Vec<char> = text.chars().collect();
    let chars = if cut_from_end > 0 && cut_from_end < chars.len() {
        &chars[..chars.len() - cut_from_end]
    } else if cut_from_end > 0 {
        return false;
    } else {
        &chars[..]
    };
    let len = chars.len();
    for seq_len in 1..=(WINDOW_SIZE / 2) {
        if seq_len > len {
            break;
        }
        let candidate = &chars[len - seq_len..];
        let max_repeats = (BASE_MAX_REPEATS * (1.0 + SCALING_FACTOR / seq_len as f64)) as usize;
        let mut repeats = 0usize;
        let mut pos = len - seq_len;
        loop {
            if &chars[pos..pos + seq_len] == candidate {
                repeats += 1;
                if repeats > max_repeats {
                    return true;
                }
                if pos < seq_len {
                    break;
                }
                pos -= seq_len;
            } else {
                break;
            }
        }
    }
    false
}

/// Chandra's whole-answer loop test: the text as generated, and the text
/// minus a 50-char tail (a loop that just ended in a stop token).
pub fn looks_looped(text: &str) -> bool {
    detect_repeat(text, 0) || (text.chars().count() > 50 && detect_repeat(text, 50))
}

/// Request-shape validation for the ocr domain, applied at `POST
/// /generate` so a caller gets a 400 instead of a queued job that fails on
/// the worker minutes later. Does not decode the image.
pub fn validate_ocr_params(params: &GenerateParams) -> Result<(), AssetAiError> {
    if params.pull_only {
        return Ok(());
    }
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params(
            "ocr: input_b64 is required (a PNG or JPEG page, or an mp4/mov keyframe)".to_string(),
        ));
    }
    if params.input_bytes.len() > MAX_INPUT_BYTES {
        return Err(AssetAiError::Params(format!(
            "ocr: input image is {} bytes, over the {MAX_INPUT_BYTES} byte limit",
            params.input_bytes.len()
        )));
    }
    image_kind(&params.input_bytes).map(|_| ()).map_err(as_ocr_error)
}

/// The shared image helpers speak as the vision domain; their refusals are
/// re-labelled for this domain without double-wrapping the error.
fn as_ocr_error(e: AssetAiError) -> AssetAiError {
    match e {
        AssetAiError::Params(m) => AssetAiError::Params(m.replacen("vision:", "ocr:", 1)),
        other => other,
    }
}

/// Refuses a decoded page whose pixel count is past what any scan in the
/// wild needs; the fit brings anything under it down to ~6 MP anyway.
pub fn check_page_dimensions(width: usize, height: usize) -> Result<(), AssetAiError> {
    if width == 0 || height == 0 {
        return Err(AssetAiError::Params("ocr: input page has a zero dimension".to_string()));
    }
    let pixels = width.saturating_mul(height);
    if pixels > MAX_INPUT_PIXELS {
        return Err(AssetAiError::Params(format!(
            "ocr: input page is {width}x{height} ({pixels} pixels), over the {MAX_INPUT_PIXELS} pixel limit"
        )));
    }
    Ok(())
}

/// The ocr domain's registry contract, checked by the crate tests.
pub fn spec_is_servable(spec: &crate::registry::ModelSpec) -> Result<(), String> {
    if spec.domain != crate::registry::Domain::Ocr {
        return Err(format!("model {} is not in the ocr domain", spec.id));
    }
    for role in ["llm-gguf", "mmproj"] {
        if spec.file_by_role(role).is_none() {
            return Err(format!("model {} has no {role:?} artifact", spec.id));
        }
    }
    Ok(())
}

/// One page to transcribe, already decoded.
pub struct OcrRequest {
    pub prompt: OcrPrompt,
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub max_new_tokens: u32,
    /// Hotter re-decodes allowed after a looping answer.
    pub retries: u32,
}

/// A transcribed page plus what it cost — the numbers a bench prints.
#[derive(Clone, Debug, Default)]
pub struct OcrPage {
    pub html: String,
    /// Page size actually fed to the tower.
    pub fed_width: usize,
    pub fed_height: usize,
    pub image_tokens: usize,
    pub output_tokens: usize,
    /// 1 = the greedy pass answered; more = loops were detected and retried.
    pub attempts: u32,
    /// True when even the last attempt still looked looped.
    pub looped: bool,
    pub encode_s: f64,
    pub prefill_s: f64,
    pub decode_s: f64,
}

#[cfg(feature = "llm")]
pub use resident::OcrBackend;

#[cfg(feature = "llm")]
mod resident {
    use super::*;
    use std::path::PathBuf;

    /// The resident OCR model: tower + session on ONE thread, kept warm
    /// across pages. One backend object per model id; `unload` drops the
    /// weights.
    pub struct OcrBackend {
        model_id: String,
        worker: Option<worker::OcrWorker>,
        loaded: Option<(PathBuf, PathBuf, usize)>,
    }

    impl OcrBackend {
        pub fn new(model_id: &str) -> Self {
            Self {
                model_id: model_id.to_string(),
                worker: None,
                loaded: None,
            }
        }

        /// Lanes the resident session was built for, or 0 while unloaded.
        pub fn lanes(&self) -> usize {
            self.loaded.as_ref().map(|(_, _, lanes)| *lanes).unwrap_or(0)
        }

        /// Brings the tower + session up from an explicit file pair.
        /// Idempotent for an unchanged pair; a changed pair drops the old
        /// session BEFORE loading the new one.
        pub fn load_from_paths(
            &mut self,
            gguf: PathBuf,
            mmproj: PathBuf,
            progress: &mut dyn FnMut(&str, f64),
        ) -> Result<(), AssetAiError> {
            self.load_from_paths_with_lanes(gguf, mmproj, 1, progress)
        }

        /// [`load_from_paths`](Self::load_from_paths) for a session that holds
        /// `lanes` pages at once.
        ///
        /// The lane count is fixed at load because it decides the shape of the
        /// caches: the attention arena is `lanes * per-lane context` rows and
        /// the recurrent arena one block per lane. Changing it means new
        /// weights on the device, so it belongs here and not on a request.
        pub fn load_from_paths_with_lanes(
            &mut self,
            gguf: PathBuf,
            mmproj: PathBuf,
            lanes: usize,
            progress: &mut dyn FnMut(&str, f64),
        ) -> Result<(), AssetAiError> {
            let lanes = lanes.max(1);
            let want = (gguf.clone(), mmproj.clone(), lanes);
            if self.worker.is_some() && self.loaded.as_ref() == Some(&want) {
                return Ok(());
            }
            self.worker = None;
            self.loaded = None;
            let worker =
                worker::OcrWorker::spawn(gguf, mmproj, lanes, self.model_id.clone(), progress)
                    .map_err(|e| AssetAiError::Backend(format!("ocr load: {e}")))?;
            self.worker = Some(worker);
            self.loaded = Some(want);
            Ok(())
        }

        /// Transcribe a batch of pages with every lane of the session
        /// resident at once, and hand each page back as it finishes.
        ///
        /// The aggregate is what improves, not the individual page: a lane
        /// decodes no faster than it did alone, but N of them share one pass
        /// over the weights per step, and that pass is what a single stream
        /// spends almost all of its time on. A finished lane is refilled from
        /// the queue while the others keep decoding.
        ///
        /// `on_page` is called with the submission index, so results can be
        /// put back in order however they finish.
        pub fn ocr_pages(
            &self,
            requests: Vec<OcrRequest>,
            cancel: &CancelToken,
            on_stage: &mut dyn FnMut(&str, f64),
            on_page: &mut dyn FnMut(usize, Result<OcrPage, String>),
        ) -> Result<(), AssetAiError> {
            let worker = self.worker.as_ref().ok_or_else(|| {
                AssetAiError::Backend("ocr backend used before ensure_loaded".to_string())
            })?;
            worker
                .ask_batch(requests, cancel, on_stage, on_page)
                .map_err(|e| {
                    if e == "cancelled" {
                        AssetAiError::Cancelled
                    } else {
                        AssetAiError::Backend(format!("ocr: {e}"))
                    }
                })
        }

        /// Transcribe one decoded page. This is the whole request path
        /// minus the wire: the bench calls it directly.
        pub fn ocr_page(
            &self,
            request: OcrRequest,
            cancel: &CancelToken,
            on_stage: &mut dyn FnMut(&str, f64),
            on_text: &mut dyn FnMut(&str),
        ) -> Result<OcrPage, AssetAiError> {
            let worker = self.worker.as_ref().ok_or_else(|| {
                AssetAiError::Backend("ocr backend used before ensure_loaded".to_string())
            })?;
            worker.ask(request, cancel, on_stage, on_text).map_err(|e| {
                if e == "cancelled" {
                    AssetAiError::Cancelled
                } else {
                    AssetAiError::Backend(format!("ocr: {e}"))
                }
            })
        }
    }

    impl ContentBackend for OcrBackend {
        fn model_id(&self) -> &str {
            &self.model_id
        }

        fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            let gguf = ctx.path_by_role("llm-gguf")?;
            let mmproj = ctx.path_by_role("mmproj")?;
            if self.worker.is_some()
                && self.loaded.as_ref() == Some(&(gguf.clone(), mmproj.clone(), 1))
            {
                return Ok(());
            }
            let gb = std::fs::metadata(&gguf).map(|m| m.len() as f64 / 1e9).unwrap_or(0.0);
            (ctx.progress)(&format!("load ocr gguf ({gb:.1}GB) + tower"), 0.1);
            self.load_from_paths_with_lanes(gguf, mmproj, 1, ctx.progress)?;
            (ctx.progress)("ocr session ready", 0.9);
            Ok(())
        }

        fn is_resident(&self) -> bool {
            self.worker.is_some()
        }

        fn unload(&mut self) -> Result<(), AssetAiError> {
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
            validate_ocr_params(params)?;
            progress("decode page", 0.02);
            let (rgb, width, height) =
                decode_image_rgb8_within(&params.input_bytes, MAX_INPUT_PIXELS).map_err(as_ocr_error)?;
            check_page_dimensions(width, height)?;
            cancel.check()?;
            let max_new_tokens = if params.max_tokens == 0 {
                DEFAULT_NEW_TOKENS
            } else {
                params.max_tokens.min(MAX_NEW_TOKENS)
            };
            let page = self.ocr_page(
                OcrRequest {
                    prompt: OcrPrompt::from_wire(&params.prompt),
                    rgb,
                    width,
                    height,
                    max_new_tokens,
                    retries: DEFAULT_RETRIES,
                },
                cancel,
                progress,
                on_text,
            )?;
            eprintln!(
                "[ocr] {}: {}x{} -> {}x{} = {} image tokens, {} out tokens, {} attempt(s){}, encode {:.2}s prefill {:.2}s decode {:.2}s",
                self.model_id,
                width,
                height,
                page.fed_width,
                page.fed_height,
                page.image_tokens,
                page.output_tokens,
                page.attempts,
                if page.looped { " (still looped)" } else { "" },
                page.encode_s,
                page.prefill_s,
                page.decode_s,
            );
            progress("done", 1.0);
            Ok(vec![ArtifactData {
                content_type: "text/html; charset=utf-8",
                ext: "html",
                bytes: page.html.into_bytes(),
            }])
        }
    }

    mod worker {
        use super::*;
        use makepad_ai_llm::{
            preprocess_rgb8, GgufFile, LlamaSamplerState, LlamaSamplingParams, LlamaSession,
            LlamaSessionConfig, LlamaTextDecoder, SlotTable, VisionConfig, VisionTower,
        };
        use std::collections::VecDeque;
        use std::sync::mpsc;
        use std::time::Instant;

        pub enum WorkerEvent {
            Stage(String, f64),
            /// Full answer text so far — prefix-stable snapshots.
            Text(String),
            Done(Result<OcrPage, String>),
        }

        /// What a batch run reports back, page by page as lanes finish.
        pub enum BatchEvent {
            Stage(String, f64),
            /// Submission index and its result.
            Page(usize, Result<OcrPage, String>),
            Done(Result<(), String>),
        }

        enum WorkerMsg {
            Ask(OcrRequest, CancelToken, mpsc::Sender<WorkerEvent>),
            AskBatch(Vec<OcrRequest>, CancelToken, mpsc::Sender<BatchEvent>),
        }

        /// Handle to the thread that owns the resident tower + session
        /// (`VisionTower` and `LlamaSession` are `!Send`).
        pub struct OcrWorker {
            tx: mpsc::Sender<WorkerMsg>,
        }

        impl OcrWorker {
            pub fn spawn(
                gguf: PathBuf,
                mmproj: PathBuf,
                lanes: usize,
                model_id: String,
                progress: &mut dyn FnMut(&str, f64),
            ) -> Result<Self, String> {
                enum BootEvt {
                    Progress(String, f64),
                    Ready(Result<(), String>),
                }
                let (tx, rx) = mpsc::channel::<WorkerMsg>();
                let (boot_tx, boot_rx) = mpsc::channel::<BootEvt>();
                std::thread::Builder::new()
                    .name("ocr-worker".to_string())
                    .spawn(move || {
                        let loaded = (|| -> Result<(VisionTower, LlamaSession, VisionConfig), String> {
                            let mmproj_path = mmproj.to_string_lossy().to_string();
                            let file = GgufFile::open(&mmproj_path)
                                .map_err(|e| format!("open mmproj: {e:?}"))?;
                            let config = VisionConfig::from_gguf(&file)
                                .map_err(|e| format!("mmproj vision config: {e:?}"))?;
                            let _ = boot_tx.send(BootEvt::Progress("load vision tower".into(), 0.15));
                            let tower = VisionTower::load(&mmproj_path)
                                .map_err(|e| format!("load vision tower: {e:?}"))?;
                            let session = LlamaSession::load_with_progress(
                                &gguf,
                                LlamaSessionConfig {
                                    // Lanes divide the budget: the arena is
                                    // `lanes * max_context` rows, so asking
                                    // each lane for the whole of it would
                                    // size the KV for a box that is not
                                    // there.
                                    max_context: Some(context_for_lanes(MAX_CONTEXT, lanes)),
                                    max_sequences: lanes.max(1) as u32,
                                    // A page is 3-6k image tokens of prefill;
                                    // the session default (32) is a chat
                                    // turn's shape and made prefill the
                                    // longest leg (4090: ~600 tok/s). 512 is
                                    // what the chat lanes run.
                                    prefill_batch_size: PREFILL_BATCH,
                                    // 512-row prefill steps through a 4B
                                    // outgrow the session default arena
                                    // (512 MiB) by a few MB on the largest
                                    // pages; reserve room for the batch.
                                    extra_activation_bytes: EXTRA_ACTIVATION_BYTES,
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
                            Ok((tower, session, config))
                        })();
                        let (mut tower, mut session, config) = match loaded {
                            Ok(parts) => {
                                eprintln!(
                                    "[ocr] {model_id} resident on {}: page fit up to {}x{}, \
                                     {lanes} lane(s) x {} tokens",
                                    parts.0.device_description(),
                                    MAX_FIT.0,
                                    MAX_FIT.1,
                                    context_for_lanes(MAX_CONTEXT, lanes)
                                );
                                let _ = boot_tx.send(BootEvt::Ready(Ok(())));
                                parts
                            }
                            Err(err) => {
                                let _ = boot_tx.send(BootEvt::Ready(Err(err)));
                                return;
                            }
                        };
                        while let Ok(msg) = rx.recv() {
                            match msg {
                                WorkerMsg::Ask(ask, cancel, events) => {
                                    let result = run_page(
                                        &mut tower, &mut session, &config, ask, &cancel, &events,
                                    );
                                    let _ = events.send(WorkerEvent::Done(result));
                                }
                                WorkerMsg::AskBatch(asks, cancel, events) => {
                                    let result = run_pages(
                                        &mut tower, &mut session, &config, lanes, asks, &cancel,
                                        &events,
                                    );
                                    let _ = events.send(BatchEvent::Done(result));
                                }
                            }
                        }
                    })
                    .map_err(|e| format!("spawn ocr worker: {e}"))?;
                loop {
                    match boot_rx.recv() {
                        Ok(BootEvt::Progress(stage, frac)) => progress(&stage, frac),
                        Ok(BootEvt::Ready(Ok(()))) => break,
                        Ok(BootEvt::Ready(Err(err))) => return Err(err),
                        Err(_) => return Err("ocr worker died during load".to_string()),
                    }
                }
                Ok(Self { tx })
            }

            pub fn ask(
                &self,
                ask: OcrRequest,
                cancel: &CancelToken,
                on_stage: &mut dyn FnMut(&str, f64),
                on_text: &mut dyn FnMut(&str),
            ) -> Result<OcrPage, String> {
                let (event_tx, event_rx) = mpsc::channel();
                self.tx
                    .send(WorkerMsg::Ask(ask, cancel.clone(), event_tx))
                    .map_err(|_| "ocr worker thread is gone".to_string())?;
                loop {
                    match event_rx.recv() {
                        Ok(WorkerEvent::Stage(stage, frac)) => on_stage(&stage, frac),
                        Ok(WorkerEvent::Text(text)) => on_text(&text),
                        Ok(WorkerEvent::Done(result)) => return result,
                        Err(_) => return Err("ocr worker dropped the reply".to_string()),
                    }
                }
            }

            pub fn ask_batch(
                &self,
                asks: Vec<OcrRequest>,
                cancel: &CancelToken,
                on_stage: &mut dyn FnMut(&str, f64),
                on_page: &mut dyn FnMut(usize, Result<OcrPage, String>),
            ) -> Result<(), String> {
                let (event_tx, event_rx) = mpsc::channel();
                self.tx
                    .send(WorkerMsg::AskBatch(asks, cancel.clone(), event_tx))
                    .map_err(|_| "ocr worker thread is gone".to_string())?;
                loop {
                    match event_rx.recv() {
                        Ok(BatchEvent::Stage(stage, frac)) => on_stage(&stage, frac),
                        Ok(BatchEvent::Page(index, page)) => on_page(index, page),
                        Ok(BatchEvent::Done(result)) => return result,
                        Err(_) => return Err("ocr worker dropped the reply".to_string()),
                    }
                }
            }
        }

        /// Prefill rows per step for the page's image tokens.
        const PREFILL_BATCH: usize = 512;
        /// Activation arena reserved beside the weights for those steps.
        const EXTRA_ACTIVATION_BYTES: usize = 1 << 30;
        /// How often a growing answer is published as `partial_text`.
        const TEXT_SNAPSHOT_EVERY: usize = 16;
        /// How often the tail is checked for a loop while decoding. The
        /// reference only checks the finished answer; checking on the way
        /// turns a 12k-token loop into a few hundred tokens and a retry.
        const LOOP_CHECK_EVERY: usize = 64;
        /// Chandra's retry temperature schedule: 0.2 hotter per retry, 0.8 max.
        const RETRY_TEMPERATURE_STEP: f32 = 0.2;
        const RETRY_TEMPERATURE_MAX: f32 = 0.8;
        const RETRY_TOP_P: f32 = 0.95;

        fn run_page(
            tower: &mut VisionTower,
            session: &mut LlamaSession,
            config: &VisionConfig,
            ask: OcrRequest,
            cancel: &CancelToken,
            events: &mpsc::Sender<WorkerEvent>,
        ) -> Result<OcrPage, String> {
            let cancelled = || "cancelled".to_string();
            let stage = |name: &str, frac: f64| {
                let _ = events.send(WorkerEvent::Stage(name.to_string(), frac));
            };
            if cancel.is_cancelled() {
                return Err(cancelled());
            }

            // Page fit: Chandra's rule, then the tower's 32 px grid. The
            // resample happens here at the fitted size, so the tower's own
            // preprocessor (built for caption-sized sheets) sees an image
            // already inside its limits and only normalises it.
            stage("fit page", 0.03);
            let (fw, fh) = page_fit(ask.width, ask.height, config.align_size());
            if fw == 0 || fh == 0 {
                return Err("empty page".to_string());
            }
            let fitted = resample_rgb8(&ask.rgb, ask.width, ask.height, fw, fh);
            let mut fitted_config = config.clone();
            fitted_config.min_pixels = 0;
            fitted_config.max_pixels = usize::MAX / 4;
            let prepared = preprocess_rgb8(&fitted, fw, fh, &fitted_config)
                .map_err(|e| format!("preprocess: {e:?}"))?;
            drop(fitted);
            if (prepared.width, prepared.height) != (fw, fh) {
                return Err(format!(
                    "preprocess resized the fitted page {fw}x{fh} to {}x{}",
                    prepared.width, prepared.height
                ));
            }

            stage("encode page", 0.06);
            let t_encode = Instant::now();
            let embeddings = tower.encode(&prepared).map_err(|e| format!("encode: {e:?}"))?;
            let encode_s = t_encode.elapsed().as_secs_f64();
            if cancel.is_cancelled() {
                return Err(cancelled());
            }

            let vocab = session.vocab();
            let prefix_ids = vocab
                .tokenize(OCR_PREFIX, false, true)
                .map_err(|e| format!("tokenize prefix: {e:?}"))?;
            let suffix = ocr_suffix(&ask.prompt.text());
            let suffix_ids = vocab
                .tokenize(&suffix, false, true)
                .map_err(|e| format!("tokenize prompt: {e:?}"))?;
            // Stop on the turn end AND the end-of-text token: Chandra's
            // generation config names only <|endoftext|>, but the model ends
            // its turn with <|im_end|>, so its reference runner adds both.
            let mut stops: Vec<i32> = session.stop_tokens();
            for name in ["<|im_end|>", "<|endoftext|>"] {
                if let Some(id) = vocab.token_id(name) {
                    if !stops.contains(&id) {
                        stops.push(id);
                    }
                }
            }
            let image_tokens = prepared.tokens_w() * prepared.tokens_h();
            let max_new = ask.max_new_tokens.max(1) as usize;
            let needed = prefix_ids.len() + image_tokens + suffix_ids.len() + max_new;
            let max_context = session.max_context();
            let max_new = if needed > max_context {
                // The output budget yields; the page never does.
                let room = max_context.saturating_sub(prefix_ids.len() + image_tokens + suffix_ids.len());
                if room < 256 {
                    return Err(format!(
                        "page needs {} tokens ({image_tokens} image + {} prompt) but the session holds {max_context}",
                        prefix_ids.len() + image_tokens + suffix_ids.len(),
                        prefix_ids.len() + suffix_ids.len()
                    ));
                }
                room
            } else {
                max_new
            };

            let mut page = OcrPage {
                fed_width: fw,
                fed_height: fh,
                image_tokens,
                encode_s,
                ..OcrPage::default()
            };
            let attempts = 1 + ask.retries;
            for attempt in 0..attempts {
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }
                page.attempts = attempt + 1;
                // A fresh turn: clear KV + recurrent state, prefill the
                // prefix, the page, the prompt.
                stage(&format!("prefill (attempt {})", attempt + 1), 0.1);
                let t_prefill = Instant::now();
                session.reset().map_err(|e| format!("reset: {e:?}"))?;
                session
                    .append_tokens(&prefix_ids)
                    .map_err(|e| format!("prefill prefix: {e:?}"))?;
                session
                    .append_image_embeddings(&embeddings, prepared.tokens_w(), prepared.tokens_h())
                    .map_err(|e| format!("prefill page: {e:?}"))?;
                session
                    .append_tokens(&suffix_ids)
                    .map_err(|e| format!("prefill prompt: {e:?}"))?;
                page.prefill_s += t_prefill.elapsed().as_secs_f64();

                let temperature = if attempt == 0 {
                    0.0
                } else {
                    (RETRY_TEMPERATURE_STEP * attempt as f32).min(RETRY_TEMPERATURE_MAX)
                };
                let params = LlamaSamplingParams {
                    temperature,
                    top_p: if attempt == 0 { 0.1 } else { RETRY_TOP_P },
                    top_k: 0,
                    seed: 0x0c8a + attempt as u64,
                    ..LlamaSamplingParams::default()
                };
                let mut sampler = LlamaSamplerState::new(params.seed);

                let t_decode = Instant::now();
                let mut text = String::new();
                let mut decoder = session.vocab().text_decoder();
                let mut generated = 0usize;
                let mut looped = false;
                stage(&format!("decode 0/{max_new}"), 0.15);
                while generated < max_new {
                    if cancel.is_cancelled() {
                        return Err(cancelled());
                    }
                    let token = {
                        let logits = session
                            .last_logits()
                            .ok_or_else(|| "session has no logits after prefill".to_string())?;
                        sampler
                            .sample_logits(logits, params)
                            .map_err(|e| format!("sample: {e:?}"))?
                    };
                    if stops.contains(&token) {
                        break;
                    }
                    session
                        .append_token(token)
                        .map_err(|e| format!("generate: {e:?}"))?;
                    generated += 1;
                    if let Some(chunk) = decoder.push_token(session.vocab(), token) {
                        text.push_str(&chunk);
                    }
                    if generated % TEXT_SNAPSHOT_EVERY == 0 {
                        let _ = events.send(WorkerEvent::Text(text.clone()));
                        stage(
                            &format!("decode {generated}/{max_new}"),
                            0.15 + 0.8 * (generated as f64 / max_new as f64),
                        );
                    }
                    if generated % LOOP_CHECK_EVERY == 0 && detect_repeat(&text, 0) {
                        looped = true;
                        break;
                    }
                }
                page.decode_s += t_decode.elapsed().as_secs_f64();
                page.output_tokens = generated;
                let looped = looped || looks_looped(&text);
                page.html = text.trim().to_string();
                page.looped = looped;
                let _ = events.send(WorkerEvent::Text(page.html.clone()));
                if !looped {
                    break;
                }
                if attempt + 1 < attempts {
                    eprintln!(
                        "[ocr] loop detected after {generated} tokens, retrying at temperature {:.1}",
                        (RETRY_TEMPERATURE_STEP * (attempt + 1) as f32).min(RETRY_TEMPERATURE_MAX)
                    );
                }
            }
            Ok(page)
        }

        // ------------------------------------------------------------------
        // Multi-lane decode: several pages resident in one session at once.
        // ------------------------------------------------------------------

        /// One page while it occupies a lane.
        ///
        /// Everything the tower and the tokenizer produced lives here, so a
        /// hotter retry re-seats the SAME embeddings instead of re-encoding a
        /// page the vision side already read correctly.
        struct LaneJob {
            /// Submission index, so results can be put back in order.
            index: usize,
            prefix_ids: Vec<i32>,
            suffix_ids: Vec<i32>,
            embeddings: Vec<f32>,
            tokens_w: usize,
            tokens_h: usize,
            max_new: usize,
            attempt: u32,
            attempts: u32,
            /// Set when this lane looped and has an attempt left: it keeps the
            /// lane, and the next refill pass re-seats it hotter.
            needs_reseat: bool,
            page: OcrPage,
            text: String,
            decoder: LlamaTextDecoder,
            sampler: LlamaSamplerState,
            params: LlamaSamplingParams,
            /// The row this lane samples its next token from: the tail of its
            /// prefill, then the tail of every step it takes part in.
            logits: Vec<f32>,
            generated: usize,
        }

        /// Sampling knobs for attempt `attempt` — the single-stream path's
        /// schedule verbatim: greedy first, then Chandra's hotter retries.
        fn attempt_params(attempt: u32) -> LlamaSamplingParams {
            let temperature = if attempt == 0 {
                0.0
            } else {
                (RETRY_TEMPERATURE_STEP * attempt as f32).min(RETRY_TEMPERATURE_MAX)
            };
            LlamaSamplingParams {
                temperature,
                top_p: if attempt == 0 { 0.1 } else { RETRY_TOP_P },
                top_k: 0,
                seed: 0x0c8a + attempt as u64,
                ..LlamaSamplingParams::default()
            }
        }

        /// Fit, resample, encode and tokenize one page — everything that has
        /// to happen before a lane is involved. Serial, because there is one
        /// tower and a page is one graph through it.
        fn prepare_job(
            tower: &mut VisionTower,
            session: &LlamaSession,
            config: &VisionConfig,
            index: usize,
            ask: OcrRequest,
        ) -> Result<LaneJob, String> {
            let (fw, fh) = page_fit(ask.width, ask.height, config.align_size());
            if fw == 0 || fh == 0 {
                return Err("empty page".to_string());
            }
            let fitted = resample_rgb8(&ask.rgb, ask.width, ask.height, fw, fh);
            let mut fitted_config = config.clone();
            fitted_config.min_pixels = 0;
            fitted_config.max_pixels = usize::MAX / 4;
            let prepared = preprocess_rgb8(&fitted, fw, fh, &fitted_config)
                .map_err(|e| format!("preprocess: {e:?}"))?;
            drop(fitted);
            if (prepared.width, prepared.height) != (fw, fh) {
                return Err(format!(
                    "preprocess resized the fitted page {fw}x{fh} to {}x{}",
                    prepared.width, prepared.height
                ));
            }
            let t_encode = Instant::now();
            let embeddings = tower.encode(&prepared).map_err(|e| format!("encode: {e:?}"))?;
            let encode_s = t_encode.elapsed().as_secs_f64();

            let vocab = session.vocab();
            let prefix_ids = vocab
                .tokenize(OCR_PREFIX, false, true)
                .map_err(|e| format!("tokenize prefix: {e:?}"))?;
            let suffix = ocr_suffix(&ask.prompt.text());
            let suffix_ids = vocab
                .tokenize(&suffix, false, true)
                .map_err(|e| format!("tokenize prompt: {e:?}"))?;

            let image_tokens = prepared.tokens_w() * prepared.tokens_h();
            let prompt_tokens = prefix_ids.len() + image_tokens + suffix_ids.len();
            // A LANE's context, not the session's total — `max_context` IS the
            // per-lane figure once the session is built for several. Refused
            // by name, because a page that does not fit is not a page to
            // half-read: 200 tokens of a transcript look like a transcript.
            let lane_context = session.max_context();
            let room = lane_context.saturating_sub(prompt_tokens);
            if room < MIN_OUTPUT_ROOM {
                return Err(format!(
                    "page needs {prompt_tokens} tokens ({image_tokens} image + {} prompt) and a \
                     lane holds {lane_context}, leaving {room} for the answer; run fewer lanes",
                    prefix_ids.len() + suffix_ids.len()
                ));
            }
            let max_new = (ask.max_new_tokens.max(1) as usize).min(room);

            Ok(LaneJob {
                index,
                prefix_ids,
                suffix_ids,
                embeddings,
                tokens_w: prepared.tokens_w(),
                tokens_h: prepared.tokens_h(),
                max_new,
                attempt: 0,
                attempts: 1 + ask.retries,
                needs_reseat: false,
                page: OcrPage {
                    fed_width: fw,
                    fed_height: fh,
                    image_tokens,
                    encode_s,
                    ..OcrPage::default()
                },
                text: String::new(),
                decoder: session.vocab().text_decoder(),
                sampler: LlamaSamplerState::new(attempt_params(0).seed),
                params: attempt_params(0),
                logits: Vec::new(),
                generated: 0,
            })
        }

        /// Seat a prepared job in a lane: clear what the lane carried over,
        /// then prefill prefix, page and prompt into its own rows.
        ///
        /// Nothing here touches another lane. The recurrent state is cleared
        /// for THIS lane only — the whole-session `reset` the single-stream
        /// path uses would take every neighbour's page with it — and every
        /// prefill graph is windowed on this lane's rows, so a lane can be
        /// refilled while the others are mid-answer.
        fn seat_job(
            session: &mut LlamaSession,
            table: &mut SlotTable,
            lane: usize,
            job: &mut LaneJob,
        ) -> Result<(), String> {
            fn cursor(table: &SlotTable, lane: usize) -> Result<(usize, usize, usize, i64), String> {
                let slot = table
                    .slot(lane)
                    .ok_or_else(|| format!("lane {lane} is outside the slot table"))?;
                Ok((
                    slot.kv_base(),
                    slot.live_state_row(),
                    slot.fill(),
                    slot.rope_pos_next(),
                ))
            }
            session
                .clear_slot_state(lane)
                .map_err(|e| format!("clear lane {lane}: {e:?}"))?;
            table
                .admit_at(lane)
                .map_err(|e| format!("admit lane {lane}: {e:?}"))?;

            let mut logits = Vec::new();
            for chunk in job.prefix_ids.chunks(PREFILL_BATCH) {
                let (kv_base, state_row, fill, rope) = cursor(table, lane)?;
                logits = session
                    .prefill_slot_chunk_at_rope(lane, kv_base, state_row, fill, rope, chunk)
                    .map_err(|e| format!("prefill prefix: {e:?}"))?;
                table
                    .advance(lane, chunk.len())
                    .map_err(|e| format!("advance lane {lane}: {e:?}"))?;
            }
            {
                let (kv_base, state_row, fill, rope) = cursor(table, lane)?;
                logits = session
                    .prefill_slot_image_embeddings(
                        lane,
                        kv_base,
                        state_row,
                        fill,
                        rope,
                        &job.embeddings,
                        job.tokens_w,
                        job.tokens_h,
                    )
                    .map_err(|e| format!("prefill page: {e:?}"))?;
                // The counter split the whole lane path rests on: the page is
                // `tokens_w * tokens_h` cache rows but only
                // `max(tokens_w, tokens_h)` M-RoPE positions.
                table
                    .advance_image_span(
                        lane,
                        job.tokens_w * job.tokens_h,
                        job.tokens_w.max(job.tokens_h),
                    )
                    .map_err(|e| format!("advance lane {lane} over the page: {e:?}"))?;
            }
            for chunk in job.suffix_ids.chunks(PREFILL_BATCH) {
                let (kv_base, state_row, fill, rope) = cursor(table, lane)?;
                logits = session
                    .prefill_slot_chunk_at_rope(lane, kv_base, state_row, fill, rope, chunk)
                    .map_err(|e| format!("prefill prompt: {e:?}"))?;
                table
                    .advance(lane, chunk.len())
                    .map_err(|e| format!("advance lane {lane}: {e:?}"))?;
            }
            table
                .begin_decoding(lane)
                .map_err(|e| format!("lane {lane} cannot decode: {e:?}"))?;

            job.page.attempts = job.attempt + 1;
            job.params = attempt_params(job.attempt);
            job.sampler = LlamaSamplerState::new(job.params.seed);
            job.decoder = session.vocab().text_decoder();
            job.text.clear();
            job.generated = 0;
            job.logits = logits;
            Ok(())
        }

        /// What a lane that stopped generating should do next.
        enum LaneEnd {
            /// Its answer stands. The lane is free.
            Done,
            /// It looped and has a hotter attempt left. It KEEPS the lane and
            /// the next refill pass re-seats it — the neighbours never learn
            /// it happened.
            Reseat,
        }

        /// Close out a lane's attempt, deciding between the two.
        fn end_attempt(job: &mut LaneJob, looped: bool) -> LaneEnd {
            let looped = looped || looks_looped(&job.text);
            job.page.html = job.text.trim().to_string();
            job.page.output_tokens = job.generated;
            job.page.looped = looped;
            if looped && job.attempt + 1 < job.attempts {
                job.attempt += 1;
                job.needs_reseat = true;
                eprintln!(
                    "[ocr] lane loop detected after {} tokens, retrying at temperature {:.1}",
                    job.generated,
                    attempt_params(job.attempt).temperature
                );
                LaneEnd::Reseat
            } else {
                LaneEnd::Done
            }
        }

        /// Transcribe a batch with every lane of the session resident.
        ///
        /// The loop is: refill idle lanes from the queue (serial — one tower,
        /// one page at a time), sample one token per decoding lane, take the
        /// lanes that stopped out of the step, and step the rest together.
        /// The aggregate is what improves: one step is one pass over the
        /// weights shared by every lane in it, and that pass is where a single
        /// stream spends almost all of its time.
        fn run_pages(
            tower: &mut VisionTower,
            session: &mut LlamaSession,
            config: &VisionConfig,
            lanes: usize,
            asks: Vec<OcrRequest>,
            cancel: &CancelToken,
            events: &mpsc::Sender<BatchEvent>,
        ) -> Result<(), String> {
            let cancelled = || "cancelled".to_string();
            let total = asks.len();
            if total == 0 {
                return Ok(());
            }
            let lanes = lanes.max(1).min(session.slot_count());
            // Chandra ends its turn with <|im_end|> but its generation config
            // names only <|endoftext|>, so the reference runner adds both.
            let mut stops: Vec<i32> = session.stop_tokens();
            for name in ["<|im_end|>", "<|endoftext|>"] {
                if let Some(id) = session.vocab().token_id(name) {
                    if !stops.contains(&id) {
                        stops.push(id);
                    }
                }
            }
            let mut table = session
                .new_slot_table()
                .map_err(|e| format!("slot table: {e:?}"))?;
            let mut queue: VecDeque<(usize, OcrRequest)> = asks.into_iter().enumerate().collect();
            let mut jobs: Vec<Option<LaneJob>> = (0..lanes).map(|_| None).collect();
            let mut finished = 0usize;

            loop {
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }

                // 1. Refill: re-seat the lanes that looped, then fill the
                //    empty ones from the queue. Both stall whoever is still
                //    decoding — that is the cost of a refill, and it is why a
                //    lane is only ever refilled once it has actually stopped.
                for lane in 0..lanes {
                    if let Some(job) = jobs[lane].as_mut() {
                        if !job.needs_reseat {
                            continue;
                        }
                        job.needs_reseat = false;
                        let t_prefill = Instant::now();
                        match seat_job(session, &mut table, lane, jobs[lane].as_mut().unwrap()) {
                            Ok(()) => {
                                if let Some(job) = jobs[lane].as_mut() {
                                    job.page.prefill_s += t_prefill.elapsed().as_secs_f64();
                                }
                            }
                            Err(err) => {
                                let index = jobs[lane].as_ref().map(|job| job.index).unwrap_or(0);
                                jobs[lane] = None;
                                let _ = table.retire(lane);
                                finished += 1;
                                let _ = events.send(BatchEvent::Page(index, Err(err)));
                            }
                        }
                        continue;
                    }
                    let Some((index, ask)) = queue.pop_front() else {
                        continue;
                    };
                    let _ = events.send(BatchEvent::Stage(
                        format!("page {} of {total} into lane {lane}", index + 1),
                        finished as f64 / total as f64,
                    ));
                    let mut job = match prepare_job(tower, session, config, index, ask) {
                        Ok(job) => job,
                        Err(err) => {
                            finished += 1;
                            let _ = events.send(BatchEvent::Page(index, Err(err)));
                            continue;
                        }
                    };
                    let t_prefill = Instant::now();
                    match seat_job(session, &mut table, lane, &mut job) {
                        Ok(()) => {
                            job.page.prefill_s += t_prefill.elapsed().as_secs_f64();
                            jobs[lane] = Some(job);
                        }
                        Err(err) => {
                            let _ = table.retire(lane);
                            finished += 1;
                            let _ = events.send(BatchEvent::Page(index, Err(err)));
                        }
                    }
                }

                if jobs.iter().all(|job| job.is_none()) {
                    if queue.is_empty() {
                        return Ok(());
                    }
                    // Every lane refused its page and the queue still has
                    // work: go round and take the next one.
                    continue;
                }

                // 2. One token per decoding lane, from the row that lane owns.
                //    A lane that stops here does not join the step.
                let mut step_lanes: Vec<usize> = Vec::with_capacity(lanes);
                let mut step_tokens: Vec<i32> = Vec::with_capacity(lanes);
                let mut stopped: Vec<usize> = Vec::new();
                let mut failed: Vec<(usize, String)> = Vec::new();
                for lane in 0..lanes {
                    let Some(job) = jobs[lane].as_mut() else {
                        continue;
                    };
                    if job.generated >= job.max_new {
                        stopped.push(lane);
                        continue;
                    }
                    match job.sampler.sample_logits(&job.logits, job.params) {
                        Ok(token) if stops.contains(&token) => stopped.push(lane),
                        Ok(token) => {
                            step_lanes.push(lane);
                            step_tokens.push(token);
                        }
                        Err(e) => failed.push((lane, format!("sample: {e:?}"))),
                    }
                }
                for (lane, err) in failed {
                    let index = jobs[lane].as_ref().map(|job| job.index).unwrap_or(0);
                    jobs[lane] = None;
                    let _ = table.retire(lane);
                    finished += 1;
                    let _ = events.send(BatchEvent::Page(index, Err(err)));
                }

                // 3. Take the finishers out BEFORE planning, so the step is
                //    exactly as wide as the lanes that still have something to
                //    say. `park` and `retire` both leave the phase idle, which
                //    is what keeps them out of `plan_step`.
                for lane in stopped {
                    close_lane(&mut jobs, &mut table, lane, false, &mut finished, events);
                }
                if step_lanes.is_empty() {
                    continue;
                }

                // 4. One pass over the weights for every lane in the step.
                //
                // A lane in `step_lanes` sampled a token, so it holds a job,
                // so it is decoding, so the plan is not empty. Unreachable —
                // and it says so rather than looping, because going round
                // again would re-sample every lane against the same logits
                // and quietly advance their samplers forever.
                let Some(plan) = table.plan_step() else {
                    return Err(format!(
                        "lanes {step_lanes:?} have tokens to decode but no lane is decoding"
                    ));
                };
                // Logit row `i` belongs to `plan.slots[i].slot`, and this
                // loop maps row `i` onto `step_lanes[i]`. Both are built in
                // ascending lane order, so they agree — but a disagreement
                // would put one page's token into another page's lane and
                // read as fluent text about the wrong book, so it is checked
                // rather than trusted.
                if plan.slots.len() != step_lanes.len()
                    || plan
                        .slots
                        .iter()
                        .zip(step_lanes.iter())
                        .any(|(planned, &lane)| planned.slot != lane)
                {
                    return Err(format!(
                        "the planned step covers lanes {:?} but the batch is for {step_lanes:?}",
                        plan.slots.iter().map(|step| step.slot).collect::<Vec<_>>()
                    ));
                }
                let t_step = Instant::now();
                let rows = session
                    .step_slots(&plan, &step_tokens)
                    .map_err(|e| format!("decode step: {e:?}"))?;
                // The step's cost is shared, so it is shared out: an equal
                // slice each, which makes the per-page decode times sum back
                // to the wall clock the batch actually spent.
                let share = t_step.elapsed().as_secs_f64() / step_lanes.len() as f64;

                let mut looped: Vec<usize> = Vec::new();
                for (row, (&lane, &token)) in rows
                    .into_iter()
                    .zip(step_lanes.iter().zip(step_tokens.iter()))
                {
                    table
                        .advance(lane, 1)
                        .map_err(|e| format!("advance lane {lane}: {e:?}"))?;
                    let Some(job) = jobs[lane].as_mut() else {
                        continue;
                    };
                    job.logits = row;
                    job.generated += 1;
                    job.page.decode_s += share;
                    if let Some(chunk) = job.decoder.push_token(session.vocab(), token) {
                        job.text.push_str(&chunk);
                    }
                    if job.generated % LOOP_CHECK_EVERY == 0 && detect_repeat(&job.text, 0) {
                        looped.push(lane);
                    }
                }
                for lane in looped {
                    close_lane(&mut jobs, &mut table, lane, true, &mut finished, events);
                }
            }
        }

        /// End a lane's attempt and either free the lane or hold it for a
        /// hotter re-seat. Either way the lane leaves the decoding phase, so
        /// the next step is planned without it.
        fn close_lane(
            jobs: &mut [Option<LaneJob>],
            table: &mut SlotTable,
            lane: usize,
            looped: bool,
            finished: &mut usize,
            events: &mpsc::Sender<BatchEvent>,
        ) {
            let Some(job) = jobs[lane].as_mut() else {
                return;
            };
            match end_attempt(job, looped) {
                LaneEnd::Reseat => {
                    let _ = table.park(lane);
                }
                LaneEnd::Done => {
                    let index = job.index;
                    let page = job.page.clone();
                    jobs[lane] = None;
                    let _ = table.retire(lane);
                    *finished += 1;
                    let _ = events.send(BatchEvent::Page(index, Ok(page)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Domain, Registry};

    fn params(prompt: &str, input: Vec<u8>) -> GenerateParams {
        let request = crate::protocol::GenerateRequestJson {
            model: "chandra-ocr-2".to_string(),
            domain: Some("ocr".to_string()),
            prompt: Some(prompt.to_string()),
            input_b64: Some(
                String::from_utf8(makepad_base64::base64_encode(
                    &input,
                    &makepad_base64::BASE64_STANDARD,
                ))
                .expect("base64 is ascii"),
            ),
            ..Default::default()
        };
        GenerateParams::from_request(&request).expect("params")
    }

    #[test]
    fn registry_carries_a_servable_ocr_entry() {
        let registry = Registry::embedded().unwrap();
        let ocr = registry.find("chandra-ocr-2").expect("ocr entry");
        assert_eq!(ocr.domain, Domain::Ocr);
        assert_eq!(ocr.backend, "ocr");
        assert!(ocr.available && !ocr.gated);
        assert_eq!(ocr.files.len(), 2);
        let gguf = ocr.file_by_role("llm-gguf").unwrap();
        assert_eq!(gguf.repo, "prithivMLmods/chandra-ocr-2-GGUF");
        assert_eq!(gguf.path, "chandra-ocr-2.Q8_0.gguf");
        assert_eq!(gguf.size, Some(5_157_833_312));
        let mmproj = ocr.file_by_role("mmproj").unwrap();
        // The tower must be true F16: the prithivMLmods "mmproj-f16" is BF16
        // inside, which the Metal tower cannot multiply.
        assert_eq!(mmproj.repo, "mradermacher/chandra-ocr-2-GGUF");
        assert_eq!(mmproj.path, "chandra-ocr-2.mmproj-f16.gguf");
        assert_eq!(mmproj.size, Some(672_423_200));
        spec_is_servable(ocr).unwrap();
        let vision = registry.find("qwen3.8-27b-vision").unwrap();
        assert!(spec_is_servable(vision).unwrap_err().contains("not in the ocr domain"));
    }

    #[test]
    fn ocr_domain_round_trips_through_the_wire_name() {
        assert_eq!(Domain::parse("ocr"), Some(Domain::Ocr));
        assert_eq!(Domain::Ocr.as_str(), "ocr");
    }

    #[test]
    fn the_prompts_are_chandras_verbatim() {
        let text = ocr_prompt();
        assert!(text.starts_with("OCR this image to HTML.\n\nOnly use these tags ['math', 'br', 'i',"));
        assert!(text.contains("attributes ['class', 'colspan',"));
        assert!(text.ends_with("Reading order should be correct and natural."));
        let layout = ocr_layout_prompt();
        assert!(layout.starts_with("OCR this image to HTML, arranged as layout blocks."));
        assert!(layout.contains("- Blank-Page\n\nOnly use these tags"));
        assert_eq!(OcrPrompt::from_wire(""), OcrPrompt::Text);
        assert_eq!(OcrPrompt::from_wire(" layout "), OcrPrompt::Layout);
        assert_eq!(OcrPrompt::from_wire("read it"), OcrPrompt::Custom("read it".into()));
        let suffix = ocr_suffix("x");
        assert!(suffix.starts_with("<|vision_end|>x<|im_end|>\n"));
        assert!(suffix.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn scale_to_fit_matches_the_reference_rule() {
        // Inside both bounds: only the 28 px block rounding.
        assert_eq!(scale_to_fit(2062, 2771), (2072, 2772));
        // A 33 MP manuscript scan comes down under 3072x2048 worth of area.
        let (w, h) = scale_to_fit(5227, 6445);
        assert!(w * h <= MAX_FIT.0 * MAX_FIT.1, "{w}x{h}");
        assert_eq!(w % FIT_GRID, 0);
        assert_eq!(h % FIT_GRID, 0);
        let ar = w as f64 / h as f64;
        assert!((ar - 5227.0 / 6445.0).abs() < 0.02, "{ar}");
        // A tiny image is raised towards the minimum area (the block
        // rounding can land just under it, as the reference does).
        let (w, h) = scale_to_fit(100, 140);
        assert!(w * h > 100 * 140 * 3, "{w}x{h}");
        assert!((w * h) as f64 >= 0.9 * (MIN_FIT.0 * MIN_FIT.1) as f64, "{w}x{h}");
        assert_eq!(scale_to_fit(0, 10), (0, 0));
    }

    #[test]
    fn page_fit_raises_small_pages_and_lands_on_the_token_grid() {
        // 1000x1678 (a Pandora page): short side to 1536 first, then the fit.
        let (w, h) = page_fit(1000, 1678, 32);
        assert_eq!(w % 32, 0);
        assert_eq!(h % 32, 0);
        assert!(w >= 1536 - 32, "{w}");
        assert!(w * h <= MAX_FIT.0 * MAX_FIT.1 + 2 * 32 * 3072, "{w}x{h}");
        // 2062x2771 stays itself, rounded to 32.
        assert_eq!(page_fit(2062, 2771, 32), (2080, 2784));
        assert_eq!(page_fit(0, 5, 32), (0, 0));
    }

    #[test]
    fn resample_shrinks_by_area_and_grows_bilinearly() {
        // 4x2 black|white halves -> 2x1: averages within each half.
        let src = vec![0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255];
        assert_eq!(resample_rgb8(&src, 4, 2, 2, 1), vec![0, 0, 0, 255, 255, 255]);
        // 2x1 black|white -> 4x1: bilinear ramp with the endpoints kept.
        let out = resample_rgb8(&[0, 0, 0, 255, 255, 255], 2, 1, 4, 1);
        assert_eq!(out[0], 0);
        assert_eq!(out[9], 255);
        assert!(out[3] > 0 && out[3] < out[6] && out[6] < 255);
        assert_eq!(resample_rgb8(&src, 4, 2, 4, 2), src);
    }

    #[test]
    fn the_repeat_detector_flags_loops_and_leaves_prose_alone() {
        let prose = "<p>Es sindt alle vnnd jede mineralia in genere beschaffen, auß einer jrrdischen wässerigen Substanz.</p>";
        assert!(!detect_repeat(prose, 0));
        assert!(!looks_looped(prose));
        // One short token repeated far past its budget.
        let loop_short = format!("{prose}{}", "ab".repeat(40));
        assert!(detect_repeat(&loop_short, 0));
        // A longer sequence: budget is 4 * (1 + 3/len), so 6 repeats of a
        // 20-char run trips it.
        let loop_long = format!("{prose}{}", "<p>Wasser. Feuer.</p>".repeat(6));
        assert!(detect_repeat(&loop_long, 0));
        // A longer loop ending in a tidy tail is caught by the cut variant
        // (the cut lands mid-unit, so the run must survive losing 50 chars).
        let tailed = format!("{prose}{}{}", "<p>Wasser. Feuer.</p>".repeat(9), " ".repeat(10));
        assert!(!detect_repeat(&tailed, 0));
        assert!(looks_looped(&tailed));
        // Four repeats of a long run is still within budget.
        let fine = format!("{prose}{}", "<p>Wasser. Feuer.</p>".repeat(4));
        assert!(!detect_repeat(&fine, 0));
        assert!(!detect_repeat("", 0));
        assert!(!detect_repeat("abc", 50));
    }

    #[test]
    fn lanes_divide_the_page_budget_they_do_not_multiply_it() {
        // The attention arena is `lanes * per-lane`, so per-lane must DIVIDE.
        // Getting this backwards asks a box for twice the KV it was sized for
        // and fails at load, with the weights already resident.
        assert_eq!(context_for_lanes(MAX_CONTEXT, 1), MAX_CONTEXT);
        assert_eq!(context_for_lanes(MAX_CONTEXT, 2), MAX_CONTEXT / 2);
        assert_eq!(context_for_lanes(MAX_CONTEXT, 4), MAX_CONTEXT / 4);
        for lanes in 1..=8 {
            let per = context_for_lanes(MAX_CONTEXT, lanes);
            assert!(
                per * lanes as u32 <= MAX_CONTEXT,
                "{lanes} lanes x {per} overruns the {MAX_CONTEXT} budget"
            );
        }
        assert_eq!(context_for_lanes(MAX_CONTEXT, 0), MAX_CONTEXT, "zero lanes reads as one");
        assert!(context_for_lanes(4, 8) >= 1, "never a zero-length context");
    }

    #[test]
    fn two_lanes_still_hold_a_page_and_an_answer() {
        // The claim the two-lane run rests on: at 20480 total, two lanes of
        // 10240 fit the largest page this backend will feed (6144 image
        // tokens) plus its prompt plus a real answer. If this stops holding,
        // two lanes stop being an honest default rather than silently
        // truncating transcripts.
        let per_lane = context_for_lanes(MAX_CONTEXT, 2) as usize;
        let largest_page = 6144 + 512;
        let room = per_lane - largest_page;
        assert!(
            room >= 3_500,
            "a lane of {per_lane} leaves only {room} tokens for the answer"
        );
        assert!(room >= MIN_OUTPUT_ROOM);
    }

    #[test]
    fn a_request_without_a_page_is_refused() {
        let err = validate_ocr_params(&params("", Vec::new())).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)), "{err}");
        assert!(err.to_string().contains("input_b64 is required"), "{err}");
        // A prompt is optional: empty means the plain transcription.
        let png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        validate_ocr_params(&params("", png.clone())).unwrap();
        let err = validate_ocr_params(&params("", b"PK\x03\x04zip".to_vec())).unwrap_err();
        assert!(matches!(&err, AssetAiError::Params(m) if m.starts_with("ocr:")), "{err}");
        let mut big = png;
        big.resize(MAX_INPUT_BYTES + 1, 0);
        assert!(validate_ocr_params(&params("", big)).unwrap_err().to_string().contains("byte limit"));
        assert!(check_page_dimensions(5227, 6445).is_ok());
        assert!(check_page_dimensions(0, 1).is_err());
        assert!(check_page_dimensions(MAX_INPUT_PIXELS, 2).is_err());
    }
}
