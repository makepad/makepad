//! Deterministic scene migration plans.
//!
//! The AI may recommend a mode and explain why, but the engine only confirms
//! or escalates: `validate()` refuses any plan whose declared mode downgrades
//! what its own reasons and operations require. Unsupported changes fail
//! closed as `HardReset` with concrete reasons rather than guessing.

use crate::codec::{canon_enum, check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::id::{check_name, GameRevisionId, SceneObjectKey, StateKey};
use crate::limits::*;

/// The minimum-safe activation ladder. Ordering is semantic: a larger mode is
/// strictly more disruptive, and a plan may escalate but never downgrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActivationMode {
    /// Presentation/parameter changes at a tick boundary; all dynamic state
    /// retained.
    HotPatch,
    /// Keyed structural changes with bounded deterministic migrators;
    /// unaffected entities keep their identity and state.
    Migrate,
    /// Whole-world replacement. The only mode that advances the realm epoch.
    HardReset,
}

canon_enum!(ActivationMode {
    HotPatch = 0,
    Migrate = 1,
    HardReset = 2,
});

impl ActivationMode {
    /// Escalate-only combination.
    pub fn at_least(self, other: ActivationMode) -> ActivationMode {
        self.max(other)
    }
}

/// Why the planner chose (or refused below) a mode. Codes are closed and
/// carry their own minimum mode so a model assertion can never argue one down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationReasonCode {
    /// Presentation-only change (materials, lights, UI, audio, fixed props).
    PresentationChange,
    /// Compatible component parameter change on a keyed object.
    CompatibleParamChange,
    /// A keyed authored object was added.
    KeyAdded,
    /// A keyed authored object was removed.
    KeyRemoved,
    /// An explicit declared rename mapping an old key to a new one.
    KeyRenamed,
    /// A keyed object changed structurally (asset class, component set).
    StructuralChange,
    /// A typed state schema evolved with a declared migration.
    StateSchemaMigration,
    /// Script state exists with no typed schema/migration contract.
    OpaqueScriptState,
    /// Duplicate or missing stable keys in the candidate source.
    UnkeyedIdentityChurn,
    /// Terrain/base-world topology changed.
    TerrainTopologyChange,
    /// Physics/collider contract changed incompatibly.
    PhysicsIncompatible,
    /// Engine, protocol, or world-coordinate incompatibility.
    EngineIncompatible,
    /// The host or user explicitly requested a reset.
    ExplicitReset,
}

canon_enum!(MigrationReasonCode {
    PresentationChange = 0,
    CompatibleParamChange = 1,
    KeyAdded = 2,
    KeyRemoved = 3,
    KeyRenamed = 4,
    StructuralChange = 5,
    StateSchemaMigration = 6,
    OpaqueScriptState = 7,
    UnkeyedIdentityChurn = 8,
    TerrainTopologyChange = 9,
    PhysicsIncompatible = 10,
    EngineIncompatible = 11,
    ExplicitReset = 12,
});

impl MigrationReasonCode {
    /// The lowest activation mode this finding permits.
    pub fn minimum_mode(self) -> ActivationMode {
        use MigrationReasonCode::*;
        match self {
            PresentationChange | CompatibleParamChange => ActivationMode::HotPatch,
            KeyAdded | KeyRemoved | KeyRenamed | StructuralChange | StateSchemaMigration => {
                ActivationMode::Migrate
            }
            OpaqueScriptState | UnkeyedIdentityChurn | TerrainTopologyChange
            | PhysicsIncompatible | EngineIncompatible | ExplicitReset => ActivationMode::HardReset,
        }
    }
}

/// One structured planner finding.
///
/// Canonical order inside a plan is the reason's complete identity —
/// `(code tag, key with keyless first, detail)` — so two planners that make
/// the same findings in different discovery order produce the identical plan
/// digest, and an exact duplicate finding is refused as noise.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrationReason {
    pub code: MigrationReasonCode,
    /// The key this finding is about, when it concerns one object/state slot.
    pub key: Option<SceneObjectKey>,
    /// Bounded human-readable detail. Diagnostic only — never authority.
    pub detail: String,
}

impl MigrationReason {
    fn sort_key(&self) -> (u8, Option<SceneObjectKey>, String) {
        (self.code.tag(), self.key.clone(), self.detail.clone())
    }
}

impl MigrationReason {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.detail.len() > MAX_REASON_DETAIL_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "reason detail",
                limit: MAX_REASON_DETAIL_BYTES as u64,
                found: self.detail.len() as u64,
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.code.tag());
        match &self.key {
            None => w.bool(false),
            Some(k) => {
                w.bool(true);
                k.encode(w);
            }
        }
        w.str(&self.detail);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            code: MigrationReasonCode::decode(r)?,
            key: if r.bool("reason key present")? {
                Some(SceneObjectKey::decode(r)?)
            } else {
                None
            },
            detail: r.str("reason detail", MAX_REASON_DETAIL_BYTES)?,
        })
    }
}

/// How one component's live state is treated across an update of its object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreserveRule {
    /// Carry the live state through unchanged.
    Preserve,
    /// Reset to the new authored configuration.
    Reset,
    /// Run the typed migrator declared for this component.
    Migrate,
}

canon_enum!(PreserveRule {
    Preserve = 0,
    Reset = 1,
    Migrate = 2,
});

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentRule {
    pub component: String,
    pub rule: PreserveRule,
}

/// One keyed operation in a migration plan.
#[derive(Clone, Debug, PartialEq)]
pub enum SceneOpKind {
    /// A new keyed object appears; it receives a fresh host entity ID.
    Create,
    /// The keyed object survives; component state follows the rules,
    /// canonical order by component name.
    Update { components: Vec<ComponentRule> },
    /// The keyed object is removed; a reliable despawn is emitted.
    Remove,
    /// Explicit declared rename. Without this declaration a rename is a
    /// delete/create and loses state by design, never by accident.
    RenameTo { new_key: SceneObjectKey },
}

impl SceneOpKind {
    fn minimum_mode(&self) -> ActivationMode {
        match self {
            // A parameter-only update can hot patch, but a component that
            // needs its typed state migrator run is a migration by
            // definition — the executor snapshots and transforms live state.
            SceneOpKind::Update { components } => {
                if components.iter().any(|c| c.rule == PreserveRule::Migrate) {
                    ActivationMode::Migrate
                } else {
                    ActivationMode::HotPatch
                }
            }
            _ => ActivationMode::Migrate,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneOp {
    pub key: SceneObjectKey,
    pub kind: SceneOpKind,
}

impl SceneOp {
    fn validate(&self) -> Result<(), AssetDataError> {
        match &self.kind {
            SceneOpKind::Update { components } => {
                if components.len() > MAX_COMPONENT_RULES_PER_OP {
                    return Err(AssetDataError::OverBudget {
                        what: "component rules",
                        limit: MAX_COMPONENT_RULES_PER_OP as u64,
                        found: components.len() as u64,
                    });
                }
                for c in components {
                    check_name(&c.component, "component rule name")?;
                }
                check_sorted_unique(components, |c| c.component.clone(), "component rule")
            }
            SceneOpKind::RenameTo { new_key } => {
                if *new_key == self.key {
                    return Err(AssetDataError::Malformed { what: "rename to same key" });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.key.encode(w);
        match &self.kind {
            SceneOpKind::Create => w.u8(0),
            SceneOpKind::Update { components } => {
                w.u8(1);
                w.u32(components.len() as u32);
                for c in components {
                    w.str(&c.component);
                    w.u8(c.rule.tag());
                }
            }
            SceneOpKind::Remove => w.u8(2),
            SceneOpKind::RenameTo { new_key } => {
                w.u8(3);
                new_key.encode(w);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let key = SceneObjectKey::decode(r)?;
        let kind = match r.u8("scene op kind")? {
            0 => SceneOpKind::Create,
            1 => {
                let n = r.count("component rules", MAX_COMPONENT_RULES_PER_OP)?;
                let mut components = Vec::with_capacity(n);
                for _ in 0..n {
                    components.push(ComponentRule {
                        component: r.str("component rule name", MAX_NAME_BYTES)?,
                        rule: PreserveRule::decode(r)?,
                    });
                }
                SceneOpKind::Update { components }
            }
            2 => SceneOpKind::Remove,
            3 => SceneOpKind::RenameTo {
                new_key: SceneObjectKey::decode(r)?,
            },
            t => {
                return Err(AssetDataError::BadTag {
                    what: "SceneOpKind",
                    found: t,
                })
            }
        };
        Ok(Self { key, kind })
    }
}

/// Declared migration for one typed state slot across schema versions.
#[derive(Clone, Debug, PartialEq)]
pub enum StateMigrationOp {
    /// Discard old data, adopt the new default.
    ResetToDefault,
    /// Adopt the value stored under another key (declared rename).
    RenameFrom { old_key: StateKey },
    /// Deterministic lossless numeric widening (e.g. bool→i64, f32 range).
    WidenNumeric,
    /// Drop the slot entirely.
    Drop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateMigration {
    pub key: StateKey,
    pub from_version: u16,
    pub to_version: u16,
    pub op: StateMigrationOp,
}

impl StateMigration {
    fn validate(&self) -> Result<(), AssetDataError> {
        // Dropping a slot is version-agnostic; every other op must actually
        // advance the schema.
        if !matches!(self.op, StateMigrationOp::Drop) && self.from_version >= self.to_version {
            return Err(AssetDataError::Malformed {
                what: "state migration versions",
            });
        }
        if let StateMigrationOp::RenameFrom { old_key } = &self.op {
            if *old_key == self.key {
                return Err(AssetDataError::Malformed {
                    what: "state rename to same key",
                });
            }
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.key.encode(w);
        w.u16(self.from_version);
        w.u16(self.to_version);
        match &self.op {
            StateMigrationOp::ResetToDefault => w.u8(0),
            StateMigrationOp::RenameFrom { old_key } => {
                w.u8(1);
                old_key.encode(w);
            }
            StateMigrationOp::WidenNumeric => w.u8(2),
            StateMigrationOp::Drop => w.u8(3),
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let key = StateKey::decode(r)?;
        let from_version = r.u16("state migration from")?;
        let to_version = r.u16("state migration to")?;
        let op = match r.u8("state migration op")? {
            0 => StateMigrationOp::ResetToDefault,
            1 => StateMigrationOp::RenameFrom {
                old_key: StateKey::decode(r)?,
            },
            2 => StateMigrationOp::WidenNumeric,
            3 => StateMigrationOp::Drop,
            t => {
                return Err(AssetDataError::BadTag {
                    what: "StateMigrationOp",
                    found: t,
                })
            }
        };
        Ok(Self {
            key,
            from_version,
            to_version,
            op,
        })
    }
}

/// Terrain/base-layer policy across the activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainPolicy {
    /// Terrain and player edits to it are untouched.
    Keep,
    /// Rebuild only the declared affected region caches.
    RebuildAffected,
    /// The base world is replaced (hard-reset class).
    Replace,
}

canon_enum!(TerrainPolicy {
    Keep = 0,
    RebuildAffected = 1,
    Replace = 2,
});

impl TerrainPolicy {
    fn minimum_mode(self) -> ActivationMode {
        match self {
            TerrainPolicy::Keep => ActivationMode::HotPatch,
            TerrainPolicy::RebuildAffected => ActivationMode::Migrate,
            TerrainPolicy::Replace => ActivationMode::HardReset,
        }
    }
}

/// Which caches the verified plan invalidates — the renderer/physics teardown
/// scope comes from here, never from a whole-world guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebuildScopes {
    pub renderer_static: bool,
    pub renderer_lighting: bool,
    pub physics_static: bool,
    pub navigation: bool,
    pub audio: bool,
}

impl RebuildScopes {
    fn encode(&self, w: &mut CanonWriter) {
        let bits = (self.renderer_static as u8)
            | (self.renderer_lighting as u8) << 1
            | (self.physics_static as u8) << 2
            | (self.navigation as u8) << 3
            | (self.audio as u8) << 4;
        w.u8(bits);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let bits = r.u8("rebuild scopes")?;
        if bits & !0b1_1111 != 0 {
            return Err(AssetDataError::Malformed {
                what: "rebuild scopes",
            });
        }
        Ok(Self {
            renderer_static: bits & 1 != 0,
            renderer_lighting: bits & 2 != 0,
            physics_static: bits & 4 != 0,
            navigation: bits & 8 != 0,
            audio: bits & 16 != 0,
        })
    }
}

/// The deterministic old→new scene migration plan. The World Server executes
/// exactly this at the activation tick and rolls back on any failure.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneMigrationPlan {
    pub from_game_revision: GameRevisionId,
    pub to_game_revision: GameRevisionId,
    pub activation_mode: ActivationMode,
    /// Keyed operations, canonical order by key.
    pub ops: Vec<SceneOp>,
    /// Typed state migrations, canonical order by key.
    pub state_migrations: Vec<StateMigration>,
    pub terrain_policy: TerrainPolicy,
    pub rebuild_scopes: RebuildScopes,
    /// Structured findings behind the mode. Canonical order by the reason's
    /// complete identity `(code, key, detail)`; exact duplicates refused.
    pub reasons: Vec<MigrationReason>,
    /// Whether activation must show the user a confirmation (always true for
    /// an escalated hard reset of a live room).
    pub requires_user_confirmation: bool,
}

impl SceneMigrationPlan {
    pub fn canonicalize(&mut self) {
        self.ops.sort_by(|a, b| a.key.cmp(&b.key));
        for op in &mut self.ops {
            if let SceneOpKind::Update { components } = &mut op.kind {
                components.sort_by(|a, b| a.component.cmp(&b.component));
            }
        }
        self.state_migrations.sort_by(|a, b| a.key.cmp(&b.key));
        self.reasons.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    }

    /// The lowest mode this plan's own findings and operations permit.
    pub fn verified_minimum(&self) -> ActivationMode {
        let mut mode = ActivationMode::HotPatch;
        for r in &self.reasons {
            mode = mode.at_least(r.code.minimum_mode());
        }
        for op in &self.ops {
            mode = mode.at_least(op.kind.minimum_mode());
        }
        if !self.state_migrations.is_empty() {
            mode = mode.at_least(ActivationMode::Migrate);
        }
        mode.at_least(self.terrain_policy.minimum_mode())
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.from_game_revision == self.to_game_revision {
            return Err(AssetDataError::Mismatch {
                what: "migration from == to revision",
            });
        }
        if self.ops.len() > MAX_MIGRATION_OPS {
            return Err(AssetDataError::OverBudget {
                what: "migration ops",
                limit: MAX_MIGRATION_OPS as u64,
                found: self.ops.len() as u64,
            });
        }
        if self.state_migrations.len() > MAX_STATE_MIGRATIONS {
            return Err(AssetDataError::OverBudget {
                what: "state migrations",
                limit: MAX_STATE_MIGRATIONS as u64,
                found: self.state_migrations.len() as u64,
            });
        }
        if self.reasons.len() > MAX_MIGRATION_REASONS {
            return Err(AssetDataError::OverBudget {
                what: "migration reasons",
                limit: MAX_MIGRATION_REASONS as u64,
                found: self.reasons.len() as u64,
            });
        }
        for op in &self.ops {
            op.validate()?;
        }
        check_sorted_unique(&self.ops, |o| o.key.clone(), "migration op key")?;
        // Rename targets must be fresh: not another op's key and not another
        // rename's target, or two objects would collide at activation.
        let mut targets: Vec<&SceneObjectKey> = Vec::new();
        for op in &self.ops {
            if let SceneOpKind::RenameTo { new_key } = &op.kind {
                if self.ops.binary_search_by(|o| o.key.cmp(new_key)).is_ok() {
                    return Err(AssetDataError::Duplicate {
                        what: "rename target collides with op key",
                    });
                }
                if targets.contains(&new_key) {
                    return Err(AssetDataError::Duplicate { what: "rename target" });
                }
                targets.push(new_key);
            }
        }
        for m in &self.state_migrations {
            m.validate()?;
        }
        check_sorted_unique(
            &self.state_migrations,
            |m| m.key.clone(),
            "state migration key",
        )?;
        for reason in &self.reasons {
            reason.validate()?;
        }
        check_sorted_unique(&self.reasons, |r| r.sort_key(), "migration reason")?;
        // The engine's finding is a floor. A declared mode below it is the
        // downgrade the contract exists to refuse.
        if self.activation_mode < self.verified_minimum() {
            return Err(AssetDataError::Mismatch {
                what: "activation mode below verified minimum",
            });
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::MIGRATION_PLAN);
        self.from_game_revision.encode(&mut w);
        self.to_game_revision.encode(&mut w);
        w.u8(self.activation_mode.tag());
        w.u32(self.ops.len() as u32);
        for op in &self.ops {
            op.encode(&mut w);
        }
        w.u32(self.state_migrations.len() as u32);
        for m in &self.state_migrations {
            m.encode(&mut w);
        }
        w.u8(self.terrain_policy.tag());
        self.rebuild_scopes.encode(&mut w);
        w.u32(self.reasons.len() as u32);
        for reason in &self.reasons {
            reason.encode(&mut w);
        }
        w.bool(self.requires_user_confirmation);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::MIGRATION_PLAN)?;
        let from_game_revision = GameRevisionId::decode(&mut r)?;
        let to_game_revision = GameRevisionId::decode(&mut r)?;
        let activation_mode = ActivationMode::decode(&mut r)?;
        let n = r.count("migration ops", MAX_MIGRATION_OPS)?;
        let mut ops = Vec::with_capacity(n);
        for _ in 0..n {
            ops.push(SceneOp::decode(&mut r)?);
        }
        let n = r.count("state migrations", MAX_STATE_MIGRATIONS)?;
        let mut state_migrations = Vec::with_capacity(n);
        for _ in 0..n {
            state_migrations.push(StateMigration::decode(&mut r)?);
        }
        let terrain_policy = TerrainPolicy::decode(&mut r)?;
        let rebuild_scopes = RebuildScopes::decode(&mut r)?;
        let n = r.count("migration reasons", MAX_MIGRATION_REASONS)?;
        let mut reasons = Vec::with_capacity(n);
        for _ in 0..n {
            reasons.push(MigrationReason::decode(&mut r)?);
        }
        let requires_user_confirmation = r.bool("requires user confirmation")?;
        r.finish()?;
        let plan = Self {
            from_game_revision,
            to_game_revision,
            activation_mode,
            ops,
            state_migrations,
            terrain_policy,
            rebuild_scopes,
            reasons,
            requires_user_confirmation,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn digest(&self) -> Result<crate::id::MigrationPlanDigest, AssetDataError> {
        Ok(crate::id::MigrationPlanDigest::hash_of(
            &self.to_canonical_bytes()?,
        ))
    }
}
