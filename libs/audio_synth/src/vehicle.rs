//! Everything a moving vehicle sounds like besides its engine: tyre squeal
//! and scrub from slip, the road surface under the wheels, wind, kerbs and
//! suspension thumps. [`VehicleSound`] combines an [`Engine`] with these so
//! one voice is one car.

use crate::dsp::*;
use crate::engine::{Engine, EngineInput, EngineSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Surface {
    #[default]
    Asphalt,
    Concrete,
    Gravel,
    Dirt,
    Grass,
    Sand,
    Snow,
    Kerb,
    Wood,
    Metal,
    Water,
}

impl Surface {
    pub fn parse(s: &str) -> Option<Surface> {
        Some(match s {
            "asphalt" | "road" | "tarmac" => Surface::Asphalt,
            "concrete" | "stone" => Surface::Concrete,
            "gravel" => Surface::Gravel,
            "dirt" | "mud" => Surface::Dirt,
            "grass" => Surface::Grass,
            "sand" => Surface::Sand,
            "snow" | "ice" => Surface::Snow,
            "kerb" | "curb" | "rumble" => Surface::Kerb,
            "wood" => Surface::Wood,
            "metal" => Surface::Metal,
            "water" => Surface::Water,
            _ => return None,
        })
    }

    /// (grip-squeal ability, roughness rumble, crunch density, softness)
    fn traits(self) -> (f32, f32, f32, f32) {
        match self {
            Surface::Asphalt => (1.0, 0.5, 0.0, 0.0),
            Surface::Concrete => (0.9, 0.7, 0.0, 0.0),
            Surface::Gravel => (0.1, 1.0, 1.0, 0.2),
            Surface::Dirt => (0.15, 0.8, 0.35, 0.6),
            Surface::Grass => (0.05, 0.6, 0.1, 1.0),
            Surface::Sand => (0.0, 0.5, 0.25, 1.0),
            Surface::Snow => (0.05, 0.3, 0.6, 0.9),
            Surface::Kerb => (0.8, 1.2, 0.0, 0.0),
            Surface::Wood => (0.6, 0.9, 0.0, 0.0),
            Surface::Metal => (0.7, 0.8, 0.0, 0.0),
            Surface::Water => (0.0, 0.3, 0.0, 1.0),
        }
    }
}

/// Per-frame tyre/body input.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TyreInput {
    /// Worst wheel slip (0 grip .. 1 sliding); ~0.1 starts to sing.
    pub slip: f32,
    /// Normalised tyre load (1 = static weight).
    pub load: f32,
    /// Speed over ground, m/s.
    pub speed: f32,
    pub surface: Surface,
    /// Wheels on the ground (0..=4); airborne silences the tyres.
    pub grounded: u8,
}

pub struct TyreRoad {
    input: TyreInput,
    slip: Smooth,
    speed: Smooth,
    squeal: [Resonator; 3],
    squeal_osc: Osc,
    wobble: f32,
    wobble_target: f32,
    rng: Rng,
    noise: Noise,
    pink: Noise,
    brown: Noise,
    rumble_lp: OnePole,
    hiss_bp: Filter,
    crunch_bp: Filter,
    soft_lp: OnePole,
    wind_bp: Filter,
    wind_gust: f32,
    gust_target: f32,
    kerb_phase: f32,
    kerb_res: Resonator,
    thump_env: f32,
    thump_osc: Osc,
    ctl: u32,
    /// Glide coefficients, recomputed at control rate.
    slip_c: f32,
    speed_c: f32,
    pub interior: f32,
}

impl TyreRoad {
    pub fn new(rate: f32) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        TyreRoad {
            input: TyreInput::default(),
            slip: Smooth::new(0.0),
            speed: Smooth::new(0.0),
            squeal: [
                Resonator::new(820.0, 28.0, 1.0, rate),
                Resonator::new(1240.0, 24.0, 0.6, rate),
                Resonator::new(1790.0, 20.0, 0.35, rate),
            ],
            squeal_osc: Osc::new(Wave::Triangle, 0.0),
            wobble: 0.0,
            wobble_target: 0.0,
            rng: Rng::new(0x5eed_1234),
            noise: Noise::new(NoiseColor::White, 0x77),
            pink: Noise::new(NoiseColor::Pink, 0x99),
            brown: Noise::new(NoiseColor::Brown, 0xaa),
            rumble_lp: OnePole::new(120.0, rate),
            hiss_bp: Filter::new(FilterMode::Bandpass),
            crunch_bp: Filter::new(FilterMode::Bandpass),
            soft_lp: OnePole::new(900.0, rate),
            wind_bp: Filter::new(FilterMode::Bandpass),
            wind_gust: 0.0,
            gust_target: 0.0,
            kerb_phase: 0.0,
            kerb_res: Resonator::new(55.0, 2.5, 1.0, rate),
            thump_env: 0.0,
            thump_osc: Osc::new(Wave::Sine, 0.0),
            ctl: 0,
            slip_c: 0.01,
            speed_c: 0.005,
            interior: 0.0,
        }
    }

    pub fn set_input(&mut self, input: TyreInput) {
        self.input = TyreInput {
            slip: finite(input.slip, 0.0).clamp(0.0, 2.0),
            load: finite(input.load, 1.0).clamp(0.0, 4.0),
            speed: finite(input.speed, 0.0).abs().min(200.0),
            ..input
        };
        self.slip.target = if input.grounded > 0 { self.input.slip } else { 0.0 };
        self.speed.target = self.input.speed;
    }

    /// A suspension bottoming / landing thump, strength 0..1.
    pub fn thump(&mut self, strength: f32) {
        self.thump_env = self.thump_env.max(finite(strength, 0.0).clamp(0.0, 1.5));
        self.thump_osc.phase = 0.0;
    }

    #[inline]
    pub fn next(&mut self, dt: f32, rate_eff: f32) -> f32 {
        if self.ctl == 0 {
            self.slip_c = Smooth::coef(0.04, rate_eff);
            self.speed_c = Smooth::coef(0.1, rate_eff);
        }
        let slip = self.slip.next(self.slip_c);
        let speed = self.speed.next(self.speed_c);
        let (grip, rough, crunch_density, soft) = self.input.surface.traits();
        let grounded = self.input.grounded > 0;
        if self.ctl == 0 {
            let load = self.input.load;
            // Squeal pitch rises a little with load and falls with slip.
            let base = 780.0 * (1.0 + 0.15 * (load - 1.0)) * (1.0 - 0.12 * slip.min(1.0));
            for (k, r) in self.squeal.iter_mut().enumerate() {
                let mult = [1.0, 1.52, 2.21][k];
                r.tune(base * mult * (1.0 + self.wobble * 0.03), [28.0, 24.0, 20.0][k], rate_eff);
            }
            self.rumble_lp.set(60.0 + speed * 5.0, rate_eff);
            self.hiss_bp.set(1400.0 + speed * 25.0, 0.9, rate_eff);
            self.crunch_bp.set(2800.0 + speed * 20.0, 0.8, rate_eff);
            self.soft_lp.set(500.0 + speed * 15.0, rate_eff);
            self.wind_bp.set(250.0 + speed * 9.0, 0.6, rate_eff);
            self.kerb_res.tune(48.0 + speed * 0.6, 2.5, rate_eff);
            if self.rng.unit() < 0.05 {
                self.wobble_target = self.rng.bipolar();
            }
            if self.rng.unit() < 0.01 {
                self.gust_target = self.rng.unit();
            }
        }
        self.ctl = (self.ctl + 1) % 32;
        self.wobble += (self.wobble_target - self.wobble) * dt * 12.0;
        self.wind_gust += (self.gust_target - self.wind_gust) * dt * 0.8;

        let speed_n = (speed / 30.0).min(2.0);
        let mut out = 0.0;
        if grounded {
            // ── squeal: slip past the grip threshold, on grippy surfaces ──
            let sing = ((slip - 0.08) / 0.25).clamp(0.0, 1.0) * grip;
            if sing > 0.0 && speed > 1.0 {
                let scrub = ((slip - 0.45) / 0.6).clamp(0.0, 1.0);
                let n = self.noise.next();
                let mut sq = 0.0;
                for r in self.squeal.iter_mut() {
                    sq += r.process(n);
                }
                let tone_hz = 780.0 * (1.0 + 0.15 * (self.input.load - 1.0)) * (1.0 + self.wobble * 0.04);
                let tone = self.squeal_osc.next(tone_hz, dt, 0.0) * 0.15;
                let amp = sing * sing * (speed / 12.0).clamp(0.3, 1.2);
                out += (sq * 1.6 * (1.0 - 0.5 * scrub) + tone * (1.0 - scrub) + self.pink.next() * scrub * 0.25) * amp * 0.35;
            }
            // ── road: rumble + tyre hiss, surface-coloured ──
            let rumble = self.rumble_lp.lp(self.brown.next()) * rough * speed_n.sqrt() * 0.35;
            let hiss = self.hiss_bp.process(self.pink.next()) * (1.0 - soft) * speed_n * 0.05;
            out += rumble + hiss;
            if crunch_density > 0.0 && speed > 0.5 {
                let p = crunch_density * (0.002 + 0.02 * speed_n + 0.05 * slip.min(1.0));
                let grain = if self.rng.unit() < p { self.rng.bipolar() * 3.0 } else { 0.0 };
                out += self.crunch_bp.process(grain + self.noise.next() * 0.05 * crunch_density) * (0.2 + 0.8 * speed_n.min(1.0)) * 0.25;
            }
            if soft > 0.0 {
                out += self.soft_lp.lp(self.pink.next()) * soft * speed_n.min(1.2) * 0.12;
            }
            if self.input.surface == Surface::Kerb && speed > 1.0 {
                // Rumble strips: ~0.4 m apart.
                self.kerb_phase += speed / 0.4 * dt;
                if self.kerb_phase >= 1.0 {
                    self.kerb_phase -= 1.0;
                }
                out += self.kerb_res.process(hann_pulse(self.kerb_phase / 0.35)) * 0.6;
            }
        }
        // ── wind: grows with the square of speed, gusting ──
        let wind_amp = (speed / 45.0).powi(2).min(1.5) * (0.7 + 0.3 * self.wind_gust);
        if wind_amp > 1.0e-4 {
            out += self.wind_bp.process(self.pink.next()) * wind_amp * 0.12 * (1.0 - 0.6 * self.interior);
        }
        // ── thump ──
        if self.thump_env > 0.001 {
            out += (self.thump_osc.next(62.0, dt, 0.0) + self.brown.next() * 0.3) * self.thump_env * 0.5;
            self.thump_env *= 1.0 - dt * 22.0;
        }
        out
    }
}

/// A recorded engine loop taken at `rpm`, for sample-layered engines: the
/// loop is re-pitched by rpm / its rpm and crossfaded with its neighbours.
#[derive(Clone)]
pub struct EngineLoop {
    /// Mono PCM at `rate`.
    pub pcm: std::sync::Arc<[f32]>,
    pub rate: f32,
    pub rpm: f32,
    pos: f64,
}

impl EngineLoop {
    pub fn new(pcm: std::sync::Arc<[f32]>, rate: f32, rpm: f32) -> Self {
        EngineLoop { pcm, rate: finite(rate, 48_000.0).max(1.0), rpm: finite(rpm, 3000.0).max(1.0), pos: 0.0 }
    }
}

pub const MAX_LOOPS: usize = 6;

/// One vehicle: engine + tyres + road + wind, mono out.
pub struct VehicleSound {
    pub engine: Engine,
    pub tyres: TyreRoad,
    pub engine_gain: f32,
    pub tyre_gain: f32,
    /// Recorded loops sorted by rpm (empty = pure synthesis).
    loops: [Option<EngineLoop>; MAX_LOOPS],
    /// 0 = synthesis only .. 1 = recordings only.
    pub sample_mix: f32,
    scratch: [f32; 256],
}

impl VehicleSound {
    pub fn new(rate: f32, spec: EngineSpec) -> Self {
        VehicleSound {
            engine: Engine::new(rate, spec),
            tyres: TyreRoad::new(rate),
            engine_gain: 1.0,
            tyre_gain: 1.0,
            loops: Default::default(),
            sample_mix: 0.0,
            scratch: [0.0; 256],
        }
    }

    /// Install recorded loops (control thread). The previous set is handed
    /// back so its buffers are freed there, never in the audio callback.
    pub fn set_loops(&mut self, mut loops: Vec<EngineLoop>, mix: f32) -> Vec<EngineLoop> {
        loops.sort_by(|a, b| a.rpm.partial_cmp(&b.rpm).unwrap_or(std::cmp::Ordering::Equal));
        loops.truncate(MAX_LOOPS);
        let mut old = Vec::new();
        for slot in self.loops.iter_mut() {
            if let Some(l) = slot.take() {
                old.push(l);
            }
        }
        for (slot, l) in self.loops.iter_mut().zip(loops) {
            *slot = Some(l);
        }
        self.sample_mix = if self.loops[0].is_some() { finite(mix, 0.7).clamp(0.0, 1.0) } else { 0.0 };
        old
    }

    /// Crossfaded recorded loops at the current rpm, ADDING into `out`.
    fn render_loops(
        loops: &mut [Option<EngineLoop>; MAX_LOOPS],
        input: EngineInput,
        sample_mix: f32,
        out: &mut [f32],
        doppler: f32,
        device_rate: f32,
    ) {
        let rpm = input.rpm.max(1.0);
        let throttle = input.throttle;
        let count = loops.iter().take_while(|l| l.is_some()).count();
        if count == 0 {
            return;
        }
        // Neighbours in log-rpm.
        let mut lo = 0;
        while lo + 1 < count && loops[lo + 1].as_ref().map(|l| l.rpm <= rpm).unwrap_or(false) {
            lo += 1;
        }
        let hi = (lo + 1).min(count - 1);
        let rpm_of = |i: usize| loops[i].as_ref().map(|l| l.rpm).unwrap_or(1.0);
        let (rl, rh) = (rpm_of(lo), rpm_of(hi));
        let w = if hi == lo || rh <= rl { 0.0 } else { ((rpm / rl).ln() / (rh / rl).ln()).clamp(0.0, 1.0) };
        let gain = sample_mix * (0.5 + 0.5 * throttle);
        for (index, weight) in [(lo, 1.0 - w), (hi, w)] {
            if weight <= 1.0e-3 || (index == hi && hi == lo) {
                continue;
            }
            let Some(l) = loops[index].as_mut() else { continue };
            let len = l.pcm.len();
            if len < 4 {
                continue;
            }
            let step = (rpm / l.rpm) as f64 * (l.rate / device_rate) as f64 * doppler as f64;
            let equal_power = (weight * core::f32::consts::FRAC_PI_2).sin();
            for o in out.iter_mut() {
                let i = l.pos as usize;
                let f = (l.pos - i as f64) as f32;
                let a = l.pcm[i % len];
                let b = l.pcm[(i + 1) % len];
                *o += (a + (b - a) * f) * equal_power * gain;
                l.pos += step;
                if l.pos >= len as f64 {
                    l.pos -= len as f64;
                }
            }
        }
    }

    pub fn set(&mut self, engine: EngineInput, tyres: TyreInput) {
        self.engine.set_input(engine);
        self.tyres.set_input(tyres);
    }

    pub fn set_interior(&mut self, interior: f32) {
        let i = finite(interior, 0.0).clamp(0.0, 1.0);
        self.engine.interior = i;
        self.tyres.interior = i;
    }

    /// Render mono, ADDING into `out`.
    pub fn render(&mut self, out: &mut [f32], doppler: f32) {
        let doppler = finite(doppler, 1.0).clamp(0.5, 2.0);
        let rate_eff = self.engine.rate() / doppler;
        let dt = 1.0 / rate_eff;
        let mut at = 0;
        while at < out.len() {
            let len = (out.len() - at).min(256);
            let buf = &mut self.scratch[..len];
            buf.fill(0.0);
            self.engine.render(buf, doppler);
            if self.sample_mix > 0.0 {
                let synth = 1.0 - self.sample_mix;
                buf.iter_mut().for_each(|x| *x *= synth);
                let rate = self.engine.rate();
                let input = self.engine.input();
                Self::render_loops(&mut self.loops, input, self.sample_mix, buf, doppler, rate);
            }
            for i in 0..len {
                out[at + i] += buf[i] * self.engine_gain + self.tyres.next(dt, rate_eff) * self.tyre_gain;
            }
            at += len;
        }
    }

    pub fn level(&self) -> f32 {
        self.engine.level
    }
}
