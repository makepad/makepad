//! CONVERT ONE CLIP for flow-warp playback: decode it, measure the motion
//! between consecutive frames, re-encode it ALL-INTRA, and append the `mkfl`
//! box. The result is one ordinary mp4 that any player plays and a
//! flow-aware player scratches at any rate, in either direction.
//!
//! Three decisions live here, and each one is the reason a step exists:
//!
//! - **All-intra.** Bounce playback runs frames BACKWARDS. On a GOP file
//!   every reverse step is a walk from the preceding key frame; on an
//!   all-intra file every frame decodes on its own. It costs bitrate and it
//!   is not optional.
//! - **Source cadence, source rate.** Nothing is interpolated and no frames
//!   are added: the flow field is what lets the player synthesize any
//!   in-between it likes, so baking extra frames would only make the file
//!   bigger and the endpoint cache smaller.
//! - **Fit the player's cache.** Flow-warp keeps every pair endpoint decoded
//!   in memory; past that budget the clip plays as plain video and the whole
//!   conversion was for nothing. So when the clip would not fit, the
//!   converter scales it down until it does — by an integer area average, and
//!   it SAYS SO in the report rather than quietly shipping something else.
//!
//! The audio track is dropped, exactly as the enhance stage drops it: the
//! flow-warp player decodes video only, so a converted clip is silent under
//! the warp no matter what the container carried.

use crate::estimate::{flow_pair, FlowParams, FramePyramid};
use crate::payload::{encode_flow_payload, mkfl_box_bytes, quantize_flow_grid};
use makepad_video::{
    nv12, VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions,
};
use std::io::Write;
use std::path::Path;

/// The flow-warp endpoint cache budget in the VJ player
/// (`apps/vj/src/flow_warp.rs`). Kept here as the converter's default target
/// so a clip is produced at a size that actually warps.
pub const DEFAULT_FIT_CACHE_BYTES: usize = 640 * 1024 * 1024;

/// Largest mp4 the player will lift into memory to scan for the box.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ConvertOptions {
    pub codec: VideoFileCodec,
    /// Fixed video bitrate; `None` derives one from the output pixel rate.
    pub bitrate_bps: Option<u32>,
    /// Endpoint-cache budget to fit by downscaling. 0 = never downscale.
    pub fit_cache_bytes: usize,
    /// Largest integer downscale the fit is allowed to reach.
    pub max_scale: usize,
    /// Refuse anything longer than this many frames rather than grinding for
    /// an hour on a feature film somebody dropped in by accident.
    pub max_frames: usize,
    pub flow: FlowParams,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            // H.264 for the same reason the enhance stage defaults to it:
            // every decoder in the building reads it, hardware included.
            codec: VideoFileCodec::H264,
            bitrate_bps: None,
            fit_cache_bytes: DEFAULT_FIT_CACHE_BYTES,
            max_scale: 4,
            max_frames: 20_000,
            flow: FlowParams::default(),
        }
    }
}

/// What one conversion did — everything a caller needs to tell the user the
/// truth about the clip they now have.
#[derive(Clone, Debug, PartialEq)]
pub struct ConvertReport {
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
    /// Integer downscale applied to fit the player's cache (1 = none).
    pub scale: usize,
    pub frames: usize,
    pub pairs: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub bytes: u64,
    /// Will the player actually be able to warp this clip, or is it over one
    /// of its budgets and destined to play as plain video?
    pub warps: bool,
    /// When `warps` is false, why.
    pub warp_note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConvertError {
    Cancelled,
    /// The input is something this converter will not take.
    Unsupported(String),
    /// The platform codec seam failed.
    Video(String),
    Io(String),
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::Cancelled => write!(f, "cancelled"),
            ConvertError::Unsupported(why) => write!(f, "{why}"),
            ConvertError::Video(why) => write!(f, "{why}"),
            ConvertError::Io(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ConvertError {}

/// Progress for one clip: `fraction` is 0..1 over the whole conversion.
#[derive(Clone, Copy, Debug)]
pub struct ConvertProgress {
    pub frames: usize,
    pub expected: usize,
    pub fraction: f64,
}

/// Even, non-zero, and never below the floor a codec can encode.
fn even_floor(v: usize) -> usize {
    (v / 2) * 2
}

/// The output geometry: the largest integer downscale that keeps the decoded
/// endpoint frames inside the player's cache budget.
pub fn choose_output_size(
    w: usize,
    h: usize,
    expected_frames: usize,
    opts: &ConvertOptions,
) -> (usize, usize, usize) {
    let frames = expected_frames.max(1);
    let mut last = (even_floor(w).max(2), even_floor(h).max(2), 1usize);
    for k in 1..=opts.max_scale.max(1) {
        let ow = even_floor(w / k);
        let oh = even_floor(h / k);
        if ow < 16 || oh < 16 {
            break;
        }
        last = (ow, oh, k);
        if opts.fit_cache_bytes == 0 {
            break;
        }
        let need = frames.saturating_mul(ow * oh * 4);
        if need <= opts.fit_cache_bytes {
            break;
        }
    }
    last
}

/// Area average of a tightly packed single-channel plane.
fn resample_plane(src: &[u8], w: usize, h: usize, ow: usize, oh: usize) -> Vec<u8> {
    let mut out = vec![0u8; ow * oh];
    let step_x = w as f32 / ow as f32;
    let step_y = h as f32 / oh as f32;
    for y in 0..oh {
        let y0 = (y as f32 * step_y) as usize;
        let y1 = (((y + 1) as f32 * step_y) as usize).clamp(y0 + 1, h);
        for x in 0..ow {
            let x0 = (x as f32 * step_x) as usize;
            let x1 = (((x + 1) as f32 * step_x) as usize).clamp(x0 + 1, w);
            let mut sum = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    sum += src[sy * w + sx] as u32;
                }
            }
            out[y * ow + x] = (sum / ((x1 - x0) * (y1 - y0)) as u32) as u8;
        }
    }
    out
}

/// Area average of a tightly packed RGB8 frame.
fn resample_rgb(src: &[u8], w: usize, h: usize, ow: usize, oh: usize) -> Vec<u8> {
    let mut out = vec![0u8; ow * oh * 3];
    let step_x = w as f32 / ow as f32;
    let step_y = h as f32 / oh as f32;
    for y in 0..oh {
        let y0 = (y as f32 * step_y) as usize;
        let y1 = (((y + 1) as f32 * step_y) as usize).clamp(y0 + 1, h);
        for x in 0..ow {
            let x0 = (x as f32 * step_x) as usize;
            let x1 = (((x + 1) as f32 * step_x) as usize).clamp(x0 + 1, w);
            let count = ((x1 - x0) * (y1 - y0)) as u32;
            for c in 0..3 {
                let mut sum = 0u32;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        sum += src[(sy * w + sx) * 3 + c] as u32;
                    }
                }
                out[(y * ow + x) * 3 + c] = (sum / count) as u8;
            }
        }
    }
    out
}

/// Bits per output pixel for the all-intra encode. All-intra needs roughly
/// three times an inter GOP for the same picture, and a VJ clip is watched on
/// a wall, so this sits at the generous end.
const INTRA_BITS_PER_PIXEL: f64 = 0.30;

fn derive_bitrate(w: usize, h: usize, fps: f64) -> u32 {
    let bits = w as f64 * h as f64 * INTRA_BITS_PER_PIXEL * fps.max(1.0);
    bits.clamp(8_000_000.0, 80_000_000.0) as u32
}

/// Convert `input` into a flow-carrying all-intra clip at `output`.
///
/// `progress` is called a few times a second, `cancel` on every frame. A
/// cancelled or failed conversion leaves NO output file behind — a partial
/// mp4 in a library is worse than no clip at all.
pub fn convert_video(
    input: &Path,
    output: &Path,
    opts: &ConvertOptions,
    progress: &mut dyn FnMut(ConvertProgress),
    cancel: &dyn Fn() -> bool,
) -> Result<ConvertReport, ConvertError> {
    let result = convert_inner(input, output, opts, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

fn convert_inner(
    input: &Path,
    output: &Path,
    opts: &ConvertOptions,
    progress: &mut dyn FnMut(ConvertProgress),
    cancel: &dyn Fn() -> bool,
) -> Result<ConvertReport, ConvertError> {
    let in_path = input
        .to_str()
        .ok_or_else(|| ConvertError::Io(format!("non-utf8 input path: {}", input.display())))?;
    let out_path = output
        .to_str()
        .ok_or_else(|| ConvertError::Io(format!("non-utf8 output path: {}", output.display())))?;
    let mut decoder =
        VideoFileDecoder::open(in_path).map_err(|e| ConvertError::Video(format!("decode open: {e}")))?;
    let info = decoder.info().clone();
    let (src_w, src_h) = (info.width as usize, info.height as usize);
    if src_w < 16 || src_h < 16 {
        return Err(ConvertError::Unsupported(format!(
            "{src_w}x{src_h} is too small to convert"
        )));
    }
    let fps_num = info.fps_num.max(1);
    let fps_den = info.fps_den.max(1);
    let fps = fps_num as f64 / fps_den as f64;
    let duration_secs = info.duration_100ns.max(0) as f64 / 10_000_000.0;
    let expected = ((duration_secs * fps).round() as usize).max(1);
    if expected > opts.max_frames {
        return Err(ConvertError::Unsupported(format!(
            "{expected} frames is longer than the {} frame conversion limit",
            opts.max_frames
        )));
    }
    let (out_w, out_h, scale) = choose_output_size(src_w, src_h, expected, opts);
    let (grid_w, grid_h) = ((out_w / 4).max(1), (out_h / 4).max(1));
    if grid_w > u16::MAX as usize || grid_h > u16::MAX as usize || out_w > u16::MAX as usize {
        return Err(ConvertError::Unsupported(format!(
            "{out_w}x{out_h} does not fit the payload's 16-bit geometry"
        )));
    }
    let bitrate = opts
        .bitrate_bps
        .unwrap_or_else(|| derive_bitrate(out_w, out_h, fps));
    let mut encoder = VideoFileEncoder::new(
        out_path,
        VideoFileEncoderOptions {
            codec: opts.codec,
            width: out_w as u32,
            height: out_h as u32,
            fps_num,
            fps_den,
            video_bitrate_bps: bitrate,
            audio: None,
            // Bounce loops play BACKWARDS: every frame must decode on its
            // own, never by forward-walking a GOP.
            keyframe_only: true,
        },
    )
    .map_err(|e| ConvertError::Video(format!("encode open: {e}")))?;

    let mut samples: Vec<u8> = Vec::new();
    let mut pairs: u32 = 0;
    let mut frames = 0usize;
    let mut prev: Option<FramePyramid> = None;
    let mut rgb = Vec::new();
    let passthrough = scale == 1 && out_w == src_w && out_h == src_h;
    let mut last_report = std::time::Instant::now();
    progress(ConvertProgress { frames: 0, expected, fraction: 0.0 });

    loop {
        if cancel() {
            return Err(ConvertError::Cancelled);
        }
        let Some(frame) = decoder
            .next_frame()
            .map_err(|e| ConvertError::Video(format!("decode: {e}")))?
        else {
            break;
        };
        if frame.width as usize != src_w || frame.height as usize != src_h {
            return Err(ConvertError::Video(format!(
                "frame {frames} is {}x{}, stream is {src_w}x{src_h}",
                frame.width, frame.height
            )));
        }
        if frames >= opts.max_frames {
            return Err(ConvertError::Unsupported(format!(
                "clip is longer than the {} frame conversion limit",
                opts.max_frames
            )));
        }
        // The Y plane IS the luma the estimator wants, so the fast path never
        // leaves YUV at all.
        let luma = if passthrough {
            frame.nv12[..src_w * src_h].to_vec()
        } else {
            resample_plane(&frame.nv12[..src_w * src_h], src_w, src_h, out_w, out_h)
        };
        if passthrough {
            encoder
                .push_frame_nv12(&frame.nv12, None)
                .map_err(|e| ConvertError::Video(format!("encode: {e}")))?;
        } else {
            nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb);
            let scaled = resample_rgb(&rgb, src_w, src_h, out_w, out_h);
            encoder
                .push_frame_rgb8(&scaled, None)
                .map_err(|e| ConvertError::Video(format!("encode: {e}")))?;
        }
        let pyramid =
            FramePyramid::from_luma(&luma, out_w, out_h, grid_w, grid_h, opts.flow.max_levels);
        if let Some(prev) = &prev {
            let pair = flow_pair(prev, &pyramid, &opts.flow);
            samples.extend_from_slice(&quantize_flow_grid(
                &pair.f0,
                &pair.f1,
                &pair.mask,
                grid_w,
                grid_h,
            ));
            pairs += 1;
        }
        prev = Some(pyramid);
        frames += 1;
        if last_report.elapsed().as_millis() >= 150 {
            last_report = std::time::Instant::now();
            progress(ConvertProgress {
                frames,
                expected,
                fraction: (frames as f64 / expected as f64).min(0.98),
            });
        }
    }
    if frames < 2 {
        return Err(ConvertError::Unsupported(format!(
            "decoded {frames} frame(s): a motion field needs at least two"
        )));
    }
    encoder
        .finish()
        .map_err(|e| ConvertError::Video(format!("encode finish: {e}")))?;

    let payload = encode_flow_payload(
        pairs,
        grid_w as u16,
        grid_h as u16,
        out_w as u16,
        out_h as u16,
        fps_num,
        fps_den,
        &samples,
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(output)
        .map_err(|e| ConvertError::Io(format!("reopen output: {e}")))?;
    file.write_all(&mkfl_box_bytes(&payload))
        .map_err(|e| ConvertError::Io(format!("append mkfl box: {e}")))?;
    file.flush().map_err(|e| ConvertError::Io(format!("flush output: {e}")))?;
    drop(file);
    let bytes = std::fs::metadata(output)
        .map_err(|e| ConvertError::Io(format!("stat output: {e}")))?
        .len();
    progress(ConvertProgress { frames, expected, fraction: 1.0 });

    // Say plainly whether the thing we just made will actually warp.
    let cache_need = frames.saturating_mul(out_w * out_h * 4);
    let (warps, warp_note) = if opts.fit_cache_bytes > 0 && cache_need > opts.fit_cache_bytes {
        (
            false,
            format!(
                "{frames} frames at {out_w}x{out_h} need {} MB of endpoint cache (budget {} MB); \
                 the clip plays as plain video",
                cache_need / (1024 * 1024),
                opts.fit_cache_bytes / (1024 * 1024)
            ),
        )
    } else if bytes > DEFAULT_MAX_OUTPUT_BYTES {
        (
            false,
            format!(
                "the converted file is {} MB, over the {} MB the player will scan; \
                 the clip plays as plain video",
                bytes / (1024 * 1024),
                DEFAULT_MAX_OUTPUT_BYTES / (1024 * 1024)
            ),
        )
    } else {
        (true, String::new())
    };
    Ok(ConvertReport {
        source_width: src_w as u32,
        source_height: src_h as u32,
        width: out_w as u32,
        height: out_h as u32,
        scale,
        frames,
        pairs,
        fps_num,
        fps_den,
        bytes,
        warps,
        warp_note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fit_downscales_only_as_far_as_it_must() {
        let opts = ConvertOptions::default();
        // A 3 second 640x360 clip fits at full size.
        assert_eq!(choose_output_size(640, 360, 90, &opts), (640, 360, 1));
        // 300 frames of 1920x1080 need 2.4 GB of endpoints: halve, and halve
        // again if that still does not fit.
        let (w, h, k) = choose_output_size(1920, 1080, 300, &opts);
        assert_eq!(k, 2);
        assert_eq!((w, h), (960, 540));
        assert!(300 * w * h * 4 <= opts.fit_cache_bytes);
        // Odd dimensions always come back even.
        let (w, h, _) = choose_output_size(641, 361, 10, &opts);
        assert_eq!((w % 2, h % 2), (0, 0));
        // Fitting off leaves the size alone no matter how long the clip is.
        let no_fit = ConvertOptions { fit_cache_bytes: 0, ..ConvertOptions::default() };
        assert_eq!(choose_output_size(1920, 1080, 100_000, &no_fit), (1920, 1080, 1));
    }

    #[test]
    fn the_downscale_stops_before_it_destroys_the_picture() {
        let opts = ConvertOptions { fit_cache_bytes: 1024, ..ConvertOptions::default() };
        // Nothing fits 1 KB; the fit gives up at max_scale rather than
        // shrinking to a postage stamp.
        let (w, h, k) = choose_output_size(1920, 1080, 10_000, &opts);
        assert_eq!(k, 4);
        assert_eq!((w, h), (480, 270));
    }

    #[test]
    fn the_area_resamplers_average_their_quads() {
        // 2x2 of 0/100 -> 50, on both the plane and the RGB path.
        let plane = [0u8, 100, 100, 0];
        assert_eq!(resample_plane(&plane, 2, 2, 1, 1), vec![50]);
        let rgb = [0u8, 0, 0, 100, 100, 100, 100, 100, 100, 0, 0, 0];
        assert_eq!(resample_rgb(&rgb, 2, 2, 1, 1), vec![50, 50, 50]);
    }

    #[test]
    fn the_bitrate_tracks_the_pixel_rate_inside_its_clamps() {
        assert_eq!(derive_bitrate(320, 240, 30.0), 8_000_000);
        assert!(derive_bitrate(1920, 1080, 30.0) > 8_000_000);
        assert_eq!(derive_bitrate(7680, 4320, 60.0), 80_000_000);
    }
}
