// Copy: a field added to it takes the restart rung (M10 keeps Copy structs' layout), the step
// that checks the state-preserving restart
#[derive(Clone, Copy)]
pub struct Conf {
    pub base: u64,
}

pub fn conf() -> Conf {
    Conf { base: 1 }
}

pub fn value(_n: u64) -> u64 {
    conf().base
}
