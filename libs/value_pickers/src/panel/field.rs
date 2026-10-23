//! The value field's arithmetic, apart from any drawing: what a typed entry
//! means (an expression, a relative change, numbers with units), how the
//! soft range stretches around a value, how far a drag moves a value and
//! the round numbers Ctrl snaps to.
//!
//! The soft range is a hint: it sets the fill bar and how fast a drag
//! moves, and it grows (to a round number) whenever a value lands outside
//! it. Only hard limits, where the value's domain has them, ever clamp.

/// A typed entry's value in the field's `unit` (`""`, `s`, `ms`, `deg`,
/// `rad`, `px`), `current` being the value now:
/// - an expression: `3.5*2`, `(1 + 2) / 4`, `pi/2`, `sqrt(2)`, `2^3`;
/// - relative, when it starts with an operator: `+0.5`, `*2`, `/3`
///   (`-0.5` is a negative number, not a subtraction);
/// - numbers may carry units, converted into the field's: `200ms` into
///   seconds is 0.2, `1.5rad` into degrees 85.94; a unit of another kind
///   (`90deg` into seconds) is an error.
pub fn eval(text: &str, current: f64, unit: &str) -> Result<f64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    let relative = t.starts_with('+') || t.starts_with('*') || t.starts_with('/');
    let mut p = Parser { s: t.as_bytes(), i: 0, unit };
    let v = if relative {
        let op = p.s[0];
        p.i = 1;
        let rhs = p.expr()?;
        match op {
            b'+' => current + rhs,
            b'*' => current * rhs,
            _ => {
                if rhs == 0.0 {
                    return Err("division by zero".into());
                }
                current / rhs
            }
        }
    } else {
        p.expr()?
    };
    p.space();
    if p.i < p.s.len() {
        return Err(format!("unexpected `{}`", &t[p.i..]));
    }
    if !v.is_finite() {
        return Err("not a finite number".into());
    }
    Ok(v)
}

/// A unit's kind and its size in that kind's base (seconds, degrees).
fn unit_of(u: &str) -> Option<(&'static str, f64)> {
    Some(match u {
        "s" => ("time", 1.0),
        "ms" => ("time", 0.001),
        "deg" | "°" => ("angle", 1.0),
        "rad" => ("angle", 180.0 / std::f64::consts::PI),
        "px" => ("length", 1.0),
        _ => return None,
    })
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    unit: &'a str,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(c) = self.peek() {
            match c {
                b'+' => {
                    self.i += 1;
                    v += self.term()?;
                }
                b'-' => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.power()?;
        while let Some(c) = self.peek() {
            match c {
                b'*' => {
                    self.i += 1;
                    v *= self.power()?;
                }
                b'/' => {
                    self.i += 1;
                    let d = self.power()?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    v /= d;
                }
                b'%' => {
                    self.i += 1;
                    let d = self.power()?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    v = v.rem_euclid(d);
                }
                _ => break,
            }
        }
        Ok(v)
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.unary()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            // Right associative: 2^3^2 is 2^9.
            let e = self.power()?;
            return Ok(base.powf(e));
        }
        Ok(base)
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err("missing `)`".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => {
                let s = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                    self.i += 1;
                }
                let name = std::str::from_utf8(&self.s[s..self.i]).unwrap_or("");
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    let mut args = vec![self.expr()?];
                    while self.peek() == Some(b',') {
                        self.i += 1;
                        args.push(self.expr()?);
                    }
                    if self.peek() != Some(b')') {
                        return Err("missing `)`".into());
                    }
                    self.i += 1;
                    return call(name, &args);
                }
                match name {
                    "pi" => Ok(std::f64::consts::PI),
                    "tau" => Ok(std::f64::consts::TAU),
                    "e" => Ok(std::f64::consts::E),
                    _ => Err(format!("unknown `{name}`")),
                }
            }
            Some(c) => Err(format!("unexpected `{}`", c as char)),
            None => Err("incomplete".into()),
        }
    }

    /// A number and its unit, in the field's unit.
    fn number(&mut self) -> Result<f64, String> {
        let s = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
            self.i += 1;
        }
        // An exponent: 1e-3.
        if self.i < self.s.len() && (self.s[self.i] == b'e' || self.s[self.i] == b'E') {
            let mut j = self.i + 1;
            if j < self.s.len() && (self.s[j] == b'-' || self.s[j] == b'+') {
                j += 1;
            }
            if j < self.s.len() && self.s[j].is_ascii_digit() {
                self.i = j;
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
        }
        let text = std::str::from_utf8(&self.s[s..self.i]).unwrap_or("");
        let v: f64 = text.parse().map_err(|_| format!("bad number `{text}`"))?;
        // Its unit, written right after it (`0.2s`, `90deg`, `90°`).
        let u0 = self.i;
        if self.s[self.i..].starts_with("°".as_bytes()) {
            self.i += "°".len();
        } else {
            while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
                self.i += 1;
            }
        }
        let u = std::str::from_utf8(&self.s[u0..self.i]).unwrap_or("");
        if u.is_empty() {
            return Ok(v);
        }
        let Some((kind, size)) = unit_of(u) else {
            return Err(format!("unknown unit `{u}`"));
        };
        match unit_of(self.unit) {
            Some((want, base)) if want == kind => Ok(v * size / base),
            Some(_) => Err(format!("`{u}` is not a {}", match self.unit { "s" | "ms" => "time", "deg" | "rad" => "angle", _ => "length" })),
            // A plain number field takes the number in its kind's base.
            None => Ok(v * size),
        }
    }
}

fn call(name: &str, a: &[f64]) -> Result<f64, String> {
    let one = |f: fn(f64) -> f64| if a.len() == 1 { Ok(f(a[0])) } else { Err(format!("`{name}` takes one number")) };
    match name {
        "sqrt" => one(f64::sqrt),
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "round" => one(f64::round),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "exp" => one(f64::exp),
        "ln" | "log" => one(f64::ln),
        "radians" => one(f64::to_radians),
        "degrees" => one(f64::to_degrees),
        "min" if !a.is_empty() => Ok(a.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" if !a.is_empty() => Ok(a.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "pow" if a.len() == 2 => Ok(a[0].powf(a[1])),
        _ => Err(format!("unknown `{name}`")),
    }
}

/// The smallest round number (1, 2 or 5 times a power of ten) at or above
/// `x` (> 0).
pub fn nice_ceil(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    let p = 10f64.powf(x.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        let v = m * p;
        // (A hair of slack: 0.30000000000000004 is 0.3.)
        if v >= x * (1.0 - 1e-12) {
            return v;
        }
    }
    10.0 * p
}

/// The soft range grown to hold `v`: the end it passed moves out to a
/// round number beyond it (0..1 with 1.7 becomes 0..2, with 3.5 0..5, with
/// -0.3 -0.5..1). A value inside leaves it as it is.
pub fn stretch((lo, hi): (f64, f64), v: f64) -> (f64, f64) {
    if !v.is_finite() {
        return (lo, hi);
    }
    let (mut lo, mut hi) = (lo, hi);
    if v > hi {
        hi = if v > 0.0 { nice_ceil(v) } else { -nice_floor(-v) };
    }
    if v < lo {
        lo = if v < 0.0 { -nice_ceil(-v) } else { nice_floor(v) };
    }
    (lo, hi)
}

/// The largest round number at or below `x` (> 0), 0 below the smallest.
fn nice_floor(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    let p = 10f64.powf(x.log10().floor());
    for m in [5.0, 2.0, 1.0] {
        let v = m * p;
        if v <= x * (1.0 + 1e-12) {
            return v;
        }
    }
    p
}

/// The round increment Ctrl snaps a drag to for a soft range `span` wide:
/// a power of ten giving the range 4 to 40 rungs (0..1: 0.1, 0..10: 1,
/// 0..360: 10); with Shift a tenth of it.
pub fn snap_rung(span: f64, fine: bool) -> f64 {
    let span = span.abs().max(1e-9);
    let rung = 10f64.powf((span / 4.0).log10().floor());
    if fine { rung * 0.1 } else { rung }
}

/// One step of the arrows, the wheel and the arrow keys: a round number
/// near a hundredth of the soft range (`whole`: at least 1, whole).
pub fn increment(span: f64, whole: bool) -> f64 {
    let raw = span.abs().max(1e-9) / 100.0;
    let p = 10f64.powf(raw.log10().floor());
    let step = if raw / p >= 5.0 { 5.0 * p } else if raw / p >= 2.0 { 2.0 * p } else { p };
    if whole { step.max(1.0).round() } else { step }
}

/// How far the value moves per point of drag: the field's width is the
/// soft range for a slider (the fill follows the pointer), the soft range
/// per 400 points for a number field; a tenth of that with Shift.
pub fn drag_rate(span: f64, width: f64, slider: bool, fine: bool) -> f64 {
    let travel = if slider { width.max(40.0) } else { 400.0 };
    let rate = span.abs().max(1e-9) / travel;
    if fine { rate * 0.1 } else { rate }
}

/// `v` on a multiple of `step` (none when `step` is 0).
pub fn snap(v: f64, step: f64) -> f64 {
    if step > 0.0 { (v / step).round() * step } else { v }
}

/// How many decimals show a step (0.05: 2, 1: 0).
pub fn decimals_of(step: f64) -> usize {
    if step >= 1.0 || step <= 0.0 { 0 } else { ((-step.log10()) - 1e-9).ceil().max(0.0) as usize }
}

/// How many decimals a number is written with (`3.250` → 3).
pub fn written_decimals(text: &str) -> usize {
    let t = text.trim();
    let digits: &str = t.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '°');
    digits.split_once('.').map_or(0, |(_, f)| f.chars().take_while(|c| c.is_ascii_digit()).count())
}

/// A value inside the hard limits, where there are any.
pub fn clamp_hard(v: f64, hard: (Option<f64>, Option<f64>)) -> f64 {
    let v = match hard.0 {
        Some(lo) if v < lo => lo,
        _ => v,
    };
    match hard.1 {
        Some(hi) if v > hi => hi,
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn expressions() {
        assert!(close(eval("3.5*2", 0.0, "").unwrap(), 7.0));
        assert!(close(eval("(1 + 2) / 4", 0.0, "").unwrap(), 0.75));
        assert!(close(eval("2^3^2", 0.0, "").unwrap(), 512.0));
        assert!(close(eval("-0.5", 3.0, "").unwrap(), -0.5));
        assert!(close(eval("pi/2", 0.0, "").unwrap(), std::f64::consts::FRAC_PI_2));
        assert!(close(eval("max(1, 4, 2)", 0.0, "").unwrap(), 4.0));
        assert!(close(eval("1e-3*2", 0.0, "").unwrap(), 0.002));
        assert!(eval("3 +", 0.0, "").is_err());
        assert!(eval("foo", 0.0, "").is_err());
        assert!(eval("1/0", 0.0, "").is_err());
    }

    #[test]
    fn relative() {
        assert!(close(eval("+0.5", 3.0, "").unwrap(), 3.5));
        assert!(close(eval("*2", 3.0, "").unwrap(), 6.0));
        assert!(close(eval("/4", 3.0, "").unwrap(), 0.75));
        assert!(close(eval("+ 2*3", 1.0, "").unwrap(), 7.0));
    }

    #[test]
    fn units() {
        assert!(close(eval("200ms", 0.0, "s").unwrap(), 0.2));
        assert!(close(eval("0.2s", 0.0, "ms").unwrap(), 200.0));
        assert!(close(eval("0.2s + 50ms", 0.0, "s").unwrap(), 0.25));
        assert!(close(eval("90deg", 0.0, "rad").unwrap(), std::f64::consts::FRAC_PI_2));
        assert!(close(eval("90°", 0.0, "deg").unwrap(), 90.0));
        assert!(close(eval("12px", 0.0, "px").unwrap(), 12.0));
        assert!(close(eval("0.2s", 0.0, "").unwrap(), 0.2));
        assert!(eval("90deg", 0.0, "s").is_err());
        assert!(eval("3kg", 0.0, "").is_err());
    }

    #[test]
    fn round_numbers() {
        assert_eq!(nice_ceil(1.7), 2.0);
        assert_eq!(nice_ceil(3.5), 5.0);
        assert_eq!(nice_ceil(7.0), 10.0);
        assert_eq!(nice_ceil(0.3), 0.5);
        assert_eq!(nice_ceil(2.0), 2.0);
        assert_eq!(nice_ceil(120.0), 200.0);
    }

    #[test]
    fn stretches() {
        assert_eq!(stretch((0.0, 1.0), 1.7), (0.0, 2.0));
        assert_eq!(stretch((0.0, 1.0), 3.5), (0.0, 5.0));
        assert_eq!(stretch((0.0, 1.0), -0.3), (-0.5, 1.0));
        assert_eq!(stretch((0.0, 1.0), 0.4), (0.0, 1.0));
        assert_eq!(stretch((2.0, 4.0), 1.5), (1.0, 4.0));
        assert_eq!(stretch((-4.0, -2.0), -1.0), (-4.0, -1.0));
    }

    #[test]
    fn rungs_and_steps() {
        assert!(close(snap_rung(1.0, false), 0.1));
        assert!(close(snap_rung(10.0, false), 1.0));
        assert!(close(snap_rung(360.0, false), 10.0));
        assert!(close(snap_rung(10.0, true), 0.1));
        assert!(close(increment(1.0, false), 0.01));
        assert!(close(increment(360.0, false), 2.0));
        assert!(close(increment(16.0, true), 1.0));
        assert_eq!(decimals_of(0.05), 2);
        assert_eq!(decimals_of(1.0), 0);
        assert_eq!(written_decimals("0.125s"), 3);
        assert_eq!(written_decimals("12"), 0);
    }

    #[test]
    fn hard_limits() {
        assert_eq!(clamp_hard(-1.0, (Some(0.0), None)), 0.0);
        assert_eq!(clamp_hard(5.0, (None, None)), 5.0);
        assert_eq!(clamp_hard(5.0, (None, Some(1.0))), 1.0);
    }
}
