//! The `mkfl` box: one motion payload format, written by every producer and
//! read by every player.
//!
//! The box rides INSIDE the mp4 as a trailing top-level box, which every
//! decoder skips, so a flow-carrying clip stays ONE plain playable video and
//! a flow-aware player re-scans the same file it plays.
//!
//! ## Payload version 1
//!
//! ```text
//! magic   b"MKFL"            (4)
//! version u16 LE = 1         flags u16 LE = 0
//! pairs   u32 LE             final consecutive-frame pairs
//! grid_w  u16 LE  grid_h u16 LE   flow grid dims (quarter source res)
//! vid_w   u16 LE  vid_h  u16 LE   final video dims
//! fps_num u32 LE  fps_den u32 LE  final video rate
//! then pairs x grid_w*grid_h x 5 planar bytes:
//!   f0x, f0y, f1x, f1y  (i8, quarter-pixel units at grid resolution)
//!   mask                (u8, 0 = frame1, 255 = frame0)
//! ```
//!
//! Playback contract: the field is computed at t=0.5. For an intermediate at
//! fractional `t` between the pair's frames, scale the stored vectors
//! linearly — `flow0(t) = flow0_half·(t/0.5)`, `flow1(t) =
//! flow1_half·((1-t)/0.5)` — warp both neighbours and fuse by `mask` and `t`.
//!
//! Units, said once: a stored i8 is a quarter of a GRID pixel, and the grid
//! is a quarter of the video, so one unit is exactly one source pixel and the
//! representable range is ±127 source pixels of displacement.

/// Payload version this crate writes and reads.
pub const PAYLOAD_VERSION: u16 = 1;
/// Bytes before the planar samples.
pub const HEADER_LEN: usize = 28;
/// Planes per pair: f0x, f0y, f1x, f1y, mask.
pub const PLANES: usize = 5;

/// Appends the flow payload as one top-level `mkfl` box. MP4 is a plain box
/// sequence, so `[u32 BE size]["mkfl"][payload]` after the last box is
/// spec-legal and skipped by decoders (the same mechanism appended XMP uses).
pub fn append_mkfl_box(mp4: &mut Vec<u8>, payload: &[u8]) {
    let size = 8u64 + payload.len() as u64;
    // The 32-bit box size covers every realistic flow payload; a >4GB
    // sidecar would be a bug upstream.
    mp4.extend_from_slice(&(size as u32).to_be_bytes());
    mp4.extend_from_slice(b"mkfl");
    mp4.extend_from_slice(payload);
}

/// The bytes of the box itself, for a producer that appends to a FILE rather
/// than to a buffer it already holds (re-reading a 200 MB all-intra clip just
/// to push 8 bytes in front of the payload is work nobody needs).
pub fn mkfl_box_bytes(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    append_mkfl_box(&mut out, payload);
    out
}

/// Extracts an `mkfl` payload from an mp4 (players use this; also the test
/// oracle for [`append_mkfl_box`]). Scans top-level boxes only.
///
/// Both size forms the container spec allows are walked, because both turn up
/// in files this repo writes: the 32-bit size, and the `size == 1` LARGESIZE
/// form with a 64-bit length after the type — AVFoundation reserves that
/// header for `mdat` when it starts writing and cannot know the final length,
/// so every mp4 the macOS encoder produces has one. A walker that stops there
/// finds no box, and the clip silently plays as plain video.
pub fn find_mkfl_box(mp4: &[u8]) -> Option<&[u8]> {
    let mut at = 0usize;
    while at + 8 <= mp4.len() {
        let declared =
            u32::from_be_bytes([mp4[at], mp4[at + 1], mp4[at + 2], mp4[at + 3]]) as u64;
        let kind = &mp4[at + 4..at + 8];
        // 0 = "to the end of the file", legal for the last box only.
        let (size, header) = match declared {
            0 => ((mp4.len() - at) as u64, 8usize),
            1 => {
                let bytes: [u8; 8] = mp4.get(at + 8..at + 16)?.try_into().ok()?;
                (u64::from_be_bytes(bytes), 16usize)
            }
            other => (other, 8usize),
        };
        if size < header as u64 || size > (mp4.len() - at) as u64 {
            return None;
        }
        if kind == b"mkfl" {
            let end = at.checked_add(size as usize)?;
            return mp4.get(at + header..end);
        }
        at = at.checked_add(size as usize)?;
    }
    None
}

/// Serializes the version-1 flow payload header + planar samples.
#[allow(clippy::too_many_arguments)]
pub fn encode_flow_payload(
    pairs: u32,
    grid_w: u16,
    grid_h: u16,
    vid_w: u16,
    vid_h: u16,
    fps_num: u32,
    fps_den: u32,
    samples: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + samples.len());
    out.extend_from_slice(b"MKFL");
    out.extend_from_slice(&PAYLOAD_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&pairs.to_le_bytes());
    out.extend_from_slice(&grid_w.to_le_bytes());
    out.extend_from_slice(&grid_h.to_le_bytes());
    out.extend_from_slice(&vid_w.to_le_bytes());
    out.extend_from_slice(&vid_h.to_le_bytes());
    out.extend_from_slice(&fps_num.to_le_bytes());
    out.extend_from_slice(&fps_den.to_le_bytes());
    out.extend_from_slice(samples);
    out
}

/// The header a payload declares. Producers use it to check their own work;
/// the players parse the same fields themselves (see `apps/vj/src/flow.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadHeader {
    pub pairs: u32,
    pub grid_w: u16,
    pub grid_h: u16,
    pub vid_w: u16,
    pub vid_h: u16,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl PayloadHeader {
    pub fn pair_stride(&self) -> usize {
        self.grid_w as usize * self.grid_h as usize * PLANES
    }
}

/// Reads a payload's header, refusing anything a player would refuse: a bad
/// magic, a future version, a zero grid, or a sample count that does not
/// match the declared geometry. A truncated payload must never become a
/// half-usable map.
pub fn parse_flow_payload(payload: &[u8]) -> Option<(PayloadHeader, &[u8])> {
    if payload.len() < HEADER_LEN || &payload[..4] != b"MKFL" {
        return None;
    }
    let u16at = |at: usize| u16::from_le_bytes([payload[at], payload[at + 1]]);
    let u32at = |at: usize| {
        u32::from_le_bytes([payload[at], payload[at + 1], payload[at + 2], payload[at + 3]])
    };
    if u16at(4) != PAYLOAD_VERSION {
        return None;
    }
    let header = PayloadHeader {
        pairs: u32at(8),
        grid_w: u16at(12),
        grid_h: u16at(14),
        vid_w: u16at(16),
        vid_h: u16at(18),
        fps_num: u32at(20),
        fps_den: u32at(24),
    };
    let samples = &payload[HEADER_LEN..];
    if header.grid_w == 0
        || header.grid_h == 0
        || samples.len() != header.pairs as usize * header.pair_stride()
    {
        return None;
    }
    Some((header, samples))
}

/// Quantizes one pair's flow field into the payload's planar bytes: flow in
/// quarter-pixel i8 at grid resolution, mask in u8. `flow`/`mask` are the
/// planar f32 fields at `(w, h)` in SOURCE pixels; the grid is the 4:1 box
/// average. This is the shape a per-pixel estimator (RIFE) produces.
pub fn quantize_flow_pair(
    flow: &[f32],
    mask: &[f32],
    w: usize,
    h: usize,
    grid_w: usize,
    grid_h: usize,
) -> Vec<u8> {
    let plane = w * h;
    let grid_plane = grid_w * grid_h;
    let mut out = vec![0u8; grid_plane * PLANES];
    let step_x = w as f32 / grid_w as f32;
    let step_y = h as f32 / grid_h as f32;
    for gy in 0..grid_h {
        for gx in 0..grid_w {
            let x0 = (gx as f32 * step_x) as usize;
            let x1 = (((gx + 1) as f32 * step_x) as usize).clamp(x0 + 1, w);
            let y0 = (gy as f32 * step_y) as usize;
            let y1 = (((gy + 1) as f32 * step_y) as usize).clamp(y0 + 1, h);
            let count = ((x1 - x0) * (y1 - y0)) as f32;
            let mut sums = [0.0f32; PLANES];
            for y in y0..y1 {
                for x in x0..x1 {
                    let src = y * w + x;
                    for c in 0..4 {
                        sums[c] += flow[c * plane + src];
                    }
                    sums[4] += mask[src];
                }
            }
            let dst = gy * grid_w + gx;
            // Flow is stored at GRID resolution in quarter-pixel units, so
            // the vector must be scaled by the grid/source ratio too.
            for c in 0..4 {
                let scale = if c % 2 == 0 {
                    grid_w as f32 / w as f32
                } else {
                    grid_h as f32 / h as f32
                };
                let value = (sums[c] / count) * scale * 4.0;
                out[c * grid_plane + dst] = (value.round().clamp(-127.0, 127.0) as i8) as u8;
            }
            out[4 * grid_plane + dst] = ((sums[4] / count).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// The same quantization for a field that is already GRID-native — what the
/// classical estimator in this crate produces. `f0`/`f1` are per-cell vectors
/// in GRID pixels, `mask` is 0..1 with 1 = the intermediate takes frame0.
pub fn quantize_flow_grid(
    f0: &[[f32; 2]],
    f1: &[[f32; 2]],
    mask: &[f32],
    grid_w: usize,
    grid_h: usize,
) -> Vec<u8> {
    let grid_plane = grid_w * grid_h;
    assert_eq!(f0.len(), grid_plane, "f0 plane size");
    assert_eq!(f1.len(), grid_plane, "f1 plane size");
    assert_eq!(mask.len(), grid_plane, "mask plane size");
    let mut out = vec![0u8; grid_plane * PLANES];
    let q = |v: f32| ((v * 4.0).round().clamp(-127.0, 127.0) as i8) as u8;
    for i in 0..grid_plane {
        out[i] = q(f0[i][0]);
        out[grid_plane + i] = q(f0[i][1]);
        out[2 * grid_plane + i] = q(f1[i][0]);
        out[3 * grid_plane + i] = q(f1[i][1]);
        out[4 * grid_plane + i] = (mask[i].clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The append-to-a-file form writes exactly the same bytes.
        assert_eq!(mkfl_box_bytes(&payload), mp4[24..].to_vec());
    }

    #[test]
    fn the_walk_survives_a_64_bit_mdat() {
        // What AVFoundation actually writes: ftyp, then an mdat whose 32-bit
        // size is the escape value 1 and whose real length follows the type.
        let payload = encode_flow_payload(1, 2, 2, 64, 64, 30, 1, &[3u8; 2 * 2 * PLANES]);
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&16u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom\0\0\0\0");
        let media = vec![0u8; 40];
        mp4.extend_from_slice(&1u32.to_be_bytes());
        mp4.extend_from_slice(b"mdat");
        mp4.extend_from_slice(&((16 + media.len()) as u64).to_be_bytes());
        mp4.extend_from_slice(&media);
        append_mkfl_box(&mut mp4, &payload);
        assert_eq!(find_mkfl_box(&mp4), Some(&payload[..]));
        // A box claiming more than the file holds is a corrupt walk, not a
        // reason to read past the end.
        let mut lying = mp4.clone();
        lying[0..4].copy_from_slice(&(mp4.len() as u32 + 64).to_be_bytes());
        assert!(find_mkfl_box(&lying).is_none());
    }

    #[test]
    fn the_header_parses_back_and_fails_closed() {
        let stride = 2 * 3 * PLANES;
        let samples: Vec<u8> = (0..(2 * stride) as u32).map(|v| v as u8).collect();
        let payload = encode_flow_payload(2, 2, 3, 640, 360, 30000, 1001, &samples);
        let (header, got) = parse_flow_payload(&payload).expect("parses");
        assert_eq!(
            header,
            PayloadHeader {
                pairs: 2,
                grid_w: 2,
                grid_h: 3,
                vid_w: 640,
                vid_h: 360,
                fps_num: 30000,
                fps_den: 1001,
            }
        );
        assert_eq!(got, &samples[..]);
        assert_eq!(header.pair_stride(), stride);
        // Truncated samples, a zero grid and a future version all refuse.
        let short = encode_flow_payload(2, 2, 3, 640, 360, 30, 1, &samples[..stride]);
        assert!(parse_flow_payload(&short).is_none());
        assert!(parse_flow_payload(&encode_flow_payload(0, 0, 3, 1, 1, 30, 1, &[])).is_none());
        let mut future = payload.clone();
        future[4] = 2;
        assert!(parse_flow_payload(&future).is_none());
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
        assert_eq!(out.len(), grid_plane * PLANES);
        assert!(out[..grid_plane].iter().all(|&b| b as i8 == 16));
        assert!(out[grid_plane..2 * grid_plane].iter().all(|&b| b as i8 == 0));
        assert!(out[4 * grid_plane..].iter().all(|&b| b == 191 || b == 192));
    }

    #[test]
    fn the_grid_native_quantizer_agrees_with_the_per_pixel_one() {
        // Same field expressed both ways: 8 source px right = 4 grid px on a
        // half-width grid. Byte-for-byte identical output is the point — one
        // format, two producers.
        let (w, h, gw, gh) = (8usize, 4usize, 4usize, 2usize);
        let plane = w * h;
        let grid_plane = gw * gh;
        let mut flow = vec![0.0f32; 4 * plane];
        flow[..plane].fill(8.0);
        flow[3 * plane..].fill(-2.0); // f1y, source px
        let mask = vec![0.75f32; plane];
        let per_pixel = quantize_flow_pair(&flow, &mask, w, h, gw, gh);
        // grid px = source px * grid/source: x scales by 1/2, y by 1/2.
        let f0 = vec![[4.0f32, 0.0]; grid_plane];
        let f1 = vec![[0.0f32, -1.0]; grid_plane];
        let grid_native = quantize_flow_grid(&f0, &f1, &vec![0.75f32; grid_plane], gw, gh);
        assert_eq!(per_pixel, grid_native);
    }
}
