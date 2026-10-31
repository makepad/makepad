//! Vulkan binding negotiation. The runtime borrows the application's Vulkan
//! instance/device; it never destroys objects owned by the game.
use super::*;
use makepad_vulkan::{vk, commands, Entry, Instance as VkInstance};
use std::ffi::{c_void, CStr};

const DEVICE_EXTENSIONS: &[&CStr] = &[
    vk::KHR_EXTERNAL_MEMORY_FD_NAME, vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
    vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME, vk::KHR_IMAGE_FORMAT_LIST_NAME,
    vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
];
const INSTANCE_EXTENSIONS: &[&CStr] = &[c"VK_KHR_get_physical_device_properties2"];

pub(super) fn vk_error(e: vk::Result) -> Result {
    eprintln!("[makepad-openxr] Vulkan: {e:?}");
    match e { vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OUT_OF_MEMORY,
        vk::Result::ERROR_DEVICE_LOST => SESSION_LOST, _ => RUNTIME_FAILURE }
}
fn enabled(instance: Handle, second: bool) -> Xr<()> {
    let r = registry(); let i = r.instance(instance)?;
    if i.extensions.contains(if second { "XR_KHR_vulkan_enable2" } else { "XR_KHR_vulkan_enable" }) { Ok(()) } else { Err(FUNCTION_UNSUPPORTED) }
}
unsafe fn requirements(instance: Handle, id: Handle, out: *mut GraphicsRequirementsVulkan, second: bool) -> Xr<()> {
    enabled(instance, second)?; system(instance, id)?;
    let out = output(out, 1000025002)?;
    out.min_version = (1 << 48) | (1 << 32); // Vulkan 1.1, expressed as XrVersion
    out.max_version = (1 << 48) | (4 << 32);
    registry().instance_mut(instance)?.graphics_checked = true; Ok(())
}
api!(graphics_requirements(instance: Handle, id: Handle, out: *mut GraphicsRequirementsVulkan) { requirements(instance, id, out, false) });
api!(graphics_requirements2(instance: Handle, id: Handle, out: *mut GraphicsRequirementsVulkan) { requirements(instance, id, out, true) });
unsafe fn extension_string(instance: Handle, id: Handle, extensions: &[&CStr], capacity: u32, count: *mut u32, out: *mut c_char) -> Xr<()> {
    enabled(instance, false)?; system(instance, id)?;
    let mut bytes: Vec<c_char> = extensions.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ").bytes().map(|c| c as c_char).collect();
    bytes.push(0); enumerate(&bytes, capacity, count, out)
}
api!(vulkan_instance_extensions(instance: Handle, id: Handle, capacity: u32, count: *mut u32, out: *mut c_char) {
    extension_string(instance, id, INSTANCE_EXTENSIONS, capacity, count, out)
});
api!(vulkan_device_extensions(instance: Handle, id: Handle, capacity: u32, count: *mut u32, out: *mut c_char) {
    extension_string(instance, id, DEVICE_EXTENSIONS, capacity, count, out)
});

// The bridge reports a DRM render node's dev_t. Match it through the Vulkan
// device's PCI identity, exactly as the renderer's DMA-BUF support query does.
unsafe fn render_device(instance: &VkInstance, physical: vk::PhysicalDevice) -> Option<u64> {
    let have = instance.enumerate_device_extension_properties(physical).ok()?;
    if !have.iter().any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == vk::EXT_PCI_BUS_INFO_NAME) { return None; }
    let mut pci = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
    let mut props = vk::PhysicalDeviceProperties2::default().next_physical_device_pci_bus_info_properties_ext(&mut pci);
    instance.get_physical_device_properties2(physical, &mut props);
    let dir = format!("/sys/bus/pci/devices/{:04x}:{:02x}:{:02x}.{:x}/drm", pci.pci_domain, pci.pci_bus, pci.pci_device, pci.pci_function);
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        if !entry.file_name().to_string_lossy().starts_with("renderD") { return None; }
        let text = std::fs::read_to_string(entry.path().join("dev")).ok()?;
        let (major, minor) = text.trim().split_once(':')?;
        let (major, minor): (u64, u64) = (major.parse().ok()?, minor.parse().ok()?);
        Some(((major & 0xfff) << 8) | (minor & 0xff) | ((minor & !0xff) << 12) | ((major & !0xfff) << 32))
    })
}
pub(crate) unsafe fn select_device(instance: &VkInstance, render: u64) -> Xr<vk::PhysicalDevice> {
    for physical in instance.enumerate_physical_devices().map_err(vk_error)? {
        if render_device(instance, physical) != Some(render) { continue; }
        let props = instance.get_physical_device_properties(physical);
        if props.api_version < vk::API_VERSION_1_1 { return Err(GRAPHICS_DEVICE_INVALID); }
        let have = instance.enumerate_device_extension_properties(physical).map_err(vk_error)?;
        if DEVICE_EXTENSIONS.iter().all(|name| have.iter().any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == *name)) { return Ok(physical); }
    }
    Err(GRAPHICS_DEVICE_INVALID)
}
unsafe fn graphics_device(instance: Handle, id: Handle, vulkan: *mut c_void, out: *mut *mut c_void, second: bool) -> Xr<()> {
    enabled(instance, second)?;
    if vulkan.is_null() || out.is_null() { return Err(VALIDATION); }
    *out = std::ptr::null_mut(); let headset = system(instance, id)?;
    let entry = Entry::load().map_err(io_error)?;
    let v = VkInstance::from_handle(&entry, vk::Instance(vulkan as usize));
    let p = select_device(&v, headset.render_device)?;
    registry().instance_mut(instance)?.vulkan = Some((vulkan as usize, p.0));
    *out = p.0 as *mut c_void; Ok(())
}
api!(get_vulkan_graphics_device(instance: Handle, id: Handle, vulkan: *mut c_void, out: *mut *mut c_void) {
    graphics_device(instance, id, vulkan, out, false)
});
api!(get_vulkan_graphics_device2(instance: Handle, info: *const VulkanGraphicsDeviceGetInfo, out: *mut *mut c_void) {
    let info = input(info, 1000090003)?; graphics_device(instance, info.system, info.instance, out, true)
});

pub(super) unsafe fn system_limits(id: Handle, headset: &crate::protocol::Headset) -> Xr<[u32; 2]> {
    if let Some(limits) = registry().instance(id)?.graphics_limits { return Ok(limits); }
    let entry = Entry::load().map_err(io_error)?;
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let instance = entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None).map_err(vk_error)?;
    let result = select_device(&instance, headset.render_device).map(|physical| {
        let limit = instance.get_physical_device_properties(physical).limits.max_image_dimension2_d;
        [limit, limit]
    });
    instance.destroy_instance(None);
    let limits = result?;
    registry().instance_mut(id)?.graphics_limits = Some(limits);
    Ok(limits)
}
unsafe fn merged_extensions(original: *const *const c_char, count: u32, needed: &[&CStr]) -> Xr<Vec<*const c_char>> {
    let mut all = array(original, count)?.to_vec();
    for p in &all { if p.is_null() { return Err(VALIDATION); } }
    for name in needed {
        if !all.iter().any(|p| CStr::from_ptr(*p) == *name) { all.push(name.as_ptr()); }
    } Ok(all)
}
api!(create_vulkan_instance(instance: Handle, info: *const VulkanInstanceCreateInfo, out: *mut *mut c_void, result: *mut i32) {
    enabled(instance, true)?;
    let info = input(info, 1000090000)?; system(instance, info.system)?;
    if info.flags != 0 || info.get_instance_proc_addr.is_null() || info.create_info.is_null() || out.is_null() || result.is_null() { return Err(VALIDATION); }
    *out = std::ptr::null_mut();
    let mut create = *info.create_info.cast::<vk::InstanceCreateInfo>();
    // enable2 aggregates the app's and runtime's Vulkan requirements. Keep
    // the caller's metadata/chain, but request the core 1.1 facilities we use.
    let mut app = if create.p_application_info.is_null() { vk::ApplicationInfo::default() } else { *create.p_application_info };
    app.api_version = app.api_version.max(vk::API_VERSION_1_1);
    create.p_application_info = &app;
    let extensions = merged_extensions(create.pp_enabled_extension_names, create.enabled_extension_count, INSTANCE_EXTENSIONS)?;
    create.enabled_extension_count = extensions.len() as u32; create.pp_enabled_extension_names = extensions.as_ptr();
    let get: commands::PfnGetInstanceProcAddr = std::mem::transmute(info.get_instance_proc_addr);
    let make: commands::PfnCreateInstance = std::mem::transmute(get(vk::Instance::null(), c"vkCreateInstance".as_ptr()).ok_or(INITIALIZATION_FAILED)?);
    let mut handle = vk::Instance::null();
    let answer = make(&create, info.allocator.cast(), &mut handle);
    *result = answer.as_raw(); *out = handle.0 as *mut c_void; Ok(())
});
api!(create_vulkan_device(instance: Handle, info: *const VulkanDeviceCreateInfo, out: *mut *mut c_void, result: *mut i32) {
    enabled(instance, true)?;
    let info = input(info, 1000090001)?; system(instance, info.system)?;
    if info.flags != 0 || info.get_instance_proc_addr.is_null() || info.create_info.is_null() || out.is_null() || result.is_null() { return Err(VALIDATION); }
    *out = std::ptr::null_mut();
    let (vulkan, physical) = registry().instance(instance)?.vulkan.ok_or(CALL_ORDER_INVALID)?;
    if physical != info.physical_device as usize { return Err(GRAPHICS_DEVICE_INVALID); }
    let mut create = *info.create_info.cast::<vk::DeviceCreateInfo>();
    let extensions = merged_extensions(create.pp_enabled_extension_names, create.enabled_extension_count, DEVICE_EXTENSIONS)?;
    create.enabled_extension_count = extensions.len() as u32; create.pp_enabled_extension_names = extensions.as_ptr();
    let get: commands::PfnGetInstanceProcAddr = std::mem::transmute(info.get_instance_proc_addr);
    let make: commands::PfnCreateDevice = std::mem::transmute(get(vk::Instance(vulkan), c"vkCreateDevice".as_ptr()).ok_or(INITIALIZATION_FAILED)?);
    let mut handle = vk::Device::null();
    let answer = make(vk::PhysicalDevice(physical), &create, info.allocator.cast(), &mut handle);
    *result = answer.as_raw(); *out = handle.0 as *mut c_void; Ok(())
});
