//! The built-in apps registry's hosting dimension (aicontrol.md §4): which
//! apps are linked in as MODULES, and which of those the person has
//! switched to module hosting.
//!
//! The launch table (`clients::registry()`: package, directory, binary,
//! launch policy — everything a PROCESS needs) stays where it is; this is
//! the overlay keyed by the same ids: the linked `AppModule`, and the
//! hosting each app gets. Desktop default is Process (decision 5): a
//! linked module is still launched as a process unless
//! `~/.makepad/wm/apps.splash` says otherwise (a settings file, never an
//! environment variable) or a dev run passes `--module <id>`. The uber
//! builds ignore the switch: everything is a module there.

use makepad_app_module::AppModule;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hosting {
    Process,
    Module,
}

pub struct AppRegistry {
    modules: Vec<&'static dyn AppModule>,
    overrides: HashMap<String, Hosting>,
}

impl Default for AppRegistry {
    fn default() -> Self {
        AppRegistry { modules: linked_modules(), overrides: HashMap::new() }
    }
}

/// A linked module by id, without a registry: what the launcher asks.
pub fn is_linked(id: &str) -> bool {
    linked_modules().iter().any(|m| m.id() == id)
}

/// The modules this build links, one entry per `app-*` feature.
fn linked_modules() -> Vec<&'static dyn AppModule> {
    let mut out: Vec<&'static dyn AppModule> = Vec::new();
    #[cfg(feature = "app-sheets")]
    out.push(&makepad_sheets::SHEETS_MODULE);
    #[cfg(feature = "app-photos")]
    out.push(&makepad_photos::PHOTOS_MODULE);
    out
}

impl AppRegistry {
    /// The registry with the person's overrides: the settings file first,
    /// then the command line's `--module <id>` flags on top.
    pub fn load(settings: &Path, args: &[String]) -> Self {
        let mut registry = Self::default();
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

    /// How a launch of `id` is hosted. On a desktop: Module only when a
    /// module is linked AND the person (or the dev flag) asked for it. In a
    /// build without processes (the web): every linked module is a module,
    /// and everything else is simply not there.
    pub fn hosting(&self, id: &str) -> Hosting {
        if !crate::host::processes_available() {
            return if self.module(id).is_some() { Hosting::Module } else { Hosting::Process };
        }
        match self.overrides.get(id) {
            Some(Hosting::Module) if self.module(id).is_some() => Hosting::Module,
            _ => Hosting::Process,
        }
    }

    /// Whether the assistant is the aichat MODULE seated in the pane
    /// in-process (feature `app-aichat`): always where there are no
    /// processes; on a desktop only when `aichat` is switched to module
    /// hosting, the child process being the default.
    pub fn pane_in_process(&self) -> bool {
        if !cfg!(feature = "app-aichat") {
            return false;
        }
        !crate::host::processes_available() || self.overrides.get("aichat") == Some(&Hosting::Module)
    }

    pub fn linked_ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
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
        let registry = AppRegistry::load(Path::new("/nonexistent/apps.splash"), &["--module".to_string(), "sheets".to_string(), "--module".to_string(), "files".to_string()]);
        // files has no linked module: the flag cannot make it one.
        assert_eq!(registry.hosting("files"), Hosting::Process);
        assert_eq!(registry.hosting("terminal"), Hosting::Process);
        #[cfg(feature = "app-sheets")]
        {
            assert_eq!(registry.hosting("sheets"), Hosting::Module);
            assert!(registry.linked_ids().contains(&"sheets"));
            let plain = AppRegistry::default();
            assert_eq!(plain.hosting("sheets"), Hosting::Process, "desktop default is a process");
        }
    }
}
