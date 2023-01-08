use {
    std::sync::mpsc,
    std::sync::mpsc::TryRecvError,
    crate::{
        makepad_live_id::{LiveId,FromLiveId},
        os::{OsMidiInput, OsMidiOutput}
    }
}; 
 
pub trait MidiOutputApi{
    fn port_desc(&self, port:MidiPortId)->Option<MidiPortDesc>;
    fn set_ports(&self, ports:&[MidiPortId]);
    fn send(&self, port:Option<MidiPortId>, data:MidiData);
}

pub trait MidiInputApi{
    fn port_desc(&self, port:MidiPortId)->Option<MidiPortDesc>;
    fn set_ports(&self, port:&[MidiPortId]);
    fn create_receiver(&self)->MidiReceiver;
}

#[derive(Default)]
pub struct MidiReceiver(pub(crate) Option<mpsc::Receiver<(MidiPortId, MidiData)>>);
unsafe impl Send for MidiReceiver {}

impl MidiReceiver{
     pub fn try_recv(&mut self) -> Result<(MidiPortId, MidiData), TryRecvError> {
         if let Some(recv) = &mut self.0{
             return recv.try_recv()
        }
        Err(TryRecvError::Empty)
    }
}

#[derive(Clone)]
pub struct MidiOutput(pub(crate) OsMidiOutput);

#[derive(Clone)]
pub struct MidiInput(pub(crate) OsMidiInput);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiData {
    pub data0: u8,
    pub data1: u8,
    pub data2: u8
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiPortType{
    Input,
    Output,
}

impl MidiPortType{
    pub fn is_input(&self)->bool{
        match self{
            Self::Input=>true,
            _=>false
        }
    }    
    pub fn is_output(&self)->bool{
        match self{
            Self::Output=>true,
            _=>false
        }
    }    
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct MidiPortId(pub LiveId);

#[derive(Clone, Debug, PartialEq)]
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
