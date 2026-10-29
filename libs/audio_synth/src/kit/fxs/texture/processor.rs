// Rust port of the nimbus processor: dsp/granular_processor.h + .cc,
// dsp/parameters.h, dsp/frame.h and dsp/sample_rate_converter.h of the MIT
// eurorack texture synthesizer. Original notices:
//
// Copyright 2014 Emilie Gillet. (dsp/granular_processor.h, dsp/granular_processor.cc,
//                                dsp/parameters.h, dsp/frame.h,
//                                dsp/sample_rate_converter.h)
//
// Author: Emilie Gillet (emilie.o.gillet@gmail.com)
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// See http://creativecommons.org/licenses/MIT/ for more information.
//
// -----------------------------------------------------------------------------
//
// Differences from the original: the two memory blocks are kept as their
// sizes, and every user of them owns storage of the largest size it can get
// from them (allocated in `new`), so a mode or quality change re-initialises
// without allocating. Persistent-state save/load (sample memories) is not
// ported. The random generator is owned here and passed down.

use super::buffer::AudioBuffer;
use super::fx::{Diffuser, PitchShifter, Reverb};
use super::grain::GranularSamplePlayer;
use super::looping::LoopingSamplePlayer;
use super::pvoc::PhaseVocoder;
use super::tables::{interp, tables, SRC_FILTER_1X_2_45};
use super::wsola::{Correlator, WsolaSamplePlayer};
use crate::kit::oscs::twist::stm::{
    one_pole, semitones_to_ratio, soft_convert, soft_limit, ParameterInterpolator, Random, Svf,
    FILTER_MODE_HIGH_PASS, FILTER_MODE_LOW_PASS, FREQUENCY_FAST,
};

pub const MAX_BLOCK_SIZE: usize = 32;
pub const DOWNSAMPLING_FACTOR: usize = 2;

/// The hardware's two memory blocks.
pub const LARGE_BUFFER_SIZE: usize = 118784;
pub const SMALL_BUFFER_SIZE: usize = 65408;

pub const PLAYBACK_MODE_GRANULAR: i32 = 0;
pub const PLAYBACK_MODE_STRETCH: i32 = 1;
pub const PLAYBACK_MODE_LOOPING_DELAY: i32 = 2;
pub const PLAYBACK_MODE_SPECTRAL: i32 = 3;
pub const PLAYBACK_MODE_LAST: i32 = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct Granular {
    pub overlap: f32,
    pub window_shape: f32,
    /// Unused by the engine (it reads `Parameters::stereo_spread`).
    #[allow(dead_code)]
    pub stereo_spread: f32,
    pub use_deterministic_seed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Spectral {
    pub quantization: f32,
    pub refresh_rate: f32,
    pub phase_randomization: f32,
    pub warp: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Parameters {
    pub position: f32,
    pub size: f32,
    pub pitch: f32,
    pub density: f32,
    pub texture: f32,
    pub dry_wet: f32,
    pub stereo_spread: f32,
    pub feedback: f32,
    pub reverb: f32,
    pub freeze: bool,
    pub trigger: bool,
    pub gate: bool,
    pub granular: Granular,
    pub spectral: Spectral,
}

/// FIR 2x converter over interleaved stereo (`RATIO` < 0 decimates).
pub struct SampleRateConverter<const RATIO: i32, const N: usize> {
    coefficients: [f32; N],
    history: Vec<[f32; 2]>,
    history_ptr: i32,
}

impl<const RATIO: i32, const N: usize> SampleRateConverter<RATIO, N> {
    pub fn new(coefficients: &[f32; N]) -> Self {
        SampleRateConverter { coefficients: *coefficients, history: vec![[0.0; 2]; 2 * N], history_ptr: N as i32 - 1 }
    }
    pub fn init(&mut self) {
        self.history.iter_mut().for_each(|h| *h = [0.0; 2]);
        self.history_ptr = N as i32 - 1;
    }
    /// `input_size` input frames from `input`, output frames into `out`.
    pub fn process(&mut self, input: &[f32], out: &mut [f32], mut input_size: usize) {
        let n = N as i32;
        let mut hp = self.history_ptr;
        let scale = if RATIO < 0 { 1.0 } else { RATIO as f32 };
        let mut ii = 0;
        let mut oo = 0;
        while input_size > 0 {
            let consumed = if RATIO < 0 { -RATIO } else { 1 };
            for _ in 0..consumed {
                let f = [input[2 * ii], input[2 * ii + 1]];
                ii += 1;
                self.history[(hp + n) as usize] = f;
                self.history[hp as usize] = f;
                input_size -= 1;
                hp -= 1;
                if hp < 0 {
                    hp += n;
                }
            }
            let produced = if RATIO > 0 { RATIO } else { 1 };
            for i in 0..produced {
                let (mut y_l, mut y_r) = (0.0f32, 0.0f32);
                let mut x = (hp + 1) as usize;
                let mut j = i as usize;
                while j < N {
                    let h = self.coefficients[j];
                    y_l += self.history[x][0] * h;
                    y_r += self.history[x][1] * h;
                    x += 1;
                    j += produced as usize;
                }
                out[2 * oo] = y_l * scale;
                out[2 * oo + 1] = y_r * scale;
                oo += 1;
            }
        }
        self.history_ptr = hp;
    }
}

pub struct GranularProcessor {
    playback_mode: i32,
    previous_playback_mode: i32,
    num_channels: i32,
    low_fidelity: bool,
    silence: bool,
    bypass: bool,
    reset_buffers: bool,
    freeze_lp: f32,
    dry_wet: f32,

    correlator: Correlator,
    player: GranularSamplePlayer,
    ws_player: WsolaSamplePlayer,
    looper: LoopingSamplePlayer,
    phase_vocoder: PhaseVocoder,
    diffuser: Diffuser,
    reverb: Reverb,
    pitch_shifter: PitchShifter,
    fb_filter: [Svf; 2],
    hp_filter: [Svf; 2],
    lp_filter: [Svf; 2],

    buffer_8: [AudioBuffer<true>; 2],
    buffer_16: [AudioBuffer<false>; 2],

    in_: [f32; MAX_BLOCK_SIZE * 2],
    in_downsampled: [f32; MAX_BLOCK_SIZE],
    out_downsampled: [f32; MAX_BLOCK_SIZE],
    out: [f32; MAX_BLOCK_SIZE * 2],
    fb: [f32; MAX_BLOCK_SIZE * 2],

    parameters: Parameters,
    src_down: SampleRateConverter<-2, 45>,
    src_up: SampleRateConverter<2, 45>,
    pub rng: Random,
}

impl GranularProcessor {
    /// `Init` with the hardware's two blocks; allocates every buffer.
    pub fn new(seed: u32) -> Box<GranularProcessor> {
        let _ = tables();
        let mut p = Box::new(GranularProcessor {
            playback_mode: PLAYBACK_MODE_GRANULAR,
            previous_playback_mode: PLAYBACK_MODE_LAST,
            num_channels: 2,
            low_fidelity: false,
            silence: false,
            bypass: false,
            reset_buffers: true,
            freeze_lp: 0.0,
            dry_wet: 0.0,
            correlator: Correlator::default(),
            player: GranularSamplePlayer::default(),
            ws_player: WsolaSamplePlayer::default(),
            looper: LoopingSamplePlayer::default(),
            phase_vocoder: PhaseVocoder::default(),
            diffuser: Diffuser::default(),
            reverb: Reverb::default(),
            pitch_shifter: PitchShifter::default(),
            fb_filter: [Svf::default(); 2],
            hp_filter: [Svf::default(); 2],
            lp_filter: [Svf::default(); 2],
            // Mono gets the whole large block, stereo the small block size each.
            buffer_8: [AudioBuffer::new(LARGE_BUFFER_SIZE), AudioBuffer::new(SMALL_BUFFER_SIZE)],
            buffer_16: [AudioBuffer::new(LARGE_BUFFER_SIZE / 2), AudioBuffer::new(SMALL_BUFFER_SIZE / 2)],
            in_: [0.0; MAX_BLOCK_SIZE * 2],
            in_downsampled: [0.0; MAX_BLOCK_SIZE],
            out_downsampled: [0.0; MAX_BLOCK_SIZE],
            out: [0.0; MAX_BLOCK_SIZE * 2],
            fb: [0.0; MAX_BLOCK_SIZE * 2],
            parameters: Parameters::default(),
            src_down: SampleRateConverter::new(&SRC_FILTER_1X_2_45),
            src_up: SampleRateConverter::new(&SRC_FILTER_1X_2_45),
            rng: Random::new(seed),
        });
        p.src_down.init();
        p.src_up.init();
        p.reset_filters();
        p
    }

    fn reset_filters(&mut self) {
        for i in 0..2 {
            self.fb_filter[i].init();
            self.lp_filter[i].init();
            self.hp_filter[i].init();
        }
    }

    #[inline]
    pub fn parameters_mut(&mut self) -> &mut Parameters {
        &mut self.parameters
    }
    #[inline]
    pub fn set_freeze(&mut self, freeze: bool) {
        self.parameters.freeze = freeze;
    }
    #[inline]
    pub fn set_silence(&mut self, silence: bool) {
        self.silence = silence;
    }
    #[inline]
    pub fn set_playback_mode(&mut self, mode: i32) {
        self.playback_mode = mode;
    }
    pub fn set_quality(&mut self, quality: i32) {
        self.set_num_channels(if quality & 1 != 0 { 1 } else { 2 });
        self.set_low_fidelity(quality >> 1 != 0);
    }
    fn set_num_channels(&mut self, n: i32) {
        self.reset_buffers = self.reset_buffers || self.num_channels != n;
        self.num_channels = n;
    }
    fn set_low_fidelity(&mut self, lf: bool) {
        self.reset_buffers = self.reset_buffers || lf != self.low_fidelity;
        self.low_fidelity = lf;
    }
    #[inline]
    fn resolution(&self) -> i32 {
        if self.low_fidelity {
            8
        } else {
            16
        }
    }
    #[inline]
    fn sample_rate(&self) -> f32 {
        32000.0 / if self.low_fidelity { DOWNSAMPLING_FACTOR as f32 } else { 1.0 }
    }

    fn process_granular(&mut self, use_downsampled: bool, size: usize) {
        let nc = self.num_channels as usize;
        let (input, output): (&[f32], &mut [f32]) = if use_downsampled {
            (&self.in_downsampled[..], &mut self.out_downsampled[..])
        } else {
            (&self.in_[..], &mut self.out[..])
        };
        let p = &mut self.parameters;
        if self.playback_mode != PLAYBACK_MODE_SPECTRAL {
            for i in 0..nc {
                if self.low_fidelity {
                    self.buffer_8[i].write_fade(input, i, size as i32, !p.freeze);
                } else {
                    self.buffer_16[i].write_fade(input, i, size as i32, !p.freeze);
                }
            }
        }
        let lofi = self.low_fidelity;
        match self.playback_mode {
            PLAYBACK_MODE_GRANULAR => {
                p.granular.use_deterministic_seed = p.density < 0.5;
                if p.density >= 0.53 {
                    p.granular.overlap = (p.density - 0.53) * 2.12;
                } else if p.density <= 0.47 {
                    p.granular.overlap = (0.47 - p.density) * 2.12;
                } else {
                    p.granular.overlap = 0.0;
                }
                p.granular.window_shape = if p.texture < 0.75 { p.texture * 1.333 } else { 1.0 };
                if lofi {
                    self.player.play(&self.buffer_8, p, output, size, &mut self.rng);
                } else {
                    self.player.play(&self.buffer_16, p, output, size, &mut self.rng);
                }
            }
            PLAYBACK_MODE_STRETCH => {
                if lofi {
                    self.ws_player.play(&self.buffer_8, &mut self.correlator, p, output, size);
                } else {
                    self.ws_player.play(&self.buffer_16, &mut self.correlator, p, output, size);
                }
            }
            PLAYBACK_MODE_LOOPING_DELAY => {
                if lofi {
                    self.looper.play(&self.buffer_8, p, output, size);
                } else {
                    self.looper.play(&self.buffer_16, p, output, size);
                }
            }
            PLAYBACK_MODE_SPECTRAL => {
                p.spectral.quantization = p.texture;
                p.spectral.refresh_rate = 0.01 + 0.99 * p.density;
                let warp = p.size - 0.5;
                p.spectral.warp = 4.0 * warp * warp * warp + 0.5;
                let mut randomization = p.density - 0.5;
                randomization *= randomization * 4.2;
                randomization -= 0.05;
                p.spectral.phase_randomization = randomization.clamp(0.0, 1.0);
                self.phase_vocoder.process(input, output, size);
                if nc == 1 {
                    for i in 0..size {
                        output[2 * i + 1] = output[2 * i];
                    }
                }
            }
            _ => {}
        }
    }

    /// Processes `size` (<= 32) 16-bit stereo frames.
    pub fn process(&mut self, input: &[[i16; 2]], output: &mut [[i16; 2]], size: usize) {
        if self.bypass {
            output[..size].copy_from_slice(&input[..size]);
            return;
        }
        if self.silence || self.reset_buffers || self.previous_playback_mode != self.playback_mode {
            output[..size].iter_mut().for_each(|f| *f = [0, 0]);
            return;
        }
        for i in 0..size {
            self.in_[2 * i] = input[i][0] as f32 / 32768.0;
            self.in_[2 * i + 1] = input[i][1] as f32 / 32768.0;
        }
        if self.num_channels == 1 {
            for i in 0..size {
                let m = (self.in_[2 * i] + self.in_[2 * i + 1]) * 0.5;
                self.in_[2 * i] = m;
                self.in_[2 * i + 1] = m;
            }
        }

        one_pole(&mut self.freeze_lp, if self.parameters.freeze { 1.0 } else { 0.0 }, 0.0005);
        let feedback = self.parameters.feedback;
        let cutoff = (20.0 + 100.0 * feedback * feedback) / self.sample_rate();
        self.fb_filter[0].set_f_q::<FREQUENCY_FAST>(cutoff, 1.0);
        let f0 = self.fb_filter[0];
        self.fb_filter[1].set(&f0);
        for i in 0..size {
            self.fb[2 * i] = self.fb_filter[0].process::<FILTER_MODE_HIGH_PASS>(self.fb[2 * i]);
            self.fb[2 * i + 1] = self.fb_filter[1].process::<FILTER_MODE_HIGH_PASS>(self.fb[2 * i + 1]);
        }
        let fb_gain = feedback * (1.0 - self.freeze_lp);
        for i in 0..2 * size {
            let x = self.in_[i];
            self.in_[i] += fb_gain * (soft_limit(fb_gain * 1.4 * self.fb[i] + x) - x);
        }

        if self.low_fidelity {
            let downsampled_size = size / DOWNSAMPLING_FACTOR;
            self.src_down.process(&self.in_, &mut self.in_downsampled, size);
            self.process_granular(true, downsampled_size);
            self.src_up.process(&self.out_downsampled, &mut self.out, downsampled_size);
        } else {
            self.process_granular(false, size);
        }

        let mode = self.playback_mode;
        if mode != PLAYBACK_MODE_SPECTRAL {
            let texture = self.parameters.texture;
            let diffusion = if mode == PLAYBACK_MODE_GRANULAR {
                if texture > 0.75 {
                    (texture - 0.75) * 4.0
                } else {
                    0.0
                }
            } else {
                self.parameters.density
            };
            self.diffuser.set_amount(diffusion);
            self.diffuser.process(&mut self.out, size);
        }

        if mode == PLAYBACK_MODE_LOOPING_DELAY && (!self.parameters.freeze || self.looper.synchronized()) {
            self.pitch_shifter.set_ratio(semitones_to_ratio(self.parameters.pitch));
            self.pitch_shifter.set_size(self.parameters.size);
            self.pitch_shifter.process(&mut self.out, size);
        }

        if mode == PLAYBACK_MODE_LOOPING_DELAY || mode == PLAYBACK_MODE_STRETCH {
            let cutoff = self.parameters.texture;
            let lp_cutoff =
                (0.5 * semitones_to_ratio((if cutoff < 0.5 { cutoff - 0.5 } else { 0.0 }) * 216.0)).clamp(0.0, 0.499);
            let hp_cutoff =
                (0.25 * semitones_to_ratio((if cutoff < 0.5 { -0.5 } else { cutoff - 1.0 }) * 216.0)).clamp(0.0, 0.499);
            let lpq = 1.0 + 3.0 * (1.0 - feedback) * (0.5 - lp_cutoff);
            self.lp_filter[0].set_f_q::<FREQUENCY_FAST>(lp_cutoff, lpq);
            for i in 0..size {
                self.out[2 * i] = self.lp_filter[0].process::<FILTER_MODE_LOW_PASS>(self.out[2 * i]);
            }
            let f = self.lp_filter[0];
            self.lp_filter[1].set(&f);
            for i in 0..size {
                self.out[2 * i + 1] = self.lp_filter[1].process::<FILTER_MODE_LOW_PASS>(self.out[2 * i + 1]);
            }
            self.hp_filter[0].set_f_q::<FREQUENCY_FAST>(hp_cutoff, 1.0);
            for i in 0..size {
                self.out[2 * i] = self.hp_filter[0].process::<FILTER_MODE_HIGH_PASS>(self.out[2 * i]);
            }
            let f = self.hp_filter[0];
            self.hp_filter[1].set(&f);
            for i in 0..size {
                self.out[2 * i + 1] = self.hp_filter[1].process::<FILTER_MODE_HIGH_PASS>(self.out[2 * i + 1]);
            }
        }

        // Fed back (without the reverb).
        self.fb[..2 * size].copy_from_slice(&self.out[..2 * size]);

        let mut reverb_amount = self.parameters.reverb * 0.95;
        reverb_amount += feedback * (2.0 - feedback) * self.freeze_lp;
        let reverb_amount = reverb_amount.clamp(0.0, 1.0);
        self.reverb.set_amount(reverb_amount * 0.54);
        self.reverb.set_diffusion(0.7);
        self.reverb.set_time(0.35 + 0.63 * reverb_amount);
        self.reverb.set_input_gain(0.2);
        self.reverb.set_lp(0.6 + 0.37 * feedback);
        self.reverb.process(&mut self.out, size);

        let post_gain = 1.2;
        let t = tables();
        let target = self.parameters.dry_wet;
        let mut dry_wet_mod = ParameterInterpolator::new(&mut self.dry_wet, target, size);
        for i in 0..size {
            let dry_wet = dry_wet_mod.next();
            let fade_in = interp(&t.xfade_in, dry_wet, 16.0);
            let fade_out = interp(&t.xfade_out, dry_wet, 16.0);
            let mut l = input[i][0] as f32 / 32768.0 * fade_out;
            let mut r = input[i][1] as f32 / 32768.0 * fade_out;
            l += self.out[2 * i] * post_gain * fade_in;
            r += self.out[2 * i + 1] * post_gain * fade_in;
            output[i] = [soft_convert(l), soft_convert(r)];
        }
    }

    pub fn prepare(&mut self) {
        let playback_mode_changed = self.previous_playback_mode != self.playback_mode;
        let benign_change = self.previous_playback_mode != PLAYBACK_MODE_SPECTRAL
            && self.playback_mode != PLAYBACK_MODE_SPECTRAL
            && self.previous_playback_mode != PLAYBACK_MODE_LAST;

        if !self.reset_buffers && playback_mode_changed && benign_change {
            self.reset_filters();
            self.pitch_shifter.clear();
            self.previous_playback_mode = self.playback_mode;
        }
        if (playback_mode_changed && !benign_change) || self.reset_buffers {
            self.parameters.freeze = false;
        }
        if self.reset_buffers || (playback_mode_changed && !benign_change) {
            // Sample memory per channel, in bytes: mono takes the large block
            // (the small one is FX workspace), stereo the small block size in
            // each (the rest of the large block is FX workspace).
            let buffer_size: [usize; 2] = if self.num_channels == 1 {
                [LARGE_BUFFER_SIZE, 0]
            } else {
                [SMALL_BUFFER_SIZE, SMALL_BUFFER_SIZE]
            };
            self.diffuser.init();
            self.reverb.init();
            self.correlator.init();
            self.pitch_shifter.init();
            let nc = self.num_channels as usize;
            if self.playback_mode == PLAYBACK_MODE_SPECTRAL {
                self.phase_vocoder.init(buffer_size, nc);
            } else {
                for i in 0..nc {
                    if self.resolution() == 8 {
                        self.buffer_8[i].init(buffer_size[i] as i32);
                    } else {
                        self.buffer_16[i].init((buffer_size[i] >> 1) as i32);
                    }
                }
                let num_grains = ((if nc == 1 { 40 } else { 32 }) * (if self.low_fidelity { 23 } else { 16 })) >> 4;
                self.player.init(nc as i32, num_grains);
                self.ws_player.init(nc as i32);
                self.looper.init(nc as i32);
            }
            self.reset_buffers = false;
            self.previous_playback_mode = self.playback_mode;
        }

        if self.playback_mode == PLAYBACK_MODE_SPECTRAL {
            let p = self.parameters;
            self.phase_vocoder.buffer(&p, &mut self.rng);
        } else if self.playback_mode == PLAYBACK_MODE_STRETCH {
            if self.resolution() == 8 {
                self.ws_player.load_correlator(&self.buffer_8, &mut self.correlator);
            } else {
                self.ws_player.load_correlator(&self.buffer_16, &mut self.correlator);
            }
            self.correlator.evaluate_some_candidates();
        }
    }
}
