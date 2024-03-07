pub use {
    std::{
        rc::Rc,
        cell::RefCell,
        io::prelude::*,
        fs::File,
        collections::HashMap,
    },
    makepad_html::*,
    crate::{
        
        shader::draw_trapezoid::DrawTrapezoidVector,
        makepad_platform::*,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        draw_list_2d::{ManyInstances, DrawList2d, RedrawingApi},
        geometry::GeometryQuad2D,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector, Point},
        makepad_vector::internal_iter::*,
        makepad_vector::path::{PathIterator, PathCommand},
    }
};

#[derive(Clone, Copy)]
pub struct CxIconSlot {
    pub t1: Vec2,
    pub t2: Vec2,
    pub chan: f32
}

#[derive(Clone)]
pub struct CxIconEntry {
    path_hash: CxIconPathHash,
    pos: DVec2,
    slot: CxIconSlot,
    args: CxIconArgs,
}

struct CxIconPathCommands {
    bounds: Rect,
    path: Vec<PathCommand>
}

impl<'a> InternalIterator for &CxIconPathCommands {
    type Item = PathCommand;
    fn for_each<F>(self, f: &mut F) -> bool
    where
    F: FnMut(PathCommand) -> bool,
    {
        for item in &self.path {
            if !f(item.clone()) {
                return false
            }
        }
        true
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct CxIconPathHash(LiveId);

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct CxIconEntryHash(LiveId);

pub struct CxIconAtlas {
    pub texture: Texture,
    pub clear_buffer: bool,
    svg_deps: HashMap<String, CxIconPathHash>,
    paths: HashMap<CxIconPathHash, Vec<CxIconPathCommands>>,
    entries: HashMap<CxIconEntryHash, CxIconEntry>,
    alloc: CxIconAtlasAlloc
}

#[derive(Default)]
pub struct CxIconAtlasAlloc {
    pub texture_size: DVec2,
    pub xpos: f64,
    pub ypos: f64,
    pub hmax: f64,
    pub todo: Vec<CxIconEntryHash>,
}

#[derive(Clone, Debug)]
pub struct CxIconArgs {
    pub linearize: f64,
    pub size: DVec2,
    pub translate: DVec2,
    pub subpixel: DVec2,
    pub scale: f64,
}

impl CxIconArgs {
    fn hash(&self) -> LiveId {
        LiveId::seeded()
            .bytes_append(&self.linearize.to_be_bytes())
            .bytes_append(&self.translate.x.to_be_bytes())
            .bytes_append(&self.translate.y.to_be_bytes())
            .bytes_append(&self.subpixel.x.to_be_bytes())
            .bytes_append(&self.subpixel.y.to_be_bytes())
            .bytes_append(&self.scale.to_be_bytes())
            .bytes_append(&self.size.x.to_be_bytes())
            .bytes_append(&self.size.y.to_be_bytes())
    }
}

impl CxIconAtlas {
    pub fn new(texture: Texture) -> Self {
        Self {
            texture,
            clear_buffer: false,
            entries: HashMap::new(),
            svg_deps: HashMap::new(),
            paths: HashMap::new(),
            alloc: CxIconAtlasAlloc {
                texture_size: DVec2 {x: 2048.0, y: 2048.0},
                xpos: 0.0,
                ypos: 0.0,
                hmax: 0.0,
                todo: Vec::new(),
            }
        }
    }
    
    pub fn parse_and_cache_path(&mut self, path_hash: CxIconPathHash, path: &[u8]) -> Option<(CxIconPathHash, Rect)> {
        match parse_svg_path(path) {
            Ok(path) => {
                let mut min = dvec2(f64::INFINITY, f64::INFINITY);
                let mut max = dvec2(-f64::INFINITY, -f64::INFINITY);
                fn bound(p: &Point, min: &mut DVec2, max: &mut DVec2) {
                    if p.x < min.x {min.x = p.x}
                    if p.y < min.y {min.y = p.y}
                    if p.x > max.x {max.x = p.x}
                    if p.y > max.y {max.y = p.y}
                }
                for cmd in &path {
                    match cmd {
                        PathCommand::MoveTo(p) => {bound(p, &mut min, &mut max)},
                        PathCommand::LineTo(p) => {bound(p, &mut min, &mut max)},
                        PathCommand::ArcTo(e, r, _, _, _) => {
                            // TODO: this is pretty rough
                            bound(&Point{x: e.x + r.x, y: e.y + r.y}, &mut min, &mut max);
                            bound(&Point{x: e.x - r.x, y: e.y - r.y}, &mut min, &mut max);
                        },
                        PathCommand::QuadraticTo(p1, p) => {
                            bound(p1, &mut min, &mut max);
                            bound(p, &mut min, &mut max);
                        },
                        PathCommand::CubicTo(p1, p2, p) => {
                            bound(p1, &mut min, &mut max);
                            bound(p2, &mut min, &mut max);
                            bound(p, &mut min, &mut max);
                        },
                        PathCommand::Close => ()
                    }
                }
                let bounds = Rect {pos: min, size: max - min};
                if let Some( foundpath) = self.paths.get_mut(&path_hash) {
                    foundpath.push(CxIconPathCommands {
                        bounds,
                        path
                    })
                }
                else
                {


                    self.paths.insert(path_hash,vec![ CxIconPathCommands {
                        bounds,
                        path
                    }]);
                }
                return Some((path_hash, bounds));
            }
            Err(e) => {
                log!("Error in SVG Path {}", e);
                return None
            }
        }
    }
   

    pub fn get_icon_bounds(&mut self, cx: &Cx, path_str: &Rc<String>, svg_dep: &Rc<String>) -> Option<(CxIconPathHash, Rect)> {
        if svg_dep.len() != 0 {
            // alright so. lets see if we have a path hash
            if let Some(path_hash) = self.svg_deps.get(svg_dep.as_str()) {
                if let Some(path) = self.paths.get(&path_hash) {
                    let mut bounds:Rect = path[0].bounds;
                    for i in 1..path.len(){
                        bounds = bounds.hull(path[i].bounds);
                    }
                    return Some((*path_hash, bounds))
                }
                return None
            }
            let path_hash = CxIconPathHash(LiveId(self.svg_deps.len() as u64));
            self.svg_deps.insert(svg_dep.as_str().to_string(), path_hash);
            // lets parse the path range out of the svg file
            match cx.get_dependency(svg_dep.as_str()) {
                Ok(data)=>{        

                    let mut errors = Some(Vec::new());
                    let svg_string = std::str::from_utf8(&data).unwrap();
                    let  doc = parse_html(svg_string, &mut errors);

                    if errors.as_ref().unwrap().len()>0{
                        log!("SVG parser returned errors {:?}", errors)
                    }
                    let mut node = doc.walk();
                    
                    while !node.empty(){
                        match node.open_tag_lc() 
                        {
                            some_id!(g)=>{
                                // do something with clipping/transform groups here.
                            }                            
                            some_id!(path)=>{
                                self.parse_and_cache_path(path_hash, node.find_attr_lc(live_id!(d)).unwrap().as_bytes());   
                                                        }         
                                                            
                            _=>()
                        }
                        match node.close_tag_lc() 
                        {
                            some_id!(g)=>
                            {
                                
                            }
                            _=>()
                        }
                        node = node.walk();
                    }
                    
                   
                    if let Some(path) = self.paths.get(&path_hash) {
                        let mut bounds:Rect = path[0].bounds;             
                        for i in 1..path.len(){
                            bounds = bounds.hull(path[i].bounds);
                        }
                        return Some((path_hash, bounds));              
                    }

                    println!("No SVG path tag found in svg file {}",path_str);
                    return None
                    
                }
                Err(_err)=>{
                    println!("Error in SVG file {}: {}",path_str, _err);
                    return None
                }
            }
        }
        if path_str.len() == 0 {
            return None
        }
        let path_hash = CxIconPathHash(LiveId(Rc::as_ptr(path_str) as u64));
        if let Some(path) = self.paths.get(&path_hash) {
            let mut bounds:Rect = path[0].bounds;
            for i in 1..path.len(){
                bounds = bounds.hull(path[i].bounds);
            }
            return Some((path_hash,bounds))
        }
        self.parse_and_cache_path(path_hash, path_str.as_str().as_bytes())
    }
    
    pub fn get_icon_slot(&mut self, args: CxIconArgs, path_hash: CxIconPathHash) -> CxIconSlot {
        let entry_hash = CxIconEntryHash(path_hash.0.id_append(args.hash()));
        
        if let Some(entry) = self.entries.get(&entry_hash) {
            return entry.slot
        }
        
        let (slot,pos) = self.alloc.alloc_icon_slot(args.size.x as f64, args.size.y as f64);
        self.entries.insert(
            entry_hash,
            CxIconEntry {
                path_hash,
                slot,
                pos,
                args
            }
        );
        self.alloc.todo.push(entry_hash);
        
        return slot
    }
    
}
impl CxIconAtlasAlloc {
    pub fn alloc_icon_slot(&mut self, w: f64, h: f64) -> (CxIconSlot,DVec2) {
        if w + self.xpos >= self.texture_size.x {
            self.xpos = 0.0;
            self.ypos += self.hmax + 1.0;
            self.hmax = 0.0;
        }
        if h + self.ypos >= self.texture_size.y {
            println!("ICON ATLAS FULL, TODO FIX THIS {} > {},", h + self.ypos, self.texture_size.y);
        }
        if h > self.hmax {
            self.hmax = h;
        }
        
        let px = self.xpos;
        let py = self.ypos;
        
        let tx1 = px / self.texture_size.x;
        let ty1 = py / self.texture_size.y;
        
        self.xpos += w + 1.0;
        
        (CxIconSlot {
            chan: 0.0,
            t1: dvec2(tx1, ty1).into(),
            t2: dvec2(tx1 + (w / self.texture_size.x), ty1 + (h / self.texture_size.y)).into()
        },dvec2(px, py).into())
    }
}

#[derive(Clone)]
pub struct CxIconAtlasRc(pub Rc<RefCell<CxIconAtlas >>);

impl CxIconAtlas {
    pub fn reset_icon_atlas(&mut self) {
        self.entries.clear();
        self.alloc.xpos = 0.;
        self.alloc.ypos = 0.;
        self.alloc.hmax = 0.;
        self.clear_buffer = true;
    }
    
    pub fn get_internal_atlas_texture(&self) -> &Texture {
        &self.texture
    }
}


impl DrawTrapezoidVector {
    // atlas drawing function used by CxAfterDraw
    fn draw_vector(&mut self, entry: &CxIconEntry, path: &CxIconPathCommands, many: &mut ManyInstances) {
        let trapezoids = {
            let mut trapezoids = Vec::new();
            //log_str(&format!("Serializing char {} {} {} {}", glyphtc.tx1 , cx.fonts_atlas.texture_size.x ,todo.subpixel_x_fract ,atlas_page.dpi_factor));
            let trapezoidate = self.trapezoidator.trapezoidate(
                path.map({
                    //log!("{:?} {:?}", entry.args, entry.pos);
                    move | cmd | {
                        let cmd = cmd.transform(
                            &AffineTransformation::identity()
                                .translate(Vector::new(entry.args.translate.x, entry.args.translate.y))
                                .uniform_scale(entry.args.scale)
                                .translate(Vector::new(entry.pos.x + entry.args.subpixel.x, entry.pos.y + entry.args.subpixel.y))
                        );
                        cmd
                    }
                }).linearize(entry.args.linearize)
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
            self.chan = 0.0 as f32;
            many.instances.extend_from_slice(self.draw_vars.as_slice());
        }
    }
}

#[derive(Clone)]
pub struct CxDrawIconAtlasRc(pub Rc<RefCell<CxDrawIconAtlas >>);

pub struct CxDrawIconAtlas {
    pub draw_trapezoid: DrawTrapezoidVector,
    pub atlas_pass: Pass,
    pub atlas_draw_list: DrawList2d,
    pub atlas_texture: Texture,
}

impl CxDrawIconAtlas {
    pub fn new(cx: &mut Cx) -> Self {
        
        let atlas_texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8{
            size: TextureSize::Auto
        });
        //cx.fonts_atlas.texture_id = Some(atlas_texture.texture_id());
        
        let draw_trapezoid = DrawTrapezoidVector::new_local(cx);
        // ok we need to initialize drawtrapezoidtext from a live pointer.
        Self {
            draw_trapezoid,
            atlas_pass: Pass::new(cx),
            atlas_draw_list: DrawList2d::new(cx),
            atlas_texture: atlas_texture
        }
    }
}

impl<'a> Cx2d<'a> {
    pub fn lazy_construct_icon_atlas(cx: &mut Cx) {
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxIconAtlasRc>() {
            
            let draw_atlas = CxDrawIconAtlas::new(cx);
            let texture = draw_atlas.atlas_texture.clone();
            cx.set_global(CxDrawIconAtlasRc(Rc::new(RefCell::new(draw_atlas))));
            
            let atlas = CxIconAtlas::new(texture);
            cx.set_global(CxIconAtlasRc(Rc::new(RefCell::new(atlas))));
        }
    }
    
    pub fn reset_icon_atlas(cx: &mut Cx) {
        if cx.has_global::<CxIconAtlasRc>() {
            let mut fonts_atlas = cx.get_global::<CxIconAtlasRc>().0.borrow_mut();
            fonts_atlas.reset_icon_atlas();
        }
    }
    
    pub fn draw_icon_atlas(&mut self) {
        let draw_atlas_rc = self.cx.get_global::<CxDrawIconAtlasRc>().clone();
        let mut draw_atlas = draw_atlas_rc.0.borrow_mut();
        let atlas_rc = self.icon_atlas_rc.clone();
        let mut atlas = atlas_rc.0.borrow_mut();
        let atlas = &mut*atlas;
        //let start = Cx::profile_time_ns();
        // we need to start a pass that just uses the texture
        if atlas.alloc.todo.len()>0 {
            self.begin_pass(&draw_atlas.atlas_pass, None);
            
            let texture_size = atlas.alloc.texture_size;
            draw_atlas.atlas_pass.set_size(self.cx, texture_size);
            
            let clear = if atlas.clear_buffer {
                atlas.clear_buffer = false;
                PassClearColor::ClearWith(Vec4::default())
            }
            else {
                PassClearColor::InitWith(Vec4::default())
            };
            
            draw_atlas.atlas_pass.clear_color_textures(self.cx);
            draw_atlas.atlas_pass.add_color_texture(self.cx, &draw_atlas.atlas_texture, clear);
            draw_atlas.atlas_draw_list.begin_always(self);
            
            let mut atlas_todo = Vec::new();
            std::mem::swap(&mut atlas.alloc.todo, &mut atlas_todo);
            
            if let Some(mut many) = self.begin_many_instances(&draw_atlas.draw_trapezoid.draw_vars) {
                for todo in atlas_todo {
                    let entry = atlas.entries.get(&todo).unwrap();
                    let path = atlas.paths.get(&entry.path_hash).unwrap();
                    for i in 0..path.len(){                        
                        draw_atlas.draw_trapezoid.draw_vector(entry, &path[i], &mut many);
                    }
                    
                }
                
                self.end_many_instances(many);
            }
            draw_atlas.atlas_draw_list.end(self);
            self.end_pass(&draw_atlas.atlas_pass);
        }
    }
    
    
}

fn parse_svg_path(path: &[u8]) -> Result<Vec<PathCommand>, String> {
    #[derive(Debug)]
    enum Cmd {
        Unknown,
        Move(bool),
        Hor(bool),
        Vert(bool),
        Line(bool),
        Arc(bool),
        Cubic(bool),
        Quadratic(bool),
        Close
    }
    impl Default for Cmd {fn default() -> Self {Self::Unknown}}
    
    #[derive(Default)]
    struct ParseState {
        cmd: Cmd,
        expect_nums: usize,
        chain: bool,
        nums: [f64; 7],
        num_count: usize,
        last_pt: Point,
        first_pt: Point,
        out: Vec<PathCommand>,
        num_state: Option<NumState>
    }
    
    #[derive(Debug)]
    struct NumState {
        num: f64,
        mul: f64,
        has_dot: bool,
    }
    
    impl NumState {
        fn new_pos(v: f64) -> Self {Self {num: v, mul: 1.0, has_dot: false}}
        fn new_min() -> Self {Self {num: 0.0, mul: -1.0, has_dot: false}}
        fn finalize(self) -> f64 {self.num * self.mul}
        fn add_digit(&mut self, digit: f64) {
            self.num *= 10.0;
            self.num += digit;
            if self.has_dot {
                self.mul *= 0.1;
            }
        }
    }
    
    impl ParseState {
        fn next_cmd(&mut self, cmd: Cmd) -> Result<(), String> {
            self.finalize_cmd() ?;
            self.chain = false;
            self.expect_nums = match cmd {
                Cmd::Unknown => panic!(),
                Cmd::Move(_) => 2,
                Cmd::Hor(_) => 1,
                Cmd::Vert(_) => 1,
                Cmd::Line(_) => 2,
                Cmd::Cubic(_) => 6,
                Cmd::Arc(_) => 7,
                Cmd::Quadratic(_) => 4,
                Cmd::Close => 0
            };
            self.cmd = cmd;
            Ok(())
        }
        
        fn add_min(&mut self) -> Result<(), String> {
            if self.num_state.is_some() {
                self.finalize_num();
            }
            if self.expect_nums == self.num_count {
                self.finalize_cmd() ?;
            }
            if self.expect_nums == 0 {
                return Err(format!("Unexpected minus"));
            }
            self.num_state = Some(NumState::new_min());
            Ok(())
        }
        
        fn add_digit(&mut self, digit: f64) -> Result<(), String> {
            if let Some(num_state) = &mut self.num_state {
                num_state.add_digit(digit);
            }
            else {
                if self.expect_nums == self.num_count {
                    self.finalize_cmd() ?;
                }
                if self.expect_nums == 0 {
                    return Err(format!("Unexpected digit"));
                }
                self.num_state = Some(NumState::new_pos(digit))
            }
            Ok(())
        }
        
        fn add_dot(&mut self) -> Result<(), String> {
            if let Some(num_state) = &mut self.num_state {
                if num_state.has_dot {
                    self.finalize_num();
                    self.add_digit(0.0) ?;
                    self.add_dot() ?;
                    return Ok(());
                }
                num_state.has_dot = true;
            }
            else {
                self.add_digit(0.0) ?;
                self.add_dot() ?;
            }
            Ok(())
        }
        
        fn finalize_num(&mut self) {
            if let Some(num_state) = self.num_state.take() {
                self.nums[self.num_count] = num_state.finalize();
                self.num_count += 1;
            }
        }
        
        fn whitespace(&mut self) -> Result<(), String> {
            self.finalize_num();
            if self.expect_nums == self.num_count {
                self.finalize_cmd() ?;
            }
            Ok(())
        }
        
        fn finalize_cmd(&mut self) -> Result<(), String> {
            self.finalize_num();
            if self.chain && self.num_count == 0 {
                return Ok(())
            }
            if self.expect_nums != self.num_count {
                return Err(format!("SVG Path command {:?} expected {} points, got {}", self.cmd, self.expect_nums, self.num_count));
            }
            match self.cmd {
                Cmd::Unknown => (),
                Cmd::Move(abs) => {
                    
                    if abs {
                        self.last_pt = Point {x: self.nums[0], y: self.nums[1]};
                    }
                    else {
                        self.last_pt += Vector {x: self.nums[0], y: self.nums[1]};
                    }
                    self.first_pt = self.last_pt;
                    self.out.push(PathCommand::MoveTo(self.last_pt));
                },
                Cmd::Hor(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.nums[0], y: self.last_pt.y};
                    }
                    else {
                        self.last_pt += Vector {x: self.nums[0], y: 0.0};
                    }
                    self.out.push(PathCommand::LineTo(self.last_pt));
                }
                Cmd::Vert(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.last_pt.x, y: self.nums[0]};
                    }
                    else {
                        self.last_pt += Vector {x: 0.0, y: self.nums[0]};
                    }
                    self.out.push(PathCommand::LineTo(self.last_pt));
                }
                Cmd::Line(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.nums[0], y: self.nums[1]};
                    }
                    else {
                        self.last_pt += Vector {x: self.nums[0], y: self.nums[1]};
                    }
                    self.out.push(PathCommand::LineTo(self.last_pt));
                },
                Cmd::Cubic(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.nums[4], y: self.nums[5]};
                        self.out.push(PathCommand::CubicTo(
                            Point {x: self.nums[0], y: self.nums[1]},
                            Point {x: self.nums[2], y: self.nums[3]},
                            self.last_pt,
                        ));
                    } else {
                        self.out.push(PathCommand::CubicTo(
                            self.last_pt + Vector {x: self.nums[0], y: self.nums[1]},
                            self.last_pt + Vector {x: self.nums[2], y: self.nums[3]},
                            self.last_pt + Vector {x: self.nums[4], y: self.nums[5]},
                        ));
                        self.last_pt += Vector {x: self.nums[4], y: self.nums[5]};
                    }
                },
                Cmd::Arc(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.nums[5], y: self.nums[6]};
                        self.out.push(PathCommand::ArcTo(
                            self.last_pt,
                            Point {x: self.nums[0], y: self.nums[1]},
                            self.nums[2],
                            self.nums[3] != 0.0,
                            self.nums[4] != 0.0,
                        ));
                    }
                    else {
                        self.out.push(PathCommand::ArcTo(
                            self.last_pt + Vector {x: self.nums[5], y: self.nums[6]},
                            Point {x: self.nums[0], y: self.nums[1]},
                            self.nums[2],
                            self.nums[3] != 0.0,
                            self.nums[4] != 0.0,
                        ));
                        self.last_pt += Vector {x: self.nums[5], y: self.nums[6]};
                    }
                },
                Cmd::Quadratic(abs) => {
                    if abs {
                        self.last_pt = Point {x: self.nums[2], y: self.nums[3]};
                        self.out.push(PathCommand::QuadraticTo(
                            Point {x: self.nums[0], y: self.nums[1]},
                            self.last_pt
                        ));
                    }
                    else {
                        self.out.push(PathCommand::QuadraticTo(
                            self.last_pt + Vector {x: self.nums[0], y: self.nums[1]},
                            self.last_pt + Vector {x: self.nums[2], y: self.nums[3]},
                        ));
                        self.last_pt += Vector {x: self.nums[2], y: self.nums[3]};
                    }
                }
                Cmd::Close => {
                    self.last_pt = self.first_pt;
                    self.out.push(PathCommand::Close);
                }
            }
            self.num_count = 0;
            self.chain = true;
            Ok(())
        }
    }
    
    let mut state = ParseState::default();
    
    for i in 0..path.len() {
        match path[i] {
            b'M' => state.next_cmd(Cmd::Move(true)) ?,
            b'm' => state.next_cmd(Cmd::Move(false)) ?,
            b'Q' => state.next_cmd(Cmd::Quadratic(true)) ?,
            b'q' => state.next_cmd(Cmd::Quadratic(false)) ?,
            b'C' => state.next_cmd(Cmd::Cubic(true)) ?,
            b'c' => state.next_cmd(Cmd::Cubic(false)) ?,
            b'H' => state.next_cmd(Cmd::Hor(true)) ?,
            b'h' => state.next_cmd(Cmd::Hor(false)) ?,
            b'V' => state.next_cmd(Cmd::Vert(true)) ?,
            b'v' => state.next_cmd(Cmd::Vert(false)) ?,
            b'L' => state.next_cmd(Cmd::Line(true)) ?,
            b'l' => state.next_cmd(Cmd::Line(false)) ?,
            b'A' => state.next_cmd(Cmd::Arc(true)) ?,
            b'a' => state.next_cmd(Cmd::Arc(false)) ?,
            b'Z' | b'z' => state.next_cmd(Cmd::Close) ?,
            b'-' => state.add_min() ?,
            b'0'..=b'9' => state.add_digit((path[i] - b'0') as f64) ?,
            b'.' => state.add_dot() ?,
            b',' | b' ' | b'\r' | b'\n' | b'\t' => state.whitespace() ?,
            x => {
                return Err(format!("Unexpected character {} - {}", x, x as char))
            }
        }
    }
    state.finalize_cmd() ?;
    
    Ok(state.out)
}
