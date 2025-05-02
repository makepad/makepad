use makepad_widgets::*;

live_design! {
    use link::widgets::*;

    PLACEHOLDER = dep("crate://self/resources/placeholder.jpg");

    ImageItem = <View> {
        width: 256,
        height: 256,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
    }

    ImageRow = {{ImageRow}} {
        <PortalList> {
            height: 256,
            flow: Right,

            ImageItem = <ImageItem> {}
        }
    }

    ImageGrid = {{ImageGrid}} {
        <PortalList> {
            flow: Down,

            ImageRow = <ImageRow> {}
        }
    }

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {
                    <ImageGrid> {}
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ImageGrid {
    #[deref]
    view: View,
}

impl Widget for ImageGrid {
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {                
                list.set_item_range(cx, 0, 3);
                while let Some(row_idx) = list.next_visible_item(cx) {
                    let row = list.item(cx, row_idx, live_id!(ImageRow));
                    row.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ImageRow {
    #[deref]
    view: View,
}

impl Widget for ImageRow {
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 4);
                while let Some(item_idx) = list.next_visible_item(cx) {
                    let item = list.item(cx, item_idx, live_id!(ImageItem));
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

app_main!(App);
