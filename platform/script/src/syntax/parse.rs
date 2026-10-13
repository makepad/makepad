//! Tokens to tree. See the module docs in `mod.rs`.

use super::*;
use crate::tokenizer::{lex, LexedToken, ScriptTokenTag};

/// Operators that continue an expression on the same line.
const BINARY: &[&str] = &[
    "==", "!=", "===", "!==", "<=", ">=", "&&", "||", "..", "...", "??", "+", "-", "*", "/", "%", "<", ">", "&", "|", "^", "<<", ">>", "++", "|?",
];
const ASSIGN: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=", ":=", "?="];
/// What follows a field name in an object.
const FIELD_OPS: &[&str] = &[":", ":=", "+:", "-:", ">:", "<:", "^:"];

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<LexedToken>,
    /// For each opening bracket token, the index of its closing one.
    close: Vec<Option<usize>>,
    at: usize,
}

type Res<T> = Result<T, SyntaxError>;

pub(super) fn parse_file(source: &str) -> Res<Node> {
    let mut p = Parser::new(source)?;
    let mut stmts = Vec::new();
    loop {
        p.skip_separators();
        if p.done() {
            break;
        }
        stmts.push(p.statement(true)?);
    }
    Ok(Node { span: 0..source.len(), kind: Kind::Block(Block { open: None, close: None, stmts }) })
}

/// Parse one value or statement (a fragment such as `keys([0, 1], [1, 2])`).
pub fn parse_value(text: &str) -> Res<Node> {
    let mut p = Parser::new(text)?;
    p.skip_separators();
    let node = p.statement(true)?;
    p.skip_separators();
    if !p.done() {
        return p.error("more text follows the value");
    }
    Ok(node)
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Res<Self> {
        let tokens = lex(source);
        let mut close = vec![None; tokens.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            match token.token().kind() {
                ScriptTokenTag::OpenCurly | ScriptTokenTag::OpenRound | ScriptTokenTag::OpenSquare => stack.push(index),
                ScriptTokenTag::CloseCurly | ScriptTokenTag::CloseRound | ScriptTokenTag::CloseSquare => {
                    let Some(open) = stack.pop() else {
                        return Err(SyntaxError { at: token.span().start, message: format!("this `{}` closes nothing", &source[token.span().clone()]) });
                    };
                    let matches = matches!(
                        (tokens[open].token().kind(), token.token().kind()),
                        (ScriptTokenTag::OpenCurly, ScriptTokenTag::CloseCurly)
                            | (ScriptTokenTag::OpenRound, ScriptTokenTag::CloseRound)
                            | (ScriptTokenTag::OpenSquare, ScriptTokenTag::CloseSquare)
                    );
                    if !matches {
                        return Err(SyntaxError { at: tokens[open].span().start, message: format!("this `{}` is never closed", &source[tokens[open].span().clone()]) });
                    }
                    close[open] = Some(index);
                }
                _ => {}
            }
        }
        if let Some(&open) = stack.first() {
            return Err(SyntaxError { at: tokens[open].span().start, message: format!("this `{}` is never closed", &source[tokens[open].span().clone()]) });
        }
        Ok(Parser { source, tokens, close, at: 0 })
    }

    // ---- token helpers

    fn here(&self) -> usize {
        self.tokens.get(self.at).map_or(self.source.len(), |t| t.span().start)
    }

    fn error<T>(&self, message: impl Into<String>) -> Res<T> {
        Err(SyntaxError { at: self.here(), message: message.into() })
    }

    fn text(&self, i: usize) -> &'a str {
        &self.source[self.tokens[i].span().clone()]
    }

    fn done(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn kind(&self, i: usize) -> Option<ScriptTokenTag> {
        self.tokens.get(i).map(|t| t.token().kind())
    }

    fn is_separator(&self, i: usize) -> bool {
        self.kind(i) == Some(ScriptTokenTag::Separator)
    }

    fn skip_separators(&mut self) {
        while self.is_separator(self.at) {
            self.at += 1;
        }
    }

    fn is_op(&self, i: usize, op: &str) -> bool {
        self.kind(i) == Some(ScriptTokenTag::Operator) && self.text(i) == op
    }

    fn is_word(&self, i: usize) -> bool {
        self.kind(i) == Some(ScriptTokenTag::Identifier)
    }

    fn is_kw(&self, i: usize, kw: &str) -> bool {
        self.is_word(i) && self.text(i) == kw
    }

    fn is_number(&self, i: usize) -> bool {
        self.tokens.get(i).is_some_and(|t| t.token().number_f64().is_some())
    }

    fn glued(&self, i: usize) -> bool {
        i > 0 && self.tokens[i - 1].span().end == self.tokens[i].span().start
    }

    fn newline_before(&self, i: usize) -> bool {
        self.tokens.get(i).is_some_and(|t| t.preceded_by_newline())
    }

    fn is_open(&self, i: usize, k: ScriptTokenTag) -> bool {
        self.kind(i) == Some(k)
    }

    fn end_of(&self, i: usize) -> usize {
        self.tokens[i].span().end
    }

    fn start_of(&self, i: usize) -> usize {
        self.tokens[i].span().start
    }

    fn last_end(&self) -> usize {
        self.end_of(self.at - 1)
    }

    // ---- statements

    fn statement(&mut self, allow_struct: bool) -> Res<Node> {
        if let Some((name, name_span, op, after)) = self.field_key(self.at) {
            // `name: value` as a statement (a file or block that is the body of an object).
            self.at = after;
            let value = self.expr(allow_struct)?;
            return Ok(Node { span: name_span.start..value.span.end, kind: Kind::Entry { name, name_span, op, value: Box::new(value) } });
        }
        if self.is_word(self.at) {
            let word = self.text(self.at);
            match word {
                "let" | "var" => return self.let_statement(),
                "use" => {
                    let start = self.start_of(self.at);
                    self.at += 1;
                    let path = self.expr(false)?;
                    return Ok(Node { span: start..path.span.end, kind: Kind::Use { path: Box::new(path) } });
                }
                "return" | "break" => {
                    let start = self.start_of(self.at);
                    let keyword = word.to_string();
                    self.at += 1;
                    let mut value = None;
                    if self.value_follows() {
                        value = Some(Box::new(self.expr(allow_struct)?));
                    }
                    let end = value.as_ref().map_or(self.end_of(self.at - 1), |v| v.span.end);
                    return Ok(Node { span: start..end, kind: Kind::Jump { keyword, value } });
                }
                "continue" => {
                    let span = self.tokens[self.at].span().clone();
                    self.at += 1;
                    return Ok(Node { span, kind: Kind::Jump { keyword: "continue".into(), value: None } });
                }
                _ => {}
            }
        }
        self.expr(allow_struct)
    }

    /// Does a value follow a `return` / `break`?
    fn value_follows(&self) -> bool {
        let Some(token) = self.tokens.get(self.at) else { return false };
        if token.preceded_by_newline() {
            return false;
        }
        !matches!(
            token.token().kind(),
            ScriptTokenTag::CloseCurly | ScriptTokenTag::CloseRound | ScriptTokenTag::CloseSquare | ScriptTokenTag::Separator
        )
    }

    fn let_statement(&mut self) -> Res<Node> {
        let start = self.start_of(self.at);
        let keyword = self.text(self.at).to_string();
        self.at += 1;
        if self.is_kw(self.at, "mut") {
            self.at += 1;
        }
        let pattern_start = self.here();
        let mut names = Vec::new();
        match self.kind(self.at) {
            Some(ScriptTokenTag::Identifier) => {
                names.push(self.text(self.at).to_string());
                self.at += 1;
            }
            Some(ScriptTokenTag::OpenSquare) | Some(ScriptTokenTag::OpenCurly) => {
                let close = self.close[self.at].unwrap();
                for i in self.at + 1..close {
                    if self.is_word(i) && !self.is_op(i - 1, ".") {
                        names.push(self.text(i).to_string());
                    }
                }
                self.at = close + 1;
            }
            _ => return self.error(format!("`{keyword}` must be followed by a name")),
        }
        // `let a: type = v`
        if self.is_op(self.at, ":") {
            self.at += 1;
            if self.is_word(self.at) {
                self.at += 1;
            }
        }
        let pattern_span = pattern_start..self.last_end();
        let mut value = None;
        if self.is_op(self.at, "=") {
            self.at += 1;
            value = Some(Box::new(self.expr(true)?));
        }
        let end = value.as_ref().map_or(pattern_span.end, |v| v.span.end);
        Ok(Node { span: start..end, kind: Kind::Let { keyword, names, pattern_span, value } })
    }

    // ---- expressions

    /// An expression: operand, operator sequence, assignment.
    /// A `-` is binary wherever it follows a value, as in the VM: `[1 -2]` is one
    /// element, `[1, -2]` two.
    fn expr(&mut self, allow_struct: bool) -> Res<Node> {
        let first = self.operand(allow_struct)?;
        // `for`, `while` and `loop` end the statement (the VM starts a new one after
        // their block: an operator on the next line does not continue it).
        if matches!(first.kind, Kind::For { .. } | Kind::While { .. } | Kind::Loop { .. }) {
            return Ok(first);
        }
        let start = first.span.start;
        let mut rest: Vec<Link> = Vec::new();
        loop {
            let Some(token) = self.tokens.get(self.at) else { break };
            // `a and b`, `a or b`, `a is b`.
            if token.token().kind() == ScriptTokenTag::Identifier && matches!(self.text(self.at), "and" | "or" | "is") && !token.preceded_by_newline() {
                let op_span = token.span().clone();
                let op = self.text(self.at).to_string();
                self.at += 1;
                let operand = self.operand(allow_struct)?;
                rest.push(Link { op, op_span, operand });
                continue;
            }
            if token.token().kind() != ScriptTokenTag::Operator {
                break;
            }
            let op = self.text(self.at);
            if BINARY.contains(&op) {
                let mut op_span = token.span().clone();
                self.at += 1;
                // `>>>`: the tokenizer reads it as `>>` then `>`.
                if op == ">>" && self.is_op(self.at, ">") && self.glued(self.at) {
                    op_span.end = self.end_of(self.at);
                    self.at += 1;
                }
                let op = self.source[op_span.clone()].to_string();
                let operand = self.operand(allow_struct)?;
                rest.push(Link { op, op_span, operand });
                continue;
            }
            if ASSIGN.contains(&op) && !token.preceded_by_newline() {
                let target = if rest.is_empty() { first } else { Node { span: start..rest.last().unwrap().operand.span.end, kind: Kind::Chain { first: Box::new(first), rest } } };
                let op_span = token.span().clone();
                self.at += 1;
                let value = self.expr(allow_struct)?;
                return Ok(Node { span: start..value.span.end, kind: Kind::Assign { target: Box::new(target), op: op.to_string(), op_span, value: Box::new(value) } });
            }
            break;
        }
        if rest.is_empty() {
            Ok(first)
        } else {
            let end = rest.last().unwrap().operand.span.end;
            Ok(Node { span: start..end, kind: Kind::Chain { first: Box::new(first), rest } })
        }
    }

    fn operand(&mut self, allow_struct: bool) -> Res<Node> {
        let Some(token) = self.tokens.get(self.at) else { return self.error("a value was expected, the text ended") };
        let index = self.at;
        let start = token.span().start;
        if token.token().kind() == ScriptTokenTag::Operator {
            let op = self.text(index);
            match op {
                "@" => {
                    if !(self.is_word(index + 1) && self.glued(index + 1)) {
                        return self.error("`@` must be followed by a name, like `@ease_out`");
                    }
                    let (name, end) = self.path(index + 1);
                    self.at = end;
                    return Ok(Node { span: start..self.end_of(end - 1), kind: Kind::Name(name) });
                }
                "-" if self.is_number(index + 1) && self.glued(index + 1) => {
                    let value = -self.tokens[index + 1].token().number_f64().unwrap();
                    self.at += 2;
                    let node = Node { span: start..self.end_of(index + 1), kind: Kind::Number(value) };
                    return self.postfix(node);
                }
                "-" | "!" | ".." | "~" => {
                    self.at += 1;
                    let inner = self.operand(allow_struct)?;
                    return Ok(Node { span: start..inner.span.end, kind: Kind::Unary { op: op.to_string(), operand: Box::new(inner) } });
                }
                "." if self.is_word(index + 1) => {
                    self.at += 1;
                    let (path, end) = self.path(self.at);
                    let ident = Node { span: self.start_of(self.at)..self.end_of(end - 1), kind: Kind::Ident(path) };
                    self.at = end;
                    let node = Node { span: start..ident.span.end, kind: Kind::Unary { op: ".".into(), operand: Box::new(ident) } };
                    return self.postfix(node);
                }
                "|" | "||" => return self.lambda(),
                other => return self.error(format!("a value was expected here, found `{other}`")),
            }
        }
        let node = self.primary(allow_struct)?;
        self.postfix(node)
    }

    /// `.name`, `(args)`, `[index]` after a value.
    fn postfix(&mut self, mut node: Node) -> Res<Node> {
        loop {
            let i = self.at;
            match self.kind(i) {
                Some(ScriptTokenTag::OpenRound) if self.glued(i) => {
                    node = self.call(node)?;
                }
                Some(ScriptTokenTag::OpenSquare) if self.glued(i) => {
                    let close_token = self.close[i].unwrap();
                    self.at = i + 1;
                    let index = self.expr(true)?;
                    self.skip_separators();
                    if self.at != close_token {
                        return self.error("one expression was expected inside `[...]`");
                    }
                    self.at = close_token + 1;
                    let (open, close) = (self.start_of(i), self.start_of(close_token));
                    node = Node { span: node.span.start..close + 1, kind: Kind::Index { base: Box::new(node), index: Box::new(index), open, close } };
                }
                Some(ScriptTokenTag::Operator) if (self.is_op(i, ".") || self.is_op(i, ".?")) && self.is_word(i + 1) && (self.glued(i) || self.newline_before(i)) && self.glued(i + 1) => {
                    let name_span = self.tokens[i + 1].span().clone();
                    let name = self.text(i + 1).to_string();
                    self.at = i + 2;
                    node = Node { span: node.span.start..name_span.end, kind: Kind::Member { base: Box::new(node), name, name_span } };
                }
                // `a?`: try-propagation.
                Some(ScriptTokenTag::Operator) if self.is_op(i, "?") && self.glued(i) => {
                    let end = self.end_of(i);
                    self.at = i + 1;
                    node = Node { span: node.span.start..end, kind: Kind::Unary { op: "?".into(), operand: Box::new(node) } };
                }
                _ => return Ok(node),
            }
        }
    }

    /// An identifier with `.`-separated parts (`mod.motion.Rect`, `mod.std.*`): the
    /// path and the index after it.
    fn path(&self, mut i: usize) -> (String, usize) {
        let mut path = self.text(i).to_string();
        i += 1;
        while self.is_op(i, ".") && self.glued(i) && self.glued(i + 1) && (self.is_word(i + 1) || self.is_op(i + 1, "*")) {
            path.push('.');
            path.push_str(self.text(i + 1));
            i += 2;
            if path.ends_with('*') {
                break;
            }
        }
        (path, i)
    }

    fn primary(&mut self, allow_struct: bool) -> Res<Node> {
        let index = self.at;
        let token = &self.tokens[index];
        let start = token.span().start;
        match token.token().kind() {
            ScriptTokenTag::String => {
                self.at += 1;
                Ok(Node { span: token.span().clone(), kind: Kind::Str(token.string().map(str::to_owned).unwrap_or_default()) })
            }
            ScriptTokenTag::StringUnfinished => self.error("this string is never closed"),
            ScriptTokenTag::Color => {
                self.at += 1;
                let text = self.text(index).trim_start_matches('#').trim_start_matches('x');
                Ok(Node { span: token.span().clone(), kind: Kind::Color(text.to_string()) })
            }
            _ if self.is_number(index) => {
                self.at += 1;
                Ok(Node { span: token.span().clone(), kind: Kind::Number(token.token().number_f64().unwrap()) })
            }
            ScriptTokenTag::RustValue => {
                self.at += 1;
                Ok(Node { span: token.span().clone(), kind: Kind::Ident(self.text(index).to_string()) })
            }
            ScriptTokenTag::OpenSquare => self.array(),
            ScriptTokenTag::OpenCurly => self.curly(),
            ScriptTokenTag::OpenRound => {
                let close_token = self.close[index].unwrap();
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_separators();
                    if self.at >= close_token {
                        break;
                    }
                    items.push(self.statement(true)?);
                    if self.at > close_token {
                        return self.error("an expression runs past its closing `)`");
                    }
                }
                self.at = close_token + 1;
                let close = self.start_of(close_token);
                Ok(Node { span: start..close + 1, kind: Kind::Paren { items, open: start, close } })
            }
            ScriptTokenTag::Identifier => {
                let word = self.text(index);
                match word {
                    "fn" => return self.function(),
                    "if" => return self.if_expr(),
                    "for" => return self.for_expr(),
                    "while" => {
                        self.at += 1;
                        let cond = self.expr(false)?;
                        let body = self.body()?;
                        return Ok(Node { span: start..body.span.end, kind: Kind::While { cond: Box::new(cond), body: Box::new(body) } });
                    }
                    "loop" => {
                        self.at += 1;
                        let body = self.body()?;
                        return Ok(Node { span: start..body.span.end, kind: Kind::Loop { body: Box::new(body) } });
                    }
                    "match" => return self.match_expr(),
                    "try" => {
                        self.at += 1;
                        let body = self.body_or_expr()?;
                        let mut handler = None;
                        if self.is_kw(self.at, "catch") {
                            self.at += 1;
                            handler = Some(Box::new(self.body_or_expr()?));
                        }
                        let end = handler.as_ref().map_or(body.span.end, |h| h.span.end);
                        return Ok(Node { span: start..end, kind: Kind::Try { body: Box::new(body), handler } });
                    }
                    "ok" if !self.is_op(index + 1, ":") => {
                        self.at += 1;
                        let body = self.body_or_expr()?;
                        return Ok(Node { span: start..body.span.end, kind: Kind::Ok { body: Box::new(body) } });
                    }
                    "let" | "var" => return self.let_statement(),
                    "use" | "return" | "break" | "continue" => return self.statement(allow_struct),
                    _ => {}
                }
                let (path, after) = self.path(index);
                self.at = after;
                let name_span = start..self.end_of(after - 1);
                let last = path.rsplit('.').next().unwrap_or(&path);
                match self.kind(self.at) {
                    Some(ScriptTokenTag::OpenCurly) => {
                        let glued = self.glued(self.at);
                        let typed = last.starts_with(|c: char| c.is_ascii_uppercase());
                        let construct = if allow_struct { glued || typed } else { glued && typed };
                        if construct && !(self.newline_before(self.at) && !typed) {
                            return self.object(Some(path), start);
                        }
                        Ok(Node { span: name_span, kind: Kind::Ident(path) })
                    }
                    _ => {
                        Ok(Node { span: name_span, kind: Kind::Ident(path) })
                    }
                }
            }
            _ => self.error(format!("a value was expected here, found `{}`", self.text(index))),
        }
    }

    fn call(&mut self, callee: Node) -> Res<Node> {
        let open_token = self.at;
        let close_token = self.close[open_token].unwrap();
        self.at += 1;
        let mut args = Vec::new();
        loop {
            self.skip_separators();
            if self.at >= close_token {
                break;
            }
            args.push(self.expr(true)?);
            if self.at > close_token {
                return self.error("an argument runs past its closing `)`");
            }
        }
        self.at = close_token + 1;
        let (open, close) = (self.start_of(open_token), self.start_of(close_token));
        Ok(Node { span: callee.span.start..close + 1, kind: Kind::Call { callee: Box::new(callee), args, open, close } })
    }

    fn array(&mut self) -> Res<Node> {
        let open_token = self.at;
        let close_token = self.close[open_token].unwrap();
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip_separators();
            if self.at >= close_token {
                break;
            }
            items.push(self.expr(true)?);
            if self.at > close_token {
                return self.error("an item runs past its closing `]`");
            }
        }
        self.at = close_token + 1;
        let (open, close) = (self.start_of(open_token), self.start_of(close_token));
        Ok(Node { span: open..close + 1, kind: Kind::Array { items, open, close } })
    }

    /// A `{` in value position: an object, unless its contents are statements.
    fn curly(&mut self) -> Res<Node> {
        let start = self.here();
        let open = self.at;
        let close = self.close[open].unwrap();
        // An object is empty, or starts with `name:` / `"name":`, or holds
        // only bare values. A block starts with a statement keyword or holds
        // statements; the first entry decides.
        let looks_like_block = close > open + 1 && {
            let first = open + 1;
            self.is_word(first) && self.field_key(first).is_none() && matches!(self.text(first), "let" | "var" | "return" | "break" | "continue" | "if" | "for" | "while" | "loop" | "use" | "match" | "try")
        };
        if looks_like_block {
            return self.block();
        }
        self.object(None, start)
    }

    /// `name:` / `a.b :=` / `"name" +:`: the key, its span, the operator and the token after it.
    fn field_key(&self, i: usize) -> Option<(String, Range<usize>, String, usize)> {
        let (name, after, span) = match self.kind(i)? {
            ScriptTokenTag::String => (self.tokens[i].string().unwrap_or("").to_string(), i + 1, self.tokens[i].span().clone()),
            ScriptTokenTag::Identifier => {
                if matches!(self.text(i), "let" | "var" | "fn" | "use" | "return" | "break" | "if" | "for" | "while" | "loop" | "match" | "try" | "ok" | "continue") && !self.is_op(i + 1, ":") {
                    return None;
                }
                let (path, after) = self.path(i);
                (path, after, self.start_of(i)..self.end_of(after - 1))
            }
            _ => return None,
        };
        if self.kind(after) != Some(ScriptTokenTag::Operator) {
            return None;
        }
        let op = self.text(after);
        if !FIELD_OPS.contains(&op) {
            return None;
        }
        if op == ":" && self.is_op(after + 1, ":") && self.glued(after + 1) {
            return None;
        }
        Some((name, span, op.to_string(), after + 1))
    }

    fn object(&mut self, ty: Option<String>, start: usize) -> Res<Node> {
        let open_token = self.at;
        let close_token = self.close[open_token].unwrap();
        self.at += 1;
        let mut fields = Vec::new();
        loop {
            self.skip_separators();
            if self.at >= close_token {
                break;
            }
            let entry_token = self.at;
            let entry = self.start_of(entry_token);
            let (name, name_span, op) = if let Some((name, span, op, after)) = self.field_key(entry_token) {
                self.at = after;
                (Some(name), span, op)
            } else {
                (None, entry..entry, String::new())
            };
            let value = if name.is_some() { self.expr(true)? } else { self.statement(true)? };
            if self.at > close_token {
                return self.error("an entry runs past its closing `}`");
            }
            fields.push(Field { name, name_span, op, span: entry..value.span.end, value });
        }
        self.at = close_token + 1;
        let (open, close) = (self.start_of(open_token), self.start_of(close_token));
        Ok(Node { span: start..close + 1, kind: Kind::Object { ty, fields, open, close } })
    }

    /// `{ statements }`.
    fn block(&mut self) -> Res<Node> {
        let open_token = self.at;
        if !self.is_open(open_token, ScriptTokenTag::OpenCurly) {
            return self.error("a `{` block was expected here");
        }
        let close_token = self.close[open_token].unwrap();
        self.at += 1;
        let mut stmts = Vec::new();
        loop {
            self.skip_separators();
            if self.at >= close_token {
                break;
            }
            stmts.push(self.statement(true)?);
            if self.at > close_token {
                return self.error("a statement runs past its closing `}`");
            }
        }
        self.at = close_token + 1;
        let (open, close) = (self.start_of(open_token), self.start_of(close_token));
        Ok(Node { span: open..close + 1, kind: Kind::Block(Block { open: Some(open), close: Some(close), stmts }) })
    }

    /// A `{` block (a construct's body).
    fn body(&mut self) -> Res<Node> {
        self.block()
    }

    fn body_or_expr(&mut self) -> Res<Node> {
        if self.is_open(self.at, ScriptTokenTag::OpenCurly) {
            self.block()
        } else {
            self.expr(false)
        }
    }

    fn function(&mut self) -> Res<Node> {
        let start = self.here();
        self.at += 1; // fn
        let mut name = None;
        let mut name_span = start..start;
        if self.is_word(self.at) {
            name = Some(self.text(self.at).to_string());
            name_span = self.tokens[self.at].span().clone();
            self.at += 1;
        }
        let mut params = Vec::new();
        if self.is_open(self.at, ScriptTokenTag::OpenRound) {
            let close = self.close[self.at].unwrap();
            params = self.params(self.at + 1, close);
            self.at = close + 1;
        }
        let mut ret = None;
        if self.is_op(self.at, "->") {
            self.at += 1;
            let from = self.here();
            // a return type: a name, maybe with generics / path
            if !self.is_word(self.at) {
                return self.error("a return type was expected after `->`");
            }
            let (_, end) = self.path(self.at);
            self.at = end;
            ret = Some(from..self.last_end());
        }
        let body = self.block()?;
        Ok(Node { span: start..body.span.end, kind: Kind::Fn { name, name_span, params, ret, body: Box::new(body) } })
    }

    /// Parameters in tokens `from..to`: `a`, `a: type`, separated by commas.
    fn params(&self, from: usize, to: usize) -> Vec<Param> {
        let mut out = Vec::new();
        let mut i = from;
        while i < to {
            if self.is_separator(i) {
                i += 1;
                continue;
            }
            let begin = i;
            let name = if self.is_word(i) { self.text(i).to_string() } else { self.text(i).to_string() };
            // To the next separator at this nesting.
            while i < to && !self.is_separator(i) {
                i = if self.close[i].is_some() { self.close[i].unwrap() + 1 } else { i + 1 };
            }
            out.push(Param { name, span: self.start_of(begin)..self.end_of(i - 1) });
        }
        out
    }

    /// `|a, b| body` / `|| body`.
    fn lambda(&mut self) -> Res<Node> {
        let start = self.here();
        let mut params = Vec::new();
        if self.is_op(self.at, "||") {
            self.at += 1;
        } else {
            self.at += 1; // |
            let from = self.at;
            while self.at < self.tokens.len() && !self.is_op(self.at, "|") {
                self.at += 1;
            }
            if self.at >= self.tokens.len() {
                return self.error("this closure's parameters are never closed with `|`");
            }
            params = self.params(from, self.at);
            self.at += 1;
        }
        let body = if self.is_open(self.at, ScriptTokenTag::OpenCurly) { self.block()? } else { self.expr(true)? };
        Ok(Node { span: start..body.span.end, kind: Kind::Fn { name: None, name_span: start..start, params, ret: None, body: Box::new(body) } })
    }

    fn if_expr(&mut self) -> Res<Node> {
        let start = self.here();
        self.at += 1; // if / elif
        let cond = self.expr(false)?;
        let then = self.body_or_expr_after_cond()?;
        let mut otherwise = None;
        if self.is_kw(self.at, "else") {
            self.at += 1;
            if self.is_kw(self.at, "if") {
                otherwise = Some(Box::new(self.if_expr()?));
            } else {
                otherwise = Some(Box::new(self.body_or_expr_after_cond()?));
            }
        } else if self.is_kw(self.at, "elif") {
            otherwise = Some(Box::new(self.if_expr()?));
        }
        let end = otherwise.as_ref().map_or(then.span.end, |o| o.span.end);
        Ok(Node { span: start..end, kind: Kind::If { cond: Box::new(cond), then: Box::new(then), otherwise } })
    }

    fn body_or_expr_after_cond(&mut self) -> Res<Node> {
        if self.is_open(self.at, ScriptTokenTag::OpenCurly) {
            self.block()
        } else {
            self.expr(true)
        }
    }

    fn for_expr(&mut self) -> Res<Node> {
        let start = self.here();
        self.at += 1;
        let binding_start = self.here();
        while self.at < self.tokens.len() && !self.is_kw(self.at, "in") {
            if self.is_open(self.at, ScriptTokenTag::OpenCurly) {
                return self.error("`for` needs `in` before its block");
            }
            self.at += 1;
        }
        if self.at >= self.tokens.len() {
            return self.error("`for` needs `in`");
        }
        let binding = binding_start..self.end_of(self.at - 1).max(binding_start);
        self.at += 1;
        let iter = self.expr(false)?;
        let body = self.block()?;
        Ok(Node { span: start..body.span.end, kind: Kind::For { binding, iter: Box::new(iter), body: Box::new(body) } })
    }

    fn match_expr(&mut self) -> Res<Node> {
        let start = self.here();
        self.at += 1;
        let subject = self.expr(false)?;
        if !self.is_open(self.at, ScriptTokenTag::OpenCurly) {
            return self.error("`match` needs a `{` block of arms");
        }
        let open_token = self.at;
        let close_token = self.close[open_token].unwrap();
        self.at += 1;
        let mut arms = Vec::new();
        loop {
            self.skip_separators();
            if self.at >= close_token {
                break;
            }
            let pattern = self.expr(true)?;
            if !self.is_op(self.at, "=>") {
                return self.error("a `match` arm needs `=>` after its pattern");
            }
            self.at += 1;
            let body = if self.is_open(self.at, ScriptTokenTag::OpenCurly) { self.block()? } else { self.statement(true)? };
            if self.at > close_token {
                return self.error("an arm runs past its closing `}`");
            }
            arms.push(Arm { pattern, body });
        }
        self.at = close_token + 1;
        let (open, close) = (self.start_of(open_token), self.start_of(close_token));
        Ok(Node { span: start..close + 1, kind: Kind::Match { subject: Box::new(subject), arms, open, close } })
    }
}
