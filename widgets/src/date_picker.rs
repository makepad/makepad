//! Date fields: a box you type a date into, the same box with a calendar
//! hanging under it, and the two-ended form for a start and an end.
//!
//! # What a date field does with a typo
//!
//! It keeps it. A date is eight to ten characters that have to be exactly
//! right, and the commonest thing that happens to one is a single wrong
//! digit. A field that answers that by emptying itself has thrown away nine
//! correct characters to punish one wrong one, and the person types the
//! whole thing again. So what will not parse stays in the box, a mark
//! appears beside it, and the field reports that it is holding no date —
//! the text is kept, the value is not.
//!
//! What parses is put back in the field's own shape when the field is left,
//! so `2026-9-1` becomes `2026-09-01` and a column of dates lines up.
//!
//! # The shape
//!
//! `format` is the shape, written with `YYYY`, `MM` and `DD` and whatever
//! separators are wanted between them: `YYYY-MM-DD`, `DD/MM/YYYY`,
//! `MM.DD.YYYY`. Reading is looser than writing — a month or a day may be
//! typed without its leading zero, and any of `-`, `/`, `.` or a space will
//! do for any other. A year may NOT be typed short: `26` is a year two
//! different people read two different ways, and there is nothing here that
//! could ask which was meant.
//!
//! # What these deliberately do not do
//!
//! **No clock and no timezone.** Nothing here asks what day it is. A host
//! that wants a field to open on today hands it today; the widget has no
//! opinion, because the answer depends on where the person is standing and
//! this crate has no way to find out.
//!
//! **No month names and no locale.** `format` writes digits. A field that
//! could show `10 Sept` would have to know a language, and there is nowhere
//! to ask which one.
//!
//! **No time of day.** These pick a date.
//!
//! # How the calendar is reached
//!
//! The picker hands the calendar a date, and reads back the date the
//! calendar is showing, as plain `YYYY-MM-DD` text through the widget's own
//! text — not through the calendar's Rust type. A date widget that could
//! not say which day it holds would not be a date widget, and going through
//! the text keeps two widgets that are useful apart from each other apart.
use crate::{
    button::ButtonWidgetRefExt,
    calendar::CivilDate,
    field_well::FieldWell,
    makepad_derive_widget::*,
    makepad_draw::*,
    popover::PopoverWidgetRefExt,
    text_input::{TextInputAction, TextInputWidgetRefExt},
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DateFieldBase = #(DateField::register_widget(vm))
    mod.widgets.DatePickerBase = #(DatePicker::register_widget(vm))
    mod.widgets.DateRangePickerBase = #(DateRangePicker::register_widget(vm))

    /** The mark a date field shows while what is typed will not parse.
     *
     * It lives in the well's leading slot and is invisible until it is
     * needed, so it costs no room in the ordinary case. Drawn as a glyph
     * rather than as a shape: a mark this small drawn as an outline path is
     * the one that does not paint. */
    mod.widgets.DateFieldWarn = mod.widgets.Label{
        width: Fit
        height: Fit
        visible: false
        text: "\u{f071}"
        draw_text +: {
            // An icon face's glyphs are pictures in their own box; centring
            // their ink moves them off the line the face asks for.
            ink_centered: false
            text_style: theme.font_icons{font_size: 9.}
            color: theme.color_error
        }
    }

    /** The button that opens a calendar under a field. */
    mod.widgets.DatePickerOpen = mod.widgets.Button{
        width: 20.
        height: 20.
        align: Align{x: 0.5, y: 0.5}
        padding: Inset{left: 0., right: 0., top: 0., bottom: 0.}
        margin: Inset{left: 0., right: 0., top: 0., bottom: 0.}
        text: "\u{f073}"
        draw_text +: {
            ink_centered: false
            text_style: theme.font_icons{font_size: 10.}
        }
    }

    /** A date typed into a box: parsed against `format`, put back in that
     * shape when the field is left, and kept as typed when it will not
     * parse. */
    mod.widgets.DateField = set_type_default() do mod.widgets.DateFieldBase{
        width: 150.
        // A real height, not Fit: the well and the input inside it are Fill
        // so they can be as tall as the box, and Fill inside Fit resolves to
        // nothing at all.
        height: 24.

        /** the shape a date is written in: YYYY, MM, DD and separators */
        format: "YYYY-MM-DD"
        /** the date the field opens on, written in `format`; empty for none */
        date: ""
        /** nothing in the field answers 0..1 step 1 */
        disabled: false

        well: mod.widgets.FieldWell{
            width: Fill
            height: Fill
            align: Align{y: 0.5}
            padding: Inset{left: theme.space_2, right: theme.space_2, top: 0., bottom: 0.}
            leading: mod.widgets.DateFieldWarn{}
            input: mod.widgets.WellInput{
                width: Fill
                height: Fill
                empty_text: ""
            }
        }
    }

    /** A date field with a calendar in a popover under it, opened by the
     * button at the field's trailing edge. */
    mod.widgets.DatePicker = set_type_default() do mod.widgets.DatePickerBase{
        width: 170.
        height: 24.

        /** the shape a date is written in: YYYY, MM, DD and separators */
        format: "YYYY-MM-DD"
        /** the date the picker opens on, written in `format`; empty for none */
        date: ""
        /** nothing in the picker answers 0..1 step 1 */
        disabled: false

        popover: mod.widgets.Popover{
            width: Fill
            height: Fill
            // Manual, not Click: the field is the anchor, and a press into
            // the text has to reach the text. Only the button opens this.
            trigger: mod.widgets.Manual
            placement: mod.widgets.BottomStart

            field := mod.widgets.DateField{
                width: Fill
                height: Fill
                well +: {
                    padding: Inset{left: theme.space_2, right: 2., top: 0., bottom: 0.}
                    trailing: mod.widgets.DatePickerOpen{}
                }
            }
            content +: {
                calendar := mod.widgets.Calendar{}
            }
        }
    }

    /** Two dates and two months: a start, an end, and a calendar for each
     * end side by side. The two are held in order however they are set. */
    mod.widgets.DateRangePicker = set_type_default() do mod.widgets.DateRangePickerBase{
        width: 330.
        height: 24.

        /** the shape both dates are written in */
        format: "YYYY-MM-DD"
        /** the date the range opens on, written in `format`; empty for none */
        start: ""
        /** the date the range ends on, written in `format`; empty for none */
        end: ""
        /** nothing in the picker answers 0..1 step 1 */
        disabled: false

        popover: mod.widgets.Popover{
            width: Fill
            height: Fill
            flow: Right
            align: Align{y: 0.5}
            spacing: theme.space_1
            trigger: mod.widgets.Manual
            placement: mod.widgets.BottomStart

            start_field := mod.widgets.DateField{width: Fill height: Fill}
            mod.widgets.Label{width: Fit text: "\u{2013}"}
            end_field := mod.widgets.DateField{width: Fill height: Fill}
            open := mod.widgets.DatePickerOpen{}

            content +: {
                flow: Right
                spacing: theme.space_3
                left := mod.widgets.Calendar{}
                right := mod.widgets.Calendar{}
            }
        }
    }
}

/// The shape the pickers and the calendars say dates to each other in,
/// whatever shape a field shows the person. Widest part first, so the text
/// sorts the way the dates do.
pub const EXCHANGE: &str = "YYYY-MM-DD";

/// One piece of a written date.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Part {
    Year,
    Month,
    Day,
    /// Anything else in the format, matched as itself.
    Mark(char),
}

/// Break a format into its pieces. Only `YYYY`, `MM` and `DD` are tokens;
/// every other character is a mark. `YY` is therefore two marks, which no
/// digit can match — a two-digit year is refused by construction rather
/// than by a rule somewhere else that could be forgotten.
fn parts(format: &str) -> Vec<Part> {
    let chars: Vec<char> = format.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let rest = &chars[at..];
        if rest.starts_with(&['Y', 'Y', 'Y', 'Y']) {
            out.push(Part::Year);
            at += 4;
        } else if rest.starts_with(&['M', 'M']) {
            out.push(Part::Month);
            at += 2;
        } else if rest.starts_with(&['D', 'D']) {
            out.push(Part::Day);
            at += 2;
        } else {
            out.push(Part::Mark(rest[0]));
            at += 1;
        }
    }
    out
}

/// A separator is a separator. People type whichever of these their hand is
/// nearest to, and refusing a date because it came with slashes instead of
/// dashes is refusing a date that was read correctly.
fn same_mark(want: char, got: char) -> bool {
    const MARKS: &[char] = &['-', '/', '.', ' '];
    want == got || (MARKS.contains(&want) && MARKS.contains(&got))
}

/// Take between `least` and `most` digits, greedily.
fn take_digits(text: &[char], at: &mut usize, least: usize, most: usize) -> Option<u32> {
    let mut value = 0u32;
    let mut taken = 0;
    while taken < most {
        match text.get(*at + taken) {
            Some(c) if c.is_ascii_digit() => {
                value = value * 10 + (*c as u32 - '0' as u32);
                taken += 1;
            }
            _ => break,
        }
    }
    if taken < least {
        return None;
    }
    *at += taken;
    Some(value)
}

/// Read a date written in `format`, or nothing.
///
/// Reading is looser than writing in two places, both of them about what a
/// person's hands do rather than about what the format says: a month or a
/// day may arrive without its leading zero, and any separator will do for
/// any other. The year may not arrive short — see [`parts`].
///
/// Anything left over at the end is a refusal. A field that read `2026-01-01`
/// out of `2026-01-01-01` would be inventing a date nobody typed.
pub fn parse_date(format: &str, text: &str) -> Option<CivilDate> {
    let text: Vec<char> = text.trim().chars().collect();
    let mut at = 0usize;
    let (mut year, mut month, mut day) = (None, None, None);
    for part in parts(format) {
        match part {
            Part::Year => year = Some(take_digits(&text, &mut at, 4, 4)? as i32),
            Part::Month => month = Some(take_digits(&text, &mut at, 1, 2)?),
            Part::Day => day = Some(take_digits(&text, &mut at, 1, 2)?),
            Part::Mark(want) => {
                if !same_mark(want, *text.get(at)?) {
                    return None;
                }
                at += 1;
            }
        }
    }
    if at != text.len() {
        return None;
    }
    // The month and day ranges, the length of the month and the leap year
    // are all one question, and the civil-date core is the one place that
    // answers it.
    CivilDate::new(year?, month?, day?)
}

/// Write a date in `format`. Months and days always carry their leading
/// zero, whatever was typed, so a column of dates lines up.
pub fn format_date(format: &str, date: CivilDate) -> String {
    let mut out = String::new();
    for part in parts(format) {
        match part {
            Part::Year => out.push_str(&format!("{:04}", date.year)),
            Part::Month => out.push_str(&format!("{:02}", date.month)),
            Part::Day => out.push_str(&format!("{:02}", date.day)),
            Part::Mark(c) => out.push(c),
        }
    }
    out
}

/// The two ends of a range while they are being chosen, and the rules that
/// keep them in order. Apart from the widget so it can be tested without a
/// script heap, and so the three ways a range can be set — two presses in a
/// calendar, a date typed in either box, a host calling in — all settle it
/// the same way.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct DateRange {
    pub start: Option<CivilDate>,
    pub end: Option<CivilDate>,
}

impl DateRange {
    /// A press on a day in either calendar.
    ///
    /// A range takes two presses. The second one BEFORE the first is read
    /// as the pair rather than as a mistake: the two presses are the two
    /// ends, and the order they were made in says nothing about which end
    /// is earlier. Throwing the first press away instead would make
    /// choosing a range backwards cost three presses.
    pub fn pick(&mut self, day: CivilDate) {
        match (self.start, self.end) {
            (Some(start), None) => {
                if day < start {
                    self.start = Some(day);
                    self.end = Some(start);
                } else {
                    self.end = Some(day);
                }
            }
            // Nothing chosen yet, or a whole range already chosen: this
            // press starts a new one.
            _ => {
                self.start = Some(day);
                self.end = None;
            }
        }
    }

    /// A start typed into the start box. The end gives way if it has to:
    /// the end the person is touching is the one that means something, and
    /// moving the one they are not would silently undo a choice they made
    /// on purpose.
    pub fn take_start(&mut self, date: Option<CivilDate>) {
        self.start = date;
        if let (Some(start), Some(end)) = (self.start, self.end) {
            if end < start {
                self.end = Some(start);
            }
        }
    }

    /// An end typed into the end box; the start gives way if it has to.
    pub fn take_end(&mut self, date: Option<CivilDate>) {
        self.end = date;
        if let (Some(start), Some(end)) = (self.start, self.end) {
            if end < start {
                self.start = Some(end);
            }
        }
    }

    /// Both ends, once both are known.
    pub fn settled(&self) -> Option<(CivilDate, CivilDate)> {
        match (self.start, self.end) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        }
    }
}

/// What day a calendar says it is showing. `text` first, since that is what
/// a widget says about itself; the snapshot value is the same answer by
/// another road, for a calendar that spends its text on something else.
fn calendar_text(calendar: &WidgetRef, cx: &Cx) -> String {
    let text = calendar.text();
    if text.is_empty() {
        calendar.snapshot_value(cx).unwrap_or_default()
    } else {
        text
    }
}

/// Put a date in front of a calendar, and remember what the calendar ends
/// up showing.
///
/// The remembering is the point. A calendar arrives already holding a day,
/// and a picker that had not looked before it opened would read that day as
/// the person's first press the moment the panel appeared.
fn sync_calendar(calendar: &WidgetRef, cx: &mut Cx, date: Option<CivilDate>, seen: &mut String) {
    if let Some(date) = date {
        calendar.set_text(cx, &format_date(EXCHANGE, date));
    }
    *seen = calendar_text(calendar, cx);
}

/// A day the person pressed in this calendar since it was last looked at.
fn take_calendar_pick(calendar: &WidgetRef, cx: &Cx, seen: &mut String) -> Option<CivilDate> {
    let now = calendar_text(calendar, cx);
    if now == *seen {
        return None;
    }
    *seen = now.clone();
    parse_date(EXCHANGE, &now)
}

#[derive(Clone, Debug, Default)]
pub enum DateFieldAction {
    /// The date the box now holds, or `None` when it holds text that will
    /// not parse — the text stays, the date does not.
    Changed(Option<CivilDate>),
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct DateField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[find]
    #[redraw]
    #[live]
    pub well: WidgetRef,
    #[walk]
    walk: Walk,

    /// The shape a date is written in.
    #[live]
    pub format: String,
    /// The date the field opens on, written in `format`. Read whenever it
    /// changes, so a live edit of it arrives the way the first one did.
    #[live]
    pub date: String,
    #[live]
    pub disabled: bool,

    #[rust]
    value: Option<CivilDate>,
    /// The box holds text that will not parse.
    #[rust]
    invalid: bool,
    /// The text last written into the input, so a redraw does not fight the
    /// caret while somebody is typing.
    #[rust]
    shown: String,
    /// The `date` last taken, so re-applying the same one does not stamp on
    /// what has been typed since.
    #[rust]
    pushed: String,
}

impl ScriptHook for DateField {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let input = self.input().as_text_input();
        // The format is the placeholder: an empty box that says what shape
        // it wants is the cheapest instruction there is. A caller who wrote
        // their own keeps it.
        let name_the_shape = input.empty_text().is_empty();
        let shape = self.fmt().to_string();
        let date = if self.date != self.pushed {
            self.pushed = self.date.clone();
            Some(self.date.clone())
        } else {
            None
        };
        let disabled = self.disabled;
        let cx = vm.cx_mut();
        if name_the_shape {
            input.set_empty_text(cx, shape);
        }
        if let Some(date) = date {
            self.set_date_text(cx, &date);
        }
        if disabled {
            self.well.set_disabled(cx, true);
        }
    }
}

impl DateField {
    /// The shape in force. A field built straight off the base preset has
    /// no format at all, and refusing every date it is shown would be a
    /// strange way to say so.
    fn fmt(&self) -> &str {
        if self.format.is_empty() {
            EXCHANGE
        } else {
            &self.format
        }
    }

    fn input(&self) -> WidgetRef {
        self.well
            .borrow::<FieldWell>()
            .map(|well| well.input.clone())
            .unwrap_or_else(WidgetRef::empty)
    }

    fn leading(&self) -> WidgetRef {
        self.well
            .borrow::<FieldWell>()
            .map(|well| well.leading.clone())
            .unwrap_or_else(WidgetRef::empty)
    }

    /// The well's trailing slot, which is where a picker keeps the button
    /// that opens its calendar.
    pub fn trailing(&self) -> WidgetRef {
        self.well
            .borrow::<FieldWell>()
            .map(|well| well.trailing.clone())
            .unwrap_or_else(WidgetRef::empty)
    }

    fn write_text(&mut self, cx: &mut Cx) {
        let text = match self.value {
            Some(date) => format_date(self.fmt(), date),
            None => String::new(),
        };
        if self.shown == text {
            return;
        }
        self.shown = text.clone();
        self.input().as_text_input().set_text(cx, &text);
    }

    /// Show or hide the mark beside the text.
    fn show_invalid(&mut self, cx: &mut Cx, invalid: bool) {
        if self.invalid == invalid {
            return;
        }
        self.invalid = invalid;
        self.leading().set_visible(cx, invalid);
        self.well.redraw(cx);
    }

    /// Read whatever is in the box, and report the date it now holds.
    ///
    /// Text that will not parse is left exactly as it was typed. The field
    /// says it holds no date, which is true, and says so beside the text
    /// rather than by emptying it: one wrong digit is a typo to fix, not
    /// nine right characters to retype.
    fn take_typed(&mut self, cx: &mut Cx, typed: &str) {
        let trimmed = typed.trim();
        let parsed = if trimmed.is_empty() {
            None
        } else {
            parse_date(self.fmt(), trimmed)
        };
        let invalid = !trimmed.is_empty() && parsed.is_none();
        let moved = parsed != self.value;
        self.value = parsed;
        self.show_invalid(cx, invalid);
        if !invalid {
            // Cleared, not compared: the box may hold the same date in a
            // looser spelling than the one last written, and that spelling
            // is what the person is waiting to see tidied.
            self.shown.clear();
            self.write_text(cx);
        }
        if moved {
            cx.widget_action(self.uid, DateFieldAction::Changed(self.value));
        }
    }

    /// Put a date into the box as text, the way the DSL's `date` does.
    pub fn set_date_text(&mut self, cx: &mut Cx, text: &str) {
        let trimmed = text.trim();
        self.value = if trimmed.is_empty() {
            None
        } else {
            parse_date(self.fmt(), trimmed)
        };
        self.show_invalid(cx, false);
        self.shown.clear();
        self.write_text(cx);
    }

    /// Change the shape, and put what is showing back in the new one.
    pub fn set_format(&mut self, cx: &mut Cx, format: &str) {
        if self.format == format {
            return;
        }
        let was = self.fmt().to_string();
        self.format = format.to_string();
        let input = self.input().as_text_input();
        // The placeholder follows the format only while it IS the format.
        if input.empty_text() == was {
            input.set_empty_text(cx, self.fmt().to_string());
        }
        if self.invalid {
            return;
        }
        self.shown.clear();
        self.write_text(cx);
    }

    pub fn selected(&self) -> Option<CivilDate> {
        self.value
    }

    /// Set the date from a host. Silent: the host already knows.
    pub fn set_selected(&mut self, cx: &mut Cx, date: Option<CivilDate>) {
        if self.value == date && !self.invalid {
            return;
        }
        self.value = date;
        self.show_invalid(cx, false);
        self.shown.clear();
        self.write_text(cx);
    }

    /// Whether the box is holding text that will not parse.
    pub fn text_is_invalid(&self) -> bool {
        self.invalid
    }
}

impl Widget for DateField {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.well.set_disabled(cx, disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The well is drawn here rather than by a container, so nothing else
        // puts it in the tree; without this a host cannot reach the input
        // inside it at all.
        cx.widget_tree_insert_child(self.uid, live_id!(well), self.well.clone());
        self.well.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        for action in cx.capture_actions(|cx| self.well.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::Returned(text, _) => {
                    self.take_typed(cx, &text);
                }
                // Leaving the field settles it too, so a date typed and then
                // clicked away from is not left half-read. The action carries
                // nothing, so the box is asked.
                TextInputAction::KeyFocusLost => {
                    let typed = self.input().as_text_input().text();
                    self.take_typed(cx, &typed);
                }
                TextInputAction::Escaped => {
                    self.show_invalid(cx, false);
                    self.shown.clear();
                    self.write_text(cx);
                }
                _ => {}
            }
        }
    }

    /// What is in the box, which while a typo is being fixed is not the same
    /// as the date the field holds.
    fn text(&self) -> String {
        self.input().as_text_input().text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_date_text(cx, v);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.text())
    }
}

impl DateFieldRef {
    /// The date the box holds, or nothing while it holds a typo.
    pub fn selected(&self) -> Option<CivilDate> {
        self.borrow().and_then(|inner| inner.selected())
    }

    pub fn set_selected(&self, cx: &mut Cx, date: Option<CivilDate>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, date);
        }
    }

    pub fn set_date_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_date_text(cx, text);
        }
    }

    pub fn set_format(&self, cx: &mut Cx, format: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_format(cx, format);
        }
    }

    /// The well's trailing slot.
    pub fn trailing(&self) -> WidgetRef {
        self.borrow()
            .map(|inner| inner.trailing())
            .unwrap_or_default()
    }

    pub fn text_is_invalid(&self) -> bool {
        self.borrow()
            .map(|inner| inner.text_is_invalid())
            .unwrap_or(false)
    }

    /// The date this field settled on, when it settled in `actions`.
    /// `Some(None)` is a real answer: the box is holding a typo.
    pub fn changed(&self, actions: &Actions) -> Option<Option<CivilDate>> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            DateFieldAction::Changed(date) => Some(date),
            DateFieldAction::None => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DatePickerAction {
    /// The date the picker now holds, however it was set.
    Changed(Option<CivilDate>),
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct DatePicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[find]
    #[redraw]
    #[live]
    pub popover: WidgetRef,
    #[walk]
    walk: Walk,

    /// The shape, and the date to open on. Both live here as well as on the
    /// field inside, so a caller writes `DatePicker{format: "DD/MM/YYYY"}`
    /// instead of a path through two slots to reach the same property.
    #[live]
    pub format: String,
    #[live]
    pub date: String,
    #[live]
    pub disabled: bool,

    /// What the calendar was last seen showing, so a press in it can be
    /// told from the day it was already on.
    #[rust]
    seen: String,
    #[rust]
    pushed: String,
}

impl ScriptHook for DatePicker {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let field = self.field();
        let format = self.format.clone();
        let date = if self.date != self.pushed {
            self.pushed = self.date.clone();
            Some(self.date.clone())
        } else {
            None
        };
        let disabled = self.disabled;
        let cx = vm.cx_mut();
        if !format.is_empty() {
            field.set_format(cx, &format);
        }
        if let Some(date) = date {
            field.set_date_text(cx, &date);
        }
        if disabled {
            self.popover
                .child_by_path(ids!(field))
                .set_disabled(cx, true);
        }
    }
}

impl DatePicker {
    fn field(&self) -> DateFieldRef {
        self.popover.child_by_path(ids!(field)).as_date_field()
    }

    fn calendar(&self) -> WidgetRef {
        self.popover
            .as_popover()
            .content()
            .child_by_path(ids!(calendar))
    }

    pub fn selected(&self) -> Option<CivilDate> {
        self.field().selected()
    }

    pub fn set_selected(&self, cx: &mut Cx, date: Option<CivilDate>) {
        self.field().set_selected(cx, date);
    }

    pub fn is_open(&self) -> bool {
        self.popover.as_popover().is_open()
    }
}

impl Widget for DatePicker {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.popover
            .child_by_path(ids!(field))
            .set_disabled(cx, disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.widget_tree_insert_child(self.uid, live_id!(popover), self.popover.clone());
        self.popover.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let actions = cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope));
        let popover = self.popover.as_popover();
        let field = self.field();

        if field.trailing().as_button().clicked(&actions) {
            // The calendar opens on the day the field is holding, so it
            // lands on the month the person is already thinking about.
            let date = field.selected();
            let calendar = self.calendar();
            sync_calendar(&calendar, cx, date, &mut self.seen);
            popover.open(cx);
        }
        if let Some(date) = field.changed(&actions) {
            let calendar = self.calendar();
            sync_calendar(&calendar, cx, date, &mut self.seen);
            cx.widget_action(self.uid, DatePickerAction::Changed(date));
        }
        if popover.is_open() {
            let calendar = self.calendar();
            if let Some(day) = take_calendar_pick(&calendar, cx, &mut self.seen) {
                field.set_selected(cx, Some(day));
                // One press is the whole answer here, so the panel goes
                // away rather than waiting to be dismissed.
                popover.close(cx);
                cx.widget_action(self.uid, DatePickerAction::Changed(Some(day)));
            }
        }
    }

    fn text(&self) -> String {
        self.popover.child_by_path(ids!(field)).text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.field().set_date_text(cx, v);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.text())
    }
}

impl DatePickerRef {
    pub fn selected(&self) -> Option<CivilDate> {
        self.borrow().and_then(|inner| inner.selected())
    }

    pub fn set_selected(&self, cx: &mut Cx, date: Option<CivilDate>) {
        if let Some(inner) = self.borrow() {
            inner.set_selected(cx, date);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// The date this picker settled on, when it settled in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<Option<CivilDate>> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            DatePickerAction::Changed(date) => Some(date),
            DatePickerAction::None => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DateRangePickerAction {
    /// Both ends, reported once both are known and in order.
    Changed(CivilDate, CivilDate),
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct DateRangePicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[find]
    #[redraw]
    #[live]
    pub popover: WidgetRef,
    #[walk]
    walk: Walk,

    #[live]
    pub format: String,
    #[live]
    pub start: String,
    #[live]
    pub end: String,
    #[live]
    pub disabled: bool,

    #[rust]
    range: DateRange,
    #[rust]
    seen_left: String,
    #[rust]
    seen_right: String,
    #[rust]
    pushed: (String, String),
}

impl ScriptHook for DateRangePicker {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let format = self.format.clone();
        let ends = if self.pushed != (self.start.clone(), self.end.clone()) {
            self.pushed = (self.start.clone(), self.end.clone());
            Some((self.start.clone(), self.end.clone()))
        } else {
            None
        };
        let disabled = self.disabled;
        let start_field = self.start_field();
        let end_field = self.end_field();
        let cx = vm.cx_mut();
        if !format.is_empty() {
            start_field.set_format(cx, &format);
            end_field.set_format(cx, &format);
        }
        if let Some((start, end)) = ends {
            // Through the range rather than straight into the two boxes, so
            // a pair written the wrong way round in the DSL is put in order
            // once instead of being shown out of order forever.
            let fmt: &str = if format.is_empty() {
                EXCHANGE
            } else {
                format.as_str()
            };
            self.range.take_start(parse_date(fmt, &start));
            self.range.take_end(parse_date(fmt, &end));
            start_field.set_selected(cx, self.range.start);
            end_field.set_selected(cx, self.range.end);
        }
        if disabled {
            self.set_slots_disabled(cx, true);
        }
    }
}

impl DateRangePicker {
    fn start_field(&self) -> DateFieldRef {
        self.popover.child_by_path(ids!(start_field)).as_date_field()
    }

    fn end_field(&self) -> DateFieldRef {
        self.popover.child_by_path(ids!(end_field)).as_date_field()
    }

    fn left(&self) -> WidgetRef {
        self.popover.as_popover().content().child_by_path(ids!(left))
    }

    fn right(&self) -> WidgetRef {
        self.popover
            .as_popover()
            .content()
            .child_by_path(ids!(right))
    }

    fn set_slots_disabled(&self, cx: &mut Cx, disabled: bool) {
        self.popover
            .child_by_path(ids!(start_field))
            .set_disabled(cx, disabled);
        self.popover
            .child_by_path(ids!(end_field))
            .set_disabled(cx, disabled);
        self.popover
            .child_by_path(ids!(open))
            .set_disabled(cx, disabled);
    }

    /// The left calendar is the start and the right one is the end, so the
    /// two panels are the two ends of the range rather than two arbitrary
    /// months. While an end is not chosen its calendar is left where it is:
    /// moving it to the other end's month would show the same month twice
    /// and mark a day nobody picked.
    fn show_ends(&mut self, cx: &mut Cx) {
        let (left, right) = (self.left(), self.right());
        let (start, end) = (self.range.start, self.range.end);
        sync_calendar(&left, cx, start, &mut self.seen_left);
        sync_calendar(&right, cx, end, &mut self.seen_right);
    }

    fn write_fields(&self, cx: &mut Cx) {
        self.start_field().set_selected(cx, self.range.start);
        self.end_field().set_selected(cx, self.range.end);
    }

    /// A day pressed in either calendar since they were last looked at.
    fn take_pick(&mut self, cx: &Cx) -> Option<CivilDate> {
        let (left, right) = (self.left(), self.right());
        if let Some(day) = take_calendar_pick(&left, cx, &mut self.seen_left) {
            return Some(day);
        }
        take_calendar_pick(&right, cx, &mut self.seen_right)
    }

    pub fn range(&self) -> Option<(CivilDate, CivilDate)> {
        self.range.settled()
    }

    pub fn set_range(&mut self, cx: &mut Cx, start: CivilDate, end: CivilDate) {
        self.range.take_start(Some(start));
        self.range.take_end(Some(end));
        self.write_fields(cx);
    }
}

impl Widget for DateRangePicker {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.set_slots_disabled(cx, disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.widget_tree_insert_child(self.uid, live_id!(popover), self.popover.clone());
        self.popover.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let actions = cx.capture_actions(|cx| self.popover.handle_event(cx, event, scope));
        let popover = self.popover.as_popover();

        if self
            .popover
            .child_by_path(ids!(open))
            .as_button()
            .clicked(&actions)
        {
            self.show_ends(cx);
            popover.open(cx);
        }

        let mut moved = false;
        let mut picked = false;
        if let Some(date) = self.start_field().changed(&actions) {
            self.range.take_start(date);
            moved = true;
        }
        if let Some(date) = self.end_field().changed(&actions) {
            self.range.take_end(date);
            moved = true;
        }
        if popover.is_open() {
            if let Some(day) = self.take_pick(cx) {
                self.range.pick(day);
                moved = true;
                picked = true;
            }
        }

        if moved {
            self.write_fields(cx);
            self.show_ends(cx);
            if let Some((start, end)) = self.range.settled() {
                cx.widget_action(self.uid, DateRangePickerAction::Changed(start, end));
                // The panel stays up between the two presses and goes away
                // once the range is whole: a range is not chosen until both
                // ends are, and closing after the first press would make the
                // second one cost another trip to the button.
                if picked {
                    popover.close(cx);
                }
            }
        }
    }

    fn text(&self) -> String {
        match self.range.settled() {
            Some((start, end)) => {
                let fmt: &str = if self.format.is_empty() {
                    EXCHANGE
                } else {
                    self.format.as_str()
                };
                format!(
                    "{} \u{2013} {}",
                    format_date(fmt, start),
                    format_date(fmt, end)
                )
            }
            None => String::new(),
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.text())
    }
}

impl DateRangePickerRef {
    /// Both ends, once both are chosen.
    pub fn range(&self) -> Option<(CivilDate, CivilDate)> {
        self.borrow().and_then(|inner| inner.range())
    }

    pub fn set_range(&self, cx: &mut Cx, start: CivilDate, end: CivilDate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_range(cx, start, end);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow()
            .map(|inner| inner.popover.as_popover().is_open())
            .unwrap_or(false)
    }

    /// Both ends, when the range settled in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<(CivilDate, CivilDate)> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            DateRangePickerAction::Changed(start, end) => Some((start, end)),
            DateRangePickerAction::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> CivilDate {
        CivilDate::new(year, month, day).expect("a real date")
    }

    #[test]
    fn a_written_date_is_read_back() {
        assert_eq!(parse_date(EXCHANGE, "2026-09-10"), Some(day(2026, 9, 10)));
        assert_eq!(format_date(EXCHANGE, day(2026, 9, 10)), "2026-09-10");
    }

    #[test]
    fn the_format_decides_the_order() {
        assert_eq!(
            parse_date("DD/MM/YYYY", "10/09/2026"),
            Some(day(2026, 9, 10))
        );
        assert_eq!(
            parse_date("MM.DD.YYYY", "09.10.2026"),
            Some(day(2026, 9, 10))
        );
        assert_eq!(format_date("DD/MM/YYYY", day(2026, 9, 10)), "10/09/2026");
    }

    #[test]
    fn what_is_not_a_date_is_refused() {
        for text in ["", "hello", "2026-09", "2026-09-10-11", "----------"] {
            assert_eq!(parse_date(EXCHANGE, text), None, "{text:?} is not a date");
        }
    }

    #[test]
    fn a_month_or_a_day_out_of_range_is_refused() {
        assert_eq!(parse_date(EXCHANGE, "2026-13-01"), None, "no thirteenth month");
        assert_eq!(parse_date(EXCHANGE, "2026-00-01"), None, "no zeroth month");
        assert_eq!(parse_date(EXCHANGE, "2026-02-30"), None, "no 30 February");
        assert_eq!(parse_date(EXCHANGE, "2026-04-31"), None, "April has thirty");
        assert_eq!(parse_date(EXCHANGE, "2026-01-00"), None, "no zeroth day");
    }

    #[test]
    fn a_two_digit_year_is_refused() {
        // 26 is a year two different people read two different ways, and
        // there is nothing here that could ask which was meant.
        assert_eq!(parse_date(EXCHANGE, "26-09-10"), None);
        assert_eq!(parse_date("DD/MM/YYYY", "10/09/26"), None);
    }

    #[test]
    fn a_leap_day_exists_only_in_a_leap_year() {
        assert_eq!(parse_date(EXCHANGE, "2024-02-29"), Some(day(2024, 2, 29)));
        assert_eq!(parse_date(EXCHANGE, "2026-02-29"), None);
    }

    #[test]
    fn a_missing_leading_zero_is_put_back() {
        let parsed = parse_date(EXCHANGE, "2026-9-1").expect("read without the zeros");
        assert_eq!(format_date(EXCHANGE, parsed), "2026-09-01");
    }

    #[test]
    fn a_separator_is_a_separator() {
        // Refusing a date that was read correctly because the hand reached
        // for the nearer key is a refusal nobody can act on.
        assert_eq!(parse_date(EXCHANGE, "2026/09/10"), Some(day(2026, 9, 10)));
        assert_eq!(parse_date(EXCHANGE, "2026.09.10"), Some(day(2026, 9, 10)));
        assert_eq!(parse_date("DD-MM-YYYY", "10 09 2026"), Some(day(2026, 9, 10)));
    }

    #[test]
    fn surrounding_space_is_not_a_typo() {
        assert_eq!(parse_date(EXCHANGE, "  2026-09-10 "), Some(day(2026, 9, 10)));
    }

    #[test]
    fn two_presses_are_the_two_ends() {
        let mut range = DateRange::default();
        range.pick(day(2026, 9, 10));
        assert_eq!(range.settled(), None, "one end is not a range");
        range.pick(day(2026, 9, 20));
        assert_eq!(range.settled(), Some((day(2026, 9, 10), day(2026, 9, 20))));
    }

    #[test]
    fn a_second_press_before_the_first_is_the_pair() {
        let mut range = DateRange::default();
        range.pick(day(2026, 9, 20));
        range.pick(day(2026, 9, 10));
        assert_eq!(
            range.settled(),
            Some((day(2026, 9, 10), day(2026, 9, 20))),
            "the order the presses were made in says nothing about which end is earlier"
        );
    }

    #[test]
    fn the_same_day_twice_is_a_one_day_range() {
        let mut range = DateRange::default();
        range.pick(day(2026, 9, 10));
        range.pick(day(2026, 9, 10));
        assert_eq!(range.settled(), Some((day(2026, 9, 10), day(2026, 9, 10))));
    }

    #[test]
    fn a_third_press_starts_a_new_range() {
        let mut range = DateRange::default();
        range.pick(day(2026, 9, 10));
        range.pick(day(2026, 9, 20));
        range.pick(day(2026, 10, 1));
        assert_eq!(range.start, Some(day(2026, 10, 1)));
        assert_eq!(range.settled(), None, "and waits for its other end");
    }

    #[test]
    fn typing_a_start_past_the_end_moves_the_end() {
        // The end the person is touching is the one that means something.
        let mut range = DateRange::default();
        range.take_start(Some(day(2026, 9, 1)));
        range.take_end(Some(day(2026, 9, 10)));
        range.take_start(Some(day(2026, 9, 20)));
        assert_eq!(range.settled(), Some((day(2026, 9, 20), day(2026, 9, 20))));
    }

    #[test]
    fn typing_an_end_before_the_start_moves_the_start() {
        let mut range = DateRange::default();
        range.take_start(Some(day(2026, 9, 10)));
        range.take_end(Some(day(2026, 9, 20)));
        range.take_end(Some(day(2026, 9, 1)));
        assert_eq!(range.settled(), Some((day(2026, 9, 1), day(2026, 9, 1))));
    }

    #[test]
    fn a_typo_in_one_box_leaves_the_other_end_alone() {
        let mut range = DateRange::default();
        range.take_start(Some(day(2026, 9, 10)));
        range.take_end(Some(day(2026, 9, 20)));
        // What will not parse arrives as None: the box keeps the text, the
        // range loses that end and nothing else.
        range.take_start(None);
        assert_eq!(range.start, None);
        assert_eq!(range.end, Some(day(2026, 9, 20)));
        assert_eq!(range.settled(), None);
    }
}
