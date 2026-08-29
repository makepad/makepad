//! DEC private / ANSI mode registry with save/restore (XTSAVE/XTRESTORE)
//! and DECRQM reporting. Port of ghostty `src/terminal/modes.zig`.
//!
//! LANE CONTRACT: keep the enum + public surface; port the full entry
//! table (already transcribed below), defaults, save/restore semantics
//! (each mode has one save slot), and `report` (DECRPM states; DEC mode
//! 117/DECECM reports permanently_reset like ghostty).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    // ANSI (SM/RM)
    DisableKeyboard,       // 2   KAM
    Insert,                // 4   IRM
    SendReceiveMode,       // 12  SRM, default true
    Linefeed,              // 20  LNM
    // DEC private (DECSET/DECRST)
    CursorKeys,            // ?1   DECCKM
    Column132,             // ?3
    SlowScroll,            // ?4
    ReverseColors,         // ?5   DECSCNM
    Origin,                // ?6   DECOM
    Wraparound,            // ?7   DECAWM, default true
    Autorepeat,            // ?8
    MouseEventX10,         // ?9
    CursorBlinking,        // ?12
    CursorVisible,         // ?25, default true
    EnableMode3,           // ?40
    ReverseWrap,           // ?45
    AltScreenLegacy,       // ?47
    KeypadKeys,            // ?66  DECNKM
    BackarrowKeyMode,      // ?67  DECBKM
    EnableLeftAndRightMargin, // ?69 DECLRMM
    MouseEventNormal,      // ?1000
    MouseEventButton,      // ?1002
    MouseEventAny,         // ?1003
    FocusEvent,            // ?1004
    MouseFormatUtf8,       // ?1005
    MouseFormatSgr,        // ?1006
    MouseAlternateScroll,  // ?1007, default true
    MouseFormatUrxvt,      // ?1015
    MouseFormatSgrPixels,  // ?1016
    IgnoreKeypadWithNumlock, // ?1035, default true
    AltEscPrefix,          // ?1036, default true
    AltSendsEscape,        // ?1039
    ReverseWrapExtended,   // ?1045
    AltScreen,             // ?1047
    SaveCursor,            // ?1048
    AltScreenSaveCursorClearEnter, // ?1049
    BracketedPaste,        // ?2004
    SynchronizedOutput,    // ?2026
    GraphemeCluster,       // ?2027
    ReportColorScheme,     // ?2031
    InBandSizeReports,     // ?2048
}

/// (numeric value, is_ansi, default)
pub fn mode_entry(mode: Mode) -> (u16, bool, bool) {
    use Mode::*;
    match mode {
        DisableKeyboard => (2, true, false),
        Insert => (4, true, false),
        SendReceiveMode => (12, true, true),
        Linefeed => (20, true, false),
        CursorKeys => (1, false, false),
        Column132 => (3, false, false),
        SlowScroll => (4, false, false),
        ReverseColors => (5, false, false),
        Origin => (6, false, false),
        Wraparound => (7, false, true),
        Autorepeat => (8, false, false),
        MouseEventX10 => (9, false, false),
        CursorBlinking => (12, false, false),
        CursorVisible => (25, false, true),
        EnableMode3 => (40, false, false),
        ReverseWrap => (45, false, false),
        AltScreenLegacy => (47, false, false),
        KeypadKeys => (66, false, false),
        BackarrowKeyMode => (67, false, false),
        EnableLeftAndRightMargin => (69, false, false),
        MouseEventNormal => (1000, false, false),
        MouseEventButton => (1002, false, false),
        MouseEventAny => (1003, false, false),
        FocusEvent => (1004, false, false),
        MouseFormatUtf8 => (1005, false, false),
        MouseFormatSgr => (1006, false, false),
        MouseAlternateScroll => (1007, false, true),
        MouseFormatUrxvt => (1015, false, false),
        MouseFormatSgrPixels => (1016, false, false),
        IgnoreKeypadWithNumlock => (1035, false, true),
        AltEscPrefix => (1036, false, true),
        AltSendsEscape => (1039, false, false),
        ReverseWrapExtended => (1045, false, false),
        AltScreen => (1047, false, false),
        SaveCursor => (1048, false, false),
        AltScreenSaveCursorClearEnter => (1049, false, false),
        BracketedPaste => (2004, false, false),
        SynchronizedOutput => (2026, false, false),
        GraphemeCluster => (2027, false, false),
        ReportColorScheme => (2031, false, false),
        InBandSizeReports => (2048, false, false),
    }
}

/// Every mode, in declaration order. `ALL_MODES[i] as usize == i` holds (see
/// tests), which is what lets the state below key its bit sets on the enum
/// discriminant.
const ALL_MODES: [Mode; MODE_COUNT] = {
    use Mode::*;
    [
        DisableKeyboard,
        Insert,
        SendReceiveMode,
        Linefeed,
        CursorKeys,
        Column132,
        SlowScroll,
        ReverseColors,
        Origin,
        Wraparound,
        Autorepeat,
        MouseEventX10,
        CursorBlinking,
        CursorVisible,
        EnableMode3,
        ReverseWrap,
        AltScreenLegacy,
        KeypadKeys,
        BackarrowKeyMode,
        EnableLeftAndRightMargin,
        MouseEventNormal,
        MouseEventButton,
        MouseEventAny,
        FocusEvent,
        MouseFormatUtf8,
        MouseFormatSgr,
        MouseAlternateScroll,
        MouseFormatUrxvt,
        MouseFormatSgrPixels,
        IgnoreKeypadWithNumlock,
        AltEscPrefix,
        AltSendsEscape,
        ReverseWrapExtended,
        AltScreen,
        SaveCursor,
        AltScreenSaveCursorClearEnter,
        BracketedPaste,
        SynchronizedOutput,
        GraphemeCluster,
        ReportColorScheme,
        InBandSizeReports,
    ]
};

/// Number of modes. Must stay <= 64 so a mode set fits in one u64.
const MODE_COUNT: usize = 41;

const _: () = assert!(MODE_COUNT <= 64);

/// DEC private mode 117 (DECECM, Erase Color Mode). We don't implement it --
/// our behaviour is fixed at the DECECM-reset equivalent -- but DECRQM has a
/// "permanently reset" response for exactly this case, so applications can
/// query it and adapt instead of getting "not recognized".
///
/// See VT520/VT525 Programmer Information, "Erase Color" and DECRQM/DECRPM:
/// <https://web.mit.edu/dosathena/doc/www/ek-vt520-rm.pdf>
const DECECM: u16 = 117;

#[inline]
fn mode_bit(mode: Mode) -> u64 {
    1u64 << (mode as u32)
}

/// Bit set of the modes whose entry default is true.
fn default_bits() -> u64 {
    let mut bits = 0u64;
    let mut i = 0;
    while i < MODE_COUNT {
        let mode = ALL_MODES[i];
        if mode_entry(mode).2 {
            bits |= mode_bit(mode);
        }
        i += 1;
    }
    bits
}

pub fn mode_from_int(value: u16, ansi: bool) -> Option<Mode> {
    let mut i = 0;
    while i < MODE_COUNT {
        let mode = ALL_MODES[i];
        let (v, a, _) = mode_entry(mode);
        if v == value && a == ansi {
            return Some(mode);
        }
        i += 1;
    }
    None
}

/// DECRPM report states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeReport {
    NotRecognized,     // 0
    Set,               // 1
    Reset,             // 2
    PermanentlySet,    // 3
    PermanentlyReset,  // 4
}

impl ModeReport {
    pub fn param(self) -> u8 {
        match self {
            ModeReport::NotRecognized => 0,
            ModeReport::Set => 1,
            ModeReport::Reset => 2,
            ModeReport::PermanentlySet => 3,
            ModeReport::PermanentlyReset => 4,
        }
    }
}

/// The state of all settable modes: current values, one save slot per mode
/// (XTSAVE/XTRESTORE), and the reset defaults.
///
/// Only one save slot per mode exists, matching other terminals that implement
/// XTSAVE/XTRESTORE -- unbounded save stacks are a DoS vector. Saving twice
/// then restoring twice therefore yields the same value both times.
#[derive(Clone, Debug)]
pub struct ModeState {
    /// Current value of each mode, keyed by `Mode as u32`.
    values: u64,
    /// The saved values (XTSAVE). Reset clears these back to the defaults.
    saved: u64,
    /// The values `reset` returns to.
    default: u64,
}

impl Default for ModeState {
    fn default() -> Self {
        Self::new()
    }
}

impl ModeState {
    pub fn new() -> Self {
        let default = default_bits();
        Self { values: default, saved: default, default }
    }

    pub fn get(&self, mode: Mode) -> bool {
        self.values & mode_bit(mode) != 0
    }

    pub fn set(&mut self, mode: Mode, value: bool) {
        if value {
            self.values |= mode_bit(mode);
        } else {
            self.values &= !mode_bit(mode);
        }
    }

    /// XTSAVE: remember the current value (one slot per mode).
    pub fn save(&mut self, mode: Mode) {
        let bit = mode_bit(mode);
        self.saved = (self.saved & !bit) | (self.values & bit);
    }

    /// XTRESTORE: restore the saved value, returning it.
    pub fn restore(&mut self, mode: Mode) -> bool {
        let bit = mode_bit(mode);
        self.values = (self.values & !bit) | (self.saved & bit);
        self.values & bit != 0
    }

    /// Reset all modes to defaults, clearing saved state (RIS/DECSTR use).
    pub fn reset(&mut self) {
        self.values = self.default;
        self.saved = self.default;
    }

    /// DECRQM report for a raw (value, ansi) tag.
    pub fn report(&self, value: u16, ansi: bool) -> ModeReport {
        if !ansi && value == DECECM {
            return ModeReport::PermanentlyReset;
        }
        match mode_from_int(value, ansi) {
            None => ModeReport::NotRecognized,
            Some(mode) => {
                if self.get(mode) {
                    ModeReport::Set
                } else {
                    ModeReport::Reset
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The bit sets key on `Mode as u32`, so the declaration order of the enum
    // and of ALL_MODES must agree and be gap-free.
    #[test]
    fn all_modes_matches_declaration_order() {
        for (i, mode) in ALL_MODES.iter().enumerate() {
            assert_eq!(*mode as usize, i, "ALL_MODES[{}] = {:?}", i, mode);
        }
        assert_eq!(ALL_MODES.len(), MODE_COUNT);
    }

    // ghostty: test modeFromInt
    #[test]
    fn mode_from_int_ghostty() {
        assert_eq!(mode_from_int(4, true), Some(Mode::Insert));
        assert_eq!(mode_from_int(9, true), None);
        assert_eq!(mode_from_int(9, false), Some(Mode::MouseEventX10));
        assert_eq!(mode_from_int(14, true), None);
    }

    // The ansi flag is what disambiguates the two colliding numbers.
    #[test]
    fn mode_from_int_ansi_disambiguates() {
        assert_eq!(mode_from_int(4, true), Some(Mode::Insert));
        assert_eq!(mode_from_int(4, false), Some(Mode::SlowScroll));
        assert_eq!(mode_from_int(12, true), Some(Mode::SendReceiveMode));
        assert_eq!(mode_from_int(12, false), Some(Mode::CursorBlinking));
    }

    #[test]
    fn mode_from_int_unknown() {
        assert_eq!(mode_from_int(9999, false), None);
        assert_eq!(mode_from_int(9999, true), None);
        // DECECM is deliberately not a mode; it only exists in `report`.
        assert_eq!(mode_from_int(117, false), None);
        // DEC numbers are not ANSI numbers.
        assert_eq!(mode_from_int(2004, true), None);
        assert_eq!(mode_from_int(0, false), None);
    }

    #[test]
    fn every_mode_round_trips() {
        for mode in ALL_MODES {
            let (value, ansi, _) = mode_entry(mode);
            assert_eq!(
                mode_from_int(value, ansi),
                Some(mode),
                "round trip failed for {:?} ({}{})",
                mode,
                if ansi { "" } else { "?" },
                value,
            );
        }
    }

    #[test]
    fn defaults_are_exactly_the_documented_set() {
        let expected = [
            Mode::SendReceiveMode,
            Mode::Wraparound,
            Mode::CursorVisible,
            Mode::MouseAlternateScroll,
            Mode::IgnoreKeypadWithNumlock,
            Mode::AltEscPrefix,
        ];
        let state = ModeState::new();
        for mode in ALL_MODES {
            let want = expected.contains(&mode);
            assert_eq!(mode_entry(mode).2, want, "entry default for {:?}", mode);
            assert_eq!(state.get(mode), want, "initial value for {:?}", mode);
        }
    }

    // ghostty: test ModeState
    #[test]
    fn set_get_save_restore() {
        let mut state = ModeState::new();

        // Normal set/get
        assert!(!state.get(Mode::CursorKeys));
        state.set(Mode::CursorKeys, true);
        assert!(state.get(Mode::CursorKeys));

        // Save/restore
        state.save(Mode::CursorKeys);
        state.set(Mode::CursorKeys, false);
        assert!(!state.get(Mode::CursorKeys));
        assert!(state.restore(Mode::CursorKeys));
        assert!(state.get(Mode::CursorKeys));
    }

    #[test]
    fn save_restore_is_per_mode() {
        let mut state = ModeState::new();
        state.set(Mode::CursorKeys, true);
        state.set(Mode::BracketedPaste, true);
        state.save(Mode::CursorKeys);

        // Saving one mode must not capture another.
        state.set(Mode::CursorKeys, false);
        state.set(Mode::BracketedPaste, false);
        assert!(state.restore(Mode::CursorKeys));
        assert!(!state.restore(Mode::BracketedPaste));
        assert!(state.get(Mode::CursorKeys));
        assert!(!state.get(Mode::BracketedPaste));

        // A default-set mode restores to its default when never saved.
        state.set(Mode::Wraparound, false);
        assert!(state.restore(Mode::Wraparound));
    }

    #[test]
    fn reset_restores_defaults_and_clears_saved() {
        let mut state = ModeState::new();

        state.set(Mode::GraphemeCluster, true);
        state.save(Mode::GraphemeCluster);
        state.set(Mode::Wraparound, false);
        state.save(Mode::Wraparound);
        state.set(Mode::CursorVisible, false);

        state.reset();

        // Current values are back to the defaults...
        assert!(!state.get(Mode::GraphemeCluster));
        assert!(state.get(Mode::Wraparound));
        assert!(state.get(Mode::CursorVisible));

        // ...and so are the saved slots.
        assert!(!state.restore(Mode::GraphemeCluster));
        assert!(state.restore(Mode::Wraparound));
    }

    // ghostty: test "getReport known DEC mode"
    #[test]
    fn report_known_dec_mode() {
        let mut state = ModeState::new();
        assert_eq!(state.report(1, false), ModeReport::Reset);
        state.set(Mode::CursorKeys, true);
        assert_eq!(state.report(1, false), ModeReport::Set);
    }

    // ghostty: test "getReport known ANSI mode"
    #[test]
    fn report_known_ansi_mode() {
        let mut state = ModeState::new();
        state.set(Mode::Insert, true);
        assert_eq!(state.report(4, true), ModeReport::Set);
        // Same number, DEC private: a different mode, still reset.
        assert_eq!(state.report(4, false), ModeReport::Reset);
    }

    // ghostty: test "getReport DECECM permanently reset"
    #[test]
    fn report_dececm_permanently_reset() {
        let mut state = ModeState::new();
        assert_eq!(state.report(117, false), ModeReport::PermanentlyReset);
        // Only as a DEC private mode; ANSI 117 is nothing.
        assert_eq!(state.report(117, true), ModeReport::NotRecognized);
        // It stays permanently reset regardless of other state.
        state.set(Mode::CursorKeys, true);
        assert_eq!(state.report(117, false), ModeReport::PermanentlyReset);
    }

    // ghostty: test "getReport unknown mode"
    #[test]
    fn report_unknown_mode() {
        let state = ModeState::new();
        assert_eq!(state.report(9999, false), ModeReport::NotRecognized);
        assert_eq!(state.report(9999, true), ModeReport::NotRecognized);
    }

    #[test]
    fn report_defaults_report_set() {
        let state = ModeState::new();
        assert_eq!(state.report(12, true), ModeReport::Set); // SRM
        assert_eq!(state.report(7, false), ModeReport::Set); // DECAWM
        assert_eq!(state.report(25, false), ModeReport::Set); // DECTCEM
        assert_eq!(state.report(12, false), ModeReport::Reset); // cursor blinking
    }

    // The DECRPM response parameter for each state (ghostty Report.encode
    // writes this number; the sequence assembly itself lives in the caller).
    #[test]
    fn report_params() {
        assert_eq!(ModeReport::NotRecognized.param(), 0);
        assert_eq!(ModeReport::Set.param(), 1);
        assert_eq!(ModeReport::Reset.param(), 2);
        assert_eq!(ModeReport::PermanentlySet.param(), 3);
        assert_eq!(ModeReport::PermanentlyReset.param(), 4);
    }
}
