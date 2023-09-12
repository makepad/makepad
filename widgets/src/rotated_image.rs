use crate::{image_cache::*, makepad_draw::*, widget::*};

live_design! {
    RotatedImageBase = {{RotatedImage}} {}
}

#[derive(Live)]
pub struct RotatedImage {
    #[live]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    draw_bg: DrawColor,

    #[live]
    source: LiveDependency,
    #[live]
    texture: Option<Texture>,
    #[live]
    scale: f64,
}

impl ImageCacheImpl for RotatedImage {
    fn get_texture(&self) -> &Option<Texture> {
        &self.texture
    }

    fn set_texture(&mut self, texture: Option<Texture>) {
        self.texture = texture;
    }
}

impl LiveHook for RotatedImage {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, RotatedImage)
    }

    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        self.lazy_create_image_cache(cx);
        let source = self.source.clone();
        if source.as_str().len()>0{
            self.load_image_dep_by_path(cx, source.as_str())
        }
    }
}

impl Widget for RotatedImage {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx)
    }

    fn walk(&mut self, _cx:&mut Cx) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
}

impl RotatedImage {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
        }
        self.draw_bg.draw_walk(cx, walk);

        WidgetDraw::done()
    }
}
