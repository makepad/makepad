
use {
    crate::{
        makepad_platform::*,
        draw_list_2d::ManyInstances,
        cx_2d::*,
        turtle::*,
    },
    resvg::tiny_skia::{Pixmap, Transform},
    resvg::usvg::{Options, Tree, fontdb},
};

script_mod!{
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res
    
    mod.draw.DrawSvg = mod.std.set_type_default() do #(DrawSvg::script_shader(vm)){
        ..mod.draw.DrawQuad
        
        svg_texture: texture_2d(float)
        color: vec4(-1.0, -1.0, -1.0, -1.0)
        
        pixel: fn(){
            let c = self.svg_texture.sample(self.pos)
            if self.color.x >= 0.0 {
                return vec4(self.color.rgb * self.color.a * c.a, self.color.a * c.a)
            }
            return vec4(c.rgb * c.a, c.a)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSvg {
    #[live] pub svg: Option<ScriptHandleRef>,
    #[rust] pub many_instances: Option<ManyInstances>,
    #[rust] texture: Option<Texture>,
    #[rust] svg_loaded: bool,
    #[rust] svg_size: DVec2,
    #[deref] pub draw_super: DrawQuad,
    #[live(vec4(-1.0, -1.0, -1.0, -1.0))] pub color: Vec4f,
}

use crate::shader::draw_quad::DrawQuad;

impl DrawSvg {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.load_svg(cx);
        if let Some(ref texture) = self.texture {
            self.draw_super.draw_vars.texture_slots[0] = Some(texture.clone());
            // Use the SVG's natural size for Fit dimensions
            let walk = Walk {
                width: match walk.width {
                    Size::Fit { .. } => Size::Fixed(self.svg_size.x),
                    other => other,
                },
                height: match walk.height {
                    Size::Fit { .. } => Size::Fixed(self.svg_size.y),
                    other => other,
                },
                ..walk
            };
            self.draw_super.draw_walk(cx, walk)
        } else {
            // No SVG loaded, return empty rect
            Rect::default()
        }
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.load_svg(cx);
        if let Some(ref texture) = self.texture {
            self.draw_super.draw_vars.texture_slots[0] = Some(texture.clone());
            self.draw_super.draw_abs(cx, rect)
        }
        // If no texture, don't draw anything
    }
    
    fn load_svg(&mut self, cx: &mut Cx) {
        if self.svg_loaded {
            return;
        }
        self.svg_loaded = true;
        
        let Some(ref handle_ref) = self.svg else {
            return;
        };
        
        let handle = handle_ref.as_handle();
        
        // Try to get the resource, if not loaded yet, trigger load_all and try again
        let data = if let Some(data) = cx.get_resource(handle) {
            data
        } else {
            // Resource not loaded yet, trigger load
            cx.script_data.resources.load_all();
            match cx.get_resource(handle) {
                Some(data) => data,
                None => return,
            }
        };
        
        let svg_str = match std::str::from_utf8(&data) {
            Ok(s) => s,
            Err(_) => return,
        };
        
        let mut opt = Options::default();
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        opt.fontdb = std::sync::Arc::new(db);
        
        match Tree::from_str(svg_str, &opt) {
            Ok(tree) => {
                let size = tree.size().to_int_size();
                let width = 2 * size.width();
                let height = 2 * size.height();
                
                let Some(mut pixmap) = Pixmap::new(width, height) else {
                    return;
                };
                
                resvg::render(&tree, Transform::from_scale(2.0, 2.0), &mut pixmap.as_mut());
                let rgba_data = pixmap.data();
                
                let mut bgra_data = Vec::with_capacity((width * height) as usize);
                for chunk in rgba_data.chunks(4) {
                    let r = chunk[0] as u32;
                    let g = chunk[1] as u32;
                    let b = chunk[2] as u32;
                    let a = chunk[3] as u32;
                    
                    let pixel = (a << 24) | (r << 16) | (g << 8) | b;
                    bgra_data.push(pixel);
                }
                
                let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                    data: Some(bgra_data),
                    width: width as usize,
                    height: height as usize,
                    updated: TextureUpdated::Full,
                });
                
                self.texture = Some(texture);
                // Store logical size (half of rendered size since we render at 2x)
                self.svg_size = dvec2(width as f64 / 2.0, height as f64 / 2.0);
            }
            Err(e) => {
                log!("SVG error: {:?}", e);
            }
        }
    }
    
    pub fn svg_size(&self) -> Option<DVec2> {
        if self.texture.is_some() {
            Some(self.svg_size)
        } else {
            None
        }
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        self.load_svg(cx);
        if let Some(ref texture) = self.texture {
            self.draw_super.draw_vars.texture_slots[0] = Some(texture.clone());
        }
        self.draw_super.begin_many_instances(cx);
    }
    
    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        self.draw_super.end_many_instances(cx);
    }
    
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        self.draw_super.new_draw_call(cx);
    }
}
