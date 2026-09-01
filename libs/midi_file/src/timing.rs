use std::collections::{HashMap, VecDeque};

use crate::error::{MidiError, MidiErrorKind, MidiResult};
use crate::model::*;

pub const DEFAULT_MICROSECONDS_PER_QUARTER: u32 = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoChange {
    pub tick: u64,
    pub microseconds_per_quarter: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoMap {
    pub division: Division,
    /// Effective changes, always including the default (or replacement) at tick 0.
    pub changes: Vec<TempoChange>,
    segments: Vec<TempoSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TempoSegment {
    start_tick: u64,
    microseconds_per_quarter: u32,
    /// Elapsed time in microsecond*tick units; divide by ticks-per-quarter.
    elapsed_microtick: u128,
}

impl TempoMap {
    /// Converts an absolute tick to seconds. Metrical accumulation uses integer
    /// microsecond*tick units until the final floating-point division.
    pub fn ticks_to_seconds(&self, tick: u64) -> f64 {
        match self.division {
            Division::TicksPerQuarter(ticks_per_quarter) => {
                let segment = self.segment_at_tick(tick);
                let within = u128::from(tick - segment.start_tick)
                    * u128::from(segment.microseconds_per_quarter);
                (segment.elapsed_microtick + within) as f64
                    / f64::from(ticks_per_quarter)
                    / 1_000_000.0
            }
            Division::Smpte {
                frames_per_second,
                ticks_per_frame,
            } => {
                let (frames_numerator, frames_denominator) = frames_per_second.ratio();
                tick as f64 * f64::from(frames_denominator)
                    / (f64::from(frames_numerator) * f64::from(ticks_per_frame))
            }
        }
    }

    /// Inverse of `ticks_to_seconds`, returning a fractional tick. Negative
    /// seconds clamp to zero; NaN remains NaN.
    pub fn seconds_to_ticks(&self, seconds: f64) -> f64 {
        if seconds.is_nan() {
            return f64::NAN;
        }
        let seconds = seconds.max(0.0);
        match self.division {
            Division::TicksPerQuarter(ticks_per_quarter) => {
                let mut selected = &self.segments[0];
                let mut selected_seconds = 0.0;
                for segment in &self.segments {
                    let start_seconds = segment.elapsed_microtick as f64
                        / f64::from(ticks_per_quarter)
                        / 1_000_000.0;
                    if start_seconds > seconds {
                        break;
                    }
                    selected = segment;
                    selected_seconds = start_seconds;
                }
                selected.start_tick as f64
                    + (seconds - selected_seconds) * 1_000_000.0
                        * f64::from(ticks_per_quarter)
                        / f64::from(selected.microseconds_per_quarter)
            }
            Division::Smpte {
                frames_per_second,
                ticks_per_frame,
            } => {
                let (frames_numerator, frames_denominator) = frames_per_second.ratio();
                seconds * f64::from(frames_numerator) * f64::from(ticks_per_frame)
                    / f64::from(frames_denominator)
            }
        }
    }

    fn segment_at_tick(&self, tick: u64) -> &TempoSegment {
        let mut selected = &self.segments[0];
        for segment in &self.segments {
            if segment.start_tick > tick {
                break;
            }
            selected = segment;
        }
        selected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedTimeSignature {
    pub tick: u64,
    pub signature: TimeSignature,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeSignatureMap {
    pub changes: Vec<TimedTimeSignature>,
}

impl TimeSignatureMap {
    pub fn at_tick(&self, tick: u64) -> TimeSignature {
        let mut current = TimeSignature::default();
        for change in &self.changes {
            if change.tick > tick {
                break;
            }
            current = change.signature;
        }
        current
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedKeySignature {
    pub tick: u64,
    pub signature: KeySignature,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeySignatureMap {
    pub changes: Vec<TimedKeySignature>,
}

impl KeySignatureMap {
    pub fn at_tick(&self, tick: u64) -> KeySignature {
        let mut current = KeySignature::default();
        for change in &self.changes {
            if change.tick > tick {
                break;
            }
            current = change.signature;
        }
        current
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PairedNote {
    pub track: usize,
    pub channel: u8,
    pub key: u8,
    pub velocity_on: u8,
    pub velocity_off: u8,
    pub tick_on: u64,
    pub tick_off: u64,
    pub time_on: f64,
    pub time_off: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnmatchedNoteOn {
    pub track: usize,
    pub channel: u8,
    pub key: u8,
    pub velocity_on: u8,
    pub tick_on: u64,
    pub time_on: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnmatchedNoteOff {
    pub track: usize,
    pub channel: u8,
    pub key: u8,
    pub velocity_off: u8,
    pub tick_off: u64,
    pub time_off: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteSequence {
    /// Zero for formats 0/1; the independent track index for format 2.
    pub sequence: usize,
    pub notes: Vec<PairedNote>,
    pub unmatched_note_ons: Vec<UnmatchedNoteOn>,
    pub unmatched_note_offs: Vec<UnmatchedNoteOff>,
}

impl MidiFile {
    /// Builds the song tempo map. Format 1 deliberately reads tempo only from
    /// track 0, following the SMF convention. Format 2 returns an error because
    /// its tracks are independent sequences.
    pub fn tempo_map(&self) -> MidiResult<TempoMap> {
        if self.header.format == Format::Sequential {
            return Err(MidiError::new(
                0,
                MidiErrorKind::IndependentSequencesRequireIndex,
            ));
        }
        self.tempo_map_for_sequence(0)
    }

    pub fn tempo_map_for_sequence(&self, sequence: usize) -> MidiResult<TempoMap> {
        let track = self.conductor_track_for_sequence(sequence)?;
        Ok(build_tempo_map(self.header.division, track))
    }

    pub fn tempo_maps(&self) -> MidiResult<Vec<TempoMap>> {
        (0..self.sequence_count())
            .map(|sequence| self.tempo_map_for_sequence(sequence))
            .collect()
    }

    pub fn time_signature_map(&self) -> MidiResult<TimeSignatureMap> {
        if self.header.format == Format::Sequential {
            return Err(MidiError::new(
                0,
                MidiErrorKind::IndependentSequencesRequireIndex,
            ));
        }
        self.time_signature_map_for_sequence(0)
    }

    pub fn time_signature_map_for_sequence(
        &self,
        sequence: usize,
    ) -> MidiResult<TimeSignatureMap> {
        let track = self.conductor_track_for_sequence(sequence)?;
        let mut changes = track
            .events
            .iter()
            .filter_map(|event| match event.kind {
                EventKind::Meta(MetaEvent::TimeSignature(signature)) => {
                    Some(TimedTimeSignature {
                        tick: event.tick,
                        signature,
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        changes.sort_by_key(|change| change.tick);
        Ok(TimeSignatureMap { changes })
    }

    pub fn key_signature_map(&self) -> MidiResult<KeySignatureMap> {
        if self.header.format == Format::Sequential {
            return Err(MidiError::new(
                0,
                MidiErrorKind::IndependentSequencesRequireIndex,
            ));
        }
        self.key_signature_map_for_sequence(0)
    }

    pub fn key_signature_map_for_sequence(
        &self,
        sequence: usize,
    ) -> MidiResult<KeySignatureMap> {
        let track = self.conductor_track_for_sequence(sequence)?;
        let mut changes = track
            .events
            .iter()
            .filter_map(|event| match event.kind {
                EventKind::Meta(MetaEvent::KeySignature(signature)) => Some(TimedKeySignature {
                    tick: event.tick,
                    signature,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        changes.sort_by_key(|change| change.tick);
        Ok(KeySignatureMap { changes })
    }

    /// Pairs note events within each track and channel. For overlapping note-ons
    /// with the same key, the next note-off closes the oldest open note (FIFO).
    /// This preserves onset order and avoids artificial nesting. Velocity-zero
    /// note-ons act as offs here while remaining raw `NoteOn` events in tracks.
    /// Unmatched ons and offs are returned explicitly.
    pub fn paired_notes(&self) -> MidiResult<Vec<NoteSequence>> {
        match self.header.format {
            Format::SingleTrack | Format::Parallel => {
                let tempo = self.tempo_map_for_sequence(0)?;
                let track_indices = (0..self.tracks.len()).collect::<Vec<_>>();
                Ok(vec![pair_tracks(self, 0, &track_indices, &tempo)])
            }
            Format::Sequential => {
                let mut result = Vec::with_capacity(self.tracks.len());
                for track_index in 0..self.tracks.len() {
                    let tempo = self.tempo_map_for_sequence(track_index)?;
                    result.push(pair_tracks(
                        self,
                        track_index,
                        &[track_index],
                        &tempo,
                    ));
                }
                Ok(result)
            }
        }
    }

    fn conductor_track_for_sequence(&self, sequence: usize) -> MidiResult<&Track> {
        let count = self.sequence_count();
        match self.header.format {
            Format::Sequential => self.tracks.get(sequence).ok_or_else(|| {
                MidiError::new(
                    0,
                    MidiErrorKind::SequenceOutOfRange { sequence, count },
                )
            }),
            Format::SingleTrack | Format::Parallel => {
                if sequence != 0 || self.tracks.is_empty() {
                    Err(MidiError::new(
                        0,
                        MidiErrorKind::SequenceOutOfRange { sequence, count },
                    ))
                } else {
                    Ok(&self.tracks[0])
                }
            }
        }
    }
}

fn build_tempo_map(division: Division, track: &Track) -> TempoMap {
    let mut explicit = track
        .events
        .iter()
        .filter_map(|event| match event.kind {
            EventKind::Meta(MetaEvent::SetTempo(microseconds_per_quarter)) => {
                Some(TempoChange {
                    tick: event.tick,
                    microseconds_per_quarter,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    explicit.sort_by_key(|change| change.tick);

    let mut changes = vec![TempoChange {
        tick: 0,
        microseconds_per_quarter: DEFAULT_MICROSECONDS_PER_QUARTER,
    }];
    for change in explicit {
        if changes.last().is_some_and(|last| last.tick == change.tick) {
            *changes.last_mut().expect("map starts nonempty") = change;
        } else {
            changes.push(change);
        }
    }

    let mut segments = Vec::with_capacity(changes.len());
    let mut elapsed_microtick = 0_u128;
    let mut previous_tick = 0_u64;
    let mut previous_tempo = changes[0].microseconds_per_quarter;
    for change in &changes {
        elapsed_microtick += u128::from(change.tick - previous_tick) * u128::from(previous_tempo);
        segments.push(TempoSegment {
            start_tick: change.tick,
            microseconds_per_quarter: change.microseconds_per_quarter,
            elapsed_microtick,
        });
        previous_tick = change.tick;
        previous_tempo = change.microseconds_per_quarter;
    }

    TempoMap {
        division,
        changes,
        segments,
    }
}

#[derive(Clone, Copy)]
struct OpenNote {
    tick: u64,
    velocity: u8,
}

fn pair_tracks(
    file: &MidiFile,
    sequence: usize,
    track_indices: &[usize],
    tempo: &TempoMap,
) -> NoteSequence {
    let mut result = NoteSequence {
        sequence,
        ..NoteSequence::default()
    };

    for &track_index in track_indices {
        let mut open: HashMap<(u8, u8), VecDeque<OpenNote>> = HashMap::new();
        for event in &file.tracks[track_index].events {
            let EventKind::Channel(channel_event) = &event.kind else {
                continue;
            };
            let (key, velocity_off) = match channel_event.message {
                ChannelMessage::NoteOn { key, velocity } if velocity > 0 => {
                    open.entry((channel_event.channel, key))
                        .or_default()
                        .push_back(OpenNote {
                            tick: event.tick,
                            velocity,
                        });
                    continue;
                }
                ChannelMessage::NoteOn { key, velocity: 0 } => (key, 0),
                ChannelMessage::NoteOff { key, velocity } => (key, velocity),
                _ => continue,
            };

            let queue = open.entry((channel_event.channel, key)).or_default();
            if let Some(note_on) = queue.pop_front() {
                result.notes.push(PairedNote {
                    track: track_index,
                    channel: channel_event.channel,
                    key,
                    velocity_on: note_on.velocity,
                    velocity_off,
                    tick_on: note_on.tick,
                    tick_off: event.tick,
                    time_on: tempo.ticks_to_seconds(note_on.tick),
                    time_off: tempo.ticks_to_seconds(event.tick),
                });
            } else {
                result.unmatched_note_offs.push(UnmatchedNoteOff {
                    track: track_index,
                    channel: channel_event.channel,
                    key,
                    velocity_off,
                    tick_off: event.tick,
                    time_off: tempo.ticks_to_seconds(event.tick),
                });
            }
        }
        for ((channel, key), queue) in open {
            for note_on in queue {
                result.unmatched_note_ons.push(UnmatchedNoteOn {
                    track: track_index,
                    channel,
                    key,
                    velocity_on: note_on.velocity,
                    tick_on: note_on.tick,
                    time_on: tempo.ticks_to_seconds(note_on.tick),
                });
            }
        }
    }

    result.notes.sort_by_key(|note| {
        (
            note.tick_on,
            note.track,
            note.channel,
            note.key,
            note.tick_off,
        )
    });
    result
        .unmatched_note_ons
        .sort_by_key(|note| (note.tick_on, note.track, note.channel, note.key));
    result
        .unmatched_note_offs
        .sort_by_key(|note| (note.tick_off, note.track, note.channel, note.key));
    result
}
