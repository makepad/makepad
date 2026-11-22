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
//!
//! Both are the `Cx`'s asynchronous loads of kind "font"
//! ([`Cx::pending_async_loads`]): a frame drawn before they land draws the
//! text without them, so a step that waits for loads draws them in the same
//! frame however slowly the pool runs.

use {
    crate::{
        makepad_platform::{
            log,
            thread::{Class, TaskHandle, TaskPool},
            Cx,
        },
        text::fonts::Fonts,
    },
    fxhash::FxHashMap,
    makepad_font_core::system_fonts::{self, SystemFont},
    std::{cell::RefCell, rc::Rc},
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
                let entry = match pool.submit(Class::Visible, move || system_fonts::find_family(&name, weight, italic)) {
                    Ok(task) => Entry::Pending(task),
                    // No pool (a single-threaded web build): no OS fonts to find there either.
                    Err(_) => Entry::Done(None),
                };
                self.entries.entry(key.clone()).or_insert(entry)
            }
        };
        land(key, entry);
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
        if let Ok(task) = pool.submit(Class::Background, system_fonts::prewarm) {
            self.prewarm = Some(task);
        }
    }

    /// Whether a lookup or the index build finished since the last draw took
    /// it in: the signal hook redraws for it. The index's arrival is news only
    /// when a glyph of this `Cx` waited for it (`index_waited`): a `Cx` whose
    /// text missed nothing, or that draws none, has nothing to draw again, and
    /// a whole redraw at a moment the background scan sets lands in the
    /// middle of its work (an offscreen render's one draw drawn twice under
    /// load, 11-11).
    /// A finished lookup is taken in here, where its redraw is asked: until
    /// then it is in flight ([`Self::in_flight`]), and a step that waits for
    /// loads would wait for a draw that only the step can give.
    pub fn has_news(&mut self, index_waited: bool) -> bool {
        let mut lookup_done = false;
        for (key, entry) in &mut self.entries {
            lookup_done |= land(key, entry);
        }
        let prewarm_done = self.prewarm.as_ref().is_some_and(|task| task.is_finished());
        if prewarm_done {
            self.prewarm = None;
        }
        lookup_done || (prewarm_done && index_waited)
    }

    /// The lookups no draw or signal has taken in yet, and the index build
    /// while a glyph waits for it (`index_waited`): what will change the text
    /// once it lands. The build alone changes nothing drawn.
    pub fn in_flight(&self, index_waited: bool) -> usize {
        let lookups = self.entries.values().filter(|entry| matches!(entry, Entry::Pending(_))).count();
        lookups + usize::from(index_waited && self.prewarm.is_some())
    }
}

/// Takes a finished lookup's answer into `entry`: true when it did.
fn land(key: &SystemFaceKey, entry: &mut Entry) -> bool {
    let Entry::Pending(task) = entry else { return false };
    let Some(result) = task.try_take() else { return false };
    let found = result.ok().flatten();
    match &found {
        Some(font) => log!("text: system font {:?} is {}#{}", key.name, font.path.display(), font.index),
        None => log!("text: system font {:?} is not installed here; its family draws without it", key.name),
    }
    *entry = Entry::Done(found);
    true
}

/// The `Cx`'s [`SystemFaces`], made the first time with its probe of kind
/// "font" ([`Cx::register_async_load_probe`]).
fn faces(cx: &mut Cx) -> &mut SystemFaces {
    if !cx.has_global::<SystemFaces>() {
        cx.register_async_load_probe("font", pending_font_loads);
    }
    cx.global::<SystemFaces>()
}

/// [`SystemFaces::in_flight`] on the `Cx`'s global (fonts borrowed elsewhere
/// count as waited for, as the signal hook takes them).
fn pending_font_loads(cx: &Cx) -> usize {
    let index_waited = cx
        .get_global_ref::<Rc<RefCell<Fonts>>>()
        .is_some_and(|fonts| fonts.try_borrow().map_or(true, |fonts| fonts.system_index_waited()));
    cx.get_global_ref::<SystemFaces>().map_or(0, |faces| faces.in_flight(index_waited))
}

/// [`SystemFaces::state`] on the `Cx`'s global.
pub(crate) fn lookup(cx: &mut Cx, key: &SystemFaceKey) -> SystemFaceState {
    let pool = cx.task_pool();
    faces(cx).state(&pool, key)
}

/// [`SystemFaces::start_prewarm`] on the `Cx`'s global.
pub(crate) fn start_prewarm(cx: &mut Cx) {
    if cx.global::<SystemFaces>().prewarm_started {
        return;
    }
    let pool = cx.task_pool();
    faces(cx).start_prewarm(&pool);
}

/// [`SystemFaces::has_news`] on the `Cx`'s global (false before any lookup);
/// `index_waited`: a glyph of this `Cx` waited for the installed-font index.
pub(crate) fn has_news(cx: &mut Cx, index_waited: bool) -> bool {
    cx.has_global::<SystemFaces>() && cx.get_global::<SystemFaces>().has_news(index_waited)
}
