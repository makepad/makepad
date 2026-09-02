//! Deterministic public static snapshots at the Asset Client's wire paths.
//!
//! Enumeration, manifests, variants, and blobs all travel through the
//! public [`AssetServerCore`] API. Embedded generation provenance is removed
//! by rewriting the complete dependency graph; legally material rights are
//! retained unchanged.

use crate::error::{io_err, ServerError, ServerResult};
use crate::host::api::{asset_manifest_value, kind_str, media_str, role_str};
use crate::host::json::{obj, s, Value};
use crate::static_export_core::{brotli_bytes, ExportEntry, ExportPlan, ExportSink, ExportStep};
use crate::{
    AssetServerCore, CandidateState, PublicExportAsset, PublicExportFilter,
};
use makepad_asset_data::{
    sha256, sha256_hex, AssetFile, AssetId, AssetKind, AssetManifest, AssetRevisionId,
    AssetRevisionRef, BlobId, DerivedVariantId, DerivedVariantManifest, FileRole, MediaType,
    Redistribution, VariantSetId, VariantSetManifest,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const STATIC_FORMAT_VERSION: u16 = 1;
const DEFAULT_VIDEO_CAP: u64 = 32 * 1024 * 1024;
const VARIANT_PAGE: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticExportOptions {
    pub namespace: Option<String>,
    pub kind: Option<AssetKind>,
    pub limit: Option<u64>,
    pub max_bytes_per_asset: u64,
    pub max_total_bytes: u64,
    /// MP4/video payloads at or below this many bytes are eligible. Zero
    /// excludes all video payloads while retaining thumbnails and stills.
    pub include_video_up_to: u64,
}

impl Default for StaticExportOptions {
    fn default() -> Self {
        Self {
            namespace: None,
            kind: None,
            limit: None,
            max_bytes_per_asset: u64::MAX,
            max_total_bytes: u64::MAX,
            include_video_up_to: DEFAULT_VIDEO_CAP,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticExportReport {
    pub snapshot_id: String,
    pub server_id: String,
    pub assets: u64,
    pub revisions: u64,
    pub aliases: u64,
    pub blobs_present: u64,
    pub blobs_omitted: u64,
    pub unique_blob_bytes: u64,
    pub excluded_rights: u64,
    pub excluded_budget: u64,
    pub excluded_kind_mismatch: u64,
}

#[derive(Clone)]
struct RootAsset {
    row: PublicExportAsset,
}

#[derive(Clone)]
struct RewrittenRevision {
    id: AssetRevisionId,
    manifest: AssetManifest,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct RewrittenVariant {
    id: DerivedVariantId,
    manifest: DerivedVariantManifest,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct RewrittenSet {
    base_old: AssetRevisionId,
    id: VariantSetId,
    bytes: Vec<u8>,
    variants: Vec<RewrittenVariant>,
}

#[derive(Clone, Default)]
struct BlobPlan {
    byte_len: u64,
    media: BTreeSet<String>,
    roles: BTreeSet<String>,
    owners: BTreeSet<AssetId>,
    groups: BTreeSet<String>,
    mandatory: bool,
    present: bool,
    reason: Option<&'static str>,
}

#[derive(Clone)]
struct FileRecord {
    path: String,
    byte_len: u64,
    sha256: String,
    content_type: &'static str,
    encoding: Option<&'static str>,
}

struct Output<'a> {
    sink: &'a mut dyn ExportSink,
    files: BTreeMap<String, FileRecord>,
}

impl<'a> Output<'a> {
    fn new(sink: &'a mut dyn ExportSink) -> Self {
        Self { sink, files: BTreeMap::new() }
    }

    fn route(
        &mut self,
        path: &str,
        bytes: &[u8],
        content_type: &'static str,
        compress: bool,
    ) -> ServerResult<()> {
        if !path.starts_with("/v1/") || path.contains("//") || path.contains("..") {
            return Err(ServerError::InvalidInput { what: "static export path" });
        }
        self.write(path, bytes, content_type, None)?;
        if compress {
            let compressed = brotli_bytes(bytes)?;
            self.write(&format!("{path}.br"), &compressed, content_type, Some("br"))?;
        }
        Ok(())
    }

    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        content_type: &'static str,
        encoding: Option<&'static str>,
    ) -> ServerResult<()> {
        let entry = ExportEntry {
            path: path.to_string(),
            bytes: bytes.to_vec(),
            content_type,
            content_encoding: encoding,
        };
        if let Some(old) = self.files.get(path) {
            if old.byte_len == bytes.len() as u64 && old.sha256 == sha256_hex(bytes) {
                return Ok(());
            }
            return Err(ServerError::Conflict { what: "static export route" });
        }
        drive_export_entry(self.sink, entry.clone())?;
        self.files.insert(
            path.to_string(),
            FileRecord {
                path: path.to_string(),
                byte_len: bytes.len() as u64,
                sha256: sha256_hex(bytes),
                content_type: entry.content_type,
                encoding: entry.content_encoding,
            },
        );
        Ok(())
    }

    fn write_untracked(&mut self, entry: ExportEntry) -> ServerResult<()> {
        drive_export_entry(self.sink, entry)
    }
}

fn drive_export_entry(sink: &mut dyn ExportSink, entry: ExportEntry) -> ServerResult<()> {
    let mut plan = ExportPlan::default();
    plan.push(entry);
    while matches!(plan.step(sink)?, ExportStep::Pending { .. }) {}
    Ok(())
}

struct FileSink {
    root: PathBuf,
}

impl ExportSink for FileSink {
    fn write_entry(&mut self, entry: &ExportEntry) -> ServerResult<()> {
        let path = entry.path.as_str();
        let bytes = entry.bytes.as_slice();
        let relative = path.strip_prefix('/').ok_or(ServerError::InvalidInput {
            what: "static export relative path",
        })?;
        let disk = self.root.join(relative);
        let parent = disk.parent().ok_or(ServerError::InvalidInput {
            what: "static export file parent",
        })?;
        std::fs::create_dir_all(parent).map_err(io_err("static export create directory"))?;
        std::fs::write(&disk, bytes).map_err(io_err("static export write file"))?;
        Ok(())
    }
}

fn rights_public(manifest: &AssetManifest) -> bool {
    matches!(
        manifest.rights.redistribution,
        Redistribution::Allowed | Redistribution::AttributionRequired
    )
}

fn load_revision(
    core: &AssetServerCore,
    target: AssetRevisionRef,
    depth: u32,
    visiting: &mut BTreeSet<AssetRevisionId>,
    denied: &mut BTreeSet<AssetRevisionId>,
    graph: &mut BTreeMap<AssetRevisionId, AssetManifest>,
) -> ServerResult<bool> {
    if graph.contains_key(&target.revision) {
        return Ok(true);
    }
    if denied.contains(&target.revision) {
        return Ok(false);
    }
    if depth > makepad_asset_data::limits::MAX_DEPENDENCY_DEPTH {
        return Err(ServerError::OverBudget {
            what: "static export dependency depth",
            limit: makepad_asset_data::limits::MAX_DEPENDENCY_DEPTH as u64,
            found: depth as u64,
        });
    }
    if !visiting.insert(target.revision) {
        return Err(ServerError::InvalidState {
            what: "static export dependency graph",
            state: "cycle",
        });
    }
    let bytes = core
        .catalog()
        .asset_revision_manifest(&target.revision)?
        .ok_or(ServerError::NotFound { what: "static export revision" })?;
    let found = AssetRevisionId::hash_of(&bytes);
    if found != target.revision {
        return Err(ServerError::DigestMismatch {
            what: "static export revision",
            expected: *target.revision.as_bytes(),
            found: *found.as_bytes(),
        });
    }
    let manifest = AssetManifest::from_canonical_bytes(&bytes)?;
    if manifest.asset_id != target.asset_id {
        return Err(ServerError::Conflict { what: "static export dependency asset" });
    }
    if !rights_public(&manifest) {
        visiting.remove(&target.revision);
        denied.insert(target.revision);
        return Ok(false);
    }
    for dependency in &manifest.dependencies {
        if !load_revision(core, *dependency, depth + 1, visiting, denied, graph)? {
            visiting.remove(&target.revision);
            denied.insert(target.revision);
            return Ok(false);
        }
    }
    visiting.remove(&target.revision);
    graph.insert(target.revision, manifest);
    Ok(true)
}

fn rewrite_revision(
    old: AssetRevisionId,
    graph: &BTreeMap<AssetRevisionId, AssetManifest>,
    visiting: &mut BTreeSet<AssetRevisionId>,
    rewritten: &mut BTreeMap<AssetRevisionId, RewrittenRevision>,
) -> ServerResult<AssetRevisionId> {
    if let Some(done) = rewritten.get(&old) {
        return Ok(done.id);
    }
    if !visiting.insert(old) {
        return Err(ServerError::InvalidState {
            what: "static export rewrite graph",
            state: "cycle",
        });
    }
    let mut manifest = graph
        .get(&old)
        .cloned()
        .ok_or(ServerError::NotFound { what: "static export rewrite revision" })?;
    for dependency in &mut manifest.dependencies {
        dependency.revision = rewrite_revision(dependency.revision, graph, visiting, rewritten)?;
    }
    manifest.provenance = None;
    manifest.canonicalize();
    let bytes = manifest.to_canonical_bytes()?;
    let id = AssetRevisionId::hash_of(&bytes);
    visiting.remove(&old);
    rewritten.insert(old, RewrittenRevision { id, manifest, bytes });
    Ok(id)
}

fn closure(
    heads: impl Iterator<Item = AssetRevisionId>,
    graph: &BTreeMap<AssetRevisionId, AssetManifest>,
) -> BTreeSet<AssetRevisionId> {
    fn visit(
        revision: AssetRevisionId,
        graph: &BTreeMap<AssetRevisionId, AssetManifest>,
        out: &mut BTreeSet<AssetRevisionId>,
    ) {
        if !out.insert(revision) {
            return;
        }
        if let Some(manifest) = graph.get(&revision) {
            for dependency in &manifest.dependencies {
                visit(dependency.revision, graph, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    for head in heads {
        visit(head, graph, &mut out);
    }
    out
}

fn discover_variants(
    core: &AssetServerCore,
    graph: &BTreeMap<AssetRevisionId, AssetManifest>,
    rewritten: &BTreeMap<AssetRevisionId, RewrittenRevision>,
) -> ServerResult<Vec<RewrittenSet>> {
    let mut sets = Vec::new();
    for (base_old, base_manifest) in graph {
        let base_new = rewritten
            .get(base_old)
            .ok_or(ServerError::NotFound { what: "static export rewritten base" })?
            .id;
        let base = AssetRevisionRef { asset_id: base_manifest.asset_id, revision: *base_old };
        let mut after = None;
        loop {
            let page = core
                .variants()
                .variant_sets_for_base(&base, after.as_ref(), VARIANT_PAGE)?;
            if page.is_empty() {
                break;
            }
            for set_id in &page {
                let bytes = core
                    .variants()
                    .variant_set_manifest(set_id)?
                    .ok_or(ServerError::NotFound { what: "static export variant set" })?;
                if VariantSetId::hash_of(&bytes) != *set_id {
                    return Err(ServerError::DigestMismatch {
                        what: "static export variant set",
                        expected: *set_id.as_bytes(),
                        found: *VariantSetId::hash_of(&bytes).as_bytes(),
                    });
                }
                let original = VariantSetManifest::from_canonical_bytes(&bytes)?;
                if original.base != base {
                    return Err(ServerError::Conflict { what: "static export variant base" });
                }
                let mut variants = Vec::new();
                for variant_id in original.variants {
                    let bytes = core
                        .variants()
                        .variant_manifest(&variant_id)?
                        .ok_or(ServerError::NotFound { what: "static export variant" })?;
                    let found = DerivedVariantId::hash_of(&bytes);
                    if found != variant_id {
                        return Err(ServerError::DigestMismatch {
                            what: "static export variant",
                            expected: *variant_id.as_bytes(),
                            found: *found.as_bytes(),
                        });
                    }
                    let mut manifest = DerivedVariantManifest::from_canonical_bytes(&bytes)?;
                    if !matches!(
                        manifest.rights.redistribution,
                        Redistribution::Allowed | Redistribution::AttributionRequired
                    ) {
                        continue;
                    }
                    manifest.base.revision = base_new;
                    manifest.canonicalize();
                    let bytes = manifest.to_canonical_bytes()?;
                    variants.push(RewrittenVariant {
                        id: DerivedVariantId::hash_of(&bytes),
                        manifest,
                        bytes,
                    });
                }
                if variants.is_empty() {
                    continue;
                }
                variants.sort_by_key(|variant| variant.id);
                variants.dedup_by_key(|variant| variant.id);
                let mut manifest = VariantSetManifest {
                    base: AssetRevisionRef { asset_id: base.asset_id, revision: base_new },
                    variants: variants.iter().map(|variant| variant.id).collect(),
                    policy_version: original.policy_version,
                };
                manifest.canonicalize();
                let bytes = manifest.to_canonical_bytes()?;
                sets.push(RewrittenSet {
                    base_old: *base_old,
                    id: VariantSetId::hash_of(&bytes),
                    bytes,
                    variants,
                });
            }
            if page.len() < VARIANT_PAGE as usize {
                break;
            }
            after = page.last().copied();
        }
    }
    sets.sort_by_key(|set| (set.base_old, set.id));
    sets.dedup_by_key(|set| set.id);
    Ok(sets)
}

fn direct_mandatory_blobs(manifest: &AssetManifest) -> BTreeMap<BlobId, u64> {
    let mut out = BTreeMap::new();
    if let Some(thumbnail) = &manifest.thumbnail {
        out.insert(thumbnail.blob, thumbnail.byte_len);
    }
    for file in &manifest.files {
        if matches!(file.role, FileRole::PreviewFront | FileRole::PreviewSide) {
            out.insert(file.blob, file.byte_len);
        }
    }
    out
}

fn root_mandatory_blobs(
    revisions: &BTreeSet<AssetRevisionId>,
    graph: &BTreeMap<AssetRevisionId, AssetManifest>,
    variants: &[RewrittenSet],
) -> ServerResult<BTreeMap<BlobId, u64>> {
    let mut out = BTreeMap::new();
    for revision in revisions {
        for (blob, size) in direct_mandatory_blobs(&graph[revision]) {
            insert_sized(&mut out, blob, size)?;
        }
    }
    for set in variants.iter().filter(|set| revisions.contains(&set.base_old)) {
        for variant in &set.variants {
            if let Some(thumbnail) = &variant.manifest.thumbnail {
                insert_sized(&mut out, thumbnail.blob, thumbnail.byte_len)?;
            }
            for output in &variant.manifest.outputs {
                if matches!(output.role, FileRole::PreviewFront | FileRole::PreviewSide) {
                    insert_sized(&mut out, output.blob, output.byte_len)?;
                }
            }
        }
    }
    Ok(out)
}

fn insert_sized(
    map: &mut BTreeMap<BlobId, u64>,
    blob: BlobId,
    size: u64,
) -> ServerResult<()> {
    if let Some(old) = map.insert(blob, size) {
        if old != size {
            return Err(ServerError::SizeMismatch {
                what: "static export duplicate blob declaration",
                expected: old,
                found: size,
            });
        }
    }
    Ok(())
}

fn add_blob(
    plans: &mut BTreeMap<BlobId, BlobPlan>,
    blob: BlobId,
    byte_len: u64,
    media: &str,
    role: &str,
    owners: &BTreeSet<AssetId>,
    mandatory: bool,
    policy_reason: Option<&'static str>,
    group: Option<String>,
) -> ServerResult<()> {
    let plan = plans.entry(blob).or_default();
    let seen = !plan.roles.is_empty();
    if plan.byte_len != 0 && plan.byte_len != byte_len {
        return Err(ServerError::SizeMismatch {
            what: "static export blob declaration",
            expected: plan.byte_len,
            found: byte_len,
        });
    }
    plan.byte_len = byte_len;
    plan.media.insert(media.to_string());
    plan.roles.insert(role.to_string());
    plan.owners.extend(owners.iter().copied());
    if let Some(group) = group {
        plan.groups.insert(group);
    }
    plan.mandatory |= mandatory;
    if mandatory {
        plan.reason = None;
    } else if !seen {
        plan.reason = policy_reason;
    } else if policy_reason.is_none() {
        plan.reason = None;
    }
    Ok(())
}

fn is_video(file: &AssetFile) -> bool {
    matches!(file.role, FileRole::Video | FileRole::Turntable)
        || (file.role == FileRole::Source && file.media == MediaType::Mp4)
}

fn file_policy(
    file: &AssetFile,
    only_source: bool,
    video_cap: u64,
) -> Option<&'static str> {
    if is_video(file) && file.byte_len > video_cap {
        Some("video_cap")
    } else if file.role == FileRole::Source && !only_source {
        Some("source_policy")
    } else {
        None
    }
}

fn role_priority(role: &str) -> u8 {
    [
        "render_glb", "lod1_glb", "lod2_glb", "collider", "ao_mesh",
        "shadow_sdf", "albedo", "normal", "orm", "texture", "preview_front",
        "preview_side", "audio", "depth", "splat", "ao_texture", "lyrics",
        "stem_drums", "stem_bass", "stem_vocals", "stem_other", "video",
        "turntable", "source", "thumbnail",
    ]
    .iter()
    .position(|candidate| *candidate == role)
    .unwrap_or(255) as u8
}

fn option_u64(value: u64) -> Value {
    if value == u64::MAX {
        Value::Null
    } else {
        Value::Int(value as i64)
    }
}

fn values(items: &[String]) -> Value {
    Value::Arr(items.iter().cloned().map(s).collect())
}

fn file_values(files: &BTreeMap<String, FileRecord>) -> Value {
    Value::Arr(
        files
            .values()
            .map(|file| {
                obj(vec![
                    ("path", s(file.path.clone())),
                    ("byte_len", Value::Int(file.byte_len as i64)),
                    ("sha256", s(file.sha256.clone())),
                    ("content_type", s(file.content_type)),
                    (
                        "content_encoding",
                        match file.encoding {
                            Some(value) => s(value),
                            None => Value::Null,
                        },
                    ),
                ])
            })
            .collect(),
    )
}

fn hex16(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Export a public, immutable snapshot into a newly-created directory.
/// Existing destinations are refused so publication cannot destroy unrelated
/// files. The completed staging directory is renamed into place in one step.
pub fn export_static(
    core: &AssetServerCore,
    out_dir: &Path,
    options: &StaticExportOptions,
) -> ServerResult<StaticExportReport> {
    if let Some(namespace) = options.namespace.as_deref() {
        crate::validate_namespace(namespace)?;
    }
    if options.limit == Some(0) {
        return Err(ServerError::InvalidInput { what: "static export limit" });
    }
    if options.limit.is_some_and(|value| value > i64::MAX as u64)
        || (options.max_bytes_per_asset != u64::MAX
            && options.max_bytes_per_asset > i64::MAX as u64)
        || (options.max_total_bytes != u64::MAX && options.max_total_bytes > i64::MAX as u64)
        || options.include_video_up_to > i64::MAX as u64
    {
        return Err(ServerError::InvalidInput {
            what: "static export numeric range",
        });
    }
    if out_dir.exists() {
        return Err(ServerError::Conflict { what: "static export output exists" });
    }
    let name = out_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or(ServerError::InvalidInput { what: "static export output directory" })?;
    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    let staging = parent.join(format!(".{name}.static-export-{}", std::process::id()));
    if staging.exists() {
        return Err(ServerError::Conflict { what: "static export staging exists" });
    }
    std::fs::create_dir_all(&staging).map_err(io_err("static export create staging"))?;
    let mut sink = FileSink { root: staging.clone() };
    let result = export_into(core, &mut sink, options);
    match result {
        Ok(report) => {
            if let Err(error) = std::fs::rename(&staging, out_dir) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(io_err("static export publish")(error));
            }
            Ok(report)
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

fn export_into(
    core: &AssetServerCore,
    sink: &mut dyn ExportSink,
    options: &StaticExportOptions,
) -> ServerResult<StaticExportReport> {
    let mut roots = Vec::new();
    let mut after = None;
    let wanted = options.limit.unwrap_or(u64::MAX);
    while (roots.len() as u64) < wanted {
        let page_size = core
            .budgets()
            .max_search_results
            .min((wanted - roots.len() as u64).min(u32::MAX as u64) as u32)
            .max(1);
        let page = core.public_export_page(PublicExportFilter {
            namespace: options.namespace.as_deref(),
            kind: options.kind,
            after,
            limit: page_size,
        })?;
        roots.extend(page.assets.into_iter().map(|row| RootAsset { row }));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }

    let mut graph = BTreeMap::new();
    let mut denied = BTreeSet::new();
    let mut allowed_roots = Vec::new();
    let mut report = StaticExportReport::default();
    for root in roots {
        let mut allowed = true;
        let mut kind_mismatch = false;
        for alias in &root.row.aliases {
            if !load_revision(
                core,
                alias.target,
                0,
                &mut BTreeSet::new(),
                &mut denied,
                &mut graph,
            )? {
                allowed = false;
                break;
            }
            if let Some(kind) = options.kind {
                if graph[&alias.target.revision].kind != kind {
                    kind_mismatch = true;
                    allowed = false;
                    break;
                }
            }
        }
        if allowed {
            allowed_roots.push(root);
        } else if kind_mismatch {
            report.excluded_kind_mismatch += 1;
        } else {
            report.excluded_rights += 1;
        }
    }

    let mut rewritten = BTreeMap::new();
    for revision in graph.keys().copied().collect::<Vec<_>>() {
        rewrite_revision(revision, &graph, &mut BTreeSet::new(), &mut rewritten)?;
    }
    let variants = discover_variants(core, &graph, &rewritten)?;

    let mut mandatory_global: BTreeMap<BlobId, u64> = BTreeMap::new();
    let mut kept_roots = Vec::new();
    for root in allowed_roots {
        let revisions = closure(root.row.aliases.iter().map(|alias| alias.target.revision), &graph);
        let mandatory = root_mandatory_blobs(&revisions, &graph, &variants)?;
        let per_asset = mandatory.values().try_fold(0u64, |sum, value| sum.checked_add(*value));
        let Some(per_asset) = per_asset else {
            return Err(ServerError::OverBudget {
                what: "static export asset bytes",
                limit: options.max_bytes_per_asset,
                found: u64::MAX,
            });
        };
        let additional = mandatory
            .iter()
            .filter(|(blob, _)| !mandatory_global.contains_key(blob))
            .try_fold(0u64, |sum, (_, size)| sum.checked_add(*size));
        let Some(additional) = additional else {
            return Err(ServerError::OverBudget {
                what: "static export total bytes",
                limit: options.max_total_bytes,
                found: u64::MAX,
            });
        };
        let global_now = mandatory_global.values().copied().sum::<u64>();
        if per_asset > options.max_bytes_per_asset
            || additional > options.max_total_bytes.saturating_sub(global_now)
        {
            report.excluded_budget += 1;
            continue;
        }
        for (blob, size) in mandatory {
            insert_sized(&mut mandatory_global, blob, size)?;
        }
        kept_roots.push(root);
    }

    let reachable = closure(
        kept_roots
            .iter()
            .flat_map(|root| root.row.aliases.iter().map(|alias| alias.target.revision)),
        &graph,
    );
    let variants: Vec<RewrittenSet> = variants
        .into_iter()
        .filter(|set| reachable.contains(&set.base_old))
        .collect();

    let mut revision_owners: BTreeMap<AssetRevisionId, BTreeSet<AssetId>> = BTreeMap::new();
    for root in &kept_roots {
        let own = closure(root.row.aliases.iter().map(|alias| alias.target.revision), &graph);
        for revision in own {
            revision_owners.entry(revision).or_default().insert(root.row.asset_id);
        }
    }

    let mut plans = BTreeMap::new();
    for revision in &reachable {
        let manifest = &graph[revision];
        let owners = &revision_owners[revision];
        if let Some(thumbnail) = &manifest.thumbnail {
            add_blob(
                &mut plans,
                thumbnail.blob,
                thumbnail.byte_len,
                match thumbnail.media {
                    makepad_asset_data::ThumbnailMedia::Png => "png",
                    makepad_asset_data::ThumbnailMedia::Jpeg => "jpeg",
                },
                "thumbnail",
                owners,
                true,
                None,
                None,
            )?;
        }
        let only_source = !manifest.files.is_empty()
            && manifest.files.iter().all(|file| file.role == FileRole::Source);
        for file in &manifest.files {
            let mandatory = matches!(file.role, FileRole::PreviewFront | FileRole::PreviewSide);
            add_blob(
                &mut plans,
                file.blob,
                file.byte_len,
                media_str(file.media),
                role_str(file.role),
                owners,
                mandatory,
                file_policy(file, only_source, options.include_video_up_to),
                file.role.is_stem().then(|| format!("asset-stems:{revision}")),
            )?;
        }
    }
    for set in &variants {
        let owners = &revision_owners[&set.base_old];
        for variant in &set.variants {
            if let Some(thumbnail) = &variant.manifest.thumbnail {
                add_blob(
                    &mut plans,
                    thumbnail.blob,
                    thumbnail.byte_len,
                    match thumbnail.media {
                        makepad_asset_data::ThumbnailMedia::Png => "png",
                        makepad_asset_data::ThumbnailMedia::Jpeg => "jpeg",
                    },
                    "thumbnail",
                    owners,
                    true,
                    None,
                    None,
                )?;
            }
            for file in &variant.manifest.outputs {
                let mandatory = matches!(file.role, FileRole::PreviewFront | FileRole::PreviewSide);
                add_blob(
                    &mut plans,
                    file.blob,
                    file.byte_len,
                    media_str(file.media),
                    role_str(file.role),
                    owners,
                    mandatory,
                    file_policy(file, false, options.include_video_up_to),
                    file.role
                        .is_stem()
                        .then(|| format!("variant-stems:{}", variant.id)),
                )?;
            }
        }
    }

    let mut owner_bytes: BTreeMap<AssetId, u64> = kept_roots
        .iter()
        .map(|root| (root.row.asset_id, 0))
        .collect();
    let mut total_bytes = 0u64;
    for plan in plans.values_mut().filter(|plan| plan.mandatory) {
        plan.present = true;
        total_bytes = total_bytes.saturating_add(plan.byte_len);
        for owner in &plan.owners {
            let count = owner_bytes.entry(*owner).or_default();
            *count = count.saturating_add(plan.byte_len);
        }
    }
    // Optional files allocate as deterministic units. The four audio stems
    // of one manifest share a unit, preserving the content contract's
    // all-or-none side-channel guarantee under either size budget.
    let mut units: BTreeMap<String, BTreeSet<BlobId>> = BTreeMap::new();
    for (blob, plan) in &plans {
        if plan.mandatory {
            continue;
        }
        if plan.groups.is_empty() {
            units.entry(format!("blob:{blob}")).or_default().insert(*blob);
        } else {
            for group in &plan.groups {
                units.entry(group.clone()).or_default().insert(*blob);
            }
        }
    }
    let mut units: Vec<_> = units.into_iter().collect();
    units.sort_by_key(|(name, blobs)| {
        let priority = blobs
            .iter()
            .flat_map(|blob| plans[blob].roles.iter())
            .map(|role| role_priority(role))
            .min()
            .unwrap_or(255);
        (priority, name.clone())
    });
    for (_, blobs) in units {
        let pending: Vec<_> = blobs
            .into_iter()
            .filter(|blob| !plans[blob].present)
            .collect();
        if pending.is_empty() {
            continue;
        }
        if let Some(reason) = pending.iter().find_map(|blob| plans[blob].reason) {
            for blob in pending {
                plans.get_mut(&blob).unwrap().reason = Some(reason);
            }
            continue;
        }
        let additional = pending.iter().map(|blob| plans[blob].byte_len).sum::<u64>();
        let mut owner_add: BTreeMap<AssetId, u64> = BTreeMap::new();
        for blob in &pending {
            let plan = &plans[blob];
            for owner in &plan.owners {
                let count = owner_add.entry(*owner).or_default();
                *count = count.saturating_add(plan.byte_len);
            }
        }
        let asset_ok = owner_add.iter().all(|(owner, additional)| {
            owner_bytes[owner].saturating_add(*additional) <= options.max_bytes_per_asset
        });
        let total_ok = total_bytes.saturating_add(additional) <= options.max_total_bytes;
        if !asset_ok || !total_ok {
            let reason = if !asset_ok { "asset_budget" } else { "total_budget" };
            for blob in pending {
                plans.get_mut(&blob).unwrap().reason = Some(reason);
            }
            continue;
        }
        total_bytes += additional;
        for (owner, additional) in owner_add {
            *owner_bytes.entry(owner).or_default() += additional;
        }
        for blob in pending {
            plans.get_mut(&blob).unwrap().present = true;
        }
    }

    let mut output = Output::new(sink);
    let mut generated_ms = 0u64;
    for root in &kept_roots {
        generated_ms = generated_ms.max(root.row.created_ms).max(root.row.search.updated_ms);
        for alias in &root.row.aliases {
            generated_ms = generated_ms.max(alias.updated_ms).max(alias.published_ms);
        }
    }

    let mut alias_values = Vec::new();
    let mut asset_values = Vec::new();
    let mut search_values = Vec::new();
    let mut aliases: Vec<&str> = kept_roots
        .iter()
        .flat_map(|root| root.row.aliases.iter().map(|alias| alias.alias.as_str()))
        .collect();
    aliases.sort_unstable();
    for pair in aliases.windows(2) {
        if pair[1].starts_with(pair[0])
            && pair[1].as_bytes().get(pair[0].len()) == Some(&b'/')
        {
            return Err(ServerError::Conflict {
                what: "static export alias path prefix",
            });
        }
    }
    for root in &kept_roots {
        let mut revisions = BTreeSet::new();
        for alias in &root.row.aliases {
            let export_revision = rewritten[&alias.target.revision].id;
            revisions.insert(export_revision);
            let alias_value = obj(vec![
                ("alias", s(alias.alias.to_string())),
                ("asset_id", s(root.row.asset_id.to_string())),
                ("head_revision", s(export_revision.to_string())),
            ]);
            output.route(
                &format!("/v1/aliases/{}", alias.alias),
                alias_value.to_json().as_bytes(),
                "application/json",
                true,
            )?;
            alias_values.push(obj(vec![
                ("alias", s(alias.alias.to_string())),
                ("asset_id", s(root.row.asset_id.to_string())),
                ("head_revision", s(export_revision.to_string())),
                ("updated_ms", Value::Int(alias.updated_ms as i64)),
            ]));
            if let Some(thumbnail) = &rewritten[&alias.target.revision].manifest.thumbnail {
                if plans[&thumbnail.blob].present {
                    let bytes = core.read_blob(&thumbnail.blob)?;
                    output.route(
                        &format!("/v1/thumbnails/alias/{}", alias.alias),
                        &bytes,
                        match thumbnail.media {
                            makepad_asset_data::ThumbnailMedia::Png => "image/png",
                            makepad_asset_data::ThumbnailMedia::Jpeg => "image/jpeg",
                        },
                        false,
                    )?;
                }
            }
        }
        asset_values.push(obj(vec![
            ("asset_id", s(root.row.asset_id.to_string())),
            ("namespace", s(root.row.namespace.clone())),
            ("created_ms", Value::Int(root.row.created_ms as i64)),
            ("revisions", Value::Arr(revisions.iter().map(|revision| s(revision.to_string())).collect())),
        ]));
        let candidates = root
            .row
            .aliases
            .iter()
            .map(|alias| {
                let row = core
                    .catalog()
                    .asset_candidates(&root.row.asset_id, 512)?
                    .into_iter()
                    .find(|candidate| candidate.revision == alias.target.revision)
                    .ok_or(ServerError::NotFound { what: "static export candidate" })?;
                if row.state != CandidateState::Published {
                    return Err(ServerError::InvalidState {
                        what: "static export candidate",
                        state: "not published",
                    });
                }
                Ok(obj(vec![
                    ("revision", s(rewritten[&alias.target.revision].id.to_string())),
                    ("state", s("published")),
                    ("staged_ms", Value::Int(row.staged_ms as i64)),
                    ("published_ms", Value::Int(row.published_ms.unwrap_or(0) as i64)),
                    ("quarantined_ms", Value::Null),
                    ("retired_ms", Value::Null),
                ]))
            })
            .collect::<ServerResult<Vec<_>>>()?;
        let mut candidates = candidates;
        candidates.sort_by(|a, b| a.to_json().cmp(&b.to_json()));
        candidates.dedup();
        let detail = obj(vec![
            ("asset_id", s(root.row.asset_id.to_string())),
            ("namespace", s(root.row.namespace.clone())),
            ("retired", Value::Bool(false)),
            ("retired_ms", Value::Null),
            ("candidates", Value::Arr(candidates)),
        ]);
        output.route(
            &format!("/v1/assets/{}", root.row.asset_id),
            detail.to_json().as_bytes(),
            "application/json",
            true,
        )?;
        let search = &root.row.search;
        search_values.push(obj(vec![
            ("asset_id", s(root.row.asset_id.to_string())),
            ("namespace", s(root.row.namespace.clone())),
            ("kind", match search.kind { Some(kind) => s(kind_str(kind)), None => Value::Null }),
            ("title", s(search.title.clone())),
            ("description", s(search.description.clone())),
            ("categories", values(&search.categories)),
            ("tags", values(&search.tags)),
            ("creator", s(search.creator.clone())),
            ("generator", s(search.generator.clone())),
            ("backend", s(search.backend.clone())),
            ("model", s(search.model.clone())),
            ("live", Value::Bool(true)),
            ("updated_ms", Value::Int(search.updated_ms as i64)),
            ("aliases", Value::Arr(root.row.aliases.iter().map(|alias| s(alias.alias.to_string())).collect())),
            ("terms", Value::Arr(search.terms.iter().map(|term| obj(vec![
                ("term", s(term.term.clone())),
                ("weight", Value::Int(term.weight as i64)),
            ])).collect())),
        ]));
    }
    alias_values.sort_by(|a, b| a.to_json().cmp(&b.to_json()));

    let mut exported_revisions: BTreeMap<AssetRevisionId, &RewrittenRevision> = BTreeMap::new();
    for old in &reachable {
        let revision = &rewritten[old];
        exported_revisions.entry(revision.id).or_insert(revision);
    }
    let mut revision_values = Vec::new();
    for revision in exported_revisions.values() {
        output.route(
            &format!("/v1/revisions/{}", revision.id),
            &revision.bytes,
            "application/octet-stream",
            false,
        )?;
        revision_values.push(obj(vec![
            ("revision", s(revision.id.to_string())),
            ("document", asset_manifest_value(&revision.manifest)),
        ]));
        if let Some(thumbnail) = &revision.manifest.thumbnail {
            if plans[&thumbnail.blob].present {
                let bytes = core.read_blob(&thumbnail.blob)?;
                output.route(
                    &format!("/v1/thumbnails/revision/{}", revision.id),
                    &bytes,
                    match thumbnail.media {
                        makepad_asset_data::ThumbnailMedia::Png => "image/png",
                        makepad_asset_data::ThumbnailMedia::Jpeg => "image/jpeg",
                    },
                    false,
                )?;
            }
        }
    }
    revision_values.sort_by(|a, b| a.to_json().cmp(&b.to_json()));
    let mut variant_values = Vec::new();
    for set in &variants {
        output.route(
            &format!("/v1/variant-sets/{}", set.id),
            &set.bytes,
            "application/octet-stream",
            false,
        )?;
        for variant in &set.variants {
            output.route(
                &format!("/v1/derived-variants/{}", variant.id),
                &variant.bytes,
                "application/octet-stream",
                false,
            )?;
        }
        variant_values.push(obj(vec![
            ("base_revision", s(rewritten[&set.base_old].id.to_string())),
            ("variant_set", s(set.id.to_string())),
            ("variants", Value::Arr(set.variants.iter().map(|variant| s(variant.id.to_string())).collect())),
        ]));
    }
    variant_values.sort_by(|a, b| a.to_json().cmp(&b.to_json()));
    variant_values.dedup();

    let mut blob_values = Vec::new();
    for (blob, plan) in &plans {
        if plan.present {
            let bytes = core.read_blob(blob)?;
            if bytes.len() as u64 != plan.byte_len {
                return Err(ServerError::SizeMismatch {
                    what: "static export blob",
                    expected: plan.byte_len,
                    found: bytes.len() as u64,
                });
            }
            output.route(
                &format!("/v1/blobs/{blob}"),
                &bytes,
                "application/octet-stream",
                false,
            )?;
            report.blobs_present += 1;
        } else {
            report.blobs_omitted += 1;
        }
        blob_values.push(obj(vec![
            ("blob", s(blob.to_string())),
            ("path", s(format!("/v1/blobs/{blob}"))),
            ("byte_len", Value::Int(plan.byte_len as i64)),
            ("sha256", s(blob.to_string().trim_start_matches("sha256:").to_string())),
            ("present", Value::Bool(plan.present)),
            ("reason", match plan.reason { Some(reason) => s(reason), None => Value::Null }),
            ("media", Value::Arr(plan.media.iter().cloned().map(s).collect())),
            ("roles", Value::Arr(plan.roles.iter().cloned().map(s).collect())),
        ]));
    }
    report.unique_blob_bytes = total_bytes;

    let zero_id = "00000000000000000000000000000000";
    let manifest_value = |snapshot_id: &str, server_id: &str, files: Value| {
        obj(vec![
            ("static_version", Value::Int(STATIC_FORMAT_VERSION as i64)),
            ("protocol_version", Value::Int(makepad_asset_client::wire::PROTOCOL_VERSION as i64)),
            ("snapshot_id", s(snapshot_id)),
            ("server_id", s(server_id)),
            ("generated_ms", Value::Int(generated_ms as i64)),
            ("assets", Value::Arr(asset_values.clone())),
            ("aliases", Value::Arr(alias_values.clone())),
            ("revisions", Value::Arr(revision_values.clone())),
            ("search", obj(vec![
                ("normalization", s("ascii-alnum-lower-v1")),
                ("ranking", s("public-weight-sum-v1")),
                ("documents", Value::Arr(search_values.clone())),
            ])),
            ("variants", Value::Arr(variant_values.clone())),
            ("blobs", Value::Arr(blob_values.clone())),
            ("files", files),
            ("policy", obj(vec![
                ("namespace", match &options.namespace { Some(namespace) => s(namespace.clone()), None => Value::Null }),
                ("kind", match options.kind { Some(kind) => s(kind_str(kind)), None => Value::Null }),
                ("limit", match options.limit { Some(limit) => Value::Int(limit as i64), None => Value::Null }),
                ("max_bytes_per_asset", option_u64(options.max_bytes_per_asset)),
                ("max_total_bytes", option_u64(options.max_total_bytes)),
                ("include_video_up_to", Value::Int(options.include_video_up_to as i64)),
            ])),
            ("totals", obj(vec![
                ("assets", Value::Int(kept_roots.len() as i64)),
                ("aliases", Value::Int(alias_values.len() as i64)),
                ("revisions", Value::Int(exported_revisions.len() as i64)),
                ("blobs_present", Value::Int(report.blobs_present as i64)),
                ("blobs_omitted", Value::Int(report.blobs_omitted as i64)),
                ("unique_blob_bytes", Value::Int(total_bytes as i64)),
            ])),
            ("exclusions", obj(vec![
                ("rights", Value::Int(report.excluded_rights as i64)),
                ("budget", Value::Int(report.excluded_budget as i64)),
                ("kind_mismatch", Value::Int(report.excluded_kind_mismatch as i64)),
            ])),
        ])
    };
    // Snapshot identity is derived from the public semantic payload and all
    // already-written routes with identity fields zeroed. Health is written
    // afterwards because it necessarily contains that derived identity.
    let seed = manifest_value(zero_id, zero_id, file_values(&output.files)).to_json();
    let identity = sha256(seed.as_bytes());
    let snapshot_id = hex16(&identity[..16]);
    let server_id = hex16(&identity[16..]);
    let health = obj(vec![
        ("server_id", s(server_id.clone())),
        ("protocol_version", Value::Int(makepad_asset_client::wire::PROTOCOL_VERSION as i64)),
    ])
    .to_json();
    output.route("/v1/health", health.as_bytes(), "application/json", true)?;
    let manifest = manifest_value(&snapshot_id, &server_id, file_values(&output.files)).to_json();
    output.write_untracked(ExportEntry {
        path: "/v1/static/manifest.json".into(),
        bytes: manifest.as_bytes().to_vec(),
        content_type: "application/json",
        content_encoding: None,
    })?;
    let manifest_br = brotli_bytes(manifest.as_bytes())?;
    output.write_untracked(ExportEntry {
        path: "/v1/static/manifest.json.br".into(),
        bytes: manifest_br,
        content_type: "application/json",
        content_encoding: Some("br"),
    })?;

    report.snapshot_id = snapshot_id;
    report.server_id = server_id;
    report.assets = kept_roots.len() as u64;
    report.revisions = exported_revisions.len() as u64;
    report.aliases = alias_values.len() as u64;
    Ok(report)
}
