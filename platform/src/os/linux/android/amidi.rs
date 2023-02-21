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


// WARNING.. AMidi has inputs and outputs naming reversed,

#[derive(Clone)]
pub struct OsMidiOutput {
    pub (crate) amidi: Arc<Mutex<AMidiAccess >>
}

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, data: MidiData) {
        self.amidi.lock().unwrap().send_midi(port_id, data);
    }
}

pub struct OsMidiInput {
    amidi: Arc<Mutex<AMidiAccess >>,
    recv: mpsc::Receiver<(MidiPortId, MidiData) >
}

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        let mut amidi = self.amidi.lock().unwrap();
        amidi.read_inputs();
        if let Ok((port_id, data)) = self.recv.try_recv() {
            return Some((port_id, data))
        }
        None
    }
}

pub struct AMidiOutput {
    port_id: MidiPortId,
    amidi_port: *mut AMidiInputPort
}

struct AMidiInput {
    port_id: MidiPortId,
    amidi_port: *mut AMidiOutputPort
}

impl AMidiInput {
    fn new(port_id: MidiPortId, device: &AMidiDevicePtr, port: usize) -> Option<Self> {
        let mut amidi_port = std::ptr::null_mut();
        if unsafe {AMidiOutputPort_open(device.amidi_device, port as i32, &mut amidi_port)} != 0
            || amidi_port == std::ptr::null_mut() {
            return None
        };
        Some(Self {
            port_id,
            amidi_port
        })
    }
    fn close(self) {
        unsafe {AMidiOutputPort_close(self.amidi_port)};
    }
}
 
impl AMidiOutput {
    fn new(port_id: MidiPortId, device: &AMidiDevicePtr, port: usize) -> Option<Self> {
        let mut amidi_port = std::ptr::null_mut();
        if unsafe {AMidiInputPort_open(device.amidi_device, port as i32, &mut amidi_port)} != 0
            || amidi_port == std::ptr::null_mut() {
            return None
        };
        Some(Self {
            port_id,
            amidi_port
        })
    }
    fn close(self) {
        unsafe {AMidiInputPort_close(self.amidi_port)};
    }
}


struct AMidiDevicePtr {
    device_name: String,
    port_descs: Vec<MidiPortDesc>,
    amidi_device: *mut AMidiDevice
}

pub struct AMidiAccess {
    init_open_all: bool,
    change_signal: Signal,
    devices: Vec<AMidiDevicePtr>,
    senders: Vec<mpsc::Sender<(MidiPortId, MidiData) >>,
    outputs: Vec<AMidiOutput>,
    inputs: Vec<AMidiInput>
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
            senders: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }));
        midi_access
    }
    
    pub fn read_inputs(&mut self) {
        const MAX_DATA: usize = 3*128;
        let mut data = [0u8; MAX_DATA]; 
        loop {
            let mut any_messages = false;
            for input in &self.inputs {
                let mut opcode = 0;
                let mut time_stamp = 0;
                let mut bytes_recv = 0;
                let messages = unsafe {AMidiOutputPort_receive(
                    input.amidi_port,
                    &mut opcode,
                    data.as_mut_ptr() as _,
                    MAX_DATA as _,
                    &mut bytes_recv,
                    &mut time_stamp 
                )};
                if messages == 1{ 
                    any_messages = true;
                    for i in (0..bytes_recv as usize).step_by(3){
                        let data = MidiData {data:[data[i], data[i+1],data[i+2]]};
                        self.senders.retain( | s | {
                            s.send((input.port_id, data)).is_ok()
                        });
                    }
                }
                if messages <0{
                    crate::log!("ERROR RECEIVING");
                }
            }
            if !any_messages { 
                break;
            }
        }
    }
    
    pub fn send_midi(&mut self, port_id: Option<MidiPortId>,data: MidiData) {
        for output in &self.outputs {
            if port_id.is_none() || port_id == Some(output.port_id){
                unsafe{AMidiInputPort_send(output.amidi_port, data.data.as_ptr() as *const _, 3)};
            } 
        }
    }
    
    pub fn create_midi_input(&mut self, amidi: Arc<Mutex<Self >>) -> MidiInput {
        let (send, recv) = mpsc::channel();
        self.senders.push(send);
        MidiInput(Some(OsMidiInput {
            amidi,
            recv
        }))
    }
    
    pub fn midi_reset(&mut self) {
        
    }
    
    fn find_device_for_port_id(&self, port_id: MidiPortId) -> Option<(usize, usize)> {
        for (device_index, device) in self.devices.iter().enumerate() {
            for (port_index, desc) in device.port_descs.iter().enumerate() {
                if desc.port_id == port_id {
                    return Some((device_index, port_index))
                }
            }
        }
        None
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        // enable the ones we use
        for port_id in ports {
            if self.outputs.iter_mut().find( | p | p.port_id == *port_id).is_none() {
                // new this one
                if let Some((device_index, port_index)) = self.find_device_for_port_id(*port_id) {
                    if let Some(output_port) = AMidiOutput::new(
                        *port_id,
                        &self.devices[device_index],
                        port_index,
                    ) {
                        self.outputs.push(output_port);
                    }
                }
            }
            
        }
        
        // disable the ones not in the set
        let mut i = 0;
        while i < self.outputs.len() {
            let port_id = self.outputs[i].port_id;
            if ports.iter().find( | p | **p == port_id).is_none() {
                self.outputs.remove(i).close();
            }
            else {
                i += 1;
            }
        }
    }
    
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        for port_id in ports { 
            if self.inputs.iter_mut().find( | p | p.port_id == *port_id).is_none() {
                // new this one
                if let Some((device_index, port_index)) = self.find_device_for_port_id(*port_id) {
                    if let Some(input_port) = AMidiInput::new(
                        *port_id,
                        &self.devices[device_index],
                        port_index,
                    ) {
                        self.inputs.push(input_port);
                    }
                }
            }
            
        }
        
        // disable the ones not in the set
        let mut i = 0;
        while i < self.inputs.len() {
            let port_id = self.inputs[i].port_id;
            if ports.iter().find( | p | **p == port_id).is_none() {
                self.inputs.remove(i).close();
            }
            else {
                i += 1;
            }
        }
    }
    
    pub fn received_midi_device(&mut self, device_name: String, java_device: jobject, to_java: &AndroidToJava) {
        
        unsafe {
            if self.devices.iter().find( | v | v.device_name == device_name).is_some() {
                // already have it
                return
            }
            let mut amidi_device = std::ptr::null_mut();
            AMidiDevice_fromJava(to_java.get_env(), java_device, &mut amidi_device);
            // how dow e check if we already had this device
            if amidi_device == std::ptr::null_mut() {
                crate::log!("Received null midi device");
            }
            else {
                // lets query the ports
                let mut port_descs = Vec::new();
                let out_ports = AMidiDevice_getNumInputPorts(amidi_device);
                let in_ports = AMidiDevice_getNumOutputPorts(amidi_device);
                for i in 0..in_ports {
                    let name = format!("{} port {}", device_name, i);
                    port_descs.push(MidiPortDesc {
                        name: format!("{} port {}", device_name, i),
                        port_id: LiveId::from_str_unchecked(&name).into(),
                        port_type: MidiPortType::Input
                    });
                }
                for i in 0..out_ports {
                    let name = format!("{} port {}", device_name, i);
                    port_descs.push(MidiPortDesc {
                        name: format!("{} port {}", device_name, i),
                        port_id: LiveId::from_str_unchecked(&name).into(),
                        port_type: MidiPortType::Output
                    });
                }
                self.devices.push(AMidiDevicePtr {
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
        for device in &self.devices { 
            descs.extend_from_slice(&device.port_descs);
        }
        //let devices = to_java.get_midi_devices();
        //crate::log!("{:#?}", devices);
        Some(descs) 
    }
    
}
