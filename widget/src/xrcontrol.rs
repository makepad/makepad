
use makepad_render::*;
use crate::widgetstyle::*;

#[derive(Clone)]
pub struct XRControl {
    pub view: View,
    pub input_shape: Cube,
    pub animator: Animator,
    pub last_xr_update: Option<XRUpdateEvent>,
    pub _left_input_area: Area,
    pub _right_input_area: Area,
}

pub enum XRControlEvent {
    None
}

impl XRControl {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: View::new(cx),
            input_shape: Cube::new(cx),
            animator: Animator::default(),
            last_xr_update: None,
            _left_input_area: Area::Empty,
            _right_input_area: Area::Empty,
        }
    }
    
    pub fn shader_shape() -> ShaderId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        // lets define the shader
        let sg = Cube::def_cube_shader().compose(shader!{"
        "});
        cx.platform.from_wasm.log(&format!(
            "{} {}",
            sg.geometry.indices.len(),
            sg.geometry.vertices.len()
        ));
        Self::shader_shape().set(cx, sg);
    }
    
    pub fn handle_xr_control(&mut self, cx: &mut Cx, xr_event: &XRUpdateEvent) -> Vec<Event> {
        // lets set the left_input matrix
        let left_matrix = Mat4::from_transform(xr_event.left_input.grip);
        let right_matrix = Mat4::from_transform(xr_event.right_input.grip);
        self.view.set_view_transform(cx, &Mat4::identity());
        //cx.platform.from_wasm.log(&format!("{:?}", left_matrix));
        self.last_xr_update = Some(xr_event.clone());
        self._left_input_area.write_mat4(cx, Cube::transform(), &left_matrix);
        self._right_input_area.write_mat4(cx, Cube::transform(), &right_matrix);
        Vec::new()
    }
    
    pub fn draw_xr_control(&mut self, cx: &mut Cx) {
        // if let Some(xr_event) = &self.last_xr_update{
        if !self.view.begin_view(cx, Layout::abs_origin_zero()).is_ok() {
            return
        };
        self.input_shape.shader = Self::shader_shape().get(cx);
        //self.animator.init(cx, | cx | Self::anim_default().get(cx));
        
        let left_matrix = Mat4::identity(); //from_transform(xr_event.left_input.grip);
        self._left_input_area = self.input_shape.draw_cube(cx, &left_matrix).into();
        let right_matrix = Mat4::identity(); //from_transform(xr_event.left_input.grip);
        self._right_input_area = self.input_shape.draw_cube(cx, &right_matrix).into();
        
        //self.animator.set_area(cx, self._bg_area);
        self.view.end_view(cx);
        // }
    }
}
