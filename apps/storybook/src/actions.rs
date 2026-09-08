//! The actions panel: what the story raised, newest at the bottom.
use crate::makepad_widgets::*;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.storybook.ActionsPanelBase = #(ActionsPanel::register_widget(vm))
    mod.storybook.ActionsPanel = set_type_default() do mod.storybook.ActionsPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        header := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            count := Label{text: "Nothing raised yet."}
            Filler{}
            clear := ButtonFlat{text: "Clear"}
        }
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Row := Label{
                width: Fill
                height: Fit
                draw_text +: {
                    text_style: theme.font_code{font_size: 8.5}
                }
                text: ""
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ActionsPanel {
    #[deref]
    view: View,
    #[rust]
    lines: VecDeque<String>,
}

impl ActionsPanel {
    pub fn push(&mut self, cx: &mut Cx, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        for line in lines {
            if self.lines.len() >= crate::canvas::LOG_CAPACITY {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
        self.refresh(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.lines.clear();
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let text = match self.lines.len() {
            0 => "Nothing raised yet.".to_string(),
            1 => "1 action".to_string(),
            n => format!("{n} actions"),
        };
        self.view.label(cx, ids!(count)).set_text(cx, &text);
        self.view.redraw(cx);
    }
}

impl Widget for ActionsPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.lines.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    let Some(line) = self.lines.get(item_id) else {
                        continue;
                    };
                    let item = list.item(cx, item_id, live_id!(Row));
                    item.set_text(cx, line);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(clear)).clicked(actions) {
                self.clear(cx);
            }
        }
    }
}

impl ActionsPanelRef {
    pub fn push(&self, cx: &mut Cx, lines: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push(cx, lines);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }
}
