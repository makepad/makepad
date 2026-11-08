//! Whole-target Vulkan display WSI. The driver owns KMS page flips; present-wait or
//! acquire-fence completion supplies frame credit. Idle outputs do not submit frames.
use crate::wide_desktop;
use super::{dma_buf, drm_sys as drm, gpu_bridge::GpuBridge, heads::{CardDriver, Holds, KmsState, PlaneBook, NO_FREE_HEAD}, kms::*,
    Command, Frame, Key, Snapshot, State};
use super::super::{io, lock};
use crate::{backend::vulkan::{buffer_usage, Buffer, Device, Image}, direct::*, session::TextureObj, types::{format, usage}};
use makepad_vulkan::vk;
use std::{collections::{HashMap, HashSet, VecDeque}, fs::{File, OpenOptions},
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd}, path::{Path, PathBuf}, sync::{Arc, atomic::Ordering}, time::{Duration, Instant}};

/// A flip outstanding this long (many refresh periods at any mode) is a stalled head.
const FLIP_STALL: Duration = Duration::from_millis(500);

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
    let connectors = scan_drm_connectors(&DrmFilter::from_env(), |_, _| {})?;
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
struct Card { file: File, saved: Vec<SavedCrtc>, dev: Arc<Device>, holds: Holds<KmsLease> }
/// A DRM lease of one connector and one CRTC (with that CRTC's planes: the master does not
/// take universal planes, so the kernel adds them). Its descriptor is master of those objects
/// alone; the display acquired through it is granted through that descriptor only.
struct KmsLease { lessee: u32, connector: u32, crtc: u32, file: OwnedFd }
/// A card's DRM master descriptor and its GPU, as `Holds` drives them.
struct Kms<'a> { file: &'a File, dev: &'a Device }
impl CardDriver for Kms<'_> {
    type Lease = KmsLease;
    fn kms(&mut self) -> Result<KmsState,String> { drm_kms_state(self.file.as_raw_fd()) }
    fn switch_off(&mut self,crtc:u32) -> Result<(),String> {
        if unsafe { drm::drmModeSetCrtc(self.file.as_raw_fd(),crtc,0,0,0,std::ptr::null_mut(),0,std::ptr::null_mut()) } < 0 {
            return Err(format!("switch off CRTC {crtc}: {}",std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn lease(&mut self,connector:u32,crtc:u32) -> Result<KmsLease,String> {
        let objects = [connector,crtc]; let mut lessee = 0;
        let fd = unsafe { drm::drmModeCreateLease(self.file.as_raw_fd(),objects.as_ptr(),objects.len() as i32,drm::O_CLOEXEC,&mut lessee) };
        if fd < 0 { return Err(format!("lease connector {connector} with CRTC {crtc}: {}",std::io::Error::from_raw_os_error(-fd))); }
        Ok(KmsLease { lessee,connector,crtc,file:unsafe { OwnedFd::from_raw_fd(fd) } })
    }
    fn revoke(&mut self,lease:KmsLease) {
        let result = if unsafe { drm::drmModeRevokeLease(self.file.as_raw_fd(),lease.lessee) } < 0 { format!(": {}",std::io::Error::last_os_error()) } else { String::new() };
        eprintln!("[hal-direct] connector {}: DRM lease {} of CRTC {} revoked{result}",lease.connector,lease.lessee,lease.crtc);
    }
    fn acquire(&mut self,display:vk::DisplayKHR,lease:Option<&KmsLease>) -> Result<(),String> {
        let dev = self.dev;
        match lease {
            Some(lease) => unsafe { dev.instance.acquire_drm_display(dev.physical,lease.file.as_raw_fd(),display) }.map_err(error),
            None => {
                // The one-descriptor WSI owns and closes the descriptor it acquires through.
                let duplicate = self.file.try_clone().map_err(|e| e.to_string())?;
                unsafe { dev.instance.acquire_drm_display(dev.physical,duplicate.as_raw_fd(),display) }.map_err(error)?;
                let _ = duplicate.into_raw_fd();
                Ok(())
            }
        }
    }
    fn release(&mut self,display:vk::DisplayKHR) {
        let result = unsafe { (self.dev.instance.table.release_display_ext.expect("VK_EXT_direct_mode_display"))(self.dev.physical,display) };
        if result != vk::Result::SUCCESS { eprintln!("[hal-direct] release display: {result:?}"); }
    }
}
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
        Ok(Self { file,saved,dev:device.clone(),holds:Holds::new(mesa) })
    }
    /// Hold `display`; `driven`: the card's other connectors the WM drives. Returns the
    /// turned-off displays given back for its head.
    fn acquire(&mut self,display:vk::DisplayKHR,connector:u32,driven:&HashSet<u32>) -> Result<Vec<vk::DisplayKHR>,String> {
        self.holds.acquire(&mut Kms { file:&self.file,dev:&self.dev },display,connector,driven)
    }
    /// Give up one display: its lease, if it has one, is revoked; every other display keeps its own.
    fn release_display(&mut self,display:vk::DisplayKHR) {
        self.holds.release(&mut Kms { file:&self.file,dev:&self.dev },display)
    }
    fn release(&mut self) {
        self.holds.release_all(&mut Kms { file:&self.file,dev:&self.dev })
    }
    /// How a held display is held, for the log: its lease and CRTC, or the card's shared descriptor.
    fn holding(&self,display:vk::DisplayKHR) -> String {
        match self.holds.displays.iter().find(|h| h.display == display).and_then(|h| h.lease()) {
            Some(lease) => format!("CRTC {} through DRM lease {}",lease.crtc,lease.lessee),
            None => "the card's shared descriptor".into(),
        }
    }
}
impl Drop for Card {
    fn drop(&mut self) {
        self.release();
        let fd = self.file.as_raw_fd();
        // NVIDIA's display release gives the heads back to the console itself; a legacy
        // SetCrtc of the saved state on top fails (EINVAL) and leaves the head mid-commit for
        // the next master (the driver's flip event timeout, then nv_flip == NULL)
        let restore = if self.holds.mesa { std::mem::take(&mut self.saved) } else { Vec::new() };
        for mut saved in restore {
            let c = &mut saved.crtc;
            // the state saved at start shows the previous master's framebuffer when that was
            // another desktop (a WM restart): it is gone with its process, and setting it fails
            // (EINVAL) and leaves the head mid-flip for the next master (the driver's flip event
            // timeout). Nothing to give back then: the next master sets its own state.
            if c.buffer_id != 0 {
                let fb = unsafe { drm::drmModeGetFB(fd,c.buffer_id) };
                if fb.is_null() {
                    eprintln!("[hal-direct] CRTC {}: the saved framebuffer {} is gone (the previous desktop's); not restored",c.crtc_id,c.buffer_id);
                    continue;
                }
                unsafe { drm::drmModeFreeFB(fb); }
            }
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
    fn submit(&mut self,signal:Option<vk::Semaphore>,image:Arc<Image>) -> Result<(),String> {
        unsafe {
            self.dev.raw.end_command_buffer(self.cb).map_err(error)?;
            self.dev.raw.reset_fences(&[self.fence]).map_err(error)?;
            let commands = [self.cb]; let signals:Vec<_> = signal.into_iter().collect();
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
/// screen in the wide desktop (the whole frame without one, or while a frame of an older
/// size is retained), never a repaint region. It is shown scaled to fit, aspect kept and
/// letterboxed (`wide_desktop::fit`; only then is the destination cleared first): an output of
/// the screen's own size gets it 1:1, a mirror output of another size a smoothly (linearly)
/// filtered blit. Unscaled images of the same format are copied, not blitted: a copy is the
/// cheaper GPU path, and across GPUs half the bytes of a clear plus a blit.
/// `same_format`: both images have the same Vulkan format.
fn blit_full(dev:&Device,cb:vk::CommandBuffer,source:vk::Image,source_layout:vk::ImageLayout,source_size:[u32;2],
    destination:vk::Image,destination_layout:vk::ImageLayout,size:[u32;2],placement:Option<wide_desktop::SliceRect>,same_format:bool) -> ScanoutOp {
    let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
    let r = placement.filter(|r| r.x + r.width <= source_size[0] && r.y + r.height <= source_size[1])
        .unwrap_or(wide_desktop::SliceRect { x:0,y:0,width:source_size[0],height:source_size[1] });
    let d = wide_desktop::fit((r.width,r.height),(size[0],size[1]));
    let (src,dst) = ([r.x,r.y,r.width,r.height],[d.x,d.y,d.width,d.height]);
    let unscaled = src[2] == dst[2] && src[3] == dst[3];
    let offsets = |r:[u32;4]| [vk::Offset3D { x:r[0] as i32,y:r[1] as i32,z:0 },vk::Offset3D { x:(r[0]+r[2]) as i32,y:(r[1]+r[3]) as i32,z:1 }];
    unsafe {
        if dst != [0,0,size[0],size[1]] {
            dev.raw.cmd_clear_color_image(cb,destination,destination_layout,&vk::ClearColorValue { float32:[0.0,0.0,0.0,1.0] },&[range()]);
            transition(dev,cb,destination,destination_layout,destination_layout);
        }
        if same_format && unscaled {
            let copy = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub)
                .src_offset(offsets(src)[0]).dst_offset(offsets(dst)[0]).extent(vk::Extent3D { width:dst[2],height:dst[3],depth:1 });
            dev.raw.cmd_copy_image(cb,source,source_layout,destination,destination_layout,&[copy]);
        } else {
            let blit = vk::ImageBlit::default().src_subresource(sub).dst_subresource(sub).src_offsets(offsets(src)).dst_offsets(offsets(dst));
            // Scaled either way (a mirror output of another size, a virtual desktop) is filtered:
            // nearest would drop or double whole pixel columns (text strokes).
            let filter = if unscaled { vk::Filter::NEAREST } else { vk::Filter::LINEAR };
            dev.raw.cmd_blit_image(cb,source,source_layout,destination,destination_layout,&[blit],filter);
        }
    }
    ScanoutOp { source: src, destination: dst, copied: same_format && unscaled }
}

/// What one `blit_full` did: the source rectangle, where it landed in the destination, and
/// whether it was a 1:1 copy (else a blit, which scales when the two rectangles differ).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScanoutOp { pub source: [u32;4], pub destination: [u32;4], pub copied: bool }

/// `cap scanout` (framecap): one output's next presented image, read back from the swapchain
/// image after the presenter's copy into it, i.e. the pixels the display plane scans out.
pub(crate) struct Scanout {
    pub output: String, pub mode: [u32;2], pub format: vk::Format,
    /// the desktop frame the copy read (its size), and the copy itself
    pub frame: [u32;2], pub op: ScanoutOp,
    /// across GPUs: the frame first crossed the bridge into a local image of the mode's size
    pub bridged: bool,
    /// RGBA8 rows of `mode` (10-bit formats are reduced to 8 bits)
    pub rgba: Vec<u8>,
    /// the desktop frame itself, RGBA8 rows of `frame`, read in the same submission (same GPU only)
    pub frame_rgba: Option<Vec<u8>>,
}
type ScanoutReply = std::sync::mpsc::Sender<Result<Scanout,String>>;
static SCANOUT: std::sync::Mutex<Vec<ScanoutReply>> = std::sync::Mutex::new(Vec::new());

/// Ask for the next presented image of any output (the first that presents answers).
pub(crate) fn request_scanout() -> std::sync::mpsc::Receiver<Result<Scanout,String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    lock(&SCANOUT).push(tx);
    rx
}

/// Swapchain pixels as RGBA8.
fn scanout_rgba(format: vk::Format, bytes: &[u8]) -> Result<Vec<u8>,String> {
    let mut out = Vec::with_capacity(bytes.len());
    for p in bytes.chunks_exact(4) {
        match format {
            vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => out.extend_from_slice(&[p[2],p[1],p[0],255]),
            vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_UNORM_PACK32 => out.extend_from_slice(&[p[0],p[1],p[2],255]),
            vk::Format::A2B10G10R10_UNORM_PACK32 => {
                let v = u32::from_le_bytes([p[0],p[1],p[2],p[3]]);
                let c = |shift: u32| (((v >> shift) & 0x3ff) >> 2) as u8;
                out.extend_from_slice(&[c(0),c(10),c(20),255]);
            }
            other => return Err(format!("scanout readback: no decoder for {other:?}")),
        }
    }
    Ok(out)
}

struct Peer {
    source: Arc<Device>, destination: Arc<Device>, bridge: Option<GpuBridge>, local: Arc<Image>,
    sent: u64, received: u64, keep: Option<Arc<Image>>, valid: bool,
    /// the last frame's copy into the bridge (`cap scanout` reports it)
    sent_op: Option<ScanoutOp>,
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
            usage:usage::RENDER_TARGET | usage::SAMPLED,bytes:0,imported:false,uploaded:0,lent:None })?.in_pool(crate::ledger::Pool::WindowCopies);
        let bridge = GpuBridge::new(&source,&destination,vk::Extent2D { width:size[0],height:size[1] })?;
        Ok(Self { source,destination,bridge:Some(bridge),local,sent:0,received:0,keep:None,valid:false,sent_op:None,sent_at:0,received_at:0 })
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
                let mut op = None;
                bridge.send_commands(&self.source,|cb,shared| {
                    op = Some(blit_full(&self.source,cb,image.image,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,[image.desc.w,image.desc.h],shared,vk::ImageLayout::GENERAL,[size.width,size.height],placement,true)); Ok(())
                })?;
                self.sent_op = op;
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
    /// Why the mode shown is not the one asked for: the requested mode is unavailable or
    /// failed, or a faster one failed to present.
    error:Option<String>, plane:u32, mode:DisplayMode, format:vk::Format,
    surface:vk::SurfaceKHR, chain:vk::SwapchainKHR, images:Vec<vk::Image>, signals:Vec<vk::Semaphore>,
    acquire:vk::Fence, acquired:Option<u32>, acquire_ready:bool, job:CopyJob, peer:Option<Peer>,
    placement:Option<wide_desktop::SliceRect>, generation:u64, serial:u64, pending:Option<u64>,
    complete:u64, last_present:u64, rebuild:bool,
    /// Why its presentation failed (set with `rebuild`; a suboptimal swapchain sets none).
    failure:Option<String>,
    /// The swapchain images can be copied from (`cap scanout`).
    readable:bool,
    progress:(u64,u64,Option<u32>,bool), progress_at:Instant,
    /// profile: the present's submit, the cross-GPU receive's submit
    present_at:u64, peer_at:u64,
}
impl Output {
    fn new(dev:Arc<Device>,compositor:Arc<Device>,connector:DrmConnector,display:vk::DisplayKHR,requested:Option<String>,mode:DisplayMode,plane:PlaneCandidate) -> Result<Self,String> {
        let job = CopyJob::new(dev.clone())?;
        let mut out = Self { dev,connector,display,requested,error:None,plane:plane.plane_index,mode,format:vk::Format::UNDEFINED,surface:vk::SurfaceKHR::null(),chain:vk::SwapchainKHR::null(),images:Vec::new(),signals:Vec::new(),
            acquire:vk::Fence::null(),acquired:None,acquire_ready:false,job,peer:None,placement:None,generation:0,serial:0,pending:None,complete:0,last_present:0,rebuild:false,failure:None,readable:false,
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
        out.readable = caps.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_SRC);
        let usage = if out.readable { vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC } else { vk::ImageUsageFlags::TRANSFER_DST };
        let count = if caps.max_image_count == 0 { caps.min_image_count + 1 } else { (caps.min_image_count+1).min(caps.max_image_count) };
        let alpha = [vk::CompositeAlphaFlagsKHR::OPAQUE,vk::CompositeAlphaFlagsKHR::INHERIT].into_iter().find(|a| caps.supported_composite_alpha.contains(*a)).ok_or("display has no opaque composite alpha")?;
        unsafe {
            out.chain = out.dev.raw.create_swapchain(&vk::SwapchainCreateInfoKHR::default().surface(out.surface).min_image_count(count)
                .image_format(format.format).image_color_space(format.color_space).image_extent(out.mode.extent).image_array_layers(1)
                .image_usage(usage).image_sharing_mode(vk::SharingMode::EXCLUSIVE)
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
        let op = blit_full(&self.dev,self.job.cb,image.image,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,[image.desc.w,image.desc.h],destination,vk::ImageLayout::TRANSFER_DST_OPTIMAL,self.size(),placement,self.format == vk::Format::B8G8R8A8_UNORM);
        let shot = self.record_scanout(destination,frame,op)?;
        transition(&self.dev,self.job.cb,destination,if shot.is_some() { vk::ImageLayout::TRANSFER_SRC_OPTIMAL } else { vk::ImageLayout::TRANSFER_DST_OPTIMAL },vk::ImageLayout::PRESENT_SRC_KHR);
        let ready = self.signals[index as usize];
        let frame_format = image.desc.format;
        self.job.submit(Some(ready),image)?;
        if let Some((reply,staging,scanout)) = shot { self.finish_scanout(reply,staging,scanout,frame_format); }
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
    /// A pending `cap scanout`: after the copy into the swapchain image, that image is copied
    /// on into a host buffer in the same submission (read once its fence signals).
    fn record_scanout(&self,destination:vk::Image,frame:&Frame,op:ScanoutOp) -> Result<Option<(ScanoutReply,[Option<Arc<Buffer>>;2],Scanout)>,String> {
        let Some(reply) = lock(&SCANOUT).pop() else { return Ok(None); };
        if !self.readable {
            let _ = reply.send(Err(format!("{}: the display surface does not offer TRANSFER_SRC swapchain images",self.connector.name)));
            return Ok(None);
        }
        let [w,h] = self.size();
        let staging = Buffer::scratch(self.dev.clone(),crate::ledger::Pool::Staging,w as u64 * h as u64 * 4,buffer_usage())?;
        transition(&self.dev,self.job.cb,destination,vk::ImageLayout::TRANSFER_DST_OPTIMAL,vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
            .image_extent(vk::Extent3D { width:w,height:h,depth:1 });
        // the frame the copy read, on the same GPU: for the pixel-identity check
        let source = if self.peer.is_none() && matches!(frame.image.desc.format,format::BGRA8 | format::RGBA8) {
            let [fw,fh] = [frame.image.desc.w,frame.image.desc.h];
            let staging = Buffer::scratch(self.dev.clone(),crate::ledger::Pool::Staging,fw as u64 * fh as u64 * 4,buffer_usage())?;
            let region = region.image_extent(vk::Extent3D { width:fw,height:fh,depth:1 });
            unsafe { self.dev.raw.cmd_copy_image_to_buffer(self.job.cb,frame.image.image,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,staging.buffer,&[region]); }
            Some(staging)
        } else { None };
        unsafe {
            self.dev.raw.cmd_copy_image_to_buffer(self.job.cb,destination,vk::ImageLayout::TRANSFER_SRC_OPTIMAL,staging.buffer,&[region]);
            self.dev.raw.cmd_pipeline_barrier(self.job.cb,vk::PipelineStageFlags::TRANSFER,vk::PipelineStageFlags::HOST,vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ)],&[],&[]);
        }
        let (frame_size,op,bridged) = match &self.peer {
            Some(peer) => ([frame.image.desc.w,frame.image.desc.h],peer.sent_op.unwrap_or(op),true),
            None => ([frame.image.desc.w,frame.image.desc.h],op,false),
        };
        Ok(Some((reply,[Some(staging),source],Scanout { output:self.connector.name.clone(),mode:[w,h],format:self.format,frame:frame_size,op,bridged,rgba:Vec::new(),frame_rgba:None })))
    }
    /// Debug tooling only: the presenter waits for this one submission (a few ms).
    fn finish_scanout(&self,reply:ScanoutReply,staging:[Option<Arc<Buffer>>;2],mut scanout:Scanout,frame_format:u32) {
        let result = (|| {
            unsafe { self.dev.raw.wait_for_fences(&[self.job.fence],true,2_000_000_000) }.map_err(error)?;
            let [out,source] = staging;
            scanout.rgba = scanout_rgba(self.format,&out.unwrap().read(scanout.mode[0] as usize * scanout.mode[1] as usize * 4)?)?;
            if let Some(source) = source {
                let bytes = source.read(scanout.frame[0] as usize * scanout.frame[1] as usize * 4)?;
                let vk_format = if frame_format == format::BGRA8 { vk::Format::B8G8R8A8_UNORM } else { vk::Format::R8G8B8A8_UNORM };
                scanout.frame_rgba = Some(scanout_rgba(vk_format,&bytes)?);
            }
            Ok(scanout)
        })();
        let _ = reply.send(result);
    }
    fn busy(&self,frame:Option<&Frame>) -> bool {
        self.job.pending || self.pending.is_some() || (self.acquired.is_some() && !self.acquire_ready)
            || frame.is_some_and(|f| f.generation != self.generation)
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        // Executed on the display worker, never under the HAL or shell state locks.
        // The last flip completes before its swapchain (and the display) goes: a flip still in
        // flight when the display is released completes into freed driver state (nvidia-drm:
        // "nv_flip == NULL", then a flip event timeout on the head's next commit).
        if self.dev.present_wait && self.serial > 0 {
            let _ = unsafe { self.dev.raw.wait_for_present(self.chain,self.serial,200_000_000) };
        }
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

/// One output that shows a screen of the desktop, as the pacing choice sees it.
struct PacingCandidate<'a> { index:usize, name:&'a str, refresh_mhz:u32, screen:usize,
    /// its screen's optimise-for output (which paces that screen)
    optimise:bool, main:bool, rendering_gpu:bool }

/// Which output paces the desktop, and whether that is the setting's own pick (`true`) or
/// Auto's "each screen at its own refresh" (`false`, reported as `paced_by: None`). Every
/// other output shows the newest frame at its own refresh and never causes a render.
/// `Auto`: each screen is paced by its optimise-for output (a mirrored screen's other outputs
/// show the newest frame at their own refresh); one screen -- it paces, reported when it is
/// mirrored; several -- credit follows the fastest of them, so each screen runs at its own
/// refresh (the panel is not held back, a slow monitor presents only the frames it can show).
/// Ties prefer the main screen, then the rendering GPU (no cross-GPU copy on the pacing path),
/// then the first. A named output that is gone falls back to Auto.
fn pacing_choice(pacing:&crate::direct::DisplayPacing,outputs:&[PacingCandidate]) -> Option<(usize,bool)> {
    use crate::direct::DisplayPacing;
    let tie = |c:&PacingCandidate| (!c.main,!c.rendering_gpu,c.index);
    let slowest = || outputs.iter().min_by_key(|c| (c.refresh_mhz,tie(c))).map(|c| c.index);
    let fastest = |only_pacers:bool| outputs.iter().filter(|c| !only_pacers || c.optimise)
        .max_by_key(|c| (c.refresh_mhz,std::cmp::Reverse(tie(c)))).map(|c| c.index);
    let auto = || {
        let first = outputs.first()?.screen;
        if outputs.iter().all(|c| c.screen == first) {
            let mirrored = outputs.len() > 1;
            outputs.iter().find(|c| c.optimise).map(|c| (c.index,mirrored))
        } else { fastest(true).map(|i| (i,false)) }
    };
    match pacing {
        DisplayPacing::Auto => auto(),
        DisplayPacing::Slowest => slowest().map(|i| (i,true)),
        DisplayPacing::Fastest => fastest(false).map(|i| (i,true)),
        DisplayPacing::Screen(name) => outputs.iter().find(|c| c.name == name).map(|c| (c.index,true)).or_else(auto),
    }
}

/// A screen of the desktop over the live outputs (see `DisplayArrangement::group`).
struct Screen {
    name:String,
    /// indices into `Context::outputs`, arranged order
    outputs:Vec<usize>,
    /// the output it is sized for (one of `outputs`)
    optimise_for:usize,
    /// its rectangle of the wide desktop; `None` when it does not fit the GPU's limits
    rect:Option<wide_desktop::SliceRect>,
}

/// The output a screen is optimised for: the arranged one when it is live, else the largest
/// (then the first).
fn optimise_for(group:&crate::direct::ScreenGroup,sizes:&[[u32;2]]) -> usize {
    group.optimise_for.unwrap_or_else(|| {
        let area = |i:&usize| u64::from(sizes[*i][0]) * u64::from(sizes[*i][1]);
        let best = group.outputs.iter().map(area).max().unwrap_or(0);
        *group.outputs.iter().find(|i| area(i) == best).unwrap()
    })
}

/// The desktop's screens over live outputs of `sizes` (their modes, in output order) named
/// `names`, laid out within the GPU's limits: the desktop size, the screens and each output's
/// rectangle (its screen's).
fn arrange_screens(arrangement:&crate::direct::DisplayArrangement,names:&[&str],sizes:&[[u32;2]],max:[u32;2])
    -> ([u32;2],Vec<Screen>,Vec<Option<wide_desktop::SliceRect>>) {
    let groups = arrangement.group(names);
    let modes:Vec<_> = groups.iter().map(|g| { let o = optimise_for(g,sizes); wide_desktop::ScreenMode::new(&g.name,sizes[o][0],sizes[o][1]) }).collect();
    let layout = wide_desktop::arrange(&modes,&[],max[0],max[1]);
    let screens:Vec<Screen> = groups.into_iter().map(|g| Screen { rect:layout.rect_of(&g.name),optimise_for:optimise_for(&g,sizes),name:g.name,outputs:g.outputs }).collect();
    let mut placements = vec![None; names.len()];
    for screen in &screens { for &i in &screen.outputs { placements[i] = screen.rect; } }
    ([layout.width,layout.height],screens,placements)
}

/// A connector that could not be driven, with the request it failed under. It is
/// retried only when the connector itself changes (replug, another monitor), its mode
/// request changes or, when it found no free head, a head may have come free; never on a
/// timer: a retry must not disturb working outputs.
struct Failed { connector:DrmConnector, requested:Option<String>, row:LinuxDisplayOutput,
    /// It found no free display head or plane: retried also when one may have come free (a
    /// monitor unplugged or turned off).
    wants_head:bool }

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

/// The virtual display's head (see `VirtualDisplay`): no connector, no swapchain. A frame
/// presents on the first tick of the display's own clock after the GPU finished rendering it
/// (an empty submission's fence on the render queue), one frame in flight: the desktop is paced
/// like a FIFO panel at the virtual refresh, and the client never runs ahead of the GPU.
struct VirtualHead {
    display:VirtualDisplay, job:CopyJob,
    generation:u64, pending:Option<u64>, complete:u64, last_present:u64,
    /// The tick the pending frame shows at (once its fence has signalled).
    due:u64,
    /// The clock's phase: the ticks are `phase + k * period`.
    phase:u64,
}
impl VirtualHead {
    fn new(dev:Arc<Device>,display:VirtualDisplay) -> Result<Self,String> {
        eprintln!("[hal-direct] virtual display {}x{} {:.3} Hz, panels {}",display.width,display.height,display.refresh_hz(),display.panels.as_str());
        Ok(Self { display,job:CopyJob::new(dev)?,generation:0,pending:None,complete:0,last_present:0,due:0,phase:crate::clock_ns() })
    }
    fn period(&self) -> u64 { 1_000_000_000_000 / u64::from(self.display.refresh_mhz.max(1)) }
    /// The first tick after `t`.
    fn tick_after(&self,t:u64) -> u64 { let p = self.period(); self.phase + (t.saturating_sub(self.phase) / p + 1) * p }
    /// A size or panels change keeps the clock; a refresh change starts it again from now.
    fn change(&mut self,display:VirtualDisplay) {
        if display.refresh_mhz != self.display.refresh_mhz { self.phase = crate::clock_ns(); }
        if display != self.display {
            eprintln!("[hal-direct] virtual display {}x{} {:.3} Hz, panels {}",display.width,display.height,display.refresh_hz(),display.panels.as_str());
        }
        self.display = display;
    }
    fn service(&mut self,frame:Option<&Frame>) -> Result<(),String> {
        let now = crate::clock_ns();
        if let Some(generation) = self.pending {
            if now < self.due { return Ok(()); }
            // Not rendered by its tick: it shows on the first tick after the GPU is done.
            if !self.job.done()? { self.due = self.tick_after(now); return Ok(()); }
            self.pending = None; self.complete = generation;
            if crate::profile::on() && self.last_present != 0 {
                crate::profile::record(crate::profile::Metric::PresentInterval,self.due - self.last_present);
            }
            self.last_present = self.due;
        }
        let Some(frame) = frame.filter(|f| f.generation != self.generation) else { return Ok(()); };
        self.job.begin()?;
        self.job.submit(None,frame.image.clone())?;
        self.due = self.tick_after(now.max(self.last_present));
        self.generation = frame.generation; self.pending = Some(frame.generation);
        Ok(())
    }
}

/// What paces the desktop: the pacing output's present state, or the virtual head's.
#[derive(Clone,Copy)]
struct Pace { generation:u64, complete:u64, last_present:u64, refresh_mhz:u32 }

/// `MAKEPAD_VIRTUAL_DISPLAY`: `WxH@Hz [mirror|off]` (default mirror), set by the session from
/// the saved display layout; empty or absent = the panels' own modes.
fn virtual_from_env() -> Option<VirtualDisplay> {
    let value = std::env::var("MAKEPAD_VIRTUAL_DISPLAY").ok()?;
    let mut words = value.split_whitespace();
    let mode = words.next()?;
    match VirtualDisplay::parse(mode,words.next().unwrap_or("mirror")) {
        Ok(display) => Some(display),
        Err(e) => { eprintln!("[hal-direct] MAKEPAD_VIRTUAL_DISPLAY={value:?}: {e}; the panels' own modes stand"); None }
    }
}

struct Context {
    compositor:Arc<Device>, devices:HashMap<vk::PhysicalDevice,Arc<Device>>, physical:Vec<vk::PhysicalDevice>,
    cards:HashMap<PathBuf,Card>, outputs:Vec<Output>, failed:Vec<Failed>,
    /// The display planes this session holds (plane -> display), across reconciles.
    planes:PlaneBook,
    /// Per connector, the last mode that showed a frame: a last resort of its mode ladder.
    working:Working,
    /// Per connector, the mode that failed to present: its ladder starts below it.
    demotions:Demotions,
    /// How the outputs make up the screens (see `DisplayArrangement`), and the screens it
    /// gives over the live outputs (`layout`).
    arrangement:DisplayArrangement, screens:Vec<Screen>,
    /// `MAKEPAD_DISPLAY_ARRANGEMENT` pinned the arrangement: no saved profile applies.
    arrangement_pinned:bool,
    /// The saved profiles' file (`display_profiles::layout_path`), read whenever the
    /// set of connected monitors changes, and the set they were last looked up for.
    profiles:Option<PathBuf>, monitors:Option<Vec<u64>>,
    /// Connectors whose mode `MAKEPAD_DRM_MODES` pinned: a profile leaves them alone.
    modes_pinned:HashSet<String>,
    pacing:crate::direct::DisplayPacing, connectors:Vec<DrmConnector>,
    filter:DrmFilter, modes:HashMap<String,String>, mode:Option<String>,
    /// An output of the main screen.
    primary:Option<String>,
    size:[u32;2], active:bool, window:Option<Key>, current:Option<Frame>, frames:VecDeque<Frame>,
    /// Frames handed to the outputs whose present has not completed on the pacing output yet,
    /// with the callback that reports it (oldest first).
    presenting:VecDeque<(u64,super::Presented)>,
    sequence:u64, demand:bool, awaiting:bool, credit_due:u64,
    /// The virtual display while it is the desktop (see `VirtualDisplay`).
    virtual_head:Option<VirtualHead>,
}
impl Context {
    fn scan_connectors(&self, probe: bool) -> Result<Vec<DrmConnector>, String> {
        let mut files = HashMap::new();
        scan_drm_connectors(&self.filter, |path, id| {
            if !probe { return; }
            // A fresh descriptor cannot force detection while this process's
            // existing descriptor owns DRM master. Reuse the owner, including
            // for disconnected connectors that have no Vulkan display yet.
            let fd = if let Some(card) = self.cards.get(path) {
                card.file.as_raw_fd()
            } else {
                let file = files.entry(path.to_owned()).or_insert_with(|| {
                    let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
                    (unsafe { drm::drmSetMaster(file.as_raw_fd()) } == 0).then_some(file)
                });
                let Some(file) = file else { return; };
                file.as_raw_fd()
            };
            // libdrm's non-Current query forces detection for the DRM master.
            // This can block, so it belongs on the display worker and runs
            // only on startup, VT resume or an actual hotplug notification.
            let connector = unsafe { drm::drmModeGetConnector(fd, id) };
            if !connector.is_null() { unsafe { drm::drmModeFreeConnector(connector); } }
        })
    }
    fn new(compositor:Arc<Device>) -> Result<Self,String> {
        let physical = unsafe { compositor.instance.enumerate_physical_devices() }.map_err(error)?;
        let mut mode = std::env::var("MAKEPAD_DRM_MODE").ok().filter(|m| !m.is_empty() && m != "auto");
        for arg in std::env::args() { if let Some(value) = arg.strip_prefix("-mode=").or_else(|| arg.strip_prefix("--mode=")) { mode = (value != "auto").then(|| value.to_owned()); } }
        let modes:HashMap<String,String> = wide_desktop::parse_mode_overrides(&std::env::var("MAKEPAD_DRM_MODES").unwrap_or_default()).into_iter().collect();
        let modes_pinned = modes.keys().cloned().collect();
        let pinned = std::env::var("MAKEPAD_DISPLAY_ARRANGEMENT").ok().filter(|v| !v.trim().is_empty()).and_then(|text| match DisplayArrangement::parse(&text) {
            Ok(arrangement) => Some(arrangement),
            Err(e) => { eprintln!("[hal-direct] MAKEPAD_DISPLAY_ARRANGEMENT={text:?}: {e}; not pinned"); None }
        });
        let profiles = crate::display_profiles::layout_path();
        let virtual_head = virtual_from_env().map(|d| VirtualHead::new(compositor.clone(),d)).transpose()?;
        Ok(Self { devices:HashMap::from([(compositor.physical,compositor.clone())]),compositor,physical,
            cards:HashMap::new(),outputs:Vec::new(),failed:Vec::new(),planes:PlaneBook::default(),working:Working::default(),demotions:Demotions::default(),
            arrangement_pinned:pinned.is_some(),arrangement:pinned.unwrap_or_default(),screens:Vec::new(),profiles,monitors:None,modes_pinned,
            pacing:Default::default(),connectors:Vec::new(),filter:DrmFilter::from_env(),
            modes,mode,primary:None,size:[0,0],active:true,window:None,current:None,frames:VecDeque::new(),presenting:VecDeque::new(),sequence:0,demand:false,awaiting:false,credit_due:0,
            virtual_head })
    }
    /// The HAL replaced its lost GPU device. The outputs' swapchains, surfaces, copy jobs and
    /// peers lived on the old device and are dropped; the cards keep DRM master and their
    /// display acquisitions (a release would hand the heads to the console), so the next
    /// reconcile rebuilds every output on the new device. The frames of the old device are let
    /// go (their presents reported, so no credit is lost) and the window asks for a new one.
    fn replace_device(&mut self,device:Arc<Device>,state:&State) {
        eprintln!("[hal-direct] the render GPU device was replaced: rebuilding the outputs");
        self.outputs.clear();
        self.frames.clear(); self.current = None;
        let now = crate::clock_ns();
        for (_,presented) in self.presenting.drain(..) { presented(now); }
        let old = self.compositor.physical;
        for card in self.cards.values_mut() { if card.dev.physical == old { card.dev = device.clone(); } }
        self.devices.insert(device.physical,device.clone());
        if let Some(head) = self.virtual_head.take() {
            match VirtualHead::new(device.clone(),head.display) {
                Ok(head) => self.virtual_head = Some(head),
                Err(e) => eprintln!("[hal-direct] virtual display on the new device: {e}"),
            }
        }
        self.compositor = device;
        self.connectors.clear(); self.failed.clear();
        state.ready.store(0,Ordering::Release);
        self.demand = self.window.is_some(); self.awaiting = false;
    }
    fn drain_outputs(&mut self) {
        self.outputs.clear(); self.planes.clear();
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
    fn fail_row(&mut self,connector:&DrmConnector,requested:Option<String>,reason:String,wants_head:bool) {
        let retry = if wants_head { "replugged, its mode changes, or a monitor is unplugged or turned off" } else { "replugged or its mode changes" };
        eprintln!("[hal-direct] {}: {reason}; retried when the connector is {retry}",connector.name);
        // The dropdown stays usable on a failed screen: list its DRM (EDID) modes.
        let (modes,native) = match self.cards.get(&connector.card) {
            Some(card) => drm_connector_modes(card.file.as_raw_fd(),connector.id),
            None => OpenOptions::new().read(true).write(true).open(&connector.card).map(|f| drm_connector_modes(f.as_raw_fd(),connector.id)).unwrap_or_default(),
        };
        let row = LinuxDisplayOutput { name:connector.name.clone(),monitor:connector.monitor.clone(),fingerprint:connector.fingerprint,
            card:card_name(&connector.card),status:reason.clone(),error:Some(reason),
            requested:requested.clone(),modes,native,pci:pci(&connector.card),mode_override:self.modes.get(&connector.name).cloned(),..Default::default() };
        self.failed.push(Failed { connector:connector.clone(),requested,row,wants_head });
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
    /// request changes, and one that found no free head also when a monitor is unplugged
    /// or turned off. Returns without touching anything when nothing changed.
    fn reconcile(&mut self,connectors:Vec<DrmConnector>,state:&State) {
        self.restore_profile(&connectors);
        if self.panels_off() {
            // The virtual display is the desktop and the panels show nothing: no display is
            // held (each goes back to the console), no swapchain, no scanout.
            let changed = !self.outputs.is_empty() || !self.cards.is_empty() || connectors != self.connectors || !self.failed.is_empty();
            if !self.outputs.is_empty() { eprintln!("[hal-direct] the virtual display is the desktop: the panels are not driven"); }
            self.drain_outputs(); self.cards.clear(); self.failed.clear();
            self.connectors = connectors;
            if changed { self.layout(); self.publish(state); }
            return;
        }
        self.demotions.keep(&connectors); self.working.keep(&connectors);
        let requests = connector_requests(&connectors,&self.modes,self.mode.as_deref());
        let requested = |name:&str| requests.get(name).cloned().flatten();
        let mut kept = Vec::new(); let mut retired = Vec::new();
        for mut output in std::mem::take(&mut self.outputs) {
            let request = requested(&output.connector.name);
            if self.arrangement.is_off(&output.connector.name) {
                // Turned off: no swapchain, no scanout. A connector that stays keeps its display
                // held, dark, with its head and plane (parked below): turning it on again needs
                // no new grant. Its head goes to another connector only when that finds none free.
                eprintln!("[hal-direct] {}: turned off",output.connector.name);
                retired.push(output); continue;
            }
            match output_fate(connectors.contains(&output.connector),output.rebuild,output.requested.as_deref(),request.as_deref()) {
                Fate::Keep => kept.push(output),
                Fate::Recheck => {
                    // A new request that names the mode already shown (e.g. the saved
                    // layout restating the running mode at start-up) changes nothing.
                    let native = self.cards.get(&output.connector.card).and_then(|c| drm_native_resolution(c.file.as_raw_fd(),output.connector.id));
                    let same = display_mode_ladder(&output.dev.instance,output.dev.physical,output.display,request.as_deref(),native,None,self.demotions.below(&output.connector))
                        .is_ok_and(|(ladder,_)| ladder[0].mode == output.mode.mode);
                    if same { output.requested = request; kept.push(output); } else { retired.push(output); }
                }
                Fate::Rebuild => {
                    let reason = if !connectors.contains(&output.connector) {
                        match connectors.iter().find(|c| c.name == output.connector.name) {
                            Some(c) => format!("connector changed ({:?} -> {:?})",(output.connector.id,output.connector.fingerprint),(c.id,c.fingerprint)),
                            None => "unplugged".to_string(),
                        }
                    } else if let Some(failure) = &output.failure {
                        let label = mode_label(&output.mode);
                        if self.demotions.failed(&output.connector,mode_key(&output.mode),output.last_present != 0,failure) {
                            format!("{label} failed to present ({failure}); stepping down to the next mode")
                        } else { format!("{label} failed to present after showing frames ({failure}); once more at {label}") }
                    } else { "its swapchain is suboptimal".to_string() };
                    eprintln!("[hal-direct] {}: rebuilding: {reason}",output.connector.name);
                    retired.push(output)
                }
            }
        }
        let failed_before = self.failed.len();
        // A monitor unplugged or turned off may free a head for a connector that found none.
        let heads_freed = self.connectors.iter().any(|c| !connectors.contains(c)) || retired.iter().any(|o:&Output| self.arrangement.is_off(&o.connector.name));
        self.failed.retain(|f| connectors.contains(&f.connector) && f.requested == requested(&f.connector.name) && !self.arrangement.is_off(&f.connector.name)
            && !(f.wants_head && heads_freed));
        let pending:Vec<_> = connectors.iter().filter(|c| !kept.iter().any(|o:&Output| o.connector == **c)
            && !self.failed.iter().any(|f| f.connector == **c) && !self.arrangement.is_off(&c.name)).cloned().collect();
        self.outputs = kept;
        let topology_changed = !retired.is_empty() || !pending.is_empty() || self.failed.len() != failed_before || connectors != self.connectors;
        self.connectors = connectors;
        if !topology_changed {
            // A rescan with nothing new (or an arrangement change) only moves screens.
            let before = self.composition();
            self.layout();
            if before == self.composition() { return; }
        }
        // A topology generation invalidates an outstanding credit: the client may have
        // discarded it while the desktop had zero size. Reflow needs a fresh full frame.
        self.awaiting = false; self.demand |= self.window.is_some();
        state.ready.store(0,Ordering::Release);
        match unsafe { self.compositor.instance.enumerate_physical_devices() } {
            Ok(physical) => self.physical = physical,
            Err(e) => eprintln!("[hal-direct] refresh Vulkan devices: {e:?}"),
        }
        for output in retired {
            // A connector that stays (new mode, failed presentation, turned off) keeps its
            // display held and its plane in the book; it only gets a new surface and swapchain:
            // its head is not handed to the console in between.
            if self.connectors.contains(&output.connector) {
                let (card,display,off) = (output.connector.card.clone(),output.display,self.arrangement.is_off(&output.connector.name));
                drop(output);
                if off { if let Some(card) = self.cards.get_mut(&card) { card.holds.park(display); } }
            } else { self.retire(output); }
        }
        self.cards.retain(|path,_| self.connectors.iter().any(|c| c.card == *path));
        // The connectors the WM drives, per card: a console on one of them is never switched off.
        let mut driven:HashMap<PathBuf,HashSet<u32>> = HashMap::new();
        for c in self.outputs.iter().map(|o| &o.connector).chain(&pending) { driven.entry(c.card.clone()).or_default().insert(c.id); }
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
                // Close the probe before the master open: a duplicate of a card's file would
                // keep that file (and its master) alive after the card is dropped.
                drop(probe);
                let device = if let Some(dev) = self.devices.get(&physical) { dev.clone() } else {
                    let dev = Device::display_peer(self.compositor.clone(),physical)?;
                    self.devices.insert(physical,dev.clone()); dev
                };
                if !self.cards.contains_key(&connector.card) { self.cards.insert(connector.card.clone(),Card::open(&connector.card,&device)?); }
                let card = self.cards.get_mut(&connector.card).unwrap();
                if unsafe { drm::drmSetMaster(card.file.as_raw_fd()) } < 0 { return Err(std::io::Error::last_os_error().to_string()); }
                card.acquire(display,connector.id,&driven[&connector.card]).map_err(|e| format!("acquire: {e}"))?;
                let card = &self.cards[&connector.card];
                let native = drm_native_resolution(card.file.as_raw_fd(),connector.id);
                // The modes to try, in order: the requested (or fastest native) mode, each lower
                // refresh at its resolution, each smaller resolution, then the last mode that
                // worked here and the native one; below a mode that failed to present. A mode
                // that is unavailable or fails never leaves the screen dark.
                let (modes,error) = display_mode_ladder(&device.instance,physical,display,request.as_deref(),native,
                    self.working.get(&connector),self.demotions.below(&connector))?;
                let first = &modes[0];
                // Occupancy from the session's book of the displays it holds (a display given
                // back above leaves it), then the driver's own report.
                self.planes.keep(|d| self.cards.values().any(|c| c.holds.holds(d)));
                let held:HashSet<_> = self.cards.values().flat_map(|c| c.holds.displays.iter().map(|h| h.display)).collect();
                let rows = plane_rows(&device.instance,physical,display,first)?;
                let candidates = plane_order(&rows,display,&self.planes.of(physical),&held,&connected_displays(&device.instance,physical)?);
                Ok::<_,String>((physical,display,modes,error,candidates))
            })();
            match result {
                Ok((physical,display,modes,error,candidates)) => plans.entry(physical).or_default().push((connector,display,request,modes,error,candidates)),
                Err(e) => { let wants_head = e.contains(NO_FREE_HEAD); self.fail_row(&connector,request,format!("failed: {e}"),wants_head) }
            }
        }
        for (physical,plans) in plans {
            let candidate_ids:Vec<_> = plans.iter().map(|(_,_,_,_,_,p)| p.iter().map(|c| c.plane_index).collect()).collect();
            let assignment = assign_planes(&candidate_ids);
            for ((connector,display,request,modes,mut error,candidates),plane) in plans.into_iter().zip(assignment) {
                let Some(candidate) = plane.and_then(|plane| candidates.iter().find(|c| c.plane_index == plane)).copied() else {
                    if let Some(card) = self.cards.get_mut(&connector.card) { card.release_display(display); }
                    self.fail_row(&connector,request,format!("failed: {NO_FREE_HEAD}: every display plane it can use shows another monitor"),true); continue;
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
                            eprintln!("[hal-direct] {}: showing {}: {e}",connector.name,mode_label(&output.mode));
                        }
                        output.error = error;
                        if let Some(card) = self.cards.get(&connector.card) {
                            eprintln!("[hal-direct] {} (connector {}): display held on {}, plane {}",connector.name,connector.id,card.holding(display),output.plane);
                        }
                        self.planes.show(physical,output.plane,display);
                        self.outputs.push(output);
                    }
                    None => {
                        if let Some(card) = self.cards.get_mut(&connector.card) { card.release_display(display); }
                        self.fail_row(&connector,request,format!("failed: {}",error.unwrap_or_default()),false);
                    }
                }
            }
        }
        self.outputs.sort_by_key(|o| connector_rank(&o.connector.name));
        self.layout(); self.publish(state);
    }
    /// A new arrangement (screens regrouped, reordered, another optimise-for output): only what
    /// each output presents changes (its screen's rectangle of the wide desktop, scaled to fit)
    /// and the screen list. Every output keeps its display, mode, plane, surface and swapchain:
    /// no blank, no modeset. When the desktop changes, the client gets a fresh credit and draws
    /// a whole frame at the new size; until then the retained frame is letterboxed. Turning an
    /// output on or off is the reconcile's (`run`).
    fn recompose(&mut self,state:&State) {
        let before = self.composition();
        self.layout();
        if before == self.composition() { self.publish(state); return; }
        self.awaiting = false; self.demand |= self.window.is_some();
        state.ready.store(0,Ordering::Release);
        eprintln!("[hal-direct] arrangement {}: desktop {}x{}, no output rebuilt",self.arrangement.text(),self.size[0],self.size[1]);
        self.publish(state);
    }
    /// What the outputs show: the desktop size, each output's rectangle and the screens.
    fn composition(&self) -> ([u32;2],Vec<Option<wide_desktop::SliceRect>>,Vec<(String,Vec<usize>,usize)>) {
        (self.size,self.outputs.iter().map(|o| o.placement).collect(),self.screens.iter().map(|s| (s.name.clone(),s.outputs.clone(),s.optimise_for)).collect())
    }
    /// The saved profile for the monitors connected now, once per new set of monitors (start-up,
    /// a monitor plugged in or out): its arrangement, modes, main screen and pacing replace the
    /// running ones. No saved profile for the set: the running arrangement stays (an output it
    /// does not name extends the desktop). Pinned by `MAKEPAD_DISPLAY_ARRANGEMENT`: never.
    fn restore_profile(&mut self,connectors:&[DrmConnector]) {
        use crate::display_profiles::{monitor_set, DisplayProfiles, OutputKey};
        let monitors = monitor_set(connectors.iter().map(|c| c.fingerprint));
        if connectors.is_empty() || self.monitors.as_ref() == Some(&monitors) { return; }
        self.monitors = Some(monitors.clone());
        let Some(path) = self.profiles.as_ref().filter(|_| !self.arrangement_pinned) else { return };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => { eprintln!("[hal-direct] saved display profiles {}: {e}",path.display()); return; }
        };
        let Some(profile) = DisplayProfiles::parse(&text).find(&monitors).cloned() else {
            eprintln!("[hal-direct] no saved display profile for these {} monitors; keeping {}",monitors.len(),self.arrangement.text());
            return;
        };
        let connected:Vec<(String,OutputKey)> = connectors.iter()
            .filter_map(|c| Some((c.name.clone(),OutputKey::new(&pci(&c.card)?,&c.name,c.fingerprint)))).collect();
        let settings = profile.resolve(&connected);
        for (name,mode) in settings.modes {
            if self.modes_pinned.contains(&name) { continue; }
            match mode { Some(mode) => { self.modes.insert(name,mode); } None => { self.modes.remove(&name); } }
        }
        self.arrangement = settings.arrangement;
        if settings.main.is_some() { self.primary = settings.main; }
        self.pacing = settings.pacing;
        eprintln!("[hal-direct] saved display profile for these {} monitors: {}",monitors.len(),self.arrangement.text());
    }
    fn layout(&mut self) {
        if let Some(head) = &self.virtual_head {
            // The virtual desktop at its own size; every panel shows it scaled to fit (a
            // panel whose mode is the same size copies it 1:1).
            self.size = [head.display.width,head.display.height];
            self.screens.clear();
            for output in &mut self.outputs { output.placement = None; }
            return;
        }
        let names:Vec<&str> = self.outputs.iter().map(|o| o.connector.name.as_str()).collect();
        let sizes:Vec<[u32;2]> = self.outputs.iter().map(|o| o.size()).collect();
        let limits = &self.compositor.properties.limits;
        let max = [limits.max_framebuffer_width.min(limits.max_image_dimension2_d),limits.max_framebuffer_height.min(limits.max_image_dimension2_d)];
        let (size,screens,placements) = arrange_screens(&self.arrangement,&names,&sizes,max);
        self.size = size; self.screens = screens;
        for (output,placement) in self.outputs.iter_mut().zip(placements) { output.placement = placement; }
    }
    /// The main screen (index into `screens`): the one showing `primary`, else the one the
    /// arrangement puts it on (its output unplugged, the rest still showing it), else the first
    /// placed one.
    fn primary_screen(&self) -> Option<usize> {
        let name = self.primary.as_deref();
        name.and_then(|n| self.screens.iter().position(|s| s.outputs.iter().any(|&i| self.outputs[i].connector.name == n)))
            .or_else(|| name.and_then(|n| self.arrangement.screens.iter().find(|a| a.outputs.iter().any(|o| o == n)))
                .and_then(|a| self.screens.iter().position(|s| s.name == a.outputs[0])))
            .filter(|&i| self.screens[i].rect.is_some())
            .or_else(|| self.screens.iter().position(|s| s.rect.is_some()))
    }
    fn pacing_candidates(&self) -> Vec<PacingCandidate<'_>> {
        let main = self.primary_screen();
        self.screens.iter().enumerate().filter(|(_,s)| s.rect.is_some()).flat_map(|(si,s)| s.outputs.iter().map(move |&i| (si,s,i)))
            .map(|(si,s,i)| { let o = &self.outputs[i]; PacingCandidate {
                index:i, name:&o.connector.name, refresh_mhz:o.mode.refresh_mhz, screen:si, optimise:s.optimise_for == i,
                main:Some(si) == main, rendering_gpu:o.dev.physical == self.compositor.physical,
            } }).collect()
    }
    /// The output whose presents pace the desktop (gives the client its frame credit), from
    /// the pacing setting, resolved again on every call so hotplug, mode and arrangement
    /// changes apply at once (see `pacing_choice`).
    fn pacing_index(&self) -> Option<usize> {
        pacing_choice(&self.pacing,&self.pacing_candidates()).map(|(i,_)| i)
            .or_else(|| self.primary_screen().map(|s| self.screens[s].optimise_for))
    }
    /// `paced_by` for the inventory: the pacing output, or `None` while every screen runs at
    /// its own refresh (Auto, several screens).
    fn paced_by(&self) -> Option<String> {
        if self.virtual_head.is_some() { return Some(VIRTUAL_OUTPUT.into()); }
        pacing_choice(&self.pacing,&self.pacing_candidates()).filter(|(_,pick)| *pick).map(|(i,_)| self.outputs[i].connector.name.clone())
    }
    fn publish(&self,state:&State) {
        let mut outputs:Vec<_> = self.outputs.iter().map(|o| LinuxDisplayOutput {
            name:o.connector.name.clone(),monitor:o.connector.monitor.clone(),fingerprint:o.connector.fingerprint,
            width:o.mode.extent.width,height:o.mode.extent.height,refresh_hz:o.mode.refresh_mhz as f64 / 1000.0,
            card:card_name(&o.connector.card),active:o.last_present != 0,
            status:match (&o.error,o.last_present != 0) {
                (Some(e),_) => format!("showing {}: {e}",mode_label(&o.mode)),
                (None,true) => "active".into(),
                (None,false) => "ready; waiting for first frame".into(),
            },modes:o.mode.modes.clone(),
            mode_override:self.modes.get(&o.connector.name).cloned(),pci:pci(&o.connector.card),
            error:o.error.clone(),requested:o.requested.clone(),
            native:self.cards.get(&o.connector.card).and_then(|c| drm_connector_modes(c.file.as_raw_fd(),o.connector.id).1),
        }).collect();
        outputs.extend(self.failed.iter().map(|f| f.row.clone()));
        // The connected outputs that are not driven, listed with their modes: every panel while
        // the virtual display is the desktop with its panels off, else the ones turned off.
        let panels_off = self.panels_off();
        outputs.extend(self.connectors.iter().filter(|c| panels_off || self.arrangement.is_off(&c.name)).map(|c| {
            let (modes,native) = match self.cards.get(&c.card) {
                Some(card) => drm_connector_modes(card.file.as_raw_fd(),c.id),
                None => OpenOptions::new().read(true).write(true).open(&c.card).map(|f| drm_connector_modes(f.as_raw_fd(),c.id)).unwrap_or_default(),
            };
            let status = if panels_off { "not driven: the virtual display is the desktop" } else { "off" };
            LinuxDisplayOutput { name:c.name.clone(),monitor:c.monitor.clone(),fingerprint:c.fingerprint,card:card_name(&c.card),status:status.into(),
                modes,native,pci:pci(&c.card),mode_override:self.modes.get(&c.name).cloned(),..Default::default() }
        }));
        outputs.sort_by_key(|o| connector_rank(&o.name));
        let main = self.primary_screen();
        let mut screens:Vec<_> = self.screens.iter().enumerate().filter_map(|(i,s)| { let r = s.rect?; Some(LinuxDisplayScreen {
            name:s.name.clone(),outputs:s.outputs.iter().map(|&o| self.outputs[o].connector.name.clone()).collect(),
            optimise_for:self.outputs[s.optimise_for].connector.name.clone(),x:r.x,y:r.y,width:r.width,height:r.height,primary:Some(i) == main,
        }) }).collect();
        if let Some(head) = &self.virtual_head {
            let d = head.display;
            outputs.insert(0,LinuxDisplayOutput { name:VIRTUAL_OUTPUT.into(),width:d.width,height:d.height,refresh_hz:d.refresh_hz(),active:head.last_present != 0,
                status:if head.last_present != 0 { "active (virtual)".into() } else { "virtual; waiting for first frame".into() },
                modes:vec![(d.width,d.height,d.refresh_hz())],..Default::default() });
            screens = vec![LinuxDisplayScreen { name:VIRTUAL_OUTPUT.into(),outputs:vec![VIRTUAL_OUTPUT.into()],optimise_for:VIRTUAL_OUTPUT.into(),
                x:0,y:0,width:d.width,height:d.height,primary:true }];
        }
        {
            let mut snapshot = lock(&state.snapshot);
            let paced_by = self.paced_by();
            let virtual_display = self.virtual_head.as_ref().map(|h| h.display);
            snapshot.displays = LinuxDisplaySnapshot { direct:true,outputs,screens,arrangement:self.arrangement.clone(),pacing:self.pacing.clone(),paced_by,virtual_display };
            snapshot.size = self.size;
            if !self.outputs.is_empty() || self.virtual_head.is_some() { state.acquired.store(true,Ordering::Release); }
            // A reacquired output has no phase until its new swapchain presents.
            snapshot.timings = self.outputs.iter().filter(|o| o.last_present != 0 && o.mode.refresh_mhz != 0)
                .map(|o| (o.connector.name.clone(), (o.last_present, 1000.0 / o.mode.refresh_mhz as f64))).collect();
            if let Some(h) = self.virtual_head.as_ref().filter(|h| h.last_present != 0) {
                snapshot.timings.insert(VIRTUAL_OUTPUT.into(),(h.last_present,1000.0 / h.display.refresh_mhz.max(1) as f64));
            }
            state.changed(&mut snapshot);
        }
    }
    /// The current frame is queued for presentation on the pacing output: the next frame may
    /// be taken and the client may render another. Credit follows the queued present, not the
    /// completed flip, so rendering and the (cross-GPU) transfer of frame N+1 overlap the flip
    /// of frame N instead of adding to it. Each output still holds one flip in flight (FIFO):
    /// a present waits for the previous one, so credit comes at most once per refresh.
    fn queued(&self) -> bool {
        self.current.as_ref().is_none_or(|f| self.pace().is_some_and(|p| p.generation >= f.generation))
    }
    /// The panels are not driven (the virtual display is the desktop, its panels `Off`).
    fn panels_off(&self) -> bool { self.virtual_head.as_ref().is_some_and(|h| h.display.panels == VirtualPanels::Off) }
    /// The present state that paces the desktop: the virtual head, else the pacing output.
    fn pace(&self) -> Option<Pace> {
        if let Some(h) = &self.virtual_head {
            return Some(Pace { generation:h.generation,complete:h.complete,last_present:h.last_present,refresh_mhz:h.display.refresh_mhz });
        }
        self.pacing_index().map(|i| { let o = &self.outputs[i]; Pace { generation:o.generation,complete:o.complete,last_present:o.last_present,refresh_mhz:o.mode.refresh_mhz } })
    }
    /// The virtual display becomes the desktop, changes, or gives way to the panels. Only
    /// a change of whether the panels are driven touches an output; a size or refresh change
    /// lays the desktop out again (a fresh credit, a whole frame at the new size).
    fn set_virtual(&mut self,display:Option<VirtualDisplay>,state:&State) -> bool {
        let was_off = self.panels_off();
        let was_virtual = self.virtual_head.is_some();
        match (display,&mut self.virtual_head) {
            (None,head) => { if head.take().is_some() { eprintln!("[hal-direct] virtual display off: the panels' own modes"); } }
            (Some(display),Some(head)) => head.change(display),
            (Some(display),head) => match VirtualHead::new(self.compositor.clone(),display) {
                Ok(new) => *head = Some(new),
                Err(e) => { eprintln!("[hal-direct] virtual display: {e}"); }
            },
        }
        self.awaiting = false; self.demand |= self.window.is_some();
        state.ready.store(0,Ordering::Release);
        self.layout(); self.publish(state);
        // Driving the panels again (or no longer) is a reconcile; leaving the virtual display
        // gives each panel its own rectangle again.
        was_off != self.panels_off() || was_virtual != self.virtual_head.is_some()
    }
    /// Report every frame whose present completed on the pacing output.
    fn notify_presented(&mut self) {
        let Some(pace) = self.pace() else { return };
        let (complete,presented_at) = (pace.complete,pace.last_present);
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
                if e.contains("ERROR_DEVICE_LOST") && !output.dev.lost.swap(true,Ordering::AcqRel) { state.shared.signal(); }
                output.rebuild = true; output.failure = Some(e); continue;
            }
            let progress = (output.generation,output.complete,output.acquired,output.acquire_ready);
            if progress != output.progress || !output.busy(self.current.as_ref()) {
                output.progress = progress; output.progress_at = Instant::now();
            } else if output.progress_at.elapsed() >= FLIP_STALL && !output.rebuild {
                // Flip watchdog: a flip (or the WSI credit after it) outstanding this long is a
                // stalled head; the output is rebuilt (modeset again) and retries the retained
                // complete frame, with no retry limit and no discarded input/image.
                let stall = format!("flip {} made no progress for {} ms",output.serial,output.progress_at.elapsed().as_millis());
                eprintln!("[hal-direct] {}: {stall}; rebuilding this output",output.connector.name);
                output.rebuild = true; output.failure = Some(stall);
            }
            if output.last_present != old {
                // The first frame shown: this mode works here.
                if old == 0 { self.working.showed(&output.connector,mode_key(&output.mode)); }
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
        if let Some(head) = &mut self.virtual_head {
            let old = head.last_present;
            head.service(self.current.as_ref())?;
            if head.last_present != old {
                first |= old == 0;
                timing_changed = true;
                let value = (head.last_present,1000.0 / head.display.refresh_mhz.max(1) as f64);
                lock(&state.snapshot).timings.insert(VIRTUAL_OUTPUT.into(),value);
            }
        }
        if first { self.publish(state); }
        if timing_changed { state.timings_changed.store(true, Ordering::Release); state.shared.signal(); }
        self.notify_presented();
        if self.queued() {
            if self.demand && !self.awaiting && self.frames.is_empty() {
                if let Some(pace) = self.pace() {
                    let period = 1_000_000_000_000u64 / u64::from(pace.refresh_mhz.max(1));
                    let now = crate::clock_ns(); let phase = pace.last_present;
                    let next = if phase == 0 { now + period } else { phase + ((now.saturating_sub(phase) / period) + 1) * period };
                    // Empty animation beats and pointer reports can request again before
                    // a rendered frame reaches this worker. Keep those requests on the
                    // display clock instead of granting credit at the input report rate.
                    if now >= self.credit_due {
                        self.credit_due = if self.credit_due == 0 { next } else {
                            self.credit_due + ((now - self.credit_due) / period + 1) * period
                        };
                        state.ready.store(next,Ordering::Release); state.shared.signal();
                        self.demand = false; self.awaiting = true;
                    }
                }
            }
        }
        Ok(self.outputs.iter().any(|o| o.busy(self.current.as_ref()))
            || (!self.frames.is_empty() && self.pace().is_some()))
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
    let mut probe = true;
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
                Command::Resume => { context.active = true; state.released.store(false,Ordering::Release); rescan = true; probe = true; context.demand = true; }
                Command::Mode(name,mode) => {
                    // A mode pick on a failed screen retries it, even when it names the same mode.
                    context.failed.retain(|f| f.connector.name != name);
                    // ...and every mode may be tried again.
                    context.demotions.forget(&name);
                    if let Some(mode) = mode { context.modes.insert(name,mode); } else { context.modes.remove(&name); }
                    rescan = true;
                }
                // A composition change, never a mode change: no output is touched, except one
                // turned on or off (the reconcile below).
                Command::Arrangement(arrangement) => {
                    rescan |= arrangement.off != context.arrangement.off;
                    context.arrangement = arrangement;
                    context.recompose(&state);
                }
                Command::Primary(name) => { context.primary = Some(name); context.publish(&state); }
                // No modeset: the next credit follows the newly resolved output.
                Command::Pacing(pacing) => { context.pacing = pacing; context.publish(&state); }
                Command::Device(device) => { context.replace_device(device,&state); rescan = true; }
                Command::Virtual(display) => { rescan |= context.set_virtual(display,&state); }
            }
        }
        if stop { break; }
        let hotplug = watch.as_ref().is_some_and(|w| w.changed());
        rescan |= hotplug;
        probe |= hotplug;
        let now = Instant::now();
        if now >= next_scan {
            next_scan = now + Duration::from_secs(1);
            match context.scan_connectors(false) {
                Ok(connectors) => { rescan |= connectors != context.connectors; }
                Err(e) => eprintln!("[hal-direct] rescan: {e}"),
            }
        }
        rescan |= context.outputs.iter().any(|o| o.rebuild);
        if rescan && context.active {
            if let Ok(connectors) = context.scan_connectors(probe) { context.reconcile(connectors,&state); probe = false; }
            rescan = false;
        }
        let busy = match context.step(&state) {
            Ok(busy) => busy,
            Err(e) => {
                eprintln!("[hal-direct] presentation: {e}; retaining the whole frame for reacquisition");
                // A lost device seen here first (no render submit to see it: the client waits
                // for this worker's credit) is the HAL's to replace: say so and wake it.
                if e.contains("ERROR_DEVICE_LOST") && !context.compositor.lost.swap(true,Ordering::AcqRel) {
                    eprintln!("[hal-direct] the render GPU device is lost; asking the HAL to replace it");
                    state.shared.signal();
                }
                // Recreate display resources on the next turn; retain the source image and
                // outstanding demand. A cable/VT failure is not a lost render device.
                rescan = true; false
            }
        };
        let mut deadline = if busy { now + Duration::from_micros(500) } else if rescan && context.active { now + Duration::from_millis(1) } else { next_scan };
        // the virtual head's pending tick (it is not `busy`: no polling between ticks)
        if let Some(due) = context.virtual_head.as_ref().filter(|_| context.active).and_then(|h| h.pending.map(|_| h.due)) {
            deadline = deadline.min(Instant::now() + Duration::from_nanos(due.saturating_sub(crate::clock_ns())));
        }
        if context.active && context.demand && !context.awaiting && context.frames.is_empty() && context.queued() && context.pace().is_some() {
            deadline = deadline.min(Instant::now() + Duration::from_nanos(context.credit_due.saturating_sub(crate::clock_ns())));
        }
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
    fn c(index:usize,name:&'static str,hz:u32,screen:usize,optimise:bool,main:bool,rendering_gpu:bool) -> PacingCandidate<'static> {
        PacingCandidate { index,name,refresh_mhz:hz*1000,screen,optimise,main,rendering_gpu }
    }
    #[test]
    fn pacing_follows_the_setting_and_each_screens_optimise_for_output() {
        // a 240 Hz panel on the Intel GPU (main), a 30 Hz HDMI monitor on the rendering GPU
        let extended = [c(0,"card0-eDP-1",240,0,true,true,false),c(1,"card1-HDMI-A-2",30,1,true,false,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,&extended),Some((0,false)),"two screens: each at its own refresh");
        let mirror = [c(0,"card0-eDP-1",240,0,false,true,false),c(1,"card1-HDMI-A-2",30,0,true,true,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,&mirror),Some((1,true)),"one mirrored screen: its optimise-for output paces");
        let mirror = [c(0,"card0-eDP-1",240,0,true,true,false),c(1,"card1-HDMI-A-2",30,0,false,true,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,&mirror),Some((0,true)),"optimised for the panel: the panel paces");
        assert_eq!(pacing_choice(&DisplayPacing::Slowest,&extended),Some((1,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Fastest,&mirror),Some((0,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Screen("card1-HDMI-A-2".into()),&extended),Some((1,true)));
        assert_eq!(pacing_choice(&DisplayPacing::Screen("card9-DP-1".into()),&mirror),Some((0,true)),"a gone output falls back to Auto");
        // [B 60] [A 144 main] [C 240 + D 60, optimised for D]: the mirror is paced by D, so A is the fastest pacer
        let users = [c(0,"B",60,0,true,false,true),c(1,"A",144,1,true,true,true),c(2,"C",240,2,false,false,true),c(3,"D",60,2,true,false,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Auto,&users),Some((1,false)),"several screens: the fastest screen's own pacer");
        assert_eq!(pacing_choice(&DisplayPacing::Fastest,&users),Some((2,true)),"Fastest: any output");
        let tie = [c(0,"card0-eDP-1",60,0,true,false,false),c(1,"card1-DP-3",60,1,true,true,true)];
        assert_eq!(pacing_choice(&DisplayPacing::Slowest,&tie),Some((1,true)),"ties prefer the main screen");
        assert_eq!(pacing_choice(&DisplayPacing::Auto,&[]),None);
    }
    #[test]
    fn the_users_screens_are_laid_out_once_each_with_the_mirror_sized_by_its_choice() {
        use crate::direct::DisplayArrangement;
        let r = |x,y,width,height| Some(wide_desktop::SliceRect { x,y,width,height });
        let names = ["A","B","C","D"];
        // A 2560x1440, B 1920x1080, C 3840x2160, D 2560x1440 (C and D both 16:9)
        let sizes = [[2560,1440],[1920,1080],[3840,2160],[2560,1440]];
        let max = [32768,32768];
        let users = DisplayArrangement::parse("B,A,C+D").unwrap();
        let (size,screens,placements) = arrange_screens(&users,&names,&sizes,max);
        assert_eq!(screens.iter().map(|s| (s.name.as_str(),s.outputs.clone(),s.optimise_for)).collect::<Vec<_>>(),
            vec![("B",vec![1],1),("A",vec![0],0),("C",vec![2,3],2)],"three screens; the mirror defaults to its largest monitor");
        assert_eq!(size,[1920+2560+3840,2160]);
        assert_eq!(placements,vec![r(1920,0,2560,1440),r(0,0,1920,1080),r(4480,0,3840,2160),r(4480,0,3840,2160)],
            "both mirror outputs show the one screen rectangle: rendered once");
        assert_eq!(wide_desktop::fit((3840,2160),(2560,1440)),wide_desktop::SliceRect { x:0,y:0,width:2560,height:1440 },"D shows it scaled, whole");
        // optimised for D: the screen takes D's size, nothing else moves
        let (size,screens,placements) = arrange_screens(&DisplayArrangement::parse("B,A,C+*D").unwrap(),&names,&sizes,max);
        assert_eq!((screens[2].name.as_str(),screens[2].optimise_for),("C",3),"the name stays: its windows stay");
        assert_eq!((size,placements[2],placements[3]),([1920+2560+2560,1440],r(4480,0,2560,1440),r(4480,0,2560,1440)));
        // a 16:10 panel mirrored with a 16:9 monitor: letterboxed both ways
        let (_,screens,placements) = arrange_screens(&DisplayArrangement::parse("P+M").unwrap(),&["P","M"],&[[2560,1600],[1920,1080]],max);
        assert_eq!((screens.len(),placements[1]),(1,r(0,0,2560,1600)));
        assert_eq!(wide_desktop::fit((2560,1600),(1920,1080)),wide_desktop::SliceRect { x:96,y:0,width:1728,height:1080 });
        let (_,_,placements) = arrange_screens(&DisplayArrangement::parse("P+*M").unwrap(),&["P","M"],&[[2560,1600],[1920,1080]],max);
        assert_eq!(placements[0],r(0,0,1920,1080));
        assert_eq!(wide_desktop::fit((1920,1080),(2560,1600)),wide_desktop::SliceRect { x:0,y:80,width:2560,height:1440 });
        // C unplugged: the mirror goes on with D, at D's size, under the same name
        let (_,screens,placements) = arrange_screens(&users,&["A","B","D"],&[[2560,1440],[1920,1080],[2560,1440]],max);
        assert_eq!((screens.len(),screens[2].name.as_str(),placements[2]),(3,"C",r(4480,0,2560,1440)));
        // mirror everything / extend everything are the two ends of the same model
        let (_,screens,_) = arrange_screens(&DisplayArrangement::parse("A+B+C+D").unwrap(),&names,&sizes,max);
        assert_eq!(screens.len(),1);
        let (_,screens,_) = arrange_screens(&DisplayArrangement::default(),&names,&sizes,max);
        assert_eq!(screens.len(),4);
        let (_,screens,placements) = arrange_screens(&DisplayArrangement::parse("off:B").unwrap(),&["A","C"],&[[2560,1440],[3840,2160]],max);
        assert_eq!((screens.len(),placements[1]),(2,r(2560,0,3840,2160)));
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
        DrmConnector { card:PathBuf::from(format!("/dev/dri/{card}")),name:format!("{card}-{name}"),id,fingerprint,monitor:String::new() }
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
