//! The kit's effects rack SLOT: an ordered chain (`fx: [@delay @reverb]`)
//! over the summed voices, each effect once. The effect family is the
//! shared one the clean-room effects spec defines and is implemented once
//! when it lands, for every layout; until then the rack holds the effects
//! this crate's own DSP already has: a tempo delay, the room reverb, the
//! glue compressor and a utility (gain, pan, width, mono lows).

use super::schema::{Builder, Unit::{self, Beats, Db, Hz, Ms, Percent, Secs}, Values};
use crate::delay::DelayLine;
use crate::dsp::{db_to_gain, OnePole};
use crate::dynamics::Compressor;
use crate::reverb::{Reverb, RoomParams};

/// The effects a rack takes now (the shared family extends this list).
pub const KINDS: &[&str] = &["delay", "reverb", "comp", "utility"];
pub const DELAY_MODES: &[&str] = &["normal", "pingpong", "tap"];

#[derive(Clone, Debug, Default)]
pub struct FxIx {
    pub delay_mode: usize,
    pub delay_time: usize,
    pub delay_offset: usize,
    pub delay_feedback: usize,
    pub delay_hp: usize,
    pub delay_lp: usize,
    pub delay_mix: usize,
    pub reverb_size: usize,
    pub reverb_decay: usize,
    pub reverb_damp: usize,
    pub reverb_predelay: usize,
    pub reverb_mix: usize,
    pub comp_threshold: usize,
    pub comp_ratio: usize,
    pub comp_attack: usize,
    pub comp_release: usize,
    pub comp_gain: usize,
    pub comp_mix: usize,
    pub utility_gain: usize,
    pub utility_pan: usize,
    pub utility_width: usize,
    pub utility_bass_mono: usize,
    pub utility_invert: usize,
}

pub fn declare(b: &mut Builder) -> FxIx {
    let mut f = FxIx::default();
    b.group("fx_delay", "delay on the tempo (pingpong bounces; tap: left at time, right at time x offset after it)");
    f.delay_mode = b.choice("delay_mode".into(), "mode", DELAY_MODES, 0, "");
    f.delay_time = b.log("delay_time".into(), "time", 0.0625, 4.0, 0.75, Beats, "");
    f.delay_offset = b.num("delay_offset".into(), "offset", 0.5, 2.0, 1.0, Unit::None, "right channel's time ratio");
    f.delay_feedback = b.num("delay_feedback".into(), "feedback", 0.0, 0.95, 0.4, Percent, "");
    f.delay_hp = b.log("delay_hp".into(), "hp", 20.0, 2000.0, 100.0, Hz, "");
    f.delay_lp = b.log("delay_lp".into(), "lp", 500.0, 20000.0, 8000.0, Hz, "");
    f.delay_mix = b.num("delay_mix".into(), "mix", 0.0, 1.0, 0.25, Percent, "");
    b.group("fx_reverb", "reverb");
    f.reverb_size = b.num("reverb_size".into(), "size", 0.0, 1.0, 0.6, Percent, "");
    f.reverb_decay = b.log("reverb_decay".into(), "decay", 0.1, 20.0, 2.5, Secs, "");
    f.reverb_damp = b.log("reverb_damp".into(), "damp", 500.0, 18000.0, 6000.0, Hz, "");
    f.reverb_predelay = b.num("reverb_predelay".into(), "predelay", 0.0, 80.0, 10.0, Ms, "");
    f.reverb_mix = b.num("reverb_mix".into(), "mix", 0.0, 1.0, 0.25, Percent, "");
    b.group("fx_comp", "compressor");
    f.comp_threshold = b.num("comp_threshold".into(), "thresh", -60.0, 0.0, -18.0, Db, "");
    f.comp_ratio = b.log("comp_ratio".into(), "ratio", 1.0, 20.0, 4.0, Unit::None, "");
    f.comp_attack = b.log("comp_attack".into(), "attack", 0.1, 200.0, 5.0, Ms, "");
    f.comp_release = b.log("comp_release".into(), "release", 5.0, 2000.0, 120.0, Ms, "");
    f.comp_gain = b.num("comp_gain".into(), "gain", -12.0, 24.0, 0.0, Db, "");
    f.comp_mix = b.num("comp_mix".into(), "mix", 0.0, 1.0, 1.0, Percent, "");
    b.group("fx_utility", "gain, pan, stereo width, mono lows, polarity");
    f.utility_gain = b.num("utility_gain".into(), "gain", -24.0, 24.0, 0.0, Db, "");
    f.utility_pan = b.num("utility_pan".into(), "pan", -1.0, 1.0, 0.0, Unit::None, "");
    f.utility_width = b.num("utility_width".into(), "width", 0.0, 2.0, 1.0, Unit::None, "0 mono, 1 as is, 2 wide");
    f.utility_bass_mono = b.log("utility_bass_mono".into(), "bass mono", 20.0, 500.0, 20.0, Hz, "below this the sides go (20: off)");
    f.utility_invert = b.flag("utility_invert".into(), "invert", false, "flip the polarity");
    f
}

/// The rack's state: one of each effect, made once (no allocation after).
pub struct Rack {
    rate: f32,
    delay: [DelayLine; 2],
    delay_hp: [OnePole; 2],
    delay_lp: [OnePole; 2],
    reverb: Box<Reverb>,
    send: Box<[f32]>,
    wet_l: Box<[f32]>,
    wet_r: Box<[f32]>,
    comp: Compressor,
    comp_key: (f32, f32, f32, f32),
    mono: [OnePole; 2],
}

pub const MAX_BLOCK: usize = 256;

impl Rack {
    pub fn new(rate: f32) -> Rack {
        let max = (rate * 8.0) as usize;
        Rack {
            rate,
            delay: [DelayLine::new(max), DelayLine::new(max)],
            delay_hp: [OnePole::new(100.0, rate); 2],
            delay_lp: [OnePole::new(8000.0, rate); 2],
            reverb: Box::new(Reverb::new(rate)),
            send: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            wet_l: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            wet_r: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            comp: Compressor::new(rate, -18.0, 4.0, 0.005, 0.12),
            comp_key: (f32::NAN, 0.0, 0.0, 0.0),
            mono: [OnePole::new(20.0, rate); 2],
        }
    }

    pub fn clear(&mut self) {
        self.delay.iter_mut().for_each(DelayLine::clear);
        self.reverb.clear();
    }

    /// Run `chain` (indices into [`KINDS`]) over a block in place.
    pub fn process(&mut self, chain: &[u8], v: &(impl Values + ?Sized), x: &FxIx, beats_per_sec: f32, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len()).min(MAX_BLOCK);
        for kind in chain {
            match kind {
                0 => self.delay(v, x, beats_per_sec, &mut l[..n], &mut r[..n]),
                1 => self.reverb(v, x, &mut l[..n], &mut r[..n]),
                2 => self.comp(v, x, &mut l[..n], &mut r[..n]),
                3 => self.utility(v, x, &mut l[..n], &mut r[..n]),
                _ => {}
            }
        }
    }

    fn delay(&mut self, v: &(impl Values + ?Sized), x: &FxIx, bps: f32, l: &mut [f32], r: &mut [f32]) {
        let mode = v.index(x.delay_mode);
        let secs = v.val(x.delay_time) / bps.max(0.1);
        let cap = (self.rate * 7.9) as f32;
        let dl = (secs * self.rate).clamp(2.0, cap);
        let dr = (secs * v.val(x.delay_offset) * self.rate).clamp(2.0, cap);
        let fb = v.val(x.delay_feedback).clamp(0.0, 0.95);
        let mix = v.val(x.delay_mix);
        for ch in 0..2 {
            self.delay_hp[ch].set(v.val(x.delay_hp), self.rate);
            self.delay_lp[ch].set(v.val(x.delay_lp), self.rate);
        }
        for i in 0..l.len() {
            let (xl, xr) = (l[i], r[i]);
            let yl = self.delay[0].read(dl);
            let yr = self.delay[1].read(dr);
            let fl = self.delay_lp[0].lp(self.delay_hp[0].hp(yl));
            let fr = self.delay_lp[1].lp(self.delay_hp[1].hp(yr));
            match mode {
                1 => {
                    // Ping-pong: the mono input enters left, each echo crosses.
                    self.delay[0].push((xl + xr) * 0.5 + fr * fb);
                    self.delay[1].push(fl * fb);
                }
                2 => {
                    // Tap: left once, then right repeats it.
                    self.delay[0].push(xl);
                    self.delay[1].push(yl + fr * fb);
                }
                _ => {
                    self.delay[0].push(xl + fl * fb);
                    self.delay[1].push(xr + fr * fb);
                }
            }
            // Keep the dry level: a delay adds echoes, it does not duck.
            l[i] = xl * (1.0 - mix * 0.5) + yl * mix;
            r[i] = xr * (1.0 - mix * 0.5) + yr * mix;
        }
    }

    fn reverb(&mut self, v: &(impl Values + ?Sized), x: &FxIx, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let size = v.val(x.reverb_size);
        self.reverb.set_room(RoomParams {
            decay_s: v.val(x.reverb_decay),
            damp_hz: v.val(x.reverb_damp),
            predelay_ms: v.val(x.reverb_predelay),
            er_level: 0.2 + 0.4 * (1.0 - size),
            er_spacing_ms: 2.0 + 20.0 * size,
            wet: 1.0,
        });
        for i in 0..n {
            self.send[i] = (l[i] + r[i]) * 0.5;
            self.wet_l[i] = 0.0;
            self.wet_r[i] = 0.0;
        }
        self.reverb.process(&self.send[..n], &mut self.wet_l[..n], &mut self.wet_r[..n]);
        let mix = v.val(x.reverb_mix);
        for i in 0..n {
            l[i] = l[i] * (1.0 - mix * 0.5) + self.wet_l[i] * mix;
            r[i] = r[i] * (1.0 - mix * 0.5) + self.wet_r[i] * mix;
        }
    }

    fn comp(&mut self, v: &(impl Values + ?Sized), x: &FxIx, l: &mut [f32], r: &mut [f32]) {
        let key = (v.val(x.comp_threshold), v.val(x.comp_ratio), v.val(x.comp_attack), v.val(x.comp_release));
        if key != self.comp_key {
            self.comp = Compressor::new(self.rate, key.0, key.1, key.2 * 0.001, key.3 * 0.001);
            self.comp_key = key;
        }
        self.comp.makeup_db = v.val(x.comp_gain);
        let mix = v.val(x.comp_mix);
        for i in 0..l.len() {
            let (a, b) = self.comp.process(l[i], r[i]);
            l[i] += (a - l[i]) * mix;
            r[i] += (b - r[i]) * mix;
        }
    }

    fn utility(&mut self, v: &(impl Values + ?Sized), x: &FxIx, l: &mut [f32], r: &mut [f32]) {
        let g = db_to_gain(v.val(x.utility_gain)) * if v.on(x.utility_invert) { -1.0 } else { 1.0 };
        let pan = v.val(x.utility_pan);
        let (pl, pr) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
        let width = v.val(x.utility_width);
        let mono_hz = v.val(x.utility_bass_mono);
        for ch in 0..2 {
            self.mono[ch].set(mono_hz, self.rate);
        }
        for i in 0..l.len() {
            let mid = (l[i] + r[i]) * 0.5;
            let mut side = (l[i] - r[i]) * 0.5 * width;
            if mono_hz > 21.0 {
                // Only the side's highs stay.
                side = self.mono[0].hp(side);
            }
            l[i] = (mid + side) * g * pl;
            r[i] = (mid - side) * g * pr;
        }
    }
}
