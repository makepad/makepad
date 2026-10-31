//! Name the program inside a `makepad` JIT process. This runs on the sampler
//! worker, before history and the UI receive the metadata. Only runner PIDs
//! are looked up: never scan the (potentially huge) temporary directory or
//! connect to a running app. Unchanged metadata keeps its shared Arc.

use crate::backend::{bounded, ProcInfo, ProcMeta, MAX_NAME_LEN};
use makepad_remote::discovery;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Default)]
pub struct JitNames {
    entries: HashMap<u32, Entry>,
}

struct Entry {
    source: Arc<ProcMeta>,
    stamp: Option<(SystemTime, u64)>,
    named: Arc<ProcMeta>,
    seen: bool,
}

impl JitNames {
    pub fn update(&mut self, processes: &mut [ProcInfo]) {
        let dir = discovery::registry_dir();
        for process in processes {
            let source = &process.meta;
            if !matches!(source.name.as_str(), "makepad" | "makepad.exe") {
                continue;
            }
            let pid = source.key.pid;
            let path = dir.join(format!("makepad-remote-{pid}.json"));
            let stamp = std::fs::metadata(&path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
            if let Some(entry) = self.entries.get_mut(&pid) {
                if Arc::ptr_eq(source, &entry.source) && stamp == entry.stamp {
                    entry.seen = true;
                    process.meta = entry.named.clone();
                    continue;
                }
            }

            // Read the shared discovery format without discovery::apps(), which
            // also deletes stale files. The monitor must only observe them.
            let app = std::fs::read_to_string(&path).ok().and_then(|text| discovery::parse(&text));
            let registered = app.as_ref().filter(|app| {
                app.pid == pid
                    && source.key.verified()
                    && app.started_ms >= started_ms(source)
                    && app.text("run").is_some_and(|run| !run.is_empty())
                    && !app.app.trim().is_empty()
            });
            // A normal `makepad run <package>` remains identifiable before its
            // endpoint appears, with inspection disabled, or across users.
            // More complex runner options use the authoritative discovery name.
            let package = registered.map(|app| app.app.as_str()).or_else(|| command_package(&source.cmdline));
            let named = if let Some(package) = package {
                let mut meta = (**source).clone();
                let package: String = package.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                meta.name = bounded(format!("{} (JIT)", package.trim()), MAX_NAME_LEN);
                meta.is_app = true;
                Arc::new(meta)
            } else {
                source.clone()
            };
            // A writer may have been in the middle of publishing the file. A
            // failed parse is retried next tick even if its timestamp is coarse.
            let stamp = if app.is_some() { stamp } else { None };
            self.entries.insert(pid, Entry { source: source.clone(), stamp, named: named.clone(), seen: true });
            process.meta = named;
        }
        self.entries.retain(|_, entry| {
            let keep = entry.seen;
            entry.seen = false;
            keep
        });
    }
}

fn started_ms(meta: &ProcMeta) -> u64 {
    #[cfg(target_os = "windows")]
    {
        // The backend's precise start identity is a Windows FILETIME.
        meta.key.start.saturating_sub(116_444_736_000_000_000) / 10_000
    }
    #[cfg(not(target_os = "windows"))]
    {
        meta.key.start / 1_000
    }
}

fn command_package(cmdline: &str) -> Option<&str> {
    let (executable, args) = cmdline.split_once(" run ")?;
    let executable = executable.trim_matches('"').rsplit(['/', '\\']).next()?;
    if !matches!(executable, "makepad" | "makepad.exe") {
        return None;
    }
    let package = args.split_whitespace().next()?;
    // Cargo package names contain only alphanumeric characters, `_` and `-`.
    // Do not guess at option values, manifests, or quoted paths.
    (!package.starts_with('-') && package.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))).then_some(package)
}
