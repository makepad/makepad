use std::{cell::Cell, panic::Location, rc::Rc, sync::OnceLock};

use crate::log;

/// Whether `MAKEPAD_CANCEL_TRACE` asks for cancel-gesture tracing.
///
/// A runtime switch rather than a `debug_assertions` build flag, because a wedged
/// `Escape` is usually met in a release build. Read once; free when unset.
fn tracing_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("MAKEPAD_CANCEL_TRACE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// A widget's standing as the foreground owner of cancel gestures — the `Escape` key
/// and the back gesture — for as long as it is the active thing: an open modal, a
/// running dictation session, a drag in progress.
///
/// Scopes nest, and the most recently begun one that is still alive is in front. A
/// press belongs to whichever scope was in front when the press began, and keeps
/// belonging to it for the key's repeats and its release. Drop the scope, or pass it
/// to [`Cx::end_cancel_scope()`](crate::cx::Cx::end_cancel_scope), to give it up;
/// dropping a widget that holds one gives it up too.
///
/// A widget asks [`Cx::owns_cancel()`](crate::cx::Cx::owns_cancel) whether the press
/// being delivered is its own, and acts only if it is. Nothing needs to consume the
/// key: only one scope can be in front, so exclusivity comes for free.
pub struct CancelScope {
    id: u64,
    alive: Rc<Cell<bool>>,
    /// Where [`Cx::begin_cancel_scope()`](crate::cx::Cx::begin_cancel_scope) was called,
    /// captured with `#[track_caller]` so a trace can name the widget holding this scope.
    origin: &'static Location<'static>,
}

impl std::fmt::Debug for CancelScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CancelScope(#{} from {}:{})", self.id, self.origin.file(), self.origin.line())
    }
}

impl Drop for CancelScope {
    fn drop(&mut self) {
        self.alive.set(false);
        // Logged here rather than in `end()` so that giving a scope up by dropping it —
        // including a widget being torn down — is reported exactly once, like ending it.
        if tracing_enabled() {
            log!("[cancel] end #{} (begun at {}:{})", self.id, self.origin.file(), self.origin.line());
        }
    }
}

/// One live scope, as the stack holds it.
struct ScopeEntry {
    id: u64,
    alive: Rc<Cell<bool>>,
    origin: &'static Location<'static>,
}

/// The stack of live cancel scopes, back to front.
#[derive(Default)]
pub(crate) struct CxCancelScopes {
    next_id: u64,
    stack: Vec<ScopeEntry>,
    /// The scope that was in front when the press being delivered began, or 0 for none.
    ///
    /// Deliberately kept even once that scope has gone: a scope that ends part-way
    /// through a press must not hand the rest of that press to whatever was behind it.
    press_owner: u64,
}

impl CxCancelScopes {
    /// `#[track_caller]` here and on `Cx::begin_cancel_scope` together make `caller()`
    /// the widget that began the scope, rather than either of these two frames.
    #[track_caller]
    pub(crate) fn begin(&mut self) -> CancelScope {
        self.prune();
        // Ids start at 1, so `press_owner: 0` is an owner no scope can match.
        self.next_id += 1;
        let alive = Rc::new(Cell::new(true));
        let origin = Location::caller();
        self.stack.push(ScopeEntry { id: self.next_id, alive: alive.clone(), origin });
        if tracing_enabled() {
            log!("[cancel] begin #{} at {}:{} ({} in front of {} others)",
                self.next_id, origin.file(), origin.line(), self.next_id, self.stack.len() - 1);
        }
        CancelScope { id: self.next_id, alive, origin }
    }

    pub(crate) fn end(&mut self, scope: CancelScope) {
        drop(scope); // its `Drop` marks it dead, and does the tracing
        self.prune();
    }

    /// Records who owns the press about to be delivered. The platform calls this once
    /// per cancel gesture, before dispatching it, and not for a key's repeats.
    pub(crate) fn begin_press(&mut self) {
        self.prune();
        self.press_owner = self.stack.last().map_or(0, |e| e.id);
        if tracing_enabled() {
            match self.stack.last() {
                Some(e) => log!("[cancel] press -> #{}, begun at {}:{}; exactly one widget should act on it",
                    e.id, e.origin.file(), e.origin.line()),
                None => log!("[cancel] press -> nobody in front; nothing should act on it"),
            }
        }
    }

    pub(crate) fn owns_press(&self, scope: &CancelScope) -> bool {
        scope.id == self.press_owner
    }

    fn prune(&mut self) {
        self.stack.retain(|e| e.alive.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_recently_begun_scope_owns_a_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.begin_press();
        assert!(scopes.owns_press(&in_front));
        assert!(!scopes.owns_press(&behind), "only the front may act on a press");
    }

    #[test]
    fn a_scope_begun_part_way_through_a_press_does_not_take_it() {
        // Ownership is settled when the key goes down, so a modal that opens while
        // Escape is held does not inherit that press.
        let mut scopes = CxCancelScopes::default();
        let owner = scopes.begin();
        scopes.begin_press();
        let latecomer = scopes.begin();
        assert!(scopes.owns_press(&owner));
        assert!(!scopes.owns_press(&latecomer));
    }

    #[test]
    fn a_scope_that_ends_part_way_keeps_the_press_from_the_one_behind() {
        // Dictation stops on the key-down and gives up its scope; the modal behind it
        // must not then close on the key-up of that same press.
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.begin_press();
        scopes.end(in_front);
        assert!(!scopes.owns_press(&behind), "the press is spent, not inherited");
        // The next press belongs to whatever is in front by then.
        scopes.begin_press();
        assert!(scopes.owns_press(&behind));
    }

    #[test]
    fn dropping_a_scope_gives_it_up_like_ending_it() {
        // A widget torn down without a tidy close must not wedge the key for everything
        // behind it.
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        drop(scopes.begin());
        scopes.begin_press();
        assert!(scopes.owns_press(&behind));
    }

    #[test]
    fn with_nothing_in_front_no_one_owns_the_press() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.end(scope);
        let later = scopes.begin();
        scopes.end(later);
        let fresh = scopes.begin();
        scopes.end(fresh);
        scopes.begin_press();
        let probe = scopes.begin();
        assert!(!scopes.owns_press(&probe), "an empty stack leaves the press unowned");
    }

    #[test]
    fn a_scope_remembers_where_it_was_begun() {
        // What lets a trace name the widget that leaked a scope, rather than only
        // reporting that one did. The location is this call site, not `begin`'s.
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        assert!(
            format!("{scope:?}").contains("cancel_scope.rs"),
            "expected the caller's location, got {scope:?}"
        );
    }

    #[test]
    fn a_scope_that_was_never_in_front_never_owns_a_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let _in_front = scopes.begin();
        scopes.begin_press();
        assert!(!scopes.owns_press(&behind));
        // Asking again does not change the answer: a press has one owner for its life.
        assert!(!scopes.owns_press(&behind));
    }
}
