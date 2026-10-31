#[inline(never)] fn mx(a: f64) -> f64 { a.max(0.0) }
#[inline(never)] fn mn(a: f64) -> f64 { a.min(1.5) }
#[inline(never)] fn mxf(a: f32) -> f32 { (-2.0f32).max(a) }
#[inline(never)] fn mnf(a: f32) -> f32 { a.min(-0.5) }
#[test] fn t() {
    for x in [f64::NAN, -1.0, 0.0, -0.0, 2.0, 1.5, f64::INFINITY, f64::NEG_INFINITY] {
        println!("{:?} {:?} {:?} {:?} {:?}", x, mx(x), mn(x), mxf(x as f32), mnf(x as f32));
    }
}
