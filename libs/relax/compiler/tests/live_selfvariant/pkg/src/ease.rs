// An enum whose impl matches its struct variants through `Self::Variant { .. }` (as
// makepad's animator.rx Ease::map does). It is never edited: the live checks of other files
// type-check it again (main.rs's layout edit re-checks the users of the edited struct).
pub enum Ease {
    Linear,
    Pow { begin: u32, end: u32 },
    Steps(u32),
}

impl Ease {
    pub fn map(&self, t: u32) -> u32 {
        match self {
            Self::Linear => t,
            Self::Pow { begin, end } => t * begin + end,
            Self::Steps(n) => t / n.max(&1),
        }
    }
}

pub fn ease<T: Into<u32>>(e: &Ease, t: T) -> u32 {
    e.map(t.into())
}
