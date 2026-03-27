pub use makepad_widgets;

use makepad_widgets::*;
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 820)
                pass.clear_color: #x10151b
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 16
                        padding: Inset{left: 20 top: 20 right: 20 bottom: 20}

                        toolbar := RoundedView{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 12
                            padding: Inset{left: 16 top: 14 right: 16 bottom: 14}
                            draw_bg +: {color: #x18212b radius: 10.0}

                            capture_button := Button{text: "Capture framebuffer"}
                            status_label := Label{text: "Waiting for first capture" draw_text.text_style.font_size: 12}
                        }

                        output_path_label := Label{
                            width: Fill
                            text: "PNG output path: pending"
                            draw_text.text_style.font_size: 11
                        }

                        content := View{
                            width: Fill
                            height: Fill
                            flow: Right
                            spacing: 16

                            source_panel := RoundedView{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 18
                                padding: Inset{left: 20 top: 20 right: 20 bottom: 20}
                                draw_bg +: {color: #x203041 radius: 14.0}

                                source_title := H2{text: "Live scene"}
                                source_subtitle := Label{
                                    text: "This panel is rendered normally. The example captures the window framebuffer, writes a PNG, then loads that PNG back into the preview on the right."
                                }
                                scene_marker := Label{
                                    text: "Scene capture #0"
                                    draw_text.text_style.font_size: 26
                                }
                                swatches := View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 12

                                    swatch_a := RoundedView{width: Fill height: 120 draw_bg +: {color: #x2f80ed radius: 12.0}}
                                    swatch_b := RoundedView{width: Fill height: 120 draw_bg +: {color: #x27ae60 radius: 12.0}}
                                    swatch_c := RoundedView{width: Fill height: 120 draw_bg +: {color: #xeb5757 radius: 12.0}}
                                }
                                source_note := Label{
                                    text: "Repeated captures include the preview itself, so after the first pass you will see the expected recursive screenshot effect."
                                }
                            }

                            preview_panel := RoundedView{
                                width: 420
                                height: Fill
                                flow: Down
                                spacing: 12
                                padding: Inset{left: 16 top: 16 right: 16 bottom: 16}
                                draw_bg +: {color: #x151c24 radius: 14.0}

                                preview_title := H3{text: "Captured PNG reloaded into Image"}
                                capture_preview := Image{width: Fill height: Fill fit: ImageFit.Smallest}
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
    #[rust]
    pending_capture_request: Option<u64>,
    #[rust]
    capture_poll: Option<NextFrame>,
    #[rust]
    capture_count: u64,
    #[rust]
    output_path: PathBuf,
}

impl App {
    fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "rgba size overflow while encoding png".to_string())?;
        if rgba.len() != expected {
            return Err(format!(
                "expected {} rgba bytes, got {}",
                expected,
                rgba.len()
            ));
        }

        let options = EncoderOptions::default()
            .set_width(width as usize)
            .set_height(height as usize)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);
        let mut encoder = PngEncoder::new(rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        Ok(out)
    }

    fn update_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn update_scene_marker(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(scene_marker))
            .set_text(cx, &format!("Scene capture #{}", self.capture_count));
    }

    fn request_framebuffer_capture(&mut self, cx: &mut Cx) {
        self.capture_count += 1;
        self.output_path = std::env::temp_dir().join(format!(
            "makepad-capture-validation-{:03}.png",
            self.capture_count
        ));
        self.ui
            .label(cx, ids!(output_path_label))
            .set_text(cx, &format!("PNG output path: {}", self.output_path.display()));
        self.update_scene_marker(cx);

        let request_id = cx.request_capture(CaptureSource::Framebuffer);
        self.pending_capture_request = Some(request_id);
        self.capture_poll = Some(cx.new_next_frame());
        self.update_status(
            cx,
            &format!("Waiting for capture result {}", self.capture_count),
        );
    }

    fn poll_capture_results(&mut self, cx: &mut Cx) {
        let Some(expected_request_id) = self.pending_capture_request else {
            return;
        };

        let mut matched = false;
        for result in cx.drain_capture_results() {
            if result.request_id != expected_request_id {
                continue;
            }

            matched = true;
            match Self::encode_png_rgba(result.width, result.height, &result.rgba) {
                Ok(png) => {
                    match std::fs::write(&self.output_path, &png) {
                        Ok(()) => {
                            if let Err(err) = self
                                .ui
                                .image(cx, ids!(capture_preview))
                                .load_image_file_by_path(cx, &self.output_path)
                            {
                                self.update_status(cx, &format!("PNG written but reload failed: {err:?}"));
                            } else {
                                self.update_status(
                                    cx,
                                    &format!(
                                        "Capture {} saved and reloaded: {}x{}",
                                        self.capture_count,
                                        result.width,
                                        result.height
                                    ),
                                );
                            }
                        }
                        Err(err) => {
                            self.update_status(cx, &format!("PNG write failed: {err}"));
                        }
                    }
                }
                Err(err) => {
                    self.update_status(cx, &format!("PNG encode failed: {err}"));
                }
            }
        }

        if matched {
            self.pending_capture_request = None;
            self.capture_poll = None;
        } else {
            self.capture_poll = Some(cx.new_next_frame());
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.request_framebuffer_capture(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(capture_button)).clicked(actions) {
            self.request_framebuffer_capture(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        if let Some(next) = self.capture_poll {
            if next.is_event(event).is_some() {
                self.poll_capture_results(cx);
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
