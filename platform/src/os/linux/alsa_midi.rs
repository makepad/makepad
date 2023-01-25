#![allow(non_upper_case_globals)]
use {
    std::sync::{Arc, Mutex, mpsc},
    std::ffi::CStr,
    super::{
        alsa_sys::*,
        alsa_audio::AlsaError,
    },
    crate::{
        makepad_live_id::*,
        midi::*,
        thread::Signal,
    }
};


#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<AlsaMidiAccess >>);

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        // alright lets send some midi.
        // send some midi here
        let _ = self.0.lock().unwrap().send_midi(port_id, d);
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct AlsaMidiOutput {
}

pub struct AlsaMidiAccess {
    input_senders: InputSenders,
    //event_sender: mpsc::Sender<AlsaMidiEvent>,
    ports: Vec<AlsaMidiPort>,
    client: Result<AlsaClient, AlsaError>,
}



macro_rules!alsa_error {
    ( $ call: expr) => {
        AlsaError::from(stringify!( $ call), $ call)
    }
}

#[derive(Clone)]
struct AlsaClientPtr(pub *mut snd_seq_t);
unsafe impl Send for AlsaClientPtr {}

#[derive(Clone)]
struct AlsaMidiSendPtr(pub *mut snd_midi_event_t);
unsafe impl Send for AlsaMidiSendPtr {}


struct AlsaClient {
    in_client: AlsaClientPtr,
    in_client_id: i32,
    in_port_id: i32,
    midi_send: AlsaMidiSendPtr,
    out_client: AlsaClientPtr,
    out_client_id: i32,
    out_port_id: i32,
}

const kRequiredInputPortCaps: ::std::os::raw::c_uint =
SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_SUBS_READ;
const kRequiredOutputPortCaps: ::std::os::raw::c_uint =
SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE;
const kCreateOutputPortCaps: ::std::os::raw::c_uint =
SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_NO_EXPORT;
const kCreateInputPortCaps: ::std::os::raw::c_uint =
SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_NO_EXPORT;
const kCreatePortType: ::std::os::raw::c_uint =
SND_SEQ_PORT_TYPE_MIDI_GENERIC | SND_SEQ_PORT_TYPE_APPLICATION;

#[derive(Clone)]
pub struct AlsaMidiPort {
    client_id: i32,
    port_id: i32,
    subscribed: bool,
    desc: MidiPortDesc
}

impl AlsaMidiPort {
    
    unsafe fn subscribe(&mut self, client: &AlsaClient) {
        if !self.subscribed {
            self.subscribed = true;
            self.config_port(client, true);
        }
    }
    
    unsafe fn unsubscribe(&mut self, client: &AlsaClient) {
        if self.subscribed {
            self.subscribed = false;
            self.config_port(client, false);
        }
    }
    
    unsafe fn config_port(&self, client: &AlsaClient, subscribe: bool) {
        let mut subs: *mut snd_seq_port_subscribe_t = 0 as *mut _;
        snd_seq_port_subscribe_malloc(&mut subs);
        
        if self.desc.port_type.is_input() {
            let sender = snd_seq_addr_t {
                client: self.client_id as _,
                port: self.port_id as _
            };
            snd_seq_port_subscribe_set_sender(subs, &sender);
            let dest = snd_seq_addr_t {
                client: client.in_client_id as _,
                port: client.in_port_id as _
            };
            snd_seq_port_subscribe_set_dest(subs, &dest);
            if subscribe {
                alsa_error!(snd_seq_subscribe_port(client.in_client.0, subs)).unwrap();
            }
            else {
                snd_seq_unsubscribe_port(client.in_client.0, subs);
            }
        }
        else {
            let sender = snd_seq_addr_t {
                client: client.out_client_id as _,
                port: client.out_port_id as _
            };
            snd_seq_port_subscribe_set_sender(subs, &sender);
            let dest = snd_seq_addr_t {
                client: self.client_id as _,
                port: self.port_id as _
            };
            snd_seq_port_subscribe_set_dest(subs, &dest);
            if subscribe {
                alsa_error!(snd_seq_subscribe_port(client.in_client.0, subs)).unwrap();
            }
            else {
                snd_seq_unsubscribe_port(client.in_client.0, subs);
            }
        }
    }
}

impl AlsaClient {
    unsafe fn new() -> Result<AlsaClient, AlsaError> {
        let mut in_client: *mut snd_seq_t = 0 as *mut _;
        alsa_error!(snd_seq_open(&mut in_client, "default\0".as_ptr(), SND_SEQ_OPEN_INPUT, 0)) ?;
        alsa_error!(snd_seq_set_client_name(in_client, "Makepad Midi In\0".as_ptr())) ?;
        let in_client_id = snd_seq_client_id(in_client);
        
        let mut out_client: *mut snd_seq_t = 0 as *mut _;
        alsa_error!(snd_seq_open(&mut out_client, "default\0".as_ptr(), SND_SEQ_OPEN_OUTPUT, 0)) ?;
        alsa_error!(snd_seq_set_client_name(out_client, "Makepad Midi Out\0".as_ptr())) ?;
        let out_client_id = snd_seq_client_id(out_client);
        
        let in_port_id = alsa_error!(snd_seq_create_simple_port(
            in_client,
            "Makepad Midi In\0".as_ptr(),
            kCreateInputPortCaps,
            kCreatePortType
        )) ?;
        
        // Subscribe to the announce port.
        let mut subs: *mut snd_seq_port_subscribe_t = 0 as *mut _;
        alsa_error!(snd_seq_port_subscribe_malloc(&mut subs)) ?;
        let announce_sender = snd_seq_addr_t {
            client: SND_SEQ_CLIENT_SYSTEM,
            port: SND_SEQ_PORT_SYSTEM_ANNOUNCE
        };
        let announce_dest = snd_seq_addr_t {
            client: in_client_id as _,
            port: in_port_id as _
        };
        snd_seq_port_subscribe_set_sender(subs, &announce_sender);
        snd_seq_port_subscribe_set_dest(subs, &announce_dest);
        
        alsa_error!(snd_seq_subscribe_port(in_client, subs)) ?;
        
        let out_port_id = alsa_error!(snd_seq_create_simple_port(
            out_client,
            "Makepad Midi Out\0".as_ptr(),
            kCreateOutputPortCaps,
            SND_SEQ_PORT_TYPE_APPLICATION
        )) ?;
        //println!("HERE!");
        
        let mut midi_send: *mut snd_midi_event_t = 0 as * mut _;
        alsa_error!(snd_midi_event_new(256, &mut midi_send))?;
        
        Ok(AlsaClient {
            in_client: AlsaClientPtr(in_client),
            in_client_id,
            in_port_id,
            midi_send: AlsaMidiSendPtr(midi_send),
            out_client: AlsaClientPtr(out_client),
            out_client_id,
            out_port_id,
        })
    }
    
    unsafe fn enumerate_ports(&self) -> Result<Vec<AlsaMidiPort>, AlsaError> {
        
        let mut client_info: *mut snd_seq_client_info_t = 0 as *mut _;
        alsa_error!(snd_seq_client_info_malloc(&mut client_info)) ?;
        
        let mut port_info: *mut snd_seq_port_info_t = 0 as *mut _;
        alsa_error!(snd_seq_port_info_malloc(&mut port_info)) ?;
        
        snd_seq_client_info_set_client(client_info, -1);
        let mut out_ports = Vec::new();
        
        while snd_seq_query_next_client(self.in_client.0, client_info) == 0 {
            let client_id = snd_seq_client_info_get_client(client_info);
            if client_id == self.in_client_id || client_id == self.out_client_id {
                continue;
            }
            
            snd_seq_port_info_set_client(port_info, client_id);
            snd_seq_port_info_set_port(port_info, -1);
            let client_name = CStr::from_ptr(snd_seq_client_info_get_name(client_info)).to_str().unwrap().to_string();
            let _client_type = snd_seq_client_info_get_type(client_info);
            if client_name == "System" {
                continue;
            }
            while snd_seq_query_next_port(self.in_client.0, port_info) == 0 {
                let addr: *const snd_seq_addr_t = snd_seq_port_info_get_addr(port_info);
                let caps = snd_seq_port_info_get_capability(port_info);
                let port_name = CStr::from_ptr(snd_seq_port_info_get_name(port_info)).to_str().unwrap().to_string();
                let is_input = (caps & kRequiredInputPortCaps) == kRequiredInputPortCaps;
                let is_output = (caps & kRequiredOutputPortCaps) == kRequiredOutputPortCaps;
                //println!("GOT PORT {} {} {}", port_name, is_input, is_output);
                //let name = format!("{} {}", client_name, port_name);
                if is_input {
                    out_ports.push(AlsaMidiPort {
                        client_id,
                        subscribed: false,
                        port_id: (*addr).port as _,
                        desc: MidiPortDesc {
                            port_id: LiveId::from_str_unchecked(&format!("{} input", port_name)).into(),
                            name: port_name.clone(),
                            port_type: MidiPortType::Input
                        }
                    })
                }
                if is_output {
                    out_ports.push(AlsaMidiPort {
                        client_id,
                        subscribed: false,
                        port_id: (*addr).port as _,
                        desc: MidiPortDesc {
                            port_id: LiveId::from_str_unchecked(&format!("{} output", port_name)).into(),
                            name: port_name,
                            port_type: MidiPortType::Output
                        }
                    })
                }
            }
        }
        Ok(out_ports)
    }
}

impl AlsaMidiAccess {
    
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        
        change_signal.set();
        
        //let (watch_sender, watch_receiver) = mpsc::channel();
        // let _ = watch_sender.send(AlsaMidiEvent::UpdateDevices);
        let input_senders = InputSenders::default();
        
        let midi_access = Arc::new(Mutex::new(Self {
            client: unsafe {AlsaClient::new()},
            ports: Vec::new(),
            //event_sender: watch_sender.clone(),
            input_senders: input_senders.clone(),
        }));
        
        let midi_access_clone = midi_access.clone();
        let change_signal_clone = change_signal.clone();
        
        let in_client = midi_access_clone.lock().unwrap().client.as_ref().unwrap().in_client.clone();
        
        std::thread::spawn(move || unsafe {
            loop {
                let mut ev: *mut snd_seq_event_t = 0 as *mut _;
                snd_seq_event_input(in_client.0, &mut ev);
                let msg = match (*ev).type_ {
                    SND_SEQ_EVENT_PORT_SUBSCRIBED |
                    SND_SEQ_EVENT_PORT_UNSUBSCRIBED |
                    SND_SEQ_EVENT_CLIENT_CHANGE |
                    SND_SEQ_EVENT_CLIENT_START |
                    SND_SEQ_EVENT_CLIENT_EXIT => None,
                    SND_SEQ_EVENT_PORT_CHANGE |
                    SND_SEQ_EVENT_PORT_START |
                    SND_SEQ_EVENT_PORT_EXIT => {
                        change_signal_clone.set();
                        None
                    },
                    SND_SEQ_EVENT_NOTEON |
                    SND_SEQ_EVENT_NOTEOFF => Some(MidiNote {
                        is_on: (*ev).type_ == SND_SEQ_EVENT_NOTEON,
                        channel: (*ev).data.note.channel,
                        note_number: (*ev).data.note.note,
                        velocity: (*ev).data.note.velocity
                    }.into()),
                    SND_SEQ_EVENT_KEYPRESS => Some(MidiAftertouch {
                        channel: (*ev).data.note.channel,
                        note_number: (*ev).data.note.note,
                        velocity: (*ev).data.note.velocity
                    }.into()),
                    SND_SEQ_EVENT_CONTROLLER => Some(MidiControlChange {
                        channel: (*ev).data.control.channel,
                        param: (*ev).data.control.param as _,
                        value: (*ev).data.control.value as _
                    }.into()),
                    SND_SEQ_EVENT_PGMCHANGE => Some(MidiProgramChange {
                        channel: (*ev).data.control.channel,
                        hi: (*ev).data.control.param as _,
                        lo: (*ev).data.control.value as _
                    }.into()),
                    SND_SEQ_EVENT_CHANPRESS => Some(MidiChannelAftertouch {
                        channel: (*ev).data.control.channel,
                        value: (8192 + (*ev).data.control.value) as _
                    }.into()),
                    SND_SEQ_EVENT_PITCHBEND => Some(MidiPitchBend {
                        channel: (*ev).data.control.channel,
                        bend: (8192 + (*ev).data.control.value) as _
                    }.into()),
                    x => {
                        println!("Unknown alsa midi event {}", x);
                        None
                    }
                };
                if let Some(msg) = msg {
                    if let Some(port_id) = midi_access_clone.lock().unwrap().find_port(
                        (*ev).source.client as i32,
                        (*ev).source.port as i32
                    ) {
                        let mut senders = input_senders.lock().unwrap();
                        senders.retain( | s | {
                            s.send((port_id, msg)).is_ok()
                        });
                        if senders.len()>0 {
                            // make sure our eventloop runs
                            Signal::set_ui_signal();
                        }
                    }
                }
            }
        });
        change_signal.set();
        midi_access
    }
    
    pub fn send_midi(&mut self, port_id: Option<MidiPortId>, d: MidiData) {
        if self.client.is_err() {
            return
        }
        let client = self.client.as_ref().unwrap();
        unsafe {
            for port in &self.ports {
                if port_id.is_none() || Some(port.desc.port_id) == port_id {
                    // send to port
                    let mut event: snd_seq_event_t = std::mem::zeroed();
                    snd_midi_event_reset_encode(client.midi_send.0);
                    let r = snd_midi_event_encode(client.midi_send.0, d.data.as_ptr(), 3, &mut event);
                    if r!= 1{
                        panic!("Unexpected result");
                    }
                    event.source.port = port.port_id as _;
                    event.dest.client = SND_SEQ_ADDRESS_SUBSCRIBERS as _;
                    event.dest.port = SND_SEQ_ADDRESS_UNKNOWN as _;
                    event.queue = SND_SEQ_QUEUE_DIRECT as _;
                    snd_seq_event_output_direct(client.out_client.0, &mut event);                    
                }
            }
        }
    }
    
    pub fn find_port(&self, client_id: i32, port_id: i32) -> Option<MidiPortId> {
        for port in &self.ports {
            if port.client_id == client_id && port.port_id == port_id {
                return Some(port.desc.port_id)
            }
        }
        None
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn midi_reset(&mut self) {
        self.get_updated_descs();
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        if self.client.is_err() {
            return
        }
        // enable the ones we use
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_output()) {
                unsafe {
                    port.subscribe(self.client.as_ref().unwrap())
                }
            }
        }
        // disable the ones not in the set
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_output() {
                    unsafe {
                        port.unsubscribe(self.client.as_ref().unwrap())
                    }
                }
            }
        }
        //self.event_sender.send(AlsaMidiEvent::UseMidiOutputs(ports.to_vec())).unwrap();
    }
    
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        if self.client.is_err() {
            return
        }
        // enable the ones we use
        for port_id in ports {
            if let Some(port) = self.ports.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                unsafe {
                    port.subscribe(self.client.as_ref().unwrap())
                }
            }
        }
        // disable the ones not in the set
        for port in &mut self.ports {
            if ports.iter().find( | p | **p == port.desc.port_id).is_none() {
                if port.desc.port_type.is_input() {
                    unsafe {
                        port.unsubscribe(self.client.as_ref().unwrap())
                    }
                }
            }
        }
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<MidiPortDesc> {
        if self.client.is_err() {
            return Vec::new();
        }
        // alright lets disconnect all midi ports
        for port in &mut self.ports {
            unsafe {port.unsubscribe(self.client.as_ref().unwrap())};
        }
        // replace the ports
        self.ports = if let Ok(client) = &self.client {
            unsafe {client.enumerate_ports().unwrap()}
        }
        else {
            Vec::new()
        };
        let mut descs = Vec::new();
        for port in &self.ports {
            descs.push(port.desc.clone());
        }
        descs
    }
    
}
