//! Line break opportunities from UAX #14 (Unicode 18.0.0), with CSS Text's rules on top of it.
//!
//! LB1 treats SA like AL, so Thai and the other SA scripts never break between their letters.
//! Runs of text pass a [`PrecedingContext`] and [`FollowingContext`] to break like their paragraph.

use super::line_break_data::{LATIN1, LEAF_SHIFT, LEAVES, MIDDLE, MIDDLE_SHIFT, PROPS, TOP};

/// Returns every line break opportunity in `text`, in order.
///
/// `before` and `after` describe what surrounds `text`. Index 0 and `text.len()` are included
/// when the boundary with the surrounding text is a break opportunity.
pub fn find_line_breaks<'a>(
    text: &'a str,
    before: PrecedingContext,
    after: FollowingContext,
) -> LineBreaks<'a> {
    let mut unit_reader = UnitReader {
        text,
        can_absorb_marks: before.last_char_class.can_absorb_marks(),
        after,
        after_unit_count: 0,
    };
    let units = [unit_reader.read_unit(), unit_reader.read_unit(), unit_reader.read_unit()];
    LineBreaks {
        text,
        before,
        byte_index: 0,
        units,
        unit_reader,
        is_done: false,
        is_last_break_from_text_alone: false,
    }
}

/// Returns the first line break opportunity in `text`, scanning no further than it.
pub fn find_first_line_break(
    text: &str,
    before: PrecedingContext,
    after: FollowingContext,
) -> Option<LineBreak> {
    find_line_breaks(text, before, after).next()
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LineBreak {
    /// The byte index where the next line would start.
    pub byte_index: usize,
    /// Whether a hard line break (BK, CR, LF or NL) right before `byte_index` forces this break.
    pub is_forced: bool,
    /// The byte length of that hard line break within this text, like 2 for CR LF.
    ///
    /// It's 0 for soft breaks and when the hard line break ended the preceding text. A CR LF split
    /// between two texts only gets a break in the LF's text.
    pub newline_len: usize,
}

/// An iterator over the line break opportunities in a text, see [`find_line_breaks`].
#[derive(Clone, Debug)]
pub struct LineBreaks<'a> {
    text: &'a str,
    before: PrecedingContext,
    byte_index: usize,
    // The first LB9 unit that starts at or after `byte_index`, and the two after it.
    units: [CharProps; 3],
    unit_reader: UnitReader<'a>,
    is_done: bool,
    is_last_break_from_text_alone: bool,
}

impl LineBreaks<'_> {
    /// Returns whether the last break we found came from the text alone.
    ///
    /// Then it, and the lack of breaks before it, stay the same whatever follows the text.
    pub fn is_last_break_from_text_alone(&self) -> bool {
        self.is_last_break_from_text_alone
    }

    fn find_next_break(&mut self, is_grapheme_aware: bool) -> Option<LineBreak> {
        while !self.is_done {
            let byte_index = self.byte_index;
            let next_char = self.text[byte_index..].chars().next();
            let next_char_props =
                next_char.map_or(self.unit_reader.after.first_char, get_char_props);
            // A mark that joins the unit before it can't start a line (LB9).
            let is_unit_start = !next_char_props.is_mark()
                || !self.before.last_char_class.can_absorb_marks();
            let mut line_break = None;
            if is_unit_start {
                line_break = match get_uax14_break(&self.before, next_char_props, self.units) {
                    Uax14Break::Prohibited => None,
                    Uax14Break::Allowed => {
                        let is_allowed = !is_grapheme_aware
                            || is_grapheme_boundary(&self.before, next_char_props);
                        is_allowed.then_some(LineBreak {
                            byte_index,
                            is_forced: false,
                            newline_len: 0,
                        })
                    }
                    Uax14Break::Mandatory => {
                        let text_before = &self.text[..byte_index];
                        Some(LineBreak {
                            byte_index,
                            is_forced: true,
                            newline_len: if text_before.ends_with("\r\n") {
                                2
                            } else {
                                text_before.chars().next_back().map_or(0, char::len_utf8)
                            },
                        })
                    }
                };
            }
            if line_break.is_some() {
                self.is_last_break_from_text_alone =
                    next_char.is_some() && self.unit_reader.after_unit_count == 0;
            }
            match next_char {
                Some(c) => {
                    if is_unit_start {
                        self.units = [self.units[1], self.units[2], self.unit_reader.read_unit()];
                    }
                    self.before.push_char(next_char_props);
                    self.byte_index += c.len_utf8();
                }
                None => self.is_done = true,
            }
            if line_break.is_some() {
                return line_break;
            }
        }
        None
    }
}

impl Iterator for LineBreaks<'_> {
    type Item = LineBreak;

    fn next(&mut self) -> Option<LineBreak> {
        self.find_next_break(true)
    }
}

/// Reads the LB9 units of a text one by one, and then those of what follows the text.
#[derive(Clone, Debug)]
struct UnitReader<'a> {
    text: &'a str,
    // Whether a mark at the start of `text` would join the unit before it.
    can_absorb_marks: bool,
    after: FollowingContext,
    // How many units we've read past the end of `text`.
    after_unit_count: usize,
}

impl UnitReader<'_> {
    fn read_unit(&mut self) -> CharProps {
        let mut chars = self.text.chars();
        while let Some(c) = chars.next() {
            let props = get_char_props(c);
            if props.is_mark() && self.can_absorb_marks {
                continue;                                                  // LB9
            }
            self.text = chars.as_str();
            self.can_absorb_marks = props.class.can_absorb_marks();
            return if props.is_mark() { LETTER_A } else { props.to_line_break_props() }; // LB10
        }
        self.text = "";
        // Leading marks of `after` that can't join the unit before them are a unit of their own.
        let has_lone_marks = self.after.first_char.is_mark() && !self.can_absorb_marks;
        let index = self.after_unit_count;
        self.after_unit_count += 1;
        match (has_lone_marks, index) {
            (true, 0) => LETTER_A,                                         // LB10
            (true, _) => self.after.next_units[index - 1],
            (false, _) => self.after.next_units[index],
        }
    }
}

/// What the line breaking rules need to know about the text before a run of text.
///
/// Start from [`PrecedingContext::PARAGRAPH_START`] or [`PrecedingContext::AFTER_ATOMIC_INLINE`],
/// then [`append`](PrecedingContext::append) each run of text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PrecedingContext {
    // The class of the last char itself, so marks show up here as CM or ZWJ.
    last_char_class: Class,
    // The last unit of LB9, a char with the marks it absorbed (or LB10's letter A).
    last_unit: CharProps,
    last_non_space_class: Class,
    is_after_zero_width_space: bool,
    is_after_opening_quote: bool,
    is_quote_after_non_east_asian: bool,
    is_word_initial_hyphen: bool,
    is_hyphen_after_hebrew: bool,
    is_virama_after_aksara: bool,
    number_context: NumberContext,
    has_odd_regional_indicators: bool,
    // The grapheme cluster rules of UAX #29 go by chars, not by LB9 units.
    last_grapheme_cluster_break: GraphemeClusterBreak,
    emoji_sequence: EmojiSequence,
    has_odd_regional_indicator_chars: bool,
    has_conjunct_linker: bool,
}

impl PrecedingContext {
    pub const PARAGRAPH_START: Self = Self {
        last_char_class: Class::Sot,
        last_unit: CharProps::new(Class::Sot, GraphemeClusterBreak::Other, 0),
        last_non_space_class: Class::Sot,
        is_after_zero_width_space: false,
        is_after_opening_quote: false,
        is_quote_after_non_east_asian: false,
        is_word_initial_hyphen: false,
        is_hyphen_after_hebrew: false,
        is_virama_after_aksara: false,
        number_context: NumberContext::None,
        has_odd_regional_indicators: false,
        last_grapheme_cluster_break: GraphemeClusterBreak::Other,
        emoji_sequence: EmojiSequence::None,
        has_odd_regional_indicator_chars: false,
        has_conjunct_linker: false,
    };

    /// The context right after an atomic inline, like an inline widget.
    ///
    /// Text right after it can start a line, unless it starts with a space, ZWSP, a hard line
    /// break, a mark, WJ, or GL other than NBSP. Later chars see the widget as U+FFFC.
    pub const AFTER_ATOMIC_INLINE: Self = Self {
        last_char_class: Class::AtomicInline,
        last_unit: OBJECT_REPLACEMENT_CHAR,
        last_non_space_class: Class::CB,
        ..Self::PARAGRAPH_START
    };

    /// Adds `text` to the end of the text this context describes.
    ///
    /// We only walk back from the end of `text` as far as the rules can look back.
    pub fn append(&mut self, text: &str) {
        use Class::*;
        // Most texts end with a char that no rule looks back past, maybe with spaces after it,
        // and replaying just those gives what the walk back below would.
        let mut spaces_start = text.len();
        while spaces_start > 0 && text.as_bytes()[spaces_start - 1] == b' ' {
            spaces_start -= 1;
        }
        if let Some(last_char) = text[..spaces_start].chars().next_back() {
            let props = get_char_props(last_char);
            let is_enough_to_replay = if spaces_start < text.len() {
                // Behind spaces, LB15a also looks at the unit before an opening quote.
                !props.is_mark() && !props.has(INITIAL_QUOTE)
            } else {
                !props.is_mark()
                    && !matches!(props.class, QU | HY | HH | VI | SY | IS | CL | CP | RI)
                    && (props.grapheme_cluster_break != GraphemeClusterBreak::Extend || props.has(EXT_PICT))
                    && props.grapheme_cluster_break != GraphemeClusterBreak::RegionalIndicator
                    && !props.has(CONJUNCT_EXTEND)
            };
            if is_enough_to_replay {
                let mut context = Self::PARAGRAPH_START;
                context.push_char(props);
                for _ in spaces_start..text.len() {
                    context.push_char(LATIN1_PROPS[b' ' as usize]);
                }
                *self = context;
                return;
            }
        }
        // The LB9 units of `text` backwards, each with the index of the char we'd have to replay
        // from to rebuild it. It stops at marks that start the text.
        let mut end = text.len();
        let units = std::iter::from_fn(move || {
            let mut unit_start = end;
            let (base_start, base) = loop {
                let (index, c) = text[..unit_start].char_indices().next_back()?;
                let props = get_char_props(c);
                if !props.is_mark() {
                    break (index, props);
                }
                unit_start = index;
            };
            if unit_start < end && !base.class.can_absorb_marks() {
                end = unit_start;
                Some((base_start, LETTER_A))
            } else {
                end = base_start;
                Some((base_start, base.to_line_break_props()))
            }
        });
        // Each rule that looks back stops at some unit or char, and we replay the text from the
        // earliest of those. If one of them runs into the start of the text, we replay it all.
        let replay_start = 'find: {
            let Some((mut earliest, last_unit)) = units.clone().next() else {
                break 'find None;
            };
            // LB8 and LB14..LB17 look back past spaces, and LB15a one unit further.
            let mut non_space_units = units.clone().skip_while(|(_, unit)| unit.class == SP);
            let Some((index, unit)) = non_space_units.next() else {
                break 'find None;
            };
            earliest = earliest.min(index);
            if unit.has(INITIAL_QUOTE) {
                let Some((index, _)) = non_space_units.next() else {
                    break 'find None;
                };
                earliest = earliest.min(index);
            }
            // LB19a, LB20a, LB21a and LB28a look at the unit before the last one.
            if matches!(last_unit.class, QU | HY | HH | VI) {
                let Some((index, _)) = units.clone().nth(1) else {
                    break 'find None;
                };
                earliest = earliest.min(index);
            }
            // LB25 looks back past a closing bracket and `(SY | IS)*` for a number.
            if matches!(last_unit.class, SY | IS | CL | CP) {
                let Some((index, _)) = units
                    .clone()
                    .skip(usize::from(matches!(last_unit.class, CL | CP)))
                    .find(|(_, unit)| !matches!(unit.class, SY | IS))
                else {
                    break 'find None;
                };
                earliest = earliest.min(index);
            }
            // LB30a counts the regional indicators.
            if last_unit.class == RI {
                let Some((index, _)) = units.clone().find(|(_, unit)| unit.class != RI) else {
                    break 'find None;
                };
                earliest = earliest.min(index);
            }
            // GB11, GB12, GB13 and GB9c look back past extenders, regional indicators and
            // conjunct extenders.
            let chars = text
                .char_indices()
                .rev()
                .map(|(index, c)| (index + c.len_utf8(), get_char_props(c)));
            let Some((_, last_char)) = chars.clone().next() else {
                break 'find None;
            };
            let emoji_start = chars
                .clone()
                .skip(usize::from(last_char.grapheme_cluster_break == GraphemeClusterBreak::ZWJ))
                .find(|(_, props)| {
                    props.grapheme_cluster_break != GraphemeClusterBreak::Extend
                        || props.has(EXT_PICT)
                });
            let regional_indicator_start = chars.clone().find(|(_, props)| {
                props.grapheme_cluster_break != GraphemeClusterBreak::RegionalIndicator
            });
            let conjunct_start = chars.clone().find(|(_, props)| !props.has(CONJUNCT_EXTEND));
            for start in [emoji_start, regional_indicator_start, conjunct_start] {
                let base_start = start.and_then(|(end, _)| {
                    text[..end]
                        .char_indices()
                        .rev()
                        .find(|&(_, c)| !get_char_props(c).is_mark())
                        .map(|(index, _)| index)
                });
                let Some(index) = base_start else {
                    break 'find None;
                };
                earliest = earliest.min(index);
            }
            Some(earliest)
        };
        let (mut context, replay_text) = match replay_start {
            Some(index) => (Self::PARAGRAPH_START, &text[index..]),
            None => (*self, text),
        };
        for c in replay_text.chars() {
            context.push_char(get_char_props(c));
        }
        *self = context;
    }

    fn push_char(&mut self, props: CharProps) {
        use Class::*;
        if !props.is_mark() || !self.last_char_class.can_absorb_marks() {
            let unit = if props.is_mark() { LETTER_A } else { props.to_line_break_props() };
            let previous = self.last_unit;
            self.is_after_opening_quote = match unit.class {
                SP => self.is_after_opening_quote,
                QU if unit.has(INITIAL_QUOTE) => {
                    matches!(previous.class, Sot | BK | CR | LF | NL | OP | QU | GL | SP | ZW)
                }
                _ => false,
            };
            if unit.class != SP {
                self.last_non_space_class = unit.class;
            }
            self.is_quote_after_non_east_asian =
                unit.class == QU && (previous.class == Sot || !previous.has(EAST_ASIAN));
            self.is_word_initial_hyphen = matches!(unit.class, HY | HH)
                && matches!(previous.class, Sot | BK | CR | LF | NL | SP | ZW | CB | GL);
            self.is_hyphen_after_hebrew = matches!(unit.class, HY | HH) && previous.class == HL;
            self.is_virama_after_aksara = unit.class == VI && previous.is_aksara();
            self.number_context = match (unit.class, self.number_context) {
                (NU, _) | (SY | IS, NumberContext::Number) => NumberContext::Number,
                (CL | CP, NumberContext::Number) => NumberContext::ClosedNumber,
                _ => NumberContext::None,
            };
            self.has_odd_regional_indicators = unit.class == RI && !self.has_odd_regional_indicators;
            // No rule reads INITIAL_QUOTE from the last unit, so we drop it and contexts that act
            // the same compare equal.
            self.last_unit = CharProps { flags: unit.flags & !INITIAL_QUOTE, ..unit };
        }
        self.last_char_class = props.class;
        self.is_after_zero_width_space = match props.class {
            ZW => true,
            SP => self.is_after_zero_width_space,
            _ => false,
        };

        self.emoji_sequence = match (props.grapheme_cluster_break, self.emoji_sequence) {
            _ if props.has(EXT_PICT) => EmojiSequence::Pictographic,
            (GraphemeClusterBreak::Extend, EmojiSequence::Pictographic) => {
                EmojiSequence::Pictographic
            }
            (GraphemeClusterBreak::ZWJ, EmojiSequence::Pictographic) => EmojiSequence::Joined,
            _ => EmojiSequence::None,
        };
        let is_regional_indicator =
            props.grapheme_cluster_break == GraphemeClusterBreak::RegionalIndicator;
        self.has_odd_regional_indicator_chars =
            is_regional_indicator && !self.has_odd_regional_indicator_chars;
        self.has_conjunct_linker = props.has(CONJUNCT_LINKER)
            || (props.has(CONJUNCT_EXTEND) && self.has_conjunct_linker);
        self.last_grapheme_cluster_break = props.grapheme_cluster_break;
    }
}

impl Default for PrecedingContext {
    fn default() -> Self {
        Self::PARAGRAPH_START
    }
}

/// What the line breaking rules need to know about the text after a run of text.
///
/// Start from [`FollowingContext::END_OF_TEXT`] or [`FollowingContext::BEFORE_ATOMIC_INLINE`],
/// then [`prepend`](FollowingContext::prepend) the runs of text before it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FollowingContext {
    first_char: CharProps,
    // The next LB9 units after any leading marks, which is as far as the rules look.
    next_units: [CharProps; UNITS_READ_AHEAD],
}

/// How many LB9 units after a position the rules read.
pub const UNITS_READ_AHEAD: usize = 3;

impl FollowingContext {
    pub const END_OF_TEXT: Self = Self {
        first_char: END_OF_TEXT_PROPS,
        next_units: [END_OF_TEXT_PROPS; UNITS_READ_AHEAD],
    };

    /// The context right before an atomic inline, like an inline widget.
    ///
    /// The text before it can end a line unless it ends with WJ, ZWJ, or GL other than NBSP.
    /// We never look past an atomic inline.
    pub const BEFORE_ATOMIC_INLINE: Self = Self {
        first_char: ATOMIC_INLINE_PROPS,
        next_units: [ATOMIC_INLINE_PROPS; UNITS_READ_AHEAD],
    };

    /// Adds `text` to the start of the text this context describes.
    ///
    /// We only read the first few chars of `text`. Returns how many of the units the rules read
    /// still come from the old context, which happens when `text` is that short.
    pub fn prepend(&mut self, text: &str) -> usize {
        let Some(mut first_char) = text.chars().next().map(get_char_props) else {
            return UNITS_READ_AHEAD;
        };
        // Leading marks join whatever comes before the text.
        let mut unit_reader = UnitReader {
            text,
            can_absorb_marks: true,
            after: *self,
            after_unit_count: 0,
        };
        let mut next_units = [unit_reader.read_unit(), unit_reader.read_unit(), unit_reader.read_unit()];
        // We keep only what the rules read from each of these, so contexts that act the same
        // compare equal.
        first_char.flags &= CONJUNCT_CONSONANT | EXT_PICT;
        next_units[0].flags &= !UNASSIGNED_EXT_PICT;
        next_units[1].flags &= EAST_ASIAN;
        next_units[2].flags = 0;
        *self = Self { first_char, next_units };
        unit_reader.after_unit_count
    }
}

impl Default for FollowingContext {
    fn default() -> Self {
        Self::END_OF_TEXT
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NumberContext {
    None,
    // After `NU (SY | IS)*` (LB25).
    Number,
    // After `NU (SY | IS)* (CL | CP)` (LB25).
    ClosedNumber,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EmojiSequence {
    None,
    // After `ExtPict Extend*` (GB11).
    Pictographic,
    // After `ExtPict Extend* ZWJ` (GB11).
    Joined,
}

/// The break UAX #14 gives at a position: `×`, `÷` or `!`.
enum Uax14Break {
    Prohibited,
    Allowed,
    Mandatory,
}

/// Returns the break UAX #14 (and CSS, for atomic inlines) gives between `before` and `next_char`.
///
/// `units` are the LB9 units from `next_char` on, and `next_char` is never a mark that joins the
/// unit before it.
fn get_uax14_break(
    before: &PrecedingContext,
    next_char: CharProps,
    units: [CharProps; 3],
) -> Uax14Break {
    use {Class::*, Uax14Break::*};
    let last_char = before.last_char_class;
    match last_char {
        Sot => return Prohibited,                                          // LB2
        BK | LF | NL => return Mandatory,                                  // LB4, LB5
        CR if next_char.class == LF => return Prohibited,                  // LB5
        CR => return Mandatory,                                            // LB5
        _ => {}
    }
    match next_char.class {
        Eot => return Allowed,                                             // LB3
        BK | CR | LF | NL | SP | ZW => return Prohibited,                  // LB6, LB7
        _ => {}
    }
    if before.is_after_zero_width_space {
        return Allowed;                                                    // LB8
    }
    if last_char == ZWJ {
        return Prohibited;                                                 // LB8a
    }
    // `l` and `r` are the units on either side, and `r1` and `r2` the ones after `r`.
    let [r, r1, r2] = units;
    let l = before.last_unit;
    let (lc, rc) = (l.class, r.class);

    // CSS lets atomic inlines break on both sides, except next to WJ and GL other than NBSP.
    if last_char == AtomicInline || rc == AtomicInline {
        let other = if rc == AtomicInline { l } else { r };
        let is_glued = other.class == WJ || (other.class == GL && !other.has(NO_BREAK_SPACE));
        return if is_glued { Prohibited } else { Allowed };
    }

    if lc == WJ || rc == WJ {
        return Prohibited;                                                 // LB11
    }
    if lc == GL {
        return Prohibited;                                                 // LB12
    }
    if rc == GL && !matches!(lc, SP | HY | HH) {
        return Prohibited;                                                 // LB12a
    }
    if matches!(rc, CL | CP | EX | SY) {
        return Prohibited;                                                 // LB13
    }
    if before.last_non_space_class == OP {
        return Prohibited;                                                 // LB14
    }
    if before.is_after_opening_quote {
        return Prohibited;                                                 // LB15a
    }
    if r.has(FINAL_QUOTE)
        && matches!(
            r1.class,
            SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW | Eot
        )
    {
        return Prohibited;                                                 // LB15b
    }
    if lc == SP && rc == IS && r1.class == NU {
        return Allowed;                                                    // LB15c
    }
    if rc == IS {
        return Prohibited;                                                 // LB15d
    }
    if rc == NS && matches!(before.last_non_space_class, CL | CP) {
        return Prohibited;                                                 // LB16
    }
    if rc == B2 && before.last_non_space_class == B2 {
        return Prohibited;                                                 // LB17
    }
    if lc == SP {
        return Allowed;                                                    // LB18
    }
    if (rc == QU && !r.has(INITIAL_QUOTE)) || (lc == QU && !l.has(FINAL_QUOTE)) {
        return Prohibited;                                                 // LB19
    }
    if (rc == QU && (!l.has(EAST_ASIAN) || !r1.has(EAST_ASIAN)))
        || (lc == QU && (!r.has(EAST_ASIAN) || before.is_quote_after_non_east_asian))
    {
        return Prohibited;                                                 // LB19a
    }
    if lc == CB || rc == CB {
        return Allowed;                                                    // LB20
    }
    if before.is_word_initial_hyphen && matches!(rc, AL | HL) {
        return Prohibited;                                                 // LB20a
    }
    if matches!(rc, BA | HH | HY | NS) || lc == BB {
        return Prohibited;                                                 // LB21
    }
    if before.is_hyphen_after_hebrew && rc != HL {
        return Prohibited;                                                 // LB21a
    }
    if lc == SY && rc == HL {
        return Prohibited;                                                 // LB21b
    }
    if rc == IN {
        return Prohibited;                                                 // LB22
    }
    if (matches!(lc, AL | HL) && rc == NU) || (lc == NU && matches!(rc, AL | HL)) {
        return Prohibited;                                                 // LB23
    }
    if (lc == PR && matches!(rc, ID | EB | EM)) || (matches!(lc, ID | EB | EM) && rc == PO) {
        return Prohibited;                                                 // LB23a
    }
    if (matches!(lc, PR | PO) && matches!(rc, AL | HL))
        || (matches!(lc, AL | HL) && matches!(rc, PR | PO))
    {
        return Prohibited;                                                 // LB24
    }
    if (matches!(rc, PO | PR) && before.number_context != NumberContext::None)
        || (matches!(lc, PO | PR)
            && (rc == NU || (rc == OP && (r1.class == NU || (r1.class == IS && r2.class == NU)))))
        || (matches!(lc, HY | IS) && rc == NU)
        || (before.number_context == NumberContext::Number && rc == NU)
    {
        return Prohibited;                                                 // LB25
    }
    if (lc == JL && matches!(rc, JL | JV | H2 | H3))
        || (matches!(lc, JV | H2) && matches!(rc, JV | JT))
        || (matches!(lc, JT | H3) && rc == JT)
    {
        return Prohibited;                                                 // LB26
    }
    if (matches!(lc, JL | JV | JT | H2 | H3) && rc == PO)
        || (lc == PR && matches!(rc, JL | JV | JT | H2 | H3))
    {
        return Prohibited;                                                 // LB27
    }
    if matches!(lc, AL | HL) && matches!(rc, AL | HL) {
        return Prohibited;                                                 // LB28
    }
    if (lc == AP && r.is_aksara())
        || (l.is_aksara() && matches!(rc, VF | VI))
        || (before.is_virama_after_aksara && (rc == AK || r.has(DOTTED_CIRCLE)))
        || (l.is_aksara() && r.is_aksara() && r1.class == VF)
    {
        return Prohibited;                                                 // LB28a
    }
    if lc == IS && matches!(rc, AL | HL) {
        return Prohibited;                                                 // LB29
    }
    if (matches!(lc, AL | HL | NU) && rc == OP && !r.has(EAST_ASIAN))
        || (lc == CP && !l.has(EAST_ASIAN) && matches!(rc, AL | HL | NU))
    {
        return Prohibited;                                                 // LB30
    }
    if lc == RI && rc == RI && before.has_odd_regional_indicators {
        return Prohibited;                                                 // LB30a
    }
    if rc == EM && (lc == EB || l.has(UNASSIGNED_EXT_PICT)) {
        return Prohibited;                                                 // LB30b
    }
    Allowed                                                                // LB31
}

/// Returns whether there's a grapheme cluster boundary between `before` and `next_char`.
///
/// This follows UAX #29 (Unicode 18.0.0).
fn is_grapheme_boundary(before: &PrecedingContext, next_char: CharProps) -> bool {
    use GraphemeClusterBreak::*;
    if matches!(before.last_char_class, Class::Sot | Class::AtomicInline)
        || matches!(next_char.class, Class::Eot | Class::AtomicInline)
    {
        return true;
    }
    match (before.last_grapheme_cluster_break, next_char.grapheme_cluster_break) {
        (CR, LF) => false,                                                 // GB3
        (Control | CR | LF, _) | (_, Control | CR | LF) => true,           // GB4, GB5
        (L, L | V | LV | LVT) | (LV | V, V | T) | (LVT | T, T) => false,   // GB6..GB8
        (_, Extend | ZWJ | SpacingMark) | (Prepend, _) => false,           // GB9..GB9b
        _ if before.has_conjunct_linker && next_char.has(CONJUNCT_CONSONANT) => false, // GB9c
        _ if before.emoji_sequence == EmojiSequence::Joined && next_char.has(EXT_PICT) => false, // GB11
        (RegionalIndicator, RegionalIndicator) => !before.has_odd_regional_indicator_chars, // GB12, GB13
        _ => true,                                                         // GB999
    }
}

/// A Line_Break class after LB1, or a pseudo class for what surrounds the text.
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Class {
    BK, CR, LF, CM, NL, WJ, ZW, GL, SP, ZWJ, B2, BA, BB, HY, HH, CB, CL, CP, EX, IN, NS, OP,
    QU, IS, NU, PO, PR, SY, AK, AL, AP, AS, EB, EM, H2, H3, HL, ID, JL, JT, JV, RI, VF, VI,
    Sot,
    Eot,
    AtomicInline,
}

impl Class {
    /// Returns whether marks right after a char of this class join its LB9 unit.
    fn can_absorb_marks(self) -> bool {
        use Class::*;
        !matches!(self, Sot | BK | CR | LF | NL | SP | ZW)
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum GraphemeClusterBreak {
    Other, CR, LF, Control, Extend, ZWJ, RegionalIndicator, Prepend, SpacingMark, L, V, T, LV, LVT,
}

/// The properties of a code point that line breaking looks at.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct CharProps {
    class: Class,
    grapheme_cluster_break: GraphemeClusterBreak,
    flags: u16,
}

// East_Asian_Width is F, W or H, which is what `$EastAsian` means in UAX #14.
pub(super) const EAST_ASIAN: u16 = 1 << 0;
// Quotation marks (QU) with General_Category Pi or Pf.
pub(super) const INITIAL_QUOTE: u16 = 1 << 1;
pub(super) const FINAL_QUOTE: u16 = 1 << 2;
pub(super) const UNASSIGNED_EXT_PICT: u16 = 1 << 3;
pub(super) const DOTTED_CIRCLE: u16 = 1 << 4;
pub(super) const NO_BREAK_SPACE: u16 = 1 << 5;
pub(super) const EXT_PICT: u16 = 1 << 6;
// Indic_Conjunct_Break values (GB9c).
pub(super) const CONJUNCT_LINKER: u16 = 1 << 7;
pub(super) const CONJUNCT_CONSONANT: u16 = 1 << 8;
pub(super) const CONJUNCT_EXTEND: u16 = 1 << 9;
const LINE_BREAK_FLAGS: u16 =
    EAST_ASIAN | INITIAL_QUOTE | FINAL_QUOTE | UNASSIGNED_EXT_PICT | DOTTED_CIRCLE | NO_BREAK_SPACE;

// LB10 treats a mark that doesn't join a unit like U+0041 A.
const LETTER_A: CharProps = CharProps::new(Class::AL, GraphemeClusterBreak::Other, 0);
const OBJECT_REPLACEMENT_CHAR: CharProps =
    CharProps::new(Class::CB, GraphemeClusterBreak::Other, 0);
const END_OF_TEXT_PROPS: CharProps = CharProps::new(Class::Eot, GraphemeClusterBreak::Other, 0);
const ATOMIC_INLINE_PROPS: CharProps =
    CharProps::new(Class::AtomicInline, GraphemeClusterBreak::Other, 0);

impl CharProps {
    pub(super) const fn new(
        class: Class,
        grapheme_cluster_break: GraphemeClusterBreak,
        flags: u16,
    ) -> Self {
        Self {
            class,
            grapheme_cluster_break,
            flags,
        }
    }

    fn has(self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn is_mark(self) -> bool {
        matches!(self.class, Class::CM | Class::ZWJ)
    }

    fn is_aksara(self) -> bool {
        matches!(self.class, Class::AK | Class::AS) || self.has(DOTTED_CIRCLE)
    }

    fn to_line_break_props(self) -> Self {
        Self::new(self.class, GraphemeClusterBreak::Other, self.flags & LINE_BREAK_FLAGS)
    }
}

static LATIN1_PROPS: [CharProps; 256] = {
    let mut props = [LETTER_A; 256];
    let mut code_point = 0;
    while code_point < props.len() {
        props[code_point] = PROPS[LATIN1[code_point] as usize];
        code_point += 1;
    }
    props
};

fn get_char_props(c: char) -> CharProps {
    let code_point = c as usize;
    if code_point < LATIN1_PROPS.len() {
        return LATIN1_PROPS[code_point];
    }
    let middle_block = TOP[code_point >> MIDDLE_SHIFT] as usize;
    let middle_offset = (code_point >> LEAF_SHIFT) & ((1 << (MIDDLE_SHIFT - LEAF_SHIFT)) - 1);
    let leaf_block = MIDDLE[(middle_block << (MIDDLE_SHIFT - LEAF_SHIFT)) | middle_offset] as usize;
    let leaf_offset = code_point & ((1 << LEAF_SHIFT) - 1);
    PROPS[LEAVES[(leaf_block << LEAF_SHIFT) | leaf_offset] as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: PrecedingContext = PrecedingContext::PARAGRAPH_START;
    const END: FollowingContext = FollowingContext::END_OF_TEXT;

    fn find_break_indices(
        text: &str,
        before: PrecedingContext,
        after: FollowingContext,
    ) -> Vec<usize> {
        find_line_breaks(text, before, after)
            .map(|line_break| line_break.byte_index)
            .collect()
    }

    /// Checks the pieces of each paragraph between its line break opportunities.
    fn check_pieces(cases: &[(&str, &[&str])]) {
        for &(text, pieces) in cases {
            let mut start = 0;
            let found: Vec<_> = find_line_breaks(text, START, END)
                .map(|line_break| {
                    let piece = &text[start..line_break.byte_index];
                    start = line_break.byte_index;
                    piece
                })
                .collect();
            assert_eq!(found, pieces, "{text:?}");
        }
    }

    /// Checks that `text` split in two at any char boundary gets the same breaks as the whole.
    ///
    /// It also checks that appending a run gives the same context as pushing each of its chars.
    fn check_split_runs(text: &str, starts: &[PrecedingContext]) {
        let whole: Vec<_> = find_line_breaks(text, START, END).collect();
        for split in text.char_indices().map(|(index, _)| index).chain([text.len()]) {
            let (prefix, suffix) = text.split_at(split);
            let mut after = END;
            after.prepend(suffix);
            let mut before = START;
            before.append(prefix);
            let mut breaks: Vec<_> = find_line_breaks(prefix, START, after).collect();
            let suffix_breaks: Vec<_> = find_line_breaks(suffix, before, END)
                .map(|line_break| LineBreak {
                    byte_index: split + line_break.byte_index,
                    ..line_break
                })
                .collect();
            let is_forced_at_split = |breaks: &[LineBreak]| {
                breaks
                    .iter()
                    .find(|line_break| line_break.byte_index == split)
                    .map(|line_break| line_break.is_forced)
            };
            assert_eq!(
                is_forced_at_split(&breaks),
                is_forced_at_split(&suffix_breaks),
                "{text:?} split at {split}"
            );
            breaks.extend(suffix_breaks.into_iter().filter(|line_break| line_break.byte_index > split));
            // The suffix only counts the part of a CR LF that's in it.
            let expected: Vec<_> = whole
                .iter()
                .map(|&line_break| LineBreak {
                    newline_len: if line_break.byte_index > split {
                        line_break.newline_len.min(line_break.byte_index - split)
                    } else {
                        line_break.newline_len
                    },
                    ..line_break
                })
                .collect();
            assert_eq!(breaks, expected, "{text:?} split at {split}");

            for &start in starts {
                let mut appended = start;
                appended.append(prefix);
                let mut pushed = start;
                for c in prefix.chars() {
                    pushed.push_char(get_char_props(c));
                }
                assert_eq!(appended, pushed, "{start:?} then {prefix:?}");
            }
        }
    }

    /// Checks that a break found from the text alone, and the ones before it, ignore what follows.
    ///
    /// What follows is the rest of `text` after each split, the end, or an atomic inline.
    fn check_breaks_from_text_alone(text: &str, starts: &[PrecedingContext]) {
        for &before in starts {
            for split in text.char_indices().map(|(index, _)| index).chain([text.len()]) {
                let (prefix, suffix) = text.split_at(split);
                let mut rest = END;
                rest.prepend(suffix);
                let breaks: Vec<_> = find_line_breaks(prefix, before, rest).collect();
                for after in [rest, END, FollowingContext::BEFORE_ATOMIC_INLINE] {
                    let mut line_breaks = find_line_breaks(prefix, before, after);
                    let mut found = Vec::new();
                    while let Some(line_break) = line_breaks.next() {
                        found.push(line_break);
                        if line_breaks.is_last_break_from_text_alone() {
                            let expected = breaks.get(..found.len());
                            assert_eq!(Some(&found[..]), expected, "{text:?} split at {split}");
                        }
                    }
                }
            }
        }
    }

    /// Checks that contexts compare equal, and hash alike, only when all their fields are equal.
    fn check_contexts_compare_by_fields<T: Copy + std::fmt::Debug + Eq + std::hash::Hash>(contexts: &[T]) {
        use std::collections::HashSet;
        let distinct_fields: HashSet<String> = contexts.iter().map(|context| format!("{context:?}")).collect();
        let distinct_contexts: HashSet<T> = contexts.iter().copied().collect();
        assert_eq!(distinct_contexts.len(), distinct_fields.len());
    }

    /// Returns contexts to append runs to, like the paragraph start and after an atomic inline.
    ///
    /// The rest come after texts that leave something for the rules to look back at.
    fn get_test_starts() -> Vec<PrecedingContext> {
        let mut starts = vec![START, PrecedingContext::AFTER_ATOMIC_INLINE];
        for text in [
            "a", " ", "( ", "« ", "1/", "(1)", "\u{1F1E6}", "\u{1F1E6}\u{1F1E6}", "😀", "😀\u{200D}",
            "\u{915}\u{94D}", "\u{200B} ", "\r", "שלום-", "-", "\u{1B18}\u{1B44}", "\u{301}",
            "\u{11A3A}",
        ] {
            let mut context = START;
            for c in text.chars() {
                context.push_char(get_char_props(c));
            }
            starts.push(context);
        }
        starts
    }

    /// Reads the LineBreakTest or GraphemeBreakTest style file that `variable` names.
    ///
    /// It returns each test string, and whether there's a break before each char and at the end.
    fn read_break_test_file(variable: &str) -> Option<Vec<(String, Vec<bool>)>> {
        let path = std::env::var(variable).ok()?;
        let contents =
            std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        let tests = contents
            .lines()
            .filter_map(|line| {
                let fields = line.split('#').next().unwrap_or("").split_whitespace();
                let mut text = String::new();
                let mut breaks = Vec::new();
                for field in fields {
                    match field {
                        "÷" => breaks.push(true),
                        "×" => breaks.push(false),
                        _ => text.push(
                            u32::from_str_radix(field, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or_else(|| panic!("{path}: bad code point {field}")),
                        ),
                    }
                }
                (!breaks.is_empty()).then_some((text, breaks))
            })
            .collect();
        Some(tests)
    }

    #[test]
    fn breaks_around_quotes_and_punctuation() {
        check_pieces(&[
            ("He said \"hello\" to me", &["He ", "said ", "\"hello\" ", "to ", "me"]),
            ("foo \" bar", &["foo ", "\" ", "bar"]),
            ("a “quoted” b", &["a ", "“quoted” ", "b"]),
            ("«\u{202F}Bonjour\u{202F}» dit-il", &["«\u{202F}Bonjour\u{202F}» ", "dit-", "il"]),
            ("Er sagte „Hallo“ heute", &["Er ", "sagte ", "„Hallo“ ", "heute"]),
            ("don't 'single'", &["don't ", "'single'"]),
            ("see (s)he (note)", &["see ", "(s)he ", "(note)"]),
            ("end.) Next [1], {x}!", &["end.) ", "Next ", "[1], ", "{x}!"]),
            ("(  x", &["(  x"]),
            ("«  x", &["«  x"]),
            ("wait... what…", &["wait... ", "what…"]),
            ("@alice:matrix.org: hi", &["@alice:matrix.org: ", "hi"]),
            ("**bold** `code` _x_", &["**bold** ", "`code` ", "_x_"]),
        ]);
    }

    #[test]
    fn breaks_in_numbers_and_urls() {
        check_pieces(&[
            ("https://example.com/a/b?c=d&e=f", &["https://", "example.com/", "a/", "b?", "c=d&e=f"]),
            ("me@example.org,", &["me@example.org,"]),
            ("$12.50 50% (12.5)% 1,000.00", &["$12.50 ", "50% ", "(12.5)% ", "1,000.00"]),
            ("subtract .5 now", &["subtract ", ".5 ", "now"]),
            ("x -13 and 5-7", &["x ", "-13 ", "and ", "5-7"]),
            ("and/or 1/2", &["and/", "or ", "1/2"]),
        ]);
    }

    #[test]
    fn breaks_around_hyphens_and_dashes() {
        check_pieces(&[
            ("state-of-the-art", &["state-", "of-", "the-", "art"]),
            ("the -ing form", &["the ", "-ing ", "form"]),
            ("a – b", &["a ", "– ", "b"]),
            ("a –\u{A0}b", &["a ", "–\u{A0}b"]),
            ("word—word", &["word", "—", "word"]),
            ("hyphen\u{AD}ation", &["hyphen\u{AD}", "ation"]),
        ]);
    }

    #[test]
    fn breaks_in_other_scripts_and_emoji() {
        check_pieces(&[
            ("日本「テスト」です。", &["日", "本", "「テ", "ス", "ト」", "で", "す。"]),
            ("チャット", &["チャッ", "ト"]),
            ("한국어 텍스트", &["한", "국", "어 ", "텍", "스", "트"]),
            ("ภาษาไทย", &["ภาษาไทย"]),
            ("ไทย-ไทย", &["ไทย-", "ไทย"]),
            ("שלום-world", &["שלום-world"]),
            ("שלום-שלום", &["שלום-", "שלום"]),
            ("a/שלום", &["a/שלום"]),
            ("अनुच्छेद", &["अनुच्छेद"]),
            ("hi 👨\u{200D}👩\u{200D}👧 yo", &["hi ", "👨\u{200D}👩\u{200D}👧 ", "yo"]),
            ("ok👍\u{1F3FD}ok", &["ok", "👍\u{1F3FD}", "ok"]),
            ("🇺🇸🇬🇧", &["🇺🇸", "🇬🇧"]),
        ]);
    }

    #[test]
    fn breaks_around_glue_and_invisible_chars() {
        check_pieces(&[
            ("Mr.\u{A0}Smith says", &["Mr.\u{A0}Smith ", "says"]),
            ("a\t\u{A0}b", &["a\t\u{A0}b"]),
            ("long\u{200B}word", &["long\u{200B}", "word"]),
            ("a\u{200B} b", &["a\u{200B} ", "b"]),
            ("no\u{2060}break", &["no\u{2060}break"]),
            ("text\u{FFFC}text \u{FFFC}.", &["text", "\u{FFFC}", "text ", "\u{FFFC}."]),
            ("(\u{FFFC})", &["(\u{FFFC})"]),
        ]);
    }

    #[test]
    fn hard_line_breaks_force_breaks() {
        let forced = |byte_index, newline_len| LineBreak {
            byte_index,
            is_forced: true,
            newline_len,
        };
        let soft = |byte_index| LineBreak {
            byte_index,
            is_forced: false,
            newline_len: 0,
        };
        let find = |text, before, after| find_line_breaks(text, before, after).collect::<Vec<_>>();
        assert_eq!(find("a\nb", START, END), [forced(2, 1), soft(3)]);
        assert_eq!(find("a\r\nb", START, END), [forced(3, 2), soft(4)]);
        assert_eq!(find("a\rb", START, END), [forced(2, 1), soft(3)]);
        assert_eq!(find("a\u{B}b\u{C}c", START, END), [forced(2, 1), forced(4, 1), soft(5)]);
        assert_eq!(find("a\u{85}b", START, END), [forced(3, 2), soft(4)]);
        assert_eq!(find("a\u{2028}b\u{2029}", START, END), [forced(4, 3), forced(8, 3)]);
        assert_eq!(find("a \nb", START, END), [forced(3, 1), soft(4)]);
        assert_eq!(find("\n\n", START, END), [forced(1, 1), forced(2, 1)]);

        // A CR LF split between two runs is one break, which the LF's run reports.
        let mut after = END;
        after.prepend("\nb");
        assert_eq!(find("a\r", START, after), []);
        let mut before = START;
        before.append("a\r");
        assert_eq!(find("\nb", before, END), [forced(1, 1), soft(2)]);
        let mut before = START;
        before.append("a\n");
        assert_eq!(find("b", before, END), [forced(0, 0), soft(1)]);
    }

    #[test]
    fn grapheme_clusters_stay_whole() {
        // UAX #14 alone would break before the mark after a space, before the emoji modifier,
        // after the Prepend chars, and around Balinese conjuncts.
        check_pieces(&[
            ("a \u{301}b", &["a \u{301}b"]),
            ("a\u{1F3FD}", &["a\u{1F3FD}"]),
            ("\u{110BD}日", &["\u{110BD}日"]),
            ("\u{11A3A}日", &["\u{11A3A}日"]),
            ("\u{1B18}\u{1B44}\u{1B27} \u{1B44}\u{1B27}", &["\u{1B18}\u{1B44}\u{1B27} \u{1B44}\u{1B27}"]),
        ]);
        let mut before = START;
        before.append("a ");
        assert_eq!(find_break_indices("\u{301}b", before, END), [3]);
        let mut after = END;
        after.prepend("\u{301}b");
        assert_eq!(find_break_indices("a ", START, after), []);
        let mut before = START;
        before.append("\u{110BD}");
        assert_eq!(find_break_indices("日", before, END), [3]);
    }

    #[test]
    fn unicode_segmentation_clusters_stay_whole() {
        use unicode_segmentation::UnicodeSegmentation;
        // Every code point between two letters, so the rules that look at one char on each side
        // see every code point on both sides.
        let text: String = ('\0'..=char::MAX).flat_map(|c| ['a', c]).chain(['a']).collect();
        let mut boundaries = text.grapheme_indices(true).map(|(index, _)| index);
        let mut before = START;
        let mut previous = None;
        for (index, c) in text.char_indices() {
            let props = get_char_props(c);
            if is_grapheme_boundary(&before, props) {
                assert_eq!(
                    boundaries.find(|&boundary| boundary >= index),
                    Some(index),
                    "{previous:?} then {c:?}"
                );
            }
            before.push_char(props);
            previous = Some(c);
        }
    }

    #[test]
    fn atomic_inlines_break_on_both_sides() {
        let after_inline = PrecedingContext::AFTER_ATOMIC_INLINE;
        assert_eq!(find_break_indices(", hi", after_inline, END), [0, 2, 4]);
        assert_eq!(find_break_indices(")", after_inline, END), [0, 1]);
        assert_eq!(find_break_indices("\"", after_inline, END), [0, 1]);
        assert_eq!(find_break_indices("\u{A0}x", after_inline, END), [0, 3]);
        assert_eq!(find_break_indices(" x", after_inline, END), [1, 2]);
        assert_eq!(find_break_indices("\u{202F}x", after_inline, END), [4]);
        assert_eq!(find_break_indices("\u{2060}x", after_inline, END), [4]);
        assert_eq!(find_break_indices("\u{200D}x", after_inline, END), [4]);
        assert_eq!(find_break_indices("\u{301}x", after_inline, END), [2, 3]);
        assert_eq!(find_break_indices("\nx", after_inline, END), [1, 2]);
        // Later chars see the inline as U+FFFC, so its hyphen stays with the word (LB20a).
        assert_eq!(find_break_indices("-x", after_inline, END), [0, 2]);

        let before_inline = FollowingContext::BEFORE_ATOMIC_INLINE;
        assert_eq!(find_break_indices("see (", START, before_inline), [4, 5]);
        assert_eq!(find_break_indices("a\u{A0}", START, before_inline), [3]);
        assert_eq!(find_break_indices("a\u{202F}", START, before_inline), []);
        assert_eq!(find_break_indices("a\u{2060}", START, before_inline), []);
        assert_eq!(find_break_indices("a\u{200D}", START, before_inline), []);
        assert_eq!(find_break_indices("", after_inline, before_inline), [0]);
        assert_eq!(find_break_indices("", START, before_inline), []);
    }

    #[test]
    fn contexts_that_act_the_same_compare_equal() {
        let append = |text| {
            let mut context = START;
            context.append(text);
            context
        };
        let prepend = |text| {
            let mut context = END;
            context.prepend(text);
            context
        };
        assert_eq!(append("a\u{AB}"), append("a\""));
        assert_eq!(prepend("\u{301}x"), prepend("\u{94D}x"));
    }

    #[test]
    fn contexts_that_differ_in_any_field_compare_unequal() {
        // The props of every code point and of what surrounds a text, and every flag on its own.
        let all_props: Vec<CharProps> = PROPS
            .iter()
            .copied()
            .chain([START.last_unit, END_OF_TEXT_PROPS, ATOMIC_INLINE_PROPS])
            .chain((0..u16::BITS).map(|bit| CharProps { flags: 1 << bit, ..LETTER_A }))
            .collect();
        let mut preceding_contexts = Vec::new();
        for start in get_test_starts() {
            for &props in &all_props {
                preceding_contexts.extend([
                    PrecedingContext { last_char_class: props.class, ..start },
                    PrecedingContext { last_unit: props, ..start },
                    PrecedingContext { last_non_space_class: props.class, ..start },
                    PrecedingContext { last_grapheme_cluster_break: props.grapheme_cluster_break, ..start },
                ]);
            }
            for number_context in [NumberContext::None, NumberContext::Number, NumberContext::ClosedNumber] {
                preceding_contexts.push(PrecedingContext { number_context, ..start });
            }
            for emoji_sequence in [EmojiSequence::None, EmojiSequence::Pictographic, EmojiSequence::Joined] {
                preceding_contexts.push(PrecedingContext { emoji_sequence, ..start });
            }
            preceding_contexts.extend([
                PrecedingContext { is_after_zero_width_space: !start.is_after_zero_width_space, ..start },
                PrecedingContext { is_after_opening_quote: !start.is_after_opening_quote, ..start },
                PrecedingContext { is_quote_after_non_east_asian: !start.is_quote_after_non_east_asian, ..start },
                PrecedingContext { is_word_initial_hyphen: !start.is_word_initial_hyphen, ..start },
                PrecedingContext { is_hyphen_after_hebrew: !start.is_hyphen_after_hebrew, ..start },
                PrecedingContext { is_virama_after_aksara: !start.is_virama_after_aksara, ..start },
                PrecedingContext { has_odd_regional_indicators: !start.has_odd_regional_indicators, ..start },
                PrecedingContext {
                    has_odd_regional_indicator_chars: !start.has_odd_regional_indicator_chars,
                    ..start
                },
                PrecedingContext { has_conjunct_linker: !start.has_conjunct_linker, ..start },
            ]);
        }
        check_contexts_compare_by_fields(&preceding_contexts);

        let mut following_contexts = Vec::new();
        for text in ["", "a", " b", "\u{301}x", "日本"] {
            let mut end = END;
            end.prepend(text);
            for &props in &all_props {
                following_contexts.push(FollowingContext { first_char: props, ..end });
                for index in 0..end.next_units.len() {
                    let mut next_units = end.next_units;
                    next_units[index] = props;
                    following_contexts.push(FollowingContext { next_units, ..end });
                }
            }
        }
        check_contexts_compare_by_fields(&following_contexts);
    }

    #[test]
    fn split_runs_get_the_same_breaks() {
        let starts = get_test_starts();
        for text in [
            "He said \"hello\" to me",
            "«\u{202F}Bonjour\u{202F}» dit-il",
            "x «  y » z",
            "”\u{301} x",
            "日“本”語 a“b”c",
            "(  x) word.)  next",
            ")  ‼ ——  —— a—b",
            "$(12.5)% and -1/2, $(.5)",
            "e.g. 3.14 subtract .5",
            "🇺🇸🇬🇧🇫 🇦\u{301}🇦🇦",
            "a\u{200B}  b \u{200B}\u{301}c",
            "👨\u{200D}👩\u{200D}👧 ok👍\u{1F3FD}ok a\u{1F3FD}",
            "שלום-world a/שלום שלום-שלום -ing",
            "\u{1B18}\u{1B44}\u{1B27} \u{1B44}\u{1B27}\u{1BF2}",
            "\u{915}\u{94D}\u{924} \u{915}\u{94D}",
            "a \u{301}b x\u{A0}\u{301}y 1\u{301} 2",
            "a\r\nb\rc\nd\u{85}e\u{2028}",
            "日本「テスト」です。 한국어 텍스트",
            "\u{FFFC}text (\u{FFFC}) \u{110BD}日",
            "\u{11A3A}\u{11A0B} \u{11A3A}日 \u{11A3A}",
        ] {
            check_split_runs(text, &starts);
            check_breaks_from_text_alone(text, &starts);
        }
    }

    #[test]
    fn char_props_match_the_ucd() {
        assert_eq!(get_char_props('A').to_line_break_props(), LETTER_A);
        assert_eq!(get_char_props('\u{FFFC}').to_line_break_props(), OBJECT_REPLACEMENT_CHAR);
        for (c, class, grapheme_cluster_break, flags) in [
            (' ', Class::SP, GraphemeClusterBreak::Other, 0),
            ('\u{A0}', Class::GL, GraphemeClusterBreak::Other, NO_BREAK_SPACE),
            ('\u{AD}', Class::HH, GraphemeClusterBreak::Control, 0),
            ('\u{E01}', Class::AL, GraphemeClusterBreak::Other, 0),
            ('\u{E31}', Class::CM, GraphemeClusterBreak::Extend, CONJUNCT_EXTEND),
            ('\u{915}', Class::AL, GraphemeClusterBreak::Other, CONJUNCT_CONSONANT),
            ('\u{94D}', Class::CM, GraphemeClusterBreak::Extend, CONJUNCT_LINKER),
            ('\u{201C}', Class::QU, GraphemeClusterBreak::Other, INITIAL_QUOTE),
            ('\u{25CC}', Class::AL, GraphemeClusterBreak::Other, DOTTED_CIRCLE),
            ('\u{3001}', Class::CL, GraphemeClusterBreak::Other, EAST_ASIAN),
            ('\u{3041}', Class::NS, GraphemeClusterBreak::Other, EAST_ASIAN),
            ('\u{11A3A}', Class::AL, GraphemeClusterBreak::Prepend, CONJUNCT_LINKER),
            ('\u{1F3FB}', Class::EM, GraphemeClusterBreak::Extend, EAST_ASIAN | CONJUNCT_EXTEND),
            ('\u{1FFFD}', Class::ID, GraphemeClusterBreak::Other, UNASSIGNED_EXT_PICT | EXT_PICT),
            ('\u{E0100}', Class::CM, GraphemeClusterBreak::Extend, CONJUNCT_EXTEND),
            ('\u{10FFFF}', Class::AL, GraphemeClusterBreak::Other, 0),
        ] {
            let expected = CharProps::new(class, grapheme_cluster_break, flags);
            assert_eq!(get_char_props(c), expected, "{c:?}");
        }
    }

    #[test]
    fn conforms_to_line_break_test_file() {
        let Some(tests) = read_break_test_file("MAKEPAD_LINE_BREAK_TEST_FILE") else {
            return;
        };
        let mut failures = Vec::new();
        for (text, expected) in &tests {
            let mut line_breaks = find_line_breaks(text, START, END);
            let found: Vec<_> = std::iter::from_fn(|| line_breaks.find_next_break(false))
                .map(|line_break| line_break.byte_index)
                .collect();
            let expected: Vec<_> = text
                .char_indices()
                .map(|(index, _)| index)
                .chain([text.len()])
                .zip(expected)
                .filter_map(|(index, &is_break)| is_break.then_some(index))
                .collect();
            if found != expected {
                failures.push(text);
            }
        }
        println!("UAX #14 conformance: {} of {} tests pass", tests.len() - failures.len(), tests.len());
        assert!(failures.is_empty(), "failed: {failures:?}");
    }

    #[test]
    fn split_runs_match_line_break_test_file() {
        let Some(tests) = read_break_test_file("MAKEPAD_LINE_BREAK_TEST_FILE") else {
            return;
        };
        let starts = get_test_starts();
        for (text, _) in &tests {
            check_split_runs(text, &starts);
            check_breaks_from_text_alone(text, &starts);
        }
        println!("Split runs: all {} tests get the same breaks at every split", tests.len());
    }

    #[test]
    fn conforms_to_grapheme_break_test_file() {
        let Some(tests) = read_break_test_file("MAKEPAD_GRAPHEME_BREAK_TEST_FILE") else {
            return;
        };
        let mut failures = Vec::new();
        for (text, expected) in &tests {
            let mut before = START;
            let mut found = Vec::new();
            for c in text.chars() {
                let props = get_char_props(c);
                found.push(is_grapheme_boundary(&before, props));
                before.push_char(props);
            }
            found.push(is_grapheme_boundary(&before, END.first_char));
            if &found != expected {
                failures.push(text);
            }
        }
        println!("UAX #29 conformance: {} of {} tests pass", tests.len() - failures.len(), tests.len());
        assert!(failures.is_empty(), "failed: {failures:?}");
    }
}
