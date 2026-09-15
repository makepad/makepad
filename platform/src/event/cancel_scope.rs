use std::{cell::Cell, rc::Rc};

use crate::event::{Event, KeyCode};

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
}

impl std::fmt::Debug for CancelScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CancelScope({})", self.id)
    }
}

impl Drop for CancelScope {
    fn drop(&mut self) {
        self.alive.set(false);
    }
}

/// One live scope, as the stack holds it.
struct ScopeEntry {
    id: u64,
    owner: u64,
    kind: CancelScopeKind,
    alive: Rc<Cell<bool>>,
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
    pub(crate) fn begin(&mut self) -> CancelScope {
        self.begin_for(CancelScopeKind::Both)
    }

    pub(crate) fn begin_for(&mut self, kind: CancelScopeKind) -> CancelScope {
        self.begin_widget(0, kind)
    }

    pub(crate) fn begin_widget(&mut self, owner: u64, kind: CancelScopeKind) -> CancelScope {
        self.prune();
        // Ids start at 1, so `press_owner: 0` is an owner no scope can match.
        self.next_id += 1;
        let alive = Rc::new(Cell::new(true));
        self.stack.push(ScopeEntry { id: self.next_id, owner, kind, alive: alive.clone() });
        CancelScope { id: self.next_id, alive }
    }

    pub(crate) fn end(&mut self, scope: CancelScope) {
        drop(scope); // its `Drop` marks it dead
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
            // The mouse's back button is the same navigation gesture, so it is arbitrated the
            // same way. It arrives as one event, with no repeats or release to carry ownership.
            Event::MouseUp(e) if e.button.is_back() && !intercepted => {
                self.begin_press(CancelScopeKind::Back, widget_owner)
            }
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
            Event::MouseUp(e) if e.button.is_back() => CancelScopeKind::Back,
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
