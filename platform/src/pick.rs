//! Pick ids: which draw made each pixel of a picture, through every pass
//! that resamples it.
//!
//! An editor that maps a click on a picture to the code that drew it
//! cannot hit-test what it recorded once a pass has warped the picture (a
//! lens, a displacement, a picture on a 3D surface). Here the renderer
//! answers instead. With pick variants on ([`Cx::enable_pick_variants`],
//! before shaders compile) every draw shader also compiles a pick variant
//! (`ShaderOutput::pick`), and a host that labels its draws
//! ([`Cx::set_pick_ids`], [`Cx::set_pick_id`]: each draw call carries the
//! id current when it was made) can ask for the picture's id image
//! ([`Cx::request_pick`]). The next repaint draws every pass that feeds
//! the picture once more with the pick variants into pick twins of their
//! targets (8-bit, the id in the colour, nothing blended): a draw writes
//! its id where it covers, and a draw that samples another target reads
//! that target's twin at the same place and passes on the id of the most
//! opaque thing it sampled. The id image comes back from the GPU as
//! [`PickImage`] ([`Cx::take_pick`]). Nothing of this runs, and nothing a
//! frame draws changes, until a pick is asked for; with ids off draw calls
//! batch exactly as without the feature.

use crate::cx::Cx;
use crate::texture::{Texture, TextureId};
use std::collections::HashMap;

/// A pick asked for ([`Cx::request_pick`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PickTicket(pub u64);

/// Why a pick gave no image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickError {
    /// This backend draws no pick variants.
    Unsupported,
    /// Pick variants were not on when the shaders compiled.
    NoVariants,
    /// No pass draws the texture.
    NotRendered,
    /// The GPU did not give the image back.
    Failed,
}

/// What drew each pixel of a picture: an id per pixel, 0 where nothing did.
#[derive(Clone, Debug)]
pub struct PickImage {
    pub width: usize,
    pub height: usize,
    /// BGRA8, the id in blue (high), green and red (low) bytes.
    pub bytes: Vec<u8>,
}

impl PickImage {
    /// The id at pixel `x`, `y` (0 outside the image).
    pub fn id_at(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let at = (y * self.width + x) * 4;
        let Some(p) = self.bytes.get(at..at + 4) else { return 0 };
        p[2] as u32 | (p[1] as u32) << 8 | (p[0] as u32) << 16
    }

    /// The ids within `r` pixels of `x`, `y`, nearest first, each once.
    pub fn ids_near(&self, x: usize, y: usize, r: usize) -> Vec<u32> {
        let mut found: Vec<(usize, u32)> = Vec::new();
        let r = r as isize;
        for dy in -r..=r {
            for dx in -r..=r {
                let (px, py) = (x as isize + dx, y as isize + dy);
                if px < 0 || py < 0 {
                    continue;
                }
                let id = self.id_at(px as usize, py as usize);
                if id != 0 {
                    found.push(((dx * dx + dy * dy) as usize, id));
                }
            }
        }
        found.sort();
        let mut out: Vec<u32> = Vec::new();
        for (_, id) in found {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

pub(crate) struct PickRequest {
    pub ticket: PickTicket,
    pub texture: Texture,
}

#[derive(Default)]
pub(crate) struct CxPick {
    /// Shaders compile pick variants.
    pub variants: bool,
    /// Draw calls carry ids (batching splits where the id changes).
    pub ids_on: bool,
    /// The id draw calls made now carry.
    pub id: u32,
    pub next_ticket: u64,
    pub requests: Vec<PickRequest>,
    /// Each render target's pick twin (and a depth twin for a pass with
    /// depth), by the target's texture.
    pub twins: HashMap<TextureId, Texture>,
    pub results: Vec<(PickTicket, Result<PickImage, PickError>)>,
    /// While a pick repaint draws.
    pub paint: Option<PickPaint>,
}

/// A pick repaint in progress.
pub(crate) struct PickPaint {
    pub ticket: PickTicket,
    /// The pass that draws the picked texture (drawn last).
    pub target: crate::draw_pass::DrawPassId,
    pub texture: TextureId,
    /// The targets whose twins hold this paint's ids.
    pub painted: std::collections::HashSet<TextureId>,
    /// Draws whose pick pipeline is still compiling: the image would miss
    /// them, so it is drawn again on a later repaint.
    pub not_ready: usize,
}

impl CxPick {
    /// The twin of render target `of` (created on first use): an 8-bit
    /// target of the same size, or a depth target for a depth texture.
    pub fn twin(&mut self, textures: &mut crate::texture::CxTexturePool, of: TextureId, depth: bool) -> Texture {
        if let Some(t) = self.twins.get(&of) {
            return t.clone();
        }
        use crate::texture::{TextureFormat, TextureSize};
        let format = if depth {
            TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true }
        } else {
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true }
        };
        let t = textures.alloc(format);
        self.twins.insert(of, t.clone());
        t
    }
}

impl Cx {
    /// Compile a pick variant of every draw shader from now on (an
    /// editor's process, before its first draw). Off, shaders compile as
    /// without the feature.
    pub fn enable_pick_variants(&mut self) {
        if self.pick.variants {
            return;
        }
        self.pick.variants = true;
        // Shaders compiled before have no pick variant: a draw made from
        // now on compiles its own.
        self.draw_shaders.cache_object_id_to_shader.clear();
        self.draw_shaders.cache_functions_to_shader.clear();
        self.draw_shaders.cache_code_to_shader.clear();
    }

    pub fn pick_variants(&self) -> bool {
        self.pick.variants
    }

    /// Draw calls carry the current pick id ([`Self::set_pick_id`]) while
    /// on: an editor records with it on, so a pick names its draws.
    pub fn set_pick_ids(&mut self, on: bool) {
        self.pick.ids_on = on;
        if !on {
            self.pick.id = 0;
        }
    }

    pub fn pick_ids_on(&self) -> bool {
        self.pick.ids_on
    }

    /// The id the draws made from now on carry (0: none of their own;
    /// they pass on what they sample).
    pub fn set_pick_id(&mut self, id: u32) {
        if self.pick.ids_on {
            self.pick.id = id & 0xff_ffff;
        }
    }

    pub fn pick_id(&self) -> u32 {
        self.pick.id
    }

    /// The draw-call id a call made now carries, `None` while ids are off
    /// (batching as without the feature).
    pub fn pick_target(&self) -> Option<u32> {
        self.pick.ids_on.then_some(self.pick.id)
    }

    /// Ask for the id image of `texture` (a render target) as drawn now:
    /// the next repaint draws it with the pick variants. Poll
    /// [`Self::take_pick`].
    pub fn request_pick(&mut self, texture: &Texture) -> PickTicket {
        self.pick.next_ticket += 1;
        let ticket = PickTicket(self.pick.next_ticket);
        if !cfg!(target_os = "macos") {
            self.pick.results.push((ticket, Err(PickError::Unsupported)));
            return ticket;
        }
        if !self.pick.variants {
            self.pick.results.push((ticket, Err(PickError::NoVariants)));
            return ticket;
        }
        let id = texture.texture_id();
        let producer = self.passes.id_iter().find(|pass| {
            !self.passes.0.is_free(pass.0) && self.passes[*pass].color_textures.iter().any(|c| c.texture.texture_id() == id)
        });
        let Some(producer) = producer else {
            self.pick.results.push((ticket, Err(PickError::NotRendered)));
            return ticket;
        };
        self.pick.requests.push(PickRequest { ticket, texture: texture.clone() });
        // (A repaint to draw it in.)
        self.repaint_pass(producer);
        ticket
    }

    /// The pick's image once it is back.
    pub fn take_pick(&mut self, ticket: PickTicket) -> Option<Result<PickImage, PickError>> {
        #[cfg(target_os = "macos")]
        self.poll_pick_results();
        let at = self.pick.results.iter().position(|(t, _)| *t == ticket)?;
        Some(self.pick.results.remove(at).1)
    }
}
