use crate::error::{MidiError, MidiErrorKind, MidiResult};
use crate::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseOptions {
    /// Missing end-of-track is accepted by default and remains observable through
    /// `Track::has_end_of_track`. Enable this for strict validation.
    pub require_end_of_track: bool,
    pub require_declared_track_count: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            require_end_of_track: false,
            require_declared_track_count: true,
        }
    }
}

pub fn parse(bytes: &[u8]) -> MidiResult<MidiFile> {
    parse_with_options(bytes, ParseOptions::default())
}

pub fn parse_with_options(bytes: &[u8], options: ParseOptions) -> MidiResult<MidiFile> {
    let mut reader = Reader::new(bytes, 0);
    let id = reader.array4("header chunk id")?;
    if &id != b"MThd" {
        return Err(MidiError::new(0, MidiErrorKind::ExpectedHeaderChunk(id)));
    }
    let header_length = reader.u32("header chunk length")?;
    if header_length < 6 {
        return Err(MidiError::new(
            4,
            MidiErrorKind::InvalidHeaderLength(header_length),
        ));
    }
    let header_start = reader.absolute_offset();
    let header_data = reader.take_chunk(id, header_length)?;
    let mut header_reader = Reader::new(header_data, header_start);
    let format_raw = header_reader.u16("SMF format")?;
    let format = match format_raw {
        0 => Format::SingleTrack,
        1 => Format::Parallel,
        2 => Format::Sequential,
        value => {
            return Err(MidiError::new(
                header_start,
                MidiErrorKind::UnsupportedFormat(value),
            ))
        }
    };
    let track_count = header_reader.u16("track count")?;
    if track_count == 0 {
        return Err(MidiError::new(
            header_start + 2,
            MidiErrorKind::InvalidTrackCount(track_count),
        ));
    }
    if format == Format::SingleTrack && track_count != 1 {
        return Err(MidiError::new(
            header_start + 2,
            MidiErrorKind::InvalidFormatZeroTrackCount(track_count),
        ));
    }
    let division_raw = header_reader.u16("division")?;
    let division = parse_division(division_raw, header_start + 4)?;
    let extra_data = header_reader.remaining().to_vec();

    let mut tracks = Vec::new();
    let mut unknown_chunks = Vec::new();
    while !reader.is_empty() {
        if reader.remaining().len() < 8 {
            return Err(MidiError::new(
                reader.absolute_offset(),
                MidiErrorKind::TrailingChunkHeader(reader.remaining().len()),
            ));
        }
        let chunk_offset = reader.absolute_offset();
        let chunk_id = reader.array4("chunk id")?;
        let chunk_length = reader.u32("chunk length")?;
        let payload_offset = reader.absolute_offset();
        let payload = reader.take_chunk(chunk_id, chunk_length)?;
        if &chunk_id == b"MTrk" {
            let track_index = tracks.len();
            let track = parse_track(payload, payload_offset)?;
            if options.require_end_of_track && !track.has_end_of_track() {
                return Err(MidiError::new(
                    chunk_offset,
                    MidiErrorKind::MissingEndOfTrack { track: track_index },
                ));
            }
            tracks.push(track);
        } else {
            unknown_chunks.push(UnknownChunk {
                id: chunk_id,
                data: payload.to_vec(),
            });
        }
    }

    if options.require_declared_track_count && tracks.len() != usize::from(track_count) {
        return Err(MidiError::new(
            bytes.len(),
            MidiErrorKind::TrackCountMismatch {
                declared: track_count,
                found: tracks.len(),
            },
        ));
    }

    Ok(MidiFile {
        header: Header {
            format,
            track_count,
            division,
            extra_data,
        },
        tracks,
        unknown_chunks,
    })
}

fn parse_division(raw: u16, offset: usize) -> MidiResult<Division> {
    if raw & 0x8000 == 0 {
        if raw == 0 {
            return Err(MidiError::new(
                offset,
                MidiErrorKind::InvalidTicksPerQuarter(raw),
            ));
        }
        return Ok(Division::TicksPerQuarter(raw));
    }
    let code = (raw >> 8) as u8 as i8;
    let frames_per_second = match code {
        -24 => SmpteFramesPerSecond::Fps24,
        -25 => SmpteFramesPerSecond::Fps25,
        -29 => SmpteFramesPerSecond::Fps29Drop,
        -30 => SmpteFramesPerSecond::Fps30,
        value => {
            return Err(MidiError::new(
                offset,
                MidiErrorKind::InvalidSmpteFramesPerSecond(value),
            ))
        }
    };
    let ticks_per_frame = raw as u8;
    if ticks_per_frame == 0 {
        return Err(MidiError::new(
            offset + 1,
            MidiErrorKind::InvalidTicksPerFrame(ticks_per_frame),
        ));
    }
    Ok(Division::Smpte {
        frames_per_second,
        ticks_per_frame,
    })
}

fn parse_track(data: &[u8], base: usize) -> MidiResult<Track> {
    let mut reader = Reader::new(data, base);
    let mut events = Vec::new();
    let mut tick = 0_u64;
    let mut running_status = None;
    let mut trailing_data = Vec::new();

    while !reader.is_empty() {
        let delta = u64::from(reader.vlq("event delta")?);
        tick = tick
            .checked_add(delta)
            .ok_or_else(|| MidiError::new(reader.absolute_offset(), MidiErrorKind::TickOverflow))?;
        let event_offset = reader.absolute_offset();
        let next = reader.peek("event status")?;
        let status = if next & 0x80 != 0 {
            reader.byte("event status")?
        } else {
            running_status.ok_or_else(|| {
                MidiError::new(
                    event_offset,
                    MidiErrorKind::RunningStatusWithoutStatus(next),
                )
            })?
        };

        let kind = match status {
            0x80..=0xef => {
                running_status = Some(status);
                EventKind::Channel(parse_channel_event(status, &mut reader)?)
            }
            0xf0 | 0xf7 => {
                let length = reader.vlq("SysEx length")? as usize;
                let data = reader.take(length, "SysEx payload")?.to_vec();
                EventKind::SysEx(SysExEvent {
                    kind: if status == 0xf0 {
                        SysExKind::F0
                    } else {
                        SysExKind::F7
                    },
                    data,
                })
            }
            0xff => {
                let meta_kind = reader.byte("meta event kind")?;
                let length = reader.vlq("meta event length")? as usize;
                let payload_offset = reader.absolute_offset();
                let data = reader.take(length, "meta event payload")?;
                EventKind::Meta(parse_meta_event(meta_kind, data, payload_offset)?)
            }
            value => {
                return Err(MidiError::new(
                    event_offset,
                    MidiErrorKind::InvalidStatus(value),
                ))
            }
        };
        let is_end = matches!(kind, EventKind::Meta(MetaEvent::EndOfTrack));
        events.push(TrackEvent { tick, kind });
        if is_end {
            trailing_data.extend_from_slice(reader.remaining());
            break;
        }
    }

    Ok(Track {
        events,
        trailing_data,
    })
}

fn parse_channel_event(status: u8, reader: &mut Reader<'_>) -> MidiResult<ChannelEvent> {
    let channel = status & 0x0f;
    let message = match status >> 4 {
        0x8 => ChannelMessage::NoteOff {
            key: reader.data_byte("note key")?,
            velocity: reader.data_byte("note-off velocity")?,
        },
        0x9 => ChannelMessage::NoteOn {
            key: reader.data_byte("note key")?,
            velocity: reader.data_byte("note-on velocity")?,
        },
        0xa => ChannelMessage::PolyphonicKeyPressure {
            key: reader.data_byte("note key")?,
            pressure: reader.data_byte("polyphonic key pressure")?,
        },
        0xb => ChannelMessage::ControlChange {
            controller: reader.data_byte("controller number")?,
            value: reader.data_byte("controller value")?,
        },
        0xc => ChannelMessage::ProgramChange {
            program: reader.data_byte("program number")?,
        },
        0xd => ChannelMessage::ChannelPressure {
            pressure: reader.data_byte("channel pressure")?,
        },
        0xe => {
            let low = u16::from(reader.data_byte("pitch bend low byte")?);
            let high = u16::from(reader.data_byte("pitch bend high byte")?);
            ChannelMessage::PitchBend {
                value: low | (high << 7),
            }
        }
        _ => unreachable!("caller only passes channel statuses"),
    };
    Ok(ChannelEvent { channel, message })
}

fn parse_meta_event(kind: u8, data: &[u8], offset: usize) -> MidiResult<MetaEvent> {
    let exact = |expected| {
        if data.len() == expected {
            Ok(())
        } else {
            Err(MidiError::new(
                offset,
                MidiErrorKind::InvalidMetaLength {
                    kind,
                    expected,
                    actual: data.len(),
                },
            ))
        }
    };
    Ok(match kind {
        0x00 => {
            exact(2)?;
            MetaEvent::SequenceNumber(u16::from_be_bytes([data[0], data[1]]))
        }
        0x01 => MetaEvent::Text(data.to_vec()),
        0x02 => MetaEvent::Copyright(data.to_vec()),
        0x03 => MetaEvent::SequenceOrTrackName(data.to_vec()),
        0x04 => MetaEvent::InstrumentName(data.to_vec()),
        0x05 => MetaEvent::Lyric(data.to_vec()),
        0x06 => MetaEvent::Marker(data.to_vec()),
        0x07 => MetaEvent::CuePoint(data.to_vec()),
        0x20 => {
            exact(1)?;
            MetaEvent::MidiChannelPrefix(data[0])
        }
        0x21 => {
            exact(1)?;
            MetaEvent::MidiPort(data[0])
        }
        0x2f => {
            exact(0)?;
            MetaEvent::EndOfTrack
        }
        0x51 => {
            exact(3)?;
            let tempo = u32::from_be_bytes([0, data[0], data[1], data[2]]);
            if tempo == 0 {
                return Err(MidiError::new(offset, MidiErrorKind::InvalidTempo(tempo)));
            }
            MetaEvent::SetTempo(tempo)
        }
        0x54 => {
            exact(5)?;
            MetaEvent::SmpteOffset(data.try_into().expect("length checked"))
        }
        0x58 => {
            exact(4)?;
            MetaEvent::TimeSignature(TimeSignature {
                numerator: data[0],
                denominator_power: data[1],
                midi_clocks_per_metronome_click: data[2],
                thirty_second_notes_per_quarter: data[3],
            })
        }
        0x59 => {
            exact(2)?;
            let sharps_flats = data[0] as i8;
            if !(-7..=7).contains(&sharps_flats) || data[1] > 1 {
                return Err(MidiError::new(
                    offset,
                    MidiErrorKind::InvalidKeySignature {
                        sharps_flats,
                        scale: data[1],
                    },
                ));
            }
            MetaEvent::KeySignature(KeySignature {
                sharps_flats,
                is_minor: data[1] == 1,
            })
        }
        0x7f => MetaEvent::SequencerSpecific(data.to_vec()),
        _ => MetaEvent::Unknown {
            kind,
            data: data.to_vec(),
        },
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], base: usize) -> Self {
        Self { data, pos: 0, base }
    }

    fn absolute_offset(&self) -> usize {
        self.base + self.pos
    }

    fn is_empty(&self) -> bool {
        self.pos == self.data.len()
    }

    fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    fn take(&mut self, length: usize, context: &'static str) -> MidiResult<&'a [u8]> {
        let remaining = self.data.len().saturating_sub(self.pos);
        if length > remaining {
            return Err(MidiError::new(
                self.absolute_offset(),
                MidiErrorKind::UnexpectedEof {
                    context,
                    needed: length,
                    remaining,
                },
            ));
        }
        let start = self.pos;
        self.pos += length;
        Ok(&self.data[start..self.pos])
    }

    fn take_chunk(&mut self, id: [u8; 4], length: u32) -> MidiResult<&'a [u8]> {
        let remaining = self.data.len().saturating_sub(self.pos);
        let length_usize = length as usize;
        if length_usize > remaining {
            return Err(MidiError::new(
                self.absolute_offset(),
                MidiErrorKind::ChunkLengthExceedsInput {
                    chunk: id,
                    length,
                    remaining,
                },
            ));
        }
        self.take(length_usize, "chunk payload")
    }

    fn peek(&self, context: &'static str) -> MidiResult<u8> {
        self.data.get(self.pos).copied().ok_or_else(|| {
            MidiError::new(
                self.absolute_offset(),
                MidiErrorKind::UnexpectedEof {
                    context,
                    needed: 1,
                    remaining: 0,
                },
            )
        })
    }

    fn byte(&mut self, context: &'static str) -> MidiResult<u8> {
        Ok(self.take(1, context)?[0])
    }

    fn data_byte(&mut self, context: &'static str) -> MidiResult<u8> {
        let offset = self.absolute_offset();
        let value = self.byte(context)?;
        if value & 0x80 != 0 {
            return Err(MidiError::new(
                offset,
                MidiErrorKind::InvalidDataByte(value),
            ));
        }
        Ok(value)
    }

    fn array4(&mut self, context: &'static str) -> MidiResult<[u8; 4]> {
        Ok(self
            .take(4, context)?
            .try_into()
            .expect("four-byte slice"))
    }

    fn u16(&mut self, context: &'static str) -> MidiResult<u16> {
        Ok(u16::from_be_bytes(
            self.take(2, context)?.try_into().expect("two-byte slice"),
        ))
    }

    fn u32(&mut self, context: &'static str) -> MidiResult<u32> {
        Ok(u32::from_be_bytes(
            self.take(4, context)?.try_into().expect("four-byte slice"),
        ))
    }

    fn vlq(&mut self, context: &'static str) -> MidiResult<u32> {
        let start = self.absolute_offset();
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self.byte(context)?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(MidiError::new(
            start,
            MidiErrorKind::InvalidVariableLengthQuantity,
        ))
    }
}
