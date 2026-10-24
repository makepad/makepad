//! Relax's middle end on RIR (relax-backend owns RIR, register allocation and machine code).
//! [`opt`] is the O1/O3 optimiser, [`loops`] and [`ranges`] its loop and range passes,
//! [`interp`] the threaded interpreter tier, [`native`] Relax's entry into the back end, and
//! [`fncost`], [`warm`] and [`exec_census`] the compile profilers.

pub mod fncost;
pub mod interp;
pub mod exec_census;
pub mod warm;
pub mod loops;
pub mod native;
pub mod opt;
pub mod ranges;
pub mod version;
pub mod web32;
