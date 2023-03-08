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
    pub (crate) amidi: Arc<Mutex<AndroidMidiAccess >>
}

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, data: MidiData) {
        self.amidi.lock().unwrap().send_midi(port_id, data);
    }
}

pub struct OsMidiInput {
    amidi: Arc<Mutex<AndroidMidiAccess >>,
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

pub struct AndroidMidiOutput {
    port_id: MidiPortId,
    amidi_port: *mut AMidiInputPort
}

struct AndroidMidiInput {
    port_id: MidiPortId,
    amidi_port: *mut AMidiOutputPort
}

impl AndroidMidiInput {
    fn new(port_id: MidiPortId, device: &AndroidMidiDevicePtr, port: usize) -> Option<Self> {
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
 
impl AndroidMidiOutput {
    fn new(port_id: MidiPortId, device: &AndroidMidiDevicePtr, port: usize) -> Option<Self> {
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


struct AndroidMidiDevicePtr {
    device_name: String,
    port_descs: Vec<MidiPortDesc>,
    amidi_device: *mut AMidiDevice
}

impl AndroidMidiDevicePtr{
    fn release(self) {
        unsafe {AMidiDevice_release(self.amidi_device)};
    }
}

enum AndroidMidiState{
    OpenAllDevices,
    OnErrorReload,
    Ready
}

pub struct AndroidMidiAccess {
    state: AndroidMidiState,
    change_signal: Signal,
    devices: Vec<AndroidMidiDevicePtr>,
    senders: Vec<mpsc::Sender<(MidiPortId, MidiData) >>,
    outputs: Vec<AndroidMidiOutput>,
    inputs: Vec<AndroidMidiInput>
}

impl AndroidMidiAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        // lets request to open midi devices
        // for each device we get
        // we should fire a change event
        change_signal.set();
        let midi_access = Arc::new(Mutex::new(Self {
            state: AndroidMidiState::OpenAllDevices,
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
            for input in &mut self.inputs {
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
                else if messages < 0{
                    self.state = AndroidMidiState::OnErrorReload;
                    self.change_signal.set();
                    // ok so this doesnt work. now what
                    // we should kinda retry 'slowly' like once every second
                }
                // skip the error case for now. rely on other ways
            }
            if !any_messages {  
                // ok so. if we go into a reconnect loop here thats bad.
                // we kinda wanna try a reconnect on this particular device
                
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
    
    
    fn midi_disconnect(&mut self) {
        let mut devices = Vec::new();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        std::mem::swap(&mut devices, &mut self.devices);
        std::mem::swap(&mut inputs, &mut self.inputs);
        std::mem::swap(&mut outputs, &mut self.outputs);
        for input in inputs{
            input.close()
        }
        for output in outputs{
            output.close()
        }
        for device in devices{
            device.release();
        }
    }
    
    pub fn midi_reset(&mut self) {
        self.midi_disconnect();
        self.state = AndroidMidiState::OpenAllDevices;
        self.change_signal.set();
    }
    
    fn find_device_for_port_id(devices: &[AndroidMidiDevicePtr], port_id: MidiPortId) -> Option<(usize, usize)> {
        for (device_index, device) in devices.iter().enumerate() {
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
                if let Some((device_index, port_index)) = Self::find_device_for_port_id(&self.devices,*port_id) {
                    if let Some(output_port) = AndroidMidiOutput::new(
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
                if let Some((device_index, port_index)) = Self::find_device_for_port_id(&self.devices, *port_id) {
                    if let Some(input_port) = AndroidMidiInput::new(
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
    
    pub fn midi_device_opened(&mut self, device_name: String, java_device: jobject, to_java: &AndroidToJava) {
        
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
                self.devices.push(AndroidMidiDevicePtr {
                    device_name,
                    port_descs,
                    amidi_device
                })
            }
            self.change_signal.set();
        }
        
    } 
    
    pub fn get_updated_descs(&mut self, to_java: &AndroidToJava) -> Option<Vec<MidiPortDesc >> {
        match self.state{
            AndroidMidiState::OpenAllDevices=>{
                to_java.open_all_midi_devices(0);
                self.state = AndroidMidiState::Ready;
                None
            }
            AndroidMidiState::OnErrorReload=>{
                self.midi_disconnect();
                to_java.open_all_midi_devices(1000);
                self.state = AndroidMidiState::Ready;
                None
            }
            AndroidMidiState::Ready=>{
                let mut descs = Vec::new();
                for device in &self.devices { 
                    descs.extend_from_slice(&device.port_descs);
                }
                Some(descs) 
            }
        }
    }
    
}
