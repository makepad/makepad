//! Wire protocol: host-authoritative messages plus the authenticated framing
//! used on both sockets.
//!
//! Trust direction is fixed and one-way. Clients send intent (input, join,
//! leave); the host sends truth (welcome, snapshots, state, events). There is
//! no per-object authority field and no takeover negotiation — the audit's
//! `XrTakeoverAccept` hole cannot be expressed in this protocol.

use crate::auth::{LobbyKey, MAC_LEN};
use makepad_lz4::{compress_bound, compress_fast_into, decompress_safe};
use makepad_micro_serde::*;
use std::io;

pub const PROTOCOL_VERSION: u16 = 1;
pub const PROTOCOL_MAGIC: u16 = 0xA9CD;

/// Frame ceiling. The XR stack needed 4 MiB only for alignment heightmaps,
/// which this crate does not carry; 256 KiB removes the decompression
/// amplification the audit flagged (M-1) by construction.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Conservative payload budget for a single datagram — stays under a 1500-byte
/// MTU once the header and MAC are added.
pub const MAX_DATAGRAM_PAYLOAD: usize = 1200;

const FRAME_RAW_TAG: u8 = 0;
const FRAME_LZ4_TAG: u8 = 1;
const LZ4_ACCELERATION: usize = 4;

/// Stable per-connection identity of a participant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, SerBin, DeBin)]
pub struct PlayerId(pub u64);

/// The host's identity, echoed in every authoritative message so clients can
/// reject anything that did not come from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, SerBin, DeBin)]
pub struct HostId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerBin, DeBin)]
pub enum LeaveReason {
    Explicit,
    Timeout,
    HostShutdown,
    LobbyFull,
    ProtocolMismatch,
    AuthFailed,
}

/// One entity's replicated pose. `seq` is **per entity** (audit P1): a
/// reordered datagram can only stale-drop the entities inside it, and only
/// those whose seq is older than what the client already applied.
#[derive(Clone, Copy, Debug, PartialEq, SerBin, DeBin)]
pub struct EntityState {
    pub id: u64,
    pub seq: u32,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub flags: u16,
}

impl EntityState {
    /// Remote floats reach gameplay math, so non-finite values are rejected at
    /// the parse boundary rather than poisoning comparisons downstream.
    pub fn is_finite(&self) -> bool {
        self.pos.iter().all(|v| v.is_finite())
            && self.vel.iter().all(|v| v.is_finite())
            && self.yaw.is_finite()
    }
}

/// Construction data for one entity: the parts that rarely or never change.
///
/// Split from [`EntityState`] on purpose. State is volatile, unreliable and
/// sent every tick; a descriptor is what a client needs to *create* the thing
/// in the first place, so it travels on the reliable channel and only when it
/// changes. Without the split, either every datagram would carry a tag string
/// or a joining client would see poses for entities it cannot build.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub struct EntityDesc {
    pub id: u64,
    /// Where it was created. Statics never move and are therefore absent from
    /// the per-tick state stream, so this is the only place a joining client
    /// learns where the ground and the scenery are.
    pub pos: [f32; 3],
    pub half: [f32; 3],
    pub color: [f32; 4],
    pub scale: [f32; 3],
    /// BodyKind discriminant, mapped by the session layer.
    pub kind: u8,
    /// Visual shape discriminant.
    pub shape: u8,
    pub glow: f32,
    pub flags: u16,
    pub tag: String,
}

impl EntityDesc {
    pub fn is_finite(&self) -> bool {
        self.pos.iter().all(|v| v.is_finite())
            && self.half.iter().all(|v| v.is_finite())
            && self.color.iter().all(|v| v.is_finite())
            && self.scale.iter().all(|v| v.is_finite())
            && self.glow.is_finite()
    }
}

/// One player's input for a tick. Camera yaw travels with the input so the
/// host can resolve camera-relative movement itself — the client's camera rig
/// stays pure presentation (game.md §Multiplayer model).
#[derive(Clone, Copy, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct InputFrame {
    pub tick: u64,
    pub axis_x: f32,
    pub axis_z: f32,
    pub cam_yaw: f32,
    pub buttons: u32,
}

impl InputFrame {
    pub fn is_finite(&self) -> bool {
        self.axis_x.is_finite() && self.axis_z.is_finite() && self.cam_yaw.is_finite()
    }
}

/// Gameplay-level requests a client may ask the host to perform. The host is
/// free to ignore any of them; nothing here mutates state directly.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub enum Intent {
    Respawn,
    Custom { kind: u32, a: f64, b: f64 },
    /// "Make the cars faster" from a device with no API key of its own: the
    /// host owns the agent, so a keyless client asks it to author instead.
    /// Text off the wire is untrusted and unbounded — see `MAX_AUTHORING_TEXT`.
    Authoring { text: String },
}

/// Longest authoring request the host will accept. A prompt is a sentence, not
/// a payload; anything past this is someone probing for a buffer to grow.
pub const MAX_AUTHORING_TEXT: usize = 4096;

/// Host-authored events on the reliable channel.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub enum GameEvent {
    Spawn { id: u64, kind: u32, state: EntityState },
    Remove { id: u64 },
    Sfx { name: u64, pos: [f32; 3] },
    Score { player: PlayerId, value: i64 },
    Custom { kind: u32, a: f64, b: f64 },
}

/// Client → host. Everything here is a request.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub enum ClientToHost {
    /// `udp_port` lets the host pin the datagram address immediately. Only the
    /// port is taken from the message — the IP always comes from the
    /// authenticated TCP connection, so this cannot redirect traffic off-host.
    Join {
        protocol: u16,
        name: String,
        udp_port: u16,
    },
    Input { frame: InputFrame },
    Intent { intent: Intent },
    Leave,
    Ping { nonce: u64 },
}

/// Host → client. Everything here is authoritative.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub enum HostToClient {
    Welcome {
        player_id: PlayerId,
        tick: u64,
        snapshot: Vec<EntityState>,
    },
    StateBatch {
        tick: u64,
        entities: Vec<EntityState>,
    },
    /// Construction data for entities the client may not have yet. Reliable
    /// and ordered, sent after `Welcome` for the whole world and thereafter
    /// only for what actually appeared or was restyled.
    Descriptors {
        tick: u64,
        descs: Vec<EntityDesc>,
    },
    Event {
        tick: u64,
        event: GameEvent,
    },
    Bye {
        reason: LeaveReason,
    },
    Pong {
        nonce: u64,
    },
}

/// Host presence beacon, broadcast on the discovery port.
#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub struct Announce {
    pub protocol: u16,
    pub host_id: HostId,
    pub name: String,
    pub players: u16,
    pub max_players: u16,
    pub tcp_port: u16,
    pub udp_port: u16,
}

/// Delivery class, which decides what gets dropped when a peer stops draining
/// (audit P1: `write_buf` had no backpressure at all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketClass {
    /// Superseded by the next tick — drop the oldest under pressure.
    State,
    /// Must arrive; disconnect rather than silently drop.
    Control,
}

/// Authenticated envelope shared by both sockets:
/// `magic | version | sender | payload | mac`, where the MAC covers everything
/// before it. Verified before any peer state is consulted or mutated.
pub struct Envelope;

const HEADER_LEN: usize = 2 + 2 + 8;

impl Envelope {
    pub fn seal(sender: u64, payload: &[u8], key: &LobbyKey) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + MAC_LEN);
        out.extend_from_slice(&PROTOCOL_MAGIC.to_le_bytes());
        out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        out.extend_from_slice(&sender.to_le_bytes());
        out.extend_from_slice(payload);
        let mac = key.mac(&out);
        out.extend_from_slice(&mac);
        out
    }

    /// Returns `(sender, payload)` only for a well-formed, correctly signed
    /// datagram of a matching protocol version.
    pub fn open<'a>(datagram: &'a [u8], key: &LobbyKey) -> Option<(u64, &'a [u8])> {
        if datagram.len() < HEADER_LEN + MAC_LEN {
            return None;
        }
        let magic = u16::from_le_bytes([datagram[0], datagram[1]]);
        let version = u16::from_le_bytes([datagram[2], datagram[3]]);
        if magic != PROTOCOL_MAGIC || version != PROTOCOL_VERSION {
            return None;
        }
        let split = datagram.len() - MAC_LEN;
        if !key.verify(&datagram[..split], &datagram[split..]) {
            return None;
        }
        let sender = u64::from_le_bytes(datagram[4..12].try_into().ok()?);
        Some((sender, &datagram[HEADER_LEN..split]))
    }
}

/// Length-prefixed, optionally LZ4-compressed, authenticated frames for the
/// reliable stream. Harvested from the XR sync codec — the size checks precede
/// every allocation — with authentication added and the cap lowered.
pub struct FrameCodec;

impl FrameCodec {
    /// Encodes one frame including its 4-byte length prefix. Compression is
    /// only kept when it actually wins, and the *post*-compression size decides
    /// acceptance, so a payload that fits compressed is not dropped (the audit
    /// flagged the reverse).
    pub fn encode(sender: u64, payload: &[u8], key: &LobbyKey) -> Option<Vec<u8>> {
        let mut body = Vec::with_capacity(1 + payload.len());
        body.push(FRAME_RAW_TAG);
        body.extend_from_slice(payload);

        let bound = compress_bound(payload.len());
        if bound != 0 {
            let mut compressed = vec![0u8; bound];
            if let Ok(len) = compress_fast_into(payload, &mut compressed, LZ4_ACCELERATION) {
                if 1 + 4 + len < body.len() {
                    let mut alt = Vec::with_capacity(1 + 4 + len);
                    alt.push(FRAME_LZ4_TAG);
                    alt.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                    alt.extend_from_slice(&compressed[..len]);
                    body = alt;
                }
            }
        }

        let sealed = Envelope::seal(sender, &body, key);
        if sealed.len() > MAX_FRAME_BYTES {
            return None;
        }
        let mut frame = Vec::with_capacity(4 + sealed.len());
        frame.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        frame.extend_from_slice(&sealed);
        Some(frame)
    }

    fn decode_body(body: &[u8]) -> io::Result<Vec<u8>> {
        let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidData, msg.to_string());
        let (&tag, rest) = body.split_first().ok_or_else(|| invalid("empty frame"))?;
        match tag {
            FRAME_RAW_TAG => Ok(rest.to_vec()),
            FRAME_LZ4_TAG => {
                if rest.len() < 4 {
                    return Err(invalid("compressed frame missing length"));
                }
                let decoded_len =
                    u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
                // Cap checked before allocating: a compression bomb cannot make
                // the receiver reserve more than one frame's worth.
                if decoded_len > MAX_FRAME_BYTES {
                    return Err(invalid("compressed frame too large"));
                }
                let mut decoded = vec![0u8; decoded_len];
                let written = decompress_safe(&rest[4..], &mut decoded)
                    .map_err(|_| invalid("failed to decompress frame"))?;
                if written != decoded_len {
                    return Err(invalid("decoded length mismatch"));
                }
                Ok(decoded)
            }
            _ => Err(invalid("unknown frame tag")),
        }
    }

    /// Pulls every complete, authenticated frame out of a stream buffer,
    /// leaving any partial tail in place. Returns `(sender, payload)` pairs.
    pub fn drain(read_buf: &mut Vec<u8>, key: &LobbyKey) -> io::Result<Vec<(u64, Vec<u8>)>> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while read_buf.len().saturating_sub(offset) >= 4 {
            let len = u32::from_le_bytes([
                read_buf[offset],
                read_buf[offset + 1],
                read_buf[offset + 2],
                read_buf[offset + 3],
            ]) as usize;
            if len > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame exceeds max size",
                ));
            }
            if read_buf.len().saturating_sub(offset + 4) < len {
                break;
            }
            let start = offset + 4;
            let sealed = &read_buf[start..start + len];
            let (sender, body) = Envelope::open(sealed, key).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "frame failed authentication")
            })?;
            out.push((sender, Self::decode_body(body)?));
            offset = start + len;
        }
        if offset > 0 {
            consume_prefix(read_buf, offset);
        }
        Ok(out)
    }
}

pub fn consume_prefix(buf: &mut Vec<u8>, len: usize) {
    if len == 0 {
        return;
    }
    if len >= buf.len() {
        buf.clear();
        return;
    }
    let remaining = buf.len() - len;
    buf.copy_within(len.., 0);
    buf.truncate(remaining);
}

/// Splits entity states into datagram-sized batches, tracking the encoded size
/// incrementally. The XR version re-serialized the whole batch per pushed
/// entity (audit M-6, O(n²)); this measures one entity at a time.
pub fn batch_entities(tick: u64, entities: &[EntityState]) -> Vec<HostToClient> {
    if entities.is_empty() {
        return Vec::new();
    }
    // Cost of the enum tag, tick and vector length prefix.
    let envelope_overhead = {
        let probe = HostToClient::StateBatch {
            tick,
            entities: Vec::new(),
        };
        probe.serialize_bin().len()
    };
    let per_entity = entities[0].serialize_bin().len();

    let capacity = MAX_DATAGRAM_PAYLOAD.saturating_sub(envelope_overhead);
    let per_batch = (capacity / per_entity.max(1)).max(1);

    entities
        .chunks(per_batch)
        .map(|chunk| HostToClient::StateBatch {
            tick,
            entities: chunk.to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> LobbyKey {
        LobbyKey::new(b"test-lobby")
    }

    fn entity(id: u64) -> EntityState {
        EntityState {
            id,
            seq: 1,
            pos: [1.0, 2.0, 3.0],
            vel: [0.0, -1.0, 0.0],
            yaw: 0.5,
            flags: 0,
        }
    }

    #[test]
    fn envelope_roundtrips_and_rejects_tampering() {
        let key = key();
        let sealed = Envelope::seal(7, b"hello", &key);
        assert_eq!(Envelope::open(&sealed, &key), Some((7, &b"hello"[..])));

        // Any single-bit edit anywhere invalidates the tag.
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 1;
            assert!(Envelope::open(&bad, &key).is_none(), "byte {i}");
        }
        // Wrong lobby key.
        assert!(Envelope::open(&sealed, &LobbyKey::new(b"other")).is_none());
        // Truncation.
        for cut in 0..sealed.len() {
            assert!(Envelope::open(&sealed[..cut], &key).is_none());
        }
    }

    #[test]
    fn frame_codec_roundtrips_raw_and_compressed() {
        let key = key();
        let small = b"tiny".to_vec();
        let big = vec![0x42u8; 8192]; // compresses hard

        let mut buf = Vec::new();
        buf.extend(FrameCodec::encode(1, &small, &key).unwrap());
        let big_frame = FrameCodec::encode(1, &big, &key).unwrap();
        assert!(big_frame.len() < big.len() / 4, "should have compressed");
        buf.extend(big_frame);

        let drained = FrameCodec::drain(&mut buf, &key).unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], (1, small));
        assert_eq!(drained[1], (1, big));
        assert!(buf.is_empty());
    }

    #[test]
    fn frame_drain_keeps_partial_tail() {
        let key = key();
        let frame = FrameCodec::encode(1, b"payload", &key).unwrap();
        let mut buf = frame[..frame.len() - 3].to_vec();
        assert!(FrameCodec::drain(&mut buf, &key).unwrap().is_empty());
        buf.extend_from_slice(&frame[frame.len() - 3..]);
        let drained = FrameCodec::drain(&mut buf, &key).unwrap();
        assert_eq!(drained, vec![(1, b"payload".to_vec())]);
    }

    #[test]
    fn frame_drain_rejects_oversized_and_unauthenticated() {
        let key = key();
        let mut buf = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        assert!(FrameCodec::drain(&mut buf, &key).is_err());

        // Correctly framed but signed with the wrong key.
        let forged = FrameCodec::encode(1, b"x", &LobbyKey::new(b"attacker")).unwrap();
        let mut buf = forged;
        assert!(FrameCodec::drain(&mut buf, &key).is_err());
    }

    #[test]
    fn batching_stays_under_the_datagram_budget() {
        let entities: Vec<_> = (0..200).map(entity).collect();
        let batches = batch_entities(9, &entities);
        assert!(batches.len() > 1, "200 entities must span datagrams");
        let mut total = 0;
        for batch in &batches {
            let bytes = batch.serialize_bin().len();
            assert!(bytes <= MAX_DATAGRAM_PAYLOAD, "batch of {bytes} bytes");
            if let HostToClient::StateBatch { entities, .. } = batch {
                total += entities.len();
            }
        }
        assert_eq!(total, 200);
    }

    #[test]
    fn non_finite_states_are_detectable() {
        let mut e = entity(1);
        assert!(e.is_finite());
        e.pos[1] = f32::NAN;
        assert!(!e.is_finite());
        e.pos[1] = f32::INFINITY;
        assert!(!e.is_finite());
    }
}
