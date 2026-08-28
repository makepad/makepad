//! Closing a generated clip into a seamless loop.
//!
//! The video model cannot do this for us. H3 conditions on a FIRST frame
//! and nothing else: `VideoJob` carries `input_rgb` and there is no
//! end-frame, last-frame or loop field anywhere in the backend, the wire
//! protocol or the model crate. So an i2v clip begins exactly on the flux
//! still and ends wherever the motion got to — play it on a pad and the
//! wrap from the last frame back to the still is a visible jump cut.
//!
//! What closes it is the oldest trick there is, applied to the decoded
//! frames the repeat cache already holds: cross-fade the clip's TAIL onto
//! its HEAD and drop the tail. A clip of `n` frames becomes a loop of
//! `n - wrap`, whose last frame is followed — across the wrap — by a frame
//! that is mostly what used to follow it. Nothing is re-encoded, nothing is
//! republished, and the operator's clip on the server is untouched: this is
//! a property of how the pad PLAYS it.
//!
//! It runs on NV12 straight off the decoder, which is the one place this
//! codebase is allowed to touch pixels on the CPU — a `wrap` of a few
//! frames is a few hundred KB of lerp, once, when the loop cache is built,
//! not per present. (The law it must not break: never convert 4K in a
//! software loop. This does not convert anything and does not run per
//! frame.)
//!
//! A cross-fade is the honest fallback, and the run row says so rather than
//! claiming a synthesised wrap. The better version interpolates the wrap
//! with the in-tree RIFE tweener so the motion CONTINUES through the seam
//! instead of dissolving across it; that upgrade slots in behind the same
//! function signature.

use crate::media::{Frame, Pixels};

/// Wrap frames for a ~3 s clip. Long enough that the dissolve reads as
/// motion rather than a dip, short enough that it costs a quarter-second of
/// a three-second loop.
pub const DEFAULT_WRAP: usize = 6;

/// How a loop was closed, for the run row's own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopClosure {
    /// Too few frames to close: the clip plays as it came.
    None,
    /// Tail cross-faded onto the head over `wrap` frames.
    Crossfade { wrap: usize },
}

impl LoopClosure {
    pub fn note(&self) -> String {
        match self {
            LoopClosure::None => "loop not closed".to_string(),
            LoopClosure::Crossfade { wrap } => format!("loop closed · {wrap}f wrap blend"),
        }
    }
}

/// Cross-fade `frames`' tail onto its head and drop the tail, in place.
///
/// The weights run tail-heavy to head-heavy across the window, so the first
/// kept frame is nearly the frame that used to follow the last kept one —
/// which is exactly what makes the wrap invisible.
///
/// Refuses (leaving `frames` untouched, returning [`LoopClosure::None`])
/// when there is not enough clip to spend: a loop whose blend covers a
/// third of its length is a dissolve, not a loop.
pub fn close_loop(frames: &mut Vec<Frame>, wrap: usize) -> LoopClosure {
    let n = frames.len();
    if wrap == 0 || n < wrap * 3 {
        return LoopClosure::None;
    }
    let keep = n - wrap;
    // The tail is read while the head is written, so it is taken first.
    let tail: Vec<Pixels> = frames[keep..].iter().map(|f| f.px.clone()).collect();
    for (i, tail_px) in tail.iter().enumerate() {
        // i = 0 sits at the seam and is almost entirely the old tail frame;
        // i = wrap-1 is almost entirely the original head.
        let head_weight = (i + 1) as f32 / (wrap + 1) as f32;
        blend_into(&mut frames[i].px, tail_px, head_weight);
    }
    frames.truncate(keep);
    LoopClosure::Crossfade { wrap }
}

/// `dst = dst * head_weight + src * (1 - head_weight)`.
///
/// Mismatched shapes are left alone rather than half-blended: a decoder
/// that changed format mid-clip is a bug to see, not to smear.
fn blend_into(dst: &mut Pixels, src: &Pixels, head_weight: f32) {
    let w = head_weight.clamp(0.0, 1.0);
    match (dst, src) {
        (
            Pixels::Nv12 { data: d, width: dw, height: dh },
            Pixels::Nv12 { data: s, width: sw, height: sh },
        ) => {
            if dw != sw || dh != sh || d.len() != s.len() {
                return;
            }
            // Y and the interleaved UV plane both lerp linearly: this is
            // what a cross-fade in YUV is.
            for (dst_byte, src_byte) in d.iter_mut().zip(s.iter()) {
                let blended = *dst_byte as f32 * w + *src_byte as f32 * (1.0 - w);
                *dst_byte = blended.round().clamp(0.0, 255.0) as u8;
            }
        }
        (Pixels::Bgra(d), Pixels::Bgra(s)) => {
            if d.len() != s.len() {
                return;
            }
            for (dst_px, src_px) in d.iter_mut().zip(s.iter()) {
                let mut out = 0u32;
                for shift in [0, 8, 16, 24] {
                    let a = ((*dst_px >> shift) & 0xff) as f32;
                    let b = ((*src_px >> shift) & 0xff) as f32;
                    let v = (a * w + b * (1.0 - w)).round().clamp(0.0, 255.0) as u32;
                    out |= v << shift;
                }
                *dst_px = out;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv12(value: u8, pts: i64) -> Frame {
        Frame {
            pts_100ns: pts,
            clip_100ns: pts,
            // 4x2 luma + 4 bytes of interleaved chroma.
            px: Pixels::Nv12 { data: vec![value; 8 + 4], width: 4, height: 2 },
        }
    }

    fn luma(frame: &Frame) -> u8 {
        match &frame.px {
            Pixels::Nv12 { data, .. } => data[0],
            Pixels::Bgra(d) => (d[0] & 0xff) as u8,
        }
    }

    /// The seam is what this exists for: after closing, stepping off the
    /// END of the loop and onto its START must be about as big a step as
    /// any other, instead of the whole clip's worth of jump.
    #[test]
    fn the_wrap_step_stops_being_a_jump_cut() {
        // A ramp: frame k has luma 10k, so the raw wrap (last -> first) is
        // a 90-unit jump while every ordinary step is 10.
        let mut frames: Vec<Frame> =
            (0..10).map(|k| nv12((k * 10) as u8, k as i64)).collect();
        let raw_wrap = luma(&frames[9]).abs_diff(luma(&frames[0]));
        assert_eq!(raw_wrap, 90);

        let closed = close_loop(&mut frames, 3);
        assert_eq!(closed, LoopClosure::Crossfade { wrap: 3 });
        assert_eq!(frames.len(), 7, "the tail is spent, not kept");

        let wrap_step = luma(&frames[6]).abs_diff(luma(&frames[0]));
        assert!(
            wrap_step < raw_wrap / 2,
            "wrap step {wrap_step} should be far under the raw {raw_wrap}"
        );
        // And the first frame now leans on the old tail, which is what
        // makes it continuous with the frame before the wrap.
        assert!(luma(&frames[0]) > 40, "{}", luma(&frames[0]));
    }

    #[test]
    fn a_clip_too_short_to_spend_is_left_alone() {
        let mut frames: Vec<Frame> = (0..5).map(|k| nv12(k as u8, k as i64)).collect();
        let before: Vec<u8> = frames.iter().map(luma).collect();
        assert_eq!(close_loop(&mut frames, 3), LoopClosure::None);
        assert_eq!(frames.len(), 5);
        assert_eq!(before, frames.iter().map(luma).collect::<Vec<u8>>());
        // A zero wrap is a no-op, not a panic.
        assert_eq!(close_loop(&mut frames, 0), LoopClosure::None);
    }

    #[test]
    fn mismatched_frames_are_left_alone_rather_than_smeared() {
        let mut frames: Vec<Frame> = (0..9).map(|k| nv12((k * 10) as u8, k as i64)).collect();
        frames[6].px = Pixels::Bgra(vec![0; 8]);
        // Blending an NV12 head with a BGRA tail is refused; the head keeps
        // its own pixels instead of taking half of something else's.
        close_loop(&mut frames, 3);
        assert_eq!(luma(&frames[0]), 0, "head untouched by the foreign tail");
    }
}
