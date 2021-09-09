use makepad_render::*;  

fn main(){
    main_app!(BareExampleApp);
}

/*
trait LiveComponent{
    fn new_live(cx:&mut Cx, id:&[Id])->Self;
    fn new_default(cx:&mut Cx)->Self;
}

// this is a normal 'Rust' component
impl LiveComponent for BareExampleApp{
    fn new_live(cx:&mut Cx, base:LiveNode, id:&[Id]){
    }
    
    fn live_update(&mut self, cx:&mut Cx, event:&mut Event){
        // thise are all default impls
    }

    // these 3 are generated
    fn new_default(cx: &mut Cx){
    }
    
    fn new_from_node(cx:&mut Cx, node:FullNodePtr)->Self{
    }
    
    // this updates self against a live structure. also can use animation
    fn update_from_node(&mut self, cx:&mut Cx, node:FullNodePtr){
    }
}*/

#[derive(Clone)]
pub struct BareExampleApp {
    //live: LiveNode, // for every component this always points to the node we deserialized from
    window: Window,
    pass: Pass,
    color_texture: Texture,
    main_view: View,
    //quad: ButtonQuad,
    //buttons: Vec<Button>,
    //design: Design
}

// what you want is putting down an indirection in the style-sheet for a codefile.

register_live!{ // alrighty. we always map application structures to a live struct
    design: render::Design{
        button: widgets::Button{ // ok so how does this thing find 'Button
        }
        children:[button]
    }
}
 
impl BareExampleApp {
    pub fn new(cx:&mut Cx)->Self{
        Self{
            window:Window::new(cx),
            pass:Pass::default(),
            color_texture:Texture::new(cx),
            main_view:View::new(),
        }
    }
    pub fn style(_cx: &mut Cx) { 
        // ok so our problem is targetting things.
        //self::ButtonQuad::register(cx);
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx){
    }
    
    pub fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        // this live updates an object
        //self.live_update(cx, event);
        
        // ok so what if we return a UIComponent.
        
        //self.buttons.push(Button::new_live(cx, self.live, id!(self::button)));
        // this deserialises SomeButton. okay great
        // now these things have nested 'variants'. 
        // so this button gets to run tweens on its own structure
         
        //let x = Bla::new(cx, id!(self::BlaThing));
        
        /*while let Some(action) = self.live.handle_live(cx, event){
            match action.id(){
                id!(self::MyUI::Button) => match action.cast::<ButtonAction>(){
                    Some(ButtonAction::Clicked) => self.myui_button_clicked(cx);
                }
            }
        }
        */
        
        match event {
            Event::Construct => {
                
            },
            Event::FingerMove(_fm) => {
                //self.count = fm.abs.x * 0.01;
            },
            _ => ()
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {

        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Vec4::color("700")));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
/*
        while let Some(custom) = self.live.draw_live(cx){
            match custom.id_path{
            }
        }*/
/*
            self.quad.counter = 0.;
            self.quad.begin_many(cx);
            
            self.quad.counter = 0.;
            self.quad.some = 0.;
            
            for i in 0..1000 {  
                let v = 0.5 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                let x = 400. + (v + self.count).sin() * 400.;
                let y = 400. + (v * 1.12 + self.count * 18.).cos() * 400.;
                self.quad.draw_quad_abs(cx, Rect {pos: vec2(x, y), size: vec2(10., 10.0)});
            }
            self.quad.end_many(cx);
            self.count += 0.001;
            self.main_view.redraw_view(cx);*/
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}

