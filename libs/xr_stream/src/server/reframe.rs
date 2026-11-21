//! Reframing: a handheld's picture spent on the app's content. The app says where its content
//! lies ([`crate::XrServer::set_content_bounds`]: boxes in the XR frame's space; the rest of its
//! view is transparent). Each pose's view is narrowed to the content's rect, an off-axis part of
//! the device's own view, before the app renders it, so the content gets the whole picture at
//! the same encode size. The device maps the picture back into its view through the eyes the
//! video head carries (they are the eyes it was rendered with). Only for a client that sets
//! `codecs::REFRAME`; `MAKEPAD_XR_REFRAME=0` on the server turns it off.
//!
//! The encoder sees a zoom as motion, so the rect moves seldom and smoothly: it grows at once
//! (the content never clipped), shrinks only after the content wanted a much smaller one for a
//! while and then slowly, and its edges snap to a grid of the view so a still device sends the
//! same view frame after frame.

/// The smallest rect: this part of the view each way (four times the pixels each way: about
/// the handheld screen's own density; more would be supersampling the encoder cannot show).
const MIN_PART: f32 = 0.25;
/// Room kept around the content each way, a part of its size: its shadow, outlines, AA fringe.
const MARGIN: f32 = 0.15;
/// Content within this part of the rect's size of its edge (or outside it) grows it at once.
const EDGE: f32 = 0.08;
/// The rect shrinks only when the content wants under this part of its size ...
const SHRINK_BELOW: f32 = 0.8;
/// ... for this long (ns) ...
const SHRINK_AFTER_NS: u64 = 400_000_000;
/// ... and then by at most this part of its size a second.
const SHRINK_RATE: f32 = 0.25;
/// The edges snap to this part of the view's span (a still device sends the same view).
const SNAP: f32 = 1.0 / 256.0;
/// The content's boxes are cut this far in front of the eye (m, along its axis): what of a box
/// reaches past the eye (beside it, or around it) is not in its view.
const NEAR_M: f32 = 0.02;

/// A part of a view as tangents of its angles: x left to right, y down to up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TanRect {
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl TanRect {
    /// From an eye's fov (angles: left, right, up, down; left and down below 0).
    pub(crate) fn of_fov(fov: [f32; 4]) -> TanRect {
        let [l, r, u, d] = fov.map(f32::tan);
        TanRect { x0: l, x1: r, y0: d, y1: u }
    }

    pub(crate) fn to_fov(self) -> [f32; 4] {
        [self.x0.atan(), self.x1.atan(), self.y1.atan(), self.y0.atan()]
    }

    fn w(&self) -> f32 {
        self.x1 - self.x0
    }

    fn h(&self) -> f32 {
        self.y1 - self.y0
    }

    fn centre(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    /// Grown by `by` of its own size each way (below 0: shrunk).
    fn grown(self, by: f32) -> TanRect {
        let (dx, dy) = (self.w() * by, self.h() * by);
        TanRect { x0: self.x0 - dx, x1: self.x1 + dx, y0: self.y0 - dy, y1: self.y1 + dy }
    }

    fn contains(&self, o: &TanRect) -> bool {
        o.x0 >= self.x0 && o.x1 <= self.x1 && o.y0 >= self.y0 && o.y1 <= self.y1
    }

    /// Its part inside `within` (None: none of it).
    fn clipped(self, within: &TanRect) -> Option<TanRect> {
        let r = TanRect { x0: self.x0.max(within.x0), x1: self.x1.min(within.x1), y0: self.y0.max(within.y0), y1: self.y1.min(within.y1) };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }

    /// Where it lies in `full` as uv (x0, y0, x1, y1; 0..1, v down from the top).
    pub(crate) fn uv_in(&self, full: &TanRect) -> [f32; 4] {
        let (w, h) = (full.w().max(1e-6), full.h().max(1e-6));
        [(self.x0 - full.x0) / w, (full.y1 - self.y1) / h, (self.x1 - full.x0) / w, (full.y1 - self.y0) / h]
    }
}

/// Where the content lies in the view `full` of an eye at `position` turned by `orientation`
/// (x, y, z, w; it looks down its -z): the rect of what of its boxes is in that view. Each box
/// is cut at [`NEAR_M`] (its corners in front, and where its edges cross the cut) and its rect
/// kept inside the view, so a device standing at or in its content frames the part it looks
/// at, and a box around the eye spans the whole view. None: nothing of it in the view.
pub(crate) fn content_rect(orientation: [f32; 4], position: [f32; 3], boxes: &[[[f32; 3]; 2]], full: &TanRect) -> Option<TanRect> {
    let [qx, qy, qz, qw] = orientation;
    // into the eye: the inverse turn of the corner's offset
    let into_eye = |p: [f32; 3]| {
        let v = [p[0] - position[0], p[1] - position[1], p[2] - position[2]];
        let q = [-qx, -qy, -qz];
        let t = [2.0 * (q[1] * v[2] - q[2] * v[1]), 2.0 * (q[2] * v[0] - q[0] * v[2]), 2.0 * (q[0] * v[1] - q[1] * v[0])];
        [v[0] + qw * t[0] + (q[1] * t[2] - q[2] * t[1]), v[1] + qw * t[1] + (q[2] * t[0] - q[0] * t[2]), v[2] + qw * t[2] + (q[0] * t[1] - q[1] * t[0])]
    };
    let union = |a: Option<TanRect>, b: TanRect| {
        Some(match a {
            None => b,
            Some(r) => TanRect { x0: r.x0.min(b.x0), x1: r.x1.max(b.x1), y0: r.y0.min(b.y0), y1: r.y1.max(b.y1) },
        })
    };
    let mut rect: Option<TanRect> = None;
    for [lo, hi] in boxes {
        let mut one: Option<TanRect> = None;
        let mut add = |e: [f32; 3], depth: f32| {
            let (x, y) = (e[0] / depth, e[1] / depth);
            one = union(one, TanRect { x0: x, x1: x, y0: y, y1: y });
        };
        let e: [[f32; 3]; 8] = std::array::from_fn(|k| into_eye([if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]));
        for (k, a) in e.iter().enumerate() {
            let da = -a[2];
            if da > NEAR_M {
                add(*a, da);
            }
            // its edges to the corners one axis over: where one crosses the cut
            for bit in [1, 2, 4] {
                if k & bit != 0 {
                    continue;
                }
                let b = e[k | bit];
                let db = -b[2];
                if (da > NEAR_M) != (db > NEAR_M) {
                    let t = (NEAR_M - da) / (db - da);
                    add([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, -NEAR_M], NEAR_M);
                }
            }
        }
        // (a box beside the view, or reaching past the eye out of it, adds nothing)
        if let Some(one) = one.and_then(|r| r.clipped(full)) {
            rect = union(rect, one);
        }
    }
    rect
}

/// `target` as a rect of the view's own shape: its short side widened to the view's aspect
/// (the encode size is fixed and its pixels stay square), at least [`MIN_PART`] of the view,
/// centred on the target and kept inside the view.
fn shaped(target: &TanRect, full: &TanRect) -> TanRect {
    let aspect = full.w() / full.h().max(1e-6);
    let (cx, cy) = target.centre();
    let w = target.w().max(target.h() * aspect).max(full.w() * MIN_PART).min(full.w());
    let h = (w / aspect).min(full.h());
    // (a rect as wide as the view: x1 - w can round a hair below x0, so the bound is kept above it)
    let x0 = (cx - w * 0.5).clamp(full.x0, (full.x1 - w).max(full.x0));
    let y0 = (cy - h * 0.5).clamp(full.y0, (full.y1 - h).max(full.y0));
    TanRect { x0, x1: x0 + w, y0, y1: y0 + h }
}

/// The rect from frame to frame (one per session: a new link starts with the full view).
#[derive(Default)]
pub(crate) struct Reframe {
    /// The rect now (None: the full view).
    rect: Option<TanRect>,
    /// Since when the content wanted a much smaller rect (ns).
    small_since: Option<u64>,
    last_ns: u64,
    /// The last pose's full view and the view it was given (for the desktop's mirror).
    pub(crate) last: Option<([f32; 4], [f32; 4])>,
    /// Why the last view is what it is, and the content's rect then (as uv of the view), for
    /// the log (`describe`); when it was last logged (ns).
    why: &'static str,
    target: Option<[f32; 4]>,
    boxes: usize,
    pub(crate) logged_ns: u64,
    /// Since the last `describe`: poses, those narrowed, and the sum of each pose's picture
    /// pixels on its content each way (the whole view's: 1).
    poses: u32,
    narrowed: u32,
    gain: f32,
}

impl Reframe {
    /// The view to render for an eye with `fov`, at `orientation` and `position`, at `now_ns`:
    /// narrowed to the content in `boxes` (None: the app said nothing; the full view).
    pub(crate) fn step(&mut self, now_ns: u64, orientation: [f32; 4], position: [f32; 3], fov: [f32; 4], boxes: Option<&[[[f32; 3]; 2]]>) -> [f32; 4] {
        let full = TanRect::of_fov(fov);
        let dt = if self.last_ns == 0 { 0.0 } else { now_ns.saturating_sub(self.last_ns) as f32 * 1e-9 };
        self.last_ns = now_ns;
        self.boxes = boxes.map_or(0, <[_]>::len);
        self.target = None;
        let rect = match boxes.map(|b| (b.is_empty(), content_rect(orientation, position, b, &full))) {
            // nothing said: the full view
            None => {
                self.why = "no content bounds (or switched off)";
                None
            }
            Some((true, _)) => {
                self.why = "no content";
                None
            }
            // the content out of view: the rect stays (nothing of it shows)
            Some((false, None)) => {
                self.why = "the content out of view";
                self.rect
            }
            Some((false, Some(target))) => {
                self.target = Some(target.uv_in(&full));
                self.why = "narrowed to the content";
                Some(self.follow(target, &full, now_ns, dt))
            }
        };
        if rect.is_none() {
            self.small_since = None;
        }
        self.rect = rect;
        let given = match rect {
            None => fov,
            Some(r) => {
                // snapped to the grid of the view: a still device sends the same view
                let (sx, sy) = (full.w() * SNAP, full.h() * SNAP);
                let snap = |v: f32, from: f32, step: f32| from + ((v - from) / step).round() * step;
                let s = TanRect { x0: snap(r.x0, full.x0, sx), x1: snap(r.x1, full.x0, sx), y0: snap(r.y0, full.y0, sy), y1: snap(r.y1, full.y0, sy) };
                // (the whole view to the grid: the device's own view, not a copy of it)
                let whole = s.x0 <= full.x0 + sx * 0.5 && s.x1 >= full.x1 - sx * 0.5 && s.y0 <= full.y0 + sy * 0.5 && s.y1 >= full.y1 - sy * 0.5;
                if whole { fov } else { s.clipped(&full).map_or(fov, TanRect::to_fov) }
            }
        };
        self.last = Some((fov, given));
        self.poses += 1;
        if given != fov {
            self.narrowed += 1;
            self.gain += full.w() / TanRect::of_fov(given).w().max(1e-6);
        } else {
            self.gain += 1.0;
            if self.why == "narrowed to the content" {
                self.why = "the content fills the view";
            }
        }
        given
    }

    /// The last step, for the log: why, the content's rect, the view given (as uv of the view);
    /// since the last call, how many poses were narrowed and the content's pixels each way on
    /// average (the whole view's: 1.0x); and each of the `boxes` as its largest side at its
    /// centre's distance from the eye at `position` (what fills a view, or reaches past it).
    pub(crate) fn describe(&mut self, position: [f32; 3], boxes: Option<&[[[f32; 3]; 2]]>) -> String {
        let uv = |r: [f32; 4]| format!("x {:.2}..{:.2} y {:.2}..{:.2}", r[0], r[2], r[1], r[3]);
        let given = self.last.and_then(|(full, given)| (full != given).then(|| TanRect::of_fov(given).uv_in(&TanRect::of_fov(full))));
        let line = format!(
            "{} box(es): {}; the content at {}; the view {}; narrowed {} of {} poses, the content at {:.2}x the pixels each way on average",
            self.boxes,
            self.why,
            self.target.map_or("-".to_string(), uv),
            given.map_or("whole".to_string(), uv),
            self.narrowed,
            self.poses,
            self.gain / self.poses.max(1) as f32
        );
        (self.poses, self.narrowed, self.gain) = (0, 0, 0.0);
        let sizes: Vec<String> = boxes
            .unwrap_or(&[])
            .iter()
            .map(|[lo, hi]| {
                let side = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f32, f32::max);
                let away = (0..3).map(|i| ((lo[i] + hi[i]) * 0.5 - position[i]).powi(2)).sum::<f32>().sqrt();
                format!("{side:.2} m at {away:.2} m")
            })
            .collect();
        if sizes.is_empty() { line } else { format!("{line}; the boxes: {}", sizes.join(", ")) }
    }

    /// The rect after this frame for the content's `target` (inside the view).
    fn follow(&mut self, target: TanRect, full: &TanRect, now_ns: u64, dt: f32) -> TanRect {
        let goal = shaped(&target.grown(MARGIN), full);
        let Some(cur) = self.rect else { return goal };
        // grow at once: the content outside the rect or near its edge
        if !cur.grown(-EDGE).contains(&target) {
            self.small_since = None;
            return goal;
        }
        // shrink slowly, after a while
        if goal.w() >= cur.w() * SHRINK_BELOW && goal.h() >= cur.h() * SHRINK_BELOW {
            self.small_since = None;
            return cur;
        }
        let since = *self.small_since.get_or_insert(now_ns);
        if now_ns.saturating_sub(since) < SHRINK_AFTER_NS {
            return cur;
        }
        // each edge the same part of the way to the goal's: both hold the content, so every
        // rect between them does
        let left = 1.0 - goal.w() / cur.w().max(1e-6);
        let f = if left <= 0.0 { 1.0 } else { (SHRINK_RATE * dt / left).min(1.0) };
        let lerp = |a: f32, b: f32| a + (b - a) * f;
        TanRect { x0: lerp(cur.x0, goal.x0), x1: lerp(cur.x1, goal.x1), y0: lerp(cur.y0, goal.y0), y1: lerp(cur.y1, goal.y1) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target that fills the view: the view's x1 - w rounds below its x0 here (a device's
    /// view whose bounds crossed made `clamp` panic and ended the uplink thread).
    #[test]
    fn a_target_filling_the_view_keeps_inside_it() {
        let full = TanRect { x0: -0.5922771, x1: 0.3002371, y0: -0.5922771, y1: 0.3002371 };
        assert!(full.x1 - full.w() < full.x0, "the view's bounds cross by a rounding");
        let r = shaped(&TanRect { x0: -1.0, x1: 1.0, y0: -1.0, y1: 1.0 }, &full);
        assert_eq!((r.x0, r.y0), (full.x0, full.y0), "the rect starts at the view's corner");
        assert!(r.w() <= full.w() && r.h() <= full.h(), "the rect is no larger than the view");
    }

    /// A device at its content: a box reaching past the eye (beside it, out of view) once gave
    /// the whole view; now the view is narrowed to what is in it. A box around the eye fills it.
    #[test]
    fn a_box_reaching_past_the_eye_still_narrows() {
        // a handheld camera's view, looking down -z from the origin
        let fov = [-0.58f32.atan(), 0.58f32.atan(), 0.43f32.atan(), -0.43f32.atan()];
        let (still, at) = ([0.0, 0.0, 0.0, 1.0], [0.0; 3]);
        let part = [[-0.05, -0.15, -0.5], [0.05, -0.05, -0.4]];
        let beside = [[0.3, -0.3, -0.3], [0.6, 0.0, 0.3]];
        let given = Reframe::default().step(1, still, at, fov, Some(&[part, beside]));
        let (full, view) = (TanRect::of_fov(fov), TanRect::of_fov(given));
        assert!(view.contains(&TanRect { x0: -0.125, x1: 0.125, y0: -0.375, y1: -0.1 }), "the part ahead is in the view given: {view:?}");
        assert!(full.w() / view.w() > 2.0, "the view is narrowed to the part, not kept whole for the box beside: {view:?}");
        let around = [[-0.2, -0.2, -0.2], [0.2, 0.2, 0.2]];
        assert_eq!(Reframe::default().step(1, still, at, fov, Some(&[part, around])), fov, "a box around the eye fills the view");
    }
}
