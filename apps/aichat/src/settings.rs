//! What the person chose: which model answers, and whether cloud models
//! are locked out. Two lines in `~/.makepad/aichat/settings`, the lock a
//! promise kept in code (a cloud choice under the lock normalises to
//! local at load and is refused at set), never a menu filter.

use makepad_ai_services::state::ProviderChoice;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct AiSettings {
    pub provider: ProviderChoice,
    /// Default ON: the person must switch it off before any cloud model
    /// can be picked.
    pub local_only: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings { provider: ProviderChoice::Local, local_only: true }
    }
}

impl AiSettings {
    /// The lock, applied: a cloud provider under the lock becomes local.
    /// "none" (no model) is always allowed — it reaches nothing.
    pub fn normalized(mut self) -> Self {
        if self.local_only {
            if let ProviderChoice::Cloud(slug) = &self.provider {
                if slug != "none" {
                    self.provider = ProviderChoice::Local;
                }
            }
        }
        self
    }

    /// Can this choice be made under the current lock?
    pub fn allows(&self, choice: &ProviderChoice) -> Result<(), String> {
        match choice {
            ProviderChoice::Cloud(slug) if self.local_only && slug != "none" => Err("Local AI only is on".into()),
            _ => Ok(()),
        }
    }

    pub fn parse(text: &str) -> AiSettings {
        let mut s = AiSettings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "provider" => s.provider = ProviderChoice::from_slug(v.trim()),
                "local_only" => s.local_only = v.trim() != "false",
                _ => {}
            }
        }
        s.normalized()
    }

    pub fn render(&self) -> String {
        format!("provider={}\nlocal_only={}\n", self.provider.slug(), self.local_only)
    }

    pub fn path() -> Option<PathBuf> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".makepad").join("aichat").join("settings"))
    }

    pub fn load() -> AiSettings {
        match Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => Self::parse(&text),
            None => AiSettings::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, self.render()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_kept_at_load_and_at_set() {
        let s = AiSettings::parse("provider=claude-api\nlocal_only=true\n");
        assert_eq!(s.provider, ProviderChoice::Local, "a cloud pref under the lock cannot resurrect a cloud model");
        assert!(s.allows(&ProviderChoice::Cloud("claude-api".into())).is_err());
        assert!(s.allows(&ProviderChoice::Cloud("none".into())).is_ok());
        let open = AiSettings::parse("provider=claude-api\nlocal_only=false\n");
        assert_eq!(open.provider, ProviderChoice::Cloud("claude-api".into()));
        assert_eq!(AiSettings::parse(&open.render()), open);
        assert_eq!(AiSettings::parse("garbage"), AiSettings::default());
    }
}
