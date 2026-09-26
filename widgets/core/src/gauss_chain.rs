//! Window-independent cached Gaussian pyramid, using the same filters as GaussStack.
//!
//! All passes are GPU work recorded by the UI thread; there is no CPU readback,
//! worker synchronization or pixel processing here. The chain requests no frames
//! by itself. Call `run` only when the source changes; animation samples its cached
//! textures. Native GPU backends and the gpusim renderer use the same shaders.
use crate::{
    makepad_draw::*,
    window::{DrawGaussDownsample, DrawGaussUpsample},
};

pub const MAX_LEVELS: u8 = 6;
pub const DEFAULT_SIZE_CAP: usize = 2048;
const GAUSS_FLOOR_LEVEL: usize = 2;
const GAUSS_SMOOTH_LEVEL_START: usize = 3;

struct GaussSmoothStage {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

struct GaussChainLevel {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
    // One tent-upsample per resolution doubling from this level's own size back up to the
    // floor size; the last stage's texture is what the snapshot exposes. Empty for levels
    // at or above the floor resolution.
    smooth_stages: Vec<GaussSmoothStage>,
}

/// Cached source (index 0) and the requested filtered levels (indices 1..=N).
/// Texture handles are reused on the next run; retain the chain with each frozen
/// capture rather than running it again while an older snapshot is still in use.
pub struct GaussLevels {
    textures: Vec<Texture>,
    radii: Vec<f64>,
    physical_size: (usize, usize),
}

impl GaussLevels {
    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    /// Per-level blur radii in logical pixels, expressed as per-axis sigma.
    /// Calibration includes the downsample and tent kernels and their bilinear
    /// taps; a consumer's own reconstruction filter is not included. At image
    /// edges or non-power-of-two sizes the discrete kernels are approximate.
    pub fn radii(&self) -> &[f64] {
        &self.radii
    }

    /// Capped physical source footprint; the source itself remains caller-owned.
    pub fn physical_size(&self) -> (usize, usize) {
        self.physical_size
    }

    /// Choose two texture indices and a linear color-mix weight for a logical
    /// blur radius (sigma). Interpolate variance, not level number: mixing two
    /// normalized kernels mixes their variances. Zero/negative/NaN selects the
    /// source; radii beyond the recorded range clamp to the last level.
    pub fn sample(&self, radius_logical_px: f64) -> (usize, usize, f32) {
        sample_radii(&self.radii, radius_logical_px)
    }
}

/// Owns up to six downsample targets and their progressive tent intermediates.
/// No window, capture pass, depth attachment or CPU readback is required.
///
/// Public chains use `live_with_parent = false`: a consumer repaint reuses the
/// results without executing the filters again. GaussStack's private constructor
/// preserves its existing live-window behavior. No timers or frames are requested.
///
/// After `run`, call `cx2d.make_child_pass(chain.output_pass().unwrap())` on
/// **every draw-list recording that consumes the textures**, even without a new
/// run. For a freshly rendered source, attach its producer to `input_pass()`:
/// `cx.attach_child_pass(source_pass.draw_pass_id(), input.draw_pass_id(),
/// cx.passes[input.draw_pass_id()].main_draw_list_id)`. Parents are consumers;
/// the platform sorts deepest producers first, independently of recycled IDs.
/// With zero filter levels, attach the source producer directly to its consumer.
/// The platform has one parent per pass: multiple readers must share a common
/// consuming pass or be serialized in a dependency chain, as BackdropCompositor
/// does. Do not reparent the same producer independently to multiple chains.
pub struct GaussChain {
    levels: Vec<GaussChainLevel>,
    result: GaussLevels,
    logical_size: Vec2d,
    size_cap: usize,
    live: bool,
    shaders: Option<(DrawGaussDownsample, DrawGaussUpsample)>,
}

impl GaussChain {
    /// `logical_size` is the viewport size at which the source will be displayed.
    /// Capture at `capture_size(dpi)` to bound source allocation as well as the
    /// chain. Arbitrary larger/smaller source textures are also accepted; their
    /// texel spacing is included in radius calibration, but remains caller-owned.
    pub fn new(cx: &mut Cx, logical_size: Vec2d) -> Self {
        Self::with_size_cap(cx, logical_size, DEFAULT_SIZE_CAP)
    }

    /// Supply the physical long-side cap (at least one pixel). This limits the
    /// virtual source footprint before level 1 halves it, preserving aspect ratio.
    pub fn with_size_cap(_cx: &mut Cx, logical_size: Vec2d, size_cap: usize) -> Self {
        assert!(logical_size.x.is_finite() && logical_size.x > 0.0);
        assert!(logical_size.y.is_finite() && logical_size.y > 0.0);
        assert!(size_cap > 0);
        Self {
            levels: Vec::new(),
            result: GaussLevels {
                textures: Vec::new(),
                radii: Vec::new(),
                physical_size: (0, 0),
            },
            logical_size,
            size_cap,
            live: false,
            shaders: None,
        }
    }

    /// Physical capture size at this display DPI. Flooring and the one-pixel
    /// minimum match the targets used by `run` and `byte_estimate`.
    pub fn capture_size(&self, dpi: f64) -> (usize, usize) {
        capture_size(self.logical_size, dpi, self.size_cap)
    }

    /// Record exactly N downsample passes plus the tent stages needed by those
    /// N levels, once for this call. N must be <= 6; zero retains only the source.
    /// Re-running with fewer levels releases the surplus targets.
    ///
    /// Call on the UI thread before recording the consuming draw list, outside
    /// an active CxDraw/Cx2d (this method creates its own draw context). An Auto
    /// source texture's producer must already have a size and DPI; fixed/vector
    /// textures supply their own dimensions. `run` does not capture or repaint
    /// that producer. Declare the source and output dependencies as above.
    pub fn run(&mut self, cx: &mut Cx, source: &Texture, levels: u8, dpi: f64) -> &GaussLevels {
        assert!(
            levels <= MAX_LEVELS,
            "a Gaussian chain has at most six levels"
        );
        let size = self.capture_size(dpi);
        let source_size = source_size(cx, source);
        self.levels.truncate(levels as usize);
        if self.levels.len() < levels as usize {
            let added = Self::allocate_levels(cx, self.levels.len(), levels as usize, false);
            // Only construct missing levels, preserving already allocated handles.
            self.levels.extend(added);
        }
        if levels > 0 {
            let (mut downsample, mut upsample) = self.shaders.take().unwrap_or_else(|| {
                cx.with_vm(|vm| {
                    (
                        DrawGaussDownsample::script_new_with_default(vm),
                        DrawGaussUpsample::script_new_with_default(vm),
                    )
                })
            });
            let event = DrawEvent::default();
            {
                let mut draw = CxDraw::new(cx, &event);
                let mut cx2d = Cx2d::new(&mut draw);
                let root = dvec2(size.0 as f64, size.1 as f64);
                self.draw_mip_chain_to(&mut cx2d, &mut downsample, source, root, levels as usize);
                self.draw_high_blur_chain_to(&mut cx2d, &mut upsample, root, levels as usize);
            }
            // Recorded draw calls own their inputs. Keeping a shader's last
            // binding would otherwise retain a surplus tent target after shrink.
            downsample.draw_vars.empty_texture(0);
            upsample.draw_vars.empty_texture(0);
            self.shaders = Some((downsample, upsample));
            // The former consumer may have gone away or been a surplus level.
            // A fresh run is reattached by its new consumer below the call site.
            let output = self.output_pass().unwrap().draw_pass_id();
            cx.passes[output].parent = CxDrawPassParent::None;
            cx.passes[output].attached_by = None;
            let dependencies = self.dependencies(levels as usize);
            for pair in dependencies.windows(2) {
                let owner = cx.passes[pair[1]].main_draw_list_id;
                cx.attach_child_pass(pair[0], pair[1], owner);
            }
        }
        self.result.textures.clear();
        self.result.textures.push(source.clone());
        self.result.textures.extend(self.mip_textures());
        self.result.radii = level_radii(self.logical_size, source_size, size, levels);
        self.result.physical_size = size;
        &self.result
    }

    /// Reuse the cached levels during animation or a later consumer repaint,
    /// without calling `run` again. None until the first run (including N = 0).
    pub fn levels(&self) -> Option<&GaussLevels> {
        (!self.result.textures.is_empty()).then_some(&self.result)
    }

    /// The first downsample pass: a same-frame source producer precedes this.
    pub fn input_pass(&self) -> Option<&DrawPass> {
        self.levels.first().map(|level| &level.pass)
    }

    /// The last pass in the internally serialized dependency chain. Attaching
    /// this to a consumer orders *all* requested level textures before it.
    pub fn output_pass(&self) -> Option<&DrawPass> {
        self.levels.last().map(|level| {
            level
                .smooth_stages
                .last()
                .map_or(&level.pass, |stage| &stage.pass)
        })
    }

    pub(crate) fn new_for_stack(cx: &mut Cx) -> Self {
        let mut chain = Self::new(cx, dvec2(1.0, 1.0));
        chain.live = true;
        chain.levels = Self::allocate_levels(cx, 0, MAX_LEVELS as usize, true);
        chain
    }

    pub(crate) fn mip_textures(&self) -> Vec<Texture> {
        self.levels
            .iter()
            .map(|level| {
                level
                    .smooth_stages
                    .last()
                    .map_or_else(|| level.texture.clone(), |stage| stage.texture.clone())
            })
            .collect()
    }

    pub(crate) fn dependencies(&self, count: usize) -> Vec<DrawPassId> {
        let mut passes: Vec<_> = self
            .levels
            .iter()
            .take(count)
            .map(|level| level.pass.draw_pass_id())
            .collect();
        for level in self.levels.iter().take(count) {
            passes.extend(
                level
                    .smooth_stages
                    .iter()
                    .map(|stage| stage.pass.draw_pass_id()),
            );
        }
        passes
    }

    fn allocate_levels(
        cx: &mut Cx,
        start: usize,
        count: usize,
        live: bool,
    ) -> Vec<GaussChainLevel> {
        let mut levels = Vec::with_capacity(count - start);
        for index in start..count {
            let pass = DrawPass::new_with_name(cx, &format!("gauss_mip_{index}"));
            pass.set_live_with_parent(cx, live);
            let draw_list = DrawList2d::new(cx);
            let texture = Self::new_render_texture(cx);
            pass.set_color_texture(
                cx,
                &texture,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            let stage_count = if index >= GAUSS_SMOOTH_LEVEL_START {
                index - GAUSS_FLOOR_LEVEL
            } else {
                0
            };
            let mut smooth_stages = Vec::with_capacity(stage_count);
            for stage in 0..stage_count {
                let smooth_pass =
                    DrawPass::new_with_name(cx, &format!("gauss_smooth_mip_{index}_{stage}"));
                smooth_pass.set_live_with_parent(cx, live);
                let smooth_draw_list = DrawList2d::new(cx);
                let smooth_texture = Self::new_render_texture(cx);
                smooth_pass.set_color_texture(
                    cx,
                    &smooth_texture,
                    DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
                );
                smooth_stages.push(GaussSmoothStage {
                    pass: smooth_pass,
                    draw_list: smooth_draw_list,
                    texture: smooth_texture,
                });
            }
            levels.push(GaussChainLevel {
                pass,
                draw_list,
                texture,
                smooth_stages,
            });
        }

        levels
    }

    fn new_render_texture(cx: &mut Cx) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        )
    }

    pub(crate) fn draw_mip_chain_to(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        source: &Texture,
        root_size: Vec2d,
        count: usize,
    ) {
        let dpi = if cx.inside_pass() {
            cx.current_dpi_factor()
        } else {
            1.0
        };
        let live = self.live;
        let mut source_texture = source.clone();

        for (index, level) in self.levels.iter_mut().take(count).enumerate() {
            // MAKEPAD_GAUSS_FAST=1: probe rig — stop the chain early to
            // measure how much of a frame the pass COUNT itself costs.
            if self.live && gauss_fast() && index > 3 {
                break;
            }
            let level_size = render_size(root_size, dpi, index, live);

            level.pass.set_size(cx, level_size);
            if cx.inside_pass() {
                cx.make_child_pass(&level.pass);
            }
            cx.begin_pass(&level.pass, Some(dpi));
            level.draw_list.begin_always(cx);

            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            downsample.draw_vars.set_texture(0, &source_texture);
            downsample.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass_size,
                },
            );
            cx.end_pass_sized_turtle();

            level.draw_list.end(cx);
            cx.end_pass(&level.pass);
            source_texture = level.texture.clone();
        }
    }

    // Re-home each deep mip at the floor resolution: starting from the level's own raw mip,
    // tent-upsample one resolution doubling at a time until the floor size is reached. The
    // progressive doubling matters — a single stretch from 1/64 straight to 1/8 would keep the
    // source's texel lattice; each doubling convolves another tent on top and gaussianizes it.
    pub(crate) fn draw_high_blur_chain_to(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
        count: usize,
    ) {
        if self.live && gauss_fast() {
            return;
        }
        let dpi = if cx.inside_pass() {
            cx.current_dpi_factor()
        } else {
            1.0
        };
        for index in GAUSS_SMOOTH_LEVEL_START..count.min(self.levels.len()) {
            let level = &mut self.levels[index];
            let mut source_texture = level.texture.clone();
            for (stage_index, stage) in level.smooth_stages.iter_mut().enumerate() {
                let stage_size = render_size(root_size, dpi, index - 1 - stage_index, self.live);
                stage.pass.set_size(cx, stage_size);
                if cx.inside_pass() {
                    cx.make_child_pass(&stage.pass);
                }
                cx.begin_pass(&stage.pass, Some(dpi));
                stage.draw_list.begin_always(cx);

                let pass_size = cx.current_pass_size();
                cx.begin_root_turtle(pass_size, Layout::flow_overlay());
                upsample.draw_vars.set_texture(0, &source_texture);
                upsample.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(0.0, 0.0),
                        size: pass_size,
                    },
                );
                cx.end_pass_sized_turtle();

                stage.draw_list.end(cx);
                cx.end_pass(&stage.pass);
                source_texture = stage.texture.clone();
            }
        }
    }
}

fn gauss_fast() -> bool {
    thread_local! { static ON: bool = std::env::var_os("MAKEPAD_GAUSS_FAST").is_some(); }
    ON.with(|v| *v)
}

fn render_size(root: Vec2d, dpi: f64, index: usize, live: bool) -> Vec2d {
    let scale = (1usize << (index + 1)) as f64;
    if live {
        let minimum = 1.0 / dpi.max(1.0);
        dvec2((root.x / scale).max(minimum), (root.y / scale).max(minimum))
    } else {
        let size = mip_size((root.x as usize, root.y as usize), index);
        dvec2(size.0 as f64, size.1 as f64)
    }
}

fn capture_size(logical: Vec2d, dpi: f64, cap: usize) -> (usize, usize) {
    assert!(
        dpi.is_finite() && dpi > 0.0,
        "DPI must be finite and positive"
    );
    let scale = dpi.min(cap as f64 / logical.x.max(logical.y));
    (
        (logical.x * scale).floor().max(1.0) as usize,
        (logical.y * scale).floor().max(1.0) as usize,
    )
}

fn mip_size(size: (usize, usize), index: usize) -> (usize, usize) {
    (
        (size.0 >> (index + 1)).max(1),
        (size.1 >> (index + 1)).max(1),
    )
}

fn source_size(cx: &mut Cx, source: &Texture) -> (usize, usize) {
    let format = source.get_format(cx);
    match format {
        TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width, height },
            ..
        }
        | TextureFormat::RenderRGBAf16 {
            size: TextureSize::Fixed { width, height },
            ..
        }
        | TextureFormat::RenderRGBAf32 {
            size: TextureSize::Fixed { width, height },
            ..
        }
        | TextureFormat::RenderRf32 {
            size: TextureSize::Fixed { width, height },
            ..
        } => {
            assert!(*width > 0 && *height > 0, "source texture must be nonempty");
            return (*width, *height);
        }
        _ => {}
    }
    if let Some(size) = format.vec_width_height() {
        assert!(size.0 > 0 && size.1 > 0, "source texture must be nonempty");
        return size;
    }
    // Auto render targets are not necessarily allocated yet: use their already
    // recorded producer's geometry rather than reading back or inspecting GPU state.
    for id in cx.passes.id_iter() {
        let pass = &cx.passes[id];
        if pass
            .color_textures
            .iter()
            .any(|attachment| attachment.texture.texture_id() == source.texture_id())
        {
            let dpi = pass
                .dpi_factor
                .expect("record the source producer before GaussChain::run");
            let size = cx
                .get_pass_rect(id, dpi)
                .expect("source producer needs a size")
                .size
                * dpi;
            return (size.x.max(1.0) as usize, size.y.max(1.0) as usize);
        }
    }
    panic!("source texture needs fixed/vector dimensions or a recorded render pass");
}

fn level_radii(
    logical: Vec2d,
    source: (usize, usize),
    size: (usize, usize),
    levels: u8,
) -> Vec<f64> {
    let spacing_squared = |size: (usize, usize)| {
        let x = logical.x / size.0 as f64;
        let y = logical.y / size.1 as f64;
        (x * x + y * y) * 0.5
    };
    let mut radii = vec![0.0];
    let mut variance = 0.0;
    let mut input_size = source;
    for index in 0..levels as usize {
        // Downsample offsets have per-axis variance 1.5; half-texel
        // bilinear reconstruction adds 0.25 at a 2:1 grid.
        variance += 1.75 * spacing_squared(input_size);
        let mut exposed_variance = variance;
        if index >= GAUSS_SMOOTH_LEVEL_START {
            for stage in 0..(index - GAUSS_FLOOR_LEVEL) {
                // Half-source-texel tent taps: 1/8; quarter-texel
                // bilinear reconstruction: 3/16. Total 5/16.
                exposed_variance += 0.3125 * spacing_squared(mip_size(size, index - stage));
            }
        }
        radii.push(exposed_variance.sqrt());
        input_size = mip_size(size, index);
    }
    radii
}

fn sample_radii(radii: &[f64], radius: f64) -> (usize, usize, f32) {
    if !(radius > 0.0) || radii.len() <= 1 {
        return (0, 0, 0.0);
    }
    for b in 1..radii.len() {
        if radius < radii[b] {
            let a = b - 1;
            let t = (radius * radius - radii[a] * radii[a])
                / (radii[b] * radii[b] - radii[a] * radii[a]);
            return (a, b, t as f32);
        }
        if radius == radii[b] {
            return (b, b, 0.0);
        }
    }
    let last = radii.len() - 1;
    (last, last, 0.0)
}

/// BGRA8 bytes for a caller-owned physical source plus N raw downsample levels
/// and all retained tent intermediates. No depth, allocator padding or mipmaps
/// are included. Pass an already capped size (`capture_size`); dimensions must
/// be nonzero and levels <= 6. Saturates at u64::MAX on unrepresentable inputs.
/// For 2048²: source 16 MiB, source + raw levels 21.33203125 MiB, with tents
/// 22.22265625 MiB (the design's conservative 22.23 MiB rounded-up budget).
pub fn byte_estimate(size: (usize, usize), levels: u8) -> u64 {
    assert!(levels <= MAX_LEVELS);
    assert!(size.0 > 0 && size.1 > 0);
    let bytes = |(w, h): (usize, usize)| (w as u64).saturating_mul(h as u64).saturating_mul(4);
    let mut total = bytes(size);
    for index in 0..levels as usize {
        total = total.saturating_add(bytes(mip_size(size, index)));
        if index >= GAUSS_SMOOTH_LEVEL_START {
            for stage in 0..(index - GAUSS_FLOOR_LEVEL) {
                total = total.saturating_add(bytes(mip_size(size, index - 1 - stage)));
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_bgra8_memory_budget() {
        let size = (2048, 2048);
        let source = byte_estimate(size, 0);
        let raw = source
            + (0..6)
                .map(|index| {
                    let (w, h) = mip_size(size, index);
                    (w * h * 4) as u64
                })
                .sum::<u64>();
        let total = byte_estimate(size, 6);
        assert_eq!(source, 16_777_216);
        assert_eq!(raw, 22_368_256);
        assert_eq!(total, 23_302_144);
        let mib = 1024.0 * 1024.0;
        assert_eq!(source as f64 / mib, 16.0);
        assert_eq!((raw as f64 / mib * 100.0).round() / 100.0, 21.33);
        // The design rounds the final budget UP; exact allocation is 22.22265625.
        assert_eq!((total as f64 / mib * 100.0).ceil() / 100.0, 22.23);
        assert_eq!(byte_estimate((2048, 1152), 6), 13_107_456);
    }

    #[test]
    fn cap_aspect_ratio_odd_sizes_and_one_pixel_levels() {
        let root = dvec2(3840.0, 2160.0);
        assert_eq!(capture_size(root, 1.0, 2048), (2048, 1152));
        assert_eq!(capture_size(root, 2.0, 2048), (2048, 1152));
        assert_eq!(capture_size(dvec2(100.0, 200.0), 1.5, 2048), (150, 300));
        assert_eq!(capture_size(dvec2(200.0, 100.0), 2.0, 100), (100, 50));
        assert_eq!(mip_size((101, 7), 0), (50, 3));
        assert_eq!(mip_size((101, 7), 5), (1, 1));
        assert_eq!(byte_estimate((1, 1), 6), 13 * 4);
        assert_eq!(
            render_size(dvec2(101.0, 7.0), 2.0, 0, true),
            dvec2(50.5, 3.5)
        );
    }

    #[test]
    fn logical_radius_is_independent_of_dpi_and_capture_cap() {
        let root = dvec2(1920.0, 1080.0);
        for (dpi, cap) in [(1.0, 2048), (2.0, 2048), (2.0, 4096), (1.0, 960)] {
            let size = capture_size(root, dpi, cap);
            let radii = level_radii(root, size, size, 6);
            for radius in [0.0, 0.5, 2.0, 6.0, 12.0] {
                let (a, b, t) = sample_radii(&radii, radius);
                let variance = radii[a].powi(2) * (1.0 - t as f64) + radii[b].powi(2) * t as f64;
                assert!((variance.sqrt() - radius).abs() < 0.00001);
            }
        }
        let root = dvec2(2048.0, 2048.0);
        let one = level_radii(root, (2048, 2048), (2048, 2048), 6);
        let two = level_radii(root, (4096, 4096), (4096, 4096), 6);
        for (radius, variance) in one
            .iter()
            .zip([0.0_f64, 1.75, 8.75, 36.75, 228.75, 996.75, 4068.75])
        {
            assert!((radius - variance.sqrt()).abs() < 1e-10);
        }
        for (a, b) in one.iter().zip(two.iter()) {
            assert!((a - b * 2.0).abs() < 0.00001);
        }
    }

    #[test]
    fn level_sampling_clamps_and_has_no_integer_boundary_jump() {
        let radii = level_radii(dvec2(2048.0, 2048.0), (2048, 2048), (2048, 2048), 6);
        assert_eq!(sample_radii(&radii, -1.0), (0, 0, 0.0));
        assert_eq!(sample_radii(&radii, f64::NAN), (0, 0, 0.0));
        assert_eq!(sample_radii(&radii, f64::INFINITY), (6, 6, 0.0));
        assert_eq!(sample_radii(&[0.0], 12.0), (0, 0, 0.0));
        for index in 1..=6 {
            assert_eq!(sample_radii(&radii, radii[index]), (index, index, 0.0));
            let (a, b, t) = sample_radii(&radii, radii[index] - 1e-8);
            assert_eq!((a, b), (index - 1, index));
            assert!((t - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn cached_run_records_only_requested_passes_and_reuses_targets() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let source = Texture::new_with_format(
            &mut cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: 256,
                    height: 128,
                },
                initial: true,
            },
        );
        let mut chain = GaussChain::new(&mut cx, dvec2(256.0, 128.0));
        assert!(chain.levels().is_none());
        cx.new_draw_event = DrawEvent::default();
        assert_eq!(chain.run(&mut cx, &source, 0, 1.0).textures().len(), 1);
        assert!(chain.output_pass().is_none());
        assert_eq!(chain.run(&mut cx, &source, 4, 1.0).textures().len(), 5);
        let dependencies = chain.dependencies(4);
        assert_eq!(dependencies.len(), 5);
        for id in &dependencies {
            assert!(!cx.passes[*id].live_with_parent);
            assert!(cx.passes[*id].paint_dirty);
            assert!(cx.passes[*id].main_draw_list_id.is_some());
        }
        for pair in dependencies.windows(2) {
            assert!(
                matches!(cx.passes[pair[0]].parent, CxDrawPassParent::DrawPass(id) if id == pair[1])
            );
            assert!(!cx.pass_attachment_is_stale(pair[0]));
        }
        let ids: Vec<_> = chain
            .result
            .textures()
            .iter()
            .map(Texture::texture_id)
            .collect();
        chain.run(&mut cx, &source, 4, 1.0);
        assert_eq!(
            ids,
            chain
                .result
                .textures()
                .iter()
                .map(Texture::texture_id)
                .collect::<Vec<_>>()
        );
        chain.run(&mut cx, &source, 6, 1.0);
        assert_eq!(chain.dependencies(6).len(), 12);
        chain.run(&mut cx, &source, 2, 1.0);
        assert_eq!(chain.dependencies(6).len(), 2);
        assert_eq!(chain.result.textures().len(), 3);
        assert_eq!(chain.levels().unwrap().textures().len(), 3);
        assert!(!cx.new_draw_event.will_redraw());
    }

    #[test]
    fn consumer_reattaches_cached_output_without_rerecording_filters() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        // Allocate the consumer first: execution must follow dependencies,
        // rather than assuming increasing pool IDs are producer order.
        let consumer = DrawPass::new(&mut cx);
        consumer.set_size(&mut cx, dvec2(256.0, 128.0));
        let mut list = DrawList2d::new(&mut cx);
        let source = Texture::new_with_format(
            &mut cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: 256,
                    height: 128,
                },
                initial: true,
            },
        );
        let mut chain = GaussChain::new(&mut cx, dvec2(256.0, 128.0));
        chain.run(&mut cx, &source, 6, 1.0);
        let dependencies = chain.dependencies(6);
        let generations: Vec<_> = dependencies
            .iter()
            .map(|id| {
                cx.passes[*id].paint_dirty = false;
                let list = cx.passes[*id].main_draw_list_id.unwrap();
                cx.draw_lists[list].redraw_id
            })
            .collect();
        let output = chain.output_pass().unwrap();
        for attach in [true, false, true] {
            let event = DrawEvent::default();
            {
                let mut draw = CxDraw::new(&mut cx, &event);
                let mut cx2d = Cx2d::new(&mut draw);
                cx2d.begin_pass(&consumer, Some(1.0));
                list.begin_always(&mut cx2d);
                if attach {
                    cx2d.make_child_pass(output);
                }
                list.end(&mut cx2d);
                cx2d.end_pass(&consumer);
            }
            assert_eq!(cx.pass_attachment_is_stale(output.draw_pass_id()), !attach);
            assert!(cx.passes[consumer.draw_pass_id()].paint_dirty);
            for (id, generation) in dependencies.iter().zip(&generations) {
                assert!(!cx.passes[*id].paint_dirty);
                assert!(!cx.passes[*id].live_with_parent);
                let list = cx.passes[*id].main_draw_list_id.unwrap();
                assert_eq!(cx.draw_lists[list].redraw_id, *generation);
            }
        }
    }
}
