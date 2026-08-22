//! Wire encode/decode for the `POST /realtime` + `GET /realtime/<id>`
//! (websocket) live session. See `protocol.rs`'s "Realtime session wire
//! protocol" doc block for the full contract (binary frame header layout,
//! JSON message shapes, and — important — why server -> client JSON
//! messages are self-describing rather than sent as WebSocket Text frames).
//! `crate::realtime` owns the session/loop logic and calls into this module
//! only for (de)serialization.

use crate::error::AssetAiError;
use makepad_micro_serde::*;

// ---------------------------------------------------------------------------
// Binary frame header (both directions)
// ---------------------------------------------------------------------------

/// `"FRFL"` read as a little-endian u32 (i.e. `encode_frame` writes the
/// bytes `[b'F', b'R', b'F', b'L']`).
pub const FRAME_MAGIC: u32 = 0x4C46_5246;
pub const FRAME_HEADER_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Raw RGB8, no compression — the fast path.
    Raw = 0,
    Png = 1,
    /// One H.264 Annex-B access unit (start-code delimited NAL units;
    /// see `platform/video`'s `stream_encoder`/`stream_decoder`). A
    /// keyframe access unit always carries SPS+PPS before the IDR slice.
    H264 = 2,
}

impl FrameKind {
    fn from_u8(value: u8) -> Result<Self, AssetAiError> {
        match value {
            0 => Ok(FrameKind::Raw),
            1 => Ok(FrameKind::Png),
            2 => Ok(FrameKind::H264),
            other => Err(AssetAiError::Params(format!(
                "realtime frame: unknown kind byte {other} (expected 0=raw, 1=png or 2=h264)"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub kind: FrameKind,
    pub width: u16,
    pub height: u16,
    pub frame_index: u32,
}

/// Encodes one binary frame: the 16-byte header (see the module doc) plus
/// `payload` verbatim.
pub fn encode_frame(header: FrameHeader, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
    out.push(header.kind as u8);
    out.push(0); // reserved u8
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved u16
    out.extend_from_slice(&header.width.to_le_bytes());
    out.extend_from_slice(&header.height.to_le_bytes());
    out.extend_from_slice(&header.frame_index.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decodes one binary frame: validates the magic/kind and (for raw frames
/// only, where the exact size is derivable) the payload length, then returns
/// the header plus a slice of `bytes` for the payload. PNG payload validity
/// is the decoder's job (`testpattern::decode_png_rgb8`), not this parser's.
pub fn decode_frame(bytes: &[u8]) -> Result<(FrameHeader, &[u8]), AssetAiError> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Err(AssetAiError::Params(format!(
            "realtime frame: {} bytes, need at least {FRAME_HEADER_LEN} for the header",
            bytes.len()
        )));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != FRAME_MAGIC {
        return Err(AssetAiError::Params(format!(
            "realtime frame: bad magic 0x{magic:08x}, expected 0x{FRAME_MAGIC:08x}"
        )));
    }
    let kind = FrameKind::from_u8(bytes[4])?;
    // bytes[5] and bytes[6..8] are reserved — ignored on read.
    let width = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
    let height = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
    let frame_index = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let payload = &bytes[FRAME_HEADER_LEN..];
    if kind == FrameKind::Raw {
        let expected = width as usize * height as usize * 3;
        if payload.len() != expected {
            return Err(AssetAiError::Params(format!(
                "realtime frame: raw payload is {} bytes, expected {expected} for {width}x{height} RGB8",
                payload.len()
            )));
        }
    }
    Ok((
        FrameHeader {
            kind,
            width,
            height,
            frame_index,
        },
        payload,
    ))
}

/// True when `bytes` starts with [`FRAME_MAGIC`] — the self-describing
/// signature a client uses to tell a pushed output frame apart from a JSON
/// message on the server -> client direction (see the module doc: the
/// in-repo `HttpServer` sends both as WebSocket Binary frames).
pub fn is_frame_message(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) == FRAME_MAGIC
}

// ---------------------------------------------------------------------------
// Client -> server JSON messages
// ---------------------------------------------------------------------------

/// Probe struct: every client message carries `"type"`; decode this first to
/// pick which full struct to decode.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct MessageTypeJson {
    #[rename(type)]
    kind: String,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct CameraUpdateJson {
    pub dolly: Option<f64>,
    pub pan_x: Option<f64>,
    pub pan_y: Option<f64>,
    pub roll: Option<f64>,
}

/// `{"type":"control", ...}` — every field is optional; only the fields
/// present in a given message change the session (see
/// `realtime::apply_control_to_config` and `realtime::RealtimeSession::
/// apply_control`, which merge this the same way).
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct ControlUpdateJson {
    #[rename(type)]
    pub kind: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub strength: Option<f64>,
    pub steps: Option<u32>,
    pub guidance: Option<f64>,
    pub seed: Option<u64>,
    pub seed_mode: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraUpdateJson>,
    pub loop_mode: Option<String>,
    pub input_encoding: Option<String>,
    pub output_encoding: Option<String>,
    pub max_fps: Option<f64>,
    pub idle_timeout_s: Option<u64>,
}

/// `{"type":"reference", "slot":0, "png_b64":"..."}`.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct ReferenceMessageJson {
    #[rename(type)]
    pub kind: String,
    pub slot: Option<u32>,
    pub png_b64: Option<String>,
}

/// One parsed client -> server text message.
#[derive(Debug)]
pub enum ClientMessage {
    Control(ControlUpdateJson),
    Reference(ReferenceMessageJson),
    Stop,
}

/// Parses one client -> server text message by its `"type"` field.
pub fn parse_client_message(text: &str) -> Result<ClientMessage, AssetAiError> {
    let probe = MessageTypeJson::deserialize_json_lenient(text)
        .map_err(|e| AssetAiError::Params(format!("realtime message: bad json: {e:?}")))?;
    match probe.kind.as_str() {
        "control" => {
            let update = ControlUpdateJson::deserialize_json_lenient(text)
                .map_err(|e| AssetAiError::Params(format!("realtime control message: {e:?}")))?;
            Ok(ClientMessage::Control(update))
        }
        "reference" => {
            let reference = ReferenceMessageJson::deserialize_json_lenient(text)
                .map_err(|e| AssetAiError::Params(format!("realtime reference message: {e:?}")))?;
            Ok(ClientMessage::Reference(reference))
        }
        "stop" => Ok(ClientMessage::Stop),
        other => Err(AssetAiError::Params(format!(
            "realtime message: unknown type {other:?} (expected \"control\", \"reference\" or \"stop\")"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Server -> client JSON messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct StageMsJson {
    pub prep: f64,
    pub model: f64,
    pub post: f64,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct StatsMessageJson {
    #[rename(type)]
    pub kind: String,
    pub frame_index: u64,
    pub fps: f64,
    pub frame_ms: f64,
    pub stage_ms: StageMsJson,
    pub frames_in: u64,
    pub frames_out: u64,
    pub dropped: u64,
    pub codec: CodecStatsJson,
}

/// Wire encoding actually in effect for each direction, plus the count of
/// input packets that failed to decode (H.264 only — see
/// `realtime::RealtimeSession::handle_binary`).
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct CodecStatsJson {
    pub input: String,
    pub output: String,
    pub dropped_decode: u64,
}

pub fn encode_stats_message(mut stats: StatsMessageJson) -> String {
    stats.kind = "stats".to_string();
    stats.serialize_json()
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct ErrorMessageJson {
    #[rename(type)]
    kind: String,
    message: String,
}

pub fn encode_error_message(message: &str) -> String {
    ErrorMessageJson {
        kind: "error".to_string(),
        message: message.to_string(),
    }
    .serialize_json()
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct StoppedMessageJson {
    #[rename(type)]
    kind: String,
    reason: String,
}

pub fn encode_stopped_message(reason: &str) -> String {
    StoppedMessageJson {
        kind: "stopped".to_string(),
        reason: reason.to_string(),
    }
    .serialize_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_raw() {
        let header = FrameHeader {
            kind: FrameKind::Raw,
            width: 4,
            height: 2,
            frame_index: 7,
        };
        let payload = vec![9u8; 4 * 2 * 3];
        let bytes = encode_frame(header, &payload);
        assert_eq!(bytes.len(), FRAME_HEADER_LEN + payload.len());
        assert_eq!(&bytes[0..4], b"FRFL");
        let (decoded, decoded_payload) = decode_frame(&bytes).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(decoded_payload, payload.as_slice());
        assert!(is_frame_message(&bytes));
    }

    #[test]
    fn frame_round_trip_png_kind_skips_length_check() {
        let header = FrameHeader {
            kind: FrameKind::Png,
            width: 4,
            height: 2,
            frame_index: 1,
        };
        // A PNG payload's byte length has no fixed relationship to
        // width*height*3 — decode_frame must not reject it.
        let payload = vec![1, 2, 3, 4, 5];
        let bytes = encode_frame(header, &payload);
        let (decoded, decoded_payload) = decode_frame(&bytes).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(decoded_payload, payload.as_slice());
    }

    #[test]
    fn decode_frame_rejects_short_buffer() {
        let err = decode_frame(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn decode_frame_rejects_bad_magic() {
        let mut bytes = vec![0u8; FRAME_HEADER_LEN];
        bytes[0] = 1; // magic mismatch
        let err = decode_frame(&bytes).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn decode_frame_rejects_bad_kind_byte() {
        let mut bytes = vec![0u8; FRAME_HEADER_LEN];
        bytes[0..4].copy_from_slice(&FRAME_MAGIC.to_le_bytes());
        bytes[4] = 9; // invalid kind
        let err = decode_frame(&bytes).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn decode_frame_rejects_raw_length_mismatch() {
        let header = FrameHeader {
            kind: FrameKind::Raw,
            width: 4,
            height: 2,
            frame_index: 0,
        };
        // One byte short of the required 4*2*3 = 24 bytes.
        let payload = vec![0u8; 23];
        let bytes = encode_frame(header, &payload);
        let err = decode_frame(&bytes).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn is_frame_message_false_for_json_text() {
        let json = encode_error_message("boom");
        assert!(!is_frame_message(json.as_bytes()));
    }

    #[test]
    fn parse_client_message_control_partial_fields() {
        let msg = parse_client_message(r#"{"type":"control","prompt":"a cat","steps":8}"#).unwrap();
        match msg {
            ClientMessage::Control(update) => {
                assert_eq!(update.prompt.as_deref(), Some("a cat"));
                assert_eq!(update.steps, Some(8));
                assert_eq!(update.strength, None);
                assert!(update.camera.is_none());
            }
            _ => panic!("expected Control"),
        }
    }

    #[test]
    fn parse_client_message_control_with_camera() {
        let msg = parse_client_message(
            r#"{"type":"control","camera":{"dolly":0.5,"roll":1.25}}"#,
        )
        .unwrap();
        match msg {
            ClientMessage::Control(update) => {
                let camera = update.camera.expect("camera present");
                assert_eq!(camera.dolly, Some(0.5));
                assert_eq!(camera.roll, Some(1.25));
                assert_eq!(camera.pan_x, None);
            }
            _ => panic!("expected Control"),
        }
    }

    #[test]
    fn parse_client_message_reference() {
        let msg = parse_client_message(r#"{"type":"reference","slot":2,"png_b64":"AA=="}"#).unwrap();
        match msg {
            ClientMessage::Reference(reference) => {
                assert_eq!(reference.slot, Some(2));
                assert_eq!(reference.png_b64.as_deref(), Some("AA=="));
            }
            _ => panic!("expected Reference"),
        }
    }

    #[test]
    fn parse_client_message_stop() {
        assert!(matches!(
            parse_client_message(r#"{"type":"stop"}"#).unwrap(),
            ClientMessage::Stop
        ));
    }

    #[test]
    fn parse_client_message_rejects_unknown_type() {
        let err = parse_client_message(r#"{"type":"nonsense"}"#).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn parse_client_message_rejects_malformed_json() {
        let err = parse_client_message("not json").unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn stats_error_stopped_messages_are_self_describing_not_frames() {
        let stats = encode_stats_message(StatsMessageJson {
            frame_index: 3,
            fps: 30.0,
            frame_ms: 33.3,
            stage_ms: StageMsJson { prep: 1.0, model: 30.0, post: 2.3 },
            frames_in: 3,
            frames_out: 3,
            dropped: 0,
            codec: CodecStatsJson::default(),
            kind: String::new(),
        });
        assert!(stats.contains("\"type\":\"stats\""));
        assert!(!is_frame_message(stats.as_bytes()));

        let error = encode_error_message("bad seed");
        assert!(error.contains("\"type\":\"error\""));
        assert!(error.contains("bad seed"));

        let stopped = encode_stopped_message("cancelled");
        assert!(stopped.contains("\"type\":\"stopped\""));
        assert!(stopped.contains("cancelled"));
    }
}
