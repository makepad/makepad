//! Ordered backdrop checkpoints for a compositor. Disjoint glass surfaces share
//! a Gaussian pyramid; only intervening content in a sampling footprint creates
//! another one. Blur work is deferred until all requested levels are known.
use crate::{
    gauss_stack::GaussStack,
    gauss_view::{GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_draw::*,
    window::{DrawGaussDownsample, DrawGaussScene, DrawGaussUpsample},
};

fn overlaps(a: Rect, b: Rect) -> bool {
    a.pos.x < b.pos.x + b.size.x
        && a.pos.x + a.size.x > b.pos.x
        && a.pos.y < b.pos.y + b.size.y
        && a.pos.y + a.size.y > b.pos.y
}
fn inflate(r: Rect, by: f64) -> Rect {
    Rect { pos: r.pos - dvec2(by, by), size: r.size + dvec2(2.0 * by, 2.0 * by) }
}
/// How far past a glass surface's rect its blur reads: the downsample kernel,
/// smooth upsampling and bicubic taps, and a lens's displacement.
fn blur_support(level: f64, reach: f64) -> f64 {
    2.0f64.powf(level.ceil() + 2.0) + reach.max(0.0)
}

#[derive(Default)]
struct BackdropReuse {
    source: Option<usize>,
    covered: Vec<Rect>,
}
impl BackdropReuse {
    fn needs_checkpoint(&self, rect: Rect, level: f64, reach: f64) -> bool {
        // Include the downsample kernel, smooth upsampling and bicubic taps —
        // and how far a lens reads OUTSIDE its rect (the peak displacement
        // plus any chroma), or content moving just past the rim would change
        // what the glass shows without a new checkpoint.
        let sample = inflate(rect, blur_support(level, reach));
        self.source.is_none() || self.covered.iter().any(|r| overlaps(*r, sample))
    }
    fn checkpoint(&mut self, source: usize) {
        self.source = Some(source);
        self.covered.clear();
    }
}

pub struct BackdropCompositor {
    stacks: Vec<GaussStack>,
    levels: Vec<usize>,
    content_passes: Vec<(usize, DrawPassId)>,
    current: usize,
    size: Vec2d,
    reuse: BackdropReuse,
    downsample: DrawGaussDownsample,
    upsample: DrawGaussUpsample,
    scene: DrawGaussScene,
}
impl BackdropCompositor {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            stacks: Vec::new(),
            levels: Vec::new(),
            content_passes: Vec::new(),
            current: 0,
            size: dvec2(0.0, 0.0),
            reuse: BackdropReuse::default(),
            downsample: cx.with_vm(|vm| DrawGaussDownsample::script_new_with_default(vm)),
            upsample: cx.with_vm(|vm| DrawGaussUpsample::script_new_with_default(vm)),
            scene: cx.with_vm(|vm| DrawGaussScene::script_new_with_default(vm)),
        }
    }

    /// The pass the desk scene is recorded into.
    pub fn scene_pass_id(&self) -> Option<DrawPassId> {
        self.stacks.first().map(|stack| stack.scene_pass_id())
    }
    /// Draw all layers in window-local coordinates, including in a cached View.
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.size = cx.owning_window_or_root_pass_size();
        self.current = 0;
        self.levels.clear();
        self.content_passes.clear();
        self.reuse = BackdropReuse::default();
        self.begin_segment(cx);
    }
    fn begin_segment(&mut self, cx: &mut Cx2d) {
        if self.stacks.len() <= self.current {
            self.stacks.push(GaussStack::new(cx));
        }
        self.levels.push(0);
        self.stacks[self.current].begin_scene_sized(cx, self.size);
        if self.current > 0 {
            self.stacks[self.current - 1].draw_scene(cx, &mut self.scene, self.size);
        }
    }
    /// Record every composited object's coverage, including opaque windows.
    pub fn content(&mut self, rect: Rect) {
        self.reuse.covered.push(rect);
    }
    /// A window capture samples the preceding backdrop and is consumed by the
    /// current scene. Include it between those passes in the dependency chain.
    pub fn content_pass(&mut self, pass: DrawPassId) {
        self.content_passes.push((self.current, pass));
    }

    fn snapshot(&mut self, cx: &Cx2d, source: usize, level: f64) -> GaussBlurSnapshot {
        // The shader samples floor(level) and floor(level)+1, even at integer levels.
        let count = (level.floor() as usize + 1).min(GAUSS_VIEW_LEVELS);
        self.levels[source] = self.levels[source].max(count);
        self.stacks[source].snapshot(
            self.size,
            0.0,
            cx.current_dpi_factor(),
        )
    }

    pub fn backdrop(&mut self, cx: &mut Cx2d, rect: Rect, level: f64) -> GaussBlurSnapshot {
        self.backdrop_with_reach(cx, rect, level, 0.0)
    }

    /// A backdrop for a LENSED surface: `reach` is how far outside `rect`
    /// its shader samples (the peak displacement plus chroma), so the reuse
    /// footprint covers what the lens draws in from around the slab.
    pub fn backdrop_with_reach(&mut self, cx: &mut Cx2d, rect: Rect, level: f64, reach: f64) -> GaussBlurSnapshot {
        if self.reuse.needs_checkpoint(rect, level, reach) {
            self.stacks[self.current].end_scene(cx);
            self.reuse.checkpoint(self.current);
            self.current += 1;
            self.begin_segment(cx);
        }
        self.snapshot(cx, self.reuse.source.unwrap(), level)
    }

    /// Finish the desktop, optionally supplying the dock's backdrop. No extra
    /// scene copy is necessary for this last consumer. Returns stack/pass counts
    /// for lightweight diagnostics as well as the snapshot.
    pub fn finish(
        &mut self,
        cx: &mut Cx2d,
        output_rect: Rect,
        final_glass: Option<(Rect, f64)>,
    ) -> (Option<GaussBlurSnapshot>, usize, usize) {
        self.stacks[self.current].end_scene(cx);
        let backdrop = final_glass.map(|(rect, level)| {
            if self.reuse.needs_checkpoint(rect, level, 0.0) {
                self.reuse.checkpoint(self.current);
            }
            self.snapshot(cx, self.reuse.source.unwrap(), level)
        });
        let mut dependencies = Vec::new();
        let mut blur_stacks = 0;
        for index in 0..=self.current {
            dependencies.extend(self.content_passes.iter().filter(|(segment,_)| *segment==index).map(|(_,pass)| *pass));
            let count = self.levels[index];
            if count > 0 {
                blur_stacks += 1;
                self.stacks[index].draw_mip_chain_to(cx, &mut self.downsample, self.size, count);
                self.stacks[index].draw_high_blur_chain_to(
                    cx,
                    &mut self.upsample,
                    self.size,
                    count,
                );
            }
            dependencies.extend(self.stacks[index].dependencies(count));
        }
        // Each pass has one consumer link. Linearizing the actual work also
        // represents sources with multiple readers, without depending on pool IDs.
        for pair in dependencies.windows(2) {
            let consumer = pair[1];
            let owner = cx.passes[consumer].main_draw_list_id;
            cx.attach_child_pass(pair[0], consumer, owner);
        }
        self.stacks[self.current].draw_scene_region(cx, &mut self.scene, self.size, output_rect);
        // Free stacks no longer required by this scene instead of retaining the
        // largest overlap depth ever visited. Live snapshots retain texture handles.
        self.stacks.truncate(self.current + 1);
        (backdrop, blur_stacks, dependencies.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }
    #[test]
    fn disjoint_surfaces_reuse_their_backdrop_but_overlaps_checkpoint() {
        let mut reuse = BackdropReuse::default();
        assert!(reuse.needs_checkpoint(rect(0.0, 0.0, 200.0, 200.0), 3.0, 0.0));
        reuse.checkpoint(0);
        reuse.covered.push(rect(0.0, 0.0, 200.0, 200.0));
        assert!(!reuse.needs_checkpoint(rect(400.0, 0.0, 200.0, 200.0), 3.0, 0.0));
        assert!(reuse.needs_checkpoint(rect(150.0, 0.0, 200.0, 200.0), 3.0, 0.0));
        reuse.checkpoint(1);
        assert!(!reuse.needs_checkpoint(rect(150.0, 0.0, 200.0, 200.0), 3.0, 0.0));
    }
    #[test]
    fn blur_support_and_opaque_intervening_content_count() {
        let mut reuse = BackdropReuse::default();
        reuse.checkpoint(0);
        reuse.covered.push(rect(100.0, 100.0, 200.0, 200.0));
        assert!(reuse.needs_checkpoint(rect(320.0, 100.0, 100.0, 100.0), 3.0, 0.0));
        assert!(!reuse.needs_checkpoint(rect(380.0, 100.0, 100.0, 100.0), 3.0, 0.0));
        assert!(reuse.needs_checkpoint(rect(380.0, 100.0, 100.0, 100.0), 4.5, 0.0));
    }
    #[test]
    fn a_lens_reaches_past_its_rect_by_its_displacement() {
        let mut reuse = BackdropReuse::default();
        reuse.checkpoint(0);
        reuse.covered.push(rect(100.0, 100.0, 200.0, 200.0));
        // Level 1 (mip0): 8 pt of blur support. Content 10 pt beyond that is
        // out of a frosted surface's footprint, but a 10 pt lens reads it.
        assert!(!reuse.needs_checkpoint(rect(318.0, 100.0, 100.0, 100.0), 1.0, 0.0));
        assert!(reuse.needs_checkpoint(rect(318.0, 100.0, 100.0, 100.0), 1.0, 10.25));
    }
}
