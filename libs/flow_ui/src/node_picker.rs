//! A host-supplied node catalog. A face widget is never an insertion requirement.

use makepad_flow::PortType;
use makepad_widgets::*;

#[derive(Clone, Debug, PartialEq)]
pub struct NodeChoice {
    pub type_name: String,
    pub description: String,
    pub inputs: Vec<PortType>,
}

#[derive(Clone, Debug, Default)]
pub enum NodePickerAction {
    #[default]
    None,
    Arm(String),
    Choose(String),
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FlowNodePicker = set_type_default() do #(FlowNodePicker::register_widget(vm)){
        width: Fill height: Fill flow: Down spacing: 6
        search := TextInput{width: Fill height: 28 empty_text: "Find a node…"}
        hint := Label{width: Fill height: Fit text: "Drag into the graph, or click to add."
            draw_text +: {color: theme.color_on_surface_variant text_style: theme.font_regular{font_size: 9}}}
        empty := Label{width: Fill height: Fit visible: false text: "No matching nodes"}
        list := PortalList{
            width: Fill height: Fill scroll_bar: ScrollBar{}
            Item := View{width: Fill height: 66 padding: Inset{bottom: 4}
                card := RoundedView{width: Fill height: Fill flow: Down padding: 7 spacing: 2
                    cursor: MouseCursor.Hand
                    draw_bg +: {color: theme.color_surface_container border_radius: 5.0}
                    name := Label{width: Fill height: Fit padding: 0
                        draw_text +: {color: theme.color_on_surface text_style: theme.font_bold{font_size: 10}}}
                    description := Label{width: Fill height: Fit padding: 0
                        draw_text +: {color: theme.color_on_surface_variant text_style: theme.font_regular{font_size: 9}}}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FlowNodePicker {
    #[deref] view: View,
    #[rust] choices: Vec<NodeChoice>,
    #[rust] rows: Vec<usize>,
    #[rust] query: String,
    #[rust] source_type: Option<PortType>,
}

impl FlowNodePicker {
    pub fn set_choices(&mut self, cx: &mut Cx, mut choices: Vec<NodeChoice>) {
        choices.sort_by(|a, b| a.type_name.cmp(&b.type_name));
        if self.choices == choices { return; }
        self.choices = choices;
        self.refilter(cx);
    }

    pub fn set_source_type(&mut self, cx: &mut Cx, source: Option<PortType>) {
        if self.source_type == source { return; }
        self.source_type = source;
        self.query.clear();
        self.view.text_input(cx, ids!(search)).set_text(cx, "");
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Cx) {
        let query = self.query.trim().to_lowercase();
        self.rows = self.choices.iter().enumerate().filter(|(_, choice)| {
            self.source_type.as_ref().is_none_or(|ty| choice.inputs.contains(ty))
                && (query.is_empty() || choice.type_name.to_lowercase().contains(&query)
                    || choice.description.to_lowercase().contains(&query))
        }).map(|(index, _)| index).collect();
        self.view.label(cx, ids!(empty)).set_visible(cx, self.rows.is_empty());
        self.view.label(cx, ids!(hint)).set_text(cx, if self.source_type.is_some() {
            "Choose a node to connect this wire."
        } else { "Drag into the graph, or click to add." });
        self.view.redraw(cx);
    }
}

impl Widget for FlowNodePicker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(query) = self.view.text_input(cx, ids!(search)).changed(actions) {
                self.query = query;
                self.refilter(cx);
            }
            let list = self.view.portal_list(cx, ids!(list));
            for (row, item) in list.items_with_actions(actions) {
                let Some(choice) = self.rows.get(row).and_then(|index| self.choices.get(*index)) else { continue };
                let card = item.view(cx, ids!(card));
                if self.source_type.is_none() && card.finger_down(actions).is_some() {
                    cx.widget_action(self.widget_uid(), NodePickerAction::Arm(choice.type_name.clone()));
                }
                if card.finger_up(actions).is_some_and(|up| up.is_over) {
                    cx.widget_action(self.widget_uid(), NodePickerAction::Choose(choice.type_name.clone()));
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list = step.as_portal_list();
            let Some(mut list) = list.borrow_mut() else { continue };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(row) = list.next_visible_item(cx) {
                let Some(choice) = self.rows.get(row).and_then(|index| self.choices.get(*index)) else { continue };
                let item = list.item(cx, row, live_id!(Item));
                item.label(cx, ids!(name)).set_text(cx, &choice.type_name);
                let description = choice.description.lines().next().unwrap_or("");
                let mut summary: String = description.chars().take(44).collect();
                if description.chars().count() > 44 { summary.push('…'); }
                item.label(cx, ids!(description)).set_text(cx, &summary);
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }
}
