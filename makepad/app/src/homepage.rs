use makepad_render::*;
use makepad_widget::*;

#[derive(Clone)]
pub struct HomePage {
    pub view: ScrollView,
    pub shadow: ScrollShadow,
    pub text: DrawText,
    pub bg: DrawColor,
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
            view: ScrollView::new_standard_hv(cx),
            bg: DrawColor::new(cx, default_shader!()),
            text: DrawText::new(cx, default_shader!()),
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
    
    pub fn style(cx: &mut Cx) {
        
        live_body!(cx, r#"
            self::color_bg: #2;
            
            self::text_style_heading: TextStyle {
                font_size: 28.0,
                line_spacing: 2.0,
                ..makepad_widget::widgetstyle::text_style_normal
            }
            
            self::text_style_body: TextStyle {
                font_size: 10.0,
                height_factor: 2.0,
                line_spacing: 3.0,
                ..makepad_widget::widgetstyle::text_style_normal
            }
            
            self::text_style_point: TextStyle {
                font_size: 8.0,
                line_spacing: 2.5,
                ..makepad_widget::widgetstyle::text_style_normal
            }
            
            self::text_color: #b;
            
            self::layout_main: Layout {
                padding: {l: 10., t: 10., r: 10., b: 10.},
                new_line_padding: 15.,
                line_wrap: MaxSize(550.),
            }
        "#)
    }
    
    pub fn handle_home_page(&mut self, cx: &mut Cx, event: &mut Event) {
        if let Event::Signal(sig) = event {
            if let Some(statusses) = sig.signals.get(&self.email_signal) {
                for status in statusses {
                    if *status == Cx::status_http_send_ok() {
                        self.email_state = EmailState::OkSending;
                    }
                    else if *status == Cx::status_http_send_fail() {
                        self.email_state = EmailState::ErrorSending;
                    }
                    self.view.redraw_view(cx);
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
            self.view.redraw_view(cx);
        }
        
        if let ButtonEvent::Clicked = self.send_mail_button.handle_normal_button(cx, event) {
            match self.email_state {
                EmailState::Valid | EmailState::ErrorSending => {
                    self.email_state = EmailState::Sending;
                    let email = self.email_input.get_value();
                    cx.http_send("POST", "/subscribe", "http", "makepad.nl", 80, "text/plain", email.as_bytes(), self.email_signal);
                    self.view.redraw_view(cx);
                },
                _ => ()
            }
        }
        
        for text_input in self.example_texts.iter() {
            text_input.handle_text_input(cx, event);
        }
        
        self.view.handle_scroll_view(cx, event);
    }
    
    pub fn draw_home_page(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, live_layout!(cx, self::layout_main)).is_err() {return};

        self.bg.color = live_vec4!(cx, self::color_bg);
        self.bg.draw_quad_rel(cx, cx.get_turtle_rect());//let inst = self.bg.begin_quad_fill(cx);
        self.bg.area().set_do_scroll(cx, false, false);

        let t = &mut self.text;
        
        t.color = live_vec4!(cx, self::text_color);
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "Introducing Makepad\n");
        
        t.text_style = live_text_style!(cx, self::text_style_body);
        t.draw_text_walk(cx, "\
            Makepad is a new VR, web and native collaborative shader programming environment. \
            It will support many different shader modes including many vertex-shaders \
            besides the well known shader toy SDF programs. This makes shader coding possible \
            for more compute constrained environments like VR goggles or mobiles.\
            Try makepad now on a Quest in the quest browser, click the goggles top right of the UI. Try touching the leaves of the tree with your hands! Magic!\n");
        

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
        
        t.draw_text_walk(cx, "\
            The Makepad development platform and library ecosystem are MIT licensed, \
            for the Quest and in the future iOS we will provide paid, native versions, \
            \n");
        
        t.draw_text_walk(cx, "\
            We are still building the collaborative backend, so for now you can simply play with the shader code\
            \n");
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "How to install the native version\n");
        
        t.text_style = live_text_style!(cx, self::text_style_body);
        t.draw_text_walk(cx, "\
            On all platforms first install Rust. \
            On windows feel free to ignore the warnings about MSVC, makepad uses the gnu chain. \
            Copy this url to your favorite browser.\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx, "\
            https://www.rust-lang.org/tools/install");
        cx.turtle_new_line();
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "MacOS\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx, "\
            git clone https://github.com/makepad/makepad\n\
            cd makepad\n\
            tools/macos_rustup.sh\n\
            cargo run -p makepad --release");
        cx.turtle_new_line();
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "Windows\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx, "\
            Clone this repo using either gitub desktop or commandline: https://github.com/makepad/makepad\n\
            Open a cmd.exe in the directory you just cloned. Gh desktop makes: Documents\\Github\\makepad\n\
            tools\\windows_rustup.bat\n\
            cargo run -p makepad --release --target x86_64-pc-windows-gnu");
        cx.turtle_new_line();
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "Linux\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx, "\
            git clone https://github.com/makepad/makepad\n\
            cd makepad\n\
            tools/linux_rustup.sh\n\
            cargo run -p makepad --release");
        cx.turtle_new_line();
        
        t.text_style = live_text_style!(cx, self::text_style_heading);
        t.draw_text_walk(cx, "Troubleshooting\n");
        
        self.example_texts.get_draw(cx).draw_text_input_static(cx, "\
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
        
        //self.bg.end_quad_fill(cx, inst); 
        self.view.end_view(cx);
    }
}
