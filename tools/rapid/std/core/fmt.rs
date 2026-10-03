//! Formatting. Rapid formats into a host-side stack of buffers: `format!`/`write!` to a
//! non-Formatter target push a buffer, format the pieces, pop it. A `Formatter` writes to
//! the current top buffer. Primitive values are formatted by the host; other types through
//! their `Display` / `Debug` impls (a structural Debug is built in for types without one).

pub struct Error;

pub type Result = crate::option::Result<(), Error>;

pub struct Formatter {
    /// packed spec (width, precision, flags, type) as Rapid's lowering builds it
    pub spec: u64,
}

pub mod intrinsics_fmt {
    extern "rapid-intrinsic" {
        /// appends to the current top format buffer
        pub fn fmt_write_str(s: &str);
        /// the text of the buffer popped last (valid until the next pop)
        pub fn fmt_pop_str<'a>() -> &'a str;
    }
}

impl Formatter {
    pub fn write_str(&mut self, s: &str) -> Result {
        unsafe { intrinsics_fmt::fmt_write_str(s) };
        crate::option::Result::Ok(())
    }
    pub fn alternate(&self) -> bool {
        (self.spec >> 36) & 1 != 0
    }
    pub fn width(&self) -> crate::option::Option<usize> {
        let w = (self.spec & 0xffff) as usize;
        if w == 0 {
            crate::option::Option::None
        } else {
            crate::option::Option::Some(w)
        }
    }
    pub fn precision(&self) -> crate::option::Option<usize> {
        if (self.spec >> 32) & 1 != 0 {
            crate::option::Option::Some(((self.spec >> 16) & 0xffff) as usize)
        } else {
            crate::option::Option::None
        }
    }
}

pub trait Display {
    fn fmt(&self, f: &mut Formatter) -> Result;
}

pub trait Debug {
    fn fmt(&self, f: &mut Formatter) -> Result;
}

pub trait LowerHex {
    fn fmt(&self, f: &mut Formatter) -> Result;
}

pub trait Write {
    fn write_str(&mut self, s: &str) -> Result;
}

impl Write for crate::string::String {
    fn write_str(&mut self, s: &str) -> Result {
        self.push_str(s);
        crate::option::Result::Ok(())
    }
}

impl Display for crate::string::String {
    fn fmt(&self, f: &mut Formatter) -> Result {
        f.write_str(self.as_str())
    }
}

impl Debug for crate::string::String {
    fn fmt(&self, f: &mut Formatter) -> Result {
        f.write_str("\"")?;
        f.write_str(self.as_str())?;
        f.write_str("\"")
    }
}

/// `format!`: the lowering pushes a buffer, formats, then calls this.
pub fn format_pop_string() -> crate::string::String {
    crate::string::String::from(unsafe { intrinsics_fmt::fmt_pop_str() })
}
