use crate::signature::{alignment, element, types};
use crate::{validate_object_path, Error, Result, Signature, MAX_ARRAY, MAX_DEPTH};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Endian {
    #[default]
    Little,
    Big,
}

/// Arrays carry their element signature, including when empty. Dictionaries are
/// arrays of DictEntry values, e.g. element signature `{sv}`.
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
    /// Wire index only; the connection does not negotiate descriptor passing.
    Fd(u32),
    Array {
        element: String,
        values: Vec<Value>,
    },
    Struct(Vec<Value>),
    DictEntry(Box<(Value, Value)>),
    Variant(Box<Value>),
}

impl Value {
    pub fn signature(&self) -> Result<String> {
        self.signature_at(0)
    }

    fn signature_at(&self, depth: usize) -> Result<String> {
        if depth > MAX_DEPTH {
            return Err(Error::invalid("values nested too deep"));
        }
        let sig = match self {
            Self::Byte(_) => "y".into(),
            Self::Bool(_) => "b".into(),
            Self::I16(_) => "n".into(),
            Self::U16(_) => "q".into(),
            Self::I32(_) => "i".into(),
            Self::U32(_) => "u".into(),
            Self::I64(_) => "x".into(),
            Self::U64(_) => "t".into(),
            Self::F64(_) => "d".into(),
            Self::Str(_) => "s".into(),
            Self::Path(_) => "o".into(),
            Self::Sig(_) => "g".into(),
            Self::Fd(_) => "h".into(),
            Self::Variant(_) => "v".into(),
            Self::Array { element: sig, .. } => {
                element(sig)?;
                format!("a{sig}")
            }
            Self::Struct(fields) => {
                let mut sig = String::from("(");
                for field in fields {
                    sig.push_str(&field.signature_at(depth + 1)?);
                }
                sig.push(')');
                sig
            }
            Self::DictEntry(pair) => format!(
                "{{{}{}}}",
                pair.0.signature_at(depth + 1)?,
                pair.1.signature_at(depth + 1)?
            ),
        };
        // A dict entry is valid only as an array element.
        if matches!(self, Self::DictEntry(_)) {
            element(&sig)?;
        } else {
            Signature::single(&sig)?;
        }
        Ok(sig)
    }

    pub fn inner(&self) -> &Self {
        let mut value = self;
        while let Self::Variant(inner) = value {
            value = inner;
        }
        value
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.inner() {
            Self::Str(s) | Self::Path(s) | Self::Sig(s) => Some(s),
            _ => None,
        }
    }
}

/// Little-endian body writer. Primitive writes do not allocate dynamic Values.
/// Container tokens must be ended in reverse order on the writer that made them.
#[derive(Default, Debug)]
pub struct Writer {
    pub(crate) buf: Vec<u8>,
    arrays: Vec<usize>,
    depth: usize,
}

#[derive(Debug)]
pub struct ArrayToken {
    length_at: usize,
    start: usize,
}

macro_rules! write_number {
    ($name:ident, $ty:ty) => {
        pub fn $name(&mut self, value: $ty) {
            self.pad(std::mem::size_of::<$ty>());
            self.buf.extend_from_slice(&value.to_le_bytes());
        }
    };
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        if !self.arrays.is_empty() {
            return Err(Error::invalid("unclosed array"));
        }
        Ok(self.buf)
    }
    pub(crate) fn pad(&mut self, align: usize) {
        self.buf.resize(self.buf.len().div_ceil(align) * align, 0);
    }
    pub fn byte(&mut self, value: u8) {
        self.buf.push(value);
    }
    pub fn bool(&mut self, value: bool) {
        self.u32(u32::from(value));
    }
    write_number!(i16, i16);
    write_number!(u16, u16);
    write_number!(i32, i32);
    write_number!(u32, u32);
    write_number!(i64, i64);
    write_number!(u64, u64);
    write_number!(f64, f64);
    pub fn fd_index(&mut self, value: u32) {
        self.u32(value);
    }
    pub fn str(&mut self, value: &str) -> Result<()> {
        if value.contains('\0') {
            return Err(Error::invalid("string contains NUL"));
        }
        let len = u32::try_from(value.len()).map_err(|_| Error::invalid("string too long"))?;
        self.u32(len);
        self.buf.extend_from_slice(value.as_bytes());
        self.byte(0);
        Ok(())
    }
    pub fn object_path(&mut self, value: &str) -> Result<()> {
        validate_object_path(value)?;
        self.str(value)
    }
    pub fn signature(&mut self, value: &str) -> Result<()> {
        Signature::new(value)?;
        self.byte(value.len() as u8);
        self.buf.extend_from_slice(value.as_bytes());
        self.byte(0);
        Ok(())
    }
    pub fn begin_struct(&mut self) {
        self.pad(8);
    }
    pub fn begin_dict_entry(&mut self) {
        self.pad(8);
    }
    pub fn begin_array(&mut self, sig: &str) -> Result<ArrayToken> {
        element(sig)?;
        self.pad(4);
        let length_at = self.buf.len();
        self.u32(0);
        self.pad(alignment(sig.as_bytes()[0]));
        self.arrays.push(length_at);
        Ok(ArrayToken {
            length_at,
            start: self.buf.len(),
        })
    }
    pub fn end_array(&mut self, token: ArrayToken) -> Result<()> {
        if self.arrays.last() != Some(&token.length_at) || token.start > self.buf.len() {
            return Err(Error::invalid("array token out of order"));
        }
        let len = self.buf.len() - token.start;
        if len > MAX_ARRAY {
            return Err(Error::invalid("array exceeds D-Bus maximum"));
        }
        self.buf[token.length_at..token.length_at + 4].copy_from_slice(&(len as u32).to_le_bytes());
        self.arrays.pop();
        Ok(())
    }
    pub fn variant(
        &mut self,
        sig: &str,
        write: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        Signature::single(sig)?;
        self.signature(sig)?;
        write(self)
    }
    pub fn value(&mut self, value: &Value) -> Result<()> {
        if self.depth > MAX_DEPTH {
            return Err(Error::invalid("values nested too deep"));
        }
        self.depth += 1;
        let result = self.value_inner(value);
        self.depth -= 1;
        result
    }
    fn value_inner(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Byte(v) => self.byte(*v),
            Value::Bool(v) => self.bool(*v),
            Value::I16(v) => self.i16(*v),
            Value::U16(v) => self.u16(*v),
            Value::I32(v) => self.i32(*v),
            Value::U32(v) => self.u32(*v),
            Value::I64(v) => self.i64(*v),
            Value::U64(v) => self.u64(*v),
            Value::F64(v) => self.f64(*v),
            Value::Fd(v) => self.fd_index(*v),
            Value::Str(v) => self.str(v)?,
            Value::Path(v) => self.object_path(v)?,
            Value::Sig(v) => self.signature(v)?,
            Value::Array { element, values } => {
                let token = self.begin_array(element)?;
                for value in values {
                    if value.signature()? != *element {
                        return Err(Error::invalid("array element type mismatch"));
                    }
                    self.value(value)?;
                }
                self.end_array(token)?;
            }
            Value::Struct(fields) => {
                self.begin_struct();
                for value in fields {
                    self.value(value)?;
                }
            }
            Value::DictEntry(pair) => {
                self.begin_dict_entry();
                self.value(&pair.0)?;
                self.value(&pair.1)?;
            }
            Value::Variant(inner) => self.variant(&inner.signature()?, |w| w.value(inner))?,
        }
        Ok(())
    }
}

/// Borrowing typed reader. Array subreaders retain the message's alignment origin
/// and cannot read past their array. `finish` rejects unconsumed data.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pub(crate) pos: usize,
    endian: Endian,
    depth: usize,
}

macro_rules! read_number {
    ($name:ident, $ty:ty, $n:literal) => {
        pub fn $name(&mut self) -> Result<$ty> {
            self.align($n)?;
            let bytes = self.take($n)?.try_into().unwrap();
            Ok(match self.endian {
                Endian::Little => <$ty>::from_le_bytes(bytes),
                Endian::Big => <$ty>::from_be_bytes(bytes),
            })
        }
    };
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], endian: Endian) -> Self {
        Self {
            data,
            pos: 0,
            endian,
            depth: 0,
        }
    }
    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn is_empty(&self) -> bool {
        self.pos == self.data.len()
    }
    pub fn finish(self) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(Error::invalid("unconsumed body or array bytes"))
        }
    }
    pub(crate) fn align(&mut self, align: usize) -> Result<()> {
        let count = (align - self.pos % align) % align;
        if self.take(count)?.iter().any(|b| *b != 0) {
            return Err(Error::invalid("nonzero alignment padding"));
        }
        Ok(())
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| Error::invalid("message ends early"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        match self.u32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::invalid("invalid boolean")),
        }
    }
    read_number!(i16, i16, 2);
    read_number!(u16, u16, 2);
    read_number!(i32, i32, 4);
    read_number!(u32, u32, 4);
    read_number!(i64, i64, 8);
    read_number!(u64, u64, 8);
    read_number!(f64, f64, 8);
    pub fn fd_index(&mut self) -> Result<u32> {
        self.u32()
    }
    fn text(&mut self, len: usize) -> Result<&'a str> {
        let bytes = self.take(len)?;
        if self.byte()? != 0 || bytes.contains(&0) {
            return Err(Error::invalid("invalid string terminator or embedded NUL"));
        }
        std::str::from_utf8(bytes).map_err(|_| Error::invalid("string is not UTF-8"))
    }
    pub fn str(&mut self) -> Result<&'a str> {
        let len = self.u32()? as usize;
        self.text(len)
    }
    pub fn object_path(&mut self) -> Result<&'a str> {
        let value = self.str()?;
        validate_object_path(value)?;
        Ok(value)
    }
    pub fn signature(&mut self) -> Result<&'a str> {
        let len = self.byte()? as usize;
        let value = self.text(len)?;
        Signature::new(value)?;
        Ok(value)
    }
    pub fn variant_signature(&mut self) -> Result<&'a str> {
        let sig = self.signature()?;
        Signature::single(sig)?;
        Ok(sig)
    }
    pub fn begin_struct(&mut self) -> Result<()> {
        self.align(8)
    }
    pub fn begin_dict_entry(&mut self) -> Result<()> {
        self.align(8)
    }
    pub fn array(&mut self, sig: &str) -> Result<Self> {
        element(sig)?;
        let len = self.u32()? as usize;
        if len > MAX_ARRAY {
            return Err(Error::invalid("array exceeds D-Bus maximum"));
        }
        self.align(alignment(sig.as_bytes()[0]))?;
        let start = self.pos;
        self.take(len)?;
        Ok(Self {
            data: &self.data[..self.pos],
            pos: start,
            endian: self.endian,
            depth: self.depth,
        })
    }
    pub fn value(&mut self, sig: &str) -> Result<Value> {
        Signature::single(sig)?;
        Ok(self.visit(sig, true)?.unwrap())
    }
    pub fn values(&mut self, sig: &str) -> Result<Vec<Value>> {
        Signature::new(sig)?
            .types()
            .map(|sig| self.value(sig))
            .collect()
    }
    /// Validate/skip without constructing a Value tree (used by message framing).
    pub fn skip(&mut self, sig: &str) -> Result<()> {
        for sig in Signature::new(sig)?.types() {
            self.visit(sig, false)?;
        }
        Ok(())
    }
    fn visit(&mut self, sig: &str, collect: bool) -> Result<Option<Value>> {
        let container = matches!(sig.as_bytes()[0], b'a' | b'(' | b'{' | b'v');
        if container && self.depth >= MAX_DEPTH {
            return Err(Error::invalid("values nested too deep"));
        }
        self.depth += usize::from(container);
        let result = self.visit_inner(sig, collect);
        self.depth -= usize::from(container);
        result
    }
    fn visit_inner(&mut self, sig: &str, collect: bool) -> Result<Option<Value>> {
        macro_rules! scalar {
            ($method:ident, $variant:ident) => {{
                let v = self.$method()?;
                collect.then(|| Value::$variant(v))
            }};
        }
        macro_rules! text {
            ($method:ident, $variant:ident) => {{
                let v = self.$method()?;
                collect.then(|| Value::$variant(v.to_owned()))
            }};
        }
        Ok(match sig.as_bytes()[0] {
            b'y' => scalar!(byte, Byte),
            b'b' => scalar!(bool, Bool),
            b'n' => scalar!(i16, I16),
            b'q' => scalar!(u16, U16),
            b'i' => scalar!(i32, I32),
            b'u' => scalar!(u32, U32),
            b'x' => scalar!(i64, I64),
            b't' => scalar!(u64, U64),
            b'd' => scalar!(f64, F64),
            b'h' => scalar!(fd_index, Fd),
            b's' => text!(str, Str),
            b'o' => text!(object_path, Path),
            b'g' => text!(signature, Sig),
            b'a' => {
                let sig = &sig[1..];
                let mut array = self.array(sig)?;
                let mut values = Vec::new();
                while !array.is_empty() {
                    if let Some(v) = array.visit(sig, collect)? {
                        values.push(v);
                    }
                }
                collect.then(|| Value::Array {
                    element: sig.to_owned(),
                    values,
                })
            }
            b'(' | b'{' => {
                self.align(8)?;
                let mut values = Vec::new();
                for sig in types(&sig[1..sig.len() - 1]) {
                    if let Some(v) = self.visit(sig, collect)? {
                        values.push(v);
                    }
                }
                if !collect {
                    None
                } else if sig.starts_with('{') {
                    let v = values.pop().unwrap();
                    Some(Value::DictEntry(Box::new((values.pop().unwrap(), v))))
                } else {
                    Some(Value::Struct(values))
                }
            }
            b'v' => {
                let sig = self.variant_signature()?;
                self.visit(sig, collect)?
                    .map(|v| Value::Variant(Box::new(v)))
            }
            _ => return Err(Error::invalid("unknown type")),
        })
    }
}
