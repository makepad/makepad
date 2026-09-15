//! `shell/wifi_linux.rs` — the Wi-Fi dropdown's backend and state, Linux
//! only (the `mod` is gated in `shell/mod.rs`; nothing here is compiled or
//! referenced on another OS).
//!
//! # What talks to what
//!
//! iwd (`net.connman.iwd`) is the Wi-Fi daemon on the target box and it
//! speaks D-Bus only. This module is an in-house adapter to that API over
//! **sd-bus**, the D-Bus client library every systemd box ships in
//! `libsystemd.so.0`. The library is loaded with `dlopen` at runtime, like
//! the other Linux system libraries the platform uses, so the binary has no
//! link-time dependency: a box without it (or without iwd) gets an honest
//! "unavailable" reading, not a link failure. No NetworkManager, no
//! `iwctl` output parsing, no subprocess, no external crate.
//!
//! # Threading (AGENTS.md "Threading and realtime ownership")
//!
//! The UI thread never touches the bus. [`WifiLink`] is the UI end: a
//! bounded command channel with non-blocking sends (a full queue keeps the
//! command and retries on the next tick), an event channel drained with
//! `try_recv`, and a wake pipe that gets the worker out of `poll()`. One
//! long-lived worker per session ([`run_worker`], spawned once through
//! `ThreadSpawner::spawn_worker` when the dropdown first opens) owns the bus
//! connection, the credential-agent object, the iwd state and the address
//! sampling; it sleeps in `poll()` on the bus fd and the wake pipe, so at
//! rest it costs nothing.
//!
//! # Secrets
//!
//! A passphrase lives in a [`Secret`] (wiped on drop and on clear), moves
//! UI → channel → worker by value (no clone anywhere on the way), is handed
//! to iwd only inside the reply to iwd's own `RequestPassphrase` for the
//! very network we asked to join, and is wiped right after. The agent
//! accepts that request only from the connection that currently owns
//! `net.connman.iwd` (checked against the bus's `GetNameOwner` answer and
//! tracked through `NameOwnerChanged`), never from another bus client. A
//! pending passphrase is dropped on `Cancel`/`Release`, on an iwd owner
//! change, when the connect call returns (success or error), when the user
//! cancels, and at shutdown. It never appears in a command line, the
//! environment, a log line, a widget's `text()` or a remote snapshot: the
//! dropdown's summary reports only whether the field is filled.
//!
//! iwd stores the accepted passphrase itself (`/var/lib/iwd/<ssid>.psk`),
//! which is the credential storage this design relies on: a saved network
//! reconnects without a prompt, "Forget" removes it.
//!
//! # Scope honesty
//!
//! * `psk` networks get the password field; `open` connect directly.
//! * `8021x` (enterprise) networks are listed but NOT offered a plain
//!   password: iwd needs a provisioning file for them, and the row says so.
//! * WEP is not supported by iwd and is listed as such.
//! * Hidden networks are not handled (no `ConnectHiddenNetwork` UI).
//! * Addresses are the machine's own IPv4 addresses from `getifaddrs`
//!   (no public-IP lookup, no web request), labelled wired/wireless by the
//!   iwd device name or `/sys/class/net/<if>/wireless`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_long, c_short, c_uint, c_ulong, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use makepad_widgets::makepad_platform::thread::{
    SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner,
};
use makepad_widgets::*;

// ======================================================================
// libc — only what the worker needs, declared here (no libc crate)
// ======================================================================

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[repr(C)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: c_long,
}

#[repr(C)]
struct SockAddr {
    sa_family: u16,
    _sa_data: [u8; 14],
}

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    _sin_zero: [u8; 8],
}

#[repr(C)]
struct IfAddrs {
    ifa_next: *mut IfAddrs,
    ifa_name: *mut c_char,
    ifa_flags: c_uint,
    ifa_addr: *mut SockAddr,
    _ifa_netmask: *mut SockAddr,
    _ifa_ifu: *mut SockAddr,
    _ifa_data: *mut c_void,
}

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn clock_gettime(clock: c_int, ts: *mut TimeSpec) -> c_int;
    fn getifaddrs(ifap: *mut *mut IfAddrs) -> c_int;
    fn freeifaddrs(ifa: *mut IfAddrs);
}

const RTLD_NOW: c_int = 2;
const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2000000;
const POLLIN: c_short = 0x1;
const CLOCK_MONOTONIC: c_int = 1;
const AF_INET: u16 = 2;
const IFF_UP: c_uint = 0x1;
const IFF_LOOPBACK: c_uint = 0x8;

fn now_usec() -> u64 {
    let mut ts = TimeSpec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + (ts.tv_nsec / 1000) as u64
}

fn now_secs() -> f64 {
    now_usec() as f64 / 1_000_000.0
}

fn errno_text(n: i32) -> String {
    match n {
        1 => "not permitted (EPERM)".into(),
        2 => "not found (ENOENT)".into(),
        6 => "no such device (ENXIO)".into(),
        11 => "try again (EAGAIN)".into(),
        12 => "out of memory (ENOMEM)".into(),
        13 => "access denied (EACCES)".into(),
        16 => "busy (EBUSY)".into(),
        22 => "invalid argument (EINVAL)".into(),
        95 => "not supported (EOPNOTSUPP)".into(),
        107 => "bus disconnected (ENOTCONN)".into(),
        110 => "timed out (ETIMEDOUT)".into(),
        111 => "connection refused (ECONNREFUSED)".into(),
        113 => "no such service (EHOSTUNREACH)".into(),
        _ => format!("errno {n}"),
    }
}

/// The machine's own IPv4 addresses, wired first. `wifi_device` is iwd's
/// device name so the wireless interface is labelled even before
/// `/sys/class/net/<if>/wireless` is looked at.
fn sample_addresses(wifi_device: Option<&str>) -> Vec<IfAddr> {
    let mut head: *mut IfAddrs = null_mut();
    if unsafe { getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut p = head;
    while !p.is_null() {
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        let family = unsafe { (*ifa.ifa_addr).sa_family };
        if family != AF_INET
            || ifa.ifa_flags & IFF_LOOPBACK != 0
            || ifa.ifa_flags & IFF_UP == 0
        {
            continue;
        }
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let sin = unsafe { &*(ifa.ifa_addr as *const SockAddrIn) };
        let a = sin.sin_addr;
        let ipv4 = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
        let wireless = wifi_device == Some(name.as_str())
            || std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists();
        out.push(IfAddr {
            name,
            wireless,
            ipv4,
        });
    }
    unsafe { freeifaddrs(head) };
    out.sort_by(|a, b| a.wireless.cmp(&b.wireless).then_with(|| a.name.cmp(&b.name)));
    out
}

// ======================================================================
// sd-bus — the subset of `<systemd/sd-bus.h>` this adapter uses, resolved
// from `libsystemd.so.0` at runtime. Only non-variadic entry points: the
// builder/reader API instead of `sd_bus_call_method(..., types, ...)`.
// ======================================================================

#[repr(C)]
pub struct SdBus {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct SdBusMessage {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct SdBusSlot {
    _opaque: [u8; 0],
}

/// `sd_bus_error` — two C strings and an ownership flag.
#[repr(C)]
struct SdBusError {
    name: *const c_char,
    message: *const c_char,
    need_free: c_int,
}

impl SdBusError {
    fn null() -> Self {
        Self {
            name: null(),
            message: null(),
            need_free: 0,
        }
    }

    fn constant(name: &'static CStr, message: &'static CStr) -> Self {
        Self {
            name: name.as_ptr(),
            message: message.as_ptr(),
            need_free: 0,
        }
    }

    fn is_set(&self) -> bool {
        !self.name.is_null()
    }

    fn name(&self) -> String {
        cstr_at(self.name)
    }

    fn message(&self) -> String {
        cstr_at(self.message)
    }
}

/// Copy a C string out immediately; sd-bus pointers live only as long as
/// the message they came from.
fn cstr_at(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

type SdBusHandler =
    unsafe extern "C" fn(*mut SdBusMessage, *mut c_void, *mut SdBusError) -> c_int;

macro_rules! sd_bus_api {
    ($( fn $name:ident($($arg:ty),*) -> $ret:ty; )*) => {
        #[derive(Clone, Copy)]
        #[allow(non_snake_case)]
        struct SdBusApi {
            $( $name: unsafe extern "C" fn($($arg),*) -> $ret, )*
        }

        impl SdBusApi {
            /// Resolve every entry point from an open `libsystemd` handle.
            unsafe fn load(lib: *mut c_void) -> Result<Self, String> {
                Ok(Self {
                    $( $name: {
                        let symbol = concat!(stringify!($name), "\0");
                        let p = dlsym(lib, symbol.as_ptr() as *const c_char);
                        if p.is_null() {
                            return Err(format!("libsystemd has no {}", stringify!($name)));
                        }
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> $ret>(p)
                    }, )*
                })
            }
        }
    };
}

sd_bus_api! {
    fn sd_bus_open_system(*mut *mut SdBus) -> c_int;
    fn sd_bus_flush_close_unref(*mut SdBus) -> *mut SdBus;
    fn sd_bus_get_fd(*mut SdBus) -> c_int;
    fn sd_bus_get_events(*mut SdBus) -> c_int;
    fn sd_bus_get_timeout(*mut SdBus, *mut u64) -> c_int;
    fn sd_bus_process(*mut SdBus, *mut *mut SdBusMessage) -> c_int;
    fn sd_bus_add_match(*mut SdBus, *mut *mut SdBusSlot, *const c_char, SdBusHandler, *mut c_void) -> c_int;
    fn sd_bus_add_object(*mut SdBus, *mut *mut SdBusSlot, *const c_char, SdBusHandler, *mut c_void) -> c_int;
    fn sd_bus_call(*mut SdBus, *mut SdBusMessage, u64, *mut SdBusError, *mut *mut SdBusMessage) -> c_int;
    fn sd_bus_call_async(*mut SdBus, *mut *mut SdBusSlot, *mut SdBusMessage, SdBusHandler, *mut c_void, u64) -> c_int;
    fn sd_bus_send(*mut SdBus, *mut SdBusMessage, *mut u64) -> c_int;
    fn sd_bus_message_new_method_call(*mut SdBus, *mut *mut SdBusMessage, *const c_char, *const c_char, *const c_char, *const c_char) -> c_int;
    fn sd_bus_message_new_method_return(*mut SdBusMessage, *mut *mut SdBusMessage) -> c_int;
    fn sd_bus_reply_method_error(*mut SdBusMessage, *const SdBusError) -> c_int;
    fn sd_bus_message_unref(*mut SdBusMessage) -> *mut SdBusMessage;
    fn sd_bus_message_append_basic(*mut SdBusMessage, c_char, *const c_void) -> c_int;
    fn sd_bus_message_open_container(*mut SdBusMessage, c_char, *const c_char) -> c_int;
    fn sd_bus_message_close_container(*mut SdBusMessage) -> c_int;
    fn sd_bus_message_read_basic(*mut SdBusMessage, c_char, *mut c_void) -> c_int;
    fn sd_bus_message_enter_container(*mut SdBusMessage, c_char, *const c_char) -> c_int;
    fn sd_bus_message_exit_container(*mut SdBusMessage) -> c_int;
    fn sd_bus_message_peek_type(*mut SdBusMessage, *mut c_char, *mut *const c_char) -> c_int;
    fn sd_bus_message_skip(*mut SdBusMessage, *const c_char) -> c_int;
    fn sd_bus_message_at_end(*mut SdBusMessage, c_int) -> c_int;
    fn sd_bus_message_get_sender(*mut SdBusMessage) -> *const c_char;
    fn sd_bus_message_get_member(*mut SdBusMessage) -> *const c_char;
    fn sd_bus_message_get_interface(*mut SdBusMessage) -> *const c_char;
    fn sd_bus_message_is_method_error(*mut SdBusMessage, *const c_char) -> c_int;
    fn sd_bus_message_get_error(*mut SdBusMessage) -> *const SdBusError;
    fn sd_bus_error_free(*mut SdBusError) -> ();
    fn sd_bus_slot_unref(*mut SdBusSlot) -> *mut SdBusSlot;
}

const IWD_NAME: &CStr = c"net.connman.iwd";
const IWD_ROOT: &CStr = c"/net/connman/iwd";
const ROOT_PATH: &CStr = c"/";
const DBUS_NAME: &CStr = c"org.freedesktop.DBus";
const DBUS_PATH: &CStr = c"/org/freedesktop/DBus";
const IFACE_DBUS: &CStr = c"org.freedesktop.DBus";
const IFACE_OBJECT_MANAGER: &CStr = c"org.freedesktop.DBus.ObjectManager";
const IFACE_PROPERTIES: &CStr = c"org.freedesktop.DBus.Properties";
const IFACE_AGENT_MANAGER: &CStr = c"net.connman.iwd.AgentManager";
const IFACE_STATION: &CStr = c"net.connman.iwd.Station";
const IFACE_NETWORK: &CStr = c"net.connman.iwd.Network";
const IFACE_KNOWN_NETWORK: &CStr = c"net.connman.iwd.KnownNetwork";
const IFACE_DEVICE: &CStr = c"net.connman.iwd.Device";
const IFACE_AGENT: &str = "net.connman.iwd.Agent";
/// Our credential agent's object path on our own connection.
const AGENT_PATH: &CStr = c"/org/makepad/wm/iwd_agent";
const AGENT_CANCELED: &CStr = c"net.connman.iwd.Agent.Error.Canceled";

const CALL_TIMEOUT_US: u64 = 5_000_000;
const CONNECT_TIMEOUT_US: u64 = 120_000_000;
/// iwd emits a burst of `PropertiesChanged` around a scan or a connect;
/// one refresh per burst.
const REFRESH_DEBOUNCE_SECS: f64 = 0.15;
/// DHCP lands after iwd says "connected": re-read the addresses a few
/// times after a state change instead of polling forever.
const ADDRESS_FOLLOWUPS_SECS: [f64; 3] = [1.5, 4.0, 10.0];
const COMMAND_QUEUE: usize = 16;

/// What went wrong on the bus: a negative errno from sd-bus or a D-Bus
/// error reply.
#[derive(Clone, Debug)]
enum BusErr {
    Sys(i32),
    Dbus { name: String, message: String },
}

impl BusErr {
    /// The error name's last segment (`net.connman.iwd.Failed` → `Failed`).
    fn suffix(&self) -> &str {
        match self {
            BusErr::Sys(_) => "",
            BusErr::Dbus { name, .. } => name.rsplit('.').next().unwrap_or(""),
        }
    }
}

impl std::fmt::Display for BusErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusErr::Sys(n) => write!(f, "{}", errno_text(*n)),
            BusErr::Dbus { name, message } => {
                if message.is_empty() {
                    write!(f, "{name}")
                } else {
                    write!(f, "{message} ({name})")
                }
            }
        }
    }
}

fn check(r: c_int) -> Result<c_int, BusErr> {
    if r < 0 {
        Err(BusErr::Sys(-r))
    } else {
        Ok(r)
    }
}

fn cstring(s: &str) -> Result<CString, BusErr> {
    CString::new(s).map_err(|_| BusErr::Sys(22))
}

/// A value read out of a D-Bus variant. Only what iwd's properties use.
#[derive(Clone, Debug, PartialEq)]
enum DbusValue {
    Str(String),
    Path(String),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    U8(u8),
    Paths(Vec<String>),
    Other,
}

type Props = Vec<(String, DbusValue)>;

fn prop_str(props: &Props, key: &str) -> Option<String> {
    props.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        DbusValue::Str(s) | DbusValue::Path(s) => Some(s.clone()),
        _ => None,
    })
}

fn prop_bool(props: &Props, key: &str) -> Option<bool> {
    props.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        DbusValue::Bool(b) => Some(*b),
        _ => None,
    })
}

/// One object out of `GetManagedObjects`.
struct DbusObject {
    path: String,
    ifaces: Vec<(String, Props)>,
}

impl DbusObject {
    fn iface(&self, name: &CStr) -> Option<&Props> {
        let name = name.to_str().unwrap_or("");
        self.ifaces.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }
}

/// One `sd_bus_message` reference: owned (unref'd on drop) for calls and
/// replies, borrowed for the messages a handler is given.
struct Msg {
    api: SdBusApi,
    m: *mut SdBusMessage,
    owned: bool,
}

impl Drop for Msg {
    fn drop(&mut self) {
        if self.owned && !self.m.is_null() {
            unsafe { (self.api.sd_bus_message_unref)(self.m) };
        }
    }
}

impl Msg {
    fn borrowed(api: SdBusApi, m: *mut SdBusMessage) -> Self {
        Self {
            api,
            m,
            owned: false,
        }
    }

    // ------------------------------------------------------------ write

    fn append_str(&self, ty: u8, s: &CStr) -> Result<(), BusErr> {
        check(unsafe {
            (self.api.sd_bus_message_append_basic)(
                self.m,
                ty as c_char,
                s.as_ptr() as *const c_void,
            )
        })
        .map(|_| ())
    }

    fn append_bool(&self, b: bool) -> Result<(), BusErr> {
        let v: c_int = b as c_int;
        check(unsafe {
            (self.api.sd_bus_message_append_basic)(
                self.m,
                b'b' as c_char,
                &v as *const c_int as *const c_void,
            )
        })
        .map(|_| ())
    }

    fn open(&self, ty: u8, contents: &CStr) -> Result<(), BusErr> {
        check(unsafe {
            (self.api.sd_bus_message_open_container)(self.m, ty as c_char, contents.as_ptr())
        })
        .map(|_| ())
    }

    fn close(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_close_container)(self.m) }).map(|_| ())
    }

    // ------------------------------------------------------------- read

    fn read_str(&self, ty: u8) -> Result<String, BusErr> {
        let mut p: *const c_char = null();
        check(unsafe {
            (self.api.sd_bus_message_read_basic)(
                self.m,
                ty as c_char,
                &mut p as *mut *const c_char as *mut c_void,
            )
        })?;
        Ok(cstr_at(p))
    }

    fn read_bool(&self) -> Result<bool, BusErr> {
        let mut v: c_int = 0;
        check(unsafe {
            (self.api.sd_bus_message_read_basic)(
                self.m,
                b'b' as c_char,
                &mut v as *mut c_int as *mut c_void,
            )
        })?;
        Ok(v != 0)
    }

    fn read_num<T: Copy + Default>(&self, ty: u8) -> Result<T, BusErr> {
        let mut v: T = T::default();
        check(unsafe {
            (self.api.sd_bus_message_read_basic)(
                self.m,
                ty as c_char,
                &mut v as *mut T as *mut c_void,
            )
        })?;
        Ok(v)
    }

    /// Enter a container; `Ok(false)` when the enclosing container has no
    /// more elements.
    fn enter(&self, ty: u8, contents: &CStr) -> Result<bool, BusErr> {
        let r = check(unsafe {
            (self.api.sd_bus_message_enter_container)(self.m, ty as c_char, contents.as_ptr())
        })?;
        Ok(r > 0)
    }

    fn exit(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_exit_container)(self.m) }).map(|_| ())
    }

    fn at_end(&self) -> Result<bool, BusErr> {
        Ok(check(unsafe { (self.api.sd_bus_message_at_end)(self.m, 0) })? > 0)
    }

    /// The type (and, for containers, the contents signature) of the next
    /// element; `None` at the end of the current container.
    fn peek(&self) -> Result<Option<(u8, String)>, BusErr> {
        let mut ty: c_char = 0;
        let mut contents: *const c_char = null();
        let r = check(unsafe { (self.api.sd_bus_message_peek_type)(self.m, &mut ty, &mut contents) })?;
        if r == 0 {
            return Ok(None);
        }
        Ok(Some((ty as u8, cstr_at(contents))))
    }

    fn skip(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_skip)(self.m, null()) }).map(|_| ())
    }

    /// One variant, whatever is inside.
    fn read_variant(&self) -> Result<DbusValue, BusErr> {
        let Some((ty, contents)) = self.peek()? else {
            return Ok(DbusValue::Other);
        };
        if ty != b'v' {
            self.skip()?;
            return Ok(DbusValue::Other);
        }
        let sig = cstring(&contents)?;
        if !self.enter(b'v', &sig)? {
            return Ok(DbusValue::Other);
        }
        let value = match contents.as_str() {
            "s" => DbusValue::Str(self.read_str(b's')?),
            "o" => DbusValue::Path(self.read_str(b'o')?),
            "b" => DbusValue::Bool(self.read_bool()?),
            "n" => DbusValue::I16(self.read_num::<i16>(b'n')?),
            "q" => DbusValue::U16(self.read_num::<u16>(b'q')?),
            "i" => DbusValue::I32(self.read_num::<i32>(b'i')?),
            "u" => DbusValue::U32(self.read_num::<u32>(b'u')?),
            "x" => DbusValue::I64(self.read_num::<i64>(b'x')?),
            "t" => DbusValue::U64(self.read_num::<u64>(b't')?),
            "y" => DbusValue::U8(self.read_num::<u8>(b'y')?),
            "ao" => {
                let mut paths = Vec::new();
                if self.enter(b'a', c"o")? {
                    while !self.at_end()? {
                        paths.push(self.read_str(b'o')?);
                    }
                    self.exit()?;
                }
                DbusValue::Paths(paths)
            }
            _ => {
                self.skip()?;
                DbusValue::Other
            }
        };
        self.exit()?;
        Ok(value)
    }

    /// `a{sv}`.
    fn read_dict_sv(&self) -> Result<Props, BusErr> {
        let mut out = Vec::new();
        if !self.enter(b'a', c"{sv}")? {
            return Ok(out);
        }
        while self.enter(b'e', c"sv")? {
            let key = self.read_str(b's')?;
            let value = self.read_variant()?;
            out.push((key, value));
            self.exit()?;
        }
        self.exit()?;
        Ok(out)
    }

    /// `a{oa{sa{sv}}}` — the `GetManagedObjects` reply.
    fn read_managed_objects(&self) -> Result<Vec<DbusObject>, BusErr> {
        let mut objects = Vec::new();
        if !self.enter(b'a', c"{oa{sa{sv}}}")? {
            return Ok(objects);
        }
        while self.enter(b'e', c"oa{sa{sv}}")? {
            let path = self.read_str(b'o')?;
            let mut ifaces = Vec::new();
            if self.enter(b'a', c"{sa{sv}}")? {
                while self.enter(b'e', c"sa{sv}")? {
                    let iface = self.read_str(b's')?;
                    let props = self.read_dict_sv()?;
                    ifaces.push((iface, props));
                    self.exit()?;
                }
                self.exit()?;
            }
            objects.push(DbusObject { path, ifaces });
            self.exit()?;
        }
        self.exit()?;
        Ok(objects)
    }

    /// `a(on)` — `GetOrderedNetworks`: (network path, signal in 1/100 dBm).
    fn read_ordered_networks(&self) -> Result<Vec<(String, i16)>, BusErr> {
        let mut out = Vec::new();
        if !self.enter(b'a', c"(on)")? {
            return Ok(out);
        }
        while self.enter(b'r', c"on")? {
            let path = self.read_str(b'o')?;
            let signal = self.read_num::<i16>(b'n')?;
            out.push((path, signal));
            self.exit()?;
        }
        self.exit()?;
        Ok(out)
    }
}

fn message_error(api: &SdBusApi, m: *mut SdBusMessage) -> Option<(String, String)> {
    unsafe {
        if (api.sd_bus_message_is_method_error)(m, null()) <= 0 {
            return None;
        }
        let e = (api.sd_bus_message_get_error)(m);
        if e.is_null() {
            return Some(("error".into(), String::new()));
        }
        Some(((*e).name(), (*e).message()))
    }
}

// ======================================================================
// Secrets
// ======================================================================

/// What iwd accepts from the agent (`crypto_passphrase_is_valid`): a
/// passphrase of 8–63 printable ASCII characters. The field takes nothing
/// else, so an impossible value is
/// refused here instead of surfacing as "wrong password" from iwd.
pub const SECRET_MAX_CHARS: usize = 63;
pub const SECRET_MIN_CHARS: usize = 8;
/// Room for the 64 ASCII bytes and then some: the buffer never
/// reallocates, so growth never leaves a stale copy behind.
const SECRET_CAPACITY: usize = 80;

fn wipe_bytes(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn wipe_vec(v: &mut Vec<u8>) {
    wipe_bytes(v.as_mut_slice());
    v.clear();
    for slot in v.spare_capacity_mut() {
        unsafe { std::ptr::write_volatile(slot.as_mut_ptr(), 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// A passphrase: wiped on clear and on drop, never cloned, never printed.
pub struct Secret(String);

impl Secret {
    pub fn new() -> Self {
        Self(String::with_capacity(SECRET_CAPACITY))
    }

    /// Whether the field takes this character at all: printable ASCII.
    pub fn accepts(ch: char) -> bool {
        ch == ' ' || ch.is_ascii_graphic()
    }

    /// Append one typed character; false when it is not a passphrase
    /// character or the field is full.
    pub fn push(&mut self, ch: char) -> bool {
        if !Self::accepts(ch) || self.len_chars() >= SECRET_MAX_CHARS {
            return false;
        }
        self.0.push(ch);
        true
    }

    /// Why iwd would refuse this as a passphrase, or `None` when it is one
    /// (8–63 printable ASCII characters).
    pub fn problem(&self) -> Option<&'static str> {
        let n = self.0.len();
        if n < SECRET_MIN_CHARS {
            Some("At least 8 characters")
        } else {
            None
        }
    }

    pub fn pop(&mut self) {
        if self.0.pop().is_some() {
            // The popped bytes are past `len` now: zero the spare room.
            let v = unsafe { self.0.as_mut_vec() };
            for slot in v.spare_capacity_mut() {
                unsafe { std::ptr::write_volatile(slot.as_mut_ptr(), 0) };
            }
        }
    }

    pub fn len_chars(&self) -> usize {
        self.0.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        wipe_vec(unsafe { self.0.as_mut_vec() });
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

// ======================================================================
// The model the dropdown draws
// ======================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WPA/WPA2/WPA3 personal — iwd's `psk`.
    Psk,
    /// 802.1X — iwd's `8021x`; needs a provisioning file, no prompt here.
    Enterprise,
    Wep,
    Unknown,
}

impl Security {
    fn from_iwd(t: Option<&str>) -> Self {
        match t.unwrap_or("") {
            "open" => Security::Open,
            "psk" => Security::Psk,
            "8021x" => Security::Enterprise,
            "wep" => Security::Wep,
            _ => Security::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Security::Open => "Open",
            Security::Psk => "WPA",
            Security::Enterprise => "802.1X",
            Security::Wep => "WEP",
            Security::Unknown => "?",
        }
    }

    pub fn secured(self) -> bool {
        !matches!(self, Security::Open)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RadioState {
    #[default]
    NoDevice,
    Off,
    On,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum StationState {
    #[default]
    Unknown,
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Roaming,
}

impl StationState {
    fn from_iwd(s: Option<&str>) -> Self {
        match s.unwrap_or("") {
            "disconnected" => StationState::Disconnected,
            "connecting" => StationState::Connecting,
            "connected" => StationState::Connected,
            "disconnecting" => StationState::Disconnecting,
            "roaming" => StationState::Roaming,
            _ => StationState::Unknown,
        }
    }
}

/// One access point row. `path` is iwd's object path — the identifier every
/// command uses, so an SSID with spaces, quotes or non-ASCII never travels
/// through a shell or a match rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub path: String,
    pub name: String,
    pub security: Security,
    pub signal_dbm: Option<i16>,
    /// The `KnownNetwork` object when iwd has credentials for it.
    pub known: Option<String>,
    pub connected: bool,
}

impl WifiNetwork {
    /// 0..4 bars from the dBm reading.
    pub fn bars(&self) -> u8 {
        match self.signal_dbm {
            None => 0,
            Some(d) if d >= -55 => 4,
            Some(d) if d >= -66 => 3,
            Some(d) if d >= -77 => 2,
            Some(_) => 1,
        }
    }
}

/// One of the machine's own IPv4 addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfAddr {
    pub name: String,
    pub wireless: bool,
    pub ipv4: String,
}

impl IfAddr {
    pub fn kind(&self) -> &'static str {
        if self.wireless {
            "Wi-Fi"
        } else {
            "Ethernet"
        }
    }
}

/// Everything the dropdown shows, published whole by the worker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiSnapshot {
    /// Why there is no Wi-Fi at all (no libsystemd, no bus, no iwd) — the
    /// addresses below are still real.
    pub unavailable: Option<String>,
    pub radio: RadioState,
    /// iwd's device name (`wlan0`).
    pub device: Option<String>,
    pub state: StationState,
    pub scanning: bool,
    /// The connected network's object path.
    pub connected: Option<String>,
    pub networks: Vec<WifiNetwork>,
    pub addresses: Vec<IfAddr>,
    /// Our credential agent is registered: password prompts will work.
    pub agent: bool,
}

impl WifiSnapshot {
    pub fn connected_name(&self) -> Option<&str> {
        let path = self.connected.as_deref()?;
        self.networks
            .iter()
            .find(|n| n.path == path)
            .map(|n| n.name.as_str())
    }
}

/// Worker → UI.
#[derive(Debug)]
pub enum WifiEvent {
    Snapshot(WifiSnapshot),
    Connecting { path: String },
    Connected { path: String },
    ConnectFailed { path: String, message: String, wrong_password: bool },
    /// iwd asked for a passphrase we did not have (a saved one went stale,
    /// or the row was joined without typing one).
    PasswordNeeded { path: String },
    /// iwd asked for enterprise credentials, which the dropdown does not do.
    EnterpriseNeeded { path: String },
    ScanDone { error: Option<String> },
    Notice { text: String, error: bool },
}

/// The dropdown's intent — what a press means. No secret inside: the WM
/// pulls the typed passphrase out of the panel when it turns a
/// `ConnectWithPassphrase` into a [`WorkerCommand`], so the widget-action
/// path never carries or clones it.
#[derive(Clone, Debug, PartialEq)]
pub enum WifiCommand {
    Scan,
    Refresh,
    RefreshAddresses,
    Connect { path: String },
    ConnectWithPassphrase { path: String },
    Disconnect,
    Forget { known: String },
    SetPowered(bool),
    /// Abandon the connect in flight: its passphrase is dropped, iwd is
    /// told to stop, and its later outcome is not reported.
    CancelConnect,
}

/// UI → worker, over the bounded channel.
#[derive(Debug)]
pub enum WorkerCommand {
    Scan,
    Refresh,
    RefreshAddresses,
    Connect { path: String, secret: Option<Secret> },
    Disconnect,
    Forget { known: String },
    SetPowered(bool),
    CancelConnect,
}

impl WorkerCommand {
    /// The refreshes: harmless to run once instead of twice.
    fn idempotent_kind(&self) -> Option<u8> {
        match self {
            WorkerCommand::Scan => Some(0),
            WorkerCommand::Refresh => Some(1),
            WorkerCommand::RefreshAddresses => Some(2),
            _ => None,
        }
    }
}

// ======================================================================
// The UI end
// ======================================================================

/// Commands that wait for room in the channel, at most.
const WAITING_MAX: usize = 8;

/// The dropdown's handle on the worker. Dropping it ends the worker.
pub struct WifiLink {
    stop: Arc<AtomicBool>,
    tx: Option<SyncSender<WorkerCommand>>,
    rx: Receiver<WifiEvent>,
    wake_fd: c_int,
    worker: Option<TaskHandle<()>>,
    /// Commands the full channel refused, in the order they were meant:
    /// nothing sent later overtakes them (a Cancel stays ahead of the
    /// Refresh that follows it). Drained by [`Self::retry`].
    waiting: VecDeque<WorkerCommand>,
}

impl WifiLink {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let mut fds: [c_int; 2] = [-1, -1];
        if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) } != 0 {
            return Err("could not create the worker's wake pipe".into());
        }
        let (tx, command_rx) = sync_channel::<WorkerCommand>(COMMAND_QUEUE);
        let (event_tx, rx) = channel::<WifiEvent>();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let wake_read = fds[0];
        let worker = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("wm-wifi".into()),
                    ..Default::default()
                },
                move || run_worker(command_rx, event_tx, wake_read, worker_stop),
            )
            .map_err(|e| {
                unsafe {
                    close(fds[0]);
                    close(fds[1]);
                }
                e.to_string()
            })?;
        Ok(Self {
            stop,
            tx: Some(tx),
            rx,
            wake_fd: fds[1],
            worker: Some(worker),
            waiting: VecDeque::new(),
        })
    }

    fn wake(&self) {
        let byte = 1u8;
        unsafe { write(self.wake_fd, &byte as *const u8 as *const c_void, 1) };
    }

    /// Non-blocking. While commands are waiting for room, a new one queues
    /// behind them so the order of intents holds; a refresh already
    /// waiting absorbs another; a Cancel drops the Connects still waiting
    /// (their passphrases with them). False when the command was refused
    /// (the waiting line is full) or the worker is gone.
    pub fn send(&mut self, cmd: WorkerCommand) -> bool {
        if self.tx.is_none() {
            return false;
        }
        if !self.waiting.is_empty() {
            return self.enqueue(cmd);
        }
        match self.tx.as_ref().unwrap().try_send(cmd) {
            Ok(()) => {
                self.wake();
                true
            }
            Err(TrySendError::Full(cmd)) => self.enqueue(cmd),
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn enqueue(&mut self, cmd: WorkerCommand) -> bool {
        if let Some(kind) = cmd.idempotent_kind() {
            if self.waiting.iter().any(|w| w.idempotent_kind() == Some(kind)) {
                return true;
            }
        }
        if matches!(cmd, WorkerCommand::CancelConnect) {
            self.waiting
                .retain(|w| !matches!(w, WorkerCommand::Connect { .. } | WorkerCommand::CancelConnect));
            // Reserve room for cancellation even when ordinary commands
            // filled the queue. There can be only one retained cancel.
            self.waiting.push_back(cmd);
            return true;
        }
        if self.waiting.len() >= WAITING_MAX {
            log!("wm: wifi: too many commands waiting, one refused");
            return false;
        }
        self.waiting.push_back(cmd);
        true
    }

    /// Push waiting commands into the channel, in order, as far as it has
    /// room. True when nothing is left waiting.
    pub fn retry(&mut self) -> bool {
        let Some(tx) = self.tx.as_ref() else {
            return false;
        };
        while let Some(cmd) = self.waiting.pop_front() {
            match tx.try_send(cmd) {
                Ok(()) => self.wake(),
                Err(TrySendError::Full(cmd)) => {
                    self.waiting.push_front(cmd);
                    return false;
                }
                Err(TrySendError::Disconnected(_)) => return false,
            }
        }
        true
    }

    pub fn has_pending(&self) -> bool {
        !self.waiting.is_empty()
    }

    pub fn try_recv(&self) -> Option<WifiEvent> {
        self.rx.try_recv().ok()
    }

    /// The worker's terminal report, once, if it ended on its own.
    pub fn worker_stopped(&mut self) -> Option<String> {
        let worker = self.worker.as_mut()?;
        if !worker.is_finished() {
            return None;
        }
        let result = worker.try_take()?;
        self.worker = None;
        Some(match result {
            Ok(()) => "Wi-Fi worker exited".into(),
            Err(e) => format!("Wi-Fi worker stopped: {e}"),
        })
    }
}

impl Drop for WifiLink {
    fn drop(&mut self) {
        // Buffered commands must not survive the UI handle. Check this
        // flag before dispatch as well as closing and waking the channel.
        self.stop.store(true, Ordering::Release);
        self.waiting.clear();
        self.tx = None;
        self.wake();
        unsafe { close(self.wake_fd) };
        // The worker unregisters its agent and closes the bus on the way
        // out; nobody waits for that here.
        self.worker.take();
    }
}

// ======================================================================
// The worker
// ======================================================================

/// State the sd-bus callbacks touch. Single-threaded: callbacks only run
/// inside `sd_bus_process`, on the worker, so `Cell`/`RefCell` suffice.
struct Shared {
    stop: Arc<AtomicBool>,
    api: SdBusApi,
    tx: Sender<WifiEvent>,
    /// The unique bus name that owns `net.connman.iwd` right now — the only
    /// caller the credential agent answers.
    iwd_owner: RefCell<Option<String>>,
    /// The connect in flight and the passphrase it may hand to iwd.
    pending: RefCell<Option<Pending>>,
    /// The newest connect attempt's generation (see [`Pending::gen`]).
    connect_gen: Cell<u32>,
    /// An iwd signal arrived: re-read the state (debounced).
    dirty: Cell<bool>,
    owner_changed: Cell<bool>,
    agent_released: Cell<bool>,
    /// Async replies collected for the loop to act on.
    done: RefCell<Vec<AsyncDone>>,
}

/// The connect attempt whose passphrase the agent may hand out. `gen` is
/// the attempt's generation: a newer attempt or a Cancel bumps the
/// worker's, and the agent only answers for the current one.
struct Pending {
    gen: u32,
    path: String,
    secret: Option<Secret>,
}

enum AsyncKind {
    Connect { gen: u32, path: String, had_secret: bool },
    Scan,
    Disconnect,
}

/// A reply the callback recorded, by the call's id; the loop owns the
/// rest of the call's state ([`InFlight`]).
struct AsyncDone {
    id: u64,
    result: Result<(), (String, String)>,
}

/// What the reply callback is given: enough to file the result. Owned by
/// the loop's [`InFlight`] entry, freed with it.
struct AsyncCtx {
    shared: *const Shared,
    id: u64,
}

/// An async call the loop is waiting on. The slot is ours (not floating):
/// unref'd once the reply is filed, or at shutdown — which cancels the
/// call, so the callback never runs after the context is freed.
struct InFlight {
    id: u64,
    slot: *mut SdBusSlot,
    ctx: *mut AsyncCtx,
    kind: AsyncKind,
}

fn notify(tx: &Sender<WifiEvent>, ev: WifiEvent) {
    let _ = tx.send(ev);
    SignalToUI::set_ui_signal();
}

unsafe extern "C" fn async_reply_cb(
    m: *mut SdBusMessage,
    userdata: *mut c_void,
    _ret_error: *mut SdBusError,
) -> c_int {
    let ctx = &*(userdata as *const AsyncCtx);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let shared = &*ctx.shared;
        let result = match message_error(&shared.api, m) {
            Some(e) => Err(e),
            None => Ok(()),
        };
        shared.done.borrow_mut().push(AsyncDone { id: ctx.id, result });
    }));
    1
}

unsafe extern "C" fn signal_cb(
    m: *mut SdBusMessage,
    userdata: *mut c_void,
    _ret_error: *mut SdBusError,
) -> c_int {
    let shared = &*(userdata as *const Shared);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let api = &shared.api;
        let iface = cstr_at((api.sd_bus_message_get_interface)(m));
        let member = cstr_at((api.sd_bus_message_get_member)(m));
        if iface == "org.freedesktop.DBus" && member == "NameOwnerChanged" {
            let msg = Msg::borrowed(*api, m);
            let name = msg.read_str(b's').unwrap_or_default();
            let _old = msg.read_str(b's').unwrap_or_default();
            let new = msg.read_str(b's').unwrap_or_default();
            if name == "net.connman.iwd" {
                *shared.iwd_owner.borrow_mut() = if new.is_empty() { None } else { Some(new) };
                // Whoever comes up next is not the daemon we were talking to.
                shared.pending.borrow_mut().take();
                shared.owner_changed.set(true);
            }
        } else {
            shared.dirty.set(true);
        }
    }));
    0
}

unsafe extern "C" fn agent_cb(
    m: *mut SdBusMessage,
    userdata: *mut c_void,
    _ret_error: *mut SdBusError,
) -> c_int {
    let shared = &*(userdata as *const Shared);
    catch_unwind(AssertUnwindSafe(|| agent_call(shared, m))).unwrap_or(-22)
}

/// The `net.connman.iwd.Agent` object (iwd `doc/agent-api.txt`):
/// `RequestPassphrase(o) -> s`, `RequestPrivateKeyPassphrase(o) -> s`,
/// `RequestUserNameAndPassword(o) -> (ss)`, `RequestUserPassword(os) -> s`,
/// `Cancel(s)`, `Release()`. Only the current owner of `net.connman.iwd`
/// is answered, and a passphrase only for the network we are joining.
fn agent_call(shared: &Shared, m: *mut SdBusMessage) -> c_int {
    let api = &shared.api;
    let iface = cstr_at(unsafe { (api.sd_bus_message_get_interface)(m) });
    if iface != IFACE_AGENT {
        return 0;
    }
    if shared.stop.load(Ordering::Acquire) {
        reply_error(api, m, AGENT_CANCELED, c"Wi-Fi controls are closing");
        return 1;
    }
    let member = cstr_at(unsafe { (api.sd_bus_message_get_member)(m) });
    let sender = cstr_at(unsafe { (api.sd_bus_message_get_sender)(m) });
    let owner = shared.iwd_owner.borrow().clone();
    if sender.is_empty() || owner.as_deref() != Some(sender.as_str()) {
        log!(
            "wm: wifi: agent call {member} from {sender} refused (iwd is {})",
            owner.as_deref().unwrap_or("not on the bus")
        );
        reply_error(api, m, AGENT_CANCELED, c"caller is not the Wi-Fi daemon");
        return 1;
    }
    let msg = Msg::borrowed(*api, m);
    match member.as_str() {
        "RequestPassphrase" => {
            let path = msg.read_str(b'o').unwrap_or_default();
            // Only the attempt in progress, only for its own network: a
            // request for anything else (a stale attempt iwd is still
            // finishing, a different network) gets no passphrase.
            let (current, handed) = {
                let mut pending = shared.pending.borrow_mut();
                match pending.as_mut() {
                    Some(p) if p.path == path && p.gen == shared.connect_gen.get() => {
                        (true, p.secret.take())
                    }
                    _ => (false, None),
                }
            };
            match handed {
                Some(secret) => {
                    reply_passphrase(api, m, &secret);
                    drop(secret);
                }
                None => {
                    reply_error(api, m, AGENT_CANCELED, c"no passphrase for this network");
                    if current { notify(&shared.tx, WifiEvent::PasswordNeeded { path }); }
                }
            }
            1
        }
        "RequestPrivateKeyPassphrase" | "RequestUserNameAndPassword" | "RequestUserPassword" => {
            let path = msg.read_str(b'o').unwrap_or_default();
            reply_error(
                api,
                m,
                AGENT_CANCELED,
                c"enterprise credentials are not entered here",
            );
            if shared.pending.borrow().as_ref().is_some_and(|p| p.path == path && p.gen == shared.connect_gen.get()) {
                notify(&shared.tx, WifiEvent::EnterpriseNeeded { path });
            }
            1
        }
        "Cancel" => {
            let reason = msg.read_str(b's').unwrap_or_default();
            shared.pending.borrow_mut().take();
            reply_empty(api, m);
            log!("wm: wifi: iwd cancelled the credential request ({reason})");
            1
        }
        "Release" => {
            shared.pending.borrow_mut().take();
            shared.agent_released.set(true);
            reply_empty(api, m);
            1
        }
        _ => 0,
    }
}

fn reply_error(api: &SdBusApi, call: *mut SdBusMessage, name: &'static CStr, message: &'static CStr) {
    let e = SdBusError::constant(name, message);
    unsafe { (api.sd_bus_reply_method_error)(call, &e) };
}

fn reply_empty(api: &SdBusApi, call: *mut SdBusMessage) {
    let mut reply: *mut SdBusMessage = null_mut();
    unsafe {
        if (api.sd_bus_message_new_method_return)(call, &mut reply) < 0 {
            return;
        }
        (api.sd_bus_send)(null_mut(), reply, null_mut());
        (api.sd_bus_message_unref)(reply);
    }
}

/// The one place a passphrase leaves this process: into the reply to iwd.
/// The NUL-terminated copy the C call needs is wiped right after.
fn reply_passphrase(api: &SdBusApi, call: *mut SdBusMessage, secret: &Secret) {
    let mut reply: *mut SdBusMessage = null_mut();
    unsafe {
        if (api.sd_bus_message_new_method_return)(call, &mut reply) < 0 {
            return;
        }
    }
    let bytes = secret.as_bytes();
    let mut buf: Vec<u8> = Vec::with_capacity(bytes.len() + 1);
    buf.extend_from_slice(bytes);
    buf.push(0);
    unsafe {
        let r = (api.sd_bus_message_append_basic)(reply, b's' as c_char, buf.as_ptr() as *const c_void);
        if r >= 0 {
            (api.sd_bus_send)(null_mut(), reply, null_mut());
        }
        (api.sd_bus_message_unref)(reply);
    }
    wipe_vec(&mut buf);
}

struct Worker {
    api: SdBusApi,
    bus: *mut SdBus,
    bus_fd: c_int,
    shared: Box<Shared>,
    rx: Receiver<WorkerCommand>,
    tx: Sender<WifiEvent>,
    wake_fd: c_int,
    /// iwd's device object (its `Station` interface lives on the same path).
    device: Option<String>,
    agent_registered: bool,
    refresh_due: Option<f64>,
    address_followups: Vec<f64>,
    snapshot: WifiSnapshot,
    /// Async calls awaiting their reply.
    in_flight: Vec<InFlight>,
    next_async_id: u64,
    /// The generation whose outcome the UI is waiting for, if any. A reply
    /// for any other generation is stale: superseded or cancelled, and not
    /// reported (the generation counter itself is `Shared::connect_gen`).
    active_connect: Option<u32>,
    /// A connect asked for while another was still in iwd's hands: starts
    /// once that one has been aborted and has answered.
    queued_connect: Option<(String, Option<Secret>)>,
}

/// Load sd-bus, open the system bus, install the signal matches and the
/// agent object.
fn open_bus(tx: &Sender<WifiEvent>, stop: Arc<AtomicBool>) -> Result<(SdBusApi, *mut SdBus, Box<Shared>), String> {
    let lib = unsafe { dlopen(c"libsystemd.so.0".as_ptr(), RTLD_NOW) };
    if lib.is_null() {
        return Err("libsystemd.so.0 is not installed (sd-bus is how the WM talks to iwd)".into());
    }
    let api = unsafe { SdBusApi::load(lib) }?;
    let mut bus: *mut SdBus = null_mut();
    let r = unsafe { (api.sd_bus_open_system)(&mut bus) };
    if r < 0 {
        return Err(format!("cannot open the system D-Bus: {}", errno_text(-r)));
    }
    let shared = Box::new(Shared {
        stop,
        api,
        tx: tx.clone(),
        iwd_owner: RefCell::new(None),
        pending: RefCell::new(None),
        connect_gen: Cell::new(0),
        dirty: Cell::new(false),
        owner_changed: Cell::new(false),
        agent_released: Cell::new(false),
        done: RefCell::new(Vec::new()),
    });
    let userdata = &*shared as *const Shared as *mut c_void;
    let rules: [&CStr; 2] = [
        c"type='signal',sender='net.connman.iwd'",
        c"type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='net.connman.iwd'",
    ];
    for rule in rules {
        let r = unsafe { (api.sd_bus_add_match)(bus, null_mut(), rule.as_ptr(), signal_cb, userdata) };
        if r < 0 {
            unsafe { (api.sd_bus_flush_close_unref)(bus) };
            return Err(format!("cannot watch iwd on the bus: {}", errno_text(-r)));
        }
    }
    let r = unsafe { (api.sd_bus_add_object)(bus, null_mut(), AGENT_PATH.as_ptr(), agent_cb, userdata) };
    if r < 0 {
        unsafe { (api.sd_bus_flush_close_unref)(bus) };
        return Err(format!("cannot install the credential agent: {}", errno_text(-r)));
    }
    Ok((api, bus, shared))
}

/// The worker's whole life. Exits when the [`WifiLink`] is dropped.
pub(crate) fn run_worker(rx: Receiver<WorkerCommand>, tx: Sender<WifiEvent>, wake_fd: c_int, stop: Arc<AtomicBool>) {
    let outcome = match open_bus(&tx, stop.clone()) {
        Ok((api, bus, shared)) => {
            let bus_fd = unsafe { (api.sd_bus_get_fd)(bus) };
            let mut worker = Worker {
                api,
                bus,
                bus_fd,
                shared,
                rx,
                tx,
                wake_fd,
                device: None,
                agent_registered: false,
                refresh_due: None,
                address_followups: Vec::new(),
                snapshot: WifiSnapshot::default(),
                in_flight: Vec::new(),
                next_async_id: 1,
                active_connect: None,
                queued_connect: None,
            };
            let result = worker.run();
            worker.shutdown();
            let Worker { rx, tx, .. } = worker;
            result.map_err(|reason| (reason, rx, tx))
        }
        Err(reason) => Err((reason, rx, tx)),
    };
    if let Err((reason, rx, tx)) = outcome {
        log!("wm: wifi: {reason}");
        run_without_bus(rx, tx, reason, &stop);
    }
    unsafe { close(wake_fd) };
}

/// No sd-bus / no bus / bus lost: the addresses are still real, everything
/// Wi-Fi answers with the reason.
fn run_without_bus(rx: Receiver<WorkerCommand>, tx: Sender<WifiEvent>, reason: String, stop: &AtomicBool) {
    let publish = |tx: &Sender<WifiEvent>| {
        notify(
            tx,
            WifiEvent::Snapshot(WifiSnapshot {
                unavailable: Some(reason.clone()),
                addresses: sample_addresses(None),
                ..Default::default()
            }),
        );
    };
    publish(&tx);
    while !stop.load(Ordering::Acquire) {
        match rx.recv() {
            Ok(WorkerCommand::Refresh) | Ok(WorkerCommand::RefreshAddresses) => publish(&tx),
            Ok(WorkerCommand::CancelConnect) => {}
            Ok(_) => notify(
                &tx,
                WifiEvent::Notice {
                    text: reason.clone(),
                    error: true,
                },
            ),
            Err(_) => break,
        }
    }
}

impl Worker {
    fn send(&self, ev: WifiEvent) {
        notify(&self.tx, ev);
    }

    fn notice(&self, text: impl Into<String>, error: bool) {
        self.send(WifiEvent::Notice {
            text: text.into(),
            error,
        });
    }

    // ------------------------------------------------------------- calls

    fn method(&self, dest: &CStr, path: &CStr, iface: &CStr, member: &CStr) -> Result<Msg, BusErr> {
        let mut m: *mut SdBusMessage = null_mut();
        check(unsafe {
            (self.api.sd_bus_message_new_method_call)(
                self.bus,
                &mut m,
                dest.as_ptr(),
                path.as_ptr(),
                iface.as_ptr(),
                member.as_ptr(),
            )
        })?;
        Ok(Msg {
            api: self.api,
            m,
            owned: true,
        })
    }

    fn iwd_method(&self, path: &str, iface: &CStr, member: &CStr) -> Result<Msg, BusErr> {
        let path = cstring(path)?;
        self.method(IWD_NAME, &path, iface, member)
    }

    /// A blocking call — on the worker only, bounded by `timeout_us`.
    fn call(&self, msg: Msg, timeout_us: u64) -> Result<Msg, BusErr> {
        let mut err = SdBusError::null();
        let mut reply: *mut SdBusMessage = null_mut();
        let r = unsafe { (self.api.sd_bus_call)(self.bus, msg.m, timeout_us, &mut err, &mut reply) };
        if r < 0 {
            let e = if err.is_set() {
                BusErr::Dbus {
                    name: err.name(),
                    message: err.message(),
                }
            } else {
                BusErr::Sys(-r)
            };
            unsafe { (self.api.sd_bus_error_free)(&mut err) };
            return Err(e);
        }
        Ok(Msg {
            api: self.api,
            m: reply,
            owned: true,
        })
    }

    /// A call whose reply lands in `Shared::done` — for anything that
    /// takes long (a connect waits on the handshake and, through the
    /// agent, on this same loop). The slot and the callback's context are
    /// owned here ([`InFlight`]) until the reply is filed or we shut down.
    fn call_async(&mut self, msg: Msg, kind: AsyncKind, timeout_us: u64) -> Result<(), BusErr> {
        let id = self.next_async_id;
        self.next_async_id += 1;
        let ctx = Box::into_raw(Box::new(AsyncCtx {
            shared: &*self.shared as *const Shared,
            id,
        }));
        let mut slot: *mut SdBusSlot = null_mut();
        let r = unsafe {
            (self.api.sd_bus_call_async)(
                self.bus,
                &mut slot,
                msg.m,
                async_reply_cb,
                ctx as *mut c_void,
                timeout_us,
            )
        };
        if r < 0 {
            drop(unsafe { Box::from_raw(ctx) });
            return Err(BusErr::Sys(-r));
        }
        self.in_flight.push(InFlight {
            id,
            slot,
            ctx,
            kind,
        });
        Ok(())
    }

    /// Release a finished (or abandoned) async call's slot and context.
    /// Unref'ing the slot cancels a call still pending, so the callback
    /// cannot run on a freed context afterwards.
    fn release_flight(&mut self, flight: InFlight) -> AsyncKind {
        unsafe {
            (self.api.sd_bus_slot_unref)(flight.slot);
            drop(Box::from_raw(flight.ctx));
        }
        flight.kind
    }

    fn connect_in_flight(&self) -> bool {
        self.in_flight
            .iter()
            .any(|f| matches!(f.kind, AsyncKind::Connect { .. }))
    }

    // -------------------------------------------------------------- loop

    fn run(&mut self) -> Result<(), String> {
        self.resolve_owner();
        self.register_agent();
        self.refresh();
        loop {
            if self.shared.stop.load(Ordering::Acquire) { return Ok(()); }
            self.pump()?;
            if self.shared.stop.load(Ordering::Acquire) { return Ok(()); }
            self.handle_done();
            if self.shared.owner_changed.replace(false) {
                self.on_owner_changed();
            }
            if self.shared.agent_released.replace(false) {
                self.agent_registered = false;
                self.register_agent();
            }
            if self.shared.dirty.replace(false) {
                self.schedule_refresh(REFRESH_DEBOUNCE_SECS);
            }
            let now = now_secs();
            if self.refresh_due.is_some_and(|t| now >= t) {
                self.refresh();
            }
            if self.address_followups.first().is_some_and(|t| now >= *t) {
                self.address_followups.remove(0);
                self.refresh_addresses();
            }
            match self.rx.try_recv() {
                Ok(cmd) => {
                    if self.shared.stop.load(Ordering::Acquire) { return Ok(()); }
                    self.handle(cmd);
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
            self.wait();
        }
    }

    /// Dispatch every queued bus message (signals, agent calls, replies).
    fn pump(&mut self) -> Result<(), String> {
        loop {
            let r = unsafe { (self.api.sd_bus_process)(self.bus, null_mut()) };
            if r > 0 {
                continue;
            }
            if r < 0 {
                return Err(format!("system bus connection lost: {}", errno_text(-r)));
            }
            return Ok(());
        }
    }

    /// Sleep until the bus needs attention, a deadline passes or the UI
    /// wakes us.
    fn wait(&mut self) {
        let mut bus_events = unsafe { (self.api.sd_bus_get_events)(self.bus) };
        if bus_events < 0 {
            bus_events = POLLIN as c_int;
        }
        let mut timeout_ms: c_int = -1;
        let mut usec: u64 = u64::MAX;
        if unsafe { (self.api.sd_bus_get_timeout)(self.bus, &mut usec) } >= 0 && usec != u64::MAX {
            timeout_ms = ((usec.saturating_sub(now_usec()) + 999) / 1000).min(i32::MAX as u64) as c_int;
        }
        let now = now_secs();
        for deadline in self
            .refresh_due
            .into_iter()
            .chain(self.address_followups.first().copied())
        {
            let ms = ((deadline - now).max(0.0) * 1000.0).ceil() as c_int;
            timeout_ms = if timeout_ms < 0 { ms } else { timeout_ms.min(ms) };
        }
        let mut fds = [
            PollFd {
                fd: self.bus_fd,
                events: bus_events as c_short,
                revents: 0,
            },
            PollFd {
                fd: self.wake_fd,
                events: POLLIN,
                revents: 0,
            },
        ];
        unsafe { poll(fds.as_mut_ptr(), 2, timeout_ms) };
        let mut sink = [0u8; 64];
        while unsafe { read(self.wake_fd, sink.as_mut_ptr() as *mut c_void, sink.len()) } > 0 {}
    }

    fn shutdown(&mut self) {
        self.shared.pending.borrow_mut().take();
        self.queued_connect = None;
        self.active_connect = None;
        // Cancel what is still pending before the bus goes: each unref
        // drops the callback registration, then the context is freed.
        for flight in std::mem::take(&mut self.in_flight) {
            self.release_flight(flight);
        }
        if self.agent_registered {
            let _ = self
                .method(IWD_NAME, IWD_ROOT, IFACE_AGENT_MANAGER, c"UnregisterAgent")
                .and_then(|m| {
                    m.append_str(b'o', AGENT_PATH)?;
                    self.call(m, 1_000_000)
                });
            self.agent_registered = false;
        }
        if !self.bus.is_null() {
            unsafe { (self.api.sd_bus_flush_close_unref)(self.bus) };
            self.bus = null_mut();
        }
    }

    fn schedule_refresh(&mut self, in_secs: f64) {
        let at = now_secs() + in_secs;
        self.refresh_due = Some(self.refresh_due.map_or(at, |t| t.min(at)));
    }

    // ---------------------------------------------------------- results

    fn handle_done(&mut self) {
        let done = std::mem::take(&mut *self.shared.done.borrow_mut());
        for d in done {
            let Some(pos) = self.in_flight.iter().position(|f| f.id == d.id) else {
                continue;
            };
            let flight = self.in_flight.remove(pos);
            let kind = self.release_flight(flight);
            match kind {
                AsyncKind::Connect {
                    gen,
                    path,
                    had_secret,
                } => {
                    if self.active_connect != Some(gen) {
                        // Superseded or cancelled: its outcome is nobody's
                        // news, and its passphrase (if any) is long gone.
                        log!("wm: wifi: connect attempt {gen} ended after being abandoned");
                    } else {
                        self.active_connect = None;
                        // Whatever happened, the passphrase's job is over.
                        self.shared.pending.borrow_mut().take();
                        match d.result {
                            Ok(()) => {
                                self.send(WifiEvent::Connected { path });
                                let now = now_secs();
                                self.address_followups =
                                    ADDRESS_FOLLOWUPS_SECS.iter().map(|s| now + s).collect();
                            }
                            Err((name, message)) => {
                                let e = BusErr::Dbus { name, message };
                                let (message, wrong_password) =
                                    friendly_connect_error(&e, had_secret);
                                self.send(WifiEvent::ConnectFailed {
                                    path,
                                    message,
                                    wrong_password,
                                });
                            }
                        }
                    }
                    self.schedule_refresh(0.0);
                    self.start_queued_connect();
                }
                AsyncKind::Scan => {
                    if let Err((name, message)) = d.result {
                        let e = BusErr::Dbus { name, message };
                        // "InProgress": iwd was already scanning — fine.
                        if e.suffix() != "InProgress" {
                            self.send(WifiEvent::ScanDone {
                                error: Some(format!("Scan failed: {e}")),
                            });
                        }
                    }
                }
                AsyncKind::Disconnect => {
                    if let Err((name, message)) = d.result {
                        let e = BusErr::Dbus { name, message };
                        if e.suffix() != "NotConnected" {
                            self.notice(format!("Disconnect failed: {e}"), true);
                        }
                    }
                    self.schedule_refresh(0.0);
                    self.start_queued_connect();
                }
            }
        }
    }

    /// The attempt waiting behind an aborted one starts once iwd has
    /// answered the old one (a second `Connect` while one runs is refused
    /// as busy).
    fn start_queued_connect(&mut self) {
        if self.connect_in_flight() {
            return;
        }
        if let Some((path, secret)) = self.queued_connect.take() {
            self.start_connect(path, secret);
        }
    }

    fn on_owner_changed(&mut self) {
        self.agent_registered = false;
        if self.shared.iwd_owner.borrow().is_some() {
            self.register_agent();
        }
        self.schedule_refresh(0.5);
    }

    // ------------------------------------------------------------- iwd

    fn resolve_owner(&mut self) {
        let owner = self
            .method(DBUS_NAME, DBUS_PATH, IFACE_DBUS, c"GetNameOwner")
            .and_then(|m| {
                m.append_str(b's', IWD_NAME)?;
                self.call(m, CALL_TIMEOUT_US)
            })
            .and_then(|reply| reply.read_str(b's'))
            .ok();
        *self.shared.iwd_owner.borrow_mut() = owner;
    }

    fn register_agent(&mut self) {
        if self.agent_registered || self.shared.iwd_owner.borrow().is_none() {
            return;
        }
        let r = self
            .method(IWD_NAME, IWD_ROOT, IFACE_AGENT_MANAGER, c"RegisterAgent")
            .and_then(|m| {
                m.append_str(b'o', AGENT_PATH)?;
                self.call(m, CALL_TIMEOUT_US)
            });
        match r {
            Ok(_) => self.agent_registered = true,
            // Our connection already has one (a re-register after a hiccup).
            Err(e) if e.suffix() == "AlreadyExists" => self.agent_registered = true,
            Err(e) => log!("wm: wifi: iwd did not accept the credential agent: {e}"),
        }
    }

    fn refresh(&mut self) {
        self.refresh_due = None;
        let was_scanning = self.snapshot.scanning;
        let was_state = self.snapshot.state.clone();
        match self.read_iwd() {
            Ok(snapshot) => self.snapshot = snapshot,
            Err(e) => {
                let reason = match e.suffix() {
                    "ServiceUnknown" | "NameHasNoOwner" => "iwd is not running".to_string(),
                    _ => format!("iwd is unreachable: {e}"),
                };
                self.device = None;
                self.snapshot = WifiSnapshot {
                    unavailable: Some(reason),
                    addresses: sample_addresses(None),
                    ..Default::default()
                };
            }
        }
        self.snapshot.agent = self.agent_registered;
        if was_scanning && !self.snapshot.scanning {
            self.send(WifiEvent::ScanDone { error: None });
        }
        if self.snapshot.state == StationState::Connected && was_state != StationState::Connected {
            let now = now_secs();
            self.address_followups = ADDRESS_FOLLOWUPS_SECS.iter().map(|s| now + s).collect();
        }
        self.send(WifiEvent::Snapshot(self.snapshot.clone()));
    }

    fn refresh_addresses(&mut self) {
        self.snapshot.addresses = sample_addresses(self.snapshot.device.as_deref());
        self.send(WifiEvent::Snapshot(self.snapshot.clone()));
    }

    /// One `GetManagedObjects` round trip (every iwd object with all its
    /// properties) plus `GetOrderedNetworks` for the ranking and signal.
    fn read_iwd(&mut self) -> Result<WifiSnapshot, BusErr> {
        let reply = self
            .method(IWD_NAME, ROOT_PATH, IFACE_OBJECT_MANAGER, c"GetManagedObjects")
            .and_then(|m| self.call(m, CALL_TIMEOUT_US))?;
        let objects = reply.read_managed_objects()?;
        let mut snap = WifiSnapshot::default();

        // The Wi-Fi device: the one with a Station interface if any, else
        // the first Device (powered off, or not in station mode).
        let device = objects
            .iter()
            .filter(|o| o.iface(IFACE_DEVICE).is_some())
            .max_by_key(|o| o.iface(IFACE_STATION).is_some());
        let Some(dev) = device else {
            self.device = None;
            snap.addresses = sample_addresses(None);
            return Ok(snap);
        };
        let dev_props = dev.iface(IFACE_DEVICE).cloned().unwrap_or_default();
        snap.device = prop_str(&dev_props, "Name");
        let powered = prop_bool(&dev_props, "Powered").unwrap_or(false);
        let adapter_powered = prop_str(&dev_props, "Adapter")
            .and_then(|ap| objects.iter().find(|o| o.path == ap))
            .and_then(|a| a.iface(c"net.connman.iwd.Adapter").and_then(|p| prop_bool(p, "Powered")))
            .unwrap_or(true);
        snap.radio = if powered && adapter_powered {
            RadioState::On
        } else {
            RadioState::Off
        };
        let dev_path = dev.path.clone();
        self.device = Some(dev_path.clone());

        if let Some(st) = dev.iface(IFACE_STATION) {
            snap.state = StationState::from_iwd(prop_str(st, "State").as_deref());
            snap.scanning = prop_bool(st, "Scanning").unwrap_or(false);
            snap.connected = prop_str(st, "ConnectedNetwork");
            let ordered = match self
                .iwd_method(&dev_path, IFACE_STATION, c"GetOrderedNetworks")
                .and_then(|m| self.call(m, CALL_TIMEOUT_US))
                .and_then(|r| r.read_ordered_networks())
            {
                Ok(list) => list,
                Err(e) => {
                    log!("wm: wifi: GetOrderedNetworks failed: {e}");
                    Vec::new()
                }
            };
            let network_of = |o: &DbusObject, signal: Option<i16>| -> Option<WifiNetwork> {
                let n = o.iface(IFACE_NETWORK)?;
                if prop_str(n, "Device").as_deref() != Some(dev_path.as_str()) {
                    return None;
                }
                Some(WifiNetwork {
                    path: o.path.clone(),
                    name: prop_str(n, "Name").unwrap_or_default(),
                    security: Security::from_iwd(prop_str(n, "Type").as_deref()),
                    signal_dbm: signal,
                    known: prop_str(n, "KnownNetwork"),
                    connected: prop_bool(n, "Connected").unwrap_or(false),
                })
            };
            let mut networks: Vec<WifiNetwork> = Vec::new();
            for (path, signal) in &ordered {
                if let Some(o) = objects.iter().find(|o| &o.path == path) {
                    if let Some(n) = network_of(o, Some(signal / 100)) {
                        networks.push(n);
                    }
                }
            }
            // A network object the ranking did not name (the connected one
            // while a scan is running) keeps its row, unranked.
            for o in &objects {
                if networks.iter().any(|n| n.path == o.path) {
                    continue;
                }
                if let Some(n) = network_of(o, None) {
                    networks.push(n);
                }
            }
            // The connected network heads the list, like every OS menu.
            networks.sort_by_key(|n| !n.connected);
            snap.networks = networks;
        }
        snap.addresses = sample_addresses(snap.device.as_deref());
        Ok(snap)
    }

    // --------------------------------------------------------- commands

    fn handle(&mut self, cmd: WorkerCommand) {
        match cmd {
            WorkerCommand::Scan => self.scan(),
            WorkerCommand::Refresh => self.refresh(),
            WorkerCommand::RefreshAddresses => self.refresh_addresses(),
            WorkerCommand::Connect { path, secret } => self.connect(path, secret),
            WorkerCommand::Disconnect => self.disconnect(),
            WorkerCommand::Forget { known } => self.forget(&known),
            WorkerCommand::SetPowered(on) => self.set_powered(on),
            WorkerCommand::CancelConnect => self.cancel_connect(),
        }
    }

    fn scan(&mut self) {
        // iwd removes the Station interface while the radio is off. Opening
        // the dropdown still refreshes status, but must not call Scan on it.
        if self.snapshot.radio == RadioState::Off {
            self.send(WifiEvent::ScanDone { error: None });
            return;
        }
        let Some(dev) = self.device.clone() else {
            self.send(WifiEvent::ScanDone {
                error: Some("No Wi-Fi device to scan with".into()),
            });
            return;
        };
        if let Err(e) = self
            .iwd_method(&dev, IFACE_STATION, c"Scan")
            .and_then(|m| self.call_async(m, AsyncKind::Scan, CALL_TIMEOUT_US))
        {
            self.send(WifiEvent::ScanDone {
                error: Some(format!("Scan failed: {e}")),
            });
        }
    }

    /// A connect request. iwd runs one attempt at a time, so a request
    /// while another is in its hands abandons that one (its outcome will
    /// not be reported, its passphrase is dropped), tells iwd to stop, and
    /// starts this one once the old attempt has answered.
    fn connect(&mut self, path: String, secret: Option<Secret>) {
        if self.connect_in_flight() {
            self.abandon_connect();
            self.queued_connect = Some((path, secret));
            self.station_disconnect(true);
            return;
        }
        self.start_connect(path, secret);
    }

    fn next_generation(&mut self) -> u32 {
        let gen = self.shared.connect_gen.get().wrapping_add(1);
        self.shared.connect_gen.set(gen);
        gen
    }

    /// The attempt in progress is no longer the one we care about.
    fn abandon_connect(&mut self) {
        self.next_generation();
        self.active_connect = None;
        self.shared.pending.borrow_mut().take();
    }

    fn start_connect(&mut self, path: String, secret: Option<Secret>) {
        let gen = self.next_generation();
        let had_secret = secret.is_some();
        *self.shared.pending.borrow_mut() = Some(Pending {
            gen,
            path: path.clone(),
            secret,
        });
        if had_secret && !self.agent_registered {
            self.resolve_owner();
            self.register_agent();
            if !self.agent_registered {
                self.shared.pending.borrow_mut().take();
                self.send(WifiEvent::ConnectFailed {
                    path,
                    message: "The password agent could not register with iwd".into(),
                    wrong_password: false,
                });
                return;
            }
        }
        let started = self
            .iwd_method(&path, IFACE_NETWORK, c"Connect")
            .and_then(|m| {
                self.call_async(
                    m,
                    AsyncKind::Connect {
                        gen,
                        path: path.clone(),
                        had_secret,
                    },
                    CONNECT_TIMEOUT_US,
                )
            });
        match started {
            Ok(()) => {
                self.active_connect = Some(gen);
                self.send(WifiEvent::Connecting { path });
            }
            Err(e) => {
                self.shared.pending.borrow_mut().take();
                let (message, wrong_password) = friendly_connect_error(&e, had_secret);
                self.send(WifiEvent::ConnectFailed {
                    path,
                    message,
                    wrong_password,
                });
            }
        }
    }

    /// The explicit Cancel: the attempt's passphrase goes, a queued attempt
    /// goes, iwd is told to stop, and whatever the attempt reports later
    /// (even success) is not the UI's news any more.
    fn cancel_connect(&mut self) {
        let had_attempt = self.connect_in_flight() || self.active_connect.is_some();
        self.abandon_connect();
        self.queued_connect = None;
        if had_attempt {
            self.station_disconnect(true);
        }
    }

    fn disconnect(&mut self) {
        self.abandon_connect();
        self.queued_connect = None;
        self.station_disconnect(false);
    }

    /// `Station.Disconnect`, which also aborts a connect in progress.
    fn station_disconnect(&mut self, quiet: bool) {
        let Some(dev) = self.device.clone() else {
            return;
        };
        if let Err(e) = self
            .iwd_method(&dev, IFACE_STATION, c"Disconnect")
            .and_then(|m| self.call_async(m, AsyncKind::Disconnect, CALL_TIMEOUT_US))
        {
            if !quiet {
                self.notice(format!("Disconnect failed: {e}"), true);
            }
        }
    }

    fn forget(&mut self, known: &str) {
        match self
            .iwd_method(known, IFACE_KNOWN_NETWORK, c"Forget")
            .and_then(|m| self.call(m, CALL_TIMEOUT_US))
        {
            Ok(_) => self.schedule_refresh(0.0),
            Err(e) => self.notice(format!("Forget failed: {e}"), true),
        }
    }

    fn set_powered(&mut self, on: bool) {
        let Some(dev) = self.device.clone() else {
            self.notice("No Wi-Fi device", true);
            return;
        };
        let result = self
            .iwd_method(&dev, IFACE_PROPERTIES, c"Set")
            .and_then(|m| {
                m.append_str(b's', IFACE_DEVICE)?;
                m.append_str(b's', c"Powered")?;
                m.open(b'v', c"b")?;
                m.append_bool(on)?;
                m.close()?;
                self.call(m, CALL_TIMEOUT_US)
            });
        match result {
            Ok(_) => {
                self.schedule_refresh(0.0);
                let now = now_secs();
                self.address_followups = ADDRESS_FOLLOWUPS_SECS.iter().map(|s| now + s).collect();
            }
            Err(e) => self.notice(
                format!("Could not turn Wi-Fi {}: {e}", if on { "on" } else { "off" }),
                true,
            ),
        }
    }
}

/// iwd's error names, read as a person would.
fn friendly_connect_error(e: &BusErr, had_secret: bool) -> (String, bool) {
    let text = match e.suffix() {
        "Failed" if had_secret => "Could not join: wrong password?",
        "Failed" => "Could not join the network",
        "Aborted" => "Connection cancelled",
        "NoAgent" => "No password agent: reopen the Wi-Fi menu and retry",
        "NotConfigured" => "Needs a provisioning file (enterprise network)",
        "NotSupported" => "Network type not supported by iwd",
        "InProgress" | "Busy" => "Already connecting",
        "InvalidFormat" => "Password must be 8–63 characters",
        "NotFound" => "Network is no longer in range",
        "Timeout" | "NoReply" => "Timed out",
        "ServiceUnknown" => "iwd is not running",
        _ => return (e.to_string(), false),
    };
    (
        text.to_string(),
        matches!(e.suffix(), "Failed" | "InvalidFormat") && had_secret,
    )
}

// ======================================================================
// The dropdown's own state — pure, drawn by `panels.rs`
// ======================================================================

/// A hit target inside the Wi-Fi dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiHit {
    Power,
    Refresh,
    Row(usize),
    Field,
    Connect,
    Cancel,
    Disconnect,
    Forget,
    Dismiss,
}

/// The password prompt for one network.
pub struct PasswordEntry {
    pub path: String,
    pub name: String,
    pub secret: Secret,
    /// Why the prompt is (back) up: a wrong password, a stale saved one.
    pub hint: Option<String>,
}

#[derive(Default)]
pub enum WifiPhase {
    #[default]
    Idle,
    Connecting {
        path: String,
        name: String,
    },
    Notice {
        text: String,
        error: bool,
    },
}

/// What the keyboard did to the password field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Not ours (no prompt up, or a WM chord): let it through.
    Ignored,
    Consumed,
    Submit,
    Cancel,
}

/// The "scanning…" reading a press starts stays up this long at most if
/// iwd never reports a scan.
const SCAN_UI_TIMEOUT_SECS: f64 = 12.0;

#[derive(Default)]
pub struct WifiUi {
    pub snapshot: WifiSnapshot,
    /// The worker has reported at least once.
    pub have_snapshot: bool,
    pub entry: Option<PasswordEntry>,
    pub phase: WifiPhase,
    /// When the user pressed scan, until iwd confirms or times out.
    pub scan_requested_at: Option<f64>,
    /// First visible network row.
    pub scroll: usize,
}

impl WifiUi {
    pub fn apply(&mut self, ev: WifiEvent) {
        match ev {
            WifiEvent::Snapshot(s) => {
                let was_scanning = self.snapshot.scanning;
                self.snapshot = s;
                self.have_snapshot = true;
                if self.snapshot.scanning || (was_scanning && !self.snapshot.scanning) {
                    self.scan_requested_at = None;
                }
                let n = self.snapshot.networks.len();
                if self.scroll >= n {
                    self.scroll = n.saturating_sub(1);
                }
            }
            WifiEvent::Connecting { path } => {
                let name = self.name_of(&path);
                self.phase = WifiPhase::Connecting { path, name };
            }
            WifiEvent::Connected { path } => {
                self.entry = None;
                self.phase = WifiPhase::Notice {
                    text: format!("Connected to {}", self.name_of(&path)),
                    error: false,
                };
            }
            WifiEvent::ConnectFailed {
                path,
                message,
                wrong_password,
            } => {
                if wrong_password {
                    let name = self.name_of(&path);
                    self.phase = WifiPhase::Idle;
                    self.entry = Some(PasswordEntry {
                        path,
                        name,
                        secret: Secret::new(),
                        hint: Some(message),
                    });
                } else {
                    self.phase = WifiPhase::Notice {
                        text: message,
                        error: true,
                    };
                }
            }
            WifiEvent::PasswordNeeded { path } => {
                let name = self.name_of(&path);
                self.phase = WifiPhase::Idle;
                self.entry = Some(PasswordEntry {
                    path,
                    name,
                    secret: Secret::new(),
                    hint: Some("Password needed".into()),
                });
            }
            WifiEvent::EnterpriseNeeded { path } => {
                let name = self.name_of(&path);
                self.entry = None;
                self.phase = WifiPhase::Notice {
                    text: format!("{name} is an 802.1X network: iwd needs a provisioning file for it"),
                    error: true,
                };
            }
            WifiEvent::ScanDone { error } => {
                self.scan_requested_at = None;
                if let Some(text) = error {
                    self.phase = WifiPhase::Notice { text, error: true };
                }
            }
            WifiEvent::Notice { text, error } => {
                self.phase = WifiPhase::Notice { text, error };
            }
        }
    }

    /// Once a second: expire a scan reading iwd never confirmed. True when
    /// something to draw changed.
    pub fn tick(&mut self, now: f64) -> bool {
        if let Some(t) = self.scan_requested_at {
            if now - t > SCAN_UI_TIMEOUT_SECS {
                self.scan_requested_at = None;
                return true;
            }
        }
        false
    }

    pub fn scanning(&self) -> bool {
        self.snapshot.scanning || self.scan_requested_at.is_some()
    }

    pub fn name_of(&self, path: &str) -> String {
        self.snapshot
            .networks
            .iter()
            .find(|n| n.path == path)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "the network".to_string())
    }

    pub fn entering(&self) -> bool {
        self.entry.is_some()
    }

    /// What a press on a network row does.
    pub fn activate_row(&mut self, index: usize) -> Option<WifiCommand> {
        let net = self.snapshot.networks.get(index)?.clone();
        self.phase = WifiPhase::Idle;
        if net.connected {
            return None;
        }
        match net.security {
            Security::Open => {
                self.entry = None;
                Some(WifiCommand::Connect { path: net.path })
            }
            Security::Psk if net.known.is_some() => {
                self.entry = None;
                Some(WifiCommand::Connect { path: net.path })
            }
            Security::Psk => {
                self.entry = Some(PasswordEntry {
                    path: net.path,
                    name: net.name,
                    secret: Secret::new(),
                    hint: None,
                });
                None
            }
            Security::Enterprise => {
                self.phase = WifiPhase::Notice {
                    text: format!("{} is an 802.1X network: iwd needs a provisioning file for it", net.name),
                    error: true,
                };
                None
            }
            Security::Wep => {
                self.phase = WifiPhase::Notice {
                    text: "WEP networks are not supported by iwd".into(),
                    error: true,
                };
                None
            }
            Security::Unknown => {
                self.phase = WifiPhase::Notice {
                    text: "Unknown network security".into(),
                    error: true,
                };
                None
            }
        }
    }

    /// The prompt's Connect: the command the WM completes with the typed
    /// passphrase ([`Self::take_secret`]).
    pub fn submit(&mut self) -> Option<WifiCommand> {
        let entry = self.entry.as_mut()?;
        if let Some(problem) = entry.secret.problem() {
            entry.hint = Some(problem.to_string());
            return None;
        }
        let path = entry.path.clone();
        let name = entry.name.clone();
        self.phase = WifiPhase::Connecting {
            path: path.clone(),
            name,
        };
        Some(WifiCommand::ConnectWithPassphrase { path })
    }

    /// Move the typed passphrase out for the worker command. The prompt is
    /// gone after this; anything left behind is wiped.
    pub fn take_secret(&mut self, path: &str) -> Option<Secret> {
        let entry = self.entry.take()?;
        if entry.path != path {
            return None;
        }
        let PasswordEntry { secret, .. } = entry;
        Some(secret)
    }

    /// Dismiss the prompt (the secret is wiped by its drop).
    pub fn cancel_entry(&mut self) -> bool {
        self.entry.take().is_some()
    }

    pub fn dismiss_notice(&mut self) {
        if matches!(self.phase, WifiPhase::Notice { .. }) {
            self.phase = WifiPhase::Idle;
        }
    }

    /// The dropdown closed: nothing typed survives it, and a stale notice
    /// does not greet the next open.
    pub fn on_close(&mut self) {
        self.cancel_entry();
        self.dismiss_notice();
    }

    /// The keyboard while the prompt is up. Characters arrive through
    /// `text_input`; every other key is swallowed so nothing typed at a
    /// password field reaches a tile — except WM chords (logo), which stay
    /// the WM's.
    pub fn key(&mut self, e: &KeyEvent) -> KeyOutcome {
        let Some(entry) = self.entry.as_mut() else {
            return KeyOutcome::Ignored;
        };
        match e.key_code {
            KeyCode::Escape => KeyOutcome::Cancel,
            KeyCode::ReturnKey | KeyCode::NumpadEnter => KeyOutcome::Submit,
            KeyCode::Backspace => {
                if e.modifiers.control || e.modifiers.alt {
                    entry.secret.clear();
                } else {
                    entry.secret.pop();
                }
                entry.hint = None;
                KeyOutcome::Consumed
            }
            KeyCode::KeyU if e.modifiers.control => {
                entry.secret.clear();
                entry.hint = None;
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    /// Typed text while the prompt is up; false when there is no prompt.
    pub fn text_input(&mut self, input: &str) -> bool {
        let Some(entry) = self.entry.as_mut() else {
            return false;
        };
        // Reject a paste as a whole. Never silently remove characters or
        // submit a truncated version of the user's credential.
        if !input.chars().all(Secret::accepts) {
            entry.hint = Some("Wi-Fi passwords use printable ASCII characters".into());
            return true;
        }
        if entry.secret.len_chars() + input.len() > SECRET_MAX_CHARS {
            entry.hint = Some(format!("At most {SECRET_MAX_CHARS} characters; paste was not entered"));
            return true;
        }
        entry.hint = None;
        for ch in input.chars() { entry.secret.push(ch); }
        true
    }

    /// The nonsecret reading a remote snapshot (`/snap`) reports for the
    /// open dropdown, so a proof can see the rows, the state and whether
    /// the prompt is up — never what was typed, not even its length.
    pub fn summary(&self) -> String {
        let s = &self.snapshot;
        let mut out = format!(
            "wifi radio={:?} state={:?} scanning={} device={} agent={}",
            s.radio,
            s.state,
            self.scanning(),
            s.device.as_deref().unwrap_or("-"),
            s.agent
        );
        if !self.have_snapshot {
            out.push_str(" waiting");
        }
        if let Some(u) = &s.unavailable {
            out.push_str(&format!(" unavailable=\"{u}\""));
        }
        for a in &s.addresses {
            out.push_str(&format!(" ip[{} {}]={}", a.name, a.kind(), a.ipv4));
        }
        for (i, n) in s.networks.iter().enumerate() {
            out.push_str(&format!(
                " net{i}=\"{}\" {} {}{}{}",
                n.name,
                n.security.label(),
                n.signal_dbm.map(|d| format!("{d}dBm")).unwrap_or_else(|| "-".into()),
                if n.known.is_some() { " saved" } else { "" },
                if n.connected { " connected" } else { "" }
            ));
        }
        match &self.phase {
            WifiPhase::Idle => {}
            WifiPhase::Connecting { name, .. } => out.push_str(&format!(" connecting=\"{name}\"")),
            WifiPhase::Notice { text, error } => {
                out.push_str(&format!(" notice=\"{text}\" error={error}"))
            }
        }
        if let Some(e) = &self.entry {
            out.push_str(&format!(
                " password_entry=\"{}\" filled={}",
                e.name,
                !e.secret.is_empty()
            ));
            if let Some(h) = &e.hint {
                out.push_str(&format!(" hint=\"{h}\""));
            }
        }
        out
    }
}
