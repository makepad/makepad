pub use {
    std::{
        rc::Rc,
        cell::RefCell,
        io::prelude::*,
        fs::File,
        collections::HashMap,
    },
    crate::{
        shader::draw_trapezoid::DrawTrapezoidVector,
        makepad_platform::*,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        view::{ManyInstances, View, ViewRedrawingApi},
        geometry::GeometryQuad2D,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::*,
        makepad_vector::path::PathIterator,
    }
};

pub struct CxIconAtlas {
    pub texture_id: TextureId,
    pub clear_buffer: bool,
    pub entries: Vec<CxIconAtlasEntry>,
    pub alloc: CxIconAtlasAlloc
}

#[derive(Default)]
pub struct CxIconAtlasAlloc {
    pub texture_size: DVec2,
    pub xpos: f64,
    pub ypos: f64,
    pub hmax: f64,
    pub todo: Vec<CxIconAtlasTodo>,
}

#[derive(Default, Debug)]
pub struct CxIconAtlasTodo {
    pub subpixel_x_fract: f64,
    pub subpixel_y_fract: f64,
}

impl CxIconAtlas {
    pub fn new(texture_id: TextureId) -> Self {
        Self {
            texture_id,
            clear_buffer: false,
            entries: Vec::new(),
            alloc: CxIconAtlasAlloc {
                texture_size: DVec2 {x: 2048.0, y: 2048.0},
                xpos: 0.0,
                ypos: 0.0,
                hmax: 0.0,
                todo: Vec::new(),
            }
        }
    }
}
impl CxIconAtlasAlloc {
    pub fn alloc_icon(&mut self, w: f64, h: f64) -> CxIconAtlasEntry {
        if w + self.xpos >= self.texture_size.x {
            self.xpos = 0.0;
            self.ypos += self.hmax + 1.0;
            self.hmax = 0.0;
        }
        if h + self.ypos >= self.texture_size.y {
            println!("VECTOR ATLAS FULL, TODO FIX THIS {} > {},", h + self.ypos, self.texture_size.y);
        }
        if h > self.hmax {
            self.hmax = h;
        }
        
        let tx1 = self.xpos / self.texture_size.x;
        let ty1 = self.ypos / self.texture_size.y;
        
        self.xpos += w + 1.0;
        
        
        CxIconAtlasEntry {
            tx1: tx1,
            ty1: ty1,
            tx2: (tx1 + (w / self.texture_size.x)),
            ty2: (ty1 + (h / self.texture_size.y))
        }
    }
}

#[derive(Clone)]
pub struct CxIconAtlasRc(pub Rc<RefCell<CxIconAtlas >>);

impl CxIconAtlas {
    pub fn reset_vector_atlas(&mut self) {
        self.alloc.xpos = 0.;
        self.alloc.ypos = 0.;
        self.alloc.hmax = 0.;
        self.clear_buffer = true;
    }
    
    pub fn get_internal_atlas_texture_id(&self) -> TextureId {
        self.texture_id
    }
}

impl DrawTrapezoidVector {
    // atlas drawing function used by CxAfterDraw
    pub fn draw_vector(&mut self, _many: &mut ManyInstances) {
        
        /*
        let mut size = 1.0;
        let trapezoids = {
            
            //log_str(&format!("Serializing char {} {} {} {}", glyphtc.tx1 , cx.fonts_atlas.texture_size.x ,todo.subpixel_x_fract ,atlas_page.dpi_factor));
            let trapezoidate = self.trapezoidator.trapezoidate(
                glyph
                    .outline
                    .commands()
                    .map({
                    move | command | {
                        command.transform(
                            &AffineTransformation::identity()
                                .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
                                .uniform_scale(font_scale_pixels * size)
                                .translate(Vector::new(tx, ty))
                        )
                    }
                }).linearize(0.5),
            );
            if let Some(trapezoidate) = trapezoidate {
                trapezoids.extend_from_internal_iter(
                    trapezoidate
                );
            }
            trapezoids
        };
        for trapezoid in trapezoids {
            self.a_xs = Vec2 {x: trapezoid.xs[0], y: trapezoid.xs[1]};
            self.a_ys = Vec4 {x: trapezoid.ys[0], y: trapezoid.ys[1], z: trapezoid.ys[2], w: trapezoid.ys[3]};
            self.chan = i as f32;
            many.instances.extend_from_slice(self.draw_vars.as_slice());
        }*/
    }
}

#[derive(Clone)]
pub struct CxDrawIconAtlasRc(pub Rc<RefCell<CxDrawIconAtlas >>);

pub struct CxDrawIconAtlas {
    pub draw_trapezoid: DrawTrapezoidVector,
    pub atlas_pass: Pass,
    pub atlas_view: View,
    pub atlas_texture: Texture,
}

impl CxDrawIconAtlas {
    pub fn new(cx: &mut Cx) -> Self {
        
        let atlas_texture = Texture::new(cx);
        
        //cx.fonts_atlas.texture_id = Some(atlas_texture.texture_id());
        
        let draw_trapezoid = DrawTrapezoidVector::new_local(cx);
        // ok we need to initialize drawtrapezoidtext from a live pointer.
        Self {
            draw_trapezoid,
            atlas_pass: Pass::new(cx),
            atlas_view: View::new(cx),
            atlas_texture: atlas_texture
        }
    }
}

impl<'a> Cx2d<'a> {
    pub fn lazy_construct_icon_atlas(cx: &mut Cx){
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxIconAtlasRc>() {
            
            let draw_atlas = CxDrawIconAtlas::new(cx);
            let texture_id = draw_atlas.atlas_texture.texture_id();
            cx.set_global(CxDrawIconAtlasRc(Rc::new(RefCell::new(draw_atlas))));
            
            let atlas = CxIconAtlas::new(texture_id);
            cx.set_global(CxIconAtlasRc(Rc::new(RefCell::new(atlas))));
        }
    }
    
    pub fn reset_icon_atlas(cx:&mut Cx){
        if cx.has_global::<CxIconAtlasRc>() {
            let mut fonts_atlas = cx.get_global::<CxIconAtlasRc>().0.borrow_mut();
            fonts_atlas.reset_vector_atlas();
        }
    }
        
    pub fn draw_icon_atlas(&mut self) {
        let draw_atlas_rc = self.cx.get_global::<CxDrawIconAtlasRc>().clone();
        let mut _draw_atlas = draw_atlas_rc.0.borrow_mut();
        let atlas_rc = self.icon_atlas_rc.clone();
        let mut atlas = atlas_rc.0.borrow_mut();
        let _atlas = &mut*atlas;
        //let start = Cx::profile_time_ns();
        // we need to start a pass that just uses the texture
        /*
        if fonts_atlas.alloc.todo.len()>0 {
            self.begin_pass(&draw_fonts_atlas.atlas_pass);

            let texture_size = fonts_atlas.alloc.texture_size;
            draw_fonts_atlas.atlas_pass.set_size(self.cx, texture_size);
            
            let clear = if fonts_atlas.clear_buffer {
                fonts_atlas.clear_buffer = false;
                PassClearColor::ClearWith(Vec4::default())
            }
            else {
                PassClearColor::InitWith(Vec4::default())
            };
            
            draw_fonts_atlas.atlas_pass.clear_color_textures(self.cx);
            draw_fonts_atlas.atlas_pass.add_color_texture(self.cx, &draw_fonts_atlas.atlas_texture, clear);
            draw_fonts_atlas.atlas_view.begin_always(self);

            let mut atlas_todo = Vec::new();
            std::mem::swap(&mut fonts_atlas.alloc.todo, &mut atlas_todo);
            
            if let Some(mut many) = self.begin_many_instances(&draw_fonts_atlas.draw_trapezoid.draw_vars) {

                for todo in atlas_todo {
                    draw_fonts_atlas.draw_trapezoid.draw_vector(todo, &mut many);
                }
                
                self.end_many_instances(many);
            }
            draw_fonts_atlas.atlas_view.end(self);
            self.end_pass(&draw_fonts_atlas.atlas_pass);
        }*/
        //println!("TOTALT TIME {}", Cx::profile_time_ns() - start);
    }
}

#[derive(Clone, Copy)]
pub struct CxIconAtlasEntry {
    pub tx1: f64,
    pub ty1: f64,
    pub tx2: f64,
    pub ty2: f64,
}

