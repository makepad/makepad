//! Ogg container: page parsing and packet reassembly.
//!
//! Only what Vorbis needs — a single logical stream, packets rebuilt from the
//! segment table. Every field on the page header is attacker-controlled, so
//! this walks forward by checked arithmetic and gives up rather than trusting
//! a length.

use crate::AudioError;

const CAPTURE: &[u8; 4] = b"OggS";
const HEADER_LEN: usize = 27;

/// One reassembled packet plus the page flags that produced it.
pub struct OggPackets {
    pub packets: Vec<Vec<u8>>,
    /// Granule position of the last page — Vorbis' total sample count.
    pub last_granule: u64,
    /// `(packets completed through this page, granule position)` per page.
    /// Vorbis signals encoder delay through the granule of the first audio
    /// page: the samples decoded up to that point exceed the samples the page
    /// claims, and the excess is discarded from the FRONT. Without per-page
    /// granules there is no way to recover that leading trim.
    pub page_ends: Vec<(usize, u64)>,
    pub serial: u32,
}

/// Cap on a single reassembled packet. Vorbis setup headers are a few KiB;
/// audio packets are far smaller. This stops a crafted chain of continuations
/// from growing a packet without bound.
const MAX_PACKET: usize = 8 * 1024 * 1024;
/// Cap on packet count so a huge file cannot exhaust memory in one go.
const MAX_PACKETS: usize = 2_000_000;

/// Split an Ogg bitstream into packets for its first logical stream.
pub fn read_packets(bytes: &[u8]) -> Result<OggPackets, AudioError> {
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut serial: Option<u32> = None;
    let mut last_granule = 0u64;
    let mut page_ends: Vec<(usize, u64)> = Vec::new();

    while pos + HEADER_LEN <= bytes.len() {
        if &bytes[pos..pos + 4] != CAPTURE {
            // Resync: scan for the next capture pattern instead of failing.
            // A truncated or spliced file is common in the wild.
            match find_capture(bytes, pos + 1) {
                Some(next) => {
                    pos = next;
                    continue;
                }
                None => break,
            }
        }
        let version = bytes[pos + 4];
        if version != 0 {
            return Err(AudioError::Malformed);
        }
        let header_type = bytes[pos + 5];
        let granule = u64::from_le_bytes(
            bytes[pos + 6..pos + 14]
                .try_into()
                .map_err(|_| AudioError::Truncated)?,
        );
        let page_serial = u32::from_le_bytes(
            bytes[pos + 14..pos + 18]
                .try_into()
                .map_err(|_| AudioError::Truncated)?,
        );
        let n_segments = bytes[pos + 26] as usize;
        let seg_table = pos + HEADER_LEN;
        let body = seg_table + n_segments;
        if body > bytes.len() {
            break;
        }
        let body_len: usize = bytes[seg_table..body].iter().map(|&s| s as usize).sum();
        let body_end = body.checked_add(body_len).ok_or(AudioError::Malformed)?;
        if body_end > bytes.len() {
            break;
        }

        // Lock onto the first stream we see and ignore any others (chained or
        // multiplexed files); Vorbis SFX are single-stream in practice.
        let this_serial = *serial.get_or_insert(page_serial);
        if page_serial != this_serial {
            pos = body_end;
            continue;
        }

        // A fresh page (not a continuation) invalidates any partial packet.
        if header_type & 0x01 == 0 && !partial.is_empty() {
            partial.clear();
        }

        let mut seg_pos = body;
        for i in 0..n_segments {
            let len = bytes[seg_table + i] as usize;
            let end = seg_pos + len;
            if end > bytes.len() {
                return Err(AudioError::Truncated);
            }
            if partial.len() + len > MAX_PACKET {
                return Err(AudioError::Malformed);
            }
            partial.extend_from_slice(&bytes[seg_pos..end]);
            seg_pos = end;
            // A segment shorter than 255 terminates the packet.
            if len < 255 {
                if packets.len() >= MAX_PACKETS {
                    return Err(AudioError::Malformed);
                }
                packets.push(std::mem::take(&mut partial));
            }
        }

        if granule != u64::MAX {
            last_granule = granule;
        }
        page_ends.push((packets.len(), granule));
        if body_end <= pos {
            return Err(AudioError::Malformed);
        }
        pos = body_end;
    }

    if packets.is_empty() {
        return Err(AudioError::NotOgg);
    }
    Ok(OggPackets {
        packets,
        last_granule,
        page_ends,
        serial: serial.unwrap_or(0),
    })
}

fn find_capture(bytes: &[u8], from: usize) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(4)
        .position(|w| w == CAPTURE)
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one Ogg page carrying `body`, split into `segments` lengths.
    fn page(serial: u32, granule: u64, header_type: u8, segments: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(CAPTURE);
        v.push(0); // version
        v.push(header_type);
        v.extend_from_slice(&granule.to_le_bytes());
        v.extend_from_slice(&serial.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // page seq
        v.extend_from_slice(&0u32.to_le_bytes()); // crc (not verified)
        v.push(segments.len() as u8);
        v.extend_from_slice(segments);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn single_page_single_packet() {
        let body = vec![1u8, 2, 3, 4];
        let p = page(7, 100, 0x02, &[4], &body);
        let out = read_packets(&p).unwrap();
        assert_eq!(out.packets.len(), 1);
        assert_eq!(out.packets[0], body);
        assert_eq!(out.serial, 7);
        assert_eq!(out.last_granule, 100);
    }

    #[test]
    fn packet_spanning_segments_is_reassembled() {
        // 255 + 3 means one packet of 258 bytes.
        let body: Vec<u8> = (0..258).map(|i| (i % 251) as u8).collect();
        let p = page(1, 0, 0x02, &[255, 3], &body);
        let out = read_packets(&p).unwrap();
        assert_eq!(out.packets.len(), 1);
        assert_eq!(out.packets[0].len(), 258);
        assert_eq!(out.packets[0], body);
    }

    #[test]
    fn packet_continued_across_pages() {
        let first: Vec<u8> = vec![0xAA; 255];
        let second: Vec<u8> = vec![0xBB; 10];
        let mut data = page(1, 0, 0x02, &[255], &first);
        data.extend(page(1, 50, 0x01, &[10], &second)); // continuation flag
        let out = read_packets(&data).unwrap();
        assert_eq!(out.packets.len(), 1);
        assert_eq!(out.packets[0].len(), 265);
        assert_eq!(out.packets[0][0], 0xAA);
        assert_eq!(out.packets[0][264], 0xBB);
    }

    #[test]
    fn garbage_yields_error_not_panic() {
        assert!(read_packets(&[]).is_err());
        assert!(read_packets(b"nothing to see here").is_err());
        // Truncated mid-header.
        assert!(read_packets(&CAPTURE[..]).is_err());
    }

    #[test]
    fn lying_segment_table_is_refused_not_indexed() {
        // Segment table claims 200 bytes but the body has none.
        let mut v = Vec::new();
        v.extend_from_slice(CAPTURE);
        v.push(0);
        v.push(0x02);
        v.extend_from_slice(&0u64.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.push(1);
        v.push(200);
        // no body at all
        assert!(read_packets(&v).is_err());
    }

    #[test]
    fn second_stream_is_ignored() {
        let a = page(1, 10, 0x02, &[2], &[1, 2]);
        let b = page(999, 20, 0x02, &[2], &[3, 4]);
        let mut data = a;
        data.extend(b);
        let out = read_packets(&data).unwrap();
        assert_eq!(out.packets.len(), 1);
        assert_eq!(out.packets[0], vec![1, 2]);
    }
}
