//! Room content sets and deterministic `AssetSlot` tables.
//!
//! A slot number is an index into an append-only table of exact revisions, so
//! gameplay descriptors can carry a compact `u32` instead of a 32-byte digest.
//! Slot numbers are indexes by construction: the table cannot have holes, and
//! extending a set never reinterprets an existing slot. The concrete hashed
//! manifest carries the FULL table plus its parent relation — prose or a bare
//! digest is not enough to decode descriptors.

use crate::codec::{dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::game::ContentLock;
use crate::id::{AssetRevisionRef, ContentSetId, GameRevisionId};
use crate::limits::MAX_CONTENT_SET_SLOTS;

/// Compact append-only index of one exact revision inside one content set.
/// Meaningless outside the content set that assigned it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetSlot(pub u32);

impl std::fmt::Display for AssetSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "slot#{}", self.0)
    }
}

/// The immutable content-set manifest. `slots[i]` is the revision assigned to
/// `AssetSlot(i)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentSetManifest {
    pub game_revision: GameRevisionId,
    /// The set this one extends; `None` for a release baseline.
    pub parent: Option<ContentSetId>,
    pub slots: Vec<AssetRevisionRef>,
}

impl ContentSetManifest {
    /// The deterministic release baseline of a game revision: the lock's
    /// closure in its canonical order. Every peer that derives a baseline from
    /// the same revision gets the identical slot table.
    pub fn baseline(
        game_revision: GameRevisionId,
        lock: &ContentLock,
    ) -> Result<Self, AssetDataError> {
        lock.validate()?;
        let set = Self {
            game_revision,
            parent: None,
            slots: lock.closure.clone(),
        };
        set.validate()?;
        Ok(set)
    }

    /// Append-only extension for temporary runtime spawns. Additions are
    /// sorted canonically before assignment so the host and every auditor
    /// derive the same table from the same delta; a revision already in the
    /// set is refused rather than silently double-slotted.
    pub fn extended(&self, additions: &[AssetRevisionRef]) -> Result<Self, AssetDataError> {
        if additions.is_empty() {
            return Err(AssetDataError::Missing {
                what: "content set additions",
            });
        }
        let mut sorted = additions.to_vec();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != additions.len() {
            return Err(AssetDataError::Duplicate {
                what: "content set addition",
            });
        }
        for add in &sorted {
            if self.slots.contains(add) {
                return Err(AssetDataError::Duplicate {
                    what: "content set addition already in set",
                });
            }
        }
        let mut slots = self.slots.clone();
        slots.extend(sorted);
        let set = Self {
            game_revision: self.game_revision,
            parent: Some(self.id()?),
            slots,
        };
        set.validate()?;
        Ok(set)
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.slots.is_empty() {
            return Err(AssetDataError::Missing { what: "content set slots" });
        }
        if self.slots.len() > MAX_CONTENT_SET_SLOTS {
            return Err(AssetDataError::OverBudget {
                what: "content set slots",
                limit: MAX_CONTENT_SET_SLOTS as u64,
                found: self.slots.len() as u64,
            });
        }
        // Append-only means the table is NOT globally sorted, but every exact
        // revision still owns exactly one slot.
        let mut seen = self.slots.clone();
        seen.sort();
        for pair in seen.windows(2) {
            if pair[0] == pair[1] {
                return Err(AssetDataError::Duplicate { what: "content set slot" });
            }
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::CONTENT_SET);
        self.game_revision.encode(&mut w);
        match &self.parent {
            None => w.bool(false),
            Some(p) => {
                w.bool(true);
                p.encode(&mut w);
            }
        }
        w.u32(self.slots.len() as u32);
        for s in &self.slots {
            s.encode(&mut w);
        }
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::CONTENT_SET)?;
        let game_revision = GameRevisionId::decode(&mut r)?;
        let parent = if r.bool("content set parent present")? {
            Some(ContentSetId::decode(&mut r)?)
        } else {
            None
        };
        let n = r.count("content set slots", MAX_CONTENT_SET_SLOTS)?;
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            slots.push(AssetRevisionRef::decode(&mut r)?);
        }
        r.finish()?;
        let set = Self {
            game_revision,
            parent,
            slots,
        };
        set.validate()?;
        Ok(set)
    }

    /// The compact content identity carried by the game session.
    pub fn id(&self) -> Result<ContentSetId, AssetDataError> {
        Ok(ContentSetId::hash_of(&self.to_canonical_bytes()?))
    }

    pub fn get(&self, slot: AssetSlot) -> Option<&AssetRevisionRef> {
        self.slots.get(slot.0 as usize)
    }

    pub fn slot_of(&self, rev: &AssetRevisionRef) -> Option<AssetSlot> {
        self.slots
            .iter()
            .position(|s| s == rev)
            .map(|i| AssetSlot(i as u32))
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Verify the append-only parent/prefix relation against the actual
    /// parent manifest: same game revision, correct parent digest, and the
    /// parent's whole table as an unchanged prefix. Changing the content set
    /// never reinterprets an existing slot.
    pub fn extends(&self, parent: &ContentSetManifest) -> Result<(), AssetDataError> {
        if self.parent != Some(parent.id()?) {
            return Err(AssetDataError::Mismatch {
                what: "content set parent digest",
            });
        }
        if self.game_revision != parent.game_revision {
            return Err(AssetDataError::Mismatch {
                what: "content set game revision",
            });
        }
        if self.slots.len() <= parent.slots.len()
            || self.slots[..parent.slots.len()] != parent.slots[..]
        {
            return Err(AssetDataError::Mismatch {
                what: "content set parent prefix",
            });
        }
        Ok(())
    }
}
