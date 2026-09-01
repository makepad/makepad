//! A small spreadsheet engine for the Sheets tab: raw cell inputs, a
//! recursive-descent formula parser (numbers, strings, cell refs, ranges,
//! functions, arithmetic and `&` concat), memoized evaluation with cycle
//! detection.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Empty,
    Num(f64),
    Text(String),
    Err(&'static str),
}

impl CellValue {
    pub fn display(&self) -> String {
        match self {
            CellValue::Empty => String::new(),
            CellValue::Num(n) => format_num(*n),
            CellValue::Text(s) => s.clone(),
            CellValue::Err(e) => e.to_string(),
        }
    }

    #[cfg(test)]
    pub fn is_err(&self) -> bool {
        matches!(self, CellValue::Err(_))
    }
}

pub fn format_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        let i = n as i64;
        // thousands separators for readability
        let s = i.abs().to_string();
        let mut out = String::new();
        let bytes = s.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 && (bytes.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(*b as char);
        }
        if n < 0.0 {
            format!("-{out}")
        } else {
            out
        }
    } else {
        let s = format!("{:.4}", n);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    }
}

#[derive(Default, Clone)]
pub struct CellFormat {
    pub bold: bool,
    /// None = auto (numbers right, text left); Some(0/0.5/1)
    pub align: Option<f64>,
    /// index into the palette used by the toolbar; 0 = none
    pub bg: usize,
}

#[derive(Default)]
pub struct Sheet {
    pub inputs: HashMap<(usize, usize), String>,
    pub formats: HashMap<(usize, usize), CellFormat>,
    cache: HashMap<(usize, usize), CellValue>,
}

impl Sheet {
    pub fn set_input(&mut self, row: usize, col: usize, input: &str) {
        let input = input.trim_end();
        if input.is_empty() {
            self.inputs.remove(&(row, col));
        } else {
            self.inputs.insert((row, col), input.to_string());
        }
        self.cache.clear();
    }

    pub fn input(&self, row: usize, col: usize) -> &str {
        self.inputs
            .get(&(row, col))
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub fn format(&self, row: usize, col: usize) -> CellFormat {
        self.formats.get(&(row, col)).cloned().unwrap_or_default()
    }

    pub fn format_mut(&mut self, row: usize, col: usize) -> &mut CellFormat {
        self.formats.entry((row, col)).or_default()
    }

    pub fn value(&mut self, row: usize, col: usize) -> CellValue {
        if let Some(v) = self.cache.get(&(row, col)) {
            return v.clone();
        }
        let mut visiting = HashSet::new();
        let v = self.eval_cell(row, col, &mut visiting);
        self.cache.insert((row, col), v.clone());
        v
    }

    fn eval_cell(
        &mut self,
        row: usize,
        col: usize,
        visiting: &mut HashSet<(usize, usize)>,
    ) -> CellValue {
        if let Some(v) = self.cache.get(&(row, col)) {
            return v.clone();
        }
        let Some(input) = self.inputs.get(&(row, col)).cloned() else {
            return CellValue::Empty;
        };
        if !visiting.insert((row, col)) {
            return CellValue::Err("#CYCLE");
        }
        let v = if let Some(formula) = input.strip_prefix('=') {
            let mut parser = Parser::new(formula);
            match parser.parse_expr() {
                Ok(expr) => {
                    if parser.at_end() {
                        self.eval_expr(&expr, visiting)
                    } else {
                        CellValue::Err("#PARSE")
                    }
                }
                Err(_) => CellValue::Err("#PARSE"),
            }
        } else if let Ok(n) = input.trim().parse::<f64>() {
            CellValue::Num(n)
        } else {
            CellValue::Text(input.clone())
        };
        visiting.remove(&(row, col));
        let v = match v {
            CellValue::Empty => CellValue::Empty,
            other => other,
        };
        self.cache.insert((row, col), v.clone());
        v
    }

    fn eval_expr(&mut self, expr: &Expr, visiting: &mut HashSet<(usize, usize)>) -> CellValue {
        match expr {
            Expr::Num(n) => CellValue::Num(*n),
            Expr::Str(s) => CellValue::Text(s.clone()),
            Expr::Ref(row, col) => match self.eval_cell(*row, *col, visiting) {
                CellValue::Empty => CellValue::Num(0.0),
                v => v,
            },
            Expr::Range(..) => CellValue::Err("#RANGE"),
            Expr::Unary(inner) => match self.eval_expr(inner, visiting) {
                CellValue::Num(n) => CellValue::Num(-n),
                CellValue::Err(e) => CellValue::Err(e),
                _ => CellValue::Err("#VALUE"),
            },
            Expr::Binary(op, a, b) => {
                let va = self.eval_expr(a, visiting);
                let vb = self.eval_expr(b, visiting);
                if let CellValue::Err(e) = va {
                    return CellValue::Err(e);
                }
                if let CellValue::Err(e) = vb {
                    return CellValue::Err(e);
                }
                if *op == '&' {
                    return CellValue::Text(format!("{}{}", va.display(), vb.display()));
                }
                let (CellValue::Num(na), CellValue::Num(nb)) = (&va, &vb) else {
                    return CellValue::Err("#VALUE");
                };
                let (na, nb) = (*na, *nb);
                let n = match op {
                    '+' => na + nb,
                    '-' => na - nb,
                    '*' => na * nb,
                    '/' => {
                        if nb == 0.0 {
                            return CellValue::Err("#DIV/0");
                        }
                        na / nb
                    }
                    '^' => na.powf(nb),
                    _ => return CellValue::Err("#OP"),
                };
                CellValue::Num(n)
            }
            Expr::Call(name, args) => self.eval_call(name, args, visiting),
        }
    }

    fn eval_call(
        &mut self,
        name: &str,
        args: &[Expr],
        visiting: &mut HashSet<(usize, usize)>,
    ) -> CellValue {
        // Collect numeric values from args, flattening ranges.
        let mut nums = Vec::new();
        let mut collect = |sheet: &mut Sheet,
                           expr: &Expr,
                           visiting: &mut HashSet<(usize, usize)>|
         -> Result<(), &'static str> {
            match expr {
                Expr::Range((r0, c0), (r1, c1)) => {
                    let (r0, r1) = (*r0.min(r1), *r0.max(r1));
                    let (c0, c1) = (*c0.min(c1), *c0.max(c1));
                    if (r1 - r0 + 1) * (c1 - c0 + 1) > 100_000 {
                        return Err("#BIG");
                    }
                    for r in r0..=r1 {
                        for c in c0..=c1 {
                            match sheet.eval_cell(r, c, visiting) {
                                CellValue::Num(n) => nums.push(n),
                                CellValue::Err(e) => return Err(e),
                                _ => (),
                            }
                        }
                    }
                    Ok(())
                }
                other => match sheet.eval_expr(other, visiting) {
                    CellValue::Num(n) => {
                        nums.push(n);
                        Ok(())
                    }
                    CellValue::Empty => Ok(()),
                    CellValue::Err(e) => Err(e),
                    CellValue::Text(_) => Ok(()),
                },
            }
        };
        for arg in args {
            if let Err(e) = collect(self, arg, visiting) {
                return CellValue::Err(e);
            }
        }
        let name = name.to_ascii_uppercase();
        let v = match name.as_str() {
            "SUM" => nums.iter().sum::<f64>(),
            "AVG" | "AVERAGE" => {
                if nums.is_empty() {
                    return CellValue::Err("#DIV/0");
                }
                nums.iter().sum::<f64>() / nums.len() as f64
            }
            "MIN" => nums.iter().cloned().fold(f64::INFINITY, f64::min),
            "MAX" => nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            "COUNT" => nums.len() as f64,
            "ABS" => {
                if nums.len() != 1 {
                    return CellValue::Err("#ARGS");
                }
                nums[0].abs()
            }
            "SQRT" => {
                if nums.len() != 1 {
                    return CellValue::Err("#ARGS");
                }
                nums[0].sqrt()
            }
            "ROUND" => match nums.len() {
                1 => nums[0].round(),
                2 => {
                    let f = 10f64.powi(nums[1] as i32);
                    (nums[0] * f).round() / f
                }
                _ => return CellValue::Err("#ARGS"),
            },
            _ => return CellValue::Err("#NAME"),
        };
        if v.is_finite() {
            CellValue::Num(v)
        } else {
            CellValue::Err("#NUM")
        }
    }
}

// ---------------------------------------------------------------------------
// parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Expr {
    Num(f64),
    Str(String),
    Ref(usize, usize),
    Range((usize, usize), (usize, usize)),
    Unary(Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn at_end(&mut self) -> bool {
        self.peek().is_none()
    }

    fn parse_expr(&mut self) -> Result<Expr, ()> {
        // lowest precedence: & concat
        let mut lhs = self.parse_add()?;
        while self.peek() == Some(b'&') {
            self.pos += 1;
            let rhs = self.parse_add()?;
            lhs = Expr::Binary('&', Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_add(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_mul()?;
        while let Some(op) = self.peek() {
            if op == b'+' || op == b'-' {
                self.pos += 1;
                let rhs = self.parse_mul()?;
                lhs = Expr::Binary(op as char, Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_pow()?;
        while let Some(op) = self.peek() {
            if op == b'*' || op == b'/' {
                self.pos += 1;
                let rhs = self.parse_pow()?;
                lhs = Expr::Binary(op as char, Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<Expr, ()> {
        let lhs = self.parse_unary()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let rhs = self.parse_pow()?;
            return Ok(Expr::Binary('^', Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ()> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(Box::new(inner)));
        }
        if self.peek() == Some(b'+') {
            self.pos += 1;
            return self.parse_unary();
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, ()> {
        let Some(c) = self.peek() else {
            return Err(());
        };
        if c == b'(' {
            self.pos += 1;
            let inner = self.parse_expr()?;
            if self.peek() != Some(b')') {
                return Err(());
            }
            self.pos += 1;
            return Ok(inner);
        }
        if c == b'"' {
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                self.pos += 1;
            }
            if self.pos >= self.bytes.len() {
                return Err(());
            }
            let s = std::str::from_utf8(&self.bytes[start..self.pos])
                .map_err(|_| ())?
                .to_string();
            self.pos += 1;
            return Ok(Expr::Str(s));
        }
        if c.is_ascii_digit() || c == b'.' {
            let start = self.pos;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'.')
            {
                self.pos += 1;
            }
            let s = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| ())?;
            return s.parse::<f64>().map(Expr::Num).map_err(|_| ());
        }
        if c.is_ascii_alphabetic() {
            let start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphanumeric() {
                self.pos += 1;
            }
            let word = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| ())?;
            // function call?
            if self.peek() == Some(b'(') {
                self.pos += 1;
                let mut args = Vec::new();
                if self.peek() != Some(b')') {
                    loop {
                        let arg = self.parse_range_or_expr()?;
                        args.push(arg);
                        match self.peek() {
                            Some(b',') => {
                                self.pos += 1;
                            }
                            Some(b')') => break,
                            _ => return Err(()),
                        }
                    }
                }
                self.pos += 1;
                return Ok(Expr::Call(word.to_string(), args));
            }
            // cell reference
            if let Some((row, col)) = parse_ref(word) {
                return Ok(Expr::Ref(row, col));
            }
            return Err(());
        }
        Err(())
    }

    fn parse_range_or_expr(&mut self) -> Result<Expr, ()> {
        let save = self.pos;
        // try REF:REF
        if let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                let start = self.pos;
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphanumeric() {
                    self.pos += 1;
                }
                let word = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| ())?;
                if let Some(a) = parse_ref(word) {
                    if self.peek() == Some(b':') {
                        self.pos += 1;
                        self.skip_ws();
                        let start2 = self.pos;
                        while self.pos < self.bytes.len()
                            && self.bytes[self.pos].is_ascii_alphanumeric()
                        {
                            self.pos += 1;
                        }
                        let word2 =
                            std::str::from_utf8(&self.bytes[start2..self.pos]).map_err(|_| ())?;
                        if let Some(b) = parse_ref(word2) {
                            return Ok(Expr::Range(a, b));
                        }
                        return Err(());
                    }
                }
            }
        }
        self.pos = save;
        self.parse_expr()
    }
}

/// "B12" -> (row 11, col 1)
pub fn parse_ref(word: &str) -> Option<(usize, usize)> {
    let bytes = word.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 || i > 3 || i >= bytes.len() {
        return None;
    }
    let mut col = 0usize;
    for b in &bytes[0..i] {
        col = col * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1;
    }
    let row: usize = word[i..].parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// (row 11, col 1) -> "B12"
pub fn ref_name(row: usize, col: usize) -> String {
    let mut n = col;
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    letters.reverse();
    format!("{}{}", String::from_utf8(letters).unwrap(), row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet_with(cells: &[((usize, usize), &str)]) -> Sheet {
        let mut s = Sheet::default();
        for ((r, c), v) in cells {
            s.set_input(*r, *c, v);
        }
        s
    }

    #[test]
    fn arithmetic_and_refs() {
        let mut s = sheet_with(&[((0, 0), "2"), ((0, 1), "3"), ((1, 0), "=A1*B1+4")]);
        assert_eq!(s.value(1, 0), CellValue::Num(10.0));
    }

    #[test]
    fn sum_range_and_concat() {
        let mut s = sheet_with(&[
            ((0, 0), "1"),
            ((1, 0), "2"),
            ((2, 0), "3"),
            ((3, 0), "=SUM(A1:A3)"),
            ((4, 0), "=\"total: \"&A4"),
        ]);
        assert_eq!(s.value(3, 0), CellValue::Num(6.0));
        assert_eq!(s.value(4, 0), CellValue::Text("total: 6".into()));
    }

    #[test]
    fn cycle_detection() {
        let mut s = sheet_with(&[((0, 0), "=B1"), ((0, 1), "=A1")]);
        assert!(s.value(0, 0).is_err());
    }

    #[test]
    fn div_zero_and_funcs() {
        let mut s = sheet_with(&[
            ((0, 0), "=1/0"),
            ((0, 1), "=ROUND(2.456, 1)"),
            ((0, 2), "=MAX(1, 5, 3)"),
            ((0, 3), "=-3^2"),
        ]);
        assert_eq!(s.value(0, 0), CellValue::Err("#DIV/0"));
        assert_eq!(s.value(0, 1), CellValue::Num(2.5));
        assert_eq!(s.value(0, 2), CellValue::Num(5.0));
        // Excel-style precedence: unary minus binds tighter than ^
        assert_eq!(s.value(0, 3), CellValue::Num(9.0));
    }

    #[test]
    fn ref_names_roundtrip() {
        assert_eq!(parse_ref("B12"), Some((11, 1)));
        assert_eq!(ref_name(11, 1), "B12");
        assert_eq!(parse_ref("AA1"), Some((0, 26)));
        assert_eq!(ref_name(0, 26), "AA1");
    }
}
