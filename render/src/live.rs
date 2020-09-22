use crate::cx::*;

impl Cx {
    pub fn add_live_body(&mut self, live_body: LiveBody) {
        let mut shader_alloc_start = self.shaders.len();
        if let Err(err) = self.live_styles.add_live_body(live_body, &mut shader_alloc_start) {
            eprintln!("{}:{} {} - {}", err.file, err.line, err.column, err.message);
        }
        // lets add the required CxShader slots
        for _ in self.shaders.len()..shader_alloc_start {
            self.shaders.push(CxShader::default());
        }
        // also add texture slots/require textures
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
macro_rules!live {
    ( $ cx: ident, $ code: literal) => {
        $ cx.add_live_body(LiveBody {
            file: file!().to_string().replace("\\", ","),
            module_path: module_path!().to_string(),
            line: line!() as usize,
            column: column!() as usize,
            code: $ code.to_string()
        })
    }
}

#[macro_export]
macro_rules!live_id {
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
