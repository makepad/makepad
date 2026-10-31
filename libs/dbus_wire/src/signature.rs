use crate::{Error, Result};

/// A validated, borrowed D-Bus signature. Iteration yields complete types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature<'a>(&'a str);

impl<'a> Signature<'a> {
    pub fn new(text: &'a str) -> Result<Self> {
        if text.len() > 255 {
            return Err(Error::invalid("signature exceeds 255 bytes"));
        }
        let mut at = 0;
        while at < text.len() {
            at = parse_type(text.as_bytes(), at, false, 0, 0)?;
        }
        Ok(Self(text))
    }

    pub fn single(text: &'a str) -> Result<Self> {
        let sig = Self::new(text)?;
        if sig.types().count() != 1 {
            return Err(Error::invalid("expected exactly one complete type"));
        }
        Ok(sig)
    }

    pub fn as_str(self) -> &'a str {
        self.0
    }

    pub fn types(self) -> impl Iterator<Item = &'a str> {
        types(self.0)
    }
}

pub(crate) fn types(mut text: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        // Only called with an already validated signature.
        let end = parse_type(text.as_bytes(), 0, true, 0, 0).ok()?;
        let (head, tail) = text.split_at(end);
        text = tail;
        Some(head)
    })
}

pub(crate) fn element(text: &str) -> Result<()> {
    if text.len() > 255 || parse_type(text.as_bytes(), 0, true, 1, 0)? != text.len() {
        return Err(Error::invalid("expected one array element type"));
    }
    Ok(())
}

fn basic(code: u8) -> bool {
    b"ybnqiuxtdsogh".contains(&code)
}

fn parse_type(s: &[u8], at: usize, dict: bool, arrays: usize, structs: usize) -> Result<usize> {
    let bad = || Error::invalid("malformed D-Bus signature");
    match *s.get(at).ok_or_else(bad)? {
        c if basic(c) || c == b'v' => Ok(at + 1),
        b'a' if arrays < 32 => parse_type(s, at + 1, true, arrays + 1, structs),
        b'(' if structs < 32 => {
            let mut next = at + 1;
            while s.get(next) != Some(&b')') {
                next = parse_type(s, next, false, arrays, structs + 1)?;
            }
            if next == at + 1 {
                return Err(bad());
            }
            Ok(next + 1)
        }
        b'{' if dict && structs < 32 => {
            if !s.get(at + 1).is_some_and(|c| basic(*c)) {
                return Err(bad());
            }
            let end = parse_type(s, at + 2, false, arrays, structs + 1)?;
            if s.get(end) != Some(&b'}') {
                return Err(bad());
            }
            Ok(end + 1)
        }
        _ => Err(bad()),
    }
}

pub(crate) fn alignment(code: u8) -> usize {
    match code {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

pub fn validate_object_path(path: &str) -> Result<()> {
    if path == "/"
        || (path.starts_with('/')
            && path[1..].split('/').all(|part| {
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            }))
    {
        Ok(())
    } else {
        Err(Error::invalid("invalid object path"))
    }
}

pub(crate) fn validate_member(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name.as_bytes()[0].is_ascii_digit()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::invalid("invalid member name"));
    }
    Ok(())
}

pub(crate) fn validate_interface(name: &str) -> Result<()> {
    if name.len() > 255 || !name.contains('.') {
        return Err(Error::invalid("invalid interface or error name"));
    }
    for part in name.split('.') {
        validate_member(part)?;
    }
    Ok(())
}

pub(crate) fn validate_bus_name(name: &str) -> Result<()> {
    let unique = name.starts_with(':');
    let body = name.strip_prefix(':').unwrap_or(name);
    if name.len() > 255
        || !body.contains('.')
        || body.split('.').any(|part| {
            part.is_empty()
                || (!unique && part.as_bytes()[0].is_ascii_digit())
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err(Error::invalid("invalid bus name"));
    }
    Ok(())
}
