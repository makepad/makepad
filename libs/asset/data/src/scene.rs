//! The authored scene plan and typed persistent state.
//!
//! Splash plan-mode evaluation produces a canonical `ScenePlan` instead of
//! immediately replacing the live world. Every durable authored object has a
//! required stable `SceneObjectKey`; source position, construction order, and
//! inferred keys are invalid because inserting one line must not rename every
//! door and car.

use crate::codec::{canon_enum, check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::geom::Transform;
use crate::id::{
    check_name, AssetRevisionRef, BlobId, ContentSetId, GameRevisionId, SceneObjectKey, StateKey,
};
use crate::limits::*;
use crate::value::{Value, ValueType};

/// How long one typed persistent-state slot survives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateLifetime {
    /// Reset when the scene changes.
    Scene,
    /// Survives scene revisions within one realm epoch.
    Realm,
    /// Survives until the current match ends.
    Match,
    /// Persisted into the player's save profile.
    Profile,
}

canon_enum!(StateLifetime {
    Scene = 0,
    Realm = 1,
    Match = 2,
    Profile = 3,
});

/// One `game.state(key, schema, default)` declaration: bounded, typed, and
/// migratable. Callback code can be replaced while this keyed data survives;
/// opaque captured VM locals get no such promise.
#[derive(Clone, Debug, PartialEq)]
pub struct StateSchema {
    pub key: StateKey,
    /// Author-declared schema version of this slot, bumped on layout change.
    pub version: u16,
    pub value_type: ValueType,
    pub default: Value,
    pub lifetime: StateLifetime,
}

impl StateSchema {
    fn validate(&self) -> Result<(), AssetDataError> {
        self.default.validate("state default")?;
        if self.default.value_type() != self.value_type {
            return Err(AssetDataError::Mismatch {
                what: "state default vs value_type",
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.key.encode(w);
        w.u16(self.version);
        self.value_type.encode(w);
        self.default.encode(w);
        w.u8(self.lifetime.tag());
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            key: StateKey::decode(r)?,
            version: r.u16("state schema version")?,
            value_type: ValueType::decode(r)?,
            default: Value::decode(r)?,
            lifetime: StateLifetime::decode(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub value: Value,
}

/// One component's configuration on an authored object, canonical param order.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentConfig {
    pub name: String,
    pub params: Vec<Param>,
}

impl ComponentConfig {
    fn validate(&self) -> Result<(), AssetDataError> {
        check_name(&self.name, "component name")?;
        if self.params.len() > MAX_PARAMS_PER_COMPONENT {
            return Err(AssetDataError::OverBudget {
                what: "component params",
                limit: MAX_PARAMS_PER_COMPONENT as u64,
                found: self.params.len() as u64,
            });
        }
        for p in &self.params {
            check_name(&p.name, "param name")?;
            p.value.validate("param value")?;
        }
        check_sorted_unique(&self.params, |p| p.name.clone(), "param name")
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.name);
        w.u32(self.params.len() as u32);
        for p in &self.params {
            w.str(&p.name);
            p.value.encode(w);
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let name = r.str("component name", MAX_NAME_BYTES)?;
        let n = r.count("component params", MAX_PARAMS_PER_COMPONENT)?;
        let mut params = Vec::with_capacity(n);
        for _ in 0..n {
            params.push(Param {
                name: r.str("param name", MAX_NAME_BYTES)?,
                value: Value::decode(r)?,
            });
        }
        Ok(Self { name, params })
    }
}

/// One keyed authored object: placement, optional exact asset, components.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneObject {
    pub key: SceneObjectKey,
    /// Exact revision from the lock; `None` for logical objects (spawn
    /// points, triggers, state anchors).
    pub asset: Option<AssetRevisionRef>,
    pub transform: Transform,
    /// Fixed scene geometry versus host-simulated authored object.
    pub fixed: bool,
    /// Canonical order by component name.
    pub components: Vec<ComponentConfig>,
}

impl SceneObject {
    fn validate(&self) -> Result<(), AssetDataError> {
        self.transform.validate("scene object transform")?;
        if self.components.len() > MAX_COMPONENTS_PER_OBJECT {
            return Err(AssetDataError::OverBudget {
                what: "scene object components",
                limit: MAX_COMPONENTS_PER_OBJECT as u64,
                found: self.components.len() as u64,
            });
        }
        for c in &self.components {
            c.validate()?;
        }
        check_sorted_unique(&self.components, |c| c.name.clone(), "component name")
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.key.encode(w);
        match &self.asset {
            None => w.bool(false),
            Some(a) => {
                w.bool(true);
                a.encode(w);
            }
        }
        self.transform.encode(w);
        w.bool(self.fixed);
        w.u32(self.components.len() as u32);
        for c in &self.components {
            c.encode(w);
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let key = SceneObjectKey::decode(r)?;
        let asset = if r.bool("scene object asset present")? {
            Some(AssetRevisionRef::decode(r)?)
        } else {
            None
        };
        let transform = Transform::decode(r, "scene object transform")?;
        let fixed = r.bool("scene object fixed")?;
        let n = r.count("scene object components", MAX_COMPONENTS_PER_OBJECT)?;
        let mut components = Vec::with_capacity(n);
        for _ in 0..n {
            components.push(ComponentConfig::decode(r)?);
        }
        Ok(Self {
            key,
            asset,
            transform,
            fixed,
            components,
        })
    }
}

/// The fixed terrain/base-world declaration, pinned by the digest of its
/// canonical declaration bytes in CAS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainDecl {
    pub blob: BlobId,
}

/// The canonical authored-scene product of one Splash plan-mode evaluation.
/// Comparing two of these is how the engine computes the minimum safe
/// activation mode.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenePlan {
    pub game_revision: GameRevisionId,
    /// The exact content set every peer must acknowledge before activation.
    pub required_content_set: ContentSetId,
    /// The callback module revision (compiled behavior source digest).
    /// Callbacks swap only after keyed data is ready.
    pub callback_module: BlobId,
    pub terrain: Option<TerrainDecl>,
    /// Keyed authored objects, canonical order by key.
    pub objects: Vec<SceneObject>,
    /// Keyed typed persistent-state schemas, canonical order by key.
    pub state_schemas: Vec<StateSchema>,
}

impl ScenePlan {
    pub fn canonicalize(&mut self) {
        self.objects.sort_by(|a, b| a.key.cmp(&b.key));
        for o in &mut self.objects {
            o.components.sort_by(|a, b| a.name.cmp(&b.name));
            for c in &mut o.components {
                c.params.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        self.state_schemas.sort_by(|a, b| a.key.cmp(&b.key));
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.objects.len() > MAX_SCENE_OBJECTS {
            return Err(AssetDataError::OverBudget {
                what: "scene objects",
                limit: MAX_SCENE_OBJECTS as u64,
                found: self.objects.len() as u64,
            });
        }
        if self.state_schemas.len() > MAX_STATE_SCHEMAS {
            return Err(AssetDataError::OverBudget {
                what: "state schemas",
                limit: MAX_STATE_SCHEMAS as u64,
                found: self.state_schemas.len() as u64,
            });
        }
        for o in &self.objects {
            o.validate()?;
        }
        check_sorted_unique(&self.objects, |o| o.key.clone(), "scene object key")?;
        for s in &self.state_schemas {
            s.validate()?;
        }
        check_sorted_unique(&self.state_schemas, |s| s.key.clone(), "state schema key")?;

        // References between authored objects use stable keys; a dangling key
        // is a validation failure, not a runtime surprise.
        for o in &self.objects {
            for c in &o.components {
                for p in &c.params {
                    if let Value::Key(k) = &p.value {
                        if self
                            .objects
                            .binary_search_by(|x| x.key.cmp(k))
                            .is_err()
                        {
                            return Err(AssetDataError::Missing {
                                what: "referenced scene object key",
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::SCENE_PLAN);
        self.game_revision.encode(&mut w);
        self.required_content_set.encode(&mut w);
        self.callback_module.encode(&mut w);
        match &self.terrain {
            None => w.bool(false),
            Some(t) => {
                w.bool(true);
                t.blob.encode(&mut w);
            }
        }
        w.u32(self.objects.len() as u32);
        for o in &self.objects {
            o.encode(&mut w);
        }
        w.u32(self.state_schemas.len() as u32);
        for s in &self.state_schemas {
            s.encode(&mut w);
        }
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::SCENE_PLAN)?;
        let game_revision = GameRevisionId::decode(&mut r)?;
        let required_content_set = ContentSetId::decode(&mut r)?;
        let callback_module = BlobId::decode(&mut r)?;
        let terrain = if r.bool("terrain present")? {
            Some(TerrainDecl {
                blob: BlobId::decode(&mut r)?,
            })
        } else {
            None
        };
        let n = r.count("scene objects", MAX_SCENE_OBJECTS)?;
        let mut objects = Vec::with_capacity(n);
        for _ in 0..n {
            objects.push(SceneObject::decode(&mut r)?);
        }
        let n = r.count("state schemas", MAX_STATE_SCHEMAS)?;
        let mut state_schemas = Vec::with_capacity(n);
        for _ in 0..n {
            state_schemas.push(StateSchema::decode(&mut r)?);
        }
        r.finish()?;
        let plan = Self {
            game_revision,
            required_content_set,
            callback_module,
            terrain,
            objects,
            state_schemas,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn digest(&self) -> Result<crate::id::ScenePlanDigest, AssetDataError> {
        Ok(crate::id::ScenePlanDigest::hash_of(
            &self.to_canonical_bytes()?,
        ))
    }

    pub fn object(&self, key: &SceneObjectKey) -> Option<&SceneObject> {
        self.objects
            .binary_search_by(|o| o.key.cmp(key))
            .ok()
            .map(|i| &self.objects[i])
    }

    /// Every exact asset revision this plan places, for content-set checks by
    /// the scene compiler and World Server.
    pub fn asset_refs(&self) -> impl Iterator<Item = &AssetRevisionRef> {
        self.objects.iter().filter_map(|o| o.asset.as_ref()).chain(
            self.objects.iter().flat_map(|o| {
                o.components.iter().flat_map(|c| {
                    c.params.iter().filter_map(|p| match &p.value {
                        Value::Asset(a) => Some(a),
                        _ => None,
                    })
                })
            }),
        )
    }
}
