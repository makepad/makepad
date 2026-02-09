use {
    super::{
        ast,
        ast::Op,
        char::CharExt,
        char_class,
        char_class::CharClass,
        error::{Error, Result},
        prog::Pred,
        range::Range,
        unicode,
    },
    std::mem,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Allow '.' to match newline characters.
    pub dot_all: bool,
    /// Ignore case when matching.
    pub ignore_case: bool,
    /// Allow '^' and '$' to match next to newline characters.
    pub multiline: bool,
}

#[derive(Debug)]
pub struct Allocs {
    expr_stack: Vec<Expr>,
    class_builder: char_class::Builder,
    ast_builder: ast::Builder,
}

impl Allocs {
    pub fn new() -> Self {
        Self {
            expr_stack: Vec::new(),
            class_builder: char_class::Builder::new(),
            ast_builder: ast::Builder::new(),
        }
    }
}

pub fn parse(pattern: &str, options: Options, allocs: &mut Allocs) -> Result<Vec<Op>> {
    Parser {
        pattern,
        pos: 0,
        dot_all: options.dot_all,
        ignore_case: options.ignore_case,
        multiline: options.multiline,
        next_cap_index: 1,
        expr_stack: &mut allocs.expr_stack,
        expr: Expr::new(Some(0)),
        char_class_builder: &mut allocs.class_builder,
        ast_builder: &mut allocs.ast_builder,
    }
    .parse()
}

#[derive(Debug)]
struct Parser<'a> {
    pattern: &'a str,
    pos: usize,
    dot_all: bool,
    ignore_case: bool,
    multiline: bool,
    next_cap_index: usize,
    expr_stack: &'a mut Vec<Expr>,
    expr: Expr,
    char_class_builder: &'a mut char_class::Builder,
    ast_builder: &'a mut ast::Builder,
}

impl<'a> Parser<'a> {
    fn parse(&mut self) -> Result<Vec<Op>> {
        loop {
            match self.peek() {
                Some('(') => {
                    self.skip();
                    let cap_index = if !self.skip_2_if(|c0, c1| (c0, c1) == ('?', ':')) {
                        let cap_index = self.next_cap_index;
                        self.next_cap_index += 1;
                        Some(cap_index)
                    } else {
                        None
                    };
                    self.push(cap_index);
                }
                Some(')') => {
                    self.skip();
                    self.pop()?;
                }
                Some('|') => {
                    self.skip();
                    self.alt();
                }
                Some('?') => {
                    self.skip();
                    let greedy = !self.skip_if(|c| c == '?');
                    if self.ast_builder.stack_depth() == 0 {
                        return Err(self.error(format!("missing operand")));
                    }
                    self.ast_builder.ques(greedy);
                }
                Some('*') => {
                    self.skip();
                    let greedy = !self.skip_if(|c| c == '?');
                    if self.ast_builder.stack_depth() == 0 {
                        return Err(self.error(format!("missing operand")));
                    }
                    self.ast_builder.star(greedy);
                }
                Some('+') => {
                    self.skip();
                    let greedy = !self.skip_if(|c| c == '?');
                    if self.ast_builder.stack_depth() == 0 {
                        return Err(self.error(format!("missing operand")));
                    }
                    self.ast_builder.plus(greedy);
                }
                Some('{') => {
                    if !self.maybe_parse_rep()? {
                        self.skip();
                        self.char('{');
                    }
                }
                Some('^') => {
                    self.skip();
                    self.assert(if self.multiline {
                        Pred::LineStart
                    } else {
                        Pred::TextStart
                    });
                }
                Some('$') => {
                    self.skip();
                    self.assert(if self.multiline {
                        Pred::LineEnd
                    } else {
                        Pred::TextEnd
                    });
                }
                Some('.') => {
                    self.skip();
                    let class = if self.dot_all {
                        CharClass::any()
                    } else {
                        self.char_class_builder.add_char(self.ignore_case, '\n');
                        self.char_class_builder.build(true)
                    };
                    self.char_class(class);
                }
                Some('[') => self.parse_class()?,
                Some('\\') => {
                    if self.maybe_parse_class_escape() {
                        let class = self.char_class_builder.build(false);
                        self.char_class(class);
                    } else {
                        let c = self.parse_char_escape()?;
                        if self.ignore_case {
                            self.char_class_builder.add_char(true, c);
                            let class = self.char_class_builder.build(false);
                            self.char_class(class);
                        } else {
                            self.char(c);
                        }
                    }
                }
                Some(c) => {
                    self.skip();
                    if self.ignore_case {
                        self.char_class_builder.add_char(true, c);
                        let class = self.char_class_builder.build(false);
                        self.char_class(class);
                    } else {
                        self.char(c);
                    }
                }
                None => break,
            }
        }
        self.alt();
        if !self.expr_stack.is_empty() {
            return Err(self.error(format!("unmatched '('")));
        }
        if self.expr.term_count == 0 {
            self.ast_builder.empty();
        }
        self.ast_builder.cap(self.expr.cap_index.unwrap());
        Ok(self.ast_builder.build())
    }

    fn maybe_parse_rep(&mut self) -> Result<bool> {
        let pos = self.pos;
        self.skip();
        let min = match self.parse_dec_int().ok() {
            Some(min) => min,
            None => {
                self.pos = pos;
                return Ok(false);
            }
        };
        let max = if self.skip_if(|c| c == ',') {
            if self.peek() != Some('}') {
                match self.parse_dec_int().ok() {
                    Some(max) => Some(max),
                    None => {
                        self.pos = pos;
                        return Ok(false);
                    }
                }
            } else {
                None
            }
        } else {
            Some(min)
        };
        if self.peek() != Some('}') {
            self.pos = pos;
            return Ok(false);
        }
        self.skip();
        let greedy = !self.skip_if(|c| c == '?');
        if max.map_or(false, |max| max < min) {
            return Err(self.error(format!("invalid max repetition count")));
        }
        if self.ast_builder.stack_depth() == 0 {
            return Err(self.error(format!("missing operand")));
        }
        self.ast_builder.rep(min, max, greedy);
        Ok(true)
    }

    fn parse_class(&mut self) -> Result<()> {
        self.skip();
        let negated = self.skip_if(|c| c == '^');
        let mut is_first = true;
        loop {
            match self.peek_2() {
                (Some(']'), _) if !is_first => {
                    self.skip();
                    break;
                }
                (Some('['), Some(':')) => {
                    self.parse_char_class_name()?;
                }
                (Some(_), _) => {
                    if !self.maybe_parse_class_escape() {
                        let range = self.parse_class_range()?;
                        self.char_class_builder.add_range(self.ignore_case, range);
                    }
                }
                (None, _) => return Err(self.error(format!("unmatched '['"))),
            }
            is_first = false;
        }
        let class = self.char_class_builder.build(negated);
        self.char_class(class);
        Ok(())
    }

    fn parse_char_class_name(&mut self) -> Result<()> {
        self.skip_2();
        let negated = self.skip_if(|c| c == '^');
        let start = self.pos;
        let end = loop {
            match self.peek_2() {
                (Some(':'), Some(']')) => {
                    let end = self.pos;
                    self.skip_2();
                    break end;
                }
                (Some(_), _) => self.skip(),
                (None, _) => return Err(self.error(format!("unmatched '[:'"))),
            }
        };
        if start == end {
            return Err(self.error(format!("empty char class name")));
        }
        let ranges = match &self.pattern[start..end] {
            "alnum" => unicode::ALNUM.as_slice(),
            "alpha" => unicode::ALPHA.as_slice(),
            "blank" => unicode::BLANK.as_slice(),
            "cntrl" => unicode::CNTRL.as_slice(),
            "digit" => unicode::DIGIT.as_slice(),
            "graph" => unicode::GRAPH.as_slice(),
            "lower" => unicode::LOWER.as_slice(),
            "print" => unicode::PRINT.as_slice(),
            "punct" => unicode::PUNCT.as_slice(),
            "space" => unicode::SPACE.as_slice(),
            "upper" => unicode::UPPER.as_slice(),
            "word" => unicode::WORD.as_slice(),
            "xdigit" => unicode::XDIGIT.as_slice(),
            _ => {
                return Err(self.error(format!(
                    "invalid char class name {}",
                    &self.pattern[start..end]
                )))
            }
        };
        self.char_class_builder
            .add_ranges(negated, self.ignore_case, ranges);
        Ok(())
    }

    fn maybe_parse_class_escape(&mut self) -> bool {
        match self.peek_2() {
            (Some('\\'), Some(c)) => {
                let group = match c {
                    'D' => Some((true, unicode::DIGIT.as_slice())),
                    'S' => Some((true, unicode::SPACE.as_slice())),
                    'W' => Some((true, unicode::WORD.as_slice())),
                    'd' => Some((false, unicode::DIGIT.as_slice())),
                    's' => Some((false, unicode::SPACE.as_slice())),
                    'w' => Some((false, unicode::WORD.as_slice())),
                    _ => None,
                };
                match group {
                    Some((negated, ranges)) => {
                        self.skip_2();
                        self.char_class_builder
                            .add_ranges(negated, self.ignore_case, ranges);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    fn parse_class_range(&mut self) -> Result<Range<char>> {
        let start = self.parse_class_char()?;
        match self.peek_2() {
            (Some('-'), c1) if c1 != Some(']') => {
                self.skip();
                let end = self.parse_class_char()?;
                return Ok(Range::new(start, end));
            }
            _ => Ok(Range::new(start, start)),
        }
    }

    fn parse_class_char(&mut self) -> Result<char> {
        match self.peek() {
            Some('\\') => self.parse_char_escape(),
            Some(c) => {
                self.skip();
                Ok(c)
            }
            _ => return Err(self.error(format!("expected character, got end of pattern"))),
        }
    }

    fn parse_char_escape(&mut self) -> Result<char> {
        self.skip();
        let c = match self.peek() {
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some(c) if !c.is_word() => c,
            Some(c) => return Err(self.error(format!("expected escape character, got `{}`", c))),
            None => {
                return Err(self.error(format!("expected escape character, got end of pattern")))
            }
        };
        self.skip();
        Ok(c)
    }

    fn parse_dec_int(&mut self) -> Result<u32> {
        let c = match self.peek() {
            Some(c) if c.is_digit(10) => c,
            Some(c) => return Err(self.error(format!("expected decimal digit, got `{}`", c))),
            None => return Err(self.error(format!("expected decimal digit, got end of pattern"))),
        };
        self.skip();
        let mut value = c.to_digit(10).unwrap();
        loop {
            let c = match self.peek() {
                Some(c) if c.is_digit(10) => {
                    self.skip();
                    c
                }
                _ => break,
            };
            value = match value.checked_mul(10) {
                Some(value) => value,
                None => return Err(self.error(format!("integer overflow"))),
            } + c.to_digit(10).unwrap();
        }
        Ok(value)
    }

    fn parse_hex_int(&mut self) -> Result<u32> {
        let c = match self.peek() {
            Some(c) if c.is_digit(16) => {
                self.skip();
                c
            }
            Some(c) => return Err(self.error(format!("expected hexadecimal digit, got `{}`", c))),
            None => {
                return Err(self.error(format!("expected hexadecimal digit, got end of pattern")))
            }
        };
        let mut value = c.to_digit(16).unwrap();
        loop {
            let c = match self.peek() {
                Some(c) if c.is_digit(16) => {
                    self.skip();
                    c
                }
                _ => break,
            };
            value = 16 * value + c.to_digit(16).unwrap();
        }
        Ok(value)
    }

    fn peek(&self) -> Option<char> {
        self.pattern[self.pos..].chars().next()
    }

    fn peek_2(&self) -> (Option<char>, Option<char>) {
        let mut chars = self.pattern[self.pos..].chars();
        (chars.next(), chars.next())
    }

    fn skip(&mut self) {
        self.pos += self.peek().unwrap().len_utf8();
    }

    fn skip_if(&mut self, f: impl FnOnce(char) -> bool) -> bool {
        if self.peek().map_or(false, f) {
            self.skip();
            true
        } else {
            false
        }
    }

    fn skip_2(&mut self) {
        let (c0, c1) = self.peek_2();
        self.pos += c0.unwrap().len_utf8();
        self.pos += c1.unwrap().len_utf8();
    }

    fn skip_2_if(&mut self, f: impl FnOnce(char, char) -> bool) -> bool {
        let (c0, c1) = self.peek_2();
        if match (c0, c1) {
            (Some(c0), Some(c1)) => f(c0, c1),
            _ => false,
        } {
            self.skip_2();
            true
        } else {
            false
        }
    }

    fn push(&mut self, cap_index: Option<usize>) {
        self.cat();
        let expr = mem::replace(&mut self.expr, Expr::new(cap_index));
        self.expr_stack.push(expr);
    }

    fn pop(&mut self) -> Result<()> {
        self.alt();
        if self.expr.term_count == 0 {
            self.ast_builder.empty();
        }
        if let Some(index) = self.expr.cap_index {
            self.ast_builder.cap(index);
        }
        self.expr = match self.expr_stack.pop() {
            Some(expr) => expr,
            None => return Err(self.error(format!("unmatched '('"))),
        };
        self.expr.fact_count += 1;
        Ok(())
    }

    fn alt(&mut self) {
        self.cat();
        if self.expr.fact_count != 0 {
            self.expr.term_count += 1;
            self.expr.fact_count = 0;
        }
        if self.expr.term_count == 2 {
            self.ast_builder.alt();
            self.expr.term_count -= 1;
        }
    }

    fn cat(&mut self) {
        if self.expr.fact_count == 2 {
            self.ast_builder.cat();
            self.expr.fact_count -= 1;
        }
    }

    fn assert(&mut self, pred: Pred) {
        self.cat();
        self.expr.fact_count += 1;
        self.ast_builder.assert(pred);
    }

    fn char(&mut self, c: char) {
        self.cat();
        self.expr.fact_count += 1;
        self.ast_builder.char(c);
    }

    fn char_class(&mut self, class: CharClass) {
        self.cat();
        self.expr.fact_count += 1;
        self.ast_builder.char_class(class);
    }

    fn error(&self, message: String) -> Error {
        return Error {
            message,
            pos: self.pos,
        };
    }
}

#[derive(Debug)]
struct Expr {
    cap_index: Option<usize>,
    term_count: usize,
    fact_count: usize,
}

impl Expr {
    fn new(cap_index: Option<usize>) -> Self {
        Self {
            cap_index,
            term_count: 0,
            fact_count: 0,
        }
    }
}
