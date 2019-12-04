use render::*;
use widget::*;

#[derive(Clone)]
pub struct HomePage {
    pub view: ScrollView,
    pub shadow: ScrollShadow,
    pub text: Text,
    pub email_input: TextInput,
    pub email_state: EmailState,
    pub email_signal: Signal,
    pub example_texts: ElementsCounted<TextInput>,
    pub send_mail_button: NormalButton,
}

#[derive(Clone)]
pub enum EmailState {
    Empty,
    Invalid,
    Valid,
    Sending,
    ErrorSending,
    OkSending
}

impl HomePage {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            view: ScrollView::proto(cx),
            text: Text::proto(cx),
            shadow: ScrollShadow::proto(cx),
            send_mail_button: NormalButton::proto(cx),
            email_signal: cx.new_signal(),
            email_input: TextInput::proto(cx, TextInputOptions {
                multiline: false,
                read_only: false,
                empty_message: "Enter email".to_string()
            }),
            email_state: EmailState::Empty,
            example_texts: ElementsCounted::new(
                TextInput::proto(cx, TextInputOptions {
                    multiline: true,
                    read_only: true,
                    empty_message: "".to_string()
                })
            ),
        }
    }
    
    pub fn color_heading() -> ColorId {uid!()}
    pub fn color_body() -> ColorId {uid!()}
    pub fn layout_main() -> LayoutId {uid!()}
    pub fn text_style_heading() -> TextStyleId {uid!()}
    pub fn text_style_body() -> TextStyleId {uid!()}
    pub fn text_style_point() -> TextStyleId {uid!()}
    pub fn walk_paragraph() -> WalkId {uid!()}
    
    pub fn style(cx: &mut Cx, opt: &StyleOptions) {
        
        Self::text_style_heading().set(cx, TextStyle {
            font_size: 28.0 * opt.scale,
            line_spacing: 2.0,
            ..Theme::text_style_normal().get(cx)
        });
        
        Self::text_style_body().set(cx, TextStyle {
            font_size: 10.0 * opt.scale,
            height_factor: 2.0,
            line_spacing: 3.0,
            ..Theme::text_style_normal().get(cx)
        });
        
        Self::text_style_point().set(cx, TextStyle {
            font_size: 8.0 * opt.scale,
            line_spacing: 2.5,
            ..Theme::text_style_normal().get(cx)
        });
        
        Self::color_heading().set(cx, color("#e"));
        Self::color_body().set(cx, color("#b"));
        Self::layout_main().set(cx, Layout {
            padding: Padding {l: 10., t: 10., r: 10., b: 10.},
            new_line_padding: 15.,
            line_wrap: LineWrap::NewLine,
            ..Layout::default()
        });
    }
    
    pub fn handle_home_page(&mut self, cx: &mut Cx, event: &mut Event) {
        if let Event::Signal(sig) = event {
            if self.email_signal.is_signal(sig) {
                match sig.value {
                    HTTP_SEND_OK => {
                        self.email_state = EmailState::OkSending;
                    },
                    HTTP_SEND_FAIL => {
                        self.email_state = EmailState::ErrorSending;
                    },
                    _ => ()
                }
                self.view.redraw_view_area(cx);
            }
        }
        if let TextEditorEvent::Change = self.email_input.handle_text_input(cx, event) {
            let email = self.email_input.get_value();
            
            if email.len()> 0 && !email.find("@").is_some() {
                self.email_state = EmailState::Invalid
            }
            else if email.len()>0 {
                self.email_state = EmailState::Valid
            }
            else {
                self.email_state = EmailState::Empty
            }
            self.view.redraw_view_area(cx);
        }
        
        if let ButtonEvent::Clicked = self.send_mail_button.handle_normal_button(cx, event) {
            match self.email_state {
                EmailState::Valid | EmailState::ErrorSending => {
                    self.email_state = EmailState::Sending;
                    let email = self.email_input.get_value();
                    cx.http_send("POST", "/subscribe", "makepad.nl", 80, "text/plain", email.as_bytes(), self.email_signal);
                    self.view.redraw_view_area(cx);
                },
                _ => ()
            }
        }
        
        self.view.handle_scroll_bars(cx, event);
    }
    
    pub fn draw_home_page(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Self::layout_main().get(cx)).is_err() {return};
        
        let t = &mut self.text;
        
        t.color = Self::color_heading().get(cx);
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "Introducing Makepad\n");
        
        t.color = Self::color_body().get(cx);
        t.text_style = Self::text_style_body().get(cx);
        
        t.draw_text(cx, "\
            Makepad is a creative software development platform built around Rust. \
            We aim to make the creative software development process as fun as possible! \
            To do this we will provide a set of visual design tools that modify your application in real time, \
            as well as a library ecosystem that allows you to write highly performant multimedia applications.\n");
        
        t.draw_text(cx, "\
            As we're working towards our first public alpha version, \
            you'll be able to see our final steps towards it here. \
            The alpha version of Makepad Basic will show off the development platform, \
            but does not include the visual design tools or library ecosystem yet.\n");
        
        self.email_input.draw_text_input(cx);
        self.send_mail_button.draw_normal_button(cx, match self.email_state {
            EmailState::Empty => "Enter email address to subscribe to our newsletter",
            EmailState::Invalid => "Email adress invalid",
            EmailState::Valid => "Click here to subscribe to our newsletter",
            EmailState::Sending => "Submitting your email adress..",
            EmailState::ErrorSending => "Could not send your email adress, please retry!",
            EmailState::OkSending => "Thank you, we'll keep up informed!"
        });
        cx.turtle_new_line();
        
        t.draw_text(cx, "\
            the web build of Makepad does not feature any compiler integration. \
            If you want to be able to compile code, you have to install Makepad locally.\n");
        
        t.draw_text(cx, "\
            The Makepad development platform and library ecosystem are MIT licensed,\
            and will be available for free as part of Makepad Basic. \
            In the near future, we will also introduce Makepad Pro, \
            which will be available as a subscription model. \
            Makepad Pro will include the visual design tools. Because the library ecosystem is MIT licensed, \
            all applications made with the Pro version are entirely free licensed.\n");
        
        t.draw_text(cx, "Features:\n");
        
        t.text_style = Self::text_style_point().get(cx);
        t.draw_text(cx, "\
            -Compiles natively to Linux, MacOS, and Windows.\n\
            -Compiles to WebAssembly for demo purposes (see caveats below).\n\
            -Built-in HTTP server with live reload support for WebAssembly development.\n\
            -Code editor with live code folding (press alt).\n\
            -Log viewer with a virtual viewport, that can handle printlns in an infinite loop.\n\
            -Dock panel system / file tree.\n\
            -Rust compiler integration, with errors/warning in the IDE.\n\
        ");
        
        /*
        cx.begin_turtle(Layout{
            walk:Walk::wh(Width::Fix(250.0), Height::Fix(250.)),
            ..Layout::default()
        }, Area::Empty);
        self.editor.code_editor.class = Self::my_code_editor();
        self.editor.draw_rust_editor(cx, &mut self.text_buffer);
        cx.end_turtle(Area::Empty);  
        */
        cx.turtle_new_line();
        self.shadow.draw_shadow_top(cx);
        self.view.end_view(cx);
    }
}