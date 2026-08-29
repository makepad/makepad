//! mpterm's VT core: a clean Rust port of Ghostty's terminal subsystem.
//!
//! Ported from github.com/ghostty-org/ghostty (`src/terminal/`, `src/input/`),
//! MIT License, Copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors.
//! The UTF-8 decoder additionally derives from Bjoern Hoehrmann's DFA decoder
//! (http://bjoern.hoehrmann.de/utf-8/decoder/dfa, MIT).
//!
//! Layering (bottom to top), mirroring Ghostty:
//!
//!   utf8      DFA UTF-8 decoder (ground-state text path)
//!   parser    vt100.net DEC ANSI state machine -> Action (CSI/ESC/OSC/DCS/APC)
//!   osc       OSC command parser, driven byte-wise by `parser`
//!   sgr       SGR (ESC [ ... m) attribute parser incl. colon subparams
//!   modes     DEC/ANSI mode registry with save/restore + DECRQM reports
//!   charsets  G0..G3 slots, DEC Special Graphics
//!   tabstops  tab stop bitmap
//!   unicode   codepoint width + grapheme break (mode 2027)
//!   page      cell/row storage with scrollback
//!   screen    cursor + page view (primary/alternate), selection
//!   terminal  the control functions (CSI/ESC/OSC semantics)
//!   stream    bytes -> utf8/parser -> terminal dispatch
//!   key_encode / mouse_encode   input -> PTY byte encodings
//!
//! Everything in this module is pure: no Makepad, no PTY, no I/O. The
//! `Terminal` consumes bytes and accumulates reply bytes for the caller to
//! write back to the PTY.

pub mod charsets;
pub mod color;
pub mod key_encode;
pub mod modes;
pub mod mouse_encode;
pub mod osc;
pub mod page;
pub mod parser;
pub mod screen;
pub mod sgr;
pub mod stream;
pub mod style;
pub mod tabstops;
pub mod terminal;
pub mod unicode;
pub mod utf8;

pub use color::{Palette, Rgb};
pub use modes::{Mode, ModeState};
pub use page::{Cell, CellContent, Row};
pub use screen::{Cursor, CursorStyle, Screen};
pub use style::{Style, Underline};
pub use terminal::Terminal;
