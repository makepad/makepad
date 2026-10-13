use super::{shell::{lock, Shared}, state};
use crate::Id;
use crate::backend::vulkan::surface::{NativeSurface, SurfaceState, VkSurfaceSource};
use makepad_vulkan::{vk, Instance};
use std::{ffi::{c_void, CStr}, sync::Arc};
use makepad_jni_sys::{JNIEnv, jobject};

#[link(name = "android")]
extern "C" {
    fn ANativeWindow_acquire(window: *mut c_void);
    fn ANativeWindow_release(window: *mut c_void);
    fn ANativeWindow_fromSurface(env: *mut JNIEnv, surface: jobject) -> *mut c_void;
    fn ANativeWindow_getWidth(window: *mut c_void) -> i32;
    fn ANativeWindow_getHeight(window: *mut c_void) -> i32;
}

/// Each lease owns an NDK reference, including the registry's current window. Replacing the
/// registry never releases the reference held by an in-flight Vulkan surface.
pub(super) struct Window(pub usize);
impl Window {
    pub unsafe fn from_java(env: *mut JNIEnv, surface: jobject) -> Option<Self> {
        let p = ANativeWindow_fromSurface(env, surface);
        if p.is_null() { None } else { Some(Self(p as usize)) }
    }
    pub fn size(&self) -> [u32; 2] {
        unsafe { [ANativeWindow_getWidth(self.0 as _) .max(0) as u32, ANativeWindow_getHeight(self.0 as _).max(0) as u32] }
    }
}
impl Clone for Window {
    fn clone(&self) -> Self { unsafe { ANativeWindow_acquire(self.0 as _); } Self(self.0) }
}
impl Drop for Window {
    fn drop(&mut self) { unsafe { ANativeWindow_release(self.0 as _); } }
}

#[derive(Default)]
pub(super) struct Registry {
    pub host: Option<Id>,
    pub state: state::SurfaceState,
    pub window: Option<Window>,
}

pub struct AndroidSurfaces;
impl VkSurfaceSource for AndroidSurfaces {
    fn instance_extensions(&self) -> Vec<&'static CStr> { vec![vk::KHR_ANDROID_SURFACE_NAME] }
    fn state(&mut self, sid: u32, id: Id, _: [u32; 2]) -> SurfaceState {
        let r = lock(&Shared::global().registry);
        if sid != crate::session::HOST || r.host != Some(id) { return SurfaceState::Lost; }
        match r.state.status() {
            state::SurfaceStatus::Lost => SurfaceState::Lost,
            state::SurfaceStatus::Native { generation, size } => SurfaceState::Native { generation, size },
        }
    }
    unsafe fn create(&mut self, instance: &Instance, sid: u32, id: Id) -> Result<NativeSurface, String> {
        let lease = {
            let r = lock(&Shared::global().registry);
            if sid != crate::session::HOST || r.host != Some(id) { return Err("unknown Android host window".into()); }
            r.window.clone().ok_or("Android surface is absent")?
        };
        let handle = instance.create_android_surface(&vk::AndroidSurfaceCreateInfoKHR::default().window(lease.0 as _), None)
            .map_err(|e| format!("vkCreateAndroidSurfaceKHR: {e:?}"))?;
        Ok(NativeSurface { handle, keep: Arc::new(lease) })
    }
}
