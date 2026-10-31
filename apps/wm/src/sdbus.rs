//! sd-bus, the D-Bus client library in `libsystemd.so.0`, loaded with `dlopen` at runtime (no
//! link-time dependency, no crate): the subset of `<systemd/sd-bus.h>` the WM uses (only
//! non-variadic entry points), D-Bus values, and a message reader/builder. Users: the Wi-Fi
//! panel (system bus, iwd) and the desktop portal backend (session bus). Linux only.
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::null;

extern "C" {
    pub(crate) fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub(crate) fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
pub(crate) const RTLD_NOW: c_int = 2;

pub(crate) fn errno_text(n: i32) -> String {
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
pub(crate) struct SdBusError {
    pub(crate) name: *const c_char,
    pub(crate) message: *const c_char,
    pub(crate) need_free: c_int,
}

impl SdBusError {
    pub(crate) fn null() -> Self {
        Self {
            name: null(),
            message: null(),
            need_free: 0,
        }
    }

    pub(crate) fn constant(name: &'static CStr, message: &'static CStr) -> Self {
        Self {
            name: name.as_ptr(),
            message: message.as_ptr(),
            need_free: 0,
        }
    }

    pub(crate) fn is_set(&self) -> bool {
        !self.name.is_null()
    }

    pub(crate) fn name(&self) -> String {
        cstr_at(self.name)
    }

    pub(crate) fn message(&self) -> String {
        cstr_at(self.message)
    }
}

/// Copy a C string out immediately; sd-bus pointers live only as long as
/// the message they came from.
pub(crate) fn cstr_at(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

pub(crate) type SdBusHandler =
    unsafe extern "C" fn(*mut SdBusMessage, *mut c_void, *mut SdBusError) -> c_int;

macro_rules! sd_bus_api {
    ($( fn $name:ident($($arg:ty),*) -> $ret:ty; )*) => {
        #[derive(Clone, Copy)]
        #[allow(non_snake_case)]
        pub(crate) struct SdBusApi {
            $( pub(crate) $name: unsafe extern "C" fn($($arg),*) -> $ret, )*
        }

        impl SdBusApi {
            /// Resolve every entry point from an open `libsystemd` handle.
            pub(crate) unsafe fn load(lib: *mut c_void) -> Result<Self, String> {
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
    fn sd_bus_open_user(*mut *mut SdBus) -> c_int;
    fn sd_bus_request_name(*mut SdBus, *const c_char, u64) -> c_int;
    fn sd_bus_message_new_signal(*mut SdBus, *mut *mut SdBusMessage, *const c_char, *const c_char, *const c_char) -> c_int;
    fn sd_bus_message_ref(*mut SdBusMessage) -> *mut SdBusMessage;
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
    fn sd_bus_message_read_array(*mut SdBusMessage, c_char, *mut *const c_void, *mut usize) -> c_int;
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


/// What went wrong on the bus: a negative errno from sd-bus or a D-Bus
/// error reply.
#[derive(Clone, Debug)]
pub(crate) enum BusErr {
    Sys(i32),
    Dbus { name: String, message: String },
}

impl BusErr {
    /// The error name's last segment (`net.connman.iwd.Failed` → `Failed`).
    pub(crate) fn suffix(&self) -> &str {
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

pub(crate) fn check(r: c_int) -> Result<c_int, BusErr> {
    if r < 0 {
        Err(BusErr::Sys(-r))
    } else {
        Ok(r)
    }
}

pub(crate) fn cstring(s: &str) -> Result<CString, BusErr> {
    CString::new(s).map_err(|_| BusErr::Sys(22))
}


/// A value read out of a D-Bus variant. Only what iwd's properties use.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DbusValue {
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
    Strs(Vec<String>),
    Other,
}

pub(crate) type Props = Vec<(String, DbusValue)>;

pub(crate) fn prop_str(props: &Props, key: &str) -> Option<String> {
    props.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        DbusValue::Str(s) | DbusValue::Path(s) => Some(s.clone()),
        _ => None,
    })
}

pub(crate) fn prop_strs(props: &Props, key: &str) -> Option<Vec<String>> {
    props.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        DbusValue::Strs(s) => Some(s.clone()),
        _ => None,
    })
}

pub(crate) fn prop_bool(props: &Props, key: &str) -> Option<bool> {
    props.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        DbusValue::Bool(b) => Some(*b),
        _ => None,
    })
}

/// One object out of `GetManagedObjects`.
pub(crate) struct DbusObject {
    pub(crate) path: String,
    pub(crate) ifaces: Vec<(String, Props)>,
}

impl DbusObject {
    pub(crate) fn iface(&self, name: &CStr) -> Option<&Props> {
        let name = name.to_str().unwrap_or("");
        self.ifaces.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }
}

/// One `sd_bus_message` reference: owned (unref'd on drop) for calls and
/// replies, borrowed for the messages a handler is given.
pub(crate) struct Msg {
    pub(crate) api: SdBusApi,
    pub(crate) m: *mut SdBusMessage,
    pub(crate) owned: bool,
}

impl Drop for Msg {
    fn drop(&mut self) {
        if self.owned && !self.m.is_null() {
            unsafe { (self.api.sd_bus_message_unref)(self.m) };
        }
    }
}

impl Msg {
    pub(crate) fn borrowed(api: SdBusApi, m: *mut SdBusMessage) -> Self {
        Self {
            api,
            m,
            owned: false,
        }
    }

    // ------------------------------------------------------------ write

    pub(crate) fn append_str(&self, ty: u8, s: &CStr) -> Result<(), BusErr> {
        check(unsafe {
            (self.api.sd_bus_message_append_basic)(
                self.m,
                ty as c_char,
                s.as_ptr() as *const c_void,
            )
        })
        .map(|_| ())
    }

    /// A fixed-size basic value (`u`, `i`, `t`, `d`, ...).
    pub(crate) fn append_num<T: Copy>(&self, ty: u8, v: T) -> Result<(), BusErr> {
        check(unsafe {
            (self.api.sd_bus_message_append_basic)(self.m, ty as c_char, &v as *const T as *const c_void)
        })
        .map(|_| ())
    }

    pub(crate) fn append_bool(&self, b: bool) -> Result<(), BusErr> {
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

    pub(crate) fn open(&self, ty: u8, contents: &CStr) -> Result<(), BusErr> {
        check(unsafe {
            (self.api.sd_bus_message_open_container)(self.m, ty as c_char, contents.as_ptr())
        })
        .map(|_| ())
    }

    pub(crate) fn close(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_close_container)(self.m) }).map(|_| ())
    }

    // ------------------------------------------------------------- read

    pub(crate) fn read_str(&self, ty: u8) -> Result<String, BusErr> {
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

    pub(crate) fn read_bool(&self) -> Result<bool, BusErr> {
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

    /// An `ay` (bytes) at once.
    pub(crate) fn read_bytes(&self) -> Result<Vec<u8>, BusErr> {
        let mut p: *const c_void = null();
        let mut size = 0usize;
        check(unsafe { (self.api.sd_bus_message_read_array)(self.m, b'y' as c_char, &mut p, &mut size) })?;
        if p.is_null() || size == 0 {
            return Ok(Vec::new());
        }
        Ok(unsafe { std::slice::from_raw_parts(p as *const u8, size) }.to_vec())
    }

    pub(crate) fn read_num<T: Copy + Default>(&self, ty: u8) -> Result<T, BusErr> {
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
    pub(crate) fn enter(&self, ty: u8, contents: &CStr) -> Result<bool, BusErr> {
        let r = check(unsafe {
            (self.api.sd_bus_message_enter_container)(self.m, ty as c_char, contents.as_ptr())
        })?;
        Ok(r > 0)
    }

    pub(crate) fn exit(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_exit_container)(self.m) }).map(|_| ())
    }

    pub(crate) fn at_end(&self) -> Result<bool, BusErr> {
        Ok(check(unsafe { (self.api.sd_bus_message_at_end)(self.m, 0) })? > 0)
    }

    /// The type (and, for containers, the contents signature) of the next
    /// element; `None` at the end of the current container.
    pub(crate) fn peek(&self) -> Result<Option<(u8, String)>, BusErr> {
        let mut ty: c_char = 0;
        let mut contents: *const c_char = null();
        let r = check(unsafe { (self.api.sd_bus_message_peek_type)(self.m, &mut ty, &mut contents) })?;
        if r == 0 {
            return Ok(None);
        }
        Ok(Some((ty as u8, cstr_at(contents))))
    }

    pub(crate) fn skip(&self) -> Result<(), BusErr> {
        check(unsafe { (self.api.sd_bus_message_skip)(self.m, null()) }).map(|_| ())
    }

    /// One variant, whatever is inside.
    pub(crate) fn read_variant(&self) -> Result<DbusValue, BusErr> {
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
            "as" => {
                let mut strs = Vec::new();
                if self.enter(b'a', c"s")? {
                    while !self.at_end()? {
                        strs.push(self.read_str(b's')?);
                    }
                    self.exit()?;
                }
                DbusValue::Strs(strs)
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
    pub(crate) fn read_dict_sv(&self) -> Result<Props, BusErr> {
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
    pub(crate) fn read_managed_objects(&self) -> Result<Vec<DbusObject>, BusErr> {
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

}

pub(crate) fn message_error(api: &SdBusApi, m: *mut SdBusMessage) -> Option<(String, String)> {
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
