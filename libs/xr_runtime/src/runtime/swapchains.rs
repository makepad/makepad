use super::*;
use crate::runtime::gpu::{Export, Gpu, Image};
use crate::protocol::{Message, SubmittedEye, Tracking};
use makepad_vulkan::vk;
use std::collections::VecDeque;

pub(crate) struct Swapchain {
    pub session: Handle,
    pub images: Vec<Arc<Image>>,
    pub state: Mutex<SwapchainState>,
    pub static_image: bool,
}
#[derive(Default)]
pub(crate) struct SwapchainState {
    acquired: VecDeque<(usize, bool)>,
    released: Option<usize>,
    used: bool,
    next: usize,
}
fn formats(gpu: &Gpu) -> Vec<i64> {
    [vk::Format::B8G8R8A8_SRGB, vk::Format::R8G8B8A8_SRGB, vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM]
        .into_iter().filter(|f| unsafe { gpu.instance.get_physical_device_format_properties(gpu.physical, *f) }.optimal_tiling_features
            .contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::BLIT_SRC))
        .map(|f| f.as_raw() as i64).collect()
}
fn swapchain(id: Handle) -> Xr<Arc<Swapchain>> { registry().swapchains.get(&id).cloned().ok_or(HANDLE_INVALID) }
api!(swapchain_formats(id: Handle, capacity: u32, count: *mut u32, out: *mut i64) {
    let s = session(id)?; enumerate(&formats(&s.gpu), capacity, count, out)
});
api!(create_swapchain(id: Handle, info: *const SwapchainCreateInfo, out: *mut Handle) {
    let s = session(id)?; let info = input(info, 9)?;
    if out.is_null() { return Err(VALIDATION); } *out = 0;
    if info.flags & !2 != 0 || info.samples != 1 || info.faces != 1 || info.array_size == 0 || info.mip_count == 0 || info.usage & !0xff != 0 || info.usage & 2 != 0 { return Err(FEATURE_UNSUPPORTED); }
    if !formats(&s.gpu).contains(&info.format) { return Err(SWAPCHAIN_FORMAT_UNSUPPORTED); }
    if info.width == 0 || info.height == 0 || info.width > s.gpu.max_size || info.height > s.gpu.max_size { return Err(VALIDATION); }
    let static_image = info.flags & 2 != 0;
    // Three images permit a writer, a released compositor image and a queued
    // acquisition. Static swapchains expose exactly their one immutable image.
    let mut images = Vec::new();
    for _ in 0..if static_image { 1 } else { 3 } {
        images.push(Arc::new(s.gpu.image(vk::Format::from_raw(info.format as i32), info.width, info.height, info.array_size, info.mip_count, info.usage)?));
    }
    let mut r = registry(); let handle = r.id();
    r.swapchains.insert(handle, Arc::new(Swapchain { session: id, images, state: Mutex::new(SwapchainState::default()), static_image }));
    *out = handle; Ok(())
});
api!(destroy_swapchain(id: Handle) {
    let old = registry().swapchains.remove(&id).ok_or(HANDLE_INVALID)?;
    // A graphics translation layer can still have commands referring to these
    // images when the application replaces its swapchain. Removing the OpenXR
    // handle is immediate; retire the allocation after the next submitted
    // frame's transfer fence, rather than invalidating queued rendering work.
    let s = session(old.session)?;
    lock(&s.retired).push(old);
    Ok(())
});
api!(swapchain_images(id: Handle, capacity: u32, count: *mut u32, out: *mut SwapchainImage) {
    let chain = swapchain(id)?;
    if count.is_null() { return Err(VALIDATION); } *count = chain.images.len() as u32;
    if capacity == 0 { return Ok(()); } if capacity < chain.images.len() as u32 { return Err(SIZE_INSUFFICIENT); }
    for (i, image) in chain.images.iter().enumerate() { output(out.add(i), 1000025001)?.image = image.image.0; }
    Ok(())
});
api!(acquire_swapchain(id: Handle, info: *const SwapchainAcquireInfo, out: *mut u32) {
    if !info.is_null() { input(info, 55)?; } if out.is_null() { return Err(VALIDATION); }
    let chain = swapchain(id)?; let mut st = lock(&chain.state);
    if chain.static_image && st.used { return Err(CALL_ORDER_INVALID); }
    let index = (0..chain.images.len()).map(|i| (st.next + i) % chain.images.len())
        .find(|i| !st.acquired.iter().any(|(a, _)| a == i) && st.released != Some(*i)).ok_or(CALL_ORDER_INVALID)?;
    st.acquired.push_back((index, false)); st.used = true; st.next = (index + 1) % chain.images.len(); *out = index as u32; Ok(())
});
api!(wait_swapchain(id: Handle, info: *const SwapchainWaitInfo) {
    let info = input(info, 56)?; if info.timeout < 0 { return Err(VALIDATION); }
    let chain = swapchain(id)?; let mut st = lock(&chain.state);
    let (_, waited) = st.acquired.iter_mut().find(|(_, waited)| !waited).ok_or(CALL_ORDER_INVALID)?;
    // EndFrame finishes the transfer before it returns: free images need no
    // deferred GPU wait here, including a zero-duration poll.
    *waited = true; Ok(())
});
api!(release_swapchain(id: Handle, info: *const SwapchainReleaseInfo) {
    if !info.is_null() { input(info, 57)?; } let chain = swapchain(id)?; let mut st = lock(&chain.state);
    let (index, waited) = st.acquired.front().copied().ok_or(CALL_ORDER_INVALID)?;
    if !waited { return Err(CALL_ORDER_INVALID); }
    st.acquired.pop_front(); st.released = Some(index); Ok(())
});
fn source(id: Handle, image: SwapchainSubImage) -> Xr<Arc<Image>> {
    let chain = swapchain(image.swapchain)?;
    if chain.session != id { return Err(HANDLE_INVALID); }
    let index = lock(&chain.state).released.ok_or(CALL_ORDER_INVALID)?;
    let src = chain.images[index].clone();
    let [x,y] = image.rect.offset; let [w,h] = image.rect.extent;
    if x < 0 || y < 0 || w <= 0 || h <= 0 || (x as u64 + w as u64) > src.width as u64 || (y as u64 + h as u64) > src.height as u64 { return Err(SWAPCHAIN_RECT_INVALID); }
    if image.array_index >= src.layers { return Err(INDEX_OUT_OF_RANGE); }
    Ok(src)
}

pub(super) fn haptics(s: &Session) -> Xr<()> {
    let pending = std::mem::take(&mut lock(&s.state).haptics);
    let mut c = lock(&s.connection);
    for (hand, pulse) in pending.into_iter().enumerate() {
        if let Some((amplitude, duration_ns)) = pulse { c.send(&Message::Haptic { hand: hand as u32, amplitude, duration_ns }, None).map_err(io_error)?; }
    } Ok(())
}
pub(super) unsafe fn projection(id: Handle, s: &Session, info: &FrameEndInfo, tracking: &Tracking) -> Xr<()> {
    let layers = array(info.layers, info.layer_count)?;
    if layers.len() != 1 { return Err(LAYER_INVALID); }
    let layer = input(layers[0].cast::<ProjectionLayer>(), 35)?;
    if layer.view_count != 2 || layer.flags & !7 != 0 { return Err(LAYER_INVALID); }
    let views = array(layer.views, 2)?;
    let base = space_pose(&registry(), layer.space, s, tracking)?;
    let retired_count = lock(&s.retired).len();
    let mut targets = lock(&s.exports);
    let mut submitted = Vec::new();
    for (n, view) in views.iter().enumerate() {
        input(view, 48)?;
        if !valid_pose(view.pose) { return Err(POSE_INVALID); }
        if !view.fov.iter().all(|v| v.is_finite() && v.abs() < std::f32::consts::FRAC_PI_2)
            || view.fov[0] >= view.fov[1] || view.fov[3] >= view.fov[2] { return Err(VALIDATION); }
        let src = source(id, view.image)?;
        let size = view.image.rect.extent.map(|v| v as u32);
        let srgb = matches!(src.format, vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB);
        let reuse = targets[n].as_ref().is_some_and(|t| t.image.width == size[0] && t.image.height == size[1]
            && (t.image.format == vk::Format::B8G8R8A8_SRGB) == srgb);
        if !reuse {
            if let Some(old) = targets[n].take() { lock(&s.connection).send(&Message::Forget { id: old.descriptor.id }, None).map_err(io_error)?; }
            let ticket = registry().id();
            targets[n] = Some(s.gpu.export(ticket, size, srgb, &s.headset.formats)?);
        }
        let target: &mut Export = targets[n].as_mut().unwrap();
        s.gpu.copy_eye(&src, view.image.rect, view.image.array_index, target)?;
        if !reuse {
            let mut c = lock(&s.connection);
            c.send(&Message::Import(target.descriptor.clone()), Some(&target.fd)).map_err(io_error)?;
            match c.recv().map_err(io_error)?.0 {
                Message::Imported { id } if id == target.descriptor.id => {},
                Message::Error(e) => return Err(io_error(e)),
                _ => return Err(RUNTIME_FAILURE),
            }
        }
        let rendered = compose(base, view.pose);
        submitted.push(SubmittedEye { image: target.descriptor.id, eye: crate::protocol::Eye {
            pose: crate::protocol::Pose { orientation: rendered.orientation, position: rendered.position }, fov: view.fov } });
    }
    // copy_eye waited for the graphics queue's transfer fence. Keep anything
    // retired concurrently with this frame for the following frame's fence.
    if retired_count != 0 {
        let completed: Vec<_> = lock(&s.retired).drain(..retired_count).collect();
        drop(completed);
    }
    let eyes = [submitted[0].clone(), submitted[1].clone()];
    let mut c = lock(&s.connection);
    c.send(&Message::Frame { token: tracking.token, eyes, alpha: false }, None).map_err(io_error)?;
    match c.recv().map_err(io_error)?.0 {
        Message::Released { token } if token == tracking.token => Ok(()),
        Message::Error(e) => Err(io_error(e)),
        _ => Err(SESSION_LOST),
    }
}
