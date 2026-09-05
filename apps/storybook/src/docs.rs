//! The docs panel: the story's note in Markdown, then every property of the
//! story's subject with its live value and its `/** */` annotation.
//!
//! The table comes from the library's reflection surface, the same reads the
//! design overlay makes, so the two never disagree. Properties the story's
//! own DSL set come first; the rest are inherited from the widget's template
//! and hidden until asked for.
use crate::makepad_widgets::reflect::{collect_row_docs, reflect_flat};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let PropRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        padding: Inset{top: 2. bottom: 2. left: 0. right: 0.}
        name := Label{width: 150. text: ""}
        value := Label{width: 90. text: ""}
        doc := Label{width: Fill text: ""}
    }

    mod.widgets.DocsPanelBase = #(DocsPanel::register_widget(vm))
    mod.widgets.DocsPanel = set_type_default() do mod.widgets.DocsPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        doc := Markdown{
            width: Fill
            height: Fit
            body: ""
        }
        added := Label{text: ""}
        props_head := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{x: 0. y: 0.5}
            props_title := H4{text: "Properties"}
            Filler{}
            inherited := CheckBox{text: "inherited"}
        }
        props := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Row := PropRow{}
        }
    }
}

#[derive(Clone)]
struct Prop {
    name: String,
    value: String,
    doc: String,
    is_set: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct DocsPanel {
    #[deref]
    view: View,
    #[rust]
    props: Vec<Prop>,
    #[rust]
    show_inherited: bool,
    /// The subject the table was read from, so it is read again only when
    /// the story is rebuilt.
    #[rust]
    subject: Option<WidgetUid>,
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
        self.props.clear();
        self.subject = None;
        self.view.redraw(cx);
    }

    /// Read the table from this widget, unless it is the one already read.
    pub fn set_subject(&mut self, cx: &mut Cx, subject: Option<&WidgetRef>) {
        let uid = subject.map(|w| w.widget_uid());
        if uid == self.subject {
            return;
        }
        self.subject = uid;
        self.props.clear();
        if let Some(widget) = subject {
            let docs = collect_row_docs(cx, widget);
            let mut props: Vec<Prop> = reflect_flat(cx, widget)
                .into_iter()
                .map(|(name, value, is_set)| Prop {
                    doc: docs.get(&name).cloned().unwrap_or_default(),
                    name,
                    value,
                    is_set,
                })
                .collect();
            props.sort_by(|a, b| b.is_set.cmp(&a.is_set).then(a.name.cmp(&b.name)));
            self.props = props;
        }
        let set = self.props.iter().filter(|p| p.is_set).count();
        let title = if self.props.is_empty() {
            "Properties".to_string()
        } else {
            format!("Properties ({} set, {} inherited)", set, self.props.len() - set)
        };
        self.view.label(cx, ids!(props_title)).set_text(cx, &title);
        self.view.redraw(cx);
    }

    fn visible(&self) -> Vec<&Prop> {
        self.props.iter().filter(|p| p.is_set || self.show_inherited).collect()
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

impl Widget for DocsPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                let rows = self.visible();
                list.set_item_range(cx, 0, rows.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    let Some(prop) = rows.get(item_id) else {
                        continue;
                    };
                    let item = list.item(cx, item_id, live_id!(Row));
                    item.label(cx, ids!(name)).set_text(cx, &prop.name);
                    item.label(cx, ids!(value)).set_text(cx, &clip(&prop.value, 24));
                    item.label(cx, ids!(doc)).set_text(cx, &clip(&prop.doc, 90));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            self.subject = None;
            self.props.clear();
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(on) = self.view.check_box(cx, ids!(inherited)).changed(actions) {
                self.show_inherited = on;
                self.view.redraw(cx);
            }
        }
    }
}

impl DocsPanelRef {
    pub fn set_story(&self, cx: &mut Cx, story: &Story) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_story(cx, story);
        }
    }

    pub fn set_subject(&self, cx: &mut Cx, subject: Option<&WidgetRef>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_subject(cx, subject);
        }
    }
}
