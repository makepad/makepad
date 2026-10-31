//! WM-native layers: one stable lent alias, imports cached by wl_buffer allocation ID.
use super::*;
use crate::backend::linux::wm::native as transport;
use crate::cmds::HalCmd;
use crate::native_layer::{kind, mode, op, state};
use makepad_studio_protocol::{NativeAppEvent, NativeAppId, NativeAppRequest};
use std::collections::HashMap;

struct Layer {
    wire: NativeAppId,
    alias: Id,
    buffers: HashMap<u64, Arc<Image>>,
    latest: Option<(u64, u64)>,
    current: Option<(u64, u64)>,
    realized: bool,
    presented: u64,
}
struct Release { id: NativeAppId, buffer: u64, completion: Option<crate::completion::Completion> }
#[derive(Default)]
pub(super) struct NativeApps {
    layers: HashMap<(u32, Id), Layer>,
    releases: Vec<Release>,
    frames: HashMap<(u32, Id, u64), Vec<(NativeAppId, u64)>>,
    samples: Vec<(NativeAppId, u64)>,
    pass_samples: Vec<(NativeAppId, u64)>,
    cached: HashMap<(u32, Id), Vec<(NativeAppId, u64)>>,
    aliases: HashMap<(u32, Id), NativeAppId>,
}
fn wire(sid: u32, id: Id) -> NativeAppId { NativeAppId { session: sid, index: id.index, generation: id.gen } }
fn key(id: NativeAppId) -> (u32, Id) { (id.session, Id::new(id.index, id.generation)) }
fn sample_into(samples: &mut Vec<(NativeAppId, u64)>, sample: (NativeAppId, u64)) {
    if let Some(old) = samples.iter_mut().find(|s| s.0 == sample.0) { old.1 = old.1.max(sample.1); }
    else { samples.push(sample); }
}

impl NativeApps {
    pub fn command(&mut self, gpu: &mut Vulkan, world: &mut World, sid: u32, command: &HalCmd) -> bool {
        if !gpu.shell.is_some_and(|s| s.window_manager()) && !transport::bridge_enabled() { return false; }
        match command {
            HalCmd::NativeLayerCreate { id, kind: kind::FOREIGN_WAYLAND, params } => {
                if self.layers.contains_key(&(sid, *id)) { return true; }
                let Ok(app) = std::str::from_utf8(params) else { return false };
                let native = wire(sid, *id);
                let alias = world.alloc_lent();
                self.layers.insert((sid, *id), Layer { wire: native, alias, buffers: HashMap::new(), latest: None, current: None, realized: false, presented: 0 });
                self.aliases.insert((sid, alias), native);
                transport::active(true);
                transport::request(NativeAppRequest::Create { id: native, app: app.to_owned() }, None);
                true
            }
            HalCmd::NativeLayerOp { id, op: operation, arg, data } if self.layers.contains_key(&(sid, *id)) => {
                let native = wire(sid, *id);
                if *operation == op::SIZE {
                    if let Ok(bytes) = <[u8; 8]>::try_from(*data) {
                        let width = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                        let height = u32::from_le_bytes(bytes[4..].try_into().unwrap());
                        if width > 0 && height > 0 { transport::request(NativeAppRequest::Configure { id: native, width, height }, None); }
                    }
                } else {
                    transport::request(NativeAppRequest::Op { id: native, op: *operation, arg: *arg, data: data.to_vec() }, None);
                }
                true
            }
            HalCmd::NativeLayerDestroy { id } if self.layers.contains_key(&(sid, *id)) => {
                self.remove(gpu, world, (sid, *id));
                true
            }
            _ => false,
        }
    }

    fn retire(&mut self, id: NativeAppId, buffer: u64) {
        if !self.releases.iter().any(|r| r.id == id && r.buffer == buffer) {
            self.releases.push(Release { id, buffer, completion: None });
        }
    }

    fn remove(&mut self, gpu: &mut Vulkan, world: &mut World, key: (u32, Id)) {
        if let Some(layer) = self.layers.remove(&key) {
            transport::active(!self.layers.is_empty());
            self.aliases.remove(&(key.0, layer.alias));
            for (buffer, _) in layer.current.into_iter().chain(layer.latest) { self.retire(layer.wire, buffer); }
            gpu.tex.remove(&(key.0, layer.alias.index));
            if let Some(s) = world.session_mut(key.0) { let _ = s.textures.remove(key.0, layer.alias); }
            transport::request(NativeAppRequest::Destroy { id: layer.wire }, None);
            world.to_session(key.0, |q| q.native_layer_state(key.1, state::CLOSED, 0, 0, &[]));
        }
    }

    pub fn close(&mut self, gpu: &mut Vulkan, world: &mut World, sid: u32) {
        let keys: Vec<_> = self.layers.keys().copied().filter(|k| k.0 == sid).collect();
        for key in keys { self.remove(gpu, world, key); }
        self.frames.retain(|k, _| k.0 != sid);
        self.cached.retain(|k, _| k.0 != sid);
        self.poll_releases(gpu);
    }

    pub fn drain(&mut self, gpu: &mut Vulkan, world: &mut World) {
        for (sid, window, frame) in transport::take_presented() { self.presented(sid, window, frame); }
        for incoming in transport::take() {
            let id = transport::id(&incoming.event);
            let key = key(id);
            let Some(layer) = self.layers.get_mut(&key) else { continue };
            match incoming.event {
                NativeAppEvent::Buffer { buffer, desc, .. } => {
                    let imported = (|| {
                        let [plane] = <[_; 1]>::try_from(desc.planes).map_err(|_| "unadvertised multi-plane native buffer")?;
                        let [fd] = <[_; 1]>::try_from(incoming.fds).map_err(|_| "missing native buffer descriptor")?;
                        let native = crate::dmabuf::DmaBuf { allocation_size: crate::dmabuf::DmaBuf::size(&fd)?, fd,
                            width: desc.width, height: desc.height, fourcc: desc.fourcc, modifier: desc.modifier,
                            offset: plane.offset as u64, row_pitch: plane.stride as u64, sample_only: true, opaque: None, timeline_fd: None };
                        share::import_foreign(gpu.dev.clone(), Arc::new(native))
                    })();
                    match imported {
                        Ok(image) => {
                            if transport::trace_enabled() { eprintln!("native-app import id={id:?} buffer={buffer} {}x{}", image.desc.w, image.desc.h); }
                            layer.buffers.insert(buffer, image);
                        }
                        Err(error) => {
                            eprintln!("[hal-wm] native import: {error}");
                            world.to_session(key.0, |q| q.native_layer_state(key.1, state::CRASHED, 0, 0, error.as_bytes()));
                        }
                    }
                }
                NativeAppEvent::Commit { buffer, commit, .. } => {
                    if !layer.buffers.contains_key(&buffer) { continue; }
                    let previous = layer.latest.replace((buffer, commit));
                    let old = previous.filter(|(old, _)| *old != buffer && layer.current.is_none_or(|c| c.0 != *old));
                    if !layer.realized {
                        layer.realized = true;
                        let alias = layer.alias.index as u64 | ((layer.alias.gen as u64) << 32);
                        world.to_session(key.0, |q| q.native_layer_state(key.1, state::REALIZED, mode::TEXTURED, alias, &[]));
                    } else {
                        world.to_session(key.0, |q| q.native_layer_state(key.1, state::FRAME, 0, 0, &[]));
                    }
                    if let Some((old, _)) = old { self.retire(id, old); }
                }
                NativeAppEvent::Forget { buffer, .. } => { layer.buffers.remove(&buffer); }
                NativeAppEvent::Unmapped { .. } => {
                    let abandoned = layer.latest.take().filter(|v| layer.current.is_none_or(|c| c.0 != v.0));
                    world.to_session(key.0, |q| q.native_layer_state(key.1, state::FRAME, 0, 0, &[]));
                    if let Some((buffer, _)) = abandoned { self.retire(id, buffer); }
                }
                NativeAppEvent::State { what, a, data, .. } => {
                    world.to_session(key.0, |q| q.native_layer_state(key.1, what, a, 0, &data));
                    if what == state::CLOSED { self.remove(gpu, world, key); }
                }
            }
        }
        self.poll_releases(gpu);
    }

    pub fn begin(&mut self, gpu: &mut Vulkan, s: &mut Session) {
        let stamp = (!self.layers.is_empty() && transport::trace_enabled()).then(std::time::Instant::now);
        self.samples.clear();
        let mut retired = Vec::new();
        for ((sid, _), layer) in &mut self.layers {
            if *sid != s.id { continue; }
            let Some(latest) = layer.latest else {
                if let Some((buffer, _)) = layer.current.take() {
                    gpu.tex.remove(&(s.id, layer.alias.index));
                    retired.push((layer.wire, buffer));
                }
                continue;
            };
            if layer.current == Some(latest) { continue; }
            let Some(image) = layer.buffers.get(&latest.0) else { continue };
            let mut object = image.desc.clone();
            object.imported = true; object.bytes = 0;
            if let Ok(entry) = s.textures.get_mut(s.id, layer.alias) { entry.val = object; }
            else if s.textures.insert(s.id, layer.alias, s.id, object).is_err() { continue; }
            gpu.tex.insert((s.id, layer.alias.index), Texture { gen: layer.alias.gen, image: image.clone() });
            if let Some(old) = layer.current.replace(latest).filter(|old| old.0 != latest.0) { retired.push((layer.wire, old.0)); }
        }
        for (id, buffer) in retired { self.retire(id, buffer); }
        // This fence covers all frames using the replaced image, before the new frame is built.
        self.poll_releases(gpu);
        if let Some(stamp) = stamp { eprintln!("native-app bind us={} layers={}", stamp.elapsed().as_micros(), self.layers.len()); }
    }

    pub fn sample(&mut self, sid: u32, alias: Id) {
        if self.layers.is_empty() { return; }
        if let Some(layer) = self.aliases.get(&(sid, alias)).and_then(|id| self.layers.get(&key(*id))) {
            if let Some((_, commit)) = layer.current {
                if commit > layer.presented { sample_into(&mut self.pass_samples, (layer.wire, commit)); }
            }
        } else if let Some(samples) = self.cached.get(&(sid, alias)) {
            for sample in samples { sample_into(&mut self.pass_samples, *sample); }
        }
    }
    pub fn begin_pass(&mut self) { self.pass_samples.clear(); }
    pub fn end_pass(&mut self, sid: u32, pass: &Pass, success: bool, pick: bool, window: bool) {
        if self.layers.is_empty() { return; }
        if !success || pick { self.pass_samples.clear(); return; }
        let samples = std::mem::take(&mut self.pass_samples);
        if window {
            for sample in samples { sample_into(&mut self.samples, sample); }
        } else {
            for (i, color) in pass.color.iter().enumerate().filter(|(_, c)| !c.is_none()) {
                let cached = self.cached.entry((sid, *color)).or_default();
                if pass.load[i] != pass_load::LOAD { cached.clear(); }
                for sample in &samples { sample_into(cached, *sample); }
                if cached.is_empty() { self.cached.remove(&(sid, *color)); }
            }
        }
    }
    pub fn forget_texture(&mut self, sid: u32, texture: Id) { self.cached.remove(&(sid, texture)); }
    pub fn end(&mut self, sid: u32, target: Id, frame: u64, submitted: bool) {
        let samples = std::mem::take(&mut self.samples);
        if submitted && !samples.is_empty() { self.frames.insert((sid, target, frame), samples); }
    }
    pub fn presented(&mut self, sid: u32, target: Id, frame: u64) {
        if let Some(samples) = self.frames.remove(&(sid, target, frame)) {
            for (id, commit) in samples {
                if let Some(layer) = self.layers.get_mut(&key(id)).filter(|l| commit > l.presented) {
                    layer.presented = commit;
                    if transport::trace_enabled() { eprintln!("native-app presented id={id:?} commit={commit} frame={frame}"); }
                    transport::request(NativeAppRequest::Presented { id, commit }, None);
                }
            }
        }
        self.frames.retain(|k, _| k.0 != sid || k.1 != target || k.2 > frame);
    }
    fn poll_releases(&mut self, gpu: &mut Vulkan) {
        self.releases.retain_mut(|release| {
            if release.completion.is_none() {
                match gpu.frame_completion(release.id.session, Arc::new(|| {})) {
                    Ok(completion) => release.completion = Some(completion),
                    Err(error) => { eprintln!("[hal-wm] native release deferred: {error}"); return true; }
                }
            }
            match release.completion.as_mut().and_then(|c| c.poll()) {
                Some(Ok(fence)) => {
                    if transport::trace_enabled() { eprintln!("native-app release id={:?} buffer={} fence={}", release.id, release.buffer, fence.is_some()); }
                    transport::request(NativeAppRequest::Release { id: release.id, buffer: release.buffer, fence: fence.is_some() }, fence);
                    false
                }
                Some(Err(error)) => { eprintln!("[hal-wm] native release deferred: {error}"); release.completion = None; true }
                None => true,
            }
        });
    }
}
