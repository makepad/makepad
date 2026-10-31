//! A small Rust symbol demangler (v0 `_R…` and legacy `_ZN…E`) for size
//! reports. Besides the readable name it gives the owner path: the crate and
//! module segments the code belongs to (for an impl method, the module of the
//! impl; for a trait default method, the implementing type), which is what a
//! per-crate breakdown groups by.

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Demangled {
    pub name: String,
    /// Crate first, then modules/items down to the function itself.
    pub owner: Vec<String>,
}

pub fn demangle(symbol: &str) -> Option<Demangled> {
    if let Some(rest) = symbol.strip_prefix("_R") {
        let mut p = V0 {
            s: rest.as_bytes(),
            pos: 0,
            depth: 0,
        };
        // An optional encoding version.
        while p.peek().map_or(false, |c| c.is_ascii_digit()) {
            p.pos += 1;
        }
        let path = p.path().ok()?;
        return Some(Demangled {
            name: path.text,
            owner: path.owner,
        });
    }
    let rest = symbol
        .strip_prefix("_ZN")
        .or_else(|| symbol.strip_prefix("__ZN"))?;
    legacy(rest)
}

fn legacy(s: &str) -> Option<Demangled> {
    let bytes = s.as_bytes();
    let mut pos = 0;
    let mut parts = Vec::new();
    while pos < bytes.len() && bytes[pos] != b'E' {
        let start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        let len: usize = s[start..pos].parse().ok()?;
        let part = s.get(pos..pos + len)?;
        pos += len;
        parts.push(part);
    }
    if let Some(last) = parts.last() {
        if last.len() == 17 && last.starts_with('h') && last[1..].bytes().all(|c| c.is_ascii_hexdigit()) {
            parts.pop();
        }
    }
    let parts: Vec<String> = parts.iter().map(|part| legacy_unescape(part)).collect();
    if parts.is_empty() {
        return None;
    }
    Some(Demangled {
        name: parts.join("::"),
        owner: parts,
    })
}

fn legacy_unescape(part: &str) -> String {
    // An identifier starting with `$` gets a leading `_`.
    let part = if part.starts_with("_$") { &part[1..] } else { part };
    let mut out = String::new();
    let mut rest = part;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("..") {
            out.push_str("::");
            rest = after;
            continue;
        }
        if rest.starts_with('$') {
            if let Some(end) = rest[1..].find('$') {
                let code = &rest[1..1 + end];
                let ch = match code {
                    "SP" => Some('@'),
                    "BP" => Some('*'),
                    "RF" => Some('&'),
                    "LT" => Some('<'),
                    "GT" => Some('>'),
                    "LP" => Some('('),
                    "RP" => Some(')'),
                    "C" => Some(','),
                    _ => code
                        .strip_prefix('u')
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .and_then(char::from_u32),
                };
                if let Some(ch) = ch {
                    out.push(ch);
                    rest = &rest[end + 2..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

struct V0<'a> {
    s: &'a [u8],
    pos: usize,
    depth: u32,
}

struct Path {
    text: String,
    owner: Vec<String>,
}

type R<T> = Result<T, ()>;

/// Deeply nested (or backref-cyclic) input is refused rather than recursed.
const MAX_DEPTH: u32 = 200;

impl<'a> V0<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn next(&mut self) -> R<u8> {
        let c = self.peek().ok_or(())?;
        self.pos += 1;
        Ok(c)
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn base62(&mut self) -> R<u64> {
        if self.eat(b'_') {
            return Ok(0);
        }
        let mut value: u64 = 0;
        loop {
            let c = self.next()?;
            if c == b'_' {
                return value.checked_add(1).ok_or(());
            }
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'z' => c - b'a' + 10,
                b'A'..=b'Z' => c - b'A' + 36,
                _ => return Err(()),
            } as u64;
            value = value.checked_mul(62).and_then(|v| v.checked_add(digit)).ok_or(())?;
        }
    }

    fn decimal(&mut self) -> R<usize> {
        let start = self.pos;
        while self.peek().map_or(false, |c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.s[start..self.pos])
            .map_err(|_| ())?
            .parse()
            .map_err(|_| ())
    }

    fn disambiguator(&mut self) -> R<u64> {
        if self.eat(b's') {
            Ok(self.base62()? + 1)
        } else {
            Ok(0)
        }
    }

    fn ident(&mut self) -> R<(String, u64)> {
        let dis = self.disambiguator()?;
        Ok((self.undisambiguated_ident()?, dis))
    }

    fn undisambiguated_ident(&mut self) -> R<String> {
        let punycode = self.eat(b'u');
        let len = self.decimal()?;
        self.eat(b'_');
        let end = self.pos.checked_add(len).ok_or(())?;
        let bytes = self.s.get(self.pos..end).ok_or(())?;
        self.pos = end;
        let text = String::from_utf8_lossy(bytes).into_owned();
        // Punycode identifiers are shown encoded; they are rare in code.
        Ok(if punycode { format!("{text}(punycode)") } else { text })
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(());
        }
        Ok(())
    }

    fn backref<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let start = self.pos - 1;
        let target = self.base62()? as usize;
        if target >= start {
            return Err(());
        }
        let saved = self.pos;
        self.pos = target;
        let out = f(self);
        self.pos = saved;
        out
    }

    fn path(&mut self) -> R<Path> {
        self.enter()?;
        let out = self.path_inner();
        self.depth -= 1;
        out
    }

    fn path_inner(&mut self) -> R<Path> {
        match self.next()? {
            b'C' => {
                let (name, _) = self.ident()?;
                Ok(Path {
                    text: name.clone(),
                    owner: vec![name],
                })
            }
            b'M' => {
                self.disambiguator()?;
                let impl_path = self.path()?;
                let ty = self.ty()?;
                Ok(Path {
                    text: format!("<{ty}>"),
                    owner: impl_path.owner,
                })
            }
            b'X' => {
                self.disambiguator()?;
                let impl_path = self.path()?;
                let ty = self.ty()?;
                let trait_path = self.path()?;
                Ok(Path {
                    text: format!("<{ty} as {}>", trait_path.text),
                    owner: impl_path.owner,
                })
            }
            b'Y' => {
                let start = self.pos;
                let ty = self.ty()?;
                let trait_path = self.path()?;
                // The implementing type owns a trait default method's copy.
                let owner = {
                    let after = self.pos;
                    self.pos = start;
                    let owner = self.type_owner().unwrap_or_default();
                    self.pos = after;
                    owner
                };
                let owner = if owner.is_empty() { trait_path.owner } else { owner };
                Ok(Path {
                    text: format!("<{ty} as {}>", trait_path.text),
                    owner,
                })
            }
            b'N' => {
                let ns = self.next()?;
                let mut inner = self.path()?;
                let (name, dis) = self.ident()?;
                let segment = match ns {
                    b'C' => format!("{{closure#{dis}}}"),
                    b'S' => format!("{{shim:{name}#{dis}}}"),
                    b'A'..=b'Z' => format!("{{{}:{name}#{dis}}}", ns as char),
                    _ => name,
                };
                inner.text.push_str("::");
                inner.text.push_str(&segment);
                inner.owner.push(segment);
                Ok(inner)
            }
            b'I' => {
                let mut inner = self.path()?;
                let mut args = Vec::new();
                while !self.eat(b'E') {
                    if let Some(arg) = self.generic_arg()? {
                        args.push(arg);
                    }
                }
                if !args.is_empty() {
                    inner.text.push_str(&format!("::<{}>", args.join(", ")));
                }
                Ok(inner)
            }
            b'B' => self.backref(|p| p.path()),
            _ => Err(()),
        }
    }

    /// The owner path of the type at the cursor, when it is a path type.
    fn type_owner(&mut self) -> R<Vec<String>> {
        match self.peek() {
            Some(b'C' | b'M' | b'X' | b'Y' | b'N' | b'I') => Ok(self.path()?.owner),
            Some(b'B') => {
                self.pos += 1;
                self.backref(|p| p.type_owner())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn generic_arg(&mut self) -> R<Option<String>> {
        if self.eat(b'L') {
            self.base62()?;
            return Ok(None);
        }
        if self.eat(b'K') {
            return Ok(Some(self.konst()?));
        }
        Ok(Some(self.ty()?))
    }

    fn konst(&mut self) -> R<String> {
        if self.eat(b'p') {
            return Ok("_".into());
        }
        if self.eat(b'B') {
            return self.backref(|p| p.konst());
        }
        let ty = self.next()?;
        let negative = self.eat(b'n');
        let start = self.pos;
        while self.peek().map_or(false, |c| c.is_ascii_hexdigit()) {
            self.pos += 1;
        }
        let hex = std::str::from_utf8(&self.s[start..self.pos]).map_err(|_| ())?.to_string();
        if !self.eat(b'_') {
            return Err(());
        }
        let value = u128::from_str_radix(if hex.is_empty() { "0" } else { &hex }, 16).ok();
        Ok(match (ty, value) {
            (b'b', Some(value)) => (value != 0).to_string(),
            (b'c', Some(value)) => char::from_u32(value as u32)
                .map_or(format!("0x{hex}"), |ch| format!("{ch:?}")),
            (_, Some(value)) => format!("{}{value}", if negative { "-" } else { "" }),
            _ => format!("0x{hex}"),
        })
    }

    fn ty(&mut self) -> R<String> {
        self.enter()?;
        let out = self.ty_inner();
        self.depth -= 1;
        out
    }

    fn ty_inner(&mut self) -> R<String> {
        let c = self.peek().ok_or(())?;
        let basic = match c {
            b'a' => Some("i8"),
            b'b' => Some("bool"),
            b'c' => Some("char"),
            b'd' => Some("f64"),
            b'e' => Some("str"),
            b'f' => Some("f32"),
            b'h' => Some("u8"),
            b'i' => Some("isize"),
            b'j' => Some("usize"),
            b'l' => Some("i32"),
            b'm' => Some("u32"),
            b'n' => Some("i128"),
            b'o' => Some("u128"),
            b's' => Some("i16"),
            b't' => Some("u16"),
            b'u' => Some("()"),
            b'v' => Some("..."),
            b'x' => Some("i64"),
            b'y' => Some("u64"),
            b'z' => Some("!"),
            b'p' => Some("_"),
            _ => None,
        };
        if let Some(basic) = basic {
            self.pos += 1;
            return Ok(basic.to_string());
        }
        self.pos += 1;
        Ok(match c {
            b'A' => {
                let ty = self.ty()?;
                format!("[{ty}; {}]", self.konst()?)
            }
            b'S' => format!("[{}]", self.ty()?),
            b'T' => {
                let mut items = Vec::new();
                while !self.eat(b'E') {
                    items.push(self.ty()?);
                }
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            b'R' | b'Q' => {
                if self.eat(b'L') {
                    self.base62()?;
                }
                let ty = self.ty()?;
                if c == b'R' {
                    format!("&{ty}")
                } else {
                    format!("&mut {ty}")
                }
            }
            b'P' => format!("*const {}", self.ty()?),
            b'O' => format!("*mut {}", self.ty()?),
            b'F' => {
                if self.eat(b'G') {
                    self.base62()?;
                }
                let is_unsafe = self.eat(b'U');
                let mut abi = String::new();
                if self.eat(b'K') {
                    abi = if self.eat(b'C') {
                        "C".into()
                    } else {
                        self.undisambiguated_ident()?.replace('_', "-")
                    };
                }
                let mut params = Vec::new();
                while !self.eat(b'E') {
                    params.push(self.ty()?);
                }
                let ret = self.ty()?;
                let mut out = String::new();
                if is_unsafe {
                    out.push_str("unsafe ");
                }
                if !abi.is_empty() {
                    out.push_str(&format!("extern \"{abi}\" "));
                }
                out.push_str(&format!("fn({})", params.join(", ")));
                if ret != "()" {
                    out.push_str(&format!(" -> {ret}"));
                }
                out
            }
            b'D' => {
                if self.eat(b'G') {
                    self.base62()?;
                }
                let mut traits = Vec::new();
                while !self.eat(b'E') {
                    let mut text = self.path()?.text;
                    let mut bindings = Vec::new();
                    while self.eat(b'p') {
                        let name = self.undisambiguated_ident()?;
                        bindings.push(format!("{name} = {}", self.ty()?));
                    }
                    if !bindings.is_empty() {
                        text.push_str(&format!("<{}>", bindings.join(", ")));
                    }
                    traits.push(text);
                }
                if !self.eat(b'L') {
                    return Err(());
                }
                self.base62()?;
                format!("dyn {}", traits.join(" + "))
            }
            b'B' => self.backref(|p| p.ty())?,
            b'C' | b'M' | b'X' | b'Y' | b'N' | b'I' => {
                self.pos -= 1;
                self.path()?.text
            }
            _ => return Err(()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_paths() {
        let d = demangle("_RNvNvNvMNtCs9PjAQrQkMR9_4core3f32f5clamp8do_panic7runtime").unwrap();
        assert_eq!(d.name, "<f32>::clamp::do_panic::runtime");
        assert_eq!(d.owner, ["core", "f32", "clamp", "do_panic", "runtime"]);

        let d = demangle(
            "_RNvXsJ_NtCs9vY86ZUnYQa_20makepad_widgets_core6dialogNtB5_6DialogNtNtB7_6widget10WidgetNode7visible",
        )
        .unwrap();
        assert_eq!(
            d.name,
            "<makepad_widgets_core::dialog::Dialog as makepad_widgets_core::widget::WidgetNode>::visible"
        );
        assert_eq!(d.owner, ["makepad_widgets_core", "dialog", "visible"]);

        let d = demangle(
            "_RINvNtCs9PjAQrQkMR9_4core3ptr9drop_glueNtNtNtCskx1QxFKzdJi_12makepad_draw4text6loader14FontDefinitionEBH_",
        )
        .unwrap();
        assert_eq!(
            d.name,
            "core::ptr::drop_glue::<makepad_draw::text::loader::FontDefinition>"
        );
        assert_eq!(d.owner[0], "core");

        // A trait default method belongs to the implementing type's crate.
        let d = demangle(
            "_RNvYNtNtCs9vY86ZUnYQa_20makepad_widgets_core9drop_down8DropDownNtNtB6_6widget6Widget4drawB6_",
        )
        .unwrap();
        assert_eq!(
            d.name,
            "<makepad_widgets_core::drop_down::DropDown as makepad_widgets_core::widget::Widget>::draw"
        );
        assert_eq!(d.owner[..2], ["makepad_widgets_core", "drop_down"]);

        let d = demangle("_RNCNvNtCs9vY86ZUnYQa_20makepad_widgets_core12tween_script10script_mods0_0B5_")
            .unwrap();
        assert_eq!(
            d.name,
            "makepad_widgets_core::tween_script::script_mod::{closure#2}"
        );
    }

    #[test]
    fn v0_types() {
        let d = demangle("_RINvMNtCs9PjAQrQkMR9_4core3stre10split_onceReECsej9q53AWmf6_14makepad_script")
            .unwrap();
        assert_eq!(d.name, "<str>::split_once::<&str>");
        assert_eq!(d.owner, ["core", "str", "split_once"]);
        assert!(demangle("_RNvC").is_none());
        assert!(demangle("_RB_").is_none());
    }

    #[test]
    fn legacy_paths() {
        let d = demangle("_ZN4core3fmt9Formatter3pad17h0123456789abcdefE").unwrap();
        assert_eq!(d.name, "core::fmt::Formatter::pad");
        let d = demangle("_ZN45_$LT$alloc..string..String$u20$as$u20$Foo$GT$3bar17h0123456789abcdefE")
            .unwrap();
        assert_eq!(d.name, "<alloc::string::String as Foo>::bar");
    }
}
