pub use {
    std::{
        borrow::Borrow,
        collections::VecDeque,
        hash::{Hash, Hasher},
        rc::Rc,
        cell::RefCell,
        io::prelude::*,
        fs::File,
        collections::HashMap,
    },
    crate::{
        makepad_platform::*,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        draw_list_2d::{ManyInstances, DrawList2d, RedrawingApi},
        geometry::GeometryQuad2D,
        shader::draw_trapezoid::DrawTrapezoidVector,
        makepad_vector::font::Glyph,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::ExtendFromInternalIterator,
        makepad_vector::path::PathIterator,
    },
    rustybuzz::{Direction, GlyphInfo, UnicodeBuffer},
};

pub struct CxFontsAtlas {
    pub fonts: Vec<Option<CxFont >>,
    pub path_to_font_id: HashMap<String, usize>,
    pub texture_id: TextureId,
    pub clear_buffer: bool,
    pub alloc: CxFontsAtlasAlloc
}

#[derive(Default)]
pub struct CxFontsAtlasAlloc {
    pub texture_size: DVec2,
    pub xpos: f64,
    pub ypos: f64,
    pub hmax: f64,
    pub todo: Vec<CxFontsAtlasTodo>,
}

impl CxFontsAtlas {
    pub fn new(texture_id: TextureId) -> Self {
        Self {
            fonts: Vec::new(),
            path_to_font_id: HashMap::new(),
            texture_id,
            clear_buffer: false,
            alloc: CxFontsAtlasAlloc {
                texture_size: DVec2 {x: 4096.0, y: 4096.0},
                xpos: 0.0,
                ypos: 0.0,
                hmax: 0.0,
                todo: Vec::new(),
            }
        }
    }
}
impl CxFontsAtlasAlloc {
    pub fn alloc_atlas_glyph(&mut self, w: f64, h: f64) -> CxFontAtlasGlyph {
        if w + self.xpos >= self.texture_size.x {
            self.xpos = 0.0;
            self.ypos += self.hmax + 1.0;
            self.hmax = 0.0;
        }
        if h + self.ypos >= self.texture_size.y {
            println!("FONT ATLAS FULL, TODO FIX THIS {} > {},", h + self.ypos, self.texture_size.y);
        }
        if h > self.hmax {
            self.hmax = h;
        }
        
        let tx1 = self.xpos / self.texture_size.x;
        let ty1 = self.ypos / self.texture_size.y;
        
        self.xpos += w + 1.0;
        
        CxFontAtlasGlyph {
            t1: dvec2(tx1, ty1).into(),
            t2: dvec2( tx1 + (w / self.texture_size.x), ty1 + (h / self.texture_size.y)).into()
        }
    }
}

#[derive(Clone, Live)]
pub struct Font {
    #[rust] pub font_id: Option<usize>,
    #[live] pub path: LiveDependency
}

#[derive(Clone)]
pub struct CxFontsAtlasRc(pub Rc<RefCell<CxFontsAtlas >>);

impl LiveHook for Font {
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        Cx2d::lazy_construct_font_atlas(cx);
        let atlas = cx.get_global::<CxFontsAtlasRc>().clone();
        self.font_id = Some(atlas.0.borrow_mut().get_font_by_path(cx, self.path.as_str()));
    }
}

impl CxFontsAtlas {
    pub fn get_font_by_path(&mut self, cx: &Cx, path: &str) -> usize {
        if path.len() == 0{
            return 0
        }
        if let Some(item) = self.path_to_font_id.get(path) {
            return *item;
        }
        let font_id = self.fonts.len();
        self.fonts.push(None);
        self.path_to_font_id.insert(path.to_string(), font_id);
        
        match cx.get_dependency(path) {
            // FIXME(eddyb) this clones the `data` `Vec<u8>`, in order to own it
            // inside a `owned_font_face::OwnedFace`.
            Ok(data) => match CxFont::load_from_ttf_bytes(data) {
                Err(_) => {
                    error!("Error loading font {} ", path);
                }
                Ok(cxfont) => {
                    self.fonts[font_id] = Some(cxfont);
                }
            }
            Err(err) => {
                error!("get_font_by_path - {} {}", path, err)
            }
        }
        font_id
    }
    
    pub fn reset_fonts_atlas(&mut self) {
        for cxfont in &mut self.fonts {
            if let Some(cxfont) = cxfont {
                cxfont.atlas_pages.clear();
            }
        }
        self.alloc.xpos = 0.;
        self.alloc.ypos = 0.;
        self.alloc.hmax = 0.;
        self.clear_buffer = true;
    }
    
    pub fn get_internal_font_atlas_texture_id(&self) -> TextureId {
        self.texture_id
    }
}

impl DrawTrapezoidVector {
    
    // atlas drawing function used by CxAfterDraw
    fn draw_todo(&mut self, fonts_atlas: &mut CxFontsAtlas, todo: CxFontsAtlasTodo, many: &mut ManyInstances) {
        //let fonts_atlas = cx.fonts_atlas_rc.0.borrow_mut();
        let mut size = 1.0;
        for i in 0..3 {
            if i == 1 {
                size = 0.75;
            }
            if i == 2 {
                size = 0.6;
            }
            let trapezoids = {
                let cxfont = fonts_atlas.fonts[todo.font_id].as_mut().unwrap();
                let units_per_em = cxfont.ttf_font.units_per_em;
                let atlas_page = &cxfont.atlas_pages[todo.atlas_page_id];
                let glyph = cxfont.owned_font_face.with_ref(|face| cxfont.ttf_font.get_glyph_by_id(face, todo.glyph_id).unwrap());
                
                let is_one_of_tab_lf_cr = ['\t', '\n', '\r'].iter().any(|&c| {
                    Some(todo.glyph_id) == cxfont.owned_font_face.with_ref(|face| face.glyph_index(c).map(|id| id.0 as usize))
                });
                if is_one_of_tab_lf_cr {
                    return
                }
                
                let glyphtc = atlas_page.atlas_glyphs[todo.glyph_id][todo.subpixel_id].unwrap();
                let tx = glyphtc.t1.x as f64 * fonts_atlas.alloc.texture_size.x + todo.subpixel_x_fract * atlas_page.dpi_factor;
                let ty = 1.0 + glyphtc.t1.y as f64 * fonts_atlas.alloc.texture_size.y - todo.subpixel_y_fract * atlas_page.dpi_factor;
                
                let font_scale_logical = atlas_page.font_size * 96.0 / (72.0 * units_per_em);
                let font_scale_pixels = font_scale_logical * atlas_page.dpi_factor;
                let mut trapezoids = Vec::new();
                //log_str(&format!("Serializing char {} {} {} {}", glyphtc.tx1 , cx.fonts_atlas.texture_size.x ,todo.subpixel_x_fract ,atlas_page.dpi_factor));
                let trapezoidate = self.trapezoidator.trapezoidate(
                    glyph
                        .outline
                        .iter()
                        .map({
                        move | command | {
                            let cmd = command.transform(
                                &AffineTransformation::identity()
                                    .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
                                    .uniform_scale(font_scale_pixels * size)
                                    .translate(Vector::new(tx, ty))
                        );
                        
                            cmd
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
            }
        }
    }
}

#[derive(Clone)]
pub struct CxDrawFontsAtlasRc(pub Rc<RefCell<CxDrawFontsAtlas >>);

pub struct CxDrawFontsAtlas {
    pub draw_trapezoid: DrawTrapezoidVector,
    pub atlas_pass: Pass,
    pub atlas_draw_list: DrawList2d,
    pub atlas_texture: Texture,
    pub counter: usize
}

impl CxDrawFontsAtlas {
    pub fn new(cx: &mut Cx) -> Self {
        
        let atlas_texture = Texture::new(cx);
        
        //cx.fonts_atlas.texture_id = Some(atlas_texture.texture_id());
        
        let draw_trapezoid = DrawTrapezoidVector::new_local(cx);
        // ok we need to initialize drawtrapezoidtext from a live pointer.
        Self {
            counter: 0,
            draw_trapezoid,
            atlas_pass: Pass::new(cx),
            atlas_draw_list: DrawList2d::new(cx),
            atlas_texture: atlas_texture
        }
    }
}

impl<'a> Cx2d<'a> {
    pub fn lazy_construct_font_atlas(cx: &mut Cx){
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxFontsAtlasRc>() {
            
            let draw_fonts_atlas = CxDrawFontsAtlas::new(cx);
            let texture_id = draw_fonts_atlas.atlas_texture.texture_id();
            cx.set_global(CxDrawFontsAtlasRc(Rc::new(RefCell::new(draw_fonts_atlas))));
            
            let fonts_atlas = CxFontsAtlas::new(texture_id);
            cx.set_global(CxFontsAtlasRc(Rc::new(RefCell::new(fonts_atlas))));
        }
    }
    
    pub fn reset_fonts_atlas(cx:&mut Cx){
        if cx.has_global::<CxFontsAtlasRc>() {
            let mut fonts_atlas = cx.get_global::<CxFontsAtlasRc>().0.borrow_mut();
            fonts_atlas.reset_fonts_atlas();
        }
    }
        
    pub fn draw_font_atlas(&mut self) {
        let draw_fonts_atlas_rc = self.cx.get_global::<CxDrawFontsAtlasRc>().clone();
        let mut draw_fonts_atlas = draw_fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas_rc = self.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;
        //let start = Cx::profile_time_ns();
        // we need to start a pass that just uses the texture
        if fonts_atlas.alloc.todo.len()>0 {
            self.begin_pass(&draw_fonts_atlas.atlas_pass, None);

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
            draw_fonts_atlas.atlas_draw_list.begin_always(self);

            let mut atlas_todo = Vec::new();
            std::mem::swap(&mut fonts_atlas.alloc.todo, &mut atlas_todo);
            
            if let Some(mut many) = self.begin_many_instances(&draw_fonts_atlas.draw_trapezoid.draw_vars) {

                for todo in atlas_todo {
                    draw_fonts_atlas.draw_trapezoid.draw_todo(fonts_atlas, todo, &mut many);
                }
                
                self.end_many_instances(many);
            }
            
            draw_fonts_atlas.counter += 1;
            draw_fonts_atlas.atlas_draw_list.end(self);
            self.end_pass(&draw_fonts_atlas.atlas_pass);
        }
        //println!("TOTALT TIME {}", Cx::profile_time_ns() - start);
    }
}

pub struct CxFont {
    pub ttf_font: makepad_vector::font::TTFFont,
    pub owned_font_face: crate::owned_font_face::OwnedFace,
    pub atlas_pages: Vec<CxFontAtlasPage>,
    pub shape_cache: ShapeCache,
}

pub struct ShapeCache {
    pub keys: VecDeque<(Direction, Rc<str>)>,
    pub glyph_ids: HashMap<(Direction, Rc<str>), Vec<usize>>,
}

impl ShapeCache {
    // The maximum number of keys that can be stored in the cache.
    const MAX_SIZE: usize = 4096;

    pub fn new() -> Self {
        Self {
            keys: VecDeque::new(),
            glyph_ids: HashMap::new(),
        }
    }

    // If there is an entry for the given key in the cache, returns the corresponding list of
    // glyph indices for that key. Otherwise, uses the given UnicodeBuffer and OwnedFace to
    // compute the list of glyph indices for the key, inserts that in the cache and then returns
    // the corresponding list.
    //
    // This method takes a UnicodeBuffer by value, and then returns the same buffer by value. This
    // is necessary because rustybuzz::shape consumes the UnicodeBuffer and then returns a
    // GlyphBuffer that reuses the same storage. Once we are done with the GlyphBuffer, we consume
    // it and then return yet another UnicodeBuffer that reuses the same storage. This allows us to
    // avoid unnecessary heap allocations.
    //
    // Note that owned_font_face should be the same as the CxFont to which this cache belongs,
    // otherwise you will not get correct results.
    pub fn get_or_compute_glyph_ids(
        &mut self, 
        key: (Direction, &str),
        mut rustybuzz_buffer: UnicodeBuffer,
        owned_font_face: &crate::owned_font_face::OwnedFace
    ) -> (&[usize], UnicodeBuffer) {
        if !self.glyph_ids.contains_key(&key as &dyn ShapeCacheKey) {
            if self.keys.len() == Self::MAX_SIZE {
                for run in self.keys.drain(..Self::MAX_SIZE / 2) {
                    self.glyph_ids.remove(&run);
                }
            }

            let (direction, string) = key;
            rustybuzz_buffer.set_direction(direction);
            rustybuzz_buffer.push_str(string);
            let glyph_buffer = owned_font_face.with_ref( | face | rustybuzz::shape(face, &[], rustybuzz_buffer));
            let glyph_ids: Vec<_> = glyph_buffer.glyph_infos().iter().map( | glyph | glyph.glyph_id as usize).collect();
            rustybuzz_buffer = glyph_buffer.clear();

            let owned_string: Rc<str> = string.into();
            self.keys.push_back((direction, owned_string.clone()));
            self.glyph_ids.insert((direction, owned_string), glyph_ids);
        }
        (&self.glyph_ids[&key as &dyn ShapeCacheKey], rustybuzz_buffer)
    }
}

// When doing inserts on the shape cache, we want to use (Direction, Rc<str>) as our key type. When
// doing lookups on the shape cache, we want to use (Direction, &str) as our key type.
// Unfortunately, Rust does not allow this, since (Direction, Rc<str>) can only be borrowed as
// &(Direction, Rc<str>). So we'd have to create a temporary key, and then borrow from that.
//
// This is unacceptable, because creating a temporary key requires us to do a heap allocation every
// time we want to do a lookup on the shape cache, which is on a very hot path. Instead, we resort
// to a bit of trickery, inspired by the following post on Stackoverflow:
// https://stackoverflow.com/questions/45786717/how-to-implement-hashmap-with-two-keys/46044391#46044391
//
// The idea is that we cannot borrow (Direction, Rc<str>) as a (Direction, &str). But what we *can* do is
// define a trait ShapeCacheKey to represent our key, with methods to access both the direction and the
// string, implement that for both (Direction, Rc<str>) and (Direction, &str), and then borrow
// (Direction, Rc<str>) as &dyn ShapeCacheKey (that is, a reference to a trait object). We can turn a
// (Direction, &str) into a &dyn ShapeCacheKey without creating a temporary key or doing any heap
// allocations, so this allows us to do what we want.
pub trait ShapeCacheKey {
    fn direction(&self) -> Direction;
    fn string(&self) -> &str;
}

impl<'a> Borrow<dyn ShapeCacheKey + 'a> for (Direction, Rc<str>) {
    fn borrow(&self) -> &(dyn ShapeCacheKey + 'a) {
        self
    }
}

impl Eq for dyn ShapeCacheKey + '_ {}

impl Hash for dyn ShapeCacheKey + '_ {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.direction().hash(hasher);
        self.string().hash(hasher);
    }
}

impl PartialEq for dyn ShapeCacheKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        if self.direction() != other.direction() {
            return false;
        }
        if self.string() != other.string() {
            return false;
        }
        true
    }
}

impl ShapeCacheKey for (Direction, &str) {
    fn direction(&self) -> Direction {
        self.0
    }

    fn string(&self) -> &str {
        self.1
    }
}

impl ShapeCacheKey for (Direction, Rc<str>) {
    fn direction(&self) -> Direction {
        self.0
    }

    fn string(&self) -> &str {
        &self.1
    }
}

pub const ATLAS_SUBPIXEL_SLOTS: usize = 64;

#[derive(Clone)]
pub struct CxFontAtlasPage {
    pub dpi_factor: f64,
    pub font_size: f64,
    pub atlas_glyphs: Vec<[Option<CxFontAtlasGlyph>; ATLAS_SUBPIXEL_SLOTS]>
}

#[derive(Clone, Copy)]
pub struct CxFontAtlasGlyph {
    pub t1: Vec2,
    pub t2: Vec2,
}

#[derive(Default, Debug)]
pub struct CxFontsAtlasTodo {
    pub subpixel_x_fract: f64,
    pub subpixel_y_fract: f64,
    pub font_id: usize,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
    pub subpixel_id: usize
}

impl CxFont {
    pub fn load_from_ttf_bytes(bytes: Rc<Vec<u8>>) -> Result<Self, crate::owned_font_face::FaceParsingError> {
        let owned_font_face = crate::owned_font_face::OwnedFace::parse(bytes, 0)?;
        let ttf_font = owned_font_face.with_ref(|face| makepad_vector::ttf_parser::from_ttf_parser_face(face));
        Ok(Self {
            ttf_font,
            owned_font_face,
            atlas_pages: Vec::new(),
            shape_cache: ShapeCache::new(),
        })
    }
    
    pub fn get_atlas_page_id(&mut self, dpi_factor: f64, font_size: f64) -> usize {
        for (index, sg) in self.atlas_pages.iter().enumerate() {
            if sg.dpi_factor == dpi_factor
                && sg.font_size == font_size {
                return index
            }
        }
        self.atlas_pages.push(CxFontAtlasPage {
            dpi_factor: dpi_factor,
            font_size: font_size,
            atlas_glyphs: {
                let mut v = Vec::new();
                v.resize(self.owned_font_face.with_ref(|face| face.number_of_glyphs() as usize), [None; ATLAS_SUBPIXEL_SLOTS]);
                v
            }
        });
        self.atlas_pages.len() - 1
    }

    pub fn get_glyph(&mut self, c:char)->Option<&Glyph>{
        if c < '\u{10000}' {
            Some(self.get_glyph_by_id(self.owned_font_face.with_ref(|face| face.glyph_index(c))?.0 as usize).unwrap())
        } else {
            None
        }
    }

    pub fn get_glyph_by_id(&mut self, id: usize) -> makepad_vector::ttf_parser::Result<&Glyph> {
        self.owned_font_face.with_ref(|face| self.ttf_font.get_glyph_by_id(face, id))
    }
}
