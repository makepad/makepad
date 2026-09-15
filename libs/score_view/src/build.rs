//! Programmatic builders for short percussion, pitched, and bass-tab scores.

use crate::document::pitch_from_midi;
use makepad_score::{
    model::*,
    symbol::Clef,
};
use std::collections::BTreeMap;

const BUILDER_ACTOR: u64 = 0x6275_696c_6465_7273;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DrumVoice {
    Kick,
    Snare,
    SideStick,
    HiHatClosed,
    HiHatOpen,
    HiHatPedal,
    TomHigh,
    TomMid,
    TomLow,
    TomFloor,
    Ride,
    RideBell,
    Crash,
}

impl DrumVoice {
    pub const ALL: [Self; 13] = [
        Self::Kick,
        Self::Snare,
        Self::SideStick,
        Self::HiHatClosed,
        Self::HiHatOpen,
        Self::HiHatPedal,
        Self::TomHigh,
        Self::TomMid,
        Self::TomLow,
        Self::TomFloor,
        Self::Ride,
        Self::RideBell,
        Self::Crash,
    ];

    pub const fn gm_note(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Snare => 38,
            Self::SideStick => 37,
            Self::HiHatClosed => 42,
            Self::HiHatOpen => 46,
            Self::HiHatPedal => 44,
            Self::TomHigh => 50,
            Self::TomMid => 48,
            Self::TomLow => 45,
            Self::TomFloor => 41,
            Self::Ride => 51,
            Self::RideBell => 53,
            Self::Crash => 49,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Kick => "Kick",
            Self::Snare | Self::SideStick => "Snare",
            Self::HiHatClosed => "HH",
            Self::HiHatOpen => "HH open",
            Self::HiHatPedal => "Pedal",
            Self::TomHigh => "Tom hi",
            Self::TomMid => "Tom mid",
            Self::TomLow => "Tom lo",
            Self::TomFloor => "Floor",
            Self::Ride => "Ride",
            Self::RideBell => "Bell",
            Self::Crash => "Crash",
        }
    }

    pub fn display(self) -> (Pitch, Notehead) {
        let (step, octave, notehead) = match self {
            Self::Kick => (Step::F, 4, Notehead::Normal),
            Self::Snare => (Step::C, 5, Notehead::Normal),
            Self::SideStick => (Step::C, 5, Notehead::X),
            Self::HiHatClosed | Self::HiHatOpen => (Step::G, 5, Notehead::X),
            Self::HiHatPedal => (Step::D, 4, Notehead::X),
            Self::TomHigh => (Step::E, 5, Notehead::Normal),
            Self::TomMid => (Step::D, 5, Notehead::Normal),
            Self::TomLow => (Step::A, 4, Notehead::Normal),
            Self::TomFloor => (Step::F, 4, Notehead::Normal),
            Self::Ride => (Step::F, 5, Notehead::X),
            Self::RideBell => (Step::F, 5, Notehead::Diamond),
            Self::Crash => (Step::A, 5, Notehead::X),
        };
        (Pitch::new(step, Alter::NATURAL, octave), notehead)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Kick => "Kick drum",
            Self::Snare => "Snare drum",
            Self::SideStick => "Side stick",
            Self::HiHatClosed => "Closed hi-hat",
            Self::HiHatOpen => "Open hi-hat",
            Self::HiHatPedal => "Pedal hi-hat",
            Self::TomHigh => "High tom",
            Self::TomMid => "Mid tom",
            Self::TomLow => "Low tom",
            Self::TomFloor => "Floor tom",
            Self::Ride => "Ride cymbal",
            Self::RideBell => "Ride bell",
            Self::Crash => "Crash cymbal",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DrumHit {
    pub time_beats: f64,
    pub voice: DrumVoice,
    pub velocity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PitchedNote {
    pub onset_beats: f64,
    pub duration_beats: f64,
    pub midi: u8,
    pub velocity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LyricWord {
    pub onset_beats: f64,
    pub end_beats: f64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub bars: u32,
    pub beats_per_bar: u32,
    pub grid: Duration,
    pub bpm: Option<f64>,
    pub title: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            bars: 1,
            beats_per_bar: 4,
            grid: Duration::new(1, 16).expect("1/16 is a duration"),
            bpm: None,
            title: None,
        }
    }
}

pub fn build_drum_score(hits: &[DrumHit], opts: &BuildOptions) -> Score {
    let mut skeleton = Skeleton::new(
        opts,
        "Percussion",
        StaffKind::Percussion(PercussionMap {
            entries: DrumVoice::ALL
                .into_iter()
                .map(|voice| {
                    let (display, _) = voice.display();
                    (
                        u16::from(voice.gm_note()),
                        PercussionSound {
                            name: voice.name().into(),
                            midi_note: voice.gm_note(),
                            display,
                        },
                    )
                })
                .collect(),
        }),
        Clef::Percussion,
        3,
        *b"DRUMSCOREBUILD01",
    );
    let total = skeleton.grid.total_ticks;
    let mut at: BTreeMap<i64, Vec<&DrumHit>> = BTreeMap::new();
    for hit in hits {
        let tick = skeleton.grid.quantize_onset(hit.time_beats);
        if tick < total {
            at.entry(tick).or_default().push(hit);
        }
    }
    let mut rhythmic = Vec::new();
    for (tick, hits) in at {
        let notes = hits
            .into_iter()
            .map(|hit| {
                let (pitch, notehead) = hit.voice.display();
                Note {
                    id: skeleton.next::<NoteTag>(),
                    performance: performance(hit.velocity),
                    written_pitch: Some(pitch),
                    unpitched_sound: Some(u16::from(hit.voice.gm_note())),
                    display_staff: skeleton.staff,
                    tie_from: None,
                    tie_to: None,
                    tab: None,
                    notehead,
                }
            })
            .collect();
        rhythmic.push(Rhythmic {
            start: tick,
            ticks: 1,
            kind: EventKind::Chord(notes),
        });
    }
    let rhythmic = fill_rests(rhythmic, &skeleton.grid);
    skeleton.finish(rhythmic)
}

pub fn build_pitched_score(notes: &[PitchedNote], opts: &BuildOptions) -> Score {
    let median = median_midi(notes);
    let (clef, line) = if median >= 60 { (Clef::G, 2) } else { (Clef::F, 4) };
    let mut skeleton = Skeleton::new(
        opts,
        "Notes",
        StaffKind::Standard,
        clef,
        line,
        *b"PITCHSCOREBUILD1",
    );
    let groups = quantized_groups(notes, &skeleton.grid);
    let rhythmic = pitched_events(groups, &mut skeleton, None);
    let rhythmic = fill_rests(rhythmic, &skeleton.grid);
    skeleton.finish(rhythmic)
}

pub fn build_pitched_score_with_lyrics(
    notes: &[PitchedNote],
    lyrics: &[LyricWord],
    opts: &BuildOptions,
) -> Score {
    let mut score = build_pitched_score(notes, opts);
    attach_lyrics(&mut score, lyrics, opts);
    score
}

fn attach_lyrics(score: &mut Score, words: &[LyricWord], opts: &BuildOptions) {
    #[derive(Clone, Copy)]
    struct LyricNote {
        onset_beats: f64,
        note: NoteId,
    }

    let mut notes = score
        .voices
        .values()
        .flat_map(|voice| &voice.events)
        .filter_map(|event| {
            let note = event
                .chord_notes()
                .iter()
                .find(|note| note.tie_from.is_none())?;
            Some(LyricNote {
                onset_beats: event.onset.0.numerator() as f64
                    / event.onset.0.denominator() as f64
                    * 4.0,
                note: note.id,
            })
        })
        .collect::<Vec<_>>();
    notes.sort_by(|left, right| left.onset_beats.total_cmp(&right.onset_beats));
    if notes.is_empty() {
        return;
    }

    let total_beats = f64::from(opts.bars.max(1)) * f64::from(opts.beats_per_bar.max(1));
    let mut assigned: Vec<Vec<&LyricWord>> = vec![Vec::new(); notes.len()];
    for word in words {
        if word.text.trim().is_empty()
            || !word.onset_beats.is_finite()
            || word.onset_beats < 0.0
            || word.onset_beats >= total_beats
        {
            continue;
        }
        let length = if word.end_beats.is_finite() {
            (word.end_beats - word.onset_beats).max(0.0)
        } else {
            0.0
        };
        let tolerance = 0.25_f64.max(length * 0.15);
        let Some((index, distance)) = notes
            .iter()
            .enumerate()
            .map(|(index, note)| (index, (note.onset_beats - word.onset_beats).abs()))
            .min_by(|left, right| {
                left.1
                    .total_cmp(&right.1)
                    .then_with(|| left.0.cmp(&right.0))
            })
        else {
            continue;
        };
        if distance <= tolerance {
            assigned[index].push(word);
        }
    }

    for words in &mut assigned {
        words.sort_by(|left, right| left.onset_beats.total_cmp(&right.onset_beats));
    }
    for index in 0..notes.len() {
        if assigned[index].is_empty() {
            continue;
        }
        let text = assigned[index]
            .iter()
            .map(|word| word.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let word_end = assigned[index]
            .iter()
            .filter_map(|word| word.end_beats.is_finite().then_some(word.end_beats))
            .max_by(f64::total_cmp)
            .unwrap_or(notes[index].onset_beats);
        let next_word_note = assigned[index + 1..]
            .iter()
            .position(|words| !words.is_empty())
            .map(|offset| index + 1 + offset)
            .unwrap_or(notes.len());
        let melisma_to = notes[index + 1..next_word_note]
            .iter()
            .take_while(|note| note.onset_beats <= word_end)
            .last()
            .map(|note| note.note);
        score.lyrics.push(LyricSyllable {
            note: notes[index].note,
            verse: 1,
            text,
            role: SyllabicRole::Single,
            elision: None,
            melisma_to,
        });
    }
}

pub fn build_bass_tab_score(notes: &[PitchedNote], tuning: &[u8], opts: &BuildOptions) -> Score {
    let tuning = if tuning.is_empty() { &[28, 33, 38, 43][..] } else { tuning };
    let pitches = tuning.iter().copied().map(pitch_from_midi).collect();
    let mut skeleton = Skeleton::new(
        opts,
        "Bass tablature",
        StaffKind::Tablature(Tuning { strings_low_to_high: pitches }),
        if tuning.len() <= 4 { Clef::Tab4String } else { Clef::Tab6String },
        3,
        *b"BASSTABSCORE0001",
    );
    let groups = quantized_groups(notes, &skeleton.grid);
    let rhythmic = pitched_events(groups, &mut skeleton, Some(tuning));
    let rhythmic = fill_rests(rhythmic, &skeleton.grid);
    skeleton.finish(rhythmic)
}

struct Grid {
    duration: Duration,
    ticks_per_beat: i64,
    total_ticks: i64,
}

impl Grid {
    fn new(opts: &BuildOptions) -> Self {
        let beats = i64::from(opts.beats_per_bar.max(1));
        let bars = i64::from(opts.bars.max(1));
        let numerator = opts.grid.0.numerator().max(1);
        let denominator = opts.grid.0.denominator() as i64;
        let ticks_per_beat = div_round(denominator, 4 * numerator).max(1);
        Self {
            duration: opts.grid,
            ticks_per_beat,
            total_ticks: ticks_per_beat * beats * bars,
        }
    }

    fn quantize_onset(&self, beats: f64) -> i64 {
        if !beats.is_finite() {
            return 0;
        }
        (beats.max(0.0) * self.ticks_per_beat as f64).round() as i64
    }

    fn quantize_duration(&self, beats: f64) -> i64 {
        if !beats.is_finite() {
            return 1;
        }
        (beats.max(0.0) * self.ticks_per_beat as f64).round().max(1.0) as i64
    }

    fn time(&self, tick: i64) -> ScoreTime {
        ScoreTime(self.rational(tick))
    }

    fn duration(&self, ticks: i64) -> Duration {
        Duration::from_rational(self.rational(ticks.max(1))).expect("positive grid duration")
    }

    fn rational(&self, ticks: i64) -> Rational {
        Rational::new(
            self.duration.0.numerator().saturating_mul(ticks),
            self.duration.0.denominator(),
        )
        .expect("grid tick is representable")
    }
}

fn div_round(numerator: i64, denominator: i64) -> i64 {
    (numerator + denominator / 2) / denominator
}

struct Skeleton {
    score: Score,
    ids: IdGenerator,
    staff: StaffId,
    voice: VoiceId,
    grid: Grid,
}

impl Skeleton {
    fn new(
        opts: &BuildOptions,
        part_name: &str,
        kind: StaffKind,
        clef: Clef,
        clef_line: u8,
        score_id: [u8; 16],
    ) -> Self {
        let grid = Grid::new(opts);
        let mut ids = IdGenerator::new(BUILDER_ACTOR);
        let part = ids.next::<PartTag>().expect("builder id space");
        let staff = ids.next::<StaffTag>().expect("builder id space");
        let voice = ids.next::<VoiceTag>().expect("builder id space");
        let mut score = Score::new(score_id);
        score.title = opts.title.clone().unwrap_or_default();
        score.parts.insert(part, Part {
            id: part,
            name: part_name.into(),
            staves: vec![staff],
            transposition: Transposition::NONE,
        });
        score.staves.insert(staff, Staff {
            id: staff,
            part,
            parent: None,
            kind,
            voices: vec![voice],
        });
        let beats = opts.beats_per_bar.max(1);
        let bars = opts.bars.max(1);
        let extent = Duration::new(i64::from(beats), 4).expect("bar duration");
        for bar in 0..bars {
            let id = ids.next::<MeasureTag>().expect("builder id space");
            let start = ScoreTime::new(i64::from(bar) * i64::from(beats), 4)
                .expect("measure start");
            score.measures.insert(id, Measure {
                id,
                ordinal: bar,
                label: (bar + 1).to_string(),
                start,
                extent,
            });
            score.flow.nodes.push(FlowNode { measure: id, ordinal: bar });
        }
        score.maps.time_signature.push(Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: Meter::Measured { groups: vec![beats as u16], unit: 4 },
        });
        if let Some(bpm) = opts.bpm.filter(|bpm| bpm.is_finite() && *bpm > 0.0) {
            score.maps.tempo.push(Change {
                at: ScoreTime::ZERO,
                scope: MapScope::Global,
                value: Tempo::Instant {
                    quarters_per_minute: Rational::new((bpm * 1000.0).round() as i64, 1000)
                        .expect("positive tempo"),
                },
            });
        }
        let mut events = vec![plain_event(
            ids.next::<EventTag>().expect("builder id space"),
            ScoreTime::ZERO,
            EventKind::Clef(ClefChange { clef, line: clef_line }),
        )];
        if let Some(bpm) = opts.bpm.filter(|bpm| bpm.is_finite() && *bpm > 0.0) {
            events.push(plain_event(
                ids.next::<EventTag>().expect("builder id space"),
                ScoreTime::ZERO,
                EventKind::Direction(DirectionEvent {
                    kind: DirectionKind::TempoText(format!("{} bpm", bpm.round() as u32)),
                    placement: None,
                    original_text: None,
                }),
            ));
        }
        score.voices.insert(voice, Voice { id: voice, staff, number: 1, events });
        Self { score, ids, staff, voice, grid }
    }

    fn next<K>(&mut self) -> Id<K> {
        self.ids.next::<K>().expect("builder id space")
    }

    fn finish(mut self, rhythmic: Vec<Rhythmic>) -> Score {
        let mut timed = rhythmic
            .into_iter()
            .map(|event| TimedEvent {
                id: self.next::<EventTag>(),
                onset: self.grid.time(event.start),
                duration: Some(self.grid.duration(event.ticks)),
                grace: None,
                kind: event.kind,
                beams: Vec::new(),
                tuplets: Vec::new(),
                articulations: Vec::new(),
                ornaments: Vec::new(),
            })
            .collect::<Vec<_>>();
        let voice = self.score.voices.get_mut(&self.voice).expect("builder voice");
        voice.events.append(&mut timed);
        voice.events.sort_by_key(|event| (event.onset, event.id));
        self.score.maps.sort();
        self.score
    }
}

fn plain_event(id: EventId, onset: ScoreTime, kind: EventKind) -> TimedEvent {
    TimedEvent {
        id,
        onset,
        duration: None,
        grace: None,
        kind,
        beams: Vec::new(),
        tuplets: Vec::new(),
        articulations: Vec::new(),
        ornaments: Vec::new(),
    }
}

struct Rhythmic {
    start: i64,
    ticks: i64,
    kind: EventKind,
}

fn fill_rests(mut events: Vec<Rhythmic>, grid: &Grid) -> Vec<Rhythmic> {
    events.sort_by_key(|event| event.start);
    let mut output = Vec::new();
    let mut cursor = 0;
    for event in events {
        if event.start > cursor {
            push_rests(&mut output, cursor, event.start, grid);
        }
        if event.start >= cursor {
            cursor = event.start + event.ticks;
            output.push(event);
        }
    }
    if cursor < grid.total_ticks {
        push_rests(&mut output, cursor, grid.total_ticks, grid);
    }
    output
}

fn push_rests(output: &mut Vec<Rhythmic>, mut start: i64, end: i64, grid: &Grid) {
    while start < end {
        let in_beat = start.rem_euclid(grid.ticks_per_beat);
        let beat_room = grid.ticks_per_beat - in_beat;
        let room = (end - start).min(beat_room);
        let mut ticks = largest_power_of_two_at_most(room);
        while grid.ticks_per_beat % ticks != 0 && ticks > 1 {
            ticks /= 2;
        }
        output.push(Rhythmic { start, ticks, kind: EventKind::Rest });
        start += ticks;
    }
}

fn largest_power_of_two_at_most(value: i64) -> i64 {
    let mut result = 1;
    while result <= value / 2 {
        result *= 2;
    }
    result
}

#[derive(Clone)]
struct NoteGroup {
    start: i64,
    ticks: i64,
    notes: Vec<(u8, f32)>,
}

fn quantized_groups(notes: &[PitchedNote], grid: &Grid) -> Vec<NoteGroup> {
    let mut grouped: BTreeMap<i64, Vec<&PitchedNote>> = BTreeMap::new();
    for note in notes {
        let start = grid.quantize_onset(note.onset_beats);
        if start < grid.total_ticks {
            grouped.entry(start).or_default().push(note);
        }
    }
    let starts = grouped.keys().copied().collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .filter_map(|(index, start)| {
            let source = &grouped[start];
            let wanted = source
                .iter()
                .map(|note| grid.quantize_duration(note.duration_beats))
                .max()
                .unwrap_or(1);
            let next = starts.get(index + 1).copied().unwrap_or(grid.total_ticks);
            let end = (*start + wanted).min(next).min(grid.total_ticks);
            (end > *start).then(|| {
                let mut pitches = BTreeMap::new();
                for note in source {
                    pitches.insert(note.midi, note.velocity);
                }
                NoteGroup {
                    start: *start,
                    ticks: end - *start,
                    notes: pitches.into_iter().collect(),
                }
            })
        })
        .collect()
}

fn pitched_events(
    groups: Vec<NoteGroup>,
    skeleton: &mut Skeleton,
    tuning: Option<&[u8]>,
) -> Vec<Rhythmic> {
    let mut output = Vec::new();
    for group in groups {
        let mut pieces = Vec::new();
        let mut cursor = group.start;
        let end = group.start + group.ticks;
        while cursor < end {
            let beat_end = ((cursor / skeleton.grid.ticks_per_beat) + 1)
                * skeleton.grid.ticks_per_beat;
            let piece_end = end.min(beat_end);
            pieces.push((cursor, piece_end - cursor));
            cursor = piece_end;
        }
        let ids = (0..pieces.len())
            .map(|_| {
                group
                    .notes
                    .iter()
                    .map(|_| skeleton.next::<NoteTag>())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for (piece_index, (start, ticks)) in pieces.into_iter().enumerate() {
            let notes = group
                .notes
                .iter()
                .enumerate()
                .map(|(note_index, &(midi, velocity))| Note {
                    id: ids[piece_index][note_index],
                    performance: performance(velocity),
                    written_pitch: Some(pitch_from_midi(midi)),
                    unpitched_sound: None,
                    display_staff: skeleton.staff,
                    tie_from: piece_index
                        .checked_sub(1)
                        .map(|previous| ids[previous][note_index]),
                    tie_to: ids.get(piece_index + 1).map(|next| next[note_index]),
                    tab: tuning.and_then(|tuning| tab_position(midi, tuning)),
                    notehead: Notehead::Normal,
                })
                .collect();
            output.push(Rhythmic { start, ticks, kind: EventKind::Chord(notes) });
        }
    }
    output
}

fn performance(velocity: f32) -> Option<NotePerformance> {
    if !velocity.is_finite() {
        return None;
    }
    Some(NotePerformance {
        velocity: (velocity.clamp(0.0, 1.0) * 127.0).round().clamp(1.0, 127.0) as u8,
    })
}

fn median_midi(notes: &[PitchedNote]) -> u8 {
    let mut values = notes.iter().map(|note| note.midi).collect::<Vec<_>>();
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or(60)
}

fn tab_position(midi: u8, tuning: &[u8]) -> Option<TabPosition> {
    tuning
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, open)| midi >= *open)
        .map(|(index, open)| {
            let string = tuning.len() - index;
            (u16::from(midi - open), string as u16)
        })
        .min()
        .map(|(fret, string)| TabPosition { string, fret, bend: Alter::NATURAL })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScoreDocument;

    fn rhythmic(score: &Score) -> Vec<&TimedEvent> {
        score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .filter(|event| event.duration.is_some())
            .collect()
    }

    fn assert_bar_exact(score: &Score) {
        assert!(score.validate().is_empty(), "{:#?}", score.validate());
        for measure in score.measures.values() {
            let end = measure.start.checked_add(measure.extent).unwrap();
            let events = rhythmic(score)
                .into_iter()
                .filter(|event| event.onset >= measure.start && event.onset < end)
                .collect::<Vec<_>>();
            assert_eq!(events.first().unwrap().onset, measure.start);
            let mut cursor = measure.start;
            for event in events {
                assert_eq!(event.onset, cursor);
                cursor = event.end().unwrap();
            }
            assert_eq!(cursor, end);
        }
    }

    #[test]
    fn gm_numbers_match_the_drum_map() {
        let got = DrumVoice::ALL.map(DrumVoice::gm_note);
        assert_eq!(got, [36, 38, 37, 42, 46, 44, 50, 48, 45, 41, 51, 53, 49]);
    }

    #[test]
    fn drum_hits_quantize_chord_and_fill_rests() {
        let opts = BuildOptions::default();
        let score = build_drum_score(&[
            DrumHit { time_beats: 0.02, voice: DrumVoice::Kick, velocity: 1.0 },
            DrumHit { time_beats: 0.03, voice: DrumVoice::HiHatClosed, velocity: 0.7 },
            DrumHit { time_beats: 0.20, voice: DrumVoice::Ride, velocity: 0.6 },
            DrumHit { time_beats: 2.0, voice: DrumVoice::Snare, velocity: 0.8 },
        ], &opts);
        assert_bar_exact(&score);
        let events = rhythmic(&score);
        assert!(matches!(&events[0].kind, EventKind::Chord(notes) if notes.len() == 2));
        assert!(events.iter().any(|event| matches!(event.kind, EventKind::Rest)));
        assert_eq!(events[0].onset, ScoreTime::ZERO);
        assert!(events
            .iter()
            .any(|event| event.onset == ScoreTime::new(1, 16).unwrap()));
    }

    #[test]
    fn pitched_notes_split_and_tie_at_beats_and_bars() {
        let opts = BuildOptions { bars: 2, ..BuildOptions::default() };
        let score = build_pitched_score(&[
            PitchedNote { onset_beats: 3.5, duration_beats: 1.5, midi: 64, velocity: 0.8 },
        ], &opts);
        assert_bar_exact(&score);
        let chain = rhythmic(&score)
            .into_iter()
            .filter_map(|event| match &event.kind { EventKind::Chord(notes) => Some(&notes[0]), _ => None })
            .collect::<Vec<_>>();
        assert!(chain.len() >= 2);
        assert_eq!(chain[0].tie_to, Some(chain[1].id));
        assert_eq!(chain[1].tie_from, Some(chain[0].id));
    }

    #[test]
    fn later_pitched_onset_wins_an_overlap() {
        let score = build_pitched_score(&[
            PitchedNote { onset_beats: 0.0, duration_beats: 3.0, midi: 60, velocity: 0.5 },
            PitchedNote { onset_beats: 1.0, duration_beats: 1.0, midi: 62, velocity: 0.5 },
        ], &BuildOptions::default());
        assert_bar_exact(&score);
        let at_one = rhythmic(&score).into_iter().find(|event| event.onset == ScoreTime::new(1, 4).unwrap()).unwrap();
        assert!(matches!(&at_one.kind, EventKind::Chord(notes) if notes[0].written_pitch == Some(pitch_from_midi(62))));
    }

    fn lyric_note_onset(score: &Score, lyric: &LyricSyllable) -> f64 {
        score
            .voices
            .values()
            .flat_map(|voice| &voice.events)
            .find(|event| event.chord_notes().iter().any(|note| note.id == lyric.note))
            .map(|event| {
                event.onset.0.numerator() as f64 / event.onset.0.denominator() as f64 * 4.0
            })
            .expect("lyric note belongs to an event")
    }

    fn lyric_notes() -> [PitchedNote; 3] {
        [
            PitchedNote { onset_beats: 0.0, duration_beats: 0.5, midi: 60, velocity: 0.8 },
            PitchedNote { onset_beats: 1.0, duration_beats: 0.5, midi: 62, velocity: 0.8 },
            PitchedNote { onset_beats: 2.0, duration_beats: 0.5, midi: 64, velocity: 0.8 },
        ]
    }

    #[test]
    fn lyrics_match_exact_note_onsets() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[
                LyricWord { onset_beats: 0.0, end_beats: 0.5, text: "sing".into() },
                LyricWord { onset_beats: 2.0, end_beats: 2.5, text: "now".into() },
            ],
            &BuildOptions::default(),
        );
        assert_eq!(score.lyrics.len(), 2);
        assert_eq!(score.lyrics[0].text, "sing");
        assert_eq!(lyric_note_onset(&score, &score.lyrics[0]), 0.0);
        assert_eq!(score.lyrics[1].text, "now");
        assert_eq!(lyric_note_onset(&score, &score.lyrics[1]), 2.0);
    }

    #[test]
    fn lyrics_choose_the_nearest_note_within_tolerance() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[LyricWord { onset_beats: 0.8, end_beats: 1.2, text: "near".into() }],
            &BuildOptions::default(),
        );
        assert_eq!(score.lyrics.len(), 1);
        assert_eq!(lyric_note_onset(&score, &score.lyrics[0]), 1.0);
    }

    #[test]
    fn words_on_one_note_are_joined() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[
                LyricWord { onset_beats: 0.0, end_beats: 0.1, text: "come".into() },
                LyricWord { onset_beats: 0.1, end_beats: 0.2, text: "on".into() },
            ],
            &BuildOptions::default(),
        );
        assert_eq!(score.lyrics.len(), 1);
        assert_eq!(score.lyrics[0].text, "come on");
    }

    #[test]
    fn a_word_held_over_later_notes_gets_a_melisma() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[LyricWord { onset_beats: 0.0, end_beats: 1.2, text: "sing".into() }],
            &BuildOptions::default(),
        );
        assert_eq!(score.lyrics.len(), 1);
        let second = score
            .voices
            .values()
            .flat_map(|voice| &voice.events)
            .find(|event| event.onset == ScoreTime::new(1, 4).unwrap())
            .and_then(|event| event.chord_notes().first())
            .expect("second note");
        assert_eq!(score.lyrics[0].melisma_to, Some(second.id));
    }

    #[test]
    fn no_words_make_no_lyrics() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[],
            &BuildOptions::default(),
        );
        assert!(score.lyrics.is_empty());
    }

    #[test]
    fn words_outside_the_loop_are_ignored() {
        let score = build_pitched_score_with_lyrics(
            &lyric_notes(),
            &[
                LyricWord { onset_beats: -0.1, end_beats: 0.1, text: "before".into() },
                LyricWord { onset_beats: 4.0, end_beats: 4.2, text: "after".into() },
            ],
            &BuildOptions::default(),
        );
        assert!(score.lyrics.is_empty());
    }

    #[test]
    fn bass_tab_chooses_the_lowest_fret() {
        let score = build_bass_tab_score(&[
            PitchedNote { onset_beats: 0.0, duration_beats: 1.0, midi: 43, velocity: 0.8 },
        ], &[28, 33, 38, 43], &BuildOptions::default());
        assert_bar_exact(&score);
        let tab = rhythmic(&score).into_iter().find_map(|event| event.chord_notes().first()?.tab).unwrap();
        assert_eq!((tab.string, tab.fret), (1, 0));
    }

    #[test]
    fn every_builder_engraves_headlessly_with_fallback_outlines() {
        let opts = BuildOptions::default();
        let pitched = [PitchedNote { onset_beats: 0.0, duration_beats: 1.0, midi: 60, velocity: 0.8 }];
        let scores = [
            build_drum_score(&[DrumHit { time_beats: 0.0, voice: DrumVoice::Kick, velocity: 1.0 }], &opts),
            build_pitched_score(&pitched, &opts),
            build_bass_tab_score(&pitched, &[28, 33, 38, 43], &opts),
        ];
        for score in scores {
            let document = ScoreDocument::new(score).unwrap();
            assert!(!document.pages().is_empty());
            assert!(!document.pages()[0].items().is_empty());
        }
    }
}
