//! A small room around the instrument: an eight-line feedback delay network
//! at low level. The piano must stand without it; it only places it.

const LINES: usize = 8;
/// Line lengths in samples at 48 kHz (mutually prime, 21..60 ms).
const LENGTHS: [usize; LINES] = [1031, 1327, 1523, 1871, 2053, 2311, 2579, 2903];

#[derive(Clone)]
pub struct Room {
    lines: Vec<Vec<f32>>,
    pos: [usize; LINES],
    damp: [f32; LINES],
    gain: [f32; LINES],
    damping: f32,
    pub mix: f32,
}

impl Room {
    pub fn new(fs: f64) -> Self {
        let mut gain = [0.0; LINES];
        let lines = LENGTHS
            .iter()
            .enumerate()
            .map(|(i, &len)| {
                let len = ((len as f64 * fs / 48_000.0).round() as usize).max(8);
                // 0.9 s to fall 60 dB at low frequencies
                gain[i] = 10f64.powf(-3.0 * len as f64 / (0.9 * fs)) as f32;
                vec![0.0; len]
            })
            .collect();
        Room { lines, pos: [0; LINES], damp: [0.0; LINES], gain, damping: (-std::f64::consts::TAU * 5200.0 / fs).exp() as f32, mix: 0.07 }
    }

    pub fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.damp = [0.0; LINES];
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if self.mix <= 0.0 {
            return;
        }
        for (l, r) in l.iter_mut().zip(r.iter_mut()) {
            let mut out = [0.0f32; LINES];
            let mut sum = 0.0;
            for i in 0..LINES {
                let x = self.lines[i][self.pos[i]];
                self.damp[i] = x + self.damping * (self.damp[i] - x);
                out[i] = self.damp[i] * self.gain[i];
                sum += out[i];
            }
            let wet_l = out[0] - out[2] + out[4] - out[6];
            let wet_r = out[1] - out[3] + out[5] - out[7];
            // Householder reflection: every line feeds every other.
            let spread = sum * (2.0 / LINES as f32);
            for i in 0..LINES {
                let input = if i % 2 == 0 { *l } else { *r };
                self.lines[i][self.pos[i]] = out[i] - spread + 0.25 * input;
                self.pos[i] += 1;
                if self.pos[i] == self.lines[i].len() {
                    self.pos[i] = 0;
                }
            }
            *l += self.mix * wet_l;
            *r += self.mix * wet_r;
        }
    }
}
