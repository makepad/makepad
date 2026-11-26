//! A call of several people over encrypted datagram links: voice and video framing, loss
//! recovery and feedback. The core is sans-IO: it takes bytes and the time and gives bytes to send
//! and events. [`driver`] runs it on worker threads with codecs and the audio callbacks, behind a
//! handle the UI thread holds; the caller still owns the links and the devices.
//!
//! - [`driver`]: a running call: its workers, the audio callbacks' halves, the codecs, synthetic sources.
//! - [`wire`]: the bytes of each datagram (clear header, voice frames, video fragments, feedback).
//! - [`video`]: access units over datagrams: fragments, resends, key frames, reassembly.
//! - [`session`]: one participant's media with one other: sealing, hops, feedback, events.
//! - [`mesh`]: one participant's call with everyone else in it, a peer per participant.
//! - [`rate`]: the sender's path rate estimate from arrival times and settled losses.
//! - [`guard`]: capture decisions and encoder settings for the sender's video budget.
//! - [`sim`]: a simulated network path on a virtual clock, for tests.

pub mod driver;
pub mod guard;
pub mod mesh;
pub mod rate;
pub mod session;
pub mod sim;
pub mod video;
pub mod wire;
