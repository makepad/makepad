//! TagField — the box holding what has already been chosen, and the place to
//! type the next one.
//!
//! A tag field is a text field whose answer is a LIST, and the whole of the
//! design sits at one moment: when a word stops being typing and becomes an
//! entry. Here that moment is Return, or one of `split_chars` arriving in the
//! text — a comma by default. Typed and pasted text go through the same rule,
//! so a list off the clipboard lands as a list of tags rather than as one
//! very long tag.
//!
//! **A refusal is spoken.** A tag the field already holds is not quietly
//! dropped: the field says which tag it already has, on a line under the box,
//! and the box's border takes the warning colour until the next keystroke.
//! The same for a field that is full. Silently ignoring what someone typed
//! leaves them believing it went in, and they only find out later, from
//! whatever was supposed to have been filtered or addressed or labelled.
//!
//! **Backspace on an empty input takes the last tag back**, which is the one
//! gesture people try first and the reason a tag field feels like a field
//! rather than a list with a box on the end. It is caught before the input
//! sees the key, because an empty input handles Backspace itself — as
//! nothing — and so never reports it as unhandled.
//!
//! **What it deliberately does not do.** It does not suggest, complete or
//! know what tags exist anywhere: a field that offers a list to choose from
//! is a combo box over data the host owns, with a popup and a different
//! keyboard, and it is a different widget. It does not judge the SHAPE of a
//! tag — that an address is an address, that a word is in some vocabulary —
//! because only the host knows what a tag means here. It enforces its own two
//! rules, `max` and no duplicates, and reports everything else for the host
//! to accept or undo.
//!
//! The tags are drawn as [`crate::chip::Chip`]s from the `chip` template on
//! the instance, so a caller keeps its own look and still gets the cross, its
//! separate hit area and the removal report for free.
use crate::{
    chip::ChipWidgetRefExt,
    link_label::LinkLabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::{text::selection::Cursor, *},
    text_input::TextInputWidgetRefExt,
    widget::*,
    widget_async::ScriptAsyncResult,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

/// What became of a tag the field was offered.
///
/// `Duplicate` carries the tag ALREADY in the list rather than the one that
/// was offered, because the message exists to point at a chip that is on
/// screen, and under a case-insensitive rule the two are spelt differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagOffer {
    Added,
    Duplicate(String),
    /// The field is full at this many.
    Full(usize),
    /// Nothing but spaces: not a refusal, and nothing to say about it.
    Blank,
}

/// The tags a run of typed or pasted text makes, and what is left over.
///
/// The leftover is everything after the last separator, and it is returned
/// untrimmed because it is still being typed — trimming it would eat the
/// space someone just pressed and put the caret back a character.
pub fn split_tags(text: &str, split_chars: &str) -> (Vec<String>, String) {
    let mut tags = Vec::new();
    let mut rest = String::new();
    for ch in text.chars() {
        if split_chars.contains(ch) {
            let piece = rest.trim();
            if !piece.is_empty() {
                tags.push(piece.to_string());
            }
            rest.clear();
        } else {
            rest.push(ch);
        }
    }
    (tags, rest)
}

/// Whether two tags are the same tag.
fn same_tag(a: &str, b: &str, match_case: bool) -> bool {
    if match_case {
        a == b
    } else {
        // Not `eq_ignore_ascii_case`: a field of names or of words in any
        // other language would then hold "Café" and "café" as two tags while
        // holding "Urgent" and "urgent" as one.
        a.to_lowercase() == b.to_lowercase()
    }
}

/// Offer `offered` to `tags`, and say what happened to it.
///
/// The duplicate rule is tested BEFORE the cap: a full field that is being
/// handed something it already has should say the useful thing, not complain
/// about its size.
pub fn offer_tag(tags: &mut Vec<String>, offered: &str, max: usize, match_case: bool) -> TagOffer {
    let tag = offered.trim();
    if tag.is_empty() {
        return TagOffer::Blank;
    }
    if let Some(existing) = tags.iter().find(|held| same_tag(held, tag, match_case)) {
        return TagOffer::Duplicate(existing.clone());
    }
    if max > 0 && tags.len() >= max {
        return TagOffer::Full(max);
    }
    tags.push(tag.to_string());
    TagOffer::Added
}

/// The line the field shows about a refusal, or nothing when there is
/// nothing to say.
fn refusal_note(offer: &TagOffer) -> Option<String> {
    match offer {
        TagOffer::Duplicate(existing) => {
            Some(format!("\u{201c}{existing}\u{201d} is already there"))
        }
        TagOffer::Full(1) => Some("this field takes one tag".to_string()),
        TagOffer::Full(max) => Some(format!("this field takes {max} tags")),
        TagOffer::Added | TagOffer::Blank => None,
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawTagFieldBase = #(DrawTagField::script_component(vm))
    set_type_default() do #(DrawTagField::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size
                self.border_size
                self.rect_size.x - self.border_size * 2.
                self.rect_size.y - self.border_size * 2.
                self.border_radius
            )
            sdf.fill_keep(
                self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_disabled, self.disabled)
            )
            // The alarm is mixed after the focus and before the disabled
            // state: a refusal has to be visible in the field someone is
            // typing in, and invisible in one that answers nothing.
            sdf.stroke(
                self.border_color
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_alarm, self.alarm)
                    .mix(self.border_color_disabled, self.disabled)
                self.border_size
            )
            return sdf.result
        }
    }

    mod.widgets.TagFieldBase = #(TagField::register_widget(vm))

    /** A box of chosen tags with a place to type the next one. Return and
     * the `split_chars` commit what is typed, Backspace on the empty input
     * takes the last tag back, and a duplicate is refused out loud. */
    mod.widgets.TagField = set_type_default() do mod.widgets.TagFieldBase{
        width: Fill
        height: Fit
        // The rows wrap: a field of eight tags is four lines tall, not one
        // line with six of them off the side.
        flow: Right{wrap: true}
        spacing: theme.space_1
        wrap_spacing: theme.space_1
        align: Align{y: 0.5}
        padding: theme.mspace_1{left: theme.space_2, right: theme.space_2}

        /** the tags the field starts with */
        tags: []
        /** the characters that end a tag as they are typed or pasted */
        split_chars: ","
        /** how many tags the field takes; 0 is no limit 0..40 step 1 */
        max: 0
        /** two tags differing only in case are two tags 0..1 step 1 */
        match_case: false
        /** offer a control under the box that drops the lot 0..1 step 1 */
        show_clear: true
        /** nothing in the field answers 0..1 step 1 */
        disabled: false
        /** the room between the box and the line under it 0..24 step 1 */
        note_gap: theme.space_1

        /* Every value here is plain, and every one has a field on the draw
         * struct behind it. No uniform(): a caller that dresses the field in
         * its own palette overrides these, and overriding a uniform on a draw
         * type that also carries instance fields regenerates the shader's
         * value table out from under them - the box then reads a border
         * thickness of garbage and draws as one flat slab. */
        draw_bg +: {
            hover: 0.0
            focus: 0.0
            disabled: 0.0
            alarm: 0.0

            /** bevel border thickness in pixels 0..4 step 0.5 */
            border_size: theme.beveling
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: theme.color_inset
            color_hover: theme.color_inset_hover
            color_focus: theme.color_inset_focus
            color_disabled: theme.color_inset_disabled

            border_color: theme.color_bevel
            border_color_hover: theme.color_bevel_hover
            border_color_focus: theme.color_bevel_focus
            border_color_alarm: theme.color_warning
            border_color_disabled: theme.color_bevel_disabled
        }

        draw_note +: {
            color: theme.color_warning
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        // Bare slots, not named instances: a slot takes a value, so this is
        // `input: WellInput{}` and never `input := WellInput{}`, which would
        // leave the slot empty and the box with nothing to type in.
        //
        // The input's Fill takes the rest of the row it is on, and its `min`
        // is what drops it onto a row of its own once the tags have left it
        // too little to type in.
        input: mod.widgets.WellInput{
            width: Fill{min: 120.}
            height: Fit
            empty_text: "Add a tag"
        }
        // The control that drops the lot. It is looked for by its click and
        // nothing else, so a host may put its own widget here.
        clear: mod.widgets.LinkLabel{text: "Clear all"}

        // A named entry, the way a list declares its item template: it lands
        // in the vec and is collected rather than drawn as a child.
        chip := mod.widgets.Chip{
            size: Small
            removable: true
        }
    }
}

/// The box: an inset rounded rect, its bevel, and the four states the field
/// paints it in.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTagField {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    /// Lit while the field is showing a refusal.
    #[live]
    alarm: f32,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_focus: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_hover: Vec4f,
    #[live]
    border_color_focus: Vec4f,
    #[live]
    border_color_alarm: Vec4f,
    #[live]
    border_color_disabled: Vec4f,
}

/// What a tag field reports. `Changed` carries the whole list after every
/// move, so a host that only wants the answer needs to read nothing else;
/// `Added`, `Removed` and `Refused` say what happened, for a host that wants
/// to undo it or say something about it.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TagFieldAction {
    Changed(Vec<String>),
    Added(String),
    Removed(String),
    /// A tag was turned away, with the line the field is showing about it.
    Refused(String),
    /// The clear control dropped the lot.
    Cleared,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct TagField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    pub draw_bg: DrawTagField,
    /// The line under the box that says why something was turned away.
    #[live]
    pub draw_note: DrawText,

    /// The tags, in the order they went in.
    #[live]
    pub tags: Vec<String>,
    /// The characters that end a tag as they are typed or pasted. A host
    /// pasting lists off the clipboard wants a newline in here as well.
    #[live(",".to_string())]
    pub split_chars: String,
    /// How many tags the field takes; 0 is no limit.
    #[live]
    pub max: usize,
    /// Whether two tags differing only in case are two tags.
    #[live]
    pub match_case: bool,
    /// Whether the control that drops the lot is offered at all. A field of
    /// one or two tags does not need one: every chip already carries a cross.
    #[live(true)]
    pub show_clear: bool,
    #[live]
    pub disabled: bool,
    /// The room between the box and the line under it.
    #[live(4.0)]
    pub note_gap: f64,

    /// Where the next tag is typed.
    #[find]
    #[live]
    pub input: WidgetRef,
    /// The control that drops the lot, drawn only while there is a lot.
    #[find]
    #[live]
    pub clear: WidgetRef,

    /// The chip template, collected from the instance by name.
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    /// One chip per tag, kept as a pool. Chips past `tags.len()` are neither
    /// drawn nor handed events, so a stale rect cannot answer a press.
    #[rust]
    chips: Vec<WidgetRef>,
    /// What the field last turned away, in the words it is showing.
    #[rust]
    note: Option<String>,
    #[rust]
    hovered: bool,
    #[rust]
    warned: bool,
}

impl ScriptHook for TagField {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // The chip template arrives as a named entry on the instance, the way
        // a list's item templates do.
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }
        if apply.is_reload() {
            // The chips come from a template that may have just changed, so
            // they are rebuilt rather than patched.
            self.chips.clear();
            self.warned = false;
        }
    }

    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.settle();
    }

    fn on_after_reload(&mut self, _vm: &mut ScriptVm) {
        self.settle();
    }
}

impl TagField {
    /// Put a list written in markup through the same rules as one typed in,
    /// so a declared field cannot start out holding a duplicate or more tags
    /// than its own `max` allows.
    fn settle(&mut self) {
        let declared = std::mem::take(&mut self.tags);
        for tag in declared {
            offer_tag(&mut self.tags, &tag, self.max, self.match_case);
        }
    }

    /// The chip standing for tag `index`, made from the template on first
    /// use. The pool is only ever appended to, so a chip keeps its place.
    fn tag_chip(&mut self, cx: &mut Cx, index: usize) -> Option<WidgetRef> {
        if let Some(chip) = self.chips.get(index) {
            return Some(chip.clone());
        }
        // Taken as a value in its own statement: the template's borrow of
        // `self` has to be over before the miss below can write to `self`.
        let template: Option<ScriptValue> = self
            .templates
            .get(&live_id!(chip))
            .map(|template| template.as_object().into());
        let Some(value) = template else {
            if !self.warned {
                self.warned = true;
                warning!("TagField has no `chip` template, so its tags cannot be drawn");
            }
            return None;
        };
        let chip = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        // A tree node under the field, so a test and the design overlay can
        // reach a tag by name rather than by hunting for a rect.
        cx.widget_tree_insert_child(
            self.uid,
            LiveId::from_str_num("tag", index as u64),
            chip.clone(),
        );
        self.chips.push(chip.clone());
        Some(chip)
    }

    fn report_change(&mut self, cx: &mut Cx) {
        let uid = self.uid;
        let tags = self.tags.clone();
        cx.widget_action(uid, TagFieldAction::Changed(tags));
        self.redraw(cx);
    }

    /// Offer one tag and answer what happened, saying so on the line under
    /// the box when it was turned away.
    pub fn add(&mut self, cx: &mut Cx, tag: &str) -> TagOffer {
        let uid = self.uid;
        let offer = offer_tag(&mut self.tags, tag, self.max, self.match_case);
        match &offer {
            TagOffer::Added => {
                let added = self.tags.last().cloned().unwrap_or_default();
                cx.widget_action(uid, TagFieldAction::Added(added));
                self.report_change(cx);
            }
            TagOffer::Blank => {}
            refused => {
                if let Some(note) = refusal_note(refused) {
                    cx.widget_action(uid, TagFieldAction::Refused(note.clone()));
                    self.note = Some(note);
                    self.redraw(cx);
                }
            }
        }
        offer
    }

    /// Take what the input is holding and commit whatever of it is finished.
    ///
    /// `whole` is Return: everything left over counts as a tag too. Without
    /// it only the pieces before a separator are committed and the rest stays
    /// where it is, because it is still being typed.
    fn take_from_input(&mut self, cx: &mut Cx, text: &str, whole: bool) {
        // Every attempt replaces the last one's message: a refusal is about
        // the tag just offered, and leaving an older one up makes the field
        // say something that is no longer true.
        if self.note.take().is_some() {
            self.redraw(cx);
        }
        let (pieces, rest) = split_tags(text, &self.split_chars);
        if pieces.is_empty() && !whole {
            return;
        }
        for piece in pieces {
            self.add(cx, &piece);
        }
        if whole {
            self.add(cx, &rest);
        }
        // A refused tag leaves the input rather than sitting in it: the field
        // has said what is wrong with it, and someone typing a list wants the
        // box ready for the next one, not holding a word they must now
        // delete by hand.
        let left = if whole { String::new() } else { rest };
        self.input.set_text(cx, &left);
        // `set_text` only floors the caret to a grapheme boundary of the new
        // text, so without this it can land in the middle of what is left.
        self.input.as_text_input().set_cursor(
            cx,
            Cursor { index: left.len(), prefer_next_row: false },
            false,
        );
    }

    fn remove_at(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.tags.len() {
            return;
        }
        let uid = self.uid;
        let gone = self.tags.remove(index);
        if self.note.take().is_some() {
            self.redraw(cx);
        }
        cx.widget_action(uid, TagFieldAction::Removed(gone));
        self.report_change(cx);
    }

    /// Drop the lot. Reported as one thing rather than as a removal each: a
    /// host undoing this is putting back a list, not five separate tags.
    pub fn clear_all(&mut self, cx: &mut Cx) {
        if self.tags.is_empty() {
            return;
        }
        let uid = self.uid;
        self.tags.clear();
        self.note = None;
        cx.widget_action(uid, TagFieldAction::Cleared);
        self.report_change(cx);
    }

    /// Hand the keyboard to the input. The box is the target a person aims
    /// at, so a press on its padding or on the gap after the last chip
    /// belongs to the input inside it.
    pub fn focus_input(&self, cx: &mut Cx) {
        let input = self.input.as_text_input();
        if input.borrow().is_some() {
            input.take_key_focus(cx);
        } else {
            self.input.set_key_focus(cx);
        }
    }

    /// The tags, whatever they are now.
    pub fn tags(&self) -> Vec<String> {
        self.tags.clone()
    }

    /// Replace the list, settled by the field's own rules. It reports
    /// nothing: a host that has just written the answer does not need to be
    /// told what the answer is, and a `Changed` here is how feedback loops
    /// start.
    pub fn set_tags(&mut self, cx: &mut Cx, tags: Vec<String>) {
        self.tags = tags;
        self.settle();
        self.note = None;
        self.redraw(cx);
    }
}

impl Widget for TagField {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(add) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(tag) = vm.bx.heap.cast_to_owned_string(value, "adding a tag") {
                        vm.with_cx_mut(|cx| {
                            self.add(cx, &tag);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(clear) {
            vm.with_cx_mut(|cx| self.clear_all(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Asked every pass rather than tracked: the focus can leave for
        // reasons this field never hears about, and one that only listened
        // would keep claiming a keyboard it no longer has.
        let focused = cx.cx.cx.has_key_focus(self.input.area());
        self.draw_bg.focus = if focused { 1.0 } else { 0.0 };
        self.draw_bg.hover = if self.hovered && !self.disabled { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };
        self.draw_bg.alarm = if self.note.is_some() && !self.disabled { 1.0 } else { 0.0 };

        // A Fit field is a field somebody wants to shrink-wrap; its box must
        // not ask to Fill inside it, or the box resolves to nothing and the
        // whole widget is laid out and never painted.
        let row_width = match walk.width {
            Size::Fit { .. } => Size::fit(),
            _ => Size::fill(),
        };
        // The box and the line under it are stacked in a turtle of their own.
        // The message belongs OUTSIDE the border: it is about the field, not
        // about a tag, and inside the box it would read as an entry.
        cx.begin_turtle(
            walk,
            Layout {
                flow: Flow::Down,
                spacing: self.note_gap,
                ..Layout::default()
            },
        );

        self.draw_bg.begin(
            cx,
            Walk { width: row_width, height: Size::fit(), ..Walk::default() },
            self.layout,
        );
        for index in 0..self.tags.len() {
            let Some(chip) = self.tag_chip(cx.cx.cx, index) else {
                break;
            };
            // Written every draw: the pool outlives any one tag, so chip 2
            // is whatever tag is second now, not the one it was made for.
            let tag = self.tags[index].clone();
            chip.set_text(cx.cx.cx, &tag);
            let chip_walk = chip.walk(cx.cx.cx);
            let _ = chip.draw_walk(cx, scope, chip_walk);
        }
        // Drawn here rather than by a container, so that a host can reach
        // ids!(field.input) at all.
        cx.widget_tree_insert_child(self.uid, live_id!(input), self.input.clone());
        let input_walk = self.input.walk(cx.cx.cx);
        let _ = self.input.draw_walk(cx, scope, input_walk);
        self.draw_bg.end(cx);

        let shows_clear = self.show_clear && !self.tags.is_empty() && !self.disabled;
        if shows_clear || self.note.is_some() {
            cx.begin_turtle(
                Walk { width: row_width, height: Size::fit(), ..Walk::default() },
                Layout {
                    flow: Flow::right(),
                    spacing: self.layout.spacing,
                    align: Align { x: 0.0, y: 0.5 },
                    ..Layout::default()
                },
            );
            if shows_clear {
                cx.widget_tree_insert_child(self.uid, live_id!(clear), self.clear.clone());
                let clear_walk = self.clear.walk(cx.cx.cx);
                let _ = self.clear.draw_walk(cx, scope, clear_walk);
            }
            if let Some(note) = self.note.clone() {
                self.draw_note.draw_walk(
                    cx,
                    Walk { width: Size::fit(), height: Size::fit(), ..Walk::default() },
                    Align::default(),
                    &note,
                );
            }
            cx.end_turtle();
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.disabled {
            // Before the input sees the key. An empty input handles Backspace
            // itself — as nothing at all — so it never reports one as
            // unhandled and there is no later moment to catch this at.
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Backspace
                    && cx.has_key_focus(self.input.area())
                    && self.input.text().is_empty()
                    && !self.tags.is_empty()
                {
                    let last = self.tags.len() - 1;
                    self.remove_at(cx, last);
                }
            }
        }

        let shown = self.tags.len().min(self.chips.len());
        let chips: Vec<WidgetRef> = self.chips[..shown].to_vec();
        if !self.disabled {
            for chip in &chips {
                chip.handle_event(cx, event, scope);
            }
            self.input.handle_event(cx, event, scope);
            // Only while it is on screen: a slot that was not drawn this pass
            // still has last pass's rect, and would answer a press in a place
            // where nothing is.
            if self.show_clear && !self.tags.is_empty() {
                self.clear.handle_event(cx, event, scope);
            }
        }

        if let Event::Actions(actions) = event {
            // One press takes one chip away; the loop stops at the first so a
            // second chip cannot be caught by the shift the first one causes.
            for (index, chip) in chips.iter().enumerate() {
                if chip.as_chip().removed(actions) {
                    self.remove_at(cx, index);
                    break;
                }
            }
            let input = self.input.as_text_input();
            if let Some(text) = input.changed(actions) {
                self.take_from_input(cx, &text, false);
            }
            if let Some((text, _mods)) = input.returned(actions) {
                let had = self.tags.len();
                self.take_from_input(cx, &text, true);
                // A single-line input drops the key focus on Return. Taken
                // back only when the Return actually committed something:
                // a Return on an empty field is the one Return that is
                // plainly not about the tags, and stealing the focus back
                // from it would leave no way out of the field by keyboard.
                if self.tags.len() != had {
                    self.focus_input(cx);
                }
            }
            if self.show_clear
                && !self.tags.is_empty()
                && self.clear.as_link_label().clicked(actions)
            {
                self.clear_all(cx);
            }
        }

        if self.disabled {
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                if !self.hovered {
                    self.hovered = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered {
                    self.hovered = false;
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(_) => {
                // The padding, the gap after the last chip and the empty end
                // of a wrapped row all belong to the input: aiming at the box
                // and getting nothing is the commonest way a field feels
                // broken.
                self.focus_input(cx);
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if !fe.cancelled => {
                // Not redundant with the press. A focused TextInput drops its
                // own focus on a mouse-up outside its rect — right for a bare
                // field, wrong for one in a box, because the box's padding is
                // outside that rect and is still the field's target.
                self.focus_input(cx);
            }
            _ => {}
        }
    }

    /// The tags in one line, so a test can read the answer without walking
    /// chips.
    fn text(&self) -> String {
        self.tags.join(", ")
    }

    /// Setting the text sets the tags, split by the field's own rules, so a
    /// host can seed a field the way it seeds any other.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let (mut tags, rest) = split_tags(v, &self.split_chars);
        if !rest.trim().is_empty() {
            tags.push(rest.trim().to_string());
        }
        self.set_tags(cx, tags);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.tags.join(", "))
    }

    /// One `disabled` reaches everything inside, rather than each caller
    /// remembering the box, the input, the clear control and every chip.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            for slot in [&self.input, &self.clear] {
                slot.set_disabled(cx, disabled);
            }
            for chip in self.chips.clone() {
                chip.set_disabled(cx, disabled);
            }
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }
}

impl TagFieldRef {
    /// The tags, whatever they are now.
    pub fn tags(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.tags()).unwrap_or_default()
    }

    pub fn set_tags(&self, cx: &mut Cx, tags: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tags(cx, tags);
        }
    }

    /// Offer one tag the way the input does, and say what happened to it.
    pub fn add(&self, cx: &mut Cx, tag: &str) -> TagOffer {
        match self.borrow_mut() {
            Some(mut inner) => inner.add(cx, tag),
            None => TagOffer::Blank,
        }
    }

    pub fn clear_all(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_all(cx);
        }
    }

    /// The whole list, after any move. The one to read for the answer.
    pub fn changed(&self, actions: &Actions) -> Option<Vec<String>> {
        for action in actions.filter_widget_actions_cast::<TagFieldAction>(self.widget_uid()) {
            if let TagFieldAction::Changed(tags) = action {
                return Some(tags);
            }
        }
        None
    }

    pub fn added(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TagFieldAction>(self.widget_uid()) {
            if let TagFieldAction::Added(tag) = action {
                return Some(tag);
            }
        }
        None
    }

    pub fn removed(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TagFieldAction>(self.widget_uid()) {
            if let TagFieldAction::Removed(tag) = action {
                return Some(tag);
            }
        }
        None
    }

    /// What the field turned away, in the words it is showing about it.
    pub fn refused(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TagFieldAction>(self.widget_uid()) {
            if let TagFieldAction::Refused(note) = action {
                return Some(note);
            }
        }
        None
    }

    pub fn cleared(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions_cast::<TagFieldAction>(self.widget_uid())
            .any(|action| matches!(action, TagFieldAction::Cleared))
    }

    /// Hand the keyboard to the input inside.
    pub fn focus_input(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow() {
            inner.focus_input(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_separator_ends_a_tag_and_the_rest_is_still_being_typed() {
        let (tags, rest) = split_tags("urgent, invoice, fol", ",");
        assert_eq!(tags, vec!["urgent".to_string(), "invoice".to_string()]);
        assert_eq!(rest, " fol", "untrimmed: the space was typed a moment ago");
    }

    /// Pasting goes through the same rule as typing, which is the whole
    /// reason a list off the clipboard becomes a list of tags.
    #[test]
    fn pasted_text_splits_on_the_same_characters() {
        let (tags, rest) = split_tags("one;two\nthree", ";\n");
        assert_eq!(tags, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(rest, "three");
    }

    #[test]
    fn nothing_between_two_separators_is_not_a_tag() {
        let (tags, rest) = split_tags(",, ,a,,", ",");
        assert_eq!(tags, vec!["a".to_string()]);
        assert_eq!(rest, "");
    }

    /// With no separators at all the field commits on Return alone, and a
    /// comma is then just a character in a tag.
    #[test]
    fn a_field_with_no_separators_splits_nothing() {
        let (tags, rest) = split_tags("a,b", "");
        assert!(tags.is_empty());
        assert_eq!(rest, "a,b");
    }

    #[test]
    fn a_duplicate_is_refused_and_names_the_tag_that_is_already_there() {
        let mut tags = vec!["urgent".to_string()];
        assert_eq!(
            offer_tag(&mut tags, " urgent ", 0, false),
            TagOffer::Duplicate("urgent".to_string())
        );
        assert_eq!(tags.len(), 1, "and nothing went in");
        // The refusal carries the tag ALREADY held, not the one offered: the
        // message points at a chip on screen, and under the case-insensitive
        // rule the two are spelt differently.
        assert_eq!(
            offer_tag(&mut tags, "URGENT", 0, false),
            TagOffer::Duplicate("urgent".to_string())
        );
        assert_eq!(offer_tag(&mut tags, "URGENT", 0, true), TagOffer::Added);
        assert_eq!(tags, vec!["urgent".to_string(), "URGENT".to_string()]);
    }

    #[test]
    fn max_caps_the_list() {
        let mut tags = Vec::new();
        assert_eq!(offer_tag(&mut tags, "a", 2, false), TagOffer::Added);
        assert_eq!(offer_tag(&mut tags, "b", 2, false), TagOffer::Added);
        assert_eq!(offer_tag(&mut tags, "c", 2, false), TagOffer::Full(2));
        assert_eq!(tags.len(), 2);
        // A full field handed something it already holds says the useful
        // thing rather than complaining about its size.
        assert_eq!(
            offer_tag(&mut tags, "a", 2, false),
            TagOffer::Duplicate("a".to_string())
        );
    }

    #[test]
    fn no_max_is_no_cap() {
        let mut tags = Vec::new();
        for i in 0..50 {
            assert_eq!(offer_tag(&mut tags, &format!("t{i}"), 0, false), TagOffer::Added);
        }
        assert_eq!(tags.len(), 50);
    }

    #[test]
    fn nothing_but_spaces_is_not_a_tag() {
        let mut tags = Vec::new();
        assert_eq!(offer_tag(&mut tags, "   ", 0, false), TagOffer::Blank);
        assert!(tags.is_empty());
        assert_eq!(refusal_note(&TagOffer::Blank), None, "and nothing to say about it");
    }

    /// A list written in markup goes through the same two rules as one typed
    /// in, so a declared field cannot start out breaking its own.
    #[test]
    fn a_declared_list_is_settled_by_the_same_rules() {
        let mut tags = Vec::new();
        for tag in ["a", "A", "b", "c", "d"] {
            offer_tag(&mut tags, tag, 3, false);
        }
        assert_eq!(tags, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    /// Both refusals say something, and both say it in words rather than in
    /// a code the host would have to translate.
    #[test]
    fn every_refusal_has_a_line_to_show() {
        assert!(refusal_note(&TagOffer::Duplicate("urgent".to_string()))
            .unwrap()
            .contains("urgent"));
        assert_eq!(refusal_note(&TagOffer::Full(1)).unwrap(), "this field takes one tag");
        assert_eq!(refusal_note(&TagOffer::Full(4)).unwrap(), "this field takes 4 tags");
        assert_eq!(refusal_note(&TagOffer::Added), None);
    }
}
