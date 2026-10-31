use crate::signature::{validate_bus_name, validate_interface, validate_member};
use crate::{Endian, Error, Reader, Result, Signature, Value, Writer, MAX_MESSAGE};

pub const NO_REPLY_EXPECTED: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    MethodCall = 1,
    MethodReturn = 2,
    Error = 3,
    Signal = 4,
}

/// Validated body bytes. Typed code can borrow strings and arrays via `reader`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Body {
    signature: String,
    bytes: Vec<u8>,
    endian: Endian,
}

impl Body {
    pub fn from_writer(signature: &str, writer: Writer) -> Result<Self> {
        Self::from_bytes(signature, writer.into_bytes()?, Endian::Little)
    }
    pub fn from_bytes(signature: &str, bytes: Vec<u8>, endian: Endian) -> Result<Self> {
        let mut reader = Reader::new(&bytes, endian);
        reader.skip(signature)?;
        reader.finish()?;
        Ok(Self {
            signature: signature.to_owned(),
            bytes,
            endian,
        })
    }
    pub fn from_values(values: &[Value]) -> Result<Self> {
        let mut writer = Writer::new();
        let mut signature = String::new();
        for value in values {
            signature.push_str(&value.signature()?);
            writer.value(value)?;
        }
        Self::from_writer(&signature, writer)
    }
    pub fn signature(&self) -> &str {
        &self.signature
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.bytes, self.endian)
    }
    pub fn values(&self) -> Result<Vec<Value>> {
        self.reader().values(&self.signature)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub kind: MessageType,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub unix_fds: u32,
    /// Unrecognized fields are preserved by the codec.
    pub unknown: Vec<(u8, Value)>,
}

impl Header {
    pub fn new(kind: MessageType) -> Self {
        Self {
            kind,
            flags: 0,
            serial: 0,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            unix_fds: 0,
            unknown: Vec::new(),
        }
    }
    fn validate(&self) -> Result<()> {
        if self.serial == 0 || self.reply_serial == Some(0) {
            return Err(Error::invalid("zero message serial"));
        }
        if let Some(v) = &self.path {
            crate::validate_object_path(v)?;
        }
        for v in [&self.interface, &self.error_name].into_iter().flatten() {
            validate_interface(v)?;
        }
        for v in [&self.destination, &self.sender].into_iter().flatten() {
            validate_bus_name(v)?;
        }
        if let Some(v) = &self.member {
            validate_member(v)?;
        }
        let valid = match self.kind {
            MessageType::MethodCall => self.path.is_some() && self.member.is_some(),
            MessageType::Signal => {
                self.path.is_some() && self.interface.is_some() && self.member.is_some()
            }
            MessageType::MethodReturn => self.reply_serial.is_some(),
            MessageType::Error => self.error_name.is_some() && self.reply_serial.is_some(),
        };
        if !valid {
            return Err(Error::invalid("missing required header field"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub header: Header,
    pub body: Body,
}

impl Message {
    pub fn method_call(
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        body: Body,
    ) -> Self {
        let mut header = Header::new(MessageType::MethodCall);
        if !destination.is_empty() {
            header.destination = Some(destination.into());
        }
        header.path = Some(path.into());
        if !interface.is_empty() {
            header.interface = Some(interface.into());
        }
        header.member = Some(member.into());
        Self { header, body }
    }
    pub fn signal(path: &str, interface: &str, member: &str, body: Body) -> Self {
        let mut message = Self::method_call("", path, interface, member, body);
        message.header.kind = MessageType::Signal;
        message
    }
    pub fn method_return(call: &Self, body: Body) -> Self {
        let mut header = Header::new(MessageType::MethodReturn);
        header.reply_serial = Some(call.header.serial);
        header.destination = call.header.sender.clone();
        Self { header, body }
    }
    pub fn error(call: &Self, name: &str, detail: &str) -> Result<Self> {
        let mut reply = Self::method_return(call, Body::from_values(&[Value::Str(detail.into())])?);
        reply.header.kind = MessageType::Error;
        reply.header.error_name = Some(name.into());
        Ok(reply)
    }
    /// Convert an error reply to Error::Remote, preserving its name and detail.
    pub fn into_result(self) -> Result<Self> {
        if self.header.kind == MessageType::Error {
            let detail = if self.body.signature().starts_with('s') {
                self.body.reader().str()?.to_owned()
            } else {
                String::new()
            };
            Err(Error::Remote {
                name: self.header.error_name.unwrap_or_default(),
                detail,
            })
        } else {
            Ok(self)
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.header.validate()?;
        let converted;
        let body = if self.body.endian == Endian::Little {
            &self.body
        } else {
            converted = Body::from_values(&self.body.values()?)?;
            &converted
        };
        let mut writer = Writer::new();
        writer
            .buf
            .extend_from_slice(&[b'l', self.header.kind as u8, self.header.flags, 1]);
        writer.u32(u32::try_from(body.bytes.len()).map_err(|_| Error::invalid("body too long"))?);
        writer.u32(self.header.serial);
        writer.u32(0);
        let mut field = |code: u8, value: Value| -> Result<()> {
            writer.begin_struct();
            writer.byte(code);
            writer.variant(&value.signature()?, |w| w.value(&value))
        };
        let h = &self.header;
        if let Some(v) = &h.path {
            field(1, Value::Path(v.clone()))?;
        }
        if let Some(v) = &h.interface {
            field(2, Value::Str(v.clone()))?;
        }
        if let Some(v) = &h.member {
            field(3, Value::Str(v.clone()))?;
        }
        if let Some(v) = &h.error_name {
            field(4, Value::Str(v.clone()))?;
        }
        if let Some(v) = h.reply_serial {
            field(5, Value::U32(v))?;
        }
        if let Some(v) = &h.destination {
            field(6, Value::Str(v.clone()))?;
        }
        if let Some(v) = &h.sender {
            field(7, Value::Str(v.clone()))?;
        }
        if !body.signature.is_empty() {
            field(8, Value::Sig(body.signature.clone()))?;
        }
        if h.unix_fds != 0 {
            field(9, Value::U32(h.unix_fds))?;
        }
        let mut seen = [false; 256];
        for (code, value) in &h.unknown {
            if *code <= 9 || seen[*code as usize] {
                return Err(Error::invalid("invalid or duplicate extension field"));
            }
            seen[*code as usize] = true;
            field(*code, value.clone())?;
        }
        let len = writer.buf.len() - 16;
        if len > crate::MAX_ARRAY {
            return Err(Error::invalid("header array too long"));
        }
        writer.buf[12..16].copy_from_slice(&(len as u32).to_le_bytes());
        writer.pad(8);
        if writer.buf.len() + body.bytes.len() > MAX_MESSAGE {
            return Err(Error::invalid("message exceeds D-Bus maximum"));
        }
        writer.buf.extend_from_slice(&body.bytes);
        writer.into_bytes()
    }
    /// Total frame size once the fixed header is available. No allocation.
    pub fn frame_len(bytes: &[u8]) -> Result<Option<usize>> {
        if bytes.len() < 16 {
            return Ok(None);
        }
        let endian = byte_order(bytes[0])?;
        if bytes[3] != 1 {
            return Err(Error::invalid("unsupported D-Bus version"));
        }
        let mut reader = Reader::new(bytes, endian);
        reader.pos = 4;
        let body = reader.u32()? as usize;
        if reader.u32()? == 0 {
            return Err(Error::invalid("zero message serial"));
        }
        let fields = reader.u32()? as usize;
        if fields > crate::MAX_ARRAY {
            return Err(Error::invalid("header array too long"));
        }
        let total = (16 + fields).div_ceil(8) * 8;
        let total = total
            .checked_add(body)
            .filter(|n| *n <= MAX_MESSAGE)
            .ok_or_else(|| Error::invalid("message exceeds D-Bus maximum"))?;
        Ok(Some(total))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let total = Self::frame_len(bytes)?.ok_or_else(|| Error::invalid("short fixed header"))?;
        if total != bytes.len() {
            return Err(Error::invalid("frame length mismatch"));
        }
        let endian = byte_order(bytes[0])?;
        let kind = match bytes[1] {
            1 => MessageType::MethodCall,
            2 => MessageType::MethodReturn,
            3 => MessageType::Error,
            4 => MessageType::Signal,
            _ => return Err(Error::invalid("unknown message type")),
        };
        let mut reader = Reader::new(bytes, endian);
        reader.pos = 8;
        let mut header = Header::new(kind);
        header.flags = bytes[2];
        header.serial = reader.u32()?;
        let fields_end = 16 + reader.u32()? as usize;
        reader = Reader::new(&bytes[..fields_end], endian);
        reader.pos = 16;
        let mut signature = String::new();
        let mut seen = [false; 256];
        while !reader.is_empty() {
            reader.begin_struct()?;
            let code = reader.byte()?;
            if code == 0 || seen[code as usize] {
                return Err(Error::invalid("invalid or duplicate header field"));
            }
            seen[code as usize] = true;
            let sig = reader.variant_signature()?;
            match code {
                1 if sig == "o" => header.path = Some(reader.object_path()?.into()),
                2 if sig == "s" => header.interface = Some(reader.str()?.into()),
                3 if sig == "s" => header.member = Some(reader.str()?.into()),
                4 if sig == "s" => header.error_name = Some(reader.str()?.into()),
                5 if sig == "u" => header.reply_serial = Some(reader.u32()?),
                6 if sig == "s" => header.destination = Some(reader.str()?.into()),
                7 if sig == "s" => header.sender = Some(reader.str()?.into()),
                8 if sig == "g" => signature = reader.signature()?.into(),
                9 if sig == "u" => header.unix_fds = reader.u32()?,
                1..=9 => return Err(Error::invalid("header field has wrong type")),
                _ => header.unknown.push((code, reader.value(sig)?)),
            }
        }
        header.validate()?;
        Signature::new(&signature)?;
        let mut reader = Reader::new(bytes, endian);
        reader.pos = fields_end;
        reader.align(8)?;
        let body = Body::from_bytes(&signature, bytes[reader.pos..].to_vec(), endian)?;
        Ok(Self { header, body })
    }
}

fn byte_order(byte: u8) -> Result<Endian> {
    match byte {
        b'l' => Ok(Endian::Little),
        b'B' => Ok(Endian::Big),
        _ => Err(Error::invalid("invalid byte order")),
    }
}
