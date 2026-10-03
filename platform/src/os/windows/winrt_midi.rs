use crate::makepad_network::mpsc;
use crate::windows::{
    core::{HRESULT, HSTRING},
    WinRT::*,
};
use crate::{makepad_live_id::LiveId, midi::*, thread::SignalToUI};
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{sync_channel, SyncSender},
        Arc, Mutex, Weak,
    },
};

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData)>>>>;
pub struct OsMidiInput(mpsc::Receiver<(MidiPortId, MidiData)>);
#[derive(Clone)]
pub struct OsMidiOutput(pub(crate) Arc<Mutex<WinRTMidiAccess>>);
impl OsMidiOutput {
    pub fn send(&self, port: Option<MidiPortId>, data: MidiData) {
        self.0.lock().unwrap().events.post(Event::Send(port, data));
    }
}
impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        self.0.try_recv().ok()
    }
}

// Payloads stay queued until the worker takes them. A bounded, coalescing wake
// channel never blocks the UI or a COM callback, and a full wake slot loses no work.
struct Events {
    pending: Mutex<VecDeque<Event>>,
    wake: SyncSender<()>,
}
impl Events {
    fn post(&self, event: Event) {
        self.pending.lock().unwrap().push_back(event);
        let _ = self.wake.try_send(());
    }
}
enum Event {
    Refresh,
    Completed(u64),
    Inputs(Vec<MidiPortId>),
    Outputs(Vec<MidiPortId>),
    Send(Option<MidiPortId>, MidiData),
    Shutdown,
}
struct Port {
    id: String,
    desc: MidiPortDesc,
}
struct Input {
    id: MidiPortId,
    port: MidiInPort,
    token: EventRegistrationToken,
}
impl Drop for Input {
    fn drop(&mut self) {
        unsafe {
            let _ = self.port.remove_MessageReceived(self.token);
            if let Ok(c) = Closable::query(self.port.as_raw()) {
                let _ = c.Close();
            }
        }
    }
}
struct Output {
    id: MidiPortId,
    port: MidiOutPort,
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            if let Ok(c) = Closable::query(self.port.as_raw()) {
                let _ = c.Close();
            }
        }
    }
}
enum Pending {
    List(u64, bool, ListDevicesOperation),
    Input(u64, MidiPortId, OpenInputOperation),
    Output(u64, MidiPortId, OpenOutputOperation),
}
impl Pending {
    fn id(&self) -> u64 {
        match self {
            Self::List(id, _, _) | Self::Input(id, _, _) | Self::Output(id, _, _) => *id,
        }
    }
}
struct Completion {
    events: Arc<Events>,
    id: u64,
}
impl ListDevicesCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&ListDevicesOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.id));
        Ok(())
    }
}
impl OpenInputCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&OpenInputOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.id));
        Ok(())
    }
}
impl OpenOutputCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&OpenOutputOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.id));
        Ok(())
    }
}
struct Watch {
    events: Arc<Events>,
}
impl DeviceAddedImpl for Watch {
    fn Invoke(
        &self,
        _: Option<&DeviceWatcher>,
        _: Option<&DeviceInformation>,
    ) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}
impl DeviceUpdatedImpl for Watch {
    fn Invoke(
        &self,
        _: Option<&DeviceWatcher>,
        _: Option<&DeviceInformationUpdate>,
    ) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}
impl DeviceEnumerationCompletedImpl for Watch {
    fn Invoke(
        &self,
        _: Option<&DeviceWatcher>,
        _: Option<&crate::windows::Win32::System::WinRT::IInspectable>,
    ) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}
// Activation factories are agile: the one looked up when the port opens
// serves every message, on whichever thread WinRT delivers it.
struct ReaderFactory(DataReaderStatics);
unsafe impl Send for ReaderFactory {}
unsafe impl Sync for ReaderFactory {}
struct Receive {
    senders: InputSenders,
    id: MidiPortId,
    readers: ReaderFactory,
}
impl MidiMessageReceivedImpl for Receive {
    fn Invoke(
        &self,
        _: Option<&MidiInPort>,
        args: Option<&MidiMessageReceivedEventArgs>,
    ) -> Result<(), HRESULT> {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let data = args.get_Message()?.get_RawData()?;
            let reader = self.readers.0.FromBuffer(&data)?;
            let mut bytes = [0u8; 3];
            // MIDI channel messages can be shorter than three bytes.
            let length = (reader.get_UnconsumedBufferLength()? as usize).min(bytes.len());
            if length == 0 {
                return Ok(());
            }
            reader.ReadBytes(&mut bytes[..length])?;
            let mut senders = self.senders.lock().unwrap();
            senders.retain(|s| s.send((self.id, MidiData { data: bytes })).is_ok());
            if !senders.is_empty() {
                SignalToUI::set_ui_signal();
            }
        }
        Ok(())
    }
}

pub struct WinRTMidiAccess {
    input_senders: InputSenders,
    events: Arc<Events>,
    descs: Vec<MidiPortDesc>,
}
impl Drop for WinRTMidiAccess {
    fn drop(&mut self) {
        self.events.post(Event::Shutdown);
    }
}
impl WinRTMidiAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        let (wake, receiver) = sync_channel(1);
        let events = Arc::new(Events {
            pending: Mutex::new(VecDeque::new()),
            wake,
        });
        let input_senders = InputSenders::default();
        let access = Arc::new(Mutex::new(Self {
            input_senders: input_senders.clone(),
            events: events.clone(),
            descs: Vec::new(),
        }));
        let owner = Arc::downgrade(&access);
        std::thread::spawn(move || {
            use crate::windows::Win32::System::WinRT::{
                RoInitialize, RoUninitialize, RO_INIT_TYPE,
            };
            if let Err(e) = unsafe { RoInitialize(RO_INIT_TYPE(1)) } {
                crate::log!("midi: WinRT initialization failed: {e:?}");
                return;
            }
            {
                let mut worker = Worker {
                    events: events.clone(),
                    owner,
                    signal: change_signal,
                    input_senders,
                    ports: Vec::new(),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    wanted_inputs: Vec::new(),
                    wanted_outputs: Vec::new(),
                    pending: Vec::new(),
                    serial: 0,
                    refresh_again: false,
                    listed: Vec::new(),
                    watchers: Vec::new(),
                };
                if let Err(e) = unsafe { worker.watch() } {
                    crate::log!("midi: device watcher failed: {e:?}");
                }
                events.post(Event::Refresh);
                'worker: while receiver.recv().is_ok() {
                    loop {
                        let next = events.pending.lock().unwrap().pop_front();
                        let Some(event) = next else {
                            break;
                        };
                        if let Event::Shutdown = event {
                            break 'worker;
                        }
                        if let Err(e) = unsafe { worker.handle(event) } {
                            crate::log!("midi: Windows MIDI operation failed: {e:?}");
                        }
                    }
                }
            }
            unsafe {
                RoUninitialize();
            }
        });
        access
    }
    pub fn create_midi_input(&self) -> MidiInput {
        let (send, recv) = mpsc::channel();
        self.input_senders.lock().unwrap().push(send);
        MidiInput(Some(OsMidiInput(recv)))
    }
    pub fn midi_reset(&self) {
        self.events.post(Event::Outputs(Vec::new()));
        self.events.post(Event::Inputs(Vec::new()));
        self.events.post(Event::Refresh);
    }
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.events.post(Event::Outputs(ports.to_vec()));
    }
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.events.post(Event::Inputs(ports.to_vec()));
    }
    pub fn get_updated_descs(&self) -> Vec<MidiPortDesc> {
        self.descs.clone()
    }
}
struct Worker {
    events: Arc<Events>,
    owner: Weak<Mutex<WinRTMidiAccess>>,
    signal: SignalToUI,
    input_senders: InputSenders,
    ports: Vec<Port>,
    inputs: Vec<Input>,
    outputs: Vec<Output>,
    wanted_inputs: Vec<MidiPortId>,
    wanted_outputs: Vec<MidiPortId>,
    pending: Vec<Pending>,
    serial: u64,
    refresh_again: bool,
    listed: Vec<Port>,
    watchers: Vec<DeviceWatcher>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        unsafe {
            for w in &self.watchers {
                let _ = w.Stop();
            }
            for p in &self.pending {
                let raw = match p {
                    Pending::List(_, _, o) => o.as_raw(),
                    Pending::Input(_, _, o) => o.as_raw(),
                    Pending::Output(_, _, o) => o.as_raw(),
                };
                if let Ok(info) = AsyncInfo::query(raw) {
                    let _ = info.Cancel();
                }
            }
        }
    }
}
impl Worker {
    fn port_name(&self, id: MidiPortId) -> &str {
        match self.ports.iter().find(|p| p.desc.port_id == id) {
            Some(port) => &port.desc.name,
            None => "(unlisted port)",
        }
    }
    fn completion(&mut self) -> Completion {
        self.serial += 1;
        Completion {
            events: self.events.clone(),
            id: self.serial,
        }
    }
    unsafe fn watch(&mut self) -> Result<(), HRESULT> {
        let information =
            DeviceInformationStatics::factory("Windows.Devices.Enumeration.DeviceInformation")?;
        let input =
            MidiInPortStatics::factory("Windows.Devices.Midi.MidiInPort")?.GetDeviceSelector()?;
        let output =
            MidiOutPortStatics::factory("Windows.Devices.Midi.MidiOutPort")?.GetDeviceSelector()?;
        for query in [input, output] {
            let w = information.CreateWatcherAqsFilter(&query)?;
            w.add_Added(&DeviceAdded::implement(Box::new(Watch {
                events: self.events.clone(),
            })))?;
            w.add_Removed(&DeviceUpdated::implement(Box::new(Watch {
                events: self.events.clone(),
            })))?;
            w.add_Updated(&DeviceUpdated::implement(Box::new(Watch {
                events: self.events.clone(),
            })))?;
            w.add_EnumerationCompleted(&DeviceEnumerationCompleted::implement(Box::new(Watch {
                events: self.events.clone(),
            })))?;
            w.Start()?;
            self.watchers.push(w);
        }
        Ok(())
    }
    unsafe fn list(&mut self, input: bool) -> Result<(), HRESULT> {
        let query = if input {
            MidiInPortStatics::factory("Windows.Devices.Midi.MidiInPort")?.GetDeviceSelector()?
        } else {
            MidiOutPortStatics::factory("Windows.Devices.Midi.MidiOutPort")?.GetDeviceSelector()?
        };
        let operation =
            DeviceInformationStatics::factory("Windows.Devices.Enumeration.DeviceInformation")?
                .FindAllAsyncAqsFilter(&query)?;
        let callback = self.completion();
        let id = callback.id;
        operation.put_Completed(&ListDevicesCompleted::implement(Box::new(callback)))?;
        self.pending.push(Pending::List(id, input, operation));
        Ok(())
    }
    unsafe fn open_ports(&mut self) -> Result<(), HRESULT> {
        self.inputs.retain(|p| self.wanted_inputs.contains(&p.id));
        self.outputs.retain(|p| self.wanted_outputs.contains(&p.id));
        for index in 0..self.ports.len() {
            let port = &self.ports[index];
            let id = port.desc.port_id;
            let input = port.desc.port_type.is_input();
            if input {
                if !self.wanted_inputs.contains(&id)
                    || self.inputs.iter().any(|p| p.id == id)
                    || self
                        .pending
                        .iter()
                        .any(|p| matches!(p,Pending::Input(_,pid,_) if *pid==id))
                {
                    continue;
                }
                let name = HSTRING::from_str(&port.id)?;
                let operation = MidiInPortStatics::factory("Windows.Devices.Midi.MidiInPort")?
                    .FromIdAsync(&name)?;
                let callback = self.completion();
                let serial = callback.id;
                operation.put_Completed(&OpenInputCompleted::implement(Box::new(callback)))?;
                self.pending.push(Pending::Input(serial, id, operation));
            } else {
                if !self.wanted_outputs.contains(&id)
                    || self.outputs.iter().any(|p| p.id == id)
                    || self
                        .pending
                        .iter()
                        .any(|p| matches!(p,Pending::Output(_,pid,_) if *pid==id))
                {
                    continue;
                }
                let name = HSTRING::from_str(&port.id)?;
                let operation = MidiOutPortStatics::factory("Windows.Devices.Midi.MidiOutPort")?
                    .FromIdAsync(&name)?;
                let callback = self.completion();
                let serial = callback.id;
                operation.put_Completed(&OpenOutputCompleted::implement(Box::new(callback)))?;
                self.pending.push(Pending::Output(serial, id, operation));
            }
        }
        Ok(())
    }
    unsafe fn handle(&mut self, event: Event) -> Result<(), HRESULT> {
        match event {
            Event::Refresh => {
                if self.pending.iter().any(|p| matches!(p, Pending::List(..))) {
                    self.refresh_again = true;
                } else {
                    self.listed.clear();
                    self.list(true)?;
                }
            }
            Event::Completed(id) => {
                let Some(index) = self.pending.iter().position(|p| p.id() == id) else {
                    return Ok(());
                };
                match self.pending.remove(index) {
                    Pending::List(_, input, operation) => {
                        // Preserve the published list if a device disappears during enumeration.
                        let result = operation.GetResults();
                        if let Ok(collection) = &result {
                            for i in 0..collection.get_Size()? {
                                let item = collection.GetAt(i)?;
                                let id = item.get_Id()?.to_string();
                                self.listed.push(Port {
                                    desc: MidiPortDesc {
                                        port_id: LiveId::from_str(&id).into(),
                                        name: item.get_Name()?.to_string(),
                                        port_type: if input {
                                            MidiPortType::Input
                                        } else {
                                            MidiPortType::Output
                                        },
                                    },
                                    id,
                                });
                            }
                            if input {
                                self.list(false)?;
                                return Ok(());
                            }
                            self.ports = std::mem::take(&mut self.listed);
                            let descs = self.ports.iter().map(|p| p.desc.clone()).collect();
                            if let Some(owner) = self.owner.upgrade() {
                                owner.lock().unwrap().descs = descs;
                            }
                            self.signal.set();
                            self.open_ports()?;
                        }
                        if self.refresh_again {
                            self.refresh_again = false;
                            self.events.post(Event::Refresh);
                        }
                        result?;
                    }
                    Pending::Input(_, id, operation) => {
                        // A device that cannot be opened completes with no port.
                        let Ok(port) = operation.GetResults() else {
                            crate::log!("Midi input could not be created {}", self.port_name(id));
                            return Ok(());
                        };
                        if self.wanted_inputs.contains(&id) {
                            let token = port.add_MessageReceived(
                                &MidiMessageReceived::implement(Box::new(Receive {
                                    senders: self.input_senders.clone(),
                                    id,
                                    readers: ReaderFactory(DataReaderStatics::factory(
                                        "Windows.Storage.Streams.DataReader",
                                    )?),
                                })),
                            )?;
                            self.inputs.push(Input { id, port, token });
                        } else if let Ok(c) = Closable::query(port.as_raw()) {
                            let _ = c.Close();
                        }
                    }
                    Pending::Output(_, id, operation) => {
                        let Ok(port) = operation.GetResults() else {
                            crate::log!("Midi output could not be created {}", self.port_name(id));
                            return Ok(());
                        };
                        if self.wanted_outputs.contains(&id) {
                            self.outputs.push(Output { id, port });
                        } else if let Ok(c) = Closable::query(port.as_raw()) {
                            let _ = c.Close();
                        }
                    }
                }
            }
            Event::Inputs(ids) => {
                self.wanted_inputs = ids;
                self.open_ports()?;
            }
            Event::Outputs(ids) => {
                self.wanted_outputs = ids;
                self.open_ports()?;
            }
            Event::Send(id, data) => {
                let writer = DataWriter::activate("Windows.Storage.Streams.DataWriter")?;
                writer.WriteBytes(data.wire())?;
                let buffer = writer.DetachBuffer()?;
                self.outputs.retain(|p| {
                    if id.is_some() && id != Some(p.id) {
                        return true;
                    }
                    if let Err(e) = p.port.SendBuffer(&buffer) {
                        crate::log!("midi: port stopped taking writes: {e:?}");
                        false
                    } else {
                        true
                    }
                });
            }
            Event::Shutdown => {}
        }
        Ok(())
    }
}
