use crate::math::Real;

/// Computes the median of a set of values.
///
/// The median is the middle value when the data is sorted. For an even number of values,
/// it returns the average of the two middle values. This function modifies the input slice
/// by sorting it in-place.
///
/// # Arguments
///
/// * `vals` - A mutable slice of values. Must contain at least one value. The slice will be
///   sorted in-place as a side effect.
///
/// # Returns
///
/// The median value as a `Real`.
///
/// # Panics
///
/// Panics if the input slice is empty.
///
/// # Examples
///
/// ## Odd Number of Values
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::math::Real;
/// use parry2d::utils::median;
///
/// let mut values = vec![5.0, 1.0, 3.0, 9.0, 2.0];
/// let med = median(&mut values);
///
/// // With 5 values, the median is the 3rd value when sorted: [1, 2, 3, 5, 9]
/// assert_eq!(med, 3.0);
/// # }
/// ```
///
/// ## Even Number of Values
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::math::Real;
/// use parry2d::utils::median;
///
/// let mut values = vec![1.0, 2.0, 3.0, 4.0];
/// let med = median(&mut values);
///
/// // With 4 values, median is average of middle two: (2 + 3) / 2 = 2.5
/// assert_eq!(med, 2.5);
/// # }
/// ```
///
/// ## Single Value
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::math::Real;
/// use parry2d::utils::median;
///
/// let mut values = vec![42.0];
/// let med = median(&mut values);
///
/// // The median of a single value is the value itself
/// assert_eq!(med, 42.0);
/// # }
/// ```
///
/// ## Values Are Sorted In-Place
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::utils::median;
///
/// let mut values = vec![5.0, 1.0, 3.0];
/// let med = median(&mut values);
///
/// // After calling median, the values are sorted
/// assert_eq!(values, vec![1.0, 3.0, 5.0]);
/// assert_eq!(med, 3.0);
/// # }
/// ```
#[inline]
pub fn median(vals: &mut [Real]) -> Real {
    assert!(
        !vals.is_empty(),
        "Cannot compute the median of zero values."
    );

    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = vals.len();

    if n.is_multiple_of(2) {
        (vals[n / 2 - 1] + vals[n / 2]) / 2.0
    } else {
        vals[n / 2]
    }
}
