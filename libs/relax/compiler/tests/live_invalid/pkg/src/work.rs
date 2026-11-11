pub struct State {
    pub a: u64,
}

impl State {
    pub fn new() -> State {
        State { a: 1 }
    }
}

/// The worker's state, made here: the worker's loop holds only a reference.
#[inline(never)]
pub fn leaked() -> &'static mut State {
    Box::leak(Box::new(State::new()))
}

#[inline(never)]
pub fn step(s: &mut State, n: u64) -> u64 {
    s.a = s.a.wrapping_add(n & 1);
    10
}
