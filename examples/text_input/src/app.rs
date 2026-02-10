use makepad_widgets::*;

live_design! {
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                window: {title: "TextInput IME Test"},
                body = {
                    <ScrollYView> {
                        flow: Down,
                        padding: {top: 20, bottom: 20, left: 20, right: 40}
                        spacing: 20,

                        <Label> {
                            draw_text: { text_style: { font_size: 16.0 } }
                            text: "TextInput IME Configuration Test"
                        }

                        // 1. Default multiline with autocorrect
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "1. Default (multiline, autocorrect, sentences capitalization)"
                            }
                            input_default = <TextInput> {
                                empty_text: "Default text input..."
                                width: Fill,
                                height: 150,
                                // All defaults: is_multiline: true, autocorrect: Default,
                                // autocapitalize: Sentences, input_mode: Text
                            }
                        }

                        // 2. Single-line (Enter submits instead of newline)
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "2. Single-line (Enter submits, Done button)"
                            }
                            input_singleline = <TextInput> {
                                empty_text: "Single line, press Enter to submit..."
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                return_key_type: Done,
                            }
                        }

                        // 3. Email input
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "3. Email"
                            }
                            input_email = <TextInput> {
                                empty_text: "email@example.com"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                input_mode: Email,
                                autocorrect: Disabled,
                                autocapitalize: None,
                            }
                        }

                        // 4. URL input
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "4. URL (url keyboard, no autocorrect)"
                            }
                            input_url = <TextInput> {
                                empty_text: "https://example.com"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                input_mode: Url,
                                autocorrect: Disabled,
                                autocapitalize: None,
                                return_key_type: Go,
                            }
                        }

                        // 5. Number input
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "5. Number (decimal pad)"
                            }
                            input_number = <TextInput> {
                                empty_text: "123.45"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                input_mode: Decimal,
                            }
                        }

                        // 6. Search input
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "6. Search (search button, autocorrect on)"
                            }
                            input_search = <TextInput> {
                                empty_text: "Search..."
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                input_mode: Search,
                                return_key_type: Search,
                            }
                        }

                        // 7. Password input
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "7. Password (secure, no autocorrect)"
                            }
                            input_password = <TextInput> {
                                empty_text: "Password"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                is_password: true,
                                autocorrect: Disabled,
                                autocapitalize: None,
                            }
                        }

                        // 8. All caps hint (mobile keyboard only)
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "8. Autocapitalize hint (mobile only, shifts keyboard to caps)"
                            }
                            input_allcaps = <TextInput> {
                                empty_text: "ALL CAPS INPUT"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                autocapitalize: AllCharacters,
                            }
                        }

                        // 9. ASCII input (filtered on all platforms)
                        <View> {
                            height: Fit
                            flow: Down,
                            spacing: 5,
                            <Label> {
                                text: "9. ASCII (filtered on all platforms, ASCII keyboard on mobile)"
                            }
                            input_ascii = <TextInput> {
                                empty_text: "ASCII input"
                                width: Fill,
                                height: 50,
                                is_multiline: false,
                                autocorrect: Enabled,
                                input_mode: Ascii,
                            }
                        }

                        // Status label for showing actions
                        <Label> {
                            draw_text: { text_style: { font_size: 12.0 } }
                            text: "Status: Ready"
                        }
                        status_label = <Label> {
                            draw_text: { text_style: { font_size: 11.0 } }
                            text: ""
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
        // Check for Returned action from single-line inputs
        let inputs = [
            ("singleline", ids!(input_singleline)),
            ("email", ids!(input_email)),
            ("url", ids!(input_url)),
            ("number", ids!(input_number)),
            ("search", ids!(input_search)),
            ("password", ids!(input_password)),
            ("allcaps", ids!(input_allcaps)),
        ];

        for (name, id) in inputs {
            if let Some((text, _mods)) = self.ui.text_input(id).returned(actions) {
                let msg = format!("Returned from {}: \"{}\"", name, text);
                log!("{}", msg);
                self.ui.label(ids!(status_label)).set_text(cx, &msg);
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
