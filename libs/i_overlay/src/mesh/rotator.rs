use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;

pub(crate) struct Rotator<T: FloatNumber> {
    a_x: T,
    a_y: T,
    b_x: T,
    b_y: T,
}

impl<T: FloatNumber> Rotator<T> {
    #[inline]
    pub(crate) fn new(cs: T, sn: T) -> Self {
        let a_x = cs;
        let a_y = sn;
        let b_x = -a_y;
        let b_y = a_x;

        Self { a_x, a_y, b_x, b_y }
    }

    #[inline]
    pub(crate) fn with_angle(angle: T) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(cos, sin)
    }

    #[inline]
    pub(crate) fn with_vector<P: FloatPointCompatible<Scalar = T>>(v: &P) -> Self {
        Self::new(v.x(), v.y())
    }

    #[inline]
    pub(crate) fn rotate<P: FloatPointCompatible<Scalar = T>>(&self, v: &P) -> P {
        let v_x = v.x();
        let v_y = v.y();
        let x = self.a_x * v_x + self.b_x * v_y;
        let y = self.a_y * v_x + self.b_y * v_y;
        P::from_xy(x, y)
    }
}

#[cfg(test)]
mod tests {
    use crate::mesh::rotator::Rotator;
    use core::f64::consts::PI;

    #[test]
    fn test_ccw_rotate() {
        let deg_45 = 0.25 * PI;
        let rotator = Rotator::with_angle(deg_45);
        let v0 = [1.0, 0.0];
        let v1 = rotator.rotate(&v0);
        let v2 = rotator.rotate(&v1);
        let v3 = rotator.rotate(&v2);

        let i_sqrt2 = 1.0 / 2.0f64.sqrt();

        compare_vecs(v1, [i_sqrt2, i_sqrt2]);
        compare_vecs(v2, [0.0, 1.0]);
        compare_vecs(v3, [-i_sqrt2, i_sqrt2]);
    }

    #[test]
    fn test_cw_rotate() {
        let deg_45 = -0.25 * PI;
        let rotator = Rotator::with_angle(deg_45);
        let v0 = [1.0, 0.0];
        let v1 = rotator.rotate(&v0);
        let v2 = rotator.rotate(&v1);
        let v3 = rotator.rotate(&v2);

        let i_sqrt2 = 1.0 / 2.0f64.sqrt();

        compare_vecs(v1, [i_sqrt2, -i_sqrt2]);
        compare_vecs(v2, [0.0, -1.0]);
        compare_vecs(v3, [-i_sqrt2, -i_sqrt2]);
    }

    fn compare_vecs(v0: [f64; 2], v1: [f64; 2]) {
        assert!((v0[0] - v1[0]).abs() < 0.0001);
        assert!((v0[1] - v1[1]).abs() < 0.0001);
    }
}
