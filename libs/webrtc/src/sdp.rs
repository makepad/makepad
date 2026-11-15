//! The browser's offer read, and our answer written (RFC 8866, JSEP RFC 8829).
//!
//! We answer as an ICE-lite peer (RFC 8445) on one host candidate, DTLS
//! passive (the browser connects), receive-only, everything bundled and
//! RTCP muxed on the one port. Of the offer we take one video section and
//! one H.264 format of it (packetization-mode 1, constrained baseline when
//! offered, then any baseline, then any); every other section is rejected
//! (port 0), as JSEP lets an answerer do.

use std::fmt::Write;
use std::net::SocketAddr;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Codec {
    pub payload_type: u8,
    /// As written, e.g. "H264".
    pub name: String,
    pub clock: u32,
    pub fmtp: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Media {
    /// "video", "audio", "application", ...
    pub kind: String,
    pub port: u16,
    pub proto: String,
    pub formats: Vec<String>,
    pub mid: Option<String>,
    pub ice_ufrag: Option<String>,
    pub ice_pwd: Option<String>,
    /// (hash name, digest) of the browser's DTLS certificate.
    pub fingerprint: Option<(String, Vec<u8>)>,
    pub setup: Option<String>,
    pub codecs: Vec<Codec>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    /// Session-level attributes count for every section without its own.
    pub session: Media,
    pub media: Vec<Media>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdpError {
    /// No video section with H.264 in packetization-mode 1.
    NoH264,
    /// The video section has no ICE credentials or no DTLS fingerprint.
    NoSecurity,
}

fn fingerprint(value: &str) -> Option<(String, Vec<u8>)> {
    let (hash, hex) = value.trim().split_once(' ')?;
    let bytes: Option<Vec<u8>> = hex.trim().split(':').map(|b| u8::from_str_radix(b, 16).ok()).collect();
    Some((hash.to_ascii_lowercase(), bytes?))
}

/// Reads an offer; unknown lines are skipped.
pub fn parse_offer(text: &str) -> Offer {
    let mut offer = Offer::default();
    for line in text.lines().map(str::trim_end) {
        let Some((kind, value)) = line.split_once('=') else { continue };
        if kind == "m" {
            let mut parts = value.split_whitespace();
            let kind = parts.next().unwrap_or_default().to_string();
            let port = parts.next().and_then(|p| p.split('/').next()?.parse().ok()).unwrap_or(0);
            let proto = parts.next().unwrap_or_default().to_string();
            let formats = parts.map(str::to_string).collect();
            offer.media.push(Media { kind, port, proto, formats, ..Media::default() });
            continue;
        }
        if kind != "a" {
            continue;
        }
        let target = offer.media.last_mut().unwrap_or(&mut offer.session);
        let (name, arg) = value.split_once(':').unwrap_or((value, ""));
        match name {
            "mid" => target.mid = Some(arg.to_string()),
            "ice-ufrag" => target.ice_ufrag = Some(arg.to_string()),
            "ice-pwd" => target.ice_pwd = Some(arg.to_string()),
            "fingerprint" => target.fingerprint = fingerprint(arg),
            "setup" => target.setup = Some(arg.to_string()),
            "rtpmap" => {
                let Some((pt, codec)) = arg.split_once(' ') else { continue };
                let mut parts = codec.split('/');
                let (Ok(payload_type), Some(name)) = (pt.parse(), parts.next()) else { continue };
                let clock = parts.next().and_then(|c| c.parse().ok()).unwrap_or(0);
                target.codecs.push(Codec { payload_type, name: name.to_string(), clock, ..Codec::default() });
            }
            "fmtp" => {
                let Some((pt, params)) = arg.split_once(' ') else { continue };
                if let Some(codec) = target.codecs.iter_mut().find(|c| pt.parse() == Ok(c.payload_type)) {
                    codec.fmtp = params.to_string();
                }
            }
            _ => {}
        }
    }
    offer
}

fn fmtp_value<'a>(fmtp: &'a str, key: &str) -> Option<&'a str> {
    fmtp.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

/// How much we like an offered format: None when we cannot take it.
fn h264_rank(codec: &Codec) -> Option<u8> {
    if !codec.name.eq_ignore_ascii_case("H264") || fmtp_value(&codec.fmtp, "packetization-mode") != Some("1") {
        return None;
    }
    let profile = fmtp_value(&codec.fmtp, "profile-level-id").unwrap_or("42e01f").to_ascii_lowercase();
    Some(if profile.starts_with("42e0") {
        0
    } else if profile.starts_with("42") {
        1
    } else {
        2
    })
}

/// What we put in the answer about ourselves.
pub struct Local<'a> {
    pub ice_ufrag: &'a str,
    pub ice_pwd: &'a str,
    /// SHA-256 of our DTLS certificate.
    pub fingerprint: &'a [u8; 32],
    /// Where the browser reaches us (one host candidate each).
    pub candidates: &'a [SocketAddr],
    pub session_id: u64,
}

/// What the answer took of the offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub sdp: String,
    pub mid: String,
    pub codec: Codec,
    pub remote_ufrag: String,
    pub remote_pwd: String,
    pub remote_fingerprint: (String, Vec<u8>),
}

/// Our answer to `offer`.
pub fn answer(offer: &Offer, local: &Local) -> Result<Answer, SdpError> {
    let video = offer
        .media
        .iter()
        .enumerate()
        .filter(|(_, m)| m.kind == "video" && m.port != 0)
        .filter_map(|(i, m)| m.codecs.iter().filter_map(|c| Some((h264_rank(c)?, i, c))).min_by_key(|(rank, _, _)| *rank))
        .min_by_key(|(rank, i, _)| (*rank, *i));
    let Some((_, at, codec)) = video else { return Err(SdpError::NoH264) };
    let m = &offer.media[at];
    let s = &offer.session;
    let (Some(ufrag), Some(pwd), Some(remote_fingerprint)) = (
        m.ice_ufrag.as_ref().or(s.ice_ufrag.as_ref()),
        m.ice_pwd.as_ref().or(s.ice_pwd.as_ref()),
        m.fingerprint.as_ref().or(s.fingerprint.as_ref()),
    ) else {
        return Err(SdpError::NoSecurity);
    };
    let mid = m.mid.clone().unwrap_or_else(|| at.to_string());
    let ours: Vec<String> = local.fingerprint.iter().map(|b| format!("{b:02X}")).collect();
    let pt = codec.payload_type;

    let mut sdp = String::new();
    let mut line = |text: String| {
        sdp.push_str(&text);
        sdp.push_str("\r\n");
    };
    line("v=0".into());
    line(format!("o=- {} 2 IN IP4 127.0.0.1", local.session_id));
    line("s=-".into());
    line("t=0 0".into());
    line("a=ice-lite".into());
    line(format!("a=group:BUNDLE {mid}"));
    for (i, media) in offer.media.iter().enumerate() {
        if i != at {
            // Not taken: rejected, its mid kept so the browser can match it.
            line(format!("m={} 0 {} {}", media.kind, media.proto, media.formats.first().map_or("0", String::as_str)));
            line("c=IN IP4 0.0.0.0".into());
            if let Some(mid) = &media.mid {
                line(format!("a=mid:{mid}"));
            }
            continue;
        }
        line(format!("m=video 9 {} {pt}", media.proto));
        line("c=IN IP4 0.0.0.0".into());
        line("a=rtcp:9 IN IP4 0.0.0.0".into());
        line(format!("a=ice-ufrag:{}", local.ice_ufrag));
        line(format!("a=ice-pwd:{}", local.ice_pwd));
        line(format!("a=fingerprint:sha-256 {}", ours.join(":")));
        line("a=setup:passive".into());
        line(format!("a=mid:{mid}"));
        line("a=recvonly".into());
        line("a=rtcp-mux".into());
        line("a=rtcp-rsize".into());
        line(format!("a=rtpmap:{pt} {}/{}", codec.name, codec.clock));
        if !codec.fmtp.is_empty() {
            line(format!("a=fmtp:{pt} {}", codec.fmtp));
        }
        for fb in ["nack", "nack pli", "goog-remb"] {
            line(format!("a=rtcp-fb:{pt} {fb}"));
        }
        for (k, addr) in local.candidates.iter().enumerate() {
            let mut c = String::new();
            let _ = write!(c, "a=candidate:{} 1 udp {} {} {} typ host", k + 1, 2_130_706_431 - k as u32, addr.ip(), addr.port());
            line(c);
        }
        line("a=end-of-candidates".into());
    }
    Ok(Answer {
        sdp,
        mid,
        codec: codec.clone(),
        remote_ufrag: ufrag.clone(),
        remote_pwd: pwd.clone(),
        remote_fingerprint: remote_fingerprint.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An offer shaped like a phone browser's: audio first, video with
    /// several H.264 formats and VP8, a data channel.
    const OFFER: &str = "v=0\r\n\
o=- 4611731400430051336 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0 1 2\r\n\
a=extmap-allow-mixed\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111 0\r\n\
c=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:Gd7q\r\n\
a=ice-pwd:ZLRb2lXo8x9C1Y4o1dkV2X0m\r\n\
a=fingerprint:sha-256 0A:1B:2C:3D:4E:5F:60:71:82:93:A4:B5:C6:D7:E8:F9:0A:1B:2C:3D:4E:5F:60:71:82:93:A4:B5:C6:D7:E8:F9\r\n\
a=setup:actpass\r\n\
a=mid:0\r\n\
a=sendrecv\r\n\
a=rtpmap:111 opus/48000/2\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96 97 102 103 125\r\n\
c=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:Gd7q\r\n\
a=ice-pwd:ZLRb2lXo8x9C1Y4o1dkV2X0m\r\n\
a=fingerprint:sha-256 0A:1B:2C:3D:4E:5F:60:71:82:93:A4:B5:C6:D7:E8:F9:0A:1B:2C:3D:4E:5F:60:71:82:93:A4:B5:C6:D7:E8:F9\r\n\
a=setup:actpass\r\n\
a=mid:1\r\n\
a=sendonly\r\n\
a=rtpmap:96 VP8/90000\r\n\
a=rtcp-fb:96 nack\r\n\
a=rtpmap:97 rtx/90000\r\n\
a=fmtp:97 apt=96\r\n\
a=rtpmap:102 H264/90000\r\n\
a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f\r\n\
a=rtpmap:103 H264/90000\r\n\
a=fmtp:103 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=640c1f\r\n\
a=rtpmap:125 H264/90000\r\n\
a=fmtp:125 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:2\r\n";

    #[test]
    fn the_answer_takes_one_h264_stream_and_rejects_the_rest() {
        let offer = parse_offer(OFFER);
        assert_eq!(offer.media.len(), 3);
        let fp = [0xabu8; 32];
        let local = Local {
            ice_ufrag: "ours",
            ice_pwd: "a-password-of-twenty-two",
            fingerprint: &fp,
            candidates: &["192.0.2.10:50000".parse().unwrap()],
            session_id: 42,
        };
        let a = answer(&offer, &local).unwrap();
        assert_eq!((a.mid.as_str(), a.codec.payload_type), ("1", 125), "constrained baseline in mode 1 wins over high and mode 0");
        assert_eq!((a.remote_ufrag.as_str(), a.remote_pwd.as_str()), ("Gd7q", "ZLRb2lXo8x9C1Y4o1dkV2X0m"));
        assert_eq!(a.remote_fingerprint.0, "sha-256");
        assert_eq!(a.remote_fingerprint.1[..2], [0x0a, 0x1b]);

        let lines: Vec<&str> = a.sdp.split("\r\n").collect();
        assert!(a.sdp.ends_with("\r\n") && !a.sdp.contains("\n\n"), "CRLF lines");
        for want in [
            "a=ice-lite",
            "a=group:BUNDLE 1",
            "m=audio 0 UDP/TLS/RTP/SAVPF 111",
            "m=video 9 UDP/TLS/RTP/SAVPF 125",
            "m=application 0 UDP/DTLS/SCTP webrtc-datachannel",
            "a=setup:passive",
            "a=recvonly",
            "a=rtcp-mux",
            "a=rtpmap:125 H264/90000",
            "a=fmtp:125 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f",
            "a=rtcp-fb:125 nack pli",
            "a=rtcp-fb:125 goog-remb",
            "a=candidate:1 1 udp 2130706431 192.0.2.10 50000 typ host",
            "a=mid:0",
            "a=mid:2",
        ] {
            assert!(lines.contains(&want), "answer line {want:?} missing:\n{}", a.sdp);
        }
        let ours = lines.iter().find(|l| l.starts_with("a=fingerprint:")).unwrap();
        assert_eq!(ours.len(), "a=fingerprint:sha-256 ".len() + 32 * 3 - 1, "32 bytes as AB:AB:..");
        assert!(ours.ends_with(":AB:AB"));

        let no_h264 = OFFER.replace("packetization-mode=1", "packetization-mode=0");
        assert_eq!(answer(&parse_offer(&no_h264), &local), Err(SdpError::NoH264));
        let no_fingerprint: String = OFFER.lines().filter(|l| !l.starts_with("a=fingerprint")).map(|l| format!("{l}\n")).collect();
        assert_eq!(answer(&parse_offer(&no_fingerprint), &local), Err(SdpError::NoSecurity));
    }
}
