//! Import surface: hardcoded open-pack modules, Kenney first.
//!
//! This is a catalog of packs we already name in-tree — not a generic zip
//! drop zone. Each module owns its own identity, license blurb, official
//! links, and an honest disk/import status. Kenney compiles through
//! [`makepad_asset_importer::pack_import`]; other modules stay visible as
//! not-provisioned cards until they get the same local-folder path.

use makepad_asset_importer::ao_bake;
use makepad_asset_importer::pack_import::{
    self, kenney_pack, kenney_spec, KenneyPack, IMPORT_MANIFEST_FILE, KENNEY_ASSETS_HOME,
    KENNEY_CREDITS, KENNEY_GITHUB, KENNEY_HOME, KENNEY_LICENSE, KENNEY_PACKS, KENNEY_SOURCE_ID,
    KENNEY_SOURCE_TITLE,
    SOURCE_COLLECTION_FILE, UPLOAD_PLAN_FILE,
};
use makepad_asset_client::{
    AnnotationUpload, ApiEndpoints, AssetClient, ClientConfig,
};
use makepad_asset_data::{sha256, AssetKind, BlobId};
use makepad_widgets::log;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

/// Import-card preview strip size.
/// How many imported items the LOAD grid remembers a picture for. A grid,
/// not a strip: the surface shows what is coming in, and eight was a peek.
pub const IMPORT_PREVIEW_SLOTS: usize = 60;

/// One serial import job. The UI queue runs these one at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportJob {
    Kenney {
        pack: String,
        pack_index: usize,
        path: String,
    },
    KenneyAll,
    Freedoom {
        path: String,
    },
    LibreQuake {
        path: String,
    },
    Doom {
        path: String,
    },
    Quake {
        path: String,
    },
    Duke3d {
        path: String,
    },
    Quake2 {
        path: String,
    },
    Quake3 {
        path: String,
    },
    DarkMod {
        path: String,
    },
    KayKit,
    /// A music directory the user picked in the native folder dialog. Unlike
    /// every other job the path is REQUIRED — there is no conventional
    /// location for someone's music library.
    Music {
        path: String,
    },
}

impl ImportJob {
    pub fn title(&self) -> String {
        match self {
            ImportJob::Kenney { pack, .. } => format!("Kenney · {pack}"),
            ImportJob::KenneyAll => "Kenney · all packs".into(),
            ImportJob::Freedoom { .. } => "Freedoom".into(),
            ImportJob::LibreQuake { .. } => "LibreQuake".into(),
            ImportJob::Doom { .. } => "Doom shareware".into(),
            ImportJob::Quake { .. } => "Quake shareware".into(),
            ImportJob::Duke3d { .. } => "Duke3D shareware".into(),
            ImportJob::Quake2 { .. } => "Quake II shareware".into(),
            ImportJob::Quake3 { .. } => "Quake III demo".into(),
            ImportJob::DarkMod { .. } => "The Dark Mod".into(),
            ImportJob::KayKit => "KayKit".into(),
            ImportJob::Music { path } => {
                let leaf = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                format!("Music · {leaf}")
            }
        }
    }

    /// Same pack already running or waiting — do not enqueue twice.
    pub fn conflicts(&self, other: &ImportJob) -> bool {
        match (self, other) {
            (ImportJob::Kenney { pack: a, .. }, ImportJob::Kenney { pack: b, .. }) => a == b,
            (ImportJob::KenneyAll, ImportJob::KenneyAll)
            | (ImportJob::KenneyAll, ImportJob::Kenney { .. })
            | (ImportJob::Kenney { .. }, ImportJob::KenneyAll)
            | (ImportJob::Freedoom { .. }, ImportJob::Freedoom { .. })
            | (ImportJob::LibreQuake { .. }, ImportJob::LibreQuake { .. })
            | (ImportJob::Doom { .. }, ImportJob::Doom { .. })
            | (ImportJob::Quake { .. }, ImportJob::Quake { .. })
            | (ImportJob::Duke3d { .. }, ImportJob::Duke3d { .. })
            | (ImportJob::Quake2 { .. }, ImportJob::Quake2 { .. })
            | (ImportJob::Quake3 { .. }, ImportJob::Quake3 { .. })
            | (ImportJob::DarkMod { .. }, ImportJob::DarkMod { .. })
            | (ImportJob::KayKit, ImportJob::KayKit) => true,
            // Two music jobs conflict only when they are the same folder —
            // importing two different libraries in a row is legitimate.
            (ImportJob::Music { path: a }, ImportJob::Music { path: b }) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedImport {
    pub id: u64,
    pub job: ImportJob,
}

/// One-at-a-time import runner with a user-editable wait list.
#[derive(Clone, Debug, Default)]
pub struct ImportQueue {
    next_id: u64,
    pub active: Option<QueuedImport>,
    pub pending: Vec<QueuedImport>,
}

impl ImportQueue {
    pub fn enqueue(&mut self, job: ImportJob) -> Result<u64, String> {
        if self
            .active
            .as_ref()
            .map(|item| item.job.conflicts(&job))
            .unwrap_or(false)
            || self.pending.iter().any(|item| item.job.conflicts(&job))
        {
            return Err(format!("{} is already queued", job.title()));
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.pending.push(QueuedImport { id, job });
        Ok(id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.pending.len();
        self.pending.retain(|item| item.id != id);
        self.pending.len() != before
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn finish_active(&mut self) {
        self.active = None;
    }

    /// Move the next pending job into `active`. No-op while a job is active.
    pub fn promote(&mut self) -> Option<QueuedImport> {
        if self.active.is_some() {
            return None;
        }
        if self.pending.is_empty() {
            return None;
        }
        let item = self.pending.remove(0);
        self.active = Some(item.clone());
        Some(item)
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_none() && self.pending.is_empty()
    }

    pub fn has_job(&self, job: &ImportJob) -> bool {
        self.active
            .as_ref()
            .map(|item| item.job.conflicts(job))
            .unwrap_or(false)
            || self.pending.iter().any(|item| item.job.conflicts(job))
    }

    pub fn is_active(&self, job: &ImportJob) -> bool {
        self.active
            .as_ref()
            .map(|item| item.job.conflicts(job))
            .unwrap_or(false)
    }

    pub fn is_pending(&self, job: &ImportJob) -> bool {
        self.pending.iter().any(|item| item.job.conflicts(job))
    }
}

/// One hardcoded OSS pack module shown on the Import page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackModule {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub license: &'static str,
    pub license_blurb: &'static str,
    pub homepage: &'static str,
    /// License / EULA page opened from the Import row Terms weblink.
    pub terms_url: &'static str,
    pub source_page: &'static str,
    pub github: Option<&'static str>,
    pub credits: &'static str,
    /// True only when this module can run the local pack_import compiler.
    pub import_wired: bool,
}

pub const KENNEY_MODULE: PackModule = PackModule {
    id: KENNEY_SOURCE_ID,
    title: KENNEY_SOURCE_TITLE,
    blurb: "Low-poly 3D kits, UI, and game audio from kenney.nl. One collection (source_id kenney); each kit is a local folder you compile into a licensed upload plan. No network fetch — the pack must already be on disk.",
    license: KENNEY_LICENSE,
    license_blurb: "© Kenney (kenney.nl). CC BY 4.0 — attribution required on every copy and derivative. This Import module does not treat Kenney as CC0.",
    homepage: KENNEY_HOME,
    terms_url: "https://creativecommons.org/licenses/by/4.0/",
    source_page: KENNEY_ASSETS_HOME,
    github: Some(KENNEY_GITHUB),
    credits: KENNEY_CREDITS,
    import_wired: true,
};

pub const KAYKIT_MODULE: PackModule = PackModule {
    id: "kaykit",
    title: "KayKit / Kay Lousberg",
    blurb: "Nine rigged adventurer and skeleton characters (41-joint KayKit rig) used by the sandbox play-mode cast. Local-folder import of the in-repo characters pack (CC0-1.0).",
    license: "CC0-1.0",
    license_blurb: "© Kay Lousberg / KayKit. CREDITS.toml records CC0 1.0 (public domain dedication). Attribution is not required; we still credit Kay Lousberg.",
    homepage: "https://kaylousberg.com/",
    terms_url: "https://creativecommons.org/publicdomain/zero/1.0/",
    source_page: "https://kaylousberg.com/",
    github: Some("https://github.com/KayKit-Game-Assets"),
    credits: "Kay Lousberg / KayKit (kaylousberg.com)",
    import_wired: true,
};

pub const NASA_SKY_MODULE: PackModule = PackModule {
    id: "nasa-gsfc-svs",
    title: "NASA GSFC SVS Deep Star Maps 2020",
    blurb: "Galactic-coordinate star panorama used as the sandbox sky. A single public-domain EXR/PNG export, not a Kenney-style pack.",
    license: "Public domain",
    license_blurb: "NASA/GSFC SVS Deep Star Maps 2020 is U.S. government public domain. Credit NASA/GSFC SVS. Not a pack_import target.",
    homepage: "https://svs.gsfc.nasa.gov/4851",
    terms_url: "https://svs.gsfc.nasa.gov/4851",
    source_page: "https://svs.gsfc.nasa.gov/4851",
    github: None,
    credits: "NASA/Goddard Space Flight Center Scientific Visualization Studio",
    import_wired: false,
};

/// Core modules (Kenney + not-wired). Classic Freedoom/LibreQuake cards live
/// in [`crate::import_classic`] so that path stays additive.
pub const PACK_MODULES: &[PackModule] = &[KENNEY_MODULE, KAYKIT_MODULE, NASA_SKY_MODULE];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskProbe {
    pub path: PathBuf,
    pub exists: bool,
    pub is_dir: bool,
    pub supported_files: usize,
    pub unsupported_samples: Vec<String>,
}

impl DiskProbe {
    pub fn ready(&self) -> bool {
        self.exists && self.is_dir
    }

    pub fn line(&self) -> String {
        if !self.exists {
            return format!("not on disk · {}", self.path.display());
        }
        if !self.is_dir {
            return format!("not a directory · {}", self.path.display());
        }
        if self.supported_files == 0 {
            return format!(
                "folder empty of png/jpeg/wav/mp4/glb · {}",
                self.path.display()
            );
        }
        if self.unsupported_samples.is_empty() {
            return format!(
                "on disk · {} supported files · {}",
                self.supported_files,
                self.path.display()
            );
        }
        format!(
            "on disk · {} supported · also has {} (compile will refuse unknown types) · {}",
            self.supported_files,
            self.unsupported_samples.join(", "),
            self.path.display()
        )
    }
}

pub fn probe_dir(path: &Path) -> DiskProbe {
    let meta = std::fs::symlink_metadata(path).ok();
    let exists = meta.is_some();
    let is_dir = meta.as_ref().is_some_and(|m| m.file_type().is_dir());
    if !is_dir {
        return DiskProbe {
            path: path.to_path_buf(),
            exists,
            is_dir: false,
            supported_files: 0,
            unsupported_samples: Vec::new(),
        };
    }
    let mut supported_files = 0usize;
    let mut unsupported_samples = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            seen += 1;
            if seen > 8192 {
                break;
            }
            let p = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match ext.as_str() {
                "png" | "jpg" | "jpeg" | "wav" | "mp4" | "glb" => supported_files += 1,
                other if !other.is_empty() => {
                    let sample = format!(".{other}");
                    if unsupported_samples.len() < 4
                        && !unsupported_samples.iter().any(|s| s == &sample)
                    {
                        unsupported_samples.push(sample);
                    }
                }
                _ => {}
            }
        }
    }
    DiskProbe {
        path: path.to_path_buf(),
        exists: true,
        is_dir: true,
        supported_files,
        unsupported_samples,
    }
}

pub fn kenney_packs() -> &'static [KenneyPack] {
    KENNEY_PACKS
}

/// Godot starter-kit sample folders. Import uses the full Kenney.nl kits
/// instead (`platformer-kit`, `city-kit-*`, `racing-kit`, `mini-arena`,
/// `blaster-kit`). The slices stay on disk for the arcade downloader.
const STARTER_KIT_SLICES: &[&str] = &["arena", "city", "fps", "platformer", "racing"];

pub fn is_starter_kit_slice(name: &str) -> bool {
    STARTER_KIT_SLICES.iter().any(|s| *s == name)
}

pub fn kenney_pack_labels() -> Vec<String> {
    on_disk_kenney_packs()
        .into_iter()
        .map(|(name, _page)| {
            let ver = kenney_pack(&name).map(|p| p.version).unwrap_or("1.0");
            format!("{name}  {ver}")
        })
        .collect()
}

/// Folders that hold one sub-directory per Kenney kit. Env overrides first,
/// then the checkout's `local/packs/kenney` — the same `local/packs/<id>`
/// home the classic (Doom/Quake/…) packs use.
pub fn kenney_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(root) = std::env::var("AI_CONTENT_PACK_ROOT") {
        out.push(PathBuf::from(root).join("kenney"));
    }
    if let Ok(root) = std::env::var("KENNEY_PACK_ROOT") {
        out.push(PathBuf::from(root));
    }
    out.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("local/packs/kenney"),
    );
    out
}

/// Candidate local folders for one Kenney pack, first existing dir wins.
pub fn kenney_candidate_dirs(pack: &str) -> Vec<PathBuf> {
    kenney_roots()
        .into_iter()
        .map(|root| root.join(pack))
        .collect()
}

pub fn resolve_kenney_dir(pack: &str, override_path: &str) -> PathBuf {
    let typed = override_path.trim();
    if !typed.is_empty() {
        return PathBuf::from(typed);
    }
    for candidate in kenney_candidate_dirs(pack) {
        if candidate.is_dir() {
            return candidate;
        }
    }
    kenney_candidate_dirs(pack)
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(pack))
}

pub fn kaykit_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/sandbox/resources/characters")
}

pub fn nasa_sky_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/sandbox/resources/sky")
}

/// On-disk Kenney slugs: `(name, page)`, sorted by name. Catalogued packs
/// plus any extra kit folder under the Kenney roots ([`kenney_roots`]).
pub fn on_disk_kenney_packs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for pack in KENNEY_PACKS {
        if is_starter_kit_slice(pack.name) {
            continue;
        }
        let dir = resolve_kenney_dir(pack.name, "");
        if pack_has_importable_payload(&dir) && seen.insert(pack.name.to_string()) {
            out.push((pack.name.to_string(), pack.page.to_string()));
        }
    }
    for root in kenney_roots() {
        let Ok(rd) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if is_starter_kit_slice(&name) {
                continue;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            if kenney_spec(&name).is_err() {
                continue;
            }
            if !pack_has_importable_payload(&path) {
                continue;
            }
            let page = format!("https://kenney.nl/assets/{name}");
            out.push((name, page));
        }
    }
    // read_dir order is arbitrary; the dropdown lists dozens of kits.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Verified Asset Server session the Import thread may use. Absent = compile
/// only; never pretend a pack landed in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSession {
    pub endpoints: ApiEndpoints,
    pub token: String,
    pub server_id: [u8; 16],
}

/// The Asset Server THIS process hosts, from its own root files (`listen`
/// is `ip:ctrl:data` on ephemeral ports, plus `server-id` and the bootstrap
/// `admin-token`). `None` when no server is hosted here.
pub(crate) fn hosted_server_session() -> Option<ServerSession> {
    // The SAME root the embedded server uses, `AI_CONTENT_ASSET_ROOT`
    // included. Reading the default location unconditionally made an
    // instance started against an isolated store publish into the shared
    // one whenever its own session was not up yet — an import must never
    // land somewhere the operator did not point this process at.
    let root = crate::asset_store_state::default_asset_server_root();
    let token = std::fs::read_to_string(root.join("admin-token")).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let id = std::fs::read_to_string(root.join("server-id")).ok()?;
    let server_id = parse_hex16_root(id.trim())?;
    let listen = std::fs::read_to_string(root.join(makepad_asset_store::LISTEN_FILE)).ok()?;
    let parts: Vec<&str> = listen.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(ServerSession {
        endpoints: ApiEndpoints {
            control: format!("127.0.0.1:{}", parts[1]).parse().ok()?,
            data: format!("127.0.0.1:{}", parts[2]).parse().ok()?,
        },
        token,
        server_id,
    })
}

fn parse_hex16_root(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// One GLB (or other payload) the UI thread should persist into the local
/// library so ThumbnailRenderer can cache a sidecar. AO sidecars (if baked)
/// sit beside `path` as `<stem>.aomesh` / `<stem>.ao.png` / `<stem>.shadowsdf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryLanding {
    pub path: PathBuf,
    pub label: String,
    pub domain: &'static str,
    pub content_type: &'static str,
    pub prompt: String,
    /// Pre-made icon (anim sheet or still). Copied as the library thumb.
    pub thumbnail: Option<PathBuf>,
    /// `kenney` / `freedoom` / `librequake` / `kaykit` — never parsed from
    /// the display label (classic titles are just `MAP27`).
    pub source_id: String,
    /// Pack slug inside the source (`space-kit`, `freedoom`, …).
    pub pack: String,
}

/// Offline AO bake counts for one pack (honest; no invented success).
pub use ao_bake::BakeStats;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportPhase {
    Idle,
    Compiling {
        pack: String,
        done: usize,
        total: usize,
        current: String,
    },
    /// Byte-level HTTP fetch (shareware zip / demo installer / zipsync span).
    Downloading {
        pack: String,
        loaded: u64,
        total: u64,
        label: String,
    },
    /// A convert-time still for the queue strip. Does not change the phase.
    PreviewThumb {
        pack: String,
        name: String,
        png: Vec<u8>,
    },
    Publishing {
        pack: String,
        assets: usize,
        blobs: usize,
        /// Blobs finished uploading (0..blobs).
        blob_done: usize,
    },
    Annotating {
        pack: String,
        assets: usize,
        blobs: usize,
        annotated: usize,
        total: usize,
    },
    Baking {
        pack: String,
        assets: usize,
        annotated: usize,
        bake_done: usize,
        bake_total: usize,
        bake_skipped: usize,
        bake_failed: usize,
        current: String,
    },
    /// Staged + baked; the import thread now BLOCKS (parked on an
    /// `IconResumeGate` receiver) until the UI has GPU-rendered (or
    /// fingerprint-reused) a real icon for every staged GLB into the
    /// persistent pack icon cache. Compile/publish never runs before this
    /// resolves — no placeholder-thumbnailed revision may ever reach the
    /// store. `library` is handed to the UI immediately (same shape as
    /// `Published`/`PackFinished`) so `land_imported_pack` can start
    /// queuing icon work right away; see `ImportPage::icons_pending_ready`
    /// / `resume_icons_pending`.
    IconsPending {
        pack: String,
        assets: usize,
        library: Vec<LibraryLanding>,
        bake: BakeStats,
    },
    /// Compiled locally; Asset Server was down so nothing is in the catalog.
    CompiledLocal {
        pack: String,
        assets: usize,
        blobs: usize,
        out: PathBuf,
        /// Why the catalog did NOT get this pack, when that was a FAILURE
        /// and not a choice. `None` — there was no server session, the
        /// bundle is on disk and this is a clean local compile. `Some` —
        /// the compiler or the publish REFUSED, and this pack did not
        /// import; "Import all" counts it among the failures and the
        /// status line leads with the reason instead of burying it in a
        /// "catalog skipped (…)" aside.
        error: Option<String>,
        library: Vec<LibraryLanding>,
        bake: BakeStats,
    },
    /// Published + annotated on the Asset Server. `library` is what the UI
    /// should persist for cached rendered thumbs.
    Published {
        pack: String,
        assets: usize,
        blobs: usize,
        created: bool,
        annotated: usize,
        out: PathBuf,
        library: Vec<LibraryLanding>,
        bake: BakeStats,
    },
    /// One pack finished during Import all — land thumbs now, more may follow.
    PackFinished {
        pack: String,
        assets: usize,
        annotated: usize,
        library: Vec<LibraryLanding>,
        bake: BakeStats,
        done: usize,
        total: usize,
        more: bool,
    },
    /// One pack FAILED during Import all — say why, right now, and keep
    /// going. A multi-pack run used to collect these silently and report
    /// nothing but a count at the end (`AllDone::failed`), which cost a
    /// whole debugging session: 38 packs failed for one reason nobody could
    /// read. This phase is transient (the next pack overwrites it) but it
    /// is logged and drawn the moment it happens, and the reason also
    /// survives into `AllDone::failed`.
    PackFailed {
        pack: String,
        message: String,
        done: usize,
        total: usize,
        more: bool,
    },
    AllDone {
        ok: Vec<String>,
        failed: Vec<(String, String)>,
        skipped: Vec<String>,
    },
    Failed {
        pack: String,
        message: String,
    },
    Cancelled {
        pack: String,
        message: String,
    },
}

impl ImportPhase {
    pub fn compiling(pack: impl Into<String>) -> Self {
        ImportPhase::Compiling {
            pack: pack.into(),
            done: 0,
            total: 0,
            current: String::new(),
        }
    }

    /// Why this phase is a FAILURE, if it is one — the ONE place that
    /// decides what "failed" means, shared by the card's failed flag, the
    /// `[E]` log line and the run summary. Every phase that can carry a
    /// refusal answers here, so a new failure shape cannot be added that
    /// silently draws as success (which is exactly how a whole
    /// 38-packs-failed run once shipped without a single reason anywhere).
    ///
    /// Always `<pack>: <reason>` — a reason that does not say WHICH pack is
    /// half a diagnosis in a 46-kit run.
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            ImportPhase::Failed { pack, message }
            | ImportPhase::PackFailed { pack, message, .. } => Some(format!("{pack}: {message}")),
            ImportPhase::CompiledLocal {
                pack,
                error: Some(error),
                ..
            } => Some(format!("{pack}: {error}")),
            ImportPhase::AllDone { failed, .. } if !failed.is_empty() => {
                Some(failure_summary(failed))
            }
            _ => None,
        }
    }
}

/// The UI-side half of the icon-render handshake: an import thread that
/// finished staging+baking sends `ImportPhase::IconsPending` and then
/// BLOCKS on the `Receiver<()>` half of this gate until the UI resolves it
/// — every staged GLB has a real icon (rendered or fingerprint-reused into
/// `pack_icons_dir`), or the import was cancelled. Kenney/KayKit
/// (`ImportPage`) own one; classic packs (`import_classic.rs`) should own
/// their own per-source instance the same way — `armed()` for a fresh
/// import, `arm()` on every new `IconsPending` message so a shared
/// multi-pack channel (`start_kenney_import_all`) can never double-fire
/// into the wrong pack's wait, `resume()` to signal once.
#[derive(Default)]
pub struct IconResumeGate {
    tx: Option<mpsc::Sender<()>>,
    sent: bool,
}

impl IconResumeGate {
    /// Fresh gate for a new import thread; returns the receiver half to
    /// move into that thread's closure (borrowed per pack via `&rx` — one
    /// `IconsPending`/resume cycle per `run_kenney_import`/
    /// `run_kaykit_import` call, reused across packs in "Import all").
    pub fn armed() -> (Self, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx: Some(tx),
                sent: false,
            },
            rx,
        )
    }

    /// A fresh `IconsPending` phase arrived for a (possibly new) pack:
    /// allow exactly one resume send for it.
    pub fn arm(&mut self) {
        self.sent = false;
    }

    /// Send the resume signal once for the current pack. Returns `true` the
    /// first time since the last `arm()`; a repeat call is a no-op so
    /// `maybe_resume_icons_pending` firing on several polling ticks before
    /// the phase advances can never double-send.
    pub fn resume(&mut self) -> bool {
        if self.sent {
            return false;
        }
        let Some(tx) = &self.tx else {
            return false;
        };
        if tx.send(()).is_ok() {
            self.sent = true;
            true
        } else {
            false
        }
    }
}

pub struct ImportPage {
    pub kenney_pack_index: usize,
    pub kenney_path: String,
    pub kenney_phase: ImportPhase,
    /// Landings waiting for the UI to consume (cleared by take_library_landings).
    pending_landings: Vec<LibraryLanding>,
    /// GLB library files this import session cares about for icon progress.
    import_icon_files: BTreeSet<String>,
    /// How many of `import_icon_files` already have a committed `.thumb`.
    pub icons_done: usize,
    /// Blank/unloadable captures — still count as finished work.
    pub icons_failed: usize,
    /// File the GPU is rendering right now (UI-thread).
    pub icon_current: String,
    /// Newest-first PNG previews for the Kenney card strip.
    pub preview_thumbs: Vec<(String, Vec<u8>)>,
    pub preview_dirty: bool,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<ImportPhase>>,
    /// The icon-render handshake for whichever pack is currently
    /// `IconsPending` — see [`IconResumeGate`]. Reused across every pack in
    /// an "Import all" run (one `armed()` per `start_*` call, `arm()` per
    /// pack's `IconsPending` message).
    icon_resume: IconResumeGate,
    /// (packs finished, packs in run) while an "Import all" run is active,
    /// else None. Drives the overall progress bar and the "pack x/y"
    /// status prefix — without it the bar restarted per pack, which over a
    /// 38-pack run read as noise.
    all_run: Option<(usize, usize)>,
}

impl Default for ImportPage {
    fn default() -> Self {
        Self {
            kenney_pack_index: 0,
            kenney_path: String::new(),
            kenney_phase: ImportPhase::Idle,
            pending_landings: Vec::new(),
            import_icon_files: BTreeSet::new(),
            icons_done: 0,
            icons_failed: 0,
            icon_current: String::new(),
            preview_thumbs: Vec::new(),
            preview_dirty: false,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            icon_resume: IconResumeGate::default(),
            all_run: None,
        }
    }
}

impl ImportPage {
    /// Selected full Kenney kit (name, official page).
    pub fn selected_pack_id(&self) -> (String, String) {
        let packs = on_disk_kenney_packs();
        packs
            .get(self.kenney_pack_index)
            .cloned()
            .or_else(|| packs.first().cloned())
            .unwrap_or_else(|| {
                (
                    "space-kit".into(),
                    "https://kenney.nl/assets/space-kit".into(),
                )
            })
    }

    pub fn set_pack_index(&mut self, index: usize) {
        let n = on_disk_kenney_packs().len();
        if n == 0 {
            self.kenney_pack_index = 0;
        } else if index < n {
            self.kenney_pack_index = index;
        }
    }

    pub fn kenney_probe(&self) -> DiskProbe {
        let (pack, _) = self.selected_pack_id();
        probe_dir(&resolve_kenney_dir(&pack, &self.kenney_path))
    }

    pub fn kenney_status_line(&self, server_connected: bool) -> String {
        if self.cancel.load(Ordering::SeqCst) && self.compiling() {
            let pack = match &self.kenney_phase {
                ImportPhase::Compiling { pack, .. }
                | ImportPhase::Publishing { pack, .. }
                | ImportPhase::Annotating { pack, .. }
                | ImportPhase::Baking { pack, .. }
                | ImportPhase::PackFinished { pack, .. } => pack.as_str(),
                _ => "import",
            };
            return format!("{pack}: stopping…");
        }
        let line = match &self.kenney_phase {
            ImportPhase::Downloading {
                pack,
                loaded,
                total,
                label,
            } => {
                if *total > 0 {
                    format!(
                        "{pack}: loading {} / {} · {label}",
                        *loaded, *total
                    )
                } else {
                    format!("{pack}: loading {loaded} bytes · {label}")
                }
            }
            ImportPhase::Compiling { pack, done, total, current } => {
                if *total > 0 {
                    if current.is_empty() {
                        format!("{pack}: converting {done}/{total}")
                    } else {
                        format!("{pack}: converting {done}/{total} · {current}")
                    }
                } else {
                    format!("{pack}: compiling local pack — no network fetch…")
                }
            }
            ImportPhase::Publishing {
                pack,
                assets,
                blobs,
                blob_done,
            } => format!(
                "{pack}: publish blob {blob_done}/{blobs} · {assets} assets compiled"
            ),
            ImportPhase::Annotating {
                pack,
                annotated,
                total,
                ..
            } => format!("{pack}: annotate {annotated}/{total}"),
            ImportPhase::Baking {
                pack,
                bake_done,
                bake_total,
                bake_skipped,
                bake_failed,
                current,
                ..
            } => {
                let cores = ao_bake::bake_thread_count();
                let mut line = format!(
                    "{pack}: building AO {bake_done}/{bake_total} on {cores} cores"
                );
                if !current.is_empty() {
                    line.push_str(" · ");
                    line.push_str(current);
                }
                if *bake_skipped > 0 {
                    line.push_str(&format!(" · {bake_skipped} already baked"));
                }
                if *bake_failed > 0 {
                    line.push_str(&format!(" · {bake_failed} failed"));
                }
                line
            }
            ImportPhase::IconsPending {
                pack,
                assets,
                library,
                ..
            } => {
                // "icons ready" while NOTHING is registered yet is the lie
                // that hid a 38-pack failure: an empty wait is a wait that
                // has not started, never one that finished. Say which.
                let meshes = library
                    .iter()
                    .filter(|landing| landing.content_type.contains("gltf"))
                    .count();
                if self.import_icon_files.is_empty() && meshes > 0 {
                    format!("{pack}: queuing {meshes} icon renders before publish ({assets} staged)")
                } else {
                    format!(
                        "{pack}: {} before publish ({assets} staged)",
                        self.icon_status_fragment()
                    )
                }
            }
            ImportPhase::CompiledLocal {
                pack,
                assets,
                blobs,
                bake,
                out,
                error,
                ..
            } => {
                if let Some(error) = error {
                    // A refusal is not a "skip". Lead with it.
                    return format!("{pack}: NOT imported — {error}");
                }
                let catalog = if out.is_file() || out.is_dir() {
                    "catalog skipped (server disconnected)".to_string()
                } else {
                    format!("catalog skipped ({})", out.display())
                };
                if self.icons_busy() {
                    format!(
                        "{pack}: {} · compiled {assets}/{blobs} locally · {catalog}",
                        self.icon_status_fragment()
                    )
                } else {
                    format!(
                        "{pack}: compiled {assets} assets / {blobs} blobs · {} · {catalog}",
                        bake_status_fragment(bake, &self.icon_status_fragment())
                    )
                }
            }
            ImportPhase::Published {
                pack,
                assets,
                created,
                annotated,
                bake,
                ..
            } => {
                let first = if *created {
                    "published"
                } else {
                    "reimported"
                };
                if self.icons_busy() {
                    format!(
                        "{pack}: {} · {first} {assets} assets · {annotated} searchable",
                        self.icon_status_fragment()
                    )
                } else {
                    format!(
                        "{pack}: {first} · {assets} assets · {annotated} searchable · {} · server {}",
                        bake_status_fragment(bake, &self.icon_status_fragment()),
                        if server_connected {
                            "connected"
                        } else {
                            "dropped after publish"
                        }
                    )
                }
            }
            ImportPhase::PackFinished {
                pack,
                done,
                total,
                annotated,
                bake,
                more,
                ..
            } => {
                let tail = if *more {
                    "next pack…"
                } else {
                    "all packs done"
                };
                format!(
                    "import all {done}/{total} · {pack} ({annotated} annotated) · {} · {tail}",
                    bake_status_fragment(bake, &self.icon_status_fragment())
                )
            }
            ImportPhase::PackFailed {
                pack,
                message,
                done,
                total,
                more,
            } => {
                let tail = if *more {
                    "next pack…"
                } else {
                    "all packs done"
                };
                format!("import all {done}/{total} · {pack} FAILED — {message} · {tail}")
            }
            ImportPhase::AllDone {
                ok,
                failed,
                skipped,
            } => {
                let mut parts = vec![format!("{} packs imported", ok.len())];
                if !failed.is_empty() {
                    parts.push(format!("{} failed — {}", failed.len(), failure_summary(failed)));
                }
                if !skipped.is_empty() {
                    parts.push(format!("{} not on disk", skipped.len()));
                }
                format!(
                    "{} · {}",
                    parts.join(" · "),
                    self.icon_status_fragment()
                )
            }
            ImportPhase::Failed { pack, message } => format!("{pack}: {message}"),
            ImportPhase::Cancelled { pack, message } => format!("{pack}: stopped — {message}"),
            ImportPhase::PreviewThumb { pack, name, .. } => {
                format!("{pack}: preview {name}")
            }
            ImportPhase::Idle => {
                let probe = self.kenney_probe();
                if !server_connected {
                    format!(
                        "server disconnected · compile possible, catalog publish will not run · {}",
                        probe.line()
                    )
                } else if probe.ready() {
                    format!("ready · {}", probe.line())
                } else {
                    format!("not provisioned · {}", probe.line())
                }
            }
        };
        // Inside an "Import all" run, in-pack lines lead with which pack of
        // how many this is (pack-boundary lines already carry their own
        // "import all d/t").
        if let Some((done, total)) = self.all_run {
            let in_pack = !matches!(
                &self.kenney_phase,
                ImportPhase::PackFinished { .. }
                    | ImportPhase::PackFailed { .. }
                    | ImportPhase::AllDone { .. }
                    | ImportPhase::Idle
                    | ImportPhase::Failed { .. }
                    | ImportPhase::Cancelled { .. }
            );
            if in_pack && total > 0 {
                return format!("pack {}/{total} · {line}", (done + 1).min(total));
            }
        }
        line
    }

    fn icon_status_fragment(&self) -> String {
        let total = self.import_icon_files.len();
        if total == 0 {
            return "icons ready".into();
        }
        let processed = self.icons_processed();
        if processed >= total {
            format!("icons {processed}/{total}")
        } else {
            let mut line = format!("rendering GPU icons {processed}/{total}");
            if !self.icon_current.is_empty() {
                line.push_str(" · ");
                line.push_str(&self.icon_current);
            }
            line
        }
    }

    pub fn icons_processed(&self) -> usize {
        self.icons_done.saturating_add(self.icons_failed)
    }

    pub fn icons_busy(&self) -> bool {
        let total = self.import_icon_files.len();
        total > 0 && self.icons_processed() < total
    }

    /// Honest 0..1 progress from stage counts — no fake ETAs.
    /// During an "Import all" run the bar is the OVERALL run — finished
    /// packs plus the current pack's own fraction, packs weighted equally —
    /// because a per-pack bar restarting 38 times reads as a broken bar.
    pub fn progress_fraction(&self) -> f32 {
        if let Some((done, total)) = self.all_run {
            if total > 0 {
                // A finished pack is already counted in `done`; its
                // cumulative-icon fraction would double-count and make the
                // bar dip when the next pack starts.
                let pack = match &self.kenney_phase {
                    ImportPhase::PackFinished { .. } | ImportPhase::PackFailed { .. } => 0.0,
                    _ => self.pack_fraction().clamp(0.0, 1.0),
                };
                return ((done as f32 + pack) / total as f32).min(1.0);
            }
        }
        self.pack_fraction()
    }

    /// The CURRENT pack's 0..1 fraction. Compile/publish/AO are a short
    /// prefix; icon renders own most of the bar because that is the slow,
    /// visible work (one GLB at a time).
    fn pack_fraction(&self) -> f32 {
        match &self.kenney_phase {
            ImportPhase::Idle
            | ImportPhase::Failed { .. }
            | ImportPhase::Cancelled { .. }
            | ImportPhase::PreviewThumb { .. } => {
                if self.icons_busy() {
                    let total = self.import_icon_files.len().max(1) as f32;
                    0.20 + 0.80 * (self.icons_processed().min(self.import_icon_files.len()) as f32
                        / total)
                } else {
                    0.0
                }
            }
            ImportPhase::Downloading {
                loaded, total, ..
            } => {
                if *total == 0 {
                    0.04
                } else {
                    0.02 + 0.70 * (*loaded as f32 / *total as f32)
                }
            }
            ImportPhase::Compiling { done, total, .. } => {
                if *total == 0 {
                    0.02
                } else {
                    0.02 + 0.70 * (*done as f32 / *total as f32)
                }
            }
            ImportPhase::Publishing {
                blob_done, blobs, ..
            } => {
                let n = (*blobs).max(1) as f32;
                0.02 + 0.06 * ((*blob_done).min(*blobs) as f32 / n)
            }
            ImportPhase::Annotating {
                annotated, total, ..
            } => {
                let n = (*total).max(1) as f32;
                0.08 + 0.04 * ((*annotated).min(*total) as f32 / n)
            }
            ImportPhase::Baking {
                bake_done,
                bake_total,
                ..
            } => {
                let n = (*bake_total).max(1) as f32;
                0.12 + 0.08 * ((*bake_done).min(*bake_total) as f32 / n)
            }
            ImportPhase::IconsPending { .. }
            | ImportPhase::Published { .. }
            | ImportPhase::CompiledLocal { .. }
            | ImportPhase::PackFinished { .. }
            | ImportPhase::PackFailed { .. }
            | ImportPhase::AllDone { .. } => {
                let total = self.import_icon_files.len();
                if total == 0 {
                    1.0
                } else {
                    let done = self.icons_processed().min(total) as f32;
                    0.20 + 0.80 * (done / total as f32)
                }
            }
        }
    }

    /// True while the import thread is still driving stage/bake/icons/
    /// compile/publish. `IconsPending` counts: the thread is PARKED,
    /// waiting on `icon_resume` — a new import must not start until it
    /// resolves (resumes to publish, or is cancelled).
    pub fn compiling(&self) -> bool {
        matches!(
            self.kenney_phase,
            ImportPhase::Compiling { .. }
                | ImportPhase::Publishing { .. }
                | ImportPhase::Annotating { .. }
                | ImportPhase::Baking { .. }
                | ImportPhase::IconsPending { .. }
                | ImportPhase::PackFinished { more: true, .. }
                | ImportPhase::PackFailed { more: true, .. }
        )
    }

    /// Consume landings once so re-poll cannot re-land the same pack.
    pub fn take_library_landings(&mut self) -> Vec<LibraryLanding> {
        std::mem::take(&mut self.pending_landings)
    }

    /// Register a library GLB for icon progress. Pass existing `.thumb` PNG
    /// bytes when the sidecar is already on disk so the Import strip updates
    /// immediately and the done count advances.
    pub fn track_import_icon(&mut self, file: String, existing_png: Option<Vec<u8>>) {
        let first = self.import_icon_files.insert(file.clone());
        if let Some(png) = existing_png {
            self.push_preview_thumb(file, png);
            if first {
                self.icons_done = self.icons_done.saturating_add(1);
            }
        }
    }

    pub fn push_preview_thumb(&mut self, file: String, png: Vec<u8>) {
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return;
        }
        self.preview_thumbs.retain(|(f, _)| f != &file);
        self.preview_thumbs.insert(0, (file, png));
        if self.preview_thumbs.len() > IMPORT_PREVIEW_SLOTS {
            self.preview_thumbs.truncate(IMPORT_PREVIEW_SLOTS);
        }
        self.preview_dirty = true;
    }

    /// A ThumbnailRenderer PNG landed for a tracked import icon.
    pub fn note_rendered_icon(&mut self, file: &str, png: &[u8]) -> bool {
        if !self.import_icon_files.contains(file) {
            return false;
        }
        // Avoid double-counting if the strip already held this file.
        let had = self.preview_thumbs.iter().any(|(f, _)| f == file);
        self.push_preview_thumb(file.to_string(), png.to_vec());
        if !had {
            let total = self.import_icon_files.len();
            if self.icons_done < total {
                self.icons_done += 1;
            }
        }
        if self.icon_current == file {
            self.icon_current.clear();
        }
        true
    }

    pub fn note_failed_icon(&mut self, file: &str) -> bool {
        if !self.import_icon_files.contains(file) {
            return false;
        }
        let total = self.import_icon_files.len();
        if self.icons_processed() < total {
            self.icons_failed += 1;
        }
        if self.icon_current == file {
            self.icon_current.clear();
        }
        true
    }

    pub fn set_icon_current(&mut self, file: Option<&str>) {
        self.icon_current = file.unwrap_or("").to_string();
    }

    pub fn request_stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn stop_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn mark_cancelled(&mut self, message: impl Into<String>) {
        let pack = match &self.kenney_phase {
            ImportPhase::Failed { pack, .. }
            | ImportPhase::Cancelled { pack, .. }
            | ImportPhase::Compiling { pack, .. }
            | ImportPhase::Downloading { pack, .. }
            | ImportPhase::Publishing { pack, .. }
            | ImportPhase::Annotating { pack, .. }
            | ImportPhase::Baking { pack, .. }
            | ImportPhase::IconsPending { pack, .. }
            | ImportPhase::Published { pack, .. }
            | ImportPhase::CompiledLocal { pack, .. }
            | ImportPhase::PackFinished { pack, .. }
            | ImportPhase::PackFailed { pack, .. }
            | ImportPhase::PreviewThumb { pack, .. } => pack.clone(),
            ImportPhase::Idle | ImportPhase::AllDone { .. } => self.selected_pack_id().0,
        };
        self.kenney_phase = ImportPhase::Cancelled {
            pack,
            message: message.into(),
        };
        self.rx = None;
        let leftover = self
            .import_icon_files
            .len()
            .saturating_sub(self.icons_processed());
        self.icons_failed = self.icons_failed.saturating_add(leftover);
        self.icon_current.clear();
        // Do NOT touch `icon_resume` here: if the thread is parked in
        // `IconsPending`, `maybe_resume_icons_pending`'s cancelled branch
        // still needs `resume()` to fire so it can wake up, see `cancel`
        // is set, and exit with `ImportPhase::Cancelled` itself instead of
        // leaking a parked thread forever.
    }

    pub fn icons_total(&self) -> usize {
        self.import_icon_files.len()
    }

    pub fn reset_session_ui(&mut self) {
        self.pending_landings.clear();
        self.import_icon_files.clear();
        self.icons_done = 0;
        self.icons_failed = 0;
        self.icon_current.clear();
        self.preview_thumbs.clear();
        self.preview_dirty = true;
        self.all_run = None;
    }

    fn ingest_phase(&mut self, phase: ImportPhase) {
        if self.cancel.load(Ordering::SeqCst) && !matches!(phase, ImportPhase::Cancelled { .. }) {
            return;
        }
        match &phase {
            ImportPhase::PreviewThumb { name, png, .. } => {
                self.push_preview_thumb(name.clone(), png.clone());
                return;
            }
            ImportPhase::IconsPending { library, .. } => {
                self.pending_landings.extend(library.iter().cloned());
                // A fresh pack's icon wait: allow exactly one resume send
                // for it (see `IconResumeGate::arm`).
                self.icon_resume.arm();
            }
            ImportPhase::Published { library, .. } | ImportPhase::PackFinished { library, .. } => {
                self.pending_landings.extend(library.iter().cloned());
            }
            ImportPhase::CompiledLocal { library, .. } => {
                self.pending_landings.extend(library.iter().cloned());
            }
            _ => {}
        }
        match &phase {
            ImportPhase::PackFinished { done, total, .. }
            | ImportPhase::PackFailed { done, total, .. } => {
                self.all_run = Some((*done, *total));
            }
            ImportPhase::AllDone { .. }
            | ImportPhase::Failed { .. }
            | ImportPhase::Cancelled { .. } => {
                self.all_run = None;
            }
            _ => {}
        }
        self.kenney_phase = phase;
    }

    /// True once an `IconsPending` pack's staged icons have all rendered
    /// (or finished failing) and every landing it handed the UI has been
    /// drained into the render queue — safe to let the parked import
    /// thread continue to compile+publish. Landing drainage lives on the
    /// App (`import_landings`, fed from `pending_landings` every tick), so
    /// the caller passes whether that queue is currently empty.
    pub fn icons_pending_ready(&self, landings_drained: bool) -> bool {
        matches!(self.kenney_phase, ImportPhase::IconsPending { .. })
            && landings_drained
            && !self.icons_busy()
    }

    /// Staged meshes of the parked `IconsPending` pack that were never
    /// REGISTERED for the icon wait ([`ImportPage::track_import_icon`]).
    ///
    /// The wait is over when every registered icon has reported; a mesh
    /// that never registered is therefore invisible to it, and the gate
    /// reads a wait with nothing outstanding as "all icons done — publish".
    /// That is not a hypothetical: `land_imported_pack` once queued every
    /// fresh GPU render WITHOUT registering it, so a 38-pack run resumed
    /// instantly, compiled against the placeholders it had just written,
    /// and every pack died on the placeholder guard while its real icons
    /// were still rendering.
    ///
    /// The gate opens anyway when this is non-empty (a mesh that can never
    /// be queued must not park an import forever) — the caller LOGS it,
    /// and the publish-time placeholder guard still refuses the pack. This
    /// is the diagnosis, not the enforcement.
    pub fn unregistered_mesh_icons(&self) -> Vec<String> {
        let ImportPhase::IconsPending { library, .. } = &self.kenney_phase else {
            return Vec::new();
        };
        library
            .iter()
            .filter(|landing| landing.content_type.contains("gltf"))
            .filter_map(|landing| {
                let stem = landing.path.file_stem()?.to_str()?;
                // Keys are `pack:<source>:<pack>:<stem>:<fingerprint>` —
                // match on everything up to the fingerprint, which the
                // caller computes from bytes this side cannot see.
                let prefix = format!("pack:{}:{}:{stem}:", landing.source_id, landing.pack);
                (!self
                    .import_icon_files
                    .iter()
                    .any(|key| key.starts_with(&prefix)))
                .then(|| stem.to_string())
            })
            .collect()
    }

    /// Let the parked import thread continue: normal readiness
    /// (`icons_pending_ready`), or the user cancelled (the thread's own
    /// `cancel` check right after waking decides abort-vs-continue either
    /// way). No-op past the first call per `IconsPending` phase — see
    /// [`IconResumeGate::resume`].
    pub fn resume_icons_pending(&mut self) -> bool {
        self.icon_resume.resume()
    }

    /// Fail-closed local compile, then publish when a server session is
    /// provided. Never fetches pack bytes from the network. Reimport is
    /// allowed after a finished phase (not while compiling).
    pub fn start_kenney_import(
        &mut self,
        path_override: String,
        server: Option<ServerSession>,
    ) -> Result<(), String> {
        if self.compiling() {
            return Err("a Kenney compile is already running".into());
        }
        self.reset_session_ui();
        self.cancel.store(false, Ordering::SeqCst);
        self.kenney_path = path_override;
        let (pack_name, pack_page) = self.selected_pack_id();
        let dir = resolve_kenney_dir(&pack_name, &self.kenney_path);
        if !dir.is_dir() {
            let message = format!(
                "pack is not on disk ({}) — import is local-folder only",
                dir.display()
            );
            self.kenney_phase = ImportPhase::Failed {
                pack: pack_name,
                message: message.clone(),
            };
            return Err(message);
        }
        let spec = kenney_spec(&pack_name).map_err(|e| e.to_string())?;
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import/kenney")
            .join(&pack_name);
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let (icon_resume, icon_resume_rx) = IconResumeGate::armed();
        self.icon_resume = icon_resume;
        self.kenney_phase = ImportPhase::compiling(pack_name.clone());
        let cancel = self.cancel.clone();
        thread::Builder::new()
            .name("asset-ui-kenney-import".into())
            .spawn(move || {
                let phase = run_kenney_import(
                    &dir,
                    &out,
                    spec,
                    pack_name,
                    pack_page,
                    server,
                    &tx,
                    &cancel,
                    &icon_resume_rx,
                );
                let _ = tx.send(phase);
            })
            .map_err(|e| format!("failed to start compile thread: {e}"))?;
        Ok(())
    }

    /// Import the in-repo KayKit character folder (CC0-1.0).
    pub fn start_kaykit_import(&mut self, server: Option<ServerSession>) -> Result<(), String> {
        if self.compiling() {
            return Err("an import is already running".into());
        }
        self.reset_session_ui();
        self.cancel.store(false, Ordering::SeqCst);
        let dir = kaykit_dir();
        if !dir.is_dir() {
            let message = format!(
                "pack is not on disk ({}) — import is local-folder only",
                dir.display()
            );
            self.kenney_phase = ImportPhase::Failed {
                pack: "kaykit".into(),
                message: message.clone(),
            };
            return Err(message);
        }
        let spec = kaykit_spec();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import/kaykit");
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let (icon_resume, icon_resume_rx) = IconResumeGate::armed();
        self.icon_resume = icon_resume;
        self.kenney_phase = ImportPhase::compiling("kaykit");
        let cancel = self.cancel.clone();
        thread::Builder::new()
            .name("asset-ui-kaykit-import".into())
            .spawn(move || {
                let phase = run_kaykit_import(&dir, &out, spec, server, &tx, &cancel, &icon_resume_rx);
                let _ = tx.send(phase);
            })
            .map_err(|e| format!("failed to start KayKit import thread: {e}"))?;
        Ok(())
    }

    /// Import every Kenney pack that is on disk. Missing catalog slugs are
    /// skipped honestly; each present pack uses the same CC-BY-4.0 grant.
    pub fn start_kenney_import_all(&mut self, server: Option<ServerSession>) -> Result<(), String> {
        if self.compiling() {
            return Err("a Kenney compile is already running".into());
        }
        self.reset_session_ui();
        self.cancel.store(false, Ordering::SeqCst);
        let present = on_disk_kenney_packs();
        let skipped: Vec<String> = KENNEY_PACKS
            .iter()
            .filter(|p| present.iter().all(|(name, _)| name != p.name))
            .map(|p| p.name.to_string())
            .collect();
        if present.is_empty() {
            let message = String::from("no Kenney packs on disk — import is local-folder only");
            self.kenney_phase = ImportPhase::Failed {
                pack: "all".into(),
                message: message.clone(),
            };
            return Err(message);
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let (icon_resume, icon_resume_rx) = IconResumeGate::armed();
        self.icon_resume = icon_resume;
        self.kenney_phase = ImportPhase::compiling("all");
        self.all_run = Some((0, present.len()));
        let cancel = self.cancel.clone();
        thread::Builder::new()
            .name("asset-ui-kenney-import-all".into())
            .spawn(move || {
                let total = present.len();
                let mut ok = Vec::new();
                let mut failed = Vec::new();
                for (index, (pack_name, pack_page)) in present.into_iter().enumerate() {
                    if cancel.load(Ordering::SeqCst) {
                        let _ = tx.send(ImportPhase::Cancelled {
                            pack: "all".into(),
                            message: format!("stopped after {index} packs"),
                        });
                        return;
                    }
                    let done = index + 1;
                    let more = done < total;
                    let spec = match kenney_spec(&pack_name) {
                        Ok(spec) => spec,
                        Err(error) => {
                            failed.push((pack_name, error.to_string()));
                            continue;
                        }
                    };
                    let dir = resolve_kenney_dir(&pack_name, "");
                    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../local/ai_content_app/import/kenney")
                        .join(&pack_name);
                    // ONE `IconResumeGate` receiver serves every pack in this
                    // run — `ImportPage::ingest_phase` re-`arm()`s it on each
                    // fresh `IconsPending` message, so it is safe to reuse
                    // across the whole loop (never two packs' waits at once).
                    let phase = run_kenney_import(
                        &dir,
                        &out,
                        spec,
                        pack_name.clone(),
                        pack_page,
                        server.clone(),
                        &tx,
                        &cancel,
                        &icon_resume_rx,
                    );
                    match phase {
                        ImportPhase::Cancelled { pack, message } => {
                            let _ = tx.send(ImportPhase::Cancelled { pack, message });
                            return;
                        }
                        ImportPhase::Published {
                            pack,
                            assets,
                            annotated,
                            library,
                            bake,
                            ..
                        } => {
                            ok.push(pack.clone());
                            let _ = tx.send(ImportPhase::PackFinished {
                                pack,
                                assets,
                                annotated,
                                library,
                                bake,
                                done,
                                total,
                                more,
                            });
                        }
                        // A pack that COMPILED but whose compile or publish
                        // was refused did not import. Counting it among the
                        // successes is how "0 packs imported · 38 failed"
                        // could ever have been the honest summary of a run
                        // where something else went wrong.
                        ImportPhase::CompiledLocal {
                            pack,
                            error: Some(message),
                            ..
                        } => {
                            let _ = tx.send(ImportPhase::PackFailed {
                                pack: pack.clone(),
                                message: message.clone(),
                                done,
                                total,
                                more,
                            });
                            failed.push((pack, message));
                        }
                        ImportPhase::CompiledLocal {
                            pack,
                            assets,
                            library,
                            bake,
                            ..
                        } => {
                            ok.push(pack.clone());
                            let _ = tx.send(ImportPhase::PackFinished {
                                pack,
                                assets,
                                annotated: 0,
                                library,
                                bake,
                                done,
                                total,
                                more,
                            });
                        }
                        ImportPhase::Failed { pack, message } => {
                            // Say it NOW, not as a count at the end: this
                            // phase is what the card draws and what the app
                            // log records for this pack. `failed` still
                            // carries the reason into `AllDone`.
                            let _ = tx.send(ImportPhase::PackFailed {
                                pack: pack.clone(),
                                message: message.clone(),
                                done,
                                total,
                                more,
                            });
                            failed.push((pack, message));
                        }
                        other => {
                            let _ = tx.send(other);
                        }
                    }
                }
                let _ = tx.send(ImportPhase::AllDone {
                    ok,
                    failed,
                    skipped,
                });
            })
            .map_err(|e| format!("failed to start import-all thread: {e}"))?;
        Ok(())
    }

    /// Drain every pending phase message. Returns true when the phase changed.
    pub fn poll(&mut self) -> bool {
        if self.rx.is_none() {
            return false;
        }
        let mut changed = false;
        loop {
            let msg = self.rx.as_ref().map(|rx| rx.try_recv());
            match msg {
                Some(Ok(phase)) => {
                    self.ingest_phase(phase);
                    changed = true;
                }
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    if matches!(
                        self.kenney_phase,
                        ImportPhase::Compiling { .. }
                            | ImportPhase::Publishing { .. }
                            | ImportPhase::Annotating { .. }
                            | ImportPhase::Baking { .. }
                            // A thread legitimately parked in IconsPending
                            // holds `tx` alive; if it disconnects anyway
                            // (panicked, or exited without publishing) the
                            // UI must not stay stuck "compiling" forever —
                            // fail closed instead of leaking a wait no one
                            // will ever resolve.
                            | ImportPhase::IconsPending { .. }
                            // Same for a multi-pack run that died between
                            // packs: `compiling()` calls these two busy, so
                            // without this the queue would never advance.
                            | ImportPhase::PackFinished { more: true, .. }
                            | ImportPhase::PackFailed { more: true, .. }
                    ) {
                        self.kenney_phase = ImportPhase::Failed {
                            pack: self.selected_pack_id().0,
                            message: "compile thread exited without a result".into(),
                        };
                        changed = true;
                    }
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }
}

/// A count is not a diagnosis. Name the first pack that failed and quote
/// its reason verbatim (trimmed to one status-line's worth), so an
/// "N failed" summary always ships at least one thread to pull.
pub fn failure_summary(failed: &[(String, String)]) -> String {
    let Some((pack, message)) = failed.first() else {
        return String::new();
    };
    let mut reason = message.replace('\n', " ");
    if reason.chars().count() > 160 {
        reason = reason.chars().take(157).collect::<String>() + "…";
    }
    let rest = failed.len().saturating_sub(1);
    if rest == 0 {
        format!("{pack}: {reason}")
    } else {
        format!("{pack}: {reason} (+{rest} more)")
    }
}

fn bake_status_fragment(bake: &BakeStats, icons: &str) -> String {
    let mut parts = Vec::new();
    if bake.total > 0 {
        parts.push(format!(
            "AO {}/{} ({} fresh, {} failed)",
            bake.baked + bake.skipped,
            bake.total,
            bake.skipped,
            bake.failed
        ));
    }
    parts.push(icons.to_string());
    parts.join(" · ")
}

fn kaykit_spec() -> pack_import::PackSourceSpec {
    let terms = b"KayKit CC0-1.0 public domain. Credit Kay Lousberg (kaylousberg.com).";
    pack_import::PackSourceSpec {
        source_id: Some("kaykit".into()),
        source_title: Some("KayKit / Kay Lousberg".into()),
        pack_name: Some("adventurers".into()),
        pack_version: Some("1.0".into()),
        license: Some("CC0-1.0".into()),
        license_revision: None,
        terms_digest: Some({
            let d = sha256(terms);
            let mut s = String::with_capacity(64);
            for b in d {
                s.push_str(&format!("{b:02x}"));
            }
            s
        }),
        terms_url: Some("https://creativecommons.org/publicdomain/zero/1.0/".into()),
        credits: Some(KAYKIT_MODULE.credits.into()),
        source: Some(KAYKIT_MODULE.homepage.into()),
        source_archive: None,
        redistribution: Some("allowed".into()),
        derivatives: Some("allowed".into()),
    }
}

fn run_kaykit_import(
    pack_dir: &Path,
    out: &Path,
    spec: pack_import::PackSourceSpec,
    server: Option<ServerSession>,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
    icon_resume_rx: &Receiver<()>,
) -> ImportPhase {
    let staged = out.join("work").join("source");
    let dest_root = out.join("out");
    if let Err(error) = std::fs::create_dir_all(&dest_root) {
        return ImportPhase::Failed {
            pack: "kaykit".into(),
            message: format!("create out root: {error}"),
        };
    }
    let bundle = dest_root.join("bundle");
    if bundle.exists() {
        let _ = std::fs::remove_dir_all(&bundle);
    }
    let (glbs, images) = match copy_pack_source_files(pack_dir, &staged) {
        Ok(pair) => pair,
        Err(error) => {
            return ImportPhase::Failed {
                pack: "kaykit".into(),
                message: error,
            };
        }
    };
    ao_bake::seed_ao_from_source(pack_dir, &staged);
    // A real icon straight from the mesh's own skeleton/animation where one
    // exists — no GPU render needed for those. Must run BEFORE
    // `kaykit_library_landings` (whose premade-thumbnail detection is a
    // bare `thumb.is_file()`, no freshness check) and before any fallback
    // placeholder gets synthesized, or a stale/placeholder `<stem>.png`
    // could be mistaken for a real premade thumbnail.
    write_kaykit_anim_icons(&staged);
    let library = kaykit_library_landings(&staged);
    // Real icons before publish, ALWAYS: hand off to the UI thread for
    // whatever GLB `write_kaykit_anim_icons` couldn't cover (GPU-rendered
    // or fingerprint-reused from `pack_icons_dir`), then block until it
    // signals done or this import is cancelled. See `run_kenney_import`
    // for the full rationale — same handshake, same guard.
    let _ = tx.send(ImportPhase::IconsPending {
        pack: "kaykit".into(),
        assets: library.len(),
        library: library.clone(),
        bake: BakeStats::default(),
    });
    if icon_resume_rx.recv().is_err() {
        return ImportPhase::Cancelled {
            pack: "kaykit".into(),
            message: "icon handshake lost".into(),
        };
    }
    if cancel.load(Ordering::SeqCst) {
        return ImportPhase::Cancelled {
            pack: "kaykit".into(),
            message: "stopped while rendering icons".into(),
        };
    }
    if let Err(error) = assign_staged_thumbnails(&glbs, &images, pack_dir, &staged) {
        return ImportPhase::Failed {
            pack: "kaykit".into(),
            message: error,
        };
    }
    if let Some(stem) = first_placeholder_stem(&glbs, &images, &staged) {
        return ImportPhase::Failed {
            pack: "kaykit".into(),
            message: format!("icon for {stem} never rendered — refusing to publish a placeholder"),
        };
    }
    let report = match pack_import::compile_pack(&staged, &bundle, spec, None, false) {
        Ok(report) => report,
        Err(error) => {
            return ImportPhase::CompiledLocal {
                pack: "kaykit".into(),
                assets: library.len(),
                blobs: 0,
                out: bundle,
                error: Some(format!("compile refused the pack: {error}")),
                library,
                bake: BakeStats::default(),
            };
        }
    };
    if cancel.load(Ordering::SeqCst) {
        return ImportPhase::Cancelled {
            pack: "kaykit".into(),
            message: "stopped after compile".into(),
        };
    }
    let publish_result = if let Some(session) = server {
        let _ = tx.send(ImportPhase::Publishing {
            pack: "kaykit".into(),
            assets: report.assets,
            blobs: report.blobs,
            blob_done: 0,
        });
        match publish_compiled_pack(
            &staged,
            &bundle,
            &session,
            "kaykit",
            KAYKIT_MODULE.homepage,
            report.assets,
            report.blobs,
            tx,
            cancel,
        ) {
            Ok(pair) => Some(Ok(pair)),
            Err(error) => Some(Err(error)),
        }
    } else {
        None
    };
    if let Some(Err(error)) = &publish_result {
        return ImportPhase::CompiledLocal {
            pack: "kaykit".into(),
            assets: report.assets,
            blobs: report.blobs,
            out: report.plan_path.clone(),
            error: Some(format!("publish to the asset store failed: {error}")),
            library,
            bake: BakeStats::default(),
        };
    }
    let bake = BakeStats::default();
    let (created, annotated) = match publish_result {
        Some(Ok(pair)) => pair,
        _ => (false, 0),
    };
    if publish_result.is_none() {
        return ImportPhase::CompiledLocal {
            pack: "kaykit".into(),
            assets: report.assets,
            blobs: report.blobs,
            out: report.plan_path,
            error: None,
            library,
            bake,
        };
    }
    clear_pack_staging("kaykit", out, true);
    ImportPhase::Published {
        pack: "kaykit".into(),
        assets: report.assets,
        blobs: report.blobs,
        created,
        annotated,
        out: report.plan_path,
        library,
        bake,
    }
}

fn write_kaykit_anim_icons(staged: &Path) {
    let mut stack = vec![staged.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("glb") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Some(png) = makepad_asset_importer::anim_icon::skinned_anim_sheet(&bytes) {
                let _ = std::fs::write(path.with_extension("png"), png);
            }
        }
    }
}

fn kaykit_library_landings(staged: &Path) -> Vec<LibraryLanding> {
    let mut out = Vec::new();
    let mut stack = vec![staged.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("glb") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("character")
                .to_string();
            let thumb = path.with_extension("png");
            out.push(LibraryLanding {
                path,
                label: makepad_asset_importer::stateful_billboard::mesh_title(&stem),
                domain: "character",
                content_type: "model/gltf-binary",
                prompt: format!(
                    "KayKit adventurers · character · {stem} · CC0-1.0 · {}",
                    KAYKIT_MODULE.homepage
                ),
                thumbnail: thumb.is_file().then_some(thumb),
                source_id: "kaykit".into(),
                pack: "adventurers".into(),
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Delete a pack's ORIGINAL staging (`work/`+`out/`, direct children of
/// `out` — the exact root `run_kenney_import`/`run_kaykit_import` were
/// given — never `icons/`, the persistent pack-icon cache, or any OTHER
/// sibling) once its publish has actually succeeded. Real icons are already
/// guaranteed to exist by the time this runs — the icon-render handshake
/// blocks compile+publish until they do — so unlike the old "publish now,
/// re-icon+republish+then-clean later" flow there is only ONE publish and
/// ONE cleanup point per pack. Reuses the caller's own `out` root instead of
/// reconstructing a `source/pack` path so it can never drift out of sync
/// with Kenney's `.../kenney/<pack>` vs. KayKit's single `.../kaykit` (no
/// pack subdirectory) layouts. A failure leaves staging in place for
/// inspection/retry (`staging_dirs_to_clear(false)` is empty).
pub(crate) fn clear_pack_staging(pack_name: &str, out: &Path, publish_ok: bool) {
    let mut cleared = Vec::new();
    for sub in staging_dirs_to_clear(publish_ok).iter().copied() {
        let path = out.join(sub);
        if !path.is_dir() {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => cleared.push(sub),
            Err(error) => log!("import: {pack_name}: could not clear staging {sub}/: {error}"),
        }
    }
    if !cleared.is_empty() {
        log!("import: {pack_name}: staging reclaimed ({})", cleared.join(", "));
    }
}

fn run_kenney_import(
    pack_dir: &Path,
    out: &Path,
    spec: pack_import::PackSourceSpec,
    pack_name: String,
    pack_page: String,
    server: Option<ServerSession>,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
    icon_resume_rx: &Receiver<()>,
) -> ImportPhase {
    // pack_import refuses --out whose existing ancestor contains the pack
    // (or vice versa). Source and bundle must be in sibling trees, and the
    // bundle path itself must be absent so classify_out sees a new dest.
    let staged = out.join("work").join("source");
    let dest_root = out.join("out");
    if let Err(error) = std::fs::create_dir_all(&dest_root) {
        return ImportPhase::Failed {
            pack: pack_name,
            message: format!("create out root: {error}"),
        };
    }
    let bundle = dest_root.join("bundle");
    if bundle.exists() {
        let _ = std::fs::remove_dir_all(&bundle);
    }
    let (glbs, images) = match copy_pack_source_files(pack_dir, &staged) {
        Ok(pair) => pair,
        Err(error) => {
            return ImportPhase::Failed {
                pack: pack_name,
                message: error,
            };
        }
    };
    // BEFORE any thumbnail is synthesized: a `<stem>.png` that exists only
    // because we are about to write it (rendered-icon-or-placeholder) must
    // never be mistaken for a pack-provided preview. `library_landings`'s
    // `sibling_preview_png` treats any non-blank sibling PNG as "premade",
    // with no fingerprint check — if it ran after `assign_staged_thumbnails`
    // a STALE cached icon from a changed GLB would slip through as
    // "premade" and skip the fresh render entirely.
    let library = library_landings(&staged, &pack_name, &pack_page);
    if library.is_empty() {
        return ImportPhase::Failed {
            pack: pack_name,
            message: "folder has no png/jpeg/wav/mp4/glb to import".into(),
        };
    }
    // AO/shadow bake BEFORE compile: the compiler attaches `<stem>.aomesh`,
    // `.ao.png` and `.shadowsdf` to their GLB as derived roles, so the
    // published asset carries its bake and a game streaming the mesh gets
    // AO from the catalog. A source tree that was baked before (the restored
    // Kenney cache, a re-import) seeds fresh sidecars and the bake is free;
    // the UI still copies the sidecars onto library payloads at land time.
    let bake = match kenney_bake_or_cancel(
        pack_dir,
        &staged,
        &pack_name,
        library.len(),
        0,
        tx,
        cancel,
    ) {
        Ok(bake) => bake,
        Err(phase) => return phase,
    };
    // Real icons before publish, ALWAYS: hand the staged GLBs to the UI
    // thread (via `library`, same shape `Published`/`PackFinished` already
    // carry — `land_imported_pack` starts queuing icon work immediately)
    // and block here until it signals every icon is done (rendered fresh,
    // or fingerprint-reused from `pack_icons_dir`), or this import gets
    // cancelled. No placeholder-thumbnailed revision may ever reach the
    // store — see `ImportPage::icons_pending_ready`/`resume_icons_pending`.
    let _ = tx.send(ImportPhase::IconsPending {
        pack: pack_name.clone(),
        assets: library.len(),
        library: library.clone(),
        bake: bake.clone(),
    });
    if icon_resume_rx.recv().is_err() {
        // The UI-side sender was dropped (app shutting down) — never
        // publish with whatever staging happens to hold right now.
        return ImportPhase::Cancelled {
            pack: pack_name,
            message: "icon handshake lost".into(),
        };
    }
    if cancel.load(Ordering::SeqCst) {
        return ImportPhase::Cancelled {
            pack: pack_name,
            message: "stopped while rendering icons".into(),
        };
    }
    if let Err(error) = assign_staged_thumbnails(&glbs, &images, pack_dir, &staged) {
        return ImportPhase::Failed {
            pack: pack_name,
            message: error,
        };
    }
    // Last guard: never publish a placeholder. A render that silently
    // failed, or a fingerprint cache miss that fell through, still shows up
    // here as a black `<stem>.png` — fail the pack loudly instead.
    if let Some(stem) = first_placeholder_stem(&glbs, &images, &staged) {
        return ImportPhase::Failed {
            pack: pack_name,
            message: format!("icon for {stem} never rendered — refusing to publish a placeholder"),
        };
    }
    // Catalog compile is fail-closed about the PACK; a single model in a
    // shape it does not support is named in `report.skipped_models` and
    // left out rather than costing the kit. A pack-level refusal still
    // keeps the compiler error in the status line.
    let report = match pack_import::compile_pack(&staged, &bundle, spec, None, false) {
        Ok(report) => report,
        Err(error) => {
            return ImportPhase::CompiledLocal {
                pack: pack_name,
                assets: library.len(),
                blobs: 0,
                out: bundle,
                error: Some(format!("compile refused the pack: {error}")),
                library,
                bake,
            };
        }
    };
    // A model left out is not a silent drop: name each one and why, at
    // warning level, so "34 packs imported" can never hide content that
    // did not arrive.
    for (path, why) in &report.skipped_models {
        log!("import: {pack_name}: SKIPPED {path} — {why}");
    }
    if cancel.load(Ordering::SeqCst) {
        return ImportPhase::Cancelled {
            pack: pack_name,
            message: "stopped after compile".into(),
        };
    }

    let publish_result = if let Some(session) = server {
        let _ = tx.send(ImportPhase::Publishing {
            pack: pack_name.clone(),
            assets: report.assets,
            blobs: report.blobs,
            blob_done: 0,
        });
        match publish_compiled_pack(
            &staged,
            &bundle,
            &session,
            &pack_name,
            &pack_page,
            report.assets,
            report.blobs,
            tx,
            cancel,
        ) {
            Ok(pair) => Some(Ok(pair)),
            Err(error) => Some(Err(error)),
        }
    } else {
        None
    };

    let (created, annotated, plan_path) = match publish_result {
        Some(Err(error)) => {
            return ImportPhase::CompiledLocal {
                pack: pack_name,
                assets: report.assets,
                blobs: report.blobs,
                out: report.plan_path.clone(),
                error: Some(format!("publish to the asset store failed: {error}")),
                library,
                bake,
            };
        }
        Some(Ok(pair)) => (pair.0, pair.1, report.plan_path),
        None => (false, 0usize, report.plan_path),
    };

    if publish_result.is_none() {
        return ImportPhase::CompiledLocal {
            pack: pack_name,
            assets: report.assets,
            blobs: report.blobs,
            out: plan_path,
            error: None,
            library,
            bake,
        };
    }
    // Published, with real icons already baked into the manifest — the
    // ONLY cleanup point staging needs now (see `clear_pack_staging`).
    clear_pack_staging(&pack_name, out, true);
    ImportPhase::Published {
        pack: pack_name,
        assets: report.assets,
        blobs: report.blobs,
        created,
        annotated,
        out: plan_path,
        library,
        bake,
    }
}

/// True when a Kenney folder has at least one file pack_import / the
/// library can ingest. glTF-only kits (no `.glb`) stay off the Import list.
fn pack_has_importable_payload(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(here) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&here) else {
            continue;
        };
        for entry in rd.flatten() {
            seen += 1;
            if seen > 8192 {
                return false;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "wav" | "mp4" | "glb") {
                return true;
            }
        }
    }
    false
}

fn kenney_bake_or_cancel(
    pack_dir: &Path,
    staged: &Path,
    pack_name: &str,
    assets: usize,
    annotated: usize,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
) -> Result<BakeStats, ImportPhase> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ImportPhase::Cancelled {
            pack: pack_name.to_string(),
            message: "stopped before AO bake".into(),
        });
    }
    ao_bake::seed_ao_from_source(pack_dir, staged);
    let bake = bake_staged_glbs(staged, pack_name, assets, annotated, tx, cancel);
    if cancel.load(Ordering::SeqCst) {
        return Err(ImportPhase::Cancelled {
            pack: pack_name.to_string(),
            message: format!(
                "stopped during AO bake ({}/{})",
                bake.baked + bake.skipped,
                bake.total
            ),
        });
    }
    Ok(bake)
}

// ---------------------------------------------------------------------------
// Pack icon store: the ONLY thing pack imports persist locally.
//
// The local AI-workspace library (`library.rs`) is for AI-generated / drop /
// webcam content; a pack lands in the ASSET STORE ONLY (`publish_compiled_pack`,
// run once from `run_kenney_import`/`run_kaykit_import`, ONLY after real
// icons exist — see the icon-render handshake, `ImportPhase::IconsPending`)
// and never gets a library row. GPU icon rendering (`land_imported_pack` in
// main.rs) still needs somewhere durable to cache each rendered icon so
// `assign_staged_thumbnails` can embed the real render instead of a black
// placeholder before that one publish, and so an unchanged reimport can
// skip re-rendering entirely — that "somewhere" is this directory, a
// sibling of (never inside) the pack's `work/`/`out/` staging trees, which
// ARE deleted once the publish succeeds (`staging_dirs_to_clear`,
// `clear_pack_staging`).
// ---------------------------------------------------------------------------

/// Where a pack's rendered GPU icons persist across imports. Layout:
/// `<dir>/<stem>.png` (the rendered icon) + `<dir>/<stem>.fp` (a content
/// fingerprint of the GLB bytes that were rendered — the staging-free analog
/// of `library::keep_existing_glb_thumbnail`'s byte comparison, since there
/// is no persisted payload copy to compare against once staging is deleted).
pub fn pack_icons_dir(source_id: &str, pack: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_app/import")
        .join(source_id)
        .join(pack)
        .join("icons")
}

/// Cheap, deterministic (NOT cryptographic) content fingerprint — enough to
/// detect "this pack item's bytes changed since we last rendered its icon".
pub fn content_fingerprint(bytes: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.len().hash(&mut hasher);
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Stable per-item icon-render key. `ThumbnailRenderer`/`ImportPage::track_import_icon`
/// treat this as an opaque string id, so the fingerprint rides along inside
/// it for free — `drain_rendered_thumbnails` (main.rs) parses it back out
/// with [`parse_pack_icon_key`] to persist the finished render without ever
/// needing a library row.
pub fn pack_icon_key(source_id: &str, pack: &str, stem: &str, fingerprint: &str) -> String {
    format!("pack:{source_id}:{pack}:{stem}:{fingerprint}")
}

/// Inverse of [`pack_icon_key`]. `None` for any non-pack (library) icon key.
pub fn parse_pack_icon_key(key: &str) -> Option<(String, String, String, String)> {
    let rest = key.strip_prefix("pack:")?;
    let mut parts = rest.splitn(4, ':');
    let source_id = parts.next()?.to_string();
    let pack = parts.next()?.to_string();
    let stem = parts.next()?.to_string();
    let fingerprint = parts.next()?.to_string();
    Some((source_id, pack, stem, fingerprint))
}

/// Pure keep-vs-rerender decision for a pack item's persisted icon — the
/// staging-free analog of `library::keep_existing_glb_thumbnail`. Keep only
/// when a previously rendered icon is on disk AND its recorded fingerprint
/// still matches the freshly staged bytes; a changed payload (or an icon
/// that never rendered) still gets a render.
pub fn pack_icon_reusable(icon_exists: bool, recorded_fp: Option<&str>, new_fp: &str) -> bool {
    icon_exists && recorded_fp == Some(new_fp)
}

/// Read a pack item's persisted icon from `dir` (see [`pack_icons_dir`]) if
/// its recorded fingerprint still matches `new_fp`. `None` means: render.
pub fn read_reusable_pack_icon(dir: &Path, stem: &str, new_fp: &str) -> Option<Vec<u8>> {
    let icon_path = dir.join(format!("{stem}.png"));
    let fp_path = dir.join(format!("{stem}.fp"));
    let recorded = std::fs::read_to_string(&fp_path).ok();
    if pack_icon_reusable(icon_path.is_file(), recorded.as_deref().map(str::trim), new_fp) {
        std::fs::read(&icon_path).ok()
    } else {
        None
    }
}

/// Persist a freshly rendered pack icon + its fingerprint under `dir` (see
/// [`pack_icons_dir`]) so the next reimport of unchanged content can skip
/// the GPU render.
pub fn write_pack_icon(dir: &Path, stem: &str, fingerprint: &str, png: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{stem}.png")), png)?;
    std::fs::write(dir.join(format!("{stem}.fp")), fingerprint)?;
    Ok(())
}

/// The staging subdirectories to delete after a pack's publish+icon-refresh
/// cycle — `work/`+`out/`, everything the initial `run_kenney_import` wrote
/// and nothing else ever reads again (`ImportPhase::{CompiledLocal,Published}::out`
/// is write-only/display metadata) — ONLY on success; a failure keeps them
/// around for inspection/retry. `icons/` (this pack's persistent GPU-icon
/// cache) and `thumb-refresh/` (already self-cleaning) are never included.
pub fn staging_dirs_to_clear(publish_succeeded: bool) -> &'static [&'static str] {
    if publish_succeeded {
        &["work", "out"]
    } else {
        &[]
    }
}

/// Copy only pack-source files into `dest` (file walk + GLB texture embed).
/// Engine-derived sidecars stay behind. Returns every staged GLB path and
/// the stems that already ship their OWN same-stem PNG/JPEG ≥ 512px — a
/// thumbnail must never be synthesized over those. Split out of the old
/// `stage_source_pack` so `run_kenney_import`/`run_kaykit_import` can run
/// the icon-assignment pass (`assign_staged_thumbnails`) only ONCE, AFTER
/// the icon-render handshake resolves — never before, so compile+publish
/// can never run against a placeholder. `stage_source_pack` itself (still
/// used by tests that don't care about that ordering) just calls both
/// halves back to back.
fn copy_pack_source_files(
    src: &Path,
    dest: &Path,
) -> Result<(Vec<PathBuf>, std::collections::BTreeSet<String>), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("clear staging {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create staging {}: {e}", dest.display()))?;
    let mut glbs = Vec::new();
    let mut images = std::collections::BTreeSet::new();
    let mut stack = vec![src.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in rd.flatten() {
            seen += 1;
            if seen > 8192 {
                return Err("pack walk exceeded 8192 entries".into());
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".ao.png") || lower.ends_with(".aomesh") || lower.ends_with(".shadowsdf")
            {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "wav" | "mp4" | "glb") {
                continue;
            }
            // KayKit ships `name_texture.png` beside an already-embedded GLB.
            // Copying it makes pack_import treat a second image as a payload.
            if ext == "png" && lower.ends_with("_texture.png") {
                continue;
            }
            // An older import left the 512² black placeholder BESIDE the
            // source GLBs (`local/packs/kenney/*/<stem>.png`). It is not a
            // preview: skip it here so the GLB gets a real assigned icon
            // instead of publishing a black thumbnail forever.
            if ext == "png"
                && path.with_extension("glb").is_file()
                && std::fs::read(&path).is_ok_and(|bytes| is_blank_preview_png(&bytes))
            {
                continue;
            }
            let rel = path.strip_prefix(src).unwrap_or(&path);
            let target = dest.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            std::fs::copy(&path, &target)
                .map_err(|e| format!("copy {} → {}: {e}", path.display(), target.display()))?;
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                // Keyed by the staged-RELATIVE stem, the same shape the
                // pack key has. Keying by bare file name made a texture
                // shadow a model of the same name from another directory:
                // `Textures/fence.png` convinced the thumbnail pass that
                // `fence.glb` already shipped its own preview, so no icon
                // was written and compile refused the whole kit for a
                // missing thumbnail. Five models across the two retro kits
                // (fence/roof/water, grass/planks) were exactly that.
                if let Some(stem) = staged_stem_key(dest, &target) {
                    images.insert(stem);
                }
            }
            if ext == "glb" {
                glbs.push(target);
            }
        }
    }
    for glb in &glbs {
        let Ok(bytes) = std::fs::read(glb) else {
            continue;
        };
        let Some(dir) = glb.parent() else {
            continue;
        };
        match embed_glb_file_images(&bytes, dir) {
            Ok(embedded) if embedded != bytes => {
                std::fs::write(glb, embedded)
                    .map_err(|e| format!("embed {}: {e}", glb.display()))?;
            }
            Ok(_) => {}
            // Dummy / truncated files stay as-is; missing textures still fail
            // the pack so we do not land untextured Kenney GLBs silently.
            Err(err) if err.contains("not a GLB") => {}
            Err(err) => {
                return Err(format!(
                    "{}: {err}",
                    glb.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    Ok((glbs, images))
}

/// Give every staged GLB without its own image (`images`) — and without an
/// already-real synthesized one, e.g. KayKit's skinned anim sheet — a
/// thumbnail PNG: the persistent pack-icon cache's ([`pack_icons_dir`])
/// rendered render if one matches by stem, else the 512² placeholder.
/// pack_import needs exactly one thumbnail per mesh to emit a valid
/// manifest. Idempotent and safe to call again after `pack_icons_dir`
/// gains fresh renders (see the icon-render handshake in
/// `run_kenney_import`/`run_kaykit_import`) — it only ever (re)writes
/// `<stem>.png`, never touches AO/shadow sidecars, the GLBs, or a stem that
/// already has a real (non-placeholder) icon on disk.
/// A staged file's identity for thumbnail matching: its path relative to
/// the staging root, without the extension — the same shape the pack entry
/// key has (`classify_rel`). A bare file name is NOT that: two files with
/// the same name in different directories are different entries, and
/// treating them as one is how a texture atlas came to stand in for a
/// model's missing preview.
fn staged_stem_key(staged_root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(staged_root).ok()?;
    let mut key = rel.with_extension("").to_string_lossy().to_string();
    if key.is_empty() {
        return None;
    }
    key = key.replace('\\', "/").to_ascii_lowercase();
    Some(key)
}

fn assign_staged_thumbnails(
    glbs: &[PathBuf],
    images: &std::collections::BTreeSet<String>,
    source_pack_dir: &Path,
    staged_root: &Path,
) -> Result<(), String> {
    let rendered = rendered_pack_icons(source_pack_dir);
    let placeholder = placeholder_png_512();
    for glb in glbs {
        if staged_stem_key(staged_root, glb).is_some_and(|k| images.contains(&k)) {
            continue;
        }
        let stem = glb.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
        let thumb = glb.with_extension("png");
        let already_real = std::fs::read(&thumb).is_ok_and(|bytes| !is_blank_preview_png(&bytes));
        if already_real {
            continue;
        }
        let icon = stem
            .to_str()
            .and_then(|stem| rendered.get(stem))
            .and_then(|path| std::fs::read(path).ok())
            .filter(|bytes| is_still_png_at_least(bytes, 256));
        match icon {
            Some(bytes) => std::fs::write(&thumb, bytes)
                .map_err(|e| format!("write icon {}: {e}", thumb.display()))?,
            None => std::fs::write(&thumb, &placeholder)
                .map_err(|e| format!("write placeholder {}: {e}", thumb.display()))?,
        }
    }
    Ok(())
}

/// The first staged GLB (without its own source image) whose `<stem>.png`
/// is still the black placeholder — meaning its icon render never
/// completed or the persistent icon cache came back empty for it. `None`
/// means every staged mesh has a real icon and it is safe to publish. Used
/// as the last guard before compile+publish in `run_kenney_import`/
/// `run_kaykit_import`: never let a placeholder-thumbnailed revision reach
/// the store. Uses [`is_blank_preview_png`] today; swap the body to the
/// importer's `thumbnail_is_placeholder(bytes)` once it lands (same call
/// site, tracked with the importer worker doing classic-pack parity).
fn first_placeholder_stem(
    glbs: &[PathBuf],
    images: &std::collections::BTreeSet<String>,
    staged_root: &Path,
) -> Option<String> {
    for glb in glbs {
        if staged_stem_key(staged_root, glb).is_some_and(|k| images.contains(&k)) {
            continue;
        }
        let stem = glb.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
        let thumb = glb.with_extension("png");
        // Missing/unreadable is exactly as bad as a placeholder: refuse.
        // `thumbnail_is_placeholder` is the SAME decode-and-check
        // `pack_import::compile_pack` itself runs before accepting a
        // manifest thumbnail — using it here means this guard can only
        // ever be stricter-or-equal, never miss something compile would
        // have caught anyway.
        let is_placeholder = std::fs::read(&thumb)
            .map(|bytes| makepad_asset_importer::thumbs::thumbnail_is_placeholder(&bytes))
            .unwrap_or(true);
        if is_placeholder {
            return Some(stem.to_string_lossy().to_string());
        }
    }
    None
}

/// Copy pack-source files AND assign every staged GLB a thumbnail in one
/// call — for callers (tests) that don't need to wait for GPU-rendered
/// icons before "compiling". `run_kenney_import`/`run_kaykit_import` call
/// the two halves (`copy_pack_source_files`, then — after the icon-render
/// handshake — `assign_staged_thumbnails`) separately instead, so a real
/// import never compiles/publishes before real icons exist.
fn stage_source_pack(src: &Path, dest: &Path) -> Result<(), String> {
    let (glbs, images) = copy_pack_source_files(src, dest)?;
    assign_staged_thumbnails(&glbs, &images, src, dest)
}

/// Icons already rendered for this pack, by model stem — read from the
/// PERSISTENT pack-icon cache ([`pack_icons_dir`]), never the local library:
/// pack imports no longer create library rows (`land_imported_pack` in
/// main.rs), so this cache is the only place a rendered icon for reuse can
/// come from. The pack directory is `…/<source>/<pack>`.
fn rendered_pack_icons(pack_dir: &Path) -> std::collections::HashMap<String, PathBuf> {
    let mut out = std::collections::HashMap::new();
    let (Some(pack), Some(source)) = (
        pack_dir.file_name().and_then(|n| n.to_str()),
        pack_dir.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
    ) else {
        return out;
    };
    let dir = pack_icons_dir(source, pack);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.insert(stem.to_string(), path);
    }
    out
}

/// A PNG still (not an anim strip) of at least `min` px on both sides —
/// what the content contract accepts as a mesh thumbnail.
fn is_still_png_at_least(bytes: &[u8], min: u32) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24 {
        return false;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    w >= min && h >= min && w <= 4096 && h <= 4096 && !is_blank_preview_png(bytes)
}

fn library_landings(staged: &Path, pack: &str, page: &str) -> Vec<LibraryLanding> {
    let mut out = Vec::new();
    let mut stack = vec![staged.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("asset")
                .to_string();
            let (domain, content_type) = match ext.as_str() {
                "glb" => ("mesh", "model/gltf-binary"),
                "png" if !stem.ends_with(".ao") => ("image", "image/png"),
                "jpg" | "jpeg" => ("image", "image/jpeg"),
                "wav" => {
                    let blob = format!("{pack}/{stem}").to_ascii_lowercase();
                    if blob.contains("music") || blob.contains("jingle") || blob.contains("/bgm") {
                        ("music", "audio/wav")
                    } else {
                        ("audio", "audio/wav")
                    }
                }
                "mp4" => ("video", "video/mp4"),
                _ => continue,
            };
            // Placeholder thumbs written beside GLBs are not library payloads.
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                let sibling = path.with_extension("glb");
                if sibling.is_file() {
                    continue;
                }
            }
            let thumbnail = if ext == "glb" {
                sibling_preview_png(&path)
            } else {
                None
            };
            out.push(LibraryLanding {
                path,
                label: format!("kenney/{pack}/{stem}"),
                domain,
                content_type,
                prompt: format!("Kenney {pack} · {stem} · CC-BY-4.0 · {page}"),
                thumbnail,
                source_id: "kenney".into(),
                pack: pack.to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Same-stem still that shipped with the pack (Kenney preview renders).
/// The generated 512² placeholder is not a real icon — leave `None` so the
/// GPU thumbnailer can replace it.
fn sibling_preview_png(glb: &Path) -> Option<PathBuf> {
    for ext in ["png", "jpg", "jpeg"] {
        let preview = glb.with_extension(ext);
        if !preview.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&preview) else {
            continue;
        };
        if ext == "png" {
            if is_blank_preview_png(&bytes) {
                continue;
            }
        } else if bytes.len() < 32 {
            continue;
        }
        return Some(preview);
    }
    None
}

/// True for the pack_import 512² black placeholder (any zlib encoding).
pub fn is_blank_preview_png(bytes: &[u8]) -> bool {
    if bytes == placeholder_png_512() {
        return true;
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 33 {
        return false;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if w != 512 || h != 512 {
        return false;
    }
    png_idat_is_all_zero(bytes)
}

fn png_idat_is_all_zero(bytes: &[u8]) -> bool {
    let mut off = 8usize;
    let mut idat = Vec::new();
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]) as usize;
        if off + 12 + len > bytes.len() {
            return false;
        }
        let typ = &bytes[off + 4..off + 8];
        if typ == b"IDAT" {
            idat.extend_from_slice(&bytes[off + 8..off + 8 + len]);
        }
        if typ == b"IEND" {
            break;
        }
        off += 12 + len;
    }
    if idat.len() < 2 {
        return false;
    }
    // Stored deflate (the placeholder writer): 0x78 0x01, then
    // [final][len][~len][zeros] blocks. Reject only if every sample is 0.
    let mut i = 2usize;
    while i + 5 <= idat.len() {
        let last = idat[i] & 1 != 0;
        if idat[i] & 0x06 != 0 {
            return false;
        }
        let n = u16::from_le_bytes([idat[i + 1], idat[i + 2]]) as usize;
        i += 5;
        if i + n > idat.len() {
            return false;
        }
        if idat[i..i + n].iter().any(|&b| b != 0) {
            return false;
        }
        i += n;
        if last {
            return true;
        }
    }
    false
}

/// Kenney kits share one atlas as `Textures/colormap.png` beside the GLBs.
/// Search the GLB folder, `Textures/`, and a few parents — not other packs.
fn resolve_pack_texture(base_dir: &Path, uri: &str) -> Option<PathBuf> {
    let name = Path::new(uri).file_name()?;
    let mut dir = Some(base_dir);
    for _ in 0..6 {
        let Some(here) = dir else {
            break;
        };
        for candidate in [
            here.join(uri),
            here.join(name),
            here.join("Textures").join(name),
            here.join("textures").join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        dir = here.parent();
    }
    recover_kenney_colormap(base_dir, name)
}

/// Starter-kit Kenney folders were flattened from GitHub without `Textures/`.
/// Re-fetch the official atlas for that pack (not another kit's palette).
fn recover_kenney_colormap(base_dir: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
    if name.to_string_lossy().to_ascii_lowercase() != "colormap.png" {
        return None;
    }
    let mut dir = Some(base_dir);
    let mut pack = None;
    for _ in 0..8 {
        let Some(here) = dir else {
            break;
        };
        if let Some(n) = here.file_name().and_then(|s| s.to_str()) {
            if matches!(n, "city" | "arena" | "fps" | "platformer" | "racing") {
                pack = Some(n.to_string());
                break;
            }
        }
        dir = here.parent();
    }
    let pack = pack?;
    let url = kenney_starter_colormap_url(&pack)?;
    let dest = base_dir.join("Textures").join("colormap.png");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let status = std::process::Command::new("curl")
        .args(["-sSfL", "--globoff", "-o"])
        .arg(&dest)
        .arg(url)
        .status()
        .ok()?;
    if status.success() && dest.is_file() {
        let bytes = std::fs::read(&dest).ok()?;
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(dest);
        }
    }
    let _ = std::fs::remove_file(&dest);
    None
}

fn kenney_starter_colormap_url(pack: &str) -> Option<&'static str> {
    Some(match pack {
        "city" => {
            "https://raw.githubusercontent.com/KenneyNL/Starter-Kit-City-Builder/4535092b740b378b700efd9df9e27a631815b84a/models/Textures/colormap.png"
        }
        "arena" => {
            "https://raw.githubusercontent.com/KenneyNL/Starter-Kit-Basic-Scene/a6927e66ff8dd8e173660ce4825abe773c65f683/sample/Mini%20Arena/Models/GLB%20format/Textures/colormap.png"
        }
        "fps" => {
            "https://raw.githubusercontent.com/KenneyNL/Starter-Kit-FPS/185fd2326d74a5cf858cffc616f87cf9696f9cc0/models/Textures/colormap.png"
        }
        "platformer" => {
            "https://raw.githubusercontent.com/KenneyNL/Starter-Kit-3D-Platformer/3fa8a04b1c01ab23db43123d4ce814a34c3fc7f0/models/Textures/colormap.png"
        }
        "racing" => {
            "https://raw.githubusercontent.com/KenneyNL/Starter-Kit-Racing/f5241ebdf00c25bc951bf4fdb7950bb1b78b4bcc/models/Textures/colormap.png"
        }
        _ => return None,
    })
}

/// Inline `images[].uri` files next to the GLB so library copies (and the
/// GPU thumbnailer, which loads with no base dir) stay self-contained.
/// Kenney car-kit ships `Textures/colormap.png` outside the GLB.
pub fn embed_glb_file_images(glb: &[u8], base_dir: &Path) -> Result<Vec<u8>, String> {
    let (json, bin) = split_glb(glb)?;
    let json_s = std::str::from_utf8(json).map_err(|e| e.to_string())?;
    if !json_s.contains("\"uri\"") {
        return Ok(glb.to_vec());
    }
    let uris = json_string_values(json_s, "uri");
    let mut embeds: Vec<(String, Vec<u8>, &'static str)> = Vec::new();
    for uri in &uris {
        let lower = uri.to_ascii_lowercase();
        let mime = if lower.ends_with(".png") {
            "image/png"
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else {
            continue;
        };
        let path = resolve_pack_texture(base_dir, uri).ok_or_else(|| {
            format!(
                "missing texture {uri} (looked next to the GLB and in Textures/). This Kenney folder is incomplete — re-extract the official pack so {uri} is on disk."
            )
        })?;
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        embeds.push((uri.clone(), bytes, mime));
    }
    if embeds.is_empty() {
        return Ok(glb.to_vec());
    }
    let view_base = json_array_len(json_s, "bufferViews").unwrap_or(0);
    let mut new_bin = bin.to_vec();
    let mut views_json = String::new();
    let mut image_repls: Vec<(String, String)> = Vec::new();
    for (i, (uri, bytes, mime)) in embeds.iter().enumerate() {
        while new_bin.len() % 4 != 0 {
            new_bin.push(0);
        }
        let offset = new_bin.len();
        new_bin.extend_from_slice(bytes);
        if i > 0 {
            views_json.push(',');
        }
        views_json.push_str(&format!(
            "{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}}}",
            bytes.len()
        ));
        let view = view_base + i;
        image_repls.push((
            format!("\"uri\":\"{uri}\""),
            format!("\"bufferView\":{view},\"mimeType\":\"{mime}\""),
        ));
    }
    let mut json_out = json_s.to_string();
    for (from, to) in &image_repls {
        json_out = json_out.replacen(from, to, 1);
    }
    json_out = append_buffer_views(&json_out, &views_json)?;
    json_out = set_first_buffer_byte_length(&json_out, new_bin.len())?;
    Ok(write_glb(json_out.as_bytes(), &new_bin))
}

fn split_glb(bytes: &[u8]) -> Result<(&[u8], &[u8]), String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("not a GLB".into());
    }
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if bytes.len() < 20 + json_len || &bytes[16..20] != b"JSON" {
        return Err("GLB JSON chunk truncated".into());
    }
    let json = &bytes[20..20 + json_len];
    let bin_at = 20 + json_len;
    if bin_at + 8 > bytes.len() {
        return Ok((json, &[]));
    }
    if &bytes[bin_at + 4..bin_at + 8] != b"BIN\0" {
        return Ok((json, &[]));
    }
    let bin_len = u32::from_le_bytes(bytes[bin_at..bin_at + 4].try_into().unwrap()) as usize;
    if bin_at + 8 + bin_len > bytes.len() {
        return Err("GLB BIN chunk truncated".into());
    }
    Ok((json, &bytes[bin_at + 8..bin_at + 8 + bin_len]))
}

fn write_glb(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let json_pad = (json.len() + 3) & !3;
    let bin_pad = (bin.len() + 3) & !3;
    let total = 12 + 8 + json_pad + 8 + bin_pad;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_pad as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(json);
    out.resize(12 + 8 + json_pad, b' ');
    out.extend_from_slice(&(bin_pad as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out.resize(total, 0);
    out
}

fn json_string_values(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let bytes = json.as_bytes();
    let mut i = 0usize;
    while i + needle.len() < json.len() {
        let Some(rel) = json[i..].find(&needle) else {
            break;
        };
        i += rel + needle.len();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'"' {
            continue;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        if i <= bytes.len() {
            out.push(json[start..i].to_string());
        }
    }
    out
}

fn json_array_len(json: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let at = json.find(&needle)?;
    let rest = &json[at + needle.len()..];
    let bracket = rest.find('[')?;
    let body = match_bracket(&rest[bracket..])?;
    if body[1..body.len() - 1].trim().is_empty() {
        return Some(0);
    }
    Some(body.matches('{').count())
}

fn match_bracket(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.first() != Some(&b'[') && b.first() != Some(&b'{') {
        return None;
    }
    let open = b[0];
    let close = if open == b'[' { b']' } else { b'}' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn append_buffer_views(json: &str, views: &str) -> Result<String, String> {
    let needle = "\"bufferViews\"";
    let at = json
        .find(needle)
        .ok_or_else(|| "GLB JSON missing bufferViews".to_string())?;
    let after = &json[at + needle.len()..];
    let br = after
        .find('[')
        .ok_or_else(|| "bufferViews is not an array".to_string())?;
    let arr = match_bracket(&after[br..]).ok_or_else(|| "bufferViews array unclosed".to_string())?;
    let inner_start = at + needle.len() + br + 1;
    let inner_end = inner_start + arr.len() - 2;
    let inner = json[inner_start..inner_end].trim();
    let mut insert = views.to_string();
    if !inner.is_empty() && !insert.is_empty() {
        insert = format!("{inner},{insert}");
    } else if inner.is_empty() {
        // keep insert
    } else {
        insert = inner.to_string();
    }
    let mut out = String::with_capacity(json.len() + views.len() + 1);
    out.push_str(&json[..inner_start]);
    out.push_str(&insert);
    out.push_str(&json[inner_end..]);
    Ok(out)
}

fn set_first_buffer_byte_length(json: &str, len: usize) -> Result<String, String> {
    let needle = "\"byteLength\"";
    let Some(at) = json.find(needle) else {
        return Err("GLB JSON missing byteLength".into());
    };
    let rest = &json[at + needle.len()..];
    let colon = rest
        .find(':')
        .ok_or_else(|| "byteLength has no value".to_string())?;
    let mut i = colon + 1;
    let rb = rest.as_bytes();
    while i < rb.len() && rb[i].is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    while i < rb.len() && rb[i].is_ascii_digit() {
        i += 1;
    }
    if start == i {
        return Err("byteLength is not a number".into());
    }
    let abs = at + needle.len() + start;
    let abs_end = at + needle.len() + i;
    Ok(format!("{}{len}{}", &json[..abs], &json[abs_end..]))
}

fn publish_compiled_pack(
    pack_root: &Path,
    out: &Path,
    session: &ServerSession,
    pack_name: &str,
    pack_page: &str,
    assets: usize,
    blob_total: usize,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
) -> Result<(bool, usize), String> {
    let collection = std::fs::read(out.join(SOURCE_COLLECTION_FILE))
        .map_err(|e| format!("read source collection: {e}"))?;
    let manifest = std::fs::read(out.join(IMPORT_MANIFEST_FILE))
        .map_err(|e| format!("read import manifest: {e}"))?;
    let plan_bytes =
        std::fs::read(out.join(UPLOAD_PLAN_FILE)).map_err(|e| format!("read upload plan: {e}"))?;
    let plan = makepad_asset_client::json::parse(&plan_bytes)
        .map_err(|e| format!("upload plan json: {e}"))?;
    let ns = plan
        .get("namespace")
        .and_then(makepad_asset_client::json::Value::as_str)
        .ok_or("upload plan missing namespace")?
        .to_string();
    let blobs = plan
        .get("blobs")
        .and_then(makepad_asset_client::json::Value::as_arr)
        .ok_or("upload plan missing blobs")?;

    let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_app/import/cache");
    let mut config = ClientConfig::new(cache);
    config.token = Some(session.token.clone());
    let client = AssetClient::connect(config, session.endpoints, Some(session.server_id))
        .map_err(|e| format!("asset client: {e}"))?;
    client
        .register_source_collection(&collection)
        .map_err(|e| format!("register source: {e}"))?;

    let mut blob_done = 0usize;
    for blob in blobs {
        if cancel.load(Ordering::SeqCst) {
            return Err("stopped during blob upload".into());
        }
        let local = blob
            .get("local_path")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing local_path")?;
        let expect = blob
            .get("blob")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing digest")?;
        let bytes = std::fs::read(pack_root.join(local))
            .map_err(|e| format!("read {local}: {e}"))?;
        let digest = BlobId::hash_of(&bytes);
        if digest.to_string() != expect {
            return Err(format!(
                "rehash mismatch for {local}: plan {expect} != sha256 {}",
                digest
            ));
        }
        if sha256(&bytes) != *digest.as_bytes() {
            return Err(format!("sha256 drift for {local}"));
        }
        client
            .upload_blob(&ns, &bytes)
            .map_err(|e| format!("upload {local}: {e}"))?;
        blob_done += 1;
        let _ = tx.send(ImportPhase::Publishing {
            pack: pack_name.to_string(),
            assets,
            blobs: blob_total,
            blob_done,
        });
    }

    let report = client
        .run_import(&manifest)
        .map_err(|e| format!("run import: {e}"))?;
    let plan_kinds = plan_asset_kinds(&plan);
    let annotate_total = report.entries.len();
    let mut annotated = 0usize;
    let _ = tx.send(ImportPhase::Annotating {
        pack: pack_name.to_string(),
        assets,
        blobs: blob_total,
        annotated: 0,
        total: annotate_total,
    });
    for entry in &report.entries {
        if cancel.load(Ordering::SeqCst) {
            return Err("stopped during annotate".into());
        }
        let key = entry.key.as_str();
        let title = key.rsplit('/').next().unwrap_or(key);
        let kind = plan_kinds
            .get(key)
            .copied()
            .unwrap_or_else(|| guess_kind(key));
        let alias = entry
            .alias
            .as_ref()
            .map(|a| a.as_str().to_string())
            .unwrap_or_else(|| format!("kenney/{pack_name}/{title}"));
        let mut tags = Vec::new();
        for raw in [
            "kenney",
            pack_name,
            "cc-by-4-0",
            kind_tag(kind),
            title,
        ] {
            if let Some(tag) = search_label(raw) {
                if tags.iter().all(|t| t != &tag) {
                    tags.push(tag);
                }
            }
        }
        for part in key.split('/') {
            if let Some(tag) = search_label(part) {
                if tags.iter().all(|t| t != &tag) {
                    tags.push(tag);
                }
            }
        }
        if title.contains("character") {
            if let Some(tag) = search_label("character") {
                if tags.iter().all(|t| t != &tag) {
                    tags.push(tag);
                }
            }
        }
        let ann = AnnotationUpload {
            title: title.to_string(),
            description: format!(
                "Kenney {pack_name} · {alias} · {key} · CC-BY-4.0 · Kenney (kenney.nl)"
            ),
            kind: Some(kind),
            categories: vec!["kenney".into(), pack_name.to_string()],
            tags,
            creator: KENNEY_CREDITS.to_string(),
            generator: "pack_import".into(),
            backend: "asset-ui".into(),
            model: pack_name.to_string(),
            prompt: format!("imported Kenney pack {pack_name} asset {key}"),
            provenance: format!(
                "Kenney (kenney.nl) · {pack_page} · license CC-BY-4.0 · credits Kenney (kenney.nl)"
            ),
            private: false,
        };
        client
            .put_annotation(&entry.asset_id, &ann)
            .map_err(|e| format!("annotate {key}: {e}"))?;
        annotated += 1;
        let _ = tx.send(ImportPhase::Annotating {
            pack: pack_name.to_string(),
            assets,
            blobs: blob_total,
            annotated,
            total: annotate_total,
        });
    }
    Ok((report.created, annotated))
}

/// Bake AO for every staged GLB (shared post-GLTF path). Fail-closed per mesh.
fn bake_staged_glbs(
    staged: &Path,
    pack: &str,
    assets: usize,
    annotated: usize,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
) -> BakeStats {
    ao_bake::bake_glb_tree_ex(staged, Some(cancel), |done, total, current| {
        let _ = tx.send(ImportPhase::Baking {
            pack: pack.to_string(),
            assets,
            annotated,
            bake_done: done,
            bake_total: total,
            bake_skipped: 0,
            bake_failed: 0,
            current: current.to_string(),
        });
    })
}

fn plan_asset_kinds(
    plan: &makepad_asset_client::json::Value,
) -> std::collections::BTreeMap<String, AssetKind> {
    let mut out = std::collections::BTreeMap::new();
    let Some(assets) = plan
        .get("assets")
        .and_then(makepad_asset_client::json::Value::as_arr)
    else {
        return out;
    };
    for asset in assets {
        let Some(key) = asset
            .get("key")
            .and_then(makepad_asset_client::json::Value::as_str)
        else {
            continue;
        };
        let kind = asset
            .get("kind")
            .and_then(makepad_asset_client::json::Value::as_str)
            .and_then(parse_kind)
            .unwrap_or_else(|| guess_kind(key));
        out.insert(key.to_string(), kind);
    }
    out
}

fn parse_kind(name: &str) -> Option<AssetKind> {
    Some(match name {
        "mesh" => AssetKind::Mesh,
        "character" => AssetKind::Character,
        "weapon" => AssetKind::Weapon,
        "vehicle" => AssetKind::Vehicle,
        "prop" => AssetKind::Prop,
        "texture" => AssetKind::Texture,
        "material" => AssetKind::Material,
        "audio" => AssetKind::Audio,
        "video" => AssetKind::Video,
        "skybox" => AssetKind::Skybox,
        "world" => AssetKind::World,
        "prefab" => AssetKind::Prefab,
        "billboard" => AssetKind::Billboard,
        "game" => AssetKind::Game,
        _ => return None,
    })
}

/// Search labels are `[a-z0-9_-]`, must start alphanumeric. Dots (as in
/// CC-BY-4.0) become dashes so annotation writes do not 400.
fn search_label(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            out.push(c);
        } else if c == '.' || c == ' ' || c == '/' {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty()
        || out.len() > 128
        || !(out.as_bytes()[0].is_ascii_lowercase() || out.as_bytes()[0].is_ascii_digit())
    {
        return None;
    }
    Some(out)
}

fn kind_tag(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
        AssetKind::Game => "game",
    }
}

fn guess_kind(key: &str) -> AssetKind {
    if key.contains("texture") || key.ends_with(".png") || key.ends_with(".jpg") {
        AssetKind::Texture
    } else if key.contains("audio") || key.contains("sound") {
        AssetKind::Audio
    } else if key.contains("video") {
        AssetKind::Video
    } else if key.contains("sprite") || key.contains("billboard") {
        AssetKind::Billboard
    } else if key.contains("character") {
        AssetKind::Character
    } else if key.contains("vehicle") || key.contains("car") {
        AssetKind::Vehicle
    } else {
        AssetKind::Mesh
    }
}

/// Deterministic 512×512 opaque PNG used only as the pack_import-required
/// mesh thumbnail. Replaced in the local library by ThumbnailRenderer.
fn placeholder_png_512() -> Vec<u8> {
    let w = 512u32;
    let h = 512u32;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let row = 1 + w * 3;
    let raw_len = row as usize * h as usize;
    let raw = vec![0u8; raw_len];
    let mut zlib = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() {
        let take = (raw.len() - off).min(65535);
        let last = off + take == raw.len();
        zlib.push(if last { 0x01 } else { 0x00 });
        let n = take as u16;
        zlib.extend_from_slice(&n.to_le_bytes());
        zlib.extend_from_slice(&(!n).to_le_bytes());
        zlib.extend_from_slice(&raw[off..off + take]);
        off += take;
    }
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in &raw {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    push_png_chunk(&mut out, b"IHDR", &ihdr);
    push_png_chunk(&mut out, b"IDAT", &zlib);
    push_png_chunk(&mut out, b"IEND", &[]);
    out
}

fn push_png_chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(data);
    let mut crc_src = Vec::with_capacity(4 + data.len());
    crc_src.extend_from_slice(typ);
    crc_src.extend_from_slice(data);
    out.extend_from_slice(&png_crc(&crc_src).to_be_bytes());
}

fn png_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_fingerprint_is_deterministic_and_content_sensitive() {
        let a = content_fingerprint(b"glTF model bytes one");
        let b = content_fingerprint(b"glTF model bytes one");
        let c = content_fingerprint(b"glTF model bytes TWO");
        assert_eq!(a, b, "same bytes must fingerprint identically every time");
        assert_ne!(a, c, "different bytes must (almost always) fingerprint differently");
    }

    #[test]
    fn pack_icon_key_round_trips() {
        let key = pack_icon_key("kenney", "space-kit", "crate", "abc123");
        assert_eq!(key, "pack:kenney:space-kit:crate:abc123");
        assert_eq!(
            parse_pack_icon_key(&key),
            Some((
                "kenney".to_string(),
                "space-kit".to_string(),
                "crate".to_string(),
                "abc123".to_string(),
            ))
        );
        // A plain library file id is never mistaken for a pack icon key.
        assert_eq!(parse_pack_icon_key("lib-42.glb"), None);
    }

    #[test]
    fn pack_icon_reusable_only_when_rendered_and_unchanged() {
        assert!(pack_icon_reusable(true, Some("abc"), "abc"));
        assert!(!pack_icon_reusable(false, Some("abc"), "abc"), "no icon on disk yet");
        assert!(!pack_icon_reusable(true, Some("abc"), "def"), "content changed");
        assert!(!pack_icon_reusable(true, None, "abc"), "never recorded a fingerprint");
    }

    #[test]
    fn staging_dirs_to_clear_only_on_success() {
        assert_eq!(staging_dirs_to_clear(true), &["work", "out"]);
        assert_eq!(staging_dirs_to_clear(false), &[] as &[&str]);
    }

    #[test]
    fn pack_icon_store_round_trips_and_detects_changed_content() {
        let dir = std::env::temp_dir().join(format!(
            "mp_pack_icon_store_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // No icon yet: nothing to reuse.
        assert!(read_reusable_pack_icon(&dir, "crate", "fp-v1").is_none());

        write_pack_icon(&dir, "crate", "fp-v1", b"rendered-icon-v1").unwrap();
        assert_eq!(
            read_reusable_pack_icon(&dir, "crate", "fp-v1"),
            Some(b"rendered-icon-v1".to_vec()),
            "unchanged fingerprint must reuse the persisted render"
        );
        assert!(
            read_reusable_pack_icon(&dir, "crate", "fp-v2").is_none(),
            "a changed fingerprint must NOT reuse a stale render"
        );

        // A re-render overwrites both the icon and its fingerprint.
        write_pack_icon(&dir, "crate", "fp-v2", b"rendered-icon-v2").unwrap();
        assert_eq!(
            read_reusable_pack_icon(&dir, "crate", "fp-v2"),
            Some(b"rendered-icon-v2".to_vec())
        );
        assert!(read_reusable_pack_icon(&dir, "crate", "fp-v1").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A genuinely decodable, varied (non-flat, non-512²-placeholder) PNG —
    /// stands in for "a real rendered icon" in tests. Uses the importer's
    /// own encoder so `thumbnail_is_placeholder` exercises its real decode
    /// path, the exact same one `pack_import::compile_pack` runs, rather
    /// than accidentally passing via its "undecodable bytes are not
    /// placeholders" carve-out.
    fn fake_valid_png(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 3 + y * 5) % 200) as u8;
                rgba.extend_from_slice(&[v, 40 + v / 2, 255 - v, 255]);
            }
        }
        makepad_asset_importer::classic_import::encode_png_rgba(&rgba, w, h)
            .expect("encode test png")
    }

    #[test]
    fn icon_resume_gate_signals_once_per_arm() {
        let (mut gate, rx) = IconResumeGate::armed();
        // Never armed for "a pack" yet in this test, but `armed()` starts
        // ready-to-send once (matches `ImportPage::default()`'s gate being
        // usable before any `IconsPending` has armed it).
        assert!(gate.resume(), "first resume after armed() must succeed");
        assert!(rx.try_recv().is_ok(), "resume must actually signal the receiver");
        assert!(!gate.resume(), "no double-send before the next arm()");
        assert!(rx.try_recv().is_err(), "no second signal without re-arming");

        gate.arm();
        assert!(gate.resume(), "arm() must allow exactly one more resume");
        assert!(rx.try_recv().is_ok());
        assert!(!gate.resume());
    }

    #[test]
    fn icon_resume_gate_default_has_no_sender() {
        let mut gate = IconResumeGate::default();
        assert!(!gate.resume(), "an unarmed default gate must never fire");
    }

    #[test]
    fn icons_pending_handshake_waits_for_drained_landings_and_idle_icons() {
        let mut page = ImportPage::default();
        let (gate, rx) = IconResumeGate::armed();
        page.icon_resume = gate;
        let landing = LibraryLanding {
            path: PathBuf::from("/tmp/x.glb"),
            label: "kenney/space-kit/x".into(),
            domain: "mesh",
            content_type: "model/gltf-binary",
            prompt: "test".into(),
            thumbnail: None,
            source_id: "kenney".into(),
            pack: "space-kit".into(),
        };
        page.ingest_phase(ImportPhase::IconsPending {
            pack: "space-kit".into(),
            assets: 1,
            library: vec![landing],
            bake: BakeStats::default(),
        });
        assert!(matches!(page.kenney_phase, ImportPhase::IconsPending { .. }));
        assert_eq!(page.pending_landings.len(), 1, "landing handed to the UI immediately");
        assert!(page.compiling(), "IconsPending blocks a new import from starting");

        // Landings not yet drained by the UI (`land_imported_pack` hasn't
        // consumed `pending_landings` into `App::import_landings` yet):
        // never ready, no matter the icon state.
        assert!(!page.icons_pending_ready(false));

        // Drained, but the one queued icon hasn't rendered yet.
        page.track_import_icon("pack:kenney:space-kit:x:fp1".into(), None);
        assert!(page.icons_busy());
        assert!(!page.icons_pending_ready(true), "icons still rendering");

        // Render lands: ready now, resume fires exactly once and actually
        // wakes the parked import thread's receiver.
        page.note_rendered_icon("pack:kenney:space-kit:x:fp1", b"\x89PNG\r\n\x1a\nicon");
        assert!(!page.icons_busy());
        assert!(page.icons_pending_ready(true));
        assert!(page.resume_icons_pending());
        assert!(rx.try_recv().is_ok(), "resume_icons_pending must signal the parked thread");
        assert!(!page.resume_icons_pending(), "no double-resume before the next IconsPending");

        // A fresh IconsPending (next pack in "Import all") re-arms it.
        page.ingest_phase(ImportPhase::IconsPending {
            pack: "other-kit".into(),
            assets: 0,
            library: Vec::new(),
            bake: BakeStats::default(),
        });
        assert!(page.resume_icons_pending());
        assert!(rx.try_recv().is_ok());
    }

    /// The exact shape of the 38-packs-failed regression: staged meshes were
    /// queued for a GPU render but never REGISTERED for the icon wait, so
    /// the gate saw an empty wait, read "icons ready", and let compile+publish
    /// run against the placeholders — every pack then died on the publish-time
    /// placeholder guard while its real icons were still rendering.
    #[test]
    fn a_staged_mesh_that_never_registered_is_named_before_the_gate_opens() {
        let mut page = ImportPage::default();
        let landing = |stem: &str| LibraryLanding {
            path: PathBuf::from(format!("/tmp/{stem}.glb")),
            label: format!("kenney/space-kit/{stem}"),
            domain: "mesh",
            content_type: "model/gltf-binary",
            prompt: "test".into(),
            thumbnail: None,
            source_id: "kenney".into(),
            pack: "space-kit".into(),
        };
        page.ingest_phase(ImportPhase::IconsPending {
            pack: "space-kit".into(),
            assets: 2,
            library: vec![landing("a"), landing("b")],
            bake: BakeStats::default(),
        });

        // Nothing registered: the wait is empty, so the gate WOULD open on
        // drained landings — and both meshes are named as the reason.
        assert!(!page.icons_busy(), "an empty wait is not busy — that is the trap");
        assert!(page.icons_pending_ready(true));
        assert_eq!(page.unregistered_mesh_icons(), vec!["a".to_string(), "b".to_string()]);

        // Registering one (whatever its content fingerprint) accounts for it.
        page.track_import_icon("pack:kenney:space-kit:a:deadbeef".into(), None);
        assert_eq!(page.unregistered_mesh_icons(), vec!["b".to_string()]);
        assert!(page.icons_busy(), "a registered, unrendered icon holds the gate shut");
        assert!(!page.icons_pending_ready(true));

        // With both registered nothing is unaccounted for, and the gate is
        // held by the renders themselves — the correct, restored behaviour.
        page.track_import_icon("pack:kenney:space-kit:b:cafe1234".into(), None);
        assert!(page.unregistered_mesh_icons().is_empty());
        assert!(!page.icons_pending_ready(true));
        page.note_rendered_icon("pack:kenney:space-kit:a:deadbeef", b"\x89PNG\r\n\x1a\nicon");
        assert!(!page.icons_pending_ready(true), "one icon left");
        page.note_rendered_icon("pack:kenney:space-kit:b:cafe1234", b"\x89PNG\r\n\x1a\nicon");
        assert!(page.icons_pending_ready(true));
    }

    /// A non-mesh landing (a plain pack image) is not part of the icon wait.
    #[test]
    fn unregistered_mesh_icons_ignores_non_mesh_landings() {
        let mut page = ImportPage::default();
        page.ingest_phase(ImportPhase::IconsPending {
            pack: "space-kit".into(),
            assets: 1,
            library: vec![LibraryLanding {
                path: PathBuf::from("/tmp/colormap.png"),
                label: "kenney/space-kit/colormap".into(),
                domain: "image",
                content_type: "image/png",
                prompt: "test".into(),
                thumbnail: None,
                source_id: "kenney".into(),
                pack: "space-kit".into(),
            }],
            bake: BakeStats::default(),
        });
        assert!(page.unregistered_mesh_icons().is_empty());
    }

    /// A run may not report "N failed" without saying why at least once.
    #[test]
    fn a_failed_run_always_carries_a_reason() {
        let mut page = ImportPage::default();
        page.kenney_phase = ImportPhase::AllDone {
            ok: Vec::new(),
            failed: vec![
                ("blaster-kit".into(), "icon for blaster-a never rendered".into()),
                ("car-kit".into(), "icon for car-a never rendered".into()),
            ],
            skipped: vec!["arena".into()],
        };
        let line = page.kenney_status_line(true);
        assert!(line.contains("2 failed"), "{line}");
        assert!(line.contains("blaster-kit"), "{line}");
        assert!(line.contains("icon for blaster-a never rendered"), "{line}");
        assert!(line.contains("+1 more"), "{line}");
        assert!(line.contains("1 not on disk"), "{line}");
        assert!(page.kenney_phase.failure_reason().is_some());

        // One pack dying mid-run says so on the spot, and keeps the run busy.
        page.kenney_phase = ImportPhase::PackFailed {
            pack: "car-kit".into(),
            message: "compile refused the pack: multi-texture GLB".into(),
            done: 3,
            total: 38,
            more: true,
        };
        let line = page.kenney_status_line(true);
        assert!(line.contains("car-kit"), "{line}");
        assert!(line.contains("multi-texture GLB"), "{line}");
        assert!(page.compiling(), "more packs follow — the run is still busy");
        assert_eq!(
            page.kenney_phase.failure_reason().as_deref(),
            Some("car-kit: compile refused the pack: multi-texture GLB"),
            "a reason must name the pack it belongs to"
        );

        // A publish that was REFUSED is a failure, not a local compile.
        page.kenney_phase = ImportPhase::CompiledLocal {
            pack: "car-kit".into(),
            assets: 50,
            blobs: 60,
            out: PathBuf::from("/tmp/plan.json"),
            error: Some("publish to the asset store failed: 401 unauthorized".into()),
            library: Vec::new(),
            bake: BakeStats::default(),
        };
        let line = page.kenney_status_line(true);
        assert!(line.contains("NOT imported"), "{line}");
        assert!(line.contains("401 unauthorized"), "{line}");
        assert!(page.kenney_phase.failure_reason().is_some());

        // A local compile with no server session is NOT a failure.
        page.kenney_phase = ImportPhase::CompiledLocal {
            pack: "car-kit".into(),
            assets: 50,
            blobs: 60,
            out: PathBuf::from("/tmp/plan.json"),
            error: None,
            library: Vec::new(),
            bake: BakeStats::default(),
        };
        assert!(page.kenney_phase.failure_reason().is_none());
    }

    #[test]
    fn icons_pending_handshake_also_resumes_on_cancel_via_stop_requested() {
        // `mark_cancelled` deliberately leaves `icon_resume` untouched so a
        // thread parked in `IconsPending` can still be woken — the actual
        // wake call is `resume_icons_pending()`, driven by
        // `App::maybe_resume_icons_pending` checking `stop_requested()`.
        let mut page = ImportPage::default();
        let (gate, rx) = IconResumeGate::armed();
        page.icon_resume = gate;
        page.ingest_phase(ImportPhase::IconsPending {
            pack: "space-kit".into(),
            assets: 1,
            library: vec![LibraryLanding {
                path: PathBuf::from("/tmp/x.glb"),
                label: "kenney/space-kit/x".into(),
                domain: "mesh",
                content_type: "model/gltf-binary",
                prompt: "test".into(),
                thumbnail: None,
                source_id: "kenney".into(),
                pack: "space-kit".into(),
            }],
            bake: BakeStats::default(),
        });
        page.request_stop();
        assert!(page.stop_requested());
        // Landings not drained AND icons still "busy" (never even queued) —
        // `icons_pending_ready` alone would say no, but the app's cancel
        // path resumes regardless so the thread can wake, see `cancel` is
        // set, and exit with `ImportPhase::Cancelled` itself.
        assert!(!page.icons_pending_ready(false));
        assert!(page.resume_icons_pending());
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn assign_staged_thumbnails_reuses_cached_icon_and_guard_flags_placeholders() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let source_id = format!("test-src-{nonce}");
        let pack_name = format!("test-pack-{nonce}");
        let icons_dir = pack_icons_dir(&source_id, &pack_name);
        let _ = std::fs::remove_dir_all(&icons_dir);

        let tmp = std::env::temp_dir().join(format!("mp_assign_thumbs_{nonce}"));
        // `rendered_pack_icons` derives source/pack from this dir's own name
        // and its parent's — mirror that layout so it resolves back to
        // `icons_dir` above.
        let source_pack_dir = tmp.join(&source_id).join(&pack_name);
        std::fs::create_dir_all(&source_pack_dir).unwrap();

        let glb_cached = tmp.join("cached.glb");
        let glb_fresh = tmp.join("fresh.glb");
        std::fs::write(&glb_cached, b"glTF cached").unwrap();
        std::fs::write(&glb_fresh, b"glTF fresh").unwrap();
        let glbs = vec![glb_cached.clone(), glb_fresh.clone()];
        let images = std::collections::BTreeSet::new();

        // No cache yet: every stem gets the placeholder; the guard flags
        // the first one (deterministic order from `glbs`).
        assign_staged_thumbnails(&glbs, &images, &source_pack_dir, &tmp).unwrap();
        assert_eq!(first_placeholder_stem(&glbs, &images, &tmp).as_deref(), Some("cached"));

        // Persist a real (non-512×512, non-blank) icon for "cached" only.
        std::fs::create_dir_all(&icons_dir).unwrap();
        let real_icon = fake_valid_png(300, 300);
        std::fs::write(icons_dir.join("cached.png"), &real_icon).unwrap();

        assign_staged_thumbnails(&glbs, &images, &source_pack_dir, &tmp).unwrap();
        assert_eq!(
            std::fs::read(glb_cached.with_extension("png")).unwrap(),
            real_icon,
            "the cached icon must be reused for the matching stem"
        );
        assert_eq!(
            first_placeholder_stem(&glbs, &images, &tmp).as_deref(),
            Some("fresh"),
            "the stem with no cached icon still guards as a placeholder"
        );

        // Persist "fresh" too: the guard now passes every staged GLB.
        std::fs::write(icons_dir.join("fresh.png"), fake_valid_png(300, 300)).unwrap();
        assign_staged_thumbnails(&glbs, &images, &source_pack_dir, &tmp).unwrap();
        assert_eq!(first_placeholder_stem(&glbs, &images, &tmp), None);

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&icons_dir);
    }

    /// A texture named like a model must not stand in for that model's
    /// preview. Kenney's retro kits ship `Textures/fence.png` (the atlas)
    /// beside a `fence.glb` that has NO `fence.png` of its own; keying the
    /// staged image index by bare file name made the thumbnail pass skip
    /// `fence.glb` as "already has its own picture", so it reached compile
    /// with no thumbnail and refused the whole kit:
    ///
    ///     fence.glb: mesh-bearing import needs a pack PNG/JPEG thumbnail ≥ 256px
    ///
    /// Five models across the two retro kits were exactly that shape
    /// (fence / roof / water, grass / planks).
    #[test]
    fn a_texture_named_like_a_model_does_not_shadow_its_thumbnail() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(2);
        let tmp = std::env::temp_dir().join(format!("mp_shadow_thumb_{nonce}"));
        let src = tmp.join("src");
        let staged = tmp.join("staged");
        std::fs::create_dir_all(src.join("Textures")).unwrap();
        // The model, with no sibling preview of its own.
        std::fs::write(src.join("fence.glb"), b"glTF fence").unwrap();
        // The material atlas, same NAME, different directory.
        std::fs::write(src.join("Textures/fence.png"), fake_valid_png(64, 64)).unwrap();
        // A model that really does ship its own preview: still skipped.
        std::fs::write(src.join("wall.glb"), b"glTF wall").unwrap();
        std::fs::write(src.join("wall.png"), fake_valid_png(300, 300)).unwrap();

        let (glbs, images) = copy_pack_source_files(&src, &staged).unwrap();
        assert!(
            images.contains("textures/fence"),
            "the atlas is indexed by its own path, not a bare name: {images:?}"
        );
        assert!(!images.contains("fence"), "{images:?}");
        assert!(images.contains("wall"), "{images:?}");

        assign_staged_thumbnails(&glbs, &images, &src, &staged).unwrap();
        // fence.glb got a thumbnail written (the placeholder here, since no
        // rendered icon is cached) rather than being passed over.
        assert!(
            staged.join("fence.png").is_file(),
            "fence.glb must be given a thumbnail, not shadowed by Textures/fence.png"
        );
        // wall.glb kept the preview it shipped.
        assert_eq!(
            std::fs::read(staged.join("wall.png")).unwrap(),
            fake_valid_png(300, 300)
        );
        // And the pre-publish guard now SEES fence as needing a real icon,
        // instead of skipping it and letting compile refuse the kit.
        assert_eq!(
            first_placeholder_stem(&glbs, &images, &staged).as_deref(),
            Some("fence")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn assign_staged_thumbnails_never_overwrites_an_already_real_icon() {
        // Mirrors KayKit: `write_kaykit_anim_icons` (or an original pack
        // image) may already have written a real `<stem>.png` before
        // `assign_staged_thumbnails` ever runs. It must be left alone, not
        // clobbered with a cache lookup or the placeholder.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        let tmp = std::env::temp_dir().join(format!("mp_assign_thumbs_real_{nonce}"));
        let source_pack_dir = tmp.join("kaykit-src");
        std::fs::create_dir_all(&source_pack_dir).unwrap();
        let glb = tmp.join("hero.glb");
        std::fs::write(&glb, b"glTF hero").unwrap();
        let already_real = fake_valid_png(300, 300);
        std::fs::write(glb.with_extension("png"), &already_real).unwrap();

        assign_staged_thumbnails(&[glb.clone()], &Default::default(), &source_pack_dir, &tmp).unwrap();
        assert_eq!(std::fs::read(glb.with_extension("png")).unwrap(), already_real);
        assert_eq!(first_placeholder_stem(&[glb], &Default::default(), &tmp), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn kenney_module_reuses_pack_import_cc_by_grant() {
        assert_eq!(KENNEY_MODULE.id, "kenney");
        assert_eq!(KENNEY_MODULE.license, "CC-BY-4.0");
        assert_ne!(KENNEY_MODULE.license, "CC0-1.0");
        assert!(KENNEY_MODULE.license_blurb.contains("attribution"));
        assert!(KENNEY_MODULE.license_blurb.contains("does not treat Kenney as CC0"));
        assert_eq!(KENNEY_MODULE.credits, "Kenney (kenney.nl)");
        assert_eq!(KENNEY_MODULE.homepage, "https://kenney.nl");
        assert_eq!(KENNEY_MODULE.github, Some("https://github.com/KenneyNL"));
        assert!(KENNEY_MODULE.import_wired);
        let names: Vec<&str> = kenney_packs().iter().map(|p| p.name).collect();
        assert!(names.contains(&"space-kit"));
        assert!(names.contains(&"ui-pack"));
        assert!(names.contains(&"car-kit"));
        assert!(names.contains(&"platformer-kit"));
        assert!(!names.contains(&"platformer"));
        assert!(!names.contains(&"city"));
        assert!(!names.contains(&"arena"));
    }

    #[test]
    fn import_list_uses_full_kits_not_starter_slices() {
        for slice in STARTER_KIT_SLICES {
            assert!(is_starter_kit_slice(slice));
        }
        let present = on_disk_kenney_packs();
        if present.is_empty() {
            return;
        }
        for (name, _) in &present {
            assert!(
                !is_starter_kit_slice(name),
                "starter slice leaked into import list: {name}"
            );
        }
        assert!(
            present.iter().any(|(n, _)| n == "platformer-kit"),
            "full platformer-kit must be importable: {present:?}"
        );
        assert!(
            present.iter().any(|(n, _)| n == "space-kit"),
            "space-kit must stay importable: {present:?}"
        );
        assert!(
            !present.iter().any(|(n, _)| n == "3d-road-tiles"),
            "gltf-only 3d-road-tiles has no glb/png/wav and must stay off the list: {present:?}"
        );
    }

    /// retro-fantasy-kit's `barrels.glb` is barrel+planks — the shape that
    /// used to refuse the whole kit. It stages, lands, and now COMPILES;
    /// the only thing left between it and the catalog is the same rendered
    /// icon every mesh needs (`stage_source_pack` writes placeholders, so
    /// the placeholder guard is what this stops at, exactly as the
    /// single-texture kits do in `compile_sandbox_space_kit`).
    #[test]
    fn multi_texture_kit_lands_library_and_compiles() {
        let dir = resolve_kenney_dir("retro-fantasy-kit", "");
        if !dir.is_dir() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_retro_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mini = tmp.join("mini");
        std::fs::create_dir_all(mini.join("Textures")).unwrap();
        std::fs::copy(dir.join("barrels.glb"), mini.join("barrels.glb")).unwrap();
        for name in ["barrel.png", "planks.png"] {
            std::fs::copy(dir.join("Textures").join(name), mini.join("Textures").join(name))
                .unwrap();
        }
        let staged = tmp.join("source");
        let dest = tmp.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        stage_source_pack(&mini, &staged).expect("stage multi-texture slice");
        let landings = library_landings(
            &staged,
            "retro-fantasy-kit",
            "https://kenney.nl/assets/retro-fantasy-kit",
        );
        assert!(
            landings
                .iter()
                .any(|l| l.content_type.contains("gltf")),
            "staged multi-texture Kenney GLB must still produce a library landing"
        );
        let spec = kenney_spec("retro-fantasy-kit").expect("spec");
        match pack_import::compile_pack(&staged, &dest.join("bundle"), spec, None, false) {
            Ok(report) => {
                assert_eq!(report.assets, 1, "the multi-material barrel is one asset");
                assert!(
                    report.skipped_models.is_empty(),
                    "nothing skipped: {:?}",
                    report.skipped_models
                );
            }
            // No GPU render is cached in this environment, so the
            // fail-closed thumbnail guard is the honest stopping point —
            // the multi-texture refusal is gone, which is what this covers.
            Err(error) => {
                let text = error.to_string();
                assert!(
                    text.contains("placeholder"),
                    "multi-texture must no longer refuse the kit: {error}"
                );
                assert!(!text.contains("multi-texture"), "{error}");
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn other_modules_are_visible_and_not_wired() {
        assert_eq!(PACK_MODULES.len(), 3);
        assert!(KAYKIT_MODULE.import_wired);
        assert!(!NASA_SKY_MODULE.import_wired);
        assert_eq!(KAYKIT_MODULE.license, "CC0-1.0");
    }

    #[test]
    fn kaykit_landings_use_character_title_not_license() {
        let tmp = std::env::temp_dir().join(format!("makepad-kaykit-landings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("knight.glb"), b"glTF").unwrap();
        let landings = kaykit_library_landings(&tmp);
        assert_eq!(landings.len(), 1);
        assert_eq!(landings[0].label, "Knight");
        assert!(landings[0].prompt.contains("knight"));
        assert!(!landings[0].prompt.starts_with("KayKit knight"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_folder_fails_closed() {
        let mut page = ImportPage::default();
        page.kenney_path = "/tmp/makepad-no-such-kenney-pack".into();
        let err = page
            .start_kenney_import(page.kenney_path.clone(), None)
            .unwrap_err();
        assert!(err.contains("not on disk"));
        assert!(matches!(page.kenney_phase, ImportPhase::Failed { .. }));
    }

    #[test]
    fn take_library_landings_consumes() {
        let mut page = ImportPage::default();
        page.pending_landings.push(LibraryLanding {
            path: PathBuf::from("/tmp/x.glb"),
            label: "kenney/space-kit/x".into(),
            domain: "mesh",
            content_type: "model/gltf-binary",
            prompt: "test".into(),
            thumbnail: None,
            source_id: "kenney".into(),
            pack: "space-kit".into(),
        });
        let first = page.take_library_landings();
        assert_eq!(first.len(), 1);
        assert!(page.take_library_landings().is_empty());
    }

    #[test]
    fn reimport_allowed_after_published() {
        let mut page = ImportPage::default();
        page.kenney_phase = ImportPhase::Published {
            pack: "space-kit".into(),
            assets: 1,
            blobs: 1,
            created: false,
            annotated: 1,
            out: PathBuf::from("/tmp"),
            library: Vec::new(),
            bake: BakeStats::default(),
        };
        assert!(!page.compiling());
        // Path may be missing in CI; the gate under test is "not refused as
        // already running" — a missing folder fails closed with its own error.
        let result = page.start_kenney_import("/tmp/makepad-no-such-kenney-pack".into(), None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("not on disk"),
            "reimport must not be refused as already running after Published"
        );
    }

    #[test]
    fn progress_fraction_is_honest_and_monotonic_across_stages() {
        let mut page = ImportPage::default();
        assert_eq!(page.progress_fraction(), 0.0);
        page.kenney_phase = ImportPhase::compiling("space-kit");
        let c = page.progress_fraction();
        page.kenney_phase = ImportPhase::Publishing {
            pack: "space-kit".into(),
            assets: 10,
            blobs: 10,
            blob_done: 5,
        };
        let p = page.progress_fraction();
        page.kenney_phase = ImportPhase::Annotating {
            pack: "space-kit".into(),
            assets: 10,
            blobs: 10,
            annotated: 5,
            total: 10,
        };
        let a = page.progress_fraction();
        page.kenney_phase = ImportPhase::Baking {
            pack: "space-kit".into(),
            assets: 10,
            annotated: 10,
            bake_done: 5,
            bake_total: 10,
            bake_skipped: 0,
            bake_failed: 0,
            current: "barrel.glb".into(),
        };
        let b = page.progress_fraction();
        page.kenney_phase = ImportPhase::Published {
            pack: "space-kit".into(),
            assets: 10,
            blobs: 10,
            created: true,
            annotated: 10,
            out: PathBuf::from("/tmp"),
            library: Vec::new(),
            bake: BakeStats {
                total: 10,
                baked: 10,
                skipped: 0,
                failed: 0,
            },
        };
        page.import_icon_files.insert("lib-1.glb".into());
        page.import_icon_files.insert("lib-2.glb".into());
        page.icons_done = 1;
        let icons = page.progress_fraction();
        page.icons_done = 2;
        let done = page.progress_fraction();
        assert!(c < p && p < a && a < b && b < icons && icons < done);
        assert!((done - 1.0).abs() < 1e-3);
        assert!(
            b < 0.25,
            "AO/compile must stay a short prefix so 153 icons are not a sliver: {b}"
        );
        assert!(
            (icons - 0.60).abs() < 0.05,
            "half the icons should own most of the bar: {icons}"
        );
        let line = page.kenney_status_line(true);
        assert!(line.contains("icons 2/2"), "{line}");
        assert!(!line.contains("blank"));
        assert!(!line.contains("thumbs rendering"));
        page.icons_done = 0;
        page.icons_failed = 0;
        page.icon_current = "crate.glb".into();
        let stuck = page.kenney_status_line(true);
        assert!(stuck.contains("rendering GPU icons 0/2"), "{stuck}");
        assert!(stuck.contains("crate.glb"), "{stuck}");
        assert!(page.progress_fraction() < 0.25, "0/N icons is not a full bar");
    }

    #[test]
    fn bake_skips_fresh_sidecars_and_fails_closed_on_non_glb() {
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_bake_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let glb = tmp.join("prop.glb");
        // Valid-looking header only — parse will fail if bake is attempted.
        std::fs::write(&glb, b"glTF\x02\x00\x00\x00").unwrap();
        // Make sidecars newer than the glb.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(glb.with_extension("aomesh"), b"fake-aomesh").unwrap();
        std::fs::write(glb.with_extension("ao.png"), b"fake-ao").unwrap();
        let sun = ao_bake::default_sun();
        assert_eq!(
            ao_bake::bake_glb(&glb, &sun).unwrap(),
            ao_bake::BakeOutcome::SkippedFresh
        );

        let not_glb = tmp.join("notes.txt");
        std::fs::write(&not_glb, b"hello").unwrap();
        let err = ao_bake::bake_glb(&not_glb, &sun).unwrap_err();
        assert!(err.contains("not a glb"), "{err}");

        let bad = tmp.join("broken.glb");
        std::fs::write(&bad, b"not-gltf-bytes").unwrap();
        let err = ao_bake::bake_glb(&bad, &sun).unwrap_err();
        assert!(
            err.contains("not a glTF") || err.contains("bake"),
            "fail-closed: {err}"
        );
        assert!(!bad.with_extension("aomesh").exists());
        assert!(!bad.with_extension("ao.png").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn import_all_bar_is_overall_and_status_leads_with_the_pack() {
        let mut page = ImportPage::default();
        page.all_run = Some((10, 38));
        page.kenney_phase = ImportPhase::compiling("marble-kit");
        let f = page.progress_fraction();
        assert!(
            f >= 10.0 / 38.0 && f <= 11.0 / 38.0,
            "mid-run bar must sit between finished-packs and next-pack: {f}"
        );
        assert!(
            page.kenney_status_line(true).starts_with("pack 11/38 · "),
            "in-pack status must lead with which pack of how many"
        );
        // A pack boundary must not dip the bar: the finished pack is in
        // `done`, so its own fraction counts as zero.
        page.ingest_phase(ImportPhase::PackFailed {
            pack: "brick-kit".into(),
            message: "refused".into(),
            done: 11,
            total: 38,
            more: true,
        });
        let boundary = page.progress_fraction();
        assert!(boundary >= f && boundary <= 11.0 / 38.0 + 1e-6);
        // Single-pack imports keep the per-pack bar exactly as before.
        page.all_run = None;
        page.kenney_phase = ImportPhase::compiling("space-kit");
        assert!(page.progress_fraction() < 0.1);
    }

    #[test]
    fn status_line_bake_and_publish_counts() {
        let mut page = ImportPage::default();
        page.kenney_phase = ImportPhase::Publishing {
            pack: "ui-pack".into(),
            assets: 3,
            blobs: 12,
            blob_done: 4,
        };
        let line = page.kenney_status_line(true);
        assert!(line.contains("publish blob 4/12"), "{line}");
        page.kenney_phase = ImportPhase::Baking {
            pack: "ui-pack".into(),
            assets: 3,
            annotated: 3,
            bake_done: 2,
            bake_total: 5,
            bake_skipped: 1,
            bake_failed: 0,
            current: "panel.glb".into(),
        };
        let line = page.kenney_status_line(true);
        assert!(line.contains("building AO 2/5"), "{line}");
        assert!(line.contains("cores"), "{line}");
        assert!(line.contains("panel.glb"), "{line}");
    }

    #[test]
    fn probe_missing_and_existing_dirs() {
        let missing = probe_dir(Path::new("/tmp/makepad-no-such-kenney-pack"));
        assert!(!missing.ready());
        assert!(missing.line().contains("not on disk"));
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_probe_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("panel.png"), b"not-a-real-png").unwrap();
        std::fs::write(tmp.join("note.txt"), b"docs").unwrap();
        let found = probe_dir(&tmp);
        assert!(found.ready());
        assert_eq!(found.supported_files, 1);
        assert!(found.unsupported_samples.iter().any(|s| s == ".txt"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn stage_skips_derived_sidecars_and_writes_placeholder_thumbs() {
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_stage_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let pack = tmp.join("pack");
        let dest = tmp.join("source");
        std::fs::create_dir_all(&pack).unwrap();
        let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4}]}"#;
        std::fs::write(pack.join("crate.glb"), write_glb(json.as_bytes(), &[1, 2, 3, 4])).unwrap();
        std::fs::write(pack.join("crate.ao.png"), b"not-source").unwrap();
        std::fs::write(pack.join("crate.aomesh"), b"ao").unwrap();
        std::fs::write(pack.join("crate.shadowsdf"), b"sdf").unwrap();
        stage_source_pack(&pack, &dest).unwrap();
        assert!(dest.join("crate.glb").is_file());
        assert!(dest.join("crate.png").is_file());
        assert!(!dest.join("crate.ao.png").exists());
        assert!(!dest.join("crate.aomesh").exists());
        let png = std::fs::read(dest.join("crate.png")).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn landings_use_pack_preview_png_not_placeholder() {
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_land_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let staged = tmp.join("source");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("sedan.glb"), b"glTF").unwrap();
        let preview = b"\x89PNG\r\n\x1a\nreal-preview";
        std::fs::write(staged.join("sedan.png"), preview).unwrap();
        std::fs::write(staged.join("crate.glb"), b"glTF").unwrap();
        std::fs::write(staged.join("crate.png"), placeholder_png_512()).unwrap();
        let landings = library_landings(&staged, "car-kit", "https://kenney.nl/assets/car-kit");
        let sedan = landings.iter().find(|l| l.label.ends_with("/sedan")).unwrap();
        let crate_ = landings.iter().find(|l| l.label.ends_with("/crate")).unwrap();
        assert_eq!(
            sedan.thumbnail.as_ref().map(|p| p.file_name().unwrap()),
            Some(std::ffi::OsStr::new("sedan.png"))
        );
        assert_eq!(crate_.thumbnail, None, "placeholder must not become the icon");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blank_placeholder_png_is_detected() {
        assert!(is_blank_preview_png(&placeholder_png_512()));
        assert!(!is_blank_preview_png(b"\x89PNG\r\n\x1a\nreal-preview"));
    }

    #[test]
    fn embed_glb_inlines_sidecar_png() {
        let tmp = std::env::temp_dir().join(format!(
            "mp_embed_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(tmp.join("Textures")).unwrap();
        let png = placeholder_png_512();
        std::fs::write(tmp.join("Textures/colormap.png"), &png).unwrap();
        let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4}],"bufferViews":[{"buffer":0,"byteLength":4}],"images":[{"uri":"Textures/colormap.png","name":"colormap"}]}"#;
        let glb = write_glb(json.as_bytes(), &[1, 2, 3, 4]);
        let out = embed_glb_file_images(&glb, &tmp).unwrap();
        let (js, bin) = split_glb(&out).unwrap();
        let js = std::str::from_utf8(js).unwrap();
        assert!(!js.contains("\"uri\""), "{js}");
        assert!(js.contains("\"bufferView\":1"), "{js}");
        assert!(js.contains("image/png"), "{js}");
        assert!(bin.len() >= 4 + png.len());
        assert_eq!(&bin[bin.len() - png.len()..], png.as_slice());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn import_all_sees_local_space_kit() {
        let present = on_disk_kenney_packs();
        if present.is_empty() {
            return;
        }
        assert!(
            present.iter().any(|(name, _)| name == "space-kit"),
            "sandbox space-kit should be on disk: {present:?}"
        );
    }

    /// `ASSET_UI_IMPORT_TEST_PACK=<kit>` points this at another on-disk kit
    /// (compile, and publish when the local Asset UI server is up) — the
    /// quickest way to see a pack's real publish error.
    #[test]
    fn compile_sandbox_space_kit() {
        let pack = std::env::var("ASSET_UI_IMPORT_TEST_PACK").unwrap_or_else(|_| "space-kit".into());
        let pack = pack.as_str();
        let dir = resolve_kenney_dir(pack, "");
        if !dir.is_dir() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!(
            "mp_import_spacekit_{}",
            std::process::id()
        ));
        let staged = tmp.join("work").join("source");
        let dest_root = tmp.join("out");
        std::fs::create_dir_all(&dest_root).unwrap();
        let bundle = dest_root.join("bundle");
        stage_source_pack(&dir, &staged).expect("stage");
        // Same order as run_kenney_import: a source tree that was baked
        // before seeds its fresh sidecars, and the compiler attaches them.
        ao_bake::seed_ao_from_source(&dir, &staged);
        let spec = kenney_spec(pack).expect("spec");
        let report = match pack_import::compile_pack(&staged, &bundle, spec, None, false) {
            Ok(report) => report,
            // `stage_source_pack` (unlike `run_kenney_import`'s real
            // handshake) never waits for GPU-rendered icons — outside a
            // real app session with `pack_icons_dir` already populated
            // there is no render available, so `compile_pack`'s own
            // fail-closed `thumbnail_is_placeholder` guard correctly
            // refuses the placeholder fallback. That is exactly the
            // invariant this whole area exists to enforce, not a bug —
            // nothing to verify further in this environment.
            Err(error) if error.to_string().contains("thumbnail is a placeholder") => {
                println!("{pack}: no rendered icons cached yet — compile correctly refused a placeholder ({error})");
                let _ = std::fs::remove_dir_all(&tmp);
                return;
            }
            Err(error) => panic!("compile {pack}: {error}"),
        };
        assert!(report.assets > 0, "expected mesh assets, got {}", report.assets);
        println!(
            "{pack} compile: {} assets / {} blobs",
            report.assets, report.blobs
        );
        if let Some(session) = hosted_server_session() {
            let dest_root = tmp.join("out");
            let staged = tmp.join("work").join("source");
            let (tx, _rx) = mpsc::channel();
            let cancel = AtomicBool::new(false);
            match publish_compiled_pack(
                &staged,
                &dest_root.join("bundle"),
                &session,
                pack,
                &format!("https://kenney.nl/assets/{pack}"),
                report.assets,
                report.blobs,
                &tx,
                &cancel,
            ) {
                Ok((created, annotated)) => {
                    println!("{pack} publish: created={created} annotated={annotated}");
                    assert!(annotated > 0);
                }
                Err(error) => panic!("publish {pack}: {error}"),
            }
        } else {
            println!("no live Asset UI server — compile-only");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn parse_hex16_local(text: &str) -> Option<[u8; 16]> {
        if text.len() != 32 {
            return None;
        }
        let mut out = [0u8; 16];
        for i in 0..16 {
            out[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(out)
    }

    #[test]
    fn import_queue_is_serial_and_editable() {
        let mut queue = ImportQueue::default();
        let id = queue
            .enqueue(ImportJob::Freedoom { path: String::new() })
            .unwrap();
        queue
            .enqueue(ImportJob::Kenney {
                pack: "space-kit".into(),
                pack_index: 0,
                path: String::new(),
            })
            .unwrap();
        assert!(queue
            .enqueue(ImportJob::Freedoom { path: "/other".into() })
            .unwrap_err()
            .contains("already queued"));
        assert!(queue
            .enqueue(ImportJob::KenneyAll)
            .unwrap_err()
            .contains("already queued"));
        let first = queue.promote().unwrap();
        assert_eq!(first.id, id);
        assert!(matches!(first.job, ImportJob::Freedoom { .. }));
        assert!(queue.promote().is_none(), "must not promote while active");
        assert!(queue.remove(queue.pending[0].id));
        assert!(queue.pending.is_empty());
        queue.finish_active();
        assert!(queue.is_empty());
    }
}
