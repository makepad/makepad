//! `VideoFileEncoder::new_fragmented` — is the file on disk a movie a
//! reader can open while the encoder is still writing it?
//!
//! Stage's WEB cache records a page's audio and picture into one mp4 that
//! a DJ deck and a VJ slot read from behind the recorder, so the container
//! has to be readable before `finish`. A plain movie is not: its index is
//! written last. A fragmented movie is: the header goes down first and
//! every interval closes a fragment behind the frames.
//!
//! Runs for real on Apple only (the platform that honours the interval);
//! elsewhere the fragmented constructor writes one movie and this test
//! only checks it still finishes.

use makepad_video::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileDecoder, VideoFileEncoder,
    VideoFileEncoderOptions,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 192;
const FPS: u32 = 30;
const SECONDS: usize = 4;
const RATE: u32 = 48_000;

fn frame(index: usize) -> Vec<u8> {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut nv12 = vec![128u8; w * h * 3 / 2];
    let level = 16 + ((index * 7) % 200) as u8;
    for y in 0..h {
        for x in 0..w {
            nv12[y * w + x] = if x < w / 2 { level } else { 235 - level };
        }
    }
    nv12
}

fn options() -> VideoFileEncoderOptions {
    VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width: WIDTH,
        height: HEIGHT,
        fps_num: FPS,
        fps_den: 1,
        video_bitrate_bps: 6_000_000,
        audio: Some(PcmAudioTrackOptions { sample_rate: RATE, channels: 2, aac_bitrate_bps: 128_000 }),
        keyframe_only: true,
    }
}

fn frames_readable(path: &str) -> Result<usize, String> {
    let mut decoder = VideoFileDecoder::open(path).map_err(|e| e.to_string())?;
    let mut count = 0;
    while decoder.next_frame().map_err(|e| e.to_string())?.is_some() {
        count += 1;
    }
    Ok(count)
}

#[test]
fn a_fragmented_recording_opens_before_it_finishes() {
    let dir = std::env::temp_dir().join(format!("makepad-fragmented-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("growing.mp4");
    let text = path.to_string_lossy().to_string();

    let mut encoder = match VideoFileEncoder::new_fragmented(&text, options(), 0.5) {
        Ok(encoder) => encoder,
        Err(e) if e.context.contains("not implemented") => return,
        Err(e) => panic!("encoder: {e:?}"),
    };
    let per_frame_audio: Vec<i16> = (0..(RATE as usize / FPS as usize) * 2)
        .map(|i| ((i / 2) % 100) as i16 * 200 - 10_000)
        .collect();
    let total = SECONDS * FPS as usize;
    for index in 0..total {
        let pts = (index as i64) * 10_000_000 / FPS as i64;
        encoder.push_frame_nv12(&frame(index), Some(pts)).expect("push frame");
        encoder.push_audio_i16(&per_frame_audio).expect("push audio");
    }

    // Still recording: the file must already be a movie with most of the
    // frames in it. The writer keeps about a second in its own pipeline
    // and the last fragment is still open, so a reader behind the
    // recorder by up to two seconds is the expectation, not a failure.
    if cfg!(target_vendor = "apple") {
        let readable = frames_readable(&text).expect("a fragmented movie opens while it is being written");
        let lag_frames = 2 * FPS as usize;
        assert!(
            readable + lag_frames >= total && readable > 0,
            "{readable} of {total} frames readable before finish"
        );
        // A reader that opens later sees more, never less.
        let again = frames_readable(&text).expect("opens again");
        assert!(again >= readable, "{again} < {readable}");
    }

    encoder.finish().expect("finish");
    let whole = frames_readable(&text).expect("the finished movie opens");
    assert_eq!(whole, total, "every frame once finished");
    let info = VideoFileDecoder::open(&text).expect("open").info().clone();
    assert!(info.has_audio, "the audio track is in the same file");
    let _ = std::fs::remove_dir_all(&dir);
}
