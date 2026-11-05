//! A session on another machine: what the app and a hub node send each other, as bytes. The
//! transport (the hub's kernel websocket) carries each message as one binary frame,
//! `kind: u8, word: u64 (a request or session id), payload`; this module is the payloads.
//!
//! ```text
//! app -> node   OPEN    request id   program, params, limits, codec, inputs (each its data or a
//!                                    hash of data the stream sent before)
//!               MSG     session id   interrupt: u8, key, value
//!               ACK     session id   output name, state: u8, version
//!               CANCEL  session id   -
//! node -> app   OPENED  request id   status: u16, session id: u64, then the node's device or the error
//!               UPDATE  session id   outputs, values, progress, end
//! ```
//!
//! Numbers are little-endian; text is `u32 bytes` + UTF-8; a value is a tag byte (nil, false,
//! true, number f64, text, array, object) and its contents. An output is its header and its
//! elements in a [`Codec`]. A node sends at most [`IN_FLIGHT`] versions of one output before the
//! app acks one; a newer version replaces the one waiting, so a slow link skips versions.
//!
//! An open may name a slot on its stream (`keep`): the node keeps that open's inputs under their
//! [`input_hash`] in the slot, in place of what the slot held, so a later open on the stream names
//! an input it sent before by its hash alone (an edit's solve sends only what changed). A node
//! that no longer keeps one answers [`NOT_KEPT`], and the app sends them all.

use crate::codec::{self, Codec, Reader};
use crate::cpu::ElemTy;
use crate::session::{Class, Input, Limits, Output, OutputData, Program, ProgramError, Updates};
use crate::value::Value;
use std::sync::Arc;

pub const OPEN: u8 = 16;
pub const MSG: u8 = 17;
pub const ACK: u8 = 18;
pub const CANCEL: u8 = 19;
pub const OPENED: u8 = 16;
pub const UPDATE: u8 = 17;

/// `OPENED`'s status when an input the open named by its hash is no longer kept.
pub const NOT_KEPT: u16 = 409;

/// An input travelling with its data, or by its hash.
const WITH_DATA: u8 = 0;
const BY_HASH: u8 = 1;

/// Unacked versions of one output a node sends.
pub const IN_FLIGHT: usize = 2;
/// The longest program source a node takes.
pub const MAX_SOURCE: usize = 256 * 1024;
/// Values (array items and object fields, all nested) in one message.
const MAX_ITEMS: usize = 1 << 20;
const MAX_DEPTH: usize = 32;

/// What the app asks a node to run.
#[derive(Clone)]
pub struct Open {
    pub program: Program,
    pub params: Value,
    pub limits: Limits,
    pub seed: u32,
    /// How the node codes outputs.
    pub codec: Codec,
    /// Who waits on it: the node runs it at this class.
    pub class: Class,
    /// The slot on the stream the node keeps the inputs in (empty: none).
    pub keep: String,
    pub inputs: Vec<Input>,
}

/// An input's content as a node keeps it: 128 bits over its words and their count (four 64-bit
/// lanes over the words two at a time, rounded and merged as xxHash64 does, finished twice). The
/// same words under another element type are the same content.
pub fn input_hash(words: &[u32]) -> u128 {
    const P1: u64 = 0x9E37_79B1_85EB_CA87;
    const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    const P3: u64 = 0x1656_67B1_9E37_79F9;
    const P4: u64 = 0x85EB_CA77_C2B2_AE63;
    const P5: u64 = 0x27D4_EB2F_1656_67C5;
    let round = |acc: u64, x: u64| acc.wrapping_add(x.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
    let pair = |w: &[u32]| w[0] as u64 | (w[1] as u64) << 32;
    let mut v = [P1.wrapping_add(P2), P2, 0, P1.wrapping_neg()];
    let stripes = words.chunks_exact(8);
    let rest = stripes.remainder();
    for s in stripes {
        for (k, lane) in v.iter_mut().enumerate() {
            *lane = round(*lane, pair(&s[2 * k..]));
        }
    }
    let avalanche = |mut h: u64| {
        h ^= h >> 33;
        h = h.wrapping_mul(P2);
        h ^= h >> 29;
        h = h.wrapping_mul(P3);
        h ^ (h >> 32)
    };
    let finish = |lanes: [u64; 4]| {
        let mut h = lanes[0].rotate_left(1).wrapping_add(lanes[1].rotate_left(7)).wrapping_add(lanes[2].rotate_left(12)).wrapping_add(lanes[3].rotate_left(18));
        for lane in lanes {
            h = (h ^ round(0, lane)).wrapping_mul(P1).wrapping_add(P4);
        }
        h = h.wrapping_add(words.len() as u64);
        for w in rest.chunks(2) {
            let x = if w.len() == 2 { pair(w) } else { w[0] as u64 | P5 << 32 };
            h = (h ^ round(0, x)).rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        }
        avalanche(h)
    };
    let low = finish(v);
    let high = finish([v[3] ^ P5, v[2], v[1].rotate_left(32), v[0] ^ P3]);
    (high as u128) << 64 | low as u128
}

/// `OPEN` with each input whose hash `kept` says the node keeps sent by its hash alone, and every
/// input's hash (in order).
pub fn encode_open(open: &Open, threads: usize, kept: &dyn Fn(u128) -> bool) -> (Vec<u8>, Vec<u128>) {
    let mut w = Vec::new();
    put_str(&mut w, &open.program.name);
    put_str(&mut w, &open.program.source);
    put_value(&mut w, &open.params);
    let l = &open.limits;
    for x in [l.fuel, l.heap_bytes as u64, l.stack_values as u64, l.call_frames as u64, l.work, l.buffer_bytes, l.read_words as u64, l.kernels as u64] {
        w.extend_from_slice(&x.to_le_bytes());
    }
    w.extend_from_slice(&open.seed.to_le_bytes());
    w.push(open.codec.id());
    w.push(open.class.budget());
    put_str(&mut w, &open.keep);
    w.extend_from_slice(&(open.inputs.len() as u32).to_le_bytes());
    let mut hashes = Vec::with_capacity(open.inputs.len());
    for input in &open.inputs {
        put_str(&mut w, &input.name);
        put_str(&mut w, input.ty.name());
        let hash = input_hash(&input.data);
        w.extend_from_slice(&hash.to_le_bytes());
        if kept(hash) {
            w.push(BY_HASH);
        } else {
            w.push(WITH_DATA);
            put_words(&mut w, &input.data, open.codec, threads);
        }
        hashes.push(hash);
    }
    (w, hashes)
}

/// An input of an `OPEN` as it came: its hash, and its data unless it came by the hash alone.
pub struct Arrived {
    pub name: String,
    pub ty: ElemTy,
    pub hash: u128,
    pub data: Option<Arc<[u32]>>,
}

/// An `OPEN` back: the open (no inputs) and its inputs as they came, in order; the inputs that
/// came with data hold at most `max_input_words` words.
pub fn decode_open(bytes: &[u8], max_input_words: usize, threads: usize) -> Result<(Open, Vec<Arrived>), String> {
    let mut r = Reader { bytes, at: 0 };
    let name = r.str()?.to_string();
    let source = r.str()?;
    if source.len() > MAX_SOURCE {
        return Err(format!("the program is over {MAX_SOURCE} bytes"));
    }
    let program = Program::new(&name, source);
    let params = get_value(&mut r, &mut 0, 0)?;
    let mut n = [0u64; 8];
    for x in &mut n {
        *x = r.u64()?;
    }
    let size = |x: u64| usize::try_from(x).unwrap_or(usize::MAX);
    let limits = Limits {
        fuel: n[0],
        heap_bytes: size(n[1]),
        stack_values: size(n[2]),
        call_frames: size(n[3]),
        work: n[4],
        buffer_bytes: n[5],
        read_words: size(n[6]),
        kernels: size(n[7]),
    };
    let seed = r.u32()?;
    let codec = Codec::from_id(r.u8()?).ok_or("an unknown codec")?;
    let class = Class::from_budget(r.u8()?).ok_or("an unknown class")?;
    let keep = r.str()?.to_string();
    let count = r.u32()? as usize;
    let mut inputs = Vec::with_capacity(count.min(64));
    let mut words_left = max_input_words;
    for _ in 0..count {
        let name = r.str()?.to_string();
        let ty = ElemTy::from_name(r.str()?).ok_or("an input of an unknown type")?;
        let hash = u128::from_le_bytes(r.take(16)?.try_into().unwrap());
        let data = match r.u8()? {
            WITH_DATA => {
                let data = get_words(&mut r, &mut words_left, threads)?;
                if data.len() % ty.words() != 0 {
                    return Err(format!("input `{name}`: not whole elements"));
                }
                Some(data.into())
            }
            BY_HASH => None,
            _ => return Err(format!("input `{name}`: neither data nor a hash")),
        };
        inputs.push(Arrived { name, ty, hash, data });
    }
    end(&r)?;
    Ok((Open { program, params, limits, seed, codec, class, keep, inputs: Vec::new() }, inputs))
}

/// `OPENED`: the session, and what it runs on.
pub fn encode_opened(session: Result<(u64, Value), (u16, String)>) -> Vec<u8> {
    let mut w = Vec::new();
    match session {
        Ok((id, device)) => {
            w.extend_from_slice(&200u16.to_le_bytes());
            w.extend_from_slice(&id.to_le_bytes());
            put_value(&mut w, &device);
        }
        Err((status, why)) => {
            w.extend_from_slice(&status.to_le_bytes());
            w.extend_from_slice(&0u64.to_le_bytes());
            put_value(&mut w, &Value::String(why));
        }
    }
    w
}

pub fn decode_opened(bytes: &[u8]) -> Result<Result<(u64, Value), (u16, String)>, String> {
    let mut r = Reader { bytes, at: 0 };
    let status = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
    let id = r.u64()?;
    let v = get_value(&mut r, &mut 0, 0)?;
    end(&r)?;
    Ok(if status == 200 {
        Ok((id, v))
    } else {
        Err((status, match v {
            Value::String(s) => s,
            _ => String::new(),
        }))
    })
}

pub fn encode_msg(key: &str, value: &Value, interrupt: bool) -> Vec<u8> {
    let mut w = vec![interrupt as u8];
    put_str(&mut w, key);
    put_value(&mut w, value);
    w
}

pub fn decode_msg(bytes: &[u8]) -> Result<(String, Value, bool), String> {
    let mut r = Reader { bytes, at: 0 };
    let interrupt = r.u8()? != 0;
    let key = r.str()?.to_string();
    let value = get_value(&mut r, &mut 0, 0)?;
    end(&r)?;
    Ok((key, value, interrupt))
}

pub fn encode_ack(name: &str, state: bool, version: u64) -> Vec<u8> {
    let mut w = Vec::new();
    put_str(&mut w, name);
    w.push(state as u8);
    w.extend_from_slice(&version.to_le_bytes());
    w
}

pub fn decode_ack(bytes: &[u8]) -> Result<(String, bool, u64), String> {
    let mut r = Reader { bytes, at: 0 };
    let name = r.str()?.to_string();
    let state = r.u8()? != 0;
    let version = r.u64()?;
    end(&r)?;
    Ok((name, state, version))
}

/// `UPDATE`: outputs coded with `codec` on up to `threads` cores.
pub fn encode_update(u: &Updates, codec: Codec, threads: usize) -> Vec<u8> {
    let mut w = Vec::new();
    w.extend_from_slice(&(u.outputs.len() as u32).to_le_bytes());
    for o in &u.outputs {
        put_str(&mut w, &o.name);
        w.push(o.state as u8);
        w.extend_from_slice(&o.version.to_le_bytes());
        w.extend_from_slice(&o.rev.to_le_bytes());
        put_str(&mut w, o.ty.name());
        let (kind, width, bytes, range): (u8, usize, &[u8], Option<(f32, f32)>) = match &o.data {
            OutputData::Words(words) => (0, 4, bytemuck::cast_slice(words), None),
            OutputData::F16(h) => (1, 2, bytemuck::cast_slice(h), None),
            OutputData::U8 { lo, hi, levels } => (2, 1, levels, Some((*lo, *hi))),
        };
        w.push(kind);
        if let Some((lo, hi)) = range {
            w.extend_from_slice(&lo.to_bits().to_le_bytes());
            w.extend_from_slice(&hi.to_bits().to_le_bytes());
        }
        w.extend_from_slice(&((bytes.len() / width) as u32).to_le_bytes());
        w.push(codec.id());
        let payload = codec::encode(bytes, width, codec, threads);
        w.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        w.extend_from_slice(&payload);
    }
    w.extend_from_slice(&(u.values.len() as u32).to_le_bytes());
    for (name, value) in &u.values {
        put_str(&mut w, name);
        put_value(&mut w, value);
    }
    match &u.progress {
        Some((f, text)) => {
            w.push(1);
            w.extend_from_slice(&f.to_bits().to_le_bytes());
            put_str(&mut w, text);
        }
        None => w.push(0),
    }
    match &u.end {
        None => w.push(0),
        Some(Ok(v)) => {
            w.push(1);
            put_value(&mut w, v);
        }
        Some(Err(e)) => {
            w.push(2);
            let (kind, text) = match e {
                ProgramError::Load(m) => (0, m.as_str()),
                ProgramError::Program(m) => (1, m.as_str()),
                ProgramError::OverBudget(what) => (2, *what),
                ProgramError::Cancelled => (3, ""),
                ProgramError::Lost(m) => (4, m.as_str()),
                ProgramError::DeviceLost => (5, ""),
            };
            w.push(kind);
            put_str(&mut w, text);
        }
    }
    w
}

/// An `UPDATE` back; no output may have more than `max_elements` elements.
pub fn decode_update(bytes: &[u8], max_elements: usize, threads: usize) -> Result<Updates, String> {
    let mut r = Reader { bytes, at: 0 };
    let mut u = Updates::default();
    let outputs = r.u32()? as usize;
    for _ in 0..outputs {
        let name = r.str()?.to_string();
        let state = r.u8()? != 0;
        let version = r.u64()?;
        let rev = r.u64()?;
        let ty = ElemTy::from_name(r.str()?).ok_or("an output of an unknown type")?;
        let kind = r.u8()?;
        let range = if kind == 2 { Some((r.f32()?, r.f32()?)) } else { None };
        let count = r.u32()? as usize;
        if count > max_elements {
            return Err(format!("output `{name}`: {count} elements, over {max_elements}"));
        }
        let codec = Codec::from_id(r.u8()?).ok_or("an unknown codec")?;
        let len = r.u32()? as usize;
        let payload = r.take(len)?;
        let data = match (kind, range) {
            (0, _) => {
                let mut words = vec![0u32; count];
                codec::decode(payload, 4, codec, bytemuck::cast_slice_mut(&mut words), threads)?;
                OutputData::Words(words.into())
            }
            (1, _) => {
                let mut h = vec![0u16; count];
                codec::decode(payload, 2, codec, bytemuck::cast_slice_mut(&mut h), threads)?;
                OutputData::F16(h.into())
            }
            (2, Some((lo, hi))) => {
                let mut b = vec![0u8; count];
                codec::decode(payload, 1, codec, &mut b, threads)?;
                OutputData::U8 { lo, hi, levels: b.into() }
            }
            _ => return Err("an output of an unknown kind".into()),
        };
        u.outputs.push(Output { name, state, version, rev, ty, data });
    }
    let values = r.u32()? as usize;
    let mut items = 0;
    for _ in 0..values {
        let name = r.str()?.to_string();
        u.values.push((name, get_value(&mut r, &mut items, 0)?));
    }
    if r.u8()? == 1 {
        let f = r.f64()?;
        u.progress = Some((f, r.str()?.to_string()));
    }
    u.end = match r.u8()? {
        0 => None,
        1 => Some(Ok(get_value(&mut r, &mut items, 0)?)),
        _ => {
            let kind = r.u8()?;
            let text = r.str()?.to_string();
            Some(Err(match kind {
                0 => ProgramError::Load(text),
                1 => ProgramError::Program(text),
                2 => ProgramError::OverBudget(budget_name(&text)),
                3 => ProgramError::Cancelled,
                5 => ProgramError::DeviceLost,
                _ => ProgramError::Lost(text),
            }))
        }
    };
    end(&r)?;
    Ok(u)
}

fn budget_name(text: &str) -> &'static str {
    ["instructions", "heap", "stack", "calls", "work", "kernels", "inputs"].into_iter().find(|n| *n == text).unwrap_or("budget")
}

fn end(r: &Reader) -> Result<(), String> {
    if r.at == r.bytes.len() {
        Ok(())
    } else {
        Err("bytes after a frame's payload".into())
    }
}

fn put_str(w: &mut Vec<u8>, s: &str) {
    w.extend_from_slice(&(s.len() as u32).to_le_bytes());
    w.extend_from_slice(s.as_bytes());
}

fn put_words(w: &mut Vec<u8>, words: &[u32], codec: Codec, threads: usize) {
    let payload = codec::encode(bytemuck::cast_slice(words), 4, codec, threads);
    w.extend_from_slice(&(words.len() as u32).to_le_bytes());
    w.push(codec.id());
    w.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    w.extend_from_slice(&payload);
}

fn get_words(r: &mut Reader, words_left: &mut usize, threads: usize) -> Result<Vec<u32>, String> {
    let n = r.u32()? as usize;
    if n > *words_left {
        return Err("the inputs are over the node's input budget".into());
    }
    *words_left -= n;
    let codec = Codec::from_id(r.u8()?).ok_or("an unknown codec")?;
    let len = r.u32()? as usize;
    let payload = r.take(len)?;
    let mut words = vec![0u32; n];
    codec::decode(payload, 4, codec, bytemuck::cast_slice_mut(&mut words), threads)?;
    Ok(words)
}

pub fn put_value(w: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Nil => w.push(0),
        Value::Bool(false) => w.push(1),
        Value::Bool(true) => w.push(2),
        Value::Number(x) => {
            w.push(3);
            w.extend_from_slice(&x.to_bits().to_le_bytes());
        }
        Value::String(s) => {
            w.push(4);
            put_str(w, s);
        }
        Value::Array(a) => {
            w.push(5);
            w.extend_from_slice(&(a.len() as u32).to_le_bytes());
            for x in a {
                put_value(w, x);
            }
        }
        Value::Object(fields) => {
            w.push(6);
            w.extend_from_slice(&(fields.len() as u32).to_le_bytes());
            for (k, x) in fields {
                put_str(w, k);
                put_value(w, x);
            }
        }
    }
}

fn get_value(r: &mut Reader, items: &mut usize, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!("a value nested past {MAX_DEPTH}"));
    }
    *items += 1;
    if *items > MAX_ITEMS {
        return Err(format!("a message of more than {MAX_ITEMS} values"));
    }
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(false),
        2 => Value::Bool(true),
        3 => Value::Number(r.f64()?),
        4 => Value::String(r.str()?.to_string()),
        5 => {
            let n = r.u32()? as usize;
            let mut a = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                a.push(get_value(r, items, depth + 1)?);
            }
            Value::Array(a)
        }
        6 => {
            let n = r.u32()? as usize;
            let mut o = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                let k = r.str()?.to_string();
                o.push((k, get_value(r, items, depth + 1)?));
            }
            Value::Object(o)
        }
        _ => return Err("a value of an unknown kind".into()),
    })
}

