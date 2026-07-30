use crate::core::solver::Solver;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;

pub(super) struct SnapRadius {
    current: usize,
    step: usize,
}

impl SnapRadius {
    pub(super) fn increment(&mut self) {
        self.current = 60.min(self.current + self.step);
    }

    pub(super) fn radius<I: IntNumber>(&self) -> I::Wide {
        I::Wide::ONE << self.current as u32
    }
}

impl Solver {
    pub(super) fn snap_radius(&self) -> SnapRadius {
        SnapRadius {
            current: self.precision.start,
            step: self.precision.progression,
        }
    }
}
