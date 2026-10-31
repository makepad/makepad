//! Kernel execution policy on the host: admission, executors, reusable jobs
//! and deterministic chunk orchestration. Compilation and borrowed-buffer
//! execution are supplied by makepad-script-compute.
pub mod admission;
pub mod kernel;
pub mod pipeline;
pub mod sched;
