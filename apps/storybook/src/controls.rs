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
}

/// One edit the panel raised: which control changed and to what.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ControlsAction {
    Changed { index: usize, value: ControlValue },
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
        (ControlKind::Color { prop, .. }, ControlValue::Color(c)) => Some(format!("{{{prop}: #x{c:08X}}}")),
        _ => None,
    }
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
/// folded section. A section's value is whether it is open.
fn visible_rows(controls: &[Control], values: &[ControlValue]) -> Vec<usize> {
    let mut open = true;
    let mut rows = Vec::new();
    for (index, control) in controls.iter().enumerate() {
        if let ControlKind::Section { .. } = control.kind {
            open = !matches!(values.get(index), Some(ControlValue::Bool(false)));
            rows.push(index);
        } else if open {
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
        | ControlKind::Color { prop, .. } => prop,
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

    fn template_for(kind: &ControlKind) -> LiveId {
        match kind {
            ControlKind::Bool { .. } => live_id!(RowBool),
            ControlKind::Number { .. } => live_id!(RowNumber),
            ControlKind::Choice { .. } | ControlKind::Preset { .. } => live_id!(RowChoice),
            ControlKind::Text { .. } => live_id!(RowText),
            ControlKind::Color { .. } => live_id!(RowColor),
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

    /// The controls a preset's option sets, by index, with their new values.
    /// A label that names no control, or names a section or another preset,
    /// is passed over.
    fn preset_changes(&self, index: usize, option: usize) -> Vec<(usize, ControlValue)> {
        let Some(ControlKind::Preset { values, .. }) = self.controls.get(index).map(|c| &c.kind) else {
            return Vec::new();
        };
        values(option)
            .into_iter()
            .filter_map(|(label, value)| {
                let target = self.controls.iter().position(|c| {
                    c.label == label && !matches!(c.kind, ControlKind::Section { .. } | ControlKind::Preset { .. })
                })?;
                Some((target, value))
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
                changes.push((index, value));
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
