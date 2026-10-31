//! The WM's xdg-desktop-portal backend: one D-Bus name on the session bus,
//! `org.freedesktop.impl.portal.desktop.makepad`, object `/org/freedesktop/portal/desktop`.
//! xdg-desktop-portal finds it through `makepad.portal` (its `Interfaces=` lists what the WM
//! serves) and the user's `portals.conf` (which interfaces go to `makepad`); browsers and other
//! apps ask xdg-desktop-portal, which asks us.
//!
//! * `org.freedesktop.impl.portal.Settings` (served here, on the bus thread): the WM's
//!   appearance as `org.freedesktop.appearance` `color-scheme` (1 dark, 2 light), with
//!   `SettingChanged` when the desktop flips between light and dark, so Chromium follows live.
//! * Other interfaces (`FileChooser`): an [`Interface`] given to [`Portal::start`] decodes its
//!   calls on the bus thread; the UI thread takes them ([`Portal::take_calls`], woken by a UI
//!   signal) and answers each later with [`Portal::reply`] / [`Portal::reply_error`].
//!
//! Threading as the Wi-Fi panel's: the UI never touches the bus. One worker owns the
//! connection and sleeps in `poll()` on the bus fd and a wake pipe. Linux only.
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_int, c_short, c_ulong, c_void, CStr, CString};
use std::ptr::null_mut;
use std::sync::mpsc::{channel, Receiver, Sender};

use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner};
use makepad_widgets::log;

use crate::sdbus::*;

pub(crate) const BUS_NAME: &CStr = c"org.freedesktop.impl.portal.desktop.makepad";
const PATH: &CStr = c"/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.impl.portal.Settings";
const SETTINGS_C: &CStr = c"org.freedesktop.impl.portal.Settings";
const APPEARANCE: &str = "org.freedesktop.appearance";
const NOT_FOUND: &CStr = c"org.freedesktop.portal.Error.NotFound";
const UNKNOWN_METHOD: &CStr = c"org.freedesktop.DBus.Error.UnknownMethod";
const FAILED: &CStr = c"org.freedesktop.DBus.Error.Failed";

/// `org.freedesktop.appearance` `color-scheme`.
pub(crate) const PREFER_DARK: u32 = 1;
pub(crate) const PREFER_LIGHT: u32 = 2;

/// A portal interface whose calls the UI thread answers.
pub(crate) struct Interface {
    pub name: &'static str,
    /// Its methods and signals as introspection XML (what goes inside its `<interface>`).
    pub xml: &'static str,
    /// Reads a call's arguments (on the bus thread) into what the UI needs. An error is the
    /// D-Bus error reply.
    pub decode: fn(member: &str, call: &Msg) -> Result<Box<dyn Any + Send>, BusErr>,
}

/// A call for the UI thread: answer it once with `Portal::reply` or `Portal::reply_error`.
pub(crate) struct Call {
    pub id: u64,
    pub interface: &'static str,
    pub member: String,
    pub args: Box<dyn Any + Send>,
}

/// Writes a reply's arguments.
pub(crate) type Write = Box<dyn FnOnce(&Msg) -> Result<(), BusErr> + Send>;

enum Cmd {
    ColorScheme(u32),
    Reply(u64, Write),
    ReplyError(u64, String, String),
}

/// The UI end. Dropping it stops the worker, which leaves the bus (the name goes with it).
pub(crate) struct Portal {
    tx: Sender<Cmd>,
    calls: Receiver<Call>,
    wake_fd: c_int,
    _worker: TaskHandle<()>,
}

#[repr(C)]
struct PollFd { fd: c_int, events: c_short, revents: c_short }
extern "C" {
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}
const POLLIN: c_short = 1;
const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2000000;

impl Portal {
    /// Connects to the session bus and takes the backend's name (never from another owner: a
    /// second WM on the same session runs without a portal). `color_scheme` is the appearance now.
    pub(crate) fn start(spawner: &ThreadSpawner, interfaces: Vec<Interface>, color_scheme: u32) -> Result<Portal, String> {
        let mut fds: [c_int; 2] = [-1, -1];
        if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) } != 0 {
            return Err("no wake pipe".into());
        }
        let (tx, rx) = channel::<Cmd>();
        let (call_tx, calls) = channel::<Call>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let wake_read = fds[0];
        let worker = spawner
            .spawn_worker(ThreadOptions { name: Some("wm-portal".into()), ..Default::default() }, move || {
                match Worker::open(interfaces, color_scheme, call_tx, rx, wake_read) {
                    Ok(mut w) => {
                        let _ = ready_tx.send(Ok(()));
                        if let Err(e) = w.run() { log!("wm portal: {e}"); }
                    }
                    Err(e) => { let _ = ready_tx.send(Err(e)); }
                }
                unsafe { close(wake_read) };
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Portal { tx, calls, wake_fd: fds[1], _worker: worker }),
            Ok(Err(e)) => { unsafe { close(fds[1]) }; Err(e) }
            Err(_) => { unsafe { close(fds[1]) }; Err("the portal worker ended".into()) }
        }
    }

    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            let byte = 1u8;
            unsafe { write(self.wake_fd, &byte as *const u8 as *const c_void, 1) };
        }
    }

    /// The desktop's appearance changed: `SettingChanged` goes out when it differs.
    pub(crate) fn set_color_scheme(&self, scheme: u32) { self.send(Cmd::ColorScheme(scheme)); }
    /// The calls of the UI's interfaces since the last take.
    pub(crate) fn take_calls(&self) -> Vec<Call> { self.calls.try_iter().collect() }
    pub(crate) fn reply(&self, id: u64, write: Write) { self.send(Cmd::Reply(id, write)); }
    pub(crate) fn reply_error(&self, id: u64, name: String, message: String) { self.send(Cmd::ReplyError(id, name, message)); }
}

impl Drop for Portal {
    fn drop(&mut self) {
        // the channel closes with `tx`; the wake gets the worker out of poll() to see it
        let byte = 1u8;
        unsafe { write(self.wake_fd, &byte as *const u8 as *const c_void, 1) };
        unsafe { close(self.wake_fd) };
    }
}

/// What the object callback touches. Callbacks run only inside `sd_bus_process`, on the worker.
struct Shared {
    api: SdBusApi,
    color_scheme: Cell<u32>,
    interfaces: Vec<Interface>,
    pending: RefCell<HashMap<u64, *mut SdBusMessage>>,
    next_id: Cell<u64>,
    calls: Sender<Call>,
}

struct Worker {
    api: SdBusApi,
    bus: *mut SdBus,
    shared: Box<Shared>,
    rx: Receiver<Cmd>,
    wake_fd: c_int,
}

impl Worker {
    fn open(interfaces: Vec<Interface>, color_scheme: u32, calls: Sender<Call>, rx: Receiver<Cmd>, wake_fd: c_int) -> Result<Worker, String> {
        let lib = unsafe { dlopen(c"libsystemd.so.0".as_ptr(), RTLD_NOW) };
        if lib.is_null() { return Err("libsystemd.so.0 is not installed".into()); }
        let api = unsafe { SdBusApi::load(lib) }?;
        let mut bus: *mut SdBus = null_mut();
        let r = unsafe { (api.sd_bus_open_user)(&mut bus) };
        if r < 0 { return Err(format!("no session bus: {}", errno_text(-r))); }
        let shared = Box::new(Shared { api, color_scheme: Cell::new(color_scheme), interfaces, pending: RefCell::new(HashMap::new()), next_id: Cell::new(1), calls });
        let userdata = &*shared as *const Shared as *mut c_void;
        let r = unsafe { (api.sd_bus_add_object)(bus, null_mut(), PATH.as_ptr(), object_cb, userdata) };
        if r < 0 {
            unsafe { (api.sd_bus_flush_close_unref)(bus) };
            return Err(format!("cannot serve {}: {}", PATH.to_string_lossy(), errno_text(-r)));
        }
        let r = unsafe { (api.sd_bus_request_name)(bus, BUS_NAME.as_ptr(), 0) };
        if r < 0 {
            unsafe { (api.sd_bus_flush_close_unref)(bus) };
            return Err(format!("cannot own {}: {} (another WM serves it)", BUS_NAME.to_string_lossy(), errno_text(-r)));
        }
        log!("wm portal: serving {} (Settings{})", BUS_NAME.to_string_lossy(),
            shared.interfaces.iter().map(|i| format!(", {}", i.name.rsplit('.').next().unwrap_or(i.name))).collect::<String>());
        Ok(Worker { api, bus, shared, rx, wake_fd })
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            loop {
                let r = unsafe { (self.api.sd_bus_process)(self.bus, null_mut()) };
                if r < 0 { return Err(format!("session bus lost: {}", errno_text(-r))); }
                if r == 0 { break; }
            }
            let mut sent = false;
            loop {
                match self.rx.try_recv() {
                    Ok(cmd) => {
                        self.command(cmd);
                        sent = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // A reply or signal just sent may have read the next call into sd-bus's queue
            // (off the socket): process before sleeping on the socket.
            if sent {
                continue;
            }
            self.wait();
        }
    }

    fn command(&mut self, cmd: Cmd) {
        let api = self.api;
        match cmd {
            Cmd::ColorScheme(scheme) => {
                if self.shared.color_scheme.replace(scheme) == scheme { return; }
                let mut m: *mut SdBusMessage = null_mut();
                if unsafe { (api.sd_bus_message_new_signal)(self.bus, &mut m, PATH.as_ptr(), SETTINGS_C.as_ptr(), c"SettingChanged".as_ptr()) } < 0 { return; }
                let msg = Msg { api, m, owned: true };
                let built = msg.append_str(b's', c"org.freedesktop.appearance")
                    .and_then(|_| msg.append_str(b's', c"color-scheme"))
                    .and_then(|_| msg.open(b'v', c"u"))
                    .and_then(|_| msg.append_num(b'u', scheme))
                    .and_then(|_| msg.close());
                if built.is_ok() { unsafe { (api.sd_bus_send)(self.bus, m, null_mut()) }; }
            }
            Cmd::Reply(id, write) => {
                let Some(call) = self.shared.pending.borrow_mut().remove(&id) else { return };
                let mut reply: *mut SdBusMessage = null_mut();
                if unsafe { (api.sd_bus_message_new_method_return)(call, &mut reply) } >= 0 {
                    let msg = Msg { api, m: reply, owned: true };
                    match write(&msg) {
                        Ok(()) => { unsafe { (api.sd_bus_send)(null_mut(), reply, null_mut()) }; }
                        Err(e) => reply_error(&api, call, FAILED, &e.to_string()),
                    }
                }
                unsafe { (api.sd_bus_message_unref)(call) };
            }
            Cmd::ReplyError(id, name, message) => {
                let Some(call) = self.shared.pending.borrow_mut().remove(&id) else { return };
                let name = CString::new(name).unwrap_or_else(|_| FAILED.to_owned());
                reply_error(&api, call, &name, &message);
                unsafe { (api.sd_bus_message_unref)(call) };
            }
        }
    }

    fn wait(&self) {
        let mut events = unsafe { (self.api.sd_bus_get_events)(self.bus) };
        if events < 0 { events = POLLIN as c_int; }
        // sd-bus's own deadline (CLOCK_MONOTONIC, usec): 0 when it already holds queued
        // messages, none (u64::MAX) when only the socket can bring news.
        let mut until: u64 = u64::MAX;
        let timeout_ms = if unsafe { (self.api.sd_bus_get_timeout)(self.bus, &mut until) } < 0 || until == u64::MAX {
            -1
        } else {
            let now = monotonic_usec();
            (until.saturating_sub(now).div_ceil(1000)).min(i32::MAX as u64) as c_int
        };
        let mut fds = [PollFd { fd: unsafe { (self.api.sd_bus_get_fd)(self.bus) }, events: events as c_short, revents: 0 },
            PollFd { fd: self.wake_fd, events: POLLIN, revents: 0 }];
        unsafe { poll(fds.as_mut_ptr(), 2, timeout_ms) };
        let mut sink = [0u8; 64];
        while unsafe { read(self.wake_fd, sink.as_mut_ptr() as *mut c_void, sink.len()) } > 0 {}
    }
}

/// CLOCK_MONOTONIC in microseconds (sd-bus's clock for its deadlines).
fn monotonic_usec() -> u64 {
    #[repr(C)]
    struct Timespec { sec: i64, nsec: i64 }
    extern "C" { fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int; }
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(1, &mut ts) };
    ts.sec as u64 * 1_000_000 + ts.nsec as u64 / 1000
}

impl Drop for Worker {
    fn drop(&mut self) {
        for (_, call) in self.shared.pending.borrow_mut().drain() {
            reply_error(&self.api, call, FAILED, "the WM is shutting down");
            unsafe { (self.api.sd_bus_message_unref)(call) };
        }
        unsafe { (self.api.sd_bus_flush_close_unref)(self.bus) };
    }
}

fn reply_error(api: &SdBusApi, call: *mut SdBusMessage, name: &CStr, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    let e = SdBusError { name: name.as_ptr(), message: message.as_ptr(), need_free: 0 };
    unsafe { (api.sd_bus_reply_method_error)(call, &e) };
}

/// Sends a method return whose arguments `write` appends.
fn reply_with(api: &SdBusApi, call: *mut SdBusMessage, write: impl FnOnce(&Msg) -> Result<(), BusErr>) {
    let mut reply: *mut SdBusMessage = null_mut();
    if unsafe { (api.sd_bus_message_new_method_return)(call, &mut reply) } < 0 { return; }
    let msg = Msg { api: *api, m: reply, owned: true };
    match write(&msg) {
        Ok(()) => { unsafe { (api.sd_bus_send)(null_mut(), reply, null_mut()) }; }
        Err(e) => reply_error(api, call, FAILED, &e.to_string()),
    }
}

/// The settings the WM publishes: (namespace, key) and how to append the value in a variant.
fn settings(color_scheme: u32) -> [(&'static str, &'static str, u32); 1] {
    [(APPEARANCE, "color-scheme", color_scheme)]
}

/// `ReadAll`'s namespace patterns: empty is all, `a.b.*` a prefix, else exact.
fn wanted(patterns: &[String], namespace: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => namespace.starts_with(prefix),
        None => p == namespace,
    })
}

fn append_u32_variant(msg: &Msg, v: u32) -> Result<(), BusErr> {
    msg.open(b'v', c"u")?;
    msg.append_num(b'u', v)?;
    msg.close()
}

/// `ReadAll(as namespaces) -> a{sa{sv}}`.
fn read_all(msg: &Msg, patterns: &[String], scheme: u32) -> Result<(), BusErr> {
    msg.open(b'a', c"{sa{sv}}")?;
    let all = settings(scheme);
    let mut namespaces: Vec<&str> = all.iter().map(|s| s.0).filter(|ns| wanted(patterns, ns)).collect();
    namespaces.dedup();
    for ns in namespaces {
        msg.open(b'e', c"sa{sv}")?;
        msg.append_str(b's', &cstring(ns)?)?;
        msg.open(b'a', c"{sv}")?;
        for (_, key, value) in all.iter().filter(|s| s.0 == ns) {
            msg.open(b'e', c"sv")?;
            msg.append_str(b's', &cstring(key)?)?;
            append_u32_variant(msg, *value)?;
            msg.close()?;
        }
        msg.close()?;
        msg.close()?;
    }
    msg.close()
}

fn introspection(interfaces: &[Interface]) -> String {
    let mut xml = String::from(concat!(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
        " <interface name=\"org.freedesktop.DBus.Introspectable\"><method name=\"Introspect\"><arg type=\"s\" direction=\"out\"/></method></interface>\n",
        " <interface name=\"org.freedesktop.DBus.Properties\"><method name=\"Get\"><arg type=\"s\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/><arg type=\"v\" direction=\"out\"/></method>",
        "<method name=\"GetAll\"><arg type=\"s\" direction=\"in\"/><arg type=\"a{sv}\" direction=\"out\"/></method></interface>\n",
        " <interface name=\"org.freedesktop.impl.portal.Settings\"><method name=\"ReadAll\"><arg type=\"as\" name=\"namespaces\" direction=\"in\"/><arg type=\"a{sa{sv}}\" name=\"value\" direction=\"out\"/></method>",
        "<method name=\"Read\"><arg type=\"s\" name=\"namespace\" direction=\"in\"/><arg type=\"s\" name=\"key\" direction=\"in\"/><arg type=\"v\" name=\"value\" direction=\"out\"/></method>",
        "<signal name=\"SettingChanged\"><arg type=\"s\" name=\"namespace\"/><arg type=\"s\" name=\"key\"/><arg type=\"v\" name=\"value\"/></signal>",
        "<property name=\"version\" type=\"u\" access=\"read\"/></interface>\n"));
    for i in interfaces { xml.push_str(&format!(" <interface name=\"{}\">{}</interface>\n", i.name, i.xml)); }
    xml.push_str("</node>\n");
    xml
}

/// Every method call on the object.
unsafe extern "C" fn object_cb(m: *mut SdBusMessage, userdata: *mut c_void, _err: *mut SdBusError) -> c_int {
    let shared = &*(userdata as *const Shared);
    let api = shared.api;
    let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let iface = cstr_at((api.sd_bus_message_get_interface)(m));
        let member = cstr_at((api.sd_bus_message_get_member)(m));
        let msg = Msg::borrowed(api, m);
        let scheme = shared.color_scheme.get();
        match (iface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus.Introspectable", "Introspect") => {
                let xml = cstring(&introspection(&shared.interfaces)).unwrap_or_default();
                reply_with(&api, m, |r| r.append_str(b's', &xml));
            }
            ("org.freedesktop.DBus.Properties", "Get" | "GetAll") => {
                let target = msg.read_str(b's').unwrap_or_default();
                let prop = if member == "Get" { msg.read_str(b's').unwrap_or_default() } else { String::new() };
                if target != SETTINGS || (member == "Get" && prop != "version") {
                    reply_error(&api, m, c"org.freedesktop.DBus.Error.UnknownProperty", "no such property");
                } else if member == "Get" {
                    reply_with(&api, m, |r| append_u32_variant(r, 1));
                } else {
                    reply_with(&api, m, |r| {
                        r.open(b'a', c"{sv}")?;
                        r.open(b'e', c"sv")?;
                        r.append_str(b's', c"version")?;
                        append_u32_variant(r, 1)?;
                        r.close()?;
                        r.close()
                    });
                }
            }
            (SETTINGS, "ReadAll") => {
                let mut patterns = Vec::new();
                if msg.enter(b'a', c"s").unwrap_or(false) {
                    while !msg.at_end().unwrap_or(true) { match msg.read_str(b's') { Ok(p) => patterns.push(p), Err(_) => break } }
                    let _ = msg.exit();
                }
                reply_with(&api, m, |r| read_all(r, &patterns, scheme));
            }
            (SETTINGS, "Read") => {
                let ns = msg.read_str(b's').unwrap_or_default();
                let key = msg.read_str(b's').unwrap_or_default();
                match settings(scheme).iter().find(|s| s.0 == ns && s.1 == key) {
                    Some(&(_, _, value)) => reply_with(&api, m, |r| append_u32_variant(r, value)),
                    None => reply_error(&api, m, NOT_FOUND, "the WM has no such setting"),
                }
            }
            _ => match shared.interfaces.iter().find(|i| i.name == iface) {
                Some(i) => match (i.decode)(&member, &msg) {
                    Ok(args) => {
                        let id = shared.next_id.get();
                        shared.next_id.set(id + 1);
                        (api.sd_bus_message_ref)(m);
                        shared.pending.borrow_mut().insert(id, m);
                        if shared.calls.send(Call { id, interface: i.name, member, args }).is_ok() {
                            SignalToUI::set_ui_signal();
                        }
                    }
                    Err(e) => reply_error(&api, m, FAILED, &e.to_string()),
                },
                None => return false,
            },
        }
        true
    }));
    match handled {
        Ok(true) => 1,
        Ok(false) => { reply_error(&api, m, UNKNOWN_METHOD, "the WM's portal has no such method"); 1 }
        Err(_) => { reply_error(&api, m, FAILED, "the WM's portal failed"); 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readall_namespace_patterns() {
        assert!(wanted(&[], APPEARANCE));
        assert!(wanted(&["org.freedesktop.*".into()], APPEARANCE));
        assert!(wanted(&[APPEARANCE.into()], APPEARANCE));
        assert!(!wanted(&["org.gnome.*".into()], APPEARANCE));
        assert!(!wanted(&["org.freedesktop.appearance.x".into()], APPEARANCE));
    }
    #[test]
    fn the_introspection_names_every_interface() {
        let xml = introspection(&[Interface { name: "org.freedesktop.impl.portal.FileChooser", xml: "", decode: |_, _| Ok(Box::new(())) }]);
        assert!(xml.contains("org.freedesktop.impl.portal.Settings") && xml.contains("org.freedesktop.impl.portal.FileChooser"));
    }
}
