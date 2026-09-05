//! The docs panel: the story's note, in Markdown.
//!
//! The property table (every property of the story's subject with its value
//! and its annotation) joins once the library exposes its reflection
//! surface; until then the note is the documentation.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DocsPanelBase = #(DocsPanel::register_widget(vm))
    mod.widgets.DocsPanel = set_type_default() do mod.widgets.DocsPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        scroll_bars: ScrollBars{
            show_scroll_x: false
            show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }
        doc := Markdown{
            width: Fill
            height: Fit
            body: ""
        }
        added := Label{text: ""}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DocsPanel {
    #[deref]
    view: View,
}

impl DocsPanel {
    pub fn set_story(&mut self, cx: &mut Cx, story: &Story) {
        self.view.markdown(cx, ids!(doc)).set_text(cx, story.doc);
        let tags = if story.tags.is_empty() {
            String::new()
        } else {
            format!(" · {}", story.tags.join(", "))
        };
        self.view
            .label(cx, ids!(added))
            .set_text(cx, &format!("Added {}{}", story.added, tags));
        self.view.redraw(cx);
    }
}

impl Widget for DocsPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}

impl DocsPanelRef {
    pub fn set_story(&self, cx: &mut Cx, story: &Story) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_story(cx, story);
        }
    }
}
