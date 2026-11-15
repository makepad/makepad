//! The RTCP a receiver sends: receiver reports (RFC 3550 6.4.2), generic
//! NACK and PLI (RFC 4585), and REMB (the receiver's bandwidth estimate,
//! draft-alvestrand-rmcat-remb). Plus the one thing it reads: the sender
//! report's NTP time, which the next receiver report echoes (LSR).

/// One reception report block about the stream from `source`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub source: u32,
    /// Lost since the last report, out of 256.
    pub fraction_lost: u8,
    /// Lost in all (24 bits, saturating).
    pub cumulative_lost: u32,
    pub highest_seq: u32,
    pub jitter: u32,
    /// The middle 32 bits of the last sender report's NTP time.
    pub last_sr: u32,
    /// Since that sender report, in 1/65536 s.
    pub delay_since_sr: u32,
}

fn header(count: u8, packet_type: u8, body: &[u8]) -> Vec<u8> {
    let words = (body.len() + 4) / 4 - 1;
    let mut out = vec![0x80 | count, packet_type];
    out.extend_from_slice(&(words as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A receiver report from `ssrc` (ours) with one block.
pub fn receiver_report(ssrc: u32, report: &Report) -> Vec<u8> {
    let mut body = ssrc.to_be_bytes().to_vec();
    body.extend_from_slice(&report.source.to_be_bytes());
    let lost = report.cumulative_lost.min(0x7f_ffff);
    body.push(report.fraction_lost);
    body.extend_from_slice(&lost.to_be_bytes()[1..]);
    for word in [report.highest_seq, report.jitter, report.last_sr, report.delay_since_sr] {
        body.extend_from_slice(&word.to_be_bytes());
    }
    header(1, 201, &body)
}

/// A generic NACK from `ssrc` for `lost` sequence numbers of `media`, in
/// the order they were lost (across a wrap too): one (PID, BLP) pair per
/// run within 17.
pub fn nack(ssrc: u32, media: u32, lost: &[u16]) -> Vec<u8> {
    let mut lost = lost.to_vec();
    lost.dedup();
    let mut body = ssrc.to_be_bytes().to_vec();
    body.extend_from_slice(&media.to_be_bytes());
    let mut i = 0;
    while i < lost.len() {
        let pid = lost[i];
        let mut blp = 0u16;
        i += 1;
        while i < lost.len() {
            let d = lost[i].wrapping_sub(pid);
            if d == 0 || d > 16 {
                break;
            }
            blp |= 1 << (d - 1);
            i += 1;
        }
        body.extend_from_slice(&pid.to_be_bytes());
        body.extend_from_slice(&blp.to_be_bytes());
    }
    header(1, 205, &body)
}

/// A picture loss indication: `media` is asked for a keyframe.
pub fn pli(ssrc: u32, media: u32) -> Vec<u8> {
    let mut body = ssrc.to_be_bytes().to_vec();
    body.extend_from_slice(&media.to_be_bytes());
    header(1, 206, &body)
}

/// REMB: what the receiver can take, in bits per second, for `media`.
pub fn remb(ssrc: u32, bitrate: u64, media: &[u32]) -> Vec<u8> {
    let mut exp = 0u32;
    let mut mantissa = bitrate;
    while mantissa > 0x3_ffff {
        mantissa >>= 1;
        exp += 1;
    }
    let mut body = ssrc.to_be_bytes().to_vec();
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(b"REMB");
    body.push(media.len() as u8);
    body.push(((exp as u8) << 2) | (mantissa >> 16) as u8);
    body.extend_from_slice(&(mantissa as u16).to_be_bytes());
    for m in media {
        body.extend_from_slice(&m.to_be_bytes());
    }
    header(15, 206, &body)
}

/// The sender reports in a (compound) RTCP packet: (sender SSRC, the
/// middle 32 bits of its NTP time).
pub fn sender_reports(packet: &[u8]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= packet.len() {
        let len = (u16::from_be_bytes([packet[at + 2], packet[at + 3]]) as usize + 1) * 4;
        let Some(one) = packet.get(at..at + len) else { break };
        if one[0] >> 6 != 2 {
            break;
        }
        if one[1] == 200 && one.len() >= 16 {
            let ssrc = u32::from_be_bytes([one[4], one[5], one[6], one[7]]);
            let ntp_middle = u32::from_be_bytes([one[10], one[11], one[12], one[13]]);
            out.push((ssrc, ntp_middle));
        }
        at += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_is_laid_out_as_the_rfcs_say() {
        let rr = receiver_report(1, &Report { source: 2, fraction_lost: 3, cumulative_lost: 5, highest_seq: 0x1_0002, ..Report::default() });
        assert_eq!(rr.len(), 32);
        assert_eq!(&rr[..4], &[0x81, 201, 0, 7], "RR with one block: 7 words after the first");
        assert_eq!(&rr[12..16], &[3, 0, 0, 5], "fraction then 24-bit cumulative loss");

        let n = nack(1, 2, &[100, 101, 103, 117, 118]);
        assert_eq!(&n[..4], &[0x81, 205, 0, 4], "generic NACK (FMT 1), two FCI words");
        assert_eq!(&n[12..16], &[0, 100, 0b0000_0000, 0b0000_0101], "PID 100, BLP for 101 and 103");
        assert_eq!(&n[16..20], &[0, 117, 0, 1], "117 is 17 after 100: a pair of its own, with 118");
        let wrap = nack(1, 2, &[65535, 0]);
        assert_eq!(&wrap[12..16], &[0xff, 0xff, 0, 1], "across the wrap");

        assert_eq!(pli(1, 2), vec![0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);

        let r = remb(1, 1_000_000, &[2]);
        assert_eq!(&r[..4], &[0x8f, 206, 0, 5], "PSFB FMT 15");
        assert_eq!(&r[12..16], b"REMB");
        let (exp, mantissa) = (r[17] >> 2, ((r[17] as u64 & 3) << 16) | u16::from_be_bytes([r[18], r[19]]) as u64);
        assert_eq!(mantissa << exp, 1_000_000 & !((1 << exp) - 1), "1 Mb/s as 18-bit mantissa and exponent");

        let mut sr = vec![0x80, 200, 0, 6, 0, 0, 0, 9, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        sr.extend_from_slice(&[0; 12]);
        let compound = [sr.clone(), pli(3, 4)].concat();
        assert_eq!(sender_reports(&compound), vec![(9, 0x3344_5566)], "the middle of the NTP time, from a compound packet");
    }
}
