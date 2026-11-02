//! A deliberately small reader: tables, string assignments, dependency inline tables and
//! Cargo.lock string arrays. Unsupported TOML makes digest lookup fall back to dates.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Token { Text(String), String(String), Punct(char), Newline }

#[derive(Default)]
pub struct Table {
    pub name: String,
    pub array: bool,
    pub fields: BTreeMap<String, Vec<Token>>,
}

pub fn string(tokens: &[Token]) -> Option<&str> {
    match tokens { [Token::String(s)] => Some(s), _ => None }
}

pub fn field<'a>(tokens: &'a [Token], key: &str) -> Option<&'a [Token]> {
    // Values are sliced at commas only at the inline table's outer level.
    let [Token::Punct('{'), rest @ .., Token::Punct('}')] = tokens else { return None };
    let mut start = 0;
    let mut depth = 0;
    for i in 0..=rest.len() {
        if i == rest.len() || (depth == 0 && rest[i] == Token::Punct(',')) {
            if let [Token::Text(k), Token::Punct('='), value @ ..] = &rest[start..i] {
                if k == key { return Some(value); }
            }
            start = i + 1;
        } else {
            match rest[i] {
                Token::Punct('{' | '[') => depth += 1,
                Token::Punct('}' | ']') => depth -= 1,
                _ => {}
            }
        }
    }
    None
}

fn tokens(text: &str) -> Result<Vec<Token>, ()> {
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut out = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push(Token::Newline),
            c if c.is_whitespace() => {}
            '#' => { while chars.peek().is_some_and(|c| *c != '\n') { chars.next(); } }
            '[' | ']' | '{' | '}' | '=' | ',' => out.push(Token::Punct(c)),
            '"' | '\'' => {
                let mut value = String::new();
                loop {
                    match chars.next().ok_or(())? {
                        end if end == c => break,
                        '\\' if c == '"' => match chars.next().ok_or(())? {
                            '"' => value.push('"'), '\\' => value.push('\\'),
                            'n' => value.push('\n'), 'r' => value.push('\r'), 't' => value.push('\t'),
                            'b' => value.push('\u{8}'), 'f' => value.push('\u{c}'),
                            u @ ('u' | 'U') => {
                                let mut hex = String::new();
                                for _ in 0..if u == 'u' { 4 } else { 8 } { hex.push(chars.next().ok_or(())?); }
                                value.push(char::from_u32(u32::from_str_radix(&hex, 16).map_err(|_| ())?).ok_or(())?);
                            }
                            _ => return Err(()),
                        },
                        '\n' | '\r' => return Err(()),
                        c => value.push(c),
                    }
                }
                out.push(Token::String(value));
            }
            _ => {
                let mut value = c.to_string();
                while chars.peek().is_some_and(|c| !c.is_whitespace() && !"[]{}=,#\"'".contains(*c)) {
                    value.push(chars.next().ok_or(())?);
                }
                out.push(Token::Text(value));
            }
        }
    }
    Ok(out)
}

pub fn parse(text: &str) -> Result<Vec<Table>, ()> {
    let tokens = tokens(text)?;
    let mut tables = vec![Table::default()];
    let mut at = 0;
    while at < tokens.len() {
        if tokens[at] == Token::Newline { at += 1; continue; }
        if tokens[at] == Token::Punct('[') {
            at += 1;
            let array = tokens.get(at) == Some(&Token::Punct('['));
            if array { at += 1; }
            let mut name = String::new();
            while let Some(Token::Text(s) | Token::String(s)) = tokens.get(at) { name.push_str(s); at += 1; }
            if name.is_empty() || tokens.get(at) != Some(&Token::Punct(']')) { return Err(()); }
            at += 1;
            if array {
                if tokens.get(at) != Some(&Token::Punct(']')) { return Err(()); }
                at += 1;
            }
            if tokens.get(at).is_some_and(|t| *t != Token::Newline) { return Err(()); }
            tables.push(Table { name, array, fields: BTreeMap::new() });
        } else {
            let key = match tokens.get(at) { Some(Token::Text(s) | Token::String(s)) => s.clone(), _ => return Err(()) };
            if tokens.get(at + 1) != Some(&Token::Punct('=')) { return Err(()); }
            at += 2;
            let mut stack = Vec::new();
            let mut value = Vec::new();
            while at < tokens.len() && !(tokens[at] == Token::Newline && stack.is_empty()) {
                match tokens[at] {
                    Token::Punct('[') => stack.push(']'),
                    Token::Punct('{') => stack.push('}'),
                    Token::Punct(c @ (']' | '}')) => {
                        if stack.pop() != Some(c) { return Err(()); }
                    }
                    _ => {}
                }
                if tokens[at] != Token::Newline { value.push(tokens[at].clone()); }
                at += 1;
            }
            if !stack.is_empty() || value.is_empty() { return Err(()); }
            if tables.last_mut().ok_or(())?.fields.insert(key, value).is_some() { return Err(()); }
        }
    }
    Ok(tables)
}

pub fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
