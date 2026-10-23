//! Open Sound Control, as much as a score speaks: messages with int32,
//! float32 and string arguments (bundles are opened, their time tags
//! ignored). `osc_send` in loop code sends them; a Clock's `osc_in` port
//! receives them (`osc("/fader")` in code, `/param/<name>` moves a param).

#[derive(Clone, Debug, PartialEq)]
pub enum OscArg {
    Int(i32),
    Float(f32),
    Str(String),
}

impl OscArg {
    pub fn num(&self) -> Option<f32> {
        match self {
            OscArg::Int(i) => Some(*i as f32),
            OscArg::Float(f) => Some(*f),
            OscArg::Str(_) => None,
        }
    }
}

fn pad(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    pad(out);
}

/// Whether this platform has UDP sockets for OSC (the web has none): asked
/// once, by opening one. Where it is false a score's `osc_in` and
/// `osc_send` do nothing.
pub fn available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| std::net::UdpSocket::bind(("127.0.0.1", 0)).is_ok())
}

/// One message as a packet.
pub fn encode(addr: &str, args: &[OscArg]) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + args.len() * 8);
    put_str(&mut out, addr);
    let tags: String = std::iter::once(',').chain(args.iter().map(|a| match a {
        OscArg::Int(_) => 'i',
        OscArg::Float(_) => 'f',
        OscArg::Str(_) => 's',
    })).collect();
    put_str(&mut out, &tags);
    for arg in args {
        match arg {
            OscArg::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
            OscArg::Float(f) => out.extend_from_slice(&f.to_be_bytes()),
            OscArg::Str(s) => put_str(&mut out, s),
        }
    }
    out
}

fn get_str(b: &[u8], at: &mut usize) -> Option<String> {
    let end = b.get(*at..)?.iter().position(|c| *c == 0)? + *at;
    let s = std::str::from_utf8(&b[*at..end]).ok()?.to_string();
    *at = (end + 4) & !3;
    Some(s)
}

fn get4(b: &[u8], at: &mut usize) -> Option<[u8; 4]> {
    let v: [u8; 4] = b.get(*at..*at + 4)?.try_into().ok()?;
    *at += 4;
    Some(v)
}

/// The messages in a packet (a bundle's, in order); what does not parse
/// is left out.
pub fn decode(b: &[u8]) -> Vec<(String, Vec<OscArg>)> {
    let mut out = Vec::new();
    decode_into(b, &mut out, 0);
    out
}

fn decode_into(b: &[u8], out: &mut Vec<(String, Vec<OscArg>)>, depth: usize) {
    if depth > 4 {
        return;
    }
    if b.starts_with(b"#bundle\0") {
        let mut at = 16;
        while let Some(size) = get4(b, &mut at) {
            let size = u32::from_be_bytes(size) as usize;
            let Some(inner) = b.get(at..at + size) else { return };
            decode_into(inner, out, depth + 1);
            at += size;
        }
        return;
    }
    let mut at = 0;
    let Some(addr) = get_str(b, &mut at) else { return };
    if !addr.starts_with('/') {
        return;
    }
    let tags = get_str(b, &mut at).unwrap_or_default();
    let mut args = Vec::new();
    for tag in tags.chars().skip(1) {
        let arg = match tag {
            'i' => get4(b, &mut at).map(|v| OscArg::Int(i32::from_be_bytes(v))),
            'f' => get4(b, &mut at).map(|v| OscArg::Float(f32::from_be_bytes(v))),
            's' | 'S' => get_str(b, &mut at).map(OscArg::Str),
            'T' => Some(OscArg::Int(1)),
            'F' | 'N' | 'I' => Some(OscArg::Int(0)),
            'd' => b.get(at..at + 8).and_then(|v| v.try_into().ok()).map(|v: [u8; 8]| {
                at += 8;
                OscArg::Float(f64::from_be_bytes(v) as f32)
            }),
            'h' => b.get(at..at + 8).and_then(|v| v.try_into().ok()).map(|v: [u8; 8]| {
                at += 8;
                OscArg::Int(i64::from_be_bytes(v) as i32)
            }),
            _ => None,
        };
        match arg {
            Some(arg) => args.push(arg),
            None => break,
        }
    }
    out.push((addr, args));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_bundles_open() {
        let args = vec![OscArg::Int(7), OscArg::Float(0.5), OscArg::Str("hi".into())];
        let packet = encode("/fader/1", &args);
        assert_eq!(packet.len() % 4, 0);
        assert_eq!(decode(&packet), [("/fader/1".to_string(), args.clone())]);
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        for p in [encode("/a", &[OscArg::Float(1.0)]), encode("/b", &[])] {
            bundle.extend_from_slice(&(p.len() as u32).to_be_bytes());
            bundle.extend_from_slice(&p);
        }
        assert_eq!(decode(&bundle).iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), ["/a", "/b"]);
        assert!(decode(b"garbage").is_empty());
    }
}
