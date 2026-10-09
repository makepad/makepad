//! Gobos (cookies): a pattern hung in front of a light, evaluated in that
//! light's own rectangle. B6 has windows (a frame and muntin bars), plain
//! grids of bars and venetian blinds; B7 adds leaves and fern fronds; B8 image
//! masks and the shadow a gobo throws onto the studio backdrop.
//!
//! Positions are face coordinates `q`: x to the right and y up as the viewer
//! sees the light, in tangent units (`LightFx::face`). Patterns are laid out
//! in "s-units", `q / s` with `s` the light's smaller tangent half extent, so
//! a pane or a slat keeps its proportions on a wide light.
//!
//! A gobo passes a fraction 0..=1 of the light and never adds any. Its
//! penumbra is a box filter of the pattern, not a softened edge: blurring
//! moves light about without losing it, so a gobo passes the same share of
//! its light at any `distance`. A window's pattern is separable (glass is
//! "inside a pane across" and "inside a pane down"), and so is a box filter,
//! so the blurred window is exactly the product of two one-axis filters.
//! Leaves and fern (`foliage.rs`) soften each blade's and stem's edge
//! instead, so a canopy passes somewhat less of its light as it moves away.

use super::*;
use super::foliage::Foliage;

/// Half-width, in s-units, of the penumbra of a gobo hanging `distance` 0..=1
/// in front of its light. Against the light (0) its edges are sharp; far off
/// (1) they blur over about an eighth of the light's smaller half size, as a
/// cookie's shadow does when it moves away from the lamp.
pub fn penumbra(distance: f32) -> f32 {
    // max/min, not clamp: a NaN distance reads as 0.
    0.002 + 0.12 * distance.max(0.0).min(1.0)
}

/// The share of the blur window `x - w ..= x + w` that falls on glass along
/// one axis of a window. The opening runs from `-inner` to `inner` and holds
/// `n` panes; neighbouring panes are separated by a bar `bar` wide, centred on
/// the pane boundary. With a frame, everything outside the opening is opaque;
/// without one (a grid, or a frame of zero width) the outer panes run on past
/// the light's edge, so the gobo does not darken the light's own edge.
fn panes_open(x: f32, inner: f32, n: u32, bar: f32, framed: bool, w: f32) -> f32 {
    let w = w.max(1.0e-5);
    let pitch = 2.0 * inner / n as f32;
    let (lo, hi) = (x - w, x + w);
    // Only the panes the window can touch.
    let first = (((lo + inner) / pitch).floor().max(0.0) as u32).min(n - 1);
    let last = (((hi + inner) / pitch).floor().max(0.0) as u32).min(n - 1);
    let mut open = 0.0f32;
    for k in first..=last {
        // A pane's glass: its cell, less half a bar on each side that has a neighbour.
        let mut a = -inner + k as f32 * pitch;
        let mut b = a + pitch;
        if k > 0 {
            a += 0.5 * bar;
        } else if !framed {
            a = f32::NEG_INFINITY;
        }
        if k + 1 < n {
            b -= 0.5 * bar;
        } else if !framed {
            b = f32::INFINITY;
        }
        open += (hi.min(b) - lo.max(a)).max(0.0);
    }
    (open / (2.0 * w)).min(1.0)
}

/// A gobo, built once per light when the map is built (`Env::new_with`), then
/// sampled per direction from many threads.
#[derive(Clone, Debug)]
pub struct Gobo {
    kind: GoboKind,
    /// The s-unit: the light's smaller tangent half extent.
    s: f32,
    /// The light's rectangle in s-units: its half extents over `s` (the smaller is 1).
    rect: Vec2f,
    /// Penumbra half-width in s-units, from `distance`.
    soft: f32,
    panes: [u32; 2],
    bar: f32,
    frame: f32,
    slats: f32,
    /// Half a slat's width, in slat pitches (`slat_width / 2`).
    slat_half: f32,
    /// sin and cos of the slats' tilt from edge-on.
    tilt_sin: f32,
    tilt_cos: f32,
    vertical: bool,
    /// Leaves or fern fronds, scattered once from the seed (B7).
    foliage: Option<Foliage>,
}

impl Gobo {
    /// The gobo of `p` over a light with tangent half extents `half`. None
    /// when the kind draws nothing: image masks (which `images` resolves)
    /// arrive with B8.
    pub fn new(p: &GoboParams, half: Vec2f, _images: &ImageSet) -> Option<Gobo> {
        let kind = p.kind();
        if kind == GoboKind::Image {
            return None;
        }
        let half = vec2f(half.x.max(1.0e-4), half.y.max(1.0e-4));
        let s = half.x.min(half.y);
        let rect = vec2f(half.x / s, half.y / s);
        let soft = penumbra(p.distance);
        let foliage = match kind {
            GoboKind::Leaves => Some(Foliage::leaves(rect, p.density, p.scale, p.seed, soft)),
            GoboKind::Fern => Some(Foliage::fern(rect, p.density, p.scale, p.seed, soft)),
            _ => None,
        };
        let (tilt_sin, tilt_cos) = p.tilt_deg.max(0.0).min(90.0).to_radians().sin_cos();
        Some(Gobo {
            kind,
            s,
            rect,
            soft,
            panes: [p.panes_x.clamp(1, 16), p.panes_y.clamp(1, 16)],
            bar: p.bar.max(0.0).min(0.5),
            frame: p.frame.max(0.0).min(0.5),
            slats: p.slats.clamp(2, 64) as f32,
            // max/min, not clamp: a NaN width reads as 0.5.
            slat_half: 0.5 * p.slat_width.max(0.5).min(1.5),
            tilt_sin,
            tilt_cos,
            vertical: p.vertical,
            foliage,
        })
    }

    /// The fraction 0..=1 of the light that passes at face coordinates `q`.
    pub fn transmission(&self, q: Vec2f) -> f32 {
        // A NaN direction passes the light instead of poisoning the map.
        if !(q.x.is_finite() && q.y.is_finite()) {
            return 1.0;
        }
        let p = q / self.s;
        let t = match self.kind {
            GoboKind::Window => self.window(p, true),
            GoboKind::Grid => self.window(p, false),
            GoboKind::Blinds => self.blinds(p, q),
            GoboKind::Leaves | GoboKind::Fern => self.foliage.as_ref().map_or(1.0, |f| f.transmission(p)),
            GoboKind::Image => 1.0,
        };
        if t.is_finite() {
            t.max(0.0).min(1.0)
        } else {
            1.0
        }
    }

    /// A window at `p` (s-units): a frame `frame` wide inside the light's
    /// edge (only when `framed`), and muntin bars between the panes inside
    /// it, `bar` of the smaller pane pitch wide, the same width both ways.
    fn window(&self, p: Vec2f, framed: bool) -> f32 {
        let fw = if framed { self.frame } else { 0.0 };
        let inner_x = (self.rect.x - fw).max(1.0e-3);
        let inner_y = (self.rect.y - fw).max(1.0e-3);
        let pitch_x = 2.0 * inner_x / self.panes[0] as f32;
        let pitch_y = 2.0 * inner_y / self.panes[1] as f32;
        let bar = self.bar * pitch_x.min(pitch_y);
        let framed = fw > 0.0;
        panes_open(p.x, inner_x, self.panes[0], bar, framed, self.soft) * panes_open(p.y, inner_y, self.panes[1], bar, framed, self.soft)
    }

    /// Venetian blinds at `p` (s-units; `q` is the same point in tangent
    /// units). `slats` slats cross the light, pivoting on lines one pitch
    /// apart; a slat is `slat_width` pitches deep (1.08 by default, so shut
    /// blinds overlap; below 1 a gap stays open).
    ///
    /// A slat tilted by `tilt` from edge-on is the segment
    /// ±(slat_width/2)·pitch·(sin tilt, cos tilt) in (across the window,
    /// toward the viewer). Seen along a ray whose slope across the window is
    /// `view` (which is the tangent coordinate itself), it hides a band
    /// (slat_width/2)·pitch·|sin tilt − cos tilt · view| either side of its
    /// pivot, and never less than its own thickness. So open blinds seen
    /// from an angle close a little, and overlapping shut blinds stay shut
    /// from every side.
    ///
    /// The gaps are box-filtered, wider as the blinds close (the gaps become
    /// slits that spread the light): a gap narrower than the blur dims
    /// instead of vanishing, and a shut one passes nothing.
    fn blinds(&self, p: Vec2f, q: Vec2f) -> f32 {
        // The axis across the slats: up the light for horizontal slats.
        let (a, h, view) = if self.vertical { (p.x, self.rect.x, q.x) } else { (p.y, self.rect.y, q.y) };
        let pitch = 2.0 * h / self.slats;
        let cover = (self.slat_half * pitch * (self.tilt_sin - self.tilt_cos * view).abs()).max(0.03 * pitch);
        // Half the open gap between two slats.
        let gap = 0.5 * pitch - cover;
        if !(gap > 0.0) {
            return 0.0;
        }
        let w = (self.soft + 0.25 * pitch * self.tilt_sin).min(0.45 * pitch).max(1.0e-5);
        // The gaps are centred on the lines half way between two pivots, which
        // are the multiples of the pitch measured from the light's lower edge.
        // Shifted by the gap's half-width they start on those multiples, and
        // the open length up to `u` is a staircase with a ramp per pitch.
        let open_up_to = |u: f32| {
            let whole = (u / pitch).floor();
            whole * 2.0 * gap + (u - whole * pitch).min(2.0 * gap)
        };
        let u = a + h + gap;
        (open_up_to(u + w) - open_up_to(u - w)) / (2.0 * w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gobo(p: GoboParams, half: Vec2f) -> Gobo {
        Gobo::new(&p, half, &ImageSet::default()).expect("a gobo")
    }

    /// Mean transmission over the light's rectangle, n x n samples.
    fn mean_over(g: &Gobo, half: Vec2f, n: usize) -> f32 {
        let mut sum = 0.0f64;
        for j in 0..n {
            for i in 0..n {
                let q = vec2f(
                    ((i as f32 + 0.5) / n as f32 * 2.0 - 1.0) * half.x,
                    ((j as f32 + 0.5) / n as f32 * 2.0 - 1.0) * half.y,
                );
                sum += g.transmission(q) as f64;
            }
        }
        (sum / (n * n) as f64) as f32
    }

    #[test]
    fn a_window_passes_its_panes_and_blocks_its_frame_and_bars() {
        // half (0.4, 0.3): s = 0.3, so the light is 1.333 x 1 s-units. A 0.1 frame
        // leaves 1.233 x 0.9 inside; two panes across put one bar on x = 0.
        let g = gobo(GoboParams { panes_x: 2, panes_y: 1, bar: 0.2, frame: 0.1, distance: 0.0, ..GoboParams::default() }, vec2f(0.4, 0.3));
        assert!(g.transmission(vec2f(0.185, 0.0)) > 0.99, "the right pane's middle");
        assert!(g.transmission(vec2f(-0.185, 0.0)) > 0.99, "the left pane's middle");
        assert!(g.transmission(vec2f(0.0, 0.0)) < 0.01, "the muntin between the panes");
        assert!(g.transmission(vec2f(0.0, 0.29)) < 0.01, "the frame along the top");
        assert!(g.transmission(vec2f(0.395, 0.0)) < 0.01, "the frame down the side");
    }

    #[test]
    fn a_grid_has_bars_but_no_frame() {
        let p = GoboParams { panes_x: 3, panes_y: 3, bar: 0.1, frame: 0.3, distance: 0.0, ..GoboParams::default() };
        let grid = gobo(GoboParams { kind: "grid".to_string(), ..p.clone() }, vec2f(0.4, 0.3));
        let window = gobo(GoboParams { kind: "window".to_string(), ..p }, vec2f(0.4, 0.3));
        // s-units (1.2, 0): inside a grid cell, but inside a 0.3 window frame.
        assert!(grid.transmission(vec2f(0.36, 0.0)) > 0.99);
        assert!(window.transmission(vec2f(0.36, 0.0)) < 0.01);
        // On a bar both are dark: three panes across put bars at a third of the width.
        assert!(grid.transmission(vec2f(0.4 / 3.0, 0.0)) < 0.01);
    }

    /// Mean transmission of 10-slat blinds over a 20 degree light.
    fn blinds_mean(tilt_deg: f32) -> f32 {
        let h = 10f32.to_radians().tan();
        let g = gobo(GoboParams { kind: "blinds".to_string(), slats: 10, tilt_deg, distance: 0.0, ..GoboParams::default() }, vec2f(h, h));
        mean_over(&g, vec2f(h, h), 200)
    }

    #[test]
    fn blinds_close_as_they_tilt() {
        assert!(blinds_mean(0.0) > 0.8, "open blinds pass most of the light: {}", blinds_mean(0.0));
        let m: Vec<f32> = [30.0, 50.0, 70.0, 90.0].iter().map(|&t| blinds_mean(t)).collect();
        for w in m.windows(2) {
            assert!(w[0] > w[1], "{m:?}");
        }
        assert_eq!(m[3], 0.0, "shut blinds pass nothing, from any side: {m:?}");
    }

    #[test]
    fn narrow_slats_leave_a_gap_when_shut() {
        let h = 10f32.to_radians().tan();
        let mean = |tilt_deg: f32, slat_width: f32| {
            let p = GoboParams { kind: "blinds".to_string(), slats: 10, tilt_deg, slat_width, distance: 0.0, ..GoboParams::default() };
            mean_over(&gobo(p, vec2f(h, h)), vec2f(h, h), 200)
        };
        // Shut, slats 0.8 pitches wide leave 0.2 of every pitch open; the
        // default 1.08 overlap and pass nothing.
        let shut = mean(90.0, 0.8);
        assert!((shut - 0.2).abs() < 0.02, "{shut}");
        assert_eq!(mean(90.0, GoboParams::default().slat_width), 0.0);
        // Narrower slats pass more at every tilt.
        for tilt in [0.0f32, 30.0, 60.0, 90.0] {
            assert!(mean(tilt, 0.8) > mean(tilt, 1.2), "tilt {tilt}");
        }
    }

    #[test]
    fn vertical_blinds_run_the_other_way() {
        let half = vec2f(0.3, 0.3);
        let make = |vertical: bool| {
            gobo(GoboParams { kind: "blinds".to_string(), slats: 8, tilt_deg: 30.0, vertical, distance: 0.0, ..GoboParams::default() }, half)
        };
        let (h, v) = (make(false), make(true));
        for i in 0..50 {
            let a = -0.29 + 0.58 * i as f32 / 49.0;
            // Horizontal slats vary only up the light, vertical ones only across it.
            assert_eq!(h.transmission(vec2f(-0.2, a)), h.transmission(vec2f(0.2, a)));
            assert_eq!(v.transmission(vec2f(a, -0.2)), v.transmission(vec2f(a, 0.2)));
        }
        let spread = |g: &Gobo, up: bool| {
            let ts: Vec<f32> = (0..200)
                .map(|i| {
                    let a = -0.29 + 0.58 * i as f32 / 199.0;
                    g.transmission(if up { vec2f(0.0, a) } else { vec2f(a, 0.0) })
                })
                .collect();
            ts.iter().cloned().fold(0.0f32, f32::max) - ts.iter().cloned().fold(1.0f32, f32::min)
        };
        assert!(spread(&h, true) > 0.5 && spread(&v, false) > 0.5);
    }

    #[test]
    fn a_gobo_only_ever_passes_part_of_the_light() {
        for kind in ["window", "blinds", "grid", "leaves", "fern"] {
            for distance in [0.0f32, 0.5, 1.0] {
                let g = gobo(GoboParams { kind: kind.to_string(), distance, ..GoboParams::default() }, vec2f(0.5, 0.2));
                for j in 0..41 {
                    for i in 0..41 {
                        // Well past the light's edges too.
                        let q = vec2f(-0.8 + 1.6 * i as f32 / 40.0, -0.4 + 0.8 * j as f32 / 40.0);
                        let t = g.transmission(q);
                        assert!(t.is_finite() && (0.0..=1.0).contains(&t), "{kind} at {q:?}: {t}");
                    }
                }
                assert_eq!(g.transmission(vec2f(f32::NAN, f32::NAN)), 1.0, "a NaN direction passes instead of poisoning the map");
            }
        }
    }

    #[test]
    fn a_wider_penumbra_keeps_the_light_a_gobo_passes() {
        // The blur is a box filter of the mask, so it moves light about and
        // loses none: sharp and soft pass the same share.
        let half = vec2f(0.5, 0.2);
        for kind in ["grid", "blinds"] {
            let mean = |distance: f32| mean_over(&gobo(GoboParams { kind: kind.to_string(), distance, ..GoboParams::default() }, half), half, 200);
            let (sharp, soft) = (mean(0.0), mean(1.0));
            assert!(sharp > 0.3 && sharp < 0.99, "{kind}: {sharp}");
            assert!((sharp - soft).abs() < 0.01 * sharp, "{kind}: sharp {sharp}, soft {soft}");
        }
        assert!(penumbra(1.0) > 20.0 * penumbra(0.0));
        assert_eq!(penumbra(f32::NAN), penumbra(0.0));
    }

    #[test]
    fn the_kinds_not_drawn_yet_are_no_gobo() {
        // Image masks arrive with B8.
        let p = GoboParams { kind: "image".to_string(), ..GoboParams::default() };
        assert!(Gobo::new(&p, vec2f(0.3, 0.2), &ImageSet::default()).is_none());
    }

    #[test]
    fn leaf_and_fern_gobos_are_part_dark_part_open() {
        for kind in ["leaves", "fern"] {
            let g = gobo(GoboParams { kind: kind.to_string(), density: 0.6, ..GoboParams::default() }, vec2f(0.4, 0.3));
            let (mut dark, mut open) = (false, false);
            for j in 0..30 {
                for i in 0..40 {
                    let q = vec2f(-0.4 + 0.8 * (i as f32 + 0.5) / 40.0, -0.3 + 0.6 * (j as f32 + 0.5) / 30.0);
                    let t = g.transmission(q);
                    assert!((0.0..=1.0).contains(&t));
                    dark |= t < 0.05;
                    open |= t > 0.95;
                }
            }
            assert!(dark && open, "{kind}: dark {dark}, open {open}");
        }
    }
}
