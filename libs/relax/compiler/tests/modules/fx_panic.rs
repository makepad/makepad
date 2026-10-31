use test_crate::FxIn;

pub fn frame(i: &FxIn) -> f32 {
    let v: Vec<f32> = Vec::new();
    v[(i.t * 4.0) as usize]
}
