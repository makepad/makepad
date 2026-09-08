# makepad-teamtalk

Extremely low-latency LAN voice chat core. Safe Rust (`#![forbid(unsafe_code)]`),
no dependencies, no makepad coupling — the app shell in `examples/teamtalk`
shows the makepad wiring, `examples/teamtalk/REVIEW.md` has the measured
latency budget and design rationale.

- **Wire**: UDP port **41531** (fixed; co-located instances fall back to
  41532…41539 and still meet). 24-byte header: codec id (`raw_i16` now,
  `ogg` reserved), team **channel** byte (0 = everyone), flags
  (SILENCE/HELLO/TALK_START/BYE), frame count, **u64 sender id** (the app's
  identity for the talker), sequence, sample timestamp. Mono 48 kHz frames,
  5 ms default (2.5–20 ms configurable).
- **Discovery**: HELLO broadcast 2/s + static peers; audio unicast to each
  discovered peer (default) or subnet broadcast. Peers expire after 3 s;
  BYE on drop. DTX: header-only silence packets keep presence, sequence and
  timing at 4.8 kB/s while the gate is closed.
- **Receive**: per-peer lock-free reorder ring → adaptive playout (start
  10 ms, floor-guarded decay), loss concealment (repeat-fade + fade-in),
  drift correction by inaudible playback-rate nudge (≤ ±0.5 %). Idle peers
  cost nothing.
- **Audio-thread contract**: `CaptureHandle` and `PlaybackHandle` are
  allocation-free and lock-free; the capture path's only syscall is the UDP
  send itself. Resampling (stateful Hermite) at both edges takes any device
  rate/block/channel-count.

```rust
use makepad_teamtalk::{VoiceLink, VoiceConfig};

let mut link = VoiceLink::bind(VoiceConfig::default())?;
let mut capture = link.take_capture().unwrap();   // -> audio input callback
let mut playback = link.take_playback().unwrap(); // -> audio output callback

// input callback:  capture.push_planar(rate, frames, channels, &buf.data);
// output callback: playback.mix_into_planar(rate, frames, channels, &mut buf.data);
// or per-peer for 3D voice:
// playback.render_peers(rate, frames, |voice| spatialise(voice.sender, voice.samples));

link.set_channel(1);                // talk on team 1 (0 = everyone)
link.set_listen_channels(&[1]);     // hear team 1 (channel 0 always plays)
link.set_output_gain(0.8);          // "others" volume
link.set_input_gain(1.0);           // mic volume
link.set_muted(false);              // push-to-talk = set_muted(!held)
for p in link.peers() { link.set_peer_gain(p.sender, 1.0); }
```

Test/bench: `cargo test -p makepad-teamtalk` (36 tests, no LAN port use);
`cargo run -p makepad-example-teamtalk --bin teamtalk-bench --release` for
the loopback latency measurement.
