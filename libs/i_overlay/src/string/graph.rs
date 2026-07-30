use crate::build::builder::GraphNode;
use crate::core::link::OverlayLink;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;

pub struct StringGraph<'a, I: IntNumber> {
    pub(crate) nodes: &'a [Vec<usize>],
    pub(crate) links: &'a mut [OverlayLink<I>],
}

impl GraphNode for Vec<usize> {
    #[inline(always)]
    fn with_indices(indices: &[usize]) -> Self {
        indices.to_vec()
    }
}
