//! Reading the SMLib database formats (ASCII and binary `.smb`/`.sms`/`.smc`/`.smp`/
//! `.pbp` streams): byte swapping, binary value reads, object type headers and the
//! `std::istream` extraction rules the ASCII readers rely on.
//!
//! Upstream: `source/SMLib/src/SmDatabaseIO.cpp`, `source/SMLib/inc/SmDatabaseIO.h`.
//!
//! | Rust                                  | Upstream                                         |
//! |---------------------------------------|--------------------------------------------------|
//! | [`DatabaseReader`]                    | `SmDatabaseIO` / `SmDatabaseIOFile` / `SmDatabaseIOStream` (read side) |
//! | [`DatabaseReader::new`]               | `SmDatabaseIO()`, `SmDatabaseIOFile()`           |
//! | [`DatabaseReader::with_file_type`]    | `SmDatabaseIO(SmFileType)`                       |
//! | [`DatabaseReader::with_stream`]       | `SmDatabaseIO(SmFileType, std::istream*)`        |
//! | [`DatabaseReader::open_for_read`]     | `SmDatabaseIOFile::OpenFileForRead`              |
//! | [`DatabaseReader::file_type`]         | `SmDatabaseIO::GetFileType`                      |
//! | [`DatabaseReader::in_stream_mut`]     | `SmDatabaseIO::GetInStreamPtr`                   |
//! | [`DatabaseReader::version`] / [`DatabaseReader::set_version`] | `GetVersionNum` / `SetVersionNum` |
//! | [`DatabaseReader::begin_reading`] / [`DatabaseReader::end_reading`] | `BeginReading` / `EndReading` |
//! | [`DatabaseReader::read_long`]         | `SmDatabaseIO::ReadLong(long&)`                  |
//! | [`DatabaseReader::read_ulong`]        | `SmDatabaseIO::ReadLong(ULONG&)`                 |
//! | [`DatabaseReader::read_longs`]        | `SmDatabaseIO::ReadLongs`                        |
//! | [`DatabaseReader::read_double`]       | `SmDatabaseIO::ReadDouble`                       |
//! | [`DatabaseReader::read_doubles`]      | `SmDatabaseIO::ReadDoubles`                      |
//! | [`DatabaseReader::read_boolean`]      | `SmDatabaseIO::ReadBoolean`                      |
//! | [`DatabaseReader::read_booleans`]     | `SmDatabaseIO::ReadBooleans`                     |
//! | [`DatabaseReader::read_short`]        | `SmDatabaseIO::ReadShort`                        |
//! | [`DatabaseReader::read_char`]         | `SmDatabaseIO::ReadChar`                         |
//! | [`DatabaseReader::read_characters`]   | `SmDatabaseIO::ReadCharacters`                   |
//! | [`DatabaseReader::read_type`]         | `SmDatabaseIO::ReadType`                         |
//! | [`DatabaseReader::go_to_next_line`]   | `SmDatabaseIO::GoToNextLine`                     |
//! | [`DatabaseReader::read_failed`]       | `SmDatabaseIO::ReadFailed`                       |
//! | [`DatabaseWriter`]                    | `SmDatabaseIO` / `SmDatabaseIOFile` (write side) |
//! | [`DatabaseWriter::open_for_write`]    | `SmDatabaseIOFile::OpenFileForWrite` (new file)  |
//! | [`DatabaseWriter::write_long`] / [`DatabaseWriter::write_longs`] | `WriteLong` / `WriteLongs` |
//! | [`DatabaseWriter::write_double`] / [`DatabaseWriter::write_doubles`] | `WriteDouble` / `WriteDoubles` |
//! | [`DatabaseWriter::write_boolean`] / [`DatabaseWriter::write_booleans`] | `WriteBoolean` / `WriteBooleans` |
//! | [`DatabaseWriter::write_short`] / [`DatabaseWriter::write_char`] | `WriteShort` / `WriteChar` |
//! | [`DatabaseWriter::write_characters`]  | `SmDatabaseIO::WriteCharacters`                  |
//! | [`DatabaseWriter::write_type`]        | `SmDatabaseIO::WriteType`                        |
//! | [`DatabaseWriter::out_str`] ... [`fixed16`] | `*GetOutStreamPtr() << value` (`fixed`, precision 16) |
//! | [`is_big_endian`]                     | `SmDatabaseIO::IsBigEndian`                      |
//! | [`byte_swap_f64`]                     | `ByteSwap64(double)`, `ByteSwap128(double)`      |
//! | [`byte_swap_u64`]                     | `ByteSwap64(unsigned long)`, `ByteSwap64(unsigned long long)` |
//! | [`byte_swap_u32`]                     | `ByteSwap32(unsigned long)`, `ByteSwap32(uint32_t)` |
//! | [`byte_swap_u16`]                     | `ByteSwap16(unsigned short)`                     |
//! | [`InputStream`]                       | the `std::istream` behind `m_pStreamIn` (libstdc++ semantics) |
//! | [`InputStream::getline`]              | `std::istream::getline(char*, std::streamsize)`  |
//! | [`FileType`]                          | `SmFileType`                                     |
//!
//! Deviations:
//! - **No filesystem.** The library must build for `wasm32-unknown-unknown`, so
//!   [`DatabaseReader::open_for_read`] takes the file's bytes; the caller loads them
//!   (upstream's "file not found" failure belongs to the caller). An in-memory stream
//!   (`SmDatabaseIOStream` over an `std::istringstream`) is the same type.
//! - **Short binary reads are errors.** When a binary read hits the end of the data,
//!   upstream sets `failbit`, returns `SM_SUCCESS` and leaves the value partly or wholly
//!   uninitialised (undefined behaviour for the `version >= 40` locals). The port sets
//!   the same stream state and returns [`SolidError::InvalidInput`] instead.
//! - **LP64 integer sizes.** The reference build is Linux x86-64, where `long` and
//!   `ULONG` are 8 bytes, so the `version >= 40` (`long long`) and `version < 40`
//!   (`long`) branches of `ReadLong`/`ReadLongs` read the same 8 bytes and their
//!   `> LONG_MAX` / `> ULONG_MAX` range checks can never fire. The port reads 8-byte
//!   integers for every version. `SmBoolean` is a 4-byte `int`, `short` 2 bytes,
//!   `char` 1 byte (so the `sizeof(char) == 2` swap branches are dead).
//! - **ASCII extraction** (`*GetInStreamPtr() >> x`) is reproduced by the
//!   `extract_*` methods of [`InputStream`], following libstdc++ (`num_get`, `sentry`,
//!   `ignore`). An extraction whose sentry fails stores nothing: the methods return
//!   `None` and the caller keeps its previous value, as upstream does. `ReadType`'s
//!   `char sBuff[SM_MAXSIZE]` words are capped at 255 bytes
//!   ([`InputStream::extract_word_limited`]): libstdc++ 11+ bounds `>> char*` by
//!   `__builtin_object_size` when the caller is optimised, as the upstream build is.
//! - `ByteSwap32(unsigned short)` is not ported: it is only reached when
//!   `sizeof(short) != 2` and always returns 0 (its shifts are truncated to 16 bits).
//! - **Write side.** [`DatabaseWriter`] keeps the stream in memory (no filesystem on
//!   wasm32): only a new file (`bNewFile = TRUE`) is supported and appending is the
//!   caller's. Upstream's byte-swapped `WriteLong`/`WriteLongs` swap the variable's
//!   address instead of its value (`ByteSwap64((unsigned long long) &llLong)`) and so
//!   write garbage on big-endian machines or with `SM_BYTESWAP`; the port swaps the
//!   value. On the little-endian reference build with `SM_ASCII` or `SM_BINARY` no
//!   swap happens and the output agrees.
//! - **Nesting bound.** Upstream recurses through nested polymorphic reads (a curve
//!   holding a curve holding ...) without limit. The port stops at
//!   [`MAX_READ_NESTING`] = 32 levels ([`DatabaseReader::enter_nested`]); upstream
//!   fixtures nest at most 3. Past the bound the read is [`SolidError::InvalidInput`]
//!   with [`NESTING_LIMIT_REASON`].
//! - **Stack use (wasm32).** Measured on x86-64 (reader fuzz driver, smallest thread
//!   stack that reads the file): at the old 64-level bound a file of 64 chained offset
//!   surfaces needed about 2 MiB in an unoptimised (debug) build and 384 KiB at
//!   `opt-level = 1`. At 32 levels it needs about 1 MiB (debug) and 210 KiB
//!   (`opt-level = 1`); 16 pairs of a surface of revolution over an iso curve on the
//!   next one need 770 KiB and 180 KiB. `wasm32-unknown-unknown` has a 1 MiB default
//!   stack and no guard page, so an overflow there corrupts memory silently: wasm
//!   consumers must build the kernel optimised (`opt-level >= 1`) or raise the stack
//!   (`-C link-arg=-zstack-size=<bytes>`, at least 2 MiB for an unoptimised build).

use crate::error::{Result, SolidError};

/// `SM_MAXSIZE`: characters `GoToNextLine` skips at most before the newline, and
/// the size of `ReadType`'s word buffer.
pub const MAX_LINE: usize = 256;

/// Default database version of a new reader (`m_lVersion(30)`).
pub const DEFAULT_VERSION: u64 = 30;

/// Database encoding (upstream `SmFileType`); the discriminant is the upstream value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// `SM_ASCII`: text with comment lines.
    Ascii = 0,
    /// `SM_BINARY`: little-endian binary.
    Binary = 1,
    /// `SM_BYTESWAP`: binary with every value byte-swapped.
    ByteSwap = 2,
    /// `SM_UNKNOWN`: not set (read as binary).
    Unknown = 3,
}

impl FileType {
    /// The upstream `SmFileType` value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The file type for an upstream `SmFileType` value.
    pub fn from_code(code: i32) -> Result<Self> {
        match code {
            0 => Ok(Self::Ascii),
            1 => Ok(Self::Binary),
            2 => Ok(Self::ByteSwap),
            3 => Ok(Self::Unknown),
            _ => Err(SolidError::InvalidInput {
                operation: "topology::database_io::FileType::from_code",
                reason: "not an SmFileType value",
            }),
        }
    }

    /// The constructors' and `OpenFileForRead`'s rule for `m_bByteSwap`.
    fn default_byte_swap(self) -> bool {
        if self == Self::ByteSwap {
            true
        } else {
            is_big_endian()
        }
    }
}

/// Whether this machine is big-endian (upstream `SmDatabaseIO::IsBigEndian`).
#[must_use]
pub const fn is_big_endian() -> bool {
    cfg!(target_endian = "big")
}

/// Reverses the bytes of a double (upstream `ByteSwap64(double)`; `ByteSwap128` is
/// the same loop and also swaps 8 bytes).
#[must_use]
pub const fn byte_swap_f64(x: f64) -> f64 {
    f64::from_bits(x.to_bits().swap_bytes())
}

/// Reverses the bytes of a 64-bit integer (upstream `ByteSwap64` on
/// `unsigned long` / `unsigned long long`).
#[must_use]
pub const fn byte_swap_u64(x: u64) -> u64 {
    x.swap_bytes()
}

/// Reverses the bytes of a 32-bit integer (upstream `ByteSwap32`; the
/// `unsigned long` overload masks to the low 32 bits, which this type already is).
#[must_use]
pub const fn byte_swap_u32(x: u32) -> u32 {
    x.swap_bytes()
}

/// Swaps the two bytes of a 16-bit integer (upstream `ByteSwap16`).
#[must_use]
pub const fn byte_swap_u16(x: u16) -> u16 {
    x.swap_bytes()
}

/// `std::ios_base::iostate` bits of an [`InputStream`] (libstdc++ values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IoState(u8);

impl IoState {
    /// `goodbit`.
    pub const GOOD: Self = Self(0);
    /// `badbit`.
    pub const BAD: Self = Self(1);
    /// `eofbit`.
    pub const EOF: Self = Self(2);
    /// `failbit`.
    pub const FAIL: Self = Self(4);

    /// The raw `iostate` value.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every bit of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// `std::ctype<char>::is(space, c)` in the "C" locale.
fn is_space(c: u8) -> bool {
    c == b' ' || (b'\t'..=b'\r').contains(&c)
}

/// A byte input stream with `std::istream` state and extraction semantics (the
/// stream behind upstream `m_pStreamIn`, as libstdc++ implements it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputStream {
    data: Vec<u8>,
    pos: usize,
    state: IoState,
}

/// Digits and flags scanned by `num_get` for an integer.
struct IntScan {
    negative: bool,
    result: u64,
    overflow: bool,
    no_digits: bool,
}

impl InputStream {
    /// A stream over `data`, positioned at its start, in the good state.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            pos: 0,
            state: IoState::GOOD,
        }
    }

    /// Current state bits (`rdstate()`).
    #[must_use]
    pub fn state(&self) -> IoState {
        self.state
    }

    /// `good()`: no bit set.
    #[must_use]
    pub fn good(&self) -> bool {
        self.state == IoState::GOOD
    }

    /// `eof()`.
    #[must_use]
    pub fn eof(&self) -> bool {
        self.state.contains(IoState::EOF)
    }

    /// `fail()`: `failbit` or `badbit`.
    #[must_use]
    pub fn fail(&self) -> bool {
        self.state.contains(IoState::FAIL) || self.state.contains(IoState::BAD)
    }

    /// `clear()`: resets the state to good.
    pub fn clear(&mut self) {
        self.state = IoState::GOOD;
    }

    /// Offset of the next byte to read.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    fn set_state(&mut self, bits: IoState) {
        self.state = self.state.with(bits);
    }

    fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// `istream::sentry`: fails (setting `failbit`) unless the stream is good;
    /// when `skip_ws`, skips white space first (`eofbit|failbit` if it runs out).
    fn sentry(&mut self, skip_ws: bool) -> bool {
        if self.good() && skip_ws {
            while let Some(c) = self.peek_byte() {
                if !is_space(c) {
                    break;
                }
                self.pos += 1;
            }
            if self.at_end() {
                self.set_state(IoState::EOF);
            }
        }
        if self.good() {
            true
        } else {
            self.set_state(IoState::FAIL);
            false
        }
    }

    /// Bytes not yet consumed. Used to bound header-declared counts before
    /// allocating (not an upstream function; see `DatabaseReader::ensure_available`).
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// `istream::read(buf, n)`: up to `n` bytes; `eofbit|failbit` when fewer were
    /// available, `failbit` (nothing read) when the stream was not good.
    pub fn read_bytes(&mut self, n: usize) -> &[u8] {
        if !self.sentry(false) {
            return &[];
        }
        let start = self.pos;
        let avail = self.data.len().saturating_sub(start);
        let take = n.min(avail);
        self.pos = start + take;
        if take < n {
            self.set_state(IoState::EOF.with(IoState::FAIL));
        }
        self.data.get(start..start + take).unwrap_or(&[])
    }

    /// `istream::ignore(n, delim)`: skips bytes until `n` were skipped, the data
    /// ends (`eofbit`), or `delim` was skipped.
    pub fn ignore(&mut self, n: usize, delim: u8) {
        let ok = self.sentry(false);
        if n == 0 || !ok {
            return;
        }
        let mut count = 0usize;
        while count < n {
            let Some(c) = self.peek_byte() else {
                self.set_state(IoState::EOF);
                break;
            };
            self.pos += 1;
            count += 1;
            if c == delim {
                break;
            }
        }
    }

    /// `istream::getline(buf, n)` (delimiter `'\n'`): the bytes stored in the
    /// `n`-byte buffer (without the terminating NUL). Stores at most `n - 1` bytes,
    /// stopping before the end of the data (`eofbit`) or at a newline (consumed, not
    /// stored); when the buffer fills before a newline, `failbit` is set and the next
    /// byte is left in the stream. Nothing extracted sets `failbit`, as does a stream
    /// that was not good (nothing is read; the buffer is empty).
    pub fn getline(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.sentry(false) {
            return out;
        }
        let mut gcount = 0usize;
        while gcount + 1 < n {
            match self.peek_byte() {
                Some(c) if c != b'\n' => {
                    out.push(c);
                    self.pos += 1;
                    gcount += 1;
                }
                _ => break,
            }
        }
        match self.peek_byte() {
            None => self.set_state(IoState::EOF),
            Some(b'\n') => {
                gcount += 1;
                self.pos += 1;
            }
            Some(_) => self.set_state(IoState::FAIL),
        }
        if gcount == 0 {
            self.set_state(IoState::FAIL);
        }
        out
    }

    /// `istream::get()`: the next byte, or `None` (`eofbit|failbit`) at the end
    /// (`failbit` if the stream was not good).
    pub fn get(&mut self) -> Option<u8> {
        if !self.sentry(false) {
            return None;
        }
        if let Some(c) = self.peek_byte() {
            self.pos += 1;
            Some(c)
        } else {
            self.set_state(IoState::EOF.with(IoState::FAIL));
            None
        }
    }

    /// `istream::unget()`: clears `eofbit`, then steps back one byte (`badbit` at
    /// the start of the data).
    pub fn unget(&mut self) {
        self.state = self.state.without(IoState::EOF);
        if !self.sentry(false) {
            return;
        }
        if self.pos == 0 {
            self.set_state(IoState::BAD);
        } else {
            self.pos -= 1;
        }
    }

    /// `is >> char_buffer` with a buffer of unknown size: the next white-space
    /// delimited word. `None` when the sentry fails (nothing stored).
    pub fn extract_word(&mut self) -> Option<Vec<u8>> {
        self.extract_word_limited(usize::MAX)
    }

    /// `is >> char_array` into a `buf_size`-byte array whose size the compiler can
    /// see: at most `buf_size - 1` bytes are taken and the rest of the word stays in
    /// the stream (libstdc++ 11+ `__builtin_object_size` overload, active when the
    /// caller is compiled with optimisation, as upstream is). `eofbit` is set when
    /// the data ends right after the extracted bytes. `None` when the sentry fails.
    pub fn extract_word_limited(&mut self, buf_size: usize) -> Option<Vec<u8>> {
        if !self.sentry(true) {
            return None;
        }
        let limit = buf_size.saturating_sub(1);
        let start = self.pos;
        while self.pos - start < limit {
            match self.peek_byte() {
                Some(c) if !is_space(c) => self.pos += 1,
                _ => break,
            }
        }
        if self.at_end() {
            self.set_state(IoState::EOF);
        }
        let word = self.data.get(start..self.pos).unwrap_or(&[]).to_vec();
        if word.is_empty() {
            self.set_state(IoState::FAIL);
        }
        Some(word)
    }

    /// `num_get::_M_extract_int` stage 1 and 2 in base 10, "C" locale.
    /// `max_for(negative)` is the largest magnitude the target type accepts.
    fn scan_int(&mut self, max_for: impl Fn(bool) -> u64) -> IntScan {
        let mut negative = false;
        if let Some(c) = self.peek_byte() {
            if c == b'-' || c == b'+' {
                negative = c == b'-';
                self.pos += 1;
            }
        }
        let mut found_zero = false;
        let mut sep_pos = 0usize;
        while self.peek_byte() == Some(b'0') {
            found_zero = true;
            sep_pos += 1;
            self.pos += 1;
        }
        let max = max_for(negative);
        let smax = max / 10;
        let mut result: u64 = 0;
        let mut overflow = false;
        while let Some(c) = self.peek_byte() {
            if !c.is_ascii_digit() {
                break;
            }
            let digit = u64::from(c - b'0');
            if result > smax {
                overflow = true;
            } else {
                result *= 10;
                overflow |= result > max - digit;
                result = result.wrapping_add(digit);
                sep_pos += 1;
            }
            self.pos += 1;
        }
        IntScan {
            negative,
            result,
            overflow,
            no_digits: sep_pos == 0 && !found_zero,
        }
    }

    /// `num_get::get(long&)` after a successful sentry.
    fn num_get_long(&mut self) -> i64 {
        let scan = self.scan_int(|negative| {
            if negative {
                1u64 << 63
            } else {
                i64::MAX as u64
            }
        });
        let value = if scan.no_digits {
            self.set_state(IoState::FAIL);
            0
        } else if scan.overflow {
            self.set_state(IoState::FAIL);
            if scan.negative {
                i64::MIN
            } else {
                i64::MAX
            }
        } else if scan.negative {
            (scan.result as i64).wrapping_neg()
        } else {
            scan.result as i64
        };
        if self.at_end() {
            self.set_state(IoState::EOF);
        }
        value
    }

    /// `num_get::get(unsigned long&)` after a successful sentry.
    fn num_get_ulong(&mut self) -> u64 {
        let scan = self.scan_int(|_| u64::MAX);
        let value = if scan.no_digits {
            self.set_state(IoState::FAIL);
            0
        } else if scan.overflow {
            self.set_state(IoState::FAIL);
            u64::MAX
        } else if scan.negative {
            scan.result.wrapping_neg()
        } else {
            scan.result
        };
        if self.at_end() {
            self.set_state(IoState::EOF);
        }
        value
    }

    /// `is >> long_value`. `None` when the sentry fails (nothing stored); otherwise
    /// the stored value (`0` with `failbit` when no digits, clamped with `failbit`
    /// on overflow).
    pub fn extract_long(&mut self) -> Option<i64> {
        if !self.sentry(true) {
            return None;
        }
        Some(self.num_get_long())
    }

    /// `is >> unsigned_long_value` (a leading `-` negates modulo 2^64, as libstdc++).
    pub fn extract_ulong(&mut self) -> Option<u64> {
        if !self.sentry(true) {
            return None;
        }
        Some(self.num_get_ulong())
    }

    /// `is >> int_value`: extracts a `long`, then clamps to `i32` (`failbit` when
    /// out of range).
    pub fn extract_int(&mut self) -> Option<i32> {
        if !self.sentry(true) {
            return None;
        }
        let l = self.num_get_long();
        Some(if l < i64::from(i32::MIN) {
            self.set_state(IoState::FAIL);
            i32::MIN
        } else if l > i64::from(i32::MAX) {
            self.set_state(IoState::FAIL);
            i32::MAX
        } else {
            l as i32
        })
    }

    /// `is >> short_value`: extracts a `long`, then clamps to `i16` (`failbit` when
    /// out of range).
    pub fn extract_short(&mut self) -> Option<i16> {
        if !self.sentry(true) {
            return None;
        }
        let l = self.num_get_long();
        Some(if l < i64::from(i16::MIN) {
            self.set_state(IoState::FAIL);
            i16::MIN
        } else if l > i64::from(i16::MAX) {
            self.set_state(IoState::FAIL);
            i16::MAX
        } else {
            l as i16
        })
    }

    /// `is >> double_value`: `num_get::_M_extract_float` then `strtod` in the "C"
    /// locale. `0` with `failbit` when the characters do not form a number,
    /// `±f64::MAX` with `failbit` on overflow. `None` when the sentry fails.
    pub fn extract_double(&mut self) -> Option<f64> {
        if !self.sentry(true) {
            return None;
        }
        let mut xtrc = String::new();
        if let Some(c) = self.peek_byte() {
            if c == b'+' || c == b'-' {
                xtrc.push(char::from(c));
                self.pos += 1;
            }
        }
        let mut found_mantissa = false;
        while self.peek_byte() == Some(b'0') {
            if !found_mantissa {
                xtrc.push('0');
                found_mantissa = true;
            }
            self.pos += 1;
        }
        let mut found_dec = false;
        let mut found_sci = false;
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_digit() {
                xtrc.push(char::from(c));
                found_mantissa = true;
            } else if c == b'.' && !found_dec && !found_sci {
                xtrc.push('.');
                found_dec = true;
            } else if (c == b'e' || c == b'E') && !found_sci && found_mantissa {
                xtrc.push('e');
                found_sci = true;
                self.pos += 1;
                match self.peek_byte() {
                    Some(s) if s == b'+' || s == b'-' => xtrc.push(char::from(s)),
                    Some(_) => continue,
                    None => break,
                }
            } else {
                break;
            }
            self.pos += 1;
        }
        let value = match xtrc.parse::<f64>() {
            Ok(v) if v == f64::INFINITY => {
                self.set_state(IoState::FAIL);
                f64::MAX
            }
            Ok(v) if v == f64::NEG_INFINITY => {
                self.set_state(IoState::FAIL);
                -f64::MAX
            }
            Ok(v) => v,
            Err(_) => {
                self.set_state(IoState::FAIL);
                0.0
            }
        };
        if self.at_end() {
            self.set_state(IoState::EOF);
        }
        Some(value)
    }
}

/// Result of [`DatabaseReader::read_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeHeader {
    /// The object type code (`SmCurve_TYPE`, ...). `None` when an ASCII stream had
    /// already failed, so upstream left its variable untouched.
    pub type_code: Option<i64>,
    /// The curve dimension, when requested and stored (`None` as for `type_code`,
    /// or when not requested).
    pub dim: Option<u64>,
}

/// Reads values from an SMLib database stream (upstream `SmDatabaseIO` read side).
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseReader {
    file_type: FileType,
    stream: Option<InputStream>,
    byte_swap: bool,
    version: u64,
    /// Current depth of nested polymorphic reads (not upstream; see `enter_nested`).
    nesting: u32,
}

/// Deepest nesting of polymorphic reads (a curve holding a curve holding ...) a
/// reader accepts. Not upstream: upstream recurses without bound, so a crafted file
/// overflows the stack. Real models nest a handful of levels (upstream fixtures at
/// most 3). 32 (64 before) halves the stack a crafted file can use; see the module
/// docs, "Stack use", for what wasm32 builds still need.
pub const MAX_READ_NESTING: u32 = 32;

/// The `reason` of the [`SolidError::InvalidInput`] that
/// [`DatabaseReader::enter_nested`] returns past [`MAX_READ_NESTING`]. Readers that
/// ignore a nested read's status as upstream does must still propagate this one.
pub const NESTING_LIMIT_REASON: &str = "objects nested too deeply in the database stream";

impl Default for DatabaseReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Error for a binary read that ran past the end of the data.
fn short_read(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "database stream ended before the value was read",
    }
}

impl DatabaseReader {
    /// A reader without a stream, file type [`FileType::Unknown`] (upstream default
    /// constructor).
    #[must_use]
    pub fn new() -> Self {
        Self {
            file_type: FileType::Unknown,
            stream: None,
            byte_swap: is_big_endian(),
            version: DEFAULT_VERSION,
            nesting: 0,
        }
    }

    /// A reader of `file_type` without a stream (`SmDatabaseIO(SmFileType)`).
    #[must_use]
    pub fn with_file_type(file_type: FileType) -> Self {
        Self {
            file_type,
            stream: None,
            byte_swap: file_type.default_byte_swap(),
            version: DEFAULT_VERSION,
            nesting: 0,
        }
    }

    /// A reader of `file_type` over `stream` (`SmDatabaseIO(SmFileType, istream*)`).
    #[must_use]
    pub fn with_stream(file_type: FileType, stream: InputStream) -> Self {
        Self {
            stream: Some(stream),
            ..Self::with_file_type(file_type)
        }
    }

    /// Starts reading `data` (the contents of a file) as `file_type`
    /// (`SmDatabaseIOFile::OpenFileForRead`): replaces any stream and resets the
    /// byte-swap flag for `file_type`. The version is kept.
    pub fn open_for_read(&mut self, data: Vec<u8>, file_type: FileType) {
        self.stream = Some(InputStream::new(data));
        self.file_type = file_type;
        self.byte_swap = file_type.default_byte_swap();
    }

    /// The file type (`GetFileType`).
    #[must_use]
    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    /// The input stream, if any (`GetInStreamPtr`).
    #[must_use]
    pub fn in_stream(&self) -> Option<&InputStream> {
        self.stream.as_ref()
    }

    /// Rejects a header-declared size before it is allocated: `count` items of at least
    /// `min_bytes_each` bytes must fit in what is left of the stream.
    ///
    /// Not an upstream function. Upstream `malloc`s whatever a file header asks for and
    /// fails later at end of data; Rust would abort on a failed huge allocation, so
    /// readers call this first. Any count this rejects cannot be read to completion
    /// from the remaining bytes. See `plan/findings.md` ("header-declared sizes").
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when there is no stream or the items cannot fit.
    pub fn ensure_available(
        &self,
        count: usize,
        min_bytes_each: usize,
        operation: &'static str,
    ) -> Result<()> {
        let left = self.stream.as_ref().map_or(0, InputStream::remaining);
        match count.checked_mul(min_bytes_each) {
            Some(need) if need <= left => Ok(()),
            _ => Err(SolidError::InvalidInput {
                operation,
                reason: "header-declared size exceeds the remaining input",
            }),
        }
    }

    /// Enters one level of nested polymorphic reading (`Curve::read_from_db` and the
    /// like call this before dispatching, and [`Self::leave_nested`] after).
    ///
    /// Not an upstream function; bounds recursion on crafted input (see
    /// [`MAX_READ_NESTING`] and `plan/findings.md`, "nested reads").
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the nesting limit is reached.
    pub fn enter_nested(&mut self, operation: &'static str) -> Result<()> {
        if self.nesting >= MAX_READ_NESTING {
            return Err(SolidError::InvalidInput {
                operation,
                reason: NESTING_LIMIT_REASON,
            });
        }
        self.nesting += 1;
        Ok(())
    }

    /// Leaves one level entered with [`Self::enter_nested`].
    pub fn leave_nested(&mut self) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    /// The input stream for ASCII extraction (`*GetInStreamPtr() >> ...`).
    pub fn in_stream_mut(&mut self) -> Option<&mut InputStream> {
        self.stream.as_mut()
    }

    /// Whether values are byte-swapped after reading (`GetByteSwap`).
    #[must_use]
    pub fn byte_swap(&self) -> bool {
        self.byte_swap
    }

    /// Sets the byte-swap flag (`SetByteSwap`).
    pub fn set_byte_swap(&mut self, byte_swap: bool) {
        self.byte_swap = byte_swap;
    }

    /// Database version (`GetVersionNum`).
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Sets the database version (`SetVersionNum`).
    pub fn set_version(&mut self, version: u64) {
        self.version = version;
    }

    /// Hook before reading (`BeginReading`); does nothing in the base reader.
    pub fn begin_reading(&mut self) {}

    /// Hook after reading (`EndReading`); does nothing in the base reader.
    pub fn end_reading(&mut self) {}

    /// Skips to the start of the next line, at most [`MAX_LINE`] bytes
    /// (`GoToNextLine`: `ignore(SM_MAXSIZE, '\n')`). At the end of the input there is no
    /// next line: skipping there would mark the stream failed, as if the last value read
    /// had not been a number, so nothing is done.
    pub fn go_to_next_line(&mut self) {
        if let Some(s) = self.stream.as_mut() {
            if !s.eof() {
                s.ignore(MAX_LINE, b'\n');
            }
        }
    }

    /// Whether a read has run past the end of the input or met text that is not a number
    /// (`ReadFailed`: the stream's `fail()`); `false` without a stream. The `read_*` and
    /// `extract_*` reads do not report either, so callers check this.
    #[must_use]
    pub fn read_failed(&self) -> bool {
        self.stream.as_ref().is_some_and(InputStream::fail)
    }

    /// Reads exactly `N` raw bytes.
    fn read_raw<const N: usize>(&mut self, operation: &'static str) -> Result<[u8; N]> {
        let s = self
            .stream
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(operation))?;
        let bytes = s.read_bytes(N);
        <[u8; N]>::try_from(bytes).map_err(|_| short_read(operation))
    }

    /// Reads `n` items of `N` raw bytes each.
    fn read_raw_items<const N: usize>(
        &mut self,
        n: usize,
        operation: &'static str,
    ) -> Result<Vec<[u8; N]>> {
        let s = self
            .stream
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(operation))?;
        let total = n.checked_mul(N).ok_or_else(|| short_read(operation))?;
        let bytes = s.read_bytes(total);
        if bytes.len() != total {
            return Err(short_read(operation));
        }
        Ok(bytes
            .chunks_exact(N)
            .map(|c| {
                let mut a = [0u8; N];
                a.copy_from_slice(c);
                a
            })
            .collect())
    }

    /// Reads a signed 8-byte integer (`ReadLong(long&)`).
    pub fn read_long(&mut self) -> Result<i64> {
        let raw = u64::from_ne_bytes(
            self.read_raw::<8>("topology::database_io::DatabaseReader::read_long")?,
        );
        Ok(if self.byte_swap {
            byte_swap_u64(raw) as i64
        } else {
            raw as i64
        })
    }

    /// Reads an unsigned 8-byte integer (`ReadLong(ULONG&)`).
    pub fn read_ulong(&mut self) -> Result<u64> {
        let raw = u64::from_ne_bytes(
            self.read_raw::<8>("topology::database_io::DatabaseReader::read_ulong")?,
        );
        Ok(if self.byte_swap {
            byte_swap_u64(raw)
        } else {
            raw
        })
    }

    /// Reads `n` unsigned 8-byte integers (`ReadLongs`).
    pub fn read_longs(&mut self, n: usize) -> Result<Vec<u64>> {
        let items =
            self.read_raw_items::<8>(n, "topology::database_io::DatabaseReader::read_longs")?;
        let swap = self.byte_swap;
        Ok(items
            .into_iter()
            .map(|b| {
                let v = u64::from_ne_bytes(b);
                if swap {
                    byte_swap_u64(v)
                } else {
                    v
                }
            })
            .collect())
    }

    /// Reads a double (`ReadDouble`).
    pub fn read_double(&mut self) -> Result<f64> {
        let raw = f64::from_ne_bytes(
            self.read_raw::<8>("topology::database_io::DatabaseReader::read_double")?,
        );
        Ok(if self.byte_swap {
            byte_swap_f64(raw)
        } else {
            raw
        })
    }

    /// Reads `n` doubles (`ReadDoubles`).
    pub fn read_doubles(&mut self, n: usize) -> Result<Vec<f64>> {
        let items =
            self.read_raw_items::<8>(n, "topology::database_io::DatabaseReader::read_doubles")?;
        let swap = self.byte_swap;
        Ok(items
            .into_iter()
            .map(|b| {
                let v = f64::from_ne_bytes(b);
                if swap {
                    byte_swap_f64(v)
                } else {
                    v
                }
            })
            .collect())
    }

    /// Reads a 4-byte `SmBoolean` (`ReadBoolean`). Upstream never byte-swaps here
    /// (unlike [`Self::read_booleans`]); the port keeps that.
    pub fn read_boolean(&mut self) -> Result<i32> {
        Ok(i32::from_ne_bytes(self.read_raw::<4>(
            "topology::database_io::DatabaseReader::read_boolean",
        )?))
    }

    /// Reads `n` 4-byte `SmBoolean`s (`ReadBooleans`), byte-swapped when enabled.
    pub fn read_booleans(&mut self, n: usize) -> Result<Vec<i32>> {
        let items =
            self.read_raw_items::<4>(n, "topology::database_io::DatabaseReader::read_booleans")?;
        let swap = self.byte_swap;
        Ok(items
            .into_iter()
            .map(|b| {
                let v = u32::from_ne_bytes(b);
                if swap {
                    byte_swap_u32(v) as i32
                } else {
                    v as i32
                }
            })
            .collect())
    }

    /// Reads a 2-byte `short` (`ReadShort`).
    pub fn read_short(&mut self) -> Result<i16> {
        let raw = u16::from_ne_bytes(
            self.read_raw::<2>("topology::database_io::DatabaseReader::read_short")?,
        );
        Ok(if self.byte_swap {
            byte_swap_u16(raw) as i16
        } else {
            raw as i16
        })
    }

    /// Reads one byte (`ReadChar`; never swapped).
    pub fn read_char(&mut self) -> Result<u8> {
        let [c] = self.read_raw::<1>("topology::database_io::DatabaseReader::read_char")?;
        Ok(c)
    }

    /// Reads `n` bytes (`ReadCharacters`; never swapped: `char` is one byte).
    pub fn read_characters(&mut self, n: usize) -> Result<Vec<u8>> {
        let items =
            self.read_raw_items::<1>(n, "topology::database_io::DatabaseReader::read_characters")?;
        Ok(items.into_iter().map(|[c]| c).collect())
    }

    /// Reads an object type header (`ReadType`): in ASCII the line
    /// `Object Type <code>` and, when `with_dim`, `Curve Dim <dim>`; in binary a
    /// long and optionally an unsigned long.
    pub fn read_type(&mut self, with_dim: bool) -> Result<TypeHeader> {
        const OP: &str = "topology::database_io::DatabaseReader::read_type";
        if self.file_type == FileType::Ascii {
            let s = self
                .stream
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            // `char sBuff[SM_MAXSIZE]`: words longer than 255 bytes are split.
            let _ = s.extract_word_limited(MAX_LINE);
            let _ = s.extract_word_limited(MAX_LINE);
            let type_code = s.extract_long();
            s.ignore(MAX_LINE, b'\n');
            let mut dim = None;
            if with_dim {
                let _ = s.extract_word_limited(MAX_LINE);
                let _ = s.extract_word_limited(MAX_LINE);
                dim = s.extract_ulong();
                s.ignore(MAX_LINE, b'\n');
            }
            Ok(TypeHeader { type_code, dim })
        } else {
            let type_code = self.read_long()?;
            let dim = if with_dim {
                Some(self.read_ulong()?)
            } else {
                None
            };
            Ok(TypeHeader {
                type_code: Some(type_code),
                dim,
            })
        }
    }
}

// ---- write side -----------------------------------------------------------------------

/// `std::ostream << double` with `fixed` and `precision(16)` (glibc `%.16f`): `inf`,
/// `-inf`, `nan` and `-nan` for the special values.
#[must_use]
pub fn fixed16(x: f64) -> String {
    if x.is_nan() {
        if x.is_sign_negative() {
            "-nan".to_string()
        } else {
            "nan".to_string()
        }
    } else if x.is_infinite() {
        if x < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        }
    } else {
        format!("{x:.16}")
    }
}

/// Writes values to an SMLib database stream (upstream `SmDatabaseIO` write side over
/// the stream `SmDatabaseIOFile::OpenFileForWrite` opens), kept in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseWriter {
    file_type: FileType,
    byte_swap: bool,
    out: Vec<u8>,
}

impl DatabaseWriter {
    /// `SmDatabaseIOFile::OpenFileForWrite(name, eType, TRUE)`: an empty stream of
    /// `file_type`, byte-swapped for [`FileType::ByteSwap`] or on a big-endian machine.
    /// An ASCII stream writes doubles `fixed` with precision 16 ([`fixed16`]).
    #[must_use]
    pub fn open_for_write(file_type: FileType) -> Self {
        Self {
            file_type,
            byte_swap: file_type.default_byte_swap(),
            out: Vec::new(),
        }
    }

    /// The file type (`GetFileType`).
    #[must_use]
    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    /// Whether binary values are byte-swapped (`m_bByteSwap`).
    #[must_use]
    pub fn byte_swap(&self) -> bool {
        self.byte_swap
    }

    /// The bytes written so far (the file's contents).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.out
    }

    /// The bytes written (the file's contents).
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }

    /// Hook before writing (`BeginWriting`); does nothing in the base writer.
    pub fn begin_writing(&mut self) {}

    /// Hook after writing (`EndWriting`); does nothing in the base writer.
    pub fn end_writing(&mut self) {}

    /// `*GetOutStreamPtr() << "text"`.
    pub fn out_str(&mut self, s: &str) -> &mut Self {
        self.out.extend_from_slice(s.as_bytes());
        self
    }

    /// `*GetOutStreamPtr() << value` for a signed integer (`long`, `int`, `short`).
    pub fn out_i64(&mut self, v: i64) -> &mut Self {
        self.out.extend_from_slice(v.to_string().as_bytes());
        self
    }

    /// `*GetOutStreamPtr() << value` for an unsigned integer (`ULONG`).
    pub fn out_u64(&mut self, v: u64) -> &mut Self {
        self.out.extend_from_slice(v.to_string().as_bytes());
        self
    }

    /// `*GetOutStreamPtr() << value` for a double ([`fixed16`]).
    pub fn out_f64(&mut self, v: f64) -> &mut Self {
        self.out.extend_from_slice(fixed16(v).as_bytes());
        self
    }

    /// `v0 << " " << v1 << ...`: doubles separated by single spaces (not an upstream
    /// function).
    pub fn out_reals(&mut self, values: &[f64]) -> &mut Self {
        for (i, &v) in values.iter().enumerate() {
            if i > 0 {
                self.out_str(" ");
            }
            self.out_f64(v);
        }
        self
    }

    /// One line of doubles, the common shape of the geometry writers (not an upstream
    /// function): `v0 << " " << v1 ... << "\n"` in ASCII, `WriteDouble` of each in
    /// binary.
    pub fn reals(&mut self, values: &[f64]) {
        if self.file_type == FileType::Ascii {
            self.out_reals(values).out_str("\n");
        } else {
            self.write_doubles(values);
        }
    }

    fn put(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    /// Writes a signed 8-byte integer (`WriteLong`).
    pub fn write_long(&mut self, v: i64) {
        let v = if self.byte_swap {
            byte_swap_u64(v as u64) as i64
        } else {
            v
        };
        self.put(&v.to_ne_bytes());
    }

    /// Writes unsigned 8-byte integers (`WriteLongs`).
    pub fn write_longs(&mut self, values: &[u64]) {
        for &v in values {
            let v = if self.byte_swap { byte_swap_u64(v) } else { v };
            self.put(&v.to_ne_bytes());
        }
    }

    /// Writes a double (`WriteDouble`).
    pub fn write_double(&mut self, v: f64) {
        let v = if self.byte_swap { byte_swap_f64(v) } else { v };
        self.put(&v.to_ne_bytes());
    }

    /// Writes doubles (`WriteDoubles`).
    pub fn write_doubles(&mut self, values: &[f64]) {
        for &v in values {
            self.write_double(v);
        }
    }

    /// Writes bytes (`WriteCharacters`; never swapped: `char` is one byte).
    pub fn write_characters(&mut self, chars: &[u8]) {
        self.put(chars);
    }

    /// Writes a 4-byte `SmBoolean` (`WriteBoolean`; never swapped, as upstream).
    pub fn write_boolean(&mut self, v: i32) {
        self.put(&v.to_ne_bytes());
    }

    /// Writes 4-byte `SmBoolean`s (`WriteBooleans`), byte-swapped when enabled.
    pub fn write_booleans(&mut self, values: &[i32]) {
        for &v in values {
            if self.byte_swap {
                self.put(&byte_swap_u32(v as u32).to_ne_bytes());
            } else {
                self.put(&v.to_ne_bytes());
            }
        }
    }

    /// Writes a 2-byte `short` (`WriteShort`).
    pub fn write_short(&mut self, v: i16) {
        let v = if self.byte_swap {
            byte_swap_u16(v as u16) as i16
        } else {
            v
        };
        self.put(&v.to_ne_bytes());
    }

    /// Writes one byte (`WriteChar`; never swapped).
    pub fn write_char(&mut self, c: u8) {
        self.put(&[c]);
    }

    /// Writes an object type header (`WriteType`): in ASCII the line
    /// `Object Type <code>` and, with `dim`, `Curve Dim <dim>`; in binary a long and
    /// optionally a second long.
    pub fn write_type(&mut self, type_code: i64, dim: Option<u64>) {
        if self.file_type == FileType::Ascii {
            self.out_str("Object Type ").out_i64(type_code);
            self.out_str("\n");
            if let Some(d) = dim {
                self.out_str("Curve Dim ").out_u64(d).out_str("\n");
            }
        } else {
            self.write_long(type_code);
            if let Some(d) = dim {
                self.write_long(d as i64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_is_bounded() {
        let mut db = DatabaseReader::new();
        for _ in 0..MAX_READ_NESTING {
            assert!(db.enter_nested("test").is_ok());
        }
        assert!(db.enter_nested("test").is_err());
        db.leave_nested();
        assert!(db.enter_nested("test").is_ok());
    }

    /// Bilinear B-spline surface record (version 30), the innermost surface of the
    /// nesting chains below; it does not count a nesting level.
    const BSPLINE_SURFACE: &str =
        "Object Type 15002\n1 1\n1 1\n0\n0 0 0\n0 3 0\n2 0 0\n2 3 1\n0\n0\n1\n1\n0\n0\n1\n1\n";
    /// One offset surface wrapper (one nesting level), as the reader fuzz builds it.
    const OFFSET_WRAPPER: &str = "Object Type 15400\n0.1\n0\n0\n0\n0\n//base\n";
    /// A surface of revolution whose generator is an iso curve on the next surface
    /// (two nesting levels), as the reader fuzz builds it.
    const REV_ISO_PAIR: &str = "Object Type 15020\n0\n0\n0 0 0\n1 0 0\n0 1 0\n0 0 1 1\n0\n//gen\n\
                                Object Type 14045\nCurve Dim 3\n0\n0.5\n1\n//surf\n";

    /// Reads the surface (type header and record) at the start of `text`.
    fn read_surface(text: &str) -> Result<crate::surface::surface::Surface> {
        let k = crate::kernel::Kernel::default();
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let ty = db.read_type(false)?.type_code.unwrap_or_default();
        crate::surface::surface::Surface::read_from_db(&k, ty, &mut db, 30)
    }

    fn is_nesting_error(r: &Result<crate::surface::surface::Surface>) -> bool {
        matches!(r, Err(SolidError::InvalidInput { reason, .. }) if *reason == NESTING_LIMIT_REASON)
    }

    /// `offsets` offset surfaces chained over a B-spline surface.
    fn offset_chain(offsets: usize) -> String {
        let mut text = OFFSET_WRAPPER.repeat(offsets);
        text.push_str(BSPLINE_SURFACE);
        // the line break each offset skips after its base record
        text.push_str(&"\n".repeat(offsets));
        text
    }

    /// `offsets` offset surfaces over `pairs` revolution/iso pairs over a B-spline
    /// surface: `offsets + 2 * pairs` nesting levels.
    fn rev_iso_chain(offsets: usize, pairs: usize) -> String {
        let mut text = OFFSET_WRAPPER.repeat(offsets);
        text.push_str(&REV_ISO_PAIR.repeat(pairs));
        text.push_str(BSPLINE_SURFACE);
        text
    }

    #[test]
    fn offset_surface_chain_reads_to_the_bound() {
        assert_eq!(MAX_READ_NESTING, 32);
        // Each offset wrapper takes two nesting levels (the `Surface::read_from_db`
        // dispatcher and `OffsetSurface::read_from_db` both enter one), so 15 wrappers
        // over the B-spline fit in the bound and 16 do not.
        for depth in [1, 14, 15] {
            let s = read_surface(&offset_chain(depth));
            assert!(s.is_ok(), "depth {depth}: {s:?}");
        }
        for depth in [16, 64, 1000] {
            let s = read_surface(&offset_chain(depth));
            assert!(is_nesting_error(&s), "depth {depth}: {s:?}");
        }
    }

    #[test]
    fn revolution_iso_chain_is_bounded() {
        // An iso curve is not derived from `SmBSplineCurve`, so the innermost surface
        // of revolution rejects its generator once every level has been read: up to
        // the bound the chain fails with that error, past it with the nesting error.
        let not_b_spline = crate::surface::surf_of_revolution::p1::not_b_spline();
        // A revolution/iso pair takes three nesting levels on main's readers, so 10
        // pairs fit in the bound and 11 do not.
        for (offsets, pairs) in [(0, 1), (0, 9), (0, 10)] {
            let s = read_surface(&rev_iso_chain(offsets, pairs));
            assert_eq!(
                s.as_ref().err(),
                Some(&not_b_spline),
                "{} levels",
                offsets + 2 * pairs
            );
        }
        for (offsets, pairs) in [(0, 11), (0, 17), (0, 32), (0, 500)] {
            let s = read_surface(&rev_iso_chain(offsets, pairs));
            assert!(
                is_nesting_error(&s),
                "{} levels: {s:?}",
                offsets + 2 * pairs
            );
        }
    }

    fn ascii(s: &str) -> InputStream {
        InputStream::new(s.as_bytes().to_vec())
    }

    #[test]
    fn swaps() {
        assert_eq!(byte_swap_u16(0x1234), 0x3412);
        assert_eq!(byte_swap_u32(0x1234_5678), 0x7856_3412);
        assert_eq!(byte_swap_u64(0x0102_0304_0506_0708), 0x0807_0605_0403_0201);
        assert_eq!(byte_swap_f64(byte_swap_f64(1.5)), 1.5);
        assert!(!is_big_endian());
    }

    #[test]
    fn binary_reads_and_short_read() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-7i64).to_le_bytes());
        data.extend_from_slice(&2.5f64.to_le_bytes());
        data.extend_from_slice(&1i32.to_le_bytes());
        data.extend_from_slice(&0x0102i16.to_le_bytes());
        data.push(b'x');
        let mut r = DatabaseReader::with_stream(FileType::Binary, InputStream::new(data));
        assert_eq!(r.read_long(), Ok(-7));
        assert_eq!(r.read_double(), Ok(2.5));
        assert_eq!(r.read_boolean(), Ok(1));
        assert_eq!(r.read_short(), Ok(0x0102));
        assert_eq!(r.read_char(), Ok(b'x'));
        assert!(r.in_stream().is_some_and(InputStream::good));
        assert!(r.read_char().is_err());
        let st = r.in_stream().map(InputStream::state);
        assert_eq!(st, Some(IoState::EOF.with(IoState::FAIL)));
        // zero-length reads succeed even on a failed stream
        assert_eq!(r.read_doubles(0), Ok(vec![]));
    }

    #[test]
    fn byte_swapped_reads() {
        let data = 3u64.to_be_bytes().to_vec();
        let mut r = DatabaseReader::with_stream(FileType::ByteSwap, InputStream::new(data));
        assert!(r.byte_swap());
        assert_eq!(r.read_ulong(), Ok(3));
    }

    #[test]
    fn no_stream_is_null_pointer() {
        let mut r = DatabaseReader::new();
        assert!(matches!(
            r.read_long(),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
        r.go_to_next_line();
    }

    #[test]
    fn ascii_type_header() {
        let mut r = DatabaseReader::with_stream(
            FileType::Ascii,
            ascii("Object Type 14010\r\nCurve Dim 2\r\n0.5\n"),
        );
        let h = r.read_type(true);
        assert_eq!(
            h,
            Ok(TypeHeader {
                type_code: Some(14010),
                dim: Some(2)
            })
        );
        let v = r.in_stream_mut().and_then(InputStream::extract_double);
        assert_eq!(v, Some(0.5));
    }

    #[test]
    fn integer_extraction_rules() {
        let mut s = ascii(" -12 +7 00 x 99999999999999999999 -5");
        assert_eq!(s.extract_long(), Some(-12));
        assert_eq!(s.extract_long(), Some(7));
        assert_eq!(s.extract_long(), Some(0));
        assert!(s.good());
        assert_eq!(s.extract_long(), Some(0));
        assert!(s.fail());
        s.clear();
        let _ = s.extract_word();
        assert_eq!(s.extract_long(), Some(i64::MAX));
        s.clear();
        assert_eq!(s.extract_ulong(), Some(5u64.wrapping_neg()));
        assert!(s.eof());
        assert_eq!(s.extract_long(), None);
        let mut s = ascii("40000 -40000");
        assert_eq!(s.extract_short(), Some(i16::MAX));
        s.clear();
        assert_eq!(s.extract_short(), Some(i16::MIN));
    }

    #[test]
    fn double_extraction_rules() {
        let mut s = ascii("1.5e3 -0.25 1e999 1e+ 7");
        assert_eq!(s.extract_double(), Some(1500.0));
        assert_eq!(s.extract_double(), Some(-0.25));
        assert_eq!(s.extract_double(), Some(f64::MAX));
        s.clear();
        assert_eq!(s.extract_double(), Some(0.0));
        assert!(s.fail());
        s.clear();
        assert_eq!(s.extract_double(), Some(7.0));
        assert!(s.eof());
    }

    #[test]
    fn ignore_get_unget() {
        let mut s = ascii("ab\ncd");
        s.ignore(1, b'\n');
        assert_eq!(s.get(), Some(b'b'));
        s.ignore(MAX_LINE, b'\n');
        assert_eq!(s.get(), Some(b'c'));
        s.unget();
        assert_eq!(s.get(), Some(b'c'));
        s.ignore(MAX_LINE, b'\n');
        assert!(s.eof() && !s.fail());
        assert_eq!(s.get(), None);
        // the delimiter is not consumed once `n` bytes were skipped
        let mut u = ascii("yyy\n");
        u.ignore(3, b'\n');
        assert_eq!((u.position(), u.good()), (3, true));
        let mut t = ascii("z");
        t.unget();
        assert!(t.state().contains(IoState::BAD));
    }
}
