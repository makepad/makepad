//! The game library and the local librarian (game.md §"AI tiers").
//!
//! A game is a directory: `game.splash` + `manifest.toml`. The local model is
//! a librarian, never a codegen: it maps an utterance onto load / restart /
//! knob-set, and everything else goes to the cloud agent. Where no local model
//! exists, fuzzy name matching covers load and restart, which is most of what
//! anyone says out loud anyway.

use std::path::{Path, PathBuf};

/// A knob the manifest declares as safely settable without codegen.
#[derive(Clone, Debug, PartialEq)]
pub struct Knob {
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub players: u32,
    pub knobs: Vec<Knob>,
}

impl Manifest {
    /// Minimal TOML reader: `key = value`, `[knobs.<name>]` tables. The full
    /// format is not worth a dependency here, and a manifest that fails to
    /// parse must degrade to "a game with a name", never to an error dialog.
    pub fn parse(text: &str, fallback_name: &str) -> Self {
        let mut m = Manifest {
            name: fallback_name.to_string(),
            players: 1,
            ..Default::default()
        };
        let mut knob: Option<Knob> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                if let Some(k) = knob.take() {
                    m.knobs.push(k);
                }
                if let Some(name) = header.strip_prefix("knobs.") {
                    knob = Some(Knob {
                        name: name.trim().to_string(),
                        value: 0.0,
                        min: f64::NEG_INFINITY,
                        max: f64::INFINITY,
                    });
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            match (&mut knob, key) {
                (Some(k), "value") => k.value = value.parse().unwrap_or(0.0),
                (Some(k), "min") => k.min = value.parse().unwrap_or(f64::NEG_INFINITY),
                (Some(k), "max") => k.max = value.parse().unwrap_or(f64::INFINITY),
                (None, "name") => m.name = value.to_string(),
                (None, "description") => m.description = value.to_string(),
                (None, "players") => m.players = value.parse().unwrap_or(1),
                _ => {}
            }
        }
        if let Some(k) = knob.take() {
            m.knobs.push(k);
        }
        m
    }

    pub fn knob(&self, name: &str) -> Option<&Knob> {
        self.knobs.iter().find(|k| k.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameEntry {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl GameEntry {
    pub fn splash_path(&self) -> PathBuf {
        self.dir.join("game.splash")
    }
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub games: Vec<GameEntry>,
}

impl Library {
    /// Scan `root` for game directories. Missing root is an empty library, not
    /// an error — a fresh device has no games yet.
    pub fn scan(root: &Path) -> Self {
        let mut games = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.join("game.splash").is_file() {
                    continue;
                }
                let fallback = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let manifest = std::fs::read_to_string(dir.join("manifest.toml"))
                    .map(|t| Manifest::parse(&t, &fallback))
                    .unwrap_or(Manifest {
                        name: fallback,
                        players: 1,
                        ..Default::default()
                    });
                games.push(GameEntry { dir, manifest });
            }
        }
        games.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        Self { games }
    }

    pub fn find(&self, name: &str) -> Option<&GameEntry> {
        self.games
            .iter()
            .find(|g| g.manifest.name.eq_ignore_ascii_case(name))
    }
}

/// What the librarian decided to do with an utterance.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Load(PathBuf),
    Restart,
    SetKnob { name: String, value: f64 },
    /// Anything creative — only the cloud agent writes code.
    AskAgent(String),
}

/// The local model, when there is one. Returning None means "no opinion",
/// which falls through to the deterministic matcher — so a flaky or absent
/// local model can never block a request, only sharpen it.
pub trait Librarian {
    fn classify(&mut self, utterance: &str, library: &Library) -> Option<Action>;
}

/// No local model: fuzzy name matching for load/restart, everything else to
/// the agent.
pub struct FuzzyLibrarian;

impl Librarian for FuzzyLibrarian {
    fn classify(&mut self, _utterance: &str, _library: &Library) -> Option<Action> {
        None
    }
}

/// Deterministic fallback, and the final arbiter for what a local model
/// returns. Word-overlap scoring over name + description.
pub fn classify_deterministic(
    utterance: &str,
    library: &Library,
    current: Option<&Manifest>,
) -> Action {
    let lower = utterance.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .collect();

    if words.iter().any(|w| matches!(*w, "restart" | "again" | "reset" | "replay")) {
        return Action::Restart;
    }

    // A knob the CURRENT game declares: "make the cars faster" -> speed.
    if let Some(manifest) = current {
        for knob in &manifest.knobs {
            let kn = knob.name.to_lowercase();
            if !words.iter().any(|w| kn.contains(*w) || w.contains(&kn)) {
                continue;
            }
            let up = words.iter().any(|w| {
                matches!(*w, "faster" | "more" | "higher" | "bigger" | "increase" | "raise")
            });
            let down = words.iter().any(|w| {
                matches!(*w, "slower" | "less" | "lower" | "smaller" | "decrease" | "reduce")
            });
            if up || down {
                let step = if knob.value.abs() > f64::EPSILON {
                    knob.value * 0.25
                } else {
                    1.0
                };
                let value = (knob.value + if up { step } else { -step })
                    .clamp(knob.min, knob.max);
                return Action::SetKnob {
                    name: knob.name.clone(),
                    value,
                };
            }
        }
    }

    // "play/load the dogfight game" — only match on an explicit play verb, so
    // "make a racing game" still reaches the agent rather than loading one.
    let wants_load = words
        .iter()
        .any(|w| matches!(*w, "play" | "load" | "open" | "start" | "switch"));
    if wants_load {
        let mut best: Option<(usize, &GameEntry)> = None;
        for game in &library.games {
            let hay = format!(
                "{} {}",
                game.manifest.name.to_lowercase(),
                game.manifest.description.to_lowercase()
            );
            let score = words.iter().filter(|w| hay.contains(**w)).count();
            if score > 0 && best.map(|(b, _)| score > b).unwrap_or(true) {
                best = Some((score, game));
            }
        }
        if let Some((_, game)) = best {
            return Action::Load(game.splash_path());
        }
    }

    Action::AskAgent(utterance.to_string())
}

/// Route an utterance: ask the local model if there is one, else (and on any
/// "no opinion") fall back to the deterministic matcher.
pub fn route(
    utterance: &str,
    library: &Library,
    current: Option<&Manifest>,
    librarian: Option<&mut dyn Librarian>,
) -> Action {
    if let Some(l) = librarian {
        if let Some(action) = l.classify(utterance, library) {
            return action;
        }
    }
    classify_deterministic(utterance, library, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        Library {
            games: vec![
                GameEntry {
                    dir: PathBuf::from("/games/racing"),
                    manifest: Manifest {
                        name: "racing".into(),
                        description: "drive cars around an oval track".into(),
                        players: 4,
                        knobs: vec![Knob {
                            name: "speed".into(),
                            value: 20.0,
                            min: 5.0,
                            max: 60.0,
                        }],
                    },
                },
                GameEntry {
                    dir: PathBuf::from("/games/dogfight"),
                    manifest: Manifest {
                        name: "dogfight".into(),
                        description: "fly planes and shoot each other".into(),
                        players: 6,
                        knobs: vec![],
                    },
                },
            ],
        }
    }

    #[test]
    fn manifest_parses_fields_and_knobs() {
        let m = Manifest::parse(
            r#"
            name = "racing"
            description = "drive cars"
            players = 4
            [knobs.speed]
            value = 20.0
            min = 5.0
            max = 60.0
            [knobs.gravity]
            value = 18.0
            "#,
            "fallback",
        );
        assert_eq!(m.name, "racing");
        assert_eq!(m.players, 4);
        assert_eq!(m.knobs.len(), 2);
        assert_eq!(m.knob("speed").unwrap().max, 60.0);
        // An unparsable manifest still yields a usable game.
        let empty = Manifest::parse("!!! not toml", "my-game");
        assert_eq!(empty.name, "my-game");
    }

    #[test]
    fn load_by_description_not_just_name() {
        let lib = library();
        assert_eq!(
            route("play the one with the planes", &lib, None, None),
            Action::Load(PathBuf::from("/games/dogfight/game.splash"))
        );
        assert_eq!(
            route("load racing", &lib, None, None),
            Action::Load(PathBuf::from("/games/racing/game.splash"))
        );
    }

    #[test]
    fn creative_requests_reach_the_agent() {
        let lib = library();
        // "make a racing game" must NOT load the existing racing game.
        let a = route("make a new racing game with boats", &lib, None, None);
        assert!(matches!(a, Action::AskAgent(_)), "{a:?}");
        let a = route("add a jump ramp", &lib, None, None);
        assert!(matches!(a, Action::AskAgent(_)), "{a:?}");
    }

    #[test]
    fn knob_writes_are_parameter_edits_and_clamp() {
        let lib = library();
        let current = lib.find("racing").unwrap().manifest.clone();
        assert_eq!(
            route("make the speed faster", &lib, Some(&current), None),
            Action::SetKnob { name: "speed".into(), value: 25.0 }
        );
        let mut slow = current.clone();
        slow.knobs[0].value = 6.0;
        // Clamped to the manifest's declared floor, never below.
        assert_eq!(
            route("speed slower", &lib, Some(&slow), None),
            Action::SetKnob { name: "speed".into(), value: 5.0 }
        );
    }

    #[test]
    fn restart_is_recognised_without_a_model() {
        let lib = library();
        assert_eq!(route("restart please", &lib, None, None), Action::Restart);
    }

    #[test]
    fn a_local_model_can_override_but_no_opinion_falls_through() {
        struct Stub(Option<Action>);
        impl Librarian for Stub {
            fn classify(&mut self, _u: &str, _l: &Library) -> Option<Action> {
                self.0.clone()
            }
        }
        let lib = library();
        let mut stub = Stub(Some(Action::Restart));
        assert_eq!(
            route("anything at all", &lib, None, Some(&mut stub)),
            Action::Restart
        );
        // No opinion -> deterministic matcher still routes correctly.
        let mut quiet = Stub(None);
        let a = route("build me a maze", &lib, None, Some(&mut quiet));
        assert!(matches!(a, Action::AskAgent(_)), "{a:?}");
    }
}
