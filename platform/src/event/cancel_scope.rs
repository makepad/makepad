use std::{cell::Cell, panic::Location, rc::Rc, sync::OnceLock};

use crate::{event::{Event, KeyCode}, log};

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

/// The gestures a cancel scope handles. Other gestures pass to the next eligible scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelScopeKind {
    Escape,
    Back,
    Both,
}

/// A widget's standing as the foreground owner of cancel gestures — the `Escape` key
/// and the back gesture — for as long as it is the active thing: an open modal, a
/// running dictation session, a drag in progress.
///
/// Scopes nest, and the most recently begun one that is still alive and accepts the
/// gesture is in front. [`Cx::begin_cancel_scope_for()`](crate::cx::Cx::begin_cancel_scope_for)
/// can restrict a scope to Escape or Back; the default scope accepts both. A
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
    kind: CancelScopeKind,
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
    /// Escape can remain held while an independent Back gesture is delivered.
    escape_owner: u64,
}

impl CxCancelScopes {
    /// `#[track_caller]` here and on `Cx::begin_cancel_scope` together make `caller()`
    /// the widget that began the scope, rather than either of these two frames.
    #[track_caller]
    pub(crate) fn begin(&mut self) -> CancelScope {
        self.begin_for(CancelScopeKind::Both)
    }

    #[track_caller]
    pub(crate) fn begin_for(&mut self, kind: CancelScopeKind) -> CancelScope {
        self.prune();
        // Ids start at 1, so `press_owner: 0` is an owner no scope can match.
        self.next_id += 1;
        let alive = Rc::new(Cell::new(true));
        let origin = Location::caller();
        self.stack.push(ScopeEntry { id: self.next_id, kind, alive: alive.clone(), origin });
        if tracing_enabled() {
            log!("[cancel] begin #{} at {}:{} ({:?}, in front of {} others)",
                self.next_id, origin.file(), origin.line(), kind, self.stack.len() - 1);
        }
        CancelScope { id: self.next_id, alive, origin }
    }

    pub(crate) fn end(&mut self, scope: CancelScope) {
        drop(scope); // its `Drop` marks it dead, and does the tracing
        self.prune();
    }

    /// Records ownership before dispatch. Follow-up actions keep the producing event's
    /// owner, but unrelated events must not inherit the last cancel gesture.
    pub(crate) fn handle_event(&mut self, event: &Event, intercepted: bool) {
        self.press_owner = 0;
        match event {
            Event::KeyDown(key) if key.key_code == KeyCode::Escape => {
                if intercepted {
                    // A platform overlay consumed this key. Its repeats and release
                    // must not dismiss the application underneath it.
                    self.escape_owner = 0;
                } else if !key.is_repeat {
                    self.begin_press(CancelScopeKind::Escape);
                    self.escape_owner = self.press_owner;
                }
                self.press_owner = self.escape_owner;
            }
            Event::KeyUp(key) if key.key_code == KeyCode::Escape => {
                if !intercepted {
                    self.press_owner = self.escape_owner;
                }
                self.escape_owner = 0;
            }
            Event::BackPressed { .. } if !intercepted => self.begin_press(CancelScopeKind::Back),
            // A release may be lost when the application stops receiving input.
            Event::WindowLostFocus(_) | Event::Pause | Event::Background => {
                self.escape_owner = 0;
            }
            _ => {}
        }
    }

    fn begin_press(&mut self, kind: CancelScopeKind) {
        self.prune();
        let owner = self.stack.iter().rev()
            .find(|e| e.kind == CancelScopeKind::Both || e.kind == kind);
        self.press_owner = owner.map_or(0, |e| e.id);
        if tracing_enabled() {
            match owner {
                Some(e) => log!("[cancel] {:?} press -> #{}, begun at {}:{}; exactly one widget should act on it",
                    kind, e.id, e.origin.file(), e.origin.line()),
                None => log!("[cancel] {:?} press -> nobody in front; nothing should act on it", kind),
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

    fn escape_down(is_repeat: bool) -> Event {
        Event::KeyDown(crate::event::KeyEvent {
            key_code: KeyCode::Escape,
            is_repeat,
            ..Default::default()
        })
    }

    fn escape_up() -> Event {
        Event::KeyUp(crate::event::KeyEvent {
            key_code: KeyCode::Escape,
            ..Default::default()
        })
    }

    #[test]
    fn escape_only_scopes_pass_back_to_the_view_behind_them() {
        let mut scopes = CxCancelScopes::default();
        let view = scopes.begin();
        let dictation = scopes.begin_for(CancelScopeKind::Escape);
        scopes.handle_event(&escape_down(false), false);
        assert!(scopes.owns_press(&dictation));
        assert!(!scopes.owns_press(&view));
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false);
        assert!(scopes.owns_press(&view));
        assert!(!scopes.owns_press(&dictation));
        scopes.handle_event(&escape_up(), false);
        assert!(scopes.owns_press(&dictation), "Back does not change Escape's owner");
        scopes.end(dictation);
        scopes.handle_event(&escape_down(false), false);
        assert!(scopes.owns_press(&view));
    }

    #[test]
    fn back_only_scopes_pass_escape_to_the_scope_behind_them() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let navigation = scopes.begin_for(CancelScopeKind::Back);
        scopes.handle_event(&escape_down(false), false);
        assert!(scopes.owns_press(&behind));
        assert!(!scopes.owns_press(&navigation));
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false);
        assert!(scopes.owns_press(&navigation));
        assert!(!scopes.owns_press(&behind));
    }

    #[test]
    fn a_scope_for_the_other_gesture_does_not_claim_an_unowned_press() {
        let mut scopes = CxCancelScopes::default();
        let escape = scopes.begin_for(CancelScopeKind::Escape);
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false);
        assert!(!scopes.owns_press(&escape));
        scopes.end(escape);
        let back = scopes.begin_for(CancelScopeKind::Back);
        scopes.handle_event(&escape_down(false), false);
        assert!(!scopes.owns_press(&back));
        scopes.handle_event(&escape_up(), false);
        assert!(!scopes.owns_press(&back));
    }

    #[test]
    fn back_does_not_reassign_a_held_escape_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.handle_event(&escape_down(false), false);
        scopes.end(in_front);
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false);
        assert!(scopes.owns_press(&behind));
        scopes.handle_event(&escape_down(true), false);
        assert!(!scopes.owns_press(&behind), "Back must not transfer the held Escape");
        scopes.handle_event(&escape_up(), false);
        assert!(!scopes.owns_press(&behind));
        scopes.handle_event(&escape_down(false), false);
        assert!(scopes.owns_press(&behind), "the next press is independent");
    }

    #[test]
    fn intercepted_escape_keeps_its_repeats_and_release_from_the_application() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), true);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(true), false);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(false), false);
        assert!(scopes.owns_press(&scope));
    }

    #[test]
    fn unrelated_events_do_not_own_cancel_and_escape_release_keeps_its_owner() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), false);
        scopes.handle_event(&Event::Signal, false);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false);
        assert!(scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false);
        assert!(!scopes.owns_press(&scope), "an unmatched release has no owner");
    }

    #[test]
    fn losing_input_focus_ends_the_held_escape_press() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), false);
        scopes.handle_event(&Event::Pause, false);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(true), false);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false);
        assert!(!scopes.owns_press(&scope));
    }

    #[test]
    fn the_most_recently_begun_scope_owns_a_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.begin_press(CancelScopeKind::Escape);
        assert!(scopes.owns_press(&in_front));
        assert!(!scopes.owns_press(&behind), "only the front may act on a press");
    }

    #[test]
    fn a_scope_begun_part_way_through_a_press_does_not_take_it() {
        // Ownership is settled when the key goes down, so a modal that opens while
        // Escape is held does not inherit that press.
        let mut scopes = CxCancelScopes::default();
        let owner = scopes.begin();
        scopes.begin_press(CancelScopeKind::Escape);
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
        scopes.begin_press(CancelScopeKind::Escape);
        scopes.end(in_front);
        assert!(!scopes.owns_press(&behind), "the press is spent, not inherited");
        // The next press belongs to whatever is in front by then.
        scopes.begin_press(CancelScopeKind::Escape);
        assert!(scopes.owns_press(&behind));
    }

    #[test]
    fn dropping_a_scope_gives_it_up_like_ending_it() {
        // A widget torn down without a tidy close must not wedge the key for everything
        // behind it.
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        drop(scopes.begin());
        scopes.begin_press(CancelScopeKind::Escape);
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
        scopes.begin_press(CancelScopeKind::Escape);
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
        scopes.begin_press(CancelScopeKind::Escape);
        assert!(!scopes.owns_press(&behind));
        // Asking again does not change the answer: a press has one owner for its life.
        assert!(!scopes.owns_press(&behind));
    }
}
