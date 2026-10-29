//! The built-in apps registry's hosting dimension (aicontrol.md §4): which
//! apps are linked in as MODULES, and which of those the person has
//! switched to module hosting.
//!
//! The launch table (`clients::registry()`: package, directory, binary,
//! launch policy — everything a PROCESS needs) stays where it is; this is
//! the overlay keyed by the same ids: the linked `AppModule`, and the
//! hosting each app gets. The linked modules come from the BUILD
//! (`build.rs`): the desktop `wm` binary links none, the all-in-one links
//! its app crates. Desktop default is a process under the JIT (`makepad run
//! <package>` started by `cargo run --profile host -p makepad-run` in the
//! checkout, `Runner::Jit`; the user's rule, 2026-10-29: every dock app runs
//! the latest synced tree under the JIT): a linked module is still launched
//! that way unless `~/.makepad/wm/apps.splash` says otherwise (a settings
//! file, never an environment variable) or a dev run passes `--module <id>`.
//! `id: Process` runs the app's own release binary instead (`cargo run
//! --release -p <package>`, `Runner::Native`). A build without processes — the web,
//! iOS, or a desktop all-in-one with `modules_only` — ignores the switch:
//! everything is a module there.

use crate::build::WmBuild;
use crate::clients::AppDef;
use makepad_app_module::AppModule;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hosting {
    /// A process of the app's own release binary (`id: Process`).
    Process,
    Module,
    /// A process running under the JIT: `makepad run <package>`. The desktop default.
    Jit,
}

/// What a menu may offer to launch: the linked modules, and — where this
/// build hosts processes — every registry app this checkout can run.
/// Handed to the shell surfaces (the launcher, the dock, the phone home
/// page) so a build without processes never lists an app it cannot start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launchable {
    pub linked: Vec<&'static str>,
    pub processes: bool,
    /// A phone build: only the registry's `mobile` apps are offered.
    pub mobile: bool,
}

impl Default for Launchable {
    /// The desktop before its build is read: processes where the platform
    /// has them, nothing linked.
    fn default() -> Self {
        Launchable {
            linked: Vec::new(),
            processes: crate::host::processes_available(),
            mobile: false,
        }
    }
}

impl Launchable {
    pub fn allows(&self, app: &AppDef) -> bool {
        if self.mobile && !makepad_app_registry::find(&app.id).is_some_and(|entry| entry.mobile) {
            return false;
        }
        self.linked.contains(&app.id.as_str())
            || (self.processes && app.is_available())
    }
}

pub struct AppRegistry {
    modules: Vec<&'static dyn AppModule>,
    overrides: HashMap<String, Hosting>,
    /// This build hosts processes (the platform can, and the build wants to).
    processes: bool,
    /// This build links the assistant's widget families (`WmBuild::assistant`).
    assistant: bool,
    domains: bool,
    /// The build comes up in a phone style: its lists offer the registry's mobile apps only.
    mobile: bool,
}

impl Default for AppRegistry {
    fn default() -> Self {
        AppRegistry {
            modules: Vec::new(),
            overrides: HashMap::new(),
            processes: crate::host::processes_available(),
            assistant: false,
            domains: false,
            mobile: false,
        }
    }
}

impl AppRegistry {
    /// The registry for a build, with the person's overrides: the settings
    /// file first, then the command line's `--module <id>` and `--jit <id>` flags on top
    /// (the fleet session runs Sandbox with `--jit sandbox`, so every node's Sandbox can be
    /// inspected and live-patched).
    /// `processes` is the host's answer (`App::processes`): the platform's
    /// capability and the build's `modules_only` together.
    pub fn load(settings: &Path, args: &[String], build: &WmBuild, processes: bool) -> Self {
        let mut registry = AppRegistry {
            modules: build.modules.clone(),
            overrides: HashMap::new(),
            processes,
            assistant: build.assistant.is_some(),
            domains: false,
            mobile: build.style.mobile(),
        };
        if let Ok(text) = std::fs::read_to_string(settings) {
            for (id, hosting) in Self::parse_overrides(&text) {
                registry.overrides.insert(id, hosting);
            }
        }
        let mut i = 0;
        while i < args.len() {
            let flag = match args[i].as_str() {
                "--module" => Some(Hosting::Module),
                "--jit" => Some(Hosting::Jit),
                _ => None,
            };
            if let Some(hosting) = flag {
                if let Some(id) = args.get(i + 1) {
                    registry.overrides.insert(id.to_lowercase(), hosting);
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        registry
    }

    /// The linked module for an app, if this build has one.
    pub fn module(&self, id: &str) -> Option<&'static dyn AppModule> {
        self.modules.iter().copied().find(|m| m.id() == id)
    }

    /// How a launch of `id` is hosted. With processes: Module only when a
    /// module is linked AND the person (or the dev flag) asked for it, its
    /// release binary when the settings say `id: Process`, else the JIT. In a
    /// build without processes (the web, iOS, a modules-only desktop
    /// build): every linked module is a module, and everything else is
    /// simply not there.
    pub fn hosting(&self, id: &str) -> Hosting {
        if !self.processes {
            return if self.module(id).is_some() { Hosting::Module } else { Hosting::Process };
        }
        match self.overrides.get(id) {
            Some(Hosting::Module) if self.module(id).is_some() => Hosting::Module,
            Some(Hosting::Process) => Hosting::Process,
            _ => Hosting::Jit,
        }
    }

    /// What runs a process launch of `id`: the JIT, unless the settings say `id: Process` (its
    /// release binary).
    pub fn runner(&self, id: &str) -> makepad_app_registry::Runner {
        match self.hosting(id) {
            Hosting::Jit => makepad_app_registry::Runner::Jit,
            _ => makepad_app_registry::Runner::Native,
        }
    }

    /// Whether the assistant is the aichat MODULE seated in the pane
    /// in-process: only when the build links it, and then always where
    /// there are no processes; on a desktop only when `aichat` is switched
    /// to module hosting, the child process being the default.
    pub fn pane_in_process(&self) -> bool {
        self.assistant && (!self.processes || self.overrides.get("aichat") == Some(&Hosting::Module))
    }

    pub fn linked_ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    pub fn enable_domains(&mut self) { self.domains = true; }

    /// What the menus may offer (see [`Launchable`]).
    pub fn launchable(&self) -> Launchable {
        let linked = if self.domains {
            makepad_app_registry::apps().iter().filter(|a| a.wm_launchable).map(|a| a.id).collect()
        } else { self.linked_ids() };
        Launchable { linked, processes: self.processes, mobile: self.mobile }
    }

    /// `~/.makepad/wm/apps.splash`: one `id: Module`, `id: Process` or `id: Jit` per
    /// line, optionally inside `{ }`, commas and `//` comments allowed —
    /// the same shape as the theme files, small enough to read without
    /// the VM.
    pub fn parse_overrides(text: &str) -> Vec<(String, Hosting)> {
        let mut out = Vec::new();
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or("").trim().trim_matches(|c| c == '{' || c == '}' || c == ',').trim();
            if line.is_empty() {
                continue;
            }
            let Some((id, hosting)) = line.split_once(':') else { continue };
            let hosting = match hosting.trim().trim_matches(',').trim().to_lowercase().as_str() {
                "module" => Hosting::Module,
                "process" => Hosting::Process,
                "jit" => Hosting::Jit,
                _ => continue,
            };
            out.push((id.trim().to_lowercase(), hosting));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_app_module::*;
    use makepad_widgets::*;

    /// A module in name only: enough for the registry, never created.
    struct Stub;
    static STUB: Stub = Stub;
    impl AppModule for Stub {
        fn id(&self) -> &'static str {
            "sheets"
        }
        fn label(&self) -> &'static str {
            "Stub"
        }
        fn register(&self, _vm: &mut ScriptVm) {}
        fn open_schema(&self) -> OpenSchema {
            OpenSchema::new(1)
        }
        fn create(&self, _vm: &mut ScriptVm, _open: ValidatedOpen, _handles: InstanceHandles) -> InstanceParts {
            unreachable!("the registry tests never create an instance")
        }
        fn capabilities(&self) -> &'static [&'static str] {
            &[]
        }
    }

    fn build(modules_only: bool, assistant: bool) -> WmBuild {
        WmBuild {
            modules: vec![&STUB],
            modules_only,
            assistant: if assistant { Some(|_vm: &mut ScriptVm| {}) } else { None },
            ..Default::default()
        }
    }

    #[test]
    fn overrides_parse_the_settings_shape_and_ignore_noise() {
        let text = "// which apps run in-process\n{\n  sheets: Module,\n  Terminal: process\n  files: Sideways\n  nonsense\n}\n";
        assert_eq!(
            AppRegistry::parse_overrides(text),
            vec![("sheets".to_string(), Hosting::Module), ("terminal".to_string(), Hosting::Process)]
        );
    }

    #[test]
    fn a_jit_override_runs_the_process_under_the_jit() {
        assert_eq!(AppRegistry::parse_overrides("{ Sandbox: Jit }"), vec![("sandbox".to_string(), Hosting::Jit)]);
        let dir = std::env::temp_dir().join(format!("wm-apps-jit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("apps.splash");
        std::fs::write(&settings, "sandbox: Jit\n").unwrap();
        let registry = AppRegistry::load(&settings, &[], &build(false, false), true);
        let _ = std::fs::remove_file(&settings);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(registry.hosting("sandbox"), Hosting::Jit);
        assert_eq!(registry.runner("sandbox"), makepad_app_registry::Runner::Jit);
        assert_eq!(registry.runner("terminal"), makepad_app_registry::Runner::Jit, "the desktop default is the JIT");
    }

    #[test]
    fn a_process_override_runs_the_release_binary() {
        assert_eq!(AppRegistry::parse_overrides("{ Terminal: Process }"), vec![("terminal".to_string(), Hosting::Process)]);
        let dir = std::env::temp_dir().join(format!("wm-apps-process-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("apps.splash");
        std::fs::write(&settings, "terminal: Process\n").unwrap();
        let registry = AppRegistry::load(&settings, &[], &build(false, false), true);
        let _ = std::fs::remove_file(&settings);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(registry.runner("terminal"), makepad_app_registry::Runner::Native);
        assert_eq!(registry.runner("files"), makepad_app_registry::Runner::Jit);
    }

    #[test]
    fn a_jit_flag_runs_the_process_under_the_jit() {
        let flags = ["-scale=1.3".to_string(), "--jit".to_string(), "Sandbox".to_string()];
        let registry = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &flags, &build(false, false), true);
        assert_eq!(registry.runner("sandbox"), makepad_app_registry::Runner::Jit);
        assert_eq!(registry.runner("terminal"), makepad_app_registry::Runner::Jit);
    }

    #[test]
    fn hosting_is_process_unless_a_linked_module_is_switched_on() {
        let flags = ["--module".to_string(), "sheets".to_string(), "--module".to_string(), "files".to_string()];
        let registry = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &flags, &build(false, false), true);
        // files has no linked module: the flag cannot make it one.
        assert_eq!(registry.hosting("files"), Hosting::Jit);
        assert_eq!(registry.hosting("terminal"), Hosting::Jit);
        assert_eq!(registry.hosting("sheets"), Hosting::Module);
        assert!(registry.linked_ids().contains(&"sheets"));
        assert!(!registry.pane_in_process(), "no assistant linked");
        let plain = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &[], &build(false, true), true);
        assert_eq!(plain.hosting("sheets"), Hosting::Jit, "desktop default is a process under the JIT");
        assert!(!plain.pane_in_process(), "the desktop's assistant is the child process unless switched");
        assert_eq!(plain.launchable(), Launchable { linked: vec!["sheets"], processes: true, mobile: false });
    }

    #[test]
    fn a_phone_build_offers_only_the_registry_mobile_apps() {
        let all: Vec<&'static str> = makepad_app_registry::apps().iter().filter(|a| a.wm_launchable).map(|a| a.id).collect();
        let phone = Launchable { linked: all.clone(), processes: false, mobile: true };
        let desk = Launchable { linked: all, processes: false, mobile: false };
        let offered = |l: &Launchable| -> Vec<String> {
            crate::clients::registry().iter().filter(|a| l.allows(a)).map(|a| a.id.clone()).collect()
        };
        let mut phone_ids = offered(&phone);
        phone_ids.sort();
        assert_eq!(phone_ids, ["amp", "calculator", "camera", "clock", "notes", "photos", "weather"]);
        let desk_ids = offered(&desk);
        for id in ["code", "task", "terminal", "files", "score"] {
            assert!(desk_ids.iter().any(|d| d == id), "the desktop still offers {id}");
        }
    }

    #[test]
    fn without_processes_every_linked_module_is_a_module_and_nothing_else_launches() {
        let all = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &[], &build(true, true), false);
        assert_eq!(all.hosting("sheets"), Hosting::Module);
        assert_eq!(all.hosting("terminal"), Hosting::Process, "not linked: not startable, whatever the name says");
        assert!(all.pane_in_process());
        let launchable = all.launchable();
        assert!(!launchable.processes);
        let sheets = crate::clients::find_app("sheets").unwrap();
        let terminal = crate::clients::find_app("terminal").unwrap();
        assert!(launchable.allows(&sheets));
        assert!(!launchable.allows(&terminal));
        // The desktop's default launchable follows the platform alone.
        assert_eq!(Launchable::default().processes, crate::host::processes_available());
    }
}
