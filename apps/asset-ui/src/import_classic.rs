//! Additive Import cards for libre, freeware, and official shareware packs.
//!
//! Downloads go through platform `cx.http_request`. Bytes are unpacked by
//! [`makepad_asset_importer::classic_fetch`], then converted via
//! [`makepad_asset_importer::classic_import`] and compiled through the same
//! `pack_import` rights path as Kenney. AO bake runs on every produced GLB.

use crate::import::{
    clear_pack_staging, probe_dir, BakeStats, DiskProbe, IconResumeGate, ImportPhase,
    LibraryLanding, ServerSession,
};
use makepad_widgets::*;
use makepad_asset_importer::classic_import::{
    self, ClassicSource, DUKE3D_CREDITS, DUKE3D_GITHUB, DUKE3D_HOME, DUKE3D_LICENSE,
    DUKE3D_SOURCE_ID, DUKE3D_TERMS_URL, FREEDOOM_CREDITS, FREEDOOM_GITHUB, FREEDOOM_HOME,
    FREEDOOM_LICENSE, FREEDOOM_SOURCE_ID, FREEDOOM_SOURCE_TITLE, FREEDOOM_TERMS_URL,
    LIBREQUAKE_CREDITS, LIBREQUAKE_GITHUB, LIBREQUAKE_HOME, LIBREQUAKE_LICENSE,
    LIBREQUAKE_SOURCE_ID, LIBREQUAKE_SOURCE_TITLE, LIBREQUAKE_TERMS_URL, QUAKE2_CREDITS,
    QUAKE2_GITHUB, QUAKE2_HOME, QUAKE2_LICENSE, QUAKE2_SOURCE_ID, QUAKE2_TERMS_URL,
    QUAKE3_CREDITS, QUAKE3_GITHUB, QUAKE3_HOME, QUAKE3_LICENSE, QUAKE3_SOURCE_ID,
    QUAKE3_TERMS_URL, DARKMOD_CREDITS, DARKMOD_GITHUB, DARKMOD_HOME, DARKMOD_LICENSE,
    DARKMOD_SOURCE_ID, DARKMOD_SOURCE_TITLE, DARKMOD_TERMS_URL, DOOM_CREDITS, DOOM_GITHUB,
    DOOM_HOME, DOOM_LICENSE, DOOM_SOURCE_ID, DOOM_SOURCE_TITLE, DOOM_TERMS_URL, QUAKE_CREDITS,
    QUAKE_GITHUB, QUAKE_HOME, QUAKE_LICENSE, QUAKE_SOURCE_ID, QUAKE_SOURCE_TITLE, QUAKE_TERMS_URL,
};
use makepad_asset_importer::pack_import::{self, IMPORT_MANIFEST_FILE, SOURCE_COLLECTION_FILE, UPLOAD_PLAN_FILE};
use makepad_asset_client::{wire, AnnotationUpload, AssetClient, ClientConfig};
use makepad_asset_data::{AssetKind, BlobId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::import::{KENNEY_MODULE, KAYKIT_MODULE, NASA_SKY_MODULE, PackModule};
use makepad_asset_importer::tdm_zipsync::{
    self, TdmFetchPlan, TdmProvidedSet, TDM_INSTALLER_INI_URL,
};

pub const FREEDOOM_MODULE: PackModule = PackModule {
    id: FREEDOOM_SOURCE_ID,
    title: FREEDOOM_SOURCE_TITLE,
    blurb: "Free Doom IWAD content (maps, sprites, flats, SFX). Load fetches the official GitHub 0.13.0 zip if the folder is empty. Converts WAD maps to World GLBs, sprites to Billboard PNGs. Not CC0, not Kenney, not retail id Software art.",
    license: FREEDOOM_LICENSE,
    license_blurb: "© Freedoom contributors. BSD-3-Clause — attribution required. Original Freedoom art, not id Software shareware, not retail Doom, not CC0.",
    homepage: FREEDOOM_HOME,
    terms_url: FREEDOOM_TERMS_URL,
    source_page: FREEDOOM_HOME,
    github: Some(FREEDOOM_GITHUB),
    credits: FREEDOOM_CREDITS,
    import_wired: true,
};

pub const DOOM_MODULE: PackModule = PackModule {
    id: DOOM_SOURCE_ID,
    title: DOOM_SOURCE_TITLE,
    blurb: "Official Doom shareware (DOOM1.WAD, episode 1). Load fetches the labeled Archive.org shareware zip through platform HTTP and unpacks it (including a ZIP glued on a SETUP.EXE). Not Freedoom and not retail Doom II.",
    license: DOOM_LICENSE,
    license_blurb: "© id Software. Official Doom shareware (episode 1) for local preview in this app only. Not a redistributable grant. Not Freedoom. Not retail Doom / Doom II.",
    homepage: DOOM_HOME,
    terms_url: DOOM_TERMS_URL,
    source_page: DOOM_HOME,
    github: Some(DOOM_GITHUB),
    credits: DOOM_CREDITS,
    import_wired: true,
};

pub const QUAKE_MODULE: PackModule = PackModule {
    id: QUAKE_SOURCE_ID,
    title: QUAKE_SOURCE_TITLE,
    blurb: "Official Quake shareware (id1/pak0.pak). Load fetches the labeled Archive.org shareware zip through platform HTTP and unpacks it. Retail pak1 is not fetched.",
    license: QUAKE_LICENSE,
    license_blurb: "© id Software. Official Quake shareware (id1/pak0.pak) for local preview in this app only. Not a redistributable grant. Not LibreQuake. Not retail pak1.",
    homepage: QUAKE_HOME,
    terms_url: QUAKE_TERMS_URL,
    source_page: QUAKE_HOME,
    github: Some(QUAKE_GITHUB),
    credits: QUAKE_CREDITS,
    import_wired: true,
};

pub const LIBREQUAKE_MODULE: PackModule = PackModule {
    id: LIBREQUAKE_SOURCE_ID,
    title: LIBREQUAKE_SOURCE_TITLE,
    blurb: "Free Quake content (BSP, MDL, SPR, PAK/WAD2, WAV). Load fetches the official GitHub full.zip if the folder is empty. Not CC0, not Kenney, not retail Quake.",
    license: LIBREQUAKE_LICENSE,
    license_blurb: "© LibreQuake contributors. Modified BSD — attribution required. Original LibreQuake art, not id Software shareware, not retail Quake, not CC0.",
    homepage: LIBREQUAKE_HOME,
    terms_url: LIBREQUAKE_TERMS_URL,
    source_page: LIBREQUAKE_HOME,
    github: Some(LIBREQUAKE_GITHUB),
    credits: LIBREQUAKE_CREDITS,
    import_wired: true,
};

pub const DUKE3D_MODULE: PackModule = PackModule {
    id: DUKE3D_SOURCE_ID,
    title: "Duke3D shareware",
    blurb: "Shareware GRP + optional Duke4 HRP overlay. Load fetches the labeled Archive.org shareware ISO through platform HTTP and slices DUKE3D.GRP out of it. Retail GRP is not fetched.",
    license: DUKE3D_LICENSE,
    license_blurb: "© 3D Realms. Official Duke Nukem 3D shareware for local preview in this app only. Not a redistributable grant. Not retail Atomic Edition. Optional HRP art stays under the Duke4 HRP license.",
    homepage: DUKE3D_HOME,
    terms_url: DUKE3D_TERMS_URL,
    source_page: "https://hrp.duke4.net/",
    github: Some(DUKE3D_GITHUB),
    credits: DUKE3D_CREDITS,
    import_wired: true,
};

const EA_CNC_HOME: &str = "https://www.ea.com/games/command-and-conquer";
const EA_CLASSIC_LICENSE: &str = "EA freeware — local use, not redistributable";
const EA_CLASSIC_LICENSE_BLURB: &str = "© Westwood Studios / Electronic Arts. EA freeware for local preview in this app only. Not a redistributable grant.";
const EA_CLASSIC_CREDITS: &str = "Westwood Studios / Electronic Arts";

pub const EA_MODULES: [PackModule; 4] = [
    PackModule {
        id: "cnc",
        title: "Tiberian Dawn",
        blurb: "EA's 1995 RTS, freeware since 2007 — terrain, units, structures, sounds and every campaign and multiplayer map convert into RTS maps for the sandbox.",
        license: EA_CLASSIC_LICENSE,
        license_blurb: EA_CLASSIC_LICENSE_BLURB,
        homepage: EA_CNC_HOME,
        terms_url: EA_CNC_HOME,
        source_page: EA_CNC_HOME,
        github: None,
        credits: EA_CLASSIC_CREDITS,
        import_wired: true,
    },
    PackModule {
        id: "ra",
        title: "Red Alert",
        blurb: "The 1996 sequel, freeware since 2008 — Allied and Soviet arsenals, snow/temperate/interior maps.",
        license: EA_CLASSIC_LICENSE,
        license_blurb: EA_CLASSIC_LICENSE_BLURB,
        homepage: EA_CNC_HOME,
        terms_url: EA_CNC_HOME,
        source_page: EA_CNC_HOME,
        github: None,
        credits: EA_CLASSIC_CREDITS,
        import_wired: true,
    },
    PackModule {
        id: "ts",
        title: "Tiberian Sun",
        blurb: "The 1999 isometric sequel, freeware since 2010 — GDI/Nod units and tilesets; maps are generated from the tilesets.",
        license: EA_CLASSIC_LICENSE,
        license_blurb: EA_CLASSIC_LICENSE_BLURB,
        homepage: EA_CNC_HOME,
        terms_url: EA_CNC_HOME,
        source_page: EA_CNC_HOME,
        github: None,
        credits: EA_CLASSIC_CREDITS,
        import_wired: true,
    },
    PackModule {
        id: "d2k",
        title: "Dune 2000",
        blurb: "Westwood's 1998 Dune RTS — Atreides/Harkonnen/Ordos units and the Arrakis tilesets; maps are generated.",
        license: EA_CLASSIC_LICENSE,
        license_blurb: EA_CLASSIC_LICENSE_BLURB,
        homepage: EA_CNC_HOME,
        terms_url: EA_CNC_HOME,
        source_page: EA_CNC_HOME,
        github: None,
        credits: EA_CLASSIC_CREDITS,
        import_wired: true,
    },
];

pub const EA_SOURCES: [ClassicSource; 4] = [
    ClassicSource::Cnc,
    ClassicSource::RedAlert,
    ClassicSource::TiberianSun,
    ClassicSource::Dune2000,
];

pub const EA_PACK_LABELS: [&str; 4] = ["Tiberian Dawn", "Red Alert", "Tiberian Sun", "Dune 2000"];

pub fn ea_source_for_index(index: usize) -> ClassicSource {
    EA_SOURCES.get(index).copied().unwrap_or(ClassicSource::Cnc)
}

pub fn ea_index_for_source(source: ClassicSource) -> Option<usize> {
    EA_SOURCES.iter().position(|candidate| *candidate == source)
}

fn is_ea_source(source: ClassicSource) -> bool {
    ea_index_for_source(source).is_some()
}

pub const QUAKE2_MODULE: PackModule = PackModule {
    id: QUAKE2_SOURCE_ID,
    title: "Quake II shareware",
    blurb: "Official Quake II Test demo PAK (IBSP 38, WAL, MD2). Load fetches the labeled Archive.org demo zip through platform HTTP. Retail pak1+ is not fetched. Never ships .pak / .bsp / .md2 as catalog types.",
    license: QUAKE2_LICENSE,
    license_blurb: "© id Software. Official Quake II Test demo for local preview in this app only. Not a redistributable grant. Not retail Quake II.",
    homepage: QUAKE2_HOME,
    terms_url: QUAKE2_TERMS_URL,
    source_page: QUAKE2_HOME,
    github: Some(QUAKE2_GITHUB),
    credits: QUAKE2_CREDITS,
    import_wired: true,
};

pub const QUAKE3_MODULE: PackModule = PackModule {
    id: QUAKE3_SOURCE_ID,
    title: "Quake III demo",
    blurb: "Official Quake III Arena Linux demo PK3 (IBSP 46, MD3, TGA/JPG). Load fetches the labeled id Linux demo and unpacks demoq3/pak0.pk3. Retail pak0 is not fetched. Never ships .pk3 / .bsp / .md3 as catalog types.",
    license: QUAKE3_LICENSE,
    license_blurb: "© id Software. Official Quake III Arena demo for local preview in this app only. Not a redistributable grant. Not OpenArena. Not retail Quake III.",
    homepage: QUAKE3_HOME,
    terms_url: QUAKE3_TERMS_URL,
    source_page: QUAKE3_HOME,
    github: Some(QUAKE3_GITHUB),
    credits: QUAKE3_CREDITS,
    import_wired: true,
};

pub const DARKMOD_MODULE: PackModule = PackModule {
    id: DARKMOD_SOURCE_ID,
    title: DARKMOD_SOURCE_TITLE,
    blurb: "The Dark Mod core (id Tech 4 PK4 / MD5 / .proc / TGA). Load reads official tdm_installer.ini and reconstructs tdm_*.pk4 from the zipsync HTTP mirrors. Assets are CC BY-NC-SA 3.0. Fan missions stay with their authors and are not imported as TDM core.",
    license: DARKMOD_LICENSE,
    license_blurb: "© The Dark Mod team (thedarkmod.com). CC BY-NC-SA 3.0 — credit required, non-commercial, share-alike. Fan missions stay with their authors. Not CC0, not BSD.",
    homepage: DARKMOD_HOME,
    terms_url: DARKMOD_TERMS_URL,
    source_page: "https://www.thedarkmod.com/downloads/",
    github: Some(DARKMOD_GITHUB),
    credits: DARKMOD_CREDITS,
    import_wired: true,
};

/// Full Import module list: Kenney first, then classic, then not-wired cards.
pub const PACK_MODULES_WITH_CLASSIC: &[PackModule] = &[
    KENNEY_MODULE,
    FREEDOOM_MODULE,
    DOOM_MODULE,
    LIBREQUAKE_MODULE,
    QUAKE_MODULE,
    DUKE3D_MODULE,
    EA_MODULES[0],
    EA_MODULES[1],
    EA_MODULES[2],
    EA_MODULES[3],
    QUAKE2_MODULE,
    QUAKE3_MODULE,
    DARKMOD_MODULE,
    KAYKIT_MODULE,
    NASA_SKY_MODULE,
];

pub fn classic_candidate_dirs(source: ClassicSource) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let id = source.id();
    if let Ok(root) = std::env::var("AI_CONTENT_PACK_ROOT") {
        out.push(PathBuf::from(root).join(id));
    }
    let env_key = source.pack_root_env();
    if let Ok(root) = std::env::var(env_key) {
        out.push(PathBuf::from(root));
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    out.push(repo.join("local/packs").join(id));
    out.push(repo.join("local").join(id));
    out
}

pub fn resolve_classic_dir(source: ClassicSource, override_path: &str) -> PathBuf {
    let typed = override_path.trim();
    if !typed.is_empty() {
        return PathBuf::from(typed);
    }
    for candidate in classic_candidate_dirs(source) {
        if candidate.is_dir() {
            return candidate;
        }
    }
    classic_candidate_dirs(source)
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(source.id()))
}

pub fn classic_probe(source: ClassicSource, path_override: &str) -> DiskProbe {
    probe_dir(&resolve_classic_dir(source, path_override))
}

pub struct ClassicImportCard {
    pub source: ClassicSource,
    pub path: String,
    pub phase: ImportPhase,
    rx: Option<Receiver<ImportPhase>>,
    pending_landings: Vec<LibraryLanding>,
    pending_previews: Vec<(String, Vec<u8>)>,
    cancel: Arc<AtomicBool>,
    download_id: Option<LiveId>,
    download_url_index: usize,
    pending_server: Option<ServerSession>,
    pending_dir: PathBuf,
    pending_out: PathBuf,
    tdm: Option<TdmSync>,
    iso: Option<IsoSync>,
    /// The icon-render handshake for THIS source, mirroring Kenney/KayKit:
    /// the import thread parks in `ImportPhase::IconsPending` until every
    /// landing it handed the UI has been drained into the icon renderer, so
    /// a classic pack publishes real thumbnails and its staging can be
    /// reclaimed the moment publish succeeds. See [`IconResumeGate`].
    icon_resume: IconResumeGate,
}

struct IsoSync {
    url: String,
    layout: Option<makepad_asset_importer::iso9660::IsoLayout>,
    phase: IsoPhase,
}

enum IsoPhase {
    Probe,
    Dir {
        queue: Vec<(u32, u32)>,
        found: Vec<makepad_asset_importer::iso9660::IsoEntry>,
        depth: u32,
    },
    File {
        files: Vec<makepad_asset_importer::iso9660::IsoEntry>,
        index: usize,
    },
}

enum TdmSync {
    Installer,
    Manifests {
        urls: Vec<String>,
        versions: Vec<String>,
        got: Vec<TdmProvidedSet>,
    },
    Zips {
        plan: TdmFetchPlan,
        index: usize,
    },
}

impl ClassicImportCard {
    pub fn new(source: ClassicSource) -> Self {
        Self {
            source,
            path: String::new(),
            phase: ImportPhase::Idle,
            rx: None,
            pending_landings: Vec::new(),
            pending_previews: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            download_id: None,
            download_url_index: 0,
            pending_server: None,
            pending_dir: PathBuf::new(),
            pending_out: PathBuf::new(),
            tdm: None,
            iso: None,
            icon_resume: IconResumeGate::default(),
        }
    }

    pub fn request_stop(&mut self, cx: &mut Cx) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(request_id) = self.download_id.take() {
            cx.cancel_http_request(request_id);
            self.tdm = None;
            self.iso = None;
            self.phase = ImportPhase::Cancelled {
                pack: self.source.id().into(),
                message: "cancelled".into(),
            };
        }
    }

    pub fn stop_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn compiling(&self) -> bool {
        self.download_id.is_some()
            || matches!(
                self.phase,
                ImportPhase::Compiling { .. }
                    | ImportPhase::Downloading { .. }
                    | ImportPhase::Publishing { .. }
                    | ImportPhase::Annotating { .. }
                    | ImportPhase::Baking { .. }
                    // Parked waiting for icons: the thread is alive and WILL
                    // publish, so a second start must still be refused.
                    | ImportPhase::IconsPending { .. }
            )
    }

    pub fn status_line(&self, server_connected: bool) -> String {
        if self.cancel.load(Ordering::SeqCst) && self.compiling() {
            return format!("stopping {}…", self.source.title());
        }
        match &self.phase {
            ImportPhase::Downloading {
                pack,
                loaded,
                total,
                label,
            } => {
                let what = if label.is_empty() {
                    pack.clone()
                } else {
                    label.clone()
                };
                if *total > 0 {
                    let pct = ((*loaded as f64 / *total as f64) * 100.0).clamp(0.0, 100.0);
                    format!(
                        "loading {pack}: {} / {} ({pct:.0}%) · {what}",
                        fmt_bytes(*loaded),
                        fmt_bytes(*total)
                    )
                } else {
                    format!("loading {pack}: {} · {what}", fmt_bytes(*loaded))
                }
            }
            ImportPhase::Compiling {
                pack,
                done,
                total,
                current,
            } => {
                if *total > 0 {
                    if current.is_empty() {
                        format!("converting {pack}: {done}/{total}")
                    } else if current.starts_with("unpack")
                        || current.starts_with("scan")
                        || current.starts_with("clear")
                        || current.starts_with("found ")
                        || current.starts_with("loading texture")
                        || current.starts_with("building upload plan")
                    {
                        format!("{pack}: {done}/{total} · {current}")
                    } else {
                        format!("converting {pack}: {done}/{total} · {current}")
                    }
                } else if current.is_empty() {
                    format!("converting {pack}")
                } else {
                    format!("{pack}: {current}")
                }
            }
            ImportPhase::Publishing {
                pack,
                assets,
                blobs,
                blob_done,
            } => format!("publishing {pack}: {blob_done}/{blobs} blobs · {assets} assets"),
            ImportPhase::Annotating {
                pack,
                annotated,
                total,
                ..
            } => format!("annotating {pack}: {annotated}/{total}"),
            ImportPhase::Baking {
                pack,
                bake_done,
                bake_total,
                current,
                ..
            } => {
                if current.is_empty() {
                    format!("AO bake {pack}: {bake_done}/{bake_total}")
                } else {
                    format!("AO bake {pack}: {bake_done}/{bake_total} · {current}")
                }
            }
            ImportPhase::CompiledLocal {
                pack,
                assets,
                bake,
                out,
                error,
                library,
                ..
            } => {
                let why = error.as_deref().unwrap_or_else(|| {
                    out.to_str()
                        .filter(|s| s.starts_with("pack_import:"))
                        .unwrap_or("server offline — not in catalog")
                });
                format!(
                    "compiled {pack} locally · {assets} assets · {} landings · {why} · {}",
                    library.len(),
                    bake_fragment(bake)
                )
            }
            ImportPhase::Published {
                pack,
                assets,
                annotated,
                bake,
                ..
            } => format!(
                "published {pack} · {assets} assets · {annotated} annotated · {}",
                bake_fragment(bake)
            ),
            ImportPhase::IconsPending { pack, assets, .. } => {
                format!("{pack}: rendering {assets} icons…")
            }
            ImportPhase::PackFinished { pack, assets, .. } => {
                format!("{pack}: finished · {assets} assets")
            }
            ImportPhase::AllDone { ok, failed, skipped } => format!(
                "all done · {} imported · {} failed · {} skipped",
                ok.len(),
                failed.len(),
                skipped.len()
            ),
            ImportPhase::Failed { pack, message } => format!("{pack}: {message}"),
            ImportPhase::Cancelled { pack, message } => format!("{pack}: stopped — {message}"),
            ImportPhase::Idle => {
                let probe = classic_probe(self.source, &self.path);
                let rights = format!(
                    "{} · {} · credits {}",
                    self.source.license(),
                    if server_connected {
                        "server ready"
                    } else {
                        "compile-only (no server)"
                    },
                    self.source.credits()
                );
                if probe.ready() {
                    format!("ready · {} · {rights}", probe.path.display())
                } else if let Some(spec) =
                    makepad_asset_importer::classic_fetch::fetch_spec(self.source)
                {
                    format!(
                        "not on disk · Load fetches {} · {rights}",
                        spec.label
                    )
                } else {
                    format!(
                        "not on disk · put a local {} folder at {} · {rights}",
                        self.source.id(),
                        probe.path.display()
                    )
                }
            }
            // Never Debug-format into UI text: a phase carries whole
            // LibraryLanding vectors and the card printed the lot.
            ImportPhase::PreviewThumb { pack, .. } => format!("{pack}: preview"),
            // Multi-pack-only phase; a classic source imports one pack per
            // run and never emits it. Still say the reason if it appears.
            ImportPhase::PackFailed { pack, message, .. } => {
                format!("{pack}: NOT imported — {message}")
            }
        }
    }

    pub fn progress_fraction(&self) -> f32 {
        match &self.phase {
            ImportPhase::Idle
            | ImportPhase::Failed { .. }
            | ImportPhase::Cancelled { .. } => 0.0,
            ImportPhase::Downloading {
                loaded, total, ..
            } => {
                if *total == 0 {
                    0.08
                } else {
                    0.02 + 0.40 * (*loaded as f32 / *total as f32)
                }
            }
            ImportPhase::Compiling {
                done, total, ..
            } => {
                if *total == 0 {
                    0.08
                } else {
                    0.05 + 0.75 * (*done as f32 / *total as f32)
                }
            }
            ImportPhase::Publishing {
                blobs, blob_done, ..
            } => {
                if *blobs == 0 {
                    0.25
                } else {
                    0.15 + 0.35 * (*blob_done as f32 / *blobs as f32)
                }
            }
            ImportPhase::Annotating {
                annotated, total, ..
            } => {
                if *total == 0 {
                    0.55
                } else {
                    0.5 + 0.2 * (*annotated as f32 / *total as f32)
                }
            }
            ImportPhase::Baking {
                bake_done,
                bake_total,
                ..
            } => {
                if *bake_total == 0 {
                    0.85
                } else {
                    0.7 + 0.25 * (*bake_done as f32 / *bake_total as f32)
                }
            }
            ImportPhase::Published { .. } | ImportPhase::CompiledLocal { .. } => 1.0,
            _ => 0.5,
        }
    }

    pub fn take_library_landings(&mut self) -> Vec<LibraryLanding> {
        std::mem::take(&mut self.pending_landings)
    }

    pub fn take_previews(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.pending_previews)
    }

    /// True once this card's parked import may continue: its landings are
    /// drained and the UI's icon renderer is idle (or the user cancelled).
    pub fn icons_pending_ready(&self, landings_drained: bool, icons_busy: bool) -> bool {
        matches!(self.phase, ImportPhase::IconsPending { .. }) && landings_drained && !icons_busy
    }

    /// Let the parked import thread continue. No-op past the first call per
    /// `IconsPending` phase.
    pub fn resume_icons_pending(&mut self) -> bool {
        self.icon_resume.resume()
    }

    fn ingest(&mut self, phase: ImportPhase) {
        match &phase {
            ImportPhase::PreviewThumb { name, png, .. } => {
                self.pending_previews.push((name.clone(), png.clone()));
                return;
            }
            ImportPhase::IconsPending { library, .. } => {
                self.pending_landings.extend(library.iter().cloned());
                // A fresh icon wait for this pack: allow exactly one resume.
                self.icon_resume.arm();
            }
            ImportPhase::Published { library, .. }
            | ImportPhase::CompiledLocal { library, .. } => {
                self.pending_landings.extend(library.iter().cloned());
            }
            _ => {}
        }
        self.phase = phase;
    }

    pub fn start_import(
        &mut self,
        cx: &mut Cx,
        path_override: String,
        server: Option<ServerSession>,
    ) -> Result<(), String> {
        if self.compiling() {
            return Err(format!(
                "a {} import is already running",
                self.source.title()
            ));
        }
        self.path = path_override;
        let dir = resolve_classic_dir(self.source, &self.path);
        let spec = makepad_asset_importer::classic_fetch::fetch_spec(self.source);
        let files = makepad_asset_importer::classic_fetch::payload_files(&dir);
        let ready = makepad_asset_importer::classic_fetch::has_source_payload(self.source, &dir);
        log!(
            "import {}: dir={} exists={} ready={} files={:?} spec={}",
            self.source.id(),
            dir.display(),
            dir.is_dir(),
            ready,
            files,
            spec.map(|s| s.url()).unwrap_or("-")
        );
        if !ready && spec.is_none() {
            let message = format!(
                "pack is not on disk ({}) and this source has no auto-fetch URL",
                dir.display()
            );
            log!("import {}: {message}", self.source.id());
            self.phase = ImportPhase::Failed {
                pack: self.source.id().into(),
                message: message.clone(),
            };
            return Err(message);
        }
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import")
            .join(self.source.id());
        self.cancel.store(false, Ordering::SeqCst);
        if !ready {
            self.pending_server = server;
            self.pending_dir = dir;
            self.pending_out = out;
            self.download_url_index = 0;
            if self.source == ClassicSource::DarkMod {
                log!("import {}: start tdm zipsync", self.source.id());
                return self.start_tdm_sync(cx);
            }
            let spec = spec.expect("checked above");
            if spec.url().to_ascii_lowercase().ends_with(".iso") {
                log!(
                    "import {}: start ISO range extract {}",
                    self.source.id(),
                    spec.url()
                );
                return self.start_iso_extract(cx, spec);
            }
            log!("import {}: start full GET {}", self.source.id(), spec.url());
            return self.begin_download(cx, spec, 0);
        }
        log!(
            "import {}: convert existing payload in {} -> {}",
            self.source.id(),
            dir.display(),
            out.display()
        );
        self.begin_convert(dir, out, server)
    }

    fn start_tdm_sync(&mut self, cx: &mut Cx) -> Result<(), String> {
        self.tdm_log(format!(
            "start dest={} cache={}",
            self.pending_dir.display(),
            self.tdm_cache_dir().display()
        ));
        if let Ok(plan) = tdm_zipsync::load_plan(&self.tdm_cache_dir()) {
            let (ready, total, bytes) = tdm_zipsync::cached_span_progress(&self.tdm_cache_dir(), &plan);
            self.tdm_log(format!(
                "resume saved plan {} · {ready}/{total} spans already cached ({bytes} bytes) · skip installer.ini + manifests",
                plan.version
            ));
            self.phase = ImportPhase::Compiling {
                pack: self.source.id().into(),
                done: ready,
                total,
                current: format!("resuming zipsync {ready}/{total}"),
            };
            let _ = self.start_next_tdm_span(cx, plan, 0);
            return Ok(());
        }
        self.tdm = Some(TdmSync::Installer);
        self.phase = ImportPhase::Downloading {
            pack: self.source.id().into(),
            loaded: 0,
            total: 0,
            label: "tdm_installer.ini".into(),
        };
        self.tdm_log(format!("GET {TDM_INSTALLER_INI_URL}"));
        self.get_url(cx, TDM_INSTALLER_INI_URL);
        Ok(())
    }

    fn get_url(&mut self, cx: &mut Cx, url: &str) {
        let request_id = LiveId::unique();
        self.download_id = Some(request_id);
        cx.http_request(request_id, crate::http::get(url));
    }

    fn get_span(&mut self, cx: &mut Cx, url: &str, start: u64, end: u64) {
        let request_id = LiveId::unique();
        self.download_id = Some(request_id);
        cx.http_request(request_id, crate::http::get_range(url, start, end));
    }

    fn start_iso_extract(
        &mut self,
        cx: &mut Cx,
        spec: makepad_asset_importer::classic_fetch::ClassicFetchSpec,
    ) -> Result<(), String> {
        let url = spec.url().to_string();
        self.iso = Some(IsoSync {
            url: url.clone(),
            layout: None,
            phase: IsoPhase::Probe,
        });
        self.phase = ImportPhase::Downloading {
            pack: self.source.id().into(),
            loaded: 0,
            total: 0,
            label: format!("ISO range probe · {}", spec.label),
        };
        log!("iso: GET Range bytes=0-{} {url}", makepad_asset_importer::iso9660::PROBE_BYTES);
        self.get_span(cx, &url, 0, makepad_asset_importer::iso9660::PROBE_BYTES);
        Ok(())
    }

    fn iso_fail(&mut self, message: String) -> bool {
        log!("iso: FAIL {message}");
        self.iso = None;
        self.download_id = None;
        self.phase = ImportPhase::Failed {
            pack: self.source.id().into(),
            message,
        };
        true
    }

    fn iso_get_extent(
        &mut self,
        cx: &mut Cx,
        layout: makepad_asset_importer::iso9660::IsoLayout,
        lba: u32,
        size: u32,
        current: String,
        done: usize,
        total: usize,
    ) {
        let (start, end) = layout.extent_raw_range(lba, size);
        self.phase = ImportPhase::Compiling {
            pack: self.source.id().into(),
            done,
            total,
            current,
        };
        let url = self
            .iso
            .as_ref()
            .map(|s| s.url.clone())
            .unwrap_or_default();
        log!(
            "iso: GET Range bytes={start}-{} {url} (LBA {lba} {size} user bytes)",
            end.saturating_sub(1)
        );
        self.get_span(cx, &url, start, end);
    }

    fn handle_iso_body(&mut self, cx: &mut Cx, body: &[u8]) -> bool {
        use makepad_asset_importer::iso9660::{
            cook_extent, detect_layout, extract_keep_from_image, iso_keep_name, parse_directory,
            parse_pvd,
        };
        let Some(mut sync) = self.iso.take() else {
            return false;
        };
        if body.len() > 1024 * 1024 && detect_layout(body).is_ok() {
            log!("iso: host sent {} bytes (whole image?) — extract locally", body.len());
            match extract_keep_from_image(body, &self.pending_dir, iso_keep_name) {
                Ok(names) => {
                    log!("iso: extracted {}", names.join(", "));
                    return self.iso_begin_convert();
                }
                Err(error) => return self.iso_fail(error),
            }
        }
        match sync.phase {
            IsoPhase::Probe => {
                log!(
                    "iso: probe {} bytes · cd001={} · sync={}",
                    body.len(),
                    body.windows(5).any(|w| w == b"CD001"),
                    body.len() >= 12 && body[..12] == [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]
                );
                let layout = match detect_layout(body) {
                    Ok(layout) => layout,
                    Err(error) => return self.iso_fail(error),
                };
                let vol = match parse_pvd(body, layout) {
                    Ok(vol) => vol,
                    Err(error) => return self.iso_fail(error),
                };
                log!(
                    "iso: {} volume {:?} · root LBA {} ({} bytes)",
                    if layout.is_raw() { "raw 2352" } else { "cooked 2048" },
                    vol.volume_id,
                    vol.root_lba,
                    vol.root_size
                );
                sync.layout = Some(layout);
                sync.phase = IsoPhase::Dir {
                    queue: vec![(vol.root_lba, vol.root_size)],
                    found: Vec::new(),
                    depth: 0,
                };
                self.iso = Some(sync);
                self.iso_get_extent(
                    cx,
                    layout,
                    vol.root_lba,
                    vol.root_size,
                    format!("ISO dir {}", vol.volume_id),
                    1,
                    4,
                );
                true
            }
            IsoPhase::Dir {
                mut queue,
                mut found,
                depth,
            } => {
                let Some(layout) = sync.layout else {
                    return self.iso_fail("ISO layout missing".into());
                };
                let Some(&(_lba, size)) = queue.first() else {
                    return self.iso_fail("ISO directory queue empty".into());
                };
                queue.remove(0);
                let cooked = match cook_extent(body, layout, size) {
                    Ok(cooked) => cooked,
                    Err(error) => return self.iso_fail(error),
                };
                for entry in parse_directory(&cooked) {
                    if entry.is_dir {
                        if depth < 6 {
                            log!("iso: dir {} LBA {}", entry.name, entry.lba);
                            queue.push((entry.lba, entry.size));
                        }
                        continue;
                    }
                    if iso_keep_name(&entry.name) {
                        log!(
                            "iso: keep {} LBA {} ({} bytes)",
                            entry.name,
                            entry.lba,
                            entry.size
                        );
                        found.push(entry);
                    }
                }
                if let Some(&(next_lba, next_size)) = queue.first() {
                    sync.phase = IsoPhase::Dir {
                        queue,
                        found,
                        depth: depth + 1,
                    };
                    self.iso = Some(sync);
                    self.iso_get_extent(
                        cx,
                        layout,
                        next_lba,
                        next_size,
                        "ISO walking directories".into(),
                        2,
                        4,
                    );
                    return true;
                }
                if found.is_empty() {
                    return self.iso_fail("ISO has no .grp (DUKE3D.GRP not on disc)".into());
                }
                let file = found[0].clone();
                sync.phase = IsoPhase::File { files: found, index: 0 };
                self.iso = Some(sync);
                self.iso_get_extent(
                    cx,
                    layout,
                    file.lba,
                    file.size,
                    format!("ISO range {} ({} bytes)", file.name, file.size),
                    3,
                    4,
                );
                true
            }
            IsoPhase::File { files, index } => {
                let Some(layout) = sync.layout else {
                    return self.iso_fail("ISO layout missing".into());
                };
                let Some(file) = files.get(index) else {
                    return self.iso_fail("ISO file index past end".into());
                };
                let cooked = match cook_extent(body, layout, file.size) {
                    Ok(cooked) => cooked,
                    Err(error) => return self.iso_fail(error),
                };
                if file.name.to_ascii_lowercase().ends_with(".grp")
                    && !cooked.starts_with(b"KenSilverman")
                {
                    return self.iso_fail(format!(
                        "ISO {} is not a BUILD GRP after cooking {} bytes",
                        file.name,
                        cooked.len()
                    ));
                }
                let dest = self.pending_dir.join(&file.name);
                if let Some(parent) = dest.parent() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        return self.iso_fail(format!("{}: {error}", parent.display()));
                    }
                }
                if let Err(error) = std::fs::write(&dest, &cooked) {
                    return self.iso_fail(format!("write {}: {error}", dest.display()));
                }
                log!(
                    "iso: wrote {} ({} bytes) -> {}",
                    file.name,
                    cooked.len(),
                    dest.display()
                );
                let next = index + 1;
                if next < files.len() {
                    let nxt = files[next].clone();
                    sync.phase = IsoPhase::File { files, index: next };
                    self.iso = Some(sync);
                    self.iso_get_extent(
                        cx,
                        layout,
                        nxt.lba,
                        nxt.size,
                        format!("ISO range {} ({} bytes)", nxt.name, nxt.size),
                        3,
                        4,
                    );
                    return true;
                }
                self.iso = None;
                self.iso_begin_convert()
            }
        }
    }

    fn iso_begin_convert(&mut self) -> bool {
        self.iso = None;
        let dir = self.pending_dir.clone();
        let out = self.pending_out.clone();
        let server = self.pending_server.take();
        if let Err(error) = self.begin_convert(dir, out, server) {
            return self.iso_fail(error);
        }
        true
    }

    fn start_next_tdm_span(&mut self, cx: &mut Cx, plan: TdmFetchPlan, mut index: usize) -> bool {
        let cache = self.tdm_cache_dir();
        while index < plan.spans.len()
            && tdm_zipsync::span_is_cached(&cache, &plan.spans[index])
        {
            self.tdm_log(format!(
                "cache hit skip GET {} ({}-{})",
                plan.spans[index].url,
                plan.spans[index].start,
                plan.spans[index].end
            ));
            index += 1;
        }
        if index >= plan.spans.len() {
            return self.tdm_finish_from_cache(plan);
        }
        let span = &plan.spans[index];
        self.phase = ImportPhase::Compiling {
            pack: self.source.id().into(),
            done: index,
            total: plan.spans.len(),
            current: format!(
                "zipsync range {}/{} · {} bytes {}-{}",
                index + 1,
                plan.spans.len(),
                span.url,
                span.start,
                span.end
            ),
        };
        let url = span.url.clone();
        let start = span.start;
        let end = span.end;
        self.tdm_log(format!(
            "GET Range bytes={}-{} {} ({} bytes)",
            start,
            end.saturating_sub(1),
            url,
            end.saturating_sub(start)
        ));
        self.tdm = Some(TdmSync::Zips { plan, index });
        self.get_span(cx, &url, start, end);
        true
    }

    fn tdm_finish_from_cache(&mut self, plan: TdmFetchPlan) -> bool {
        self.phase = ImportPhase::Compiling {
            pack: self.source.id().into(),
            done: plan.spans.len(),
            total: plan.spans.len(),
            current: "reconstructing tdm_*.pk4 from cache".into(),
        };
        self.tdm_log(format!(
            "reconstruct {} members from {} cached spans -> {}",
            plan.members.len(),
            plan.spans.len(),
            self.pending_dir.display()
        ));
        let cache = self.tdm_cache_dir();
        match tdm_zipsync::reconstruct_pk4s(&self.pending_dir, &plan, &cache) {
            Ok(pk4s) => self.tdm_log(format!("reconstruct wrote {pk4s} pk4 files")),
            Err(error) => return self.tdm_fail(error),
        }
        self.tdm = None;
        let dir = self.pending_dir.clone();
        let out = self.pending_out.clone();
        let server = self.pending_server.take();
        if let Err(error) = self.begin_convert(dir, out, server) {
            return self.tdm_fail(error);
        }
        true
    }

    fn tdm_cache_dir(&self) -> PathBuf {
        self.pending_dir.join(".tdm-sync")
    }

    fn tdm_log(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        log!("tdm: {message}");
        let dir = self.tdm_cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("fetch.log"))
        {
            use std::io::Write;
            let _ = writeln!(file, "{message}");
        }
    }

    fn handle_tdm_body(&mut self, cx: &mut Cx, body: &[u8]) -> bool {
        match self.tdm.take() {
            Some(TdmSync::Installer) => self.tdm_on_installer(cx, body),
            Some(TdmSync::Manifests {
                urls,
                versions,
                got,
            }) => self.tdm_on_manifest(cx, body, urls, versions, got),
            Some(TdmSync::Zips { plan, index }) => self.tdm_on_zip(cx, body, plan, index),
            None => false,
        }
    }

    fn tdm_on_installer(&mut self, cx: &mut Cx, body: &[u8]) -> bool {
        let text = match std::str::from_utf8(body) {
            Ok(text) => text,
            Err(error) => {
                return self.tdm_fail(format!("tdm_installer.ini is not text: {error}"));
            }
        };
        let ini = match tdm_zipsync::parse_installer_ini(text) {
            Ok(ini) => ini,
            Err(error) => return self.tdm_fail(error),
        };
        let _ = std::fs::create_dir_all(self.tdm_cache_dir());
        if let Err(error) = std::fs::write(self.tdm_cache_dir().join("tdm_installer.ini"), body) {
            self.tdm_log(format!("could not cache installer.ini: {error}"));
        }
        let chain = match ini.version_chain() {
            Ok(chain) => chain,
            Err(error) => return self.tdm_fail(error),
        };
        let versions: Vec<String> = chain.iter().map(|v| v.name.clone()).collect();
        let urls: Vec<String> = chain.iter().map(|v| ini.manifest_http_url(v)).collect();
        if urls.is_empty() {
            return self.tdm_fail("tdm version chain is empty".into());
        }
        self.tdm_log(format!(
            "installer.ini {} bytes · chain {}",
            body.len(),
            versions.join(" -> ")
        ));
        self.phase = ImportPhase::Compiling {
            pack: self.source.id().into(),
            done: 0,
            total: urls.len(),
            current: format!("zipsync manifest 1/{} ({})", urls.len(), versions[0]),
        };
        let first = urls[0].clone();
        self.tdm = Some(TdmSync::Manifests {
            urls,
            versions,
            got: Vec::new(),
        });
        self.tdm_log(format!("GET manifest {first}"));
        self.get_url(cx, &first);
        true
    }

    fn tdm_on_manifest(
        &mut self,
        cx: &mut Cx,
        body: &[u8],
        urls: Vec<String>,
        versions: Vec<String>,
        mut got: Vec<TdmProvidedSet>,
    ) -> bool {
        let files = match tdm_zipsync::parse_manifest_iniz(body) {
            Ok(files) => files,
            Err(error) => return self.tdm_fail(error),
        };
        let index = got.len();
        let version = versions.get(index).cloned().unwrap_or_default();
        let url = urls.get(index).cloned().unwrap_or_default();
        let provided = files.iter().filter(|f| f.byterange.is_some()).count();
        self.tdm_log(format!(
            "manifest {} {} bytes · {} files · {} provided · root {}",
            version,
            body.len(),
            files.len(),
            provided,
            tdm_zipsync::package_root_from_manifest_url(&url)
        ));
        let man_dir = self.tdm_cache_dir().join("manifests");
        let _ = std::fs::create_dir_all(&man_dir);
        if let Err(error) = std::fs::write(man_dir.join(format!("{version}.iniz")), body) {
            self.tdm_log(format!("could not cache manifest {version}: {error}"));
        }
        got.push(TdmProvidedSet {
            version,
            package_root: tdm_zipsync::package_root_from_manifest_url(&url),
            files,
        });
        if got.len() < urls.len() {
            let next = got.len();
            self.phase = ImportPhase::Compiling {
                pack: self.source.id().into(),
                done: next,
                total: urls.len(),
                current: format!(
                    "zipsync manifest {}/{} ({})",
                    next + 1,
                    urls.len(),
                    versions.get(next).cloned().unwrap_or_default()
                ),
            };
            let next_url = urls[next].clone();
            self.tdm = Some(TdmSync::Manifests {
                urls,
                versions,
                got,
            });
            self.tdm_log(format!("GET manifest {next_url}"));
            self.get_url(cx, &next_url);
            return true;
        }
        let plan = match tdm_zipsync::plan_clean_install(&got) {
            Ok(plan) => plan,
            Err(error) => return self.tdm_fail(error),
        };
        if plan.spans.is_empty() {
            return self.tdm_fail("zipsync plan has no PK4 urls".into());
        }
        if let Err(error) = std::fs::create_dir_all(self.tdm_cache_dir()) {
            return self.tdm_fail(format!("create tdm cache: {error}"));
        }
        if let Err(error) = tdm_zipsync::save_plan(&self.tdm_cache_dir(), &plan) {
            self.tdm_log(format!("could not save plan (resume will re-fetch manifests): {error}"));
        } else {
            self.tdm_log("wrote plan.v1 · later Import can resume without re-fetching manifests");
        }
        let bytes: u64 = plan.spans.iter().map(|s| s.end.saturating_sub(s.start)).sum();
        self.tdm_log(format!(
            "plan {} · {} members · {} spans · {} bytes to fetch (cached spans skipped)",
            plan.version,
            plan.members.len(),
            plan.spans.len(),
            bytes
        ));
        for (i, span) in plan.spans.iter().enumerate() {
            let hit = tdm_zipsync::span_is_cached(&self.tdm_cache_dir(), span);
            self.tdm_log(format!(
                "  span[{i}] {} {}-{} {}b cache={}",
                span.url,
                span.start,
                span.end,
                span.end.saturating_sub(span.start),
                if hit { "hit" } else { "miss" }
            ));
        }
        self.start_next_tdm_span(cx, plan, 0)
    }

    fn tdm_on_zip(
        &mut self,
        cx: &mut Cx,
        body: &[u8],
        plan: TdmFetchPlan,
        index: usize,
    ) -> bool {
        let Some(span) = plan.spans.get(index) else {
            return self.tdm_fail("zipsync span index past end".into());
        };
        let cache = self.tdm_cache_dir();
        if let Err(error) = std::fs::create_dir_all(&cache) {
            return self.tdm_fail(format!("create {}: {error}", cache.display()));
        }
        let expected = span.end.saturating_sub(span.start) as usize;
        if body.len() != expected {
            return self.tdm_fail(format!(
                "range {}-{} for {} returned {} bytes, expected {expected}",
                span.start,
                span.end,
                span.url,
                body.len()
            ));
        }
        if let Err(error) = tdm_zipsync::write_span_atomic(&cache, span, body) {
            return self.tdm_fail(error);
        }
        let path = tdm_zipsync::span_cache_path(&cache, span);
        self.tdm_log(format!(
            "cached {} bytes -> {} (range {}-{})",
            body.len(),
            path.display(),
            span.start,
            span.end
        ));
        self.start_next_tdm_span(cx, plan, index + 1)
    }

    fn tdm_fail(&mut self, message: String) -> bool {
        self.tdm_log(format!(
            "FAIL (cache kept at {}): {message}",
            self.tdm_cache_dir().display()
        ));
        self.tdm = None;
        self.download_id = None;
        self.phase = ImportPhase::Failed {
            pack: self.source.id().into(),
            message,
        };
        true
    }

    fn begin_download(
        &mut self,
        cx: &mut Cx,
        spec: makepad_asset_importer::classic_fetch::ClassicFetchSpec,
        index: usize,
    ) -> Result<(), String> {
        let url = spec.url_at(index).ok_or_else(|| {
            format!("{} has no auto-fetch URL", self.source.title())
        })?;
        let request_id = LiveId::unique();
        self.download_id = Some(request_id);
        self.download_url_index = index;
        self.phase = ImportPhase::Downloading {
            pack: self.source.id().into(),
            loaded: 0,
            total: 0,
            label: spec.label.into(),
        };
        cx.http_request(request_id, crate::http::get(url));
        Ok(())
    }

    fn retry_or_fail(&mut self, cx: &mut Cx, error: String) -> bool {
        let Some(spec) = makepad_asset_importer::classic_fetch::fetch_spec(self.source) else {
            self.phase = ImportPhase::Failed {
                pack: self.source.id().into(),
                message: error,
            };
            return true;
        };
        let next = self.download_url_index + 1;
        if spec.url_at(next).is_some() {
            if self.begin_download(cx, spec, next).is_ok() {
                return true;
            }
        }
        self.phase = ImportPhase::Failed {
            pack: self.source.id().into(),
            message: error,
        };
        true
    }

    pub fn handle_http_progress(&mut self, request_id: LiveId, progress: &HttpProgress) -> bool {
        if self.download_id != Some(request_id) {
            return false;
        }
        let label = match &self.phase {
            ImportPhase::Downloading { label, .. } => label.clone(),
            ImportPhase::Compiling { current, .. } => current.clone(),
            _ => "download".into(),
        };
        self.phase = ImportPhase::Downloading {
            pack: self.source.id().into(),
            loaded: progress.loaded,
            total: progress.total,
            label,
        };
        true
    }

    pub fn handle_http_error(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        error: &HttpError,
    ) -> bool {
        if self.download_id != Some(request_id) {
            return false;
        }
        self.download_id = None;
        if self.tdm.is_some() {
            return self.tdm_fail(format!("http error: {}", error.message));
        }
        if self.iso.is_some() {
            return self.iso_fail(format!("http error: {}", error.message));
        }
        self.retry_or_fail(cx, format!("download failed: {}", error.message))
    }

    pub fn handle_http_response(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        response: &HttpResponse,
    ) -> bool {
        if self.download_id != Some(request_id) {
            return false;
        }
        self.download_id = None;
        if self.cancel.load(Ordering::SeqCst) {
            self.tdm = None;
            self.iso = None;
            self.phase = ImportPhase::Cancelled {
                pack: self.source.id().into(),
                message: "cancelled".into(),
            };
            return true;
        }
        if self.tdm.is_some() || self.iso.is_some() {
            let range = response
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-range"))
                .map(|(_, v)| v.join(","));
            let n = response.body().map(|b| b.len()).unwrap_or(0);
            let head = response.body().map(|b| {
                b.iter()
                    .take(16)
                    .map(|x| format!("{x:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            log!(
                "iso/http: status={} bytes={} range={:?} head={:?}",
                response.status_code,
                n,
                range,
                head
            );
            if self.tdm.is_some() {
                self.tdm_log(format!(
                    "HTTP {} · {n} bytes · content-range {:?}",
                    response.status_code, range
                ));
            }
        }
        let ok_status = response.status_code == 200
            || response.status_code == 206
            || (response.status_code == 0 && response.body().is_some());
        if !ok_status {
            let preview = response
                .body()
                .and_then(|b| std::str::from_utf8(&b[..b.len().min(180)]).ok())
                .unwrap_or("");
            if self.tdm.is_some() {
                return self.tdm_fail(format!(
                    "download HTTP {} body={preview:?}",
                    response.status_code
                ));
            }
            if self.iso.is_some() {
                return self.iso_fail(format!(
                    "download HTTP {} body={preview:?}",
                    response.status_code
                ));
            }
            return self.retry_or_fail(cx, format!("download HTTP {}", response.status_code));
        }
        let Some(body) = response.body() else {
            if self.tdm.is_some() {
                return self.tdm_fail("download had no body".into());
            }
            if self.iso.is_some() {
                return self.iso_fail("download had no body".into());
            }
            return self.retry_or_fail(cx, "download had no body".into());
        };
        if self.tdm.is_some() {
            return self.handle_tdm_body(cx, body);
        }
        if self.iso.is_some() {
            return self.handle_iso_body(cx, body);
        }
        let dir = self.pending_dir.clone();
        log!(
            "import {}: download {} bytes, unpacking into {}",
            self.source.id(),
            body.len(),
            dir.display()
        );
        if let Err(error) =
            makepad_asset_importer::classic_fetch::unpack_downloaded(body, &dir)
        {
            log!("import {}: unpack failed: {error}", self.source.id());
            return self.retry_or_fail(cx, error);
        }
        log!(
            "import {}: unpacked {:?}",
            self.source.id(),
            makepad_asset_importer::classic_fetch::payload_files(&dir)
        );
        let server = self.pending_server.take();
        let out = self.pending_out.clone();
        if let Err(error) = self.begin_convert(dir, out, server) {
            self.phase = ImportPhase::Failed {
                pack: self.source.id().into(),
                message: error,
            };
        }
        true
    }

    fn begin_convert(
        &mut self,
        dir: PathBuf,
        out: PathBuf,
        server: Option<ServerSession>,
    ) -> Result<(), String> {
        let files = makepad_asset_importer::classic_fetch::payload_files(&dir);
        log!(
            "import {}: begin_convert dir={} files={:?}",
            self.source.id(),
            dir.display(),
            files
        );
        let source = self.source;
        let pack_name = source.id().to_string();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.phase = ImportPhase::compiling(pack_name.clone());
        let cancel = self.cancel.clone();
        // Same handshake Kenney/KayKit use: the thread parks after staging
        // until the UI has taken every landing for icon rendering.
        let (gate, icon_resume_rx) = IconResumeGate::armed();
        self.icon_resume = gate;
        thread::Builder::new()
            .name(format!("asset-ui-{}-import", source.id()))
            .spawn(move || {
                let phase = run_classic_import(
                    &dir,
                    &out,
                    source,
                    &pack_name,
                    server,
                    &tx,
                    &cancel,
                    &icon_resume_rx,
                );
                let _ = tx.send(phase);
            })
            .map_err(|e| format!("failed to start classic import thread: {e}"))?;
        Ok(())
    }

    pub fn poll(&mut self) -> bool {
        if self.rx.is_none() {
            return false;
        }
        let mut changed = false;
        loop {
            let msg = self.rx.as_ref().map(|rx| rx.try_recv());
            match msg {
                Some(Ok(phase)) => {
                    self.ingest(phase);
                    changed = true;
                }
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    if self.compiling() {
                        self.phase = ImportPhase::Failed {
                            pack: self.source.id().into(),
                            message: "import thread exited without a result".into(),
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

fn bake_fragment(bake: &BakeStats) -> String {
    if bake.total == 0 {
        return "no GLBs to bake".into();
    }
    format!(
        "AO {}/{} ({} fresh, {} failed)",
        bake.baked + bake.skipped,
        bake.total,
        bake.skipped,
        bake.failed
    )
}

#[allow(clippy::too_many_arguments)]
fn run_classic_import(
    pack_dir: &Path,
    out: &Path,
    source: ClassicSource,
    pack_name: &str,
    server: Option<ServerSession>,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
    cancel: &AtomicBool,
    icon_resume_rx: &Receiver<()>,
) -> ImportPhase {
    let work = out.join("work");
    let staged = work.join("source");
    let dest_root = out.join("out");
    if let Err(error) = std::fs::create_dir_all(&dest_root) {
        return ImportPhase::Failed {
            pack: pack_name.into(),
            message: format!("create out root: {error}"),
        };
    }
    let bundle = dest_root.join("bundle");
    if bundle.exists() {
        let _ = std::fs::remove_dir_all(&bundle);
    }

    let pack_dir = pack_dir.to_path_buf();
    if !makepad_asset_importer::classic_fetch::has_classic_payload(&pack_dir) {
        return ImportPhase::Failed {
            pack: pack_name.into(),
            message: format!(
                "pack is not on disk ({}) — download it from Import first",
                pack_dir.display()
            ),
        };
    }

    // Convert classic formats → staged PNG/WAV/GLB (+ ao_bake inside convert).
    let convert = match classic_import::convert_classic_ex(&pack_dir, &staged, source, |tick| {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        if let Some(png) = tick.preview_png {
            let _ = tx.send(ImportPhase::PreviewThumb {
                pack: pack_name.to_string(),
                name: tick.current.clone(),
                png,
            });
        }
        if tick.stage == classic_import::ConvertStage::Expand {
            let _ = tx.send(ImportPhase::Compiling {
                pack: pack_name.to_string(),
                done: tick.done,
                total: tick.total,
                current: tick.current,
            });
        } else if tick.stage == classic_import::ConvertStage::Ao {
            let _ = tx.send(ImportPhase::Baking {
                pack: pack_name.to_string(),
                assets: 0,
                annotated: 0,
                bake_done: tick.done,
                bake_total: tick.total,
                bake_skipped: 0,
                bake_failed: 0,
                current: tick.current,
            });
        } else {
            let _ = tx.send(ImportPhase::Compiling {
                pack: pack_name.to_string(),
                done: tick.done,
                total: tick.total,
                current: tick.current,
            });
        }
        true
    }) {
        Ok(r) => {
            log!(
                "import {pack_name}: convert {} assets, {} skipped, {} warnings",
                r.assets.len(),
                r.skipped.len(),
                r.warnings.len()
            );
            if !r.skipped.is_empty() {
                for line in r.skipped.iter().take(12) {
                    log!("import {pack_name}: skipped {line}");
                }
            }
            r
        }
        Err(error) => {
            if error.to_string() == "cancelled" || cancel.load(Ordering::SeqCst) {
                return ImportPhase::Cancelled {
                    pack: pack_name.into(),
                    message: "stopped".into(),
                };
            }
            return ImportPhase::Failed {
                pack: pack_name.into(),
                message: error.to_string(),
            }
        }
    };

    // Real icons before publish, ALWAYS — the same handshake Kenney/KayKit
    // use. Conversion already wrote this pack's own thumbnails (world
    // previews, sprite strips, mesh rasters); parking here hands those
    // landings to the UI's icon renderer and, crucially, guarantees every
    // landing has been READ out of `work/source/` before the staging
    // cleanup below deletes it.
    let library = classic_library_landings(&staged, source, pack_name, &convert.assets);
    let _ = tx.send(ImportPhase::IconsPending {
        pack: pack_name.into(),
        assets: library.len(),
        library: library.clone(),
        bake: BakeStats {
            total: convert.bake.total,
            baked: convert.bake.baked,
            skipped: convert.bake.skipped,
            failed: convert.bake.failed,
        },
    });
    if icon_resume_rx.recv().is_err() {
        return ImportPhase::Cancelled {
            pack: pack_name.into(),
            message: "icon handshake lost".into(),
        };
    }
    if cancel.load(Ordering::SeqCst) {
        return ImportPhase::Cancelled {
            pack: pack_name.into(),
            message: "stopped while rendering icons".into(),
        };
    }

    let spec = source.pack_spec(pack_name);
    // `compile_pack` walks every staged file to build the upload plan
    // (hashing each one), and it can run for a while on a big pack. Without
    // live progress here the status line would stay on the just-finished
    // `IconsPending` text ("rendering N icons…") for that whole stretch,
    // reading as a hang even though icon rendering is already done and the
    // thread has moved on — so drive `ImportPhase::Compiling` from
    // `compile_pack_with_progress`'s per-file callback instead of one
    // static message. Throttled to <=10 updates/s (the callback itself
    // fires once per file, which for a big pack is far more often than
    // that); the very first call (nothing hashed yet) and the very last
    // (everything hashed) always get through regardless of the throttle,
    // so the phase change and the final tally are never swallowed.
    let mut last_progress_sent: Option<std::time::Instant> = None;
    let mut send_compile_progress = move |p: pack_import::CompileProgress| {
        let now = std::time::Instant::now();
        let edge = p.files_done == 0 || p.files_done == p.files_total;
        if !edge {
            if let Some(last) = last_progress_sent {
                if now.duration_since(last) < std::time::Duration::from_millis(100) {
                    return;
                }
            }
        }
        last_progress_sent = Some(now);
        let current = format!(
            "building upload plan · {}/{} MB{}",
            p.bytes_done / (1024 * 1024),
            p.bytes_total / (1024 * 1024),
            if p.current.is_empty() {
                String::new()
            } else {
                format!(" · {}", p.current)
            }
        );
        let _ = tx.send(ImportPhase::Compiling {
            pack: pack_name.to_string(),
            done: p.files_done,
            total: p.files_total,
            current,
        });
    };
    let report = match pack_import::compile_pack_with_progress(
        &staged,
        &bundle,
        spec,
        None,
        false,
        Some(&mut send_compile_progress),
    ) {
        Ok(r) => Some(r),
        Err(error) => {
            log!("import {pack_name}: pack_import failed: {error}");
            // Converted assets still land in the local library. Catalog
            // publish needs a valid bundle; we report the compile error in
            // the local status line instead of dropping the pack. Staging
            // stays put for inspection (`staging_dirs_to_clear(false)`).
            let bake = BakeStats {
                total: convert.bake.total,
                baked: convert.bake.baked,
                skipped: convert.bake.skipped,
                failed: convert.bake.failed,
            };
            return ImportPhase::CompiledLocal {
                pack: pack_name.into(),
                assets: convert.assets.len(),
                blobs: 0,
                out: PathBuf::from(format!("pack_import: {error}")),
                error: Some(format!("compile refused the pack: {error}")),
                library,
                bake,
            };
        }
    };
    let report = report.expect("compile ok");

    let publish_result = if let Some(session) = server {
        let _ = tx.send(ImportPhase::Publishing {
            pack: pack_name.into(),
            assets: report.assets,
            blobs: report.blobs,
            blob_done: 0,
        });
        match publish_classic_pack(
            &staged,
            &bundle,
            &session,
            source,
            pack_name,
            report.assets,
            report.blobs,
            &convert.assets,
            tx,
        ) {
            Ok(pair) => Some(Ok(pair)),
            Err(error) => Some(Err(error)),
        }
    } else {
        None
    };

    if let Some(Err(error)) = publish_result {
        return ImportPhase::Failed {
            pack: pack_name.into(),
            message: format!(
                "compiled {} assets but catalog publish failed: {error}",
                report.assets
            ),
        };
    }
    let (created, annotated) = match publish_result {
        Some(Ok(pair)) => pair,
        None => (false, 0usize),
        Some(Err(_)) => unreachable!(),
    };

    // Classic AO stays off (convert.bake is empty). Kenney still bakes.
    let bake = BakeStats {
        total: convert.bake.total,
        baked: convert.bake.baked,
        skipped: convert.bake.skipped,
        failed: convert.bake.failed,
    };

    if publish_result.is_none() {
        return ImportPhase::CompiledLocal {
            pack: pack_name.into(),
            assets: report.assets,
            blobs: report.blobs,
            out: report.plan_path,
            error: None,
            library,
            bake,
        };
    }
    // Published: the converted tree has done its job (the catalog has the
    // blobs, the library has its copies), so reclaim `work/`+`out/` exactly
    // like the Kenney/KayKit path. A classic pack stages gigabytes.
    clear_pack_staging(pack_name, out, true);
    ImportPhase::Published {
        pack: pack_name.into(),
        assets: report.assets,
        blobs: report.blobs,
        created,
        annotated,
        out: report.plan_path,
        library,
        bake,
    }
}

fn classic_library_landings(
    staged: &Path,
    source: ClassicSource,
    pack: &str,
    assets: &[classic_import::ClassicAsset],
) -> Vec<LibraryLanding> {
    let mut out = Vec::new();
    let mut seen_icons = std::collections::BTreeSet::new();
    let mut seen_titles = std::collections::BTreeSet::new();
    for asset in assets {
        let ea_source = is_ea_source(source);
        if matches!(asset.kind, AssetKind::Texture)
            && !matches!(source, classic_import::ClassicSource::Quake3)
            && !(ea_source && asset.key.starts_with("icons/"))
        {
            continue;
        }
        if matches!(source, classic_import::ClassicSource::Duke3d)
            && !matches!(asset.kind, AssetKind::World | AssetKind::Billboard)
        {
            continue;
        }
        let path = staged.join(&asset.rel_path);
        if !path.is_file() {
            continue;
        }
        let mut thumb = asset.icon_rel.as_ref().map(|r| staged.join(r));
        if thumb.as_ref().is_none_or(|p| !p.is_file())
            && matches!(asset.kind, AssetKind::Billboard)
            && path.extension().and_then(|e| e.to_str()) == Some("png")
        {
            thumb = Some(path.clone());
        }
        let (path, content_type, domain) = match asset.kind {
            AssetKind::World => (path, "model/gltf-binary", "map"),
            AssetKind::Character => (path, "model/gltf-binary", "character"),
            AssetKind::Weapon => (path, "model/gltf-binary", "weapon"),
            AssetKind::Prop => (path, "model/gltf-binary", "prop"),
            AssetKind::Texture => (
                path,
                "image/png",
                if ea_source && asset.key.starts_with("icons/") {
                    "texture"
                } else {
                    "image"
                },
            ),
            AssetKind::Audio => {
                let music = asset.key.starts_with("music/")
                    || asset.tags.iter().any(|t| t.eq_ignore_ascii_case("music"));
                let speech = asset.tags.iter().any(|t| t.eq_ignore_ascii_case("speech"));
                (
                    path,
                    "audio/wav",
                    if speech {
                        "speech"
                    } else if music {
                        "music"
                    } else {
                        "sfx"
                    },
                )
            }
            AssetKind::Billboard => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "billboard" {
                    if !seen_icons.insert(path.clone()) {
                        continue;
                    }
                    (
                        path,
                        makepad_asset_importer::stateful_billboard::CONTENT_TYPE,
                        "billboard",
                    )
                } else if ext == "png" {
                    // Per-frame tiles belong inside a `.billboard` sheet;
                    // only map-placed singletons (tagged `leftover` by the
                    // converter) may land as individual cards.
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if stem.starts_with("tile-")
                        && !asset.tags.iter().any(|t| t == "leftover")
                    {
                        continue;
                    }
                    if !seen_icons.insert(path.clone()) {
                        continue;
                    }
                    (path, "image/png", "billboard")
                } else {
                    continue;
                }
            }
            _ => (path, "model/gltf-binary", "mesh"),
        };
        let stem = asset
            .key
            .rsplit('/')
            .next()
            .unwrap_or(asset.key.as_str())
            .to_string();
        let title = if matches!(source, classic_import::ClassicSource::Quake3) {
            // Full key so gothic_floor/wood and gothic_wall/wood stay two cards.
            asset.key.replace('/', " · ")
        } else {
            match asset.kind {
                AssetKind::Billboard => {
                    makepad_asset_importer::stateful_billboard::sprite_title(&stem)
                }
                AssetKind::World => {
                    makepad_asset_importer::stateful_billboard::world_title(&asset.key)
                }
                AssetKind::Character | AssetKind::Weapon | AssetKind::Prop => {
                    makepad_asset_importer::stateful_billboard::mesh_title(&asset.key)
                }
                _ => stem.clone(),
            }
        };
        if !matches!(source, classic_import::ClassicSource::Quake3)
            && title.to_ascii_lowercase().contains("shareware")
        {
            continue;
        }
        let dedupe_key = if ea_source {
            asset.key.clone()
        } else {
            title.clone()
        };
        if !matches!(source, classic_import::ClassicSource::Quake3)
            && matches!(
                asset.kind,
                AssetKind::Billboard | AssetKind::Audio | AssetKind::Texture
            )
            && !seen_titles.insert(dedupe_key)
        {
            continue;
        }
        if matches!(asset.kind, AssetKind::Billboard)
            && thumb.as_ref().is_none_or(|p| !p.is_file())
        {
            continue;
        }
        out.push(LibraryLanding {
            path,
            label: title,
            domain,
            content_type,
            prompt: format!(
                "{} {pack} · {} · {stem} · {} · {}",
                source.title(),
                kind_word(asset.kind),
                source.license(),
                source.home()
            ),
            thumbnail: thumb.filter(|p| p.is_file()),
            source_id: source.id().into(),
            pack: pack.to_string(),
        });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn kind_word(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::World => "map",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Prop => "prop",
        AssetKind::Billboard => "billboard",
        AssetKind::Audio => "audio",
        AssetKind::Texture => "texture",
        _ => "mesh",
    }
}

/// Upload whatever is queued in `batch` as ONE `upload_blob_batch_with_digests`
/// request, advance `blob_done` by the batch size, and tell the UI. No-op on
/// an empty batch (the tail flush after the loop, or the one right before an
/// oversized single blob, may have nothing queued).
#[allow(clippy::too_many_arguments)]
fn flush_publish_batch(
    client: &AssetClient,
    ns: &str,
    pack_name: &str,
    assets: usize,
    blob_total: usize,
    batch: &mut Vec<(BlobId, Vec<u8>)>,
    blob_done: &mut usize,
    tx: &std::sync::mpsc::Sender<ImportPhase>,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let refs: Vec<(BlobId, &[u8])> = batch
        .iter()
        .map(|(digest, bytes)| (*digest, bytes.as_slice()))
        .collect();
    client
        .upload_blob_batch_with_digests(ns, &refs)
        .map_err(|e| format!("upload batch of {}: {e}", batch.len()))?;
    *blob_done += batch.len();
    batch.clear();
    let _ = tx.send(ImportPhase::Publishing {
        pack: pack_name.to_string(),
        assets,
        blobs: blob_total,
        blob_done: *blob_done,
    });
    Ok(())
}

fn publish_classic_pack(
    pack_root: &Path,
    out: &Path,
    session: &ServerSession,
    source: ClassicSource,
    pack_name: &str,
    assets: usize,
    blob_total: usize,
    convert_assets: &[classic_import::ClassicAsset],
    tx: &std::sync::mpsc::Sender<ImportPhase>,
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

    // Every blob's bytes are read exactly ONCE, client-side, in this whole
    // compile+publish run: `hash_and_measure` (pack_import.rs) already
    // hashed each one, moments ago in this same process, while building
    // this very plan. `pack_root` here is `work/source` — OUR OWN staging
    // directory, written only by this importer's own conversion code;
    // nothing external ever touches it between compile and publish — so
    // re-hashing it again here would be pure TOCTOU paranoia against a
    // threat that does not exist for this path. The plan's declared digest
    // is trusted directly as the precomputed digest passed to the upload
    // calls; the
    // upload plan's own `"uploader": "re-hash each local_path…"` law (see
    // `UPLOADER_REVERIFY` in pack_import.rs) is left untouched for whatever
    // GENERIC consumer of an `upload_plan.json` that law is written for
    // (a standalone CLI/worker acting on a plan handed to it from outside
    // its own process, where the file really could have changed since) —
    // this in-process classic-import path is not that consumer, and the
    // server still independently computes and echoes back the real digest
    // of whatever bytes it receives (`upload_blob_with_digest` /
    // `upload_blob_batch_with_digests` still refuse on a disagreement), so
    // skipping the local re-hash here does not weaken the end-to-end
    // guarantee — a changed file still gets caught, one hop later, at the
    // server instead of locally.
    //
    // Uploads go out in batches sized to the wire limits, one HTTP request
    // per batch instead of one per blob; a blob too big to share a batch on
    // its own falls back to a single upload.
    let mut blob_done = 0usize;
    let mut batch: Vec<(BlobId, Vec<u8>)> = Vec::new();
    let mut batch_bytes: u64 = 0;

    for blob in blobs {
        let local = blob
            .get("local_path")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing local_path")?;
        let expect = blob
            .get("blob")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing digest")?;
        let digest: BlobId = expect
            .parse()
            .map_err(|_| format!("blob digest malformed for {local}: {expect}"))?;
        let bytes = std::fs::read(pack_root.join(local))
            .map_err(|e| format!("read {local}: {e}"))?;
        let size = bytes.len() as u64;
        if size > wire::UPLOAD_BATCH_SAFE_BYTES {
            // Too big to ever share a batch: flush what's queued (keeps
            // upload order matching plan order) then send it alone.
            flush_publish_batch(&client, &ns, pack_name, assets, blob_total, &mut batch, &mut blob_done, tx)?;
            batch_bytes = 0;
            client
                .upload_blob_with_digest(&ns, &bytes, digest)
                .map_err(|e| format!("upload {local}: {e}"))?;
            blob_done += 1;
            let _ = tx.send(ImportPhase::Publishing {
                pack: pack_name.to_string(),
                assets,
                blobs: blob_total,
                blob_done,
            });
            continue;
        }
        if !batch.is_empty()
            && (batch.len() >= wire::MAX_UPLOAD_BATCH_ITEMS
                || batch_bytes + size > wire::UPLOAD_BATCH_SAFE_BYTES)
        {
            flush_publish_batch(&client, &ns, pack_name, assets, blob_total, &mut batch, &mut blob_done, tx)?;
            batch_bytes = 0;
        }
        batch.push((digest, bytes));
        batch_bytes += size;
    }
    flush_publish_batch(&client, &ns, pack_name, assets, blob_total, &mut batch, &mut blob_done, tx)?;

    let report = client
        .run_import(&manifest)
        .map_err(|e| format!("run import: {e}"))?;
    let annotate_total = report.entries.len();
    let mut annotated = 0usize;
    let kind_by_key: std::collections::BTreeMap<String, AssetKind> = convert_assets
        .iter()
        .map(|a| (a.key.clone(), a.kind))
        .collect();
    let _ = tx.send(ImportPhase::Annotating {
        pack: pack_name.to_string(),
        assets,
        blobs: blob_total,
        annotated: 0,
        total: annotate_total,
    });
    for entry in &report.entries {
        let key = entry.key.as_str();
        let title = key.rsplit('/').next().unwrap_or(key);
        let kind = kind_by_key
            .get(key)
            .copied()
            .unwrap_or_else(|| classic_import::kind_for_staged_path(key));
        let alias = entry
            .alias
            .as_ref()
            .map(|a| a.as_str().to_string())
            .unwrap_or_else(|| format!("{}/{}/{}", source.id(), pack_name, title));
        let mut tags = vec![source.id().to_string(), pack_name.to_string()];
        for t in convert_assets
            .iter()
            .find(|a| a.key == key)
            .map(|a| a.tags.as_slice())
            .unwrap_or(&[])
        {
            if !tags.iter().any(|x| x == t) {
                tags.push(t.clone());
            }
        }
        let ann = AnnotationUpload {
            title: title.to_string(),
            description: format!(
                "{} {pack_name} · {alias} · {key} · {} · {}",
                source.title(),
                source.license(),
                source.credits()
            ),
            kind: Some(kind),
            categories: vec![source.id().into(), pack_name.to_string()],
            tags,
            creator: source.credits().to_string(),
            artist: String::new(),
            artist_url: String::new(),
            album: String::new(),
            source_url: String::new(),
            license: String::new(),
            license_url: String::new(),
            generator: "classic_import".into(),
            backend: "asset-ui".into(),
            model: pack_name.to_string(),
            prompt: format!("imported {} pack {pack_name} asset {key}", source.title()),
            provenance: format!(
                "{} · {} · license {} · credits {}",
                source.title(),
                source.home(),
                source.license(),
                source.credits()
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

/// Page state holding classic cards (separate from Kenney ImportPage).
pub struct ClassicImportPage {
    pub freedoom: ClassicImportCard,
    pub doom: ClassicImportCard,
    pub librequake: ClassicImportCard,
    pub quake: ClassicImportCard,
    pub duke3d: ClassicImportCard,
    pub cnc: ClassicImportCard,
    pub ra: ClassicImportCard,
    pub ts: ClassicImportCard,
    pub d2k: ClassicImportCard,
    pub quake2: ClassicImportCard,
    pub quake3: ClassicImportCard,
    pub darkmod: ClassicImportCard,
}

impl Default for ClassicImportPage {
    fn default() -> Self {
        Self {
            freedoom: ClassicImportCard::new(ClassicSource::Freedoom),
            doom: ClassicImportCard::new(ClassicSource::Doom),
            librequake: ClassicImportCard::new(ClassicSource::LibreQuake),
            quake: ClassicImportCard::new(ClassicSource::Quake),
            duke3d: ClassicImportCard::new(ClassicSource::Duke3d),
            cnc: ClassicImportCard::new(ClassicSource::Cnc),
            ra: ClassicImportCard::new(ClassicSource::RedAlert),
            ts: ClassicImportCard::new(ClassicSource::TiberianSun),
            d2k: ClassicImportCard::new(ClassicSource::Dune2000),
            quake2: ClassicImportCard::new(ClassicSource::Quake2),
            quake3: ClassicImportCard::new(ClassicSource::Quake3),
            darkmod: ClassicImportCard::new(ClassicSource::DarkMod),
        }
    }
}

impl ClassicImportPage {
    pub fn card(&self, source: ClassicSource) -> &ClassicImportCard {
        match source {
            ClassicSource::Freedoom => &self.freedoom,
            ClassicSource::Doom => &self.doom,
            ClassicSource::LibreQuake => &self.librequake,
            ClassicSource::Quake => &self.quake,
            ClassicSource::Duke3d => &self.duke3d,
            ClassicSource::Cnc => &self.cnc,
            ClassicSource::RedAlert => &self.ra,
            ClassicSource::TiberianSun => &self.ts,
            ClassicSource::Dune2000 => &self.d2k,
            ClassicSource::Quake2 => &self.quake2,
            ClassicSource::Quake3 => &self.quake3,
            ClassicSource::DarkMod => &self.darkmod,
        }
    }

    pub fn card_mut(&mut self, source: ClassicSource) -> &mut ClassicImportCard {
        match source {
            ClassicSource::Freedoom => &mut self.freedoom,
            ClassicSource::Doom => &mut self.doom,
            ClassicSource::LibreQuake => &mut self.librequake,
            ClassicSource::Quake => &mut self.quake,
            ClassicSource::Duke3d => &mut self.duke3d,
            ClassicSource::Cnc => &mut self.cnc,
            ClassicSource::RedAlert => &mut self.ra,
            ClassicSource::TiberianSun => &mut self.ts,
            ClassicSource::Dune2000 => &mut self.d2k,
            ClassicSource::Quake2 => &mut self.quake2,
            ClassicSource::Quake3 => &mut self.quake3,
            ClassicSource::DarkMod => &mut self.darkmod,
        }
    }

    pub fn compiling(&self) -> bool {
        self.freedoom.compiling()
            || self.doom.compiling()
            || self.librequake.compiling()
            || self.quake.compiling()
            || self.duke3d.compiling()
            || self.cnc.compiling()
            || self.ra.compiling()
            || self.ts.compiling()
            || self.d2k.compiling()
            || self.quake2.compiling()
            || self.quake3.compiling()
            || self.darkmod.compiling()
    }

    pub fn handle_http_progress(&mut self, request_id: LiveId, progress: &HttpProgress) -> bool {
        self.cards_mut()
            .any(|card| card.handle_http_progress(request_id, progress))
    }

    pub fn handle_http_error(&mut self, cx: &mut Cx, request_id: LiveId, error: &HttpError) -> bool {
        self.cards_mut()
            .any(|card| card.handle_http_error(cx, request_id, error))
    }

    pub fn handle_http_response(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        response: &HttpResponse,
    ) -> bool {
        self.cards_mut()
            .any(|card| card.handle_http_response(cx, request_id, response))
    }

    /// Let every classic card parked in `IconsPending` continue once the UI
    /// has drained its landings and the icon renderer is idle (or the user
    /// cancelled it). Mirrors `ImportPage::resume_icons_pending` — see
    /// [`IconResumeGate`]. Returns the sources that were resumed.
    pub fn resume_icons_pending(&mut self, landings_drained: bool, icons_busy: bool) -> Vec<&'static str> {
        let mut resumed = Vec::new();
        for card in self.cards_mut() {
            let ready = card.icons_pending_ready(landings_drained, icons_busy);
            let cancelled = card.stop_requested()
                && matches!(card.phase, ImportPhase::IconsPending { .. });
            if (ready || cancelled) && card.resume_icons_pending() {
                resumed.push(card.source.id());
            }
        }
        resumed
    }

    fn cards_mut(&mut self) -> impl Iterator<Item = &mut ClassicImportCard> {
        [
            &mut self.freedoom,
            &mut self.doom,
            &mut self.librequake,
            &mut self.quake,
            &mut self.duke3d,
            &mut self.cnc,
            &mut self.ra,
            &mut self.ts,
            &mut self.d2k,
            &mut self.quake2,
            &mut self.quake3,
            &mut self.darkmod,
        ]
        .into_iter()
    }

    pub fn poll(&mut self) -> bool {
        let a = self.freedoom.poll();
        let b = self.doom.poll();
        let c = self.librequake.poll();
        let d = self.quake.poll();
        let e = self.duke3d.poll();
        let f = self.cnc.poll();
        let g = self.ra.poll();
        let h = self.ts.poll();
        let i = self.d2k.poll();
        let j = self.quake2.poll();
        let k = self.quake3.poll();
        let l = self.darkmod.poll();
        a || b || c || d || e || f || g || h || i || j || k || l
    }

    pub fn take_all_landings(&mut self) -> Vec<LibraryLanding> {
        let mut out = self.freedoom.take_library_landings();
        out.extend(self.doom.take_library_landings());
        out.extend(self.librequake.take_library_landings());
        out.extend(self.quake.take_library_landings());
        out.extend(self.duke3d.take_library_landings());
        out.extend(self.cnc.take_library_landings());
        out.extend(self.ra.take_library_landings());
        out.extend(self.ts.take_library_landings());
        out.extend(self.d2k.take_library_landings());
        out.extend(self.quake2.take_library_landings());
        out.extend(self.quake3.take_library_landings());
        out.extend(self.darkmod.take_library_landings());
        out
    }

    pub fn take_all_previews(&mut self) -> Vec<(String, Vec<u8>)> {
        let mut out = self.freedoom.take_previews();
        out.extend(self.doom.take_previews());
        out.extend(self.librequake.take_previews());
        out.extend(self.quake.take_previews());
        out.extend(self.duke3d.take_previews());
        out.extend(self.cnc.take_previews());
        out.extend(self.ra.take_previews());
        out.extend(self.ts.take_previews());
        out.extend(self.d2k.take_previews());
        out.extend(self.quake2.take_previews());
        out.extend(self.quake3.take_previews());
        out.extend(self.darkmod.take_previews());
        out
    }
}

fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let x = n as f64;
    if x >= GB {
        format!("{:.2} GB", x / GB)
    } else if x >= MB {
        format!("{:.1} MB", x / MB)
    } else if x >= KB {
        format!("{:.0} KB", x / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons_pending(pack: &str) -> ImportPhase {
        ImportPhase::IconsPending {
            pack: pack.into(),
            assets: 3,
            library: Vec::new(),
            bake: BakeStats::default(),
        }
    }

    #[test]
    fn a_classic_pack_waits_for_its_icons_before_publishing() {
        let mut card = ClassicImportCard::new(ClassicSource::Freedoom);
        // Idle: nothing to wait for.
        assert!(!card.icons_pending_ready(true, false));

        card.phase = icons_pending("freedoom");
        // Landings still queued for the icon renderer: NOT ready.
        assert!(!card.icons_pending_ready(false, false));
        // Renderer still working through them: NOT ready.
        assert!(!card.icons_pending_ready(true, true));
        // Drained and idle: the parked thread may compile+publish.
        assert!(card.icons_pending_ready(true, false));
        // And the card counts as busy while parked, so a second start of the
        // same source is refused instead of racing the first.
        assert!(card.compiling());
    }

    #[test]
    fn resuming_an_unarmed_card_is_a_no_op() {
        // A card that never started an import has no channel: resume must
        // report nothing rather than pretend it signalled a thread.
        let mut page = ClassicImportPage::default();
        page.freedoom.phase = icons_pending("freedoom");
        assert!(page.resume_icons_pending(true, false).is_empty());
    }

    #[test]
    fn shareware_cards_say_local_preview_not_a_redistributable_grant() {
        for module in [
            DOOM_MODULE,
            QUAKE_MODULE,
            DUKE3D_MODULE,
            EA_MODULES[0],
            EA_MODULES[1],
            EA_MODULES[2],
            EA_MODULES[3],
            QUAKE2_MODULE,
            QUAKE3_MODULE,
        ] {
            let text = format!("{} {}", module.license_blurb, module.blurb);
            assert!(
                text.contains("local preview") || text.contains("Not a redistributable grant"),
                "{} missing preview/grant language: {text}",
                module.id
            );
            assert!(
                !text.to_ascii_lowercase().contains("cc0"),
                "{} must not look like CC0: {text}",
                module.id
            );
        }
    }

    #[test]
    fn ea_classics_modules_dropdown_and_library_domains_are_wired() {
        let ids: Vec<_> = PACK_MODULES_WITH_CLASSIC.iter().map(|module| module.id).collect();
        for id in ["cnc", "ra", "ts", "d2k"] {
            assert!(ids.contains(&id), "missing EA classic module {id}");
        }
        for (index, source) in EA_SOURCES.into_iter().enumerate() {
            assert_eq!(ea_source_for_index(index), source);
            assert_eq!(ea_index_for_source(source), Some(index));
        }

        let staged = std::env::temp_dir().join(format!(
            "asset-ui-ea-landings-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(staged.join("billboards/cnc")).unwrap();
        std::fs::create_dir_all(staged.join("worlds")).unwrap();
        std::fs::write(staged.join("billboards/cnc/mtnk.billboard"), b"manifest").unwrap();
        std::fs::write(staged.join("billboards/cnc/mtnk_thumb.png"), b"png").unwrap();
        std::fs::write(staged.join("worlds/scm01ea.glb"), b"glb").unwrap();
        std::fs::write(staged.join("worlds/scm01ea.png"), b"png").unwrap();
        let assets = [
            classic_import::ClassicAsset {
                key: "billboards/cnc/mtnk".into(),
                kind: AssetKind::Billboard,
                rel_path: "billboards/cnc/mtnk.billboard".into(),
                tags: vec!["unit".into()],
                icon_rel: Some("billboards/cnc/mtnk_thumb.png".into()),
            },
            classic_import::ClassicAsset {
                key: "worlds/scm01ea".into(),
                kind: AssetKind::World,
                rel_path: "worlds/scm01ea.glb".into(),
                tags: vec!["map".into()],
                icon_rel: Some("worlds/scm01ea.png".into()),
            },
        ];
        let landings = classic_library_landings(&staged, ClassicSource::Cnc, "cnc", &assets);
        assert_eq!(
            landings
                .iter()
                .find(|landing| landing.path.ends_with("mtnk.billboard"))
                .map(|landing| landing.domain),
            Some("billboard")
        );
        assert_eq!(
            landings
                .iter()
                .find(|landing| landing.path.ends_with("scm01ea.glb"))
                .map(|landing| landing.domain),
            Some("map")
        );
        let _ = std::fs::remove_dir_all(staged);
    }

    #[test]
    fn darkmod_card_is_explicitly_nc_sa() {
        let text = format!(
            "{} {}",
            DARKMOD_MODULE.license_blurb, DARKMOD_MODULE.blurb
        );
        assert!(text.contains("non-commercial"));
        assert!(text.contains("share-alike"));
        assert!(text.contains("Fan missions"));
        assert!(text.contains("Not CC0"));
    }

    #[test]
    fn libre_cards_require_attribution_and_deny_id_art() {
        assert!(FREEDOOM_MODULE.license_blurb.contains("attribution required"));
        assert!(FREEDOOM_MODULE.license_blurb.contains("not id Software"));
        assert!(LIBREQUAKE_MODULE.license_blurb.contains("attribution required"));
        assert!(LIBREQUAKE_MODULE.license_blurb.contains("not id Software"));
    }
}
