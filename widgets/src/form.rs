//! Form — a controller that knows the fields under it by the path each one
//! is bound to.
//!
//! A field on its own can say whether what it holds is acceptable. What it
//! cannot say is whether the FORM is, and that is the question the send
//! button is actually asking. So this walks the fields under it, keeps the
//! state each one needs to know when to speak, and answers with the AND of
//! them.
//!
//! # Why the controller earns its place
//!
//! One behaviour: **a refused submit puts the keyboard in the first field
//! that is wrong.** Everything else here — the state per field, the moments,
//! the message under the input — a caller could hand-roll per form and often
//! does. What a caller cannot hand-roll without a controller is the ORDER,
//! because no single field knows where it sits among the others or whether
//! anything above it already failed. Without it a refused submit leaves a
//! person to hunt down the page for the red line, and on a long form they
//! scroll past it.
//!
//! # When a rule speaks
//!
//! Each rule says its own moment: `change` while the value is being typed,
//! `blur` when the keyboard leaves, `submit` only when the form is sent. The
//! three are a ladder, not three separate lists — a rule due on change is
//! still due at blur and at submit. A rule with no moment written on it is a
//! blur rule, because the moment a person is done typing is the moment they
//! are ready to hear about it, and complaining on every keystroke while
//! somebody is halfway through an address is the commonest way a form
//! becomes unpleasant.
//!
//! A field says nothing at all until it has been touched — until the
//! keyboard has been in it and left, or a submit has been refused. An
//! untouched field is still *evaluated*, since the form's validity is the
//! AND of every field whether or not it has been visited; it just does not
//! shout about a value nobody has typed yet.
//!
//! # What it is not
//!
//! It is not a data binder. Nothing here writes to a model, watches one, or
//! knows what a server thinks; `bind` is a name a field answers to inside
//! this form and nothing more. There is no async rule — a rule is a pure
//! function of the values on the page, and a check that has to ask a server
//! whether a name is taken belongs in the host, which can set the message
//! itself.
//!
//! It does not repaint the input when the value is wrong. The well has
//! hover, focus and disabled states and no invalid one, and adding a fourth
//! would mean a second copy of every field's chrome; the line under the
//! field is the signal.
//!
//! It only knows `FormField`s. A bare `TextInput` dropped into a form is
//! laid out and drawn like anything else and is invisible to the rules,
//! because the script layer offers no way to read a `bind` property off an
//! arbitrary widget without the heap that made it.
use crate::{
    field::FieldWellWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::TextInputWidgetRefExt,
    view::View,
    widget::*,
    widget_async::ScriptAsyncResult,
    CxWidgetExt,
};
use std::collections::BTreeMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FormBase = #(Form::register_widget(vm))
    mod.widgets.FormFieldBase = #(FormField::register_widget(vm))

    /** The words above a field's input. */
    mod.widgets.FormFieldLabel = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: 0.0
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** The line under a field that says what is wrong with it. */
    mod.widgets.FormFieldMessage = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: 0.0
        visible: false
        draw_text +: {
            color: theme.color_error
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** One bound field: a label, an input, and the line that says what is
     * wrong with it.
     *
     * The three are properties, not named children — write
     * `input: FieldWell{...}`, since `input := FieldWell{...}` builds a
     * child beside the slot and leaves the slot empty. */
    mod.widgets.FormField = set_type_default() do mod.widgets.FormFieldBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1

        /** the name this field answers to inside its form */
        bind: ""
        /** the words above the input; empty leaves the label slot alone */
        title: ""

        label: mod.widgets.FormFieldLabel{}
        input: mod.widgets.FieldWell{input: mod.widgets.WellInput{}}
        message: mod.widgets.FormFieldMessage{}
    }

    /** The controller over the fields written inside it. */
    mod.widgets.Form = set_type_default() do mod.widgets.FormBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        /** nothing under the form answers, and a submit is refused */
        disabled: false
    }
}

// ---------------------------------------------------------------------------
// The rule engine. No Cx, no script heap, no widgets: a rule is a pure
// function of the values on the page, which is what makes it testable and
// what keeps "when does this speak" out of the drawing code.
// ---------------------------------------------------------------------------

/// Every field's value by the path it is bound to. The rules that read
/// another field read it from here.
pub type Values = BTreeMap<String, String>;

/// When a rule is due.
///
/// The order is the ladder: a rule due at `Change` is also due at `Blur` and
/// at `Submit`. Deriving `Ord` from the declaration order is what makes
/// `rule.when <= moment` mean that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum When {
    /// While the value is being typed.
    Change,
    /// When the keyboard leaves the field.
    Blur,
    /// Only when the form is sent.
    Submit,
}

impl When {
    fn parse(word: &str) -> Option<When> {
        match word {
            "change" => Some(When::Change),
            "blur" => Some(When::Blur),
            "submit" => Some(When::Submit),
            _ => None,
        }
    }
}

/// One test a value must pass.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// Something, once the surrounding whitespace is gone.
    Required,
    MinLen(usize),
    MaxLen(usize),
    /// Parses as a number, and lands between the two inclusive.
    Range(f64, f64),
    /// Matches a shape — see `matches_mask`.
    Pattern(String),
    /// Equal to the value of another field, by path.
    SameAs(String),
}

/// A rule as a field writes it: when it is due, what it tests, and what to
/// say when it fails.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub when: When,
    pub check: Check,
    /// What the field says instead of the default wording.
    pub message: Option<String>,
}

/// One field as the engine sees it, with no widget attached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldState {
    pub bind: String,
    pub rules: Vec<Rule>,
    pub value: String,
}

/// A shape, not a language.
///
/// `#` is one digit, `@` one letter, `*` any one character, `\` makes the
/// next character literal, and everything else stands for itself. The value
/// must be exactly as long as the mask.
///
/// There is no regular-expression engine in this repository, and the
/// patterns fields actually need — a postcode, a phone number, a date, a
/// reference — are shapes rather than languages. A mask says the shape in a
/// form a person reading the DSL can check at a glance, which a regular
/// expression famously does not.
pub fn matches_mask(value: &str, mask: &str) -> bool {
    let mut chars = value.chars();
    let mut marks = mask.chars();
    loop {
        let Some(mark) = marks.next() else {
            return chars.next().is_none();
        };
        let literal = if mark == '\\' {
            match marks.next() {
                Some(escaped) => Some(escaped),
                // A mask ending in a lone backslash asked for a character
                // that was never written; nothing can match it.
                None => return false,
            }
        } else {
            None
        };
        let Some(c) = chars.next() else {
            return false;
        };
        let ok = match literal {
            Some(escaped) => c == escaped,
            None => match mark {
                '#' => c.is_ascii_digit(),
                '@' => c.is_alphabetic(),
                '*' => true,
                other => c == other,
            },
        };
        if !ok {
            return false;
        }
    }
}

/// A number as a person would write it in a sentence: no trailing `.0`.
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

impl Check {
    /// Whether `value` passes, given every field's value by path.
    pub fn passes(&self, value: &str, values: &Values) -> bool {
        let v = value.trim();
        // Only `required` has anything to say about an empty field. A
        // length, a range or a shape that also complained about emptiness
        // would quietly make every optional field on the form compulsory,
        // and a form nobody can submit without filling in the fields it
        // called optional is the failure this line exists to prevent.
        if v.is_empty() && !matches!(self, Check::Required) {
            return true;
        }
        match self {
            Check::Required => !v.is_empty(),
            Check::MinLen(n) => v.chars().count() >= *n,
            Check::MaxLen(n) => v.chars().count() <= *n,
            Check::Range(lo, hi) => match v.parse::<f64>() {
                Ok(n) => n >= *lo && n <= *hi,
                Err(_) => false,
            },
            Check::Pattern(mask) => matches_mask(v, mask),
            Check::SameAs(path) => values.get(path.as_str()).map(|other| other.trim()) == Some(v),
        }
    }

    /// What the field says when nothing better was written on the rule.
    pub fn default_message(&self) -> String {
        match self {
            Check::Required => "Required.".to_string(),
            Check::MinLen(n) => format!("At least {n} characters."),
            Check::MaxLen(n) => format!("At most {n} characters."),
            Check::Range(lo, hi) => format!("A number from {} to {}.", number(*lo), number(*hi)),
            Check::Pattern(mask) => format!("Shaped like {mask}."),
            Check::SameAs(path) => format!("Must match {path}."),
        }
    }
}

impl Rule {
    /// Read one written rule.
    ///
    /// `[moment:] check [arguments] [| message]`, so
    /// `blur: min_len 8 | Eight characters at least.`
    pub fn parse(src: &str) -> Result<Rule, String> {
        let (head, message) = match src.split_once('|') {
            Some((head, message)) => (head, Some(message.trim().to_string())),
            None => (src, None),
        };
        let head = head.trim();

        // The moment is only a moment when the word before the colon is one
        // of the three. A mask carries colons of its own — `##:##` for a
        // time — and splitting on the first colon regardless would silently
        // truncate the shape to `##`, matching nothing a person could type.
        let mut when = When::Blur;
        let mut rest = head;
        if let Some((lead, tail)) = head.split_once(':') {
            if let Some(parsed) = When::parse(lead.trim()) {
                when = parsed;
                rest = tail;
            }
        }
        let rest = rest.trim();
        let (kind, args) = match rest.split_once(char::is_whitespace) {
            Some((kind, args)) => (kind, args.trim()),
            None => (rest, ""),
        };

        let check = match kind {
            "required" => Check::Required,
            "min_len" => Check::MinLen(parse_len(args, kind)?),
            "max_len" => Check::MaxLen(parse_len(args, kind)?),
            "range" => {
                let (lo, hi) = args
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| "range wants two numbers".to_string())?;
                let lo = lo
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("range low \"{}\" is not a number", lo.trim()))?;
                let hi = hi
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("range high \"{}\" is not a number", hi.trim()))?;
                Check::Range(lo, hi)
            }
            "pattern" => {
                if args.is_empty() {
                    return Err("pattern wants a mask".to_string());
                }
                Check::Pattern(args.to_string())
            }
            "same_as" => {
                if args.is_empty() {
                    return Err("same_as wants the path of another field".to_string());
                }
                Check::SameAs(args.to_string())
            }
            "" => return Err("empty rule".to_string()),
            other => return Err(format!("unknown rule \"{other}\"")),
        };

        Ok(Rule {
            when,
            check,
            message: message.filter(|m| !m.is_empty()),
        })
    }

    pub fn message(&self) -> String {
        match &self.message {
            Some(message) => message.clone(),
            None => self.check.default_message(),
        }
    }
}

fn parse_len(args: &str, kind: &str) -> Result<usize, String> {
    args.parse::<usize>()
        .map_err(|_| format!("{kind} wants a whole number, not \"{args}\""))
}

/// The first rule of one field that is due by `moment` and fails, as the
/// message it wants to show.
pub fn first_failure(
    rules: &[Rule],
    moment: When,
    value: &str,
    values: &Values,
) -> Option<String> {
    rules
        .iter()
        .find(|rule| rule.when <= moment && !rule.check.passes(value, values))
        .map(|rule| rule.message())
}

/// Every field's value by path, for the rules that read another field.
pub fn values_of(fields: &[FieldState]) -> Values {
    fields
        .iter()
        .filter(|field| !field.bind.is_empty())
        .map(|field| (field.bind.clone(), field.value.clone()))
        .collect()
}

/// The first field that fails at `moment`, by its index and its message.
///
/// First in declaration order, which is the order a person reads down the
/// page — not the worst failure, and not the last one found. A refused
/// submit sends the keyboard here, so this being the top of the page and not
/// the bottom of it is the whole behaviour.
pub fn first_invalid(fields: &[FieldState], moment: When) -> Option<(usize, String)> {
    let values = values_of(fields);
    fields.iter().enumerate().find_map(|(index, field)| {
        first_failure(&field.rules, moment, &field.value, &values)
            .map(|message| (index, message))
    })
}

// ---------------------------------------------------------------------------
// The widgets.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum FormFieldAction {
    /// The keyboard left this field.
    ///
    /// The form does not need to be told which field it was — it re-reads
    /// them all — but it does need to be woken, and an action is what turns
    /// a focus change into an action pass the form can hear.
    Blurred,
    #[default]
    None,
}

#[derive(Clone, Debug, Default)]
pub enum FormAction {
    /// A field's value changed: its path, and what it now holds.
    Changed(String, String),
    /// Every rule passed and the form may be sent.
    Submitted,
    /// A submit was refused; the path of the field the keyboard went to.
    Rejected(String),
    /// Every field is back where it started.
    Reset,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct FormField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The field's own rect, drawn as nothing. It exists so the three slots
    /// have a turtle to sit in and the widget has an area to redraw.
    #[redraw]
    #[live]
    draw_bg: DrawQuad,

    /// The words above the input.
    #[find]
    #[live]
    pub label: WidgetRef,
    /// The input itself. Its `text()` is the field's value, whatever the
    /// widget is.
    #[find]
    #[live]
    pub input: WidgetRef,
    /// The line under the input, shown only while there is something to say.
    #[find]
    #[live]
    pub message: WidgetRef,

    /// The name this field answers to inside its form.
    #[live]
    pub bind: String,
    /// The label's text, pushed into the label slot each draw so a live edit
    /// of this property reaches the screen.
    #[live]
    pub title: String,
    /// The rules, one per line. See `Rule::parse`.
    #[live]
    pub rules: Vec<String>,

    #[rust]
    parsed: Vec<Rule>,
    /// What the field held when it was built, and what `reset` puts back.
    #[rust]
    initial: String,
    /// The value as of the last pass, so a change can be noticed.
    #[rust]
    last: String,
    /// Whether the keyboard has been in this field and left, or a submit has
    /// been refused. An untouched field keeps quiet.
    #[rust]
    touched: bool,
    /// The furthest moment this field has reached. A field that has blurred
    /// keeps being judged at blur even while somebody else is typing.
    #[rust(When::Change)]
    reached: When,
    #[rust]
    failure: Option<String>,
    #[rust]
    had_focus: bool,
}

impl ScriptHook for FormField {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        let lines = self.rules.clone();
        self.parsed.clear();
        for line in &lines {
            match Rule::parse(line) {
                Ok(rule) => self.parsed.push(rule),
                // A misspelt rule that silently passed would be worse than
                // no rule at all: the field would look guarded while letting
                // anything through.
                Err(why) => error!("form field {}: {} in \"{}\"", self.bind, why, line),
            }
        }
        // Whatever the input was given in the DSL is what this field starts
        // with and what `reset` puts back, so a caller fills the input
        // rather than saying the same value twice.
        self.initial = self.input.text();
        self.last = self.initial.clone();
        vm.with_cx_mut(|cx| {
            self.message.set_visible(cx, false);
        });
    }
}

impl FormField {
    /// What the field holds now.
    pub fn current(&self) -> String {
        self.input.text()
    }

    /// Whether the value has moved since the field was built or last reset.
    pub fn is_dirty(&self) -> bool {
        self.current() != self.initial
    }

    pub fn is_touched(&self) -> bool {
        self.touched
    }

    /// The area the keyboard actually lands on, which is the input inside
    /// the well rather than the well itself.
    fn focus_area(&self) -> Area {
        let well = self.input.as_field_well();
        if well.borrow().is_some() {
            return well.input().area();
        }
        self.input.area()
    }

    fn input_focused(&self, cx: &Cx) -> bool {
        let area = self.focus_area();
        // An empty area matches an empty key focus, which would report every
        // field as focused before the first draw.
        !area.is_empty() && cx.has_key_focus(area)
    }

    /// Hand the keyboard to the input, with the caret lit.
    pub fn focus(&self, cx: &mut Cx) {
        let well = self.input.as_field_well();
        if well.borrow().is_some() {
            well.focus_input(cx);
            return;
        }
        let input = self.input.as_text_input();
        if input.borrow().is_some() {
            input.take_key_focus(cx);
            return;
        }
        self.input.set_key_focus(cx);
    }

    /// Put a value in without treating it as something the person typed.
    fn put(&mut self, cx: &mut Cx, value: &str) {
        self.input.set_text(cx, value);
        self.last = value.to_string();
    }

    fn reset(&mut self, cx: &mut Cx) {
        let initial = self.initial.clone();
        self.put(cx, &initial);
        self.touched = false;
        self.reached = When::Change;
        self.failure = None;
    }

    /// The value if it moved since the last look.
    fn take_change(&mut self) -> Option<String> {
        let now = self.current();
        if now == self.last {
            return None;
        }
        self.last = now.clone();
        Some(now)
    }

    /// Re-judge the field and put what it has to say on the screen.
    fn refresh(&mut self, cx: &mut Cx, values: &Values) {
        let value = self.current();
        self.failure = if self.touched {
            first_failure(&self.parsed, self.reached, &value, values)
        } else {
            None
        };
        let text = self.failure.clone().unwrap_or_default();
        self.message.set_text(cx, &text);
        self.message.set_visible(cx, !text.is_empty());
    }
}

impl Widget for FormField {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.title.is_empty() {
            self.label.set_text(cx.cx.cx, &self.title);
        }
        // A Label with no text draws one blank line rather than nothing, so
        // a field with no title would carry a gap where its label would go.
        let titled = !self.label.text().trim().is_empty();
        self.label.set_visible(cx.cx.cx, titled);

        self.draw_bg.begin(cx, walk, self.layout);
        for (name, slot) in [
            (live_id!(label), &mut self.label),
            (live_id!(input), &mut self.input),
            (live_id!(message), &mut self.message),
        ] {
            // Nothing else puts the slots in the tree, so without this a
            // host could not reach ids!(field.input) at all.
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for slot in [&self.label, &self.input, &self.message] {
            slot.handle_event(cx, event, scope);
        }
        // The keyboard leaving is not a hit on anything this widget owns —
        // the focus is on the input inside the well — so the focus change
        // itself is what is watched.
        if let Event::KeyFocus(_) = event {
            let now = self.input_focused(cx);
            if self.had_focus && !now {
                self.touched = true;
                if self.reached < When::Blur {
                    self.reached = When::Blur;
                }
                cx.widget_action(self.uid, FormFieldAction::Blurred);
            }
            self.had_focus = now;
        }
    }

    /// The field's value, so a host reads the field the way it read the
    /// input.
    fn text(&self) -> String {
        self.current()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.put(cx, v);
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        for slot in [&self.label, &self.input, &self.message] {
            slot.set_disabled(cx, disabled);
        }
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.input.disabled(cx)
    }
}

#[derive(Script, Widget)]
pub struct Form {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    /// No field under the form answers while this is on, and a submit is
    /// refused without judging anything — the state a form is in while the
    /// last submit is still in flight.
    #[live]
    pub disabled: bool,
}

impl ScriptHook for Form {
    /// A `disabled: true` written in the DSL sets the field directly and
    /// never reaches `set_disabled`, so without this a form declared off
    /// would still take typing.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if self.disabled {
            vm.with_cx_mut(|cx| {
                for field in self.fields() {
                    field.set_disabled(cx, true);
                }
            });
        }
    }
}

impl Form {
    /// Every field under this form, in declaration order.
    fn fields(&self) -> Vec<WidgetRef> {
        let mut top = Vec::new();
        self.view.children(&mut |_id, child| top.push(child));
        let mut out = Vec::new();
        for child in top {
            Self::collect(&child, &mut out);
        }
        out
    }

    fn collect(node: &WidgetRef, out: &mut Vec<WidgetRef>) {
        if node.borrow::<FormField>().is_some() {
            out.push(node.clone());
            return;
        }
        // Gathered before recursing: the visit holds a borrow on the node,
        // and a field's own slots are widgets like any other.
        let mut kids = Vec::new();
        if !node.try_children(&mut |_id, child| kids.push(child)) {
            return;
        }
        for child in kids {
            Self::collect(&child, out);
        }
    }

    /// The fields as the engine sees them. One entry per widget in `fields`,
    /// at the same index, so an answer from `first_invalid` names a widget.
    fn states(fields: &[WidgetRef]) -> Vec<FieldState> {
        fields
            .iter()
            .map(|field| match field.borrow::<FormField>() {
                Some(inner) => FieldState {
                    bind: inner.bind.clone(),
                    rules: inner.parsed.clone(),
                    value: inner.current(),
                },
                None => FieldState::default(),
            })
            .collect()
    }

    /// Re-read every field, report what moved, and re-judge them all.
    ///
    /// Every field every pass rather than only the one that changed: a rule
    /// may read another field, so typing a password re-judges the field that
    /// has to match it, and that field is not the one anybody touched.
    fn sync(&mut self, cx: &mut Cx) {
        let fields = self.fields();
        let states = Self::states(&fields);
        let values = values_of(&states);

        let mut changed = Vec::new();
        for field in &fields {
            if let Some(mut inner) = field.borrow_mut::<FormField>() {
                if let Some(value) = inner.take_change() {
                    changed.push((inner.bind.clone(), value));
                }
            }
        }
        for field in &fields {
            if let Some(mut inner) = field.borrow_mut::<FormField>() {
                inner.refresh(cx, &values);
            }
        }
        let uid = self.widget_uid();
        for (path, value) in changed {
            cx.widget_action(uid, FormAction::Changed(path, value));
        }
    }

    /// Whether every rule would pass if the form were sent now.
    pub fn is_valid(&self) -> bool {
        let fields = self.fields();
        first_invalid(&Self::states(&fields), When::Submit).is_none()
    }

    /// Whether any field has moved since it was built or last reset.
    pub fn is_dirty(&self) -> bool {
        self.fields().iter().any(|field| {
            field
                .borrow::<FormField>()
                .map(|inner| inner.is_dirty())
                .unwrap_or(false)
        })
    }

    pub fn value(&self, path: &str) -> String {
        self.fields()
            .iter()
            .find_map(|field| {
                let inner = field.borrow::<FormField>()?;
                (inner.bind == path).then(|| inner.current())
            })
            .unwrap_or_default()
    }

    pub fn set_value(&mut self, cx: &mut Cx, path: &str, value: &str) {
        let fields = self.fields();
        for field in &fields {
            let is_it = field
                .borrow::<FormField>()
                .map(|inner| inner.bind == path)
                .unwrap_or(false);
            if is_it {
                if let Some(mut inner) = field.borrow_mut::<FormField>() {
                    inner.put(cx, value);
                }
            }
        }
        self.sync(cx);
    }

    /// Send the form, or refuse it and put the keyboard in the first field
    /// that is wrong.
    pub fn submit(&mut self, cx: &mut Cx) -> bool {
        if self.disabled {
            return false;
        }
        let fields = self.fields();
        // A submit is what makes every field speak, including the ones
        // nobody visited: a person who tabbed straight to the button must
        // still be told what is missing.
        for field in &fields {
            if let Some(mut inner) = field.borrow_mut::<FormField>() {
                inner.touched = true;
                inner.reached = When::Submit;
            }
        }
        let states = Self::states(&fields);
        let values = values_of(&states);
        for field in &fields {
            if let Some(mut inner) = field.borrow_mut::<FormField>() {
                inner.refresh(cx, &values);
            }
        }
        let uid = self.widget_uid();
        match first_invalid(&states, When::Submit) {
            Some((index, _)) => {
                if let Some(field) = fields.get(index) {
                    if let Some(inner) = field.borrow::<FormField>() {
                        inner.focus(cx);
                    }
                }
                let path = states
                    .get(index)
                    .map(|state| state.bind.clone())
                    .unwrap_or_default();
                cx.widget_action(uid, FormAction::Rejected(path));
                false
            }
            None => {
                cx.widget_action(uid, FormAction::Submitted);
                true
            }
        }
    }

    /// Every field back to what it held when it was built, and quiet again.
    pub fn reset(&mut self, cx: &mut Cx) {
        let fields = self.fields();
        for field in &fields {
            if let Some(mut inner) = field.borrow_mut::<FormField>() {
                inner.reset(cx);
            }
        }
        self.sync(cx);
        cx.widget_action(self.widget_uid(), FormAction::Reset);
    }
}

impl Widget for Form {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        // Every action pass rather than only the ones carrying a field's
        // own action: a text input reports its change one pass before the
        // field could pass it on, and re-reading is cheaper than the extra
        // hop plus the bugs of a message that refreshes one pass late.
        if let Event::Actions(_) = event {
            self.sync(cx);
        }
    }

    /// One flag reaches every field, rather than each caller remembering to
    /// switch off five inputs and forgetting the fifth.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled == disabled {
            return;
        }
        self.disabled = disabled;
        for field in self.fields() {
            field.set_disabled(cx, disabled);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(submit) {
            let sent = vm.with_cx_mut(|cx| self.submit(cx));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(sent));
        }
        if method == live_id!(reset) {
            vm.with_cx_mut(|cx| self.reset(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(is_valid) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.is_valid()));
        }
        if method == live_id!(value) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(path) = vm
                        .bx
                        .heap
                        .cast_to_owned_string(value, "reading a form field path")
                    {
                        let held = self.value(&path);
                        let out = vm.bx.heap.new_string_from_str(&held);
                        return ScriptAsyncResult::Return(out.into());
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(set_value) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let path = vm.bx.heap.vec_value(args_obj, 0, trap);
                let value = vm.bx.heap.vec_value(args_obj, 1, trap);
                if !path.is_err() && !value.is_err() {
                    let path = vm
                        .bx
                        .heap
                        .cast_to_owned_string(path, "reading a form field path");
                    let value = vm
                        .bx
                        .heap
                        .cast_to_owned_string(value, "reading a form field value");
                    if let (Some(path), Some(value)) = (path, value) {
                        vm.with_cx_mut(|cx| self.set_value(cx, &path, &value));
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }
}

impl FormFieldRef {
    pub fn value(&self) -> String {
        self.borrow().map(|inner| inner.current()).unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        self.borrow().map(|inner| inner.is_dirty()).unwrap_or(false)
    }

    pub fn is_touched(&self) -> bool {
        self.borrow().map(|inner| inner.is_touched()).unwrap_or(false)
    }

    /// What the field is saying, or nothing.
    pub fn message(&self) -> Option<String> {
        self.borrow().and_then(|inner| inner.failure.clone())
    }

    pub fn focus(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow() {
            inner.focus(cx);
        }
    }

    /// The keyboard left this field.
    pub fn blurred(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), FormFieldAction::Blurred))
            .unwrap_or(false)
    }
}

impl FormRef {
    pub fn is_valid(&self) -> bool {
        self.borrow().map(|inner| inner.is_valid()).unwrap_or(false)
    }

    pub fn is_dirty(&self) -> bool {
        self.borrow().map(|inner| inner.is_dirty()).unwrap_or(false)
    }

    pub fn value(&self, path: &str) -> String {
        self.borrow()
            .map(|inner| inner.value(path))
            .unwrap_or_default()
    }

    pub fn set_value(&self, cx: &mut Cx, path: &str, value: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, path, value);
        }
    }

    pub fn submit(&self, cx: &mut Cx) -> bool {
        match self.borrow_mut() {
            Some(mut inner) => inner.submit(cx),
            None => false,
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    /// The form was sent.
    pub fn submitted(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), FormAction::Submitted))
            .unwrap_or(false)
    }

    /// A submit was refused; the path of the field the keyboard went to.
    pub fn rejected(&self, actions: &Actions) -> Option<String> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            FormAction::Rejected(path) => Some(path),
            _ => None,
        }
    }

    /// A field's new value, as it is typed.
    pub fn changed(&self, actions: &Actions) -> Option<(String, String)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            FormAction::Changed(path, value) => Some((path, value)),
            _ => None,
        }
    }

    pub fn was_reset(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), FormAction::Reset))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(lines: &[&str]) -> Vec<Rule> {
        lines
            .iter()
            .map(|line| Rule::parse(line).expect("rule parses"))
            .collect()
    }

    fn field(bind: &str, lines: &[&str], value: &str) -> FieldState {
        FieldState {
            bind: bind.to_string(),
            rules: rules(lines),
            value: value.to_string(),
        }
    }

    fn judge(lines: &[&str], value: &str) -> Option<String> {
        first_failure(&rules(lines), When::Submit, value, &Values::new())
    }

    #[test]
    fn required_is_the_only_rule_that_minds_an_empty_field() {
        assert_eq!(judge(&["required"], "  "), Some("Required.".to_string()));
        assert_eq!(judge(&["required"], "x"), None);
        // An empty optional field passes everything else: a length, a range
        // or a shape that also complained here would make every optional
        // field on the form compulsory by accident.
        assert_eq!(judge(&["min_len 4"], ""), None);
        assert_eq!(judge(&["range 1 10"], ""), None);
        assert_eq!(judge(&["pattern ###"], "   "), None);
    }

    #[test]
    fn lengths_count_characters_not_bytes() {
        // Five characters, six bytes.
        assert_eq!(judge(&["min_len 4"], "héllo"), None);
        assert_eq!(judge(&["max_len 4"], "héllo"), Some("At most 4 characters.".to_string()));
        assert_eq!(judge(&["min_len 6"], "héllo"), Some("At least 6 characters.".to_string()));
    }

    #[test]
    fn a_range_refuses_what_is_not_a_number() {
        assert_eq!(judge(&["range 18 120"], "42"), None);
        assert_eq!(judge(&["range 18 120"], " 18 "), None);
        assert_eq!(
            judge(&["range 18 120"], "17"),
            Some("A number from 18 to 120.".to_string())
        );
        assert_eq!(
            judge(&["range 18 120"], "old"),
            Some("A number from 18 to 120.".to_string())
        );
    }

    #[test]
    fn a_mask_is_a_shape_and_must_be_the_whole_value() {
        assert!(matches_mask("12-345", "##-###"));
        assert!(!matches_mask("1a-345", "##-###"));
        // Too long and too short both fail: a mask that only checked a
        // prefix would pass anything that started right.
        assert!(!matches_mask("12-3456", "##-###"));
        assert!(!matches_mask("12-34", "##-###"));
        assert!(matches_mask("AB9", "@@*"));
        // A backslash makes the next mark literal, so a mask can ask for a
        // real hash.
        assert!(matches_mask("#7", "\\##"));
        assert!(!matches_mask("47", "\\##"));
        // And a mask ending in a lone backslash asked for a character that
        // was never written.
        assert!(!matches_mask("4", "#\\"));
    }

    #[test]
    fn a_rule_can_read_another_field() {
        let fields = [
            field("pass", &["required"], "opensesame"),
            field("again", &["same_as pass"], "opensesame"),
        ];
        assert_eq!(first_invalid(&fields, When::Submit), None);

        let fields = [
            field("pass", &["required"], "opensesame"),
            field("again", &["same_as pass"], "opensesami"),
        ];
        assert_eq!(
            first_invalid(&fields, When::Submit),
            Some((1, "Must match pass.".to_string()))
        );
    }

    #[test]
    fn a_dependent_rule_that_names_nothing_fails_rather_than_passing() {
        let fields = [field("again", &["same_as nowhere"], "typed")];
        assert!(first_invalid(&fields, When::Submit).is_some());
    }

    #[test]
    fn the_moments_are_a_ladder() {
        let rules = rules(&["change: max_len 3", "submit: required"]);
        let values = Values::new();
        // The change rule is still due at blur and at submit.
        assert!(first_failure(&rules, When::Change, "abcd", &values).is_some());
        assert!(first_failure(&rules, When::Blur, "abcd", &values).is_some());
        assert!(first_failure(&rules, When::Submit, "abcd", &values).is_some());
        // The submit rule is not due before then.
        assert_eq!(first_failure(&rules, When::Change, "", &values), None);
        assert_eq!(first_failure(&rules, When::Blur, "", &values), None);
        assert!(first_failure(&rules, When::Submit, "", &values).is_some());
    }

    #[test]
    fn a_rule_with_no_moment_written_on_it_speaks_at_blur() {
        assert_eq!(Rule::parse("required").unwrap().when, When::Blur);
        assert_eq!(Rule::parse("change: required").unwrap().when, When::Change);
        assert_eq!(Rule::parse("submit: required").unwrap().when, When::Submit);
    }

    #[test]
    fn a_written_message_replaces_the_default_wording() {
        let rule = Rule::parse("blur: min_len 8 | Eight at least.").unwrap();
        assert_eq!(rule.when, When::Blur);
        assert_eq!(rule.check, Check::MinLen(8));
        assert_eq!(rule.message(), "Eight at least.");
        assert_eq!(Rule::parse("min_len 8").unwrap().message(), "At least 8 characters.");
    }

    #[test]
    fn a_mask_may_carry_a_colon_without_becoming_a_moment() {
        // Splitting on the first colon regardless would leave the mask as
        // `##`, which no time of day matches.
        let rule = Rule::parse("blur: pattern ##:##").unwrap();
        assert_eq!(rule.when, When::Blur);
        assert_eq!(rule.check, Check::Pattern("##:##".to_string()));
        assert!(rule.check.passes("09:30", &Values::new()));

        let rule = Rule::parse("pattern ##:##").unwrap();
        assert_eq!(rule.check, Check::Pattern("##:##".to_string()));
    }

    #[test]
    fn a_rule_nobody_can_read_is_an_error_and_not_a_silent_pass() {
        assert!(Rule::parse("requird").is_err());
        assert!(Rule::parse("min_len").is_err());
        assert!(Rule::parse("min_len two").is_err());
        assert!(Rule::parse("range 1").is_err());
        assert!(Rule::parse("pattern").is_err());
        assert!(Rule::parse("same_as").is_err());
        assert!(Rule::parse("").is_err());
    }

    #[test]
    fn a_refused_submit_names_the_first_field_down_the_page() {
        let fields = [
            field("name", &["required"], "Someone"),
            field("age", &["range 18 120"], "9"),
            field("code", &["required"], ""),
        ];
        // Not the emptiest, not the last: the first one a person reading
        // down the page would reach.
        assert_eq!(
            first_invalid(&fields, When::Submit),
            Some((1, "A number from 18 to 120.".to_string()))
        );
    }

    #[test]
    fn the_form_is_the_and_of_its_fields() {
        let ok = [
            field("name", &["required"], "Someone"),
            field("age", &["range 18 120"], "30"),
        ];
        assert!(first_invalid(&ok, When::Submit).is_none());

        let one_bad = [
            field("name", &["required"], "Someone"),
            field("age", &["range 18 120"], "300"),
        ];
        assert!(first_invalid(&one_bad, When::Submit).is_some());
    }

    #[test]
    fn a_field_with_no_bind_is_left_out_of_what_the_rules_can_read() {
        let fields = [
            field("", &["required"], "loose"),
            field("pass", &[], "secret"),
        ];
        let values = values_of(&fields);
        assert_eq!(values.len(), 1);
        assert_eq!(values.get("pass").map(String::as_str), Some("secret"));
    }
}
