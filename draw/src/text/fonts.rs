use {
    super::{
        font_atlas::{ColorAtlas, GrayscaleAtlas},
        font_loader::FontDefinitions,
        geom::Point,
        image::Rgba,
        layout,
        layout::{LaidoutText, LayoutParams, Layouter},
        sdfer::Sdfer,
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
    pub fn new(cx: &mut Cx, definitions: FontDefinitions, settings: layout::Settings) -> Self {
        let layouter = Layouter::new(definitions, settings);
        let grayscale_atlas_size = layouter.grayscale_atlas().borrow().size();
        let color_atlas_size = layouter.color_atlas().borrow().size();
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

    pub fn sdfer(&self) -> &Rc<RefCell<Sdfer>> {
        self.layouter.sdfer()
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<GrayscaleAtlas>> {
        self.layouter.grayscale_atlas()
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<ColorAtlas>> {
        self.layouter.color_atlas()
    }

    pub fn grayscale_texture(&self) -> &Texture {
        &self.grayscale_texture
    }

    pub fn color_texture(&self) -> &Texture {
        &self.color_texture
    }

    pub fn get_or_layout(&mut self, params: LayoutParams) -> Rc<LaidoutText> {
        self.layouter.get_or_layout(params)
    }

    pub fn update_textures(&mut self, cx: &mut Cx) {
        self.update_grayscale_texture(cx);
        self.update_color_texture(cx);
    }

    fn update_grayscale_texture(&mut self, cx: &mut Cx) {
        let mut texture_data = self.grayscale_texture.take_vec_u8(cx);
        let mut atlas = self.layouter.grayscale_atlas().borrow_mut();
        let atlas_size = atlas.size();
        let dirty_image = atlas.take_dirty_image();
        let dirty_rect = dirty_image.bounds();
        for src_y in 0..dirty_rect.size.height {
            for src_x in 0..dirty_rect.size.width {
                let dst_x = dirty_rect.origin.x + src_x;
                let dst_y = dirty_rect.origin.y + src_y;
                let pixel = dirty_image[Point::new(src_x, src_y)];
                texture_data[dst_y * atlas_size.width + dst_x] = pixel.r;
            }
        }
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
        fn rgba_to_u32(pixel: Rgba) -> u32 {
            let r = u32::from(pixel.r);
            let g = u32::from(pixel.g);
            let b = u32::from(pixel.b);
            let a = u32::from(pixel.a);
            (a << 24) | (r << 16) | (g << 8) | b
        }

        let mut texture_data = self.color_texture.take_vec_u32(cx);
        let mut atlas = self.layouter.color_atlas().borrow_mut();
        let atlas_size = atlas.size();
        let dirty_image = atlas.take_dirty_image();
        let dirty_rect = dirty_image.bounds();
        for src_y in 0..dirty_rect.size.height {
            for src_x in 0..dirty_rect.size.width {
                let dst_x = dirty_rect.origin.x + src_x;
                let dst_y = dirty_rect.origin.y + src_y;
                let pixel = dirty_image[Point::new(src_x, src_y)];
                texture_data[dst_y * atlas_size.width + dst_x] = rgba_to_u32(pixel);
            }
        }
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
