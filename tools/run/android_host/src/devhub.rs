//! The embedded devhub node (local/agent_state/devhub/DESIGN.md §12): the headset or phone is a
//! fleet `paired` device the Mac master dials for events, the explorer and full inspect, and
//! pushes sources to (`makepad devhub sync push <device>`). There is no checkout here: pushed files
//! go into the run's edits folder over the APK's source store (relax vfs) and the live watcher
//! patches the running program.
//!
//! Identity: `$HOME/.makepad/fleet` (HOME is the app's files folder). The key is made here on
//! first start and never leaves the device; only its public point is logged, for the admin to sign
//! (`fleet-id sign <name> paired <point>`). The credential and the fleet root (no secrets) are
//! picked up from the app's external files folder (`adb push` reaches it without run-as):
//! `/sdcard/Android/data/<package>/files/fleet/{id.cred,root.pub,revoked}`.

use crate::logcat;
use makepad_devhub::sync::files::Change;
use makepad_devhub::sync::SourceStore;
use makepad_network::fleet::id;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the host conf says about the store (`devhub_name=`, `rev=<repo> <sha>`, `prefix=<repo> <path>`).
#[derive(Clone, Default)]
pub struct StoreConf {
    pub name: String,
    /// the commit of each repo the APK's source archive was packed from (the first push's base)
    pub revs: Vec<(String, String)>,
    pub prefixes: Vec<(String, String)>,
}

/// The repo's folder inside the mounted workspace.
fn repo_dir(workspace: &str, repo: &str) -> String {
    match repo {
        "commercial" => format!("{workspace}/apps/commercial"),
        _ => workspace.to_string(),
    }
}

struct Store {
    workspace: String,
    /// `files/devhub`: `<repo>.rev` (the rev the store holds)
    dir: PathBuf,
    conf: StoreConf,
    lock: Mutex<()>,
}

impl SourceStore for Store {
    fn rev(&self, repo: &str) -> Option<String> {
        if let Ok(r) = std::fs::read_to_string(self.dir.join(format!("{repo}.rev"))) {
            let r = r.trim();
            if r.len() == 40 {
                return Some(r.to_string());
            }
        }
        self.conf.revs.iter().find(|(r, _)| r == repo).map(|(_, s)| s.clone())
    }
    fn prefixes(&self, repo: &str) -> Vec<String> {
        self.conf.prefixes.iter().filter(|(r, _)| r == repo).map(|(_, p)| p.clone()).collect()
    }
    fn write(&self, repo: &str, rev: &str, changes: &[Change]) -> Result<(), String> {
        let _g = self.lock.lock().unwrap();
        let root = repo_dir(&self.workspace, repo);
        for (rel, data) in changes {
            let path = format!("{root}/{rel}");
            match data {
                Some(bytes) => {
                    let disk = relax_compiler::vfs::prepare_edit(&path, false).map_err(|e| format!("{rel}: {e}"))?;
                    let tmp = disk.with_extension("devhub-new");
                    std::fs::write(&tmp, bytes).map_err(|e| format!("{rel}: {e}"))?;
                    std::fs::rename(&tmp, &disk).map_err(|e| format!("{rel}: {e}"))?;
                }
                None => {
                    // an edited file goes; a file only the APK's store holds stays (the store is read-only)
                    if let Some(disk) = relax_compiler::vfs::edited_disk_path(&path) {
                        std::fs::remove_file(&disk).map_err(|e| format!("{rel}: {e}"))?;
                    }
                }
            }
        }
        let f = self.dir.join(format!("{repo}.rev"));
        let tmp = f.with_extension("rev.new");
        std::fs::write(&tmp, rev).and_then(|_| std::fs::rename(&tmp, &f)).map_err(|e| format!("{}: {e}", f.display()))
    }
    fn patch(&self, _repo: &str, paths: &[String]) -> Result<String, String> {
        relax_compiler::reload::sources_changed();
        logcat::log(&format!("devhub: {} pushed files handed to the live watcher", paths.len()));
        Ok(format!("{} files handed to the live watcher", paths.len()))
    }
}

/// The edits folder of the run: pushed sources live there over the APK's store.
pub fn edits_dir(files: &Path) -> PathBuf {
    files.join("devhub").join("edits")
}

/// The app's external files folder, from its internal one (`/data/user/0/<pkg>/files`).
fn external_dir(files: &Path) -> Option<PathBuf> {
    let package = files.parent()?.file_name()?.to_str()?;
    Some(PathBuf::from(format!("/sdcard/Android/data/{package}/files")))
}

/// Makes the key on first start; copies a pushed credential, root and revocation list in.
fn identity(files: &Path) -> Result<(), String> {
    let pushed = external_dir(files).map(|ext| ext.join("fleet"));
    let (point, installed) = id::enrol_device(pushed.as_deref())?;
    logcat::log(&format!("devhub: fleet point {}", makepad_network::tls::to_hex(&point)));
    for name in installed {
        logcat::log(&format!("devhub: installed {name} from the pushed fleet folder"));
    }
    Ok(())
}

/// Installs the store applier and starts the node on its own thread (never in the boot's way).
pub fn start(files: &Path, workspace: &str, conf: StoreConf) {
    let dir = files.join("devhub");
    let _ = std::fs::create_dir_all(&dir);
    makepad_devhub::procinfo::set_battery_source(|| {
        let v = makepad_hal::hal_battery();
        (v >= 0).then(|| ((v & 0xff) as u32, v & 0x100 != 0))
    });
    makepad_devhub::sync::set_source_store(Arc::new(Store { workspace: workspace.to_string(), dir, conf: conf.clone(), lock: Mutex::new(()) }));
    // the fleet key stays in the app's internal storage (never the shared external folder)
    std::env::set_var("MAKEPAD_FLEET_DIR", files.join("fleet"));
    let files = files.to_path_buf();
    let _ = std::thread::Builder::new().name("devhub".into()).spawn(move || {
        if let Err(e) = identity(&files) {
            logcat::log(&format!("devhub: {e}"));
        }
        let mut nc = makepad_devhub::conf::NodeConf::default();
        nc.name = if conf.name.is_empty() { makepad_devhub::proto::default_machine_name() } else { conf.name.clone() };
        nc.embedded = true;
        nc.discovery = true;
        nc.launch = Vec::new();
        let log: makepad_devhub::trust::Log = Arc::new(|l: &str| logcat::log(l));
        let trust: Option<Arc<dyn makepad_devhub::trust::Trust>> = match makepad_devhub::trust::FleetTrust::load(&nc.name) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                logcat::log(&format!("devhub: local only: {e}"));
                None
            }
        };
        match makepad_devhub::node::Node::start(nc, trust, log) {
            Ok(node) => {
                logcat::log(&format!(
                    "devhub: node `{}` {}",
                    node.conf.name,
                    match (&node.addr, &node.lan_off) {
                        (Some(a), _) => format!("serves the master on {a}"),
                        (None, Some(why)) => why.clone(),
                        (None, None) => "is local only".into(),
                    }
                ));
                // the node's threads serve; this one keeps it
                loop {
                    std::thread::park();
                    let _ = &node;
                }
            }
            Err(e) => logcat::log(&format!("devhub: the node did not start: {e}")),
        }
    });
}
