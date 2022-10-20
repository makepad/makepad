use crate::makepad_draw_2d::*;


live_design!{
    import makepad_draw_2d::shader::std::*;
    
    Slides:[
        JsToRust,
        CodeEditorDogFood,
        StylingWithShaders,
        PerfAndExpressiveness,
        InlineWidgets,
        RunsOnWebViaWasm,
        DesktopNative2mb,
        RustAndLiveDSL,
        BuildTimeFast,
        Designtooling,
        EditItself,
        ArVr,
        OpensourceMIT,
        Documented6Mo,
        RustMacroReflection
    ]
    
    ShaderView= {{ShaderView}} {
        bg_quad: {
            instance hover: 0.0
            instance pressed: 0.0
            
            fn dist(self, pos:vec2)->float{
                let fx = 1.0-pow(1.2-self.pressed*0.1*pos.y - sin(pos.x * PI), 4.8);
                let fy = 1.0-pow(1.2-self.pressed*0.2 - cos(pos.y * 0.5*PI), 10.8)
                return fx+fy
            }
            
            fn pixel(self) -> vec4 {
                let delta = 0.01;
                let d = self.dist(self.pos+vec2(0,0))
                let dy = self.dist(self.pos+vec2(0,delta))
                let dx = self.dist(self.pos+vec2(delta,0))
                let normal = normalize(cross(vec3(delta,0,dx-d), vec3(0,delta,dy-d)))
                let light = normalize(vec3(0.5,0.5,0.5))
                let diff = pow(max(dot(light, normal),0.),5.0)
                return mix(#00, #ff, diff)
            }
        }
        
        pad: vec2(10., 10.)
        
        size: vec2(200., 200)
        
        state:{
            hover = {
                default:off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {bg_quad: {pressed: 0.0, hover: 0.0}}
                }
                
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {bg_quad: {pressed: 0.0, hover: 1.0}}
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {bg_quad: {pressed: 1.0, hover: 1.0}}
                }
            }
        }
    }
}


#[derive(Live, LiveHook)]
pub struct ShaderView {
    bg_quad: DrawQuad,
    
    size: DVec2,
    pad: DVec2,

    state: State
}


impl ShaderView {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.bg_quad.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                self.animate_state(cx, id!(hover.pressed));
            },
            Hit::FingerUp(fe) => {
                if fe.is_over && fe.digit.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else {
                    self.animate_state(cx, id!(hover.off));
                }
            }
            Hit::FingerMove(_) => {
            },
            _ => ()
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.bg_quad.area().redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        let pos = cx.turtle().pos() + self.pad;
        self.bg_quad.draw_abs(cx, Rect {pos, size: self.size})
    }
}