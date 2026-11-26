//! The video codecs a call encodes and decodes with, behind two small traits: the platform's
//! hardware codecs for a real call, and a test codec that needs no device for tests on any box.

use crate::guard::Rung;
use std::sync::Arc;

/// One encoded access unit.
pub struct Packet {
    pub data: Vec<u8>,
    pub key: bool,
    pub pts_100ns: i64,
}

/// One decoded picture, 4:2:0 (NV12) at its own size.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub pts_100ns: i64,
}

pub trait Encoder: Send {
    /// One picture of the rung's size; returns what the encoder has finished.
    fn push(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<Packet>, String>;
    fn request_key(&mut self);
    fn set_kbps(&mut self, kbps: u32);
    fn name(&self) -> String;
}

pub trait Decoder: Send {
    fn push(&mut self, au: &[u8], pts_100ns: i64) -> Result<Vec<Picture>, String>;
    fn name(&self) -> String;
    /// A key frame arrived: false when it starts a stream this decoder can't continue (a new
    /// size), so the caller opens a new one. The first call records the stream.
    fn accepts(&mut self, _key_au: &[u8]) -> bool {
        true
    }
}

pub trait Codecs: Send + Sync {
    fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String>;
    fn decoder(&self) -> Result<Box<dyn Decoder>, String>;
}

/// The platform's H.264 codecs, low latency, as the call probe measured them.
pub fn platform_codecs() -> Arc<dyn Codecs> {
    Arc::new(PlatformCodecs)
}

struct PlatformCodecs;

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod platform {
    use super::*;
    use makepad_video::{StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions};

    pub(super) struct PlatformEncoder(pub VideoStreamEncoder);
    pub(super) struct PlatformDecoder {
        pub decoder: VideoStreamDecoder,
        /// The stream's sequence parameters, from its first key frame.
        pub sps: Vec<u8>,
    }

    impl Encoder for PlatformEncoder {
        fn push(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<Packet>, String> {
            let packets = self.0.push_frame_nv12(nv12, pts_100ns).map_err(|e| format!("{e:?}"))?;
            Ok(packets.into_iter().map(|p| Packet { data: p.data, key: p.is_key, pts_100ns: p.pts_100ns }).collect())
        }
        fn request_key(&mut self) {
            self.0.request_keyframe();
        }
        fn set_kbps(&mut self, kbps: u32) {
            self.0.set_bitrate_kbps(kbps);
        }
        fn name(&self) -> String {
            self.0.description()
        }
    }

    impl Decoder for PlatformDecoder {
        fn push(&mut self, au: &[u8], pts_100ns: i64) -> Result<Vec<Picture>, String> {
            let pictures = self.decoder.push_packet(au, pts_100ns).map_err(|e| format!("{e:?}"))?;
            Ok(pictures.into_iter().map(|p| Picture { width: p.width, height: p.height, nv12: p.nv12, pts_100ns: p.pts_100ns }).collect())
        }
        fn name(&self) -> String {
            self.decoder.name().unwrap_or_default()
        }
        fn accepts(&mut self, key_au: &[u8]) -> bool {
            use makepad_video::annex_b::{nal_unit_type, split_annex_b, NAL_TYPE_SPS};
            let Some(sps) = split_annex_b(key_au).into_iter().find(|nal| nal_unit_type(nal) == NAL_TYPE_SPS) else { return true };
            if self.sps.is_empty() {
                self.sps.extend_from_slice(sps);
                return true;
            }
            self.sps.as_slice() == sps
        }
    }

    pub(super) fn encoder(rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> {
        let options = VideoStreamEncoderOptions {
            codec: StreamVideoCodec::H264,
            width: rung.width,
            height: rung.height,
            fps: rung.fps,
            bitrate_kbps: kbps,
            keyint: 0,
            low_latency: true,
            full_chroma: false,
        };
        VideoStreamEncoder::new(options).map(|e| Box::new(PlatformEncoder(e)) as Box<dyn Encoder>).map_err(|e| format!("{e:?}"))
    }

    pub(super) fn decoder() -> Result<Box<dyn Decoder>, String> {
        VideoStreamDecoder::new_low_latency(StreamVideoCodec::H264)
            .map(|decoder| Box::new(PlatformDecoder { decoder, sps: Vec::new() }) as Box<dyn Decoder>)
            .map_err(|e| format!("{e:?}"))
    }
}

impl Codecs for PlatformCodecs {
    fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> {
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        return platform::encoder(rung, kbps);
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        return Err(format!("no video encoder on this platform for {rung} at {kbps} kbit/s"));
    }

    fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        return platform::decoder();
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        return Err("no video decoder on this platform".into());
    }
}

/// A codec for tests: no device, any box. Its access unit is a 24-byte header (a marker, key,
/// width, height, frame number, pts) padded to the rung's per-picture budget at the asked rate
/// (key frames four times that), and it decodes to a picture of the right size whose first eight
/// bytes are the frame number. A test can check order, keys and sizes.
pub struct TestCodecs;

const TEST_MARK: u8 = b'T';

struct TestEncoder {
    rung: Rung,
    kbps: u32,
    frame: u32,
    key_due: bool,
}

struct TestDecoder;

impl Codecs for TestCodecs {
    fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> {
        Ok(Box::new(TestEncoder { rung, kbps, frame: 0, key_due: true }))
    }
    fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
        Ok(Box::new(TestDecoder))
    }
}

impl Encoder for TestEncoder {
    fn push(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<Packet>, String> {
        let (w, h) = (self.rung.width as usize, self.rung.height as usize);
        if nv12.len() != w * h * 3 / 2 {
            return Err(format!("a {}x{} picture needs {} bytes, got {}", w, h, w * h * 3 / 2, nv12.len()));
        }
        let key = std::mem::take(&mut self.key_due);
        let budget = (u64::from(self.kbps) * 1000 / 8 / u64::from(self.rung.fps.max(1))) as usize * if key { 4 } else { 1 };
        let mut data = Vec::with_capacity(budget.max(24));
        data.push(TEST_MARK);
        data.push(key as u8);
        data.extend_from_slice(&(self.rung.width as u16).to_be_bytes());
        data.extend_from_slice(&(self.rung.height as u16).to_be_bytes());
        data.extend_from_slice(&self.frame.to_be_bytes());
        data.extend_from_slice(&[0; 6]);
        data.extend_from_slice(&pts_100ns.to_be_bytes());
        data.resize(budget.max(24), 0x5a);
        self.frame += 1;
        Ok(vec![Packet { data, key, pts_100ns }])
    }
    fn request_key(&mut self) {
        self.key_due = true;
    }
    fn set_kbps(&mut self, kbps: u32) {
        self.kbps = kbps;
    }
    fn name(&self) -> String {
        "test codec".into()
    }
}

impl Decoder for TestDecoder {
    fn push(&mut self, au: &[u8], pts_100ns: i64) -> Result<Vec<Picture>, String> {
        if au.len() < 24 || au[0] != TEST_MARK {
            return Err("not a test codec access unit".into());
        }
        let (w, h) = (u16::from_be_bytes([au[2], au[3]]) as u32, u16::from_be_bytes([au[4], au[5]]) as u32);
        let mut nv12 = vec![128u8; (w * h * 3 / 2) as usize];
        nv12[..8].copy_from_slice(&u64::from(u32::from_be_bytes(au[6..10].try_into().unwrap())).to_be_bytes());
        Ok(vec![Picture { width: w, height: h, nv12, pts_100ns }])
    }
    fn name(&self) -> String {
        "test codec".into()
    }
}
