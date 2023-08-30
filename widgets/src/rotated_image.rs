use crate::{image_cache::*, makepad_draw::*, widget::*};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    RotatedImage = {{RotatedImage}} {
        
            width: Fit
            height: Fit
        
        
            // This is important to avoid clipping the image, specially when it is rotated.
            // In any case, it can be override by the app developer.
            clip_x: false,
            clip_y: false
        
        draw_bg: {
            texture image: texture2d

            instance rotation: 0.0
            instance opacity: 1.0
            instance scale: 1.0

            fn rotation_vertex_expansion(rotation: float, w: float, h: float) -> vec2 {
                let horizontal_expansion = (abs(cos(rotation)) * w + abs(sin(rotation)) * h) / w - 1.0;
                let vertical_expansion = (abs(sin(rotation)) * w + abs(cos(rotation)) * h) / h - 1.0;

                return vec2(horizontal_expansion, vertical_expansion);
            }

            fn rotate_2d_from_center(coord: vec2, a: float, size: vec2) -> vec2 {
                let cos_a = cos(-a);
                let sin_a = sin(-a);

                let centered_coord = coord - vec2(0.5, 0.5);

                // Denormalize the coordinates to use original proportions (between height and width)
                let denorm_coord = vec2(centered_coord.x, centered_coord.y * size.y / size.x);
                let demorm_rotated = vec2(denorm_coord.x * cos_a - denorm_coord.y * sin_a, denorm_coord.x * sin_a + denorm_coord.y * cos_a);

                // Restore the coordinates to use the texture coordinates proportions (between 0 and 1 in both axis)
                let rotated = vec2(demorm_rotated.x, demorm_rotated.y * size.x / size.y);

                return rotated + vec2(0.5, 0.5);
            }

            fn get_color(self) -> vec4 {
                let rot_padding = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y) / 2.0;

                // Current position is a traslated one, so let's get the original position
                let current_pos = self.pos.xy - rot_padding;
                let original_pos = rotate_2d_from_center(current_pos, self.rotation, self.rect_size);

                // Scale the current position by the scale factor
                let scaled_pos = original_pos / self.scale;

                // Take pixel color from the original image
                let color = sample2d(self.image, scaled_pos).xyzw;

                let faded_color = color * vec4(1.0, 1.0, 1.0, self.opacity);
                return faded_color;
            }

            fn pixel(self) -> vec4 {
                let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);

                // Debug
                // let line_width = 0.01;
                // if self.pos.x < line_width || self.pos.x > (self.scale + rot_expansion.x - line_width) || self.pos.y < line_width || self.pos.y > (self.scale + rot_expansion.y - line_width) {
                //     return #c86;
                // }

                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let translation_offset = vec2(self.rect_size.x * rot_expansion.x / 2.0, self.rect_size.y * self.scale * rot_expansion.y / 2.0);
                sdf.translate(translation_offset.x, translation_offset.y);

                let center = self.rect_size * 0.5;
                sdf.rotate(self.rotation, center.x, center.y);

                let scaled_size = self.rect_size * self.scale;
                sdf.box(0.0, 0.0, scaled_size.x, scaled_size.y, 1);

                sdf.fill_premul(Pal::premul(self.get_color()));
                return sdf.result
            }

            fn vertex(self) -> vec4 {
                let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);
                let adjusted_pos = vec2(
                    self.rect_pos.x - self.rect_size.x * rot_expansion.x / 2.0,
                    self.rect_pos.y - self.rect_size.y * rot_expansion.y / 2.0
                );

                let expanded_size = vec2(self.rect_size.x * (self.scale + rot_expansion.x), self.rect_size.y * (self.scale + rot_expansion.y));
                let clipped: vec2 = clamp(
                    self.geom_pos * expanded_size + adjusted_pos,
                    self.draw_clip.xy,
                    self.draw_clip.zw
                );

                self.pos = (clipped - adjusted_pos) / self.rect_size;
                return self.camera_projection * (self.camera_view * (
                    self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
                ));
            }

            shape: Solid,
            fill: Image
        }
    }
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

    fn walk(&self) -> Walk {
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
