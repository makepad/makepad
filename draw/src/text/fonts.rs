use {
    super::{
        font::{Font, FontId},
        font_faces::FontFaces,
        font_family::{FontFamily, FontFamilyId},
        geometry::{Point, Size},
        image::Subimage,
        image_atlas::ImageAtlas,
        pixels::{Bgra, R},
        shaper::Shaper,
    },
    makepad_platform::*,
    std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc},
};

const IBM_PLEX_SANS_TEXT: &[u8] = include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LXGWWenKaiRegular.ttf");
const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/resources/NotoColorEmoji.ttf");

#[derive(Debug)]
pub struct FontsWithTextures {
    fonts: Fonts,
    grayscale_texture: Texture,
    color_texture: Texture,
}

impl FontsWithTextures {
    pub fn new(cx: &mut Cx, atlas_size: Size<usize>, definitions: Definitions) -> Self {
        Self {
            fonts: Fonts::new(atlas_size, definitions),
            grayscale_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: atlas_size.width,
                    height: atlas_size.height,
                    data: Some(vec![]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            color_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: atlas_size.width,
                    height: atlas_size.height,
                    data: Some(vec![]),
                    updated: TextureUpdated::Empty,
                },
            ),
        }
    }

    pub fn grayscale_texture(&self) -> &Texture {
        &self.grayscale_texture
    }

    pub fn color_texture(&self) -> &Texture {
        &self.color_texture
    }

    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    pub fn fonts_mut(&mut self) -> &mut Fonts {
        &mut self.fonts
    }

    pub fn update_textures(&mut self, cx: &mut Cx) {
        self.update_grayscale_texture(cx);
        self.update_color_texture(cx);
    }

    fn update_grayscale_texture(&mut self, cx: &mut Cx) {
        let mut texture_data = self.grayscale_texture.take_vec_u8(cx);
        let texture_size = self.fonts.atlas_size();
        let dirty_rect = self
            .fonts
            .take_dirty_grayscale_atlas_image_with(|dirty_image| {
                let dirty_rect = dirty_image.bounds();
                for src_y in 0..dirty_rect.size.height {
                    for src_x in 0..dirty_rect.size.width {
                        let dst_x = dirty_rect.origin.x + src_x;
                        let dst_y = dirty_rect.origin.y + src_y;
                        let pixel = dirty_image[Point::new(src_x, src_y)];
                        texture_data[dst_y * texture_size.width + dst_x] = pixel.r;
                    }
                }
                dirty_rect
            });
        self.grayscale_texture.put_back_vec_u8(
            cx,
            texture_data,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
    }

    fn update_color_texture(&mut self, cx: &mut Cx) {
        fn bgra_to_u32(pixel: Bgra<u8>) -> u32 {
            let b = u32::from(pixel.b);
            let g = u32::from(pixel.g);
            let r = u32::from(pixel.r);
            let a = u32::from(pixel.a);
            (b << 24) | (g << 16) | (r << 8) | a
        }

        let mut texture_data = self.color_texture.take_vec_u32(cx);
        let texture_size = self.fonts.atlas_size();
        let dirty_rect = self.fonts.take_dirty_color_atlas_image_with(|dirty_image| {
            let dirty_rect = dirty_image.bounds();
            for src_y in 0..dirty_rect.size.height {
                for src_x in 0..dirty_rect.size.width {
                    let dst_x = dirty_rect.origin.x + src_x;
                    let dst_y = dirty_rect.origin.y + src_y;
                    let pixel = dirty_image[Point::new(src_x, src_y)];
                    texture_data[dst_y * texture_size.width + dst_x] = bgra_to_u32(pixel);
                }
            }
            dirty_rect
        });
        self.color_texture.put_back_vec_u32(
            cx,
            texture_data,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
    }
}

#[derive(Clone, Debug)]
pub struct Fonts {
    shaper: Rc<RefCell<Shaper>>,
    grayscale_atlas: Rc<RefCell<ImageAtlas<R<u8>>>>,
    color_atlas: Rc<RefCell<ImageAtlas<Bgra<u8>>>>,
    definitions: Definitions,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

impl Fonts {
    pub fn new(atlas_size: Size<usize>, definitions: Definitions) -> Self {
        Self {
            shaper: Rc::new(RefCell::new(Shaper::new())),
            grayscale_atlas: Rc::new(RefCell::new(ImageAtlas::new(atlas_size))),
            color_atlas: Rc::new(RefCell::new(ImageAtlas::new(atlas_size))),
            definitions,
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn atlas_size(&self) -> Size<usize> {
        self.grayscale_atlas.borrow().size()
    }

    pub fn take_dirty_grayscale_atlas_image_with<T>(
        &mut self,
        f: impl FnOnce(Subimage<'_, R<u8>>) -> T,
    ) -> T {
        f(self.grayscale_atlas.borrow_mut().take_dirty_image())
    }

    pub fn take_dirty_color_atlas_image_with<T>(
        &mut self,
        f: impl FnOnce(Subimage<'_, Bgra<u8>>) -> T,
    ) -> T {
        f(self.color_atlas.borrow_mut().take_dirty_image())
    }

    pub fn get_or_create_font_family(&mut self, font_family_id: &FontFamilyId) -> Rc<FontFamily> {
        if !self.font_family_cache.contains_key(font_family_id) {
            let definition = self
                .definitions
                .font_families
                .remove(font_family_id)
                .unwrap_or_else(|| panic!("font family {:?} is not defined", font_family_id));
            let font_family = Rc::new(FontFamily::new(
                font_family_id.clone(),
                self.shaper.clone(),
                definition
                    .font_ids
                    .into_iter()
                    .map(|font_id| self.get_or_create_font(&font_id))
                    .collect(),
            ));
            self.font_family_cache
                .insert(font_family_id.clone(), font_family);
        }
        self.font_family_cache.get(font_family_id).unwrap().clone()
    }

    pub fn get_or_create_font(&mut self, font_id: &FontId) -> Rc<Font> {
        if !self.font_cache.contains_key(font_id) {
            let definition = self
                .definitions
                .fonts
                .remove(font_id)
                .unwrap_or_else(|| panic!("font {:?} is not defined", font_id));
            let font = Rc::new(Font::new(
                font_id.clone(),
                self.grayscale_atlas.clone(),
                self.color_atlas.clone(),
                FontFaces::from_data_and_index(definition.data, definition.index).unwrap(),
            ));
            self.font_cache.insert(font_id.clone(), font);
        }
        self.font_cache.get(font_id).unwrap().clone()
    }
}

#[derive(Clone, Debug)]
pub struct Definitions {
    pub font_families: HashMap<FontFamilyId, FontFamilyDefinition>,
    pub fonts: HashMap<FontId, FontDefinition>,
}

impl Default for Definitions {
    fn default() -> Self {
        Self {
            font_families: [(
                FontFamilyId::Sans,
                FontFamilyDefinition {
                    font_ids: [
                        "IBM Plex Sans Text".into(),
                        "LXG WWen Kai Regular".into(),
                        "Noto Color Emoji".into(),
                    ]
                    .into(),
                },
            )]
            .into_iter()
            .collect(),
            fonts: [
                (
                    "IBM Plex Sans Text".into(),
                    FontDefinition {
                        data: Cow::Borrowed(IBM_PLEX_SANS_TEXT).into(),
                        index: 0,
                    },
                ),
                (
                    "LXG WWen Kai Regular".into(),
                    FontDefinition {
                        data: Cow::Borrowed(LXG_WEN_KAI_REGULAR).into(),
                        index: 0,
                    },
                ),
                (
                    "Noto Color Emoji".into(),
                    FontDefinition {
                        data: Cow::Borrowed(NOTO_COLOR_EMOJI).into(),
                        index: 0,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: Rc<Cow<'static, [u8]>>,
    pub index: u32,
}

mod tests {
    use {
        super::*,
        std::{fs::File, io::BufWriter},
    };

    #[test]
    fn test() {
        let atlas_size = Size::new(512, 512);
        let mut fonts = Fonts::new(atlas_size, Definitions::default());
        let font_family = fonts.get_or_create_font_family(&FontFamilyId::Sans);
        let glyphs = font_family.shape("HalloRik!繁😊😔");
        for glyph in glyphs {
            glyph.font.allocate_glyph(glyph.id, 64.0);
        }

        let file = File::create("/Users/ejpbruel/Desktop/grayscale.png").unwrap();
        let writer = BufWriter::new(file);
        let atlas = fonts.grayscale_atlas.borrow();
        let mut encoder =
            png::Encoder::new(writer, atlas_size.width as u32, atlas_size.height as u32);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = atlas.image().pixels();
        let data =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len()) };
        writer.write_image_data(&data).unwrap();

        let file = File::create("/Users/ejpbruel/Desktop/color.png").unwrap();
        let writer = BufWriter::new(file);
        let atlas = fonts.color_atlas.borrow();
        let mut encoder =
            png::Encoder::new(writer, atlas_size.width as u32, atlas_size.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = atlas.image().pixels();
        let data =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
        writer.write_image_data(&data).unwrap();
    }
}
