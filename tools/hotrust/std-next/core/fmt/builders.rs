//! debug_struct / debug_tuple / debug_list / debug_set / debug_map, with `{:#?}` pretty
//! printing through a PadAdapter, as in real core.

use super::{Debug, Formatter, Result, Write};

struct PadState {
    on_newline: bool,
}

/// Indents everything written through it by four spaces after each newline.
struct PadAdapter<'a, 'b> {
    buf: &'a mut (dyn Write + 'b),
    state: &'a mut PadState,
}

impl<'a, 'b> Write for PadAdapter<'a, 'b> {
    fn write_str(&mut self, s: &str) -> Result {
        let mut rest = s;
        while !rest.is_empty() {
            let line = match rest.find('\n') {
                Some(i) => &rest[..i + 1],
                None => rest,
            };
            if self.state.on_newline {
                self.buf.write_str("    ")?;
            }
            self.state.on_newline = line.ends_with('\n');
            self.buf.write_str(line)?;
            rest = &rest[line.len()..];
        }
        Ok(())
    }
    fn write_char(&mut self, c: char) -> Result {
        if self.state.on_newline {
            self.buf.write_str("    ")?;
        }
        self.state.on_newline = c == '\n';
        self.buf.write_char(c)
    }
}

/// Runs `f` with a formatter that writes through a PadAdapter over `fmt`'s sink.
fn padded(fmt: &mut Formatter<'_>, state: &mut PadState, f: &mut dyn FnMut(&mut Formatter<'_>) -> Result) -> Result {
    let flags = fmt.flags;
    let fill = fmt.fill;
    let align = fmt.align;
    let width = fmt.width;
    let precision = fmt.precision;
    let mut pad = PadAdapter { buf: &mut *fmt.buf, state };
    let mut sub = Formatter { flags, fill, align, width, precision, buf: &mut pad };
    f(&mut sub)
}

// ---------------------------------------------------------------- DebugStruct

pub struct DebugStruct<'a, 'b: 'a> {
    fmt: &'a mut Formatter<'b>,
    result: Result,
    has_fields: bool,
}

pub(super) fn debug_struct_new<'a, 'b>(fmt: &'a mut Formatter<'b>, name: &str) -> DebugStruct<'a, 'b> {
    let result = fmt.write_str(name);
    DebugStruct { fmt, result, has_fields: false }
}

impl<'a, 'b: 'a> DebugStruct<'a, 'b> {
    pub fn field(&mut self, name: &str, value: &dyn Debug) -> &mut DebugStruct<'a, 'b> {
        if self.result.is_ok() {
            self.result = if self.fmt.alternate() {
                self.field_pretty(name, value)
            } else {
                let prefix = if self.has_fields { ", " } else { " { " };
                self.field_plain(prefix, name, value)
            };
        }
        self.has_fields = true;
        self
    }

    fn field_plain(&mut self, prefix: &str, name: &str, value: &dyn Debug) -> Result {
        self.fmt.write_str(prefix)?;
        self.fmt.write_str(name)?;
        self.fmt.write_str(": ")?;
        value.fmt(self.fmt)
    }

    fn field_pretty(&mut self, name: &str, value: &dyn Debug) -> Result {
        if !self.has_fields {
            self.fmt.write_str(" {\n")?;
        }
        let mut state = PadState { on_newline: true };
        padded(self.fmt, &mut state, &mut |w| {
            w.write_str(name)?;
            w.write_str(": ")?;
            value.fmt(w)?;
            w.write_str(",\n")
        })
    }

    pub fn finish_non_exhaustive(&mut self) -> Result {
        if self.result.is_ok() {
            self.result = if self.has_fields {
                if self.fmt.alternate() {
                    let mut state = PadState { on_newline: true };
                    match padded(self.fmt, &mut state, &mut |w| w.write_str("..\n")) {
                        Ok(()) => self.fmt.write_str("}"),
                        Err(e) => Err(e),
                    }
                } else {
                    self.fmt.write_str(", .. }")
                }
            } else {
                self.fmt.write_str(" { .. }")
            };
        }
        self.result
    }

    pub fn finish(&mut self) -> Result {
        if self.has_fields && self.result.is_ok() {
            self.result = if self.fmt.alternate() { self.fmt.write_str("}") } else { self.fmt.write_str(" }") };
        }
        self.result
    }
}

// ---------------------------------------------------------------- DebugTuple

pub struct DebugTuple<'a, 'b: 'a> {
    fmt: &'a mut Formatter<'b>,
    result: Result,
    fields: usize,
    empty_name: bool,
}

pub(super) fn debug_tuple_new<'a, 'b>(fmt: &'a mut Formatter<'b>, name: &str) -> DebugTuple<'a, 'b> {
    let result = fmt.write_str(name);
    DebugTuple { fmt, result, fields: 0, empty_name: name.is_empty() }
}

impl<'a, 'b: 'a> DebugTuple<'a, 'b> {
    pub fn field(&mut self, value: &dyn Debug) -> &mut DebugTuple<'a, 'b> {
        if self.result.is_ok() {
            self.result = if self.fmt.alternate() { self.field_pretty(value) } else { self.field_plain(value) };
        }
        self.fields += 1;
        self
    }

    fn field_plain(&mut self, value: &dyn Debug) -> Result {
        let prefix = if self.fields == 0 { "(" } else { ", " };
        self.fmt.write_str(prefix)?;
        value.fmt(self.fmt)
    }

    fn field_pretty(&mut self, value: &dyn Debug) -> Result {
        if self.fields == 0 {
            self.fmt.write_str("(\n")?;
        }
        let mut state = PadState { on_newline: true };
        padded(self.fmt, &mut state, &mut |w| {
            value.fmt(w)?;
            w.write_str(",\n")
        })
    }

    pub fn finish_non_exhaustive(&mut self) -> Result {
        if self.result.is_ok() {
            self.result = if self.fields > 0 {
                if self.fmt.alternate() {
                    let mut state = PadState { on_newline: true };
                    match padded(self.fmt, &mut state, &mut |w| w.write_str("..\n")) {
                        Ok(()) => self.fmt.write_str(")"),
                        Err(e) => Err(e),
                    }
                } else {
                    self.fmt.write_str(", ..)")
                }
            } else {
                self.fmt.write_str("(..)")
            };
        }
        self.result
    }

    pub fn finish(&mut self) -> Result {
        if self.fields > 0 && self.result.is_ok() {
            if self.fields == 1 && self.empty_name && !self.fmt.alternate() {
                self.result = self.fmt.write_str(",");
                if self.result.is_err() {
                    return self.result;
                }
            }
            self.result = self.fmt.write_str(")");
        }
        self.result
    }
}

// ---------------------------------------------------------------- list / set

struct DebugInner<'a, 'b: 'a> {
    fmt: &'a mut Formatter<'b>,
    result: Result,
    has_fields: bool,
}

impl<'a, 'b: 'a> DebugInner<'a, 'b> {
    fn entry(&mut self, entry: &dyn Debug) {
        if self.result.is_ok() {
            self.result = if self.fmt.alternate() { self.entry_pretty(entry) } else { self.entry_plain(entry) };
        }
        self.has_fields = true;
    }

    fn entry_plain(&mut self, entry: &dyn Debug) -> Result {
        if self.has_fields {
            self.fmt.write_str(", ")?;
        }
        entry.fmt(self.fmt)
    }

    fn entry_pretty(&mut self, entry: &dyn Debug) -> Result {
        if !self.has_fields {
            self.fmt.write_str("\n")?;
        }
        let mut state = PadState { on_newline: true };
        padded(self.fmt, &mut state, &mut |w| {
            entry.fmt(w)?;
            w.write_str(",\n")
        })
    }

    fn non_exhaustive(&mut self) -> Result {
        if self.has_fields {
            if self.fmt.alternate() {
                let mut state = PadState { on_newline: true };
                padded(self.fmt, &mut state, &mut |w| w.write_str("..\n"))
            } else {
                self.fmt.write_str(", ..")
            }
        } else {
            self.fmt.write_str("..")
        }
    }
}

pub struct DebugSet<'a, 'b: 'a> {
    inner: DebugInner<'a, 'b>,
}

pub(super) fn debug_set_new<'a, 'b>(fmt: &'a mut Formatter<'b>) -> DebugSet<'a, 'b> {
    let result = fmt.write_str("{");
    DebugSet { inner: DebugInner { fmt, result, has_fields: false } }
}

impl<'a, 'b: 'a> DebugSet<'a, 'b> {
    pub fn entry(&mut self, entry: &dyn Debug) -> &mut DebugSet<'a, 'b> {
        self.inner.entry(entry);
        self
    }
    pub fn entries<D: Debug, I: IntoIterator<Item = D>>(&mut self, entries: I) -> &mut DebugSet<'a, 'b> {
        for entry in entries {
            self.inner.entry(&entry);
        }
        self
    }
    pub fn finish_non_exhaustive(&mut self) -> Result {
        if self.inner.result.is_ok() {
            self.inner.result = self.inner.non_exhaustive();
        }
        self.finish()
    }
    pub fn finish(&mut self) -> Result {
        if self.inner.result.is_ok() {
            self.inner.result = self.inner.fmt.write_str("}");
        }
        self.inner.result
    }
}

pub struct DebugList<'a, 'b: 'a> {
    inner: DebugInner<'a, 'b>,
}

pub(super) fn debug_list_new<'a, 'b>(fmt: &'a mut Formatter<'b>) -> DebugList<'a, 'b> {
    let result = fmt.write_str("[");
    DebugList { inner: DebugInner { fmt, result, has_fields: false } }
}

impl<'a, 'b: 'a> DebugList<'a, 'b> {
    pub fn entry(&mut self, entry: &dyn Debug) -> &mut DebugList<'a, 'b> {
        self.inner.entry(entry);
        self
    }
    pub fn entries<D: Debug, I: IntoIterator<Item = D>>(&mut self, entries: I) -> &mut DebugList<'a, 'b> {
        for entry in entries {
            self.inner.entry(&entry);
        }
        self
    }
    pub fn finish_non_exhaustive(&mut self) -> Result {
        if self.inner.result.is_ok() {
            self.inner.result = self.inner.non_exhaustive();
        }
        self.finish()
    }
    pub fn finish(&mut self) -> Result {
        if self.inner.result.is_ok() {
            self.inner.result = self.inner.fmt.write_str("]");
        }
        self.inner.result
    }
}

// ---------------------------------------------------------------- map

pub struct DebugMap<'a, 'b: 'a> {
    fmt: &'a mut Formatter<'b>,
    result: Result,
    has_fields: bool,
    has_key: bool,
    /// pad state shared by a pretty key and its value
    state: PadState,
}

pub(super) fn debug_map_new<'a, 'b>(fmt: &'a mut Formatter<'b>) -> DebugMap<'a, 'b> {
    let result = fmt.write_str("{");
    DebugMap { fmt, result, has_fields: false, has_key: false, state: PadState { on_newline: true } }
}

impl<'a, 'b: 'a> DebugMap<'a, 'b> {
    pub fn entry(&mut self, key: &dyn Debug, value: &dyn Debug) -> &mut DebugMap<'a, 'b> {
        self.key(key);
        self.value(value);
        self
    }

    pub fn key(&mut self, key: &dyn Debug) -> &mut DebugMap<'a, 'b> {
        if self.result.is_ok() {
            if self.has_key {
                crate::panicking::panic_str("attempted to begin a new map entry without completing the previous one");
            }
            self.result = if self.fmt.alternate() { self.key_pretty(key) } else { self.key_plain(key) };
            if self.result.is_ok() {
                self.has_key = true;
            }
        }
        self
    }

    fn key_plain(&mut self, key: &dyn Debug) -> Result {
        if self.has_fields {
            self.fmt.write_str(", ")?;
        }
        key.fmt(self.fmt)?;
        self.fmt.write_str(": ")
    }

    fn key_pretty(&mut self, key: &dyn Debug) -> Result {
        if !self.has_fields {
            self.fmt.write_str("\n")?;
        }
        self.state = PadState { on_newline: true };
        padded(self.fmt, &mut self.state, &mut |w| {
            key.fmt(w)?;
            w.write_str(": ")
        })
    }

    pub fn value(&mut self, value: &dyn Debug) -> &mut DebugMap<'a, 'b> {
        if self.result.is_ok() {
            if !self.has_key {
                crate::panicking::panic_str("attempted to format a map value before its key");
            }
            self.result = if self.fmt.alternate() {
                padded(self.fmt, &mut self.state, &mut |w| {
                    value.fmt(w)?;
                    w.write_str(",\n")
                })
            } else {
                value.fmt(self.fmt)
            };
            self.has_key = false;
        }
        self.has_fields = true;
        self
    }

    pub fn entries<K: Debug, V: Debug, I: IntoIterator<Item = (K, V)>>(&mut self, entries: I) -> &mut DebugMap<'a, 'b> {
        for (k, v) in entries {
            self.entry(&k, &v);
        }
        self
    }

    pub fn finish_non_exhaustive(&mut self) -> Result {
        if self.result.is_ok() {
            self.result = if self.has_fields {
                if self.fmt.alternate() {
                    let mut state = PadState { on_newline: true };
                    match padded(self.fmt, &mut state, &mut |w| w.write_str("..\n")) {
                        Ok(()) => self.fmt.write_str("}"),
                        Err(e) => Err(e),
                    }
                } else {
                    self.fmt.write_str(", ..}")
                }
            } else {
                self.fmt.write_str("..}")
            };
        }
        self.result
    }

    pub fn finish(&mut self) -> Result {
        if self.result.is_ok() {
            if self.has_key {
                crate::panicking::panic_str("attempted to finish a map with a partial entry");
            }
            self.result = self.fmt.write_str("}");
        }
        self.result
    }
}
