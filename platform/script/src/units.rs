use relax_api::relax_api;

/// One registered suffix: a literal `<number><suffix>` evaluates to `number * mul / div`.
///
/// The scale is a ratio so decimal units stay exact: when `mul` and `div` are powers of ten the
/// literal's digits are read with the decimal exponent shifted, so `18mm` is the double nearest
/// 0.018 (`18 * 0.001` is not), `8.06kohm` is 8060 and `0.1uF` the double nearest 1e-7.
#[relax_api(handle, clone, debug, eq, run)]
#[derive(Clone, Debug, PartialEq)]
pub struct UnitEntry {
    pub suffix: String,
    pub mul: f64,
    pub div: f64,
}

#[relax_api(run)]
impl UnitEntry {
    /// The value of `v` written in this unit, in the base unit.
    pub fn to_base(&self, v: f64) -> f64 {
        v * self.mul / self.div
    }

    /// The value of a literal whose number is written `digits` (`8.06`, `1e3`) in this unit, in
    /// the base unit: exact for a power-of-ten ratio (the decimal exponent is shifted), else
    /// `to_base` of the parsed digits.
    pub fn literal(&self, digits: &str) -> f64 {
        if let (Some(a), Some(b)) = (pow10(self.mul), pow10(self.div)) {
            let (mantissa, exp) = match digits.find(|c: char| c == 'e' || c == 'E') {
                Some(at) => (&digits[..at], digits[at + 1..].parse::<i32>().ok()),
                None => (digits, Some(0)),
            };
            if let Some(exp) = exp {
                if let Ok(v) = format!("{mantissa}e{}", exp + a - b).parse::<f64>() {
                    return v;
                }
            }
        }
        self.to_base(digits.parse::<f64>().unwrap_or(0.0))
    }
}

/// `k` when `x` is exactly `10^k` (0 to 22, the powers a double holds exactly).
fn pow10(x: f64) -> Option<i32> {
    (0..=22).find(|&k| 10f64.powi(k) == x)
}

/// The unit suffixes number literals may carry. Case-sensitive (`mV`, `MHz`, `mohm`, `Mohm` differ).
#[relax_api(handle, clone, debug, eq, default, run)]
#[derive(Clone, Debug, PartialEq)]
pub struct UnitTable {
    entries: Vec<UnitEntry>,
}

/// Index of an entry in the table the token was lexed with.
#[relax_api(plain, run)]
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UnitIx(pub u16);

#[relax_api(run)]
impl UnitTable {
    /// Today's units with today's values: s, rad, px and m are base units; ms 1/1000, deg and °
    /// PI/180, cm 1/100, mm 1/1000.
    pub fn standard() -> UnitTable {
        let pi = std::f64::consts::PI;
        UnitTable { entries: Vec::new() }
            .with("s", 1.0, 1.0).with("ms", 1.0, 1000.0).with("deg", pi, 180.0).with("°", pi, 180.0)
            .with("rad", 1.0, 1.0).with("px", 1.0, 1.0).with("m", 1.0, 1.0).with("cm", 1.0, 100.0).with("mm", 1.0, 1000.0)
    }

    /// This table plus one suffix whose literal `v` is `v * mul / div` in the base unit (replacing
    /// an entry with the same suffix). Builder style.
    pub fn with(mut self, suffix: &str, mul: f64, div: f64) -> UnitTable {
        if let Some(ix) = self.lookup(suffix) {
            let entry = &mut self.entries[ix.0 as usize];
            (entry.mul, entry.div) = (mul, div);
        } else {
            assert!(self.entries.len() <= u16::MAX as usize, "unit table exceeds UnitIx capacity");
            self.entries.push(UnitEntry { suffix: suffix.to_string(), mul, div });
        }
        self
    }

    /// The entry whose suffix is exactly `suffix`.
    pub fn lookup(&self, suffix: &str) -> Option<UnitIx> {
        self.entries.iter().position(|entry| entry.suffix == suffix).map(|ix| UnitIx(ix as u16))
    }

    /// The suffix and ratio at `ix` in this table.
    pub fn entry(&self, ix: UnitIx) -> &UnitEntry { &self.entries[ix.0 as usize] }

    /// The registered suffixes, in index order.
    pub fn entries(&self) -> &[UnitEntry] { &self.entries }

    /// Whether some suffix starts with `c` (the tokenizer asks this for the type-suffix letters).
    pub fn has_suffix_starting_with(&self, c: char) -> bool {
        self.entries.iter().any(|entry| entry.suffix.starts_with(c))
    }
}

impl Default for UnitTable {
    fn default() -> Self { Self::standard() }
}
