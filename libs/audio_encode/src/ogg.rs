//! Ogg page writer: packets in, a framed logical stream out.
//!
//! The mirror of the decoder's `ogg.rs` reader. Pages close on the segment
//! table filling (255 lacing values), on a size threshold, or on an explicit
//! flush; a packet larger than the remaining table space spans pages with the
//! continuation flag, exactly the shape the reader reassembles.

/// Close a page once its body reaches this size (the table limit of 255
/// segments closes it sooner for small-packet streams).
const PAGE_BODY_TARGET: usize = 8 * 1024;

/// A page that carries no granule position.
const GRANULE_NONE: u64 = u64::MAX;

pub struct OggWriter {
    serial: u32,
    out: Vec<u8>,
    sequence: u32,
    /// Current page under construction.
    segs: Vec<u8>,
    body: Vec<u8>,
    /// The page opens continuing a packet begun on the previous page.
    continued: bool,
    /// Granule of the last packet completed on the current page, if any.
    granule: Option<u64>,
    /// No page has been emitted yet (the next page is BOS).
    first: bool,
    eos_pending: bool,
}

impl OggWriter {
    pub fn new(serial: u32) -> OggWriter {
        OggWriter {
            serial,
            out: Vec::new(),
            sequence: 0,
            segs: Vec::new(),
            body: Vec::new(),
            continued: false,
            granule: None,
            first: true,
            eos_pending: false,
        }
    }

    /// Append one packet. `granule` is the stream position after this packet
    /// (for audio packets; header packets pass 0). `eos` marks the last
    /// packet of the stream.
    pub fn packet(&mut self, data: &[u8], granule: u64, eos: bool) {
        let mut at = 0usize;
        loop {
            // Lacing values for this packet: len/255 full segments, then the
            // remainder (a multiple of 255 needs its 0 terminator).
            let remaining = data.len() - at;
            let take = remaining.min(255);
            self.segs.push(take as u8);
            self.body.extend_from_slice(&data[at..at + take]);
            at += take;
            let packet_done = take < 255 || at == data.len() && take == 255 && {
                // A packet whose length is an exact multiple of 255 ends with
                // a 0 lacing value; emit it (possibly on the next page).
                if self.segs.len() == 255 {
                    // The 0-lacing terminator lands on the next page, which
                    // therefore opens mid-packet.
                    self.flush_page(false);
                    self.continued = true;
                }
                self.segs.push(0);
                true
            };
            if packet_done {
                self.granule = Some(granule);
                if eos {
                    self.eos_pending = true;
                }
                if self.segs.len() >= 255 || self.body.len() >= PAGE_BODY_TARGET || eos {
                    self.flush_page(eos);
                }
                return;
            }
            if self.segs.len() == 255 {
                // Page full mid-packet: flush and continue on the next one.
                self.flush_page(false);
                self.continued = true;
            }
        }
    }

    /// Force the current page out (header pages must not share with audio).
    pub fn flush(&mut self) {
        if !self.segs.is_empty() {
            self.flush_page(self.eos_pending);
        }
    }

    /// The complete stream. Call after the `eos` packet.
    pub fn finish(mut self) -> Vec<u8> {
        self.flush();
        self.out
    }

    fn flush_page(&mut self, eos: bool) {
        let mut flags = 0u8;
        if self.continued {
            flags |= 0x01;
        }
        if self.first {
            flags |= 0x02;
        }
        if eos {
            flags |= 0x04;
        }
        let granule = self.granule.take().unwrap_or(GRANULE_NONE);
        let mut page = Vec::with_capacity(27 + self.segs.len() + self.body.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // crc, patched below
        page.push(self.segs.len() as u8);
        page.extend_from_slice(&self.segs);
        page.extend_from_slice(&self.body);
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.out.extend_from_slice(&page);
        self.sequence += 1;
        self.first = false;
        self.continued = false;
        self.segs.clear();
        self.body.clear();
    }
}

/// Ogg's CRC-32: polynomial 0x04c11db7, MSB-first, init 0, no final xor.
fn crc32(bytes: &[u8]) -> u32 {
    // Small enough to build per call in tests; cached for the encode path.
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
            }
            *slot = r;
        }
        t
    });
    let mut crc = 0u32;
    for &b in bytes {
        crc = (crc << 8) ^ table[(((crc >> 24) as u8) ^ b) as usize];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_decode::ogg::PacketReader;

    fn read_all(bytes: &[u8]) -> Vec<(Vec<u8>, Option<u64>, bool)> {
        let mut r = PacketReader::new(bytes);
        let mut out = Vec::new();
        while let Ok(Some(p)) = r.next_packet() {
            out.push((p.data.to_vec(), p.granule, p.end_of_stream));
        }
        out
    }

    #[test]
    fn small_packets_round_trip_with_granules() {
        let mut w = OggWriter::new(0x1234);
        w.packet(&[1, 2, 3], 0, false);
        w.flush();
        w.packet(&[4, 5], 512, false);
        w.packet(&[6], 1024, true);
        let stream = w.finish();
        let got = read_all(&stream);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, vec![1, 2, 3]);
        assert_eq!(got[1].0, vec![4, 5]);
        assert_eq!(got[2], (vec![6], Some(1024), true));
        // First page is BOS and carries its own granule.
        assert_eq!(&stream[0..4], b"OggS");
        assert_eq!(stream[5] & 0x02, 0x02);
    }

    #[test]
    fn a_large_packet_spans_pages() {
        let big: Vec<u8> = (0..100_000usize).map(|i| (i % 251) as u8).collect();
        let mut w = OggWriter::new(7);
        w.packet(&big, 4096, true);
        let stream = w.finish();
        let got = read_all(&stream);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, big);
        assert_eq!(got[0].1, Some(4096));
        assert!(got[0].2);
        // More than one page was needed.
        let pages = stream.windows(4).filter(|w| w == b"OggS").count();
        assert!(pages >= 2, "{pages} pages");
    }

    #[test]
    fn an_exact_multiple_of_255_gets_its_terminator() {
        let data = vec![9u8; 510];
        let mut w = OggWriter::new(1);
        w.packet(&data, 1, true);
        let got = read_all(&w.finish());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.len(), 510);
    }

    #[test]
    fn many_packets_split_across_pages_keep_positions() {
        let mut w = OggWriter::new(3);
        let mut want = Vec::new();
        for i in 0..2000u64 {
            let data = vec![(i % 256) as u8; (i as usize * 7) % 40 + 1];
            want.push(data.clone());
            w.packet(&data, i * 512, i == 1999);
        }
        let stream = w.finish();
        let got = read_all(&stream);
        assert_eq!(got.len(), 2000);
        for (i, (data, _, _)) in got.iter().enumerate() {
            assert_eq!(data, &want[i], "packet {i}");
        }
        // The final packet is flagged EOS and carries its granule.
        assert_eq!(got[1999].1, Some(1999 * 512));
        assert!(got[1999].2);
        // Sequence numbers are dense from zero (reader drops on gaps only via
        // CRC; check by hand).
        let mut seqs = Vec::new();
        let mut at = 0usize;
        while let Some(pos) = stream[at..].windows(4).position(|w| w == b"OggS") {
            let p = at + pos;
            seqs.push(u32::from_le_bytes(stream[p + 18..p + 22].try_into().unwrap()));
            at = p + 27;
        }
        for (i, &s) in seqs.iter().enumerate() {
            assert_eq!(s as usize, i);
        }
    }
}
