//! A MIDI file as an [`Arrangement`], written as a Score a person can read
//! ([`crate::arrangement::to_score`]): one loop per part, named for its
//! role (`drums`, `bass`, `keys`, `melody` ...), each playing its note list
//! on its GM program, the drums on a GM kit for the genre
//! ([`crate::gm::voice`], [`crate::gm::kit_for`]); the form as
//! Sections found from repetition ([`crate::form`]), carrying the file's
//! tempo map; and a mix and master chain.
//!
//! What a MIDI file holds and how it is played:
//! - notes, velocities: `[beat note dur vel]`, vel = velocity x expression
//!   (CC 11) at the note's start;
//! - program changes: `[beat @program n]` on a SoundFont part;
//! - the sustain pedal (CC 64): `[beat @pedal 1]` / `[beat @pedal 0]` on a
//!   native part; on a synth preset the notes are held to the pedal's
//!   release instead;
//! - channel volume (CC 7) and pan (CC 10): the loop's `amp` and `pan`;
//! - pitch bend at a note's start: a synth plays the bent pitch (a
//!   fractional note), a SoundFont the nearest semitone; bends inside a
//!   note and the modulation wheel are not played (the header counts them);
//! - the tempo map and time signature: the Clock and the Sections' `tempo`.

use crate::analyse::{self, Part, Song};
use crate::arrangement::{self, Arrangement, Note};
use crate::form::{self, Section as FormSection};
use crate::gm::{self, Role, Voice};

#[derive(Clone, Debug, Default)]
pub struct About {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    /// The library id and archive path, for the credit.
    pub id: String,
    pub path: String,
    pub genre: String,
    pub tags: Vec<(String, f32)>,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Every part on its GM program and the standard GM kit.
    pub gm_only: bool,
    /// The engine's production presets for the synth roles instead of
    /// their GM programs (`gm::preset`).
    pub presets: bool,
    /// The drum kit, instead of the one the genre picks.
    pub kit: Option<Voice>,
    /// Only measures `from..to` (of the song from its first note): an
    /// excerpt that plays once.
    pub excerpt: Option<(usize, usize)>,
    /// The master limiter's gain (the loudness a render fits it to).
    pub limiter_gain: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { gm_only: false, presets: false, kit: None, excerpt: None, limiter_gain: 1.6 }
    }
}

/// One part as imported.
#[derive(Clone, Debug)]
pub struct PartInfo {
    pub name: String,
    pub role: Role,
    pub melody: bool,
    pub voice: Voice,
    pub notes: usize,
    pub track_name: String,
}

#[derive(Clone, Debug)]
pub struct Imported {
    pub source: String,
    pub arrangement: Arrangement,
    /// The sections (measures counted from the song's first bar of music).
    pub sections: Vec<FormSection>,
    pub parts: Vec<PartInfo>,
    /// The length in beats (without the ring-out bar).
    pub beats: f64,
    pub bpm: f64,
}

fn voice_for(part: &Part, opts: &Options, about: &About) -> Voice {
    if part.drums {
        if opts.gm_only {
            return Voice::GmKit(0);
        }
        return opts.kit.clone().unwrap_or_else(|| gm::kit_for(&about.genre, &about.tags));
    }
    if opts.presets && !opts.gm_only {
        if let Some(preset) = gm::preset(part.program) {
            return preset;
        }
    }
    gm::voice(part.program)
}

/// The tick the sustain pedal next comes up at or after `tick` while it is
/// down there, else `tick`.
fn pedal_release(pedal: &[(u64, u8)], tick: u64) -> u64 {
    let i = pedal.partition_point(|p| p.0 <= tick);
    if i == 0 || pedal[i - 1].1 < 64 {
        return tick;
    }
    pedal[i..].iter().find(|p| p.1 < 64).map_or(tick, |p| p.0)
}

/// Import `bytes` (a Standard MIDI File) as a Score.
pub fn import(bytes: &[u8], about: &About, opts: &Options) -> Result<Imported, String> {
    let song = analyse::read(bytes)?;
    if song.parts.is_empty() {
        return Err("the file has no notes".into());
    }
    import_song(&song, about, opts)
}

fn clean(s: &str) -> String {
    s.replace(['"', '\\'], "'")
}

pub fn import_song(song: &Song, about: &About, opts: &Options) -> Result<Imported, String> {
    let all = song.measures();
    let first = song.parts.iter().filter_map(|p| p.notes.first()).map(|n| n.on).min().unwrap_or(0);
    let m0 = all.partition_point(|m| m.0 <= first).saturating_sub(1);
    let measures: Vec<(u64, u64)> = all[m0..].to_vec();
    let mut sections = form::sections(song, &measures);
    let (from, to) = opts.excerpt.unwrap_or((0, measures.len()));
    let to = to.min(measures.len());
    if from >= to {
        return Err(format!("the excerpt's bars {from}..{to} are outside the song's {} bars", measures.len()));
    }
    let origin = measures[from].0;
    let stop = if to < measures.len() { measures[to].0 } else { song.end.max(measures[to - 1].0 + measures[to - 1].1) };
    if opts.excerpt.is_some() {
        // The excerpt is one section, named for the sections it covers.
        let names: Vec<&str> = sections.iter().filter(|s| s.to > from && s.from < to).map(|s| s.name.as_str()).collect();
        let name = names.first().map_or("excerpt".to_string(), |n| n.to_string());
        sections = vec![FormSection { name, kind: "excerpt".into(), from, to, letter: 'A', energy: 0.0 }];
    }
    let ppq = song.ppq as f64;
    let beat = |tick: u64| (tick.saturating_sub(origin)) as f64 / ppq;
    // The bars of the arrangement: the measures from..to, rebased.
    let mut bars: Vec<f64> = measures[from..to].iter().map(|m| beat(m.0)).collect();
    bars.push(beat(stop));

    // Parts in role order (the tune first among the leads), named for
    // their role.
    let mut order: Vec<usize> = (0..song.parts.len()).collect();
    order.sort_by_key(|&i| {
        let p = &song.parts[i];
        (if p.melody { (Role::Lead, 0) } else { (p.role, 1) }, p.track, p.channel)
    });
    let base_of = |p: &Part| if p.melody { "melody" } else if p.role == Role::Fx { "effects" } else { p.role.name() };
    let mut parts: Vec<arrangement::Part> = Vec::new();
    let mut infos: Vec<PartInfo> = Vec::new();
    for &i in &order {
        let p = &song.parts[i];
        let base = base_of(p);
        let count = order.iter().filter(|&&j| base_of(&song.parts[j]) == base).count();
        let nth = infos.iter().filter(|n| n.name == base || n.name.starts_with(&format!("{base}_"))).count() + 1;
        let name = if count > 1 { format!("{base}_{nth}") } else { base.to_string() };
        let voice = voice_for(p, opts, about);
        let native = voice.native();
        let mut notes = Vec::new();
        for n in &p.notes {
            if n.on < origin || n.on >= stop {
                continue;
            }
            let off = if native || p.drums { n.off } else { pedal_release(&p.pedal, n.off).max(n.off) };
            let off = off.min(stop);
            // A SoundFont plays a bent start at the nearest semitone.
            let key = if p.drums || n.bend.abs() < 0.25 { n.key as f32 } else if native { (n.key as f32 + n.bend).round() } else { ((n.key as f32 + n.bend) * 100.0).round() / 100.0 };
            let vel = (n.vel as f64 / 127.0 * (n.expr as f64).powi(2)).clamp(0.02, 1.0);
            notes.push(Note { beat: (beat(n.on) * 1e4).round() / 1e4, key, dur: ((off - n.on) as f64 / ppq * 1e4).round() / 1e4, vel: (vel * 100.0).round() / 100.0, fx: None });
        }
        let in_range = |tick: u64| tick < stop;
        let programs: Vec<(f64, u8)> = if native && !p.drums { p.changes.iter().filter(|c| c.0 >= origin && in_range(c.0)).map(|c| (beat(c.0), c.1)).collect() } else { Vec::new() };
        let pedal: Vec<(f64, bool)> = if native && !p.drums { p.pedal.iter().filter(|c| in_range(c.0)).map(|c| (beat(c.0.max(origin)), c.1 >= 64)).collect() } else { Vec::new() };
        let gm_trim = if matches!(voice, Voice::Gm(_) | Voice::GmKit(_)) { 0.4 } else { 1.0 };
        let volume = (p.volume as f64 / 100.0).powi(2).clamp(0.3, 1.6);
        let amp = ((arrangement::level(p.role, p.melody) * volume * gm_trim).clamp(0.05, 1.5) * 100.0).round() / 100.0;
        let pan = (((p.pan as f64 - 64.0) / 63.0 * 0.8).clamp(-0.8, 0.8) * 100.0).round() / 100.0;
        let track = if p.name.is_empty() { format!("track {}", p.track) } else { format!("track {} \"{}\"", p.track, p.name.replace(['"', '\n', '\r'], "'")) };
        let program = if p.drums { "drums".to_string() } else { format!("GM {} {}", p.program, gm::PROGRAMS[p.program as usize & 127]) };
        infos.push(PartInfo { name: name.clone(), role: p.role, melody: p.melody, voice: voice.clone(), notes: notes.len(), track_name: p.name.clone() });
        parts.push(arrangement::Part { name, role: p.role, melody: p.melody, voice, about: format!("{track}, channel {}, {program}", p.channel + 1), notes, pedal, programs, amp, pan });
    }
    parts.retain(|p| !p.notes.is_empty());
    infos.retain(|i| i.notes > 0);

    let bpm0 = (song.bpm_at(origin) * 1000.0).round() / 1000.0;
    let tempo: Vec<(f64, f64)> = song.tempo.changes.iter().filter(|c| c.tick > origin && c.tick < stop).map(|c| (beat(c.tick), (60e6 / c.microseconds_per_quarter.max(1) as f64 * 1000.0).round() / 1000.0)).collect();
    let varies = tempo.iter().any(|t| (t.1 - bpm0).abs() > 0.05);
    let bent: usize = song.parts.iter().map(|p| p.bent).sum();
    let mods: usize = song.parts.iter().map(|p| p.mod_moves).sum();
    let mut notes = vec![
        "A cover imported from MIDI.".to_string(),
        crate::dataset::NOTICE.to_string(),
        format!("Source: {}, file {} ({}).", crate::dataset::CREDIT, about.path, about.id),
    ];
    if bent + mods > 0 {
        notes.push(format!("Not played: pitch bends inside notes and {mods} modulation-wheel moves ({bent} notes start bent: their start pitch plays)."));
    }
    let year = about.year.map_or(String::new(), |y| format!(" ({y})"));
    let a = Arrangement {
        title: about.title.clone(),
        artist: about.artist.clone(),
        year: about.year,
        genre: about.genre.clone(),
        key: analyse::key(song),
        bpm: bpm0,
        sig: song.sig_at(origin),
        bars,
        parts,
        sections: sections.iter().map(|s| arrangement::Section { name: s.name.clone(), kind: s.kind.clone(), letter: s.letter, from: s.from - from, to: s.to.min(to) - from }).collect(),
        tempo: if varies { tempo } else { Vec::new() },
        notes,
        rights: Some((
            "the song: its owners' copyright; the transcription: CC-BY-4.0".into(),
            clean(&format!("{} -- {}{year}. {}", about.title, about.artist, crate::dataset::CREDIT)),
            clean(&about.path),
            "https://creativecommons.org/licenses/by/4.0/ (the transcription); publishing a recording of the song needs a licence from its rights holder".into(),
        )),
        limiter_gain: opts.limiter_gain,
        excerpt: opts.excerpt.is_some(),
    };
    let source = arrangement::to_score(&a);
    let beats = a.beats();
    Ok(Imported { source, sections, parts: infos, beats, bpm: bpm0, arrangement: a })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_imports_loads_and_plays_every_note() {
        let bytes = crate::analyse::tests::band();
        let about = About { title: "Test Song".into(), artist: "The Band".into(), year: Some(1985), id: "0000".into(), path: "clean_midi/The Band/Test Song.mid".into(), genre: "rock".into(), tags: vec![] };
        for gm_only in [false, true] {
            let out = import(&bytes, &about, &Options { gm_only, ..Options::default() }).unwrap();
            let loaded = makepad_score_live::model::load(&out.source).unwrap_or_else(|d| panic!("{d:?}\n{}", out.source));
            let names: Vec<&str> = loaded.doc.loops.iter().map(|l| l.name.as_str()).collect();
            assert_eq!(names, vec!["drums", "bass", "keys", "melody"], "{}", out.source);
            // Every note of the file plays, at its beat.
            let mut composer = makepad_score_live::compose::Composer::new(loaded);
            let mut events = Vec::new();
            composer.pump(64.0, &mut events);
            let notes = events.iter().filter(|e| e.kind == makepad_score_player::EventKind::Note).count();
            assert_eq!(notes, 64 + 32 + 24 + 32, "{}", out.source);
            assert!((out.bpm - 100.0).abs() < 1e-9);
            assert_eq!(out.beats, 32.0);
            assert!(out.source.contains("copyrighted works"));
        }
        let excerpt = import(&bytes, &about, &Options { excerpt: Some((2, 4)), ..Options::default() }).unwrap();
        assert_eq!(excerpt.beats, 8.0);
        let loaded = makepad_score_live::model::load(&excerpt.source).unwrap_or_else(|d| panic!("{d:?}\n{}", excerpt.source));
        assert_eq!(loaded.doc.sections.len(), 1);
    }

    #[test]
    fn the_pedal_holds_a_synths_notes() {
        assert_eq!(pedal_release(&[(10, 127), (50, 0)], 20), 50);
        assert_eq!(pedal_release(&[(10, 127), (50, 0)], 60), 60);
        assert_eq!(arrangement::bars_text(14.0, 4.0), "3 + 1/2");
        assert_eq!(arrangement::note_name(70, true), "bb4");
    }
}
