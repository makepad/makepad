use makepad_widgets::*;

live_design!(
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;

    App = {{App}} {
        ui: <Window> {
            window: {inner_size: vec2(600, 800)},
            body = {
                flow: Overlay,
                padding: 0.0
                spacing: 0,
                
                <View> {
                    height: Fit
                    width: Fit
                    spacing: 20
                    padding: {left: 50, top: 100 }

                    align: { x: 0, y: 0.5}

                    title = <Label> {
                        text: "Floating elements"
                        draw_text: {
                            text_style: {
                                font_size: 12.0
                            }
                            color: #f38
                        }
                        
                        hover_actions_enabled: true
                    }

                    button_modal = <Button> {
                        text: "Open modal"
                        draw_text: {color: #f00}
                    }

                    button_notification = <Button> {
                        text: "Open popup notification"
                        draw_text: {color: #f00}
                    }
                }

                tooltip = <Tooltip> {}

                notification = <PopupNotification> {
                    content: {
                        height: Fit,
                        width: Fit,

                        padding: 10,

                        <RoundedView> {
                            height: Fit,
                            width: 240,

                            padding: 30,
                            show_bg: true,
                            draw_bg: {
                                color: #3c3c3c
                                radius: 3.0
                            }
                            <Label> {
                                width: 170
                                text: "This is a popup notification\nwith some content"
                            }
                            close_popup_button = <Button> {
                                width: Fit,
                                height: Fit,

                                margin: {top: -20 },

                                draw_icon: {
                                    svg_file: dep("crate://self/resources/close.svg"),
                                    fn get_color(self) -> vec4 {
                                        return #000;
                                    }
                                }
                                icon_walk: {width: 10, height: 10}
                            }
                        }
                    }
                }

                modal = <Modal> {
                    content: {
                        height: 100,
                        width: 150,
                        show_bg: true,
                        draw_bg: {
                            color: #3c3c3c
                        }
                        align: {
                            x: 0.5,
                            y: 0.5
                        }
                        <Label> {
                            text: "Content in the modal"
                        }
                    }
                }
            }
        }
    }
);

#[derive(Live, LiveHook)]
struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button_modal)).clicked(&actions) {
            self.ui.modal(id!(modal)).open(cx);
        }

        if self.ui.button(id!(button_notification)).clicked(&actions) {
            self.ui.popup_notification(id!(notification)).open(cx);
        }

        if self.ui.button(id!(close_popup_button)).clicked(&actions) {
            self.ui.popup_notification(id!(notification)).close(cx);
        }

        let label = self.ui.label(id!(title));
        let mut tooltip = self.ui.tooltip(id!(tooltip));
        if let Some(rect) = label.hover_in(&actions) {
            tooltip.show_with_options(cx, rect.pos, "Here is a tooltip");
        }
        if label.hover_out(&actions) {
            tooltip.hide(cx);
        }
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

app_main!(App);
