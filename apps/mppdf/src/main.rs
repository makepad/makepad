//! mppdf: a PDF viewer. `mppdf <path>` opens a document as one continuous
//! scroll of pages on a dark mp_theme background. `mppdf --preview <path>`
//! sizes the window as a small popup instead — that is mpfiles' Quick Look.

pub use makepad_widgets;
use makepad_widgets::*;
use mppdf::preview::{PreviewAction, PreviewState};
use mppdf::theme::Palette;
use mppdf::widget::{FitMode, MpPdfAction, MpPdfView, PdfStatus};
use mppdf::Args;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ToolButton = View{
        width: 28
        height: 24
        flow: Overlay
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        sel := SolidView{
            visible: false
            width: Fill
            height: Fill
            draw_bg +: {color: mod.mpp.accent}
        }
    }

    let ToolGap = View{
        width: 10
        height: Fill
    }

    let ToolLabel = Label{
        height: Fill
        align: Align{x: 0.5 y: 0.5}
        draw_text +: {
            color: mod.mpp.fg
            text_style: theme.font_regular{font_size: 9.0}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1000, 800)
                window.title: "mppdf"
                pass +: { clear_color: mod.mpp.bg }
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 0
                        padding: 0

                        toolbar := RectView{
                            width: Fill
                            height: 32
                            flow: Right
                            spacing: 2
                            align: Align{y: 0.5}
                            padding: Inset{left: 6 right: 6}
                            draw_bg +: {color: mod.mpp.bg}

                            thumbs_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/thumbnails.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }

                            ToolGap{}

                            prev_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/page-up.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }
                            next_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/page-down.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }

                            page_box := RectView{
                                width: 44
                                height: 22
                                margin: Inset{left: 4 right: 4}
                                draw_bg +: {
                                    color: mod.mpp.bg_light
                                    border_color: mod.mpp.dim
                                    border_size: 1.0
                                    border_radius: 0.0
                                }
                                page_field := TextInput{
                                    width: Fill
                                    height: Fill
                                    margin: 0.0
                                    padding: Inset{left: 4 right: 4 top: 3 bottom: 3}
                                    empty_text: "1"
                                    draw_bg +: {
                                        border_radius: uniform(0.0)
                                        border_size: uniform(0.0)
                                        color: mod.mpp.bg_light
                                        color_hover: uniform(mod.mpp.bg_light)
                                        color_focus: uniform(mod.mpp.bg_light)
                                        color_down: uniform(mod.mpp.bg_light)
                                        color_empty: uniform(mod.mpp.bg_light)
                                        color_disabled: uniform(mod.mpp.bg_light)
                                        color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
                                        border_color: uniform(mod.mpp.bg_light)
                                        border_color_hover: uniform(mod.mpp.bg_light)
                                        border_color_focus: uniform(mod.mpp.bg_light)
                                        border_color_down: uniform(mod.mpp.bg_light)
                                        border_color_empty: uniform(mod.mpp.bg_light)
                                        border_color_disabled: uniform(mod.mpp.bg_light)
                                        border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
                                    }
                                    draw_text +: {
                                        color: mod.mpp.fg
                                        color_hover: uniform(mod.mpp.fg)
                                        color_focus: uniform(mod.mpp.fg)
                                        color_down: uniform(mod.mpp.fg)
                                        color_disabled: uniform(mod.mpp.dim)
                                        color_empty: uniform(mod.mpp.dim)
                                        color_empty_hover: uniform(mod.mpp.dim)
                                        color_empty_focus: uniform(mod.mpp.dim)
                                        text_style: theme.font_regular{font_size: 9.0}
                                    }
                                    draw_cursor +: {color: uniform(mod.mpp.accent)}
                                    draw_selection +: {
                                        border_radius: uniform(0.0)
                                        color: uniform(mod.mpp.accent)
                                        color_hover: uniform(mod.mpp.accent)
                                        color_focus: uniform(mod.mpp.accent)
                                        color_down: uniform(mod.mpp.accent)
                                    }
                                }
                            }

                            page_of := ToolLabel{
                                width: 46
                                align: Align{x: 0.0 y: 0.5}
                                text: "/ 0"
                                draw_text +: {color: mod.mpp.dim}
                            }

                            Filler{}

                            zoom_out_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/zoom-out.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }
                            zoom_label := ToolLabel{
                                width: 48
                                text: "100%"
                            }
                            zoom_in_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/zoom-in.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }

                            ToolGap{}

                            fit_width_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/fit-width.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }
                            fit_page_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/fit-page.svg")
                                        color: mod.mpp.fg
                                    }
                                }
                            }
                        }

                        viewer := MpPdfView{
                            width: Fill
                            height: Fill
                        }

                        status_row := RectView{
                            width: Fill
                            height: 22
                            align: Align{y: 0.5}
                            padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
                            draw_bg +: {color: mod.mpp.bg}

                            status := Label{
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                text: ""
                                draw_text +: {
                                    color: mod.mpp.dim
                                    text_style: theme.font_regular{font_size: 9}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// True while the page field owns the keyboard: the viewer's own
    /// shortcuts stand down so typing "12" is a page number, not two zoom
    /// commands.
    #[rust]
    page_field_focused: bool,
    #[rust]
    status: PdfStatus,
    /// The Quick Look v2 warm-viewer state: whether this run is a preview
    /// panel, and what it currently shows. See `preview.rs`.
    #[rust]
    preview: PreviewState,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Args = mppdf::parse_args(std::env::args().skip(1));

        // mp_theme::apply already retinted theme.color_bg_app and
        // mod.widgets.Window.pass.clear_color from a theme.splash when one
        // is exported; patch the pass clear color explicitly too so the
        // ground behind the toolbar is right either way.
        {
            let color = Palette::shared().bg_vec4();
            let mut window = self.ui.window(cx, ids!(main_window));
            script_apply_eval!(cx, window, {
                pass +: { clear_color: #(color) }
            });
        }

        self.preview = PreviewState::new(args.preview, args.path.as_deref());

        if let Some(mut viewer) = self.ui.widget(cx, ids!(viewer)).borrow_mut::<MpPdfView>() {
            viewer.set_preview(args.preview);
            if let Some(path) = &args.path {
                viewer.open(cx, path);
            }
        }

        self.set_title(cx);
        // A --stdin-loop child is tiled by the compositor, which owns the
        // window size; a standalone --preview run gets the popup size.
        if args.preview && std::env::var("STUDIO_HOST").is_err() {
            self.ui
                .window(cx, ids!(main_window))
                .resize(cx, dvec2(900.0, 700.0));
        }
    }

    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        // Cmd+G is the one shortcut that belongs to the chrome rather than
        // to the document: it puts the caret in the page field.
        if e.key_code == KeyCode::KeyG && e.modifiers.is_primary() {
            let page_field = self.ui.text_input(cx, ids!(page_field));
            page_field.take_key_focus(cx);
            if let Some(mut inner) = page_field.borrow_mut() {
                inner.select_all(cx);
            }
            self.set_field_focus(cx, true);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            // A raw downcast never matches a widget action: unwrap the
            // WidgetAction envelope first, then cast.
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if let MpPdfAction::Status(status) = widget_action.cast() {
                self.apply_status(cx, status);
            }
        }

        self.handle_toolbar(cx, actions);
        self.handle_page_field(cx, actions);
    }
}

impl App {
    fn viewer(&self, cx: &Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(viewer))
    }

    fn with_viewer(&self, cx: &mut Cx, f: impl FnOnce(&mut MpPdfView, &mut Cx)) {
        let widget = self.viewer(cx);
        let Some(mut viewer) = widget.borrow_mut::<MpPdfView>() else {
            return;
        };
        f(&mut viewer, cx);
        // Release the borrow before `widget` goes out of scope.
        drop(viewer);
    }

    /// Quick Look v2: `StudioToApp::Custom` from mpwm reaches a hosted app
    /// as `Event::Custom(json)`; this is what the viewer half of the
    /// protocol does with it. `PreviewUnload` never ends the process.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &mp_wm_api::WmEvent) {
        match self.preview.on_wm_event(event) {
            PreviewAction::Show(path) => {
                self.with_viewer(cx, |v, cx| v.open(cx, &path));
                self.set_title(cx);
            }
            PreviewAction::Unload => {
                self.with_viewer(cx, |v, cx| v.unload(cx));
                self.set_title(cx);
            }
            PreviewAction::Close => cx.quit(),
            PreviewAction::Ignore => {}
        }
    }

    /// The window's own caption, and the title the WM shows for the tile.
    fn set_title(&mut self, cx: &mut Cx) {
        let title = self.preview.title("mppdf");
        self.ui.window(cx, ids!(main_window)).set_title(cx, &title);
        mp_wm_api::set_title(cx, &title);
    }

    fn handle_toolbar(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.clicked(cx, actions, ids!(prev_button)) {
            self.with_viewer(cx, |v, cx| v.page_back(cx));
        }
        if self.clicked(cx, actions, ids!(next_button)) {
            self.with_viewer(cx, |v, cx| v.page_forward(cx));
        }
        if self.clicked(cx, actions, ids!(zoom_in_button)) {
            self.with_viewer(cx, |v, cx| v.zoom_in(cx));
        }
        if self.clicked(cx, actions, ids!(zoom_out_button)) {
            self.with_viewer(cx, |v, cx| v.zoom_out(cx));
        }
        if self.clicked(cx, actions, ids!(fit_width_button)) {
            self.with_viewer(cx, |v, cx| v.set_fit(cx, FitMode::Width));
        }
        if self.clicked(cx, actions, ids!(fit_page_button)) {
            self.with_viewer(cx, |v, cx| v.set_fit(cx, FitMode::Page));
        }
        if self.clicked(cx, actions, ids!(thumbs_button)) {
            self.with_viewer(cx, |v, cx| v.toggle_thumbs(cx));
        }
    }

    fn clicked(&self, cx: &mut Cx, actions: &Actions, path: &[LiveId]) -> bool {
        self.ui.view(cx, path).finger_down(actions).is_some()
    }

    fn handle_page_field(&mut self, cx: &mut Cx, actions: &Actions) {
        let page_field = self.ui.text_input(cx, ids!(page_field));
        if let Some((text, _)) = page_field.returned(actions) {
            if let Ok(number) = text.trim().parse::<usize>() {
                if number >= 1 {
                    self.with_viewer(cx, |v, cx| v.go_to_page(cx, number - 1));
                }
            }
            // Hand the keyboard back to the document.
            cx.set_key_focus(Area::Empty);
        }
        if page_field.escaped(actions) {
            let text = format!("{}", self.status.page.max(1));
            page_field.set_text(cx, &text);
        }
        // Focus is a state, not an event: asking beats bookkeeping, and it
        // cannot drift when focus moves for a reason we did not see.
        let focused = page_field.key_focus(cx);
        self.set_field_focus(cx, focused);
    }

    fn set_field_focus(&mut self, cx: &mut Cx, focused: bool) {
        if self.page_field_focused == focused {
            return;
        }
        self.page_field_focused = focused;
        self.with_viewer(cx, move |v, _| v.set_keys_enabled(!focused));
    }

    /// Paint the chrome from one viewer status.
    fn apply_status(&mut self, cx: &mut Cx, status: PdfStatus) {
        if status == self.status {
            return;
        }

        if status.page != self.status.page && !self.page_field_focused {
            let text = if status.page == 0 {
                String::new()
            } else {
                format!("{}", status.page)
            };
            self.ui.text_input(cx, ids!(page_field)).set_text(cx, &text);
        }

        self.ui
            .label(cx, ids!(page_of))
            .set_text(cx, &format!("/ {}", status.page_count));
        self.ui
            .label(cx, ids!(zoom_label))
            .set_text(cx, &format!("{}%", status.zoom_pct));

        self.ui
            .widget(cx, ids!(fit_width_button.sel))
            .set_visible(cx, status.fit_width);
        self.ui
            .widget(cx, ids!(fit_page_button.sel))
            .set_visible(cx, status.fit_page);
        self.ui
            .widget(cx, ids!(thumbs_button.sel))
            .set_visible(cx, status.thumbs);

        self.ui
            .label(cx, ids!(status))
            .set_text(cx, &status_line(&status));

        self.status = status;
    }
}

/// The status line: what the document is, how far it has loaded, where we
/// are in it.
fn status_line(status: &PdfStatus) -> String {
    if let Some(error) = &status.error {
        return error.clone();
    }
    if status.page_count == 0 && status.name.is_empty() {
        // An unloaded warm viewer has nothing to say.
        return String::new();
    }
    if status.page_count == 0 {
        return if status.name == "no document" {
            "No document \u{2014} mppdf <file.pdf>".to_string()
        } else {
            format!("{}    opening\u{2026}", status.name)
        };
    }
    let loading = if status.loaded < status.page_count {
        format!("    loading {} / {}", status.loaded, status.page_count)
    } else {
        String::new()
    };
    format!(
        "{}    page {} of {}    {}%{}",
        status.name, status.page, status.page_count, status.zoom_pct, loading
    )
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        mp_theme::apply(vm);
        Palette::shared().publish(vm);
        mppdf::thumbs::script_mod(vm);
        mppdf::widget::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if let Some(wm) = mp_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
