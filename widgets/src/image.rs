use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    image_cache::*,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptArrayStorage,
    widget::*,
    widget_async::ScriptAsyncResult,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ImageFit = #(ImageFit::script_api(vm))

    set_type_default() do #(DrawImage::script_shader(vm)){
        ..mod.draw.DrawQuad
        image_texture: texture_2d(float)
        opacity: 1.0
        image_scale: vec2(1.0, 1.0)
        image_pan: vec2(0.0, 0.0)
        fit_scale: vec2(1.0, 1.0)
        fit_pan: vec2(0.0, 0.0)
        async_load: 0.0
        rotation: 0.0
        image_dim_w: 0.0
        image_dim_h: 0.0

        get_color_scale_pan: fn(scale: vec2, pan: vec2) {
            // When image_dim is set, rotate the image rigidly and aspect-correct:
            // map each quad pixel back through the rotation into the image's own
            // pixel rect, so non-square images aren't squished at any angle.
            if self.image_dim_w > 0.0 {
                let angle = self.rotation * 3.141592653589793 / 180.0
                let cos_a = cos(-angle)
                let sin_a = sin(-angle)
                let c = (self.pos - vec2(0.5, 0.5)) * self.rect_size
                let cr = vec2(c.x * cos_a - c.y * sin_a, c.x * sin_a + c.y * cos_a)
                let iuv = cr / vec2(self.image_dim_w, self.image_dim_h) + vec2(0.5, 0.5)
                let uv = iuv * scale + pan
                if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                return self.image_texture.sample_as_bgra(uv)
            }
            let uv = self.pos * scale + pan
            return self.image_texture.sample_as_bgra(uv)
        }

        get_color: fn() {
            return self.get_color_scale_pan(
                self.fit_scale * self.image_scale,
                self.fit_pan * self.image_scale + self.image_pan
            )
        }

        pixel: fn() {
            let color = mix(self.get_color(), #3, self.async_load)
            return Pal.premul(vec4(color.xyz, color.w * self.opacity))
        }
    }

    mod.widgets.ImageBase = #(Image::register_widget(vm))

    mod.widgets.Image = set_type_default() do mod.widgets.ImageBase{
        width: 100
        height: 100
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawImage {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub opacity: f32,
    #[live]
    pub image_scale: Vec2f,
    #[live]
    pub image_pan: Vec2f,
    #[live]
    fit_scale: Vec2f,
    #[live]
    fit_pan: Vec2f,
    #[live]
    async_load: f32,
    #[live]
    pub rotation: f32,
    /// When non-zero, `get_color` rotates the image rigidly (aspect-correct):
    /// the image of this pixel size is rotated by `rotation` and inscribed in the
    /// quad, instead of rotating texture UVs in normalized space (which squishes
    /// non-square images). The image viewer drives these per frame.
    #[live]
    pub image_dim_w: f32,
    #[live]
    pub image_dim_h: f32,
}

#[derive(Copy, Clone, Debug, Default, Script, ScriptHook)]
pub enum ImageAnimation {
    Stop,
    Once,
    #[default]
    Loop,
    Bounce,
    #[live(0.0)]
    Frame(f64),
    #[live(0.0)]
    Factor(f64),
    #[live(60.0)]
    OnceFps(f64),
    #[live(60.0)]
    LoopFps(f64),
    #[live(60.0)]
    BounceFps(f64),
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Image {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    pub walk: Walk,
    #[apply_default]
    animator: Animator,
    #[redraw]
    #[live]
    pub draw_bg: DrawImage,
    #[live]
    min_width: u64,
    #[live]
    min_height: u64,
    #[live(1.0)]
    width_scale: f64,
    #[live(ImageAnimation::BounceFps(25.0))]
    animation: ImageAnimation,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    animation_frame: f64,
    #[visible]
    #[live(true)]
    visible: bool,
    #[rust]
    next_frame: NextFrame,
    #[live]
    fit: ImageFit,
    /// HTTP/file resource handle for loading image data (set via `http_resource()` or `crate_resource()`)
    #[live]
    src: Option<ScriptHandleRef>,
    #[rust]
    src_loaded: bool,
    #[rust]
    async_image_path: Option<PathBuf>,
    #[rust]
    async_image_size: Option<(usize, usize)>,
    #[rust]
    texture: Option<Texture>,
    /// `Some` only while showing an SVG (and `texture` is then `None`); lazily
    /// allocated, so non-SVG images pay just a pointer, not a whole `DrawSvg`.
    #[rust]
    draw_svg: Option<Box<DrawSvg>>,
    /// Animation clock (seconds) for animated SVGs, advanced via `next_frame`.
    #[rust]
    svg_time: f64,
}

impl ImageCacheImpl for Image {
    fn get_texture(&self, _id: usize) -> &Option<Texture> {
        &self.texture
    }

    fn set_texture(&mut self, texture: Option<Texture>, _id: usize) {
        self.texture = texture;
        // Keep the invariant that `draw_svg` is `Some` only while showing an SVG.
        self.draw_svg = None;
    }

    fn load_image_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        if looks_like_svg(data) {
            self.load_svg_from_data(cx, data)
        } else {
            let image = decode_image_from_data(data)?;
            self.set_texture(Some(image.into_new_texture(cx)), id);
            Ok(())
        }
    }
}

impl Image {
    fn load_from_resource(&mut self, cx: &mut Cx) {
        if self.src_loaded {
            return;
        }
        let Some(ref handle_ref) = self.src else {
            self.src_loaded = true;
            return;
        };
        let handle = handle_ref.as_handle();
        let data = if let Some(data) = cx.get_resource(handle) {
            data
        } else {
            cx.load_script_resource(handle);
            match cx.get_resource(handle) {
                Some(data) => data,
                None => {
                    let resources = cx.script_data.resources.resources.borrow();
                    if let Some(res) = resources.iter().find(|r| r.handle == handle) {
                        if res.is_error() {
                            drop(resources);
                            self.src_loaded = true;
                            return;
                        }
                    } else {
                        self.src_loaded = true;
                    }
                    return; // Not yet loaded (HTTP pending) — retry on next draw
                }
            }
        };
        self.src_loaded = true;
        self.lazy_create_image_cache(cx);
        let path = {
            let resources = cx.script_data.resources.resources.borrow();
            resources
                .iter()
                .find(|r| r.handle == handle)
                .map(|r| PathBuf::from(&r.abs_path))
                .unwrap_or_else(|| PathBuf::from("http_resource"))
        };
        let _ = self.load_image_from_data_async(cx, &path, Arc::new((*data).clone()));
    }
}

impl Widget for Image {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_src) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if value.is_nil() {
                        vm.with_cx_mut(|cx| {
                            self.src = None;
                            self.src_loaded = false;
                            self.texture = None;
                            self.async_image_path = None;
                            self.async_image_size = None;
                            self.redraw(cx);
                        });
                    } else if let Some(handle) = value.as_handle() {
                        let handle_ref = vm.bx.heap.new_handle_ref(handle);
                        vm.with_cx_mut(|cx| {
                            self.src = Some(handle_ref);
                            self.src_loaded = false;
                            self.texture = None;
                            self.async_image_path = None;
                            self.async_image_size = None;
                            self.redraw(cx);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(load_image_from_data_async) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(data_array) = value.as_array() {
                        if let ScriptArrayStorage::U8(data) = vm.bx.heap.array_storage(data_array) {
                            let path = PathBuf::from(format!(
                                "script_image_data://{}",
                                LiveId::unique().0
                            ));
                            let bytes = Arc::new(data.clone());
                            vm.with_cx_mut(|cx| {
                                let _ = self.load_image_from_data_async(cx, &path, bytes);
                            });
                        }
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        if let Event::NetworkResponses(e) = event {
            handle_image_cache_network_responses(cx, e);
        }
        // lets check if we have a post action
        if let Event::Actions(actions) = &event {
            for action in actions {
                if let Some(AsyncImageLoad { image_path, result }) = &action.downcast_ref() {
                    if let Some(result) = result.borrow_mut().take() {
                        // we have a result for the image_cache to load up
                        self.process_async_image_load(cx, image_path, result);
                    }
                    if self.async_image_size.is_some()
                        && self.async_image_path.clone() == Some(image_path.to_path_buf())
                    {
                        // see if we can load from cache
                        self.load_image_from_cache(cx, image_path, 0);
                        self.async_image_size = None;
                        self.animator_play(cx, ids!(async_load.off));
                        self.redraw(cx);
                    }
                }
            }
        }
        if let Some(nf) = self.next_frame.is_event(event) {
            // compute the next frame and patch things up
            if self.draw_svg.is_some() {
                // Animated SVG: advance the clock; the draw step reschedules.
                self.svg_time = nf.time;
                self.redraw(cx);
            } else if let Some(image_texture) = &self.texture {
                let (texture_width, texture_height) = image_texture
                    .get_format(cx)
                    .vec_width_height()
                    .unwrap_or((self.min_width as usize, self.min_height as usize));
                if let Some(animation) = image_texture.animation(cx).clone() {
                    let delta = if let Some(last_time) = &self.last_time {
                        nf.time - last_time
                    } else {
                        0.0
                    };
                    self.last_time = Some(nf.time);
                    let num_frames = animation.num_frames as f64;
                    match self.animation {
                        ImageAnimation::Stop => {}
                        ImageAnimation::Frame(frame) => {
                            self.animation_frame = frame;
                        }
                        ImageAnimation::Factor(pos) => {
                            self.animation_frame = pos * (num_frames - 1.0);
                        }
                        ImageAnimation::Once => {
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames {
                                self.animation_frame = num_frames - 1.0;
                            } else {
                                self.next_frame = cx.new_next_frame();
                            }
                        }
                        ImageAnimation::Loop => {
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames {
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::Bounce => {
                            self.animation_frame += 1.0;
                            if self.animation_frame >= num_frames * 2.0 {
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::OnceFps(fps) => {
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames {
                                self.animation_frame = num_frames - 1.0;
                            } else {
                                self.next_frame = cx.new_next_frame();
                            }
                        }
                        ImageAnimation::LoopFps(fps) => {
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames {
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                        ImageAnimation::BounceFps(fps) => {
                            self.animation_frame += delta * fps;
                            if self.animation_frame >= num_frames * 2.0 {
                                self.animation_frame = 0.0;
                            }
                            self.next_frame = cx.new_next_frame();
                        }
                    }
                    // alright now lets turn animation_frame into the right image_pan
                    let last_pan = self.draw_bg.image_pan;

                    let frame = if self.animation_frame >= num_frames {
                        num_frames * 2.0 - 1.0 - self.animation_frame
                    } else {
                        self.animation_frame
                    } as usize;

                    let horizontal_frames = texture_width / animation.width;
                    let xpos = ((frame % horizontal_frames) * animation.width) as f32
                        / texture_width as f32;
                    let ypos = ((frame / horizontal_frames) * animation.height) as f32
                        / texture_height as f32;
                    self.draw_bg.image_pan = vec2(xpos, ypos);
                    if self.draw_bg.image_pan != last_pan {
                        // patch it into the area
                        self.draw_bg.update_instance_area_value(cx, ids!(image_pan))
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.load_from_resource(cx);
        self.draw_walk_image(cx, walk)
    }
}

impl Image {
    fn set_crop_to_fill_transform(
        &mut self,
        source_width: f64,
        source_height: f64,
        target_width: f64,
        target_height: f64,
    ) {
        if !source_width.is_finite()
            || !source_height.is_finite()
            || !target_width.is_finite()
            || !target_height.is_finite()
            || source_width <= 0.0
            || source_height <= 0.0
            || target_width <= 0.0
            || target_height <= 0.0
        {
            self.draw_bg.fit_scale = vec2(1.0, 1.0);
            self.draw_bg.fit_pan = vec2(0.0, 0.0);
            return;
        }

        let source_aspect = source_width / source_height;
        let target_aspect = target_width / target_height;
        let mut crop_scale = vec2(1.0, 1.0);

        if source_aspect > target_aspect {
            crop_scale.x = (target_aspect / source_aspect) as f32;
        } else {
            crop_scale.y = (source_aspect / target_aspect) as f32;
        }

        let crop_pan = (vec2(1.0, 1.0) - crop_scale) * 0.5;
        self.draw_bg.fit_scale = crop_scale;
        self.draw_bg.fit_pan = crop_pan;
    }

    /// Returns the original size of the image in pixels (not its displayed size).
    ///
    /// Returns `None` if the image has not been loaded into a texture yet.
    pub fn size_in_pixels(&self, cx: &mut Cx) -> Option<(usize, usize)> {
        if let Some(draw_svg) = self.draw_svg.as_ref() {
            return draw_svg
                .svg_size()
                .map(|sz| (sz.x as usize, sz.y as usize));
        }
        self.texture
            .as_ref()
            .and_then(|t| t.get_format(cx).vec_width_height())
    }

    /// True if a texture has been set on this `Image`.
    pub fn has_texture(&self) -> bool {
        self.texture.is_some()
    }

    /// Loads an SVG into this `Image` by parsing the UTF-8 SVG `data` and drawing
    /// it with makepad's native vector engine instead of a raster texture.
    ///
    /// The `DrawSvg` is allocated on first use, so images that never show an SVG
    /// carry only a null pointer.
    pub fn load_svg_from_data(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        let svg_str = std::str::from_utf8(data).map_err(|_| ImageError::UnsupportedFormat)?;
        if self.draw_svg.is_none() {
            self.draw_svg = Some(cx.with_vm(|vm| Box::new(DrawSvg::script_new_with_default(vm))));
        }
        if let Some(draw_svg) = self.draw_svg.as_mut() {
            draw_svg.load_from_str(svg_str);
        }
        self.texture = None;
        self.redraw(cx);
        Ok(())
    }

    pub fn draw_walk_image(&mut self, cx: &mut Cx2d, mut walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let svg_time = self.svg_time as f32;
        if let Some(draw_svg) = self.draw_svg.as_mut() {
            draw_svg.draw_walk_time(cx, walk, svg_time);
            let animating = draw_svg.has_animations;
            if animating {
                // Keep ticking so SMIL/CSS-animated SVGs advance.
                self.next_frame = cx.new_next_frame();
            }
            return DrawStep::done();
        }
        // alright we get a walk. depending on our aspect ratio
        // we change either nothing, or width or height
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();

        let (width, height) = if let Some((w, h)) = &self.async_image_size {
            // still loading
            (*w as f64, *h as f64)
        } else if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
            let (width, height) = image_texture
                .get_format(cx)
                .vec_width_height()
                .unwrap_or((self.min_width as usize, self.min_height as usize));
            if let Some(animation) = image_texture.animation(cx) {
                let (w, h) = (animation.width as f64, animation.height as f64);
                self.next_frame = cx.new_next_frame();
                // we have an animation. lets compute the scale and zoom for a certain frame
                let scale_x = w as f32 / width as f32;
                let scale_y = h as f32 / height as f32;
                self.draw_bg.image_scale = vec2(scale_x, scale_y);
                (w, h)
            } else if image_texture.get_format(cx).is_render() {
                // GL render target textures use Y-up (bottom-left origin) convention.
                // Flip Y so the image displays correctly in Makepad's Y-down coordinate system.
                self.draw_bg.image_scale = vec2(1.0, -1.0);
                self.draw_bg.image_pan = vec2(0.0, 1.0);
                (width as f64 * self.width_scale, height as f64)
            } else {
                (width as f64 * self.width_scale, height as f64)
            }
        } else {
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };

        let aspect = width / height;
        // When `walk.height` is `Size::Fit { max: Some(m) }`, the rect returned by
        // `peek_walk_turtle` has `size.y == NaN` (Fit evaluates to NaN — see
        // `Turtle::next_walk_height`), so the max would otherwise be ignored by the
        // aspect calculations below. Treat it as the effective vertical bound so
        // the image scales proportionally instead of rendering at its natural
        // height and getting clipped by an outer view.
        let height_cap = if let Size::Fit { max: Some(fb), .. } = walk.height {
            fb.eval_height(cx).unwrap_or(f64::INFINITY)
        } else {
            f64::INFINITY
        };
        let avail_height = if rect.size.y.is_nan() {
            height_cap
        } else {
            rect.size.y.min(height_cap)
        };
        self.draw_bg.fit_scale = vec2(1.0, 1.0);
        self.draw_bg.fit_pan = vec2(0.0, 0.0);
        match self.fit {
            ImageFit::Size => {
                walk.width = Size::Fixed(width);
                walk.height = Size::Fixed(height);
            }
            ImageFit::Stretch => {}
            ImageFit::CropToFill => {
                self.set_crop_to_fill_transform(
                    width,
                    height,
                    rect.size.x,
                    avail_height,
                );
            }
            ImageFit::Horizontal => {
                walk.height = Size::Fixed(rect.size.x / aspect);
            }
            ImageFit::Vertical => {
                walk.width = Size::Fixed(avail_height * aspect);
                walk.height = Size::Fixed(avail_height);
            }
            ImageFit::Smallest => {
                let walk_height = rect.size.x / aspect;
                if walk_height > avail_height {
                    walk.width = Size::Fixed(avail_height * aspect);
                    walk.height = Size::Fixed(avail_height);
                } else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
            ImageFit::Biggest => {
                let walk_height = rect.size.x / aspect;
                if walk_height < avail_height {
                    walk.width = Size::Fixed(avail_height * aspect);
                    walk.height = Size::Fixed(avail_height);
                } else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
        }

        self.draw_bg.draw_walk(cx, walk);

        DrawStep::done()
    }

    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path_async(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
    ) -> Result<(), ImageError> {
        self.lazy_create_image_cache(cx);
        if let Ok(result) = self.load_image_file_by_path_async_impl(cx, image_path, 0) {
            match result {
                AsyncLoadResult::Loading(w, h) => {
                    self.async_image_size = Some((w, h));
                    self.async_image_path = Some(image_path.into());
                    self.animator_play(cx, ids!(async_load.on));
                    self.redraw(cx);
                }
                AsyncLoadResult::Loaded => {
                    self.redraw(cx);
                }
            }
        }
        Ok(())
    }

    pub fn load_image_from_data_async(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<Vec<u8>>,
    ) -> Result<(), ImageError> {
        self.lazy_create_image_cache(cx);
        if let Ok(result) = self.load_image_from_data_async_impl(cx, image_path, data, 0) {
            match result {
                AsyncLoadResult::Loading(w, h) => {
                    self.async_image_size = Some((w, h));
                    self.async_image_path = Some(image_path.into());
                    self.animator_play(cx, ids!(async_load.on));
                    self.redraw(cx);
                }
                AsyncLoadResult::Loaded => {
                    self.redraw(cx);
                }
            }
        }
        Ok(())
    }

    pub fn load_image_http_by_url_async(
        &mut self,
        cx: &mut Cx,
        url: &str,
    ) -> Result<(), ImageError> {
        self.lazy_create_image_cache(cx);
        if let Ok(result) = self.load_image_http_by_url_async_impl(cx, url, 0) {
            match result {
                AsyncLoadResult::Loading(w, h) => {
                    self.async_image_size = Some((w, h));
                    self.async_image_path = Some(PathBuf::from(url));
                    self.animator_play(cx, ids!(async_load.on));
                    self.redraw(cx);
                }
                AsyncLoadResult::Loaded => {
                    self.redraw(cx);
                }
            }
        }
        Ok(())
    }
}

pub enum AsyncLoad {
    Yes,
    No,
}

impl ImageRef {
    /// Loads the image at the given `image_path` resource into this `ImageRef`.
    pub fn load_image_dep_by_path(&self, cx: &mut Cx, image_path: &str) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            inner.load_image_dep_by_path(cx, image_path, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path(
        &self,
        cx: &mut Cx,
        image_path: &Path,
    ) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            inner.load_image_file_by_path(cx, image_path, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_file_by_path_async(
        &self,
        cx: &mut Cx,
        image_path: &Path,
    ) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.load_image_file_by_path_async(cx, image_path);
        }
        Ok(())
    }

    /// Loads the image at the given `image_path` on disk into this `ImageRef`.
    pub fn load_image_from_data_async(
        &self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<Vec<u8>>,
    ) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.load_image_from_data_async(cx, image_path, data);
        }
        Ok(())
    }

    /// Loads an image from a URL using platform HTTP + async decode.
    pub fn load_image_http_by_url_async(&self, cx: &mut Cx, url: &str) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.load_image_http_by_url_async(cx, url);
        }
        Ok(())
    }

    /// Loads a JPEG into this `ImageRef` by decoding the given encoded JPEG `data`.
    pub fn load_jpg_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_jpg_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads a PNG into this `ImageRef` by decoding the given encoded PNG `data`.
    pub fn load_png_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_png_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads a BMP into this `ImageRef` by decoding the given encoded BMP `data`.
    pub fn load_bmp_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_bmp_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads a QOI into this `ImageRef` by decoding the given encoded QOI `data`.
    pub fn load_qoi_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_qoi_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads an ICO into this `ImageRef` by decoding the given encoded ICO `data`.
    pub fn load_ico_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_ico_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads a GIF into this `ImageRef` by decoding the given encoded GIF `data`.
    pub fn load_gif_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_gif_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads a WebP into this `ImageRef` by decoding the given encoded WebP `data`.
    pub fn load_webp_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_webp_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads an image into this `ImageRef` by decoding the given encoded `data`,
    /// auto-detecting any image format that makepad supports (including SVG).
    pub fn load_image_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.lazy_create_image_cache(cx);
            ImageCacheImpl::load_image_from_data(&mut *inner, cx, data, 0)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    /// Loads an SVG into this `ImageRef` by rendering the given UTF-8 SVG `data`
    /// with makepad's native vector engine.
    pub fn load_svg_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_svg_from_data(cx, data)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    pub fn set_texture(&self, cx: &mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.texture = texture;
            if cx.in_draw_event() {
                inner.redraw(cx);
            }
        }
    }

    pub fn set_uniform(&self, cx: &Cx, uniform: LiveId, value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }

    /// See [`Image::size_in_pixels()`].
    pub fn size_in_pixels(&self, cx: &mut Cx) -> Option<(usize, usize)> {
        if let Some(inner) = self.borrow() {
            inner.size_in_pixels(cx)
        } else {
            None
        }
    }

    /// See [`Image::has_texture()`].
    pub fn has_texture(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.has_texture()
        } else {
            false
        }
    }
}
