//! The built-in apps registry's hosting dimension (aicontrol.md §4): which
//! apps are linked in as MODULES, and which of those the person has
//! switched to module hosting.
//!
//! The launch table (`clients::registry()`: package, directory, binary,
//! launch policy — everything a PROCESS needs) stays where it is; this is
//! the overlay keyed by the same ids: the linked `AppModule`, and the
//! hosting each app gets. The linked modules come from the BUILD
//! (`build.rs`): the desktop `wm` binary links none, the all-in-one links
//! its app crates. Desktop default is Process (decision 5): a linked
//! module is still launched as a process unless `~/.makepad/wm/apps.splash`
//! says otherwise (a settings file, never an environment variable) or a
//! dev run passes `--module <id>`. A build without processes — the web,
//! iOS, or a desktop all-in-one with `modules_only` — ignores the switch:
//! everything is a module there.

use crate::build::WmBuild;
use crate::clients::AppDef;
use makepad_app_module::AppModule;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hosting {
    Process,
    Module,
}

/// What a menu may offer to launch: the linked modules, and — where this
/// build hosts processes — every registry app this checkout can run.
/// Handed to the shell surfaces (the launcher, the dock, the phone home
/// page) so a build without processes never lists an app it cannot start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launchable {
    pub linked: Vec<&'static str>,
    pub processes: bool,
}

impl Default for Launchable {
    /// The desktop before its build is read: processes where the platform
    /// has them, nothing linked.
    fn default() -> Self {
        Launchable { linked: Vec::new(), processes: crate::host::processes_available() }
    }
}

impl Launchable {
    pub fn allows(&self, app: &AppDef) -> bool {
        self.linked.contains(&app.id.as_str()) || (self.processes && app.is_available())
    }
}

pub struct AppRegistry {
    modules: Vec<&'static dyn AppModule>,
    overrides: HashMap<String, Hosting>,
    /// This build hosts processes (the platform can, and the build wants to).
    processes: bool,
    /// This build links the assistant's widget families (`WmBuild::assistant`).
    assistant: bool,
}

impl Default for AppRegistry {
    fn default() -> Self {
        AppRegistry {
            modules: Vec::new(),
            overrides: HashMap::new(),
            processes: crate::host::processes_available(),
            assistant: false,
        }
    }
}

impl AppRegistry {
    /// The registry for a build, with the person's overrides: the settings
    /// file first, then the command line's `--module <id>` flags on top.
    /// `processes` is the host's answer (`App::processes`): the platform's
    /// capability and the build's `modules_only` together.
    pub fn load(settings: &Path, args: &[String], build: &WmBuild, processes: bool) -> Self {
        let mut registry = AppRegistry {
            modules: build.modules.clone(),
            overrides: HashMap::new(),
            processes,
            assistant: build.assistant.is_some(),
        };
        if let Ok(text) = std::fs::read_to_string(settings) {
            for (id, hosting) in Self::parse_overrides(&text) {
                registry.overrides.insert(id, hosting);
            }
        }
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--module" {
                if let Some(id) = args.get(i + 1) {
                    registry.overrides.insert(id.to_lowercase(), Hosting::Module);
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
    /// module is linked AND the person (or the dev flag) asked for it. In a
    /// build without processes (the web, iOS, a modules-only desktop
    /// build): every linked module is a module, and everything else is
    /// simply not there.
    pub fn hosting(&self, id: &str) -> Hosting {
        if !self.processes {
            return if self.module(id).is_some() { Hosting::Module } else { Hosting::Process };
        }
        match self.overrides.get(id) {
            Some(Hosting::Module) if self.module(id).is_some() => Hosting::Module,
            _ => Hosting::Process,
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

    /// What the menus may offer (see [`Launchable`]).
    pub fn launchable(&self) -> Launchable {
        Launchable { linked: self.linked_ids(), processes: self.processes }
    }

    /// `~/.makepad/wm/apps.splash`: one `id: Module` or `id: Process` per
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
    fn hosting_is_process_unless_a_linked_module_is_switched_on() {
        let flags = ["--module".to_string(), "sheets".to_string(), "--module".to_string(), "files".to_string()];
        let registry = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &flags, &build(false, false), true);
        // files has no linked module: the flag cannot make it one.
        assert_eq!(registry.hosting("files"), Hosting::Process);
        assert_eq!(registry.hosting("terminal"), Hosting::Process);
        assert_eq!(registry.hosting("sheets"), Hosting::Module);
        assert!(registry.linked_ids().contains(&"sheets"));
        assert!(!registry.pane_in_process(), "no assistant linked");
        let plain = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &[], &build(false, true), true);
        assert_eq!(plain.hosting("sheets"), Hosting::Process, "desktop default is a process");
        assert!(!plain.pane_in_process(), "the desktop's assistant is the child process unless switched");
        assert_eq!(plain.launchable(), Launchable { linked: vec!["sheets"], processes: true });
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
