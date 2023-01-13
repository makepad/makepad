use {
    std::sync::{Arc, Mutex},
    std::sync::mpsc,
    crate::{
        makepad_live_id::{live_id, LiveId},
        midi::*,
        cx::Cx,
        cx_api::CxOsApi,
        windows_crate::{
            Foundation::{
                EventRegistrationToken,
                TypedEventHandler,
            },
            Storage::Streams::{
                DataReader
            },
            Devices::Enumeration::{
                DeviceWatcher,
                DeviceInformation,
                DeviceInformationUpdate
            },
            Devices::Midi::{MidiInPort, MidiOutPort, MidiMessageReceivedEventArgs},
        }
    },
};


type WindowsResult<T> = crate::windows_crate::core::Result<T>;

#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<WinRTMidiAccess >>);

impl OsMidiOutput {
    pub fn send(&self, _port_id: Option<MidiPortId>, _d: MidiData) {
        // send to a specific port or all ports
        /*
        let mut win32_midi = self.0.lock().unwrap();
        let _short_msg = ((d.data2 as u32) << 16) | ((d.data1 as u32) << 8) | d.data0 as u32;
        for port in &mut win32_midi.ports {
            if port.desc.port_type.is_output()
                && (port_id.is_none() || port.desc.port_id == port_id.unwrap()) {
                /*if let Win32MidiHandle::OpenOut(hmidiout) = port.handle{
                    unsafe{
                        midiOutShortMsg(hmidiout, short_msg);
                    }
                }*/
            }
        }*/
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct WinRTMidiPort {
    winrt_id: String,
    desc: MidiPortDesc
}

#[derive(Clone)]
pub struct WinRTMidiInput {
    port_id: MidiPortId,
    event_token: EventRegistrationToken,
    midi_input: MidiInPort,
}

pub struct WinRTMidiAccess {
    input_senders: InputSenders,
    watch_sender: mpsc::Sender<WatchEvent>,
    descs: Vec<MidiPortDesc>,
}

#[derive(Clone)]
enum WatchEvent {
    UpdateDevices,
    Terminate,
    UseMidiInputs(Vec<MidiPortId>),
    UseMidiOutputs(Vec<MidiPortId>),
}

impl WinRTMidiAccess {
    
    async fn create_midi_port(winrt_id: &str) -> WindowsResult<MidiInPort> {
        let port = MidiInPort::FromIdAsync(&winrt_id.into()) ? .await ?;
        Ok(port)
    }
    
    
    async fn get_ports_list() -> WindowsResult<Vec<WinRTMidiPort >> {
        let input_query = MidiInPort::GetDeviceSelector().unwrap();
        let mut ports = Vec::new();
        let collection = DeviceInformation::FindAllAsyncAqsFilter(&input_query) ? .await ?;
        for item in collection {
            let winrt_id = item.Id().unwrap().to_string();
            ports.push(WinRTMidiPort {
                desc: MidiPortDesc {
                    name: item.Name().unwrap().to_string(),
                    port_id: LiveId::from_str_unchecked(&winrt_id).into(),
                    port_type: MidiPortType::Input,
                },
                winrt_id
            });
        }
        let output_query = MidiOutPort::GetDeviceSelector().unwrap();
        let collection = DeviceInformation::FindAllAsyncAqsFilter(&output_query) ? .await ?;
        for item in collection {
            let winrt_id = item.Id().unwrap().to_string();
            ports.push(WinRTMidiPort {
                desc: MidiPortDesc {
                    name: item.Name().unwrap().to_string(),
                    port_id: LiveId::from_str_unchecked(&winrt_id).into(),
                    port_type: MidiPortType::Output,
                },
                winrt_id
            });
        }
        Ok(ports)
    }
    
    pub fn new() -> Arc<Mutex<Self >> {
        
        let (watch_sender, watch_receiver) = mpsc::channel();
        let input_senders = InputSenders::default();
        let midi_access = Arc::new(Mutex::new(Self {
            descs: Vec::new(),
            watch_sender: watch_sender.clone(),
            input_senders,
        }));
        let midi_access_clone = midi_access.clone();
        
        std::thread::spawn(move || {
            
            let mut ports_list = Vec::new();
            
            let mut midi_inputs = Vec::new();
            
            // initiate device list update
            watch_sender.send(WatchEvent::UpdateDevices).unwrap();
            // now lets watch device changes
            let query = MidiInPort::GetDeviceSelector().unwrap();
            let input_watcher = DeviceInformation::CreateWatcherAqsFilter(&query).unwrap();
            let query = MidiInPort::GetDeviceSelector().unwrap();
            let output_watcher = DeviceInformation::CreateWatcherAqsFilter(&query).unwrap();
            
            fn bind_watcher(watch_sender: mpsc::Sender::<WatchEvent>, watcher: &DeviceWatcher) {
                let sender = watch_sender.clone();
                watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move | _, _ | {
                    let _ = sender.send(WatchEvent::UpdateDevices);
                    Ok(())
                })).unwrap();
                let sender = watch_sender.clone();
                watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move | _, _ | {
                    let _ = sender.send(WatchEvent::UpdateDevices);
                    Ok(())
                })).unwrap();
                let sender = watch_sender.clone();
                watcher.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move | _, _ | {
                    let _ = sender.send(WatchEvent::UpdateDevices);
                    Ok(())
                })).unwrap();
                let sender = watch_sender.clone();
                watcher.EnumerationCompleted(&TypedEventHandler::new(move | _, _ | {
                    let _ = sender.send(WatchEvent::UpdateDevices);
                    Ok(())
                })).unwrap();
            }
            
            bind_watcher(watch_sender.clone(), &input_watcher);
            bind_watcher(watch_sender.clone(), &output_watcher);
            input_watcher.Start().unwrap();
            output_watcher.Start().unwrap();
            
            while let Ok(msg) = watch_receiver.recv() {
                match msg {
                    WatchEvent::UpdateDevices => {
                        ports_list = futures::executor::block_on(Self::get_ports_list()).unwrap();
                        let mut descs = Vec::new();
                        for port in &ports_list {
                            descs.push(port.desc.clone());
                        }
                        midi_access_clone.lock().unwrap().descs = descs;
                        Cx::post_signal(live_id!(WinRTMidiPortsChanged).into());
                    }
                    WatchEvent::Terminate => {
                        break;
                    }
                    WatchEvent::UseMidiOutputs(ports) => {
                        //let cself = midi_access_clone.lock().unwrap();
                        // find all ports we want enabled
                        for port_id in &ports {
                            if let Some(_port) = ports_list.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_output()) {
                                // open this output
                            }
                        }
                        // and the ones disabled
                        for port in &mut ports_list {
                            if ports.iter().find( | p | **p == port.desc.port_id).is_none() && port.desc.port_type.is_output() {
                                // close this output
                            }
                        }
                    }
                    WatchEvent::UseMidiInputs(ports) => {
                        // find all ports we want enabled
                        for port_id in &ports {
                            if let Some(port) = ports_list.iter_mut().find( | p | p.desc.port_id == *port_id && p.desc.port_type.is_input()) {
                                // open this input
                                let midi_input = futures::executor::block_on(Self::create_midi_port(&port.winrt_id)).unwrap();
                                
                                let input_senders = midi_access_clone.lock().unwrap().input_senders.clone();
                                let port_id = *port_id;
                                let event_token = midi_input.MessageReceived(&TypedEventHandler::<MidiInPort, MidiMessageReceivedEventArgs>::new(move | _, msg | {
                                    let raw_data = msg.as_ref().unwrap().Message().unwrap().RawData().unwrap();
                                    let data_reader = DataReader::FromBuffer(&raw_data).unwrap();
                                    let mut data = [0u8;3];
                                    if data_reader.ReadBytes(&mut data).is_ok(){
                                        let mut senders = input_senders.lock().unwrap();
                                        senders.retain( | s | {
                                            s.send((port_id, MidiData {
                                                data,
                                            })).is_ok()
                                        });
                                    }
                                    Ok(())
                                })).unwrap();
                                midi_inputs.push(WinRTMidiInput{
                                    event_token,
                                    port_id,
                                    midi_input
                                });
                            }
                        }
                        // and the ones disabled
                        for port in &mut ports_list {
                            if ports.iter().find( | p | **p == port.desc.port_id).is_none() && port.desc.port_type.is_input() {
                                //close this input
                                if let Some(index) = midi_inputs.iter().position(|v| v.port_id == port.desc.port_id){
                                    let inp = &midi_inputs[index];
                                    inp.midi_input.RemoveMessageReceived(inp.event_token).unwrap();
                                    inp.midi_input.Close().unwrap();
                                    midi_inputs.remove(index);
                                }
                            }
                        }
                    }
                }
            }
            input_watcher.Stop().unwrap();
            output_watcher.Stop().unwrap();
        });
        
        
        //output_watcher.Start().unwrap();
        // alrighty lets initialize midi.
        Cx::post_signal(live_id!(WinRTMidiInputsChanged).into());
        midi_access
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.watch_sender.send(WatchEvent::UseMidiOutputs(ports.to_vec())).unwrap();
    }
    
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.watch_sender.send(WatchEvent::UseMidiInputs(ports.to_vec())).unwrap();
    }
    
    pub fn get_descs(&self) -> Vec<MidiPortDesc> {
        self.descs.clone()
    }
    
}
