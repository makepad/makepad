//! The link's UDP sender, called from the application's audio input callback.
//!
//! [`Framer`] folds device channels to mono, applies input gain, resamples
//! to [`INTERNAL_RATE`], and frames, gates, fades and limits the samples.
//! [`CaptureHandle`] encodes each finished frame and sends it directly to
//! the link's destinations as a single datagram.
//!
//! A closed gate or muted link sends a header-only silence packet with the
//! frame's sequence number, sample count and timestamp.

use crate::codec::{adpcm_encode_into, ogg_page_into, AdpcmState};
use crate::framer::{interleaved_samples, planar_samples, Frame, Framer, FramerConfig};
use crate::link::{Delivery, Shared};
use crate::peers::unpack_addr;
use crate::wire::{
    encode_header_only, encode_raw_i16, flags, Codec, Header, HEADER_LEN, INTERNAL_RATE, MAX_PACKET,
};
use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// The capture half of a [`crate::VoiceLink`]. `Send` (move it into the audio
/// input callback), not `Clone`: exactly one thread feeds it.
pub struct CaptureHandle {
    shared: Arc<Shared>,
    framer: Framer,
    sender: PacketSender,
}

struct PacketSender {
    socket: UdpSocket,
    packet: [u8; MAX_PACKET],
    ogg_serial: u32,
    ogg_granule: u64,
    adpcm_state: AdpcmState,
    adpcm_buf: Vec<u8>,
    page_buf: Vec<u8>,
}

impl CaptureHandle {
    pub(crate) fn new(shared: Arc<Shared>, socket: UdpSocket) -> Self {
        let framer = Framer::new(FramerConfig {
            out_rate: INTERNAL_RATE,
            frame_samples: shared.frame_samples,
            gate_threshold_rms: shared.gate_threshold_rms,
            gate_hangover_ms: shared.gate_hangover_ms,
        })
        .expect("VoiceLink::bind validates the frame size");
        let ogg_serial = shared.sender_id.load(Ordering::Relaxed) as u32;
        let mut page_buf = Vec::with_capacity(MAX_PACKET);
        // Initialize the codec's shared CRC table before the audio callback.
        ogg_page_into(ogg_serial, 0, 0, &[], &mut page_buf);
        let sender = PacketSender {
            socket,
            packet: [0; MAX_PACKET],
            ogg_serial,
            ogg_granule: 0,
            adpcm_state: AdpcmState::default(),
            // Eight bytes of state followed by at most four bits per sample.
            adpcm_buf: Vec::with_capacity(8 + shared.frame_samples.div_ceil(2)),
            page_buf,
        };
        Self {
            shared,
            framer,
            sender,
        }
    }

    /// Feed a mono block at `rate` Hz.
    pub fn push_mono(&mut self, rate: f64, samples: &[f32]) {
        self.push(rate, samples.iter().copied());
    }

    /// Feed a planar (channel-major) block, as
    /// `makepad_platform::audio::AudioBuffer` stores it: `data.len()` must be
    /// at least `frames * channels`. Channels are averaged to mono.
    pub fn push_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &[f32]) {
        self.push(rate, planar_samples(frames, channels, data));
    }

    /// Feed an interleaved block. Channels are averaged to mono.
    pub fn push_interleaved(&mut self, rate: f64, channels: usize, data: &[f32]) {
        self.push(rate, interleaved_samples(channels, data));
    }

    /// Frames sent so far (audio and silence).
    pub fn frames_sent(&self) -> u32 {
        self.framer.frames_out()
    }

    fn push(&mut self, rate: f64, samples: impl ExactSizeIterator<Item = f32>) {
        let shared = &*self.shared;
        self.framer.push_with_controls(
            rate,
            samples,
            0,
            || f32::from_bits(shared.input_gain.load(Ordering::Relaxed)),
            || shared.muted.load(Ordering::Relaxed),
            &mut |frame| self.sender.send_frame(shared, frame),
        );
    }
}

impl PacketSender {
    fn send_frame(&mut self, shared: &Shared, frame: &Frame<'_>) {
        let mut header = Header {
            codec: Codec::RawI16,
            channel: shared.channel.load(Ordering::Relaxed),
            flags: if frame.talk_start {
                flags::TALK_START
            } else {
                0
            },
            frames: shared.frame_samples as u16,
            room: shared.room.load(Ordering::Relaxed),
            sender: shared.sender_id.load(Ordering::Relaxed),
            seq: frame.seq,
            timestamp: frame.timestamp,
        };
        let len = if frame.silent {
            header.flags = flags::SILENCE;
            encode_header_only(header, &mut self.packet)
        } else if Codec::from_u8(shared.wire_codec.load(Ordering::Relaxed)) == Some(Codec::Ogg) {
            let bits = shared.adpcm_bits.load(Ordering::Relaxed);
            adpcm_encode_into(
                &mut self.adpcm_state,
                bits,
                frame.samples,
                &mut self.adpcm_buf,
            );
            self.ogg_granule += frame.samples.len() as u64;
            ogg_page_into(
                self.ogg_serial,
                frame.seq,
                self.ogg_granule,
                &self.adpcm_buf,
                &mut self.page_buf,
            );
            header.codec = Codec::Ogg;
            header.write(&mut self.packet[..HEADER_LEN]);
            let len = HEADER_LEN + self.page_buf.len();
            self.packet[HEADER_LEN..len].copy_from_slice(&self.page_buf);
            len
        } else {
            encode_raw_i16(header, frame.samples, &mut self.packet)
        };
        self.send(shared, len);
    }

    fn send(&self, shared: &Shared, len: usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{VoiceCodec, VoiceEncoder};
    use crate::{VoiceConfig, VoiceLink};
    use std::net::Ipv4Addr;
    use std::time::Duration;

    #[test]
    fn framer_matches_capture_datagrams_at_the_link_rate() {
        let config = VoiceConfig {
            port: 0,
            broadcast: false,
            delivery: Delivery::Broadcast,
            gate_hangover_ms: 5,
            gate_threshold_rms: 0.03,
            ..VoiceConfig::default()
        };
        let mut framer = Framer::new(FramerConfig {
            out_rate: INTERNAL_RATE,
            frame_samples: config.frame_samples,
            gate_hangover_ms: config.gate_hangover_ms,
            gate_threshold_rms: config.gate_threshold_rms,
        })
        .unwrap();
        let mut link = VoiceLink::bind(config).unwrap();
        let mut capture = link.take_capture().unwrap();
        // Route just the audio datagrams to the reader, independently of discovery.
        let reader = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        capture.shared.broadcast_targets[0].store(
            crate::peers::pack_addr(reader.local_addr().unwrap()),
            Ordering::Relaxed,
        );
        link.set_input_gain(2.0);
        framer.set_input_gain(2.0);
        let mut stereo = [0.0; 480];
        let mut ogg = VoiceEncoder::new(VoiceCodec::Ogg, 4, link.sender_id() as u32);
        let blocks = [
            (0.0, false, Codec::RawI16),
            (1.1, false, Codec::RawI16),
            (1.1, false, Codec::Ogg),
            (0.0, false, Codec::RawI16),
            (0.0, false, Codec::RawI16),
            (0.0, false, Codec::RawI16),
            (0.0, false, Codec::Ogg),
            (0.2, false, Codec::Ogg),
            (0.2, true, Codec::Ogg),
        ];
        for (amplitude, muted, codec) in blocks {
            link.set_codec(codec);
            for (i, pair) in stereo.chunks_exact_mut(2).enumerate() {
                pair[0] = amplitude * ((i as f32) * 0.07).cos();
                pair[1] = 0.5 * pair[0];
            }
            link.set_muted(muted);
            framer.set_muted(muted);
            capture.push_interleaved(INTERNAL_RATE, 2, &stereo);
            let block_us = 3_000_000 + u64::from(framer.frames_out()) * 5_000;
            framer.push_interleaved(INTERNAL_RATE, 2, &stereo, block_us, &mut |frame| {
                assert_eq!(frame.capture_us, block_us);
                let header = Header {
                    codec: if frame.silent { Codec::RawI16 } else { codec },
                    channel: 0,
                    flags: if frame.silent {
                        flags::SILENCE
                    } else if frame.talk_start {
                        flags::TALK_START
                    } else {
                        0
                    },
                    frames: 240,
                    room: 0,
                    sender: link.sender_id(),
                    seq: frame.seq,
                    timestamp: frame.timestamp,
                };
                let mut expected = [0; MAX_PACKET];
                let len = if frame.silent {
                    encode_header_only(header, &mut expected)
                } else if codec == Codec::Ogg {
                    let page = ogg.encode(frame.seq, frame.samples);
                    header.write(&mut expected[..HEADER_LEN]);
                    let len = HEADER_LEN + page.len();
                    expected[HEADER_LEN..len].copy_from_slice(&page);
                    len
                } else {
                    encode_raw_i16(header, frame.samples, &mut expected)
                };
                let mut received = [0; MAX_PACKET];
                let n = reader.recv(&mut received).unwrap();
                assert_eq!(&received[..n], &expected[..len], "datagram {}", frame.seq);
            });
        }
        assert_eq!(framer.frames_out() as usize, blocks.len());
        assert_eq!(capture.frames_sent(), framer.frames_out());
    }
}
