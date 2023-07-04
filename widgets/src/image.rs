use {
    crate::{
        makepad_derive_widget::*,
        makepad_image_formats::jpeg,
        makepad_image_formats::png,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    Image = {{Image}} {
        walk:{
            width:Fit
            height:Fit
        }
        draw_bg: {
            texture image: texture2d
            instance scale: vec2(1.0, 1.0)
            fn get_color(self) -> vec4 {
                return sample2d(self.image, self.pos * self.scale).xyzw;
            }
            
            fn pixel(self) -> vec4 {
                return Pal::premul(self.get_color())
            }
            
            shape: Solid,
            fill: Image
        }
    }
}

#[derive(Live)]
pub struct Image {
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[live] draw_bg: DrawColor,
    
    #[live] source: LiveDependency,
    #[live] texture: Option<Texture>,
    #[live] scale: f64,
} 

impl LiveHook for Image {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Image)
    }
    
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        // lets load the image resource
        let image_path = self.source.as_str();
        if image_path.len()>0 {
            let mut image_buffer = None;
            match cx.get_dependency(image_path) {
                Ok(data) => {
                    if image_path.ends_with(".jpg") {
                        match jpeg::decode(data) {
                            Ok(image) => {
                                if self.scale != 0.0 {
                                    self.walk = Walk::fixed_size(DVec2 {x: image.width as f64 * self.scale, y: image.height as f64 * self.scale});
                                }
                                image_buffer = Some(image);
                            }
                            Err(err) => {
                                cx.apply_image_decoding_failed(live_error_origin!(), index, nodes, image_path, &err);
                            }
                        }
                    }
                    else if image_path.ends_with(".png") {
                        match png::decode(data) {
                            Ok(image) => {
                                if self.scale != 0.0 {
                                    self.walk = Walk::fixed_size(DVec2 {x: image.width as f64 * self.scale, y: image.height as f64 * self.scale});
                                }
                                image_buffer = Some(image);
                            }
                            Err(err) => {
                                cx.apply_image_decoding_failed(live_error_origin!(), index, nodes, image_path, &err);
                            }
                        }
                    }
                    else {
                        cx.apply_image_type_not_supported(live_error_origin!(), index, nodes, image_path);
                    }
                }
                Err(err) => {
                    cx.apply_resource_not_loaded(live_error_origin!(), index, nodes, image_path, &err);
                }
            }
            if let Some(mut image_buffer) = image_buffer.take() {
                if self.texture.is_none() {
                    self.texture = Some(Texture::new(cx));
                }
                if let Some(texture) = &mut self.texture {
                    texture.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(image_buffer.width),
                        height: Some(image_buffer.height),
                    });
                    texture.swap_image_u32(cx, &mut image_buffer.data);
                }
            }
        }
    }
}

impl Widget for Image {
    fn redraw(&mut self, cx:&mut Cx) {
        self.draw_bg.redraw(cx)
    }
    
    fn get_walk(&self)->Walk {
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk:Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
}

impl Image {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk:Walk) -> WidgetDraw {        
        if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
        }
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_bg.end(cx);

        WidgetDraw::done()
    }
}