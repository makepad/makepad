//! Durable acknowledgement of model-weight licence identities.
//!
//! Acknowledgements are deliberately separate from the weight cache: clearing
//! downloads must not erase the record of what text the operator accepted,
//! and changing that text's identity must make the model prompt again.

use crate::error::AssetAiError;
use crate::home::makepad_home;
use crate::registry::{LicenseRestriction, ModelSpec};
use makepad_micro_serde::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LICENSE_ACKS_FILE: &str = "license_acks.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicensePrompt {
    pub model_id: String,
    pub name: String,
    pub url: String,
    pub summary: String,
    pub restriction: LicenseRestriction,
    pub identity: String,
}

impl LicensePrompt {
    pub fn from_spec(spec: &ModelSpec) -> Self {
        match &spec.license {
            Some(license) => Self {
                model_id: spec.id.clone(),
                name: license.name.clone(),
                url: license.url.clone(),
                summary: license.summary.clone(),
                restriction: license.restriction,
                identity: license.identity(),
            },
            None => Self {
                model_id: spec.id.clone(),
                name: "Unknown weight licence".to_string(),
                url: "https://huggingface.co/".to_string(),
                summary: format!(
                    "{} has no licence record in the registry. It cannot be cleared for download or generation until a licence record is added.",
                    spec.id
                ),
                restriction: LicenseRestriction::Restricted,
                identity: "missing".to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, SerJson, DeJson, PartialEq, Eq)]
pub struct LicenseAcknowledgement {
    pub model_id: String,
    pub identity: String,
    pub acknowledged_at: u64,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
struct LicenseFile {
    version: u32,
    acknowledgements: Vec<LicenseAcknowledgement>,
}

pub struct LicenseStore {
    path: PathBuf,
    records: Vec<LicenseAcknowledgement>,
}

impl LicenseStore {
    pub fn open() -> Result<Self, AssetAiError> {
        Self::open_at(makepad_home().join(LICENSE_ACKS_FILE))
    }

    pub(crate) fn open_at(path: PathBuf) -> Result<Self, AssetAiError> {
        let records = match fs::read_to_string(&path) {
            Ok(text) => LicenseFile::deserialize_json(&text)
                .map_err(|error| {
                    AssetAiError::Io(format!("parse {}: {error:?}", path.display()))
                })?
                .acknowledgements,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(AssetAiError::Io(format!(
                    "read {}: {error}",
                    path.display()
                )))
            }
        };
        Ok(Self { path, records })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn records(&self) -> &[LicenseAcknowledgement] {
        &self.records
    }

    pub fn acknowledged(&self, model_id: &str, identity: &str) -> bool {
        self.records
            .iter()
            .any(|record| record.model_id == model_id && record.identity == identity)
    }

    pub fn acknowledge(&mut self, model_id: &str, identity: &str) -> Result<(), AssetAiError> {
        if self.acknowledged(model_id, identity) {
            return Ok(());
        }
        self.records.push(LicenseAcknowledgement {
            model_id: model_id.to_string(),
            identity: identity.to_string(),
            acknowledged_at: unix_time(),
        });
        if let Err(error) = self.persist() {
            self.records.pop();
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), AssetAiError> {
        let parent = self.path.parent().ok_or_else(|| {
            AssetAiError::Io(format!("licence acknowledgement path has no parent: {}", self.path.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| AssetAiError::Io(format!("mkdir {}: {error}", parent.display())))?;
        let file = LicenseFile {
            version: 1,
            acknowledgements: self.records.clone(),
        };
        let part = parent.join(format!(
            ".license_acks-{}-{}.part",
            std::process::id(),
            unix_nanos()
        ));
        fs::write(&part, file.serialize_json()).map_err(|error| {
            AssetAiError::Io(format!("write {}: {error}", part.display()))
        })?;
        if let Ok(handle) = fs::OpenOptions::new().write(true).open(&part) {
            let _ = handle.sync_all();
        }
        #[cfg(windows)]
        if self.path.exists() {
            fs::remove_file(&self.path).map_err(|error| {
                AssetAiError::Io(format!("replace {}: {error}", self.path.display()))
            })?;
        }
        fs::rename(&part, &self.path).map_err(|error| {
            let _ = fs::remove_file(&part);
            AssetAiError::Io(format!(
                "rename {} to {}: {error}",
                part.display(),
                self.path.display()
            ))
        })?;
        Ok(())
    }
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Opened at an explicit path: sibling tests drive MAKEPAD_HOME themselves and
    // run in parallel, so touching the process environment here would race them.
    #[test]
    fn license_persistence_round_trip_at_an_explicit_path() {
        let root = std::env::temp_dir().join(format!(
            "makepad-ai-license-test-{}-{}",
            std::process::id(),
            unix_nanos()
        ));
        let path = root.join(LICENSE_ACKS_FILE);
        let mut store = LicenseStore::open_at(path.clone()).unwrap();
        assert_eq!(store.path(), path.as_path());
        assert!(!store.acknowledged("model-a", "licence-v1"));
        store.acknowledge("model-a", "licence-v1").unwrap();
        assert_eq!(store.records().len(), 1);
        let reopened = LicenseStore::open_at(path).unwrap();
        assert!(reopened.acknowledged("model-a", "licence-v1"));
        assert!(!reopened.acknowledged("model-a", "licence-v2"));
        let _ = fs::remove_dir_all(root);
    }
}
