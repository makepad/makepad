//! The packet on the wire.
//!
//! Every datagram starts with a fixed 24-byte header, little-endian:
//!
//! ```text
//! offset  size  field
//!      0     2  magic      b"TT"
//!      2     1  version    1
//!      3     1  codec      0 = raw i16 mono, 1 = ogg (payload opaque to this crate)
//!      4     1  channel    team channel: 0 = everyone hears it, 1..=255 = a team
//!      5     1  flags      bit0 SILENCE (no payload, `frames` zeros), bit1 HELLO
//!                          (presence only, no audio position), bit2 TALK_START,
//!                          bit3 BYE
//!      6     2  frames     samples per channel in this packet (0 for HELLO/BYE)
//!      8     8  room       room tag: receivers drop packets from other rooms
//!                          before they touch any peer state
//!     16     8  sender     application identity of the sender
//!     24     4  seq        frame sequence number, +1 per frame (silence included)
//!     28     4  timestamp  sender sample clock at [`INTERNAL_RATE`] of the first sample
//!     32     -  payload    codec specific; raw i16: `frames` little-endian i16
//! ```
//!
//! Audio is mono at [`INTERNAL_RATE`] on the wire.
//!
//! `room` scopes a session: two chats on one LAN must not hear each other,
//! and `sender` ids are only unique *within* a room, so a receiver rejects a
//! foreign room tag before the packet reaches the peer table or a jitter
//! buffer. The application derives the tag from its session secret (e.g. the
//! first 8 bytes of a keyed MAC over a fixed label); 0 is the default
//! "public" room.
//!
//! `sender` is what a receiver keys its per-peer state on, and is handed
//! back per rendered frame so an application can map a voice to a player
//! entity; the source address is only used to unicast back. In the sandbox
//! session protocol it is the net player id, and the HOST is
//! [`HOST_SENDER_ID`] (`u64::MAX`) — the same sentinel as the player-body
//! table; do not invent another.

/// Sample rate of everything on the wire and inside the crate, in Hz.
pub const INTERNAL_RATE: f64 = 48_000.0;
/// Magic at offset 0 of every packet.
pub const MAGIC: [u8; 2] = *b"TT";
/// Wire version this crate speaks.
pub const VERSION: u8 = 1;
/// Fixed header size in bytes.
pub const HEADER_LEN: usize = 32;
/// The `sender` id of a session HOST, by convention shared with the game's
/// player-body table. Only meaningful within a room (see the module docs).
pub const HOST_SENDER_ID: u64 = u64::MAX;
/// Largest frame (samples) a packet may carry: 20 ms at 48 kHz.
pub const MAX_FRAME: usize = 960;
/// Largest payload in bytes (raw i16 of [`MAX_FRAME`]).
pub const MAX_PAYLOAD: usize = MAX_FRAME * 2;
/// Largest datagram this crate sends or accepts.
pub const MAX_PACKET: usize = HEADER_LEN + MAX_PAYLOAD;

/// Payload codec identifier. `Ogg` is reserved for a compressed payload that a
/// codec layer above this crate produces and consumes; this crate forwards
/// the bytes untouched and cannot render them.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Little-endian signed 16-bit PCM, mono, `frames` samples.
    RawI16 = 0,
    /// Compressed payload; opaque here.
    Ogg = 1,
}

impl Codec {
    pub fn from_u8(v: u8) -> Option<Codec> {
        match v {
            0 => Some(Codec::RawI16),
            1 => Some(Codec::Ogg),
            _ => None,
        }
    }
}

/// Header flag bits.
pub mod flags {
    /// The frame is silence: no payload, the receiver plays `frames` zeros.
    pub const SILENCE: u8 = 1;
    /// Presence/keepalive only; carries no audio and no sequence position.
    pub const HELLO: u8 = 2;
    /// First audio frame after silence (informational).
    pub const TALK_START: u8 = 4;
    /// The sender is leaving; the receiver may drop the peer at once.
    pub const BYE: u8 = 8;
}

/// The decoded fixed header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub codec: Codec,
    pub channel: u8,
    pub flags: u8,
    pub frames: u16,
    /// Session/room tag; receivers drop foreign rooms outright.
    pub room: u64,
    pub sender: u64,
    pub seq: u32,
    pub timestamp: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    TooShort,
    BadMagic,
    BadVersion(u8),
    UnknownCodec(u8),
    FrameCount(u16),
    PayloadLength { expected: usize, got: usize },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::TooShort => write!(f, "packet shorter than the header"),
            WireError::BadMagic => write!(f, "bad magic"),
            WireError::BadVersion(v) => write!(f, "unsupported wire version {v}"),
            WireError::UnknownCodec(c) => write!(f, "unknown codec {c}"),
            WireError::FrameCount(n) => write!(f, "frame count {n} out of range"),
            WireError::PayloadLength { expected, got } => {
                write!(f, "payload length {got}, expected {expected}")
            }
        }
    }
}

impl std::error::Error for WireError {}

impl Header {
    pub fn is_hello(&self) -> bool {
        self.flags & flags::HELLO != 0
    }
    pub fn is_silence(&self) -> bool {
        self.flags & flags::SILENCE != 0
    }
    pub fn is_bye(&self) -> bool {
        self.flags & flags::BYE != 0
    }
    /// Whether the packet carries an audio sequence position (frames) at all.
    pub fn has_frame(&self) -> bool {
        !self.is_hello() && !self.is_bye() && self.frames > 0
    }

    /// Serialise into `out[..HEADER_LEN]`. `out` must be at least [`HEADER_LEN`].
    pub fn write(&self, out: &mut [u8]) {
        out[0..2].copy_from_slice(&MAGIC);
        out[2] = VERSION;
        out[3] = self.codec as u8;
        out[4] = self.channel;
        out[5] = self.flags;
        out[6..8].copy_from_slice(&self.frames.to_le_bytes());
        out[8..16].copy_from_slice(&self.room.to_le_bytes());
        out[16..24].copy_from_slice(&self.sender.to_le_bytes());
        out[24..28].copy_from_slice(&self.seq.to_le_bytes());
        out[28..32].copy_from_slice(&self.timestamp.to_le_bytes());
    }

    /// Parse a datagram: validates the header and, for codecs this crate
    /// knows, that the payload length matches `frames`. Returns the header
    /// and the payload slice.
    pub fn parse(buf: &[u8]) -> Result<(Header, &[u8]), WireError> {
        if buf.len() < HEADER_LEN {
            return Err(WireError::TooShort);
        }
        if buf[0..2] != MAGIC {
            return Err(WireError::BadMagic);
        }
        if buf[2] != VERSION {
            return Err(WireError::BadVersion(buf[2]));
        }
        let codec = Codec::from_u8(buf[3]).ok_or(WireError::UnknownCodec(buf[3]))?;
        let header = Header {
            codec,
            channel: buf[4],
            flags: buf[5],
            frames: u16::from_le_bytes([buf[6], buf[7]]),
            room: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            sender: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            seq: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
            timestamp: u32::from_le_bytes(buf[28..32].try_into().unwrap()),
        };
        let payload = &buf[HEADER_LEN..];
        if header.frames as usize > MAX_FRAME {
            return Err(WireError::FrameCount(header.frames));
        }
        let expected = if header.is_hello() || header.is_bye() || header.is_silence() {
            0
        } else {
            if header.frames == 0 {
                return Err(WireError::FrameCount(0));
            }
            match codec {
                Codec::RawI16 => header.frames as usize * 2,
                // Opaque: any length.
                Codec::Ogg => payload.len(),
            }
        };
        if payload.len() != expected {
            return Err(WireError::PayloadLength {
                expected,
                got: payload.len(),
            });
        }
        Ok((header, payload))
    }
}

/// Encode `samples` (f32, -1..1) as a raw-i16 audio packet. Returns the
/// datagram length. `header.frames` and `header.codec` are overwritten.
pub fn encode_raw_i16(mut header: Header, samples: &[f32], out: &mut [u8; MAX_PACKET]) -> usize {
    let n = samples.len().min(MAX_FRAME);
    header.codec = Codec::RawI16;
    header.frames = n as u16;
    header.flags &= !flags::SILENCE;
    header.write(&mut out[..HEADER_LEN]);
    for (i, &s) in samples[..n].iter().enumerate() {
        let v = (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        let o = HEADER_LEN + i * 2;
        out[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    HEADER_LEN + n * 2
}

/// Encode a header-only packet (silence, hello, bye). Returns the length.
pub fn encode_header_only(header: Header, out: &mut [u8; MAX_PACKET]) -> usize {
    header.write(&mut out[..HEADER_LEN]);
    HEADER_LEN
}

/// Decode a raw-i16 payload into `out`, returning the number of samples.
pub fn decode_raw_i16(payload: &[u8], out: &mut [i16]) -> usize {
    let n = (payload.len() / 2).min(out.len());
    for i in 0..n {
        out[i] = i16::from_le_bytes([payload[i * 2], payload[i * 2 + 1]]);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            codec: Codec::RawI16,
            channel: 2,
            flags: flags::TALK_START,
            frames: 0,
            room: 0xA5A5_0000_FFFF_1234,
            sender: 0x1122_3344_5566_7788,
            seq: 4_000_000_000,
            timestamp: 123_456,
        }
    }

    #[test]
    fn audio_packet_round_trips() {
        let samples: Vec<f32> = (0..240).map(|i| ((i as f32) / 240.0) * 2.0 - 1.0).collect();
        let mut out = [0u8; MAX_PACKET];
        let len = encode_raw_i16(header(), &samples, &mut out);
        assert_eq!(len, HEADER_LEN + 480);
        let (h, payload) = Header::parse(&out[..len]).unwrap();
        assert_eq!(h.codec, Codec::RawI16);
        assert_eq!(h.channel, 2);
        assert_eq!(h.flags, flags::TALK_START);
        assert_eq!(h.frames, 240);
        assert_eq!(h.room, 0xA5A5_0000_FFFF_1234);
        assert_eq!(h.sender, 0x1122_3344_5566_7788);
        assert_eq!(h.seq, 4_000_000_000);
        assert_eq!(h.timestamp, 123_456);
        let mut pcm = [0i16; MAX_FRAME];
        assert_eq!(decode_raw_i16(payload, &mut pcm), 240);
        for i in 0..240 {
            let back = pcm[i] as f32 / 32767.0;
            assert!((back - samples[i]).abs() < 1.0 / 32767.0 + 1e-6, "sample {i}");
        }
    }

    #[test]
    fn header_only_packets_round_trip() {
        let mut out = [0u8; MAX_PACKET];
        let mut h = header();
        h.flags = flags::SILENCE;
        h.frames = 240;
        let len = encode_header_only(h, &mut out);
        assert_eq!(len, HEADER_LEN);
        let (p, payload) = Header::parse(&out[..len]).unwrap();
        assert!(p.is_silence() && p.has_frame());
        assert_eq!(p.frames, 240);
        assert!(payload.is_empty());

        h.flags = flags::HELLO;
        h.frames = 0;
        let len = encode_header_only(h, &mut out);
        let (p, _) = Header::parse(&out[..len]).unwrap();
        assert!(p.is_hello() && !p.has_frame());
    }

    #[test]
    fn malformed_packets_are_refused() {
        let mut out = [0u8; MAX_PACKET];
        let len = encode_raw_i16(header(), &[0.0; 240], &mut out);
        assert_eq!(Header::parse(&out[..HEADER_LEN - 1]), Err(WireError::TooShort));
        // Truncated payload.
        assert!(matches!(
            Header::parse(&out[..len - 2]),
            Err(WireError::PayloadLength { expected: 480, got: 478 })
        ));
        let mut bad = out;
        bad[0] = b'X';
        assert_eq!(Header::parse(&bad[..len]), Err(WireError::BadMagic));
        let mut bad = out;
        bad[2] = 9;
        assert_eq!(Header::parse(&bad[..len]), Err(WireError::BadVersion(9)));
        let mut bad = out;
        bad[3] = 7;
        assert_eq!(Header::parse(&bad[..len]), Err(WireError::UnknownCodec(7)));
        let mut bad = out;
        bad[6..8].copy_from_slice(&2000u16.to_le_bytes());
        assert_eq!(Header::parse(&bad[..len]), Err(WireError::FrameCount(2000)));
        // Audio with zero frames is meaningless.
        let mut bad = out;
        bad[6..8].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(Header::parse(&bad[..HEADER_LEN]), Err(WireError::FrameCount(0)));
    }

    #[test]
    fn the_host_sentinel_round_trips() {
        let mut h = header();
        h.sender = HOST_SENDER_ID;
        h.flags = flags::HELLO;
        let mut out = [0u8; MAX_PACKET];
        let len = encode_header_only(h, &mut out);
        let (p, _) = Header::parse(&out[..len]).unwrap();
        assert_eq!(p.sender, u64::MAX);
    }

    #[test]
    fn ogg_payload_is_opaque() {
        let mut out = [0u8; MAX_PACKET];
        let mut h = header();
        h.codec = Codec::Ogg;
        h.frames = 480;
        h.write(&mut out[..HEADER_LEN]);
        out[HEADER_LEN..HEADER_LEN + 37].fill(0xAB);
        let (p, payload) = Header::parse(&out[..HEADER_LEN + 37]).unwrap();
        assert_eq!(p.codec, Codec::Ogg);
        assert_eq!(payload.len(), 37);
    }

    #[test]
    fn encode_clamps_and_rounds() {
        let mut out = [0u8; MAX_PACKET];
        let len = encode_raw_i16(header(), &[2.0, -2.0, 0.5], &mut out);
        let (_, payload) = Header::parse(&out[..len]).unwrap();
        let mut pcm = [0i16; 3];
        decode_raw_i16(payload, &mut pcm);
        assert_eq!(pcm, [32767, -32768, 16384]);
    }
}
