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

/// The depth-1 lookahead scheduler behind `OcrBackend::ocr_pages`: submits
/// each item in order via `submit`, keeping at most `lookahead + 1` items
/// in flight (submitted but not yet collected) at a time, then drains
/// `collect` for each item — in submission order, once each — as capacity
/// allows. With `lookahead >= 1`, item i+1 is always submitted BEFORE item
/// i is collected, which is what lets a page's tower work (fit + encode)
/// overlap the PREVIOUS page's session work (prefill + decode): `submit`
/// hands work to a background thread and returns immediately with a
/// receipt, `collect` blocks on that receipt until the result is ready.
///
/// This function is pure queueing logic — it does not know or care what
/// `submit`/`collect` actually do — deliberately kept free of the `llm`
/// feature gate and every OCR/GPU type, so its ordering can be exercised
/// directly by the tests below with no worker, tower or GPU involved.
fn pipeline_submit_collect<T, R>(
    lookahead: usize,
    items: impl IntoIterator<Item = T>,
    mut submit: impl FnMut(usize, T) -> R,
    mut collect: impl FnMut(usize, R),
) {
    let mut pending: std::collections::VecDeque<(usize, R)> = std::collections::VecDeque::new();
    for (index, item) in items.into_iter().enumerate() {
        let receipt = submit(index, item);
        pending.push_back((index, receipt));
        if pending.len() > lookahead {
            let (index, receipt) = pending.pop_front().expect("just pushed one");
            collect(index, receipt);
        }
    }
    while let Some((index, receipt)) = pending.pop_front() {
        collect(index, receipt);
    }
}

#[cfg(feature = "llm")]
pub use resident::OcrBackend;

#[cfg(feature = "llm")]
mod resident {
    use super::*;
    use std::path::PathBuf;

    /// The resident OCR model: a tower thread and a session thread, kept
    /// warm across pages and pipelined (see `worker::OcrWorker`) so a
    /// batch's wall time per page approaches max(encode, prefill+decode)
    /// instead of their sum. One backend object per model id; `unload`
    /// drops the weights.
    pub struct OcrBackend {
        model_id: String,
        worker: Option<worker::OcrWorker>,
        loaded: Option<(PathBuf, PathBuf)>,
    }

    impl OcrBackend {
        pub fn new(model_id: &str) -> Self {
            Self {
                model_id: model_id.to_string(),
                worker: None,
                loaded: None,
            }
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
            let want = (gguf.clone(), mmproj.clone());
            if self.worker.is_some() && self.loaded.as_ref() == Some(&want) {
                return Ok(());
            }
            self.worker = None;
            self.loaded = None;
            let worker = worker::OcrWorker::spawn(gguf, mmproj, self.model_id.clone(), progress)
                .map_err(|e| AssetAiError::Backend(format!("ocr load: {e}")))?;
            self.worker = Some(worker);
            self.loaded = Some(want);
            Ok(())
        }

        /// Transcribe one decoded page. This is the whole request path
        /// minus the wire: the bench calls it directly. Internally this
        /// still crosses the tower thread then the session thread (see
        /// `worker::OcrWorker`), but a single call waits for both stages
        /// in order, so it is computation-for-computation identical to the
        /// pre-pipeline single-thread path — same fit, same encode, same
        /// prefill/decode/retry sequence, same sampler seeds.
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
            worker.ask(request, cancel, on_stage, on_text).map_err(map_worker_err)
        }

        /// Transcribe many pages with the tower's fit/preprocess/encode for
        /// page N+1 overlapping the session's prefill/decode for page N: a
        /// request is submitted to the tower thread one page ahead of the
        /// page whose result is being waited on, so the pipeline (see
        /// `worker::OcrWorker`) stays fed. Results reach `on_page` in
        /// submission order, each exactly once, tagged with its index in
        /// `requests`. A retry re-prefills from that page's own cached
        /// embeddings (computed once, in the tower stage) and never
        /// touches another page's encode — retries are entirely a
        /// session-thread affair, same as before.
        ///
        /// A single page (or the last page of a batch) takes the same path
        /// `ocr_page` does: nothing to overlap with, so it just waits.
        ///
        /// The submit-ahead-of-collect scheduling is `pipeline_submit_collect`
        /// (module top level, feature-independent) with one page of
        /// lookahead; its own tests exercise that ordering without any
        /// worker, tower or GPU involved.
        pub fn ocr_pages(
            &self,
            requests: impl IntoIterator<Item = OcrRequest>,
            cancel: &CancelToken,
            mut on_page: impl FnMut(usize, Result<OcrPage, AssetAiError>),
        ) -> Result<(), AssetAiError> {
            let worker = self.worker.as_ref().ok_or_else(|| {
                AssetAiError::Backend("ocr backend used before ensure_loaded".to_string())
            })?;
            pipeline_submit_collect(
                1,
                requests,
                |_index, request| worker.ask_async(request, cancel),
                |index, submitted| {
                    let result = match submitted {
                        Ok(rx) => worker::OcrWorker::collect(rx, &mut |_, _| {}, &mut |_| {}),
                        Err(e) => Err(e),
                    };
                    on_page(index, result.map_err(map_worker_err));
                },
            );
            Ok(())
        }
    }

    /// The worker's plain string errors, re-labelled into the crate's error
    /// type. Shared by `ocr_page` and `ocr_pages` so a cancellation reads
    /// the same either way.
    fn map_worker_err(e: String) -> AssetAiError {
        if e == "cancelled" {
            AssetAiError::Cancelled
        } else {
            AssetAiError::Backend(format!("ocr: {e}"))
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
            if self.worker.is_some() && self.loaded.as_ref() == Some(&(gguf.clone(), mmproj.clone()))
            {
                return Ok(());
            }
            let gb = std::fs::metadata(&gguf).map(|m| m.len() as f64 / 1e9).unwrap_or(0.0);
            (ctx.progress)(&format!("load ocr gguf ({gb:.1}GB) + tower"), 0.1);
            self.load_from_paths(gguf, mmproj, ctx.progress)?;
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

    /// The tower thread (fit + preprocess + encode) and the session thread
    /// (prefill + decode), and the depth-1 hand-off queue between them.
    ///
    /// `VisionTower` and `LlamaSession` are both `!Send`, so each is
    /// constructed and lives entirely on the thread that uses it — there is
    /// no cross-thread move of either. Before this pipeline, both lived on
    /// ONE thread (`ocr-worker`); the change here MOVES the tower to its
    /// own thread rather than loading a second one, so there is still
    /// exactly one resident tower and no extra VRAM (loading a second
    /// tower instead would cost ~0.7 GB of weights, which is not needed:
    /// nothing about a page's fit/encode depends on the session, so the
    /// existing tower simply runs on a thread of its own).
    ///
    /// Per page: the tower thread fits the page to Chandra's target size,
    /// preprocesses it and runs it through the vision tower, then hands the
    /// resulting embeddings (plus everything the session needs: prompt,
    /// token budget, retry count) to the session thread across a
    /// `mpsc::sync_channel(1)` — a bounded, depth-1 queue. "Depth-1" bounds
    /// what can sit BETWEEN the threads unconsumed to one page's embeddings
    /// (a few hundred MB at 6k image tokens on the 4090 case), not the
    /// whole batch; the tower can still be a little further ahead than
    /// that in wall-clock terms (finishing page N+2's encode while page
    /// N+1's embeddings sit in the queue and page N is still decoding) —
    /// `OcrBackend::ocr_pages` bounds submission itself to one page of
    /// lookahead, so in practice the tower is never more than one page
    /// ahead of the session.
    ///
    /// The session thread pops a hand-off, tokenizes the prompt (cheap —
    /// unlike the tower's CPU fit/preprocess, this is not worth overlapping
    /// with anything), then prefills and decodes exactly as `run_page` did
    /// before this split, including the retry loop: a retry resets the
    /// session and re-prefills from the SAME embeddings the tower handed
    /// over, entirely on the session thread — it never asks the tower for
    /// anything, so a retry cannot consume the next page's encode.
    mod worker {
        use super::*;
        use makepad_ai_llm::{
            preprocess_rgb8, GgufFile, LlamaSamplerState, LlamaSamplingParams, LlamaSession,
            LlamaSessionConfig, VisionConfig, VisionTower,
        };
        use std::sync::mpsc;
        use std::time::Instant;

        pub enum WorkerEvent {
            Stage(String, f64),
            /// Full answer text so far — prefix-stable snapshots.
            Text(String),
            Done(Result<OcrPage, String>),
        }

        enum TowerMsg {
            Ask(OcrRequest, CancelToken, mpsc::Sender<WorkerEvent>),
        }

        /// Everything the session thread needs for one page, handed over by
        /// the tower thread once fit + preprocess + encode are done. Owns
        /// the embeddings outright, so the session thread's retry loop
        /// re-prefills from them without touching the tower again.
        struct EncodedPage {
            prompt: OcrPrompt,
            max_new_tokens: u32,
            retries: u32,
            fed_width: usize,
            fed_height: usize,
            tokens_w: usize,
            tokens_h: usize,
            embeddings: Vec<f32>,
            encode_s: f64,
            cancel: CancelToken,
            events: mpsc::Sender<WorkerEvent>,
        }

        /// Handle to the OCR pipeline: submits requests to the tower
        /// thread, which hands encoded pages to the session thread across
        /// the depth-1 queue described above.
        pub struct OcrWorker {
            tower_tx: mpsc::Sender<TowerMsg>,
        }

        impl OcrWorker {
            pub fn spawn(
                gguf: PathBuf,
                mmproj: PathBuf,
                model_id: String,
                progress: &mut dyn FnMut(&str, f64),
            ) -> Result<Self, String> {
                enum BootEvt {
                    Progress(String, f64),
                    Ready(Result<(), String>),
                }
                let (tower_tx, tower_rx) = mpsc::channel::<TowerMsg>();
                // The depth-1 hand-off: the tower blocks trying to send an
                // encoded page here until the session thread has taken the
                // previous one out.
                let (hand_tx, hand_rx) = mpsc::sync_channel::<EncodedPage>(1);
                let (boot_tx, boot_rx) = mpsc::channel::<BootEvt>();

                // Session thread: loads the language session, then prefills
                // + decodes (+ retries) whatever the tower hands it, page
                // after page, in the order the tower produced them (both
                // ends of the hand-off channel are FIFO).
                let session_boot_tx = boot_tx.clone();
                let session_gguf = gguf.clone();
                std::thread::Builder::new()
                    .name("ocr-session".to_string())
                    .spawn(move || {
                        let loaded = LlamaSession::load_with_progress(
                            &session_gguf,
                            LlamaSessionConfig {
                                max_context: Some(MAX_CONTEXT),
                                // A page is 3-6k image tokens of prefill;
                                // the session default (32) is a chat turn's
                                // shape and made prefill the longest leg
                                // (4090: ~600 tok/s). 512 is what the chat
                                // lanes run.
                                prefill_batch_size: PREFILL_BATCH,
                                // 512-row prefill steps through a 4B
                                // outgrow the session default arena (512
                                // MiB) by a few MB on the largest pages;
                                // reserve room for the batch.
                                extra_activation_bytes: EXTRA_ACTIVATION_BYTES,
                                ..LlamaSessionConfig::default()
                            },
                            &mut |stage, frac| {
                                let _ = session_boot_tx
                                    .send(BootEvt::Progress(stage.to_string(), frac * 0.5));
                            },
                        )
                        .map_err(|e| format!("load llm session: {e:?}"));
                        let mut session = match loaded {
                            Ok(session) => {
                                let _ = session_boot_tx.send(BootEvt::Ready(Ok(())));
                                session
                            }
                            Err(err) => {
                                let _ = session_boot_tx.send(BootEvt::Ready(Err(err)));
                                return;
                            }
                        };
                        while let Ok(encoded) = hand_rx.recv() {
                            prefill_and_decode(&mut session, encoded);
                        }
                    })
                    .map_err(|e| format!("spawn ocr session thread: {e}"))?;

                // Tower thread: loads the vision tower, then fits +
                // preprocesses + encodes whatever is asked of it, handing
                // each result to the session thread. An ask that fails
                // before or during encode is answered directly (Done)
                // without involving the session thread at all.
                std::thread::Builder::new()
                    .name("ocr-tower".to_string())
                    .spawn(move || {
                        let loaded = (|| -> Result<(VisionTower, VisionConfig), String> {
                            let mmproj_path = mmproj.to_string_lossy().to_string();
                            let file = GgufFile::open(&mmproj_path)
                                .map_err(|e| format!("open mmproj: {e:?}"))?;
                            let config = VisionConfig::from_gguf(&file)
                                .map_err(|e| format!("mmproj vision config: {e:?}"))?;
                            let _ = boot_tx.send(BootEvt::Progress("load vision tower".into(), 0.1));
                            let tower = VisionTower::load(&mmproj_path)
                                .map_err(|e| format!("load vision tower: {e:?}"))?;
                            Ok((tower, config))
                        })();
                        let (mut tower, config) = match loaded {
                            Ok(parts) => {
                                eprintln!(
                                    "[ocr] {model_id} resident on {}: page fit up to {}x{}, max_context {} (tower thread + session thread, pipelined)",
                                    parts.0.device_description(),
                                    MAX_FIT.0,
                                    MAX_FIT.1,
                                    MAX_CONTEXT
                                );
                                let _ = boot_tx.send(BootEvt::Ready(Ok(())));
                                parts
                            }
                            Err(err) => {
                                let _ = boot_tx.send(BootEvt::Ready(Err(err)));
                                return;
                            }
                        };
                        while let Ok(TowerMsg::Ask(ask, cancel, events)) = tower_rx.recv() {
                            match fit_and_encode(&mut tower, &config, ask, &cancel, &events) {
                                Ok(encoded) => {
                                    // Blocks here until the session thread
                                    // has consumed the previous hand-off —
                                    // the depth-1 bound. If the session
                                    // thread is gone (failed load, or the
                                    // backend was dropped), stop; nothing
                                    // more can be served.
                                    if hand_tx.send(encoded).is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = events.send(WorkerEvent::Done(Err(e)));
                                }
                            }
                        }
                    })
                    .map_err(|e| format!("spawn ocr tower thread: {e}"))?;

                let mut ready = 0u32;
                loop {
                    match boot_rx.recv() {
                        Ok(BootEvt::Progress(stage, frac)) => progress(&stage, frac),
                        Ok(BootEvt::Ready(Ok(()))) => {
                            ready += 1;
                            if ready == 2 {
                                break;
                            }
                        }
                        Ok(BootEvt::Ready(Err(err))) => return Err(err),
                        Err(_) => return Err("ocr worker died during load".to_string()),
                    }
                }
                Ok(Self { tower_tx })
            }

            /// Submit one page and block until it is fully transcribed,
            /// forwarding stage/text events as they arrive. Used by the
            /// wire path and by `ocr_page`.
            pub fn ask(
                &self,
                ask: OcrRequest,
                cancel: &CancelToken,
                on_stage: &mut dyn FnMut(&str, f64),
                on_text: &mut dyn FnMut(&str),
            ) -> Result<OcrPage, String> {
                let rx = self.ask_async(ask, cancel)?;
                Self::collect(rx, on_stage, on_text)
            }

            /// Submit one page without waiting: returns as soon as the
            /// request reaches the tower thread's queue. The caller drains
            /// the returned receiver with `collect` whenever it wants the
            /// result — submitting the NEXT page first is what lets that
            /// page's fit/encode overlap this one's prefill/decode.
            pub fn ask_async(
                &self,
                ask: OcrRequest,
                cancel: &CancelToken,
            ) -> Result<mpsc::Receiver<WorkerEvent>, String> {
                let (event_tx, event_rx) = mpsc::channel();
                self.tower_tx
                    .send(TowerMsg::Ask(ask, cancel.clone(), event_tx))
                    .map_err(|_| "ocr worker thread is gone".to_string())?;
                Ok(event_rx)
            }

            /// Drain one page's event stream to its result.
            pub fn collect(
                rx: mpsc::Receiver<WorkerEvent>,
                on_stage: &mut dyn FnMut(&str, f64),
                on_text: &mut dyn FnMut(&str),
            ) -> Result<OcrPage, String> {
                loop {
                    match rx.recv() {
                        Ok(WorkerEvent::Stage(stage, frac)) => on_stage(&stage, frac),
                        Ok(WorkerEvent::Text(text)) => on_text(&text),
                        Ok(WorkerEvent::Done(result)) => return result,
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

        /// The tower-thread half of what used to be `run_page`: page fit,
        /// preprocess, and the vision-tower encode. Runs on its own thread
        /// so page N+1 can be here while page N is in `prefill_and_decode`
        /// on the session thread.
        fn fit_and_encode(
            tower: &mut VisionTower,
            config: &VisionConfig,
            ask: OcrRequest,
            cancel: &CancelToken,
            events: &mpsc::Sender<WorkerEvent>,
        ) -> Result<EncodedPage, String> {
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

            Ok(EncodedPage {
                prompt: ask.prompt,
                max_new_tokens: ask.max_new_tokens,
                retries: ask.retries,
                fed_width: fw,
                fed_height: fh,
                tokens_w: prepared.tokens_w(),
                tokens_h: prepared.tokens_h(),
                embeddings,
                encode_s,
                cancel: cancel.clone(),
                events: events.clone(),
            })
        }

        /// The session-thread half of what used to be `run_page`: tokenize
        /// the prompt, then prefill + decode, retrying (from the SAME
        /// embeddings — no re-encode) while the answer looks looped. Sends
        /// its own `Done` event; callers do not see a return value.
        fn prefill_and_decode(session: &mut LlamaSession, encoded: EncodedPage) {
            let EncodedPage {
                prompt,
                max_new_tokens,
                retries,
                fed_width,
                fed_height,
                tokens_w,
                tokens_h,
                embeddings,
                encode_s,
                cancel,
                events,
            } = encoded;
            let result = (|| -> Result<OcrPage, String> {
                let cancelled = || "cancelled".to_string();
                let stage = |name: &str, frac: f64| {
                    let _ = events.send(WorkerEvent::Stage(name.to_string(), frac));
                };
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }

                let vocab = session.vocab();
                let prefix_ids = vocab
                    .tokenize(OCR_PREFIX, false, true)
                    .map_err(|e| format!("tokenize prefix: {e:?}"))?;
                let suffix = ocr_suffix(&prompt.text());
                let suffix_ids = vocab
                    .tokenize(&suffix, false, true)
                    .map_err(|e| format!("tokenize prompt: {e:?}"))?;
                // Stop on the turn end AND the end-of-text token: Chandra's
                // generation config names only <|endoftext|>, but the model
                // ends its turn with <|im_end|>, so its reference runner
                // adds both.
                let mut stops: Vec<i32> = session.stop_tokens();
                for name in ["<|im_end|>", "<|endoftext|>"] {
                    if let Some(id) = vocab.token_id(name) {
                        if !stops.contains(&id) {
                            stops.push(id);
                        }
                    }
                }
                let image_tokens = tokens_w * tokens_h;
                let max_new = max_new_tokens.max(1) as usize;
                let needed = prefix_ids.len() + image_tokens + suffix_ids.len() + max_new;
                let max_context = session.max_context();
                let max_new = if needed > max_context {
                    // The output budget yields; the page never does.
                    let room = max_context
                        .saturating_sub(prefix_ids.len() + image_tokens + suffix_ids.len());
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
                    fed_width,
                    fed_height,
                    image_tokens,
                    encode_s,
                    ..OcrPage::default()
                };
                let attempts = 1 + retries;
                for attempt in 0..attempts {
                    if cancel.is_cancelled() {
                        return Err(cancelled());
                    }
                    page.attempts = attempt + 1;
                    // A fresh turn: clear KV + recurrent state, prefill the
                    // prefix, the page, the prompt. A retry lands here too,
                    // re-prefilling from the SAME `embeddings` this
                    // function was handed — no second encode, ever.
                    stage(&format!("prefill (attempt {})", attempt + 1), 0.1);
                    let t_prefill = Instant::now();
                    session.reset().map_err(|e| format!("reset: {e:?}"))?;
                    session
                        .append_tokens(&prefix_ids)
                        .map_err(|e| format!("prefill prefix: {e:?}"))?;
                    session
                        .append_image_embeddings(&embeddings, tokens_w, tokens_h)
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
            })();
            let _ = events.send(WorkerEvent::Done(result));
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

    // ---------------------------------------------------- pipeline ordering
    //
    // `pipeline_submit_collect` is the scheduling core of `ocr_pages` — it
    // is generic and feature-independent, so these run with no worker, no
    // tower, no session and no GPU. They cover: (1) the exact call order a
    // given lookahead produces, including the zero-lookahead (fully serial)
    // and empty/singleton edge cases; (2) that results are delivered once
    // each, in submission order, regardless of lookahead; (3) that with a
    // real bounded hand-off queue standing in for the tower->session
    // channel, submitting page N+1 before collecting page N actually
    // overlaps two threads' work in wall-clock time — the property the
    // whole restructure exists for.

    #[test]
    fn lookahead_one_submits_the_next_item_before_collecting_the_current_one() {
        // `submit` and `collect` are two separate `FnMut` closures alive at
        // once, so the shared log needs interior mutability — a `RefCell`,
        // not a plain `Vec`, which is exactly the kind of external state a
        // real submit/collect pair (channels, thread handles) also needs.
        let order = std::cell::RefCell::new(Vec::<String>::new());
        pipeline_submit_collect(
            1,
            0..4,
            |i, _| order.borrow_mut().push(format!("submit {i}")),
            |i, ()| order.borrow_mut().push(format!("collect {i}")),
        );
        assert_eq!(
            *order.borrow(),
            vec![
                "submit 0", "submit 1", "collect 0", "submit 2", "collect 1", "submit 3",
                "collect 2", "collect 3",
            ]
        );
    }

    #[test]
    fn lookahead_zero_is_fully_serial() {
        let order = std::cell::RefCell::new(Vec::<String>::new());
        pipeline_submit_collect(
            0,
            0..3,
            |i, _| order.borrow_mut().push(format!("submit {i}")),
            |i, ()| order.borrow_mut().push(format!("collect {i}")),
        );
        assert_eq!(
            *order.borrow(),
            vec!["submit 0", "collect 0", "submit 1", "collect 1", "submit 2", "collect 2"]
        );
    }

    #[test]
    fn lookahead_two_keeps_up_to_three_items_in_flight() {
        let order = std::cell::RefCell::new(Vec::<String>::new());
        pipeline_submit_collect(
            2,
            0..5,
            |i, _| order.borrow_mut().push(format!("submit {i}")),
            |i, ()| order.borrow_mut().push(format!("collect {i}")),
        );
        assert_eq!(
            *order.borrow(),
            vec![
                "submit 0", "submit 1", "submit 2", "collect 0", "submit 3", "collect 1",
                "submit 4", "collect 2", "collect 3", "collect 4",
            ]
        );
    }

    #[test]
    fn a_lookahead_past_the_item_count_never_blocks_and_still_collects_everything() {
        let mut collected: Vec<usize> = Vec::new();
        pipeline_submit_collect(10, 0..3, |_, _| (), |i, ()| collected.push(i));
        assert_eq!(collected, vec![0, 1, 2]);
    }

    #[test]
    fn empty_and_singleton_batches_do_not_hang_or_misfire() {
        let mut collected: Vec<usize> = Vec::new();
        pipeline_submit_collect(1, std::iter::empty::<()>(), |_, _| (), |i, ()| collected.push(i));
        assert!(collected.is_empty());

        let mut collected: Vec<usize> = Vec::new();
        pipeline_submit_collect(1, 0..1, |_, _| (), |i, ()| collected.push(i));
        assert_eq!(collected, vec![0]);
    }

    #[test]
    fn results_are_delivered_once_each_in_submission_order_regardless_of_lookahead() {
        // The items carry a payload (not just their index) so this also
        // proves `submit`'s return value rides through to `collect`
        // unchanged — the same contract `ocr_pages` relies on to carry an
        // `OcrRequest`'s eventual `OcrPage` result back to the right slot.
        for lookahead in [0usize, 1, 3, 8] {
            let items: Vec<String> = (0..7).map(|i| format!("page-{i}")).collect();
            let mut collected: Vec<(usize, String)> = Vec::new();
            pipeline_submit_collect(
                lookahead,
                items.clone(),
                |_, item| item.to_uppercase(),
                |i, payload| collected.push((i, payload)),
            );
            let expected: Vec<(usize, String)> =
                items.iter().enumerate().map(|(i, s)| (i, s.to_uppercase())).collect();
            assert_eq!(collected, expected, "lookahead={lookahead}");
        }
    }

    /// A stand-in for the real tower->session architecture: two threads, a
    /// bounded (depth-1) hand-off channel between them exactly like
    /// `worker::OcrWorker` wires the tower thread to the session thread,
    /// and `pipeline_submit_collect` driving submission from the calling
    /// thread exactly like `OcrBackend::ocr_pages` does. No GPU, no
    /// `VisionTower`, no `LlamaSession` — "encode" and "decode" are sleeps,
    /// timestamped so the test can prove real overlap happened, not just
    /// that the call order was right (the tests above already cover that).
    #[test]
    fn a_bounded_two_thread_pipeline_overlaps_encode_with_the_previous_pages_decode() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        const PAGES: usize = 5;
        const ENCODE: Duration = Duration::from_millis(30);
        const DECODE: Duration = Duration::from_millis(60);

        // "tower" thread: pops a page number, sleeps for ENCODE (the
        // fit+preprocess+encode stand-in), then hands it to the "session"
        // thread across a depth-1 bounded channel — blocking there exactly
        // as the real tower thread blocks on `hand_tx.send`.
        let (tower_tx, tower_rx) = mpsc::channel::<usize>();
        let (hand_tx, hand_rx) = mpsc::sync_channel::<usize>(1);
        let tower = std::thread::spawn(move || {
            while let Ok(page) = tower_rx.recv() {
                std::thread::sleep(ENCODE);
                if hand_tx.send(page).is_err() {
                    break;
                }
            }
        });

        // "session" thread: pops a handed-off page, sleeps for DECODE, and
        // replies on that page's own reply channel — one channel per page,
        // same shape as `WorkerEvent::Done` per request.
        let (reply_tx, reply_rx) = mpsc::channel::<(usize, mpsc::Sender<Instant>)>();
        let session = std::thread::spawn(move || {
            while let Ok(page) = hand_rx.recv() {
                std::thread::sleep(DECODE);
                // Look up (there is at most one outstanding) reply target.
                if let Ok((expect_page, reply)) = reply_rx.try_recv() {
                    assert_eq!(expect_page, page, "session must process pages in FIFO order");
                    let _ = reply.send(Instant::now());
                }
            }
        });

        let t_start = Instant::now();
        let mut done_at: Vec<Duration> = Vec::new();
        pipeline_submit_collect(
            1,
            0..PAGES,
            |_, page| {
                let (page_reply_tx, page_reply_rx) = mpsc::channel();
                reply_tx.send((page, page_reply_tx)).expect("session thread alive");
                tower_tx.send(page).expect("tower thread alive");
                page_reply_rx
            },
            |_, page_reply_rx: mpsc::Receiver<Instant>| {
                let at = page_reply_rx.recv().expect("session replies");
                done_at.push(at.duration_since(t_start));
            },
        );
        drop(tower_tx);
        tower.join().unwrap();
        session.join().unwrap();

        assert_eq!(done_at.len(), PAGES);
        // Serial (no overlap) would be PAGES * (ENCODE + DECODE). With one
        // page of lookahead, page 0 pays ENCODE once up front and every
        // page after that is gated only by DECODE (the longer stage),
        // since its ENCODE already ran during the previous page's DECODE:
        // total ~= ENCODE + PAGES * DECODE. Assert comfortably under the
        // serial total and reasonably close to the pipelined estimate —
        // wide margins keep this from flaking under CI scheduling jitter.
        let serial_total = (ENCODE + DECODE) * PAGES as u32;
        let pipelined_estimate = ENCODE + DECODE * PAGES as u32;
        let wall = *done_at.last().unwrap();
        assert!(
            wall < serial_total,
            "wall {wall:?} did not beat serial estimate {serial_total:?} — no overlap happened"
        );
        assert!(
            wall < pipelined_estimate + Duration::from_millis(150),
            "wall {wall:?} far exceeds the pipelined estimate {pipelined_estimate:?}"
        );
        // Every page's own decode-only spacing should be close to DECODE,
        // not ENCODE + DECODE — direct evidence the encode was hidden.
        for pair in done_at.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                gap < ENCODE + DECODE,
                "consecutive pages {gap:?} apart — encode was not hidden behind decode"
            );
        }
    }
}
