//! The stream codec keeps what the XR picture carries: the eye's strong colours (chroma) and the
//! coverage band below them (luma as grey squares on black). One picture, held for a few frames,
//! goes through the stream encoder and decoder; the colours keep their saturation and the band
//! its white and black.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_video::{StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions};

const W: usize = 320;
const H: usize = 240;
/// The colour part is the top two thirds; the band below it.
const COLOUR_ROWS: usize = H * 2 / 3;

/// Four strongly coloured quadrants over the top, a checkerboard of white and black in the band.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn picture() -> Vec<u8> {
    let mut rgb = vec![0u8; W * H * 3];
    for y in 0..H {
        for x in 0..W {
            let c = if y < COLOUR_ROWS {
                match (x * 2 / W, y * 2 / COLOUR_ROWS) {
                    (0, 0) => [230, 30, 30],
                    (1, 0) => [30, 200, 40],
                    (0, 1) => [30, 60, 230],
                    _ => [220, 40, 200],
                }
            } else if ((x / 16) + ((y - COLOUR_ROWS) / 16)) % 2 == 0 {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            };
            rgb[(y * W + x) * 3..][..3].copy_from_slice(&c);
        }
    }
    rgb
}

fn saturation(px: &[u8]) -> f64 {
    let (hi, lo) = (px.iter().copied().max().unwrap_or(0), px.iter().copied().min().unwrap_or(0));
    (hi - lo) as f64 / (hi as f64).max(1.0)
}

/// Mean saturation of the colour part, and the mean luma of the band's white and black squares
/// (their centres).
fn measure(rgb: &[u8]) -> (f64, f64, f64) {
    let mut sat = 0.0;
    let mut n = 0.0;
    for y in (8..COLOUR_ROWS - 8).step_by(4) {
        for x in (8..W - 8).step_by(4) {
            if (x as i64 - (W / 2) as i64).abs() > 6 && (y as i64 - (COLOUR_ROWS / 2) as i64).abs() > 6 {
                sat += saturation(&rgb[(y * W + x) * 3..][..3]);
                n += 1.0;
            }
        }
    }
    let (mut white, mut black, mut nw, mut nb) = (0.0, 0.0, 0.0, 0.0);
    for y in (COLOUR_ROWS..H - 8).filter(|y| (y - COLOUR_ROWS) % 16 == 8) {
        for x in (8..W).step_by(16) {
            let p = &rgb[(y * W + x) * 3..][..3];
            let l = p.iter().map(|v| *v as f64).sum::<f64>() / 3.0;
            if ((x / 16) + ((y - COLOUR_ROWS) / 16)) % 2 == 0 {
                white += l;
                nw += 1.0;
            } else {
                black += l;
                nb += 1.0;
            }
        }
    }
    (sat / n, white / nw, black / nb)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn round_trip(codec: StreamVideoCodec) {
    let options = VideoStreamEncoderOptions { codec, width: W as u32, height: H as u32, fps: 30, bitrate_kbps: 15_000, keyint: 30, low_latency: true, full_chroma: false };
    let Ok(mut encoder) = VideoStreamEncoder::new(options) else { return eprintln!("{codec:?}: no encoder here") };
    let Ok(mut decoder) = VideoStreamDecoder::new(codec) else { return eprintln!("{codec:?}: no decoder here") };
    let source = picture();
    let (src_sat, src_white, src_black) = measure(&source);
    let mut decoded = Vec::new();
    for i in 0..12i64 {
        for p in encoder.push_frame_rgb8(&source, i * 333_333).expect("encode") {
            decoded.extend(decoder.push_packet(&p.data, p.pts_100ns).expect("decode"));
        }
    }
    decoded.extend(decoder.flush().expect("flush"));
    let last = decoded.last().expect("a decoded picture");
    let (sat, white, black) = measure(&last.to_rgb8());
    eprintln!("{codec:?}: saturation {src_sat:.2} -> {sat:.2}, band white {src_white:.0} -> {white:.0}, black {src_black:.0} -> {black:.0}");
    assert!(sat > src_sat * 0.8, "{codec:?}: the colours came back grey: saturation {src_sat:.2} -> {sat:.2}");
    assert!(white > 200.0 && black < 40.0, "{codec:?}: the coverage band lost its levels: white {white:.0}, black {black:.0}");
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn colours_and_the_coverage_band_survive_the_stream_codec() {
    round_trip(StreamVideoCodec::H264);
    round_trip(StreamVideoCodec::Hevc);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn the_picture_measure_reads_colour_and_band() {
    let rgb = vec![0u8; W * H * 3];
    let (sat, _, black) = measure(&rgb);
    assert_eq!((sat, black), (0.0, 0.0));
}
