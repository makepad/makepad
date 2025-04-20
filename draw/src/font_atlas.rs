pub use {
    std::{
        borrow::{Borrow, Cow},
        collections::VecDeque,
        env,
        hash::{Hash, Hasher},
        rc::Rc,
        cell::RefCell,
        io,
        io::prelude::*,
        fs::{File, OpenOptions},
        collections::HashMap,
        mem,
        path::Path,
    },
    crate::{
        glyph_rasterizer::{RasterizedGlyph, GlyphRasterizer},
        font_loader::FontLoader,
        makepad_platform::*,
        cx_draw::CxDraw,
        turtle::{Walk, Layout},
        draw_list_2d::{ManyInstances, DrawList2d, RedrawingApi},
        geometry::GeometryQuad2D,
        glyph_rasterizer::{Command, Params},
        makepad_vector::font::Glyph,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::ExtendFromInternalIterator,
        makepad_vector::path::PathIterator,
        text_shaper::TextShaper,
    },
    fxhash::FxHashMap,
    makepad_rustybuzz::{Direction, GlyphBuffer},
    makepad_vector::ttf_parser::GlyphId,
    unicode_segmentation::UnicodeSegmentation
};

pub(crate) const ATLAS_WIDTH: usize = 4096;
pub(crate) const ATLAS_HEIGHT: usize = 4096;

#[derive(Debug)]
pub struct CxFontAtlas {
    pub texture_sdf: Texture,
    pub texture_svg: Texture,
    pub texture_size: DVec2,
    pub full: bool,
    pub xpos: usize,
    pub ypos: usize,
    pub hmax: usize,
    pub commands: Vec<Command>,
    pub sdf: Option<CxFontsAtlasSdfConfig>,
}

#[derive(Debug)]
pub struct CxFontsAtlasSdfConfig {
    pub params: sdfer::esdt::Params,
}

impl CxFontAtlas {
    pub fn new(texture_sdf: Texture, texture_svg: Texture) -> Self {
        Self {
            texture_sdf,
            texture_svg,
            full: false,
            texture_size: DVec2 {
                x: ATLAS_WIDTH as f64,
                y: ATLAS_HEIGHT as f64
            },
            xpos: 0,
            ypos: 0,
            hmax: 0,
            commands: Vec::new(),
            // Set this to `None` to use CPU-rasterized glyphs instead of SDF.
            sdf: Some(CxFontsAtlasSdfConfig {
                params: sdfer::esdt::Params {
                    pad: 4,
                    radius: 8.0,
                    cutoff: 0.25,
                    ..Default::default()
                },
            })
        }
    }
}

impl CxFontAtlas {
    pub fn alloc_atlas_glyph(&mut self, w: f64, h: f64, command: Command) -> CxFontAtlasGlyph {
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

        self.commands.push(command);

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

#[derive(Debug, Clone, Live, LiveRegister)]
pub struct Font {
    #[rust] pub font_id: Option<usize>,
    #[live] pub path: LiveDependency
}

#[derive(Clone)]
pub struct CxFontsAtlasRc(pub Rc<RefCell<CxFontAtlas>>);

impl LiveHook for Font {
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        CxDraw::lazy_construct_font_loader(cx);
        let loader = cx.get_global::<Rc<RefCell<FontLoader>>>().clone();
        self.font_id = Some(loader.borrow_mut().get_or_load(cx, self.path.as_str()));
    }
}

impl CxFontAtlas {
    pub fn reset_fonts_atlas(&mut self, font_loader: &mut FontLoader) {
        for (_, _, cxfont) in font_loader {
            if let Some(cxfont) = cxfont {
                cxfont.atlas_pages.clear();
            }
        }
        self.commands.clear();
        self.full = false;
        self.xpos = 0;
        self.ypos = 0;
        self.hmax = 0;
    }

    pub fn get_internal_font_atlas_texture_id(&self) -> Texture {
        self.texture_sdf.clone()
    }
}

impl<'a> CxDraw<'a> {
    pub fn lazy_construct_font_loader(cx: &mut Cx) {
        if !cx.has_global::<Rc<RefCell<FontLoader>>>() {
            cx.set_global(Rc::new(RefCell::new(FontLoader::new())));
        }
    }

    pub fn lazy_construct_text_shaper(cx: &mut Cx) {
        if !cx.has_global::<Rc<RefCell<TextShaper>>>() {
            cx.set_global(Rc::new(RefCell::new(TextShaper::new())));
        }
    }

    pub fn lazy_construct_glyph_rasterizer(cx: &mut Cx) {
        if !cx.has_global::<Rc<RefCell<GlyphRasterizer>>>() {
            let cache_dir = cx.os_type().get_cache_dir();
            let cache_dir = cache_dir.as_ref().map(|string| Path::new(string));
            cx.set_global(Rc::new(RefCell::new(GlyphRasterizer::new(cache_dir))));
        }
    }

    pub fn lazy_construct_font_atlas(cx: &mut Cx){
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxFontsAtlasRc>() {

            let texture_sdf = Texture::new_with_format(cx, TextureFormat::VecRu8 {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                data: Some(vec![]),
                unpack_row_length: None,
                updated: TextureUpdated::Empty,
            });

            let texture_svg = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
                data: Some(vec![]),
                updated: TextureUpdated::Full,
            });

            let fonts_atlas = CxFontAtlas::new(texture_sdf, texture_svg);
            cx.set_global(CxFontsAtlasRc(Rc::new(RefCell::new(fonts_atlas))));
        }
    }

    pub fn reset_fonts_atlas(cx:&mut Cx,) {
        if cx.has_global::<CxFontsAtlasRc>() {
            let font_loader = cx.get_global::<Rc<RefCell<FontLoader>>>().clone();
            let mut fonts_atlas = cx.get_global::<CxFontsAtlasRc>().0.borrow_mut();
            fonts_atlas.reset_fonts_atlas(&mut *font_loader.borrow_mut());
        }
    }

    pub fn draw_font_atlas(&mut self) {
        let font_loader_rc = self.font_loader.clone();
        let mut font_loader_ref = font_loader_rc.borrow_mut();
        let font_loader = &mut *font_loader_ref;

        let fonts_atlas_rc = self.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;

        if fonts_atlas.full {
            fonts_atlas.reset_fonts_atlas(font_loader);
        }

        for todo in mem::take(&mut fonts_atlas.commands) {
            self.swrast_atlas_todo(font_loader, fonts_atlas, todo);
        }
    }

    fn swrast_atlas_todo(
        &mut self,
        font_loader: &mut FontLoader,
        fonts_atlas: &mut CxFontAtlas,
        command @ Command {
            params: Params {
                font_id,
                atlas_page_id,
                glyph_id,
            },
            ..
        }: Command
    ) {
        let cxfont = font_loader[font_id].as_mut().unwrap();
        let _atlas_page = &cxfont.atlas_pages[atlas_page_id];
        let _glyph = cxfont.owned_font_face.with_ref(|face| cxfont.ttf_font.get_glyph_by_id(face, glyph_id).unwrap());

        self.swrast_atlas_todo_sdf(font_loader, fonts_atlas, command);
    }

    fn swrast_atlas_todo_sdf(
        &mut self,
        font_loader: &mut FontLoader,
        font_atlas: &mut CxFontAtlas,
        command @ Command {
            params: Params {
                font_id,
                atlas_page_id,
                glyph_id,
            },
            ..
        }: Command
    ) {
        let font = font_loader[font_id].as_mut().unwrap();
        let _atlas_page = &font.atlas_pages[atlas_page_id];

        if ['\t', '\n', '\r'].iter().any(|&c| {
            Some(glyph_id) == font.owned_font_face.with_ref(|face| face.glyph_index(c).map(|id| id.0 as usize))
        }) {
            return;
        }

        let glyph_rasterizer_rc = self.glyph_rasterizer.clone();
        let mut glyph_rasterizer_ref = glyph_rasterizer_rc.borrow_mut();
        let glyph_rasterizer = &mut *glyph_rasterizer_ref;
        
        let RasterizedGlyph {
            size,
            bytes,
        } = glyph_rasterizer.get_or_rasterize_glyph(
            font_loader,
            font_atlas,
            command,
        );

        let font = font_loader[font_id].as_mut().unwrap();
        let atlas_page = &font.atlas_pages[atlas_page_id];
        let atlas_glyph = atlas_page.atlas_glyphs.get(&glyph_id).unwrap();

        let mut atlas_data = font_atlas.texture_sdf.take_vec_u8(self.cx);
        let (atlas_w, atlas_h) = font_atlas.texture_sdf.get_format(self.cx).vec_width_height().unwrap();
        if atlas_data.is_empty() {
            atlas_data = vec![0; atlas_w * atlas_h];
        } else {
            assert_eq!(atlas_data.len(), atlas_w * atlas_h);
        }

        let sdf_pad = font_atlas.sdf.as_ref().map_or(0, |sdf| sdf.params.pad);
        let atlas_x0 = (atlas_glyph.t1.x as f64 * font_atlas.texture_size.x) as usize - sdf_pad;
        let atlas_y0 = (atlas_glyph.t1.y as f64 * font_atlas.texture_size.y) as usize - sdf_pad;

        let mut index = 0;
        for y in 0..size.height {
            let dst = &mut atlas_data[(atlas_h - atlas_y0 - 1 - y) * atlas_w..][..atlas_w][atlas_x0..][..size.width];
            for dst in dst {
                *dst = bytes[index];
                index += 1;
            }
        }
        crate::log!("PUTTING BACK U8 {:?}",PointUsize::new(atlas_x0, atlas_h - atlas_y0 - size.height));
        font_atlas.texture_sdf.put_back_vec_u8(self.cx, atlas_data, Some(RectUsize::new(
            PointUsize::new(atlas_x0, atlas_h - atlas_y0 - size.height),
            size,
        )));
    }
}

#[derive(Debug)]
pub struct CxFont {
    pub ttf_font: makepad_vector::font::TTFFont,
    pub owned_font_face: crate::owned_font_face::OwnedFace,
    pub glyph_ids: Box<[Option<GlyphId>]>,
    pub atlas_pages: Vec<CxFontAtlasPage>,
}

#[derive(Clone, Debug)]
pub struct CxFontAtlasPage {
    pub font_size_in_device_pixels: f64,
    pub atlas_glyphs: HashMap<usize, CxFontAtlasGlyph>
}

#[derive(Clone, Copy, Debug)]
pub struct CxFontAtlasGlyph {
    pub t1: Vec2,
    pub t2: Vec2,
}

impl CxFont {
    pub fn load_from_ttf_bytes(bytes: Rc<Vec<u8>>) -> Result<Self, crate::owned_font_face::FaceParsingError> {
        let owned_font_face = crate::owned_font_face::OwnedFace::parse(bytes, 0)?;
        let ttf_font = owned_font_face.with_ref(|face| makepad_vector::ttf_parser::from_ttf_parser_face(face));
        Ok(Self {
            ttf_font,
            owned_font_face,
            glyph_ids: vec![None; 0x10FFFF].into_boxed_slice(),
            atlas_pages: Vec::new(),
        })
    }

    pub fn get_atlas_page_id(&mut self, font_size_in_device_pixels: f64) -> usize {
        for (index, sg) in self.atlas_pages.iter().enumerate() {
            if sg.font_size_in_device_pixels == font_size_in_device_pixels {
                return index
            }
        }
        self.atlas_pages.push(CxFontAtlasPage {
            font_size_in_device_pixels,
            atlas_glyphs: HashMap::new(),
        });
        self.atlas_pages.len() - 1
    }

    pub fn glyph_id(&mut self, c: char) -> GlyphId {
        if let Some(id) = self.glyph_ids[c as usize] {
            id
        } else {
            let id = self.owned_font_face.with_ref(|face| {
                face.glyph_index(c).unwrap_or(GlyphId(0))
            });
            self.glyph_ids[c as usize] = Some(id);
            id
        }
    }

    pub fn get_glyph(&mut self, c:char)->Option<&Glyph>{
        if c < '\u{10000}' {
            let id = self.glyph_id(c);
            Some(self.get_glyph_by_id(id.0 as usize).unwrap())
        } else {
            None
        }
    }

    pub fn get_glyph_by_id(&mut self, id: usize) -> makepad_vector::ttf_parser::Result<&Glyph> {
        self.owned_font_face.with_ref(|face| self.ttf_font.get_glyph_by_id(face, id))
    }

    pub fn get_advance_width_for_char(&mut self, c: char) -> Option<f64> {
        let id = self.glyph_id(c);
        self.get_advance_width_for_glyph(id)
    }

    pub fn get_advance_width_for_glyph(&mut self, id: GlyphId) -> Option<f64> {
        self.owned_font_face.with_ref(|face| face.glyph_hor_advance(id).map(|advance_width| advance_width as f64))
    }
}