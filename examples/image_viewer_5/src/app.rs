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
    LEFT_ARROW = dep("crate://self/resources/left_arrow.svg");
    RIGHT_ARROW = dep("crate://self/resources/right_arrow.svg");

    TopMenu = <View> {
        width: Fill,
        height: Fit,

        <Filler> {}
        slideshow_button = <Button> {
            text: "Slideshow"
        }
    }

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

    ImageBrowser = <View> {
        flow: Down,

        <TopMenu> {}
        <ImageGrid> {}
    }

    SlideshowButton = <Button> {
        width: 50,
        height: Fill,
        grab_key_focus: false,
        draw_bg: {
            color: #fff0,
            color_down: #fff2,
        }
        icon_walk: { width: 9 },
        text: ""
    }

    SlideshowOverlay = <View> {
        height: Fill,
        width: Fill,
        cursor: Arrow,
        capture_overload: true,

        left = <SlideshowButton> {
            draw_icon: { svg_file: (LEFT_ARROW) }
        }
        <Filler> {}
        right = <SlideshowButton> {
            draw_icon: { svg_file: (RIGHT_ARROW) }
        }
    }

    Slideshow = <View> {
        flow: Overlay,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }

        overlay = <SlideshowOverlay> {}
    }

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <PageFlip> {
                    active_page: image_browser,

                    image_browser = <ImageBrowser> {}
                    slideshow = <Slideshow> {}
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
    pub fn update_image_paths(&mut self, cx: &mut Cx, path: &Path) {
        self.state.image_paths.clear();
        for entry in path.read_dir().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            self.state.image_paths.push(path);
        }
        if self.state.image_paths.is_empty() {
            self.set_current_image(cx, None);
        } else {
            self.set_current_image(cx, Some(0));
        }
    }

    pub fn set_current_image(&mut self, cx: &mut Cx, image_idx: Option<usize>) {
        self.state.current_image_idx = image_idx;
        let image = self.ui.image(id!(slideshow.image));
        if let Some(image_idx) = self.state.current_image_idx {
            let image_path = &self.state.image_paths[image_idx];
            image.load_image_file_by_path_async(cx, &image_path).unwrap();
        } else {
            image
                .load_image_dep_by_path(
                    cx,
                    "crate://self/resources/placeholder.jpg",
                )
                .unwrap();
        }
        self.ui.view(id!(slideshow)).redraw(cx);
    }

    pub fn navigate_left(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx > 0 {
                self.set_current_image(cx, Some(image_idx - 1));
            }
        }
    }

    pub fn navigate_right(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx + 1 < self.state.image_paths.len() {
                self.set_current_image(cx, Some(image_idx + 1));
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}

impl LiveHook for App {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.update_image_paths(cx, env::args().nth(1).unwrap().as_ref());
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(id!(slideshow_button)).clicked(&actions) {
            self.ui
                .page_flip(id!(body))
                .set_active_page(cx, live_id!(slideshow));
            self.ui.view(id!(slideshow.overlay)).set_key_focus(cx);
        }
        if self.ui.button(id!(left)).clicked(&actions) {
            self.navigate_left(cx);
        }
        if self.ui.button(id!(right)).clicked(&actions) {
            self.navigate_right(cx);
        }
        if let Some(event) =
            self.ui.view(id!(slideshow.overlay)).key_down(&actions)
        {
            match event.key_code {
                KeyCode::ArrowLeft => self.navigate_left(cx),
                KeyCode::ArrowRight => self.navigate_right(cx),
                KeyCode::Escape => self
                    .ui
                    .page_flip(id!(body))
                    .set_active_page(cx, live_id!(image_browser)),
                _ => {}
            }
        }
    }
}

#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    images_per_row: usize,
    current_image_idx: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            images_per_row: 4,
            current_image_idx: None,
        }
    }
}

app_main!(App);
