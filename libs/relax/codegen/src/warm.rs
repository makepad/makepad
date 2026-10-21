//! Opt-in accounting for compiler work on a fully validated warm launch.
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    sync::OnceLock,
};
use relax_session::sync::Mutex;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_WARM_STRICT").as_deref() == Ok("1"))
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum Id {
    Slot(u32),
    Body(u32, Vec<u32>),
    Name(String),
}
#[derive(Clone)]
struct Event {
    id: Id,
    name: String,
    phase: &'static str,
    cpu: u64,
}
#[derive(Default)]
struct State {
    valid: Option<bool>,
    presented: bool,
    names: HashMap<u32, String>,
    bodies: HashMap<(u32, Vec<u32>), u32>,
    reasons: HashMap<u32, String>,
    events: Vec<Event>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
relax_session::gen_local! {
    static SLOTS: RefCell<Vec<(u32, u8)>>;
    static CHILD_CPU: RefCell<Vec<u64>>;
}
pub struct Context(bool);
impl Context {
    pub fn new(slot: u32, tier: u8) -> Self {
        let on = enabled()
            && SLOTS
                .try_with(|s| s.borrow_mut().push((slot, tier)))
                .is_ok();
        Self(on)
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if self.0 {
            let _ = SLOTS.try_with(|s| s.borrow_mut().pop());
        }
    }
}
pub fn name(slot: u32, name: &str) {
    if enabled() {
        state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .names
            .insert(slot, name.to_owned());
    }
}
pub fn instance(slot: u32, def: u32, args: &[u32]) {
    if enabled() {
        state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bodies
            .insert((def, args.to_vec()), slot);
    }
}
pub fn reason(slot: u32, reason: &str) {
    if enabled() {
        state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reasons
            .insert(slot, reason.to_owned());
    }
}
/// Classification happens after all stores and late-input ledgers were validated.
/// Work before that point is retained, so early type checking cannot escape the check.
pub fn classify(valid: bool) {
    if !enabled() {
        return;
    }
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    s.valid = Some(valid);
    if !valid {
        s.events.clear();
    }
}

const BACKGROUND_CHECK: &str = "background borrow check";

pub struct Scope(Option<(usize, Event, u64)>);
impl Scope {
    fn new(id: Id, name: String, phase: &'static str) -> Self {
        if state().lock().unwrap_or_else(|e| e.into_inner()).valid == Some(false) {
            return Self(None);
        }
        if CHILD_CPU.try_with(|s| s.borrow_mut().push(0)).is_err() {
            return Self(None);
        }
        let event = Event {
            id,
            name,
            phase,
            cpu: 0,
        };
        let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
        let index = s.events.len();
        if !s.presented {
            s.events.push(event.clone());
        }
        Self(Some((index, event, relax_backend::cpu_ns())))
    }
    pub fn slot(slot: u32, phase: &'static str) -> Self {
        if !enabled() {
            return Self(None);
        }
        Self::new(Id::Slot(slot), format!("slot {}", slot), phase)
    }
    /// `background`: the type check is the background borrow checker's (Gandalf), not a
    /// build's: a body without a stored verdict, never a code-cache miss.
    pub fn body(def: u32, args: &[u32], background: bool, name: impl FnOnce() -> String) -> Self {
        if !enabled() {
            return Self(None);
        }
        Self::new(Id::Body(def, args.to_vec()), name(), if background { BACKGROUND_CHECK } else { "body typeck" })
    }
    pub fn optimization() -> Self {
        if !enabled() {
            return Self(None);
        }
        let slot = SLOTS
            .try_with(|s| s.borrow().last().map(|x| x.0))
            .ok()
            .flatten();
        match slot {
            Some(slot) => Self::slot(slot, "optimization"),
            None => Self(None),
        }
    }
    pub fn backend(name: &str) -> Self {
        if !enabled() {
            return Self(None);
        }
        let id = SLOTS
            .try_with(|s| s.borrow().last().map(|x| Id::Slot(x.0)))
            .ok()
            .flatten()
            .unwrap_or_else(|| Id::Name(name.to_owned()));
        Self::new(id, name.to_owned(), "emission")
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let Some((index, mut event, start)) = self.0.take() else {
            return;
        };
        let elapsed = relax_backend::cpu_ns().saturating_sub(start);
        let child = CHILD_CPU
            .try_with(|s| {
                let mut s = s.borrow_mut();
                let child = s.pop().unwrap_or(0);
                if let Some(parent) = s.last_mut() {
                    *parent += elapsed;
                }
                child
            })
            .unwrap_or(0);
        event.cpu = elapsed.saturating_sub(child);
        let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
        if s.valid == Some(false) {
            return;
        }
        if s.presented && s.valid == Some(true) {
            let (_, name, reason) = describe(&s, &event);
            eprintln!("makepad: warm miss: {} ({}; {})", name, reason, event.phase);
        } else if let Some(pending) = s.events.get_mut(index) {
            pending.cpu = event.cpu;
        }
    }
}
fn describe(s: &State, e: &Event) -> (Id, String, String) {
    if e.phase == BACKGROUND_CHECK {
        return (e.id.clone(), e.name.clone(), "no stored borrow-check verdict for this body".to_owned());
    }
    let slot = match &e.id {
        Id::Slot(id) => Some(*id),
        Id::Body(d, args) => s.bodies.get(&(*d, args.clone())).copied(),
        Id::Name(_) => None,
    };
    let name = slot
        .and_then(|id| s.names.get(&id))
        .cloned()
        .unwrap_or_else(|| e.name.clone());
    let reason = slot
        .and_then(|id| s.reasons.get(&id))
        .cloned()
        .unwrap_or_else(|| {
            "no standalone cold lookup/record observed; precise cause unresolved".to_owned()
        });
    (
        slot.map(Id::Slot).unwrap_or_else(|| e.id.clone()),
        name,
        reason,
    )
}
pub fn first_present() {
    report(true);
}
/// Non-windowed programs still report each miss when their run ends.
pub fn finish() {
    report(false);
}
fn report(present: bool) {
    if !enabled() {
        return;
    }
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    if s.presented {
        return;
    }
    s.presented = true;
    if s.valid != Some(true) {
        s.events.clear();
        return;
    }
    let mut rows: HashMap<Id, (String, String, BTreeSet<&'static str>, u64)> = HashMap::new();
    for e in &s.events {
        let (id, name, reason) = describe(&s, e);
        let row = rows
            .entry(id)
            .or_insert_with(|| (name, reason, BTreeSet::new(), 0));
        row.2.insert(e.phase);
        row.3 += e.cpu;
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, reason, phases, _) in &rows {
        eprintln!(
            "makepad: warm miss: {} ({}; {})",
            name,
            reason,
            phases.iter().copied().collect::<Vec<_>>().join(", ")
        );
    }
    if present {
        eprintln!(
            "makepad: warm misses {}, CPU {:.3} ms",
            rows.len(),
            rows.iter().map(|r| r.3).sum::<u64>() as f64 / 1e6
        );
    }
    s.events.clear();
}
