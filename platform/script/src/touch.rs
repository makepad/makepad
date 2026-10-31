//! Splash touches: which top-level definitions of the `script_mod!` blocks a
//! run really used (local/agent_state/wasm/DESIGN.md "Splash shaking").
//!
//! A definition is used when, outside the registrations, something reads it:
//! a lookup that finds it or a field in it (`mod.widgets.Button`,
//! `button.draw_bg`), a walk over its entries (an apply onto a Rust type), or
//! a new object made with it as its prototype (`Button{..}`). Being held is
//! no use: Rust's registries hold every registered definition from the
//! registration on. Reads during a registration (between
//! `census_begin` and `census_end`) do not count either: a definition built
//! on another (`ButtonFlat = Button{..}`) names it in its text, which keeps
//! it whenever the one built on it is kept.
//!
//! The VM side is small: while [`TOUCH_RECORD`] is on, a new heap gets a
//! [`ScriptTouch`]; the census maps each registered block's definitions to
//! the ip of the opcode that made each one ([`ScriptHeap::touch_define`],
//! after every evaluation and `census_end` through [`TOUCH_MAP`]) and the first touch of an
//! object made at a mapped ip reports that definition to the census
//! ([`TOUCH_SINK`]) at once, from whatever VM and thread: a document's VM
//! that lives to the end of the run or dies early reports the same way.
//! A read inside a registration is an edge instead: the statement running
//! (the reading opcode's ip) needs what it read, so whenever that statement
//! is kept, so is what it read ([`ScriptHeap::touch_take_edges`]).
//! Off (every shipped program), a read costs one load of a None.
use crate::heap::ScriptHeap;
use crate::value::{ScriptIp, ScriptObject, ScriptValue};
use crate::vm::ScriptCode;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Heaps made from now on record touches (the census sets it).
pub static TOUCH_RECORD: AtomicBool = AtomicBool::new(false);

/// The census's mapping of the definitions of every block not mapped yet:
/// run at each registration's end.
pub static TOUCH_MAP: OnceLock<fn(&ScriptHeap, &ScriptCode)> = OnceLock::new();

/// The census's note that a definition (its slot, from `touch_define`) was used.
pub static TOUCH_SINK: OnceLock<fn(u32)> = OnceLock::new();

/// One heap's touch record.
#[derive(Default)]
pub struct ScriptTouch {
    /// open registrations: a read inside one is no use
    paused: Cell<u32>,
    /// made-at ip (`body << 32 | index`) of a definition's object -> the census's slots of the
    /// definitions it is (an alias is a second slot)
    defs: RefCell<HashMap<u64, Vec<u32>>>,
    /// made-at ips reported used
    reported: RefCell<HashSet<u64>>,
    /// by object index: the object is a definition not reported yet (the fast test of a read;
    /// a reused slot only costs a slow look)
    pending: RefCell<Vec<u64>>,
    /// reads inside registrations: (reading ip, made-at ip of what it read)
    edges: RefCell<HashSet<(u64, u64)>>,
    /// bodies mapped, by id and content hash
    mapped: RefCell<HashMap<u16, u64>>,
}

impl ScriptTouch {
    pub(crate) fn new_if_recording() -> Option<Box<ScriptTouch>> {
        TOUCH_RECORD.load(Ordering::Relaxed).then(Box::default)
    }

    /// Whether a read of the object at `index` may report something (outside a registration).
    #[inline(always)]
    fn may_report(&self, index: u32) -> bool {
        let pending = self.pending.borrow();
        pending.get(index as usize / 64).is_some_and(|w| w & (1 << (index % 64)) != 0)
    }

    fn set_pending(&self, index: u32, on: bool) {
        let mut pending = self.pending.borrow_mut();
        let w = index as usize / 64;
        if pending.len() <= w {
            pending.resize(w + 1, 0);
        }
        if on {
            pending[w] |= 1 << (index % 64);
        } else {
            pending[w] &= !(1 << (index % 64));
        }
    }
}

fn ip_key(ip: ScriptIp) -> u64 {
    ((ip.body as u64) << 32) | ip.index as u64
}

fn ip_of(key: u64) -> ScriptIp {
    ScriptIp { body: (key >> 32) as u16, index: key as u32 }
}

impl ScriptHeap {
    /// `obj` was read (a no-op unless recording).
    #[inline(always)]
    pub(crate) fn touch_obj(&self, obj: ScriptObject) {
        if let Some(t) = &self.touch {
            if t.paused.get() == 0 && t.may_report(obj.index) {
                self.touch_slow(t, obj);
            }
        }
    }

    /// `obj` and `value` were read by the opcode at `at` (an obj_get): inside a registration, an
    /// edge from that opcode; else a use.
    #[inline(always)]
    pub(crate) fn touch_read(&self, obj: ScriptObject, value: ScriptValue, at: Option<ScriptIp>) {
        if let Some(t) = &self.touch {
            if t.paused.get() == 0 {
                self.touch_obj(obj);
                if let Some(o) = value.as_object() {
                    self.touch_obj(o);
                }
            } else {
                self.touch_read_slow(t, obj, value, at);
            }
        }
    }

    #[inline(never)]
    fn touch_read_slow(&self, t: &ScriptTouch, obj: ScriptObject, value: ScriptValue, at: Option<ScriptIp>) {
        for o in [Some(obj), value.as_object()].into_iter().flatten() {
            let Some(at) = at.filter(|ip| !ip.is_unknown()) else { continue };
            if !self.objects.is_valid(o) {
                continue;
            }
            let made_at = self.objects[o].made_at;
            if !made_at.is_unknown() && made_at != at {
                t.edges.borrow_mut().insert((ip_key(at), ip_key(made_at)));
            }
        }
    }

    #[inline(never)]
    fn touch_slow(&self, t: &ScriptTouch, obj: ScriptObject) {
        if t.paused.get() > 0 || !self.objects.is_valid(obj) {
            return;
        }
        let made_at = self.objects[obj].made_at;
        if made_at.is_unknown() {
            return;
        }
        let key = ip_key(made_at);
        let defs = t.defs.borrow();
        let Some(slots) = defs.get(&key) else { return };
        t.set_pending(obj.index, false);
        if !t.reported.borrow_mut().insert(key) {
            return;
        }
        if let Some(sink) = TOUCH_SINK.get() {
            for &slot in slots {
                sink(slot);
            }
        }
    }

    /// Whether this heap records touches.
    #[inline]
    pub fn touch_recording(&self) -> bool {
        self.touch.is_some()
    }

    /// Record touches in this heap from now on (one made before [`TOUCH_RECORD`] was set).
    pub fn touch_start(&mut self) {
        if self.touch.is_none() {
            self.touch = Some(Box::default());
        }
    }

    /// The census: the definition `slot` is `obj`, made at `made_at`.
    /// True when this construction site was already read (including an alias
    /// mapped later). The caller merges that use without re-entering its sink.
    pub fn touch_define(&self, obj: ScriptObject, made_at: ScriptIp, slot: u32) -> bool {
        if let Some(t) = &self.touch {
            let key = ip_key(made_at);
            let mut defs = t.defs.borrow_mut();
            let slots = defs.entry(key).or_default();
            if !slots.contains(&slot) { slots.push(slot); }
            let reported = t.reported.borrow().contains(&key);
            if !reported {
                t.set_pending(obj.index, true);
            }
            return reported;
        }
        false
    }

    /// The census: the definition slots of the object made at `made_at`.
    pub fn touch_slots(&self, made_at: ScriptIp) -> Vec<u32> {
        self.touch.as_ref().and_then(|t| t.defs.borrow().get(&ip_key(made_at)).cloned()).unwrap_or_default()
    }

    /// The census: the registrations' reads since the last call, as (reading ip, made-at ip of
    /// what it read).
    pub fn touch_take_edges(&self) -> Vec<(ScriptIp, ScriptIp)> {
        let Some(t) = &self.touch else { return Vec::new() };
        std::mem::take(&mut *t.edges.borrow_mut()).into_iter().map(|(a, b)| (ip_of(a), ip_of(b))).collect()
    }

    /// The census: whether body `body` with text hash `hash` is mapped for the first time
    /// (false when it was mapped before, or the heap records nothing).
    pub fn touch_map_body(&self, body: u16, hash: u64) -> bool {
        self.touch.as_ref().is_some_and(|t| t.mapped.borrow_mut().insert(body, hash) != Some(hash))
    }

    /// Re-evaluation can replace a module's definitions with new objects even
    /// when its text is unchanged. Rearm those objects at the mapping boundary.
    pub(crate) fn touch_evaluated_body(&self, body: u16) {
        if let Some(t) = &self.touch {
            t.mapped.borrow_mut().remove(&body);
        }
    }

    /// A registration begins (reads inside it are no use).
    pub(crate) fn touch_pause(&self) {
        if let Some(t) = &self.touch {
            t.paused.set(t.paused.get() + 1);
        }
    }

    /// A registration ends: what it defined is mapped ([`TOUCH_MAP`]).
    pub(crate) fn touch_resume(&self, code: &ScriptCode) {
        if let Some(t) = &self.touch {
            t.paused.set(t.paused.get().saturating_sub(1));
            self.touch_map(code);
        }
    }

    /// A body ran (a registration's, or a text the app evaluates: a document, a kit): what it
    /// defined is mapped. Its reads of its own definitions while it ran came before, so they are
    /// no use (its text names them); its reads of what was mapped before are.
    #[inline(always)]
    pub(crate) fn touch_map(&self, code: &ScriptCode) {
        if self.touch.is_some() {
            if let Some(map) = TOUCH_MAP.get() {
                map(self, code);
            }
        }
    }
}
