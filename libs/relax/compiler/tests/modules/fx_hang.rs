use test_crate::FxIn;

pub fn frame(i: &FxIn) -> f32 {
    let mut x = i.t;
    while x > -1.0 {
        x = x * 1.0000001 + 0.5;
    }
    x
}
