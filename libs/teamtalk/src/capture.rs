//! The send side: called from the application's audio *input* callback.
//!
//! [`CaptureHandle::push_planar`] (or `push_mono` / `push_interleaved`) takes
//! whatever the device delivers — any rate, any channel count, any block
//! size — folds it to mono, resamples it to the wire rate, cuts it into
//! frames, gates/limits them, and sends each finished frame as one UDP
//! datagram straight from the calling thread. On a LAN a nonblocking UDP
//! send is a ~10 µs syscall, which is cheaper and far lower-latency than
//! waking a sender thread; there is no allocation and no lock anywhere on
//! this path.
//!
//! While the gate is closed (or the link is muted) a 24-byte silence header
//! is sent per frame instead of audio, so receivers keep sequence continuity,
//! presence, and exact silence timing for free. The gate has no lookahead —
//! it can never delay or clip an onset: the frame speech starts in is sent
//! whole, with only a 1 ms anti-click fade at its very start.

use crate::codec::{VoiceCodec, VoiceEncoder};
use crate::dsp::{fade_in, fade_out_tail, fold_interleaved_to_mono, Gate, GateState, Limiter};
use crate::link::{Delivery, Shared};
use crate::peers::unpack_addr;
use crate::resample::Resampler;
use crate::wire::{
    encode_header_only, encode_raw_i16, flags, Codec, Header, HEADER_LEN, INTERNAL_RATE, MAX_FRAME,
    MAX_PACKET,
};
use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Device samples folded/consumed per inner iteration.
const CHUNK: usize = 512;
/// Staging for resampled output of one chunk: covers device rates down to 6 kHz.
const STAGING: usize = CHUNK * 8 + 8;
/// Anti-click fade at gate edges, in samples (1 ms at 48 kHz).
const EDGE_FADE: usize = 48;

/// The capture half of a [`crate::VoiceLink`]. `Send` (move it into the audio
/// input callback), not `Clone`: exactly one thread feeds it.
pub struct CaptureHandle {
    shared: Arc<Shared>,
    socket: UdpSocket,
    resampler: Resampler,
    staging: [f32; STAGING],
    mono: [f32; CHUNK],
    frame: [f32; MAX_FRAME],
    frame_len: usize,
    fill: usize,
    seq: u32,
    timestamp: u32,
    gate: Gate,
    limiter: Limiter,
    packet: [u8; MAX_PACKET],
    encoder: VoiceEncoder,
    /// Reused Ogg page buffer: no allocation per frame once grown.
    page_buf: Vec<u8>,
}

impl CaptureHandle {
    pub(crate) fn new(shared: Arc<Shared>, socket: UdpSocket) -> Self {
        let frame_len = shared.frame_samples;
        let hangover_frames =
            (shared.gate_hangover_ms as f64 * INTERNAL_RATE / 1000.0 / frame_len as f64) as u32;
        let serial = shared.sender_id.load(Ordering::Relaxed) as u32;
        let bits = shared.adpcm_bits.load(Ordering::Relaxed);
        Self {
            gate: Gate::new(shared.gate_threshold_rms, hangover_frames.max(1)),
            limiter: Limiter::new(0.95, 0.5, 60.0),
            encoder: VoiceEncoder::new(VoiceCodec::Ogg, bits, serial),
            page_buf: Vec::with_capacity(MAX_PACKET),
            shared,
            socket,
            resampler: Resampler::new(),
            staging: [0.0; STAGING],
            mono: [0.0; CHUNK],
            frame: [0.0; MAX_FRAME],
            frame_len,
            fill: 0,
            seq: 0,
            timestamp: 0,
            packet: [0; MAX_PACKET],
        }
    }

    /// Feed a mono block at `rate` Hz.
    pub fn push_mono(&mut self, rate: f64, samples: &[f32]) {
        for chunk in samples.chunks(CHUNK) {
            self.mono[..chunk.len()].copy_from_slice(chunk);
            self.push_chunk(rate, chunk.len());
        }
    }

    /// Feed a planar (channel-major) block, as
    /// `makepad_platform::audio::AudioBuffer` stores it: `data.len()` must be
    /// at least `frames * channels`. Channels are averaged to mono.
    pub fn push_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &[f32]) {
        let channels = channels.max(1);
        if data.len() < frames * channels {
            return;
        }
        let scale = 1.0 / channels as f32;
        let mut off = 0;
        while off < frames {
            let n = (frames - off).min(CHUNK);
            for f in 0..n {
                let mut acc = 0.0;
                for c in 0..channels {
                    acc += data[c * frames + off + f];
                }
                self.mono[f] = acc * scale;
            }
            self.push_chunk(rate, n);
            off += n;
        }
    }

    /// Feed an interleaved block. Channels are averaged to mono.
    pub fn push_interleaved(&mut self, rate: f64, channels: usize, data: &[f32]) {
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let mut off = 0;
        while off < frames {
            let n = (frames - off).min(CHUNK);
            fold_interleaved_to_mono(
                channels,
                &data[off * channels..(off + n) * channels],
                &mut self.mono[..n],
            );
            self.push_chunk(rate, n);
            off += n;
        }
    }

    /// Frames sent so far (audio and silence).
    pub fn frames_sent(&self) -> u32 {
        self.seq
    }

    /// `self.mono[..n]` holds folded device-rate samples: resample + frame + send.
    fn push_chunk(&mut self, rate: f64, n: usize) {
        let gain = f32::from_bits(self.shared.input_gain.load(Ordering::Relaxed));
        self.resampler.set_ratio(rate.max(1000.0), INTERNAL_RATE, 0.0);
        // Split borrows: the resampler pushes into the staging area.
        let resampler = &mut self.resampler;
        let staging = &mut self.staging;
        let mut staged = 0usize;
        for i in 0..n {
            let x = self.mono[i] * gain;
            resampler.push(x, |y| {
                if staged < STAGING {
                    staging[staged] = y;
                    staged += 1;
                }
            });
        }
        for i in 0..staged {
            let y = self.staging[i];
            self.frame[self.fill] = y;
            self.fill += 1;
            if self.fill == self.frame_len {
                self.fill = 0;
                self.finish_frame();
            }
        }
    }

    fn finish_frame(&mut self) {
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let channel = self.shared.channel.load(Ordering::Relaxed);
        let room = self.shared.room.load(Ordering::Relaxed);
        let sender = self.shared.sender_id.load(Ordering::Relaxed);
        let state = if muted {
            GateState::Silent
        } else {
            self.gate.process(&self.frame[..self.frame_len])
        };
        let mut header = Header {
            codec: Codec::RawI16,
            channel,
            flags: 0,
            frames: self.frame_len as u16,
            room,
            sender,
            seq: self.seq,
            timestamp: self.timestamp,
        };
        let len = match state {
            GateState::Silent => {
                header.flags = flags::SILENCE;
                encode_header_only(header, &mut self.packet)
            }
            audible => {
                match audible {
                    GateState::Opened => {
                        header.flags = flags::TALK_START;
                        fade_in(&mut self.frame[..self.frame_len], EDGE_FADE);
                    }
                    GateState::Closing => {
                        fade_out_tail(&mut self.frame[..self.frame_len], EDGE_FADE);
                    }
                    _ => {}
                }
                self.limiter.process(&mut self.frame[..self.frame_len]);
                match Codec::from_u8(self.shared.wire_codec.load(Ordering::Relaxed)) {
                    Some(Codec::Ogg) => {
                        // The vendored Ogg/ADPCM codec, fed at INTERNAL_RATE.
                        // Every page is self-contained, so a lost datagram
                        // cannot drift the decoder.
                        self.encoder.codec = VoiceCodec::Ogg;
                        self.encoder.bits = self.shared.adpcm_bits.load(Ordering::Relaxed);
                        let page_buf = std::mem::take(&mut self.page_buf);
                        let mut page = page_buf;
                        self.encoder
                            .encode_into(self.seq, &self.frame[..self.frame_len], &mut page);
                        header.codec = Codec::Ogg;
                        header.write(&mut self.packet[..HEADER_LEN]);
                        // 960-sample 4-bit pages are ≤ 517 B; this cannot
                        // exceed MAX_PACKET, but never trust that silently.
                        let len = (HEADER_LEN + page.len()).min(MAX_PACKET);
                        self.packet[HEADER_LEN..len]
                            .copy_from_slice(&page[..len - HEADER_LEN]);
                        self.page_buf = page;
                        len
                    }
                    _ => encode_raw_i16(header, &self.frame[..self.frame_len], &mut self.packet),
                }
            }
        };
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(self.frame_len as u32);
        self.send(len);
    }

    fn send(&self, len: usize) {
        let shared = &self.shared;
        let packet = &self.packet[..len];
        let mut sent = 0u64;
        match shared.delivery() {
            Delivery::Unicast => {
                for slot in shared.peers.slots() {
                    if !slot.is_active() {
                        continue;
                    }
                    if let Some(addr) = unpack_addr(slot.addr.load(Ordering::Relaxed)) {
                        match self.socket.send_to(packet, addr) {
                            Ok(_) => sent += 1,
                            Err(_) => {
                                shared.stats.send_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
            Delivery::Broadcast => {
                for target in shared.broadcast_targets.iter() {
                    if let Some(addr) = unpack_addr(target.load(Ordering::Relaxed)) {
                        match self.socket.send_to(packet, addr) {
                            Ok(_) => sent += 1,
                            Err(_) => {
                                shared.stats.send_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        }
        if sent > 0 {
            shared.stats.packets_sent.fetch_add(sent, Ordering::Relaxed);
            shared
                .stats
                .bytes_sent
                .fetch_add(sent * len as u64, Ordering::Relaxed);
        }
    }
}
