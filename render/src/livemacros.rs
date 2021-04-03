use crate::cx::*;
use crate::log;

impl Cx {
    pub fn process_live_styles_changes(&mut self) -> Vec<LiveBodyError> {
        let mut errors = Vec::new();
        self.live_styles.process_changed_live_bodies(&mut errors);
        self.live_styles.process_changed_deps(&mut errors);
        self.ensure_live_style_shaders_allocated();
        errors
    }
    
    pub fn init_live_styles(&mut self) {
        let errors = self.process_live_styles_changes();
        
        for error in &errors {
            eprintln!("{}", error);
        }
        if errors.len()>0 {
            panic!();
        }
    }
    
    pub fn ensure_live_style_shaders_allocated(&mut self) {
        for _ in self.shaders.len()..(self.live_styles.shader_alloc.len()) {
            self.shaders.push(CxShader::default());
        }
    }
    
    pub fn add_live_body(&mut self, live_body: LiveBody) {
        self.live_styles.add_live_body(live_body,);
    }
    
    pub fn process_live_style_errors(&self) {
        let mut ae = self.live_styles.live_access_errors.borrow_mut();
        for err in ae.iter() {
            log!("{}", err);
        }
        ae.truncate(0)
    }
}

pub trait DrawInputType {
    fn slots() -> usize;
    fn ty_expr() -> TyExpr;
    // this writes a value to the area (wether texture, uniform or instance)
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str);
    
    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized;
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized;
}

impl DrawInputType for f32 {
    fn slots() -> usize {1}
    
    fn ty_expr() -> TyExpr {
        TyLit::Float.to_ty_expr()
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, live_item_id, Ty::Float, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride] = self;
            }
        }
    }

    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_float(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_float(cx, live_item_id, time)
    }
}

impl DrawInputType for Vec2 {
    fn slots() -> usize {2}
    
    fn ty_expr() -> TyExpr {
        TyLit::Vec2.to_ty_expr()
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, live_item_id, Ty::Vec2, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
            }
        }
    }

    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec2(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec2(cx, live_item_id, time)
    }
}

impl DrawInputType for Vec3 {
    fn slots() -> usize {3}
    
    fn ty_expr() -> TyExpr {
        TyLit::Vec3.to_ty_expr()
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, live_item_id, Ty::Vec3, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
                wr.buffer[i * wr.stride + 2] = self.z;
            }
        }
    }


    fn last_animate( animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec3(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec3(cx, live_item_id, time)
    }

}

impl DrawInputType for Vec4 {
    fn slots() -> usize {4}
    
    fn ty_expr() -> TyExpr {
        TyLit::Vec4.to_ty_expr()
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, live_item_id, Ty::Vec4, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
                wr.buffer[i * wr.stride + 2] = self.z;
                wr.buffer[i * wr.stride + 3] = self.w;
            }
        }
    }


    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec4(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec4(cx, live_item_id, time)
    }

}

impl DrawInputType for Mat4 {
    fn slots() -> usize {16}
    
    fn ty_expr() -> TyExpr {
        TyLit::Mat4.to_ty_expr()
    }
    
    // find uniform, then find instance prop
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, live_item_id, Ty::Mat4, name) {
            for i in 0..wr.repeat {
                for j in 0..16 {
                    wr.buffer[i * wr.stride + j] = self.v[j];
                }
            }
        }
    }

    fn last_animate(_animator:&Animator, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }
    
    fn animate(_cx: &mut Cx, _animator:&mut Animator, _time:f64, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }

}

impl DrawInputType for Texture2D {
    fn slots() -> usize {0}
    
    fn ty_expr() -> TyExpr {
        TyLit::Texture2D.to_ty_expr()
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, live_item_id: LiveItemId, name: &str) {
        if let Some(u) = self.0 {
            area.write_texture_2d_id(cx, live_item_id, name, u as usize)
        }
    }

    fn last_animate(_animator:&Animator, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }
    
    fn animate(_cx: &mut Cx, _animator:&mut Animator, _time:f64, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }
}

#[macro_export]
macro_rules!write_draw_input {
    ( $ cx: ident, $ area: expr, $ path: path, $ value: expr) => {
        ( $ value).write_draw_input( $ cx, $ area, live_str_to_id(module_path!(), stringify!( $ path)), stringify!( $ path))
    }
}

#[macro_export]
macro_rules!uid {
    () => {{
        struct Unique {}
        std::any::TypeId::of::<Unique>().into()
    }};
}


#[macro_export]
macro_rules!live_body {
    ( $ cx: ident, $ code: literal) => {
        $ cx.add_live_body(LiveBody {
            file: file!().to_string().replace("\\", "/"),
            module_path: module_path!().to_string(),
            line: line!() as usize,
            column: column!() as usize,
            code: $ code.to_string(),
        })
    }
}

#[macro_export]
macro_rules!live_item_id {
    ( $ path: path) => {
        live_str_to_id(module_path!(), stringify!( $ path))
    }
}

#[macro_export]
macro_rules!live_style_begin {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.style_begin(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_style_end {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.style_end(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_shader {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_shader(
            live_str_to_id(module_path!(), stringify!( $ path)),
            live_location_hash(file!(), line!() as u64, column!() as u64),
            module_path!(),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!default_shader_overload {
    ( $ cx: ident, $ base: ident, $ path: path) => {
        $ cx.live_styles.get_default_shader_overload(
            $ base,
            live_str_to_id(module_path!(), stringify!( $ path)),
            module_path!(),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!default_shader {
    () => {
        Shader {shader_id: 0, location_hash: live_location_hash(file!(), line!() as u64, column!() as u64)}
    }
}

#[macro_export]
macro_rules!live_geometry {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_geometry(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_float {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_float(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_vec2 {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_vec2(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_vec3 {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_vec3(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_vec4 {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_vec4(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}


#[macro_export]
macro_rules!live_text_style {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_text_style(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!live_anim {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_anim(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}


#[macro_export]
macro_rules!live_walk {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_walk(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}


#[macro_export]
macro_rules!live_layout {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_layout(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path)
        )
    }
}
