//! Lane D. The material texture thumbnail: one decoded RGBA8 image from the
//! document, drawn aspect-fit over a checkerboard (so alpha reads as alpha),
//! uploaded to the GPU **once per image** and cached by a stable key — never
//! per frame. The Properties panel's Material tab builds its texture rows out
//! of these; clicking one raises the panel's enlarge popover.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    set_type_default() do #(DrawTexThumb::script_shader(vm)){
        ..mod.draw.DrawQuad

        tex: texture_2d(float)
        has_tex: 0.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
            // Checkerboard under the image so an alpha cutout is visible.
            let cell = floor(self.pos * self.rect_size / 6.0)
            let check = modf(cell.x + cell.y, 2.0)
            let bg = vec4(0.16, 0.16, 0.16, 1.0).mix(vec4(0.22, 0.22, 0.22, 1.0), check)
            let img = self.tex.sample_as_bgra(self.pos)
            let color = bg.mix(vec4(img.xyz, 1.0), img.w * self.has_tex)
            sdf.fill_keep(color)
            sdf.stroke(fab.color_border, 1.0)
            return sdf.result
        }
    }

    mod.widgets.FabTexThumbBase = #(FabTexThumb::register_widget(vm))
    mod.widgets.FabTexThumb = set_type_default() do mod.widgets.FabTexThumbBase{
        width: 48
        height: 48
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTexThumb {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    has_tex: f32,
}

#[derive(Clone, Debug, Default)]
pub enum TexThumbAction {
    Clicked,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabTexThumb {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_tex: DrawTexThumb,
    #[walk]
    walk: Walk,
    /// The image the current GPU texture was built from; a new key uploads
    /// once, the same key never re-uploads.
    #[rust]
    cache_key: u64,
    #[rust]
    texture: Option<Texture>,
    #[rust]
    img_size: (u32, u32),
    #[rust]
    down: bool,
}

impl FabTexThumb {
    /// Show `rgba8` (`w`×`h`, row-major). `key` identifies the image —
    /// uploading happens only when it changes.
    pub fn set_image(&mut self, cx: &mut Cx, key: u64, w: u32, h: u32, rgba8: &[u8]) {
        if key == self.cache_key && self.texture.is_some() {
            return;
        }
        let pixels = (w as usize) * (h as usize);
        if pixels == 0 || rgba8.len() < pixels * 4 {
            self.clear(cx);
            return;
        }
        let mut data = Vec::with_capacity(pixels);
        for p in 0..pixels {
            let r = rgba8[p * 4] as u32;
            let g = rgba8[p * 4 + 1] as u32;
            let b = rgba8[p * 4 + 2] as u32;
            let a = rgba8[p * 4 + 3] as u32;
            data.push((a << 24) | (r << 16) | (g << 8) | b);
        }
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: w as usize,
                height: h as usize,
                data: Some(data),
                updated: TextureUpdated::Full,
            },
        );
        self.draw_tex.draw_super.draw_vars.set_texture(0, &texture);
        self.texture = Some(texture);
        self.cache_key = key;
        self.img_size = (w, h);
        self.draw_tex.has_tex = 1.0;
        self.draw_tex.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        if self.texture.is_none() && self.cache_key == 0 {
            return;
        }
        self.texture = None;
        self.cache_key = 0;
        self.img_size = (0, 0);
        self.draw_tex.has_tex = 0.0;
        self.draw_tex.redraw(cx);
    }

    pub fn image_size(&self) -> (u32, u32) {
        self.img_size
    }
}

impl Widget for FabTexThumb {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Aspect-fit inside the walk's rect: the widget's box is the hit
        // target and checker frame, the image keeps its own proportions.
        let rect = cx.walk_turtle(walk);
        let mut draw = rect;
        if self.img_size.0 > 0 && self.img_size.1 > 0 {
            let iw = self.img_size.0 as f64;
            let ih = self.img_size.1 as f64;
            let scale = (rect.size.x / iw).min(rect.size.y / ih);
            let size = dvec2(iw * scale, ih * scale);
            draw = Rect {
                pos: rect.pos + (rect.size - size) * 0.5,
                size,
            };
        }
        self.draw_tex.draw_abs(cx, draw);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_tex.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.down = true;
            }
            Hit::FingerUp(fe) => {
                if self.down && fe.is_over {
                    cx.widget_action(uid, TexThumbAction::Clicked);
                }
                self.down = false;
            }
            _ => {}
        }
    }
}

impl FabTexThumbRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let TexThumbAction::Clicked = item.cast() {
                return true;
            }
        }
        false
    }

    pub fn set_image(&self, cx: &mut Cx, key: u64, w: u32, h: u32, rgba8: &[u8]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_image(cx, key, w, h, rgba8);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    pub fn image_size(&self) -> (u32, u32) {
        self.borrow().map_or((0, 0), |i| i.image_size())
    }
}
