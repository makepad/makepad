use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*
};

live_design!{
    ImageBase = {{Image}} {}
}

#[derive(Live, WidgetRegister)]
pub struct Image {
    #[walk] walk: Walk,
    #[live] draw_bg: DrawQuad,
    #[live] min_width: i64,
    #[live] min_height: i64,
    #[live(1.0)] width_scale: f64,
    #[live] fit: ImageFit,
    #[live] source: LiveDependency,
    #[rust] texture: Option<Texture>,
}

impl ImageCacheImpl for Image {
    fn get_texture(&self) -> &Option<Texture> {
        &self.texture
    }
    
    fn set_texture(&mut self, texture: Option<Texture>) {
        self.texture = texture;
    }
}

impl LiveHook for Image{
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        self.lazy_create_image_cache(cx);
        let source = self.source.clone();
        if source.as_str().len()>0 {
            self.load_image_dep_by_path(cx, source.as_str())
        }
    }
}

impl Widget for Image {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx)
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {
        self.walk
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut WidgetScope, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
}

impl Image {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> WidgetDraw {
        // alright we get a walk. depending on our aspect ratio
        // we change either nothing, or width or height
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        let (width, height) = if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
            let (width,height) = image_texture.get_format(cx).vec_width_height().unwrap_or((self.min_width as usize, self.min_height as usize));
            (width as f64 * self.width_scale, height as f64)
        }
        else {
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };
        
        let aspect = width / height;
        match self.fit {
            ImageFit::Stretch => {},
            ImageFit::Horizontal => {
                walk.height = Size::Fixed(rect.size.x / aspect);
            },
            ImageFit::Vertical => {
                walk.width = Size::Fixed(rect.size.y * aspect);
            },
            ImageFit::Smallest => {
                let walk_height = rect.size.x / aspect;
                if walk_height > rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
            ImageFit::Biggest => {
                let walk_height = rect.size.x / aspect;
                if walk_height < rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
        }
        
        // lets start a turtle and center horizontally
        
        self.draw_bg.draw_walk(cx, walk);
        
        WidgetDraw::done()
    }
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct ImageRef(WidgetRef);

impl ImageRef {
    pub fn load_image_dep_by_path(&self, cx: &mut Cx, image_path: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_image_dep_by_path(cx, image_path)
        }
    }
    
    pub fn load_jpg_from_data(&self, cx: &mut Cx, data: &[u8]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_jpg_from_data(cx, data)
        }
    }
    
    pub fn load_png_from_data(&self, cx: &mut Cx, data: &[u8]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_png_from_data(cx, data)
        }
    }
    
    pub fn set_texture(&self, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.texture = texture
        }
    }
    
    pub fn set_uniform(&self, cx: &Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }    
}

#[derive(Clone, Default, WidgetSet)]
pub struct ImageSet(WidgetSet);

impl ImageSet {
}
