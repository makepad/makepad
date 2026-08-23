//! THE DECK STAND-INS — the two test pictures a transition preview is baked
//! from, in ONE place.
//!
//! A transition document composites two program decks; a thumbnail (and the
//! gallery, when it previews one standalone) has no decks, so it feeds two
//! generated pictures instead. Both consumers — `fx_thumbs::transition_input`
//! (the authoritative one: every transition tile in the grid is baked off
//! this) and the gallery's static preview — call the SAME texel function, so
//! a change to the look can never land on one and not the other.
//!
//! ## They are deliberately QUIET
//!
//! The first pass shipped full-saturation primaries — a warm orange ramp with
//! a white disc against a cyan grid with a white bar — and a wall of seventy
//! transition thumbnails then glowed over the whole dark UI: the stand-in art
//! became the loudest thing on screen and the transition MASK, which is the
//! only reason the tile exists, read as an afterthought. So both pictures now
//! live in the app's own register (the UI sits around `#x1c2129`): two moody
//! slates, one warm and one cool, no channel over [`PEAK`], no hotspots, and
//! plenty of dark in each. They stay OBVIOUSLY different — that is the
//! functional requirement, and warm-vs-cool plus disc-vs-grid carries it at
//! 192x120 without any brightness at all.

/// Ceiling for every channel of both pictures. Low on purpose: a transition
/// thumbnail must sit quietly next to a generator effect's thumbnail, and a
/// mask edge is legible against a dim field exactly as well as a bright one.
pub const PEAK: f32 = 0.52;

/// Stand-in picture size (both consumers).
pub const W: usize = 192;
pub const H: usize = 120;

/// One texel of the crossfaded stand-in pair, linear 0..1.
///
/// * `u`, `v` — 0..1 across the picture, `v` down.
/// * `m` — the dissolve: 0 is pure deck A, 1 is pure deck B.
/// * `drift` — (x, y) offset of A's disc, `bar` — B's bar position 0..1.
///   Pass zeros for a static picture (the gallery does, so capture sweeps
///   stay deterministic).
pub fn texel(u: f32, v: f32, m: f32, drift: (f32, f32), bar: f32) -> (f32, f32, f32) {
    let m = m.clamp(0.0, 1.0);
    // ---- A: a dim WARM slate, lit by a soft drifting disc ---------------
    // The ramp runs corner to corner so the picture has a direction even
    // where the disc is not; the disc itself is smoothstepped, never a
    // stamped-in hotspot.
    let wash = 0.30 + 0.34 * u - 0.16 * v;
    let ddx = u - 0.5 - drift.0;
    let ddy = (v - 0.5 - drift.1) * (H as f32 / W as f32) * 2.0;
    let d = (ddx * ddx + ddy * ddy).sqrt();
    let disc = smoothstep(0.30, 0.10, d);
    let lift = 0.22 * disc;
    let ar = (0.20 + 0.62 * wash + lift) * PEAK;
    let ag = (0.14 + 0.42 * wash + lift * 0.8) * PEAK;
    let ab = (0.11 + 0.20 * wash + lift * 0.5) * PEAK;

    // ---- B: a dim COOL slate, ruled by a grid + a travelling bar --------
    let gx = fold(u * (W as f32 / 24.0));
    let gy = fold(v * (H as f32 / 24.0));
    let grid = (1.0 - (gx.min(gy) * 9.0).min(1.0)) * 0.55;
    let barx = ((u + 1.0 - bar).fract() - 0.5).abs();
    let stripe = smoothstep(0.045, 0.012, barx) * 0.45;
    let deep = 0.24 + 0.34 * v;
    let br = (0.10 + 0.16 * deep + grid * 0.5 + stripe * 0.6) * PEAK;
    let bg = (0.16 + 0.42 * deep + grid * 0.9 + stripe * 0.95) * PEAK;
    let bb = (0.26 + 0.66 * deep + grid + stripe) * PEAK;

    (
        (ar + (br - ar) * m).clamp(0.0, PEAK),
        (ag + (bg - ag) * m).clamp(0.0, PEAK),
        (ab + (bb - ab) * m).clamp(0.0, PEAK),
    )
}

/// The texel as one opaque BGRA word, ready for `TextureFormat::VecBGRAu8_32`.
pub fn texel_bgra(u: f32, v: f32, m: f32, drift: (f32, f32), bar: f32) -> u32 {
    let (r, g, b) = texel(u, v, m, drift, bar);
    0xff00_0000
        | (((r * 255.0) as u32) << 16)
        | (((g * 255.0) as u32) << 8)
        | ((b * 255.0) as u32)
}

/// Distance to the nearest cell edge, 0 at the line and 0.5 mid-cell.
fn fold(t: f32) -> f32 {
    let f = t.fract();
    f.min(1.0 - f)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the calm-down: nothing in either picture may glow.
    #[test]
    fn neither_stand_in_ever_exceeds_the_peak() {
        for step in 0..=4 {
            let m = step as f32 / 4.0;
            for y in 0..H {
                for x in 0..W {
                    let (r, g, b) = texel(
                        x as f32 / W as f32,
                        y as f32 / H as f32,
                        m,
                        (0.2, -0.15),
                        0.37,
                    );
                    assert!(
                        r <= PEAK + 1e-4 && g <= PEAK + 1e-4 && b <= PEAK + 1e-4,
                        "stand-in texel blew the ceiling: {r} {g} {b}"
                    );
                }
            }
        }
    }

    /// …and they still have to be two OBVIOUSLY different pictures: warm
    /// against cool, with real contrast inside each.
    #[test]
    fn the_two_stand_ins_stay_clearly_distinct() {
        let mut warm = 0.0f32;
        let mut cool = 0.0f32;
        let (mut a_lo, mut a_hi) = (1.0f32, 0.0f32);
        let (mut b_lo, mut b_hi) = (1.0f32, 0.0f32);
        for y in 0..H {
            for x in 0..W {
                let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
                let (ar, _ag, ab) = texel(u, v, 0.0, (0.0, 0.0), 0.3);
                let (br, _bg, bb) = texel(u, v, 1.0, (0.0, 0.0), 0.3);
                warm += ar - ab;
                cool += bb - br;
                let al = luma(texel(u, v, 0.0, (0.0, 0.0), 0.3));
                let bl = luma(texel(u, v, 1.0, (0.0, 0.0), 0.3));
                a_lo = a_lo.min(al);
                a_hi = a_hi.max(al);
                b_lo = b_lo.min(bl);
                b_hi = b_hi.max(bl);
            }
        }
        assert!(warm > 0.0, "deck A must read WARM (r above b)");
        assert!(cool > 0.0, "deck B must read COOL (b above r)");
        assert!(a_hi - a_lo > 0.08, "deck A is a flat field: {a_lo}..{a_hi}");
        assert!(b_hi - b_lo > 0.08, "deck B is a flat field: {b_lo}..{b_hi}");
    }

    fn luma((r, g, b): (f32, f32, f32)) -> f32 {
        0.30 * r + 0.59 * g + 0.11 * b
    }
}
