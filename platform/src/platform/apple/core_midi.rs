use {
    std::ptr,
    std::mem,
    crate::{
        platform::apple::frameworks::*,
        platform::apple::apple_util::*,
        objc_block,
    },
};

pub struct MidiEndpoint {
    pub id: i32,
    pub name: String,
    pub manufacturer: String,
    endpoint: MIDIEndpointRef
}

#[derive(Clone, Copy, Debug)]
pub struct Midi1Data {
    pub input: usize,
    pub data0: u8,
    pub data1: u8,
    pub data2: u8
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
            input: 0,
            data0: (if self.is_on {0x9}else {0x8} << 4) | self.channel,
            data1: self.note_number,
            data2: self.velocity
        }
    }
}

impl Midi1Data {
    pub fn decode(&self) -> Midi1Event {
        let status = self.data0 >> 4;
        let channel = self.data0 & 0xf;
        match status {
            0x8 | 0x9 => Midi1Event::Note(Midi1Note {is_on: status == 0x9, channel, note_number: self.data1, velocity: self.data2}),
            _ => Midi1Event::Unknown
        }
    }
}

pub struct Midi {
    pub sources: Vec<MidiEndpoint>,
    pub destinations: Vec<MidiEndpoint>
}

impl MidiEndpoint {
    unsafe fn new(endpoint: MIDIEndpointRef) -> Result<Self,
    OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut id = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer)) ?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name)) ?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut id)) ?;
        Ok(Self {
            id,
            name: cfstring_ref_to_string(name),
            manufacturer: cfstring_ref_to_string(manufacturer),
            endpoint
        })
    }
}

impl Midi {
    pub fn new_midi_1_input<F: Fn(Midi1Data) + Send + 'static>(message_callback: F) -> Result<Self,
    OSError> {
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            println!("Midi device added/removed");
        });
        
        let mut midi_receive = objc_block!(move | event_list: &MIDIEventList, user_data: u64 | {
            let packets = unsafe {std::slice::from_raw_parts(event_list.packet.as_ptr(), event_list.numPackets as usize)};
            for packet in packets {
                for i in 0 .. packet.wordCount.min(64) {
                    let ump = packet.words[i as usize];
                    let ty = ((ump >> 28) & 0xf) as u8;
                    let _group = ((ump >> 24) & 0xf) as u8;
                    let data0 = ((ump >> 16) & 0xff) as u8;
                    let data1 = ((ump >> 8) & 0xff) as u8;
                    let data2 = (ump & 0xff) as u8;
                    if ty == 0x02 { // midi 1.0 channel voice
                        message_callback(Midi1Data {
                            input: user_data as usize,
                            data0,
                            data1,
                            data2
                        })
                    }
                }
            }
        });
        
        let mut midi_client = 0 as MIDIClientRef;
        let mut midi_in_port = 0 as MIDIPortRef;
        let mut midi_out_port = 0 as MIDIPortRef;
        let mut destinations = Vec::new();
        let mut sources = Vec::new();
        unsafe {
            OSError::from(MIDIClientCreateWithBlock(
                ccfstr_from_str("Makepad"),
                &mut midi_client,
                &mut midi_notify as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIInputPortCreateWithProtocol(
                midi_client,
                ccfstr_from_str("MIDI Input"),
                kMIDIProtocol_1_0,
                &mut midi_in_port,
                &mut midi_receive as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIOutputPortCreate(
                midi_client,
                ccfstr_from_str("MIDI Output"),
                &mut midi_out_port
            )) ?;
            
            for i in 0..MIDIGetNumberOfDestinations() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetDestination(i)) {
                    destinations.push(ep);
                }
            }
            for i in 0..MIDIGetNumberOfSources() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetSource(i)) {
                    MIDIPortConnectSource(midi_in_port, ep.endpoint, i as *mut _);
                    sources.push(ep);
                }
            }
        }
        
        Ok(Self {
            sources,
            destinations
        })
    }
}