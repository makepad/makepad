//! Compiled, allocation-free-at-evaluation size expressions.

use std::collections::HashMap;

/// Maximum evaluator stack depth. Compilation rejects programs that would
/// exceed this bound, so evaluation can use a fixed stack.
pub const SIZE_EXPR_STACK_CAP: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SizeExprId(pub u32);

impl SizeExprId {
    pub const INVALID: Self = Self(u32::MAX);
}

impl Default for SizeExprId {
    fn default() -> Self {
        Self::INVALID
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SizeExprSimple {
    Abs(f64),
    Rel { unit: SizeExprUnit, factor: f64 },
    Compound(SizeExprId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeExprUnit {
    Parent,
    Vw,
    Vh,
    Cqw,
    Cqh,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SizeExprContext {
    pub parent: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub container_width: f64,
    pub container_height: f64,
}

impl SizeExprContext {
    #[inline]
    fn base(self, unit: SizeExprUnit) -> f64 {
        match unit {
            SizeExprUnit::Parent => self.parent,
            SizeExprUnit::Vw => self.viewport_width,
            SizeExprUnit::Vh => self.viewport_height,
            SizeExprUnit::Cqw => self.container_width,
            SizeExprUnit::Cqh => self.container_height,
        }
    }
}

#[derive(Clone, Debug)]
struct SizeExpression {
    source: String,
    program: Vec<Op>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Scalar(f64),
    Length(f64),
    Relative(SizeExprUnit, f64),
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Clamp,
}

#[derive(Default, Debug)]
pub struct SizeExprStore {
    expressions: Vec<SizeExpression>,
    by_source: HashMap<String, SizeExprId>,
}

impl SizeExprStore {
    /// Compiles and interns `source`. Equivalent expressions with the same
    /// normalized spelling share an id for the lifetime of this store.
    pub fn intern(&mut self, source: &str) -> Result<SizeExprSimple, String> {
        let id = self.intern_id(source)?;
        Ok(self.classify(id))
    }

    /// Compiles and interns `source`, returning its stable program handle even
    /// when the expression can also be represented as a simple absolute or
    /// relative size.
    pub fn intern_id(&mut self, source: &str) -> Result<SizeExprId, String> {
        let normalized = normalize_source(source);
        if normalized.is_empty() {
            return Err("empty size expression".into());
        }
        if let Some(id) = self.by_source.get(&normalized).copied() {
            return Ok(id);
        }

        let program = Parser::new(&normalized).parse()?;
        let id = SizeExprId(
            u32::try_from(self.expressions.len())
                .map_err(|_| "too many size expressions in this Cx".to_string())?,
        );
        self.expressions.push(SizeExpression {
            source: normalized.clone(),
            program,
        });
        self.by_source.insert(normalized, id);
        Ok(id)
    }

    pub fn source(&self, id: SizeExprId) -> Option<&str> {
        self.expressions.get(id.0 as usize).map(|expr| expr.source.as_str())
    }

    pub fn len(&self) -> usize {
        self.expressions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.expressions.is_empty()
    }

    /// Returns whether evaluating `id` needs the current parent/container
    /// dimension. Viewport-relative expressions remain content-independent.
    pub fn requires_parent_or_container(&self, id: SizeExprId) -> bool {
        self.expressions.get(id.0 as usize).is_none_or(|expression| {
            expression.program.iter().any(|op| {
                matches!(
                    op,
                    Op::Relative(
                        SizeExprUnit::Parent | SizeExprUnit::Cqw | SizeExprUnit::Cqh,
                        _
                    )
                )
            })
        })
    }

    /// Evaluates a compiled program without allocation or synchronization.
    pub fn eval(&self, id: SizeExprId, context: SizeExprContext) -> f64 {
        let Some(expression) = self.expressions.get(id.0 as usize) else {
            return f64::NAN;
        };
        let mut stack = [0.0; SIZE_EXPR_STACK_CAP];
        let mut len = 0usize;
        for op in &expression.program {
            match *op {
                Op::Scalar(value) | Op::Length(value) => push(&mut stack, &mut len, value),
                Op::Relative(unit, factor) => {
                    push(&mut stack, &mut len, context.base(unit) * factor)
                }
                Op::Neg => unary(&mut stack, len, |value| -value),
                Op::Add => binary(&mut stack, &mut len, |a, b| a + b),
                Op::Sub => binary(&mut stack, &mut len, |a, b| a - b),
                Op::Mul => binary(&mut stack, &mut len, |a, b| a * b),
                Op::Div => binary(&mut stack, &mut len, |a, b| {
                    if b == 0.0 { f64::NAN } else { a / b }
                }),
                Op::Min => binary(&mut stack, &mut len, finite_min),
                Op::Max => binary(&mut stack, &mut len, finite_max),
                Op::Clamp => {
                    if len < 3 {
                        return f64::NAN;
                    }
                    let max = stack[len - 1];
                    let value = stack[len - 2];
                    let min = stack[len - 3];
                    len -= 2;
                    stack[len - 1] = if min.is_finite() && value.is_finite() && max.is_finite() {
                        value.max(min).min(max.max(min))
                    } else {
                        f64::NAN
                    };
                }
            }
        }
        if len == 1 && stack[0].is_finite() {
            stack[0]
        } else {
            f64::NAN
        }
    }

    fn classify(&self, id: SizeExprId) -> SizeExprSimple {
        let program = &self.expressions[id.0 as usize].program;
        match program.as_slice() {
            [Op::Scalar(value)] | [Op::Length(value)] => SizeExprSimple::Abs(*value),
            [Op::Relative(unit, factor)] => SizeExprSimple::Rel {
                unit: *unit,
                factor: *factor,
            },
            [Op::Scalar(value), Op::Neg] | [Op::Length(value), Op::Neg] => {
                SizeExprSimple::Abs(-*value)
            }
            [Op::Relative(unit, factor), Op::Neg] => SizeExprSimple::Rel {
                unit: *unit,
                factor: -*factor,
            },
            _ => SizeExprSimple::Compound(id),
        }
    }
}

#[inline]
fn push(stack: &mut [f64; SIZE_EXPR_STACK_CAP], len: &mut usize, value: f64) {
    if *len < SIZE_EXPR_STACK_CAP {
        stack[*len] = value;
        *len += 1;
    }
}

#[inline]
fn unary(stack: &mut [f64; SIZE_EXPR_STACK_CAP], len: usize, op: impl FnOnce(f64) -> f64) {
    if len != 0 {
        let value = stack[len - 1];
        stack[len - 1] = if value.is_finite() { op(value) } else { f64::NAN };
    }
}

#[inline]
fn binary(
    stack: &mut [f64; SIZE_EXPR_STACK_CAP],
    len: &mut usize,
    op: impl FnOnce(f64, f64) -> f64,
) {
    if *len < 2 {
        *len = 0;
        return;
    }
    let right = stack[*len - 1];
    let left = stack[*len - 2];
    *len -= 1;
    stack[*len - 1] = if left.is_finite() && right.is_finite() {
        let value = op(left, right);
        if value.is_finite() { value } else { f64::NAN }
    } else {
        f64::NAN
    };
}

#[inline]
fn finite_min(a: f64, b: f64) -> f64 {
    if a.is_finite() && b.is_finite() { a.min(b) } else { f64::NAN }
}

#[inline]
fn finite_max(a: f64, b: f64) -> f64 {
    if a.is_finite() && b.is_finite() { a.max(b) } else { f64::NAN }
}

fn normalize_source(source: &str) -> String {
    source.trim().to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dimension {
    Scalar,
    Length,
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    program: Vec<Op>,
    stack_depth: usize,
    max_stack_depth: usize,
    nesting: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            program: Vec::new(),
            stack_depth: 0,
            max_stack_depth: 0,
            nesting: 0,
        }
    }

    fn parse(mut self) -> Result<Vec<Op>, String> {
        self.skip_ws();
        let dimension = if self.consume_ident("calc") {
            self.expect(b'(')?;
            self.with_nesting(|parser| {
                let dimension = parser.parse_add_sub()?;
                parser.expect(b')')?;
                Ok(dimension)
            })?
        } else {
            self.parse_add_sub()?
        };
        self.skip_ws();
        if self.pos != self.source.len() {
            return Err(self.error("unexpected trailing input"));
        }
        // A plain scalar result is interpreted as logical pixels.
        let _final_dimension = match dimension {
            Dimension::Scalar | Dimension::Length => Dimension::Length,
        };
        if self.stack_depth != 1 {
            return Err(self.error("invalid evaluator stack"));
        }
        Ok(self.program)
    }

    fn parse_add_sub(&mut self) -> Result<Dimension, String> {
        let mut left = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            let op = if self.consume(b'+') {
                Op::Add
            } else if self.consume(b'-') {
                Op::Sub
            } else {
                break;
            };
            let right = self.parse_mul_div()?;
            if left != right {
                return Err(self.error("addition and subtraction require matching dimensions"));
            }
            self.binary_op(op)?;
            left = right;
        }
        Ok(left)
    }

    fn parse_mul_div(&mut self) -> Result<Dimension, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.consume(b'*') {
                let right = self.parse_unary()?;
                left = match (left, right) {
                    (Dimension::Length, Dimension::Length) => {
                        return Err(self.error("cannot multiply two lengths"));
                    }
                    (Dimension::Length, Dimension::Scalar)
                    | (Dimension::Scalar, Dimension::Length) => Dimension::Length,
                    (Dimension::Scalar, Dimension::Scalar) => Dimension::Scalar,
                };
                self.binary_op(Op::Mul)?;
            } else if self.consume(b'/') {
                let right = self.parse_unary()?;
                if right != Dimension::Scalar {
                    return Err(self.error("a divisor must be unitless"));
                }
                self.binary_op(Op::Div)?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Dimension, String> {
        let mut sign_count = 0;
        let mut negate = false;
        loop {
            self.skip_ws();
            if self.consume(b'-') {
                negate = !negate;
            } else if !self.consume(b'+') {
                break;
            }
            sign_count += 1;
            if sign_count > SIZE_EXPR_STACK_CAP {
                return Err(self.error("expression unary prefix exceeds parser limit"));
            }
        }
        let dimension = self.parse_primary()?;
        if negate {
            self.program.push(Op::Neg);
        }
        Ok(dimension)
    }

    fn parse_primary(&mut self) -> Result<Dimension, String> {
        self.skip_ws();
        if self.consume(b'(') {
            return self.with_nesting(|parser| {
                let dimension = parser.parse_add_sub()?;
                parser.expect(b')')?;
                Ok(dimension)
            });
        }

        if self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            let name = self.parse_ident().to_string();
            return self.parse_function(&name);
        }

        let number = self.parse_number()?;
        let unit_start = self.pos;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'%') {
            self.pos += 1;
        }
        let unit = &self.source[unit_start..self.pos];
        match unit {
            "" => {
                self.push_op(Op::Scalar(number))?;
                Ok(Dimension::Scalar)
            }
            "px" => {
                self.push_op(Op::Length(number))?;
                Ok(Dimension::Length)
            }
            "%" => {
                self.push_op(Op::Relative(SizeExprUnit::Parent, number * 0.01))?;
                Ok(Dimension::Length)
            }
            "vw" => {
                self.push_op(Op::Relative(SizeExprUnit::Vw, number * 0.01))?;
                Ok(Dimension::Length)
            }
            "vh" => {
                self.push_op(Op::Relative(SizeExprUnit::Vh, number * 0.01))?;
                Ok(Dimension::Length)
            }
            "cqw" => {
                self.push_op(Op::Relative(SizeExprUnit::Cqw, number * 0.01))?;
                Ok(Dimension::Length)
            }
            "cqh" => {
                self.push_op(Op::Relative(SizeExprUnit::Cqh, number * 0.01))?;
                Ok(Dimension::Length)
            }
            _ => Err(self.error("unknown size unit")),
        }
    }

    fn parse_function(&mut self, name: &str) -> Result<Dimension, String> {
        self.expect(b'(')?;
        self.with_nesting(|parser| match name {
            "min" | "max" => {
                let dimension = parser.parse_add_sub()?;
                let mut count = 1;
                while parser.consume_comma() {
                    let next = parser.parse_add_sub()?;
                    if next != dimension {
                        return Err(parser.error("function arguments require matching dimensions"));
                    }
                    parser.binary_op(if name == "min" { Op::Min } else { Op::Max })?;
                    count += 1;
                }
                if count < 2 {
                    return Err(parser.error("min/max require at least two arguments"));
                }
                parser.expect(b')')?;
                Ok(dimension)
            }
            "clamp" => {
                let min = parser.parse_add_sub()?;
                parser.expect_comma()?;
                let value = parser.parse_add_sub()?;
                parser.expect_comma()?;
                let max = parser.parse_add_sub()?;
                if min != value || value != max {
                    return Err(parser.error("clamp arguments require matching dimensions"));
                }
                parser.expect(b')')?;
                if parser.stack_depth < 3 {
                    return Err(parser.error("invalid clamp evaluator stack"));
                }
                parser.stack_depth -= 2;
                parser.program.push(Op::Clamp);
                Ok(value)
            }
            "calc" => Err(parser.error("calc is only allowed at the top level")),
            _ => Err(parser.error("unknown size function")),
        })
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        self.skip_ws();
        let start = self.pos;
        let mut digits = false;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.pos += 1;
            digits = true;
        }
        if self.consume(b'.') {
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.pos += 1;
                digits = true;
            }
        }
        if !digits {
            return Err(self.error("expected a number"));
        }
        if self.peek().is_some_and(|byte| byte == b'e' || byte == b'E') {
            self.pos += 1;
            if self.peek().is_some_and(|byte| byte == b'+' || byte == b'-') {
                self.pos += 1;
            }
            let exponent_start = self.pos;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.pos += 1;
            }
            if exponent_start == self.pos {
                return Err(self.error("expected exponent digits"));
            }
        }
        let value = self.source[start..self.pos]
            .parse::<f64>()
            .map_err(|_| self.error("invalid number"))?;
        if !value.is_finite() {
            return Err(self.error("numbers must be finite"));
        }
        Ok(value)
    }

    fn push_op(&mut self, op: Op) -> Result<(), String> {
        self.stack_depth += 1;
        self.max_stack_depth = self.max_stack_depth.max(self.stack_depth);
        if self.max_stack_depth > SIZE_EXPR_STACK_CAP {
            return Err(self.error("expression exceeds evaluator stack limit"));
        }
        self.program.push(op);
        Ok(())
    }

    fn binary_op(&mut self, op: Op) -> Result<(), String> {
        if self.stack_depth < 2 {
            return Err(self.error("invalid binary expression"));
        }
        self.stack_depth -= 1;
        self.program.push(op);
        Ok(())
    }

    fn with_nesting<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.nesting += 1;
        if self.nesting > SIZE_EXPR_STACK_CAP {
            return Err(self.error("expression nesting exceeds parser limit"));
        }
        let result = parse(self);
        self.nesting -= 1;
        result
    }

    fn consume_ident(&mut self, expected: &str) -> bool {
        let saved = self.pos;
        let actual = self.parse_ident();
        if actual == expected {
            true
        } else {
            self.pos = saved;
            false
        }
    }

    fn parse_ident(&mut self) -> &str {
        self.skip_ws();
        let start = self.pos;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        &self.source[start..self.pos]
    }

    fn consume_comma(&mut self) -> bool {
        self.skip_ws();
        self.consume(b',')
    }

    fn expect_comma(&mut self) -> Result<(), String> {
        if self.consume_comma() {
            Ok(())
        } else {
            Err(self.error("expected ','"))
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        self.skip_ws();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", expected as char)))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn error(&self, message: &str) -> String {
        format!("{message} at byte {} in {:?}", self.pos, self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> SizeExprContext {
        SizeExprContext {
            parent: 200.0,
            viewport_width: 1000.0,
            viewport_height: 800.0,
            container_width: 500.0,
            container_height: 300.0,
        }
    }

    #[test]
    fn precedence_units_functions_and_round_trip() {
        let mut store = SizeExprStore::default();
        let id = match store.intern(" calc(10px + 50% * 2) ").unwrap() {
            SizeExprSimple::Compound(id) => id,
            other => panic!("expected compound expression, got {other:?}"),
        };
        assert_eq!(store.eval(id, context()), 210.0);
        assert_eq!(store.source(id), Some("calc(10px + 50% * 2)"));

        let id = match store.intern("clamp(25vw, max(40px, 60cqw), 90vh)").unwrap() {
            SizeExprSimple::Compound(id) => id,
            other => panic!("expected compound expression, got {other:?}"),
        };
        assert_eq!(store.eval(id, context()), 300.0);
    }

    #[test]
    fn simple_values_dedupe_and_all_units() {
        let mut store = SizeExprStore::default();
        assert_eq!(store.intern("240").unwrap(), SizeExprSimple::Abs(240.0));
        assert_eq!(store.intern("240px").unwrap(), SizeExprSimple::Abs(240.0));
        assert_eq!(store.intern("50%").unwrap(), SizeExprSimple::Rel { unit: SizeExprUnit::Parent, factor: 0.5 });
        assert_eq!(store.intern("25vw").unwrap(), SizeExprSimple::Rel { unit: SizeExprUnit::Vw, factor: 0.25 });
        assert_eq!(store.intern("40vh").unwrap(), SizeExprSimple::Rel { unit: SizeExprUnit::Vh, factor: 0.4 });
        assert_eq!(store.intern("60cqw").unwrap(), SizeExprSimple::Rel { unit: SizeExprUnit::Cqw, factor: 0.6 });
        assert_eq!(store.intern("30cqh").unwrap(), SizeExprSimple::Rel { unit: SizeExprUnit::Cqh, factor: 0.3 });
        let before = store.len();
        let first = store.intern("10px + 5px").unwrap();
        let after_first = store.len();
        let second = store.intern("10px + 5px").unwrap();
        assert_eq!(first, second);
        assert_eq!(store.len(), after_first);
        assert_eq!(after_first, before + 1);
    }

    #[test]
    fn dimensions_errors_stack_cap_and_nonfinite_eval() {
        let mut store = SizeExprStore::default();
        assert!(store.intern("10px + 2").is_err());
        assert!(store.intern("10px * 2px").is_err());
        assert!(store.intern("10 / 2px").is_err());
        assert!(store.intern("min(1px)").is_err());
        assert!(store.intern("var(--x)").is_err());
        assert!(store.intern("Line").is_err());
        assert!(store.intern("Unused").is_err());

        let too_deep = format!("{}1px{}", "(".repeat(SIZE_EXPR_STACK_CAP + 1), ")".repeat(SIZE_EXPR_STACK_CAP + 1));
        assert!(store.intern(&too_deep).is_err());

        let too_many_signs = format!("{}1px", "-".repeat(100_000));
        assert!(store
            .intern(&too_many_signs)
            .unwrap_err()
            .contains("unary prefix exceeds parser limit"));
        assert_eq!(store.intern("---10px").unwrap(), SizeExprSimple::Abs(-10.0));
        assert_eq!(store.intern("--+10px").unwrap(), SizeExprSimple::Abs(10.0));

        let mut too_wide = "1px".to_string();
        for _ in 0..SIZE_EXPR_STACK_CAP {
            too_wide = format!("1px + ({too_wide})");
        }
        assert!(store
            .intern(&too_wide)
            .unwrap_err()
            .contains("evaluator stack limit"));

        let id = match store.intern("10px / 0").unwrap() {
            SizeExprSimple::Compound(id) => id,
            other => panic!("expected compound expression, got {other:?}"),
        };
        assert!(store.eval(id, context()).is_nan());
        let id = match store.intern("10px + 50%").unwrap() {
            SizeExprSimple::Compound(id) => id,
            other => panic!("expected compound expression, got {other:?}"),
        };
        assert!(store.eval(id, SizeExprContext { parent: f64::NAN, ..context() }).is_nan());
    }

    #[test]
    fn invalid_handle_never_aliases_the_first_program() {
        let mut store = SizeExprStore::default();
        let first = store.intern_id("10px").unwrap();
        assert_eq!(first, SizeExprId(0));
        assert_ne!(SizeExprId::default(), first);
        assert!(store.eval(SizeExprId::default(), context()).is_nan());
        assert!(store.eval(SizeExprId(999), context()).is_nan());
        assert_eq!(store.source(SizeExprId::default()), None);
    }
}
