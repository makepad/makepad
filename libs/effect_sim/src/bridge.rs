//! The bridge between the stepper and kernel buffers (KERNELS.md §3.6.3).
//!
//! **Out of physics.** Body poses are extracted from the world at every
//! step (a CPU copy proportional to the body count, measured in
//! [`crate::StepperStats::extract_ns`]) and, per drawn frame, interpolated
//! and written as instance records into a ring slot of
//! `makepad-render-kernels` ([`publish_bodies`]): the draw reads the
//! published slot, and a slot a submitted frame still reads is never
//! written. The record is whatever the draw shader reflects (fields by
//! name: pos, rot, scale, color, seed, id, prev_pos, prev_rot, age,
//! radius, speed, group), or [`crate::rigid_layout`].
//!
//! **Into physics.** A kernel's `Body` records are spawns; each record is
//! validated before anything is created ([`crate::BodyRecord::validate`]).
//! Contacts and events come from the retained history, as `Contact` and
//! `Event` records a decoration kernel reads (`input(Contact)`).
//!
//! **Queries.** [`PHYSICS_MODULE`] is Splash for kernels over `Rigid`
//! records (the bodies of a group, bound as an input): nearest body, a ray
//! against the bodies' bounding spheres, overlap counts. A host passes it
//! to the kernel compiler as the module `physics`, so a kernel calls
//! `physics.nearest(bodies, n, p)` without a `use`.

use crate::stepper::View;
use makepad_render_kernels::compute::kernel::Layout;
use makepad_render_kernels::{FrameFences, OutputRing, PublishError, Topology};

/// The kernel module a host registers as `physics` (queries over `Rigid`
/// records; `n` is the record count, which the host sets as the param
/// `<input>_count`). Loops over records are bounded by the kernel
/// language's runtime-loop cap of 1024 iterations; beyond that, split the
/// query or read a record by index.
pub const PHYSICS_MODULE: &str = r#"
// The position of body k.
fn pos(bodies, k) { bodies[k].pos }

// The index of the body nearest to p (-1 when there are none).
fn nearest(bodies, n, p) {
    let best = -1
    let d = 1.0e30
    for k in 0..n {
        let q = bodies[k].pos - p
        let e = dot(q, q)
        if e < d {
            d = e
            best = k
        }
    }
    best
}

// How many bodies' bounding spheres reach within r of p.
fn overlaps(bodies, n, p, r) {
    let c = 0
    for k in 0..n {
        let q = bodies[k].pos - p
        let rr = bodies[k].radius + r
        if dot(q, q) < rr * rr {
            c = c + 1
        }
    }
    c
}

// The first hit of a ray (origin o, unit direction d) against the bodies'
// bounding spheres, up to max_t: vec2(t, index), or vec2(-1, -1).
fn raycast(bodies, n, o, d, max_t) {
    let hit_t = max_t
    let hit = -1
    for k in 0..n {
        let oc = o - bodies[k].pos
        let r = bodies[k].radius
        let b = dot(oc, d)
        let c = dot(oc, oc) - r * r
        let h = b * b - c
        if h >= 0.0 {
            let t = -b - sqrt(h)
            if t >= 0.0 && t < hit_t {
                hit_t = t
                hit = k
            }
        }
    }
    if hit < 0 { vec2(-1.0, -1.0) } else { vec2(hit_t, float(hit)) }
}
"#;

/// A ring for a group's instances in `layout` (two slots: the CPU path).
pub fn body_ring(layout_id: u64, layout: &Layout) -> OutputRing {
    OutputRing::new(2, Topology::Instances, layout_id, layout.stride)
}

/// Writes the bodies of `group` at the view's time as `layout` records
/// into a free slot of `ring` and publishes it; returns the published
/// generation and record count. With no free slot (every slot still read
/// by frames in flight) the previous slot keeps drawing: `Ok(None)`.
pub fn publish_bodies(view: &View, group: &str, layout: &Layout, ring: &mut OutputRing, fences: &dyn FrameFences) -> Result<Option<(u64, u32)>, PublishError> {
    let Ok(mut lease) = ring.begin_write(fences) else { return Ok(None) };
    let mut data = lease.take_data();
    let n = view.write_bodies(group, layout, &mut data);
    lease.set_data(data);
    let generation = ring.publish(lease, n as u32, fences)?;
    Ok(Some((generation, n as u32)))
}

/// The two kernel sources of a `Sim{}` (KERNELS.md §3.5.6): a Sim is a
/// kernel whose state buffers are `prev` (the last state, any element
/// readable) and `next` (this step's, element-local writes), in the state
/// layout. `init: fn(i) { next[i].pos = .. }` writes the first state,
/// `update: fn(i) { next[i] = prev[i]  next[i].life = prev[i].life - 0.1 }`
/// every later one. `helpers` are the document's functions and constants
/// the bodies call (as kernel items), `decls` extra declarations (params,
/// inputs such as `let hits = input(Contact)`, an `emit_buffer(Body, n)`
/// for spawns).
pub struct SimSources {
    pub init: String,
    pub update: String,
}

pub fn sim_sources(state: &str, init_param: &str, init_body: &str, update_param: &str, update_body: &str, helpers: &str, decls: &str) -> SimSources {
    let init = format!("{helpers}\n{decls}\nlet next = output({state})\nfn element({init_param}) {{\n{init_body}\n}}\n");
    let update = format!("{helpers}\n{decls}\nlet prev = input({state})\nlet next = output({state})\nfn element({update_param}) {{\n{update_body}\n}}\n");
    SimSources { init, update }
}
