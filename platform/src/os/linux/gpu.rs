//! Linux renderer identity and explicit, coordinated device transitions.
//! The host pauses its clients before preparing, installs their replacement
//! swapchains while prepared, then commits and retires the old generation.
//! Accepted control requests do not imply GPU completion; poll the snapshot.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinuxGpuDevice {
    pub uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub pci_address: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinuxGpuPhase {
    #[default]
    Idle,
    Quiescing,
    Copying,
    Prepared,
    Committed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinuxGpuSnapshot {
    pub devices: Vec<LinuxGpuDevice>,
    pub renderer: Option<LinuxGpuDevice>,
    pub display: Option<LinuxGpuDevice>,
    pub renderer_generation: u64,
    pub transition: u64,
    pub phase: LinuxGpuPhase,
    pub copied_bytes: u64,
    pub error: Option<String>,
}

#[cfg(all(not(gpusim), linux_direct, use_vulkan))]
impl crate::cx::Cx {
    pub fn linux_gpu_snapshot(&self) -> LinuxGpuSnapshot {
        self.os
            .vulkan
            .as_ref()
            .map(|gpu| gpu.gpu_snapshot())
            .unwrap_or_default()
    }

    /// The host must first stop old-generation producer submissions. It keeps
    /// pumping control messages and drains client frames until they quiesce.
    /// `generation` must increase; `transition` is never reused in a process.
    pub fn linux_prepare_gpu(
        &mut self,
        transition: u64,
        generation: u64,
        target: [u8; 16],
    ) -> Result<(), String> {
        self.os
            .vulkan
            .as_mut()
            .ok_or("Vulkan renderer unavailable")?
            .begin_gpu_transition(transition, generation, target)?;
        self.redraw_all();
        Ok(())
    }

    /// Commit only after every required client has acknowledged its prepared
    /// renderer and replacement framebuffer transport. No process restarts.
    pub fn linux_commit_gpu(&mut self, transition: u64) -> Result<(), String> {
        self.os
            .vulkan
            .as_mut()
            .ok_or("Vulkan renderer unavailable")?
            .request_gpu_commit(transition)
    }

    /// Old clients must have acknowledged commit and released their imports.
    pub fn linux_retire_gpu(&mut self, transition: u64) -> Result<(), String> {
        self.os
            .vulkan
            .as_mut()
            .ok_or("Vulkan renderer unavailable")?
            .retire_gpu_transition(transition)
    }

    /// Cancellation is available before commit. It finishes outstanding
    /// bounded GPU transfers before unfreezing the original renderer.
    pub fn linux_cancel_gpu(&mut self, transition: u64) -> Result<(), String> {
        self.os
            .vulkan
            .as_mut()
            .ok_or("Vulkan renderer unavailable")?
            .request_gpu_cancel(transition)
    }
}

#[cfg(not(all(not(gpusim), linux_direct, use_vulkan)))]
impl crate::cx::Cx {
    pub fn linux_gpu_snapshot(&self) -> LinuxGpuSnapshot {
        LinuxGpuSnapshot::default()
    }
    pub fn linux_prepare_gpu(&mut self, _: u64, _: u64, _: [u8; 16]) -> Result<(), String> {
        Err("GPU migration requires the direct Linux Vulkan backend".into())
    }
    pub fn linux_commit_gpu(&mut self, _: u64) -> Result<(), String> {
        Err("GPU migration requires the direct Linux Vulkan backend".into())
    }
    pub fn linux_retire_gpu(&mut self, _: u64) -> Result<(), String> {
        Err("GPU migration requires the direct Linux Vulkan backend".into())
    }
    pub fn linux_cancel_gpu(&mut self, _: u64) -> Result<(), String> {
        Err("GPU migration requires the direct Linux Vulkan backend".into())
    }
}
