//! Formatting machinery, shaped like real core's so output matches it byte for byte.
//!
//! Rapid lowers the format macros (coord.md S2): `format_args!("a{}b{:>5}", x, y)` becomes
//! `Arguments::new_v1_formatted(&["a", "b"], &[rt::Argument::new_display(&x),
//! rt::Argument::new_display(&y)], &[placeholder0, placeholder1])` (or `new_v1` when every
//! placeholder is the default `{}`/`{:?}` in argument order, `new_const` when there are no
//! arguments). `format!` = `crate::fmt::format(args)`, `write!(w, ..)` = `w.write_fmt(args)`,
//! `print!` = std::io::_print(args), `panic!` = core::panicking::panic_fmt(args).

mod builders;
mod float;
mod num;

pub use builders::{DebugList, DebugMap, DebugSet, DebugStruct, DebugTuple};

use crate::cell::{Cell, Ref, RefCell, RefMut, UnsafeCell};
use crate::marker::PhantomData;

pub type Result = crate::result::Result<(), Error>;

/// The error type returned by formatting traits (carries no information).
#[derive(Copy, Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Error;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Display::fmt("an error occurred when formatting an argument", f)
    }
}

/// A sink for formatted text.
pub trait Write {
    fn write_str(&mut self, s: &str) -> Result;

    fn write_char(&mut self, c: char) -> Result {
        let mut buf = [0u8; 4];
        self.write_str(c.encode_utf8(&mut buf))
    }

    fn write_fmt(mut self: &mut Self, args: Arguments<'_>) -> Result {
        // `&mut &mut Self` is Sized and Write (impl below), so it coerces to `&mut dyn Write`
        write(&mut self, args)
    }
}

impl<'a, W: Write + ?Sized> Write for &'a mut W {
    fn write_str(&mut self, s: &str) -> Result {
        (**self).write_str(s)
    }
    fn write_char(&mut self, c: char) -> Result {
        (**self).write_char(c)
    }
    fn write_fmt(&mut self, args: Arguments<'_>) -> Result {
        (**self).write_fmt(args)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Right,
    Center,
}

/// Runtime representation of format strings, built by Rapid's lowering.
pub mod rt {
    use super::{Formatter, Result};

    pub const ALIGN_LEFT: u8 = 0;
    pub const ALIGN_RIGHT: u8 = 1;
    pub const ALIGN_CENTER: u8 = 2;
    pub const ALIGN_UNKNOWN: u8 = 3;

    pub const FLAG_PLUS: u32 = 1;
    pub const FLAG_MINUS: u32 = 2;
    pub const FLAG_ALTERNATE: u32 = 4;
    pub const FLAG_ZERO_PAD: u32 = 8;
    pub const FLAG_DEBUG_LOWER_HEX: u32 = 16;
    pub const FLAG_DEBUG_UPPER_HEX: u32 = 32;

    #[derive(Copy, Clone)]
    pub enum Count {
        /// `{:5}`
        Is(usize),
        /// `{:w$}` / `{:.*}`: the usize value of argument n
        Param(usize),
        Implied,
    }

    /// One `{...}` with a non-default spec. `position` indexes `Arguments::args`.
    #[derive(Copy, Clone)]
    pub struct Placeholder {
        pub position: usize,
        pub fill: char,
        pub align: u8,
        pub flags: u32,
        pub precision: Count,
        pub width: Count,
    }

    impl Placeholder {
        pub const fn new(position: usize, fill: char, align: u8, flags: u32, precision: Count, width: Count) -> Placeholder {
            Placeholder { position, fill, align, flags, precision, width }
        }
    }

    /// A type-erased argument: a pointer to the value and its formatting function.
    #[derive(Copy, Clone)]
    pub struct Argument<'a> {
        value: *const u8,
        formatter: fn(*const u8, &mut Formatter<'_>) -> Result,
        _m: crate::marker::PhantomData<&'a ()>,
    }

    impl<'a> Argument<'a> {
        /// `f` must be the formatting fn of `T`; the value is passed back to it as `&T`.
        pub fn new<T>(x: &'a T, f: fn(&T, &mut Formatter<'_>) -> Result) -> Argument<'a> {
            Argument {
                value: x as *const T as *const u8,
                formatter: unsafe {
                    crate::intrinsics_mem::transmute::<fn(&T, &mut Formatter<'_>) -> Result, fn(*const u8, &mut Formatter<'_>) -> Result>(f)
                },
                _m: crate::marker::PhantomData,
            }
        }
        pub fn new_display<T: super::Display>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::Display>::fmt)
        }
        pub fn new_debug<T: super::Debug>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::Debug>::fmt)
        }
        pub fn new_octal<T: super::Octal>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::Octal>::fmt)
        }
        pub fn new_lower_hex<T: super::LowerHex>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::LowerHex>::fmt)
        }
        pub fn new_upper_hex<T: super::UpperHex>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::UpperHex>::fmt)
        }
        pub fn new_pointer<T: super::Pointer>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::Pointer>::fmt)
        }
        pub fn new_binary<T: super::Binary>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::Binary>::fmt)
        }
        pub fn new_lower_exp<T: super::LowerExp>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::LowerExp>::fmt)
        }
        pub fn new_upper_exp<T: super::UpperExp>(x: &'a T) -> Argument<'a> {
            Argument::new(x, <T as super::UpperExp>::fmt)
        }
        /// A width/precision parameter (`{:w$}`): must be a usize.
        pub fn from_usize(x: &'a usize) -> Argument<'a> {
            Argument::new(x, <usize as super::Display>::fmt)
        }
        pub(crate) fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            (self.formatter)(self.value, f)
        }
        pub(crate) fn as_usize(&self) -> usize {
            unsafe { *(self.value as *const usize) }
        }
    }
}

/// A precompiled format string and its arguments.
#[derive(Copy, Clone)]
pub struct Arguments<'a> {
    pieces: &'a [&'static str],
    /// None = every argument formatted with the default spec, in order
    fmt: Option<&'a [rt::Placeholder]>,
    args: &'a [rt::Argument<'a>],
}

impl<'a> Arguments<'a> {
    pub const fn new_const(pieces: &'a [&'static str]) -> Arguments<'a> {
        Arguments { pieces, fmt: None, args: &[] }
    }
    pub fn new_v1(pieces: &'a [&'static str], args: &'a [rt::Argument<'a>]) -> Arguments<'a> {
        Arguments { pieces, fmt: None, args }
    }
    pub fn new_v1_formatted(
        pieces: &'a [&'static str],
        args: &'a [rt::Argument<'a>],
        placeholders: &'a [rt::Placeholder],
    ) -> Arguments<'a> {
        Arguments { pieces, fmt: Some(placeholders), args }
    }
    /// The text if there are no arguments.
    pub fn as_str(&self) -> Option<&'static str> {
        match (self.pieces, self.args) {
            ([], []) => Some(""),
            ([s], []) => Some(s),
            _ => None,
        }
    }
    pub(crate) fn estimated_capacity(&self) -> usize {
        let mut pieces_length = 0usize;
        for p in self.pieces {
            pieces_length += p.len();
        }
        if self.args.is_empty() {
            pieces_length
        } else if !self.pieces.is_empty() && self.pieces[0].is_empty() && pieces_length < 16 {
            0
        } else {
            pieces_length.wrapping_mul(2)
        }
    }
}

impl<'a> Debug for Arguments<'a> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result {
        Display::fmt(self, fmt)
    }
}

impl<'a> Display for Arguments<'a> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result {
        write(fmt.buf, *self)
    }
}

/// Writes formatted arguments into `output`.
pub fn write(output: &mut dyn Write, args: Arguments<'_>) -> Result {
    let mut fmt = Formatter::new(output);
    let mut idx = 0;
    match args.fmt {
        None => {
            let mut i = 0;
            while i < args.args.len() {
                let piece = if i < args.pieces.len() { args.pieces[i] } else { "" };
                if !piece.is_empty() {
                    fmt.buf.write_str(piece)?;
                }
                args.args[i].fmt(&mut fmt)?;
                idx += 1;
                i += 1;
            }
        }
        Some(specs) => {
            let mut i = 0;
            while i < specs.len() {
                let piece = if i < args.pieces.len() { args.pieces[i] } else { "" };
                if !piece.is_empty() {
                    fmt.buf.write_str(piece)?;
                }
                run(&mut fmt, &specs[i], args.args)?;
                idx += 1;
                i += 1;
            }
        }
    }
    if idx < args.pieces.len() {
        fmt.buf.write_str(args.pieces[idx])?;
    }
    Ok(())
}

fn run(fmt: &mut Formatter<'_>, arg: &rt::Placeholder, args: &[rt::Argument<'_>]) -> Result {
    fmt.fill = arg.fill;
    fmt.align = arg.align;
    fmt.flags = arg.flags;
    fmt.width = getcount(args, &arg.width);
    fmt.precision = getcount(args, &arg.precision);
    let value = &args[arg.position];
    let r = value.fmt(fmt);
    fmt.fill = ' ';
    fmt.align = rt::ALIGN_UNKNOWN;
    fmt.flags = 0;
    fmt.width = None;
    fmt.precision = None;
    r
}

fn getcount(args: &[rt::Argument<'_>], cnt: &rt::Count) -> Option<usize> {
    match *cnt {
        rt::Count::Is(n) => Some(n),
        rt::Count::Implied => None,
        rt::Count::Param(i) => Some(args[i].as_usize()),
    }
}

/// `format!`: formats into a new String.
pub fn format(args: Arguments<'_>) -> crate::string::String {
    match args.as_str() {
        Some(s) => crate::string::String::from(s),
        None => {
            let mut output = crate::string::String::with_capacity(args.estimated_capacity());
            let _ = output.write_fmt(args);
            output
        }
    }
}

// ---------------------------------------------------------------- Formatter

/// Configuration for one formatting call plus the output sink.
pub struct Formatter<'a> {
    flags: u32,
    fill: char,
    align: u8,
    width: Option<usize>,
    precision: Option<usize>,
    buf: &'a mut (dyn Write + 'a),
}

/// Padding written after the value by `Formatter::padding`.
pub(crate) struct PostPadding {
    fill: char,
    padding: usize,
}

impl PostPadding {
    pub(crate) fn write(self, f: &mut Formatter<'_>) -> Result {
        let mut i = 0;
        while i < self.padding {
            f.buf.write_char(self.fill)?;
            i += 1;
        }
        Ok(())
    }
}

impl<'a> Formatter<'a> {
    pub fn new(buf: &'a mut (dyn Write + 'a)) -> Formatter<'a> {
        Formatter { flags: 0, fill: ' ', align: rt::ALIGN_UNKNOWN, width: None, precision: None, buf }
    }

    /// A formatter writing to `buf` with this formatter's options.
    pub(crate) fn wrap_buf<'b>(&'b mut self, buf: &'b mut (dyn Write + 'b)) -> Formatter<'b> {
        Formatter {
            flags: self.flags,
            fill: self.fill,
            align: self.align,
            width: self.width,
            precision: self.precision,
            buf,
        }
    }

    /// Writes `prefix`, `buf` (the digits) with sign, `#` prefix, width and zero padding,
    /// as integer Display/hex/... do.
    pub fn pad_integral(&mut self, is_nonnegative: bool, prefix: &str, buf: &str) -> Result {
        let mut width = buf.len();
        let mut sign: Option<char> = None;
        if !is_nonnegative {
            sign = Some('-');
            width += 1;
        } else if self.sign_plus() {
            sign = Some('+');
            width += 1;
        }
        let prefix = if self.alternate() {
            width += prefix.chars().count();
            Some(prefix)
        } else {
            None
        };

        fn write_prefix(f: &mut Formatter<'_>, sign: Option<char>, prefix: Option<&str>) -> Result {
            if let Some(c) = sign {
                f.buf.write_char(c)?;
            }
            if let Some(prefix) = prefix {
                f.buf.write_str(prefix)
            } else {
                Ok(())
            }
        }

        match self.width {
            None => {
                write_prefix(self, sign, prefix)?;
                self.buf.write_str(buf)
            }
            Some(min) if width >= min => {
                write_prefix(self, sign, prefix)?;
                self.buf.write_str(buf)
            }
            Some(min) if self.sign_aware_zero_pad() => {
                let old_fill = crate::mem::replace(&mut self.fill, '0');
                let old_align = crate::mem::replace(&mut self.align, rt::ALIGN_RIGHT);
                write_prefix(self, sign, prefix)?;
                let post = self.padding(min - width, Alignment::Right)?;
                self.buf.write_str(buf)?;
                post.write(self)?;
                self.fill = old_fill;
                self.align = old_align;
                Ok(())
            }
            Some(min) => {
                let post = self.padding(min - width, Alignment::Right)?;
                write_prefix(self, sign, prefix)?;
                self.buf.write_str(buf)?;
                post.write(self)
            }
        }
    }

    /// Writes `s` honouring width, fill, alignment and precision (max chars), as str
    /// Display does.
    pub fn pad(&mut self, s: &str) -> Result {
        if self.width.is_none() && self.precision.is_none() {
            return self.buf.write_str(s);
        }
        let s = if let Some(max) = self.precision {
            // truncate to `max` chars
            let mut iter = s.char_indices();
            match iter.nth(max) {
                Some((i, _)) => &s[..i],
                None => s,
            }
        } else {
            s
        };
        match self.width {
            None => self.buf.write_str(s),
            Some(width) => {
                let chars_count = s.chars().count();
                if chars_count >= width {
                    self.buf.write_str(s)
                } else {
                    let post = self.padding(width - chars_count, Alignment::Left)?;
                    self.buf.write_str(s)?;
                    post.write(self)
                }
            }
        }
    }

    /// Writes the pre-padding and returns the post-padding.
    pub(crate) fn padding(&mut self, padding: usize, default: Alignment) -> crate::result::Result<PostPadding, Error> {
        let align = match self.align {
            rt::ALIGN_LEFT => Alignment::Left,
            rt::ALIGN_RIGHT => Alignment::Right,
            rt::ALIGN_CENTER => Alignment::Center,
            _ => default,
        };
        let (pre_pad, post_pad) = match align {
            Alignment::Left => (0, padding),
            Alignment::Right => (padding, 0),
            Alignment::Center => (padding / 2, (padding + 1) / 2),
        };
        let mut i = 0;
        while i < pre_pad {
            self.buf.write_char(self.fill)?;
            i += 1;
        }
        Ok(PostPadding { fill: self.fill, padding: post_pad })
    }

    /// Writes already-formatted number parts (sign + body) with width/zero padding.
    /// Used by float formatting: `sign` is "", "-" or "+".
    pub(crate) fn pad_formatted_parts(&mut self, sign: &str, body: &str) -> Result {
        let width = match self.width {
            Some(w) => w,
            None => {
                self.buf.write_str(sign)?;
                return self.buf.write_str(body);
            }
        };
        let len = sign.len() + body.len();
        if len >= width {
            self.buf.write_str(sign)?;
            return self.buf.write_str(body);
        }
        if self.sign_aware_zero_pad() {
            self.buf.write_str(sign)?;
            let old_fill = crate::mem::replace(&mut self.fill, '0');
            let old_align = crate::mem::replace(&mut self.align, rt::ALIGN_RIGHT);
            let post = self.padding(width - len, Alignment::Right)?;
            self.buf.write_str(body)?;
            post.write(self)?;
            self.fill = old_fill;
            self.align = old_align;
            Ok(())
        } else {
            let post = self.padding(width - len, Alignment::Right)?;
            self.buf.write_str(sign)?;
            self.buf.write_str(body)?;
            post.write(self)
        }
    }

    pub fn write_str(&mut self, data: &str) -> Result {
        self.buf.write_str(data)
    }

    pub fn write_char(&mut self, c: char) -> Result {
        self.buf.write_char(c)
    }

    pub fn write_fmt(&mut self, fmt: Arguments<'_>) -> Result {
        if let Some(s) = fmt.as_str() {
            self.buf.write_str(s)
        } else {
            write(self.buf, fmt)
        }
    }

    pub fn fill(&self) -> char {
        self.fill
    }
    pub fn align(&self) -> Option<Alignment> {
        match self.align {
            rt::ALIGN_LEFT => Some(Alignment::Left),
            rt::ALIGN_RIGHT => Some(Alignment::Right),
            rt::ALIGN_CENTER => Some(Alignment::Center),
            _ => None,
        }
    }
    pub fn width(&self) -> Option<usize> {
        self.width
    }
    pub fn precision(&self) -> Option<usize> {
        self.precision
    }
    pub fn sign_plus(&self) -> bool {
        self.flags & rt::FLAG_PLUS != 0
    }
    pub fn sign_minus(&self) -> bool {
        self.flags & rt::FLAG_MINUS != 0
    }
    pub fn alternate(&self) -> bool {
        self.flags & rt::FLAG_ALTERNATE != 0
    }
    pub fn sign_aware_zero_pad(&self) -> bool {
        self.flags & rt::FLAG_ZERO_PAD != 0
    }
    pub(crate) fn debug_lower_hex(&self) -> bool {
        self.flags & rt::FLAG_DEBUG_LOWER_HEX != 0
    }
    pub(crate) fn debug_upper_hex(&self) -> bool {
        self.flags & rt::FLAG_DEBUG_UPPER_HEX != 0
    }

    pub fn debug_struct<'b>(&'b mut self, name: &str) -> DebugStruct<'b, 'a> {
        builders::debug_struct_new(self, name)
    }
    pub fn debug_tuple<'b>(&'b mut self, name: &str) -> DebugTuple<'b, 'a> {
        builders::debug_tuple_new(self, name)
    }
    pub fn debug_list<'b>(&'b mut self) -> DebugList<'b, 'a> {
        builders::debug_list_new(self)
    }
    pub fn debug_set<'b>(&'b mut self) -> DebugSet<'b, 'a> {
        builders::debug_set_new(self)
    }
    pub fn debug_map<'b>(&'b mut self) -> DebugMap<'b, 'a> {
        builders::debug_map_new(self)
    }

    /// derive(Debug) helpers (what rustc's derive expands to; Rapid's derive may use them).
    pub fn debug_struct_field1_finish(&mut self, name: &str, name1: &str, value1: &dyn Debug) -> Result {
        let mut b = builders::debug_struct_new(self, name);
        b.field(name1, value1);
        b.finish()
    }
    pub fn debug_struct_fields_finish(&mut self, name: &str, names: &[&str], values: &[&dyn Debug]) -> Result {
        let mut b = builders::debug_struct_new(self, name);
        let mut i = 0;
        while i < names.len() {
            b.field(names[i], values[i]);
            i += 1;
        }
        b.finish()
    }
    pub fn debug_tuple_field1_finish(&mut self, name: &str, value1: &dyn Debug) -> Result {
        let mut b = builders::debug_tuple_new(self, name);
        b.field(value1);
        b.finish()
    }
    pub fn debug_tuple_fields_finish(&mut self, name: &str, values: &[&dyn Debug]) -> Result {
        let mut b = builders::debug_tuple_new(self, name);
        let mut i = 0;
        while i < values.len() {
            b.field(values[i]);
            i += 1;
        }
        b.finish()
    }
}

impl<'a> Write for Formatter<'a> {
    fn write_str(&mut self, s: &str) -> Result {
        self.buf.write_str(s)
    }
    fn write_char(&mut self, c: char) -> Result {
        self.buf.write_char(c)
    }
    fn write_fmt(&mut self, args: Arguments<'_>) -> Result {
        if let Some(s) = args.as_str() {
            self.buf.write_str(s)
        } else {
            write(self.buf, args)
        }
    }
}

// ---------------------------------------------------------------- traits

pub trait Debug {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait Display {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait Octal {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait Binary {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait LowerHex {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait UpperHex {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait Pointer {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait LowerExp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}
pub trait UpperExp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result;
}

// References forward to the referent.
impl<'a, T: Debug + ?Sized> Debug for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Debug::fmt(&**self, f)
    }
}
impl<'a, T: Debug + ?Sized> Debug for &'a mut T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Debug::fmt(&**self, f)
    }
}
impl<'a, T: Display + ?Sized> Display for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Display::fmt(&**self, f)
    }
}
impl<'a, T: Display + ?Sized> Display for &'a mut T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Display::fmt(&**self, f)
    }
}
impl<'a, T: LowerHex + ?Sized> LowerHex for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        LowerHex::fmt(&**self, f)
    }
}
impl<'a, T: UpperHex + ?Sized> UpperHex for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        UpperHex::fmt(&**self, f)
    }
}
impl<'a, T: Octal + ?Sized> Octal for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Octal::fmt(&**self, f)
    }
}
impl<'a, T: Binary + ?Sized> Binary for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Binary::fmt(&**self, f)
    }
}
impl<'a, T: LowerExp + ?Sized> LowerExp for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        LowerExp::fmt(&**self, f)
    }
}
impl<'a, T: UpperExp + ?Sized> UpperExp for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        UpperExp::fmt(&**self, f)
    }
}

// ---------------------------------------------------------------- impls for core types

impl Debug for bool {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Display::fmt(self, f)
    }
}
impl Display for bool {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Display::fmt(if *self { "true" } else { "false" }, f)
    }
}

impl Debug for str {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_char('"')?;
        crate::str::write_escape_debug(self, f.buf)?;
        f.write_char('"')
    }
}
impl Display for str {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.pad(self)
    }
}

impl Debug for char {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_char('\'')?;
        crate::char::write_escape_debug_char(*self, f.buf)?;
        f.write_char('\'')
    }
}
impl Display for char {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        if f.width.is_none() && f.precision.is_none() {
            f.write_char(*self)
        } else {
            let mut buf = [0u8; 4];
            f.pad(self.encode_utf8(&mut buf))
        }
    }
}

impl Debug for () {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.pad("()")
    }
}

#[cfg(not(rapid_check))]
impl Debug for ! {
    fn fmt(&self, _f: &mut Formatter<'_>) -> Result {
        *self
    }
}
#[cfg(not(rapid_check))]
impl Display for ! {
    fn fmt(&self, _f: &mut Formatter<'_>) -> Result {
        *self
    }
}

impl<T: ?Sized> Debug for PhantomData<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str("PhantomData<")?;
        f.write_str(crate::any::type_name::<T>())?;
        f.write_str(">")
    }
}

impl<T: Debug> Debug for [T] {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: ?Sized> Pointer for *const T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        pointer_fmt_inner(*self as *const u8 as usize, f)
    }
}
impl<T: ?Sized> Pointer for *mut T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        pointer_fmt_inner(*self as *const u8 as usize, f)
    }
}
impl<'a, T: ?Sized> Pointer for &'a T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Pointer::fmt(&(*self as *const T), f)
    }
}
impl<'a, T: ?Sized> Pointer for &'a mut T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Pointer::fmt(&(&**self as *const T), f)
    }
}
impl<T: ?Sized> Debug for *const T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Pointer::fmt(self, f)
    }
}
impl<T: ?Sized> Debug for *mut T {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Pointer::fmt(self, f)
    }
}

/// `{:p}`: `0x` + lowercase hex; `{:#p}` zero-pads to the pointer width.
pub(crate) fn pointer_fmt_inner(addr: usize, f: &mut Formatter<'_>) -> Result {
    let old_width = f.width;
    let old_flags = f.flags;
    if f.alternate() {
        f.flags |= rt::FLAG_ZERO_PAD;
        if f.width.is_none() {
            f.width = Some(crate::mem::size_of::<usize>() * 2 + 2);
        }
    }
    f.flags |= rt::FLAG_ALTERNATE;
    let ret = LowerHex::fmt(&addr, f);
    f.width = old_width;
    f.flags = old_flags;
    ret
}

impl<T: Copy + Debug> Debug for Cell<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("Cell").field("value", &self.get()).finish()
    }
}

impl<T: ?Sized + Debug> Debug for RefCell<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let mut d = f.debug_struct("RefCell");
        match self.try_borrow() {
            Ok(borrow) => d.field("value", &borrow),
            Err(_) => d.field("value", &BorrowedPlaceholder),
        };
        d.finish()
    }
}

struct BorrowedPlaceholder;

impl Debug for BorrowedPlaceholder {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str("<borrowed>")
    }
}

impl<'b, T: ?Sized + Debug> Debug for Ref<'b, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Debug::fmt(&**self, f)
    }
}
impl<'b, T: ?Sized + Debug> Debug for RefMut<'b, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Debug::fmt(&**self, f)
    }
}
impl<T: ?Sized> Debug for UnsafeCell<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("UnsafeCell").finish_non_exhaustive()
    }
}
