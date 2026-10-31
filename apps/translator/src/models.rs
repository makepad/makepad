//! The phone's models: what is installed, what is downloading, and the
//! interface the real downloader (verify, resume, hub or mirror) implements.
//!
//! The screens only know [`ModelStore`]. [`FakeStore`] stands in for it with a
//! timeline of states so the cards and rows can be built and looked at before
//! the real store exists; it is used when the environment says so
//! (`MAKEPAD_TRANSLATOR_FAKE_STORE=1`).

use std::time::{Duration, Instant};

/// What a model is for. Each capability turns on by itself as its model is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Text,
    Camera,
    Listen,
    Speak,
}

impl Capability {
    pub fn label(self) -> &'static str {
        match self {
            Capability::Text => "Translation",
            Capability::Camera => "Camera",
            Capability::Listen => "Listening",
            Capability::Speak => "Speaking",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Absent,
    Downloading { done: u64, total: u64, bytes_per_s: u64 },
    Paused { done: u64, total: u64 },
    Verifying,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelState {
    pub id: &'static str,
    pub capability: Capability,
    pub label: &'static str,
    pub bytes: u64,
    pub state: State,
}

pub trait ModelStore: Send {
    fn models(&self) -> Vec<ModelState>;
    /// Start (or resume) downloading; no-op when ready.
    fn start(&mut self, id: &str);
    fn pause(&mut self, id: &str);
    fn cancel(&mut self, id: &str);
    /// Delete everything the store put on the phone.
    fn remove_all(&mut self);
}

pub fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", (bytes + (1 << 19)) >> 20)
    }
}

/// One row of the Models card as text: `Downloading 1.2 / 3.1 GB · 8 MB/s`.
pub fn state_text(m: &ModelState) -> String {
    match &m.state {
        State::Absent => format!("{} · not installed", size_text(m.bytes)),
        State::Downloading { done, total, bytes_per_s } => {
            format!("{} / {} · {} MB/s", size_text(*done), size_text(*total), bytes_per_s >> 20)
        }
        State::Paused { done, total } => format!("Paused at {} / {}", size_text(*done), size_text(*total)),
        State::Verifying => "Checking the download…".to_string(),
        State::Ready => format!("Ready · {}", size_text(m.bytes)),
        State::Failed(why) => format!("Failed: {why}"),
    }
}

/// The text on the row's button for this state.
pub fn action_text(state: &State) -> &'static str {
    match state {
        State::Absent => "Download",
        State::Downloading { .. } => "Pause",
        State::Paused { .. } => "Resume",
        State::Verifying => "",
        State::Ready => "",
        State::Failed(_) => "Retry",
    }
}

/// The share of the file that is on the phone, 0..=1.
pub fn fraction(state: &State) -> f32 {
    match state {
        State::Downloading { done, total, .. } | State::Paused { done, total } => (*done as f64 / (*total).max(1) as f64) as f32,
        State::Verifying | State::Ready => 1.0,
        _ => 0.0,
    }
}

/// Everything not yet ready, as one line for the offer card.
pub fn offer_text(models: &[ModelState]) -> String {
    let missing: u64 = models.iter().filter(|m| m.state != State::Ready).map(|m| m.bytes).sum();
    format!(
        "Translate offline? Download the models ({}).",
        size_text(missing)
    )
}

pub fn all_ready(models: &[ModelState]) -> bool {
    models.iter().all(|m| m.state == State::Ready)
}

// ----------------------------------------------------------------- fake

struct Fake {
    state: ModelState,
    /// When the download (re)started and how much was done then.
    since: Option<(Instant, u64)>,
    fail_once: bool,
}

/// A store that downloads nothing: each model "downloads" at a fixed speed, is
/// "verified" for a moment, and the last (speaking) fails once so the failure
/// row can be seen.
pub struct FakeStore {
    models: Vec<Fake>,
    speed: u64,
}

impl FakeStore {
    pub fn new() -> FakeStore {
        let mut spec = vec![
            ("qwen", Capability::Text, "Qwen3.5-4B", 3_143_656_608u64, false),
            ("vision", Capability::Camera, "Vision tower", 672_423_616, false),
            ("whisper", Capability::Listen, "Whisper", 539_000_000, false),
            ("kokoro", Capability::Speak, "Kokoro voices", 327_000_000, true),
        ];
        let models = spec
            .drain(..)
            .map(|(id, capability, label, bytes, fail_once)| Fake {
                state: ModelState { id, capability, label, bytes, state: State::Absent },
                since: None,
                fail_once,
            })
            .collect();
        // 40 MB/s: the 3 GB model takes ~80 s to watch.
        FakeStore { models, speed: 40 << 20 }
    }

    fn step(&mut self) {
        let speed = self.speed;
        for m in &mut self.models {
            let Some((t0, base)) = m.since else { continue };
            let total = m.state.bytes;
            let done = base + (t0.elapsed().as_secs_f64() * speed as f64) as u64;
            if done >= total {
                if matches!(m.state.state, State::Downloading { .. }) {
                    m.state.state = State::Verifying;
                    m.since = Some((Instant::now(), total));
                } else if matches!(m.state.state, State::Verifying) && t0.elapsed() > Duration::from_secs(2) {
                    m.state.state = if m.fail_once {
                        m.fail_once = false;
                        m.since = None;
                        State::Failed("the download did not match its checksum".to_string())
                    } else {
                        m.since = None;
                        State::Ready
                    };
                }
            } else if matches!(m.state.state, State::Downloading { .. }) {
                m.state.state = State::Downloading { done, total, bytes_per_s: speed };
            }
        }
    }
}

impl ModelStore for FakeStore {
    fn models(&self) -> Vec<ModelState> {
        // `step` needs &mut; the screens poll through `&self`, so work on a copy.
        let mut copy = FakeStore { models: self.models.iter().map(|m| Fake { state: m.state.clone(), since: m.since, fail_once: m.fail_once }).collect(), speed: self.speed };
        copy.step();
        copy.models.into_iter().map(|m| m.state).collect()
    }

    fn start(&mut self, id: &str) {
        self.step();
        if let Some(m) = self.models.iter_mut().find(|m| m.state.id == id) {
            let base = match m.state.state {
                State::Paused { done, .. } => done,
                State::Ready | State::Downloading { .. } | State::Verifying => return,
                _ => 0,
            };
            m.state.state = State::Downloading { done: base, total: m.state.bytes, bytes_per_s: self.speed };
            m.since = Some((Instant::now(), base));
        }
    }

    fn pause(&mut self, id: &str) {
        self.step();
        if let Some(m) = self.models.iter_mut().find(|m| m.state.id == id) {
            if let State::Downloading { done, total, .. } = m.state.state {
                m.state.state = State::Paused { done, total };
                m.since = None;
            }
        }
    }

    fn cancel(&mut self, id: &str) {
        if let Some(m) = self.models.iter_mut().find(|m| m.state.id == id) {
            if m.state.state != State::Ready {
                m.state.state = State::Absent;
                m.since = None;
            }
        }
    }

    fn remove_all(&mut self) {
        for m in &mut self.models {
            m.state.state = State::Absent;
            m.since = None;
        }
    }
}

// ------------------------------------------------------------- the real store

use makepad_ai_hub::model_store as hub_store;

/// The hub crate's installer (resumable, sha256-verified downloads from a
/// paired hub on the LAN first, then the mirror) behind this app's interface.
pub struct HubStore {
    inner: hub_store::ModelStore,
}

fn capability(c: hub_store::Capability) -> Capability {
    match c {
        hub_store::Capability::Text => Capability::Text,
        hub_store::Capability::Vision => Capability::Camera,
        hub_store::Capability::Listen => Capability::Listen,
        hub_store::Capability::Speak => Capability::Speak,
    }
}

fn state(p: hub_store::Phase) -> State {
    match p {
        hub_store::Phase::Absent => State::Absent,
        hub_store::Phase::Downloading { done, total, bytes_per_s } => State::Downloading { done, total, bytes_per_s },
        hub_store::Phase::Paused { done, total } => State::Paused { done, total },
        hub_store::Phase::Verifying => State::Verifying,
        hub_store::Phase::Ready => State::Ready,
        hub_store::Phase::Failed(why) => State::Failed(why),
    }
}

impl HubStore {
    /// `lan_hub` is the paired hub's base URL (`https://host:port`), tried
    /// before `mirror`.
    pub fn new(home: std::path::PathBuf, lan_hub: Option<&str>, mirror: &str, wake: Option<hub_store::Wake>) -> HubStore {
        HubStore { inner: hub_store::ModelStore::new(home, hub_store::phone_translator_catalogue(lan_hub, mirror), wake) }
    }
}

impl ModelStore for HubStore {
    fn models(&self) -> Vec<ModelState> {
        self.inner
            .models()
            .into_iter()
            .map(|m| ModelState { id: m.id, capability: capability(m.capability), label: m.label, bytes: m.bytes, state: state(m.state) })
            .collect()
    }

    fn start(&mut self, id: &str) {
        self.inner.start(id);
    }

    fn pause(&mut self, id: &str) {
        self.inner.pause(id);
    }

    fn cancel(&mut self, id: &str) {
        self.inner.cancel(id);
    }

    fn remove_all(&mut self) {
        for m in self.inner.models() {
            self.inner.remove(m.id);
        }
    }
}

/// Where the phone files are mirrored when no paired hub holds them
/// (`MAKEPAD_TRANSLATOR_MIRROR` overrides; the public host is not decided yet).
const DEFAULT_MIRROR: &str = "https://models.makepad.nl/phone";

/// The store the app uses: the fake one when asked for (development), the real
/// installer on a phone, nothing elsewhere.
pub fn open(lan_hub: Option<&str>, wake: Option<hub_store::Wake>) -> Option<Box<dyn ModelStore>> {
    if std::env::var("MAKEPAD_TRANSLATOR_FAKE_STORE").is_ok() {
        return Some(Box::new(FakeStore::new()));
    }
    let home = crate::ondevice::install_home()?;
    let mirror = std::env::var("MAKEPAD_TRANSLATOR_MIRROR").unwrap_or_else(|_| DEFAULT_MIRROR.to_string());
    Some(Box::new(HubStore::new(home, lan_hub, &mirror, wake)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_say_what_the_state_is() {
        let mut m = ModelState { id: "x", capability: Capability::Text, label: "X", bytes: 3_143_656_608, state: State::Absent };
        assert_eq!(state_text(&m), "2.9 GB · not installed");
        assert_eq!(action_text(&m.state), "Download");
        m.state = State::Downloading { done: 1 << 30, total: 3 << 30, bytes_per_s: 8 << 20 };
        assert_eq!(state_text(&m), "1.0 GB / 3.0 GB · 8 MB/s");
        assert!((fraction(&m.state) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(action_text(&m.state), "Pause");
        m.state = State::Failed("no space".into());
        assert_eq!(action_text(&m.state), "Retry");
    }

    #[test]
    fn the_fake_store_downloads_pauses_and_fails_once() {
        let mut s = FakeStore::new();
        s.speed = 1 << 40; // instant
        s.start("kokoro");
        std::thread::sleep(Duration::from_millis(30));
        s.step();
        assert!(matches!(s.models[3].state.state, State::Verifying));
        s.models[3].since = Some((Instant::now() - Duration::from_secs(3), s.models[3].state.bytes));
        s.step();
        assert!(matches!(s.models[3].state.state, State::Failed(_)));
        s.start("kokoro");
        s.pause("kokoro");
        assert!(matches!(s.models[3].state.state, State::Paused { .. }));
        s.start("qwen");
        assert!(matches!(s.models[0].state.state, State::Downloading { .. }));
        s.remove_all();
        assert!(s.models.iter().all(|m| m.state.state == State::Absent));
    }
}
