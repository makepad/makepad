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

pub enum SvgPathOp {
    MoveTo(DVec2),
    LineTo(DVec2),
    Cubic {c1: DVec2, c2: DVec2, p: DVec2},
    Quadratic {c1: DVec2, p: DVec2},
    Close
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
    
    fn parse_svg_path(path: &[u8]) -> Result<Vec<SvgPathOp>, String> {
        #[derive(Debug)]
        enum Cmd {
            Unknown,
            Move(bool),
            Hor(bool),
            Vert(bool),
            Line(bool),
            Cubic(bool),
            Quadratic(bool),
            Close
        }
        impl Default for Cmd {fn default() -> Self {Self::Unknown}}
        
        #[derive(Default)]
        struct ParseState {
            cmd: Cmd,
            expect_nums: usize,
            nums: [f64; 8],
            num_count: usize,
            last_pt: DVec2,
            out: Vec<SvgPathOp>,
            num_state: Option<NumState>
        }
        
        struct NumState {
            num: f64,
            num_div: f64,
            num_dot: bool,
        }
        
        impl NumState {
            fn new(v: f64) -> Self {Self {num: v, num_div: 1.0, num_dot: false}}
            fn finalize(self) -> f64 {if self.num_dot {self.num / self.num_div}else {self.num}}
            fn add_digit(&mut self, digit: f64) {
                self.num *= 10.0;
                self.num += digit;
                if self.num_dot {
                    self.num_div *= 10.0
                }
            }
        }
        
        impl ParseState {
            fn next_cmd(&mut self, cmd: Cmd) -> Result<(), String> {
                self.finalize_cmd()?;
                self.expect_nums = match cmd {
                    Cmd::Unknown => panic!(),
                    Cmd::Move(_) => 2,
                    Cmd::Hor(_) => 1,
                    Cmd::Vert(_) => 1,
                    Cmd::Line(_) => 2,
                    Cmd::Cubic(_) => 8,
                    Cmd::Quadratic(_) => 6,
                    Cmd::Close => 0
                };
                self.cmd = cmd;
                Ok(())
            }
            
            fn add_digit(&mut self, digit: f64) -> Result<(), String> {
                if let Some(num_state) = &mut self.num_state {
                    num_state.add_digit(digit);
                }
                else {
                    if self.expect_nums == self.num_count {
                        self.finalize_cmd()?;
                    }
                    if self.expect_nums == 0{
                        return Err(format!("Unexpected digit"));
                    }
                    self.num_state = Some(NumState::new(digit))
                }
                Ok(())
            }
            
            fn add_dot(&mut self) -> Result<(), String> {
                if let Some(num_state) = &mut self.num_state {
                    if num_state.num_dot{
                        return Err(format!("Unexpected ."));
                    }
                    num_state.num_dot = true;
                }
                else {
                    return Err(format!("Unexpected ."));
                }
                Ok(())
            }
            
            fn finalize_num(&mut self) {
                if let Some(num_state) = self.num_state.take() {
                    self.nums[self.num_count] = num_state.finalize();
                    self.num_count += 1;
                }
            }
            
            fn finalize_cmd(&mut self) -> Result<(), String> {
                self.finalize_num();
                if self.expect_nums != self.num_count {
                    return Err(format!("SVG Path command {:?} expected {} points, got {}", self.cmd, self.expect_nums, self.num_count));
                }
                match self.cmd {
                    Cmd::Unknown => (),
                    Cmd::Move(abs) => {
                        if abs {
                            self.last_pt = dvec2(self.nums[0], self.nums[1]);
                        }
                        else{
                            self.last_pt += dvec2(self.nums[0], self.nums[1]);
                        }
                        self.out.push(SvgPathOp::MoveTo(self.last_pt));
                    },
                    Cmd::Hor(abs)=>{
                        if abs{
                            self.last_pt = dvec2(self.nums[0], self.last_pt.y);
                        }
                        else{
                            self.last_pt += dvec2(self.nums[0], 0.0);
                        }
                        self.out.push(SvgPathOp::LineTo(self.last_pt));
                    }
                    Cmd::Vert(abs)=>{
                        if abs{
                            self.last_pt = dvec2(self.last_pt.x, self.nums[0]);
                        }
                        else{
                            self.last_pt += dvec2(0.0, self.nums[0]);
                        }
                        self.out.push(SvgPathOp::LineTo(self.last_pt));
                    }
                    Cmd::Line(abs)=>{
                        let pt = dvec2(self.nums[0], self.nums[1]);
                        if abs{
                            self.last_pt = pt;
                        }
                        else{
                            self.last_pt += pt;
                        }
                        self.out.push(SvgPathOp::LineTo(self.last_pt));
                    },
                    Cmd::Cubic(abs)=> {
                        let pt = dvec2(self.nums[4], self.nums[5]);
                        if abs{
                            self.last_pt = pt;
                        }
                        else{
                            self.last_pt += pt;
                        }
                        self.out.push(SvgPathOp::Cubic{
                            c1: dvec2(self.nums[0], self.nums[1]),
                            c2: dvec2(self.nums[2], self.nums[3]),
                            p: self.last_pt
                        });
                    },
                    Cmd::Quadratic(abs) => {
                        let pt = dvec2(self.nums[2], self.nums[3]);
                        if abs{
                            self.last_pt = pt;
                        }
                        else{
                            self.last_pt += pt;
                        }
                        self.out.push(SvgPathOp::Quadratic{
                            c1: dvec2(self.nums[0], self.nums[1]),
                            p: self.last_pt
                        });                        
                    }
                    Cmd::Close => {
                        self.out.push(SvgPathOp::Close);
                    }
                }
                self.num_count = 0;
                Ok(())
            }
        }
        
        let mut state = ParseState::default();
        for i in 0..path.len(){
            match path[i] as char {
                'M' => state.next_cmd(Cmd::Move(true))?,
                'm' => state.next_cmd(Cmd::Move(false))?,
                'Q' => state.next_cmd(Cmd::Quadratic(true))?,
                'q' => state.next_cmd(Cmd::Quadratic(false))?,
                'C' => state.next_cmd(Cmd::Cubic(true))?,
                'c' => state.next_cmd(Cmd::Cubic(false))?,
                'H' => state.next_cmd(Cmd::Hor(true))?,
                'h' => state.next_cmd(Cmd::Hor(false))?,
                'V' => state.next_cmd(Cmd::Vert(true))?,
                'v' => state.next_cmd(Cmd::Vert(false))?,
                'L' => state.next_cmd(Cmd::Line(true))?,
                'l' => state.next_cmd(Cmd::Line(false))?,
                'Z' | 'z' => state.next_cmd(Cmd::Close)?,
                '0'..='9' => {
                    state.add_digit((path[i] - '0' as u8) as f64)?;
                }
                '.' => {
                    state.add_dot()?;
                }
                _ => ()
            }
        }
        Ok(state.out)
    }
    
    pub fn get_icon_pos(&mut self, _fract: Vec2, size: Vec2, path: &str) -> CxIconAtlasEntry {
        let _ = Self::parse_svg_path(path.as_bytes());
        // lets parse this thing
        self.alloc.alloc_icon(size.x as f64, size.y as f64)
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
            t1: dvec2(tx1, ty1).into(),
            t2: dvec2(tx1 + (w / self.texture_size.x), ty1 + (h / self.texture_size.y)).into()
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
    pub fn lazy_construct_icon_atlas(cx: &mut Cx) {
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxIconAtlasRc>() {
            
            let draw_atlas = CxDrawIconAtlas::new(cx);
            let texture_id = draw_atlas.atlas_texture.texture_id();
            cx.set_global(CxDrawIconAtlasRc(Rc::new(RefCell::new(draw_atlas))));
            
            let atlas = CxIconAtlas::new(texture_id);
            cx.set_global(CxIconAtlasRc(Rc::new(RefCell::new(atlas))));
        }
    }
    
    pub fn reset_icon_atlas(cx: &mut Cx) {
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
    pub t1: Vec2,
    pub t2: Vec2,
}

