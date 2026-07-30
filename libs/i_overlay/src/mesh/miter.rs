use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

pub(super) struct Miter;

pub(super) enum SharpMiter<I: IntNumber> {
    Degenerate,
    AB(IntPoint<I>, IntPoint<I>),
    AcB(IntPoint<I>, IntPoint<I>, IntPoint<I>),
}

impl Miter {
    #[inline]
    pub(super) fn sharp<P: FloatPointCompatible, I: IntNumber>(
        pa: P,
        pb: P,
        va: P,
        vb: P,
        adapter: &FloatPointAdapter<P, I>,
    ) -> SharpMiter<I> {
        let ia = adapter.float_to_int(&pa);
        let ib = adapter.float_to_int(&pb);

        if ia == ib {
            return SharpMiter::Degenerate;
        }

        let c = Self::peak(pa, pb, va, vb);

        let ic = adapter.float_to_int(&c);

        if ia == ic || ib == ic {
            SharpMiter::AB(ia, ib)
        } else {
            SharpMiter::AcB(ia, ic, ib)
        }
    }

    #[inline]
    pub(super) fn peak<P: FloatPointCompatible>(pa: P, pb: P, va: P, vb: P) -> P {
        let pax = pa.x();
        let pay = pa.y();
        let pbx = pb.x();
        let pby = pb.y();
        let vax = va.x();
        let vay = va.y();
        let vbx = vb.x();
        let vby = vb.y();

        let xx = vax + vbx;
        let yy = vay + vby;

        let k = if xx.abs() > yy.abs() {
            (pbx - pax) / xx
        } else {
            (pby - pay) / yy
        };

        let x = pax + k * vax;
        let y = pay + k * vay;

        P::from_xy(x, y)
    }
}
