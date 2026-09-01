use crate::{
    makepad_live_id::{FromLiveId, LiveId},
    os::{OsMidiInput, OsMidiOutput},
};

#[derive(Clone, Debug)]
pub struct MidiPortsEvent {
    pub descs: Vec<MidiPortDesc>,
}

impl MidiPortsEvent {
    pub fn all_inputs(&self) -> Vec<MidiPortId> {
        let mut out = Vec::new();
        for d in &self.descs {
            if d.port_type.is_input() {
                out.push(d.port_id);
            }
        }
        out
    }
    pub fn all_outputs(&self) -> Vec<MidiPortId> {
        let mut out = Vec::new();
        for d in &self.descs {
            if d.port_type.is_output() {
                out.push(d.port_id);
            }
        }
        out
    }
}

impl std::fmt::Display for MidiPortsEvent {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        write!(f, "MIDI ports:\n").unwrap();
        for desc in &self.descs {
            if desc.port_type.is_input() {
                write!(f, "[Input] {}\n", desc.name).unwrap()
            } else {
                write!(f, "[Output] {}\n", desc.name).unwrap()
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for MidiPortDesc {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("name").field(&self.name).finish()
    }
}

#[derive(Default)]
pub struct MidiInput(pub(crate) Option<OsMidiInput>);
unsafe impl Send for MidiInput {}

impl MidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        self.0.as_mut().unwrap().receive()
    }
}

#[derive(Default)]
pub struct MidiOutput(pub(crate) Option<OsMidiOutput>);
unsafe impl Send for MidiOutput {}

impl MidiOutput {
    pub fn send(&self, port: Option<MidiPortId>, data: MidiData) {
        let output = self.0.as_ref().unwrap();
        output.send(port, data);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiData {
    pub data: [u8; 3],
}

impl std::convert::From<u32> for MidiData {
    fn from(data: u32) -> Self {
        MidiData {
            data: [
                ((data >> 16) & 0xff) as u8,
                ((data >> 8) & 0xff) as u8,
                ((data >> 0) & 0xff) as u8,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiPortType {
    Input,
    Output,
}

impl MidiPortType {
    pub fn is_input(&self) -> bool {
        match self {
            Self::Input => true,
            _ => false,
        }
    }
    pub fn is_output(&self) -> bool {
        match self {
            Self::Output => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct MidiPortId(pub LiveId);

#[derive(Clone, PartialEq)]
pub struct MidiPortDesc {
    pub name: String,
    pub port_id: MidiPortId,
    pub port_type: MidiPortType,
}

#[derive(Clone, Copy, Debug)]
pub struct MidiNote {
    pub is_on: bool,
    pub channel: u8,
    pub note_number: u8,
    pub velocity: u8,
}

impl Into<MidiData> for MidiNote {
    fn into(self) -> MidiData {
        MidiData {
            data: [
                (if self.is_on { 0x9 } else { 0x8 } << 4) | self.channel,
                self.note_number,
                self.velocity,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MidiAftertouch {
    pub channel: u8,
    pub note_number: u8,
    pub velocity: u8,
}

impl Into<MidiData> for MidiAftertouch {
    fn into(self) -> MidiData {
        MidiData {
            data: [0xA0 | self.channel, self.note_number, self.velocity],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MidiControlChange {
    pub channel: u8,
    pub param: u8,
    pub value: u8,
}

impl Into<MidiData> for MidiControlChange {
    fn into(self) -> MidiData {
        MidiData {
            data: [0xB0 | self.channel, self.param, self.value],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MidiProgramChange {
    pub channel: u8,
    pub hi: u8,
    pub lo: u8,
}

impl Into<MidiData> for MidiProgramChange {
    fn into(self) -> MidiData {
        MidiData {
            data: [0xC0 | self.channel, self.hi, self.lo],
        }
    }
}

/// Channel pressure. Unlike polyphonic aftertouch this is a TWO-byte message:
/// one 7-bit pressure value, so only the first data byte is part of it.
#[derive(Clone, Copy, Debug)]
pub struct MidiChannelAftertouch {
    pub channel: u8,
    pub value: u16,
}

impl Into<MidiData> for MidiChannelAftertouch {
    fn into(self) -> MidiData {
        MidiData {
            data: [0xD0 | self.channel, (self.value & 0x7f) as u8, 0],
        }
    }
}

/// 14-bit pitch bend, 0..=16383 with 8192 as centre. On the wire the two data
/// bytes are LSB FIRST (status, LSB, MSB) — the opposite of the order they are
/// usually written in, and the classic way this message gets encoded backwards.
#[derive(Clone, Copy, Debug)]
pub struct MidiPitchBend {
    pub channel: u8,
    pub bend: u16,
}

impl Into<MidiData> for MidiPitchBend {
    fn into(self) -> MidiData {
        MidiData {
            data: [
                0xE0 | self.channel,
                ((self.bend as u32) & 0x7f) as u8,
                (((self.bend as u32) >> 7) & 0x7f) as u8,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MidiSystem {
    pub channel: u8,
    pub hi: u8,
    pub lo: u8,
}

impl Into<MidiData> for MidiSystem {
    fn into(self) -> MidiData {
        MidiData {
            data: [0xF0 | self.channel, self.hi, self.lo],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum MidiEvent {
    Note(MidiNote),
    Aftertouch(MidiAftertouch),
    ControlChange(MidiControlChange),
    ProgramChange(MidiProgramChange),
    PitchBend(MidiPitchBend),
    ChannelAftertouch(MidiChannelAftertouch),
    System(MidiSystem),
    Unknown(MidiData),
}

impl MidiEvent {
    pub fn on_note(&self) -> Option<MidiNote> {
        match self {
            Self::Note(note) => Some(*note),
            _ => None,
        }
    }
}

impl MidiData {
    pub fn status(&self) -> u8 {
        self.data[0] >> 4
    }
    pub fn channel(&self) -> u8 {
        self.data[0] & 0xf
    }

    pub fn decode(&self) -> MidiEvent {
        let status = self.status();
        let channel = self.channel();
        match status {
            0x8 | 0x9 => {
                let velocity = self.data[2];
                let is_on = if status == 0x8 || velocity == 0 {
                    false
                } else {
                    true
                };
                MidiEvent::Note(MidiNote {
                    is_on,
                    channel,
                    note_number: self.data[1],
                    velocity,
                })
            }
            0xA => MidiEvent::Aftertouch(MidiAftertouch {
                channel,
                note_number: self.data[1],
                velocity: self.data[2],
            }),
            0xB => MidiEvent::ControlChange(MidiControlChange {
                channel,
                param: self.data[1],
                value: self.data[2],
            }),
            0xC => MidiEvent::ProgramChange(MidiProgramChange {
                channel,
                hi: self.data[1],
                lo: self.data[2],
            }),
            0xD => MidiEvent::ChannelAftertouch(MidiChannelAftertouch {
                channel,
                value: self.data[1] as u16,
            }),
            0xE => MidiEvent::PitchBend(MidiPitchBend {
                channel,
                bend: ((self.data[2] as u16) << 7) | self.data[1] as u16,
            }),
            0xF => MidiEvent::System(MidiSystem {
                channel,
                hi: self.data[1],
                lo: self.data[2],
            }),
            _ => MidiEvent::Unknown(*self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pitch bend is little-endian on the wire: status, LSB, MSB. Encoding it
    /// MSB-first still round-trips through our own decoder, so only a check
    /// against the actual bytes catches it.
    #[test]
    fn pitch_bend_data_bytes_are_lsb_then_msb() {
        let data: MidiData = MidiPitchBend { channel: 0, bend: 8192 }.into();
        assert_eq!(data.data, [0xE0, 0x00, 0x40]);

        // One step above centre moves the LSB only.
        let data: MidiData = MidiPitchBend { channel: 3, bend: 8193 }.into();
        assert_eq!(data.data, [0xE3, 0x01, 0x40]);

        // Full range end points.
        let data: MidiData = MidiPitchBend { channel: 0, bend: 0 }.into();
        assert_eq!(data.data, [0xE0, 0x00, 0x00]);
        let data: MidiData = MidiPitchBend { channel: 0, bend: 16383 }.into();
        assert_eq!(data.data, [0xE0, 0x7f, 0x7f]);
    }

    #[test]
    fn pitch_bend_round_trips_through_the_wire_bytes() {
        for bend in [0u16, 1, 8191, 8192, 8193, 16383] {
            let data: MidiData = MidiPitchBend { channel: 5, bend }.into();
            match data.decode() {
                MidiEvent::PitchBend(pb) => {
                    assert_eq!(pb.bend, bend);
                    assert_eq!(pb.channel, 5);
                }
                other => panic!("expected pitch bend, got {:?}", other),
            }
        }
    }

    /// Channel pressure carries one 7-bit value, so the third byte is not part
    /// of the message and must not be folded into the value.
    #[test]
    fn channel_aftertouch_is_a_two_byte_message() {
        let data: MidiData = MidiChannelAftertouch { channel: 2, value: 64 }.into();
        assert_eq!(data.data, [0xD2, 64, 0]);

        // A stale third byte must not leak into the decoded value.
        let data = MidiData { data: [0xD2, 64, 0x7f] };
        match data.decode() {
            MidiEvent::ChannelAftertouch(at) => assert_eq!(at.value, 64),
            other => panic!("expected channel aftertouch, got {:?}", other),
        }
    }
}
