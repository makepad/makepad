pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let Card = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 10
        padding: 16
        draw_bg.color: #x151c27
        draw_bg.border_radius: 14.0
    }

    let ActionButton = Button{
        width: Fit
        height: 36
        padding: Inset{left: 14 right: 14 top: 0 bottom: 0}
        draw_bg +: {
            color: #x2563eb
            color_hover: #x1d4ed8
            color_down: #x1e40af
            border_color: #x2563eb
            border_color_hover: #x1d4ed8
            border_color_down: #x1e40af
            border_radius: 8.0
        }
        draw_text +: {
            color: #xffffff
            color_hover: #xffffff
            color_down: #xffffff
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "File Dialog Example"
                window.inner_size: vec2(720, 640)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 16
                        padding: 24
                        draw_bg.color: #x0d1117

                        Label{
                            text: "System file dialogs"
                            draw_text.color: #xeff5ff
                            draw_text.text_style: theme.font_bold{font_size: 26}
                        }
                        Label{
                            width: Fill
                            text: "Open / save / folder pickers. Paths may be filesystem paths, content URIs (Android / OpenHarmony), or inline names + bytes (Web)."
                            draw_text.color: #x97a9c0
                        }

                        Card{
                            Label{
                                text: "Actions"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 15}
                            }
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 10
                                open_file := ActionButton{text: "Open file"}
                                open_images := ActionButton{text: "Open image…"}
                                save_file := ActionButton{text: "Save file"}
                                open_folder := ActionButton{text: "Open folder"}
                            }
                        }

                        Card{
                            Label{
                                text: "Last result"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 15}
                            }
                            status_label := Label{
                                width: Fill
                                text: "No dialog opened yet."
                                draw_text.color: #x9fd3af
                            }
                            meta_label := Label{
                                width: Fill
                                text: ""
                                draw_text.color: #x8aa0bc
                            }
                            paths_label := Label{
                                width: Fill
                                text: ""
                                draw_text.color: #xc9d5e7
                            }
                        }

                        Card{
                            width: Fill
                            height: Fill
                            Label{
                                text: "Content preview"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 15}
                            }
                            Label{
                                width: Fill
                                text: "Uses FileDialogResultEvent::read_bytes when path_kind is Filesystem or Inline."
                                draw_text.color: #x8aa0bc
                            }
                            preview := TextInput{
                                width: Fill
                                height: Fill
                                is_multiline: true
                                empty_text: "Select a text file to preview its contents…"
                                draw_bg +: {
                                    color: #x0d1117
                                    color_hover: #x0d1117
                                    color_focus: #x0d1117
                                    color_empty: #x0d1117
                                    border_color: #x243041
                                    border_color_hover: #x243041
                                    border_color_focus: #x2563eb
                                    border_radius: 8.0
                                }
                                draw_text +: {
                                    color: #xc9d5e7
                                    color_empty: #x64748b
                                    text_style +: {font_size: theme.font_size_code}
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
}

impl App {
    fn set_status(&mut self, cx: &mut Cx, status: &str, meta: &str, paths: &str, preview: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, status);
        self.ui.label(cx, ids!(meta_label)).set_text(cx, meta);
        self.ui.label(cx, ids!(paths_label)).set_text(cx, paths);
        self.ui.text_input(cx, ids!(preview)).set_text(cx, preview);
    }

    fn format_result(&self, e: &FileDialogResultEvent) -> (String, String, String, String) {
        let status = match e.status {
            FileDialogStatus::Ok => "Ok",
            FileDialogStatus::Cancelled => "Cancelled",
            FileDialogStatus::Unsupported => "Unsupported",
            FileDialogStatus::Error => "Error",
        };
        let kind = match e.kind {
            FileDialogKind::OpenFile => "OpenFile",
            FileDialogKind::SaveFile => "SaveFile",
            FileDialogKind::OpenFolder => "OpenFolder",
            FileDialogKind::SaveFolder => "SaveFolder",
        };
        let path_kind = match e.path_kind {
            FileDialogPathKind::Filesystem => "Filesystem",
            FileDialogPathKind::ContentUri => "ContentUri",
            FileDialogPathKind::Inline => "Inline",
        };

        let mut status_line = format!("status: {status}");
        if let Some(message) = &e.message {
            status_line.push_str(&format!(" — {message}"));
        }

        let meta = format!("request_id: {}  kind: {kind}  path_kind: {path_kind}", e.request_id);

        let paths = if e.paths.is_empty() {
            "paths: (none)".to_string()
        } else {
            format!("paths:\n{}", e.paths.join("\n"))
        };

        let preview = if e.is_ok() {
            match e.read_bytes(0) {
                Ok(bytes) => {
                    const MAX: usize = 8 * 1024;
                    let truncated = bytes.len() > MAX;
                    let slice = &bytes[..bytes.len().min(MAX)];
                    match std::str::from_utf8(slice) {
                        Ok(text) => {
                            if truncated {
                                format!("{text}\n… truncated ({} bytes total)", bytes.len())
                            } else {
                                text.to_string()
                            }
                        }
                        Err(_) => format!(
                            "<binary {} bytes{}>",
                            bytes.len(),
                            if truncated { ", preview truncated" } else { "" }
                        ),
                    }
                }
                Err(FileDialogIoError::NeedsPlatformApi) => {
                    "Content URI selected — use a platform content reader (not std::fs).".to_string()
                }
                Err(err) => format!("Could not read bytes: {err}"),
            }
        } else {
            String::new()
        };

        (status_line, meta, paths, preview)
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(open_file)).clicked(actions) {
            cx.open_system_openfile_dialog_with(
                FileDialog::new().set_title("Open any file".to_string()),
            );
        }
        if self.ui.button(cx, ids!(open_images)).clicked(actions) {
            cx.open_system_openfile_dialog_with(
                FileDialog::new()
                    .set_title("Open an image".to_string())
                    .add_filter(
                        "Images".to_string(),
                        vec![
                            "png".to_string(),
                            "jpg".to_string(),
                            "jpeg".to_string(),
                            "gif".to_string(),
                            "webp".to_string(),
                            "bmp".to_string(),
                        ],
                    ),
            );
        }
        if self.ui.button(cx, ids!(save_file)).clicked(actions) {
            cx.open_system_savefile_dialog_with(
                FileDialog::new()
                    .set_title("Save as".to_string())
                    .set_filename("untitled.txt".to_string())
                    .add_filter("Text".to_string(), vec!["txt".to_string()]),
            );
        }
        if self.ui.button(cx, ids!(open_folder)).clicked(actions) {
            cx.open_system_openfolder_dialog_with(
                FileDialog::new().set_title("Choose a folder".to_string()),
            );
        }
    }

    fn handle_file_dialog_result(&mut self, cx: &mut Cx, e: &FileDialogResultEvent) {
        let (status, meta, paths, preview) = self.format_result(e);
        self.set_status(cx, &status, &meta, &paths, &preview);
        self.ui.redraw(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
