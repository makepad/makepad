//! The `video-enhance` backend: ENHANCE domain — video in, video out, with
//! one decode and one encode no matter how many transforms run in between:
//!
//! - `upscale: 2|4` — RealESRGAN x4plus per frame (`2` = the x4 pass folded
//!   down with an exact 2:1 box average; the network itself is 4x-only).
//! - `interpolate: 2|4` — RIFE v4.26 frame-rate multiplication, performed at
//!   the OUTPUT resolution (upscaling the smaller original frames is cheaper
//!   than upscaling the multiplied set; RIFE is the cheap pass).
//! - `flow_map: true` — a per-final-frame-pair RIFE motion field (flow +
//!   occlusion mask at t=0.5) appended to the finished mp4 as a trailing
//!   custom `mkfl` box. MP4 readers skip unknown top-level boxes, so the
//!   artifact stays ONE plain playable video; a flow-aware player finds the
//!   box and warps between decoded frames at arbitrary timesteps.
//!
//! Both models are small (67 MB + 22 MB) and stay co-resident; their weight
//! files are shared cache entries with `realesrgan-x4plus` and the H3 tiers'
//! `interpolate` role, so a box that has either pays nothing extra.
//!
//! The input's audio track is dropped — this is a video-only stage (the VJ
//! loop pipeline generates silent clips by design), and the drop is stated
//! in the model note rather than happening as a silent surprise.
//!
//! ## The `mkfl` box payload
//!
//! The format, its quantization and its box walk are NOT defined here: they
//! live in `makepad-video-flow`, shared with the VJ's import converter, which
//! writes the same box from a classical (model-free) motion field. One
//! definition, two producers, one player contract — see that crate's
//! `payload` module for the byte layout. The names below are re-exported so
//! this backend's callers and tests keep their existing surface.
//!
//! What stays here is what is genuinely this backend's: RIFE as the source of
//! the field, and the upscale/interpolate stages around it.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;

pub use makepad_video_flow::{
    append_mkfl_box, encode_flow_payload, find_mkfl_box, quantize_flow_pair,
};

#[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
mod native {
    pub use makepad_ai_common::DiffusionError;
    pub use makepad_ai_rife::{Rife, RifeFramePair, RifeScale, RifeWeights};
    pub use makepad_ai_vision::realesrgan::{RealEsrgan, RealEsrganImage, RealEsrganWeights};
    pub use makepad_video::{
        VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions,
    };
    pub use std::path::PathBuf;
}
#[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
use native::*;

/// Pluggable run for tests: `(input mp4 bytes, upscale, interpolate,
/// flow_map)` -> output mp4 bytes.
pub type EnhanceFn =
    Box<dyn FnMut(&[u8], u32, u32, bool, ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(EnhanceFn),
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    Native,
}

pub struct VideoEnhanceBackend {
    model_id: String,
    gen: Gen,
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    upscale_path: Option<PathBuf>,
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    upscaler: Option<RealEsrgan>,
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    rife_path: Option<PathBuf>,
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    rife: Option<Rife>,
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    cache_dir: Option<PathBuf>,
}

impl VideoEnhanceBackend {
    /// Test/CI constructor: the whole transform is the closure.
    pub fn with_stub(model_id: &str, gen: EnhanceFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            upscale_path: None,
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            upscaler: None,
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            rife_path: None,
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            rife: None,
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            cache_dir: None,
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            upscale_path: None,
            upscaler: None,
            rife_path: None,
            rife: None,
            cache_dir: None,
        }
    }
}

/// Validates the enhance request shape shared by stub and native paths.
/// Returns `(upscale, interpolate, flow_map)` with 1 = off.
pub fn enhance_factors(params: &GenerateParams) -> Result<(u32, u32, bool), AssetAiError> {
    let upscale = params.upscale.unwrap_or(1);
    let interpolate = params.interpolate.unwrap_or(1);
    let flow_map = params.flow_map;
    if upscale == 1 && interpolate == 1 && !flow_map {
        return Err(AssetAiError::Params(
            "enhance: nothing to do — set upscale 2|4, interpolate 2|4 and/or flow_map"
                .to_string(),
        ));
    }
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params(
            "enhance: needs an input video (input_b64, video/mp4)".to_string(),
        ));
    }
    if !params.input_content_type.starts_with("video/") {
        return Err(AssetAiError::Params(format!(
            "enhance: input must be a video, got {:?}",
            params.input_content_type
        )));
    }
    Ok((upscale, interpolate, flow_map))
}

/// Exact 2:1 box average of an interleaved RGB8 frame (both extents even).
pub fn box_downsample_rgb8(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (ow, oh) = (w / 2, h / 2);
    let mut out = vec![0u8; ow * oh * 3];
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..3 {
                let a = rgb[((2 * y) * w + 2 * x) * 3 + c] as u32;
                let b = rgb[((2 * y) * w + 2 * x + 1) * 3 + c] as u32;
                let d = rgb[((2 * y + 1) * w + 2 * x) * 3 + c] as u32;
                let e = rgb[((2 * y + 1) * w + 2 * x + 1) * 3 + c] as u32;
                out[(y * ow + x) * 3 + c] = ((a + b + d + e + 2) / 4) as u8;
            }
        }
    }
    out
}

impl ContentBackend for VideoEnhanceBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            Gen::Native => {
                self.cache_dir = Some(ctx.cache_dir.to_path_buf());
                let upscale_path = ctx.path_by_role("native-upscale")?;
                let rife_path = ctx.path_by_role("interpolate")?;
                let cancel = ctx.cancel.clone();
                let cancelled = || cancel.is_cancelled();
                if self.upscaler.is_none() || self.upscale_path.as_ref() != Some(&upscale_path) {
                    self.upscaler = None;
                    let weights =
                        RealEsrganWeights::load(&upscale_path).map_err(enhance_err)?;
                    self.upscaler = Some(
                        RealEsrgan::prepare_controlled(&weights, Some(&cancelled), None)
                            .map_err(enhance_err)?,
                    );
                    self.upscale_path = Some(upscale_path);
                }
                if self.rife.is_none() || self.rife_path.as_ref() != Some(&rife_path) {
                    self.rife = None;
                    let weights = RifeWeights::load(&rife_path).map_err(enhance_err)?;
                    self.rife = Some(
                        Rife::prepare_controlled(
                            &weights,
                            RifeScale::Full,
                            Some(&cancelled),
                            None,
                        )
                        .map_err(enhance_err)?,
                    );
                    self.rife_path = Some(rife_path);
                }
                Ok(())
            }
        }
    }

    // `cancel` is only read by the `Gen::Native` arm below, which exists
    // solely under the full upscale/interpolate/video feature combination;
    // a build with a narrower feature set still needs the parameter to
    // satisfy the trait signature.
    #[cfg_attr(
        not(all(feature = "upscale-native", feature = "interpolate", feature = "video")),
        allow(unused_variables)
    )]
    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let (upscale, interpolate, flow_map) = enhance_factors(params)?;
        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, upscale, interpolate, flow_map, progress)?,
            #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
            Gen::Native => self.run_native(params, upscale, interpolate, flow_map, progress, cancel)?,
        };
        Ok(vec![ArtifactData {
            content_type: "video/mp4",
            ext: "mp4",
            bytes,
        }])
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
        {
            self.upscaler = None;
            self.rife = None;
            let _ = makepad_ai_vision::realesrgan::unload_realesrgan();
            let _ = makepad_ai_rife::unload_rife();
        }
        Ok(())
    }
}

#[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
fn enhance_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("enhance: {other}")),
    }
}

#[cfg(all(feature = "upscale-native", feature = "interpolate", feature = "video"))]
impl VideoEnhanceBackend {
    #[allow(clippy::too_many_arguments)]
    fn run_native(
        &mut self,
        params: &GenerateParams,
        upscale: u32,
        interpolate: u32,
        flow_map: bool,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, AssetAiError> {
        let upscaler = self
            .upscaler
            .as_ref()
            .ok_or_else(|| AssetAiError::Backend("enhance: upscaler not loaded".to_string()))?;
        let rife = self
            .rife
            .as_ref()
            .ok_or_else(|| AssetAiError::Backend("enhance: rife not loaded".to_string()))?;
        let tmp_dir = self
            .cache_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join("tmp");
        std::fs::create_dir_all(&tmp_dir)
            .map_err(|e| AssetAiError::Io(format!("enhance tmp dir: {e}")))?;
        let stamp = format!(
            "{}-{:x}",
            std::process::id(),
            params.seed ^ params.input_bytes.len() as u64
        );
        let in_path = tmp_dir.join(format!("enhance-in-{stamp}.mp4"));
        let out_path = tmp_dir.join(format!("enhance-out-{stamp}.mp4"));
        // Both temps die with this call, success or failure.
        let result = (|| {
            std::fs::write(&in_path, &params.input_bytes)
                .map_err(|e| AssetAiError::Io(format!("enhance input write: {e}")))?;
            let mut decoder = VideoFileDecoder::open(
                in_path
                    .to_str()
                    .ok_or_else(|| AssetAiError::Io("enhance: non-utf8 tmp path".to_string()))?,
            )
            .map_err(|e| AssetAiError::Backend(format!("enhance decode open: {e}")))?;
            let info = decoder.info().clone();
            let (src_w, src_h) = (info.width as usize, info.height as usize);
            if src_w == 0 || src_h == 0 || src_w % 2 != 0 || src_h % 2 != 0 {
                return Err(AssetAiError::Params(format!(
                    "enhance: input is {src_w}x{src_h}; even, non-zero dimensions required"
                )));
            }
            let (out_w, out_h) = (src_w * upscale as usize, src_h * upscale as usize);
            let fps_num = info.fps_num.max(1) * interpolate;
            let fps_den = info.fps_den.max(1);
            // Bitrate scales with the pixel rate so a 4x clip does not get
            // starved down to the source bitrate; all-intra (below) needs
            // roughly 3x an inter GOP for the same quality.
            let bitrate =
                (24_000_000u64 * (out_w * out_h) as u64 / (src_w * src_h).max(1) as u64)
                    .clamp(24_000_000, 160_000_000) as u32;
            let mut encoder = VideoFileEncoder::new(
                out_path
                    .to_str()
                    .ok_or_else(|| AssetAiError::Io("enhance: non-utf8 tmp path".to_string()))?,
                VideoFileEncoderOptions {
                    codec: if params.codec == "h265" {
                        VideoFileCodec::H265
                    } else {
                        VideoFileCodec::H264
                    },
                    width: out_w as u32,
                    height: out_h as u32,
                    fps_num,
                    fps_den,
                    video_bitrate_bps: bitrate,
                    audio: None,
                    // Bounce loops play BACKWARDS: every frame must decode
                    // on its own, never by forward-walking a GOP.
                    keyframe_only: true,
                },
            )
            .map_err(|e| AssetAiError::Backend(format!("enhance encode open: {e}")))?;

            // Expected frame count for progress; the container may lie, so it
            // only shapes the bar, never the loop.
            let expected = if info.duration_100ns > 0 {
                ((info.duration_100ns as f64 / 10_000_000.0)
                    * (info.fps_num.max(1) as f64 / fps_den as f64))
                    .round()
                    .max(1.0)
            } else {
                1.0
            };

            let cancelled = || cancel.is_cancelled();
            let timesteps = makepad_ai_rife::interpolation_timesteps(interpolate);
            let flow_grid_w = (src_w / 4).max(1);
            let flow_grid_h = (src_h / 4).max(1);
            let mut flow_samples: Vec<u8> = Vec::new();
            let mut flow_pairs: u32 = 0;

            // prev at OUTPUT res (encode order), prev at SOURCE res (flow).
            let mut prev_out: Option<Vec<u8>> = None;
            let mut prev_src: Option<Vec<u8>> = None;
            let mut frames_in = 0usize;

            while let Some(frame) = decoder
                .next_frame()
                .map_err(|e| AssetAiError::Backend(format!("enhance decode: {e}")))?
            {
                cancel.check()?;
                if frame.width as usize != src_w || frame.height as usize != src_h {
                    return Err(AssetAiError::Backend(format!(
                        "enhance: frame {} is {}x{}, stream is {}x{}",
                        frames_in, frame.width, frame.height, src_w, src_h
                    )));
                }
                let src_rgb = frame.to_rgb8();
                let out_rgb = match upscale {
                    1 => src_rgb.clone(),
                    2 => {
                        let x4 = upscaler
                            .upscale_rgb8_controlled(
                                RealEsrganImage::rgb8(&src_rgb, src_w, src_h)
                                    .map_err(enhance_err)?,
                                Some(&cancelled),
                                None,
                            )
                            .map_err(enhance_err)?;
                        box_downsample_rgb8(&x4, src_w * 4, src_h * 4)
                    }
                    _ => upscaler
                        .upscale_rgb8_controlled(
                            RealEsrganImage::rgb8(&src_rgb, src_w, src_h)
                                .map_err(enhance_err)?,
                            Some(&cancelled),
                            None,
                        )
                        .map_err(enhance_err)?,
                };

                if let Some(prev) = &prev_out {
                    // In-betweens at the output resolution, encode order.
                    for &timestep in &timesteps {
                        cancel.check()?;
                        let pair = RifeFramePair::new(prev, &out_rgb, out_w, out_h)
                            .map_err(enhance_err)?;
                        let mid = rife
                            .interpolate_rgb8_controlled(pair, timestep, Some(&cancelled))
                            .map_err(enhance_err)?;
                        encoder
                            .push_frame_rgb8(&mid, None)
                            .map_err(|e| AssetAiError::Backend(format!("enhance encode: {e}")))?;
                    }
                }
                encoder
                    .push_frame_rgb8(&out_rgb, None)
                    .map_err(|e| AssetAiError::Backend(format!("enhance encode: {e}")))?;

                if flow_map {
                    if let Some(prev) = &prev_src {
                        // Playback flow between SOURCE-cadence neighbours at
                        // source res: with the flow map the player tweens at
                        // ANY rate itself, so the baked in-between pairs
                        // need no fields of their own.
                        let pair = RifeFramePair::new(prev, &src_rgb, src_w, src_h)
                            .map_err(enhance_err)?;
                        let field = rife
                            .flow_field_rgb8(pair, 0.5, Some(&cancelled))
                            .map_err(enhance_err)?;
                        flow_samples.extend_from_slice(&quantize_flow_pair(
                            &field.flow,
                            &field.mask,
                            src_w,
                            src_h,
                            flow_grid_w,
                            flow_grid_h,
                        ));
                        flow_pairs += 1;
                    }
                    prev_src = Some(src_rgb);
                }
                prev_out = Some(out_rgb);
                frames_in += 1;
                progress(
                    &format!("enhance {frames_in} frames"),
                    (frames_in as f64 / expected).min(0.98) * 0.9,
                );
            }
            if frames_in == 0 {
                return Err(AssetAiError::Backend(
                    "enhance: input decoded to zero frames".to_string(),
                ));
            }
            progress("encode finish", 0.94);
            encoder
                .finish()
                .map_err(|e| AssetAiError::Backend(format!("enhance encode finish: {e}")))?;
            let mut bytes = std::fs::read(&out_path)
                .map_err(|e| AssetAiError::Io(format!("enhance output read: {e}")))?;
            if flow_map {
                progress("flow sidecar", 0.97);
                let payload = encode_flow_payload(
                    flow_pairs,
                    flow_grid_w as u16,
                    flow_grid_h as u16,
                    out_w as u16,
                    out_h as u16,
                    fps_num,
                    fps_den,
                    &flow_samples,
                );
                append_mkfl_box(&mut bytes, &payload);
            }
            progress("done", 1.0);
            Ok(bytes)
        })();
        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&out_path);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(upscale: Option<u32>, interpolate: Option<u32>, flow_map: bool) -> GenerateParams {
        let mut params = crate::jobs::tests::generate_params("video-enhance");
        params.input_bytes = vec![0u8; 32];
        params.input_content_type = "video/mp4".to_string();
        params.upscale = upscale;
        params.interpolate = interpolate;
        params.flow_map = flow_map;
        params
    }

    #[test]
    fn factors_refuse_a_no_op_and_a_missing_input() {
        assert!(enhance_factors(&params(None, None, false)).is_err());
        let mut no_input = params(Some(2), None, false);
        no_input.input_bytes.clear();
        assert!(enhance_factors(&no_input).is_err());
        let mut wrong_type = params(Some(2), None, false);
        wrong_type.input_content_type = "image/png".to_string();
        assert!(enhance_factors(&wrong_type).is_err());
        assert_eq!(enhance_factors(&params(Some(2), Some(4), true)).unwrap(), (2, 4, true));
        // flow_map alone is a valid job: the player tweens, we only measure.
        assert_eq!(enhance_factors(&params(None, None, true)).unwrap(), (1, 1, true));
    }

    #[test]
    fn mkfl_box_roundtrips_after_real_mp4_boxes() {
        // A minimal plausible box stream: ftyp + free.
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&16u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom\0\0\0\0");
        mp4.extend_from_slice(&8u32.to_be_bytes());
        mp4.extend_from_slice(b"free");
        let payload = encode_flow_payload(3, 216, 120, 1728, 960, 48, 1, &[7u8; 12]);
        append_mkfl_box(&mut mp4, &payload);
        let found = find_mkfl_box(&mp4).expect("mkfl present");
        assert_eq!(found, &payload[..]);
        assert_eq!(&found[..4], b"MKFL");
    }

    #[test]
    fn flow_quantization_is_quarter_pixel_at_grid_scale() {
        // A constant 8-pixel rightward flow on a 8x4 field, grid 4x2: the
        // grid is half the source width, so the stored vector is 4 grid px
        // = 16 quarter-pixel units.
        let (w, h, gw, gh) = (8usize, 4usize, 4usize, 2usize);
        let plane = w * h;
        let mut flow = vec![0.0f32; 4 * plane];
        flow[..plane].fill(8.0); // f0x
        let mask = vec![0.75f32; plane];
        let out = quantize_flow_pair(&flow, &mask, w, h, gw, gh);
        let grid_plane = gw * gh;
        assert_eq!(out.len(), grid_plane * 5);
        assert!(out[..grid_plane].iter().all(|&b| b as i8 == 16));
        assert!(out[grid_plane..2 * grid_plane].iter().all(|&b| b as i8 == 0));
        assert!(out[4 * grid_plane..].iter().all(|&b| b == 191 || b == 192));
    }

    #[test]
    fn box_downsample_averages_quads() {
        // 2x2 checkerboard of 0/100 -> single pixel of 50.
        let rgb = [
            0u8, 0, 0, 100, 100, 100, //
            100, 100, 100, 0, 0, 0,
        ];
        let out = box_downsample_rgb8(&rgb, 2, 2);
        assert_eq!(out, vec![50, 50, 50]);
    }

    #[test]
    fn stub_backend_flows_the_factors_through() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new((0u32, 0u32, false)));
        let seen_in = seen.clone();
        let mut backend = VideoEnhanceBackend::with_stub(
            "video-enhance",
            Box::new(move |input, upscale, interpolate, flow, _progress| {
                *seen_in.lock().unwrap() = (upscale, interpolate, flow);
                Ok(input.to_vec())
            }),
        );
        let artifacts = backend
            .generate(&params(Some(4), Some(2), true), &mut |_, _| {}, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "video/mp4");
        assert_eq!(*seen.lock().unwrap(), (4, 2, true));
    }
}
