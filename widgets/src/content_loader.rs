use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, View};

live_design! {
    LoaderPlaceholderBase = {{LoaderPlaceholder}} {}
    ContentLoaderBase = {{ContentLoader}} {}
}

#[derive(Clone, Debug, DefaultNone)]
pub enum ContentLoaderAction {
    None,
    ContentLoaded,
}

#[derive(Live, LiveHook, Widget)]
pub struct ContentLoader {
    #[redraw]
    #[live]
    draw_bg: DrawQuad,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// A placeholder view to show while the content is loading.
    #[live]
    placeholder: LoaderPlaceholder,

    /// The actual content to be displayed once loaded.
    #[live]
    content: View,

    /// An error view to show if the content fails to load.
    #[live]
    error: View,

    /// Wether to display the image as soon as it's loaded.
    #[rust(true)]
    show_content_on_load: bool,

    #[rust]
    content_loading_status: ContentLoadingStatus,
}

#[derive(Default, Debug)]
pub enum ContentLoadingStatus {
    #[default]
    NotStarted,
    Loading,
    Loaded,
    Failed,
}

impl Widget for ContentLoader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.placeholder.handle_event(cx, event, scope);
        self.content.handle_event(cx, event, scope);
        self.error.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);

        match self.content_loading_status {
            ContentLoadingStatus::Loaded => {
                if self.show_content_on_load {
                    let _ = self.content.draw_walk(cx, scope, walk);
                }
            }
            ContentLoadingStatus::NotStarted | ContentLoadingStatus::Loading => {
                let _ = self.placeholder.draw_walk(cx, scope, walk);
            }
            ContentLoadingStatus::Failed => {
                let _ = self.error.draw_walk(cx, scope, walk);
            }
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }
}

impl ContentLoader {
    pub fn set_content_loading_status(&mut self, status: ContentLoadingStatus) {
        self.content_loading_status = status;
    }

    pub fn set_loaded_and_show_content(&mut self, cx: &mut Cx) {
        self.content_loading_status = ContentLoadingStatus::Loaded;
        self.show_content_on_load = true;
        self.redraw(cx);
        cx.widget_action(
            self.widget_uid(),
            &HeapLiveIdPath::default(),
            ContentLoaderAction::ContentLoaded,
        );
    }

    pub fn set_show_content_on_load(&mut self, show: bool) {
        self.show_content_on_load = show;
    }

    pub fn content_view(&mut self) -> &mut View {
        &mut self.content
    }
    
    pub fn error_view(&mut self) -> &mut View {
        &mut self.error
    }

    pub fn placeholder_view(&mut self) -> &mut LoaderPlaceholder {
        &mut self.placeholder
    }
}

impl ContentLoaderRef {
    pub fn set_content_loading_status(&mut self, status: ContentLoadingStatus) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_content_loading_status(status)
        }
    }

    pub fn set_loaded_and_show_content(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_loaded_and_show_content(cx);
        }
    }

    pub fn set_show_content_on_load(&mut self, show: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_show_content_on_load(show);
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct LoaderPlaceholder {
    #[deref]
    view: View,

    #[animator]
    animator: Animator,
}

impl Widget for LoaderPlaceholder {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.view.redraw(cx);
        }
        if self.animator.need_init() {
            self.animator_play(cx, id!(spinner.spin));
        }

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl LoaderPlaceholderRef {
    pub fn set_animator_play(&mut self, cx: &mut Cx, id: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, id);
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.view.visible = visible;
        }
    }
}
