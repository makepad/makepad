//! # makepad-teamtalk
//!
//! Extremely low-latency LAN voice chat, in safe Rust with no dependencies.
//!
//! The design goal is a wired-LAN "helicopter headset": total mouth-to-ear
//! delay dominated by the sound device's own block sizes, with the network
//! part adding ~5-10 ms. To get there the transport sends small mono frames
//! (5 ms by default) of raw 16-bit PCM at 48 kHz over UDP, receives each
//! peer through a small lock-free reorder ring with an adaptive target of a
//! frame or two, conceals losses, and corrects clock drift with an inaudible
//! playback-rate nudge instead of ever letting buffers grow.
//!
//! ```no_run
//! use makepad_teamtalk::{VoiceLink, VoiceConfig};
//!
//! let mut link = VoiceLink::bind(VoiceConfig::default())?;
//! let mut capture = link.take_capture().unwrap();
//! let mut playback = link.take_playback().unwrap();
//!
//! // In the audio input callback (any rate / channel count / block size):
//! // capture.push_planar(info.sample_rate, buf.frame_count(),
//! //                     buf.channel_count(), &buf.data);
//!
//! // In the audio output callback, after the game mix:
//! // playback.mix_into_planar(info.sample_rate, out.frame_count(),
//! //                          out.channel_count(), &mut out.data);
//!
//! // Control, from any thread:
//! link.set_channel(1);                  // talk to team 1
//! link.set_listen_channels(&[1]);       // hear team 1 (and channel 0)
//! link.set_output_gain(0.8);            // "others" volume
//! for peer in link.peers() {
//!     link.set_peer_gain(peer.sender, 1.0);
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! For 3D-positioned voices render peers separately instead of mixing:
//! [`PlaybackHandle::render_peers`] hands each talker's mono block to the
//! caller together with the packet `sender` id, which the application
//! controls ([`VoiceConfig::sender_id`] / [`VoiceLink::set_sender_id`]) and
//! can therefore map straight to a player entity.
//!
//! Both audio-side handles are allocation-free and lock-free: safe to call
//! from real-time audio callbacks. The one syscall on the capture path is
//! the UDP send itself.
//!
//! The UDP port is fixed at [`DEFAULT_PORT`] (41531) so LAN firewall rules
//! can whitelist it once; co-located instances fall back to the next few
//! ports and still find each other. The wire header carries a codec id so a
//! compressed payload (Ogg) can ride the same transport later, and a
//! `channel` byte for team filtering at the receiver.

#![forbid(unsafe_code)]

pub mod codec;
pub mod dsp;
pub mod jitter;
pub mod link;
pub mod resample;
pub mod wire;

mod capture;
mod peers;
mod playback;

pub use capture::CaptureHandle;
pub use jitter::PlayoutConfig;
pub use link::{Delivery, LinkStats, PeerInfo, VoiceConfig, VoiceLink};
pub use playback::{PeerVoice, PlaybackHandle};
pub use wire::{Codec, HOST_SENDER_ID, INTERNAL_RATE, MAX_FRAME};

/// The well-known UDP port: keep it fixed, firewalls whitelist it.
pub const DEFAULT_PORT: u16 = 41531;
