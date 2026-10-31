//! ALSA sequencer owned by a session's MIDI worker. No blocking reader or global client.
use super::alsa_audio::AlsaError;
use super::alsa_sys::*;
use crate::media::{name_hash, MidiPortInfo};
use std::{collections::VecDeque, ffi::CStr};
macro_rules! alsa_error {
    ($call:expr) => {
        AlsaError::from(stringify!($call), $call)
    };
}
struct Port {
    client_id: i32,
    port_id: i32,
    subscribed: bool,
    desc: MidiPortInfo,
}
pub struct Midi {
    input: *mut snd_seq_t,
    output: *mut snd_seq_t,
    input_id: i32,
    output_id: i32,
    input_port: i32,
    output_port: i32,
    decoder: *mut snd_midi_event_t,
    encoder: *mut snd_midi_event_t,
    ports: Vec<Port>,
    wanted: [Vec<u64>; 2],
    pending: VecDeque<(u64, u32)>,
    pressured: bool,
}
impl Midi {
    pub fn active(&self) -> bool {
        !self.pending.is_empty() || self.ports.iter().any(|p| p.subscribed && !p.desc.output)
    }
    pub fn new() -> Result<Self, String> {
        unsafe {
            let mut this = Self {
                input: std::ptr::null_mut(),
                output: std::ptr::null_mut(),
                input_id: 0,
                output_id: 0,
                input_port: 0,
                output_port: 0,
                decoder: std::ptr::null_mut(),
                encoder: std::ptr::null_mut(),
                ports: Vec::new(),
                wanted: Default::default(),
                pending: VecDeque::new(),
                pressured: false,
            };
            let result = (|| {
                alsa_error!(snd_seq_open(
                    &mut this.input,
                    b"default\0".as_ptr(),
                    SND_SEQ_OPEN_INPUT,
                    1
                ))?;
                alsa_error!(snd_seq_open(
                    &mut this.output,
                    b"default\0".as_ptr(),
                    SND_SEQ_OPEN_OUTPUT,
                    1
                ))?;
                snd_seq_set_client_name(this.input, b"Makepad HAL MIDI In\0".as_ptr());
                snd_seq_set_client_name(this.output, b"Makepad HAL MIDI Out\0".as_ptr());
                this.input_id = snd_seq_client_id(this.input);
                this.output_id = snd_seq_client_id(this.output);
                this.input_port = alsa_error!(snd_seq_create_simple_port(
                    this.input,
                    b"Makepad In\0".as_ptr(),
                    SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_NO_EXPORT,
                    SND_SEQ_PORT_TYPE_APPLICATION
                ))?;
                this.output_port = alsa_error!(snd_seq_create_simple_port(
                    this.output,
                    b"Makepad Out\0".as_ptr(),
                    SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_NO_EXPORT,
                    SND_SEQ_PORT_TYPE_APPLICATION
                ))?;
                alsa_error!(snd_midi_event_new(3, &mut this.decoder))?;
                alsa_error!(snd_midi_event_new(3, &mut this.encoder))?;
                // Every HAL message carries its own status byte, never running status.
                snd_midi_event_no_status(this.decoder, 1);
                Ok::<_, AlsaError>(())
            })();
            result.map_err(|e| e.0)?;
            Ok(this)
        }
    }
    unsafe fn enumerate_ports(&self) -> Result<Vec<Port>, AlsaError> {
        let mut client_info: *mut snd_seq_client_info_t = 0 as *mut _;
        alsa_error!(snd_seq_client_info_malloc(&mut client_info))?;

        let mut port_info: *mut snd_seq_port_info_t = 0 as *mut _;
        if let Err(error) = alsa_error!(snd_seq_port_info_malloc(&mut port_info)) {
            snd_seq_client_info_free(client_info);
            return Err(error);
        }

        snd_seq_client_info_set_client(client_info, -1);
        let mut out_ports = Vec::new();

        while snd_seq_query_next_client(self.input, client_info) == 0 {
            let client_id = snd_seq_client_info_get_client(client_info);
            if client_id == self.input_id || client_id == self.output_id {
                continue;
            }

            snd_seq_port_info_set_client(port_info, client_id);
            snd_seq_port_info_set_port(port_info, -1);
            let client_name = CStr::from_ptr(snd_seq_client_info_get_name(client_info))
                .to_string_lossy()
                .into_owned();
            let _client_type = snd_seq_client_info_get_type(client_info);
            if client_name == "System" {
                continue;
            }
            while snd_seq_query_next_port(self.input, port_info) == 0 {
                let addr: *const snd_seq_addr_t = snd_seq_port_info_get_addr(port_info);
                let caps = snd_seq_port_info_get_capability(port_info);
                let port_name = CStr::from_ptr(snd_seq_port_info_get_name(port_info))
                    .to_string_lossy()
                    .into_owned();
                let is_input = (caps & (SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_SUBS_READ))
                    == (SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_SUBS_READ);
                let is_output = (caps & (SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE))
                    == (SND_SEQ_PORT_CAP_WRITE | SND_SEQ_PORT_CAP_SUBS_WRITE);
                if is_input {
                    out_ports.push(Port {
                        client_id,
                        subscribed: false,
                        port_id: (*addr).port as _,
                        desc: MidiPortInfo {
                            id: name_hash(&format!("{} input", port_name)),
                            name: port_name.clone(),
                            output: false,
                        },
                    })
                }
                if is_output {
                    out_ports.push(Port {
                        client_id,
                        subscribed: false,
                        port_id: (*addr).port as _,
                        desc: MidiPortInfo {
                            id: name_hash(&format!("{} output", port_name)),
                            name: port_name,
                            output: true,
                        },
                    })
                }
            }
        }
        snd_seq_port_info_free(port_info);
        snd_seq_client_info_free(client_info);
        Ok(out_ports)
    }
    pub fn enumerate(&mut self) -> Vec<MidiPortInfo> {
        let fresh = unsafe { self.enumerate_ports() };
        match fresh {
            Ok(mut ports) => {
                // Keep subscriptions of unchanged addresses. Re-enumeration must not
                // introduce a MIDI gap once a second.
                for port in &self.ports {
                    if let Some(fresh) = ports.iter_mut().find(|p| {
                        p.client_id == port.client_id
                            && p.port_id == port.port_id
                            && p.desc == port.desc
                    }) {
                        fresh.subscribed = port.subscribed;
                    } else if port.subscribed {
                        self.subscribe(port, false);
                    }
                }
                self.ports = ports;
                self.apply();
            }
            Err(e) => eprintln!("[hal-midi] enumeration: {}", e.0),
        }
        self.ports.iter().map(|p| p.desc.clone()).collect()
    }
    fn subscribe(&self, port: &Port, on: bool) -> bool {
        unsafe {
            let mut sub = std::ptr::null_mut();
            if snd_seq_port_subscribe_malloc(&mut sub) < 0 {
                return false;
            }
            let (sender, dest, client) = if port.desc.output {
                (
                    snd_seq_addr_t {
                        client: self.output_id as _,
                        port: self.output_port as _,
                    },
                    snd_seq_addr_t {
                        client: port.client_id as _,
                        port: port.port_id as _,
                    },
                    self.output,
                )
            } else {
                (
                    snd_seq_addr_t {
                        client: port.client_id as _,
                        port: port.port_id as _,
                    },
                    snd_seq_addr_t {
                        client: self.input_id as _,
                        port: self.input_port as _,
                    },
                    self.input,
                )
            };
            snd_seq_port_subscribe_set_sender(sub, &sender);
            snd_seq_port_subscribe_set_dest(sub, &dest);
            let result = if on {
                snd_seq_subscribe_port(client, sub)
            } else {
                snd_seq_unsubscribe_port(client, sub)
            };
            if result < 0 {
                eprintln!("[hal-midi] subscription: {result}");
            }
            snd_seq_port_subscribe_free(sub);
            result >= 0
        }
    }
    fn apply(&mut self) {
        for i in 0..self.ports.len() {
            let p = &self.ports[i];
            let on = self.wanted[p.desc.output as usize].contains(&p.desc.id);
            if p.subscribed != on {
                if self.subscribe(p, on) {
                    self.ports[i].subscribed = on;
                }
            }
        }
    }
    pub fn use_ports(&mut self, output: bool, ports: Vec<u64>) {
        self.wanted[output as usize] = ports;
        self.apply();
    }
    pub fn reset(&mut self) {
        self.wanted = Default::default();
        self.apply();
    }
    pub fn send(&mut self, port: u64, data: u32) {
        self.pending.push_back((port, data));
    }
    pub fn poll(&mut self) -> Vec<(u64, u32)> {
        unsafe {
            let mut result = Vec::new();
            loop {
                let mut event = std::ptr::null_mut();
                if snd_seq_event_input(self.input, &mut event) < 0 || event.is_null() {
                    break;
                }
                let mut data = [0u8; 3];
                let n = snd_midi_event_decode(self.decoder, data.as_mut_ptr(), 3, event);
                if n > 0 {
                    if let Some(port) = self.ports.iter().find(|p| {
                        !p.desc.output
                            && p.client_id == (*event).source.client as i32
                            && p.port_id == (*event).source.port as i32
                    }) {
                        result.push((
                            port.desc.id,
                            ((data[0] as u32) << 16) | ((data[1] as u32) << 8) | data[2] as u32,
                        ));
                    }
                }
            }
            while let Some(&(id, data)) = self.pending.front() {
                // Expand broadcast before sending so a full sequencer retries exactly the unsent ports.
                if id == 0 {
                    self.pending.pop_front();
                    for port in self
                        .ports
                        .iter()
                        .rev()
                        .filter(|p| p.desc.output && p.subscribed)
                    {
                        self.pending.push_front((port.desc.id, data));
                    }
                    continue;
                }
                let Some(port) = self.ports.iter().find(|p| p.desc.output && p.desc.id == id)
                else {
                    self.pending.pop_front();
                    continue;
                };
                let bytes = [(data >> 16) as u8, (data >> 8) as u8, data as u8];
                let len = match bytes[0] {
                    0xc0..=0xdf | 0xf1 | 0xf3 => 2,
                    0xf6..=0xff => 1,
                    _ => 3,
                };
                let mut event = std::mem::zeroed();
                snd_midi_event_reset_encode(self.encoder);
                if snd_midi_event_encode(self.encoder, bytes.as_ptr(), len, &mut event) != len {
                    self.pending.pop_front();
                    continue;
                }
                event.source.port = self.output_port as _;
                event.dest.client = port.client_id as _;
                event.dest.port = port.port_id as _;
                event.queue = SND_SEQ_QUEUE_DIRECT as _;
                let sent = snd_seq_event_output_direct(self.output, &mut event);
                if sent == -11 {
                    if !self.pressured {
                        eprintln!("[hal-midi] sequencer full; retaining unsent MIDI");
                        self.pressured = true;
                    }
                    break;
                }
                if sent < 0 {
                    eprintln!("[hal-midi] send: {sent}");
                }
                self.pending.pop_front();
            }
            if self.pending.is_empty() {
                self.pressured = false;
            }
            result
        }
    }
}
impl Drop for Midi {
    fn drop(&mut self) {
        unsafe {
            if !self.decoder.is_null() {
                snd_midi_event_free(self.decoder)
            }
            if !self.encoder.is_null() {
                snd_midi_event_free(self.encoder)
            }
            if !self.input.is_null() {
                snd_seq_close(self.input);
            }
            if !self.output.is_null() {
                snd_seq_close(self.output);
            }
        }
    }
}
