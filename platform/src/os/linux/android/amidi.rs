#![allow(non_upper_case_globals)]
use {
    std::sync::{Arc, Mutex, mpsc},
    super::{
        android_jni::*,
        jni_sys::jobject,
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
    pub fn send(&self, _port_id: Option<MidiPortId>, _d: MidiData) {
        //let _ = self.0.lock().unwrap().send_midi(port_id, d);
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct AlsaMidiOutput {
}

struct AMidiJavaDevice {
    device_name: String,
    port_descs: Vec<MidiPortDesc>,
    amidi_device: *mut AMidiDevice
}

pub struct AMidiAccess {
    input_senders: InputSenders,
    init_open_all: bool,
    change_signal: Signal,
    devices: Vec<AMidiJavaDevice>
}

impl AMidiAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        // lets request to open midi devices
        // for each device we get
        // we should fire a change event
        change_signal.set();
        let midi_access = Arc::new(Mutex::new(Self {
            init_open_all: true,
            change_signal,
            devices: Default::default(),
            input_senders: InputSenders::default(),
        }));
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
    
    pub fn received_midi_device(&mut self, device_name: String, java_device: jobject, to_java: &AndroidToJava) {
        unsafe {
            let mut amidi_device = std::ptr::null_mut();
            AMidiDevice_fromJava(to_java.get_env(), java_device, &mut amidi_device);
            if amidi_device == std::ptr::null_mut(){
                crate::log!("Received null midi device");
            }
            else{
                // lets query the ports
                let mut port_descs = Vec::new();
                let in_ports = AMidiDevice_getNumInputPorts(amidi_device);
                let out_ports = AMidiDevice_getNumOutputPorts(amidi_device);
                for i in 0..in_ports{
                    let name = format!("{} port {}", device_name, i);
                    port_descs.push(MidiPortDesc{
                        name: format!("{} port {}", device_name, i),
                        port_id: LiveId::from_str_unchecked(&name).into(),
                        port_type: MidiPortType::Input
                    });
                }
                for i in 0..out_ports{
                    let name = format!("{} port {}", device_name, i);
                    port_descs.push(MidiPortDesc{
                        name: format!("{} port {}", device_name, i),
                        port_id: LiveId::from_str_unchecked(&name).into(),
                        port_type: MidiPortType::Output
                    });
                }
                self.devices.push(AMidiJavaDevice{
                    device_name,
                    port_descs,
                    amidi_device
                })
            }
            self.change_signal.set();
        }
        
    }
    
    pub fn get_updated_descs(&mut self, to_java: &AndroidToJava) -> Option<Vec<MidiPortDesc >> {
        if self.init_open_all {
            self.init_open_all = false;
            to_java.open_all_midi_devices();
            return None
        }
        let mut descs = Vec::new();
        // lets query our midi devices for ports/etc
        for device in &self.devices{
            descs.extend_from_slice(&device.port_descs);
        }
        //let devices = to_java.get_midi_devices();
        //crate::log!("{:#?}", devices);
        Some(descs)
    }
    
}
