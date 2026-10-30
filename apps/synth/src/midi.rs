//! MIDI in: every input port in use (a device plugged in later joins when
//! the platform lists it). One thread takes each message as it arrives and
//! forwards it, stamped with when it arrived, to the audio engine's MIDI-in
//! hub (`makepad_audio_engine::feed::midi_in`), so the score's `MidiIn`
//! loops play it on its sample on the audio thread; a copy goes to the UI
//! thread for the composer (`held()`, `cc()`, the Clock's controllers) and
//! the displays. Where the platform has no threads (the web) the UI thread
//! takes and forwards them.
//!
//! Ports a test declares through the remote bridge (`midi_inject`) are
//! adopted as if the platform had listed them.

use makepad_audio_engine::feed::midi_in::{self, MidiMsg};
use makepad_score_live::live::Cmd;
use makepad_score_live::LiveScore;
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// How long the forwarding thread waits for a message before it looks again
/// (injected test messages are seen at its next wake).
const WAIT: Duration = Duration::from_millis(20);

#[derive(Default)]
pub struct Midi {
    started: bool,
    /// The UI thread's own handle where there is no thread.
    input: Option<MidiInput>,
    /// What the thread took, for the composer.
    taken: Option<Receiver<[u8; 3]>>,
}

fn forward(port: MidiPortId, data: MidiData, age: f64) {
    let at = makepad_audio_engine::clock::now() - age.clamp(0.0, 1.0);
    midi_in::push(MidiMsg { at, port: port.0 .0, data: data.data });
}

impl Midi {
    /// Open MIDI in (once).
    pub fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        let mut input = cx.midi_input();
        if makepad_score_live::live::threads_available() {
            let (send, taken) = channel();
            let spawned = cx.thread_spawner().spawn_worker(
                ThreadOptions { name: Some("synth-midi-in".into()), priority: CxThreadPriority::UserInteractive, ..Default::default() },
                move || loop {
                    if let Some((port, data, age)) = input.wait(WAIT) {
                        forward(port, data, age);
                        if send.send(data.data).is_err() {
                            return;
                        }
                        // The desk's lights and the composer follow at once.
                        SignalToUI::set_ui_signal();
                    }
                },
            );
            match spawned {
                Ok(_) => self.taken = Some(taken),
                Err(error) => log!("MIDI in: no thread ({error}); the UI thread takes it"),
            }
            if self.taken.is_some() {
                return;
            }
            self.input = Some(cx.midi_input());
        } else {
            self.input = Some(input);
        }
    }

    /// The ports changed (a device plugged in or out): every input in use,
    /// their names for the `MidiIn` device filters and pickers.
    pub fn handle_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        cx.use_midi_inputs(&ports.all_inputs());
        midi_in::set_ports(ports.descs.iter().filter(|d| d.port_type.is_input()).map(|d| (d.port_id.0 .0, d.name.clone())).collect());
    }

    /// What arrived since the last pump: to the composer (and, without a
    /// thread, to the hub).
    pub fn pump(&mut self, cx: &mut Cx, live: Option<&LiveScore>) {
        if !self.started {
            return;
        }
        if let Some(descs) = makepad_widgets::makepad_platform::midi_inject::take_declared_ports() {
            self.handle_ports(cx, &MidiPortsEvent { descs });
        }
        let send = |data: [u8; 3]| {
            if let Some(live) = live {
                live.send(Cmd::Midi(data));
            }
        };
        if let Some(taken) = &self.taken {
            for data in taken.try_iter().take(256) {
                send(data);
            }
        }
        if let Some(input) = self.input.as_mut() {
            for _ in 0..256 {
                let Some((port, data, age)) = input.receive_timed() else { break };
                forward(port, data, age);
                send(data.data);
            }
        }
    }
}
