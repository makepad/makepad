//! The OS layer of the live image runtime (jit.rs): one module per concern, each choosing its
//! Linux / macOS / Windows half in one place at its top. jit.rs and the rest of the compiler
//! name no OS API; everything they need from the system goes through these small interfaces.
//!
//! - [`mem`]: the unit's reservation (code + data), committed pages, zeroed read-write memory,
//!   code writes (W^X / icache), compiler stack segments.
//! - [`fault`]: hardware faults in program code (signals / vectored exceptions) handed to the
//!   runtime's portable policy (`jit::on_fault`) as a [`fault::Fault`], the per-thread fault
//!   stack.
//! - [`thread`]: thread ids, names, stack bounds, the watchdog's interrupt, priorities, fork hooks.
//! - [`sample`]: the tier-up sampler's per-thread sources.
//! - [`tls`]: the OS key holding each thread's thread-local block and the fast path JIT code
//!   reads it through.
//! - [`proc`]: foreign symbols and libraries, power state, process arguments, private files.

// explicit paths: the file is also loaded through #[path] by the bring-up driver, where nested
// modules would otherwise resolve next to it (the Win32 declarations are not here: relax_session::win32)
#[path = "os/fault.rs"]
pub mod fault;
#[path = "os/mem.rs"]
pub mod mem;
#[path = "os/proc.rs"]
pub mod proc;
#[path = "os/sample.rs"]
pub mod sample;
#[path = "os/thread.rs"]
pub mod thread;
#[path = "os/tls.rs"]
pub mod tls;
#[cfg(windows)]
pub(crate) use relax_session::win32;
