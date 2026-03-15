// "Complete Interval Arithmetic and its Implementation on the Computer"
// Ulrich W. Kulisch
use alloc::{vec, vec::Vec};
use core::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};
use num::{One, Zero};
use num_traits::AsPrimitive;
use simba::scalar::RealField;

/// A derivable valued function which can be bounded on intervals.
pub trait IntervalFunction<T> {
    /// Evaluate the function at `t`.
    fn eval(&self, t: T) -> T;
    /// Bounds all the values of this function on the interval `t`.
    fn eval_interval(&self, t: Interval<T>) -> Interval<T>;
    /// Bounds all the values of the gradient of this function on the interval `t`.
    fn eval_interval_gradient(&self, t: Interval<T>) -> Interval<T>;
}

/// Execute the Interval Newton Method to isolate all the roots of the given nonlinear function.
///
/// The results are stored in `results`. The `candidate` buffer is just a workspace buffer used
/// to avoid allocations.
pub fn find_root_intervals_to<T: Copy + PartialOrd + RealField>(
    function: &impl IntervalFunction<T>,
    init: Interval<T>,
    min_interval_width: T,
    min_image_width: T,
    max_recursions: usize,
    results: &mut Vec<Interval<T>>,
    candidates: &mut Vec<(Interval<T>, usize)>,
) where
    f64: AsPrimitive<T>,
{
    candidates.clear();

    let push_candidate = |candidate,
                          recursion,
                          results: &mut Vec<Interval<T>>,
                          candidates: &mut Vec<(Interval<T>, usize)>| {
        let candidate_image = function.eval_interval(candidate);
        let is_small_range =
            candidate.width() < min_interval_width || candidate_image.width() < min_image_width;

        if candidate_image.contains(T::zero()) {
            if recursion == max_recursions || is_small_range {
                results.push(candidate);
            } else {
                candidates.push((candidate, recursion + 1));
            }
        } else if is_small_range
            && function.eval(candidate.midpoint()).abs() < T::default_epsilon().sqrt()
        {
            // If we have a small range, and we are close to zero,
            // consider that we reached zero.
            results.push(candidate);
        }
    };

    push_candidate(init, 0, results, candidates);

    while let Some((candidate, recursion)) = candidates.pop() {
        // println!(
        //     "Candidate: {:?}, recursion: {}, image: {:?}",
        //     candidate,
        //     recursion,
        //     function.eval_interval(candidate)
        // );

        // NOTE: we don't check the max_recursions at the beginning of the
        //       loop here because that would make us loose the candidate
        //       we just popped.
        let mid = candidate.midpoint();
        let f_mid = function.eval(mid);
        let gradient = function.eval_interval_gradient(candidate);
        let (shift1, shift2) = Interval(f_mid, f_mid) / gradient;

        let new_candidates = [
            (Interval(mid, mid) - shift1).intersect(candidate),
            shift2.and_then(|shift2| (Interval(mid, mid) - shift2).intersect(candidate)),
        ];

        let prev_width = candidate.width();

        for new_candidate in new_candidates.iter().flatten() {
            if new_candidate.width() > prev_width * 0.75.as_() {
                // If the new candidate range is still quite big compared to
                // new candidate, split it to accelerate the search.
                let [a, b] = new_candidate.split();
                push_candidate(a, recursion, results, candidates);
                push_candidate(b, recursion, results, candidates);
            } else {
                push_candidate(*new_candidate, recursion, results, candidates);
            }
        }
    }
}

/// Execute the Interval Newton Method to isolate all the roots of the given nonlinear function.
pub fn find_root_intervals<T: Copy + PartialOrd + RealField>(
    function: &impl IntervalFunction<T>,
    init: Interval<T>,
    min_interval_width: T,
    min_image_width: T,
    max_recursions: usize,
) -> Vec<Interval<T>>
where
    f64: AsPrimitive<T>,
{
    let mut results = vec![];
    let mut candidates = vec![];
    find_root_intervals_to(
        function,
        init,
        min_interval_width,
        min_image_width,
        max_recursions,
        &mut results,
        &mut candidates,
    );
    results
}

/// An interval implementing interval arithmetic.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Interval<T>(pub T, pub T);

impl<T> Interval<T> {
    /// Create the interval `[min(a, b), max(a, b)]`.
    #[must_use]
    pub fn sort(a: T, b: T) -> Self
    where
        T: PartialOrd,
    {
        if a < b {
            Self(a, b)
        } else {
            Self(b, a)
        }
    }

    /// Create the interval `[e, e]` (single value).
    #[must_use]
    pub fn splat(e: T) -> Self
    where
        T: Clone,
    {
        Self(e.clone(), e)
    }

    /// Does this interval contain the given value?
    #[must_use]
    pub fn contains(&self, t: T) -> bool
    where
        T: PartialOrd<T>,
    {
        self.0 <= t && self.1 >= t
    }

    /// The width of this interval.
    #[must_use]
    pub fn width(self) -> T::Output
    where
        T: Sub<T>,
    {
        self.1 - self.0
    }

    /// The average of the two interval endpoints.
    #[must_use]
    pub fn midpoint(self) -> T
    where
        T: Copy + Add<T, Output = T> + Div<T, Output = T> + One,
    {
        let two = T::one() + T::one();
        (self.0 + self.1) / two
    }

    /// Splits this interval at its midpoint.
    #[must_use]
    pub fn split(self) -> [Self; 2]
    where
        T: Copy + Add<T, Output = T> + Div<T, Output = T> + One,
    {
        let mid = self.midpoint();
        [Interval(self.0, mid), Interval(mid, self.1)]
    }

    /// Computes a new interval that contains both `self` and `t`.
    #[must_use]
    pub fn enclose(self, t: T) -> Self
    where
        T: PartialOrd,
    {
        if t < self.0 {
            Interval(t, self.1)
        } else if t > self.1 {
            Interval(self.0, t)
        } else {
            self
        }
    }

    /// Computes the intersection between two intervals.
    ///
    /// Returns `None` if the intervals are disjoint.
    #[must_use]
    pub fn intersect(self, rhs: Self) -> Option<Self>
    where
        T: PartialOrd + RealField,
    {
        let result = Interval(self.0.max(rhs.0), self.1.min(rhs.1));

        if result.0 > result.1 {
            // The range is invalid if there is no intersection.
            None
        } else {
            Some(result)
        }
    }

    /// Bounds the image of the`sin` and `cos` functions on this interval.
    #[must_use]
    pub fn sin_cos(self) -> (Self, Self)
    where
        T: Copy + RealField,
        f64: AsPrimitive<T>,
    {
        (self.sin(), self.cos())
    }

    /// Bounds the image of the sinus function on this interval.
    #[must_use]
    pub fn sin(self) -> Self
    where
        T: Copy + RealField,
        f64: AsPrimitive<T>,
    {
        let two_pi = T::two_pi();
        let one = T::one();
        let pi = T::pi();
        let frac_pi_2 = T::frac_pi_2();

        if self.width() >= two_pi {
            Interval(one * (-1.0).as_(), one)
        } else {
            let sin0 = self.0.sin();
            let sin1 = self.1.sin();
            let mut result = Interval::sort(sin0, sin1);

            let orig = (self.0 / two_pi).floor() * two_pi;
            let crit = [orig + frac_pi_2, orig + pi + frac_pi_2];
            let crit_vals = [one, one * (-1.0).as_()];

            for i in 0..2 {
                if self.contains(crit[i]) || self.contains(crit[i] + two_pi) {
                    result = result.enclose(crit_vals[i])
                }
            }

            result
        }
    }

    /// Bounds the image of the cosinus function on this interval.
    #[must_use]
    pub fn cos(self) -> Self
    where
        T: Copy + RealField,
        f64: AsPrimitive<T>,
    {
        let two_pi = T::two_pi();
        let one = T::one();
        let pi = T::pi();

        if self.width() >= two_pi {
            Interval(one * (-1.0).as_(), one)
        } else {
            let cos0 = self.0.cos();
            let cos1 = self.1.cos();
            let mut result = Interval::sort(cos0, cos1);

            let orig = (self.0 / two_pi).floor() * two_pi;
            let crit = [orig, orig + pi];
            let crit_vals = [one, one * (-1.0).as_()];

            for i in 0..2 {
                if self.contains(crit[i]) || self.contains(crit[i] + two_pi) {
                    result = result.enclose(crit_vals[i])
                }
            }

            result
        }
    }
}

impl<T: Add<T> + Copy> Add<T> for Interval<T> {
    type Output = Interval<<T as Add<T>>::Output>;

    fn add(self, rhs: T) -> Self::Output {
        Interval(self.0 + rhs, self.1 + rhs)
    }
}

impl<T: Add<T>> Add<Interval<T>> for Interval<T> {
    type Output = Interval<<T as Add<T>>::Output>;

    fn add(self, rhs: Self) -> Self::Output {
        Interval(self.0 + rhs.0, self.1 + rhs.1)
    }
}

impl<T: Sub<T> + Copy> Sub<T> for Interval<T> {
    type Output = Interval<<T as Sub<T>>::Output>;

    fn sub(self, rhs: T) -> Self::Output {
        Interval(self.0 - rhs, self.1 - rhs)
    }
}

impl<T: Sub<T> + Copy> Sub<Interval<T>> for Interval<T> {
    type Output = Interval<<T as Sub<T>>::Output>;

    fn sub(self, rhs: Self) -> Self::Output {
        Interval(self.0 - rhs.1, self.1 - rhs.0)
    }
}

impl<T: Neg> Neg for Interval<T> {
    type Output = Interval<T::Output>;

    fn neg(self) -> Self::Output {
        Interval(-self.1, -self.0)
    }
}

impl<T: Mul<T>> Mul<T> for Interval<T>
where
    T: Copy + PartialOrd + Zero,
{
    type Output = Interval<<T as Mul<T>>::Output>;

    fn mul(self, rhs: T) -> Self::Output {
        if rhs < T::zero() {
            Interval(self.1 * rhs, self.0 * rhs)
        } else {
            Interval(self.0 * rhs, self.1 * rhs)
        }
    }
}

impl<T: Mul<T>> Mul<Interval<T>> for Interval<T>
where
    T: Copy + PartialOrd + Zero + RealField,
    <T as Mul<T>>::Output: PartialOrd + RealField,
{
    type Output = Interval<<T as Mul<T>>::Output>;

    fn mul(self, rhs: Self) -> Self::Output {
        let Interval(a1, a2) = self;
        let Interval(b1, b2) = rhs;

        if a2 <= T::zero() {
            if b2 <= T::zero() {
                Interval(a2 * b2, a1 * b1)
            } else if b1 < T::zero() {
                Interval(a1 * b2, a1 * b1)
            } else {
                Interval(a1 * b2, a2 * b1)
            }
        } else if a1 < T::zero() {
            if b2 <= T::zero() {
                Interval(a2 * b1, a1 * b1)
            } else if b1 < T::zero() {
                Interval((a1 * b2).min(b2 * b1), (a1 * b1).max(a2 * b2))
            } else {
                Interval(a1 * b2, a2 * b2)
            }
        } else if b2 <= T::zero() {
            Interval(a2 * b1, a1 * b2)
        } else if b1 < T::zero() {
            Interval(a2 * b1, a2 * b2)
        } else {
            Interval(a1 * b1, a2 * b2)
        }
    }
}

impl<T: Div<T>> Div<Interval<T>> for Interval<T>
where
    T: Copy + One + Zero + PartialOrd + 'static,
    <T as Div<T>>::Output: PartialOrd + RealField,
    f64: AsPrimitive<T>,
{
    type Output = (
        Interval<<T as Div<T>>::Output>,
        Option<Interval<<T as Div<T>>::Output>>,
    );

    fn div(self, rhs: Self) -> Self::Output {
        let infinity = T::one() / T::zero();
        let neg_one: T = (-1.0).as_();
        let neg_infinity = neg_one / T::zero();

        let Interval(a1, a2) = self;
        let Interval(b1, b2) = rhs;

        if b1 <= T::zero() && b2 >= T::zero() {
            // rhs contains T::zero() so we my have to return
            // two intervals.
            if a2 < T::zero() {
                if b2 == T::zero() {
                    (Interval(a2 / b1, infinity), None)
                } else if b1 != T::zero() {
                    (
                        Interval(neg_infinity, a2 / b2),
                        Some(Interval(a2 / b1, infinity)),
                    )
                } else {
                    (Interval(neg_infinity, a2 / b2), None)
                }
            } else if a1 <= T::zero() {
                (Interval(neg_infinity, infinity), None)
            } else if b2 == T::zero() {
                (Interval(neg_infinity, a1 / b1), None)
            } else if b1 != T::zero() {
                (
                    Interval(neg_infinity, a1 / b1),
                    Some(Interval(a1 / b2, infinity)),
                )
            } else {
                (Interval(a1 / b2, infinity), None)
            }
        } else if a2 <= T::zero() {
            if b2 < T::zero() {
                (Interval(a2 / b1, a1 / b2), None)
            } else {
                (Interval(a1 / b1, a2 / b2), None)
            }
        } else if a1 < T::zero() {
            if b2 < T::zero() {
                (Interval(a2 / b2, a1 / b2), None)
            } else {
                (Interval(a1 / b1, a2 / b1), None)
            }
        } else if b2 < T::zero() {
            (Interval(a2 / b2, a1 / b1), None)
        } else {
            (Interval(a1 / b2, a2 / b1), None)
        }
    }
}

impl<T: Copy + Add<T, Output = T>> AddAssign<Interval<T>> for Interval<T> {
    fn add_assign(&mut self, rhs: Interval<T>) {
        *self = *self + rhs;
    }
}

impl<T: Copy + Sub<T, Output = T>> SubAssign<Interval<T>> for Interval<T> {
    fn sub_assign(&mut self, rhs: Interval<T>) {
        *self = *self - rhs;
    }
}

impl<T: Mul<T, Output = T>> MulAssign<Interval<T>> for Interval<T>
where
    T: Copy + PartialOrd + Zero + RealField,
    <T as Mul<T>>::Output: PartialOrd + RealField,
{
    fn mul_assign(&mut self, rhs: Interval<T>) {
        *self = *self * rhs;
    }
}

impl<T: Zero + Add<T>> Zero for Interval<T> {
    fn zero() -> Self {
        Self(T::zero(), T::zero())
    }

    fn is_zero(&self) -> bool {
        self.0.is_zero() && self.1.is_zero()
    }
}

impl<T: One + Mul<T>> One for Interval<T>
where
    Interval<T>: Mul<Interval<T>, Output = Interval<T>>,
{
    fn one() -> Self {
        Self(T::one(), T::one())
    }
}

#[cfg(test)]
mod test {
    use super::{Interval, IntervalFunction};

    #[test]
    fn roots_sin() {
        struct Sin;

        impl IntervalFunction<f32> for Sin {
            fn eval(&self, t: f32) -> f32 {
                t.sin()
            }

            fn eval_interval(&self, t: Interval<f32>) -> Interval<f32> {
                t.sin()
            }

            fn eval_interval_gradient(&self, t: Interval<f32>) -> Interval<f32> {
                t.cos()
            }
        }

        let function = Sin;
        let two_pi = core::f32::consts::TAU;
        let roots =
            super::find_root_intervals(&function, Interval(0.0, two_pi), 1.0e-5, 1.0e-5, 100);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn interval_sin_cos() {
        let a = core::f32::consts::PI / 6.0;
        let b = core::f32::consts::FRAC_PI_2 + core::f32::consts::PI / 6.0;
        let c = core::f32::consts::PI + core::f32::consts::PI / 6.0;
        let d = core::f32::consts::PI + core::f32::consts::FRAC_PI_2 + core::f32::consts::PI / 6.0;
        let two_pi = core::f32::consts::TAU;
        let shifts = [0.0, two_pi * 100.0, -two_pi * 100.0];

        for shift in shifts.iter() {
            // Test sinus.
            assert_eq!(
                Interval(a + *shift, b + *shift).sin(),
                Interval((a + *shift).sin(), 1.0)
            );
            assert_eq!(
                Interval(a + *shift, c + *shift).sin(),
                Interval((c + *shift).sin(), 1.0)
            );
            assert_eq!(Interval(a + *shift, d + *shift).sin(), Interval(-1.0, 1.0));

            // Test cosinus.
            assert_eq!(
                Interval(a + *shift, b + *shift).cos(),
                Interval((b + *shift).cos(), (a + *shift).cos())
            );
            assert_eq!(
                Interval(a + *shift, c + *shift).cos(),
                Interval(-1.0, (a + *shift).cos())
            );
            assert_eq!(
                Interval(a + *shift, d + *shift).cos(),
                Interval(-1.0, (a + *shift).cos())
            );
        }
    }
}
