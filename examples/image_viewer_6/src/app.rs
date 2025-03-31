use {makepad_widgets::*, std::{env, path::PathBuf}};

live_design! {
    use link::widgets::*;

    PLACEHOLDER_IMAGE = dep("crate://self/resources/placeholder_image.jpg");
    SEARCH_ICON = dep("crate://self/resources/search_icon.svg");
    
    Search = <View> {
        width: Fit,
        height: Fit,
        align: { y: 0.5 },
        
        <Icon> {
            icon_walk: { width: 12.0 },
            draw_icon: { svg_file: (SEARCH_ICON) }
        }

        query = <TextInput> {
            empty_message: "Search"
            draw_text: {
                text_style: { font_size: 10 }
                color: #8,
            }
        }
    }

    ImageItem = <View> {
        width: 256,
        height: 256,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER_IMAGE)
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
                    flow: Down,

                    <Search> {}
                    <ImageGrid> {}
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ImageRow {
    #[deref] view: View,
}

impl Widget for ImageRow {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let state = scope.data.get_mut::<State>().unwrap();
                let row_index = scope.props.get::<usize>().unwrap();
                let item_count = state.images_per_row.min(state.filtered_image_paths.len() - row_index * state.images_per_row);
                list.set_item_range(cx, 0, item_count);
                while let Some(item_index) = list.next_visible_item(cx) {
                    if item_index >= item_count {
                        continue;
                    }
                    let item = list.item(cx, item_index, live_id!(ImageItem));
                    let image = item.image(id!(image));
                    let image_index = row_index * state.images_per_row + item_index;
                    let image_path = &state.filtered_image_paths[image_index];
                    image.load_image_file_by_path(cx, &image_path.to_string_lossy()).unwrap();
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
    #[deref] view: View,
}

impl Widget for ImageGrid {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let state = scope.data.get_mut::<State>().unwrap();
                let num_rows = state.filtered_image_paths.len().div_ceil(state.images_per_row);
                list.set_item_range(cx, 0, num_rows);
                while let Some(row_index) = list.next_visible_item(cx) {
                    if row_index >= num_rows {
                        continue;
                    }
                    let item = list.item(cx, row_index, live_id!(ImageRow));
                    item.draw_all(cx, &mut Scope::with_data_props(state, &row_index));
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
    #[live] ui: WidgetRef,
    #[rust] state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}

impl LiveHook for App {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        let path: PathBuf = env::args().nth(1).unwrap().into();
        if !path.is_dir() {
            panic!();
        }
        self.state.image_paths.clear();
        for entry in path.read_dir().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            self.state.image_paths.push(path);
        }
        self.state.filter_image_paths("");
    }
}
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions) {
        if let Some(query) = self.ui.text_input(id!(query)).changed(&actions) {
            self.state.filter_image_paths(&query);
        }
    }
}

#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    filtered_image_paths: Vec<PathBuf>,
    images_per_row: usize,
}

impl State {
    pub fn filter_image_paths(&mut self, query: &str) {
        self.filtered_image_paths.clear();
        for image_path in &self.image_paths {
            if image_path.to_string_lossy().contains(query) {
                self.filtered_image_paths.push(image_path.clone());
            }
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            filtered_image_paths: Vec::new(),
            images_per_row: 4,
        }
    }
}

app_main!(App); 