// makepad-piano-model — a physically modelled grand piano.
//
// No samples, no impulse responses, no dependencies: a nonlinear felt hammer
// integrated against the wave impedance of the string (with the agraffe
// reflection), driving inharmonic modal strings (f_n = n f0 sqrt(1 + B n^2))
// in detuned multi-string unisons with per-string decay splits (double decay
// + beating), frequency-dependent damping, damper/sustain/sostenuto/una-corda
// behaviour with half-pedalling, a sympathetic-resonance bank for every
// undamped string, a shared modal soundboard, and an algorithmic output stage
// (early reflections, FDN reverb, tone, soft saturation).
//
// Synthesis technique: modal synthesis with an explicitly integrated
// nonlinear exciter, rather than FDTD or a waveguide.
// - versus FDTD of the stiff-string PDE: identical partial structure by
//   construction (modes ARE the analytic solution), but unconditionally
//   stable — every mode is a contraction |C| < 1, so there is no CFL bound
//   to violate at any pitch, velocity or pedal state; FDTD stiff-string
//   schemes are implicit or conditionally stable and much more expensive.
// - versus waveguides + dispersion allpasses: waveguides need many allpass
//   sections to fit the strongly inharmonic bass partials and make precise
//   per-partial decay control awkward; modal banks give exact frequency and
//   decay per partial (which this crate's tests verify against the physical
//   law) and vectorise perfectly (4/8-wide across modes).
// The one thing given up is automatic two-way coupling (hammer<->string,
// string<->string at the bridge). The hammer solves that with its own local
// string-impedance model (hammer.rs), unison coupling is folded into
// per-string decay-rate splits (keys.rs, Weinreich normal modes), and
// sympathetic coupling is one-directional bridge drive (sympathetic.rs) —
// each an established approximation, each structurally incapable of
// instability.
//
// Real-time contract: Piano::process never allocates, locks, blocks, does IO
// or panics (debug asserts aside); all state is preallocated at new().
// Events land at their exact sample offset inside a block, and output is
// bit-identical for any block-size decomposition of the same event stream:
// all control decisions happen on an absolute 64-sample grid and at event
// boundaries, never on host-buffer boundaries.
//
// The scalar/SIMD kernel selection and its verification, and the multicore
// (offline) path, are described in modal.rs and mt.rs.

pub mod simd;
pub mod modal;
pub mod params;
mod hammer;
pub mod keys;
mod voice;
mod sympathetic;
mod soundboard;
pub mod fx;
mod mt;
pub mod learned;

use fx::{soft_clip, DcBlock, EarlyReflections, Eq, Limiter, Perspective, Reverb, ReverbParams, ReverbPreset, Tone};
use keys::{build_key, KeyDesign, FIRST_KEY, LAST_KEY, NUM_KEYS};
pub use params::{DesignParams, PianoPreset, Voicing, PIANO_PRESETS};
use modal::{detect_path, run_modes, KernelPath, MAX_CHUNK};
use soundboard::{Soundboard, BOARD_REGIONS};
use sympathetic::SymBank;
use voice::Voice;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PianoEvent {
    /// velocity 0 is treated as NoteOff (MIDI convention).
    NoteOn { key: u8, velocity: u8 },
    NoteOff { key: u8 },
    /// Continuous sustain pedal 0.0..=1.0; >= 0.75 is a full lift,
    /// in between is half-pedalling (partial damper contact).
    Sustain { value: f32 },
    Sostenuto { on: bool },
    SoftPedal { on: bool },
    AllSoundOff,
}

/// An event placed at an exact sample offset inside the current block.
/// Offsets must be < block length and non-decreasing across the slice.
#[derive(Clone, Copy, Debug)]
pub struct TimedEvent {
    pub offset: u32,
    pub event: PianoEvent,
}

/// Static per-key design facts (see Piano::key_info).
#[derive(Clone, Copy, Debug)]
pub struct KeyInfo {
    /// Stretched fundamental (Hz).
    pub f0: f32,
    /// Inharmonicity coefficient B in f_n = n f0 sqrt(1 + B n^2).
    pub b_coeff: f32,
    /// Physical unison size (1..3).
    pub n_strings: usize,
    /// Partials synthesised per string.
    pub n_partials: usize,
    /// True for the top keys that have no damper.
    pub undamped: bool,
}

/// The seam for wiring modelled instruments to a playback engine without
/// this crate knowing about scores, MIDI or UI. Other modelled instruments
/// implement the same trait later.
pub trait Instrument {
    fn sample_rate(&self) -> f32;
    /// Render one block. Never allocates, locks, blocks or panics.
    fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]);
    fn reset(&mut self);
}

// ---------------------------------------------------------------------------
// Mix constants (bridge-force domain unless noted)
// ---------------------------------------------------------------------------

/// Per-voice panned direct radiation relative to the board's modal part
/// (both direct paths run through the same plateau-normalised radiation
/// filter, so this is a plateau gain comparable to the board's `direct`).
/// The instant paths must sit within a few dB of the modal board: when the
/// resonators dominated by ~30 dB the whole instrument spoke with their
/// 15-45 ms rise — a swell, not a strike. (Value: DesignParams.direct_string;
/// the sympathetic sends are DesignParams.sym_in / sym_out.)
/// Bridge-force domain -> output domain.
/// Sized so a median-velocity classical performance (velocities ~30-60)
/// lands near -20 dBFS RMS with the default room, fortissimo material peaks
/// just into the soft saturator (which then acts as the mastering limiter
/// every commercial piano recording goes through), and a pp note stays
/// ~20 dB under a ff one.
/// 0.32, re-anchored 2026-08-31: the bridge-coupling split gave notes
/// their real prompt/aftersound structure (the Salamander C4 falls 25 dB
/// in the first second and then holds), which honestly lowered the
/// sustained RMS of median material by ~3-4 dB; the median-performance
/// operating point is a product calibration, so the master comes up to
/// keep it, and the faster note drain means flat-forte material engages
/// the limiter/knee LESS at equal master than the old sustained decay
/// did.
/// 0.37 after the per-partial normal-mode reduction: the aftersound now
/// sits at its measured level (~-16 dB re the prompt) instead of the old
/// half-drive slow members, which lowered sustained RMS ~1.3 dB further.
const MASTER_GAIN: f32 = 0.37;

/// A voice whose 64-sample bridge-force energy stays below this for ~16 ms
/// is put to sleep (and its state zeroed, keeping wake-ups deterministic).
/// The acc domain is post-trim bridge force, but radiativity is applied
/// AFTER it: keys whose partials ride the radiation bumps (the bass keys
/// on the first-resonance step) are radiated up to ~9 dB louder than the
/// same acc power elsewhere, so the gate must sit well under the old
/// -64 dBFS-ish point — at 1e-5 the first-resonance lift made a
/// pianissimo A0 audibly vanish at ~200 ms.
const VOICE_SILENCE_POWER: f32 = 1e-7;
/// Minimum ringing energy for a damper landing to make contact noise.
const DAMPER_NOISE_POWER: f32 = 0.1;
const DAMPER_NOISE_AMP: f32 = 0.25;
/// Sustain pedal value at/above which dampers are fully lifted.
const PEDAL_FULL_LIFT: f32 = 0.75;
/// Gain and colour corner of the DIRECT case-radiation path used for the
/// (1 - attack_body) share of the mechanical attack: band-limited like a
/// massive wooden case (~700 Hz, -12 dB/oct above), calibrated so the two
/// ends of the attack_body axis sit at comparable loudness.
const CASE_GAIN: f32 = 6.0;
const CASE_LP_HZ: f64 = 700.0;

/// Radiation filter for the panned direct-string path: differentiate
/// (pressure couples to velocity), flatten at the bottom of the radiativity
/// plateau (~150 Hz), and roll off above ~2.4 kHz — the same R(f) shape as
/// the modal soundboard (see soundboard::radiativity). Plateau-normalised:
/// the per-sample difference is scaled by fs / (2 pi 150), so the path's
/// plateau gain is its coefficient and is sample-rate independent.
struct RadTilt {
    dx1: f32,
    dlp: f32,
    dlp2: f32,
    c: f32,
    c2: f32,
    scale: f32,
}

impl RadTilt {
    fn new(sample_rate: f64, lp_hz: f64, vel_hz: f64) -> Self {
        Self {
            dx1: 0.0,
            dlp: 0.0,
            dlp2: 0.0,
            c: (1.0 - (-core::f64::consts::TAU * vel_hz / sample_rate).exp()) as f32,
            c2: (1.0 - (-core::f64::consts::TAU * lp_hz / sample_rate).exp()) as f32,
            scale: (sample_rate / (core::f64::consts::TAU * vel_hz)) as f32,
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let diff = x - self.dx1;
        self.dx1 = x;
        self.dlp += self.c * (diff - self.dlp);
        self.dlp2 += self.c2 * (self.dlp - self.dlp2);
        self.scale * self.dlp2
    }

    fn reset(&mut self) {
        self.dx1 = 0.0;
        self.dlp = 0.0;
        self.dlp2 = 0.0;
    }
}

/// Bridge region for a key index: the bass bridge carries the wound
/// strings (single and double unisons), then the long bridge in thirds.
#[inline(always)]
fn key_region(idx: usize) -> usize {
    if idx < 20 {
        0
    } else if idx < 44 {
        1
    } else if idx < 66 {
        2
    } else {
        3
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Everything except the per-key design tables and the voices; split out so
/// the multicore path can hand voices to workers while the main thread keeps
/// driving the rest (see mt.rs).
pub(crate) struct EngineCore {
    sample_rate: f32,
    sym: Vec<SymBank>,
    board: Soundboard,
    er: EarlyReflections,
    reverb: Reverb,
    tone: Tone,
    limiter: Limiter,
    eq: Eq,
    voicing: Voicing,
    /// sym-bank openness derived from voicing.sympathetic > 1 (dampers
    /// conceptually lifting on the resonance bed)
    openness: f32,
    dc_l: DcBlock,
    dc_r: DcBlock,
    sustain: f32,
    soft: bool,
    direct_string: f32,
    sym_in: f32,
    sym_out_gain: f32,
    sym_damped_gain: f32,
    sym_gate: f32,
    /// bus power accumulated since the last control tick (64-grid)
    bus_pow_acc: f32,
    /// damped-bank drive, SMOOTHED on the control grid (never mid-chunk,
    /// so the value cannot depend on host-buffer chunk splits). This was a
    /// boolean gate: with the bus power hovering near the threshold it
    /// flipped the whole damped-string bed on and off chunk to chunk — a
    /// per-64-sample switched mechanism, exactly the class of hard edge
    /// the listener kept flagging. The threshold still decides the TARGET;
    /// the applied gain glides.
    damped_gain: f32,
    couple_loss: f32,
    // quantised bath-loading extra radius currently applied to voices
    bath_r: f32,
    // duplex / aliquot bank (shared, driven by the bridge bus)
    dup_zr: Vec<f32>,
    dup_zi: Vec<f32>,
    dup_cr: Vec<f32>,
    dup_ci: Vec<f32>,
    dup_gin: Vec<f32>,
    dup_gout: Vec<f32>,
    dup_gain: f32,
    perspective: Perspective,
    pan_sign: f32,
    dry: f32,
    wet: f32,
    er_level: f32,
    soft_clip_on: bool,
    master: f32,
    master_user: f32,
    tone_bass_db: f32,
    tone_treble_db: f32,
    pub(crate) global_sample: u64,
    pub(crate) path: KernelPath,
    // diagnostic path scaling (1.0 in normal use; see debug_set_path_gains)
    dbg_board_modal: f32,
    dbg_direct: f32,
    // chunk scratch
    bus: [f32; MAX_CHUNK],
    sym_out: [f32; MAX_CHUNK],
    /// per-bridge-region board drive (voice bridge force + its mechanical
    /// noise land in the voice's own region; see soundboard.rs)
    board_in: [[f32; MAX_CHUNK]; BOARD_REGIONS],
    board_l: [f32; MAX_CHUNK],
    board_r: [f32; MAX_CHUNK],
    dir_l: [f32; MAX_CHUNK],
    dir_r: [f32; MAX_CHUNK],
    case_l: [f32; MAX_CHUNK],
    case_r: [f32; MAX_CHUNK],
    case_lp: [f32; 4],
    case_lp_c: f32,

    dir_tilt_l: RadTilt,
    dir_tilt_r: RadTilt,
}

pub struct Piano {
    pub(crate) keys: Vec<KeyDesign>,
    pub(crate) core: EngineCore,
    pub(crate) voices: Vec<Voice>,
}

impl Piano {
    /// Builds the full instrument (all 88 key designs, voices, sympathetic
    /// banks, soundboard, effects). Allocation happens only here.
    pub fn new(sample_rate: f32) -> Self {
        Self::new_with_params(sample_rate, &DesignParams::default())
    }

    /// Builds one of the shipped instrument presets (see
    /// params::PIANO_PRESETS): the reference design, plus the preset's
    /// voicing and room.
    pub fn new_with_preset(sample_rate: f32, preset: &PianoPreset) -> Self {
        let mut p = Self::new(sample_rate);
        p.apply_preset_live(preset);
        p
    }

    /// Applies a preset (voicing + room) to this instrument. A preset is
    /// entirely runtime state, so this never rebuilds anything.
    pub fn apply_preset_live(&mut self, preset: &PianoPreset) {
        self.set_voicing(preset.voicing);
        self.set_reverb_preset(preset.room);
        self.set_reverb_mix(preset.reverb_mix);
    }

    /// Same instrument, explicit design parameters (see params.rs). Used by
    /// verification tooling that walks the design space against reference
    /// recordings; `DesignParams::default()` IS `Piano::new`.
    pub fn new_with_params(sample_rate: f32, dp: &DesignParams) -> Self {
        assert!((8000.0..=192_000.0).contains(&sample_rate), "unsupported sample rate {sample_rate}");
        let fs = sample_rate as f64;
        let keys: Vec<KeyDesign> = (FIRST_KEY..=LAST_KEY).map(|k| build_key(k, fs, dp)).collect();
        let voices: Vec<Voice> = keys.iter().enumerate().map(|(i, k)| Voice::new(i, k)).collect();
        let sym: Vec<SymBank> = keys.iter().map(SymBank::new).collect();
        // Duplex / aliquot scale: the non-speaking string segments behind
        // the bridge, a shared bank of lightly damped resonators across the
        // duplex band, rung by the summed bridge force.
        let dup = {
            const N: usize = 32;
            let mut zr = vec![0.0f32; N];
            let zi = vec![0.0f32; N];
            let mut cr = vec![0.0f32; N];
            let mut ci = vec![0.0f32; N];
            let mut gin = vec![0.0f32; N];
            let mut gout = vec![0.0f32; N];
            let dt = 1.0 / fs;
            let lo = dp.duplex_lo.max(200.0);
            let hi = dp.duplex_hi.max(lo * 1.2);
            for m in 0..28usize {
                let jit = 0.96 + 0.08 * (((m as u32).wrapping_mul(0x9e37_79b9) >> 8) & 0xffff) as f64 / 65536.0;
                let f = lo * (hi / lo).powf(m as f64 / 27.0) * jit;
                if f >= 0.45 * fs {
                    continue;
                }
                let sigma = dp.duplex_sigma.max(1.0);
                let r = (-sigma * dt).exp();
                let th = core::f64::consts::TAU * f * dt;
                cr[m] = (r * th.cos()) as f32;
                ci[m] = (r * th.sin()) as f32;
                gin[m] = 1.0;
                let sign = if m % 2 == 0 { 1.0 } else { -1.0 };
                gout[m] = (sign * sigma * 0.006 * (48000.0 / fs)) as f32;
            }
            zr.fill(0.0);
            (zr, zi, cr, ci, gin, gout)
        };
        let core = EngineCore {
            sample_rate,
            sym,
            board: Soundboard::new(fs, dp),
            er: EarlyReflections::new(sample_rate),
            reverb: Reverb::new(sample_rate),
            tone: Tone::new(sample_rate),
            limiter: Limiter::new(sample_rate),
            eq: Eq::new(fs),
            voicing: Voicing::default(),
            openness: 0.0,
            dc_l: DcBlock::new(sample_rate),
            dc_r: DcBlock::new(sample_rate),
            sustain: 0.0,
            soft: false,
            direct_string: dp.direct_string as f32,
            sym_in: dp.sym_in as f32,
            sym_out_gain: dp.sym_out as f32,
            sym_damped_gain: dp.sym_damped as f32,
            sym_gate: dp.sym_gate as f32,
            bus_pow_acc: 0.0,
            damped_gain: 0.0,
            couple_loss: dp.couple_loss as f32,
            bath_r: 1.0,
            dup_zr: dup.0,
            dup_zi: dup.1,
            dup_cr: dup.2,
            dup_ci: dup.3,
            dup_gin: dup.4,
            dup_gout: dup.5,
            dup_gain: dp.duplex_gain as f32,
            perspective: Perspective::Player,
            pan_sign: 1.0,
            dry: 1.0,
            wet: 0.3,
            er_level: 0.7,
            soft_clip_on: true,
            master: MASTER_GAIN,
            master_user: 1.0,
            tone_bass_db: 0.0,
            tone_treble_db: 0.0,
            global_sample: 0,
            path: detect_path(),
            dbg_board_modal: 1.0,
            dbg_direct: 1.0,
            bus: [0.0; MAX_CHUNK],
            sym_out: [0.0; MAX_CHUNK],
            board_in: [[0.0; MAX_CHUNK]; BOARD_REGIONS],
            board_l: [0.0; MAX_CHUNK],
            board_r: [0.0; MAX_CHUNK],
            dir_l: [0.0; MAX_CHUNK],
            dir_r: [0.0; MAX_CHUNK],
            case_l: [0.0; MAX_CHUNK],
            case_r: [0.0; MAX_CHUNK],
            case_lp: [0.0; 4],
            case_lp_c: (1.0 - (-core::f64::consts::TAU * CASE_LP_HZ / sample_rate as f64).exp()) as f32,

            dir_tilt_l: RadTilt::new(sample_rate as f64, dp.rad_lp, dp.rad_vel_hz),
            dir_tilt_r: RadTilt::new(sample_rate as f64, dp.rad_lp, dp.rad_vel_hz),
        };
        Self { keys, core, voices }
    }

    // --- settings (control path; never called from inside process) -------

    /// Force the always-available scalar kernels (for verification).
    pub fn set_force_scalar(&mut self, scalar: bool) {
        self.core.path = if scalar { KernelPath::Scalar } else { detect_path() };
    }

    pub fn kernel_path(&self) -> KernelPath {
        self.core.path
    }

    // ------------------------------------------------------------------
    // Room / output controls. This is the whole surface a settings UI
    // binds; every setter has a matching getter, every value is clamped
    // to its documented range, and all of it is safe to call between
    // process() calls (control path: no allocation, no locks).
    //
    //   control                    range        default
    //   set_reverb_preset          ALL[5]       SmallHall
    //   set_reverb_params          see fields   SmallHall.params()
    //   set_reverb_mix             0.0..=1.5    0.3
    //   set_early_reflection_level 0.0..=1.5    0.7
    //   set_perspective            Player/Audience   Player
    //   set_tone (bass, treble dB) -12.0..=12.0 0.0, 0.0
    //   set_master_gain            0.0..=10.0   1.0
    //   set_soft_clip              bool         true
    //
    // The dry instrument is always at unity: reverb_mix and the early
    // reflections are send levels ON TOP of the direct sound, so turning
    // the room up never pulls the piano itself down, and mix 0.0 is
    // exactly the dry instrument. 1.0 is an equal-level wet return
    // (drenched); useful musical values sit around 0.15-0.5.
    // ------------------------------------------------------------------

    /// Applies one of the ready-made rooms (see `ReverbPreset::ALL`).
    /// Equivalent to `set_reverb_params(preset.params())`; the preset's
    /// parameters can be read back with `reverb_params`.
    pub fn set_reverb_preset(&mut self, preset: ReverbPreset) {
        self.core.reverb.set_preset(preset);
    }

    /// Sets the four continuous room parameters (clamped; see the
    /// `ReverbParams` field docs for ranges).
    pub fn set_reverb_params(&mut self, params: ReverbParams) {
        self.core.reverb.set_params(params);
    }

    /// The effective (post-clamp) reverb parameters currently in use.
    pub fn reverb_params(&self) -> ReverbParams {
        self.core.reverb.params()
    }

    /// Reverb tail send level: 0.0 = dry (reverb defeated), ~0.3 = a
    /// natural room, 1.0+ = drenched. Clamped to 0.0..=1.5.
    pub fn set_reverb_mix(&mut self, wet: f32) {
        self.core.wet = if wet.is_finite() { wet.clamp(0.0, 1.5) } else { 0.0 };
    }

    pub fn reverb_mix(&self) -> f32 {
        self.core.wet
    }

    /// Early-reflection send level (the close lid/wall slapback that gives
    /// the room its size cue): 0.0 defeats it. Clamped to 0.0..=1.5.
    pub fn set_early_reflection_level(&mut self, level: f32) {
        self.core.er_level = if level.is_finite() { level.clamp(0.0, 1.5) } else { 0.0 };
    }

    pub fn early_reflection_level(&self) -> f32 {
        self.core.er_level
    }

    /// Listening position: at the keys (bass left, tight reflections) or in
    /// the hall (image mirrored, later reflections). Changes the stereo
    /// image and reflection pattern only — the level controls are yours and
    /// are left untouched.
    pub fn set_perspective(&mut self, p: Perspective) {
        self.core.perspective = p;
        self.core.pan_sign = match p {
            Perspective::Player => 1.0,
            Perspective::Audience => -1.0,
        };
        let sr = self.core.sample_rate;
        self.core.er.set_perspective(p, sr);
    }

    pub fn perspective(&self) -> Perspective {
        self.core.perspective
    }

    // ------------------------------------------------------------------
    // Voicing: the runtime mechanism mix (see params::Voicing). Six
    // continuous amounts, 0.0 = mechanism off, 1.0 = the reference-matched
    // level, up to 2.5 for deliberate exaggeration; presets are named
    // points in the same space. Safe between process() calls: plain
    // scalars, consumed at note-on / per chunk, no allocation. New strikes
    // pick up slider moves; ringing notes keep the voicing they were
    // struck with (except the sympathetic field, which is live).
    // What is NOT voicing: the strike-vs-pluck modal quadrature (fixed
    // physics, see modal.rs) and the construction-time string/radiation
    // design (Piano::new_with_params).
    // ------------------------------------------------------------------

    pub fn set_voicing(&mut self, v: Voicing) {
        self.core.voicing = v.clamped();
    }

    pub fn voicing(&self) -> Voicing {
        self.core.voicing
    }

    // ------------------------------------------------------------------
    // Output EQ (after the instrument, before the room sends): a treble
    // shelf with settable corner and one parametric presence bell. Flat
    // (and bypassed) by default; the physical voicing stays the primary
    // character and this is the engineer's trim on top.
    //   set_eq_shelf(gain_db -24..=12, corner_hz 1k..=16k)
    //   set_eq_bell(freq_hz 200..=12k, gain_db -24..=12, q 0.3..=8)
    // ------------------------------------------------------------------

    pub fn set_eq_shelf(&mut self, gain_db: f32, corner_hz: f32) {
        self.core.eq.set_shelf(gain_db, corner_hz);
    }

    pub fn eq_shelf(&self) -> (f32, f32) {
        self.core.eq.shelf()
    }

    pub fn set_eq_bell(&mut self, freq_hz: f32, gain_db: f32, q: f32) {
        self.core.eq.set_bell(freq_hz, gain_db, q);
    }

    pub fn eq_bell(&self) -> (f32, f32, f32) {
        self.core.eq.bell()
    }

    /// Gentle output shelves, +/-12 dB at 120 Hz / 6 kHz (clamped).
    pub fn set_tone(&mut self, bass_db: f32, treble_db: f32) {
        self.core.tone_bass_db = bass_db.clamp(-12.0, 12.0);
        self.core.tone_treble_db = treble_db.clamp(-12.0, 12.0);
        self.core.tone.set(self.core.tone_bass_db, self.core.tone_treble_db);
    }

    /// (bass_db, treble_db) as currently applied.
    pub fn tone(&self) -> (f32, f32) {
        (self.core.tone_bass_db, self.core.tone_treble_db)
    }

    /// Soft output saturation instead of digital clipping (default on).
    pub fn set_soft_clip(&mut self, on: bool) {
        self.core.soft_clip_on = on;
    }

    pub fn soft_clip(&self) -> bool {
        self.core.soft_clip_on
    }

    /// Output gain as a plain factor on the calibrated level: 1.0 is the
    /// default (a median classical performance near -20 dBFS RMS), clamped
    /// to 0.0..=10.0.
    pub fn set_master_gain(&mut self, gain: f32) {
        let g = if gain.is_finite() { gain.clamp(0.0, 10.0) } else { 1.0 };
        self.core.master_user = g;
        self.core.master = g * MASTER_GAIN;
    }

    pub fn master_gain(&self) -> f32 {
        self.core.master_user
    }

    // --- render ----------------------------------------------------------

    /// See trait docs; the real-time entry point.
    pub fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        debug_assert!(events.windows(2).all(|w| w[0].offset <= w[1].offset), "events must be sorted by offset");
        debug_assert!(events.iter().all(|e| (e.offset as usize) < len.max(1)), "event offsets must lie inside the block");
        let core = &mut self.core;
        let keys = &self.keys[..];
        let voices = &mut self.voices[..];
        let mut pos = 0usize;
        let mut ev = 0usize;
        while pos < len {
            if core.global_sample % MAX_CHUNK as u64 == 0 {
                core.control_tick(keys, voices);
            }
            while ev < events.len() && (events[ev].offset as usize) <= pos {
                core.apply_event(keys, voices, &events[ev].event);
                ev += 1;
            }
            let next_ev = events.get(ev).map(|e| (e.offset as usize).min(len)).unwrap_or(len);
            let room = MAX_CHUNK - (core.global_sample % MAX_CHUNK as u64) as usize;
            let n = (len - pos).min(next_ev - pos).min(room);
            for v in voices.iter_mut() {
                if v.active {
                    v.render(&keys[v.key_idx], core.path, n);
                }
            }
            core.finish_chunk(keys, voices, n, &mut out_l[pos..pos + n], &mut out_r[pos..pos + n]);
            pos += n;
            core.global_sample += n as u64;
        }
    }

    /// Static design facts for one key, for verification and tooling.
    pub fn key_info(&self, key: u8) -> Option<KeyInfo> {
        if !(FIRST_KEY..=LAST_KEY).contains(&key) {
            return None;
        }
        let k = &self.keys[(key - FIRST_KEY) as usize];
        Some(KeyInfo {
            f0: k.f0,
            b_coeff: k.b_coeff,
            n_strings: k.n_strings,
            n_partials: k.modes_per_osc,
            undamped: k.undamped,
        })
    }

    /// Scales the radiation paths for diagnostics/verification only:
    /// `board_modal` scales the modal soundboard response, `direct` scales
    /// both instant (non-modal) radiation paths. (1.0, 1.0) is the shipped
    /// instrument. Used by tests to assert the soundboard's share of the
    /// output; never call this from an app.
    #[doc(hidden)]
    pub fn debug_set_path_gains(&mut self, board_modal: f32, direct: f32) {
        self.core.dbg_board_modal = board_modal;
        self.core.dbg_direct = direct;
        self.core.board.dbg_direct = direct;
    }

    /// Renders the isolated hammer force pulse for one key/velocity
    /// (diagnostics/verification: the audio path is untouched).
    #[doc(hidden)]
    pub fn debug_hammer_pulse(&self, key: u8, velocity: u8) -> Vec<f32> {
        let mut out = vec![0.0f32; (0.08 * self.core.sample_rate) as usize];
        if !(FIRST_KEY..=LAST_KEY).contains(&key) {
            return out;
        }
        let k = &self.keys[(key - FIRST_KEY) as usize];
        let mut h = crate::hammer::Hammer::new();
        let speed = keys::velocity_to_speed(velocity);
        let speed = k.speed_pivot * (speed / k.speed_pivot).powf(k.speed_q);
        h.strike(
            speed,
            k.hammer_mass,
            k.felt_k,
            k.felt_p,
            k.felt_u_lock,
            k.felt_lock_w,
            k.felt_lambda,
            k.z_total,
            k.t1_seconds,
            self.core.sample_rate as f64,
            1.0,
            (k.rough_depth as f64 * (speed / 6.0)).min(0.5) as f32,
            (key as u32).wrapping_mul(0x51ed_270b) ^ 0x5bd1,
            k.img_fc_mul,
            k.img_g_base,
            k.img_g_slope,
            k.core_k,
            k.u_core,
        );
        let mut pos = 0;
        while pos + MAX_CHUNK <= out.len() {
            if !h.render_force(&mut out[pos..pos + MAX_CHUNK], MAX_CHUNK) {
                break;
            }
            pos += MAX_CHUNK;
        }
        out
    }

    /// Renders one voice in isolation and reports (peak, rms) of its bridge
    /// force signal (diagnostics: calibrates the phantom-partial drive
    /// normalisation; the audio path is untouched).
    #[doc(hidden)]
    pub fn debug_bridge_stats(&mut self, key: u8, velocity: u8, secs: f64) -> (f32, f64) {
        if !(FIRST_KEY..=LAST_KEY).contains(&key) {
            return (0.0, 0.0);
        }
        self.reset();
        let n = (secs * self.core.sample_rate as f64) as usize;
        self.core.apply_event(&self.keys, &mut self.voices, &PianoEvent::NoteOn { key, velocity });
        let i = (key - FIRST_KEY) as usize;
        let mut peak = 0.0f32;
        let mut e = 0.0f64;
        let mut pos = 0usize;
        while pos < n {
            let m = MAX_CHUNK.min(n - pos);
            let v = &mut self.voices[i];
            if v.active {
                v.render(&self.keys[i], self.core.path, m);
            }
            for k in 0..m {
                let a = self.voices[i].acc[k].abs();
                if a > peak {
                    peak = a;
                }
                e += (a as f64) * (a as f64);
            }
            pos += m;
        }
        self.reset();
        (peak, (e / n.max(1) as f64).sqrt())
    }

    /// EXPERIMENTAL — learned-hybrid prototyping (see learned.rs and
    /// tests/learned_targets.rs): reshapes one key's string partials.
    /// `gain[m]` multiplies partial m+1's output weight on every unison
    /// string; `sigma_scale[m]` raises that partial's pole radius to the
    /// given power (r -> r^s, so s > 1 decays faster and s < 1 sustains
    /// longer; values are clamped to 0.25..=4.0). Control path only —
    /// call between process() calls, never from inside; allocates nothing.
    /// The shipped instrument never calls this: it exists so offline
    /// experiments can impose learned per-partial targets on the physical
    /// excitation/coupling and be listened to.
    #[doc(hidden)]
    pub fn debug_shape_partials(&mut self, key: u8, gain: &[f32], sigma_scale: &[f32]) {
        if !(FIRST_KEY..=LAST_KEY).contains(&key) {
            return;
        }
        let i = (key - FIRST_KEY) as usize;
        let k = &mut self.keys[i];
        let mp = k.modes_padded;
        for osc in 0..k.n_osc {
            for m in 0..k.modes_per_osc {
                let idx = osc * mp + m;
                if let Some(&g) = gain.get(m) {
                    if g.is_finite() && g >= 0.0 {
                        k.gout[idx] *= g;
                        k.gout_re[idx] *= g;
                    }
                }
                if let Some(&s) = sigma_scale.get(m) {
                    if s.is_finite() {
                        let s = s.clamp(0.25, 4.0) as f64;
                        let (cr, ci) = (k.cr_sus[idx] as f64, k.ci_sus[idx] as f64);
                        let r = (cr * cr + ci * ci).sqrt();
                        if r > 1e-12 && r < 1.0 {
                            let scale = r.powf(s - 1.0);
                            k.cr_sus[idx] = (cr * scale) as f32;
                            k.ci_sus[idx] = (ci * scale) as f32;
                        }
                    }
                }
            }
        }
        let v = &mut self.voices[i];
        let eng = v.eng;
        v.rebuild(k, eng);
    }

    /// Read-only view of the per-key design tables (diagnostics/verification:
    /// lets offline tooling compare the designed per-partial decay structure
    /// with two-exponential fits of reference recordings).
    #[doc(hidden)]
    pub fn keys_debug(&self) -> &[keys::KeyDesign] {
        &self.keys
    }

    /// Full state reset (voices, pedals, resonance, effects, clock).
    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.silence();
            v.held = false;
            v.sost_held = false;
            v.strike_count = 0;
            v.eng = 1.0;
            v.extra_r = 1.0;
        }
        for (s, k) in self.core.sym.iter_mut().zip(self.keys.iter()) {
            s.clear();
            s.rebuild(k, 1.0);
            s.off_ticks = 0;
        }
        for (v, k) in self.voices.iter_mut().zip(self.keys.iter()) {
            v.rebuild(k, 1.0);
        }
        self.core.board.reset();
        self.core.dup_zr.fill(0.0);
        self.core.dup_zi.fill(0.0);
        self.core.bath_r = 1.0;
        self.core.bus_pow_acc = 0.0;
        self.core.damped_gain = 0.0;
        self.core.er.reset();
        self.core.reverb.reset();
        self.core.tone.reset();
        self.core.eq.reset();
        self.core.dc_l.reset();
        self.core.dc_r.reset();
        self.core.dir_tilt_l.reset();
        self.core.dir_tilt_r.reset();
        self.core.case_lp = [0.0; 4];
        self.core.sustain = 0.0;
        self.core.soft = false;
        self.core.global_sample = 0;
    }
}

impl Instrument for Piano {
    fn sample_rate(&self) -> f32 {
        self.core.sample_rate
    }

    fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        Piano::process(self, events, out_l, out_r)
    }

    fn reset(&mut self) {
        Piano::reset(self)
    }
}

impl EngineCore {
    pub(crate) fn apply_event(&mut self, keys: &[KeyDesign], voices: &mut [Voice], ev: &PianoEvent) {
        match *ev {
            PianoEvent::NoteOn { key, velocity } => {
                if velocity == 0 {
                    return self.apply_event(keys, voices, &PianoEvent::NoteOff { key });
                }
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                let i = (key - FIRST_KEY) as usize;
                let vc = self.voicing;
                voices[i].note_on(&keys[i], velocity, self.soft, self.sample_rate as f64, &vc);
                // The damper leaves the string early in the key travel,
                // before the hammer arrives.
                voices[i].rebuild(&keys[i], 0.0);
            }
            PianoEvent::NoteOff { key } => {
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                voices[(key - FIRST_KEY) as usize].held = false;
            }
            PianoEvent::Sustain { value } => {
                self.sustain = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
            }
            PianoEvent::Sostenuto { on } => {
                for v in voices.iter_mut() {
                    v.sost_held = on && v.held;
                }
            }
            PianoEvent::SoftPedal { on } => {
                self.soft = on;
            }
            PianoEvent::AllSoundOff => {
                for v in voices.iter_mut() {
                    v.silence();
                    v.held = false;
                    v.sost_held = false;
                }
                for s in &mut self.sym {
                    s.clear();
                }
                self.dup_zr.fill(0.0);
                self.dup_zi.fill(0.0);
                self.board.reset();
                self.er.reset();
                self.reverb.reset();
            }
        }
    }

    /// Runs on the absolute 64-sample grid, never on host-buffer boundaries:
    /// damper motion, sympathetic bank activity, voice sleep decisions.
    pub(crate) fn control_tick(&mut self, keys: &[KeyDesign], voices: &mut [Voice]) {
        // Dampers lift over the top of the pedal travel and the felt only
        // grips near full engagement: a strongly nonlinear curve is what
        // makes half-pedalling usable.
        let damped_target = if self.sym_damped_gain > 0.0 && self.bus_pow_acc > self.sym_gate { 1.0 } else { 0.0 };
        self.damped_gain += 0.35 * (damped_target - self.damped_gain);
        if self.damped_gain < 1e-3 {
            self.damped_gain = 0.0;
        }
        self.bus_pow_acc = 0.0;
        let lift_target = ((PEDAL_FULL_LIFT - self.sustain).max(0.0) / PEDAL_FULL_LIFT).min(1.0).powf(2.5);
        // Bath-loading: how much open string is there for a sounding string
        // to bleed into through the bridge (quantised so voices only
        // rebuild on material change; the factor can only add damping).
        let mut bath_r = 1.0f32;
        let sym_amt = self.voicing.sympathetic;
        self.openness = ((sym_amt - 1.0).max(0.0) / 1.5).min(1.0);
        if self.couple_loss > 0.0 && sym_amt > 0.0 {
            let mut open_w = 0.0f32;
            for v in voices.iter() {
                open_w += 1.0 - v.eng;
            }
            let w = (open_w / NUM_KEYS as f32).clamp(0.0, 1.0);
            let sx_q = (self.couple_loss * sym_amt.min(1.5) * w / 0.05).round() * 0.05;
            bath_r = (-sx_q / self.sample_rate).exp();
        }
        self.bath_r = bath_r;
        for i in 0..NUM_KEYS {
            let key = &keys[i];
            let v = &mut voices[i];
            let s = &mut self.sym[i];
            let target = if v.held || v.sost_held || key.undamped { 0.0 } else { lift_target };
            let old = v.eng;
            let mut eng = old + 0.35 * (target - old);
            if (eng - target).abs() < 1e-3 {
                eng = target;
            }
            if v.active && v.extra_r != bath_r {
                v.rebuild_with(key, eng, bath_r);
            }
            if eng != old {
                if v.active {
                    v.rebuild(key, eng);
                    if old < 0.35 && eng >= 0.35 && v.power > DAMPER_NOISE_POWER {
                        let seed =
                            (i as u32).wrapping_mul(0xc2b2_ae35) ^ v.strike_count.wrapping_mul(0x27d4_eb2f) ^ 0x9e37;
                        let amp = ((v.power / 64.0).sqrt() * DAMPER_NOISE_AMP).min(2.0);
                        let lp_c = 1.0 - (-core::f32::consts::TAU * 2500.0 / self.sample_rate).exp();
                        v.damper_noise.start((0.012 * self.sample_rate) as u32, amp, lp_c, seed, 0);
                    }
                } else {
                    v.eng = eng;
                }
            }
            // Sympathetic bank runs whenever this key's damper is off the
            // string; when the damper lands it rings out briefly, then is
            // cleared (deterministically, on this grid). The voicing's
            // sympathetic amount above 1.0 lifts the resonance bed's
            // dampers ("all dampers off" at the top of the slider) —
            // rebuilds are in-place radius updates, no allocation.
            let s_eng = eng * (1.0 - self.openness);
            if s_eng < 0.98 {
                s.active = true;
                s.off_ticks = 0;
                if s.eng != s_eng {
                    s.rebuild(key, s_eng);
                }
            } else if s.active {
                if s.eng != s_eng {
                    s.rebuild(key, s_eng);
                }
                s.off_ticks += 1;
                if s.off_ticks > 240 {
                    s.clear();
                }
            }
            if v.active && !v.hammer.active {
                if v.power < VOICE_SILENCE_POWER {
                    v.quiet_ticks += 1;
                    if v.quiet_ticks > 12 {
                        v.silence();
                    }
                } else {
                    v.quiet_ticks = 0;
                }
            }
            v.power = 0.0;
        }
    }

    /// Merge rendered voices (in fixed key order — this is what makes the
    /// multicore path bit-identical), drive sympathetic banks and the
    /// soundboard, then the effect chain, and write the output.
    pub(crate) fn finish_chunk(
        &mut self,
        keys: &[KeyDesign],
        voices: &mut [Voice],
        n: usize,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        for k in 0..n {
            self.bus[k] = 0.0;
            self.sym_out[k] = 0.0;
            self.dir_l[k] = 0.0;
            self.dir_r[k] = 0.0;
            self.case_l[k] = 0.0;
            self.case_r[k] = 0.0;
            self.board_l[k] = 0.0;
            self.board_r[k] = 0.0;
        }
        for reg in 0..BOARD_REGIONS {
            for k in 0..n {
                self.board_in[reg][k] = 0.0;
            }
        }
        // attack_body: the share of the mechanical attack that couples
        // through the board (the woody attack the listener chose) vs the
        // tight case-coloured direct path; see params::Voicing.
        let ab = self.voicing.attack_body;
        let ab_dir = (1.0 - ab) * CASE_GAIN;
        for v in voices.iter_mut() {
            if !v.active {
                continue;
            }
            let pan = keys[v.key_idx].pan * self.pan_sign;
            let ang = (pan + 1.0) * core::f32::consts::FRAC_PI_4;
            let (pl, pr) = (ang.cos(), ang.sin());
            // which stretch of bridge this key's strings terminate on:
            // the bass bridge, then thirds of the long bridge (mirrored
            // with the pan sign so Audience view flips the board too)
            let reg = {
                let r = key_region(v.key_idx);
                if self.pan_sign >= 0.0 { r } else { BOARD_REGIONS - 1 - r }
            };
            let mut p = v.power;
            for k in 0..n {
                let a = v.acc[k];
                self.bus[k] += a;
                self.dir_l[k] += pl * a;
                self.dir_r[k] += pr * a;
                // The attack complex (thump, click, damper felt) couples
                // INTO the board with the bridge force. A round of this
                // work rerouted it to a direct, unresonant path because the
                // coupled version measured a +19 dB low-mid tail at 300 ms
                // — and the listener rejected the result twice ("weird",
                // "way too loud") while calling the coupled build "the
                // beginning of an actual piano". The tail IS the
                // instrument's body answering the blow; the character is
                // the coupling, and the proportion is set by the noise
                // generators' levels (already well below the original
                // build's at forte). The ear outranks the energy metric
                // here, same as the quadrature lesson.
                self.board_in[reg][k] += a + v.noise_buf[k] + ab * v.case_buf[k];
                self.case_l[k] += ab_dir * pl * v.case_buf[k];
                self.case_r[k] += ab_dir * pr * v.case_buf[k];
                p += a * a;
            }
            v.power = p;
        }
        let mut bus_pow = 0.0f32;
        for k in 0..n {
            bus_pow += self.bus[k] * self.bus[k];
        }
        self.bus_pow_acc += bus_pow;
        let damped_drive = if self.voicing.sympathetic > 0.0 { self.damped_gain } else { 0.0 };
        let sym_send = self.sym_out_gain * self.voicing.sympathetic;
        for i in 0..NUM_KEYS {
            let s = &mut self.sym[i];
            if s.active {
                s.render(&keys[i], self.path, &self.bus[..n], self.sym_in, &mut self.sym_out[..n]);
            } else if damped_drive > 0.0 {
                // felt-damped strings still couple: heavily damped rotations,
                // small drive, faded in and out on the control grid
                s.render(
                    &keys[i],
                    self.path,
                    &self.bus[..n],
                    self.sym_in * self.sym_damped_gain * damped_drive,
                    &mut self.sym_out[..n],
                );
            }
        }
        if self.dup_gain > 0.0 && self.voicing.sympathetic > 0.0 {
            run_modes(
                self.path,
                &mut self.dup_zr,
                &mut self.dup_zi,
                &self.dup_cr,
                &self.dup_ci,
                &self.dup_gin,
                &self.dup_gout,
                &self.bus[..n],
                self.dup_gain * self.voicing.sympathetic.min(1.6),
                &mut self.sym_out[..n],
            );
        }
        // the sympathetic bed and duplex ring couple through the whole
        // bridge: spread them evenly over the regions
        let sym_spread = sym_send * (1.0 / BOARD_REGIONS as f32);
        for reg in 0..BOARD_REGIONS {
            for k in 0..n {
                self.board_in[reg][k] += sym_spread * self.sym_out[k];
            }
        }
        self.board.render(self.path, &self.board_in, n, &mut self.board_l[..n], &mut self.board_r[..n]);
        for k in 0..n {
            let dsl = self.dir_tilt_l.process(self.dir_l[k]);
            let dsr = self.dir_tilt_r.process(self.dir_r[k]);
            // case colour for the direct share (see CASE_LP_HZ)
            let cc = self.case_lp_c;
            self.case_lp[0] += cc * (self.case_l[k] - self.case_lp[0]);
            self.case_lp[1] += cc * (self.case_lp[0] - self.case_lp[1]);
            self.case_lp[2] += cc * (self.case_r[k] - self.case_lp[2]);
            self.case_lp[3] += cc * (self.case_lp[2] - self.case_lp[3]);
            let pl = self.master
                * (self.dbg_board_modal * self.board_l[k]
                    + self.dbg_direct * (self.direct_string * dsl + self.case_lp[1]));
            let pr = self.master
                * (self.dbg_board_modal * self.board_r[k]
                    + self.dbg_direct * (self.direct_string * dsr + self.case_lp[3]));
            // channel EQ ahead of the room: the reflections and tail hear
            // the EQ'd source, the way a desk insert feeds the sends
            let (pl, pr) = self.eq.process(pl, pr);
            let (el, er) = self.er.process(pl, pr);
            let (wl, wr) = self.reverb.process(pl, pr);
            let l = self.dry * pl + self.er_level * el + self.wet * wl;
            let r = self.dry * pr + self.er_level * er + self.wet * wr;
            let (tl, tr) = self.tone.process(l, r);
            let mut l = self.dc_l.process(tl);
            let mut r = self.dc_r.process(tr);
            // Ride the gain first; the knee is only the last line.
            if self.soft_clip_on {
                let (limited_l, limited_r) = self.limiter.process(l, r);
                l = soft_clip(limited_l);
                r = soft_clip(limited_r);
            }
            out_l[k] = l;
            out_r[k] = r;
        }
    }
}
