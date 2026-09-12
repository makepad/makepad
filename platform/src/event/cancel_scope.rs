use std::{cell::Cell, rc::Rc};

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

/// The stack of live cancel scopes, back to front.
#[derive(Default)]
pub(crate) struct CxCancelScopes {
    next_id: u64,
    stack: Vec<(u64, Rc<Cell<bool>>)>,
    /// The scope that was in front when the press being delivered began, or 0 for none.
    ///
    /// Deliberately kept even once that scope has gone: a scope that ends part-way
    /// through a press must not hand the rest of that press to whatever was behind it.
    press_owner: u64,
}

impl CxCancelScopes {
    pub(crate) fn begin(&mut self) -> CancelScope {
        self.prune();
        // Ids start at 1, so `press_owner: 0` is an owner no scope can match.
        self.next_id += 1;
        let alive = Rc::new(Cell::new(true));
        self.stack.push((self.next_id, alive.clone()));
        CancelScope { id: self.next_id, alive }
    }

    pub(crate) fn end(&mut self, scope: CancelScope) {
        drop(scope); // its `Drop` marks it dead
        self.prune();
    }

    /// Records who owns the press about to be delivered. The platform calls this once
    /// per cancel gesture, before dispatching it, and not for a key's repeats.
    pub(crate) fn begin_press(&mut self) {
        self.prune();
        self.press_owner = self.stack.last().map_or(0, |(id, _)| *id);
    }

    pub(crate) fn owns_press(&self, scope: &CancelScope) -> bool {
        scope.id == self.press_owner
    }

    fn prune(&mut self) {
        self.stack.retain(|(_, alive)| alive.get());
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
