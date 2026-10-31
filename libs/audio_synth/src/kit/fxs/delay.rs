//! Delay (type 1): two block-written delay lines (left, right) read with
//! the windowed-sinc kernel, feedback and crossfeed with selectable
//! clipping, low/high cut in the loop, a rounded-triangle time modulation
//! and two line modes: Tape (time changes glide, bending pitch) and Clean
//! (time changes crossfade between taps).

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotUnit, SlotValue};

const LEN: usize = 1 << 18;
const LINE_MODES: &[&str] = &["Tape", "Clean"];
const CLIP_MODES: &[&str] = &["Off", "Soft", "Tanh", "Hard", "Hard +-8"];
/// Per-sample lag of the read time (tape glide).
const GLIDE: f32 = 1e-4;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        deform(envtime("Left", -8.0, 5.0, -2.0), LINE_MODES),
        deform(about(deact(envtime("Right", -8.0, 5.0, -2.0)), "deactivated = linked to Left"), LINE_MODES),
        deform(about(extend(percent("Feedback", 0.5)), "extended = bipolar (negative inverts the repeats)"), CLIP_MODES),
        about(extend(percent("Crossfeed", 0.0)), "extended = bipolar"),
        freq_d("Low Cut", -24.0),
        freq_d("High Cut", 30.0),
        lforate("Rate", -2.0),
        about(extend(fl("Depth", 0.0, 2.0, 0.0, SlotUnit::None)), "time modulation depth, x100 = cents (extended x6)"),
        about(bipolar("Channel", 0.0), "input pan: -1 left only, +1 right only"),
        unused(),
        percent("Mix", 0.5),
        width_db("Width"),
    ]
}

pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[2].deform = 1;
}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 15 {
        p[4].deactivated = false;
        p[5].deactivated = false;
        p[1].deactivated = false;
    }
    if rev <= 17 {
        p[2].deform = 1;
    }
    if rev <= 18 {
        p[2].extend = false;
    }
    if rev <= 21 {
        p[3].extend = false;
        p[7].extend = false;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

#[derive(Clone, Copy, Debug, Default)]
struct Chan {
    /// The current read time (samples, kernel centre already subtracted).
    time: f32,
    /// The glided modulation offset.
    lfo: f32,
    /// Clean mode: the current tap's base time.
    base: f32,
    clean: bool,
    xf_from: f32,
    xf_pos: usize,
    xf_len: usize,
}

pub struct Delay {
    sr: f32,
    line: [BlockDelay; 2],
    lp: Biquad,
    hp: Biquad,
    phase: f32,
    dir: f32,
    m: f32,
    ch: [Chan; 2],
    first: bool,
    fb: Line,
    cf: Line,
    mix: Line,
    width: Line,
    pan: Line,
}

#[inline]
fn clip(mode: i32, x: f32) -> f32 {
    match mode {
        1 => softclip(x),
        2 => tanh7(x),
        3 => hardclip(x),
        4 => hardclip8(x),
        _ => x,
    }
}

impl Delay {
    pub fn new(sr: f32) -> Delay {
        Delay {
            sr,
            line: [BlockDelay::new(LEN), BlockDelay::new(LEN)],
            lp: Biquad::new(),
            hp: Biquad::new(),
            phase: 0.0,
            dir: 1.0,
            m: 0.0,
            ch: [Chan::default(); 2],
            first: true,
            fb: Line::new(),
            cf: Line::new(),
            mix: Line::new(),
            width: Line::new(),
            pan: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
        self.lp = Biquad::new();
        self.hp = Biquad::new();
        self.phase = 0.0;
        self.dir = 1.0;
        self.m = 0.0;
        self.ch = [Chan::default(); 2];
        self.first = true;
        self.fb = Line::new();
        self.cf = Line::new();
        self.mix = Line::new();
        self.width = Line::new();
        self.pan = Line::new();
    }

    /// The modulation: a phase in [-0.5, 0.5) whose wrap flips the
    /// direction of a leaky integrator (a rounded triangle in samples).
    fn control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.phase += rate_per_block(&p[6], ctx.tsr, self.sr);
        if self.phase >= 0.5 {
            self.phase -= 1.0;
            self.dir = -self.dir;
        }
        let depth = ext(&p[7], |v| 6.0 * v);
        let inc = ((depth / 12.0).exp2() - 1.0) * B as f32;
        self.m = 0.99 * self.m + self.dir * inc;
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.control(p, ctx);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        self.control(p, ctx);
        let fbp = ext(&p[2], |v| 2.0 * v - 1.0);
        let neg = fbp < 0.0;
        self.fb.set_target_smoothed(fbp.abs().powi(3));
        let cfp = ext(&p[3], |v| 2.0 * v - 1.0).max(0.0);
        self.cf.set_target_smoothed(cfp * cfp * cfp);
        self.mix.set_target_smoothed(p[10].v);
        self.width.set_target_smoothed(db_to_linear(p[11].v));
        self.pan.set_target_smoothed(p[8].v);

        // Right deactivated = linked: value, sync flag and line mode of Left.
        let src = if p[1].deactivated { &p[0] } else { &p[1] };
        let bases = [sr * time_secs(&p[0], ctx.tsr), sr * time_secs(src, ctx.tsr)];
        // Unknown line modes play as Tape.
        let clean = [p[0].deform == 1, src.deform == 1];
        let offs = [self.m, -self.m];
        let xf_min = (B as f32).max(0.01 * sr);
        let xf_max = 0.5 * sr;

        let mut tap = [[0f32; B]; 2];
        for c in 0..2 {
            let ch = &mut self.ch[c];
            let line = &self.line[c];
            let (base, off) = (bases[c], offs[c]);
            if self.first {
                ch.time = base + off - 6.0;
                ch.lfo = off;
                ch.base = base;
                ch.clean = clean[c];
                ch.xf_len = 0;
            }
            for k in 0..B {
                ch.lfo += GLIDE * (off - ch.lfo);
                if !clean[c] {
                    ch.clean = false;
                    ch.xf_len = 0;
                    ch.time += GLIDE * (base + off - 6.0 - ch.time);
                    tap[c][k] = line.read(ch.time, k);
                } else {
                    if !ch.clean {
                        // Entering Clean: start from the glided tap.
                        ch.base = ch.time - ch.lfo + 6.0;
                        ch.clean = true;
                        ch.xf_len = 0;
                    }
                    if ch.xf_len == 0 && (base - ch.base).abs() >= 0.5 {
                        ch.xf_from = ch.base;
                        ch.base = base;
                        ch.xf_pos = 0;
                        ch.xf_len = (0.5 * base).clamp(xf_min, xf_max) as usize;
                    }
                    let t = ch.base + ch.lfo - 6.0;
                    let mut y = line.read(t, k);
                    if ch.xf_len > 0 {
                        let a = ch.xf_pos as f32 / ch.xf_len as f32;
                        let old = line.read(ch.xf_from + ch.lfo - 6.0, k);
                        y = old * (1.0 - a) + y * a;
                        ch.xf_pos += 1;
                        if ch.xf_pos >= ch.xf_len {
                            ch.xf_len = 0;
                        }
                    }
                    ch.time = t;
                    tap[c][k] = y;
                }
            }
        }
        self.first = false;

        let mode = p[2].deform;
        for ch in tap.iter_mut() {
            for s in ch.iter_mut() {
                let t = if neg { -*s } else { *s };
                *s = clip(mode, t);
            }
        }
        let [mut tl, mut tr] = tap;
        if !p[5].deactivated {
            self.lp.lp2b(calc_omega(p[5].v / 12.0, sr), 0.707);
            self.lp.process(&mut tl, &mut tr);
        }
        if !p[4].deactivated {
            self.hp.hp(calc_omega(p[4].v / 12.0, sr), 0.707);
            self.hp.process(&mut tl, &mut tr);
        }

        let mut il = *l;
        let mut ir = *r;
        tri_pan(&mut il, &mut ir, &self.pan);
        for k in 0..B {
            let fb = self.fb.at(k);
            let cf = self.cf.at(k);
            self.line[0].write_at(k, il[k] + fb * tl[k] + cf * tr[k]);
            self.line[1].write_at(k, ir[k] + fb * tr[k] + cf * tl[k]);
        }
        self.line[0].advance();
        self.line[1].advance();

        apply_width(&mut tl, &mut tr, &self.width);
        self.mix.fade_2_blocks(l, r, &tl, &tr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(fx: &mut Delay, p: &[SlotValue; 12], input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let ctx = FxCtx::new(48000.0);
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for chunk in input.chunks(B) {
            let mut l = [0f32; B];
            l[..chunk.len()].copy_from_slice(chunk);
            let mut r = l;
            fx.process(p, &ctx, &mut l, &mut r);
            ol.extend_from_slice(&l);
            or.extend_from_slice(&r);
        }
        (ol, or)
    }

    fn argmax(x: &[f32]) -> usize {
        let mut best = 0;
        for i in 0..x.len() {
            if x[i].abs() > x[best].abs() {
                best = i;
            }
        }
        best
    }

    #[test]
    fn delay_time_lands_on_the_sample() {
        for line_mode in [0, 1] {
            let mut p = default_values(1);
            p[0].v = 0.1f32.log2();
            p[0].deform = line_mode;
            p[1].deactivated = true;
            p[2].v = 0.0;
            p[4].deactivated = true;
            p[5].deactivated = true;
            p[10].v = 1.0;
            let mut fx = Delay::new(48000.0);
            let mut x = vec![0f32; 8000];
            x[0] = 1.0;
            let (l, r) = render(&mut fx, &p, &x);
            let (il, ir) = (argmax(&l), argmax(&r));
            assert!((il as i32 - 4800).abs() <= 1, "mode {line_mode}: left peak at {il}");
            assert!((ir as i32 - 4800).abs() <= 1, "mode {line_mode}: right peak at {ir}");
            assert!(l[il] > 0.7, "{}", l[il]);
        }
    }

    #[test]
    fn noise_and_sine_finite_and_repeats_die_away() {
        let p = default_values(1);
        let mut fx = Delay::new(48000.0);
        let mut rng = Rng::new(7);
        let n = 48000 * 7;
        let x: Vec<f32> = (0..n)
            .map(|i| if i < 9600 { 0.5 * rng.bipolar() + 0.3 * (i as f32 * 0.05).sin() } else { 0.0 })
            .collect();
        let (l, r) = render(&mut fx, &p, &x);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        let peak = l.iter().fold(0f32, |a, s| a.max(s.abs()));
        let tail = l[n - 24000..].iter().fold(0f32, |a, s| a.max(s.abs()));
        assert!(tail < 1e-4 * peak, "tail {tail} peak {peak}");
    }
}
