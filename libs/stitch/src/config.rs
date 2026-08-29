pub(crate) const MAX_TYPE_COUNT: usize = 1_000_000;
pub(crate) const MAX_IMPORT_COUNT: usize = 100_000;
pub(crate) const MAX_FUNC_COUNT: usize = 1_000_000;
pub(crate) const MAX_FUNC_PARAM_COUNT: usize = 1_000;
pub(crate) const MAX_FUNC_RESULT_COUNT: usize = 1_000;
pub(crate) const MAX_TABLE_COUNT: usize = 100;
pub(crate) const MAX_MEMORY_COUNT: usize = 1;
pub(crate) const MAX_GLOBAL_COUNT: usize = 1_000_000;
pub(crate) const MAX_EXPORT_COUNT: usize = 100_000;
pub(crate) const MAX_ELEM_COUNT: usize = 100_000;
pub(crate) const MAX_ELEM_SIZE: usize = 1_000_000;
pub(crate) const MAX_FUNC_LOCAL_COUNT: usize = 50_000;
pub(crate) const MAX_FUNC_BODY_SIZE: usize = 128 * 1_024;
pub(crate) const MAX_DATA_COUNT: usize = 100_000;
pub(crate) const MAX_DATA_SIZE: usize = 1_000_000;

/// Nonstandard engine extensions.
///
/// All extensions are off by default, in which case the engine accepts
/// exactly the same modules as before. Enable them with
/// [`Engine::new_with_extensions`](crate::Engine::new_with_extensions).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extensions {
    /// Enables the NONSTANDARD float math opcodes (0xE0-prefixed):
    /// scalar `f32`/`f64` and packed `f32x4` sin, cos, tan, asin, acos,
    /// atan, exp, ln, atan2, pow, rmin and rmax.
    ///
    /// A module using these opcodes is not a valid Wasm module for any
    /// other engine, so this is opt-in: modules compiled by AOT compilers
    /// that target stitch specifically (e.g. the makepad splash math AOT)
    /// enable it, everything else leaves it off.
    pub ext_math: bool,
}
