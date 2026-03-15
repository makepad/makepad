use core::cmp::PartialOrd;
use core::mem;
use core::ops::Deref;

/// A pair of elements sorted in increasing order.
///
/// This structure guarantees that the first element is always less than or equal to
/// the second element. It is useful for representing edges, connections, or any
/// unordered pair where you want canonical ordering (e.g., ensuring that edge (A, B)
/// and edge (B, A) are treated as the same edge).
///
/// The sorted pair implements `Deref` to `(T, T)` for convenient access to the elements.
///
/// # Examples
///
/// ## Creating a Sorted Pair
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::utils::SortedPair;
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::utils::SortedPair;
///
/// // Create a pair - the elements will be sorted automatically
/// let pair1 = SortedPair::new(5, 2);
/// let pair2 = SortedPair::new(2, 5);
///
/// // Both pairs are equal because they contain the same sorted elements
/// assert_eq!(pair1, pair2);
///
/// // Access elements via dereferencing
/// assert_eq!(pair1.0, 2);
/// assert_eq!(pair1.1, 5);
/// # }
/// # }
/// ```
///
/// ## Using as HashMap Keys
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::utils::SortedPair;
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::utils::SortedPair;
/// use std::collections::HashMap;
///
/// let mut edge_weights = HashMap::new();
///
/// // These represent the same edge, so they'll map to the same entry
/// edge_weights.insert(SortedPair::new(1, 3), 10.0);
/// edge_weights.insert(SortedPair::new(3, 1), 20.0); // Overwrites previous
///
/// assert_eq!(edge_weights.len(), 1);
/// assert_eq!(edge_weights.get(&SortedPair::new(1, 3)), Some(&20.0));
/// # }
/// # }
/// ```
///
/// ## Representing Graph Edges
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::utils::SortedPair;
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::utils::SortedPair;
///
/// // Represent undirected edges in a graph
/// let edges = vec![
///     SortedPair::new(0, 1),  // Edge between vertices 0 and 1
///     SortedPair::new(1, 2),  // Edge between vertices 1 and 2
///     SortedPair::new(2, 0),  // Edge between vertices 2 and 0
/// ];
///
/// // Check if a specific edge exists (order doesn't matter)
/// let query_edge = SortedPair::new(2, 1);
/// assert!(edges.contains(&query_edge));
/// # }
/// # }
/// ```
///
/// ## Ordering
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::utils::SortedPair;
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::utils::SortedPair;
///
/// let pair1 = SortedPair::new(1, 5);
/// let pair2 = SortedPair::new(2, 3);
/// let pair3 = SortedPair::new(1, 6);
///
/// // Pairs are compared lexicographically (first element, then second)
/// assert!(pair1 < pair2);  // (1, 5) < (2, 3)
/// assert!(pair1 < pair3);  // (1, 5) < (1, 6)
/// # }
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct SortedPair<T: PartialOrd>([T; 2]);

impl<T: PartialOrd> SortedPair<T> {
    /// Sorts two elements in increasing order into a new pair.
    ///
    /// # Arguments
    ///
    /// * `element1` - The first element
    /// * `element2` - The second element
    ///
    /// # Returns
    ///
    /// A `SortedPair` where the smaller element comes first.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::utils::SortedPair;
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::utils::SortedPair;
    ///
    /// let pair = SortedPair::new(10, 5);
    ///
    /// // Elements are automatically sorted
    /// assert_eq!(pair.0, 5);
    /// assert_eq!(pair.1, 10);
    /// # }
    /// # }
    /// ```
    pub fn new(element1: T, element2: T) -> Self {
        if element1 > element2 {
            SortedPair([element2, element1])
        } else {
            SortedPair([element1, element2])
        }
    }
}

impl<T: PartialOrd> Deref for SortedPair<T> {
    type Target = (T, T);

    fn deref(&self) -> &(T, T) {
        unsafe { mem::transmute(self) }
    }
}

// TODO: can we avoid these manual impls of Hash/PartialEq/Eq for the archived types?
#[cfg(feature = "rkyv")]
impl<T: rkyv::Archive + PartialOrd> core::hash::Hash for ArchivedSortedPair<T>
where
    [<T as rkyv::Archive>::Archived; 2]: core::hash::Hash,
{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

#[cfg(feature = "rkyv")]
impl<T: rkyv::Archive + PartialOrd> PartialEq for ArchivedSortedPair<T>
where
    [<T as rkyv::Archive>::Archived; 2]: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

#[cfg(feature = "rkyv")]
impl<T: rkyv::Archive + PartialOrd> Eq for ArchivedSortedPair<T> where
    [<T as rkyv::Archive>::Archived; 2]: Eq
{
}
