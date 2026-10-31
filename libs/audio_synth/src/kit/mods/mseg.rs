//! The MSEG (multi-segment envelope / LFO generator, LFO shape 8): the data
//! model the patch loader fills (`<msegs><mseg …><segments><segment …/>`),
//! its derived timing cache, the two default MSEGs and the evaluator.
//!
//! Time is in LFO cycles: the LFO's unwrapped phase (cycle + fraction)
//! indexes the segment timeline directly.

use crate::dsp::Rng;

/// Segment type (interop ids 1..21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    Linear,
    QuadBezier,
    SCurve,
    Sine,
    Stairs,
    Brownian,
    Square,
    Triangle,
    Hold,
    Sawtooth,
    /// Id 11: legacy; holds the segment's *stored* v0 (Hold uses the
    /// gated-release start value instead).
    None,
    Bump,
    SmoothStairs,
    /// Ids 14..21: K = 1..8 sub-hits.
    Ratchet(u8),
}

impl SegmentType {
    pub fn from_id(id: i32) -> SegmentType {
        match id {
            2 => SegmentType::QuadBezier,
            3 => SegmentType::SCurve,
            4 => SegmentType::Sine,
            5 => SegmentType::Stairs,
            6 => SegmentType::Brownian,
            7 => SegmentType::Square,
            8 => SegmentType::Triangle,
            9 => SegmentType::Hold,
            10 => SegmentType::Sawtooth,
            11 => SegmentType::None,
            12 => SegmentType::Bump,
            13 => SegmentType::SmoothStairs,
            14..=21 => SegmentType::Ratchet((id - 13) as u8),
            _ => SegmentType::Linear,
        }
    }

    pub fn id(self) -> i32 {
        match self {
            SegmentType::Linear => 1,
            SegmentType::QuadBezier => 2,
            SegmentType::SCurve => 3,
            SegmentType::Sine => 4,
            SegmentType::Stairs => 5,
            SegmentType::Brownian => 6,
            SegmentType::Square => 7,
            SegmentType::Triangle => 8,
            SegmentType::Hold => 9,
            SegmentType::Sawtooth => 10,
            SegmentType::None => 11,
            SegmentType::Bump => 12,
            SegmentType::SmoothStairs => 13,
            SegmentType::Ratchet(k) => 13 + k.clamp(1, 8) as i32,
        }
    }
}

/// One segment as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MsegSegment {
    pub duration: f32,
    /// Start value, -1..1.
    pub v0: f32,
    /// End value (cached: the next segment's v0, see [`Mseg::rebuild`]).
    pub nv1: f32,
    /// Control-point time, 0..1 of the segment.
    pub cpduration: f32,
    /// Control-point value, -1..1.
    pub cpv: f32,
    pub ty: SegmentType,
    pub use_deform: bool,
    pub invert_deform: bool,
    pub retrigger_feg: bool,
    pub retrigger_aeg: bool,
}

impl MsegSegment {
    /// A linear segment with a centred control point.
    pub fn linear(duration: f32, v0: f32, nv1: f32) -> MsegSegment {
        MsegSegment {
            duration,
            v0,
            nv1,
            cpduration: 0.5,
            cpv: 0.0,
            ty: SegmentType::Linear,
            use_deform: true,
            invert_deform: false,
            retrigger_feg: false,
            retrigger_aeg: false,
        }
    }
}

/// `loopMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    OneShot = 1,
    Loop = 2,
    GatedLoop = 3,
}

impl LoopMode {
    pub fn from_id(i: i32) -> LoopMode {
        match i {
            1 => LoopMode::OneShot,
            3 => LoopMode::GatedLoop,
            _ => LoopMode::Loop,
        }
    }
}

/// The new-style loop markers' "unset" value.
pub const LOOP_MARKER_UNSET: i32 = -13214;

/// One LFO's MSEG.
#[derive(Clone, Debug, PartialEq)]
pub struct Mseg {
    pub segments: Vec<MsegSegment>,
    /// `endpointMode` 1: the last segment ends at segment 0's start value.
    pub endpoint_locked: bool,
    /// `editMode` 1: LFO (one cycle = the whole MSEG, total forced to 1).
    pub lfo_mode: bool,
    pub loop_mode: LoopMode,
    /// Segment index the loop starts at (may equal n = the last point).
    pub loop_start: i32,
    /// Index of the last segment inside the loop (-1 = point 0).
    pub loop_end: i32,
    // View state, kept for round trips.
    pub h_snap_default: f32,
    pub v_snap_default: f32,
    pub h_snap: f32,
    pub v_snap: f32,
    pub axis_width: f32,
    pub axis_start: f32,
    // Derived by `rebuild`.
    starts: Vec<f32>,
    ends: Vec<f32>,
    total: f32,
    loop_start_time: f32,
    loop_end_time: f32,
}

impl Default for Mseg {
    fn default() -> Self {
        Mseg::default_voice()
    }
}

impl Mseg {
    /// An empty MSEG with the stored defaults (free endpoint, envelope edit
    /// mode, loop, markers unset); the loader fills it and calls `rebuild`.
    pub fn empty() -> Mseg {
        Mseg {
            segments: Vec::new(),
            endpoint_locked: false,
            lfo_mode: false,
            loop_mode: LoopMode::Loop,
            loop_start: 0,
            loop_end: -1,
            h_snap_default: 0.125,
            v_snap_default: 0.25,
            h_snap: 0.0,
            v_snap: 0.0,
            axis_width: -1.0,
            axis_start: -1.0,
            starts: Vec::new(),
            ends: Vec::new(),
            total: 0.0,
            loop_start_time: 0.0,
            loop_end_time: 0.0,
        }
    }

    /// The voice LFOs' default: 0 -> 1 -> 0.5 -> 0.5 -> 0, linear, each one
    /// cycle long, gated loop on segment 2 (the sustain plateau).
    pub fn default_voice() -> Mseg {
        let mut m = Mseg::empty();
        m.segments = vec![
            MsegSegment::linear(1.0, 0.0, 1.0),
            MsegSegment::linear(1.0, 1.0, 0.5),
            MsegSegment::linear(1.0, 0.5, 0.5),
            MsegSegment::linear(1.0, 0.5, 0.0),
        ];
        m.loop_mode = LoopMode::GatedLoop;
        m.loop_start = 2;
        m.loop_end = 2;
        m.rebuild();
        m
    }

    /// The scene LFOs' default: a triangle-like cycle 0 -> 1 -> 0 -> -1 -> 0,
    /// each a quarter, looping over everything.
    pub fn default_scene() -> Mseg {
        let mut m = Mseg::empty();
        m.segments = vec![
            MsegSegment::linear(0.25, 0.0, 1.0),
            MsegSegment::linear(0.25, 1.0, 0.0),
            MsegSegment::linear(0.25, 0.0, -1.0),
            MsegSegment::linear(0.25, -1.0, 0.0),
        ];
        m.lfo_mode = true;
        m.endpoint_locked = true;
        m.loop_mode = LoopMode::Loop;
        m.loop_start = 0;
        m.loop_end = 3;
        m.rebuild();
        m
    }

    pub fn default_for(is_scene: bool) -> Mseg {
        if is_scene { Mseg::default_scene() } else { Mseg::default_voice() }
    }

    /// Resolve the stored loop markers: the point-index attributes when
    /// present (`LOOP_MARKER_UNSET` = absent; -1 for the end means point 0),
    /// else the legacy segment-index ones (negative = unset). Unset start =
    /// segment 0, unset end = the last segment. Call after `segments` is set.
    pub fn set_loop_markers(&mut self, start_point: Option<i32>, end_point: Option<i32>, legacy_start: Option<i32>, legacy_end: Option<i32>) {
        let n = self.segments.len() as i32;
        let start = match start_point.filter(|v| *v != LOOP_MARKER_UNSET) {
            Some(v) => Some(v),
            None => legacy_start.filter(|v| *v >= 0),
        };
        let end = match end_point.filter(|v| *v != LOOP_MARKER_UNSET) {
            Some(v) => Some(v),
            None => legacy_end.filter(|v| *v >= 0),
        };
        self.loop_start = start.unwrap_or(0).clamp(0, n.max(0));
        self.loop_end = end.unwrap_or(n - 1).clamp(-1, (n - 1).max(-1));
    }

    /// Rebuild the derived cache after load / edit: sanitise numbers, chain
    /// end values, cumulative times, total (forced to 1 in LFO mode), loop
    /// timings. Allocates; never call from a render path.
    pub fn rebuild(&mut self) {
        let n = self.segments.len();
        self.segments.truncate(256);
        for s in &mut self.segments {
            if !s.v0.is_finite() {
                s.v0 = 0.0;
            }
            if !s.cpv.is_finite() {
                s.cpv = 0.0;
            }
            if !s.nv1.is_finite() {
                s.nv1 = 0.0;
            }
            if !s.duration.is_finite() {
                s.duration = 0.1;
            }
            if !s.cpduration.is_finite() {
                s.cpduration = 0.6;
            }
            s.duration = s.duration.max(0.0);
            s.cpduration = s.cpduration.clamp(0.0, 1.0);
            s.cpv = s.cpv.clamp(-1.0, 1.0);
        }
        let n = n.min(256);
        for i in 0..n {
            if i + 1 < n {
                self.segments[i].nv1 = self.segments[i + 1].v0;
            } else if self.endpoint_locked {
                self.segments[i].nv1 = self.segments[0].v0;
            }
        }
        self.starts.clear();
        self.ends.clear();
        let mut t = 0.0f32;
        for s in &self.segments {
            self.starts.push(t);
            t += s.duration;
            self.ends.push(t);
        }
        if self.lfo_mode && n > 0 {
            t = 1.0;
            self.ends[n - 1] = 1.0;
        }
        self.total = t;
        self.loop_start = self.loop_start.clamp(0, n as i32);
        self.loop_end = self.loop_end.clamp(-1, n as i32 - 1);
        self.loop_start_time = if (self.loop_start as usize) < n { self.starts[self.loop_start as usize] } else { t };
        self.loop_end_time = if self.loop_end < 0 { 0.0 } else { self.ends[self.loop_end as usize] };
    }

    /// Total duration (cycles); 1 in LFO mode.
    #[inline]
    pub fn total(&self) -> f32 {
        self.total
    }

    /// The cycle-count scale the LFO applies to its start phase: the total
    /// for an envelope-mode MSEG longer than one cycle, else 1.
    #[inline]
    pub fn phase_scale(&self) -> f32 {
        if !self.lfo_mode && self.total > 1.0 { self.total } else { 1.0 }
    }

    pub fn loop_times(&self) -> (f32, f32) {
        (self.loop_start_time, self.loop_end_time)
    }

    fn value_at_loop_end(&self) -> f32 {
        if self.loop_end < 0 { self.segments[0].v0 } else { self.segments[self.loop_end as usize].nv1 }
    }

    /// Evaluate at unwrapped phase `u` (cycles since the attack's origin);
    /// sets `run`'s retrigger flags when a segment is entered.
    pub fn eval(&self, run: &mut MsegRun, u: f64, deform: f32, released: bool) -> f32 {
        run.retrigger_aeg = false;
        run.retrigger_feg = false;
        let n = self.segments.len();
        if n == 0 || self.starts.len() != n {
            return deform;
        }
        let d = deform.clamp(-1.0, 1.0);
        let total = self.total as f64;
        let looping = !self.lfo_mode && self.loop_mode != LoopMode::OneShot;
        if !self.lfo_mode && self.loop_mode == LoopMode::OneShot && u >= total {
            return self.finish(run, self.segments[n - 1].nv1);
        }
        // The gated-loop release branch depends on the loop mode only: an
        // LFO-edit-mode MSEG ignores its markers while held but still plays
        // its post-loop segments once released.
        let gated = self.loop_mode == LoopMode::GatedLoop;
        if gated && released && run.rel.is_none() {
            run.rel = Some((u, run.last));
        }
        let mut over: Option<(usize, f32)> = None;
        let time: f64 = if let (true, Some((ur, vr))) = (gated, run.rel) {
            if self.loop_end >= n as i32 - 1 {
                return self.finish(run, vr);
            }
            let t = u - ur + self.loop_end_time as f64;
            if t >= total {
                return self.finish(run, self.segments[n - 1].nv1);
            }
            over = Some(((self.loop_end + 1) as usize, vr));
            t
        } else if !looping {
            if total > 0.0 { u.rem_euclid(total) } else { 0.0 }
        } else {
            let (ls, le) = (self.loop_start_time as f64, self.loop_end_time as f64);
            if u < le {
                u
            } else if self.loop_start > self.loop_end || le - ls <= 0.0 {
                return self.finish(run, self.value_at_loop_end());
            } else {
                ls + (u - le).rem_euclid(le - ls)
            }
        };
        let time = time as f32;
        let idx = self.ends.partition_point(|e| *e <= time).min(n - 1);
        let seg = &self.segments[idx];
        let local = if seg.duration > 0.0 { ((time - self.starts[idx]) / seg.duration).clamp(0.0, 1.0) } else { 0.0 };
        let entered = run.prev_seg != idx as i32 || local < run.prev_local;
        if entered {
            run.retrigger_aeg = seg.retrigger_aeg;
            run.retrigger_feg = seg.retrigger_feg;
            run.brown_x = seg.v0;
            run.brown_tau = 0.0;
        }
        run.prev_seg = idx as i32;
        run.prev_local = local;
        let mut a = seg.v0;
        let b = seg.nv1;
        let mut cpv = seg.cpv;
        if let Some((oi, vr)) = over {
            if oi == idx {
                if (b - a).abs() > 1e-9 {
                    let k = (cpv - a) / (b - a);
                    cpv = vr + k * (b - vr);
                }
                if entered {
                    run.brown_x = vr;
                }
                a = vr;
            }
        }
        let mut dd = if seg.use_deform { d } else { 0.0 };
        if seg.invert_deform {
            dd = -dd;
        }
        let v = if seg.duration <= 0.0 { 0.5 * (a + b) } else { segment_value(seg, a, b, cpv, local, dd, run) };
        self.finish(run, v)
    }

    fn finish(&self, run: &mut MsegRun, v: f32) -> f32 {
        let v = v.clamp(-1.0, 1.0);
        run.last = v;
        v
    }
}

/// Per-instance evaluation state (owned by the LFO).
#[derive(Clone, Copy, Debug)]
pub struct MsegRun {
    prev_seg: i32,
    prev_local: f32,
    last: f32,
    rel: Option<(f64, f32)>,
    brown_x: f32,
    brown_tau: f32,
    rng: Rng,
    pub retrigger_aeg: bool,
    pub retrigger_feg: bool,
}

impl MsegRun {
    pub fn new(seed: u32) -> MsegRun {
        MsegRun {
            prev_seg: -1,
            prev_local: 0.0,
            last: 0.0,
            rel: None,
            brown_x: 0.0,
            brown_tau: 0.0,
            rng: Rng::new(seed),
            retrigger_aeg: false,
            retrigger_feg: false,
        }
    }

    /// Attack: forget the segment position and any release record.
    pub fn reset(&mut self) {
        self.prev_seg = -1;
        self.prev_local = 0.0;
        self.rel = None;
        self.last = 0.0;
        self.retrigger_aeg = false;
        self.retrigger_feg = false;
    }
}

/// The bend applied twice: x := x - a x^2 + a.
#[inline]
fn bend_twice(x: f32, a: f32) -> f32 {
    let x = x - a * x * x + a;
    x - a * x * x + a
}

#[inline]
fn deform_pow(x: f32, d: f32) -> f32 {
    let c = if d < 0.0 { 1.0 + 0.7 * d } else { 1.0 + 3.0 * d };
    x.max(0.0).powf(c)
}

/// Cycles / steps count from a control-point value.
#[inline]
fn cp_count(cpv: f32) -> f32 {
    (100.0 * ((5.0 * (cpv + 1.0) / 2.0).exp() - 1.0) / (5f32.exp() - 1.0)).floor()
}

fn segment_value(seg: &MsegSegment, a: f32, b: f32, cpv: f32, t: f32, d: f32, run: &mut MsegRun) -> f32 {
    match seg.ty {
        SegmentType::Hold => a,
        SegmentType::None => seg.v0,
        SegmentType::Linear | SegmentType::SCurve => {
            if a == b {
                return a;
            }
            let scurve = seg.ty == SegmentType::SCurve;
            let mut t = t;
            let mut second = false;
            if scurve {
                if d.abs() > 1e-4 {
                    t = ((10.0 * d * t).exp() - 1.0) / ((10.0 * d).exp() - 1.0);
                }
                if t <= 0.5 {
                    t *= 2.0;
                } else {
                    t = 2.0 - 2.0 * t;
                    second = true;
                }
            }
            let mut v = (cpv + 1.0) / 2.0;
            let mut s = 1.0;
            if v < 0.5 {
                v = 1.0 - v;
                s = -1.0;
            }
            let k = if v > 1e-3 {
                let q = ((1.0 - (1.0 - 4.0 * v * (1.0 - v)).max(0.0).sqrt()) / (2.0 * v)).clamp(1e-5, 1e6);
                s * 2.0 * q.ln()
            } else {
                0.0
            };
            let mut c = if k.abs() <= 1e-3 { t } else { ((k * t).exp() - 1.0) / (k.exp() - 1.0) };
            if scurve {
                c = if second { 1.0 - c / 2.0 } else { c / 2.0 };
            } else {
                let x = bend_twice(2.0 * c - 1.0, -0.5 * d.clamp(-3.0, 3.0));
                c = (x + 1.0) / 2.0;
            }
            a + c * (b - a)
        }
        SegmentType::QuadBezier => {
            let dur = seg.duration;
            let time = t * dur;
            let mut tc = seg.cpduration * dur;
            if (tc - dur / 2.0).abs() < 1e-5 {
                tc += 1e-4;
            }
            let m = 0.5 * (a + b);
            let x1 = dur / 2.0 + 2.0 * (tc - dur / 2.0);
            let y1 = m + 2.0 * (cpv - m);
            let qa = dur - 2.0 * x1;
            let qb = 2.0 * x1;
            // Always the "+" root, no range check and no choice between
            // roots; linear only for A == 0 exactly or a negative discriminant.
            let disc = qb * qb + 4.0 * qa * time;
            let beta = if qa == 0.0 || disc < 0.0 { t } else { (-qb + disc.sqrt()) / (2.0 * qa) };
            // Deform warps only when non-zero (a negative root is floored at 0
            // there; the reference's pow would give NaN).
            let beta = if d == 0.0 { beta } else { deform_pow(beta, d) };
            (1.0 - beta) * (1.0 - beta) * a + 2.0 * (1.0 - beta) * beta * y1 + beta * beta * b
        }
        SegmentType::Bump => {
            let k = 20.0 + 500.0 * ((1.0 - d) / 2.0).powi(3);
            let g = (-k * (t - 0.5) * (t - 0.5)).exp();
            a + (b - a) * t + (cpv - 0.5 * (a + b)) * g
        }
        SegmentType::Sine | SegmentType::Sawtooth | SegmentType::Triangle | SegmentType::Square => {
            let n = cp_count(cpv);
            let kappa = match seg.ty {
                SegmentType::Sine => ((1.0 + 2.0 * n) * std::f32::consts::PI * t).cos(),
                SegmentType::Sawtooth => 1.0 - 2.0 * ((n + 1.0) * t).fract(),
                SegmentType::Triangle => {
                    let ph = ((n + 0.5) * t).fract();
                    if ph < 0.5 { 1.0 - 4.0 * ph } else { 4.0 * (ph - 0.5) - 1.0 }
                }
                _ => {
                    if ((n + 1.0) * t).fract() < (d + 1.0) / 2.0 { 1.0 } else { -1.0 }
                }
            };
            let kappa = bend_twice(kappa, -0.5 * d.clamp(-3.0, 3.0));
            b + (a - b) * (kappa + 1.0) / 2.0
        }
        SegmentType::Stairs => {
            let m = cp_count(cpv) + 2.0;
            let f = deform_pow((m * t).floor() / (m - 1.0), d);
            a + f * (b - a)
        }
        SegmentType::SmoothStairs => {
            let m = cp_count(cpv) + 2.0;
            let z = deform_pow(t, d);
            let q = (z * m).floor() / m;
            let r = (z - q) * m;
            let beta = 2.0 * r - 1.0;
            a + (b - a) * ((beta * beta * beta + 1.0) / (2.0 * m) + q)
        }
        SegmentType::Brownian => {
            if t >= 1.0 {
                return b;
            }
            let step = (0.2 * (1.0 - seg.cpduration) * (1.0 - seg.cpduration)).max(1e-4);
            let amp = 0.1 * cpv;
            let mut x = run.brown_x;
            let mut tau = run.brown_tau;
            while tau < t && tau < 1.0 {
                let dt = step.min(1.0 - tau);
                let drift = (b - x) / (1.0 - tau).max(1e-6);
                let rnd = amp * run.rng.bipolar();
                x = (x + drift * dt + rnd).clamp(-1.0, 1.0);
                tau += dt;
            }
            run.brown_x = x;
            run.brown_tau = tau;
            if cpv < 0.0 {
                let q = (-24.0 * cpv).floor() + 1.0;
                x = (x * q).round() / q;
            }
            x
        }
        SegmentType::Ratchet(k) => {
            let kk = k.clamp(1, 8) as f32;
            let h = (kk * t).floor().min(kk - 1.0);
            let lambda = kk * t - h;
            let th = h / kk;
            let cpd = seg.cpduration;
            let start = if th < cpd {
                if cpd > 0.0 { a + (cpv - a) * th / cpd } else { cpv }
            } else if cpd < 1.0 {
                cpv + (b - cpv) * (th - cpd) / (1.0 - cpd)
            } else {
                cpv
            };
            let kc = if d < 0.0 { 3.0 + 9.0 * d.abs() } else { 3.0 - 2.999 * d };
            let c = (1.0 - (-kc * lambda).exp()) / (1.0 - (-kc).exp());
            start + (b - start) * c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lfo_mseg(segs: Vec<MsegSegment>) -> Mseg {
        let mut m = Mseg::empty();
        m.segments = segs;
        m.lfo_mode = true;
        m.loop_end = 99;
        m.rebuild();
        m
    }

    #[test]
    fn linear_and_hold_exact() {
        let mut hold = MsegSegment::linear(0.5, 0.3, 0.0);
        hold.ty = SegmentType::Hold;
        let m = lfo_mseg(vec![MsegSegment::linear(0.5, -1.0, 0.3), hold]);
        let mut run = MsegRun::new(1);
        for i in 0..100 {
            let u = i as f64 / 100.0;
            let v = m.eval(&mut run, u, 0.0, false);
            let want = if u < 0.5 { -1.0 + 1.3 * (u as f32 / 0.5) } else { 0.3 };
            assert!((v - want).abs() < 1e-6, "{u} {v} {want}");
        }
        // LFO mode wraps.
        assert!((m.eval(&mut run, 3.25, 0.0, false) - (-1.0 + 1.3 * 0.5)).abs() < 1e-6);
        // Unused deform leaves a linear segment linear only when use_deform is off.
        let mut s = MsegSegment::linear(1.0, -1.0, 1.0);
        s.use_deform = false;
        let m2 = lfo_mseg(vec![s]);
        assert!((m2.eval(&mut run, 0.25, 0.8, false) - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn retrigger_flags_on_entry() {
        let mut s1 = MsegSegment::linear(0.5, 0.0, 1.0);
        s1.retrigger_aeg = true;
        let m = lfo_mseg(vec![MsegSegment::linear(0.5, 0.0, 0.0), s1]);
        let mut run = MsegRun::new(1);
        m.eval(&mut run, 0.1, 0.0, false);
        assert!(!run.retrigger_aeg);
        m.eval(&mut run, 0.6, 0.0, false);
        assert!(run.retrigger_aeg);
        m.eval(&mut run, 0.7, 0.0, false);
        assert!(!run.retrigger_aeg);
    }

    #[test]
    fn default_voice_gated_loop() {
        let m = Mseg::default_voice();
        assert_eq!(m.total(), 4.0);
        let mut run = MsegRun::new(1);
        assert!((m.eval(&mut run, 0.5, 0.0, false) - 0.5).abs() < 1e-6);
        assert!((m.eval(&mut run, 1.5, 0.0, false) - 0.75).abs() < 1e-6);
        // Sustain plateau loops while gated.
        for k in 0..20 {
            assert!((m.eval(&mut run, 2.3 + k as f64, 0.0, false) - 0.5).abs() < 1e-6);
        }
        // Release: the last segment plays from the plateau value.
        let u0 = 30.0;
        assert!((m.eval(&mut run, u0, 0.0, true) - 0.5).abs() < 1e-6);
        assert!((m.eval(&mut run, u0 + 0.5, 0.0, true) - 0.25).abs() < 1e-6);
        assert_eq!(m.eval(&mut run, u0 + 1.5, 0.0, true), 0.0);
    }

    #[test]
    fn default_scene_triangle() {
        let m = Mseg::default_scene();
        let mut run = MsegRun::new(1);
        for (u, want) in [(0.125, 0.5), (0.25, 1.0), (0.5, 0.0), (0.75, -1.0), (1.125, 0.5)] {
            assert!((m.eval(&mut run, u, 0.0, false) - want).abs() < 1e-6, "{u}");
        }
    }

    #[test]
    fn one_shot_and_loop_markers() {
        let mut m = Mseg::empty();
        m.segments = vec![MsegSegment::linear(1.0, 0.0, 1.0), MsegSegment::linear(1.0, 1.0, -0.5)];
        m.loop_mode = LoopMode::OneShot;
        m.set_loop_markers(None, None, Some(-1), Some(-1));
        assert_eq!((m.loop_start, m.loop_end), (0, 1));
        m.rebuild();
        let mut run = MsegRun::new(1);
        assert_eq!(m.eval(&mut run, 5.0, 0.0, false), -0.5);
        m.loop_mode = LoopMode::Loop;
        m.set_loop_markers(Some(1), Some(1), None, None);
        m.rebuild();
        assert!((m.eval(&mut run, 0.5, 0.0, false) - 0.5).abs() < 1e-6);
        assert!((m.eval(&mut run, 3.5, 0.0, false) - 0.25).abs() < 1e-6);
        // Reversed markers sit at the loop end's value.
        m.set_loop_markers(Some(2), Some(0), None, None);
        m.rebuild();
        assert_eq!(m.eval(&mut run, 1.5, 0.0, false), 1.0);
    }

    #[test]
    fn shapes_stay_in_range_and_hit_endpoints() {
        for id in 1..=21 {
            let mut s = MsegSegment::linear(1.0, -0.8, 0.6);
            s.ty = SegmentType::from_id(id);
            s.cpv = 0.3;
            assert_eq!(s.ty.id(), id);
            let mut end = MsegSegment::linear(1.0, 0.6, 0.6);
            end.ty = SegmentType::Hold;
            let mut m = Mseg::empty();
            m.segments = vec![s, end];
            m.loop_mode = LoopMode::OneShot;
            m.rebuild();
            let mut run = MsegRun::new(7);
            for d in [-1.0f32, -0.3, 0.0, 0.5, 1.0] {
                run.reset();
                for i in 0..200 {
                    let v = m.eval(&mut run, i as f64 / 200.0, d, false);
                    assert!(v.is_finite() && (-1.0..=1.0).contains(&v), "type {id} d {d}");
                }
            }
        }
    }

    #[test]
    fn stairs_and_bezier() {
        let mut s = MsegSegment::linear(1.0, 0.0, 1.0);
        s.ty = SegmentType::Stairs;
        s.cpv = -1.0; // M = 2 steps: 0 then 1
        let m = lfo_mseg(vec![s]);
        let mut run = MsegRun::new(1);
        assert_eq!(m.eval(&mut run, 0.25, 0.0, false), 0.0);
        assert_eq!(m.eval(&mut run, 0.75, 0.0, false), 1.0);
        // A Bezier with its control point on the chord is (nearly) linear.
        let mut q = MsegSegment::linear(1.0, -1.0, 1.0);
        q.ty = SegmentType::QuadBezier;
        q.cpduration = 0.25;
        q.cpv = -0.5;
        let m = lfo_mseg(vec![q]);
        for i in 1..10 {
            let u = i as f64 / 10.0;
            let v = m.eval(&mut run, u, 0.0, false);
            assert!((v - (2.0 * u as f32 - 1.0)).abs() < 1e-3, "{u} {v}");
        }
    }
    #[test]
    fn lfo_mode_gated_loop_releases_into_post_loop() {
        // 0 -> 1 (0.5), 1 -> 0 (0.5); gated loop on segment 0, LFO edit mode.
        // (Ending at 0 keeps the moved control value at 0: linear.)
        let mut m = Mseg::empty();
        m.segments = vec![MsegSegment::linear(0.5, 0.0, 1.0), MsegSegment::linear(0.5, 1.0, 0.0)];
        m.lfo_mode = true;
        m.loop_mode = LoopMode::GatedLoop;
        m.loop_start = 0;
        m.loop_end = 0;
        m.rebuild();
        let mut run = MsegRun::new(1);
        // Held: markers ignored, the whole MSEG cycles.
        assert!((m.eval(&mut run, 2.875, 0.0, false) - 0.25).abs() < 1e-6);
        assert!((m.eval(&mut run, 3.25, 0.0, false) - 0.5).abs() < 1e-6);
        // Released at u = 3.25 (value 0.5): segment 1 plays from 0.5 toward 0.
        assert!((m.eval(&mut run, 3.25, 0.0, true) - 0.5).abs() < 1e-6);
        assert!((m.eval(&mut run, 3.5, 0.0, true) - 0.25).abs() < 1e-6);
        assert_eq!(m.eval(&mut run, 3.9, 0.0, true), 0.0);
        // Loop end on the last segment: hold the release value.
        m.loop_end = 1;
        m.rebuild();
        run.reset();
        assert!((m.eval(&mut run, 0.25, 0.0, false) - 0.5).abs() < 1e-6);
        for u in [0.3, 0.8, 5.0] {
            assert!((m.eval(&mut run, u, 0.0, true) - 0.5).abs() < 1e-6, "{u}");
        }
        // Plain loop mode in LFO edit mode keeps cycling after release.
        m.loop_mode = LoopMode::Loop;
        m.rebuild();
        run.reset();
        assert!((m.eval(&mut run, 1.875, 0.0, true) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn none_segment_uses_stored_v0() {
        // Gated loop on segment 0; post-loop segment 1 is None with stored v0 0.8.
        let mut none = MsegSegment::linear(1.0, 0.8, 0.0);
        none.ty = SegmentType::None;
        let mut m = Mseg::empty();
        m.segments = vec![MsegSegment::linear(1.0, 0.0, 0.8), none];
        m.loop_mode = LoopMode::GatedLoop;
        m.loop_start = 0;
        m.loop_end = 0;
        m.rebuild();
        let mut run = MsegRun::new(1);
        assert!((m.eval(&mut run, 0.5, 0.0, false) - 0.4).abs() < 1e-6);
        // Release at 0.4: None ignores the release start value, Hold uses it.
        assert!((m.eval(&mut run, 0.5, 0.0, true) - 0.8).abs() < 1e-6);
        m.segments[1].ty = SegmentType::Hold;
        m.rebuild();
        run.reset();
        assert!((m.eval(&mut run, 0.5, 0.0, false) - 0.4).abs() < 1e-6);
        assert!((m.eval(&mut run, 0.5, 0.0, true) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn bezier_takes_the_plus_root_unchecked() {
        // Control point at the segment end: x1 = 1.5 D, A = -2, B = 3; the
        // "+" root only reaches beta = 1/2 at the end of the segment, so the
        // curve ends at the Bezier's midpoint (0 here), not at b.
        let mut q = MsegSegment::linear(1.0, -1.0, 1.0);
        q.ty = SegmentType::QuadBezier;
        q.cpduration = 1.0;
        q.cpv = 0.0;
        let m = lfo_mseg(vec![q]);
        let mut run = MsegRun::new(1);
        for i in 0..10 {
            let time = i as f32 / 10.0 + 0.05;
            let beta = (3.0 - (9.0 - 8.0 * time).sqrt()) / 4.0;
            let want = -(1.0 - beta) * (1.0 - beta) + beta * beta;
            let v = m.eval(&mut run, time as f64, 0.0, false);
            assert!((v - want).abs() < 1e-5, "{time} {v} {want}");
        }
        assert!(m.eval(&mut run, 0.999, 0.0, false).abs() < 5e-3);
    }
}
