#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Midi1InputData{
    pub input_id: usize,
    pub data: Midi1Data,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Midi1Data {
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
pub struct Midi1Note {
    pub is_on: bool,
    pub channel: u8,
    pub note_number: u8,
    pub velocity: u8,
}


#[derive(Clone, Copy, Debug)]
pub enum Midi1Event {
    Note(Midi1Note),
    Unknown
}

impl Into<Midi1Data> for Midi1Note {
    fn into(self) -> Midi1Data {
        Midi1Data {
            data0: (if self.is_on {0x9}else {0x8} << 4) | self.channel,
            data1: self.note_number,
            data2: self.velocity
        }
    }
}

impl Midi1Data {
    pub fn status(&self) -> u8 {
        self.data0>>4
    }
    pub fn channel(&self) -> u8 {
        self.data0 & 0xf
    }
    
    pub fn decode(&self) -> Midi1Event {
        let status = self.status();
        let channel = self.channel();
        match status {
            0x8 | 0x9 => Midi1Event::Note(Midi1Note {is_on: status == 0x9, channel, note_number: self.data1, velocity: self.data2}),
            _ => Midi1Event::Unknown
        }
    }
}
