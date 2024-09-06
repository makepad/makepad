use makepad_widgets::*;

live_design!(
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

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
                    button = <Button> {
                        text: "Open modal"
                        draw_text: {color: #f00}
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

                tooltip = <Tooltip> {}
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
        if self.ui.button(id!(button)).clicked(&actions) {
            self.ui.modal(id!(modal)).open(cx);
        }

        let label = self.ui.label(id!(title));
        let mut tooltip = self.ui.tooltip(id!(tooltip));
        if let Some(rect) = label.hover_in(&actions) {
            tooltip.show_with_options(cx, rect.pos, "Click in the button to open a modal");
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
