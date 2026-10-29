//! One AT-SPI connection owner for all Linux shells, including headless logic runs.
//! Bus I/O, marshalling and tree diffing belong to this worker, never the HAL/UI thread.
mod check;
mod events;
mod objects;
mod text;

use super::io::{PollFd, READ, WRITE};
use crate::a11y;
use makepad_a11y::{consumer, NodeId, Notification};
use makepad_atspi::*;
use makepad_dbus_wire::{Body, Connection, Message, MessageType, Value};
use objects::Objects;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const ROOT: &str = "/org/a11y/atspi/accessible/root";
const PREFIX: &str = "/org/a11y/atspi/accessible";
const CACHE: &str = "/org/a11y/atspi/cache";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const BUS: &str = "org.a11y.Bus";
const BUS_PATH: &str = "/org/a11y/bus";
const TIMEOUT: Duration = Duration::from_secs(5);
// A consumer grace period, not a limit on pending operations or tree size.
const IDLE: Duration = Duration::from_secs(30);

enum Notice {
    Update(u64, Vec<Notification>),
    SessionClosed(u32),
    Geometry(u64, Option<(f32, f32)>),
    Focus(u64, bool),
}
struct Queue {
    send: SyncSender<Notice>,
    // Preserve order and all events when the bounded channel fills. The worker takes
    // the overflow by swapping the deque, so its lock never covers tree work or I/O.
    overflow: VecDeque<Notice>,
}
struct Mailbox {
    queue: Mutex<Queue>,
    wake: UnixStream,
}
static WORKER: OnceLock<Option<Arc<Mailbox>>> = OnceLock::new();
#[derive(Default)]
struct Identity {
    connection: Option<(String, String)>,
    last_request: Option<(u64, u64, NodeId)>,
    completed: HashMap<u64, u8>,
}
fn identity() -> &'static Mutex<Identity> {
    static IDENTITY: OnceLock<Mutex<Identity>> = OnceLock::new();
    IDENTITY.get_or_init(Default::default)
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn start() {
    WORKER.get_or_init(|| {
        a11y::set_os_selfcheck(check::selfcheck);
        let result = (|| -> std::io::Result<Arc<Mailbox>> {
            let (read, write) = UnixStream::pair()?;
            read.set_nonblocking(true)?;
            write.set_nonblocking(true)?;
            let (send, recv) = mpsc::sync_channel(1);
            let mailbox = Arc::new(Mailbox {
                queue: Mutex::new(Queue {
                    send,
                    overflow: VecDeque::new(),
                }),
                wake: write,
            });
            let worker = mailbox.clone();
            std::thread::Builder::new()
                .name("atspi".into())
                .spawn(move || run(worker, recv, read))?;
            Ok(mailbox)
        })();
        match result {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("[atspi] idle: {e}");
                None
            }
        }
    });
}
fn post(notice: Notice) {
    let Some(Some(mailbox)) = WORKER.get() else {
        return;
    };
    {
        let mut q = lock(&mailbox.queue);
        if !q.overflow.is_empty() {
            q.overflow.push_back(notice);
        } else if let Err(error) = q.send.try_send(notice) {
            match error {
                TrySendError::Full(notice) => {
                    static SAID: std::sync::Once = std::sync::Once::new();
                    SAID.call_once(|| {
                        eprintln!(
                            "[atspi] notification channel full; retaining overflow for worker"
                        )
                    });
                    q.overflow.push_back(notice);
                }
                TrySendError::Disconnected(_) => eprintln!("[atspi] worker disconnected"),
            }
        }
    }
    match (&mailbox.wake).write(&[1]) {
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
        Err(e) => eprintln!("[atspi] wake: {e}"),
    }
}
pub(crate) fn notify(key: u64, notifications: &[Notification]) {
    post(Notice::Update(key, notifications.to_vec()));
}
pub(crate) fn session_closed(session: u32) {
    post(Notice::SessionClosed(session));
}
pub(crate) fn geometry(key: u64, origin: Option<(f32, f32)>) {
    post(Notice::Geometry(key, origin));
}
pub(crate) fn focus(key: u64, focused: bool) {
    post(Notice::Focus(key, focused));
}
fn drain(mailbox: &Mailbox, recv: &Receiver<Notice>, read: &UnixStream) -> VecDeque<Notice> {
    let mut bytes = [0; 256];
    loop {
        match (&*read).read(&mut bytes) {
            Ok(0) => break,
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let mut q = lock(&mailbox.queue);
    let first = recv.try_recv().ok();
    let mut notices = std::mem::take(&mut q.overflow);
    drop(q);
    if let Some(first) = first {
        notices.push_front(first);
    }
    notices
}
fn run(mailbox: Arc<Mailbox>, recv: Receiver<Notice>, read: UnixStream) {
    let mut said = false;
    let shared = Arc::new(Mutex::new(objects::Shared::default()));
    loop {
        let result = connected(&mailbox, &recv, &read, shared.clone());
        lock(identity()).connection = None;
        a11y::request_consumer(consumer::OS, false);
        if let Err(e) = result {
            if !said {
                eprintln!("[atspi] idle: {e}");
                said = true;
            }
        }
        // Missing/restarted daemons are normal. Reconnect without spawning a new thread.
        let until = Instant::now() + TIMEOUT;
        while Instant::now() < until {
            let _ = super::io::wait(read.as_raw_fd(), &mut [], Some(until));
            for notice in drain(&mailbox, &recv, &read) {
                apply_shell_notice(&shared, notice);
            }
        }
    }
}
fn apply_shell_notice(shared: &Mutex<objects::Shared>, notice: Notice) {
    let mut s = lock(shared);
    match notice {
        Notice::Geometry(key, origin) => {
            s.origins.insert(key, origin);
        }
        Notice::Focus(key, focused) => {
            s.focus.insert(key, focused);
        }
        Notice::SessionClosed(session) => {
            s.origins.retain(|k, _| (*k >> 32) as u32 != session);
            s.focus.retain(|k, _| (*k >> 32) as u32 != session);
            s.pending.retain(|(_, key)| (*key >> 32) as u32 != session);
        }
        Notice::Update(key, _) => {
            if a11y::with_tree(key, |_| ()).is_none() {
                s.origins.remove(&key);
                s.focus.remove(&key);
            }
        }
    }
}
fn connected(
    mailbox: &Mailbox,
    recv: &Receiver<Notice>,
    read: &UnixStream,
    shared: Arc<Mutex<objects::Shared>>,
) -> Result<(), String> {
    connected_inner(mailbox, recv, read, shared).map_err(|e| e.to_string())
}
fn connected_inner(
    mailbox: &Mailbox,
    recv: &Receiver<Notice>,
    read: &UnixStream,
    shared: Arc<Mutex<objects::Shared>>,
) -> makepad_dbus_wire::Result<()> {
    let mut session = Connection::session(TIMEOUT)?;
    session.add_match("type='signal',sender='org.a11y.Bus',path='/org/a11y/bus',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'", TIMEOUT)?;
    let mut enabled = call_status_get_is_enabled(&mut session, BUS, BUS_PATH, TIMEOUT)?;
    let mut reader_enabled =
        call_status_get_screen_reader_enabled(&mut session, BUS, BUS_PATH, TIMEOUT)?;
    let address = call_bus_get_address(&mut session, BUS, BUS_PATH, TIMEOUT)?;
    let mut bus = Connection::connect(&address, TIMEOUT)?;
    bus.add_match("type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.a11y.atspi.Registry'", TIMEOUT)?;
    let name = bus.unique_name().unwrap().to_owned();
    let mut objects = Objects::new(name.clone(), address.clone(), shared.clone());
    let mut server = makepad_dbus_wire::ObjectServer::new();
    server.register_prefix("/org/a11y/atspi", objects.clone())?;
    // Embed is asynchronous: the registry can call Application.Id before replying.
    let embed = bus.send(Message::method_call(
        REGISTRY,
        ROOT,
        SOCKET_INTERFACE,
        "Embed",
        Body::from_values(&[objects.reference(ROOT).value()])?,
    ))?;
    let mut pending_embed = Some((embed, Instant::now() + TIMEOUT));
    let mut last_call = None::<Instant>;
    let mut active = false;
    let mut history = events::History::default();
    loop {
        session.read_ready()?;
        bus.read_ready()?;
        while let Some(message) = session.pop_message() {
            if message.header.kind == MessageType::Signal
                && message.header.member.as_deref() == Some("PropertiesChanged")
            {
                // Read both, including invalidated properties; this bus cannot call our exported objects.
                enabled = call_status_get_is_enabled(&mut session, BUS, BUS_PATH, TIMEOUT)?;
                reader_enabled =
                    call_status_get_screen_reader_enabled(&mut session, BUS, BUS_PATH, TIMEOUT)?;
            }
        }
        if let Some((serial, deadline)) = pending_embed {
            if let Some(reply) = bus.take_reply(serial) {
                let reply = reply.into_result()?;
                let mut r = reply.body.reader();
                r.begin_struct()?;
                let parent = ObjRef {
                    bus: r.str()?.to_owned(),
                    path: r.object_path()?.to_owned(),
                };
                r.finish()?;
                lock(&shared).parent = Some(parent);
                lock(identity()).connection = Some((address.clone(), name.clone()));
                pending_embed = None;
                history.refresh_all(&mut objects, &mut bus)?;
            } else if Instant::now() >= deadline {
                return Err(makepad_dbus_wire::Error::Timeout);
            }
        }
        while let Some(message) = bus.pop_message() {
            if pending_embed.is_none()
                && message.header.kind == MessageType::Signal
                && message.header.interface.as_deref() == Some("org.freedesktop.DBus")
                && message.header.member.as_deref() == Some("NameOwnerChanged")
            {
                let values = message.body.values()?;
                // Ignore the registry's initial activation; recover a replaced or lost owner.
                if matches!(values.as_slice(), [Value::Str(service), Value::Str(old), Value::Str(_)] if service == REGISTRY && !old.is_empty())
                {
                    return Err(makepad_dbus_wire::Error::Invalid(
                        "accessibility registry restarted; reconnecting".into(),
                    ));
                }
            }
            if message.header.kind == MessageType::MethodCall {
                // The registry probes "/", outside our object prefix. Reply at any path,
                // without keeping the semantic consumer alive for transport traffic.
                if message.header.interface.as_deref() == Some("org.freedesktop.DBus.Peer")
                    && message.header.member.as_deref() == Some("Ping")
                {
                    if message.header.flags & makepad_dbus_wire::NO_REPLY_EXPECTED == 0 {
                        bus.send(Message::method_return(&message, Body::default()))?;
                    }
                    continue;
                }
                // Transport liveness probes do not consume the semantic tree.
                if message.header.interface.as_deref() != Some("org.freedesktop.DBus.Peer") {
                    last_call = Some(Instant::now());
                    if !active {
                        a11y::request_consumer(consumer::OS, true);
                        active = true;
                    }
                }
                if let Some(reply) = server.dispatch(&message)? {
                    bus.send(reply)?;
                }
            }
        }
        for notice in drain(mailbox, recv, read) {
            match notice {
                Notice::Update(key, notifications) => {
                    history.update(key, &notifications, &mut objects, &mut bus)?
                }
                Notice::SessionClosed(session) => {
                    apply_shell_notice(&shared, Notice::SessionClosed(session));
                    history.refresh_all(&mut objects, &mut bus)?;
                }
                Notice::Focus(key, focused) => {
                    let previous = lock(&shared).focus.insert(key, focused);
                    if previous != Some(focused) {
                        history.focus(key, focused, &mut objects, &mut bus)?;
                    }
                }
                notice => apply_shell_notice(&shared, notice),
            }
        }
        let on = enabled || reader_enabled || last_call.is_some_and(|t| t.elapsed() < IDLE);
        if on != active {
            a11y::request_consumer(consumer::OS, on);
            active = on;
        }
        objects.reap_actions();
        session.flush()?;
        bus.flush()?;
        let mut fds = [
            PollFd {
                fd: session.fd(),
                events: READ | if session.wants_write() { WRITE } else { 0 },
                revents: 0,
            },
            PollFd {
                fd: bus.fd(),
                events: READ | if bus.wants_write() { WRITE } else { 0 },
                revents: 0,
            },
        ];
        let deadline = if !lock(&shared).pending.is_empty() {
            Some(Instant::now() + Duration::from_millis(20))
        } else if let Some((_, deadline)) = pending_embed {
            Some(deadline)
        } else if active && !enabled && !reader_enabled {
            last_call.map(|t| t + IDLE)
        } else {
            None
        };
        super::io::wait(read.as_raw_fd(), &mut fds, deadline)
            .map_err(makepad_dbus_wire::Error::Invalid)?;
    }
}
trait RefValue {
    fn value(&self) -> Value;
}
impl RefValue for ObjRef {
    fn value(&self) -> Value {
        Value::Struct(vec![
            Value::Str(self.bus.clone()),
            Value::Path(self.path.clone()),
        ])
    }
}
