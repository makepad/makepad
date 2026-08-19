//! Ogg container framing: pages in, one packet at a time out.
//!
//! Adapted from the same author's decoder in the sandbox repo
//! (`apps/sandbox/libs/audio/src/ogg.rs`), re-homed here so the non-sandbox
//! apps can use it, and rewritten from "collect every packet into a
//! `Vec<Vec<u8>>`" into a streaming reader: a DJ deck starts a 100 MB track
//! without first materialising a hundred thousand packet allocations.
//!
//! Only what Vorbis needs — one logical stream, packets rebuilt from the
//! segment table. Every field on the page header is attacker-controlled, so
//! this walks forward by checked arithmetic and gives up rather than trusting
//! a length.
//!
//! Page checksums *are* verified. A page whose CRC does not match is dropped
//! whole (and with it any packet it was continuing) and the reader resyncs on
//! the next `OggS`, which is what it does for a page of garbage too — a damaged
//! file then truncates cleanly instead of decoding noise. The CRC is also what
//! makes the backwards scan in [`last_page`] safe: an `OggS` that happens to
//! occur inside packet data essentially never checksums.

use crate::error::AudioError;
use std::sync::OnceLock;

/// Page capture pattern.
pub const CAPTURE: &[u8; 4] = b"OggS";
/// Fixed part of a page header, before the segment table.
const HEADER_LEN: usize = 27;

/// Cap on one reassembled packet. Vorbis setup headers are a few KiB and audio
/// packets are far smaller; this stops a crafted chain of continuations from
/// growing a packet without bound.
pub const MAX_PACKET: usize = 1 << 21;

/// A page that carries no granule position ("-1" in the spec) uses this.
pub const GRANULE_NONE: u64 = u64::MAX;

const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

/// Ogg's CRC: 32 bits, polynomial 0x04c11db7, MSB-first, no reflection, init 0,
/// no final xor. Table built once — a bitwise loop over a 100 MB file is not
/// free.
fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
            }
            *slot = r;
        }
        table
    })
}

fn crc_update(mut crc: u32, bytes: &[u8]) -> u32 {
    let table = crc_table();
    for &b in bytes {
        crc = (crc << 8) ^ table[(((crc >> 24) as u8) ^ b) as usize];
    }
    crc
}

/// One parsed page header plus the byte ranges it points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// Offset of the `OggS` capture pattern.
    pub start: usize,
    /// Offset of the segment (lacing) table.
    pub seg_table: usize,
    pub n_segments: usize,
    /// Offset of the page body, one past the segment table.
    pub body: usize,
    /// One past the last body byte, i.e. one past the whole page.
    pub end: usize,
    /// Sample position, or [`GRANULE_NONE`] when the page carries none.
    pub granule: u64,
    pub serial: u32,
    pub sequence: u32,
    pub flags: u8,
}

impl Page {
    /// This page opens with the tail of a packet begun on an earlier page.
    pub fn continued(&self) -> bool {
        self.flags & FLAG_CONTINUED != 0
    }
    pub fn begin_of_stream(&self) -> bool {
        self.flags & FLAG_BOS != 0
    }
    pub fn end_of_stream(&self) -> bool {
        self.flags & FLAG_EOS != 0
    }
}

/// Parse the page whose capture pattern sits at `at`, verifying the checksum.
/// `None` means "there is no valid page here" and the caller resyncs.
pub fn parse_page(bytes: &[u8], at: usize) -> Option<Page> {
    let header = bytes.get(at..at.checked_add(HEADER_LEN)?)?;
    if &header[0..4] != CAPTURE || header[4] != 0 {
        return None;
    }
    let flags = header[5];
    let granule = u64::from_le_bytes(header[6..14].try_into().ok()?);
    let serial = u32::from_le_bytes(header[14..18].try_into().ok()?);
    let sequence = u32::from_le_bytes(header[18..22].try_into().ok()?);
    let stored_crc = u32::from_le_bytes(header[22..26].try_into().ok()?);
    let n_segments = header[26] as usize;
    let seg_table = at + HEADER_LEN;
    let body = seg_table.checked_add(n_segments)?;
    let lacing = bytes.get(seg_table..body)?;
    let body_len: usize = lacing.iter().map(|&s| s as usize).sum();
    let end = body.checked_add(body_len)?;
    if end > bytes.len() {
        return None;
    }
    // The checksum covers the whole page with the CRC field itself zeroed.
    let mut crc = crc_update(0, &header[0..22]);
    crc = crc_update(crc, &[0, 0, 0, 0]);
    crc = crc_update(crc, &bytes[at + 26..end]);
    if crc != stored_crc {
        return None;
    }
    Some(Page { start: at, seg_table, n_segments, body, end, granule, serial, sequence, flags })
}

/// Next `OggS` at or after `from`.
fn find_capture(bytes: &[u8], from: usize) -> Option<usize> {
    if from >= bytes.len() || bytes.len() - from < 4 {
        return None;
    }
    bytes[from..].windows(4).position(|w| w == CAPTURE).map(|p| from + p)
}

/// First valid page of the stream, whatever leading junk precedes it.
pub fn first_page(bytes: &[u8]) -> Option<Page> {
    let mut pos = 0usize;
    while let Some(at) = find_capture(bytes, pos) {
        if let Some(page) = parse_page(bytes, at) {
            return Some(page);
        }
        pos = at + 1;
    }
    None
}

/// Last valid page of `serial` (or of any stream when `serial` is `None`),
/// found by scanning backwards from the end in growing windows.
///
/// This is how a duration probe avoids decoding: an Ogg page is at most 64 KiB,
/// so the first window nearly always holds the end of the stream and the work
/// is O(tail) rather than O(file).
pub fn last_page(bytes: &[u8], serial: Option<u32>) -> Option<Page> {
    let mut window = 128 * 1024usize;
    loop {
        let start = bytes.len().saturating_sub(window);
        let mut pos = start;
        let mut best: Option<Page> = None;
        while let Some(at) = find_capture(bytes, pos) {
            match parse_page(bytes, at) {
                Some(page) => {
                    if serial.is_none_or(|s| s == page.serial) {
                        best = Some(page);
                    }
                    // A validated page cannot overlap another, so skip it whole.
                    pos = page.end.max(at + 1);
                }
                None => pos = at + 1,
            }
        }
        if best.is_some() {
            return best;
        }
        if start == 0 {
            return None;
        }
        window = window.saturating_mul(8);
    }
}

/// One reassembled packet.
pub struct Packet<'a> {
    pub data: &'a [u8],
    /// Granule position of the page this packet finished on, when this packet
    /// is the last one that page completes and the page carries a position.
    ///
    /// Vorbis signals encoder delay and end-of-stream truncation through it:
    /// the samples decoded by that point exceed what the page claims, and the
    /// excess is discarded (from the front on the first page, from the back on
    /// the last). Without per-page granules there is no way to recover either.
    pub granule: Option<u64>,
    /// This packet finished on the stream's final page.
    pub end_of_stream: bool,
}

/// Streaming packet reader over one logical stream.
///
/// The reader locks onto the first serial number it sees and ignores every
/// other one, so a chained or multiplexed file plays its first stream rather
/// than splicing two decoders' bitstreams together.
#[derive(Clone)]
pub struct PacketReader<'a> {
    bytes: &'a [u8],
    /// Next byte to look at when the current page runs out.
    pos: usize,
    serial: Option<u32>,
    /// Assembly buffer for packets spanning pages. Reused across packets.
    buf: Vec<u8>,
    page: Option<PageCursor>,
    /// The previous page ended mid-packet (its last lacing value was 255).
    carry: bool,
    /// Drop bytes until the next packet boundary: either we joined a packet
    /// that began on a page we never saw, or one grew past [`MAX_PACKET`].
    resync_packet: bool,
    finished: bool,
}

#[derive(Clone, Copy)]
struct PageCursor {
    page: Page,
    /// Next lacing value to consume.
    seg: usize,
    /// Next body byte to consume.
    at: usize,
}

impl<'a> PacketReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            serial: None,
            buf: Vec::new(),
            page: None,
            carry: false,
            resync_packet: false,
            finished: false,
        }
    }

    /// Serial number of the stream this reader locked onto, once it has seen a
    /// page.
    pub fn serial(&self) -> Option<u32> {
        self.serial
    }

    /// Next complete packet, or `None` at the end of the stream.
    ///
    /// The packet borrows either the input or an internal buffer, so it must be
    /// dropped before the next call. Nothing is allocated once that buffer has
    /// grown to the largest packet seen.
    pub fn next_packet(&mut self) -> Result<Option<Packet<'_>>, AudioError> {
        if self.finished {
            return Ok(None);
        }
        let bytes = self.bytes;
        self.buf.clear();
        // Where the finished packet lives: a range of the input when it is one
        // contiguous run inside a single page (the common case), else the
        // assembly buffer.
        let mut direct: Option<(usize, usize)> = None;
        let mut granule: Option<u64> = None;
        let mut eos = false;

        'packet: loop {
            if self.page.is_none() && !self.load_page() {
                self.finished = true;
                // A packet still under assembly at the end of the input is
                // truncated: hand back nothing rather than half a packet.
                return Ok(None);
            }
            let Some(mut cursor) = self.page else {
                self.finished = true;
                return Ok(None);
            };
            let lacing = bytes
                .get(cursor.page.seg_table..cursor.page.seg_table + cursor.page.n_segments)
                .unwrap_or(&[]);
            // Start of the current packet's run of bytes within this page.
            let mut run_start = cursor.at;

            while cursor.seg < lacing.len() {
                let len = lacing[cursor.seg] as usize;
                cursor.seg += 1;
                cursor.at += len;
                if cursor.at > bytes.len() {
                    // `parse_page` proved the body fits, so this is unreachable;
                    // keeping it makes the slicing below locally obvious.
                    self.page = None;
                    self.finished = true;
                    return Err(AudioError::Truncated);
                }
                if !self.resync_packet && self.buf.len() + (cursor.at - run_start) > MAX_PACKET {
                    // Refuse the packet, not the file.
                    self.buf.clear();
                    self.resync_packet = true;
                }
                if len == 255 {
                    // Packet continues past this segment.
                    continue;
                }
                if self.resync_packet {
                    // Boundary reached: the junk ends here, start fresh.
                    self.resync_packet = false;
                    self.buf.clear();
                    run_start = cursor.at;
                    continue;
                }
                if self.buf.is_empty() {
                    direct = Some((run_start, cursor.at));
                } else {
                    self.buf.extend_from_slice(&bytes[run_start..cursor.at]);
                }
                // A page's granule belongs to the last packet it completes.
                if !lacing[cursor.seg..].iter().any(|&l| l < 255) {
                    if cursor.page.granule != GRANULE_NONE {
                        granule = Some(cursor.page.granule);
                    }
                    eos = cursor.page.end_of_stream();
                }
                self.carry = false;
                self.page = if cursor.seg < lacing.len() { Some(cursor) } else { None };
                break 'packet;
            }

            // Page exhausted. Anything after `run_start` belongs to a packet
            // that continues on the next page.
            if !self.resync_packet && cursor.at > run_start {
                self.buf.extend_from_slice(&bytes[run_start..cursor.at]);
                self.carry = true;
            }
            self.page = None;
        }

        let data = match direct {
            Some((a, b)) => &bytes[a..b],
            None => &self.buf[..],
        };
        Ok(Some(Packet { data, granule, end_of_stream: eos }))
    }

    /// Advance to the next page of our stream. `false` at end of input.
    fn load_page(&mut self) -> bool {
        loop {
            let Some(at) = find_capture(self.bytes, self.pos) else {
                return false;
            };
            let Some(page) = parse_page(self.bytes, at) else {
                // Garbage, a lying segment table, or a failed checksum: none of
                // it is trustworthy, so drop whatever packet was in flight.
                self.pos = at + 1;
                if self.carry || !self.buf.is_empty() {
                    self.buf.clear();
                    self.carry = false;
                    self.resync_packet = true;
                }
                continue;
            };
            self.pos = page.end.max(at + 1);
            let ours = *self.serial.get_or_insert(page.serial);
            if page.serial != ours {
                continue;
            }
            if page.continued() {
                if !self.carry && self.buf.is_empty() {
                    // The head of this packet was on a page we never saw.
                    self.resync_packet = true;
                }
            } else {
                // A fresh page starts a packet at its first segment, abandoning
                // any partial one and ending any resync.
                self.buf.clear();
                self.resync_packet = false;
            }
            self.carry = false;
            self.page = Some(PageCursor { page, seg: 0, at: page.body });
            return true;
        }
    }
}

#[cfg(test)]
pub(crate) mod test_pages {
    use super::*;

    /// Build one Ogg page carrying `body`, split into `segments` lacing values,
    /// with a correct checksum.
    pub fn page(
        serial: u32,
        sequence: u32,
        granule: u64,
        flags: u8,
        segments: &[u8],
        body: &[u8],
    ) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(CAPTURE);
        v.push(0);
        v.push(flags);
        v.extend_from_slice(&granule.to_le_bytes());
        v.extend_from_slice(&serial.to_le_bytes());
        v.extend_from_slice(&sequence.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // crc placeholder
        v.push(segments.len() as u8);
        v.extend_from_slice(segments);
        v.extend_from_slice(body);
        let crc = crc_update(0, &v);
        v[22..26].copy_from_slice(&crc.to_le_bytes());
        v
    }

    /// Lacing values for a packet of `len` bytes.
    pub fn lacing(len: usize) -> Vec<u8> {
        let mut out = vec![255u8; len / 255];
        out.push((len % 255) as u8);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_pages::page;
    use super::*;

    fn packets(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut r = PacketReader::new(bytes);
        let mut out = Vec::new();
        while let Ok(Some(p)) = r.next_packet() {
            out.push(p.data.to_vec());
        }
        out
    }

    #[test]
    fn single_page_single_packet() {
        let body = vec![1u8, 2, 3, 4];
        let p = page(7, 0, 100, FLAG_BOS, &[4], &body);
        let mut r = PacketReader::new(&p);
        let (data, granule) = {
            let got = r.next_packet().unwrap().unwrap();
            (got.data.to_vec(), got.granule)
        };
        assert_eq!(data, body);
        assert_eq!(granule, Some(100));
        assert_eq!(r.serial(), Some(7));
        assert!(r.next_packet().unwrap().is_none());
    }

    #[test]
    fn packet_spanning_segments_is_reassembled() {
        // 255 + 3 is one packet of 258 bytes.
        let body: Vec<u8> = (0..258).map(|i| (i % 251) as u8).collect();
        let p = page(1, 0, 0, FLAG_BOS, &[255, 3], &body);
        let got = packets(&p);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], body);
    }

    #[test]
    fn packet_continued_across_pages() {
        let first: Vec<u8> = vec![0xAA; 255];
        let second: Vec<u8> = vec![0xBB; 10];
        let mut data = page(1, 0, GRANULE_NONE, FLAG_BOS, &[255], &first);
        data.extend(page(1, 1, 50, FLAG_CONTINUED, &[10], &second));
        let got = packets(&data);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].len(), 265);
        assert_eq!(got[0][0], 0xAA);
        assert_eq!(got[0][264], 0xBB);
    }

    #[test]
    fn packet_continued_across_three_pages() {
        let mut data = page(1, 0, GRANULE_NONE, FLAG_BOS, &[255, 255], &vec![1u8; 510]);
        data.extend(page(1, 1, GRANULE_NONE, FLAG_CONTINUED, &[255], &vec![2u8; 255]));
        data.extend(page(1, 2, 9, FLAG_CONTINUED | FLAG_EOS, &[4], &[3u8; 4]));
        let got = packets(&data);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].len(), 769);
        assert_eq!(got[0][509], 1);
        assert_eq!(got[0][510], 2);
        assert_eq!(got[0][765], 3);
    }

    #[test]
    fn granule_belongs_to_the_last_packet_a_page_completes() {
        let p = page(1, 0, 4096, FLAG_BOS, &[2, 3], &[1, 2, 3, 4, 5]);
        let mut r = PacketReader::new(&p);
        {
            let a = r.next_packet().unwrap().unwrap();
            assert_eq!(a.data, &[1, 2][..]);
            assert_eq!(a.granule, None);
        }
        let b = r.next_packet().unwrap().unwrap();
        assert_eq!(b.data, &[3, 4, 5][..]);
        assert_eq!(b.granule, Some(4096));
    }

    #[test]
    fn zero_length_packets_are_packets() {
        let p = page(1, 0, 0, FLAG_BOS, &[0, 0, 1], &[9]);
        let got = packets(&p);
        assert_eq!(got.len(), 3);
        assert!(got[0].is_empty() && got[1].is_empty());
        assert_eq!(got[2], vec![9]);
    }

    #[test]
    fn a_255_multiple_packet_needs_its_terminator() {
        // A packet of exactly 255 bytes laces as [255, 0].
        let body = vec![7u8; 255];
        let p = page(1, 0, 0, FLAG_BOS, &[255, 0], &body);
        let got = packets(&p);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].len(), 255);
    }

    #[test]
    fn garbage_yields_no_packets_and_no_panic() {
        assert!(packets(&[]).is_empty());
        assert!(packets(b"nothing to see here").is_empty());
        assert!(packets(CAPTURE).is_empty());
        assert!(packets(&[b'O', b'g', b'g', b'S', 0, 2, 9]).is_empty());
    }

    #[test]
    fn leading_junk_is_skipped_by_resync() {
        let mut data = vec![0x11u8; 500];
        data.extend(page(3, 0, 1, FLAG_BOS, &[2], &[8, 9]));
        assert_eq!(packets(&data), vec![vec![8u8, 9]]);
    }

    #[test]
    fn lying_segment_table_is_refused_not_indexed() {
        let mut v = page(1, 0, 0, FLAG_BOS, &[1], &[1]);
        v[26] = 200; // claim 200 segments that are not there
        assert!(packets(&v).is_empty());
    }

    #[test]
    fn second_stream_is_ignored() {
        let mut data = page(1, 0, 10, FLAG_BOS, &[2], &[1, 2]);
        data.extend(page(999, 0, 20, FLAG_BOS, &[2], &[3, 4]));
        assert_eq!(packets(&data), vec![vec![1u8, 2]]);
    }

    #[test]
    fn a_flipped_byte_drops_its_page_not_the_file() {
        let mut data = page(1, 0, 10, FLAG_BOS, &[2], &[1, 2]);
        let bad_at = data.len();
        data.extend(page(1, 1, 20, 0, &[2], &[3, 4]));
        data.extend(page(1, 2, 30, FLAG_EOS, &[2], &[5, 6]));
        data[bad_at + HEADER_LEN + 1] ^= 0xff;
        assert_eq!(packets(&data), vec![vec![1u8, 2], vec![5u8, 6]]);
    }

    #[test]
    fn continuation_without_a_beginning_is_dropped() {
        // The reader joins mid-packet: the orphan tail must not be handed out.
        let mut data = page(1, 5, 10, FLAG_CONTINUED, &[3], &[1, 2, 3]);
        data.extend(page(1, 6, 20, 0, &[2], &[4, 5]));
        assert_eq!(packets(&data), vec![vec![4u8, 5]]);
    }

    #[test]
    fn orphan_tail_and_a_packet_on_the_same_page() {
        let mut data = page(1, 5, 10, FLAG_CONTINUED, &[3, 2], &[1, 2, 3, 4, 5]);
        data.extend(page(1, 6, 20, 0, &[1], &[6]));
        assert_eq!(packets(&data), vec![vec![4u8, 5], vec![6u8]]);
    }

    #[test]
    fn fresh_page_abandons_a_partial_packet() {
        let mut data = page(1, 0, GRANULE_NONE, FLAG_BOS, &[255], &vec![0xAA; 255]);
        // The next page is not flagged as a continuation, so the partial is lost.
        data.extend(page(1, 1, 20, 0, &[2], &[4, 5]));
        assert_eq!(packets(&data), vec![vec![4u8, 5]]);
    }

    #[test]
    fn end_of_stream_flag_is_reported() {
        let mut data = page(1, 0, 10, FLAG_BOS, &[1], &[1]);
        data.extend(page(1, 1, 20, FLAG_EOS, &[1], &[2]));
        let mut r = PacketReader::new(&data);
        assert!(!r.next_packet().unwrap().unwrap().end_of_stream);
        assert!(r.next_packet().unwrap().unwrap().end_of_stream);
    }

    #[test]
    fn last_page_scans_backwards() {
        let mut data = vec![0u8; 300_000];
        data.extend(page(1, 0, 10, FLAG_BOS, &[1], &[1]));
        data.extend(page(1, 1, 44_100, FLAG_EOS, &[1], &[2]));
        let p = last_page(&data, Some(1)).unwrap();
        assert_eq!(p.granule, 44_100);
        assert!(p.end_of_stream());
        assert_eq!(first_page(&data).unwrap().granule, 10);
    }

    #[test]
    fn last_page_grows_its_window_past_a_long_tail() {
        let mut data = page(1, 0, 4242, FLAG_EOS, &[1], &[1]);
        data.extend(vec![0u8; 400_000]);
        assert_eq!(last_page(&data, None).unwrap().granule, 4242);
    }

    #[test]
    fn crc_rejects_a_capture_pattern_inside_a_body() {
        let body = b"OggS\0\x02rubbish".to_vec();
        let data = page(1, 0, 7, FLAG_BOS | FLAG_EOS, &[body.len() as u8], &body);
        assert_eq!(last_page(&data, None).unwrap().granule, 7);
        assert_eq!(packets(&data), vec![body]);
    }

    #[test]
    fn oversized_packet_is_refused_and_the_stream_resyncs() {
        // Chain enough full pages to blow MAX_PACKET, then send a good packet.
        let mut data = Vec::new();
        let seg = [255u8; 255];
        let body = vec![0u8; 255 * 255];
        let pages = MAX_PACKET / body.len() + 2;
        for i in 0..pages {
            let flags = if i == 0 { FLAG_BOS } else { FLAG_CONTINUED };
            data.extend(page(1, i as u32, GRANULE_NONE, flags, &seg, &body));
        }
        data.extend(page(1, pages as u32, 99, 0, &[2], &[1, 2]));
        assert_eq!(packets(&data), vec![vec![1u8, 2]]);
    }

    #[test]
    fn truncated_page_body_yields_nothing() {
        let mut data = page(1, 0, 10, FLAG_BOS, &[200], &[3u8; 200]);
        data.truncate(data.len() - 40);
        assert!(packets(&data).is_empty());
    }
}
