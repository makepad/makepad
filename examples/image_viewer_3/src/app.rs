use {
    makepad_widgets::*,
    std::{
        env,
        path::{Path, PathBuf},
    },
};

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
                let row_idx = scope.props.get::<usize>().unwrap();
                let first_image_idx = row_idx * state.images_per_row;
                let remaining_image_count =
                    state.image_paths.len() - first_image_idx;
                let item_count =
                    state.images_per_row.min(remaining_image_count);
                list.set_item_range(cx, 0, item_count);
                while let Some(item_idx) = list.next_visible_item(cx) {
                    if item_idx >= item_count {
                        continue;
                    }
                    let image_idx = first_image_idx + item_idx;
                    let image_path = &state.image_paths[image_idx];
                    let item = list.item(cx, item_idx, live_id!(ImageItem));
                    let image = item.image(id!(image));
                    image
                        .load_image_file_by_path_async(cx, &image_path)
                        .unwrap();
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
                let num_rows =
                    state.image_paths.len().div_ceil(state.images_per_row);
                list.set_item_range(cx, 0, num_rows);
                while let Some(row_idx) = list.next_visible_item(cx) {
                    let row = list.item(cx, row_idx, live_id!(ImageRow));
                    row.draw_all(
                        cx,
                        &mut Scope::with_data_props(state, &row_idx),
                    );
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl App {
    pub fn update_image_paths(&mut self, path: &Path) {
        self.state.image_paths.clear();
        for entry in path.read_dir().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            self.state.image_paths.push(path);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}

impl LiveHook for App {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.update_image_paths(env::args().nth(1).unwrap().as_ref())
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    images_per_row: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            images_per_row: 4,
        }
    }
}

app_main!(App);
