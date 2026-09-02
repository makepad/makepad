//! First run with no map data: fetch Amsterdam and bake it, in the window.
//!
//! The maps this app normally draws are built by `tools/download_map.sh`
//! and measured in tens of gigabytes. Someone who has just cloned the repo
//! has none of them, and until now got a black window and a log line about
//! a missing archive. So: on a first run with nothing on disk, the app
//! downloads one city extract and bakes it into a real map — tiles, a
//! routing graph and a search index — while showing what it is doing.
//!
//! Split of labour, and why:
//!
//! * The **download** runs on the platform's HTTP stack (`cx.http_request`),
//!   which streams the body and reports `HttpProgress` as it goes on every
//!   OS we ship. That is where the honest byte counter comes from.
//! * The **bake** runs on one worker thread through
//!   [`makepad_map_build::testmap::bake`] — the same passes the
//!   `makepad-map-tiles testmap` CLI runs, with a progress sink installed so
//!   their commentary lands in this window instead of a terminal nobody is
//!   watching.
//!
//! Both halves report into [`TestMapBuild`], which the app polls like any
//! other worker (`nav_data`, radar, wind).

use makepad_map_build::progress::{Report, SinkGuard};
use makepad_map_build::testmap::{self, BakeOptions, NoFetch, TestMapPaths};
use makepad_widgets::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Lines kept in the popup's log pane. Enough to show the current pass and
/// how it got there, few enough that the panel never grows.
const LOG_LINES: usize = 7;

/// The archives a fully provisioned machine has (the paths the MapView is
/// declared with). Any one of them present means this is not a first run
/// and nothing should be offered.
const PRODUCTION_ARCHIVES: [&str; 3] = [
    "world.mkmap",
    "europe-base-br-faces.mbtiles",
    "europe-shortbread.mbtiles",
];
const MAPS_ROOT_PREF: &str = "route/maps-root";

/// The first production archive under this app's one resolved maps root.
pub fn production_archive(maps_root: &Path) -> Option<PathBuf> {
    PRODUCTION_ARCHIVES
        .iter()
        .map(|name| maps_root.join(name))
        .find(|path| path.is_file())
}

/// Resolve once at startup: a checked-out executable uses that checkout's
/// `local/maps`; an installed/copied executable uses Makepad's per-user home.
/// The saved setting wins over both. The process cwd is deliberately absent.
pub fn resolve_maps_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let home = makepad_widgets::makepad_platform::home::makepad_home();
        let explicit = fs::read_to_string(home.join(MAPS_ROOT_PREF))
            .ok()
            .map(|value| PathBuf::from(value.trim()))
            .filter(|path| !path.as_os_str().is_empty());
        let executable = std::env::current_exe().unwrap_or_default();
        resolve_maps_root_from(&executable, &home, explicit.as_deref())
    })
    .clone()
}

fn resolve_maps_root_from(executable: &Path, home: &Path, explicit: Option<&Path>) -> PathBuf {
    resolve_maps_root_with(executable, home, explicit, |manifest| {
        fs::read_to_string(manifest).is_ok_and(|text| {
            text.lines().any(|line| {
                let line = line.trim();
                line == "[workspace]" || line.starts_with("workspace.members")
            })
        })
    })
}

fn resolve_maps_root_with(
    executable: &Path,
    home: &Path,
    explicit: Option<&Path>,
    mut is_workspace_manifest: impl FnMut(&Path) -> bool,
) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit.to_path_buf();
    }
    let mut directory = executable.parent();
    while let Some(candidate) = directory {
        if is_workspace_manifest(&candidate.join("Cargo.toml")) {
            return candidate.join("local/maps");
        }
        directory = candidate.parent();
    }
    home.join("maps")
}

/// Persist the settings-panel override. Empty restores automatic resolution;
/// a non-empty setting must be absolute so it can never regain cwd semantics.
pub fn save_maps_root_setting(value: &str) -> Result<(), String> {
    let home = makepad_widgets::makepad_platform::home::makepad_home();
    let path = home.join(MAPS_ROOT_PREF);
    let value = value.trim();
    if value.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("remove {}: {error}", path.display())),
        }
    }
    let root = Path::new(value);
    if !root.is_absolute() {
        return Err("maps root must be an absolute path".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let partial = path.with_extension("part");
    fs::write(&partial, value).map_err(|error| format!("write {}: {error}", partial.display()))?;
    fs::rename(&partial, &path).map_err(|error| format!("publish {}: {error}", path.display()))
}

/// What the worker thread sends back.
enum BakeMsg {
    Progress(Report),
    Done { skipped: usize },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// Nothing to do — the app has map data.
    Idle,
    /// Waiting for the user to say go.
    Offered,
    Fetching { loaded: u64, total: u64 },
    Baking,
    Done,
    Failed(String),
}

pub struct TestMapBuild {
    pub paths: TestMapPaths,
    pub stage: Stage,
    /// Headline: the pass now running, in its own words.
    pub headline: String,
    /// The last few detail lines under that headline.
    pub log: Vec<String>,
    /// Whole-recipe progress, 0..1.
    pub fraction: f32,
    request_id: Option<LiveId>,
    rx: ToUIReceiver<BakeMsg>,
    /// Held for the life of the bake; dropping it releases the global sink.
    sink: Option<SinkGuard>,
}

impl Default for TestMapBuild {
    fn default() -> Self {
        Self {
            paths: TestMapPaths::in_dir(resolve_maps_root(), "amsterdam"),
            stage: Stage::Idle,
            headline: String::new(),
            log: Vec::new(),
            fraction: 0.0,
            request_id: None,
            rx: ToUIReceiver::default(),
            sink: None,
        }
    }
}

impl TestMapBuild {
    pub fn set_maps_root(&mut self, maps_root: &Path) {
        self.paths = TestMapPaths::in_dir(maps_root, "amsterdam");
    }

    /// True while the popup should be on screen.
    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Waiting to be started (or restarted after a failure): the only
    /// states in which the start button does anything.
    pub fn can_start(&self) -> bool {
        matches!(self.stage, Stage::Offered | Stage::Failed(_))
    }

    /// Freshly offered and not yet running.
    pub fn is_offered(&self) -> bool {
        self.stage == Stage::Offered
    }

    /// True while the bake is running and must not be started twice.
    pub fn is_running(&self) -> bool {
        matches!(self.stage, Stage::Fetching { .. } | Stage::Baking)
    }

    /// Offer the bake when this machine has no map to draw. Production
    /// archives win: someone with the real maps must never be asked to
    /// download a test one.
    pub fn offer_if_no_map(&mut self, production_archive_present: bool) {
        if production_archive_present || self.paths.is_complete() {
            return;
        }
        self.stage = Stage::Offered;
        self.headline = "No map data on this machine".to_string();
        self.log = vec![
            "Building an Amsterdam test map: ~143 MB download, then a couple".to_string(),
            "of minutes of baking. Tiles with baked road faces, a routing".to_string(),
            format!(
                "graph and a search index land under {}.",
                self.paths
                    .archive
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .display()
            ),
        ];
    }

    /// Start (or resume) the recipe: download the extract if it is not
    /// already here, then bake.
    pub fn start(&mut self, cx: &mut Cx) {
        if !self.can_start() {
            return;
        }
        self.fraction = 0.0;
        if self.paths.pbf.is_file() {
            self.start_bake();
            return;
        }
        self.stage = Stage::Fetching { loaded: 0, total: testmap::AMSTERDAM_PBF_APPROX_BYTES };
        self.headline = "Downloading Amsterdam extract".to_string();
        self.push_log(format!("GET {}", testmap::AMSTERDAM_PBF_URL));
        let request_id = LiveId::unique();
        self.request_id = Some(request_id);
        let request = HttpRequest::new(testmap::AMSTERDAM_PBF_URL.to_string(), HttpMethod::GET);
        cx.http_request(request_id, request);
    }

    /// The popup's second button: cancel a download, or put the offer away
    /// (it comes back on the next launch — the machine still has no map),
    /// or close the popup on a finished bake.
    pub fn dismiss(&mut self) {
        match self.stage {
            Stage::Fetching { .. } => self.cancel(),
            _ => self.stage = Stage::Idle,
        }
    }

    /// Give up on a download in flight. The bake itself has no cancel: its
    /// passes are minutes of straight-line CPU work, and killing one
    /// mid-write is what the scratch store's resume markers are for.
    pub fn cancel(&mut self) {
        if let Stage::Fetching { .. } = self.stage {
            self.request_id = None;
            self.stage = Stage::Offered;
            self.headline = "Download cancelled".to_string();
        }
    }

    /// True when this response belongs to the extract download.
    pub fn owns_request(&self, request_id: LiveId) -> bool {
        self.request_id == Some(request_id)
    }

    pub fn handle_http_progress(&mut self, request_id: LiveId, progress: &HttpProgress) -> bool {
        if !self.owns_request(request_id) {
            return false;
        }
        let total = if progress.total > 0 {
            progress.total
        } else {
            testmap::AMSTERDAM_PBF_APPROX_BYTES
        };
        self.stage = Stage::Fetching { loaded: progress.loaded, total };
        self.fraction =
            testmap::overall_fraction("fetch", progress.loaded as f32 / total.max(1) as f32);
        true
    }

    pub fn handle_http_response(&mut self, request_id: LiveId, response: &HttpResponse) -> bool {
        if !self.owns_request(request_id) {
            return false;
        }
        self.request_id = None;
        if response.status_code != 200 {
            self.fail(format!("download failed: HTTP {}", response.status_code));
            return true;
        }
        let body = response.body.as_deref().unwrap_or(&[]);
        if body.len() < 1_000_000 {
            self.fail(format!("download failed: {} bytes is not an extract", body.len()));
            return true;
        }
        // Landed under a partial name and renamed, so an interrupted run
        // never leaves something that looks like a finished extract.
        let part = self.paths.pbf.with_extension("pbf.part");
        if let Some(dir) = part.parent() {
            if let Err(err) = fs::create_dir_all(dir) {
                self.fail(format!("create {}: {err}", dir.display()));
                return true;
            }
        }
        if let Err(err) = fs::write(&part, body) {
            self.fail(format!("write {}: {err}", part.display()));
            return true;
        }
        if let Err(err) = fs::rename(&part, &self.paths.pbf) {
            self.fail(format!("rename {}: {err}", part.display()));
            return true;
        }
        self.push_log(format!("extract saved: {:.0} MB", body.len() as f64 / 1.0e6));
        self.start_bake();
        true
    }

    pub fn handle_http_error(&mut self, request_id: LiveId, err: &str) -> bool {
        if !self.owns_request(request_id) {
            return false;
        }
        self.request_id = None;
        self.fail(format!("download failed: {err}"));
        true
    }

    /// Drain worker messages. Returns true when something changed, and
    /// `Some(true)` finishing means the map is ready to be pointed at.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            match msg {
                BakeMsg::Progress(report) => {
                    if let Some(fraction) = report.fraction {
                        self.fraction = testmap::overall_fraction(report.stage, fraction);
                    }
                    if report.line.is_empty() {
                        continue;
                    }
                    if report.headline {
                        self.headline = report.line;
                    } else {
                        self.push_log(report.line);
                    }
                }
                BakeMsg::Done { skipped } => {
                    self.sink = None;
                    self.stage = Stage::Done;
                    self.fraction = 1.0;
                    self.headline = if skipped == 0 {
                        "Test map ready".to_string()
                    } else {
                        format!("Test map built, {skipped} tiles skipped")
                    };
                }
                BakeMsg::Failed(error) => {
                    self.sink = None;
                    self.fail(error);
                }
            }
        }
        changed
    }

    fn start_bake(&mut self) {
        self.stage = Stage::Baking;
        self.headline = "Baking tiles".to_string();
        let sender = Mutex::new(self.rx.sender());
        // Installed for the whole bake: every pass reports through it, from
        // whichever worker thread it happens to be on.
        self.sink = SinkGuard::install(move |report| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(BakeMsg::Progress(report));
            }
        });
        if self.sink.is_none() {
            self.fail("a bake is already running in this process".to_string());
            return;
        }
        let mut options = BakeOptions::amsterdam();
        options.paths = self.paths.clone();
        let done = self.rx.sender();
        std::thread::spawn(move || {
            // NoFetch: the extract is on disk before this thread starts —
            // downloading is the window's job, where progress comes free.
            //
            // A pass-level panic must still reach the popup with its actual
            // payload. Tile-local face failures are caught lower down and
            // return as successful builds with a skipped count.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                testmap::bake(&options, &mut NoFetch)
            }))
            .unwrap_or_else(|payload| {
                Err(format!("bake panicked: {}", testmap::panic_message(payload)))
            });
            let _ = done.send(match result {
                Ok(stats) => BakeMsg::Done { skipped: stats.skipped_tiles },
                Err(error) => BakeMsg::Failed(error),
            });
        });
    }

    fn fail(&mut self, error: String) {
        self.push_log(error.clone());
        self.headline = "Test map build failed".to_string();
        self.stage = Stage::Failed(error);
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line.trim_end().to_string());
        if self.log.len() > LOG_LINES {
            self.log.remove(0);
        }
    }

    /// The line under the bar: bytes while downloading, the pass otherwise.
    pub fn status_line(&self) -> String {
        match &self.stage {
            Stage::Fetching { loaded, total } => format!(
                "{:.0} of {:.0} MB",
                *loaded as f64 / 1.0e6,
                *total as f64 / 1.0e6
            ),
            Stage::Baking => format!("{:.0}%", self.fraction * 100.0),
            Stage::Done => format!(
                "{:.1} GB under {}",
                self.paths.bytes_on_disk() as f64 / 1.0e9,
                self.paths
                    .archive
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .display()
            ),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_root_uses_checkout_found_from_executable() {
        let executable = Path::new("/checkout/target/release/route");
        let root = resolve_maps_root_with(executable, Path::new("/home/.makepad"), None, |path| {
            path == Path::new("/checkout/Cargo.toml")
        });
        assert_eq!(root, PathBuf::from("/checkout/local/maps"));
    }

    #[test]
    fn maps_root_for_a_binary_copy_uses_makepad_home() {
        let root = resolve_maps_root_with(
            Path::new("/Applications/Makepad/route"),
            Path::new("/home/.makepad"),
            None,
            |_| false,
        );
        assert_eq!(root, PathBuf::from("/home/.makepad/maps"));
    }

    #[test]
    fn explicit_maps_root_wins_over_checkout() {
        let root = resolve_maps_root_with(
            Path::new("/checkout/target/release/route"),
            Path::new("/home/.makepad"),
            Some(Path::new("/data/maps")),
            |_| true,
        );
        assert_eq!(root, PathBuf::from("/data/maps"));
    }
}
