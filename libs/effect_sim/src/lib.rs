//! Physics as a VFX layer (KERNELS.md §3.6, phase P7): a deterministic
//! stepper that hosts a box3d world and stateful kernel sims for a
//! document, on a fixed, exact time grid, seeded, seekable.
//!
//! - [`time`]: sim time as whole ticks, the rational step, `step(t)`,
//!   `alpha(t)`, and spawn rounding (`ceil(t / dt)`: a beat is exact to
//!   one step).
//! - [`desc`]: what a document declares, `Physics{}`, `Body{}`,
//!   `Spawn{}`, `Sim{}`, as plain values with compiled kernels.
//! - [`stepper`]: stepping, complete in-memory checkpoints (the world's
//!   snapshot and geometry registry, spawn cursor and queue, contact and
//!   event history, sim states, stable ids, a version stamp), seek, the
//!   realtime `advance_to`, and [`View`]s (bodies interpolated between
//!   steps, contacts and events in the retention window, sim states).
//! - [`layouts`]: the `Body`, `Rigid`, `Contact` and `Event` records
//!   kernels read and write.
//! - [`bridge`]: bodies published as instance records through
//!   `makepad-render-kernels` rings, the kernel query module, and the
//!   `Sim{}` kernel sources.
//!
//! **Contract: same-machine reproducibility** (user decision 2026-09-30).
//! Export, seek from any checkpoint and re-render give the same result on
//! the machine and build that renders. Nothing here is compared across
//! machines. Gameplay physics stays in Sandbox's own sim.

pub mod bridge;
pub mod desc;
pub mod layouts;
pub mod stepper;
pub mod time;
mod world;

pub use bridge::{body_ring, publish_bodies, sim_sources, SimSources, PHYSICS_MODULE};
pub use desc::*;
pub use layouts::*;
pub use stepper::{slerp, stamp_of, Drive, Fnv, NoDrive, Progress, Stepper, StepperStats, View};
pub use time::{DtError, SimTime, StepDt, TICKS_PER_SECOND};

/// The kernel module list a host passes to its kernel compiler so kernels
/// reach the physics queries as `physics.*`.
pub fn modules() -> [makepad_render_kernels::compute::module::Module<'static>; 1] {
    [makepad_render_kernels::compute::module::Module { path: "physics", source: PHYSICS_MODULE }]
}

/// The AI's guide to physics in a document (one dense block, the Scene3D
/// guide's form).
pub const GUIDE: &str = r#"## Physics (effects only)
`physics: Physics{step: 1 / 240 seed: 7 gravity: vec3(0, -9.8, 0) retain: 0.3 hit_speed: 0.05 bodies: [..] spawn: [..]}` on a Scene3D. Fixed step, seeded: the same document renders the same at any seek, export or re-render on this machine.
`Body{name group shape: @box | @sphere | @capsule | @plane fixed: true | kinematic: true position rotation size scale color vel spin drag spin_drag density friction bounce lift life}`: a declared body; `kinematic` follows its keyable `position`/`rotation` and pushes the rest.
`Spawn{at: beat(16) group: "confetti" count: 600 shape: @box size from: <kernel>}`: at the first step at or after `at`, a kernel's `out: output(Body)` (one record per element) or `emit_buffer(Body, n)` makes bodies. Records: pos size rot vel spin drag spin_drag density friction bounce lift delay life color scale shape (1 box, 2 sphere, 3 capsule; 0 the spawn's). Zero means default: rot identity, density 1000, friction 0.6, color white, scale 1. `delay` (s) staggers a launch; `life` (s) removes the body after it.
`bodies("g")` is an Instances/Points source (interpolated between steps); in a kernel `input(bodies("g"))` gives `Rigid` records: pos rot scale color seed id prev_pos prev_rot age radius speed group.
`input(contacts("g"))` gives `Contact` records of the last `retain` seconds involving the group: pos normal impulse speed time a b step; `input(events("g"))` gives `Event`: kind (0 spawn 1 expire 2 sleep) id time step pos. The element count is the record count; `<input>_count` holds it.
`Sim{state: {pos: vec3(0, 0, 0) life: 0.0} count every: 1 init: fn(i) { next[i].pos = .. } update: fn(i) { next[i] = prev[i] .. }}`: state that steps with physics (trails, boids, sparks); `prev[k]` reads any element of the last state, `next[i]` is this element's new state; a `drops: emit_buffer(Body, n)` field spawns bodies into group `drops`. Its state is a Points source. `physics.nearest(bodies, n, p)`, `physics.overlaps(bodies, n, p, r)`, `physics.raycast(bodies, n, o, d, max_t)` query bodies from a kernel.
Time is forward only: launch on the beat and tune by preview; physics never solves backwards to land on a beat."#;
