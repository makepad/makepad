use {
    crate::{
        makepad_live_id::LiveId,
        midi::*,
        thread::SignalToUI,
        windows::{
            Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher},
            Devices::Midi::{IMidiOutPort, MidiInPort, MidiMessageReceivedEventArgs, MidiOutPort},
            Foundation::TypedEventHandler,
            Storage::Streams::{DataReader, DataWriter},
        },
    },
    makepad_futures_legacy::executor,
    std::sync::mpsc,
    std::sync::{Arc, Mutex},
};

type WindowsResult<T> = crate::windows::core::Result<T>;

pub struct OsMidiInput(mpsc::Receiver<(MidiPortId, MidiData)>);

#[derive(Clone)]
pub struct OsMidiOutput(pub(crate) Arc<Mutex<WinRTMidiAccess>>);

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .event_sender
            .send(WinRTMidiEvent::SendMidi(port_id, d));
    }
}

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        if let Ok((port_id, data)) = self.0.try_recv() {
            return Some((port_id, data));
        }
        None
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData)>>>>;

#[derive(Clone)]
pub struct WinRTMidiPort {
    winrt_id: String,
    desc: MidiPortDesc,
}

#[derive(Clone)]
pub struct WinRTMidiInput {
    port_id: MidiPortId,
    event_token: i64,
    midi_input: MidiInPort,
}

#[derive(Clone)]
pub struct WinRTMidiOutput {
    port_id: MidiPortId,
    midi_output: IMidiOutPort,
}

pub struct WinRTMidiAccess {
    input_senders: InputSenders,
    event_sender: mpsc::Sender<WinRTMidiEvent>,
    descs: Vec<MidiPortDesc>,
}

#[derive(Clone)]
enum WinRTMidiEvent {
    UpdateDevices,
    SendMidi(Option<MidiPortId>, MidiData),
    UseMidiInputs(Vec<MidiPortId>),
    UseMidiOutputs(Vec<MidiPortId>),
}

impl WinRTMidiAccess {
    async fn create_midi_in_port(winrt_id: &str) -> WindowsResult<MidiInPort> {
        let port = MidiInPort::FromIdAsync(&winrt_id.into())?.await?;
        Ok(port)
    }

    async fn create_midi_out_port(winrt_id: &str) -> WindowsResult<IMidiOutPort> {
        let port = MidiOutPort::FromIdAsync(&winrt_id.into())?.await?;
        Ok(port)
    }

    async fn get_ports_list() -> WindowsResult<Vec<WinRTMidiPort>> {
        let input_query = MidiInPort::GetDeviceSelector().unwrap();
        let mut ports = Vec::new();
        let collection = DeviceInformation::FindAllAsyncAqsFilter(&input_query)?.await?;
        for item in collection {
            let winrt_id = item.Id().unwrap().to_string();
            ports.push(WinRTMidiPort {
                desc: MidiPortDesc {
                    name: item.Name().unwrap().to_string(),
                    port_id: LiveId::from_str(&winrt_id).into(),
                    port_type: MidiPortType::Input,
                },
                winrt_id,
            });
        }
        let output_query = MidiOutPort::GetDeviceSelector().unwrap();
        let collection = DeviceInformation::FindAllAsyncAqsFilter(&output_query)?.await?;
        for item in collection {
            let winrt_id = item.Id().unwrap().to_string();
            ports.push(WinRTMidiPort {
                desc: MidiPortDesc {
                    name: item.Name().unwrap().to_string(),
                    port_id: LiveId::from_str(&winrt_id).into(),
                    port_type: MidiPortType::Output,
                },
                winrt_id,
            });
        }
        Ok(ports)
    }

    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        let (watch_sender, watch_receiver) = mpsc::channel();
        let input_senders = InputSenders::default();
        let midi_access = Arc::new(Mutex::new(Self {
            descs: Vec::new(),
            event_sender: watch_sender.clone(),
            input_senders,
        }));
        let midi_access_clone = midi_access.clone();
        let change_signal_clone = change_signal.clone();
        std::thread::spawn(move || {
            let mut ports_list = Vec::new();

            let mut midi_inputs: Vec<WinRTMidiInput> = Vec::new();
            let mut midi_outputs: Vec<WinRTMidiOutput> = Vec::new();

            // initiate device list update
            watch_sender.send(WinRTMidiEvent::UpdateDevices).unwrap();
            // now lets watch device changes
            let query = MidiInPort::GetDeviceSelector().unwrap();
            let input_watcher = DeviceInformation::CreateWatcherAqsFilter(&query).unwrap();
            // The OUTPUT selector, which this asked for by name and then
            // passed the input one to: both watchers watched inputs, so a
            // device with no input end -- a synth, a lighting interface --
            // arriving or leaving mid-session announced itself to nobody,
            // and the enumeration below (which does list outputs) was never
            // asked to run. A device with both ends masked it.
            let query = MidiOutPort::GetDeviceSelector().unwrap();
            let output_watcher = DeviceInformation::CreateWatcherAqsFilter(&query).unwrap();

            fn bind_watcher(watch_sender: mpsc::Sender<WinRTMidiEvent>, watcher: &DeviceWatcher) {
                let sender = watch_sender.clone();
                watcher
                    .Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
                        move |_, _| {
                            let _ = sender.send(WinRTMidiEvent::UpdateDevices);
                            Ok(())
                        },
                    ))
                    .unwrap();
                let sender = watch_sender.clone();
                watcher
                    .Removed(
                        &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
                            move |_, _| {
                                let _ = sender.send(WinRTMidiEvent::UpdateDevices);
                                Ok(())
                            },
                        ),
                    )
                    .unwrap();
                let sender = watch_sender.clone();
                watcher
                    .Updated(
                        &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
                            move |_, _| {
                                let _ = sender.send(WinRTMidiEvent::UpdateDevices);
                                Ok(())
                            },
                        ),
                    )
                    .unwrap();
                let sender = watch_sender.clone();
                watcher
                    .EnumerationCompleted(&TypedEventHandler::new(move |_, _| {
                        let _ = sender.send(WinRTMidiEvent::UpdateDevices);
                        Ok(())
                    }))
                    .unwrap();
            }

            bind_watcher(watch_sender.clone(), &input_watcher);
            bind_watcher(watch_sender.clone(), &output_watcher);
            input_watcher.Start().unwrap();
            output_watcher.Start().unwrap();

            while let Ok(msg) = watch_receiver.recv() {
                match msg {
                    WinRTMidiEvent::UpdateDevices => {
                        // A device leaving mid-enumeration fails this. The
                        // last list is better than no MIDI at all for the
                        // rest of the session, which is what taking the
                        // thread down means.
                        let listed = executor::block_on(Self::get_ports_list());
                        ports_list = match listed {
                            Ok(list) => list,
                            Err(error) => {
                                crate::log!("midi: device list failed ({error:?}), keeping the last one");
                                continue;
                            }
                        };
                        let mut descs = Vec::new();
                        for port in &ports_list {
                            descs.push(port.desc.clone());
                        }
                        midi_access_clone.lock().unwrap().descs = descs;
                        change_signal_clone.set();
                    }
                    WinRTMidiEvent::UseMidiOutputs(ports) => {
                        //let cself = midi_access_clone.lock().unwrap();
                        // find all ports we want enabled
                        for port_id in &ports {
                            if let Some(port) = ports_list.iter_mut().find(|p| {
                                p.desc.port_id == *port_id && p.desc.port_type.is_output()
                            }) {
                                if midi_outputs
                                    .iter()
                                    .find(|v| v.port_id == *port_id)
                                    .is_some()
                                {
                                    continue;
                                }
                                // open this output
                                if let Ok(midi_output) =
                                    executor::block_on(Self::create_midi_out_port(&port.winrt_id))
                                {
                                    midi_outputs.push(WinRTMidiOutput {
                                        port_id: *port_id,
                                        midi_output,
                                    });
                                } else {
                                    crate::log!(
                                        "Midi output could not be created {}",
                                        port.desc.name
                                    );
                                }
                            }
                        }
                        let mut index = 0;
                        while index < midi_outputs.len() {
                            if ports
                                .iter()
                                .find(|p| **p == midi_outputs[index].port_id)
                                .is_none()
                            {
                                // Closing a port whose device is already
                                // gone fails, and the point of the call is
                                // that we are done with it either way.
                                let out = &midi_outputs[index];
                                let _ = out.midi_output.Close();
                                midi_outputs.remove(index);
                            } else {
                                index += 1;
                            }
                        }
                    }
                    WinRTMidiEvent::UseMidiInputs(ports) => {
                        // find all ports we want enabled
                        for port_id in &ports {
                            // check if the port is an input
                            if let Some(port) = ports_list
                                .iter_mut()
                                .find(|p| p.desc.port_id == *port_id && p.desc.port_type.is_input())
                            {
                                // check if we dont have it in our midi_inputs yet
                                if midi_inputs.iter().find(|v| v.port_id == *port_id).is_some() {
                                    continue;
                                }
                                // open this input
                                if let Ok(midi_input) =
                                    executor::block_on(Self::create_midi_in_port(&port.winrt_id))
                                {
                                    let input_senders =
                                        midi_access_clone.lock().unwrap().input_senders.clone();
                                    let port_id = *port_id;
                                    let event_token = midi_input
                                        .MessageReceived(&TypedEventHandler::<
                                            MidiInPort,
                                            MidiMessageReceivedEventArgs,
                                        >::new(
                                            move |_, msg| {
                                                let msg = msg.as_ref().unwrap().Message().unwrap();
                                                let raw_data = msg.RawData().unwrap();
                                                let data_reader =
                                                    DataReader::FromBuffer(&raw_data).unwrap();
                                                let mut data = [0u8; 3];
                                                if data_reader.ReadBytes(&mut data).is_ok() {
                                                    let mut senders = input_senders.lock().unwrap();
                                                    senders.retain(|s| {
                                                        s.send((port_id, MidiData { data })).is_ok()
                                                    });
                                                    if senders.len() > 0 {
                                                        // make sure our eventloop runs
                                                        SignalToUI::set_ui_signal();
                                                    }
                                                }
                                                Ok(())
                                            },
                                        ))
                                        .unwrap();
                                    midi_inputs.push(WinRTMidiInput {
                                        event_token,
                                        port_id,
                                        midi_input,
                                    });
                                } else {
                                    crate::log!(
                                        "Midi input could not be created {}",
                                        port.desc.name
                                    );
                                }
                            }
                        }
                        let mut index = 0;
                        while index < midi_inputs.len() {
                            if ports
                                .iter()
                                .find(|p| **p == midi_inputs[index].port_id)
                                .is_none()
                            {
                                let inp = &midi_inputs[index];
                                let _ = inp.midi_input.RemoveMessageReceived(inp.event_token);
                                let _ = inp.midi_input.Close();
                                midi_inputs.remove(index);
                            } else {
                                index += 1;
                            }
                        }
                    }
                    WinRTMidiEvent::SendMidi(port_id, midi_data) => {
                        // Nothing in here may panic. A surface unplugged
                        // mid-set fails its next write, and the app writes
                        // to it twenty times a second, so the window is
                        // always open: this thread dying takes the app's
                        // control calls with it, since those send down a
                        // channel whose far end has just gone. The port is
                        // dropped instead, and the removal that follows
                        // tidies it up.
                        let Ok(writer) = DataWriter::new() else { continue };
                        if writer.WriteBytes(midi_data.wire()).is_err() {
                            continue;
                        }
                        let Ok(buffer) = writer.DetachBuffer() else { continue };
                        let mut lost = Vec::new();
                        for output in &mut midi_outputs {
                            if port_id.is_some() && Some(output.port_id) != port_id {
                                continue;
                            }
                            if output.midi_output.SendBuffer(&buffer).is_err() {
                                lost.push(output.port_id);
                            }
                        }
                        if !lost.is_empty() {
                            crate::log!("midi: {} port(s) stopped taking writes", lost.len());
                            midi_outputs.retain(|out| !lost.contains(&out.port_id));
                        }
                    }
                }
            }
            let _ = input_watcher.Stop();
            let _ = output_watcher.Stop();
        });

        //output_watcher.Start().unwrap();
        // alrighty lets initialize midi.
        change_signal.set();
        midi_access
    }

    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(OsMidiInput(recv)))
    }

    pub fn midi_reset(&self) {
        self.event_sender
            .send(WinRTMidiEvent::UseMidiOutputs(vec![]))
            .unwrap();
        self.event_sender
            .send(WinRTMidiEvent::UseMidiInputs(vec![]))
            .unwrap();
        self.event_sender
            .send(WinRTMidiEvent::UpdateDevices)
            .unwrap();
    }

    // These two run on the UI thread. If the MIDI thread is gone there is
    // nobody to tell, and telling nobody is not a reason to take the app
    // down in front of an audience -- which is exactly what it did, one
    // port event after the surface was unplugged.
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        let _ = self.event_sender.send(WinRTMidiEvent::UseMidiOutputs(ports.to_vec()));
    }

    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        let _ = self.event_sender.send(WinRTMidiEvent::UseMidiInputs(ports.to_vec()));
    }

    pub fn get_updated_descs(&self) -> Vec<MidiPortDesc> {
        self.descs.clone()
    }
}
