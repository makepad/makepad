// Not Copy, named fields, the app's own crate: a field edit applies live (M10) and keeps the
// running layout (no hold, no invalidation).
pub struct State {
    pub a: u64,
}

impl State {
    pub fn new() -> State {
        State { a: 1 }
    }
}

/// The layout edits add a variant to it (M10: variants added at the end apply in place): code built
/// before such an edit can meet a variant it does not know, so it never runs again.
pub enum Mode {
    A,
}

#[inline(never)]
pub fn mode(_n: u64) -> Mode {
    Mode::A
}

/// The worker's state, made here: the worker's loop holds only a reference (no State on its frame,
/// no drop glue of it), so a layout patch does not rebuild the loop and holds the worker only where
/// the test puts it (a loop with State on its frame is rebuilt: its first version is held).
#[inline(never)]
pub fn leaked() -> &'static mut State {
    Box::leak(Box::new(State::new()))
}

#[inline(never)]
pub fn step(s: &mut State, n: u64) -> u64 {
    if crate::HOLD0.swap(false, std::sync::atomic::Ordering::SeqCst) {
        crate::park_here(n);
    }
    s.a = s.a.wrapping_add(n & 1);
    10
}
