use {
    std::sync::{Arc, Mutex},
    std::sync::mpsc,
    crate::{
        thread::SignalToUI,
        makepad_live_id::{LiveId},
        midi::*,
        os::apple::apple_sys::*,
        os::apple::apple_util::*,
        makepad_objc_sys::objc_block,
    },
};

#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<CoreMidiAccess >>);

pub struct OsMidiInput(mpsc::Receiver<(MidiPortId, MidiData) >);

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        if let Ok((port_id, data)) = self.0.try_recv() {
            return Some((port_id, data))
        }
        None
    }
}

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        let mut words = [0u32; 64];
        words[0] = (0x20000000) | ((d.data[0] as u32) << 16) | ((d.data[1] as u32) << 8) | d.data[2] as u32;
        let event_list = MIDIEventList {
            protocol: kMIDIProtocol_1_0,
            numPackets: 1,
            packet: [MIDIEventPacket {
                timeStamp: 0,
                wordCount: 1,
                words
            }]
        };
        let core_midi = self.0.lock().unwrap();
        for port in &core_midi.ports {
            if port.desc.port_type.is_output()
                && (port_id.is_none() || port.desc.port_id == port_id.unwrap()) {
                unsafe {
                    MIDISendEventList(core_midi.midi_out_port, port.endpoint, &event_list);
                }
            }
        }
    }
}

impl CoreMidiPort {
    unsafe fn new(port_type: MidiPortType, endpoint: MIDIEndpointRef) -> Result<Self,
    OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut uid = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer)) ?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name)) ?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut uid)) ?;
        let name = format!("{} {}", cfstring_ref_to_string(manufacturer), cfstring_ref_to_string(name));
        let port_id = LiveId::from_str(&format!("{}{}", name, uid));
        Ok(Self {
            endpoint,
            desc: MidiPortDesc {
                port_type,
                name,
                port_id: port_id.into()
            }
        })
    }
}

pub struct CoreMidiPort {
    endpoint: MIDIEndpointRef,
    desc: MidiPortDesc
}

type MidiInputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

pub struct CoreMidiAccess {
    change_signal: SignalToUI,
    input_senders: MidiInputSenders,
    midi_in_port: MIDIPortRef,
    midi_out_port: MIDIPortRef,
    ports: Vec<CoreMidiPort>,
}

impl CoreMidiAccess {
    
    pub fn new(change_signal:SignalToUI) -> Arc<Mutex<Self>> {
        let change_signal_clone = change_signal.clone();
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            change_signal_clone.set();
        });
        
        let input_senders = MidiInputSenders::default();
        let senders = input_senders.clone();
        let mut midi_receive = objc_block!(move | event_list: &MIDIEventList, user_data: u64 | {
            let midi_port_id = MidiPortId(LiveId(user_data));
            let mut senders = senders.lock().unwrap();
            let packets = unsafe {std::slice::from_raw_parts(event_list.packet.as_ptr(), event_list.numPackets as usize)};
            for packet in packets {
                for i in 0 .. packet.wordCount.min(64) {
                    let ump = packet.words[i as usize];
                    let ty = ((ump >> 28) & 0xf) as u8;
                    let _group = ((ump >> 24) & 0xf) as u8;
                    let data = [
                        ((ump >> 16) & 0xff) as u8,
                        ((ump >> 8) & 0xff) as u8,
                        (ump & 0xff) as u8
                    ];
                    if ty == 0x02 { // midi 1.0 channel voice
                        senders.retain( | s | {
                            s.send((midi_port_id, MidiData {data})).is_ok()
                        });
                    }
                }
            }
            if senders.len()>0 {
                // make sure our eventloop runs
                SignalToUI::set_ui_signal();
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
            )).unwrap();
            
            OSError::from(MIDIInputPortCreateWithProtocol(
                midi_client,
                ccfstr_from_str("MIDI Input"),
                kMIDIProtocol_1_0,
                &mut midi_in_port,
                &mut midi_receive as *mut _ as ObjcId
            )).unwrap();
            OSError::from(MIDIOutputPortCreate(
                midi_client,
                ccfstr_from_str("MIDI Output"),
                &mut midi_out_port
            )).unwrap();
        }
        change_signal.set();
        Arc::new(Mutex::new(Self {
            change_signal,
            input_senders,
            midi_in_port,
            midi_out_port,
            ports: Vec::new(),
        }))
    }
    
    pub fn use_midi_inputs(&self, ports: &[MidiPortId]) {
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                unsafe {
                    MIDIPortConnectSource(self.midi_in_port, port.endpoint, port.desc.port_id.0.0 as *mut _);
                }
            }
        }
        // and the ones disabled
        for port in &self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_input() {
                    unsafe {
                        MIDIPortDisconnectSource(self.midi_in_port, port.endpoint);
                    }
                }
            }
        }
    }
        
    pub fn use_midi_outputs(&self, _ports: &[MidiPortId]) {
    }

    pub fn midi_port_desc(&self, port:MidiPortId)->Option<MidiPortDesc>{
        if let Some(port) = self.ports.iter().find( | p | p.desc.port_id == port) {
            return Some(port.desc.clone())
        }
        None
    }

    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(OsMidiInput(recv)))
    }
    

    pub fn midi_reset(&self){
        self.use_midi_inputs(&[]);
        self.change_signal.set();
    }
    
    pub fn get_updated_descs(&mut self)-> Vec<MidiPortDesc> {
        self.ports.clear();
        unsafe {
            for i in 0..MIDIGetNumberOfSources() {
                let ep = MIDIGetSource(i);
                if let Ok(port) = CoreMidiPort::new(MidiPortType::Input, ep){
                    self.ports.push(port);
                }
            }
            for i in 0..MIDIGetNumberOfDestinations() {
                let ep = MIDIGetDestination(i);
                if let Ok(port) = CoreMidiPort::new(MidiPortType::Output, ep){
                    self.ports.push(port);
                }
            }
        }
        let mut out = Vec::new();
        for port in &self.ports {
            out.push(port.desc.clone())
        }
        out
    }
}
