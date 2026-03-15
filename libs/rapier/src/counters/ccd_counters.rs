use crate::counters::Timer;
use std::fmt::{Display, Formatter, Result};

/// Performance counters related to continuous collision detection (CCD).
#[derive(Default, Clone, Copy)]
pub struct CCDCounters {
    /// The number of substeps actually performed by the CCD resolution.
    pub num_substeps: usize,
    /// The total time spent for TOI computation in the CCD resolution.
    pub toi_computation_time: Timer,
    /// The total time spent for force computation and integration in the CCD resolution.
    pub solver_time: Timer,
    /// The total time spent by the broad-phase in the CCD resolution.
    pub broad_phase_time: Timer,
    /// The total time spent by the narrow-phase in the CCD resolution.
    pub narrow_phase_time: Timer,
}

impl CCDCounters {
    /// Creates a new counter initialized to zero.
    pub fn new() -> Self {
        CCDCounters {
            num_substeps: 0,
            toi_computation_time: Timer::new(),
            solver_time: Timer::new(),
            broad_phase_time: Timer::new(),
            narrow_phase_time: Timer::new(),
        }
    }

    /// Resets this counter to 0.
    pub fn reset(&mut self) {
        self.num_substeps = 0;
        self.toi_computation_time.reset();
        self.solver_time.reset();
        self.broad_phase_time.reset();
        self.narrow_phase_time.reset();
    }
}

impl Display for CCDCounters {
    fn fmt(&self, f: &mut Formatter) -> Result {
        writeln!(f, "Number of substeps: {}", self.num_substeps)?;
        writeln!(f, "TOI computation time: {}", self.toi_computation_time)?;
        writeln!(f, "Constraints solver time: {}", self.solver_time)?;
        writeln!(f, "Broad-phase time: {}", self.broad_phase_time)?;
        writeln!(f, "Narrow-phase time: {}", self.narrow_phase_time)
    }
}
