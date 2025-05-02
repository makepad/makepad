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
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let mut scope = Scope::with_data(&mut self.state);
        self.ui.handle_event(cx, event, &mut scope);
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

#[derive(Debug)]
pub struct State {
    num_images: usize,
    max_images_per_row: usize,
}

impl State {
    fn num_images(&self) -> usize {
        self.num_images
    }

    fn num_rows(&self) -> usize {
        self.num_images().div_ceil(self.max_images_per_row)
    }

    fn first_image_idx_for_row(&self, row_idx: usize) -> usize {
        row_idx * self.max_images_per_row
    }

    fn num_images_for_row(&self, row_idx: usize) -> usize {
        let first_image_idx = self.first_image_idx_for_row(row_idx);
        let num_remaining_images = self.num_images() - first_image_idx;
        self.max_images_per_row.min(num_remaining_images)
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            num_images: 7,
            max_images_per_row: 4,
        }
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
                let state = scope.data.get_mut::<State>().unwrap();
                
                list.set_item_range(cx, 0, state.num_rows());
                while let Some(row_idx) = list.next_visible_item(cx) {
                    if row_idx >= state.num_rows() {
                        continue;
                    }
                    
                    let row = list.item(cx, row_idx, live_id!(ImageRow));
                    let mut scope = Scope::with_data_props(state, &row_idx);
                    row.draw_all(cx, &mut scope);
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
                let state = scope.data.get_mut::<State>().unwrap();
                let row_idx = *scope.props.get::<usize>().unwrap();
                
                list.set_item_range(cx, 0, state.num_images_for_row(row_idx));
                while let Some(item_idx) = list.next_visible_item(cx) {
                    if item_idx >= state.num_images_for_row(row_idx) {
                        continue;
                    }
                    
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
