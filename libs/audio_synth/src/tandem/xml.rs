//! A small lenient XML reader and writer for patch documents: elements,
//! attributes, entities, comments, declarations; text content is kept
//! only as an element's concatenated text. Input that is not valid UTF-8
//! is read as Latin-1. Parsing never panics; broken tails end the tree.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn new(name: &str) -> Element {
        Element { name: name.to_string(), ..Default::default() }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some(a) => a.1 = value,
            None => self.attrs.push((name.to_string(), value)),
        }
    }

    pub fn with(mut self, name: &str, value: impl Into<String>) -> Element {
        self.set(name, value);
        self
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// An attribute read like C's atof: the longest numeric prefix, else None.
    pub fn attr_f64(&self, name: &str) -> Option<f64> {
        self.attr(name).and_then(atof)
    }

    /// An attribute read like C's atoi (a float text truncates).
    pub fn attr_i64(&self, name: &str) -> Option<i64> {
        self.attr(name).and_then(atoi)
    }
}

/// The numeric prefix of `s` as f64 (leading spaces, sign, digits, one
/// point, exponent; `inf`/`nan` too); None when nothing numeric starts it.
pub fn atof(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let lower = t[i..].to_ascii_lowercase();
    if lower.starts_with("inf") || lower.starts_with("nan") {
        let v: f64 = if lower.starts_with("inf") { f64::INFINITY } else { f64::NAN };
        return Some(if b.first() == Some(&b'-') { -v } else { v });
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == digits_start || (i == digits_start + 1 && b[digits_start] == b'.') {
        return None;
    }
    let mantissa_end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let k = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > k {
            i = j;
        }
    }
    t[..i].parse::<f64>().ok().or_else(|| t[..mantissa_end].parse::<f64>().ok())
}

/// The integer prefix of `s` (C atoi: sign + digits; "3.7" is 3).
pub fn atoi(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    // Saturate like a long would on overflow.
    t[..i].parse::<i64>().ok().or(Some(if b[0] == b'-' { i64::MIN } else { i64::MAX }))
}

/// Bytes to text: UTF-8 if valid, else Latin-1.
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Parse a document; the root element (None if there is none).
pub fn parse(src: &str) -> Option<Element> {
    let mut p = Parser { s: src.as_bytes(), src, i: 0 };
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    while p.i < p.s.len() {
        if p.s[p.i] != b'<' {
            let start = p.i;
            while p.i < p.s.len() && p.s[p.i] != b'<' {
                p.i += 1;
            }
            let t = &src[start..p.i];
            if let Some(top) = stack.last_mut().filter(|_| !t.trim().is_empty()) {
                top.text.push_str(&unescape(t));
            }
            continue;
        }
        if p.starts("<!--") {
            p.skip_past("-->");
            continue;
        }
        if p.starts("<![CDATA[") {
            let start = p.i + 9;
            p.skip_past("]]>");
            let end = p.i.saturating_sub(3).max(start);
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&src[start..end]);
            }
            continue;
        }
        if p.starts("<?") {
            p.skip_past("?>");
            continue;
        }
        if p.starts("<!") {
            p.skip_past(">");
            continue;
        }
        if p.starts("</") {
            p.i += 2;
            let _name = p.name();
            p.skip_past(">");
            if let Some(done) = stack.pop() {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => {
                        if root.is_none() {
                            root = Some(done);
                        }
                    }
                }
            }
            continue;
        }
        // Start tag.
        p.i += 1;
        let name = p.name();
        if name.is_empty() {
            p.i += 1;
            continue;
        }
        let mut el = Element::new(name);
        let mut self_close = false;
        loop {
            p.ws();
            if p.i >= p.s.len() {
                break;
            }
            if p.s[p.i] == b'/' {
                self_close = true;
                p.i += 1;
                continue;
            }
            if p.s[p.i] == b'>' {
                p.i += 1;
                break;
            }
            let key = p.name();
            if key.is_empty() {
                p.i += 1;
                continue;
            }
            p.ws();
            let mut value = String::new();
            if p.i < p.s.len() && p.s[p.i] == b'=' {
                p.i += 1;
                p.ws();
                if p.i < p.s.len() && (p.s[p.i] == b'"' || p.s[p.i] == b'\'') {
                    let q = p.s[p.i];
                    p.i += 1;
                    let start = p.i;
                    while p.i < p.s.len() && p.s[p.i] != q {
                        p.i += 1;
                    }
                    value = unescape(&src[start..p.i.min(p.s.len())]);
                    p.i += 1;
                } else {
                    let start = p.i;
                    while p.i < p.s.len() && !p.s[p.i].is_ascii_whitespace() && p.s[p.i] != b'>' && p.s[p.i] != b'/' {
                        p.i += 1;
                    }
                    value = unescape(&src[start..p.i]);
                }
            }
            el.attrs.push((key.to_string(), value));
        }
        if self_close {
            match stack.last_mut() {
                Some(parent) => parent.children.push(el),
                None => {
                    if root.is_none() {
                        root = Some(el);
                    }
                }
            }
        } else {
            stack.push(el);
        }
    }
    // Unclosed elements fold into their parents.
    while let Some(done) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(done),
            None => {
                if root.is_none() {
                    root = Some(done);
                }
            }
        }
    }
    root
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn starts(&self, t: &str) -> bool {
        self.s[self.i..].starts_with(t.as_bytes())
    }
    fn skip_past(&mut self, t: &str) {
        let t = t.as_bytes();
        while self.i < self.s.len() {
            if self.s[self.i..].starts_with(t) {
                self.i += t.len();
                return;
            }
            self.i += 1;
        }
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn name(&mut self) -> &'a str {
        let start = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.' || c == b':' || c >= 0x80 {
                self.i += 1;
            } else {
                break;
            }
        }
        // Stay on a char boundary.
        while !self.src.is_char_boundary(self.i) {
            self.i += 1;
        }
        &self.src[start..self.i]
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest.find(';').filter(|e| *e <= 10);
        let Some(e) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..e];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
}

/// Write a document with a declaration, one element a line, indented.
pub fn write(root: &Element) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n");
    write_el(root, 0, &mut out);
    out
}

fn write_el(e: &Element, depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("    ");
    }
    out.push('<');
    out.push_str(&e.name);
    for (k, v) in &e.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape(v, out);
        out.push('"');
    }
    if e.children.is_empty() && e.text.is_empty() {
        out.push_str(" />\n");
        return;
    }
    out.push('>');
    if !e.text.is_empty() {
        escape(&e.text, out);
    }
    if !e.children.is_empty() {
        out.push('\n');
        for c in &e.children {
            write_el(c, depth + 1, out);
        }
        for _ in 0..depth {
            out.push_str("    ");
        }
    }
    out.push_str("</");
    out.push_str(&e.name);
    out.push_str(">\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attributes_entities_and_nesting() {
        let x = r#"<?xml version="1.0"?><!-- c --><patch revision="9"><meta name="A &amp; B" category='Leads'><tags><tag tag="x"/></tags></meta><parameters><a_pitch type="2" value="0.25"><modrouting source="6" depth="1.5"/></a_pitch></parameters></patch>"#;
        let r = parse(x).unwrap();
        assert_eq!(r.name, "patch");
        assert_eq!(r.attr_i64("revision"), Some(9));
        let meta = r.child("meta").unwrap();
        assert_eq!(meta.attr("name"), Some("A & B"));
        assert_eq!(meta.attr("category"), Some("Leads"));
        let p = r.child("parameters").unwrap().child("a_pitch").unwrap();
        assert_eq!(p.attr_f64("value"), Some(0.25));
        assert_eq!(p.child("modrouting").unwrap().attr_f64("depth"), Some(1.5));
        let back = parse(&write(&r)).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn c_style_numbers() {
        assert_eq!(atof("0.47142899036407"), Some(0.47142899036407));
        assert_eq!(atof(" -3e2x"), Some(-300.0));
        assert_eq!(atof("1e"), Some(1.0));
        assert_eq!(atof("abc"), None);
        assert_eq!(atoi("3.7"), Some(3));
        assert_eq!(atoi("-12"), Some(-12));
        assert_eq!(decode_text(&[0x41, 0xE9]), "Aé");
    }
}
