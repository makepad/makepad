use crate::Measure;

pub trait Metric<T> {
    type Measure: Measure;

    fn measure(item: &T) -> Self::Measure;
}

impl<T> Metric<T> for () {
    type Measure = ();

    fn measure(_item: &T) -> Self::Measure {
        ()
    }
}

pub struct DefaultMetric;

impl<T> Metric<T> for DefaultMetric {
    type Measure = ();

    fn measure(_item: &T) -> Self::Measure {
        ()
    }
}
