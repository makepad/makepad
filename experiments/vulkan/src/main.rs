use {
    std::{
        ffi::{c_char, CString},
        mem,
        mem::MaybeUninit,
        ptr,
        sync::Arc,
    },
    vk_sys::{ApplicationInfo, EntryPoints, InstanceCreateInfo, InstancePointers},
    winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        window::{Window, WindowId},
    },
};

const VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR: u32 = 0x00000001;

#[link(name = "vulkan.1.3.290")]
extern "C" {
    fn vkGetInstanceProcAddr(
        instance: vk_sys::Instance,
        pName: *const c_char,
    ) -> vk_sys::PFN_vkVoidFunction;
}

struct App {
    window: Option<Window>,
}

impl App {
    pub fn new() -> Self {
        Self { window: None }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes();
        let window = event_loop.create_window(attributes).unwrap();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
pub struct Instance {
    instance: vk_sys::Instance,
    pointers: InstancePointers,
}

impl Instance {
    unsafe fn create() -> Arc<Self> {
        let entry_points =
            EntryPoints::load(|name| mem::transmute(vkGetInstanceProcAddr(0, name.as_ptr())));

        let application_name = CString::new("Hello, Vulkan!").unwrap();
        let engine_name = CString::new("Makepad").unwrap();

        let mut application_info: ApplicationInfo = MaybeUninit::zeroed().assume_init();
        application_info.sType = vk_sys::STRUCTURE_TYPE_APPLICATION_INFO;
        application_info.pApplicationName = application_name.as_ptr();
        application_info.applicationVersion = 1 << 22 | 0 << 12 | 0;
        application_info.pEngineName = engine_name.as_ptr();
        application_info.engineVersion = 1 << 22 | 0 << 12 | 0;
        application_info.apiVersion = 1 << 22 | 0 << 12 | 0;

        let vk_khr_portability_enumeration_extension_name =
            CString::new("VK_KHR_portability_enumeration").unwrap();
        let enabled_extension_names = [vk_khr_portability_enumeration_extension_name.as_ptr()];

        let mut create_info: InstanceCreateInfo = MaybeUninit::zeroed().assume_init();
        create_info.sType = vk_sys::STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
        create_info.flags = VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR;
        create_info.pApplicationInfo = &application_info;
        create_info.enabledExtensionCount = enabled_extension_names.len() as u32;
        create_info.ppEnabledExtensionNames = enabled_extension_names.as_ptr();

        let mut instance = 0;
        let result = entry_points.CreateInstance(&create_info, ptr::null(), &mut instance);
        if result != vk_sys::SUCCESS {
            panic!();
        }

        let pointers = InstancePointers::load(|name| {
            mem::transmute(vkGetInstanceProcAddr(instance, name.as_ptr()))
        });

        Arc::new(Self { instance, pointers })
    }

    fn pointers(&self) -> &InstancePointers {
        &self.pointers
    }

    unsafe fn enumerate_physical_devices(self: &Arc<Self>) -> Vec<PhysicalDevice> {
        let mut physical_device_count = 0;
        self.pointers.EnumeratePhysicalDevices(
            self.instance,
            &mut physical_device_count,
            ptr::null_mut(),
        );
        let mut physical_devices = vec![0; physical_device_count as usize];
        self.pointers.EnumeratePhysicalDevices(
            self.instance,
            &mut physical_device_count,
            physical_devices.as_mut_ptr(),
        );
        physical_devices
            .into_iter()
            .map(|physical_device| PhysicalDevice {
                instance: self.clone(),
                physical_device,
            })
            .collect()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            self.pointers.DestroyInstance(self.instance, ptr::null());
        }
    }
}

#[derive(Debug)]
struct PhysicalDevice {
    instance: Arc<Instance>,
    physical_device: vk_sys::PhysicalDevice,
}

impl PhysicalDevice {
    unsafe fn get_physical_device_properties(&self) -> vk_sys::PhysicalDeviceProperties {
        let mut properties = MaybeUninit::zeroed().assume_init();
        self.instance.pointers().GetPhysicalDeviceProperties(
            self.physical_device,
            &mut properties,
        );
        properties
    }
}

fn main() {
    unsafe {
        let instance = Instance::create();
        let physical_devices = instance.enumerate_physical_devices();
        for physical_device in &physical_devices {
            let properties = physical_device.get_physical_device_properties();
        }
        println!("{:?}", physical_devices);
    }
    let event_loop = EventLoop::new().unwrap();
    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap()
}
