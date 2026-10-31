//! The public "binn" binary serialization (typed values and containers),
//! as used inside a patch's compressed block storage. Read into a value
//! tree and written back the same way. Multi-byte numbers are big-endian;
//! container, string and blob sizes take 1 byte (<= 127), or 4 bytes with
//! the top bit set.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    Str(String),
    Blob(Vec<u8>),
    List(Vec<Value>),
    Map(Vec<(i32, Value)>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn blob(&self) -> Option<&[u8]> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }
}

const LIST: u8 = 0xE0;
const MAP: u8 = 0xE1;
const OBJECT: u8 = 0xE2;

struct R<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> R<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.i).ok_or("binn: truncated")?;
        self.i += 1;
        Ok(v)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.i + n > self.b.len() {
            return Err("binn: truncated".into());
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn be(&mut self, n: usize) -> Result<u64, String> {
        Ok(self.bytes(n)?.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
    }
    /// 1-byte size, or 4 bytes with the top bit set.
    fn size(&mut self) -> Result<usize, String> {
        let first = *self.b.get(self.i).ok_or("binn: truncated")?;
        if first & 0x80 != 0 {
            Ok((self.be(4)? & 0x7FFF_FFFF) as usize)
        } else {
            self.i += 1;
            Ok(first as usize)
        }
    }
}

/// Parse one value (a container at the root, normally).
pub fn parse(b: &[u8]) -> Result<Value, String> {
    let mut r = R { b, i: 0 };
    value(&mut r, 0)
}

fn value(r: &mut R, depth: usize) -> Result<Value, String> {
    if depth > 64 {
        return Err("binn: too deep".into());
    }
    let mut t = r.u8()? as u32;
    let storage = t & 0xE0;
    // Extended subtype: a second type byte.
    if t & 0x10 != 0 {
        t = (t << 8) | r.u8()? as u32;
    }
    let sub = t;
    Ok(match storage {
        0x00 => match sub {
            0x01 => Value::Bool(true),
            0x02 => Value::Bool(false),
            _ => Value::Null,
        },
        0x20 => {
            let v = r.u8()?;
            if sub == 0x21 { Value::Int(v as i8 as i64) } else { Value::UInt(v as u64) }
        }
        0x40 => {
            let v = r.be(2)? as u16;
            if sub == 0x41 { Value::Int(v as i16 as i64) } else { Value::UInt(v as u64) }
        }
        0x60 => {
            let v = r.be(4)? as u32;
            match sub {
                0x61 => Value::Int(v as i32 as i64),
                0x62 => Value::F32(f32::from_bits(v)),
                _ => Value::UInt(v as u64),
            }
        }
        0x80 => {
            let v = r.be(8)?;
            match sub {
                0x81 => Value::Int(v as i64),
                0x82 => Value::F64(f64::from_bits(v)),
                _ => Value::UInt(v),
            }
        }
        0xA0 => {
            let n = r.size()?;
            let s = r.bytes(n)?;
            let _nul = r.u8();
            Value::Str(String::from_utf8_lossy(s).into_owned())
        }
        0xC0 => {
            let n = r.size()?;
            Value::Blob(r.bytes(n)?.to_vec())
        }
        0xE0 => {
            let start = r.i - 1;
            let size = r.size()?;
            let count = r.size()?;
            let end = start + size;
            if end > r.b.len() {
                return Err("binn: container past the end".into());
            }
            let v = match sub {
                LIST_SUB => {
                    let mut items = Vec::with_capacity(count.min(4096));
                    for _ in 0..count {
                        items.push(value(r, depth + 1)?);
                    }
                    Value::List(items)
                }
                MAP_SUB => {
                    let mut items = Vec::with_capacity(count.min(4096));
                    for _ in 0..count {
                        let k = r.be(4)? as u32 as i32;
                        items.push((k, value(r, depth + 1)?));
                    }
                    Value::Map(items)
                }
                _ => {
                    let mut items = Vec::with_capacity(count.min(4096));
                    for _ in 0..count {
                        let n = r.u8()? as usize;
                        let k = String::from_utf8_lossy(r.bytes(n)?).into_owned();
                        items.push((k, value(r, depth + 1)?));
                    }
                    Value::Object(items)
                }
            };
            r.i = end.max(r.i);
            v
        }
        _ => return Err(format!("binn: unknown type {t:#x}")),
    })
}

const LIST_SUB: u32 = LIST as u32;
const MAP_SUB: u32 = MAP as u32;

fn put_size(out: &mut Vec<u8>, n: usize) {
    if n < 128 {
        out.push(n as u8);
    } else {
        out.extend_from_slice(&((n as u32) | 0x8000_0000).to_be_bytes());
    }
}

/// Serialize a value (containers with their size headers).
pub fn write(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    put(v, &mut out);
    out
}

fn put(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(0x00),
        Value::Bool(b) => out.push(if *b { 0x01 } else { 0x02 }),
        Value::Int(i) => {
            out.push(0x81);
            out.extend_from_slice(&i.to_be_bytes());
        }
        Value::UInt(u) => {
            out.push(0x80);
            out.extend_from_slice(&u.to_be_bytes());
        }
        Value::F32(f) => {
            out.push(0x62);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Value::F64(f) => {
            out.push(0x82);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Value::Str(s) => {
            out.push(0xA0);
            put_size(out, s.len());
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        Value::Blob(b) => {
            out.push(0xC0);
            put_size(out, b.len());
            out.extend_from_slice(b);
        }
        Value::List(items) => container(out, LIST, items.len(), |body| items.iter().for_each(|x| put(x, body))),
        Value::Map(items) => container(out, MAP, items.len(), |body| {
            for (k, x) in items {
                body.extend_from_slice(&k.to_be_bytes());
                put(x, body);
            }
        }),
        Value::Object(items) => container(out, OBJECT, items.len(), |body| {
            for (k, x) in items {
                let k = &k.as_bytes()[..k.len().min(255)];
                body.push(k.len() as u8);
                body.extend_from_slice(k);
                put(x, body);
            }
        }),
    }
}

fn container(out: &mut Vec<u8>, ty: u8, count: usize, fill: impl FnOnce(&mut Vec<u8>)) {
    let mut body = Vec::new();
    fill(&mut body);
    let mut count_bytes = Vec::new();
    put_size(&mut count_bytes, count);
    // The size covers the whole container, header included.
    let small = 1 + 1 + count_bytes.len() + body.len();
    let size = if small < 128 { small } else { 1 + 4 + count_bytes.len() + body.len() };
    out.push(ty);
    put_size(out, size);
    out.extend_from_slice(&count_bytes);
    out.extend_from_slice(&body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = Value::Object(vec![
            ("fx3".into(), Value::Object(vec![("irname".into(), Value::Blob(b"room".to_vec())), ("left".into(), Value::Blob(vec![0; 300]))])),
            ("osc_0_1".into(), Value::Object(vec![("snap_0".into(), Value::List(vec![Value::Blob(vec![1, 2, 3, 4])]))])),
            ("n".into(), Value::Int(-5)),
            ("s".into(), Value::Str("x".repeat(200))),
            ("f".into(), Value::F32(0.5)),
        ]);
        let b = write(&v);
        assert_eq!(parse(&b).unwrap(), v);
    }
}
