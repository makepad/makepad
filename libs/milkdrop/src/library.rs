//! Preset packs on disk: an index to browse and search, the person's
//! favourites and ratings, and the on-demand download of the projectM
//! "cream of the crop" pack.
//!
//! Presets are never part of the build. A host names a data directory it
//! owns (the app's own data folder); [`download_pack`] fetches the pack's
//! zip from GitHub into it, and [`scan_cached`] indexes whatever `.milk`
//! files are there, remembering what it read in a cache file so the next
//! walk reads only new or changed presets. Both block and do file I/O: run
//! them on a worker thread, never on the UI thread. Filtering, the category
//! tree, stepping and shuffling ([`browse`]) are cheap and can run anywhere.
//!
//! The pack's folders are its categories (`Fractal/Sierpinski/...`); what a
//! file name says about title and authors is read by [`names`].

mod browse;
mod index;
mod names;
mod user;

pub use browse::{step, CategoryNode, Filter, Shuffle};
pub use index::{relative, scan_cached, sniff, MilkVersion, ScanStats, Sniff};
pub use names::{parse_name, PresetName};
pub use user::{UserPresets, RECENT_KEPT};

use makepad_widgets::makepad_platform::makepad_network::blocking_http::{
    request_to_writer_no_redirect, CancelToken, Limits, Request,
};
use makepad_zip_file::zip_read_central_directory;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The projectM "cream of the crop" pack (9,795 presets, about 160 MB
/// unpacked), as a zip of its default branch. GitHub's codeload host serves
/// it without a redirect, which the one-hop HTTP client requires.
pub const CREAM_OF_THE_CROP_ZIP: &str =
    "https://codeload.github.com/projectM-visualizer/presets-cream-of-the-crop/zip/refs/heads/master";

/// The directory name the pack unpacks to inside the host's data folder.
pub const CREAM_OF_THE_CROP_DIR: &str = "cream-of-the-crop";

/// One preset file, as the index knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetEntry {
    pub path: PathBuf,
    /// The path under the library root with `/` separators: the stable name
    /// favourites, ratings and the index cache use.
    pub rel: String,
    /// First folder under the library root ("Fractal"), empty at the root.
    pub category: String,
    /// The folders below the category, joined with '/'.
    pub subcategory: String,
    /// File stem.
    pub name: String,
    /// The name without its authors and edit notes.
    pub title: String,
    pub authors: Vec<String>,
    pub editors: Vec<String>,
    pub version: MilkVersion,
    pub warp_shader: bool,
    pub comp_shader: bool,
    /// Title, authors, editors and folders folded for search (case and
    /// accents do not matter).
    search: String,
}

#[derive(Clone, Debug, Default)]
pub struct PresetLibrary {
    pub root: PathBuf,
    pub entries: Vec<PresetEntry>,
}

impl PresetEntry {
    fn new(path: PathBuf, rel: String, sniffed: Sniff) -> PresetEntry {
        let folders: Vec<&str> = rel.split('/').collect();
        let folders = &folders[..folders.len().saturating_sub(1)];
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let PresetName { title, authors, editors } = parse_name(&name);
        let category = folders.first().map(|c| c.to_string()).unwrap_or_default();
        let subcategory = folders.iter().skip(1).copied().collect::<Vec<_>>().join("/");
        let search = makepad_search::fold_words(&format!("{} {} {} {}", name, authors.join(" "), editors.join(" "), folders.join(" ")));
        PresetEntry {
            path,
            rel,
            category,
            subcategory,
            name,
            title,
            authors,
            editors,
            version: sniffed.version,
            warp_shader: sniffed.warp_shader,
            comp_shader: sniffed.comp_shader,
            search,
        }
    }

    /// A preset of the pack's "! Transition" folder: made to fade between
    /// presets (most of them to black), not to be watched on its own.
    pub fn is_transition(&self) -> bool {
        self.category.starts_with('!')
    }

    /// Authors as one line (`Flexi, Martin`), or empty.
    pub fn authors_text(&self) -> String {
        self.authors.join(", ")
    }
}

/// What a download is doing, reported as it goes.
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadEvent {
    /// Bytes of the zip received (and the total, when the server says).
    Downloading { received: u64, total: Option<u64> },
    /// Files unpacked so far out of the archive's entries.
    Unpacking { done: usize, total: usize },
    /// Finished: presets now in `dir`.
    Done { presets: usize, dir: PathBuf },
}

/// Download the pack at `url` (a zip) and unpack its `.milk` files into
/// `data_dir/dir_name`, reporting progress through `on_event`. Blocking.
///
/// Atomic: the zip goes to a temporary file and unpacks into a sibling
/// directory that is renamed over the destination only when complete, so an
/// interrupted download leaves the previous pack (or nothing) in place.
/// Paths inside the archive are taken apart and only plain names are
/// joined, so no entry can land outside the destination. `cancel` stops it
/// at the next network or file step.
pub fn download_pack(
    url: &str,
    data_dir: &Path,
    dir_name: &str,
    cancel: CancelToken,
    mut on_event: impl FnMut(DownloadEvent),
) -> Result<usize, String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let zip_path = data_dir.join(format!("{dir_name}.zip.part"));
    let received = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(u64::MAX));
    let limits = Limits {
        max_body_bytes: 1 << 30,
        idle_timeout: Duration::from_secs(120),
        ..Limits::default()
    };
    let request = {
        let (received, total) = (received.clone(), total.clone());
        Request::get(url).limits(limits).cancel_token(cancel.clone()).on_body_progress(move |got, all| {
            received.store(got, Ordering::Relaxed);
            total.store(all.unwrap_or(u64::MAX), Ordering::Relaxed);
        })
    };
    // Progress arrives on the transfer's thread; report it from a small
    // writer wrapper instead, on this thread, as bytes are written.
    struct Progress<'a, W: Write, F: FnMut(DownloadEvent)> {
        inner: W,
        received: Arc<AtomicU64>,
        total: Arc<AtomicU64>,
        on_event: &'a mut F,
        last: u64,
    }
    impl<W: Write, F: FnMut(DownloadEvent)> Write for Progress<'_, W, F> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(buf)?;
            let got = self.received.load(Ordering::Relaxed);
            if got >= self.last + (1 << 20) {
                self.last = got;
                let total = self.total.load(Ordering::Relaxed);
                (self.on_event)(DownloadEvent::Downloading {
                    received: got,
                    total: (total != u64::MAX).then_some(total),
                });
            }
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    let file = fs::File::create(&zip_path).map_err(|e| format!("cannot write {}: {e}", zip_path.display()))?;
    let mut writer = Progress { inner: BufWriter::new(file), received, total, on_event: &mut on_event, last: 0 };
    let response = request_to_writer_no_redirect(request, &mut writer).map_err(|e| {
        let _ = fs::remove_file(&zip_path);
        format!("download failed: {e}")
    })?;
    writer.flush().map_err(|e| format!("cannot write {}: {e}", zip_path.display()))?;
    drop(writer);
    if response.status != 200 {
        let _ = fs::remove_file(&zip_path);
        return Err(format!("download failed: HTTP {}", response.status));
    }
    let result = unpack(&zip_path, data_dir, dir_name, &cancel, &mut on_event);
    let _ = fs::remove_file(&zip_path);
    result
}

/// Unpack the `.milk` files of `zip_path` (and the pack's LICENSE and README)
/// into `data_dir/dir_name`; returns how many presets it holds.
fn unpack(
    zip_path: &Path,
    data_dir: &Path,
    dir_name: &str,
    cancel: &CancelToken,
    on_event: &mut impl FnMut(DownloadEvent),
) -> Result<usize, String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("cannot read the download: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    let dir = zip_read_central_directory(&mut reader).map_err(|e| format!("not a zip archive: {e:?}"))?;
    let staging = data_dir.join(format!("{dir_name}.part"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    let total = dir.file_headers.len();
    let mut presets = 0;
    for (done, header) in dir.file_headers.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = fs::remove_dir_all(&staging);
            return Err("cancelled".into());
        }
        if done % 256 == 0 {
            on_event(DownloadEvent::Unpacking { done, total });
        }
        let Some(rel) = pack_relative(&header.file_name) else { continue };
        let bytes = header.extract(&mut reader).map_err(|e| format!("{}: {e:?}", header.file_name))?;
        let target = staging.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        fs::write(&target, bytes).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        if is_preset(&rel) {
            presets += 1;
        }
    }
    let dest = data_dir.join(dir_name);
    let old = data_dir.join(format!("{dir_name}.old"));
    let _ = fs::remove_dir_all(&old);
    if dest.exists() {
        fs::rename(&dest, &old).map_err(|e| format!("cannot replace {}: {e}", dest.display()))?;
    }
    if let Err(e) = fs::rename(&staging, &dest) {
        let _ = fs::rename(&old, &dest);
        return Err(format!("cannot install {}: {e}", dest.display()));
    }
    let _ = fs::remove_dir_all(&old);
    on_event(DownloadEvent::Done { presets, dir: dest });
    Ok(presets)
}

/// The pack files worth keeping besides the presets, at the pack's top.
const PACK_NOTES: [&str; 2] = ["LICENSE.md", "README.md"];

fn is_preset(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk"))
}

/// The path of an archive entry inside the pack, without the zip's top
/// folder (`presets-cream-of-the-crop-master/`), for a `.milk` file or one
/// of the pack's notes; `None` for anything else or any path that is not
/// plain names.
fn pack_relative(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_owned()),
            _ => return None,
        }
    }
    if parts.len() < 2 {
        return None;
    }
    let rel: PathBuf = parts[1..].iter().collect();
    let note = parts.len() == 2 && PACK_NOTES.iter().any(|n| rel.as_os_str() == *n);
    (is_preset(&rel) || note).then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_folders_and_name() {
        let root = Path::new("/lib");
        let rel = "Fractal/Sierpinski/Flexi + Martin - explore space --- Isosceles edit.milk";
        let e = PresetEntry::new(root.join(rel), rel.to_string(), Sniff::default());
        assert_eq!((e.category.as_str(), e.subcategory.as_str()), ("Fractal", "Sierpinski"));
        assert_eq!(e.title, "explore space");
        assert_eq!(e.authors_text(), "Flexi, Martin");
        assert_eq!(e.editors, vec!["Isosceles".to_string()]);
        let top = PresetEntry::new(root.join("x.milk"), "x.milk".to_string(), Sniff::default());
        assert_eq!((top.category.as_str(), top.subcategory.as_str(), top.title.as_str()), ("", "", "x"));
    }

    #[test]
    fn archive_paths_are_contained() {
        assert_eq!(
            pack_relative("presets-cream-of-the-crop-master/Fractal/A/x.milk"),
            Some(PathBuf::from("Fractal/A/x.milk"))
        );
        assert_eq!(pack_relative("presets-cream-of-the-crop-master/README.md"), Some(PathBuf::from("README.md")));
        assert_eq!(pack_relative("presets-cream-of-the-crop-master/Fractal/README.md"), None);
        assert_eq!(pack_relative("presets-cream-of-the-crop-master/Fractal/a.png"), None);
        assert_eq!(pack_relative("top/../../etc/x.milk"), None);
        assert_eq!(pack_relative("/abs/x.milk"), None);
        assert_eq!(pack_relative("x.milk"), None);
    }

    #[test]
    fn unpack_installs_atomically() {
        use makepad_zip_file::{ZipMethod, ZipWriter};
        let dir = std::env::temp_dir().join(format!("milkdrop-lib-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut w = ZipWriter::new();
        w.add("pack-master/Cat/Sub/A - one.milk", b"[preset00]\nzoom=1\n", ZipMethod::Deflate).unwrap();
        w.add("pack-master/LICENSE.md", b"x", ZipMethod::Store).unwrap();
        w.add("pack-master/Cat/B - two.milk", b"[preset00]\n", ZipMethod::Store).unwrap();
        let zip = dir.join("p.zip");
        fs::write(&zip, w.finish().unwrap()).unwrap();
        let mut events = Vec::new();
        let n = unpack(&zip, &dir, "pack", &CancelToken::new(), &mut |e| events.push(e)).unwrap();
        assert_eq!(n, 2);
        let (lib, _) = scan_cached(&dir.join("pack"), None).unwrap();
        assert_eq!(lib.entries.len(), 2);
        assert!(dir.join("pack/LICENSE.md").is_file());
        assert!(matches!(events.last(), Some(DownloadEvent::Done { presets: 2, .. })));
        assert!(!dir.join("pack.part").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
