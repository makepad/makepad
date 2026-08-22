//! Reader for the `mkfl` motion-vector box its two producers append to their
//! mp4s: the enhance service (RIFE fields, see
//! libs/asset/ai/src/enhance_backend.rs) and this app's own import converter
//! (classical fields, see libs/video_flow). The box rides INSIDE the mp4 — a
//! trailing top-level box every decoder skips — so a flow-aware player just
//! re-scans the same file it plays.
//!
//! The box walk and the header parse are the SHARED ones
//! (`makepad_video_flow::payload`), so a file this machine writes is a file
//! this player reads — including the 64-bit `largesize` `mdat` AVFoundation
//! puts in every mp4 it produces.
//!
//! Payload v1, all little-endian after the 4CC:
//!
//! ```text
//! magic   b"MKFL"
//! version u16 = 1   flags u16 = 0
//! pairs   u32                      final consecutive-frame pairs
//! grid_w  u16   grid_h u16        flow grid dims (quarter source res)
//! vid_w   u16   vid_h  u16        final video dims
//! fps_num u32   fps_den u32       final video rate
//! pairs × grid_w*grid_h × 5 planar bytes:
//!   f0x f0y f1x f1y  (i8, quarter-pixel units at GRID resolution)
//!   mask             (u8, 255 = the intermediate takes frame0's warp)
//! ```
//!
//! Playback contract: the field was computed at t=0.5. For an intermediate
//! at fractional `t` between pair frames (i, i+1), scale the stored vectors
//! linearly — `flow0(t) ≈ flow0_half * (t / 0.5)`, `flow1(t) ≈ flow1_half *
//! ((1 - t) / 0.5)` — warp both neighbours and blend by `mask` (and `t`).

use std::sync::Arc;

/// One clip's parsed motion payload. `samples` stays a single shared
/// allocation; per-pair views are computed slices.
#[derive(Clone)]
pub struct FlowMap {
    pub pairs: u32,
    pub grid_w: u16,
    pub grid_h: u16,
    pub vid_w: u16,
    pub vid_h: u16,
    pub fps_num: u32,
    pub fps_den: u32,
    samples: Arc<[u8]>,
}

impl FlowMap {
    /// Bytes of one pair's planar samples: 4 planes of i8 flow then 1 plane
    /// of u8 mask, each `grid_w * grid_h` long. `None` past the end.
    pub fn pair(&self, index: u32) -> Option<&[u8]> {
        if index >= self.pairs {
            return None;
        }
        let stride = self.pair_stride();
        let at = index as usize * stride;
        self.samples.get(at..at + stride)
    }

    pub fn pair_stride(&self) -> usize {
        self.grid_w as usize * self.grid_h as usize * 5
    }
}

/// Scans top-level mp4 boxes for a `mkfl` payload and parses it. `None` for
/// a clip without motion vectors, a malformed box walk, or a payload whose
/// declared geometry does not match its byte count — a truncated payload
/// must never become a half-usable map.
pub fn parse_mkfl(mp4: &[u8]) -> Option<FlowMap> {
    let payload = makepad_video_flow::find_mkfl_box(mp4)?;
    let (header, samples) = makepad_video_flow::parse_flow_payload(payload)?;
    Some(FlowMap {
        pairs: header.pairs,
        grid_w: header.grid_w,
        grid_h: header.grid_h,
        vid_w: header.vid_w,
        vid_h: header.vid_h,
        fps_num: header.fps_num,
        fps_den: header.fps_den,
        samples: Arc::from(samples),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(pairs: u32, grid_w: u16, grid_h: u16, samples: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"MKFL");
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&pairs.to_le_bytes());
        out.extend_from_slice(&grid_w.to_le_bytes());
        out.extend_from_slice(&grid_h.to_le_bytes());
        out.extend_from_slice(&1728u16.to_le_bytes());
        out.extend_from_slice(&960u16.to_le_bytes());
        out.extend_from_slice(&48u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(samples);
        out
    }

    fn wrap(payload: &[u8]) -> Vec<u8> {
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&16u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom\0\0\0\0");
        mp4.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
        mp4.extend_from_slice(b"mkfl");
        mp4.extend_from_slice(payload);
        mp4
    }

    #[test]
    fn parses_a_wrapped_payload_and_slices_pairs() {
        let stride = 2usize * 2 * 5;
        let samples: Vec<u8> = (0..(3 * stride) as u32).map(|v| v as u8).collect();
        let mp4 = wrap(&payload(3, 2, 2, &samples));
        let map = parse_mkfl(&mp4).expect("parses");
        assert_eq!((map.pairs, map.grid_w, map.grid_h), (3, 2, 2));
        assert_eq!((map.vid_w, map.vid_h, map.fps_num, map.fps_den), (1728, 960, 48, 1));
        assert_eq!(map.pair(0).unwrap(), &samples[..stride]);
        assert_eq!(map.pair(2).unwrap(), &samples[2 * stride..]);
        assert!(map.pair(3).is_none());
    }

    #[test]
    fn refuses_plain_videos_truncation_and_future_versions() {
        // No mkfl box at all.
        let mut plain = Vec::new();
        plain.extend_from_slice(&16u32.to_be_bytes());
        plain.extend_from_slice(b"ftyp");
        plain.extend_from_slice(b"isom\0\0\0\0");
        assert!(parse_mkfl(&plain).is_none());
        // Truncated samples: declared 2 pairs, carried 1.
        let stride = 2usize * 2 * 5;
        let short: Vec<u8> = vec![0; stride];
        assert!(parse_mkfl(&wrap(&payload(2, 2, 2, &short))).is_none());
        // Future version fails closed.
        let mut future = payload(1, 2, 2, &vec![0; stride]);
        future[4] = 2;
        assert!(parse_mkfl(&wrap(&future)).is_none());
        // Zero grid fails closed.
        assert!(parse_mkfl(&wrap(&payload(0, 0, 2, &[]))).is_none());
    }

    #[test]
    fn what_the_shared_writer_produces_is_what_this_reader_takes() {
        // The import converter's own path, end to end in miniature: a
        // grid-native field, quantized and framed by makepad-video-flow, then
        // read back HERE. This is the byte-compatibility contract between the
        // two producers and this player, and it is the reason both sides use
        // one crate.
        let (gw, gh) = (4usize, 3usize);
        let n = gw * gh;
        let f0 = vec![[-1.25f32, 0.5]; n];
        let f1 = vec![[1.25f32, -0.5]; n];
        let mask = vec![0.5f32; n];
        let samples = makepad_video_flow::quantize_flow_grid(&f0, &f1, &mask, gw, gh);
        let payload = makepad_video_flow::encode_flow_payload(
            1, gw as u16, gh as u16, 640, 360, 30, 1, &samples,
        );
        // Framed after a 64-bit `largesize` mdat, which is what every mp4 the
        // macOS encoder writes actually looks like.
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&1u32.to_be_bytes());
        mp4.extend_from_slice(b"mdat");
        mp4.extend_from_slice(&24u64.to_be_bytes());
        mp4.extend_from_slice(&[0u8; 8]);
        makepad_video_flow::append_mkfl_box(&mut mp4, &payload);

        let map = parse_mkfl(&mp4).expect("the shared writer's box parses here");
        assert_eq!((map.pairs, map.grid_w, map.grid_h), (1, gw as u16, gh as u16));
        assert_eq!((map.vid_w, map.vid_h, map.fps_num, map.fps_den), (640, 360, 30, 1));
        let pair = map.pair(0).expect("pair 0");
        assert_eq!(pair.len(), map.pair_stride());
        // -1.25 grid px = -5 quarter-cell units; +0.5 = +2; mask 0.5 = 128.
        assert!(pair[..n].iter().all(|&b| b as i8 == -5));
        assert!(pair[n..2 * n].iter().all(|&b| b as i8 == 2));
        assert!(pair[2 * n..3 * n].iter().all(|&b| b as i8 == 5));
        assert!(pair[3 * n..4 * n].iter().all(|&b| b as i8 == -2));
        assert!(pair[4 * n..].iter().all(|&b| b == 128));
    }
}
