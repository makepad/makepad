//! Relax's assembler for `asm!`, `global_asm!` and `naked_asm!` (blocked by `--no-asm`).
//!
//! rustc hands the template to LLVM's integrated assembler after substituting each operand
//! (rustc_codegen_llvm/src/asm.rs: `${N:mod}` placeholders the AsmPrinter prints as register
//! names, symbols or integers). Relax never substitutes text: the template is lexed into tokens
//! and a placeholder becomes one token that already holds what it stands for -- a physical
//! register with its template modifier ([`Tok::Reg`]), a symbol ([`Tok::Sym`]) or an integer
//! ([`Tok::Int`]). The target-independent part here runs what LLVM's AsmParser does around the
//! instructions: statements, comments, labels (named and numeric `1:` / `1b` / `1f`), the
//! directives programs use (`.text` / `.data` / `.bss` / `.section`, `.p2align` / `.balign` /
//! `.align`, `.globl` / `.hidden`, `.set` / `.equ`, `.rept` / `.endr`, `.macro` / `.endm`, data
//! directives, `.cfi_*` ignored), expressions, and fixups. The instructions themselves are
//! encoded by [`x64`] (Intel syntax, the default; `att_syntax`) and [`arm64`].
//!
//! Output: bytes per section plus the fixups that refer to what the bytes cannot hold (symbols,
//! labels of other sections). Fixups between labels of one section are resolved here; what is
//! left is linked by the caller (an inline block's symbol fixups become the function's
//! relocations, a global block is linked where it is placed).

pub mod arm64;
pub mod x64;

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    pub fn host() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        }
    }
}

/// The object format the template is written for (Mach-O's `@PAGE` / `L` locals, ELF's
/// `:lo12:` / `.L` locals, COFF), and which comment characters the target's assembler takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Format {
    MachO,
    Elf,
    Coff,
}

impl Format {
    pub fn host() -> Format {
        if cfg!(target_vendor = "apple") {
            Format::MachO
        } else if cfg!(windows) {
            Format::Coff
        } else {
            Format::Elf
        }
    }
}

/// A physical register: the integer file (`n` = hardware number: x64 rax = 0 .. r15 = 15,
/// arm64 x0 .. x30) or the vector/float file (x64 xmm0 .. xmm31, arm64 v0 .. v31). Packed as
/// the backends' clobber masks pack them: `bit()` = n for integers, 32 + n for vectors.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct PReg {
    pub vec: bool,
    pub n: u8,
}

impl PReg {
    pub const fn int(n: u8) -> PReg {
        PReg { vec: false, n }
    }
    pub const fn vec(n: u8) -> PReg {
        PReg { vec: true, n }
    }
    pub fn bit(self) -> u32 {
        self.n as u32 + if self.vec { 32 } else { 0 }
    }
}

/// A symbol an operand or a template names: a Relax symbol (`rir::Sym`: id + offset), or a name
/// the block does not define (global asm calling a `#[no_mangle]` fn or a C function by name).
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub enum SymRef {
    Sym(crate::rir::Sym),
    Name(String),
}

/// One template operand as the template sees it (`{N}` / `{name}`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Arg {
    /// a register operand: the register the value is in during the block
    Reg(PReg),
    /// `sym path`
    Sym(SymRef),
    /// `const expr`: rustc prints the value as a decimal integer
    Const(i128),
}

/// A template piece (rustc_builtin_macros/src/asm.rs: InlineAsmTemplatePiece).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Piece {
    Text(String),
    /// operand index and template modifier (`{0:e}` -> Some('e'))
    Op(u32, Option<char>),
}

/// Splits rustc's template strings (joined with newlines, as rustc joins them) into pieces:
/// `{}` (the next positional operand), `{N}`, `{name}`, `{:m}`, `{N:m}`, `{name:m}`; `{{` and
/// `}}` are literal braces. `names` maps a named operand to its index; `positional` is how many
/// leading operands are positional. With `raw` the template is text only.
pub fn parse_template(templates: &[&str], names: &[(String, u32)], positional: u32, n_ops: u32, raw: bool) -> Result<Vec<Piece>, String> {
    let mut out: Vec<Piece> = Vec::new();
    let mut text = String::new();
    let mut next = 0u32;
    for (ti, t) in templates.iter().enumerate() {
        if ti > 0 {
            text.push('\n');
        }
        if raw {
            text.push_str(t);
            continue;
        }
        let b = t.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == b'{' && b.get(i + 1) == Some(&b'{') {
                text.push('{');
                i += 2;
                continue;
            }
            if c == b'}' && b.get(i + 1) == Some(&b'}') {
                text.push('}');
                i += 2;
                continue;
            }
            if c == b'}' {
                return Err("invalid asm template string: unmatched `}` found".to_string());
            }
            if c != b'{' {
                // whole UTF-8 characters
                let ch = t[i..].chars().next().unwrap();
                text.push(ch);
                i += ch.len_utf8();
                continue;
            }
            let close = t[i..].find('}').ok_or_else(|| "invalid asm template string: expected `}`, found end of string".to_string())? + i;
            let inner = &t[i + 1..close];
            let (arg, modifier) = match inner.split_once(':') {
                Some((a, m)) => (a, Some(m)),
                None => (inner, None),
            };
            let modifier = match modifier {
                None => None,
                Some(m) => {
                    let mut cs = m.chars();
                    match (cs.next(), cs.next()) {
                        (Some(c), None) => Some(c),
                        _ => return Err(format!("invalid asm template string: invalid modifier `{}`", m)),
                    }
                }
            };
            let idx = if arg.is_empty() {
                let k = next;
                next += 1;
                k
            } else if let Ok(k) = arg.parse::<u32>() {
                k
            } else if let Some((_, k)) = names.iter().find(|(n, _)| n == arg) {
                *k
            } else {
                return Err(format!("there is no argument named `{}`", arg));
            };
            if idx >= n_ops {
                return Err(if n_ops == 0 { "invalid reference to argument at index 0".to_string() } else { format!("invalid reference to argument at index {}", idx) });
            }
            let _ = positional;
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Op(idx, modifier));
            i = close + 1;
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

// ---------------------------------------------------------------- tokens

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    /// a name: mnemonics, registers written in the text, labels, directives (`.globl`),
    /// relocation specifiers (`@PAGE` lexes as Punct('@') Ident("PAGE")); `.` is part of a name
    /// (`b.ge`, `.L1`, `v0.4s` is Ident("v0.4s")), so is `$` (x64 AT&T `$1` is Punct('$')
    /// followed by Int)
    Ident(String),
    Int(i128),
    /// a punctuation byte: `, [ ] ( ) + - * / % & | ^ ~ ! # : = < > { }` (and `<<` / `>>` as
    /// Punct(b'<') twice: [`shift_at`] reads them)
    Punct(u8),
    Str(Vec<u8>),
    /// a register operand with its template modifier
    Reg(PReg, Option<char>),
    /// a `sym` operand
    Sym(SymRef),
    /// a numeric label reference `1b` / `1f`: (number, forward)
    LocalRef(u32, bool),
}

impl Tok {
    pub fn ident(&self) -> Option<&str> {
        match self {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }
    pub fn is(&self, c: u8) -> bool {
        matches!(self, Tok::Punct(p) if *p == c)
    }
}

/// `<<` / `>>` at toks[i..]: Some(true) for left.
pub fn shift_at(t: &[Tok], i: usize) -> Option<bool> {
    match (t.get(i), t.get(i + 1)) {
        (Some(Tok::Punct(b'<')), Some(Tok::Punct(b'<'))) => Some(true),
        (Some(Tok::Punct(b'>')), Some(Tok::Punct(b'>'))) => Some(false),
        _ => None,
    }
}

/// Where a statement came from, for errors: the template line (1-based) and its text.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub n: u32,
    pub text: String,
}

struct Lexer<'a> {
    arch: Arch,
    format: Format,
    args: &'a [Arg],
}

/// A statement: tokens up to a newline or separator.
#[derive(Clone, Debug)]
struct Stmt {
    toks: Vec<Tok>,
    line: u32,
}

impl Lexer<'_> {
    /// The line comment starts of the target's assembler (LLVM MCAsmInfo CommentString, plus the
    /// `//` / `/* */` comments AsmLexer takes everywhere) and its statement separator.
    fn line_comment(&self) -> &'static [u8] {
        match (self.arch, self.format) {
            (Arch::X64, _) => b"#",
            (Arch::Arm64, Format::MachO) => b";",
            (Arch::Arm64, _) => b"//",
        }
    }
    fn separator(&self) -> &'static [u8] {
        match (self.arch, self.format) {
            (Arch::Arm64, Format::MachO) => b"%%",
            _ => b";",
        }
    }

    /// Lexes pieces into statements.
    fn lex(&self, pieces: &[Piece]) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        let mut cur: Vec<Tok> = Vec::new();
        let mut line = 1u32;
        let mut in_block_comment = false;
        let comment = self.line_comment();
        let sep = self.separator();
        let flush = |cur: &mut Vec<Tok>, stmts: &mut Vec<Stmt>, line: u32| {
            if !cur.is_empty() {
                stmts.push(Stmt { toks: std::mem::take(cur), line });
            }
        };
        for p in pieces {
            let text = match p {
                Piece::Op(k, m) => {
                    if in_block_comment {
                        continue;
                    }
                    match &self.args[*k as usize] {
                        Arg::Reg(r) => cur.push(Tok::Reg(*r, *m)),
                        Arg::Sym(s) => cur.push(Tok::Sym(s.clone())),
                        Arg::Const(v) => cur.push(Tok::Int(*v)),
                    }
                    continue;
                }
                Piece::Text(t) => t.as_bytes(),
            };
            let mut i = 0;
            while i < text.len() {
                let c = text[i];
                if in_block_comment {
                    if text[i..].starts_with(b"*/") {
                        in_block_comment = false;
                        i += 2;
                    } else {
                        if c == b'\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                    continue;
                }
                if c == b'\n' {
                    flush(&mut cur, &mut stmts, line);
                    line += 1;
                    i += 1;
                    continue;
                }
                if text[i..].starts_with(sep) {
                    flush(&mut cur, &mut stmts, line);
                    i += sep.len();
                    continue;
                }
                if text[i..].starts_with(b"/*") {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                if text[i..].starts_with(comment) || text[i..].starts_with(b"//") {
                    while i < text.len() && text[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                if c == b' ' || c == b'\t' || c == b'\r' {
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    let mut s = Vec::new();
                    i += 1;
                    while i < text.len() && text[i] != b'"' {
                        if text[i] == b'\\' && i + 1 < text.len() {
                            i += 1;
                            let e = text[i];
                            match e {
                                b'n' => s.push(b'\n'),
                                b't' => s.push(b'\t'),
                                b'r' => s.push(b'\r'),
                                b'0'..=b'7' => {
                                    let mut v = 0u32;
                                    let mut k = 0;
                                    while k < 3 && i < text.len() && (b'0'..=b'7').contains(&text[i]) {
                                        v = v * 8 + (text[i] - b'0') as u32;
                                        i += 1;
                                        k += 1;
                                    }
                                    s.push(v as u8);
                                    continue;
                                }
                                _ => s.push(e),
                            }
                            i += 1;
                            continue;
                        }
                        s.push(text[i]);
                        i += 1;
                    }
                    if i >= text.len() {
                        return Err("unterminated string constant".to_string());
                    }
                    i += 1;
                    cur.push(Tok::Str(s));
                    continue;
                }
                if c.is_ascii_digit() {
                    let (v, end, radix) = lex_int(text, i)?;
                    i = end;
                    // numeric label reference: 1b / 1f (a hex literal 0x1f was taken whole)
                    if radix == 10 && i < text.len() && (text[i] == b'b' || text[i] == b'f') && !(i + 1 < text.len() && is_ident_char(text[i + 1])) {
                        cur.push(Tok::LocalRef(v as u32, text[i] == b'f'));
                        i += 1;
                        continue;
                    }
                    cur.push(Tok::Int(v));
                    continue;
                }
                if c == b'\'' && i + 2 < text.len() && text[i + 2] == b'\'' {
                    cur.push(Tok::Int(text[i + 1] as i128));
                    i += 3;
                    continue;
                }
                if is_ident_start(c) || (c == b'\\' && i + 1 < text.len() && is_ident_start(text[i + 1])) {
                    let start = i;
                    i += 1;
                    while i < text.len() && is_ident_char(text[i]) {
                        i += 1;
                    }
                    cur.push(Tok::Ident(String::from_utf8_lossy(&text[start..i]).into_owned()));
                    continue;
                }
                cur.push(Tok::Punct(c));
                i += 1;
            }
        }
        if in_block_comment {
            return Err("unterminated comment".to_string());
        }
        flush(&mut cur, &mut stmts, line);
        Ok(stmts)
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'.' || c == b'$'
}
fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$'
}

/// An integer literal at text[i..]: decimal, 0x / 0b / 0o (and C-style leading-0 octal, as
/// LLVM's AsmLexer). Returns the value, the end and the radix.
fn lex_int(text: &[u8], mut i: usize) -> Result<(i128, usize, u32), String> {
    let (radix, skip) = if text[i..].starts_with(b"0x") || text[i..].starts_with(b"0X") {
        (16, 2)
    } else if (text[i..].starts_with(b"0b") || text[i..].starts_with(b"0B")) && text.get(i + 2).is_some_and(|c| *c == b'0' || *c == b'1') {
        (2, 2)
    } else if text[i..].starts_with(b"0o") {
        (8, 2)
    } else if text[i] == b'0' && text.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        (8, 1)
    } else {
        (10, 0)
    };
    i += skip;
    let start = i;
    let mut v: i128 = 0;
    while i < text.len() {
        let d = match text[i] {
            c @ b'0'..=b'9' => (c - b'0') as u32,
            c @ b'a'..=b'f' if radix == 16 => (c - b'a' + 10) as u32,
            c @ b'A'..=b'F' if radix == 16 => (c - b'A' + 10) as u32,
            b'_' => {
                i += 1;
                continue;
            }
            _ => break,
        };
        if d >= radix {
            // `1b` (binary radix never reached here); a decimal digit past the radix ends it
            break;
        }
        v = v.checked_mul(radix as i128).and_then(|v| v.checked_add(d as i128)).ok_or("integer literal out of range")?;
        i += 1;
    }
    if i == start {
        return Err("invalid integer literal".to_string());
    }
    Ok((v, i, radix))
}

// ---------------------------------------------------------------- values, fixups

/// A label: a named one (by name) or an anonymous numbered instance of a numeric label.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub enum LabelId {
    Named(String),
    /// numeric label `n`, its k-th definition
    Local(u32, u32),
    /// `.`: a fixed position (section, offset)
    At(u32, u32),
}

/// What an expression evaluates to.
#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    Const(i128),
    /// a label of this block (perhaps not defined yet) + addend
    Label(LabelId, i64),
    /// a symbol from outside the block + addend
    Sym(SymRef, i64),
}

impl Value {
    pub fn konst(&self) -> Option<i128> {
        match self {
            Value::Const(k) => Some(*k),
            _ => None,
        }
    }
}

/// How a fixup's field is encoded. `at` is the field's first byte; pc-relative kinds measure
/// from the address given per kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FixKind {
    /// x64 disp32 at `at`; `tail` more bytes of the instruction follow it (an immediate): value
    /// - (address of at + 4 + tail)
    X64Rel32 { tail: u8 },
    /// x64 jmp / call / jcc rel32 at `at` (ends the instruction): value - (address of at + 4)
    X64Branch32,
    /// x64 rel8 at `at` (jcc / jmp short): value - (address of at + 1)
    X64Rel8,
    /// 8 bytes = value
    Abs64,
    /// 4 bytes = value (must fit)
    Abs32,
    /// arm64 b / bl imm26 at `at`: (value - pc) / 4
    A64Branch26,
    /// arm64 b.cond / cbz / cbnz / ldr literal imm19 (bits 5..24): (value - pc) / 4
    A64Branch19,
    /// arm64 tbz / tbnz imm14 (bits 5..19)
    A64Branch14,
    /// arm64 adr imm21: value - pc
    A64Adr21,
    /// arm64 adrp: page(value) - page(pc)
    A64Page21,
    /// arm64 add/ldr/str unsigned imm12 (bits 10..22) = (value & 0xfff) >> scale
    A64Lo12 { scale: u8 },
}

/// A field to fill: in `section` at `at`, of `kind`, with `target`.
#[derive(Clone, PartialEq, Debug)]
pub struct Fixup {
    pub section: u32,
    pub at: u32,
    pub kind: FixKind,
    pub target: Value,
}

/// A section of a block: `.text`, `.data`, `.bss`, `.rodata`, or one `.section` names.
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub bytes: Vec<u8>,
    /// executable (`.text`)
    pub exec: bool,
    /// zero-initialised (`.bss`: `bytes` are all zero)
    pub bss: bool,
    pub align: u32,
}

/// A label the block defines that others may name: (name, section, offset, global).
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub section: u32,
    pub offset: u32,
    pub global: bool,
}

/// The assembled block.
#[derive(Clone, Debug)]
pub struct Output {
    pub sections: Vec<Section>,
    /// fixups left after resolving pc-relative ones between labels of one section: symbol
    /// targets, and labels of other sections / absolute label addresses (global blocks)
    pub fixups: Vec<Fixup>,
    pub defs: Vec<Def>,
}

/// Where an error is: the template line and what is wrong.
#[derive(Clone, Debug)]
pub struct AsmError {
    pub line: u32,
    pub msg: String,
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{} (asm template line {})", self.msg, self.line)
        } else {
            f.write_str(&self.msg)
        }
    }
}

/// The interface the instruction encoders get: the current section, labels, expressions,
/// fixups.
pub struct Ctx {
    pub arch: Arch,
    pub format: Format,
    /// AT&T syntax (x64 `options(att_syntax)` / `.att_syntax`)
    pub att: bool,
    sections: Vec<Section>,
    cur: u32,
    fixups: Vec<Fixup>,
    /// label -> (section, offset)
    labels: HashMap<LabelId, (u32, u32)>,
    /// numeric label n -> how many definitions so far
    local_count: HashMap<u32, u32>,
    /// `.set` symbols
    sets: HashMap<String, Value>,
    globals: Vec<String>,
    defs_order: Vec<String>,
    /// inline asm: one text section, no section switching
    inline: bool,
}

impl Ctx {
    fn new(arch: Arch, format: Format, att: bool, inline: bool) -> Ctx {
        Ctx { arch, format, att, sections: vec![Section { name: ".text".to_string(), bytes: Vec::new(), exec: true, bss: false, align: 4 }], cur: 0, fixups: Vec::new(), labels: HashMap::new(), local_count: HashMap::new(), sets: HashMap::new(), globals: Vec::new(), defs_order: Vec::new(), inline }
    }

    /// Offset in the current section.
    pub fn pos(&self) -> u32 {
        self.sections[self.cur as usize].bytes.len() as u32
    }
    pub fn emit(&mut self, b: &[u8]) {
        self.sections[self.cur as usize].bytes.extend_from_slice(b);
    }
    pub fn emit_u8(&mut self, b: u8) {
        self.sections[self.cur as usize].bytes.push(b);
    }
    pub fn emit_u32(&mut self, w: u32) {
        self.emit(&w.to_le_bytes());
    }
    /// The bytes of the current section (an encoder patching a field it just emitted).
    pub fn bytes_mut(&mut self) -> &mut Vec<u8> {
        &mut self.sections[self.cur as usize].bytes
    }
    /// A field at `at` of the current section to fill with `target`. A constant target is an
    /// error for pc-relative kinds (the encoders take constants directly).
    pub fn fixup(&mut self, at: u32, kind: FixKind, target: Value) {
        self.fixups.push(Fixup { section: self.cur, at, kind, target });
    }

    /// The value of a label right now: Some((section, offset)) when defined (backward
    /// reference) -- encoders choose short forms from it.
    pub fn label_now(&self, v: &Value) -> Option<(u32, i64)> {
        match v {
            Value::Label(l, k) => self.label_pos(l).map(|(s, o)| (s, o as i64 + k)),
            _ => None,
        }
    }
    pub fn section(&self) -> u32 {
        self.cur
    }

    /// Parses an expression at toks[*i..] (LLVM AsmParser::parseExpression precedence: `|`
    /// < `^` < `&` < `<< >>` < `+ -` < `* / %` < unary). Stops before a token it does not take
    /// (`,` `]` `)`, a relocation specifier `@`, end).
    pub fn expr(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        self.expr_or(t, i)
    }
    fn expr_or(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_xor(t, i)?;
        while t.get(*i).is_some_and(|x| x.is(b'|')) && !t.get(*i + 1).is_some_and(|x| x.is(b'|')) {
            *i += 1;
            let b = self.expr_xor(t, i)?;
            a = Self::bin(a, b, |x, y| x | y)?;
        }
        Ok(a)
    }
    fn expr_xor(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_and(t, i)?;
        while t.get(*i).is_some_and(|x| x.is(b'^')) {
            *i += 1;
            let b = self.expr_and(t, i)?;
            a = Self::bin(a, b, |x, y| x ^ y)?;
        }
        Ok(a)
    }
    fn expr_and(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_shift(t, i)?;
        while t.get(*i).is_some_and(|x| x.is(b'&')) {
            *i += 1;
            let b = self.expr_shift(t, i)?;
            a = Self::bin(a, b, |x, y| x & y)?;
        }
        Ok(a)
    }
    fn expr_shift(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_add(t, i)?;
        while let Some(left) = shift_at(t, *i) {
            *i += 2;
            let b = self.expr_add(t, i)?;
            a = Self::bin(a, b, |x, y| if left { x.wrapping_shl(y as u32) } else { x.wrapping_shr(y as u32) })?;
        }
        Ok(a)
    }
    fn expr_add(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_mul(t, i)?;
        loop {
            let plus = match t.get(*i) {
                Some(Tok::Punct(b'+')) => true,
                Some(Tok::Punct(b'-')) => false,
                _ => break,
            };
            *i += 1;
            let b = self.expr_mul(t, i)?;
            a = match (a, b) {
                (Value::Const(x), Value::Const(y)) => Value::Const(if plus { x.wrapping_add(y) } else { x.wrapping_sub(y) }),
                (Value::Label(l, k), Value::Const(y)) => Value::Label(l, if plus { k + y as i64 } else { k - y as i64 }),
                (Value::Sym(s, k), Value::Const(y)) => Value::Sym(s, if plus { k + y as i64 } else { k - y as i64 }),
                (Value::Const(x), Value::Label(l, k)) if plus => Value::Label(l, k + x as i64),
                (Value::Const(x), Value::Sym(s, k)) if plus => Value::Sym(s, k + x as i64),
                // label - label of one section, both defined: a constant
                (Value::Label(l1, k1), Value::Label(l2, k2)) if !plus => match (self.label_pos(&l1), self.label_pos(&l2)) {
                    (Some((s1, o1)), Some((s2, o2))) if s1 == s2 => Value::Const((o1 as i64 + k1 - o2 as i64 - k2) as i128),
                    _ => return Err("expression: a difference of labels must be of labels defined before it in one section".to_string()),
                },
                _ => return Err("expression: unsupported combination of symbols".to_string()),
            };
        }
        Ok(a)
    }
    fn expr_mul(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        let mut a = self.expr_unary(t, i)?;
        loop {
            let op = match t.get(*i) {
                Some(Tok::Punct(c @ (b'*' | b'/' | b'%'))) => *c,
                _ => break,
            };
            *i += 1;
            let b = self.expr_unary(t, i)?;
            a = match (a, b) {
                (Value::Const(x), Value::Const(y)) => Value::Const(match op {
                    b'*' => x.wrapping_mul(y),
                    _ if y == 0 => return Err("expression: division by zero".to_string()),
                    b'/' => x / y,
                    _ => x % y,
                }),
                _ => return Err("expression: `*` `/` `%` need constants".to_string()),
            };
        }
        Ok(a)
    }
    fn expr_unary(&self, t: &[Tok], i: &mut usize) -> Result<Value, String> {
        match t.get(*i) {
            Some(Tok::Punct(b'-')) => {
                *i += 1;
                match self.expr_unary(t, i)? {
                    Value::Const(x) => Ok(Value::Const(x.wrapping_neg())),
                    _ => Err("expression: cannot negate a symbol".to_string()),
                }
            }
            Some(Tok::Punct(b'+')) => {
                *i += 1;
                self.expr_unary(t, i)
            }
            Some(Tok::Punct(b'~')) => {
                *i += 1;
                match self.expr_unary(t, i)? {
                    Value::Const(x) => Ok(Value::Const(!x)),
                    _ => Err("expression: cannot complement a symbol".to_string()),
                }
            }
            Some(Tok::Punct(b'!')) => {
                *i += 1;
                match self.expr_unary(t, i)? {
                    Value::Const(x) => Ok(Value::Const((x == 0) as i128)),
                    _ => Err("expression: `!` needs a constant".to_string()),
                }
            }
            Some(Tok::Punct(b'(')) => {
                *i += 1;
                let v = self.expr_or(t, i)?;
                if !t.get(*i).is_some_and(|x| x.is(b')')) {
                    return Err("expression: expected `)`".to_string());
                }
                *i += 1;
                Ok(v)
            }
            Some(Tok::Int(k)) => {
                *i += 1;
                Ok(Value::Const(*k))
            }
            Some(Tok::Sym(s)) => {
                *i += 1;
                Ok(Value::Sym(s.clone(), 0))
            }
            Some(Tok::LocalRef(n, fwd)) => {
                *i += 1;
                let k = self.local_count.get(n).copied().unwrap_or(0);
                if *fwd {
                    Ok(Value::Label(LabelId::Local(*n, k), 0))
                } else if k == 0 {
                    Err(format!("directional label `{}b` has no definition before it", n))
                } else {
                    Ok(Value::Label(LabelId::Local(*n, k - 1), 0))
                }
            }
            Some(Tok::Ident(name)) => {
                *i += 1;
                if name == "." {
                    return Ok(Value::Label(LabelId::At(self.cur, self.pos()), 0));
                }
                if let Some(v) = self.sets.get(name) {
                    return Ok(v.clone());
                }
                Ok(Value::Label(LabelId::Named(name.clone()), 0))
            }
            Some(t) => Err(format!("expression: unexpected {:?}", t)),
            None => Err("expression: expected a value".to_string()),
        }
    }
    /// Where a label is, if defined.
    fn label_pos(&self, l: &LabelId) -> Option<(u32, u32)> {
        match l {
            LabelId::At(s, o) => Some((*s, *o)),
            _ => self.labels.get(l).copied(),
        }
    }
    fn bin(a: Value, b: Value, f: impl Fn(i128, i128) -> i128) -> Result<Value, String> {
        match (a, b) {
            (Value::Const(x), Value::Const(y)) => Ok(Value::Const(f(x, y))),
            _ => Err("expression: bitwise operators need constants".to_string()),
        }
    }
    /// A constant expression (directive arguments, immediates).
    pub fn const_expr(&self, t: &[Tok], i: &mut usize) -> Result<i128, String> {
        match self.expr(t, i)? {
            Value::Const(k) => Ok(k),
            _ => Err("expected a constant expression".to_string()),
        }
    }

    fn define_label(&mut self, l: LabelId) -> Result<(), String> {
        let at = (self.cur, self.pos());
        if self.labels.insert(l.clone(), at).is_some() {
            if let LabelId::Named(n) = &l {
                return Err(format!("symbol `{}` is already defined", n));
            }
        }
        if let LabelId::Named(n) = l {
            self.defs_order.push(n);
        }
        Ok(())
    }
}

/// An instruction encoder: one statement whose first token is a mnemonic.
pub(crate) trait Encoder {
    /// Encodes the instruction (`mnemonic` lowercase, prefixes such as x64 `lock` / `rep` are
    /// mnemonics of their own followed by the instruction). `ops` are the tokens after it.
    fn instruction(&mut self, ctx: &mut Ctx, mnemonic: &str, ops: &[Tok]) -> Result<(), String>;
    /// Directives of the target (x64 `.intel_syntax` / `.att_syntax` / `.code64`, arm64 `.inst`):
    /// Ok(false) when not one.
    fn directive(&mut self, ctx: &mut Ctx, name: &str, args: &[Tok]) -> Result<bool, String>;
    /// The alignment padding byte pattern of an executable section (x64 nops, arm64 nop words)
    /// for `n` bytes.
    fn code_fill(&self, n: usize) -> Vec<u8>;
}

/// What a block is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `asm!`: one instruction stream inside a function
    Inline,
    /// `global_asm!` / `naked_asm!`: sections, global symbols
    Global,
}

/// Assembles a template. `args` are the operands by index.
pub fn assemble(arch: Arch, format: Format, kind: Kind, pieces: &[Piece], args: &[Arg], att: bool) -> Result<Output, AsmError> {
    let lexer = Lexer { arch, format, args };
    let stmts = lexer.lex(pieces).map_err(|msg| AsmError { line: 0, msg })?;
    let mut ctx = Ctx::new(arch, format, att, kind == Kind::Inline);
    let mut enc: Box<dyn Encoder> = match arch {
        Arch::X64 => Box::new(x64::X64::default()),
        Arch::Arm64 => Box::new(arm64::Arm64::default()),
    };
    let mut macros: HashMap<String, Macro> = HashMap::new();
    run(&mut ctx, &mut *enc, &stmts, &mut macros, 0)?;
    finish(ctx, &*enc)
}

#[derive(Clone)]
struct Macro {
    params: Vec<(String, Option<Vec<Tok>>)>,
    body: Vec<Stmt>,
}

fn err(line: u32, msg: impl Into<String>) -> AsmError {
    AsmError { line, msg: msg.into() }
}

/// Splits directive arguments at top-level commas.
fn split_commas(t: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, x) in t.iter().enumerate() {
        match x {
            Tok::Punct(b'(') | Tok::Punct(b'[') | Tok::Punct(b'{') => depth += 1,
            Tok::Punct(b')') | Tok::Punct(b']') | Tok::Punct(b'}') => depth -= 1,
            Tok::Punct(b',') if depth == 0 => {
                out.push(&t[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < t.len() || !out.is_empty() {
        out.push(&t[start..]);
    }
    out
}

fn run(ctx: &mut Ctx, enc: &mut dyn Encoder, stmts: &[Stmt], macros: &mut HashMap<String, Macro>, depth: u32) -> Result<(), AsmError> {
    if depth > 64 {
        return Err(err(0, "macro or .rept nesting too deep"));
    }
    let mut si = 0;
    while si < stmts.len() {
        let st = &stmts[si];
        si += 1;
        let line = st.line;
        let mut t: &[Tok] = &st.toks;
        // labels: `name:` / `1:` (several may lead one statement)
        loop {
            match (t.first(), t.get(1)) {
                (Some(Tok::Ident(n)), Some(Tok::Punct(b':'))) if !(t.get(2).is_some_and(|x| x.is(b':'))) => {
                    ctx.define_label(LabelId::Named(n.clone())).map_err(|m| err(line, m))?;
                    t = &t[2..];
                }
                (Some(Tok::Int(n)), Some(Tok::Punct(b':'))) => {
                    let n = *n as u32;
                    let k = ctx.local_count.get(&n).copied().unwrap_or(0);
                    ctx.define_label(LabelId::Local(n, k)).map_err(|m| err(line, m))?;
                    ctx.local_count.insert(n, k + 1);
                    t = &t[2..];
                }
                (Some(Tok::Sym(SymRef::Name(n))), Some(Tok::Punct(b':'))) => {
                    ctx.define_label(LabelId::Named(n.clone())).map_err(|m| err(line, m))?;
                    t = &t[2..];
                }
                _ => break,
            }
        }
        let Some(first) = t.first() else { continue };
        let name = match first {
            Tok::Ident(n) => n.clone(),
            other => return Err(err(line, format!("expected an instruction or directive, found {:?}", other))),
        };
        let args = &t[1..];
        // `name = expr` / `name == expr`: .set
        if args.first().is_some_and(|x| x.is(b'=')) {
            let mut i = 1;
            if args.get(1).is_some_and(|x| x.is(b'=')) {
                i = 2;
            }
            let v = ctx.expr(args, &mut i).map_err(|m| err(line, m))?;
            ctx.sets.insert(name, v);
            continue;
        }
        if let Some(m) = macros.get(&name).cloned() {
            let body = expand_macro(&m, args).map_err(|m| err(line, m))?;
            run(ctx, enc, &body, macros, depth + 1)?;
            continue;
        }
        if !name.starts_with('.') || name.len() == 1 {
            let mn = name.to_ascii_lowercase();
            enc.instruction(ctx, &mn, args).map_err(|m| err(line, m))?;
            continue;
        }
        let d = name.to_ascii_lowercase();
        match d.as_str() {
            ".rept" => {
                let mut i = 0;
                let n = ctx.const_expr(args, &mut i).map_err(|m| err(line, m))?;
                let (body, end) = block_until(stmts, si, ".rept", ".endr").map_err(|m| err(line, m))?;
                si = end;
                for _ in 0..n.max(0) {
                    run(ctx, enc, &body, macros, depth + 1)?;
                }
            }
            ".irp" => {
                // .irp sym, a, b, c ... .endr
                let parts = split_commas(args);
                let Some((sym, vals)) = parts.split_first() else { return Err(err(line, ".irp needs a symbol")) };
                let Some(Tok::Ident(sym)) = sym.first() else { return Err(err(line, ".irp needs a symbol")) };
                let (body, end) = block_until(stmts, si, ".irp", ".endr").map_err(|m| err(line, m))?;
                si = end;
                for v in vals {
                    let m = Macro { params: vec![(sym.clone(), None)], body: body.clone() };
                    let b = expand_macro(&m, v).map_err(|m| err(line, m))?;
                    run(ctx, enc, &b, macros, depth + 1)?;
                }
            }
            ".macro" => {
                let mut it = args.iter();
                let Some(Tok::Ident(mname)) = it.next() else { return Err(err(line, ".macro needs a name")) };
                let rest: Vec<Tok> = it.cloned().collect();
                let mut params = Vec::new();
                for p in split_commas(&rest) {
                    // `name` or `name=default` (space-separated params too)
                    let mut j = 0;
                    while j < p.len() {
                        if let Tok::Ident(pn) = &p[j] {
                            if p.get(j + 1).is_some_and(|x| x.is(b'=')) {
                                params.push((pn.clone(), Some(p[j + 2..].to_vec())));
                                break;
                            }
                            params.push((pn.clone(), None));
                        }
                        j += 1;
                    }
                }
                let (body, end) = block_until(stmts, si, ".macro", ".endm").map_err(|m| err(line, m))?;
                si = end;
                macros.insert(mname.clone(), Macro { params, body });
            }
            ".endr" | ".endm" => return Err(err(line, format!("unmatched {}", d))),
            ".set" | ".equ" => {
                let parts = split_commas(args);
                let (Some(n), Some(e)) = (parts.first(), parts.get(1)) else { return Err(err(line, format!("{} needs a name and a value", d))) };
                let Some(Tok::Ident(n)) = n.first() else { return Err(err(line, format!("{} needs a name", d))) };
                let mut i = 0;
                let v = ctx.expr(e, &mut i).map_err(|m| err(line, m))?;
                ctx.sets.insert(n.clone(), v);
            }
            ".text" => switch_section(ctx, ".text").map_err(|m| err(line, m))?,
            ".data" => switch_section(ctx, ".data").map_err(|m| err(line, m))?,
            ".bss" => switch_section(ctx, ".bss").map_err(|m| err(line, m))?,
            ".rodata" | ".const" => switch_section(ctx, ".rodata").map_err(|m| err(line, m))?,
            ".section" | ".pushsection" => {
                let n = match args.first() {
                    Some(Tok::Ident(n)) => n.clone(),
                    Some(Tok::Str(s)) => String::from_utf8_lossy(s).into_owned(),
                    _ => return Err(err(line, ".section needs a name")),
                };
                let n = canonical_section(&n, args);
                switch_section(ctx, &n).map_err(|m| err(line, m))?;
            }
            ".popsection" | ".previous" => switch_section(ctx, ".text").map_err(|m| err(line, m))?,
            ".globl" | ".global" => {
                for p in split_commas(args) {
                    match p.first() {
                        Some(Tok::Ident(n)) | Some(Tok::Sym(SymRef::Name(n))) => ctx.globals.push(n.clone()),
                        _ => return Err(err(line, format!("{} needs a symbol name", d))),
                    }
                }
            }
            // visibility and symbol type/size: nothing to do for code Relax places itself
            ".hidden" | ".private_extern" | ".protected" | ".internal" | ".weak" | ".weak_definition" | ".weak_reference" | ".type" | ".size" | ".local" | ".file" | ".ident" | ".loc" | ".no_dead_strip" | ".alt_entry" | ".subsections_via_symbols" | ".build_version" | ".end" => {}
            d if d.starts_with(".cfi_") => {}
            ".p2align" | ".balign" | ".align" | ".p2alignw" | ".p2alignl" | ".balignw" | ".balignl" => {
                let parts = split_commas(args);
                let mut i = 0;
                let a = parts.first().map(|p| ctx.const_expr(p, &mut i)).transpose().map_err(|m| err(line, m))?.unwrap_or(0);
                // .align is a power of two on arm64 / Mach-O, bytes on x64 ELF (GNU as / LLVM)
                let pow = d.starts_with(".p2") || (d == ".align" && (ctx.arch == Arch::Arm64 || ctx.format == Format::MachO));
                let bytes: u64 = if pow {
                    if !(0..=16).contains(&a) {
                        return Err(err(line, "alignment too large"));
                    }
                    1u64 << a
                } else {
                    a as u64
                };
                if bytes == 0 || !bytes.is_power_of_two() {
                    return Err(err(line, "alignment must be a power of 2"));
                }
                let fill = match parts.get(1) {
                    Some(p) if !p.is_empty() => {
                        let mut i = 0;
                        Some(ctx.const_expr(p, &mut i).map_err(|m| err(line, m))? as u8)
                    }
                    _ => None,
                };
                align(ctx, enc, bytes as u32, fill);
            }
            ".byte" | ".short" | ".hword" | ".2byte" | ".value" | ".word" | ".long" | ".int" | ".4byte" | ".quad" | ".xword" | ".8byte" | ".dword" => {
                let size = match d.as_str() {
                    ".byte" => 1,
                    ".short" | ".hword" | ".2byte" | ".value" => 2,
                    // .word is 4 bytes on arm64 and 2 on x86 (GNU as / LLVM)
                    ".word" if ctx.arch == Arch::X64 => 2,
                    ".word" | ".long" | ".int" | ".4byte" => 4,
                    _ => 8,
                };
                for p in split_commas(args) {
                    let mut i = 0;
                    let v = ctx.expr(p, &mut i).map_err(|m| err(line, m))?;
                    data_value(ctx, size, v).map_err(|m| err(line, m))?;
                }
            }
            ".zero" | ".space" | ".skip" | ".fill" => {
                let parts = split_commas(args);
                let mut i = 0;
                let n = parts.first().map(|p| ctx.const_expr(p, &mut i)).transpose().map_err(|m| err(line, m))?.unwrap_or(0);
                let (unit, fillv) = if d == ".fill" {
                    let size = match parts.get(1) {
                        Some(p) => {
                            let mut i = 0;
                            ctx.const_expr(p, &mut i).map_err(|m| err(line, m))?
                        }
                        None => 1,
                    };
                    let v = match parts.get(2) {
                        Some(p) => {
                            let mut i = 0;
                            ctx.const_expr(p, &mut i).map_err(|m| err(line, m))?
                        }
                        None => 0,
                    };
                    (size.clamp(0, 8) as usize, v)
                } else {
                    let v = match parts.get(1) {
                        Some(p) => {
                            let mut i = 0;
                            ctx.const_expr(p, &mut i).map_err(|m| err(line, m))?
                        }
                        None => 0,
                    };
                    (1, v)
                };
                if n < 0 || n > (1 << 28) {
                    return Err(err(line, format!("{} size out of range", d)));
                }
                for _ in 0..n {
                    let b = (fillv as u64).to_le_bytes();
                    ctx.emit(&b[..unit]);
                }
            }
            ".ascii" | ".asciz" | ".string" => {
                for p in split_commas(args) {
                    match p.first() {
                        Some(Tok::Str(s)) => {
                            ctx.emit(&s.clone());
                            if d != ".ascii" {
                                ctx.emit_u8(0);
                            }
                        }
                        _ => return Err(err(line, format!("{} needs a string", d))),
                    }
                }
            }
            _ => {
                if !enc.directive(ctx, &d, args).map_err(|m| err(line, m))? {
                    return Err(err(line, format!("unknown directive `{}`", name)));
                }
            }
        }
    }
    Ok(())
}

/// `.section __TEXT,__text,...` / `.section .text.foo,"ax",@progbits` -> one of Relax's kinds
/// by what it holds: code, read-only data, data, zero-filled.
fn canonical_section(n: &str, args: &[Tok]) -> String {
    let flags: String = args.iter().filter_map(|t| match t {
        Tok::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
        _ => None,
    }).collect();
    let n_l = n.to_ascii_lowercase();
    if n_l.starts_with(".text") || n_l.starts_with("__text") || flags.contains('x') {
        return ".text".to_string();
    }
    if n_l.starts_with(".bss") || n_l.starts_with("__bss") || n_l.contains("zerofill") {
        return ".bss".to_string();
    }
    if n_l.starts_with(".rodata") || n_l.starts_with("__const") || n_l.starts_with("__cstring") {
        return ".rodata".to_string();
    }
    if n_l.starts_with(".data") || n_l.starts_with("__data") || flags.contains('w') {
        return ".data".to_string();
    }
    n.to_string()
}

fn switch_section(ctx: &mut Ctx, n: &str) -> Result<(), String> {
    if ctx.inline && n != ".text" {
        return Err(format!("section `{}` in inline assembly: Relax places an asm! block inside its function only", n));
    }
    if let Some(i) = ctx.sections.iter().position(|s| s.name == n) {
        ctx.cur = i as u32;
        return Ok(());
    }
    ctx.sections.push(Section { name: n.to_string(), bytes: Vec::new(), exec: n == ".text", bss: n == ".bss", align: 1 });
    ctx.cur = ctx.sections.len() as u32 - 1;
    Ok(())
}

fn align(ctx: &mut Ctx, enc: &dyn Encoder, bytes: u32, fill: Option<u8>) {
    let s = &mut ctx.sections[ctx.cur as usize];
    s.align = s.align.max(bytes);
    let pad = (bytes - s.bytes.len() as u32 % bytes) % bytes;
    if pad == 0 {
        return;
    }
    let b = match fill {
        Some(f) => vec![f; pad as usize],
        None if s.exec => enc.code_fill(pad as usize),
        None => vec![0; pad as usize],
    };
    s.bytes.extend_from_slice(&b);
}

fn data_value(ctx: &mut Ctx, size: usize, v: Value) -> Result<(), String> {
    match v {
        Value::Const(k) => {
            let b = (k as u64).to_le_bytes();
            ctx.emit(&b[..size]);
        }
        other => {
            let at = ctx.pos();
            let kind = match size {
                8 => FixKind::Abs64,
                4 => FixKind::Abs32,
                _ => return Err("a symbol in a data directive needs 4 or 8 bytes".to_string()),
            };
            ctx.emit(&vec![0; size]);
            ctx.fixup(at, kind, other);
        }
    }
    Ok(())
}

/// The statements between stmts[from..] and the matching end directive; returns them and the
/// index after the end.
fn block_until(stmts: &[Stmt], from: usize, open: &str, close: &str) -> Result<(Vec<Stmt>, usize), String> {
    let mut depth = 1;
    let mut i = from;
    let opens: &[&str] = if close == ".endr" { &[".rept", ".irp", ".irpc"] } else { &[open] };
    while i < stmts.len() {
        if let Some(Tok::Ident(n)) = stmts[i].toks.first() {
            let n = n.to_ascii_lowercase();
            if opens.contains(&n.as_str()) {
                depth += 1;
            } else if n == close {
                depth -= 1;
                if depth == 0 {
                    return Ok((stmts[from..i].to_vec(), i + 1));
                }
            }
        }
        i += 1;
    }
    Err(format!("no {} for {}", close, open))
}

/// A macro body with `\param` replaced by the argument tokens (GNU as / LLVM macro semantics:
/// positional arguments, separated by commas or spaces; defaults).
fn expand_macro(m: &Macro, args: &[Tok]) -> Result<Vec<Stmt>, String> {
    let mut vals: Vec<Vec<Tok>> = split_commas(args).iter().map(|p| p.to_vec()).collect();
    if vals.len() == 1 && vals[0].is_empty() {
        vals.clear();
    }
    let mut bind: HashMap<String, Vec<Tok>> = HashMap::new();
    for (k, (p, def)) in m.params.iter().enumerate() {
        let v = match vals.get(k) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => def.clone().unwrap_or_default(),
        };
        bind.insert(format!("\\{}", p), v);
    }
    let mut out = Vec::new();
    for st in &m.body {
        let mut toks = Vec::new();
        for t in &st.toks {
            match t {
                Tok::Ident(n) if n.starts_with('\\') => {
                    // `\name` may be followed by more name characters only through `\()`; LLVM
                    // also substitutes a `\name` prefix of an identifier (`\target@PAGE` lexes
                    // as `\target` `@` `PAGE`)
                    match bind.get(n.as_str()) {
                        Some(v) => toks.extend(v.iter().cloned()),
                        None => return Err(format!("unknown macro parameter `{}`", n)),
                    }
                }
                other => toks.push(other.clone()),
            }
        }
        out.push(Stmt { toks, line: st.line });
    }
    Ok(out)
}

/// Resolves label fixups within a section and returns what is left.
fn finish(mut ctx: Ctx, _enc: &dyn Encoder) -> Result<Output, AsmError> {
    let mut sections = std::mem::take(&mut ctx.sections);
    let mut left = Vec::new();
    for f in std::mem::take(&mut ctx.fixups) {
        let target = match &f.target {
            Value::Label(l, k) => match ctx.label_pos(l) {
                Some((s, o)) => Some((s, o as i64 + k)),
                None => {
                    // a named label the block does not define: a symbol from outside
                    match l {
                        LabelId::Named(n) if ctx.sets.contains_key(n) => None,
                        LabelId::Named(n) => {
                            left.push(Fixup { target: Value::Sym(SymRef::Name(n.clone()), *k), ..f });
                            continue;
                        }
                        LabelId::Local(n, _) => return Err(err(0, format!("directional label `{}f` has no definition after it", n))),
                        LabelId::At(..) => unreachable!(),
                    }
                }
            },
            _ => None,
        };
        match target {
            Some((s, off)) if s == f.section && is_pc_relative(f.kind) => {
                let pc = f.at as i64;
                let b = &mut sections[s as usize].bytes;
                patch(b, f.at as usize, f.kind, off, pc).map_err(|m| err(0, m))?;
            }
            _ => left.push(f),
        }
    }
    let globals = ctx.globals;
    let mut defs = Vec::new();
    for n in &ctx.defs_order {
        let &(s, o) = &ctx.labels[&LabelId::Named(n.clone())];
        defs.push(Def { name: n.clone(), section: s, offset: o, global: globals.contains(n) });
    }
    for g in &globals {
        if !ctx.labels.contains_key(&LabelId::Named(g.clone())) {
            return Err(err(0, format!("`.globl {}` names no label of the block", g)));
        }
    }
    Ok(Output { sections, fixups: left, defs })
}

pub fn is_pc_relative(k: FixKind) -> bool {
    matches!(k, FixKind::X64Rel32 { .. } | FixKind::X64Branch32 | FixKind::X64Rel8 | FixKind::A64Branch26 | FixKind::A64Branch19 | FixKind::A64Branch14 | FixKind::A64Adr21)
}

/// Writes the field of a pc-relative fixup whose target is `target` and whose instruction is at
/// `pc` (x64: the field's own position; the kind adds its size), both offsets of one section or
/// both absolute addresses; and absolute kinds with `target` the value.
pub fn patch(b: &mut [u8], at: usize, kind: FixKind, target: i64, pc: i64) -> Result<(), String> {
    let rd = |b: &[u8]| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    if at + kind_len(kind) > b.len() {
        return Err("fixup past the end of its section".to_string());
    }
    match kind {
        FixKind::X64Rel32 { .. } | FixKind::X64Branch32 => {
            let tail = if let FixKind::X64Rel32 { tail } = kind { tail } else { 0 };
            let d = target - (pc + 4 + tail as i64);
            if d < i32::MIN as i64 || d > i32::MAX as i64 {
                return Err("rel32 out of range".to_string());
            }
            b[at..at + 4].copy_from_slice(&(d as i32).to_le_bytes());
        }
        FixKind::X64Rel8 => {
            let d = target - (pc + 1);
            if !(-128..128).contains(&d) {
                return Err("short jump out of range".to_string());
            }
            b[at] = d as i8 as u8;
        }
        FixKind::Abs64 => b[at..at + 8].copy_from_slice(&(target as u64).to_le_bytes()),
        FixKind::Abs32 => {
            if target < i32::MIN as i64 || target > u32::MAX as i64 {
                return Err("value does not fit 32 bits".to_string());
            }
            b[at..at + 4].copy_from_slice(&(target as u32).to_le_bytes());
        }
        FixKind::A64Branch26 | FixKind::A64Branch19 | FixKind::A64Branch14 => {
            let d = target - pc;
            if d & 3 != 0 {
                return Err("branch target not 4-byte aligned".to_string());
            }
            let (bits, shift) = match kind {
                FixKind::A64Branch26 => (26, 0),
                FixKind::A64Branch19 => (19, 5),
                _ => (14, 5),
            };
            let w = d >> 2;
            if w < -(1 << (bits - 1)) || w >= 1 << (bits - 1) {
                return Err("branch target out of range".to_string());
            }
            let mask = ((1u32 << bits) - 1) << shift;
            let v = (rd(b) & !mask) | (((w as u32) << shift) & mask);
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        FixKind::A64Adr21 => {
            let d = target - pc;
            if !(-(1 << 20)..(1 << 20)).contains(&d) {
                return Err("adr target out of range".to_string());
            }
            let d = d as u32;
            let v = (rd(b) & !((3 << 29) | (0x7ffff << 5))) | ((d & 3) << 29) | (((d >> 2) & 0x7ffff) << 5);
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        FixKind::A64Page21 => {
            let d = ((target & !0xfff) - (pc & !0xfff)) >> 12;
            if !(-(1 << 20)..(1 << 20)).contains(&d) {
                return Err("adrp target out of range".to_string());
            }
            let d = d as u32;
            let v = (rd(b) & !((3 << 29) | (0x7ffff << 5))) | ((d & 3) << 29) | (((d >> 2) & 0x7ffff) << 5);
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        FixKind::A64Lo12 { scale } => {
            let lo = (target & 0xfff) as u32;
            if lo & ((1 << scale) - 1) != 0 {
                return Err("low 12 bits not aligned for the access size".to_string());
            }
            let v = (rd(b) & !(0xfff << 10)) | ((lo >> scale) << 10);
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    Ok(())
}

fn kind_len(k: FixKind) -> usize {
    match k {
        FixKind::X64Rel8 => 1,
        FixKind::Abs64 => 8,
        _ => 4,
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_x64;
#[cfg(test)]
mod tests_arm64;
