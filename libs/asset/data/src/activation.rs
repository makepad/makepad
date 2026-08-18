//! Scene/content/realm activation transaction and ticket DTOs for the
//! multiplayer content protocol.
//!
//! These are the shared payloads the World Server, clients, and Asset Server
//! agree on. Every prepare/ready/commit message carries the full transaction
//! tuple, so a delayed acknowledgement from an abandoned preparation can never
//! activate anything. The session envelope, ordering, and retransmission
//! belong to the protocol crates — only the canonical payloads live here.

use crate::codec::{dockind, CanonReader, CanonWriter};
use crate::content_set::AssetSlot;
use crate::derived::RESOLUTION_POLICY_V1;
use crate::error::AssetDataError;
use crate::id::{
    AssetRevisionRef, ContentSetId, GameRevisionId, MigrationPlanDigest, RealmResolutionDigest,
    ScenePlanDigest, TransactionId,
};
use crate::limits::*;
use crate::migration::ActivationMode;

/// Monotonic whole-world generation. Only a hard reset advances it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmEpoch(pub u64);

/// Monotonic same-realm authored-scene generation. A state-preserving scene
/// revision advances this without touching the epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneSequence(pub u64);

/// Host simulation tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(pub u64);

impl std::fmt::Display for RealmEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "epoch#{}", self.0)
    }
}
impl std::fmt::Display for SceneSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "scene#{}", self.0)
    }
}
impl std::fmt::Display for Tick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tick#{}", self.0)
    }
}

/// The canonical `{realm_epoch, scene_sequence}` tag every reliable structural
/// record, authoritative UDP entity-state batch, and client intent carries.
/// A receiver accepts only its current tuple; delayed prior-sequence traffic
/// is discarded and future-sequence traffic is held only inside the matching
/// prepared transaction.
///
/// Ordering is semantic: epoch first, then sequence, so `Ord` answers
/// "older/newer scene" directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneTag {
    pub realm_epoch: RealmEpoch,
    pub scene_sequence: SceneSequence,
}

/// Where an incoming tagged record stands relative to the receiver's current
/// scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneTagDisposition {
    /// Same tuple: process normally.
    Current,
    /// Older epoch or sequence: discard, never apply to the new scene.
    Stale,
    /// Newer tuple: hold only inside the matching prepared transaction.
    Future,
}

impl SceneTag {
    /// Compact wire form for embedding in per-record/per-packet headers where
    /// the full canonical document header would be waste. Big-endian, 16
    /// bytes, same bytes on every platform.
    pub const WIRE_LEN: usize = 16;

    pub fn to_wire_bytes(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[..8].copy_from_slice(&self.realm_epoch.0.to_be_bytes());
        out[8..].copy_from_slice(&self.scene_sequence.0.to_be_bytes());
        out
    }

    pub fn from_wire_bytes(bytes: [u8; Self::WIRE_LEN]) -> Self {
        let mut epoch = [0u8; 8];
        epoch.copy_from_slice(&bytes[..8]);
        let mut seq = [0u8; 8];
        seq.copy_from_slice(&bytes[8..]);
        Self {
            realm_epoch: RealmEpoch(u64::from_be_bytes(epoch)),
            scene_sequence: SceneSequence(u64::from_be_bytes(seq)),
        }
    }

    /// Classify an incoming record's tag against the receiver's current tag.
    pub fn classify(self, current: SceneTag) -> SceneTagDisposition {
        match self.cmp(&current) {
            std::cmp::Ordering::Equal => SceneTagDisposition::Current,
            std::cmp::Ordering::Less => SceneTagDisposition::Stale,
            std::cmp::Ordering::Greater => SceneTagDisposition::Future,
        }
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.realm_epoch.0);
        w.u64(self.scene_sequence.0);
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            realm_epoch: RealmEpoch(r.u64("scene tag epoch")?),
            scene_sequence: SceneSequence(r.u64("scene tag sequence")?),
        })
    }
}

impl std::fmt::Display for SceneTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.realm_epoch, self.scene_sequence)
    }
}

/// What a room does with a peer that misses a readiness deadline. A stalled
/// peer cannot freeze content changes indefinitely; silently entering with
/// missing content is not an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadyPeerPolicy {
    /// Keep waiting past the deadline (host explicitly accepts the stall).
    Wait,
    /// Demote the peer to a loading spectator until it catches up.
    DemoteToSpectator,
    /// Disconnect the peer.
    Disconnect,
}

crate::codec::canon_enum!(UnreadyPeerPolicy {
    Wait = 0,
    DemoteToSpectator = 1,
    Disconnect = 2,
});

/// The declared per-transaction readiness contract, carried in every prepare
/// record so peers and operations views see the same policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessPolicy {
    pub on_unready: UnreadyPeerPolicy,
    /// Bounded deadline in milliseconds; even `Wait` declares one so the
    /// stall is visible and attributable.
    pub deadline_millis: u32,
}

impl ReadinessPolicy {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.deadline_millis == 0 {
            return Err(AssetDataError::Malformed {
                what: "readiness deadline",
            });
        }
        if self.deadline_millis > MAX_READINESS_DEADLINE_MILLIS {
            return Err(AssetDataError::OverBudget {
                what: "readiness deadline",
                limit: MAX_READINESS_DEADLINE_MILLIS as u64,
                found: self.deadline_millis as u64,
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.on_unready.tag());
        w.u32(self.deadline_millis);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            on_unready: UnreadyPeerPolicy::decode(r)?,
            deadline_millis: r.u32("readiness deadline")?,
        })
    }
}

/// The room's complete content position:
/// `{realm, game_revision, scene_sequence, content_set}`. A scene revision
/// changes the latter three without changing the realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomContentTuple {
    pub realm_epoch: RealmEpoch,
    pub game_revision: GameRevisionId,
    pub scene_sequence: SceneSequence,
    pub content_set: ContentSetId,
}

/// The tuple a join/preparation is pinned to. Snapshot and readiness messages
/// repeat it exactly; a stale ticket cannot embody a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinTicket {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub game_revision: GameRevisionId,
    pub content_set: ContentSetId,
}

impl JoinTicket {
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.game_revision.encode(w);
        self.content_set.encode(w);
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            game_revision: GameRevisionId::decode(r)?,
            content_set: ContentSetId::decode(r)?,
        })
    }
}

/// A small helper so every DTO gets the same document plumbing.
macro_rules! canon_doc {
    ($ty:ident, $kind:expr) => {
        impl $ty {
            pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
                self.validate()?;
                let mut w = CanonWriter::new($kind);
                self.encode(&mut w);
                w.finish()
            }
            pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
                let mut r = CanonReader::new(bytes, $kind)?;
                let v = Self::decode(&mut r)?;
                r.finish()?;
                v.validate()?;
                Ok(v)
            }
        }
    };
}

impl RoomContentTuple {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.realm_epoch.0);
        self.game_revision.encode(w);
        w.u64(self.scene_sequence.0);
        self.content_set.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            game_revision: GameRevisionId::decode(r)?,
            scene_sequence: SceneSequence(r.u64("scene sequence")?),
            content_set: ContentSetId::decode(r)?,
        })
    }
}
canon_doc!(RoomContentTuple, dockind::ROOM_CONTENT_TUPLE);

/// Sent to an authenticated peer instead of inline source: everything needed
/// to fetch, verify, and evaluate the active content before embodiment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealmDescriptor {
    pub ticket: JoinTicket,
    /// The deterministic variant-resolution policy version every peer must
    /// run against this realm's frozen variant sets. Unknown versions refuse
    /// at the boundary — a peer never guesses a resolution algorithm.
    pub variant_policy_version: u32,
    /// Ordered Asset Server origins (URL strings). Bytes from any origin are
    /// equivalent only after digest verification.
    pub origins: Vec<String>,
    /// Short-lived read capability scoped to the active content set. Opaque.
    pub read_capability: Vec<u8>,
}

impl RealmDescriptor {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.variant_policy_version != RESOLUTION_POLICY_V1 {
            return Err(AssetDataError::UnsupportedSchema {
                found: self.variant_policy_version as u16,
            });
        }
        if self.origins.is_empty() {
            return Err(AssetDataError::Missing { what: "origins" });
        }
        if self.origins.len() > MAX_ORIGINS {
            return Err(AssetDataError::OverBudget {
                what: "origins",
                limit: MAX_ORIGINS as u64,
                found: self.origins.len() as u64,
            });
        }
        for o in &self.origins {
            if o.is_empty() || o.len() > MAX_ORIGIN_BYTES {
                return Err(AssetDataError::Malformed { what: "origin" });
            }
        }
        if self.read_capability.len() > MAX_CAPABILITY_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "read capability",
                limit: MAX_CAPABILITY_BYTES as u64,
                found: self.read_capability.len() as u64,
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.ticket.encode(w);
        w.u32(self.variant_policy_version);
        w.u32(self.origins.len() as u32);
        for o in &self.origins {
            w.str(o);
        }
        w.bytes(&self.read_capability);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let ticket = JoinTicket::decode(r)?;
        let variant_policy_version = r.u32("variant policy version")?;
        let n = r.count("origins", MAX_ORIGINS)?;
        let mut origins = Vec::with_capacity(n);
        for _ in 0..n {
            origins.push(r.str("origin", MAX_ORIGIN_BYTES)?);
        }
        let read_capability = r.bytes("read capability", MAX_CAPABILITY_BYTES)?;
        Ok(Self {
            ticket,
            variant_policy_version,
            origins,
            read_capability,
        })
    }
}
canon_doc!(RealmDescriptor, dockind::REALM_DESCRIPTOR);

/// The joining client proves it fetched, verified, and evaluated the exact
/// pinned tuple, including its exact aggregate variant resolution over every
/// pinned set. The host embodies the player only after this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinContentReady {
    pub ticket: JoinTicket,
    /// Digest of the peer's exact `RealmResolution` over the pinned content
    /// set. A ready with the wrong aggregate cannot embody.
    pub resolution: RealmResolutionDigest,
}

impl JoinContentReady {
    /// Host-side gate: the acknowledgement must name the exact pinned ticket
    /// AND the exact aggregate resolution the host computed/accepted for
    /// this peer's declared profile. A stale or wrong ready is a typed
    /// refusal, never a silent embodiment.
    pub fn verify(
        &self,
        expected_ticket: &JoinTicket,
        expected_resolution: &RealmResolutionDigest,
    ) -> Result<(), AssetDataError> {
        if self.ticket != *expected_ticket {
            return Err(AssetDataError::Mismatch {
                what: "join ready ticket",
            });
        }
        if self.resolution != *expected_resolution {
            return Err(AssetDataError::Mismatch {
                what: "join ready resolution",
            });
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.ticket.encode(w);
        self.resolution.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            ticket: JoinTicket::decode(r)?,
            resolution: RealmResolutionDigest::decode(r)?,
        })
    }
}
canon_doc!(JoinContentReady, dockind::JOIN_CONTENT_READY);

/// Stage a state-preserving scene transaction while the old scene plays on.
/// `hard_reset` never travels this path — it uses `PrepareRealm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareSceneChange {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub parent_game_revision: GameRevisionId,
    pub next_game_revision: GameRevisionId,
    pub parent_scene_sequence: SceneSequence,
    pub next_content_set: ContentSetId,
    pub scene_plan_digest: ScenePlanDigest,
    pub migration_plan_digest: MigrationPlanDigest,
    pub activation_mode: ActivationMode,
    pub proposed_activation_tick: Tick,
    pub readiness: ReadinessPolicy,
}

impl PrepareSceneChange {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.activation_mode == ActivationMode::HardReset {
            return Err(AssetDataError::Mismatch {
                what: "hard reset on scene-change path",
            });
        }
        if self.parent_game_revision == self.next_game_revision {
            return Err(AssetDataError::Mismatch {
                what: "scene change to same revision",
            });
        }
        self.readiness.validate()
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.parent_game_revision.encode(w);
        self.next_game_revision.encode(w);
        w.u64(self.parent_scene_sequence.0);
        self.next_content_set.encode(w);
        self.scene_plan_digest.encode(w);
        self.migration_plan_digest.encode(w);
        w.u8(self.activation_mode.tag());
        w.u64(self.proposed_activation_tick.0);
        self.readiness.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            parent_game_revision: GameRevisionId::decode(r)?,
            next_game_revision: GameRevisionId::decode(r)?,
            parent_scene_sequence: SceneSequence(r.u64("parent scene sequence")?),
            next_content_set: ContentSetId::decode(r)?,
            scene_plan_digest: ScenePlanDigest::decode(r)?,
            migration_plan_digest: MigrationPlanDigest::decode(r)?,
            activation_mode: ActivationMode::decode(r)?,
            proposed_activation_tick: Tick(r.u64("proposed activation tick")?),
            readiness: ReadinessPolicy::decode(r)?,
        })
    }
}
canon_doc!(PrepareSceneChange, dockind::PREPARE_SCENE_CHANGE);

/// A peer proves it staged the exact next scene tuple and resolved its exact
/// aggregate variant resolution against the next content set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneContentReady {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_game_revision: GameRevisionId,
    pub next_content_set: ContentSetId,
    pub resolution: RealmResolutionDigest,
}

impl SceneContentReady {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.next_game_revision.encode(w);
        self.next_content_set.encode(w);
        self.resolution.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            next_game_revision: GameRevisionId::decode(r)?,
            next_content_set: ContentSetId::decode(r)?,
            resolution: RealmResolutionDigest::decode(r)?,
        })
    }
}
canon_doc!(SceneContentReady, dockind::SCENE_CONTENT_READY);

/// Commit the staged scene transaction at one tick boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitSceneChange {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_game_revision: GameRevisionId,
    pub next_scene_sequence: SceneSequence,
    pub next_content_set: ContentSetId,
    pub activation_tick: Tick,
}

impl CommitSceneChange {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.next_game_revision.encode(w);
        w.u64(self.next_scene_sequence.0);
        self.next_content_set.encode(w);
        w.u64(self.activation_tick.0);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            next_game_revision: GameRevisionId::decode(r)?,
            next_scene_sequence: SceneSequence(r.u64("next scene sequence")?),
            next_content_set: ContentSetId::decode(r)?,
            activation_tick: Tick(r.u64("activation tick")?),
        })
    }
}
canon_doc!(CommitSceneChange, dockind::COMMIT_SCENE_CHANGE);

/// A peer confirms it applied the committed scene sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneApplied {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_scene_sequence: SceneSequence,
}

impl SceneApplied {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        w.u64(self.next_scene_sequence.0);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            next_scene_sequence: SceneSequence(r.u64("next scene sequence")?),
        })
    }
}
canon_doc!(SceneApplied, dockind::SCENE_APPLIED);

/// Append-only content-set extension for a live world: phase one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareContentChange {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_set: ContentSetId,
    /// The exact revisions a peer may be missing, canonical order.
    pub required_delta: Vec<AssetRevisionRef>,
    pub proposed_activation_tick: Tick,
    pub readiness: ReadinessPolicy,
}

impl PrepareContentChange {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.required_delta.is_empty() {
            return Err(AssetDataError::Missing { what: "required delta" });
        }
        if self.required_delta.len() > MAX_REQUIRED_DELTA {
            return Err(AssetDataError::OverBudget {
                what: "required delta",
                limit: MAX_REQUIRED_DELTA as u64,
                found: self.required_delta.len() as u64,
            });
        }
        crate::codec::check_sorted_unique(&self.required_delta, |d| *d, "required delta entry")?;
        self.readiness.validate()
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.next_set.encode(w);
        w.u32(self.required_delta.len() as u32);
        for d in &self.required_delta {
            d.encode(w);
        }
        w.u64(self.proposed_activation_tick.0);
        self.readiness.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let transaction_id = TransactionId::decode(r)?;
        let realm_epoch = RealmEpoch(r.u64("realm epoch")?);
        let next_set = ContentSetId::decode(r)?;
        let n = r.count("required delta", MAX_REQUIRED_DELTA)?;
        let mut required_delta = Vec::with_capacity(n);
        for _ in 0..n {
            required_delta.push(AssetRevisionRef::decode(r)?);
        }
        let proposed_activation_tick = Tick(r.u64("proposed activation tick")?);
        let readiness = ReadinessPolicy::decode(r)?;
        Ok(Self {
            transaction_id,
            realm_epoch,
            next_set,
            required_delta,
            proposed_activation_tick,
            readiness,
        })
    }
}
canon_doc!(PrepareContentChange, dockind::PREPARE_CONTENT_CHANGE);

/// A peer proves it holds the complete next content set and resolved its
/// exact aggregate variant resolution against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentChangeReady {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_set: ContentSetId,
    pub resolution: RealmResolutionDigest,
}

impl ContentChangeReady {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.next_set.encode(w);
        self.resolution.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            realm_epoch: RealmEpoch(r.u64("realm epoch")?),
            next_set: ContentSetId::decode(r)?,
            resolution: RealmResolutionDigest::decode(r)?,
        })
    }
}
canon_doc!(ContentChangeReady, dockind::CONTENT_CHANGE_READY);

/// One ordered activation: install the new slot table at the tick, then spawn.
/// Dynamic spawn slots must exist in the committed set; full descriptors ride
/// the session protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitContentChange {
    pub transaction_id: TransactionId,
    pub realm_epoch: RealmEpoch,
    pub next_set: ContentSetId,
    pub dynamic_spawn_slots: Vec<AssetSlot>,
    pub activation_tick: Tick,
}

impl CommitContentChange {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.dynamic_spawn_slots.len() > MAX_DYNAMIC_SPAWNS {
            return Err(AssetDataError::OverBudget {
                what: "dynamic spawns",
                limit: MAX_DYNAMIC_SPAWNS as u64,
                found: self.dynamic_spawn_slots.len() as u64,
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.realm_epoch.0);
        self.next_set.encode(w);
        w.u32(self.dynamic_spawn_slots.len() as u32);
        for s in &self.dynamic_spawn_slots {
            w.u32(s.0);
        }
        w.u64(self.activation_tick.0);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let transaction_id = TransactionId::decode(r)?;
        let realm_epoch = RealmEpoch(r.u64("realm epoch")?);
        let next_set = ContentSetId::decode(r)?;
        let n = r.count("dynamic spawns", MAX_DYNAMIC_SPAWNS)?;
        let mut dynamic_spawn_slots = Vec::with_capacity(n);
        for _ in 0..n {
            dynamic_spawn_slots.push(AssetSlot(r.u32("dynamic spawn slot")?));
        }
        let activation_tick = Tick(r.u64("activation tick")?);
        Ok(Self {
            transaction_id,
            realm_epoch,
            next_set,
            dynamic_spawn_slots,
            activation_tick,
        })
    }
}
canon_doc!(CommitContentChange, dockind::COMMIT_CONTENT_CHANGE);

/// Whole-world replacement, phase one. Orthogonal to the peer's active old
/// realm: only old-realm input is accepted until commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareRealm {
    pub transaction_id: TransactionId,
    pub next_epoch: RealmEpoch,
    pub game_revision: GameRevisionId,
    pub content_set: ContentSetId,
    pub readiness: ReadinessPolicy,
}

impl PrepareRealm {
    fn validate(&self) -> Result<(), AssetDataError> {
        self.readiness.validate()
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.next_epoch.0);
        self.game_revision.encode(w);
        self.content_set.encode(w);
        self.readiness.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            next_epoch: RealmEpoch(r.u64("next epoch")?),
            game_revision: GameRevisionId::decode(r)?,
            content_set: ContentSetId::decode(r)?,
            readiness: ReadinessPolicy::decode(r)?,
        })
    }
}
canon_doc!(PrepareRealm, dockind::PREPARE_REALM);

/// Whole-world replacement, commit. Advances the epoch and installs one
/// coherent new-world snapshot; a stale prepare/ready cannot commit this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitRealm {
    pub transaction_id: TransactionId,
    pub next_epoch: RealmEpoch,
    pub game_revision: GameRevisionId,
    pub content_set: ContentSetId,
}

impl CommitRealm {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.transaction_id.encode(w);
        w.u64(self.next_epoch.0);
        self.game_revision.encode(w);
        self.content_set.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            transaction_id: TransactionId::decode(r)?,
            next_epoch: RealmEpoch(r.u64("next epoch")?),
            game_revision: GameRevisionId::decode(r)?,
            content_set: ContentSetId::decode(r)?,
        })
    }
}
canon_doc!(CommitRealm, dockind::COMMIT_REALM);

/// Why a peer cannot satisfy a join/prepare tuple. Closed codes: host policy
/// (wait, spectator, disconnect) branches on these, and substitution is never
/// one of the options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentRefusalCode {
    /// One or more required revisions/blobs could not be fetched.
    MissingContent,
    /// No allowed visual variant matches this device.
    UnsupportedVariant,
    /// Engine/protocol compatibility declared by the revision is not met.
    UnsupportedEngine,
    /// Local cache/download quota or admission budget exceeded.
    QuotaExceeded,
    /// The fetched content failed digest or admission validation.
    ValidationFailed,
    /// Staging evaluation of the Splash source failed locally.
    EvaluationFailed,
    /// The readiness deadline passed before the peer completed.
    DeadlineExpired,
    /// The peer's pinned ticket no longer matches the transaction.
    TicketMismatch,
}

crate::codec::canon_enum!(ContentRefusalCode {
    MissingContent = 0,
    UnsupportedVariant = 1,
    UnsupportedEngine = 2,
    QuotaExceeded = 3,
    ValidationFailed = 4,
    EvaluationFailed = 5,
    DeadlineExpired = 6,
    TicketMismatch = 7,
});

/// Structured refusal for a join or prepare transaction, pinned to the exact
/// ticket it refuses so it can never be misread against a later preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRefusal {
    pub code: ContentRefusalCode,
    pub ticket: JoinTicket,
    /// For `MissingContent`/`UnsupportedVariant`: the first offending exact
    /// revisions, canonical order, truncated at the budget.
    pub missing: Vec<AssetRevisionRef>,
    /// True when `missing` was truncated to its budget.
    pub missing_truncated: bool,
    /// Bounded local diagnostic. Never authority, never substituted content.
    pub detail: String,
}

impl ContentRefusal {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.missing.len() > MAX_REFUSAL_MISSING {
            return Err(AssetDataError::OverBudget {
                what: "refusal missing list",
                limit: MAX_REFUSAL_MISSING as u64,
                found: self.missing.len() as u64,
            });
        }
        let carries_missing = matches!(
            self.code,
            ContentRefusalCode::MissingContent | ContentRefusalCode::UnsupportedVariant
        );
        if !carries_missing && (!self.missing.is_empty() || self.missing_truncated) {
            return Err(AssetDataError::Mismatch {
                what: "missing list on non-content refusal",
            });
        }
        crate::codec::check_sorted_unique(&self.missing, |m| *m, "refusal missing entry")?;
        if self.detail.len() > MAX_REFUSAL_DETAIL_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "refusal detail",
                limit: MAX_REFUSAL_DETAIL_BYTES as u64,
                found: self.detail.len() as u64,
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.code.tag());
        self.ticket.encode(w);
        w.u32(self.missing.len() as u32);
        for m in &self.missing {
            m.encode(w);
        }
        w.bool(self.missing_truncated);
        w.str(&self.detail);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let code = ContentRefusalCode::decode(r)?;
        let ticket = JoinTicket::decode(r)?;
        let n = r.count("refusal missing list", MAX_REFUSAL_MISSING)?;
        let mut missing = Vec::with_capacity(n);
        for _ in 0..n {
            missing.push(AssetRevisionRef::decode(r)?);
        }
        let missing_truncated = r.bool("refusal missing truncated")?;
        let detail = r.str("refusal detail", MAX_REFUSAL_DETAIL_BYTES)?;
        Ok(Self {
            code,
            ticket,
            missing,
            missing_truncated,
            detail,
        })
    }
}
canon_doc!(ContentRefusal, dockind::CONTENT_REFUSAL);
