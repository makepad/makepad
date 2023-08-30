use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    Image = {{Image}} {
        
            width: 100
            height: 100
        
        draw_bg: {
            texture image: texture2d
            instance opacity: 1.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
            
            fn get_color_scale_pan(self, scale:vec2, pan:vec2) -> vec4 {
                return sample2d(self.image, self.pos*scale+pan).xyzw;
            }
            
            fn get_color(self) -> vec4 {
                return self.get_color_scale_pan(self.image_scale, self.image_pan)
            }
            
            fn pixel(self) -> vec4 {
                let color = self.get_color();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity))
            }
        }
    }
}

#[derive(Live)]
pub struct Image {
    #[walk] walk: Walk,
    #[live] draw_bg: DrawQuad,
    #[live] min_width: i64,
    #[live] min_height: i64,
    
    #[live] fit: ImageFit,
    #[live] source: LiveDependency,
    #[live] texture: Option<Texture>,
}

impl ImageCacheImpl for Image {
    fn get_texture(&self) -> &Option<Texture> {
        &self.texture
    }
    
    fn set_texture(&mut self, texture: Option<Texture>) {
        self.texture = texture;
    }
}

impl LiveHook for Image {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Image)
    }
    
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
    
    fn walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
}

impl Image {

    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> WidgetDraw {
        // alright we get a walk. depending on our aspect ratio 
        // we change either nothing, or width or height 
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        let (width,height) = if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
            let desc = image_texture.get_desc(cx);
            (desc.width.unwrap_or(self.min_width as usize) as f64 / dpi,
            desc.height.unwrap_or(self.min_height as usize) as f64 / dpi)
        }
        else{
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };
        let aspect = width/height;
        match self.fit{
            ImageFit::Stretch=>{},
            ImageFit::Horizontal=>{
                walk.height = Size::Fixed(rect.size.x/aspect);
            },
            ImageFit::Vertical=>{
                walk.width = Size::Fixed(rect.size.y*aspect);
            },
            ImageFit::Smallest=>{
                let walk_height = rect.size.x/aspect;
                if walk_height > rect.size.y{
                    walk.width = Size::Fixed(rect.size.y*aspect);
                }
                else{
                    walk.height = Size::Fixed(walk_height);
                }
            }
            ImageFit::Biggest=>{
                let walk_height = rect.size.x/aspect;
                if walk_height < rect.size.y{
                    walk.width = Size::Fixed(rect.size.y*aspect);
                }
                else{
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
    pub fn load_image_dep_by_path(&self,cx: &mut Cx,image_path: &str) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.load_image_dep_by_path(cx, image_path)
        }
    }
    
    pub fn load_jpg_from_data(&self, cx:&mut Cx, data:&[u8]){
        if let Some(mut inner) = self.borrow_mut(){
            inner.load_jpg_from_data(cx, data)
        }
    }
    
    pub fn load_png_from_data(&self, cx:&mut Cx, data:&[u8]){
        if let Some(mut inner) = self.borrow_mut(){
            inner.load_png_from_data(cx, data)
        }
    }
    
    pub fn set_texture(&self, texture:Option<Texture>){
        if let Some(mut inner) = self.borrow_mut(){
            inner.texture = texture
        }
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct ImageSet(WidgetSet);

impl ImageSet {
}
