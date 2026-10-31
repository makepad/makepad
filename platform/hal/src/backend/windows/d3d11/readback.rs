//! Uploads and copies. A readback copies into a staging texture on the immediate context and
//! is mapped by the completion thread once the GPU is done (never on the HAL's thread). 8-bit
//! colour comes back as RGBA; other formats keep their texel layout. Moved from
//! os/windows/d3d11.rs (`d3d_capture_texture_readbacks`, `read_texture_rgba`,
//! `update_vec_texture`'s uploads).

use super::complete::{Batch, Staged, Ticket};
use super::device::{dxgi, hr};
use super::resources::Texture;
use super::D3d11;
use crate::backend::Pixels;
use crate::session::{Session, World};
use crate::types::*;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use std::sync::Arc;

fn valid_rect(t: &Texture, mip: u32, rect: [u32; 4]) -> bool {
    if mip >= t.desc.mips.max(1) {
        return false;
    }
    let [x, y, w, h] = rect;
    let (mw, mh) = ((t.desc.w >> mip).max(1), (t.desc.h >> mip).max(1));
    w > 0 && h > 0 && x.checked_add(w).is_some_and(|v| v <= mw) && y.checked_add(h).is_some_and(|v| v <= mh)
        && (!format::is_block(t.desc.format) || (x % 4 == 0 && y % 4 == 0 && (w % 4 == 0 || x + w == mw) && (h % 4 == 0 || y + h == mh)))
}

impl D3d11 {
    pub(super) fn upload(&mut self, s: &mut Session, id: Id, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) -> Result<(), String> {
        let t = self.resolve(s, id).ok_or("missing upload texture")?;
        if !valid_rect(&t, mip as u32, rect) || layer >= t.desc.depth.max(1) {
            return Err("invalid upload region".into());
        }
        if data.len() as u64 != format::level_bytes(t.desc.format, rect[2], rect[3]) {
            return Err("upload byte count differs from region".into());
        }
        let block = format::is_block(t.desc.format);
        let pitch = if block { format::level_bytes(t.desc.format, rect[2], 4) } else { format::level_bytes(t.desc.format, rect[2], 1) } as u32;
        let bx = D3D11_BOX { left: rect[0], top: rect[1], front: 0, right: rect[0] + rect[2], bottom: rect[1] + rect[3], back: 1 };
        unsafe { self.dev.ctx.UpdateSubresource(t.res.as_ID3D11Resource(), t.sub(mip as u32, layer), Some(&bx), data.as_ptr() as _, pitch, 0) };
        Ok(())
    }

    /// A staging copy of `rect` of mip 0 / layer 0, for the completion thread to map.
    fn stage(&self, t: &Texture, rect: [u32; 4]) -> Result<Staged, String> {
        let fmt = dxgi(t.desc.format).ok_or("format without a D3D11 equivalent")?;
        if format::is_depth(t.desc.format) {
            return Err("depth readback is not supported by the D3D11 backend".into());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: rect[2],
            Height: rect[3],
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt.resource,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe { self.dev.device.CreateTexture2D(&desc, None, Some(&mut staging)) }.map_err(|e| hr("CreateTexture2D(staging)", e))?;
        let staging: ID3D11Texture2D = staging.ok_or("CreateTexture2D(staging) returned nothing")?;
        let bx = D3D11_BOX { left: rect[0], top: rect[1], front: 0, right: rect[0] + rect[2], bottom: rect[1] + rect[3], back: 1 };
        unsafe { self.dev.ctx.CopySubresourceRegion(staging.as_ID3D11Resource(), 0, 0, 0, 0, t.res.as_ID3D11Resource(), 0, Some(&bx)) };
        Ok(Staged { tex: staging, format: t.desc.format, w: rect[2], h: rect[3] })
    }

    pub(super) fn read(&mut self, s: &mut Session, tex: Option<Arc<Texture>>, req: u32, rect: [u32; 4], pick: Option<[f32; 2]>) {
        let result = (|| {
            let t = tex.ok_or("readback texture has no storage")?;
            if rect[0] >= t.desc.w || rect[1] >= t.desc.h {
                return Err("readback origin exceeds texture".to_string());
            }
            let rect = [rect[0], rect[1], rect[2].min(t.desc.w - rect[0]), rect[3].min(t.desc.h - rect[1])];
            if !valid_rect(&t, 0, rect) {
                return Err("invalid readback region".into());
            }
            let staged = self.stage(&t, rect)?;
            let mut batch = Batch::default();
            batch.tickets.push(Ticket::Read { token: Self::token(s), req, rect, staged, pick });
            self.complete.submit(batch);
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[hal-d3d11] readback: {e}");
            if pick.is_some() {
                s.events.pick_result(req, &[]);
            } else {
                s.events.readback(req, readback::FAILED, rect, &[]);
            }
        }
    }

    /// The ids drawn around `pos` in the window's newest pick frame: the 8-texel neighbourhood
    /// (as Metal and Vulkan read it), nearest first.
    pub(super) fn read_pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) {
        let Some(id) = self.pick_ids.get(&(s.id, window)).copied() else { s.events.pick_result(req, &[]); return };
        if self.pick_pending.contains(&(s.id, id)) || pos.iter().any(|p| !p.is_finite()) {
            s.events.pick_result(req, &[]);
            return;
        }
        let tex = self.resolve(s, id);
        let Some(t) = &tex else { s.events.pick_result(req, &[]); return };
        let pos = pos.map(f32::floor);
        let x = (pos[0] as i64).saturating_sub(8).clamp(0, t.desc.w as i64) as u32;
        let y = (pos[1] as i64).saturating_sub(8).clamp(0, t.desc.h as i64) as u32;
        let x1 = (pos[0] as i64).saturating_add(9).clamp(x as i64, t.desc.w as i64) as u32;
        let y1 = (pos[1] as i64).saturating_add(9).clamp(y as i64, t.desc.h as i64) as u32;
        if x1 == x || y1 == y {
            s.events.pick_result(req, &[]);
            return;
        }
        self.read(s, tex, req, [x, y, x1 - x, y1 - y], Some(pos));
    }

    pub(super) fn copy_window(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) {
        let result = (|| {
            let source = self.window_image(s, window).ok_or("window has no retained frame")?;
            let dest = self.resolve(s, texture).ok_or("copy destination has no storage")?;
            if source.desc.w != dest.desc.w || source.desc.h != dest.desc.h || source.desc.format != dest.desc.format || Arc::ptr_eq(&source, &dest) {
                return Err("window copy dimensions/format differ".to_string());
            }
            unsafe { self.dev.ctx.CopySubresourceRegion(dest.res.as_ID3D11Resource(), 0, 0, 0, 0, source.res.as_ID3D11Resource(), 0, None) };
            let mut batch = Batch::default();
            batch.tickets.push(Ticket::Copy { token: Self::token(s), req, rect: [0, 0, source.desc.w, source.desc.h] });
            self.complete.submit(batch);
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[hal-d3d11] window copy: {e}");
            s.events.readback(req, readback::FAILED, [0; 4], &[]);
        }
    }

    /// The host's synchronous read of a window's newest frame (automation). Never called on the
    /// window system's thread.
    pub(super) fn host_window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        let s = world.session_mut(sid)?;
        let id = window.or_else(|| s.windows.keys_of(sid).into_iter().next())?;
        let t = self.window_image(s, id)?;
        if !matches!(t.desc.format, format::BGRA8 | format::RGBA8) {
            return None;
        }
        let staged = self.stage(&t, [0, 0, t.desc.w, t.desc.h]).ok()?;
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        let mut batch = Batch::default();
        batch.tickets.push(Ticket::Host { reply, staged });
        self.complete.submit(batch);
        rx.recv().ok().flatten()
    }
}
