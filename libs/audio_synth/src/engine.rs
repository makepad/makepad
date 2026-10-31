//! Physically-inspired engine sound.
//!
//! A piston engine is not a pitched oscillator: it is a train of exhaust
//! blow-down pulses, one per cylinder firing, whose TIMING comes from the
//! crank angle and the firing order, whose STRENGTH comes from the load, and
//! whose COLOUR comes from the pipes and boxes they travel through. This
//! model renders exactly that:
//!
//! - the crank turns at the given rpm; every cylinder fires at its own angle
//!   in the 720° (four-stroke) or 360° (two-stroke) cycle, so an inline-4,
//!   a cross-plane V8's uneven per-bank pattern (the burble), a 45° V-twin's
//!   potato-potato or a V12's smooth tear all fall out of the firing table;
//! - each firing is a smooth pressure pulse (band-limited by its width in
//!   crank degrees) plus turbulent blow-down noise, scaled by combustion
//!   pressure (load) with per-cycle variation (roughness, idle lope);
//! - each bank's pulses run down a header (a lossy feedback comb tuned to the
//!   pipe length) into a shared collector pipe, then a muffler of resonant
//!   chambers, a load-dependent brightness low-pass and a saturation stage;
//! - an intake path (induction noise through a Helmholtz resonance), valve
//!   train ticks and gear whine sit beside it and dominate the interior mix;
//! - events: overrun crackle and pops on lift-off, ignition cut on shifts and
//!   at the rev limiter, turbo spool whistle and blow-off, supercharger
//!   whine, starter motor and catch.
//!
//! Electric motors, gas turbines and rotors use the same voice with their
//! own source models. Time runs through one `dt`, so doppler (a playback-
//! rate factor) shifts every partial and resonance together.
//!
//! Real-time: all delay lines are allocated in [`Engine::new`]; `render`
//! never allocates.

use crate::delay::Comb;
use crate::dsp::*;

pub const MAX_CYL: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Piston,
    Electric,
    Turbine,
    Rotor,
}

/// Everything that makes one engine sound like itself. Presets fill it; a
/// script can override any field by name ([`EngineSpec::set`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineSpec {
    pub kind: EngineKind,
    pub cylinders: usize,
    /// 720 for four-stroke, 360 for two-stroke.
    pub cycle_deg: f32,
    /// Firing angle of each cylinder in the cycle, in firing order.
    pub firing: [f32; MAX_CYL],
    /// Exhaust bank (0/1) of each firing.
    pub bank: [u8; MAX_CYL],
    pub idle_rpm: f32,
    pub redline_rpm: f32,
    /// Width of one blow-down pulse in crank degrees.
    pub pulse_deg: f32,
    /// Header (primary pipe) round-trip per bank, ms.
    pub header_ms: [f32; 2],
    /// Collector/tail pipe round-trip, ms.
    pub pipe_ms: f32,
    /// Open-end reflection (negative: pressure inverts at an open end).
    pub reflect: f32,
    /// Muffler chambers: (Hz, Q, gain).
    pub muffler: [(f32, f32, f32); 3],
    /// Engine block / cabin boom resonance, Hz.
    pub body_hz: f32,
    pub intake_hz: f32,
    pub intake: f32,
    /// Scales the exhaust low-pass corner (1 = normal).
    pub brightness: f32,
    pub drive: f32,
    /// Cycle-to-cycle combustion variation 0..1.
    pub roughness: f32,
    /// Big-cam idle lope 0..1 (uneven idle that smooths with revs).
    pub lope: f32,
    pub crackle: f32,
    pub turbo: f32,
    pub supercharger: f32,
    pub gear_whine: f32,
    pub mech: f32,
    /// Propeller / rotor blades.
    pub blades: u8,
    /// Propeller rpm per crank rpm (0 = no propeller).
    pub prop_ratio: f32,
    /// Tail rotor rpm per main rotor rpm (rotor kind).
    pub tail_ratio: f32,
    pub volume: f32,
}

impl Default for EngineSpec {
    fn default() -> Self {
        preset("inline4").unwrap()
    }
}

fn even(n: usize, cycle: f32) -> [f32; MAX_CYL] {
    let mut f = [0.0; MAX_CYL];
    for (i, v) in f.iter_mut().enumerate().take(n) {
        *v = cycle * i as f32 / n as f32;
    }
    f
}

fn alternate(n: usize) -> [u8; MAX_CYL] {
    let mut b = [0; MAX_CYL];
    for (i, v) in b.iter_mut().enumerate().take(n) {
        *v = (i % 2) as u8;
    }
    b
}

fn base() -> EngineSpec {
    EngineSpec {
        kind: EngineKind::Piston,
        cylinders: 4,
        cycle_deg: 720.0,
        firing: even(4, 720.0),
        bank: alternate(4),
        idle_rpm: 850.0,
        redline_rpm: 7000.0,
        pulse_deg: 110.0,
        header_ms: [1.6, 1.9],
        pipe_ms: 7.5,
        reflect: -0.55,
        muffler: [(120.0, 3.0, 0.8), (340.0, 4.0, 0.5), (900.0, 3.0, 0.25)],
        body_hz: 85.0,
        intake_hz: 260.0,
        intake: 0.35,
        brightness: 1.0,
        drive: 1.2,
        roughness: 0.06,
        lope: 0.0,
        crackle: 0.15,
        turbo: 0.0,
        supercharger: 0.0,
        gear_whine: 0.1,
        mech: 0.25,
        blades: 0,
        prop_ratio: 0.0,
        tail_ratio: 0.0,
        volume: 1.0,
    }
}

/// Every preset name, primary spelling.
pub const PRESET_NAMES: &[&str] = &[
    "inline4", "inline4_turbo", "v6", "inline6", "v8", "v8_flat", "flat6", "v10", "v12",
    "diesel", "electric", "bike_single", "bike_twin", "bike_inline4", "kart", "outboard",
    "radial", "jet", "helicopter",
];

/// Named presets (with a few friendly aliases).
pub fn preset(name: &str) -> Option<EngineSpec> {
    let mut s = base();
    match name {
        // Hatchback: 1.6 l inline-4, 4-2-1 header, small muffler.
        "inline4" | "i4" | "hatch" | "hatchback" | "car" | "sedan" => {}
        "inline4_turbo" | "turbo4" | "rally" | "hot_hatch" => {
            s.turbo = 1.0;
            s.crackle = 0.55;
            s.brightness = 1.2;
            s.drive = 1.8;
            s.redline_rpm = 7500.0;
            s.muffler = [(140.0, 3.0, 0.7), (420.0, 3.5, 0.45), (1300.0, 3.0, 0.3)];
        }
        // 60° V6: 120° even, banks alternate.
        "v6" => {
            s.cylinders = 6;
            s.firing = even(6, 720.0);
            s.bank = alternate(6);
            s.header_ms = [1.8, 2.3];
            s.pulse_deg = 100.0;
            s.muffler = [(110.0, 3.0, 0.8), (310.0, 4.0, 0.5), (1000.0, 3.0, 0.25)];
            s.redline_rpm = 6800.0;
        }
        // Straight-six: smooth, one bank, long collector.
        "inline6" | "i6" | "straight6" => {
            s.cylinders = 6;
            s.firing = even(6, 720.0);
            s.bank = [0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            s.header_ms = [2.4, 2.4];
            s.pipe_ms = 9.0;
            s.pulse_deg = 95.0;
            s.brightness = 1.15;
            s.muffler = [(130.0, 3.0, 0.7), (390.0, 4.5, 0.5), (1400.0, 3.0, 0.3)];
            s.redline_rpm = 7200.0;
        }
        // Cross-plane V8 (GM order 1-8-4-3-6-5-7-2, odd bank left): even 90°
        // firing overall, UNEVEN per bank (L R R L R L L R) = the burble.
        "v8" | "muscle" | "v8_muscle" | "pickup" | "truck" | "police" => {
            s.cylinders = 8;
            s.firing = even(8, 720.0);
            s.bank = [0, 1, 1, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
            s.idle_rpm = 750.0;
            s.redline_rpm = 6200.0;
            s.pulse_deg = 125.0;
            s.header_ms = [2.8, 3.3];
            s.pipe_ms = 11.0;
            s.reflect = -0.62;
            s.muffler = [(75.0, 2.5, 1.0), (210.0, 3.0, 0.6), (620.0, 3.0, 0.25)];
            s.body_hz = 60.0;
            s.intake_hz = 180.0;
            s.brightness = 0.8;
            s.drive = 2.2;
            s.roughness = 0.1;
            s.lope = 0.45;
            s.crackle = 0.5;
            s.supercharger = 0.0;
        }
        // Flat-plane V8 (race / Italian): banks alternate evenly = scream.
        "v8_flat" | "flatplane" | "supercar" | "gt" => {
            s.cylinders = 8;
            s.firing = even(8, 720.0);
            s.bank = alternate(8);
            s.redline_rpm = 8800.0;
            s.pulse_deg = 100.0;
            s.header_ms = [1.5, 1.7];
            s.pipe_ms = 6.0;
            s.muffler = [(160.0, 2.5, 0.6), (520.0, 3.0, 0.5), (1600.0, 3.0, 0.35)];
            s.brightness = 1.4;
            s.drive = 2.0;
            s.crackle = 0.6;
            s.gear_whine = 0.3;
        }
        // Boxer six (1-6-2-4-3-5, banks alternate): the GT3 howl.
        "flat6" | "boxer6" | "gt3" | "racecar" | "race" => {
            s.cylinders = 6;
            s.firing = even(6, 720.0);
            s.bank = alternate(6);
            s.redline_rpm = 9000.0;
            s.idle_rpm = 950.0;
            s.pulse_deg = 95.0;
            s.header_ms = [1.4, 1.6];
            s.pipe_ms = 5.5;
            s.muffler = [(180.0, 2.5, 0.5), (560.0, 3.5, 0.55), (1700.0, 3.5, 0.35)];
            s.intake_hz = 420.0;
            s.intake = 0.5;
            s.brightness = 1.5;
            s.drive = 1.8;
            s.crackle = 0.45;
            s.gear_whine = 0.45;
            s.mech = 0.35;
        }
        // F1 3.0 V10 (72° even, banks alternate): the screamer.
        "v10" | "f1" | "formula" | "screamer" => {
            s.cylinders = 10;
            s.firing = even(10, 720.0);
            s.bank = alternate(10);
            s.idle_rpm = 4000.0;
            s.redline_rpm = 18_500.0;
            s.pulse_deg = 80.0;
            s.header_ms = [0.9, 1.0];
            s.pipe_ms = 3.0;
            s.reflect = -0.45;
            s.muffler = [(300.0, 2.0, 0.3), (900.0, 2.5, 0.5), (2400.0, 3.0, 0.4)];
            s.body_hz = 140.0;
            s.intake_hz = 700.0;
            s.intake = 0.6;
            s.brightness = 1.9;
            s.drive = 2.4;
            s.roughness = 0.03;
            s.crackle = 0.3;
            s.gear_whine = 0.6;
            s.mech = 0.4;
        }
        "v12" => {
            s.cylinders = 12;
            s.firing = even(12, 720.0);
            s.bank = alternate(12);
            s.redline_rpm = 8500.0;
            s.pulse_deg = 85.0;
            s.header_ms = [1.3, 1.45];
            s.pipe_ms = 6.5;
            s.muffler = [(150.0, 2.5, 0.5), (480.0, 3.0, 0.5), (1500.0, 3.0, 0.35)];
            s.brightness = 1.5;
            s.drive = 1.7;
            s.roughness = 0.03;
            s.crackle = 0.35;
        }
        // Turbo-diesel straight six: low redline, knock, big turbo.
        "diesel" | "lorry" | "bus" | "tractor" => {
            s.cylinders = 6;
            s.firing = even(6, 720.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 600.0;
            s.redline_rpm = 2600.0;
            s.pulse_deg = 70.0;
            s.header_ms = [3.5, 3.5];
            s.pipe_ms = 14.0;
            s.muffler = [(60.0, 2.5, 1.0), (180.0, 3.0, 0.5), (500.0, 3.0, 0.2)];
            s.body_hz = 45.0;
            s.brightness = 0.7;
            s.drive = 1.5;
            s.roughness = 0.12;
            s.crackle = 0.0;
            s.turbo = 0.8;
            s.mech = 0.9;
        }
        "electric" | "ev" | "tram" => {
            s.kind = EngineKind::Electric;
            s.cylinders = 0;
            s.idle_rpm = 0.0;
            s.redline_rpm = 16_000.0;
            s.gear_whine = 0.6;
            s.crackle = 0.0;
            s.mech = 0.2;
        }
        // Big single four-stroke (thumper).
        "bike_single" | "single" | "dirtbike" | "scooter" => {
            s.cylinders = 1;
            s.firing = even(1, 720.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 1300.0;
            s.redline_rpm = 9500.0;
            s.pulse_deg = 150.0;
            s.header_ms = [2.2, 2.2];
            s.pipe_ms = 5.0;
            s.muffler = [(140.0, 2.0, 0.6), (450.0, 3.0, 0.4), (1400.0, 3.0, 0.3)];
            s.body_hz = 110.0;
            s.brightness = 1.1;
            s.drive = 2.5;
            s.roughness = 0.12;
            s.crackle = 0.5;
            s.gear_whine = 0.0;
            s.mech = 0.5;
        }
        // 45° V-twin: fires at 0° and 315° — the potato-potato.
        "bike_twin" | "vtwin" | "v_twin" | "cruiser" | "chopper" | "motorbike" | "motorcycle" => {
            s.cylinders = 2;
            let mut f = [0.0; MAX_CYL];
            f[1] = 315.0;
            s.firing = f;
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 900.0;
            s.redline_rpm = 5800.0;
            s.pulse_deg = 150.0;
            s.header_ms = [2.6, 3.0];
            s.pipe_ms = 6.0;
            s.muffler = [(90.0, 2.0, 0.9), (260.0, 3.0, 0.5), (800.0, 3.0, 0.3)];
            s.body_hz = 70.0;
            s.brightness = 0.9;
            s.drive = 2.6;
            s.roughness = 0.12;
            s.lope = 0.25;
            s.crackle = 0.6;
            s.gear_whine = 0.0;
            s.mech = 0.6;
        }
        // Inline-4 superbike: 180° crank, 14k redline.
        "bike_inline4" | "superbike" | "sportbike" => {
            s.cylinders = 4;
            s.firing = even(4, 720.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 1300.0;
            s.redline_rpm = 14_000.0;
            s.pulse_deg = 95.0;
            s.header_ms = [1.2, 1.2];
            s.pipe_ms = 3.5;
            s.muffler = [(220.0, 2.0, 0.4), (700.0, 3.0, 0.5), (2200.0, 3.0, 0.4)];
            s.intake_hz = 500.0;
            s.intake = 0.6;
            s.brightness = 1.6;
            s.drive = 2.0;
            s.crackle = 0.55;
            s.gear_whine = 0.1;
        }
        // Kart 2-stroke single: fires every revolution, expansion chamber.
        "kart" | "two_stroke" | "2stroke" | "moped" => {
            s.cylinders = 1;
            s.cycle_deg = 360.0;
            s.firing = even(1, 360.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 2500.0;
            s.redline_rpm = 15_000.0;
            s.pulse_deg = 120.0;
            s.header_ms = [1.8, 1.8];
            s.pipe_ms = 2.6;
            s.reflect = -0.7;
            s.muffler = [(400.0, 2.0, 0.5), (1100.0, 2.5, 0.6), (3000.0, 3.0, 0.3)];
            s.body_hz = 160.0;
            s.intake_hz = 900.0;
            s.intake = 0.5;
            s.brightness = 1.7;
            s.drive = 3.0;
            s.roughness = 0.15;
            s.crackle = 0.2;
            s.gear_whine = 0.0;
            s.mech = 0.3;
        }
        // Outboard: 2-stroke triple, wet exhaust.
        "outboard" | "boat" | "jetski" => {
            s.cylinders = 3;
            s.cycle_deg = 360.0;
            s.firing = even(3, 360.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 750.0;
            s.redline_rpm = 6000.0;
            s.pulse_deg = 100.0;
            s.header_ms = [2.0, 2.0];
            s.pipe_ms = 8.0;
            s.muffler = [(100.0, 2.0, 0.8), (320.0, 2.5, 0.5), (900.0, 3.0, 0.2)];
            s.brightness = 0.75;
            s.drive = 1.8;
            s.roughness = 0.1;
            s.crackle = 0.0;
            s.gear_whine = 0.35;
            s.mech = 0.4;
        }
        // 9-cylinder radial with a 3-blade propeller.
        "radial" | "prop" | "plane" | "warbird" => {
            s.cylinders = 9;
            s.firing = even(9, 720.0);
            s.bank = [0; MAX_CYL];
            s.idle_rpm = 600.0;
            s.redline_rpm = 2700.0;
            s.pulse_deg = 90.0;
            s.header_ms = [1.2, 1.2];
            s.pipe_ms = 2.5;
            s.reflect = -0.4;
            s.muffler = [(90.0, 2.0, 0.8), (300.0, 2.5, 0.5), (1000.0, 3.0, 0.3)];
            s.body_hz = 55.0;
            s.brightness = 1.1;
            s.drive = 2.2;
            s.roughness = 0.15;
            s.crackle = 0.25;
            s.gear_whine = 0.0;
            s.mech = 0.4;
            s.blades = 3;
            s.prop_ratio = 0.9;
        }
        "jet" | "turbine" | "turbofan" | "fighter" | "jetliner" => {
            s.kind = EngineKind::Turbine;
            s.cylinders = 0;
            s.idle_rpm = 0.25;
            s.redline_rpm = 1.0;
            s.blades = 24;
            s.crackle = 0.0;
        }
        "helicopter" | "heli" | "chopper_heli" | "rotor" => {
            s.kind = EngineKind::Rotor;
            s.cylinders = 0;
            s.idle_rpm = 300.0;
            s.redline_rpm = 330.0;
            s.blades = 4;
            s.tail_ratio = 5.2;
            s.crackle = 0.0;
        }
        _ => return None,
    }
    Some(s)
}

impl EngineSpec {
    /// Override one field by name (the script-facing spelling).
    pub fn set(&mut self, key: &str, v: f32) -> Result<(), String> {
        let v = finite(v, 0.0);
        match key {
            "cylinders" => {
                // An even-fire engine of that many cylinders, banks alternating.
                let n = (v as usize).clamp(1, MAX_CYL);
                self.cylinders = n;
                self.firing = even(n, self.cycle_deg);
                self.bank = alternate(n);
            }
            "strokes" => {
                self.cycle_deg = if v <= 2.5 { 360.0 } else { 720.0 };
                self.firing = even(self.cylinders.max(1), self.cycle_deg);
            }
            "idle" | "idle_rpm" => self.idle_rpm = v.clamp(0.0, 30_000.0),
            "redline" | "redline_rpm" => self.redline_rpm = v.clamp(1.0, 30_000.0),
            "pulse" | "pulse_deg" => self.pulse_deg = v.clamp(20.0, 300.0),
            "pipe" | "pipe_ms" => self.pipe_ms = v.clamp(0.5, 40.0),
            "header" | "header_ms" => self.header_ms = [v.clamp(0.3, 20.0), (v * 1.15).clamp(0.3, 20.0)],
            "brightness" => self.brightness = v.clamp(0.2, 4.0),
            "drive" => self.drive = v.clamp(0.0, 8.0),
            "roughness" => self.roughness = v.clamp(0.0, 1.0),
            "lope" => self.lope = v.clamp(0.0, 1.0),
            "crackle" | "pops" => self.crackle = v.clamp(0.0, 2.0),
            "turbo" => self.turbo = v.clamp(0.0, 2.0),
            "supercharger" | "blower" => self.supercharger = v.clamp(0.0, 2.0),
            "whine" | "gear_whine" => self.gear_whine = v.clamp(0.0, 2.0),
            "mech" | "mechanical" => self.mech = v.clamp(0.0, 2.0),
            "intake" => self.intake = v.clamp(0.0, 2.0),
            "blades" => self.blades = v.clamp(0.0, 12.0) as u8,
            "volume" => self.volume = v.clamp(0.0, 4.0),
            "muffler" => {
                // Shift all chambers: 1 = as preset, 2 = an octave up.
                for m in self.muffler.iter_mut() {
                    m.0 = (m.0 * v.clamp(0.25, 4.0)).clamp(30.0, 8000.0);
                }
            }
            _ => return Err(format!("unknown engine key '{key}'")),
        }
        Ok(())
    }
}

/// What drives the engine each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineInput {
    pub rpm: f32,
    /// Pedal 0..1.
    pub throttle: f32,
    /// Engine load −1..1 (negative = overrun / engine braking).
    pub load: f32,
    pub gear: i8,
    /// Road/air speed in m/s (gear whine, wind).
    pub speed: f32,
}

impl Default for EngineInput {
    fn default() -> Self {
        EngineInput { rpm: 800.0, throttle: 0.0, load: 0.0, gear: 0, speed: 0.0 }
    }
}

const CTL: u32 = 32;

/// One running engine.
pub struct Engine {
    pub spec: EngineSpec,
    rate: f32,
    rng: Rng,
    // Smoothed inputs.
    rpm: Smooth,
    throttle: Smooth,
    load: Smooth,
    input: EngineInput,
    // Piston state.
    crank: f32,
    last_local: [f32; MAX_CYL],
    amp: [f32; MAX_CYL],
    cut: [bool; MAX_CYL],
    pop: [f32; 2],
    headers: [Comb; 2],
    pipe: Comb,
    muffler: [Resonator; 3],
    body: Resonator,
    intake_res: Resonator,
    intake_res2: Resonator,
    bright: Filter,
    tail_hp: DcBlock,
    cabin_lp: OnePole,
    noise: Noise,
    noise2: Noise,
    crack_env: f32,
    crack_hp: OnePole,
    tick_env: f32,
    tick_hp: OnePole,
    // Events.
    overrun_t: f32,
    heat: f32,
    shift_cut: f32,
    limiter_t: f32,
    limiter_cut: bool,
    // Turbo / blower.
    spool: f32,
    bov_env: f32,
    bov_bp: Filter,
    whistle: Osc,
    whistle_bp: Filter,
    throttle_hist: f32,
    blower: Osc,
    // Gear whine.
    whine: Osc,
    // Electric.
    motor: [Osc; 3],
    carrier: Osc,
    // Turbine.
    n1: f32,
    comp: [Osc; 2],
    buzz: Osc,
    roar_lp: Filter,
    roar_bp: Filter,
    // Rotor / propeller.
    blade_phase: f32,
    tail_phase: f32,
    slap_bp: Resonator,
    slap_bp2: Resonator,
    tail_bp: Resonator,
    prop_lp: OnePole,
    // Starter.
    starter_t: f32,
    starting: bool,
    running: bool,
    flare: f32,
    starter_osc: Osc,
    ctl: u32,
    /// 0 = exterior (exhaust), 1 = interior (cabin).
    pub interior: f32,
    interior_s: Smooth,
    /// Per-sample glide coefficient for `interior_s` (set per render).
    interior_c: f32,
    /// Peak of the last render, for voice stealing and metering.
    pub level: f32,
}

impl Engine {
    pub fn new(rate: f32, spec: EngineSpec) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        // Room for the longest pipe at half doppler.
        let max = (0.045 * rate * 2.0) as usize;
        let mut e = Engine {
            spec,
            rate,
            rng: Rng::new(0x9e37_79b9),
            rpm: Smooth::new(spec.idle_rpm),
            throttle: Smooth::new(0.0),
            load: Smooth::new(0.0),
            input: EngineInput { rpm: spec.idle_rpm, ..Default::default() },
            crank: 0.0,
            last_local: [0.0; MAX_CYL],
            amp: [1.0; MAX_CYL],
            cut: [false; MAX_CYL],
            pop: [0.0; 2],
            headers: [Comb::new(max), Comb::new(max)],
            pipe: Comb::new(max),
            muffler: [Resonator::new(100.0, 1.0, 0.0, rate); 3],
            body: Resonator::new(80.0, 3.0, 1.0, rate),
            intake_res: Resonator::new(250.0, 4.0, 1.0, rate),
            intake_res2: Resonator::new(700.0, 3.0, 1.0, rate),
            bright: {
                let mut f = Filter::new(FilterMode::Lowpass);
                f.steep = true;
                f
            },
            tail_hp: DcBlock::default(),
            cabin_lp: OnePole::new(700.0, rate),
            noise: Noise::new(NoiseColor::White, 0x1234_5678),
            noise2: Noise::new(NoiseColor::Pink, 0x0bad_cafe),
            crack_env: 0.0,
            crack_hp: OnePole::new(900.0, rate),
            tick_env: 0.0,
            tick_hp: OnePole::new(4000.0, rate),
            overrun_t: 0.0,
            heat: 0.0,
            shift_cut: 0.0,
            limiter_t: 0.0,
            limiter_cut: false,
            spool: 0.0,
            bov_env: 0.0,
            bov_bp: Filter::new(FilterMode::Bandpass),
            whistle: Osc::new(Wave::Sine, 0.0),
            whistle_bp: Filter::new(FilterMode::Bandpass),
            throttle_hist: 0.0,
            blower: Osc::new(Wave::Triangle, 0.0),
            whine: Osc::new(Wave::Sine, 0.0),
            motor: [Osc::new(Wave::Sine, 0.0), Osc::new(Wave::Sine, 0.3), Osc::new(Wave::Triangle, 0.6)],
            carrier: Osc::new(Wave::Sine, 0.0),
            n1: 0.25,
            comp: [Osc::new(Wave::Sine, 0.0), Osc::new(Wave::Sine, 0.5)],
            buzz: Osc::new(Wave::Saw, 0.0),
            roar_lp: Filter::new(FilterMode::Lowpass),
            roar_bp: Filter::new(FilterMode::Bandpass),
            blade_phase: 0.0,
            tail_phase: 0.0,
            slap_bp: Resonator::new(90.0, 2.0, 1.0, rate),
            slap_bp2: Resonator::new(260.0, 2.5, 1.0, rate),
            tail_bp: Resonator::new(420.0, 3.0, 1.0, rate),
            prop_lp: OnePole::new(1800.0, rate),
            starter_t: 0.0,
            starting: false,
            running: true,
            flare: 0.0,
            starter_osc: Osc::new(Wave::Saw, 0.0),
            ctl: 0,
            interior: 0.0,
            interior_s: Smooth::new(0.0),
            interior_c: 0.0,
            level: 0.0,
        };
        e.set_spec(spec);
        e
    }

    pub fn set_spec(&mut self, spec: EngineSpec) {
        self.spec = spec;
        for (i, m) in self.muffler.iter_mut().enumerate() {
            let (hz, q, g) = spec.muffler[i];
            *m = Resonator::new(hz, q, g, self.rate);
        }
        self.ctl = 0;
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Seed per-instance variation (two identical cars must not phase).
    pub fn seed(&mut self, seed: u32) {
        self.rng = Rng::new(seed.wrapping_mul(0x2545_f491) | 1);
        self.crank = self.rng.unit() * self.spec.cycle_deg;
    }

    pub fn set_input(&mut self, input: EngineInput) {
        let input = EngineInput {
            rpm: finite(input.rpm, self.spec.idle_rpm).clamp(0.0, 40_000.0),
            throttle: finite(input.throttle, 0.0).clamp(0.0, 1.0),
            load: finite(input.load, 0.0).clamp(-1.0, 1.0),
            gear: input.gear,
            speed: finite(input.speed, 0.0),
        };
        // Shift: an ignition cut for the shift time.
        if input.gear != self.input.gear && input.gear != 0 && self.input.gear != 0 {
            self.shift_cut = 0.085;
        }
        self.input = input;
        self.rpm.target = input.rpm;
        self.throttle.target = input.throttle;
        self.load.target = input.load;
    }

    pub fn input(&self) -> EngineInput {
        self.input
    }

    /// Crank the starter; the engine catches after ~0.7 s.
    pub fn start(&mut self) {
        self.starting = true;
        self.running = false;
        self.starter_t = 0.0;
    }

    /// Ignition off: combustion stops and the engine winds down.
    pub fn stop(&mut self) {
        self.running = false;
        self.starting = false;
    }

    pub fn running(&self) -> bool {
        self.running || self.starting
    }

    fn retune(&mut self, rate_eff: f32, rpm_norm: f32, load_pos: f32) {
        let s = &self.spec;
        for (i, m) in self.muffler.iter_mut().enumerate() {
            let (hz, q, g) = s.muffler[i];
            m.tune(hz, q, rate_eff);
            m.gain = g;
        }
        self.body.tune(s.body_hz, 3.0, rate_eff);
        let ih = s.intake_hz * (0.8 + 0.5 * rpm_norm);
        self.intake_res.tune(ih, 4.0, rate_eff);
        self.intake_res2.tune(ih * 2.7, 3.0, rate_eff);
        let cutoff = s.brightness * (450.0 + 2300.0 * load_pos + 2000.0 * rpm_norm);
        self.bright.set(cutoff, 0.6, rate_eff);
        for b in 0..2 {
            self.headers[b].delay = s.header_ms[b] * 0.001 * rate_eff;
            self.headers[b].feedback = s.reflect;
            self.headers[b].damp = 0.35;
        }
        self.pipe.delay = s.pipe_ms * 0.001 * rate_eff;
        self.pipe.feedback = s.reflect * 0.6;
        self.pipe.damp = 0.45;
        self.crack_hp.set(900.0, rate_eff);
        self.tick_hp.set(3500.0, rate_eff);
        self.cabin_lp.set(650.0 + 500.0 * load_pos, rate_eff);
    }

    /// Render mono samples, ADDING into `out`. `doppler` is a playback-rate
    /// factor applied to everything (1 = none).
    pub fn render(&mut self, out: &mut [f32], doppler: f32) {
        let doppler = finite(doppler, 1.0).clamp(0.5, 2.0);
        let rate_eff = self.rate / doppler;
        let dt = 1.0 / rate_eff;
        let rc = Smooth::coef(0.035, rate_eff);
        let tc = Smooth::coef(0.02, rate_eff);
        self.interior_c = Smooth::coef(0.2, rate_eff);
        self.interior_s.target = self.interior;
        let mut peak = 0.0f32;
        for sample in out.iter_mut() {
            let throttle = self.throttle.next(tc);
            let load = self.load.next(tc);
            let mut rpm = self.rpm.next(rc);
            // Starter and catch override the driven rpm.
            if self.starting {
                self.starter_t += dt;
                rpm = 200.0 + 40.0 * (self.starter_t * 9.0).sin();
                if self.starter_t > 0.75 {
                    self.starting = false;
                    self.running = true;
                    self.flare = 1.0;
                }
            } else if !self.running {
                // Wind down.
                self.flare = 0.0;
            }
            if self.flare > 0.0 {
                self.flare = (self.flare - dt * 1.4).max(0.0);
                let f = self.flare;
                rpm = rpm.max(self.spec.idle_rpm * (1.0 + 0.9 * f * f * (1.0 - f) * 6.75));
            }
            let x = match self.spec.kind {
                EngineKind::Piston => self.piston(rpm, throttle, load, dt, rate_eff),
                EngineKind::Electric => self.electric(rpm, load, dt),
                EngineKind::Turbine => self.turbine(throttle, dt, rate_eff),
                EngineKind::Rotor => self.rotor(rpm, throttle, dt, rate_eff),
            };
            let x = x * self.spec.volume;
            peak = peak.max(x.abs());
            *sample += x;
        }
        self.level = self.level * 0.5 + peak * 0.5;
    }

    #[inline]
    fn piston(&mut self, rpm_in: f32, throttle: f32, load: f32, dt: f32, rate_eff: f32) -> f32 {
        let s = self.spec;
        let ignition = self.running;
        let rpm = if ignition || self.starting { rpm_in } else { (self.rpm.value * (1.0 - dt * 1.2)).max(0.0) };
        if !ignition && !self.starting {
            self.rpm.value = rpm;
        }
        let span = (s.redline_rpm - s.idle_rpm).max(1.0);
        let rpm_norm = ((rpm - s.idle_rpm) / span).clamp(0.0, 1.2);
        let load_pos = load.max(0.0).max(throttle * 0.6);
        if self.ctl == 0 {
            self.retune(rate_eff, rpm_norm.min(1.0), load_pos);
        }
        self.ctl = (self.ctl + 1) % CTL;

        // ── events ──
        // Overrun = the engine is being driven by the car (no fuel, load at
        // or below zero), not merely a light throttle.
        if throttle < 0.08 && load <= 0.02 && rpm_norm > 0.3 {
            self.overrun_t += dt;
        } else {
            self.overrun_t = 0.0;
        }
        self.heat += ((load_pos * rpm_norm.min(1.0)) - self.heat) * dt * 0.8;
        if self.shift_cut > 0.0 {
            self.shift_cut -= dt;
        }
        let at_limiter = rpm >= s.redline_rpm * 0.985 && throttle > 0.4;
        if at_limiter {
            self.limiter_t += dt;
            // Hard-cut limiter: ~18 Hz on/off.
            self.limiter_cut = (self.limiter_t * 18.0).fract() < 0.5;
        } else {
            self.limiter_cut = false;
        }
        let combustion = if !ignition {
            0.06 // compression only (starter / wind-down)
        } else if self.shift_cut > 0.0 {
            0.04
        } else if throttle < 0.05 && load <= 0.02 {
            0.13 // overrun: hollow
        } else {
            0.25 + 0.75 * load_pos
        };

        // ── crank and firings ──
        self.crank += rpm * 6.0 * dt;
        let cycle = s.cycle_deg.max(1.0);
        if self.crank >= cycle {
            self.crank -= cycle;
        }
        let mut bank_in = [0.0f32; 2];
        let mut activity = 0.0f32;
        let n = s.cylinders.min(MAX_CYL);
        let pw = s.pulse_deg.max(10.0);
        let crackle_chance = s.crackle
            * if self.overrun_t > 0.0 {
                0.35 * (-self.overrun_t / 1.6).exp() * (0.3 + self.heat)
            } else if self.shift_cut > 0.0 {
                0.5
            } else if self.limiter_cut {
                0.25
            } else {
                0.0
            };
        for i in 0..n {
            let mut local = self.crank - s.firing[i];
            if local < 0.0 {
                local += cycle;
            }
            if local < self.last_local[i] {
                // A new firing of cylinder i: roll its strength.
                let lope = s.lope * (1.0 - rpm_norm.min(1.0) * 2.0).max(0.0);
                let r = self.rng.bipolar();
                self.amp[i] = (1.0 + s.roughness * r + lope * self.rng.bipolar() * 1.2).max(0.1);
                self.cut[i] = self.limiter_cut && self.rng.unit() < 0.8;
                if ignition && self.rng.unit() < crackle_chance {
                    let b = (s.bank[i] as usize).min(1);
                    self.pop[b] = 1.5 + 3.0 * self.rng.unit();
                    self.crack_env = self.crack_env.max(0.5 + 0.5 * self.rng.unit());
                }
                if s.mech > 0.0 {
                    self.tick_env = 1.0;
                }
            }
            self.last_local[i] = local;
            if local < pw {
                let u = local / pw;
                // Fast rise, slower decay (peaks at u = 1/3), smooth at both ends.
                let shape = u * (1.0 - u) * (1.0 - u) * 6.75;
                let strength = if self.cut[i] { 0.03 } else { combustion };
                let b = (s.bank[i] as usize).min(1);
                // Blow-down turbulence rides the pulse.
                let turb = 1.0 + self.noise.next() * (0.12 + 0.3 * load_pos);
                bank_in[b] += shape * self.amp[i] * strength * turb;
                activity += shape;
            }
        }
        // Pops: an extra combustion in the hot header.
        for b in 0..2 {
            if self.pop[b] > 0.0 {
                bank_in[b] += self.pop[b] * self.noise.next().abs();
                self.pop[b] *= 1.0 - dt * 900.0;
                if self.pop[b] < 0.01 {
                    self.pop[b] = 0.0;
                }
            }
        }

        // ── exhaust ──
        let h0 = self.headers[0].process(bank_in[0]);
        let h1 = self.headers[1].process(bank_in[1]);
        let coll = self.pipe.process((h0 + h1) * 0.7);
        let mut m = coll * 0.7;
        for r in self.muffler.iter_mut() {
            m += r.process(coll) * 0.6;
        }
        // High-amplitude pulses steepen as they travel (the rasp): saturate
        // FIRST, then let the pipe/muffler low-pass take the fizz back out,
        // so the distortion's upper harmonics never reach the ear raw.
        let drive = 1.0 + s.drive * (0.3 + 0.7 * load_pos);
        let sat = asym_clip(m * drive * 1.3, 0.12) / drive.sqrt();
        let exhaust = self.tail_hp.process(self.bright.process(sat));
        let boom = self.body.process(bank_in[0] + bank_in[1]);

        // ── pops' crack ──
        let mut crack = 0.0;
        if self.crack_env > 0.001 {
            crack = self.crack_hp.hp(self.noise.next()) * self.crack_env * 0.9;
            self.crack_env *= 1.0 - dt * 160.0;
        }

        // ── intake ──
        let induction = self.noise2.next() * (0.2 + activity * 0.8) * (0.15 + 0.85 * throttle);
        let intake = (self.intake_res.process(induction) + 0.4 * self.intake_res2.process(induction)) * s.intake;

        // ── mechanical ──
        let mut mech = 0.0;
        if self.tick_env > 0.001 {
            mech = self.tick_hp.hp(self.noise.next()) * self.tick_env * s.mech * 0.08;
            self.tick_env *= 1.0 - dt * 900.0;
        }
        let whine_hz = self.input.speed.abs() * 38.0 + rpm * 0.02;
        let whine = if s.gear_whine > 0.0 && whine_hz > 40.0 {
            self.whine.next(whine_hz, dt, 0.0) * s.gear_whine * 0.012 * (0.4 + 0.6 * (1.0 - throttle)) * (self.input.speed.abs() / 40.0).min(1.0)
        } else {
            0.0
        };

        // ── forced induction ──
        let mut forced = 0.0;
        if s.turbo > 0.0 {
            let target = ((rpm_norm * 1.3 - 0.15) * load_pos * 1.4).clamp(0.0, 1.0);
            let k = if target > self.spool { 1.2 } else { 2.5 };
            self.spool += (target - self.spool) * dt * k;
            // Blow-off: a fast lift with boost up.
            let lift = self.throttle_hist - throttle;
            self.throttle_hist += (throttle - self.throttle_hist) * dt * 6.0;
            if lift > 0.35 && self.spool > 0.3 && self.bov_env < 0.1 {
                self.bov_env = self.spool;
                self.spool *= 0.3;
            }
            let wh = 1600.0 + 6500.0 * self.spool;
            self.whistle_bp.set(wh, 14.0, rate_eff);
            let tone = self.whistle.next(wh, dt, 0.0) * 0.6 + self.whistle_bp.process(self.noise.next()) * 2.0;
            forced += tone * self.spool * self.spool * (0.3 + 0.7 * throttle) * s.turbo * 0.05;
            if self.bov_env > 0.001 {
                let f = 900.0 + 2400.0 * self.bov_env;
                self.bov_bp.set(f, 1.4, rate_eff);
                forced += self.bov_bp.process(self.noise.next()) * self.bov_env * s.turbo * 0.35;
                self.bov_env *= 1.0 - dt * 3.5;
            }
        }
        if s.supercharger > 0.0 {
            let f = rpm / 60.0 * 9.0;
            forced += self.blower.next(f, dt, 0.0) * s.supercharger * 0.03 * (0.2 + 0.8 * throttle) * (0.3 + rpm_norm.min(1.0));
        }

        // ── starter motor ──
        let mut starter = 0.0;
        if self.starting {
            let f = 150.0 + 20.0 * (self.starter_t * 9.0).sin();
            starter = soft_clip(self.starter_osc.next(f, dt, 0.0) * 2.0) * 0.06 + self.tick_hp.hp(self.noise.next()) * 0.01;
        }

        // ── propeller (radial) ──
        let mut prop = 0.0;
        if s.prop_ratio > 0.0 && s.blades > 0 {
            let bp_hz = rpm * s.prop_ratio / 60.0 * s.blades as f32;
            self.blade_phase += bp_hz * dt;
            if self.blade_phase >= 1.0 {
                self.blade_phase -= 1.0;
            }
            let u = self.blade_phase / 0.3;
            let pulse = hann_pulse(u) * (0.5 + 0.5 * throttle);
            prop = self.prop_lp.lp(pulse + self.noise2.next() * pulse * 0.4) * 0.5;
        }

        let i = self.interior_s.next(self.interior_c);
        let outside = exhaust + crack * 0.35 + intake * 0.25 + mech + whine + forced + starter + prop + boom * 0.02;
        let inside = self.cabin_lp.lp(exhaust + prop) * 0.8 + intake * 0.8 + boom * 0.08 + mech * 1.6 + whine * 1.5 + forced * 0.6 + starter + crack * 0.1;
        (outside * (1.0 - i) + inside * i) * 0.6
    }

    #[inline]
    fn electric(&mut self, rpm: f32, load: f32, dt: f32) -> f32 {
        let s = self.spec;
        let f_m = rpm.max(0.0) / 60.0 * 4.0;
        let norm = (rpm / s.redline_rpm.max(1.0)).clamp(0.0, 1.0);
        let effort = load.abs().max(0.15);
        let regen = load < -0.05;
        let mut x = 0.0;
        if f_m > 5.0 {
            x += self.motor[0].next(f_m, dt, 0.0) * 0.05;
            x += self.motor[1].next(f_m * 2.0, dt, 0.0) * 0.03;
            x += self.motor[2].next(f_m * 6.0, dt, 0.0) * 0.012 * effort;
        }
        // Inverter: a carrier modulated by the motor's electrical frequency,
        // loudest at low speed (the tram sing), fading as it spins up.
        let carrier_hz = 1800.0 + 1400.0 * norm;
        let carrier = self.carrier.next(carrier_hz, dt, (f_m * 0.001).sin() * 0.0) * 0.02 * (1.0 - norm).max(0.15) * effort;
        // Reduction gear whine: THE EV sound at speed.
        let g_hz = rpm / 60.0 * 23.0;
        let whine = if g_hz > 40.0 {
            self.whine.next(g_hz, dt, 0.0) * s.gear_whine * 0.04 * (0.3 + 0.7 * norm) * if regen { 0.7 } else { 1.0 }
        } else {
            0.0
        };
        let hiss = self.noise2.next() * 0.004 * effort;
        (x * (0.4 + 0.6 * effort) + carrier + whine + hiss) * 1.3
    }

    #[inline]
    fn turbine(&mut self, throttle: f32, dt: f32, rate_eff: f32) -> f32 {
        let s = self.spec;
        // The driven "rpm" for a turbine is N1 as a fraction (0.25 idle .. 1).
        let target = if self.input.rpm > 0.0 && self.input.rpm <= 1.2 {
            self.input.rpm
        } else {
            s.idle_rpm + (1.0 - s.idle_rpm) * throttle
        };
        let k = if target > self.n1 { 0.45 } else { 0.6 };
        self.n1 += (target - self.n1) * dt * k;
        let n1 = self.n1.clamp(0.0, 1.1);
        if self.ctl == 0 {
            self.roar_lp.set(250.0 + 2600.0 * n1 * n1, 0.7, rate_eff);
            self.roar_bp.set(1200.0 + 800.0 * n1, 1.2, rate_eff);
        }
        self.ctl = (self.ctl + 1) % CTL;
        let shaft = n1 * 160.0;
        let blade = shaft * s.blades.max(1) as f32;
        let whine = self.comp[0].next(blade, dt, 0.0) * 0.05 + self.comp[1].next(blade * 1.52, dt, 0.0) * 0.025;
        let buzz = if n1 > 0.78 { self.buzz.next(shaft, dt, 0.0) * (n1 - 0.78) * 0.25 } else { 0.0 };
        let roar = self.roar_lp.process(self.noise2.next()) * (0.1 + 0.9 * n1 * n1) * 0.5;
        let hiss = self.roar_bp.process(self.noise.next()) * n1 * 0.15;
        let mut ab = 0.0;
        if throttle > 0.95 && n1 > 0.9 {
            // Afterburner: a low tearing roar with crackle.
            let mut v = self.noise.next();
            v = if self.rng.unit() < 0.02 { v * 6.0 } else { v };
            ab = self.body.process(v) * 0.3;
        }
        (whine * (0.6 + 0.4 * n1) + buzz + roar + hiss + ab) * 1.1
    }

    #[inline]
    fn rotor(&mut self, rpm: f32, throttle: f32, dt: f32, rate_eff: f32) -> f32 {
        let s = self.spec;
        if self.ctl == 0 {
            self.slap_bp.tune(95.0, 2.2, rate_eff);
            self.slap_bp2.tune(280.0, 2.5, rate_eff);
            self.tail_bp.tune(480.0, 3.0, rate_eff);
            self.crack_hp.set(1500.0, rate_eff);
            self.roar_lp.set(4500.0, 0.7, rate_eff);
        }
        self.ctl = (self.ctl + 1) % CTL;
        let rotor_hz = rpm.max(0.0) / 60.0;
        let bp_hz = rotor_hz * s.blades.max(1) as f32;
        self.blade_phase += bp_hz * dt;
        if self.blade_phase >= 1.0 {
            self.blade_phase -= 1.0;
        }
        // Blade slap: sharp with collective (blade-vortex interaction).
        let collective = throttle;
        let width = 0.16 - 0.08 * collective;
        let u = self.blade_phase / width;
        let slap = hann_pulse(u);
        // Blade-vortex crack: band-limited noise (a slap is sharp, not hiss).
        let bvi = if u < 1.0 { self.roar_lp.process(self.crack_hp.hp(self.noise.next())) * slap * collective * 1.2 } else { 0.0 };
        let body = self.slap_bp.process(slap) * 1.2 + self.slap_bp2.process(slap) * 0.5;
        // Continuous rotor wash, amplitude-modulated by the blade pass.
        let wash = self.noise2.next() * (0.35 + 0.65 * (1.0 - (self.blade_phase * TAU).cos()) * 0.5) * 0.12;
        // Tail rotor buzz.
        self.tail_phase += rotor_hz * s.tail_ratio * 2.0 * dt;
        if self.tail_phase >= 1.0 {
            self.tail_phase -= 1.0;
        }
        let tail = self.tail_bp.process(hann_pulse(self.tail_phase / 0.3)) * 0.25;
        // Turboshaft whine.
        let whine = self.comp[0].next(6_300.0, dt, 0.0) * 0.006 + self.comp[1].next(9_450.0, dt, 0.0) * 0.002;
        ((body + bvi) * (0.5 + 0.5 * collective) + wash + tail + whine) * 0.8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(spec: EngineSpec, rpm: f32, throttle: f32, secs: f32) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, spec);
        e.set_input(EngineInput { rpm, throttle, load: throttle, gear: 2, speed: 20.0 });
        let mut out = vec![0.0; (48_000.0 * secs) as usize];
        for chunk in out.chunks_mut(256) {
            e.render(chunk, 1.0);
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_preset_renders_finite_bounded_audio() {
        for name in PRESET_NAMES {
            let spec = preset(name).unwrap();
            for (rpm, thr) in [(spec.idle_rpm, 0.0), (spec.redline_rpm * 0.8, 1.0)] {
                let out = render(spec, rpm, thr, 0.5);
                assert!(out.iter().all(|v| v.is_finite()), "{name}");
                let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
                assert!(peak < 1.5, "{name} peak {peak} at {rpm}");
                // An electric motor at standstill is legitimately silent.
                if rpm > 0.0 {
                    assert!(rms(&out) > 0.003, "{name} silent at {rpm}");
                }
            }
        }
    }

    #[test]
    fn the_firing_frequency_follows_rpm() {
        // An inline-4 at 3000 rpm fires 100 times a second. Autocorrelation
        // of the output must peak at a 10 ms lag.
        let out = render(preset("inline4").unwrap(), 3000.0, 0.6, 1.0);
        let x = &out[24_000..];
        let ac = |lag: usize| x.iter().zip(&x[lag..]).map(|(a, b)| a * b).sum::<f32>();
        let at = ac(480);
        let off = ac(330);
        assert!(at > off, "{at} vs {off}");
    }

    #[test]
    fn load_makes_it_louder_and_brighter() {
        let spec = preset("v8").unwrap();
        let light = rms(&render(spec, 3000.0, 0.05, 0.5)[12_000..]);
        let heavy = rms(&render(spec, 3000.0, 1.0, 0.5)[12_000..]);
        assert!(heavy > light * 1.3, "{heavy} vs {light}");
    }
}
