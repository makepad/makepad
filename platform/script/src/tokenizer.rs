// Makepad script streaming tokenizer

use crate::colorhex::hex_bytes_to_u32;
use crate::heap::*;
use crate::makepad_live_id::LiveId;
use crate::makepad_live_id_macros::*;
use crate::value::*;

#[derive(Copy, Clone, Debug)]
pub enum ScriptToken {
    End,
    StreamEnd,
    Identifier(LiveId),
    Operator(LiveId),
    Separator(LiveId),
    OpenCurly,
    CloseCurly,
    OpenRound,
    CloseRound,
    OpenSquare,
    CloseSquare,
    StringUnfinished,
    String(ScriptValue),
    F32(f32),
    U32(u32),
    I32(i32),
    F16(f32),
    F64(f64),
    U40(u64),
    Color(u32),
    RustValue(u32),
}

impl ScriptToken {
    pub fn identifier(&self) -> LiveId {
        match self {
            ScriptToken::Identifier(id) => *id,
            _ => id!(),
        }
    }
    pub fn operator(&self) -> LiveId {
        match self {
            ScriptToken::Operator(id) => *id,
            _ => id!(),
        }
    }
    pub fn separator(&self) -> LiveId {
        match self {
            ScriptToken::Separator(id) => *id,
            _ => id!(),
        }
    }
    pub fn f64(&self) -> f64 {
        match self {
            ScriptToken::F64(v) => *v,
            _ => 0.0,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ScriptToken::F64(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_u40(&self) -> Option<u64> {
        match self {
            ScriptToken::U40(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            ScriptToken::F32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            ScriptToken::U32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            ScriptToken::I32(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_f16(&self) -> Option<f32> {
        match self {
            ScriptToken::F16(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_color(&self) -> Option<u32> {
        match self {
            ScriptToken::Color(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_string(&self) -> Option<ScriptValue> {
        match self {
            ScriptToken::String(v) => Some(*v),
            ScriptToken::StringUnfinished => Some(ScriptValue::EMPTY_STRING),
            _ => None,
        }
    }
    pub fn as_rust_value(&self) -> Option<u32> {
        match self {
            ScriptToken::RustValue(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_identifier(&self) -> bool {
        match self {
            ScriptToken::Identifier { .. } => true,
            _ => false,
        }
    }
    pub fn is_operator(&self) -> bool {
        match self {
            ScriptToken::Operator(_) => true,
            _ => false,
        }
    }
    pub fn is_open_curly(&self) -> bool {
        match self {
            ScriptToken::OpenCurly => true,
            _ => false,
        }
    }
    pub fn is_close_curly(&self) -> bool {
        match self {
            ScriptToken::CloseCurly => true,
            _ => false,
        }
    }
    pub fn is_open_round(&self) -> bool {
        match self {
            ScriptToken::OpenRound => true,
            _ => false,
        }
    }
    pub fn is_close_round(&self) -> bool {
        match self {
            ScriptToken::CloseRound => true,
            _ => false,
        }
    }
    pub fn is_open_square(&self) -> bool {
        match self {
            ScriptToken::OpenSquare => true,
            _ => false,
        }
    }
    pub fn is_close_square(&self) -> bool {
        match self {
            ScriptToken::CloseSquare => true,
            _ => false,
        }
    }
    pub fn is_string(&self) -> bool {
        match self {
            ScriptToken::StringUnfinished | ScriptToken::String(_) => true,
            _ => false,
        }
    }
    pub fn is_f64(&self) -> bool {
        match self {
            ScriptToken::F64(_) => true,
            _ => false,
        }
    }
    pub fn is_u40(&self) -> bool {
        match self {
            ScriptToken::U40(_) => true,
            _ => false,
        }
    }
    pub fn is_f32(&self) -> bool {
        match self {
            ScriptToken::F32(_) => true,
            _ => false,
        }
    }
    pub fn is_u32(&self) -> bool {
        match self {
            ScriptToken::U32(_) => true,
            _ => false,
        }
    }
    pub fn is_i32(&self) -> bool {
        match self {
            ScriptToken::I32(_) => true,
            _ => false,
        }
    }
    pub fn is_f16(&self) -> bool {
        match self {
            ScriptToken::F16(_) => true,
            _ => false,
        }
    }
    pub fn is_color(&self) -> bool {
        match self {
            ScriptToken::Color(_) => true,
            _ => false,
        }
    }
    pub fn is_rust_value(&self) -> bool {
        match self {
            ScriptToken::RustValue(_) => true,
            _ => false,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ScriptTokenPos {
    pub token: ScriptToken,
    pos: usize,
    /// True if whitespace containing a newline separated this token from the
    /// previous one. Lets the parser keep statements newline-delimited (a
    /// continuation token on a new line begins a new statement, Go/Swift-style).
    pub preceded_by_newline: bool,
}

/// One captured `/** ... */` doc annotation (see `ScriptTokenizer::docs`).
#[derive(Clone, Debug)]
pub struct ScriptTokDoc {
    /// Index the NEXT token gets (`tokens.len()` at capture end).
    pub next_token: u32,
    pub text: String,
}

#[derive(Default, Eq, PartialEq)]
enum State {
    #[default]
    Whitespace,
    Identifier,
    Operator,
    RustValue,
    String(bool),
    EscapeInString(bool),
    UnicodeHexInString(bool),
    UnicodeCurlyInString(bool),
    AsciiHexInString(bool),
    BlockComment(usize),
    MaybeEndBlock(usize),
    /// Just entered `/*`; a following `*` may open a `/**name*/` doc.
    BlockCommentStart,
    /// Saw `/**`; the next char decides doc (`/**x`), empty (`/**/`) or
    /// plain (`/***`, Rust convention).
    BlockDocStart,
    /// Inside `/**...*/`: text accumulates into `temp`.
    BlockDoc,
    /// Saw `*` inside a block doc; `/` closes it.
    BlockDocMaybeEnd,
    LineComment,
    Number,
    Color,
}

#[derive(Default)]
pub struct ScriptTokenizer {
    pos: usize,
    /// Set when a newline is consumed; stamped onto (and cleared by) the next
    /// emitted token as `preceded_by_newline`.
    newline_pending: bool,
    pub tokens: Vec<ScriptTokenPos>,
    /// Captured `/** ... */` doc annotations, keyed by the index the NEXT
    /// token gets (`tokens.len()` at capture end). ONE form; position
    /// determines meaning at resolution time (`docs::resolve_docs`):
    /// before `key:` it documents the field, before an object literal it
    /// documents the object, immediately before a value literal it names
    /// that value (`/**glow tint*/ #8f0`). The parser never sees these;
    /// `//` and `/* */` remain plain discarded comments.
    pub docs: Vec<ScriptTokDoc>,
    pub original: String,
    unfinished: String,
    temp: String,
    state: State,
}

pub struct ScriptLoc {
    pub row: usize,
    pub col: usize,
}

impl ScriptTokenizer {
    pub fn clear(&mut self) {
        self.pos = 0;
        self.newline_pending = false;
        self.tokens.clear();
        self.docs.clear();
        self.original.clear();
        self.unfinished.clear();
        self.temp.clear();
        self.state = State::Whitespace
    }

    /// Iterate over all string values in the token stream.
    /// Used by GC to mark tokenizer strings as roots.
    pub fn iter_strings(&self) -> impl Iterator<Item = ScriptValue> + '_ {
        self.tokens.iter().filter_map(|tp| {
            if let ScriptToken::String(v) = tp.token {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn token_index_to_row_col(&self, tok_index: u32) -> Option<(u32, u32)> {
        // first find the real pos

        let char_index = self.tokens[tok_index as usize].pos;

        let mut line = 0;
        let mut line_start = 0;
        for (i, c) in self.original.chars().enumerate() {
            if i >= char_index as usize {
                return Some((line as u32, (i - line_start) as u32));
            }
            if c == '\n' {
                line_start = i + 1;
                line += 1;
            }
        }
        None
    }

    pub fn dump_tokens(&self, heap: &ScriptHeap) {
        for i in 0..self.tokens.len() {
            match self.tokens[i].token {
                ScriptToken::End => print!("End"),
                ScriptToken::StreamEnd => print!("StreamEnd"),
                ScriptToken::Identifier(id) => print!("{id}"),
                ScriptToken::Operator(id) => print!("{id}"),
                ScriptToken::Separator(id) => print!("{id}"),
                ScriptToken::OpenCurly => print!("{{"),
                ScriptToken::CloseCurly => print!("}}"),
                ScriptToken::OpenRound => print!("("),
                ScriptToken::CloseRound => print!(")"),
                ScriptToken::OpenSquare => print!("["),
                ScriptToken::CloseSquare => print!("]"),
                ScriptToken::StringUnfinished => print!("\"\".."),
                ScriptToken::String(v) => {
                    let mut s = String::new();
                    heap.cast_to_string(v, &mut s);
                    print!("\"{}\"", s)
                }
                ScriptToken::U40(v) => print!("{v}"),
                ScriptToken::F64(v) => print!("{v}"),
                ScriptToken::F32(v) => print!("{v}"),
                ScriptToken::I32(v) => print!("{v}"),
                ScriptToken::U32(v) => print!("{v}"),
                ScriptToken::F16(v) => print!("{v}"),
                ScriptToken::Color(v) => print!("{:08x}", v),
                ScriptToken::RustValue(v) => print!("#({v})"),
            }
            print!(" ");
        }
        print!("\n");
    }

    /// Emits a token, stamping whether a newline preceded it (and clearing the
    /// pending flag). All token emission funnels through here.
    fn push_tok(&mut self, pos: usize, token: ScriptToken) {
        let preceded_by_newline = self.newline_pending;
        self.newline_pending = false;
        self.tokens.push(ScriptTokenPos { token, pos, preceded_by_newline });
    }

    /// Whether token `i` was preceded by a newline (see `preceded_by_newline`).
    pub fn token_preceded_by_newline(&self, i: u32) -> bool {
        self.tokens.get(i as usize).is_some_and(|t| t.preceded_by_newline)
    }


    pub fn pos_to_loc(&self, pos: usize) -> Option<ScriptLoc> {
        let mut row = 0;
        let mut col = 0;
        for (i, c) in self.original.chars().enumerate() {
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
            if i >= pos {
                return Some(ScriptLoc { row, col });
            }
        }
        None
    }

    fn emit_rust_value(&mut self) {
        let number = if let Ok(v) = self.temp.parse::<u32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::RustValue(number));
    }

    fn emit_f64(&mut self) {
        // Measure before clearing: a float token's position was taken from
        // an already-emptied `temp`, so it pointed one past its terminator —
        // a float ending a line resolved to the NEXT line, column 0.
        let len = self.temp.len();
        let number = if let Ok(v) = self.temp.parse::<f64>() {
            // allow the shader compiler to recognise the difference btween 1 and 1.
            if !(self.temp.contains('.') || self.temp.contains('e') || self.temp.contains('E'))
                && v <= 0xFF_FFFF_FFFFu64 as f64
            {
                self.temp.clear();
                self.push_tok(self.pos - len, ScriptToken::U40(v as u64));
                return;
            }
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F64(number));
    }

    fn emit_f32(&mut self) {
        let len = self.temp.len();
        let number = if let Ok(v) = self.temp.parse::<f32>() {
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F32(number));
    }

    fn emit_u32(&mut self) {
        let number = if let Ok(v) = self.temp.parse::<u32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::U32(number));
    }

    fn emit_i32(&mut self) {
        let len = self.temp.len();
        let number = if let Ok(v) = self.temp.parse::<i32>() {
            self.temp.clear();
            v
        } else {
            0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::I32(number));
    }

    fn emit_f16(&mut self) {
        let len = self.temp.len();
        let number = if let Ok(v) = self.temp.parse::<f32>() {
            self.temp.clear();
            v
        } else {
            0.0
        };
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::F16(number));
    }

    fn emit_identifier(&mut self) {
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Identifier(id));
    }

    fn emit_operator(&mut self) {
        if self.temp.len() == 0 {
            return;
        }
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Operator(id));
    }

    fn emit_separator(&mut self, c: char) {
        if self.temp.len() != 0 {
            panic!()
        }
        self.temp.push(c);
        let id = match LiveId::from_str_with_lut(&self.temp) {
            Err(str) => {
                println!(
                    "--WARNING-- LiveId LUT collision between {} and {}",
                    self.temp, str
                );
                LiveId::from_str(&self.temp)
            }
            Ok(id) => id,
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Separator(id));
    }

    fn emit_color(&mut self) {
        let color = match hex_bytes_to_u32(&self.temp.as_bytes()) {
            Err(()) => 0xff00ffff,
            Ok(color) => color,
        };
        let len = self.temp.len();
        self.temp.clear();
        self.push_tok(self.pos - len, ScriptToken::Color(color));
    }

    fn emit_token_here(&mut self, token: ScriptToken) {
        self.push_tok(self.pos, token)
    }

    fn append_unfinished_string(&mut self, c: char) {
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last_mut()
        {
            self.unfinished.push(c);
        } else {
            self.unfinished.clear();
            self.unfinished.push(c);
            self.push_tok(self.pos, ScriptToken::StringUnfinished);
        }
    }

    /// If the last token is `StringUnfinished`, intern the unfinished buffer content
    /// via the heap and return it as a ScriptValue. Does NOT modify the token — the
    /// tokenizer state remains unchanged for the next `tokenize()` call.
    /// Used at incremental parsing boundaries so the parser gets the real partial string.
    pub fn intern_unfinished_string(&mut self, heap: &mut ScriptHeap) -> Option<ScriptValue> {
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last()
        {
            Some(heap.new_string_from_str(&self.unfinished))
        } else {
            None
        }
    }

    fn finish_string(&mut self, heap: &mut ScriptHeap) {
        if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last()
        {
            if let Some(ScriptTokenPos {
                token: ScriptToken::StringUnfinished,
                pos,
                ..
            }) = self.tokens.pop()
            {
                let v = heap.new_string_from_str(&self.unfinished);
                self.unfinished.clear();
                self.push_tok(pos, ScriptToken::String(v))
            }
        } else {
            self.push_tok(self.pos, ScriptToken::String(ScriptValue::EMPTY_STRING))
        }
    }

    pub fn tokenize(&mut self, new_chars: &str, heap: &mut ScriptHeap) -> &[ScriptTokenPos] {
        let mut iter = new_chars.chars();

        fn is_operator(c: char) -> bool {
            c == '!'
                || c == '^'
                || c == '&'
                || c == '*'
                || c == '+'
                || c == '-'
                || c == '|'
                || c == '?'
                || c == ':'
                || c == '='
                || c == '@'
                || c == '>'
                || c == '<'
                || c == '.'
                || c == '/'
                || c == '~'
                || c == '%'
        }
        fn is_separator(c: char) -> bool {
            c == ',' || c == ';'
        }
        fn is_block(c: char) -> Option<ScriptToken> {
            match c {
                '{' => Some(ScriptToken::OpenCurly),
                '}' => Some(ScriptToken::CloseCurly),
                '[' => Some(ScriptToken::OpenSquare),
                ']' => Some(ScriptToken::CloseSquare),
                '(' => Some(ScriptToken::OpenRound),
                ')' => Some(ScriptToken::CloseRound),
                _ => None,
            }
        }
        // unfinished string at the end
        let start = if let Some(ScriptTokenPos {
            token: ScriptToken::StringUnfinished,
            ..
        }) = self.tokens.last_mut()
        {
            self.tokens.len() - 1
        } else {
            self.tokens.len()
        };

        while let Some(c) = iter.next() {
            self.original.push(c);
            self.pos += 1;
            match self.state {
                State::Whitespace => {
                    if c.is_numeric() {
                        self.state = State::Number;
                        self.temp.push(c);
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.state = State::Color;
                    } else if is_separator(c) {
                        self.emit_separator(c);
                    } else if is_operator(c) {
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if c == '"' {
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_token_here(tok);
                    }
                }
                State::Identifier => {
                    if c == '_' || c == '$' || c.is_alphanumeric() {
                        self.temp.push(c);
                    } else if c.is_whitespace() {
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    } else if is_operator(c) {
                        self.emit_identifier();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_identifier();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '#' {
                        self.emit_identifier();
                        self.state = State::Color;
                    } else if let Some(tok) = is_block(c) {
                        self.emit_identifier();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_identifier();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_identifier();
                        self.state = State::String(false);
                    } else {
                        self.emit_identifier();
                        self.state = State::Whitespace;
                    }
                }
                State::Operator => {
                    // Helper to check if a string is a valid operator or valid prefix of an operator
                    fn is_valid_operator(s: &str) -> bool {
                        matches!(
                            s,
                            // Single character operators
                            "!" | "~" | "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" |
                            "<" | ">" | "=" | "." | "?" | ":" | "@" |
                            // Double character operators
                            "==" | "!=" | "<=" | ">=" | "&&" | "||" | "|?" |
                            "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | ":=" |
                            "<<" | ">>" | ".." | "->" | ".?" | ">:" | "<:" | "^:" | "+:" | "?=" |
                            "++" | "-:" | "=>" |
                            "/*" | "//" |
                            // Triple character operators
                            "===" | "!==" | "<<=" | ">>=" | "..."
                        )
                    }

                    fn could_be_operator_prefix(s: &str) -> bool {
                        // Check if s could be the start of a valid multi-char operator
                        matches!(
                            s,
                            // Single chars that could become 2-char operators
                            "!" |  // !=, !==
                            "=" |  // ==, ===
                            "<" |  // <=, <<, <:, <<=
                            ">" |  // >=, >>, >:, >>=
                            "&" |  // &&
                            "|" |  // ||, |?, |=
                            "+" |  // +=, +:, ++
                            "-" |  // -=, ->, -:
                            "*" |  // *=
                            "/" |  // /=, /*, //
                            "%" |  // %=
                            "^" |  // ^=, ^:
                            ":" |  // :=
                            "." |  // .., .?, ...
                            "?" |  // ?=
                            // Double chars that could become 3-char operators
                            "==" | // ===
                            "!=" | // !==
                            "<<" | // <<=
                            ">>" | // >>=
                            ".." // ...
                        )
                    }

                    // Special case: @( starts a RustValue
                    if self.temp == "@" && c == '(' {
                        self.temp.clear();
                        self.state = State::RustValue;
                        continue;
                    }

                    // Handle non-operator characters - emit current operator and transition
                    if c.is_whitespace() {
                        self.emit_operator();
                        self.state = State::Whitespace;
                    } else if c.is_numeric() {
                        // Handle .5 as 0.5 (float literal starting with dot)
                        if self.temp == "." {
                            self.temp.push(c);
                            self.state = State::Number;
                        } else {
                            self.emit_operator();
                            self.state = State::Number;
                            self.temp.push(c);
                        }
                    } else if is_separator(c) {
                        self.emit_operator();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.emit_operator();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_operator();
                        self.state = State::Color;
                    } else if c == '"' {
                        self.emit_operator();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_operator();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_operator();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else if is_operator(c) {
                        // Try to extend the current operator
                        let mut extended = self.temp.clone();
                        extended.push(c);

                        if is_valid_operator(&extended) || could_be_operator_prefix(&extended) {
                            // Valid extension, keep building
                            self.temp.push(c);
                        } else {
                            // Can't extend - emit current operator and start new one
                            self.emit_operator();
                            self.temp.push(c);
                        }
                    } else {
                        self.emit_operator();
                        self.state = State::Whitespace;
                    }

                    // Check for comment start
                    if self.temp == "/*" {
                        self.state = State::BlockCommentStart;
                        self.temp.clear();
                    } else if self.temp == "//" {
                        self.state = State::LineComment;
                        self.temp.clear();
                    }
                    // Emit complete operators that can't be extended
                    else if is_valid_operator(&self.temp) && !could_be_operator_prefix(&self.temp)
                    {
                        self.emit_operator();
                    }
                }
                State::EscapeInString(double) => {
                    // ok lets see what we have for an escape character sequence
                    if c == '\\' {
                        self.append_unfinished_string('\\');
                        self.state = State::String(double);
                    } else if c == '"' {
                        self.append_unfinished_string('"');
                        self.state = State::String(double);
                    } else if c == '\'' {
                        self.append_unfinished_string('\'');
                        self.state = State::String(double);
                    } else if c == 'r' {
                        self.append_unfinished_string('\r');
                        self.state = State::String(double);
                    } else if c == 'n' {
                        self.append_unfinished_string('\n');
                        self.state = State::String(double);
                    } else if c == 't' {
                        self.append_unfinished_string('\t');
                        self.state = State::String(double);
                    } else if c == '0' {
                        self.append_unfinished_string('\0');
                        self.state = State::String(double);
                    } else if c == 'x' {
                        self.state = State::AsciiHexInString(double);
                    } else if c == 'u' {
                        self.state = State::UnicodeHexInString(double);
                    }
                }
                State::AsciiHexInString(double) => {
                    self.temp.push(c);
                    if self.temp.len() == 2 {
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                            self.append_unfinished_string(v as u8 as char);
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    }
                }
                State::UnicodeHexInString(double) => {
                    if c == '{' {
                        self.state = State::UnicodeCurlyInString(double);
                    } else {
                        // its kinda unknown how long we need to keep pushing sself
                        self.temp.push(c);
                        if self.temp.len() == 4 {
                            if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                                if let Some(v) = char::from_u32(v as u32) {
                                    self.append_unfinished_string(v);
                                }
                            }
                            self.temp.clear();
                            self.state = State::String(double);
                        }
                    }
                }
                State::UnicodeCurlyInString(double) => {
                    if c == '}' {
                        if let Ok(v) = i64::from_str_radix(&self.temp, 16) {
                            if let Some(v) = char::from_u32(v as u32) {
                                self.append_unfinished_string(v);
                            }
                        }
                        self.temp.clear();
                        self.state = State::String(double);
                    } else {
                        self.temp.push(c);
                    }
                }
                State::String(double) => {
                    // check last token is
                    if c == '\\' {
                        // escape char
                        self.temp.clear();
                        self.state = State::EscapeInString(double);
                    } else if (double && c == '"') || (!double && c == '\'') {
                        self.finish_string(heap);
                        self.state = State::Whitespace;
                    } else {
                        self.append_unfinished_string(c);
                    }
                }
                State::BlockComment(depth) => {
                    if c == '*' {
                        // end block comment
                        self.state = State::MaybeEndBlock(depth);
                    }
                }
                State::MaybeEndBlock(depth) => {
                    if c == '/' {
                        // end block comment
                        if depth > 0 {
                            self.state = State::BlockComment(depth - 1)
                        } else {
                            self.state = State::Whitespace;
                        }
                    } else {
                        self.state = State::BlockComment(depth)
                    }
                }
                State::LineComment => {
                    if c == '\n' {
                        // end line comment
                        self.state = State::Whitespace;
                    }
                }
                State::BlockCommentStart => {
                    if c == '*' {
                        self.state = State::BlockDocStart;
                    } else if c == '/' {
                        // `/*/` : half-open plain comment, still open
                        self.state = State::BlockComment(0);
                    } else {
                        self.state = State::BlockComment(0);
                    }
                }
                State::BlockDocStart => {
                    if c == '/' {
                        // `/**/` : empty plain comment
                        self.state = State::Whitespace;
                    } else if c == '*' {
                        // `/***` : plain comment, per Rust convention; the
                        // `*` we saw may begin the closer
                        self.state = State::MaybeEndBlock(0);
                    } else {
                        self.temp.clear();
                        self.temp.push(c);
                        self.state = State::BlockDoc;
                    }
                }
                State::BlockDoc => {
                    if c == '*' {
                        self.state = State::BlockDocMaybeEnd;
                    } else {
                        self.temp.push(c);
                    }
                }
                State::BlockDocMaybeEnd => {
                    if c == '/' {
                        let text = self.temp.trim().to_string();
                        if !text.is_empty() {
                            self.docs.push(ScriptTokDoc {
                                next_token: self.tokens.len() as u32,
                                text,
                            });
                        }
                        self.temp.clear();
                        self.state = State::Whitespace;
                    } else if c == '*' {
                        self.temp.push('*');
                        // stay: this `*` may begin the closer
                    } else {
                        self.temp.push('*');
                        self.temp.push(c);
                        self.state = State::BlockDoc;
                    }
                }
                State::Number => {
                    if c.is_numeric() {
                        self.temp.push(c);
                    } else if c == '.' && self.temp.chars().last() == Some('.') {
                        self.temp.pop();
                        self.emit_f64();
                        self.temp.push('.');
                        self.temp.push('.');
                        self.emit_operator();
                        self.state = State::Whitespace
                    } else if c == '.' && self.temp.chars().position(|v| v == '.').is_none() {
                        self.temp.push(c);
                    } else if (c == 'e' || c == 'E')
                        && self
                            .temp
                            .chars()
                            .position(|v| v == 'e' || v == 'E')
                            .is_none()
                    {
                        self.temp.push(c);
                    } else if (c == '+' || c == '-')
                        && matches!(self.temp.chars().last(), Some('e') | Some('E'))
                    {
                        // Handle exponent sign in scientific notation like 1e+20 or 1e-5
                        self.temp.push(c);
                    } else if (c == 'x' || c == 'X')
                        && self
                            .temp
                            .chars()
                            .position(|v| v == 'x' || v == 'X')
                            .is_none()
                    {
                        self.temp.push(c);
                    } else if c == 'f' {
                        self.emit_f32();
                        self.state = State::Whitespace
                    } else if c == 'u' {
                        self.emit_u32();
                        self.state = State::Whitespace
                    } else if c == 'i' {
                        self.emit_i32();
                        self.state = State::Whitespace
                    } else if c == 'h' {
                        self.emit_f16();
                        self.state = State::Whitespace
                    } else if c == '_' {
                        // skip these
                        self.state = State::Whitespace
                    } else if c == '$' || c.is_alphabetic() {
                        self.emit_f64();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_f64();
                        self.state = State::Color;
                        self.temp.push(c);
                    } else if is_operator(c) {
                        self.emit_f64();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_f64();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_f64();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_f64();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_f64();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else {
                        self.emit_f64();
                        self.state = State::Whitespace;
                    }
                }
                State::RustValue => {
                    if c >= '0' && c <= '9' {
                        self.temp.push(c);
                    } else {
                        self.emit_rust_value();
                        self.state = State::Whitespace
                    }
                }
                State::Color => {
                    if self.temp.len() == 0 && c == '(' {
                        self.state = State::RustValue
                    } else if c >= '0' && c <= '9' || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F' {
                        self.temp.push(c);
                        if self.temp.len() == 8 {
                            self.emit_color();
                            self.state = State::Whitespace
                        }
                    } else if c == 'x' && self.temp.len() == 0 { // eat first x
                    } else if c == '_' || c == '$' || c.is_alphabetic() {
                        self.emit_color();
                        self.state = State::Identifier;
                        self.temp.push(c);
                    } else if c == '#' {
                        self.emit_color();
                        self.state = State::Color;
                        self.temp.push(c);
                    } else if is_operator(c) {
                        self.emit_color();
                        self.state = State::Operator;
                        self.temp.push(c);
                    } else if is_separator(c) {
                        self.emit_color();
                        self.emit_separator(c);
                        self.state = State::Whitespace;
                    } else if c == '"' {
                        self.emit_color();
                        self.state = State::String(true);
                    } else if c == '\'' {
                        self.emit_color();
                        self.state = State::String(false);
                    } else if let Some(tok) = is_block(c) {
                        self.emit_color();
                        self.emit_token_here(tok);
                        self.state = State::Whitespace;
                    } else {
                        self.emit_color();
                        self.state = State::Whitespace;
                    }
                }
            }
            // Register the newline AFTER this char's token emission, so a token
            // terminated BY this newline keeps whatever preceded it, and the flag
            // attaches to the NEXT token instead.
            if c == '\n' {
                self.newline_pending = true;
            }
        }
        &self.tokens[start..self.tokens.len()]
    }
}
