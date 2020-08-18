 
use makepad_render::*;
use crate::buttonlogic::*;
use crate::widgetstyle::*;

#[derive(Clone)]
pub struct XRHandController {
    pub view: View,
    pub shape: Cube,
    pub animator: Animator,
    pub _shape_area: Area,
}

pub enum XRHandControllerEvent{
    None
}

impl XRHandController {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: View::default(),
            shape: Cube::new(cx),
            animator: Animator::default(),
            _shape_area: Area::Empty,
        }
    }

    pub fn shader_shape() -> ShaderId{uid!()}

    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        // lets define the shader
        Self::shader_shape().set(cx, Cube::def_cube_shader().compose(shader!{"
        "}));
    }
    
    pub fn handle_xr_hand_controller(&mut self, cx: &mut Cx, event: &mut Event) -> XRHandControllerEvent {
        XRHandControllerEvent::None
    }
    
    pub fn draw_xr_hand_controller(&mut self, cx: &mut Cx, label: &str) {
        self.shape.shader = Self::shader_shape().get(cx);
        self.animator.init(cx, | cx | Self::anim_default().get(cx));
        
        let bg_inst = self.bg.begin_quad(cx, Self::layout_bg().get(cx));
        
        // i could do this
        
        bg_inst.push_last_float(cx, &self.animator, Self::hover());
        bg_inst.push_last_float(cx, &self.animator, Self::down());
        
        self.text.text_style = Self::text_style_label().get(cx);
        self.text.color = self.animator.last_color(cx, Text::color());
        
        self._text_area = self.text.draw_text(cx, label);
        
        self._bg_area = self.bg.end_quad(cx, bg_inst);
        self.animator.set_area(cx, self._bg_area);
    }
}
