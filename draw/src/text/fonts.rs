use {
    super::{
        font::FontId,
        font_family::FontFamilyId,
        geom::Point,
        image::Rgba,
        layouter::{self, LaidoutText, LayoutParams, Layouter},
        loader::{FontDefinition, FontFamilyDefinition},
        rasterizer::Rasterizer,
    },
    makepad_platform::*,
    std::{cell::RefCell, rc::Rc},
};

#[derive(Debug)]
pub struct Fonts {
    layouter: Layouter,
    grayscale_texture: Texture,
    color_texture: Texture,
}

impl Fonts {
    pub fn new(cx: &mut Cx, settings: layouter::Settings) -> Self {
        let layouter = Layouter::new(settings);
        let rasterizer = layouter.rasterizer().borrow();
        let grayscale_atlas_size = rasterizer.grayscale_atlas_size();
        let color_atlas_size = rasterizer.color_atlas_size();
        drop(rasterizer);
        Self {
            layouter,
            grayscale_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: grayscale_atlas_size.width,
                    height: grayscale_atlas_size.height,
                    data: Some(vec![
                        0;
                        grayscale_atlas_size.width * grayscale_atlas_size.height
                    ]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            color_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: color_atlas_size.width,
                    height: color_atlas_size.height,
                    data: Some(vec![
                        0;
                        grayscale_atlas_size.width
                            * grayscale_atlas_size.height
                            * 4
                    ]),
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

    pub fn update_textures(&mut self, cx: &mut Cx) -> bool {
        if !self.update_grayscale_texture(cx) {
            return false;
        }
        if !self.update_color_texture(cx) {
            return false;
        }
        true
    }

    fn update_grayscale_texture(&mut self, cx: &mut Cx) -> bool {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        if rasterizer.reset_grayscale_atlas_if_needed() {
            return false;
        }
        let mut data = self.grayscale_texture.take_vec_u8(cx);
        let size = rasterizer.grayscale_atlas_size();
        let dirty_image = rasterizer.take_grayscale_atlas_dirty_image();
        let dirty_rect = dirty_image.bounds();
        for src_y in 0..dirty_rect.size.height {
            for src_x in 0..dirty_rect.size.width {
                let dst_x = dirty_rect.origin.x + src_x;
                let dst_y = dirty_rect.origin.y + src_y;
                let pixel = dirty_image[Point::new(src_x, src_y)];
                data[dst_y * size.width + dst_x] = pixel.r;
            }
        }

        self.grayscale_texture.put_back_vec_u8(
            cx,
            data,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
        true
    }

    fn update_color_texture(&mut self, cx: &mut Cx) -> bool {
        fn rgba_to_u32(pixel: Rgba) -> u32 {
            let r = u32::from(pixel.r);
            let g = u32::from(pixel.g);
            let b = u32::from(pixel.b);
            let a = u32::from(pixel.a);
            (a << 24) | (r << 16) | (g << 8) | b
        }

        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        if rasterizer.reset_color_atlas_if_needed() {
            return false;
        }
        let mut data = self.color_texture.take_vec_u32(cx);
        let size = rasterizer.color_atlas_size();
        let dirty_image = rasterizer.take_color_atlas_dirty_image();
        let dirty_rect = dirty_image.bounds();
        for src_y in 0..dirty_rect.size.height {
            for src_x in 0..dirty_rect.size.width {
                let dst_x = dirty_rect.origin.x + src_x;
                let dst_y = dirty_rect.origin.y + src_y;
                let pixel = dirty_image[Point::new(src_x, src_y)];
                data[dst_y * size.width + dst_x] = rgba_to_u32(pixel);
            }
        }
        self.color_texture.put_back_vec_u32(
            cx,
            data,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
        true
    }
}
