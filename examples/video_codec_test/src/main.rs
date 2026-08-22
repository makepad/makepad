//! Self-verifying test for the platform video file codec seam.
//!
//! Encodes 120 synthetic frames (moving gradient + tracked white square) and a
//! 440 Hz tone into HEVC and H.264 MP4s via the hardware-transform sink
//! writer, then decodes the HEVC file back and checks stream info, square
//! motion, frame count, audio length and tone frequency.
//!
//! Run with `--long` for a 1080p 60s encode (for watching hardware encoder
//! utilization from the outside, e.g. nvidia-smi / task manager).

use makepad_platform::video_file::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileDecoder, VideoFileEncoder,
    VideoFileEncoderOptions,
};

const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 30;
const FRAMES: u32 = 120;
const SQUARE: u32 = 40;
const AUDIO_RATE: u32 = 48000;
const TONE_HZ: f64 = 440.0;

fn square_x(frame: u32) -> u32 {
    (frame * 4) % (W - SQUARE)
}

fn make_frame(frame: u32, rgb: &mut Vec<u8>) {
    rgb.clear();
    rgb.resize((W * H * 3) as usize, 0);
    let sx = square_x(frame);
    let sy = (H - SQUARE) / 2;
    for y in 0..H {
        for x in 0..W {
            let p = ((y * W + x) * 3) as usize;
            if x >= sx && x < sx + SQUARE && y >= sy && y < sy + SQUARE {
                rgb[p] = 255;
                rgb[p + 1] = 255;
                rgb[p + 2] = 255;
            } else {
                // Moving gradient, kept below the square-detect threshold.
                rgb[p] = (((x + frame * 2) * 160) / W) as u8;
                rgb[p + 1] = ((y * 160) / H) as u8;
                rgb[p + 2] = 40;
            }
        }
    }
}

fn make_tone(seconds: f64) -> Vec<i16> {
    let n = (seconds * AUDIO_RATE as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / AUDIO_RATE as f64;
            ((t * TONE_HZ * std::f64::consts::TAU).sin() * 0.5 * i16::MAX as f64) as i16
        })
        .collect()
}

struct Check {
    failures: u32,
}

impl Check {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        if ok {
            println!("PASS {}: {}", name, detail);
        } else {
            println!("FAIL {}: {}", name, detail);
            self.failures += 1;
        }
    }
}

fn encode(
    path: &str,
    codec: VideoFileCodec,
    with_audio: bool,
    frames: u32,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoder = VideoFileEncoder::new(
        path,
        VideoFileEncoderOptions {
            codec,
            width,
            height,
            fps_num: FPS,
            fps_den: 1,
            video_bitrate_bps: 6_000_000,
            audio: with_audio.then(|| PcmAudioTrackOptions {
                sample_rate: AUDIO_RATE,
                channels: 1,
                aac_bitrate_bps: 128_000,
            }),
            ..Default::default()
        },
    )?;
    match encoder.video_transform() {
        Some(info) => println!(
            "ENCODER {:?} -> {} | transform: '{}' | hardware: {}",
            codec, path, info.name, info.is_hardware
        ),
        None => println!("ENCODER {:?} -> {} | transform: unreported", codec, path),
    }

    let mut rgb = Vec::new();
    let scale_w = width / W;
    let scale_h = height / H;
    let mut scaled = Vec::new();
    for frame in 0..frames {
        make_frame(frame % FRAMES, &mut rgb);
        if width == W && height == H {
            encoder.push_frame_rgb8(&rgb, None)?;
        } else {
            // Nearest-neighbour upscale for the --long soak.
            scaled.clear();
            scaled.resize((width * height * 3) as usize, 0);
            for y in 0..height {
                let sy = (y / scale_h.max(1)).min(H - 1);
                for x in 0..width {
                    let sx = (x / scale_w.max(1)).min(W - 1);
                    let src = ((sy * W + sx) * 3) as usize;
                    let dst = ((y * width + x) * 3) as usize;
                    scaled[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
                }
            }
            encoder.push_frame_rgb8(&scaled, None)?;
        }
    }
    if with_audio {
        let tone = make_tone(frames as f64 / FPS as f64);
        // Push in ~100ms chunks, the shape a streaming producer would use.
        for chunk in tone.chunks((AUDIO_RATE / 10) as usize) {
            encoder.push_audio_i16(chunk)?;
        }
    }
    encoder.finish()?;
    let bytes = std::fs::metadata(path)?.len();
    println!("ENCODED {} ({} bytes)", path, bytes);
    Ok(())
}

fn decode_and_verify(path: &str, check: &mut Check) -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = VideoFileDecoder::open(path)?;
    let info = decoder.info().clone();
    println!("DECODER {} | info: {:?}", path, info);

    check.check(
        "decode.size",
        info.width == W && info.height == H,
        format!("{}x{}", info.width, info.height),
    );
    check.check(
        "decode.fps",
        info.fps_num == FPS && info.fps_den == 1,
        format!("{}/{}", info.fps_num, info.fps_den),
    );
    check.check(
        "decode.codec",
        info.video_codec == Some(VideoFileCodec::H265),
        format!("{:?} (fourcc {:#x})", info.video_codec, info.video_codec_fourcc),
    );
    let duration_s = info.duration_100ns as f64 / 1e7;
    let expected_s = FRAMES as f64 / FPS as f64;
    check.check(
        "decode.duration",
        (duration_s - expected_s).abs() < 0.25,
        format!("{:.3}s (expected ~{:.3}s)", duration_s, expected_s),
    );
    check.check(
        "decode.audio_present",
        info.has_audio && info.audio_sample_rate == AUDIO_RATE && info.audio_channels == 1,
        format!(
            "has_audio: {} rate: {} channels: {}",
            info.has_audio, info.audio_sample_rate, info.audio_channels
        ),
    );

    // Video: count frames, track the white square in a few of them.
    let mut frame_count = 0u32;
    let mut square_err_max = 0f64;
    let mut square_checked = 0u32;
    while let Some(frame) = decoder.next_frame()? {
        let index = frame_count;
        frame_count += 1;
        if index % 20 != 10 {
            continue;
        }
        let rgb = frame.to_rgb8();
        // Centroid of near-white pixels.
        let (mut sum_x, mut sum_y, mut n) = (0f64, 0f64, 0f64);
        for y in 0..H {
            for x in 0..W {
                let p = ((y * W + x) * 3) as usize;
                if rgb[p] > 200 && rgb[p + 1] > 200 && rgb[p + 2] > 200 {
                    sum_x += x as f64;
                    sum_y += y as f64;
                    n += 1.0;
                }
            }
        }
        if n < 100.0 {
            check.check(
                "decode.square_found",
                false,
                format!("frame {}: only {} bright pixels", index, n),
            );
            continue;
        }
        let cx = sum_x / n;
        let cy = sum_y / n;
        let expected_cx = square_x(index) as f64 + SQUARE as f64 / 2.0 - 0.5;
        let expected_cy = ((H - SQUARE) / 2) as f64 + SQUARE as f64 / 2.0 - 0.5;
        let err = ((cx - expected_cx).powi(2) + (cy - expected_cy).powi(2)).sqrt();
        square_err_max = square_err_max.max(err);
        square_checked += 1;
        println!(
            "frame {:3}: square centroid ({:6.1},{:6.1}) expected ({:6.1},{:6.1}) err {:.1}px n {}",
            index, cx, cy, expected_cx, expected_cy, err, n
        );
    }
    check.check(
        "decode.frame_count",
        frame_count == FRAMES,
        format!("{} (expected {})", frame_count, FRAMES),
    );
    check.check(
        "decode.square_motion",
        square_checked >= 5 && square_err_max < 8.0,
        format!(
            "{} frames checked, max centroid error {:.1}px",
            square_checked, square_err_max
        ),
    );

    // Audio: total length and dominant frequency via zero crossings.
    let mut samples: Vec<i16> = Vec::new();
    while let Some(chunk) = decoder.next_audio()? {
        samples.extend_from_slice(&chunk.samples);
    }
    let expected_samples = (FRAMES as f64 / FPS as f64 * AUDIO_RATE as f64) as usize;
    check.check(
        "decode.audio_length",
        (samples.len() as f64 - expected_samples as f64).abs()
            < expected_samples as f64 * 0.15,
        format!("{} samples (expected ~{})", samples.len(), expected_samples),
    );
    // Skip AAC priming ramp at both ends.
    let inner = &samples[samples.len() / 8..samples.len() * 7 / 8];
    let mut crossings = 0u32;
    for pair in inner.windows(2) {
        if (pair[0] < 0) != (pair[1] < 0) {
            crossings += 1;
        }
    }
    let freq = crossings as f64 * AUDIO_RATE as f64 / (2.0 * inner.len() as f64);
    check.check(
        "decode.tone",
        (freq - TONE_HZ).abs() < TONE_HZ * 0.08,
        format!("~{:.1} Hz (expected {} Hz)", freq, TONE_HZ),
    );
    Ok(())
}

fn main() {
    let long = std::env::args().any(|a| a == "--long");
    let mut check = Check { failures: 0 };

    // --decode <path>: decode-verify an existing file carrying this test's
    // content (cross-platform artifact check: e.g. NVENC-encoded on a
    // Windows box, decoded through VideoToolbox on a mac).
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--decode") {
        let path = args.get(i + 1).expect("--decode <path>");
        if let Err(e) = decode_and_verify(path, &mut check) {
            println!("FAIL decode: {}", e);
            std::process::exit(1);
        }
        if check.failures == 0 {
            println!("ALL CHECKS PASSED");
        } else {
            println!("{} CHECKS FAILED", check.failures);
            std::process::exit(1);
        }
        return;
    }

    if long {
        // 1080p, 60s: enough runtime to watch the hardware encoder engine
        // from the outside (nvidia-smi encoder %, task manager video encode).
        if let Err(e) = encode("codec_test_long.mp4", VideoFileCodec::H265, true, 1800, 1920, 1080) {
            println!("FAIL encode.long: {}", e);
            std::process::exit(1);
        }
        println!("long encode done");
        return;
    }

    if let Err(e) = encode("codec_test.hevc.mp4", VideoFileCodec::H265, true, FRAMES, W, H) {
        println!("FAIL encode.hevc: {}", e);
        std::process::exit(1);
    }
    if let Err(e) = encode("codec_test.h264.mp4", VideoFileCodec::H264, false, FRAMES, W, H) {
        println!("FAIL encode.h264: {}", e);
        std::process::exit(1);
    }
    if let Err(e) = decode_and_verify("codec_test.hevc.mp4", &mut check) {
        println!("FAIL decode: {}", e);
        std::process::exit(1);
    }

    // H.264 file: same decode path through the other codec MFT.
    match VideoFileDecoder::open("codec_test.h264.mp4") {
        Ok(mut decoder) => {
            let info = decoder.info().clone();
            check.check(
                "decode.h264_codec",
                info.video_codec == Some(VideoFileCodec::H264)
                    && info.width == W
                    && info.height == H,
                format!("{:?} {}x{}", info.video_codec, info.width, info.height),
            );
            let mut frames = 0u32;
            loop {
                match decoder.next_frame() {
                    Ok(Some(_)) => frames += 1,
                    Ok(None) => break,
                    Err(e) => {
                        check.check("decode.h264_frames", false, format!("{}", e));
                        break;
                    }
                }
            }
            check.check(
                "decode.h264_frames",
                frames == FRAMES,
                format!("{} (expected {})", frames, FRAMES),
            );
        }
        Err(e) => check.check("decode.h264_codec", false, format!("{}", e)),
    }

    if check.failures == 0 {
        println!("ALL CHECKS PASSED");
    } else {
        println!("{} CHECKS FAILED", check.failures);
        std::process::exit(1);
    }
}
