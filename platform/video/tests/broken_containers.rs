#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use makepad_video::{VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions};
use std::path::{Path, PathBuf};

const MP4_FTYP: &[u8; 24] = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2";
const MOV_FTYP: &[u8; 20] = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";

struct Fixtures(PathBuf);

impl Fixtures {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("makepad-video-broken-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn headers(&self) -> [PathBuf; 3] {
        [
            self.write("header.mp4", MP4_FTYP),
            self.write("padded.mp4", &[MP4_FTYP.as_slice(), &[0; 8]].concat()),
            self.write("padded.mov", &[MOV_FTYP.as_slice(), &[0; 8]].concat()),
        ]
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_video_refused(path: &Path) {
    let path = path.to_str().unwrap();
    match VideoFileDecoder::open(path) {
        Ok(_) => panic!("video open accepted {path}"),
        Err(error) => assert!(!error.to_string().is_empty(), "missing video error for {path}"),
    }
}

#[test]
fn header_only_mp4_and_mov_refuse_without_stalling() {
    let fixtures = Fixtures::new("refusal");
    let truncated_moov = [MP4_FTYP.as_slice(), b"\0\0\x03\xe8moov\0\0\0\0"].concat();
    let empty_moov = [MP4_FTYP.as_slice(), b"\0\0\0\x10moov\0\0\0\x08free"].concat();
    let other = [
        fixtures.write("empty.mp4", &[]),
        fixtures.write("plain.txt", b"plain text"),
        fixtures.write("truncated.mp4", &truncated_moov),
        fixtures.write("empty_moov.mp4", &empty_moov),
    ];
    for path in fixtures.headers().iter().chain(&other) {
        assert_video_refused(path);
        assert!(VideoFileDecoder::open_audio(path.to_str().unwrap()).is_err(), "audio open accepted {path:?}");
    }
}

#[test]
fn repeated_and_concurrent_broken_opens_never_stall_and_leave_valid_opens_healthy() {
    let fixtures = Fixtures::new("concurrent");
    let headers = fixtures.headers();
    let path = fixtures.0.join("valid.mp4");
    let valid = path.to_str().unwrap();
    let (width, height) = (64, 48);
    let mut encoder = VideoFileEncoder::new(valid, VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width,
        height,
        ..Default::default()
    }).expect("create H.264 encoder");
    for grey in [96, 160] {
        encoder.push_frame_rgb8(&vec![grey; (width * height * 3) as usize], None).expect("encode frame");
    }
    encoder.finish().expect("finish two-frame clip");

    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for round in 0..4 {
                    for header in &headers {
                        assert_video_refused(header);
                    }
                    if round % 2 == 1 {
                        let mut decoder = VideoFileDecoder::open(valid).expect("open the valid clip");
                        assert_eq!((decoder.info().width, decoder.info().height), (width, height));
                        assert!(decoder.next_frame().expect("decode the valid clip").is_some(), "missing first frame");
                    }
                }
            });
        }
    });
}
