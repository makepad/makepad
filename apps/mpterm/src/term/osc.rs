//! OSC (Operating System Command) parser.
//!
//! Port of ghostty `src/terminal/osc.zig` and the per-command parsers under
//! `src/terminal/osc/parsers/` (MIT, Copyright (c) 2024 Mitchell Hashimoto,
//! Ghostty contributors).
//!
//! Ghostty runs a byte-wise state machine over the numeric OSC prefix and then
//! "captures" the trailing bytes into a fixed 2048-byte buffer (or an
//! allocating one for OSC 52/66/72/99). We instead buffer the whole string
//! (prefix included) into one bounded `Vec` and split it at `end`. The
//! observable behavior is the same:
//!
//!   * an unknown/invalid prefix yields `None` (ghostty goes to `.invalid`
//!     and discards the rest of the string),
//!   * data past the bound makes the whole command invalid — ghostty's
//!     capture write fails, the state goes `.invalid` and `end` returns null.
//!     There is no command for which ghostty tolerates a truncated capture,
//!     so overflow is always `None` here too.
//!
//! Commands supported (matching ghostty's behavior, including the edge cases
//! covered by its test suite):
//!   0   set icon + window title            -> ChangeWindowTitle
//!   1   set icon name                      -> ChangeWindowIcon
//!   2   set window title                   -> ChangeWindowTitle
//!   4   set/query palette colors, multi-pair `4;i;spec[;i;spec...]`,
//!       `?` spec = query                    -> Colors
//!   7   report pwd (file:// URL)           -> ReportPwd
//!   8   hyperlink `8;params;uri` (id= key) -> Hyperlink / HyperlinkEnd
//!   9   iTerm2 growl notification          -> ShowDesktopNotification
//!   10/11/12 set/query default fg/bg/cursor color (multi-value form sets
//!       fg then bg then cursor like xterm) -> Colors
//!   104 reset palette color(s) (empty = all)-> Colors (Reset ops)
//!   110/111/112 reset default fg/bg/cursor -> Colors
//!   52  clipboard set/query `52;c;base64` or `52;c;?` -> ClipboardContents
//!   133 shell integration A/B/C/D          -> Prompt*/EndOf*
//!   777 `777;notify;title;body`            -> ShowDesktopNotification
//! Everything else returns `None` from `end`.
//!
//! Deviations from ghostty, all forced by this crate's command surface (see
//! the individual parse functions for details):
//!   * OSC 5/13-19/105/113-119 (xterm "special" colors: bold, underline,
//!     blink, reverse, italic) are not represented, so they are rejected, and
//!     OSC 4/104 entries with index >= 256 (which address those same special
//!     colors) are skipped rather than reported.
//!   * The dynamic-color chain used by the xterm multi-value form stops after
//!     cursor (ghostty continues into pointer/tektronix/highlight colors).
//!   * OSC 9 is always the iTerm2 notification form; the ConEmu `9;N;...`
//!     extensions are not implemented (ghostty falls back to a notification
//!     for malformed ConEmu payloads anyway, which is what we always do).
//!   * OSC 133 L/N/P/I (fresh_line, new_command, prompt_start,
//!     end_prompt_start_input_terminate_eol) have no command variant and are
//!     rejected; A/B/C/D behave as ghostty does.
//!   * Payload bytes are decoded lossily as UTF-8 into `String` (ghostty keeps
//!     raw bytes and defers latin1/utf8 title decoding to the app layer).
//!
//! `end(terminator)` receives the byte that terminated the string (0x07 BEL
//! or 0x1b for ESC-backslash ST, 0x9c for C1 ST, `None` on abort) so query
//! replies can echo the same terminator kind (`OscTerminator`).

use crate::term::color::{parse_color_spec, Rgb};

pub const MAX_OSC_DATA: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OscTerminator {
    #[default]
    St,
    Bel,
}

impl OscTerminator {
    pub fn from_byte(byte: Option<u8>) -> Self {
        match byte {
            Some(0x07) => OscTerminator::Bel,
            _ => OscTerminator::St,
        }
    }

    /// The bytes to end a reply with, mirroring the request's terminator.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            OscTerminator::St => b"\x1b\\",
            OscTerminator::Bel => b"\x07",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorKind {
    Palette(u8),
    Foreground,
    Background,
    Cursor,
}

impl ColorKind {
    /// The next dynamic color in the xterm multi-value chain (OSC 10 with
    /// several specs sets foreground, then background, then cursor). Ghostty
    /// continues into pointer/tektronix/highlight colors; we stop at cursor
    /// because those have no representation here.
    fn next_dynamic(self) -> Option<ColorKind> {
        match self {
            ColorKind::Foreground => Some(ColorKind::Background),
            ColorKind::Background => Some(ColorKind::Cursor),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColorOp {
    Set(ColorKind, Rgb),
    Query(ColorKind),
    Reset(ColorKind),
}

#[derive(Clone, Debug, PartialEq)]
pub enum OscCommand {
    ChangeWindowTitle(String),
    ChangeWindowIcon(String),
    Colors {
        ops: Vec<ColorOp>,
        terminator: OscTerminator,
    },
    /// OSC 8 with a non-empty URI starts a hyperlink; empty ends it.
    Hyperlink {
        id: Option<String>,
        uri: String,
    },
    HyperlinkEnd,
    /// OSC 52. `kind` is the clipboard selection byte ('c', 'p', 's', ...).
    /// `data` is the raw base64 payload, or "?" for a query.
    ClipboardContents {
        kind: u8,
        data: String,
        terminator: OscTerminator,
    },
    ReportPwd {
        url: String,
    },
    /// OSC 133;A
    PromptStart {
        aid: Option<String>,
        redraw: bool,
    },
    /// OSC 133;B
    PromptEnd,
    /// OSC 133;C
    EndOfInput,
    /// OSC 133;D[;exit_code]
    EndOfCommand {
        exit_code: Option<u8>,
    },
    ShowDesktopNotification {
        title: String,
        body: String,
    },
}

pub struct OscParser {
    /// The raw OSC string, numeric prefix included, bounded by MAX_OSC_DATA.
    buf: Vec<u8>,
    /// Set once a byte had to be dropped; the command is then invalid, which
    /// is what ghostty does when a capture write fails.
    overflow: bool,
}

impl Default for OscParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OscParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(256),
            overflow: false,
        }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.overflow = false;
    }

    /// Feed one payload byte (between `ESC ]` / 0x9d and the terminator).
    pub fn next(&mut self, byte: u8) {
        if self.buf.len() >= MAX_OSC_DATA {
            self.overflow = true;
            return;
        }
        self.buf.push(byte);
    }

    /// The string terminated with `terminator` (0x07, 0x1b of ESC-\, 0x9c,
    /// or None on abort/CAN). Returns the parsed command, if valid.
    ///
    /// The parser is reset and ready for the next string when this returns.
    pub fn end(&mut self, terminator: Option<u8>) -> Option<OscCommand> {
        // Move the buffer out so we can parse while resetting; the allocation
        // is put back (cleared) so repeated commands don't re-allocate.
        let buf = std::mem::take(&mut self.buf);
        let overflow = self.overflow;
        let cmd = if overflow {
            None
        } else {
            parse(&buf, OscTerminator::from_byte(terminator))
        };
        self.buf = buf;
        self.reset();
        cmd
    }
}

/// Decode payload bytes as UTF-8, replacing invalid sequences. Ghostty keeps
/// raw bytes; every payload we surface is text (title, URI, base64, options)
/// so we decode once here and parse with `&str` slicing below. All the
/// delimiters we split on are ASCII, so byte offsets stay char boundaries.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Split `<code>[;<payload>]` and dispatch. This stands in for ghostty's
/// prefix state machine: a code we don't recognize (or trailing junk in the
/// code, like `01` or `0x`) is invalid, exactly as a bad state transition is.
fn parse(data: &[u8], terminator: OscTerminator) -> Option<OscCommand> {
    let (code, payload) = match data.iter().position(|&b| b == b';') {
        Some(i) => (&data[..i], Some(&data[i + 1..])),
        None => (data, None),
    };
    let code = std::str::from_utf8(code).ok()?;

    match code {
        // OSC 0 sets both the icon name and the window title; we only carry
        // the title. OSC 2 is the title alone. Both require the `;`.
        "0" | "2" => Some(OscCommand::ChangeWindowTitle(lossy(payload?))),
        "1" => Some(OscCommand::ChangeWindowIcon(lossy(payload?))),
        "7" => Some(OscCommand::ReportPwd {
            url: lossy(payload?),
        }),
        "8" => parse_hyperlink(&lossy(payload?)),
        // OSC 9: iTerm2 notification, body only.
        "9" => Some(OscCommand::ShowDesktopNotification {
            title: String::new(),
            body: lossy(payload?),
        }),
        "52" => parse_clipboard(&lossy(payload?), terminator),
        "133" => parse_semantic_prompt(&lossy(payload?)),
        "777" => parse_rxvt_extension(&lossy(payload?)),
        // Color operations accept a missing payload: `OSC 104` alone resets
        // the whole palette and `OSC 110` alone resets the foreground.
        "4" | "10" | "11" | "12" | "104" | "110" | "111" | "112" => {
            let body = payload.map(lossy).unwrap_or_default();
            Some(OscCommand::Colors {
                ops: parse_color_ops(code, &body),
                terminator,
            })
        }
        _ => None,
    }
}

/// OSC 4/10/11/12/104/110/111/112. Ghostty's `parsers/color.zig`.
fn parse_color_ops(code: &str, body: &str) -> Vec<ColorOp> {
    match code {
        "4" => parse_get_set_palette(body),
        "10" => parse_get_set_dynamic(body, ColorKind::Foreground),
        "11" => parse_get_set_dynamic(body, ColorKind::Background),
        "12" => parse_get_set_dynamic(body, ColorKind::Cursor),
        "104" => parse_reset_palette(body),
        "110" => parse_reset_dynamic(body, ColorKind::Foreground),
        "111" => parse_reset_dynamic(body, ColorKind::Background),
        "112" => parse_reset_dynamic(body, ColorKind::Cursor),
        _ => Vec::new(),
    }
}

/// OSC 4: repeated `index;spec` pairs. On *any* error we return what we
/// accumulated so far and stop, which is what xterm does (misc.c
/// ChangeAnsiColorRequest) and what ghostty copies.
fn parse_get_set_palette(body: &str) -> Vec<ColorOp> {
    let mut ops = Vec::new();
    // Ghostty tokenizes on ';', which skips empty tokens.
    let mut it = body.split(';').filter(|s| !s.is_empty());
    loop {
        // A pair is required; a dangling index ends the list.
        let (Some(index_str), Some(spec)) = (it.next(), it.next()) else {
            return ops;
        };
        // Index must be numeric and fit ghostty's u9 (palette + specials).
        let Ok(index) = index_str.parse::<u16>() else {
            return ops;
        };
        if index > 511 {
            return ops;
        }
        if index > 255 {
            // 256+ addresses the xterm special colors (bold, underline,
            // blink, reverse, italic) which we have no ColorKind for. Ghostty
            // reports them; we skip the pair and keep going.
            continue;
        }
        let kind = ColorKind::Palette(index as u8);
        if spec == "?" {
            ops.push(ColorOp::Query(kind));
            continue;
        }
        let Some(rgb) = parse_color_spec(spec) else {
            return ops;
        };
        ops.push(ColorOp::Set(kind, rgb));
    }
}

/// OSC 104: reset palette entries. Unlike OSC 4, a bad entry is skipped and
/// parsing continues (ghostty follows kitty here, not xterm). With no entries
/// at all the whole palette is reset -- ghostty emits one `reset_palette`
/// request, we expand it to the 256 individual resets our ColorOp can carry.
fn parse_reset_palette(body: &str) -> Vec<ColorOp> {
    let mut ops = Vec::new();
    for tok in body.split(';').filter(|s| !s.is_empty()) {
        let Ok(index) = tok.parse::<u16>() else {
            continue;
        };
        // > 255 is a special color (or out of range): skipped, see above.
        if index > 255 {
            continue;
        }
        ops.push(ColorOp::Reset(ColorKind::Palette(index as u8)));
    }
    if ops.is_empty() {
        return (0..=255)
            .map(|i| ColorOp::Reset(ColorKind::Palette(i)))
            .collect();
    }
    ops
}

/// OSC 10-12: get/set dynamic colors. Each successive value applies to the
/// next color in the chain, so `10;fg;bg;cursor` sets all three in one go.
/// As with OSC 4, any error returns the accumulated results.
fn parse_get_set_dynamic(body: &str, start: ColorKind) -> Vec<ColorOp> {
    let mut ops = Vec::new();
    let mut kind = start;
    for tok in body.split(';').filter(|s| !s.is_empty()) {
        if tok == "?" {
            ops.push(ColorOp::Query(kind));
        } else {
            let Some(rgb) = parse_color_spec(tok) else {
                return ops;
            };
            ops.push(ColorOp::Set(kind, rgb));
        }
        let Some(next) = kind.next_dynamic() else {
            return ops;
        };
        kind = next;
    }
    ops
}

/// OSC 110-112: reset a dynamic color. Any parameter at all invalidates the
/// request (xterm allows a bare trailing `;` but nothing else, not even
/// whitespace).
fn parse_reset_dynamic(body: &str, kind: ColorKind) -> Vec<ColorOp> {
    if body.split(';').any(|s| !s.is_empty()) {
        return Vec::new();
    }
    vec![ColorOp::Reset(kind)]
}

/// OSC 8: `params;uri`, params being `key=value` pairs separated by `:`.
/// Only `id` is standardized; unknown keys are ignored and a key without a
/// `=` stops option parsing (ghostty breaks out of its kv loop there).
fn parse_hyperlink(payload: &str) -> Option<OscCommand> {
    let sep = payload.find(';')?;
    let (params, uri) = (&payload[..sep], &payload[sep + 1..]);

    let mut id: Option<String> = None;
    for kv in params.split(':') {
        let Some(eq) = kv.find('=') else {
            break;
        };
        let (key, value) = (&kv[..eq], &kv[eq + 1..]);
        if key == "id" && !value.is_empty() {
            id = Some(value.to_string());
        }
    }

    if uri.is_empty() {
        // `OSC 8 ; ; ST` closes a link. An id with no URI is malformed.
        if id.is_some() {
            return None;
        }
        return Some(OscCommand::HyperlinkEnd);
    }

    Some(OscCommand::Hyperlink {
        id,
        uri: uri.to_string(),
    })
}

/// OSC 52: `kind;data`. An empty kind means the clipboard ('c'). Ghostty
/// requires the kind to be exactly one byte; we keep its first byte and
/// ignore the rest (xterm allows a set of selection characters).
fn parse_clipboard(payload: &str, terminator: OscTerminator) -> Option<OscCommand> {
    let sep = payload.find(';')?;
    let kind = payload.as_bytes().first().copied().filter(|_| sep > 0);
    Some(OscCommand::ClipboardContents {
        kind: kind.unwrap_or(b'c'),
        data: payload[sep + 1..].to_string(),
        terminator,
    })
}

/// OSC 133: semantic prompts. The command letter is followed either by
/// nothing or by `;` and an option string.
fn parse_semantic_prompt(payload: &str) -> Option<OscCommand> {
    let bytes = payload.as_bytes();
    let kind = *bytes.first()?;
    let options = if bytes.len() == 1 {
        ""
    } else {
        if bytes[1] != b';' {
            return None;
        }
        &payload[2..]
    };

    match kind {
        b'A' => Some(OscCommand::PromptStart {
            aid: read_option(options, "aid").map(str::to_string),
            // Kitty's `redraw` extension: only an explicit 0 disables it,
            // anything else (including ghostty's `last`) leaves it on.
            redraw: read_option(options, "redraw") != Some("0"),
        }),
        b'B' => Some(OscCommand::PromptEnd),
        b'C' => Some(OscCommand::EndOfInput),
        b'D' => Some(OscCommand::EndOfCommand {
            exit_code: read_exit_code(options),
        }),
        // L / N / P / I have no command variant here.
        _ => None,
    }
}

/// Read a `key=value` option out of an OSC 133 option string. Options are
/// `;`-separated; the first match wins, malformed entries are skipped, and an
/// entry with no `=` at the very end stops the scan (ghostty's Option.read).
fn read_option<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let mut remaining = raw;
    while !remaining.is_empty() {
        let len = remaining.find(';').unwrap_or(remaining.len());
        let full = &remaining[..len];
        if let Some(eq) = full.find('=') {
            if &full[..eq] == key {
                return Some(&full[eq + 1..]);
            }
        }
        if len < remaining.len() {
            remaining = &remaining[len + 1..];
            continue;
        }
        break;
    }
    None
}

/// OSC 133;D carries the exit code as the first (unkeyed) option field.
fn read_exit_code(options: &str) -> Option<u8> {
    if options.is_empty() {
        return None;
    }
    let first = options.split(';').next().unwrap_or("");
    // Ghostty parses an i32; we only surface codes that fit a u8.
    first.parse::<i32>().ok().and_then(|v| u8::try_from(v).ok())
}

/// OSC 777: the rxvt extension protocol. `notify` is the only extension we
/// (and ghostty) implement.
fn parse_rxvt_extension(payload: &str) -> Option<OscCommand> {
    let k = payload.find(';')?;
    if &payload[..k] != "notify" {
        return None;
    }
    let rest = &payload[k + 1..];
    let t = rest.find(';')?;
    Some(OscCommand::ShowDesktopNotification {
        title: rest[..t].to_string(),
        body: rest[t + 1..].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_bytes(input: &[u8], terminator: Option<u8>) -> Option<OscCommand> {
        let mut p = OscParser::new();
        for b in input {
            p.next(*b);
        }
        p.end(terminator)
    }

    fn parse_str(input: &str, terminator: Option<u8>) -> Option<OscCommand> {
        parse_bytes(input.as_bytes(), terminator)
    }

    fn osc(input: &str) -> Option<OscCommand> {
        parse_str(input, None)
    }

    fn colors(input: &str) -> Vec<ColorOp> {
        match osc(input) {
            Some(OscCommand::Colors { ops, .. }) => ops,
            other => panic!("expected Colors, got {other:?}"),
        }
    }

    const RED: Rgb = Rgb {
        r: 255,
        g: 0,
        b: 0,
    };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    // ---- OSC 0/1/2: titles -------------------------------------------

    #[test]
    fn osc0_change_window_title() {
        assert_eq!(
            osc("0;ab"),
            Some(OscCommand::ChangeWindowTitle("ab".into()))
        );
    }

    #[test]
    fn osc0_longer_than_buffer() {
        let mut input = String::from("0;");
        input.push_str(&"a".repeat(MAX_OSC_DATA + 2));
        assert_eq!(osc(&input), None);
    }

    #[test]
    fn osc0_one_shorter_than_buffer() {
        let title = "a".repeat(MAX_OSC_DATA - 3);
        let cmd = osc(&format!("0;{title}"));
        assert_eq!(cmd, Some(OscCommand::ChangeWindowTitle(title)));
    }

    #[test]
    fn osc0_exactly_at_buffer_length() {
        // The bound covers the whole string, prefix included: 2 bytes of
        // "0;" plus MAX_OSC_DATA-2 of title is the largest accepted title.
        let title = "a".repeat(MAX_OSC_DATA - 2);
        assert_eq!(
            osc(&format!("0;{title}")),
            Some(OscCommand::ChangeWindowTitle(title))
        );
        let too_long = "a".repeat(MAX_OSC_DATA - 1);
        assert_eq!(osc(&format!("0;{too_long}")), None);
    }

    #[test]
    fn osc2_change_window_title() {
        assert_eq!(
            osc("2;ab"),
            Some(OscCommand::ChangeWindowTitle("ab".into()))
        );
    }

    #[test]
    fn osc2_change_window_title_with_utf8() {
        // EM DASH U+2014, then HYPHEN U+2010 whose 0x90 tail collides with a
        // C1 control byte.
        let input = b"2;\xE2\x80\x94 \xE2\x80\x90";
        assert_eq!(
            parse_bytes(input, None),
            Some(OscCommand::ChangeWindowTitle("— ‐".into()))
        );
    }

    #[test]
    fn osc2_change_window_title_empty() {
        assert_eq!(osc("2;"), Some(OscCommand::ChangeWindowTitle(String::new())));
    }

    #[test]
    fn osc1_change_window_icon() {
        assert_eq!(osc("1;ab"), Some(OscCommand::ChangeWindowIcon("ab".into())));
    }

    #[test]
    fn title_without_payload_is_invalid() {
        // No `;` means the state machine never starts capturing.
        assert_eq!(osc("0"), None);
        assert_eq!(osc("1"), None);
        assert_eq!(osc("2"), None);
    }

    // ---- OSC 4: palette get/set --------------------------------------

    #[test]
    fn osc4_set_and_query_every_index() {
        for idx in 0..=255u8 {
            assert_eq!(
                colors(&format!("4;{idx};red")),
                vec![ColorOp::Set(ColorKind::Palette(idx), RED)]
            );
            assert_eq!(
                colors(&format!("4;{idx};?")),
                vec![ColorOp::Query(ColorKind::Palette(idx))]
            );
            // Trailing junk produces the results up to that point.
            assert_eq!(
                colors(&format!("4;{idx};red;")),
                vec![ColorOp::Set(ColorKind::Palette(idx), RED)]
            );
            // xterm rejects trailing whitespace in a spec; kitty and ghostty
            // allow it, so we do too.
            assert_eq!(
                colors(&format!("4;{idx};red ")),
                vec![ColorOp::Set(ColorKind::Palette(idx), RED)]
            );
        }
    }

    #[test]
    fn osc4_multiple_requests() {
        assert_eq!(
            colors("4;0;red;1;blue"),
            vec![
                ColorOp::Set(ColorKind::Palette(0), RED),
                ColorOp::Set(ColorKind::Palette(1), BLUE),
            ]
        );
        // The same index twice is kept twice; the last one wins downstream.
        assert_eq!(
            colors("4;0;red;0;blue"),
            vec![
                ColorOp::Set(ColorKind::Palette(0), RED),
                ColorOp::Set(ColorKind::Palette(0), BLUE),
            ]
        );
    }

    #[test]
    fn osc4_rgb_spec() {
        assert_eq!(
            colors("4;1;rgb:ff/00/00"),
            vec![ColorOp::Set(ColorKind::Palette(1), RED)]
        );
        assert_eq!(
            colors("4;1;#ff0000"),
            vec![ColorOp::Set(ColorKind::Palette(1), RED)]
        );
    }

    #[test]
    fn osc4_empty_param() {
        // Ghostty's `4;;` test asserts null only because that parser needs an
        // allocator it wasn't given; with one it yields an empty request list.
        assert_eq!(colors("4;;"), vec![]);
    }

    #[test]
    fn osc4_stops_at_bad_data() {
        // Unparseable index, bad spec, and a dangling index all stop parsing
        // but keep what came before.
        assert_eq!(
            colors("4;0;red;bogus;blue"),
            vec![ColorOp::Set(ColorKind::Palette(0), RED)]
        );
        assert_eq!(
            colors("4;0;red;1;notacolor"),
            vec![ColorOp::Set(ColorKind::Palette(0), RED)]
        );
        assert_eq!(
            colors("4;0;red;1"),
            vec![ColorOp::Set(ColorKind::Palette(0), RED)]
        );
        assert_eq!(colors("4;0;notacolor"), vec![]);
        assert_eq!(colors("4;999;red"), vec![]);
    }

    #[test]
    fn osc4_special_index_is_skipped() {
        // 256.. addresses xterm's special colors, which we can't represent:
        // the pair is skipped and the rest of the request still parses.
        assert_eq!(
            colors("4;256;red;1;blue"),
            vec![ColorOp::Set(ColorKind::Palette(1), BLUE)]
        );
    }

    #[test]
    fn osc4_no_payload() {
        assert_eq!(colors("4"), vec![]);
    }

    // ---- OSC 104: palette reset --------------------------------------

    #[test]
    fn osc104_every_index() {
        for idx in 0..=255u8 {
            assert_eq!(
                colors(&format!("104;{idx}")),
                vec![ColorOp::Reset(ColorKind::Palette(idx))]
            );
        }
    }

    #[test]
    fn osc104_empty_index() {
        assert_eq!(
            colors("104;0;;1"),
            vec![
                ColorOp::Reset(ColorKind::Palette(0)),
                ColorOp::Reset(ColorKind::Palette(1)),
            ]
        );
    }

    #[test]
    fn osc104_invalid_index_is_skipped() {
        assert_eq!(
            colors("104;ffff;1"),
            vec![ColorOp::Reset(ColorKind::Palette(1))]
        );
        assert_eq!(
            colors("104;300;1"),
            vec![ColorOp::Reset(ColorKind::Palette(1))]
        );
    }

    #[test]
    fn osc104_reset_all() {
        for input in ["104", "104;"] {
            let ops = colors(input);
            assert_eq!(ops.len(), 256);
            assert_eq!(ops[0], ColorOp::Reset(ColorKind::Palette(0)));
            assert_eq!(ops[255], ColorOp::Reset(ColorKind::Palette(255)));
        }
    }

    // ---- OSC 10/11/12: dynamic colors --------------------------------

    #[test]
    fn osc10_11_12_set() {
        for (code, kind) in [
            ("10", ColorKind::Foreground),
            ("11", ColorKind::Background),
            ("12", ColorKind::Cursor),
        ] {
            assert_eq!(colors(&format!("{code};red")), vec![ColorOp::Set(kind, RED)]);
        }
    }

    #[test]
    fn osc10_11_12_query() {
        for (code, kind) in [
            ("10", ColorKind::Foreground),
            ("11", ColorKind::Background),
            ("12", ColorKind::Cursor),
        ] {
            assert_eq!(colors(&format!("{code};?")), vec![ColorOp::Query(kind)]);
        }
    }

    #[test]
    fn osc11_query_with_bel_terminator() {
        let cmd = parse_str("11;?", Some(0x07));
        assert_eq!(
            cmd,
            Some(OscCommand::Colors {
                ops: vec![ColorOp::Query(ColorKind::Background)],
                terminator: OscTerminator::Bel,
            })
        );
    }

    #[test]
    fn osc11_query_with_st_terminator() {
        let cmd = parse_str("11;?", Some(0x1b));
        assert_eq!(
            cmd,
            Some(OscCommand::Colors {
                ops: vec![ColorOp::Query(ColorKind::Background)],
                terminator: OscTerminator::St,
            })
        );
        // C1 ST and an aborted string are both "ST" as far as replies go.
        assert!(matches!(
            parse_str("11;?", Some(0x9c)),
            Some(OscCommand::Colors {
                terminator: OscTerminator::St,
                ..
            })
        ));
    }

    #[test]
    fn osc10_multiple_values_walks_the_chain() {
        // xterm's multi-value form: each spec applies to the next color.
        assert_eq!(
            colors("10;red;blue"),
            vec![
                ColorOp::Set(ColorKind::Foreground, RED),
                ColorOp::Set(ColorKind::Background, BLUE),
            ]
        );
        assert_eq!(
            colors("11;red;blue"),
            vec![
                ColorOp::Set(ColorKind::Background, RED),
                ColorOp::Set(ColorKind::Cursor, BLUE),
            ]
        );
        assert_eq!(
            colors("10;red;blue;white"),
            vec![
                ColorOp::Set(ColorKind::Foreground, RED),
                ColorOp::Set(ColorKind::Background, BLUE),
                ColorOp::Set(ColorKind::Cursor, Rgb::new(255, 255, 255)),
            ]
        );
        // Past cursor the chain ends here (ghostty continues into colors we
        // don't model), so the fourth value is dropped.
        assert_eq!(colors("10;red;blue;white;black").len(), 3);
    }

    #[test]
    fn osc10_mixed_query_and_set() {
        assert_eq!(
            colors("10;?;blue"),
            vec![
                ColorOp::Query(ColorKind::Foreground),
                ColorOp::Set(ColorKind::Background, BLUE),
            ]
        );
    }

    #[test]
    fn osc10_bad_spec_stops() {
        assert_eq!(colors("10;notacolor"), vec![]);
        assert_eq!(
            colors("10;red;notacolor"),
            vec![ColorOp::Set(ColorKind::Foreground, RED)]
        );
    }

    // ---- OSC 110/111/112: dynamic reset ------------------------------

    #[test]
    fn osc110_111_112_reset() {
        for (code, kind) in [
            ("110", ColorKind::Foreground),
            ("111", ColorKind::Background),
            ("112", ColorKind::Cursor),
        ] {
            assert_eq!(colors(code), vec![ColorOp::Reset(kind)]);
            // xterm allows a bare trailing `;`.
            assert_eq!(colors(&format!("{code};")), vec![ColorOp::Reset(kind)]);
            // ...but nothing else, not even whitespace.
            assert_eq!(colors(&format!("{code}; ")), vec![]);
            assert_eq!(colors(&format!("{code};red")), vec![]);
        }
    }

    // ---- OSC 7: pwd --------------------------------------------------

    #[test]
    fn osc7_report_pwd() {
        assert_eq!(
            osc("7;file:///tmp/example"),
            Some(OscCommand::ReportPwd {
                url: "file:///tmp/example".into()
            })
        );
    }

    #[test]
    fn osc7_report_pwd_empty() {
        assert_eq!(
            osc("7;"),
            Some(OscCommand::ReportPwd { url: String::new() })
        );
    }

    // ---- OSC 8: hyperlinks -------------------------------------------

    #[test]
    fn osc8_hyperlink() {
        assert_eq!(
            parse_str("8;;http://example.com", Some(0x1b)),
            Some(OscCommand::Hyperlink {
                id: None,
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_id() {
        assert_eq!(
            osc("8;id=foo;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: Some("foo".into()),
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_empty_id() {
        assert_eq!(
            osc("8;id=;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: None,
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_incomplete_key() {
        assert_eq!(
            osc("8;id;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: None,
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_empty_key() {
        assert_eq!(
            osc("8;=value;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: None,
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_empty_key_and_id() {
        assert_eq!(
            osc("8;=value:id=foo;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: Some("foo".into()),
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_unknown_key_ignored() {
        assert_eq!(
            osc("8;foo=bar:id=x;http://example.com"),
            Some(OscCommand::Hyperlink {
                id: Some("x".into()),
                uri: "http://example.com".into()
            })
        );
    }

    #[test]
    fn osc8_hyperlink_with_empty_uri() {
        // An id with no URI is malformed.
        assert_eq!(osc("8;id=foo;"), None);
    }

    #[test]
    fn osc8_hyperlink_end() {
        assert_eq!(osc("8;;"), Some(OscCommand::HyperlinkEnd));
    }

    #[test]
    fn osc8_without_uri_separator() {
        assert_eq!(osc("8;id=foo"), None);
        assert_eq!(osc("8"), None);
    }

    // ---- OSC 52: clipboard -------------------------------------------

    #[test]
    fn osc52_get_set_clipboard() {
        assert_eq!(
            osc("52;s;?"),
            Some(OscCommand::ClipboardContents {
                kind: b's',
                data: "?".into(),
                terminator: OscTerminator::St,
            })
        );
    }

    #[test]
    fn osc52_get_clipboard_with_bel_terminator() {
        assert_eq!(
            parse_str("52;c;?", Some(0x07)),
            Some(OscCommand::ClipboardContents {
                kind: b'c',
                data: "?".into(),
                terminator: OscTerminator::Bel,
            })
        );
    }

    #[test]
    fn osc52_optional_kind_defaults_to_clipboard() {
        assert_eq!(
            osc("52;;?"),
            Some(OscCommand::ClipboardContents {
                kind: b'c',
                data: "?".into(),
                terminator: OscTerminator::St,
            })
        );
    }

    #[test]
    fn osc52_clear_clipboard() {
        assert_eq!(
            osc("52;;"),
            Some(OscCommand::ClipboardContents {
                kind: b'c',
                data: String::new(),
                terminator: OscTerminator::St,
            })
        );
    }

    #[test]
    fn osc52_set_base64_payload() {
        assert_eq!(
            osc("52;c;aGVsbG8="),
            Some(OscCommand::ClipboardContents {
                kind: b'c',
                data: "aGVsbG8=".into(),
                terminator: OscTerminator::St,
            })
        );
    }

    #[test]
    fn osc52_multi_char_kind_keeps_first_byte() {
        // xterm allows a set of selection characters; ghostty requires
        // exactly one. We keep the first, which agrees on all single-char
        // forms and is more permissive on the rest.
        assert_eq!(
            osc("52;pc;?"),
            Some(OscCommand::ClipboardContents {
                kind: b'p',
                data: "?".into(),
                terminator: OscTerminator::St,
            })
        );
    }

    #[test]
    fn osc52_missing_data_is_invalid() {
        assert_eq!(osc("52;"), None);
        assert_eq!(osc("52;c"), None);
        assert_eq!(osc("52"), None);
    }

    // ---- OSC 133: semantic prompts -----------------------------------

    #[test]
    fn osc133_prompt_start() {
        assert_eq!(
            osc("133;A"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_with_aid() {
        assert_eq!(
            osc("133;A;aid=14"),
            Some(OscCommand::PromptStart {
                aid: Some("14".into()),
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_with_equals_in_aid() {
        assert_eq!(
            osc("133;A;aid=a=b"),
            Some(OscCommand::PromptStart {
                aid: Some("a=b".into()),
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_with_trailing_semicolon() {
        assert_eq!(
            osc("133;A;"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_with_bare_key() {
        assert_eq!(
            osc("133;A;barekey"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_with_multiple_options() {
        assert_eq!(
            osc("133;A;aid=foo;cl=line"),
            Some(OscCommand::PromptStart {
                aid: Some("foo".into()),
                redraw: true
            })
        );
        // Order doesn't matter and unknown options are ignored.
        assert_eq!(
            osc("133;A;cl=line;aid=myaid;k=i"),
            Some(OscCommand::PromptStart {
                aid: Some("myaid".into()),
                redraw: true
            })
        );
    }

    #[test]
    fn osc133_prompt_start_redraw() {
        assert_eq!(
            osc("133;A;redraw=0"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: false
            })
        );
        assert_eq!(
            osc("133;A;redraw=1"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
        // ghostty's "last" extension and any invalid value keep redraw on.
        assert_eq!(
            osc("133;A;redraw=last"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
        assert_eq!(
            osc("133;A;redraw=bogus"),
            Some(OscCommand::PromptStart {
                aid: None,
                redraw: true
            })
        );
        assert_eq!(
            osc("133;A;aid=foo;redraw=0"),
            Some(OscCommand::PromptStart {
                aid: Some("foo".into()),
                redraw: false
            })
        );
    }

    #[test]
    fn osc133_prompt_start_extra_contents() {
        assert_eq!(osc("133;Aextra"), None);
    }

    #[test]
    fn osc133_prompt_end() {
        assert_eq!(osc("133;B"), Some(OscCommand::PromptEnd));
        assert_eq!(osc("133;B;aid=foo"), Some(OscCommand::PromptEnd));
        assert_eq!(osc("133;Bextra"), None);
    }

    #[test]
    fn osc133_end_of_input() {
        assert_eq!(osc("133;C"), Some(OscCommand::EndOfInput));
        assert_eq!(osc("133;C;aid=foo"), Some(OscCommand::EndOfInput));
        assert_eq!(osc("133;Cextra"), None);
    }

    #[test]
    fn osc133_end_of_command() {
        assert_eq!(
            osc("133;D"),
            Some(OscCommand::EndOfCommand { exit_code: None })
        );
        assert_eq!(
            osc("133;D;0"),
            Some(OscCommand::EndOfCommand { exit_code: Some(0) })
        );
        assert_eq!(
            osc("133;D;12;aid=foo"),
            Some(OscCommand::EndOfCommand {
                exit_code: Some(12)
            })
        );
        // No numeric first field means no exit code.
        assert_eq!(
            osc("133;D;aid=foo"),
            Some(OscCommand::EndOfCommand { exit_code: None })
        );
        assert_eq!(
            osc("133;D;"),
            Some(OscCommand::EndOfCommand { exit_code: None })
        );
        // Out of u8 range.
        assert_eq!(
            osc("133;D;-1"),
            Some(OscCommand::EndOfCommand { exit_code: None })
        );
        assert_eq!(osc("133;Dextra"), None);
    }

    #[test]
    fn osc133_unsupported_and_invalid_kinds() {
        // L/N/P/I are valid OSC 133 marks in ghostty but have no command
        // variant here.
        for input in ["133;L", "133;N", "133;P", "133;I", "133;Z", "133;"] {
            assert_eq!(osc(input), None, "input: {input}");
        }
        assert_eq!(osc("133"), None);
    }

    #[test]
    fn option_reader_matches_ghostty() {
        assert_eq!(read_option("aid=test123", "aid"), Some("test123"));
        assert_eq!(read_option("cl=line;aid=myaid;k=i", "aid"), Some("myaid"));
        assert_eq!(read_option("cl=line;k=i", "aid"), None);
        assert_eq!(read_option("aid=", "aid"), Some(""));
        assert_eq!(read_option("k=i;aid=last", "aid"), Some("last"));
        assert_eq!(read_option("aid=first;k=i", "aid"), Some("first"));
        assert_eq!(read_option("", "aid"), None);
        assert_eq!(read_option("aid", "aid"), None);
        assert_eq!(read_option(";;aid=value;;", "aid"), Some("value"));
    }

    // ---- OSC 9 / 777: notifications ----------------------------------

    #[test]
    fn osc9_show_desktop_notification() {
        assert_eq!(
            parse_str("9;Hello world", Some(0x1b)),
            Some(OscCommand::ShowDesktopNotification {
                title: String::new(),
                body: "Hello world".into()
            })
        );
        assert_eq!(
            osc("9;H"),
            Some(OscCommand::ShowDesktopNotification {
                title: String::new(),
                body: "H".into()
            })
        );
        assert_eq!(
            osc("9;"),
            Some(OscCommand::ShowDesktopNotification {
                title: String::new(),
                body: String::new()
            })
        );
        assert_eq!(osc("9"), None);
    }

    #[test]
    fn osc777_show_desktop_notification_with_title() {
        assert_eq!(
            parse_str("777;notify;Title;Body", Some(0x1b)),
            Some(OscCommand::ShowDesktopNotification {
                title: "Title".into(),
                body: "Body".into()
            })
        );
        // The body may itself contain semicolons.
        assert_eq!(
            osc("777;notify;Title;Body;more"),
            Some(OscCommand::ShowDesktopNotification {
                title: "Title".into(),
                body: "Body;more".into()
            })
        );
    }

    #[test]
    fn osc777_invalid() {
        // Unknown extension, and notify without a body field.
        assert_eq!(osc("777;bogus;Title;Body"), None);
        assert_eq!(osc("777;notify;Title"), None);
        assert_eq!(osc("777;notify"), None);
        assert_eq!(osc("777"), None);
    }

    // ---- invalid / garbage -------------------------------------------

    #[test]
    fn invalid_sequences() {
        for input in [
            "", "x", "abc", ";", ";foo", "3;foo", "5;red", "6;x", "22;text", "99;x", "1337;x",
            "00;title", "0x;title", "01;title", "104x;1", "-1;foo",
        ] {
            assert_eq!(osc(input), None, "input: {input}");
        }
    }

    #[test]
    fn invalid_utf8_in_code_is_rejected() {
        assert_eq!(parse_bytes(b"\xff\xfe;title", None), None);
    }

    #[test]
    fn invalid_utf8_in_payload_is_lossy() {
        let cmd = parse_bytes(b"0;a\xffb", None);
        assert_eq!(
            cmd,
            Some(OscCommand::ChangeWindowTitle("a\u{fffd}b".into()))
        );
    }

    // ---- parser lifecycle --------------------------------------------

    #[test]
    fn parser_is_reusable_after_end() {
        let mut p = OscParser::new();
        for b in b"0;first" {
            p.next(*b);
        }
        assert_eq!(
            p.end(Some(0x07)),
            Some(OscCommand::ChangeWindowTitle("first".into()))
        );
        for b in b"2;second" {
            p.next(*b);
        }
        assert_eq!(
            p.end(None),
            Some(OscCommand::ChangeWindowTitle("second".into()))
        );
    }

    #[test]
    fn parser_is_reusable_after_overflow() {
        let mut p = OscParser::new();
        for b in format!("0;{}", "a".repeat(MAX_OSC_DATA + 8)).into_bytes() {
            p.next(b);
        }
        assert_eq!(p.end(None), None);
        for b in b"2;ok" {
            p.next(*b);
        }
        assert_eq!(p.end(None), Some(OscCommand::ChangeWindowTitle("ok".into())));
    }

    #[test]
    fn parser_is_reusable_after_reset() {
        let mut p = OscParser::new();
        for b in b"0;abandoned" {
            p.next(*b);
        }
        p.reset();
        for b in b"2;ok" {
            p.next(*b);
        }
        assert_eq!(p.end(None), Some(OscCommand::ChangeWindowTitle("ok".into())));
    }

    #[test]
    fn empty_string_is_no_command() {
        let mut p = OscParser::new();
        assert_eq!(p.end(None), None);
    }

    #[test]
    fn terminator_from_byte() {
        assert_eq!(OscTerminator::from_byte(Some(0x07)), OscTerminator::Bel);
        assert_eq!(OscTerminator::from_byte(Some(0x1b)), OscTerminator::St);
        assert_eq!(OscTerminator::from_byte(Some(0x9c)), OscTerminator::St);
        assert_eq!(OscTerminator::from_byte(None), OscTerminator::St);
        assert_eq!(OscTerminator::Bel.bytes(), b"\x07");
        assert_eq!(OscTerminator::St.bytes(), b"\x1b\\");
    }
}
