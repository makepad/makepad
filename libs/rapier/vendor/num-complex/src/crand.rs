//! Rand implementations for complex numbers

use crate::Complex;
use num_traits::Num;
use rand::distributions::Standard;
use rand::prelude::*;

impl<T> Distribution<Complex<T>> for Standard
where
    T: Num + Clone,
    Standard: Distribution<T>,
{
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Complex<T> {
        Complex::new(self.sample(rng), self.sample(rng))
    }
}

/// A generic random value distribution for complex numbers.
#[derive(Clone, Copy, Debug)]
pub struct ComplexDistribution<Re, Im = Re> {
    re: Re,
    im: Im,
}

impl<Re, Im> ComplexDistribution<Re, Im> {
    /// Creates a complex distribution from independent
    /// distributions of the real and imaginary parts.
    pub fn new(re: Re, im: Im) -> Self {
        ComplexDistribution { re, im }
    }
}

impl<T, Re, Im> Distribution<Complex<T>> for ComplexDistribution<Re, Im>
where
    T: Num + Clone,
    Re: Distribution<T>,
    Im: Distribution<T>,
{
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Complex<T> {
        Complex::new(self.re.sample(rng), self.im.sample(rng))
    }
}
