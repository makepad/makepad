//! Chunked snapshot framing for asset-aware joining.
//!
//! After content readiness the host streams one coherent live snapshot as
//! bounded typed section chunks. The framing here is generic and
//! deterministic: payload bytes are opaque to this crate — the session layer
//! maps its actual records into them — but the identity (snapshot ID plus
//! full join ticket), the scene tag, the per-section counts, chunk ordering,
//! byte budgets, and the end digest are all fixed contract, so the World
//! Server and clients verify exactly the same things. A client adopts a
//! snapshot atomically at `SnapshotEnd` and never draws half of one; a stale
//! `SnapshotReady` can never embody a player.

use crate::activation::{JoinTicket, SceneTag};
use crate::codec::{canon_enum, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::id::{RealmResolutionDigest, SnapshotDigest};
use crate::limits::*;
use crate::sha256::Sha256;

/// Host-issued identity of one snapshot capture. Monotonic per connection;
/// meaningless across hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(pub u64);

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "snapshot#{}", self.0)
    }
}

/// The authoritative late-join tiers, in their fixed transfer order. Future
/// authoritative systems become new typed sections (and a schema bump), never
/// a global dirty broadcast after join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapshotSection {
    /// Reliable entity descriptors (create/configure records).
    Descriptor,
    /// Kits/shared systems: health, loadouts, mounts, race state.
    KitState,
    /// Volatile entity state at the captured tick.
    EntityState,
    /// Player-to-body and seat/mount identity.
    PlayerBodyMount,
    /// Terrain structure.
    TerrainStructure,
    /// Terrain/voxel chunk state.
    TerrainVoxel,
}

canon_enum!(SnapshotSection {
    Descriptor = 0,
    KitState = 1,
    EntityState = 2,
    PlayerBodyMount = 3,
    TerrainStructure = 4,
    TerrainVoxel = 5,
});

impl SnapshotSection {
    pub const ALL: [SnapshotSection; 6] = [
        SnapshotSection::Descriptor,
        SnapshotSection::KitState,
        SnapshotSection::EntityState,
        SnapshotSection::PlayerBodyMount,
        SnapshotSection::TerrainStructure,
        SnapshotSection::TerrainVoxel,
    ];
}

/// Declared record count per section, fixed at `SnapshotBegin` so the client
/// knows completeness without trusting the stream to end politely.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotCounts {
    pub descriptor: u32,
    pub kit_state: u32,
    pub entity_state: u32,
    pub player_body_mount: u32,
    pub terrain_structure: u32,
    pub terrain_voxel: u32,
}

impl SnapshotCounts {
    pub fn of(&self, section: SnapshotSection) -> u32 {
        match section {
            SnapshotSection::Descriptor => self.descriptor,
            SnapshotSection::KitState => self.kit_state,
            SnapshotSection::EntityState => self.entity_state,
            SnapshotSection::PlayerBodyMount => self.player_body_mount,
            SnapshotSection::TerrainStructure => self.terrain_structure,
            SnapshotSection::TerrainVoxel => self.terrain_voxel,
        }
    }

    pub fn total(&self) -> u64 {
        SnapshotSection::ALL
            .iter()
            .map(|&s| self.of(s) as u64)
            .sum()
    }

    fn validate(&self) -> Result<(), AssetDataError> {
        for section in SnapshotSection::ALL {
            if self.of(section) > MAX_SNAPSHOT_RECORDS_PER_SECTION {
                return Err(AssetDataError::OverBudget {
                    what: "snapshot section records",
                    limit: MAX_SNAPSHOT_RECORDS_PER_SECTION as u64,
                    found: self.of(section) as u64,
                });
            }
        }
        Ok(())
    }

    fn encode(&self, w: &mut CanonWriter) {
        w.u32(self.descriptor);
        w.u32(self.kit_state);
        w.u32(self.entity_state);
        w.u32(self.player_body_mount);
        w.u32(self.terrain_structure);
        w.u32(self.terrain_voxel);
    }

    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            descriptor: r.u32("snapshot counts")?,
            kit_state: r.u32("snapshot counts")?,
            entity_state: r.u32("snapshot counts")?,
            player_body_mount: r.u32("snapshot counts")?,
            terrain_structure: r.u32("snapshot counts")?,
            terrain_voxel: r.u32("snapshot counts")?,
        })
    }
}

/// Opens one snapshot transfer, repeating the exact join ticket it serves.
/// If the room tuple changes mid-transfer, the host aborts and begins a new
/// snapshot; the old ID can never complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotBegin {
    pub snapshot_id: SnapshotId,
    pub ticket: JoinTicket,
    pub scene: SceneTag,
    pub snapshot_tick: crate::activation::Tick,
    pub counts: SnapshotCounts,
}

impl SnapshotBegin {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.scene.realm_epoch != self.ticket.realm_epoch {
            return Err(AssetDataError::Mismatch {
                what: "snapshot scene epoch vs ticket",
            });
        }
        self.counts.validate()
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.snapshot_id.0);
        self.ticket.encode(w);
        self.scene.encode(w);
        w.u64(self.snapshot_tick.0);
        self.counts.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            snapshot_id: SnapshotId(r.u64("snapshot id")?),
            ticket: JoinTicket::decode(r)?,
            scene: SceneTag::decode(r)?,
            snapshot_tick: crate::activation::Tick(r.u64("snapshot tick")?),
            counts: SnapshotCounts::decode(r)?,
        })
    }
}

/// One bounded slice of one section's records. Records are contiguous:
/// `first_record` is the index of the first record carried, and chunks of a
/// section arrive in order after all chunks of earlier sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotChunk {
    pub snapshot_id: SnapshotId,
    pub scene: SceneTag,
    pub section: SnapshotSection,
    pub first_record: u32,
    pub record_count: u32,
    /// Opaque record bytes; the record encoding is the session protocol's
    /// contract, the framing/budget is this crate's.
    pub payload: Vec<u8>,
}

impl SnapshotChunk {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.record_count == 0 {
            return Err(AssetDataError::Malformed {
                what: "snapshot chunk record_count",
            });
        }
        if self.payload.is_empty() {
            return Err(AssetDataError::Malformed {
                what: "snapshot chunk payload",
            });
        }
        if self.payload.len() > MAX_SNAPSHOT_CHUNK_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "snapshot chunk payload",
                limit: MAX_SNAPSHOT_CHUNK_BYTES as u64,
                found: self.payload.len() as u64,
            });
        }
        if self.first_record.checked_add(self.record_count).is_none() {
            return Err(AssetDataError::Malformed {
                what: "snapshot chunk record range",
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.snapshot_id.0);
        self.scene.encode(w);
        w.u8(self.section.tag());
        w.u32(self.first_record);
        w.u32(self.record_count);
        w.bytes(&self.payload);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            snapshot_id: SnapshotId(r.u64("snapshot id")?),
            scene: SceneTag::decode(r)?,
            section: SnapshotSection::decode(r)?,
            first_record: r.u32("snapshot chunk first_record")?,
            record_count: r.u32("snapshot chunk record_count")?,
            payload: r.bytes("snapshot chunk payload", MAX_SNAPSHOT_CHUNK_BYTES)?,
        })
    }
}

/// Closes the transfer with the digest over the whole ordered chunk stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotEnd {
    pub snapshot_id: SnapshotId,
    pub scene: SceneTag,
    pub digest: SnapshotDigest,
}

impl SnapshotEnd {
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.snapshot_id.0);
        self.scene.encode(w);
        self.digest.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            snapshot_id: SnapshotId(r.u64("snapshot id")?),
            scene: SceneTag::decode(r)?,
            digest: SnapshotDigest::decode(r)?,
        })
    }
}

/// The client's proof of atomic installation, rechecking the full ticket and
/// its exact aggregate variant resolution. The host accepts input only after
/// this matches its current tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotReady {
    pub snapshot_id: SnapshotId,
    pub ticket: JoinTicket,
    /// The peer's acknowledged `RealmResolution` digest, repeated so a
    /// snapshot completed against an abandoned resolution cannot embody.
    pub resolution: RealmResolutionDigest,
}

impl SnapshotReady {
    /// True only when this acknowledgement names the exact snapshot, the
    /// exact pinned tuple, AND the exact acknowledged aggregate resolution.
    /// A stale ready message can never embody a player.
    pub fn matches(
        &self,
        snapshot_id: SnapshotId,
        ticket: &JoinTicket,
        resolution: &RealmResolutionDigest,
    ) -> bool {
        self.snapshot_id == snapshot_id
            && self.ticket == *ticket
            && self.resolution == *resolution
    }
    fn validate(&self) -> Result<(), AssetDataError> {
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.snapshot_id.0);
        self.ticket.encode(w);
        self.resolution.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            snapshot_id: SnapshotId(r.u64("snapshot id")?),
            ticket: JoinTicket::decode(r)?,
            resolution: RealmResolutionDigest::decode(r)?,
        })
    }
}

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

canon_doc!(SnapshotBegin, dockind::SNAPSHOT_BEGIN);
canon_doc!(SnapshotChunk, dockind::SNAPSHOT_CHUNK);
canon_doc!(SnapshotEnd, dockind::SNAPSHOT_END);
canon_doc!(SnapshotReady, dockind::SNAPSHOT_READY);

/// Streaming digest over the ordered chunk stream. Host producer and client
/// installer both feed every accepted chunk in transfer order; the result is
/// what `SnapshotEnd` carries. The digest covers section identity, record
/// range, and payload bytes, so a reordered, duplicated, or altered chunk
/// changes it.
pub struct SnapshotDigestBuilder {
    hasher: Sha256,
}

impl Default for SnapshotDigestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotDigestBuilder {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    pub fn add_chunk(&mut self, chunk: &SnapshotChunk) {
        self.hasher.update(&[chunk.section.tag()]);
        self.hasher.update(&chunk.first_record.to_be_bytes());
        self.hasher.update(&chunk.record_count.to_be_bytes());
        self.hasher.update(&(chunk.payload.len() as u32).to_be_bytes());
        self.hasher.update(&chunk.payload);
    }

    pub fn finalize(self) -> SnapshotDigest {
        SnapshotDigest::from_bytes(self.hasher.finalize())
    }
}

/// Client-side bounded verifier: enforces snapshot identity, scene tag,
/// section order, record contiguity/completeness, byte budgets, and the end
/// digest — everything the contract fixes — while leaving record decoding to
/// the session layer. `finish` succeeding is the precondition for atomic
/// adoption and `SnapshotReady`.
pub struct SnapshotAssembler {
    begin: SnapshotBegin,
    received: [u32; 6],
    current_section: usize,
    total_bytes: u64,
    digest: SnapshotDigestBuilder,
}

impl SnapshotAssembler {
    pub fn new(begin: SnapshotBegin) -> Result<Self, AssetDataError> {
        begin.validate()?;
        Ok(Self {
            begin,
            received: [0; 6],
            current_section: 0,
            total_bytes: 0,
            digest: SnapshotDigestBuilder::new(),
        })
    }

    pub fn begin(&self) -> &SnapshotBegin {
        &self.begin
    }

    /// Accept the next chunk in transfer order. Refusals leave the assembler
    /// unchanged, so a host abort/restart maps to dropping the assembler.
    pub fn accept(&mut self, chunk: &SnapshotChunk) -> Result<(), AssetDataError> {
        chunk.validate()?;
        if chunk.snapshot_id != self.begin.snapshot_id {
            return Err(AssetDataError::Mismatch {
                what: "chunk snapshot id",
            });
        }
        if chunk.scene != self.begin.scene {
            return Err(AssetDataError::Mismatch {
                what: "chunk scene tag",
            });
        }
        let section = chunk.section.tag() as usize;
        // Sections arrive in fixed order; a chunk for an earlier section
        // after a later one began is a framing violation, not a reorder to
        // tolerate.
        if section < self.current_section {
            return Err(AssetDataError::NotSorted {
                what: "snapshot section order",
            });
        }
        self.current_section = section;
        if chunk.first_record != self.received[section] {
            return Err(AssetDataError::Mismatch {
                what: "chunk record contiguity",
            });
        }
        let declared = self.begin.counts.of(chunk.section);
        let end = chunk.first_record as u64 + chunk.record_count as u64;
        if end > declared as u64 {
            return Err(AssetDataError::OverBudget {
                what: "chunk records beyond declared count",
                limit: declared as u64,
                found: end,
            });
        }
        let total = self.total_bytes + chunk.payload.len() as u64;
        if total > MAX_SNAPSHOT_TOTAL_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "snapshot total bytes",
                limit: MAX_SNAPSHOT_TOTAL_BYTES,
                found: total,
            });
        }
        self.received[section] = end as u32;
        self.total_bytes = total;
        self.digest.add_chunk(chunk);
        Ok(())
    }

    /// Verify the end record: identity, completeness of every declared
    /// section, and the stream digest. Consumes the assembler; only after
    /// `Ok` may the staged world be adopted atomically.
    pub fn finish(self, end: &SnapshotEnd) -> Result<(), AssetDataError> {
        if end.snapshot_id != self.begin.snapshot_id {
            return Err(AssetDataError::Mismatch {
                what: "end snapshot id",
            });
        }
        if end.scene != self.begin.scene {
            return Err(AssetDataError::Mismatch {
                what: "end scene tag",
            });
        }
        for section in SnapshotSection::ALL {
            if self.received[section.tag() as usize] != self.begin.counts.of(section) {
                return Err(AssetDataError::Missing {
                    what: "snapshot section records",
                });
            }
        }
        if self.digest.finalize() != end.digest {
            return Err(AssetDataError::Mismatch {
                what: "snapshot digest",
            });
        }
        Ok(())
    }
}
