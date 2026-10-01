pub mod opt;
pub mod wasm_strip;
pub use opt::profile::{
    wasm_size_profile, DataSegmentSize, FunctionSize, GroupSize, Profile, ProfileOptions,
    SectionSize,
};
pub use opt::{
    wasm_optimize, wasm_optimize_checked, wasm_validate, OptimizeOptions, OptimizePassReport,
    OptimizeReport,
};
pub use wasm_strip::*;
