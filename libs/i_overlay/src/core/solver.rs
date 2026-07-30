use crate::core::solver::Strategy::{Auto, Frag, List, Tree};
use crate::segm::segment::Segment;
use i_float::int::number::int::IntNumber;

/// Represents the selection strategy or algorithm for processing geometric data, aimed at optimizing performance under various conditions.
///
/// This enum allows for the explicit selection of a computational approach to geometric data processing. The choice of solver is crucial as it directly affects the efficiency of operations, especially in relation to the complexity and size of the dataset involved.
///
/// Cases:
/// - `List`: A linear list-based approach for organizing and processing geometric data. Typically, performs better for smaller datasets, approximately with fewer than 10,000 edges, due to its straightforward processing model. For small to moderate datasets, this method can offer a balance of simplicity and speed.
/// - `Tree`: Implements a tree-based data structure (e.g., a binary search tree or a spatial partitioning tree) to manage geometric data. This method is generally more efficient for larger datasets or scenarios requiring complex spatial queries, as it can significantly reduce the number of comparisons needed for operations. However, its performance advantage becomes more apparent as the dataset size exceeds a certain threshold (roughly estimated at 10,000 edges).
/// - `Auto`: Delegates the choice of solver to the system, which determines the most suitable approach based on the size and complexity of the dataset. This option is designed to dynamically select between `list` and `tree` strategies, aiming to optimize performance without requiring a priori knowledge of the data's characteristics. It's the recommended choice for users looking for a balance between performance and ease of use, as it adapts to the specific requirements of each operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    List,
    Tree,
    Frag,
    Auto,
}

/// Represents the precision level used by the solver to determine
/// the tolerance for snapping to the nearest edge ends.
///
/// The precision determines a radius calculated as `2^value`,
/// where `value` starts at `start` and increases in increments
/// defined by `progression` in each iteration.
///
/// - `start`: The initial exponent value.
/// - `progression`: The step size for incrementing the exponent
///   in each iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Precision {
    /// The initial exponent value for the radius calculation.
    pub start: usize,
    /// The amount by which the exponent increases in each iteration.
    pub progression: usize,
}

impl Precision {
    /// Absolute precision with no progression.
    /// (Radius remains at `2^0 = 1`)
    pub const ABSOLUTE: Precision = Self {
        start: 0,
        progression: 0,
    };

    /// High precision, starting at `2^0 = 1` and doubling every loop.
    pub const HIGH: Precision = Self {
        start: 0,
        progression: 1,
    };

    /// Medium-high precision, starting at `2^1 = 2` and doubling every loop.
    pub const MEDIUM_HIGH: Precision = Self {
        start: 1,
        progression: 1,
    };

    /// Medium precision, starting at `2^0 = 1` and quadrupling every loop.
    pub const MEDIUM: Precision = Self {
        start: 0,
        progression: 2,
    };

    /// Medium-low precision, starting at `2^2 = 4` and quadrupling every loop.
    pub const MEDIUM_LOW: Precision = Self {
        start: 2,
        progression: 2,
    };

    /// Low precision, starting at `2^2 = 4` and increasing by a factor of 8 every loop.
    pub const LOW: Precision = Self {
        start: 2,
        progression: 3,
    };
}

#[derive(Clone, Copy)]
pub struct MultithreadOptions {
    pub par_sort_min_size: usize,
}

impl MultithreadOptions {
    const DEFAULT_PAR_SORT_MIN_SIZE: usize = 32768;
}

impl Default for MultithreadOptions {
    fn default() -> Self {
        Self {
            par_sort_min_size: 32768,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Solver {
    pub strategy: Strategy,
    pub precision: Precision,
    pub multithreading: Option<MultithreadOptions>,
}

impl Default for Solver {
    fn default() -> Self {
        Solver::AUTO
    }
}

impl Solver {
    pub const LIST: Self = Self {
        strategy: List,
        precision: Precision::HIGH,
        multithreading: Some(MultithreadOptions {
            par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
        }),
    };

    pub const TREE: Self = Self {
        strategy: Tree,
        precision: Precision::HIGH,
        multithreading: Some(MultithreadOptions {
            par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
        }),
    };

    pub const FRAG: Self = Self {
        strategy: Frag,
        precision: Precision::HIGH,
        multithreading: Some(MultithreadOptions {
            par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
        }),
    };

    pub const AUTO: Self = Self {
        strategy: Auto,
        precision: Precision::HIGH,
        multithreading: Some(MultithreadOptions {
            par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
        }),
    };

    const MAX_SPLIT_LIST_COUNT: usize = 4_000;
    const MIN_FRAGMENT_COUNT: usize = 16_000;
    const MAX_FILL_LIST_COUNT: usize = 8_000;

    pub fn with_precision(precision: Precision) -> Self {
        Self {
            strategy: Auto,
            precision,
            multithreading: Some(MultithreadOptions {
                par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
            }),
        }
    }

    pub fn with_strategy_and_precision(strategy: Strategy, precision: Precision) -> Self {
        Self {
            strategy,
            precision,
            multithreading: Some(MultithreadOptions {
                par_sort_min_size: MultithreadOptions::DEFAULT_PAR_SORT_MIN_SIZE,
            }),
        }
    }

    pub(crate) fn is_list_split<C: Send, D: Send, I: IntNumber>(
        &self,
        segments: &[Segment<C, I, D>],
    ) -> bool {
        match self.strategy {
            List => true,
            Tree | Frag => false,
            Auto => segments.len() < Self::MAX_SPLIT_LIST_COUNT,
        }
    }

    pub(crate) fn is_fragmentation_required<C: Send, D: Send, I: IntNumber>(
        &self,
        segments: &[Segment<C, I, D>],
    ) -> bool {
        segments.len() > Self::MIN_FRAGMENT_COUNT || self.strategy == Frag
    }

    pub(crate) fn is_list_fill<C: Send, D: Send, I: IntNumber>(&self, segments: &[Segment<C, I, D>]) -> bool {
        match self.strategy {
            List => true,
            Tree | Frag => false,
            Auto => segments.len() < Self::MAX_FILL_LIST_COUNT,
        }
    }

    #[inline(always)]
    pub(crate) fn is_parallel_sort_allowed(&self) -> bool {
        self.multithreading.is_some()
    }
}
