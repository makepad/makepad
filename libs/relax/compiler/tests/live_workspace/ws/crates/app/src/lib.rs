pub fn f(x: u32) -> u32 {
    lw_core::base(x) + 1
}

pub fn state() -> usize {
    lw_state::state()
}
