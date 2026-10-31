//! The game mixer: fixed voice pools for recipes, vehicles, sustained tones
//! and instruments, each voice placed in 3D by its own [`Spatial`], a shared
//! room reverb, and a master compressor + look-ahead limiter.
//!
//! Threading: the bus is one value behind the host's lock. The control side
//! (UI thread) calls the `play_*`/`set_*` methods; the audio thread calls
//! [`SynthBus::render`]. Neither path allocates once the bus is built: pools
//! are sized at construction, instruments arrive already built, and anything
//! that owns heap memory is handed BACK to the caller when replaced so it is
//! dropped on the control side, never in the callback.
//!
//! Voice budget: when a pool is full the quietest voice (placement gain ×
//! its own level) yields — faded over ~3 ms, never hard-cut — and only to a
//! newcomer at least as important. The render measures its own CPU cost and,
//! under sustained overload, sheds the quietest recipe voices first.

use crate::dsp::{finite, Osc, Smooth, Wave};
use crate::dynamics::{Compressor, Ducker, Limiter};
use crate::engine::{EngineInput, EngineSpec};
use crate::instrument::Instrument;
use crate::recipe::{Recipe, RecipeVoice};
use crate::reverb::{Reverb, RoomParams, RoomPreset};
use crate::spatial::{place, Distance, Listener3, Placement3, Spatial, V3};
use crate::vehicle::{TyreInput, VehicleSound};

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

struct RecipeSlot {
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
    /// A retiring slot's gain ramp, 1 -> 0; no lookup finds it. The control side reaps it at 0
    /// ([`SynthBus::reap_instruments`]), so the callback never drops it.
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
    /// Reverb send for the external (sampled) input.
    pub external_send: f32,
    mono: [f32; BLOCK],
    dry_l: [f32; BLOCK],
    dry_r: [f32; BLOCK],
    send: [f32; BLOCK],
    inst_l: [f32; BLOCK],
    inst_r: [f32; BLOCK],
    next_id: u64,
    seed: u32,
    frame: u64,
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
            external_send: 0.12,
            mono: [0.0; BLOCK],
            dry_l: [0.0; BLOCK],
            dry_r: [0.0; BLOCK],
            send: [0.0; BLOCK],
            inst_l: [0.0; BLOCK],
            inst_r: [0.0; BLOCK],
            next_id: 1,
            seed: 0x1234,
            frame: 0,
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
    /// Returns the voice id, or `None` when every slot outranks it or it is
    /// out of earshot.
    pub fn play_recipe(
        &mut self,
        recipe: &Recipe,
        pitch: f32,
        gain: f32,
        emitter: Emitter,
        priority: Priority,
        key: u64,
    ) -> Option<u64> {
        let place = emitter.placement(&self.listener);
        let loudness = place.gain * gain;
        if emitter.positional && place.gain <= 0.0 {
            return None;
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
                    _ => return None,
                }
            }
        };
        let seed = self.next_seed();
        let id = self.next_id;
        self.next_id += 1;
        let slot = &mut self.recipes[index];
        slot.active = true;
        slot.id = id;
        slot.key = key;
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
        Some(id)
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
            Some(i) => i,
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
                let slot = &mut self.vehicles[index];
                slot.active = true;
                slot.key = key;
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

    /// Add an instrument under `key`. Replacing an existing key hands the
    /// old instrument back (drop it outside the lock). When every slot is
    /// taken the new one is handed back in `Err`.
    pub fn add_instrument(
        &mut self,
        key: u64,
        inst: Box<dyn Instrument>,
        emitter: Emitter,
        gain: f32,
    ) -> Result<Option<Box<dyn Instrument>>, Box<dyn Instrument>> {
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
            return Ok(Some(old.inst));
        }
        // Retiring slots fade beside the new ones and do not count.
        if self.instruments.iter().filter(|s| s.live()).count() >= INSTRUMENT_SLOTS {
            return Err(inst);
        }
        self.instruments.push(make(inst));
        Ok(None)
    }

    /// Remove an instrument, handing it back to drop (or to keep).
    pub fn remove_instrument(&mut self, key: u64) -> Option<Box<dyn Instrument>> {
        let i = self.instruments.iter().position(|s| s.live() && s.key == key)?;
        Some(self.instruments.swap_remove(i).inst)
    }

    /// Retire an instrument without a click: held notes release and its
    /// output fades to silence over `secs`. From now on no lookup finds it,
    /// so the same key can be installed again at once;
    /// [`SynthBus::reap_instruments`] frees it once the fade has run out.
    pub fn fade_instrument(&mut self, key: u64, secs: f32) {
        let rate = self.rate;
        if let Some(s) = self.instruments.iter_mut().find(|s| s.live() && s.key == key) {
            s.inst.all_notes_off();
            s.fade_step = 1.0 / (finite(secs, 0.25).max(0.005) * rate);
        }
    }

    /// Hand back every retired instrument whose fade has finished, to be
    /// dropped by the caller outside its lock.
    pub fn reap_instruments(&mut self) -> Vec<Box<dyn Instrument>> {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.instruments.len() {
            let s = &self.instruments[i];
            if !s.live() && s.fade <= 0.0 {
                done.push(self.instruments.swap_remove(i).inst);
            } else {
                i += 1;
            }
        }
        done
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
    pub fn vehicle_loops(&mut self, key: u64, loops: Vec<crate::vehicle::EngineLoop>, mix: f32) -> Vec<crate::vehicle::EngineLoop> {
        match self.vehicles.iter_mut().find(|s| s.active && s.key == key) {
            Some(s) => s.sound.set_loops(loops, mix),
            None => loops,
        }
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
            self.render_block(&mut out_l[at..at + len], &mut out_r[at..at + len], &mut peak);
            at += len;
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
        for i in 0..len {
            let g = self.fx_gain.next(group_coef);
            dry_l[i] = dry_l[i] * g + out_l[i];
            dry_r[i] = dry_r[i] * g + out_r[i];
            send[i] = send[i] * g + (out_l[i] + out_r[i]) * 0.5 * ext_send;
        }
        // One ramp per group for this block, shared by its instruments.
        let music = &mut self.mono[..len];
        for m in music.iter_mut() {
            *m = self.music_gain.next(group_coef);
        }
        let fx_now = self.fx_gain.value;

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
            for i in 0..len {
                // A retiring slot ramps to silence and stays there until
                // the control side reaps it.
                let f = s.fade;
                if step > 0.0 {
                    s.fade = (s.fade - step).max(0.0);
                }
                let f = f * if s.group == Group::Music { music[i] } else { fx_now };
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
        assert!(bus.play_recipe(&r, 1.0, 1.0, Emitter::FLAT, Priority::Normal, 0).is_some());
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
            bus.play_recipe(&r, 1.0, 1.0, far, Priority::Normal, 0);
        }
        assert!(bus.live_counts().0 <= RECIPE_VOICES);
        assert!(bus.stats().steals > 0);
    }

    #[test]
    fn the_master_never_clips() {
        let mut bus = SynthBus::new(48_000.0);
        let r = Recipe::parse(builtin("explosion_big").unwrap()).unwrap();
        for _ in 0..20 {
            bus.play_recipe(&r, 1.0, 4.0, Emitter::FLAT, Priority::High, 0);
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
    fn a_retired_instrument_fades_frees_its_key_and_is_reaped() {
        let mut bus = SynthBus::new(48_000.0);
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0).is_ok());
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        bus.render(&mut l, &mut r);
        let before = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(before > 0.005 && before < 0.5, "the drone sounds, under the limiter: {before}");
        bus.fade_instrument(1, 0.2);
        assert!(bus.instrument_mut(1).is_none(), "a retiring slot is not found");
        // The same key installs again at once, beside the fading one.
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 0.0).is_ok());
        assert_eq!(bus.live_counts().3, 2);
        // The first 50 ms of the fade: no cut, and falling.
        let (mut l, mut r) = (vec![0.0; 2400], vec![0.0; 2400]);
        bus.render(&mut l, &mut r);
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        let (first, later) = (peak(&l[..240]), peak(&l[2160..]));
        assert!(first > before * 0.5, "no cut at the retire: {first} vs {before}");
        assert!(later < first, "falling: {later} after {first}");
        assert!(bus.reap_instruments().is_empty(), "not reaped mid-fade");
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        bus.render(&mut l, &mut r);
        // Only the room's (slowly decaying) tail is left.
        let after = l[4800..].iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(after < before * 0.5, "the fade ended: {after} vs {before}");
        assert_eq!(bus.reap_instruments().len(), 1, "the retired one is handed back");
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
        assert!(bus.add_instrument(1, Box::new(Drone), Emitter::FLAT, 1.0).is_ok());
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

    #[test]
    fn an_undriven_vehicle_fades_out() {
        let mut bus = SynthBus::new(48_000.0);
        let spec = crate::engine::preset("v8").unwrap();
        bus.vehicle(7, &spec, EngineInput::default(), TyreInput::default(), Emitter::FLAT, 0.0, 1.0);
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
