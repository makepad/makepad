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
/// Widget scopes follow the current widget hierarchy: hidden branches are ignored,
/// descendants take priority over ancestors, and the newest scope wins across
/// unrelated branches. [`Cx::begin_widget_cancel_scope()`](crate::cx::Cx::begin_widget_cancel_scope)
/// binds a scope to its widget and can restrict it to Escape or Back. A
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
    owner: u64,
    kind: CancelScopeKind,
    alive: Rc<Cell<bool>>,
    origin: &'static Location<'static>,
}

/// Cancel scopes in activation order; widget ancestry determines their priority.
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
        self.begin_widget(0, kind)
    }

    #[track_caller]
    pub(crate) fn begin_widget(&mut self, owner: u64, kind: CancelScopeKind) -> CancelScope {
        self.prune();
        // Ids start at 1, so `press_owner: 0` is an owner no scope can match.
        self.next_id += 1;
        let alive = Rc::new(Cell::new(true));
        let origin = Location::caller();
        self.stack.push(ScopeEntry { id: self.next_id, owner, kind, alive: alive.clone(), origin });
        if tracing_enabled() {
            log!("[cancel] begin #{} at {}:{} ({:?}, widget {}, {} other live scopes)",
                self.next_id, origin.file(), origin.line(), kind, owner, self.stack.len() - 1);
        }
        CancelScope { id: self.next_id, alive, origin }
    }

    pub(crate) fn end(&mut self, scope: CancelScope) {
        drop(scope); // its `Drop` marks it dead, and does the tracing
        self.prune();
    }

    /// Records ownership before dispatch. Follow-up actions keep the producing event's
    /// owner, but unrelated events must not inherit the last cancel gesture.
    pub(crate) fn handle_event(&mut self, event: &Event, intercepted: bool, widget_owner: Option<u64>) {
        self.press_owner = 0;
        match event {
            Event::KeyDown(key) if key.key_code == KeyCode::Escape => {
                if intercepted {
                    // A platform overlay consumed this key. Its repeats and release
                    // must not dismiss the application underneath it.
                    self.escape_owner = 0;
                } else if !key.is_repeat {
                    self.begin_press(CancelScopeKind::Escape, widget_owner);
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
            Event::BackPressed { .. } if !intercepted => self.begin_press(CancelScopeKind::Back, widget_owner),
            // A release may be lost when the application stops receiving input.
            Event::WindowLostFocus(_) | Event::Pause | Event::Background => {
                self.escape_owner = 0;
            }
            _ => {}
        }
    }

    /// Resolve current widget visibility only at the start of an independent press.
    /// The resolver receives a lookup rather than a copied list of live scopes.
    pub(crate) fn resolve_widget_owner(
        &self,
        event: &Event,
        intercepted: bool,
        resolve: impl FnOnce(&dyn Fn(u64) -> Option<u64>) -> Option<u64>,
    ) -> Option<u64> {
        if intercepted {
            return None;
        }
        let kind = match event {
            Event::KeyDown(key) if key.key_code == KeyCode::Escape && !key.is_repeat => CancelScopeKind::Escape,
            Event::BackPressed { .. } => CancelScopeKind::Back,
            _ => return None,
        };
        if !self.stack.iter().any(|e| e.owner != 0 && e.alive.get()
            && (e.kind == CancelScopeKind::Both || e.kind == kind))
        {
            return None;
        }
        resolve(&|owner| {
            self.stack.iter().rev().find(|e| owner != 0 && e.owner == owner && e.alive.get()
                && (e.kind == CancelScopeKind::Both || e.kind == kind))
                .map(|e| e.id)
        })
    }

    fn begin_press(&mut self, kind: CancelScopeKind, widget_owner: Option<u64>) {
        self.prune();
        let owner = self.stack.iter().rev()
            .find(|e| (e.owner == 0 || Some(e.id) == widget_owner)
                && (e.kind == CancelScopeKind::Both || e.kind == kind));
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

    pub(crate) fn has_press_owner(&self) -> bool {
        self.press_owner != 0
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
    fn widget_lookup_filters_gestures_dead_scopes_and_global_scopes() {
        let mut scopes = CxCancelScopes::default();
        let both = scopes.begin_widget(10, CancelScopeKind::Both);
        let escape = scopes.begin_widget(10, CancelScopeKind::Escape);
        let back = scopes.begin_widget(10, CancelScopeKind::Back);
        drop(scopes.begin_widget(10, CancelScopeKind::Both));
        let _global = scopes.begin();
        let owner = scopes.resolve_widget_owner(&escape_down(false), false, |lookup| {
            assert_eq!(lookup(0), None);
            assert_eq!(lookup(99), None);
            lookup(10)
        });
        assert_eq!(owner, Some(escape.id));
        let owner = scopes.resolve_widget_owner(&Event::BackPressed { handled: Cell::new(false) }, false,
            |lookup| lookup(10));
        assert_eq!(owner, Some(back.id));
        drop(escape);
        let owner = scopes.resolve_widget_owner(&escape_down(false), false, |lookup| lookup(10));
        assert_eq!(owner, Some(both.id));
    }

    #[test]
    fn widget_resolution_only_runs_for_new_matching_presses() {
        let mut scopes = CxCancelScopes::default();
        let unexpected = |_: &dyn Fn(u64) -> Option<u64>| -> Option<u64> {
            panic!("this event must not traverse the widget hierarchy")
        };
        scopes.resolve_widget_owner(&escape_down(false), false, unexpected);
        let _global = scopes.begin();
        scopes.resolve_widget_owner(&escape_down(false), false, unexpected);
        let bound = scopes.begin_widget(10, CancelScopeKind::Escape);
        scopes.resolve_widget_owner(&Event::BackPressed { handled: Cell::new(false) }, false, unexpected);
        scopes.resolve_widget_owner(&escape_down(false), true, unexpected);
        scopes.resolve_widget_owner(&escape_down(true), false, unexpected);
        scopes.resolve_widget_owner(&escape_up(), false, unexpected);
        scopes.resolve_widget_owner(&Event::Signal, false, unexpected);
        scopes.resolve_widget_owner(&Event::Draw(Default::default()), false, unexpected);
        drop(bound);
        scopes.resolve_widget_owner(&escape_down(false), false, unexpected);
    }

    #[test]
    fn hidden_widget_scopes_do_not_block_global_scopes_or_unowned_presses() {
        let mut scopes = CxCancelScopes::default();
        let global = scopes.begin();
        let _hidden = scopes.begin_widget(10, CancelScopeKind::Both);
        let owner = scopes.resolve_widget_owner(&escape_down(false), false, |_| None);
        scopes.handle_event(&escape_down(false), false, owner);
        assert!(scopes.owns_press(&global));
        drop(global);
        scopes.handle_event(&escape_down(false), false, None);
        assert!(!scopes.has_press_owner(), "a missing resolver cannot authorize a bound scope");
    }

    #[test]
    fn global_and_resolved_widget_scopes_keep_activation_order() {
        let mut scopes = CxCancelScopes::default();
        let global = scopes.begin();
        let widget = scopes.begin_widget(10, CancelScopeKind::Both);
        scopes.begin_press(CancelScopeKind::Escape, Some(widget.id));
        assert!(scopes.owns_press(&widget));
        let newer_global = scopes.begin();
        scopes.begin_press(CancelScopeKind::Escape, Some(widget.id));
        assert!(scopes.owns_press(&newer_global));
        drop(newer_global);
        scopes.begin_press(CancelScopeKind::Escape, Some(u64::MAX));
        assert!(scopes.owns_press(&global), "only a real matching scope can be resolved");
    }

    #[test]
    fn cx_resolves_widget_owners_before_dispatch_and_keeps_press_snapshots() {
        let mut cx = crate::cx::Cx::new(Box::new(|cx, event| {
            if matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::BackPressed { .. }) {
                assert!(cx.has_cancel_owner(), "ownership must exist before the app receives input");
            }
        }));
        cx.cancel_scope_resolver = Some(|cx, lookup| lookup(cx.keyboard_shift as u64));
        cx.keyboard_shift = 10.0;
        let first = cx.begin_widget_cancel_scope(10, CancelScopeKind::Both);
        let second = cx.begin_widget_cancel_scope(20, CancelScopeKind::Both);
        cx.call_event_handler(&escape_down(false));
        assert!(cx.owns_cancel(&first));
        cx.keyboard_shift = 20.0;
        drop(first);
        cx.call_event_handler(&Event::BackPressed { handled: Cell::new(false) });
        assert!(cx.owns_cancel(&second), "an independent Back resolves current eligibility");
        cx.call_event_handler(&escape_down(true));
        assert!(!cx.owns_cancel(&second), "a held Escape does not change owner with visibility");
        assert!(cx.has_cancel_owner(), "dropping the owner leaves this press spent");
        cx.call_event_handler(&escape_up());
        assert!(!cx.owns_cancel(&second));
        assert!(cx.has_cancel_owner());
        cx.call_event_handler(&Event::Signal);
        assert!(!cx.has_cancel_owner(), "unrelated events cannot inherit cancellation");
        cx.call_event_handler(&escape_down(false));
        assert!(cx.owns_cancel(&second));
    }

    #[test]
    fn escape_only_scopes_pass_back_to_the_view_behind_them() {
        let mut scopes = CxCancelScopes::default();
        let view = scopes.begin();
        let dictation = scopes.begin_for(CancelScopeKind::Escape);
        scopes.handle_event(&escape_down(false), false, None);
        assert!(scopes.owns_press(&dictation));
        assert!(!scopes.owns_press(&view));
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false, None);
        assert!(scopes.owns_press(&view));
        assert!(!scopes.owns_press(&dictation));
        scopes.handle_event(&escape_up(), false, None);
        assert!(scopes.owns_press(&dictation), "Back does not change Escape's owner");
        scopes.end(dictation);
        scopes.handle_event(&escape_down(false), false, None);
        assert!(scopes.owns_press(&view));
    }

    #[test]
    fn back_only_scopes_pass_escape_to_the_scope_behind_them() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let navigation = scopes.begin_for(CancelScopeKind::Back);
        scopes.handle_event(&escape_down(false), false, None);
        assert!(scopes.owns_press(&behind));
        assert!(!scopes.owns_press(&navigation));
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false, None);
        assert!(scopes.owns_press(&navigation));
        assert!(!scopes.owns_press(&behind));
    }

    #[test]
    fn a_scope_for_the_other_gesture_does_not_claim_an_unowned_press() {
        let mut scopes = CxCancelScopes::default();
        let escape = scopes.begin_for(CancelScopeKind::Escape);
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false, None);
        assert!(!scopes.owns_press(&escape));
        scopes.end(escape);
        let back = scopes.begin_for(CancelScopeKind::Back);
        scopes.handle_event(&escape_down(false), false, None);
        assert!(!scopes.owns_press(&back));
        scopes.handle_event(&escape_up(), false, None);
        assert!(!scopes.owns_press(&back));
    }

    #[test]
    fn back_does_not_reassign_a_held_escape_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.handle_event(&escape_down(false), false, None);
        scopes.end(in_front);
        scopes.handle_event(&Event::BackPressed { handled: Cell::new(false) }, false, None);
        assert!(scopes.owns_press(&behind));
        scopes.handle_event(&escape_down(true), false, None);
        assert!(!scopes.owns_press(&behind), "Back must not transfer the held Escape");
        scopes.handle_event(&escape_up(), false, None);
        assert!(!scopes.owns_press(&behind));
        scopes.handle_event(&escape_down(false), false, None);
        assert!(scopes.owns_press(&behind), "the next press is independent");
    }

    #[test]
    fn intercepted_escape_keeps_its_repeats_and_release_from_the_application() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), true, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(true), false, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(false), false, None);
        assert!(scopes.owns_press(&scope));
    }

    #[test]
    fn unrelated_events_do_not_own_cancel_and_escape_release_keeps_its_owner() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), false, None);
        scopes.handle_event(&Event::Signal, false, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false, None);
        assert!(scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false, None);
        assert!(!scopes.owns_press(&scope), "an unmatched release has no owner");
    }

    #[test]
    fn losing_input_focus_ends_the_held_escape_press() {
        let mut scopes = CxCancelScopes::default();
        let scope = scopes.begin();
        scopes.handle_event(&escape_down(false), false, None);
        scopes.handle_event(&Event::Pause, false, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_down(true), false, None);
        assert!(!scopes.owns_press(&scope));
        scopes.handle_event(&escape_up(), false, None);
        assert!(!scopes.owns_press(&scope));
    }

    #[test]
    fn the_most_recently_begun_scope_owns_a_press() {
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        let in_front = scopes.begin();
        scopes.begin_press(CancelScopeKind::Escape, None);
        assert!(scopes.owns_press(&in_front));
        assert!(!scopes.owns_press(&behind), "only the front may act on a press");
    }

    #[test]
    fn a_scope_begun_part_way_through_a_press_does_not_take_it() {
        // Ownership is settled when the key goes down, so a modal that opens while
        // Escape is held does not inherit that press.
        let mut scopes = CxCancelScopes::default();
        let owner = scopes.begin();
        scopes.begin_press(CancelScopeKind::Escape, None);
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
        scopes.begin_press(CancelScopeKind::Escape, None);
        scopes.end(in_front);
        assert!(!scopes.owns_press(&behind), "the press is spent, not inherited");
        // The next press belongs to whatever is in front by then.
        scopes.begin_press(CancelScopeKind::Escape, None);
        assert!(scopes.owns_press(&behind));
    }

    #[test]
    fn dropping_a_scope_gives_it_up_like_ending_it() {
        // A widget torn down without a tidy close must not wedge the key for everything
        // behind it.
        let mut scopes = CxCancelScopes::default();
        let behind = scopes.begin();
        drop(scopes.begin());
        scopes.begin_press(CancelScopeKind::Escape, None);
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
        scopes.begin_press(CancelScopeKind::Escape, None);
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
        scopes.begin_press(CancelScopeKind::Escape, None);
        assert!(!scopes.owns_press(&behind));
        // Asking again does not change the answer: a press has one owner for its life.
        assert!(!scopes.owns_press(&behind));
    }
}
