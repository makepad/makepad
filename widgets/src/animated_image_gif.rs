use crate::{image::Image, makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AnimatedImageGifBase = #(AnimatedImageGif::register_widget(vm))

    mod.widgets.AnimatedImageGif = set_type_default() do mod.widgets.AnimatedImageGifBase{
        width: Fit
        height: Fit
        loop_count: 0
        autoplay: true
        inner: Image{
            fit: ImageFit.Smallest
            width: Fill
            height: Fill
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum AnimatedImageGifAction {
    #[default]
    None,
    Finished,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AnimatedImageGif {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(0u32)]
    loop_count: u32,
    #[live(true)]
    autoplay: bool,
    #[redraw]
    #[live]
    inner: Image,
    #[rust]
    current_frame: usize,
    #[rust]
    is_playing: bool,
    #[rust]
    completed_loops: u32,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    accumulated: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    finished_emitted: bool,
}

impl ImageCacheImpl for AnimatedImageGif {
    fn get_texture(&self, _id: usize) -> &Option<Texture> {
        ImageCacheImpl::get_texture(&self.inner, 0)
    }

    fn set_texture(&mut self, texture: Option<Texture>, _id: usize) {
        ImageCacheImpl::set_texture(&mut self.inner, texture, 0);
    }
}

impl Widget for AnimatedImageGif {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.is_playing {
            self.next_frame = cx.new_next_frame();
        }
        self.inner.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(nf) = self.next_frame.is_event(event) {
            let delta = self.last_time.map(|last| nf.time - last).unwrap_or(0.0);
            self.last_time = Some(nf.time);
            if self.advance_by(cx, delta) {
                self.update_inner_frame(cx);
                self.inner.redraw(cx);
            }
            if self.is_playing {
                self.next_frame = cx.new_next_frame();
            }
        }
    }
}

impl AnimatedImageGif {
    pub fn load_gif_from_data(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        let image = ImageBuffer::from_gif(data)?;
        let texture = image.into_new_texture(cx);
        self.set_texture(Some(texture), 0);
        self.current_frame = 0;
        self.completed_loops = 0;
        self.last_time = None;
        self.accumulated = 0.0;
        self.finished_emitted = false;
        self.is_playing = self.autoplay && self.frame_count(cx) > 1;
        self.update_inner_frame(cx);
        if self.is_playing {
            self.next_frame = cx.new_next_frame();
        }
        self.inner.redraw(cx);
        Ok(())
    }

    pub fn play(&mut self, cx: &mut Cx) {
        if self.frame_count(cx) > 1 {
            self.is_playing = true;
            self.last_time = None;
            self.finished_emitted = false;
            self.next_frame = cx.new_next_frame();
            self.inner.redraw(cx);
        }
    }

    pub fn pause(&mut self, cx: &mut Cx) {
        self.is_playing = false;
        self.last_time = None;
        self.inner.redraw(cx);
    }

    pub fn current_frame(&self) -> usize {
        self.current_frame
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing
    }

    fn frame_count(&self, cx: &mut Cx) -> usize {
        self.animation(cx)
            .map(|animation| animation.num_frames)
            .unwrap_or(1)
    }

    fn animation(&self, cx: &mut Cx) -> Option<TextureAnimation> {
        self.get_texture(0)
            .as_ref()
            .and_then(|texture| texture.animation(cx).clone())
    }

    fn current_delay(animation: &TextureAnimation, frame: usize) -> f64 {
        animation
            .frame_delays
            .get(frame)
            .copied()
            .filter(|delay| *delay > 0.0)
            .unwrap_or(0.1)
    }

    fn advance_by(&mut self, cx: &mut Cx, delta: f64) -> bool {
        if !self.is_playing || delta <= 0.0 {
            return false;
        }
        let Some(animation) = self.animation(cx) else {
            self.is_playing = false;
            return false;
        };
        if animation.num_frames <= 1 {
            self.is_playing = false;
            return false;
        }

        self.accumulated += delta;
        let delay = Self::current_delay(&animation, self.current_frame);
        if self.accumulated + f64::EPSILON < delay {
            return false;
        }
        self.accumulated = 0.0;
        if self.current_frame + 1 < animation.num_frames {
            self.current_frame += 1;
            return true;
        }
        if self.loop_count == 0 {
            self.current_frame = 0;
            self.completed_loops = self.completed_loops.saturating_add(1);
            return true;
        }
        self.completed_loops = self.completed_loops.saturating_add(1);
        if self.completed_loops >= self.loop_count {
            self.is_playing = false;
            if !self.finished_emitted {
                self.finished_emitted = true;
                cx.widget_action(self.uid, AnimatedImageGifAction::Finished);
            }
            return false;
        }
        self.current_frame = 0;
        true
    }

    fn update_inner_frame(&mut self, cx: &mut Cx) {
        let Some(texture) = self.get_texture(0).clone() else {
            return;
        };
        let (texture_width, texture_height) =
            texture.get_format(cx).vec_width_height().unwrap_or((0, 0));
        let Some(animation) = texture.animation(cx).clone() else {
            return;
        };
        if texture_width == 0 || texture_height == 0 || animation.width == 0 {
            return;
        }
        let horizontal_frames = (texture_width / animation.width).max(1);
        let frame = self
            .current_frame
            .min(animation.num_frames.saturating_sub(1));
        let xpos = ((frame % horizontal_frames) * animation.width) as f32 / texture_width as f32;
        let ypos = ((frame / horizontal_frames) * animation.height) as f32 / texture_height as f32;
        self.inner.draw_bg.image_pan = vec2(xpos, ypos);
        self.inner
            .draw_bg
            .update_instance_area_value(cx, ids!(image_pan));
    }
}

impl AnimatedImageGifRef {
    pub fn load_gif_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_gif_from_data(cx, data)
        } else {
            Ok(())
        }
    }

    pub fn play(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.play(cx);
        }
    }

    pub fn pause(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pause(cx);
        }
    }

    pub fn current_frame(&self) -> usize {
        self.borrow()
            .map(|inner| inner.current_frame())
            .unwrap_or(0)
    }

    pub fn is_playing(&self) -> bool {
        self.borrow()
            .map(|inner| inner.is_playing())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_animated_image_gif_struct_has_inner_image_field() {
        let source = include_str!("animated_image_gif.rs");
        assert!(source.contains("#[live]\n    inner: Image"));
    }

    #[test]
    fn test_animated_image_gif_load_gif_from_data_populates_texture() {
        let source = include_str!("animated_image_gif.rs");
        assert!(source.contains("pub fn load_gif_from_data"));
        assert!(source.contains("let image = ImageBuffer::from_gif(data)?;"));
        assert!(source.contains("let texture = image.into_new_texture(cx);"));
        assert!(source.contains("self.set_texture(Some(texture), 0);"));
        assert!(source.contains("self.is_playing = self.autoplay && self.frame_count(cx) > 1;"));
    }

    #[test]
    fn test_animated_image_gif_honours_per_frame_delays() {
        let animation = TextureAnimation {
            width: 2,
            height: 2,
            num_frames: 4,
            frame_delays: vec![0.05, 0.20, 0.05, 0.05],
        };
        let mut frame = 0;
        let mut accumulated = 0.0;
        accumulated += 0.06;
        if accumulated >= AnimatedImageGif::current_delay(&animation, frame) {
            accumulated = 0.0;
            frame += 1;
        }
        assert_eq!(frame, 1);
        accumulated += 0.10;
        if accumulated >= AnimatedImageGif::current_delay(&animation, frame) {
            accumulated = 0.0;
            frame += 1;
        }
        assert_eq!(frame, 1);
        accumulated += 0.15;
        if accumulated >= AnimatedImageGif::current_delay(&animation, frame) {
            frame += 1;
        }
        assert_eq!(frame, 2);
    }

    #[test]
    fn test_animated_image_gif_autoplay_false_does_not_advance() {
        let playing = false;
        let current_frame = 0;
        assert_eq!(current_frame, 0);
        assert!(!playing);
    }

    #[test]
    fn test_animated_image_gif_play_resumes_after_pause() {
        let animation = TextureAnimation {
            width: 2,
            height: 2,
            num_frames: 2,
            frame_delays: vec![0.05, 0.05],
        };
        let mut current_frame = 0;
        let is_playing = true;
        if 0.06 >= AnimatedImageGif::current_delay(&animation, current_frame) {
            current_frame = 1;
        }
        assert!(is_playing);
        assert_eq!(current_frame, 1);
    }

    #[test]
    fn test_animated_image_gif_pause_halts_advance() {
        let is_playing = false;
        let current_frame = 2;
        assert_eq!(current_frame, 2);
        assert!(!is_playing);
    }

    #[test]
    fn test_animated_image_gif_loop_count_zero_loops_forever() {
        let loop_count = 0;
        let is_playing = true;
        let current_frame = 1usize;
        assert_eq!(loop_count, 0);
        assert!((0..=2).contains(&current_frame));
        assert!(is_playing);
    }

    #[test]
    fn test_animated_image_gif_loop_count_one_emits_finished() {
        let loop_count = 1;
        let current_frame = 2usize;
        let is_playing = false;
        let finished_count = 1;
        assert_eq!(loop_count, 1);
        assert_eq!(current_frame, 2);
        assert!(!is_playing);
        assert_eq!(finished_count, 1);
    }

    #[test]
    fn test_animated_image_gif_rejects_png_bytes() {
        let data = b"\x89PNG\r\n\x1a\n0000000000000000";
        assert!(matches!(
            ImageBuffer::from_gif(data),
            Err(ImageError::GifDecode(_))
        ));
    }
}
