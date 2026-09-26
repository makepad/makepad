//! PropertyInspector — an object's properties as rows of name and editor,
//! grouped under headings, each row taking the editor its value asks for.
//!
//! The input is the shape reflection already produces. `reflect_flat`
//! ([`crate::reflect`]) answers with `(name, value-as-text, is_set)` for
//! every property of a live widget, one level of dotted nesting, and that is
//! what a [`Prop`] carries: a name, a value as TEXT, and the few things the
//! host knows that the text cannot say — bounds for a number, the options
//! behind an enum, whether a change is allowed at all. What comes back out
//! is the same shape: a name, and a new value as text.
//!
//! Text as the single channel is the decision the rest of this module falls
//! out of. Five editors write five kinds of value, and a panel that modelled
//! each one would need the host's type system inside it to do so. A colour
//! is `#3c5a8aff`, a number is `12.5`, a flag is `true`; the host parses back
//! into whatever it actually keeps, which is where that knowledge lives.
//!
//! # Which number field, and why there is no third one
//!
//! Two draggable numbers already exist here and both are used; neither was
//! rewritten.
//!
//! * `mod.widgets.FabValueInput` ([`crate::fab_controls`]) is the default,
//!   because it is the property-panel field: a short fixed row with the
//!   number right-anchored so a column of them lines up, three points of
//!   travel before a press becomes a drag (so a click meaning to open the
//!   keyboard cannot nudge the value on the way), `Changed` live and `Ended`
//!   once at the commit point, and a double-click that asks for the default
//!   back.
//! * `mod.widgets.ValueInput` ([`crate::value_input`]) is the
//!   `row_number_wide` template, chosen by `wide_numbers: true`, for an
//!   inspector used as a form on a page rather than as a side panel:
//!   ordinary widget height, the library's own theme instead of the panel
//!   token table, and a step arrow at each end. It reports once per gesture,
//!   so for it the change and the commit are the same moment.
//!
//! How many decimals a number shows belongs to the template, not to the
//! property: a column reads best sharing one precision, and both fields take
//! it where the row is defined.
//!
//! # Give it a width
//!
//! The rows fill the panel and the panel fills its parent. A panel laid out
//! `width: Fit` gives its rows nothing to fill, and they lay out and never
//! paint — put it in something with a width, or say one.
//!
//! # What it deliberately does not do
//!
//! **It never writes to the object it describes.** A row reports and the
//! host applies. The host owns undo, the clamp, and what a property means;
//! an inspector that wrote through would be a second author of the same
//! state, and the two would disagree the first time a value was refused.
//!
//! **It does not scroll or recycle rows.** A panel is a few headings and a
//! few dozen rows: that draws in one pass and folds instantly. Thousands of
//! properties want a virtual list, and a virtual list wants rows of one
//! height, which is the opposite of a column of mixed editors.
//!
//! **It does not open nested values.** An inset is one row of text, not
//! four. A host that wants four rows hands over four properties.
//!
//! **It is not a form.** Validation, when a message is due, and where the
//! keyboard goes after a refused submit are a form controller's job: they
//! need to know the order the fields were meant to be filled in, and this
//! panel's order is whatever the host handed over.

use crate::{
    animator::Animate,
    button::ButtonAction,
    check_box::{CheckBox, CheckBoxAction, CheckState},
    drop_down::{DropDownAction, DropDownWidgetRefExt},
    fab_controls::{
        format_hex, parse_hex, FabColorPick, FabColorPickAction, FabValueInput,
        FabValueInputAction,
    },
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::TextInputAction,
    value_input::{ValueInput, ValueInputAction},
    widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

// ===========================================================================
// The row model. Pure: no Cx, no script heap. Every decision the panel takes
// before it draws anything is here, and the tests at the bottom are of this.
// ===========================================================================

/// The editor a property gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    Number,
    Text,
    Bool,
    Color,
    /// One name out of a fixed set.
    Choice,
    /// Shown, never edited.
    Info,
}

/// One property, as the host hands it over.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    /// The name reflection reports, dots and all: `draw_bg.color`.
    pub name: String,
    /// The value as the editor should show it. A host reading from
    /// reflection strips the quotes reflection puts around a string; what
    /// arrives here is what a person is meant to read.
    pub value: String,
    /// The heading this row sits under. Empty takes the group from the
    /// name's own prefix, and a name with no prefix leads the panel with no
    /// heading over it.
    pub group: String,
    /// Force the editor. `None` reads it off the value.
    pub kind: Option<PropKind>,
    /// Number rows: the scrub's bounds. `max <= min` is unbounded — the
    /// field then moves one step per pixel and draws no fill bar.
    pub min: f64,
    pub max: f64,
    /// Number rows: one step of the gesture.
    pub step: f64,
    /// Choice rows: the options in menu order. A value that is not one of
    /// them shows as the first.
    pub options: Vec<String>,
    /// False for a property the host will not accept a change to.
    pub editable: bool,
}

impl Prop {
    /// A property from what reflection already gives: a name and a value as
    /// text. Everything else is read off those two.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
            group: String::new(),
            kind: None,
            min: 0.0,
            max: 0.0,
            step: 0.01,
            options: Vec::new(),
            editable: true,
        }
    }

    /// Put the row under a heading of the host's own choosing rather than
    /// under the one its name implies.
    pub fn in_group(mut self, group: &str) -> Self {
        self.group = group.to_string();
        self
    }

    /// A number with a range: the field draws the value's place in it and
    /// one drag across the row sweeps it.
    pub fn number(mut self, min: f64, max: f64, step: f64) -> Self {
        self.kind = Some(PropKind::Number);
        self.min = min;
        self.max = max;
        self.step = step;
        self
    }

    /// A number with no range: one step per pixel of travel, no fill bar.
    pub fn unbounded(mut self, step: f64) -> Self {
        self.kind = Some(PropKind::Number);
        self.min = 0.0;
        self.max = 0.0;
        self.step = step;
        self
    }

    /// The names this property can hold. Giving them is what turns a word
    /// into a menu — the text alone cannot say what else was possible.
    pub fn choices(mut self, options: &[&str]) -> Self {
        self.options = options.iter().map(|o| o.to_string()).collect();
        self
    }

    /// Shown, not edited.
    pub fn read_only(mut self) -> Self {
        self.editable = false;
        self
    }
}

/// The editor a value's own text asks for. This is the overlay's reading of
/// a reflected value ([`crate::reflect`]) and nothing more: a hash and hex
/// digits are a colour, something that parses is a number, `true`/`false` is
/// a flag, and everything else is text. A quoted string needs no case of its
/// own — the quotes drop it out of the number and colour tests by
/// themselves.
///
/// It never answers [`PropKind::Choice`]: no value can say what the other
/// choices would have been. Only the host can, by listing them.
pub fn classify(value: &str) -> PropKind {
    let value = value.trim();
    if value.starts_with('#') && parse_hex(value).is_some() {
        return PropKind::Color;
    }
    if value.parse::<f64>().is_ok() {
        return PropKind::Number;
    }
    if value == "true" || value == "false" {
        return PropKind::Bool;
    }
    PropKind::Text
}

/// The editor a row gets: nothing at all for a property that cannot be
/// changed, then what the host insisted on, then a menu if there are options
/// to put in one, then whatever the value reads as. A menu with nothing in
/// it is text, because a list of no choices is not a choice.
pub fn editor_of(prop: &Prop) -> PropKind {
    if !prop.editable {
        return PropKind::Info;
    }
    let kind = match prop.kind {
        Some(kind) => kind,
        None if !prop.options.is_empty() => PropKind::Choice,
        None => classify(&prop.value),
    };
    if kind == PropKind::Choice && prop.options.is_empty() {
        return PropKind::Text;
    }
    kind
}

/// The heading a property sits under: what the host said, or the part of the
/// name before the first dot, or nothing.
pub fn group_of(prop: &Prop) -> &str {
    if !prop.group.is_empty() {
        return prop.group.as_str();
    }
    prop.name
        .split_once('.')
        .map(|(head, _)| head)
        .unwrap_or("")
}

/// What a row's label says. A group taken from the name's own prefix is
/// already written on the heading above it, so the row shows the leaf; a
/// group the host named itself is nowhere in the name, so the row shows all
/// of it.
pub fn label_of(prop: &Prop) -> &str {
    if prop.group.is_empty() {
        if let Some((_, leaf)) = prop.name.split_once('.') {
            return leaf;
        }
    }
    prop.name.as_str()
}

/// One line of the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum InspectorRow {
    /// A group's heading. `count` is how many properties belong to it,
    /// whether or not they are currently shown.
    Heading {
        group: String,
        count: usize,
        open: bool,
    },
    /// A property: which one, and the editor it takes.
    Field { index: usize, kind: PropKind },
}

/// The panel's lines, in order.
///
/// Groups keep the order their first property appeared in, and a group
/// gathers every property that names it however far apart they were handed
/// over — an inspector whose rows re-ordered themselves as values arrived
/// would be unusable. The properties with no group lead, with no heading
/// over them: they are the object's own, and a heading reading "General"
/// would be a word this module invented.
///
/// `closed` names the groups whose properties are folded away. A closed
/// group still shows its heading and still counts everything it has.
pub fn build_rows(props: &[Prop], closed: &[String]) -> Vec<InspectorRow> {
    let mut order: Vec<&str> = Vec::new();
    for prop in props {
        let group = group_of(prop);
        if !order.iter().any(|seen| *seen == group) {
            order.push(group);
        }
    }
    // Stable, so the named groups keep first-appearance order behind the
    // ungrouped ones.
    order.sort_by_key(|group| !group.is_empty());

    let mut rows = Vec::new();
    for group in order {
        let members: Vec<usize> = props
            .iter()
            .enumerate()
            .filter(|(_, prop)| group_of(prop) == group)
            .map(|(index, _)| index)
            .collect();
        let open = if group.is_empty() {
            true
        } else {
            let open = !closed.iter().any(|shut| shut == group);
            rows.push(InspectorRow::Heading {
                group: group.to_string(),
                count: members.len(),
                open,
            });
            open
        };
        if open {
            for index in members {
                rows.push(InspectorRow::Field {
                    index,
                    kind: editor_of(&props[index]),
                });
            }
        }
    }
    rows
}

/// A number as the text a host reads back. Six decimals with the trailing
/// zeros taken off: a scrub lands on values like `0.30000000000000004`, and
/// a property panel that showed one would be reporting the float's arithmetic
/// rather than the value.
pub fn number_text(value: f64) -> String {
    let mut text = format!("{value:.6}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// A heading's line: the fold mark, the group, and how many properties are
/// under it.
fn heading_text(group: &str, count: usize, open: bool) -> String {
    // U+25BC and U+25B2 are both in the default face chain. The sideways
    // triangle a fold usually uses is not on the checked list, and a glyph
    // no face carries draws as a box.
    let mark = if open { "\u{25bc}" } else { "\u{25b2}" };
    format!("{mark}  {group}   {count}")
}

/// What a row of the panel reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum PropertyInspectorAction {
    /// A property's editor moved: its name, and its new value as text. Live
    /// — one per step of a scrub.
    Changed(String, String),
    /// The gesture behind a change finished: a mouse up, an Enter, a toggle,
    /// a menu pick. This is where a host writes the value down; following
    /// every `Changed` into a document writes hundreds of times per drag.
    Committed(String, String),
    /// A row asked for its property's default back (a double-click on a
    /// number). Only the host knows what that default is.
    Reset(String),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.PropertyInspectorBase = #(PropertyInspector::register_widget(vm))

    /** An object's properties as rows of name and editor, grouped under
     * headings. The host hands over the properties and takes the changes
     * back; the panel owns neither the values nor what they mean. */
    mod.widgets.PropertyInspector = set_type_default() do mod.widgets.PropertyInspectorBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: 1.
        padding: theme.mspace_1
        /** number rows take the roomier field instead of the panel one */
        wide_numbers: false
        /** a click on a heading folds its group away */
        folding: true

        draw_bg +: {
            color: theme.color_surface_container_low
        }

        // The row templates arrive as NAMED entries: a name lands in the
        // object's vec, where the widget collects it, instead of being drawn
        // as a child. Every one of them calls its label `name` and its
        // editor `value`, so one lookup dresses any row.
        row_heading := mod.widgets.ButtonFlatter{
            width: Fill
            height: 22.
            align: Align{x: 0. y: 0.5}
            margin: 0.
            padding: theme.mspace_h_1
            text: ""
            draw_text +: {
                color: theme.color_text
                text_style: theme.font_bold{font_size: theme.font_size_p}
            }
        }

        row_number := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.FabValueInput{
                width: 116.
                height: 20.
                precision: 2
            }
        }

        row_number_wide := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.ValueInput{
                width: 116.
                height: 22.
                precision: 2.0
            }
        }

        row_text := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.TextInput{
                width: 116.
                height: 20.
                empty_text: ""
            }
        }

        row_bool := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            // The same width as every other editor, though the mark needs a
            // fraction of it: a column whose boxes started somewhere else
            // from its fields reads as two columns.
            value := mod.widgets.CheckBox{
                width: 116.
                height: Fit
                text: ""
            }
        }

        row_color := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.FabColorPick{
                width: 116.
                height: 18.
            }
        }

        row_choice := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.DropDown{
                width: 116.
                margin: 0.
            }
        }

        row_info := mod.widgets.View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            padding: theme.mspace_h_1
            name := mod.widgets.Label{
                width: Fill
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {color: theme.color_text_meta}
            }
            value := mod.widgets.Label{
                width: 116.
                height: Fit
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
            }
        }
    }
}

/// One row on screen: the widget, the template it came from, and whether it
/// has been handed the things that are pushed once rather than every draw.
struct RowItem {
    template: LiveId,
    widget: WidgetRef,
    /// A menu's options go in when the row is built. `set_labels` redraws
    /// whether or not the list changed, and asking for a redraw during a
    /// draw asks for another draw, forever.
    seeded: bool,
}

#[derive(Script, Widget)]
pub struct PropertyInspector {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The panel's own ground. The rows are separate widgets, so without a
    /// rect of its own the panel has nothing to paint, to redraw or to be
    /// found by.
    #[redraw]
    #[live]
    draw_bg: DrawColor,

    /// Number rows take the roomier field (`ValueInput`) instead of the
    /// panel one (`FabValueInput`) — see the module doc for which suits
    /// where.
    #[live]
    pub wide_numbers: bool,
    /// A click on a heading folds its group away.
    #[live(true)]
    pub folding: bool,

    /// The row templates, collected from the instance by name.
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    props: Vec<Prop>,
    #[rust]
    rows: Vec<InspectorRow>,
    #[rust]
    closed: Vec<String>,
    #[rust]
    items: HashMap<LiveId, RowItem>,
}

impl ScriptHook for PropertyInspector {
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
        // The row templates arrive as named entries on the instance, the way
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
            // The rows were built from templates that may have just changed,
            // so they are rebuilt rather than patched.
            self.items.clear();
        }
    }
}

/// The key a heading's widget is kept under.
fn heading_key(group: &str) -> LiveId {
    LiveId::from_str(group)
}

/// The key a property's row is kept under: the property's place in the list,
/// which survives a fold. InspectorRow POSITIONS do not — folding a group above moves
/// every row below it, and the rows would swap widgets under the pointer.
///
/// A group name whose 64-bit hash landed on a small integer is the only way
/// these two can collide.
fn field_key(index: usize) -> LiveId {
    LiveId(index as u64 + 1)
}

impl PropertyInspector {
    /// The properties to show. Handing over the same list with new values is
    /// what a host does on every refresh, so only a change of name, group or
    /// editor rebuilds the row widgets — rebuilding on a value change would
    /// take the field out from under the hand dragging it.
    pub fn set_props(&mut self, cx: &mut Cx, props: Vec<Prop>) {
        if self.props == props {
            return;
        }
        // The options count as shape: a menu is handed its list once, when
        // its row is built (see `RowItem::seeded`), so a new list needs a new
        // row.
        let same_shape = self.props.len() == props.len()
            && self.props.iter().zip(props.iter()).all(|(was, now)| {
                was.name == now.name
                    && group_of(was) == group_of(now)
                    && editor_of(was) == editor_of(now)
                    && was.options == now.options
            });
        if !same_shape {
            self.items.clear();
        }
        self.props = props;
        self.rows = build_rows(&self.props, &self.closed);
        self.redraw(cx);
    }

    pub fn props(&self) -> &[Prop] {
        &self.props
    }

    /// The value the panel currently shows for a property.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|prop| prop.name == name)
            .map(|prop| prop.value.as_str())
    }

    pub fn is_group_open(&self, group: &str) -> bool {
        !self.closed.iter().any(|shut| shut == group)
    }

    pub fn set_group_open(&mut self, cx: &mut Cx, group: &str, open: bool) {
        let at = self.closed.iter().position(|shut| shut == group);
        match (at, open) {
            (Some(at), true) => {
                self.closed.remove(at);
            }
            (None, false) => self.closed.push(group.to_string()),
            _ => return,
        }
        self.rows = build_rows(&self.props, &self.closed);
        self.redraw(cx);
    }

    fn template_for(&self, kind: PropKind) -> LiveId {
        match kind {
            PropKind::Number if self.wide_numbers => live_id!(row_number_wide),
            PropKind::Number => live_id!(row_number),
            PropKind::Text => live_id!(row_text),
            PropKind::Bool => live_id!(row_bool),
            PropKind::Color => live_id!(row_color),
            PropKind::Choice => live_id!(row_choice),
            PropKind::Info => live_id!(row_info),
        }
    }

    /// The widget for one row, built from its template the first time and
    /// kept afterwards. Answers whether it was built just now, for the
    /// things that are pushed once. A row whose editor changed gets a new
    /// widget: the template decides the type, and a swatch cannot become a
    /// checkbox.
    fn item(&mut self, cx: &mut Cx, key: LiveId, template: LiveId) -> Option<(WidgetRef, bool)> {
        if let Some(item) = self.items.get(&key) {
            if item.template == template {
                return Some((item.widget.clone(), !item.seeded));
            }
        }
        let Some(template_ref) = self.templates.get(&template) else {
            warning!("PropertyInspector: no row template named {template}");
            return None;
        };
        let value: ScriptValue = template_ref.as_object().into();
        let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        // A tree node under the panel, so the design overlay can pick a row
        // and style the template it came from.
        cx.widget_tree_insert_child(self.uid, key, widget.clone());
        self.items.insert(
            key,
            RowItem {
                template,
                widget: widget.clone(),
                seeded: false,
            },
        );
        Some((widget, true))
    }

    /// Put the property's name and value into a row's two children.
    fn dress(&mut self, cx: &mut Cx, row: &WidgetRef, index: usize, kind: PropKind, fresh: bool) {
        let Some(prop) = self.props.get(index).cloned() else {
            return;
        };
        row.child(live_id!(name)).set_text(cx, label_of(&prop));
        let field = row.child(live_id!(value));
        match kind {
            PropKind::Number => {
                let value = prop.value.trim().parse::<f64>().unwrap_or(0.0);
                let bounded = prop.max > prop.min;
                // Both number fields answer here. Whichever template this row
                // came from, one of the two borrows succeeds — which is the
                // whole point of not writing a third one.
                if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                    input.set_hint(
                        bounded.then_some(prop.min),
                        bounded.then_some(prop.max),
                        Some(prop.step),
                    );
                    input.set_value(cx, value);
                }
                if let Some(mut input) = field.borrow_mut::<ValueInput>() {
                    if bounded {
                        input.min = prop.min;
                        input.max = prop.max;
                    }
                    input.step = prop.step;
                    input.set_value(cx, value);
                }
            }
            PropKind::Text => {
                // Never while the keyboard is in it: pushing the model's text
                // into a field somebody is typing in moves their cursor to
                // the end of it.
                let owned = field.area() != Area::Empty && cx.has_key_focus(field.area());
                if !owned && field.text() != prop.value {
                    field.set_text(cx, &prop.value);
                }
            }
            PropKind::Bool => {
                if let Some(mut check) = field.borrow_mut::<CheckBox>() {
                    let want = prop.value.trim() == "true";
                    // Only on a real difference: cutting the animator
                    // re-applies and redraws whether or not the box is
                    // already there, and a redraw asked for during a draw
                    // asks for another draw. The cost is that a box whose
                    // shader uniform went stale over a script reload waits
                    // for the next real change to catch up.
                    if (check.state(cx) == CheckState::On) != want {
                        check.set_active(cx, want, Animate::No);
                    }
                }
            }
            PropKind::Color => {
                if let Some((rgba, _)) = parse_hex(&prop.value) {
                    if let Some(mut pick) = field.borrow_mut::<FabColorPick>() {
                        // set_rgba redraws unconditionally, so the same
                        // colour every draw would spin. The tolerance is half
                        // a step of an 8-bit channel: a colour that has been
                        // round-tripped through hex is the same colour.
                        let now = pick.rgba();
                        let moved = now
                            .iter()
                            .zip(rgba.iter())
                            .any(|(now, want)| (now - want).abs() > 0.002);
                        if moved && !pick.is_open() {
                            pick.set_rgba(cx, rgba);
                        }
                    }
                }
            }
            PropKind::Choice => {
                let menu = field.as_drop_down();
                if fresh {
                    menu.set_labels(cx, prop.options.clone());
                }
                let at = prop
                    .options
                    .iter()
                    .position(|option| *option == prop.value)
                    .unwrap_or(0);
                menu.set_selected_item(cx, at);
            }
            PropKind::Info => {
                field.set_text(cx, &prop.value);
            }
        }
        if fresh {
            if let Some(item) = self.items.get_mut(&field_key(index)) {
                item.seeded = true;
            }
        }
    }

    /// A change from one row: the panel's own state first, then the report.
    /// `Changed` only when the value really moved, `Committed` whenever the
    /// gesture behind it finished.
    fn publish(&mut self, cx: &mut Cx, index: usize, value: String, ended: bool) {
        let uid = self.uid;
        let Some(prop) = self.props.get_mut(index) else {
            return;
        };
        let moved = prop.value != value;
        prop.value = value.clone();
        let name = prop.name.clone();
        if moved {
            cx.widget_action(
                uid,
                PropertyInspectorAction::Changed(name.clone(), value.clone()),
            );
        }
        if ended {
            cx.widget_action(uid, PropertyInspectorAction::Committed(name, value));
        }
    }
}

impl Widget for PropertyInspector {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let rows = self.rows.clone();
        for row in &rows {
            match row {
                InspectorRow::Heading { group, count, open } => {
                    let Some((widget, _)) =
                        self.item(cx.cx.cx, heading_key(group), live_id!(row_heading))
                    else {
                        continue;
                    };
                    widget.set_text(cx.cx.cx, &heading_text(group, *count, *open));
                    let row_walk = widget.walk(cx.cx.cx);
                    let _ = widget.draw_walk(cx, scope, row_walk);
                }
                InspectorRow::Field { index, kind } => {
                    let template = self.template_for(*kind);
                    let Some((widget, fresh)) = self.item(cx.cx.cx, field_key(*index), template)
                    else {
                        continue;
                    };
                    self.dress(cx.cx.cx, &widget, *index, *kind, fresh);
                    let row_walk = widget.walk(cx.cx.cx);
                    let _ = widget.draw_walk(cx, scope, row_walk);
                }
            }
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The rows on screen, not every row ever built: a folded group's
        // widgets are kept for when it opens again, and handing them events
        // would let a field nobody can see hold the keyboard.
        let mut widgets: Vec<WidgetRef> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let key = match row {
                InspectorRow::Heading { group, .. } => heading_key(group),
                InspectorRow::Field { index, .. } => field_key(*index),
            };
            if let Some(item) = self.items.get(&key) {
                widgets.push(item.widget.clone());
            }
        }
        for widget in &widgets {
            widget.handle_event(cx, event, scope);
        }
        let Event::Actions(actions) = event else {
            return;
        };
        let rows = self.rows.clone();
        for row in &rows {
            match row {
                InspectorRow::Heading { group, .. } => {
                    if !self.folding {
                        continue;
                    }
                    let uid = match self.items.get(&heading_key(group)) {
                        Some(item) => item.widget.widget_uid(),
                        None => continue,
                    };
                    let Some(action) = actions.find_widget_action(uid) else {
                        continue;
                    };
                    if let ButtonAction::Clicked(_) = action.cast() {
                        let open = self.is_group_open(group);
                        self.set_group_open(cx, group, !open);
                    }
                }
                InspectorRow::Field { index, kind } => {
                    let field = match self.items.get(&field_key(*index)) {
                        Some(item) => item.widget.child(live_id!(value)),
                        None => continue,
                    };
                    let Some(action) = actions.find_widget_action(field.widget_uid()) else {
                        continue;
                    };
                    let Some(prop) = self.props.get(*index).cloned() else {
                        continue;
                    };
                    let change = match kind {
                        PropKind::Number => match action.cast::<FabValueInputAction>() {
                            FabValueInputAction::Changed(v) => Some((number_text(v), false)),
                            FabValueInputAction::Ended(v) => Some((number_text(v), true)),
                            FabValueInputAction::Reset => {
                                let uid = self.uid;
                                cx.widget_action(
                                    uid,
                                    PropertyInspectorAction::Reset(prop.name.clone()),
                                );
                                None
                            }
                            // The roomier field reports once per gesture, so
                            // for it the change and the commit are the same
                            // moment.
                            FabValueInputAction::None => match action.cast::<ValueInputAction>() {
                                ValueInputAction::Changed(v) => Some((number_text(v), true)),
                                ValueInputAction::None => None,
                            },
                        },
                        PropKind::Text => match action.cast::<TextInputAction>() {
                            TextInputAction::Changed(text) => Some((text, false)),
                            TextInputAction::Returned(text, _) => Some((text, true)),
                            // Leaving the field is a commit: a person who
                            // clicks away has finished with it.
                            TextInputAction::KeyFocusLost => Some((field.text(), true)),
                            _ => None,
                        },
                        PropKind::Bool => match action.cast::<CheckBoxAction>() {
                            CheckBoxAction::Change(on) => Some((on.to_string(), true)),
                            CheckBoxAction::None => None,
                        },
                        PropKind::Color => {
                            // The panel answers in the shape it was given: a
                            // colour handed over with an alpha byte goes back
                            // with one.
                            let with_alpha = parse_hex(&prop.value)
                                .map(|(_, had_alpha)| had_alpha)
                                .unwrap_or(true);
                            match action.cast::<FabColorPickAction>() {
                                FabColorPickAction::Changed(c) => Some((
                                    format_hex([c.x, c.y, c.z, c.w], with_alpha),
                                    false,
                                )),
                                FabColorPickAction::Ended(c) => {
                                    Some((format_hex([c.x, c.y, c.z, c.w], with_alpha), true))
                                }
                                _ => None,
                            }
                        }
                        PropKind::Choice => match action.cast::<DropDownAction>() {
                            DropDownAction::Select(at) => {
                                prop.options.get(at).map(|option| (option.clone(), true))
                            }
                            DropDownAction::None => None,
                        },
                        PropKind::Info => None,
                    };
                    if let Some((value, ended)) = change {
                        self.publish(cx, *index, value, ended);
                    }
                }
            }
        }
    }

    /// The panel in one line, so a test can read it without walking rows.
    fn text(&self) -> String {
        self.props
            .iter()
            .map(|prop| format!("{}={}", prop.name, prop.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl PropertyInspectorRef {
    pub fn set_props(&self, cx: &mut Cx, props: Vec<Prop>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_props(cx, props);
        }
    }

    /// How many properties the panel is showing.
    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.props().len()).unwrap_or(0)
    }

    pub fn value(&self, name: &str) -> Option<String> {
        self.borrow()
            .and_then(|inner| inner.value(name).map(|value| value.to_string()))
    }

    pub fn set_group_open(&self, cx: &mut Cx, group: &str, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_group_open(cx, group, open);
        }
    }

    pub fn is_group_open(&self, group: &str) -> bool {
        self.borrow()
            .map(|inner| inner.is_group_open(group))
            .unwrap_or(true)
    }

    /// Everything the panel said in one pass. It says more than one thing
    /// often — a change and the commit that ended it, or two rows moving at
    /// once — so each reader below scans instead of taking the first.
    fn said(&self, actions: &Actions) -> Vec<PropertyInspectorAction> {
        let uid = self.widget_uid();
        let mut said = Vec::new();
        for action in actions {
            if let Some(action) = action.as_widget_action() {
                if action.widget_uid == uid {
                    said.push(action.cast::<PropertyInspectorAction>());
                }
            }
        }
        said
    }

    /// The property that moved this pass and its new value, live.
    pub fn changed(&self, actions: &Actions) -> Option<(String, String)> {
        self.said(actions).into_iter().find_map(|action| match action {
            PropertyInspectorAction::Changed(name, value) => Some((name, value)),
            _ => None,
        })
    }

    /// The property whose gesture finished this pass. Where a host writes.
    pub fn committed(&self, actions: &Actions) -> Option<(String, String)> {
        self.said(actions).into_iter().find_map(|action| match action {
            PropertyInspectorAction::Committed(name, value) => Some((name, value)),
            _ => None,
        })
    }

    /// The property asked to go back to its default this pass.
    pub fn reset(&self, actions: &Actions) -> Option<String> {
        self.said(actions).into_iter().find_map(|action| match action {
            PropertyInspectorAction::Reset(name) => Some(name),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props() -> Vec<Prop> {
        vec![
            Prop::new("visible", "true"),
            Prop::new("draw_bg.color", "#3c5a8aff"),
            Prop::new("text", "Untitled"),
            Prop::new("draw_bg.radius", "4.0"),
        ]
    }

    /// A name with a dot carries its own heading, and the row under that
    /// heading does not repeat it.
    #[test]
    fn a_group_comes_from_the_name_when_the_host_does_not_say() {
        let dotted = Prop::new("draw_bg.color", "#000");
        assert_eq!(group_of(&dotted), "draw_bg");
        assert_eq!(label_of(&dotted), "color");

        let plain = Prop::new("visible", "true");
        assert_eq!(group_of(&plain), "");
        assert_eq!(label_of(&plain), "visible");

        // A heading the host chose is nowhere in the name, so the row keeps
        // the whole of it.
        let told = Prop::new("draw_bg.color", "#000").in_group("Style");
        assert_eq!(group_of(&told), "Style");
        assert_eq!(label_of(&told), "draw_bg.color");
    }

    /// The object's own properties lead, with nothing written over them.
    #[test]
    fn the_ungrouped_properties_lead_the_panel_without_a_heading() {
        let rows = build_rows(&props(), &[]);
        assert_eq!(
            rows[0],
            InspectorRow::Field {
                index: 0,
                kind: PropKind::Bool
            }
        );
        assert_eq!(
            rows[1],
            InspectorRow::Field {
                index: 2,
                kind: PropKind::Text
            }
        );
        assert!(matches!(rows[2], InspectorRow::Heading { .. }));
    }

    /// A group gathers everything that names it, however far apart the
    /// properties were handed over — a panel whose rows re-ordered
    /// themselves as values arrived would be unusable.
    #[test]
    fn a_group_gathers_its_properties_wherever_they_appeared() {
        let rows = build_rows(&props(), &[]);
        assert_eq!(
            rows[2],
            InspectorRow::Heading {
                group: "draw_bg".to_string(),
                count: 2,
                open: true,
            }
        );
        assert_eq!(
            rows[3],
            InspectorRow::Field {
                index: 1,
                kind: PropKind::Color
            }
        );
        assert_eq!(
            rows[4],
            InspectorRow::Field {
                index: 3,
                kind: PropKind::Number
            }
        );
        assert_eq!(rows.len(), 5);
    }

    /// Groups keep the order their first property appeared in, not the
    /// order the names sort in.
    #[test]
    fn groups_keep_the_order_their_first_property_arrived_in() {
        let props = vec![
            Prop::new("zebra.one", "1"),
            Prop::new("alpha.one", "1"),
            Prop::new("zebra.two", "2"),
        ];
        let headings: Vec<String> = build_rows(&props, &[])
            .into_iter()
            .filter_map(|row| match row {
                InspectorRow::Heading { group, .. } => Some(group),
                _ => None,
            })
            .collect();
        assert_eq!(headings, vec!["zebra".to_string(), "alpha".to_string()]);
    }

    /// A closed group keeps its heading and still counts what it has: the
    /// count is the reason to open it again.
    #[test]
    fn a_closed_group_keeps_its_heading_and_its_count_and_shows_nothing() {
        let rows = build_rows(&props(), &["draw_bg".to_string()]);
        assert_eq!(
            rows.last().unwrap(),
            &InspectorRow::Heading {
                group: "draw_bg".to_string(),
                count: 2,
                open: false,
            }
        );
        assert_eq!(rows.len(), 3, "two ungrouped rows and one heading");
    }

    /// The editor comes off the value's own text, the same reading the
    /// design overlay makes of a reflected property.
    #[test]
    fn the_editor_comes_from_the_value_when_nothing_overrides_it() {
        assert_eq!(classify("#3c5a8a"), PropKind::Color);
        assert_eq!(classify("#3c5a8aff"), PropKind::Color);
        assert_eq!(classify("12.5"), PropKind::Number);
        assert_eq!(classify("-3"), PropKind::Number);
        assert_eq!(classify("true"), PropKind::Bool);
        assert_eq!(classify("false"), PropKind::Bool);
        assert_eq!(classify("Ellipsis"), PropKind::Text);
        // Quotes need no case of their own: they drop the value out of the
        // number and colour tests by themselves.
        assert_eq!(classify("\"12\""), PropKind::Text);
        // A hash with something that is not hex behind it is a word.
        assert_eq!(classify("#nothex"), PropKind::Text);
    }

    /// Only the host can say what the other choices would have been, so
    /// listing them is what turns a word into a menu — and a menu with
    /// nothing in it is not a menu.
    #[test]
    fn options_make_a_menu_and_a_menu_with_nothing_in_it_is_text() {
        let free = Prop::new("flow", "Down");
        assert_eq!(editor_of(&free), PropKind::Text);

        let listed = Prop::new("flow", "Down").choices(&["Right", "Down", "Overlay"]);
        assert_eq!(editor_of(&listed), PropKind::Choice);

        let mut empty = Prop::new("flow", "Down");
        empty.kind = Some(PropKind::Choice);
        assert_eq!(editor_of(&empty), PropKind::Text);
    }

    /// What the host insists on beats what the text reads as: a number
    /// range is a claim the value alone cannot make.
    #[test]
    fn the_host_can_override_what_the_text_reads_as() {
        let forced = Prop::new("weight", "3").number(0.0, 10.0, 0.5);
        assert_eq!(editor_of(&forced), PropKind::Number);
        assert_eq!(forced.max, 10.0);

        let mut text = Prop::new("count", "3");
        text.kind = Some(PropKind::Text);
        assert_eq!(editor_of(&text), PropKind::Text);
    }

    /// A property that cannot be changed is shown and not edited, whatever
    /// its value reads as.
    #[test]
    fn a_property_that_cannot_be_edited_is_shown_and_not_edited() {
        let locked = Prop::new("draw_bg.color", "#3c5a8aff").read_only();
        assert_eq!(editor_of(&locked), PropKind::Info);
        let locked = Prop::new("weight", "3").number(0.0, 10.0, 0.5).read_only();
        assert_eq!(editor_of(&locked), PropKind::Info);
    }

    /// A scrub lands on values like 0.30000000000000004, and a panel that
    /// showed one would be reporting the float's arithmetic.
    #[test]
    fn a_number_reads_back_as_the_text_a_person_would_write() {
        assert_eq!(number_text(1.0), "1");
        assert_eq!(number_text(0.5), "0.5");
        assert_eq!(number_text(12.25), "12.25");
        assert_eq!(number_text(-0.1), "-0.1");
        assert_eq!(number_text(0.1 + 0.2), "0.3");
    }

    /// The fold mark says which way the group will go, and the count says
    /// what is behind it.
    #[test]
    fn a_heading_says_the_group_the_count_and_which_way_it_folds() {
        assert_eq!(heading_text("draw_bg", 2, true), "\u{25bc}  draw_bg   2");
        assert_eq!(heading_text("draw_bg", 2, false), "\u{25b2}  draw_bg   2");
    }
}
