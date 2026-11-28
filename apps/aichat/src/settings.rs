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
        if self.local_only && self.provider.is_cloud() {
            self.provider = ProviderChoice::Local;
        }
        self
    }

    /// Can this choice be made under the current lock?
    pub fn allows(&self, choice: &ProviderChoice) -> Result<(), String> {
        if self.local_only && choice.is_cloud() {
            return Err("Local AI only is on".into());
        }
        Ok(())
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
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Self::path_from(var("MAKEPAD_AICHAT_DIR"), var("MAKEPAD_TEST_HOME"), var("HOME").or_else(|| var("USERPROFILE")))
    }

    /// A hidden test instance keeps its settings and saved chats apart: in its
    /// `MAKEPAD_AICHAT_DIR`, else below an absolute `MAKEPAD_TEST_HOME`, as
    /// `makepad_platform::home::app_state_home` keeps an app's own state. Fab's
    /// chats went to a box's own `~/.makepad` from a lane's run (11-27).
    fn path_from(dir: Option<PathBuf>, test_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
        if let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) {
            return Some(dir.join("settings"));
        }
        let home = test_home.filter(|h| h.is_absolute()).or(home)?;
        Some(home.join(".makepad").join("aichat").join("settings"))
    }

    pub fn load() -> AiSettings {
        Self::load_with_provider("")
    }

    /// A host supplies the first provider; an explicit saved pick survives relaunch.
    pub fn load_with_provider(provider: &str) -> AiSettings {
        let saved = Self::path().and_then(|p| std::fs::read_to_string(p).ok());
        Self::saved_or_provider(saved.as_deref(), provider)
    }

    /// The settings a named provider implies: a cloud model lifts the lock.
    pub fn for_provider(slug: &str) -> AiSettings {
        let provider = ProviderChoice::from_slug(slug);
        let local_only = !provider.is_cloud();
        AiSettings { provider, local_only }
    }

    fn saved_or_provider(saved: Option<&str>, provider: &str) -> AiSettings {
        if let Some(text) = saved {
            Self::parse(text)
        } else if !provider.trim().is_empty() {
            Self::for_provider(provider)
        } else {
            Self::default()
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
        // The hub's Claude is a cloud model; the hub's Qwen is not.
        assert!(s.allows(&ProviderChoice::Hub("claude-cli".into())).is_err());
        assert!(s.allows(&ProviderChoice::Hub("qwen3.5-4b".into())).is_ok());
        let open = AiSettings::parse("provider=claude-api\nlocal_only=false\n");
        assert_eq!(open.provider, ProviderChoice::Cloud("claude-api".into()));
        assert_eq!(AiSettings::parse(&open.render()), open);
        #[cfg(feature = "engine")]
        for &(id, _) in makepad_ai_services::engine::models::CLAUDE_MODELS {
            for base in ["claude-cli", "claude-api"] {
                let choice = ProviderChoice::from_slug(&format!("{base}:{id}"));
                let picked = AiSettings { provider: choice.clone(), local_only: false };
                assert_eq!(AiSettings::saved_or_provider(Some(&picked.render()), "claude-cli"), picked, "saved model beats host default");
                assert!(s.allows(&choice).is_err(), "model suffix cannot bypass local-only lock");
            }
        }
        assert_eq!(AiSettings::saved_or_provider(None, "claude-cli").provider.slug(), "claude-cli");
        assert_eq!(AiSettings::parse("garbage"), AiSettings::default());
    }

    #[test]
    fn a_test_instance_keeps_its_settings_and_chats_below_its_test_home() {
        let (home, test) = (PathBuf::from("person-home"), std::env::temp_dir().join("test-home"));
        let path = |dir: Option<&str>, test_home: Option<PathBuf>| AiSettings::path_from(dir.map(PathBuf::from), test_home, Some(home.clone()));
        assert_eq!(path(None, None), Some(home.join(".makepad/aichat/settings")), "no test home: the person's own, as before");
        assert_eq!(path(None, Some(test.clone())), Some(test.join(".makepad/aichat/settings")), "an absolute test home holds a test instance's");
        assert_eq!(path(None, Some(PathBuf::from("rel"))), Some(home.join(".makepad/aichat/settings")), "a relative test home is no test home");
        assert_eq!(path(Some("chat-dir"), Some(test)), Some(PathBuf::from("chat-dir").join("settings")), "MAKEPAD_AICHAT_DIR comes first");
        assert_eq!(path(Some(""), None), Some(home.join(".makepad/aichat/settings")), "an empty MAKEPAD_AICHAT_DIR counts as unset");
    }
}
