//! Host-authoritative LAN transport for Makepad Arcade (see repo-root game.md).
//!
//! The trust model is one-way and structural: a [`Host`] simulates and owns all
//! state, [`Client`]s send input and intent. There is no per-object authority
//! and no takeover handshake to subvert. Every datagram and frame is
//! authenticated with a lobby HMAC before it can touch peer state.
//!
//! Endpoints are pumped rather than threaded, so a full session runs inside one
//! test process on a virtual clock — see `tests/`.

pub mod auth;
pub mod endpoint;
pub mod protocol;

pub use auth::LobbyKey;
pub use endpoint::{
    Client, ClientEvent, Host, HostConfig, HostEvent, NetStats, HANDSHAKE_DEADLINE, MAX_PENDING_CONNECTIONS,
    MAX_PLAYERS, PEER_TIMEOUT,
};
pub use protocol::{
    batch_entities, Announce, ClientToHost, CoeditChange, CoeditRefusal, CoeditRequest,
    CoeditResponse, EntityState, Envelope, FrameCodec, GameEvent, HostId,
    HostToClient,
    InputFrame, Intent, LeaveReason, PacketClass, PlayerId, MAX_COEDIT_LEASE_TTL,
    MAX_COEDIT_REGION, MAX_COEDIT_SOURCE, MAX_DATAGRAM_PAYLOAD, MAX_FRAME_BYTES,
    PROTOCOL_VERSION,
};
