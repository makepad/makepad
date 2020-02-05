use makepad_render::*;
use makepad_widget::*;

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
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: ScrollView::new(cx),
            text: Text::new(cx),
            shadow: ScrollShadow::new(cx),
            send_mail_button: NormalButton::new(cx),
            email_signal: cx.new_signal(),
            email_input: TextInput::new(cx, TextInputOptions {
                multiline: false,
                read_only: false,
                empty_message: "Enter email".to_string()
            }),
            email_state: EmailState::Empty,
            example_texts: ElementsCounted::new(
                TextInput::new(cx, TextInputOptions {
                    multiline: true,
                    read_only: true,
                    empty_message: "".to_string()
                })
            ),
        }
    }
    
    pub fn text_color() -> ColorId {uid!()}
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
        
        Self::text_color().set(cx, color("#b"));
        Self::layout_main().set(cx, Layout {
            padding: Padding {l: 10., t: 10., r: 10., b: 10.},
            new_line_padding: 15.,
            line_wrap: LineWrap::MaxSize(550.),
            ..Layout::default()
        });
    }
    
    pub fn handle_home_page(&mut self, cx: &mut Cx, event: &mut Event) {
        if let Event::Signal(sig) = event {
            if let Some(statusses) = sig.signals.get(&self.email_signal) {
                for status in statusses{
                    if *status == Cx::status_http_send_ok(){
                        self.email_state = EmailState::OkSending;
                    }
                    else if *status == Cx::status_http_send_fail(){
                        self.email_state = EmailState::ErrorSending;
                    }
                    self.view.redraw_view_area(cx);
                }
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
                    cx.http_send("POST", "/subscribe", "http", "makepad.nl", 80, "text/plain", email.as_bytes(), self.email_signal);
                    self.view.redraw_view_area(cx);
                },
                _ => ()
            }
        }
        
        for text_input in self.example_texts.iter(){
            text_input.handle_text_input(cx, event);
        }
        
        self.view.handle_scroll_bars(cx, event);
    }
    
    pub fn draw_home_page(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Self::layout_main().get(cx)).is_err() {return};
        
        let t = &mut self.text;
        
        t.color = Self::text_color().get(cx);
        
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "Introducing Makepad\n");
        
        t.text_style = Self::text_style_body().get(cx);
        t.draw_text(cx, "\
            Makepad is a creative software development platform built around Rust. \
            We aim to make the creative software development process as fun as possible! \
            To do this we will provide a set of visual design tools that modify your \
            application in real time, as well as a library ecosystem that allows you to \
            write highly performant multimedia applications. Please note the following text \
            input doesn't work on mobile-web yet. We also won't email you a confirmation right now, that will follow later.\n");
        
        
        self.email_input.draw_text_input(cx);
        self.send_mail_button.draw_normal_button(cx, match self.email_state {
            EmailState::Empty => "Sign up for our newsletter here.",
            EmailState::Invalid => "Email adress invalid",
            EmailState::Valid => "Click here to subscribe to our newsletter",
            EmailState::Sending => "Submitting your email adress..",
            EmailState::ErrorSending => "Could not send your email adress, please retry!",
            EmailState::OkSending => "Thank you, we'll keep you informed!"
        });
        cx.turtle_new_line();
        
        t.draw_text(cx, "\
            The Makepad development platform and library ecosystem are MIT licensed, \
            and will be available for free as part of Makepad Basic. In the near future, \
            we will also introduce Makepad Pro, which will be available as a subscription \
            model. Makepad Pro will include the visual design tools. Because the library \
            ecosystem is MIT licensed, all applications made with the Pro version are \
            entirely free licensed.\n");
        
        t.draw_text(cx, "\
            Today, we launch an early alpha of Makepad Basic. This version shows off \
            the development platform, but does not include the visual design tools or \
            library ecosystem yet. It is intended as a starting point for feedback \
            from you! Although Makepad is primarily a native application, its UI \
            is perfectly capable of running on the web. Try browsing the source code and pressing alt \
            in a large code file!. To compile code yourself, you have to install \
            the native version. Right now makepad is set up compile a simple WASM example you run in a browser from a localhost url.\n");
        
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "How to use\n");
        
        t.text_style = Self::text_style_body().get(cx);
        t.draw_text(cx, "\
            After install (see below) you can open the following file in makepad, and when you change the rust code, \
            the browser should live reload the wasm application as you type.\
            \n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            open this file the makepad editor UI: main/makepad/examples/webgl_example_wasm/src/sierpinski.rs \n\
            open this url in your browser: http://127.0.0.1:8000/makepad/examples/webgl_example_wasm/");
        cx.turtle_new_line();
        
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "How to install\n");
        
        t.text_style = Self::text_style_body().get(cx);
        t.draw_text(cx, "\
            On all platforms first install Rust. \
            On windows feel free to ignore the warnings about MSVC, makepad uses the gnu chain. \
            Copy this url to your favorite browser.\n");

        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            https://www.rust-lang.org/tools/install");
        cx.turtle_new_line();
        
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "MacOS\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            git clone https://github.com/makepad/makepad\n\
            cd makepad\n\
            tools/macos_rustup.sh\n\
            cargo run -p makepad --release");
        cx.turtle_new_line();

        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "Windows\n");

        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            Clone this repo using either gitub desktop or commandline: https://github.com/makepad/makepad\n\
            Open a cmd.exe in the directory you just cloned. Gh desktop makes: Documents\\Github\\makepad\n\
            tools\\windows_rustup.bat\n\
            cargo run -p makepad --release --target x86_64-pc-windows-gnu");
        cx.turtle_new_line();

        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "Linux\n");

        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            git clone https://github.com/makepad/makepad\n\
            cd makepad\n\
            tools/linux_rustup.sh\n\
            cargo run -p makepad --release");
        cx.turtle_new_line();
        
        t.text_style = Self::text_style_heading().get(cx);
        t.draw_text(cx, "Troubleshooting\n"); 

        self.example_texts.get_draw(cx).draw_text_input_static(cx,"\
            Delete old settings unix: rm *.ron\n\
            Delete old settings windows: del *.ron\n\
            Make sure you are on master: git checkout master\n\
            Update rust: rustup update\n\
            Make sure you have wasm: rustup target add wasm32-unknown-unknown\n\
            Pull the latest: git pull\n\
            If gnu chain for some reason doesn't work on windows, use the msvc chain\n\
            Still have a problem? Report here: https://github.com/makepad/makepad/issues");
        cx.turtle_new_line();

        self.shadow.draw_shadow_top(cx);
        self.view.end_view(cx);
    }
}
