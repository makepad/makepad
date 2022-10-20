#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiInputData {
    pub input_id: usize,
    pub data: MidiData,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiData {
    pub data0: u8,
    pub data1: u8,
    pub data2: u8
}

#[derive(Clone, Debug, PartialEq)]
pub struct MidiInputInfo {
    pub manufacturer: String,
    pub name: String,
    pub uid: String,
}

#[derive(Clone, Copy, Debug)]
pub struct MidiNote {
    pub is_on: bool,
    pub channel: u8,
    pub note_number: u8,
    pub velocity: u8,
}


#[derive(Clone, Copy, Debug)]
pub enum MidiEvent {
    Note(MidiNote),
    Unknown
}

impl MidiEvent {
    pub fn on_note(&self) -> Option<MidiNote> {
        match self {
            Self::Note(note) => Some(*note),
            Self::Unknown => None
        }
    }
}

impl Into<MidiData> for MidiNote {
    fn into(self) -> MidiData {
        MidiData {
            data0: (if self.is_on {0x9}else {0x8} << 4) | self.channel,
            data1: self.note_number,
            data2: self.velocity
        }
    }
}

impl MidiData {
    pub fn status(&self) -> u8 {
        self.data0 >> 4
    }
    pub fn channel(&self) -> u8 {
        self.data0 & 0xf
    }
    
    pub fn decode(&self) -> MidiEvent {
        let status = self.status();
        let channel = self.channel();
        match status {
            0x8 | 0x9 => MidiEvent::Note(MidiNote {is_on: status == 0x9, channel, note_number: self.data1, velocity: self.data2}),
            _ => MidiEvent::Unknown
        }
    }
}
