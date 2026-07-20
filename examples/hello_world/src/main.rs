pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const SINGLE_FRAME_GIF: &[u8] = include_bytes!("../makepad_gifs/single_frame.gif");
const ANIMATED_GIF: &[u8] = include_bytes!("../makepad_gifs/giphy.gif");

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(520, 320)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 14
                        padding: 24
                        align: Center

                        Label{
                            text: "Hello GIF"
                            draw_text.text_style.font_size: 26
                        }

                        status_label := Label{
                            text: "Loading embedded GIF fixtures..."
                        }

                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 18
                            align: Center

                            View{
                                width: Fit
                                height: Fit
                                flow: Down
                                spacing: 8
                                align: Center

                                Label{text: "Single frame"}
                                single_gif_image := AnimatedImageGif{
                                    width: 96
                                    height: 96
                                    inner: Image{
                                        fit: ImageFit.Stretch
                                        width: Fill
                                        height: Fill
                                    }
                                }
                            }

                            View{
                                width: Fit
                                height: Fit
                                flow: Down
                                spacing: 8
                                align: Center

                                Label{text: "Animated"}
                                animated_gif_image := AnimatedImageGif{
                                    width: 96
                                    height: 96
                                    inner: Image{
                                        fit: ImageFit.Stretch
                                        width: Fill
                                        height: Fill
                                    }
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

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let single = ImageBuffer::from_gif(SINGLE_FRAME_GIF);
        let animated = ImageBuffer::from_gif(ANIMATED_GIF);
        let status = match (&single, &animated) {
            (Ok(single), Ok(animated)) => format!(
                "GIF decode ok: single {}x{}, animated {} frames",
                single.width,
                single.height,
                animated
                    .animation
                    .as_ref()
                    .map(|animation| animation.num_frames)
                    .unwrap_or(1)
            ),
            (Err(error), _) => format!("Single-frame GIF decode failed: {:?}", error),
            (_, Err(error)) => format!("Animated GIF decode failed: {:?}", error),
        };

        self.ui.label(cx, ids!(status_label)).set_text(cx, &status);

        let _ = self
            .ui
            .animated_image_gif(cx, ids!(single_gif_image))
            .load_gif_from_data(cx, SINGLE_FRAME_GIF);
        let _ = self
            .ui
            .animated_image_gif(cx, ids!(animated_gif_image))
            .load_gif_from_data(cx, ANIMATED_GIF);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world_single_frame_gif_decodes() {
        let image = ImageBuffer::from_gif(SINGLE_FRAME_GIF).unwrap();

        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert!(image.animation.is_none());
    }

    #[test]
    fn hello_world_animated_gif_decodes_with_animation_metadata() {
        let image = ImageBuffer::from_gif(ANIMATED_GIF).unwrap();
        let animation = image.animation.unwrap();

        assert_eq!(animation.width, 1);
        assert_eq!(animation.height, 1);
        assert_eq!(animation.num_frames, 2);
    }
}
