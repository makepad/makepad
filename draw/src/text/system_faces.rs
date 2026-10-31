//! Installed fonts named by family (`FontMember{system: "Menlo"}`) and the
//! installed-font index the OS fallback reads.
//!
//! The UI thread never looks a family up itself: the first time a family
//! names a system face, the lookup goes to the task pool
//! ([`makepad_font_core::system_fonts::find_family`]); until it answers, the
//! face is pending, the family draws with its other members and is completed
//! on the redraw the answer brings (the pool raises the UI signal, a signal
//! hook redraws). After the first frame the index of installed fonts is
//! built in the background ([`makepad_font_core::system_fonts::prewarm`]),
//! so neither a by-name lookup nor the OS fallback waits for the scan.

use {
    crate::makepad_platform::{
        log,
        thread::{Lane, TaskHandle, TaskPool},
        Cx,
    },
    fxhash::FxHashMap,
    makepad_font_core::system_fonts::{self, SystemFont},
};

/// A system face as a font chain member asks for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SystemFaceKey {
    /// Lowercased family name, or [`system_fonts::SYSTEM_UI_FAMILY`].
    pub name: String,
    /// CSS weight, 100..=900.
    pub weight: u16,
    pub italic: bool,
}

impl SystemFaceKey {
    pub fn new(name: &str, weight: f32, italic: bool) -> Self {
        let weight = if weight.is_finite() && weight > 0.0 { weight.round().clamp(1.0, 1000.0) as u16 } else { 400 };
        Self { name: name.trim().to_lowercase(), weight, italic }
    }
}

/// Where a lookup stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SystemFaceState {
    Pending,
    Found(SystemFont),
    /// Not installed here (or no font discovery on this platform).
    Missing,
}

enum Entry {
    Pending(TaskHandle<Option<SystemFont>>),
    Done(Option<SystemFont>),
}

/// A `Cx` global: [`lookup`], [`start_prewarm`], [`has_news`].
#[derive(Default)]
pub(crate) struct SystemFaces {
    entries: FxHashMap<SystemFaceKey, Entry>,
    /// The background build of the installed-font index, while it runs.
    prewarm: Option<TaskHandle<()>>,
    prewarm_started: bool,
}

impl SystemFaces {
    /// The state of `key`'s lookup, starting it on the task pool the first
    /// time. Never waits.
    pub fn state(&mut self, pool: &TaskPool, key: &SystemFaceKey) -> SystemFaceState {
        let entry = match self.entries.get_mut(key) {
            Some(entry) => entry,
            None => {
                let name = key.name.clone();
                let (weight, italic) = (key.weight, key.italic);
                let entry = match pool.submit(Lane::Light, move || system_fonts::find_family(&name, weight, italic)) {
                    Ok(task) => Entry::Pending(task),
                    // No pool (a single-threaded web build): no OS fonts to find there either.
                    Err(_) => Entry::Done(None),
                };
                self.entries.entry(key.clone()).or_insert(entry)
            }
        };
        if let Entry::Pending(task) = entry {
            match task.try_take() {
                None => return SystemFaceState::Pending,
                Some(result) => {
                    let found = result.ok().flatten();
                    match &found {
                        Some(font) => log!("text: system font {:?} is {}#{}", key.name, font.path.display(), font.index),
                        None => log!("text: system font {:?} is not installed here; its family draws without it", key.name),
                    }
                    *entry = Entry::Done(found);
                }
            }
        }
        match entry {
            Entry::Done(Some(font)) => SystemFaceState::Found(font.clone()),
            Entry::Done(None) => SystemFaceState::Missing,
            Entry::Pending(_) => SystemFaceState::Pending,
        }
    }

    /// Starts the background build of the installed-font index, once (after
    /// the first frame).
    pub fn start_prewarm(&mut self, pool: &TaskPool) {
        if self.prewarm_started {
            return;
        }
        self.prewarm_started = true;
        if !system_fonts::needs_prewarm() {
            return;
        }
        if let Ok(task) = pool.submit(Lane::Heavy, system_fonts::prewarm) {
            self.prewarm = Some(task);
        }
    }

    /// Whether a lookup or the index build finished since the last draw took
    /// it in: the signal hook redraws for it.
    pub fn has_news(&mut self) -> bool {
        let lookup_done = self.entries.values().any(|entry| matches!(entry, Entry::Pending(task) if task.is_finished()));
        let prewarm_done = self.prewarm.as_ref().is_some_and(|task| task.is_finished());
        if prewarm_done {
            self.prewarm = None;
        }
        lookup_done || prewarm_done
    }
}

/// [`SystemFaces::state`] on the `Cx`'s global.
pub(crate) fn lookup(cx: &mut Cx, key: &SystemFaceKey) -> SystemFaceState {
    let pool = cx.task_pool();
    cx.global::<SystemFaces>().state(&pool, key)
}

/// [`SystemFaces::start_prewarm`] on the `Cx`'s global.
pub(crate) fn start_prewarm(cx: &mut Cx) {
    if cx.global::<SystemFaces>().prewarm_started {
        return;
    }
    let pool = cx.task_pool();
    cx.global::<SystemFaces>().start_prewarm(&pool);
}

/// [`SystemFaces::has_news`] on the `Cx`'s global (false before any lookup).
pub(crate) fn has_news(cx: &mut Cx) -> bool {
    cx.has_global::<SystemFaces>() && cx.get_global::<SystemFaces>().has_news()
}
