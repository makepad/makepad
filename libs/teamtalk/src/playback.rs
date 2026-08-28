//! The receive side: called from the application's audio *output* callback.
//!
//! [`PlaybackHandle::mix_into_planar`] (or the mono/interleaved variants)
//! renders every audible peer through its jitter buffer and resampler and
//! adds the mix into the device block. [`PlaybackHandle::render_peers`] hands
//! each peer's block out separately instead — for a game that spatialises
//! voices itself, keyed by the packet's `sender` id. No allocation, no locks;
//! idle peers (silent for 200 ms) cost nothing.

use crate::jitter::Playout;
use crate::link::Shared;
use crate::peers::{PeerSlot, MAX_PEERS};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Longest run rendered at once; longer device blocks are processed in
/// chunks of this many frames.
const MAX_BLOCK: usize = 4096;

/// One peer's rendered audio for [`PlaybackHandle::render_peers`].
pub struct PeerVoice<'a> {
    /// The sender id from the packets (the app's identity for the talker).
    pub sender: u64,
    /// The team channel the peer is currently sending on.
    pub channel: u8,
    /// Peer slot index (stable while the peer stays active).
    pub slot: usize,
    /// Mono samples at the device rate, per-peer gain already applied.
    pub samples: &'a [f32],
}

struct PeerRender {
    generation: u32,
    playout: Playout,
    smoothed_gain: f32,
}

/// The playback half of a [`crate::VoiceLink`]. `Send` (move it into the
/// audio output callback), not `Clone`: exactly one thread drains it.
pub struct PlaybackHandle {
    shared: Arc<Shared>,
    peers: Vec<PeerRender>,
    scratch: Vec<f32>,
    master_gain: f32,
}

impl PlaybackHandle {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        let playout_cfg = shared.playout;
        Self {
            shared,
            peers: (0..MAX_PEERS)
                .map(|_| PeerRender {
                    generation: 0,
                    playout: Playout::new(playout_cfg),
                    smoothed_gain: 1.0,
                })
                .collect(),
            scratch: vec![0.0; MAX_BLOCK],
            master_gain: 1.0,
        }
    }

    /// Render every audible peer and hand each one's mono block (device
    /// rate, peer gain applied, master gain not) to `f`.
    pub fn render_peers(&mut self, rate: f64, frames: usize, mut f: impl FnMut(PeerVoice<'_>)) {
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.render_chunk(rate, n, &mut f);
            done += n;
        }
    }

    fn render_chunk(&mut self, rate: f64, n: usize, f: &mut impl FnMut(PeerVoice<'_>)) {
        let shared = self.shared.clone();
        for (i, slot) in shared.peers.slots().iter().enumerate() {
            if !slot.is_active() {
                continue;
            }
            let render = &mut self.peers[i];
            let generation = slot.generation.load(Ordering::Acquire);
            if render.generation != generation {
                render.generation = generation;
                render.playout.reset();
                render.smoothed_gain = f32::from_bits(slot.gain.load(Ordering::Relaxed));
            }
            if slot.muted.load(Ordering::Relaxed) {
                slot.talking.store(false, Ordering::Relaxed);
                continue;
            }
            if !render.playout.wants_render(&slot.ring) {
                Self::publish(slot, render);
                continue;
            }
            render.playout.render(&slot.ring, rate, &mut self.scratch[..n]);
            // Per-peer gain, ramped across the block against zipper noise.
            let target = f32::from_bits(slot.gain.load(Ordering::Relaxed));
            let start = render.smoothed_gain;
            if (start - target).abs() > 1e-6 {
                let step = (target - start) / n as f32;
                for (k, v) in self.scratch[..n].iter_mut().enumerate() {
                    *v *= start + step * (k as f32 + 1.0);
                }
                render.smoothed_gain = target;
            } else if (target - 1.0).abs() > 1e-6 {
                for v in self.scratch[..n].iter_mut() {
                    *v *= target;
                }
            }
            Self::publish(slot, render);
            f(PeerVoice {
                sender: slot.sender.load(Ordering::Relaxed),
                channel: slot.channel.load(Ordering::Relaxed),
                slot: i,
                samples: &self.scratch[..n],
            });
        }
    }

    fn publish(slot: &PeerSlot, render: &PeerRender) {
        slot.talking
            .store(render.playout.is_talking(), Ordering::Relaxed);
        slot.buffered_ms_x10.store(
            (render.playout.buffered_ms(&slot.ring) * 10.0) as u32,
            Ordering::Relaxed,
        );
        slot.target_frames
            .store(render.playout.target_frames(), Ordering::Relaxed);
    }

    /// Add the voice mix (all audible peers, master gain applied) into a
    /// mono block at the device rate.
    pub fn mix_into_mono(&mut self, rate: f64, out: &mut [f32]) {
        let master = self.master();
        let frames = out.len();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            // Rust: split so the closure may borrow `out` while self renders
            // into scratch.
            let (_, tail) = out.split_at_mut(done);
            let dst = &mut tail[..n];
            self.render_chunk(rate, n, &mut |voice: PeerVoice<'_>| {
                for (o, &s) in dst.iter_mut().zip(voice.samples) {
                    *o += s * master;
                }
            });
            done += n;
        }
    }

    /// Add the voice mix into a planar block
    /// (`makepad_platform::audio::AudioBuffer` layout: `data.len() >=
    /// frames * channels`, channel-major). The mono mix goes to every channel.
    pub fn mix_into_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &mut [f32]) {
        let channels = channels.max(1);
        if data.len() < frames * channels {
            return;
        }
        let master = self.master();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.render_chunk(rate, n, &mut |voice: PeerVoice<'_>| {
                for c in 0..channels {
                    let base = c * frames + done;
                    for (k, &s) in voice.samples.iter().enumerate() {
                        data[base + k] += s * master;
                    }
                }
            });
            done += n;
        }
    }

    /// Add the voice mix into an interleaved block.
    pub fn mix_into_interleaved(&mut self, rate: f64, channels: usize, data: &mut [f32]) {
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let master = self.master();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.render_chunk(rate, n, &mut |voice: PeerVoice<'_>| {
                for (k, &s) in voice.samples.iter().enumerate() {
                    let base = (done + k) * channels;
                    for c in 0..channels {
                        data[base + c] += s * master;
                    }
                }
            });
            done += n;
        }
    }

    fn master(&mut self) -> f32 {
        // One smoothed step per callback is enough: blocks are 1-20 ms.
        let target = f32::from_bits(self.shared.output_gain.load(Ordering::Relaxed));
        self.master_gain += (target - self.master_gain) * 0.3;
        self.master_gain
    }
}
