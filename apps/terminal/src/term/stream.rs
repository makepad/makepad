//! Byte stream -> terminal. Port of ghostty `src/terminal/stream.zig`
//! (scalar path; the SIMD fast paths are not ported).

use crate::term::parser::{Parser, State};
use crate::term::terminal::Terminal;
use crate::term::utf8::Utf8Decoder;

pub struct Stream {
    pub parser: Parser,
    pub utf8: Utf8Decoder,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            utf8: Utf8Decoder::new(),
        }
    }

    pub fn process(&mut self, bytes: &[u8], terminal: &mut Terminal) {
        for &b in bytes {
            self.next(b, terminal);
        }
    }

    #[inline]
    pub fn next(&mut self, byte: u8, terminal: &mut Terminal) {
        if self.parser.state == State::Ground {
            self.next_utf8(byte, terminal);
        } else {
            self.next_non_utf8(byte, terminal);
        }
    }

    #[inline]
    fn next_utf8(&mut self, byte: u8, terminal: &mut Terminal) {
        let (cp, consumed) = self.utf8.next(byte);
        if let Some(cp) = cp {
            self.handle_codepoint(cp, terminal);
        }
        if !consumed {
            let (cp, consumed2) = self.utf8.next(byte);
            debug_assert!(consumed2);
            if let Some(cp) = cp {
                self.handle_codepoint(cp, terminal);
            }
        }
    }

    /// A decoded codepoint in ground state: C0 executes, ESC enters the
    /// parser, UTF-8-decoded C1 is ignored (xterm behavior), the rest
    /// prints.
    #[inline]
    fn handle_codepoint(&mut self, cp: u32, terminal: &mut Terminal) {
        if (cp & !0x9f) == 0 {
            if cp == 0x1b {
                self.parser.state = State::Escape;
                self.parser.clear();
                return;
            }
            if cp > 0x1f {
                // C1 via UTF-8: ignore.
                return;
            }
            terminal.execute(cp as u8);
            return;
        }
        terminal.print(cp);
    }

    fn next_non_utf8(&mut self, byte: u8, terminal: &mut Terminal) {
        debug_assert!(self.parser.state != State::Ground);
        for action in self.parser.next(byte).into_iter().flatten() {
            terminal.dispatch(action);
        }
    }
}
