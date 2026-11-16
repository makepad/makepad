//! The receiving end of a WebRTC call: a browser (a phone's camera) sends
//! H.264 video to this machine, which answers as an ICE-lite peer on one UDP
//! port and hands the app whole access units. Nothing is sent back but what
//! the protocol needs (STUN answers, the DTLS handshake, RTCP feedback).
//!
//! Every piece but `calls` is sans-IO: it takes bytes and the time and answers bytes to
//! send and events; the caller owns the socket and the thread.
//!
//! - `stun`: STUN binding requests and answers (RFC 5389), with
//!   MESSAGE-INTEGRITY and FINGERPRINT; what ICE-lite needs (RFC 8445).
//! - `srtp`: SRTP and SRTCP with AES-128 counter mode and HMAC-SHA1-80
//!   (RFC 3711): key derivation, rollover counter, replay window.
//! - `sdp`: the browser's offer read, and the answer written.
//! - `rtp`: RTP packets in order, H.264 depacketized (RFC 6184: single NAL
//!   units, STAP-A, FU-A) into Annex-B access units.
//! - `rtcp`: receiver reports, NACK and PLI (RFC 4585), REMB.
//! - `dtls`: the DTLS-SRTP handshake (RFC 5764) on the system's OpenSSL,
//!   the peer known by its SDP fingerprint; it gives the SRTP keys.
//! - `crypto`: HMAC-SHA1 and AES-128-CTR from the system's libcrypto.
//! - `peer`: one browser's call put together: its offer answered, its
//!   checks, handshake and packets taken, its pictures and our feedback out.
//! - `calls`: every call on one UDP port, run by one worker thread; the
//!   one piece that does IO.
//! - `browser` (tests, and the `browser` feature): a phone browser's end of
//!   a call, for tests of the apps that take calls.
//!
//! Linux only for everything that touches the crypto; the SDP and RTP
//! pieces build everywhere.

#[cfg(all(target_os = "linux", any(test, feature = "browser")))]
pub mod browser;
#[cfg(target_os = "linux")]
pub mod calls;
#[cfg(target_os = "linux")]
pub mod crypto;
#[cfg(target_os = "linux")]
pub mod dtls;
#[cfg(target_os = "linux")]
pub mod peer;
pub mod rtcp;
pub mod rtp;
pub mod sdp;
#[cfg(target_os = "linux")]
pub mod srtp;
#[cfg(target_os = "linux")]
pub mod stun;
