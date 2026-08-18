//! Deterministic external source and pack import contracts.
//!
//! Kenney and every other stock or third-party pack enters through an explicit
//! pinned [`ImportManifest`], never a client filesystem scan. The manifest
//! pins the approved source, exact per-entry source digests, stable pack-entry
//! keys, media roles, and rights, so the same manifest on two clean servers
//! yields the same [`crate::id::ImportRevisionId`], asset IDs, revisions, and
//! blobs. Re-submitting is idempotent; a changed pack creates new immutable
//! records without editing old ones.
//!
//! Asset identity mapping is versioned explicit policy
//! ([`IMPORT_ASSET_ID_POLICY_V1`]): the asset ID is derived from
//! `{source id, pack name, entry key}` and deliberately NOT the pack version,
//! so a new pack version revises the same assets instead of forking them.

use crate::asset::{
    Anchor, AssetFile, AssetKind, AssetManifest, Capabilities, CoordinateSystem, Metrics,
    Provenance, Redistribution, Rights, SpawnRecipe, ThumbnailMeta,
};
use crate::codec::{canon_enum, check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::geom::Bounds;
use crate::id::{AssetAlias, AssetId, ImportRevisionId, PackEntryKey, SourceCollectionId};
use crate::limits::*;
use crate::sha256::Sha256;

/// Version tag of the deterministic `{source, pack, key} -> AssetId` mapping.
pub const IMPORT_ASSET_ID_POLICY_V1: u32 = 1;

/// Source and pack slugs use the alias-segment charset — lowercase, digits,
/// `-`, `_`, starting alphanumeric — because both become alias segments and
/// catalog namespaces. Kenney packs are canonically hyphenated ("space-kit").
pub(crate) fn check_slug(s: &str, what: &'static str) -> Result<(), AssetDataError> {
    if s.is_empty() || s.len() > MAX_ALIAS_SEGMENT_BYTES {
        return Err(AssetDataError::Malformed { what });
    }
    let bytes = s.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(AssetDataError::Malformed { what });
    }
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {}
            _ => return Err(AssetDataError::Malformed { what }),
        }
    }
    Ok(())
}

/// How a source's content bytes reach the server. v1 supports only bytes that
/// are already in CAS via authenticated upload — the server never fetches an
/// arbitrary URL (that would be an SSRF service).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceOrigin {
    Upload,
}

canon_enum!(SourceOrigin { Upload = 0 });

/// One approved external content source (for example the Kenney pack family).
/// Registering a collection is an administrative act; imports must name an
/// approved collection or they refuse. The collection's `terms` are the
/// AUTHORITATIVE rights for everything imported under it: an import manifest
/// must carry exactly these terms or it refuses — license, attribution,
/// redistribution, and derivative policy are never caller-selected.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceCollection {
    /// Machine name, also the catalog namespace imported assets land in.
    pub id: String,
    pub title: String,
    pub origin: SourceOrigin,
    /// The registered, authoritative rights of this source.
    pub terms: Rights,
}

impl SourceCollection {
    pub fn validate(&self) -> Result<(), AssetDataError> {
        check_slug(&self.id, "source collection id")?;
        if self.title.is_empty() || self.title.len() > MAX_NAME_BYTES * 2 {
            return Err(AssetDataError::Malformed {
                what: "source collection title",
            });
        }
        self.terms.validate()
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::SOURCE_COLLECTION);
        w.str(&self.id);
        w.str(&self.title);
        w.u8(self.origin.tag());
        self.terms.encode(&mut w);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::SOURCE_COLLECTION)?;
        let id = r.str("source collection id", MAX_NAME_BYTES)?;
        let title = r.str("source collection title", MAX_NAME_BYTES * 2)?;
        let origin = SourceOrigin::decode(&mut r)?;
        let terms = Rights::decode(&mut r)?;
        r.finish()?;
        let collection = Self {
            id,
            title,
            origin,
            terms,
        };
        collection.validate()?;
        Ok(collection)
    }

    pub fn digest(&self) -> Result<SourceCollectionId, AssetDataError> {
        Ok(SourceCollectionId::hash_of(&self.to_canonical_bytes()?))
    }
}

/// Windows reserved device names. A path segment equal to one — or whose
/// stem before the first dot is one — silently aliases a device on Windows
/// hosts, so it is refused everywhere.
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Canonical normalized archive-relative path of one pack entry file.
/// Lowercase, `/`-separated, no traversal, no absolute or hidden segments, no
/// Windows trailing-dot or reserved device segments.
pub(crate) fn check_pack_path(path: &str, what: &'static str) -> Result<(), AssetDataError> {
    if path.is_empty() || path.len() > MAX_PACK_PATH_BYTES {
        return Err(AssetDataError::Malformed { what });
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(AssetDataError::Malformed { what });
        }
        let bytes = segment.as_bytes();
        // Must start alphanumeric: refuses hidden-file and option-like names.
        if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
            return Err(AssetDataError::Malformed { what });
        }
        for &c in bytes {
            match c {
                b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {}
                _ => return Err(AssetDataError::Malformed { what }),
            }
        }
        // Windows strips trailing dots, which would give one entry two
        // on-disk spellings; refuse rather than normalize.
        if bytes[bytes.len() - 1] == b'.' {
            return Err(AssetDataError::Malformed { what });
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        if WINDOWS_RESERVED.contains(&stem) {
            return Err(AssetDataError::Malformed { what });
        }
    }
    Ok(())
}

/// One source file of one imported asset: the normalized pack path pinned to
/// an exact content digest, plus the typed role it plays in the produced
/// asset revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportFile {
    pub path: String,
    pub file: AssetFile,
}

impl ImportFile {
    fn validate(&self) -> Result<(), AssetDataError> {
        check_pack_path(&self.path, "import file path")
        // The wrapped AssetFile is validated by the produced AssetManifest.
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.path);
        self.file.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            path: r.str("import file path", MAX_PACK_PATH_BYTES)?,
            file: AssetFile::decode(r)?,
        })
    }
}

/// The pack preview image that becomes the mandatory mesh thumbnail.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportThumbnail {
    pub path: String,
    pub meta: ThumbnailMeta,
}

/// One asset an import produces: a stable entry key plus everything needed to
/// build its canonical [`AssetManifest`] deterministically.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportAsset {
    pub key: PackEntryKey,
    pub kind: AssetKind,
    /// Typed source files, canonical `(role, tier, lod)` order.
    pub files: Vec<ImportFile>,
    /// Mandatory for mesh-bearing kinds, exactly like the asset manifest.
    pub thumbnail: Option<ImportThumbnail>,
    pub metrics: Metrics,
    pub coordinate_system: CoordinateSystem,
    pub bounds: Bounds,
    pub anchors: Vec<Anchor>,
    pub capabilities: Capabilities,
    pub spawn_recipe: Option<SpawnRecipe>,
}

impl ImportAsset {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.key.as_str().split('/').count() > MAX_IMPORT_KEY_SEGMENTS {
            return Err(AssetDataError::OverBudget {
                what: "import key segments",
                limit: MAX_IMPORT_KEY_SEGMENTS as u64,
                found: self.key.as_str().split('/').count() as u64,
            });
        }
        if self.files.is_empty() {
            return Err(AssetDataError::Missing {
                what: "import asset files",
            });
        }
        if self.files.len() > MAX_IMPORT_FILES_PER_ASSET {
            return Err(AssetDataError::OverBudget {
                what: "import asset files",
                limit: MAX_IMPORT_FILES_PER_ASSET as u64,
                found: self.files.len() as u64,
            });
        }
        for f in &self.files {
            f.validate()?;
        }
        if let Some(t) = &self.thumbnail {
            check_pack_path(&t.path, "import thumbnail path")?;
        }
        Ok(())
    }

    fn encode(&self, w: &mut CanonWriter) {
        self.key.encode(w);
        w.u8(self.kind.tag());
        w.u32(self.files.len() as u32);
        for f in &self.files {
            f.encode(w);
        }
        match &self.thumbnail {
            None => w.bool(false),
            Some(t) => {
                w.bool(true);
                w.str(&t.path);
                t.meta.encode(w);
            }
        }
        self.metrics.encode(w);
        self.coordinate_system.encode(w);
        self.bounds.encode(w);
        w.u32(self.anchors.len() as u32);
        for a in &self.anchors {
            a.encode(w);
        }
        self.capabilities.encode(w);
        match &self.spawn_recipe {
            None => w.bool(false),
            Some(s) => {
                w.bool(true);
                s.encode(w);
            }
        }
    }

    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let key = PackEntryKey::decode(r)?;
        let kind = AssetKind::decode(r)?;
        let n = r.count("import asset files", MAX_IMPORT_FILES_PER_ASSET)?;
        let mut files = Vec::with_capacity(n);
        for _ in 0..n {
            files.push(ImportFile::decode(r)?);
        }
        let thumbnail = if r.bool("import thumbnail present")? {
            Some(ImportThumbnail {
                path: r.str("import thumbnail path", MAX_PACK_PATH_BYTES)?,
                meta: ThumbnailMeta::decode(r)?,
            })
        } else {
            None
        };
        let metrics = Metrics::decode(r)?;
        let coordinate_system = CoordinateSystem::decode(r)?;
        let bounds = Bounds::decode(r, "import asset bounds")?;
        let n = r.count("import asset anchors", MAX_ANCHORS_PER_ASSET)?;
        let mut anchors = Vec::with_capacity(n);
        for _ in 0..n {
            anchors.push(Anchor::decode(r)?);
        }
        let capabilities = Capabilities::decode(r)?;
        let spawn_recipe = if r.bool("import spawn recipe present")? {
            Some(SpawnRecipe::decode(r)?)
        } else {
            None
        };
        Ok(Self {
            key,
            kind,
            files,
            thumbnail,
            metrics,
            coordinate_system,
            bounds,
            anchors,
            capabilities,
            spawn_recipe,
        })
    }
}

/// The canonical, pinned import manifest for one external pack. Its digest is
/// the [`ImportRevisionId`] — the identity of this exact deterministic import.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportManifest {
    /// Digest of the approved [`SourceCollection`] this import runs under.
    pub source_collection: SourceCollectionId,
    /// The collection's machine id, repeated so asset identity derivation is
    /// self-contained. The server verifies it matches the registered
    /// collection before running anything.
    pub source_id: String,
    pub pack_name: String,
    pub pack_version: String,
    /// Asset-ID mapping policy version; unknown versions refuse.
    pub policy_version: u32,
    /// Canonical order by entry key.
    pub assets: Vec<ImportAsset>,
    /// Pack-level rights, inherited verbatim into every produced revision.
    /// Must equal the registered source collection's terms exactly — the
    /// server refuses an import whose manifest claims different terms.
    pub rights: Rights,
}

impl ImportManifest {
    pub fn canonicalize(&mut self) {
        self.assets.sort_by(|a, b| a.key.cmp(&b.key));
        for asset in &mut self.assets {
            asset.files.sort_by_key(|f| f.file.sort_key());
            asset.anchors.sort_by(|a, b| a.name.cmp(&b.name));
            if let Some(recipe) = &mut asset.spawn_recipe {
                recipe.params.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        check_slug(&self.source_id, "import source id")?;
        check_slug(&self.pack_name, "import pack name")?;
        check_pack_version(&self.pack_version)?;
        if self.policy_version != IMPORT_ASSET_ID_POLICY_V1 {
            return Err(AssetDataError::UnsupportedSchema {
                found: self.policy_version as u16,
            });
        }
        if self.assets.is_empty() {
            return Err(AssetDataError::Missing {
                what: "import assets",
            });
        }
        if self.assets.len() > MAX_IMPORT_ASSETS {
            return Err(AssetDataError::OverBudget {
                what: "import assets",
                limit: MAX_IMPORT_ASSETS as u64,
                found: self.assets.len() as u64,
            });
        }
        for a in &self.assets {
            a.validate()?;
        }
        check_sorted_unique(&self.assets, |a| a.key.clone(), "import asset key")?;
        // A pack file may be shared by several assets (Kenney packs share
        // texture sheets and previews) ONLY as the exact same canonical
        // object: identical path spelling AND identical role/tier/lod/media/
        // blob/length/dims (or identical thumbnail meta). One archive entry
        // can never mean two different things, and a file use can never alias
        // a thumbnail use. Case-colliding spellings cannot arise because
        // `check_pack_path` admits lowercase only; an uppercase spelling is
        // malformed outright, not a second spelling of the same entry.
        enum PathUse<'a> {
            File(&'a AssetFile),
            Thumb(&'a ThumbnailMeta),
        }
        let mut paths: Vec<(&str, PathUse)> = Vec::new();
        for a in &self.assets {
            for f in &a.files {
                paths.push((f.path.as_str(), PathUse::File(&f.file)));
            }
            if let Some(t) = &a.thumbnail {
                paths.push((t.path.as_str(), PathUse::Thumb(&t.meta)));
            }
        }
        paths.sort_by(|a, b| a.0.cmp(b.0));
        for pair in paths.windows(2) {
            if pair[0].0 != pair[1].0 {
                continue;
            }
            let same_object = match (&pair[0].1, &pair[1].1) {
                (PathUse::File(a), PathUse::File(b)) => a == b,
                (PathUse::Thumb(a), PathUse::Thumb(b)) => a == b,
                _ => false,
            };
            if !same_object {
                return Err(AssetDataError::Mismatch {
                    what: "shared import path",
                });
            }
        }
        self.rights.validate()?;
        // Content that may not be redistributed can never be imported into a
        // catalog that serves peers: refuse before any publication.
        if self.rights.redistribution == Redistribution::Forbidden {
            return Err(AssetDataError::Mismatch {
                what: "forbidden redistribution on import",
            });
        }
        // Every produced asset manifest must be valid and every publish alias
        // must fit the alias contract; the import is refused whole if any
        // entry would produce an invalid revision. The digest placeholder
        // does not affect structural validity.
        for a in &self.assets {
            self.alias_for(&a.key)?;
            self.asset_manifest_with(a, [0u8; 32])?.validate()?;
        }
        Ok(())
    }

    /// The deterministic catalog alias one entry publishes under:
    /// `{source_id}/{pack_name}/{key}`. Bounded by the alias contract; an
    /// entry whose alias cannot exist refuses at import validation, not at
    /// publication time.
    pub fn alias_for(&self, key: &PackEntryKey) -> Result<AssetAlias, AssetDataError> {
        AssetAlias::new(format!(
            "{}/{}/{}",
            self.source_id,
            self.pack_name,
            key.as_str()
        ))
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::IMPORT_MANIFEST);
        self.source_collection.encode(&mut w);
        w.str(&self.source_id);
        w.str(&self.pack_name);
        w.str(&self.pack_version);
        w.u32(self.policy_version);
        w.u32(self.assets.len() as u32);
        for a in &self.assets {
            a.encode(&mut w);
        }
        self.rights.encode(&mut w);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::IMPORT_MANIFEST)?;
        let source_collection = SourceCollectionId::decode(&mut r)?;
        let source_id = r.str("import source id", MAX_NAME_BYTES)?;
        let pack_name = r.str("import pack name", MAX_NAME_BYTES)?;
        let pack_version = r.str("import pack version", MAX_PACK_VERSION_BYTES)?;
        let policy_version = r.u32("import policy version")?;
        let n = r.count("import assets", MAX_IMPORT_ASSETS)?;
        let mut assets = Vec::with_capacity(n);
        for _ in 0..n {
            assets.push(ImportAsset::decode(&mut r)?);
        }
        let rights = Rights::decode(&mut r)?;
        r.finish()?;
        let manifest = Self {
            source_collection,
            source_id,
            pack_name,
            pack_version,
            policy_version,
            assets,
            rights,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// The immutable identity of this exact import.
    pub fn revision(&self) -> Result<ImportRevisionId, AssetDataError> {
        Ok(ImportRevisionId::hash_of(&self.to_canonical_bytes()?))
    }

    /// The deterministic asset identity of one entry under policy v1. Pack
    /// version is deliberately excluded: a new pack version revises the same
    /// assets rather than forking their identity.
    pub fn asset_id_for(&self, key: &PackEntryKey) -> AssetId {
        let mut h = Sha256::new();
        h.update(b"mpc1:import-asset-id:v1\0");
        h.update(self.source_id.as_bytes());
        h.update(b"\0");
        h.update(self.pack_name.as_bytes());
        h.update(b"\0");
        h.update(key.as_str().as_bytes());
        let digest = h.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        AssetId::from_bytes(id)
    }

    /// Build the canonical asset manifest one entry produces.
    /// `import_revision` is this manifest's own digest; the produced revision
    /// pins it through `provenance.params_digest`, while `provenance.model`/
    /// `version` carry the collection/pack locator. Rights are inherited
    /// VERBATIM — the upstream source URL, terms digest, and archive digest
    /// survive; internal locators never overwrite upstream provenance.
    pub fn asset_manifest_for(
        &self,
        asset: &ImportAsset,
        import_revision: &ImportRevisionId,
    ) -> Result<AssetManifest, AssetDataError> {
        let manifest = self.asset_manifest_with(asset, *import_revision.as_bytes())?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn asset_manifest_with(
        &self,
        asset: &ImportAsset,
        params_digest: [u8; 32],
    ) -> Result<AssetManifest, AssetDataError> {
        let mut model = format!("{}/{}", self.source_id, self.pack_name);
        model.truncate(MAX_NAME_BYTES);
        Ok(AssetManifest {
            asset_id: self.asset_id_for(&asset.key),
            kind: asset.kind,
            files: asset.files.iter().map(|f| f.file).collect(),
            dependencies: Vec::new(),
            thumbnail: asset.thumbnail.as_ref().map(|t| t.meta),
            metrics: asset.metrics,
            coordinate_system: asset.coordinate_system,
            bounds: asset.bounds,
            anchors: asset.anchors.clone(),
            capabilities: asset.capabilities,
            spawn_recipe: asset.spawn_recipe.clone(),
            provenance: Some(Provenance {
                generator: "import".into(),
                model,
                version: self.pack_version.clone(),
                seed: 0,
                parents: Vec::new(),
                params_digest: Some(params_digest),
            }),
            rights: self.rights.clone(),
        })
    }
}

/// Pack version: bounded `[a-z0-9._-]`, starting alphanumeric.
fn check_pack_version(version: &str) -> Result<(), AssetDataError> {
    let what = "import pack version";
    if version.is_empty() || version.len() > MAX_PACK_VERSION_BYTES {
        return Err(AssetDataError::Malformed { what });
    }
    let bytes = version.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(AssetDataError::Malformed { what });
    }
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' => {}
            _ => return Err(AssetDataError::Malformed { what }),
        }
    }
    Ok(())
}
