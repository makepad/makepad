//! Wire encode/decode for the `POST /realtime` + `GET /realtime/<id>`
//! (websocket) live session. See `protocol.rs`'s "Realtime session wire
//! protocol" doc block for the full contract (binary frame header layout,
//! JSON message shapes, and — important — why server -> client JSON
//! messages are self-describing rather than sent as WebSocket Text frames).
//! `crate::realtime` owns the session/loop logic and calls into this module
//! only for (de)serialization.

use crate::error::AssetAiError;
use makepad_micro_serde::*;

/// Admission is retryable only when the server definitively created no job.
/// Missing fields are not null: a truncated/legacy error or transport failure
/// may conceal an accepted session and must never trigger another POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RealtimeOpenResponse {
    Accepted { job_id: String, ws_path: String },
    Unavailable { reason: String },
    Failed { reason: String },
}

pub fn classify_realtime_open_response(status: u16, body: &[u8]) -> RealtimeOpenResponse {
    use makepad_strict_json::Value;
    let failed = || RealtimeOpenResponse::Failed {
        reason: format!("realtime open: http {status} {}", String::from_utf8_lossy(body).chars().take(200).collect::<String>()),
    };
    // Unlike optional typed fields, strict JSON preserves explicit nulls,
    // rejects duplicate keys, invalid UTF-8 and trailing/truncated data.
    let Ok(root @ Value::Obj(_)) = makepad_strict_json::parse(body) else { return failed() };
    let error = root.get("error");
    if status == 200 && matches!(error, None | Some(Value::Null)) {
        if let (Some(Value::Str(job_id)), Some(Value::Str(ws_path))) = (root.get("job_id"), root.get("ws_path")) {
            if !job_id.is_empty() && ws_path == &format!("/realtime/{job_id}") {
                return RealtimeOpenResponse::Accepted { job_id: job_id.clone(), ws_path: ws_path.clone() };
            }
        }
    }
    if matches!(status, 409 | 429 | 503)
        && matches!(root.get("job_id"), Some(Value::Null))
        && matches!(root.get("ws_path"), Some(Value::Null))
    {
        if let Some(Value::Str(reason)) = error {
            // These are admission reasons, not arbitrary model/backend errors.
            let local_use = reason.strip_prefix("model unavailable: local-use:")
                .or_else(|| reason.strip_prefix("local-use:"))
                .is_some_and(|s| !s.trim().is_empty());
            let queue_full = reason.strip_prefix("queue full: ")
                .and_then(|s| s.strip_suffix(" jobs already queued on this node"))
                .is_some_and(|n| n.parse::<usize>().is_ok());
            if local_use || queue_full || matches!(reason.as_str(),
                "busy: a job is already queued or running" | "temporarily unavailable" | "model unavailable: temporarily unavailable")
            {
                return RealtimeOpenResponse::Unavailable { reason: reason.clone() };
            }
        }
    }
    failed()
}

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

/// Partial update of `backend::DriftParams` (feedback loop colour
/// treatment) — same all-optional merge pattern as [`CameraUpdateJson`].
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct DriftUpdateJson {
    /// Degrees per frame.
    pub hue: Option<f64>,
    pub gain: Option<f64>,
    pub anchor: Option<f64>,
    pub grain: Option<f64>,
    pub sharpen: Option<f64>,
    /// "reflect" | "clamp" | "source".
    pub border: Option<String>,
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
    /// Feedback loop: share of the warped previous output in the next init.
    pub feedback: Option<f64>,
    /// Feedback loop: how far the anchor (the edit's conditioning image)
    /// follows the trip — 0 pins it to the source, 1 lets it ride the
    /// previous output entirely. See `realtime`'s loop for why a pinned
    /// anchor makes the whole feed converge to a still.
    pub anchor_follow: Option<f64>,
    /// "hold" | "reroll" | "auto".
    pub noise_mode: Option<String>,
    pub drift: Option<DriftUpdateJson>,
    /// `true`: forget the previous output, so the next feedback frame is a
    /// cold start (one full edit) from the current source.
    pub reset: Option<bool>,
}

/// `{"type":"seed_output", "raw_b64":"...", "w":512, "h":512}` — or the same
/// picture as `png_b64`.
///
/// The one thing a live session cannot rebuild for itself: the trip so far.
/// A client moving a feed from one box to another (the wall's cluster does
/// this when the picture in the middle of the view should be on a faster
/// machine) sends the departing session's LAST OUTPUT here, and the arriving
/// session continues from it instead of cold-starting a fresh diffusion of
/// the source — which is a visible snap back toward the clean picture.
///
/// It is NOT a source: the anchor stays the true picture (reference slot 0),
/// which is what the colour drift pulls against. Nothing else travels — the
/// noise field and the prompt embeds are re-derived locally.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct SeedOutputMessageJson {
    #[rename(type)]
    pub kind: String,
    /// Raw RGB8, base64. Needs `w`/`h`, since raw bytes carry no size.
    pub raw_b64: Option<String>,
    /// The same picture as a PNG, for a client that has one to hand.
    pub png_b64: Option<String>,
    pub w: Option<u32>,
    pub h: Option<u32>,
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
    /// The trip so far, carried in from another box (see
    /// [`SeedOutputMessageJson`]).
    SeedOutput(SeedOutputMessageJson),
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
        "seed_output" => {
            let seed = SeedOutputMessageJson::deserialize_json_lenient(text)
                .map_err(|e| AssetAiError::Params(format!("realtime seed_output message: {e:?}")))?;
            Ok(ClientMessage::SeedOutput(seed))
        }
        "stop" => Ok(ClientMessage::Stop),
        other => Err(AssetAiError::Params(format!(
            "realtime message: unknown type {other:?} (expected \"control\", \"reference\", \"seed_output\" or \"stop\")"
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
    /// The text encoder's share of `model` (0 when the backend has none or
    /// served cached prompt embeds).
    pub text_encode: f64,
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
    /// "feed" | "feedback" — the loop mode this frame was produced in.
    pub loop_mode: String,
    /// Mean absolute difference (0..255) between this output frame and the
    /// previous one — the "is the loop still moving" number a client would
    /// otherwise have to compute itself. Absent on the first frame and
    /// whenever the two differ in size.
    pub frame_diff: Option<f64>,
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

/// Wraps an already-validated backend JSON value without parsing or
/// re-serializing it. Server pushes are websocket Binary payloads, so the
/// returned bytes intentionally have no FRFL frame header.
pub fn encode_aux_message(frame_index: u64, data_json: &str) -> Vec<u8> {
    format!(
        "{{\"type\":\"aux\",\"frame_index\":{frame_index},\"data\":{data_json}}}"
    )
    .into_bytes()
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
    fn realtime_admission_requires_explicit_no_job_temporary_refusal() {
        let body = br#"{"job_id":null,"ws_path":null,"error":"model unavailable: local-use: quiet-hysteresis"}"#;
        for status in [409, 429, 503] {
            assert!(matches!(classify_realtime_open_response(status, body), RealtimeOpenResponse::Unavailable { .. }));
        }
        for status in [200, 400, 404, 500] {
            assert!(matches!(classify_realtime_open_response(status, body), RealtimeOpenResponse::Failed { .. }));
        }
        for reason in ["busy: a job is already queued or running", "queue full: 3 jobs already queued on this node", "temporarily unavailable"] {
            let body = format!(r#"{{"job_id":null,"ws_path":null,"error":"{reason}"}}"#);
            assert!(matches!(classify_realtime_open_response(409, body.as_bytes()), RealtimeOpenResponse::Unavailable { .. }));
        }
    }

    #[test]
    fn realtime_admission_never_retries_accepted_ambiguous_or_malformed_responses() {
        for body in [
            "", "null", "{}", "{", "not json",
            r#"{"error":"model unavailable: local-use: busy"}"#,
            r#"{"job_id":null,"error":"model unavailable: local-use: busy"}"#,
            r#"{"ws_path":null,"error":"model unavailable: local-use: busy"}"#,
            r#"{"job_id":"job-1","ws_path":null,"error":"local-use: busy"}"#,
            r#"{"job_id":null,"ws_path":"/realtime/job-1","error":"local-use: busy"}"#,
            r#"{"job_id":0,"ws_path":null,"error":"local-use: busy"}"#,
            r#"{"job_id":"","ws_path":"","error":"local-use: busy"}"#,
            r#"{"job_id":null,"ws_path":null,"error":"local-use: busy"} garbage"#,
            r#"{"job_id":"job-1","job_id":null,"ws_path":null,"error":"local-use: busy"}"#,
            r#"{"job_id":null,"ws_path":null,"error":"bad request: local-use: busy"}"#,
            r#"{"job_id":null,"ws_path":null,"error":"model unavailable: backend not compiled"}"#,
            r#"{"job_id":null,"ws_path":null,"error":"unknown model: flux"}"#,
            r#"{"job_id":null,"ws_path":null,"error":"model unavailable: local-use:"}"#,
        ] {
            assert!(matches!(classify_realtime_open_response(409, body.as_bytes()), RealtimeOpenResponse::Failed { .. }), "{body}");
        }
        assert!(matches!(classify_realtime_open_response(503, b"\xff"), RealtimeOpenResponse::Failed { .. }));
        let accepted = br#"{"job_id":"job-1","ws_path":"/realtime/job-1"}"#;
        assert_eq!(classify_realtime_open_response(200, accepted), RealtimeOpenResponse::Accepted {
            job_id: "job-1".into(), ws_path: "/realtime/job-1".into(),
        });
        assert!(matches!(classify_realtime_open_response(409, accepted), RealtimeOpenResponse::Failed { .. }));
        let accepted_error = br#"{"job_id":"job-1","ws_path":"/realtime/job-1","error":"local-use: busy"}"#;
        assert!(matches!(classify_realtime_open_response(200, accepted_error), RealtimeOpenResponse::Failed { .. }));
    }

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
    fn aux_message_embeds_raw_json_without_a_frame_header() {
        let data = r#"{"n_people":1,"opaque":[1, 2]}"#;
        let bytes = encode_aux_message(17, data);
        assert_eq!(
            bytes,
            br#"{"type":"aux","frame_index":17,"data":{"n_people":1,"opaque":[1, 2]}}"#
        );
        assert!(!is_frame_message(&bytes));
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
    fn parse_client_message_control_with_feedback_fields() {
        let msg = parse_client_message(
            r#"{"type":"control","loop_mode":"feedback","feedback":0.7,"noise_mode":"hold",
                "drift":{"hue":0.6,"anchor":0.05,"border":"source"},"reset":true}"#,
        )
        .unwrap();
        match msg {
            ClientMessage::Control(update) => {
                assert_eq!(update.loop_mode.as_deref(), Some("feedback"));
                assert_eq!(update.feedback, Some(0.7));
                assert_eq!(update.noise_mode.as_deref(), Some("hold"));
                assert_eq!(update.reset, Some(true));
                let drift = update.drift.expect("drift present");
                assert_eq!(drift.hue, Some(0.6));
                assert_eq!(drift.anchor, Some(0.05));
                assert_eq!(drift.border.as_deref(), Some("source"));
                // Fields the message did not carry stay None (partial merge).
                assert_eq!(drift.gain, None);
                assert_eq!(drift.grain, None);
                assert_eq!(drift.sharpen, None);
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

    /// The one message a moving feed sends: the trip so far, as one image.
    #[test]
    fn parse_client_message_seed_output() {
        let msg = parse_client_message(r#"{"type":"seed_output","raw_b64":"AAEC","w":1,"h":1}"#).unwrap();
        match msg {
            ClientMessage::SeedOutput(seed) => {
                assert_eq!(seed.raw_b64.as_deref(), Some("AAEC"));
                assert_eq!((seed.w, seed.h), (Some(1), Some(1)));
                assert!(seed.png_b64.is_none());
            }
            _ => panic!("expected SeedOutput"),
        }
        let png = parse_client_message(r#"{"type":"seed_output","png_b64":"iVBOR"}"#).unwrap();
        assert!(matches!(png, ClientMessage::SeedOutput(seed) if seed.png_b64.as_deref() == Some("iVBOR")));
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
            stage_ms: StageMsJson { prep: 1.0, model: 30.0, text_encode: 4.0, post: 2.3 },
            frames_in: 3,
            frames_out: 3,
            dropped: 0,
            codec: CodecStatsJson::default(),
            kind: String::new(),
            loop_mode: "feedback".to_string(),
            frame_diff: Some(12.5),
        });
        assert!(stats.contains("\"type\":\"stats\""));
        assert!(stats.contains("\"frame_diff\":12.5"));
        assert!(stats.contains("\"loop_mode\":\"feedback\""));
        assert!(!is_frame_message(stats.as_bytes()));

        let error = encode_error_message("bad seed");
        assert!(error.contains("\"type\":\"error\""));
        assert!(error.contains("bad seed"));

        let stopped = encode_stopped_message("cancelled");
        assert!(stopped.contains("\"type\":\"stopped\""));
        assert!(stopped.contains("cancelled"));
    }
}
