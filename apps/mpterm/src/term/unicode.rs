//! Codepoint width and grapheme-cluster break rules for terminal cells.
//!
//! Port of ghostty `src/unicode/` semantics (which uses generated uucode
//! tables): cell width clamped to [0,2], and UAX #29 extended grapheme
//! cluster boundary rules for mode 2027 grapheme clustering.
//!
//! LANE CONTRACT: implement with embedded static range tables (sorted
//! ranges + binary search is fine). Required behaviors:
//! - `char_width`: 0 for zero-width (Mn/Me combining, Cf format chars incl.
//!   ZWJ U+200D and ZWNJ, U+200B, Hangul jungseong/jongseong V/T jamo),
//!   2 for East Asian Wide + Fullwidth (incl. emoji presentation range
//!   U+1F300.. blocks, CJK, Hangul syllables), 1 otherwise. Control chars
//!   never reach this (stream handles C0/C1). Soft hyphen U+00AD is 1
//!   (matches ghostty/uucode).
//! - `grapheme_break(before, after, state)`: UAX #29 rules ported from
//!   ghostty `src/unicode/grapheme.zig` including GB9b/GB11 (extended
//!   pictographic ZWJ sequences, needs the state), GB12/13 (regional
//!   indicator pairs, needs the state), Hangul L/V/T composition.
//! Port ghostty's grapheme break tests where feasible.
//!
//! ## Data
//!
//! The range tables in `unicode_tables.rs` are generated from the Unicode
//! Character Database 15.1.0 by `local/vendor/ucd/gen_tables.py`; that
//! script documents which UCD property maps to which table. Nothing here
//! allocates and nothing here needs an external crate.
//!
//! GB9c (Indic conjunct breaks, added in Unicode 15.1) is implemented as
//! well, so a consonant-virama-consonant conjunct stays one cluster.
//!
//! Checked against `auxiliary/GraphemeBreakTest.txt`: every conformance
//! line that does not contain a Control/CR/LF codepoint passes, bar the two
//! that exercise the deliberate emoji-modifier deviation below.
//!
//! ## Deviations from a literal UAX #29 reading
//!
//! Two, both to match ghostty (see `src/build/uucode_config.zig` and the
//! tests at the bottom of `src/unicode/grapheme.zig`):
//!
//! - Emoji modifiers (skin tones, U+1F3FB..U+1F3FF) have
//!   Grapheme_Cluster_Break=Extend, so UAX #29 glues them to whatever
//!   precedes. Ghostty instead joins them only to an Emoji_Modifier_Base,
//!   and gives a lone modifier width 2, so that a stray skin tone renders
//!   as its own colour patch instead of vanishing into the previous cell.
//! - GB3/GB4/GB5 (the CR, LF and Control rules) are absent, as they are in
//!   ghostty: the VT stream layer dispatches C0/C1 as control functions, so
//!   they never reach the printer. Controls classify as `Other` here, which
//!   is what ghostty's `grapheme_break_no_control` does.

include!("unicode_tables.rs");

/// True when `cp` falls inside one of the sorted, non-overlapping inclusive
/// ranges in `table`.
fn range_contains(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(first, last)| {
            if cp < first {
                core::cmp::Ordering::Greater
            } else if cp > last {
                core::cmp::Ordering::Less
            } else {
                core::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Terminal cell width of a codepoint: 0, 1 or 2.
pub fn char_width(cp: u32) -> u8 {
    // C0, DEL and C1 never reach the printer (the stream layer dispatches
    // them as control functions); report no cell width, like ghostty's
    // wcwidth clamped into [0,2].
    if cp < 0x20 || (0x7F..=0x9F).contains(&cp) {
        return 0;
    }
    // The overwhelmingly common case: printable ASCII.
    if cp < 0x7F {
        return 1;
    }
    if range_contains(&ZERO, cp) {
        return 0;
    }
    if range_contains(&WIDE, cp) {
        return 2;
    }
    1
}

/// Grapheme_Cluster_Break property class, as ghostty's uucode tables model
/// it: one value per codepoint, with Extended_Pictographic folded in where
/// the break property itself is `Other`, and CR/LF folded into `Control`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphemeClass {
    Other,
    /// Control, CR and LF. The printer never sees these; they behave
    /// exactly like `Other` in the rules below, matching ghostty's
    /// `grapheme_break_no_control`.
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
    ExtendedPictographic,
    /// Emoji_Modifier. UAX #29 calls these Extend; ghostty splits them out
    /// so a skin tone only joins an Emoji_Modifier_Base.
    EmojiModifier,
}

fn grapheme_class(cp: u32) -> GraphemeClass {
    use GraphemeClass::*;

    // ASCII and Latin-1: only controls are interesting, combining marks
    // start at U+0300.
    if cp < 0x300 {
        return if cp < 0x20 || (0x7F..=0x9F).contains(&cp) || cp == 0xAD {
            Control
        } else {
            Other
        };
    }

    // The precomposed Hangul syllables are the only LV/LVT codepoints, and
    // they are the two biggest tables (they interleave every 28 codepoints),
    // so gate them behind their block instead of searching them for
    // everything.
    if (0xAC00..=0xD7A3).contains(&cp) {
        if range_contains(&GB_LV, cp) {
            return Lv;
        }
        if range_contains(&GB_LVT, cp) {
            return Lvt;
        }
        return Other;
    }

    if range_contains(&GB_ZWJ, cp) {
        return Zwj;
    }
    // Before Extend: the skin tones live in the Extend table.
    if range_contains(&EMOJI_MODIFIER, cp) {
        return EmojiModifier;
    }
    if range_contains(&GB_EXTEND, cp) {
        return Extend;
    }
    if range_contains(&GB_SPACINGMARK, cp) {
        return SpacingMark;
    }
    if range_contains(&GB_PREPEND, cp) {
        return Prepend;
    }
    if range_contains(&GB_REGIONAL_INDICATOR, cp) {
        return RegionalIndicator;
    }
    if range_contains(&GB_L, cp) {
        return L;
    }
    if range_contains(&GB_V, cp) {
        return V;
    }
    if range_contains(&GB_T, cp) {
        return T;
    }
    if range_contains(&GB_CONTROL, cp) {
        return Control;
    }
    // Only where the break property is Other, which the generator already
    // guaranteed by trimming the overlap out of this table.
    if range_contains(&GB_EXTENDED_PICTOGRAPHIC, cp) {
        return ExtendedPictographic;
    }
    Other
}

/// Opaque state for the incremental grapheme break iterator (tracks
/// extended-pictographic ZWJ and regional-indicator parity like ghostty's
/// `BreakState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphemeState {
    pub bits: u8,
}

/// An Extended_Pictographic has been seen in this cluster, so a following
/// ZWJ may glue another one on (GB11).
const STATE_EXTENDED_PICTOGRAPHIC: u8 = 1 << 0;
/// The previous codepoint closed a regional-indicator pair, so the next
/// regional indicator starts a new flag (GB12/GB13).
const STATE_REGIONAL_INDICATOR: u8 = 1 << 1;
/// An Indic_Conjunct_Break=Consonant has been seen, possibly followed by
/// InCB=Extend codepoints, but no linker yet (GB9c).
const STATE_INCB_CONSONANT: u8 = 1 << 2;
/// ... and a linker has since been seen, so a following consonant joins.
const STATE_INCB_LINKER: u8 = 1 << 3;

impl GraphemeState {
    /// Back to the state a cluster starts in.
    pub fn reset(&mut self) {
        self.bits = 0;
    }

    fn get(&self, mask: u8) -> bool {
        self.bits & mask != 0
    }

    fn set(&mut self, mask: u8, on: bool) {
        if on {
            self.bits |= mask;
        } else {
            self.bits &= !mask;
        }
    }
}

/// True when an extended grapheme cluster boundary exists between `cp1`
/// (previous) and `cp2` (next). `state` must persist across consecutive
/// calls over a run of text and be reset at cluster starts.
pub fn grapheme_break(cp1: u32, cp2: u32, state: &mut GraphemeState) -> bool {
    use GraphemeClass::*;

    let c1 = grapheme_class(cp1);
    let c2 = grapheme_class(cp2);

    // GB11 bookkeeping: remember an Extended_Pictographic so that a later
    // `ZWJ x Extended_Pictographic` can join across intervening Extends.
    if !state.get(STATE_EXTENDED_PICTOGRAPHIC) && c1 == ExtendedPictographic {
        state.set(STATE_EXTENDED_PICTOGRAPHIC, true);
    }

    // GB9c bookkeeping: walk the
    // `Consonant [Extend | Linker]* Linker [Extend | Linker]*` prefix. Each
    // codepoint of a run reaches this as `cp1` exactly once.
    let in_conjunct = state.bits & (STATE_INCB_CONSONANT | STATE_INCB_LINKER) != 0;
    if cp1 < 0x300 {
        // Nothing below U+0300 carries an InCB value, so this ends the run.
        state.set(STATE_INCB_CONSONANT | STATE_INCB_LINKER, false);
    } else if range_contains(&INCB_CONSONANT, cp1) {
        state.set(STATE_INCB_CONSONANT, true);
        state.set(STATE_INCB_LINKER, false);
    } else if in_conjunct && range_contains(&INCB_LINKER, cp1) {
        state.set(STATE_INCB_LINKER, true);
    } else if !(in_conjunct && range_contains(&INCB_EXTEND, cp1)) {
        // Anything else ends the prefix. InCB=Extend leaves it standing.
        state.set(STATE_INCB_CONSONANT | STATE_INCB_LINKER, false);
    }

    // GB3/GB4/GB5 (CR LF and controls) are deliberately absent: this
    // function is never called with them, exactly as in ghostty.

    // GB6: L x (L | V | LV | LVT)
    if c1 == L && matches!(c2, L | V | Lv | Lvt) {
        return false;
    }

    // GB7: (LV | V) x (V | T)
    if matches!(c1, Lv | V) && matches!(c2, V | T) {
        return false;
    }

    // GB8: (LVT | T) x T
    if matches!(c1, Lvt | T) && c2 == T {
        return false;
    }

    // GB9b: Prepend x any
    if c1 == Prepend {
        return false;
    }

    // GB9a: any x SpacingMark
    if c2 == SpacingMark {
        return false;
    }

    // GB9: any x (Extend | ZWJ)
    if matches!(c2, Extend | Zwj) {
        return false;
    }

    // GB9c: Consonant [Extend | Linker]* Linker [Extend | Linker]* x Consonant
    if state.get(STATE_INCB_LINKER) && range_contains(&INCB_CONSONANT, cp2) {
        return false;
    }

    // Ghostty's refinement of GB9 for skin tones: a modifier only joins an
    // Emoji_Modifier_Base, so a lone one stands (and paints) by itself.
    if c2 == EmojiModifier {
        return !range_contains(&EMOJI_MODIFIER_BASE, cp1);
    }

    // GB11: Extended_Pictographic Extend* ZWJ x Extended_Pictographic
    if state.get(STATE_EXTENDED_PICTOGRAPHIC) && c1 == Zwj && c2 == ExtendedPictographic {
        state.set(STATE_EXTENDED_PICTOGRAPHIC, false);
        return false;
    }

    // GB12/GB13: regional indicators pair up, so break on every second one.
    if c1 == RegionalIndicator && c2 == RegionalIndicator {
        let paired = state.get(STATE_REGIONAL_INDICATOR);
        state.set(STATE_REGIONAL_INDICATOR, !paired);
        return paired;
    }

    // GB999: otherwise, break.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joins(cp1: u32, cp2: u32) -> bool {
        let mut state = GraphemeState::default();
        !grapheme_break(cp1, cp2, &mut state)
    }

    /// Segment a codepoint run the way a printer with mode 2027 would:
    /// carry the break state along a cluster, reset it at each new one.
    fn clusters(cps: &[u32]) -> Vec<Vec<u32>> {
        let mut out: Vec<Vec<u32>> = Vec::new();
        let mut state = GraphemeState::default();
        for (i, &cp) in cps.iter().enumerate() {
            if i == 0 || grapheme_break(cps[i - 1], cp, &mut state) {
                state.reset();
                out.push(vec![cp]);
            } else {
                out.last_mut().unwrap().push(cp);
            }
        }
        out
    }

    /// Width of a whole cluster, the way ghostty's `graphemeWidth` adds up:
    /// the widest contributing codepoint wins, clamped to 2.
    fn cluster_widths(cps: &[u32]) -> Vec<u8> {
        clusters(cps)
            .iter()
            .map(|c| c.iter().map(|&cp| char_width(cp)).max().unwrap_or(0))
            .collect()
    }

    #[test]
    fn tables_are_sorted_and_merged() {
        for (name, table) in [
            ("WIDE", &WIDE[..]),
            ("ZERO", &ZERO[..]),
            ("GB_EXTEND", &GB_EXTEND[..]),
            ("GB_ZWJ", &GB_ZWJ[..]),
            ("GB_SPACINGMARK", &GB_SPACINGMARK[..]),
            ("GB_PREPEND", &GB_PREPEND[..]),
            ("GB_REGIONAL_INDICATOR", &GB_REGIONAL_INDICATOR[..]),
            ("GB_L", &GB_L[..]),
            ("GB_V", &GB_V[..]),
            ("GB_T", &GB_T[..]),
            ("GB_LV", &GB_LV[..]),
            ("GB_LVT", &GB_LVT[..]),
            ("GB_CONTROL", &GB_CONTROL[..]),
            ("GB_EXTENDED_PICTOGRAPHIC", &GB_EXTENDED_PICTOGRAPHIC[..]),
            ("EMOJI_MODIFIER", &EMOJI_MODIFIER[..]),
            ("EMOJI_MODIFIER_BASE", &EMOJI_MODIFIER_BASE[..]),
            ("INCB_CONSONANT", &INCB_CONSONANT[..]),
            ("INCB_LINKER", &INCB_LINKER[..]),
            ("INCB_EXTEND", &INCB_EXTEND[..]),
        ] {
            assert!(!table.is_empty(), "{name} is empty");
            for (i, &(first, last)) in table.iter().enumerate() {
                assert!(first <= last, "{name}[{i}] is inverted");
                assert!(last <= 0x10FFFF, "{name}[{i}] is out of range");
                if i > 0 {
                    // Strictly increasing with a gap: adjacent ranges would
                    // mean the generator failed to merge them.
                    assert!(
                        table[i - 1].1 + 1 < first,
                        "{name}[{i}] touches or overlaps its predecessor"
                    );
                }
            }
        }
    }

    /// `grapheme_class` shortcuts the two big Hangul tables by testing the
    /// syllable block and treating "not LV" as LVT; that only holds if the
    /// two tables together tile U+AC00..U+D7A3 exactly.
    #[test]
    fn hangul_syllable_block_is_exactly_lv_plus_lvt() {
        for cp in 0xAC00..=0xD7A3u32 {
            assert!(
                range_contains(&GB_LV, cp) != range_contains(&GB_LVT, cp),
                "U+{cp:04X} is not exactly one of LV / LVT"
            );
        }
        for cp in [0xABFF, 0xD7A4] {
            assert!(!range_contains(&GB_LV, cp) && !range_contains(&GB_LVT, cp));
        }
    }

    // ------------------------------------------------------------- width

    #[test]
    fn width_narrow() {
        assert_eq!(char_width('a' as u32), 1);
        assert_eq!(char_width(' ' as u32), 1);
        assert_eq!(char_width('~' as u32), 1);
        assert_eq!(char_width(0x00A0), 1); // no-break space
        assert_eq!(char_width(0x00E9), 1); // é
        assert_eq!(char_width(0x2502), 1); // box drawing, East Asian ambiguous
        assert_eq!(char_width(0x1F1E6 - 1), 1); // just below the flag block
    }

    #[test]
    fn width_wide() {
        assert_eq!(char_width('漢' as u32), 2); // U+6F22, East Asian Wide
        assert_eq!(char_width(0xFF21), 2); // fullwidth A
        assert_eq!(char_width(0x3042), 2); // hiragana A
        assert_eq!(char_width(0xAC00), 2); // Hangul syllable GA
        assert_eq!(char_width(0x1F44D), 2); // 👍 emoji presentation
        assert_eq!(char_width(0x1F1E6), 2); // regional indicator A
        assert_eq!(char_width(0x231A), 2); // ⌚ emoji presentation
        assert_eq!(char_width(0x1F3FF), 2); // skin tone, standalone patch
        assert_eq!(char_width(0x4E00), 2); // CJK ideograph
    }

    #[test]
    fn width_zero() {
        assert_eq!(char_width(0), 0);
        assert_eq!(char_width(0x0301), 0); // combining acute
        assert_eq!(char_width(0x200D), 0); // ZWJ
        assert_eq!(char_width(0x200C), 0); // ZWNJ
        assert_eq!(char_width(0x200B), 0); // zero width space
        assert_eq!(char_width(0x2060), 0); // word joiner
        assert_eq!(char_width(0xFE0F), 0); // variation selector 16
        assert_eq!(char_width(0xFE0E), 0); // variation selector 15
        assert_eq!(char_width(0xFEFF), 0); // BOM / ZWNBSP
        assert_eq!(char_width(0x20E3), 0); // combining enclosing keycap (Me)
        assert_eq!(char_width(0x1160), 0); // Hangul jungseong filler (V)
        assert_eq!(char_width(0x11A8), 0); // Hangul jongseong kiyeok (T)
        assert_eq!(char_width(0xD7B0), 0); // Hangul Jamo Extended-B, V
        assert_eq!(char_width(0xE0100), 0); // variation selector 17
    }

    #[test]
    fn width_ghostty_exceptions() {
        // Soft hyphen is Cf but ghostty/uucode give it a cell.
        assert_eq!(char_width(0x00AD), 1);
        // Prepend codepoints keep their standalone width even though they
        // are Cf, so they do not vanish from the screen.
        assert_eq!(char_width(0x0600), 1); // Arabic number sign
        assert_eq!(char_width(0x110BD), 1); // Kaithi number sign
        // Emoji modifiers are Extend but paint as a colour patch alone.
        assert_eq!(char_width(0x1F3FB), 2);
    }

    #[test]
    fn width_is_always_clamped() {
        for cp in 0..0x11000u32 {
            assert!(char_width(cp) <= 2, "U+{cp:04X}");
        }
    }

    // ----------------------------------------------------- grapheme break

    #[test]
    fn break_plain_text() {
        assert!(!joins('a' as u32, 'b' as u32));
        assert!(!joins('漢' as u32, '字' as u32));
        assert_eq!(clusters(&['a' as u32, 'b' as u32]).len(), 2);
    }

    #[test]
    fn break_combining_marks() {
        // GB9: e + combining acute is one cluster.
        assert!(joins('e' as u32, 0x0301));
        assert_eq!(clusters(&['e' as u32, 0x0301]), vec![vec!['e' as u32, 0x0301]]);
        // Several marks in a row keep extending it.
        let cps = ['e' as u32, 0x0301, 0x0308, 'x' as u32];
        assert_eq!(clusters(&cps).len(), 2);
        assert_eq!(cluster_widths(&cps), vec![1, 1]);
        // GB9a: a spacing mark attaches to the base.
        assert!(joins(0x0915, 0x093E)); // ka + vowel sign aa
        // GB9b: a prepend attaches to what follows.
        assert!(joins(0x0600, 0x0661)); // Arabic number sign + digit one
    }

    #[test]
    fn break_hangul() {
        // GB6: L x V
        assert!(joins(0x1100, 0x1161));
        // GB6: L x L, L x LV, L x LVT
        assert!(joins(0x1100, 0x1100));
        assert!(joins(0x1100, 0xAC00));
        assert!(joins(0x1100, 0xAC01));
        // GB7: V x V, V x T, LV x V, LV x T
        assert!(joins(0x1161, 0x1161));
        assert!(joins(0x1161, 0x11A8));
        assert!(joins(0xAC00, 0x1161));
        assert!(joins(0xAC00, 0x11A8));
        // GB8: LVT x T, T x T
        assert!(joins(0xAC01, 0x11A8));
        assert!(joins(0x11A8, 0x11A8));
        // But not the other way around.
        assert!(!joins(0x1161, 0x1100)); // V then L
        assert!(!joins(0x11A8, 0x1161)); // T then V
        // A full syllable spelled in jamo is one cluster, one cell.
        let cps = [0x1100, 0x1161, 0x11A8];
        assert_eq!(clusters(&cps).len(), 1);
        assert_eq!(cluster_widths(&cps), vec![2]);
    }

    /// GB9c: a Devanagari conjunct is one cluster, so क्त does not fall
    /// apart into two cells. Cases taken from GraphemeBreakTest.txt.
    #[test]
    fn break_indic_conjuncts() {
        // ka + virama + ta
        assert_eq!(clusters(&[0x0915, 0x094D, 0x0924]).len(), 1);
        // a doubled virama still links
        assert_eq!(clusters(&[0x0915, 0x094D, 0x094D, 0x0924]).len(), 1);
        // a ZWJ between the linker and the consonant is InCB=Extend
        assert_eq!(clusters(&[0x0915, 0x094D, 0x200D, 0x0924]).len(), 1);
        // nukta (InCB=Extend) before the linker
        assert_eq!(clusters(&[0x0915, 0x093C, 0x094D, 0x0924]).len(), 1);
        // chains restart at every consonant
        assert_eq!(clusters(&[0x0915, 0x094D, 0x0924, 0x094D, 0x092F]).len(), 1);
        // without a linker the two consonants are separate clusters
        assert_eq!(clusters(&[0x0915, 0x0924]).len(), 2);
        // and a consonant that follows something else does not join
        assert_eq!(clusters(&['a' as u32, 0x094D, 0x0924]).len(), 2);
        // the whole conjunct occupies one cell
        assert_eq!(cluster_widths(&[0x0915, 0x094D, 0x0924]), vec![1]);
    }

    #[test]
    fn break_regional_indicators() {
        // Two regional indicators make a flag, a third starts a new one.
        let mut state = GraphemeState::default();
        assert!(!grapheme_break(0x1F1E6, 0x1F1E7, &mut state));
        assert!(grapheme_break(0x1F1E7, 0x1F1E8, &mut state));

        assert_eq!(clusters(&[0x1F1E6, 0x1F1E7]).len(), 1);
        assert_eq!(clusters(&[0x1F1E6, 0x1F1E7, 0x1F1E8]).len(), 2);
        assert_eq!(clusters(&[0x1F1E6, 0x1F1E7, 0x1F1E8, 0x1F1E9]).len(), 2);
        assert_eq!(clusters(&[0x1F1E6, 0x1F1E7, 0x1F1E8, 0x1F1E9, 0x1F1EA]).len(), 3);
        // A flag is two cells wide; so is a lone half of one.
        assert_eq!(cluster_widths(&[0x1F1E6, 0x1F1E7, 0x1F1E8]), vec![2, 2]);
        // Text either side does not confuse the parity.
        assert_eq!(
            clusters(&['x' as u32, 0x1F1E6, 0x1F1E7, 'y' as u32]).len(),
            3
        );
    }

    #[test]
    fn break_emoji_zwj_sequences() {
        // 👨‍👩‍👧 family: man ZWJ woman ZWJ girl is a single cluster.
        let family = [0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F467];
        assert_eq!(clusters(&family).len(), 1);
        assert_eq!(cluster_widths(&family), vec![2]);

        // Emoji + variation selector + ZWJ + emoji + variation selector.
        let pirate = [0x1F3F4, 0x200D, 0x2620, 0xFE0F];
        assert_eq!(clusters(&pirate).len(), 1);

        // A ZWJ that is not between pictographs still attaches (GB9), but
        // does not glue the following letter on.
        assert_eq!(clusters(&['a' as u32, 0x200D, 'b' as u32]).len(), 2);
    }

    #[test]
    fn break_keycap_and_variation_selectors() {
        // # + VS16 + combining enclosing keycap is one cluster.
        assert_eq!(clusters(&['#' as u32, 0xFE0F, 0x20E3]).len(), 1);
        // Variation selectors never start a cluster of their own.
        assert!(joins('x' as u32, 0xFE0F));
        assert!(joins(0x2764, 0xFE0F));
    }

    /// Ported from ghostty `src/unicode/grapheme.zig`,
    /// `test "grapheme break: emoji modifier"`.
    #[test]
    fn ghostty_grapheme_break_emoji_modifier() {
        // Emoji and modifier.
        let mut state = GraphemeState::default();
        assert!(!grapheme_break(0x261D, 0x1F3FF, &mut state));

        // Non-emoji and emoji modifier.
        let mut state = GraphemeState::default();
        assert!(grapheme_break(0x22, 0x1F3FF, &mut state));
    }

    /// Ported from ghostty `src/unicode/grapheme.zig`,
    /// `test "long emoji zwj sequences"`: 👩‍👩‍👧‍👦 followed by `_`.
    #[test]
    fn ghostty_long_emoji_zwj_sequences() {
        let cps = [
            0x1F469, 0x200D, 0x1F469, 0x200D, 0x1F467, 0x200D, 0x1F466, '_' as u32,
        ];
        let mut state = GraphemeState::default();
        for i in 0..cps.len() - 2 {
            assert!(
                !grapheme_break(cps[i], cps[i + 1], &mut state),
                "unexpected break at {i}"
            );
        }
        // The trailing `_` breaks.
        assert!(grapheme_break(cps[cps.len() - 2], cps[cps.len() - 1], &mut state));

        assert_eq!(clusters(&cps).len(), 2);
        assert_eq!(cluster_widths(&cps), vec![2, 1]);
    }

    /// Ported from ghostty `src/unicode/grapheme.zig`, the segmentation and
    /// emoji-sequence halves of its `graphemeWidth` tests. The width-effect
    /// rules for VS15/VS16 (which can narrow a cluster) live in the printer,
    /// not here, so only the segmentation and the widest-codepoint width are
    /// checked.
    #[test]
    fn ghostty_grapheme_width_segmentation() {
        assert_eq!(cluster_widths(&['a' as u32]), vec![1]);
        assert_eq!(cluster_widths(&['a' as u32, 'b' as u32]), vec![1, 1]);
        assert_eq!(cluster_widths(&[0x1F1E6, 0x1F1E7, 0x1F1E8]), vec![2, 2]);
        assert_eq!(cluster_widths(&[0x1F1E8]), vec![2]);
        assert_eq!(cluster_widths(&[]), Vec::<u8>::new());
        // A cluster made only of combining marks has no width at all.
        assert_eq!(clusters(&[0x0301, 0x0302]).len(), 1);
        assert_eq!(cluster_widths(&[0x0301, 0x0302]), vec![0]);
        // 👋🏿 waving hand + dark skin tone: one cluster, two cells.
        assert_eq!(cluster_widths(&[0x1F44B, 0x1F3FF]), vec![2]);
        // #️⃣ keycap.
        assert_eq!(cluster_widths(&['#' as u32, 0xFE0F, 0x20E3]), vec![1]);
        // 1 + keycap without the selector still clusters.
        assert_eq!(clusters(&['1' as u32, 0x20E3]).len(), 1);
    }

    #[test]
    fn state_resets() {
        let mut state = GraphemeState::default();
        assert!(!grapheme_break(0x1F1E6, 0x1F1E7, &mut state));
        assert_ne!(state, GraphemeState::default());
        state.reset();
        assert_eq!(state, GraphemeState::default());
    }

    #[test]
    fn unicode_version_is_recorded() {
        assert_eq!(UNICODE_VERSION, "15.1.0");
    }
}
