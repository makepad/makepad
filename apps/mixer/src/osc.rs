//! Minimal OSC 1.0 codec — exactly what the mixer dialect needs.
//!
//! Wire format (OSC 1.0, big-endian):
//!   address  NUL-terminated ASCII starting with '/', padded to 4 bytes
//!   typetag  ','-prefixed string, one char per arg, NUL-terminated, padded
//!   args     each 4-byte aligned; i=int32 BE, f=float32 BE, s=string,
//!            b=blob (BE int32 byte length + bytes + pad)
//!
//! A message with NO arguments is a query (GET) in this dialect; a message
//! WITH arguments is a SET. Encoding therefore always goes through the
//! safety layer — this module is deliberately dumb about meaning.

#[derive(Clone, Debug, PartialEq)]
pub enum OscArg {
    I(i32),
    F(f32),
    S(String),
    B(Vec<u8>),
}

impl OscArg {
    pub fn type_tag(&self) -> u8 {
        match self {
            OscArg::I(_) => b'i',
            OscArg::F(_) => b'f',
            OscArg::S(_) => b's',
            OscArg::B(_) => b'b',
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscMsg {
    pub addr: String,
    pub args: Vec<OscArg>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OscErr {
    Truncated,
    BadAddress,
    BadTypeTag(u8),
    BadString,
}

fn push_padded_str(out: &mut Vec<u8>, s: &str) {
    let start = out.len();
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    while (out.len() - start) % 4 != 0 {
        out.push(0);
    }
}

impl OscMsg {
    pub fn query(addr: &str) -> OscMsg {
        OscMsg { addr: addr.to_string(), args: Vec::new() }
    }

    pub fn with_args(addr: &str, args: Vec<OscArg>) -> OscMsg {
        OscMsg { addr: addr.to_string(), args }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        push_padded_str(&mut out, &self.addr);
        let mut tags = String::with_capacity(self.args.len() + 1);
        tags.push(',');
        for a in &self.args {
            tags.push(a.type_tag() as char);
        }
        push_padded_str(&mut out, &tags);
        for a in &self.args {
            match a {
                OscArg::I(v) => out.extend_from_slice(&v.to_be_bytes()),
                OscArg::F(v) => out.extend_from_slice(&v.to_be_bytes()),
                OscArg::S(v) => push_padded_str(&mut out, v),
                OscArg::B(v) => {
                    out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                    out.extend_from_slice(v);
                    while out.len() % 4 != 0 {
                        out.push(0);
                    }
                }
            }
        }
        out
    }

    /// Tolerant decode: a missing type tag string is treated as "no args"
    /// (the console accepts and produces the older notation).
    pub fn decode(bytes: &[u8]) -> Result<OscMsg, OscErr> {
        let (addr, mut pos) = read_padded_str(bytes, 0)?;
        if !addr.starts_with('/') {
            return Err(OscErr::BadAddress);
        }
        if pos >= bytes.len() || bytes[pos] != b',' {
            return Ok(OscMsg { addr, args: Vec::new() });
        }
        let (tags, tag_end) = read_padded_str(bytes, pos)?;
        pos = tag_end;
        let mut args = Vec::new();
        for t in tags.bytes().skip(1) {
            match t {
                b'i' => {
                    let v = read_be_i32(bytes, pos).ok_or(OscErr::Truncated)?;
                    args.push(OscArg::I(v));
                    pos += 4;
                }
                b'f' => {
                    let v = read_be_i32(bytes, pos).ok_or(OscErr::Truncated)?;
                    args.push(OscArg::F(f32::from_bits(v as u32)));
                    pos += 4;
                }
                b's' => {
                    let (s, end) = read_padded_str(bytes, pos)?;
                    args.push(OscArg::S(s));
                    pos = end;
                }
                b'b' => {
                    let len = read_be_i32(bytes, pos).ok_or(OscErr::Truncated)? as usize;
                    pos += 4;
                    if pos + len > bytes.len() {
                        return Err(OscErr::Truncated);
                    }
                    args.push(OscArg::B(bytes[pos..pos + len].to_vec()));
                    pos += (len + 3) & !3; // blob content pads to 4, no NUL of its own
                }
                other => return Err(OscErr::BadTypeTag(other)),
            }
        }
        Ok(OscMsg { addr, args })
    }
}

/// Reads just the address out of an encoded packet. The transmit guard uses
/// this on the exact bytes about to hit the socket, so the check cannot be
/// bypassed by lying metadata.
pub fn peek_address(bytes: &[u8]) -> Result<String, OscErr> {
    let (addr, _) = read_padded_str(bytes, 0)?;
    if !addr.starts_with('/') {
        return Err(OscErr::BadAddress);
    }
    Ok(addr)
}

/// Whether the encoded packet carries any argument. In this dialect a
/// message WITH an argument is a SET — the read-only transmit gate refuses
/// those wholesale, judged from the exact outgoing bytes.
pub fn peek_has_args(bytes: &[u8]) -> Result<bool, OscErr> {
    let (_, pos) = read_padded_str(bytes, 0)?;
    if pos >= bytes.len() || bytes[pos] != b',' {
        return Ok(false);
    }
    let (tags, _) = read_padded_str(bytes, pos)?;
    Ok(tags.len() > 1)
}

fn read_be_i32(bytes: &[u8], pos: usize) -> Option<i32> {
    let b = bytes.get(pos..pos + 4)?;
    Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_padded_str(bytes: &[u8], start: usize) -> Result<(String, usize), OscErr> {
    let rel = bytes
        .get(start..)
        .and_then(|b| b.iter().position(|&c| c == 0))
        .ok_or(OscErr::Truncated)?;
    let s = std::str::from_utf8(&bytes[start..start + rel])
        .map_err(|_| OscErr::BadString)?
        .to_string();
    let mut end = start + rel + 1;
    while end % 4 != 0 {
        end += 1;
    }
    Ok((s, end.min(bytes.len().max(start + rel + 1))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_no_args() {
        let m = OscMsg::query("/ch/01/mix/fader");
        let d = OscMsg::decode(&m.encode()).unwrap();
        assert_eq!(d.addr, "/ch/01/mix/fader");
        assert!(d.args.is_empty());
    }

    #[test]
    fn roundtrip_float_int_string() {
        let m = OscMsg::with_args(
            "/x",
            vec![OscArg::F(0.75), OscArg::I(1), OscArg::S("hello".into())],
        );
        let d = OscMsg::decode(&m.encode()).unwrap();
        assert_eq!(d, m);
    }

    #[test]
    fn peek_matches_decode() {
        let m = OscMsg::with_args("/lr/mix/on", vec![OscArg::I(1)]);
        assert_eq!(peek_address(&m.encode()).unwrap(), "/lr/mix/on");
    }

    #[test]
    fn official_meters_example_bytes() {
        // /meters ,si "/meters/0" 8 — hex layout from the official 4-pager.
        let m = OscMsg::with_args(
            "/meters",
            vec![OscArg::S("/meters/0".into()), OscArg::I(8)],
        );
        let bytes = m.encode();
        let expect: &[u8] = &[
            0x2f, 0x6d, 0x65, 0x74, 0x65, 0x72, 0x73, 0x00, 0x2c, 0x73, 0x69, 0x00, 0x2f, 0x6d,
            0x65, 0x74, 0x65, 0x72, 0x73, 0x2f, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08,
        ];
        assert_eq!(&bytes[..], expect);
    }

    #[test]
    fn rejects_bad_address() {
        assert!(peek_address(b"notaddr\0").is_err());
    }

    #[test]
    fn roundtrip_blob() {
        let m = OscMsg::with_args(
            "/meters/1",
            vec![OscArg::B(vec![1, 2, 3, 4, 5])],
        );
        let d = OscMsg::decode(&m.encode()).unwrap();
        assert_eq!(d, m);
        // and a trailing arg after the blob survives the padding math
        let m2 = OscMsg::with_args(
            "/meters/1",
            vec![OscArg::B(vec![9, 9, 9]), OscArg::I(7)],
        );
        assert_eq!(OscMsg::decode(&m2.encode()).unwrap(), m2);
    }
}
