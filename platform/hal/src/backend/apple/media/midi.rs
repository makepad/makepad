//! CoreMIDI: the port list, input ports delivering channel messages and output ports sending
//! them. Moved from os/apple/core_midi.rs. Input messages wait in a mailbox the session's step
//! turns into `MidiData` events; the port list is read when the system says it changed.

use super::util::{ccfstr_from_str, cfstring_ref_to_string, OSError};
use super::Signal;
use crate::media::{name_hash, MidiPortInfo};
use makepad_apple_sys::*;
use std::sync::{Arc, Mutex};

pub struct CoreMidiPort {
    endpoint: MIDIEndpointRef,
    desc: MidiPortInfo,
}

impl CoreMidiPort {
    unsafe fn new(output: bool, endpoint: MIDIEndpointRef) -> Result<Self, OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut uid = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer))?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name))?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut uid))?;
        let name = format!("{} {}", cfstring_ref_to_string(manufacturer), cfstring_ref_to_string(name));
        let id = name_hash(&format!("{}{}", name, uid));
        Ok(Self { endpoint, desc: MidiPortInfo { id, output, name } })
    }
}

/// Messages the input ports received and the step has not turned into events: (port, data).
pub type Inbox = Arc<Mutex<Vec<(u64, u32)>>>;

pub struct CoreMidiAccess {
    change_signal: Signal,
    pub inbox: Inbox,
    midi_in_port: MIDIPortRef,
    midi_out_port: MIDIPortRef,
    ports: Vec<CoreMidiPort>,
}

// SAFETY: CoreMIDI refs are usable from any thread
unsafe impl Send for CoreMidiAccess {}

impl CoreMidiAccess {
    /// `change_signal` rings when the ports change, `data_signal` when input messages arrived.
    pub fn new(change_signal: Signal, data_signal: Signal) -> Arc<Mutex<Self>> {
        let change_signal_clone = change_signal.clone();
        let mut midi_notify = objc_block!(move |_notification: &MIDINotification| {
            change_signal_clone.set();
        });

        let inbox = Inbox::default();
        let receiving = inbox.clone();
        let received = data_signal;
        let mut midi_receive = objc_block!(move |event_list: &MIDIEventList, user_data: u64| {
            let mut any = false;
            {
                let mut inbox = receiving.lock().unwrap_or_else(|e| e.into_inner());
                let packets = unsafe { std::slice::from_raw_parts(event_list.packet.as_ptr(), event_list.numPackets as usize) };
                for packet in packets {
                    for i in 0..packet.wordCount.min(64) {
                        let ump = packet.words[i as usize];
                        let ty = ((ump >> 28) & 0xf) as u8;
                        if ty == 0x02 {
                            // midi 1.0 channel voice: status, data1, data2
                            inbox.push((user_data, ump & 0x00ff_ffff));
                            any = true;
                        }
                    }
                }
            }
            if any {
                // make sure the session's pump runs
                received.set();
            }
        });

        let mut midi_client = 0 as MIDIClientRef;
        let mut midi_in_port = 0 as MIDIPortRef;
        let mut midi_out_port = 0 as MIDIPortRef;
        unsafe {
            OSError::from(MIDIClientCreateWithBlock(ccfstr_from_str("Makepad"), &mut midi_client, &mut midi_notify as *mut _ as ObjcId)).unwrap();
            OSError::from(MIDIInputPortCreateWithProtocol(midi_client, ccfstr_from_str("MIDI Input"), kMIDIProtocol_1_0, &mut midi_in_port, &mut midi_receive as *mut _ as ObjcId)).unwrap();
            OSError::from(MIDIOutputPortCreate(midi_client, ccfstr_from_str("MIDI Output"), &mut midi_out_port)).unwrap();
        }
        change_signal.set();
        Arc::new(Mutex::new(Self { change_signal, inbox, midi_in_port, midi_out_port, ports: Vec::new() }))
    }

    pub fn use_midi_inputs(&self, ports: &[u64]) {
        // find all ports we want enabled
        for port_id in ports {
            if let Some(port) = self.ports.iter().find(|p| p.desc.id == *port_id && !p.desc.output) {
                unsafe {
                    MIDIPortConnectSource(self.midi_in_port, port.endpoint, port.desc.id as *mut _);
                }
            }
        }
        // and the ones disabled
        for port in &self.ports {
            if !ports.contains(&port.desc.id) && !port.desc.output {
                unsafe {
                    MIDIPortDisconnectSource(self.midi_in_port, port.endpoint);
                }
            }
        }
    }

    pub fn midi_reset(&self) {
        self.use_midi_inputs(&[]);
        self.change_signal.set();
    }

    /// One 3-byte channel message to `port`, or to every output port when `port` is 0.
    pub fn send(&self, port: u64, data: u32) {
        let (status, d1, d2) = ((data >> 16) & 0xff, (data >> 8) & 0xff, data & 0xff);
        let mut words = [0u32; 64];
        // The universal-packet message type has to match the status: type 1 is system real-time
        // and common, type 2 is a channel voice message. Hard-coding 2 made a clock tick a
        // malformed voice packet, which is dropped rather than sounded.
        let message_type: u32 = if status >= 0xf0 { 0x1 } else { 0x2 };
        words[0] = (message_type << 28) | (status << 16) | (d1 << 8) | d2;
        let event_list = MIDIEventList { protocol: kMIDIProtocol_1_0, numPackets: 1, packet: [MIDIEventPacket { timeStamp: 0, wordCount: 1, words }] };
        for p in &self.ports {
            if p.desc.output && (port == 0 || p.desc.id == port) {
                unsafe {
                    MIDISendEventList(self.midi_out_port, p.endpoint, &event_list);
                }
            }
        }
    }

    pub fn get_updated_descs(&mut self) -> Vec<MidiPortInfo> {
        self.ports.clear();
        unsafe {
            for i in 0..MIDIGetNumberOfSources() {
                let ep = MIDIGetSource(i);
                if let Ok(port) = CoreMidiPort::new(false, ep) {
                    self.ports.push(port);
                }
            }
            for i in 0..MIDIGetNumberOfDestinations() {
                let ep = MIDIGetDestination(i);
                if let Ok(port) = CoreMidiPort::new(true, ep) {
                    self.ports.push(port);
                }
            }
        }
        self.ports.iter().map(|p| p.desc.clone()).collect()
    }
}
