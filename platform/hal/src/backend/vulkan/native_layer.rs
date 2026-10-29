//! Android's bounded segment rings. A busy ring retains the latest whole packet for retry.
use super::{frame::slice, share::{sync, NativeHandle}, Device};
use crate::{backend::android::native_layer::{self, Frame, Segment, Slot, Views}, buffers::CmdView, session::{KeptFrame, Session, TextureObj}, types::*};
use std::{collections::HashMap, sync::Arc};

#[derive(Default)]
struct Window { rings: [Vec<Arc<Slot>>; 5] }

#[derive(Default)]
pub(super) struct Layers {
    pub views: Views,
    windows: HashMap<(u32, Id), Window>,
    pub pending: HashMap<(u32, Id), (u32, KeptFrame)>,
    pub frame: Option<Frame>,
    logged_busy: bool,
}

pub(super) enum Plan { Plain, Ready(Frame), Busy }

impl Layers {
    pub fn prepare(&mut self, dev: &Arc<Device>, s: &mut Session, target: Id, packet: u32, view: &CmdView) -> Result<Plan, String> {
        let packet = view.frames.get(packet as usize).ok_or("missing native frame")?;
        let layers = &view.layers[slice(packet.layers, view.layers.len()).ok_or("invalid native layer range")?];
        let geometry = self.views.geometry(s, layers, view);
        if !geometry.is_empty() && !native_layer::is_activity_window(s, target) {
            for (layer, _) in geometry {
                s.events.native_layer_state(layer.native, crate::native_layer::state::REALIZED, crate::native_layer::mode::UNSUPPORTED, 0,
                    b"Android native views require the Activity window");
            }
            return Ok(Plan::Plain);
        }
        if !geometry.is_empty() {
            static FIRST: std::sync::Once = std::sync::Once::new();
            FIRST.call_once(|| eprintln!("[hal-android-layer] preparing {} native layers", geometry.len()));
        }
        let key = (s.id, target);
        if geometry.is_empty() {
            return if self.windows.remove(&key).is_some() { Ok(Plan::Ready(Frame::new(s.id, Vec::new(), false))) } else { Ok(Plan::Plain) };
        }
        let composited = native_layer::compositing() && sync::sync_files(dev);
        if composited && native_layer::api_level() < 34 {
            static UNSYNCED: std::sync::Once = std::sync::Once::new();
            UNSYNCED.call_once(|| eprintln!("[hal-android-layer] API 29-33: SurfaceControl geometry may lag WebView layout by one frame"));
        }
        let mut frame = Frame::new(s.id, geometry, composited);
        let window = self.windows.entry(key).or_default();
        if !composited {
            static LOG: std::sync::Once = std::sync::Once::new();
            LOG.call_once(|| eprintln!("[hal-android-layer] API {}: native web views are topmost; draw-synchronised segments unavailable", native_layer::api_level()));
            return Ok(Plan::Ready(frame));
        }
        let passes = &view.passes[slice(packet.passes, view.passes.len()).ok_or("invalid native pass range")?];
        let pass = passes.iter().find(|p| p.color.iter().all(Id::is_none) && layers.iter().filter(|l| l.kind == Layer::DRAWS).all(|l|
            l.draws.off >= p.draws.off && l.draws.off + l.draws.len <= p.draws.off + p.draws.len)).ok_or("native layers require a window main pass")?;
        let size = [(pass.viewport[0] + pass.viewport[2]).ceil() as i32, (pass.viewport[1] + pass.viewport[3]).ceil() as i32];
        let mut groups = vec![Range { off: pass.draws.off, len: 0 }];
        for layer in layers {
            if layer.kind == Layer::NATIVE && groups.len() <= frame.layers.len() && frame.layers.iter().any(|(g, _)| g.native == layer.native) {
                let end = groups.last().unwrap().off + groups.last().unwrap().len;
                groups.push(Range { off: end, len: 0 });
            } else if layer.kind == Layer::DRAWS {
                let last = groups.last_mut().unwrap();
                last.len = layer.draws.off + layer.draws.len - last.off;
            }
        }
        for (index, draws) in groups.into_iter().enumerate() {
            let bounds = if index == 0 { Some([0, 0, size[0], size[1]]) } else {
                let draws = &view.draws[slice(draws, view.draws.len()).ok_or("invalid segment draw range")?];
                bounds(draws, size)
            };
            let Some(rect) = bounds.filter(|r| r[2] > 0 && r[3] > 0) else { continue; };
            let ring = &mut window.rings[index];
            let available = ring.iter().position(Slot::available);
            let slot = match available {
                Some(at) if ring[at].image.desc.w == rect[2] as u32 && ring[at].image.desc.h == rect[3] as u32 => ring[at].clone(),
                at if at.is_some() || ring.len() < 3 => {
                    let image = NativeHandle::compositor(dev.clone(), TextureObj { format: format::RGBA8, w: rect[2] as u32, h: rect[3] as u32,
                        depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED | usage::SHARED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
                    let slot = Slot::new(image);
                    if let Some(at) = at { ring[at] = slot.clone(); } else { ring.push(slot.clone()); }
                    slot
                }
                _ => {
                    if !self.logged_busy { self.logged_busy = true; eprintln!("[hal-android-layer] all three segment buffers held; retaining frame and retrying"); }
                    return Ok(Plan::Busy);
                }
            };
            frame.segments.push(Segment { index, draws, rect, slot });
        }
        Ok(Plan::Ready(frame))
    }

    pub fn close(&mut self, sid: u32) {
        if self.windows.keys().any(|(s, _)| *s == sid) { self.views.hide(sid); }
        self.views.close(sid);
        self.windows.retain(|(s, _), _| *s != sid);
        self.pending.retain(|(s, _), _| *s != sid);
    }
    pub fn close_window(&mut self, sid: u32, window: Id) {
        if self.windows.remove(&(sid, window)).is_some() { self.views.hide(sid); }
        self.pending.remove(&(sid, window));
    }
    pub fn reset(&mut self) {
        let sessions: std::collections::HashSet<_> = self.windows.keys().map(|(s, _)| *s).collect();
        for sid in sessions { self.views.hide(sid); }
        self.windows.clear(); self.pending.clear(); self.frame = None;
    }
}

fn bounds(draws: &[Draw], size: [i32; 2]) -> Option<[i32; 4]> {
    let mut extent: Option<[f32; 4]> = None;
    for draw in draws.iter().filter(|d| d.index[1] != 0 && d.inst.count != 0) {
        let [x, y, w, h] = draw.bounds;
        if draw.bounds.iter().any(|v| !v.is_finite()) { return Some([0, 0, size[0], size[1]]); }
        if w <= 0.0 || h <= 0.0 { continue; }
        extent = Some(match extent {
            None => [x, y, x+w, y+h],
            Some(e) => [e[0].min(x), e[1].min(y), e[2].max(x+w), e[3].max(y+h)],
        });
    }
    extent.map(|e| {
        let x = e[0].floor().max(0.0).min(size[0] as f32) as i32;
        let y = e[1].floor().max(0.0).min(size[1] as f32) as i32;
        let right = e[2].ceil().max(x as f32).min(size[0] as f32) as i32;
        let bottom = e[3].ceil().max(y as f32).min(size[1] as f32) as i32;
        [x, y, right-x, bottom-y]
    })
}
