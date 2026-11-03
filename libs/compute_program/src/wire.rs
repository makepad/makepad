//! A session on another machine: what the app and a hub node send each other, as bytes. The
//! transport (the hub's kernel websocket) carries each message as one binary frame,
//! `kind: u8, word: u64 (a request or session id), payload`; this module is the payloads.
//!
//! ```text
//! app -> node   OPEN    request id   program, params, limits, codec, inputs
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

use crate::codec::{self, Codec, Reader};
use crate::cpu::ElemTy;
use crate::session::{Class, Input, Limits, Output, OutputData, Program, ProgramError, Updates};
use crate::value::Value;

pub const OPEN: u8 = 16;
pub const MSG: u8 = 17;
pub const ACK: u8 = 18;
pub const CANCEL: u8 = 19;
pub const OPENED: u8 = 16;
pub const UPDATE: u8 = 17;

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
    pub inputs: Vec<Input>,
}

pub fn encode_open(open: &Open, threads: usize) -> Vec<u8> {
    let mut w = Vec::new();
    put_str(&mut w, &open.program.name);
    put_str(&mut w, &open.program.source);
    put_value(&mut w, &open.params);
    let l = &open.limits;
    for x in [l.fuel as u64, l.heap_bytes as u64, l.stack_values as u64, l.call_frames as u64, l.work, l.buffer_bytes, l.read_words as u64, l.kernels as u64] {
        w.extend_from_slice(&x.to_le_bytes());
    }
    w.extend_from_slice(&open.seed.to_le_bytes());
    w.push(open.codec.id());
    w.push(open.class.budget());
    w.extend_from_slice(&(open.inputs.len() as u32).to_le_bytes());
    for input in &open.inputs {
        put_str(&mut w, &input.name);
        put_str(&mut w, input.ty.name());
        put_words(&mut w, &input.data, open.codec, threads);
    }
    w
}

pub fn decode_open(bytes: &[u8], max_input_words: usize, threads: usize) -> Result<Open, String> {
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
        fuel: size(n[0]),
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
    let count = r.u32()? as usize;
    let mut inputs = Vec::with_capacity(count.min(64));
    let mut words_left = max_input_words;
    for _ in 0..count {
        let name = r.str()?.to_string();
        let ty = ElemTy::from_name(r.str()?).ok_or("an input of an unknown type")?;
        let data = get_words(&mut r, &mut words_left, threads)?;
        if data.len() % ty.words() != 0 {
            return Err(format!("input `{name}`: not whole elements"));
        }
        inputs.push(Input::words(&name, ty, data.into()));
    }
    end(&r)?;
    Ok(Open { program, params, limits, seed, codec, class, inputs })
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
        let (kind, width, bytes, range): (u8, usize, Vec<u8>, Option<(f32, f32)>) = match &o.data {
            OutputData::Words(words) => (0, 4, words.iter().flat_map(|x| x.to_le_bytes()).collect(), None),
            OutputData::F16(h) => (1, 2, h.iter().flat_map(|x| x.to_le_bytes()).collect(), None),
            OutputData::U8 { lo, hi, levels } => (2, 1, levels.to_vec(), Some((*lo, *hi))),
        };
        w.push(kind);
        if let Some((lo, hi)) = range {
            w.extend_from_slice(&lo.to_bits().to_le_bytes());
            w.extend_from_slice(&hi.to_bits().to_le_bytes());
        }
        w.extend_from_slice(&((bytes.len() / width) as u32).to_le_bytes());
        w.push(codec.id());
        let payload = codec::encode(&bytes, width, codec, threads);
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
                let mut b = vec![0u8; count * 4];
                codec::decode(payload, 4, codec, &mut b, threads)?;
                OutputData::Words(b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
            }
            (1, _) => {
                let mut b = vec![0u8; count * 2];
                codec::decode(payload, 2, codec, &mut b, threads)?;
                OutputData::F16(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
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
    let bytes: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    let payload = codec::encode(&bytes, 4, codec, threads);
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
    let mut b = vec![0u8; n * 4];
    codec::decode(payload, 4, codec, &mut b, threads)?;
    Ok(b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
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

