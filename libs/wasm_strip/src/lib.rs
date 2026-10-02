pub mod opt;
pub mod wasm_strip;
pub use opt::profile::{
    wasm_size_profile, DataSegmentSize, FunctionSize, GroupSize, Profile, ProfileOptions,
    SectionSize,
};
pub use opt::units::{Coverage, ModuleUse, COVERAGE_EXPORT};
pub use opt::{
    wasm_coverage, wasm_guard_waits, wasm_instrument_coverage, wasm_optimize, wasm_optimize_checked, wasm_validate,
    OptimizeOptions, OptimizePassReport, OptimizeReport,
};
pub use wasm_strip::*;
