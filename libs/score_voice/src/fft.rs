//! A small in-place radix-2 FFT (analysis only; never on the audio thread).

pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    /// `n` must be a power of two.
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let (cos, sin) = (0..n / 2)
            .map(|i| {
                let a = -std::f64::consts::TAU * i as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        Fft { n, cos, sin, rev }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Forward transform of (re, im), in place.
    pub fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }

    /// |X|² of a real frame, bins 0..=n/2.
    pub fn power(&self, frame: &[f32], re: &mut Vec<f32>, im: &mut Vec<f32>, out: &mut Vec<f32>) {
        re.clear();
        re.extend_from_slice(&frame[..self.n]);
        im.clear();
        im.resize(self.n, 0.0);
        self.run(re, im);
        out.clear();
        out.extend((0..=self.n / 2).map(|k| re[k] * re[k] + im[k] * im[k]));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sine_lands_in_its_bin() {
        let n = 256;
        let fft = super::Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 10.0 * i as f32 / n as f32).sin()).collect();
        let (mut re, mut im, mut p) = (Vec::new(), Vec::new(), Vec::new());
        fft.power(&x, &mut re, &mut im, &mut p);
        let peak = (0..p.len()).max_by(|a, b| p[*a].partial_cmp(&p[*b]).unwrap()).unwrap();
        assert_eq!(peak, 10);
        assert!((p[10].sqrt() - n as f32 / 2.0).abs() < 0.01 * n as f32);
    }
}
