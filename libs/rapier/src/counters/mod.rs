//! Counters for benchmarking various parts of the physics engine.

use core::time::Duration;
use std::fmt::{Display, Formatter, Result};

pub use self::ccd_counters::CCDCounters;
pub use self::collision_detection_counters::CollisionDetectionCounters;
pub use self::solver_counters::SolverCounters;
pub use self::stages_counters::StagesCounters;
pub use self::timer::Timer;

mod ccd_counters;
mod collision_detection_counters;
mod solver_counters;
mod stages_counters;
mod timer;

/// Aggregation of all the performances counters tracked by rapier.
#[derive(Clone, Copy)]
pub struct Counters {
    /// Whether this counter is enabled or not.
    pub enabled: bool,
    /// Timer for a whole timestep.
    pub step_time: Timer,
    /// Timer used for debugging.
    pub custom: Timer,
    /// Counters of every stage of one time step.
    pub stages: StagesCounters,
    /// Counters of the collision-detection stage.
    pub cd: CollisionDetectionCounters,
    /// Counters of the constraints resolution and force computation stage.
    pub solver: SolverCounters,
    /// Counters for the CCD resolution stage.
    pub ccd: CCDCounters,
}

impl Counters {
    /// Create a new set of counters initialized to zero.
    pub fn new(enabled: bool) -> Self {
        Counters {
            enabled,
            step_time: Timer::new(),
            custom: Timer::new(),
            stages: StagesCounters::new(),
            cd: CollisionDetectionCounters::new(),
            solver: SolverCounters::new(),
            ccd: CCDCounters::new(),
        }
    }

    /// Enable all the counters.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Return `true` if the counters are enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Disable all the counters.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Notify that the time-step has started.
    pub fn step_started(&mut self) {
        if self.enabled {
            self.step_time.start();
        }
    }

    /// Notify that the time-step has finished.
    pub fn step_completed(&mut self) {
        if self.enabled {
            self.step_time.pause();
        }
    }

    /// Total time spent for one of the physics engine.
    pub fn step_time(&self) -> Duration {
        self.step_time.time()
    }

    /// Total time spent for one of the physics engine, in milliseconds.
    pub fn step_time_ms(&self) -> f64 {
        self.step_time.time_ms()
    }

    /// Notify that the custom operation has started.
    pub fn custom_started(&mut self) {
        if self.enabled {
            self.custom.start();
        }
    }

    /// Notify that the custom operation has finished.
    pub fn custom_completed(&mut self) {
        if self.enabled {
            self.custom.pause();
        }
    }

    /// Total time of a custom event.
    pub fn custom_time(&self) -> Duration {
        self.custom.time()
    }

    /// Total time of a custom event, in milliseconds.
    pub fn custom_time_ms(&self) -> f64 {
        self.custom.time_ms()
    }

    /// Set the number of constraints generated.
    pub fn set_nconstraints(&mut self, n: usize) {
        self.solver.nconstraints = n;
    }

    /// Set the number of contacts generated.
    pub fn set_ncontacts(&mut self, n: usize) {
        self.solver.ncontacts = n;
    }

    /// Set the number of contact pairs generated.
    pub fn set_ncontact_pairs(&mut self, n: usize) {
        self.cd.ncontact_pairs = n;
    }

    /// Resets all the counters and timers.
    pub fn reset(&mut self) {
        if self.enabled {
            self.step_time.reset();
            self.custom.reset();
            self.stages.reset();
            self.cd.reset();
            self.solver.reset();
            self.ccd.reset();
        }
    }
}

macro_rules! measure_method {
    ($started:ident, $stopped:ident, $time_ms:ident, $info:ident. $timer:ident) => {
        impl Counters {
            /// Start this timer.
            pub fn $started(&mut self) {
                if self.enabled {
                    self.$info.$timer.start()
                }
            }

            /// Stop this timer.
            pub fn $stopped(&mut self) {
                if self.enabled {
                    self.$info.$timer.pause()
                }
            }

            /// Gets the time elapsed for this timer, in milliseconds.
            pub fn $time_ms(&self) -> f64 {
                if self.enabled {
                    self.$info.$timer.time_ms()
                } else {
                    0.0
                }
            }
        }
    };
}

measure_method!(
    update_started,
    update_completed,
    update_time_ms,
    stages.update_time
);
measure_method!(
    collision_detection_started,
    collision_detection_completed,
    collision_detection_time_ms,
    stages.collision_detection_time
);
measure_method!(
    island_construction_started,
    island_construction_completed,
    island_construction_time_ms,
    stages.island_construction_time
);
measure_method!(
    solver_started,
    solver_completed,
    solver_time_ms,
    stages.solver_time
);
measure_method!(ccd_started, ccd_completed, ccd_time_ms, stages.ccd_time);

measure_method!(
    assembly_started,
    assembly_completed,
    assembly_time_ms,
    solver.velocity_assembly_time
);
measure_method!(
    velocity_resolution_started,
    velocity_resolution_completed,
    velocity_resolution_time_ms,
    solver.velocity_resolution_time
);
measure_method!(
    velocity_update_started,
    velocity_update_completed,
    velocity_update_time_ms,
    solver.velocity_update_time
);
measure_method!(
    broad_phase_started,
    broad_phase_completed,
    broad_phase_time_ms,
    cd.broad_phase_time
);
measure_method!(
    narrow_phase_started,
    narrow_phase_completed,
    narrow_phase_time_ms,
    cd.narrow_phase_time
);

impl Display for Counters {
    fn fmt(&self, f: &mut Formatter) -> Result {
        writeln!(f, "Total timestep time: {}", self.step_time)?;
        self.stages.fmt(f)?;
        self.cd.fmt(f)?;
        self.solver.fmt(f)?;
        writeln!(f, "Custom timer: {}", self.custom)
    }
}

impl Default for Counters {
    fn default() -> Self {
        Self::new(false)
    }
}
