// Rust port of the twist engine's wavetable model (engine 5):
// dsp/engine/wavetable_engine.h + .cc of the MIT eurorack macro-oscillator
// (user wavetables are not ported). Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/wavetable_engine.h, .cc)
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


use super::engine::*;
use super::osc_wavetable::{interpolate_wave_hermite, Differentiator};
use super::stm::*;
use super::tables::{tables, INTEGRATED_WAVE_SIZE};

const NUM_BANKS: usize = 4;
const NUM_WAVES_PER_BANK: usize = 64;
const NUM_WAVES: usize = 192;
const TABLE_SIZE_F: f32 = 128.0;

/// 8x8x3 wave terrain (plus the fourth, user bank: without user data a fixed
/// reshuffle of the built-in waves).
pub struct WavetableEngine {
    phase: f32,
    x_pre_lp: f32,
    y_pre_lp: f32,
    z_pre_lp: f32,
    x_lp: f32,
    y_lp: f32,
    z_lp: f32,
    previous_x: f32,
    previous_y: f32,
    previous_z: f32,
    previous_f0: f32,
    /// (bank, x, y) -> offset of the wave in the integrated tables.
    wave_map: [usize; NUM_BANKS * NUM_WAVES_PER_BANK],
    diff_out: Differentiator,
}

impl WavetableEngine {
    pub fn new() -> Self {
        let mut e = WavetableEngine {
            phase: 0.0,
            x_pre_lp: 0.0,
            y_pre_lp: 0.0,
            z_pre_lp: 0.0,
            x_lp: 0.0,
            y_lp: 0.0,
            z_lp: 0.0,
            previous_x: 0.0,
            previous_y: 0.0,
            previous_z: 0.0,
            previous_f0: A0,
            wave_map: [0; NUM_BANKS * NUM_WAVES_PER_BANK],
            diff_out: Differentiator::default(),
        };
        e.diff_out.init();
        e.load_user_data();
        e
    }

    /// `LoadUserData(NULL)`: no user waves exist in a patch.
    fn load_user_data(&mut self) {
        for bank in 0..NUM_BANKS {
            for wave in 0..NUM_WAVES_PER_BANK {
                let i = bank * NUM_WAVES_PER_BANK + wave;
                let mut w = i;
                if bank == NUM_BANKS - 1 {
                    w = w * 101 % NUM_WAVES;
                }
                self.wave_map[i] = w * INTEGRATED_WAVE_SIZE;
            }
        }
    }

    #[inline]
    fn read_wave(&self, waves: &[i16], x: i32, y: i32, z: i32, pi: i32, pf: f32) -> f32 {
        let idx = (x + y * 8 + z * NUM_WAVES_PER_BANK as i32) as usize;
        interpolate_wave_hermite(&waves[self.wave_map[idx]..], pi, pf)
    }
}

#[inline]
fn clamp(x: f32, amount: f32) -> f32 {
    constrain((x - 0.5) * amount, -0.5, 0.5) + 0.5
}

impl Engine for WavetableEngine {
    fn reset(&mut self) {}

    fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool, _rng: &mut Random) {
        let size = out.len();
        let waves = &tables().integrated_waves[..];
        let f0 = note_to_frequency(p.note);

        one_pole(&mut self.x_pre_lp, p.timbre * 6.9999, 0.2);
        one_pole(&mut self.y_pre_lp, p.morph * 6.9999, 0.2);
        one_pole(&mut self.z_pre_lp, p.harmonics * 6.9999, 0.05);

        let x = self.x_pre_lp;
        let y = self.y_pre_lp;
        let z = self.z_pre_lp;

        let quantization = (z - 3.0).max(0.0).min(1.0);
        let lp_coefficient = (2.0 * f0 * (4.0 - 3.0 * quantization)).max(0.01).min(0.1);

        let (xi, mut xf) = integral_fractional(x);
        let (yi, mut yf) = integral_fractional(y);
        let (zi, mut zf) = integral_fractional(z);

        xf += quantization * (clamp(xf, 16.0) - xf);
        yf += quantization * (clamp(yf, 16.0) - yf);
        zf += quantization * (clamp(zf, 16.0) - zf);

        let mut previous_x = self.previous_x;
        let mut previous_y = self.previous_y;
        let mut previous_z = self.previous_z;
        let mut previous_f0 = self.previous_f0;
        {
            let mut x_modulation = ParameterInterpolator::new(&mut previous_x, xi as f32 + xf, size);
            let mut y_modulation = ParameterInterpolator::new(&mut previous_y, yi as f32 + yf, size);
            let mut z_modulation = ParameterInterpolator::new(&mut previous_z, zi as f32 + zf, size);
            let mut f0_modulation = ParameterInterpolator::new(&mut previous_f0, f0, size);

            for i in 0..size {
                let f0 = f0_modulation.next();
                let gain = (1.0 / (f0 * 131072.0)) * (0.95 - f0);
                let cutoff = (TABLE_SIZE_F * f0).min(1.0);

                one_pole(&mut self.x_lp, x_modulation.next(), lp_coefficient);
                one_pole(&mut self.y_lp, y_modulation.next(), lp_coefficient);
                one_pole(&mut self.z_lp, z_modulation.next(), lp_coefficient);

                let (x_integral, x_fractional) = integral_fractional(self.x_lp);
                let (y_integral, y_fractional) = integral_fractional(self.y_lp);
                let (z_integral, z_fractional) = integral_fractional(self.z_lp);

                self.phase += f0;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }

                let (pi, pf) = integral_fractional(self.phase * TABLE_SIZE_F);

                let x0 = x_integral;
                let x1 = x_integral + 1;
                let y0 = y_integral;
                let y1 = y_integral + 1;
                let mut z0 = z_integral;
                let mut z1 = z_integral + 1;
                if z0 >= 4 {
                    z0 = 7 - z0;
                }
                if z1 >= 4 {
                    z1 = 7 - z1;
                }

                let x0y0z0 = self.read_wave(waves, x0, y0, z0, pi, pf);
                let x1y0z0 = self.read_wave(waves, x1, y0, z0, pi, pf);
                let xy0z0 = x0y0z0 + (x1y0z0 - x0y0z0) * x_fractional;

                let x0y1z0 = self.read_wave(waves, x0, y1, z0, pi, pf);
                let x1y1z0 = self.read_wave(waves, x1, y1, z0, pi, pf);
                let xy1z0 = x0y1z0 + (x1y1z0 - x0y1z0) * x_fractional;

                let xyz0 = xy0z0 + (xy1z0 - xy0z0) * y_fractional;

                let x0y0z1 = self.read_wave(waves, x0, y0, z1, pi, pf);
                let x1y0z1 = self.read_wave(waves, x1, y0, z1, pi, pf);
                let xy0z1 = x0y0z1 + (x1y0z1 - x0y0z1) * x_fractional;

                let x0y1z1 = self.read_wave(waves, x0, y1, z1, pi, pf);
                let x1y1z1 = self.read_wave(waves, x1, y1, z1, pi, pf);
                let xy1z1 = x0y1z1 + (x1y1z1 - x0y1z1) * x_fractional;

                let xyz1 = xy0z1 + (xy1z1 - xy0z1) * y_fractional;

                let mut mix = xyz0 + (xyz1 - xyz0) * z_fractional;
                mix = self.diff_out.process(cutoff, mix) * gain;
                out[i] = mix;
                aux[i] = ((mix * 32.0) as i32) as f32 / 32.0;
            }
        }
        self.previous_x = previous_x;
        self.previous_y = previous_y;
        self.previous_z = previous_z;
        self.previous_f0 = previous_f0;
    }
}
