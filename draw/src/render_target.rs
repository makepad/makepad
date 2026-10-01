//! Offscreen render targets: [`RenderTarget`] (one pass, its draw list and
//! its colour texture, plus a depth texture on demand, reused frame after
//! frame) and [`RenderTargetPool`] (targets lent out per frame by size and
//! format, given back after a run of frames without a use).
//!
//! A pass names its attachments too, so a texture a host lets go of stays
//! alive while a pass still has it attached. [`RenderTarget::release`]
//! drops both, which is what makes an idle target's memory come back.

use crate::{cx_2d::Cx2d, draw_list_2d::{DrawList2d, DrawListExt}, makepad_platform::*, turtle::Layout};

/// The colour format of a [`RenderTarget`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RenderTargetFormat {
    /// 8-bit BGRA (a display-referred picture).
    #[default]
    Bgra8,
    /// Half-float RGBA (a linear, HDR picture).
    Rgba16f,
}

/// One offscreen pass, its draw list and its colour target.
pub struct RenderTarget {
    pub pass: DrawPass,
    pub list: DrawList2d,
    pub texture: Texture,
    depth: Option<Texture>,
    size: (u32, u32),
    format: RenderTargetFormat,
}

fn unsized_texture(cx: &mut Cx) -> Texture {
    Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true })
}

impl RenderTarget {
    /// A target with no pixels yet (sized by [`RenderTarget::ensure`]).
    pub fn new(cx: &mut Cx, name: &str) -> Self {
        Self {
            pass: DrawPass::new_with_name(cx, name),
            list: DrawList2d::new(cx),
            texture: unsized_texture(cx),
            depth: None,
            size: (0, 0),
            format: RenderTargetFormat::Bgra8,
        }
    }

    /// Its pixel size ((0, 0) while released).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> RenderTargetFormat {
        self.format
    }

    /// Size it (and set its format) for this frame: a new texture only
    /// when either changed. Returns whether it was reallocated.
    pub fn ensure(&mut self, cx: &mut Cx, size: (u32, u32), format: RenderTargetFormat) -> bool {
        if self.size == size && self.format == format {
            return false;
        }
        let fixed = TextureSize::Fixed { width: size.0.max(1) as usize, height: size.1.max(1) as usize };
        self.texture = Texture::new_with_format(cx, match format {
            RenderTargetFormat::Bgra8 => TextureFormat::RenderBGRAu8 { size: fixed, initial: true },
            RenderTargetFormat::Rgba16f => TextureFormat::RenderRGBAf16 { size: fixed, initial: true },
        });
        self.depth = None;
        self.size = size;
        self.format = format;
        true
    }

    /// Its depth texture, made at its size on first use (freed with the
    /// colour target).
    pub fn depth(&mut self, cx: &mut Cx) -> Texture {
        let (w, h) = self.size;
        self.depth
            .get_or_insert_with(|| {
                Texture::new_with_format(cx, TextureFormat::DepthD32 {
                    size: TextureSize::Fixed { width: w.max(1) as usize, height: h.max(1) as usize },
                    initial: true,
                })
            })
            .clone()
    }

    /// Give its textures back, the pass's attachments included (the target
    /// stays usable: the next `ensure` sizes it again).
    pub fn release(&mut self, cx: &mut Cx) {
        self.pass.clear_color_textures(cx);
        self.pass.clear_depth_texture(cx);
        self.texture = unsized_texture(cx);
        self.depth = None;
        self.size = (0, 0);
    }

    /// Begin drawing into it, cleared to `clear`, as a child of the pass
    /// being drawn, at its pixel size and dpi 1.
    pub fn begin(&mut self, cx: &mut Cx2d, clear: Vec4f) {
        let size = self.attach(cx, clear);
        cx.begin_root_turtle(size, Layout::flow_overlay());
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }

    /// As [`RenderTarget::begin`], without the pass's clip (a list drawn
    /// through view transforms reaches past the pass rect).
    pub fn begin_unclipped(&mut self, cx: &mut Cx2d, clear: Vec4f) {
        let size = self.attach(cx, clear);
        cx.begin_unclipped_root_turtle(size, Layout::flow_overlay());
    }

    pub fn end_unclipped(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle_no_clip();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }

    fn attach(&mut self, cx: &mut Cx2d, clear: Vec4f) -> Vec2d {
        let size = dvec2(self.size.0 as f64, self.size.1 as f64);
        self.pass.set_size(cx, size);
        self.pass.set_color_texture(cx, &self.texture, DrawPassClearColor::ClearWith(clear));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.list.begin_always(cx);
        size
    }
}

struct Pooled {
    target: RenderTarget,
    /// Taken in the frame being drawn.
    used: bool,
    /// Frames in a row without a use.
    idle: u32,
}

/// Render targets lent out per frame: [`RenderTargetPool::take`] gives a
/// target of a size and format for one use in the frame, and
/// [`RenderTargetPool::next_frame`] frees them all for the next one,
/// releasing the textures of targets idle for `idle_release` frames.
pub struct RenderTargetPool {
    targets: Vec<Pooled>,
    idle_release: u32,
}

impl RenderTargetPool {
    pub fn new(idle_release: u32) -> Self {
        Self { targets: Vec::new(), idle_release }
    }

    /// Start a frame: every target goes free; one unused for
    /// `idle_release` frames gives its textures back.
    pub fn next_frame(&mut self, cx: &mut Cx) {
        for t in &mut self.targets {
            if !t.used {
                t.idle = t.idle.saturating_add(1);
                if t.idle == self.idle_release && t.target.size != (0, 0) {
                    t.target.release(cx);
                }
            }
            t.used = false;
        }
    }

    /// A target of `size` and `format` for one use in this frame: a free
    /// one of that size and format (no new texture), else a released one
    /// or one idle for two frames, resized, else a new one. The same
    /// draws ask for the same sizes every frame, so a steady scene
    /// allocates nothing, and a use that appears or goes does not move
    /// every other use to another target. Returns its index.
    pub fn take(&mut self, cx: &mut Cx, size: (u32, u32), format: RenderTargetFormat, name: &str) -> usize {
        let free = |t: &Pooled| !t.used;
        let index = match self.targets.iter().position(|t| free(t) && t.target.size == size && t.target.format == format) {
            Some(i) => i,
            // Not one this or the last frame used at another size (likely
            // wanted at that size again next frame).
            None => match self
                .targets
                .iter()
                .position(|t| free(t) && t.target.size == (0, 0))
                .or_else(|| self.targets.iter().position(|t| free(t) && t.idle >= 2))
            {
                Some(i) => i,
                None => {
                    self.targets.push(Pooled { target: RenderTarget::new(cx, name), used: false, idle: 0 });
                    self.targets.len() - 1
                }
            },
        };
        let t = &mut self.targets[index];
        t.used = true;
        t.idle = 0;
        t.target.ensure(cx, size, format);
        index
    }

    /// Every target, used or not.
    pub fn iter(&self) -> impl Iterator<Item = &RenderTarget> {
        self.targets.iter().map(|t| &t.target)
    }
}

impl std::ops::Index<usize> for RenderTargetPool {
    type Output = RenderTarget;
    fn index(&self, index: usize) -> &RenderTarget {
        &self.targets[index].target
    }
}

impl std::ops::IndexMut<usize> for RenderTargetPool {
    fn index_mut(&mut self, index: usize) -> &mut RenderTarget {
        &mut self.targets[index].target
    }
}
