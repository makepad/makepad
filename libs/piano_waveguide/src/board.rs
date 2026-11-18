//! The soundboard: what the bridge motion becomes on its way to the ears.
//!
//! Below about 2 kHz the board is a set of discrete, damped modes; each
//! bridge region drives each mode with its own strength and each mode
//! radiates to both ears. Above that the board's modes overlap into a smooth
//! response, which is a direct path here. Each mode's shape along the bridge
//! is sampled at a few places (`BoardShape::positions`) from the bass end to
//! the treble end; a key feeds the two places nearest it.

use crate::voice::jitter;
use std::f64::consts::TAU;

pub const BUSES: usize = 4;
pub const BLOCK: usize = 32;
const CASE_MODES: usize = 28;
const SOUND_SPEED: f64 = 343.0;
/// The listener's ears (two microphones) are this far above the board.
const LISTENER_HEIGHT: f64 = 0.6;
/// Length of the board along the bridge, m: bass end at 0.
const BOARD_LENGTH: f64 = 1.4;

/// One stage of the radiation high-pass: nothing radiates well below the
/// board's first modes, where front and back of the board cancel.
#[derive(Clone, Copy, Default)]
struct HighPass {
    b0: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl HighPass {
    fn new(hz: f64, q: f64, fs: f64) -> Self {
        let k = (std::f64::consts::PI * hz / fs).tan();
        let norm = 1.0 / (1.0 + k / q + k * k);
        HighPass { b0: norm, a1: 2.0 * (k * k - 1.0) * norm, a2: (1.0 - k / q + k * k) * norm, s1: 0.0, s2: 0.0 }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        // b = b0 * [1, -2, 1]
        let y = self.b0 * x + self.s1;
        self.s1 = -2.0 * self.b0 * x - self.a1 * y + self.s2;
        self.s2 = self.b0 * x - self.a2 * y;
        y
    }
}

#[derive(Clone)]
pub struct Board {
    modes: usize,
    /// Places along the bridge where the mode shapes are sampled.
    points: usize,
    cr: Vec<f32>,
    ci: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Each mode's shape at each place, mode by mode.
    drive: Vec<f32>,
    /// Each mode's gain to each ear, in phase with its state and a quarter
    /// turn behind it: together an amplitude and a delay.
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    lag_l: Vec<f32>,
    lag_r: Vec<f32>,
    /// Removes the half sample of input a sampled cosine response starts with.
    through_l: Vec<f32>,
    through_r: Vec<f32>,
    direct_l: Vec<f32>,
    direct_r: Vec<f32>,
    direct_coef: f32,
    direct_low: Vec<f32>,
    /// The places that carry motion in the block being run.
    live: Vec<usize>,
    radiation: [[HighPass; 2]; 2],
    /// What reaches the ears without the board's resonances: gain, the
    /// one-pole coefficient of its low corner, and that filter's state per ear.
    floor: f32,
    floor_coef: f32,
    floor_low: [f32; 2],
    input: [f32; BLOCK],
    /// The case, frame and keybed: a few slow modes that only a hammer blow
    /// excites. Rotation, state and gain to each ear per mode.
    case: Vec<[f32; 6]>,
    case_rings: bool,
    knock: f32,
}

/// The board's power gain to the ears (the mean of the two) at each place
/// on the bridge, smoothed over a third of an octave, on a grid of sixtieths
/// of an octave from 20 Hz; with the cross term of each pair of neighbouring
/// places, so a key that drives two of them reads exactly. What a technician
/// evens by ear: the register's trend, not where one of the board's modes
/// happens to fall on a key's fundamental.
#[derive(Clone)]
pub struct Smoothed {
    lo: f64,
    per: f64,
    own: Vec<Vec<f64>>,
    next: Vec<Vec<f64>>,
}

impl Smoothed {
    /// The smoothed power gain at `f` Hz for motion shared between place
    /// `point` (1 - `mix`) and the next one (`mix`).
    pub fn power(&self, point: usize, mix: f64, f: f64) -> f64 {
        let n = self.own[0].len();
        let x = ((f / self.lo).log2() * self.per).clamp(0.0, (n - 1) as f64);
        let (g, t) = ((x.floor() as usize).min(n - 2), x - x.floor().min((n - 2) as f64));
        let at = |v: &Vec<f64>| v[g] + t * (v[g + 1] - v[g]);
        let (a, b) = (1.0 - mix, mix);
        let mut power = a * a * at(&self.own[point]);
        if b > 0.0 {
            power += b * b * at(&self.own[point + 1]) + 2.0 * a * b * at(&self.next[point]);
        }
        power
    }
}

/// Tunable shape of the board.
#[derive(Clone, Copy, Debug)]
pub struct BoardShape {
    pub first_mode: f64,
    pub last_mode: f64,
    pub spacing: f64,
    pub loss_low: f64,
    pub loss_high: f64,
    pub radiation_corner: f64,
    /// Modes below this radiate progressively less, Hz.
    pub efficient_above: f64,
    /// Modes below this carry more of the sound (a board is livelier in
    /// its low-middle than a flat mobility says), Hz.
    pub warmth: f64,
    pub direct_corner: f64,
    pub direct_gain: f64,
    /// Distance between the two listening points, m.
    pub width: f64,
    /// Above this the pair closes up in proportion to the wavelength, Hz.
    pub coincident_above: f64,
    /// How much of the bridge motion one string causes reaches the other
    /// strings of its region: what makes undamped strings ring along.
    pub coupling: f64,
    /// Level of the sound that does not pass through the board's
    /// resonances (the board moving as a whole below its first mode, the
    /// rim and the case), against the board's own in its middle range. It
    /// is what is left of a low fundamental, and it fills the gaps between
    /// the board's resonances.
    pub floor: f64,
    /// Strength of the duplex segments: the share of a region's bridge
    /// motion its 16 modes see. A key's own three segments see all of it and
    /// its neighbours' a quarter, which comes to 0.16 for the bank (0 = none).
    pub duplex: f64,
    /// How loudly the case answers a hammer blow (0 = no knock).
    pub knock: f64,
    /// Places along the bridge at which each mode's shape is sampled; a key
    /// drives a mode with the shape interpolated between the two places
    /// nearest it. 88 puts a place under every key: the shape is evaluated
    /// where the key is.
    pub positions: usize,
    /// Share of each mode's difference in arrival time between the two
    /// listening points that is kept: 1 as the geometry gives it, 0 for
    /// level differences only (both points hear every mode with the timing
    /// of the point between them).
    pub ear_phase: f64,
}

impl Default for BoardShape {
    fn default() -> Self {
        BoardShape {
            first_mode: 78.0,
            last_mode: 2000.0,
            spacing: 9.0,
            loss_low: 0.06,
            loss_high: 0.03,
            radiation_corner: 90.0,
            efficient_above: 100.0,
            warmth: 300.0,
            direct_corner: 1700.0,
            direct_gain: 1.0,
            width: 0.4,
            coincident_above: 300.0,
            coupling: 0.25,
            floor: 0.14,
            duplex: 0.16,
            knock: 1.5,
            positions: BUSES,
            ear_phase: 1.0,
        }
    }
}

impl Board {
    pub fn new(fs: f64, shape: &BoardShape) -> Self {
        let mut freqs = Vec::new();
        let mut f = shape.first_mode;
        while f < shape.last_mode {
            freqs.push(f);
            f += shape.spacing * (1.0 + 0.6 * jitter(freqs.len() as u32, 11));
        }
        let modes = freqs.len();
        let points = shape.positions.max(2);
        let mut board = Board {
            modes,
            points,
            cr: vec![0.0; modes],
            ci: vec![0.0; modes],
            re: vec![0.0; modes],
            im: vec![0.0; modes],
            drive: vec![0.0; modes * points],
            out_l: vec![0.0; modes],
            out_r: vec![0.0; modes],
            lag_l: vec![0.0; modes],
            lag_r: vec![0.0; modes],
            through_l: vec![0.0; points],
            through_r: vec![0.0; points],
            direct_l: vec![0.0; points],
            direct_r: vec![0.0; points],
            direct_coef: (-TAU * shape.direct_corner / fs).exp() as f32,
            direct_low: vec![0.0; points],
            live: Vec::with_capacity(points),
            // fourth-order Butterworth as two stages
            radiation: [[HighPass::new(shape.radiation_corner, 0.5412, fs), HighPass::new(shape.radiation_corner, 1.3066, fs)]; 2],
            floor: shape.floor as f32,
            floor_coef: (-TAU * 60.0 / fs).exp() as f32,
            floor_low: [0.0; 2],
            input: [0.0; BLOCK],
            case_rings: false,
            knock: shape.knock as f32,
            case: (0..CASE_MODES)
                .map(|m| {
                    // 180 Hz to 1.9 kHz, ringing 0.35 s at the bottom and
                    // 0.12 s at the top
                    let t = (m as f64 + 0.5 * jitter(m as u32, 21)) / (CASE_MODES - 1) as f64;
                    let f = 180.0 * (1900.0f64 / 180.0).powf(t.clamp(0.0, 1.0));
                    let radius = (-6.908 / ((0.35 - 0.23 * t) * fs)).exp();
                    let w = TAU * f / fs;
                    let side = std::f64::consts::FRAC_PI_4 * (1.0 + 0.7 * jitter(m as u32, 22));
                    // newtons to output: a 30 N blow of a millisecond is
                    // a knock some 25 dB under a mezzo-forte note
                    let gain = 2.0e-5 * 48_000.0 / fs * jitter(m as u32, 23).signum();
                    [(radius * w.cos()) as f32, (radius * w.sin()) as f32, 0.0, 0.0, (gain * side.cos()) as f32, (gain * side.sin()) as f32]
                })
                .collect(),
        };
        // Two listening points above the board, bass side left, `width`
        // of a metre apart; sound from a place on the board reaches each
        // weaker and later with distance.
        let ears = [0.5 * BOARD_LENGTH - 0.5 * shape.width, 0.5 * BOARD_LENGTH + 0.5 * shape.width];
        let heard = |place: f64, ear: f64| {
            let distance = ((place - ear).powi(2) + LISTENER_HEIGHT * LISTENER_HEIGHT).sqrt();
            (LISTENER_HEIGHT / distance, (distance - LISTENER_HEIGHT) / SOUND_SPEED)
        };
        let mut gains = vec![[(0.0f64, 0.0f64); 2]; modes];
        let mut couplings = vec![vec![0.0f64; points]; modes];
        for (m, &f) in freqs.iter().enumerate() {
            let id = m as u32;
            let loss = shape.loss_low + (shape.loss_high - shape.loss_low) * (f / 1000.0).min(1.0);
            let radius = (-std::f64::consts::PI * loss * f / fs).exp();
            let w = TAU * f / fs;
            board.cr[m] = (radius * w.cos()) as f32;
            board.ci[m] = (radius * w.sin()) as f32;
            // Mode shape along the bridge: longer waves for the low modes.
            let wavenumber = 0.5 + 0.9 * (f / 100.0).sqrt() * (1.0 + 0.3 * jitter(id, 12));
            let phase = std::f64::consts::PI * jitter(id, 13);
            for b in 0..points {
                couplings[m][b] = (wavenumber * b as f64 / (points - 1) as f64 * std::f64::consts::PI + phase).sin();
            }
            // Low modes radiate to both ears alike; higher ones differ.
            // Where on the board the mode radiates from. The lowest modes
            // move the whole board; higher ones have their own place.
            let place = 0.5 * BOARD_LENGTH * (1.0 + jitter(id, 14) * (f / 250.0).min(1.0));
            // The lowest modes move the whole board against itself and
            // radiate less.
            let efficiency = (f / shape.efficient_above).min(1.0).powi(3) * (1.0 + (shape.warmth / f).powi(2)).sqrt();
            // Above `coincident_above` the two points move together in proportion, as
            // a spaced pair gives way to a coincident one: the difference
            // in arrival stops growing in periods, so one note does not
            // land hard on one side.
            let closer = (shape.coincident_above / f).min(1.0);
            for (ear, x) in ears.iter().enumerate() {
                let (amplitude, _) = heard(place, *x);
                let (_, delay) = heard(place, 0.5 * BOARD_LENGTH + shape.ear_phase * closer * (*x - 0.5 * BOARD_LENGTH));
                let (lag, gain) = (TAU * f * delay).sin_cos();
                gains[m][ear] = (efficiency * amplitude * gain, efficiency * amplitude * lag);
            }
        }
        for m in 0..modes {
            board.out_l[m] = gains[m][0].0 as f32;
            board.out_r[m] = gains[m][1].0 as f32;
            board.lag_l[m] = gains[m][0].1 as f32;
            board.lag_r[m] = gains[m][1].1 as f32;
            for b in 0..points {
                board.drive[m * points + b] = couplings[m][b] as f32;
                board.through_l[b] -= (0.5 * gains[m][0].0 * couplings[m][b]) as f32;
                board.through_r[b] -= (0.5 * gains[m][1].0 * couplings[m][b]) as f32;
            }
        }
        // Scale so a bus reaches the ears at unit RMS gain over the body
        // band; the direct path then takes over at the same level.
        let (mut power, mut count, mut f) = (0.0, 0.0, 300.0);
        let mut mix = vec![0.0; points];
        while f < 1500.0 {
            for b in 0..points {
                mix.fill(0.0);
                mix[b] = 1.0;
                let (l, r) = board.response(&mix, f, fs);
                power += 0.5 * (l + r);
                count += 1.0;
            }
            f += 7.3;
        }
        let norm = (count / power).sqrt() as f32;
        for g in board.out_l.iter_mut().chain(board.out_r.iter_mut()).chain(board.lag_l.iter_mut()).chain(board.lag_r.iter_mut()).chain(board.through_l.iter_mut()).chain(board.through_r.iter_mut()) {
            *g *= norm;
        }
        for b in 0..points {
            // As the player hears it: bass to the left, treble to the right.
            // The direct path leaves from the region's place on the bridge
            // and differs between the ears in level only: a key feeds two
            // regions, and two arrival times would comb its treble.
            let place = BOARD_LENGTH * (0.15 + 0.7 * b as f64 / (points - 1) as f64);
            let (left, right) = (heard(place, ears[0]), heard(place, ears[1]));
            board.direct_l[b] = (shape.direct_gain * left.0 * std::f64::consts::SQRT_2) as f32;
            board.direct_r[b] = (shape.direct_gain * right.0 * std::f64::consts::SQRT_2) as f32;
        }
        board
    }

    /// Places along the bridge where the mode shapes are sampled.
    pub fn points(&self) -> usize {
        self.points
    }

    /// Power gain to the left and the right ear at `f` Hz for bridge motion
    /// spread over the places by `mix`: the whole path `process` runs.
    pub fn response(&self, mix: &[f64], f: f64, fs: f64) -> (f64, f64) {
        let [l, r] = self.transfer(mix, f, fs);
        (l.0 * l.0 + l.1 * l.1, r.0 * r.0 + r.1 * r.1)
    }

    /// The same path as a complex gain (re, im) to each ear.
    pub fn transfer(&self, mix: &[f64], f: f64, fs: f64) -> [(f64, f64); 2] {
        let w = TAU * f / fs;
        let (sin, cos) = w.sin_cos();
        // one-pole low-pass of the direct path's crossover, then its complement
        let c = self.direct_coef as f64;
        let (dr, di) = (1.0 - c * cos, c * sin);
        let d = dr * dr + di * di;
        let (high_re, high_im) = (1.0 - (1.0 - c) * dr / d, (1.0 - c) * di / d);
        let mut left = (0.0, 0.0);
        let mut right = (0.0, 0.0);
        for b in 0..self.points {
            let (tl, tr, dl, dr) = (self.through_l[b] as f64, self.through_r[b] as f64, self.direct_l[b] as f64, self.direct_r[b] as f64);
            left.0 += mix[b] * (tl + dl * high_re);
            left.1 += mix[b] * dl * high_im;
            right.0 += mix[b] * (tr + dr * high_re);
            right.1 += mix[b] * dr * high_im;
        }
        for m in 0..self.modes {
            let (pr, pi) = (self.cr[m] as f64, self.ci[m] as f64);
            // z[n] = p z[n-1] + x[n]: A and B are its response and that of
            // its conjugate; Re z = (A + B) / 2, Im z = (A - B) / 2j.
            let mut ab = [(0.0, 0.0); 2];
            for (k, sign) in [1.0, -1.0].into_iter().enumerate() {
                let (dr, di) = (1.0 - (pr * cos + sign * pi * sin), pr * sin - sign * pi * cos);
                let d = dr * dr + di * di;
                ab[k] = (dr / d, -di / d);
            }
            let re = (0.5 * (ab[0].0 + ab[1].0), 0.5 * (ab[0].1 + ab[1].1));
            let im = (0.5 * (ab[0].1 - ab[1].1), -0.5 * (ab[0].0 - ab[1].0));
            let shape = &self.drive[m * self.points..(m + 1) * self.points];
            let drive: f64 = (0..self.points).map(|b| mix[b] * shape[b] as f64).sum();
            let (gl, gr, ll, lr) = (drive * self.out_l[m] as f64, drive * self.out_r[m] as f64, drive * self.lag_l[m] as f64, drive * self.lag_r[m] as f64);
            left.0 += gl * re.0 + ll * im.0;
            left.1 += gl * re.1 + ll * im.1;
            right.0 += gr * re.0 + lr * im.0;
            right.1 += gr * re.1 + lr * im.1;
        }
        // radiation corner: each stage b0 (1 - z^-1)^2 over 1 + a1 z^-1 + a2 z^-2
        let (sin2, cos2) = (2.0 * w).sin_cos();
        let mut radiation = (1.0, 0.0);
        for stage in &self.radiation[0] {
            let num = (stage.b0 * (1.0 - 2.0 * cos + cos2), stage.b0 * (2.0 * sin - sin2));
            let den = (1.0 + stage.a1 * cos + stage.a2 * cos2, -stage.a1 * sin - stage.a2 * sin2);
            let d = den.0 * den.0 + den.1 * den.1;
            let h = ((num.0 * den.0 + num.1 * den.1) / d, (num.1 * den.0 - num.0 * den.1) / d);
            radiation = (radiation.0 * h.0 - radiation.1 * h.1, radiation.0 * h.1 + radiation.1 * h.0);
        }
        // the floor's own corner: 1 minus a one-pole low-pass
        let c = self.floor_coef as f64;
        let (dr, di) = (1.0 - c * cos, c * sin);
        let d = dr * dr + di * di;
        let high = (1.0 - (1.0 - c) * dr / d, (1.0 - c) * di / d);
        let ear = |sum: (f64, f64), direct: &[f32]| {
            let floor = self.floor as f64 * (0..self.points).map(|b| mix[b] * direct[b] as f64).sum::<f64>();
            let re = sum.0 * radiation.0 - sum.1 * radiation.1 + floor * high.0;
            let im = sum.0 * radiation.1 + sum.1 * radiation.0 + floor * high.1;
            (re, im)
        };
        [ear(left, &self.direct_l), ear(right, &self.direct_r)]
    }

    /// The power gain to the ears smoothed over a third of an octave (see
    /// [`Smoothed`]).
    pub fn smoothed(&self, fs: f64) -> Smoothed {
        let (lo, per) = (20.0, 60.0);
        let n = ((0.45 * fs / lo).log2() * per) as usize + 1;
        let points = self.points;
        let mut unit = vec![0.0; points];
        let (mut own, mut next) = (vec![vec![0.0; n]; points], vec![vec![0.0; n]; points - 1]);
        for g in 0..n {
            let f = lo * 2f64.powf(g as f64 / per);
            let t: Vec<[(f64, f64); 2]> = (0..points)
                .map(|b| {
                    unit.fill(0.0);
                    unit[b] = 1.0;
                    self.transfer(&unit, f, fs)
                })
                .collect();
            for b in 0..points {
                own[b][g] = 0.5 * t[b].iter().map(|e| e.0 * e.0 + e.1 * e.1).sum::<f64>();
                if b + 1 < points {
                    next[b][g] = 0.5 * t[b].iter().zip(&t[b + 1]).map(|(a, c)| a.0 * c.0 + a.1 * c.1).sum::<f64>();
                }
            }
        }
        // a third of an octave: ten sixtieths either side
        let smooth = |x: &Vec<f64>| -> Vec<f64> {
            (0..n)
                .map(|g| {
                    let (a, z) = (g.saturating_sub(10), (g + 11).min(n));
                    x[a..z].iter().sum::<f64>() / (z - a) as f64
                })
                .collect()
        };
        Smoothed { lo, per, own: own.iter().map(smooth).collect(), next: next.iter().map(smooth).collect() }
    }

    /// How loudly the case answers a blow (see [`BoardShape::knock`]).
    pub fn set_knock(&mut self, knock: f64) {
        self.knock = knock.max(0.0) as f32;
    }

    pub fn clear(&mut self) {
        self.re.fill(0.0);
        self.im.fill(0.0);
        self.direct_low.fill(0.0);
        self.floor_low = [0.0; 2];
        for mode in &mut self.case {
            mode[2] = 0.0;
            mode[3] = 0.0;
        }
        for stage in self.radiation.iter_mut().flatten() {
            stage.s1 = 0.0;
            stage.s2 = 0.0;
        }
    }

    /// `bus[b][..n]` is the bridge velocity at each place (`points` of
    /// them); writes the radiated sound to `l[..n]`, `r[..n]`. `blow` is
    /// the summed hammer force, which the case answers; `struck` says
    /// whether any is non-zero.
    pub fn process(&mut self, bus: &[[f32; BLOCK]], blow: &[f32; BLOCK], struck: bool, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len()).min(BLOCK);
        let mut acc_l = [0.0f32; BLOCK];
        let mut acc_r = [0.0f32; BLOCK];
        let mut floor_l = [0.0f32; BLOCK];
        let mut floor_r = [0.0f32; BLOCK];
        if (struck && self.knock > 0.0) || self.case_rings {
            let mut energy = 0.0f32;
            for mode in &mut self.case {
                let [cr, ci, mut re, mut im, gl, gr] = *mode;
                let (gl, gr) = (gl * self.knock, gr * self.knock);
                for i in 0..n {
                    let nr = cr * re - ci * im + blow[i];
                    im = ci * re + cr * im;
                    re = nr;
                    // the sine-phase state: no step at the blow itself
                    acc_l[i] += gl * im;
                    acc_r[i] += gr * im;
                }
                mode[2] = re;
                mode[3] = im;
                energy += re * re + im * im;
            }
            self.case_rings = energy > 1e-12;
            if !self.case_rings {
                for mode in &mut self.case {
                    mode[2] = 0.0;
                    mode[3] = 0.0;
                }
            }
        }
        self.live.clear();
        for b in 0..self.points {
            let (tl, tr, dl, dr) = (self.through_l[b], self.through_r[b], self.direct_l[b], self.direct_r[b]);
            if bus[b][..n].iter().any(|&x| x != 0.0) {
                self.live.push(b);
            }
            let mut low = self.direct_low[b];
            for i in 0..n {
                let x = bus[b][i];
                low = x + self.direct_coef * (low - x);
                let high = x - low;
                acc_l[i] += tl * x + dl * high;
                acc_r[i] += tr * x + dr * high;
                floor_l[i] += dl * x;
                floor_r[i] += dr * x;
            }
            self.direct_low[b] = low;
        }
        let points = self.points;
        for m in 0..self.modes {
            // a place that carries nothing adds nothing: skip it
            let shape = &self.drive[m * points..(m + 1) * points];
            self.input[..n].fill(0.0);
            for &b in &self.live {
                let (d, x) = (shape[b], &bus[b]);
                for i in 0..n {
                    self.input[i] += d * x[i];
                }
            }
            let (cr, ci, gl, gr, ll, lr) = (self.cr[m], self.ci[m], self.out_l[m], self.out_r[m], self.lag_l[m], self.lag_r[m]);
            let (mut re, mut im) = (self.re[m], self.im[m]);
            for i in 0..n {
                let nr = cr * re - ci * im + self.input[i];
                im = ci * re + cr * im;
                re = nr;
                acc_l[i] += gl * re + ll * im;
                acc_r[i] += gr * re + lr * im;
            }
            self.re[m] = re;
            self.im[m] = im;
        }
        for i in 0..n {
            let [left, right] = &mut self.radiation;
            let x = left[0].run(acc_l[i] as f64);
            let y = right[0].run(acc_r[i] as f64);
            // the floor passes beside the radiation corner, above 60 Hz
            let [low_l, low_r] = &mut self.floor_low;
            *low_l = floor_l[i] + self.floor_coef * (*low_l - floor_l[i]);
            *low_r = floor_r[i] + self.floor_coef * (*low_r - floor_r[i]);
            l[i] = left[1].run(x) as f32 + self.floor * (floor_l[i] - *low_l);
            r[i] = right[1].run(y) as f32 + self.floor * (floor_r[i] - *low_r);
        }
    }
}
