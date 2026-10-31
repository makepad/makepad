//! The open connection-type registry.
//!
//! A connection type is a [`TypeKey`] with a schema revision, a carrier and
//! a rate. Providers register their own keys (a model, a world snapshot, a
//! pose, a collection) without the core gaining a variant or an import.
//! Conversions and replication codecs are separate, optional tables.
//!
//! Builtin [`PortType`] aliases keep their wire spellings. Registered keys
//! use the same schemas during document loading and runtime admission.
//! A registry is built by the host before starting the engine, then shared
//! immutably across evaluation, execution and editor metadata.

use crate::PortType;
use makepad_micro_serde::*;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeKey(pub Arc<str>);

macro_rules! string_id_json {
    ($name:ident) => {
        impl SerJson for $name {
            fn ser_json(&self, depth: usize, state: &mut SerJsonState) {
                self.0.as_ref().ser_json(depth, state);
            }
        }
        impl DeJson for $name {
            fn de_json(state: &mut DeJsonState, input: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
                Ok(Self(Arc::from(String::de_json(state, input)?)))
            }
        }
    };
}
string_id_json!(TypeKey);
string_id_json!(SerialCodecId);
string_id_json!(LoweringId);
string_id_json!(ReplCodecId);

impl TypeKey {
    pub fn new(key: &str) -> Self {
        Self(Arc::from(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TypeKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// How a value of the type travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum Carrier {
    /// A content-addressed [`crate::Value`] handled by the job scheduler.
    Artifact,
    Control,
    Event,
    Clock,
    AudioPort,
    Frame,
    /// A provider handle.
    Ref,
    /// An immutable stamped provider extract.
    Snapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum Rate {
    Static,
    Control,
    Event,
    Block,
    Frame,
    Tick,
    Job,
}

/// Names a provider-registered serial codec (document, literal or artifact
/// form).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SerialCodecId(pub Arc<str>);

/// Names the declared codec that lowers a rich event type to the fixed-size
/// packet of the audio boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoweringId(pub Arc<str>);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplCodecId(pub Arc<str>);

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct TypeSchema {
    pub key: TypeKey,
    pub schema_rev: u32,
    pub carrier: Carrier,
    pub rate: Rate,
    /// GPU, device, session and VM handles. Never serialised or replicated.
    pub local_only: bool,
    pub serial: Option<SerialCodecId>,
    pub audio_lowering: Option<LoweringId>,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversion {
    pub from: TypeKey,
    pub to: TypeKey,
    /// The node type that performs the conversion; inserted visibly.
    pub node: Arc<str>,
    pub cost: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum ReplicationTier {
    Shared,
    Derived,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct ReplicationCodec {
    pub key: TypeKey,
    pub codec: ReplCodecId,
    pub tier: ReplicationTier,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Unknown(TypeKey),
    /// The key exists with a different schema.
    Conflict(TypeKey),
    /// A `local_only` type may not declare a serial codec.
    LocalOnlySerial(TypeKey),
    /// A `local_only` type may not be replicated.
    LocalOnlyReplication(TypeKey),
    /// An audio lowering only applies to an event type.
    LoweringNotEvent(TypeKey),
    /// Keys are lower-case dotted identifiers such as `scene.mesh`.
    MalformedKey(String),
    UnsupportedArtifact(TypeKey),
    InvalidSchema(TypeKey),
    ReservedKey(TypeKey),
    Capacity,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(key) => write!(formatter, "connection type `{key}` is not registered"),
            Self::Conflict(key) => write!(formatter, "connection type `{key}` is already registered with a different schema"),
            Self::LocalOnlySerial(key) => write!(formatter, "local-only type `{key}` cannot declare a serial codec"),
            Self::LocalOnlyReplication(key) => write!(formatter, "local-only type `{key}` cannot be replicated"),
            Self::LoweringNotEvent(key) => write!(formatter, "type `{key}` declares an audio lowering but is not an event type"),
            Self::MalformedKey(key) => write!(formatter, "`{key}` is not a lower-case type key of at most 128 bytes"),
            Self::UnsupportedArtifact(key) => write!(formatter, "type `{key}` is not a serializable job artifact using the core.value codec"),
            Self::InvalidSchema(key) => write!(formatter, "type `{key}` has an invalid schema revision or oversized metadata"),
            Self::ReservedKey(key) => write!(formatter, "type key `{key}` uses the reserved core namespace"),
            Self::Capacity => write!(formatter, "type registry exceeds 1024 schemas"),
        }
    }
}

impl std::error::Error for TypeError {}

/// The builtin alias of a closed [`PortType`] variant. Its serialised
/// spelling in flow sources stays `PortType::as_str`.
pub fn builtin_key(ty: &PortType) -> &str {
    match ty {
        PortType::Text => "core.text",
        PortType::Image => "core.image",
        PortType::Audio => "core.audio",
        PortType::Video => "core.video",
        PortType::Mesh => "core.mesh",
        PortType::Json => "core.json",
        PortType::List => "core.list",
        PortType::Bytes => "core.bytes",
        PortType::Registered(key) => key.as_str(),
    }
}

const BUILTINS: [PortType; 8] = [
    PortType::Text,
    PortType::Image,
    PortType::Audio,
    PortType::Video,
    PortType::Mesh,
    PortType::Json,
    PortType::List,
    PortType::Bytes,
];

#[derive(Clone, Debug)]
pub struct TypeRegistry {
    schemas: BTreeMap<TypeKey, TypeSchema>,
    conversions: Vec<Conversion>,
    replication: BTreeMap<TypeKey, ReplicationCodec>,
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl TypeRegistry {
    /// A registry holding the builtin aliases: every closed `PortType` is an
    /// artifact at job rate, carried as a content-addressed `Value`.
    pub fn with_builtins() -> Self {
        let mut schemas = BTreeMap::new();
        for ty in BUILTINS {
            let key = TypeKey::new(builtin_key(&ty));
            schemas.insert(
                key.clone(),
                TypeSchema {
                    key,
                    schema_rev: 1,
                    carrier: Carrier::Artifact,
                    rate: Rate::Job,
                    local_only: false,
                    serial: Some(SerialCodecId(Arc::from("core.value"))),
                    audio_lowering: None,
                    doc: ty.as_str().to_owned(),
                },
            );
        }
        Self { schemas, conversions: Vec::new(), replication: BTreeMap::new() }
    }

    /// Register a provider type. Registering the identical schema twice is
    /// fine; a different schema under the same key is a conflict.
    pub fn register(&mut self, schema: TypeSchema) -> Result<(), TypeError> {
        if !well_formed(schema.key.as_str()) || !schema.key.as_str().contains('.') {
            return Err(TypeError::MalformedKey(schema.key.as_str().to_string()));
        }
        if schema.schema_rev == 0 || schema.doc.len() > 8192
            || schema.serial.as_ref().is_some_and(|id| !well_formed(&id.0))
            || schema.audio_lowering.as_ref().is_some_and(|id| !well_formed(&id.0)) {
            return Err(TypeError::InvalidSchema(schema.key));
        }
        if schema.local_only && schema.serial.is_some() {
            return Err(TypeError::LocalOnlySerial(schema.key));
        }
        if schema.audio_lowering.is_some() && schema.carrier != Carrier::Event {
            return Err(TypeError::LoweringNotEvent(schema.key));
        }
        match self.schemas.get(&schema.key) {
            Some(existing) if *existing == schema => Ok(()),
            Some(_) => Err(TypeError::Conflict(schema.key)),
            None => {
                if schema.key.as_str().starts_with("core.") {
                    return Err(TypeError::ReservedKey(schema.key));
                }
                if self.schemas.len() >= 1024 { return Err(TypeError::Capacity) }
                self.schemas.insert(schema.key.clone(), schema);
                Ok(())
            }
        }
    }

    /// The schema of `key`, or an error naming the unknown key.
    pub fn resolve(&self, key: &str) -> Result<&TypeSchema, TypeError> {
        let key = TypeKey::new(key);
        self.schemas.get(&key).ok_or(TypeError::Unknown(key))
    }

    pub fn resolve_port(&self, ty: &PortType) -> Result<&TypeSchema, TypeError> {
        if PortType::from_str(ty.as_str()).as_ref() != Some(ty) {
            return Err(TypeError::MalformedKey(ty.as_str().to_owned()));
        }
        self.resolve(ty.key())
    }

    /// The job engine only carries serialized artifacts. Registered streams,
    /// clocks and local handles remain metadata until their owner is installed.
    pub fn require_artifact(&self, ty: &PortType) -> Result<&TypeSchema, TypeError> {
        let schema = self.resolve_port(ty)?;
        if schema.carrier != Carrier::Artifact || schema.rate != Rate::Job || schema.local_only
            || schema.serial.as_ref().map(|id| id.0.as_ref()) != Some("core.value") {
            return Err(TypeError::UnsupportedArtifact(schema.key.clone()));
        }
        Ok(schema)
    }

    pub fn register_conversion(&mut self, conversion: Conversion) -> Result<(), TypeError> {
        self.resolve(conversion.from.as_str())?;
        self.resolve(conversion.to.as_str())?;
        if !self.conversions.contains(&conversion) {
            self.conversions.push(conversion);
        }
        Ok(())
    }

    /// The cheapest registered conversion, if any. It is never implicit: the
    /// caller inserts `node` as a visible node.
    pub fn conversion(&self, from: &TypeKey, to: &TypeKey) -> Option<&Conversion> {
        self.conversions
            .iter()
            .filter(|conversion| &conversion.from == from && &conversion.to == to)
            .min_by_key(|conversion| conversion.cost)
    }

    pub fn register_replication(&mut self, codec: ReplicationCodec) -> Result<(), TypeError> {
        if self.resolve(codec.key.as_str())?.local_only {
            return Err(TypeError::LocalOnlyReplication(codec.key));
        }
        self.replication.insert(codec.key.clone(), codec);
        Ok(())
    }

    pub fn replication(&self, key: &TypeKey) -> Option<&ReplicationCodec> {
        self.replication.get(key)
    }

    pub fn schemas(&self) -> impl Iterator<Item = &TypeSchema> {
        self.schemas.values()
    }
}

pub fn well_formed(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128
        && key.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().next().is_some_and(|byte| byte.is_ascii_lowercase())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}
