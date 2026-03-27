use crate::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MpSurfaceColorFormat {
    #[default]
    BgraU8,
}

pub struct MpSurface {
    pass: DrawPass,
    color_texture: Texture,
    size: Vec2d,
    clear_color: DrawPassClearColor,
}

impl MpSurface {
    pub fn new(
        cx: &mut Cx,
        size: Vec2d,
        color_format: MpSurfaceColorFormat,
        _with_depth: bool,
    ) -> Self {
        let size = normalized_size(size);
        let clear_color = DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0));
        let color_texture =
            Texture::new_with_format(cx, color_texture_format(color_format, size, true));
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        pass.set_color_texture(cx, &color_texture, clear_color.clone());
        Self {
            pass,
            color_texture,
            size,
            clear_color,
        }
    }

    pub fn pass(&self) -> &DrawPass {
        &self.pass
    }

    pub fn color_texture(&self) -> &Texture {
        &self.color_texture
    }

    pub fn size(&self) -> Vec2d {
        self.size
    }

    pub fn resize(&mut self, cx: &mut Cx, size: Vec2d) {
        let size = normalized_size(size);
        if self.size == size {
            return;
        }
        self.size = size;
        *self.color_texture.get_format(cx) = color_texture_format(MpSurfaceColorFormat::BgraU8, size, true);
        self.pass
            .set_color_texture(cx, &self.color_texture, self.clear_color.clone());
        self.pass.set_size(cx, size);
    }

    pub fn begin(&mut self, cx: &mut Cx2d, dpi_factor: Option<f64>) {
        if cx.inside_pass() {
            cx.make_child_pass(&self.pass);
        }
        cx.begin_pass(&self.pass, dpi_factor);
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass(&self.pass);
    }
}

fn normalized_size(size: Vec2d) -> Vec2d {
    dvec2(size.x.max(1.0), size.y.max(1.0))
}

fn fixed_size(size: Vec2d) -> TextureSize {
    TextureSize::Fixed {
        width: size.x.ceil().max(1.0) as usize,
        height: size.y.ceil().max(1.0) as usize,
    }
}

fn color_texture_format(color_format: MpSurfaceColorFormat, size: Vec2d, initial: bool) -> TextureFormat {
    match color_format {
        MpSurfaceColorFormat::BgraU8 => TextureFormat::RenderBGRAu8 {
            size: fixed_size(size),
            initial,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_resize_updates_fixed_render_target() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut surface = MpSurface::new(
            &mut cx,
            dvec2(64.0, 32.0),
            MpSurfaceColorFormat::BgraU8,
            false,
        );

        surface.resize(&mut cx, dvec2(90.0, 45.0));

        match surface.color_texture.get_format(&mut cx) {
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed { width, height },
                ..
            } => {
                assert_eq!((*width, *height), (90, 45));
            }
            other => panic!("unexpected color texture format: {other:?}"),
        }
    }
}
