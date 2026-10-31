//! Adapters from Makepad's instruments to the shared [`Instrument`] trait,
//! and a factory that builds any of them by kind.
//!
//! - [`PianoInstrument`]: the physically modelled grand
//!   (`makepad-piano-model`). Presets: "Concert Grand" (the default).
//! - [`PhysDrums`]: the modelled drum kit (`makepad-drumkit-phys`), no files.
//! - [`SampledDrums`]: the Salamander sample kit (`makepad-drumkit`), from a
//!   directory of the kit's WAVs.
//! - [`SoundFontInstrument`]: an SF2 preset (`makepad-soundfont`), or the
//!   crate's procedural piano when there is no file.
//! - Ironfish ([`crate::ironfish::Ironfish`]) with named presets, and the
//!   recipe poly-synth ([`crate::poly::PolySynth`]).
//!
//! Drum kits take General MIDI percussion notes (kick 36, snare 38, closed
//! hat 42, ...; see [`crate::instrument::drum_note`]); notes the kits lack map to
//! their nearest piece.
//!
//! Construction may be slow (the piano builds 88 modal key designs, a
//! SoundFont is parsed and indexed) and allocates: build off the audio
//! thread, then hand the boxed instrument to the renderer.

use crate::instrument::{Control, Instrument};
use crate::ironfish::{FilterKind, Ironfish, IronfishPatch, OscillatorKind};
use crate::recipe::Recipe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use makepad_piano_model::{Piano, PianoEvent, TimedEvent as PianoTimed, PIANO_PRESETS};
use makepad_soundfont::{
    piano_fallback, NoSamples, Sampler, SamplerEvent, SoundFont, TimedEvent as SfTimed, VoiceParameters,
};

const MAX_PENDING: usize = 256;
const CHUNK: usize = 256;

// ─────────────────────────────── piano ───────────────────────────────

pub struct PianoInstrument {
    piano: Piano,
    events: [PianoTimed; MAX_PENDING],
    count: usize,
}

impl PianoInstrument {
    /// Preset names accepted by [`PianoInstrument::new`] (case-insensitive).
    pub fn preset_names() -> impl Iterator<Item = &'static str> {
        PIANO_PRESETS.iter().map(|p| p.name)
    }

    pub fn has_preset(name: &str) -> bool {
        PIANO_PRESETS.iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The modelled grand; an unknown or missing preset name is the default.
    pub fn new(rate: f32, preset_name: Option<&str>) -> Self {
        let preset = preset_name
            .and_then(|name| PIANO_PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name)))
            .or_else(|| PIANO_PRESETS.iter().find(|p| p.is_default));
        let piano = match preset {
            Some(p) => Piano::new_with_preset(rate, p),
            None => Piano::new(rate),
        };
        PianoInstrument {
            piano,
            events: [PianoTimed { offset: 0, event: PianoEvent::AllSoundOff }; MAX_PENDING],
            count: 0,
        }
    }

    fn push(&mut self, event: PianoEvent) {
        // A full queue drops the newest event: 256 events inside one audio
        // block is a stuck producer, not music.
        if self.count < MAX_PENDING {
            self.events[self.count] = PianoTimed { offset: 0, event };
            self.count += 1;
        }
    }
}

impl Instrument for PianoInstrument {
    fn name(&self) -> &str {
        "piano"
    }

    fn sample_rate(&self) -> f32 {
        makepad_piano_model::Instrument::sample_rate(&self.piano)
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.push(PianoEvent::NoteOn { key: note.min(127), velocity: velocity.min(127) });
    }

    fn note_off(&mut self, note: u8) {
        self.push(PianoEvent::NoteOff { key: note.min(127) });
    }

    fn control(&mut self, control: Control, value: f32) {
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        match control {
            Control::Sustain => self.push(PianoEvent::Sustain { value }),
            Control::Soft => self.push(PianoEvent::SoftPedal { on: value >= 0.5 }),
            Control::Sostenuto => self.push(PianoEvent::Sostenuto { on: value >= 0.5 }),
            Control::Volume => self.piano.set_master_gain(value * 2.0),
            _ => {}
        }
    }

    fn all_notes_off(&mut self) {
        // Released, not cut: the dampers fall and the room rings out.
        self.push(PianoEvent::Sustain { value: 0.0 });
        for key in 21..=108u8 {
            self.push(PianoEvent::NoteOff { key });
        }
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        if len == 0 {
            return;
        }
        out_l[..len].fill(0.0);
        out_r[..len].fill(0.0);
        self.piano.process(&self.events[..self.count], &mut out_l[..len], &mut out_r[..len]);
        self.count = 0;
    }
}

// ─────────────────────────────── drums ───────────────────────────────

/// A GM percussion note on the nearest piece both kits have (they share the
/// same 14 pieces and note numbers).
fn kit_note(note: u8) -> Option<u8> {
    Some(match note {
        35 | 36 => 36,
        37 => 37,
        38 | 40 => 38,
        39 => 39,
        41 | 43 => 41,
        42 => 42,
        44 => 44,
        45 | 47 => 45,
        48 => 48,
        50 => 50,
        46 => 46,
        49 | 52 | 55 | 57 => 49,
        51 | 59 => 51,
        53 => 53,
        54 | 56 | 58 => 37,
        _ => return None,
    })
}

pub struct PhysDrums {
    kit: makepad_drumkit_phys::DrumKit,
    scratch: [[f32; 2]; CHUNK],
}

impl PhysDrums {
    pub fn new(rate: f32) -> Self {
        PhysDrums { kit: makepad_drumkit_phys::DrumKit::new(rate), scratch: [[0.0; 2]; CHUNK] }
    }
}

impl Instrument for PhysDrums {
    fn name(&self) -> &str {
        "drums"
    }

    fn sample_rate(&self) -> f32 {
        self.kit.sample_rate()
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if let Some(voice) = kit_note(note).and_then(|n| makepad_drumkit_phys::DrumVoice::try_from(n).ok()) {
            self.kit.trigger(voice, velocity.min(127) as f32 / 127.0);
        }
    }

    fn note_off(&mut self, _note: u8) {}

    fn all_notes_off(&mut self) {
        self.kit.all_off();
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        let mut at = 0;
        while at < len {
            let n = (len - at).min(CHUNK);
            let s = &mut self.scratch[..n];
            s.fill([0.0; 2]);
            self.kit.process(s);
            for (i, [l, r]) in s.iter().enumerate() {
                out_l[at + i] = *l;
                out_r[at + i] = *r;
            }
            at += n;
        }
    }

    fn is_active(&self) -> bool {
        self.kit.active()
    }
}

pub struct SampledDrums {
    kit: makepad_drumkit::DrumKit,
    scratch: [[f32; 2]; CHUNK],
}

impl SampledDrums {
    /// Where the Salamander subset is looked for: `MAKEPAD_DRUMKIT_DIR`, else
    /// `local/score-corpus/drums/OH` in the Makepad checkout (the directory
    /// the drumkit crate's own tests and demo use). `None` when absent.
    pub fn default_dir() -> Option<PathBuf> {
        let dir = std::env::var_os("MAKEPAD_DRUMKIT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/score-corpus/drums/OH"));
        dir.is_dir().then_some(dir)
    }

    pub fn load(rate: f32, dir: &Path) -> Result<Self, String> {
        let bank = makepad_drumkit::SampleBank::load(dir)?;
        let mut kit = makepad_drumkit::DrumKit::new(rate);
        kit.set_bank(Arc::new(bank));
        Ok(SampledDrums { kit, scratch: [[0.0; 2]; CHUNK] })
    }
}

impl Instrument for SampledDrums {
    fn name(&self) -> &str {
        "drums_sampled"
    }

    fn sample_rate(&self) -> f32 {
        self.kit.sample_rate()
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if let Some(voice) = kit_note(note).and_then(|n| makepad_drumkit::DrumVoice::try_from(n).ok()) {
            self.kit.trigger(voice, velocity.min(127) as f32 / 127.0);
        }
    }

    fn note_off(&mut self, _note: u8) {}

    fn all_notes_off(&mut self) {
        self.kit.all_off();
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        let mut at = 0;
        while at < len {
            let n = (len - at).min(CHUNK);
            let s = &mut self.scratch[..n];
            s.fill([0.0; 2]);
            self.kit.process(s);
            for (i, [l, r]) in s.iter().enumerate() {
                out_l[at + i] = *l;
                out_r[at + i] = *r;
            }
            at += n;
        }
    }

    fn is_active(&self) -> bool {
        self.kit.active()
    }
}

// ───────────────────────────── soundfont ─────────────────────────────

const SF_VOICES: usize = 32;
const SF_LAYERS: usize = 8;
const SF_ZONES: usize = 4;

type ZoneTable = Box<[[Option<VoiceParameters>; SF_ZONES]]>;

/// An SF2 preset (or the procedural piano). Zone selection is resolved per
/// key and per velocity eighth at construction, so a note-on is a table
/// lookup, never the allocating `select`.
pub struct SoundFontInstrument {
    font: Option<Arc<SoundFont>>,
    table: ZoneTable,
    sampler: Sampler<SF_VOICES>,
    events: [SfTimed; MAX_PENDING],
    count: usize,
    note_ids: [u32; 128],
    next_id: u32,
    sustain: bool,
    sustained: [bool; 128],
    rate: f32,
}

impl SoundFontInstrument {
    fn with(rate: f32, font: Option<Arc<SoundFont>>, table: ZoneTable) -> Self {
        SoundFontInstrument {
            font,
            table,
            sampler: Sampler::new(rate),
            events: [SfTimed { offset: 0, event: SamplerEvent::AllNotesOff }; MAX_PENDING],
            count: 0,
            note_ids: [0; 128],
            next_id: 1,
            sustain: false,
            sustained: [false; 128],
            rate,
        }
    }

    pub fn from_sf2_bytes(rate: f32, bytes: &[u8], bank: u16, program: u8) -> Result<Self, String> {
        Self::from_font(rate, Arc::new(parse_font(bytes)?), bank, program)
    }

    /// One preset of an already parsed font: the programs of one file share
    /// its decoded samples.
    pub fn from_font(rate: f32, font: Arc<SoundFont>, bank: u16, program: u8) -> Result<Self, String> {
        let mut table: Vec<[Option<VoiceParameters>; SF_ZONES]> = vec![[None; SF_ZONES]; 128 * SF_LAYERS];
        let mut any = false;
        for key in 0..128u8 {
            for layer in 0..SF_LAYERS {
                let vel = (layer * 16 + 8).min(127) as u8;
                let zones = font.select_bank(bank, program as u16, key, vel);
                for (slot, zone) in table[key as usize * SF_LAYERS + layer].iter_mut().zip(zones) {
                    *slot = Some(zone);
                    any = true;
                }
            }
        }
        if !any {
            return Err(format!("SoundFont has no preset bank {bank} program {program}"));
        }
        Ok(Self::with(rate, Some(font), table.into_boxed_slice()))
    }

    /// No file: the soundfont crate's procedural piano.
    pub fn fallback_piano(rate: f32) -> Self {
        Self::with(rate, None, Vec::new().into_boxed_slice())
    }

    fn push(&mut self, event: SamplerEvent) {
        if self.count < MAX_PENDING {
            self.events[self.count] = SfTimed { offset: 0, event };
            self.count += 1;
        }
    }

    fn release(&mut self, note: u8) {
        let id = self.note_ids[note as usize];
        if id != 0 {
            self.note_ids[note as usize] = 0;
            self.push(SamplerEvent::NoteOff { note_id: id });
        }
    }
}

impl Instrument for SoundFontInstrument {
    fn name(&self) -> &str {
        "soundfont"
    }

    fn sample_rate(&self) -> f32 {
        self.rate
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        let note = note.min(127);
        if velocity == 0 {
            return self.note_off(note);
        }
        self.release(note);
        self.sustained[note as usize] = false;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.note_ids[note as usize] = id;
        let velocity = velocity.min(127);
        if self.font.is_none() {
            self.push(SamplerEvent::NoteOn { note_id: id, parameters: piano_fallback(note, velocity) });
            return;
        }
        let layer = (velocity as usize / 16).min(SF_LAYERS - 1);
        let row = note as usize * SF_LAYERS + layer;
        // The table's gain is its layer's velocity; the note's own sets it.
        let table_velocity = (layer * 16 + 8).min(127) as u8;
        let rescale = makepad_soundfont::velocity_gain(velocity) / makepad_soundfont::velocity_gain(table_velocity);
        for zone in self.table[row] {
            if let Some(mut parameters) = zone {
                parameters.key = note;
                parameters.velocity = velocity;
                parameters.gain *= rescale;
                self.push(SamplerEvent::NoteOn { note_id: id, parameters });
            }
        }
    }

    fn note_off(&mut self, note: u8) {
        let note = note.min(127);
        if self.sustain {
            self.sustained[note as usize] = true;
        } else {
            self.release(note);
        }
    }

    fn control(&mut self, control: Control, value: f32) {
        if control == Control::Sustain {
            self.sustain = value.is_finite() && value >= 0.5;
            if !self.sustain {
                for note in 0..128u8 {
                    if std::mem::take(&mut self.sustained[note as usize]) {
                        self.release(note);
                    }
                }
            }
        }
    }

    fn all_notes_off(&mut self) {
        self.sustain = false;
        self.sustained = [false; 128];
        self.note_ids = [0; 128];
        self.push(SamplerEvent::AllNotesOff);
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        if len == 0 {
            return;
        }
        let events = &self.events[..self.count];
        match &self.font {
            Some(font) => self.sampler.render(&**font, events, &mut out_l[..len], &mut out_r[..len]),
            None => self.sampler.render(&NoSamples, events, &mut out_l[..len], &mut out_r[..len]),
        };
        self.count = 0;
    }

    fn is_active(&self) -> bool {
        self.count > 0 || self.sampler.active_voice_count() > 0
    }
}

/// Parse an SF2. A General MIDI font holds ~74 M sample points, past the
/// parser's default ceiling for untrusted files, so the ceiling is raised
/// to what a full GM set needs.
fn parse_font(bytes: &[u8]) -> Result<SoundFont, String> {
    let limits = makepad_soundfont::ParseLimits { max_sample_points: 256 << 20, ..Default::default() };
    makepad_soundfont::parse_sf2_with_limits(bytes, limits).map_err(|e| format!("SoundFont: {e}"))
}

/// The parsed font of `bytes`, shared by every instrument built from the
/// same bytes while one of them lives: a General MIDI score plays several
/// programs of one file, and each would otherwise decode its own ~300 MB of
/// samples. The `Weak` to the bytes keeps their allocation from being
/// reused while the entry stands, so pointer equality is identity.
fn shared_font(bytes: &Arc<[u8]>) -> Result<Arc<SoundFont>, String> {
    static FONTS: Mutex<Vec<(Weak<[u8]>, Weak<SoundFont>)>> = Mutex::new(Vec::new());
    let mut fonts = FONTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    fonts.retain(|(bytes, font)| bytes.strong_count() > 0 && font.strong_count() > 0);
    let key = Arc::downgrade(bytes);
    if let Some(font) = fonts.iter().find(|(bytes, _)| Weak::ptr_eq(bytes, &key)).and_then(|(_, font)| font.upgrade()) {
        return Ok(font);
    }
    let font = Arc::new(parse_font(bytes)?);
    fonts.push((key, Arc::downgrade(&font)));
    Ok(font)
}

// ───────────────────────────── ironfish ─────────────────────────────

/// Named Ironfish programs.
pub const IRONFISH_PRESETS: &[&str] =
    &["classic", "pad", "acid", "bass", "bells", "crush", "supersaw", "swell", "pluck"];

pub fn ironfish_patch(name: &str) -> Option<IronfishPatch> {
    let index = match name.to_ascii_lowercase().as_str() {
        "classic" | "default" => 0,
        "pad" | "warm" => 1,
        "acid" | "hyper" => 2,
        "bass" => 3,
        "bells" | "harmonic" => 4,
        "crush" | "lofi" => 5,
        "supersaw" | "trance" | "lead" => 6,
        "swell" | "ambient" => 7,
        "pluck" => {
            let mut p = IronfishPatch::default();
            p.osc1.kind = OscillatorKind::DpwSawPulse;
            p.osc2.kind = OscillatorKind::SuperSaw;
            p.amp.attack = 0.0;
            p.amp.decay = 0.3;
            p.amp.sustain = 0.0;
            p.amp.release = 0.25;
            p.filter.kind = FilterKind::LowPass;
            p.filter.cutoff = 0.3;
            p.filter.envelope_amount = 0.6;
            p.modulation.decay = 0.2;
            p.delay.send = 0.25;
            return Some(p.sanitise());
        }
        _ => return None,
    };
    Some(IronfishPatch::preset(index))
}

// ───────────────────────────── factory ─────────────────────────────

#[derive(Clone)]
pub enum InstrumentKind {
    Piano { preset: Option<String> },
    Drums,
    SampledDrums { dir: PathBuf },
    SoundFont { bytes: Arc<[u8]>, bank: u16, program: u8 },
    Ironfish { preset: Option<String> },
    Synth { patch: Recipe },
}

impl std::fmt::Debug for InstrumentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstrumentKind::Piano { preset } => write!(f, "Piano({preset:?})"),
            InstrumentKind::Drums => write!(f, "Drums"),
            InstrumentKind::SampledDrums { dir } => write!(f, "SampledDrums({})", dir.display()),
            InstrumentKind::SoundFont { bytes, bank, program } => {
                write!(f, "SoundFont({} bytes, bank {bank}, program {program})", bytes.len())
            }
            InstrumentKind::Ironfish { preset } => write!(f, "Ironfish({preset:?})"),
            InstrumentKind::Synth { .. } => write!(f, "Synth"),
        }
    }
}

/// A kind by the names a script uses. `preset` is the piano preset, the
/// Ironfish program, or for `synth` a poly patch name or recipe text.
/// SoundFonts need bytes, so they are built with [`InstrumentKind::SoundFont`]
/// directly; `None` for anything unknown.
pub fn parse_kind(name: &str, preset: Option<&str>) -> Option<InstrumentKind> {
    let name = name.trim().to_ascii_lowercase();
    let preset_string = preset.map(str::to_string);
    Some(match name.as_str() {
        "piano" | "grand" | "grand_piano" => InstrumentKind::Piano { preset: preset_string },
        "drums" | "drumkit" | "kit" => InstrumentKind::Drums,
        "drums_sampled" | "salamander" => InstrumentKind::SampledDrums { dir: SampledDrums::default_dir()? },
        "ironfish" => InstrumentKind::Ironfish { preset: preset_string },
        "synth" => {
            let preset = preset.unwrap_or("lead");
            let text = crate::poly::patch(preset).unwrap_or(preset);
            InstrumentKind::Synth { patch: Recipe::parse(text).ok()? }
        }
        other => InstrumentKind::Synth { patch: Recipe::parse(crate::poly::patch(other)?).ok()? },
    })
}

/// Build an instrument. Slow for the piano and SoundFonts: never on the
/// audio thread.
pub fn build(kind: &InstrumentKind, rate: f32) -> Result<Box<dyn Instrument>, String> {
    Ok(match kind {
        InstrumentKind::Piano { preset } => {
            if let Some(p) = preset.as_deref() {
                if !PianoInstrument::has_preset(p) {
                    let names: Vec<&str> = PianoInstrument::preset_names().collect();
                    return Err(format!("unknown piano preset '{p}' (have: {})", names.join(", ")));
                }
            }
            Box::new(PianoInstrument::new(rate, preset.as_deref()))
        }
        InstrumentKind::Drums => Box::new(PhysDrums::new(rate)),
        InstrumentKind::SampledDrums { dir } => Box::new(SampledDrums::load(rate, dir)?),
        InstrumentKind::SoundFont { bytes, bank, program } => {
            Box::new(SoundFontInstrument::from_font(rate, shared_font(bytes)?, *bank, *program)?)
        }
        InstrumentKind::Ironfish { preset } => {
            let patch = match preset.as_deref() {
                Some(p) => ironfish_patch(p).ok_or_else(|| {
                    format!("unknown ironfish preset '{p}' (have: {})", IRONFISH_PRESETS.join(", "))
                })?,
                None => IronfishPatch::default(),
            };
            Box::new(Ironfish::new(rate, patch))
        }
        InstrumentKind::Synth { patch } => Box::new(crate::poly::PolySynth::new(rate, *patch)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(inst: &mut dyn Instrument, note: u8, frames: usize) -> (f32, bool) {
        inst.note_on(note, 110);
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            inst.render(cl, cr);
        }
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, x| a.max(x.abs()));
        (peak, l.iter().chain(r.iter()).all(|x| x.is_finite()))
    }

    fn rms(inst: &mut dyn Instrument, frames: usize) -> f32 {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            inst.render(cl, cr);
        }
        (l.iter().map(|x| x * x).sum::<f32>() / frames as f32).sqrt()
    }

    #[test]
    fn every_buildable_kind_is_audible_and_finite() {
        for (name, preset) in [
            ("drums", None),
            ("ironfish", Some("supersaw")),
            ("ironfish", Some("pluck")),
            ("synth", Some("epiano")),
            ("pad", None),
        ] {
            let kind = parse_kind(name, preset).unwrap_or_else(|| panic!("{name}"));
            let mut inst = build(&kind, 48_000.0).unwrap();
            let note = if name == "drums" { 38 } else { 60 };
            let (peak, finite) = play(inst.as_mut(), note, 24_000);
            assert!(finite, "{name}");
            assert!(peak > 0.005, "{name} {preset:?} peak {peak}");
        }
        let mut sf = SoundFontInstrument::fallback_piano(48_000.0);
        let (peak, finite) = play(&mut sf, 60, 24_000);
        assert!(finite && peak > 0.005, "fallback piano {peak}");
        assert!(parse_kind("kazoo", None).is_none());
        assert!(build(&InstrumentKind::Ironfish { preset: Some("nope".into()) }, 48_000.0).is_err());
    }

    #[test]
    fn the_piano_sounds_damps_on_release_and_sustains_with_the_pedal() {
        let rate = 48_000.0;
        let mut p = PianoInstrument::new(rate, None);
        let (peak, finite) = play(&mut p, 60, 12_000);
        assert!(finite && peak > 0.005, "piano peak {peak}");
        p.note_off(60);
        rms(&mut p, 24_000);
        let damped = rms(&mut p, 12_000);

        let mut q = PianoInstrument::new(rate, Some("concert grand"));
        q.control(Control::Sustain, 1.0);
        play(&mut q, 60, 12_000);
        q.note_off(60);
        rms(&mut q, 24_000);
        let held = rms(&mut q, 12_000);
        assert!(held > damped * 2.0, "pedal {held} vs damped {damped}");
    }
}
