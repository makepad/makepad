use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    image_cache::*,
    image_slice::*,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptArrayStorage,
    widget::*,
    widget_async::ScriptAsyncResult,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SVG_BYTES: usize = 16 * 1024 * 1024;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ImageFit = #(ImageFit::script_api(vm))
    mod.widgets.ImageSliceEdge = #(ImageSliceEdge::script_api(vm))
    mod.widgets.ImageSliceCenter = #(ImageSliceCenter::script_api(vm))
    mod.widgets.ImageSliceUnits = #(ImageSliceUnits::script_api(vm))

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
        sample_mode: 0.0
        image_dim_w: 0.0
        image_dim_h: 0.0
        border_radius: 0.0
        border_size: 0.0
        border_color: #0000
        letterbox_color: #0000
        slice_inset: vec4(0.0, 0.0, 0.0, 0.0)
        slice_texel_points: 1.0
        slice_mode: vec2(0.0, 0.0)

        // The texture at `uv`, filtered or as one whole texel by `sample_mode`.
        // Whole texels are had by snapping to the texel's centre and reading
        // with the ordinary filter: at the magnification where it matters
        // every tap then lands on that one texel. Snapping rather than asking
        // the sampler for nearest keeps the read on `sample_as_bgra`, the one
        // form whose channel order the web backend corrects; a nearest read
        // there comes back with red and blue swapped. `scale` is the share of
        // the texture the quad spans, so a cropped picture measures its texels
        // at the size they are drawn.
        sample_at: fn(uv: vec2, scale: vec2) -> vec4 {
            // Nobody asked: the filtered read, and none of the arithmetic
            // below is reached to arrive at it.
            if self.sample_mode == 0.0 {
                return self.image_texture.sample_as_bgra(uv)
            }
            let size = self.image_texture.size()
            let texels_x = max(size.x, 1.0)
            let texels_y = max(size.y, 1.0)
            let device_px_per_texel = self.rect_size.x * self.sample_mode / (texels_x * max(scale.x, 0.0001))
            if self.sample_mode < 0.0 || device_px_per_texel > 4.0 {
                let snapped = vec2(
                    (floor(uv.x * texels_x) + 0.5) / texels_x,
                    (floor(uv.y * texels_y) + 0.5) / texels_y
                )
                return self.image_texture.sample_as_bgra(snapped)
            }
            return self.image_texture.sample_as_bgra(uv)
        }

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
                    return self.letterbox_color
                }
                return self.sample_at(uv, scale)
            }
            let uv = self.pos * scale + pan
            return self.sample_at(uv, scale)
        }

        // One axis of a sliced picture: where along the window the point `p` of a
        // box `len` long reads, in texels, and how many points a texel is drawn at
        // there. `a` and `b` are the borders in texels and `s` the points per border
        // texel after any shrink. `mode` is 1 stretch, 2 tile, 3 a whole number of
        // tiles. Every read stays half a texel inside its own part, so a filtered
        // read never borrows the texels of the part beside it. This is
        // `image_slice::slice_axis`, line for line.
        slice_axis: fn(p: float, len: float, texels: float, a: float, b: float, s: float, mode: float) -> vec2 {
            let n = max(texels - a - b, 0.0)
            let start = a * s
            let end = len - b * s
            if p < start {
                return vec2(clamp(p / s, 0.5, max(a - 0.5, 0.5)), s)
            }
            if p >= end {
                let first = texels - b
                return vec2(clamp(first + (p - end) / s, first + 0.5, max(texels - 0.5, first + 0.5)), s)
            }
            let mid = max(end - start, 0.0001)
            let lo = a + 0.5
            let hi = max(a + n - 0.5, lo)
            if mode >= 2.0 {
                let mut tile = max(n * s, 0.0001)
                if mode >= 3.0 {
                    tile = mid / max(floor(mid / tile + 0.5), 1.0)
                }
                let t = fract((p - start - mid * 0.5) / tile + 0.5)
                return vec2(clamp(a + t * n, lo, hi), tile / max(n, 0.0001))
            }
            return vec2(clamp(a + (p - start) / mid * n, lo, hi), mid / max(n, 0.0001))
        }

        // The sliced picture's colour here: the borders shrink together when the
        // box cannot hold them, each axis picks the mode of the part it is in, and
        // the read goes through `sample_at` like every other read of this texture.
        // `at` is measured on the unclipped rect, so a panel a scroll view has half
        // hidden keeps its corners where they belong.
        slice_color: fn() -> vec4 {
            let texture = max(self.image_texture.size(), vec2(1.0, 1.0))
            let window = max(texture * self.image_scale, vec2(1.0, 1.0))
            let size = max(self.rect_size, vec2(0.0001, 0.0001))
            let at = self.pos * self.rect_size
            let inset = self.slice_inset
            let wide = (inset.x + inset.z) * self.slice_texel_points
            let tall = (inset.y + inset.w) * self.slice_texel_points
            let mut fit = 1.0
            if wide > 0.0 {
                fit = min(fit, size.x / wide)
            }
            if tall > 0.0 {
                fit = min(fit, size.y / tall)
            }
            let s = max(self.slice_texel_points * fit, 0.0001)
            let across = step(inset.x * s, at.x) * (1.0 - step(size.x - inset.z * s, at.x))
            let down = step(inset.y * s, at.y) * (1.0 - step(size.y - inset.w * s, at.y))
            if across * down > 0.5 && self.slice_mode.y == 0.0 {
                return #0000
            }
            // A top or bottom edge fills across with the edges' mode and the
            // middle with its own; the same holds down the left and right.
            let mode_x = mix(self.slice_mode.x, self.slice_mode.y, down)
            let mode_y = mix(self.slice_mode.x, self.slice_mode.y, across)
            let x = self.slice_axis(at.x, size.x, window.x, inset.x, inset.z, s, mode_x)
            let y = self.slice_axis(at.y, size.y, window.y, inset.y, inset.w, s, mode_y)
            let read = self.image_pan + vec2(x.x, y.x) / texture
            // The share `sample_at` measures texel density against, chosen so it
            // works out the density this part is actually drawn at.
            let share = size / (texture * max(vec2(x.y, y.y), vec2(0.0001, 0.0001)))
            return self.sample_at(read, share)
        }

        get_color: fn() {
            // A sliced picture maps every pixel through its nine parts and is never
            // framed: the widget only turns this on with the framing at rest.
            if self.slice_mode.x > 0.0 {
                return self.slice_color()
            }
            // Where the FRAMING left the picture behind is a bar, not the edge
            // texel smeared across the rest of the box: a picture the box
            // cannot hold whole leaves its ends over, and `fit_scale` and
            // `fit_pan` are where that is written down. What the caller pans
            // and zooms for itself is not framing — `image_pan` is how a
            // sprite sheet picks a cell and how a viewer moves a picture
            // around under its window, and both still get the edge texel they
            // always got, because at rest the framing is 1 and 0 and this
            // never fires. The rotated path tests its own mapping for itself.
            if self.image_dim_w <= 0.0 {
                let framed = self.pos * self.fit_scale + self.fit_pan
                // A framing that only just reaches the texture's own edge is
                // still inside it. At rest the framing is exactly 1 and 0,
                // which puts this comparison on the boundary at the quad's
                // last pixel, and an edge fragment interpolated a hair past
                // one would come back a clear bar where every caller has
                // always had the edge texel. The slack is well under one
                // texel of any texture, so a bar that was really asked for
                // still begins where it began.
                if framed.x < -0.0001 || framed.x > 1.0001 || framed.y < -0.0001 || framed.y > 1.0001 {
                    return self.letterbox_color
                }
            }
            return self.get_color_scale_pan(
                self.fit_scale * self.image_scale,
                self.fit_pan * self.image_scale + self.image_pan
            )
        }

        pixel: fn() {
            let color = mix(self.get_color(), #3, self.async_load)
            let picture = Pal.premul(vec4(color.xyz, color.w * self.opacity))
            // No radius and no stroke is the plain quad, returned before any
            // of the shape work below.
            if self.border_radius <= 0.0 && self.border_size <= 0.0 {
                return picture
            }
            // The box RoundedView draws, with the picture for its fill: the
            // number that rounds a view rounds the picture in it the same, and
            // the stroke sits on the edge where the view's does, half in and
            // half out, so the two line up when they meet.
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size,
                self.border_size,
                self.rect_size.x - self.border_size * 2.0,
                self.rect_size.y - self.border_size * 2.0,
                self.border_radius
            )
            sdf.fill_keep_premul(picture)
            if self.border_size > 0.0 {
                sdf.stroke(vec4(self.border_color.xyz, self.border_color.w * self.opacity), self.border_size)
            }
            return sdf.result
        }
    }

    mod.widgets.ImageBase = #(Image::register_widget(vm))

    mod.widgets.Image = set_type_default() do mod.widgets.ImageBase{
        width: 100
        height: 100
        /** how much of a `CropToFill` picture's overflow is cropped away 0..1 step 0.05 */
        crop: 1.0
        /** texels at each edge a sliced picture keeps at their own size 0..256 step 1 */
        slice: 0
        /** how a sliced picture's edges fill their length */
        slice_edge: mod.widgets.ImageSliceEdge.Stretch
        /** how a sliced picture's middle fills */
        slice_center: mod.widgets.ImageSliceCenter.Stretch
        /** size one border texel is drawn at 0.25..8 step 0.25 */
        slice_scale: 1.0
        /** whether slice_scale counts layout points or device pixels */
        slice_units: mod.widgets.ImageSliceUnits.Points
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
    /// How the texture is read. Zero is filtered. Below zero every read is one
    /// whole texel, for pixel art and for looking closely. Above zero is the
    /// device pixel ratio: reads stay filtered until a texel is drawn more
    /// than four device pixels wide, and snap to whole texels past that, where
    /// filtering reads as blur rather than smoothness.
    #[live]
    pub sample_mode: f32,
    /// When non-zero, `get_color` rotates the image rigidly (aspect-correct):
    /// the image of this pixel size is rotated by `rotation` and inscribed in the
    /// quad, instead of rotating texture UVs in normalized space (which squishes
    /// non-square images). The image viewer drives these per frame.
    #[live]
    pub image_dim_w: f32,
    #[live]
    pub image_dim_h: f32,
    // The picture's own shape. A template that overrides `draw_bg` has two
    // things to know about these four: write them as plain values, because an
    // `instance(...)` declaration of a name a field already owns is dropped on
    // the way in and the field keeps its own default; and they are the bitmap
    // path's, because an `Image` handed an SVG draws through `draw_svg` and
    // never reaches this shader at all.
    /// Corner radius of the picture, in the number `RoundedView` takes for its
    /// own `border_radius`, handed to the same box function. A view floors its
    /// own radius at one and this does not, so the two round alike at every
    /// radius a corner is visible at. Zero with no stroke is the plain quad,
    /// returned before any shape work.
    #[live]
    pub border_radius: f32,
    /// Width of the stroke on the picture's edge, placed where `RoundedView`
    /// places its own: centred on the edge, so half the band lies over the
    /// picture and half outside it. Zero is none.
    #[live]
    pub border_size: f32,
    #[live]
    pub border_color: Vec4f,
    /// What shows where the picture does not reach: the bars of a picture
    /// dialled towards contain, and the corners around a rotated one. Clear
    /// by default, so the ground behind shows through.
    #[live]
    pub letterbox_color: Vec4f,
    // The slice state below is what `Image` resolved for the shader on its
    // last draw, not a setting: anything written here under `draw_bg +:` is
    // overwritten on the next draw, and a `uniform(..)` or `instance(..)`
    // declaration of one of these names in a `draw_bg +:` merge is dropped
    // on the way in. Slicing is set on the widget: `fit: ImageFit.Slice`,
    // `slice`, `slice_edge`, `slice_center`, `slice_scale`, `slice_units`.
    /// Written by `Image` on every draw; not an input. The border widths in
    /// texels of the window: left, top, right, bottom.
    #[live]
    pub slice_inset: Vec4f,
    /// Written by `Image` on every draw; not an input. Layout points one
    /// border texel is drawn at before the shader shrinks the borders to fit.
    #[live]
    pub slice_texel_points: f32,
    /// Written by `Image` on every draw; not an input. x: 0 not sliced,
    /// 1 edges stretch, 2 tile, 3 round. y: 0 middle hidden, 1 stretch,
    /// 2 tile, 3 tile at the edges' rounded spacing.
    #[live]
    pub slice_mode: Vec2f,
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
    placeholder_width: u64,
    #[live]
    placeholder_height: u64,
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
    /// How much of the overflow a `CropToFill` picture crops away: one covers
    /// the box and crops what will not fit, zero puts the whole picture inside
    /// it and leaves `letterbox_color` over the ends, and between the two it is
    /// between — the picture reads large without losing its middle. One is the
    /// crop this fit has always done, so it is the default; the other fits read
    /// nothing here, because they change the rect instead of the picture in it.
    #[live(1.0)]
    pub crop: f64,
    /// Texels of the window that belong to each border when `fit` is `Slice`.
    /// The texture's own pixels, because that is where the cut is: the same
    /// corner at every size the picture is drawn.
    #[live]
    pub slice: Inset,
    /// How a sliced picture's four edges fill their length.
    #[live]
    pub slice_edge: ImageSliceEdge,
    /// How a sliced picture's middle fills.
    #[live]
    pub slice_center: ImageSliceCenter,
    /// Size one border texel is drawn at, in `slice_units`.
    #[live(1.0)]
    pub slice_scale: f64,
    /// Whether `slice_scale` counts layout points or device pixels.
    #[live]
    pub slice_units: ImageSliceUnits,
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
    /// The async-load key that produced `texture`, when it came from a completed
    /// async decode. `None` for textures installed explicitly via `set_texture`
    /// (the current occupant's own content, e.g. a blurhash placeholder), which
    /// must survive the start of an async load; only a texture left behind by an
    /// async load for a different key is stale and gets cleared.
    #[rust]
    texture_async_source: Option<PathBuf>,
    /// `Some` only while showing an SVG (and `texture` is then `None`); lazily
    /// allocated, so non-SVG images pay just a pointer, not a whole `DrawSvg`.
    #[rust]
    draw_svg: Option<Box<DrawSvg>>,
    /// The SVG source currently loaded into `draw_svg`, when the caller supplied it
    /// as shared bytes (see [`Image::load_svg_from_shared_data`]). Holding a share of
    /// the caller's bytes costs a pointer, not a copy, and keeps them alive so their
    /// address stays a valid identity to compare against. `Some` only while `draw_svg`
    /// is, and only for loads that came through the shared-bytes entry point.
    #[rust]
    svg_source: Option<Arc<[u8]>>,
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
        self.svg_source = None;
        // The texture is now this widget's content: drop any pending async-load
        // state so the draw path binds it instead of the loading placeholder and
        // a stale decode result for an older key can no longer replace it. It was
        // installed explicitly, so it carries no async source.
        self.async_image_size = None;
        self.async_image_path = None;
        self.texture_async_source = None;
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
    /// Updates layout and aspect fitting without evaluating script. This is
    /// useful for widgets owned by an isolated VM: their typed Rust state can
    /// be changed safely even while the host VM is active.
    pub fn set_walk_and_fit(&mut self, cx: &mut Cx, walk: Walk, fit: ImageFit) {
        self.walk = walk;
        self.fit = fit;
        self.redraw(cx);
    }

    pub fn fit(&self) -> ImageFit {
        self.fit
    }

    pub fn set_fit(&mut self, cx: &mut Cx, fit: ImageFit) {
        self.fit = fit;
        self.redraw(cx);
    }

    /// The border texels a sliced picture keeps, as the `slice` field says.
    pub fn set_slice(&mut self, cx: &mut Cx, slice: Inset) {
        self.slice = slice;
        self.redraw(cx);
    }

    pub fn set_slice_modes(&mut self, cx: &mut Cx, edge: ImageSliceEdge, center: ImageSliceCenter) {
        self.slice_edge = edge;
        self.slice_center = center;
        self.redraw(cx);
    }

    pub fn set_slice_scale(&mut self, cx: &mut Cx, scale: f64, units: ImageSliceUnits) {
        self.slice_scale = scale;
        self.slice_units = units;
        self.redraw(cx);
    }

    pub fn slice(&self) -> Inset {
        self.slice
    }

    pub fn slice_modes(&self) -> (ImageSliceEdge, ImageSliceCenter) {
        (self.slice_edge, self.slice_center)
    }

    pub fn slice_scale(&self) -> (f64, ImageSliceUnits) {
        (self.slice_scale, self.slice_units)
    }

    fn load_from_resource(&mut self, cx: &mut Cx) {
        if self.src_loaded {
            return;
        }
        let Some(ref handle_ref) = self.src else {
            self.src_loaded = true;
            return;
        };
        let handle = handle_ref.as_handle();
        let heap_key = handle_ref.heap_key();
        let data = if let Some(data) = cx.get_resource(heap_key, handle) {
            data
        } else {
            cx.load_script_resource(heap_key, handle);
            match cx.get_resource(heap_key, handle) {
                Some(data) => data,
                None => {
                    let resources = cx.script_data.resources.resources.borrow();
                    if let Some(res) = resources.iter().find(|r| r.has_handle(heap_key, handle)) {
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
                .find(|r| r.has_handle(heap_key, handle))
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
                    // Only apply a completed decode for the load this widget is still
                    // waiting on; results for other keys belong to a previous occupant
                    // of this (possibly recycled) widget and must be ignored.
                    if self.async_image_size.is_some()
                        && self.async_image_path.as_deref() == Some(image_path.as_path())
                    {
                        // see if we can load from cache
                        self.load_image_from_cache(cx, image_path, 0);
                        self.async_image_size = None;
                        self.async_image_path = None;
                        // Record which async load produced the texture, so a later
                        // load for a different key knows it is stale.
                        self.texture_async_source = Some(image_path.to_path_buf());
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
                    .unwrap_or((self.placeholder_width as usize, self.placeholder_height as usize));
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
        // The window on the texture, as a share of it. Covering takes the
        // smaller window — less of the texture, drawn bigger, the rest cropped
        // off; containing takes the larger one — the whole texture with room
        // to spare, and the room over is the bar. The two are one expression
        // apart, so the dial between them is a mix of the pair; and because
        // the window narrows on one axis exactly as fast as it widens on the
        // other, the mix is the picture's own shape at every setting and not
        // a squash somewhere in the middle.
        let ratio = source_aspect / target_aspect;
        // A dial past its ends is held at them, and a dial that is not a
        // number at all is the crop this fit has always done. `clamp` hands
        // NaN straight back, and a NaN window compares false against every
        // pixel of the quad: not a bar and not a picture, just a sample taken
        // nowhere.
        let crop = if self.crop.is_nan() {
            1.0
        } else {
            self.crop.clamp(0.0, 1.0)
        };
        let axis = |contain: f64, cover: f64| contain + (cover - contain) * crop;
        let crop_scale = vec2(
            axis((1.0 / ratio).max(1.0), (1.0 / ratio).min(1.0)) as f32,
            axis(ratio.max(1.0), ratio.min(1.0)) as f32,
        );

        // Centred either way: half the crop comes off each side, half the room
        // over goes to each end.
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

    /// True if this `Image` currently has displayable content:
    /// either a raster texture or a loaded SVG.
    ///
    /// This distinguishes "the load was accepted" from "there is actually
    /// something to draw". Note that a pending async load does not imply this
    /// is false: content deliberately kept visible while the load decodes
    /// (a `set_texture` placeholder, or a previous load of the same key)
    /// still counts as content.
    pub fn has_content(&self) -> bool {
        self.texture.is_some() || self.draw_svg.is_some()
    }

    /// Loads an SVG into this `Image` by parsing the UTF-8 SVG `data` and drawing
    /// it with makepad's native vector engine instead of a raster texture.
    ///
    /// The `DrawSvg` is allocated on first use, so images that never show an SVG
    /// carry only a null pointer.
    pub fn load_svg_from_data(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        self.parse_and_show_svg(cx, data)
    }

    /// Like [`Image::load_svg_from_data`], but for source the caller holds in shared
    /// bytes, which lets re-loading the very same source be recognized and skipped.
    ///
    /// Prefer this wherever the load is re-issued on every draw (list items are
    /// repopulated per frame): unlike a raster load, this path is synchronous with no
    /// cache behind it, so without the check it re-parses the source and re-builds its
    /// geometry every single frame. The cached raster path gets the same treatment in
    /// `finish_async_load`.
    pub fn load_svg_from_shared_data(
        &mut self,
        cx: &mut Cx,
        data: Arc<[u8]>,
    ) -> Result<(), ImageError> {
        // Identity of the shared bytes is the check: the same allocation is the same
        // drawing, and holding a share of it keeps the address meaningful (nothing
        // else can be freed into it). This costs one pointer comparison, and holding
        // the source costs a refcount rather than a copy of it.
        if self.draw_svg.is_some()
            && self
                .svg_source
                .as_ref()
                .is_some_and(|shown| Arc::ptr_eq(shown, &data))
        {
            return Ok(());
        }
        self.parse_and_show_svg(cx, &data)?;
        self.svg_source = Some(data);
        Ok(())
    }

    fn parse_and_show_svg(&mut self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if data.len() > MAX_SVG_BYTES {
            return Err(ImageError::DataTooLarge {
                bytes: data.len(),
                limit: MAX_SVG_BYTES,
            });
        }
        let svg_str = std::str::from_utf8(data).map_err(|_| ImageError::UnsupportedFormat)?;
        if self.draw_svg.is_none() {
            self.draw_svg = Some(cx.with_vm(|vm| Box::new(DrawSvg::script_new_with_default(vm))));
        }
        if let Some(draw_svg) = self.draw_svg.as_mut() {
            draw_svg.load_from_str(svg_str);
        }
        // Only a caller that shared its bytes leaves an identity behind to skip on;
        // `load_svg_from_shared_data` records it once this has succeeded.
        self.svg_source = None;
        self.texture = None;
        self.texture_async_source = None;
        // The SVG is now this widget's content: a pending raster load no longer
        // applies, and its decode result must not replace the SVG when it lands.
        self.async_image_size = None;
        self.async_image_path = None;
        self.redraw(cx);
        Ok(())
    }

    pub fn draw_walk_image(&mut self, cx: &mut Cx2d, mut walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
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
        // The bound texture's own size in texels, which a sliced picture
        // measures its window in. The loading branch's size is the pending
        // decode's, not the texture it keeps showing meanwhile.
        let mut texture_texels: Option<Vec2d> = None;

        let (width, height) = if let Some((w, h)) = &self.async_image_size {
            // Still loading. Any texture present here is legitimate current content
            // (the occupant's own placeholder, or a previous load of this same
            // source; begin_async_load already cleared stale ones), so keep showing
            // it. Otherwise bind the empty texture, never whatever a previous
            // occupant left in the draw vars.
            if let Some(image_texture) = &self.texture {
                self.draw_bg.draw_vars.set_texture(0, image_texture);
                texture_texels = texels_of(image_texture, cx);
            } else {
                self.draw_bg.draw_vars.empty_texture(0);
            }
            (*w as f64, *h as f64)
        } else if let Some(image_texture) = &self.texture {
            self.draw_bg.draw_vars.set_texture(0, image_texture);
            texture_texels = texels_of(image_texture, cx);
            let (width, height) = image_texture
                .get_format(cx)
                .vec_width_height()
                // A FIXED-size render target (e.g. a video convert pass)
                // has real dimensions too — without this it sized as
                // min_* (usually zero) and the picture silently vanished.
                .or_else(|| image_texture.get_format(cx).render_fixed_width_height())
                .unwrap_or((self.placeholder_width as usize, self.placeholder_height as usize));
            if let Some(animation) = image_texture.animation(cx) {
                let (w, h) = (animation.width as f64, animation.height as f64);
                self.next_frame = cx.new_next_frame();
                // we have an animation. lets compute the scale and zoom for a certain frame
                let scale_x = w as f32 / width as f32;
                let scale_y = h as f32 / height as f32;
                self.draw_bg.image_scale = vec2(scale_x, scale_y);
                (w, h)
            } else if image_texture.get_format(cx).is_render() {
                // Render targets are stored top-left on EVERY backend now
                // (GL renders offscreen through a Y-inverted projection),
                // so they sample exactly like any other texture — the old
                // unconditional flip here showed them upside down.
                (width as f64 * self.width_scale, height as f64)
            } else {
                (width as f64 * self.width_scale, height as f64)
            }
        } else {
            self.draw_bg.draw_vars.empty_texture(0);
            (
                self.placeholder_width as f64 / dpi,
                self.placeholder_height as f64 / dpi,
            )
        };

        // Slicing is resolved before the fit arm because a Fit axis of a sliced
        // picture is its natural size, which needs the points per texel. The
        // window is the part of the texture being drawn, so an animated cell
        // or a sprite-sheet window is sliced inside itself. A rotated picture
        // is the viewer's own mapping and is left to it.
        self.draw_bg.slice_mode = vec2(0.0, 0.0);
        let points_per_texel = slice_points_per_texel(self.slice_scale, self.slice_units, dpi);
        let slice_window = texture_texels.map(|t| {
            dvec2(
                t.x * self.draw_bg.image_scale.x as f64,
                t.y * self.draw_bg.image_scale.y as f64,
            )
        });
        if let (ImageFit::Slice, Some(window)) = (self.fit, slice_window) {
            if self.draw_bg.image_dim_w <= 0.0 {
                let [left, top, right, bottom] = slice_texels(self.slice, window);
                self.draw_bg.slice_inset = vec4(left as f32, top as f32, right as f32, bottom as f32);
                self.draw_bg.slice_texel_points = points_per_texel as f32;
                self.draw_bg.slice_mode = slice_modes(self.slice_edge, self.slice_center);
            }
        }
        let slice_natural = slice_window
            .map(|window| window * points_per_texel)
            .unwrap_or(dvec2(width, height));

        let aspect = width / height;
        // A Fit height peeks as NaN, so use its effective content-box max
        // (including a Walk-level max) while preserving intrinsic aspect.
        let height_cap = cx.walk_max_height(walk).unwrap_or(f64::INFINITY);
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
            // The box it is given, but an axis left to `Fit` is the picture at
            // its natural size, as `Size` would draw it, corners included.
            ImageFit::Slice => {
                if walk.width.is_fit() {
                    walk.width = Size::Fixed(slice_natural.x);
                }
                if walk.height.is_fit() {
                    walk.height = Size::Fixed(slice_natural.y);
                }
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
        match self.load_image_file_by_path_async_impl(cx, image_path, 0) {
            Ok(AsyncLoadResult::Loading(w, h)) => {
                self.begin_async_load(cx, image_path, (w, h));
            }
            Ok(AsyncLoadResult::Loaded) => {
                self.finish_async_load(cx, image_path);
            }
            Err(_) => {
                self.cancel_async_load(cx);
            }
        }
        Ok(())
    }

    pub fn load_image_from_data_async<D>(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<D>,
    ) -> Result<(), ImageError>
    where
        D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
    {
        self.lazy_create_image_cache(cx);
        match self.load_image_from_data_async_impl(cx, image_path, data, 0) {
            Ok(AsyncLoadResult::Loading(w, h)) => {
                self.begin_async_load(cx, image_path, (w, h));
            }
            Ok(AsyncLoadResult::Loaded) => {
                self.finish_async_load(cx, image_path);
            }
            Err(_) => {
                self.cancel_async_load(cx);
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
        match self.load_image_http_by_url_async_impl(cx, url, 0) {
            Ok(AsyncLoadResult::Loading(w, h)) => {
                self.begin_async_load(cx, Path::new(url), (w, h));
            }
            Ok(AsyncLoadResult::Loaded) => {
                self.finish_async_load(cx, Path::new(url));
            }
            Err(_) => {
                self.cancel_async_load(cx);
            }
        }
        Ok(())
    }

    /// Records `image_path` as this widget's one pending async load, replacing any
    /// previous request so a decode finishing for an older key is never applied.
    fn begin_async_load(&mut self, cx: &mut Cx, image_path: &Path, size: (usize, usize)) {
        // A texture left behind by an async load for a different key belongs to a
        // previous occupant of this (possibly recycled) widget and must not stay
        // visible while the new source decodes. A texture installed via
        // `set_texture` is the current occupant's own content (e.g. a blurhash
        // placeholder) and stays visible until the decode lands, as does the
        // result of a previous load of this same key.
        let texture_is_stale = self
            .texture_async_source
            .as_deref()
            .is_some_and(|source| source != image_path);
        if texture_is_stale {
            self.texture = None;
            self.texture_async_source = None;
        }
        // An SVG is always different content from an incoming raster load.
        self.draw_svg = None;
        self.svg_source = None;
        self.async_image_size = Some(size);
        self.async_image_path = Some(image_path.into());
        self.animator_play(cx, ids!(async_load.on));
        self.redraw(cx);
    }

    /// The requested image was already cached and its texture has been set: clear
    /// the pending-load state so the draw path binds the texture directly.
    fn finish_async_load(&mut self, cx: &mut Cx, image_path: &Path) {
        // Re-loading the image that is already bound changes nothing; skip the
        // animator and redraw so widgets that re-issue loads on every draw
        // (e.g. list items repopulated per frame) don't dirty themselves into
        // an endless redraw loop.
        if self.async_image_path.is_none()
            && self.texture_async_source.as_deref() == Some(image_path)
        {
            return;
        }
        self.async_image_size = None;
        self.async_image_path = None;
        // Record which load produced the texture, just like the decode-completion
        // path does. Without this, a cache-hit texture has no provenance and a
        // later load for a different key would wrongly keep it visible on a
        // recycled widget while the new source decodes.
        self.texture_async_source = Some(image_path.to_path_buf());
        self.animator_play(cx, ids!(async_load.off));
        self.redraw(cx);
    }

    /// A failed load leaves the widget's intended content unknown: clear both the
    /// pending request (so a decode finishing for a previous key is never applied)
    /// and any displayed content, which may belong to a previous occupant of a
    /// recycled widget. Blank is strictly safer than someone else's image.
    fn cancel_async_load(&mut self, cx: &mut Cx) {
        self.async_image_size = None;
        self.async_image_path = None;
        self.texture = None;
        self.texture_async_source = None;
        self.draw_svg = None;
        self.svg_source = None;
        self.animator_play(cx, ids!(async_load.off));
        self.redraw(cx);
    }
}

/// A texture's size in texels, where it has one: a vector texture, or a
/// render target of a fixed size. The same read the draw path sizes the
/// picture by, without its fallback to the placeholder size, which is not
/// a texture a slice could be measured in.
fn texels_of(texture: &Texture, cx: &mut Cx) -> Option<Vec2d> {
    let format = texture.get_format(cx);
    format
        .vec_width_height()
        .or_else(|| format.render_fixed_width_height())
        .map(|(w, h)| dvec2(w as f64, h as f64))
}

pub enum AsyncLoad {
    Yes,
    No,
}

impl ImageRef {
    /// See [`Image::set_walk_and_fit`].
    pub fn set_walk_and_fit(&self, cx: &mut Cx, walk: Walk, fit: ImageFit) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_walk_and_fit(cx, walk, fit);
        }
    }

    /// See [`Image::fit()`]. `Stretch`, the default fit, when empty.
    pub fn fit(&self) -> ImageFit {
        self.borrow().map(|inner| inner.fit()).unwrap_or(ImageFit::Stretch)
    }

    /// See [`Image::set_fit`].
    pub fn set_fit(&self, cx: &mut Cx, fit: ImageFit) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_fit(cx, fit);
        }
    }

    /// See [`Image::set_slice`].
    pub fn set_slice(&self, cx: &mut Cx, slice: Inset) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_slice(cx, slice);
        }
    }

    /// See [`Image::set_slice_modes`].
    pub fn set_slice_modes(&self, cx: &mut Cx, edge: ImageSliceEdge, center: ImageSliceCenter) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_slice_modes(cx, edge, center);
        }
    }

    /// See [`Image::set_slice_scale`].
    pub fn set_slice_scale(&self, cx: &mut Cx, scale: f64, units: ImageSliceUnits) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_slice_scale(cx, scale, units);
        }
    }

    /// See [`Image::slice()`]. No inset when empty.
    pub fn slice(&self) -> Inset {
        self.borrow().map(|inner| inner.slice()).unwrap_or_default()
    }

    /// See [`Image::slice_modes()`]. The defaults when empty.
    pub fn slice_modes(&self) -> (ImageSliceEdge, ImageSliceCenter) {
        self.borrow().map(|inner| inner.slice_modes()).unwrap_or_default()
    }

    /// See [`Image::slice_scale()`]. The defaults when empty.
    pub fn slice_scale(&self) -> (f64, ImageSliceUnits) {
        self.borrow()
            .map(|inner| inner.slice_scale())
            .unwrap_or((1.0, ImageSliceUnits::Points))
    }

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
    pub fn load_image_from_data_async<D>(
        &self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<D>,
    ) -> Result<(), ImageError>
    where
        D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
    {
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

    /// See [`Image::load_svg_from_shared_data`]: the same, but re-loading the very
    /// same source is recognized and skipped, so a caller that re-issues its load on
    /// every draw doesn't re-parse the SVG every frame.
    pub fn load_svg_from_shared_data(
        &self,
        cx: &mut Cx,
        data: Arc<[u8]>,
    ) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_svg_from_shared_data(cx, data)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }

    pub fn set_texture(&self, cx: &mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            // Route through the trait impl so the content invariants (draw_svg,
            // async-load state, texture provenance) live in one place.
            ImageCacheImpl::set_texture(&mut *inner, texture, 0);
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

    /// See [`Image::has_content()`].
    pub fn has_content(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.has_content()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod flattened_walk_collision_tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;

    #[test]
    fn image_exposes_walk_bounds_and_distinct_placeholder_dimensions() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            Image::script_proto(vm);
            let props = &vm
                .bx
                .heap
                .registered_type(Image::script_type_id_static())
                .unwrap()
                .props
                .props;
            for field in [
                live_id!(min_width),
                live_id!(max_width),
                live_id!(min_height),
                live_id!(max_height),
                live_id!(aspect),
                live_id!(placeholder_width),
                live_id!(placeholder_height),
            ] {
                assert!(props.contains_key(&field), "missing flattened/reflected field {field:?}");
            }
        });
        });
    }
}

#[cfg(test)]
mod framing_tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    /// Every number worked out here is a half, a quarter or an eighth, but
    /// the aspect it starts from is two lengths divided by the same dpi, so
    /// the comparison is given the slack of that one division.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// A picture twice as wide as it is tall, with nothing loaded into it:
    /// a placeholder size is a size, and the framing is worked out from the
    /// aspect either way.
    fn wide_picture_in_a_square(cx: &mut Cx) -> Image {
        cx.with_vm(|vm| {
            let source = script! {
                use mod.prelude.widgets.*
                Image{
                    width: Fill height: Fill
                    fit: ImageFit.CropToFill
                    placeholder_width: 200 placeholder_height: 100
                }
            };
            let value = vm.eval(source);
            Image::script_from_value(vm, value)
        })
    }

    /// One pass over a square box. The framing is written during the draw,
    /// so reading it back has to go through the widget's own draw entry
    /// rather than the arithmetic behind it.
    fn draw_once(cx: &mut Cx, image: &mut Image) -> (Vec2f, Vec2f) {
        let size = dvec2(100.0, 100.0);
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        let mut draw_list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        assert!(image
            .draw_walk(&mut cx2d, &mut Scope::empty(), image.walk)
            .is_done());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
        drop(cx2d);
        (image.draw_bg.fit_scale, image.draw_bg.fit_pan)
    }

    /// The dial's two ends and its middle. At one it is the crop this fit
    /// has always done — a window on half the texture, centred, the sides
    /// cropped off. At zero the window is bigger than the texture, which is
    /// what leaves room over for a bar. Halfway is halfway: bigger than
    /// contain, smaller than cover, still centred.
    #[test]
    fn crop_dials_between_the_whole_picture_and_a_covering_one() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        for (crop, scale, pan) in [
            (1.0, (0.5, 1.0), (0.25, 0.0)),
            (0.5, (0.75, 1.5), (0.125, -0.25)),
            (0.0, (1.0, 2.0), (0.0, -0.5)),
        ] {
            image.crop = crop;
            let (fit_scale, fit_pan) = draw_once(&mut cx, &mut image);
            assert!(
                close(fit_scale.x, scale.0) && close(fit_scale.y, scale.1),
                "crop {crop}: window {fit_scale:?} is not {scale:?}"
            );
            assert!(
                close(fit_pan.x, pan.0) && close(fit_pan.y, pan.1),
                "crop {crop}: offset {fit_pan:?} is not {pan:?}"
            );
        }
        });
    }

    /// A picture between contain and cover is drawn at a size between the
    /// two, and it has to be the picture's own shape at every one of them —
    /// a dial that squashed in the middle would be a dial nobody could use.
    /// The window narrows on one axis exactly as fast as it widens on the
    /// other, which is what keeps the ratio of the two fixed all the way
    /// along: here the box is square and the picture twice as wide, so the
    /// window is always half as wide as it is tall.
    #[test]
    fn the_dial_never_squashes_the_picture() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        for crop in [0.0, 0.25, 0.5, 0.75, 1.0] {
            image.crop = crop;
            let (scale, _) = draw_once(&mut cx, &mut image);
            assert!(
                close(scale.x / scale.y, 0.5),
                "crop {crop}: the window is {scale:?}, which is not the picture's shape"
            );
        }
        });
    }

    /// The bar is drawn where the framing left the picture behind, so a
    /// framing that reaches past the texture is the whole of what turns it
    /// on. Out of range is not a second meaning: a dial past its ends is
    /// held at them, and one that is not a number at all is the crop this
    /// fit has always done.
    #[test]
    fn only_a_dialled_back_crop_asks_for_a_bar() {
        crate::on_test_cx(|| {
        fn frames_the_whole_box(scale: Vec2f, pan: Vec2f) -> bool {
            pan.x >= 0.0 && pan.y >= 0.0 && pan.x + scale.x <= 1.0 && pan.y + scale.y <= 1.0
        }
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        for crop in [1.0, 2.0, f64::INFINITY, f64::NAN] {
            image.crop = crop;
            let (scale, pan) = draw_once(&mut cx, &mut image);
            assert!(frames_the_whole_box(scale, pan), "crop {crop} left a bar");
        }
        for crop in [0.0, 0.99, -1.0] {
            image.crop = crop;
            let (scale, pan) = draw_once(&mut cx, &mut image);
            assert!(!frames_the_whole_box(scale, pan), "crop {crop} left no bar");
        }
        });
    }

    /// Every other fit resizes the rect and hands the picture the whole of
    /// the texture, and the dial is not theirs to read. That is what keeps
    /// the bar off every picture in the library that never asked for one.
    #[test]
    fn the_other_fits_frame_nothing_whatever_the_dial_says() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        for fit in [
            ImageFit::Size,
            ImageFit::Stretch,
            ImageFit::Horizontal,
            ImageFit::Vertical,
            ImageFit::Smallest,
            ImageFit::Biggest,
            ImageFit::Slice,
        ] {
            image.fit = fit;
            image.crop = 0.0;
            let (scale, pan) = draw_once(&mut cx, &mut image);
            assert!(
                close(scale.x, 1.0) && close(scale.y, 1.0) && close(pan.x, 0.0) && close(pan.y, 0.0),
                "{fit:?} framed the picture: window {scale:?} at {pan:?}"
            );
        }
        });
    }

    /// What a caller gets by writing `Image{}`: no radius, no stroke, a
    /// clear bar colour and a filtered read, which together are the plain
    /// quad this widget has always drawn.
    #[test]
    fn an_unasked_image_is_the_plain_quad() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let image = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
            Image::script_new_with_default(vm)
        });
        assert_eq!(makepad_platform::shader_error::take(), None, "the image shader failed to compile");
        assert_eq!(image.draw_bg.border_radius, 0.0);
        assert_eq!(image.draw_bg.border_size, 0.0);
        assert_eq!(image.draw_bg.letterbox_color.w, 0.0);
        assert_eq!(image.draw_bg.sample_mode, 0.0);
        assert_eq!(image.crop, 1.0, "the crop it has always done is the one it still does");
        assert_eq!(image.draw_bg.slice_mode, vec2(0.0, 0.0), "a picture nobody sliced asks the shader to slice");
        let slice = image.slice();
        assert!(
            slice.left == 0.0 && slice.top == 0.0 && slice.right == 0.0 && slice.bottom == 0.0,
            "a picture nobody sliced carries an inset: {slice:?}"
        );
        assert_eq!(image.slice_modes(), (ImageSliceEdge::Stretch, ImageSliceCenter::Stretch));
        assert_eq!(image.slice_scale(), (1.0, ImageSliceUnits::Points));
        });
    }

    /// The rounding, the stroke and the bar are read off the draw struct by
    /// the shader, so a name that does not reach a field of it is a setting
    /// that quietly does nothing — the DSL would take it either way and
    /// declare a prop of its own. Building the shader with all four set is
    /// also what turns a mistake in the pixel function into a failed test.
    #[test]
    fn a_rounded_and_stroked_picture_carries_what_the_dsl_wrote() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let image = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
            let source = script! {
                use mod.prelude.widgets.*
                Image{
                    width: Fill height: Fill
                    fit: ImageFit.CropToFill
                    crop: 0.4
                    slice: Inset{left: 4 top: 5 right: 6 bottom: 7}
                    slice_edge: ImageSliceEdge.Tile
                    slice_center: ImageSliceCenter.Hidden
                    slice_scale: 2.0
                    slice_units: ImageSliceUnits.DevicePixels
                    draw_bg +: {
                        border_radius: 8.0
                        border_size: 1.5
                        border_color: #f00
                        letterbox_color: #000
                        sample_mode: -1.0
                    }
                }
            };
            let value = vm.eval(source);
            Image::script_from_value(vm, value)
        });
        assert_eq!(makepad_platform::shader_error::take(), None, "the image shader failed to compile");
        assert_eq!(image.draw_bg.border_radius, 8.0);
        assert_eq!(image.draw_bg.border_size, 1.5);
        assert_eq!(image.draw_bg.border_color.x, 1.0);
        assert_eq!(image.draw_bg.letterbox_color.w, 1.0);
        assert_eq!(image.draw_bg.sample_mode, -1.0);
        assert_eq!(image.crop, 0.4);
        let slice = image.slice();
        assert_eq!((slice.left, slice.top, slice.right, slice.bottom), (4.0, 5.0, 6.0, 7.0));
        assert_eq!(image.slice_modes(), (ImageSliceEdge::Tile, ImageSliceCenter::Hidden));
        assert_eq!(image.slice_scale(), (2.0, ImageSliceUnits::DevicePixels));
        });
    }

    /// A dial that is not a number is the crop this fit has always done, and
    /// not a window of NaNs: every comparison against one of those is false,
    /// so nothing is a bar, nothing is inside the picture, and the read is
    /// taken nowhere at all.
    #[test]
    fn a_dial_that_is_not_a_number_is_the_crop_it_always_did() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        image.crop = f64::NAN;
        let (scale, pan) = draw_once(&mut cx, &mut image);
        assert!(
            close(scale.x, 0.5) && close(scale.y, 1.0) && close(pan.x, 0.25) && close(pan.y, 0.0),
            "a dial that is not a number framed {scale:?} at {pan:?}"
        );
        });
    }

    /// The dial, the rounding, the stroke and the bar are all the bitmap
    /// path's. A picture handed a vector source draws through the vector
    /// call and never reaches this shader, so the dial leaves no mark on it
    /// — and that is the one thing the markup accepts without a word, since
    /// the settings are perfectly good names either way.
    #[test]
    fn a_vector_source_is_drawn_by_the_vector_call() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = wide_picture_in_a_square(&mut cx);
        // A dial a bitmap of this shape would frame at (1, 2) offset (0, -0.5).
        image.crop = 0.0;
        image
            .load_svg_from_data(
                &mut cx,
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="red"/></svg>"#,
            )
            .expect("the vector source loads");
        let (scale, pan) = draw_once(&mut cx, &mut image);
        assert!(
            close(scale.x, 1.0) && close(scale.y, 1.0) && close(pan.x, 0.0) && close(pan.y, 0.0),
            "a vector source reached the bitmap framing: window {scale:?} at {pan:?}"
        );
        });
    }

    /// A plain texture of the given size, installed the way a caller installs
    /// one. Slicing reads only its size; the draw binds it.
    fn install_texture(cx: &mut Cx, image: &mut Image, width: usize, height: usize) {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width,
                height,
                data: Some(vec![0xff80_8080; width * height]),
                updated: TextureUpdated::Full,
            },
        );
        ImageCacheImpl::set_texture(image, Some(texture), 0);
    }

    /// The catalogue's panel: sixteen texels of border, rounded edges and a
    /// tiled middle, filling the box it is drawn in.
    fn sliced_panel(cx: &mut Cx) -> Image {
        cx.with_vm(|vm| {
            let source = script! {
                use mod.prelude.widgets.*
                Image{
                    width: Fill height: Fill
                    fit: ImageFit.Slice
                    slice: 16
                    slice_edge: ImageSliceEdge.Round
                    slice_center: ImageSliceCenter.Tile
                }
            };
            let value = vm.eval(source);
            Image::script_from_value(vm, value)
        })
    }

    fn at_rest(scale: Vec2f, pan: Vec2f) -> bool {
        close(scale.x, 1.0) && close(scale.y, 1.0) && close(pan.x, 0.0) && close(pan.y, 0.0)
    }

    /// A sliced picture takes the box it is given and is never framed: the
    /// shader's slice branch returns before the framing test, so framing left
    /// anywhere but at rest would be a bar nobody could see the cause of.
    /// What reaches the shader is the inset in texels, the points one of
    /// them is drawn at, and the two modes the markup asked for.
    #[test]
    fn a_sliced_picture_keeps_its_box_and_its_framing_at_rest() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = sliced_panel(&mut cx);
        install_texture(&mut cx, &mut image, 64, 64);
        let (scale, pan) = draw_once(&mut cx, &mut image);
        assert!(at_rest(scale, pan), "a sliced picture was framed: window {scale:?} at {pan:?}");
        assert_eq!(image.draw_bg.slice_inset, vec4(16.0, 16.0, 16.0, 16.0));
        assert_eq!(image.draw_bg.slice_texel_points, 1.0);
        assert_eq!(image.draw_bg.slice_mode, vec2(3.0, 3.0), "a tiled middle between rounded edges takes their spacing");
        assert_eq!(image.draw_bg.rect_size, vec2(100.0, 100.0), "a sliced picture did not take its box");
        });
    }

    /// The shader slices whenever `slice_mode.x` is above zero, so every
    /// draw under any other fit has to put it back, or a picture switched
    /// from slicing to another fit would keep drawing sliced.
    #[test]
    fn only_a_sliced_fit_asks_the_shader_to_slice() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = sliced_panel(&mut cx);
        install_texture(&mut cx, &mut image, 64, 64);
        draw_once(&mut cx, &mut image);
        assert!(image.draw_bg.slice_mode.x > 0.0, "the sliced picture was not sliced");
        for fit in [
            ImageFit::Size,
            ImageFit::Stretch,
            ImageFit::Horizontal,
            ImageFit::Vertical,
            ImageFit::Smallest,
            ImageFit::Biggest,
            ImageFit::CropToFill,
        ] {
            image.fit = fit;
            draw_once(&mut cx, &mut image);
            assert_eq!(image.draw_bg.slice_mode.x, 0.0, "{fit:?} asked the shader to slice");
        }
        });
    }

    /// With nothing bound there is no texture to measure an inset in, and a
    /// rotated picture is the viewer's own mapping, which slicing would
    /// replace wholesale. Both draw the path they drew before.
    #[test]
    fn an_empty_or_rotated_picture_is_not_sliced() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = sliced_panel(&mut cx);
        draw_once(&mut cx, &mut image);
        assert_eq!(image.draw_bg.slice_mode.x, 0.0, "a picture with nothing loaded was sliced");
        install_texture(&mut cx, &mut image, 64, 64);
        image.draw_bg.image_dim_w = 10.0;
        image.draw_bg.image_dim_h = 10.0;
        draw_once(&mut cx, &mut image);
        assert_eq!(image.draw_bg.slice_mode.x, 0.0, "a rotated picture was sliced");
        image.draw_bg.image_dim_w = 0.0;
        draw_once(&mut cx, &mut image);
        assert!(image.draw_bg.slice_mode.x > 0.0, "the same picture unrotated was not sliced");
        });
    }

    /// `Fit` on a sliced picture is the size `ImageFit::Size` would give it,
    /// scaled by what one border texel is drawn at, so a panel that has not
    /// been given a size is the texture as drawn, corners and all.
    #[test]
    fn a_fit_axis_is_the_sliced_picture_at_its_natural_size() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let source = script! {
                use mod.prelude.widgets.*
                Image{
                    width: Fit height: Fit
                    fit: ImageFit.Slice
                    slice: 16
                    slice_scale: 2.0
                }
            };
            let value = vm.eval(source);
            Image::script_from_value(vm, value)
        });
        install_texture(&mut cx, &mut image, 64, 48);
        draw_once(&mut cx, &mut image);
        assert_eq!(image.draw_bg.rect_size, vec2(128.0, 96.0));
        assert_eq!(image.draw_bg.slice_texel_points, 2.0);
        });
    }

    /// A sprite sheet's cell, or an animated texture's frame, is the part of
    /// the texture being drawn, so the inset is measured inside that window:
    /// here half the width of a 64 by 48 texture, where forty texels a side
    /// cannot fit and both pairs are scaled down to the window they are in.
    #[test]
    fn a_sprite_window_is_sliced_inside_itself() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let mut image = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let source = script! {
                use mod.prelude.widgets.*
                Image{
                    width: Fill height: Fill
                    fit: ImageFit.Slice
                    slice: 40
                    draw_bg +: {image_scale: vec2(0.5, 1.0)}
                }
            };
            let value = vm.eval(source);
            Image::script_from_value(vm, value)
        });
        install_texture(&mut cx, &mut image, 64, 48);
        draw_once(&mut cx, &mut image);
        assert_eq!(image.draw_bg.slice_inset, vec4(16.0, 24.0, 16.0, 24.0));
        });
    }
}

/// The shape, the sampler and the bar are the shader's own work, and nothing
/// in a `cargo test` can rasterize a quad to look at the result. So these
/// read the code the shader compiles to instead: a test that only checked
/// the four fields arrived would stay green with the whole of `pixel`
/// replaced by `return picture`, or with `sample_at` collapsed to one
/// filtered read, which is exactly the pair of traps this widget was asked
/// to close.
#[cfg(test)]
mod shader_tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;

    /// The picture's fragment source, compiled for the web backend — the one
    /// whose sampler helpers carry the channel-order correction in their own
    /// names, so the source says which read was asked for.
    fn fragment_source(cx: &mut Cx) -> String {
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let shader = vm
                .bx
                .heap
                .type_default_for_id(DrawImage::script_type_id_static())
                .expect("the picture's draw shader is registered");
            let source = script! {
                mod.shader.test_compile_draw_source(#(ScriptValue::from(shader)), "glsl", false)
            };
            let value = vm.eval(source);
            let text = vm
                .bx
                .heap
                .string_with(value, |_heap, text| text.to_string())
                .expect("the compiler answers with source");
            assert!(
                !text.starts_with("ERRORS:"),
                "the picture's shader did not compile: {text}"
            );
            assert!(!text.is_empty(), "the picture's shader compiled to nothing");
            text
        })
    }

    /// The one emitted line that mentions `needle`, so a call can be pinned
    /// by what it was handed without pinning the whole of the expression the
    /// compiler wrote around it.
    fn line_with<'a>(source: &'a str, needle: &str) -> &'a str {
        source
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("nothing in the compiled shader has {needle}"))
    }

    /// The `count` emitted lines from the one that mentions `needle`, for
    /// pinning a branch together with what it returns.
    fn block_from(source: &str, needle: &str, count: usize) -> String {
        let start = source
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("nothing in the compiled shader has {needle}"));
        source
            .lines()
            .skip(start)
            .take(count)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The body of the emitted function whose signature line mentions
    /// `needle`, found by counting braces, so an `if` block inside it does
    /// not end it early.
    fn function_body(source: &str, needle: &str) -> String {
        let mut lines = source.lines().skip_while(|line| !line.contains(needle));
        let head = lines
            .next()
            .unwrap_or_else(|| panic!("nothing in the compiled shader has {needle}"));
        let mut depth = head.matches('{').count() as i64 - head.matches('}').count() as i64;
        let mut body = Vec::new();
        for line in lines {
            if depth <= 0 {
                break;
            }
            depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
            body.push(line);
        }
        body.join("\n")
    }

    /// A sliced picture maps every pixel through its nine parts, and every
    /// read it takes still goes through `sample_at`: the whole-texel snap and
    /// the channel-order-corrected read are what a panel skin shown on the
    /// web target needs just as much as a photograph does. The branch comes
    /// first in `get_color`, before the framing test, which is what keeps a
    /// sliced picture from ever drawing a bar. The tiling and the half-texel
    /// clamp are the axis function's own, so they are pinned there.
    #[test]
    fn a_sliced_picture_reads_through_the_corrected_helper() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let source = fragment_source(&mut cx);
        let branch = block_from(&source, "rustinst_slice_mode.x > 0.0", 3);
        assert!(
            branch.contains("io_slice_color()"),
            "slicing turned on does not return the sliced colour: {branch}"
        );
        let get_color = function_body(&source, "vec4 io_get_color(");
        let sliced = get_color.find("rustinst_slice_mode.x > 0.0");
        let framed = get_color.find("rustinst_image_dim_w <= 0.0");
        assert!(
            matches!((sliced, framed), (Some(sliced), Some(framed)) if sliced < framed),
            "the slice branch does not come before the framing test: {get_color}"
        );
        let colour = function_body(&source, "vec4 io_slice_color(");
        assert!(
            colour.contains("io_sample_at("),
            "the sliced colour does not read through sample_at: {colour}"
        );
        assert!(!colour.contains("sample2d"), "the sliced colour reads the texture itself: {colour}");
        let axis = function_body(&source, "vec2 io_slice_axis(");
        for call in ["fract(", "floor(", "clamp("] {
            assert!(axis.contains(call), "the slice axis has no {call}: {axis}");
        }
        assert!(
            !source.contains("sample2d("),
            "a read in the picture's shader skips the channel-order correction"
        );
        });
    }

    /// The headline behaviour: the picture clips itself to a rounded box and
    /// strokes its own edge, in the shader that draws the picture and
    /// nowhere else.
    #[test]
    fn the_picture_clips_and_strokes_itself() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let source = fragment_source(&mut cx);
        // The box a view draws: inset by the stroke on all four sides and
        // rounded by the radius the caller wrote.
        let shape = line_with(&source, "Sdf2d_box(l_sdf");
        assert!(
            shape.contains("rustinst_border_radius"),
            "the box is not rounded by border_radius: {shape}"
        );
        assert!(
            shape.contains("rustinst_border_size"),
            "the box is not inset by the stroke: {shape}"
        );
        // The picture is that box's fill, which is what clips it.
        assert!(
            source.contains("Sdf2d_fill_keep_premul(l_sdf"),
            "the picture is not the fill of the box, so nothing clips it"
        );
        // And the edge takes the stroke it was asked for.
        let stroke = line_with(&source, "Sdf2d_stroke(l_sdf");
        assert!(
            stroke.contains("rustinst_border_color"),
            "the edge is not stroked in border_color: {stroke}"
        );
        assert!(
            stroke.contains("rustinst_border_size"),
            "the stroke is not the width asked for: {stroke}"
        );
        // Neither one asked for leaves before any of it, so a picture that
        // wanted no shape draws the quad it always drew.
        let plain = line_with(&source, "rustinst_border_radius <= 0.0");
        assert!(
            plain.contains("rustinst_border_size <= 0.0"),
            "an unasked picture no longer returns before the shape work: {plain}"
        );
        });
    }

    /// `sample_mode` reads the texture a whole texel at a time by snapping
    /// the coordinate to the texel's centre and filtering anyway, never by
    /// asking the sampler for a nearest read: only the corrected read has
    /// its channel order fixed on the web target, where the helper's name is
    /// which read it is, so a nearest one there comes back with red and blue
    /// swapped. Zero reads straight through, as the picture always did.
    #[test]
    fn a_whole_texel_read_snaps_and_keeps_the_channel_order() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let source = fragment_source(&mut cx);
        assert!(
            source.contains("sample2d_bgra("),
            "the picture does not read its texture through the corrected helper"
        );
        assert!(
            !source.contains("sample2d("),
            "a read in the picture's shader skips the channel-order correction"
        );
        // A whole texel is had by snapping, and the snapped coordinate is
        // what gets read.
        let snap = line_with(&source, "l_snapped =");
        assert!(snap.contains("floor("), "the snapped read does not snap: {snap}");
        assert!(
            source.contains("sample2d_bgra(tex_image_texture, l_snapped)"),
            "the snapped coordinate is worked out and then not read"
        );
        // Below zero every read is snapped; above zero the number is the
        // device pixel ratio, and the read stays filtered until a texel is
        // drawn more than four device pixels wide.
        let choice = line_with(&source, "rustinst_sample_mode < 0.0");
        assert!(
            choice.contains("l_device_px_per_texel > 4.0"),
            "the four-device-pixel threshold is gone: {choice}"
        );
        let ratio = line_with(&source, "l_device_px_per_texel =");
        assert!(
            ratio.contains("rustinst_sample_mode"),
            "the ratio is not scaled by sample_mode: {ratio}"
        );
        assert!(
            source.contains("rustinst_sample_mode == 0.0"),
            "a picture that asked for nothing no longer reads straight through"
        );
        });
    }

    /// The bar is decided by the FRAMING — `fit_scale` and `fit_pan` — and
    /// not by the coordinate the texture is finally read at, which carries
    /// the caller's own pan and would turn a sprite sheet's cell or a zoomed
    /// viewer into a transparent band. The rotated picture keeps its own
    /// mapping and its own test.
    #[test]
    fn the_framing_is_what_turns_the_bar_on() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let source = fragment_source(&mut cx);
        // The framing, the test on it and the bar, in the branch that says
        // the picture is not rotated.
        let bar = block_from(&source, "rustinst_image_dim_w <= 0.0", 4);
        assert!(
            bar.contains("rustinst_fit_scale") && bar.contains("rustinst_fit_pan"),
            "the bar is not decided by the framing: {bar}"
        );
        assert!(
            !bar.contains("rustinst_image_pan"),
            "the caller's own pan reaches the bar test: {bar}"
        );
        assert!(
            bar.contains("rustinst_letterbox_color"),
            "past the framing is not the bar colour: {bar}"
        );
        assert!(
            bar.contains("1.0001"),
            "the framing test lost the slack that keeps an edge fragment out of the bar: {bar}"
        );
        // And the read itself tests nothing: past the edge of an unrotated
        // picture is the edge texel, exactly as it always was.
        let read = block_from(&source, "l_uv = ((var_pos", 2);
        assert!(
            read.contains("io_sample_at"),
            "the unrotated read no longer samples: {read}"
        );
        assert!(
            !read.contains("rustinst_letterbox_color"),
            "the bar moved onto the final coordinate, where a pan would grow one: {read}"
        );
        });
    }

    /// The module's markup names the slice enums, and a name the markup
    /// cannot see is a logged error rather than a failure: the Rust
    /// defaults happen to match, so nothing else would notice. Captured,
    /// the module evaluates clean.
    #[test]
    fn the_image_markup_evaluates_without_script_errors() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            crate::script_mod(vm);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:#?}");
            let value = crate::script_eval!(vm, {use mod.widgets.* Image{}});
            let image = Image::script_from_value(vm, value);
            assert_eq!(image.slice_edge, ImageSliceEdge::Stretch);
            assert_eq!(image.slice_center, ImageSliceCenter::Stretch);
            assert_eq!(image.slice_units, ImageSliceUnits::Points);
        });
        });
    }
}
