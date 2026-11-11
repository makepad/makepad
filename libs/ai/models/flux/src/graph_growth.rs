//! Growing a weight arena's graph reserve when a graph build runs out of it.
//!
//! The FLUX-family encoders and decoders build their graphs in the host
//! context that holds their weights, behind a reserve sized at load. When a
//! build runs out of that reserve, the weights are reloaded with twice the
//! reserve and the build runs again. The reload must give the same weights:
//! the same file, the same component of a combined checkpoint, the same
//! merged LoRA adapters and the same owner, so the device copies already
//! uploaded under that owner stay valid and nothing is uploaded twice.

use crate::{DiffusionError, Result};

/// How many times one build may double the reserve before its context
/// out-of-memory error is returned.
pub(crate) const MAX_GRAPH_GROWTH_ATTEMPTS: usize = 3;

/// Weights that keep a graph reserve in their host context and can be
/// reloaded with a larger one.
pub(crate) trait GraphReserve: Sized {
    fn graph_reserve_bytes(&self) -> usize;

    /// These weights (file, component scope, LoRA merge and owner) over
    /// `extra_bytes` of graph reserve.
    fn reloaded_with_graph_reserve(&self, extra_bytes: usize) -> Result<Self>;
}

pub(crate) fn is_context_oom(err: &DiffusionError) -> bool {
    matches!(err, DiffusionError::Model(message) if message.starts_with("context out of memory allocating "))
}

/// Runs `build` over `weights`. While it runs out of context memory, the
/// weights are reloaded with twice the reserve and `build` runs again, at
/// most [`MAX_GRAPH_GROWTH_ATTEMPTS`] times.
pub(crate) fn build_with_graph_growth<W: GraphReserve, G>(
    weights: &mut W,
    mut build: impl FnMut(&mut W) -> Result<G>,
) -> Result<G> {
    let mut grown = 0;
    loop {
        match build(weights) {
            Err(err) if is_context_oom(&err) && grown < MAX_GRAPH_GROWTH_ATTEMPTS => {
                let extra_bytes = weights
                    .graph_reserve_bytes()
                    .checked_mul(2)
                    .ok_or_else(|| DiffusionError::model("graph reserve overflow"))?;
                *weights = weights.reloaded_with_graph_reserve(extra_bytes)?;
                grown += 1;
            }
            result => return result,
        }
    }
}
