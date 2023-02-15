#![allow(non_upper_case_globals)]
use {
    std::sync::{Arc, Mutex, mpsc},
    super::{
        android_jni::*,
        jni_sys::jobject,
        amidi_sys::*,
    },
    crate::{
        midi::*,
        thread::Signal,
    }
};


#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<AMidiAccess >>);

impl OsMidiOutput {
    pub fn send(&self, _port_id: Option<MidiPortId>, _d: MidiData) {
        //let _ = self.0.lock().unwrap().send_midi(port_id, d);
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct AlsaMidiOutput {
}

struct AMidiDevice {
    
}

pub struct AMidiAccess {
    input_senders: InputSenders,
    init_open_all: bool,
    devices: Vec<AMidiDevice>
}

impl AMidiAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        change_signal.set();
        // lets request to open midi devices
        // for each device we get
        // we should fire a change event
        
        let midi_access = Arc::new(Mutex::new(Self {
            init_open_all: true,
            devices: Default::default(),
            input_senders: InputSenders::default(),
        }));
        change_signal.set();
        midi_access
    }
    
    pub fn send_midi(&mut self, _port_id: Option<MidiPortId>, _d: MidiData) {
        
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn midi_reset(&mut self) {
        
    }
    
    pub fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
        
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
    
    pub fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
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
    
    pub fn received_midi_device(&mut self, _name: String, device: jobject, to_java: &AndroidToJava) {
        unsafe {
            let mut amidi_device = std::ptr::null_mut();
            crate::log!("GOT HERE {}", _name);
            AMidiDevice_fromJava(to_java.get_env(), device, &mut amidi_device);
              
        }
        
    }
    
    pub fn get_updated_descs(&mut self, to_java: &AndroidToJava) -> Option<Vec<MidiPortDesc >> {
        if self.init_open_all {
            self.init_open_all = false;
            to_java.open_all_midi_devices();
            return None
        }
        // lets query our midi devices for ports/etc
        
        //let devices = to_java.get_midi_devices();
        //crate::log!("{:#?}", devices);
        Some(Vec::new())
    }
    
}
