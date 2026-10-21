//! Offscreen render targets: [`RenderTarget`] (one pass, its draw list and
//! its colour texture, plus a depth texture on demand, reused frame after
//! frame) and [`RenderTargetPool`] (targets lent out per frame by size and
//! format, given back after a run of frames without a use).
//!
//! A pass names its attachments too, so a texture a host lets go of stays
//! alive while a pass still has it attached. [`RenderTarget::release`]
//! drops both, which is what makes an idle target's memory come back. The textures are
//! transient (`Texture::set_transient`): every use renders them again, so the platform also
//! gives their storage back after a few idle seconds, however few frames the owner draws.

use crate::{cx_2d::Cx2d, draw_list_2d::{DrawList2d, DrawListExt}, makepad_platform::*, turtle::Layout};

/// The colour format of a [`RenderTarget`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RenderTargetFormat {
    /// 8-bit BGRA (a display-referred picture).
    #[default]
    Bgra8,
    /// Half-float RGBA (a linear, HDR picture).
    Rgba16f,
    /// 8-bit BGRA another GPU user reads by its platform handle
    /// (`Cx::shared_texture_handle`: an encoder taking the picture as it
    /// lies in GPU memory). Kept, never transient.
    SharedBgra8,
}

/// One offscreen pass, its draw list and its colour target.
pub struct RenderTarget {
    pub pass: DrawPass,
    pub list: DrawList2d,
    pub texture: Texture,
    depth: Option<Texture>,
    size: (u32, u32),
    format: RenderTargetFormat,
    /// Its owner in the GPU ledger (`gpu top`).
    name: &'static str,
}

fn unsized_texture(cx: &mut Cx) -> Texture {
    Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true })
}

impl RenderTarget {
    /// A target with no pixels yet (sized by [`RenderTarget::ensure`]).
    pub fn new(cx: &mut Cx, name: &'static str) -> Self {
        Self {
            pass: DrawPass::new_with_name(cx, name),
            list: DrawList2d::new(cx),
            texture: unsized_texture(cx),
            depth: None,
            size: (0, 0),
            format: RenderTargetFormat::Bgra8,
            name,
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
        self.texture = match format {
            RenderTargetFormat::Bgra8 => Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: fixed, initial: true }),
            RenderTargetFormat::Rgba16f => Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: fixed, initial: true }),
            #[cfg(not(target_arch = "wasm32"))]
            RenderTargetFormat::SharedBgra8 => cx.create_shared_texture(size.0.max(1) as usize, size.1.max(1) as usize),
            // the browser shares no textures with another renderer: a plain target
            #[cfg(target_arch = "wasm32")]
            RenderTargetFormat::SharedBgra8 => Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: fixed, initial: true }),
        }.label(cx, self.name);
        if format != RenderTargetFormat::SharedBgra8 {
            self.texture.set_transient(cx, true);
        }
        self.depth = None;
        self.size = size;
        self.format = format;
        true
    }

    /// As [`RenderTarget::ensure`], for a target that keeps its picture
    /// between frames (a cache a host draws into now and then and samples
    /// in the frames between): its texture is not transient, so the platform
    /// never gives its storage back for idleness while the picture is still
    /// wanted (a transient one comes back empty, and a pass that is not
    /// recorded again never paints it).
    pub fn ensure_kept(&mut self, cx: &mut Cx, size: (u32, u32), format: RenderTargetFormat) -> bool {
        let reallocated = self.ensure(cx, size, format);
        if reallocated {
            self.texture.set_transient(cx, false);
        }
        reallocated
    }

    /// Its depth texture, made at its size on first use (freed with the
    /// colour target).
    pub fn depth(&mut self, cx: &mut Cx) -> Texture {
        let (w, h) = self.size;
        self.depth
            .get_or_insert_with(|| {
                let depth = Texture::new_with_format(cx, TextureFormat::DepthD32 {
                    size: TextureSize::Fixed { width: w.max(1) as usize, height: h.max(1) as usize },
                    initial: true,
                }).label(cx, self.name);
                depth.set_transient(cx, true);
                depth
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
    pub fn take(&mut self, cx: &mut Cx, size: (u32, u32), format: RenderTargetFormat, name: &'static str) -> usize {
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

    /// Gives every target's textures back now (its owner stopped drawing for good measure).
    pub fn release_all(&mut self, cx: &mut Cx) {
        for t in &mut self.targets {
            if t.target.size != (0, 0) {
                t.target.release(cx);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_target_is_not_transient_and_a_frame_target_is() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut frame = RenderTarget::new(&mut cx, "frame");
        let mut kept = RenderTarget::new(&mut cx, "kept");
        assert!(frame.ensure(&mut cx, (8, 8), RenderTargetFormat::Bgra8));
        assert!(kept.ensure_kept(&mut cx, (8, 8), RenderTargetFormat::Bgra8));
        assert!(frame.texture.is_transient(&cx), "a per-frame target is rebuilt by every use");
        assert!(!kept.texture.is_transient(&cx), "a kept picture is sampled in frames that do not draw it");
        // The same size again keeps the texture (and its state).
        assert!(!kept.ensure_kept(&mut cx, (8, 8), RenderTargetFormat::Bgra8));
        assert!(!kept.texture.is_transient(&cx));
        // A new size allocates anew: still kept.
        assert!(kept.ensure_kept(&mut cx, (16, 8), RenderTargetFormat::Bgra8));
        assert!(!kept.texture.is_transient(&cx));
    }
}
