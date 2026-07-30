use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;

pub trait IntArea<P: FloatPointCompatible, I: IntNumber> {
    /// The area of the `Path`.
    /// - Returns: A positive double area if path is counter-clockwise and negative double area otherwise.
    fn unsafe_int_area(&self, adapter: &FloatPointAdapter<P, I>) -> I::Wide;
}

impl<P: FloatPointCompatible, I: IntNumber> IntArea<P, I> for [P] {
    fn unsafe_int_area(&self, adapter: &FloatPointAdapter<P, I>) -> I::Wide {
        let n = self.len();
        let mut p0 = adapter.float_to_int(&self[n - 1]);
        let mut area = I::Wide::ZERO;

        for pi in self.iter() {
            let p1 = adapter.float_to_int(pi);
            let a = p0.x.wide().wrapping_mul(p1.y.wide());
            let b = p0.y.wide().wrapping_mul(p1.x.wide());
            area = area.wrapping_add(a).wrapping_sub(b);
            p0 = p1;
        }

        area
    }
}

#[cfg(test)]
mod tests {
    use crate::float::int_area::IntArea;
    use crate::path;
    use i_float::adapter::FloatPointAdapter;

    #[test]
    fn test_0() {
        let square = path![[-1f32, -1f32], [1f32, -1f32], [1f32, 1f32], [-1f32, 1f32],];
        let adapter = FloatPointAdapter::<_, i32>::with_iter(square.iter());

        let area = square.unsafe_int_area(&adapter);
        assert!(area > 0i64);
    }
}
