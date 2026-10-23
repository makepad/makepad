//! First-use install of the files a host runs in-process: model weights,
//! SoundFonts, fonts, opt-in datasets.
//!
//! A file is described once ([`Installable`]: a pinned URL or HuggingFace
//! file, its size and SHA-256, its licence and what it may be part of) and
//! the downloader of `makepad-ai-hub` fetches it: resumable `.part` files,
//! pinned size + sha256, atomic commit, a receipt beside the file and a
//! cross-process lock per artifact. Where it lands is under one of two
//! roots: the checkout's `local/` tree ([`Root::Checkout`]) or the AI Hub's
//! weights folder ([`Root::Weights`]), beside the Kokoro voices, outside
//! every repository.
//!
//! Nothing downloads until the host asks, and the host asks only after the
//! operator confirmed a dialog naming the files, their sizes and their
//! licences. The confirmation is recorded: every install writes a provenance
//! record beside the file saying what was fetched, from where, under which
//! licence, for which use, and when the operator acknowledged that.
//!
//! The install worker, [`verify`] and [`uninstall`] need the `download`
//! feature (on by default); the description and the path logic do not.

use std::path::{Path, PathBuf};

#[cfg(feature = "download")]
mod worker;
#[cfg(feature = "download")]
pub use worker::{start_install, uninstall, verify, InstallHandle, InstallMsg};

/// What a file may be part of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUse {
    /// Weights under a licence that allows a product to carry them.
    Commercial,
    /// Weights with no licence of their own, or trained on data whose terms
    /// stop at research: fine on a developer's machine, never in a build
    /// anybody pays for.
    DevelopmentOnly,
    /// A library the person opts into (a dataset of songs, samples, ...):
    /// never shipped with the product; fetched only when they install it,
    /// after its notice and licence are shown, and credited per item.
    Dataset,
}

impl ModelUse {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelUse::Commercial => "commercial",
            ModelUse::DevelopmentOnly => "development-only",
            ModelUse::Dataset => "dataset",
        }
    }
}

/// Which folder an [`Installable`]'s `cache_as` is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    /// The checkout's `local/` tree ([`cache_root`]).
    Checkout,
    /// The AI Hub's weights folder ([`weights_root`]).
    Weights,
}

/// One installable file: identity for the ack record, provenance for the
/// log, and the pinned download the shared downloader verifies against.
pub struct Installable {
    /// Stable id the ack file records.
    pub id: &'static str,
    /// Short name for the one-line progress display.
    pub short: &'static str,
    pub license: &'static str,
    /// The licence's full text as its origin publishes it, for the install
    /// dialog; empty when the name says enough.
    pub license_text: &'static str,
    /// Where the file and its licence come from, in words.
    pub origin: &'static str,
    /// What the file may go into.
    pub use_: ModelUse,
    pub bytes: u64,
    pub sha256: &'static str,
    /// HuggingFace repo, empty when `path` is an absolute URL.
    pub repo: &'static str,
    /// Repo-relative file, or an absolute URL fetched verbatim.
    pub path: &'static str,
    /// The repo revision the pinned hash was read at; empty for the head.
    /// A repo that moves on must not turn an install into a hash mismatch.
    pub revision: &'static str,
    /// Destination under the file's [`Root`], '/'-separated.
    pub cache_as: &'static str,
    pub root: Root,
    /// The copy this machine actually uses, when that is not simply the
    /// managed destination: an environment override, an alternate probe path.
    /// `Some(path)` means installed there; `None` means not installed.
    /// Without a hook the managed destination at its pinned size decides.
    pub found: Option<fn() -> Option<PathBuf>>,
}

impl Installable {
    /// A file a host fetches verbatim from `url` into the weights folder
    /// ([`Root::Weights`]) under a licence that allows a product to carry
    /// it: what a host lists beside the models (the fonts documents name).
    /// `id` is also its `cache_as`.
    #[allow(clippy::too_many_arguments)]
    pub const fn resource(short: &'static str, license: &'static str, license_text: &'static str, origin: &'static str, bytes: u64, sha256: &'static str, url: &'static str, cache_as: &'static str) -> Installable {
        Installable { id: cache_as, short, license, license_text, origin, use_: ModelUse::Commercial, bytes, sha256, repo: "", path: url, revision: "", cache_as, root: Root::Weights, found: None }
    }

    /// A library file the person opts into ([`ModelUse::Dataset`]), fetched
    /// verbatim from `url` into the weights folder when they install it (the
    /// notice in `license_text` shown first): the MIDI song library. `id` is
    /// also its `cache_as`.
    #[allow(clippy::too_many_arguments)]
    pub const fn dataset(short: &'static str, license: &'static str, license_text: &'static str, origin: &'static str, bytes: u64, sha256: &'static str, url: &'static str, cache_as: &'static str) -> Installable {
        Installable { id: cache_as, short, license, license_text, origin, use_: ModelUse::Dataset, bytes, sha256, repo: "", path: url, revision: "", cache_as, root: Root::Weights, found: None }
    }

    /// The destination under `root`, '/'-separated parts joined natively.
    pub fn dest_under(&self, root: &Path) -> PathBuf {
        let mut out = root.to_path_buf();
        for part in self.cache_as.split('/') {
            out.push(part);
        }
        out
    }
}

/// The checkout's `local/` tree.
pub fn cache_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local")
}

/// The AI Hub's weights folder: `MAKEPAD_ASSET_AI_CACHE`, else the Makepad
/// home's `weights/` (`MAKEPAD_HOME`, else `~/.makepad`), as
/// `makepad_ai_hub::home` resolves it. Needs no file system to answer: on
/// the web it is a relative path that is never there.
pub fn weights_root() -> PathBuf {
    let set = |key: &str| std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from);
    set("MAKEPAD_ASSET_AI_CACHE").unwrap_or_else(|| {
        set("MAKEPAD_HOME")
            .or_else(|| set("USERPROFILE").or_else(|| set("HOME")).map(|home| home.join(".makepad")))
            .unwrap_or_else(|| PathBuf::from(".makepad"))
            .join("weights")
    })
}

/// The folder an installable's `cache_as` is under.
pub fn root_of(model: &Installable) -> PathBuf {
    match model.root {
        Root::Checkout => cache_root(),
        Root::Weights => weights_root(),
    }
}

/// Where a file lands under the managed root.
pub fn dest_path(model: &Installable) -> PathBuf {
    model.dest_under(&root_of(model))
}

/// The provenance record kept beside a file.
pub fn provenance_path(weights: &Path) -> PathBuf {
    let mut name = weights.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".provenance");
    weights.with_file_name(name)
}

/// Seconds since the epoch, or zero on a clock that cannot say.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where a file is fetched from: its URL, or `repo/path` on HuggingFace.
pub fn source_url(model: &Installable) -> String {
    if model.repo.is_empty() {
        model.path.to_string()
    } else {
        format!("{}/{}", model.repo, model.path)
    }
}

/// What an install writes down about what it fetched: the one place a
/// release audit reads to know which files are on a machine, under which
/// terms, and whether anybody said yes to them.
pub fn provenance_text(model: &Installable, weights: &Path, acknowledged: Option<u64>) -> String {
    let source = source_url(model);
    let acknowledged = acknowledged.map(|at| at.to_string()).unwrap_or_else(|| "none".to_string());
    let mut out = String::new();
    for (key, value) in [
        ("id", model.id.to_string()),
        ("file", weights.display().to_string()),
        ("bytes", model.bytes.to_string()),
        ("sha256", model.sha256.to_string()),
        ("source", source),
        ("license", model.license.to_string()),
        ("use", model.use_.as_str().to_string()),
        ("installed", unix_now().to_string()),
        ("acknowledged", acknowledged),
    ] {
        out.push_str(key);
        out.push(' ');
        out.push_str(&value.replace('\n', " "));
        out.push('\n');
    }
    out
}

/// The record an earlier install left beside the file, as `key value`
/// pairs; empty when there is none.
pub fn read_provenance(weights: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(provenance_path(weights))
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once(' '))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A managed install only counts when the bytes on disk are exactly the
/// pinned artifact — `is_file()` alone let a truncated or hand-copied file
/// read as provisioned, and then load broken. One stat, no hashing (the
/// downloader's fsync-then-rename commit means a file of the right length
/// at the dest went through the sha check).
pub fn dest_is_installed(model: &Installable) -> bool {
    file_is_pinned_size(&dest_path(model), model.bytes)
}

pub fn file_is_pinned_size(path: &Path, bytes: u64) -> bool {
    std::fs::metadata(path).map(|meta| meta.len() == bytes).unwrap_or(false)
}

/// Whether a file is on this machine, by the resolution its consumer uses:
/// its [`Installable::found`] hook when it has one (env overrides and
/// alternate probe paths count as installed, the operator's own hands are
/// trusted), else the managed destination at its pinned size.
pub fn installed(model: &Installable) -> bool {
    match model.found {
        Some(found) => found().is_some(),
        None => dest_is_installed(model),
    }
}

/// Where a file is read from on this machine: the copy its hook found, the
/// managed destination otherwise. The provenance record is looked for
/// beside this file.
pub fn weights_path(model: &Installable) -> PathBuf {
    model.found.and_then(|found| found()).unwrap_or_else(|| dest_path(model))
}

/// The files of `wanted` this machine still lacks. Each host names the files
/// it runs: a console that never separates with the vocals model must never
/// offer its download, nor record an acknowledgement for it.
pub fn missing_for(wanted: &[&'static Installable]) -> Vec<&'static Installable> {
    wanted.iter().copied().filter(|model| !installed(model)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_file_never_reads_as_installed() {
        let dir = std::env::temp_dir().join(format!("model-install-gate-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("model.bin");
        assert!(!file_is_pinned_size(&path, 8), "absent file counted as installed");
        std::fs::write(&path, b"1234").unwrap();
        assert!(!file_is_pinned_size(&path, 8), "truncated file counted as installed");
        std::fs::write(&path, b"12345678").unwrap();
        assert!(file_is_pinned_size(&path, 8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resource_lands_in_the_weights_folder_under_its_own_path() {
        const FONT: Installable = Installable::resource("Archivo", "SIL Open Font License 1.1", "", "", 4, "00", "https://example.invalid/a.ttf", "fonts/archivo/Archivo-Variable.ttf");
        assert_eq!(dest_path(&FONT), weights_root().join("fonts").join("archivo").join("Archivo-Variable.ttf"));
        assert_eq!(source_url(&FONT), "https://example.invalid/a.ttf");
        assert_eq!(FONT.id, FONT.cache_as);
        assert_eq!(FONT.use_, ModelUse::Commercial);
        assert!(!dest_path(&FONT).starts_with(cache_root()), "a resource outside the checkout");
    }

    #[test]
    fn a_dataset_is_marked_as_one_and_hf_files_name_their_repo() {
        const SET: Installable = Installable::dataset("set", "CC BY 4.0", "notice", "somewhere", 9, "00", "https://example.invalid/set.tar", "datasets/set.tar");
        assert_eq!(SET.use_, ModelUse::Dataset);
        assert_eq!(ModelUse::Dataset.as_str(), "dataset");
        let hf = Installable { repo: "org/name", path: "w.bin", ..Installable::resource("w", "MIT", "", "", 1, "00", "", "m/w.bin") };
        assert_eq!(source_url(&hf), "org/name/w.bin");
    }

    #[test]
    fn the_checkout_root_is_the_repository_local_tree() {
        assert!(cache_root().ends_with("libs/model_install/../../local"));
        assert!(cache_root().join("../libs/model_install/Cargo.toml").is_file());
    }

    #[test]
    fn a_hook_decides_where_the_file_is_read_from() {
        fn elsewhere() -> Option<PathBuf> {
            Some(PathBuf::from("/elsewhere/model.bin"))
        }
        fn nowhere() -> Option<PathBuf> {
            None
        }
        let base = Installable::resource("m", "MIT", "", "", 1, "00", "", "m/model.bin");
        let there = Installable { found: Some(elsewhere), ..Installable::resource("m", "MIT", "", "", 1, "00", "", "m/model.bin") };
        let absent = Installable { found: Some(nowhere), ..base };
        assert!(installed(&there) && weights_path(&there) == PathBuf::from("/elsewhere/model.bin"));
        assert!(!installed(&absent) && weights_path(&absent) == dest_path(&absent));
    }

    #[test]
    fn the_provenance_record_round_trips() {
        let model = Installable::resource("m", "MIT", "", "origin", 4, "ab", "https://example.invalid/m.bin", "m/m.bin");
        let dir = std::env::temp_dir().join(format!("model-install-prov-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let weights = dir.join("m.bin");
        std::fs::write(provenance_path(&weights), provenance_text(&model, &weights, Some(7))).unwrap();
        let record = read_provenance(&weights);
        let get = |key: &str| record.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!((get("id"), get("use"), get("acknowledged"), get("license")), (Some("m/m.bin"), Some("commercial"), Some("7"), Some("MIT")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
