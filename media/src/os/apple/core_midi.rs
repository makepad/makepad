use {
    crate::{
        midi::*,
        os::apple::frameworks::*,
        makepad_platform::{
            os::apple::apple_util::*,
            objc_block,
        }
    },
};
/*
pub struct MidiEndpoint {
    pub id: i32,
    pub name: String,
    pub manufacturer: String,
    endpoint: MIDIEndpointRef
}*/
/*
pub struct Midi {
    pub sources: Vec<MidiEndpoint>,
    pub destinations: Vec<MidiEndpoint>
}*/

impl MidiInputInfo {
    unsafe fn from_endpoint(endpoint: MIDIEndpointRef) -> Result<Self,
    OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut uid = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer)) ?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name)) ?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut uid)) ?;
        Ok(Self {
            uid: format!("{}", uid),
            name: cfstring_ref_to_string(name),
            manufacturer: cfstring_ref_to_string(manufacturer),
        })
    }
}

pub struct CoreMidiAccess {
    //_midi_client : MIDIClientRef,
    midi_in_port: MIDIPortRef,
    midi_out_port: MIDIPortRef,
    destinations: Vec<MIDIEndpointRef>
}

impl CoreMidiAccess {
    
    pub fn new_midi_input<F, G>(data_callback: F, notify_callback: G) -> Result<Self,
    OSError> where
    F: Fn(Vec<MidiInputData>) + Send + 'static,
    G: Fn() + Send + 'static
    {
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            println!("NOTIFY!");
            notify_callback();
        });
        
        let mut midi_receive = objc_block!(move | event_list: &MIDIEventList, user_data: u64 | {
            let mut datas = Vec::new();
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
                        datas.push(MidiInputData {
                            input_id: user_data as usize,
                            data: MidiData {
                                data0,
                                data1,
                                data2
                            }
                        });
                        
                    }
                }
            }
            if datas.len()>0 {
                data_callback(datas)
            }
        });
        
        let mut midi_client = 0 as MIDIClientRef;
        let mut midi_in_port = 0 as MIDIPortRef;
        let mut midi_out_port = 0 as MIDIPortRef;
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
        }
        Ok(Self {
            midi_in_port,
            midi_out_port,
            destinations: Vec::new()
        })
    }
    
    pub fn send_midi_1_data(&self, d:MidiData){
        let mut words = [0u32;64];
        words[0] = (0x20000000)|((d.data0 as u32)<<16)|((d.data1 as u32)<<8)|d.data2 as u32;
        let event_list = MIDIEventList{
            protocol: kMIDIProtocol_1_0,
            numPackets:1,
            packet:[MIDIEventPacket{
                timeStamp:0,
                wordCount: 1,
                words
            }]
        };
        for dest in &self.destinations{
            println!("SENDING {:?}", d);
            unsafe{
                MIDISendEventList(self.midi_out_port, *dest, &event_list);
            }
        }
    }
    
    pub fn update_destinations(&mut self)  {
        let mut destinations = Vec::new();
        unsafe {
            for i in 0..MIDIGetNumberOfDestinations() {
                let dest =  MIDIGetDestination(i);
                destinations.push(dest);
            }
        }
        self.destinations = destinations;
    }
    
    pub fn connect_all_inputs(&self) -> Vec<MidiInputInfo> {
        /*
        for i in 0..MIDIGetNumberOfDestinations() {
            if let Ok(ep) = MidiEndpoint::new(MIDIGetDestination(i)) {
                destinations.push(ep);
            }
        }
        */
        let mut input_infos = Vec::new();
        unsafe {
            for i in 0..MIDIGetNumberOfSources() {
                let ep = MIDIGetSource(i);
                if let Ok(info) = MidiInputInfo::from_endpoint(ep) {
                    MIDIPortConnectSource(self.midi_in_port, ep, i as *mut _);
                    input_infos.push(info);
                }
            }
        }
        input_infos
    }
}
