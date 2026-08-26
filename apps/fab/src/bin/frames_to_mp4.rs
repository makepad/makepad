//! Encode a directory of `frame_%06d.png` files into an H.264 mp4 via the
//! platform hardware encoder (`makepad-video` / VideoToolbox on macOS).
//!
//! ```text
//! frames_to_mp4 <dir> <out.mp4> [--fps 24] [--start N] [--end M] [--bitrate BPS] [--crf N]
//! ```

use makepad_video::{VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: frames_to_mp4 <dir> <out.mp4> [--fps 24] [--start N] [--end M] [--bitrate BPS] [--crf N]";
const DEFAULT_FPS: u32 = 24;
const DEFAULT_BITRATE: u32 = 8_000_000;

struct Args {
    dir: PathBuf,
    out: PathBuf,
    fps: u32,
    start: Option<u64>,
    end: Option<u64>,
    bitrate: u32,
}

fn parse_u32(flag: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{flag} expects an integer, got {value:?}"))
}

fn parse_u64(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} expects an integer, got {value:?}"))
}

/// AVFoundation exposes average bitrate, not CRF. Map a ffmpeg-like CRF
/// (0 = lossless-ish, 51 = worst) onto a bitrate so `--crf` is accepted.
fn bitrate_from_crf(crf: u32) -> u32 {
    let crf = crf.min(51);
    let scale = (51 - crf) as u64;
    (500_000 + scale * scale * 7_500).min(40_000_000) as u32
}

fn take_value<'a>(flag: &str, rest: &mut std::iter::Peekable<std::slice::Iter<'a, String>>) -> Result<&'a str, String> {
    rest.next()
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut pos = Vec::new();
    let mut fps = DEFAULT_FPS;
    let mut start = None;
    let mut end = None;
    let mut bitrate = DEFAULT_BITRATE;
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--fps" => {
                let v = take_value("--fps", &mut iter)?;
                fps = parse_u32("--fps", v)?;
                if fps == 0 {
                    return Err("--fps must be > 0".into());
                }
            }
            "--start" => {
                let v = take_value("--start", &mut iter)?;
                start = Some(parse_u64("--start", v)?);
            }
            "--end" => {
                let v = take_value("--end", &mut iter)?;
                end = Some(parse_u64("--end", v)?);
            }
            "--bitrate" => {
                let v = take_value("--bitrate", &mut iter)?;
                bitrate = parse_u32("--bitrate", v)?;
                if bitrate == 0 {
                    return Err("--bitrate must be > 0".into());
                }
            }
            "--crf" => {
                let v = take_value("--crf", &mut iter)?;
                bitrate = bitrate_from_crf(parse_u32("--crf", v)?);
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            other => pos.push(other.to_string()),
        }
    }
    if pos.len() != 2 {
        return Err(USAGE.to_string());
    }
    if let (Some(a), Some(b)) = (start, end) {
        if a > b {
            return Err(format!("--start {a} is after --end {b}"));
        }
    }
    Ok(Args {
        dir: PathBuf::from(&pos[0]),
        out: PathBuf::from(&pos[1]),
        fps,
        start,
        end,
        bitrate,
    })
}

fn encoder_options(width: u32, height: u32, args: &Args) -> VideoFileEncoderOptions {
    VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width,
        height,
        fps_num: args.fps,
        fps_den: 1,
        video_bitrate_bps: args.bitrate,
        audio: None,
        keyframe_only: false,
    }
}

fn parse_frame_name(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("frame_")?.strip_suffix(".png")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn list_frame_files(dir: &Path) -> Result<BTreeMap<u64, PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut map = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let name = entry.file_name();
        let Some(idx) = parse_frame_name(&name.to_string_lossy()) else {
            continue;
        };
        map.insert(idx, entry.path());
    }
    Ok(map)
}

fn select_frames(
    map: &BTreeMap<u64, PathBuf>,
    start: Option<u64>,
    end: Option<u64>,
) -> (Vec<(u64, PathBuf)>, Vec<u64>) {
    if map.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let lo = start.unwrap_or_else(|| *map.keys().next().unwrap());
    let hi = end.unwrap_or_else(|| *map.keys().next_back().unwrap());
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for i in lo..=hi {
        match map.get(&i) {
            Some(path) => present.push((i, path.clone())),
            None => missing.push(i),
        }
    }
    (present, missing)
}

fn decode_png_rgb(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(&bytes), options);
    let pixels = decoder
        .decode_raw()
        .map_err(|e| format!("decode {}: {e:?}", path.display()))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| format!("{}: no dimensions", path.display()))?;
    let components = decoder
        .colorspace()
        .ok_or_else(|| format!("{}: no colorspace", path.display()))?
        .num_components();
    if components == 0 {
        return Err(format!("{}: empty colorspace", path.display()));
    }
    let n = width * height;
    let need = n.saturating_mul(components);
    if pixels.len() < need {
        return Err(format!(
            "{}: expected at least {need} bytes, got {}",
            path.display(),
            pixels.len()
        ));
    }
    let mut rgb = vec![0u8; n * 3];
    if components == 3 {
        rgb.copy_from_slice(&pixels[..n * 3]);
    } else {
        for i in 0..n {
            let src = i * components;
            if components == 1 {
                let g = pixels[src];
                rgb[i * 3] = g;
                rgb[i * 3 + 1] = g;
                rgb[i * 3 + 2] = g;
            } else {
                rgb[i * 3] = pixels[src];
                rgb[i * 3 + 1] = pixels[src + 1];
                rgb[i * 3 + 2] = pixels[src + 2.min(components - 1)];
            }
        }
    }
    Ok((rgb, width as u32, height as u32))
}

fn crop_rgb_even(rgb: &[u8], width: u32, height: u32, even_w: u32, even_h: u32) -> Vec<u8> {
    if width == even_w && height == even_h {
        return rgb.to_vec();
    }
    let mut out = vec![0u8; even_w as usize * even_h as usize * 3];
    let src_stride = width as usize * 3;
    let dst_stride = even_w as usize * 3;
    for y in 0..even_h as usize {
        let src = y * src_stride;
        let dst = y * dst_stride;
        out[dst..dst + dst_stride].copy_from_slice(&rgb[src..src + dst_stride]);
    }
    out
}

fn encode_dir(args: &Args) -> Result<(u64, f64, u32, u32), String> {
    let map = list_frame_files(&args.dir)?;
    if map.is_empty() {
        return Err(format!("no frame_*.png files in {}", args.dir.display()));
    }
    let (frames, missing) = select_frames(&map, args.start, args.end);
    for idx in &missing {
        eprintln!("warning: missing frame_{idx:06}.png");
    }
    if frames.is_empty() {
        return Err("no frames to encode in the requested range".into());
    }

    let mut encoder: Option<VideoFileEncoder> = None;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut encoded = 0u64;

    for (_idx, path) in &frames {
        let (rgb, w, h) = match decode_png_rgb(path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skip {}: {e}", path.display());
                continue;
            }
        };
        let even_w = w & !1;
        let even_h = h & !1;
        if even_w == 0 || even_h == 0 {
            eprintln!("warning: skip {}: size {w}x{h} is too small", path.display());
            continue;
        }
        let rgb = if w != even_w || h != even_h {
            crop_rgb_even(&rgb, w, h, even_w, even_h)
        } else {
            rgb
        };
        if encoder.is_none() {
            width = even_w;
            height = even_h;
            let options = encoder_options(width, height, args);
            let path_text = args.out.to_string_lossy();
            encoder = Some(
                VideoFileEncoder::new(&path_text, options)
                    .map_err(|e| format!("encoder open {}: {e}", args.out.display()))?,
            );
        } else if even_w != width || even_h != height {
            eprintln!(
                "warning: skip {}: size {w}x{h} != {width}x{height}",
                path.display()
            );
            continue;
        }
        encoder
            .as_mut()
            .unwrap()
            .push_frame_rgb8(&rgb, None)
            .map_err(|e| format!("push {}: {e}", path.display()))?;
        encoded += 1;
    }

    let Some(enc) = encoder else {
        return Err("no frames encoded".into());
    };
    enc.finish().map_err(|e| format!("encoder finish: {e}"))?;
    let duration = encoded as f64 / args.fps as f64;
    Ok((encoded, duration, width, height))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match encode_dir(&args) {
        Ok((encoded, duration, width, height)) => {
            println!(
                "encoded {encoded} frames, duration {duration:.3}s ({width}x{height} @ {} fps) -> {}",
                args.fps,
                args.out.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parse_defaults() {
        let a = parse_args(&argv("frames out.mp4")).unwrap();
        assert_eq!(a.dir, PathBuf::from("frames"));
        assert_eq!(a.out, PathBuf::from("out.mp4"));
        assert_eq!(a.fps, 24);
        assert_eq!(a.start, None);
        assert_eq!(a.end, None);
        assert_eq!(a.bitrate, DEFAULT_BITRATE);
    }

    #[test]
    fn parse_flags_anywhere() {
        let a = parse_args(&argv("--fps 30 d --start 10 o.mp4 --end 20 --bitrate 4000000")).unwrap();
        assert_eq!(a.dir, PathBuf::from("d"));
        assert_eq!(a.out, PathBuf::from("o.mp4"));
        assert_eq!(a.fps, 30);
        assert_eq!(a.start, Some(10));
        assert_eq!(a.end, Some(20));
        assert_eq!(a.bitrate, 4_000_000);
    }

    #[test]
    fn parse_crf_maps_to_bitrate() {
        let a = parse_args(&argv("d o.mp4 --crf 23")).unwrap();
        assert_eq!(a.bitrate, bitrate_from_crf(23));
        assert!(a.bitrate > 0);
    }

    #[test]
    fn parse_rejects_bad_usage() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&argv("--fps 24")).is_err());
        assert!(parse_args(&argv("only_one")).is_err());
        assert!(parse_args(&argv("d o.mp4 --unknown 1")).is_err());
        assert!(parse_args(&argv("d o.mp4 --fps 0")).is_err());
        assert!(parse_args(&argv("d o.mp4 --start 5 --end 1")).is_err());
    }

    #[test]
    fn constructs_h264_encoder_options() {
        let a = parse_args(&argv("d o.mp4 --fps 24 --bitrate 16000000")).unwrap();
        let o = encoder_options(1920, 1080, &a);
        assert_eq!(o.codec, VideoFileCodec::H264);
        assert_eq!(o.width, 1920);
        assert_eq!(o.height, 1080);
        assert_eq!(o.fps_num, 24);
        assert_eq!(o.fps_den, 1);
        assert_eq!(o.video_bitrate_bps, 16_000_000);
        assert!(o.audio.is_none());
        assert!(!o.keyframe_only);
    }

    #[test]
    fn frame_names_sort_numerically() {
        assert_eq!(parse_frame_name("frame_000001.png"), Some(1));
        assert_eq!(parse_frame_name("frame_12.png"), Some(12));
        assert_eq!(parse_frame_name("frame_000000.png"), Some(0));
        assert_eq!(parse_frame_name("other.png"), None);
        assert_eq!(parse_frame_name("frame_00a.png"), None);
    }

    #[test]
    fn select_skips_missing_indices() {
        let mut map = BTreeMap::new();
        map.insert(0, PathBuf::from("frame_000000.png"));
        map.insert(2, PathBuf::from("frame_000002.png"));
        let (present, missing) = select_frames(&map, Some(0), Some(2));
        assert_eq!(present.len(), 2);
        assert_eq!(present[0].0, 0);
        assert_eq!(present[1].0, 2);
        assert_eq!(missing, vec![1]);
    }

    #[test]
    fn bitrate_from_crf_is_sane() {
        assert!(bitrate_from_crf(0) > bitrate_from_crf(23));
        assert!(bitrate_from_crf(23) > bitrate_from_crf(51));
        assert!(bitrate_from_crf(23) > 0);
    }
}
