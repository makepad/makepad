use makepad_strict_json::{self as json, Value};

pub const VERSION: u64 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const HELLO: u8 = 1;
pub const SNAPSHOT: u8 = 2;
pub const OUTPUT: u8 = 3;
pub const EXIT: u8 = 4;
pub const ERROR: u8 = 5;
pub const INPUT: u8 = 6;
pub const RESIZE: u8 = 7;
pub const STOP: u8 = 8;
pub const STATUS: u8 = 9;
pub const STATUS_REPLY: u8 = 10;
pub const TEXT: u8 = 11;
pub const TEXT_REPLY: u8 = 12;
pub const NAME: u8 = 13;

pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}
pub fn encode_frame(kind: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    if !(HELLO..=NAME).contains(&kind) || payload.len() > MAX_FRAME {
        return Err("Invalid screen frame kind or size".into());
    }
    let mut bytes = Vec::with_capacity(payload.len() + 5);
    bytes.push(kind);
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}
pub fn decode_frame(input: &mut Vec<u8>) -> Result<Option<Frame>, String> {
    if input.len() < 5 {
        return Ok(None);
    }
    let kind = input[0];
    let length = u32::from_be_bytes(input[1..5].try_into().unwrap()) as usize;
    if !(HELLO..=NAME).contains(&kind) || length > MAX_FRAME {
        return Err("Invalid screen frame kind or size".into());
    }
    if input.len() < length + 5 {
        return Ok(None);
    }
    let payload = input[5..length + 5].to_vec();
    input.drain(..length + 5);
    Ok(Some(Frame { kind, payload }))
}
pub fn valid_session(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
pub fn parse_object(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > 16 * 1024 {
        return Err("Screen control exceeds 16 KiB".into());
    }
    let value = json::parse_depth(bytes, 8).map_err(str::to_owned)?;
    if !matches!(value, Value::Obj(_)) {
        return Err("Screen control must be a JSON object".into());
    }
    Ok(value)
}
pub fn dimensions(value: &Value) -> Result<(u16, u16), String> {
    let cols = value
        .get("cols")
        .and_then(Value::as_u64)
        .filter(|n| (2..=400).contains(n))
        .ok_or("cols must be 2..400")?;
    let rows = value
        .get("rows")
        .and_then(Value::as_u64)
        .filter(|n| (2..=200).contains(n))
        .ok_or("rows must be 2..200")?;
    Ok((cols as u16, rows as u16))
}
