//! Microtuning shared by every layout: a scale (the Scala `.scl` text
//! format: a period and its degrees in cents or ratios), a keyboard mapping
//! (the `.kbm` text format: which MIDI key plays which degree, where the
//! scale starts and which key sounds at which frequency) and the [`Tuning`]
//! they make: a frequency for every note -256..255 (MIDI convention, 69 =
//! A440 in standard tuning) and the tuned note number (fractional) that
//! engines feed their voices. [`library`] is our own set of scales, built
//! from their definitions (equal divisions, just and historical ratios).

use std::sync::Arc;

/// A scale: degrees 1..=n in cents above degree 0; the last is the period
/// (an octave, 1200, for most).
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    pub description: String,
    pub cents: Vec<f64>,
}

/// Lines of a Scala text that are not comments (`!` starts one).
fn data_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim_start().starts_with('!'))
}

/// A pitch line: cents when it has a `.`, else a ratio `n/d` or a whole
/// number `n` (= n/1). Text after the value is ignored.
fn pitch_cents(line: &str) -> Result<f64, String> {
    let word = line.split_whitespace().next().ok_or_else(|| "an empty pitch line".to_string())?;
    if word.contains('.') {
        return word.parse::<f64>().map_err(|_| format!("`{word}` is not a cents value"));
    }
    let (n, d) = match word.split_once('/') {
        Some((n, d)) => (n, d),
        None => (word, "1"),
    };
    let (n, d) = (n.parse::<f64>().map_err(|_| format!("`{word}` is not a ratio"))?, d.parse::<f64>().map_err(|_| format!("`{word}` is not a ratio"))?);
    if n <= 0.0 || d <= 0.0 {
        return Err(format!("`{word}`: a ratio must be positive"));
    }
    Ok(1200.0 * (n / d).log2())
}

impl Scale {
    /// Parse `.scl` text: a description line, the degree count, then one
    /// pitch per degree.
    pub fn parse(text: &str) -> Result<Scale, String> {
        let mut lines = data_lines(text);
        let description = lines.next().ok_or_else(|| "the scale has no description line".to_string())?.trim().to_string();
        let count_line = lines.next().ok_or_else(|| "the scale has no degree count".to_string())?;
        let n: usize = count_line.split_whitespace().next().and_then(|w| w.parse().ok()).ok_or_else(|| format!("`{}` is not a degree count", count_line.trim()))?;
        if n == 0 {
            return Err("a scale needs at least one degree".into());
        }
        let mut cents = Vec::with_capacity(n);
        for line in lines.filter(|l| !l.trim().is_empty()).take(n) {
            cents.push(pitch_cents(line)?);
        }
        if cents.len() != n {
            return Err(format!("the scale says {n} degrees but lists {}", cents.len()));
        }
        if !(cents[n - 1] > 0.0) {
            return Err("the period (last degree) must be above 1/1".into());
        }
        Ok(Scale { description, cents })
    }

    /// `n` equal divisions of `period` cents.
    pub fn equal(description: &str, n: usize, period: f64) -> Scale {
        Scale { description: description.into(), cents: (1..=n).map(|k| period * k as f64 / n as f64).collect() }
    }

    /// Degrees from frequency ratios (n, d), the last the period.
    pub fn ratios(description: &str, r: &[(u32, u32)]) -> Scale {
        Scale { description: description.into(), cents: r.iter().map(|(n, d)| 1200.0 * (*n as f64 / *d as f64).log2()).collect() }
    }

    pub fn len(&self) -> usize {
        self.cents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cents.is_empty()
    }

    pub fn period(&self) -> f64 {
        self.cents[self.cents.len() - 1]
    }

    /// Cents of a degree (any integer: periods repeat).
    pub fn degree_cents(&self, degree: i64) -> f64 {
        let n = self.len() as i64;
        let (p, k) = (degree.div_euclid(n), degree.rem_euclid(n));
        p as f64 * self.period() + if k == 0 { 0.0 } else { self.cents[k as usize - 1] }
    }

    /// The `.scl` text of this scale (cents, six decimals).
    pub fn to_text(&self) -> String {
        let mut out = format!("! {}\n{}\n{}\n!\n", self.description, self.description, self.len());
        for c in &self.cents {
            out.push_str(&format!("{c:.6}\n"));
        }
        out
    }
}

/// A keyboard mapping. `map` empty = every key plays the next degree
/// (linear); else key `middle + k` plays `map[k mod size]` (None: silent)
/// plus `octave_degree` per repeat of the map.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyMap {
    pub first: i32,
    pub last: i32,
    /// The key that plays degree 0.
    pub middle: i32,
    pub ref_key: i32,
    pub ref_hz: f64,
    /// Degrees per map repeat (0: the scale's length).
    pub octave_degree: i32,
    pub map: Vec<Option<i32>>,
}

impl Default for KeyMap {
    /// Degree 0 on middle C, A4 (69) at 440 Hz.
    fn default() -> Self {
        KeyMap { first: 0, last: 127, middle: 60, ref_key: 69, ref_hz: 440.0, octave_degree: 0, map: Vec::new() }
    }
}

impl KeyMap {
    /// Parse `.kbm` text: map size, first key, last key, middle key,
    /// reference key, reference frequency, octave degree, then one entry a
    /// map position (a degree or `x` for an unmapped key).
    pub fn parse(text: &str) -> Result<KeyMap, String> {
        let mut words = data_lines(text).filter(|l| !l.trim().is_empty()).map(|l| l.split_whitespace().next().unwrap_or(""));
        let mut int = |what: &str| -> Result<i32, String> {
            let w = words.next().ok_or_else(|| format!("the mapping has no {what}"))?;
            w.parse::<i32>().map_err(|_| format!("`{w}` is not a {what}"))
        };
        let size = int("map size")?;
        let first = int("first key")?;
        let last = int("last key")?;
        let middle = int("middle key")?;
        let ref_key = int("reference key")?;
        let w = words.next().ok_or_else(|| "the mapping has no reference frequency".to_string())?;
        let ref_hz = w.parse::<f64>().map_err(|_| format!("`{w}` is not a frequency"))?;
        if !(ref_hz > 0.0) {
            return Err("the reference frequency must be positive".into());
        }
        let w = words.next().ok_or_else(|| "the mapping has no octave degree".to_string())?;
        let octave_degree = w.parse::<i32>().map_err(|_| format!("`{w}` is not an octave degree"))?;
        let mut map = Vec::with_capacity(size.max(0) as usize);
        for _ in 0..size.max(0) {
            // A short map leaves the rest unmapped.
            match words.next() {
                Some("x") | Some("X") | None => map.push(None),
                Some(w) => map.push(Some(w.parse::<i32>().map_err(|_| format!("`{w}` is not a degree or x"))?)),
            }
        }
        Ok(KeyMap { first, last, middle, ref_key, ref_hz, octave_degree, map })
    }

    /// The degree key `k` plays (None: unmapped or out of the key range).
    pub fn degree(&self, k: i32, scale_len: usize) -> Option<i64> {
        if k < self.first || k > self.last {
            return None;
        }
        let off = (k - self.middle) as i64;
        if self.map.is_empty() {
            return Some(off);
        }
        let size = self.map.len() as i64;
        let octave = if self.octave_degree > 0 { self.octave_degree as i64 } else { scale_len as i64 };
        let entry = self.map[off.rem_euclid(size) as usize]?;
        Some(entry as i64 + off.div_euclid(size) * octave)
    }

    /// As [`KeyMap::degree`] without the key range (for the reference key).
    fn degree_unbounded(&self, k: i32, scale_len: usize) -> Option<i64> {
        KeyMap { first: i32::MIN, last: i32::MAX, ..self.clone() }.degree(k, scale_len)
    }
}

/// Notes the tables cover: -256..255.
pub const NOTES: usize = 512;
const LOW: i32 = -256;

/// A scale on a keyboard: frequencies and tuned note numbers.
#[derive(Clone, Debug)]
pub struct Tuning {
    pub scale: Scale,
    pub map: KeyMap,
    pub name: String,
    /// Hz of note i - 256 (0 for an unmapped key).
    hz: Box<[f64; NOTES]>,
}

/// The note number (MIDI convention, fractional) of a frequency.
#[inline]
pub fn hz_to_note(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

impl Tuning {
    pub fn new(name: &str, scale: Scale, map: KeyMap) -> Tuning {
        let n = scale.len();
        let ref_cents = map.degree_unbounded(map.ref_key, n).map(|d| scale.degree_cents(d)).unwrap_or(0.0);
        let mut hz = Box::new([0.0f64; NOTES]);
        for (i, h) in hz.iter_mut().enumerate() {
            let k = i as i32 + LOW;
            if let Some(d) = map.degree(k, n) {
                *h = map.ref_hz * ((scale.degree_cents(d) - ref_cents) / 1200.0).exp2();
            }
        }
        Tuning { scale, map, name: name.to_string(), hz }
    }

    /// From `.scl` and (optional, empty = the standard mapping) `.kbm` text.
    pub fn from_text(name: &str, scl: &str, kbm: &str) -> Result<Tuning, String> {
        let scale = Scale::parse(scl)?;
        let map = if kbm.trim().is_empty() { KeyMap::default() } else { KeyMap::parse(kbm)? };
        Ok(Tuning::new(name, scale, map))
    }

    /// Standard tuning: 12 equal steps, A4 = 440 Hz.
    pub fn standard() -> Tuning {
        Tuning::new("12-TET", Scale::equal("12 equal divisions of the octave", 12, 1200.0), KeyMap::default())
    }

    /// True when every MIDI key plays its standard pitch (within 0.01 cents).
    pub fn is_standard(&self) -> bool {
        (0..128).all(|k| {
            let want = 440.0 * ((k - 69) as f64 / 12.0).exp2();
            self.key_hz(k).is_some_and(|h| (1200.0 * (h / want).log2()).abs() < 0.01)
        })
    }

    /// Hz of key `k` (None: unmapped).
    pub fn key_hz(&self, k: i32) -> Option<f64> {
        let i = (k - LOW).clamp(0, NOTES as i32 - 1) as usize;
        (self.hz[i] > 0.0).then_some(self.hz[i])
    }

    /// The tuned note number a key plays (what "retune the MIDI notes"
    /// feeds a voice); None for an unmapped key.
    pub fn key_note(&self, k: i32) -> Option<f32> {
        self.key_hz(k).map(|h| hz_to_note(h) as f32)
    }

    /// A fractional note (in key units) through the scale: the tuned note
    /// numbers of the two keys around it, linearly interpolated (what
    /// "retune everything" does to every pitch lookup). Unmapped keys take
    /// their standard pitch.
    pub fn retune(&self, n: f32) -> f32 {
        let f = n.floor();
        let t = n - f;
        let k = f as i32;
        let at = |k: i32| self.key_note(k).unwrap_or(k as f32);
        at(k) + t * (at(k + 1) - at(k))
    }
}

/// Our tuning library: (name, about, scale). Ratios and divisions are
/// the scales' definitions.
pub fn library() -> &'static [(&'static str, &'static str, Arc<Tuning>)] {
    use std::sync::OnceLock;
    static LIB: OnceLock<Vec<(&'static str, &'static str, Arc<Tuning>)>> = OnceLock::new();
    LIB.get_or_init(|| {
        let mut out: Vec<(&'static str, &'static str, Scale)> = vec![("12tet", "standard: 12 equal steps", Scale::equal("12 equal divisions of the octave", 12, 1200.0))];
        for (name, n) in [("edo5", 5usize), ("edo7", 7), ("edo10", 10), ("edo17", 17), ("edo19", 19), ("edo22", 22), ("edo24", 24), ("edo31", 31), ("edo41", 41), ("edo53", 53)] {
            let about: &'static str = Box::leak(format!("{n} equal steps to the octave").into_boxed_str());
            out.push((name, about, Scale::equal(about, n, 1200.0)));
        }
        out.push(("just", "5-limit just intonation on C (12 notes)", Scale::ratios("5-limit just intonation", &[(16, 15), (9, 8), (6, 5), (5, 4), (4, 3), (45, 32), (3, 2), (8, 5), (5, 3), (9, 5), (15, 8), (2, 1)])));
        out.push(("pythagorean", "pure fifths (3-limit, 12 notes)", Scale::ratios("Pythagorean", &[(256, 243), (9, 8), (32, 27), (81, 64), (4, 3), (729, 512), (3, 2), (128, 81), (27, 16), (16, 9), (243, 128), (2, 1)])));
        // Quarter-comma meantone: fifths of 5^(1/4), from E-flat to G-sharp around C.
        let fifth = 1200.0 * 5f64.log2() / 4.0;
        let mut mt: Vec<f64> = (-3..=8).map(|k: i32| (k as f64 * fifth).rem_euclid(1200.0)).filter(|c| *c > 1e-9).collect();
        mt.sort_by(f64::total_cmp);
        mt.push(1200.0);
        out.push(("meantone", "quarter-comma meantone (pure major thirds)", Scale { description: "quarter-comma meantone".into(), cents: mt }));
        out.push(("harmonics", "harmonics 8 to 16 (an 8-note overtone scale)", Scale::ratios("harmonics 8-16", &[(9, 8), (10, 8), (11, 8), (12, 8), (13, 8), (14, 8), (15, 8), (2, 1)])));
        out.push(("bohlen_pierce", "13 equal steps to the tritave (3/1)", Scale::equal("13 equal divisions of 3/1", 13, 1200.0 * 3f64.log2())));
        out.into_iter().map(|(n, a, s)| (n, a, Arc::new(Tuning::new(n, s, KeyMap::default())))).collect()
    })
}

/// A library tuning by name.
pub fn by_name(name: &str) -> Option<Arc<Tuning>> {
    let name = name.trim().trim_start_matches('@');
    library().iter().find(|(n, ..)| *n == name).map(|(_, _, t)| t.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_twelve_tet() {
        let t = Tuning::standard();
        assert!(t.is_standard());
        assert!((t.key_hz(69).unwrap() - 440.0).abs() < 1e-9);
        assert!((t.key_note(61).unwrap() - 61.0).abs() < 1e-4);
        assert!((t.retune(60.25) - 60.25).abs() < 1e-4);
    }

    #[test]
    fn scl_and_kbm_parse() {
        let scl = "! test.scl\n!\nA just triad scale\n 3\n!\n5/4 major third\n3/2\n1200.0\n";
        let s = Scale::parse(scl).unwrap();
        assert_eq!(s.description, "A just triad scale");
        assert!((s.cents[0] - 386.3137).abs() < 1e-3 && (s.cents[1] - 701.955).abs() < 1e-3 && s.period() == 1200.0);
        // Degree 0 on key 60 = 261.6256 Hz; keys 60,61,62,63 -> 1/1 5/4 3/2 2/1.
        let kbm = "! map\n0\n0\n127\n60\n60\n261.625565\n0\n";
        let t = Tuning::from_text("triad", scl, kbm).unwrap();
        let c = 261.625565;
        for (k, r) in [(60, 1.0), (61, 1.25), (62, 1.5), (63, 2.0), (57, 0.5), (59, 0.75)] {
            assert!((t.key_hz(k).unwrap() - c * r).abs() < 1e-6, "{k}");
        }
        // A 12-key map with two unmapped keys, its octave at degree 3.
        let kbm = "12\n0\n127\n60\n69\n440.0\n3\n0\nx\n1\nx\n2\n0\n0\n0\n0\n0\n0\n0\n";
        let t = Tuning::from_text("m", scl, kbm).unwrap();
        assert!(t.key_hz(61).is_none() && t.key_hz(63).is_none());
        assert!((t.key_hz(72).unwrap() / t.key_hz(60).unwrap() - 2.0).abs() < 1e-9);
        assert!((t.key_hz(62).unwrap() / t.key_hz(60).unwrap() - 1.25).abs() < 1e-9);
        assert!(Scale::parse("x\n2\n3/2\n").is_err());
        assert!(KeyMap::parse("0\n0\n127\n60\n69\n").is_err());
    }

    #[test]
    fn the_library_is_in_tune() {
        assert!(by_name("12tet").unwrap().is_standard());
        let e19 = by_name("edo19").unwrap();
        assert!((e19.key_hz(69).unwrap() - 440.0).abs() < 1e-9);
        assert!((1200.0 * (e19.key_hz(61).unwrap() / e19.key_hz(60).unwrap()).log2() - 1200.0 / 19.0).abs() < 1e-9);
        let just = by_name("just").unwrap();
        assert!((just.key_hz(67).unwrap() / just.key_hz(60).unwrap() - 1.5).abs() < 1e-9);
        let mt = by_name("meantone").unwrap();
        assert_eq!(mt.scale.len(), 12);
        assert!((mt.key_hz(64).unwrap() / mt.key_hz(60).unwrap() - 1.25).abs() < 1e-9, "pure third");
        let bp = by_name("bohlen_pierce").unwrap();
        assert!((bp.key_hz(73).unwrap() / bp.key_hz(60).unwrap() - 3.0).abs() < 1e-9);
        // The text form round-trips.
        let s = &by_name("edo31").unwrap().scale;
        let back = Scale::parse(&s.to_text()).unwrap();
        assert!(back.cents.iter().zip(&s.cents).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
