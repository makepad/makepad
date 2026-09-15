//! Linux GPU transitions coordinate the compositor and its hosted processes.
//! Vulkan objects stay in their owning processes; this protocol transfers
//! readiness and new framebuffer handles, never application CPU state.
use crate::{App, ClientId, MpRunView, WmDesk};
use makepad_studio_protocol::{
    AppToHostGpu, GpuDeviceIdentity, HostToAppGpu, StudioToApp, StudioToAppVec, GPU_CONTROL_VERSION,
};
use makepad_widgets::makepad_micro_serde::SerBin;
use makepad_widgets::makepad_platform::linux_gpu::LinuxGpuPhase;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone)]
struct Peer {
    version: u32,
    generation: u64,
    device: GpuDeviceIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Quiescing,
    Preparing,
    Importing,
    Committing,
    Retiring,
    Cancelling,
    CancelStalled,
}

struct Member {
    socket: u64,
    target: [u8; 16],
    windows: Vec<usize>,
    quiesced: bool,
    prepared: bool,
    exported: bool,
    ready: HashSet<usize>,
    committed: bool,
    cancelled: bool,
    view_committed: bool,
    commit_sent: bool,
    retry_at: f64,
    retired: bool,
    prepared_device: Option<GpuDeviceIdentity>,
}

struct Transition {
    id: u64,
    generation: u64,
    target: [u8; 16],
    compositor: bool,
    policy: Option<(ClientId, Option<[u8; 16]>)>,
    members: HashMap<ClientId, Member>,
    departed: Vec<ClientId>,
    phase: Phase,
    deadline: f64,
    wm_started: bool,
    wm_committed: bool,
    views_committed: bool,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct LinuxGpuController {
    peers: HashMap<ClientId, Peer>,
    policies: HashMap<ClientId, [u8; 16]>,
    transition: Option<Transition>,
    next_id: u64,
    pub notice: String,
    pub deferred_apps: VecDeque<String>,
    pub deferred_requests: VecDeque<(ClientId, makepad_wm_api::WmRequest)>,
    pub deferred_open: VecDeque<crate::preview::OpenRequest>,
    pub deferred_pane: bool,
    timer: Timer,
}

impl LinuxGpuController {
    pub fn active(&self) -> bool {
        self.transition.is_some()
    }
    pub fn app_policy(&self, client: ClientId) -> Option<[u8; 16]> {
        self.policies.get(&client).copied()
    }
}

impl App {
    pub(crate) fn linux_gpu_app_status(
        &self,
    ) -> Option<(ClientId, String, Option<[u8; 16]>, [u8; 16])> {
        let state = self.state.as_ref()?;
        let client = state.layout.focused_client()?;
        let slot = state.clients.get(&client)?;
        if slot.child.is_none() || slot.warm {
            return None;
        }
        let peer = self.linux_gpu.peers.get(&client)?;
        Some((
            client,
            slot.app.clone(),
            self.linux_gpu.app_policy(client),
            peer.device.device_uuid,
        ))
    }
    fn with_linux_gpu_view<R>(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        f: impl FnOnce(&mut Cx, &mut MpRunView) -> R,
    ) -> Option<R> {
        if self.state.as_ref()?.clients.get(&client)?.warm {
            return None;
        }
        if self.ai_bus.is_pane(client) {
            return self.with_pane_run_view(cx, f);
        }
        self.desk(cx)
            .borrow_mut::<WmDesk>()?
            .with_run_view(cx, client, f)
    }

    fn send_linux_gpu(&self, client: ClientId, message: HostToAppGpu) -> Result<(), String> {
        let sender = self
            .state
            .as_ref()
            .and_then(|state| state.clients.get(&client))
            .and_then(|slot| slot.sender.as_ref())
            .ok_or("app disconnected during GPU switch")?;
        sender
            .send(StudioToAppVec(vec![StudioToApp::Gpu(message)]).serialize_bin())
            .map_err(|_| "App GPU control channel is closed".into())
    }

    pub(crate) fn change_compositor_gpu(
        &mut self,
        cx: &mut Cx,
        target: [u8; 16],
    ) -> Result<(), String> {
        self.begin_linux_gpu_change(cx, target, None)
    }

    pub(crate) fn change_app_gpu(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        target: Option<[u8; 16]>,
    ) -> Result<(), String> {
        let compositor = cx
            .linux_gpu_snapshot()
            .renderer
            .ok_or("Vulkan compositor is unavailable")?
            .uuid;
        self.begin_linux_gpu_change(cx, target.unwrap_or(compositor), Some((client, target)))
    }

    fn begin_linux_gpu_change(
        &mut self,
        cx: &mut Cx,
        target: [u8; 16],
        policy: Option<(ClientId, Option<[u8; 16]>)>,
    ) -> Result<(), String> {
        if self.linux_gpu.active() {
            return Err("A GPU switch is already in progress".into());
        }
        let snapshot = cx.linux_gpu_snapshot();
        if !matches!(cx.os_type(), OsType::LinuxDirect) || snapshot.renderer.is_none() {
            return Err("GPU switching requires the Linux direct Vulkan compositor".into());
        }
        if !snapshot.devices.iter().any(|device| device.uuid == target) {
            return Err("GPU is no longer available".into());
        }
        if policy.is_none()
            && snapshot
                .renderer
                .as_ref()
                .is_some_and(|device| device.uuid == target)
        {
            return Ok(());
        }
        let rows: Vec<_> = self
            .state
            .as_ref()
            .ok_or("WM is not ready")?
            .clients
            .iter()
            .filter(|(id, slot)| {
                slot.child.is_some() && policy.is_none_or(|(selected, _)| selected == **id)
            })
            .map(|(&id, slot)| {
                (
                    id,
                    slot.app.clone(),
                    slot.socket,
                    slot.ready,
                    slot.warm,
                    slot.closing.is_some(),
                )
            })
            .collect();
        if rows.len() > 64 {
            return Err("Too many hosted apps for one GPU transition".into());
        }
        if policy.is_some() && rows.is_empty() {
            return Err("The selected app is no longer running".into());
        }
        let mut members = HashMap::new();
        let mut generation = snapshot.renderer_generation;
        for (id, app, socket, ready, warm, closing) in rows {
            let peer = self
                .linux_gpu
                .peers
                .get(&id)
                .ok_or_else(|| format!("Wait for {app} to finish starting"))?
                .clone();
            if peer.version != GPU_CONTROL_VERSION {
                return Err(format!("{app} needs the current GPU protocol"));
            }
            if !ready || closing {
                return Err(format!("Wait for {app} to finish starting or closing"));
            }
            let socket = socket.ok_or_else(|| format!("{app} is not connected"))?;
            generation = generation.max(peer.generation);
            let windows = if warm {
                Vec::new()
            } else {
                vec![self
                    .with_linux_gpu_view(cx, id, |_, view| view.gpu_window_id())
                    .flatten()
                    .ok_or_else(|| format!("{app} has no ready hosted window"))?]
            };
            let app_target = if policy.is_some() {
                target
            } else {
                self.linux_gpu.app_policy(id).unwrap_or(target)
            };
            members.insert(
                id,
                Member {
                    socket,
                    target: app_target,
                    windows,
                    quiesced: false,
                    prepared: false,
                    exported: false,
                    ready: HashSet::new(),
                    committed: false,
                    cancelled: false,
                    view_committed: false,
                    commit_sent: false,
                    retry_at: 0.0,
                    retired: false,
                    prepared_device: None,
                },
            );
        }
        generation = generation
            .checked_add(1)
            .ok_or("GPU generation exhausted")?;
        self.linux_gpu.next_id = self
            .linux_gpu
            .next_id
            .max(snapshot.transition)
            .checked_add(1)
            .ok_or("GPU transaction ID exhausted")?;
        let id = self.linux_gpu.next_id;
        log!(
            "[gpu] begin id={} generation={} compositor={} target={:02x?} members={}",
            id,
            generation,
            policy.is_none(),
            target,
            members.len()
        );
        let mut error = None;
        for (&client, member) in &members {
            self.with_linux_gpu_view(cx, client, |_, view| view.gpu_freeze(id));
            if let Err(message) = self.send_linux_gpu(
                client,
                HostToAppGpu::Prepare {
                    transition: id,
                    renderer_generation: generation,
                    target_device_uuid: member.target,
                    windows: member.windows.clone(),
                },
            ) {
                error = Some(message);
            }
        }
        self.linux_gpu.notice = "Preparing app renderers…".into();
        self.linux_gpu.timer = cx.start_interval(0.01);
        self.linux_gpu.transition = Some(Transition {
            id,
            generation,
            target,
            compositor: policy.is_none(),
            policy,
            members,
            departed: Vec::new(),
            phase: Phase::Quiescing,
            deadline: crate::host::now() + 30.0,
            wm_started: false,
            wm_committed: false,
            views_committed: false,
            error,
        });
        self.redraw_all(cx);
        Ok(())
    }

    pub(crate) fn on_linux_gpu_message(&mut self, client: ClientId, message: AppToHostGpu) {
        log!("[gpu] reply client={} {:?}", client, message);
        if let AppToHostGpu::Hello {
            version,
            renderer_generation,
            device,
        } = message
        {
            self.linux_gpu.peers.insert(
                client,
                Peer {
                    version,
                    generation: renderer_generation,
                    device,
                },
            );
            return;
        }
        let Some(transition) = self.linux_gpu.transition.as_mut() else {
            return;
        };
        let Some(member) = transition.members.get_mut(&client) else {
            return;
        };
        match message {
            AppToHostGpu::Quiesced { transition: id } if id == transition.id => {
                member.quiesced = true
            }
            AppToHostGpu::Prepared {
                transition: id,
                renderer_generation,
                device,
            } if id == transition.id => {
                if renderer_generation != transition.generation
                    || device.device_uuid != member.target
                {
                    transition.error = Some("App prepared the wrong GPU generation".into());
                } else {
                    member.quiesced = true;
                    member.prepared = true;
                    member.prepared_device = Some(device);
                }
            }
            AppToHostGpu::SwapchainReady {
                transition: id,
                transport_epoch,
                window_id,
            } if id == transition.id
                && transport_epoch == transition.id
                && member.windows.contains(&window_id) =>
            {
                member.ready.insert(window_id);
            }
            AppToHostGpu::Committed {
                transition: id,
                renderer_generation,
            } if id == transition.id && renderer_generation == transition.generation => {
                member.committed = true
            }
            AppToHostGpu::Cancelled { transition: id } if id == transition.id => {
                member.cancelled = true
            }
            AppToHostGpu::Retired { transition: id } if id == transition.id => {
                member.retired = true
            }
            AppToHostGpu::Failed {
                transition: id,
                message,
            } if id == transition.id => {
                transition.error = Some(message);
                member.commit_sent = false;
                member.retry_at = crate::host::now() + 1.0;
            }
            _ => {}
        }
    }

    pub(crate) fn poll_linux_gpu(&mut self, cx: &mut Cx) {
        // The direct platform has already consumed SignalToUI before calling
        // Event::Signal. Drain replies here on the controller timer as well.
        self.drain_hub(cx);
        let Some(mut transition) = self.linux_gpu.transition.take() else {
            return;
        };
        let previous_phase = transition.phase;
        let previous_error = transition.error.clone();
        let now = crate::host::now();
        let snapshot = cx.linux_gpu_snapshot();
        // Paint can complete the queued WM commit between host callbacks.
        // Observe that irreversible boundary before reacting to peer errors.
        if transition.compositor
            && snapshot.transition == transition.id
            && snapshot.phase == LinuxGpuPhase::Committed
        {
            transition.wm_committed = true;
        }
        let disconnected: Vec<_> = transition
            .members
            .iter()
            .filter(|(client, member)| {
                self.state
                    .as_ref()
                    .and_then(|state| state.clients.get(client))
                    .and_then(|slot| slot.socket)
                    != Some(member.socket)
            })
            .map(|(&client, _)| client)
            .collect();
        for client in disconnected {
            transition.members.remove(&client);
            transition.departed.push(client);
            self.linux_gpu.peers.remove(&client);
            if !transition.wm_committed {
                transition.error = Some("An app disconnected while preparing its GPU".into());
            }
        }
        if now >= transition.deadline && transition.error.is_none() {
            transition.error = Some("GPU switch timed out".into());
        }
        if now >= transition.deadline && transition.phase == Phase::Cancelling {
            transition.phase = Phase::CancelStalled;
            transition.error =
                Some("GPU cleanup has stalled; previous resources are retained".into());
        }
        let result = self.advance_linux_gpu(cx, &mut transition);
        if let Err(error) = result {
            transition.error = Some(error);
        }
        if let Some(error) = &transition.error {
            self.linux_gpu.notice = format!("GPU switch: {error}");
            if !transition.wm_committed
                && !matches!(transition.phase, Phase::Cancelling | Phase::CancelStalled)
            {
                transition.phase = Phase::Cancelling;
                transition.deadline = now + 30.0;
                for &client in transition.members.keys() {
                    let _ = self.send_linux_gpu(
                        client,
                        HostToAppGpu::Cancel {
                            transition: transition.id,
                        },
                    );
                }
                if transition.wm_started {
                    let _ = cx.linux_cancel_gpu(transition.id);
                }
            }
        }
        let complete = if matches!(transition.phase, Phase::Cancelling | Phase::CancelStalled) {
            let wm_done = !transition.wm_started
                || matches!(
                    cx.linux_gpu_snapshot().phase,
                    LinuxGpuPhase::Cancelled | LinuxGpuPhase::Failed
                );
            if wm_done && transition.members.values().all(|member| member.cancelled) {
                for &client in transition.members.keys() {
                    self.with_linux_gpu_view(cx, client, |cx, view| {
                        view.gpu_cancel(cx, transition.id)
                    });
                }
                true
            } else {
                false
            }
        } else if transition.views_committed
            && transition.members.values().all(|member| member.committed)
        {
            if transition.phase != Phase::Retiring {
                transition.phase = Phase::Retiring;
                for member in transition.members.values_mut() {
                    member.retry_at = 0.0;
                }
            }
            for (&client, member) in &mut transition.members {
                if !member.retired && now >= member.retry_at {
                    let _ = self.send_linux_gpu(
                        client,
                        HostToAppGpu::Retire {
                            transition: transition.id,
                        },
                    );
                    member.retry_at = now + 1.0;
                }
            }
            let mut retired = transition.members.values().all(|member| member.retired);
            if retired && transition.compositor && cx.linux_retire_gpu(transition.id).is_err() {
                retired = false;
            }
            if retired {
                for (&client, member) in &transition.members {
                    self.with_linux_gpu_view(cx, client, |cx, view| {
                        view.gpu_retire(cx, transition.id)
                    });
                    if let Some(peer) = self.linux_gpu.peers.get_mut(&client) {
                        peer.generation = transition.generation;
                        if let Some(device) = member.prepared_device {
                            peer.device = device;
                        }
                    }
                }
                if let Some((client, choice)) = transition.policy {
                    match choice {
                        Some(uuid) => {
                            self.linux_gpu.policies.insert(client, uuid);
                        }
                        None => {
                            self.linux_gpu.policies.remove(&client);
                        }
                    }
                }
                self.linux_gpu.notice = "GPU switch complete".into();
            }
            retired
        } else {
            false
        };
        if previous_phase != transition.phase || previous_error != transition.error {
            log!(
                "[gpu] id={} phase={:?} backend={:?} error={:?}",
                transition.id,
                transition.phase,
                cx.linux_gpu_snapshot().phase,
                transition.error
            );
        }
        if complete {
            let snapshot = cx.linux_gpu_snapshot();
            log!(
                "[gpu] finish id={} phase={:?} generation={} renderer={:02x?} copied_bytes={} error={:?}",
                transition.id,
                transition.phase,
                snapshot.renderer_generation,
                snapshot.renderer.as_ref().map(|device| device.uuid),
                snapshot.copied_bytes,
                transition.error
            );
            cx.stop_timer(self.linux_gpu.timer);
            self.finish_linux_gpu_choice(cx, transition.phase == Phase::Retiring);
            for client in transition.departed {
                // The socket no longer represents this transaction's peer.
                // Keep its transport until GPU retirement, then clear its
                // stale target rather than leaving a permanently frozen view.
                self.with_linux_gpu_view(cx, client, |cx, view| view.clear_run_target(cx));
            }
            self.redraw_all(cx);
            while let Some(app) = self.linux_gpu.deferred_apps.pop_front() {
                self.launch_app(cx, &app);
            }
            while let Some((client, request)) = self.linux_gpu.deferred_requests.pop_front() {
                self.on_wm_request(cx, client, request);
            }
            while let Some(request) = self.linux_gpu.deferred_open.pop_front() {
                self.open_request(cx, request);
            }
            if std::mem::take(&mut self.linux_gpu.deferred_pane) {
                self.open_ai_pane(cx);
            }
        } else {
            self.linux_gpu.transition = Some(transition);
        }
    }

    fn advance_linux_gpu(
        &mut self,
        cx: &mut Cx,
        transition: &mut Transition,
    ) -> Result<(), String> {
        if (transition.error.is_some() && !transition.wm_committed)
            || matches!(transition.phase, Phase::Cancelling | Phase::CancelStalled)
        {
            return Ok(());
        }
        for &client in transition.members.keys() {
            if !transition.wm_committed {
                if let Some(Some(error)) = self
                    .with_linux_gpu_view(cx, client, |_, view| view.gpu_error().map(str::to_string))
                {
                    return Err(error);
                }
            }
        }
        if transition.phase == Phase::Quiescing
            && transition.members.values().all(|member| member.quiesced)
        {
            if transition.compositor {
                cx.linux_prepare_gpu(transition.id, transition.generation, transition.target)?;
                transition.wm_started = true;
            }
            transition.phase = Phase::Preparing;
            self.linux_gpu.notice = "Preparing compositor resources…".into();
        }
        if transition.wm_started && cx.linux_gpu_snapshot().phase == LinuxGpuPhase::Failed {
            return Err(cx
                .linux_gpu_snapshot()
                .error
                .unwrap_or_else(|| "Compositor GPU preparation failed".into()));
        }
        if transition.phase == Phase::Preparing
            && transition.members.values().all(|member| member.prepared)
            && (!transition.compositor || cx.linux_gpu_snapshot().phase == LinuxGpuPhase::Prepared)
        {
            transition.phase = Phase::Importing;
            self.linux_gpu.notice = "Connecting app framebuffers…".into();
        }
        if transition.phase == Phase::Importing {
            for (&client, member) in &mut transition.members {
                if member.exported {
                    continue;
                }
                member.exported = if member.windows.is_empty() {
                    true
                } else {
                    self.with_linux_gpu_view(cx, client, |cx, view| {
                        view.gpu_prepare(cx, transition.id, transition.id)
                    })
                    .ok_or("Hosted view disappeared during GPU preparation")??
                };
            }
            if transition.members.values().all(|member| {
                member.exported
                    && member
                        .windows
                        .iter()
                        .all(|window| member.ready.contains(window))
            }) {
                if transition.compositor {
                    cx.linux_commit_gpu(transition.id)?;
                }
                transition.phase = Phase::Committing;
                self.linux_gpu.notice = "Switching app renderers…".into();
            }
        }
        if transition.phase == Phase::Committing
            && (!transition.compositor || cx.linux_gpu_snapshot().phase == LinuxGpuPhase::Committed)
        {
            for (&client, member) in &mut transition.members {
                if member.committed || crate::host::now() < member.retry_at {
                    continue;
                }
                if !member.view_committed {
                    if !member.windows.is_empty() {
                        match self
                            .with_linux_gpu_view(cx, client, |cx, view| {
                                view.gpu_commit(cx, transition.id)
                            })
                            .ok_or("Hosted view disappeared at GPU commit".to_string())
                            .and_then(|result| result)
                        {
                            Ok(()) => {}
                            Err(error) => {
                                transition.error = Some(error);
                                member.retry_at = crate::host::now() + 1.0;
                                continue;
                            }
                        }
                    }
                    member.view_committed = true;
                    transition.wm_committed = true;
                }
                if !member.commit_sent {
                    match self.send_linux_gpu(
                        client,
                        HostToAppGpu::Commit {
                            transition: transition.id,
                        },
                    ) {
                        Ok(()) => member.commit_sent = true,
                        Err(error) => {
                            transition.error = Some(error);
                            member.retry_at = crate::host::now() + 1.0;
                        }
                    }
                }
            }
            transition.views_committed = transition
                .members
                .values()
                .all(|member| member.view_committed);
        }
        Ok(())
    }
}
