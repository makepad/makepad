//! Renderer replacement owns both devices until the host explicitly retires
//! the previous generation. Display ownership is never transplanted between
//! VkDevices, including two logical devices on the same physical GPU.
use super::*;
use super::{desktop::PreparedOutputRoute, migrate::ResourceMigration};
use crate::linux_gpu::{LinuxGpuDevice, LinuxGpuPhase, LinuxGpuSnapshot};

#[derive(Default)]
pub(super) struct GpuState {
    pub devices: Vec<LinuxGpuDevice>,
    pub device: LinuxGpuDevice,
    pub generation: u64,
    last_transition: u64,
    phase: LinuxGpuPhase,
    copied_bytes: u64,
    error: Option<String>,
    pending: Option<Transition>,
    retired: Option<RetiredRenderer>,
}

enum Stage {
    Quiescing,
    Copying(ResourceMigration),
    Prepared {
        target: Box<CxVulkan>,
        output: Option<PreparedOutputRoute>,
    },
}

struct Transition {
    id: u64,
    generation: u64,
    target: [u8; 16],
    cancel: bool,
    commit: bool,
    stage: Stage,
}

pub(super) enum RetiredRenderer {
    Separate(Box<CxVulkan>),
    /// This renderer is now owned exactly once, by routed.display. Only its
    /// app caches retire; its display resources and device remain in use.
    PresenterCache,
}

impl CxVulkan {
    pub(crate) fn gpu_snapshot(&self) -> LinuxGpuSnapshot {
        let state = &self.desktop.gpu;
        LinuxGpuSnapshot {
            devices: state.devices.clone(),
            renderer: Some(state.device.clone()),
            display: self.gpu_display_identity(),
            renderer_generation: state.generation,
            transition: state.last_transition,
            phase: state.phase,
            copied_bytes: state.copied_bytes,
            error: state.error.clone(),
        }
    }

    pub(crate) fn gpu_transition_pending(&self) -> bool {
        self.desktop.gpu.pending.is_some()
    }

    pub(super) fn export_transition_image(
        &mut self,
        texture: &crate::texture::Texture,
        width: u32,
        height: u32,
    ) -> Result<crate::os::shared_framebuf::LinuxOwnedImage, String> {
        match self.desktop.gpu.pending.as_mut() {
            Some(Transition {
                stage: Stage::Prepared { target, .. },
                ..
            }) => target.export_shared_image(texture, width, height),
            Some(_) => Err("GPU transition has not prepared framebuffer exports".into()),
            None => self.export_shared_image(texture, width, height),
        }
    }

    pub(crate) fn import_transition_image(
        &mut self,
        id: u64,
        texture: &crate::texture::Texture,
        width: u32,
        height: u32,
        image: crate::os::shared_framebuf::LinuxOwnedImage,
    ) -> Result<(), String> {
        if self.desktop.gpu.phase != LinuxGpuPhase::Prepared {
            return Err("GPU candidate is not prepared".into());
        }
        match self.desktop.gpu.pending.as_mut() {
            Some(Transition {
                id: pending,
                stage: Stage::Prepared { target, .. },
                ..
            }) if *pending == id => target.import_shared_image(texture, width, height, image),
            _ => Err("stale GPU swapchain import".into()),
        }
    }

    pub(crate) fn prepared_gpu_identity(
        &self,
        id: u64,
    ) -> Option<(u64, crate::linux_gpu::LinuxGpuDevice)> {
        match self.desktop.gpu.pending.as_ref() {
            Some(Transition {
                id: pending,
                generation,
                stage: Stage::Prepared { target, .. },
                ..
            }) if *pending == id && self.desktop.gpu.phase == LinuxGpuPhase::Prepared => {
                Some((*generation, target.desktop.gpu.device.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn prepared_imports_ready(&mut self, id: u64) -> Result<bool, String> {
        match self.desktop.gpu.pending.as_mut() {
            Some(Transition {
                id: pending,
                stage: Stage::Prepared { target, .. },
                ..
            }) if *pending == id => target.hosted_routes_ready(),
            _ => Err("GPU import candidate is not prepared".into()),
        }
    }

    pub(crate) fn begin_gpu_transition(
        &mut self,
        id: u64,
        generation: u64,
        target: [u8; 16],
    ) -> Result<(), String> {
        let state = &mut self.desktop.gpu;
        if state.pending.is_some() || state.retired.is_some() {
            return Err("the previous GPU transition has not retired".into());
        }
        if id == 0 || id <= state.last_transition || generation <= state.generation {
            return Err("stale GPU transition or renderer generation".into());
        }
        if !state.devices.iter().any(|device| device.uuid == target) {
            return Err("the requested Vulkan device is unavailable".into());
        }
        state.last_transition = id;
        state.phase = LinuxGpuPhase::Quiescing;
        state.error = None;
        state.copied_bytes = 0;
        state.pending = Some(Transition {
            id,
            generation,
            target,
            cancel: false,
            commit: false,
            stage: Stage::Quiescing,
        });
        self.freeze_gpu_output_size(true);
        Ok(())
    }

    pub(crate) fn request_gpu_commit(&mut self, id: u64) -> Result<(), String> {
        if self.desktop.gpu.phase != LinuxGpuPhase::Prepared {
            return Err("GPU transition is not ready to commit".into());
        }
        if !self.prepared_imports_ready(id)? {
            return Err("GPU framebuffer bridges are still initializing".into());
        }
        let transition = self
            .desktop
            .gpu
            .pending
            .as_mut()
            .ok_or("no prepared GPU transition")?;
        if transition.id != id
            || transition.cancel
            || !matches!(transition.stage, Stage::Prepared { .. })
        {
            return Err("GPU transition is not ready to commit".into());
        }
        transition.commit = true;
        Ok(())
    }

    pub(crate) fn request_gpu_cancel(&mut self, id: u64) -> Result<(), String> {
        let transition = self
            .desktop
            .gpu
            .pending
            .as_mut()
            .ok_or("no pending GPU transition")?;
        if transition.id != id {
            return Err("stale GPU cancellation".into());
        }
        transition.cancel = true;
        transition.commit = false;
        Ok(())
    }

    pub(crate) fn retire_gpu_transition(&mut self, id: u64) -> Result<(), String> {
        if self.desktop.gpu.last_transition == id && self.desktop.gpu.phase == LinuxGpuPhase::Idle {
            return Ok(());
        }
        if self.desktop.gpu.last_transition != id
            || self.desktop.gpu.phase != LinuxGpuPhase::Committed
        {
            return Err("GPU transition has not committed".into());
        }
        match self.desktop.gpu.retired.take() {
            Some(RetiredRenderer::Separate(old)) => drop(old),
            Some(RetiredRenderer::PresenterCache) => self.retire_presenter_app_cache(),
            None => {}
        }
        self.desktop.gpu.phase = LinuxGpuPhase::Idle;
        Ok(())
    }

    /// One nonblocking progress step. The caller continues servicing input,
    /// protocol messages and the independent retained display between steps.
    pub(crate) fn advance_gpu_transition(mut self, cx: &mut Cx) -> Self {
        let Some(mut transition) = self.desktop.gpu.pending.take() else {
            return self;
        };
        if let Err(error) = self.poll_transition(cx, &mut transition) {
            crate::error!("Vulkan GPU transition {} failed: {error}", transition.id);
            self.desktop.gpu.error = Some(error);
            transition.cancel = true;
            transition.commit = false;
        }
        if transition.cancel {
            // A queued snapshot owns the shared leases until its fence has
            // completed. Cancellation must not resume normal drawing early.
            let drained = self.poll_shared_cancel(cx).and_then(|ready| {
                if ready {
                    self.discard_ready(&transition)
                } else {
                    Ok(false)
                }
            });
            match drained {
                Ok(true) => {
                    self.discard_transition(transition);
                    match self.resume_shared_frames() {
                        Ok(()) => {
                            self.freeze_gpu_output_size(false);
                            self.desktop.gpu.phase = if self.desktop.gpu.error.is_some() {
                                LinuxGpuPhase::Failed
                            } else {
                                LinuxGpuPhase::Cancelled
                            };
                            cx.redraw_all();
                            return self;
                        }
                        Err(error) => self.desktop.gpu.error = Some(error),
                    }
                    // A failed fence restoration cannot safely resume. Leave
                    // a frozen terminal state instead of drawing on it.
                    transition = Transition {
                        id: self.desktop.gpu.last_transition,
                        generation: self.desktop.gpu.generation + 1,
                        target: self.desktop.gpu.device.uuid,
                        cancel: true,
                        commit: false,
                        stage: Stage::Quiescing,
                    };
                }
                Ok(false) => {}
                Err(error) => self.desktop.gpu.error = Some(error),
            }
        } else if transition.commit {
            if let Stage::Prepared { mut target, output } = transition.stage {
                target.desktop.gpu.last_transition = transition.id;
                target.desktop.gpu.generation = transition.generation;
                target.desktop.gpu.phase = LinuxGpuPhase::Committed;
                target.desktop.gpu.copied_bytes = self.desktop.gpu.copied_bytes;
                let retired = if let Some(output) = output {
                    target.install_gpu_output(self, output)
                } else {
                    Some(RetiredRenderer::Separate(Box::new(self)))
                };
                target.desktop.gpu.retired = retired;
                // New app launches inherit the committed compositor device.
                if target.gpu_display_identity().is_some() {
                    let value: String = target
                        .desktop
                        .gpu
                        .device
                        .uuid
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
                    std::env::set_var(crate::linux_display::LINUX_VULKAN_DEVICE_ENV, value);
                }
                cx.redraw_all();
                return *target;
            }
        }
        self.desktop.gpu.pending = Some(transition);
        self
    }

    fn poll_transition(&mut self, cx: &mut Cx, transition: &mut Transition) -> Result<(), String> {
        if transition.cancel {
            return Ok(());
        }
        match &mut transition.stage {
            Stage::Quiescing => {
                if !self.hosted_routes_idle()? {
                    return Ok(());
                }
                if !self.quiesce_shared_frames(cx)? || !self.gpu_output_idle()? {
                    return Ok(());
                }
                let target = Box::new(Self::new_offscreen_on(Some(transition.target))?);
                let copy = unsafe { ResourceMigration::prepare(self, cx, target) }?;
                transition.stage = Stage::Copying(copy);
                self.desktop.gpu.phase = LinuxGpuPhase::Copying;
            }
            Stage::Copying(copy) => {
                let done = unsafe { copy.poll(self) }?;
                self.desktop.gpu.copied_bytes = copy.copied_bytes;
                if done {
                    let Stage::Copying(copy) =
                        std::mem::replace(&mut transition.stage, Stage::Quiescing)
                    else {
                        unreachable!()
                    };
                    let mut target = unsafe { copy.finish(self) };
                    let output = target.prepare_gpu_output(self)?;
                    transition.stage = Stage::Prepared { target, output };
                    // The presentation bridge has its own initialization
                    // fence; readiness is published after that completes too.
                }
            }
            Stage::Prepared { target, output } => {
                if let Some(output) = output {
                    if !target.prepare_gpu_output_ready(self, output)? {
                        return Ok(());
                    }
                }
                self.desktop.gpu.phase = LinuxGpuPhase::Prepared;
            }
        }
        Ok(())
    }

    fn discard_transition(&mut self, transition: Transition) {
        match transition.stage {
            Stage::Quiescing => {}
            Stage::Copying(copy) => drop(unsafe { copy.finish(self) }),
            Stage::Prepared { mut target, output } => {
                if let Some(output) = output {
                    target.destroy_prepared_gpu_output(self, output);
                }
                drop(target);
            }
        }
    }

    fn discard_ready(&self, transition: &Transition) -> Result<bool, String> {
        match &transition.stage {
            Stage::Quiescing => Ok(true),
            Stage::Copying(copy) => unsafe { copy.drained(self) },
            Stage::Prepared { target, output } => {
                if !target.hosted_routes_idle()? {
                    return Ok(false);
                }
                match output {
                    Some(output) => target.prepared_gpu_output_idle(self, output),
                    None => Ok(true),
                }
            }
        }
    }

    pub(super) fn destroy_gpu_transition(&mut self) {
        if let Some(transition) = self.desktop.gpu.pending.take() {
            self.device_wait_idle();
            self.discard_transition(transition);
        }
        // RetiredRenderer::Separate has an independent device. Drop it before
        // any source parents, without touching the live presenter's ownership.
        drop(self.desktop.gpu.retired.take());
    }
}

impl Cx {
    pub(crate) fn linux_poll_gpu_transition(&mut self) {
        if self
            .os
            .vulkan
            .as_ref()
            .is_some_and(CxVulkan::gpu_transition_pending)
        {
            let gpu = self.os.vulkan.take().unwrap();
            self.os.vulkan = Some(gpu.advance_gpu_transition(self));
        }
    }
}
