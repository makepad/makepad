use sdfer::NDCursor as _;

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
        makepad_vector::font::Glyph,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::ExtendFromInternalIterator,
        makepad_vector::path::PathIterator,
    },
    rustybuzz::{Direction, GlyphInfo, UnicodeBuffer},
};

pub(crate) const ATLAS_WIDTH: usize = 4096;
pub(crate) const ATLAS_HEIGHT: usize = 4096;

pub struct CxFontsAtlas {
    pub fonts: Vec<Option<CxFont >>,
    pub path_to_font_id: HashMap<String, usize>,
    pub texture: Texture,
    pub clear_buffer: bool,
    pub alloc: CxFontsAtlasAlloc
}

#[derive(Default)]
pub struct CxFontsAtlasAlloc {
    pub texture_size: DVec2,
    pub full: bool,
    pub xpos: usize,
    pub ypos: usize,
    pub hmax: usize,
    pub todo: Vec<CxFontsAtlasTodo>,
    pub sdf: Option<CxFontsAtlasSdfConfig>,
}

pub struct CxFontsAtlasSdfConfig {
    pub params: sdfer::esdt::Params,
}

impl CxFontsAtlas {
    pub fn new(texture: Texture) -> Self {
        Self {
            fonts: Vec::new(),
            path_to_font_id: HashMap::new(),
            texture,
            clear_buffer: false,
            alloc: CxFontsAtlasAlloc {
                full: false,
                texture_size: DVec2 {
                    x: ATLAS_WIDTH as f64,
                    y: ATLAS_HEIGHT as f64
                },
                xpos: 0,
                ypos: 0,
                hmax: 0,
                todo: Vec::new(),
                // Set this to `None` to use CPU-rasterized glyphs instead of SDF.
                sdf: Some(CxFontsAtlasSdfConfig {
                    params: sdfer::esdt::Params {
                        pad: 4,
                        radius: 8.0,
                        cutoff: 0.25,
                        ..Default::default()
                    },
                })
            },
        }
    }
}
impl CxFontsAtlasAlloc {
    pub fn alloc_atlas_glyph(&mut self, w: f64, h: f64, todo: CxFontsAtlasTodo) -> CxFontAtlasGlyph {
        // In SDF mode, leave enough room around each glyph (i.e. padding).
        let pad = self.sdf.as_ref().map_or(0, |sdf| sdf.params.pad);

        // Preserve the aspect ratio, while still scaling up at least one side
        // to a power of 2, and that side has to the larger side, due to the
        // potential for extreme aspect ratios massively increasing the size.
        let max = w.max(h);
        // NOTE(eddyb) the `* 1.5` ensures that sizes which are already close to
        // a power of 2, still get scaled up by at least 50%.
        // FIXME(eddyb) the choice of pow2 here should probably be used as the
        // atlas page (so that similar enough font sizes reuse the same pow2),
        // but that's currently complicated by the `w`/`h` computation inside
        // `DrawText::draw_inner` (and duplicated by `swrast_atlas_todo` below),
        // which adds its own padding, relative to the font size (and DPI).
        let scale = ((max * 1.5).ceil() as usize).next_power_of_two().max(64) as f64 / max;

        let (w, h) = (
            (w * scale).ceil() as usize + pad * 2,
            (h * scale).ceil() as usize + pad * 2,
        );

        if w + self.xpos >= self.texture_size.x as usize {
            self.xpos = 0;
            self.ypos += self.hmax;
            self.hmax = 0;
        }
        if h + self.ypos >= self.texture_size.y as usize {
            // ok so the fontatlas is full..
            self.full = true;
            println!("FONT ATLAS FULL, TODO FIX THIS {} > {},", h + self.ypos, self.texture_size.y);
        }
        if h > self.hmax {
            self.hmax = h;
        }

        let x_range = self.xpos..(self.xpos + w);
        let y_range = self.ypos..(self.ypos + h);

        self.xpos += w;

        self.todo.push(todo);

        CxFontAtlasGlyph {
            t1: (dvec2(
                (x_range.start + pad) as f64,
                (y_range.start + pad) as f64,
            ) / self.texture_size).into(),

            // NOTE(eddyb) `- 1` is because the texture coordinate rectangle
            // formed by `t1` and `t2` is *inclusive*, while the integer ranges
            // (i.e. `x_range` and `y_range`) are (inherently) *exclusive*.
            t2: (dvec2(
                (x_range.end - pad - 1) as f64,
                (y_range.end - pad - 1) as f64,
            ) / self.texture_size).into(),
        }
    }
}

#[derive(Clone, Live, LiveRegister)]
pub struct Font {
    #[rust] pub font_id: Option<usize>,
    #[live] pub path: LiveDependency
}

#[derive(Clone)]
pub struct CxFontsAtlasRc(pub Rc<RefCell<CxFontsAtlas >>);

impl LiveHook for Font {
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
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
        self.alloc.todo.clear();
        self.alloc.full = false;
        self.alloc.xpos = 0;
        self.alloc.ypos = 0;
        self.alloc.hmax = 0;
        self.clear_buffer = true;
    }
    
    pub fn get_internal_font_atlas_texture_id(&self) -> Texture {
        self.texture.clone()
    }
}

impl<'a> Cx2d<'a> {
    pub fn lazy_construct_font_atlas(cx: &mut Cx){
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxFontsAtlasRc>() {
            
            let texture = Texture::new_with_format(cx, TextureFormat::VecRu8 {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                data: vec![],
                unpack_row_length: None
            });
            
            let fonts_atlas = CxFontsAtlas::new(texture);
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
        let fonts_atlas_rc = self.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;

        if fonts_atlas.alloc.full {
            fonts_atlas.reset_fonts_atlas();
        }

        // Will be automatically filled after the first use.
        let mut reuse_sdfer_bufs = None;

        for todo in std::mem::take(&mut fonts_atlas.alloc.todo) {
            self.swrast_atlas_todo(fonts_atlas, todo, &mut reuse_sdfer_bufs);
        }
    }

    fn swrast_atlas_todo(
        &mut self,
        fonts_atlas: &mut CxFontsAtlas,
        todo: CxFontsAtlasTodo,
        reuse_sdfer_bufs: &mut Option<sdfer::esdt::ReusableBuffers>,
    ) {
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

        let glyphtc = atlas_page.atlas_glyphs.get(&todo.glyph_id).unwrap();

        let font_scale_logical = atlas_page.font_size * 96.0 / (72.0 * units_per_em);
        let font_scale_pixels = font_scale_logical * atlas_page.dpi_factor;

        // HACK(eddyb) ideally these values computed by `DrawText::draw_inner`
        // would be kept in each `CxFontsAtlasTodo`, to avoid recomputation here.
        let render_pad_dpx = 2.0;
        let render_wh = dvec2(
            ((glyph.bounds.p_max.x - glyph.bounds.p_min.x) * font_scale_pixels).ceil() + render_pad_dpx * 2.0,
            ((glyph.bounds.p_max.y - glyph.bounds.p_min.y) * font_scale_pixels).ceil() + render_pad_dpx * 2.0,
        );

        // NOTE(eddyb) `+ 1.0` is because the texture coordinate rectangle
        // formed by `t1` and `t2` is *inclusive*, see also the comment in
        // `alloc_atlas_glyph` (about its `- 1` counterpart to this `+ 1.0`).
        let atlas_alloc_wh = dvec2(
            (glyphtc.t2.x - glyphtc.t1.x) as f64 * fonts_atlas.alloc.texture_size.x + 1.0,
            (glyphtc.t2.y - glyphtc.t1.y) as f64 * fonts_atlas.alloc.texture_size.y + 1.0,
        );

        // HACK(eddyb) because `render_wh` can be larger than the `glyph.bounds`
        // scaled by `font_scale_pixels`, and `alloc_atlas_glyph` performs some
        // non-trivial scaling on `render_wh` to get better SDF quality, this
        // division is required to properly map the glyph outline into atlas
        // space, *without* encroaching into the extra space `render_wh` added.
        let atlas_scaling = atlas_alloc_wh / render_wh;

        let transform = AffineTransformation::identity()
            .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
            .uniform_scale(font_scale_pixels)
            .translate(Vector::new(render_pad_dpx, render_pad_dpx))
            .scale(Vector::new(atlas_scaling.x, atlas_scaling.y));
        let commands = glyph
            .outline
            .iter()
            .map(move |command| command.transform(&transform));

        // FIXME(eddyb) try reusing this buffer.
        let mut glyph_rast = sdfer::Image2d::<_, Vec<_>>::new(
            atlas_alloc_wh.x.ceil() as usize,
            atlas_alloc_wh.y.ceil() as usize,
        );

        let mut cur = ab_glyph_rasterizer::point(0.0, 0.0);
        let to_ab = |p: makepad_vector::geometry::Point| ab_glyph_rasterizer::point(p.x as f32, p.y as f32);
        commands
        .fold(ab_glyph_rasterizer::Rasterizer::new(
            glyph_rast.width(),
            glyph_rast.height()
        ), |mut rasterizer, cmd| match cmd {
            makepad_vector::path::PathCommand::MoveTo(p) => {
                cur = to_ab(p);
                rasterizer
            }
            makepad_vector::path::PathCommand::LineTo(p1) => {
                let (p0, p1) = (cur, to_ab(p1));
                rasterizer.draw_line(p0, p1);
                cur = p1;
                rasterizer
            }
            makepad_vector::path::PathCommand::ArcTo(..) => {
                unreachable!("font glyphs should not use arcs");
            }
            makepad_vector::path::PathCommand::QuadraticTo(p1, p2) => {
                let (p0, p1, p2) = (cur, to_ab(p1), to_ab(p2));
                rasterizer.draw_quad(p0, p1, p2);
                cur = p2;
                rasterizer
            }
            makepad_vector::path::PathCommand::CubicTo(p1, p2, p3) => {
                let (p0, p1, p2, p3) = (cur, to_ab(p1), to_ab(p2), to_ab(p3));
                rasterizer.draw_cubic(p0, p1, p2, p3);
                cur = p3;
                rasterizer
            }
            makepad_vector::path::PathCommand::Close => rasterizer
        })
        .for_each_pixel_2d(|x, y, a| {
            glyph_rast[(x as usize, y as usize)] = sdfer::Unorm8::encode(a);
        });

        let mut glyph_out = if let Some(sdf_config) = &fonts_atlas.alloc.sdf {
            let (glyph_sdf, new_reuse_bufs) = sdfer::esdt::glyph_to_sdf(
                &mut glyph_rast,
                sdf_config.params,
                reuse_sdfer_bufs.take(),
            );
            *reuse_sdfer_bufs = Some(new_reuse_bufs);
            glyph_sdf
        } else {
            glyph_rast
        };

        let mut atlas_data = vec![];
        fonts_atlas.texture.swap_vec_u8(self.cx, &mut atlas_data);
        let (atlas_w, atlas_h) = fonts_atlas.texture.get_format(self.cx).vec_width_height().unwrap();
        if atlas_data.is_empty() {
            atlas_data = vec![0; atlas_w*atlas_h];
        } else {
            assert_eq!(atlas_data.len(), atlas_w*atlas_h);
        }

        let sdf_pad = fonts_atlas.alloc.sdf.as_ref().map_or(0, |sdf| sdf.params.pad);
        let atlas_x0 = (glyphtc.t1.x as f64 * fonts_atlas.alloc.texture_size.x) as usize - sdf_pad;
        let atlas_y0 = (glyphtc.t1.y as f64 * fonts_atlas.alloc.texture_size.y) as usize - sdf_pad;

        for y in 0..glyph_out.height() {
            let dst = &mut atlas_data[(atlas_h - atlas_y0 - 1 - y) * atlas_w..][..atlas_w][atlas_x0..][..glyph_out.width()];
            let mut src = glyph_out.cursor_at(0, y);
            for dst in dst {
                *dst = src.get_mut().to_bits();
                src.advance((1, 0));
            }
        }
        fonts_atlas.texture.swap_vec_u8(self.cx, &mut atlas_data);
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

#[derive(Clone)]
pub struct CxFontAtlasPage {
    pub dpi_factor: f64,
    pub font_size: f64,
    pub atlas_glyphs: HashMap<usize, CxFontAtlasGlyph>
}

#[derive(Clone, Copy)]
pub struct CxFontAtlasGlyph {
    pub t1: Vec2,
    pub t2: Vec2,
}

#[derive(Default, Debug)]
pub struct CxFontsAtlasTodo {
    pub font_id: usize,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
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
            atlas_glyphs: HashMap::new(),
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
