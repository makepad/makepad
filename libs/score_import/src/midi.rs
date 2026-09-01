use crate::diagnostic::{
    ImportError, ImportReport, Inference, InferenceKind, SourceLocation,
};
use crate::ids::{score_id, stable_id};
use makepad_midi_file::{
    Division, EventKind as MidiEventKind, Format, MetaEvent, MidiFile, NoteSequence, PairedNote,
};
use makepad_score::model::*;
use makepad_score::symbol::Clef;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiImportOptions {
    /// Smallest ordinary notation grid, measured in whole notes.
    pub quantize_grid: Duration,
    /// Zero keeps performed timing; 1000 snaps fully to the selected grid.
    pub quantize_strength_milli: u16,
    /// Widest simultaneous reach one hand is allowed, in semitones. A
    /// simultaneity wider than this is divided between the hands.
    pub max_hand_span_semitones: u8,
    /// Silence up to this fraction of the slot between two onsets counts as
    /// articulation on the earlier note rather than as a written rest. A
    /// performance detaches notes, so the written value follows onset-to-onset
    /// timing, never the release.
    pub articulation_fill_ratio_milli: u16,
    pub detect_tuplets: bool,
    /// Sequence index for format-2 files. Ignored for formats 0 and 1.
    pub sequence: usize,
}

impl Default for MidiImportOptions {
    fn default() -> Self {
        Self {
            quantize_grid: Duration::new(1, 16).expect("constant duration"),
            quantize_strength_milli: 1000,
            max_hand_span_semitones: 15,
            articulation_fill_ratio_milli: 500,
            detect_tuplets: true,
            sequence: 0,
        }
    }
}

/// Which hand — and therefore which staff of the piano brace — plays a note.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Hand {
    Left,
    Right,
}

/// How the hand of a note was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandSource {
    /// A track name such as `Piano right` / `Piano left` named the hand.
    TrackName,
    /// The file separates exactly two note streams by track or channel, and
    /// they were read as the two hands.
    StreamLayout,
    /// No explicit hand information: the cost-based assignment decided.
    Inferred,
}

/// The performed take of one imported note, kept beside the notated result so
/// the exact release, velocity and hand evidence survive the notation pass.
#[derive(Clone, Debug, PartialEq)]
pub struct PerformedNote {
    /// The first notated piece of this note; later pieces are tied to it.
    pub note: NoteId,
    /// Index into [`RawPerformance::sequence`]`.notes`.
    pub source_index: usize,
    pub track: usize,
    pub channel: u8,
    pub key: u8,
    pub velocity: u8,
    /// Exact performed timing, before quantization.
    pub played_onset: ScoreTime,
    pub played_end: ScoreTime,
    /// Quantized sounding timing: still the release, not the written value.
    pub sounding_onset: ScoreTime,
    pub sounding_end: ScoreTime,
    /// Notated onset and value. `None` marks a grace note, which has no extent.
    pub written_onset: ScoreTime,
    pub written_duration: Option<Duration>,
    pub hand: Hand,
    pub hand_source: HandSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawPerformance {
    pub file: MidiFile,
    pub sequence: NoteSequence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MidiImportResult {
    pub score: makepad_score::model::Score,
    pub performance: RawPerformance,
    /// Per-note bridge between the performance and the notation, in the order
    /// of `performance.sequence.notes`.
    pub performed_notes: Vec<PerformedNote>,
    pub report: ImportReport,
}

pub fn import_midi_bytes(bytes: &[u8]) -> Result<MidiImportResult, ImportError> {
    let file = MidiFile::parse(bytes)?;
    import_midi(&file, MidiImportOptions::default())
}

pub fn import_midi(
    file: &MidiFile,
    options: MidiImportOptions,
) -> Result<MidiImportResult, ImportError> {
    if options.quantize_strength_milli > 1000 {
        return Err(ImportError::InvalidSource(
            "quantize_strength_milli must be at most 1000".to_string(),
        ));
    }
    let sequences = file.paired_notes()?;
    let sequence_index = if file.header.format == Format::Sequential {
        options.sequence
    } else {
        0
    };
    let sequence = sequences.get(sequence_index).cloned().ok_or_else(|| {
        ImportError::InvalidSource(format!("MIDI sequence {sequence_index} does not exist"))
    })?;
    let performance = RawPerformance {
        file: file.clone(),
        sequence: sequence.clone(),
    };
    let mut report = ImportReport::default();
    for note in &sequence.unmatched_note_ons {
        report.ignored(
            "midi.unmatched-note-on",
            format!("unmatched note-on for MIDI key {}", note.key),
            midi_location(sequence_index, Some(note.track), Some(note.tick_on), None),
        );
    }
    for note in &sequence.unmatched_note_offs {
        report.ignored(
            "midi.unmatched-note-off",
            format!("unmatched note-off for MIDI key {}", note.key),
            midi_location(sequence_index, Some(note.track), Some(note.tick_off), None),
        );
    }

    let mut notes = sequence
        .notes
        .iter()
        .enumerate()
        .map(|(index, note)| performance_note(file.header.division, note, index, &mut report))
        .collect::<Result<Vec<_>, _>>()?;
    if !notes.is_empty() {
        report.approximated(
            "midi.velocity-dynamics",
            "note velocities remain exact in raw performance; they were not promoted to discrete notation dynamics",
            midi_location(sequence_index, None, None, None),
        );
    }
    notes.sort_by_key(|note| (note.onset, note.end, note.key, note.source_index));
    quantize_notes(&mut notes, options, &mut report)?;
    refine_dense_slots(&mut notes, options, sequence_index, &mut report)?;
    resolve_collapsed_notes(&mut notes, options, sequence_index, &mut report)?;
    assign_hands(&mut notes, file, options, sequence_index, &mut report)?;

    let (fifths, minor, key_confidence, key_from_file) = infer_key(file, sequence_index, &notes)?;
    report.inferences.push(Inference {
        kind: InferenceKind::KeySignature,
        confidence_milli: key_confidence,
        detail: format!(
            "{} key signature with {fifths} fifths{}",
            if key_from_file { "used file" } else { "inferred" },
            if minor { " (minor mode; mode is report-only)" } else { "" }
        ),
        source: midi_location(sequence_index, None, Some(0), None),
    });
    if minor {
        report.approximated(
            "midi.minor-mode",
            "the score key map stores accidentals but not major/minor mode; minor mode remains in the inference report",
            midi_location(sequence_index, None, Some(0), None),
        );
    }

    let (meter, meter_confidence, meter_from_file) = infer_meter(file, sequence_index, &notes)?;
    report.inferences.push(Inference {
        kind: InferenceKind::TimeSignature,
        confidence_milli: meter_confidence,
        detail: format!(
            "{} {}",
            if meter_from_file { "used file meter" } else { "inferred meter" },
            meter_description(&meter)
        ),
        source: midi_location(sequence_index, None, Some(0), None),
    });

    let title = midi_title(file, sequence_index).unwrap_or_else(|| "MIDI import".to_string());
    let mut score = makepad_score::model::Score::new(score_id(&format!(
        "midi:{title}:{}:{}",
        file.tracks.len(),
        sequence.notes.len()
    )));
    score.title = title;
    let performed_notes = build_midi_score(
        &mut score,
        &notes,
        fifths,
        meter.clone(),
        options,
        sequence_index,
        &mut report,
    )?;
    import_midi_maps(file, sequence_index, &mut score, &mut report)?;
    score.maps.time_signature.insert(
        0,
        Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: meter,
        },
    );
    score.maps.key.insert(
        0,
        Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: KeySignature {
                fifths,
                custom: Vec::new(),
            },
        },
    );
    score.maps.sort();
    report_non_notation_events(file, sequence_index, &mut report);
    Ok(MidiImportResult {
        score,
        performance,
        performed_notes,
        report,
    })
}

#[derive(Clone, Debug)]
struct PerformanceNote {
    source_index: usize,
    track: usize,
    channel: u8,
    key: u8,
    velocity: u8,
    /// Exact performed timing, before quantization.
    played_onset: ScoreTime,
    played_end: ScoreTime,
    /// Sounding timing after quantization and strength blending.
    onset: ScoreTime,
    end: ScoreTime,
    /// The fully snapped grid position of the onset. Simultaneity, chords and
    /// hand slices are decided on this, never on the blended onset.
    grid_onset: ScoreTime,
    tuplet: bool,
    /// A note too short to notate that shares its slot with a later note.
    grace: bool,
    hand: Hand,
    hand_source: HandSource,
}

fn performance_note(
    division: Division,
    note: &PairedNote,
    source_index: usize,
    report: &mut ImportReport,
) -> Result<PerformanceNote, ImportError> {
    let (onset, end) = match division {
        Division::TicksPerQuarter(ppq) if ppq > 0 => (
            ScoreTime::new(
                i64::try_from(note.tick_on).map_err(|_| ImportError::InvalidSource("MIDI tick overflow".to_string()))?,
                u64::from(ppq) * 4,
            )?,
            ScoreTime::new(
                i64::try_from(note.tick_off).map_err(|_| ImportError::InvalidSource("MIDI tick overflow".to_string()))?,
                u64::from(ppq) * 4,
            )?,
        ),
        Division::TicksPerQuarter(_) => {
            return Err(ImportError::InvalidSource(
                "MIDI ticks-per-quarter division is zero".to_string(),
            ));
        }
        Division::Smpte {
            frames_per_second,
            ticks_per_frame,
        } if ticks_per_frame > 0 => {
            let (fps_num, fps_den) = frames_per_second.ratio();
            // With no beat domain in SMPTE files, establish the conventional
            // 120-QPM transcription clock: a whole note is two seconds.
            let denominator = u64::from(fps_num) * u64::from(ticks_per_frame) * 2;
            let convert = |tick: u64| {
                let numerator = i64::try_from(
                    u128::from(tick) * u128::from(fps_den),
                )
                .map_err(|_| ImportError::InvalidSource("SMPTE tick overflow".to_string()))?;
                Ok::<_, ImportError>(ScoreTime::new(numerator, denominator)?)
            };
            report.approximated(
                "midi.smpte-beat-clock",
                "SMPTE-timed MIDI has no beat grid; notation used an explicit 120-QPM transcription clock",
                midi_location(0, Some(note.track), Some(note.tick_on), None),
            );
            (convert(note.tick_on)?, convert(note.tick_off)?)
        }
        Division::Smpte { .. } => {
            return Err(ImportError::InvalidSource(
                "MIDI SMPTE ticks-per-frame is zero".to_string(),
            ));
        }
    };
    Ok(PerformanceNote {
        source_index,
        track: note.track,
        channel: note.channel,
        key: note.key,
        velocity: note.velocity_on,
        played_onset: onset,
        played_end: end,
        onset,
        end,
        grid_onset: onset,
        tuplet: false,
        grace: false,
        hand: Hand::Right,
        hand_source: HandSource::Inferred,
    })
}

fn quantize_notes(
    notes: &mut [PerformanceNote],
    options: MidiImportOptions,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let strength = Rational::new(i64::from(options.quantize_strength_milli), 1000)?;
    let triplet_grid = Duration::from_rational(
        options
            .quantize_grid
            .0
            .checked_mul(Rational::new(2, 3)?)?,
    )?;
    let half_grid = Duration::from_rational(
        options
            .quantize_grid
            .0
            .checked_div(Rational::new(2, 1)?)?,
    )?;
    let mut total_error = Rational::ZERO;
    for note in notes.iter_mut() {
        let ordinary_onset = snap(note.played_onset, options.quantize_grid)?;
        let ordinary_end = snap(note.played_end, options.quantize_grid)?;
        let ordinary_error = distance(note.played_onset, ordinary_onset)?
            .checked_add(distance(note.played_end, ordinary_end)?)?;
        let triplet_onset = snap(note.played_onset, triplet_grid)?;
        let triplet_end = snap(note.played_end, triplet_grid)?;
        let triplet_error = distance(note.played_onset, triplet_onset)?
            .checked_add(distance(note.played_end, triplet_end)?)?;
        // A note that sits off the grid may be a triplet, or it may simply be a
        // finer duple value. Halving the grid explains the second case, and it
        // has to be ruled out before a tuplet is claimed: without this, every
        // thirty-second in a sixteenth-note grid reads as a triplet, because
        // one twenty-fourth happens to lie nearer than one sixteenth.
        let half_error = distance(note.played_onset, snap(note.played_onset, half_grid)?)?
            .checked_add(distance(note.played_end, snap(note.played_end, half_grid)?)?)?;
        let use_triplet = options.detect_tuplets
            && triplet_error < ordinary_error
            && triplet_error < half_error
            && triplet_error
                .checked_mul(Rational::new(3, 2)?)?
                < ordinary_error;
        let (target_onset, target_end, error) = if use_triplet {
            note.tuplet = true;
            (triplet_onset, triplet_end, triplet_error)
        } else {
            (ordinary_onset, ordinary_end, ordinary_error)
        };
        total_error = total_error.checked_add(error)?;
        note.grid_onset = target_onset;
        note.onset = blend_time(note.played_onset, target_onset, strength)?;
        note.end = blend_time(note.played_end, target_end, strength)?;
    }
    let confidence = if notes.is_empty() {
        1000
    } else {
        let normalized = total_error
            .checked_div(options.quantize_grid.0)?
            .checked_div(Rational::new(notes.len() as i64, 1)?)?;
        confidence_from_error(normalized)
    };
    report.inferences.push(Inference {
        kind: InferenceKind::Quantization,
        confidence_milli: confidence,
        detail: format!(
            "grid {}, strength {}/1000; {} notes selected a triplet grid",
            options.quantize_grid.0,
            options.quantize_strength_milli,
            notes.iter().filter(|note| note.tuplet).count()
        ),
        source: midi_location(0, None, None, None),
    });
    Ok(())
}

fn snap(time: ScoreTime, grid: Duration) -> Result<ScoreTime, ImportError> {
    let quotient = time.0.checked_div(grid.0)?;
    let rounded = round_rational(quotient)?;
    Ok(ScoreTime(grid.0.checked_mul(Rational::new(rounded, 1)?)?))
}

fn blend_time(
    original: ScoreTime,
    target: ScoreTime,
    strength: Rational,
) -> Result<ScoreTime, ImportError> {
    let delta = target.0.checked_sub(original.0)?;
    Ok(ScoreTime(original.0.checked_add(delta.checked_mul(strength)?)?))
}

fn distance(left: ScoreTime, right: ScoreTime) -> Result<Rational, ImportError> {
    let difference = left.0.checked_sub(right.0)?;
    if difference.numerator() < 0 {
        Ok(difference.checked_neg()?)
    } else {
        Ok(difference)
    }
}

fn round_rational(value: Rational) -> Result<i64, ImportError> {
    let numerator = value.numerator();
    let denominator = i64::try_from(value.denominator())
        .map_err(|_| ImportError::InvalidSource("rational denominator overflow".to_string()))?;
    Ok(if numerator >= 0 {
        numerator
            .checked_add(denominator / 2)
            .ok_or(makepad_score::model::RationalError::Overflow)?
            / denominator
    } else {
        numerator
            .checked_sub(denominator / 2)
            .ok_or(makepad_score::model::RationalError::Overflow)?
            / denominator
    })
}

fn confidence_from_error(error: Rational) -> u16 {
    if error >= Rational::ONE {
        return 0;
    }
    let remaining = Rational::ONE.checked_sub(error).unwrap_or(Rational::ZERO);
    let scaled = remaining
        .checked_mul(Rational::new(1000, 1).expect("constant"))
        .unwrap_or(Rational::ZERO);
    u16::try_from(scaled.numerator() / scaled.denominator() as i64)
        .unwrap_or(0)
        .min(1000)
}

/// One notation grid does not fit every passage: an Adagio written in
/// thirty-seconds puts two attacks inside one sixteenth-note slot, and rounding
/// them together would hide half the movement. Where a stream really did play
/// two separated attacks in one slot, the slot alone is subdivided; the rest of
/// the piece keeps the coarse grid, so a rolled chord still reads as one attack.
fn refine_dense_slots(
    notes: &mut [PerformanceNote],
    options: MidiImportOptions,
    sequence: usize,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let strength = Rational::new(i64::from(options.quantize_strength_milli), 1000)?;
    let grid = options.quantize_grid;
    let fine = Duration::from_rational(grid.0.checked_div(Rational::new(2, 1)?)?)?;
    let separation = grid.0.checked_div(Rational::new(4, 1)?)?;
    let mut streams: BTreeMap<(usize, u8), Vec<usize>> = BTreeMap::new();
    for (index, note) in notes.iter().enumerate() {
        streams
            .entry((note.track, note.channel))
            .or_default()
            .push(index);
    }
    for indices in streams.values() {
        let mut slots: BTreeMap<ScoreTime, Vec<usize>> = BTreeMap::new();
        for &index in indices {
            slots.entry(notes[index].grid_onset).or_default().push(index);
        }
        let occupied = slots.keys().copied().collect::<BTreeSet<_>>();
        for (slot, members) in &slots {
            if members.len() < 2 {
                continue;
            }
            let earliest = members
                .iter()
                .map(|&index| notes[index].played_onset)
                .min()
                .expect("non-empty slot");
            let latest = members
                .iter()
                .map(|&index| notes[index].played_onset)
                .max()
                .expect("non-empty slot");
            // A tuplet slot is already on its own grid; halving the duple grid
            // under it would only turn a triplet into a wrong duple value.
            if latest.0.checked_sub(earliest.0)? < separation
                || members.iter().any(|&index| notes[index].tuplet)
            {
                continue;
            }
            let mut placed = Vec::with_capacity(members.len());
            for &index in members {
                placed.push((index, snap(notes[index].played_onset, fine)?));
            }
            let distinct = placed.iter().map(|(_, at)| *at).collect::<BTreeSet<_>>();
            // A refined position must stay inside its own slot's neighbourhood:
            // landing on a slot that already has notes would merge them instead.
            let escapes = placed
                .iter()
                .any(|(_, at)| at != slot && occupied.contains(at));
            if distinct.len() < 2 || escapes {
                continue;
            }
            for (index, at) in placed {
                let end = snap(notes[index].played_end, fine)?.max(at.checked_add(fine)?);
                notes[index].grid_onset = at;
                notes[index].onset = blend_time(notes[index].played_onset, at, strength)?;
                notes[index].end = blend_time(notes[index].played_end, end, strength)?;
            }
            report.repaired(
                "midi.subdivided-slot",
                "two separate attacks landed in one grid slot; that slot was quantized on a finer grid so both stayed real notes",
                midi_location(sequence, Some(notes[members[0]].track), None, None),
            );
        }
    }
    Ok(())
}

/// Quantization can shrink an ornamental note to nothing, and it can land two
/// notes of one stream on the same grid point where the later would swallow the
/// earlier. Neither is allowed to make a note disappear: a collapsed note keeps
/// a real, minimal written value when its slot is its own, moves back one grid
/// unit into a free slot when a same-pitch neighbour claims the slot, and
/// otherwise becomes a grace note in front of the note it collided with.
fn resolve_collapsed_notes(
    notes: &mut [PerformanceNote],
    options: MidiImportOptions,
    sequence: usize,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let grid = options.quantize_grid;
    let mut streams: BTreeMap<(usize, u8), Vec<usize>> = BTreeMap::new();
    for (index, note) in notes.iter().enumerate() {
        streams
            .entry((note.track, note.channel))
            .or_default()
            .push(index);
    }
    for indices in streams.values() {
        let mut occupied: BTreeSet<(ScoreTime, u8)> = indices
            .iter()
            .map(|&index| (notes[index].grid_onset, notes[index].key))
            .collect();
        // An attack this much later than another on the same grid point was
        // played ahead of the beat, not rolled as part of one chord.
        let ornament_gap = grid.0.checked_div(Rational::new(4, 1)?)?;
        for &index in indices {
            let collapsed = notes[index].end <= notes[index].onset;
            // Another note of this stream that starts later in the performance
            // but shares the grid point would absorb this one.
            let shadowed = indices.iter().any(|&other| {
                if other == index || notes[other].grid_onset != notes[index].grid_onset {
                    return false;
                }
                if notes[other].key == notes[index].key {
                    return other > index;
                }
                notes[other]
                    .played_onset
                    .0
                    .checked_sub(notes[index].played_onset.0)
                    .is_ok_and(|delta| delta >= ornament_gap)
            });
            if !collapsed && !shadowed {
                continue;
            }
            if !collapsed && shadowed {
                // A repeated pitch that quantized onto its own predecessor: the
                // second strike would vanish behind the first notehead.
                let earlier = ScoreTime(notes[index].grid_onset.0.checked_sub(grid.0)?);
                if earlier.0.numerator() >= 0
                    && occupied.insert((earlier, notes[index].key))
                {
                    occupied.remove(&(notes[index].grid_onset, notes[index].key));
                    notes[index].grid_onset = earlier;
                    notes[index].onset = earlier;
                    notes[index].end = earlier.checked_add(grid)?;
                    report.repaired(
                        "midi.repeated-note-collision",
                        "two strikes of one pitch quantized onto the same grid point; the first was moved back one grid unit so it stays a separate note",
                        midi_location(sequence, Some(notes[index].track), None, None),
                    );
                    continue;
                }
                notes[index].grace = true;
                notes[index].end = notes[index].onset;
                report.repaired(
                    "midi.grace-note",
                    "a repeated pitch quantized onto the following note and no free grid slot was left; it was notated as a grace note",
                    midi_location(sequence, Some(notes[index].track), None, None),
                );
                continue;
            }
            if shadowed {
                notes[index].grace = true;
                notes[index].end = notes[index].onset;
                report.repaired(
                    "midi.grace-note",
                    "a note shorter than the quantization grid shared its grid point with the note it decorates; it was notated as a grace note",
                    midi_location(sequence, Some(notes[index].track), None, None),
                );
                continue;
            }
            notes[index].end = notes[index].onset.checked_add(grid)?;
            report.repaired(
                "midi.short-note-lengthened",
                "a note shorter than half the quantization grid was notated at one grid unit rather than dropped",
                midi_location(sequence, Some(notes[index].track), None, None),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Hand assignment
// ---------------------------------------------------------------------------

/// Reach a hand takes without effort; wider costs, and wider than
/// [`MidiImportOptions::max_hand_span_semitones`] costs a great deal more.
const SPAN_COMFORT_SEMITONES: i64 = 10;
const COST_OVER_COMFORT: i64 = 60;
const COST_OVER_MAX: i64 = 300;
/// Per note beyond five in one simultaneity: a hand has five fingers.
const COST_EXTRA_FINGER: i64 = 250;
/// Per note the left hand reaches over the right. Crossing is allowed, and the
/// corpus does infer it, but it has to beat the uncrossed reading outright.
const COST_CROSSING: i64 = 220;
/// Per semitone a note sits on the far side of the piece's own median pitch.
/// Weak on purpose: it only decides notes that continuity leaves open.
const COST_REGISTER_PRIOR: i64 = 6;
/// Per semitone between where a hand was and where it must now be.
const COST_HAND_GAP: i64 = 2;
const COST_HAND_TRAVEL: i64 = 24;
/// Per note a hand's chord grows or shrinks by. A hand keeps its texture: this
/// is what lets a hand reach over rather than swap roles with the other.
const COST_HAND_SHAPE: i64 = 100;
/// Per hand that starts or stops playing between two simultaneities.
const COST_HAND_ALTERNATION: i64 = 40;
const FINGERS: usize = 5;

/// Reads a hand out of a track or instrument name. Matching is by word, not by
/// substring: a `Bright Acoustic Piano` patch name is not a right hand.
fn hand_from_name(name: &str) -> Option<Hand> {
    let lower = name.to_ascii_lowercase();
    let words = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let has = |needles: &[&str]| {
        words
            .iter()
            .any(|word| needles.iter().any(|needle| word.starts_with(needle)))
    };
    // `l.h.` and `r.h.` split into single letters, so they are read in sequence.
    let initials = |first: &str| {
        words
            .windows(2)
            .any(|pair| pair[0] == first && pair[1] == "h")
    };
    for (left, right) in [
        (
            &["left", "lh", "linke", "gauche", "sinistra", "izquierda"][..],
            &["right", "rh", "rechte", "droite", "destra", "derecha"][..],
        ),
        (&["lower"][..], &["upper"][..]),
        (&["bass"][..], &["treble"][..]),
    ] {
        match (has(left) || initials("l"), has(right) || initials("r")) {
            (true, false) => return Some(Hand::Left),
            (false, true) => return Some(Hand::Right),
            _ => {}
        }
    }
    None
}

fn track_name(file: &MidiFile, track: usize) -> Option<String> {
    let track = file.tracks.get(track)?;
    track.events.iter().find_map(|event| match &event.kind {
        MidiEventKind::Meta(
            MetaEvent::SequenceOrTrackName(bytes) | MetaEvent::InstrumentName(bytes),
        ) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    })
}

#[derive(Clone, Copy, Debug)]
struct StreamSummary {
    notes: usize,
    pitch_sum: i64,
    first: ScoreTime,
    last: ScoreTime,
}

/// Reads whatever hand information the file states outright, then infers the
/// rest. Explicit evidence is never overridden: labelled notes become hard
/// constraints that the cost-based pass must respect.
fn assign_hands(
    notes: &mut [PerformanceNote],
    file: &MidiFile,
    options: MidiImportOptions,
    sequence: usize,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    if notes.is_empty() {
        report.inferences.push(Inference {
            kind: InferenceKind::StaffSplit,
            confidence_milli: 1000,
            detail: "no notes to place on a staff".to_string(),
            source: midi_location(sequence, None, None, None),
        });
        return Ok(());
    }
    let mut streams: BTreeMap<(usize, u8), StreamSummary> = BTreeMap::new();
    for note in notes.iter() {
        let entry = streams
            .entry((note.track, note.channel))
            .or_insert(StreamSummary {
                notes: 0,
                pitch_sum: 0,
                first: note.grid_onset,
                last: note.grid_onset,
            });
        entry.notes += 1;
        entry.pitch_sum += i64::from(note.key);
        entry.first = entry.first.min(note.grid_onset);
        entry.last = entry.last.max(note.end);
    }

    let (explicit, evidence) = explicit_hands(&streams, file, notes.len());
    let explicit_count = explicit.values().map(|(_, count)| count).sum::<usize>();
    let inferred_count = notes.len() - explicit_count;
    let mut per_note: Vec<Option<Hand>> = notes
        .iter()
        .map(|note| explicit.get(&(note.track, note.channel)).map(|(hand, _)| *hand))
        .collect();
    let inference_cost = if inferred_count > 0 {
        infer_hands(notes, &mut per_note, options)?
    } else {
        0
    };
    for (note, hand) in notes.iter_mut().zip(&per_note) {
        note.hand = hand.unwrap_or(Hand::Right);
        note.hand_source = if explicit.contains_key(&(note.track, note.channel)) {
            evidence
        } else {
            HandSource::Inferred
        };
    }

    let right = notes.iter().filter(|note| note.hand == Hand::Right).count();
    let left = notes.len() - right;
    let inferred_confidence = if inferred_count == 0 {
        1000
    } else {
        let mean = inference_cost / inferred_count as i64;
        u16::try_from((1000 - mean.clamp(0, 1000)).max(0)).unwrap_or(0)
    };
    let confidence = u16::try_from(
        (explicit_count as i64 * 1000 + inferred_count as i64 * i64::from(inferred_confidence))
            / notes.len() as i64,
    )
    .unwrap_or(0)
    .min(1000);
    let source = match evidence {
        HandSource::TrackName => "named tracks",
        HandSource::StreamLayout => "the file's two note streams",
        HandSource::Inferred => "no explicit hand information",
    };
    report.inferences.push(Inference {
        kind: InferenceKind::StaffSplit,
        confidence_milli: confidence,
        detail: format!(
            "{source} fixed {explicit_count} notes; {inferred_count} were placed by \
             span/continuity cost minimisation (max span {} semitones, crossing allowed); \
             {right} right-hand and {left} left-hand notes",
            options.max_hand_span_semitones
        ),
        source: midi_location(sequence, None, None, None),
    });
    Ok(())
}

/// Explicit hand evidence, in order of trustworthiness: track (or instrument)
/// names that say which hand plays, then a file that separates its notes into
/// exactly two concurrent streams by track or channel — the classic
/// two-staff piano export. Anything else is left to inference.
fn explicit_hands(
    streams: &BTreeMap<(usize, u8), StreamSummary>,
    file: &MidiFile,
    total: usize,
) -> (BTreeMap<(usize, u8), (Hand, usize)>, HandSource) {
    let mut named: BTreeMap<(usize, u8), (Hand, usize)> = BTreeMap::new();
    let mut names: BTreeMap<usize, Option<Hand>> = BTreeMap::new();
    for (key, summary) in streams {
        let hand = *names
            .entry(key.0)
            .or_insert_with(|| track_name(file, key.0).as_deref().and_then(hand_from_name));
        if let Some(hand) = hand {
            named.insert(*key, (hand, summary.notes));
        }
    }
    let has_left = named.values().any(|(hand, _)| *hand == Hand::Left);
    let has_right = named.values().any(|(hand, _)| *hand == Hand::Right);
    if has_left && has_right {
        return (named, HandSource::TrackName);
    }

    // Two streams that sound over the same stretch of the piece are the two
    // hands of a piano export; the higher one is the right hand.
    if streams.len() == 2 {
        let entries = streams.iter().collect::<Vec<_>>();
        let (left_key, left_summary) = entries[0];
        let (right_key, right_summary) = entries[1];
        let small = left_summary.notes.min(right_summary.notes);
        let overlap = left_summary.last.min(right_summary.last).0
            .checked_sub(left_summary.first.max(right_summary.first).0)
            .unwrap_or(Rational::ZERO);
        let shorter = (left_summary.last.0.checked_sub(left_summary.first.0))
            .and_then(|left| {
                right_summary
                    .last
                    .0
                    .checked_sub(right_summary.first.0)
                    .map(|right| left.min(right))
            })
            .unwrap_or(Rational::ZERO);
        let concurrent = shorter.is_zero()
            || overlap
                .checked_mul(Rational::new(2, 1).expect("constant"))
                .is_ok_and(|doubled| doubled >= shorter);
        let mean_left = left_summary.pitch_sum / left_summary.notes.max(1) as i64;
        let mean_right = right_summary.pitch_sum / right_summary.notes.max(1) as i64;
        if small * 20 >= total && concurrent && (mean_left - mean_right).abs() >= 2 {
            let mut layout = BTreeMap::new();
            let (lower, upper) = if mean_left < mean_right {
                ((left_key, left_summary), (right_key, right_summary))
            } else {
                ((right_key, right_summary), (left_key, left_summary))
            };
            layout.insert(*lower.0, (Hand::Left, lower.1.notes));
            layout.insert(*upper.0, (Hand::Right, upper.1.notes));
            return (layout, HandSource::StreamLayout);
        }
    }
    (BTreeMap::new(), HandSource::Inferred)
}

#[derive(Clone, Copy, Debug)]
struct HandPlace {
    low: i64,
    high: i64,
    centre: i64,
    /// How many notes the hand struck: a hand keeps its texture, so an
    /// accompanying hand that has been holding a chord tends to keep holding it.
    count: i64,
}

#[derive(Clone, Debug)]
struct Candidate {
    /// Bit `i` set means the `i`-th note of the slice is played left handed.
    left_mask: u64,
    cost: i64,
    left: Option<HandPlace>,
    right: Option<HandPlace>,
}

#[derive(Clone, Copy, Debug)]
struct DpCell {
    cost: i64,
    parent: usize,
    left: Option<HandPlace>,
    right: Option<HandPlace>,
    /// Which hands actually struck this simultaneity, as opposed to where they
    /// are still resting from an earlier one.
    played_left: bool,
    played_right: bool,
}

/// Assigns the notes that carry no explicit hand by minimising, over the whole
/// piece, hand travel plus span and crossing penalties. Simultaneities are
/// enumerated as pitch-ordered divisions plus a single-crossing variant of each,
/// so the hands may cross where crossing is genuinely cheaper than the
/// alternative, but never gratuitously. Returns the total inference cost.
fn infer_hands(
    notes: &[PerformanceNote],
    per_note: &mut [Option<Hand>],
    options: MidiImportOptions,
) -> Result<i64, ImportError> {
    let max_span = i64::from(options.max_hand_span_semitones).max(SPAN_COMFORT_SEMITONES);
    let register = register_boundary(notes);
    let mut slices: BTreeMap<ScoreTime, Vec<usize>> = BTreeMap::new();
    for (index, note) in notes.iter().enumerate() {
        slices.entry(note.grid_onset).or_default().push(index);
    }
    let slices = slices.into_iter().collect::<Vec<_>>();

    let mut previous: Vec<DpCell> = Vec::new();
    let mut previous_time = ScoreTime::ZERO;
    let mut chosen: Vec<(Vec<usize>, Vec<Candidate>, Vec<DpCell>)> = Vec::with_capacity(slices.len());
    for (slice_index, (time, members)) in slices.iter().enumerate() {
        let mut members = members.clone();
        members.sort_by_key(|index| (notes[*index].key, notes[*index].source_index));
        let candidates = slice_candidates(notes, &members, per_note, max_span, register);
        let travel = if slice_index == 0 {
            1
        } else {
            travel_divisor(previous_time, *time)
        };
        let mut cells = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            if slice_index == 0 {
                cells.push(DpCell {
                    cost: candidate.cost,
                    parent: 0,
                    left: candidate.left,
                    right: candidate.right,
                    played_left: candidate.left.is_some(),
                    played_right: candidate.right.is_some(),
                });
                continue;
            }
            let mut best = (i64::MAX, 0_usize);
            for (parent, cell) in previous.iter().enumerate() {
                let step = cell.cost.saturating_add(
                    transition_cost(cell, candidate, travel).saturating_add(candidate.cost),
                );
                if step < best.0 {
                    best = (step, parent);
                }
            }
            let parent_cell = previous[best.1];
            cells.push(DpCell {
                cost: best.0,
                parent: best.1,
                left: settle(parent_cell.left, candidate.left),
                right: settle(parent_cell.right, candidate.right),
                played_left: candidate.left.is_some(),
                played_right: candidate.right.is_some(),
            });
        }
        previous = cells.clone();
        previous_time = *time;
        chosen.push((members, candidates, cells));
    }

    let mut total = 0;
    let mut selected = 0_usize;
    if let Some((_, _, cells)) = chosen.last() {
        selected = cells
            .iter()
            .enumerate()
            .min_by_key(|(index, cell)| (cell.cost, *index))
            .map(|(index, _)| index)
            .unwrap_or(0);
        total = cells.get(selected).map_or(0, |cell| cell.cost);
    }
    for slice_index in (0..chosen.len()).rev() {
        let (members, candidates, cells) = &chosen[slice_index];
        let candidate = &candidates[selected];
        for (position, &index) in members.iter().enumerate() {
            if per_note[index].is_none() {
                per_note[index] = Some(if masked_left(candidate.left_mask, position) {
                    Hand::Left
                } else {
                    Hand::Right
                });
            }
        }
        selected = cells[selected].parent;
    }
    Ok(total)
}

/// A soft register boundary from the piece's own pitch distribution, used only
/// as a weak prior: it never decides a note on its own.
fn register_boundary(notes: &[PerformanceNote]) -> i64 {
    let mut keys = notes.iter().map(|note| i64::from(note.key)).collect::<Vec<_>>();
    keys.sort_unstable();
    // The piece's own median, kept within reach of middle C: a piece that lives
    // entirely above the staff still has a right hand and a resting left one.
    keys[keys.len() / 2].clamp(54, 66)
}

/// Whether the `position`-th note of a slice is left handed under `mask`.
/// A simultaneity of more than sixty-four notes has no piano reading; the
/// overflow keeps the register default rather than panicking on the shift.
fn masked_left(mask: u64, position: usize) -> bool {
    position < 64 && mask >> position & 1 == 1
}

fn slice_candidates(
    notes: &[PerformanceNote],
    members: &[usize],
    per_note: &[Option<Hand>],
    max_span: i64,
    register: i64,
) -> Vec<Candidate> {
    let count = members.len();
    let mut masks: Vec<u64> = Vec::new();
    if count <= 63 {
        for split in 0..=count {
            masks.push(if split == 0 { 0 } else { (1_u64 << split) - 1 });
        }
        // Crossings. Two hands at one instant nearly always divide the pitches
        // in order, so the divisions above are the ordinary reading — but real
        // piano writing does cross, in three shapes: a single exchanged note,
        // the left hand reaching over the right for the top note, and the right
        // hand reaching under the left for the bottom one.
        for split in 1..count {
            let base = (1_u64 << split) - 1;
            masks.push(base & !(1 << (split - 1)) | 1 << split);
        }
        for split in 0..count.saturating_sub(1) {
            let base = if split == 0 { 0 } else { (1_u64 << split) - 1 };
            masks.push(base | 1 << (count - 1));
        }
        for split in 2..=count {
            masks.push(((1_u64 << split) - 1) & !1);
        }
    }
    // The layout the explicit labels already ask for is always available, so a
    // labelled crossing can never be discarded for want of a candidate.
    let mut forced = 0_u64;
    for (position, &index) in members.iter().enumerate() {
        let hand = per_note[index].unwrap_or_else(|| {
            if i64::from(notes[index].key) < register {
                Hand::Left
            } else {
                Hand::Right
            }
        });
        if hand == Hand::Left && position < 64 {
            forced |= 1 << position;
        }
    }
    masks.push(forced);
    masks.sort_unstable();
    masks.dedup();

    masks
        .into_iter()
        .filter(|mask| {
            members.iter().enumerate().all(|(position, &index)| {
                match per_note[index] {
                    Some(hand) => masked_left(*mask, position) == (hand == Hand::Left),
                    None => true,
                }
            })
        })
        .map(|mask| candidate_for(notes, members, mask, max_span, register))
        .collect()
}

fn candidate_for(
    notes: &[PerformanceNote],
    members: &[usize],
    left_mask: u64,
    max_span: i64,
    register: i64,
) -> Candidate {
    let mut left_keys = Vec::new();
    let mut right_keys = Vec::new();
    let mut cost = 0;
    for (position, &index) in members.iter().enumerate() {
        let key = i64::from(notes[index].key);
        if masked_left(left_mask, position) {
            left_keys.push(key);
            cost += COST_REGISTER_PRIOR * (key - register).max(0);
        } else {
            right_keys.push(key);
            cost += COST_REGISTER_PRIOR * (register - key).max(0);
        }
    }
    // A crossing is the left hand reaching over the right in one simultaneity:
    // allowed, and priced by how many notes reached over, not by how many pairs
    // that makes — one hand over another is one gesture however wide the chord
    // underneath it happens to be.
    let crossings = right_keys.iter().min().map_or(0, |lowest| {
        left_keys.iter().filter(|left| *left > lowest).count()
    });
    cost += COST_CROSSING * crossings as i64;
    let place = |keys: &[i64]| -> Option<HandPlace> {
        let low = *keys.iter().min()?;
        let high = *keys.iter().max()?;
        Some(HandPlace {
            low,
            high,
            centre: keys.iter().sum::<i64>() / keys.len() as i64,
            count: keys.len() as i64,
        })
    };
    for keys in [&left_keys, &right_keys] {
        if keys.is_empty() {
            continue;
        }
        let span = keys.iter().max().copied().unwrap_or(0) - keys.iter().min().copied().unwrap_or(0);
        cost += COST_OVER_COMFORT * (span - SPAN_COMFORT_SEMITONES).max(0);
        cost += COST_OVER_MAX * (span - max_span).max(0);
        cost += COST_EXTRA_FINGER * keys.len().saturating_sub(FINGERS) as i64;
    }
    Candidate {
        left_mask,
        cost,
        left: place(&left_keys),
        right: place(&right_keys),
    }
}

/// How much cheaper travel gets when the hand has time to move. A long rest
/// between two simultaneities means the leap costs the player almost nothing.
fn travel_divisor(previous: ScoreTime, current: ScoreTime) -> i64 {
    let Ok(delta) = current.0.checked_sub(previous.0) else {
        return 1;
    };
    let half = Rational::new(1, 2).expect("constant");
    if delta >= Rational::ONE {
        4
    } else if delta >= half {
        2
    } else {
        1
    }
}

/// Where a hand is after playing this simultaneity. The centre lags behind the
/// attack so a single reach does not redefine the hand's whole region; the
/// extremes are the notes actually just played.
fn settle(previous: Option<HandPlace>, current: Option<HandPlace>) -> Option<HandPlace> {
    match (previous, current) {
        (Some(previous), Some(current)) => Some(HandPlace {
            centre: (previous.centre + current.centre * 2) / 3,
            ..current
        }),
        (None, current) => current,
        (previous, None) => previous,
    }
}

fn transition_cost(previous: &DpCell, candidate: &Candidate, travel: i64) -> i64 {
    // Handing a line from one hand to the other is a decision a player makes on
    // purpose. Without a price for it, a single narrow melody would be dealt out
    // between the staves note by note, because each hand then barely moves.
    let mut cost = COST_HAND_ALTERNATION
        * (i64::from(previous.played_left != candidate.left.is_some())
            + i64::from(previous.played_right != candidate.right.is_some()));
    for (before, after) in [
        (previous.left, candidate.left),
        (previous.right, candidate.right),
    ] {
        let (Some(before), Some(after)) = (before, after) else {
            continue;
        };
        let gap = (after.low - before.high).max(before.low - after.high).max(0);
        cost += COST_HAND_GAP * gap
            + COST_HAND_TRAVEL * (after.centre - before.centre).abs()
            + COST_HAND_SHAPE * (after.count - before.count).abs();
    }
    cost / travel
}

fn infer_key(
    file: &MidiFile,
    sequence: usize,
    notes: &[PerformanceNote],
) -> Result<(i8, bool, u16, bool), ImportError> {
    let map = file.key_signature_map_for_sequence(sequence)?;
    if let Some(change) = map.changes.first() {
        return Ok((
            change.signature.sharps_flats,
            change.signature.is_minor,
            1000,
            true,
        ));
    }
    const MAJOR: [i64; 12] = [635, 223, 348, 233, 438, 409, 252, 519, 239, 366, 229, 288];
    const MINOR: [i64; 12] = [633, 268, 352, 538, 260, 353, 254, 475, 398, 269, 334, 317];
    let mut counts = [0_i64; 12];
    for note in notes {
        counts[usize::from(note.key % 12)] += i64::from(note.velocity.max(1));
    }
    let mut candidates = Vec::new();
    for fifths in -7_i8..=7 {
        let major_tonic = (i16::from(fifths) * 7).rem_euclid(12) as usize;
        let minor_tonic = (major_tonic + 9) % 12;
        let major_score = profile_score(&counts, &MAJOR, major_tonic) - i64::from(fifths.abs()) * 20;
        let minor_score = profile_score(&counts, &MINOR, minor_tonic) - i64::from(fifths.abs()) * 20;
        candidates.push((major_score, fifths, false));
        candidates.push((minor_score, fifths, true));
    }
    candidates.sort_by_key(|candidate| candidate.0);
    let best = candidates.last().copied().unwrap_or((0, 0, false));
    let second = candidates.iter().rev().nth(1).copied().unwrap_or(best);
    let margin = best.0.saturating_sub(second.0);
    let confidence = if best.0 <= 0 {
        0
    } else {
        u16::try_from((margin.saturating_mul(4000) / best.0).clamp(0, 1000)).unwrap_or(0)
    };
    Ok((best.1, best.2, confidence, false))
}

fn profile_score(counts: &[i64; 12], profile: &[i64; 12], tonic: usize) -> i64 {
    (0..12)
        .map(|pitch_class| counts[pitch_class] * profile[(pitch_class + 12 - tonic) % 12])
        .sum()
}

fn infer_meter(
    file: &MidiFile,
    sequence: usize,
    notes: &[PerformanceNote],
) -> Result<(Meter, u16, bool), ImportError> {
    let map = file.time_signature_map_for_sequence(sequence)?;
    if let Some(change) = map.changes.first() {
        let denominator = change.signature.denominator().ok_or_else(|| {
            ImportError::InvalidSource("MIDI time-signature denominator overflow".to_string())
        })?;
        return Ok((
            Meter::Measured {
                groups: vec![u16::from(change.signature.numerator)],
                unit: u16::try_from(denominator).map_err(|_| {
                    ImportError::InvalidSource("MIDI time-signature denominator too large".to_string())
                })?,
            },
            1000,
            true,
        ));
    }
    let candidates = [
        Meter::Measured {
            groups: vec![3],
            unit: 4,
        },
        Meter::Measured {
            groups: vec![4],
            unit: 4,
        },
        Meter::Measured {
            groups: vec![6],
            unit: 8,
        },
    ];
    let mut scored = candidates
        .into_iter()
        .map(|meter| {
            let length = meter.duration().ok().flatten().expect("candidate meter");
            let score = notes
                .iter()
                .map(|note| {
                    let position = modulo_rational(note.onset.0, length.0);
                    if position.is_zero() {
                        i64::from(note.velocity) * 4
                    } else if is_beat_boundary(position, &meter) {
                        i64::from(note.velocity) * 2
                    } else {
                        i64::from(note.velocity)
                    }
                })
                .sum::<i64>();
            (score, meter)
        })
        .collect::<Vec<_>>();
    scored.sort_by_key(|item| item.0);
    let (best_score, best) = scored.pop().expect("candidate meters");
    let second_score = scored.last().map_or(best_score, |item| item.0);
    let confidence = if best_score <= 0 {
        0
    } else {
        u16::try_from(((best_score - second_score) * 3000 / best_score).clamp(0, 850))
            .unwrap_or(0)
    };
    Ok((best, confidence, false))
}

fn modulo_rational(value: Rational, modulus: Rational) -> Rational {
    let quotient = value.checked_div(modulus).expect("positive meter");
    let floor = quotient.numerator().div_euclid(quotient.denominator() as i64);
    value
        .checked_sub(
            modulus
                .checked_mul(Rational::new(floor, 1).expect("integer"))
                .expect("meter arithmetic"),
        )
        .expect("meter arithmetic")
}

fn is_beat_boundary(position: Rational, meter: &Meter) -> bool {
    let Meter::Measured { unit, .. } = meter else {
        return false;
    };
    let beat = Rational::new(1, u64::from(*unit)).expect("positive meter unit");
    modulo_rational(position, beat).is_zero()
}

#[allow(clippy::too_many_arguments)]
fn build_midi_score(
    score: &mut makepad_score::model::Score,
    notes: &[PerformanceNote],
    fifths: i8,
    meter: Meter,
    options: MidiImportOptions,
    sequence: usize,
    report: &mut ImportReport,
) -> Result<Vec<PerformedNote>, ImportError> {
    let part_id = stable_id::<PartTag>("part", None, &format!("midi/{sequence}/piano"));
    let upper_staff = stable_id::<StaffTag>("staff", None, &format!("midi/{sequence}/upper"));
    let lower_staff = stable_id::<StaffTag>("staff", None, &format!("midi/{sequence}/lower"));
    score.parts.insert(
        part_id,
        Part {
            id: part_id,
            name: "Piano".to_string(),
            staves: vec![upper_staff, lower_staff],
            transposition: Transposition::NONE,
        },
    );
    for staff in [upper_staff, lower_staff] {
        score.staves.insert(
            staff,
            Staff {
                id: staff,
                part: part_id,
                parent: None,
                kind: StaffKind::Standard,
                voices: Vec::new(),
            },
        );
    }
    report.imported("MIDI piano part");

    let meter_duration = meter
        .duration()?
        .unwrap_or(Duration::new(1, 1)?) ;
    // The shortest silence worth writing out as a rest between two notes of one
    // voice: anything shorter reads as articulation on the earlier note.
    let beat = match &meter {
        Meter::Measured { unit, .. } => Duration::new(1, u64::from(*unit))?,
        Meter::Free => Duration::new(1, 4)?,
    };
    let max_end = notes
        .iter()
        .map(|note| note.end)
        .max()
        .unwrap_or(ScoreTime(meter_duration.0));
    let measure_count = ceil_ratio(max_end.0, meter_duration.0).max(1);
    for ordinal in 0..measure_count {
        let start = ScoreTime(
            meter_duration
                .0
                .checked_mul(Rational::new(i64::from(ordinal), 1)?)?,
        );
        let id = stable_id::<MeasureTag>(
            "measure",
            None,
            &format!("midi/{sequence}/measure/{ordinal}"),
        );
        score.measures.insert(
            id,
            Measure {
                id,
                ordinal,
                label: (ordinal + 1).to_string(),
                start,
                extent: meter_duration,
            },
        );
        score.flow.nodes.push(FlowNode { measure: id, ordinal });
    }
    let score_end = ScoreTime(
        meter_duration
            .0
            .checked_mul(Rational::new(i64::from(measure_count), 1)?)?,
    );

    let upper = notes
        .iter()
        .filter(|note| note.hand == Hand::Right)
        .cloned()
        .collect::<Vec<_>>();
    let lower = notes
        .iter()
        .filter(|note| note.hand == Hand::Left)
        .cloned()
        .collect::<Vec<_>>();

    let upper_voices = separate_voices_with_graces(upper);
    let lower_voices = separate_voices_with_graces(lower);
    report.inferences.push(Inference {
        kind: InferenceKind::VoiceSeparation,
        confidence_milli: voice_confidence(&upper_voices, &lower_voices),
        detail: format!(
            "greedy non-overlap/pitch-continuity separation produced {} upper and {} lower voices",
            upper_voices.len(),
            lower_voices.len()
        ),
        source: midi_location(sequence, None, None, None),
    });
    let total_voices = upper_voices.len() + lower_voices.len();
    let mut voice_ordinal = 0_u16;
    let mut performed = Vec::with_capacity(notes.len());
    for (staff, clef, hands) in [
        (upper_staff, Clef::G, upper_voices),
        (lower_staff, Clef::F, lower_voices),
    ] {
        for hand in hands {
            voice_ordinal = voice_ordinal.saturating_add(1);
            let voice_id = stable_id::<VoiceTag>(
                "voice",
                None,
                &format!("midi/{sequence}/voice/{voice_ordinal}"),
            );
            let mut events = build_voice_events(
                &hand,
                voice_id,
                staff,
                fifths,
                meter_duration,
                score_end,
                beat,
                sequence,
                options,
                &mut performed,
                report,
            )?;
            events.push(TimedEvent {
                id: stable_id::<EventTag>(
                    "event",
                    None,
                    &format!("midi/{sequence}/voice/{voice_ordinal}/clef"),
                ),
                onset: ScoreTime::ZERO,
                duration: None,
                grace: None,
                kind: EventKind::Clef(ClefChange {
                    clef,
                    line: if clef == Clef::F { 4 } else { 2 },
                }),
                beams: Vec::new(),
                tuplets: Vec::new(),
                articulations: Vec::new(),
                ornaments: Vec::new(),
            });
            events.sort_by_key(|event| (event.onset, event.id));
            score.voices.insert(
                voice_id,
                Voice {
                    id: voice_id,
                    staff,
                    number: voice_ordinal,
                    events,
                },
            );
            score
                .staves
                .get_mut(&staff)
                .expect("created staff")
                .voices
                .push(voice_id);
        }
    }
    if total_voices == 0 {
        report.approximated(
            "midi.empty-performance",
            "MIDI contained no paired notes; an empty piano score was created",
            midi_location(sequence, None, None, None),
        );
    }
    performed.sort_by_key(|note| note.source_index);
    Ok(performed)
}

/// Grace notes carry no written extent, so they take no part in the
/// non-overlap reasoning: the sounding notes are separated first and each grace
/// note then joins the voice it decorates.
fn separate_voices_with_graces(notes: Vec<PerformanceNote>) -> Vec<Vec<PerformanceNote>> {
    let (graces, sounding): (Vec<_>, Vec<_>) = notes.into_iter().partition(|note| note.grace);
    let mut voices = separate_voices(sounding);
    for grace in graces {
        let target = voices
            .iter()
            .enumerate()
            .filter_map(|(index, voice)| {
                let closest = voice
                    .iter()
                    .filter(|note| note.grid_onset >= grace.grid_onset)
                    .min_by_key(|note| {
                        (
                            note.grid_onset,
                            i16::from(note.key).abs_diff(i16::from(grace.key)),
                        )
                    })?;
                Some((
                    index,
                    closest.grid_onset,
                    i16::from(closest.key).abs_diff(i16::from(grace.key)),
                ))
            })
            .min_by_key(|(_, onset, distance)| (*onset, *distance))
            .map(|(index, _, _)| index);
        match target {
            Some(index) => voices[index].push(grace),
            None => voices.push(vec![grace]),
        }
    }
    for voice in &mut voices {
        voice.sort_by_key(|note| {
            (
                note.grid_onset,
                !note.grace,
                note.onset,
                note.end,
                note.key,
                note.source_index,
            )
        });
    }
    voices
}

/// Splits one hand into voices. A performance overlaps consecutive notes of a
/// single line — pedalled legato holds the previous key past the next attack —
/// so a small overlap keeps the line together and only a genuinely sustained
/// note against moving ones opens a second voice.
fn separate_voices(mut notes: Vec<PerformanceNote>) -> Vec<Vec<PerformanceNote>> {
    notes.sort_by_key(|note| (note.grid_onset, note.onset, note.end, note.key, note.source_index));
    let mut voices: Vec<Vec<PerformanceNote>> = Vec::new();
    for note in notes {
        let candidate = voices
            .iter()
            .enumerate()
            .filter_map(|(index, voice)| {
                let last = voice.last()?;
                let compatible = if last.grid_onset == note.grid_onset {
                    // One attack. A chord carries a single written value, so the
                    // notes must have been released together within a factor of
                    // two; a held note against moving ones is real polyphony.
                    let last_length = last.end.checked_sub(last.onset).unwrap_or(ScoreTime::ZERO);
                    let length = note.end.checked_sub(note.onset).unwrap_or(ScoreTime::ZERO);
                    let (short, long) = if last_length <= length {
                        (last_length, length)
                    } else {
                        (length, last_length)
                    };
                    short
                        .0
                        .checked_mul(Rational::new(2, 1).expect("constant"))
                        .is_ok_and(|doubled| doubled >= long.0)
                } else if last.end <= note.onset {
                    true
                } else {
                    // Overlap is legato, not polyphony, while it stays under
                    // half the sounding length of the note being overlapped.
                    let overlap = last.end.checked_sub(note.onset).unwrap_or(ScoreTime::ZERO);
                    let length = last.end.checked_sub(last.onset).unwrap_or(ScoreTime::ZERO);
                    last.grid_onset < note.grid_onset
                        && overlap
                            .0
                            .checked_mul(Rational::new(2, 1).expect("constant"))
                            .is_ok_and(|doubled| doubled <= length.0)
                };
                compatible.then_some((
                    index,
                    i16::from(last.key).abs_diff(i16::from(note.key)),
                ))
            })
            .min_by_key(|(_, cost)| *cost)
            .map(|(index, _)| index);
        if let Some(index) = candidate {
            voices[index].push(note);
        } else {
            voices.push(vec![note]);
        }
    }
    voices
}

#[derive(Clone)]
struct NotePiece {
    source_index: usize,
    piece_index: usize,
    onset: ScoreTime,
    duration: Duration,
    pitch: Pitch,
    velocity: u8,
    tuplet: bool,
    grace: bool,
    tie_from: Option<NoteId>,
    tie_to: Option<NoteId>,
    note_id: NoteId,
}

/// The written value of every note of one voice. A performance release is not a
/// note value — a staccato sixteenth is released after a thirty-second, and
/// writing that back gives a thirty-second with three beams — so the value comes
/// from quantized onset-to-onset timing, rounded onto the notation grid. `None`
/// marks a grace note, which has no written extent.
fn written_values(
    notes: &[PerformanceNote],
    beat: Duration,
    options: MidiImportOptions,
) -> Result<Vec<Option<Duration>>, ImportError> {
    let mut sounding: BTreeMap<ScoreTime, Rational> = BTreeMap::new();
    for note in notes.iter().filter(|note| !note.grace) {
        let length = note.end.0.checked_sub(note.onset.0)?;
        let entry = sounding.entry(note.onset).or_insert(length);
        if length > *entry {
            *entry = length;
        }
    }
    // One written value per attack: the notes of a chord share it, so they stay
    // one event rather than splintering on their individual releases.
    let onsets = sounding.keys().copied().collect::<Vec<_>>();
    let mut values: BTreeMap<ScoreTime, Duration> = BTreeMap::new();
    let mut previous_span: Option<Rational> = None;
    for (position, onset) in onsets.iter().enumerate() {
        let span = match onsets.get(position + 1) {
            Some(next) => Some(next.0.checked_sub(onset.0)?),
            // Nothing follows the last note to mark out its slot, so it keeps
            // the pulse the passage just established — unless it is held longer,
            // in which case its own length wins.
            None => match previous_span {
                Some(previous) => Some(previous.max(notatable_at_least(sounding[onset])?.0)),
                None => None,
            },
        };
        previous_span = span;
        values.insert(
            *onset,
            written_duration(sounding[onset], span, beat, options)?,
        );
    }
    Ok(notes
        .iter()
        .map(|note| {
            if note.grace {
                None
            } else {
                values.get(&note.onset).copied()
            }
        })
        .collect())
}

/// Decides between "the player detached the note" and "the composer wrote a
/// rest". A performance cannot distinguish the two outright — a staccato
/// quarter and a written eighth followed by an eighth rest sound the same — so
/// the silence is only written out when it is both a substantial part of the
/// slot and at least one beat long. Shorter silences are articulation, and the
/// note is written up to the next onset.
fn written_duration(
    sounding: Rational,
    span: Option<Rational>,
    beat: Duration,
    options: MidiImportOptions,
) -> Result<Duration, ImportError> {
    let Some(span) = span else {
        return notatable_at_least(sounding);
    };
    let value = notatable_at_least(sounding)?;
    if value.0 >= span {
        return Ok(Duration::from_rational(span)?);
    }
    let silence = span.checked_sub(value.0)?;
    let allowance = span.checked_mul(Rational::new(
        i64::from(options.articulation_fill_ratio_milli),
        1000,
    )?)?;
    if silence <= allowance && silence < beat.0 {
        return Ok(Duration::from_rational(span)?);
    }
    Ok(value)
}

/// The shortest ordinary written value that is at least `value`. A length that
/// is already a written value — a triplet one included — keeps itself; anything
/// else rounds up on the plain duple values, because a duple length must never
/// be talked into a tuplet. Longer than a longa keeps its own length and is
/// split at the barline like any other note.
fn notatable_at_least(value: Rational) -> Result<Duration, ImportError> {
    if written_values_table(true)?.contains(&value) {
        return Ok(Duration::from_rational(value)?);
    }
    let mut best: Option<Rational> = None;
    for candidate in written_values_table(false)? {
        if candidate >= value && best.is_none_or(|best| candidate < best) {
            best = Some(candidate);
        }
    }
    Ok(Duration::from_rational(best.unwrap_or(value))?)
}

/// Every ordinary written value, longest first: powers of two from a longa down
/// to a hundred-twenty-eighth, with up to two dots, and — with `tuplets` — the
/// triplet form of each.
fn written_values_table(tuplets: bool) -> Result<Vec<Rational>, ImportError> {
    let mut values = Vec::with_capacity(60);
    for (numerator, denominator) in [
        (4, 1_u64),
        (2, 1),
        (1, 1),
        (1, 2),
        (1, 4),
        (1, 8),
        (1, 16),
        (1, 32),
        (1, 64),
        (1, 128),
    ] {
        let base = Rational::new(numerator, denominator)?;
        for (dot_numerator, dot_denominator) in [(1, 1_u64), (3, 2), (7, 4)] {
            let dotted = base.checked_mul(Rational::new(dot_numerator, dot_denominator)?)?;
            values.push(dotted);
            if tuplets {
                values.push(dotted.checked_mul(Rational::new(2, 3)?)?);
            }
        }
    }
    values.sort_by(|left, right| right.cmp(left));
    values.dedup();
    Ok(values)
}

/// Splits a written length starting at `onset` into ordinary note values, tied
/// together by the caller. Five sixteenths on a beat becomes a quarter tied to a
/// sixteenth; the same length one sixteenth later becomes a sixteenth tied to a
/// quarter, because a written value has to start where it can be read.
fn notated_pieces(onset: ScoreTime, length: Rational) -> Result<Vec<Duration>, ImportError> {
    let exact = written_values_table(true)?;
    let duple = written_values_table(false)?;
    let mut pieces = Vec::new();
    let mut at = onset.0;
    let mut left = length;
    while left.is_positive() && pieces.len() < 16 {
        if exact.contains(&left) {
            pieces.push(Duration::from_rational(left)?);
            break;
        }
        let aligned = |candidate: &Rational| {
            *candidate < left
                && at
                    .checked_div(*candidate)
                    .is_ok_and(|quotient| quotient.denominator() == 1)
        };
        // Ordinary values first. A length that begins inside a tuplet has no
        // duple value that starts where it does, so tuplet values answer for it.
        let chosen = duple
            .iter()
            .copied()
            .find(aligned)
            .or_else(|| exact.iter().copied().find(aligned));
        let Some(chosen) = chosen else {
            pieces.push(Duration::from_rational(left)?);
            break;
        };
        pieces.push(Duration::from_rational(chosen)?);
        at = at.checked_add(chosen)?;
        left = left.checked_sub(chosen)?;
    }
    if pieces.is_empty() && length.is_positive() {
        pieces.push(Duration::from_rational(length)?);
    }
    Ok(pieces)
}

#[allow(clippy::too_many_arguments)]
fn build_voice_events(
    notes: &[PerformanceNote],
    voice: VoiceId,
    staff: StaffId,
    fifths: i8,
    meter: Duration,
    score_end: ScoreTime,
    beat: Duration,
    sequence: usize,
    options: MidiImportOptions,
    performed: &mut Vec<PerformedNote>,
    report: &mut ImportReport,
) -> Result<Vec<TimedEvent>, ImportError> {
    let pitches = spell_midi_pitches(
        &notes.iter().map(|note| note.key).collect::<Vec<_>>(),
        fifths,
    );
    let written = written_values(notes, beat, options)?;
    let mut pieces = Vec::new();
    for ((note, pitch), written) in notes.iter().zip(pitches).zip(written) {
        let mut onset = note.onset;
        let mut piece_index = 0_usize;
        let mut chain = Vec::new();
        let piece_id = |piece_index: usize| {
            stable_id::<NoteTag>(
                "note",
                None,
                &format!(
                    "midi/{sequence}/track/{}/channel/{}/note/{}/piece/{piece_index}",
                    note.track, note.channel, note.source_index
                ),
            )
        };
        let Some(written) = written else {
            chain.push(NotePiece {
                source_index: note.source_index,
                piece_index: 0,
                onset,
                duration: options.quantize_grid,
                pitch,
                velocity: note.velocity,
                tuplet: note.tuplet,
                grace: true,
                tie_from: None,
                tie_to: None,
                note_id: piece_id(0),
            });
            performed.push(performed_note(note, &chain[0], None));
            pieces.extend(chain);
            continue;
        };
        // Rounding a value up must not push the last note past the bar grid.
        let written_end = note.onset.checked_add(written)?.min(score_end);
        while onset < written_end {
            let bar = floor_ratio(onset.0, meter.0);
            let boundary = ScoreTime(
                meter
                    .0
                    .checked_mul(Rational::new(bar.saturating_add(1), 1)?)?,
            );
            let end = written_end.min(boundary);
            for duration in notated_pieces(onset, end.0.checked_sub(onset.0)?)? {
                chain.push(NotePiece {
                    source_index: note.source_index,
                    piece_index,
                    onset,
                    duration,
                    pitch,
                    velocity: note.velocity,
                    tuplet: note.tuplet,
                    grace: false,
                    tie_from: None,
                    tie_to: None,
                    note_id: piece_id(piece_index),
                });
                onset = onset.checked_add(duration)?;
                piece_index += 1;
            }
            if onset < end {
                onset = end;
            }
        }
        if let Some(first) = chain.first() {
            performed.push(performed_note(note, first, Some(written)));
        }
        for index in 0..chain.len() {
            if index > 0 {
                chain[index].tie_from = Some(chain[index - 1].note_id);
            }
            if index + 1 < chain.len() {
                chain[index].tie_to = Some(chain[index + 1].note_id);
            }
        }
        if chain.len() > 1 {
            report.imported("MIDI barline tie splits");
        }
        pieces.extend(chain);
    }
    report.inferences.push(Inference {
        kind: InferenceKind::PitchSpelling,
        confidence_milli: spelling_confidence(&pitches_for_pieces(&pieces), fifths),
        detail: format!("dynamic-programming spelling in the {fifths}-fifths key context"),
        source: midi_location(sequence, None, None, None),
    });

    pieces.sort_by_key(|piece| (piece.onset, !piece.grace, piece.duration, piece.source_index));
    let mut events = Vec::new();
    for piece in pieces {
        let group = if piece.tuplet && !piece.grace {
            Some(stable_id::<SpannerTag>(
                "tuplet",
                None,
                &format!(
                    "midi/{sequence}/voice/{voice:?}/group/{}",
                    piece.source_index / 3
                ),
            ))
        } else {
            None
        };
        let note = Note {
            id: piece.note_id,
            // How it was actually struck. Notation has nowhere to put a
            // per-note velocity, so it rides hidden on the note and playback
            // reads it back; without this every note of a performance is
            // played at one manufactured dynamic.
            performance: Some(NotePerformance { velocity: piece.velocity.max(1) }),
            written_pitch: Some(piece.pitch),
            unpitched_sound: None,
            display_staff: staff,
            tie_from: piece.tie_from,
            tie_to: piece.tie_to,
            tab: None,
            notehead: Notehead::Normal,
        };
        let piece_duration = (!piece.grace).then_some(piece.duration);
        if let Some(existing) = events.iter_mut().rev().find(|event: &&mut TimedEvent| {
            event.onset == piece.onset
                && event.duration == piece_duration
                && event.grace.is_some() == piece.grace
                && matches!(event.kind, EventKind::Chord(_))
        }) {
            if let EventKind::Chord(notes) = &mut existing.kind {
                notes.push(note);
            }
        } else {
            let mut tuplets = Vec::new();
            if let Some(group) = group {
                tuplets.push(TupletNotation {
                    actual: 3,
                    normal: 2,
                    group,
                    level: 1,
                    bracket: false,
                });
                report.imported("MIDI inferred tuplets");
            }
            events.push(TimedEvent {
                id: stable_id::<EventTag>(
                    "event",
                    None,
                    &format!(
                        "midi/{sequence}/note/{}/piece/{}/event",
                        piece.source_index, piece.piece_index
                    ),
                ),
                onset: piece.onset,
                duration: piece_duration,
                grace: piece.grace.then_some(GraceTiming {
                    position: GracePosition::BeforeBeat,
                    steal: None,
                    slash: true,
                }),
                kind: EventKind::Chord(vec![note]),
                beams: Vec::new(),
                tuplets,
                articulations: Vec::new(),
                ornaments: Vec::new(),
            });
        }
        let _ = piece.velocity;
    }
    events.sort_by_key(|event| (event.onset, event.id));
    fill_rests(&mut events, voice, meter, score_end, sequence)?;
    Ok(events)
}

fn performed_note(
    note: &PerformanceNote,
    piece: &NotePiece,
    written: Option<Duration>,
) -> PerformedNote {
    PerformedNote {
        note: piece.note_id,
        source_index: note.source_index,
        track: note.track,
        channel: note.channel,
        key: note.key,
        velocity: note.velocity,
        played_onset: note.played_onset,
        played_end: note.played_end,
        sounding_onset: note.onset,
        sounding_end: note.end,
        written_onset: piece.onset,
        written_duration: written,
        hand: note.hand,
        hand_source: note.hand_source,
    }
}

fn fill_rests(
    events: &mut Vec<TimedEvent>,
    voice: VoiceId,
    meter: Duration,
    score_end: ScoreTime,
    sequence: usize,
) -> Result<(), ImportError> {
    let mut cursor = ScoreTime::ZERO;
    let sounding = events.clone();
    let mut rests = Vec::new();
    let mut rest_index = 0_usize;
    for event in sounding {
        if event.onset > cursor {
            append_rest_range(
                &mut rests,
                cursor,
                event.onset,
                voice,
                meter,
                sequence,
                &mut rest_index,
            )?;
            // A grace note leaves the cursor where it is, so the gap must be
            // closed here or the next event at this onset fills it twice.
            cursor = event.onset;
        }
        if let Some(duration) = event.duration {
            let end = event.onset.checked_add(duration)?;
            if end > cursor {
                cursor = end;
            }
        }
    }
    if cursor < score_end {
        append_rest_range(
            &mut rests,
            cursor,
            score_end,
            voice,
            meter,
            sequence,
            &mut rest_index,
        )?;
    }
    events.extend(rests);
    events.sort_by_key(|event| (event.onset, event.id));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_rest_range(
    output: &mut Vec<TimedEvent>,
    mut start: ScoreTime,
    end: ScoreTime,
    voice: VoiceId,
    meter: Duration,
    sequence: usize,
    rest_index: &mut usize,
) -> Result<(), ImportError> {
    while start < end {
        let bar = floor_ratio(start.0, meter.0);
        let boundary = ScoreTime(
            meter
                .0
                .checked_mul(Rational::new(bar.saturating_add(1), 1)?)?,
        );
        let rest_end = end.min(boundary);
        let duration = Duration::from_rational(rest_end.0.checked_sub(start.0)?)?;
        output.push(TimedEvent {
            id: stable_id::<EventTag>(
                "event",
                None,
                &format!("midi/{sequence}/voice/{voice:?}/rest/{rest_index}"),
            ),
            onset: start,
            duration: Some(duration),
            grace: None,
            kind: EventKind::Rest,
            beams: Vec::new(),
            tuplets: Vec::new(),
            articulations: Vec::new(),
            ornaments: Vec::new(),
        });
        *rest_index += 1;
        start = rest_end;
    }
    Ok(())
}

/// Contextual MIDI pitch spelling. The dynamic program minimizes deviation
/// from the active key signature, awkward double accidentals, inconsistent
/// repeated notes, and implausible letter distances between melodic notes.
pub fn spell_midi_pitches(keys: &[u8], fifths: i8) -> Vec<Pitch> {
    if keys.is_empty() {
        return Vec::new();
    }
    let candidates = keys
        .iter()
        .map(|key| spelling_candidates(*key, fifths))
        .collect::<Vec<_>>();
    let mut costs = candidates[0]
        .iter()
        .map(|candidate| local_spelling_cost(*candidate, fifths))
        .collect::<Vec<_>>();
    let mut parents = vec![Vec::<usize>::new(); candidates.len()];
    for index in 1..candidates.len() {
        let mut next_costs = Vec::with_capacity(candidates[index].len());
        let mut next_parents = Vec::with_capacity(candidates[index].len());
        for candidate in &candidates[index] {
            let (parent, cost) = candidates[index - 1]
                .iter()
                .enumerate()
                .map(|(parent, previous)| {
                    (
                        parent,
                        costs[parent]
                            + local_spelling_cost(*candidate, fifths)
                            + transition_spelling_cost(
                                keys[index - 1],
                                *previous,
                                keys[index],
                                *candidate,
                            ),
                    )
                })
                .min_by_key(|(_, cost)| *cost)
                .expect("each MIDI key has spelling candidates");
            next_parents.push(parent);
            next_costs.push(cost);
        }
        parents[index] = next_parents;
        costs = next_costs;
    }
    let mut selected = vec![0_usize; keys.len()];
    selected[keys.len() - 1] = costs
        .iter()
        .enumerate()
        .min_by_key(|(_, cost)| **cost)
        .map(|(index, _)| index)
        .unwrap_or(0);
    for index in (1..keys.len()).rev() {
        selected[index - 1] = parents[index][selected[index]];
    }
    selected
        .into_iter()
        .enumerate()
        .map(|(index, selected)| candidates[index][selected])
        .collect()
}

fn spelling_candidates(key: u8, fifths: i8) -> Vec<Pitch> {
    let target = i16::from(key);
    let approximate_octave = i16::from(key / 12) - 1;
    let mut candidates = Vec::new();
    for octave in (approximate_octave - 1)..=(approximate_octave + 1) {
        for step in [Step::C, Step::D, Step::E, Step::F, Step::G, Step::A, Step::B] {
            let natural = (octave + 1) * 12 + natural_semitone(step);
            let alter = target - natural;
            if (-2..=2).contains(&alter) {
                candidates.push(Pitch {
                    step,
                    alter: Alter(Rational::new(i64::from(alter), 1).expect("integer alter")),
                    octave: i8::try_from(octave).unwrap_or(0),
                });
            }
        }
    }
    candidates.sort_by_key(|pitch| local_spelling_cost(*pitch, fifths));
    candidates
}

fn local_spelling_cost(pitch: Pitch, fifths: i8) -> i64 {
    let expected = key_step_alter(pitch.step, fifths);
    let alter = pitch.alter.0.numerator();
    let deviation = (alter - i64::from(expected)).abs();
    let double_penalty = alter.abs().saturating_sub(1) * 80;
    let directional = if fifths > 0 && alter < 0 || fifths < 0 && alter > 0 {
        25
    } else {
        0
    };
    deviation * 35 + double_penalty + directional
}

fn transition_spelling_cost(
    previous_key: u8,
    previous: Pitch,
    key: u8,
    candidate: Pitch,
) -> i64 {
    if previous_key == key {
        return if previous.step == candidate.step
            && previous.alter == candidate.alter
            && previous.octave == candidate.octave
        {
            0
        } else {
            250
        };
    }
    let semitones = i16::from(key).abs_diff(i16::from(previous_key));
    let previous_diatonic = i16::from(previous.octave) * 7 + previous.step.index();
    let candidate_diatonic = i16::from(candidate.octave) * 7 + candidate.step.index();
    let letters = candidate_diatonic.abs_diff(previous_diatonic);
    let ideal = match semitones {
        0 => 0,
        1..=2 => 1,
        3..=4 => 2,
        5..=6 => 3,
        7 => 4,
        8..=9 => 5,
        10..=11 => 6,
        _ => u16::from(semitones / 12) * 7,
    };
    i64::from(letters.abs_diff(ideal)) * 12
}

fn key_step_alter(step: Step, fifths: i8) -> i8 {
    const SHARPS: [Step; 7] = [Step::F, Step::C, Step::G, Step::D, Step::A, Step::E, Step::B];
    const FLATS: [Step; 7] = [Step::B, Step::E, Step::A, Step::D, Step::G, Step::C, Step::F];
    if fifths > 0 && SHARPS[..usize::from(fifths.min(7) as u8)].contains(&step) {
        1
    } else if fifths < 0 && FLATS[..usize::from(fifths.unsigned_abs().min(7))].contains(&step) {
        -1
    } else {
        0
    }
}

fn natural_semitone(step: Step) -> i16 {
    match step {
        Step::C => 0,
        Step::D => 2,
        Step::E => 4,
        Step::F => 5,
        Step::G => 7,
        Step::A => 9,
        Step::B => 11,
    }
}

fn pitches_for_pieces(pieces: &[NotePiece]) -> Vec<Pitch> {
    pieces.iter().map(|piece| piece.pitch).collect()
}

fn spelling_confidence(pitches: &[Pitch], fifths: i8) -> u16 {
    if pitches.is_empty() {
        return 1000;
    }
    let cost: i64 = pitches
        .iter()
        .map(|pitch| local_spelling_cost(*pitch, fifths))
        .sum();
    (1000_i64 - cost / pitches.len() as i64 * 5).clamp(0, 1000) as u16
}

fn import_midi_maps(
    file: &MidiFile,
    sequence: usize,
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    match file.header.division {
        Division::TicksPerQuarter(ppq) if ppq > 0 => {
            let tick_time = |tick: u64| -> Result<ScoreTime, ImportError> {
                Ok(ScoreTime::new(
                    i64::try_from(tick).map_err(|_| {
                        ImportError::InvalidSource("MIDI map tick overflow".to_string())
                    })?,
                    u64::from(ppq) * 4,
                )?)
            };
            let tempo = file.tempo_map_for_sequence(sequence)?;
            for change in tempo.changes {
                if change.microseconds_per_quarter == 0 {
                    report.ignored(
                        "midi.invalid-tempo",
                        "zero microseconds-per-quarter tempo was ignored",
                        midi_location(sequence, None, Some(change.tick), None),
                    );
                    continue;
                }
                score.maps.tempo.push(Change {
                    at: tick_time(change.tick)?,
                    scope: MapScope::Global,
                    value: Tempo::Instant {
                        quarters_per_minute: Rational::new(
                            60_000_000,
                            u64::from(change.microseconds_per_quarter),
                        )?,
                    },
                });
                report.imported("MIDI tempo changes");
            }
            // The sustain pedal. A performance without it is a performance
            // with the dampers nailed down: the notes stop the instant the
            // finger leaves, which is exactly what an engraving-derived
            // playback sounds like. Notation has only a "Ped." span, so the
            // controller's own positions ride in the map beside the tempo.
            let mut pedal_moves = 0usize;
            for track in &file.tracks {
                for event in &track.events {
                    let MidiEventKind::Channel(channel) = &event.kind else { continue };
                    let makepad_midi_file::ChannelMessage::ControlChange { controller: 64, value } = channel.message
                    else {
                        continue;
                    };
                    score.maps.pedal.push(Change {
                        at: tick_time(event.tick)?,
                        scope: MapScope::Global,
                        value: PedalLevel { value },
                    });
                    pedal_moves += 1;
                }
            }
            if pedal_moves > 0 {
                report.imported("MIDI sustain pedal");
            }
            for change in file.time_signature_map_for_sequence(sequence)?.changes {
                if change.tick == 0 {
                    continue;
                }
                report.approximated(
                    "midi.later-meter-change",
                    "later time-signature change was imported into the map, but the first-pass inferred bar grid was not rebarred",
                    midi_location(sequence, None, Some(change.tick), None),
                );
                let Some(denominator) = change.signature.denominator() else {
                    report.ignored(
                        "midi.time-signature-overflow",
                        "time-signature denominator was too large",
                        midi_location(sequence, None, Some(change.tick), None),
                    );
                    continue;
                };
                let Ok(unit) = u16::try_from(denominator) else {
                    continue;
                };
                score.maps.time_signature.push(Change {
                    at: tick_time(change.tick)?,
                    scope: MapScope::Global,
                    value: Meter::Measured {
                        groups: vec![u16::from(change.signature.numerator)],
                        unit,
                    },
                });
            }
            for change in file.key_signature_map_for_sequence(sequence)?.changes {
                if change.tick == 0 {
                    continue;
                }
                report.approximated(
                    "midi.later-key-change",
                    "later key-signature change was imported into the map; pitch spelling used the initial key context",
                    midi_location(sequence, None, Some(change.tick), None),
                );
                score.maps.key.push(Change {
                    at: tick_time(change.tick)?,
                    scope: MapScope::Global,
                    value: KeySignature {
                        fifths: change.signature.sharps_flats,
                        custom: Vec::new(),
                    },
                });
            }
        }
        Division::TicksPerQuarter(_) => {}
        Division::Smpte { .. } => {
            report.approximated(
                "midi.smpte-tempo-map",
                "tempo meta-events in an SMPTE-timed file were retained in raw performance but not projected onto the inferred beat clock",
                midi_location(sequence, None, None, None),
            );
            score.maps.tempo.push(Change {
                at: ScoreTime::ZERO,
                scope: MapScope::Global,
                value: Tempo::Instant {
                    quarters_per_minute: Rational::new(120, 1)?,
                },
            });
        }
    }
    Ok(())
}

fn report_non_notation_events(file: &MidiFile, sequence: usize, report: &mut ImportReport) {
    let tracks: Vec<usize> = if file.header.format == Format::Sequential {
        vec![sequence]
    } else {
        (0..file.tracks.len()).collect()
    };
    let mut seen_codes = BTreeSet::new();
    for track_index in tracks {
        let Some(track) = file.tracks.get(track_index) else {
            continue;
        };
        for (event_index, event) in track.events.iter().enumerate() {
            let (code, description) = match &event.kind {
                MidiEventKind::Channel(channel) => match channel.message {
                    makepad_midi_file::ChannelMessage::NoteOn { .. }
                    | makepad_midi_file::ChannelMessage::NoteOff { .. } => continue,
                    makepad_midi_file::ChannelMessage::ControlChange { .. } => (
                        "midi.control-change",
                        "controller data remains in raw performance but is not notation",
                    ),
                    makepad_midi_file::ChannelMessage::ProgramChange { .. } => (
                        "midi.program-change",
                        "program change remains in raw performance but was not mapped to score instrumentation",
                    ),
                    makepad_midi_file::ChannelMessage::PitchBend { .. } => (
                        "midi.pitch-bend",
                        "pitch bend remains in raw performance; continuous bends are not inferred as notation",
                    ),
                    _ => (
                        "midi.channel-expression",
                        "channel expression remains in raw performance but is not notation",
                    ),
                },
                MidiEventKind::Meta(meta) => match meta {
                    MetaEvent::SetTempo(_)
                    | MetaEvent::TimeSignature(_)
                    | MetaEvent::KeySignature(_)
                    | MetaEvent::SequenceOrTrackName(_)
                    | MetaEvent::EndOfTrack => continue,
                    MetaEvent::Lyric(_) => (
                        "midi.unaligned-lyric",
                        "MIDI lyric text remains raw; no reliable note-syllable alignment was inferred",
                    ),
                    MetaEvent::Marker(_) | MetaEvent::CuePoint(_) => (
                        "midi.marker",
                        "MIDI marker remains raw; playback-flow meaning was not inferred",
                    ),
                    _ => (
                        "midi.meta-event",
                        "meta-event remains in raw performance but has no notation mapping",
                    ),
                },
                MidiEventKind::SysEx(_) => (
                    "midi.sysex",
                    "system-exclusive data remains in raw performance but is not notation",
                ),
            };
            if seen_codes.insert(code) {
                report.ignored(
                    code,
                    description,
                    midi_location(
                        sequence,
                        Some(track_index),
                        Some(event.tick),
                        Some(event_index),
                    ),
                );
            }
        }
    }
}

fn midi_title(file: &MidiFile, sequence: usize) -> Option<String> {
    let tracks: Vec<usize> = if file.header.format == Format::Sequential {
        vec![sequence]
    } else {
        (0..file.tracks.len()).collect()
    };
    tracks.into_iter().find_map(|track| {
        file.tracks.get(track)?.events.iter().find_map(|event| {
            let MidiEventKind::Meta(MetaEvent::SequenceOrTrackName(bytes)) = &event.kind else {
                return None;
            };
            String::from_utf8(bytes.clone()).ok()
        })
    })
}

fn ceil_ratio(value: Rational, unit: Rational) -> u32 {
    let quotient = value.checked_div(unit).expect("positive meter duration");
    let numerator = quotient.numerator().max(0) as u64;
    let denominator = quotient.denominator();
    u32::try_from(numerator.saturating_add(denominator - 1) / denominator).unwrap_or(u32::MAX)
}

fn floor_ratio(value: Rational, unit: Rational) -> i64 {
    let quotient = value.checked_div(unit).expect("positive meter duration");
    quotient
        .numerator()
        .div_euclid(quotient.denominator() as i64)
}

fn voice_confidence(
    upper: &[Vec<PerformanceNote>],
    lower: &[Vec<PerformanceNote>],
) -> u16 {
    let count = upper.len() + lower.len();
    match count {
        0..=2 => 900,
        3..=4 => 750,
        5..=6 => 550,
        _ => 350,
    }
}

fn meter_description(meter: &Meter) -> String {
    match meter {
        Meter::Measured { groups, unit } => format!(
            "{}/{}",
            groups.iter().map(u16::to_string).collect::<Vec<_>>().join("+"),
            unit
        ),
        Meter::Free => "free meter".to_string(),
    }
}

fn midi_location(
    sequence: usize,
    track: Option<usize>,
    tick: Option<u64>,
    event: Option<usize>,
) -> SourceLocation {
    SourceLocation::Midi {
        sequence,
        track,
        tick,
        event,
    }
}
