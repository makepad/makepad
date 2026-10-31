// Rust port of the twist engine's particle noise model (engine 10):
// dsp/engine/particle_engine.h + particle_engine.cc and dsp/noise/particle.h of
// the MIT eurorack macro-oscillator. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/engine/particle_engine.h, dsp/engine/particle_engine.cc,
//                                dsp/noise/particle.h)
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
use super::fx_diffuser::Diffuser;
use super::stm::*;

// ---------------------------------------------------------- noise/particle.h

/// Random impulse train processed by a resonant filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Particle {
    pre_gain: f32,
    filter: Svf,
}

impl Particle {
    pub fn init(&mut self) {
        self.pre_gain = 0.0;
        self.filter.init();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sync: bool,
        density: f32,
        gain: f32,
        frequency: f32,
        spread: f32,
        q: f32,
        out: &mut [f32],
        aux: &mut [f32],
        rng: &mut Random,
    ) {
        let mut u = rng.get_float();
        if sync {
            u = density;
        }
        let mut can_randomize_frequency = true;
        for i in 0..out.len() {
            let mut s = 0.0;
            if u <= density {
                s = u * gain;
                if can_randomize_frequency {
                    let u = 2.0 * rng.get_float() - 1.0;
                    let f = (semitones_to_ratio(spread * u) * frequency).min(0.25);
                    self.pre_gain = 0.5 / (q * f * density.sqrt()).sqrt();
                    self.filter.set_f_q::<FREQUENCY_DIRTY>(f, q);
                    // Keep the cutoff constant for this whole block.
                    can_randomize_frequency = false;
                }
            }
            aux[i] += s;
            out[i] += self.filter.process::<FILTER_MODE_BAND_PASS>(self.pre_gain * s);
            u = rng.get_float();
        }
    }
}

// ------------------------------------------------ engine/particle_engine.*

const NUM_PARTICLES: usize = 6;

/// Filtered random pulses.
pub struct ParticleEngine {
    particle: [Particle; NUM_PARTICLES],
    diffuser: Diffuser,
    post_filter: Svf,
}

impl ParticleEngine {
    pub fn new() -> Self {
        let mut e = ParticleEngine {
            particle: [Particle::default(); NUM_PARTICLES],
            diffuser: Diffuser::new(),
            post_filter: Svf::default(),
        };
        for p in e.particle.iter_mut() {
            p.init();
        }
        e.post_filter.init();
        e
    }
}

impl Engine for ParticleEngine {
    fn reset(&mut self) {
        self.diffuser.reset();
    }

    fn render(
        &mut self,
        p: &EngineParameters,
        out: &mut [f32],
        aux: &mut [f32],
        _already_enveloped: &mut bool,
        rng: &mut Random,
    ) {
        let f0 = note_to_frequency(p.note);
        let density_sqrt = note_to_frequency(60.0 + p.timbre * p.timbre * 72.0);
        let density = density_sqrt * density_sqrt * (1.0 / NUM_PARTICLES as f32);
        let gain = 1.0 / density;
        let q_sqrt = semitones_to_ratio(if p.morph >= 0.5 { (p.morph - 0.5) * 120.0 } else { 0.0 });
        let q = 0.5 + q_sqrt * q_sqrt;
        let spread = 48.0 * p.harmonics * p.harmonics;
        let raw_diffusion_sqrt = 2.0 * (p.morph - 0.5).abs();
        let raw_diffusion = raw_diffusion_sqrt * raw_diffusion_sqrt;
        let diffusion = if p.morph < 0.5 { raw_diffusion } else { 0.0 };
        let sync = p.trigger & TRIGGER_RISING_EDGE != 0;

        out.fill(0.0);
        aux.fill(0.0);
        for particle in self.particle.iter_mut() {
            particle.render(sync, density, gain, f0, spread, q, out, aux, rng);
        }
        self.post_filter.set_f_q::<FREQUENCY_DIRTY>(f0.min(0.49), 0.5);
        self.post_filter.process_in_place::<FILTER_MODE_LOW_PASS>(out);
        self.diffuser.process(0.8 * diffusion * diffusion, 0.5 * diffusion + 0.25, out);
    }
}
