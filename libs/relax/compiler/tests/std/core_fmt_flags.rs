// Formatter::flags (deprecated since 1.24, still there): the sign, alternate, zero-pad and
// debug-hex flags as one word. enumflags2 0.7 reads `fmt.flags() >> 4` to tell `{:x?}` and
// `{:X?}` from `{:?}` (src/formatting.rs: there is no stable accessor for the debug-hex flags),
// and the std here had no such method: "no method named `flags` found for `&mut Formatter`".
#![allow(deprecated)]
use std::fmt;

struct Flags;
impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let flags = f.flags();
        write!(f, "{}", flags)
    }
}
impl fmt::Display for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let flags = f.flags();
        write!(f, "{}", flags)
    }
}

// enumflags2's shape: binary unless a hex debug format was asked for
struct Bits(u8);
impl fmt::Debug for Bits {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format_hex = fmt.flags() >> 4;
        let width = fmt.width().unwrap_or(0);
        if format_hex & 1 != 0 {
            write!(fmt, "{:#0width$x?}", &self.0, width = width)
        } else if format_hex & 2 != 0 {
            write!(fmt, "{:#0width$X?}", &self.0, width = width)
        } else {
            write!(fmt, "{:#0width$b}", &self.0, width = width)
        }
    }
}

#[test]
fn the_word_rustc_std_returns() {
    assert_eq!(format!("{}", Flags), "0");
    assert_eq!(format!("{:+}", Flags), "1");
    assert_eq!(format!("{:-}", Flags), "2");
    assert_eq!(format!("{:#}", Flags), "4");
    assert_eq!(format!("{:04}", Flags), "8");
    assert_eq!(format!("{:?}", Flags), "0");
    assert_eq!(format!("{:x?}", Flags), "16");
    assert_eq!(format!("{:X?}", Flags), "32");
    assert_eq!(format!("{:+#06x?}", Flags), "29");
    // width, precision, fill and alignment are not flags
    assert_eq!(format!("{:*>8.3}", Flags), "0");
}

#[test]
fn debug_hex_is_told_from_plain_debug() {
    assert_eq!(format!("{:?}", Bits(0b1010)), "0b1010");
    assert_eq!(format!("{:x?}", Bits(0xab)), "0xab");
    assert_eq!(format!("{:X?}", Bits(0xab)), "0xAB");
    assert_eq!(format!("{:06x?}", Bits(0xab)), "0x00ab");
    assert_eq!(format!("{:010?}", Bits(0b101)), "0b00000101");
}
