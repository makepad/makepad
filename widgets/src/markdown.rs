use crate::{
    link_label::LinkLabel, makepad_derive_widget::*, makepad_draw::*, text_flow::TextFlow,
    theme_desktop_light, widget::*, Html, WidgetMatchEvent,
};

use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

live_design! {
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use crate::link_label::LinkLabelBase;
    use crate::html::Html

    pub MarkdownBase = {{Markdown}} <Html> {
    }

    pub Markdown = <MarkdownBase> {
    }

}

#[derive(Live, LiveHook, Widget)]
pub struct Markdown {
    #[deref]
    inner_html: Html,
    #[rust]
    md_text: String,
}

impl Widget for Markdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.inner_html.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.inner_html.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.md_text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.md_text = v.to_string();

        let html_text = self.parse_md_to_html(&self.md_text);
        self.inner_html.set_text(cx, &html_text);

        self.redraw(cx);
    }
}

impl MarkdownRef {
    pub fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.set_text(cx, v)
    }
}

impl Markdown {
    fn parse_md_to_html(&self, md_text: &str) -> String {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        let parser = Parser::new_ext(&md_text, options);

        let mut html_output = String::new();
        pulldown_cmark::html::push_html(&mut html_output, parser);

        html_output
    }
}
