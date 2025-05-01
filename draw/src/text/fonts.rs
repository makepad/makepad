use {
    super::{
        font::FontId,
        font_family::FontFamilyId,
        layouter::{self, LaidoutText, LayoutParams, Layouter},
        loader::{FontDefinition, FontFamilyDefinition},
        rasterizer::Rasterizer,
    },
    makepad_platform::*,
    std::{cell::RefCell, mem, rc::Rc},
};

#[derive(Debug)]
pub struct Fonts {
    layouter: Layouter,
    needs_prepare_atlases: bool,
    grayscale_texture: Texture,
    color_texture: Texture,
}

impl Fonts {
    pub fn new(cx: &mut Cx, settings: layouter::Settings) -> Self {
        let layouter = Layouter::new(settings);
        let rasterizer = layouter.rasterizer().borrow();
        let grayscale_atlas_size = rasterizer.grayscale_atlas().size();
        let color_atlas_size = rasterizer.color_atlas().size();
        drop(rasterizer);
        Self {
            layouter,
            needs_prepare_atlases: false,
            grayscale_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: grayscale_atlas_size.width,
                    height: grayscale_atlas_size.height,
                    data: None,
                    unpack_row_length: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            color_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: color_atlas_size.width,
                    height: color_atlas_size.height,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            ),
        }
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        self.layouter.rasterizer()
    }

    pub fn grayscale_texture(&self) -> &Texture {
        &self.grayscale_texture
    }

    pub fn color_texture(&self) -> &Texture {
        &self.color_texture
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.layouter.is_font_family_known(id)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        self.layouter.is_font_known(id)
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        self.layouter.define_font_family(id, definition);
    }

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        self.layouter.define_font(id, definition);
    }

    pub fn get_or_layout(&mut self, params: impl LayoutParams) -> Rc<LaidoutText> {
        self.layouter.get_or_layout(params)
    }

    pub fn prepare_textures(&mut self, cx: &mut Cx) -> bool {
        assert!(!self.needs_prepare_atlases);
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        if rasterizer.grayscale_atlas_mut().reset_if_needed() {
            return false;
        }
        if rasterizer.color_atlas_mut().reset_if_needed() {
            return false;
        }
        drop(rasterizer);
        self.prepare_grayscale_texture(cx);
        self.prepare_color_texture(cx);
        self.needs_prepare_atlases = true;
        true
    }

    fn prepare_grayscale_texture(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let dirty_rect = rasterizer.grayscale_atlas().dirty_rect();
        let pixels: Vec<u8> = unsafe {
            mem::transmute(rasterizer.grayscale_atlas_mut().replace_pixels(Vec::new()))
        };
        self.grayscale_texture.put_back_vec_u8(
            cx,
            pixels,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
    }

    fn prepare_color_texture(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let dirty_rect = rasterizer.color_atlas().dirty_rect();
        let pixels: Vec<u32> = unsafe {
            mem::transmute(rasterizer.color_atlas_mut().replace_pixels(Vec::new()))
        };
        self.color_texture.put_back_vec_u32(
            cx,
            pixels,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        )
    }

    pub fn prepare_atlases_if_needed(&mut self, cx: &mut Cx) {
        if !self.needs_prepare_atlases {
            return;
        }
        self.prepare_grayscale_atlas(cx);
        self.prepare_color_atlas(cx);
        self.needs_prepare_atlases = false;
    }

    fn prepare_grayscale_atlas(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let pixels = self.grayscale_texture.take_vec_u8(cx);
        unsafe {
            rasterizer.grayscale_atlas_mut().replace_pixels(mem::transmute(pixels))
        };
    }

    fn prepare_color_atlas(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let pixels = self.color_texture.take_vec_u32(cx);
        unsafe {
            rasterizer.color_atlas_mut().replace_pixels(mem::transmute(pixels))
        };
    }
}
