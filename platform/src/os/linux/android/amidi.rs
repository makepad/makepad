#![allow(non_upper_case_globals)]
use {
    std::sync::{Arc, Mutex, mpsc},
    std::ffi::CStr,
    std::os::raw::{
        c_uint,
    },
    super::{
        android_jni::*,
        amidi_sys::*,
    },
    crate::{
        makepad_live_id::*,
        midi::*,
        thread::Signal,
    }
};
 

#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<AMidiAccess >>);

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        //let _ = self.0.lock().unwrap().send_midi(port_id, d);
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct AlsaMidiOutput {
}

pub struct AMidiAccess {
    input_senders: InputSenders,
}

impl AMidiAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        change_signal.set();
        
        let midi_access = Arc::new(Mutex::new(Self {
            input_senders: InputSenders::default(),
        }));
        change_signal.set();
        midi_access
    }
    
    pub fn send_midi(&mut self, port_id: Option<MidiPortId>, d: MidiData) {
       
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn midi_reset(&mut self) {
       
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        
        // enable the ones we use
        /*for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_output()) {
                
            }
        }
        // disable the ones not in the set
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_output() {
                   
                }
            }
        }*/
        //self.event_sender.send(AlsaMidiEvent::UseMidiOutputs(ports.to_vec())).unwrap();
    }
    
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        /*// enable the ones we use
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                
            }
        }
        // disable the ones not in the set
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_input() {
                    
                }
            }
        }*/
    }
    
    pub fn get_updated_descs(&mut self, to_java: &AndroidToJava) -> Vec<MidiPortDesc> {
       Vec::new()
    }
    
}
