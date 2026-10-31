#![feature(unsize)]
// rustc_hir_analysis/src/coherence/mod.rs:73-94: Unsize has its own E0328 code.
struct Value;
impl std::marker::Unsize<Value> for Value {}
