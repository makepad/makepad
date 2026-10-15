//! Whole-target Vulkan display WSI. The driver owns KMS page flips; present-wait or
//! acquire-fence completion supplies frame credit. Idle outputs do not submit frames.
use crate::wide_desktop;
use super::{dma_buf, drm_sys as drm, gpu_bridge::GpuBridge, kms::*,
    Command, Frame, Key, Snapshot, State};
use super::super::{io, lock};
use crate::{backend::vulkan::{Device, Image}, direct::*, session::TextureObj, types::{format, usage}};
use makepad_vulkan::vk;
use std::{collections::{HashMap, HashSet, VecDeque}, fs::{File, OpenOptions},
    os::fd::{AsRawFd, IntoRawFd}, path::{Path, PathBuf}, sync::{Arc, atomic::Ordering}, time::{Duration, Instant}};

fn error(e: vk::Result) -> String { format!("Vulkan display: {e:?}") }
fn uuid(instance: &makepad_vulkan::Instance, physical: vk::PhysicalDevice) -> String {
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().next_physical_device_id_properties(&mut id);
    unsafe { instance.get_physical_device_properties2(physical, &mut properties); }
    id.device_uuid.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn export_device(device: &Device) {
    // Set before Startup, while the host has not spawned any hosted children yet.
    std::env::set_var(LINUX_VULKAN_DEVICE_ENV, uuid(&device.instance, device.physical));
}
fn device_pci(instance: &makepad_vulkan::Instance, physical: vk::PhysicalDevice) -> Option<String> {
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }.ok()?;
    if !extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(vk::EXT_PCI_BUS_INFO_NAME)) { return None; }
    let mut pci = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().next_physical_device_pci_bus_info_properties_ext(&mut pci);
    unsafe { instance.get_physical_device_properties2(physical, &mut properties); }
    Some(format!("{:04x}:{:02x}:{:02x}.{}", pci.pci_domain, pci.pci_bus, pci.pci_device, pci.pci_function))
}
fn card_name(path: &Path) -> String { path.file_name().unwrap_or_default().to_string_lossy().into_owned() }
fn pci(path: &Path) -> Option<String> {
    std::fs::canonicalize(Path::new("/sys/class/drm").join(card_name(path)).join("device"))
        .ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
}

pub(super) fn order_devices(instance: &makepad_vulkan::Instance, devices: &mut Vec<vk::PhysicalDevice>) -> Result<(), String> {
    let connectors = scan_drm_connectors(&DrmFilter::from_env())?;
    let files: Vec<_> = connectors.iter().filter_map(|c| OpenOptions::new().read(true).write(true).open(&c.card).ok().map(|f| (c,f))).collect();
    let pin = std::env::var("MAKEPAD_VULKAN_COMPOSITOR_UUID").ok();
    if pin.as_ref().is_some_and(|s| s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("MAKEPAD_VULKAN_COMPOSITOR_UUID must contain 32 hex digits".into());
    }
    let pci_pin = std::env::var("MAKEPAD_VULKAN_COMPOSITOR_PCI").ok().filter(|s| !s.is_empty());
    let mut ranked = Vec::new();
    for (rank, physical) in devices.iter().copied().enumerate() {
        let owned: Vec<_> = files.iter().filter(|(c,f)| unsafe { instance.get_drm_display(physical,f.as_raw_fd(),c.id) }.is_ok_and(|d| d != vk::DisplayKHR::null())).collect();
        let selected = pin.as_ref().is_some_and(|p| p.eq_ignore_ascii_case(&uuid(instance,physical)))
            || (pin.is_none() && pci_pin.as_ref().is_some_and(|p| device_pci(instance,physical).as_ref() == Some(p)
                || owned.iter().any(|(c,_)| pci(&c.card).as_ref() == Some(p))));
        let builtin = owned.iter().map(|(c,_)| connector_rank(&c.name).0).min().unwrap_or(2);
        let kind = unsafe { instance.get_physical_device_properties(physical) }.device_type;
        ranked.push((render_rank(selected, kind, builtin, rank), physical));
    }
    let selected = ranked.iter().any(|r| !r.0.0);
    if pin.is_some() && !selected {
        return Err("requested compositor GPU UUID is not available".into());
    }
    if pci_pin.is_some() && !selected {
        eprintln!("[hal-direct] saved compositor PCI is unavailable; using the display GPU");
    }
    if selected { ranked.retain(|r| !r.0.0); }
    ranked.sort_by_key(|r| r.0);
    *devices = ranked.into_iter().map(|r| r.1).collect();
    Ok(())
}

/// The compositor's render GPU, best first: an explicit pin (MAKEPAD_VULKAN_COMPOSITOR_UUID/PCI),
/// then a discrete GPU over an integrated one, whichever GPU owns the panel. A GPU's
/// connectors make it a scan-out target only: the bridge feeds the outputs of every other GPU
/// (the eDP on an Intel card0 of a laptop is shown from the discrete GPU's frames). The
/// built-in-connector owner only breaks ties between GPUs of the same kind, then enumeration.
fn render_rank(selected: bool, kind: vk::PhysicalDeviceType, builtin: u8, index: usize) -> (bool, u8, u8, usize) {
    let kind = match kind {
        vk::PhysicalDeviceType::DISCRETE_GPU => 0,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
        _ => 2,
    };
    (!selected, kind, builtin, index)
}

struct SavedCrtc { crtc: drm::Crtc, connectors: Vec<u32> }
struct Card { file: File, saved: Vec<SavedCrtc>, displays: Vec<(Arc<Device>,vk::DisplayKHR)>, mesa: bool }
impl Card {
    fn open(path: &Path, device: &Arc<Device>) -> Result<Self,String> {
        let file = OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{}: {e}",path.display()))?;
        if unsafe { drm::drmSetMaster(file.as_raw_fd()) } < 0 { return Err(format!("{}: DRM master: {}",path.display(),std::io::Error::last_os_error())); }
        let fd = file.as_raw_fd();
        let mut saved = Vec::new();
        unsafe {
            let resources = drm::drmModeGetResources(fd);
            if !resources.is_null() {
                let resources_ref = &*resources;
                let mut assignments: HashMap<u32, Vec<u32>> = HashMap::new();
                if resources_ref.count_connectors > 0 && !resources_ref.connectors.is_null() {
                    for id in std::slice::from_raw_parts(resources_ref.connectors,resources_ref.count_connectors as usize) {
                        let connector = drm::drmModeGetConnector(fd,*id);
                        if !connector.is_null() {
                            let encoder = drm::drmModeGetEncoder(fd,(*connector).encoder_id);
                            if !encoder.is_null() { assignments.entry((*encoder).crtc_id).or_default().push(*id); drm::drmModeFreeEncoder(encoder); }
                            drm::drmModeFreeConnector(connector);
                        }
                    }
                }
                if resources_ref.count_crtcs > 0 && !resources_ref.crtcs.is_null() {
                    for id in std::slice::from_raw_parts(resources_ref.crtcs,resources_ref.count_crtcs as usize) {
                        let crtc = drm::drmModeGetCrtc(fd,*id);
                        if !crtc.is_null() { saved.push(SavedCrtc { crtc: *crtc, connectors: assignments.remove(id).unwrap_or_default() }); drm::drmModeFreeCrtc(crtc); }
                    }
                }
                drm::drmModeFreeResources(resources);
            }
        }
        let extensions = unsafe { device.instance.enumerate_device_extension_properties(device.physical) }.map_err(error)?;
        let mut driver = vk::PhysicalDeviceDriverProperties::default();
        let mesa = if extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(vk::KHR_DRIVER_PROPERTIES_NAME)) {
            let mut props = vk::PhysicalDeviceProperties2::default().next_physical_device_driver_properties(&mut driver);
            unsafe { device.instance.get_physical_device_properties2(device.physical,&mut props); }
            driver.driver_info_as_c_str().ok().is_some_and(|s| s.to_bytes().starts_with(b"Mesa "))
        } else { false };
        Ok(Self { file,saved,displays:Vec::new(),mesa })
    }
    fn acquire(&mut self,device:Arc<Device>,display:vk::DisplayKHR) -> Result<(),String> {
        if self.displays.iter().any(|(_,d)| *d == display) { return Ok(()); }
        if !self.mesa { unsafe { device.instance.acquire_drm_display(device.physical,self.file.as_raw_fd(),display) }.map_err(error)?; }
        else if self.displays.is_empty() {
            // Mesa display WSI owns and closes one descriptor for the entire card.
            let duplicate = self.file.try_clone().map_err(|e| e.to_string())?;
            unsafe { device.instance.acquire_drm_display(device.physical,duplicate.as_raw_fd(),display) }.map_err(error)?;
            let _ = duplicate.into_raw_fd();
        }
        self.displays.push((device,display)); Ok(())
    }
    /// Give up one display. Mesa's acquisition covers the whole card and a release of
    /// any display ends it, so there the card is released with its last display only.
    fn release_display(&mut self,display:vk::DisplayKHR) {
        let Some(index) = self.displays.iter().position(|(_,d)| *d == display) else { return; };
        let (dev,display) = self.displays.remove(index);
        if !self.mesa || self.displays.is_empty() {
            let result = unsafe { (dev.instance.table.release_display_ext.expect("VK_EXT_direct_mode_display"))(dev.physical,display) };
            if result != vk::Result::SUCCESS { eprintln!("[hal-direct] release display: {result:?}"); }
        }
    }
    fn release(&mut self) {
        for (index,(dev,display)) in self.displays.drain(..).enumerate() {
            if !self.mesa || index == 0 {
                let result = unsafe { (dev.instance.table.release_display_ext.expect("VK_EXT_direct_mode_display"))(dev.physical,display) };
                if result != vk::Result::SUCCESS { eprintln!("[hal-direct] release display: {result:?}"); }
            }
        }
    }
}
impl Drop for Card {
    fn drop(&mut self) {
        self.release();
        let fd = self.file.as_raw_fd();
        for saved in &mut self.saved {
            let c = &mut saved.crtc;
            let mode = if c.mode_valid != 0 { &mut c.mode as *mut _ } else { std::ptr::null_mut() };
            if unsafe { drm::drmModeSetCrtc(fd,c.crtc_id,c.buffer_id,c.x,c.y,saved.connectors.as_mut_ptr(),saved.connectors.len() as i32,mode) } < 0 {
                eprintln!("[hal-direct] restore CRTC {}: {}",c.crtc_id,std::io::Error::last_os_error());
            }
        }
        if unsafe { drm::drmDropMaster(fd) } < 0 { eprintln!("[hal-direct] drop DRM master: {}",std::io::Error::last_os_error()); }
    }
}

/// Reusable command/fence pair. Only the display worker calls it. Image leases are
/// held until done; queue synchronization also covers the HAL's render submissions.
struct CopyJob { dev: Arc<Device>, pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence, pending: bool, keep: Option<Arc<Image>> }
impl CopyJob {
    fn new(dev: Arc<Device>) -> Result<Self,String> {
        let mut job = Self { dev,pool:vk::CommandPool::null(),cb:vk::CommandBuffer::null(),fence:vk::Fence::null(),pending:false,keep:None };
        unsafe {
            job.pool = job.dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(job.dev.family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),None).map_err(error)?;
            job.cb = job.dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(job.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(error)?[0];
            job.fence = job.dev.raw.create_fence(&vk::FenceCreateInfo::default(),None).map_err(error)?;
        }
        Ok(job)
    }
    fn done(&mut self) -> Result<bool,String> {
        if self.pending && !unsafe { self.dev.raw.get_fence_status(self.fence) }.map_err(error)? { return Ok(false); }
        self.pending = false; self.keep = None; Ok(true)
    }
    fn begin(&self) -> Result<(),String> { unsafe {
        self.dev.raw.reset_command_buffer(self.cb,vk::CommandBufferResetFlags::empty()).map_err(error)?;
        self.dev.raw.begin_command_buffer(self.cb,&vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)
    } }
    fn submit(&mut self,signal:vk::Semaphore,image:Arc<Image>) -> Result<(),String> {
        unsafe {
            self.dev.raw.end_command_buffer(self.cb).map_err(error)?;
            self.dev.raw.reset_fences(&[self.fence]).map_err(error)?;
            let commands = [self.cb]; let signals = [signal];
            let submit = vk::SubmitInfo::default().command_buffers(&commands).signal_semaphores(&signals);
            let _queue = lock(&self.dev.queue_lock);
            self.dev.raw.queue_submit(self.dev.queue,&[submit],self.fence).map_err(error)?;
        }
        self.pending = true; self.keep = Some(image); Ok(())
    }
}
impl Drop for CopyJob {
    fn drop(&mut self) { unsafe {
        if self.pending { let _ = self.dev.raw.wait_for_fences(&[self.fence],true,u64::MAX); }
        self.dev.raw.destroy_fence(self.fence,None); self.dev.raw.destroy_command_pool(self.pool,None);
    } }
}

fn range() -> vk::ImageSubresourceRange { vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1) }
fn transition(dev:&Device,cb:vk::CommandBuffer,image:vk::Image,old:vk::ImageLayout,new:vk::ImageLayout) { unsafe {
    let barrier = vk::ImageMemoryBarrier::default().image(image).old_layout(old).new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .src_access_mask(if old == vk::ImageLayout::UNDEFINED { vk::AccessFlags::empty() } else { vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE })
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE).subresource_range(range());
    dev.raw.cmd_pipeline_barrier(cb,vk::PipelineStageFlags::ALL_COMMANDS,vk::PipelineStageFlags::ALL_COMMANDS,vk::DependencyFlags::empty(),&[],&[],&[barrier]);
} }

/// Every call writes the complete destination. The source rectangle is an output's
/// position in the wide desktop, never a repaint region. Overflow outputs are letterboxed
/// (only then is the destination cleared first). Unscaled images of the same format are
/// copied, not blitted: a copy is the cheaper GPU path, and across GPUs half the bytes of
/// a clear plus a blit. `same_format`: both images have the same Vulkan format.
fn blit_full(dev:&Device,cb:vk::CommandBuffer,source:vk::Image,source_layout:vk::ImageLayout,source_size:[u32;2],
    destination:vk::Image,destination_layout:vk::ImageLayout,size:[u32;2],placement:Option<wide_desktop::SliceRect>,same_format:bool) {
    let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
    let (src,dst) = if let Some(r) = placement.filter(|r| r.x + r.width <= source_size[0] && r.y + r.height <= source_size[1]) {
        ([r.x,r.y,r.width,r.height],[0,0,size[0],size[1]])
    } else {
        let scale = (size[0] as f64 / source_size[0].max(1) as f64).min(size[1] as f64 / source_size[1].max(1) as f64);
        let w = (source_size[0] as f64 * scale).round().clamp(1.0,size[0] as f64) as u32;
        let h = (source_size[1] as f64 * scale).round().clamp(1.0,size[1] as f64) as u32;
        ([0,0,source_size[0],source_size[1]],[(size[0]-w)/2,(size[1]-h)/2,w,h])
    };
    let offsets = |r:[u32;4]| [vk::Offset3D { x:r[0] as i32,y:r[1] as i32,z:0 },vk::Offset3D { x:(r[0]+r[2]) as i32,y:(r[1]+r[3]) as i32,z:1 }];
    unsafe {
        if dst != [0,0,size[0],size[1]] {
            dev.raw.cmd_clear_color_image(cb,destination,destination_layout,&vk::ClearColorValue { float32:[0.0,0.0,0.0,1.0] },&[range()]);
            transition(dev,cb,destination,destination_layout,destination_layout);
        }
        if same_format && src[2] == dst[2] && src[3] == dst[3] {
            let copy = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub)
                .src_offset(offsets(src)[0]).dst_offset(offsets(dst)[0]).extent(vk::Extent3D { width:dst[2],height:dst[3],depth:1 });
            dev.raw.cmd_copy_image(cb,source,source_layout,destination,destination_layout,&[copy]);
        } else {
            let blit = vk::ImageBlit::default().src_subresource(sub).dst_subresource(sub).src_offsets(offsets(src)).dst_offsets(offsets(dst));
            dev.raw.cmd_blit_image(cb,source,source_layout,destination,destination_layout,&[blit],vk::Filter::NEAREST);
        }
    }
}

struct Peer {
    source: Arc<Device>, destination: Arc<Device>, bridge: Option<GpuBridge>, local: Arc<Image>,
    sent: u64, received: u64, keep: Option<Arc<Image>>, valid: bool,
    /// profile: when the last send / receive was submitted
    sent_at: u64, received_at: u64,
}
impl Peer {
    fn new(source:Arc<Device>,destination:Arc<Device>,size:[u32;2]) -> Result<Self,String> {
        if !unsafe { dma_buf::supports_extensions(&source.instance,source.physical) && dma_buf::supports_extensions(&destination.instance,destination.physical)
            && dma_buf::supports_sync_files(&source.instance,source.physical) && dma_buf::supports_sync_files(&destination.instance,destination.physical) } {
            return Err("display peer requires DMA-BUF modifiers and SYNC_FD on both GPUs".into());
        }
        let local = Image::new(destination.clone(),TextureObj { format:format::BGRA8,w:size[0],h:size[1],depth:1,mips:1,
            usage:usage::RENDER_TARGET | usage::SAMPLED,bytes:0,imported:false,uploaded:0,lent:None })?;
        let bridge = GpuBridge::new(&source,&destination,vk::Extent2D { width:size[0],height:size[1] })?;
        Ok(Self { source,destination,bridge:Some(bridge),local,sent:0,received:0,keep:None,valid:false,sent_at:0,received_at:0 })
    }
    fn service(&mut self,frame:&Frame,placement:Option<wide_desktop::SliceRect>) -> Result<bool,String> {
        let bridge = self.bridge.as_mut().unwrap();
        unsafe {
            if !bridge.poll_initialization(&self.source,&self.destination)? { return Ok(false); }
            if bridge.idle(&self.source,&self.destination)? { self.keep = None; }
            if bridge.pending() {
                let local = self.local.image;
                let old = if self.valid { vk::ImageLayout::TRANSFER_SRC_OPTIMAL } else { vk::ImageLayout::UNDEFINED };
                let size = [self.local.desc.w,self.local.desc.h];
                let received = bridge.receive_commands(&self.source,&self.destination,|cb,shared| {
                    transition(&self.destination,cb,local,old,vk::ImageLayout::TRANSFER_DST_OPTIMAL);
                    blit_full(&self.destination,cb,shared,vk::ImageLayout::GENERAL,size,local,vk::ImageLayout::TRANSFER_DST_OPTIMAL,size,
                        Some(wide_desktop::SliceRect { x:0,y:0,width:size[0],height:size[1] }),true);
                    transition(&self.destination,cb,local,vk::ImageLayout::TRANSFER_DST_OPTIMAL,vk::ImageLayout::TRANSFER_SRC_OPTIMAL); Ok(())
                })?;
                if received {
                    self.received = self.sent; self.valid = true;
                    crate::profile::since(crate::profile::Metric::XgpuSource, self.sent_at);
                    self.received_at = crate::clock_ns();
                }
            }
            if self.sent != frame.generation && bridge.writable(&self.source,&self.destination)? {
                let image = &frame.image;
                let size = bridge.extent();
                bridge.send_commands(&self.source,|cb,shared| {
                    blit_full(&self.source,cb,image.image,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,[image.desc.w,image.desc.h],shared,vk::ImageLayout::GENERAL,[size.width,size.height],placement,true); Ok(())
                })?;
                self.keep = Some(image.clone()); self.sent = frame.generation; self.sent_at = crate::clock_ns();
            }
        }
        Ok(self.received == frame.generation)
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        // Only the display worker drops peers, after display work is drained. Its bridge
        // queues also need to finish before destroying the imported/exported allocations.
        for dev in [&self.source,&self.destination] { let _queue = lock(&dev.queue_lock); let _ = unsafe { dev.raw.queue_wait_idle(dev.queue) }; }
        if let Some(bridge) = self.bridge.take() { unsafe { bridge.destroy(&self.source,&self.destination); } }
    }
}

struct Output {
    dev:Arc<Device>, connector:DrmConnector, display:vk::DisplayKHR, requested:Option<String>,
    /// Why the requested mode is not the one shown (it failed; a fallback is shown).
    error:Option<String>, plane:u32, mode:DisplayMode, format:vk::Format,
    surface:vk::SurfaceKHR, chain:vk::SwapchainKHR, images:Vec<vk::Image>, signals:Vec<vk::Semaphore>,
    acquire:vk::Fence, acquired:Option<u32>, acquire_ready:bool, job:CopyJob, peer:Option<Peer>,
    placement:Option<wide_desktop::SliceRect>, generation:u64, serial:u64, pending:Option<u64>,
    complete:u64, last_present:u64, rebuild:bool,
    progress:(u64,u64,Option<u32>,bool), progress_at:Instant,
    /// profile: the present's submit, the cross-GPU receive's submit
    present_at:u64, peer_at:u64,
}
impl Output {
    fn new(dev:Arc<Device>,compositor:Arc<Device>,connector:DrmConnector,display:vk::DisplayKHR,requested:Option<String>,mode:DisplayMode,plane:PlaneCandidate) -> Result<Self,String> {
        let job = CopyJob::new(dev.clone())?;
        let mut out = Self { dev,connector,display,requested,error:None,plane:plane.plane_index,mode,format:vk::Format::UNDEFINED,surface:vk::SurfaceKHR::null(),chain:vk::SwapchainKHR::null(),images:Vec::new(),signals:Vec::new(),
            acquire:vk::Fence::null(),acquired:None,acquire_ready:false,job,peer:None,placement:None,generation:0,serial:0,pending:None,complete:0,last_present:0,rebuild:false,
            progress:(0,0,None,false),progress_at:Instant::now(),present_at:0,peer_at:0 };
        out.surface = create_plane_surface(&out.dev.instance,&out.mode,&plane)?;
        if !unsafe { out.dev.instance.get_physical_device_surface_support(out.dev.physical,out.dev.family,out.surface) }.map_err(|e| format!("surface support: {}",error(e)))? { return Err("graphics queue cannot present to the display plane".into()); }
        let caps = unsafe { out.dev.instance.get_physical_device_surface_capabilities(out.dev.physical,out.surface) }.map_err(|e| format!("surface capabilities: {}",error(e)))?;
        if !caps.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_DST) { return Err("display swapchain lacks TRANSFER_DST".into()); }
        let mut formats = unsafe { out.dev.instance.get_physical_device_surface_formats(out.dev.physical,out.surface) }.map_err(error)?;
        formats.retain(|f| {
            let format = if f.format == vk::Format::UNDEFINED { vk::Format::B8G8R8A8_UNORM } else { f.format };
            unsafe { out.dev.instance.get_physical_device_format_properties(out.dev.physical,format) }
                .optimal_tiling_features.contains(vk::FormatFeatureFlags::BLIT_DST)
        });
        let format = choose_output_format(&formats)?;
        out.format = format.format;
        let count = if caps.max_image_count == 0 { caps.min_image_count + 1 } else { (caps.min_image_count+1).min(caps.max_image_count) };
        let alpha = [vk::CompositeAlphaFlagsKHR::OPAQUE,vk::CompositeAlphaFlagsKHR::INHERIT].into_iter().find(|a| caps.supported_composite_alpha.contains(*a)).ok_or("display has no opaque composite alpha")?;
        unsafe {
            out.chain = out.dev.raw.create_swapchain(&vk::SwapchainCreateInfoKHR::default().surface(out.surface).min_image_count(count)
                .image_format(format.format).image_color_space(format.color_space).image_extent(out.mode.extent).image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::TRANSFER_DST).image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform).composite_alpha(alpha).present_mode(vk::PresentModeKHR::FIFO).clipped(true),None).map_err(|e| format!("create swapchain {}x{} @ {:.3} Hz: {}",out.mode.extent.width,out.mode.extent.height,out.mode.refresh_mhz as f64 / 1000.0,error(e)))?;
            out.images = out.dev.raw.get_swapchain_images(out.chain).map_err(error)?;
            for _ in &out.images { out.signals.push(out.dev.raw.create_semaphore(&vk::SemaphoreCreateInfo::default(),None).map_err(error)?); }
            out.acquire = out.dev.raw.create_fence(&vk::FenceCreateInfo::default(),None).map_err(error)?;
        }
        if out.dev.physical != compositor.physical { out.peer = Some(Peer::new(compositor,out.dev.clone(),out.size())?); }
        eprintln!("[hal-direct] {} {}x{} {:.3} Hz, plane {}, FIFO, present_wait={}",out.connector.name,out.mode.extent.width,out.mode.extent.height,out.mode.refresh_mhz as f64 / 1000.0,plane.plane_index,out.dev.present_wait);
        Ok(out)
    }
    fn size(&self) -> [u32;2] { [self.mode.extent.width,self.mode.extent.height] }
    fn finish_present(&mut self) {
        if let Some(generation) = self.pending.take() {
            self.complete = generation;
            let now = crate::clock_ns();
            if crate::profile::on() {
                use crate::profile::{record, since, Metric};
                since(Metric::PresentWait, std::mem::take(&mut self.present_at));
                let metric = if self.peer.is_some() { Metric::PeerPresentInterval } else { Metric::PresentInterval };
                if self.last_present != 0 { record(metric, now - self.last_present); }
                since(Metric::CreditToGlass, crate::profile::credit_at());
            }
            self.last_present = now;
        }
    }
    fn service(&mut self,frame:Option<&Frame>) -> Result<(),String> {
        let pending = self.job.pending;
        if !self.job.done()? { return Ok(()); }
        if pending && self.peer.is_some() { crate::profile::since(crate::profile::Metric::XgpuDestination, std::mem::take(&mut self.peer_at)); }
        let has_frame = frame.is_some_and(|f| f.generation != self.generation);
        // A frame for another GPU's output crosses over while this output's previous flip is
        // still pending: the transfer is off the flip's critical path. `local` is free here
        // (the copy job that read it is done).
        let mut transferred = false;
        if let (Some(peer),Some(frame)) = (&mut self.peer,frame.filter(|_| has_frame)) {
            transferred = peer.service(frame,self.placement)?;
            if transferred { self.peer_at = peer.received_at; }
        }
        if self.pending.is_some() && self.dev.present_wait {
            match unsafe { self.dev.raw.wait_for_present(self.chain,self.serial,0) } {
                Ok(()) => self.finish_present(),
                Err(vk::Result::TIMEOUT) => return Ok(()),
                Err(e) => return Err(error(e)),
            }
        }
        // Acquire once after a present on drivers without present-wait: its fence is the
        // WSI page-flip credit. Keep that acquired image for the next requested frame.
        if self.acquired.is_none() && (has_frame || self.pending.is_some()) {
            unsafe { self.dev.raw.reset_fences(&[self.acquire]) }.map_err(error)?;
            match unsafe { self.dev.raw.acquire_next_image(self.chain,0,vk::Semaphore::null(),self.acquire) } {
                Ok((index,suboptimal)) => { self.acquired = Some(index); self.acquire_ready = false; self.rebuild |= suboptimal; }
                Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => return Ok(()),
                Err(e) => return Err(error(e)),
            }
        }
        if self.acquired.is_some() && !self.acquire_ready {
            if !unsafe { self.dev.raw.get_fence_status(self.acquire) }.map_err(error)? { return Ok(()); }
            self.acquire_ready = true;
            if !self.dev.present_wait { self.finish_present(); }
        }
        let Some(frame) = frame.filter(|_| has_frame) else { return Ok(()); };
        if !self.acquire_ready { return Ok(()); }
        let (image,placement) = if let Some(peer) = &self.peer {
            if !transferred { return Ok(()); }
            (peer.local.clone(),Some(wide_desktop::SliceRect { x:0,y:0,width:peer.local.desc.w,height:peer.local.desc.h }))
        } else { (frame.image.clone(),self.placement) };
        let index = self.acquired.take().unwrap(); self.acquire_ready = false;
        self.job.begin()?;
        let destination = self.images[index as usize];
        transition(&self.dev,self.job.cb,destination,vk::ImageLayout::UNDEFINED,vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        blit_full(&self.dev,self.job.cb,image.image,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,[image.desc.w,image.desc.h],destination,vk::ImageLayout::TRANSFER_DST_OPTIMAL,self.size(),placement,self.format == vk::Format::B8G8R8A8_UNORM);
        transition(&self.dev,self.job.cb,destination,vk::ImageLayout::TRANSFER_DST_OPTIMAL,vk::ImageLayout::PRESENT_SRC_KHR);
        let ready = self.signals[index as usize];
        self.job.submit(ready,image)?;
        self.serial += 1;
        let ids = [self.serial]; let mut present_id = vk::PresentIdKHR::default().present_ids(&ids);
        let waits = [ready]; let chains = [self.chain]; let indices = [index];
        let mut info = vk::PresentInfoKHR::default().wait_semaphores(&waits).swapchains(&chains).image_indices(&indices);
        if self.dev.present_wait { info = info.next_present_id_khr(&mut present_id); }
        let result = { let _queue = lock(&self.dev.queue_lock); unsafe { self.dev.raw.queue_present(self.dev.queue,&info) } };
        self.rebuild |= result.map_err(error)?;
        if crate::backend::log_enabled() {
            eprintln!("[hal-direct] present {} generation={} id={} whole={}x{}",self.connector.name,frame.generation,self.serial,self.mode.extent.width,self.mode.extent.height);
        }
        if crate::profile::on() {
            self.present_at = crate::clock_ns();
            crate::profile::since(crate::profile::Metric::ToPresent, frame.taken);
        }
        self.generation = frame.generation; self.pending = Some(frame.generation); Ok(())
    }
    fn busy(&self,frame:Option<&Frame>) -> bool {
        self.job.pending || self.pending.is_some() || (self.acquired.is_some() && !self.acquire_ready)
            || frame.is_some_and(|f| f.generation != self.generation)
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        // Executed on the display worker, never under the HAL or shell state locks.
        let _queue = lock(&self.dev.queue_lock);
        let _ = unsafe { self.dev.raw.queue_wait_idle(self.dev.queue) };
        if self.acquired.is_some() && !self.acquire_ready
            && !unsafe { self.dev.raw.get_fence_status(self.acquire) }.unwrap_or(false) {
            // An unplugged presentation engine may never signal an acquire fence. Park it
            // like the native backend: never reset/destroy a pending fence, and never hold VT.
            eprintln!("[hal-direct] {}: parking pending acquire fence",self.connector.name);
            lock(&PARKED_ACQUIRES).push((self.dev.clone(),std::mem::take(&mut self.acquire)));
        }
        unsafe {
            self.dev.raw.destroy_swapchain(self.chain,None);
            for signal in self.signals.drain(..) { self.dev.raw.destroy_semaphore(signal,None); }
            self.dev.raw.destroy_fence(self.acquire,None);
            self.dev.instance.destroy_surface(self.surface,None);
        }
    }
}

// A lost WSI acquire may remain pending even after its swapchain has retired. Keep the
// owning device alive until that fence signals. At process exit, leave unsignaled objects
// to the driver, as the native direct backend does, instead of destroying live children.
static PARKED_ACQUIRES: std::sync::Mutex<Vec<(Arc<Device>,vk::Fence)>> = std::sync::Mutex::new(Vec::new());
fn drain_parked_acquires() {
    let fences = std::mem::take(&mut *lock(&PARKED_ACQUIRES));
    let mut pending = Vec::new();
    for (device,fence) in fences {
        // an error is a lost device: nothing will signal the fence any more
        if unsafe { device.raw.get_fence_status(fence) }.unwrap_or(true) {
            unsafe { device.raw.destroy_fence(fence,None); }
        } else { pending.push((device,fence)); }
    }
    lock(&PARKED_ACQUIRES).extend(pending);
}

/// One output that shows the desktop, as the pacing choice sees it.
struct PacingCandidate<'a> { index:usize, name:&'a str, refresh_mhz:u32, external:bool, main:bool, rendering_gpu:bool }

/// Which output paces the desktop, and whether that is the setting's own pick (`true`) or
/// Auto's "each screen at its own refresh" (`false`, reported as `paced_by: None`). Every
/// other output shows the newest frame at its own refresh and never causes a render.
/// `Auto`: extended -- credit follows the fastest screen, so each screen runs at its own
/// refresh (the panel is not held back, a slow monitor presents only the frames it can
/// show); mirror -- the slowest EXTERNAL output (the audience screen shows every frame),
/// else the built-in. Ties prefer the main screen, then the rendering GPU (no cross-GPU
/// copy on the pacing path), then the first. A named screen that is gone falls back to Auto.
fn pacing_choice(pacing:&crate::direct::DisplayPacing,mirror:bool,outputs:&[PacingCandidate]) -> Option<(usize,bool)> {
    use crate::direct::DisplayPacing;
    let tie = |c:&PacingCandidate| (!c.main,!c.rendering_gpu,c.index);
    let slowest = |external:bool| outputs.iter().filter(|c| !external || c.external).min_by_key(|c| (c.refresh_mhz,tie(c))).map(|c| c.index);
    let fastest = || outputs.iter().max_by_key(|c| (c.refresh_mhz,std::cmp::Reverse(tie(c)))).map(|c| c.index);
    let auto = || if mirror { slowest(true).or_else(|| slowest(false)).map(|i| (i,true)) } else { fastest().map(|i| (i,false)) };
    match pacing {
        DisplayPacing::Auto => auto(),
        DisplayPacing::Slowest => slowest(false).map(|i| (i,true)),
        DisplayPacing::Fastest => fastest().map(|i| (i,true)),
        DisplayPacing::Screen(name) => outputs.iter().find(|c| c.name == name).map(|c| (c.index,true)).or_else(auto),
    }
}

/// A connector that could not be driven, with the request it failed under. It is
/// retried only when the connector itself changes (replug, another monitor) or its
/// mode request changes, never on a timer: a retry must not disturb working outputs.
struct Failed { connector:DrmConnector, requested:Option<String>, row:LinuxDisplayOutput }

fn mode_label(mode:&DisplayMode) -> String {
    format!("{}x{}@{}",mode.extent.width,mode.extent.height,(mode.refresh_mhz + 500) / 1000)
}

/// What a reconcile does with a live output.
#[derive(Debug,PartialEq,Eq)]
enum Fate { Keep, Recheck, Rebuild }
fn output_fate(present:bool,rebuild:bool,old:Option<&str>,new:Option<&str>) -> Fate {
    if !present || rebuild { Fate::Rebuild } else if old == new { Fate::Keep } else { Fate::Recheck }
}

/// Each connector's mode request: its own saved override, else the global one
/// (`MAKEPAD_DRM_MODE` / `-mode=`) for the first connector (rank order) without one,
/// else none, which selects the monitor's own preferred (EDID) resolution at its
/// fastest refresh. One screen's override never reaches another screen.
fn connector_requests(connectors:&[DrmConnector],modes:&HashMap<String,String>,global:Option<&str>) -> HashMap<String,Option<String>> {
    let mut global = global;
    connectors.iter().map(|c| {
        let request = modes.get(&c.name).cloned().or_else(|| global.take().map(str::to_owned));
        (c.name.clone(),request)
    }).collect()
}

struct Context {
    compositor:Arc<Device>, devices:HashMap<vk::PhysicalDevice,Arc<Device>>, physical:Vec<vk::PhysicalDevice>,
    cards:HashMap<PathBuf,Card>, outputs:Vec<Output>, failed:Vec<Failed>,
    /// Per connector, the last mode an output was created with: the first fallback.
    working:HashMap<String,DisplayMode>,
    /// Mirror's optimised screen (see `DisplayArrangement::Mirror`); `None`: extended.
    mirror:Option<String>, pacing:crate::direct::DisplayPacing, connectors:Vec<DrmConnector>,
    filter:DrmFilter, modes:HashMap<String,String>, mode:Option<String>, order:Vec<String>, primary:Option<String>,
    size:[u32;2], active:bool, window:Option<Key>, current:Option<Frame>, frames:VecDeque<Frame>,
    /// Frames handed to the outputs whose present has not completed on the pacing output yet,
    /// with the callback that reports it (oldest first).
    presenting:VecDeque<(u64,super::Presented)>,
    sequence:u64, demand:bool, awaiting:bool,
}
impl Context {
    fn new(compositor:Arc<Device>) -> Result<Self,String> {
        let physical = unsafe { compositor.instance.enumerate_physical_devices() }.map_err(error)?;
        let mut mode = std::env::var("MAKEPAD_DRM_MODE").ok().filter(|m| !m.is_empty() && m != "auto");
        for arg in std::env::args() { if let Some(value) = arg.strip_prefix("-mode=").or_else(|| arg.strip_prefix("--mode=")) { mode = (value != "auto").then(|| value.to_owned()); } }
        let modes = wide_desktop::parse_mode_overrides(&std::env::var("MAKEPAD_DRM_MODES").unwrap_or_default()).into_iter().collect();
        let order = wide_desktop::parse_display_order(&std::env::var("MAKEPAD_DISPLAY_ORDER").unwrap_or_default());
        Ok(Self { devices:HashMap::from([(compositor.physical,compositor.clone())]),compositor,physical,
            cards:HashMap::new(),outputs:Vec::new(),failed:Vec::new(),working:HashMap::new(),mirror:None,pacing:Default::default(),connectors:Vec::new(),filter:DrmFilter::from_env(),
            modes,mode,order,primary:None,size:[0,0],active:true,window:None,current:None,frames:VecDeque::new(),presenting:VecDeque::new(),sequence:0,demand:false,awaiting:false })
    }
    /// The HAL replaced its lost GPU device. The outputs' swapchains, surfaces, copy jobs and
    /// peers lived on the old device and are dropped; the cards keep DRM master and their
    /// display acquisitions (a release would restore the console's CRTCs, and NVIDIA refuses
    /// a display re-acquired on the same descriptor), so the next reconcile rebuilds every
    /// output on the new device. The frames of the old device are let go (their presents
    /// reported, so no credit is lost) and the window asks for a new one.
    fn replace_device(&mut self,device:Arc<Device>,state:&State) {
        eprintln!("[hal-direct] the render GPU device was replaced: rebuilding the outputs");
        self.outputs.clear();
        self.frames.clear(); self.current = None;
        let now = crate::clock_ns();
        for (_,presented) in self.presenting.drain(..) { presented(now); }
        let old = self.compositor.physical;
        for card in self.cards.values_mut() {
            for (dev,_) in &mut card.displays { if dev.physical == old { *dev = device.clone(); } }
        }
        self.devices.insert(device.physical,device.clone());
        self.compositor = device;
        self.connectors.clear(); self.failed.clear();
        state.ready.store(0,Ordering::Release);
        self.demand = self.window.is_some(); self.awaiting = false;
    }
    fn drain_outputs(&mut self) {
        self.outputs.clear();
        for card in self.cards.values_mut() { card.release(); }
    }
    fn pause(&mut self,state:&State) {
        self.active = false; self.drain_outputs(); self.cards.clear();
        // Resume retries every connector, including ones that failed before the switch.
        self.failed.clear(); self.connectors.clear();
        state.ready.store(0,Ordering::Release); self.awaiting = false;
        {
            let mut snapshot = lock(&state.snapshot);
            snapshot.displays.direct = false;
            for output in &mut snapshot.displays.outputs { output.active = false; output.status = "VT inactive".into(); }
            snapshot.timings.clear();
            state.changed(&mut snapshot);
        }
        state.released.store(true,Ordering::Release); state.shared.signal();
    }
    fn fail_row(&mut self,connector:&DrmConnector,requested:Option<String>,reason:String) {
        eprintln!("[hal-direct] {}: {reason}; retried when the connector is replugged or its mode changes",connector.name);
        // The dropdown stays usable on a failed screen: list its DRM (EDID) modes.
        let (modes,native) = match self.cards.get(&connector.card) {
            Some(card) => drm_connector_modes(card.file.as_raw_fd(),connector.id),
            None => OpenOptions::new().read(true).write(true).open(&connector.card).map(|f| drm_connector_modes(f.as_raw_fd(),connector.id)).unwrap_or_default(),
        };
        let row = LinuxDisplayOutput { name:connector.name.clone(),card:card_name(&connector.card),status:reason.clone(),error:Some(reason),
            requested:requested.clone(),modes,native,pci:pci(&connector.card),mode_override:self.modes.get(&connector.name).cloned(),..Default::default() };
        self.failed.push(Failed { connector:connector.clone(),requested,row });
    }
    /// Drop one output and give its display back. Only this output's display goes;
    /// every other output on the card keeps its acquisition and its scanout.
    fn retire(&mut self,output:Output) {
        let (card,display) = (output.connector.card.clone(),output.display);
        drop(output);
        if let Some(card) = self.cards.get_mut(&card) { card.release_display(display); }
    }
    /// Bring the outputs in line with the connected connectors. A working output is
    /// never torn down for another one: it is rebuilt only when its own connector
    /// vanished or changed, its own mode request resolves to another mode, or its own
    /// presentation failed. A connector that failed is not retried on a timer; it is
    /// retried when it is replugged (or another monitor appears on it) or its mode
    /// request changes. Returns without touching anything when nothing changed.
    fn reconcile(&mut self,connectors:Vec<DrmConnector>,state:&State) {
        let requests = connector_requests(&connectors,&self.modes,self.mode.as_deref());
        let requested = |name:&str| requests.get(name).cloned().flatten();
        let mut kept = Vec::new(); let mut retired = Vec::new();
        for mut output in std::mem::take(&mut self.outputs) {
            let request = requested(&output.connector.name);
            match output_fate(connectors.contains(&output.connector),output.rebuild,output.requested.as_deref(),request.as_deref()) {
                Fate::Keep => kept.push(output),
                Fate::Recheck => {
                    // A new request that names the mode already shown (e.g. the saved
                    // layout restating the running mode at start-up) changes nothing.
                    let native = self.cards.get(&output.connector.card).and_then(|c| drm_native_resolution(c.file.as_raw_fd(),output.connector.id));
                    let same = select_display_mode(&output.dev.instance,output.dev.physical,output.display,request.as_deref(),native)
                        .is_ok_and(|m| m.mode == output.mode.mode);
                    if same { output.requested = request; kept.push(output); } else { retired.push(output); }
                }
                Fate::Rebuild => {
                    let reason = if output.rebuild { "its presentation failed".to_string() }
                        else if let Some(c) = connectors.iter().find(|c| c.name == output.connector.name) { format!("connector changed ({:?} -> {:?})",(output.connector.id,output.connector.fingerprint),(c.id,c.fingerprint)) }
                        else { "unplugged".to_string() };
                    eprintln!("[hal-direct] {}: rebuilding: {reason}",output.connector.name);
                    retired.push(output)
                }
            }
        }
        let failed_before = self.failed.len();
        self.failed.retain(|f| connectors.contains(&f.connector) && f.requested == requested(&f.connector.name));
        let pending:Vec<_> = connectors.iter().filter(|c| !kept.iter().any(|o:&Output| o.connector == **c)
            && !self.failed.iter().any(|f| f.connector == **c)).cloned().collect();
        self.outputs = kept;
        let topology_changed = !retired.is_empty() || !pending.is_empty() || self.failed.len() != failed_before || connectors != self.connectors;
        self.connectors = connectors;
        if !topology_changed {
            // An order change (or a rescan with nothing new) only moves screens.
            let before:Vec<_> = self.outputs.iter().map(|o| o.placement).collect();
            let size = self.size;
            self.layout();
            if size == self.size && before == self.outputs.iter().map(|o| o.placement).collect::<Vec<_>>() { return; }
        }
        // A topology generation invalidates an outstanding credit: the client may have
        // discarded it while the desktop had zero size. Reflow needs a fresh full frame.
        self.awaiting = false; self.demand |= self.window.is_some();
        state.ready.store(0,Ordering::Release);
        match unsafe { self.compositor.instance.enumerate_physical_devices() } {
            Ok(physical) => self.physical = physical,
            Err(e) => eprintln!("[hal-direct] refresh Vulkan devices: {e:?}"),
        }
        let mut freed:HashMap<vk::PhysicalDevice,HashSet<u32>> = HashMap::new();
        for output in retired {
            freed.entry(output.dev.physical).or_default().insert(output.plane);
            // A connector that stays (new mode, failed presentation) keeps its display
            // acquired and only gets a new surface and swapchain: the NVIDIA driver
            // cannot hand a released connector back through the same descriptor.
            if self.connectors.contains(&output.connector) { drop(output); } else { self.retire(output); }
        }
        let mut busy:HashMap<vk::PhysicalDevice,HashSet<u32>> = HashMap::new();
        for output in &self.outputs { busy.entry(output.dev.physical).or_default().insert(output.plane); }
        self.cards.retain(|path,_| self.connectors.iter().any(|c| c.card == *path));
        let mut plans:HashMap<vk::PhysicalDevice,Vec<(DrmConnector,vk::DisplayKHR,Option<String>,Vec<DisplayMode>,Option<String>,Vec<PlaneCandidate>)>> = HashMap::new();
        for connector in pending {
            let request = requested(&connector.name);
            let result = (|| {
                // Discover ownership before acquiring. DRM card descriptors are opened once
                // for the display lifetime, including every connector on that card.
                let probe = if let Some(card) = self.cards.get(&connector.card) { card.file.try_clone() }
                    else { OpenOptions::new().read(true).write(true).open(&connector.card) }.map_err(|e| e.to_string())?;
                let (physical,display) = self.physical.iter().find_map(|physical| unsafe {
                    self.compositor.instance.get_drm_display(*physical,probe.as_raw_fd(),connector.id).ok()
                        .filter(|d| *d != vk::DisplayKHR::null()).map(|d| (*physical,d))
                }).ok_or("no Vulkan device owns this connector")?;
                // Close the probe before any master open: a duplicate of a card's file
                // would keep that file (and its master) alive across a reopen below.
                drop(probe);
                let device = if let Some(dev) = self.devices.get(&physical) { dev.clone() } else {
                    let dev = Device::display_peer(self.compositor.clone(),physical)?;
                    self.devices.insert(physical,dev.clone()); dev
                };
                if !self.cards.contains_key(&connector.card) { self.cards.insert(connector.card.clone(),Card::open(&connector.card,&device)?); }
                let card = self.cards.get_mut(&connector.card).unwrap();
                if unsafe { drm::drmSetMaster(card.file.as_raw_fd()) } < 0 { return Err(std::io::Error::last_os_error().to_string()); }
                if let Err(e) = card.acquire(device.clone(),display).map_err(|e| format!("acquire: {e}")) {
                    // The NVIDIA driver grants a connector to a DRM descriptor once; after
                    // vkReleaseDisplayEXT the grant stays on that descriptor and acquiring
                    // again through it fails (VK_ERROR_INITIALIZATION_FAILED). Only closing
                    // the descriptor revokes it, so a card holding no other display is
                    // reopened and acquired once more.
                    if !card.displays.is_empty() { return Err(e); }
                    eprintln!("[hal-direct] {}: {e}; reopening {} for a fresh grant",connector.name,connector.card.display());
                    self.cards.remove(&connector.card);
                    self.cards.insert(connector.card.clone(),Card::open(&connector.card,&device)?);
                    self.cards.get_mut(&connector.card).unwrap().acquire(device.clone(),display).map_err(|e| format!("acquire after reopen: {e}"))?;
                }
                let card = &self.cards[&connector.card];
                let native = drm_native_resolution(card.file.as_raw_fd(),connector.id);
                // The modes to fall back to, in order: the last one that worked on this
                // connector, then the monitor's preferred (EDID) mode. A requested mode
                // that is unavailable or fails never leaves the screen dark.
                let mut modes = Vec::new();
                let mut error = None;
                match select_display_mode(&device.instance,physical,display,request.as_deref(),native) {
                    Ok(mode) => modes.push(mode),
                    Err(e) => error = Some(e),
                }
                if request.is_some() {
                    if let Some(working) = self.working.get(&connector.name) { modes.push(working.clone()); }
                    if let Ok(preferred) = select_display_mode(&device.instance,physical,display,None,native) { modes.push(preferred); }
                }
                let mut seen = HashSet::new();
                modes.retain(|m| seen.insert(m.mode));
                let first = modes.first().ok_or_else(|| error.clone().unwrap_or_else(|| "no display mode".into()))?;
                let candidates = plane_candidates(&device.instance,physical,display,first,busy.entry(physical).or_default(),freed.entry(physical).or_default())?;
                Ok::<_,String>((physical,display,modes,error,candidates))
            })();
            match result {
                Ok((physical,display,modes,error,candidates)) => plans.entry(physical).or_default().push((connector,display,request,modes,error,candidates)),
                Err(e) => self.fail_row(&connector,request,format!("failed: {e}")),
            }
        }
        for (physical,plans) in plans {
            let candidate_ids:Vec<_> = plans.iter().map(|(_,_,_,_,_,p)| p.iter().map(|c| c.plane_index).collect()).collect();
            let assignment = assign_planes(&candidate_ids);
            for ((connector,display,request,modes,mut error,candidates),plane) in plans.into_iter().zip(assignment) {
                let Some(candidate) = plane.and_then(|plane| candidates.iter().find(|c| c.plane_index == plane)).copied() else {
                    if let Some(card) = self.cards.get_mut(&connector.card) { card.release_display(display); }
                    self.fail_row(&connector,request,"failed: no compatible unoccupied display plane".into()); continue;
                };
                let mut created = None;
                for mode in modes {
                    let label = mode_label(&mode);
                    match Output::new(self.devices[&physical].clone(),self.compositor.clone(),connector.clone(),display,request.clone(),mode,candidate) {
                        Ok(output) => { created = Some(output); break; }
                        Err(e) => {
                            eprintln!("[hal-direct] {}: {label} failed: {e}",connector.name);
                            error.get_or_insert(format!("{label}: {e}"));
                        }
                    }
                }
                match created {
                    Some(mut output) => {
                        if let Some(e) = &error {
                            eprintln!("[hal-direct] {}: requested mode {:?} failed ({e}); showing {}",connector.name,request,mode_label(&output.mode));
                        }
                        output.error = error;
                        self.working.insert(connector.name.clone(),output.mode.clone());
                        self.outputs.push(output);
                    }
                    None => {
                        if let Some(card) = self.cards.get_mut(&connector.card) { card.release_display(display); }
                        self.fail_row(&connector,request,format!("failed: {}",error.unwrap_or_default()));
                    }
                }
            }
        }
        self.outputs.sort_by_key(|o| connector_rank(&o.connector.name));
        self.layout(); self.publish(state);
    }
    /// Mirror <-> Extended: only what each output presents changes (the whole shared
    /// desktop scaled into its swapchain, or its own rectangle of the wide desktop) and the
    /// screen list. Every output keeps its display, mode, plane, surface and swapchain: no
    /// blank, no modeset. The desktop size changes, so the client gets a fresh credit and
    /// draws a whole frame at the new size; until then the retained frame is letterboxed.
    fn recompose(&mut self,state:&State) {
        let before = (self.size,self.outputs.iter().map(|o| o.placement).collect::<Vec<_>>());
        self.layout();
        if before == (self.size,self.outputs.iter().map(|o| o.placement).collect::<Vec<_>>()) { self.publish(state); return; }
        self.awaiting = false; self.demand |= self.window.is_some();
        state.ready.store(0,Ordering::Release);
        eprintln!("[hal-direct] arrangement {}: desktop {}x{}, no output rebuilt",self.mirror_target().map_or("extended".to_string(),|n| format!("mirror of {n}")),self.size[0],self.size[1]);
        self.publish(state);
    }
    /// The mirrored screen, when mirroring is asked for and that screen is shown.
    fn mirror_target(&self) -> Option<&str> {
        self.mirror.as_deref().filter(|name| self.outputs.iter().any(|o| o.connector.name == *name))
    }
    fn layout(&mut self) {
        if let Some(target) = self.mirror_target().map(str::to_owned) {
            // Mirror: one desktop at the optimised screen's mode. That screen shows it
            // 1:1; every other output has no rectangle of its own, so its copy (or its
            // cross-GPU bridge blit) scales the whole desktop in, aspect kept.
            let size = self.outputs.iter().find(|o| o.connector.name == target).map(|o| o.size()).unwrap();
            self.size = size;
            for output in &mut self.outputs {
                output.placement = (output.connector.name == target).then_some(wide_desktop::SliceRect { x:0,y:0,width:size[0],height:size[1] });
            }
            return;
        }
        let screens:Vec<_> = self.outputs.iter().map(|o| wide_desktop::ScreenMode::new(&o.connector.name,o.mode.extent.width,o.mode.extent.height)).collect();
        let limits = &self.compositor.properties.limits;
        let layout = wide_desktop::arrange(&screens,&self.order,limits.max_framebuffer_width.min(limits.max_image_dimension2_d),limits.max_framebuffer_height.min(limits.max_image_dimension2_d));
        self.size = [layout.width,layout.height];
        for output in &mut self.outputs { output.placement = layout.rect_of(&output.connector.name); }
    }
    fn primary_index(&self) -> Option<usize> {
        self.primary.as_ref().and_then(|n| self.outputs.iter().position(|o| &o.connector.name == n))
            .or_else(|| self.outputs.iter().position(|o| o.placement.is_some()))
    }
    /// The output whose presents pace the desktop (gives the client its frame credit), from
    /// the pacing setting, resolved again on every call so hotplug, mode and arrangement
    /// changes apply at once (see `pacing_choice`).
    fn pacing_index(&self) -> Option<usize> {
        let main = self.primary_index();
        let mirror = self.mirror_target().is_some();
        let candidates:Vec<_> = self.outputs.iter().enumerate().filter(|(_,o)| mirror || o.placement.is_some()).map(|(i,o)| PacingCandidate {
            index:i, name:&o.connector.name, refresh_mhz:o.mode.refresh_mhz, external:connector_rank(&o.connector.name).0 == 1,
            main:Some(i) == main, rendering_gpu:o.dev.physical == self.compositor.physical,
        }).collect();
        pacing_choice(&self.pacing,mirror,&candidates).map(|(i,_)| i).or(main)
    }
    /// `paced_by` for the inventory: the pacing output, or `None` while every screen runs at
    /// its own refresh (Auto, extended).
    fn paced_by(&self) -> Option<String> {
        let main = self.primary_index();
        let mirror = self.mirror_target().is_some();
        let candidates:Vec<_> = self.outputs.iter().enumerate().filter(|(_,o)| mirror || o.placement.is_some()).map(|(i,o)| PacingCandidate {
            index:i, name:&o.connector.name, refresh_mhz:o.mode.refresh_mhz, external:connector_rank(&o.connector.name).0 == 1,
            main:Some(i) == main, rendering_gpu:o.dev.physical == self.compositor.physical,
        }).collect();
        pacing_choice(&self.pacing,mirror,&candidates).filter(|(_,pick)| *pick).map(|(i,_)| self.outputs[i].connector.name.clone())
    }
    fn publish(&self,state:&State) {
        let main = self.primary_index();
        let mut outputs:Vec<_> = self.outputs.iter().enumerate().map(|(i,o)| LinuxDisplayOutput {
            name:o.connector.name.clone(),width:o.mode.extent.width,height:o.mode.extent.height,refresh_hz:o.mode.refresh_mhz as f64 / 1000.0,
            primary:Some(i) == main,card:card_name(&o.connector.card),desktop_position:o.placement.map(|r| (r.x,r.y)),active:o.last_present != 0,
            status:match (&o.error,o.last_present != 0) {
                (Some(e),_) => format!("requested mode failed, showing {}: {e}",mode_label(&o.mode)),
                (None,true) => "active".into(),
                (None,false) => "ready; waiting for first frame".into(),
            },modes:o.mode.modes.clone(),
            mode_override:self.modes.get(&o.connector.name).cloned(),pci:pci(&o.connector.card),
            error:o.error.clone(),requested:o.requested.clone(),
            native:self.cards.get(&o.connector.card).and_then(|c| drm_connector_modes(c.file.as_raw_fd(),o.connector.id).1),
        }).collect();
        outputs.extend(self.failed.iter().map(|f| f.row.clone())); outputs.sort_by_key(|o| connector_rank(&o.name));
        {
            let mut snapshot = lock(&state.snapshot);
            let arrangement = self.mirror_target().map_or(crate::direct::DisplayArrangement::Extended,|name| crate::direct::DisplayArrangement::Mirror { optimise_for:name.to_owned() });
            let paced_by = self.paced_by();
            snapshot.displays = LinuxDisplaySnapshot { direct:true,outputs,arrangement,pacing:self.pacing.clone(),paced_by }; snapshot.size = self.size;
            if !self.outputs.is_empty() { state.acquired.store(true,Ordering::Release); }
            // A reacquired output has no phase until its new swapchain presents.
            snapshot.timings = self.outputs.iter().filter(|o| o.last_present != 0 && o.mode.refresh_mhz != 0)
                .map(|o| (o.connector.name.clone(), (o.last_present, 1000.0 / o.mode.refresh_mhz as f64))).collect();
            state.changed(&mut snapshot);
        }
    }
    /// The current frame is queued for presentation on the pacing output: the next frame may
    /// be taken and the client may render another. Credit follows the queued present, not the
    /// completed flip, so rendering and the (cross-GPU) transfer of frame N+1 overlap the flip
    /// of frame N instead of adding to it. Each output still holds one flip in flight (FIFO):
    /// a present waits for the previous one, so credit comes at most once per refresh.
    fn queued(&self) -> bool {
        self.current.as_ref().is_none_or(|f| self.pacing_index().is_some_and(|i| self.outputs[i].generation >= f.generation))
    }
    /// Report every frame whose present completed on the pacing output.
    fn notify_presented(&mut self) {
        let Some(index) = self.pacing_index() else { return };
        let (complete,presented_at) = (self.outputs[index].complete,self.outputs[index].last_present);
        while self.presenting.front().is_some_and(|(generation,_)| *generation <= complete) {
            let (_,presented) = self.presenting.pop_front().unwrap();
            presented(presented_at);
        }
    }
    fn step(&mut self,state:&State) -> Result<bool,String> {
        drain_parked_acquires();
        if !self.active { return Ok(false); }
        self.notify_presented();
        if self.queued() {
            if let Some(mut frame) = self.frames.pop_front() {
                crate::profile::since(crate::profile::Metric::DisplayQueue, frame.arrived);
                frame.taken = crate::clock_ns();
                if let Some(presented) = frame.presented.take() { self.presenting.push_back((frame.generation,presented)); }
                self.current = Some(frame);
            }
        }
        let mut first = false;
        let mut timing_changed = false;
        for output in &mut self.outputs {
            let old = output.last_present;
            if output.rebuild { continue; }
            if let Err(e) = output.service(self.current.as_ref()) {
                // One output's presentation failure (cable pulled, VT, driver) rebuilds that
                // output alone; the others keep presenting the retained whole frame.
                eprintln!("[hal-direct] {}: presentation: {e}; rebuilding this output",output.connector.name);
                output.rebuild = true; continue;
            }
            let progress = (output.generation,output.complete,output.acquired,output.acquire_ready);
            if progress != output.progress || !output.busy(self.current.as_ref()) {
                output.progress = progress; output.progress_at = Instant::now();
            } else if output.progress_at.elapsed() >= Duration::from_secs(2) && !output.rebuild {
                // Native acquire watchdog: retry the retained complete frame, with no retry
                // limit and no discarded input/image when WSI stops returning credit.
                eprintln!("[hal-direct] {}: presentation made no progress for 2 s; reacquiring",output.connector.name);
                output.rebuild = true;
            }
            if output.last_present != old {
                first |= old == 0;
                timing_changed = true;
                if output.mode.refresh_mhz != 0 {
                    let value = (output.last_present,1000.0 / output.mode.refresh_mhz as f64);
                    let mut snapshot = lock(&state.snapshot);
                    if let Some(timing) = snapshot.timings.get_mut(&output.connector.name) { *timing = value; }
                    else { snapshot.timings.insert(output.connector.name.clone(),value); }
                }
            }
        }
        if first { self.publish(state); }
        if timing_changed { state.timings_changed.store(true, Ordering::Release); state.shared.signal(); }
        self.notify_presented();
        if self.queued() {
            if self.demand && !self.awaiting && self.frames.is_empty() {
                if let Some(index) = self.pacing_index() {
                    let output = &self.outputs[index];
                    let period = 1_000_000_000_000u64 / u64::from(output.mode.refresh_mhz.max(1));
                    let now = crate::clock_ns(); let phase = output.last_present;
                    let next = if phase == 0 { now + period } else { phase + ((now.saturating_sub(phase) / period) + 1) * period };
                    state.ready.store(next,Ordering::Release); state.shared.signal();
                    self.demand = false; self.awaiting = true;
                }
            }
        }
        Ok(self.outputs.iter().any(|o| o.busy(self.current.as_ref()))
            || (!self.frames.is_empty() && self.pacing_index().is_some()))
    }
}

pub(super) fn run(state:Arc<State>,device:Arc<Device>) {
    let mut context = match Context::new(device) {
        Ok(context) => context,
        Err(e) => {
            eprintln!("[hal-direct] startup: {e}");
            { let mut ops = lock(&state.ops); state.stopped.store(true,Ordering::Release); ops.clear(); }
            state.shared.signal(); return;
        }
    };
    let watch = super::hotplug::Watch::open();
    if watch.is_none() { eprintln!("[hal-direct] DRM uevent socket unavailable; keeping periodic connector discovery"); }
    let mut rescan = true;
    let mut next_scan = Instant::now();
    let mut stop = false;
    while !stop {
        for command in state.take() {
            match command {
                Command::Stop => { stop = true; break; }
                Command::Window(key) => {
                    context.window = key; context.frames.clear(); context.current = None; context.presenting.clear(); context.demand = false; context.awaiting = false;
                }
                // A request after a FrameReady means the client used that beat without
                // submitting (e.g. its first packet waited for glyphs/pipelines): the credit
                // is spent, so the next beat must not wait for a present that never comes.
                Command::Request => { if context.window.is_some() { context.demand = true; context.awaiting = false; } }
                Command::Frame(mut frame) => {
                    if context.window == Some(frame.key) {
                        context.sequence += 1; frame.generation = context.sequence;
                        context.frames.push_back(frame); context.awaiting = false;
                    }
                }
                Command::Pause => context.pause(&state),
                Command::Resume => { context.active = true; state.released.store(false,Ordering::Release); rescan = true; context.demand = true; }
                Command::Mode(name,mode) => {
                    // A mode pick on a failed screen retries it, even when it names the same mode.
                    context.failed.retain(|f| f.connector.name != name);
                    if let Some(mode) = mode { context.modes.insert(name,mode); } else { context.modes.remove(&name); }
                    rescan = true;
                }
                Command::Order(order) => { context.order = order; rescan = true; }
                // A composition change, never a mode change: no output is touched.
                Command::Arrangement(arrangement) => {
                    context.mirror = match arrangement { crate::direct::DisplayArrangement::Mirror { optimise_for } => Some(optimise_for), _ => None };
                    context.recompose(&state);
                }
                Command::Primary(name) => { context.primary = Some(name); context.publish(&state); }
                // No modeset: the next credit follows the newly resolved output.
                Command::Pacing(pacing) => { context.pacing = pacing; context.publish(&state); }
                Command::Device(device) => { context.replace_device(device,&state); rescan = true; }
            }
        }
        if stop { break; }
        rescan |= watch.as_ref().is_some_and(|w| w.changed());
        let now = Instant::now();
        if now >= next_scan {
            next_scan = now + Duration::from_secs(1);
            match scan_drm_connectors(&context.filter) {
                Ok(connectors) => { rescan |= connectors != context.connectors; }
                Err(e) => eprintln!("[hal-direct] rescan: {e}"),
            }
        }
        rescan |= context.outputs.iter().any(|o| o.rebuild);
        if rescan && context.active {
            if let Ok(connectors) = scan_drm_connectors(&context.filter) { context.reconcile(connectors,&state); }
            rescan = false;
        }
        let busy = match context.step(&state) {
            Ok(busy) => busy,
            Err(e) => {
                eprintln!("[hal-direct] presentation: {e}; retaining the whole frame for reacquisition");
                // Recreate display resources on the next turn; retain the source image and
                // outstanding demand. A cable/VT failure is not a lost render device.
                rescan = true; false
            }
        };
        let deadline = if busy { now + Duration::from_micros(500) } else if rescan && context.active { now + Duration::from_millis(1) } else { next_scan };
        let mut fds:Vec<_> = watch.iter().map(|w| io::PollFd { fd:w.fd(),events:io::READ,revents:0 }).collect();
        if let Err(e) = io::wait(state.reader.as_raw_fd(),&mut fds,Some(deadline)) { eprintln!("[hal-direct] wait: {e}"); }
    }
    context.pause(&state);
    drop(context);
    drain_parked_acquires();
    let parked = lock(&PARKED_ACQUIRES).len();
    if parked != 0 { eprintln!("[hal-direct] {parked} pending WSI acquire fence(s) retained until process exit"); }
    {
        let mut snapshot = lock(&state.snapshot);
        *snapshot = Snapshot::default();
        state.changed(&mut snapshot);
    }
    {
        let mut ops = lock(&state.ops);
        state.stopped.store(true,Ordering::Release); ops.clear();
    }
    state.shared.signal();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct::DisplayPacing;
    fn c(index:usize,name:&'static str,hz:u32,external:bool,main:bool,rendering_gpu:bool) -> PacingCandidate<'static> {
        PacingCandidate { index,name,refresh_mhz:hz*1000,external,main,rendering_gpu }
    }
    #[test]
    fn pacing_follows_the_setting_and_the_arrangement() {
        // 149: the 240 Hz panel on the Intel GPU (main), a 30 Hz HDMI monitor on the rendering GPU
        let both = [c(0,"card0-eDP-1",240,false,true,false),c(1,"card1-HDMI-A-2",30,true,false,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,false,&both),Some((0,false)),"extended: each screen at its own refresh");
        assert_eq!(pacing_choice(&DisplayPacing::Auto,true,&both),Some((1,true)),"mirror: the audience screen paces");
        assert_eq!(pacing_choice(&DisplayPacing::Slowest,false,&both),Some((1,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Fastest,true,&both),Some((0,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Screen("card1-HDMI-A-2".into()),false,&both),Some((1,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Screen("card9-DP-1".into()),true,&both),Some((1,true)),"a gone output falls back to Auto");
        let panel = [c(0,"card0-eDP-1",240,false,true,false)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,true,&panel),Some((0,true)),"mirror without an external: the built-in");
        let two_external = [c(0,"card0-eDP-1",60,false,true,true),c(1,"card1-DP-3",144,true,false,true),c(2,"card1-HDMI-A-2",120,true,false,false)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,true,&two_external),Some((2,true)),"mirror: the slowest external, not the slower panel");
        let tie = [c(0,"card0-eDP-1",60,false,false,false),c(1,"card1-DP-3",60,true,true,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Slowest,false,&tie),Some((1,true)),"ties prefer the main screen");
        assert_eq!(pacing_choice(&DisplayPacing::Auto,false,&[]),None);
    }
    fn render_order(devices:&[(bool,vk::PhysicalDeviceType,u8)]) -> Vec<usize> {
        let mut ranked:Vec<_> = devices.iter().enumerate().map(|(i,(selected,kind,builtin))| (render_rank(*selected,*kind,*builtin,i),i)).collect();
        ranked.sort_by_key(|r| r.0);
        ranked.into_iter().map(|r| r.1).collect()
    }
    #[test]
    fn the_discrete_gpu_renders_and_the_panel_gpu_only_scans_out() {
        use vk::PhysicalDeviceType as T;
        // 149: the eDP on the Intel GPU (enumerated first), HDMI on the NVIDIA GPU
        assert_eq!(render_order(&[(false,T::INTEGRATED_GPU,0),(false,T::DISCRETE_GPU,1)])[0],1,"the dGPU renders, the iGPU's eDP is bridged");
        assert_eq!(render_order(&[(false,T::DISCRETE_GPU,1),(false,T::INTEGRATED_GPU,0)])[0],0);
        assert_eq!(render_order(&[(false,T::DISCRETE_GPU,2),(false,T::INTEGRATED_GPU,0)])[0],0,"a dGPU with no outputs still renders");
        assert_eq!(render_order(&[(false,T::INTEGRATED_GPU,0)]),vec![0],"iGPU only: the iGPU");
        assert_eq!(render_order(&[(false,T::DISCRETE_GPU,0)]),vec![0],"dGPU only: the dGPU");
        assert_eq!(render_order(&[(false,T::DISCRETE_GPU,1),(false,T::DISCRETE_GPU,0)])[0],1,"two dGPUs: the panel's one");
        assert_eq!(render_order(&[(false,T::CPU,0),(false,T::INTEGRATED_GPU,1)])[0],1,"never a CPU device while a GPU is there");
        assert_eq!(render_order(&[(false,T::DISCRETE_GPU,1),(true,T::INTEGRATED_GPU,0)])[0],1,"an explicit pin wins");
    }
    fn connector(card:&str,name:&str,id:u32,fingerprint:u64) -> DrmConnector {
        DrmConnector { card:PathBuf::from(format!("/dev/dri/{card}")),name:format!("{card}-{name}"),id,fingerprint }
    }
    #[test]
    fn a_saved_mode_stays_on_its_own_screen() {
        let screens = [connector("card0","eDP-1",1,1),connector("card1","HDMI-A-2",2,2)];
        let modes = HashMap::from([("card0-eDP-1".to_string(),"2560x1600@240".to_string())]);
        let requests = connector_requests(&screens,&modes,None);
        assert_eq!(requests["card0-eDP-1"].as_deref(),Some("2560x1600@240"));
        assert_eq!(requests["card1-HDMI-A-2"],None,"the external screen takes its EDID mode");
    }
    #[test]
    fn the_global_mode_goes_to_the_first_screen_without_its_own() {
        let screens = [connector("card0","eDP-1",1,1),connector("card1","HDMI-A-2",2,2)];
        let modes = HashMap::from([("card0-eDP-1".to_string(),"1920x1200".to_string())]);
        let requests = connector_requests(&screens,&modes,Some("1280x720"));
        assert_eq!(requests["card0-eDP-1"].as_deref(),Some("1920x1200"));
        assert_eq!(requests["card1-HDMI-A-2"].as_deref(),Some("1280x720"));
        let requests = connector_requests(&screens,&HashMap::new(),Some("1280x720"));
        assert_eq!(requests["card0-eDP-1"].as_deref(),Some("1280x720"));
        assert_eq!(requests["card1-HDMI-A-2"],None);
    }
    #[test]
    fn a_working_output_survives_everything_but_its_own_change() {
        assert_eq!(output_fate(true,false,None,None),Fate::Keep);
        assert_eq!(output_fate(true,false,Some("2560x1600@240"),Some("2560x1600@240")),Fate::Keep);
        assert_eq!(output_fate(true,false,None,Some("2560x1600@240")),Fate::Recheck);
        assert_eq!(output_fate(false,false,None,None),Fate::Rebuild,"unplugged");
        assert_eq!(output_fate(true,true,None,None),Fate::Rebuild,"its own presentation failed");
    }
}
