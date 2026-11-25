//! The game mixer: fixed voice pools for recipes, vehicles, sustained tones
//! and instruments, each voice placed in 3D by its own [`Spatial`], a shared
//! room reverb, and a master compressor + look-ahead limiter.
//!
//! Threading: a host either calls the methods on one thread, or keeps the bus
//! on its audio thread and sends [`Command`]s (applied with [`SynthBus::apply`]),
//! receiving everything the bus lets go of as [`Retired`] and its state as a
//! [`BusView`]. Apply, render and reap neither allocate nor free.
//!
//! Voice budget: when a pool is full the quietest voice (placement gain ×
//! its own level) yields — faded over ~3 ms, never hard-cut — and only to a
//! newcomer at least as important. The render measures its own CPU cost and,
//! under sustained overload, sheds the quietest recipe voices first.

use crate::dsp::{finite, Osc, Smooth, Wave};
use crate::dynamics::{Compressor, Ducker, Limiter};
use crate::engine::{EngineInput, EngineSpec};
use crate::instrument::{Control, Instrument};
use crate::recipe::{Recipe, RecipeVoice};
use crate::reverb::{Reverb, RoomParams, RoomPreset};
use crate::spatial::{place, Distance, Listener3, Placement3, Spatial, V3};
use crate::vehicle::{LoopSet, TyreInput, VehicleSound};

pub const RECIPE_VOICES: usize = 48;
pub const VEHICLE_VOICES: usize = 12;
pub const TONE_VOICES: usize = 8;
pub const INSTRUMENT_SLOTS: usize = 8;
const BLOCK: usize = 256;
/// Control frames a vehicle may go undriven before it fades out.
pub const STALE_FRAMES: u64 = 8;
/// Stolen voices fade over this many seconds.
const STEAL_FADE: f32 = 0.003;
/// A stopped tone's level falls this much per second (~55 ms).
const TONE_RELEASE_RATE: f32 = 18.0;

/// A player-facing volume group. Recipes, vehicles and tones are effects;
/// each instrument is in the group it was put in
/// ([`SynthBus::set_instrument_group`]; effects until then). The external
/// (sampled) input is already balanced by its host and passes as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Fx,
    Music,
}

/// Seconds a group volume change glides over (a mute is a quick fade).
const GROUP_GLIDE: f32 = 0.08;

/// A finite linear transition in audio time. Commands may retarget it mid-ramp.
#[derive(Clone, Copy)]
struct Glide {
    value: f32,
    target: f32,
    left: f64,
}

impl Glide {
    const fn new(value: f32) -> Self { Self { value, target: value, left: 0.0 } }

    fn set(&mut self, target: f32, seconds: f64) {
        self.target = target;
        self.left = if target == self.value { 0.0 } else { seconds };
        if self.left == 0.0 { self.value = target; }
    }

    fn advance(&mut self, frames: usize, rate: f32) -> f32 {
        if self.left > 0.0 {
            let remaining = (self.left * rate as f64).round().max(1.0);
            if frames as f64 >= remaining {
                self.value = self.target;
                self.left = 0.0;
            } else {
                self.value += (self.target - self.value) * (frames as f64 / remaining) as f32;
                self.left -= frames as f64 / rate as f64;
            }
        }
        self.value
    }
}

fn seconds(s: f32) -> f64 { finite(s, 0.0).max(0.0) as f64 }

/// Script volume and a retriggerable duck, independent of the player's volume.
#[derive(Clone, Copy)]
pub struct GroupMix {
    script: Glide,
    duck: Glide,
    floor: f32,
    hold: f64,
    release: f64,
}

impl Default for GroupMix {
    fn default() -> Self {
        Self { script: Glide::new(1.0), duck: Glide::new(1.0), floor: 1.0, hold: 0.0, release: 0.0 }
    }
}

impl GroupMix {
    pub fn set_gain(&mut self, gain: f32, ease: f32) {
        self.script.set(finite(gain, 1.0).clamp(0.0, 4.0), seconds(ease));
    }

    pub fn duck(&mut self, to: f32, attack: f32, hold: f32, release: f32) {
        let to = finite(to, 1.0).clamp(0.0, 1.0);
        let (attack, hold, release) = (seconds(attack), seconds(hold), seconds(release));
        let old_end = self.duck.left + self.hold + self.release;
        if old_end == 0.0 && (to == 1.0 || attack + hold + release == 0.0) { return; }
        let old_hold_end = if self.duck.target < 1.0 { self.duck.left + self.hold } else { 0.0 };
        let hold_end = old_hold_end.max(attack + hold);
        let end = old_end.max(attack + hold + release);
        let attack = if old_hold_end > 0.0 && to > self.floor {
            self.duck.left
        } else if self.duck.target <= to {
            self.duck.left.min(attack)
        } else { attack };
        self.floor = if old_end > 0.0 { self.floor.min(to) } else { to };
        self.duck.set(self.floor, attack);
        self.hold = (hold_end - self.duck.left).max(0.0);
        self.release = end - hold_end;
        self.begin_release();
    }

    fn begin_release(&mut self) {
        if self.duck.left == 0.0 && self.hold == 0.0 && self.duck.target != 1.0 {
            self.duck.set(1.0, self.release);
            self.release = 0.0;
        }
    }

    /// Write one gain per frame for all sources in this group. Unity leaves
    /// the scratch untouched. Split at phase boundaries even for a one-buffer duck.
    pub fn levels(&mut self, out: &mut [f32], rate: f32) -> bool {
        if self.script.value == 1.0 && self.script.left == 0.0 && self.duck.value == 1.0
            && self.duck.left == 0.0 && self.hold == 0.0 && self.release == 0.0 {
            return false;
        }
        let mut offset = 0;
        while offset < out.len() {
            let holding = self.duck.left == 0.0 && self.hold > 0.0;
            let mut n = out.len() - offset;
            for time in [self.script.left, self.duck.left, if holding { self.hold } else { 0.0 }] {
                if time > 0.0 { n = n.min((time * rate as f64).round().max(1.0) as usize); }
            }
            let script = self.script.value;
            let duck = self.duck.value;
            let ds = (self.script.advance(n, rate) - script) / n as f32;
            let dd = (self.duck.advance(n, rate) - duck) / n as f32;
            let span = &mut out[offset..offset+n];
            if ds == 0.0 && dd == 0.0 {
                span.fill(script * duck);
            } else {
                for (i, g) in span.iter_mut().enumerate() {
                    let t = (i + 1) as f32;
                    *g = (script + t * ds) * (duck + t * dd);
                }
            }
            if holding {
                let remaining = (self.hold * rate as f64).round();
                self.hold = if n as f64 >= remaining { 0.0 } else { self.hold - n as f64 / rate as f64 };
            }
            self.begin_release();
            offset += n;
        }
        true
    }
}

/// Glides the feedback coefficient of two cascaded low-pass sections.
/// A zero feedback coefficient is the exact identity at the end of a glide.
#[derive(Clone, Copy)]
pub struct Muffle {
    amount: f32,
    rate: f32,
    pole: Glide,
}

impl Default for Muffle {
    fn default() -> Self { Self { amount: 0.0, rate: 0.0, pole: Glide::new(0.0) } }
}

impl Muffle {
    pub fn bypassed(&self) -> bool {
        self.amount == 0.0 && self.pole.value == 0.0
    }

    pub fn set(&mut self, amount: f32, ease: f32, rate: f32) {
        self.amount = finite(amount, 0.0).clamp(0.0, 1.0);
        self.retune(rate, seconds(ease));
    }

    fn retune(&mut self, rate: f32, ease: f64) {
        self.rate = rate;
        let pole = if self.amount == 0.0 { 0.0 } else {
            let cutoff = 20_000.0 * 0.03_f32.powf(self.amount);
            (-std::f32::consts::TAU * cutoff / rate).exp()
        };
        self.pole.set(pole, ease);
    }

    /// The active prefix of the block. Frames after it bypass the filter.
    pub fn poles(&mut self, out: &mut [f32], rate: f32) -> usize {
        if self.bypassed() { return 0; }
        if self.rate != rate { self.retune(rate, self.pole.left); }
        let n = out.len().min((self.pole.left * rate as f64).round() as usize);
        if n > 0 {
            let start = self.pole.value;
            let end = self.pole.advance(n, rate);
            let step = (end - start) / n as f32;
            for (i, p) in out[..n].iter_mut().enumerate() { *p = start + step * (i + 1) as f32; }
            out[n-1] = end;
        } else if self.pole.left > 0.0 {
            self.pole.set(self.pole.target, 0.0);
        }
        if self.pole.value == 0.0 { return n.saturating_sub(1); }
        out[n..].fill(self.pole.value);
        out.len()
    }
}

#[derive(Default, Clone, Copy)]
pub struct LowPassPair {
    first: f32,
    second: f32,
}

impl LowPassPair {
    #[inline]
    pub fn filter(&mut self, x: f32, pole: f32) -> f32 {
        self.first = x + pole * (self.first - x);
        self.second = self.first + pole * (self.second - self.first);
        self.second
    }
}

/// Where a voice is and how it carries. `positional: false` is a 2D sound
/// (UI, the player's own gun): centred, no distance, no doppler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub positional: bool,
    pub pos: V3,
    pub vel: V3,
    pub dist: Distance,
    /// 0 = clear line of sight .. 1 = fully behind a wall (a low-pass hook).
    pub occlusion: f32,
    /// Doppler strength (0 = off, 1 = physical).
    pub doppler: f32,
    /// Stereo position of a 2D (non-positional) emitter, -1..1.
    pub pan: f32,
}

impl Emitter {
    pub const FLAT: Emitter = Emitter {
        positional: false,
        pos: [0.0; 3],
        vel: [0.0; 3],
        dist: Distance { near: 1.0, range: 40.0, rolloff: 1.0 },
        occlusion: 0.0,
        doppler: 0.0,
        pan: 0.0,
    };

    pub fn at(pos: V3, range: f32) -> Emitter {
        Emitter {
            positional: true,
            pos,
            dist: Distance { near: 1.5, range: range.max(1.0), rolloff: 0.8 },
            doppler: 1.0,
            ..Emitter::FLAT
        }
    }

    fn placement(&self, listener: &Listener3) -> Placement3 {
        if self.positional {
            place(listener, self.pos, self.vel, &self.dist, self.doppler)
        } else {
            Placement3 { pan: self.pan.clamp(-1.0, 1.0), ..Placement3::FLAT }
        }
    }

    /// A 2D emitter at a stereo position.
    pub fn panned(pan: f32) -> Emitter {
        Emitter { pan: finite(pan, 0.0).clamp(-1.0, 1.0), ..Emitter::FLAT }
    }
}

/// Voice importance, low to high.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

/// What the bus lets go of: it never frees heap memory itself, so whoever drives it drops these
/// where freeing is allowed (off the audio thread).
pub enum Retired {
    /// An instrument that left the bus: replaced under its key, removed, faded out ([`SynthBus::reap`]),
    /// or a retiring one cut short to make room.
    Instrument { key: u64, inst: Box<dyn Instrument> },
    /// An instrument the bus had no slot for (every slot holds a live instrument).
    Refused { key: u64, inst: Box<dyn Instrument> },
    /// A vehicle's replaced loop set, or the given set when no vehicle has its key.
    Loops(LoopSet),
}

/// One change to the bus as a value: what a control thread sends to the thread that renders the
/// bus ([`SynthBus::apply`]). One variant per mutating method, with that method's arguments.
pub enum Command {
    Listener(Listener3),
    Room(RoomParams),
    Master(f32),
    GroupGain { group: Group, gain: f32 },
    GroupScript { group: Group, gain: f32, ease: f32 },
    GroupDuck { group: Group, to: f32, attack: f32, hold: f32, release: f32 },
    Muffle { amount: f32, ease: f32 },
    ResetScriptMix,
    InstrumentGroup { key: u64, group: Group },
    Duck(f32),
    PlayRecipe { id: u64, recipe: Recipe, pitch: f32, gain: f32, emitter: Emitter, priority: Priority, key: u64 },
    /// An interface cue keeps the player's effects volume and bypasses script mix controls.
    PlayUnmixedRecipe { id: u64, recipe: Recipe, pitch: f32, gain: f32, emitter: Emitter, priority: Priority, key: u64 },
    MoveVoice { id: u64, pos: V3, vel: V3 },
    StopVoice { id: u64 },
    Vehicle { key: u64, spec: EngineSpec, engine: EngineInput, tyres: TyreInput, emitter: Emitter, interior: f32, gain: f32 },
    VehicleStart { key: u64 },
    VehicleStopEngine { key: u64 },
    VehicleThump { key: u64, strength: f32 },
    VehicleRelease { key: u64 },
    /// The vehicle voice of `from` is `to`'s from now on ([`SynthBus::vehicle_rekey`]).
    VehicleRekey { from: u64, to: u64 },
    VehicleLoops { key: u64, loops: LoopSet, mix: f32 },
    EndFrame,
    Tone { id: u64, freq: f32, wave: Option<Wave>, gain: f32 },
    ToneSet { id: u64, freq: Option<f32>, gain: Option<f32> },
    ToneStop { id: u64 },
    /// Every tone whose `id >> shift == prefix` ([`SynthBus::fade_tones`]); shift 0 names one tone.
    /// (apply uses `id.checked_shr(shift).unwrap_or(0)`, so a shift of 64 or more matches prefix 0 only.)
    FadeTones { prefix: u64, shift: u32, secs: f32 },
    StopAllTones,
    AddInstrument { key: u64, inst: Box<dyn Instrument>, emitter: Emitter, gain: f32 },
    RemoveInstrument { key: u64 },
    FadeInstrument { key: u64, secs: f32 },
    InstrumentEmitter { key: u64, emitter: Emitter, gain: Option<f32> },
    NoteOn { key: u64, note: u8, velocity: u8 },
    NoteOff { key: u64, note: u8 },
    Control { key: u64, control: Control, value: f32 },
    Silence,
    Clear,
}

struct RecipeSlot {
    bypass_mix: bool,
    active: bool,
    id: u64,
    key: u64,
    voice: RecipeVoice,
    spatial: Spatial,
    emitter: Emitter,
    place: Placement3,
    gain: f32,
    send: f32,
    fade: f32,
    fading: bool,
    priority: Priority,
}

struct VehicleSlot {
    active: bool,
    key: u64,
    sound: VehicleSound,
    spatial: Spatial,
    emitter: Emitter,
    place: Placement3,
    gain: Smooth,
    stopping: bool,
    seen: u64,
    send: f32,
    /// This voice's incarnation (see [`VehicleView::serial`]).
    serial: u64,
}

struct ToneSlot {
    active: bool,
    id: u64,
    osc: Osc,
    freq: Smooth,
    gain: Smooth,
    level: f32,
    releasing: bool,
    /// Level lost per second while releasing.
    release_rate: f32,
    noise: crate::dsp::Noise,
    is_noise: bool,
}

struct InstrumentSlot {
    key: u64,
    inst: Box<dyn Instrument>,
    spatial_l: Spatial,
    spatial_r: Spatial,
    emitter: Emitter,
    place: Placement3,
    gain: f32,
    send: f32,
    /// A retiring slot's gain ramp, 1 -> 0; no lookup finds it. At 0 it goes to
    /// [`SynthBus::reap`], so the callback never drops it.
    fade: f32,
    /// Per-sample ramp step; 0 while the slot is live.
    fade_step: f32,
    group: Group,
}

impl InstrumentSlot {
    fn live(&self) -> bool {
        self.fade_step == 0.0
    }
}

/// Live numbers for diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BusStats {
    /// Render time / buffer time, smoothed (1.0 = a full core's budget).
    pub cpu_load: f32,
    pub peak_cpu_load: f32,
    pub recipe_voices: usize,
    pub vehicle_voices: usize,
    pub tone_voices: usize,
    pub instruments: usize,
    pub steals: u64,
    pub shed: u64,
    /// Master peak of the last buffer after the limiter.
    pub peak: f32,
    /// Gain reduction of the glue compressor, dB.
    pub reduction_db: f32,
    pub blocks: u64,
}

/// One live vehicle voice, for diagnostics and the control side's view of the bus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleView {
    pub key: u64,
    pub input: EngineInput,
    pub gain: f32,
    pub level: f32,
    pub interior: f32,
    pub stopping: bool,
    /// Which incarnation of `key` this is: a new number each time a drive starts the voice
    /// (a free or stolen slot, or one that was stopping), so a control thread knows when a
    /// vehicle needs its loops and starter again.
    pub serial: u64,
}

/// What the bus looks like after a block, by value: what its renderer publishes for a control
/// thread that cannot look at the bus itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BusView {
    pub rate: f32,
    pub stats: BusStats,
    pub recipe_voices: usize,
    pub tone_voices: usize,
    /// Instrument slots, retiring ones included.
    pub instrument_slots: usize,
    /// The live (not retiring) instruments' keys, `instrument_count` of them.
    pub instruments: [u64; INSTRUMENT_SLOTS],
    pub instrument_count: usize,
    /// The active vehicle voices, `vehicle_count` of them.
    pub vehicles: [VehicleView; VEHICLE_VOICES],
    pub vehicle_count: usize,
}

pub struct SynthBus {
    rate: f32,
    listener: Listener3,
    recipes: Vec<RecipeSlot>,
    vehicles: Vec<VehicleSlot>,
    tones: Vec<ToneSlot>,
    instruments: Vec<InstrumentSlot>,
    reverb: Reverb,
    comp: Compressor,
    limiter: Limiter,
    ducker: Ducker,
    duck_key: f32,
    master: f32,
    /// Group volumes ([`Group`]), smoothed so a mute fades.
    fx_gain: Smooth,
    music_gain: Smooth,
    script: [GroupMix; 2],
    muffle: Muffle,
    low_pass: [LowPassPair; 3],
    // Reusable control ramps and the dry L/R + send that bypass the effects filter.
    fx_levels: [f32; BLOCK],
    music_levels: [f32; BLOCK],
    poles: [f32; BLOCK],
    clear: [[f32; BLOCK]; 3],
    /// Reverb send for the external (sampled) input.
    pub external_send: f32,
    mono: [f32; BLOCK],
    dry_l: [f32; BLOCK],
    dry_r: [f32; BLOCK],
    send: [f32; BLOCK],
    inst_l: [f32; BLOCK],
    inst_r: [f32; BLOCK],
    seed: u32,
    frame: u64,
    /// The last vehicle incarnation handed out.
    serial: u64,
    stats: BusStats,
    shed_level: usize,
}

impl SynthBus {
    /// Build every pool at `rate`. This allocates (delay lines for twelve
    /// engines and a reverb); do it off the audio thread.
    pub fn new(rate: f32) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        let default_recipe = Recipe::default();
        let recipes = (0..RECIPE_VOICES)
            .map(|_| RecipeSlot {
                bypass_mix: false,
                active: false,
                id: 0,
                key: 0,
                voice: RecipeVoice::new(&default_recipe, 1.0, None, false, 1),
                spatial: Spatial::new(rate),
                emitter: Emitter::FLAT,
                place: Placement3::FLAT,
                gain: 1.0,
                send: 0.0,
                fade: 1.0,
                fading: false,
                priority: Priority::Normal,
            })
            .collect();
        let vehicles = (0..VEHICLE_VOICES)
            .map(|_| VehicleSlot {
                active: false,
                key: 0,
                sound: VehicleSound::new(rate, EngineSpec::default()),
                spatial: Spatial::new(rate),
                emitter: Emitter::FLAT,
                place: Placement3::FLAT,
                gain: Smooth::new(0.0),
                stopping: false,
                seen: 0,
                send: 0.1,
                serial: 0,
            })
            .collect();
        let tones = (0..TONE_VOICES)
            .map(|i| ToneSlot {
                active: false,
                id: 0,
                osc: Osc::new(Wave::Sine, 0.0),
                freq: Smooth::new(220.0),
                gain: Smooth::new(0.0),
                level: 0.0,
                releasing: false,
                release_rate: TONE_RELEASE_RATE,
                noise: crate::dsp::Noise::new(crate::dsp::NoiseColor::Pink, 0x51ed + i as u32),
                is_noise: false,
            })
            .collect();
        let mut reverb = Reverb::new(rate);
        reverb.snap_room(RoomPreset::Outdoor.params());
        SynthBus {
            rate,
            listener: Listener3::default(),
            recipes,
            vehicles,
            tones,
            // Room for a full set retiring (fading) beside a full new set:
            // a world switch never allocates in a lock the callback takes.
            instruments: Vec::with_capacity(INSTRUMENT_SLOTS * 2),
            reverb,
            comp: {
                let mut c = Compressor::new(rate, -14.0, 2.5, 0.01, 0.2);
                c.makeup_db = 1.5;
                c
            },
            limiter: Limiter::new(rate, -1.0),
            ducker: Ducker::new(rate, 9.0),
            duck_key: 0.0,
            master: 1.0,
            fx_gain: Smooth::new(1.0),
            music_gain: Smooth::new(1.0),
            script: [GroupMix::default(); 2],
            muffle: Muffle::default(),
            low_pass: [LowPassPair::default(); 3],
            fx_levels: [0.0; BLOCK],
            music_levels: [0.0; BLOCK],
            poles: [0.0; BLOCK],
            clear: [[0.0; BLOCK]; 3],
            external_send: 0.12,
            mono: [0.0; BLOCK],
            dry_l: [0.0; BLOCK],
            dry_r: [0.0; BLOCK],
            send: [0.0; BLOCK],
            inst_l: [0.0; BLOCK],
            inst_r: [0.0; BLOCK],
            seed: 0x1234,
            frame: 0,
            serial: 0,
            stats: BusStats::default(),
            shed_level: 0,
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn stats(&self) -> BusStats {
        self.stats
    }

    pub fn view(&self) -> BusView {
        let (recipe_voices, vehicle_count, tone_voices, instrument_slots) = self.live_counts();
        let mut view = BusView {
            rate: self.rate,
            stats: self.stats,
            recipe_voices,
            tone_voices,
            instrument_slots,
            vehicle_count,
            ..BusView::default()
        };
        for key in self.instrument_keys() {
            view.instruments[view.instrument_count] = key;
            view.instrument_count += 1;
        }
        for (v, s) in view.vehicles.iter_mut().zip(self.vehicles.iter().filter(|s| s.active)) {
            *v = VehicleView {
                key: s.key,
                input: s.sound.engine.input(),
                gain: s.place.gain * s.gain.value,
                level: s.sound.level(),
                interior: s.sound.engine.interior,
                stopping: s.stopping,
                serial: s.serial,
            };
        }
        view
    }

    /// Apply one command. Allocates and frees nothing: a payload it replaces
    /// or cannot place goes to `retire`. With [`SynthBus::render`] and
    /// [`SynthBus::reap`] this is everything the audio thread calls.
    pub fn apply(&mut self, cmd: Command, retire: &mut impl FnMut(Retired)) {
        let bypass_mix = matches!(&cmd, Command::PlayUnmixedRecipe { .. });
        match cmd {
            Command::Listener(listener) => self.set_listener(listener),
            Command::Room(room) => self.set_room(room),
            Command::Master(gain) => self.set_master(gain),
            Command::GroupGain { group, gain } => self.set_group_gain(group, gain),
            Command::GroupScript { group, gain, ease } => self.set_group_script(group, gain, ease),
            Command::GroupDuck { group, to, attack, hold, release } => self.duck_group(group, to, attack, hold, release),
            Command::Muffle { amount, ease } => self.set_muffle(amount, ease),
            Command::ResetScriptMix => self.reset_script_mix(),
            Command::InstrumentGroup { key, group } => self.set_instrument_group(key, group),
            Command::Duck(key) => self.set_duck(key),
            Command::PlayRecipe { id, recipe, pitch, gain, emitter, priority, key }
            | Command::PlayUnmixedRecipe { id, recipe, pitch, gain, emitter, priority, key } => {
                self.start_recipe(id, &recipe, pitch, gain, emitter, priority, key, bypass_mix);
            }
            Command::MoveVoice { id, pos, vel } => self.move_voice(id, pos, vel),
            Command::StopVoice { id } => self.stop_voice(id),
            Command::Vehicle { key, spec, engine, tyres, emitter, interior, gain } => {
                self.vehicle(key, &spec, engine, tyres, emitter, interior, gain);
            }
            Command::VehicleStart { key } => self.vehicle_start(key),
            Command::VehicleStopEngine { key } => self.vehicle_stop_engine(key),
            Command::VehicleThump { key, strength } => self.vehicle_thump(key, strength),
            Command::VehicleRelease { key } => self.vehicle_release(key),
            Command::VehicleRekey { from, to } => self.vehicle_rekey(from, to),
            Command::VehicleLoops { key, loops, mix } => self.vehicle_loops(key, loops, mix, retire),
            Command::EndFrame => self.end_frame(),
            Command::Tone { id, freq, wave, gain } => self.tone(id, freq, wave, gain),
            Command::ToneSet { id, freq, gain } => self.tone_set(id, freq, gain),
            Command::ToneStop { id } => self.tone_stop(id),
            Command::FadeTones { prefix, shift, secs } => self.fade_tones(|id| id.checked_shr(shift).unwrap_or(0) == prefix, secs),
            Command::StopAllTones => self.stop_all_tones(),
            Command::AddInstrument { key, inst, emitter, gain } => {
                self.add_instrument(key, inst, emitter, gain, retire);
            }
            Command::RemoveInstrument { key } => {
                self.remove_instrument(key, retire);
            }
            Command::FadeInstrument { key, secs } => self.fade_instrument(key, secs),
            Command::InstrumentEmitter { key, emitter, gain } => self.set_instrument_emitter(key, emitter, gain),
            Command::NoteOn { key, note, velocity } => {
                if let Some(inst) = self.instrument_mut(key) {
                    inst.note_on(note, velocity);
                }
            }
            Command::NoteOff { key, note } => {
                if let Some(inst) = self.instrument_mut(key) {
                    inst.note_off(note);
                }
            }
            Command::Control { key, control, value } => {
                if let Some(inst) = self.instrument_mut(key) {
                    inst.control(control, value);
                }
            }
            Command::Silence => self.silence(),
            Command::Clear => self.clear(),
        }
    }

    pub fn set_listener(&mut self, listener: Listener3) {
        self.listener = listener;
    }

    pub fn listener(&self) -> Listener3 {
        self.listener
    }

    pub fn set_room(&mut self, room: RoomParams) {
        self.reverb.set_room(room);
    }

    pub fn room(&self) -> RoomParams {
        self.reverb.room()
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = finite(gain, 1.0).clamp(0.0, 4.0);
    }

    /// A group's volume, 0..4 (0 = that group muted); glides.
    pub fn set_group_gain(&mut self, group: Group, gain: f32) {
        let g = finite(gain, 1.0).clamp(0.0, 4.0);
        match group {
            Group::Fx => self.fx_gain.target = g,
            Group::Music => self.music_gain.target = g,
        }
    }

    pub fn set_group_script(&mut self, group: Group, gain: f32, ease: f32) {
        self.script[group as usize].set_gain(gain, ease);
    }

    pub fn duck_group(&mut self, group: Group, to: f32, attack: f32, hold: f32, release: f32) {
        self.script[group as usize].duck(to, attack, hold, release);
    }

    pub fn set_muffle(&mut self, amount: f32, ease: f32) {
        if self.muffle.bypassed() && amount > 0.0 { self.low_pass = Default::default(); }
        self.muffle.set(amount, ease, self.rate);
    }

    pub fn reset_script_mix(&mut self) {
        self.script = Default::default();
        self.muffle = Default::default();
    }

    pub fn group_gain(&self, group: Group) -> f32 {
        match group {
            Group::Fx => self.fx_gain.target,
            Group::Music => self.music_gain.target,
        }
    }

    /// Which volume group an instrument plays in.
    pub fn set_instrument_group(&mut self, key: u64, group: Group) {
        if let Some(s) = self.instruments.iter_mut().find(|s| s.live() && s.key == key) {
            s.group = group;
        }
    }

    /// Voice-activity key 0..1 (NPC speech, voice chat): ducks the bus.
    pub fn set_duck(&mut self, key: f32) {
        self.duck_key = finite(key, 0.0).clamp(0.0, 1.0);
    }

    fn next_seed(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.seed
    }

    // ── recipes ──────────────────────────────────────────────────────────

    /// Start a recipe. `key` (non-zero) makes it replace a still-sounding
    /// voice with the same key (one footstep per body, one pain per enemy).
    /// The caller mints `id`, unique per bus, for `move_voice`/`stop_voice`.
    /// Returns true when started, false when every slot outranks it or it is
    /// out of earshot.
    pub fn play_recipe(
        &mut self,
        id: u64,
        recipe: &Recipe,
        pitch: f32,
        gain: f32,
        emitter: Emitter,
        priority: Priority,
        key: u64,
    ) -> bool {
        self.start_recipe(id, recipe, pitch, gain, emitter, priority, key, false)
    }

    fn start_recipe(&mut self, id: u64, recipe: &Recipe, pitch: f32, gain: f32, emitter: Emitter,
                    priority: Priority, key: u64, bypass_mix: bool) -> bool {
        let place = emitter.placement(&self.listener);
        let loudness = place.gain * gain;
        if emitter.positional && place.gain <= 0.0 {
            return false;
        }
        // Same key: the old voice fades, the new one takes a fresh slot.
        if key != 0 {
            for slot in self.recipes.iter_mut().filter(|s| s.active && s.key == key && !s.fading) {
                slot.fading = true;
            }
        }
        let cap = RECIPE_VOICES - self.shed_level;
        let live = self.recipes.iter().filter(|s| s.active).count();
        let index = match self.recipes.iter().position(|s| !s.active).filter(|_| live < cap) {
            Some(i) => i,
            None => {
                // Steal the quietest that does not outrank the newcomer.
                let victim = self
                    .recipes
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.active && s.priority <= priority)
                    .min_by(|a, b| {
                        let la = a.1.place.gain * a.1.gain * a.1.voice.level.max(0.01) * a.1.fade;
                        let lb = b.1.place.gain * b.1.gain * b.1.voice.level.max(0.01) * b.1.fade;
                        la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, s)| (i, s.place.gain * s.gain * s.voice.level.max(0.01)));
                match victim {
                    Some((i, level)) if level <= loudness * 0.5 || self.recipes[i].priority < priority => {
                        self.stats.steals += 1;
                        // A free slot (if the cap, not the pool, was full)
                        // lets the victim fade instead of cutting.
                        match self.recipes.iter().position(|s| !s.active) {
                            Some(free) => {
                                self.recipes[i].fading = true;
                                free
                            }
                            None => i,
                        }
                    }
                    _ => return false,
                }
            }
        };
        let seed = self.next_seed();
        let slot = &mut self.recipes[index];
        slot.active = true;
        slot.id = id;
        slot.key = key;
        slot.bypass_mix = bypass_mix;
        slot.voice = RecipeVoice::new(recipe, pitch, None, false, seed);
        slot.emitter = emitter;
        slot.place = place;
        slot.spatial.flat = !emitter.positional;
        slot.spatial.width = 1.0;
        slot.spatial.snap(&place, emitter.occlusion);
        slot.gain = finite(gain, 1.0).clamp(0.0, 4.0);
        slot.send = recipe.reverb;
        slot.fade = 1.0;
        slot.fading = false;
        slot.priority = priority;
        true
    }

    /// Move a playing recipe voice (a projectile's whoosh).
    pub fn move_voice(&mut self, id: u64, pos: V3, vel: V3) {
        if let Some(s) = self.recipes.iter_mut().find(|s| s.active && s.id == id) {
            s.emitter.pos = pos;
            s.emitter.vel = vel;
        }
    }

    pub fn stop_voice(&mut self, id: u64) {
        if let Some(s) = self.recipes.iter_mut().find(|s| s.active && s.id == id) {
            s.voice.release();
        }
    }

    // ── vehicles ─────────────────────────────────────────────────────────

    /// Drive the vehicle voice for `key` this frame (starting one if
    /// needed). Vehicles not updated in a frame fade out at
    /// [`SynthBus::end_frame`].
    #[allow(clippy::too_many_arguments)]
    pub fn vehicle(
        &mut self,
        key: u64,
        spec: &EngineSpec,
        engine: EngineInput,
        tyres: TyreInput,
        emitter: Emitter,
        interior: f32,
        gain: f32,
    ) -> bool {
        let place = emitter.placement(&self.listener);
        let frame = self.frame;
        let index = match self.vehicles.iter().position(|s| s.active && s.key == key) {
            Some(i) => {
                if self.vehicles[i].stopping {
                    self.serial += 1;
                    self.vehicles[i].serial = self.serial;
                }
                i
            }
            None => {
                if emitter.positional && place.gain <= 0.0 {
                    return false;
                }
                let index = match self.vehicles.iter().position(|s| !s.active) {
                    Some(i) => i,
                    None => {
                        // Steal the quietest vehicle if the newcomer is louder.
                        let (i, level) = self
                            .vehicles
                            .iter()
                            .enumerate()
                            .map(|(i, s)| (i, s.place.gain * s.gain.target))
                            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                            .unwrap_or((0, 0.0));
                        if level >= place.gain * gain {
                            return false;
                        }
                        self.stats.steals += 1;
                        i
                    }
                };
                let seed = self.next_seed();
                self.serial += 1;
                let slot = &mut self.vehicles[index];
                slot.active = true;
                slot.key = key;
                slot.serial = self.serial;
                slot.sound.engine.set_spec(*spec);
                slot.sound.engine.seed(seed);
                slot.sound.engine.set_input(engine);
                slot.gain = Smooth::new(0.0);
                slot.stopping = false;
                slot.spatial.snap(&place, emitter.occlusion);
                index
            }
        };
        let slot = &mut self.vehicles[index];
        if slot.sound.engine.spec != *spec {
            slot.sound.engine.set_spec(*spec);
        }
        slot.sound.set(engine, tyres);
        slot.sound.set_interior(interior);
        slot.emitter = emitter;
        slot.place = place;
        slot.spatial.flat = !emitter.positional;
        // Inside the cabin the engine surrounds you: narrow the image.
        slot.spatial.width = 1.0 - 0.7 * interior.clamp(0.0, 1.0);
        slot.gain.target = finite(gain, 1.0).clamp(0.0, 4.0);
        slot.stopping = false;
        slot.seen = frame;
        slot.send = 0.1 * (1.0 - interior.clamp(0.0, 1.0));
        true
    }

    pub fn vehicle_start(&mut self, key: u64) {
        if let Some(s) = self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            s.sound.engine.start();
        }
    }

    pub fn vehicle_stop_engine(&mut self, key: u64) {
        if let Some(s) = self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            s.sound.engine.stop();
        }
    }

    pub fn vehicle_thump(&mut self, key: u64, strength: f32) {
        if let Some(s) = self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            s.sound.tyres.thump(strength);
        }
    }

    pub fn vehicle_release(&mut self, key: u64) {
        if let Some(s) = self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            s.stopping = true;
            s.gain.target = 0.0;
        }
    }

    /// The body a vehicle voice follows goes on under another key (a world
    /// rebuilt around it gave it a new id): the voice keeps sounding as it
    /// was, its engine, loops and incarnation unchanged, and the next drive
    /// of `to` is its next frame. A voice already under `to` keeps the key,
    /// and `from`'s fades.
    pub fn vehicle_rekey(&mut self, from: u64, to: u64) {
        if from == to {
            return;
        }
        let taken = self.vehicles.iter().any(|s| s.active && s.key == to);
        if let Some(s) = self.vehicles.iter_mut().find(|s| s.active && s.key == from) {
            if taken {
                s.stopping = true;
                s.gain.target = 0.0;
            } else {
                s.key = to;
            }
        }
    }

    /// Close a control frame: every vehicle nobody drove for the last
    /// [`STALE_FRAMES`] frames fades out (its body is gone or out of
    /// earshot). Several views may share one bus and each closes its own
    /// frames, hence a margin rather than "not this frame".
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        for s in self.vehicles.iter_mut().filter(|s| s.active && frame.saturating_sub(s.seen) >= STALE_FRAMES) {
            s.stopping = true;
            s.gain.target = 0.0;
        }
        self.frame += 1;
    }

    // ── sustained tones (the legacy `game.tone` primitive) ───────────────

    pub fn tone(&mut self, id: u64, freq: f32, wave: Option<Wave>, gain: f32) {
        let index = self
            .tones
            .iter()
            .position(|t| t.active && t.id == id)
            .or_else(|| self.tones.iter().position(|t| !t.active))
            .unwrap_or_else(|| {
                // Full: the quietest yields.
                (0..TONE_VOICES)
                    .min_by(|&a, &b| self.tones[a].gain.value.partial_cmp(&self.tones[b].gain.value).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(0)
            });
        let t = &mut self.tones[index];
        let fresh = !(t.active && t.id == id);
        t.active = true;
        t.id = id;
        t.is_noise = wave.is_none();
        if let Some(w) = wave {
            t.osc.wave = w;
        }
        let f = finite(freq, 220.0).clamp(20.0, 8000.0);
        if fresh {
            t.freq = Smooth::new(f);
            t.gain = Smooth::new(0.0);
            t.level = 0.0;
        }
        t.freq.target = f;
        t.gain.target = finite(gain, 0.0).clamp(0.0, 1.0);
        t.releasing = false;
        t.release_rate = TONE_RELEASE_RATE;
    }

    pub fn tone_set(&mut self, id: u64, freq: Option<f32>, gain: Option<f32>) {
        if let Some(t) = self.tones.iter_mut().find(|t| t.active && t.id == id) {
            if let Some(f) = freq {
                t.freq.target = finite(f, 220.0).clamp(20.0, 8000.0);
            }
            if let Some(g) = gain {
                t.gain.target = finite(g, 0.0).clamp(0.0, 1.0);
            }
        }
    }

    pub fn tone_stop(&mut self, id: u64) {
        if let Some(t) = self.tones.iter_mut().find(|t| t.active && t.id == id) {
            t.releasing = true;
        }
    }

    /// Release every tone `owned` claims, fading its gain out over `secs`
    /// (a world leaving: slower than a stop, so a hum does not end in a
    /// cut).
    pub fn fade_tones(&mut self, owned: impl Fn(u64) -> bool, secs: f32) {
        for t in self.tones.iter_mut().filter(|t| t.active && owned(t.id)) {
            t.releasing = true;
            t.release_rate = 1.0 / finite(secs, 0.05).max(0.005);
        }
    }

    pub fn stop_all_tones(&mut self) {
        for t in self.tones.iter_mut() {
            t.releasing = true;
        }
    }

    // ── instruments ──────────────────────────────────────────────────────

    /// Add an instrument under `key`, returning true when installed. A live
    /// key is replaced in place and its old instrument goes to `retire`.
    /// A full live set refuses the new instrument through `retire` and returns
    /// false. Otherwise a new slot is used; at capacity the retiring slot with
    /// the lowest fade goes to `retire` first, so the pool never grows.
    pub fn add_instrument(
        &mut self,
        key: u64,
        inst: Box<dyn Instrument>,
        emitter: Emitter,
        gain: f32,
        retire: &mut impl FnMut(Retired),
    ) -> bool {
        let place = emitter.placement(&self.listener);
        let make = |inst| {
            let mut spatial_l = Spatial::new(self.rate);
            let mut spatial_r = Spatial::new(self.rate);
            spatial_l.flat = !emitter.positional;
            spatial_r.flat = !emitter.positional;
            spatial_l.snap(&place, emitter.occlusion);
            spatial_r.snap(&place, emitter.occlusion);
            InstrumentSlot {
                key,
                inst,
                spatial_l,
                spatial_r,
                emitter,
                place,
                gain: finite(gain, 1.0).clamp(0.0, 4.0),
                send: 0.15,
                fade: 1.0,
                fade_step: 0.0,
                group: Group::Fx,
            }
        };
        if let Some(slot) = self.instruments.iter_mut().find(|s| s.live() && s.key == key) {
            let new = make(inst);
            let old = std::mem::replace(slot, new);
            retire(Retired::Instrument { key, inst: old.inst });
            return true;
        }
        // Retiring slots fade beside the new ones and do not count.
        if self.instruments.iter().filter(|s| s.live()).count() >= INSTRUMENT_SLOTS {
            retire(Retired::Refused { key, inst });
            return false;
        }
        // At capacity (more slots than live instruments, so one is retiring) the quietest
        // retiring slot makes room.
        if self.instruments.len() == self.instruments.capacity() {
            let quietest = self.instruments.iter().enumerate().filter(|(_, s)| !s.live())
                .min_by(|a, b| a.1.fade.partial_cmp(&b.1.fade).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
            if let Some(i) = quietest {
                let old = self.instruments.swap_remove(i);
                retire(Retired::Instrument { key: old.key, inst: old.inst });
            }
        }
        self.instruments.push(make(inst));
        true
    }

    /// Remove an instrument, handing it back to drop (or to keep).
    pub fn remove_instrument(&mut self, key: u64, retire: &mut impl FnMut(Retired)) -> bool {
        let Some(i) = self.instruments.iter().position(|s| s.live() && s.key == key) else { return false };
        let old = self.instruments.swap_remove(i);
        retire(Retired::Instrument { key, inst: old.inst });
        true
    }

    /// Retire an instrument without a click: held notes release and its
    /// output fades to silence over `secs`. From now on no lookup finds it,
    /// so the same key can be installed again at once;
    /// [`SynthBus::reap`] hands it back once the fade has run out.
    pub fn fade_instrument(&mut self, key: u64, secs: f32) {
        let rate = self.rate;
        if let Some(s) = self.instruments.iter_mut().find(|s| s.live() && s.key == key) {
            s.inst.all_notes_off();
            s.fade_step = 1.0 / (finite(secs, 0.25).max(0.005) * rate);
        }
    }

    /// Hand back every retired instrument whose fade has finished, to be
    /// dropped by the caller off the audio thread.
    pub fn reap(&mut self, retire: &mut impl FnMut(Retired)) {
        let mut i = 0;
        while i < self.instruments.len() {
            let s = &self.instruments[i];
            if !s.live() && s.fade <= 0.0 {
                let old = self.instruments.swap_remove(i);
                retire(Retired::Instrument { key: old.key, inst: old.inst });
            } else {
                i += 1;
            }
        }
    }

    pub fn instrument_mut(&mut self, key: u64) -> Option<&mut (dyn Instrument + 'static)> {
        self.instruments.iter_mut().find(|s| s.live() && s.key == key).map(|s| s.inst.as_mut())
    }

    /// Keys of the live (not retiring) instruments.
    pub fn instrument_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.instruments.iter().filter(|s| s.live()).map(|s| s.key)
    }

    pub fn set_instrument_emitter(&mut self, key: u64, emitter: Emitter, gain: Option<f32>) {
        if let Some(s) = self.instruments.iter_mut().find(|s| s.live() && s.key == key) {
            s.emitter = emitter;
            s.spatial_l.flat = !emitter.positional;
            s.spatial_r.flat = !emitter.positional;
            if let Some(g) = gain {
                s.gain = finite(g, 1.0).clamp(0.0, 4.0);
            }
        }
    }

    /// Everything off: voices fade, tones release, instruments go quiet.
    pub fn silence(&mut self) {
        for s in self.recipes.iter_mut().filter(|s| s.active) {
            s.fading = true;
        }
        for s in self.vehicles.iter_mut().filter(|s| s.active) {
            s.stopping = true;
            s.gain.target = 0.0;
        }
        self.stop_all_tones();
        for s in self.instruments.iter_mut() {
            s.inst.all_notes_off();
        }
    }

    /// Hard stop (mute, teardown): every voice, vehicle and tone ends NOW,
    /// instruments release their notes. Control thread only; the caller is
    /// about to stop rendering or wants silence regardless of clicks.
    pub fn clear(&mut self) {
        for s in self.recipes.iter_mut() {
            s.active = false;
        }
        for s in self.vehicles.iter_mut() {
            s.active = false;
        }
        for t in self.tones.iter_mut() {
            t.active = false;
        }
        for s in self.instruments.iter_mut() {
            s.inst.all_notes_off();
        }
    }

    /// Install recorded engine loops on a vehicle voice (see
    /// [`VehicleSound::set_loops`]); the replaced set comes back to drop.
    pub fn vehicle_loops(&mut self, key: u64, loops: LoopSet, mix: f32, retire: &mut impl FnMut(Retired)) {
        let old = match self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            Some(s) => s.sound.set_loops(loops, mix),
            None => loops,
        };
        retire(Retired::Loops(old));
    }

    /// (key, engine input, placement gain, engine level, interior) of every
    /// live vehicle voice — diagnostics.
    pub fn vehicle_snapshot(&self) -> impl Iterator<Item = (u64, EngineInput, f32, f32, f32)> + '_ {
        self.vehicles
            .iter()
            .filter(|s| s.active)
            .map(|s| (s.key, s.sound.engine.input(), s.place.gain * s.gain.value, s.sound.level(), s.sound.engine.interior))
    }

    pub fn has_vehicle(&self, key: u64) -> bool {
        self.vehicles.iter().any(|s| s.active && s.key == key && !s.stopping)
    }

    pub fn live_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.recipes.iter().filter(|s| s.active).count(),
            self.vehicles.iter().filter(|s| s.active).count(),
            self.tones.iter().filter(|s| s.active).count(),
            self.instruments.len(),
        )
    }

    // ── render ───────────────────────────────────────────────────────────

    /// Render into planar stereo. `out_l`/`out_r` may already hold an
    /// EXTERNAL mix (the sampled bus): it is kept, sent to the room at
    /// [`SynthBus::external_send`], and passes through the same master
    /// dynamics, so samples and synthesis share one bus.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let started = cpu_seconds();
        let n = out_l.len().min(out_r.len());
        let mut at = 0;
        let mut peak = 0.0f32;
        while at < n {
            let len = (n - at).min(BLOCK);
            let end = at + len;
            // Resume the original summation order on the first fully clear frame,
            // even when the release ends in the middle of this render block.
            if self.muffle.amount == 0.0 && self.muffle.pole.left > 0.0 {
                let wet = ((self.muffle.pole.left * self.rate as f64).round() as usize).saturating_sub(1);
                if wet > 0 && wet < len {
                    self.render_block(&mut out_l[at..at + wet], &mut out_r[at..at + wet], &mut peak);
                    at += wet;
                }
            }
            self.render_block(&mut out_l[at..end], &mut out_r[at..end], &mut peak);
            at = end;
        }
        // CPU accounting and graceful shedding.
        let spent = match (started, cpu_seconds()) {
            (Some(a), Some(b)) => (b - a) as f32,
            _ => 0.0,
        };
        let budget = n as f32 / self.rate;
        if budget > 0.0 && started.is_some() {
            let load = spent / budget;
            self.stats.cpu_load += (load - self.stats.cpu_load) * 0.05;
            self.stats.peak_cpu_load = self.stats.peak_cpu_load.max(load);
            if self.stats.cpu_load > 0.5 && self.shed_level < RECIPE_VOICES - 8 {
                self.shed_level += 4;
                self.stats.shed += 4;
            } else if self.stats.cpu_load < 0.25 && self.shed_level > 0 {
                self.shed_level -= 1;
            }
        }
        self.stats.peak = peak;
        self.stats.reduction_db = self.comp.reduction_db();
        self.stats.blocks += 1;
        let (r, v, t, i) = self.live_counts();
        self.stats.recipe_voices = r;
        self.stats.vehicle_voices = v;
        self.stats.tone_voices = t;
        self.stats.instruments = i;
    }

    fn render_block(&mut self, out_l: &mut [f32], out_r: &mut [f32], peak: &mut f32) {
        let len = out_l.len();
        let rate = self.rate;
        let inv = 1.0 / rate;
        let listener = self.listener;
        let dry_l = &mut self.dry_l[..len];
        let dry_r = &mut self.dry_r[..len];
        let send = &mut self.send[..len];
        // The effects (recipes, vehicles, tones) mix alone first, so their
        // group volume can scale them before the external input and the
        // instruments join.
        dry_l.fill(0.0);
        dry_r.fill(0.0);
        send.fill(0.0);
        let group_coef = crate::dsp::Smooth::coef(GROUP_GLIDE, rate);
        let mono = &mut self.mono[..len];

        let fx_script = self.script[Group::Fx as usize].levels(&mut self.fx_levels[..len], rate);
        let music_script = self.script[Group::Music as usize].levels(&mut self.music_levels[..len], rate);
        let filtered = self.muffle.poles(&mut self.poles[..len], rate);
        let bypass_voices = (filtered > 0 || fx_script) && self.recipes.iter().any(|s| s.active && s.bypass_mix);
        let split_fx = filtered > 0 || bypass_voices;
        let [clear_l, clear_r, clear_send] = &mut self.clear;
        if bypass_voices {
            clear_l[..len].fill(0.0);
            clear_r[..len].fill(0.0);
            clear_send[..len].fill(0.0);
        }

        // Recipes.
        let fade_step = len as f32 * inv / STEAL_FADE;
        for s in self.recipes.iter_mut().filter(|s| s.active) {
            s.place = s.emitter.placement(&listener);
            s.spatial.set(&s.place, s.emitter.occlusion);
            mono.fill(0.0);
            s.voice.render(mono, rate, s.place.doppler);
            let (f0, f1) = if s.fading {
                let f0 = s.fade;
                s.fade = (s.fade - fade_step).max(0.0);
                (f0, s.fade)
            } else {
                (1.0, 1.0)
            };
            let (dry_l, dry_r, send) = if split_fx && s.bypass_mix {
                (&mut clear_l[..len], &mut clear_r[..len], &mut clear_send[..len])
            } else {
                (&mut *dry_l, &mut *dry_r, &mut *send)
            };
            let g = s.gain;
            for i in 0..len {
                let f = f0 + (f1 - f0) * (i as f32 / len as f32);
                let x = mono[i] * g * f;
                let (l, r) = s.spatial.process(x);
                dry_l[i] += l;
                dry_r[i] += r;
                send[i] += x * s.send * s.spatial.gain();
            }
            if s.voice.finished() || (s.fading && s.fade <= 0.0) {
                s.active = false;
            }
        }

        // Vehicles.
        let gc = crate::dsp::Smooth::coef(0.05, rate);
        for s in self.vehicles.iter_mut().filter(|s| s.active) {
            s.place = s.emitter.placement(&listener);
            s.spatial.set(&s.place, s.emitter.occlusion);
            mono.fill(0.0);
            s.sound.render(mono, s.place.doppler);
            for i in 0..len {
                let x = mono[i] * s.gain.next(gc);
                let (l, r) = s.spatial.process(x);
                dry_l[i] += l;
                dry_r[i] += r;
                send[i] += x * s.send * s.spatial.gain();
            }
            if s.stopping && s.gain.value < 1.0e-4 {
                s.active = false;
            }
        }

        // Tones (2D, centred: an engine hum belongs to the whole scene).
        let tc = crate::dsp::Smooth::coef(0.03, rate);
        for t in self.tones.iter_mut().filter(|t| t.active) {
            for i in 0..len {
                let f = t.freq.next(tc);
                let g = t.gain.next(tc);
                if t.releasing {
                    t.level -= t.release_rate * inv;
                } else {
                    t.level = (t.level + 60.0 * inv).min(1.0);
                }
                if t.level <= 0.0 {
                    t.active = false;
                    break;
                }
                let raw = if t.is_noise { t.noise.next() } else { t.osc.next(f, inv, 0.0) };
                let x = raw * g * t.level * 0.7;
                dry_l[i] += x;
                dry_r[i] += x;
            }
        }

        // The effects' group volume, then the external input (already
        // balanced by its host) as it is.
        let ext_send = self.external_send;
        if split_fx {
            for i in 0..len {
                let player = self.fx_gain.next(group_coef);
                let g = if fx_script { player * self.fx_levels[i] } else { player };
                dry_l[i] *= g;
                dry_r[i] *= g;
                send[i] *= g;
                if bypass_voices {
                    clear_l[i] = clear_l[i] * player + out_l[i];
                    clear_r[i] = clear_r[i] * player + out_r[i];
                    clear_send[i] = clear_send[i] * player + (out_l[i] + out_r[i]) * 0.5 * ext_send;
                } else {
                    clear_l[i] = out_l[i];
                    clear_r[i] = out_r[i];
                    clear_send[i] = (out_l[i] + out_r[i]) * 0.5 * ext_send;
                }
            }
        } else {
            for i in 0..len {
                let player = self.fx_gain.next(group_coef);
                let g = if fx_script { player * self.fx_levels[i] } else { player };
                dry_l[i] = dry_l[i] * g + out_l[i];
                dry_r[i] = dry_r[i] * g + out_r[i];
                send[i] = send[i] * g + (out_l[i] + out_r[i]) * 0.5 * ext_send;
            }
        }
        // One ramp per group for this block, shared by its instruments.
        let music = &mut self.mono[..len];
        for m in music.iter_mut() { *m = self.music_gain.next(group_coef); }
        if music_script {
            for (m, script) in music.iter_mut().zip(&self.music_levels) { *m *= script; }
        }
        let fx_now = self.fx_gain.value;
        if fx_script && self.instruments.iter().any(|s| s.group == Group::Fx) {
            for level in &mut self.fx_levels[..len] { *level *= fx_now; }
        }

        // Instruments (stereo sources: each side placed at the emitter).
        let il = &mut self.inst_l[..len];
        let ir = &mut self.inst_r[..len];
        for s in self.instruments.iter_mut() {
            s.inst.render(il, ir);
            s.place = s.emitter.placement(&listener);
            s.spatial_l.set(&s.place, s.emitter.occlusion);
            s.spatial_r.set(&s.place, s.emitter.occlusion);
            let g = s.gain;
            let step = s.fade_step;
            let (dry_l, dry_r, send) = if split_fx && s.group == Group::Music {
                (&mut clear_l[..len], &mut clear_r[..len], &mut clear_send[..len])
            } else {
                (&mut *dry_l, &mut *dry_r, &mut *send)
            };
            for i in 0..len {
                // A retiring slot ramps to silence and stays there until
                // the control side reaps it.
                let f = s.fade;
                if step > 0.0 {
                    s.fade = (s.fade - step).max(0.0);
                }
                let f = f * if s.group == Group::Music { music[i] } else if fx_script { self.fx_levels[i] } else { fx_now };
                let (a, b) = (il[i] * g * f, ir[i] * g * f);
                if s.emitter.positional {
                    // A positioned instrument is a point source: its own
                    // stereo image folds into the placement.
                    let (l, r) = s.spatial_l.process((a + b) * 0.5);
                    dry_l[i] += l;
                    dry_r[i] += r;
                } else {
                    dry_l[i] += a;
                    dry_r[i] += b;
                }
                send[i] += (a + b) * 0.5 * s.send * if s.emitter.positional { s.spatial_l.gain() } else { 0.4 };
            }
        }

        if split_fx {
            // The room is shared. Only the effects' dry stereo and mono send pass the poles.
            for i in 0..filtered {
                let pole = self.poles[i];
                dry_l[i] = self.low_pass[0].filter(dry_l[i], pole);
                dry_r[i] = self.low_pass[1].filter(dry_r[i], pole);
                send[i] = self.low_pass[2].filter(send[i], pole);
            }
            for i in 0..len {
                dry_l[i] += clear_l[i];
                dry_r[i] += clear_r[i];
                send[i] += clear_send[i];
            }
        }

        // Room.
        self.reverb.process(send, dry_l, dry_r);

        // Master: duck, glue, limit.
        for i in 0..len {
            let d = self.ducker.gain(self.duck_key) * self.master;
            let (l, r) = self.comp.process(dry_l[i] * d, dry_r[i] * d);
            let (l, r) = self.limiter.process(l, r);
            out_l[i] = l;
            out_r[i] = r;
            *peak = peak.max(l.abs()).max(r.abs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::builtin;

    #[test]
    fn a_recipe_plays_through_the_bus_and_ends() {
        let mut bus = SynthBus::new(48_000.0);
        let r = Recipe::parse(builtin("coin").unwrap()).unwrap();
        assert!(bus.play_recipe(1, &r, 1.0, 1.0, Emitter::FLAT, Priority::Normal, 0));
        let (mut l, mut rr) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        bus.render(&mut l, &mut rr);
        assert!(l.iter().any(|x| x.abs() > 0.01));
        assert_eq!(bus.live_counts().0, 0);
    }

    #[test]
    fn a_full_pool_steals_the_quietest_without_exceeding_the_cap() {
        let mut bus = SynthBus::new(48_000.0);
        let r = Recipe::parse("sine freq=440 a=0.01 hold=5 d=0.1 s=1 r=0.1").unwrap();
        for i in 0..200 {
            let far = Emitter::at([0.0, 0.0, -(1.0 + i as f32 * 0.1)], 100.0);
            bus.play_recipe(i + 1, &r, 1.0, 1.0, far, Priority::Normal, 0);
        }
        assert!(bus.live_counts().0 <= RECIPE_VOICES);
        assert!(bus.stats().steals > 0);
    }

    #[test]
    fn the_master_never_clips() {
        let mut bus = SynthBus::new(48_000.0);
        let r = Recipe::parse(builtin("explosion_big").unwrap()).unwrap();
        for i in 0..20 {
            bus.play_recipe(i + 1, &r, 1.0, 4.0, Emitter::FLAT, Priority::High, 0);
        }
        let (mut l, mut rr) = (vec![0.0; 96_000], vec![0.0; 96_000]);
        bus.render(&mut l, &mut rr);
        let peak = l.iter().chain(rr.iter()).fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(peak <= 0.9, "{peak}");
    }

    /// A drone that ignores note-offs: whatever the bus outputs after a
    /// retire is the fade alone, not the instrument's own release.
    struct Drone;
    impl Instrument for Drone {
        fn name(&self) -> &str {
            "drone"
        }
        fn sample_rate(&self) -> f32 {
            48_000.0
        }
        fn note_on(&mut self, _: u8, _: u8) {}
        fn note_off(&mut self, _: u8) {}
        fn all_notes_off(&mut self) {}
        fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
            for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
                let x = if i % 64 < 32 { 0.02 } else { -0.02 };
                *a = x;
                *b = x;
            }
        }
    }

    #[test]
    fn commands_play_as_the_methods_do() {
        let mut a = SynthBus::new(48_000.0);
        let mut b = SynthBus::new(48_000.0);
        let listener = Listener3 { pos: [2.0, 1.0, 3.0], ..Listener3::default() };
        let room = RoomPreset::Room.params();
        let recipe = Recipe::parse(builtin("coin").unwrap()).unwrap();
        let emitter = Emitter::at([1.0, 0.0, -4.0], 40.0);
        let spec = crate::engine::preset("v8").unwrap();
        let engine = EngineInput { rpm: 3000.0, throttle: 0.7, load: 0.5, gear: 2, speed: 12.0 };
        let tyres = TyreInput { speed: 12.0, load: 1.0, grounded: 4, ..TyreInput::default() };
        a.set_listener(listener);
        a.set_room(room);
        a.set_group_gain(Group::Music, 0.5);
        assert!(a.play_recipe(1, &recipe, 1.1, 0.6, emitter, Priority::Normal, 0));
        a.tone(2, 220.0, Some(Wave::Sine), 0.3);
        a.tone_set(2, Some(330.0), Some(0.2));
        assert!(a.vehicle(7, &spec, engine, tyres, emitter, 0.25, 0.6));
        a.end_frame();
        assert!(a.add_instrument(3, Box::new(Drone), Emitter::FLAT, 0.8, &mut |_| panic!("no retirement")));
        a.set_instrument_group(3, Group::Music);
        a.instrument_mut(3).unwrap().note_on(60, 100);
        for cmd in [
            Command::Listener(listener),
            Command::Room(room),
            Command::GroupScript { group: Group::Music, gain: 1.0, ease: 0.3 },
            Command::GroupScript { group: Group::Fx, gain: 1.0, ease: 0.3 },
            Command::GroupDuck { group: Group::Music, to: 1.0, attack: 0.1, hold: 0.2, release: 0.1 },
            Command::Muffle { amount: 0.0, ease: 0.1 },
            Command::GroupGain { group: Group::Music, gain: 0.5 },
            Command::PlayRecipe { id: 1, recipe, pitch: 1.1, gain: 0.6, emitter, priority: Priority::Normal, key: 0 },
            Command::Tone { id: 2, freq: 220.0, wave: Some(Wave::Sine), gain: 0.3 },
            Command::ToneSet { id: 2, freq: Some(330.0), gain: Some(0.2) },
            Command::Vehicle { key: 7, spec, engine, tyres, emitter, interior: 0.25, gain: 0.6 },
            Command::EndFrame,
            Command::AddInstrument { key: 3, inst: Box::new(Drone), emitter: Emitter::FLAT, gain: 0.8 },
            Command::InstrumentGroup { key: 3, group: Group::Music },
            Command::NoteOn { key: 3, note: 60, velocity: 100 },
        ] {
            b.apply(cmd, &mut |_| panic!("no retirement"));
        }
        let (mut al, mut ar) = (vec![0.0; 4800], vec![0.0; 4800]);
        let (mut bl, mut br) = (vec![0.0; 4800], vec![0.0; 4800]);
        a.render(&mut al, &mut ar);
        b.render(&mut bl, &mut br);
        assert!(al.iter().chain(&ar).zip(bl.iter().chain(&br)).all(|(a,b)| a.to_bits() == b.to_bits()),
            "explicit unity script gains, unity duck and bypass leave both output channels bit-identical");
        // CPU timings measure separate renders, not synthesis state.
        let same_view = |a: &SynthBus, b: &SynthBus| {
            let (mut a, mut b) = (a.view(), b.view());
            a.stats.cpu_load = 0.0;
            a.stats.peak_cpu_load = 0.0;
            b.stats.cpu_load = 0.0;
            b.stats.peak_cpu_load = 0.0;
            assert_eq!(a, b);
        };
        same_view(&a, &b);
        a.fade_instrument(3, 0.05);
        a.stop_voice(1);
        a.fade_tones(|id| id == 2, 0.05);
        a.vehicle_release(7);
        for cmd in [
            Command::FadeInstrument { key: 3, secs: 0.05 },
            Command::StopVoice { id: 1 },
            Command::FadeTones { prefix: 2, shift: 0, secs: 0.05 },
            Command::VehicleRelease { key: 7 },
        ] {
            b.apply(cmd, &mut |_| panic!("no retirement"));
        }
        let (mut al, mut ar) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        let (mut bl, mut br) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        a.render(&mut al, &mut ar);
        b.render(&mut bl, &mut br);
        assert_eq!(al, bl);
        assert_eq!(ar, br);
        for bus in [&mut a, &mut b] {
            let mut reaped = 0;
            bus.reap(&mut |r| {
                assert!(matches!(r, Retired::Instrument { key: 3, .. }));
                reaped += 1;
            });
            assert_eq!(reaped, 1);
        }
        same_view(&a, &b);
    }

    #[test]
    fn a_full_bus_refuses_cuts_the_oldest_fade_and_never_grows() {
        let mut bus = SynthBus::new(48_000.0);
        let capacity = bus.instruments.capacity();
        let mut retired = Vec::new();
        for key in 1..=8 {
            assert!(bus.add_instrument(key, Box::new(Drone), Emitter::FLAT, 1.0, &mut |r| retired.push(r)));
            assert_eq!(bus.instruments.capacity(), capacity);
        }
        assert!(retired.is_empty());
        assert!(!bus.add_instrument(9, Box::new(Drone), Emitter::FLAT, 1.0, &mut |r| retired.push(r)));
        assert_eq!(retired.len(), 1);
        assert!(matches!(retired.pop(), Some(Retired::Refused { key: 9, .. })));
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0, &mut |r| retired.push(r)));
        assert_eq!(retired.len(), 1);
        assert!(matches!(retired.pop(), Some(Retired::Instrument { key: 1, .. })));
        assert_eq!(bus.instruments.capacity(), capacity);
        for key in 1..=8 {
            bus.fade_instrument(key, key as f32 * 0.1);
        }
        let (mut l, mut r) = (vec![0.0; 2400], vec![0.0; 2400]);
        bus.render(&mut l, &mut r);
        for key in 11..=18 {
            assert!(bus.add_instrument(key, Box::new(Drone), Emitter::FLAT, 1.0, &mut |r| retired.push(r)));
            assert_eq!(bus.instruments.capacity(), capacity);
        }
        assert!(retired.is_empty());
        assert_eq!(bus.instruments.len(), capacity);
        bus.fade_instrument(11, 0.1);
        let (mut l, mut r) = ([0.0; 240], [0.0; 240]);
        bus.render(&mut l, &mut r);
        assert!(bus.add_instrument(21, Box::new(Drone), Emitter::FLAT, 1.0, &mut |r| retired.push(r)));
        assert_eq!(retired.len(), 1);
        assert!(matches!(retired.pop(), Some(Retired::Instrument { key: 1, .. })));
        assert_eq!(bus.instruments.capacity(), capacity);
        assert_eq!(bus.view().instrument_count, 8);
    }

    #[test]
    fn a_retired_instrument_fades_frees_its_key_and_is_reaped() {
        let mut bus = SynthBus::new(48_000.0);
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0, &mut |_| {}));
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        bus.render(&mut l, &mut r);
        let before = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(before > 0.005 && before < 0.5, "the drone sounds, under the limiter: {before}");
        bus.fade_instrument(1, 0.2);
        assert!(bus.instrument_mut(1).is_none(), "a retiring slot is not found");
        // The same key installs again at once, beside the fading one.
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 0.0, &mut |_| {}));
        assert_eq!(bus.live_counts().3, 2);
        // The first 50 ms of the fade: no cut, and falling.
        let (mut l, mut r) = (vec![0.0; 2400], vec![0.0; 2400]);
        bus.render(&mut l, &mut r);
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        let (first, later) = (peak(&l[..240]), peak(&l[2160..]));
        assert!(first > before * 0.5, "no cut at the retire: {first} vs {before}");
        assert!(later < first, "falling: {later} after {first}");
        let mut reaped = 0;
        bus.reap(&mut |r| {
            assert!(matches!(r, Retired::Instrument { key: 1, .. }));
            reaped += 1;
        });
        assert_eq!(reaped, 0, "not reaped mid-fade");
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        bus.render(&mut l, &mut r);
        // Only the room's (slowly decaying) tail is left.
        let after = l[4800..].iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(after < before * 0.5, "the fade ended: {after} vs {before}");
        bus.reap(&mut |r| {
            assert!(matches!(r, Retired::Instrument { key: 1, .. }));
            reaped += 1;
        });
        assert_eq!(reaped, 1, "the retired one is handed back");
        assert_eq!(bus.live_counts().3, 1, "the new one stays");
    }

    #[test]
    fn a_group_volume_silences_its_group_only() {
        let peak = |bus: &mut SynthBus| {
            // Long enough for the room's tail of the previous state to die.
            let (mut l, mut r) = (vec![0.0; 144_000], vec![0.0; 144_000]);
            bus.render(&mut l, &mut r);
            l[139_200..].iter().fold(0.0f32, |a, x| a.max(x.abs()))
        };
        // Music: an instrument in the music group, muted -> near silence.
        let mut bus = SynthBus::new(48_000.0);
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0, &mut |_| {}));
        bus.set_instrument_group(1, Group::Music);
        let loud = peak(&mut bus);
        bus.set_group_gain(Group::Music, 0.0);
        let muted = peak(&mut bus);
        assert!(loud > 0.005 && muted < loud * 0.05, "music muted: {muted} vs {loud}");
        // The effects group does not touch it...
        bus.set_group_gain(Group::Music, 1.0);
        bus.set_group_gain(Group::Fx, 0.0);
        let still = peak(&mut bus);
        assert!(still > loud * 0.5, "fx volume left music alone: {still} vs {loud}");
        // ...but silences a tone.
        let mut bus = SynthBus::new(48_000.0);
        bus.tone(5, 220.0, Some(Wave::Sine), 0.5);
        let tone = peak(&mut bus);
        bus.set_group_gain(Group::Fx, 0.0);
        let quiet = peak(&mut bus);
        assert!(tone > 0.01 && quiet < tone * 0.05, "fx muted: {quiet} vs {tone}");

        // Below the master compressor threshold, a tone measures the actual group product.
        let frames = |bus: &mut SynthBus, n| {
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            bus.render(&mut l, &mut r);
            l
        };
        let amplitude = |x: &[f32]| x.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        let mut bus = SynthBus::new(48_000.0);
        bus.tone(1, 1000.0, Some(Wave::Sine), 0.05);
        let full = amplitude(&frames(&mut bus, 24_000)[23_000..]);
        bus.set_group_gain(Group::Fx, 0.5);
        frames(&mut bus, 36_000);
        bus.set_group_script(Group::Fx, 0.5, 0.3);
        let glide = frames(&mut bus, 14_400 + 384);
        let ratio = |x: &[f32]| amplitude(x) / full;
        assert!((ratio(&glide[7200..7296]) - 0.375).abs() < 0.015, "script gain is halfway through its 0.3 s ease");
        assert!((ratio(&glide[14_600..]) - 0.25).abs() < 0.01, "script and player gains multiply");
        bus.duck_group(Group::Fx, 0.55, 0.005, 0.035, 0.01);
        let held = frames(&mut bus, 960);
        assert!((ratio(&held[720..]) * 4.0 - 0.55).abs() < 0.03, "duck holds 0.55 of the scaled group");
        bus.duck_group(Group::Fx, 0.3, 0.005, 0.015, 0.01);
        bus.duck_group(Group::Fx, 0.8, 0.005, 0.05, 0.01);
        let overlap = frames(&mut bus, 3840);
        assert!((ratio(&overlap[2160..2400]) * 4.0 - 0.3).abs() < 0.03, "overlap holds the deeper target through the later hold");
        assert!((ratio(&overlap[3600..]) * 4.0 - 1.0).abs() < 0.03, "release restores the player's scaled level");
    }

    #[test]
    fn muffle_attenuates_the_effects_band_and_spares_unmixed_recipes() {
        for (freq, minimum, maximum) in [(4000.0, -100.0, -20.0), (200.0, -3.0, 0.1)] {
            let measured = |amount| {
                let mut bus = SynthBus::new(48_000.0);
                bus.set_muffle(amount, 0.01);
                bus.tone(1, freq, Some(Wave::Sine), 0.05);
                let mut l = [0.0; 4800];
                bus.render(&mut l, &mut [0.0; 4800]);
                l[2400..].iter().map(|x| x*x).sum::<f32>()
            };
            let db = 10.0 * (measured(1.0) / measured(0.0)).log10();
            assert!(db > minimum && db < maximum, "effects tone at {freq} Hz has {db} dB change");
        }
        // Refer the independently filtered dry tone through the same master, so
        // the comparison includes its look-ahead without assuming a delay length.
        let mut dry = SynthBus::new(48_000.0);
        let mut wet = SynthBus::new(48_000.0);
        let mut reference = SynthBus::new(48_000.0);
        reference.external_send = 0.0;
        for bus in [&mut dry, &mut wet] { bus.tone(1, 4000.0, Some(Wave::Sine), 0.05); }
        let mut history = [0.0_f64; 2];
        let closed = (-std::f64::consts::TAU * 600.0 / 48_000.0).exp();
        let mut error = 0.0_f32;
        for (stage, amount) in [0.0, 1.0, 0.0].into_iter().enumerate() {
            wet.set_muffle(amount, 0.005);
            let mut at = 0;
            for n in [157, 131, 211] {
                let (mut l, mut r) = ([0.0; BLOCK], [0.0; BLOCK]);
                dry.render(&mut l[..n], &mut r[..n]);
                for i in 0..n {
                    let x = dry.dry_l[i] as f64;
                    let t = ((at + i + 1) as f64 / 240.0).min(1.0);
                    let pole = match stage { 1 => t * closed, 2 => (1.0 - t) * closed, _ => 0.0 };
                    if stage == 0 {
                        l[i] = x as f32;
                    } else {
                        history[0] = x + pole * (history[0] - x);
                        history[1] = history[0] + pole * (history[1] - history[0]);
                        l[i] = history[1] as f32;
                    }
                    r[i] = l[i];
                }
                reference.render(&mut l[..n], &mut r[..n]);
                let (mut wl, mut wr) = ([0.0; BLOCK], [0.0; BLOCK]);
                wet.render(&mut wl[..n], &mut wr[..n]);
                for i in 0..n { error = error.max((wl[i] - l[i]).abs()).max((wr[i] - r[i]).abs()); }
                at += n;
            }
        }
        assert!(error < 2.0e-5, "bus muffle transitions match the glided reference within 2e-5 full scale: {error}");

        let render = |controlled, music| {
            let mut bus = SynthBus::new(48_000.0);
            if music {
                bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0, &mut |_| {});
                bus.set_instrument_group(1, Group::Music);
            } else {
                bus.apply(Command::PlayUnmixedRecipe {
                    id: 1, recipe: Recipe::parse(builtin("coin").unwrap()).unwrap(), pitch: 1.0, gain: 0.25,
                    emitter: Emitter::FLAT, priority: Priority::Normal, key: 0,
                }, &mut |_| {});
            }
            if controlled {
                bus.duck_group(Group::Fx, 0.0, 0.0, 0.5, 0.0);
                bus.set_muffle(1.0, 0.0);
            }
            let mut l = [0.0; 4096];
            bus.render(&mut l, &mut [0.0; 4096]);
            l.map(f32::to_bits)
        };
        assert_eq!(render(true, false), render(false, false), "a UI recipe and its room send bypass the effects controls");
        assert_eq!(render(true, true), render(false, true), "a music instrument and its room send bypass the effects controls");
    }

    #[test]
    fn faded_tones_release_only_the_owned_ones() {
        let mut bus = SynthBus::new(48_000.0);
        bus.tone(1 << 56 | 3, 220.0, Some(Wave::Sine), 0.5);
        bus.tone(2 << 56 | 3, 330.0, Some(Wave::Sine), 0.5);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        bus.render(&mut l, &mut r);
        bus.fade_tones(|id| id >> 56 == 1, 0.2);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        bus.render(&mut l, &mut r);
        assert_eq!(bus.live_counts().2, 2, "100 ms into a 200 ms fade");
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        bus.render(&mut l, &mut r);
        assert_eq!(bus.live_counts().2, 1, "only the owned tone ended");
    }

    /// An undriven vehicle fades out; its view keeps one incarnation while it is driven and gets
    /// a new one when a drive revives it from stopping.
    #[test]
    fn an_undriven_vehicle_fades_out() {
        let mut bus = SynthBus::new(48_000.0);
        let spec = crate::engine::preset("v8").unwrap();
        let serial = |bus: &SynthBus| bus.view().vehicles[..bus.view().vehicle_count].iter().find(|v| v.key == 7).map(|v| v.serial);
        bus.vehicle(7, &spec, EngineInput::default(), TyreInput::default(), Emitter::FLAT, 0.0, 1.0);
        let first = serial(&bus);
        bus.vehicle(7, &spec, EngineInput::default(), TyreInput::default(), Emitter::FLAT, 0.0, 1.0);
        assert_eq!(serial(&bus), first, "driving it again is the same incarnation");
        bus.vehicle_release(7);
        bus.vehicle(7, &spec, EngineInput::default(), TyreInput::default(), Emitter::FLAT, 0.0, 1.0);
        assert!(first.is_some() && serial(&bus) != first, "a drive while stopping starts a new incarnation");
        bus.end_frame();
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        bus.render(&mut l, &mut r);
        assert_eq!(bus.live_counts().1, 1);
        for _ in 0..STALE_FRAMES {
            bus.end_frame(); // not driven any more
        }
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        bus.render(&mut l, &mut r);
        assert_eq!(bus.live_counts().1, 0);
    }
}

/// Seconds on a monotonic clock for the bus's CPU accounting, where the
/// platform has one std can read; on the web (wasm32, no std clock) the
/// bus renders without measuring itself, so it never sheds voices there.
#[cfg(not(target_arch = "wasm32"))]
fn cpu_seconds() -> Option<f64> {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    Some(START.get_or_init(Instant::now).elapsed().as_secs_f64())
}

#[cfg(target_arch = "wasm32")]
fn cpu_seconds() -> Option<f64> {
    None
}
