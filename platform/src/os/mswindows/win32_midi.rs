use {
    std::sync::{Arc, Mutex},
    std::sync::mpsc,
    crate::{
        makepad_live_id::{live_id, LiveId},
        midi::*,
    },
};

#[derive(Clone)]
pub struct OsMidiInput(pub(crate) Arc<Mutex<Win32MidiAccess >>);

#[derive(Clone)]
pub struct OsMidiOutput(pub(crate) Arc<Mutex<Win32MidiAccess >>);

impl MidiOutputApi for MidiOutput {
    fn port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc> {
        self.0.0.lock().unwrap().port_desc(port)
    }
    
    fn set_ports(&self, _ports: &[MidiPortId]) {
    }
    
    fn send(&self, _port_id: Option<MidiPortId>, _d: MidiData) {
    }
}

impl MidiInputApi for MidiInput {
    fn port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc> {
        self.0.0.lock().unwrap().port_desc(port)
    }
    
    fn set_ports(&self, ports: &[MidiPortId]) {
        //return;
        if ports.len() == 0{
            return
        }
        let core_midi = self.0.0.lock().unwrap();
        // find all ports we want enabled
        for port_id in ports{
            if let Some(_port) = core_midi.ports.iter().find(|p| p.desc.port_id == *port_id && p.desc.port_type.is_input()){
                //unsafe{
                    //MIDIPortConnectSource(core_midi.midi_in_port, port.endpoint, port.desc.port_id.0.0 as *mut _);
                //}
            }
        }
        // and the ones disabled
        for port in &core_midi.ports{
            if ports.iter().find(|p| **p == port.desc.port_id).is_none(){
                if port.desc.port_type.is_input(){
                    //unsafe{
                       // MIDIPortDisconnectSource(core_midi.midi_in_port, port.endpoint);
                    //}
                }
            }
        }
    }
    
    fn create_receiver(&self)->MidiReceiver{
        let senders = self.0.0.lock().unwrap().input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiReceiver(Some(recv))
    }
}

impl Win32MidiPort {
    unsafe fn new(port_type:MidiPortType) -> Result<Self,()>{
        Ok(Self {
            desc: MidiPortDesc{
                port_type,
                name:"".to_string(),
                manufacturer:"".to_string(),
                port_id: live_id!(a).into()
            }
        })
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData)>>>>;

pub struct Win32MidiPort{
    desc: MidiPortDesc
}

pub struct Win32MidiAccess {
    input_senders: InputSenders,
    ports: Vec<Win32MidiPort>,
}

impl Win32MidiAccess {
    pub fn port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc> {
        if let Some(port) = self.ports.iter().find(|p| p.desc.port_id == port){
            return Some(port.desc.clone())
        }
        None
    }
    
    pub fn new() -> Result<Self,()> {
        Err(())
    }

    pub fn update_port_list(&mut self){
        self.ports.clear();
        //unsafe { 
        //}
    }
    
    pub fn get_ports(&self)->Vec<MidiPortId>{
        let mut out = Vec::new();
        for port in &self.ports{
            out.push(port.desc.port_id)
        }
        out
    }

}
