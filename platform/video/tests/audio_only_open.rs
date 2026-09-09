//! `VideoFileDecoder::open_audio` — a container opened for its sound, and a
//! container that has no picture at all.
//!
//! Every opener behind `open` asks the container for a video track before it
//! looks at an audio one, so a file with only sound in it could not be opened
//! for the sound it had. `open_audio` is the second door: the audio stream is
//! configured and no picture is negotiated.
//!
//! The fixtures are built here, in process, by the in-repo encoder: nothing
//! in this tree writes an audio-only container (the encoder refuses a clip
//! with no picture, and the audio encoder writes Ogg Vorbis), so the
//! picture-less case is made by striking the video track out of a clip this
//! test just encoded. The surgery is twenty lines of box walking and it
//! asserts on its own output, so a demuxer that dislikes the doctored file is
//! told apart from a bug in the door.
//!
//! Runs for real on the two desktop platforms with a decoder behind this
//! facade.
#![cfg(any(target_os = "windows", target_os = "macos"))]

use makepad_video::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileDecoder, VideoFileEncoder,
    VideoFileEncoderOptions,
};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const FPS: u32 = 30;
const FRAME_COUNT: usize = 30;
const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;

/// One directory per test: they run at once and each clears up after
/// itself, so a shared one is a test deleting another's fixture.
fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("makepad-audio-only-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    dir
}

/// A one-second clip: a flat grey picture and a tone, or a flat grey picture
/// alone when `with_audio` is false.
fn encode_clip(path: &std::path::Path, with_audio: bool) {
    let mut encoder = VideoFileEncoder::new(
        path.to_str().expect("utf8 path"),
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: WIDTH,
            height: HEIGHT,
            fps_num: FPS,
            fps_den: 1,
            video_bitrate_bps: 2_000_000,
            audio: with_audio.then_some(PcmAudioTrackOptions {
                sample_rate: AUDIO_RATE,
                channels: AUDIO_CHANNELS,
                aac_bitrate_bps: 128_000,
            }),
            keyframe_only: false,
        },
    )
    .expect("file encoder creation");
    let frame = vec![128u8; WIDTH as usize * HEIGHT as usize * 3];
    for _ in 0..FRAME_COUNT {
        encoder.push_frame_rgb8(&frame, None).expect("push frame");
    }
    if with_audio {
        let total = AUDIO_RATE as usize * AUDIO_CHANNELS as usize;
        let samples: Vec<i16> = (0..total)
            .map(|i| (((i / AUDIO_CHANNELS as usize) % 200) as i16 - 100) * 160)
            .collect();
        encoder.push_audio_i16(&samples).expect("push audio");
    }
    encoder.finish().expect("finish");
}

/// Overwrite the type of the container's video track with `free`, the skip
/// box every demuxer ignores. Nothing moves and the file keeps its length, so
/// every offset in the audio track's sample table stays where it was.
fn strike_the_picture(bytes: &mut [u8]) -> bool {
    fn boxes(bytes: &[u8], mut at: usize, end: usize, mut each: impl FnMut(usize, &[u8; 4], usize, usize)) {
        while at + 8 <= end {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().unwrap();
            let (body, next) = match size {
                0 => (at + 8, end),
                1 if at + 16 <= end => {
                    let large = u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap());
                    (at + 16, (at + large as usize).min(end))
                }
                _ if size >= 8 => (at + 8, (at + size).min(end)),
                _ => return,
            };
            each(at, &kind, body, next);
            if next <= at {
                return;
            }
            at = next;
        }
    }

    // moov -> trak -> mdia -> hdlr, whose handler type sits four bytes into
    // the full box's payload.
    let mut struck = None;
    let end = bytes.len();
    boxes(bytes, 0, end, |at, kind, body, next| {
        if kind != b"moov" {
            return;
        }
        boxes(bytes, body, next, |trak_at, kind, body, next| {
            if kind != b"trak" {
                return;
            }
            boxes(bytes, body, next, |_, kind, body, next| {
                if kind != b"mdia" {
                    return;
                }
                boxes(bytes, body, next, |_, kind, body, _| {
                    if kind == b"hdlr" && body + 12 <= bytes.len() && &bytes[body + 8..body + 12] == b"vide" {
                        struck = Some(trak_at);
                    }
                });
            });
        });
        let _ = at;
    });
    let Some(at) = struck else { return false };
    bytes[at + 4..at + 8].copy_from_slice(b"free");
    true
}

/// The precondition everything else rests on: this encoder and this decoder
/// agree about a soundtrack on THIS machine. If it fails, the fixtures are
/// the problem and the door below is not on trial.
#[test]
fn the_encoder_writes_a_soundtrack_this_decoder_can_read_back() {
    let dir = dir("readback");
    let path = dir.join("both.mp4");
    encode_clip(&path, true);
    let mut decoder = VideoFileDecoder::open(path.to_str().unwrap()).expect("open");
    assert!(decoder.info().has_audio, "the clip carries a soundtrack");
    assert_eq!(decoder.info().audio_sample_rate, AUDIO_RATE);
    let mut chunks = 0;
    while let Some(_chunk) = decoder.next_audio().expect("next audio") {
        chunks += 1;
    }
    assert!(chunks > 0, "and the decoder reads it back");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_container_with_no_picture_opens_for_its_sound() {
    let dir = dir("sound-only");
    let source = dir.join("both-for-surgery.mp4");
    encode_clip(&source, true);
    let mut bytes = std::fs::read(&source).expect("read the clip back");
    let length = bytes.len();
    assert!(strike_the_picture(&mut bytes), "the clip had a picture track to strike");
    assert_eq!(bytes.len(), length, "the surgery moves nothing");
    let struck = dir.join("sound-only.mp4");
    std::fs::write(&struck, &bytes).expect("write the struck clip");

    let mut decoder =
        VideoFileDecoder::open_audio(struck.to_str().unwrap()).expect("open for sound");
    {
        let info = decoder.info();
        assert!(info.has_audio, "the sound is there");
        assert_eq!(info.audio_sample_rate, AUDIO_RATE);
        assert_eq!(info.audio_channels, AUDIO_CHANNELS);
        assert_eq!((info.width, info.height), (0, 0), "no picture was negotiated");
        assert!(info.video_codec.is_none(), "and none was named");
    }
    let mut frames = 0usize;
    let mut chunks = 0usize;
    while let Some(chunk) = decoder.next_audio().expect("next audio") {
        assert_eq!(chunk.sample_rate, AUDIO_RATE);
        frames += chunk.samples.len() / chunk.channels.max(1) as usize;
        chunks += 1;
    }
    assert!(chunks > 0, "the sound came back");
    assert!(
        (40_000..=60_000).contains(&frames),
        "about a second of it, head and tail padding allowed: {frames}",
    );
    assert!(
        decoder.next_frame().expect("next frame is not an error").is_none(),
        "and asking for a picture is end of stream, not a failure",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The picture door keeps its own refusal. Without this pin the two doors
/// could be folded into one and a file with no picture would quietly reach a
/// slot player that has nothing to draw.
#[test]
fn the_picture_opener_still_refuses_a_file_with_no_picture() {
    let dir = dir("refusal");
    let source = dir.join("both-for-refusal.mp4");
    encode_clip(&source, true);
    let mut bytes = std::fs::read(&source).expect("read the clip back");
    assert!(strike_the_picture(&mut bytes), "the clip had a picture track to strike");
    let struck = dir.join("sound-only-refusal.mp4");
    std::fs::write(&struck, &bytes).expect("write the struck clip");

    let opened = VideoFileDecoder::open(struck.to_str().unwrap());
    match opened {
        Ok(_) => panic!("the picture door opened a file with no picture"),
        Err(error) => assert!(
            format!("{error}").contains("no video stream in file"),
            "and says why: {error}",
        ),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_for_sound_ignores_a_picture_that_is_there() {
    let dir = dir("with-picture");
    let path = dir.join("both-for-sound.mp4");
    encode_clip(&path, true);
    let with_picture = VideoFileDecoder::open(path.to_str().unwrap()).expect("open");
    assert_eq!(with_picture.info().width, WIDTH, "the picture door sees it");
    assert!(with_picture.info().has_audio);
    drop(with_picture);

    let mut sound = VideoFileDecoder::open_audio(path.to_str().unwrap()).expect("open for sound");
    assert_eq!(sound.info().width, 0, "the sound door does not");
    assert!(sound.info().video_codec.is_none());
    assert!(sound.info().has_audio);
    let mut chunks = 0;
    while let Some(chunk) = sound.next_audio().expect("next audio") {
        assert_eq!(chunk.sample_rate, AUDIO_RATE);
        chunks += 1;
    }
    assert!(chunks > 0, "and reads the sound of a file that has both");
    assert!(sound.next_frame().expect("not an error").is_none());
    // An observation rather than a pin: nothing here can make the decoder
    // re-read its picture type, so this cannot be relied on to catch that
    // guard going away.
    assert_eq!(sound.info().width, 0, "still no picture after the sound ran out");
    let _ = std::fs::remove_dir_all(&dir);
}
