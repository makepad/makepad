//! What a body check leaves for the crate-wide passes, retained by owner before it is merged.

use crate::borrowck::{event::BorrowError, field_views::{FvRecordRaw, RelaxedRecord}};
use crate::fxhash::{FxBuild, HashMap};
use crate::lint::{AttrKey, Reported};
use crate::lint_dead::BodyInfo;
use crate::program::{capture_import_uses, DefId, DefKind, ImportUses, Program};
use relax_session::sync::Mutex;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The body phase's window: item checks before it and crate passes after it keep their sinks.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Facts whose owner scopes have ended, ready for the crate-wide passes after the body phase.
static FACTS: Mutex<HashMap<DefId, BodyFacts>> = Mutex::new(HashMap::with_hasher(FxBuild::new()));
/// Sink writes inside the body phase without an owner: the check tests guard against escapes.
static ESCAPED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The active owner on this thread, so nested work for the same body shares its buffer.
    static OWNER: Cell<Option<DefId>> = const { Cell::new(None) };
    /// This owner's side effects, kept off the shared sinks until its scope ends.
    static BUFFER: RefCell<BodyFacts> = RefCell::new(BodyFacts::default());
}

/// One owner's facts in recording order, including its closures and anonymous constants.
#[derive(Default)]
pub struct BodyFacts {
    /// Lint reports with their original sequence numbers, for the check's diagnostic ordering.
    pub reported: Vec<Reported>,
    /// Expectations met by this body, for the later unfulfilled-expectations pass.
    pub fulfilled: Vec<AttrKey>,
    /// References and live-body notes, with the holder used by the dead-code pass.
    pub(crate) refs: Vec<(DefId, BodyInfo)>,
    /// Traits whose methods were used, attributed to the crate whose imports they fulfil.
    pub method_traits: Vec<(u32, DefId)>,
    /// Resolutions through imports, retained without making the front end depend on types.
    pub import_uses: ImportUses,
    /// Conflicts accepted only in relaxed Rust, for the end-of-check report.
    pub fv_only: Vec<BorrowError>,
    /// Field-view rewrites, for the optional strict-source export.
    pub fv_records: Vec<FvRecordRaw>,
    /// Other relaxed acceptances, for strict reports and fixes.
    pub relaxed_records: Vec<RelaxedRecord>,
}

/// Combining scopes preserves each owner's recording order, including work from retries.
impl BodyFacts {
    /// Adds a completed scope without reordering the facts already held for this owner.
    fn append(&mut self, mut other: BodyFacts) {
        self.reported.append(&mut other.reported);
        self.fulfilled.append(&mut other.fulfilled);
        self.refs.append(&mut other.refs);
        self.method_traits.append(&mut other.method_traits);
        self.import_uses.append(other.import_uses);
        self.fv_only.append(&mut other.fv_only);
        self.fv_records.append(&mut other.fv_records);
        self.relaxed_records.append(&mut other.relaxed_records);
    }
}

/// Starts the body phase; the escape count belongs to this window alone.
pub fn open() { ESCAPED.store(0, Ordering::Relaxed); ACTIVE.store(true, Ordering::Release); }

/// Ends body capture before the crate-wide passes read the merged sinks.
pub fn close() { ACTIVE.store(false, Ordering::Release); }

/// The number of unowned sink writes in this body phase, for the check's test-only guard.
pub fn escaped() -> usize { ESCAPED.load(Ordering::Relaxed) }

/// Child bodies belong to their item; `root_owner` starts at the parent, so items keep themselves.
fn fact_owner(prog: &Program, d: DefId) -> DefId {
    if matches!(prog.def(d).kind, DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy) { crate::borrowck::body::root_owner(prog, d) } else { d }
}

/// Restores a nested owner and publishes this scope's facts even when a body deliberately unwinds.
struct Restore {
    /// The owner whose completed buffer is appended to the store.
    key: DefId,
    /// The enclosing scope's owner, or none when this thread entered its first body.
    owner: Option<DefId>,
    /// The enclosing scope's facts, suspended while a different owner is checked.
    buffer: BodyFacts,
}

/// Scope exit restores the thread before publishing, on both normal return and unwind.
impl Drop for Restore {
    /// Keeps partial facts from a failed attempt just as the former global writes did.
    fn drop(&mut self) {
        let got = BUFFER.with(|b| b.replace(std::mem::take(&mut self.buffer)));
        OWNER.with(|o| o.set(self.owner));
        let _k = crate::zone_alloc::kept();
        FACTS.lock().unwrap_or_else(|e| e.into_inner()).entry(self.key).or_default().append(got);
    }
}

/// Runs a body under its fact owner while the window is open; same-owner queries share its scope.
pub fn with_owner<R>(prog: &Program, d: DefId, f: impl FnOnce() -> R) -> R {
    if !ACTIVE.load(Ordering::Acquire) { return f(); }
    let key = fact_owner(prog, d);
    if OWNER.with(|o| o.get() == Some(key)) { return f(); }
    let _restore = Restore { key, owner: OWNER.with(|o| o.replace(Some(key))), buffer: BUFFER.with(|b| std::mem::take(&mut *b.borrow_mut())) };
    // Catch inside the front-end capture so it returns import uses even on unwind; then pass
    // the same panic on to def_work after those uses join this owner's other partial facts.
    let (r, uses) = capture_import_uses(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    {
        let _k = crate::zone_alloc::kept();
        BUFFER.with(|b| b.borrow_mut().import_uses.append(uses));
    }
    match r { Ok(r) => r, Err(p) => std::panic::resume_unwind(p) }
}

/// Records on the current owner, or counts a body-phase escape and leaves the caller its old sink.
pub fn record(f: impl FnOnce(&mut BodyFacts)) -> bool {
    if OWNER.with(|o| o.get().is_some()) {
        let _k = crate::zone_alloc::kept();
        BUFFER.with(|b| f(&mut b.borrow_mut()));
        true
    } else {
        if ACTIVE.load(Ordering::Acquire) { ESCAPED.fetch_add(1, Ordering::Relaxed); }
        false
    }
}

/// Publishes the completed body phase in owner order before any crate-wide sink reader runs.
pub fn merge(prog: &Program) {
    let got = std::mem::take(&mut *FACTS.lock().unwrap_or_else(|e| e.into_inner()));
    let mut owners: Vec<_> = got.into_iter().collect();
    owners.sort_unstable_by_key(|(d, _)| *d);
    for (_, f) in owners {
        crate::lint::merge_reported(f.reported);
        crate::lint::merge_fulfilled(f.fulfilled);
        for (holder, info) in f.refs { crate::lint_dead::merge_refs(holder, info); }
        crate::lint_imports::merge_method_traits(f.method_traits);
        prog.add_import_uses(&f.import_uses);
        crate::borrowck::merge_verdict_notes(f.fv_only, f.fv_records, f.relaxed_records);
    }
}

/// What this module holds of a run, as a fresh process has it; no run thread may still use it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&ACTIVE, AtomicBool::new(false));
    reset(&FACTS, Mutex::new(HashMap::default()));
    reset(&ESCAPED, AtomicUsize::new(0));
}
