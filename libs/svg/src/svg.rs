use {
    makepad_widgets::*,
    resvg::tiny_skia::{Pixmap, Transform},
    resvg::usvg::{Options, Tree, fontdb},
};

live_design!{
    link widgets;
    
    use link::shaders::*;
    
    DrawSvg = {{DrawSvg}} {
        texture tex: texture2d

        fn pixel(self) -> vec4 {
            return sample2d(self.tex, self.pos);
        }
    }
    
    pub Svg = {{Svg}} {
        draw_svg: {
            texture tex: texture2d
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Svg {
    #[live]
    draw_svg: DrawSvg,
    
    #[redraw]
    #[rust]
    area: Area,
    
    #[walk]
    walk: Walk,
    
    #[live]
    text: String,
    
    #[rust]
    old_text: String,
    
    #[rust]
    texture: Option<Texture>,
}

impl Widget for Svg {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.render_svg(cx); 
        if let Some(texture) = &self.texture {
             let (width, height) = texture.get_format(cx).vec_width_height().unwrap_or((0, 0));
             walk.width = Size::Fixed(width as f64);
             walk.height = Size::Fixed(height as f64);
             self.draw_svg.draw_vars.set_texture(0, texture);
             self.draw_svg.draw_walk(cx, walk);
        }
        DrawStep::done()
    }
}

impl Svg {
    fn render_svg(&mut self, cx: &mut Cx) {
        if self.text == self.old_text {
            return;
        }
        self.old_text = self.text.clone();

        let mut opt = Options::default();
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        opt.fontdb = std::sync::Arc::new(db);

        match Tree::from_str(&self.text, &opt) {
            Ok(tree) => {
                let size = tree.size().to_int_size();
                let width = 2 * size.width();
                let height = 2 * size.height();
                let mut pixmap = Pixmap::new(width, height).unwrap();
                resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
                let rgba_data = pixmap.data();

                let mut bgra_data = Vec::with_capacity((width * height) as usize);
                for chunk in rgba_data.chunks(4) {
                    let r = chunk[0] as u32;
                    let g = chunk[1] as u32;
                    let b = chunk[2] as u32;
                    let a = chunk[3] as u32;
                     
                    let pixel = (a << 24) | (r << 16) | (g << 8) | b;
                    bgra_data.push(pixel);
                }
                 
                let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                    data: Some(bgra_data),
                    width: width as usize,
                    height: height as usize,
                    updated: TextureUpdated::Full,
                });
                 
                self.texture = Some(texture);
                 
                self.walk.width = Size::Fixed(width as f64 / 2.0); 
                self.walk.height = Size::Fixed(height as f64 / 2.0);
            }
            Err(e) => {
                log!("SVG error: {:?}", e);
            }
        }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawSvg {
    #[deref]
    draw_super: DrawQuad,
}
