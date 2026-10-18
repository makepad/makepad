//! Tower cranes: the city camera, on city lots the city leaves empty, most beside an unfinished
//! concrete frame; slot 1 beyond 1.25 x the downtown distance, else the props layer hidden by
//! nearer city surfaces. Shares the town group's helpers (the lot hash `z.rng.hcell`). Zone: `z.crane`.
//!
//! Port of the page's `pkCranes` (stage C2, group 1): emit instances and parts through [`Ctx`].
//! Stub: emits nothing yet.
use super::kit::Ctx;

pub fn build(_c: &mut Ctx) {}
