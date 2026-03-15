#[cfg(feature = "dim3")]
#[inline]
/// Sorts a set of two values in increasing order.
///
/// This is a simple utility function that takes two values and returns them
/// as a tuple in sorted order (smallest first). This function is only available
/// in 3D mode (`dim3` feature).
///
/// # Arguments
///
/// * `a` - The first value
/// * `b` - The second value
///
/// # Returns
///
/// A tuple `(min, max)` where `min` is the smaller value and `max` is the larger value.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::utils::sort2;
///
/// let (min, max) = sort2(5.0, 2.0);
/// assert_eq!(min, 2.0);
/// assert_eq!(max, 5.0);
///
/// // Already sorted values remain in the same order
/// let (min, max) = sort2(1.0, 3.0);
/// assert_eq!(min, 1.0);
/// assert_eq!(max, 3.0);
/// # }
/// ```
pub fn sort2<T: PartialOrd + Copy>(a: T, b: T) -> (T, T) {
    if a > b {
        (b, a)
    } else {
        (a, b)
    }
}

/// Sorts a set of three values in increasing order.
///
/// This function efficiently sorts three values using a minimal number of comparisons
/// (between 2 and 3 comparisons). The values are passed by reference and references
/// are returned, making this efficient for larger types.
///
/// # Arguments
///
/// * `a` - Reference to the first value
/// * `b` - Reference to the second value
/// * `c` - Reference to the third value
///
/// # Returns
///
/// A tuple of three references `(&min, &mid, &max)` in sorted order.
///
/// # Examples
///
/// ## Basic Sorting
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::utils::sort3;
///
/// let x = 5.0;
/// let y = 2.0;
/// let z = 8.0;
///
/// let (min, mid, max) = sort3(&x, &y, &z);
///
/// assert_eq!(*min, 2.0);
/// assert_eq!(*mid, 5.0);
/// assert_eq!(*max, 8.0);
/// # }
/// ```
///
/// ## All Permutations Work
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::utils::sort3;
///
/// let a = 1.0;
/// let b = 2.0;
/// let c = 3.0;
///
/// // All orderings produce the same sorted result
/// let (min1, mid1, max1) = sort3(&a, &b, &c);
/// let (min2, mid2, max2) = sort3(&c, &a, &b);
/// let (min3, mid3, max3) = sort3(&b, &c, &a);
///
/// assert_eq!(*min1, *min2);
/// assert_eq!(*min1, *min3);
/// assert_eq!(*max1, *max2);
/// assert_eq!(*max1, *max3);
/// # }
/// ```
///
/// ## Finding Median of Three
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::utils::sort3;
///
/// let values = [10.0, 5.0, 7.0];
/// let (_, median, _) = sort3(&values[0], &values[1], &values[2]);
///
/// // The middle value is the median
/// assert_eq!(*median, 7.0);
/// # }
/// ```
///
/// ## Works with Integers
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::utils::sort3;
///
/// let x = 42;
/// let y = 17;
/// let z = 99;
///
/// let (min, mid, max) = sort3(&x, &y, &z);
///
/// assert_eq!(*min, 17);
/// assert_eq!(*mid, 42);
/// assert_eq!(*max, 99);
/// # }
/// ```
#[inline]
pub fn sort3<'a, T: PartialOrd + Copy>(a: &'a T, b: &'a T, c: &'a T) -> (&'a T, &'a T, &'a T) {
    let a_b = *a > *b;
    let a_c = *a > *c;
    let b_c = *b > *c;

    let sa;
    let sb;
    let sc;

    // Sort the three values.
    if a_b {
        // a > b
        if a_c {
            // a > c
            sc = a;

            if b_c {
                // b > c
                sa = c;
                sb = b;
            } else {
                // b <= c
                sa = b;
                sb = c;
            }
        } else {
            // a <= c
            sa = b;
            sb = a;
            sc = c;
        }
    } else {
        // a < b
        if !a_c {
            // a <= c
            sa = a;

            if b_c {
                // b > c
                sb = c;
                sc = b;
            } else {
                sb = b;
                sc = c;
            }
        } else {
            // a > c
            sa = c;
            sb = a;
            sc = b;
        }
    }

    (sa, sb, sc)
}
