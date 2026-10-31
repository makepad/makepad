use crate::{Literal, PortType};
use crate::digest::{to_hex, Sha256};
use makepad_strict_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub ty: PortType,
    pub content_type: String,
    pub bytes: Arc<[u8]>,
    pub digest: [u8; 32],
}

impl Value {
    pub fn text(text: impl AsRef<str>) -> Self {
        Self::new(
            PortType::Text,
            "text/plain; charset=utf-8".to_string(),
            Arc::from(text.as_ref().as_bytes()),
        )
    }

    pub fn json(json: impl AsRef<str>) -> Self {
        Self::new(
            PortType::Json,
            "application/json".to_string(),
            Arc::from(json.as_ref().as_bytes()),
        )
    }

    pub fn media(
        ty: PortType,
        content_type: impl Into<String>,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        assert!(ty.is_media(), "Value::media requires a media/bytes port type");
        Self::new(ty, content_type.into(), bytes.into())
    }

    /// Construct an opaque artifact only after its host schema admits the
    /// common byte-value codec. Streams and local device/GPU handles cannot
    /// enter the job value store through this constructor.
    pub fn artifact(
        ty: PortType,
        content_type: impl Into<String>,
        bytes: impl Into<Arc<[u8]>>,
        types: &crate::document::types::TypeRegistry,
    ) -> Result<Self, String> {
        types.require_artifact(&ty).map_err(|error| error.to_string())?;
        Ok(Self::new(ty, content_type.into(), bytes.into()))
    }

    pub fn list(json: impl AsRef<str>) -> Self {
        Self::new(
            PortType::List,
            "application/json".to_string(),
            Arc::from(json.as_ref().as_bytes()),
        )
    }

    pub fn from_literal(ty: PortType, literal: &Literal) -> Result<Self, String> {
        match ty {
            PortType::Text => match literal {
                Literal::Str(value) | Literal::Id(value) => Ok(Self::text(value)),
                _ => Err("text value must be a string".to_string()),
            },
            PortType::Json => Ok(Self::json(literal_json(literal).to_json())),
            PortType::List => match literal {
                Literal::Arr(_) => Ok(Self::list(literal_json(literal).to_json())),
                _ => Err("list value must be an array".to_string()),
            },
            _ => Err(format!("{} values cannot be literal", ty.as_str())),
        }
    }

    pub fn digest_hex(&self) -> String {
        to_hex(&self.digest)
    }

    pub fn as_text(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.bytes).map_err(|error| format!("value is not utf-8: {error}"))
    }

    fn new(ty: PortType, content_type: String, bytes: Arc<[u8]>) -> Self {
        let mut sha = Sha256::new();
        sha.update(&bytes);
        let digest = sha.finish();
        Self {
            ty,
            content_type,
            bytes,
            digest,
        }
    }
}

/// An ordered run of values on one port: typed even when empty, so an
/// empty sequence still says what it is a sequence of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub ty: PortType,
    pub values: Vec<Value>,
}

impl Sequence {
    pub fn empty(ty: PortType) -> Self {
        Self { ty, values: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// What one port carries: exactly one value (a one port), or an ordered
/// sequence of `0..=bound` values (an ordered port). The one grouped form
/// every table, row and instance keeps, so nothing collapses to a last
/// value and an empty sequence stays distinct from an absent one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortValue {
    One(Value),
    Seq(Sequence),
}

impl PortValue {
    pub fn ty(&self) -> PortType {
        match self {
            Self::One(value) => value.ty.clone(),
            Self::Seq(sequence) => sequence.ty.clone(),
        }
    }

    pub fn is_ordered(&self) -> bool {
        matches!(self, Self::Seq(_))
    }

    /// The value of a one port; `None` for a sequence, however long.
    pub fn one(&self) -> Option<&Value> {
        match self {
            Self::One(value) => Some(value),
            Self::Seq(_) => None,
        }
    }

    /// Every element in order: one for a one port.
    pub fn values(&self) -> &[Value] {
        match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Seq(sequence) => &sequence.values,
        }
    }

    pub fn len(&self) -> usize {
        self.values().len()
    }

    pub fn is_empty(&self) -> bool {
        self.values().is_empty()
    }

    /// Bytes of every element, what an event or a row keeps alive.
    pub fn bytes_len(&self) -> usize {
        self.values().iter().map(|value| value.bytes.len()).sum()
    }

    pub fn digests(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        self.values().iter().map(|value| value.digest)
    }

    /// Whether this value fits a port's shape: the element type equal, a
    /// one port holding exactly one value, an ordered port a sequence of
    /// at most its bound. The bound is schema, not a tag on the value.
    pub fn matches(&self, shape: crate::Shape) -> Result<(), String> {
        match (self, shape.many) {
            (Self::One(value), None) => {
                if value.ty != shape.ty {
                    return Err(format!("expected {}, got {}", shape.ty.as_str(), value.ty.as_str()));
                }
                Ok(())
            }
            (Self::One(_), Some(_)) => Err(format!("expected an ordered {} sequence, got one value", shape.ty.as_str())),
            (Self::Seq(_), None) => Err(format!("expected one {} value, got a sequence", shape.ty.as_str())),
            (Self::Seq(sequence), Some(bound)) => {
                if sequence.ty != shape.ty {
                    return Err(format!(
                        "expected a sequence of {}, got a sequence of {}",
                        shape.ty.as_str(),
                        sequence.ty.as_str()
                    ));
                }
                if let Some(value) = sequence.values.iter().find(|value| value.ty != shape.ty) {
                    return Err(format!("a {} element in a sequence of {}", value.ty.as_str(), shape.ty.as_str()));
                }
                if sequence.values.len() > bound as usize {
                    return Err(format!("{} values, over the bound of {bound}", sequence.values.len()));
                }
                Ok(())
            }
        }
    }
}

pub(crate) fn literal_json(value: &Literal) -> JsonValue {
    match value {
        Literal::Null => JsonValue::Null,
        Literal::Bool(value) => JsonValue::Bool(*value),
        Literal::Num(value) if value.fract() == 0.0
            && *value >= i64::MIN as f64 && *value < -(i64::MIN as f64) => JsonValue::Int(*value as i64),
        Literal::Num(value) => JsonValue::F64(*value),
        Literal::Str(value) | Literal::Id(value) => JsonValue::Str(value.clone()),
        Literal::Arr(values) => JsonValue::Arr(values.iter().map(literal_json).collect()),
        Literal::Obj(values) => JsonValue::Obj(
            values
                .iter()
                .map(|(name, value)| (name.clone(), literal_json(value)))
                .collect(),
        ),
    }
}

#[derive(Clone)]
struct StoredMeta {
    content_type: String,
    bytes: usize,
    touched: SystemTime,
}

pub struct ValueStore {
    pub ram_budget: usize,
    pub spill_dir: PathBuf,
    pub ttl: Duration,
    ram: HashMap<([u8; 32], PortType), (Value, SystemTime)>,
    spilled: HashMap<([u8; 32], PortType), StoredMeta>,
    ram_bytes: usize,
}

impl ValueStore {
    pub fn new(spill_dir: PathBuf) -> Self {
        Self {
            ram_budget: 256 * 1024 * 1024,
            spill_dir,
            ttl: Duration::from_secs(60 * 60),
            ram: HashMap::new(),
            spilled: HashMap::new(),
            ram_bytes: 0,
        }
    }

    pub fn put(&mut self, value: Value) -> [u8; 32] {
        let digest = value.digest;
        let key = (digest, value.ty.clone());
        let now = SystemTime::now();
        if let Some((_, touched)) = self.ram.get_mut(&key) {
            *touched = now;
            return digest;
        }
        if let Some(meta) = self.spilled.get_mut(&key) {
            meta.touched = now;
            return digest;
        }
        self.ram_bytes = self.ram_bytes.saturating_add(value.bytes.len());
        self.ram.insert(key, (value, now));
        self.evict();
        digest
    }

    /// Byte-addressed legacy retrieval. Equal bytes can have several actual
    /// stored types; choose a stable one for the byte-only data endpoint.
    /// Typed graph inputs always use `get_typed`, never this method.
    pub fn get(&mut self, digest: &[u8; 32]) -> Option<Value> {
        let ty = self.ram.keys().chain(self.spilled.keys())
            .filter(|(stored, _)| stored == digest)
            .map(|(_, ty)| ty)
            .min_by(|a, b| a.key().cmp(b.key()))?.clone();
        self.get_typed(digest, &ty)
    }

    pub fn get_typed(&mut self, digest: &[u8; 32], ty: &PortType) -> Option<Value> {
        let key = (*digest, ty.clone());
        if let Some((value, touched)) = self.ram.get_mut(&key) {
            *touched = SystemTime::now();
            return Some(value.clone());
        }
        let path = self.spill_path(digest);
        let meta = self.spilled.get_mut(&key)?;
        meta.touched = SystemTime::now();
        let content_type = meta.content_type.clone();
        let bytes = std::fs::read(path).ok()?;
        let value = Value::new(ty.clone(), content_type, Arc::from(bytes));
        (value.digest == *digest).then_some(value)
    }

    /// Inspect exact typed admission metadata without reading a spilled value
    /// or extending its lifetime. A matching digest never permits retagging.
    #[cfg(all(feature = "runtime", not(target_arch = "wasm32")))]
    pub(crate) fn type_and_size_typed(&self, digest: &[u8; 32], ty: &PortType) -> Option<(PortType, usize)> {
        let key = (*digest, ty.clone());
        self.ram.get(&key).map(|(value, _)| (value.ty.clone(), value.bytes.len()))
            .or_else(|| self.spilled.get(&key).map(|meta| (ty.clone(), meta.bytes)))
    }

    pub fn touch(&mut self, digest: &[u8; 32]) -> bool {
        let now = SystemTime::now();
        let mut found = false;
        for ((stored, _), (_, touched)) in &mut self.ram {
            if stored == digest { *touched = now; found = true; }
        }
        for ((stored, _), meta) in &mut self.spilled {
            if stored == digest { meta.touched = now; found = true; }
        }
        found
    }

    pub fn evict(&mut self) {
        while self.ram_bytes > self.ram_budget {
            let Some(key) = self.ram.iter().min_by_key(|(_, (_, touched))| *touched)
                .map(|(key, _)| key.clone()) else { break };
            let Some((value, touched)) = self.ram.remove(&key) else { break };
            let path = self.spill_path(&key.0);
            let written = std::fs::create_dir_all(&self.spill_dir)
                .and_then(|_| std::fs::write(&path, &value.bytes));
            if written.is_err() {
                self.ram.insert(key, (value, touched));
                break;
            }
            self.ram_bytes = self.ram_bytes.saturating_sub(value.bytes.len());
            self.spilled.insert(key, StoredMeta {
                content_type: value.content_type,
                bytes: value.bytes.len(),
                touched,
            });
        }
    }

    pub fn expire(&mut self, now: SystemTime, live: &HashSet<[u8; 32]>) {
        let expired: Vec<_> = self.spilled.iter()
            .filter_map(|(key, meta)| {
                (!live.contains(&key.0)
                    && now.duration_since(meta.touched).unwrap_or_default() >= self.ttl)
                    .then_some(key.clone())
            }).collect();
        for key in expired {
            self.spilled.remove(&key);
            // Several typed entries may share the same physical bytes.
            if !self.spilled.keys().any(|other| other.0 == key.0) {
                let _ = std::fs::remove_file(self.spill_path(&key.0));
            }
        }
    }

    pub fn ram_bytes(&self) -> usize {
        self.ram_bytes
    }

    pub fn spilled_bytes(&self) -> usize {
        self.spilled.values().map(|meta| meta.bytes).sum()
    }

    fn spill_path(&self, digest: &[u8; 32]) -> PathBuf {
        self.spill_dir.join(to_hex(digest))
    }
}
