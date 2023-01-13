use {
    std::sync::{Arc, Mutex},
    std::sync::mpsc,
    crate::{
        makepad_live_id::{live_id, LiveId},
        midi::*,
        cx::Cx,
        cx_api::CxOsApi,
        windows_crate::{
            Win32::Media::{  
                MM_MIM_OPEN,
                MM_MIM_CLOSE,
                MM_MIM_DATA,
                MM_MIM_LONGDATA,
                MM_MIM_ERROR,
                MM_MIM_LONGERROR,
                MM_MIM_MOREDATA
            },
            Win32::Media::Audio::{
                CALLBACK_FUNCTION,
                CALLBACK_NULL,
                MIDIINCAPSW,
                MIDIOUTCAPSW,
                HMIDIIN,
                HMIDIOUT,
                midiOutShortMsg,
                midiInOpen,
                midiInStart,
                midiInClose,
                midiOutOpen,
                midiOutClose,
                midiOutGetDevCapsW,
                midiOutGetNumDevs,
                midiInGetDevCapsW,
                midiInGetNumDevs,
            }
        }
    },
};

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
                if let Win32MidiHandle::OpenOut(hmidiout) = port.handle{
                    unsafe{
                        midiOutShortMsg(hmidiout, short_msg);
                    }
                }
            }
        }
    }
} 

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
enum Win32MidiHandle {
    Closed,
    OpenIn(HMIDIIN, *const InputProcContext),
    OpenOut(HMIDIOUT)
}

impl Drop for Win32MidiHandle {
    fn drop(&mut self) {
        match self {
            Self::OpenIn(hmidiin, context_ptr) => {
                unsafe {
                    let _ = Box::from_raw(context_ptr);
                    midiInClose(*hmidiin);
                }
            }
            Self::OpenOut(hmidiout) => {
                unsafe {
                    midiOutClose(*hmidiout);
                }
            }
            _ => ()
        }
    }
}

struct InputProcContext {
    senders: InputSenders,
    midi_port_id: MidiPortId
}

#[allow(non_snake_case)]
unsafe extern "system" fn Win32MidiInputProc(
    _hMidiIn: HMIDIIN,
    wMsg: u32,
    dwInstance: usize,
    dwParam1: usize,
    _wParam2: usize,
) {
    match wMsg {
        MM_MIM_OPEN => (),
        MM_MIM_CLOSE => (),
        MM_MIM_DATA => {
            let data2 = ((dwParam1 >> 16) & 0xff) as u8;
            let data1 = ((dwParam1 >> 8) & 0xff) as u8;
            let data0 = (dwParam1 & 0xff) as u8;
            let context = dwInstance as *const InputProcContext;
            let mut senders = (*context).senders.lock().unwrap();
            senders.retain( | s | {
                s.send(((*context).midi_port_id, MidiData {
                    data0,
                    data1,
                    data2
                })).is_ok()
            });
        },
        MM_MIM_LONGDATA => (),
        MM_MIM_ERROR => (),
        MM_MIM_LONGERROR => (),
        MM_MIM_MOREDATA => (),
        _ => {
            println!("Unexpected midi input message")
        }
    }
    
}
impl Win32MidiHandle {
    
    fn open_input(device_id: u32, midi_port_id: MidiPortId, senders: InputSenders) -> Self {
        unsafe {
            let context_ptr = Box::into_raw(Box::new(InputProcContext {
                senders,
                midi_port_id
            }));
            let cb_ptr = Win32MidiInputProc as *const () as usize;
            let mut hmidiin = HMIDIIN(0);
            if midiInOpen(&mut hmidiin, device_id, cb_ptr, context_ptr as *const _ as usize, CALLBACK_FUNCTION) == 0 {
                midiInStart(hmidiin);
                return Win32MidiHandle::OpenIn(hmidiin, context_ptr);
            }
        }
        Win32MidiHandle::Closed
    }
    
    fn open_output(device_id: u32) -> Self {
        unsafe {
            let mut hmidiout = HMIDIOUT(0);
            if midiOutOpen(&mut hmidiout, device_id, 0, 0, CALLBACK_NULL) == 0 {
                return Win32MidiHandle::OpenOut(hmidiout);
            }
        }
        Win32MidiHandle::Closed
    }
    
    fn is_closed(&self) -> bool {
        match self {
            Self::Closed => true,
            _ => false
        }
    }
}

#[derive(Clone)]
pub struct Win32MidiPort {
    handle: Win32MidiHandle,
    device_id: u32,
    desc: MidiPortDesc
}

pub struct Win32MidiAccess {
    input_senders: InputSenders,
    ports: Vec<Win32MidiPort>,
}


impl Win32MidiAccess {
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_output()) {
                // alright lets open the right handle
                if port.handle.is_closed() {
                    port.handle = Win32MidiHandle::open_output(port.device_id);
                }
            }
        }
        // and the ones disabled
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_output() {
                    port.handle = Win32MidiHandle::Closed;
                }
            }
        }
    }
        
    pub  fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        let input_senders = self.input_senders.clone();
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                // alright lets open the right handle
                if port.handle.is_closed() {
                    port.handle = Win32MidiHandle::open_input(port.device_id, *port_id, input_senders.clone());
                }
            }
        }
        // and the ones disabled
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_input() {
                    port.handle = Win32MidiHandle::Closed;
                }
            }
        }
    }
    
    pub fn new() -> Arc<Mutex<Self>> {
        // alrighty lets initialize midi.
        let input_senders = InputSenders::default();
        Cx::post_signal(live_id!(Win32MidiInputsChanged).into());
        Arc::new(Mutex::new(Win32MidiAccess {
            input_senders,
            ports: Vec::new()
        }))
    }
    
    pub fn update_port_list(&mut self) {
        let old_ports = self.ports.clone();
        self.ports.clear();
        fn szname32_to_string(name: [u16; 32]) -> Option<String> {
            if let Some(end) = name.iter().position( | v | *v == 0) {
                return Some(String::from_utf16(&name[0..end]).unwrap());
            }  
            None
        }
        fn reuse_handle(old_ports: &[Win32MidiPort], port_id: MidiPortId) -> Win32MidiHandle {
            if let Some(old_port) = old_ports.iter().find( | v | v.desc.port_id == port_id) {
                return old_port.handle.clone()
            }
            Win32MidiHandle::Closed
        }
         
        unsafe {     
            for i in 0..midiInGetNumDevs() {
                let mut caps = MIDIINCAPSW::default();
                if midiInGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MIDIOUTCAPSW>() as u32) == 0 {
                    let name = szname32_to_string(caps.szPname).unwrap_or("bad utf16".into());
                    let port_id = self.get_unique_port_id(&name, caps.wMid, caps.wPid);
                    self.ports.push(Win32MidiPort {
                        handle: reuse_handle(&old_ports, port_id),
                        device_id: i,
                        desc: MidiPortDesc {
                            port_type: MidiPortType::Input,
                            name,
                            port_id
                        }
                    });
                }
            }
            for i in 0..midiOutGetNumDevs() {
                let mut caps = MIDIOUTCAPSW::default();
                if midiOutGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MIDIOUTCAPSW>() as u32) == 0 {
                    let name = szname32_to_string(caps.szPname).unwrap_or("bad utf16".into());
                    let port_id = self.get_unique_port_id(&name, caps.wMid, caps.wPid);
                    self.ports.push(Win32MidiPort {
                        handle: reuse_handle(&old_ports, port_id),
                        device_id: i,
                        desc: MidiPortDesc {
                            port_type: MidiPortType::Output,
                            name,
                            port_id
                        }
                    });
                }
            }
        }
    }
    
    pub fn midi_reset(&self){
        self.use_midi_inputs(&self, &[]);
        self.use_midi_outputs(&self, &[]);
        Cx::post_signal(live_id!(Win32MidiInputsChanged).into());
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
