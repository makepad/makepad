use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    LoaderLabel = <Label> {
        width: Fill, height: Fill
        draw_text:{text_style: {font_size: 13.}}
    }

    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                window: {inner_size: vec2(1280, 890)},
                show_bg: true
                width: Fill,
                height: Fill

                draw_bg: {
                    fn pixel(self) -> vec4 {
                        return mix(#7, #3, self.pos.y);
                    }
                }

                body = <ScrollXYView>{
                    flow: Down,
                    spacing: 20,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    padding: 100
                    input1 = <TextInput> {
                        width: 900, height: Fit
                        text: "https://images.unsplash.com/photo-1667053312990-c17655704190?q=80&w=1770&auto=format&fit=crop&ixlib=rb-4.0.3&ixid=M3wxMjA3fDB8MHxwaG90by1wYWdlfHx8fGVufDB8fHx8fA%3D%3D"
                        empty_message: "someurl.com/image.jpg"
                    }
                    button1 = <Button> {
                        text: "Update image"
                        draw_text:{color:#f00}
                    }

                    image_loader = <ImageLoader> {
                        width: 896, height: 560

                        content: {
                            image = <Image> {
                                width: Fill, height: Fill
                                fit: Vertical
                            }
                        }
                    }

                    content_loader = <ContentLoader> {
                        width: 896, height: 560

                        placeholder: <View> {
                            label = <LoaderLabel> {
                                text: "Loading..."
                            }
                        }

                        content: <View> {
                            label = <LoaderLabel> {
                                text: "Your image is ready!"
                            }
                        }

                        error: <View> {
                            label = <LoaderLabel> { 
                                text: "Ooops, sorry about that. Please try some other image."
                            }
                        }
                    }
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            match action.as_widget_action().cast() {
                ImageLoaderAction::ImageLoaded => {
                    self.ui
                        .content_loader(id!(content_loader))
                        .set_content_loading_status(ContentLoadingStatus::Loaded);
                    self.ui.redraw(cx);
                }
                ImageLoaderAction::ImageLoadingFailed => {
                    self.ui
                        .content_loader(id!(content_loader))
                        .set_content_loading_status(ContentLoadingStatus::Failed);
                    self.ui.redraw(cx);
                }
                _ => (),
            }
        }

        if self.ui.button(id!(button1)).clicked(&actions) {
            let uri = self.ui.text_input(id!(input1)).text();
            self.ui
                .image_loader(id!(image_loader))
                .load_from_url(cx, &uri);

            self.ui
                .content_loader(id!(content_loader))
                .set_content_loading_status(ContentLoadingStatus::Loading);
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
