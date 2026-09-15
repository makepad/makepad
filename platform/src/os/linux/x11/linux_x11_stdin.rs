use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    draw_pass::{CxDrawPassColorTexture, CxDrawPassParent, DrawPassClearColor},
    event::{Event, WindowGeom},
    makepad_math::*,
    makepad_micro_serde::*,
    os::shared_framebuf::{
        aux_chan, HostPresentableImage,
        HostSwapchain, PollTimer, PresentableDraw,
    },
    texture::{Texture, TextureFormat},
    thread::SignalToUI,
    web_socket::WebSocketMessage,
    window::CxWindowPool,
    CxOsApi,
};
#[cfg(not(use_vulkan))]
use crate::{gl_sys, texture::TextureSize, os::shared_framebuf::LinuxSharedSoftwareBuffer};
use makepad_studio_protocol::{AppToStudio, GCSample, StudioToApp, StudioToAppVec};
#[cfg(not(all(use_vulkan, linux_direct)))]
use crate::os::shared_framebuf::shared_presentable_image_recv_fds_from_aux_chan;

#[derive(Default)]
pub(crate) struct StdinWindow {
    swapchain: Option<HostSwapchain>,
    present_index: usize,
    #[cfg(all(use_vulkan, linux_direct))]
    pending_gpu_swapchain: Option<(u64, u64, HostSwapchain)>,
    #[cfg(all(use_vulkan, linux_direct))]
    retired_gpu_swapchain: Option<HostSwapchain>,
    #[cfg(all(use_vulkan, linux_direct))]
    gpu_transport_epoch: u64,
    #[cfg(all(use_vulkan, linux_direct))]
    gpu_window_id: Option<crate::window::WindowId>,
    #[cfg(all(use_vulkan, linux_direct))]
    gpu_host_announced: bool,
    #[cfg(all(use_vulkan, linux_direct))]
    gpu_ready_epoch: u64,
    #[cfg(not(use_vulkan))]
    readback_framebuffer: Option<u32>,
    last_trace_draw: Option<(u32, u32, u32, u32, u64)>,
}

#[cfg(all(use_vulkan, linux_direct))]
impl Cx {
    fn stdin_gpu_event_loop(&mut self, windows: &mut Vec<StdinWindow>) {
        use std::collections::VecDeque;
        let mut pending = VecDeque::new();
        let mut deferred = VecDeque::new();
        let mut backlog_reported = false;
        let mut pressure_reported = false;
        let mut socket_closed = false;
        loop {
            self.stdin_poll_routed_presents();
            self.stdin_poll_gpu_inbox(windows);
            self.linux_poll_gpu_transition();
            self.stdin_report_gpu_transition(windows);
            self.stdin_poll_gpu_ready(windows);
            if !Self::has_studio_web_socket() { break; }
            if !self.os.gpu_callbacks_paused && !deferred.is_empty() {
                deferred.append(&mut pending);
                std::mem::swap(&mut pending, &mut deferred);
                backlog_reported = false;
            }
            if pending.is_empty() {
                if socket_closed { break; }
                // Take the whole queued backlog, not one batch: a child that
                // fell behind then collapses the queued Ticks into one draw
                // and the queued pointer positions into the latest one.
                let mut batch = Vec::new();
                loop {
                    match self.try_recv_studio_websocket_message() {
                        Some(WebSocketMessage::Binary(data)) => match StudioToAppVec::deserialize_bin(&data) {
                            Ok(messages) => batch.extend(messages.0),
                            Err(error) => crate::error!("Invalid hosted binary command: {error:?}"),
                        },
                        Some(WebSocketMessage::String(text)) => match StudioToApp::deserialize_json(&text) {
                            Ok(message) => batch.push(message),
                            Err(error) => crate::error!("Invalid hosted text command: {error:?}"),
                        },
                        Some(WebSocketMessage::Error(error)) => { crate::error!("Hosted websocket: {error}"); socket_closed = true; break; }
                        Some(WebSocketMessage::Closed) => { socket_closed = true; break; }
                        Some(WebSocketMessage::Opened) => {}
                        None => break,
                    }
                }
                Self::stdin_coalesce_host_batch(&mut batch);
                pending.extend(batch);
                if socket_closed && pending.is_empty() { break; }
            }
            let mut processed = 0;
            while let Some(message) = pending.front() {
                let needs_inbox = matches!(message, StudioToApp::Swapchain(_)
                    | StudioToApp::Gpu(makepad_studio_protocol::HostToAppGpu::Swapchain { .. }));
                if needs_inbox && self.os.gpu_inbox.as_ref().is_some_and(|inbox| !inbox.has_capacity()) {
                    // Retain the command and its position in the descriptor
                    // stream. Never read another websocket batch until this
                    // bounded worker queue has credit again.
                    if !pressure_reported { crate::warning!("Hosted GPU inbox full; retaining command for retry"); }
                    pressure_reported = true;
                    break;
                }
                pressure_reported = false;
                let message = pending.pop_front().unwrap();
                if self.os.gpu_callbacks_paused
                    && !matches!(message, StudioToApp::Gpu(_) | StudioToApp::Swapchain(_) | StudioToApp::Kill) {
                    // Acknowledged window attachments cannot change before
                    // Commit. Keep input in order; redundant frame pulses and
                    // consecutive pointer positions need no deferred replay.
                    if !matches!(message, StudioToApp::Tick | StudioToApp::KeepAlive | StudioToApp::None) {
                        if matches!(message, StudioToApp::MouseMove(_)) && matches!(deferred.back(), Some(StudioToApp::MouseMove(_))) {
                            deferred.pop_back();
                        }
                        deferred.push_back(message);
                        if deferred.len() >= 256 && !backlog_reported {
                            backlog_reported = true;
                            Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed {
                                transition: self.linux_gpu_snapshot().transition,
                                message: "GPU handoff is holding a large input backlog; finish or cancel it".into(),
                            });
                        }
                    }
                } else if self.stdin_handle_host_to_stdin(message, &mut None, windows) { return; }
                if !self.os.gpu_callbacks_paused && !deferred.is_empty() {
                    // Retire/Cancel can share a packet with fresh input.
                    // Replay older deferred edges before that packet's tail.
                    deferred.append(&mut pending);
                    std::mem::swap(&mut pending, &mut deferred);
                    backlog_reported = false;
                }
                processed += 1;
                if processed == 64 { break; }
            }
            if !self.os.gpu_callbacks_paused {
                self.handle_actions();
                self.run_live_edit_if_needed("linux-stdin");
            }
            if processed == 0 {
                // An event-loop idle interval, independent of any worker or
                // network channel. GPU/descriptor completion is polled again
                // even if the host has stopped sending frame ticks.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    fn stdin_gpu_reply(reply: makepad_studio_protocol::AppToHostGpu) {
        Self::stdin_send_to_host(AppToStudio::Gpu(reply));
    }

    fn stdin_poll_routed_presents(&mut self) {
        let Some(gpu) = self.os.vulkan.as_mut() else { return; };
        match gpu.poll_routed_presents() {
            Ok(draws) => for draw in draws { Self::stdin_send_to_host(AppToStudio::DrawCompleteAndFlip(draw)); },
            Err(message) => {
                crate::error!("Hosted GPU framebuffer route: {message}");
                Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed {
                    transition: self.linux_gpu_snapshot().transition, message,
                });
            }
        }
    }

    fn stdin_gpu_hello(&self) {
        if let Some(device) = self.linux_gpu_snapshot().renderer {
            Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Hello {
                version: makepad_studio_protocol::GPU_CONTROL_VERSION,
                renderer_generation: self.linux_gpu_snapshot().renderer_generation,
                device: makepad_studio_protocol::GpuDeviceIdentity {
                    device_uuid: device.uuid, driver_uuid: device.driver_uuid,
                },
            });
        }
    }

    fn stdin_queue_gpu_swapchain(&mut self, swapchain: makepad_studio_protocol::SharedSwapchain,
        transition: Option<(u64, u64)>) -> Result<(), String> {
        self.stdin_start_gpu_inbox()?;
        let window_id = self.windows.id_iter().find(|id| id.id() == swapchain.window_id);
        let renderer_generation = if transition.is_none()
            && self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending()) {
            // Consume the descriptor batch to preserve stream framing, but
            // never admit ordinary transport during a renderer transaction.
            0
        } else { self.linux_gpu_snapshot().renderer_generation };
        self.os.gpu_inbox.as_mut().ok_or("GPU descriptor inbox unavailable")?
            .enqueue(crate::os::linux::hosted_gpu::SwapchainRequest {
                transition, renderer_generation, window_id, swapchain,
            })
    }

    fn stdin_start_gpu_inbox(&mut self) -> Result<(), String> {
        if self.os.gpu_inbox.is_none() {
            self.os.gpu_inbox = Some(crate::os::linux::hosted_gpu::GpuInbox::start(&self.thread_spawner())?);
        }
        Ok(())
    }

    fn stdin_install_gpu_window(&mut self, index: usize, window: &mut StdinWindow, swapchain: HostSwapchain) {
        window.present_index = 0;
        window.last_trace_draw = None;
        let Some(id) = window.gpu_window_id else { return; };
        debug_assert_eq!(id.id(), index);
        if let Some(pass_id) = self.windows[id].main_pass_id {
            let pass = &mut self.passes[pass_id];
            pass.color_textures = vec![CxDrawPassColorTexture {
                texture: swapchain.presentable_images[0].texture.clone(),
                clear_color: DrawPassClearColor::ClearWith(pass.clear_color), cube_face: None,
            }];
        }
        window.swapchain = Some(swapchain);
        self.redraw_all();
    }

    fn stdin_accept_gpu_swapchain(&mut self, batch: crate::os::linux::hosted_gpu::LoadedSwapchain,
        windows: &mut [StdinWindow]) -> Result<(), String> {
        let shared = batch.request.swapchain;
        let window = windows.get_mut(shared.window_id).ok_or("swapchain names an unknown window")?;
        let current_id = self.windows.id_iter().find(|id| id.id() == shared.window_id);
        if batch.request.window_id.is_none() || batch.request.window_id != current_id
            || batch.request.window_id != window.gpu_window_id
            || !self.windows[current_id.unwrap()].is_created {
            return Err("swapchain names a retired window incarnation".into());
        }
        let snapshot = self.linux_gpu_snapshot();
        if let Some((transition, epoch)) = batch.request.transition {
            if snapshot.transition != transition || snapshot.phase != crate::linux_gpu::LinuxGpuPhase::Prepared
                || epoch <= window.gpu_transport_epoch
                || window.pending_gpu_swapchain.as_ref().is_some_and(|(_, pending, _)| epoch <= *pending) {
                return Err("stale or unprepared GPU swapchain".into());
            }
        } else if batch.request.renderer_generation != snapshot.renderer_generation
            || self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending()) {
            return Err("ordinary swapchain belongs to an inactive renderer generation".into());
        }
        if batch.request.transition.is_none() && window.swapchain.as_ref().is_some_and(|active|
            active.window_id == shared.window_id && active.alloc_width == shared.alloc_width
                && active.alloc_height == shared.alloc_height
                && active.presentable_images.iter().zip(&shared.presentable_images)
                    .all(|(resident, incoming)| resident.id == incoming.id)) {
            // Bootstrap retries own a new set of FDs for the same allocation.
            // They have been drained already. Retain the original producer's
            // pending bridge and timeline sequence instead of importing a
            // second writer for that memory while its first frame is in flight.
            return Ok(());
        }
        let mut images = Vec::new();
        for image in batch.images {
            let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
                id: image.id, width: shared.alloc_width as usize, height: shared.alloc_height as usize, initial: true,
            });
            let gpu = self.os.vulkan.as_mut().ok_or("hosted Vulkan renderer unavailable")?;
            if let Some((transition, _)) = batch.request.transition {
                gpu.import_transition_image(transition, &texture, shared.alloc_width, shared.alloc_height, image.image)?;
            } else {
                gpu.import_shared_image(&texture, shared.alloc_width, shared.alloc_height, image.image)?;
            }
            images.push(HostPresentableImage { id: image.id, texture, software_buffer: None });
        }
        let swapchain = HostSwapchain {
            window_id: shared.window_id, alloc_width: shared.alloc_width, alloc_height: shared.alloc_height,
            presentable_images: images.try_into().map_err(|_| "incomplete GPU framebuffer batch")?,
        };
        if let Some((transition, epoch)) = batch.request.transition {
            window.pending_gpu_swapchain = Some((transition, epoch, swapchain));
        } else {
            self.stdin_install_gpu_window(shared.window_id, window, swapchain);
        }
        Ok(())
    }

    fn stdin_poll_gpu_inbox(&mut self, windows: &mut [StdinWindow]) {
        let Some(inbox) = self.os.gpu_inbox.as_mut() else { return; };
        match inbox.poll() {
            Ok(batches) => for batch in batches {
                let transition = batch.request.transition;
                if let Err(error) = self.stdin_accept_gpu_swapchain(batch, windows) {
                    if let Some((transition, _)) = transition {
                        Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed { transition, message: error });
                    } else { crate::error!("Hosted GPU import: {error}"); }
                }
            },
            Err(error) => {
                let transition = self.linux_gpu_snapshot().transition;
                if self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending()) {
                    Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed { transition, message: error });
                } else { crate::error!("Hosted GPU descriptor inbox: {error}"); }
            }
        }
    }

    fn stdin_gpu_control(&mut self, control: makepad_studio_protocol::HostToAppGpu, windows: &mut Vec<StdinWindow>) {
        use makepad_studio_protocol::{AppToHostGpu as Reply, HostToAppGpu as Command};
        use crate::linux_gpu::LinuxGpuPhase;
        let id = control.transition();
        self.stdin_poll_gpu_inbox(windows);
        let result = (|| -> Result<(), String> {
            match control {
                Command::Prepare { transition, renderer_generation, target_device_uuid, windows: required } => {
                    let mut participants = Vec::new();
                    for index in required {
                        let id = self.windows.id_iter().find(|id| id.id() == index)
                            .ok_or("GPU preparation names an unknown window")?;
                        if !self.windows[id].is_created || !windows.get(index)
                            .is_some_and(|window| window.gpu_window_id == Some(id) && window.gpu_host_announced) {
                            return Err("GPU preparation names an unannounced window incarnation".into());
                        }
                        if participants.contains(&id) { return Err("duplicate GPU window participant".into()); }
                        participants.push(id);
                    }
                    self.linux_prepare_gpu(transition, renderer_generation, target_device_uuid)?;
                    self.os.gpu_participants = participants;
                    self.os.gpu_callbacks_paused = true;
                }
                Command::Poll { .. } => {
                    if self.linux_gpu_snapshot().transition != id { return Err("stale GPU poll".into()); }
                }
                Command::Swapchain { transition, transport_epoch, swapchain } => {
                    // The host has already sent these descriptors. Even a
                    // cancelled transaction must drain its complete batch;
                    // admission is checked after the worker owns the FDs.
                    self.stdin_queue_gpu_swapchain(swapchain, Some((transition, transport_epoch)))?;
                }
                Command::Commit { transition } => {
                    let snapshot = self.linux_gpu_snapshot();
                    if snapshot.transition == transition
                        && matches!(snapshot.phase, LinuxGpuPhase::Committed | LinuxGpuPhase::Idle) {
                        Self::stdin_gpu_reply(Reply::Committed { transition, renderer_generation: snapshot.renderer_generation });
                        return Ok(());
                    }
                    for &window_id in &self.os.gpu_participants {
                        if !self.windows.is_valid(window_id) { return Err("GPU window incarnation changed before commit".into()); }
                        let cxwindow = &self.windows[window_id];
                        if !cxwindow.is_created { return Err("GPU window closed before commit".into()); }
                        let window = windows.get(window_id.id()).ok_or("GPU commit names an unknown window")?;
                        let geom = &cxwindow.window_geom;
                        let width = (geom.inner_size.x * geom.dpi_factor).max(1.0) as u32;
                        let height = (geom.inner_size.y * geom.dpi_factor).max(1.0) as u32;
                        if window.gpu_window_id != Some(window_id) || !window.pending_gpu_swapchain.as_ref()
                            .is_some_and(|(pending, epoch, swapchain)| *pending == transition && window.gpu_ready_epoch == *epoch
                                && swapchain.alloc_width >= width && swapchain.alloc_height >= height) {
                            return Err("GPU commit needs a replacement framebuffer covering the current window geometry".into());
                        }
                    }
                    self.linux_commit_gpu(transition)?;
                }
                Command::Retire { transition } => {
                    self.linux_retire_gpu(transition)?;
                    self.os.gpu_participants.clear();
                    for window in windows.iter_mut() { window.retired_gpu_swapchain = None; }
                    self.os.gpu_callbacks_paused = false;
                    Self::stdin_gpu_reply(Reply::Retired { transition });
                    return Ok(());
                }
                Command::Cancel { transition } => {
                    let snapshot = self.linux_gpu_snapshot();
                    let pending = self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending());
                    if !pending && (snapshot.transition < transition
                        || (snapshot.transition == transition && matches!(snapshot.phase, LinuxGpuPhase::Cancelled | LinuxGpuPhase::Failed))) {
                        // Rejected Prepare never created a candidate. Retrying
                        // its cleanup is idempotent and cannot cancel another
                        // transaction or undo a committed renderer.
                        Self::stdin_gpu_reply(Reply::Cancelled { transition });
                        return Ok(());
                    }
                    self.linux_cancel_gpu(transition)?;
                }
            }
            self.linux_poll_gpu_transition();
            self.stdin_report_gpu_transition(windows);
            self.stdin_poll_gpu_ready(windows);
            Ok(())
        })();
        if let Err(message) = result { Self::stdin_gpu_reply(Reply::Failed { transition: id, message }); }
    }
    fn stdin_report_gpu_transition(&mut self, windows: &mut [StdinWindow]) {
        use makepad_studio_protocol::AppToHostGpu as Reply;
        use crate::linux_gpu::LinuxGpuPhase;
    let snapshot = self.linux_gpu_snapshot();
    let id = snapshot.transition;
    if self.os.gpu_reported == Some((id, snapshot.phase)) { return; }
    self.os.gpu_reported = Some((id, snapshot.phase));
    match snapshot.phase {
        LinuxGpuPhase::Copying => Self::stdin_gpu_reply(Reply::Quiesced { transition: id }),
        LinuxGpuPhase::Prepared => {
            Self::stdin_gpu_reply(Reply::Quiesced { transition: id });
            if let Some((generation, device)) = self.os.vulkan.as_ref().and_then(|gpu| gpu.prepared_gpu_identity(id)) {
                Self::stdin_gpu_reply(Reply::Prepared { transition: id, renderer_generation: generation,
                    device: makepad_studio_protocol::GpuDeviceIdentity { device_uuid: device.uuid, driver_uuid: device.driver_uuid } });
            }
        }
        LinuxGpuPhase::Committed => {
            for (index, window) in windows.iter_mut().enumerate() {
                if window.pending_gpu_swapchain.as_ref().is_some_and(|(pending, _, _)| *pending == id) {
                    let (_, epoch, swapchain) = window.pending_gpu_swapchain.take().unwrap();
                    window.retired_gpu_swapchain = window.swapchain.take();
                    window.gpu_transport_epoch = epoch;
                    self.stdin_install_gpu_window(index, window, swapchain);
                }
            }
            Self::stdin_gpu_reply(Reply::Committed { transition: id, renderer_generation: snapshot.renderer_generation });
        }
        LinuxGpuPhase::Cancelled | LinuxGpuPhase::Failed => {
            self.os.gpu_callbacks_paused = false;
            self.os.gpu_participants.clear();
            for window in windows.iter_mut() { window.pending_gpu_swapchain = None; }
            if let Some(message) = snapshot.error { Self::stdin_gpu_reply(Reply::Failed { transition: id, message }); }
            Self::stdin_gpu_reply(Reply::Cancelled { transition: id });
        }
        _ => {}
    }
    }

    fn stdin_poll_gpu_ready(&mut self, windows: &mut [StdinWindow]) {
        let snapshot = self.linux_gpu_snapshot();
        if snapshot.phase != crate::linux_gpu::LinuxGpuPhase::Prepared { return; }
        let ready = self.os.vulkan.as_mut().unwrap().prepared_imports_ready(snapshot.transition);
        match ready {
            Ok(false) => return,
            Err(message) => {
                Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed { transition: snapshot.transition, message });
                let _ = self.linux_cancel_gpu(snapshot.transition);
                return;
            }
            Ok(true) => {},
        }
        for (index, window) in windows.iter_mut().enumerate() {
            let Some((transition, epoch, _)) = &window.pending_gpu_swapchain else { continue; };
            if *transition != snapshot.transition || window.gpu_ready_epoch == *epoch { continue; }
            window.gpu_ready_epoch = *epoch;
            Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::SwapchainReady {
                transition: *transition, transport_epoch: *epoch, window_id: index,
            });
        }
    }

}

impl Cx {
    #[cfg(use_vulkan)]
    fn stdin_vulkan_draw_pass(&mut self, draw_pass_id: crate::draw_pass::DrawPassId) -> Result<(), String> {
        let mut vulkan = self.os.vulkan.take().expect("hosted Vulkan renderer initialized");
        let result = vulkan.draw_pass_to_texture(self, draw_pass_id);
        self.os.vulkan = Some(vulkan);
        result
    }


    fn stdin_send_to_host(msg: AppToStudio) {
        Cx::send_studio_message(msg);
    }

    pub(crate) fn stdin_handle_repaint(&mut self, windows: &mut Vec<StdinWindow>) {
        #[cfg(all(use_vulkan, linux_direct))]
        if self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending()) { return; }
        #[cfg(not(use_vulkan))]
        self.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;

        let time_now = self.os.stdin_timers.time_now();
        for &draw_pass_id in &passes_todo {
            let uniforms_gen = self.next_uniform_gen();
            self.passes[draw_pass_id].set_time(time_now as f32, uniforms_gen);
            match self.passes[draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    // only render to swapchain if swapchain exists
                    let window = &mut windows[window_id.id()];
                    if let Some(swapchain) = &mut window.swapchain {
                        #[cfg(all(use_vulkan, linux_direct))]
                        {
                            let dpi = self.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
                            let Some(rect) = self.get_pass_rect(draw_pass_id, dpi) else { continue; };
                            if window.gpu_window_id != Some(window_id)
                                || (rect.size.x * dpi).max(1.0) as u32 > swapchain.alloc_width
                                || (rect.size.y * dpi).max(1.0) as u32 > swapchain.alloc_height {
                                crate::trace!("runview.blocked", "window geometry pass={:?} window={:?}/{:?} rect={:?} dpi={} alloc={}x{}", draw_pass_id, window.gpu_window_id, window_id, rect, dpi, swapchain.alloc_width, swapchain.alloc_height);
                                // Keep paint_dirty set while the descriptor worker
                                // imports a sufficiently large replacement.
                                continue;
                            }
                        }
                        #[cfg(not(use_vulkan))]
                        let current_index = window.present_index;
                        #[cfg(use_vulkan)]
                        let Some(current_index) = (0..swapchain.presentable_images.len())
                            .map(|offset| (window.present_index + offset) % swapchain.presentable_images.len())
                            .find(|index| self.os.vulkan.as_ref().unwrap()
                                .shared_write_available(swapchain.presentable_images[*index].texture.texture_id())
                                .unwrap_or_else(|error| panic!("Shared Vulkan image availability: {error}")))
                        else { continue; };
                        window.present_index =
                            (current_index + 1) % swapchain.presentable_images.len();
                        let current_image = &mut swapchain.presentable_images[current_index];

                        // render to swapchain
                        #[cfg(not(use_vulkan))]
                        self.draw_pass_to_texture(draw_pass_id, Some(&current_image.texture));
                        #[cfg(use_vulkan)]
                        {
                            let pass = &mut self.passes[draw_pass_id];
                            pass.color_textures[0].texture = current_image.texture.clone();
                            // Match the GL target override: preserve the app's
                            // window clear, including transparency and changes
                            // made after this swapchain was allocated.
                            pass.color_textures[0].clear_color =
                                DrawPassClearColor::ClearWith(pass.clear_color);
                            if let Err(error) = self.stdin_vulkan_draw_pass(draw_pass_id) {
                                // A failed submission leaves the command/fence
                                // state unusable. Let the host report the exit
                                // instead of flooding it with failed retries.
                                panic!("Vulkan hosted draw failed: {error}");
                            }
                        }

                        // wait for GPU to finish rendering
                        #[cfg(not(use_vulkan))]
                        unsafe {
                            (self.os.gl().glFinish)();
                        }

                        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
                        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
                        let presentable_draw = PresentableDraw {
                            #[cfg(not(use_vulkan))]
                            sequence: 0,
                            #[cfg(use_vulkan)]
                            sequence: self.os.vulkan.as_ref().unwrap().shared_draw_sequence(current_image.texture.texture_id()),
                            window_id: window_id.id(),
                            target_id: current_image.id,
                            width: (pass_rect.size.x * dpi_factor) as u32,
                            height: (pass_rect.size.y * dpi_factor) as u32,
                        };

                        if crate::makepad_error_log::trace_enabled("runview.dpi") {
                            let trace_draw = (
                                presentable_draw.width,
                                presentable_draw.height,
                                swapchain.alloc_width,
                                swapchain.alloc_height,
                                dpi_factor.to_bits(),
                            );
                            let should_log = window.last_trace_draw != Some(trace_draw);
                            window.last_trace_draw = Some(trace_draw);
                            if should_log {
                                crate::trace!(
                                    "runview.dpi",
                                    "runview child draw window={} logical=({}, {}) dpi={} frame_px=({}, {}) swapchain_alloc=({}, {})",
                                    window_id.id(),
                                    pass_rect.size.x,
                                    pass_rect.size.y,
                                    dpi_factor,
                                    presentable_draw.width,
                                    presentable_draw.height,
                                    swapchain.alloc_width,
                                    swapchain.alloc_height
                                );
                            }
                        }

                        #[cfg(not(use_vulkan))]
                        if let Some(software_buffer) = current_image.software_buffer.as_mut() {
                            #[cfg(not(use_vulkan))]
                            software_buffer.as_bytes_mut().fill(0);
                            #[cfg(not(use_vulkan))]
                            unsafe {
                                let gl = self.os.gl();

                                while (gl.glGetError)() != 0 {}

                                if window.readback_framebuffer.is_none() {
                                    let mut framebuffer = std::mem::MaybeUninit::uninit();
                                    (gl.glGenFramebuffers)(1, framebuffer.as_mut_ptr());
                                    window.readback_framebuffer = Some(framebuffer.assume_init());
                                }
                                let readback_framebuffer = window.readback_framebuffer.unwrap();
                                let gl_texture = match self.textures
                                    [current_image.texture.texture_id()]
                                .os
                                .gl_texture
                                {
                                    Some(texture) => texture,
                                    None => continue,
                                };

                                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, readback_framebuffer);
                                (gl.glFramebufferTexture2D)(
                                    gl_sys::FRAMEBUFFER,
                                    gl_sys::COLOR_ATTACHMENT0,
                                    gl_sys::TEXTURE_2D,
                                    gl_texture,
                                    0,
                                );
                                (gl.glPixelStorei)(gl_sys::PACK_ALIGNMENT, 1);
                                (gl.glPixelStorei)(gl_sys::PACK_ROW_LENGTH, 0);
                                (gl.glPixelStorei)(gl_sys::PACK_SKIP_PIXELS, 0);
                                (gl.glPixelStorei)(gl_sys::PACK_SKIP_ROWS, 0);
                                (gl.glReadPixels)(
                                    0,
                                    0,
                                    swapchain.alloc_width as i32,
                                    swapchain.alloc_height as i32,
                                    gl_sys::RGBA,
                                    gl_sys::UNSIGNED_BYTE,
                                    software_buffer.as_mut_ptr(),
                                );
                                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);

                                let gl_error = (gl.glGetError)();
                                if gl_error != 0 {
                                    crate::error!(
                                        "software fallback readback glReadPixels error={}",
                                        gl_error
                                    );
                                }
                            }

                            // Keep RunView size pixels in-band, matching other backends.
                            let encode_size_pixel = |size: u32| {
                                [((size >> 8) & 0xff) as u8, 0, (size & 0xff) as u8, 0xff]
                            };
                            if let Ok(stride) = usize::try_from(software_buffer.stride) {
                                let width_px = encode_size_pixel(presentable_draw.width);
                                let height_px = encode_size_pixel(presentable_draw.height);
                                let bytes = software_buffer.as_bytes_mut();
                                if stride >= 8 && bytes.len() >= 8 {
                                    bytes[0..4].copy_from_slice(&width_px);
                                    bytes[4..8].copy_from_slice(&height_px);

                                    if swapchain.alloc_height > 1 {
                                        let last_row = (swapchain.alloc_height as usize - 1)
                                            .saturating_mul(stride);
                                        if last_row + 8 <= bytes.len() {
                                            bytes[last_row..last_row + 4]
                                                .copy_from_slice(&width_px);
                                            bytes[last_row + 4..last_row + 8]
                                                .copy_from_slice(&height_px);
                                        }
                                    }
                                }
                            }
                        }

                        // inform host that frame is ready
                        #[cfg(all(use_vulkan, linux_direct))]
                        match self.os.vulkan.as_mut().unwrap().queue_routed_present(&current_image.texture, presentable_draw) {
                            Ok(true) => continue,
                            Ok(false) => {},
                            Err(message) => {
                                crate::error!("Hosted Vulkan framebuffer routing: {message}");
                                Self::stdin_gpu_reply(makepad_studio_protocol::AppToHostGpu::Failed {
                                    transition: self.linux_gpu_snapshot().transition, message,
                                });
                                continue;
                            }
                        }
                        Self::stdin_send_to_host(AppToStudio::DrawCompleteAndFlip(
                            presentable_draw,
                        ));
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    #[cfg(not(use_vulkan))]
                    self.draw_pass_to_texture(draw_pass_id, None);
                    #[cfg(use_vulkan)]
                    if let Err(error) = self.stdin_vulkan_draw_pass(draw_pass_id) {
                        crate::error!("Vulkan hosted offscreen pass failed: {error}");
                    }
                }
                CxDrawPassParent::None => {
                    #[cfg(not(use_vulkan))]
                    self.draw_pass_to_texture(draw_pass_id, None);
                    #[cfg(use_vulkan)]
                    if let Err(error) = self.stdin_vulkan_draw_pass(draw_pass_id) {
                        crate::error!("Vulkan hosted offscreen pass failed: {error}");
                    }
                }
            }
        }
    }

    #[cfg(not(all(use_vulkan, linux_direct)))]
    fn stdin_aux_chan_endpoint(
        aux_chan_client_endpoint: &mut Option<aux_chan::ClientEndpoint>,
    ) -> Option<&aux_chan::ClientEndpoint> {
        if aux_chan_client_endpoint.is_none() {
            match aux_chan::ClientEndpoint::connect_from_studio_env() {
                Ok(endpoint) => {
                    *aux_chan_client_endpoint = Some(endpoint);
                }
                Err(err) => {
                    crate::error!("failed to acquire auxiliary channel: {}", err);
                    return None;
                }
            }
        }
        aux_chan_client_endpoint.as_ref()
    }

    pub fn stdin_event_loop(&mut self) {
        Self::stdin_send_to_host(AppToStudio::BeforeStartup);

        let mut stdin_windows: Vec<StdinWindow> = Vec::new();
        #[cfg(not(all(use_vulkan, linux_direct)))]
        let mut aux_chan_client_endpoint = None;

        self.set_physical_keyboard_state(true);
        self.call_event_handler(&Event::Startup);
        Self::stdin_send_to_host(AppToStudio::AfterStartup);
        #[cfg(all(use_vulkan, linux_direct))]
        self.stdin_gpu_hello();
        self.stdin_handle_platform_ops(&mut stdin_windows);

        #[cfg(all(use_vulkan, linux_direct))]
        self.stdin_gpu_event_loop(&mut stdin_windows);
        #[cfg(not(all(use_vulkan, linux_direct)))]
        'stdin_loop: loop {
            if !Self::has_studio_web_socket() {
                crate::error!("--stdin-loop mode requires a studio websocket");
                break;
            }
            let incoming = match self.recv_studio_websocket_message() {
                Some(incoming) => incoming,
                None => break,
            };

            match incoming {
                WebSocketMessage::Binary(data) => match StudioToAppVec::deserialize_bin(&data) {
                    Ok(msgs) => {
                        let mut batch = msgs.0;
                        let closed = self.stdin_drain_host_batches(&mut batch);
                        Self::stdin_coalesce_host_batch(&mut batch);
                        for msg in batch {
                            if self.stdin_handle_host_to_stdin(
                                msg,
                                &mut aux_chan_client_endpoint,
                                &mut stdin_windows,
                            ) {
                                break 'stdin_loop;
                            }
                        }
                        self.handle_actions();
                        if closed {
                            break 'stdin_loop;
                        }
                    }
                    Err(err) => {
                        crate::error!(
                            "Cant parse studio websocket binary payload in --stdin-loop: {:?}",
                            err
                        );
                    }
                },
                WebSocketMessage::String(text) => {
                    if let Ok(msg) = StudioToApp::deserialize_json(&text) {
                        if self.stdin_handle_host_to_stdin(
                            msg,
                            &mut aux_chan_client_endpoint,
                            &mut stdin_windows,
                        ) {
                            break 'stdin_loop;
                        }
                    } else if !text.trim().is_empty() {
                        crate::warning!(
                            "Ignoring unexpected studio websocket text: {}",
                            text.trim()
                        );
                    }
                }
                WebSocketMessage::Error(err) => {
                    crate::error!("Studio websocket error in --stdin-loop: {}", err);
                    break;
                }
                WebSocketMessage::Closed => break,
                WebSocketMessage::Opened => {}
            }
            self.run_live_edit_if_needed("linux-stdin");
        }
        #[cfg(all(use_vulkan, linux_direct))]
        self.os.gpu_inbox.take();
    }

    fn stdin_handle_host_to_stdin(
        &mut self,
        msg: StudioToApp,
        aux_chan_client_endpoint: &mut Option<aux_chan::ClientEndpoint>,
        stdin_windows: &mut Vec<StdinWindow>,
    ) -> bool {
        #[cfg(all(use_vulkan, linux_direct))]
        let _ = aux_chan_client_endpoint;
        match msg {
            #[cfg(all(use_vulkan, linux_direct))]
            StudioToApp::Gpu(control) => self.stdin_gpu_control(control, stdin_windows),
            // Mouse events: resolve window_id from coordinates (stdin mode
            // supports multiple virtual windows).
            StudioToApp::MouseDown(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::MouseMove(ref e) => {
                let (window_id, pos) = if let Some((_, window_id)) = self.fingers.first_mouse_button
                {
                    (window_id, self.windows[window_id].window_geom.position)
                } else {
                    self.windows.window_id_contains(dvec2(e.x, e.y))
                };
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::TweakRay(e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                let dpi_factor = self.windows[window_id].window_geom.dpi_factor.max(1.0);
                let tweak_ray = crate::event::TweakRayEvent {
                    abs: dvec2(e.x - pos.x, e.y - pos.y),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
                    dpi_factor,
                    hit_widget_uids: std::cell::RefCell::new(Vec::new()),
                    hit_rect: std::cell::Cell::new(None),
                };
                self.call_event_handler(&Event::TweakRay(tweak_ray));
            }
            StudioToApp::MouseUp(ref e) => {
                let (window_id, pos) = if let Some((_, window_id)) = self.fingers.first_mouse_button
                {
                    (window_id, self.windows[window_id].window_geom.position)
                } else {
                    self.windows.window_id_contains(dvec2(e.x, e.y))
                };
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::Scroll(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            // Stdin-specific: window geometry and swapchain management.
            StudioToApp::WindowGeomChange {
                dpi_factor,
                left: _left,
                top: _top,
                width,
                height,
                window_id,
            } => {
                #[cfg(all(use_vulkan, linux_direct))]
                if let Err(error) = self.stdin_start_gpu_inbox() { crate::error!("Hosted GPU startup: {error}"); }
                let window_id = CxWindowPool::from_usize(window_id);
                let old_geom = self.windows[window_id].window_geom.clone();
                let new_geom = WindowGeom {
                    dpi_factor,
                    position: dvec2(0.0, 0.0),
                    inner_size: dvec2(width, height),
                    ..Default::default()
                };
                let geom_changed = old_geom.dpi_factor != new_geom.dpi_factor
                    || old_geom.inner_size != new_geom.inner_size
                    || old_geom.position != new_geom.position;
                if geom_changed && crate::makepad_error_log::trace_enabled("runview.dpi") {
                    crate::trace!(
                        "runview.dpi",
                        "runview child geom window={} logical=({}, {}) dpi={} px=({}, {}) old_logical=({}, {}) old_dpi={}",
                        window_id.id(),
                        width,
                        height,
                        dpi_factor,
                        width * dpi_factor,
                        height * dpi_factor,
                        old_geom.inner_size.x,
                        old_geom.inner_size.y,
                        old_geom.dpi_factor
                    );
                }
                let re = self.windows.stdin_apply_native_geom(window_id, new_geom);
                if geom_changed || re.old_geom != re.new_geom {
                    self.redraw_all();
                    self.call_event_handler(&Event::WindowGeomChange(re));
                }
                #[cfg(not(all(use_vulkan, linux_direct)))]
                let _ = Self::stdin_aux_chan_endpoint(aux_chan_client_endpoint);
            }
            StudioToApp::Swapchain(new_swapchain) => {
                #[cfg(all(use_vulkan, linux_direct))]
                if let Err(error) = self.stdin_queue_gpu_swapchain(new_swapchain, None) {
                    crate::error!("Hosted GPU swapchain: {error}");
                }
                #[cfg(not(all(use_vulkan, linux_direct)))]
                {
                let Some(aux_chan_client_endpoint) =
                    Self::stdin_aux_chan_endpoint(aux_chan_client_endpoint)
                else {
                    return false;
                };
                let window_id = new_swapchain.window_id;
                let alloc_width = new_swapchain.alloc_width;
                let alloc_height = new_swapchain.alloc_height;
                let shared_images = new_swapchain.presentable_images;
                let presentable_images = std::array::from_fn(|i| {
                    let shared_pi = shared_images[i];
                    let mut texture = Texture::new(self);
                    #[cfg(not(use_vulkan))]
                    let mut software_buffer = None;
                    #[cfg(use_vulkan)]
                    let software_buffer = None;
                    match shared_presentable_image_recv_fds_from_aux_chan(
                        shared_pi,
                        aux_chan_client_endpoint,
                    ) {
                        Ok(pi) => {
                            #[cfg(use_vulkan)]
                            {
                                texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
                                    id: pi.id, width: alloc_width as usize, height: alloc_height as usize, initial: true,
                                });
                                self.os.vulkan.as_mut().unwrap().import_shared_image(&texture, alloc_width, alloc_height, pi.image)
                                    .unwrap_or_else(|error| panic!("Shared Vulkan image import: {error}"));
                            }
                            #[cfg(not(use_vulkan))]
                            if pi.image.is_software_fallback() {
                                texture = Texture::new_with_format(
                                    self,
                                    TextureFormat::RenderBGRAu8 {
                                        size: TextureSize::Fixed {
                                            width: alloc_width as usize,
                                            height: alloc_height as usize,
                                        },
                                        initial: true,
                                    },
                                );
                                let stride = pi.image.plane.stride;
                                let maybe_len =
                                    usize::try_from(alloc_height).ok().and_then(|height| {
                                        usize::try_from(stride)
                                            .ok()
                                            .and_then(|stride| stride.checked_mul(height))
                                    });
                                match maybe_len {
                                    Some(len) => {
                                        match LinuxSharedSoftwareBuffer::from_fd(
                                            pi.image.plane.dma_buf_fd,
                                            len,
                                            stride,
                                        ) {
                                            Ok(buffer) => software_buffer = Some(buffer),
                                            Err(err) => {
                                                crate::error!(
                                                    "failed to map software fallback swapchain image: {err:?}"
                                                );
                                            }
                                        }
                                    }
                                    None => {
                                        crate::error!(
                                            "software fallback swapchain size overflow ({alloc_width}x{alloc_height}, stride={stride})"
                                        );
                                    }
                                }
                            } else {
                                #[cfg(not(use_vulkan))]
                                {
                                let desc = TextureFormat::SharedBGRAu8 {
                                    id: pi.id,
                                    width: alloc_width as usize,
                                    height: alloc_height as usize,
                                    initial: true,
                                };
                                texture = Texture::new_with_format(self, desc);
                                self.textures[texture.texture_id()]
                                    .update_from_shared_dma_buf_image(
                                        self.os.gl(),
                                        self.os.opengl_cx.as_ref().unwrap(),
                                        &pi.image,
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            crate::error!(
                                "failed to receive new swapchain on auxiliary channel: {err:?}"
                            );
                        }
                    }
                    HostPresentableImage {
                        id: shared_pi.id,
                        texture,
                        software_buffer,
                    }
                });
                let new_swapchain = HostSwapchain {
                    window_id,
                    alloc_width,
                    alloc_height,
                    presentable_images,
                };
                let stdin_window = &mut stdin_windows[window_id];
                stdin_window.swapchain = Some(new_swapchain);
                stdin_window.present_index = 0;

                let window = &mut self.windows[CxWindowPool::from_usize(window_id)];
                let pass = &mut self.passes[window.main_pass_id.unwrap()];
                if let Some(swapchain) = &stdin_window.swapchain {
                    pass.color_textures = vec![CxDrawPassColorTexture {
                        clear_color: DrawPassClearColor::ClearWith(pass.clear_color),
                        texture: swapchain.presentable_images[stdin_window.present_index]
                            .texture
                            .clone(),
                        cube_face: None,
                    }];
                }

                self.redraw_all();
                self.stdin_handle_platform_ops(stdin_windows);
                }
            }
            StudioToApp::RunViewFrameRequest(_) => {}
            StudioToApp::Tick => {
                if SignalToUI::check_and_clear_ui_signal() {
                    self.handle_termination_signal();
                    self.handle_media_signals();
                    self.handle_script_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }
                self.poll_control_channel();

                let events = self.os.stdin_timers.get_dispatch();
                for event in events {
                    self.handle_script_timer(&event);
                    self.call_event_handler(&Event::Timer(event));
                }

                self.run_live_edit_if_needed("linux-stdin");
                #[cfg(not(all(use_vulkan, linux_direct)))]
                self.handle_networking_events();
                #[cfg(all(use_vulkan, linux_direct))]
                if self.os.gpu_control_net.is_some() {
                    self.handle_networking_events();
                } else {
                    self.dispatch_storage_responses();
                }
                self.stdin_handle_platform_ops(stdin_windows);

                let time_now = self.seconds_since_app_start();
                #[cfg(all(use_vulkan, linux_direct))]
                {
                    self.stdin_poll_gpu_inbox(stdin_windows);
                    self.linux_poll_gpu_transition();
                    self.stdin_report_gpu_transition(stdin_windows);
                    self.stdin_poll_gpu_ready(stdin_windows);
                    if self.os.vulkan.as_ref().is_some_and(|gpu| gpu.gpu_transition_pending()) {
                        // Consumed, not drawn: the host keeps pacing on the ack.
                        Self::stdin_send_to_host(AppToStudio::TickDone);
                        return false;
                    }
                }
                if !self.new_next_frames.is_empty() {
                    self.call_next_frame_event(time_now);
                }

                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    #[cfg(not(use_vulkan))]
                    self.opengl_compile_shaders();
                }

                self.stdin_handle_repaint(stdin_windows);

                let gc_start = self.seconds_since_app_start();
                let mut gc_heap_live = None;
                self.with_vm(|vm| {
                    if vm.heap().needs_gc() {
                        vm.gc();
                        gc_heap_live = Some(vm.heap().gc_live_len() as u64);
                    }
                });
                if let Some(heap_live) = gc_heap_live {
                    let gc_end = self.seconds_since_app_start();
                    Cx::send_studio_message(AppToStudio::GCSample(GCSample {
                        start: gc_start,
                        end: gc_end,
                        heap_live,
                    }));
                }
                // One Tick consumed: the host sends the next one on this,
                // never ahead of it (run_view.rs tick pacing).
                Self::stdin_send_to_host(AppToStudio::TickDone);
            }
            // All other variants (Key*, Text*, Screenshot, WidgetTreeDump,
            // Kill, KeepAlive, LiveChange, None) handled by shared dispatch.
            other => {
                return self.dispatch_studio_msg(other, CxWindowPool::id_zero(), dvec2(0.0, 0.0));
            }
        }
        false
    }

    fn stdin_handle_platform_ops(&mut self, stdin_windows: &mut Vec<StdinWindow>) {
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    while window_id.id() >= stdin_windows.len() {
                        stdin_windows.push(StdinWindow::default());
                    }
                    #[cfg(all(use_vulkan, linux_direct))]
                    { stdin_windows[window_id.id()] = StdinWindow {
                        gpu_window_id: Some(window_id), gpu_host_announced: true, ..Default::default()
                    }; }
                    let window = &mut self.windows[window_id];
                    window.is_created = true;
                    Self::stdin_send_to_host(AppToStudio::CreateWindow {
                        window_id: window_id.id(),
                        kind_id: window.kind_id,
                    });
                }
                CxOsOp::CreatePopupWindow { window_id, .. } => {
                    while window_id.id() >= stdin_windows.len() {
                        stdin_windows.push(StdinWindow::default());
                    }
                    #[cfg(all(use_vulkan, linux_direct))]
                    { stdin_windows[window_id.id()] = StdinWindow {
                        gpu_window_id: Some(window_id), ..Default::default()
                    }; }
                    self.windows[window_id].is_created = true;
                }
                CxOsOp::SetCursor(cursor) => {
                    Self::stdin_send_to_host(AppToStudio::SetCursor(cursor.into()));
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    self.os
                        .stdin_timers
                        .timers
                        .insert(timer_id, PollTimer::new(interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.stdin_timers.timers.remove(&timer_id);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::CopyToClipboard(content) => {
                    Self::stdin_send_to_host(AppToStudio::SetClipboard(content));
                }
                CxOsOp::StartExternalDragging { .. } => {
                    crate::error!(
                        "external file dragging is not available in the Studio stdin runtime"
                    );
                    self.call_event_handler(&Event::DragEnd);
                }
                _ => (), /*
                         CxOsOp::CloseWindow(_window_id) => {},
                         CxOsOp::MinimizeWindow(_window_id) => {},
                         CxOsOp::MaximizeWindow(_window_id) => {},
                         CxOsOp::RestoreWindow(_window_id) => {},
                         CxOsOp::FullscreenWindow(_window_id) => {},
                         CxOsOp::NormalizeWindow(_window_id) => {}
                         CxOsOp::SetTopmost(_window_id, _is_topmost) => {}
                         CxOsOp::XrStartPresenting(_) => {},
                         CxOsOp::XrStopPresenting(_) => {},
                         CxOsOp::ShowTextIME(_area, _pos, _config) => {},
                         CxOsOp::HideTextIME => {},
                         CxOsOp::SetCursor(_cursor) => {},
                         CxOsOp::StartTimer {timer_id, interval, repeats} => {},
                         CxOsOp::StopTimer(timer_id) => {},
                         CxOsOp::StartDragging(dragged_item) => {}
                         CxOsOp::UpdateMenu(menu) => {}*/
            }
        }
    }
}
