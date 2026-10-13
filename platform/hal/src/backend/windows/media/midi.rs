//! WinRT MIDI (Windows.Devices.Midi): the port list kept current by device watchers, input ports
//! whose messages land in the session's inbox, output ports the app's sends go to.
//!
//! Moved from os/windows/winrt_midi.rs. Every WinRT call runs on one long-lived MTA worker
//! (`makepad-winrt-midi`); the HAL's thread and the COM callbacks only queue events for it and
//! never wait. Async operations (listing, opening ports) complete through generated completion
//! handlers that queue a `Completed` event; message callbacks fill the inbox and ring the session.

use super::Signal;
use crate::media::{name_hash, MidiPortInfo};
use makepad_windows_sys::core::{HRESULT, HSTRING};
use makepad_windows_sys::WinRT::*;
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the worker is asked to do. Payloads stay queued until the worker takes them; the wake
/// channel is bounded and coalescing, so a post never blocks and a full wake slot loses nothing.
enum Event {
    Refresh,
    Completed(u64),
    Inputs(Vec<u64>),
    Outputs(Vec<u64>),
    /// a channel message (`status << 16 | d1 << 8 | d2`) to one port, or every open output (0)
    Send(u64, u32),
    Shutdown,
}

struct Events {
    pending: Mutex<VecDeque<Event>>,
    wake: SyncSender<()>,
}

impl Events {
    fn post(&self, event: Event) {
        lock(&self.pending).push_back(event);
        let _ = self.wake.try_send(());
    }
}

/// What the worker shares with the HAL's thread.
pub struct Shared {
    ports: Mutex<Vec<MidiPortInfo>>,
    /// messages of connected inputs, taken by the session's step
    pub inbox: Mutex<Vec<(u64, u32)>>,
    change: Signal,
    data: Signal,
}

/// The session's MIDI client.
pub struct WinRtMidiAccess {
    events: Arc<Events>,
    pub shared: Arc<Shared>,
}

impl Drop for WinRtMidiAccess {
    fn drop(&mut self) {
        self.events.post(Event::Shutdown);
    }
}

impl WinRtMidiAccess {
    /// Starts the worker; the first port list follows (rings `change`).
    pub fn new(change: Signal, data: Signal) -> WinRtMidiAccess {
        let (wake, receiver) = sync_channel(1);
        let events = Arc::new(Events { pending: Mutex::new(VecDeque::new()), wake });
        let shared = Arc::new(Shared { ports: Mutex::new(Vec::new()), inbox: Mutex::new(Vec::new()), change, data });
        let (worker_events, worker_shared) = (events.clone(), shared.clone());
        let spawned = std::thread::Builder::new().name("makepad-winrt-midi".into()).spawn(move || {
            use makepad_windows_sys::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_TYPE};
            // RO_INIT_MULTITHREADED
            if let Err(e) = unsafe { RoInitialize(RO_INIT_TYPE(1)) } {
                eprintln!("[hal-media] midi: WinRT initialization failed: {e}");
                // the app still gets its (empty) list
                worker_shared.change.set();
                return;
            }
            {
                let mut worker = Worker {
                    events: worker_events.clone(),
                    shared: worker_shared.clone(),
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
                    eprintln!("[hal-media] midi: device watcher failed: {e}");
                }
                worker_events.post(Event::Refresh);
                'worker: while receiver.recv().is_ok() {
                    loop {
                        let next = lock(&worker_events.pending).pop_front();
                        let Some(event) = next else { break };
                        if let Event::Shutdown = event {
                            break 'worker;
                        }
                        if let Err(e) = unsafe { worker.handle(event) } {
                            eprintln!("[hal-media] midi: Windows MIDI operation failed: {e}");
                        }
                    }
                }
            }
            unsafe { RoUninitialize() };
        });
        if let Err(e) = spawned {
            eprintln!("[hal-media] midi: worker thread: {e}");
        }
        WinRtMidiAccess { events, shared }
    }

    pub fn ports(&self) -> Vec<MidiPortInfo> {
        lock(&self.shared.ports).clone()
    }

    pub fn use_inputs(&self, ports: &[u64]) {
        self.events.post(Event::Inputs(ports.to_vec()));
    }

    pub fn use_outputs(&self, ports: &[u64]) {
        self.events.post(Event::Outputs(ports.to_vec()));
    }

    /// Every input and output closed, the ports listed again.
    pub fn reset(&self) {
        self.events.post(Event::Outputs(Vec::new()));
        self.events.post(Event::Inputs(Vec::new()));
        self.events.post(Event::Refresh);
    }

    pub fn send(&self, port: u64, data: u32) {
        self.events.post(Event::Send(port, data));
    }
}

/// The bytes a channel or system message occupies on the wire (status first): program change
/// and channel pressure carry one data byte, quarter-frame and song select one, song position
/// two, the other system messages none.
pub fn wire_len(status: u8) -> usize {
    match status {
        0xc0..=0xdf => 2,
        0x80..=0xbf | 0xe0..=0xef => 3,
        0xf1 | 0xf3 => 2,
        0xf2 => 3,
        _ => 1,
    }
}

struct Port {
    /// the WinRT device id
    device: String,
    info: MidiPortInfo,
}

struct Input {
    id: u64,
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
    id: u64,
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
    Input(u64, u64, OpenInputOperation),
    Output(u64, u64, OpenOutputOperation),
}

impl Pending {
    fn serial(&self) -> u64 {
        match self {
            Self::List(s, _, _) | Self::Input(s, _, _) | Self::Output(s, _, _) => *s,
        }
    }
}

/// Completion of an async operation: queues `Completed(serial)` for the worker.
struct Completion {
    events: Arc<Events>,
    serial: u64,
}

impl ListDevicesCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&ListDevicesOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.serial));
        Ok(())
    }
}

impl OpenInputCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&OpenInputOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.serial));
        Ok(())
    }
}

impl OpenOutputCompletedImpl for Completion {
    fn Invoke(&self, _: Option<&OpenOutputOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        self.events.post(Event::Completed(self.serial));
        Ok(())
    }
}

/// The device watchers: any change lists the ports again.
struct Watch {
    events: Arc<Events>,
}

impl DeviceAddedImpl for Watch {
    fn Invoke(&self, _: Option<&DeviceWatcher>, _: Option<&DeviceInformation>) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}

impl DeviceUpdatedImpl for Watch {
    fn Invoke(&self, _: Option<&DeviceWatcher>, _: Option<&DeviceInformationUpdate>) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}

impl DeviceEnumerationCompletedImpl for Watch {
    fn Invoke(&self, _: Option<&DeviceWatcher>, _: Option<&makepad_windows_sys::Win32::System::WinRT::IInspectable>) -> Result<(), HRESULT> {
        self.events.post(Event::Refresh);
        Ok(())
    }
}

/// Activation factories are agile: the one looked up when the port opens serves every message,
/// on whichever thread WinRT delivers it.
struct ReaderFactory(DataReaderStatics);
unsafe impl Send for ReaderFactory {}
unsafe impl Sync for ReaderFactory {}

/// An input port's message handler: the message goes to the inbox, the session is rung.
struct Receive {
    shared: Arc<Shared>,
    id: u64,
    readers: ReaderFactory,
}

impl MidiMessageReceivedImpl for Receive {
    fn Invoke(&self, _: Option<&MidiInPort>, args: Option<&MidiMessageReceivedEventArgs>) -> Result<(), HRESULT> {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let data = args.get_Message()?.get_RawData()?;
            let reader = self.readers.0.FromBuffer(&data)?;
            let mut bytes = [0u8; 3];
            // channel messages can be shorter than three bytes; system exclusive is not a channel
            // message and its first three bytes are all a 3-byte slot can carry
            let length = (reader.get_UnconsumedBufferLength()? as usize).min(bytes.len());
            if length == 0 {
                return Ok(());
            }
            reader.ReadBytes(&mut bytes[..length])?;
            let word = (bytes[0] as u32) << 16 | (bytes[1] as u32) << 8 | bytes[2] as u32;
            lock(&self.shared.inbox).push((self.id, word));
            self.shared.data.set();
        }
        Ok(())
    }
}

struct Worker {
    events: Arc<Events>,
    shared: Arc<Shared>,
    ports: Vec<Port>,
    inputs: Vec<Input>,
    outputs: Vec<Output>,
    wanted_inputs: Vec<u64>,
    wanted_outputs: Vec<u64>,
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
        self.inputs.clear();
        self.outputs.clear();
    }
}

const IN_PORT: &str = "Windows.Devices.Midi.MidiInPort";
const OUT_PORT: &str = "Windows.Devices.Midi.MidiOutPort";
const DEVICE_INFORMATION: &str = "Windows.Devices.Enumeration.DeviceInformation";

impl Worker {
    fn port_name(&self, id: u64) -> &str {
        self.ports.iter().find(|p| p.info.id == id).map_or("(unlisted port)", |p| p.info.name.as_str())
    }

    fn completion(&mut self) -> Completion {
        self.serial += 1;
        Completion { events: self.events.clone(), serial: self.serial }
    }

    unsafe fn watch(&mut self) -> Result<(), HRESULT> {
        let information = DeviceInformationStatics::factory(DEVICE_INFORMATION)?;
        let input = MidiInPortStatics::factory(IN_PORT)?.GetDeviceSelector()?;
        let output = MidiOutPortStatics::factory(OUT_PORT)?.GetDeviceSelector()?;
        for query in [input, output] {
            let w = information.CreateWatcherAqsFilter(&query)?;
            w.add_Added(&DeviceAdded::implement(Box::new(Watch { events: self.events.clone() })))?;
            w.add_Removed(&DeviceUpdated::implement(Box::new(Watch { events: self.events.clone() })))?;
            w.add_Updated(&DeviceUpdated::implement(Box::new(Watch { events: self.events.clone() })))?;
            w.add_EnumerationCompleted(&DeviceEnumerationCompleted::implement(Box::new(Watch { events: self.events.clone() })))?;
            w.Start()?;
            self.watchers.push(w);
        }
        Ok(())
    }

    unsafe fn list(&mut self, input: bool) -> Result<(), HRESULT> {
        let query = if input { MidiInPortStatics::factory(IN_PORT)?.GetDeviceSelector()? } else { MidiOutPortStatics::factory(OUT_PORT)?.GetDeviceSelector()? };
        let operation = DeviceInformationStatics::factory(DEVICE_INFORMATION)?.FindAllAsyncAqsFilter(&query)?;
        let callback = self.completion();
        let serial = callback.serial;
        operation.put_Completed(&ListDevicesCompleted::implement(Box::new(callback)))?;
        self.pending.push(Pending::List(serial, input, operation));
        Ok(())
    }

    unsafe fn open_ports(&mut self) -> Result<(), HRESULT> {
        let (wanted_inputs, wanted_outputs) = (self.wanted_inputs.clone(), self.wanted_outputs.clone());
        self.inputs.retain(|p| wanted_inputs.contains(&p.id));
        self.outputs.retain(|p| wanted_outputs.contains(&p.id));
        for index in 0..self.ports.len() {
            let (id, output, device) = (self.ports[index].info.id, self.ports[index].info.output, self.ports[index].device.clone());
            if !output {
                if !wanted_inputs.contains(&id) || self.inputs.iter().any(|p| p.id == id) || self.pending.iter().any(|p| matches!(p, Pending::Input(_, pid, _) if *pid == id)) {
                    continue;
                }
                let operation = MidiInPortStatics::factory(IN_PORT)?.FromIdAsync(&HSTRING::from_str(&device)?)?;
                let callback = self.completion();
                let serial = callback.serial;
                operation.put_Completed(&OpenInputCompleted::implement(Box::new(callback)))?;
                self.pending.push(Pending::Input(serial, id, operation));
            } else {
                if !wanted_outputs.contains(&id) || self.outputs.iter().any(|p| p.id == id) || self.pending.iter().any(|p| matches!(p, Pending::Output(_, pid, _) if *pid == id)) {
                    continue;
                }
                let operation = MidiOutPortStatics::factory(OUT_PORT)?.FromIdAsync(&HSTRING::from_str(&device)?)?;
                let callback = self.completion();
                let serial = callback.serial;
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
            Event::Completed(serial) => {
                let Some(index) = self.pending.iter().position(|p| p.serial() == serial) else { return Ok(()) };
                match self.pending.remove(index) {
                    Pending::List(_, input, operation) => {
                        // a device that disappears during enumeration keeps the published list
                        let result = operation.GetResults();
                        if let Ok(collection) = &result {
                            for i in 0..collection.get_Size()? {
                                let item = collection.GetAt(i)?;
                                let device = item.get_Id()?.to_string();
                                let info = MidiPortInfo { id: name_hash(&device), output: !input, name: item.get_Name()?.to_string() };
                                self.listed.push(Port { device, info });
                            }
                            if input {
                                self.list(false)?;
                                return Ok(());
                            }
                            self.ports = std::mem::take(&mut self.listed);
                            *lock(&self.shared.ports) = self.ports.iter().map(|p| p.info.clone()).collect();
                            self.shared.change.set();
                            self.open_ports()?;
                        }
                        if self.refresh_again {
                            self.refresh_again = false;
                            self.events.post(Event::Refresh);
                        }
                        result?;
                    }
                    Pending::Input(_, id, operation) => {
                        // a device that cannot be opened completes with no port
                        let Ok(port) = operation.GetResults() else {
                            eprintln!("[hal-media] midi: input \"{}\" could not be opened", self.port_name(id));
                            return Ok(());
                        };
                        if self.wanted_inputs.contains(&id) {
                            let handler = Receive { shared: self.shared.clone(), id, readers: ReaderFactory(DataReaderStatics::factory("Windows.Storage.Streams.DataReader")?) };
                            let token = port.add_MessageReceived(&MidiMessageReceived::implement(Box::new(handler)))?;
                            self.inputs.push(Input { id, port, token });
                        } else if let Ok(c) = Closable::query(port.as_raw()) {
                            let _ = c.Close();
                        }
                    }
                    Pending::Output(_, id, operation) => {
                        let Ok(port) = operation.GetResults() else {
                            eprintln!("[hal-media] midi: output \"{}\" could not be opened", self.port_name(id));
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
            Event::Send(port, data) => {
                let bytes = [(data >> 16) as u8, (data >> 8) as u8, data as u8];
                let writer = DataWriter::activate("Windows.Storage.Streams.DataWriter")?;
                writer.WriteBytes(&bytes[..wire_len(bytes[0])])?;
                let buffer = writer.DetachBuffer()?;
                self.outputs.retain(|p| {
                    if port != 0 && port != p.id {
                        return true;
                    }
                    match p.port.SendBuffer(&buffer) {
                        Ok(()) => true,
                        Err(e) => {
                            eprintln!("[hal-media] midi: an output stopped taking writes: {e}");
                            false
                        }
                    }
                });
            }
            Event::Shutdown => {}
        }
        Ok(())
    }
}
