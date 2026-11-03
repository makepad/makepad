//! Compute programs: long-running, sandboxed Splash programs that drive CPU (and, with a GPU
//! device, GPU) kernels, run in-process or on an AI hub node with the same source.
//!
//! A program is one Splash document whose last value is `{run: fn(s, p) {..}}`. `run` executes
//! in its own isolate (its own VM and heap, domain `compute`: the core modules, `mod.kernel` and
//! `mod.compute`, never fs, net, host or cx) on the session's driver thread. `s` is the session
//! (`mod.compute`), `p` the parameters the app opened it with.
//!
//! ```splash
//! use mod.pod.*
//! use mod.kernel.*
//! let axpy = {label: "axpy" x: input(f32) y: output(f32) a: param(0.0)
//!             element: fn(i) { y[i] = y[i] + a * x[i] }}
//! {run: fn(s, p) {
//!     let x = s.buffer(f32, p.n)
//!     let y = s.buffer(f32, p.n)
//!     s.fill(x, 1.0)
//!     s.run(axpy, p.n, {a: 2.0}, {x: x, y: y})
//!     let block = s.buffer(f32, 1)
//!     s.sum(y, block, 0)
//!     return s.read(block)[0]
//! }}
//! ```
//!
//! - **Ops record, boundaries run them.** Buffer and kernel ops are recorded into a segment; a
//!   boundary (`read`, `flush`, `publish`, `state`, `inbox`, `wait_inbox`, the end of `run`)
//!   runs it. The CPU device runs each kernel over the executor's workers; any worker count gives
//!   the same bits, and so does the interpreter.
//! - **One thread unless own records**: a kernel runs on every worker only when each access to a
//!   buffer it writes is `buf[i]` (its element's own record); `buf[i * 3 + k]` runs it on one
//!   thread. Keep a vector's components in separate buffers, or write a record's values through
//!   views of its stride (`c1: output(@f64, 48, 2, "cf")`: f64 1 of 24 an element).
//! - **Handles** (`{compute_buffer, gen}`) index the session's own table; kernels are the
//!   program's `mod.kernel` objects, compiled once each in the isolate.
//! - **Steering**: the app's messages land in the inbox, the newest per key
//!   (`let m = s.wait_inbox({stop: nil, view: nil})`: `m.msgs` has every key the program
//!   names, nil where no message came). An interrupting
//!   message also stops the device work in flight and drops the segment not yet run: the next
//!   `read` returns nil until the program takes the inbox.
//! - **Outputs**: `publish` (a buffer at a chosen precision), `state` (full precision, for
//!   resume), `value` and `progress`, each the newest per name ([`Updates`]).
//! - **Limits are counted, never timed** ([`Limits`]): VM instructions, the live heap, stack and
//!   call depth, kernel work, buffer bytes. A program over one ends with
//!   [`ProgramError::OverBudget`]; a buffer the bytes do not cover comes back as a refusal value
//!   the program can answer (`{refused, wanted, available}`).

pub mod codec;
mod cpu;
mod driver;
mod session;
mod value;
pub mod wire;

/// The proof programs, as Splash source.
pub mod programs {
    /// Fractal tiles on request: `{tile}` messages, each published as `tile0` / `tile1`.
    pub const FRACTAL: &str = include_str!("../programs/fractal.splash");
    /// The most iterations a [`FRACTAL`] tile runs (64 passes of 2048).
    pub const FRACTAL_MAX_ITER: u32 = 131_072;
    /// A PCG-solved elastic cantilever and its SIMP optimisation, steered by `{load}`.
    pub const ELASTIC: &str = include_str!("../programs/elastic.splash");
    /// D3Q19 lattice-Boltzmann flow past a sphere, with its drag.
    pub const FLOW: &str = include_str!("../programs/flow.splash");
}

pub use cpu::ElemTy;
pub use session::{Class, Input, Limits, Output, OutputData, Program, ProgramError, Session, SessionConfig, Updates};
pub use value::Value;
pub use makepad_script_compute::Backend;
pub use makepad_script_compute_sched::sched::Executor;
