//! The binding-expression VM: how a document ties any animatable parameter
//! to the VJ's music-synced signals.
//!
//! A binding is a STRING in the document — `sway: "0.2 + 0.6 * env(phase)"`
//! — compiled ONCE at load into a tiny stack bytecode and evaluated per
//! frame in nanoseconds. This is the one legitimately per-frame piece of
//! the configuration layer; the splash interpreter itself never runs in the
//! frame path.
//!
//! # Signals
//!
//! | name     | meaning                                             |
//! |----------|-----------------------------------------------------|
//! | `time`   | effect-local seconds (speed-scaled)                 |
//! | `dt`     | last frame's delta seconds                          |
//! | `beat`   | continuous beat position (grows by 1 every beat)    |
//! | `phase`  | beat phase 0..1 at the document's `beat_rate`       |
//! | `bar`    | bar phase 0..1 (`bar_beats` beats per bar, def. 4)  |
//! | `bpm`    | tempo                                               |
//! | `pulse`  | eased beat envelope 0..1 ((1-phase)^3)              |
//! | `energy` | overall audio level 0..1 (0 until the host feeds it)|
//! | `bass`   | low band 0..1 (stub 0 until fed)                    |
//! | `mid`    | mid band 0..1 (stub 0 until fed)                    |
//! | `high`   | high band 0..1 (stub 0 until fed)                   |
//! | `p0`..`p3` | the user params for THIS frame: the host's dial   |
//! |          | override when touched, else the doc's own `p0:` value |
//! |          | (a p-param's OWN binding sees other p's as 0)       |
//!
//! Constants: `pi`, `tau`.
//!
//! # Grammar
//!
//! Numbers, signals, `+ - * /`, unary `-`, parentheses, and calls:
//! `sin cos abs floor fract sqrt` (1 arg), `tri saw env` (1 arg: phase-like
//! 0..1 waves — triangle, rising saw = identity fract, cubic decay
//! envelope), `min max pow step` (2), `clamp mix` (3).

/// Signal vector: order is the `Sig::*` indices.
pub const SIG_COUNT: usize = 15;

#[derive(Clone, Copy)]
pub struct Signals(pub [f32; SIG_COUNT]);

impl Signals {
    pub const TIME: usize = 0;
    pub const DT: usize = 1;
    pub const BEAT: usize = 2;
    pub const PHASE: usize = 3;
    pub const BAR: usize = 4;
    pub const BPM: usize = 5;
    pub const PULSE: usize = 6;
    pub const ENERGY: usize = 7;
    pub const BASS: usize = 8;
    pub const MID: usize = 9;
    pub const HIGH: usize = 10;
    /// p0..p3 — resolved user params (host dial override, else the doc's
    /// own binding). Contiguous: `P0 + n` is pn.
    pub const P0: usize = 11;
    pub const P1: usize = 12;
    pub const P2: usize = 13;
    pub const P3: usize = 14;
}

fn signal_index(name: &str) -> Option<usize> {
    Some(match name {
        "time" => Signals::TIME,
        "dt" => Signals::DT,
        "beat" => Signals::BEAT,
        "phase" => Signals::PHASE,
        "bar" => Signals::BAR,
        "bpm" => Signals::BPM,
        "pulse" => Signals::PULSE,
        "energy" => Signals::ENERGY,
        "bass" => Signals::BASS,
        "mid" => Signals::MID,
        "high" => Signals::HIGH,
        "p0" => Signals::P0,
        "p1" => Signals::P1,
        "p2" => Signals::P2,
        "p3" => Signals::P3,
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Const(f32),
    Sig(u8),
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    // 1-arg
    Sin,
    Cos,
    Abs,
    Floor,
    Fract,
    Sqrt,
    Tri,
    Saw,
    Env,
    // 2-arg
    Min,
    Max,
    Pow,
    Step,
    // 3-arg
    Clamp,
    Mix,
}

fn fn_op(name: &str) -> Option<(Op, usize)> {
    Some(match name {
        "sin" => (Op::Sin, 1),
        "cos" => (Op::Cos, 1),
        "abs" => (Op::Abs, 1),
        "floor" => (Op::Floor, 1),
        "fract" => (Op::Fract, 1),
        "sqrt" => (Op::Sqrt, 1),
        "tri" => (Op::Tri, 1),
        "saw" => (Op::Saw, 1),
        "env" => (Op::Env, 1),
        "min" => (Op::Min, 2),
        "max" => (Op::Max, 2),
        "pow" => (Op::Pow, 2),
        "step" => (Op::Step, 2),
        "clamp" => (Op::Clamp, 3),
        "mix" => (Op::Mix, 3),
        _ => return None,
    })
}

/// A compiled binding expression.
#[derive(Clone, Debug)]
pub struct Expr {
    ops: Vec<Op>,
}

impl Expr {
    /// Compile `source`. Errors carry a human/LLM-readable message.
    pub fn compile(source: &str) -> Result<Expr, String> {
        let mut parser = Parser { chars: source.char_indices().peekable(), src: source, ops: Vec::new() };
        parser.expr()?;
        parser.skip_ws();
        if let Some(&(at, c)) = parser.chars.peek() {
            return Err(format!("unexpected '{c}' at {at}"));
        }
        if parser.ops.len() > 256 {
            return Err("expression too long (max 256 ops)".to_string());
        }
        Ok(Expr { ops: parser.ops })
    }

    /// Evaluate against the frame's signals. Non-finite results clamp to 0.
    pub fn eval(&self, sig: &Signals) -> f32 {
        let mut stack = [0.0f32; 32];
        let mut sp = 0usize;
        macro_rules! pop {
            () => {{
                sp -= 1;
                stack[sp]
            }};
        }
        macro_rules! push {
            ($v:expr) => {{
                if sp < 32 {
                    stack[sp] = $v;
                    sp += 1;
                }
            }};
        }
        for op in &self.ops {
            match *op {
                Op::Const(v) => push!(v),
                Op::Sig(i) => push!(sig.0[i as usize]),
                Op::Add => {
                    let b = pop!();
                    let a = pop!();
                    push!(a + b)
                }
                Op::Sub => {
                    let b = pop!();
                    let a = pop!();
                    push!(a - b)
                }
                Op::Mul => {
                    let b = pop!();
                    let a = pop!();
                    push!(a * b)
                }
                Op::Div => {
                    let b = pop!();
                    let a = pop!();
                    push!(if b.abs() < 1e-9 { 0.0 } else { a / b })
                }
                Op::Neg => {
                    let a = pop!();
                    push!(-a)
                }
                Op::Sin => {
                    let a = pop!();
                    push!(a.sin())
                }
                Op::Cos => {
                    let a = pop!();
                    push!(a.cos())
                }
                Op::Abs => {
                    let a = pop!();
                    push!(a.abs())
                }
                Op::Floor => {
                    let a = pop!();
                    push!(a.floor())
                }
                Op::Fract => {
                    let a = pop!();
                    push!(a.fract())
                }
                Op::Sqrt => {
                    let a = pop!();
                    push!(a.max(0.0).sqrt())
                }
                Op::Tri => {
                    let a = pop!();
                    let p = (a.fract() + 1.0).fract();
                    push!(1.0 - (p * 2.0 - 1.0).abs())
                }
                Op::Saw => {
                    let a = pop!();
                    push!((a.fract() + 1.0).fract())
                }
                Op::Env => {
                    let a = pop!();
                    let p = ((a.fract() + 1.0).fract()).clamp(0.0, 1.0);
                    push!((1.0 - p).powi(3))
                }
                Op::Min => {
                    let b = pop!();
                    let a = pop!();
                    push!(a.min(b))
                }
                Op::Max => {
                    let b = pop!();
                    let a = pop!();
                    push!(a.max(b))
                }
                Op::Pow => {
                    let b = pop!();
                    let a = pop!();
                    push!(a.abs().powf(b))
                }
                Op::Step => {
                    let b = pop!();
                    let a = pop!();
                    push!(if b >= a { 1.0 } else { 0.0 })
                }
                Op::Clamp => {
                    let hi = pop!();
                    let lo = pop!();
                    let a = pop!();
                    push!(a.clamp(lo.min(hi), hi.max(lo)))
                }
                Op::Mix => {
                    let t = pop!();
                    let b = pop!();
                    let a = pop!();
                    push!(a + (b - a) * t)
                }
            }
        }
        let v = if sp > 0 { stack[sp - 1] } else { 0.0 };
        if v.is_finite() {
            v
        } else {
            0.0
        }
    }
}

/// A parameter that is either a constant or a compiled binding.
#[derive(Clone)]
pub enum Animatable {
    Const(f32),
    Bound(Expr),
}

impl Animatable {
    pub fn value(&self, sig: &Signals) -> f32 {
        match self {
            Animatable::Const(v) => *v,
            Animatable::Bound(e) => e.eval(sig),
        }
    }
    pub fn constant(v: f32) -> Self {
        Animatable::Const(v)
    }
}

// ---------------------------------------------------------------------------
// Recursive-descent parser: expr -> term (± term)*, term -> unary (*/ unary)*,
// unary -> - unary | atom, atom -> number | name | name(args) | (expr).
// ---------------------------------------------------------------------------

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    src: &'a str,
    ops: Vec<Op>,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(&(_, c)) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, want: char) -> bool {
        self.skip_ws();
        if let Some(&(_, c)) = self.chars.peek() {
            if c == want {
                self.chars.next();
                return true;
            }
        }
        false
    }

    fn expr(&mut self) -> Result<(), String> {
        self.term()?;
        loop {
            self.skip_ws();
            match self.chars.peek() {
                Some(&(_, '+')) => {
                    self.chars.next();
                    self.term()?;
                    self.ops.push(Op::Add);
                }
                Some(&(_, '-')) => {
                    self.chars.next();
                    self.term()?;
                    self.ops.push(Op::Sub);
                }
                _ => return Ok(()),
            }
        }
    }

    fn term(&mut self) -> Result<(), String> {
        self.unary()?;
        loop {
            self.skip_ws();
            match self.chars.peek() {
                Some(&(_, '*')) => {
                    self.chars.next();
                    self.unary()?;
                    self.ops.push(Op::Mul);
                }
                Some(&(_, '/')) => {
                    self.chars.next();
                    self.unary()?;
                    self.ops.push(Op::Div);
                }
                _ => return Ok(()),
            }
        }
    }

    fn unary(&mut self) -> Result<(), String> {
        self.skip_ws();
        if self.eat('-') {
            self.unary()?;
            self.ops.push(Op::Neg);
            return Ok(());
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<(), String> {
        self.skip_ws();
        let Some(&(start, c)) = self.chars.peek() else {
            return Err("expression ended early".to_string());
        };
        if c == '(' {
            self.chars.next();
            self.expr()?;
            if !self.eat(')') {
                return Err(format!("missing ')' after position {start}"));
            }
            return Ok(());
        }
        if c.is_ascii_digit() || c == '.' {
            let mut end = start;
            while let Some(&(at, c)) = self.chars.peek() {
                if c.is_ascii_digit() || c == '.' {
                    end = at + c.len_utf8();
                    self.chars.next();
                } else {
                    break;
                }
            }
            let text = &self.src[start..end];
            let v: f32 = text
                .parse()
                .map_err(|_| format!("bad number '{text}' at {start}"))?;
            self.ops.push(Op::Const(v));
            return Ok(());
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut end = start;
            while let Some(&(at, c)) = self.chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    end = at + c.len_utf8();
                    self.chars.next();
                } else {
                    break;
                }
            }
            let name = &self.src[start..end];
            self.skip_ws();
            if let Some(&(_, '(')) = self.chars.peek() {
                // Function call.
                self.chars.next();
                let Some((op, arity)) = fn_op(name) else {
                    return Err(format!(
                        "unknown function '{name}' (sin cos abs floor fract sqrt tri saw env \
                         min max pow step clamp mix)"
                    ));
                };
                for i in 0..arity {
                    if i > 0 && !self.eat(',') {
                        return Err(format!("'{name}' needs {arity} arguments"));
                    }
                    self.expr()?;
                }
                if !self.eat(')') {
                    return Err(format!("missing ')' closing '{name}('"));
                }
                self.ops.push(op);
                return Ok(());
            }
            // Signal or constant.
            return match name {
                "pi" => {
                    self.ops.push(Op::Const(std::f32::consts::PI));
                    Ok(())
                }
                "tau" => {
                    self.ops.push(Op::Const(std::f32::consts::TAU));
                    Ok(())
                }
                _ => match signal_index(name) {
                    Some(i) => {
                        self.ops.push(Op::Sig(i as u8));
                        Ok(())
                    }
                    None => Err(format!(
                        "unknown signal '{name}' (time dt beat phase bar bpm pulse energy \
                         bass mid high p0 p1 p2 p3, constants pi tau)"
                    )),
                },
            };
        }
        Err(format!("unexpected '{c}' at {start}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig() -> Signals {
        let mut s = Signals([0.0; SIG_COUNT]);
        s.0[Signals::TIME] = 2.0;
        s.0[Signals::PHASE] = 0.25;
        s.0[Signals::BAR] = 0.5;
        s.0[Signals::BPM] = 120.0;
        s.0[Signals::ENERGY] = 0.8;
        s
    }

    #[test]
    fn arithmetic_and_precedence() {
        let e = Expr::compile("1 + 2 * 3 - 4 / 2").unwrap();
        assert_eq!(e.eval(&sig()), 5.0);
        let e = Expr::compile("(1 + 2) * 3").unwrap();
        assert_eq!(e.eval(&sig()), 9.0);
        let e = Expr::compile("-time * 2").unwrap();
        assert_eq!(e.eval(&sig()), -4.0);
    }

    #[test]
    fn signals_and_functions() {
        let e = Expr::compile("sin(bar * tau)").unwrap();
        assert!((e.eval(&sig()) - (0.5f32 * std::f32::consts::TAU).sin()).abs() < 1e-5);
        let e = Expr::compile("mix(0.2, 1.0, energy)").unwrap();
        assert!((e.eval(&sig()) - 0.84).abs() < 1e-5);
        let e = Expr::compile("env(phase)").unwrap();
        assert!((e.eval(&sig()) - 0.75f32.powi(3)).abs() < 1e-5);
        let e = Expr::compile("clamp(bpm / 120, 0, 1)").unwrap();
        assert_eq!(e.eval(&sig()), 1.0);
        let e = Expr::compile("tri(0.75)").unwrap();
        assert!((e.eval(&sig()) - 0.5).abs() < 1e-5);
    }

#[test]
fn bare_signal_expr() {
    let mut s = Signals([0.0; SIG_COUNT]);
    s.0[Signals::P0] = 0.7;
    let e = Expr::compile("p0").unwrap();
    assert!((e.eval(&s) - 0.7).abs() < 1e-6);
}

    #[test]
    fn user_param_signals() {
        // p0..p3 are signals: dial-routed bindings like "0.4 + p1*1.2".
        let mut s = sig();
        s.0[Signals::P0] = 0.5;
        s.0[Signals::P1] = 0.25;
        s.0[Signals::P3] = 1.0;
        let e = Expr::compile("0.4 + p1 * 1.2").unwrap();
        assert!((e.eval(&s) - 0.7).abs() < 1e-5);
        let e = Expr::compile("mix(0.2, 1.0, p0) + p2 + p3").unwrap();
        assert!((e.eval(&s) - 1.6).abs() < 1e-5);
    }

    #[test]
    fn errors_are_readable() {
        assert!(Expr::compile("sin(").is_err());
        assert!(Expr::compile("blorp * 2").unwrap_err().contains("unknown signal"));
        assert!(Expr::compile("warp(1)").unwrap_err().contains("unknown function"));
        assert!(Expr::compile("1 + ").is_err());
    }

    #[test]
    fn division_by_zero_and_nan_are_tamed() {
        let e = Expr::compile("1 / 0").unwrap();
        assert_eq!(e.eval(&sig()), 0.0);
        let e = Expr::compile("pow(-2, 0.5)").unwrap();
        assert!(e.eval(&sig()).is_finite());
    }
}
