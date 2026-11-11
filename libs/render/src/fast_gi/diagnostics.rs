//! On-demand GPU state counts. Nothing here runs in the per-frame GI loop.
use super::*;
use makepad_platform::{ReadbackError, ReadbackRequest, ReadbackTicket};

struct CountGpu {
    pass: DrawPass,
    list: DrawList,
    draw: shaders::DrawGiProbeCounts,
    texture: Texture,
}

struct Sample {
    ticket: ReadbackTicket,
    generation: u64,
    frame: u64,
    config: GiConfig,
    scene_key: Option<SceneKey>,
    voxel_jobs: usize,
    /// Eligibility belongs to the field we read, not the later poll's scheduler.
    guard_eligible: bool,
}

#[derive(Default)]
pub(super) struct ProbeCounts {
    gpu: Option<CountGpu>,
    pending: Option<Sample>,
    empty_reported: bool,
}

impl ProbeCounts {
    pub fn release(&mut self, retired: &mut Vec<Texture>) {
        if let Some(gpu) = self.gpu.take() {
            retired.push(gpu.texture);
        }
        // Keep ownership of an accepted ticket until it can be drained. Its
        // old generation can never publish counts or trip the new field's guard.
        self.empty_reported = false;
    }
}

impl FastGi {
    pub fn request_probe_counts(&mut self, cx: &mut CxDraw) -> Result<bool, ReadbackError> {
        if self.mode == GiMode::Off || self.gpu.is_none() || !cx.inside_pass() {
            return Err(ReadbackError::NotRendered);
        }
        if self.probe_counts.pending.is_some() {
            if let Some(counts) = &self.probe_counts.gpu {
                // A pipeline deferred beyond the requesting draw may have
                // been orphaned by the next scene recording. Retry only on
                // this explicit request/cadence, keeping the original sample's
                // eligibility (a retry must never promote a warm-up sample).
                cx.make_child_pass(&counts.pass);
                if self.stats.batch > 0 {
                    self.gpu.as_ref().unwrap().stages[3].pass.set_pass_parent(cx.cx, counts.pass.draw_pass_id());
                }
                cx.repaint_pass(counts.pass.draw_pass_id());
            }
            return Ok(false);
        }
        let config = self.config;
        let guard_eligible = self.stats.instances > 0
            && !self.scene_stale
            && self.pending_scene.is_none()
            && self.voxel_job.is_none()
            && self.cascades.len() == config.cascades
            && self.cascades.iter().all(Cascade::full_sweep_issued);
        if self.probe_counts.gpu.is_none() {
            let draw = cx.try_with_vm(shaders::DrawGiProbeCounts::script_new_with_default)
                .ok_or(ReadbackError::NotRendered)?;
            self.probe_counts.gpu = Some(CountGpu {
                pass: DrawPass::new_with_name(cx.cx, "GI probe counts"),
                list: DrawList::new(cx.cx),
                draw,
                texture: Texture::new_with_format(cx.cx, TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Fixed { width: config.cascades, height: 1 },
                    initial: true,
                }),
            });
        }
        let field = self.gpu.as_ref().unwrap();
        let counts = self.probe_counts.gpu.as_mut().unwrap();
        let mut cx2 = Cx2d::new(cx);
        let size = dvec2(config.cascades as f64, 1.0);
        counts.draw.quad.draw_vars.refresh_after_reload(cx2.cx);
        bind_texture(cx2.cx, &mut counts.draw.quad.draw_vars, live_id!(gi_field), &field.field);
        counts.draw.quad.draw_vars.set_uniform(cx2.cx, live_id!(gi_atlas), &config.atlas().map(|v| v as f32));
        counts.draw.quad.draw_vars.set_uniform(cx2.cx, live_id!(gi_count_size), &[config.probe_count() as f32, config.cascades as f32]);
        cx2.make_child_pass(&counts.pass);
        if self.stats.batch > 0 {
            // The readback must observe this frame's last scatter, including
            // the batch that drains the initial queue. Never read the gather.
            field.stages[3].pass.set_pass_parent(cx2.cx, counts.pass.draw_pass_id());
        }
        cx2.begin_pass(&counts.pass, Some(1.0));
        counts.pass.set_size(cx2.cx, size);
        counts.pass.clear_color_textures(cx2.cx);
        // An undrawn reduction is an error, not a plausible empty field.
        counts.pass.set_color_texture(cx2.cx, &counts.texture, DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 1.0, 1.0)));
        counts.list.begin_always(&mut cx2);
        cx2.begin_root_turtle(size, Layout::flow_overlay());
        counts.draw.quad.draw_abs(&mut cx2, Rect { pos: dvec2(0.0, 0.0), size });
        cx2.end_pass_sized_turtle();
        counts.list.end(&mut cx2);
        cx2.end_pass(&counts.pass);
        let ticket = counts.texture.read_back(cx2.cx, ReadbackRequest { next_render: true })?;
        self.probe_counts.pending = Some(Sample {
            ticket, generation: self.generation, frame: self.frame, config, guard_eligible,
            scene_key: self.scene_key, voxel_jobs: self.stats.voxel_jobs,
        });
        Ok(true)
    }

    pub fn poll_probe_counts(&mut self, cx: &mut Cx) -> Result<Option<GiStats>, ReadbackError> {
        let Some(sample) = &self.probe_counts.pending else { return Ok(None) };
        let Some(result) = cx.try_take_texture_readbacks_for(&[sample.ticket]).into_iter().next() else {
            return Ok(None);
        };
        let sample = self.probe_counts.pending.take().unwrap();
        if sample.generation != self.generation {
            return Ok(None);
        }
        if let Some(counts) = &self.probe_counts.gpu {
            if let Some(field) = &self.gpu {
                let scatter = &field.stages[3].pass;
                if let (CxDrawPassParent::DrawPass(parent), CxDrawPassParent::DrawPass(scene)) =
                    (scatter.parent(cx), counts.pass.parent(cx))
                {
                    if parent == counts.pass.draw_pass_id() { scatter.set_pass_parent(cx, scene); }
                }
            }
            counts.pass.withdraw(cx);
        }
        let bytes = result.data?;
        if result.width != sample.config.cascades || result.height != 1 || bytes.len() != result.width * 4 {
            return Err(ReadbackError::InvalidSize);
        }
        let (mut valid, mut invalid, mut untraced) = (0, 0, 0);
        for pixel in bytes.chunks_exact(4) {
            // Ticket pixels are normalized RGBA, on every backend.
            let v = u16::from_le_bytes([pixel[0], pixel[1]]) as usize;
            let i = u16::from_le_bytes([pixel[2], pixel[3]]) as usize;
            let Some(u) = sample.config.probe_count().checked_sub(v + i) else {
                return Err(ReadbackError::Failed);
            };
            valid += v;
            invalid += i;
            untraced += u;
        }
        self.stats.valid_probes = valid;
        self.stats.invalid_probes = invalid;
        self.stats.untraced_probes = untraced;
        self.stats.probe_counts_frame = Some(sample.frame);
        // A delayed pipeline may have sampled after a scroll or scene edit.
        // Such a result is still a count, but cannot prove the old sweep.
        self.report_probe_health(sample.guard_eligible
            && sample.scene_key == self.scene_key
            && sample.voxel_jobs == self.stats.voxel_jobs);
        Ok(Some(self.stats))
    }

    /// Runtime report guard, not a GPU test. Called only for a completed,
    /// successfully decoded sample by the periodic report or an explicit poll.
    fn report_probe_health(&mut self, full_sweep_with_geometry: bool) {
        if full_sweep_with_geometry && self.stats.valid_probes == 0 && !self.probe_counts.empty_reported {
            error!("fast GI: no valid probes after a full sweep: the field is empty");
            self.probe_counts.empty_reported = true;
        }
    }
}
