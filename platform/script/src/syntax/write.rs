//! Rust values to Splash text. This is the one way text is produced for an
//! edit: strings are escaped here, numbers always read back as numbers, names
//! are checked, and a value that has no Splash spelling (a NaN) is an error
//! instead of text that parses as something else.

/// A value to write.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    /// An integer literal (`3`).
    Int(i64),
    /// A float literal (`3.0`, `0.25`, `1e-7`).
    Num(f64),
    /// A single-precision float, written with its shortest round-trip digits.
    F32(f32),
    Str(String),
    /// `@name`.
    Name(String),
    /// `#rrggbb` / `#rrggbbaa` / `#rgb`, given as the hex digits.
    Color(String),
    /// A variable or dotted path (`accent`, `mod.motion.Rect`).
    Ident(String),
    /// `vec2(x, y)` / `vec3` / `vec4` from two to four numbers.
    Vec(Vec<f64>),
    /// `callee(args...)`.
    Call(String, Vec<Value>),
    /// `Type{name: value ...}`; no type for a bare object.
    Object(Option<String>, Vec<(String, Value)>),
    Array(Vec<Value>),
    /// `let name = value` / `var name = value`.
    Let { keyword: &'static str, name: String, value: Box<Value> },
    /// Text copied from an existing node (checked to parse as one value).
    Verbatim(String),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }
    pub fn ident(s: impl Into<String>) -> Value {
        Value::Ident(s.into())
    }
    pub fn call(callee: impl Into<String>, args: Vec<Value>) -> Value {
        Value::Call(callee.into(), args)
    }
    pub fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(None, fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn typed(ty: &str, fields: Vec<(&str, Value)>) -> Value {
        Value::Object(Some(ty.to_string()), fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn let_(name: impl Into<String>, value: Value) -> Value {
        Value::Let { keyword: "let", name: name.into(), value: Box::new(value) }
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Num(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::F32(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::Int(v as i64)
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Value {
        Value::Int(v as i64)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}

/// A Splash string literal for `text`, double-quoted.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A float literal that reads back as the same number and as a float.
pub fn number(v: f64) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("{v} has no Splash literal"));
    }
    let mut s = format!("{v}");
    if !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    Ok(s)
}

/// [`number`] for a single-precision value: `0.1f32` is `0.1`, not `0.10000000149`.
pub fn number_f32(v: f32) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("{v} has no Splash literal"));
    }
    let mut s = format!("{v}");
    if !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    Ok(s)
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_path(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_ident)
}

/// An object key: a plain name, or a quoted string for anything else.
pub fn key(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("a field name cannot be empty".into());
    }
    if is_ident(name) && !is_reserved(name) {
        Ok(name.to_string())
    } else {
        Ok(quote(name))
    }
}

fn is_reserved(name: &str) -> bool {
    matches!(name, "let" | "var" | "fn" | "use" | "if" | "else" | "elif" | "for" | "while" | "loop" | "match" | "try" | "ok" | "return" | "break" | "continue" | "true" | "false" | "nil" | "me" | "scope" | "in")
}

/// How wide a line may get before objects and arrays break into lines.
pub const WIDTH: usize = 100;

/// Write `value` compactly on one line.
pub fn write(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    compact(value, &mut out)?;
    Ok(out)
}

/// Write `value`; containers that would pass [`WIDTH`] break into one entry
/// per line, indented by four spaces per level from `indent`.
pub fn write_indented(value: &Value, indent: &str) -> Result<String, String> {
    let mut out = String::new();
    pretty(value, indent, &mut out)?;
    Ok(out)
}

fn compact(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Nil => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Num(n) => out.push_str(&number(*n)?),
        Value::F32(n) => out.push_str(&number_f32(*n)?),
        Value::Str(s) => out.push_str(&quote(s)),
        Value::Name(n) => {
            if !is_path(n) {
                return Err(format!("`@{n}` is not a name"));
            }
            out.push('@');
            out.push_str(n);
        }
        Value::Color(c) => {
            if !matches!(c.len(), 3 | 4 | 6 | 8) || !c.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("`#{c}` is not a colour (3, 4, 6 or 8 hex digits)"));
            }
            out.push('#');
            out.push_str(c);
        }
        Value::Ident(i) => {
            if !is_path(i) {
                return Err(format!("`{i}` is not an identifier"));
            }
            out.push_str(i);
        }
        Value::Vec(v) => {
            if !(2..=4).contains(&v.len()) {
                return Err(format!("a vector has 2 to 4 numbers, not {}", v.len()));
            }
            out.push_str(&format!("vec{}(", v.len()));
            for (i, n) in v.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&number(*n)?);
            }
            out.push(')');
        }
        Value::Call(callee, args) => {
            if !is_path(callee) {
                return Err(format!("`{callee}` is not a callable path"));
            }
            out.push_str(callee);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                compact(a, out)?;
            }
            out.push(')');
        }
        Value::Object(ty, fields) => {
            if let Some(ty) = ty {
                if !is_path(ty) {
                    return Err(format!("`{ty}` is not a type name"));
                }
                out.push_str(ty);
            }
            out.push('{');
            for (i, (k, v)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&key(k)?);
                out.push_str(": ");
                compact(v, out)?;
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                compact(v, out)?;
            }
            out.push(']');
        }
        Value::Let { keyword, name, value } => {
            if !is_ident(name) || is_reserved(name) {
                return Err(format!("`{name}` is not a binding name"));
            }
            out.push_str(keyword);
            out.push(' ');
            out.push_str(name);
            out.push_str(" = ");
            compact(value, out)?;
        }
        Value::Verbatim(text) => {
            super::parse_value(text).map_err(|e| format!("verbatim text does not parse: {e}"))?;
            out.push_str(text);
        }
    }
    Ok(())
}

fn pretty(value: &Value, indent: &str, out: &mut String) -> Result<(), String> {
    let flat = write(value)?;
    let multi = |s: &str| s.contains('\n');
    let fits = indent.len() + flat.len() <= WIDTH && !multi(&flat);
    let inner = format!("{indent}    ");
    match value {
        Value::Object(ty, fields) if !fits && !fields.is_empty() => {
            if let Some(ty) = ty {
                out.push_str(ty);
            }
            out.push_str("{\n");
            for (k, v) in fields {
                out.push_str(&inner);
                out.push_str(&key(k)?);
                out.push_str(": ");
                pretty(v, &inner, out)?;
                out.push('\n');
            }
            out.push_str(indent);
            out.push('}');
        }
        Value::Array(items) if !fits && !items.is_empty() => {
            out.push_str("[\n");
            for v in items {
                out.push_str(&inner);
                pretty(v, &inner, out)?;
                out.push_str(",\n");
            }
            out.push_str(indent);
            out.push(']');
        }
        Value::Call(callee, args) if !fits && !args.is_empty() => {
            // Validate through the compact writer first (already done by `write`).
            out.push_str(callee);
            out.push('(');
            // A single object / array argument stays on the call's line.
            if args.len() == 1 && matches!(args[0], Value::Object(..) | Value::Array(_)) {
                pretty(&args[0], indent, out)?;
            } else {
                out.push('\n');
                for (i, a) in args.iter().enumerate() {
                    out.push_str(&inner);
                    pretty(a, &inner, out)?;
                    if i + 1 < args.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(indent);
            }
            out.push(')');
        }
        Value::Let { keyword, name, value } if !fits => {
            out.push_str(keyword);
            out.push(' ');
            out.push_str(name);
            out.push_str(" = ");
            pretty(value, indent, out)?;
        }
        _ => out.push_str(&flat),
    }
    Ok(())
}
