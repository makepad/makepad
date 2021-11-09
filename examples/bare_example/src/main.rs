use makepad_render::*;
use makepad_widget::*;

live_register!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;
    use makepad_widget::normalbutton::NormalButton;
    
    App: Component {
        rust_type: {{BareExampleApp}}
        draw_quad: DrawColor {
            color: #f00
            fn pixel(self) -> vec4 {
                return mix(#f00, #0f0, self.geom_pos.y)
            }
        }
        
        draw_text: DrawText {
        }
        
        normal_button: NormalButton {
        }
        /*
        canvas: Canvas {
            button1: NormalButton {
                text: {color: #fff},
                margin: {right: 200.}
            }
            button2: NormalButton {
                text: {color: #f0f}
            }
            
            subcanvas: Canvas{
                label: DrawText{}
                text_input: TextInput{}
            }
            
            root_view: { // ok so. how is this going to work api wise
                layout:...
                children:[button1, button2, area]
            }
        }*/
    }
}

main_app!(BareExampleApp);

#[derive(LiveComponent, LiveComponentHooks)]
pub struct BareExampleApp {
    #[hidden(Window::new(cx))] window: Window,
    #[hidden()] pass: Pass,
    #[hidden(Texture::new(cx))] color_texture: Texture,
    #[hidden(View::new())] main_view: View,
    #[live()] draw_quad: DrawColor,
    #[live()] draw_text: DrawText,
    #[live()] normal_button: NormalButton
}

impl BareExampleApp {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
    }
    
    pub fn new(cx: &mut Cx) -> Self {
        let mut new = Self::live_new(cx);
        new.live_update(cx, cx.live_ptr_from_id(&module_path!(), id!(App)));
        new
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        self.normal_button.handle_normal_button(cx, event);
        /*
        while let (event) = self.live_interp.handle_all_things() {
            match event.id {
                id!(button1) if let (arg) = event.arg.cast_down::<ButtonEvent>() {
                    match arg {
                        ButtonEvent::Down => {
                            
                        }
                    }
                }
            }
        }
        */
        match event {
            Event::Construct => {
            },
            Event::FingerMove(_fm) => {
            },
            _ => ()
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Vec4::color("000")));
        
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            self.draw_quad.draw_quad_abs(cx, Rect {pos: Vec2 {x: 30., y: 30.}, size: Vec2 {x: 100., y: 100.}});
            self.draw_text.draw_text_abs(cx, Vec2 {x: 60., y: 60.}, "HELLO WORLD");
            
            let gen = gen!{
                text:{color: #x00f}
            };
            self.normal_button.apply(cx, gen);
            
            //println!("{:?}", self.normal_button.text.color);
            
            self.normal_button.draw_normal_button(cx, "HELLO");
            /*
            let mut out = Vec::new();
            let x = gen!{
                somevalue: Thing{x:1.0}
                NormalButton{id:1, label:(format!(".."))},
                NormalButton{id:"hello", label:(format!(".."))}
            };
            out.extend(x);
            */
            
            self.main_view.end_view(cx);
        }
        
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

