use test_crate::FxIn;

fn down(n: u64, pad: [u64; 64]) -> u64 {
    if n == 0 { pad[3] } else { down(n + 1, pad) + 1 }
}

pub fn frame(i: &FxIn) -> f32 {
    down(i.t as u64 + 1, [7; 64]) as f32
}
