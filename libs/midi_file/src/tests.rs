use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::*;

const FORMAT_ZERO: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0, b'M', b'T', b'r', b'k',
    0, 0, 0, 0x10, 0x00, 0xc0, 0x05, 0x00, 0x90, 0x3c, 0x64, 0x83, 0x60, 0x80, 0x3c,
    0x40, 0x00, 0xff, 0x2f, 0x00,
];

const RUNNING_STATUS: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0, b'M', b'T', b'r', b'k',
    0, 0, 0, 0x12, 0x00, 0x90, 0x3c, 0x64, 0x0a, 0x3e, 0x50, 0x0a, 0x3c, 0x00,
    0x00, 0x80, 0x3e, 0x40, 0x00, 0xff, 0x2f, 0x00,
];

const FORMAT_ONE_TEMPOS: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 1, 0, 2, 0x01, 0xe0,
    // Track 0: three tempos, 3/4, F minor, EOT (41 bytes).
    b'M', b'T', b'r', b'k', 0, 0, 0, 0x29, 0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20,
    0x83, 0x60, 0xff, 0x51, 0x03, 0x0f, 0x42, 0x40, 0x83, 0x60, 0xff, 0x51, 0x03,
    0x03, 0xd0, 0x90, 0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, 0x00, 0xff,
    0x59, 0x02, 0xff, 0x01, 0x00, 0xff, 0x2f, 0x00,
    // Track 1: a one-second note under the conductor map.
    b'M', b'T', b'r', b'k', 0, 0, 0, 0x0d, 0x00, 0x90, 0x3c, 0x64, 0x87, 0x40, 0x80,
    0x3c, 0x20, 0x00, 0xff, 0x2f, 0x00,
];

const SMPTE: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0xe7, 0x28, b'M', b'T', b'r',
    b'k', 0, 0, 0, 4, 0, 0xff, 0x2f, 0,
];

const SYSEX_AND_UNKNOWN_META: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0, b'M', b'T', b'r', b'k',
    0, 0, 0, 0x15, 0x00, 0xf0, 0x03, 0x01, 0x02, 0xf7, 0x00, 0xf7, 0x02, 0x03,
    0xf7, 0x00, 0xff, 0x7a, 0x02, 0xaa, 0xbb, 0x00, 0xff, 0x2f, 0x00,
];

const FORMAT_TWO: &[u8] = &[
    b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 2, 0, 2, 0x01, 0xe0,
    // Independent sequence 0 at 500,000 us/qn.
    b'M', b'T', b'r', b'k', 0, 0, 0, 0x14, 0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20,
    0x00, 0x90, 0x3c, 0x64, 0x83, 0x60, 0x80, 0x3c, 0x20, 0x00, 0xff, 0x2f, 0x00,
    // Independent sequence 1 at 1,000,000 us/qn.
    b'M', b'T', b'r', b'k', 0, 0, 0, 0x14, 0x00, 0xff, 0x51, 0x03, 0x0f, 0x42, 0x40,
    0x00, 0x90, 0x40, 0x50, 0x83, 0x60, 0x80, 0x40, 0x10, 0x00, 0xff, 0x2f, 0x00,
];

#[test]
fn parses_format_zero_and_channel_events() {
    let midi = parse(FORMAT_ZERO).unwrap();
    assert_eq!(midi.header.format, Format::SingleTrack);
    assert_eq!(midi.header.track_count, 1);
    assert_eq!(midi.header.division, Division::TicksPerQuarter(480));
    assert_eq!(midi.tracks[0].events.len(), 4);
    assert_eq!(midi.tracks[0].events[2].tick, 480);
    assert!(matches!(
        midi.tracks[0].events[0].kind,
        EventKind::Channel(ChannelEvent {
            channel: 0,
            message: ChannelMessage::ProgramChange { program: 5 }
        })
    ));
    assert!(midi.tracks[0].has_end_of_track());
}

#[test]
fn parses_every_channel_voice_message() {
    let bytes = one_track(&[
        0, 0x80, 60, 1, 0, 0x90, 61, 2, 0, 0xa0, 62, 3, 0, 0xb0, 7, 100, 0, 0xc0, 9,
        0, 0xd0, 10, 0, 0xe0, 0, 64, 0, 0xff, 0x2f, 0,
    ]);
    let midi = parse(&bytes).unwrap();
    let messages = midi.tracks[0]
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Channel(event) => Some(&event.message),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(messages[0], ChannelMessage::NoteOff { .. }));
    assert!(matches!(messages[1], ChannelMessage::NoteOn { .. }));
    assert!(matches!(
        messages[2],
        ChannelMessage::PolyphonicKeyPressure { .. }
    ));
    assert!(matches!(messages[3], ChannelMessage::ControlChange { .. }));
    assert!(matches!(messages[4], ChannelMessage::ProgramChange { .. }));
    assert!(matches!(messages[5], ChannelMessage::ChannelPressure { .. }));
    assert_eq!(messages[6], &ChannelMessage::PitchBend { value: 8192 });
}

#[test]
fn running_status_and_velocity_zero_are_raw_but_pair_as_note_off() {
    let midi = parse(RUNNING_STATUS).unwrap();
    assert!(matches!(
        midi.tracks[0].events[2].kind,
        EventKind::Channel(ChannelEvent {
            message: ChannelMessage::NoteOn {
                key: 60,
                velocity: 0
            },
            ..
        })
    ));
    let sequences = midi.paired_notes().unwrap();
    assert_eq!(sequences.len(), 1);
    assert_eq!(sequences[0].notes.len(), 2);
    assert_eq!(sequences[0].notes[0].key, 60);
    assert_eq!(sequences[0].notes[0].tick_off, 20);
    assert_eq!(sequences[0].notes[0].velocity_off, 0);
    assert!(sequences[0].unmatched_note_ons.is_empty());
}

#[test]
fn overlapping_same_pitch_uses_fifo_and_reports_unmatched_ons() {
    let bytes = one_track(&[
        0, 0x90, 60, 100, 10, 0x90, 60, 80, 10, 0x80, 60, 64, 10, 0x80, 60, 32, 0,
        0x90, 62, 70, 0, 0xff, 0x2f, 0,
    ]);
    let midi = parse(&bytes).unwrap();
    let notes = midi.paired_notes().unwrap();
    assert_eq!(notes[0].notes.len(), 2);
    assert_eq!(notes[0].notes[0].velocity_on, 100);
    assert_eq!(notes[0].notes[0].tick_off, 20);
    assert_eq!(notes[0].notes[1].velocity_on, 80);
    assert_eq!(notes[0].notes[1].tick_off, 30);
    assert_eq!(notes[0].unmatched_note_ons.len(), 1);
    assert_eq!(notes[0].unmatched_note_ons[0].key, 62);
}

#[test]
fn format_one_uses_track_zero_and_accumulates_multiple_tempos_exactly() {
    let midi = parse(FORMAT_ONE_TEMPOS).unwrap();
    assert_eq!(midi.header.format, Format::Parallel);
    let tempo = midi.tempo_map().unwrap();
    // Hand calculation at tick 720: 480/480*0.5 + 240/480*1.0 = 1.0 s.
    approx(tempo.ticks_to_seconds(720), 1.0);
    // At tick 1200: 0.5 + 1.0 + 240/480*0.25 = 1.625 s.
    approx(tempo.ticks_to_seconds(1200), 1.625);
    approx(tempo.seconds_to_ticks(1.0), 720.0);
    approx(tempo.seconds_to_ticks(1.625), 1200.0);

    let time = midi.time_signature_map().unwrap();
    assert_eq!(time.at_tick(959), TimeSignature::default());
    assert_eq!(time.at_tick(960).numerator, 3);
    assert_eq!(time.at_tick(960).denominator(), Some(4));
    let key = midi.key_signature_map().unwrap();
    assert_eq!(
        key.at_tick(960),
        KeySignature {
            sharps_flats: -1,
            is_minor: true
        }
    );

    let paired = midi.paired_notes().unwrap();
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].notes[0].track, 1);
    approx(paired[0].notes[0].time_off, 1.5);
}

#[test]
fn smpte_division_uses_frame_timing_and_ignores_tempo_for_conversion() {
    let midi = parse(SMPTE).unwrap();
    assert_eq!(
        midi.header.division,
        Division::Smpte {
            frames_per_second: SmpteFramesPerSecond::Fps25,
            ticks_per_frame: 40
        }
    );
    let tempo = midi.tempo_map().unwrap();
    approx(tempo.ticks_to_seconds(1000), 1.0);
    approx(tempo.seconds_to_ticks(1.25), 1250.0);
}

#[test]
fn preserves_f0_f7_sysex_and_unknown_meta() {
    let midi = parse(SYSEX_AND_UNKNOWN_META).unwrap();
    assert_eq!(
        midi.tracks[0].events[0].kind,
        EventKind::SysEx(SysExEvent {
            kind: SysExKind::F0,
            data: vec![1, 2, 0xf7]
        })
    );
    assert_eq!(
        midi.tracks[0].events[1].kind,
        EventKind::SysEx(SysExEvent {
            kind: SysExKind::F7,
            data: vec![3, 0xf7]
        })
    );
    assert_eq!(
        midi.tracks[0].events[2].kind,
        EventKind::Meta(MetaEvent::Unknown {
            kind: 0x7a,
            data: vec![0xaa, 0xbb]
        })
    );
    assert_eq!(write(&midi).unwrap(), SYSEX_AND_UNKNOWN_META);
}

#[test]
fn format_two_sequences_are_never_merged() {
    let midi = parse(FORMAT_TWO).unwrap();
    assert_eq!(midi.sequence_count(), 2);
    assert!(matches!(
        midi.tempo_map().unwrap_err().kind,
        MidiErrorKind::IndependentSequencesRequireIndex
    ));
    approx(midi.tempo_map_for_sequence(0).unwrap().ticks_to_seconds(480), 0.5);
    approx(midi.tempo_map_for_sequence(1).unwrap().ticks_to_seconds(480), 1.0);
    let paired = midi.paired_notes().unwrap();
    assert_eq!(paired.len(), 2);
    assert_eq!(paired[0].notes[0].key, 60);
    assert_eq!(paired[1].notes[0].key, 64);
    approx(paired[0].notes[0].time_off, 0.5);
    approx(paired[1].notes[0].time_off, 1.0);
    assert!(matches!(
        write(&midi),
        Err(WriteError::UnsupportedFormat(2))
    ));
}

#[test]
fn all_required_meta_variants_round_trip_semantically() {
    let events = vec![
        MetaEvent::SequenceNumber(7),
        MetaEvent::Text(b"text".to_vec()),
        MetaEvent::Copyright(b"copyright".to_vec()),
        MetaEvent::SequenceOrTrackName(b"track".to_vec()),
        MetaEvent::InstrumentName(b"piano".to_vec()),
        MetaEvent::Lyric(b"lyric".to_vec()),
        MetaEvent::Marker(b"marker".to_vec()),
        MetaEvent::CuePoint(b"cue".to_vec()),
        MetaEvent::MidiChannelPrefix(3),
        MetaEvent::MidiPort(2),
        MetaEvent::SetTempo(600_000),
        MetaEvent::TimeSignature(TimeSignature::default()),
        MetaEvent::KeySignature(KeySignature::default()),
        MetaEvent::SequencerSpecific(vec![1, 2, 3]),
        MetaEvent::EndOfTrack,
    ];
    let file = MidiFile {
        header: Header {
            format: Format::SingleTrack,
            track_count: 1,
            division: Division::TicksPerQuarter(96),
            extra_data: Vec::new(),
        },
        tracks: vec![Track {
            events: events
                .into_iter()
                .enumerate()
                .map(|(tick, event)| TrackEvent {
                    tick: tick as u64,
                    kind: EventKind::Meta(event),
                })
                .collect(),
            trailing_data: Vec::new(),
        }],
        unknown_chunks: Vec::new(),
    };
    let reparsed = parse(&write(&file).unwrap()).unwrap();
    assert_eq!(reparsed, file);
}

#[test]
fn canonical_bytes_round_trip_exactly_and_running_status_is_semantic() {
    let parsed = parse(FORMAT_ZERO).unwrap();
    assert_eq!(write(&parsed).unwrap(), FORMAT_ZERO);

    let running = parse(RUNNING_STATUS).unwrap();
    let explicit = write(&running).unwrap();
    assert_ne!(explicit, RUNNING_STATUS);
    assert_eq!(parse(&explicit).unwrap(), running);

    let compressed = write_with_options(
        &running,
        WriteOptions {
            running_status: true,
        },
    )
    .unwrap();
    assert_eq!(parse(&compressed).unwrap(), running);
}

#[test]
fn unknown_chunks_are_skipped_and_retained() {
    let mut bytes = Vec::from(&FORMAT_ZERO[..14]);
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(&3_u32.to_be_bytes());
    bytes.extend_from_slice(&[1, 2, 3]);
    bytes.extend_from_slice(&FORMAT_ZERO[14..]);
    let midi = parse(&bytes).unwrap();
    assert_eq!(midi.tracks.len(), 1);
    assert_eq!(
        midi.unknown_chunks,
        vec![UnknownChunk {
            id: *b"JUNK",
            data: vec![1, 2, 3]
        }]
    );
}

#[test]
fn missing_end_of_track_is_observable_and_optionally_strict() {
    let bytes = one_track(&[0, 0x90, 60, 64]);
    let midi = parse(&bytes).unwrap();
    assert!(!midi.tracks[0].has_end_of_track());
    let error = parse_with_options(
        &bytes,
        ParseOptions {
            require_end_of_track: true,
            ..ParseOptions::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error.kind,
        MidiErrorKind::MissingEndOfTrack { track: 0 }
    ));
}

#[test]
fn malformed_lengths_vlqs_and_running_status_are_typed_errors() {
    let mut wrong_length = FORMAT_ZERO.to_vec();
    wrong_length[21] = 0x11;
    assert!(matches!(
        parse(&wrong_length).unwrap_err().kind,
        MidiErrorKind::ChunkLengthExceedsInput { .. }
    ));

    let invalid_vlq = one_track(&[0x81, 0x80, 0x80, 0x80, 0]);
    assert!(matches!(
        parse(&invalid_vlq).unwrap_err().kind,
        MidiErrorKind::InvalidVariableLengthQuantity
    ));

    let no_running_status = one_track(&[0, 60, 64]);
    assert!(matches!(
        parse(&no_running_status).unwrap_err().kind,
        MidiErrorKind::RunningStatusWithoutStatus(60)
    ));
}

#[test]
fn truncation_and_single_byte_corruption_never_panic() {
    for length in 0..SYSEX_AND_UNKNOWN_META.len() {
        let outcome = catch_unwind(AssertUnwindSafe(|| parse(&SYSEX_AND_UNKNOWN_META[..length])));
        assert!(outcome.is_ok(), "parser panicked at prefix length {length}");
        assert!(outcome.unwrap().is_err(), "truncated prefix unexpectedly parsed");
    }
    for index in 0..FORMAT_ONE_TEMPOS.len() {
        let mut corrupted = FORMAT_ONE_TEMPOS.to_vec();
        corrupted[index] ^= 0xff;
        let outcome = catch_unwind(AssertUnwindSafe(|| parse(&corrupted)));
        assert!(outcome.is_ok(), "parser panicked after corrupting byte {index}");
    }
}

fn one_track(track: &[u8]) -> Vec<u8> {
    let mut bytes = vec![
        b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0, b'M', b'T', b'r',
        b'k',
    ];
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(track);
    bytes
}

fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}
