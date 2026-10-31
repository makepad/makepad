//! Small XML reader for the checked-in binding generators. No external entities.
use std::collections::BTreeMap;

#[derive(Default, Debug)]
pub struct Element {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<usize>,
    /// Text including descendants, in document order (C declarations need this).
    pub text: String,
}
impl Element {
    pub fn attr(&self, name: &str) -> &str {
        self.attrs.get(name).map(String::as_str).unwrap_or("")
    }
}
pub struct Document {
    pub nodes: Vec<Element>,
}
impl Document {
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut doc = Self { nodes: vec![Element::default()] };
        let mut stack = vec![0usize];
        let mut at = 0;
        while at < source.len() {
            let rest = &source[at..];
            if rest.starts_with("<!--") {
                at += rest.find("-->").ok_or("unterminated XML comment")? + 3;
            } else if rest.starts_with("<?") {
                at += rest.find("?>").ok_or("unterminated XML instruction")? + 2;
            } else if rest.starts_with("<![CDATA[") {
                let end = rest.find("]]>").ok_or("unterminated CDATA")?;
                doc.nodes[*stack.last().unwrap()].text.push_str(&rest[9..end]);
                at += end + 3;
            } else if rest.starts_with("<!DOCTYPE") {
                at += declaration_end(rest)?;
            } else if rest.starts_with("</") {
                let end = rest.find('>').ok_or("unterminated XML closing tag")?;
                if stack.len() == 1 { return Err("unexpected XML closing tag".into()); }
                let id = stack.pop().unwrap();
                if doc.nodes[id].name != rest[2..end].trim() {
                    return Err(format!("XML closing tag mismatch: {}", &rest[..=end]));
                }
                let text = doc.nodes[id].text.clone();
                doc.nodes[*stack.last().unwrap()].text.push_str(&text);
                at += end + 1;
            } else if rest.starts_with('<') {
                let end = tag_end(rest)?;
                let inside = rest[1..end].trim();
                let empty = inside.ends_with('/');
                let inside = if empty { inside[..inside.len()-1].trim_end() } else { inside };
                let element = start_tag(inside)?;
                let id = doc.nodes.len();
                doc.nodes.push(element);
                doc.nodes[*stack.last().unwrap()].children.push(id);
                if !empty { stack.push(id); }
                at += end + 1;
            } else {
                let end = rest.find('<').unwrap_or(rest.len());
                doc.nodes[*stack.last().unwrap()].text.push_str(&unescape(&rest[..end])?);
                at += end;
            }
        }
        if stack.len() != 1 { return Err("unclosed XML element".into()); }
        Ok(doc)
    }
    pub fn children(&self, id: usize, name: &str) -> Vec<usize> {
        self.nodes[id].children.iter().copied().filter(|&i| self.nodes[i].name == name).collect()
    }
    pub fn child(&self, id: usize, name: &str) -> Option<usize> {
        self.nodes[id].children.iter().copied().find(|&i| self.nodes[i].name == name)
    }
    pub fn child_text(&self, id: usize, name: &str) -> &str {
        self.child(id, name).map(|i| self.nodes[i].text.trim()).unwrap_or("")
    }
}
fn tag_end(text: &str) -> Result<usize, String> {
    let mut quote = 0;
    for (i, b) in text.bytes().enumerate().skip(1) {
        if quote != 0 {
            if b == quote { quote = 0; }
        } else if b == b'\'' || b == b'"' { quote = b; }
        else if b == b'>' { return Ok(i); }
    }
    Err("unterminated XML opening tag".into())
}
fn declaration_end(text: &str) -> Result<usize, String> {
    let mut depth = 0;
    let mut quote = 0;
    for (i, b) in text.bytes().enumerate() {
        if quote != 0 { if quote == b { quote = 0; } }
        else if b == b'\'' || b == b'"' { quote = b; }
        else if b == b'[' { depth += 1; }
        else if b == b']' { depth -= 1; }
        else if b == b'>' && depth == 0 { return Ok(i + 1); }
    }
    Err("unterminated XML declaration".into())
}
fn start_tag(text: &str) -> Result<Element, String> {
    let first = text.find(char::is_whitespace).unwrap_or(text.len());
    if first == 0 { return Err("empty XML element name".into()); }
    let mut element = Element { name: text[..first].into(), ..Element::default() };
    let mut rest = text[first..].trim_start();
    while !rest.is_empty() {
        let end = rest.find('=').ok_or("XML attribute lacks =")?;
        let key = rest[..end].trim();
        if key.is_empty() || key.contains(char::is_whitespace) { return Err("invalid XML attribute".into()); }
        rest = rest[end+1..].trim_start();
        let quote = rest.chars().next().ok_or("missing XML attribute value")?;
        if quote != '\'' && quote != '"' { return Err("unquoted XML attribute".into()); }
        rest = &rest[1..];
        let end = rest.find(quote).ok_or("unterminated XML attribute value")?;
        if element.attrs.insert(key.into(), unescape(&rest[..end])?).is_some() {
            return Err(format!("duplicate XML attribute {key}"));
        }
        rest = rest[end+1..].trim_start();
    }
    Ok(element)
}
fn unescape(text: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        output.push_str(&rest[..at]);
        rest = &rest[at+1..];
        let end = rest.find(';').ok_or("unterminated XML entity")?;
        let name = &rest[..end];
        match name {
            "lt" => output.push('<'), "gt" => output.push('>'), "amp" => output.push('&'),
            "quot" => output.push('"'), "apos" => output.push('\''),
            _ => {
                let value = if let Some(hex) = name.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else { name.strip_prefix('#').and_then(|s| s.parse::<u32>().ok()) };
                output.push(value.and_then(char::from_u32).ok_or_else(|| format!("unsupported XML entity &{name};"))?);
            }
        }
        rest = &rest[end+1..];
    }
    output.push_str(rest);
    Ok(output)
}
