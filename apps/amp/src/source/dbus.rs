//! Just enough D-Bus for a remote control: the session bus, a method call
//! and its reply. Little-endian messages, `EXTERNAL` authentication, no
//! file descriptors. Arguments can be the basic types and variants of them;
//! replies decode completely (arrays, dicts, structs, variants).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A call that has not been answered by then fails.
const TIMEOUT: Duration = Duration::from_secs(3);
/// A reply larger than this is not one this client asked for.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Path(String),
    Sig(String),
    /// A file descriptor's index; never sent.
    Fd(u32),
    Array(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Struct(Vec<Value>),
    Variant(Box<Value>),
}

impl Value {
    /// The value inside any number of variants.
    pub fn inner(&self) -> &Value {
        match self {
            Value::Variant(inner) => inner.inner(),
            other => other,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.inner() {
            Value::Str(text) | Value::Path(text) | Value::Sig(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.inner() {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Any number, as a float.
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self.inner() {
            Value::Byte(v) => *v as f64,
            Value::I16(v) => *v as f64,
            Value::U16(v) => *v as f64,
            Value::I32(v) => *v as f64,
            Value::U32(v) => *v as f64,
            Value::I64(v) => *v as f64,
            Value::U64(v) => *v as f64,
            Value::F64(v) => *v,
            _ => return None,
        })
    }

    /// The strings of an array of strings (or one string).
    pub fn as_strings(&self) -> Vec<String> {
        match self.inner() {
            Value::Array(items) => items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect(),
            other => other.as_str().map(|text| vec![text.to_string()]).unwrap_or_default(),
        }
    }

    /// The value under `key` in a dict with string keys.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self.inner() {
            Value::Dict(entries) => entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    fn signature(&self) -> Result<String, String> {
        Ok(match self {
            Value::Byte(_) => "y".into(),
            Value::Bool(_) => "b".into(),
            Value::I16(_) => "n".into(),
            Value::U16(_) => "q".into(),
            Value::I32(_) => "i".into(),
            Value::U32(_) => "u".into(),
            Value::I64(_) => "x".into(),
            Value::U64(_) => "t".into(),
            Value::F64(_) => "d".into(),
            Value::Str(_) => "s".into(),
            Value::Path(_) => "o".into(),
            Value::Sig(_) => "g".into(),
            Value::Variant(_) => "v".into(),
            _ => return Err("only basic values and variants are sent".into()),
        })
    }
}

// ---- marshalling ----

fn alignment(code: u8) -> usize {
    match code {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn pad(&mut self, align: usize) {
        while self.buf.len() % align != 0 {
            self.buf.push(0);
        }
    }

    fn u32(&mut self, value: u32) {
        self.pad(4);
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn string(&mut self, text: &str) {
        self.u32(text.len() as u32);
        self.buf.extend_from_slice(text.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, text: &str) {
        self.buf.push(text.len() as u8);
        self.buf.extend_from_slice(text.as_bytes());
        self.buf.push(0);
    }

    fn value(&mut self, value: &Value) -> Result<(), String> {
        match value {
            Value::Byte(v) => self.buf.push(*v),
            Value::Bool(v) => self.u32(*v as u32),
            Value::I16(v) => {
                self.pad(2);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::U16(v) => {
                self.pad(2);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::I32(v) => {
                self.pad(4);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::U32(v) => self.u32(*v),
            Value::I64(v) => {
                self.pad(8);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::U64(v) => {
                self.pad(8);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::F64(v) => {
                self.pad(8);
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            Value::Str(v) | Value::Path(v) => self.string(v),
            Value::Sig(v) => self.signature(v),
            Value::Variant(inner) => {
                let signature = inner.signature()?;
                self.signature(&signature);
                self.value(inner)?;
            }
            _ => return Err("only basic values and variants are sent".into()),
        }
        Ok(())
    }
}

/// Where the complete type starting at `at` in `sig` ends.
fn type_end(sig: &[u8], at: usize) -> Result<usize, String> {
    let bad = || "malformed signature".to_string();
    match *sig.get(at).ok_or_else(bad)? {
        b'a' => type_end(sig, at + 1),
        open @ (b'(' | b'{') => {
            let close = if open == b'(' { b')' } else { b'}' };
            let mut next = at + 1;
            while *sig.get(next).ok_or_else(bad)? != close {
                next = type_end(sig, next)?;
            }
            Ok(next + 1)
        }
        _ => Ok(at + 1),
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    depth: u32,
}

impl<'a> Reader<'a> {
    fn align(&mut self, align: usize) -> Result<(), String> {
        self.pos = self.pos.div_ceil(align) * align;
        if self.pos > self.data.len() {
            return Err("message ends early".into());
        }
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.data.len()).ok_or("message ends early")?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.align(N)?;
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.fixed::<4>()?))
    }

    fn text(&mut self, len: usize) -> Result<String, String> {
        let bytes = self.take(len + 1)?;
        String::from_utf8(bytes[..len].to_vec()).map_err(|_| "string is not UTF-8".to_string())
    }

    fn signature(&mut self) -> Result<String, String> {
        let len = self.take(1)?[0] as usize;
        self.text(len)
    }

    /// One value of the complete type `sig` (exactly one complete type).
    fn value(&mut self, sig: &[u8]) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("values nested too deep".into());
        }
        let value = self.value_inner(sig);
        self.depth -= 1;
        value
    }

    fn value_inner(&mut self, sig: &[u8]) -> Result<Value, String> {
        Ok(match sig.first().copied().ok_or("empty signature")? {
            b'y' => Value::Byte(self.take(1)?[0]),
            b'b' => Value::Bool(self.u32()? != 0),
            b'n' => Value::I16(i16::from_le_bytes(self.fixed::<2>()?)),
            b'q' => Value::U16(u16::from_le_bytes(self.fixed::<2>()?)),
            b'i' => Value::I32(i32::from_le_bytes(self.fixed::<4>()?)),
            b'u' => Value::U32(self.u32()?),
            b'h' => Value::Fd(self.u32()?),
            b'x' => Value::I64(i64::from_le_bytes(self.fixed::<8>()?)),
            b't' => Value::U64(u64::from_le_bytes(self.fixed::<8>()?)),
            b'd' => Value::F64(f64::from_le_bytes(self.fixed::<8>()?)),
            b's' => {
                let len = self.u32()? as usize;
                Value::Str(self.text(len)?)
            }
            b'o' => {
                let len = self.u32()? as usize;
                Value::Path(self.text(len)?)
            }
            b'g' => Value::Sig(self.signature()?),
            b'v' => {
                let inner = self.signature()?;
                if type_end(inner.as_bytes(), 0)? != inner.len() {
                    return Err("variant holds more than one type".into());
                }
                Value::Variant(Box::new(self.value(inner.as_bytes())?))
            }
            b'a' => {
                let len = self.u32()? as usize;
                let element = &sig[1..type_end(sig, 1)?];
                self.align(alignment(element[0]))?;
                let end = self.pos.checked_add(len).filter(|end| *end <= self.data.len()).ok_or("array ends past the message")?;
                if element[0] == b'{' {
                    let key_end = type_end(element, 1)?;
                    let (key, value) = (&element[1..key_end], &element[key_end..element.len() - 1]);
                    let mut entries = Vec::new();
                    while self.pos < end {
                        self.align(8)?;
                        let k = self.value(key)?;
                        let v = self.value(value)?;
                        entries.push((k, v));
                    }
                    Value::Dict(entries)
                } else {
                    let mut items = Vec::new();
                    while self.pos < end {
                        items.push(self.value(element)?);
                    }
                    Value::Array(items)
                }
            }
            b'(' => {
                self.align(8)?;
                let mut fields = Vec::new();
                let mut at = 1;
                while sig.get(at).is_some_and(|c| *c != b')') {
                    let end = type_end(sig, at)?;
                    fields.push(self.value(&sig[at..end])?);
                    at = end;
                }
                Value::Struct(fields)
            }
            other => return Err(format!("cannot read type '{}'", other as char)),
        })
    }

    /// Every value of a body with signature `sig`.
    fn values(&mut self, sig: &str) -> Result<Vec<Value>, String> {
        let sig = sig.as_bytes();
        let mut out = Vec::new();
        let mut at = 0;
        while at < sig.len() {
            let end = type_end(sig, at)?;
            out.push(self.value(&sig[at..end])?);
            at = end;
        }
        Ok(out)
    }
}

const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;

/// A method call message, ready to write.
fn method_call(serial: u32, destination: &str, path: &str, interface: &str, member: &str, args: &[Value]) -> Result<Vec<u8>, String> {
    let mut body = Writer::default();
    let mut signature = String::new();
    for arg in args {
        signature.push_str(&arg.signature()?);
        body.value(arg)?;
    }
    let mut fields: Vec<(u8, Value)> = vec![
        (1, Value::Path(path.into())),
        (2, Value::Str(interface.into())),
        (3, Value::Str(member.into())),
        (6, Value::Str(destination.into())),
    ];
    if !signature.is_empty() {
        fields.push((8, Value::Sig(signature)));
    }
    let mut message = Writer::default();
    message.buf.extend_from_slice(&[b'l', METHOD_CALL, 0, 1]);
    message.u32(body.buf.len() as u32);
    message.u32(serial);
    let length_at = message.buf.len();
    message.u32(0);
    let start = message.buf.len();
    for (code, value) in &fields {
        message.pad(8);
        message.buf.push(*code);
        message.value(&Value::Variant(Box::new(value.clone())))?;
    }
    let length = (message.buf.len() - start) as u32;
    message.buf[length_at..length_at + 4].copy_from_slice(&length.to_le_bytes());
    message.pad(8);
    message.buf.extend_from_slice(&body.buf);
    Ok(message.buf)
}

/// A message read off the bus.
struct Message {
    kind: u8,
    reply_serial: Option<u32>,
    error: Option<String>,
    body: Vec<Value>,
}

/// Parse one whole message.
fn parse(bytes: &[u8]) -> Result<Message, String> {
    if bytes.len() < 16 || bytes[0] != b'l' {
        return Err("not a little-endian message".into());
    }
    let mut reader = Reader { data: bytes, pos: 12, depth: 0 };
    let fields_len = reader.u32()? as usize;
    let fields_end = 16 + fields_len;
    let (mut reply_serial, mut error, mut signature) = (None, None, String::new());
    while reader.pos < fields_end {
        reader.align(8)?;
        let code = reader.take(1)?[0];
        let value = reader.value(b"v")?;
        match (code, value.inner()) {
            (4, Value::Str(name)) => error = Some(name.clone()),
            (5, Value::U32(serial)) => reply_serial = Some(*serial),
            (8, Value::Sig(sig)) => signature = sig.clone(),
            _ => {}
        }
    }
    let body_start = fields_end.div_ceil(8) * 8;
    let body = bytes.get(body_start..).ok_or("message ends early")?;
    let body = Reader { data: body, pos: 0, depth: 0 }.values(&signature)?;
    Ok(Message { kind: bytes[1], reply_serial, error, body })
}

extern "C" {
    fn getuid() -> u32;
}

/// A connection to the session bus.
pub struct Bus {
    stream: UnixStream,
    serial: u32,
}

impl Bus {
    /// The session bus of this login (`DBUS_SESSION_BUS_ADDRESS`, else
    /// `$XDG_RUNTIME_DIR/bus`), authenticated and said hello to.
    pub fn session() -> Result<Self, String> {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .filter(|address| !address.is_empty())
            .or_else(|| std::env::var("XDG_RUNTIME_DIR").ok().filter(|dir| !dir.is_empty()).map(|dir| format!("unix:path={dir}/bus")))
            .ok_or("no session bus (DBUS_SESSION_BUS_ADDRESS is not set)")?;
        let mut last = String::from("no usable address");
        for entry in address.split(';') {
            match connect(entry) {
                Ok(stream) => {
                    let mut bus = Bus { stream, serial: 0 };
                    bus.authenticate()?;
                    bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[])?;
                    return Ok(bus);
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn authenticate(&mut self) -> Result<(), String> {
        let uid = unsafe { getuid() }.to_string();
        let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
        self.stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        let line = self.line()?;
        if !line.starts_with("OK ") {
            return Err(format!("the bus refused us: {line}"));
        }
        self.stream.write_all(b"BEGIN\r\n").map_err(|e| e.to_string())
    }

    fn line(&mut self) -> Result<String, String> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            if line.len() > 4096 {
                return Err("the bus says too much".into());
            }
            self.stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
            line.push(byte[0]);
        }
        Ok(String::from_utf8_lossy(&line[..line.len() - 2]).into_owned())
    }

    fn read_message(&mut self) -> Result<Message, String> {
        let mut head = [0u8; 16];
        self.stream.read_exact(&mut head).map_err(|e| e.to_string())?;
        if head[0] != b'l' {
            return Err("the bus speaks big-endian".into());
        }
        let body_len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let fields_len = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
        let total = (16 + fields_len).div_ceil(8) * 8 + body_len;
        if total > MAX_MESSAGE {
            return Err("message too large".into());
        }
        let mut bytes = head.to_vec();
        bytes.resize(total, 0);
        self.stream.read_exact(&mut bytes[16..]).map_err(|e| e.to_string())?;
        parse(&bytes)
    }

    /// Call a method and return what it answered. Signals that arrive in
    /// between are skipped.
    pub fn call(&mut self, destination: &str, path: &str, interface: &str, member: &str, args: &[Value]) -> Result<Vec<Value>, String> {
        self.serial = self.serial.wrapping_add(1).max(1);
        let serial = self.serial;
        let message = method_call(serial, destination, path, interface, member, args)?;
        self.stream.write_all(&message).map_err(|e| e.to_string())?;
        loop {
            let reply = self.read_message()?;
            if reply.reply_serial != Some(serial) {
                continue;
            }
            return match reply.kind {
                METHOD_RETURN => Ok(reply.body),
                ERROR => {
                    let detail = reply.body.first().and_then(Value::as_str).unwrap_or_default().to_string();
                    Err(format!("{}: {detail}", reply.error.unwrap_or_default()))
                }
                _ => continue,
            };
        }
    }
}

fn connect(address: &str) -> Result<UnixStream, String> {
    let (transport, params) = address.split_once(':').ok_or("malformed bus address")?;
    if transport != "unix" {
        return Err(format!("bus transport {transport} is not supported"));
    }
    let param = |name: &str| params.split(',').find_map(|kv| kv.strip_prefix(name).and_then(|rest| rest.strip_prefix('='))).map(unescape);
    let stream = if let Some(path) = param("path") {
        UnixStream::connect(path).map_err(|e| e.to_string())?
    } else if let Some(name) = param("abstract") {
        use std::os::linux::net::SocketAddrExt;
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).map_err(|e| e.to_string())?;
        UnixStream::connect_addr(&address).map_err(|e| e.to_string())?
    } else {
        return Err("bus address has no path".into());
    };
    stream.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    Ok(stream)
}

/// A bus address value with its %xx escapes decoded.
fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[at + 1..at + 3], 16) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_reads_back_as_itself() {
        let bytes = method_call(7, "org.mpris.MediaPlayer2.spotify", "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", "SetPosition", &[
            Value::Path("/com/spotify/track/abc".into()),
            Value::I64(-5),
            Value::Variant(Box::new(Value::F64(0.5))),
        ])
        .unwrap();
        let message = parse(&bytes).unwrap();
        assert_eq!(message.kind, METHOD_CALL);
        assert_eq!(message.body, vec![Value::Path("/com/spotify/track/abc".into()), Value::I64(-5), Value::Variant(Box::new(Value::F64(0.5)))]);
    }

    #[test]
    fn dicts_of_variants_decode() {
        // a{sv} {"a": <int64 3>, "b": <["x", "y"]>}
        let mut w = Writer::default();
        let mut entries = Writer::default();
        entries.pad(8);
        entries.string("a");
        entries.signature("x");
        entries.pad(8);
        entries.buf.extend_from_slice(&3i64.to_le_bytes());
        entries.pad(8);
        entries.string("b");
        entries.signature("as");
        entries.pad(4);
        let length_at = entries.buf.len();
        entries.u32(0);
        let start = entries.buf.len();
        entries.string("x");
        entries.string("y");
        let length = (entries.buf.len() - start) as u32;
        entries.buf[length_at..length_at + 4].copy_from_slice(&length.to_le_bytes());
        // The array's contents start 8-aligned after its length.
        w.u32((entries.buf.len()) as u32);
        w.pad(8);
        w.buf.extend_from_slice(&entries.buf);
        let values = Reader { data: &w.buf, pos: 0, depth: 0 }.values("a{sv}").unwrap();
        let dict = &values[0];
        assert_eq!(dict.get("a").and_then(Value::as_f64), Some(3.0));
        assert_eq!(dict.get("b").map(Value::as_strings), Some(vec!["x".to_string(), "y".to_string()]));
    }

    /// Against this login's session bus:
    /// `cargo test -p makepad-app-amp live_bus -- --ignored`.
    #[test]
    #[ignore]
    fn live_bus_answers() {
        let mut bus = Bus::session().unwrap();
        let reply = bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", &[]).unwrap();
        assert!(reply[0].as_strings().iter().any(|name| name == "org.freedesktop.DBus"));
        let owned = bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &[Value::Str("org.freedesktop.DBus".into())]).unwrap();
        assert_eq!(owned[0].as_bool(), Some(true));
        let error = bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NoSuchMethod", &[]).unwrap_err();
        assert!(error.starts_with("org.freedesktop.DBus.Error"), "{error}");
    }

    #[test]
    fn addresses_unescape() {
        assert_eq!(unescape("/run/user/1000/bus"), "/run/user/1000/bus");
        assert_eq!(unescape("/tmp/a%20b"), "/tmp/a b");
    }
}
