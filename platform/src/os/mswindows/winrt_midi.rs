use { 
    std::sync::{Arc, Mutex},     
    std::sync::mpsc,
    crate::{
        makepad_live_id::{live_id, LiveId},
        midi::*,
        cx::Cx,
        cx_api::CxOsApi,
        windows_crate::{
            Devices::Enumeration::DeviceInformation,
            Devices::Midi::MidiInPort,
        }
    }, 
};
type WindowsResult<T> = crate::windows_crate::core::Result<T>;

async fn enumerate_midi_devices() -> WindowsResult<()> {
    let input_query = MidiInPort::GetDeviceSelector().unwrap();
    let collection = DeviceInformation::FindAllAsyncAqsFilter(&input_query)?.await?;
    for item in collection{
        println!("{}", item.Name().unwrap()) 
    }
    println!("GOT HERE"); 
    Ok(()) 
} 


#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<Win32MidiAccess >>);

impl OsMidiOutput{
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        // send to a specific port or all ports
        let mut win32_midi = self.0.lock().unwrap();
        let short_msg = ((d.data2 as u32) << 16) | ((d.data1 as u32) << 8) | d.data0 as u32;
        for port in &mut win32_midi.ports {
            if port.desc.port_type.is_output()
                && (port_id.is_none() || port.desc.port_id == port_id.unwrap()) {
                /*if let Win32MidiHandle::OpenOut(hmidiout) = port.handle{
                    unsafe{
                        midiOutShortMsg(hmidiout, short_msg);
                    }
                }*/
            }
        }
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct Win32MidiPort {
    desc: MidiPortDesc
}

pub struct Win32MidiAccess {
    input_senders: InputSenders,
    ports: Vec<Win32MidiPort>,
}


impl Win32MidiAccess {
    
    pub fn new() -> Result<Self,
    ()> {
        std::thread::spawn(move || {
            // lets enumerate devices
            futures::executor::block_on(enumerate_midi_devices());
            
            
        });
            
        // alrighty lets initialize midi.
        let input_senders = InputSenders::default();
        Cx::post_signal(live_id!(Win32MidiInputsChanged).into());
        Ok(Win32MidiAccess {
            input_senders,
            ports: Vec::new()
        })
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        if ports.len() == 0 {
            return
        }
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_output()) {
                // open this output
            }
        }
        // and the ones disabled
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() && port.desc.port_type.is_output(){
               // close this output
            }
        }
    }
        
    pub  fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        //return;
        if ports.len() == 0 {
            return
        }
        let input_senders = self.input_senders.clone();
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                // open this input
            }
        }
        // and the ones disabled
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() && port.desc.port_type.is_input() {
                //close this input
            }
        }
    }
    
    
    pub fn update_port_list(&mut self) {
        let old_ports = self.ports.clone();
        self.ports.clear();
        
    }
    
    pub fn get_unique_port_id(&self, name: &str, w_mid: u16, w_pid: u16) -> MidiPortId {
        let name = format!("{}{}{}", name, w_mid, w_pid);
        for i in 0..100 {
            let port_id = LiveId::from_str_unchecked(&format!("{}{}", name, i)).into();
            if self.ports.iter().find( | v | v.desc.port_id == port_id).is_none() {
                return port_id
            }
        }
        panic!("No unique midi port id available")
    }
    
    pub fn get_descs(&self) -> Vec<MidiPortDesc> {
        let mut out = Vec::new();
        for port in &self.ports {
            out.push(port.desc.clone())
        }
        out
    }
    
}
