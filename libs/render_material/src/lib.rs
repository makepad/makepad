//! Materials for the 3D renderer (KERNELS.md §3.3).
//!
//! Two built-in kinds, both drawn by the render crate's model lanes: `Pbr`
//! (the metal/rough lane) and `Unlit`. A Splash material extends either with
//! hook functions (`hooks`), compiled by the shader DSL at runtime: no
//! recompile of the engine for a new look. A host allows a subset of hooks
//! (`HookMask`); variants (the lighting composition, IBL, Unlit, shadow
//! casters) are derived from the hooks and the blend (`variants`), and a
//! material that fails to compile reports its errors with the author's
//! source lines (`program`) and draws as the error material.
pub mod builtin;
pub mod hooks;
pub mod ibl;
pub mod program;
pub mod variants;

pub use builtin::Builtin;
pub use hooks::{Hook, HookMask, HookSpec, Stage, HOOKS};
pub use program::{build_program, diagnose, install_program, frontend_errors, frontend_errors_for, parse_diagnostic, Diagnostic, HookSet, MaterialError, ProgramRequest};
pub use variants::{plan, MaterialDesc, ShadowVariant, VariantPlan};
pub use makepad_draw::makepad_platform::makepad_script::shader_backend::ShaderBackend;
