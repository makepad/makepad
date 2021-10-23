use makepad_render::*;  

main_app!(BareExampleApp);

pub struct BareExampleApp {
    //live: LiveNode, // for every component this always points to the node we deserialized from
    window: Window,
    pass: Pass, 
    color_texture: Texture,
    main_view: View,
    draw_quad: DrawQuad
}

// what you want is putting down an indirection in the style-sheet for a codefile.
live_body!{
    use makepad_render::drawquad::DrawQuad
    MyDrawQuad:DrawQuad{
    }
}

impl BareExampleApp {
    pub fn new(cx:&mut Cx)->Self{
        let mut dq = DrawQuad::live_new(cx);
        
        let live_ptr = cx.shader_registry.live_registry.live_ptr_from_path(
            ModulePath::from_str(&module_path!()).unwrap(),
            &[id!(MyDrawQuad)]
        ).unwrap();

        dq.live_update(cx, live_ptr);
        
        Self{
            window:Window::new(cx),
            pass:Pass::default(),
            color_texture:Texture::new(cx),
            main_view:View::new(),
            draw_quad:dq
        }
    }
    pub fn live_register(cx: &mut Cx) {
        cx.register_live_body(live_body());
    }
    
    pub fn myui_button_clicked(&mut self, _cx: &mut Cx){
    }
    
    pub fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        
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
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Vec4::color("000")));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            self.draw_quad.draw_quad_abs(cx, Rect{pos:Vec2{x:10.,y:10.},size:Vec2{x:100.,y:100.}});

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

