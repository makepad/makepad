/// VT100 state machine parser.
/// Implements the state machine from https://vt100.net/emu/dec_ansi_parser
/// Pure: no side effects, no terminal state. Bytes in, actions out.

const MAX_PARAMS: usize = 16;
const MAX_INTERMEDIATES: usize = 2;
const MAX_OSC_LEN: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    #[allow(unused)]
    DcsParam,
    #[allow(unused)]
    DcsIntermediate,
    DcsPassthrough,
    OscString,
    SosPmApcString,
}

/// CSI parameters
#[derive(Clone, Debug)]
pub struct CsiParams {
    pub params: [u16; MAX_PARAMS],
    pub len: usize,
    pub intermediates: [u8; MAX_INTERMEDIATES],
    pub intermediates_len: usize,
    pub has_colon: bool,
}

impl CsiParams {
    fn new() -> Self {
        Self {
            params: [0; MAX_PARAMS],
            len: 0,
            intermediates: [0; MAX_INTERMEDIATES],
            intermediates_len: 0,
            has_colon: false,
        }
    }

    /// Get param at index, or default if missing/zero
    pub fn get(&self, idx: usize, default: u16) -> u16 {
        if idx < self.len && self.params[idx] != 0 {
            self.params[idx]
        } else {
            default
        }
    }

    pub fn has_intermediate(&self, b: u8) -> bool {
        self.intermediates_len > 0 && self.intermediates[0] == b
    }
}

/// Parser output action
#[derive(Clone, Debug)]
pub enum Action {
    /// Print a unicode character
    Print(char),
    /// Execute a C0/C1 control function
    Execute(u8),
    /// CSI sequence dispatched
    CsiDispatch { params: CsiParams, final_char: u8 },
    /// ESC sequence dispatched
    EscDispatch {
        intermediates: [u8; MAX_INTERMEDIATES],
        intermediates_len: usize,
        final_char: u8,
    },
    /// OSC (Operating System Command) dispatched
    OscDispatch { command: Vec<u8> },
}

pub struct Parser {
    state: State,
    // CSI accumulation
    params: [u16; MAX_PARAMS],
    params_len: usize,
    param_acc: u16,
    param_started: bool,
    intermediates: [u8; MAX_INTERMEDIATES],
    intermediates_len: usize,
    csi_has_colon: bool,
    // OSC accumulation
    osc_buf: Vec<u8>,
    osc_esc_pending: bool,
    // UTF-8 decoding
    utf8_buf: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            params_len: 0,
            param_acc: 0,
            param_started: false,
            intermediates: [0; MAX_INTERMEDIATES],
            intermediates_len: 0,
            csi_has_colon: false,
            osc_buf: Vec::with_capacity(MAX_OSC_LEN),
            osc_esc_pending: false,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_expected: 0,
        }
    }

    fn clear_params(&mut self) {
        self.params_len = 0;
        self.param_acc = 0;
        self.param_started = false;
        self.intermediates_len = 0;
        self.csi_has_colon = false;
    }

    fn finish_param(&mut self) {
        if self.params_len < MAX_PARAMS {
            self.params[self.params_len] = self.param_acc;
            self.params_len += 1;
        }
        self.param_acc = 0;
    }

    fn collect_intermediate(&mut self, b: u8) {
        if self.intermediates_len < MAX_INTERMEDIATES {
            self.intermediates[self.intermediates_len] = b;
            self.intermediates_len += 1;
        }
    }

    fn make_csi_params(&self) -> CsiParams {
        let mut p = CsiParams::new();
        p.params[..self.params_len].copy_from_slice(&self.params[..self.params_len]);
        p.len = self.params_len;
        p.intermediates[..self.intermediates_len]
            .copy_from_slice(&self.intermediates[..self.intermediates_len]);
        p.intermediates_len = self.intermediates_len;
        p.has_colon = self.csi_has_colon;
        p
    }

    /// Process a single byte. Returns 0-2 actions.
    pub fn advance(&mut self, byte: u8, actions: &mut Vec<Action>) {
        // Handle C0 controls anywhere (except in some states)
        match byte {
            // ESC always transitions
            0x1b => {
                if self.state == State::OscString {
                    // Potential ST terminator for OSC (ESC \).
                    self.state = State::Escape;
                    self.clear_params();
                    self.osc_esc_pending = true;
                    return;
                }
                // If we were in the middle of something, that's abandoned
                self.state = State::Escape;
                self.clear_params();
                self.osc_esc_pending = false;
                return;
            }
            // C0 controls executable in most states
            0x00..=0x06 | 0x08..=0x0e | 0x10..=0x17 | 0x19 | 0x1c..=0x1f
                if self.state != State::DcsPassthrough
                    && self.state != State::OscString
                    && self.state != State::SosPmApcString =>
            {
                actions.push(Action::Execute(byte));
                return;
            }
            // BEL (0x07) — special: terminates OSC strings
            0x07 if self.state == State::OscString => {
                let cmd = std::mem::take(&mut self.osc_buf);
                actions.push(Action::OscDispatch { command: cmd });
                self.state = State::Ground;
                return;
            }
            0x07 if self.state != State::DcsPassthrough && self.state != State::SosPmApcString => {
                actions.push(Action::Execute(byte));
                return;
            }
            // ST (String Terminator, C1 form) ends string-like states.
            0x9c if self.state == State::DcsPassthrough || self.state == State::SosPmApcString => {
                self.state = State::Ground;
                return;
            }
            // CAN/SUB abort sequence
            0x18 | 0x1a => {
                self.state = State::Ground;
                return;
            }
            _ => {}
        }

        match self.state {
            State::Ground => {
                self.handle_ground(byte, actions);
            }
            State::Escape => {
                self.handle_escape(byte, actions);
            }
            State::EscapeIntermediate => {
                self.handle_escape_intermediate(byte, actions);
            }
            State::CsiEntry => {
                self.handle_csi_entry(byte, actions);
            }
            State::CsiParam => {
                self.handle_csi_param(byte, actions);
            }
            State::CsiIntermediate => {
                self.handle_csi_intermediate(byte, actions);
            }
            State::CsiIgnore => {
                self.handle_csi_ignore(byte, actions);
            }
            State::OscString => {
                self.handle_osc_string(byte, actions);
            }
            State::DcsEntry | State::DcsParam | State::DcsIntermediate => {
                // Skip DCS for now — treat like ignore
                if byte >= 0x40 && byte <= 0x7e {
                    self.state = State::DcsPassthrough;
                }
            }
            State::DcsPassthrough => {
                // ST (ESC \) handled by ESC case above
                // For now, ignore DCS content
            }
            State::SosPmApcString => {
                // ST (ESC \) handled by ESC case above
                // Ignore content
            }
        }
    }

    fn handle_ground(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            // UTF-8 multi-byte start
            0xc0..=0xdf => {
                self.utf8_buf[0] = byte;
                self.utf8_len = 1;
                self.utf8_expected = 2;
            }
            0xe0..=0xef => {
                self.utf8_buf[0] = byte;
                self.utf8_len = 1;
                self.utf8_expected = 3;
            }
            0xf0..=0xf7 => {
                self.utf8_buf[0] = byte;
                self.utf8_len = 1;
                self.utf8_expected = 4;
            }
            // UTF-8 continuation
            0x80..=0xbf if self.utf8_len > 0 => {
                self.utf8_buf[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_expected {
                    if let Ok(s) = std::str::from_utf8(&self.utf8_buf[..self.utf8_len]) {
                        if let Some(c) = s.chars().next() {
                            actions.push(Action::Print(c));
                        }
                    }
                    self.utf8_len = 0;
                    self.utf8_expected = 0;
                }
            }
            // C1 controls (8-bit)
            0x90 => {
                // DCS
                self.state = State::DcsEntry;
                self.clear_params();
            }
            0x9b => {
                // CSI
                self.state = State::CsiEntry;
                self.clear_params();
            }
            0x9d => {
                // OSC
                self.state = State::OscString;
                self.osc_buf.clear();
            }
            0x98 | 0x9e | 0x9f => {
                // SOS, PM, APC
                self.state = State::SosPmApcString;
            }
            // Printable ASCII
            0x20..=0x7e => {
                self.utf8_len = 0; // reset any partial UTF-8
                actions.push(Action::Print(byte as char));
            }
            0x7f => {
                // DEL — ignore in ground
            }
            _ => {
                // Other high bytes — could be Latin-1 or broken UTF-8
                // Reset UTF-8 state and ignore
                self.utf8_len = 0;
            }
        }
    }

    fn handle_escape(&mut self, byte: u8, actions: &mut Vec<Action>) {
        if self.osc_esc_pending {
            if byte == b'\\' {
                // ST terminates OSC and dispatches collected command.
                let cmd = std::mem::take(&mut self.osc_buf);
                actions.push(Action::OscDispatch { command: cmd });
                self.osc_esc_pending = false;
                self.state = State::Ground;
                return;
            }
            // ESC inside OSC but not ST: abort OSC payload.
            self.osc_buf.clear();
            self.osc_esc_pending = false;
        }
        match byte {
            // Intermediates (space through /)
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.state = State::EscapeIntermediate;
            }
            // CSI (ESC [)
            b'[' => {
                self.state = State::CsiEntry;
                self.clear_params();
            }
            // OSC (ESC ])
            b']' => {
                self.state = State::OscString;
                self.osc_buf.clear();
            }
            // DCS (ESC P)
            b'P' => {
                self.state = State::DcsEntry;
                self.clear_params();
            }
            // SOS (ESC X), PM (ESC ^), APC (ESC _)
            b'X' | b'^' | b'_' => {
                self.state = State::SosPmApcString;
            }
            // ST (ESC \) — string terminator, ignore if not in string
            b'\\' => {
                self.state = State::Ground;
            }
            // Final characters (0x30-0x7e)
            0x30..=0x7e => {
                actions.push(Action::EscDispatch {
                    intermediates: self.intermediates,
                    intermediates_len: self.intermediates_len,
                    final_char: byte,
                });
                self.state = State::Ground;
            }
            _ => {
                self.state = State::Ground;
            }
        }
    }

    fn handle_escape_intermediate(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x20..=0x2f => {
                self.collect_intermediate(byte);
            }
            0x30..=0x7e => {
                actions.push(Action::EscDispatch {
                    intermediates: self.intermediates,
                    intermediates_len: self.intermediates_len,
                    final_char: byte,
                });
                self.state = State::Ground;
            }
            _ => {
                self.state = State::Ground;
            }
        }
    }

    fn handle_csi_entry(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            // Parameter bytes
            b'0'..=b'9' => {
                self.param_acc = (byte - b'0') as u16;
                self.param_started = true;
                self.state = State::CsiParam;
            }
            // Private marker (?, >, <, =)
            b'?' | b'>' | b'<' | b'=' => {
                self.collect_intermediate(byte);
                self.state = State::CsiParam;
            }
            // Separator
            b';' => {
                self.finish_param();
                self.state = State::CsiParam;
            }
            // Intermediate
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            // Final character — dispatch with empty params
            0x40..=0x7e => {
                // If no params were added, we still need to finish any pending
                if self.param_started {
                    self.finish_param();
                }
                actions.push(Action::CsiDispatch {
                    params: self.make_csi_params(),
                    final_char: byte,
                });
                self.state = State::Ground;
            }
            _ => {
                self.state = State::CsiIgnore;
            }
        }
    }

    fn handle_csi_param(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            b'0'..=b'9' => {
                self.param_acc = self
                    .param_acc
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u16);
                self.param_started = true;
            }
            b';' => {
                self.finish_param();
            }
            b':' => {
                self.csi_has_colon = true;
                self.finish_param();
            }
            0x20..=0x2f => {
                // Finish current param, then collect intermediate
                if self.param_started {
                    self.finish_param();
                }
                self.collect_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                // Final character — dispatch
                if self.param_started || self.params_len > 0 {
                    self.finish_param();
                }
                actions.push(Action::CsiDispatch {
                    params: self.make_csi_params(),
                    final_char: byte,
                });
                self.state = State::Ground;
            }
            _ => {
                self.state = State::CsiIgnore;
            }
        }
    }

    fn handle_csi_intermediate(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x20..=0x2f => {
                self.collect_intermediate(byte);
            }
            0x40..=0x7e => {
                actions.push(Action::CsiDispatch {
                    params: self.make_csi_params(),
                    final_char: byte,
                });
                self.state = State::Ground;
            }
            _ => {
                self.state = State::CsiIgnore;
            }
        }
    }

    fn handle_csi_ignore(&mut self, byte: u8, _actions: &mut Vec<Action>) {
        match byte {
            0x40..=0x7e => {
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn handle_osc_string(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            // ST via ESC \ is handled in the ESC handler
            // BEL terminator handled in the main advance()
            0x07 => {
                // Already handled above
            }
            // 0x9c — 8-bit ST
            0x9c => {
                let cmd = std::mem::take(&mut self.osc_buf);
                actions.push(Action::OscDispatch { command: cmd });
                self.state = State::Ground;
            }
            _ => {
                if self.osc_buf.len() < MAX_OSC_LEN {
                    self.osc_buf.push(byte);
                }
            }
        }
    }

    /// Process a slice of bytes, collecting actions.
    pub fn process(&mut self, bytes: &[u8], actions: &mut Vec<Action>) {
        for &b in bytes {
            self.advance(b, actions);
        }
    }
}
