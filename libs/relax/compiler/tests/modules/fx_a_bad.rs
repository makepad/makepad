use test_crate::{host_scale, FxIn};

struct Acc {
    sum: f32,
}

fn mix(a: f32, b: f32) -> f32 {
    let acc = Acc { sum: a + b };
    acc.sum
}

pub fn frame(i: &FxIn) -> f32 {
    mix(host_scale(i.t), "kick")
}
