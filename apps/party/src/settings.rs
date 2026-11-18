//! The host's settings, kept across starts in `<makepad home>/party/settings.conf`
//! (one `key = value` a line): how the live redraw computes. Each is a trade
//! between speed and staying exact to the model's reference; Fast is the
//! default, and the host picks Exact on the big screen. The file holds
//! nothing of any guest.
//!
//! - `live_arithmetic = fast | exact`: the hub's fast arithmetic, or the
//!   model's own.
//! - `live_prompt = fast | exact`: the model reads the prompt's own words,
//!   or the whole text window its reference reads (pad rows after them).

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::hub::{Arithmetic, Compute};
use std::path::PathBuf;
use std::sync::Mutex;

/// One side of a speed-against-exactness setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    #[default]
    Fast,
    Exact,
}

impl Detail {
    fn name(self) -> &'static str {
        match self {
            Detail::Fast => "fast",
            Detail::Exact => "exact",
        }
    }

    fn parse(value: &str) -> Option<Detail> {
        match value {
            "fast" => Some(Detail::Fast),
            "exact" => Some(Detail::Exact),
            _ => None,
        }
    }
}

/// How the live redraw computes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveSettings {
    pub arithmetic: Detail,
    pub prompt: Detail,
}

impl LiveSettings {
    /// What a live session asks the hub for.
    pub fn compute(self) -> Compute {
        Compute {
            arithmetic: match self.arithmetic {
                Detail::Fast => Arithmetic::Fast,
                Detail::Exact => Arithmetic::Stored,
            },
            text_rows: match self.prompt {
                Detail::Fast => Some(0),
                Detail::Exact => None,
            },
        }
    }

    /// The settings in `text`; a line it does not know keeps the default.
    fn parse(text: &str) -> LiveSettings {
        let mut live = LiveSettings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let Some(detail) = Detail::parse(value.trim()) else { continue };
            match key.trim() {
                "live_arithmetic" => live.arithmetic = detail,
                "live_prompt" => live.prompt = detail,
                _ => {}
            }
        }
        live
    }

    fn render(self) -> String {
        format!("live_arithmetic = {}\nlive_prompt = {}\n", self.arithmetic.name(), self.prompt.name())
    }
}

/// The host's settings, shared by the screen (which changes them) and the
/// live thread (which reads them as a session opens). The lock is held for
/// a copy, never across the file.
#[derive(Default)]
pub struct Settings {
    /// Where they are saved; `None`: in memory only (tests).
    path: Option<PathBuf>,
    live: Mutex<LiveSettings>,
}

impl Settings {
    /// The party's settings file.
    pub fn file() -> PathBuf {
        makepad_network::fleet::makepad_home().join("party").join("settings.conf")
    }

    /// The settings saved at `path`, the defaults when there are none.
    pub fn load(path: PathBuf) -> Settings {
        let live = match std::fs::read_to_string(&path) {
            Ok(text) => LiveSettings::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LiveSettings::default(),
            Err(e) => {
                eprintln!("[party] settings {}: {e}; the defaults for now", path.display());
                LiveSettings::default()
            }
        };
        Settings { path: Some(path), live: Mutex::new(live) }
    }

    pub fn live(&self) -> LiveSettings {
        *self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Takes `live` from now on (the next live session computes so) and
    /// saves it.
    pub fn set_live(&self, live: LiveSettings) -> Result<(), String> {
        *self.live.lock().unwrap_or_else(|p| p.into_inner()) = live;
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let new = path.with_extension("conf.new");
        std::fs::write(&new, live.render()).map_err(|e| format!("{}: {e}", new.display()))?;
        std::fs::rename(&new, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_runs_fast_until_the_host_picks_exact_and_the_pick_outlasts_a_restart() {
        let fast = LiveSettings::default();
        assert_eq!(fast.compute(), Compute { arithmetic: Arithmetic::Fast, text_rows: Some(0) }, "fast: the fast arithmetic over the prompt's own words");
        let exact = LiveSettings { arithmetic: Detail::Exact, prompt: Detail::Exact };
        assert_eq!(exact.compute(), Compute::STORED, "exact: the model's own arithmetic over its whole text window");
        assert_eq!(LiveSettings::parse("live_prompt = exact\nlive_arithmetic=weird\n# a note\n"), LiveSettings { arithmetic: Detail::Fast, prompt: Detail::Exact }, "a line it does not know keeps the default");

        let path = std::env::temp_dir().join(format!("party-settings-{}", std::process::id())).join("settings.conf");
        assert_eq!(Settings::load(path.clone()).live(), fast, "no file: the defaults");
        Settings::load(path.clone()).set_live(exact).unwrap();
        assert_eq!(Settings::load(path.clone()).live(), exact, "a restart keeps the host's pick");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
