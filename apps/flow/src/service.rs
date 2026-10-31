//! The window's service thread: it owns the embedded Flow server, the
//! session connector and the test-pattern seam, from
//! bootstrap to their blocking stop and drop. The UI keeps a handle it
//! never waits on and takes no lock through: the service thread reads the
//! connector's status and client (that is where its locks are taken) and
//! publishes a snapshot whenever they change, through a bounded channel the
//! UI polls; the UI retains the last snapshot it saw. What reaches the UI
//! is the newest observed state, never an older one after a newer one. At
//! shutdown the UI sends one `Stop` (or just drops the handle, which the
//! thread reads the same way). Nothing owned here is ever dropped on the UI
//! thread.

use crate::assets::{self, Bootstrap};
use makepad_flow::client::{FlowClient, SessionStatus};
use makepad_widgets::makepad_platform::thread::SignalToUI;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the service re-reads the connector while parked. A status
/// change reaches the UI within this.
const WATCH_EVERY: Duration = Duration::from_millis(100);

pub enum ServiceEvent {
    Started { embedded: bool },
    /// The connector's status and client as the service last read them.
    Session { status: SessionStatus, client: Option<Arc<Mutex<FlowClient>>> },
    Failed(String),
}

enum Command {
    Stop,
}

pub struct ServiceHandle {
    commands: Option<SyncSender<Command>>,
    events: Receiver<ServiceEvent>,
}

impl ServiceHandle {
    /// The next thing the service said, if any. Never waits.
    pub fn poll(&mut self) -> Option<ServiceEvent> {
        self.events.try_recv().ok()
    }

    /// Ask the service to stop everything it owns, on its own thread. The
    /// command queue holds one entry, so a second stop is simply the
    /// handle going away, which the thread reads as the same request.
    pub fn stop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.try_send(Command::Stop);
        }
    }
}

/// Start the service thread; `media_max_bytes` is the machine-scaled media
/// admission budget the embedded server runs with.
pub fn start(media_max_bytes: usize) -> ServiceHandle {
    let (command_tx, command_rx) = sync_channel::<Command>(1);
    let (event_tx, event_rx) = sync_channel::<ServiceEvent>(8);
    let spawn_failed_tx = event_tx.clone();
    let spawned = std::thread::Builder::new().name("flow-service".into()).spawn(move || {
        let bootstrap = match assets::start(media_max_bytes) {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                let _ = event_tx.send(ServiceEvent::Failed(error));
                SignalToUI::set_ui_signal();
                return;
            }
        };
        if event_tx.send(ServiceEvent::Started { embedded: bootstrap.host.is_some() }).is_err() {
            shutdown(bootstrap);
            return;
        }
        SignalToUI::set_ui_signal();
        // Parked until told to stop or until the handle is gone; meanwhile
        // the connector is watched and every change published. What is
        // published is always the newest observed state: a snapshot the
        // full channel refused is not kept, the next read is sent instead,
        // and a state that came back to what was published sends nothing.
        // A client is identified by its allocation, not by presence, so a
        // reconnect that yields the same status still publishes.
        let mut published: Option<(SessionStatus, Option<usize>)> = None;
        loop {
            match command_rx.recv_timeout(WATCH_EVERY) {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let status = bootstrap.session.status();
            let client = bootstrap.session.client();
            let key = (status.clone(), client.as_ref().map(|client| Arc::as_ptr(client) as usize));
            if published.as_ref() == Some(&key) {
                continue;
            }
            match event_tx.try_send(ServiceEvent::Session { status, client }) {
                Ok(()) => {
                    published = Some(key);
                    SignalToUI::set_ui_signal();
                }
                // Refused for room: nothing is kept, the next read publishes
                // whatever the state is then, which is never older.
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        shutdown(bootstrap);
    });
    if let Err(error) = spawned {
        let _ = spawn_failed_tx.try_send(ServiceEvent::Failed(format!("cannot start the service thread: {error}")));
    }
    ServiceHandle { commands: Some(command_tx), events: event_rx }
}

/// Every blocking stop, here and nowhere else.
fn shutdown(mut bootstrap: Bootstrap) {
    bootstrap.session.stop();
    if let Some(host) = bootstrap.host.take() {
        host.shutdown();
    }
    drop(bootstrap.testpattern.take());
}
