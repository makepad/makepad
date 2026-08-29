//! The picture a tween pair is made of, and the small CPU helpers the
//! producers need on top of it.
//!
//! Lifted verbatim out of the VJ's `media.rs` when the tweener became a
//! library — the VJ re-exports these names, so its own call sites (and
//! its behaviour) never moved.

use makepad_widgets::makepad_platform::video_file::nv12;

pub struct Frame {
    /// Pacing timestamp — monotonic for the clock (synthetic in bounce).
    pub pts_100ns: i64,
    /// TRUE clip position of this picture, for the position readout: in
    /// bounce the pacing stamps climb forever while the picture runs
    /// backward — the scrub bar follows this, never the pacing stamp.
    pub clip_100ns: i64,
    pub px: Pixels,
}

/// A frame's pixel payload. NV12 is the RESIDENT form — 1.5 bytes per
/// pixel STRAIGHT OFF THE DECODER, untouched by the CPU anywhere in the
/// pipeline; the GPU unpacks it to RGBA in a texture-to-texture pass at
/// present time (the operator's law: never convert 4K in a software
/// loop). Bgra remains for producers that only have RGBA.
#[derive(Clone)]
pub enum Pixels {
    Bgra(Vec<u32>),
    Nv12 { data: Vec<u8>, width: u32, height: u32 },
}

impl Pixels {
    pub fn byte_len(&self) -> usize {
        match self {
            Pixels::Bgra(v) => v.len() * 4,
            Pixels::Nv12 { data, .. } => data.len(),
        }
    }

    /// CPU view of the picture as packed BGRA words — tests and the few
    /// point-sampling consumers; the presentation path never calls this.
    pub fn to_bgra(&self) -> Vec<u32> {
        match self {
            Pixels::Bgra(v) => v.clone(),
            Pixels::Nv12 { data, width, height } => {
                let mut out = Vec::new();
                nv12::nv12_to_bgra_u32(data, *width, *height, &mut out);
                out
            }
        }
    }
}

impl Default for Pixels {
    fn default() -> Self {
        Pixels::Bgra(Vec::new())
    }
}

/// Sparse mean-abs luma difference between two NV12 frames (0..255): the
/// scene-cut score. A 24x14 grid is plenty — a hard cut moves most of the
/// picture, a pan moves edges.
pub fn nv12_cut_score(a: &[u8], b: &[u8], w: usize, h: usize) -> f32 {
    if w == 0 || h == 0 || a.len() < w * h || b.len() < w * h {
        return 0.0;
    }
    let (gw, gh) = (24usize, 14usize);
    let mut sum = 0.0f32;
    for gy in 0..gh {
        let y = (gy * h + h / 2) / gh;
        for gx in 0..gw {
            let x = (gx * w + w / 2) / gw;
            let at = y.min(h - 1) * w + x.min(w - 1);
            sum += (a[at] as f32 - b[at] as f32).abs();
        }
    }
    sum / (gw * gh) as f32
}

/// Tightly packed RGB8 proxy of an NV12 frame at a reduced resolution —
/// the RIFE worker's input format. Nearest-sampled, BT.709 limited.
pub fn nv12_proxy_rgb8(data: &[u8], w: usize, h: usize, pw: usize, ph: usize) -> Vec<u8> {
    let mut out = vec![0u8; pw * ph * 3];
    if w == 0 || h == 0 || data.len() < w * h * 3 / 2 {
        return out;
    }
    let (y_plane, uv_plane) = data.split_at(w * h);
    for py in 0..ph {
        let sy = ((py * h + h / 2) / ph.max(1)).min(h - 1);
        let uv_row = &uv_plane[(sy / 2) * w..];
        for px_i in 0..pw {
            let sx = ((px_i * w + w / 2) / pw.max(1)).min(w - 1);
            let c = y_plane[sy * w + sx] as i32 - 16;
            let d = uv_row[(sx / 2) * 2] as i32 - 128;
            let e = uv_row[(sx / 2) * 2 + 1] as i32 - 128;
            let at = (py * pw + px_i) * 3;
            out[at] = ((298 * c + 459 * e + 128) >> 8).clamp(0, 255) as u8;
            out[at + 1] = ((298 * c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u8;
            out[at + 2] = ((298 * c + 541 * d + 128) >> 8).clamp(0, 255) as u8;
        }
    }
    out
}

/// Nearest-sampled RGB8 -> RGB8 rescale: the same proxy step for callers
/// whose frames never were NV12 (a diffusion feed hands us RGB8 already).
pub fn rgb8_proxy(data: &[u8], w: usize, h: usize, pw: usize, ph: usize) -> Vec<u8> {
    let mut out = vec![0u8; pw * ph * 3];
    if w == 0 || h == 0 || data.len() < w * h * 3 {
        return out;
    }
    for py in 0..ph {
        let sy = ((py * h + h / 2) / ph.max(1)).min(h - 1);
        for px_i in 0..pw {
            let sx = ((px_i * w + w / 2) / pw.max(1)).min(w - 1);
            let src = (sy * w + sx) * 3;
            let at = (py * pw + px_i) * 3;
            out[at..at + 3].copy_from_slice(&data[src..src + 3]);
        }
    }
    out
}

/// Sparse mean-abs luma difference between two packed RGB8 frames — the
/// RGB twin of `nv12_cut_score`, on the same 0..255 scale so one cut
/// threshold serves both input forms.
pub fn rgb8_cut_score(a: &[u8], b: &[u8], w: usize, h: usize) -> f32 {
    if w == 0 || h == 0 || a.len() < w * h * 3 || b.len() < w * h * 3 {
        return 0.0;
    }
    let (gw, gh) = (24usize, 14usize);
    let luma = |px: &[u8], at: usize| -> f32 {
        16.0 + 219.0
            * (px[at] as f32 * 0.2126 + px[at + 1] as f32 * 0.7152 + px[at + 2] as f32 * 0.0722)
            / 255.0
    };
    let mut sum = 0.0f32;
    for gy in 0..gh {
        let y = ((gy * h + h / 2) / gh).min(h - 1);
        for gx in 0..gw {
            let x = ((gx * w + w / 2) / gw).min(w - 1);
            let at = (y * w + x) * 3;
            sum += (luma(a, at) - luma(b, at)).abs();
        }
    }
    sum / (gw * gh) as f32
}

/// Timeline tracing, on when the host app asks for it. `VJ_TL` is the
/// VJ's own switch and stays honoured verbatim; `FRAMETWEEN_TL` is the
/// library name for every other host.
pub fn tl_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("VJ_TL").is_some() || std::env::var_os("FRAMETWEEN_TL").is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb8_proxy_keeps_a_moving_edge_where_it_was() {
        // A vertical edge at 40% across a 100-wide frame survives the
        // rescale to 25 wide at the same fraction.
        let (w, h) = (100usize, 8usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 40..w {
                let at = (y * w + x) * 3;
                rgb[at..at + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let small = rgb8_proxy(&rgb, w, h, 25, 4);
        let edge = (0..25).find(|x| small[(1 * 25 + x) * 3] > 128).unwrap();
        assert_eq!(edge, 10, "40% of 25 columns");
    }

    #[test]
    fn a_cut_scores_far_above_a_nudge() {
        let (w, h) = (64usize, 32usize);
        let black = vec![0u8; w * h * 3];
        let white = vec![255u8; w * h * 3];
        let mut nudged = black.clone();
        for px in nudged.iter_mut().take(w * 3) {
            *px = 255;
        }
        assert!(rgb8_cut_score(&black, &white, w, h) > 200.0);
        assert!(rgb8_cut_score(&black, &nudged, w, h) < 28.0);
    }
}
