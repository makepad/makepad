//! The node's sync settings: `~/.makepad/devhub/sync.conf` (`MAKEPAD_HOME` moves it), `key =
//! value` lines, all optional. A node without the file syncs the WM's own tree the way the
//! retired deploy scripts did (fleetmp/full-update.sh: ship in place, prebuild with the WM's env,
//! restart the WM through its service).
//!
//! ```text
//! checkout = /home/arch/makepad   # the synced checkout (default: MAKEPAD_WM_ROOT, else ~/makepad)
//! build = wm                      # wm: the WM, then makepad-wm-build of what it starts (build-next.sh)
//!                                 # apps: each `prebuild` app the way a launch builds it; off: no build
//! prebuild = hub aichat terminal browser files task code sandbox
//! wm_flags = --jit sandbox        # default: read from the installed session script (one source)
//! jobs = 8
//! wm_service = makepad-wm.service # restarted through `sudo -n systemctl restart` (the sudoers rule);
//!                                 # `none`: this node has no WM to restart
//! ```

use std::path::{Path, PathBuf};

/// The session script the WM's unit runs (installed by the sudoers rule's `install` line).
pub const SESSION_SCRIPT: &str = "/usr/local/bin/makepad-wm-session";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Wm,
    Apps,
    Off,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyncConf {
    pub checkout: PathBuf,
    pub build: Build,
    pub prebuild: Vec<String>,
    pub wm_flags: Vec<String>,
    pub jobs: u32,
    pub wm_service: Option<String>,
}

pub fn path() -> PathBuf {
    crate::proto::home().join("sync.conf")
}

fn home() -> PathBuf {
    crate::os::home_dir()
}

/// `wm_flags=(--jit sandbox)` of the installed session script, read as data (never run).
pub fn session_wm_flags(script: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(script).ok()?;
    let line = text.lines().map(str::trim).find(|l| l.starts_with("wm_flags=("))?;
    let inner = line.strip_prefix("wm_flags=(")?.split(')').next()?;
    Some(inner.split_whitespace().map(|w| w.trim_matches('"').trim_matches('\'').to_string()).collect())
}

impl SyncConf {
    pub fn parse(text: &str) -> SyncConf {
        let mut c = SyncConf {
            checkout: std::env::var_os("MAKEPAD_WM_ROOT").map(PathBuf::from).unwrap_or_else(|| home().join("makepad")),
            build: Build::Wm,
            prebuild: "hub aichat terminal browser files task code sandbox".split(' ').map(str::to_string).collect(),
            wm_flags: Vec::new(),
            jobs: 8,
            wm_service: Some("makepad-wm.service".into()),
        };
        let mut flags_set = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "checkout" if !v.is_empty() => c.checkout = PathBuf::from(v),
                "build" => {
                    c.build = match v {
                        "apps" => Build::Apps,
                        "off" | "none" => Build::Off,
                        _ => Build::Wm,
                    }
                }
                "prebuild" => c.prebuild = v.split_whitespace().map(str::to_string).collect(),
                "wm_flags" => {
                    c.wm_flags = v.split_whitespace().map(str::to_string).collect();
                    flags_set = true;
                }
                "jobs" => c.jobs = v.parse().unwrap_or(c.jobs).clamp(1, 64),
                "wm_service" => c.wm_service = (!matches!(v, "" | "none" | "off")).then(|| v.to_string()),
                _ => eprintln!("devhub sync: conf: skipped `{line}`"),
            }
        }
        if !flags_set {
            c.wm_flags = session_wm_flags(Path::new(SESSION_SCRIPT)).unwrap_or_default();
        }
        c
    }

    pub fn load() -> SyncConf {
        Self::parse(&std::fs::read_to_string(path()).unwrap_or_default())
    }

    /// The nested commercial checkout.
    pub fn commercial(&self) -> PathBuf {
        self.checkout.join("apps").join("commercial")
    }

    /// Where a repo's files live in the checkout.
    pub fn repo_dir(&self, repo: &str) -> Option<PathBuf> {
        match repo {
            "makepad" => Some(self.checkout.clone()),
            "commercial" => Some(self.commercial()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_parses() {
        let c = SyncConf::parse("checkout = /x/makepad\nbuild = apps\nprebuild = sandbox\nwm_flags = --jit sandbox,code\njobs = 4\nwm_service = none\n");
        assert_eq!(c.checkout, PathBuf::from("/x/makepad"));
        assert_eq!(c.build, Build::Apps);
        assert_eq!(c.prebuild, vec!["sandbox"]);
        assert_eq!(c.wm_flags, vec!["--jit", "sandbox,code"]);
        assert_eq!(c.jobs, 4);
        assert_eq!(c.wm_service, None);
        assert_eq!(c.repo_dir("commercial"), Some(PathBuf::from("/x/makepad/apps/commercial")));
    }

    #[test]
    fn session_flags_are_read_as_data() {
        let dir = std::env::temp_dir().join(format!("devhub-sync-conf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("session.sh");
        std::fs::write(&f, "#!/bin/bash\n# The WM's own flags\nwm_flags=(--jit sandbox)\nexec cargo run -- \"${wm_flags[@]}\"\n").unwrap();
        assert_eq!(session_wm_flags(&f), Some(vec!["--jit".to_string(), "sandbox".to_string()]));
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_dir(&dir);
    }
}
