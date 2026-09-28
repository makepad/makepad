//! The controls panel: the story's declared controls as live editors.
//!
//! A story names the properties a reader may drive and what kind of value
//! each takes; the panel draws one row per control with the stock widget
//! for that kind and, on every change, hands the app a script chunk
//! (`{ prop: value }`) for the canvas to apply to the story's subject. The
//! values live here, so a rebuilt story gets them again.
//!
//! A story with many controls groups them under sections: a heading row
//! with a disclosure arrow, clicked to fold the controls under it away or
//! back. The fold is the panel's own state and writes nothing to the story.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ControlRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
        padding: Inset{top: 3. bottom: 3. left: 0. right: 0.}
        name := Label{width: 110. text: ""}
    }

    mod.storybook.ControlsPanelBase = #(ControlsPanel::register_widget(vm))
    mod.storybook.ControlsPanel = set_type_default() do mod.storybook.ControlsPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        note := Label{text: "This story declares no controls."}
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            RowBool := ControlRow{
                value := CheckBox{text: ""}
            }
            RowNumber := ControlRow{
                value := Slider{width: 200.}
            }
            RowChoice := ControlRow{
                // The list as wide as the button, so an option that fits the
                // button is not cut short in the list.
                value := DropDown{width: 200. popup_menu +: {width: 200.}}
            }
            RowText := ControlRow{
                value := TextInput{width: 200.}
            }
            RowColor := ControlRow{
                value := TextInput{width: 110.}
                swatch := RoundedView{
                    width: 22.
                    height: 22.
                    show_bg: true
                    draw_bg +: {color: #x888888FF}
                }
            }
            RowDisabled := ControlRow{
                value := CheckBox{text: "disabled"}
            }
            // A curve wants the panel's width, so its name goes over it
            // rather than beside it.
            RowCurve := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: Inset{top: 3. bottom: 6. left: 0. right: 0.}
                name := Label{text: ""}
                // The editor is Fit tall round its canvas and toolbar, so the
                // canvas is what takes the height.
                value := CurveEditor{width: Fill canvas +: {height: 150.}}
            }
            // A section heading. The whole row is the click target, and the
            // arrow points right while folded and down while open.
            RowSection := View{
                width: Fill
                height: Fit
                padding: Inset{top: 8. bottom: 2. left: 0. right: 0.}
                head := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0. y: 0.5}
                    cursor: MouseCursor.Hand
                    arrow := View{
                        width: 12.
                        height: 12.
                        show_bg: true
                        draw_bg +: {
                            open: instance(1.0)
                            color: uniform(theme.color_label_inner)
                            pixel: fn() {
                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                let c = self.rect_size * 0.5
                                // Right-pointing folded, down-pointing open:
                                // each corner slides a quarter turn.
                                let t = self.open
                                sdf.move_to(c.x + mix(-2.0, 3.5, t), c.y + mix(-3.5, -2.0, t))
                                sdf.line_to(c.x + mix(2.5, 0.0, t), c.y + mix(0.0, 2.5, t))
                                sdf.line_to(c.x + mix(-2.0, -3.5, t), c.y + mix(3.5, -2.0, t))
                                sdf.close_path()
                                sdf.fill(self.color)
                                return sdf.result
                            }
                        }
                    }
                    name := Label{text: "" draw_text +: {text_style: theme.font_bold{}}}
                }
            }
        }
    }
}

/// A control's current value.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlValue {
    Bool(bool),
    Number(f64),
    Choice(usize),
    Text(String),
    Color(u32),
    /// A curve's anchors, `[x, y, kind]` each.
    Curve(Vec<[f64; 3]>),
}

/// One edit the panel raised: which control changed and to what.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ControlsAction {
    Changed { index: usize, value: ControlValue },
    #[default]
    None,
}

/// A widget on a story's page moving one of the story's own controls, by
/// label: a gallery item picked by a click, a knob turned on the page. The
/// panel shows the new value and writes it as a hand edit would, so the
/// control, the page and a page rebuilt from its edits agree.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum StoryControlAction {
    Set { label: &'static str, value: ControlValue },
    #[default]
    None,
}

/// The chunk that writes a control's value, and whether it is the disabled
/// switch instead (which is applied through the widget, not a chunk).
pub fn chunk_for(control: &Control, value: &ControlValue) -> Option<String> {
    match (&control.kind, value) {
        (ControlKind::Bool { prop, .. }, ControlValue::Bool(b)) => Some(format!("{{{prop}: {b}}}")),
        (ControlKind::Number { prop, .. }, ControlValue::Number(v)) => Some(format!("{{{prop}: {v:?}}}")),
        (ControlKind::Choice { prop, options, .. }, ControlValue::Choice(i)) => {
            options.get(*i).map(|o| format!("{{{prop}: {o}}}"))
        }
        (ControlKind::Text { prop, .. }, ControlValue::Text(t)) => {
            let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
            Some(format!("{{{prop}: \"{escaped}\"}}"))
        }
        (ControlKind::Color { prop, .. }, ControlValue::Color(c)) => {
            let writes: Vec<String> = prop.split_whitespace().map(|p| format!("{p}: #x{c:08X}")).collect();
            Some(format!("{{{}}}", writes.join(" ")))
        }
        (ControlKind::Curve { prop, .. }, ControlValue::Curve(anchors)) => {
            let rows: Vec<String> = anchors.iter().map(|[x, y, k]| format!("[{x:?}, {y:?}, {k:?}]")).collect();
            Some(format!("{{{prop}: [{}]}}", rows.join(", ")))
        }
        _ => None,
    }
}

/// Whether a control is one the panel links to others by label: a section
/// is a heading and a preset a picker, and neither moves with a namesake.
fn links(control: &Control) -> bool {
    !matches!(control.kind, ControlKind::Section { .. } | ControlKind::Preset { .. })
}

/// Every control that is one control with `index`: those sharing its label
/// (see [`Control::label`]), itself among them.
fn linked(controls: &[Control], index: usize) -> Vec<usize> {
    let Some(control) = controls.get(index) else {
        return Vec::new();
    };
    if !links(control) {
        return vec![index];
    }
    (0..controls.len()).filter(|&i| links(&controls[i]) && controls[i].label == control.label).collect()
}

/// A lane prop split into the vector property and the lane:
/// `draw_bg.material_light[2]` is `("draw_bg.material_light", 2)`.
pub fn lane_of(prop: &str) -> Option<(&str, usize)> {
    let (base, lane) = prop.strip_suffix(']')?.rsplit_once('[')?;
    Some((base, lane.parse().ok()?))
}

/// The property and chunk that write control `index` with the values the
/// panel holds. A lane writes its whole vector, every lane read from the
/// control for it on the same target (see [`ControlKind`]); anything else is
/// [`chunk_for`]. None for a control that writes no property.
pub fn write_for(controls: &[Control], values: &[ControlValue], index: usize) -> Option<(String, String)> {
    let control = controls.get(index)?;
    let prop = prop_of(control);
    let Some((base, _)) = lane_of(prop) else {
        return chunk_for(control, values.get(index)?).map(|chunk| (prop.to_string(), chunk));
    };
    let mut lanes = [0.0f64; 4];
    let mut len = 0;
    for (other, value) in controls.iter().zip(values) {
        if other.target != control.target {
            continue;
        }
        let (Some((other_base, lane)), ControlValue::Number(v)) = (lane_of(prop_of(other)), value) else {
            continue;
        };
        if other_base == base && lane < lanes.len() {
            lanes[lane] = *v;
            len = len.max(lane + 1);
        }
    }
    if len < 2 {
        return None;
    }
    let parts: Vec<String> = lanes[..len].iter().map(|v| format!("{v:?}")).collect();
    Some((base.to_string(), format!("{{{base}: vec{len}({})}}", parts.join(", "))))
}

/// The rows the list shows: every control's index, less those under a
/// folded section and those an earlier control of the same label already
/// shows. A section's value is whether it is open.
fn visible_rows(controls: &[Control], values: &[ControlValue]) -> Vec<usize> {
    let mut open = true;
    let mut rows = Vec::new();
    for (index, control) in controls.iter().enumerate() {
        if let ControlKind::Section { .. } = control.kind {
            open = !matches!(values.get(index), Some(ControlValue::Bool(false)));
            rows.push(index);
        } else if open && linked(controls, index).first() == Some(&index) {
            rows.push(index);
        }
    }
    rows
}

/// The words a choice shows for an option. An option is the DSL the choice
/// writes, and neither a theme token path nor a qualified enum makes a good
/// name. The eight easings all start `theme.motion_ease_`, and a narrow
/// drop-down cuts each to that same prefix, so a theme token shows its own
/// name in words, its family prefix left off:
/// `theme.motion_ease_standard_decelerate` is "Standard decelerate". A
/// qualified enum shows its variant, as an enum the DSL takes bare already
/// shows: `ImageSliceEdge.Round` is "Round". The type is the same for every
/// option of one choice, so it says nothing the row's own label does not.
/// Any other option shows as written. What the choice writes is still the
/// option itself.
pub fn choice_label(option: &str) -> String {
    if let Some((ty, variant)) = option.split_once('.') {
        let name = |part: &str| {
            part.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && part.chars().all(|c| c.is_ascii_alphanumeric())
        };
        if name(ty) && name(variant) {
            return variant.to_string();
        }
    }
    let Some(token) = option.strip_prefix("theme.") else {
        return option.to_string();
    };
    let name = token.strip_prefix("motion_ease_").unwrap_or(token).replace('_', " ");
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => option.to_string(),
    }
}

pub fn prop_of(control: &Control) -> &'static str {
    match &control.kind {
        ControlKind::Bool { prop, .. }
        | ControlKind::Number { prop, .. }
        | ControlKind::Choice { prop, .. }
        | ControlKind::Text { prop, .. }
        | ControlKind::Color { prop, .. }
        | ControlKind::Curve { prop, .. } => prop,
        ControlKind::Disabled { .. } => "disabled",
        ControlKind::Section { .. } | ControlKind::Preset { .. } => "",
    }
}

pub fn default_of(control: &Control) -> ControlValue {
    match &control.kind {
        ControlKind::Bool { default, .. } => ControlValue::Bool(*default),
        ControlKind::Number { default, .. } => ControlValue::Number(*default),
        ControlKind::Choice { default, .. } => ControlValue::Choice(*default),
        ControlKind::Text { default, .. } => ControlValue::Text(default.to_string()),
        ControlKind::Color { default, .. } => ControlValue::Color(*default),
        ControlKind::Curve { default, .. } => ControlValue::Curve(default.to_vec()),
        ControlKind::Disabled { default } => ControlValue::Bool(*default),
        ControlKind::Section { open } => ControlValue::Bool(*open),
        ControlKind::Preset { default, .. } => ControlValue::Choice(*default),
    }
}

/// Parse `#RRGGBB`, `#RRGGBBAA`, with or without the `#`/`#x` prefix.
pub fn parse_color(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('#').trim_start_matches('x');
    let rgba = match t.len() {
        6 => u32::from_str_radix(t, 16).ok()? << 8 | 0xFF,
        8 => u32::from_str_radix(t, 16).ok()?,
        _ => return None,
    };
    Some(rgba)
}

fn color_to_vec4(c: u32) -> Vec4 {
    Vec4 {
        x: ((c >> 24) & 0xFF) as f32 / 255.0,
        y: ((c >> 16) & 0xFF) as f32 / 255.0,
        z: ((c >> 8) & 0xFF) as f32 / 255.0,
        w: (c & 0xFF) as f32 / 255.0,
    }
}

/// One write the story is asked for: the control it came from, the value
/// that control has now, and the property and chunk that carry it. The
/// disabled switch, a section and a preset carry no chunk.
pub struct Edit {
    pub control: &'static Control,
    pub value: ControlValue,
    pub write: Option<(String, String)>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ControlsPanel {
    #[deref]
    view: View,
    #[rust]
    controls: &'static [Control],
    #[rust]
    values: Vec<ControlValue>,
    /// Controls whose row widgets already carry their value. A row is filled
    /// once: filling on every draw would write the stored value back over
    /// whatever the user is typing or dragging.
    #[rust]
    synced: Vec<bool>,
    /// The control each list row shows, in order: every control less those
    /// under a folded section.
    #[rust]
    rows: Vec<usize>,
}

impl ControlsPanel {
    pub fn set_story(&mut self, cx: &mut Cx, story: &Story) {
        self.controls = story.controls;
        self.values = story.controls.iter().map(default_of).collect();
        self.refold(cx);
        self.view.label(cx, ids!(note)).set_visible(cx, self.controls.is_empty());
    }

    /// Every control back to its default. A section stays folded or open
    /// as the reader left it: that is how the panel is read, not a value
    /// on the story.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.values = self
            .controls
            .iter()
            .zip(self.values.iter())
            .map(|(control, value)| match control.kind {
                ControlKind::Section { .. } => value.clone(),
                _ => default_of(control),
            })
            .collect();
        self.refold(cx);
    }

    /// Work out the rows again and fill every one afresh: a row's widget
    /// may now show a different control than it did.
    fn refold(&mut self, cx: &mut Cx) {
        self.rows = visible_rows(self.controls, &self.values);
        self.synced = vec![false; self.controls.len()];
        self.view.redraw(cx);
    }

    pub fn values(&self) -> Vec<(&'static Control, ControlValue)> {
        self.controls.iter().zip(self.values.iter().cloned()).collect()
    }

    /// Set the control under `label` as if it had been moved by hand: its
    /// row shows the value and the edit goes out like any other. A label
    /// that names no control, or names a section, does nothing.
    pub fn set_by_label(&mut self, cx: &mut Cx, label: &str, value: ControlValue) {
        let Some(first) = self
            .controls
            .iter()
            .position(|c| c.label == label && !matches!(c.kind, ControlKind::Section { .. }))
        else {
            return;
        };
        if self.values.get(first) == Some(&value) {
            return;
        }
        for index in linked(self.controls, first) {
            self.values[index] = value.clone();
            if let Some(synced) = self.synced.get_mut(index) {
                *synced = false;
            }
            cx.widget_action(self.widget_uid(), ControlsAction::Changed { index, value: value.clone() });
        }
        self.view.redraw(cx);
    }

    fn template_for(kind: &ControlKind) -> LiveId {
        match kind {
            ControlKind::Bool { .. } => live_id!(RowBool),
            ControlKind::Number { .. } => live_id!(RowNumber),
            ControlKind::Choice { .. } | ControlKind::Preset { .. } => live_id!(RowChoice),
            ControlKind::Text { .. } => live_id!(RowText),
            ControlKind::Color { .. } => live_id!(RowColor),
            ControlKind::Curve { .. } => live_id!(RowCurve),
            ControlKind::Disabled { .. } => live_id!(RowDisabled),
            ControlKind::Section { .. } => live_id!(RowSection),
        }
    }

    fn fill_row(&self, cx: &mut Cx, item: &WidgetRef, control: &Control, value: &ControlValue) {
        item.label(cx, ids!(name)).set_text(cx, control.label);
        match (&control.kind, value) {
            (ControlKind::Bool { .. }, ControlValue::Bool(b))
            | (ControlKind::Disabled { .. }, ControlValue::Bool(b)) => {
                item.check_box(cx, ids!(value)).set_active(cx, *b, Animate::No);
            }
            (ControlKind::Number { min, max, step, .. }, ControlValue::Number(v)) => {
                let mut slider = item.widget(cx, ids!(value));
                let (min, max, step) = (*min, *max, *step);
                script_apply_eval!(cx, slider, {
                    min: #(min)
                    max: #(max)
                    step: #(step)
                });
                item.slider(cx, ids!(value)).set_value(cx, *v);
            }
            (ControlKind::Choice { options, .. }, ControlValue::Choice(i))
            | (ControlKind::Preset { options, .. }, ControlValue::Choice(i)) => {
                let dd = item.drop_down(cx, ids!(value));
                dd.set_labels(cx, options.iter().map(|o| choice_label(o)).collect());
                dd.set_selected_item(cx, *i);
            }
            (ControlKind::Text { .. }, ControlValue::Text(t)) => {
                item.widget(cx, ids!(value)).set_text(cx, t);
            }
            (ControlKind::Color { .. }, ControlValue::Color(c)) => {
                item.widget(cx, ids!(value)).set_text(cx, &format!("#{c:08X}"));
                let mut swatch = item.widget(cx, ids!(swatch));
                let color = color_to_vec4(*c);
                script_apply_eval!(cx, swatch, {
                    draw_bg +: {color: #(color)}
                });
            }
            (ControlKind::Curve { left, right, guide, guide_label, mirror, .. }, ControlValue::Curve(anchors)) => {
                // The dressing first: an apply that brings no `anchors`
                // leaves the editor's curve alone, and the curve goes in
                // after it. Below 0 is the editor's "no guide".
                let mut editor = item.widget(cx, ids!(value));
                let (left, right, mirror) = (*left, *right, *mirror);
                let (guide, guide_label) = match guide {
                    Some(y) => (*y, *guide_label),
                    None => (-1.0, ""),
                };
                script_apply_eval!(cx, editor, {
                    left_label: #(left)
                    right_label: #(right)
                    guide: #(guide)
                    guide_label: #(guide_label)
                    mirror: #(mirror)
                });
                item.curve_editor(cx, ids!(value)).set_anchors(cx, anchors);
            }
            (ControlKind::Section { .. }, ControlValue::Bool(open)) => {
                let mut arrow = item.widget(cx, ids!(arrow));
                let open = if *open { 1.0 } else { 0.0 };
                script_apply_eval!(cx, arrow, {
                    draw_bg +: {open: #(open)}
                });
            }
            _ => {}
        }
    }

    /// The controls a preset's option sets, by index, with their new values:
    /// every control of each label it names. A label that names no control,
    /// or names a section or another preset, is passed over.
    fn preset_changes(&self, index: usize, option: usize) -> Vec<(usize, ControlValue)> {
        let Some(ControlKind::Preset { values, .. }) = self.controls.get(index).map(|c| &c.kind) else {
            return Vec::new();
        };
        let controls = self.controls;
        values(option)
            .into_iter()
            .flat_map(|(label, value)| {
                (0..controls.len())
                    .filter(move |&i| links(&controls[i]) && controls[i].label == label)
                    .map(move |i| (i, value.clone()))
            })
            .collect()
    }
}

impl Widget for ControlsPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.rows.len());
                while let Some(row) = list.next_visible_item(cx) {
                    let Some(index) = self.rows.get(row).copied() else {
                        continue;
                    };
                    let controls: &'static [Control] = self.controls;
                    let control = &controls[index];
                    let value = self.values[index].clone();
                    let (item, existed) = list.item_with_existed(cx, row, Self::template_for(&control.kind));
                    if !existed || !self.synced.get(index).copied().unwrap_or(true) {
                        self.fill_row(cx, &item, control, &value);
                        self.synced[index] = true;
                    }
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            // The rows are rebuilt from their templates; fill them again.
            self.synced = vec![false; self.controls.len()];
        }
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let list = self.view.portal_list(cx, ids!(list));
        let mut changes: Vec<(usize, ControlValue)> = Vec::new();
        let mut folded = false;
        for (row, item) in list.items_with_actions(actions) {
            let Some(index) = self.rows.get(row).copied() else {
                continue;
            };
            let controls: &'static [Control] = self.controls;
            let control = &controls[index];
            let value = match &control.kind {
                ControlKind::Bool { .. } | ControlKind::Disabled { .. } => {
                    item.check_box(cx, ids!(value)).changed(actions).map(ControlValue::Bool)
                }
                ControlKind::Number { .. } => {
                    item.slider(cx, ids!(value)).slided(actions).map(ControlValue::Number)
                }
                ControlKind::Choice { .. } => {
                    item.drop_down(cx, ids!(value)).selected(actions).map(ControlValue::Choice)
                }
                ControlKind::Text { .. } => item
                    .text_input(cx, ids!(value))
                    .returned(actions)
                    .map(|(t, _)| ControlValue::Text(t)),
                ControlKind::Color { .. } => item
                    .text_input(cx, ids!(value))
                    .returned(actions)
                    .and_then(|(t, _)| parse_color(&t))
                    .map(ControlValue::Color),
                ControlKind::Curve { .. } => {
                    // A drag sends every step, and its release a commit that
                    // is most often the curve the last step sent: that one
                    // is not sent twice. The row keeps what the editor
                    // shows; only a value from outside fills it again.
                    let editor = item.curve_editor(cx, ids!(value));
                    editor
                        .changed(actions)
                        .or_else(|| editor.committed(actions))
                        .map(ControlValue::Curve)
                        .filter(|curve| self.values.get(index) != Some(curve))
                }
                ControlKind::Section { .. } => {
                    // A press the list took away as a scroll folds nothing.
                    if item
                        .view(cx, ids!(head))
                        .finger_up(actions)
                        .is_some_and(|e| e.is_over && !e.cancelled)
                    {
                        let open = matches!(self.values[index], ControlValue::Bool(true));
                        self.values[index] = ControlValue::Bool(!open);
                        folded = true;
                    }
                    None
                }
                ControlKind::Preset { .. } => {
                    let picked = item.drop_down(cx, ids!(value)).selected(actions);
                    if let Some(option) = picked {
                        for (target, value) in self.preset_changes(index, option) {
                            // Its row, if it is showing, takes the new value.
                            self.synced[target] = false;
                            changes.push((target, value));
                        }
                        self.view.redraw(cx);
                    }
                    picked.map(ControlValue::Choice)
                }
            };
            if let Some(value) = value {
                for index in linked(controls, index) {
                    changes.push((index, value.clone()));
                }
            }
        }
        if folded {
            self.refold(cx);
        }
        for (index, value) in changes {
            self.values[index] = value.clone();
            cx.widget_action(self.widget_uid(), ControlsAction::Changed { index, value });
        }
    }
}

impl ControlsPanelRef {
    pub fn set_by_label(&self, cx: &mut Cx, label: &str, value: ControlValue) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_by_label(cx, label, value);
        }
    }

    pub fn set_story(&self, cx: &mut Cx, story: &Story) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_story(cx, story);
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    /// The writes the edits raised this pass ask of the story, in order.
    /// Written against the values the panel holds once they are all in,
    /// and one per target and property: a preset moves four lanes of one
    /// vector and the vector is written once, with all four.
    pub fn edits(&self, actions: &Actions) -> Vec<Edit> {
        let mut out: Vec<Edit> = Vec::new();
        let Some(inner) = self.borrow() else {
            return out;
        };
        for action in actions.filter_widget_actions(self.widget_uid()) {
            let ControlsAction::Changed { index, value } = action.cast() else {
                continue;
            };
            let controls: &'static [Control] = inner.controls;
            let Some(control) = controls.get(index) else {
                continue;
            };
            let value = inner.values.get(index).cloned().unwrap_or(value);
            let write = write_for(inner.controls, &inner.values, index);
            out.retain(|earlier| {
                let same_write = match (&earlier.write, &write) {
                    (Some((a, _)), Some((b, _))) => earlier.control.target == control.target && a == b,
                    _ => false,
                };
                !(same_write || std::ptr::eq(earlier.control, control))
            });
            out.push(Edit { control, value, write });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_render_each_kind() {
        let text = Control { label: "Label", target: "", kind: ControlKind::Text { prop: "text", default: "" } };
        assert_eq!(
            chunk_for(&text, &ControlValue::Text("say \"hi\"".into())).unwrap(),
            "{text: \"say \\\"hi\\\"\"}"
        );
        let num = Control {
            label: "Radius",
            target: "",
            kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 8., step: 0.5, default: 2. },
        };
        assert_eq!(chunk_for(&num, &ControlValue::Number(4.0)).unwrap(), "{draw_bg.border_radius: 4.0}");
        let color = Control { label: "Fill", target: "", kind: ControlKind::Color { prop: "draw_bg.color", default: 0 } };
        assert_eq!(chunk_for(&color, &ControlValue::Color(0xFF5C39FF)).unwrap(), "{draw_bg.color: #xFF5C39FF}");
        let choice = Control {
            label: "Flow",
            target: "",
            kind: ControlKind::Choice { prop: "flow", options: &["Right", "Down"], default: 0 },
        };
        assert_eq!(chunk_for(&choice, &ControlValue::Choice(1)).unwrap(), "{flow: Down}");
        let face = Control {
            label: "Face",
            target: "",
            kind: ControlKind::Color { prop: "draw_bg.color draw_bg.color_hover", default: 0 },
        };
        assert_eq!(
            chunk_for(&face, &ControlValue::Color(0x0E1013FF)).unwrap(),
            "{draw_bg.color: #x0E1013FF draw_bg.color_hover: #x0E1013FF}"
        );
        let curve = profile("Profile", "prof");
        let anchors = vec![[0.0, 1.0, 1.0], [0.5, 0.5, 2.0], [1.0, 0.0, 1.0]];
        assert_eq!(
            chunk_for(&curve, &ControlValue::Curve(anchors.clone())).unwrap(),
            "{prof: [[0.0, 1.0, 1.0], [0.5, 0.5, 2.0], [1.0, 0.0, 1.0]]}"
        );
        // A curve is no lane: it writes itself, whole.
        let written = write_for(std::slice::from_ref(&curve), &[ControlValue::Curve(anchors)], 0);
        assert_eq!(
            written,
            Some(("prof".to_string(), "{prof: [[0.0, 1.0, 1.0], [0.5, 0.5, 2.0], [1.0, 0.0, 1.0]]}".to_string()))
        );
        assert_eq!(default_of(&curve), ControlValue::Curve(LINE.to_vec()));
        // Neither a curve for another kind nor another kind for a curve.
        assert_eq!(chunk_for(&curve, &ControlValue::Number(1.0)), None);
        assert_eq!(chunk_for(&num, &ControlValue::Curve(LINE.to_vec())), None);
    }

    const LINE: [[f64; 3]; 2] = [[0.0, 1.0, 1.0], [1.0, 0.0, 1.0]];

    /// A curve control on the subject that opens on [`LINE`].
    const fn profile(label: &'static str, prop: &'static str) -> Control {
        Control {
            label,
            target: "",
            kind: ControlKind::Curve {
                prop,
                default: &LINE,
                left: "CENTRE",
                right: "SKIRT RIM",
                guide: Some(0.5),
                guide_label: "CAP TOP",
                mirror: true,
            },
        }
    }

    /// A panel built from its template, as the app builds it.
    fn panel(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            super::script_mod(vm);
            let storybook = vm.module(id!(storybook));
            let value = vm.bx.heap.value(
                storybook,
                id!(ControlsPanel).into(),
                crate::makepad_widgets::makepad_script::trap::NoTrap,
            );
            assert!(value.as_object().is_some(), "no ControlsPanel template");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// A curve row builds from its template with no error, and filling it
    /// names it and hands its editor the curve.
    #[test]
    fn a_curve_row_fills_its_editor() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let panel = panel(&mut cx);
        let control = profile("Profile", "prof");
        let anchors = vec![[0.0, 1.0, 2.0], [0.3, 0.8, 1.0], [0.6, 0.4, 3.0], [1.0, 0.0, 0.0]];
        cx.with_vm(|vm| vm.bx.captured_errors = Some(Vec::new()));
        let row = panel.portal_list(&mut cx, ids!(list)).item(&mut cx, 0, live_id!(RowCurve));
        assert!(!row.is_empty(), "no RowCurve template");
        let inner = panel.borrow::<ControlsPanel>().expect("a ControlsPanel");
        inner.fill_row(&mut cx, &row, &control, &ControlValue::Curve(anchors.clone()));
        drop(inner);
        let errors = cx.with_vm(|vm| vm.take_errors());
        assert!(errors.is_empty(), "the curve row did not build and fill cleanly: {errors:?}");
        assert_eq!(row.label(&cx, ids!(name)).text(), "Profile");
        assert_eq!(row.curve_editor(&cx, ids!(value)).anchors(), anchors);
    }

    /// A curve moves by label and by preset like any other control: every
    /// control of its label takes the curve, and each writes it.
    #[test]
    fn a_curve_moves_by_label_and_by_preset() {
        static CONTROLS: [Control; 4] = [
            Control {
                label: "Shape",
                target: "",
                kind: ControlKind::Preset { options: &["Line", "Bend"], default: 0, values: bend },
            },
            profile("Profile", "prof"),
            Control { label: "Depth", target: "", kind: ControlKind::Number { prop: "depth", min: 0., max: 1., step: 0.1, default: 0. } },
            profile("Profile", "prof_too"),
        ];
        fn bend(option: usize) -> Vec<(&'static str, ControlValue)> {
            let curve = if option == 1 { vec![[0.0, 1.0, 1.0], [0.4, 0.9, 2.0], [1.0, 0.0, 1.0]] } else { LINE.to_vec() };
            vec![("Profile", ControlValue::Curve(curve)), ("Depth", ControlValue::Number(option as f64))]
        }
        let story = Story {
            key: "a/b/c",
            category: "A",
            component: "B",
            also: &[],
            name: "C",
            dsl: "X",
            added: "2026-09-27",
            tags: &[],
            doc: "",
            subject: "",
            feature: None,
            controls: &CONTROLS,
            on_actions: None,
        };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let panel = panel(&mut cx);
        let mut inner = panel.borrow_mut::<ControlsPanel>().expect("a ControlsPanel");
        inner.set_story(&mut cx, &story);
        assert_eq!(visible_rows(&CONTROLS, &inner.values), vec![0, 1, 2], "two curves of one label are one row");

        let bent = bend(1)[0].1.clone();
        let changes = inner.preset_changes(0, 1);
        assert_eq!(changes, vec![(1, bent.clone()), (3, bent), (2, ControlValue::Number(1.0))]);

        let curve = vec![[0.0, 0.5, 0.0], [1.0, 0.5, 0.0]];
        inner.set_by_label(&mut cx, "Profile", ControlValue::Curve(curve.clone()));
        assert_eq!(inner.values[1], ControlValue::Curve(curve.clone()));
        assert_eq!(inner.values[3], ControlValue::Curve(curve));
        assert!(!inner.synced[1] && !inner.synced[3], "a curve set from outside fills its row again");
        let writes: Vec<String> = [1, 3].iter().filter_map(|&i| write_for(&CONTROLS, &inner.values, i)).map(|(_, c)| c).collect();
        assert_eq!(writes, vec!["{prof: [[0.0, 0.5, 0.0], [1.0, 0.5, 0.0]]}", "{prof_too: [[0.0, 0.5, 0.0], [1.0, 0.5, 0.0]]}"]);
    }

    /// Controls that share a label show as one row, the first, and each is
    /// linked to all of them; a section never links, whatever it is called.
    #[test]
    fn a_shared_label_is_one_row() {
        const fn ground(target: &'static str) -> Control {
            Control { label: "Ground", target, kind: ControlKind::Color { prop: "draw_bg.color", default: 0 } }
        }
        let controls = [
            Control { label: "Ground", target: "", kind: ControlKind::Section { open: true } },
            ground("stage"),
            Control { label: "Ink", target: "a", kind: ControlKind::Color { prop: "draw_text.color", default: 0 } },
            ground("a b"),
        ];
        let values: Vec<ControlValue> = controls.iter().map(default_of).collect();
        assert_eq!(visible_rows(&controls, &values), vec![0, 1, 2]);
        assert_eq!(linked(&controls, 3), vec![1, 3]);
        assert_eq!(linked(&controls, 0), vec![0]);
        assert_eq!(linked(&controls, 2), vec![2]);
    }

    /// Controls that share a label are one control, so they must be one
    /// kind with one default, or the row would show one value and write
    /// another.
    #[test]
    fn shared_labels_agree() {
        for story in crate::registry::all() {
            for (index, control) in story.controls.iter().enumerate() {
                for other in linked(story.controls, index) {
                    let other = &story.controls[other];
                    assert_eq!(
                        default_of(control),
                        default_of(other),
                        "{} / {}: two controls of one label start apart",
                        story.key,
                        control.label
                    );
                    assert_eq!(
                        std::mem::discriminant(&control.kind),
                        std::mem::discriminant(&other.kind),
                        "{} / {}: two controls of one label are different kinds",
                        story.key,
                        control.label
                    );
                }
            }
        }
    }

    /// The theme's easings read as words, each different from the rest, a
    /// qualified enum reads as its variant, and every other option reads as
    /// written.
    #[test]
    fn theme_tokens_read_as_words() {
        let eases = [
            ("theme.motion_ease_standard", "Standard"),
            ("theme.motion_ease_standard_decelerate", "Standard decelerate"),
            ("theme.motion_ease_standard_accelerate", "Standard accelerate"),
            ("theme.motion_ease_emphasized_decelerate", "Emphasized decelerate"),
            ("theme.motion_ease_emphasized_accelerate", "Emphasized accelerate"),
            ("theme.motion_ease_linear", "Linear"),
            ("theme.motion_ease_spring", "Spring"),
            ("theme.motion_ease_bounce", "Bounce"),
        ];
        for (option, words) in eases {
            assert_eq!(choice_label(option), words);
        }
        for (option, bare) in [
            ("RadialLook.Frosted", "Frosted"),
            ("ImageSliceEdge.Round", "Round"),
            ("ImageSliceCenter.Tile", "Tile"),
            ("ImageSliceUnits.DevicePixels", "DevicePixels"),
            ("ImageFit.Slice", "Slice"),
        ] {
            assert_eq!(choice_label(option), bare);
        }
        for option in ["Right", "BottomCenter", "", "0.5", "Ease.OutElastic2x", "a.B", "Ab.c"] {
            let shown = choice_label(option);
            if option == "Ease.OutElastic2x" {
                assert_eq!(shown, "OutElastic2x");
            } else {
                assert_eq!(shown, option);
            }
        }
    }

    /// Shown shorter than it is written, no option of any story's choice may
    /// read the same as another option of that choice, or the drop-down
    /// would offer two rows nobody can tell apart.
    #[test]
    fn every_choice_shows_its_options_apart() {
        let mut choices = 0;
        for story in crate::registry::all() {
            for control in story.controls.iter() {
                let ControlKind::Choice { options, .. } = &control.kind else {
                    continue;
                };
                choices += 1;
                let mut shown: Vec<String> = options.iter().map(|option| choice_label(option)).collect();
                shown.sort();
                let before = shown.len();
                shown.dedup();
                assert_eq!(shown.len(), before, "{} / {}: two options show as one: {options:?}", story.key, control.label);
            }
        }
        assert!(choices > 20, "the registry lost its choices ({choices})");
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#FF5C39"), Some(0xFF5C39FF));
        assert_eq!(parse_color("#xFF5C3980"), Some(0xFF5C3980));
        assert_eq!(parse_color("ff5c39"), Some(0xFF5C39FF));
        assert_eq!(parse_color("nope"), None);
    }
}
