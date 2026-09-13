//! The controls panel: the story's declared controls as live editors.
//!
//! A story names the properties a reader may drive and what kind of value
//! each takes; the panel draws one row per control with the stock widget
//! for that kind and, on every change, hands the app a script chunk
//! (`{ prop: value }`) for the canvas to apply to the story's subject. The
//! values live here, so a rebuilt story gets them again.
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

#[derive(Script, ScriptHook, Widget)]
pub struct ControlsPanel {
    #[deref]
    view: View,
    #[rust]
    controls: &'static [Control],
    #[rust]
    values: Vec<ControlValue>,
    /// Rows whose widgets already carry their value. A row is filled once:
    /// filling on every draw would write the stored value back over whatever
    /// the user is typing or dragging.
    #[rust]
    synced: Vec<bool>,
}

impl ControlsPanel {
    pub fn set_story(&mut self, cx: &mut Cx, story: &Story) {
        self.controls = story.controls;
        self.values = story.controls.iter().map(default_of).collect();
        self.synced = vec![false; story.controls.len()];
        self.view.label(cx, ids!(note)).set_visible(cx, self.controls.is_empty());
        self.view.redraw(cx);
    }

    pub fn reset(&mut self, cx: &mut Cx) {
        self.values = self.controls.iter().map(default_of).collect();
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
            ControlKind::Choice { .. } => live_id!(RowChoice),
            ControlKind::Text { .. } => live_id!(RowText),
            ControlKind::Color { .. } => live_id!(RowColor),
            ControlKind::Disabled { .. } => live_id!(RowDisabled),
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
            (ControlKind::Choice { options, .. }, ControlValue::Choice(i)) => {
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
            _ => {}
        }
    }
}

impl Widget for ControlsPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.controls.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    let Some(control) = self.controls.get(item_id) else {
                        continue;
                    };
                    let value = self.values[item_id].clone();
                    let item = list.item(cx, item_id, Self::template_for(&control.kind));
                    if !self.synced.get(item_id).copied().unwrap_or(true) {
                        self.fill_row(cx, &item, control, &value);
                        self.synced[item_id] = true;
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
        for (item_id, item) in list.items_with_actions(actions) {
            let Some(control) = self.controls.get(item_id) else {
                continue;
            };
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
            };
            if let Some(value) = value {
                changes.push((item_id, value));
            }
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

    /// The edits raised this pass: (control, value).
    pub fn changed(&self, actions: &Actions) -> Vec<(&'static Control, ControlValue)> {
        let mut out = Vec::new();
        let Some(inner) = self.borrow() else {
            return out;
        };
        for action in actions.filter_widget_actions(self.widget_uid()) {
            if let ControlsAction::Changed { index, value } = action.cast() {
                if let Some(control) = inner.controls.get(index) {
                    out.push((control, value));
                }
            }
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
