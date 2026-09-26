//! The 32-bit `.res` file rc.exe writes. link.exe (through its cvtres) and
//! lld-link accept any number of `.res` inputs and merge them, while they
//! refuse a second pre-converted resource object, so for MSVC targets this is
//! the composable form: another crate or tool adding its own `.res` still
//! links.
use crate::tree::{ResId, Resource};

const MEMORY_FLAGS: u16 = 0x1030; // MOVEABLE | PURE | DISCARDABLE, as rc writes icons

fn pad4(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 { b.push(0); }
}

fn write_id(out: &mut Vec<u8>, id: &ResId) {
    match id {
        ResId::Id(id) => {
            out.extend_from_slice(&0xffffu16.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        }
        ResId::Name(name) => {
            for u in name.to_uppercase().encode_utf16().chain([0]) { out.extend_from_slice(&u.to_le_bytes()); }
        }
    }
}

fn entry(out: &mut Vec<u8>, kind: &ResId, name: &ResId, lang: u16, flags: u16, data: &[u8]) {
    let mut header = Vec::new();
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    write_id(&mut header, kind);
    write_id(&mut header, name);
    pad4(&mut header);
    header.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&lang.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // Version
    header.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    let size = header.len() as u32;
    header[4..8].copy_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    pad4(out);
}

pub fn write_res(resources: &[Resource]) -> Vec<u8> {
    let mut out = Vec::new();
    // The leading empty entry marks a 32-bit resource file.
    entry(&mut out, &ResId::Id(0), &ResId::Id(0), 0, 0, &[]);
    for r in resources {
        entry(&mut out, &r.kind, &r.name, r.lang, MEMORY_FLAGS, &r.data);
    }
    out
}

pub fn read_res(bytes: &[u8]) -> Result<Vec<Resource>, String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| format!(".res truncated at {at}"));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!(".res truncated at {at}"));
    let read_id = |at: &mut usize| -> Result<ResId, String> {
        if u16_at(*at)? == 0xffff {
            let id = u16_at(*at + 2)?;
            *at += 4;
            return Ok(ResId::Id(id));
        }
        let mut units = Vec::new();
        loop {
            let u = u16_at(*at)?;
            *at += 2;
            if u == 0 { break; }
            units.push(u);
        }
        Ok(ResId::Name(String::from_utf16_lossy(&units)))
    };
    let mut out = Vec::new();
    let mut at = 0;
    let mut first = true;
    while at < bytes.len() {
        let data_size = u32_at(at)? as usize;
        let header_size = u32_at(at + 4)? as usize;
        let mut o = at + 8;
        let kind = read_id(&mut o)?;
        let name = read_id(&mut o)?;
        o = (o + 3) & !3;
        let lang = u16_at(o + 6)?;
        let data_at = at + header_size;
        let data = bytes.get(data_at..data_at + data_size).ok_or(".res data truncated")?.to_vec();
        if first {
            if data_size != 0 || kind != ResId::Id(0) {
                return Err("not a 32-bit .res file".into());
            }
        } else {
            out.push(Resource { kind, name, lang, data });
        }
        first = false;
        at = (data_at + data_size + 3) & !3;
    }
    Ok(out)
}
