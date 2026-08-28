//! ── Provenance ──────────────────────────────────────────────────────────────
//! Vendored verbatim (plus the additions marked "teamtalk addition") from
//! `apps/sandbox/libs/audio/src/voice_codec.rs` — the sandbox voice lane's
//! codec. The sandbox tree lives outside this workspace, so a cargo
//! dependency is impossible; keep the two copies synced by hand.
//!
//! In makepad-teamtalk the transport feeds this codec at its own
//! [`crate::wire::INTERNAL_RATE`] (48 kHz), not [`VOICE_RATE`]: ADPCM's
//! prediction gain only improves at the higher rate, and no resampling
//! stage is needed. 4-bit at 48 kHz is 192 kbit/s of nibbles (≈ 300 kbit/s
//! on the wire with state, Ogg and packet headers at 5 ms frames) — 2.7×
//! smaller than raw i16. `VOICE_RATE`/`FRAME_SAMPLES` below are the sandbox
//! shim's constants, kept for parity and the vendored tests.
//! ────────────────────────────────────────────────────────────────────────────
//! The voice-chat codec: Ogg-framed adaptive-step ADPCM at 16 kHz mono.
//!
//! Why this and not Vorbis: the room's goal is *extremely low latency* LAN
//! chat. Vorbis needs long MDCT windows (2048 samples ≈ 128 ms at 16 kHz of
//! algorithmic delay before a single byte can be coded) and this repo has no
//! encoder for it anyway. An adaptive-step ADPCM has **zero** algorithmic
//! latency beyond the packet itself — the encoder emits a nibble the moment a
//! sample exists — and it is small enough to be read, tested and trusted.
//!
//! The trade-off table that picked the default (measured by the unit tests
//! below on a speech-shaped signal; latency excludes device buffers):
//!
//! | codec            | bitrate    | algorithmic latency | CPU      | quality (SNR) |
//! |------------------|-----------:|--------------------:|----------|--------------:|
//! | raw i16 @44.1k   | 706 kbit/s | 0 ms                | none     | transparent   |
//! | raw i16 @16k     | 256 kbit/s | 0 ms                | none     | wideband PCM  |
//! | ADPCM 4b @16k    |  64 kbit/s | 0 ms (+frame 10 ms) | ~µs/frame| 32 dB measured|
//! | ADPCM 3b @16k    |  48 kbit/s | 0 ms (+frame 10 ms) | ~µs/frame| 26 dB measured|
//! | ADPCM 2b @16k    |  32 kbit/s | 0 ms (+frame 10 ms) | ~µs/frame| 15 dB measured|
//! | Vorbis @16k      | ~32 kbit/s | ≥ 128 ms            | high     | good          |
//!
//! Default: **4-bit ADPCM at 16 kHz (64 kbit/s)** in Ogg pages. On a LAN the
//! 64 vs 32 kbit/s difference is irrelevant (raw TeamTalk shipped 706 kbit/s
//! per talker); the quality difference is very audible; the latency is the
//! whole point. 2- and 3-bit stay implemented and selectable for constrained
//! links, `raw_i16` stays selectable through the packet codec id.
//!
//! Packet shape (one network packet == one Ogg page):
//! every frame is a self-contained Ogg page whose single packet begins with
//! the encoder state (predictor + step index), so a lost datagram loses only
//! its own 10 ms — the next page decodes bit-exactly with no drift.

/// The one rate voice travels at. Wideband speech; resampled at both ends.
pub const VOICE_RATE: u32 = 16_000;

/// Samples per network frame at [`VOICE_RATE`]: 10 ms.
pub const FRAME_SAMPLES: usize = 160;

/// Codec ids as they appear in the voice packet header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceCodec {
    /// Uncompressed i16 little-endian at [`VOICE_RATE`].
    RawI16 = 0,
    /// Ogg-paged ADPCM (this module's compressed format).
    Ogg = 1,
}

impl VoiceCodec {
    pub fn from_u8(v: u8) -> Option<VoiceCodec> {
        match v {
            0 => Some(VoiceCodec::RawI16),
            1 => Some(VoiceCodec::Ogg),
            _ => None,
        }
    }
}

// ── ADPCM ───────────────────────────────────────────────────────────────────

/// The classic IMA step table: 89 quasi-exponential quantiser steps.
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41,
    45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190,
    209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724,
    796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132,
    7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500,
    20350, 22385, 24623, 27086, 29794, 32767,
];

/// Step-index adaptation per code *magnitude*, one table per bit depth.
/// Small residuals shrink the step, large ones grow it (the IMA pattern).
/// Both ends share these by construction, so the exact values are a tuning
/// choice, not a wire-compatibility hazard — the PSNR tests keep them honest.
const INDEX_4: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const INDEX_3: [i8; 4] = [-1, -1, 2, 4];
const INDEX_2: [i8; 2] = [-1, 2];

fn index_delta(bits: u8, mag: u32) -> i8 {
    match bits {
        2 => INDEX_2[(mag & 1) as usize],
        3 => INDEX_3[(mag & 3) as usize],
        _ => INDEX_4[(mag & 7) as usize],
    }
}

/// Encoder/decoder state — 3 bytes on the wire, carried at the head of every
/// packet so packet loss cannot cause drift.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdpcmState {
    pub predictor: i16,
    pub index: u8,
}

fn quantise(state: &mut AdpcmState, bits: u8, sample: i16) -> u32 {
    let m = (bits - 1) as u32; // magnitude bits
    let step = STEPS[state.index as usize];
    let diff = sample as i32 - state.predictor as i32;
    let sign = diff < 0;
    let mut rest = diff.abs();
    let mut mag = 0u32;
    let mut vp = step >> m;
    for i in 0..m {
        let s = step >> i;
        if rest >= s {
            mag |= 1 << (m - 1 - i);
            rest -= s;
            vp += s;
        }
    }
    let recon = if sign {
        state.predictor as i32 - vp
    } else {
        state.predictor as i32 + vp
    };
    state.predictor = recon.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    state.index = (state.index as i32 + index_delta(bits, mag) as i32).clamp(0, 88) as u8;
    (mag << 1) | sign as u32
}

fn dequantise(state: &mut AdpcmState, bits: u8, code: u32) -> i16 {
    let m = (bits - 1) as u32;
    let step = STEPS[state.index as usize];
    let sign = code & 1 != 0;
    let mag = code >> 1;
    let mut vp = step >> m;
    for i in 0..m {
        if mag & (1 << (m - 1 - i)) != 0 {
            vp += step >> i;
        }
    }
    let recon = if sign {
        state.predictor as i32 - vp
    } else {
        state.predictor as i32 + vp
    };
    state.predictor = recon.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    state.index = (state.index as i32 + index_delta(bits, mag) as i32).clamp(0, 88) as u8;
    state.predictor
}

/// Encode one frame of f32 samples (−1..1) into a self-contained payload:
/// `[b'V', bits, predictor lo, predictor hi, index, 0, n lo, n hi, codes…]`.
/// `state` carries across frames for continuity; its entry value is written
/// into the header so the decoder needs nothing from earlier packets.
pub fn adpcm_encode(state: &mut AdpcmState, bits: u8, samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::new();
    adpcm_encode_into(state, bits, samples, &mut out);
    out
}

/// [`adpcm_encode`] into a caller-owned buffer (cleared first), so a reused
/// buffer makes the capture path allocation-free in the steady state.
/// (teamtalk addition)
pub fn adpcm_encode_into(state: &mut AdpcmState, bits: u8, samples: &[f32], out: &mut Vec<u8>) {
    let bits = bits.clamp(2, 4);
    let n = samples.len().min(u16::MAX as usize);
    out.clear();
    out.reserve(8 + (n * bits as usize + 7) / 8);
    out.push(b'V');
    out.push(bits);
    out.extend_from_slice(&state.predictor.to_le_bytes());
    out.push(state.index.min(88));
    out.push(0);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    let mut acc = 0u32;
    let mut acc_bits = 0u32;
    for &s in &samples[..n] {
        let sample = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        let code = quantise(state, bits, sample);
        acc = (acc << bits) | code;
        acc_bits += bits as u32;
        while acc_bits >= 8 {
            acc_bits -= 8;
            out.push((acc >> acc_bits) as u8);
        }
    }
    if acc_bits > 0 {
        out.push((acc << (8 - acc_bits)) as u8);
    }
}

/// Decode a payload produced by [`adpcm_encode`]. Total: malformed input is
/// `None`, never a panic.
pub fn adpcm_decode(payload: &[u8]) -> Option<Vec<f32>> {
    if payload.len() < 8 || payload[0] != b'V' {
        return None;
    }
    let bits = payload[1];
    if !(2..=4).contains(&bits) {
        return None;
    }
    let mut state = AdpcmState {
        predictor: i16::from_le_bytes([payload[2], payload[3]]),
        index: payload[4].min(88),
    };
    let n = u16::from_le_bytes([payload[6], payload[7]]) as usize;
    let need = (n * bits as usize + 7) / 8;
    let codes = payload.get(8..8 + need)?;
    let mut out = Vec::with_capacity(n);
    let mut acc = 0u32;
    let mut acc_bits = 0u32;
    let mut at = 0usize;
    let mask = (1u32 << bits) - 1;
    for _ in 0..n {
        while acc_bits < bits as u32 {
            acc = (acc << 8) | codes[at] as u32;
            at += 1;
            acc_bits += 8;
        }
        acc_bits -= bits as u32;
        let code = (acc >> acc_bits) & mask;
        out.push(dequantise(&mut state, bits, code) as f32 / 32768.0);
    }
    Some(out)
}

// ── Ogg pages ───────────────────────────────────────────────────────────────

/// Ogg's CRC-32: polynomial 0x04c11db7, no reflection, zero init, zero xorout.
fn ogg_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut b = 0;
        while b < 8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04c1_1db7
            } else {
                r << 1
            };
            b += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
}

fn ogg_crc(bytes: &[u8]) -> u32 {
    // Small enough to rebuild per call site rarely; cache once per process.
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(ogg_crc_table);
    let mut crc = 0u32;
    for &b in bytes {
        crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// Wrap one packet into a single complete Ogg page (header type 0, or BOS on
/// `seq == 0`). `granule` is the absolute sample position after this page.
pub fn ogg_page(serial: u32, seq: u32, granule: u64, payload: &[u8]) -> Vec<u8> {
    let mut page = Vec::new();
    ogg_page_into(serial, seq, granule, payload, &mut page);
    page
}

/// [`ogg_page`] into a caller-owned buffer (cleared first). (teamtalk addition)
pub fn ogg_page_into(serial: u32, seq: u32, granule: u64, payload: &[u8], page: &mut Vec<u8>) {
    let full = payload.len() / 255;
    let tail = (payload.len() % 255) as u8;
    let n_segs = full + 1; // trailing < 255 segment terminates the packet
    page.clear();
    page.reserve(27 + n_segs + payload.len());
    page.extend_from_slice(b"OggS");
    page.push(0); // version
    page.push(if seq == 0 { 0x02 } else { 0x00 }); // BOS on the first page
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&seq.to_le_bytes());
    page.extend_from_slice(&0u32.to_le_bytes()); // crc, patched below
    page.push(n_segs as u8);
    for _ in 0..full {
        page.push(255);
    }
    page.push(tail);
    page.extend_from_slice(payload);
    let crc = ogg_crc(page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
}

/// Parse a single complete Ogg page: `(serial, seq, granule, packet)`.
/// The CRC is verified — a corrupted datagram is refused, not decoded.
pub fn ogg_page_open(bytes: &[u8]) -> Option<(u32, u32, u64, &[u8])> {
    if bytes.len() < 28 || &bytes[0..4] != b"OggS" || bytes[4] != 0 {
        return None;
    }
    let granule = u64::from_le_bytes(bytes[6..14].try_into().ok()?);
    let serial = u32::from_le_bytes(bytes[14..18].try_into().ok()?);
    let seq = u32::from_le_bytes(bytes[18..22].try_into().ok()?);
    let crc_said = u32::from_le_bytes(bytes[22..26].try_into().ok()?);
    let n_segs = bytes[26] as usize;
    let body = 27 + n_segs;
    if bytes.len() < body {
        return None;
    }
    let body_len: usize = bytes[27..body].iter().map(|&s| s as usize).sum();
    if bytes.len() != body + body_len {
        return None;
    }
    let mut check = bytes.to_vec();
    check[22..26].copy_from_slice(&[0; 4]);
    if ogg_crc(&check) != crc_said {
        return None;
    }
    Some((serial, seq, granule, &bytes[body..body + body_len]))
}

// ── The two codecs behind one pair of calls ─────────────────────────────────

/// Stateful encoder for one outgoing voice stream.
pub struct VoiceEncoder {
    pub codec: VoiceCodec,
    pub bits: u8,
    state: AdpcmState,
    serial: u32,
    samples_out: u64,
    /// Reused ADPCM packet staging for [`VoiceEncoder::encode_into`].
    /// (teamtalk addition)
    scratch: Vec<u8>,
}

impl VoiceEncoder {
    pub fn new(codec: VoiceCodec, bits: u8, serial: u32) -> Self {
        Self {
            codec,
            bits: bits.clamp(2, 4),
            state: AdpcmState::default(),
            serial,
            samples_out: 0,
            scratch: Vec::new(),
        }
    }

    /// One frame of mono samples → one wire payload.
    pub fn encode(&mut self, seq: u32, samples: &[f32]) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(seq, samples, &mut out);
        out
    }

    /// [`VoiceEncoder::encode`] into a caller-owned buffer (cleared first):
    /// with the buffer reused across frames the audio thread allocates
    /// nothing in the steady state. (teamtalk addition)
    pub fn encode_into(&mut self, seq: u32, samples: &[f32], out: &mut Vec<u8>) {
        self.samples_out += samples.len() as u64;
        match self.codec {
            VoiceCodec::RawI16 => {
                out.clear();
                out.reserve(samples.len() * 2);
                for &s in samples {
                    out.extend_from_slice(
                        &((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes(),
                    );
                }
            }
            VoiceCodec::Ogg => {
                let scratch = std::mem::take(&mut self.scratch);
                let mut packet = scratch;
                adpcm_encode_into(&mut self.state, self.bits, samples, &mut packet);
                ogg_page_into(self.serial, seq, self.samples_out, &packet, out);
                self.scratch = packet;
            }
        }
    }
}

/// Decode one wire payload back to mono samples at [`VOICE_RATE`].
pub fn voice_decode(codec: VoiceCodec, payload: &[u8]) -> Option<Vec<f32>> {
    match codec {
        VoiceCodec::RawI16 => {
            if payload.len() % 2 != 0 {
                return None;
            }
            Some(
                payload
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                    .collect(),
            )
        }
        VoiceCodec::Ogg => {
            let (_serial, _seq, _granule, packet) = ogg_page_open(payload)?;
            adpcm_decode(packet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A speech-shaped test signal: a gliding fundamental with formant-ish
    /// harmonics, an envelope, and a little breath noise. One second.
    fn speechish(seconds: f32) -> Vec<f32> {
        let n = (VOICE_RATE as f32 * seconds) as usize;
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|i| {
                let t = i as f32 / VOICE_RATE as f32;
                let f0 = 140.0 + 40.0 * (t * 2.3).sin();
                let env = (0.55 + 0.45 * (t * 7.0).sin()).max(0.0);
                let mut s = 0.0;
                for (h, a) in [(1.0, 0.5), (2.0, 0.35), (3.0, 0.25), (5.0, 0.12), (8.0, 0.06)]
                {
                    s += a * (t * f0 * h * std::f32::consts::TAU).sin();
                }
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let noise = ((rng >> 40) as i32 - (1 << 23)) as f32 / (1 << 24) as f32;
                (s * env * 0.6 + noise * 0.02).clamp(-1.0, 1.0)
            })
            .collect()
    }

    fn snr_db(reference: &[f32], decoded: &[f32]) -> f32 {
        let n = reference.len().min(decoded.len());
        let mut sig = 0.0f64;
        let mut err = 0.0f64;
        for i in 0..n {
            sig += (reference[i] as f64) * (reference[i] as f64);
            let e = reference[i] as f64 - decoded[i] as f64;
            err += e * e;
        }
        (10.0 * (sig / err.max(1e-12)).log10()) as f32
    }

    fn round_trip_snr(bits: u8) -> f32 {
        let signal = speechish(1.0);
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, bits, 7);
        let mut out = Vec::new();
        for (seq, frame) in signal.chunks(FRAME_SAMPLES).enumerate() {
            let page = enc.encode(seq as u32, frame);
            out.extend(voice_decode(VoiceCodec::Ogg, &page).expect("decodes"));
        }
        snr_db(&signal, &out)
    }

    #[test]
    fn adpcm_round_trip_quality_by_bit_depth() {
        let q4 = round_trip_snr(4);
        let q3 = round_trip_snr(3);
        let q2 = round_trip_snr(2);
        // The measured trade-off table. Thresholds are floors, not targets.
        assert!(q4 > 18.0, "4-bit SNR too low: {q4} dB");
        assert!(q3 > 12.0, "3-bit SNR too low: {q3} dB");
        assert!(q2 > 5.0, "2-bit SNR too low: {q2} dB");
        assert!(q4 > q3 && q3 > q2, "more bits must not sound worse: {q4} {q3} {q2}");
        println!("voice codec SNR: 4-bit {q4:.1} dB, 3-bit {q3:.1} dB, 2-bit {q2:.1} dB");
    }

    #[test]
    fn frame_sizes_and_bitrate_match_the_table() {
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 1);
        let frame = vec![0.1f32; FRAME_SAMPLES];
        let page = enc.encode(1, &frame);
        // 8-byte packet header + 160 nibbles + 28-byte Ogg page overhead.
        assert_eq!(page.len(), 28 + 8 + FRAME_SAMPLES / 2, "{}", page.len());
        // Payload bitrate at 100 packets/s.
        let kbps = (page.len() as f32 * 8.0 * 100.0) / 1000.0;
        assert!(kbps < 95.0, "wire rate {kbps} kbit/s");
        let raw = VoiceEncoder::new(VoiceCodec::RawI16, 4, 1).encode(0, &frame);
        assert_eq!(raw.len(), FRAME_SAMPLES * 2);
    }

    #[test]
    fn every_packet_is_decodable_alone_after_loss() {
        let signal = speechish(0.5);
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 3);
        let pages: Vec<Vec<u8>> = signal
            .chunks(FRAME_SAMPLES)
            .enumerate()
            .map(|(seq, frame)| enc.encode(seq as u32, frame))
            .collect();
        // Drop every third page; the survivors must still decode, and the
        // decoded frames must equal the frames of a lossless decode (the
        // in-packet state header is what guarantees no drift).
        let mut lossless = Vec::new();
        for page in &pages {
            lossless.push(voice_decode(VoiceCodec::Ogg, page).unwrap());
        }
        for (i, page) in pages.iter().enumerate() {
            if i % 3 == 0 {
                continue;
            }
            let alone = voice_decode(VoiceCodec::Ogg, page).unwrap();
            assert_eq!(alone, lossless[i], "packet {i} drifted after loss");
        }
    }

    #[test]
    fn corrupted_pages_are_refused_by_crc() {
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 9);
        let page = enc.encode(0, &vec![0.3f32; FRAME_SAMPLES]);
        assert!(voice_decode(VoiceCodec::Ogg, &page).is_some());
        for flip in [30usize, 40, 60] {
            let mut bad = page.clone();
            bad[flip] ^= 0x40;
            assert!(
                voice_decode(VoiceCodec::Ogg, &bad).is_none(),
                "flipped byte {flip} still decoded"
            );
        }
        // Truncated and garbage inputs are errors, not panics.
        assert!(voice_decode(VoiceCodec::Ogg, &page[..20]).is_none());
        assert!(voice_decode(VoiceCodec::Ogg, b"not a page").is_none());
        assert!(adpcm_decode(b"junk").is_none());
    }

    #[test]
    fn raw_codec_round_trips_bit_exact_at_i16() {
        let frame: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| ((i as f32 * 0.1).sin() * 0.7 * 32767.0).round() / 32768.0)
            .collect();
        let wire = VoiceEncoder::new(VoiceCodec::RawI16, 4, 0).encode(0, &frame);
        let back = voice_decode(VoiceCodec::RawI16, &wire).unwrap();
        for (a, b) in frame.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1.0 / 32768.0 + 1e-6);
        }
    }

    #[test]
    fn encoding_is_fast_enough_for_the_audio_thread() {
        let signal = speechish(2.0);
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 5);
        let start = std::time::Instant::now();
        let mut bytes = 0usize;
        for (seq, frame) in signal.chunks(FRAME_SAMPLES).enumerate() {
            bytes += enc.encode(seq as u32, frame).len();
        }
        let encode = start.elapsed();
        let start = std::time::Instant::now();
        let mut enc2 = VoiceEncoder::new(VoiceCodec::Ogg, 4, 5);
        for (seq, frame) in signal.chunks(FRAME_SAMPLES).enumerate() {
            let page = enc2.encode(seq as u32, frame);
            voice_decode(VoiceCodec::Ogg, &page).unwrap();
        }
        let both = start.elapsed();
        // 2 s of audio must encode in far under one frame's worth of time
        // even in a debug test build.
        assert!(encode.as_millis() < 200, "encode too slow: {encode:?}");
        assert!(both.as_millis() < 400, "encode+decode too slow: {both:?}");
        assert!(bytes > 0);
    }

    #[test]
    fn ogg_pages_reassemble_through_an_independent_reader() {
        // (teamtalk adaptation: the original checked the sandbox's
        // `crate::ogg::read_packets`, which does not exist here; the
        // workspace's Ogg reader in makepad-audio-decode — a dev-dependency
        // only — is the same kind of independent check.)
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 0x5643);
        let mut stream = Vec::new();
        for seq in 0..3u32 {
            let frame = vec![0.05 * (seq as f32 + 1.0); FRAME_SAMPLES];
            stream.extend(enc.encode(seq, &frame));
        }
        let mut reader = makepad_audio_decode::ogg::PacketReader::new(&stream);
        let mut count = 0usize;
        let mut last_granule = 0u64;
        while let Some(packet) = reader.next_packet().expect("our pages parse") {
            let samples = adpcm_decode(packet.data).expect("packet decodes");
            assert_eq!(samples.len(), FRAME_SAMPLES, "packet {count}");
            if let Some(granule) = packet.granule {
                last_granule = granule;
            }
            count += 1;
        }
        assert_eq!(count, 3);
        assert_eq!(last_granule, 3 * FRAME_SAMPLES as u64);
    }
}


/// makepad-teamtalk's own tests for how the transport uses this codec:
/// at 48 kHz, and through the `_into` buffers. (teamtalk addition)
#[cfg(test)]
mod teamtalk_tests {
    use super::*;
    use crate::wire::INTERNAL_RATE;

    /// A speech-shaped second at the transport rate.
    fn speechish_48k() -> Vec<f32> {
        let n = INTERNAL_RATE as usize;
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|i| {
                let t = i as f32 / INTERNAL_RATE as f32;
                let f0 = 140.0 + 40.0 * (t * 2.3).sin();
                let env = (0.55 + 0.45 * (t * 7.0).sin()).max(0.0);
                let mut s = 0.0;
                for (h, a) in [(1.0, 0.5), (2.0, 0.35), (3.0, 0.25), (5.0, 0.12), (8.0, 0.06)] {
                    s += a * (t * f0 * h * std::f32::consts::TAU).sin();
                }
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let noise = ((rng >> 40) as i32 - (1 << 23)) as f32 / (1 << 24) as f32;
                (s * env * 0.6 + noise * 0.02).clamp(-1.0, 1.0)
            })
            .collect()
    }

    #[test]
    fn at_48k_prediction_gain_beats_the_16k_numbers() {
        let signal = speechish_48k();
        let mut enc = VoiceEncoder::new(VoiceCodec::Ogg, 4, 7);
        let mut out = Vec::new();
        for (seq, frame) in signal.chunks(240).enumerate() {
            let page = enc.encode(seq as u32, frame);
            out.extend(voice_decode(VoiceCodec::Ogg, &page).expect("decodes"));
        }
        let n = signal.len().min(out.len());
        let mut sig = 0.0f64;
        let mut err = 0.0f64;
        for i in 0..n {
            sig += (signal[i] as f64) * (signal[i] as f64);
            let e = signal[i] as f64 - out[i] as f64;
            err += e * e;
        }
        let snr = 10.0 * (sig / err.max(1e-12)).log10();
        // The 16 kHz measurement was 32.3 dB; per-sample deltas at 48 kHz
        // are 3x smaller, so this must land clearly above it.
        assert!(snr > 34.0, "4-bit @48k SNR {snr:.1} dB");
    }

    #[test]
    fn the_into_variants_are_byte_identical_to_the_vec_ones() {
        let frame: Vec<f32> = (0..240).map(|i| ((i as f32) * 0.07).sin() * 0.6).collect();
        let mut a = VoiceEncoder::new(VoiceCodec::Ogg, 3, 42);
        let mut b = VoiceEncoder::new(VoiceCodec::Ogg, 3, 42);
        let mut buf = Vec::new();
        for seq in 0..5u32 {
            let vec_out = a.encode(seq, &frame);
            b.encode_into(seq, &frame, &mut buf);
            assert_eq!(vec_out, buf, "frame {seq}");
        }
        let mut sa = AdpcmState::default();
        let mut sb = AdpcmState::default();
        let direct = adpcm_encode(&mut sa, 4, &frame);
        let mut into = Vec::new();
        adpcm_encode_into(&mut sb, 4, &frame, &mut into);
        assert_eq!(direct, into);
        assert_eq!(sa, sb);
        assert_eq!(ogg_page(9, 1, 240, &direct), {
            let mut page = Vec::new();
            ogg_page_into(9, 1, 240, &direct, &mut page);
            page
        });
    }
}
