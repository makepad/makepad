//! VT-series escape/control sequence parser.
//!
//! Port of ghostty `src/terminal/Parser.zig` + `parse_table.zig`, which
//! implement the state machine described at
//! https://vt100.net/emu/dec_ansi_parser (with ghostty's deviations, e.g.
//! colon params accepted and recorded as subparam separators, only honored
//! for SGR 'm').
//!
//! Invariants carried over from the Zig:
//! - `Parser::next(byte)` returns up to 3 actions in order: state-exit
//!   action, transition action, state-entry action. Exit and entry actions
//!   only fire when the state actually changes.
//! - The transition table is generated from the same single/range calls in
//!   the same order as `parse_table.zig` (later writes override earlier
//!   ones, which is how dcs_passthrough/dcs_ignore/osc_string take back the
//!   0x80-0xFF range from the "anywhere" C1 transitions).
//! - `param_acc` uses saturating arithmetic like ghostty (`*|`/`+|`).
//! - Params: max 24; separator after param i recorded in `params_sep`
//!   bitset when it was a colon. Colon/mixed separators are only allowed
//!   through for final byte 'm'; any other final with colon seps consumes
//!   the sequence and dispatches nothing (ghostty warnCsiSepMismatch path,
//!   minus the logging).

use crate::term::osc::{OscCommand, OscParser};

pub const MAX_INTERMEDIATE: usize = 4;
pub const MAX_PARAMS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiIntermediate,
    CsiParam,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApcString,
}

const NUM_STATES: usize = 14;

const fn state_from_index(i: usize) -> State {
    match i {
        0 => State::Ground,
        1 => State::Escape,
        2 => State::EscapeIntermediate,
        3 => State::CsiEntry,
        4 => State::CsiIntermediate,
        5 => State::CsiParam,
        6 => State::CsiIgnore,
        7 => State::DcsEntry,
        8 => State::DcsParam,
        9 => State::DcsIntermediate,
        10 => State::DcsPassthrough,
        11 => State::DcsIgnore,
        12 => State::OscString,
        _ => State::SosPmApcString,
    }
}

// `state_from_index` must be the inverse of `State as usize`.
const _: () = assert!(State::Ground as usize == 0);
const _: () = assert!(State::DcsPassthrough as usize == 10);
const _: () = assert!(State::SosPmApcString as usize == NUM_STATES - 1);

/// Transition action, taken during a state transition. Internal to the
/// state machine; ghostty `TransitionAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransitionAction {
    /// ghostty `.none`
    Nop,
    Ignore,
    Print,
    Execute,
    Collect,
    Param,
    EscDispatch,
    CsiDispatch,
    Put,
    OscPut,
    ApcPut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Effect {
    state: State,
    action: TransitionAction,
}

type OptTable = [[Option<Effect>; NUM_STATES]; 256];
type Table = [[Effect; NUM_STATES]; 256];

const fn single(t: &mut OptTable, c: u8, s0: State, s1: State, a: TransitionAction) {
    t[c as usize][s0 as usize] = Some(Effect {
        state: s1,
        action: a,
    });
}

const fn range(t: &mut OptTable, from: u8, to: u8, s0: State, s1: State, a: TransitionAction) {
    if from > to {
        return;
    }
    let mut i = from;
    loop {
        single(t, i, s0, s1, a);
        // `to` may be 0xFF, so break before the increment overflows.
        if i == to {
            break;
        }
        i += 1;
    }
}

/// The full state transition table, generated exactly as ghostty's
/// `parse_table.zig` genTable(): unset entries default to "stay in the
/// current state, do nothing".
const fn gen_table() -> Table {
    use State as S;
    use TransitionAction as A;

    let mut t: OptTable = [[None; NUM_STATES]; 256];

    // anywhere transitions
    let mut si = 0;
    while si < NUM_STATES {
        let source = state_from_index(si);

        // anywhere => ground
        single(&mut t, 0x18, source, S::Ground, A::Execute);
        single(&mut t, 0x1A, source, S::Ground, A::Execute);
        range(&mut t, 0x80, 0x8F, source, S::Ground, A::Execute);
        range(&mut t, 0x91, 0x97, source, S::Ground, A::Execute);
        single(&mut t, 0x99, source, S::Ground, A::Execute);
        single(&mut t, 0x9A, source, S::Ground, A::Execute);
        single(&mut t, 0x9C, source, S::Ground, A::Nop);

        // anywhere => escape
        single(&mut t, 0x1B, source, S::Escape, A::Nop);

        // anywhere => sos_pm_apc_string
        single(&mut t, 0x98, source, S::SosPmApcString, A::Nop);
        single(&mut t, 0x9E, source, S::SosPmApcString, A::Nop);
        single(&mut t, 0x9F, source, S::SosPmApcString, A::Nop);

        // anywhere => csi_entry
        single(&mut t, 0x9B, source, S::CsiEntry, A::Nop);

        // anywhere => dcs_entry
        single(&mut t, 0x90, source, S::DcsEntry, A::Nop);

        // anywhere => osc_string
        single(&mut t, 0x9D, source, S::OscString, A::Nop);

        si += 1;
    }

    // ground
    {
        let source = S::Ground;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        range(&mut t, 0x20, 0x7F, source, source, A::Print);
    }

    // escape_intermediate
    {
        let source = S::EscapeIntermediate;

        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        range(&mut t, 0x20, 0x2F, source, source, A::Collect);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x30, 0x7E, source, S::Ground, A::EscDispatch);
    }

    // sos_pm_apc_string
    {
        let source = S::SosPmApcString;

        // events
        single(&mut t, 0x19, source, source, A::ApcPut);
        range(&mut t, 0, 0x17, source, source, A::ApcPut);
        range(&mut t, 0x1C, 0x1F, source, source, A::ApcPut);
        range(&mut t, 0x20, 0x7F, source, source, A::ApcPut);
    }

    // escape
    {
        let source = S::Escape;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x30, 0x4F, source, S::Ground, A::EscDispatch);
        range(&mut t, 0x51, 0x57, source, S::Ground, A::EscDispatch);
        range(&mut t, 0x60, 0x7E, source, S::Ground, A::EscDispatch);
        single(&mut t, 0x59, source, S::Ground, A::EscDispatch);
        single(&mut t, 0x5A, source, S::Ground, A::EscDispatch);
        single(&mut t, 0x5C, source, S::Ground, A::EscDispatch);

        // => escape_intermediate
        range(&mut t, 0x20, 0x2F, source, S::EscapeIntermediate, A::Collect);

        // => sos_pm_apc_string
        single(&mut t, 0x58, source, S::SosPmApcString, A::Nop);
        single(&mut t, 0x5E, source, S::SosPmApcString, A::Nop);
        single(&mut t, 0x5F, source, S::SosPmApcString, A::Nop);

        // => dcs_entry
        single(&mut t, 0x50, source, S::DcsEntry, A::Nop);

        // => csi_entry
        single(&mut t, 0x5B, source, S::CsiEntry, A::Nop);

        // => osc_string
        single(&mut t, 0x5D, source, S::OscString, A::Nop);
    }

    // dcs_entry
    {
        let source = S::DcsEntry;

        // events
        single(&mut t, 0x19, source, source, A::Ignore);
        range(&mut t, 0, 0x17, source, source, A::Ignore);
        range(&mut t, 0x1C, 0x1F, source, source, A::Ignore);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => dcs_intermediate
        range(&mut t, 0x20, 0x2F, source, S::DcsIntermediate, A::Collect);

        // => dcs_ignore
        single(&mut t, 0x3A, source, S::DcsIgnore, A::Nop);

        // => dcs_param
        range(&mut t, 0x30, 0x39, source, S::DcsParam, A::Param);
        single(&mut t, 0x3B, source, S::DcsParam, A::Param);
        range(&mut t, 0x3C, 0x3F, source, S::DcsParam, A::Collect);

        // => dcs_passthrough
        range(&mut t, 0x40, 0x7E, source, S::DcsPassthrough, A::Nop);
    }

    // dcs_intermediate
    {
        let source = S::DcsIntermediate;

        // events
        single(&mut t, 0x19, source, source, A::Ignore);
        range(&mut t, 0, 0x17, source, source, A::Ignore);
        range(&mut t, 0x1C, 0x1F, source, source, A::Ignore);
        range(&mut t, 0x20, 0x2F, source, source, A::Collect);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => dcs_ignore
        range(&mut t, 0x30, 0x3F, source, S::DcsIgnore, A::Nop);

        // => dcs_passthrough
        range(&mut t, 0x40, 0x7E, source, S::DcsPassthrough, A::Nop);
    }

    // dcs_ignore
    {
        let source = S::DcsIgnore;

        // events
        single(&mut t, 0x19, source, source, A::Ignore);
        range(&mut t, 0, 0x17, source, source, A::Ignore);
        range(&mut t, 0x1C, 0x1F, source, source, A::Ignore);

        // High bytes are ignored payload data, overriding the "anywhere"
        // C1 transitions. See dcs_passthrough below: the extra concern
        // here is that a UTF-8 payload in an ignored DCS could otherwise
        // begin a live sequence mid-string (e.g. 0x9B => csi_entry).
        range(&mut t, 0x80, 0xFF, source, source, A::Ignore);
    }

    // dcs_param
    {
        let source = S::DcsParam;

        // events
        single(&mut t, 0x19, source, source, A::Ignore);
        range(&mut t, 0, 0x17, source, source, A::Ignore);
        range(&mut t, 0x1C, 0x1F, source, source, A::Ignore);
        range(&mut t, 0x30, 0x39, source, source, A::Param);
        single(&mut t, 0x3B, source, source, A::Param);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => dcs_ignore
        single(&mut t, 0x3A, source, S::DcsIgnore, A::Nop);
        range(&mut t, 0x3C, 0x3F, source, S::DcsIgnore, A::Nop);

        // => dcs_intermediate
        range(&mut t, 0x20, 0x2F, source, S::DcsIntermediate, A::Collect);

        // => dcs_passthrough
        range(&mut t, 0x40, 0x7E, source, S::DcsPassthrough, A::Nop);
    }

    // dcs_passthrough
    {
        let source = S::DcsPassthrough;

        // events
        single(&mut t, 0x19, source, source, A::Put);
        range(&mut t, 0, 0x17, source, source, A::Put);
        range(&mut t, 0x1C, 0x1F, source, source, A::Put);
        range(&mut t, 0x20, 0x7E, source, source, A::Put);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // High bytes are payload data, overriding the "anywhere" C1
        // transitions, matching osc_string below. DCS payloads carry
        // UTF-8 text (e.g. tmux control mode pane content): without this
        // a continuation byte in the C1 range terminates or corrupts the
        // string. This includes 0x9C (8-bit ST) on purpose, since a raw
        // 0x9C is indistinguishable from a UTF-8 continuation byte.
        // DCS strings terminate via 7-bit ST (ESC \) and abort via
        // CAN/SUB, which are unaffected here.
        range(&mut t, 0x80, 0xFF, source, source, A::Put);
    }

    // csi_param
    {
        let source = S::CsiParam;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        range(&mut t, 0x30, 0x39, source, source, A::Param);
        single(&mut t, 0x3A, source, source, A::Param);
        single(&mut t, 0x3B, source, source, A::Param);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x40, 0x7E, source, S::Ground, A::CsiDispatch);

        // => csi_ignore
        range(&mut t, 0x3C, 0x3F, source, S::CsiIgnore, A::Nop);

        // => csi_intermediate
        range(&mut t, 0x20, 0x2F, source, S::CsiIntermediate, A::Collect);
    }

    // csi_ignore
    {
        let source = S::CsiIgnore;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        range(&mut t, 0x20, 0x3F, source, source, A::Ignore);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x40, 0x7E, source, S::Ground, A::Nop);
    }

    // csi_intermediate
    {
        let source = S::CsiIntermediate;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        range(&mut t, 0x20, 0x2F, source, source, A::Collect);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x40, 0x7E, source, S::Ground, A::CsiDispatch);

        // => csi_ignore
        range(&mut t, 0x30, 0x3F, source, S::CsiIgnore, A::Nop);
    }

    // csi_entry
    {
        let source = S::CsiEntry;

        // events
        single(&mut t, 0x19, source, source, A::Execute);
        range(&mut t, 0, 0x17, source, source, A::Execute);
        range(&mut t, 0x1C, 0x1F, source, source, A::Execute);
        single(&mut t, 0x7F, source, source, A::Ignore);

        // => ground
        range(&mut t, 0x40, 0x7E, source, S::Ground, A::CsiDispatch);

        // => csi_ignore
        single(&mut t, 0x3A, source, S::CsiIgnore, A::Nop);

        // => csi_intermediate
        range(&mut t, 0x20, 0x2F, source, S::CsiIntermediate, A::Collect);

        // => csi_param
        range(&mut t, 0x30, 0x39, source, S::CsiParam, A::Param);
        single(&mut t, 0x3B, source, S::CsiParam, A::Param);
        range(&mut t, 0x3C, 0x3F, source, S::CsiParam, A::Collect);
    }

    // osc_string
    {
        let source = S::OscString;

        // events
        single(&mut t, 0x19, source, source, A::Ignore);
        range(&mut t, 0, 0x06, source, source, A::Ignore);
        range(&mut t, 0x08, 0x17, source, source, A::Ignore);
        range(&mut t, 0x1C, 0x1F, source, source, A::Ignore);
        range(&mut t, 0x20, 0xFF, source, source, A::OscPut);

        // XTerm accepts either BEL or ST for terminating OSC sequences,
        // and when returning information uses the same terminator that
        // was used in the query.
        single(&mut t, 0x07, source, S::Ground, A::Nop);
    }

    // Create our immutable version.
    let mut final_table: Table = [[Effect {
        state: State::Ground,
        action: A::Nop,
    }; NUM_STATES]; 256];
    let mut c = 0;
    while c < 256 {
        let mut j = 0;
        while j < NUM_STATES {
            final_table[c][j] = match t[c][j] {
                Some(e) => e,
                None => Effect {
                    state: state_from_index(j),
                    action: A::Nop,
                },
            };
            j += 1;
        }
        c += 1;
    }

    final_table
}

static TABLE: Table = gen_table();

/// A dispatched CSI sequence.
#[derive(Clone, Debug)]
pub struct Csi {
    pub intermediates: [u8; MAX_INTERMEDIATE],
    pub intermediates_len: usize,
    pub params: [u16; MAX_PARAMS],
    pub params_len: usize,
    /// Bit i set = the separator AFTER params[i] was a colon.
    /// Example: `0;4:3` has bit 1 set.
    pub params_sep: u32,
    pub final_byte: u8,
}

impl Default for Csi {
    fn default() -> Self {
        Self {
            intermediates: [0; MAX_INTERMEDIATE],
            intermediates_len: 0,
            params: [0; MAX_PARAMS],
            params_len: 0,
            params_sep: 0,
            final_byte: 0,
        }
    }
}

impl Csi {
    pub fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediates_len]
    }

    pub fn params(&self) -> &[u16] {
        &self.params[..self.params_len]
    }

    /// Param at `idx`, with `default` when missing or zero.
    pub fn get(&self, idx: usize, default: u16) -> u16 {
        if idx < self.params_len && self.params[idx] != 0 {
            self.params[idx]
        } else {
            default
        }
    }

    /// Param at `idx`, with `default` only when missing (zero is honored).
    pub fn get_allow_zero(&self, idx: usize, default: u16) -> u16 {
        if idx < self.params_len {
            self.params[idx]
        } else {
            default
        }
    }

    pub fn sep_is_colon(&self, idx: usize) -> bool {
        self.params_sep & (1 << idx) != 0
    }

    pub fn has_intermediate(&self, byte: u8) -> bool {
        self.intermediates().contains(&byte)
    }

    /// True for DEC private sequences (`?` prefix).
    pub fn is_private(&self) -> bool {
        self.has_intermediate(b'?')
    }
}

/// A dispatched ESC sequence.
#[derive(Clone, Debug)]
pub struct Esc {
    pub intermediates: [u8; MAX_INTERMEDIATE],
    pub intermediates_len: usize,
    pub final_byte: u8,
}

impl Esc {
    pub fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediates_len]
    }
}

/// DCS hook data.
#[derive(Clone, Debug)]
pub struct Dcs {
    pub intermediates: [u8; MAX_INTERMEDIATE],
    pub intermediates_len: usize,
    pub params: [u16; MAX_PARAMS],
    pub params_len: usize,
    pub final_byte: u8,
}

#[derive(Clone, Debug)]
pub enum Action {
    /// Draw a character. The parser itself only produces this for
    /// single-byte prints; UTF-8 ground text is decoded by the stream.
    Print(char),
    /// Execute a C0/C1 control function.
    Execute(u8),
    CsiDispatch(Csi),
    EscDispatch(Esc),
    OscDispatch(OscCommand),
    DcsHook(Dcs),
    DcsPut(u8),
    DcsUnhook,
    ApcStart,
    ApcPut(u8),
    ApcEnd,
}

pub struct Parser {
    pub state: State,
    pub intermediates: [u8; MAX_INTERMEDIATE],
    pub intermediates_idx: usize,
    pub params: [u16; MAX_PARAMS],
    pub params_sep: u32,
    pub params_idx: usize,
    pub param_acc: u16,
    pub param_acc_idx: u8,
    pub osc_parser: OscParser,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            intermediates: [0; MAX_INTERMEDIATE],
            intermediates_idx: 0,
            params: [0; MAX_PARAMS],
            params_sep: 0,
            params_idx: 0,
            param_acc: 0,
            param_acc_idx: 0,
            osc_parser: OscParser::new(),
        }
    }

    /// Consume one byte; returns exit/transition/entry actions in order.
    pub fn next(&mut self, byte: u8) -> [Option<Action>; 3] {
        let effect = TABLE[byte as usize][self.state as usize];
        let next_state = effect.state;
        let changed = self.state != next_state;

        // When going from one state to another, the actions take place in
        // this order: exit action from the old state, transition action,
        // entry action into the new state.
        let exit = if !changed {
            None
        } else {
            match self.state {
                State::OscString => self.osc_parser.end(Some(byte)).map(Action::OscDispatch),
                State::DcsPassthrough => Some(Action::DcsUnhook),
                State::SosPmApcString => Some(Action::ApcEnd),
                _ => None,
            }
        };

        let transition = self.do_action(effect.action, byte);

        let entry = if !changed {
            None
        } else {
            match next_state {
                State::Escape | State::DcsEntry | State::CsiEntry => {
                    self.clear();
                    None
                }
                State::OscString => {
                    self.osc_parser.reset();
                    None
                }
                State::DcsPassthrough => {
                    // Ignore too many parameters
                    if self.params_idx >= MAX_PARAMS {
                        None
                    } else {
                        // Finalize parameters
                        if self.param_acc_idx > 0 {
                            self.params[self.params_idx] = self.param_acc;
                            self.params_idx += 1;
                        }
                        Some(Action::DcsHook(Dcs {
                            intermediates: self.intermediates,
                            intermediates_len: self.intermediates_idx,
                            params: self.params,
                            params_len: self.params_idx,
                            final_byte: byte,
                        }))
                    }
                }
                State::SosPmApcString => Some(Action::ApcStart),
                _ => None,
            }
        };

        self.state = next_state;

        [exit, transition, entry]
    }

    #[inline]
    fn do_action(&mut self, action: TransitionAction, c: u8) -> Option<Action> {
        match action {
            TransitionAction::Nop | TransitionAction::Ignore => None,
            TransitionAction::Print => Some(Action::Print(c as char)),
            TransitionAction::Execute => Some(Action::Execute(c)),
            TransitionAction::Collect => {
                self.collect(c);
                None
            }
            TransitionAction::Param => {
                // Semicolon separates parameters. If we encounter a
                // separator we store the accumulator and move on to the
                // next parameter.
                if c == b';' || c == b':' {
                    // Ignore too many parameters
                    if self.params_idx >= MAX_PARAMS {
                        return None;
                    }

                    // Set param final value
                    self.params[self.params_idx] = self.param_acc;
                    if c == b':' {
                        self.params_sep |= 1 << self.params_idx;
                    }
                    self.params_idx += 1;

                    // Reset current param value to 0
                    self.param_acc = 0;
                    self.param_acc_idx = 0;
                    return None;
                }

                // A numeric value. Add it to our accumulator. The table
                // only routes digits and the separators above here.
                self.param_acc = self.param_acc.saturating_mul(10);
                self.param_acc = self.param_acc.saturating_add(c.wrapping_sub(b'0') as u16);

                // Increment our accumulator index. If we overflow then
                // we're out of bounds and we exit immediately.
                let (idx, overflow) = self.param_acc_idx.overflowing_add(1);
                self.param_acc_idx = idx;
                if overflow {
                    return None;
                }

                // The client is expected to perform no action.
                None
            }
            TransitionAction::OscPut => {
                self.osc_parser.next(c);
                None
            }
            TransitionAction::CsiDispatch => {
                // Ignore too many parameters
                if self.params_idx >= MAX_PARAMS {
                    return None;
                }

                // Finalize parameters if we have one
                if self.param_acc_idx > 0 {
                    self.params[self.params_idx] = self.param_acc;
                    self.params_idx += 1;
                }

                // We only allow colon or mixed separators for the 'm'
                // command (ghostty warnCsiSepMismatch).
                if c != b'm' && self.params_sep != 0 {
                    return None;
                }

                Some(Action::CsiDispatch(Csi {
                    intermediates: self.intermediates,
                    intermediates_len: self.intermediates_idx,
                    params: self.params,
                    params_len: self.params_idx,
                    params_sep: self.params_sep,
                    final_byte: c,
                }))
            }
            TransitionAction::EscDispatch => Some(Action::EscDispatch(Esc {
                intermediates: self.intermediates,
                intermediates_len: self.intermediates_idx,
                final_byte: c,
            })),
            TransitionAction::Put => Some(Action::DcsPut(c)),
            TransitionAction::ApcPut => Some(Action::ApcPut(c)),
        }
    }

    #[inline]
    fn collect(&mut self, c: u8) {
        if self.intermediates_idx >= MAX_INTERMEDIATE {
            return;
        }
        self.intermediates[self.intermediates_idx] = c;
        self.intermediates_idx += 1;
    }

    /// Clear intermediates/params accumulation (ghostty `clear`).
    pub fn clear(&mut self) {
        self.intermediates_idx = 0;
        self.params_idx = 0;
        self.params_sep = 0;
        self.param_acc = 0;
        self.param_acc_idx = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed bytes that must produce no actions at all.
    fn feed_silent(p: &mut Parser, bytes: &str) {
        for c in bytes.bytes() {
            let a = p.next(c);
            assert!(a[0].is_none(), "unexpected exit action on {:?}", c as char);
            assert!(
                a[1].is_none(),
                "unexpected transition action on {:?}",
                c as char
            );
            assert!(a[2].is_none(), "unexpected entry action on {:?}", c as char);
        }
    }

    fn csi_of(a: &[Option<Action>; 3]) -> &Csi {
        match &a[1] {
            Some(Action::CsiDispatch(csi)) => csi,
            other => panic!("expected csi_dispatch, got {:?}", other),
        }
    }

    fn esc_of(a: &[Option<Action>; 3]) -> &Esc {
        match &a[1] {
            Some(Action::EscDispatch(esc)) => esc,
            other => panic!("expected esc_dispatch, got {:?}", other),
        }
    }

    fn dcs_of(a: &[Option<Action>; 3]) -> &Dcs {
        match &a[2] {
            Some(Action::DcsHook(dcs)) => dcs,
            other => panic!("expected dcs_hook, got {:?}", other),
        }
    }

    #[test]
    fn basic() {
        let mut p = Parser::new();
        let a = p.next(0x9E);
        assert_eq!(p.state, State::SosPmApcString);
        assert!(matches!(a[2], Some(Action::ApcStart)));

        let a = p.next(0x9C);
        assert_eq!(p.state, State::Ground);
        assert!(matches!(a[0], Some(Action::ApcEnd)));

        {
            let a = p.next(b'a');
            assert_eq!(p.state, State::Ground);
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::Print('a'))));
            assert!(a[2].is_none());
        }

        {
            let a = p.next(0x19);
            assert_eq!(p.state, State::Ground);
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::Execute(0x19))));
            assert!(a[2].is_none());
        }
    }

    #[test]
    fn esc_paren_b() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let _ = p.next(b'(');

        let a = p.next(b'B');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = esc_of(&a);
        assert_eq!(d.final_byte, b'B');
        assert_eq!(d.intermediates(), &[b'(']);
    }

    #[test]
    fn csi_esc_bracket_h() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let _ = p.next(0x5B);

        let a = p.next(0x48);
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, 0x48);
        assert_eq!(d.params().len(), 0);
    }

    #[test]
    fn csi_1_4_h() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let _ = p.next(0x5B);
        let _ = p.next(0x31); // 1
        let _ = p.next(0x3B); // ;
        let _ = p.next(0x34); // 4

        let a = p.next(0x48); // H
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'H');
        assert_eq!(d.params(), &[1, 4]);
    }

    #[test]
    fn csi_sgr_38_colon_2() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[38:2");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params(), &[38, 2]);
        assert!(d.sep_is_colon(0));
        assert!(!d.sep_is_colon(1));
    }

    #[test]
    fn csi_sgr_colon_followed_by_semicolon() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[48:2");

        {
            let a = p.next(b'm');
            assert_eq!(p.state, State::Ground);
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
            assert!(a[2].is_none());
        }

        let _ = p.next(0x1B);
        let _ = p.next(b'[');
        {
            let a = p.next(b'H');
            assert_eq!(p.state, State::Ground);
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
            assert!(a[2].is_none());
        }
    }

    #[test]
    fn csi_sgr_mixed_colon_and_semicolon() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[38:5:1;48:5:0");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
        assert!(a[2].is_none());
    }

    #[test]
    fn csi_sgr_48_colon_2() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[48:2:240:143:104");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params(), &[48, 2, 240, 143, 104]);
        assert!(d.sep_is_colon(0));
        assert!(d.sep_is_colon(1));
        assert!(d.sep_is_colon(2));
        assert!(d.sep_is_colon(3));
        assert!(!d.sep_is_colon(4));
    }

    #[test]
    fn csi_sgr_4_colon_3() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[4:3");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params(), &[4, 3]);
        assert!(d.sep_is_colon(0));
        assert!(!d.sep_is_colon(1));
    }

    #[test]
    fn csi_sgr_with_many_blank_and_colon() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[58:2::240:143:104");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params(), &[58, 2, 0, 240, 143, 104]);
        for i in 0..5 {
            assert!(d.sep_is_colon(i), "sep {} should be colon", i);
        }
        assert!(!d.sep_is_colon(5));
    }

    // This is from a Kakoune actual SGR sequence.
    #[test]
    fn csi_sgr_mixed_colon_and_semicolon_with_blank() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[;4:3;38;2;175;175;215;58:2::190:80:70");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(
            d.params(),
            &[0, 4, 3, 38, 2, 175, 175, 215, 58, 2, 0, 190, 80, 70]
        );
        let colon: [bool; 14] = [
            false, true, false, false, false, false, false, false, true, true, true, true, true,
            false,
        ];
        for (i, want) in colon.iter().enumerate() {
            assert_eq!(d.sep_is_colon(i), *want, "sep {}", i);
        }
    }

    // This is from a Kakoune actual SGR sequence also.
    #[test]
    fn csi_sgr_mixed_colon_and_semicolon_underline_bg_fg() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[4:3;38;2;51;51;51;48;2;170;170;170;58;2;255;97;136");

        let a = p.next(b'm');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(
            d.params(),
            &[4, 3, 38, 2, 51, 51, 51, 48, 2, 170, 170, 170, 58, 2, 255, 97, 136]
        );
        assert!(d.sep_is_colon(0));
        for i in 1..17 {
            assert!(!d.sep_is_colon(i), "sep {} should be semicolon", i);
        }
    }

    #[test]
    fn csi_colon_for_non_m_final() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[38:2h");
        assert_eq!(p.state, State::Ground);
    }

    #[test]
    fn csi_request_mode_decrqm() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[?2026$");

        let a = p.next(b'p');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'p');
        assert_eq!(d.intermediates(), &[b'?', b'$']);
        assert_eq!(d.params(), &[2026]);
        assert!(d.is_private());
    }

    #[test]
    fn csi_change_cursor() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[3 ");

        let a = p.next(b'q');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[2].is_none());

        let d = csi_of(&a);
        assert_eq!(d.final_byte, b'q');
        assert_eq!(d.intermediates(), &[b' ']);
        assert_eq!(d.params(), &[3]);
    }

    #[test]
    fn csi_too_many_params() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let _ = p.next(b'[');
        for _ in 0..100 {
            let _ = p.next(b'1');
            let _ = p.next(b';');
        }
        let _ = p.next(b'1');

        let a = p.next(b'C');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        assert!(a[2].is_none());
    }

    #[test]
    fn csi_sgr_up_to_max_params() {
        for max in 1..=MAX_PARAMS {
            let mut p = Parser::new();
            let _ = p.next(0x1B);
            let _ = p.next(b'[');

            for _ in 0..max - 1 {
                let _ = p.next(b'1');
                let _ = p.next(b';');
            }
            let _ = p.next(b'2');

            let a = p.next(b'H');
            assert_eq!(p.state, State::Ground);
            assert!(a[0].is_none());
            assert!(a[2].is_none());

            let csi = csi_of(&a);
            assert_eq!(csi.params().len(), max);
            assert_eq!(csi.params()[max - 1], 2);
        }
    }

    #[test]
    fn csi_sgr_beyond_max_drops_it() {
        // Has to be +2 for the loop below
        let max = MAX_PARAMS + 2;

        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let _ = p.next(b'[');

        for _ in 0..max - 1 {
            let _ = p.next(b'1');
            let _ = p.next(b';');
        }
        let _ = p.next(b'2');

        let a = p.next(b'H');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        assert!(a[2].is_none());
    }

    #[test]
    fn dcs_xtgettcap() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "P+");

        let a = p.next(b'q');
        assert_eq!(p.state, State::DcsPassthrough);
        assert!(a[0].is_none());
        assert!(a[1].is_none());

        let hook = dcs_of(&a);
        assert_eq!(&hook.intermediates[..hook.intermediates_len], &[b'+']);
        assert_eq!(&hook.params[..hook.params_len], &[] as &[u16]);
        assert_eq!(hook.final_byte, b'q');
    }

    #[test]
    fn dcs_params() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "P1000");

        let a = p.next(b'p');
        assert_eq!(p.state, State::DcsPassthrough);
        assert!(a[0].is_none());
        assert!(a[1].is_none());

        let hook = dcs_of(&a);
        assert_eq!(&hook.params[..hook.params_len], &[1000]);
        assert_eq!(hook.final_byte, b'p');
    }

    #[test]
    fn dcs_too_many_params() {
        // Regression test for a crash found by fuzzing (afl). When a DCS
        // sequence has more than MAX_PARAMS parameters and param_acc_idx
        // > 0, entering dcs_passthrough wrote params[params_idx] without
        // a bounds check.
        let mut p = Parser::new();
        let _ = p.next(0x1B); // ESC
        let _ = p.next(b'P'); // DCS entry

        // A digit then MAX_PARAMS semicolons fills all param slots.
        let _ = p.next(b'6');
        for _ in 0..MAX_PARAMS {
            let _ = p.next(b';');
        }
        // Another digit so param_acc_idx > 0 while params_idx == MAX_PARAMS.
        let _ = p.next(b'7');

        // The final byte triggers entry to dcs_passthrough. The DCS is
        // dropped entirely, consistent with how CSI handles overflow.
        let a = p.next(b'p');
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        assert!(a[2].is_none());
    }

    #[test]
    fn dcs_put_and_unhook() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "P");
        let a = p.next(b'q');
        assert!(matches!(a[2], Some(Action::DcsHook(_))));

        let a = p.next(b'x');
        assert!(matches!(a[1], Some(Action::DcsPut(b'x'))));

        // 7-bit ST: ESC leaves passthrough (unhook), '\' dispatches.
        let a = p.next(0x1B);
        assert!(matches!(a[0], Some(Action::DcsUnhook)));
        assert_eq!(p.state, State::Escape);
        let a = p.next(b'\\');
        assert_eq!(p.state, State::Ground);
        assert!(matches!(a[1], Some(Action::EscDispatch(_))));
    }

    #[test]
    fn apc_start_put_end() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        let a = p.next(b'_');
        assert_eq!(p.state, State::SosPmApcString);
        assert!(matches!(a[2], Some(Action::ApcStart)));

        let a = p.next(b'G');
        assert!(matches!(a[1], Some(Action::ApcPut(b'G'))));

        let a = p.next(0x1B);
        assert!(matches!(a[0], Some(Action::ApcEnd)));
        assert_eq!(p.state, State::Escape);
        let _ = p.next(b'\\');
        assert_eq!(p.state, State::Ground);
    }

    #[test]
    fn c1_controls_enter_states() {
        // 8-bit C1 introducers act per the anywhere transitions.
        let mut p = Parser::new();
        let _ = p.next(0x9B); // CSI
        assert_eq!(p.state, State::CsiEntry);
        let a = p.next(b'H');
        assert_eq!(p.state, State::Ground);
        assert!(matches!(a[1], Some(Action::CsiDispatch(_))));

        let _ = p.next(0x90); // DCS
        assert_eq!(p.state, State::DcsEntry);
        let _ = p.next(0x18); // CAN aborts
        assert_eq!(p.state, State::Ground);

        let _ = p.next(0x9D); // OSC
        assert_eq!(p.state, State::OscString);
        // Feeding payload stays in osc_string; do not terminate here since
        // OscParser::end is another lane's surface.
        let a = p.next(b'0');
        assert_eq!(p.state, State::OscString);
        assert!(a[0].is_none() && a[1].is_none() && a[2].is_none());
    }

    #[test]
    fn can_and_sub_abort_to_ground() {
        for abort in [0x18u8, 0x1A] {
            let mut p = Parser::new();
            let _ = p.next(0x1B);
            let _ = p.next(b'[');
            let _ = p.next(b'1');
            let a = p.next(abort);
            assert_eq!(p.state, State::Ground);
            assert!(matches!(a[1], Some(Action::Execute(_))));
        }
    }

    #[test]
    fn csi_ignore_consumes_sequence() {
        // A second private marker after params begins => csi_ignore, and
        // the final byte dispatches nothing.
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[1;2<");
        assert_eq!(p.state, State::CsiIgnore);
        let a = p.next(b'H');
        assert_eq!(p.state, State::Ground);
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        assert!(a[2].is_none());
    }

    #[test]
    fn param_saturates() {
        let mut p = Parser::new();
        let _ = p.next(0x1B);
        feed_silent(&mut p, "[99999999999");
        let a = p.next(b'H');
        let d = csi_of(&a);
        assert_eq!(d.params(), &[u16::MAX]);
    }

    // parse_table.zig: dcs_passthrough high bytes are payload data
    #[test]
    fn table_dcs_passthrough_high_bytes() {
        for c in 0x80..=0xFFusize {
            let entry = TABLE[c][State::DcsPassthrough as usize];
            assert_eq!(entry.state, State::DcsPassthrough, "byte {:#x}", c);
            assert_eq!(entry.action, TransitionAction::Put, "byte {:#x}", c);
        }
    }

    // parse_table.zig: dcs_ignore high bytes are ignored payload data
    #[test]
    fn table_dcs_ignore_high_bytes() {
        for c in 0x80..=0xFFusize {
            let entry = TABLE[c][State::DcsIgnore as usize];
            assert_eq!(entry.state, State::DcsIgnore, "byte {:#x}", c);
            assert_eq!(entry.action, TransitionAction::Ignore, "byte {:#x}", c);
        }
    }

    // parse_table.zig: ESC, CAN and SUB still exit dcs_passthrough
    #[test]
    fn table_dcs_passthrough_esc_can_sub_exit() {
        let esc = TABLE[0x1B][State::DcsPassthrough as usize];
        assert_eq!(esc.state, State::Escape);
        let can = TABLE[0x18][State::DcsPassthrough as usize];
        assert_eq!(can.state, State::Ground);
        let sub = TABLE[0x1A][State::DcsPassthrough as usize];
        assert_eq!(sub.state, State::Ground);
    }

    // osc_string keeps 0x80-0xFF as payload (UTF-8), BEL terminates.
    #[test]
    fn table_osc_string_high_bytes_and_bel() {
        for c in 0x80..=0xFFusize {
            let entry = TABLE[c][State::OscString as usize];
            assert_eq!(entry.state, State::OscString, "byte {:#x}", c);
            assert_eq!(entry.action, TransitionAction::OscPut, "byte {:#x}", c);
        }
        let bel = TABLE[0x07][State::OscString as usize];
        assert_eq!(bel.state, State::Ground);
        assert_eq!(bel.action, TransitionAction::Nop);
    }

    // Unset entries default to "stay put, do nothing".
    #[test]
    fn table_defaults_stay_in_state() {
        // 0x7F (DEL) prints in ground but is ignored elsewhere.
        assert_eq!(
            TABLE[0x7F][State::Ground as usize],
            Effect {
                state: State::Ground,
                action: TransitionAction::Print
            }
        );
        assert_eq!(
            TABLE[0x7F][State::CsiParam as usize],
            Effect {
                state: State::CsiParam,
                action: TransitionAction::Ignore
            }
        );
        // High bytes in escape_intermediate have no entry beyond the
        // anywhere transitions: 0xA0 stays put doing nothing.
        assert_eq!(
            TABLE[0xA0][State::EscapeIntermediate as usize],
            Effect {
                state: State::EscapeIntermediate,
                action: TransitionAction::Nop
            }
        );
    }
}
