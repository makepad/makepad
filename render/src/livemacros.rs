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

#[macro_export]
macro_rules!uid {
    () => {{
        struct Unique {};
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
macro_rules!live_draw_input {
    ( $ cx: ident, $ path: path) => {
        $cx.live_styles.add_live_draw_input(
            live_str_to_id(module_path!(), stringify!( $ path)),
            stringify!( $ path),
            $path :: draw_input_def()
        )
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
macro_rules!live_color {
    ( $ cx: ident, $ path: path) => {
        $ cx.live_styles.get_color(
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
