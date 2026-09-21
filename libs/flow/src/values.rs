use crate::{Literal, PortType};
use makepad_ai_hub::sha256::{to_hex, Sha256};
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

fn literal_json(value: &Literal) -> JsonValue {
    match value {
        Literal::Null => JsonValue::Null,
        Literal::Bool(value) => JsonValue::Bool(*value),
        Literal::Num(value) if value.fract() == 0.0 => JsonValue::Int(*value as i64),
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
    ty: PortType,
    content_type: String,
    bytes: usize,
    touched: SystemTime,
}

pub struct ValueStore {
    pub ram_budget: usize,
    pub spill_dir: PathBuf,
    pub ttl: Duration,
    ram: HashMap<[u8; 32], (Value, SystemTime)>,
    spilled: HashMap<[u8; 32], StoredMeta>,
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
        if self.ram.contains_key(&digest) || self.spilled.contains_key(&digest) {
            self.touch(&digest);
            return digest;
        }
        self.ram_bytes = self.ram_bytes.saturating_add(value.bytes.len());
        self.ram.insert(digest, (value, SystemTime::now()));
        self.evict();
        digest
    }

    pub fn get(&mut self, digest: &[u8; 32]) -> Option<Value> {
        if let Some((value, touched)) = self.ram.get_mut(digest) {
            *touched = SystemTime::now();
            return Some(value.clone());
        }
        let path = self.spill_path(digest);
        let meta = self.spilled.get_mut(digest)?;
        meta.touched = SystemTime::now();
        let ty = meta.ty;
        let content_type = meta.content_type.clone();
        let bytes = std::fs::read(path).ok()?;
        let value = Value::new(ty, content_type, Arc::from(bytes));
        (value.digest == *digest).then_some(value)
    }

    pub fn touch(&mut self, digest: &[u8; 32]) -> bool {
        let now = SystemTime::now();
        if let Some((_, touched)) = self.ram.get_mut(digest) {
            *touched = now;
            return true;
        }
        if let Some(meta) = self.spilled.get_mut(digest) {
            meta.touched = now;
            return true;
        }
        false
    }

    pub fn evict(&mut self) {
        while self.ram_bytes > self.ram_budget {
            let Some((&digest, _)) = self.ram.iter().min_by_key(|(_, (_, touched))| *touched)
            else {
                break;
            };
            let Some((value, touched)) = self.ram.remove(&digest) else {
                break;
            };
            let path = self.spill_path(&digest);
            let written = std::fs::create_dir_all(&self.spill_dir)
                .and_then(|_| std::fs::write(&path, &value.bytes));
            if written.is_err() {
                self.ram.insert(digest, (value, touched));
                break;
            }
            self.ram_bytes = self.ram_bytes.saturating_sub(value.bytes.len());
            self.spilled.insert(
                digest,
                StoredMeta {
                    ty: value.ty,
                    content_type: value.content_type,
                    bytes: value.bytes.len(),
                    touched,
                },
            );
        }
    }

    pub fn expire(&mut self, now: SystemTime, live: &HashSet<[u8; 32]>) {
        let expired: Vec<_> = self
            .spilled
            .iter()
            .filter_map(|(digest, meta)| {
                (!live.contains(digest)
                    && now.duration_since(meta.touched).unwrap_or_default() >= self.ttl)
                    .then_some(*digest)
            })
            .collect();
        for digest in expired {
            self.spilled.remove(&digest);
            let _ = std::fs::remove_file(self.spill_path(&digest));
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
