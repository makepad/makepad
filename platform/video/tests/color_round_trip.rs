//! Colour metadata both ways: clips written by our encoder in each colour
//! (BT.709 limited and full, BT.601 525 and 625, BT.2020 SDR, PQ and HLG)
//! read back through our decoder, which must report the same colour for the
//! stream and every frame, the same YCbCr bytes on flat patches, and tag the
//! container's `colr` box with the colour's H.273 code points.
//!
//! The clips stay in `$MAKEPAD_COLOR_CLIPS` (default: the temp dir) for the
//! GPU decode checks that draw them.

#![cfg(target_os = "macos")]

use makepad_video::color::{pq_inverse_eotf, HDR_REFERENCE_WHITE_NITS};
use makepad_video::mp4_first_frame::first_access_unit;
use makepad_video::{VideoColor, VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions, VideoMatrix, VideoPrimaries, VideoRange, VideoTransfer};

/// Four vertical bands, each a multiple of 16 pixels wide.
pub const PATCHES: usize = 4;

pub struct Clip {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub codec: VideoFileCodec,
    pub color: VideoColor,
    /// R'G'B' signal per patch, in the clip's own primaries and transfer.
    pub signal: [[f32; 3]; PATCHES],
}

fn pq_gray(nits: f32) -> [f32; 3] {
    [pq_inverse_eotf(nits); 3]
}

pub fn clips() -> Vec<Clip> {
    let sdr = [[0.9, 0.9, 0.9], [0.8, 0.15, 0.1], [0.1, 0.7, 0.2], [0.15, 0.2, 0.85]];
    let c = |matrix, range, primaries, transfer| VideoColor { matrix, range, primaries, transfer };
    use VideoMatrix as M;
    use VideoPrimaries as P;
    use VideoRange as R;
    use VideoTransfer as T;
    vec![
        Clip { name: "bt709-limited", width: 1280, height: 720, codec: VideoFileCodec::H264, color: VideoColor::BT709, signal: sdr },
        Clip { name: "bt709-full", width: 1280, height: 720, codec: VideoFileCodec::H264, color: c(M::Bt709, R::Full, P::Bt709, T::Sdr), signal: sdr },
        Clip { name: "bt601-525", width: 720, height: 480, codec: VideoFileCodec::H264, color: c(M::Bt601, R::Limited, P::Smpte170m, T::Sdr), signal: sdr },
        Clip { name: "bt601-625", width: 720, height: 576, codec: VideoFileCodec::H264, color: c(M::Bt601, R::Limited, P::Bt470bg, T::Sdr), signal: sdr },
        Clip { name: "bt2020-sdr", width: 1920, height: 1080, codec: VideoFileCodec::H265, color: c(M::Bt2020, R::Limited, P::Bt2020, T::Sdr), signal: sdr },
        Clip {
            name: "bt2020-pq",
            width: 1920,
            height: 1080,
            codec: VideoFileCodec::H265,
            color: c(M::Bt2020, R::Limited, P::Bt2020, T::Pq),
            // 100 nits, reference white, the 1000 nit peak, and a red at reference white
            signal: [pq_gray(100.0), pq_gray(HDR_REFERENCE_WHITE_NITS), pq_gray(1000.0), [pq_inverse_eotf(HDR_REFERENCE_WHITE_NITS), pq_inverse_eotf(5.0), pq_inverse_eotf(5.0)]],
        },
        Clip {
            name: "bt2020-hlg",
            width: 1920,
            height: 1080,
            codec: VideoFileCodec::H265,
            color: c(M::Bt2020, R::Limited, P::Bt2020, T::Hlg),
            // 50%, the 75% reference white, peak, and a red
            signal: [[0.5; 3], [0.75; 3], [1.0; 3], [0.75, 0.2, 0.2]],
        },
    ]
}

pub fn bytes_of(clip: &Clip) -> [[u8; 3]; PATCHES] {
    clip.signal.map(|s| clip.color.encode_ycbcr(s))
}

pub fn nv12_of(clip: &Clip) -> Vec<u8> {
    let (w, h) = (clip.width as usize, clip.height as usize);
    let band = w / PATCHES;
    let mut nv12 = vec![0u8; w * h + w * h / 2];
    let bytes = bytes_of(clip);
    for y in 0..h {
        for x in 0..w {
            nv12[y * w + x] = bytes[(x / band).min(PATCHES - 1)][0];
        }
    }
    for y in 0..h / 2 {
        for bx in 0..w / 2 {
            let p = bytes[(bx * 2 / band).min(PATCHES - 1)];
            nv12[w * h + y * w + bx * 2] = p[1];
            nv12[w * h + y * w + bx * 2 + 1] = p[2];
        }
    }
    nv12
}

fn clip_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("MAKEPAD_COLOR_CLIPS").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("makepad-color-clips"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_colour_round_trips_with_its_tags() {
    let dir = clip_dir();
    for clip in clips() {
        let path = dir.join(format!("{}.mp4", clip.name));
        let path = path.to_str().unwrap();
        let mut e = VideoFileEncoder::new(
            path,
            VideoFileEncoderOptions {
                codec: clip.codec,
                width: clip.width,
                height: clip.height,
                fps_num: 30,
                fps_den: 1,
                video_bitrate_bps: 20_000_000,
                keyframe_only: true,
                color: clip.color,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e:?}", clip.name));
        let nv12 = nv12_of(&clip);
        for k in 0..3 {
            e.push_frame_nv12(&nv12, Some(k * 333_333)).unwrap();
        }
        if clip.color.is_hdr() {
            let rgb = vec![0u8; clip.width as usize * clip.height as usize * 3];
            assert!(e.push_frame_rgb8(&rgb, None).is_err(), "{}: RGB into HDR must be refused", clip.name);
        }
        e.finish().unwrap();

        // the container's colr box
        let file = std::fs::read(path).unwrap();
        let au = first_access_unit(&file).unwrap();
        assert_eq!(au.color, clip.color, "{}: colr box of {path}", clip.name);

        // the decoder: stream, frames, bytes
        let mut d = VideoFileDecoder::open(path).unwrap();
        assert_eq!(d.info().color, clip.color, "{}: stream colour", clip.name);
        let frame = d.next_frame().unwrap().expect("a frame");
        assert_eq!(frame.color, clip.color, "{}: frame colour", clip.name);
        let (w, h) = (frame.width as usize, frame.height as usize);
        let band = w / PATCHES;
        for (i, want) in bytes_of(&clip).iter().enumerate() {
            let (x, y) = (i * band + band / 2, h / 2);
            let got = [frame.nv12[y * w + x], frame.nv12[w * h + (y / 2) * w + (x / 2) * 2], frame.nv12[w * h + (y / 2) * w + (x / 2) * 2 + 1]];
            for c in 0..3 {
                assert!((got[c] as i32 - want[c] as i32).abs() <= 2, "{} patch {i}: {got:?} vs {want:?}", clip.name);
            }
        }
        println!("{}: ok {:?} -> sdr {:?}", clip.name, clip.color, bytes_of(&clip).map(|b| clip.color.reference_rgb(b[0], b[1], b[2]).map(|v| (v * 255.0).round() as u8)));
    }
}
