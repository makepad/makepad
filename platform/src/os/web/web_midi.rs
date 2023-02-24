use {
    std::sync::{mpsc, mpsc::TryRecvError, Arc, Mutex},
    self::super::{
        from_wasm::{FromWasmQueryMidiPorts, FromWasmSendMidiOutput, FromWasmUseMidiInputs},
        to_wasm::{ToWasmMidiPortList, ToWasmMidiInputData}
    },
    crate::{
        makepad_live_id::*,
        makepad_wasm_bridge::{FromWasmMsg},
        midi::*,
        thread::Signal,
        os::web::CxOs,
    }
};

pub struct OsMidiOutput {
    sender: mpsc::Sender<(Option<MidiPortId>, MidiData)>
}

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
        let _ = self.sender.send((port_id, d));
        Signal::set_ui_signal();
    }
}

#[derive(Default)]
pub struct WebMidiAccess {
    output_receivers: Vec<mpsc::Receiver<(Option<MidiPortId>, MidiData) >>,
    input_senders: Vec<mpsc::Sender<(MidiPortId, MidiData) >>,
    change_signal: Signal,
    ports: Vec<WebMidiPort>,
}

struct WebMidiPort {
    uid: String,
    desc: MidiPortDesc
}

impl WebMidiAccess {
    pub fn new(os: &mut CxOs, change_signal: Signal) -> Arc<Mutex<Self >> {
        os.from_wasm(FromWasmQueryMidiPorts {});
        
        Arc::new(Mutex::new(Self {
            output_receivers: Default::default(),
            input_senders: Default::default(),
            change_signal,
            ports: Default::default(),
        }))
    }
    
    pub fn create_midi_input(&mut self) -> MidiInput {
        let (send, recv) = mpsc::channel();
        self.input_senders.push(send);
        MidiInput(Some(OsMidiInput(recv)))
    }
    
    pub fn create_midi_output(&mut self) -> MidiOutput {
        let (send, _recv) = mpsc::channel();
        MidiOutput(Some(OsMidiOutput {
            sender: send
        }))
    }
    
    pub fn use_midi_inputs(&mut self, os: &mut CxOs, port_ids: &[MidiPortId]) {
        // send to wasm the list of midi inputs we wanna use
        let mut input_uids = Vec::new();
        for port_id in port_ids {
            if let Some(port) = self.ports.iter().find( | v | v.desc.port_id == *port_id) {
                input_uids.push(port.uid.clone())
            }
        }
        os.from_wasm(FromWasmUseMidiInputs {
            input_uids
        })
    }
    
    pub fn use_midi_outputs(&mut self, _os: &mut CxOs, _ports: &[MidiPortId]) {
    }
    
    pub fn midi_reset(&mut self, os: &mut CxOs) {
        os.from_wasm(FromWasmUseMidiInputs {input_uids:vec![]});
        os.from_wasm(FromWasmQueryMidiPorts {});
    }
    
    pub fn to_wasm_midi_input_data(&mut self, tw:ToWasmMidiInputData){
        if let Some(port) = self.ports.iter().find(|v| v.uid == tw.uid){
            let data = MidiData{data:[((tw.data>>16)&0xff) as u8,((tw.data>>8)&0xff) as u8,((tw.data>>0)&0xff) as u8]};
            self.input_senders.retain(|send|{
                send.send((port.desc.port_id, data)).is_ok()
            })
        }
    }
        
    pub fn to_wasm_midi_port_list(&mut self, tw: ToWasmMidiPortList) {
        self.ports.clear();
        for port in tw.ports {
            self.ports.push(WebMidiPort {
                desc: MidiPortDesc {
                    name: port.name,
                    port_id: LiveId::from_str_unchecked(&port.uid).into(),
                    port_type: if port.is_output {MidiPortType::Output}else {MidiPortType::Input}
                },
                uid: port.uid,
            })
        }
        self.change_signal.set()
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<MidiPortDesc> {
        let mut descs = Vec::new();
        for port in &self.ports {
            descs.push(port.desc.clone())
        }
        descs
    }
    
    pub fn send_midi_output_data(&mut self, from_wasm: &mut FromWasmMsg) {
        let ports = &self.ports;
        self.output_receivers.retain( | recv | {
            loop {
                match recv.try_recv() {
                    Ok((port_id, d)) => {
                        for port in ports {
                            if port_id.is_none() || Some(port.desc.port_id) == port_id {
                                from_wasm.from_wasm(FromWasmSendMidiOutput {
                                    uid: port.uid.clone(),
                                    data: (d.data[0] as u32) << 16 | (d.data[1] as u32) << 8 | (d.data[2] as u32) << 0
                                })
                            }
                        }
                    },
                    Err(TryRecvError::Empty) => {
                        return true
                    },
                    Err(TryRecvError::Disconnected) => {
                        return false
                    }
                }
            }
        })
    }
    
}

impl CxOs {
    
    pub fn handle_web_midi_signals(&mut self) {
        // lets receive output and send
        if let Some(web_midi) = &self.media.web_midi {
            web_midi.lock().unwrap().send_midi_output_data(self.from_wasm.as_mut().unwrap());
        }
    }
}
