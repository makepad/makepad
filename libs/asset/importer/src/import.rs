//! Idempotent importer for an existing ai-content library directory.
//!
//! Reads `index.json` (the app's persisted `{items:[{file,label,domain,
//! content_type,prompt,…}], next_id}` format) and the payload files —
//! NOTHING in the library directory is ever written or trusted for
//! previews. Per item:
//!
//! - text payloads are skipped,
//! - PNG publishes as `Texture` with its real pixel dimensions (contract-
//!   mandatory) and itself as the thumbnail,
//! - WAV publishes as `Audio` in the `sfx` category with a FRESHLY rendered
//!   canonical 512×512 waveform thumbnail (the on-disk `.thumb` sidecars
//!   are a known provenance bug — byte-copies of unrelated pipeline images
//!   — and are never read),
//! - GLB publishes as `Character` (skinned+animated) or `Mesh` with
//!   MEASURED vertex/triangle/joint/clip metadata; its thumbnail is the
//!   library's rendered `<file>.thumb` when that is a valid in-bounds PNG,
//!   else the GLB's embedded base-color image, else the honest placeholder,
//! - MP4 publishes as `Video` with measured duration + first-frame thumb,
//! - PLY publishes as `World` in the `splat` category with its Gaussian
//!   splat payload (`Splat`/`Ply`) — the bytes are PARSED before publication
//!   so a malformed scene FAILS rather than landing unloadable; the
//!   thumbnail is the library's rendered `<file>.thumb` when that is a
//!   valid in-bounds PNG/JPEG, else the honest placeholder.
//!
//! SCOPE — generated rows only (pipeline runs, plus dropped files and webcam
//! snaps, which no other flow publishes). The same library also holds rows the app's
//! pack / classic-import flows ALREADY published to the server with their
//! own typed kinds (Billboard, world meshes, …). Re-importing those through
//! this generic path would duplicate them under the wrong kind, so a row is
//! imported only when it is tagged `generated` (or carries a legacy
//! `run-…` group id, which is exactly what the app's tag backfill turns
//! into that tag). Everything else lands in `ImportReport::skipped_scope`.
//!
//! ONLY PRODUCTS BECOME ASSETS. A pipeline run writes every stage artifact:
//! the source still, the cutout matte, the untextured mesh, the PBR maps —
//! and one row that is the thing the user asked for. ONLY that row enters the
//! catalog. Everything else stays in the library where an inspector can read
//! it, and lands in `ImportReport::skipped_intermediate` /
//! `skipped_attached` with its reason. The flag is authored by the Asset UI
//! at route time (`product` in index.json); [`classify_products`] only INFERS
//! it for legacy rows, and that inference assumes a COMPLETE group.
//!
//! A PRODUCT CARRIES ITS OWN MATERIAL. The maps a paint stage wrote beside a
//! GLB, the bakes the Asset UI wrote beside the landed payload, and the
//! ORIGIN PICTURES the run was made from are NOT assets: they are files of
//! the product, published in ONE [`PublishBundle`] under the typed roles the
//! content contract already has — `RenderGlb`/`Texture`/… for the primary,
//! `Albedo`/`Normal`/`Orm`, `AoMesh`/`AoTexture`/`ShadowSdf`, and `Source`
//! for the origin pictures. See [`RunPlan`] for the exact rules. Nothing
//! attached ever publishes standalone, so the catalog cannot grow orphanable
//! map/origin entries a delete could break — and an image run's four variants
//! still stay four assets, because each is content, not material.
//!
//! RUN IDENTITY. Every product carries the label `run-<group>` and an
//! owner-only provenance line naming the run and its stage chain, so "what
//! else came from this run" is one tag query.
//!
//! IDENTITY vs NAME. The artifact digest derives the asset id (first 16
//! digest bytes) — the identity, independent of any text, so renaming a row
//! can never mint a second copy of the same bytes. The catalog ALIAS is a
//! readable name built for a reader (a game's LLM binds assets by alias):
//! `<namespace>/<class>/<slug>-<8 digest hex>`, e.g. `gen/paint/elf-3f9a2b1c`
//! beside a pack's `kenney/car-kit/race`. The digest suffix is what makes
//! names unique without making them unstable — see [`derived_alias`].
//!
//! Idempotency is therefore "this row's NAME already points at THIS row's
//! asset": a rerun resolves the alias, compares the asset id, and skips.
//! A renamed row publishes under its new name onto the same asset id (same
//! immutable revision — the title is annotation, not manifest), so names
//! accumulate and content never duplicates. Legacy provenance is
//! typed-honest: the index records only the prompt, so `manifest_provenance`
//! stays `None` (never fabricated).

use crate::glb::inspect_glb;
use crate::thumbs::{
    encode_jpeg_bgra, jpeg_dims, parse_wav, placeholder_bgra_512, png_dims, waveform_bgra_512,
    THUMB_DIM,
};
use crate::videothumb::probe_video;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::util::{sanitize_text, to_hex};
use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishFile, PublishRequest,
    PublishRights, PublishStats, PublishThumbnail,
};
use makepad_asset_data::{
    limits::{MAX_ALIAS_BYTES, MAX_LOD},
    AssetAlias, AssetId, AssetKind, AssetRevisionId, BlobId, DeviceTier, FileRole, MediaType,
    ThumbnailMedia,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Largest payload the importer will lift (library items are ≤ a few MB).
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ImportReport {
    pub published: Vec<(String, String)>,
    pub skipped_existing: Vec<String>,
    pub skipped_kind: Vec<String>,
    /// Rows outside this importer's scope — not generated content (pack /
    /// classic imports own their own typed publication path).
    pub skipped_scope: Vec<String>,
    /// Rows that publish as FILES OF a product row instead of as assets of
    /// their own — channel maps, bakes, origin pictures: `(row, carrier)`.
    pub skipped_attached: Vec<(String, String)>,
    /// A run's scaffolding: never a catalog asset. `(row, reason)`.
    pub skipped_intermediate: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct IndexItem {
    pub(crate) file: String,
    pub(crate) label: String,
    pub(crate) domain: String,
    pub(crate) content_type: String,
    pub(crate) prompt: String,
    /// Pipeline-run / import group. Rows of one run share it, in pipeline
    /// order; `None` predates grouping.
    pub(crate) group_id: Option<String>,
    /// App-side tags (`generated`, `kenney`, `freedoom`, …) — the scope gate.
    pub(crate) tags: Vec<String>,
    /// Authored by the app at route time; `None` on legacy rows and then
    /// resolved by [`resolve_products`] before any import decision.
    pub(crate) product: Option<bool>,
}

pub(crate) fn parse_index(bytes: &[u8]) -> Result<Vec<IndexItem>, String> {
    let value = json::parse(bytes).map_err(|e| format!("index.json: {e}"))?;
    let items = value
        .get("items")
        .and_then(Value::as_arr)
        .ok_or("index.json: no items array")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let text = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let file = text("file");
        if file.is_empty()
            || file.contains('/')
            || file.contains('\\')
            || file.starts_with('.')
        {
            return Err(format!("index.json: refusing file name {file:?}"));
        }
        let group_id = item
            .get("group_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.is_empty());
        let tags = item
            .get("tags")
            .and_then(Value::as_arr)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.push(IndexItem {
            file,
            label: text("label"),
            domain: text("domain"),
            content_type: text("content_type"),
            prompt: text("prompt"),
            group_id,
            tags,
            product: item.get("product").and_then(Value::as_bool),
        });
    }
    Ok(out)
}

/// One index row, reduced to exactly what product inference needs. Input
/// order MUST be index order, which for a pipeline run is stage order.
pub struct ProductRow<'a> {
    pub domain: &'a str,
    pub content_type: &'a str,
    pub group_id: Option<&'a str>,
    /// The app's authored flag. When present it always wins.
    pub product: Option<bool>,
}

/// The one product/intermediate rule, shared by the importer and the Asset
/// UI's legacy backfill. Returns one flag per input row, same order.
///
/// An explicit `product` wins. Otherwise the rows are grouped by `group_id`
/// (`None` = a group of one) and inferred:
///
/// - the group's LAST row names the product domain — earlier stages of the
///   chain (source image, cutout matte, untextured mesh) are scaffolding;
/// - text/json rows are never products (run sidecars);
/// - if the product domain carries a GLB, its PNGs are texture maps, not
///   products (the `paint` stage emits albedo/normal/ORM beside the GLB);
/// - a single-row group is its own last row, so it is the product.
///
/// This inference is only sound for a COMPLETE group: it reads the last row
/// as the end of the chain. Live runs never rely on it — the Asset UI
/// authors `product` at route time, where the stage index is known.
pub fn classify_products(rows: &[ProductRow<'_>]) -> Vec<bool> {
    use std::collections::HashMap;
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        match row.group_id.filter(|id| !id.is_empty()) {
            Some(id) => {
                let slot = *by_id.entry(id).or_insert_with(|| {
                    groups.push(Vec::new());
                    groups.len() - 1
                });
                groups[slot].push(index);
            }
            None => groups.push(vec![index]),
        }
    }
    let mut out = vec![false; rows.len()];
    for group in &groups {
        let Some(&last) = group.last() else { continue };
        let product_domain = rows[last].domain;
        let domain_has_mesh = group.iter().any(|&index| {
            same_domain(rows[index].domain, product_domain)
                && is_mesh_media(rows[index].content_type)
        });
        for &index in group {
            let row = &rows[index];
            out[index] = match row.product {
                Some(explicit) => explicit,
                None => {
                    same_domain(row.domain, product_domain)
                        && !is_sidecar_media(row.content_type)
                        && !(domain_has_mesh && is_image_media(row.content_type))
                }
            };
        }
    }
    out
}

fn same_domain(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn is_sidecar_media(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("text/") || ct == "application/json"
}

fn is_mesh_media(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.contains("gltf") || ct.contains("glb")
}

fn is_image_media(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().starts_with("image/")
}

/// The image media type of a row, or `None` when it is not an image the
/// contract can carry as a texture file.
fn image_media(content_type: &str) -> Option<MediaType> {
    let ct = content_type.to_ascii_lowercase();
    if ct == "image/png" {
        Some(MediaType::Png)
    } else if ct == "image/jpeg" || ct == "image/jpg" {
        Some(MediaType::Jpeg)
    } else {
        None
    }
}

fn is_mesh_row(item: &IndexItem) -> bool {
    item.file.to_ascii_lowercase().ends_with(".glb") || is_mesh_media(&item.content_type)
}

/// Domains whose stage emits a GLB as its primary output and images only as
/// the channel maps beside it. This is the Asset UI's own routing law
/// (`stage_primary_output` in apps/asset-ui: for `mesh|paint|rig|motion|
/// character` the GLB is the product, "never the channel maps beside it") —
/// read here as a library fact, not re-invented. Everywhere else an image
/// row is content in its own right (source picture, matte, upscale, …).
fn is_map_bearing_domain(domain: &str) -> bool {
    matches!(
        domain.to_ascii_lowercase().as_str(),
        "mesh" | "paint" | "rig" | "motion" | "character"
    )
}

/// Origin pictures a product may carry. Two: the picture the product was
/// made FROM (lod 0) and the one that came before it (lod 1) — a mesh run's
/// cutout and the still it was cut out of. Deeper history is scaffolding, and
/// a candidate fan-out would otherwise flood one manifest with near-misses.
const MAX_SOURCE_LODS: u8 = 2;

/// Does this row publish as a catalog asset at all? Only a run's PRODUCT
/// does. `product` is resolved for the whole snapshot in [`read_index`], so
/// in practice this is always a decided fact; an UNRESOLVED `None` publishes
/// (this importer never invents an intermediate).
fn publishes_as_asset(item: &IndexItem) -> bool {
    item.product != Some(false)
}

/// One library row that is a FILE OF another row rather than an asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachedFile {
    pub(crate) file: String,
    pub(crate) role: FileRole,
    pub(crate) media: MediaType,
    pub(crate) lod: u8,
}

/// What one library snapshot says about its runs: which rows are FILES of
/// which product, and what each run is.
///
/// **Channel maps.** A map row `m` attaches to mesh row `g` when: both are in
/// scope; both carry the SAME non-empty `group_id`; their `domain` is equal
/// and map-bearing, so the stage's own contract says the GLB is the product
/// and images beside it are its channel maps ([`is_map_bearing_domain`]); `m`
/// comes AFTER `g` in index order (index order IS emission order, and `g` is
/// the nearest preceding mesh); and `m` is a PNG/JPEG that is not itself a
/// product. Roles come from emission order, which is the paint backend's
/// documented artifact order (`artifact_roles: textured_glb 0, albedo 1,
/// normal 2, orm 3` in libs/asset/ai/src/paint_backend.rs): the first
/// unclaimed slot of `Albedo → Normal → Orm`, then `Texture` (LOD-indexed)
/// for a map whose meaning is unknown. A file-name hint wins when the row HAS
/// one, but the managed library names payloads `lib-N.png` and labels embed
/// prompt text, so order — not text — is the fact this rule stands on.
///
/// **Origin pictures.** For every row that publishes, walk BACKWARDS from it
/// through its own group and take up to [`MAX_SOURCE_LODS`] image rows that
/// (a) do not publish themselves — a sibling product is content, not an
/// origin — and (b) are not already a map/bake of something, so an albedo can
/// never be mistaken for a source picture. The nearest one is the primary
/// (`Source` lod 0, the picture this product was actually made from), the one
/// before it is lod 1, deeper history is skipped. Products of the same run
/// are walked THROUGH, not stopped at, so all four variants of an image run
/// attach the same input still.
///
/// **Run identity.** The ordered stage domains of each group, for the
/// provenance line every product carries.
#[derive(Debug, Default)]
pub(crate) struct RunPlan {
    /// attached row file → (the row that carries it, the role it lands in).
    owner: HashMap<String, (String, FileRole)>,
    /// product row file → its typed files, in attachment order.
    files: HashMap<String, Vec<AttachedFile>>,
    /// group id → the run's stage domains, in order, deduplicated.
    pipeline: HashMap<String, Vec<String>>,
}

impl RunPlan {
    /// Plan over one WHOLE ordered index snapshot. A single row cannot tell
    /// whether it is a map, an origin picture, or the answer: those are facts
    /// about its run.
    pub(crate) fn plan(items: &[IndexItem]) -> RunPlan {
        let mut out = RunPlan::default();
        // In-scope rows only, index order preserved — the walks below are
        // over the run's own stage order.
        let scoped: Vec<&IndexItem> = items
            .iter()
            .filter(|item| is_generated_row(&item.tags, item.group_id.as_deref()))
            .collect();

        for item in &scoped {
            let Some(group) = group_of(item) else { continue };
            if item.domain.is_empty() {
                continue;
            }
            let stages = out.pipeline.entry(group.to_string()).or_default();
            if stages.last().map(String::as_str) != Some(item.domain.as_str()) {
                stages.push(item.domain.clone());
            }
        }

        // Pass 1 — channel maps, per (group, geometry domain).
        let mut host: HashMap<(&str, String), &str> = HashMap::new();
        for item in &scoped {
            let Some(group) = group_of(item) else {
                // An ungrouped row has no run to belong to: it is its own
                // asset, always.
                continue;
            };
            let domain = item.domain.to_ascii_lowercase();
            if !is_map_bearing_domain(&domain) {
                continue;
            }
            let key = (group, domain);
            if is_mesh_row(item) {
                host.insert(key, item.file.as_str());
                continue;
            }
            let Some(media) = image_media(&item.content_type) else {
                continue;
            };
            // The run's answer is never one of its own mesh's maps.
            if publishes_as_asset(item) {
                continue;
            }
            let Some(&mesh) = host.get(&key) else {
                continue;
            };
            let slots = out.files.entry(mesh.to_string()).or_default();
            let Some((role, lod)) = next_map_slot(&item.file, slots) else {
                // More maps than the contract has slots for: leave the
                // overflow to publish as its own row rather than drop it.
                continue;
            };
            slots.push(AttachedFile { file: item.file.clone(), role, media, lod });
            out.owner.insert(item.file.clone(), (mesh.to_string(), role));
        }

        // Pass 2 — origin pictures, for every row that publishes.
        for (index, item) in scoped.iter().enumerate() {
            if !publishes_as_asset(item) {
                continue;
            }
            let Some(group) = group_of(item) else { continue };
            let mut lod = 0u8;
            for earlier in scoped[..index].iter().rev() {
                if lod >= MAX_SOURCE_LODS {
                    break;
                }
                if group_of(earlier) != Some(group) {
                    continue;
                }
                let Some(media) = image_media(&earlier.content_type) else {
                    continue;
                };
                // A sibling product is content of its own, and a channel map
                // is already spoken for. An origin picture already claimed by
                // an earlier sibling is NOT excluded: one still is the origin
                // of every variant a run made from it.
                let is_map = out
                    .owner
                    .get(&earlier.file)
                    .is_some_and(|(_, role)| *role != FileRole::Source);
                if publishes_as_asset(earlier) || is_map {
                    continue;
                }
                out.files.entry(item.file.clone()).or_default().push(AttachedFile {
                    file: earlier.file.clone(),
                    role: FileRole::Source,
                    media,
                    lod,
                });
                // One picture can be the origin of every variant of a run;
                // the first claimant is the one the skip log names.
                out.owner
                    .entry(earlier.file.clone())
                    .or_insert_with(|| (item.file.clone(), FileRole::Source));
                lod += 1;
            }
        }
        out
    }

    /// The row that carries `file` and the role it lands in, when `file` is
    /// not an asset of its own.
    pub(crate) fn owner_of(&self, file: &str) -> Option<(&str, FileRole)> {
        self.owner.get(file).map(|(owner, role)| (owner.as_str(), *role))
    }

    /// The library rows `product` carries, in attachment order.
    pub(crate) fn files_of(&self, product: &str) -> &[AttachedFile] {
        self.files.get(product).map_or(&[], Vec::as_slice)
    }

    /// `image>matte>mesh>paint` — the run's stages, for the provenance line.
    pub(crate) fn pipeline_of(&self, group: &str) -> Option<&[String]> {
        self.pipeline.get(group).map(Vec::as_slice)
    }
}

fn group_of(item: &IndexItem) -> Option<&str> {
    item.group_id.as_deref().filter(|id| !id.is_empty())
}

/// The next free `(role, lod)` slot for one more map of a mesh.
fn next_map_slot(file: &str, taken: &[AttachedFile]) -> Option<(FileRole, u8)> {
    let used = |role: FileRole, lod: u8| taken.iter().any(|m| m.role == role && m.lod == lod);
    if let Some(hint) = map_role_hint(file) {
        if !used(hint, 0) {
            return Some((hint, 0));
        }
    }
    for role in [FileRole::Albedo, FileRole::Normal, FileRole::Orm] {
        if !used(role, 0) {
            return Some((role, 0));
        }
    }
    (0..=MAX_LOD)
        .find(|&lod| !used(FileRole::Texture, lod))
        .map(|lod| (FileRole::Texture, lod))
}

/// A role a payload FILE NAME states outright. Only the name is consulted:
/// the row label is `<kind> <first 14 chars of the prompt>`, so a prompt like
/// "a normal door" would otherwise mis-assign a map.
fn map_role_hint(file: &str) -> Option<FileRole> {
    let name = file.to_ascii_lowercase();
    let has = |needle: &str| name.contains(needle);
    if has("albedo") || has("basecolor") || has("base_color") || has("base-color") {
        return Some(FileRole::Albedo);
    }
    if has("normal") {
        return Some(FileRole::Normal);
    }
    if has("orm") || has("roughness") || has("metallic") || has("metalness") {
        return Some(FileRole::Orm);
    }
    None
}

/// Scope gate: this importer publishes GENERATED rows only. `generated` is
/// the tag the app writes on pipeline outputs; a `run-…` group id is the
/// same fact on rows written before tags existed (and is exactly what the
/// app's own tag backfill turns into that tag). Pack / classic-import rows
/// are already in the catalog with their proper kinds and must never be
/// re-published through this generic path.
pub fn is_generated_row(tags: &[String], group_id: Option<&str>) -> bool {
    tags.iter().any(|tag| tag.eq_ignore_ascii_case("generated"))
        || group_id.is_some_and(|id| {
            // Pipeline runs, plus what the user hands the app directly
            // (dropped files, webcam snaps): nothing else publishes those.
            // Pack/classic imports (`import:…`) publish themselves.
            id.starts_with("run-") || id.starts_with("drop-") || id.starts_with("webcam-")
        })
}

/// Fill in every row's product flag from the whole (ordered) index.
pub(crate) fn resolve_products(items: &mut [IndexItem]) {
    let flags = {
        let rows: Vec<ProductRow<'_>> = items
            .iter()
            .map(|item| ProductRow {
                domain: &item.domain,
                content_type: &item.content_type,
                group_id: item.group_id.as_deref(),
                product: item.product,
            })
            .collect();
        classify_products(&rows)
    };
    for (item, flag) in items.iter_mut().zip(flags) {
        item.product = Some(flag);
    }
}

/// Hex digits of the artifact digest that end every generated alias. Two
/// assets that happen to share a name still get distinct aliases; the same
/// bytes always land on the same one.
const ALIAS_DIGEST_HEX: usize = 8;
/// Longest human part of the last alias segment. The contract caps a segment
/// at 48 bytes and we append `-<8 hex>`, so 39 + 1 + 8 = 48 exactly.
const MAX_ALIAS_SLUG: usize = 39;
/// Longest middle (class) segment — a domain word, kept short so the whole
/// alias stays comfortably readable.
const MAX_ALIAS_CLASS: usize = 24;

/// Stable identity from the artifact digest: the first 16 digest bytes. This
/// is label-INDEPENDENT on purpose — renaming a row must produce the same
/// asset under a new name, never a second copy of the same bytes.
pub(crate) fn derived_asset_id(bytes: &[u8]) -> AssetId {
    let digest = BlobId::hash_of(bytes);
    AssetId::from_bytes(digest.as_bytes()[..16].try_into().expect("16 bytes"))
}

/// The readable catalog name for one row:
///
/// ```text
/// <namespace> "/" <class> "/" <slug> "-" <short>
/// ```
///
/// * `<namespace>` — the publishing namespace (`gen`). First segment by
///   contract: the catalog refuses an alias that points across namespaces.
/// * `<class>` — the row's `domain` (`paint`, `image`, `music`, `splat`, …),
///   slugified, ≤24 bytes; the media family (`image`/`audio`/`video`/`mesh`/
///   `world`) when the row has no usable domain.
/// * `<slug>` — the row's best human name, slugified and cut on a word
///   boundary: `label`, else the `prompt` (leading article dropped), else the
///   class. Empty only if none of them yields a single ASCII-able character,
///   and then the segment is the digest alone.
/// * `<short>` — 8 lowercase hex of the artifact digest. It makes the name
///   unique WITHOUT making it unstable: same bytes → same alias forever, two
///   different assets called "elf" → two different aliases.
///
/// Examples: `gen/paint/elf-3f9a2b1c`, `gen/image/a-mossy-stump-77c01e42`,
/// `gen/music/neon-drift-0a1b2c3d`.
pub(crate) fn derived_alias(
    item: &IndexItem,
    bytes: &[u8],
    namespace: &str,
) -> Result<AssetAlias, String> {
    let short = to_hex(BlobId::hash_of(bytes).as_bytes())[..ALIAS_DIGEST_HEX].to_string();
    let class = alias_class(item);
    // Keep the WHOLE alias inside the contract's total budget, not just the
    // per-segment one: a long namespace shortens the human part instead of
    // failing the publication.
    let head = namespace.len() + 1 + class.len() + 1;
    let budget = MAX_ALIAS_BYTES
        .saturating_sub(head + 1 + ALIAS_DIGEST_HEX)
        .min(MAX_ALIAS_SLUG);
    let slug = alias_name(item, budget);
    let leaf = if slug.is_empty() { short } else { format!("{slug}-{short}") };
    AssetAlias::from_str(&format!("{namespace}/{class}/{leaf}"))
        .map_err(|_| format!("row cannot form a catalog alias: {namespace}/{class}/{leaf}"))
}

/// Identity + name together, the pair every publication needs.
pub(crate) fn derived_identity(
    item: &IndexItem,
    bytes: &[u8],
    namespace: &str,
) -> Result<(AssetId, AssetAlias), String> {
    Ok((derived_asset_id(bytes), derived_alias(item, bytes, namespace)?))
}

/// The middle alias segment: what KIND of thing this is, in the pipeline's
/// own vocabulary.
fn alias_class(item: &IndexItem) -> String {
    let domain = alias_slug(&item.domain, MAX_ALIAS_CLASS);
    if !domain.is_empty() {
        return domain;
    }
    let ct = item.content_type.to_ascii_lowercase();
    if ct.starts_with("image/") {
        "image"
    } else if ct.starts_with("audio/") {
        "audio"
    } else if ct.starts_with("video/") {
        "video"
    } else if is_mesh_media(&ct) {
        "mesh"
    } else if ct == "application/x-ply" || item.file.to_ascii_lowercase().ends_with(".ply") {
        "world"
    } else {
        "asset"
    }
    .to_string()
}

/// The human part of the alias, from the same source as the catalog title so
/// the two always agree: the row's label, else its prompt, else its class.
fn alias_name(item: &IndexItem, budget: usize) -> String {
    let label = alias_slug(&item.label, budget);
    if !label.is_empty() {
        return label;
    }
    // A prompt is a sentence; an article at the front spends the budget on
    // nothing ("a mossy stump" → `mossy-stump`).
    let prompt = alias_slug(strip_leading_article(&item.prompt), budget);
    if !prompt.is_empty() {
        return prompt;
    }
    alias_slug(&item.domain, budget)
}

fn strip_leading_article(prompt: &str) -> &str {
    let trimmed = prompt.trim_start();
    for article in ["the ", "a ", "an "] {
        // `get` (not slicing) — a prompt may start with a multi-byte char.
        if trimmed
            .get(..article.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(article))
        {
            return &trimmed[article.len()..];
        }
    }
    trimmed
}

/// Deterministic alias-safe slug: lowercase ASCII words joined by single
/// dashes, never leading/trailing punctuation, cut on a WORD boundary at
/// `budget` bytes (so a truncated name still reads). Accented Latin letters
/// fold to their ASCII form; everything else (CJK, emoji, punctuation) is a
/// word separator — the result is always inside the alias charset
/// `[a-z0-9][a-z0-9_-]*`, and may be empty when the input carries no
/// ASCII-able character at all.
pub(crate) fn alias_slug(raw: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let mut folded = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        push_folded(&mut folded, c);
    }
    let mut out = String::new();
    for word in folded.split('-').filter(|word| !word.is_empty()) {
        if out.is_empty() {
            // One over-long first word is cut mid-word rather than dropped:
            // a name is better than no name. Every byte here is ASCII.
            out.push_str(&word[..word.len().min(budget)]);
        } else if out.len() + 1 + word.len() <= budget {
            out.push('-');
            out.push_str(word);
        } else {
            break;
        }
    }
    out
}

/// One lowercase char into a slug: ASCII alphanumerics as themselves, the
/// common accented Latin letters folded to ASCII, everything else a `-`.
fn push_folded(out: &mut String, c: char) {
    let folded = match c {
        'a'..='z' | '0'..='9' => {
            out.push(c);
            return;
        }
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'æ' => "ae",
        'ç' => "c",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ð' => "d",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ý' | 'ÿ' => "y",
        'þ' => "th",
        'ß' => "ss",
        _ => "-",
    };
    out.push_str(folded);
}

/// Longest catalog label the server accepts, so the run tag is cut to fit
/// rather than refused at annotation time.
const MAX_LABEL_BYTES: usize = 48;
/// Bound on the provenance line the importer writes.
const MAX_PROVENANCE_TEXT: usize = 400;

/// The run a row came from, as ONE searchable catalog label:
/// `run-<sanitised group id>`. Every product of a pipeline run carries it, so
/// "what else came from this run" is `tag=run-7` — no join, no scan.
///
/// The catalog label charset is `[a-z0-9][a-z0-9_-]*` (48 bytes), which has
/// no `:`, so the vocabulary separator is `-`. Group ids already read
/// `run-7` / `drop-…` / `webcam-…`; a `run-` prefix that is already there is
/// not doubled, and any other producer's id gets one so the whole vocabulary
/// is greppable.
fn run_tag(item: &IndexItem) -> Option<String> {
    let group = group_of(item)?;
    let slug = alias_slug(group, MAX_LABEL_BYTES);
    if slug.is_empty() {
        return None;
    }
    if slug.starts_with("run-") {
        return Some(slug);
    }
    let room = MAX_LABEL_BYTES - "run-".len();
    Some(format!("run-{}", alias_slug(group, room)))
}

/// The run's shape, as the annotation's provenance line:
/// `run=<group> pipeline=<domain>><domain>… prompt=<prompt>`.
///
/// Provenance is indexed OWNER-ONLY server-side (exactly like the prompt
/// field), so this adds a private, greppable record of how a product came to
/// be without leaking anything into public search.
fn run_provenance(item: &IndexItem, plan: &RunPlan) -> String {
    let mut out = String::new();
    if let Some(group) = group_of(item) {
        out.push_str("run=");
        out.push_str(&sanitize_text(group, 64));
        if let Some(stages) = plan.pipeline_of(group) {
            if !stages.is_empty() {
                out.push_str(" pipeline=");
                for (index, stage) in stages.iter().enumerate() {
                    if index > 0 {
                        out.push('>');
                    }
                    out.push_str(&sanitize_text(stage, 32));
                }
            }
        }
    }
    if !item.prompt.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str("prompt=");
        out.push_str(&sanitize_text(&item.prompt, 240));
    }
    sanitize_text(&out, MAX_PROVENANCE_TEXT)
}

/// A candidate 512-class thumbnail image (PNG/JPEG bytes) if its declared
/// dimensions sit inside the content contract's 256..=4096 window.
pub fn usable_image_thumb(bytes: &[u8]) -> Option<(Vec<u8>, ThumbnailMedia, u32, u32)> {
    if let Some((w, h)) = png_dims(bytes) {
        if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
            return Some((bytes.to_vec(), ThumbnailMedia::Png, w, h));
        }
    }
    if let Some((w, h)) = jpeg_dims(bytes) {
        if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
            return Some((bytes.to_vec(), ThumbnailMedia::Jpeg, w, h));
        }
    }
    None
}

pub fn placeholder_thumb() -> Result<PublishThumbnail, String> {
    Ok(PublishThumbnail {
        bytes: encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM)?,
        media: ThumbnailMedia::Jpeg,
        width: THUMB_DIM as u32,
        height: THUMB_DIM as u32,
    })
}

/// Import one library directory. Never writes into it.
pub fn import_library(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    log: bool,
) -> Result<ImportReport, String> {
    let items = read_index(dir)?;
    let plan = RunPlan::plan(&items);
    Ok(import_items(client, dir, namespace, rights, &items, &plan, log))
}

/// Read one atomically committed library index snapshot. Watch mode treats
/// errors as transient (a foreign writer might not use the library's normal
/// rename protocol) and retries on its next bounded poll.
pub(crate) fn read_index(dir: &Path) -> Result<Vec<IndexItem>, String> {
    let index_bytes =
        std::fs::read(dir.join("index.json")).map_err(|e| format!("index.json: {e}"))?;
    let mut items = parse_index(&index_bytes)?;
    // Resolve the product flag HERE, over the whole ordered snapshot: a
    // single row cannot tell where its run ends. The watcher's row
    // fingerprint therefore also covers the flag, so a legacy row that a
    // later stage reclassifies is re-published with the right tags.
    resolve_products(&mut items);
    Ok(items)
}

/// Import only the selected index rows. This is the continuous watcher's
/// new/changed-only seam; the one-shot importer simply passes every row.
///
/// `plan` is computed over the WHOLE index snapshot even when `items` is a
/// single row: whether a row is a texture of a mesh is a fact about its run,
/// not about the row (see [`RunPlan`]).
pub(crate) fn import_items(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    items: &[IndexItem],
    plan: &RunPlan,
    log: bool,
) -> ImportReport {
    let mut report = ImportReport::default();
    for item in items {
        let outcome = import_item(client, dir, namespace, rights, item, plan);
        match outcome {
            ItemOutcome::Published(asset) => {
                if log {
                    eprintln!("[asset-worker] imported {} -> {asset}", item.file);
                }
                report.published.push((item.file.clone(), asset));
            }
            ItemOutcome::AlreadyPublished => {
                if log {
                    eprintln!("[asset-worker] skip (already published) {}", item.file);
                }
                report.skipped_existing.push(item.file.clone());
            }
            ItemOutcome::SkippedKind => {
                if log {
                    eprintln!("[asset-worker] skip (kind) {}", item.file);
                }
                report.skipped_kind.push(item.file.clone());
            }
            // Out-of-scope rows are the BULK of a library that also holds
            // pack imports; they are a standing structural fact, not an
            // event, so only the report counts them.
            ItemOutcome::SkippedScope => report.skipped_scope.push(item.file.clone()),
            ItemOutcome::SkippedAttached { owner, role } => {
                if log {
                    eprintln!(
                        "[asset-worker] skip ({} of {owner}, published inside it) {}",
                        role_name(role),
                        item.file
                    );
                }
                report.skipped_attached.push((item.file.clone(), owner));
            }
            ItemOutcome::SkippedIntermediate(reason) => {
                if log {
                    eprintln!("[asset-worker] skip ({reason}) {}", item.file);
                }
                report.skipped_intermediate.push((item.file.clone(), reason));
            }
            ItemOutcome::Failed(error) => {
                if log {
                    eprintln!("[asset-worker] FAILED {}: {error}", item.file);
                }
                report.failed.push((item.file.clone(), error));
            }
        }
    }
    report
}

enum ItemOutcome {
    Published(String),
    AlreadyPublished,
    SkippedKind,
    SkippedScope,
    /// A file of the named product row: it lands inside that row's bundle,
    /// never as an asset of its own.
    SkippedAttached { owner: String, role: FileRole },
    /// A run stage that is not the run's product: it does not enter the
    /// catalog at all (an inspector reads the library for those).
    SkippedIntermediate(String),
    Failed(String),
}

/// Role names for skip logs — short, stable, and the same words the file
/// roles carry in the content contract.
fn role_name(role: FileRole) -> &'static str {
    match role {
        FileRole::Albedo => "albedo map",
        FileRole::Normal => "normal map",
        FileRole::Orm => "orm map",
        FileRole::Texture => "texture map",
        FileRole::Source => "origin picture",
        FileRole::AoMesh => "ao mesh",
        FileRole::AoTexture => "ao atlas",
        FileRole::ShadowSdf => "shadow sdf",
        _ => "file",
    }
}

/// A publication in either shape: one plain media artifact, or one mesh with
/// every typed file that belongs to it.
enum Publication {
    Single(Box<PublishRequest>),
    Bundle(Box<PublishBundle>),
}

impl Publication {
    /// Everything the importer authors identically for both shapes.
    fn finish(
        &mut self,
        namespace: &str,
        asset_id: AssetId,
        alias: AssetAlias,
        item: &IndexItem,
        rights: &PublishRights,
        plan: &RunPlan,
    ) {
        let (ns, id, al, prompt, creator, terms, tags, provenance) = match self {
            Publication::Single(r) => (
                &mut r.namespace,
                &mut r.asset_id,
                &mut r.alias,
                &mut r.prompt,
                &mut r.creator,
                &mut r.rights,
                &mut r.tags,
                &mut r.provenance,
            ),
            Publication::Bundle(b) => (
                &mut b.namespace,
                &mut b.asset_id,
                &mut b.alias,
                &mut b.prompt,
                &mut b.creator,
                &mut b.rights,
                &mut b.tags,
                &mut b.provenance,
            ),
        };
        *ns = namespace.to_string();
        *id = Some(asset_id);
        *al = Some(alias);
        *prompt = item.prompt.clone();
        *creator = "ai-content-library".to_string();
        // The operator's explicit declaration for this library — the index
        // format records no rights, and this importer NEVER invents any.
        *terms = rights.clone();
        if !item.domain.is_empty() {
            tags.push(item.domain.clone());
        }
        // Run identity, so "what else came from this run" is one tag query
        // and the run's shape is readable on every product it made.
        if let Some(tag) = run_tag(item) {
            tags.push(tag);
        }
        *provenance = run_provenance(item, plan);
    }

    fn publish(&self, client: &mut AssetClient) -> Result<String, ClientError> {
        match self {
            Publication::Single(r) => {
                client.publish_artifact(r).map(|p| p.asset_id.to_string())
            }
            Publication::Bundle(b) => client.publish_bundle(b).map(|p| p.asset_id.to_string()),
        }
    }
}

fn import_item(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    item: &IndexItem,
    plan: &RunPlan,
) -> ItemOutcome {
    if !is_generated_row(&item.tags, item.group_id.as_deref()) {
        return ItemOutcome::SkippedScope;
    }
    // A channel map, a bake or an origin picture is not an asset — it is a
    // FILE of the row it belongs to. The decision is made from the index
    // alone (never from what has already landed), so it holds whatever order
    // the watcher happens to observe the rows in, and those bytes reach the
    // catalog exactly once: inside their product's bundle.
    if let Some((owner, role)) = plan.owner_of(&item.file) {
        return ItemOutcome::SkippedAttached { owner: owner.to_string(), role };
    }
    let content_type = item.content_type.to_ascii_lowercase();
    let is_glb = item.file.ends_with(".glb") || content_type.contains("gltf");
    let is_ply = item.file.ends_with(".ply") || content_type == "application/x-ply";
    if content_type.starts_with("text/") || content_type == "application/json" {
        return ItemOutcome::SkippedKind;
    }
    // A run's scaffolding stays in the library and OUT of the catalog: the
    // source still, the cutout matte, the untextured mesh are how the product
    // was made, not things to browse, bind or delete by accident. (An
    // inspector reads the library directly for those.)
    if !publishes_as_asset(item) {
        let reason = match group_of(item) {
            Some(group) => format!("intermediate: stage output of {group}, not its product"),
            None => "intermediate: not a run product".to_string(),
        };
        return ItemOutcome::SkippedIntermediate(reason);
    }
    let path = dir.join(&item.file);
    let (bytes, before) = match std::fs::metadata(&path).and_then(|meta| {
        if meta.len() > MAX_IMPORT_BYTES {
            return Err(std::io::Error::other("payload over import budget"));
        }
        let bytes = std::fs::read(&path)?;
        if bytes.len() as u64 != meta.len() {
            return Err(std::io::Error::other("payload changed while reading"));
        }
        Ok((bytes, meta))
    }) {
        Ok(snapshot) => snapshot,
        Err(error) => return ItemOutcome::Failed(error.to_string()),
    };
    if bytes.is_empty() {
        return ItemOutcome::Failed("empty payload".to_string());
    }

    // Identity is the digest-derived asset id; the alias is only its NAME.
    let (asset_id, alias) = match derived_identity(item, &bytes, namespace) {
        Ok(identity) => identity,
        Err(error) => return ItemOutcome::Failed(error),
    };
    // Typed files this row OWNS: its run's channel maps and origin pictures
    // (index facts) and the bake sidecars the Asset UI writes beside a landed
    // GLB (a disk fact). All are located without reading a byte, so the
    // common already-published row still costs exactly one alias probe.
    let attached = attachment_slots(dir, item, plan, is_glb);
    // Idempotency marker: this row's name ALREADY POINTING AT THIS ROW's
    // asset. Asking the name alone would be wrong twice over — a name that
    // resolves to some other asset is not our publication (fall through and
    // claim it), and a row that was renamed publishes under its new name
    // onto the SAME asset id, so nothing is ever duplicated. A mesh with
    // attached files re-checks the published manifest below before it skips,
    // which is what lets maps/bakes that landed later still reach it.
    let published_head = match client.resolve_alias(&alias) {
        Ok(resolved) if resolved.asset_id == asset_id => {
            if attached.is_empty() {
                return ItemOutcome::AlreadyPublished;
            }
            Some(resolved.head_revision)
        }
        Ok(_) => None,
        Err(ClientError::NotFound { .. }) => None,
        Err(error) => return ItemOutcome::Failed(format!("alias probe: {error}")),
    };

    // ONE primary artifact per row, built exactly as before…
    let built = if content_type == "image/png" {
        build_png(item, bytes)
    } else if content_type.starts_with("audio/") {
        build_wav(item, bytes)
    } else if is_glb {
        build_glb(item, dir, bytes)
    } else if content_type.starts_with("video/") {
        build_video(item, &path, bytes)
    } else if is_ply {
        build_splat(item, dir, bytes)
    } else {
        return ItemOutcome::SkippedKind;
    };
    // …then the multi-file shape when the row owns typed files, so a mesh,
    // a picture, a clip and a splat all carry their run's material the same
    // way.
    let mut publication = match built
        .and_then(|request| into_publication(request, &attached, rights))
    {
        Ok(publication) => publication,
        Err(error) => return ItemOutcome::Failed(error),
    };
    // Already published AND already carrying every attached file: nothing to
    // do. A head that predates a map (or a bake sidecar) falls through and
    // re-publishes the SAME asset id with the complete file set.
    if let (Some(head), Publication::Bundle(bundle)) = (&published_head, &publication) {
        if bundle_already_published(client, head, bundle) {
            return ItemOutcome::AlreadyPublished;
        }
    }
    publication.finish(namespace, asset_id, alias, item, rights, plan);
    // A writer that does not use the AI library's normal payload-then-index
    // commit order may still race this read/probe. Never publish a torn
    // snapshot: the watcher will observe the changed metadata and retry.
    match std::fs::metadata(&path) {
        Ok(after)
            if after.len() == before.len()
                && after.modified().ok() == before.modified().ok() => {}
        Ok(_) => return ItemOutcome::Failed("payload changed while importing".to_string()),
        Err(error) => return ItemOutcome::Failed(format!("payload recheck: {error}")),
    }
    // Legacy provenance is prompt-only: typed provenance stays honest-None.
    match publication.publish(client) {
        Ok(asset) => ItemOutcome::Published(asset),
        Err(error) => ItemOutcome::Failed(format!("publish: {error}")),
    }
}

/// Does the published head already carry every file of this bundle, byte for
/// byte? Blob identities are compared (not just slots), so a re-baked map is
/// a real difference and lands as a new revision of the same asset.
fn bundle_already_published(
    client: &mut AssetClient,
    head: &AssetRevisionId,
    bundle: &PublishBundle,
) -> bool {
    let Ok(manifest) = client.fetch_asset_manifest(head) else {
        return false;
    };
    bundle.files.iter().all(|file| {
        let blob = BlobId::hash_of(&file.bytes);
        manifest.files.iter().any(|published| {
            published.role == file.role
                && published.tier == file.tier
                && published.lod == file.lod
                && published.blob == blob
        })
    })
}

/// One typed file a mesh row carries, located but not yet read.
struct AttachedSlot {
    path: PathBuf,
    role: FileRole,
    media: MediaType,
    lod: u8,
}

/// Everything that publishes INSIDE this row's asset: the run's channel maps
/// and origin pictures (from the index, see [`RunPlan`]) and — for a GLB —
/// the offline bake sidecars the Asset UI writes beside the landed payload:
/// `<stem>.aomesh` + `<stem>.ao.png` (the atlas and the mesh whose `ao_uv`
/// lane samples it, attached only as a PAIR because neither is usable alone)
/// and `<stem>.shadowsdf`.
fn attachment_slots(
    dir: &Path,
    item: &IndexItem,
    plan: &RunPlan,
    is_glb: bool,
) -> Vec<AttachedSlot> {
    let mut out: Vec<AttachedSlot> = plan
        .files_of(&item.file)
        .iter()
        .filter_map(|attached| {
            // A file the index promises but disk does not have yet is simply
            // not attached on THIS pass — never a hard failure that would
            // keep re-failing an otherwise publishable product. The next pass
            // sees it and the manifest comparison republishes.
            let path = dir.join(&attached.file);
            path.is_file().then(|| AttachedSlot {
                path,
                role: attached.role,
                media: attached.media,
                lod: attached.lod,
            })
        })
        .collect();
    if !is_glb {
        return out;
    }
    let payload = dir.join(&item.file);
    let sidecar = |ext: &str| {
        let path = payload.with_extension(ext);
        path.is_file().then_some(path)
    };
    if let (Some(mesh), Some(atlas)) = (sidecar("aomesh"), sidecar("ao.png")) {
        out.push(AttachedSlot {
            path: mesh,
            role: FileRole::AoMesh,
            media: MediaType::Bin,
            lod: 0,
        });
        out.push(AttachedSlot {
            path: atlas,
            role: FileRole::AoTexture,
            media: MediaType::Png,
            lod: 0,
        });
    }
    if let Some(sdf) = sidecar("shadowsdf") {
        out.push(AttachedSlot {
            path: sdf,
            role: FileRole::ShadowSdf,
            media: MediaType::Bin,
            lod: 0,
        });
    }
    out
}

/// Read one attached file, refusing a torn snapshot the same way the primary
/// payload read does.
fn read_stable(path: &Path) -> Result<Vec<u8>, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let before = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if before.len() > MAX_IMPORT_BYTES {
        return Err(format!("{name}: over import budget"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let after = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(format!("{name}: changed while reading"));
    }
    if bytes.is_empty() {
        return Err(format!("{name}: empty payload"));
    }
    Ok(bytes)
}

/// The catalog title, from the SAME human name the alias slug is built from
/// (label, else prompt, else the payload file name) so a row reads the same
/// way in a search result and in the alias a game binds it by.
fn title_of(item: &IndexItem) -> String {
    for source in [
        item.label.as_str(),
        strip_leading_article(&item.prompt),
        item.file.as_str(),
    ] {
        let title = sanitize_text(source, 120);
        if !title.is_empty() {
            return title;
        }
    }
    "Imported asset".to_string()
}

fn build_png(item: &IndexItem, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let (width, height) = png_dims(&bytes).ok_or("png: malformed header")?;
    let thumbnail = match usable_image_thumb(&bytes) {
        Some((thumb, media, w, h)) => {
            PublishThumbnail { bytes: thumb, media, width: w, height: h }
        }
        None => placeholder_thumb()?,
    };
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Texture,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((width, height)),
        },
        thumbnail,
    );
    request.categories = vec!["image".to_string()];
    Ok(request)
}

fn build_wav(item: &IndexItem, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let pcm = parse_wav(&bytes)?;
    // ALWAYS a fresh canonical waveform — the on-disk sidecars are stale.
    let strip = waveform_bgra_512(&pcm);
    let thumbnail = PublishThumbnail {
        bytes: encode_jpeg_bgra(&strip, THUMB_DIM, THUMB_DIM)?,
        media: ThumbnailMedia::Jpeg,
        width: THUMB_DIM as u32,
        height: THUMB_DIM as u32,
    };
    let millis = pcm.millis();
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Audio,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Wav,
            role: FileRole::Audio,
            media_millis: millis,
            dims: None,
        },
        thumbnail,
    );
    // VJ has deliberately separate long-form DJ tracks and one-shot pads.
    // The AI library's domain is authoritative for that behavior: Music3
    // writes `music`, while SA3/Woosh/MOSS write `audio` and remain SFX.
    request.categories = vec![audio_category(item).to_string()];
    Ok(request)
}

fn audio_category(item: &IndexItem) -> &'static str {
    if item.domain.eq_ignore_ascii_case("music") {
        "music"
    } else {
        "sfx"
    }
}

/// The mesh row's own preview: the library's rendered `<file>.thumb` when it
/// is a valid in-bounds image, else the GLB's embedded base color, else the
/// honest placeholder. The importer only READS sidecars, never writes them.
fn glb_thumbnail(
    item: &IndexItem,
    dir: &Path,
    stats: &crate::glb::GlbStats,
) -> Result<PublishThumbnail, String> {
    let rendered = std::fs::read(dir.join(format!("{}.thumb", item.file)))
        .ok()
        .and_then(|thumb| usable_image_thumb(&thumb));
    match rendered.or_else(|| stats.base_color.as_deref().and_then(usable_image_thumb)) {
        Some((thumb, media, width, height)) => {
            Ok(PublishThumbnail { bytes: thumb, media, width, height })
        }
        None => placeholder_thumb(),
    }
}

fn glb_kind(stats: &crate::glb::GlbStats) -> (AssetKind, &'static str) {
    if stats.skinned {
        (AssetKind::Character, "dancer")
    } else {
        (AssetKind::Mesh, "prop")
    }
}

fn build_glb(item: &IndexItem, dir: &Path, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let stats = inspect_glb(&bytes)?;
    let thumbnail = glb_thumbnail(item, dir, &stats)?;
    let (kind, category) = glb_kind(&stats);
    let mut request = PublishRequest::new(
        "gen",
        kind,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Glb,
            role: FileRole::RenderGlb,
            media_millis: 0,
            dims: None,
        },
        thumbnail,
    );
    request.stats = PublishStats {
        triangles: stats.triangles,
        vertices: stats.vertices,
        joints: stats.joints,
        clips: stats.clips,
    };
    request.categories = vec![category.to_string()];
    Ok(request)
}

/// ONE asset for a product and everything that belongs to it: the primary
/// artifact exactly as its own builder made it, plus the run's channel maps,
/// bake sidecars and origin pictures as typed files of the same revision.
/// None of those is ever an asset of its own — a catalog entry a delete could
/// orphan is exactly what the file roles exist to prevent.
///
/// With nothing attached the row stays on the single-artifact path, so a
/// plain picture or clip is published byte-for-byte as before.
fn into_publication(
    request: PublishRequest,
    attached: &[AttachedSlot],
    rights: &PublishRights,
) -> Result<Publication, String> {
    if attached.is_empty() {
        return Ok(Publication::Single(Box::new(request)));
    }
    let mut files = vec![PublishBundleFile {
        role: request.artifact.role,
        tier: DeviceTier::Any,
        lod: 0,
        media: request.artifact.media,
        bytes: request.artifact.bytes,
        dims: request.artifact.dims,
    }];
    for slot in attached {
        let bytes = read_stable(&slot.path)?;
        // Image files carry MANDATORY measured dimensions; an unreadable
        // header is a failure, never a fabricated size.
        let dims = match slot.media {
            MediaType::Png => Some(png_dims(&bytes).ok_or_else(|| {
                format!("{}: malformed png header", slot.path.display())
            })?),
            MediaType::Jpeg => Some(jpeg_dims(&bytes).ok_or_else(|| {
                format!("{}: malformed jpeg header", slot.path.display())
            })?),
            _ => None,
        };
        files.push(PublishBundleFile {
            role: slot.role,
            tier: DeviceTier::Any,
            lod: slot.lod,
            media: slot.media,
            bytes,
            dims,
        });
    }
    let mut bundle = PublishBundle::new(
        request.namespace,
        request.kind,
        request.title,
        files,
        request.thumbnail,
        rights.clone(),
    );
    bundle.categories = request.categories;
    bundle.tags = request.tags;
    bundle.stats = request.stats;
    bundle.media_millis = request.artifact.media_millis;
    // Measured, never assumed: the GLB inspection put real joint/clip counts
    // in the stats, and those are what "rigged"/"animated" mean.
    bundle.capabilities.rigged = request.stats.joints > 0;
    bundle.capabilities.animated = request.stats.clips > 0;
    Ok(Publication::Bundle(Box::new(bundle)))
}

/// A Gaussian splat scene. The render payload of a splat `World`: one PLY
/// file in the `Splat` role, never meshed. The bytes are PARSED here (not
/// sniffed) so an unloadable scene is a hard failure at import time rather
/// than a catalog entry no renderer can open.
fn build_splat(item: &IndexItem, dir: &Path, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    // The path hint pins the PLY branch of the loader; the header still has
    // to agree, so a mislabelled `.ply` refuses here.
    let scene = makepad_splat::load_splat_from_bytes(&bytes, Some(Path::new(&item.file)))
        .map_err(|e| format!("ply: {e}"))?;
    if scene.splats.is_empty() {
        return Err("ply: no splats".to_string());
    }
    // The app renders `<file>.thumb` beside the payload; only READ, never
    // regenerate or write sidecars (same rule as the GLB path).
    let thumbnail = match std::fs::read(dir.join(format!("{}.thumb", item.file)))
        .ok()
        .and_then(|thumb| usable_image_thumb(&thumb))
    {
        Some((thumb, media, w, h)) => {
            PublishThumbnail { bytes: thumb, media, width: w, height: h }
        }
        None => placeholder_thumb()?,
    };
    let mut request = PublishRequest::new(
        "gen",
        // No new asset kind: a splat scene IS a world, distinguished by its
        // `splat` category and its file role.
        AssetKind::World,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Ply,
            role: FileRole::Splat,
            media_millis: 0,
            dims: None,
        },
        thumbnail,
    );
    request.categories = vec!["splat".to_string()];
    // The two producers of splat PLYs mean different things: the asset-ui
    // `splat` domain makes single objects, FlashWorld's `world` domain makes
    // whole scenes. The scope tag is what a consumer filters on.
    if let Some(tag) = splat_scope_tag(item) {
        request.tags.push(tag.to_string());
    }
    Ok(request)
}

/// `object` for an object-scale splat, `world` for a scene-scale one; `None`
/// when the row's domain says neither (never invented) — and `None` as well
/// when the scope tag would only repeat the domain tag `import_item` already
/// pushes, so a world splat carries `world` exactly once.
fn splat_scope_tag(item: &IndexItem) -> Option<&'static str> {
    let scope = if item.domain.eq_ignore_ascii_case("splat") {
        "object"
    } else if item.domain.eq_ignore_ascii_case("world") {
        "world"
    } else {
        return None;
    };
    (!item.domain.eq_ignore_ascii_case(scope)).then_some(scope)
}

fn build_video(item: &IndexItem, path: &Path, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let probe = probe_video(path)?;
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Video,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Mp4,
            role: FileRole::Video,
            media_millis: probe.duration_ms,
            dims: None,
        },
        PublishThumbnail {
            bytes: probe.thumbnail_jpeg,
            media: ThumbnailMedia::Jpeg,
            width: THUMB_DIM as u32,
            height: THUMB_DIM as u32,
        },
    );
    request.categories = vec!["generated".to_string()];
    if !probe.real_frame {
        request.tags.push("no-preview-frame".to_string());
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_asset_import_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    fn named(label: &str, domain: &str, content_type: &str) -> IndexItem {
        IndexItem {
            file: "lib-1.png".to_string(),
            label: label.to_string(),
            domain: domain.to_string(),
            content_type: content_type.to_string(),
            ..IndexItem::default()
        }
    }

    #[test]
    fn identity_is_digest_stable_and_the_alias_is_a_readable_name() {
        let elf = named("Elf", "paint", "model/gltf-binary");
        let (asset_a, alias_a) = derived_identity(&elf, b"payload one", "gen").unwrap();
        let (asset_b, alias_b) = derived_identity(&elf, b"payload one", "gen").unwrap();
        let (asset_c, alias_c) = derived_identity(&elf, b"payload two", "gen").unwrap();
        // Same bytes → same identity AND same name, forever.
        assert_eq!(asset_a, asset_b);
        assert_eq!(alias_a, alias_b);
        // Same NAME, different bytes → different asset, different alias.
        assert_ne!(asset_a, asset_c);
        assert_ne!(alias_a.as_str(), alias_c.as_str());

        // `gen/<class>/<slug>-<8 hex>` — three readable segments.
        let segments: Vec<&str> = alias_a.as_str().split('/').collect();
        assert_eq!(segments.len(), 3, "{}", alias_a.as_str());
        assert_eq!(segments[0], "gen");
        assert_eq!(segments[1], "paint");
        let (slug, short) = segments[2].rsplit_once('-').unwrap();
        assert_eq!(slug, "elf");
        assert_eq!(short.len(), 8);
        assert!(short.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        // Renaming the row keeps the IDENTITY and changes only the name.
        let renamed = named("Wood elf", "paint", "model/gltf-binary");
        let (asset_d, alias_d) = derived_identity(&renamed, b"payload one", "gen").unwrap();
        assert_eq!(asset_d, asset_a, "a rename is never a second asset");
        assert_eq!(alias_d.as_str().rsplit('-').next(), alias_a.as_str().rsplit('-').next());
        assert!(alias_d.as_str().starts_with("gen/paint/wood-elf-"), "{alias_d}");

        assert!(derived_identity(&elf, b"payload", "bad namespace").is_err());
    }

    #[test]
    fn alias_slugs_survive_unicode_punctuation_and_long_prompts() {
        // Accented Latin folds, punctuation separates, runs collapse, and
        // nothing leading/trailing survives.
        assert_eq!(alias_slug("Café Racer!", 39), "cafe-racer");
        assert_eq!(alias_slug("  ¡¿Ñandú — Straße?!  ", 39), "nandu-strasse");
        assert_eq!(alias_slug("A/B__C", 39), "a-b-c");
        assert_eq!(alias_slug("---", 39), "");
        assert_eq!(alias_slug("東京", 39), "", "no ASCII-able character at all");
        assert_eq!(alias_slug("2001 a space odyssey", 39), "2001-a-space-odyssey");
        // The cut is on a word boundary, and the budget is never exceeded.
        let long = alias_slug("a low poly wooden crate with iron bands and rope", 39);
        assert_eq!(long, "a-low-poly-wooden-crate-with-iron-bands");
        assert!(long.len() <= 39);
        // …but one over-long word is cut rather than dropped.
        assert_eq!(alias_slug(&"z".repeat(80), 39), "z".repeat(39));
        assert_eq!(alias_slug("anything", 0), "");

        // Every slug is inside the alias charset and can lead a segment.
        for raw in ["Café Racer!", "-leading", "9 lives", "__x__"] {
            let slug = alias_slug(raw, 39);
            if slug.is_empty() {
                continue;
            }
            assert!(slug.as_bytes()[0].is_ascii_alphanumeric(), "{slug}");
            assert!(
                slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{slug}"
            );
        }
    }

    #[test]
    fn alias_names_fall_back_label_then_prompt_then_class() {
        // Label wins.
        let mut item = named("Rusty Crate", "mesh", "model/gltf-binary");
        item.prompt = "a low poly crate".to_string();
        assert_eq!(alias_name(&item, MAX_ALIAS_SLUG), "rusty-crate");
        // No label: the prompt, with its leading article dropped.
        item.label.clear();
        assert_eq!(alias_name(&item, MAX_ALIAS_SLUG), "low-poly-crate");
        // Nothing human at all: the class carries the name.
        item.prompt.clear();
        assert_eq!(alias_name(&item, MAX_ALIAS_SLUG), "mesh");
        // …and with no domain either, the alias is class + digest only.
        item.domain.clear();
        assert_eq!(alias_name(&item, MAX_ALIAS_SLUG), "");
        assert_eq!(alias_class(&item), "mesh", "media family names an empty domain");
        let alias = derived_alias(&item, b"bytes", "gen").unwrap();
        let leaf = alias.as_str().rsplit('/').next().unwrap();
        assert_eq!(leaf.len(), 8, "digest-only leaf: {alias}");

        // The title reads the same way the alias does.
        let mut titled = named("", "image", "image/png");
        titled.prompt = "A mossy stump".to_string();
        assert_eq!(title_of(&titled), "mossy stump");
        assert_eq!(alias_name(&titled, MAX_ALIAS_SLUG), "mossy-stump");
    }

    #[test]
    fn aliases_stay_inside_the_contract_for_hostile_inputs() {
        // A long namespace shrinks the human part instead of failing.
        let mut item = named("a".repeat(300).as_str(), "b".repeat(300).as_str(), "image/png");
        item.prompt = "c".repeat(300);
        for namespace in ["gen", &"n".repeat(48)] {
            let alias = derived_alias(&item, b"bytes", namespace).unwrap();
            assert!(alias.as_str().len() <= MAX_ALIAS_BYTES, "{alias}");
            let segments: Vec<&str> = alias.as_str().split('/').collect();
            assert_eq!(segments.len(), 3);
            assert_eq!(segments[0], namespace);
            for segment in &segments[1..] {
                assert!(segment.len() <= 48, "{segment}");
            }
            // Re-parsing is the contract check itself.
            assert!(AssetAlias::from_str(alias.as_str()).is_ok());
        }
        // Emoji-only label + emoji-only prompt still names something.
        let mut emoji = named("🔥🔥", "image", "image/png");
        emoji.prompt = "🌊".to_string();
        assert_eq!(
            derived_alias(&emoji, b"bytes", "gen").unwrap().as_str().split('/').nth(1),
            Some("image")
        );
        // A namespace no alias segment can hold refuses with a clear reason
        // instead of publishing under a broken name.
        let error = derived_alias(&emoji, b"bytes", &"n".repeat(64)).unwrap_err();
        assert!(error.contains("cannot form a catalog alias"), "{error}");
    }

    #[test]
    fn index_parses_and_refuses_hostile_file_names() {
        let good = br#"{"items":[{"file":"lib-1.png","label":"a","domain":"image",
            "content_type":"image/png","prompt":"p"}],"next_id":2}"#;
        let items = parse_index(good).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file, "lib-1.png");
        for hostile in [
            br#"{"items":[{"file":"../../etc/passwd","content_type":"image/png"}]}"#.as_slice(),
            br#"{"items":[{"file":".hidden","content_type":"image/png"}]}"#.as_slice(),
        ] {
            assert!(parse_index(hostile).is_err());
        }
        assert!(parse_index(b"junk").is_err());
    }

    #[test]
    fn imported_titles_are_utf8_safe_and_byte_bounded() {
        let item = IndexItem {
            file: "fallback.png".to_string(),
            label: "é".repeat(100),
            domain: "image".to_string(),
            content_type: "image/png".to_string(),
            ..IndexItem::default()
        };
        let title = title_of(&item);
        assert!(title.len() <= 120);
        assert!(title.is_char_boundary(title.len()));
        assert_eq!(title, "é".repeat(60));
    }

    #[test]
    fn audio_domain_routes_music_to_decks_and_other_audio_to_sfx() {
        let mut item = IndexItem {
            file: "track.wav".to_string(),
            label: "track".to_string(),
            domain: "music".to_string(),
            content_type: "audio/wav".to_string(),
            ..IndexItem::default()
        };
        assert_eq!(audio_category(&item), "music");
        item.domain = "audio".to_string();
        assert_eq!(audio_category(&item), "sfx");
        item.domain = "speech".to_string();
        assert_eq!(audio_category(&item), "sfx");
    }

    fn row<'a>(
        domain: &'a str,
        content_type: &'a str,
        group_id: Option<&'a str>,
    ) -> ProductRow<'a> {
        ProductRow { domain, content_type, group_id, product: None }
    }

    #[test]
    fn product_inference_keeps_only_the_last_stage_primary_output() {
        // `image → mesh → PBR`, exactly as the app writes it.
        let rows = [
            row("image", "image/png", Some("run-1")),
            row("mesh", "model/gltf-binary", Some("run-1")),
            row("paint", "model/gltf-binary", Some("run-1")),
            row("paint", "image/png", Some("run-1")),
            row("paint", "image/png", Some("run-1")),
            row("paint", "image/png", Some("run-1")),
            row("paint", "application/json", Some("run-1")),
        ];
        assert_eq!(
            classify_products(&rows),
            vec![false, false, true, false, false, false, false],
            "only the painted GLB: the untextured mesh is an earlier stage, \
             the maps and the sidecar are not primary outputs"
        );

        // `image → cutout → mesh → hunyuan PBR`: the matte is a stage too.
        let rows = [
            row("image", "image/png", Some("run-2")),
            row("matte", "image/png", Some("run-2")),
            row("mesh", "model/gltf-binary", Some("run-2")),
            row("paint", "model/gltf-binary", Some("run-2")),
            row("paint", "image/png", Some("run-2")),
        ];
        assert_eq!(classify_products(&rows), vec![false, false, false, true, false]);

        // Chains whose last stage has no mesh: the last domain's payload IS
        // the product (image, cutout, video, music).
        let rows = [
            row("image", "image/png", Some("run-3")),
            row("matte", "image/png", Some("run-3")),
        ];
        assert_eq!(classify_products(&rows), vec![false, true]);
        let rows = [
            row("text", "text/plain", Some("run-4")),
            row("image", "image/png", Some("run-4")),
            row("video", "video/mp4", Some("run-4")),
        ];
        assert_eq!(classify_products(&rows), vec![false, false, true]);
        assert_eq!(
            classify_products(&[row("music", "audio/wav", Some("run-5"))]),
            vec![true],
            "a single-row group is its own product"
        );
    }

    #[test]
    fn product_inference_is_per_group_and_the_authored_flag_wins() {
        // Interleaved groups (concurrent runs) must not borrow each other's
        // last row, and ungrouped rows are groups of one.
        let rows = [
            ProductRow { domain: "image", content_type: "image/png", group_id: Some("a"), product: None },
            ProductRow { domain: "image", content_type: "image/png", group_id: Some("b"), product: None },
            ProductRow { domain: "mesh", content_type: "model/gltf-binary", group_id: Some("a"), product: None },
            ProductRow { domain: "image", content_type: "image/png", group_id: None, product: None },
            // Authored false on a row inference would call the product.
            ProductRow { domain: "video", content_type: "video/mp4", group_id: Some("b"), product: Some(false) },
        ];
        assert_eq!(classify_products(&rows), vec![false, false, true, true, false]);
    }

    #[test]
    fn index_reads_group_tags_and_product_and_scopes_to_generated() {
        let items = parse_index(
            br#"{"items":[
                {"file":"lib-1.png","domain":"image","content_type":"image/png",
                 "group_id":"run-9","tags":["generated"],"product":false},
                {"file":"lib-2.png","domain":"image","content_type":"image/png",
                 "group_id":"import:kenney:blocks","tags":["kenney"]},
                {"file":"lib-3.png","domain":"image","content_type":"image/png",
                 "group_id":"run-legacy"}
            ],"next_id":4}"#,
        )
        .unwrap();
        assert_eq!(items[0].tags, vec!["generated".to_string()]);
        assert_eq!(items[0].product, Some(false));
        assert_eq!(items[1].group_id.as_deref(), Some("import:kenney:blocks"));
        assert_eq!(items[1].product, None);
        assert!(items[2].tags.is_empty());

        assert!(is_generated_row(&items[0].tags, items[0].group_id.as_deref()));
        assert!(!is_generated_row(&items[1].tags, items[1].group_id.as_deref()));
        // Legacy generated rows carry no tag but a `run-…` group id — the
        // same fact the app's own tag backfill reads.
        assert!(is_generated_row(&items[2].tags, items[2].group_id.as_deref()));
    }

    #[test]
    fn only_products_reach_the_catalog_and_pack_rows_stay_out_of_scope() {
        use makepad_asset_store::{AssetServer, ServerConfig};
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};

        let root = test_root("scope-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("scope-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("scope-library");
        std::fs::create_dir_all(&library).unwrap();
        let png = |seed: u8| {
            let mut bytes = vec![seed; 25];
            bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
            bytes[12..16].copy_from_slice(b"IHDR");
            bytes[16..20].copy_from_slice(&512u32.to_be_bytes());
            bytes[20..24].copy_from_slice(&512u32.to_be_bytes());
            bytes
        };
        for (file, seed) in [("lib-1.png", 1u8), ("lib-2.png", 2), ("lib-3.png", 3)] {
            std::fs::write(library.join(file), png(seed)).unwrap();
        }
        // One `image → cutout` run plus one already-published pack row.
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[
                {"file":"lib-1.png","label":"source","domain":"image","content_type":"image/png",
                 "prompt":"p","group_id":"run-7","tags":["generated"]},
                {"file":"lib-2.png","label":"cutout","domain":"matte","content_type":"image/png",
                 "prompt":"p","group_id":"run-7","tags":["generated"]},
                {"file":"lib-3.png","label":"crate","domain":"image","content_type":"image/png",
                 "prompt":"kenney blocks","group_id":"import:kenney:blocks","tags":["kenney"]}
            ],"next_id":4}"#,
        )
        .unwrap();

        let report = import_library(
            &mut client,
            &library,
            "gen",
            &PublishRights::declared(
                "CC0-1.0",
                "",
                "",
                makepad_asset_data::Redistribution::Allowed,
                makepad_asset_data::DerivativePolicy::Allowed,
            ),
            false,
        )
        .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        // ONE asset: the cutout the run was FOR. The source picture is not a
        // catalog entry at all — it rides along as the cutout's origin.
        assert_eq!(
            report.published.iter().map(|(file, _)| file.as_str()).collect::<Vec<_>>(),
            vec!["lib-2.png"]
        );
        assert_eq!(
            report.skipped_attached,
            vec![("lib-1.png".to_string(), "lib-2.png".to_string())]
        );
        assert!(report.skipped_intermediate.is_empty());
        assert_eq!(report.skipped_scope, vec!["lib-3.png".to_string()]);

        let all = client
            .catalog_search(&CatalogQuery::browse(32), None)
            .expect("browse");
        assert_eq!(all.hits.len(), 1, "no scaffolding in the catalog");
        assert_eq!(all.hits[0].title, "cutout");
        // Run identity is one label the whole run shares.
        let run = client
            .catalog_search(
                &CatalogQuery { tag: Some("run-7".into()), ..CatalogQuery::browse(32) },
                None,
            )
            .expect("run tag search");
        assert_eq!(run.hits.len(), 1);
        assert_eq!(run.hits[0].title, "cutout");
        // The origin picture is a FILE of it, with measured dims.
        let rows = read_index(&library).unwrap();
        let cutout = rows.iter().find(|item| item.file == "lib-2.png").unwrap();
        let alias =
            derived_alias(cutout, &std::fs::read(library.join("lib-2.png")).unwrap(), "gen")
                .unwrap();
        let head = client.resolve_alias(&alias).unwrap().head_revision;
        let manifest = client.fetch_asset_manifest(&head).unwrap();
        assert_eq!(
            manifest.files.iter().map(|f| (f.role, f.lod)).collect::<Vec<_>>(),
            vec![(FileRole::Texture, 0), (FileRole::Source, 0)]
        );
        let _ = std::fs::remove_dir_all(library);
    }

    /// The smallest valid ascii Gaussian-splat PLY: `count` vertices with
    /// position, DC color, opacity, log-scale and quaternion — the same
    /// property vocabulary `makepad_splat`'s own tests use.
    fn ascii_splat_ply(count: usize) -> Vec<u8> {
        let mut out = format!(
            "ply\nformat ascii 1.0\ncomment makepad importer test\n\
             element vertex {count}\n\
             property float x\nproperty float y\nproperty float z\n\
             property float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\n\
             property float opacity\n\
             property float scale_0\nproperty float scale_1\nproperty float scale_2\n\
             property float rot_0\nproperty float rot_1\nproperty float rot_2\n\
             property float rot_3\nend_header\n"
        )
        .into_bytes();
        for i in 0..count {
            let f = i as f32;
            out.extend_from_slice(
                format!(
                    "{f} {f} {f} 0.5 0.4 0.3 2.0 -3.0 -3.0 -3.0 0 0 0 1\n"
                )
                .as_bytes(),
            );
        }
        out
    }

    /// A 512×512 PNG header — enough for `png_dims`, which is all the
    /// importer reads out of a thumbnail sidecar.
    fn png_512(seed: u8) -> Vec<u8> {
        let mut bytes = vec![seed; 25];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&512u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&512u32.to_be_bytes());
        bytes
    }

    #[test]
    fn splat_ply_builds_as_a_world_with_the_splat_role_and_a_scope_tag() {
        let bytes = ascii_splat_ply(4);
        let dir = test_root("splat-build");
        std::fs::create_dir_all(&dir).unwrap();

        // Object-scale splat (asset-ui `splat` domain) WITH a rendered
        // sidecar: the sidecar is used verbatim.
        std::fs::write(dir.join("lib-1.ply.thumb"), png_512(9)).unwrap();
        let object = IndexItem {
            file: "lib-1.ply".to_string(),
            label: "mossy stump".to_string(),
            domain: "splat".to_string(),
            content_type: "application/x-ply".to_string(),
            ..IndexItem::default()
        };
        let request = build_splat(&object, &dir, bytes.clone()).unwrap();
        assert_eq!(request.kind, AssetKind::World);
        assert_eq!(request.artifact.role, FileRole::Splat);
        assert_eq!(request.artifact.media, MediaType::Ply);
        assert_eq!(request.artifact.dims, None, "a PLY is not an image");
        assert_eq!(request.categories, vec!["splat".to_string()]);
        assert_eq!(request.tags, vec!["object".to_string()]);
        assert_eq!(request.thumbnail.media, ThumbnailMedia::Png);
        assert_eq!((request.thumbnail.width, request.thumbnail.height), (512, 512));

        // Scene-scale splat (FlashWorld `world` domain), no sidecar: the
        // honest placeholder, and no scope tag because `import_item`
        // already pushes the `world` domain tag.
        let world = IndexItem {
            file: "lib-2.ply".to_string(),
            label: "coastal world".to_string(),
            domain: "world".to_string(),
            content_type: "application/x-ply".to_string(),
            ..IndexItem::default()
        };
        let request = build_splat(&world, &dir, bytes).unwrap();
        assert_eq!(request.kind, AssetKind::World);
        assert!(request.tags.is_empty());
        assert_eq!(request.thumbnail.media, ThumbnailMedia::Jpeg);
        assert_eq!(splat_scope_tag(&world), None);
        assert_eq!(splat_scope_tag(&object), Some("object"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_ply_fails_it_is_never_silently_skipped() {
        let dir = test_root("splat-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let item = IndexItem {
            file: "lib-1.ply".to_string(),
            domain: "splat".to_string(),
            content_type: "application/x-ply".to_string(),
            ..IndexItem::default()
        };
        // Not a PLY at all, a truncated body, and a well-formed header with
        // zero vertices: all three refuse rather than publishing a scene no
        // renderer can open.
        for bad in [
            b"not a ply at all".to_vec(),
            b"ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\n".to_vec(),
            ascii_splat_ply(0),
        ] {
            assert!(
                build_splat(&item, &dir, bad).is_err(),
                "malformed PLY must fail the import"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn splat_and_world_ply_rows_are_the_stage_primary_output() {
        // `text → image → splat`: the PLY is what the user asked for.
        let rows = [
            row("text", "text/plain", Some("run-8")),
            row("image", "image/png", Some("run-8")),
            row("splat", "application/x-ply", Some("run-8")),
        ];
        assert_eq!(classify_products(&rows), vec![false, false, true]);
        // FlashWorld: `text → image → world`, the world PLY is the product.
        let rows = [
            row("text", "text/plain", Some("run-9")),
            row("image", "image/png", Some("run-9")),
            row("world", "application/x-ply", Some("run-9")),
        ];
        assert_eq!(classify_products(&rows), vec![false, false, true]);
        // A lone splat row is its own product.
        assert_eq!(
            classify_products(&[row("splat", "application/x-ply", None)]),
            vec![true]
        );
    }

    #[test]
    fn splat_ply_publishes_end_to_end_as_world_splat_ply() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let root = test_root("splat-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("splat-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("splat-library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("lib-1.ply"), ascii_splat_ply(4)).unwrap();
        std::fs::write(library.join("lib-1.ply.thumb"), png_512(3)).unwrap();
        std::fs::write(library.join("lib-2.ply"), ascii_splat_ply(6)).unwrap();
        std::fs::write(library.join("lib-3.ply"), b"not a ply").unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[
                {"file":"lib-1.ply","label":"mossy stump","domain":"splat",
                 "content_type":"application/x-ply","prompt":"a mossy stump",
                 "group_id":"run-11","tags":["generated"],"product":true},
                {"file":"lib-2.ply","label":"coastal world","domain":"world",
                 "content_type":"application/x-ply","prompt":"a coast",
                 "group_id":"run-12","tags":["generated"],"product":true},
                {"file":"lib-3.ply","label":"broken","domain":"splat",
                 "content_type":"application/x-ply","prompt":"broken",
                 "group_id":"run-13","tags":["generated"],"product":true}
            ],"next_id":4}"#,
        )
        .unwrap();

        let rights = PublishRights::declared(
            "CC0-1.0",
            "",
            "",
            makepad_asset_data::Redistribution::Allowed,
            makepad_asset_data::DerivativePolicy::Allowed,
        );
        let report = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert_eq!(report.published.len(), 2, "{:?}", report);
        assert!(report.skipped_kind.is_empty(), "a PLY is never an unknown kind");
        assert_eq!(report.failed.len(), 1, "the malformed PLY fails, loudly");
        assert_eq!(report.failed[0].0, "lib-3.ply");
        assert!(report.failed[0].1.contains("ply:"), "{}", report.failed[0].1);

        // The catalog carries the splat category and the honest scope tags.
        let hits = client
            .catalog_search(
                &CatalogQuery {
                    category: Some("splat".into()),
                    ..CatalogQuery::browse(32)
                },
                None,
            )
            .expect("category search");
        assert_eq!(hits.hits.len(), 2);
        for tag in ["object", "world"] {
            let tagged = client
                .catalog_search(
                    &CatalogQuery { tag: Some(tag.into()), ..CatalogQuery::browse(32) },
                    None,
                )
                .expect("tag search");
            assert_eq!(tagged.hits.len(), 1, "one {tag} splat");
        }

        // The manifest that actually landed: World / Splat / Ply, under a
        // name a game can bind by.
        let stump = read_index(&library)
            .unwrap()
            .into_iter()
            .find(|item| item.file == "lib-1.ply")
            .unwrap();
        let alias =
            derived_alias(&stump, &std::fs::read(library.join("lib-1.ply")).unwrap(), "gen")
                .unwrap();
        assert!(alias.as_str().starts_with("gen/splat/mossy-stump-"), "{alias}");
        let resolved = client.resolve_alias(&alias).expect("splat alias");
        let manifest = client
            .fetch_asset_manifest(&resolved.head_revision)
            .expect("canonical manifest");
        assert_eq!(manifest.kind, AssetKind::World);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].role, FileRole::Splat);
        assert_eq!(manifest.files[0].media, MediaType::Ply);
        assert!(manifest.thumbnail.is_some());

        // Idempotent: a rerun republishes nothing (the malformed row keeps
        // failing, which is the point).
        let rerun = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(rerun.published.is_empty(), "{:?}", rerun.published);
        assert_eq!(rerun.skipped_existing.len(), 2);
        let _ = std::fs::remove_dir_all(library);
    }

    /// A single-triangle GLB with real measured topology, so the content
    /// contract's `triangles > 0 && vertices >= 3` mesh law is satisfied.
    fn triangle_glb(scale: f32) -> Vec<u8> {
        makepad_gltf::write_glb_mesh(
            &[[0.0, 0.0, 0.0], [scale, 0.0, 0.0], [0.0, scale, 0.0]],
            &[0, 1, 2],
        )
    }

    fn paint_row(file: &str, domain: &str, content_type: &str, product: bool) -> IndexItem {
        IndexItem {
            file: file.to_string(),
            label: "elf".to_string(),
            domain: domain.to_string(),
            content_type: content_type.to_string(),
            prompt: "a normal looking elf".to_string(),
            group_id: Some("run-paint".to_string()),
            tags: vec!["generated".to_string()],
            product: Some(product),
        }
    }

    /// The plan's view of one product row: `(file, role, lod)` in order.
    fn slots_of<'a>(plan: &'a RunPlan, product: &str) -> Vec<(&'a str, FileRole, u8)> {
        plan.files_of(product)
            .iter()
            .map(|f| (f.file.as_str(), f.role, f.lod))
            .collect()
    }

    #[test]
    fn a_run_hangs_its_maps_and_origin_pictures_off_the_product() {
        // One `image → matte → mesh → paint` run exactly as the app writes
        // it: the paint stage emits the textured GLB and then albedo/normal/
        // ORM, and the two earlier pictures are what it was made from.
        let items = vec![
            paint_row("lib-1.png", "image", "image/png", false),
            paint_row("lib-2.png", "matte", "image/png", false),
            paint_row("lib-3.glb", "mesh", "model/gltf-binary", false),
            paint_row("lib-4.glb", "paint", "model/gltf-binary", true),
            paint_row("lib-5.png", "paint", "image/png", false),
            paint_row("lib-6.png", "paint", "image/png", false),
            paint_row("lib-7.png", "paint", "image/png", false),
            paint_row("lib-8.json", "paint", "application/json", false),
        ];
        let plan = RunPlan::plan(&items);
        assert_eq!(
            slots_of(&plan, "lib-4.glb"),
            vec![
                // Maps in emission order = the paint backend's role order…
                ("lib-5.png", FileRole::Albedo, 0),
                ("lib-6.png", FileRole::Normal, 0),
                ("lib-7.png", FileRole::Orm, 0),
                // …then the origin pictures, nearest first.
                ("lib-2.png", FileRole::Source, 0),
                ("lib-1.png", FileRole::Source, 1),
            ]
        );
        for file in ["lib-5.png", "lib-6.png", "lib-7.png"] {
            assert_eq!(plan.owner_of(file).map(|(o, _)| o), Some("lib-4.glb"));
        }
        assert_eq!(plan.owner_of("lib-2.png"), Some(("lib-4.glb", FileRole::Source)));
        assert_eq!(plan.owner_of("lib-1.png"), Some(("lib-4.glb", FileRole::Source)));
        // The untextured mesh is scaffolding, not a carrier and not an asset.
        assert!(plan.files_of("lib-3.glb").is_empty());
        assert_eq!(plan.owner_of("lib-8.json"), None, "a sidecar is not a file");
        // The run's shape travels with the product.
        assert_eq!(
            plan.pipeline_of("run-paint").unwrap(),
            ["image", "matte", "mesh", "paint"]
        );
    }

    #[test]
    fn origin_pictures_stop_at_two_and_never_take_a_map_or_a_sibling_product() {
        // Deeper history than two pictures is scaffolding, and a candidate
        // fan-out must not flood one manifest.
        let items = vec![
            paint_row("lib-1.png", "image", "image/png", false),
            paint_row("lib-2.png", "image", "image/png", false),
            paint_row("lib-3.png", "image", "image/png", false),
            paint_row("lib-4.glb", "paint", "model/gltf-binary", true),
        ];
        let plan = RunPlan::plan(&items);
        assert_eq!(
            slots_of(&plan, "lib-4.glb"),
            vec![("lib-3.png", FileRole::Source, 0), ("lib-2.png", FileRole::Source, 1)]
        );
        assert_eq!(plan.owner_of("lib-1.png"), None, "the third picture is not carried");

        // A LATER product must not adopt an earlier product's channel maps
        // as origin pictures — they are already spoken for.
        let items = vec![
            paint_row("lib-1.glb", "paint", "model/gltf-binary", true),
            paint_row("lib-2.png", "paint", "image/png", false),
            paint_row("lib-3.png", "paint", "image/png", false),
            paint_row("lib-4.glb", "rig", "model/gltf-binary", true),
        ];
        let plan = RunPlan::plan(&items);
        assert!(slots_of(&plan, "lib-4.glb").is_empty(), "maps are not origins");
        assert_eq!(plan.owner_of("lib-2.png").map(|(o, _)| o), Some("lib-1.glb"));

        // An image run's four variants are each content, and each carries the
        // one input still the run started from.
        let mut items = vec![paint_row("lib-1.png", "image", "image/png", false)];
        for n in 2..=5 {
            items.push(paint_row(&format!("lib-{n}.png"), "upscale", "image/png", true));
        }
        let plan = RunPlan::plan(&items);
        for n in 2..=5 {
            assert_eq!(
                slots_of(&plan, &format!("lib-{n}.png")),
                vec![("lib-1.png", FileRole::Source, 0)],
                "variant {n} keeps its own identity and its origin"
            );
        }
        assert_eq!(plan.owner_of("lib-1.png").map(|(o, _)| o), Some("lib-2.png"));
    }

    #[test]
    fn attachment_rule_refuses_everything_it_cannot_prove() {
        // No group: an ungrouped row is its own asset, always.
        let mut ungrouped = vec![
            paint_row("lib-1.glb", "paint", "model/gltf-binary", true),
            paint_row("lib-2.png", "paint", "image/png", false),
        ];
        for row in &mut ungrouped {
            row.group_id = None;
        }
        let plan = RunPlan::plan(&ungrouped);
        assert_eq!(plan.owner_of("lib-2.png"), None);

        // Different runs never borrow each other's rows.
        let mut other = paint_row("lib-2.png", "paint", "image/png", false);
        other.group_id = Some("run-other".to_string());
        let plan = RunPlan::plan(&[
            paint_row("lib-1.glb", "paint", "model/gltf-binary", true),
            other,
        ]);
        assert_eq!(plan.owner_of("lib-2.png"), None);

        // An image BEFORE the mesh is not a map — it is the origin picture.
        let plan = RunPlan::plan(&[
            paint_row("lib-1.png", "paint", "image/png", false),
            paint_row("lib-2.glb", "paint", "model/gltf-binary", true),
        ]);
        assert_eq!(plan.owner_of("lib-1.png"), Some(("lib-2.glb", FileRole::Source)));

        // A non-geometry domain's images are content, not channel maps: with
        // no product in the group nothing is carried at all.
        let plan = RunPlan::plan(&[
            paint_row("lib-1.glb", "image", "model/gltf-binary", false),
            paint_row("lib-2.png", "image", "image/png", false),
        ]);
        assert_eq!(plan.owner_of("lib-2.png"), None);

        // The run's own product is never one of its mesh's maps.
        let plan = RunPlan::plan(&[
            paint_row("lib-1.glb", "paint", "model/gltf-binary", false),
            paint_row("lib-2.png", "paint", "image/png", true),
        ]);
        assert_eq!(plan.owner_of("lib-2.png"), None);

        // Out-of-scope (pack import) rows are not this importer's business.
        let mut pack: Vec<IndexItem> = vec![
            paint_row("lib-1.glb", "paint", "model/gltf-binary", true),
            paint_row("lib-2.png", "paint", "image/png", false),
        ];
        for row in &mut pack {
            row.tags = vec!["kenney".to_string()];
            row.group_id = Some("import:kenney:blocks".to_string());
        }
        let plan = RunPlan::plan(&pack);
        assert_eq!(plan.owner_of("lib-2.png"), None);
    }

    #[test]
    fn run_identity_is_one_label_and_one_owner_only_provenance_line() {
        let mut item = paint_row("lib-1.glb", "paint", "model/gltf-binary", true);
        let plan = RunPlan::plan(std::slice::from_ref(&item));
        // The catalog label charset has no `:`, so the vocabulary is `run-`.
        assert_eq!(run_tag(&item).as_deref(), Some("run-paint"));
        assert_eq!(
            run_provenance(&item, &plan),
            "run=run-paint pipeline=paint prompt=a normal looking elf"
        );
        // Producers that are not pipeline runs get the prefix, once.
        item.group_id = Some("drop-2026".to_string());
        assert_eq!(run_tag(&item).as_deref(), Some("run-drop-2026"));
        item.group_id = Some("Webcam Snap!".to_string());
        assert_eq!(run_tag(&item).as_deref(), Some("run-webcam-snap"));
        // A tag the catalog could never store is not invented.
        item.group_id = Some("···".to_string());
        assert_eq!(run_tag(&item), None);
        item.group_id = None;
        assert_eq!(run_tag(&item), None);
        assert_eq!(run_provenance(&item, &plan), "prompt=a normal looking elf");
        // Every label the rule can produce is inside the catalog's charset.
        for group in ["run-7", "drop-a b c", "webcam-ÉLAN", &"x".repeat(200)] {
            let mut row = paint_row("lib-1.png", "image", "image/png", true);
            row.group_id = Some(group.to_string());
            let tag = run_tag(&row).expect("a tag");
            assert!(tag.len() <= MAX_LABEL_BYTES, "{tag}");
            assert!(tag.as_bytes()[0].is_ascii_alphanumeric(), "{tag}");
            assert!(
                tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{tag}"
            );
        }
    }

    #[test]
    fn extra_maps_fall_back_to_texture_slots_and_names_win_when_present() {
        // A named payload states its own role; unnamed ones take the next
        // free slot, and a fourth map has no known meaning at all.
        let items = vec![
            paint_row("lib-1.glb", "paint", "model/gltf-binary", true),
            paint_row("lib-2.normal.png", "paint", "image/png", false),
            paint_row("lib-3.png", "paint", "image/png", false),
            paint_row("lib-4.png", "paint", "image/png", false),
            paint_row("lib-5.png", "paint", "image/png", false),
            paint_row("lib-6.png", "paint", "image/png", false),
        ];
        let plan = RunPlan::plan(&items);
        assert_eq!(
            slots_of(&plan, "lib-1.glb"),
            vec![
                ("lib-2.normal.png", FileRole::Normal, 0),
                ("lib-3.png", FileRole::Albedo, 0),
                ("lib-4.png", FileRole::Orm, 0),
                ("lib-5.png", FileRole::Texture, 0),
                ("lib-6.png", FileRole::Texture, 1),
            ]
        );
        // The label carries prompt text ("a normal looking elf") and must
        // never be read as a role hint.
        assert_eq!(map_role_hint("lib-3.png"), None);
        assert_eq!(map_role_hint("crate_albedo.png"), Some(FileRole::Albedo));
        assert_eq!(map_role_hint("crate-ORM.png"), Some(FileRole::Orm));
    }

    #[test]
    fn a_paint_run_publishes_one_mesh_asset_carrying_maps_and_bakes() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let root = test_root("paint-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("paint-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("paint-library");
        std::fs::create_dir_all(&library).unwrap();
        // `image → matte → mesh → paint`: source still, cutout, untextured
        // mesh, textured mesh + albedo/normal/ORM + the run's JSON sidecar.
        std::fs::write(library.join("lib-1.png"), png_512(1)).unwrap();
        std::fs::write(library.join("lib-2.png"), png_512(2)).unwrap();
        std::fs::write(library.join("lib-3.glb"), triangle_glb(1.0)).unwrap();
        let painted = triangle_glb(2.0);
        std::fs::write(library.join("lib-4.glb"), &painted).unwrap();
        for (file, seed) in [("lib-5.png", 5u8), ("lib-6.png", 6), ("lib-7.png", 7)] {
            std::fs::write(library.join(file), png_512(seed)).unwrap();
        }
        std::fs::write(library.join("lib-8.json"), b"{\"material\":{}}").unwrap();
        // The bakes the Asset UI writes beside the landed GLB.
        std::fs::write(library.join("lib-4.aomesh"), b"aomesh bytes").unwrap();
        std::fs::write(library.join("lib-4.ao.png"), png_512(9)).unwrap();
        std::fs::write(library.join("lib-4.shadowsdf"), b"sdf bytes").unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[
                {"file":"lib-1.png","label":"source","domain":"image","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-2.png","label":"cutout","domain":"matte","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-3.glb","label":"mesh","domain":"mesh","content_type":"model/gltf-binary",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-4.glb","label":"elf","domain":"paint","content_type":"model/gltf-binary",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":true},
                {"file":"lib-5.png","label":"elf","domain":"paint","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-6.png","label":"elf","domain":"paint","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-7.png","label":"elf","domain":"paint","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false},
                {"file":"lib-8.json","label":"material","domain":"paint","content_type":"application/json",
                 "prompt":"an elf","group_id":"run-paint","tags":["generated"],"product":false}
            ],"next_id":9}"#,
        )
        .unwrap();

        let rights = PublishRights::generated_cc0();
        let report = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        // EXACTLY ONE asset: the textured mesh. The still, the cutout and the
        // three maps are files of it; the untextured mesh is scaffolding.
        assert_eq!(
            report.published.iter().map(|(file, _)| file.as_str()).collect::<Vec<_>>(),
            vec!["lib-4.glb"]
        );
        assert_eq!(
            report.skipped_attached,
            vec![
                ("lib-1.png".to_string(), "lib-4.glb".to_string()),
                ("lib-2.png".to_string(), "lib-4.glb".to_string()),
                ("lib-5.png".to_string(), "lib-4.glb".to_string()),
                ("lib-6.png".to_string(), "lib-4.glb".to_string()),
                ("lib-7.png".to_string(), "lib-4.glb".to_string()),
            ]
        );
        assert_eq!(
            report.skipped_intermediate,
            vec![(
                "lib-3.glb".to_string(),
                "intermediate: stage output of run-paint, not its product".to_string()
            )],
            "every skip states its reason"
        );
        assert_eq!(report.skipped_kind, vec!["lib-8.json".to_string()]);

        // ONE asset, every file of the mesh inside its manifest — under the
        // readable name a game binds by.
        let rows = read_index(&library).unwrap();
        let row = |file: &str| rows.iter().find(|item| item.file == file).unwrap().clone();
        let alias = derived_alias(&row("lib-4.glb"), &painted, "gen").unwrap();
        assert!(alias.as_str().starts_with("gen/paint/elf-"), "{alias}");
        assert_eq!(alias.as_str().len(), "gen/paint/elf-".len() + 8, "{alias}");
        let resolved = client.resolve_alias(&alias).expect("mesh alias");
        let manifest = client
            .fetch_asset_manifest(&resolved.head_revision)
            .expect("canonical manifest");
        assert_eq!(manifest.kind, AssetKind::Mesh);
        let slots: Vec<(FileRole, u8, MediaType)> =
            manifest.files.iter().map(|f| (f.role, f.lod, f.media)).collect();
        assert_eq!(
            slots,
            vec![
                (FileRole::RenderGlb, 0, MediaType::Glb),
                (FileRole::AoMesh, 0, MediaType::Bin),
                (FileRole::ShadowSdf, 0, MediaType::Bin),
                (FileRole::Albedo, 0, MediaType::Png),
                (FileRole::Normal, 0, MediaType::Png),
                (FileRole::Orm, 0, MediaType::Png),
                // The cutout it was made from, then the still before it.
                (FileRole::Source, 0, MediaType::Png),
                (FileRole::Source, 1, MediaType::Png),
                (FileRole::AoTexture, 0, MediaType::Png),
            ],
            "canonical role order, one asset carrying the whole mesh"
        );
        for role in [FileRole::Albedo, FileRole::Normal, FileRole::Orm, FileRole::AoTexture] {
            let file = manifest.files.iter().find(|f| f.role == role).unwrap();
            let dims = file.dims.expect("image files carry measured dims");
            assert_eq!((dims.width, dims.height), (512, 512));
        }
        // The origin pictures are the exact library bytes, in walk order.
        let source_blob = |lod: u8| {
            manifest
                .files
                .iter()
                .find(|f| f.role == FileRole::Source && f.lod == lod)
                .unwrap()
                .blob
        };
        let blob_of = |file: &str| BlobId::hash_of(&std::fs::read(library.join(file)).unwrap());
        assert_eq!(source_blob(0), blob_of("lib-2.png"), "the cutout is the primary");
        assert_eq!(source_blob(1), blob_of("lib-1.png"));
        assert!(manifest.thumbnail.is_some());
        assert!(manifest.metrics.triangles > 0);

        // NOTHING else is in the catalog, and the one asset carries the run.
        let all = client
            .catalog_search(&CatalogQuery::browse(32), None)
            .expect("browse");
        assert_eq!(all.hits.len(), 1, "no maps, no origin pics, no scaffolding");
        assert_eq!(all.hits[0].title, "elf");
        let run = client
            .catalog_search(
                &CatalogQuery { tag: Some("run-paint".into()), ..CatalogQuery::browse(32) },
                None,
            )
            .expect("run tag search");
        assert_eq!(run.hits.len(), 1);
        assert_eq!(run.hits[0].asset_id, resolved.asset_id);
        // The provenance line (owner-only indexed) reached the server: only
        // the pipeline text mentions the run's earlier stages — no public
        // field of this asset does. The client API exposes no annotation
        // read-back, so the index is the observable.
        let staged = client
            .catalog_search(&CatalogQuery::text("matte", 32), None)
            .expect("provenance term search");
        assert_eq!(staged.hits.len(), 1, "the run's pipeline is on the product");
        assert_eq!(staged.hits[0].asset_id, resolved.asset_id);
        for attached in ["lib-1.png", "lib-2.png", "lib-5.png", "lib-6.png", "lib-7.png"] {
            let bytes = std::fs::read(library.join(attached)).unwrap();
            let own_alias = derived_alias(&row(attached), &bytes, "gen").unwrap();
            assert!(
                matches!(client.resolve_alias(&own_alias), Err(ClientError::NotFound { .. })),
                "{attached} must not exist as an asset of its own"
            );
        }

        // The watcher's seam: ONE ready row plus the whole-snapshot plan. A
        // map observed on its own still never becomes an asset.
        let plan = RunPlan::plan(&rows);
        let map_row = row("lib-5.png");
        let single = import_items(
            &mut client,
            &library,
            "gen",
            &rights,
            std::slice::from_ref(&map_row),
            &plan,
            false,
        );
        assert!(single.published.is_empty());
        assert_eq!(
            single.skipped_attached,
            vec![("lib-5.png".to_string(), "lib-4.glb".to_string())]
        );

        // A second pass publishes nothing new.
        let rerun = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(rerun.failed.is_empty(), "{:?}", rerun.failed);
        assert!(rerun.published.is_empty(), "{:?}", rerun.published);
        assert_eq!(rerun.skipped_existing, vec!["lib-4.glb".to_string()]);
        assert_eq!(rerun.skipped_attached.len(), 5);
        assert_eq!(rerun.skipped_intermediate.len(), 1);
        let _ = std::fs::remove_dir_all(library);
    }

    #[test]
    fn an_image_runs_variants_each_stay_content_and_each_carry_the_origin() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let root = test_root("variants-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("variants-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("variants-library");
        std::fs::create_dir_all(&library).unwrap();
        // One input still, four upscales of it — an image run's variants are
        // each their own content.
        for (file, seed) in [
            ("lib-1.png", 1u8),
            ("lib-2.png", 2),
            ("lib-3.png", 3),
            ("lib-4.png", 4),
            ("lib-5.png", 5),
        ] {
            std::fs::write(library.join(file), png_512(seed)).unwrap();
        }
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[
                {"file":"lib-1.png","label":"input","domain":"image","content_type":"image/png",
                 "prompt":"a neon city","group_id":"run-img","tags":["generated"],"product":false},
                {"file":"lib-2.png","label":"neon city","domain":"upscale","content_type":"image/png",
                 "prompt":"a neon city","group_id":"run-img","tags":["generated"],"product":true},
                {"file":"lib-3.png","label":"neon city","domain":"upscale","content_type":"image/png",
                 "prompt":"a neon city","group_id":"run-img","tags":["generated"],"product":true},
                {"file":"lib-4.png","label":"neon city","domain":"upscale","content_type":"image/png",
                 "prompt":"a neon city","group_id":"run-img","tags":["generated"],"product":true},
                {"file":"lib-5.png","label":"neon city","domain":"upscale","content_type":"image/png",
                 "prompt":"a neon city","group_id":"run-img","tags":["generated"],"product":true}
            ],"next_id":6}"#,
        )
        .unwrap();

        let rights = PublishRights::generated_cc0();
        let report = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(
            report.published.iter().map(|(file, _)| file.as_str()).collect::<Vec<_>>(),
            vec!["lib-2.png", "lib-3.png", "lib-4.png", "lib-5.png"],
            "each variant is its own asset"
        );
        assert_eq!(
            report.skipped_attached,
            vec![("lib-1.png".to_string(), "lib-2.png".to_string())],
            "the input still is a file, not an asset"
        );

        // Four assets, one run label, and every one of them carries the same
        // origin picture under a name of its own.
        let all = client
            .catalog_search(&CatalogQuery::browse(32), None)
            .expect("browse");
        assert_eq!(all.hits.len(), 4);
        let run = client
            .catalog_search(
                &CatalogQuery { tag: Some("run-img".into()), ..CatalogQuery::browse(32) },
                None,
            )
            .expect("run tag search");
        assert_eq!(run.hits.len(), 4, "one tag query finds the whole run");
        let rows = read_index(&library).unwrap();
        let origin = BlobId::hash_of(&std::fs::read(library.join("lib-1.png")).unwrap());
        let mut aliases = Vec::new();
        for file in ["lib-2.png", "lib-3.png", "lib-4.png", "lib-5.png"] {
            let row = rows.iter().find(|item| item.file == file).unwrap();
            let alias =
                derived_alias(row, &std::fs::read(library.join(file)).unwrap(), "gen").unwrap();
            assert!(alias.as_str().starts_with("gen/upscale/neon-city-"), "{alias}");
            let head = client.resolve_alias(&alias).expect("variant alias").head_revision;
            let manifest = client.fetch_asset_manifest(&head).unwrap();
            assert_eq!(
                manifest.files.iter().map(|f| (f.role, f.lod)).collect::<Vec<_>>(),
                vec![(FileRole::Texture, 0), (FileRole::Source, 0)]
            );
            assert_eq!(
                manifest.files.iter().find(|f| f.role == FileRole::Source).unwrap().blob,
                origin,
                "one still is the origin of every variant"
            );
            aliases.push(alias.as_str().to_string());
        }
        aliases.sort();
        aliases.dedup();
        assert_eq!(aliases.len(), 4, "same label, four distinct names");

        // A second pass publishes nothing.
        let rerun = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(rerun.published.is_empty(), "{:?}", rerun.published);
        assert_eq!(rerun.skipped_existing.len(), 4);
        assert_eq!(rerun.skipped_attached.len(), 1);
        let _ = std::fs::remove_dir_all(library);
    }

    #[test]
    fn maps_that_land_after_their_mesh_published_still_reach_it() {
        use makepad_asset_client::{ApiEndpoints, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let root = test_root("late-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("late-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("late-library");
        std::fs::create_dir_all(&library).unwrap();
        let painted = triangle_glb(3.0);
        std::fs::write(library.join("lib-1.glb"), &painted).unwrap();
        let mesh_only = br#"{"items":[
            {"file":"lib-1.glb","label":"elf","domain":"paint","content_type":"model/gltf-binary",
             "prompt":"an elf","group_id":"run-late","tags":["generated"],"product":true}
        ],"next_id":2}"#;
        std::fs::write(library.join("index.json"), mesh_only).unwrap();
        let rights = PublishRights::generated_cc0();
        let first = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert_eq!(first.published.len(), 1, "{:?}", first);
        let mesh_row = read_index(&library).unwrap().remove(0);
        let alias = derived_alias(&mesh_row, &painted, "gen").unwrap();
        let head = client.resolve_alias(&alias).unwrap().head_revision;
        assert_eq!(client.fetch_asset_manifest(&head).unwrap().files.len(), 1);

        // The albedo row lands one index commit later — the mesh's alias
        // already resolves, so only the manifest comparison can catch it.
        std::fs::write(library.join("lib-2.png"), png_512(9)).unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[
                {"file":"lib-1.glb","label":"elf","domain":"paint","content_type":"model/gltf-binary",
                 "prompt":"an elf","group_id":"run-late","tags":["generated"],"product":true},
                {"file":"lib-2.png","label":"elf","domain":"paint","content_type":"image/png",
                 "prompt":"an elf","group_id":"run-late","tags":["generated"],"product":false}
            ],"next_id":3}"#,
        )
        .unwrap();
        let second = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(second.failed.is_empty(), "{:?}", second.failed);
        assert_eq!(second.published.len(), 1, "the mesh gains a revision");
        assert_eq!(second.skipped_attached.len(), 1);
        let head = client.resolve_alias(&alias).unwrap().head_revision;
        let manifest = client.fetch_asset_manifest(&head).unwrap();
        assert_eq!(
            manifest.files.iter().map(|f| f.role).collect::<Vec<_>>(),
            vec![FileRole::RenderGlb, FileRole::Albedo]
        );
        // …and it settles: a third pass is a no-op.
        let third = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(third.published.is_empty(), "{:?}", third.published);
        assert_eq!(third.skipped_existing, vec!["lib-1.glb".to_string()]);
        let _ = std::fs::remove_dir_all(library);
    }

    #[test]
    fn same_name_different_bytes_never_collide_and_a_rename_keeps_the_asset() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let root = test_root("name-server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("name-cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("name-library");
        std::fs::create_dir_all(&library).unwrap();
        // TWO different pictures the user called the same thing.
        std::fs::write(library.join("lib-1.png"), png_512(1)).unwrap();
        std::fs::write(library.join("lib-2.png"), png_512(2)).unwrap();
        let index = |first_label: &str| {
            format!(
                r#"{{"items":[
                {{"file":"lib-1.png","label":"{first_label}","domain":"image","content_type":"image/png",
                 "prompt":"a wooden crate","group_id":"run-name","tags":["generated"],"product":true}},
                {{"file":"lib-2.png","label":"Wooden Crate","domain":"image","content_type":"image/png",
                 "prompt":"a wooden crate","group_id":"run-name","tags":["generated"],"product":true}}
            ],"next_id":3}}"#
            )
        };
        std::fs::write(library.join("index.json"), index("Wooden Crate")).unwrap();

        let rights = PublishRights::generated_cc0();
        let report = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.published.len(), 2);

        // Same human name, different bytes → two readable aliases that only
        // differ in their digest suffix, pointing at two different assets.
        let rows = read_index(&library).unwrap();
        let alias_of = |file: &str, rows: &[IndexItem]| {
            let row = rows.iter().find(|item| item.file == file).unwrap();
            derived_alias(row, &std::fs::read(library.join(file)).unwrap(), "gen").unwrap()
        };
        let one = alias_of("lib-1.png", &rows);
        let two = alias_of("lib-2.png", &rows);
        assert!(one.as_str().starts_with("gen/image/wooden-crate-"), "{one}");
        assert!(two.as_str().starts_with("gen/image/wooden-crate-"), "{two}");
        assert_ne!(one.as_str(), two.as_str(), "the digest suffix keeps them apart");
        let first = client.resolve_alias(&one).expect("first alias");
        let second = client.resolve_alias(&two).expect("second alias");
        assert_ne!(first.asset_id, second.asset_id);

        // Idempotent: a second pass publishes nothing.
        let rerun = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(rerun.published.is_empty(), "{:?}", rerun.published);
        assert_eq!(rerun.skipped_existing.len(), 2);

        // Rename the FIRST row. Same bytes → same asset id, new name, and
        // the catalog gains no second copy of anything.
        std::fs::write(library.join("index.json"), index("Steel Crate")).unwrap();
        let renamed = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(renamed.failed.is_empty(), "{:?}", renamed.failed);
        assert_eq!(
            renamed.published.iter().map(|(file, _)| file.as_str()).collect::<Vec<_>>(),
            vec!["lib-1.png"],
            "only the renamed row is republished"
        );
        let rows = read_index(&library).unwrap();
        let renamed_alias = alias_of("lib-1.png", &rows);
        assert!(renamed_alias.as_str().starts_with("gen/image/steel-crate-"), "{renamed_alias}");
        assert_eq!(
            renamed_alias.as_str().rsplit('-').next(),
            one.as_str().rsplit('-').next(),
            "the digest suffix is the bytes, so it does not move on a rename"
        );
        let after = client.resolve_alias(&renamed_alias).expect("renamed alias");
        assert_eq!(after.asset_id, first.asset_id, "a rename is the SAME asset");
        // The title is mutable control-plane data, not manifest identity, so
        // the immutable revision is untouched — a rename re-annotates and
        // re-names, it never mints content.
        assert_eq!(after.head_revision, first.head_revision);
        let all = client
            .catalog_search(&CatalogQuery::browse(32), None)
            .expect("browse");
        assert_eq!(all.hits.len(), 2, "no duplicate asset was created");
        let mut titles: Vec<&str> = all.hits.iter().map(|hit| hit.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, vec!["Steel Crate", "Wooden Crate"]);
        // The old name still resolves to the same asset — names accumulate,
        // nothing is ever left dangling.
        assert_eq!(client.resolve_alias(&one).unwrap().asset_id, first.asset_id);

        // …and it settles again.
        let settled = import_library(&mut client, &library, "gen", &rights, false).unwrap();
        assert!(settled.published.is_empty(), "{:?}", settled.published);
        assert_eq!(settled.skipped_existing.len(), 2);
        let _ = std::fs::remove_dir_all(library);
    }

    #[test]
    fn importer_recovers_published_without_alias_then_skips_exact_rerun() {
        use makepad_asset_store::{AssetServer, ServerConfig};
        use makepad_asset_client::{ApiEndpoints, ClientConfig};

        let root = test_root("server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints {
                control: server.control_addr(),
                data: server.data_addr(),
            },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("library");
        std::fs::create_dir_all(&library).unwrap();
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&512u32.to_be_bytes());
        png[20..24].copy_from_slice(&512u32.to_be_bytes());
        std::fs::write(library.join("lib-1.png"), &png).unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[{"file":"lib-1.png","label":"Recovered PNG","domain":"image","content_type":"image/png","prompt":"test prompt","group_id":"run-1","tags":["generated"],"product":true}],"next_id":2}"#,
        )
        .unwrap();

        let item = IndexItem {
            file: "lib-1.png".to_string(),
            label: "Recovered PNG".to_string(),
            domain: "image".to_string(),
            content_type: "image/png".to_string(),
            prompt: "test prompt".to_string(),
            group_id: Some("run-1".to_string()),
            tags: vec!["generated".to_string()],
            product: Some(true),
        };
        let (asset_id, alias) = derived_identity(&item, &png, "gen").unwrap();
        assert!(alias.as_str().starts_with("gen/image/recovered-png-"), "{alias}");
        let mut partial = build_png(&item, png).unwrap();
        partial.namespace = "gen".to_string();
        partial.asset_id = Some(asset_id);
        partial.prompt = item.prompt.clone();
        partial.creator = "ai-content-library".to_string();
        partial.tags.push(item.domain.clone());
        client
            .publish_artifact(&partial)
            .expect("land revision without the importer alias");
        assert!(matches!(
            client.resolve_alias(&alias),
            Err(ClientError::NotFound { .. })
        ));

        let recovered = import_library(
            &mut client,
            &library,
            "gen",
            &PublishRights::declared(
                "CC0-1.0",
                "",
                "",
                makepad_asset_data::Redistribution::Allowed,
                makepad_asset_data::DerivativePolicy::Allowed,
            ),
            false,
        )
        .unwrap();
        assert!(recovered.failed.is_empty(), "{:?}", recovered.failed);
        assert_eq!(recovered.published.len(), 1);
        let resolved = client.resolve_alias(&alias).expect("importer recovered alias");
        assert_eq!(resolved.asset_id, asset_id);

        let rerun = import_library(
            &mut client,
            &library,
            "gen",
            &PublishRights::declared(
                "CC0-1.0",
                "",
                "",
                makepad_asset_data::Redistribution::Allowed,
                makepad_asset_data::DerivativePolicy::Allowed,
            ),
            false,
        )
        .unwrap();
        assert!(rerun.failed.is_empty(), "{:?}", rerun.failed);
        assert_eq!(rerun.skipped_existing, vec!["lib-1.png".to_string()]);
        assert!(rerun.published.is_empty());
    }
}
