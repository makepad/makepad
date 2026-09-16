//! Source-only SQL lexer shared by the editor and the SQL frontend.
//!
//! Byte offsets address the original document. Continuation state is owned
//! here: block comments, `'...'` strings and dollar-quoted bodies survive
//! line breaks; quoted identifiers are line-bounded.
//!
//! The keyword table is a bounded upper-case union of SQL:2016 reserved words
//! plus common PostgreSQL, MySQL, SQLite and T-SQL reserved/statement words.
//! Contextual non-reserved words that are common column names (`name`, `type`,
//! `value`, `data`, `status`, `id`) are deliberately absent so they colour and
//! parse as identifiers.

use crate::id::Dialect;
use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const SQL_LEXER_VERSION: u32 = 1;

const DOLLAR_TAG_CAP: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SqlDialect {
    #[default]
    Generic,
    Postgres,
    MySql,
    TSql,
    Oracle,
}

impl SqlDialect {
    pub fn from_dialect(d: Dialect) -> Self {
        match d.name {
            "postgres" => SqlDialect::Postgres,
            "mysql" => SqlDialect::MySql,
            "tsql" => SqlDialect::TSql,
            "oracle" => SqlDialect::Oracle,
            _ => SqlDialect::Generic,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SqlDialect::Generic => "generic",
            SqlDialect::Postgres => "postgres",
            SqlDialect::MySql => "mysql",
            SqlDialect::TSql => "tsql",
            SqlDialect::Oracle => "oracle",
        }
    }

    fn dollar_quoting(self) -> bool {
        matches!(self, SqlDialect::Generic | SqlDialect::Postgres)
    }

    fn dollar_ident(self) -> bool {
        matches!(
            self,
            SqlDialect::MySql | SqlDialect::TSql | SqlDialect::Oracle
        )
    }

    fn nested_block_comments(self) -> bool {
        matches!(self, SqlDialect::Postgres)
    }

    fn hash_line_comment(self) -> bool {
        matches!(self, SqlDialect::MySql)
    }

    fn double_quote_is_string(self) -> bool {
        matches!(self, SqlDialect::MySql)
    }

    fn bracket_ident(self) -> bool {
        matches!(self, SqlDialect::TSql)
    }

    fn sqlite_dot_meta(self) -> bool {
        matches!(self, SqlDialect::Generic)
    }

    fn psql_meta(self) -> bool {
        matches!(self, SqlDialect::Generic | SqlDialect::Postgres)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum SqlMode {
    #[default]
    Normal,
    BlockComment,
    String,
    DollarString,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SqlState {
    dialect: SqlDialect,
    mode: SqlMode,
    block_comment_depth: u8,
    dollar_tag: [u8; DOLLAR_TAG_CAP],
    dollar_tag_len: u8,
    /// Next byte of a continued string is escaped (`MySql` / `E'...'`).
    string_escape: bool,
}

impl Default for SqlState {
    fn default() -> Self {
        SqlState::for_dialect(SqlDialect::Generic)
    }
}

impl SqlState {
    pub fn for_dialect(d: SqlDialect) -> Self {
        SqlState {
            dialect: d,
            mode: SqlMode::Normal,
            block_comment_depth: 0,
            dollar_tag: [0; DOLLAR_TAG_CAP],
            dollar_tag_len: 0,
            string_escape: false,
        }
    }

    pub fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    fn reset_string(&mut self) {
        self.mode = SqlMode::Normal;
        self.string_escape = false;
        self.dollar_tag = [0; DOLLAR_TAG_CAP];
        self.dollar_tag_len = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlKind {
    Whitespace,
    Comment,
    Identifier,
    QuotedIdentifier,
    Keyword,
    Number,
    String,
    Parameter,
    Variable,
    Punctuator,
    Delimiter,
    MetaCommand,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqlToken {
    pub start: u32,
    pub end: u32,
    pub kind: SqlKind,
}

impl SqlKind {
    pub fn role(self) -> TokenRole {
        match self {
            SqlKind::Whitespace => TokenRole::Whitespace,
            SqlKind::Comment => TokenRole::Comment,
            SqlKind::Identifier | SqlKind::QuotedIdentifier => TokenRole::Identifier,
            SqlKind::Keyword => TokenRole::Keyword,
            SqlKind::Number => TokenRole::Number,
            SqlKind::String => TokenRole::String,
            SqlKind::Parameter => TokenRole::Constant,
            SqlKind::Variable => TokenRole::Identifier,
            SqlKind::Punctuator => TokenRole::Punctuator,
            SqlKind::Delimiter => TokenRole::Delimiter,
            SqlKind::MetaCommand => TokenRole::Preprocessor,
            SqlKind::Unknown => TokenRole::Unknown,
        }
    }

    fn is_trivia(self) -> bool {
        matches!(self, SqlKind::Whitespace | SqlKind::Comment)
    }
}

/// Upper-case union of SQL:2016 reserved words and common dialect statement
/// words. Sorted for binary search. `NAME`, `TYPE`, `VALUE`, `DATA`, `STATUS`
/// and `ID` are omitted: they are common column names and are not reserved
/// in the dialects this lexer covers.
const KEYWORDS: &[&str] = &[
    "ABS",
    "ADD",
    "AFTER",
    "AGGREGATE",
    "ALGORITHM",
    "ALL",
    "ALTER",
    "ALWAYS",
    "ANALYSE",
    "ANALYZE",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "AT",
    "ATOMIC",
    "ATTACH",
    "AUTHORIZATION",
    "AVG",
    "BEFORE",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BIGSERIAL",
    "BINARY",
    "BIT",
    "BLOB",
    "BODY",
    "BOOL",
    "BOOLEAN",
    "BOTH",
    "BREAK",
    "BY",
    "BYTEA",
    "CACHE",
    "CALL",
    "CASCADE",
    "CASE",
    "CAST",
    "CATCH",
    "CHAR",
    "CHARACTER",
    "CHECK",
    "CLOB",
    "CLOSE",
    "CLUSTER",
    "CLUSTERED",
    "COALESCE",
    "COLLATE",
    "COLUMN",
    "COMMENT",
    "COMMIT",
    "CONSTRAINT",
    "CONTINUE",
    "CONVERT",
    "COPY",
    "COUNT",
    "CREATE",
    "CROSS",
    "CUBE",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "CYCLE",
    "DATABASE",
    "DATE",
    "DATETIME",
    "DATETIME2",
    "DAY",
    "DEALLOCATE",
    "DEC",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DEFERRABLE",
    "DEFERRED",
    "DEFINER",
    "DELETE",
    "DELIMITER",
    "DESC",
    "DISTINCT",
    "DISTRIBUTED",
    "DO",
    "DOMAIN",
    "DOUBLE",
    "DROP",
    "EACH",
    "ELSE",
    "ELSEIF",
    "ELSIF",
    "ENABLE",
    "END",
    "ENUM",
    "ESCAPE",
    "EVENT",
    "EXCEPT",
    "EXCEPTION",
    "EXCLUDE",
    "EXCLUSIVE",
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXIT",
    "EXPLAIN",
    "EXTENSION",
    "EXTERNAL",
    "EXTRACT",
    "FALSE",
    "FETCH",
    "FIRST",
    "FLOAT",
    "FOR",
    "FORCE",
    "FOREACH",
    "FOREIGN",
    "FROM",
    "FULL",
    "FULLTEXT",
    "FUNCTION",
    "GENERATED",
    "GLOBAL",
    "GO",
    "GOTO",
    "GRANT",
    "GROUP",
    "HAVING",
    "HOUR",
    "IDENTITY",
    "IF",
    "IGNORE",
    "ILIKE",
    "IMMEDIATE",
    "IMMUTABLE",
    "IN",
    "INCLUDE",
    "INCLUDING",
    "INCREMENT",
    "INDEX",
    "INDEXES",
    "INHERIT",
    "INHERITS",
    "INITIALLY",
    "INNER",
    "INSERT",
    "INSTEAD",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "ISOLATION",
    "ITERATE",
    "JOIN",
    "JSON",
    "JSONB",
    "KEY",
    "LANGUAGE",
    "LATERAL",
    "LEADING",
    "LEAVE",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LISTEN",
    "LOAD",
    "LOCAL",
    "LOCK",
    "LOGGED",
    "LOOP",
    "MATCH",
    "MATCHED",
    "MATERIALIZED",
    "MAX",
    "MERGE",
    "MIN",
    "MONEY",
    "MONTH",
    "MOVE",
    "NATURAL",
    "NCHAR",
    "NEXT",
    "NO",
    "NONCLUSTERED",
    "NOT",
    "NOWAIT",
    "NULL",
    "NUMERIC",
    "NVARCHAR",
    "OF",
    "OFFSET",
    "ON",
    "ONLY",
    "OPERATOR",
    "OR",
    "ORDER",
    "OUT",
    "OUTER",
    "OVER",
    "OWNER",
    "PACKAGE",
    "PARTITION",
    "PERFORM",
    "PERIOD",
    "POLICY",
    "PRECISION",
    "PRIMARY",
    "PROC",
    "PROCEDURE",
    "PUBLIC",
    "PUBLICATION",
    "RAISE",
    "RANGE",
    "REAL",
    "RECURSIVE",
    "REFERENCES",
    "REINDEX",
    "RENAME",
    "REPEAT",
    "REPLACE",
    "RESET",
    "RESTART",
    "RESTRICT",
    "RETURN",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLE",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "RULE",
    "SAVEPOINT",
    "SCHEMA",
    "SECOND",
    "SECURITY",
    "SELECT",
    "SEQUENCE",
    "SERIAL",
    "SERVER",
    "SESSION",
    "SET",
    "SHARE",
    "SIMILAR",
    "SMALLINT",
    "SOME",
    "SPATIAL",
    "SQL",
    "STABLE",
    "START",
    "STRICT",
    "SUBSCRIPTION",
    "SUM",
    "TABLE",
    "TABLESPACE",
    "TEMP",
    "TEMPORARY",
    "THEN",
    "THROW",
    "TIES",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TINYINT",
    "TO",
    "TRAILING",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "TRY",
    "UNION",
    "UNIQUE",
    "UNKNOWN",
    "UNLOGGED",
    "UNTIL",
    "UPDATE",
    "USER",
    "USING",
    "VACUUM",
    "VALIDATE",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "VARCHAR2",
    "VARIADIC",
    "VARYING",
    "VERBOSE",
    "VIEW",
    "VOLATILE",
    "WHEN",
    "WHENEVER",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "WORK",
    "XML",
    "YEAR",
    "ZONE",
];

const TYPE_KEYWORDS: &[&str] = &[
    "ARRAY",
    "BIGINT",
    "BIGSERIAL",
    "BINARY",
    "BIT",
    "BLOB",
    "BOOL",
    "BOOLEAN",
    "BYTEA",
    "CHAR",
    "CHARACTER",
    "CLOB",
    "DATE",
    "DATETIME",
    "DATETIME2",
    "DECIMAL",
    "DOUBLE",
    "FLOAT",
    "INT",
    "INTEGER",
    "INTERVAL",
    "JSON",
    "JSONB",
    "MONEY",
    "NCHAR",
    "NUMBER",
    "NUMERIC",
    "NVARCHAR",
    "PRECISION",
    "REAL",
    "SERIAL",
    "SMALLINT",
    "TEXT",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TINYINT",
    "UUID",
    "VARBINARY",
    "VARCHAR",
    "VARCHAR2",
    "XML",
];

const fn bytes_lt(a: &[u8], b: &[u8]) -> bool {
    let n = if a.len() < b.len() { a.len() } else { b.len() };
    let mut i = 0;
    while i < n {
        if a[i] < b[i] {
            return true;
        }
        if a[i] > b[i] {
            return false;
        }
        i += 1;
    }
    a.len() < b.len()
}

const fn table_sorted(table: &[&str]) -> bool {
    let mut i = 1;
    while i < table.len() {
        if !bytes_lt(table[i - 1].as_bytes(), table[i].as_bytes()) {
            return false;
        }
        i += 1;
    }
    true
}

const _: () = assert!(table_sorted(KEYWORDS));
const _: () = assert!(table_sorted(TYPE_KEYWORDS));

fn ascii_upper<'a>(word: &str, buf: &'a mut [u8]) -> Option<&'a str> {
    if !word.is_ascii() || word.len() > buf.len() {
        return None;
    }
    for (i, b) in word.bytes().enumerate() {
        buf[i] = b.to_ascii_uppercase();
    }
    std::str::from_utf8(&buf[..word.len()]).ok()
}

pub fn is_keyword(word: &str) -> bool {
    let mut buf = [0u8; 32];
    let Some(upper) = ascii_upper(word, &mut buf) else {
        return false;
    };
    KEYWORDS.binary_search(&upper).is_ok()
}

pub fn is_type_keyword(word: &str) -> bool {
    let mut buf = [0u8; 32];
    let Some(upper) = ascii_upper(word, &mut buf) else {
        return false;
    };
    TYPE_KEYWORDS.binary_search(&upper).is_ok()
}

/// Strip SQL identifier quotes. `"a""b"` → `a"b`; `` `x` `` → `x`;
/// `[x]` → `x`; `U&"x"` → `x`; bare text is returned as written.
pub fn unquote_identifier(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut s = raw;
    if bytes.len() >= 3 && (bytes[0] == b'U' || bytes[0] == b'u') && bytes[1] == b'&' {
        s = &raw[2..];
    }
    let b = s.as_bytes();
    if b.len() >= 2 {
        let open = b[0];
        let close = b[b.len() - 1];
        if open == b'"' && close == b'"' {
            return s[1..s.len() - 1].replace("\"\"", "\"");
        }
        if open == b'`' && close == b'`' {
            return s[1..s.len() - 1].replace("``", "`");
        }
        if open == b'[' && close == b']' {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn word_eq_upper(word: &str, upper: &str) -> bool {
    if word.len() != upper.len() || !word.is_ascii() {
        return false;
    }
    word.bytes()
        .zip(upper.bytes())
        .all(|(a, b)| a.to_ascii_uppercase() == b)
}

fn paren_stay_keyword(word: &str) -> bool {
    word_eq_upper(word, "VALUES")
        || word_eq_upper(word, "IN")
        || word_eq_upper(word, "EXISTS")
        || word_eq_upper(word, "CAST")
        || word_eq_upper(word, "COUNT")
}

fn classify_word(word: &str, next_is_paren: bool) -> TokenRole {
    if is_type_keyword(word) {
        return TokenRole::Typename;
    }
    if paren_stay_keyword(word) {
        return TokenRole::Keyword;
    }
    if next_is_paren && !word.is_empty() {
        return TokenRole::Function;
    }
    if word_eq_upper(word, "CASE")
        || word_eq_upper(word, "WHEN")
        || word_eq_upper(word, "THEN")
        || word_eq_upper(word, "ELSE")
        || word_eq_upper(word, "ELSIF")
        || word_eq_upper(word, "ELSEIF")
        || word_eq_upper(word, "END")
        || word_eq_upper(word, "IF")
        || word_eq_upper(word, "EXCEPTION")
        || word_eq_upper(word, "RETURN")
        || word_eq_upper(word, "RAISE")
        || word_eq_upper(word, "THROW")
        || word_eq_upper(word, "TRY")
        || word_eq_upper(word, "CATCH")
        || word_eq_upper(word, "GOTO")
    {
        return TokenRole::BranchKeyword;
    }
    if word_eq_upper(word, "LOOP")
        || word_eq_upper(word, "WHILE")
        || word_eq_upper(word, "FOR")
        || word_eq_upper(word, "FOREACH")
        || word_eq_upper(word, "REPEAT")
        || word_eq_upper(word, "UNTIL")
        || word_eq_upper(word, "EXIT")
        || word_eq_upper(word, "CONTINUE")
        || word_eq_upper(word, "BREAK")
        || word_eq_upper(word, "ITERATE")
        || word_eq_upper(word, "LEAVE")
    {
        return TokenRole::LoopKeyword;
    }
    if word_eq_upper(word, "TRUE")
        || word_eq_upper(word, "FALSE")
        || word_eq_upper(word, "NULL")
        || word_eq_upper(word, "UNKNOWN")
    {
        return TokenRole::Constant;
    }
    if is_keyword(word) {
        TokenRole::Keyword
    } else {
        TokenRole::Identifier
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_ws(b: u8) -> bool {
    is_space(b) || is_newline(b)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8, dialect: SqlDialect) -> bool {
    if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
        return true;
    }
    b == b'$' && (dialect.dollar_ident() || !dialect.dollar_quoting())
}

fn ident_continue_in_token(b: u8, dialect: SqlDialect) -> bool {
    if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
        return true;
    }
    // `$` continues an already-started identifier in every dialect; a dollar
    // quote is only recognised at token start.
    b == b'$' && (dialect.dollar_ident() || dialect.dollar_quoting())
}

fn tok(start: u32, end: usize, kind: SqlKind) -> SqlToken {
    SqlToken {
        start,
        end: end as u32,
        kind,
    }
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if end == 0 {
        return;
    }
    if out.last().map(|(_, r)| *r) == Some(role) {
        out.last_mut().unwrap().0 = end;
    } else {
        out.push((end, role));
    }
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0'
        && matches!(
            bytes.get(i + 1).map(|b| b.to_ascii_lowercase()),
            Some(b'x')
        )
    {
        let mut j = i + 2;
        if j < bytes.len() && bytes[j].is_ascii_hexdigit() {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                j += 1;
            }
            return j;
        }
        return i + 1;
    }
    if bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        return consume_exponent(bytes, i);
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        if next == b'.' {
            return i;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    consume_exponent(bytes, i)
}

fn consume_exponent(bytes: &[u8], mut i: usize) -> usize {
    if bytes.get(i).map(|b| b.to_ascii_lowercase()) == Some(b'e') {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j + 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"->>") || rest.starts_with(b"#>>") || rest.starts_with(b"!~*") {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"::" | b":="
                | b"<>"
                | b"!="
                | b"<="
                | b">="
                | b"||"
                | b"->"
                | b"#>"
                | b"@>"
                | b"<@"
                | b"?|"
                | b"?&"
                | b"<<"
                | b">>"
                | b"~*"
                | b"!~"
                | b"**"
                | b"=>"
                | b".."
        )
    ) {
        return 2;
    }
    1
}

fn is_delimiter_byte(b: u8, dialect: SqlDialect) -> bool {
    matches!(b, b'(' | b')' | b',' | b';') || (!dialect.bracket_ident() && matches!(b, b'[' | b']'))
}

/// `$tag$` opener at `i`. Tag is `[A-Za-z_][A-Za-z0-9_]*` of at most 16 bytes.
fn dollar_opener(bytes: &[u8], i: usize) -> Option<(usize, [u8; DOLLAR_TAG_CAP], u8)> {
    if bytes.get(i) != Some(&b'$') {
        return None;
    }
    if bytes.get(i + 1) == Some(&b'$') {
        return Some((i + 2, [0; DOLLAR_TAG_CAP], 0));
    }
    let mut j = i + 1;
    let start = j;
    if j >= bytes.len() {
        return None;
    }
    let b = bytes[j];
    if !(b.is_ascii_alphabetic() || b == b'_') {
        return None;
    }
    j += 1;
    while j < bytes.len() {
        let c = bytes[j];
        if c.is_ascii_alphanumeric() || c == b'_' {
            j += 1;
        } else {
            break;
        }
    }
    let tag_len = j - start;
    if tag_len == 0 || tag_len > DOLLAR_TAG_CAP {
        return None;
    }
    if bytes.get(j) != Some(&b'$') {
        return None;
    }
    let mut tag = [0u8; DOLLAR_TAG_CAP];
    tag[..tag_len].copy_from_slice(&bytes[start..j]);
    Some((j + 1, tag, tag_len as u8))
}

fn dollar_closer(bytes: &[u8], i: usize, tag: &[u8]) -> bool {
    if bytes.get(i) != Some(&b'$') {
        return false;
    }
    let after = i + 1;
    if tag.is_empty() {
        return bytes.get(after) == Some(&b'$');
    }
    if after + tag.len() > bytes.len() {
        return false;
    }
    bytes[after..after + tag.len()] == *tag && bytes.get(after + tag.len()) == Some(&b'$')
}

fn string_prefix_end(bytes: &[u8], i: usize, dialect: SqlDialect) -> Option<(usize, u8, bool)> {
    // Returns (quote_index, quote_byte, escape). Prefix bytes [i, quote_index)
    // belong to the string token.
    if i >= bytes.len() {
        return None;
    }
    let b = bytes[i];
    // U&'...' / U&"..."
    if (b == b'U' || b == b'u') && bytes.get(i + 1) == Some(&b'&') {
        match bytes.get(i + 2).copied() {
            Some(q @ (b'\'' | b'"')) => return Some((i + 2, q, false)),
            _ => {}
        }
    }
    // E N B X immediately before a single quote.
    if matches!(b.to_ascii_uppercase(), b'E' | b'N' | b'B' | b'X')
        && bytes.get(i + 1) == Some(&b'\'')
    {
        let escape = b.to_ascii_uppercase() == b'E'
            && matches!(dialect, SqlDialect::Generic | SqlDialect::Postgres);
        return Some((i + 1, b'\'', escape));
    }
    // MySQL _charset introducer.
    if dialect == SqlDialect::MySql && b == b'_' {
        let mut j = i + 1;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if j > i + 1 {
            if let Some(q @ (b'\'' | b'"')) = bytes.get(j).copied() {
                return Some((j, q, true));
            }
        }
    }
    None
}

fn scan_string_body(bytes: &[u8], i: &mut usize, state: &mut SqlState, quote: u8, escape: bool) {
    if state.string_escape {
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        state.string_escape = false;
    }
    while *i < bytes.len() {
        let b = bytes[*i];
        if escape && b == b'\\' {
            *i += 1;
            if *i >= bytes.len() {
                state.string_escape = true;
                return;
            }
            if is_newline(bytes[*i]) {
                // Escaped newline stays inside the string; consume it.
                if bytes[*i] == b'\r' {
                    *i += 1;
                    if bytes.get(*i) == Some(&b'\n') {
                        *i += 1;
                    }
                } else {
                    *i += 1;
                }
                continue;
            }
            *i += 1;
            continue;
        }
        if b == quote {
            *i += 1;
            if bytes.get(*i) == Some(&quote) {
                *i += 1;
                continue;
            }
            state.reset_string();
            return;
        }
        *i += 1;
    }
}

fn scan_dollar_body(bytes: &[u8], i: &mut usize, state: &mut SqlState) {
    let tag_len = state.dollar_tag_len as usize;
    let mut tag = [0u8; DOLLAR_TAG_CAP];
    tag[..tag_len].copy_from_slice(&state.dollar_tag[..tag_len]);
    let tag = &tag[..tag_len];
    while *i < bytes.len() {
        if dollar_closer(bytes, *i, tag) {
            *i += 1 + tag.len() + 1;
            state.reset_string();
            return;
        }
        *i += 1;
    }
}

fn scan_quoted_ident(bytes: &[u8], i: &mut usize, quote: u8, close: u8) -> bool {
    // Returns true if closed before newline/EOF.
    while *i < bytes.len() {
        let b = bytes[*i];
        if is_newline(b) {
            return false;
        }
        if b == close {
            *i += 1;
            if quote == close && bytes.get(*i) == Some(&quote) {
                *i += 1;
                continue;
            }
            return true;
        }
        *i += 1;
    }
    false
}

fn consume_ident(bytes: &[u8], i: &mut usize, dialect: SqlDialect) {
    *i += 1;
    while *i < bytes.len() && ident_continue_in_token(bytes[*i], dialect) {
        *i += 1;
    }
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut SqlState,
    line_start: &mut bool,
) -> Result<SqlToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }
    let dialect = state.dialect;

    match state.mode {
        SqlMode::BlockComment => {
            while *i < bytes.len() {
                if dialect.nested_block_comments()
                    && bytes[*i] == b'/'
                    && bytes.get(*i + 1) == Some(&b'*')
                {
                    *i += 2;
                    state.block_comment_depth = state.block_comment_depth.saturating_add(1);
                    continue;
                }
                if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                    *i += 2;
                    if state.block_comment_depth > 0 {
                        state.block_comment_depth -= 1;
                    }
                    if state.block_comment_depth == 0 {
                        state.mode = SqlMode::Normal;
                        break;
                    }
                    continue;
                }
                if is_newline(bytes[*i]) {
                    *line_start = true;
                } else if !is_space(bytes[*i]) {
                    *line_start = false;
                }
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, SqlKind::Comment));
        }
        SqlMode::String => {
            // Quote lives in dollar_tag[0]; escape-enabled flag in dollar_tag[1]
            // so both survive line continuation without growing state.
            let quote = if state.dollar_tag_len == 1 {
                state.dollar_tag[0]
            } else {
                b'\''
            };
            let enable_escape = dialect == SqlDialect::MySql || state.dollar_tag.get(1) == Some(&1);
            scan_string_body(bytes, i, state, quote, enable_escape);
            if *i == start_i && *i < bytes.len() {
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *line_start = false;
            return Ok(tok(start, *i, SqlKind::String));
        }
        SqlMode::DollarString => {
            scan_dollar_body(bytes, i, state);
            if *i == start_i && *i < bytes.len() {
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *line_start = false;
            return Ok(tok(start, *i, SqlKind::String));
        }
        SqlMode::Normal => {}
    }

    let b = bytes[*i];
    if is_ws(b) {
        *i += 1;
        while *i < bytes.len() && is_ws(bytes[*i]) {
            *i += 1;
        }
        if bytes[start_i..*i].iter().any(|&c| is_newline(c)) {
            *line_start = true;
        }
        return Ok(tok(start, *i, SqlKind::Whitespace));
    }

    let was_line_start = *line_start;
    *line_start = false;

    if was_line_start && dialect.psql_meta() && b == b'\\' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if next.is_ascii_alphabetic() {
            *i += 2;
            while *i < bytes.len() && bytes[*i].is_ascii_alphabetic() {
                *i += 1;
            }
            while *i < bytes.len() && !is_newline(bytes[*i]) {
                *i += 1;
            }
            return Ok(tok(start, *i, SqlKind::MetaCommand));
        }
    }
    if was_line_start && dialect.sqlite_dot_meta() && b == b'.' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if next.is_ascii_alphabetic() {
            *i += 2;
            while *i < bytes.len() && bytes[*i].is_ascii_alphabetic() {
                *i += 1;
            }
            while *i < bytes.len() && !is_newline(bytes[*i]) {
                *i += 1;
            }
            return Ok(tok(start, *i, SqlKind::MetaCommand));
        }
    }

    if b == b'-' && bytes.get(*i + 1) == Some(&b'-') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, SqlKind::Comment));
    }
    if dialect.hash_line_comment() && b == b'#' {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, SqlKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = SqlMode::BlockComment;
        state.block_comment_depth = 1;
        while *i < bytes.len() {
            if dialect.nested_block_comments()
                && bytes[*i] == b'/'
                && bytes.get(*i + 1) == Some(&b'*')
            {
                *i += 2;
                state.block_comment_depth = state.block_comment_depth.saturating_add(1);
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if state.block_comment_depth > 0 {
                    state.block_comment_depth -= 1;
                }
                if state.block_comment_depth == 0 {
                    state.mode = SqlMode::Normal;
                    break;
                }
                continue;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, SqlKind::Comment));
    }

    if let Some((quote_at, quote, escape)) = string_prefix_end(bytes, *i, dialect) {
        *i = quote_at + 1;
        if quote == b'"' && !dialect.double_quote_is_string() {
            let closed = scan_quoted_ident(bytes, i, b'"', b'"');
            if closed {
                return Ok(tok(start, *i, SqlKind::QuotedIdentifier));
            }
            return Ok(tok(start, *i, SqlKind::Unknown));
        }
        state.mode = SqlMode::String;
        state.dollar_tag = [0; DOLLAR_TAG_CAP];
        state.dollar_tag[0] = quote;
        state.dollar_tag_len = 1;
        state.dollar_tag[1] = if escape { 1 } else { 0 };
        state.string_escape = false;
        scan_string_body(bytes, i, state, quote, escape);
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, SqlKind::String));
    }

    if b == b'\'' {
        *i += 1;
        state.mode = SqlMode::String;
        state.dollar_tag = [0; DOLLAR_TAG_CAP];
        state.dollar_tag[0] = b'\'';
        state.dollar_tag_len = 1;
        let escape = dialect == SqlDialect::MySql;
        state.dollar_tag[1] = if escape { 1 } else { 0 };
        state.string_escape = false;
        scan_string_body(bytes, i, state, b'\'', escape);
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, SqlKind::String));
    }

    if b == b'"' {
        *i += 1;
        if dialect.double_quote_is_string() {
            state.mode = SqlMode::String;
            state.dollar_tag = [0; DOLLAR_TAG_CAP];
            state.dollar_tag[0] = b'"';
            state.dollar_tag_len = 1;
            state.dollar_tag[1] = 1;
            state.string_escape = false;
            scan_string_body(bytes, i, state, b'"', true);
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, SqlKind::String));
        }
        let closed = scan_quoted_ident(bytes, i, b'"', b'"');
        if closed {
            return Ok(tok(start, *i, SqlKind::QuotedIdentifier));
        }
        return Ok(tok(start, *i, SqlKind::Unknown));
    }
    if b == b'`' {
        *i += 1;
        let closed = scan_quoted_ident(bytes, i, b'`', b'`');
        if closed {
            return Ok(tok(start, *i, SqlKind::QuotedIdentifier));
        }
        return Ok(tok(start, *i, SqlKind::Unknown));
    }
    if b == b'[' && dialect.bracket_ident() {
        *i += 1;
        let closed = scan_quoted_ident(bytes, i, b'[', b']');
        if closed {
            return Ok(tok(start, *i, SqlKind::QuotedIdentifier));
        }
        return Ok(tok(start, *i, SqlKind::Unknown));
    }

    if dialect.dollar_quoting() && b == b'$' {
        if let Some(d) = bytes.get(*i + 1).copied() {
            if d.is_ascii_digit() {
                *i += 2;
                while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                return Ok(tok(start, *i, SqlKind::Parameter));
            }
        }
        if let Some((body, tag, tag_len)) = dollar_opener(bytes, *i) {
            *i = body;
            state.mode = SqlMode::DollarString;
            state.dollar_tag = tag;
            state.dollar_tag_len = tag_len;
            state.string_escape = false;
            scan_dollar_body(bytes, i, state);
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, SqlKind::String));
        }
        *i += 1;
        return Ok(tok(start, *i, SqlKind::Punctuator));
    }

    if b == b'?' {
        let n = punct_len(bytes, *i);
        if n > 1 {
            *i += n;
            return Ok(tok(start, *i, SqlKind::Punctuator));
        }
        *i += 1;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        return Ok(tok(start, *i, SqlKind::Parameter));
    }

    if b == b'@' {
        let n = punct_len(bytes, *i);
        if n > 1 {
            *i += n;
            return Ok(tok(start, *i, SqlKind::Punctuator));
        }
        *i += 1;
        if bytes.get(*i) == Some(&b'@') {
            *i += 1;
        }
        if *i < bytes.len() && is_ident_start(bytes[*i]) {
            consume_ident(bytes, i, dialect);
            return Ok(tok(start, *i, SqlKind::Variable));
        }
        return Ok(tok(start, *i, SqlKind::Punctuator));
    }

    if b == b':' {
        let n = punct_len(bytes, *i);
        if n > 1 {
            *i += n;
            return Ok(tok(start, *i, SqlKind::Punctuator));
        }
        let prev = if start_i > 0 {
            bytes[start_i - 1]
        } else {
            0
        };
        if prev != b':' {
            if let Some(&n) = bytes.get(*i + 1) {
                if is_ident_start(n) {
                    *i += 1;
                    consume_ident(bytes, i, dialect);
                    return Ok(tok(start, *i, SqlKind::Variable));
                }
            }
        }
        *i += 1;
        return Ok(tok(start, *i, SqlKind::Punctuator));
    }

    if dialect == SqlDialect::TSql && b == b'#' {
        let n = punct_len(bytes, *i);
        if n > 1 {
            *i += n;
            return Ok(tok(start, *i, SqlKind::Punctuator));
        }
        *i += 1;
        if bytes.get(*i) == Some(&b'#') {
            *i += 1;
        }
        if *i < bytes.len() && (is_ident_start(bytes[*i]) || bytes[*i].is_ascii_digit()) {
            while *i < bytes.len() && ident_continue_in_token(bytes[*i], dialect) {
                *i += 1;
            }
            return Ok(tok(start, *i, SqlKind::Identifier));
        }
        return Ok(tok(start, *i, SqlKind::Punctuator));
    }

    if dialect.dollar_ident() && b == b'$' && is_ident_continue(b, dialect) {
        consume_ident(bytes, i, dialect);
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            SqlKind::Keyword
        } else {
            SqlKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }

    if is_ident_start(b) {
        consume_ident(bytes, i, dialect);
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            SqlKind::Keyword
        } else {
            SqlKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }

    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, SqlKind::Number));
    }

    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && is_delimiter_byte(b, dialect) {
        SqlKind::Delimiter
    } else if n == 1 && !b.is_ascii_graphic() {
        SqlKind::Unknown
    } else {
        SqlKind::Punctuator
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn next_is_paren_bytes(bytes: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < bytes.len() && is_ws(bytes[k]) {
        k += 1;
    }
    bytes.get(k) == Some(&b'(')
}

fn role_for_token(bytes: &[u8], t: &SqlToken, next_is_paren: bool) -> TokenRole {
    if t.kind == SqlKind::Keyword || t.kind == SqlKind::Identifier {
        let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
        classify_word(ident, next_is_paren)
    } else {
        t.kind.role()
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    dialect: SqlDialect,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<SqlToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = SqlState::for_dialect(dialect);
    let mut line_start = true;
    let mut i = 0usize;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                if t.end <= t.start {
                    return Err(LexError::Nonprogress { at: t.start });
                }
                tokens.push(t);
            }
            Err(LexError::Nonprogress { at }) => {
                if i == start_i && i < bytes.len() {
                    i += 1;
                    tokens.push(tok(at, i, SqlKind::Unknown));
                    continue;
                }
                return Err(LexError::Nonprogress { at });
            }
            Err(e) => return Err(e),
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: SqlState) -> (SqlState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, SqlMode::Normal);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let next_paren = if t.kind == SqlKind::Keyword || t.kind == SqlKind::Identifier
                    {
                        next_is_paren_bytes(bytes, end)
                    } else {
                        false
                    };
                    let role = role_for_token(bytes, &t, next_paren);
                    push_role(end, role, &mut out);
                }
                i = end.max(i);
            }
            Err(LexError::Nonprogress { .. }) => {
                if i == start && i < bytes.len() {
                    i += 1;
                    push_role(i, TokenRole::Unknown, &mut out);
                } else if i < bytes.len() {
                    i = bytes.len();
                    push_role(i, TokenRole::Unknown, &mut out);
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
        if i == start {
            if i < bytes.len() {
                i += 1;
                push_role(i, TokenRole::Unknown, &mut out);
            } else {
                break;
            }
        }
    }
    if i < bytes.len() {
        let role = match state.mode {
            SqlMode::BlockComment => TokenRole::Comment,
            SqlMode::Normal => TokenRole::Unknown,
            SqlMode::String | SqlMode::DollarString => TokenRole::String,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[SqlToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let next_is_paren = if t.kind == SqlKind::Keyword || t.kind == SqlKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.is_trivia() {
                j += 1;
            }
            j < tokens.len()
                && tokens[j].kind == SqlKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(')
        } else {
            false
        };
        let role = role_for_token(bytes, t, next_is_paren);
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
