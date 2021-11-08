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
            
            self.normal_button.draw_normal_button(cx, "HELLO");
            // ok so we have 2 apply trees
            // one is static mem
            // the other one is dynamic
            
            
            /*
            self.canvas.draw(cx,nodes!{
                NormalButton{id:"mybutton", label:"HI"}
            })
            
            self.normal_button.apply_draw(cx, nodes!{label:"HELLO"});
            */
            // OK SO. we have a thing the designer modifies
            // and we have the API use of the thing
            // both events and doing 'multiples' of things
            // ok so. how do we iterate inside one of these things spawning 'n' items
            // or dynamically showing things
            // how should we interact with objects 'in' the canvas
            
            
            /*
            // ok so problem. how do we reference a string in this thing
            enum Overlay<'a>{
                Value{id:Id, value:f32},
                Str{id:Id, value:&'a str},
                String{id:Id, value:String},
                Subtree{id:Id, value:&'a [Overlay<'a>]}
            }
            
            let value = Overlay::Subtree{
                id:id!(hi),
                value:&[Overlay::Value{id:id!(x), value:1.0}, Overlay::Str{id:id!(y), value:&format!("HI")}]
            };*/
            
            // ok so. how about this live design is a kinda
            // dynamic interface. and you can only instance things from it
            /*
            trait ForceCast<T: std::any::Any + Sized = Self>{
                fn cast(&mut self, cx:&mut Cx)->&mut T;
            }
            
            let button:&mut NormalButton = self.live_design.apply(live!{
                MyButton{bg:{color:value}}
            }).downcast_mut().unwrap();
        
            // in design mode this spawns a 'new' empty if the live template doesnt match anything NormalButton
            // ok this looks great. However now 
            
            OK so we use the id:value system if its omitted its a counted u64
            
            // ok so this is the absolute prettiest i can get it
            let button:&mut NormalButton = self.live_design.get(cx, apply!{
                MyButton{id:u64, bg:{color:value}}
            }).cast(cx);
            
            // this draws directly with the new value overlay
            self.live_design.draw(cx, apply!{
                MyButton{id:u64, bg:{color:value} }
            });
            
            // ok next problem. How does a live design create a structured instance
            // and how does it work with this kind of manual iteration
            
            
            // ok now we have a normal button, but the live_design should be locked now
            */
            /*
            let mut s = String::new();
            let value_any = &mut s as &mut dyn std::any::Any;

            let s:&mut String = value_any.downcast_mut().unwrap();
            */
            /*
            match value_any.downcast_ref::<String>() {
                Some(as_string) => {
                    println!("String ({}): {}", as_string.len(), as_string);
                }
                None => {
                    println!("{:?}", s);
                }
            }*/
            /*
            if let Some(as_str) : Option<String>{
                
            }*/
            /*
            live!{self.live_design, NormalButton, {
                label:&format!("hello world")
            },{
                button.apply({
                    label:&format!("hello world")
                })
            }}
            
            live_design!{
                design: self.live_design,
                for i in 0..10{
                    button:{
                        label:&format!("hello world {i}", i)
                    }
                }
            }*/
            /*
            self.live_design.begin(cx);
            for i  in 0..10{
                // reading and writin valuse
                // how do i know gto 
                if let Some(button) = self.live_design.add(live!{
                    button:{
                        label:"hello world"
                    }
                }).cast::<NormalButton>(){
                    button.draw_button(cx)
                }
            }
            self.live_design.end(cx);
            
            while let Some(step) in self.live_design.next_step(cx) {
                // ok so what if we wanna draw N buttons based on data
                for i in 0..10{
                    let item = self.live_design.get_item(id!(normal_button));
                    
                }
                match step.id{
                    id!(something)=>step.apply(overlay!{
                        bg: {
                            color: ident
                        }
                    });
                }
                step.draw_element(cx);
                
                self.live_design.end_step(step);
            }
            */
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

