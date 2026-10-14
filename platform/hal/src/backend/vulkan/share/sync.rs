//! FD synchronization. Queue operations run on the submit worker; only nonblocking
//! timeline queries run on the encoder. Semaphore leases last through GPU completion.
use super::super::device::{error, Device};
use makepad_vulkan::vk;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};

pub(crate) struct Semaphore { pub dev: Arc<Device>, pub raw: vk::Semaphore }
impl Drop for Semaphore {
    fn drop(&mut self) { unsafe { self.dev.raw.destroy_semaphore(self.raw, None); } }
}

pub(crate) fn sync_files(dev: &Device) -> bool {
    if !dev.supports(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME) { return false; }
    let query = vk::PhysicalDeviceExternalSemaphoreInfo::default().handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut props = vk::ExternalSemaphoreProperties::default();
    unsafe { dev.instance.get_physical_device_external_semaphore_properties(dev.physical, &query, &mut props); }
    props.external_semaphore_features.contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE | vk::ExternalSemaphoreFeatureFlags::IMPORTABLE)
}

pub(crate) fn timeline_supported(dev: &Device) -> bool {
    if !dev.supports(vk::KHR_TIMELINE_SEMAPHORE_NAME) || !dev.supports(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME) { return false; }
    let mut ty = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    let query = vk::PhysicalDeviceExternalSemaphoreInfo::default()
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD).next_semaphore_type_create_info(&mut ty);
    let mut props = vk::ExternalSemaphoreProperties::default();
    unsafe { dev.instance.get_physical_device_external_semaphore_properties(dev.physical, &query, &mut props); }
    props.external_semaphore_features.contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE | vk::ExternalSemaphoreFeatureFlags::IMPORTABLE)
}

impl Semaphore {
    pub fn sync_file(dev: Arc<Device>) -> Result<Arc<Self>, String> {
        if !sync_files(&dev) { return Err("Vulkan SYNC_FD semaphores unavailable".into()); }
        let mut export = vk::ExportSemaphoreCreateInfo::default().handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        let raw = unsafe { dev.raw.create_semaphore(&vk::SemaphoreCreateInfo::default().next_export_semaphore_create_info(&mut export), None) }.map_err(error)?;
        Ok(Arc::new(Self { dev, raw }))
    }

    pub fn export(&self, kind: vk::ExternalSemaphoreHandleTypeFlags) -> Result<Option<OwnedFd>, String> {
        let fd = unsafe { self.dev.raw.get_semaphore_fd(&vk::SemaphoreGetFdInfoKHR::default().semaphore(self.raw).handle_type(kind)) }.map_err(error)?;
        // SYNC_FD -1 is an already-signaled fence, not a Unix descriptor.
        if fd == -1 && kind == vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD { return Ok(None); }
        if fd < 0 { return Err("invalid exported semaphore FD".into()); }
        Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
}

pub(crate) struct Timeline {
    pub semaphore: Arc<Semaphore>,
    pub producer: bool,
    next: AtomicU64,
    accepted: AtomicU64,
}
impl Timeline {
    pub fn create(dev: Arc<Device>) -> Result<Arc<Self>, String> {
        let mut export = vk::ExportSemaphoreCreateInfo::default().handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
        let mut ty = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(2);
        let raw = unsafe { dev.raw.create_semaphore(&vk::SemaphoreCreateInfo::default()
            .next_export_semaphore_create_info(&mut export).next_semaphore_type_create_info(&mut ty), None) }.map_err(error)?;
        Ok(Arc::new(Self { semaphore: Arc::new(Semaphore { dev, raw }), producer: false, next: AtomicU64::new(2), accepted: AtomicU64::new(0) }))
    }

    pub fn import(dev: Arc<Device>, fd: &OwnedFd) -> Result<Arc<Self>, String> {
        if !timeline_supported(&dev) { return Err("hosted external timelines unavailable".into()); }
        let mut ty = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let raw = unsafe { dev.raw.create_semaphore(&vk::SemaphoreCreateInfo::default().next_semaphore_type_create_info(&mut ty), None) }.map_err(error)?;
        let semaphore = Arc::new(Semaphore { dev, raw });
        let fd = fd.try_clone().map_err(|e| e.to_string())?;
        unsafe { semaphore.dev.raw.import_semaphore_fd(&vk::ImportSemaphoreFdInfoKHR::default().semaphore(raw)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD).fd(fd.as_raw_fd())) }.map_err(error)?;
        let _ = fd.into_raw_fd();
        let value = unsafe { semaphore.dev.raw.get_semaphore_counter_value(raw) }.map_err(error)?;
        let next = value.checked_add(value & 1).filter(|n| *n >= 2).ok_or("invalid hosted timeline state")?;
        Ok(Arc::new(Self { semaphore, producer: true, next: AtomicU64::new(next), accepted: AtomicU64::new(0) }))
    }

    pub fn export(&self) -> Result<OwnedFd, String> {
        self.semaphore.export(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD)?.ok_or("missing timeline FD".into())
    }
    pub fn available(&self) -> Result<bool, String> {
        let value = unsafe { self.semaphore.dev.raw.get_semaphore_counter_value(self.semaphore.raw) }.map_err(error)?;
        Ok(value >= self.next.load(Ordering::Acquire))
    }
    pub fn submitted(&self) -> Result<(u64, u64), String> {
        let wait = self.next.load(Ordering::Acquire);
        let next = wait.checked_add(2).ok_or("hosted timeline exhausted; replace the swapchain")?;
        self.next.store(next, Ordering::Release);
        Ok((wait, wait + 1))
    }
    /// A batch that called [`Self::submitted`] was never submitted (its recording failed): the
    /// frame it would have published does not exist, the image stays the writer's.
    pub fn unsubmitted(&self, wait: u64) {
        let _ = self.next.compare_exchange(wait + 2, wait, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn sequence(&self) -> u64 { self.next.load(Ordering::Acquire).saturating_sub(1) }
    pub fn accept(&self, sequence: u64) -> Result<(), String> {
        if self.producer || sequence < 3 || sequence & 1 == 0 || sequence == u64::MAX
            || sequence <= self.accepted.load(Ordering::Acquire) { return Err("stale or invalid hosted frame lease".into()); }
        let value = unsafe { self.semaphore.dev.raw.get_semaphore_counter_value(self.semaphore.raw) }.map_err(error)?;
        if value != sequence { return Err("hosted frame has not completed or was already released".into()); }
        self.accepted.store(sequence, Ordering::Release);
        Ok(())
    }
}
