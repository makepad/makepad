use crate::counters::Timer;
use std::fmt::{Display, Formatter, Result};

/// Performance counters related to each stage of the time step.
#[derive(Default, Clone, Copy)]
pub struct StagesCounters {
    /// Time spent for updating the kinematic and dynamics of every body.
    pub update_time: Timer,
    /// Total time spent for the collision detection (including both broad- and narrow- phases).
    pub collision_detection_time: Timer,
    /// Time spent for the computation of collision island and body activation/deactivation (sleeping).
    pub island_construction_time: Timer,
    /// Time spent for collecting awake constraints from islands.
    pub island_constraints_collection_time: Timer,
    /// Total time spent for the constraints resolution and position update.t
    pub solver_time: Timer,
    /// Total time spent for CCD and CCD resolution.
    pub ccd_time: Timer,
    /// Total time spent propagating user changes.
    pub user_changes: Timer,
}

impl StagesCounters {
    /// Create a new counter initialized to zero.
    pub fn new() -> Self {
        StagesCounters {
            update_time: Timer::new(),
            collision_detection_time: Timer::new(),
            island_construction_time: Timer::new(),
            island_constraints_collection_time: Timer::new(),
            solver_time: Timer::new(),
            ccd_time: Timer::new(),
            user_changes: Timer::new(),
        }
    }

    /// Resets all the counters and timers.
    pub fn reset(&mut self) {
        self.update_time.reset();
        self.collision_detection_time.reset();
        self.island_construction_time.reset();
        self.island_constraints_collection_time.reset();
        self.solver_time.reset();
        self.ccd_time.reset();
        self.user_changes.reset();
    }
}

impl Display for StagesCounters {
    fn fmt(&self, f: &mut Formatter) -> Result {
        writeln!(f, "Update time: {}", self.update_time)?;
        writeln!(
            f,
            "Collision detection time: {}",
            self.collision_detection_time
        )?;
        writeln!(
            f,
            "Island construction time: {}",
            self.island_construction_time
        )?;
        writeln!(
            f,
            "Island construction time: {}",
            self.island_constraints_collection_time
        )?;
        writeln!(f, "Solver time: {}", self.solver_time)?;
        writeln!(f, "CCD time: {}", self.ccd_time)?;
        writeln!(f, "User changes time: {}", self.user_changes)
    }
}
