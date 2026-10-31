//! The `frames` service: window images, produced by the app and consumed by a client that writes
//! the files. OPEN `frames` with `window=ID scale=F codec=png|raw [n=N] [fps=F] [once=1]`; each
//! DATA message is one record `u32 LE json_len | json | bytes`, the json carrying
//! `{seq, w, h, window, t_ns, codec}` (+ timings). An image never becomes a file on the app's
//! side. The app registers the service (`Endpoint::register("frames", 1, ..)`); this module is the
//! record format and the client side of it.

use crate::conn::{Event, CLOSE_NORMAL};
use crate::link::{Client, Error};

/// One frame: its header json and the image bytes (a PNG, or tightly packed RGBA for `raw`).
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub json: String,
    pub bytes: Vec<u8>,
}

pub fn encode_record(json: &str, bytes: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(4 + json.len() + bytes.len());
    m.extend_from_slice(&(json.len() as u32).to_le_bytes());
    m.extend_from_slice(json.as_bytes());
    m.extend_from_slice(bytes);
    m
}

pub fn decode_record(msg: &[u8]) -> Result<Record, String> {
    let head: [u8; 4] = msg.get(..4).and_then(|h| h.try_into().ok()).ok_or("frames record shorter than its length field")?;
    let n = u32::from_le_bytes(head) as usize;
    let json = msg.get(4..4 + n).ok_or_else(|| format!("frames record announces {n} header bytes, has {}", msg.len() - 4))?;
    let json = std::str::from_utf8(json).map_err(|_| "frames record header is not utf8")?;
    Ok(Record { json: json.to_string(), bytes: msg[4 + n..].to_vec() })
}

impl Record {
    fn raw(&self, key: &str) -> Option<String> {
        crate::discovery::fields(&self.json).into_iter().find(|f| f.0 == key).map(|f| f.1)
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        self.raw(key)?.parse().ok()
    }

    pub fn text(&self, key: &str) -> Option<String> {
        crate::discovery::string(self.raw(key)?.as_bytes(), &mut 0)
    }
}

/// What a client asks the `frames` service for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub window: Option<usize>,
    pub scale: Option<f64>,
    pub raw: bool,
    /// the codec when not png/raw (`rle`): set by `rec`
    pub codec: Option<&'static str>,
    /// a fixed number of frames; none = until the channel is closed
    pub n: Option<usize>,
    pub fps: Option<f64>,
    /// every presented frame, streamed until the client closes (`cap=1`); `fps` caps the rate
    pub cap: bool,
}

impl Request {
    /// The OPEN arguments.
    pub fn args(&self) -> String {
        let mut a = Vec::new();
        if let Some(w) = self.window {
            a.push(format!("window={w}"));
        }
        if let Some(s) = self.scale {
            a.push(format!("scale={s}"));
        }
        a.push(format!("codec={}", self.codec.unwrap_or(if self.raw { "raw" } else { "png" })));
        match self.n {
            Some(1) => a.push("once=1".into()),
            Some(n) => a.push(format!("n={n}")),
            None => {}
        }
        if let Some(f) = self.fps {
            a.push(format!("fps={f}"));
        }
        if self.cap {
            a.push("cap=1".into());
        }
        a.join(" ")
    }
}

/// Opens the service and hands every record to `each` (which may stop early with `Err`) until the
/// service ends the channel. A refusal or a failure on the app's side comes back as the text it
/// gave.
pub fn fetch(client: &Client, req: &Request, each: &mut dyn FnMut(Record) -> Result<(), String>) -> Result<(), String> {
    let ch = client.open("frames", &req.args()).map_err(|e| e.to_string())?;
    loop {
        let event = match client.timeout() {
            None => ch.recv(),
            Some(t) => match ch.recv_timeout(t) {
                Ok(e) => Some(e),
                Err(true) => return Err(Error::Timeout.to_string()),
                Err(false) => None,
            },
        };
        match event {
            Some(Event::Data(d)) => each(decode_record(&d)?)?,
            Some(Event::Closed(code, why)) => return if code == CLOSE_NORMAL { Ok(()) } else { Err(why) },
            Some(Event::Opened(_)) => {}
            None => return Err("link lost".into()),
        }
    }
}
