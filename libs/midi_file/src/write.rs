use crate::error::WriteError;
use crate::model::*;

const MAX_VLQ: u64 = 0x0fff_ffff;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// Disabled by default. Explicit status bytes are simpler and universally valid.
    pub running_status: bool,
}

pub fn write(file: &MidiFile) -> Result<Vec<u8>, WriteError> {
    write_with_options(file, WriteOptions::default())
}

pub fn write_with_options(
    file: &MidiFile,
    options: WriteOptions,
) -> Result<Vec<u8>, WriteError> {
    match file.header.format {
        Format::SingleTrack if file.tracks.len() != 1 => {
            return Err(WriteError::InvalidFormatZeroTrackCount(file.tracks.len()))
        }
        Format::SingleTrack | Format::Parallel => {}
        Format::Sequential => {
            return Err(WriteError::UnsupportedFormat(
                file.header.format.as_u16(),
            ))
        }
    }
    if file.tracks.is_empty() {
        return Err(WriteError::InvalidTrackCount(0));
    }
    if file.tracks.len() > usize::from(u16::MAX) {
        return Err(WriteError::TooManyTracks(file.tracks.len()));
    }
    if usize::from(file.header.track_count) != file.tracks.len() {
        return Err(WriteError::HeaderTrackCountMismatch {
            header: file.header.track_count,
            actual: file.tracks.len(),
        });
    }

    let mut header = Vec::with_capacity(6 + file.header.extra_data.len());
    header.extend_from_slice(&file.header.format.as_u16().to_be_bytes());
    header.extend_from_slice(&file.header.track_count.to_be_bytes());
    header.extend_from_slice(&encode_division(file.header.division)?.to_be_bytes());
    header.extend_from_slice(&file.header.extra_data);

    let mut output = Vec::new();
    push_chunk(&mut output, *b"MThd", &header)?;
    for (track_index, track) in file.tracks.iter().enumerate() {
        let track_data = write_track(track, track_index, options)?;
        push_chunk(&mut output, *b"MTrk", &track_data)?;
    }
    for chunk in &file.unknown_chunks {
        push_chunk(&mut output, chunk.id, &chunk.data)?;
    }
    Ok(output)
}

fn encode_division(division: Division) -> Result<u16, WriteError> {
    Ok(match division {
        Division::TicksPerQuarter(value) => {
            if value == 0 || value & 0x8000 != 0 {
                return Err(WriteError::InvalidTicksPerQuarter(value));
            }
            value
        }
        Division::Smpte {
            frames_per_second,
            ticks_per_frame,
        } => {
            if ticks_per_frame == 0 {
                return Err(WriteError::InvalidTicksPerFrame(ticks_per_frame));
            }
            (u16::from(frames_per_second.smf_code() as u8) << 8) | u16::from(ticks_per_frame)
        }
    })
}

fn push_chunk(output: &mut Vec<u8>, id: [u8; 4], data: &[u8]) -> Result<(), WriteError> {
    let length = u32::try_from(data.len()).map_err(|_| WriteError::ChunkTooLarge(data.len()))?;
    output.extend_from_slice(&id);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(data);
    Ok(())
}

fn write_track(
    track: &Track,
    track_index: usize,
    options: WriteOptions,
) -> Result<Vec<u8>, WriteError> {
    let mut output = Vec::new();
    let mut previous_tick = 0_u64;
    let mut running_status = None;
    for event in &track.events {
        if event.tick < previous_tick {
            return Err(WriteError::EventsNotSorted {
                track: track_index,
                previous: previous_tick,
                next: event.tick,
            });
        }
        let delta = event.tick - previous_tick;
        if delta > MAX_VLQ {
            return Err(WriteError::DeltaTooLarge {
                track: track_index,
                delta,
            });
        }
        push_vlq(&mut output, delta)?;
        write_event(&mut output, &event.kind, options, &mut running_status)?;
        previous_tick = event.tick;
    }
    output.extend_from_slice(&track.trailing_data);
    Ok(output)
}

fn write_event(
    output: &mut Vec<u8>,
    kind: &EventKind,
    options: WriteOptions,
    running_status: &mut Option<u8>,
) -> Result<(), WriteError> {
    match kind {
        EventKind::Channel(event) => {
            if event.channel > 15 {
                return Err(WriteError::InvalidChannel(event.channel));
            }
            let status_nibble = match event.message {
                ChannelMessage::NoteOff { .. } => 0x80,
                ChannelMessage::NoteOn { .. } => 0x90,
                ChannelMessage::PolyphonicKeyPressure { .. } => 0xa0,
                ChannelMessage::ControlChange { .. } => 0xb0,
                ChannelMessage::ProgramChange { .. } => 0xc0,
                ChannelMessage::ChannelPressure { .. } => 0xd0,
                ChannelMessage::PitchBend { .. } => 0xe0,
            };
            let status = status_nibble | event.channel;
            if !options.running_status || *running_status != Some(status) {
                output.push(status);
            }
            *running_status = Some(status);
            match event.message {
                ChannelMessage::NoteOff { key, velocity }
                | ChannelMessage::NoteOn { key, velocity } => {
                    push_data(output, key)?;
                    push_data(output, velocity)?;
                }
                ChannelMessage::PolyphonicKeyPressure { key, pressure } => {
                    push_data(output, key)?;
                    push_data(output, pressure)?;
                }
                ChannelMessage::ControlChange { controller, value } => {
                    push_data(output, controller)?;
                    push_data(output, value)?;
                }
                ChannelMessage::ProgramChange { program } => push_data(output, program)?,
                ChannelMessage::ChannelPressure { pressure } => push_data(output, pressure)?,
                ChannelMessage::PitchBend { value } => {
                    if value > 0x3fff {
                        return Err(WriteError::InvalidPitchBend(value));
                    }
                    output.push((value & 0x7f) as u8);
                    output.push((value >> 7) as u8);
                }
            }
        }
        EventKind::SysEx(event) => {
            output.push(match event.kind {
                SysExKind::F0 => 0xf0,
                SysExKind::F7 => 0xf7,
            });
            push_vlq(output, event.data.len() as u64)?;
            output.extend_from_slice(&event.data);
        }
        EventKind::Meta(event) => write_meta(output, event)?,
    }
    Ok(())
}

fn write_meta(output: &mut Vec<u8>, event: &MetaEvent) -> Result<(), WriteError> {
    output.push(0xff);
    let (kind, data) = match event {
        MetaEvent::SequenceNumber(number) => (0x00, number.to_be_bytes().to_vec()),
        MetaEvent::Text(data) => (0x01, data.clone()),
        MetaEvent::Copyright(data) => (0x02, data.clone()),
        MetaEvent::SequenceOrTrackName(data) => (0x03, data.clone()),
        MetaEvent::InstrumentName(data) => (0x04, data.clone()),
        MetaEvent::Lyric(data) => (0x05, data.clone()),
        MetaEvent::Marker(data) => (0x06, data.clone()),
        MetaEvent::CuePoint(data) => (0x07, data.clone()),
        MetaEvent::MidiChannelPrefix(channel) => (0x20, vec![*channel]),
        MetaEvent::MidiPort(port) => (0x21, vec![*port]),
        MetaEvent::EndOfTrack => (0x2f, Vec::new()),
        MetaEvent::SetTempo(tempo) => {
            if *tempo == 0 || *tempo > 0x00ff_ffff {
                return Err(WriteError::InvalidTempo(*tempo));
            }
            (0x51, tempo.to_be_bytes()[1..].to_vec())
        }
        MetaEvent::SmpteOffset(value) => (0x54, value.to_vec()),
        MetaEvent::TimeSignature(signature) => (
            0x58,
            vec![
                signature.numerator,
                signature.denominator_power,
                signature.midi_clocks_per_metronome_click,
                signature.thirty_second_notes_per_quarter,
            ],
        ),
        MetaEvent::KeySignature(signature) => {
            if !(-7..=7).contains(&signature.sharps_flats) {
                return Err(WriteError::InvalidKeySignature(signature.sharps_flats));
            }
            (
                0x59,
                vec![
                    signature.sharps_flats as u8,
                    u8::from(signature.is_minor),
                ],
            )
        }
        MetaEvent::SequencerSpecific(data) => (0x7f, data.clone()),
        MetaEvent::Unknown { kind, data } => (*kind, data.clone()),
    };
    output.push(kind);
    push_vlq(output, data.len() as u64)?;
    output.extend_from_slice(&data);
    Ok(())
}

fn push_data(output: &mut Vec<u8>, value: u8) -> Result<(), WriteError> {
    if value & 0x80 != 0 {
        return Err(WriteError::InvalidDataByte(value));
    }
    output.push(value);
    Ok(())
}

fn push_vlq(output: &mut Vec<u8>, mut value: u64) -> Result<(), WriteError> {
    if value > MAX_VLQ {
        return Err(WriteError::VariableLengthValueTooLarge(value));
    }
    let mut bytes = [0_u8; 4];
    let mut index = 3;
    bytes[index] = (value & 0x7f) as u8;
    while {
        value >>= 7;
        value != 0
    } {
        index -= 1;
        bytes[index] = ((value & 0x7f) as u8) | 0x80;
    }
    output.extend_from_slice(&bytes[index..]);
    Ok(())
}
