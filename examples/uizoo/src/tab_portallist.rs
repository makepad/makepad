use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Post = View{
        width: Fill height: Fit
        padding: Inset{top: 10. bottom: 10.}

        body := RoundedView{
            width: Fill height: Fit
            content := View{
                width: Fill height: Fit
                text := P{text: ""}
            }
        }
    }

    mod.widgets.NewsFeedBase = #(NewsFeed::register_widget(vm))

    mod.widgets.NewsFeed = set_type_default() do mod.widgets.NewsFeedBase{
        list := PortalList{
            scroll_bar: ScrollBar{}
            TopSpace := View{height: 0.}
            BottomSpace := View{height: 100.}

            Post := CachedView{
                flow: Down
                Post{}
                Hr{}
            }
        }
    }

    mod.widgets.DemoPortalList = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# PortalList\n\nPortalList renders large lists efficiently."}
        }
        demos +: {
            news_feed := mod.widgets.NewsFeed{}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct NewsFeed {
    #[uid]
    uid: WidgetUid,
    #[deref]
    view: View,
}

impl Widget for NewsFeed {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, 1000);
                while let Some(item_id) = list.next_visible_item(cx) {
                    let template = match item_id {
                        0 => live_id!(TopSpace),
                        _ => live_id!(Post),
                    };
                    let item = list.item(cx, item_id, template);
                    let text = match item_id % 4 {
                        1 => format!("At vero eos et accusam et justo duo dolores et ea rebum. Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet. Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua."),
                        2 => format!("How are you?"),
                        3 => format!("Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet."),
                        _ => format!("Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua."),
                    };
                    item.label(cx, ids!(content.text)).set_text(cx, &text);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}
