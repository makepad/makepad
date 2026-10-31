//! Channel 0, the control channel: HELLO / HELLO_OK / RESUME / GOODBYE. Each is a DATA frame on
//! channel 0 whose first byte is the opcode, followed by `key=value` lines (utf8).

use crate::frame::{kind, write_frame, FrameReader};
use std::io::{self, Write};

pub const PROTO_MAJOR: u16 = 1;
pub const PROTO_MINOR: u16 = 0;

pub const OP_HELLO: u8 = 1;
pub const OP_HELLO_OK: u8 = 2;
pub const OP_RESUME: u8 = 3;
pub const OP_GOODBYE: u8 = 4;

/// What a side tells the other about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hello {
    pub major: u16,
    pub minor: u16,
    /// "client" / "endpoint"
    pub role: String,
    pub name: String,
    /// identifies a client across reconnects (RESUME refers to it)
    pub link_id: String,
    /// `service/version` of every service served (HELLO_OK only)
    pub caps: Vec<String>,
    /// the shared secret of a loopback carrier (in HELLO; an endpoint's own value is the one it
    /// requires)
    pub token: String,
}

fn clean(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn encode(op: u8, h: &Hello) -> Vec<u8> {
    let mut s = format!("major={}\nminor={}\nrole={}\nname={}\nlink_id={}\n", h.major, h.minor, clean(&h.role), clean(&h.name), clean(&h.link_id));
    if op == OP_HELLO && !h.token.is_empty() {
        s.push_str(&format!("token={}\n", clean(&h.token)));
    }
    for c in &h.caps {
        s.push_str(&format!("cap={}\n", clean(c)));
    }
    let mut v = vec![op];
    v.extend_from_slice(s.as_bytes());
    v
}

fn decode(body: &[u8]) -> Hello {
    let text = String::from_utf8_lossy(body);
    let mut h = Hello::default();
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        match k {
            "major" => h.major = v.parse().unwrap_or(0),
            "minor" => h.minor = v.parse().unwrap_or(0),
            "role" => h.role = v.into(),
            "name" => h.name = v.into(),
            "link_id" => h.link_id = v.into(),
            "token" => h.token = v.into(),
            "cap" => h.caps.push(v.into()),
            _ => {}
        }
    }
    h
}

/// Per-service last sequence number a reconnecting client saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resume {
    pub link_id: String,
    pub seqs: Vec<(String, u64)>,
}

pub fn encode_resume(r: &Resume) -> Vec<u8> {
    let mut s = format!("link_id={}\n", clean(&r.link_id));
    for (svc, seq) in &r.seqs {
        s.push_str(&format!("seq={}:{}\n", clean(svc), seq));
    }
    let mut v = vec![OP_RESUME];
    v.extend_from_slice(s.as_bytes());
    v
}

pub fn decode_resume(body: &[u8]) -> Resume {
    let text = String::from_utf8_lossy(body);
    let mut r = Resume::default();
    for l in text.lines() {
        match l.split_once('=') {
            Some(("link_id", v)) => r.link_id = v.into(),
            Some(("seq", v)) => {
                if let Some((svc, n)) = v.rsplit_once(':') {
                    if let Ok(n) = n.parse() {
                        r.seqs.push((svc.into(), n));
                    }
                }
            }
            _ => {}
        }
    }
    r
}

pub fn encode_goodbye(reason: &str) -> Vec<u8> {
    let mut v = vec![OP_GOODBYE];
    v.extend_from_slice(reason.as_bytes());
    v
}

fn version_message(own: (u16, u16), peer: (u16, u16)) -> String {
    format!("makepad-remote protocol major version differs: this side speaks {}.{}, the peer speaks {}.{}", own.0, own.1, peer.0, peer.1)
}

#[derive(Debug)]
pub enum HandshakeError {
    /// The peer refused (GOODBYE) with this message.
    Refused(String),
    /// This side refused the peer (already told it).
    Mismatch(String),
    Io(io::Error),
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Refused(m) | HandshakeError::Mismatch(m) => f.write_str(m),
            HandshakeError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for HandshakeError {
    fn from(e: io::Error) -> Self {
        HandshakeError::Io(e)
    }
}

fn next_control(r: &mut FrameReader) -> Result<Vec<u8>, HandshakeError> {
    loop {
        let f = r.next()?;
        if f.chan == 0 && f.kind == kind::DATA && !f.payload.is_empty() {
            return Ok(f.payload);
        }
    }
}

/// Client side: sends HELLO, waits for HELLO_OK.
pub fn client_handshake(r: &mut FrameReader, w: &mut dyn Write, hello: &Hello) -> Result<Hello, HandshakeError> {
    let mine = Hello { major: PROTO_MAJOR, minor: PROTO_MINOR, role: "client".into(), ..hello.clone() };
    write_frame(w, 0, kind::DATA, 0, &encode(OP_HELLO, &mine))?;
    w.flush()?;
    let p = next_control(r)?;
    match p[0] {
        OP_HELLO_OK => {
            let ok = decode(&p[1..]);
            if ok.major != PROTO_MAJOR {
                return Err(HandshakeError::Mismatch(version_message((PROTO_MAJOR, PROTO_MINOR), (ok.major, ok.minor))));
            }
            Ok(ok)
        }
        OP_GOODBYE => Err(HandshakeError::Refused(String::from_utf8_lossy(&p[1..]).into_owned())),
        op => Err(HandshakeError::Io(io::Error::new(io::ErrorKind::InvalidData, format!("unexpected control opcode {op} during hello")))),
    }
}

/// Equal, without stopping at the first difference.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Endpoint side: reads HELLO, answers HELLO_OK with `own` (its name and caps) or refuses a
/// different major version with a message naming both.
pub fn server_handshake(r: &mut FrameReader, w: &mut dyn Write, own: &Hello) -> Result<Hello, HandshakeError> {
    let p = next_control(r)?;
    if p[0] != OP_HELLO {
        return Err(HandshakeError::Io(io::Error::new(io::ErrorKind::InvalidData, "the peer did not start with HELLO")));
    }
    let peer = decode(&p[1..]);
    if peer.major != PROTO_MAJOR {
        let msg = version_message((PROTO_MAJOR, PROTO_MINOR), (peer.major, peer.minor));
        let _ = write_frame(w, 0, kind::DATA, 0, &encode_goodbye(&msg));
        let _ = w.flush();
        return Err(HandshakeError::Mismatch(msg));
    }
    if !own.token.is_empty() && !same_secret(&own.token, &peer.token) {
        let msg = "this endpoint requires the token from its discovery file".to_string();
        let _ = write_frame(w, 0, kind::DATA, 0, &encode_goodbye(&msg));
        let _ = w.flush();
        return Err(HandshakeError::Mismatch(msg));
    }
    let mine = Hello { major: PROTO_MAJOR, minor: PROTO_MINOR, role: "endpoint".into(), link_id: peer.link_id.clone(), token: String::new(), ..own.clone() };
    write_frame(w, 0, kind::DATA, 0, &encode(OP_HELLO_OK, &mine))?;
    w.flush()?;
    Ok(peer)
}
