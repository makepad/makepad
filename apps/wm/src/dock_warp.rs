//! A whole-window texture pulled into its dock icon. No app layout or resize
//! occurs during the warp; the same frozen surface is used for restoration.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*
    mod.widgets.DrawDockWarp = set_type_default() do #(DrawDockWarp::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        dock: vec4(0.0, 0.0, 1.0, 1.0)
        progress: 0.0
        y_flip: 0.0
        pixel: fn() {
            let p = self.rect_pos + self.pos * self.rect_size
            let pull = smoothstep(0.0, 0.65, self.progress)
            let suck = smoothstep(0.25, 1.0, self.progress)
            let top = mix(self.source.y, self.dock.y, suck)
            let bottom = max(top + 0.5, mix(self.source.y + self.source.w, self.dock.y + self.dock.w, pull))
            let v = (p.y - top) / (bottom - top)
            if v < 0.0 || v > 1.0 { discard() }
            let curve = smoothstep(0.0, 1.0, v) * pull
            let bend = curve + (1.0 - curve) * suck
            let left = mix(self.source.x, self.dock.x, bend)
            let width = mix(self.source.z, self.dock.z, bend)
            let u = (p.x - left) / max(width, 0.5)
            if u < 0.0 || u > 1.0 { discard() }
            let aa = max(length(vec2(dFdx(u), dFdy(u))), 0.00001)
            let edge = smoothstep(0.0, aa, u) * smoothstep(0.0, aa, 1.0-u)
            let uv = vec2(u, mix(v, 1.0-v, self.y_flip))
            return self.image.sample(uv) * edge * (1.0 - smoothstep(0.94, 1.0, self.progress))
        }
    }
}
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDockWarp {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    source: Vec4f,
    #[live]
    dock: Vec4f,
    #[live]
    progress: f32,
    #[live]
    y_flip: f32,
}

pub struct WindowFrame {
    pass: DrawPass,
    list: DrawList2d,
    texture: Texture,
    _depth: Texture,
    frozen: bool,
}
impl WindowFrame {
    pub fn new(cx: &mut Cx) -> Self {
        Self::new_with_name(cx, "wm_dock_window")
    }
    pub fn new_with_name(cx: &mut Cx, name: &str) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        let depth = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        pass.set_color_texture(
            cx,
            &texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        Self {
            pass,
            list: DrawList2d::new(cx),
            texture,
            _depth: depth,
            frozen: false,
        }
    }
    pub fn frozen(&self) -> bool {
        self.frozen
    }
    pub fn pass_id(&self) -> DrawPassId {
        self.pass.draw_pass_id()
    }
    pub fn texture(&self) -> &Texture { &self.texture }
    pub fn begin(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.frozen = false;
        if std::env::var_os("MAKEPAD_WM_TRACE_WARP").is_some() {
            log!("warp: capture begin {:?}", rect);
        }
        let dpi = cx.current_dpi_factor();
        let root_size = cx.owning_window_or_root_pass_size();
        self.pass.set_size(cx, rect.size);
        cx.set_pass_shift_scale(&self.pass, rect.pos, dvec2(1.0, 1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(dpi));
        self.list.begin_always(cx);
        cx.begin_root_turtle(root_size, Layout::flow_overlay());
    }
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
        if std::env::var_os("MAKEPAD_WM_TRACE_WARP").is_some() {
            log!("warp: capture end");
        }
    }
    pub fn freeze(&mut self, cx: &mut Cx) {
        if self.frozen {
            return;
        }
        let pass = &mut cx.passes[self.pass.draw_pass_id()];
        // Several logical draws may precede a GPU submission. Keep the producer
        // attached until its first paint, otherwise its texture remains empty.
        if pass.paint_dirty {
            return;
        }
        pass.main_draw_list_id = None;
        pass.parent = CxDrawPassParent::None;
        pass.paint_dirty = false;
        pass.live_with_parent = false;
        self.frozen = true;
    }
}

pub struct DockWarp {
    pub source: Rect,
    pub dock: Rect,
    pub progress: f64,
    pub minimized: bool,
    pub frame: Option<WindowFrame>,
    pub refresh: bool,
    duration: f64,
}
impl DockWarp {
    pub fn new(source: Rect, dock: Rect, restoring: bool) -> Self {
        Self {
            source,
            dock,
            progress: if restoring { 1.0 } else { 0.0 },
            minimized: !restoring,
            frame: None,
            refresh: false,
            duration: if cfg!(test) {
                0.62
            } else {
                std::env::var("MAKEPAD_WM_WARP_SECONDS")
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|s| s.is_finite() && *s >= 0.1 && *s <= 30.0)
                    .unwrap_or(0.62)
            },
        }
    }
    pub fn active(&self) -> bool {
        if self.minimized {
            self.progress < 1.0
        } else {
            self.progress > 0.0
        }
    }
    pub fn step(&mut self, dt: f64) {
        let direction = if self.minimized { 1.0 } else { -1.0 };
        self.progress = (self.progress + direction * dt / self.duration).clamp(0.0, 1.0);
    }
    pub fn bounds(&self) -> Rect {
        let x = self.source.pos.x.min(self.dock.pos.x);
        let y = self.source.pos.y.min(self.dock.pos.y);
        let right =
            (self.source.pos.x + self.source.size.x).max(self.dock.pos.x + self.dock.size.x);
        let bottom =
            (self.source.pos.y + self.source.size.y).max(self.dock.pos.y + self.dock.size.y);
        Rect {
            pos: dvec2(x, y),
            size: dvec2(right - x, bottom - y),
        }
    }
    pub fn draw(&mut self, cx: &mut Cx2d, draw: &mut DrawDockWarp) {
        let Some(frame) = &self.frame else {
            return;
        };
        draw.draw_vars.set_texture(0, &frame.texture);
        draw.source = vec4(
            self.source.pos.x as f32,
            self.source.pos.y as f32,
            self.source.size.x as f32,
            self.source.size.y as f32,
        );
        draw.dock = vec4(
            self.dock.pos.x as f32,
            self.dock.pos.y as f32,
            self.dock.size.x as f32,
            self.dock.size.y as f32,
        );
        draw.progress = self.progress as f32;
        draw.y_flip = if matches!(cx.os_type(), OsType::Android(_)) {
            1.0
        } else {
            0.0
        };
        draw.draw_abs(cx, self.bounds());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reversal_keeps_the_same_surface_and_progress() {
        let source = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(640.0, 480.0),
        };
        let dock = Rect {
            pos: dvec2(900.0, 820.0),
            size: dvec2(48.0, 48.0),
        };
        let mut warp = DockWarp::new(source, dock, false);
        warp.step(0.2);
        let progress = warp.progress;
        warp.minimized = false;
        assert_eq!(warp.progress, progress);
        warp.step(0.1);
        assert!(warp.progress < progress && warp.active());
        warp.step(1.0);
        assert_eq!(warp.progress, 0.0);
        assert!(!warp.active());
        assert_eq!(warp.source, source);
        let bounds = warp.bounds();
        assert!(bounds.contains(source.pos) && bounds.contains(dock.pos));
        warp.minimized = true;
        warp.step(1.0);
        assert_eq!(warp.progress, 1.0);
        assert!(!warp.active());
    }
}
