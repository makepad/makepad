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
use crate::texture::Texture;
#[cfg(not(target_arch = "wasm32"))]
use crate::texture::TextureId;
#[cfg(not(target_arch = "wasm32"))]
use crate::{draw_list::DrawListId, draw_pass::DrawPassId};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;

/// The draws of a mesh's pick repaint: `(first_index, count, id)` covering `0..index_count` in
/// order. Indices before the first run, and runs with id 0, take `call_id`. Starts are clamped
/// to `index_count` and to the previous start, so a bad table never draws out of range; empty
/// runs are skipped and neighbours with the same resolved id merge, so the counts sum to
/// `index_count`.
pub fn pick_runs(ranges: &[[u32; 2]], index_count: u32, call_id: u32) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
    let mut ranges = ranges.iter();
    let mut first = 0;
    let mut id = call_id;
    let mut pending: Option<(u32, u32, u32)> = None;
    std::iter::from_fn(move || loop {
        if first == index_count {
            return pending.take();
        }
        let (end, next_id) = match ranges.next() {
            Some(&[start, id]) => (start.min(index_count).max(first), if id == 0 { call_id } else { id }),
            None => (index_count, call_id),
        };
        let run = (first, end - first, id);
        first = end;
        id = next_id;
        if run.1 == 0 {
            continue;
        }
        match pending.as_mut() {
            Some(prev) if prev.2 == run.2 => prev.1 += run.1,
            Some(_) => return pending.replace(run),
            None => pending = Some(run),
        }
    })
}

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
    /// RGBA8, the id in red (low), green and blue (high) bytes.
    pub bytes: crate::HostBuf,
}

impl PickImage {
    /// The id at pixel `x`, `y` (0 outside the image).
    pub fn id_at(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let at = (y * self.width + x) * 4;
        let Some(p) = self.bytes.get(at..at + 4) else {
            return 0;
        };
        p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16
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

#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    pub requests: Vec<PickRequest>,
    /// Each render target's pick twin (and a depth twin for a pass with
    /// depth), by the target's texture.
    #[cfg(not(target_arch = "wasm32"))]
    pub twins: HashMap<TextureId, Texture>,
    pub results: Vec<(PickTicket, Result<PickImage, PickError>)>,
    /// While a pick repaint draws.
    #[cfg(not(target_arch = "wasm32"))]
    pub paint: Option<PickPaint>,
}

/// A pick repaint in progress.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct PickPaint {
    /// The targets whose twins hold this paint's ids.
    pub painted: std::collections::HashSet<TextureId>,
}

#[cfg(not(target_arch = "wasm32"))]
impl CxPick {
    /// The twin of render target `of` (created on first use): an 8-bit
    /// target of the same size, or a depth target for a depth texture.
    pub fn twin(
        &mut self,
        textures: &mut crate::texture::CxTexturePool,
        of: TextureId,
        depth: bool,
    ) -> Texture {
        if let Some(t) = self.twins.get(&of) {
            return t.clone();
        }
        use crate::texture::{TextureFormat, TextureSize};
        let format = if depth {
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            }
        } else {
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            }
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !self.pick.variants {
                self.pick.results.push((ticket, Err(PickError::NoVariants)));
                return ticket;
            }
            let id = texture.texture_id();
            let producer = self.passes.id_iter().find(|pass| {
                !self.passes.0.is_free(pass.0)
                    && self.passes[*pass]
                        .color_textures
                        .iter()
                        .any(|c| c.texture.texture_id() == id)
            });
            let Some(producer) = producer else {
                self.pick
                    .results
                    .push((ticket, Err(PickError::NotRendered)));
                return ticket;
            };
            self.pick.requests.push(PickRequest {
                ticket,
                texture: texture.clone(),
            });
            self.repaint_pass(producer);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = texture;
            self.pick
                .results
                .push((ticket, Err(PickError::Unsupported)));
        }
        ticket
    }

    /// The pick's image once it is back.
    pub fn take_pick(&mut self, ticket: PickTicket) -> Option<Result<PickImage, PickError>> {
        let at = self.pick.results.iter().position(|(t, _)| *t == ticket)?;
        Some(self.pick.results.remove(at).1)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Cx {
    /// The draw lists a pass draws, its sub-lists too.
    pub(crate) fn pick_lists(&self, list: DrawListId, out: &mut Vec<DrawListId>) {
        if out.contains(&list) || self.draw_lists.is_id_freed(list) {
            return;
        }
        out.push(list);
        let len = self.draw_lists[list].draw_item_order_len();
        for order in 0..len {
            let Some(item) = self.draw_lists[list].draw_item_id_at_order_index(order) else {
                continue;
            };
            if let Some(sub) = self.draw_lists[list].draw_items[item].kind.sub_list() {
                self.pick_lists(sub, out);
            }
        }
    }

    /// `pass` after the passes it samples (depth first): what feeds it.
    pub(crate) fn pick_feeders(
        &self,
        pass: DrawPassId,
        out: &mut Vec<DrawPassId>,
        visiting: &mut Vec<DrawPassId>,
    ) {
        if out.contains(&pass) || visiting.contains(&pass) || self.passes.0.is_free(pass.0) {
            return;
        }
        let Some(list) = self.passes[pass].main_draw_list_id else {
            return;
        };
        visiting.push(pass);
        let mut lists = Vec::new();
        self.pick_lists(list, &mut lists);
        let mut sampled: Vec<crate::texture::TextureId> = Vec::new();
        for list in &lists {
            let items = &self.draw_lists[*list].draw_items;
            for i in 0..items.len() {
                let Some(call) = items[i].kind.draw_call() else {
                    continue;
                };
                for t in call.texture_slots.iter().flatten() {
                    if !sampled.contains(&t.texture_id()) {
                        sampled.push(t.texture_id());
                    }
                }
            }
        }
        for texture in sampled {
            let producer = self.passes.id_iter().find(|p| {
                !self.passes.0.is_free(p.0)
                    && self.passes[*p]
                        .color_textures
                        .iter()
                        .any(|c| c.texture.texture_id() == texture)
            });
            if let Some(producer) = producer {
                self.pick_feeders(producer, out, visiting);
            }
        }
        visiting.pop();
        if !self.passes[pass].color_textures.is_empty() {
            out.push(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick_runs;

    #[test]
    fn pick_runs_cover_the_mesh_once() {
        let runs: Vec<_> = pick_runs(&[[3, 5], [6, 0], [9, 9], [12, 7], [12, 4], [25, 4], [40, 2]], 30, 9).collect();
        assert_eq!(runs, vec![(0, 3, 9), (3, 3, 5), (6, 6, 9), (12, 18, 4)],
            "leading gaps and id 0 take the call id, equal neighbours merge, empty runs vanish, and starts past the end clamp away");
        assert_eq!(runs.iter().map(|run| run.1).sum::<u32>(), 30, "run counts cover the mesh exactly once");
        assert_eq!(pick_runs(&[[10, 1], [4, 2]], 20, 3).collect::<Vec<_>>(), vec![(0, 10, 3), (10, 10, 2)],
            "descending starts clamp to the previous start");
        assert_eq!(pick_runs(&[], 12, 3).collect::<Vec<_>>(), vec![(0, 12, 3)],
            "an empty table picks the whole mesh as the call id");
        assert!(pick_runs(&[], 0, 3).collect::<Vec<_>>().is_empty(), "an empty mesh emits no runs");
    }
}
