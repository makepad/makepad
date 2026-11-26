//! The bytes of a call's datagrams.
//!
//! Every media datagram, feedback included, has one layout (fixed fields big-endian):
//!
//! ```text
//! off  len
//! 0    1    0xe2      media, beside the link stream's 0xe1 and the punch probe's 0xe0
//! 1    2    hop       per link and direction, +1 for every media datagram sent on it
//! 3    2    src       the sender's tag
//! 5    1    stream    low 4 bits: the stream id; high 4 bits: key frame starts, frame ends,
//!                     talking, and one zero bit
//! 6    1    key id    0..=127 pairwise secrets, 128..=255 a secret shared by everyone
//! 7    6    seq       per (sender, stream, key): the sealer's own counter
//! 13   n    ciphertext
//! 13+n 16   tag over bytes 3..13+n (the seal's associated data is bytes 3..7)
//! ```
//!
//! `hop` sits outside the tag because a forwarder rewrites it; a receiver counts it only for a
//! datagram whose tag verified. The seal itself (`makepad_network::p2p::seal`) knows nothing of
//! this layout: it authenticates `bytes[3..7] | seq | ciphertext`.
//!
//! Inside the ciphertext, a flags byte first, then:
//! - voice: `flags | frame seq u32 | capture_us u64 | payload` (one coded frame);
//! - video: `flags | frame u32 | base u16 | index u16 | count u16 | capture_us u64 | payload`
//!   (one fragment of one access unit; `base` is how many frames back the frame it predicts
//!   from is, 1 for the plain chain, 0 for a key frame);
//! - feedback: sections `type u8 | length u16 | body`; a receiver skips types it doesn't know.
//!
//! `capture_us` is the sender's media clock: one monotonic microsecond clock for all of its
//! streams, so a receiver lines voice and pictures up. Every reader here is total: short or
//! malformed bytes are `None`, never a panic.

/// The first byte of every media datagram on the link's socket.
pub const MEDIA_BYTE: u8 = 0xe2;
/// The clear header: media byte, hop, src, stream and flags, key id, seq.
pub const HEADER: usize = 13;
/// The seal's tag.
pub const TAG: usize = 16;
/// A datagram's largest size on the link.
pub const MAX_DATAGRAM: usize = 1200;
/// The largest plaintext one datagram carries.
pub const MAX_PLAINTEXT: usize = MAX_DATAGRAM - HEADER - TAG;
/// The opener's replay window per stream kind, in sequences: video's is wide because resends
/// of lost fragments come late.
pub const VOICE_WINDOW: usize = 128;
pub const VIDEO_WINDOW: usize = 2048;
/// Where the seal's associated data sits in the clear header.
pub const AAD: std::ops::Range<usize> = 3..7;

/// The stream ids (low four bits of the stream byte).
pub mod stream {
    pub const VOICE: u8 = 0;
    pub const CAMERA_LOW: u8 = 1;
    pub const CAMERA_HIGH: u8 = 2;
    pub const SHARE: u8 = 3;
    pub const FEEDBACK: u8 = 15;

    pub fn is_video(id: u8) -> bool {
        matches!(id, CAMERA_LOW | CAMERA_HIGH | SHARE)
    }
}

/// The high four bits of the stream byte, readable without opening the datagram (a forwarder
/// switches layers at key frames and finds the speaker from them).
pub mod stream_flags {
    pub const KEY_START: u8 = 0x10;
    pub const FRAME_END: u8 = 0x20;
    pub const TALKING: u8 = 0x40;
    /// Must be zero.
    pub const RESERVED: u8 = 0x80;
}

/// The clear header of one media datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub hop: u16,
    pub src: u16,
    pub stream: u8,
    pub flags: u8,
    pub key_id: u8,
    pub seq: u64,
}

impl Header {
    /// Splits a datagram into its header and the sealed part (`seq | ciphertext | tag`, what the
    /// opener takes); `None` for anything that can't be a media datagram.
    pub fn parse(bytes: &[u8]) -> Option<(Header, &[u8])> {
        if bytes.len() < HEADER + TAG || bytes.len() > MAX_DATAGRAM || bytes[0] != MEDIA_BYTE {
            return None;
        }
        let byte5 = bytes[5];
        if byte5 & stream_flags::RESERVED != 0 {
            return None;
        }
        let mut seq = [0u8; 8];
        seq[2..].copy_from_slice(&bytes[7..13]);
        let header = Header {
            hop: u16::from_be_bytes([bytes[1], bytes[2]]),
            src: u16::from_be_bytes([bytes[3], bytes[4]]),
            stream: byte5 & 0x0f,
            flags: byte5 & 0xf0,
            key_id: bytes[6],
            seq: u64::from_be_bytes(seq),
        };
        Some((header, &bytes[7..]))
    }

    /// The four bytes the seal authenticates in front of its own: `src | stream byte | key id`.
    pub fn aad(src: u16, stream: u8, flags: u8, key_id: u8) -> [u8; 4] {
        let s = src.to_be_bytes();
        [s[0], s[1], (stream & 0x0f) | (flags & 0x70), key_id]
    }

    /// Starts a datagram in `out` (cleared): the media byte, `hop`, then the associated data.
    /// The sealer appends `seq | ciphertext | tag`.
    pub fn begin(out: &mut Vec<u8>, hop: u16, aad: &[u8; 4]) {
        out.clear();
        out.push(MEDIA_BYTE);
        out.extend_from_slice(&hop.to_be_bytes());
        out.extend_from_slice(aad);
    }

    /// Rewrites the hop of a sealed datagram (a resend, a forwarder's next hop).
    pub fn set_hop(datagram: &mut [u8], hop: u16) {
        datagram[1..3].copy_from_slice(&hop.to_be_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u48(&mut self) -> Option<u64> {
        let mut b = [0u8; 8];
        b[2..].copy_from_slice(self.take(6)?);
        Some(u64::from_be_bytes(b))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn rest(&mut self) -> &'a [u8] {
        let s = &self.bytes[self.at..];
        self.at = self.bytes.len();
        s
    }
    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

pub mod voice_flags {
    /// No audio in this frame (gate closed or muted); the payload is empty.
    pub const SILENT: u8 = 1;
    /// Speech starts in this frame.
    pub const TALK_START: u8 = 2;
}

/// One coded voice frame (the plaintext of a voice datagram).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voice<'a> {
    pub flags: u8,
    pub seq: u32,
    pub capture_us: u64,
    pub payload: &'a [u8],
}

pub const VOICE_HEADER: usize = 1 + 4 + 8;

impl<'a> Voice<'a> {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.clear();
        out.push(self.flags);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.capture_us.to_be_bytes());
        out.extend_from_slice(self.payload);
    }

    pub fn read(bytes: &'a [u8]) -> Option<Self> {
        let mut r = Reader::new(bytes);
        Some(Voice { flags: r.u8()?, seq: r.u32()?, capture_us: r.u64()?, payload: r.rest() })
    }
}

pub mod video_flags {
    /// The access unit is a key frame (it begins with its parameter sets).
    pub const KEY: u8 = 1;
}

/// One fragment of one encoded access unit (the plaintext of a video datagram).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fragment<'a> {
    pub flags: u8,
    pub frame: u32,
    pub base: u16,
    pub index: u16,
    pub count: u16,
    pub capture_us: u64,
    pub payload: &'a [u8],
}

pub const FRAGMENT_HEADER: usize = 1 + 4 + 2 + 2 + 2 + 8;
/// The most access-unit bytes one fragment carries.
pub const MAX_FRAGMENT: usize = MAX_PLAINTEXT - FRAGMENT_HEADER;

impl<'a> Fragment<'a> {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.clear();
        out.push(self.flags);
        out.extend_from_slice(&self.frame.to_be_bytes());
        out.extend_from_slice(&self.base.to_be_bytes());
        out.extend_from_slice(&self.index.to_be_bytes());
        out.extend_from_slice(&self.count.to_be_bytes());
        out.extend_from_slice(&self.capture_us.to_be_bytes());
        out.extend_from_slice(self.payload);
    }

    /// Valid only with `index < count` and a payload.
    pub fn read(bytes: &'a [u8]) -> Option<Self> {
        let mut r = Reader::new(bytes);
        let f = Fragment {
            flags: r.u8()?,
            frame: r.u32()?,
            base: r.u16()?,
            index: r.u16()?,
            count: r.u16()?,
            capture_us: r.u64()?,
            payload: r.rest(),
        };
        (f.index < f.count && !f.payload.is_empty()).then_some(f)
    }

    pub fn is_key(&self) -> bool {
        self.flags & video_flags::KEY != 0
    }
}

/// "Not seen" in an ARRIVALS section.
pub const NOT_SEEN: u16 = 0xffff;

/// One section of a feedback datagram's plaintext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    /// The hops from `base` on, each as its age before `report_time` in 0.1 ms units on the
    /// receiver's clock, or [`NOT_SEEN`].
    Arrivals { base: u16, report_time: u32, ages: Vec<u16> },
    /// Ranges of sequences missing from one sender's stream under one key.
    Nack(Vec<NackRange>),
    /// Streams the receiver can't go on without a key frame of.
    Key(Vec<(u16, u8)>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NackRange {
    pub src: u16,
    pub stream: u8,
    pub key_id: u8,
    pub first: u64,
    pub count: u8,
}

mod section_type {
    pub const ARRIVALS: u8 = 1;
    pub const NACK: u8 = 2;
    pub const KEY: u8 = 3;
}

/// Writes sections into one feedback plaintext (cleared first). The caller keeps it under
/// [`MAX_PLAINTEXT`] by what it puts in (ARRIVALS of at most [`MAX_ARRIVALS`] hops).
pub fn write_feedback(sections: &[Section], out: &mut Vec<u8>) {
    out.clear();
    out.push(0); // the flags byte, as every plaintext starts
    for section in sections {
        let (ty, at) = (
            match section {
                Section::Arrivals { .. } => section_type::ARRIVALS,
                Section::Nack(_) => section_type::NACK,
                Section::Key(_) => section_type::KEY,
            },
            out.len(),
        );
        out.push(ty);
        out.extend_from_slice(&[0, 0]);
        match section {
            Section::Arrivals { base, report_time, ages } => {
                out.extend_from_slice(&base.to_be_bytes());
                out.extend_from_slice(&report_time.to_be_bytes());
                out.extend_from_slice(&(ages.len() as u16).to_be_bytes());
                for a in ages {
                    out.extend_from_slice(&a.to_be_bytes());
                }
            }
            Section::Nack(ranges) => {
                for n in ranges {
                    out.extend_from_slice(&n.src.to_be_bytes());
                    out.push(n.stream);
                    out.push(n.key_id);
                    out.extend_from_slice(&n.first.to_be_bytes()[2..]);
                    out.push(n.count);
                }
            }
            Section::Key(keys) => {
                for (src, stream) in keys {
                    out.extend_from_slice(&src.to_be_bytes());
                    out.push(*stream);
                }
            }
        }
        let len = (out.len() - at - 3) as u16;
        out[at + 1..at + 3].copy_from_slice(&len.to_be_bytes());
    }
}

/// The most hops one ARRIVALS section reports, so a feedback datagram always fits.
pub const MAX_ARRIVALS: usize = (MAX_PLAINTEXT - 1 - 3 - 8 - 3 - 120) / 2;

/// The sections of one feedback plaintext; `None` when malformed (a known section with a body
/// that doesn't parse whole, a section running past the end).
pub fn read_feedback(bytes: &[u8]) -> Option<Vec<Section>> {
    let mut r = Reader::new(bytes);
    let _flags = r.u8()?;
    let mut sections = Vec::new();
    while !r.done() {
        let ty = r.u8()?;
        let len = r.u16()? as usize;
        let mut b = Reader::new(r.take(len)?);
        let section = match ty {
            section_type::ARRIVALS => {
                let base = b.u16()?;
                let report_time = b.u32()?;
                let count = b.u16()? as usize;
                let mut ages = Vec::with_capacity(count.min(len / 2));
                for _ in 0..count {
                    ages.push(b.u16()?);
                }
                Section::Arrivals { base, report_time, ages }
            }
            section_type::NACK => {
                let mut ranges = Vec::new();
                while !b.done() {
                    ranges.push(NackRange { src: b.u16()?, stream: b.u8()?, key_id: b.u8()?, first: b.u48()?, count: b.u8()? });
                }
                Section::Nack(ranges)
            }
            section_type::KEY => {
                let mut keys = Vec::new();
                while !b.done() {
                    keys.push((b.u16()?, b.u8()?));
                }
                Section::Key(keys)
            }
            _ => continue,
        };
        if !b.done() {
            return None;
        }
        sections.push(section);
    }
    Some(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clear_header_parses_what_begin_wrote() {
        let aad = Header::aad(0xbeef, stream::CAMERA_HIGH, stream_flags::KEY_START | stream_flags::FRAME_END, 130);
        let mut d = Vec::new();
        Header::begin(&mut d, 7, &aad);
        d.extend_from_slice(&0x0000_1234_5678u64.to_be_bytes()[2..]); // as a sealer appends its seq
        d.extend_from_slice(&[0xaa; 40]); // ciphertext and tag
        let (h, sealed) = Header::parse(&d).expect("a media datagram");
        assert_eq!(h, Header { hop: 7, src: 0xbeef, stream: 2, flags: 0x30, key_id: 130, seq: 0x1234_5678 }, "header fields");
        assert_eq!(&d[AAD], &aad, "the associated data is where the layout says");
        assert_eq!(sealed.len(), d.len() - 7, "the sealed part starts at the sequence");

        Header::set_hop(&mut d, 9);
        assert_eq!(Header::parse(&d).unwrap().0.hop, 9, "the hop is rewritable");

        for n in 0..HEADER + TAG {
            assert!(Header::parse(&d[..n]).is_none(), "cut to {n} bytes");
        }
        let mut bad = d.clone();
        bad[0] = 0xe1;
        assert!(Header::parse(&bad).is_none(), "the stream's byte is not media");
        bad = d.clone();
        bad[5] |= stream_flags::RESERVED;
        assert!(Header::parse(&bad).is_none(), "the reserved bit must be zero");
        assert!(Header::parse(&vec![MEDIA_BYTE; MAX_DATAGRAM + 1]).is_none(), "over the datagram size");
    }

    #[test]
    fn bodies_round_trip_and_truncations_are_refused() {
        let mut b = Vec::new();
        let voice = Voice { flags: voice_flags::TALK_START, seq: 77, capture_us: 1 << 40, payload: b"adpcm" };
        voice.write(&mut b);
        assert_eq!(Voice::read(&b), Some(voice), "voice round trip");
        for n in 0..VOICE_HEADER {
            assert_eq!(Voice::read(&b[..n]), None, "voice cut to {n}");
        }

        let frag = Fragment { flags: video_flags::KEY, frame: 9, base: 0, index: 3, count: 4, capture_us: 123, payload: &[7; 100] };
        frag.write(&mut b);
        assert_eq!(Fragment::read(&b), Some(frag), "fragment round trip");
        for n in 0..=FRAGMENT_HEADER {
            assert_eq!(Fragment::read(&b[..n]), None, "fragment cut to {n} (an empty payload is no fragment)");
        }
        b[7..11].copy_from_slice(&[0, 4, 0, 4]); // index 4 of count 4
        assert_eq!(Fragment::read(&b), None, "an index at the count is refused");
        assert!(FRAGMENT_HEADER + MAX_FRAGMENT == MAX_PLAINTEXT, "a full fragment fills a datagram");
    }

    #[test]
    fn feedback_round_trips_fits_and_skips_unknown_sections() {
        let sections = vec![
            Section::Arrivals { base: 65530, report_time: 123_456, ages: vec![12, NOT_SEEN, 0, 400] },
            Section::Nack(vec![
                NackRange { src: 1, stream: stream::CAMERA_HIGH, key_id: 3, first: (1 << 47) + 5, count: 2 },
                NackRange { src: 2, stream: stream::SHARE, key_id: 200, first: 0, count: 255 },
            ]),
            Section::Key(vec![(1, stream::CAMERA_HIGH), (2, stream::SHARE)]),
        ];
        let mut out = Vec::new();
        write_feedback(&sections, &mut out);
        assert_eq!(read_feedback(&out), Some(sections), "feedback round trip");

        // the largest ARRIVALS, with room for some NACKs and KEYs, fits a datagram
        let big = vec![
            Section::Arrivals { base: 0, report_time: 0, ages: vec![1; MAX_ARRIVALS] },
            Section::Nack(vec![NackRange { src: 0, stream: 2, key_id: 0, first: 0, count: 1 }; 8]),
            Section::Key(vec![(0, 2); 8]),
        ];
        write_feedback(&big, &mut out);
        assert!(out.len() <= MAX_PLAINTEXT, "{} bytes of feedback in {MAX_PLAINTEXT}", out.len());

        let mut d = vec![0, 200, 0, 2, 1, 2];
        d.extend_from_slice(&[section_type::KEY, 0, 3, 0, 9, 2]);
        assert_eq!(read_feedback(&d), Some(vec![Section::Key(vec![(9, 2)])]), "an unknown section is skipped");
        assert_eq!(read_feedback(&[0, section_type::KEY, 0, 2, 0, 9]), None, "a KEY body of two bytes is malformed");
        assert_eq!(read_feedback(&[0, section_type::NACK, 0, 50, 1]), None, "a section running past the end is malformed");
        assert_eq!(read_feedback(&[0, section_type::ARRIVALS, 0, 8, 0, 0, 0, 0, 0, 0, 0, 5]), None, "five ages promised, none there");
    }
}
