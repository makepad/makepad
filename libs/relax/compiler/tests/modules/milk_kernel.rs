// Identical numerical source in the host, trusted module and sandbox twins.
pub struct MilkState {
    vars: [f64; 64],
    rng: u64,
    mega: Vec<f64>,
}

pub fn new_state() -> Box<MilkState> {
    Box::new(MilkState { vars: [0.25; 64], rng: 7, mega: vec![0.5; 1 << 20] })
}

#[inline(never)]
pub fn kernel(s: &mut MilkState, input: &[f64], output: &mut [f64], mode: u32) {
    if mode == 0 { return; }
    let mut i = 0;
    while i < output.len() {
        s.rng = s.rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = ((s.rng >> 32) as usize) & ((1 << 20) - 1);
        let x = input[i] + s.vars[i & 63];
        let y = if mode == 2 { x * 0.125 + 0.2 } else {
            (x.sin() * x.cos() + (x.abs() + 0.1).powf(1.3)).sqrt()
        };
        let z = y + s.mega[j] * 0.125;
        s.mega[j] = z * 0.25;
        s.vars[i & 63] = z * 0.125;
        output[i] = z;
        i += 1;
    }
}

pub fn batch(s: &mut MilkState, input: &[f64], output: &mut [f64], mode: u32) -> u64 {
    let start = test_crate::tick();
    kernel(s, input, output, mode);
    test_crate::tick() - start
}

pub fn touch(s: &mut MilkState) -> f64 { s.vars[0] }
pub fn empty() -> f64 { 0.25 }
