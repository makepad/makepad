use super::{D3d11, device::hr, resources::Buffer, complete::{Batch, Ticket}};
use crate::{backend::exec::PipeStatus, buffers::{CmdView, PipelineBlob}, compute::{self, KernelLayout}, session::{Session, World}, types::*};
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use std::sync::{Arc, OnceLock};

pub(super) struct Program { layout: KernelLayout, shader: ID3D11ComputeShader }
pub(super) struct Pipe { pub gen: u32, state: Arc<OnceLock<Result<Program, String>>>, pub announced: bool }

impl D3d11 {
    pub(super) fn compute_ready(&self, sid: u32, passes: &[Pass], view: &CmdView) -> bool {
        for pass in passes.iter().filter(|p| p.kind == pass_kind::COMPUTE) {
            let Some(dispatches) = compute::records::<Dispatch>(view.arena, pass.dispatches) else { return false; };
            for d in dispatches {
                let Some(p) = self.compute.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { return false; };
                if !matches!(p.state.get(), Some(Ok(_))) { return false; }
            }
        }
        true
    }

    pub(super) fn create_compute(&mut self, s: &mut Session, id: Id, blob: &PipelineBlob) -> PipeStatus {
        let layout = match KernelLayout::parse(blob) { Ok(l) => l, Err(e) => return PipeStatus::Failed(e) };
        if layout.buffers.iter().any(|b| b.index >= if b.writable != 0 { 8 } else { 128 }) || layout.textures.iter().any(|t| t.index >= 8) {
            return PipeStatus::Failed("D3D11.0 compute permits eight UAV slots".into());
        }
        if let Err(e) = layout.validate_device(&self.info) { return PipeStatus::Failed(e); }
        let source = match std::str::from_utf8(blob.code) { Ok(s) => s.to_owned(), Err(e) => return PipeStatus::Failed(e.to_string()) };
        let state = Arc::new(OnceLock::new());
        self.compute.insert((s.id, id.index), Pipe { gen: id.gen, state: state.clone(), announced: false });
        let dev = self.dev.clone(); let wake = self.mailbox.wake.clone();
        self.next_compiler = (self.next_compiler + 1) % self.compilers.len();
        self.compilers[self.next_compiler].push(move || {
            let result = (|| {
                let bytes = super::pipeline::compile_stage("cs_5_0", "kernel_main", &source, layout.desc.precise_math != 0)?;
                let mut shader = None;
                unsafe { dev.device.CreateComputeShader(&bytes, None, Some(&mut shader)) }.map_err(|e| hr("CreateComputeShader", e))?;
                Ok(Program { layout, shader: shader.ok_or("no compute shader")? })
            })();
            let _ = state.set(result); wake();
        });
        PipeStatus::Pending
    }
    pub(super) fn announce_compute(&mut self, world: &mut World) {
        for ((sid, index), pipe) in &mut self.compute {
            if pipe.announced { continue; }
            let Some(result) = pipe.state.get() else { continue; };
            let Some(s) = world.session_mut(*sid) else { continue; };
            pipe.announced = true; s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
            let id = Id::new(*index, pipe.gen);
            match result { Ok(_) => s.events.pipeline_ready(id), Err(e) => s.events.pipeline_failed(id, e.as_bytes()) }
        }
    }
    pub(super) fn unbind_compute(&self) { unsafe {
        let empty: [Option<ID3D11UnorderedAccessView>; 8] = Default::default();
        self.dev.ctx.CSSetUnorderedAccessViews(0, 8, Some(empty.as_ptr()), None);
        self.dev.ctx.CSSetShaderResources(0, Some(&[const { None }; 128]));
    } }
    pub(super) fn encode_compute(&mut self, s: &mut Session, pass: &Pass, view: &CmdView, mut timer: Option<&mut super::complete::Timer>) -> Result<(), String> {
        let dispatches = compute::records::<Dispatch>(view.arena, pass.dispatches).ok_or("invalid dispatch records")?;
        // Explicitly remove previous read and output bindings before binding UAVs. D3D11
        // silently nulls conflicting views, so implicit conflict handling is never used.
        unsafe {
            self.dev.ctx.VSSetShaderResources(0, Some(&[const { None }; 128]));
            self.dev.ctx.PSSetShaderResources(0, Some(&[const { None }; 128]));
            self.dev.ctx.OMSetRenderTargets(None, None);
        }
        let result = (|| {
            for d in dispatches {
                self.unbind_compute();
                let pipe = self.compute.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).ok_or("stale compute pipeline")?;
                let p = pipe.state.get().ok_or("compute pipeline pending")?.as_ref().map_err(|e| e.clone())?;
                p.layout.validate_dispatch(s, &d, view.arena)?;
                let ids = compute::ids(view.arena, d.buffers).ok_or("invalid buffers")?;
                let textures = compute::ids(view.arena, d.textures).ok_or("invalid textures")?;
                let params = compute::bytes(view.arena, d.params).ok_or("invalid params")?;
                if ids.len() != p.layout.buffers.len() || textures.len() != p.layout.textures.len() || params.len() != p.layout.desc.push_size as usize { return Err("compute resource count mismatch".into()); }
                let mut uavs: [Option<ID3D11UnorderedAccessView>; 8] = Default::default();
                for (id, binding) in ids.iter().zip(&p.layout.buffers) {
                    let buffer = &self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).ok_or("stale compute buffer")?.buf;
                    if binding.writable != 0 { uavs[binding.index as usize] = Some(buffer.uav.clone().ok_or("buffer lacks UAV")?); }
                    else { unsafe { self.dev.ctx.CSSetShaderResources(binding.index, Some(&[buffer.words.clone()])); } }
                }
                for (id, binding) in textures.iter().zip(&p.layout.textures) {
                    let texture = &self.tex.get(&(s.id, id.index)).filter(|t| t.gen == id.gen).ok_or("stale storage texture")?.tex;
                    if texture.desc.format != binding.format { return Err("storage image format mismatch".into()); }
                    uavs[binding.index as usize] = Some(texture.uav.clone().ok_or("image lacks UAV")?);
                }
                let indirect = if d.indirect_buffer.is_none() { None } else {
                    let buffer = self.buf.get(&(s.id, d.indirect_buffer.index)).filter(|b| b.gen == d.indirect_buffer.gen).ok_or("stale indirect buffer")?.buf.clone();
                    if d.indirect_offset % 4 != 0 || d.indirect_offset.checked_add(12).is_none_or(|end| end > buffer.size) { return Err("invalid indirect offset".into()); }
                    Some(buffer)
                };
                if p.layout.desc.param_binding_size != 0 {
                    let mut bytes = vec![0; p.layout.desc.param_binding_size as usize];
                    bytes[..params.len()].copy_from_slice(params);
                    if p.layout.desc.group_count_offset != u32::MAX && indirect.is_none() {
                        let at = p.layout.desc.group_count_offset as usize;
                        for (i, v) in d.groups.iter().enumerate() { bytes[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes()); }
                    }
                    let constant = Buffer::new(self.dev.clone(), buffer_kind::UNIFORM, bytes.len() as u64)?;
                    constant.write(0, &bytes);
                    if let Some(indirect) = indirect.as_ref().filter(|_| p.layout.desc.group_count_offset != u32::MAX) {
                        // D3D11.0 forbids partial copies into constant buffers. Assemble the
                        // host params and GPU-written group counts in a same-sized default
                        // buffer, then copy the whole resource into b0. No CPU wait/readback.
                        let assembled = Buffer::new(self.dev.clone(), buffer_kind::STORAGE, bytes.len() as u64)?;
                        assembled.write(0, &bytes);
                        let region = D3D11_BOX { left: d.indirect_offset as u32, right: d.indirect_offset as u32 + 12, top: 0, bottom: 1, front: 0, back: 1 };
                        unsafe {
                            self.dev.ctx.CopySubresourceRegion(assembled.buf.as_ID3D11Resource(), 0, p.layout.desc.group_count_offset, 0, 0, indirect.buf.as_ID3D11Resource(), 0, Some(&region));
                            self.dev.ctx.CopyResource(constant.buf.as_ID3D11Resource(), assembled.buf.as_ID3D11Resource());
                        }
                    }
                    unsafe { self.dev.ctx.CSSetConstantBuffers(0, Some(&[Some(constant.buf.clone())])); }
                }
                let stamps = if d.timer != 0 { timer.as_mut().and_then(|t| t.stamp_pair(&self.dev)) } else { None };
                if let (Some(q), Some(t)) = (stamps, timer.as_ref()) {
                    unsafe { self.dev.ctx.End(t.stamp(q).as_ID3D11Asynchronous()); }
                }
                unsafe {
                    self.dev.ctx.CSSetShader(Some(&p.shader), None);
                    self.dev.ctx.CSSetUnorderedAccessViews(0, 8, Some(uavs.as_ptr()), None);
                    if let Some(indirect) = indirect { self.dev.ctx.DispatchIndirect(&indirect.buf, d.indirect_offset as u32); }
                    else { self.dev.ctx.Dispatch(d.groups[0], d.groups[1], d.groups[2]); }
                }
                // The immediate context orders dispatches and copies. Unbinding all UAVs
                // here makes writes visible to later SRV/indirect/copy operations.
                self.unbind_compute();
                if let (Some(q), Some(t)) = (stamps, timer.as_mut()) {
                    unsafe { self.dev.ctx.End(t.stamp(q + 1).as_ID3D11Asynchronous()); }
                    t.dispatches.push((s.string(d.timer).unwrap_or("compute").to_string(), q));
                }
            }
            Ok(())
        })();
        self.unbind_compute();
        result
    }
    pub(super) fn read_compute_buffer(&mut self, s: &mut Session, id: Id, req: u32, offset: u64, len: u64) {
        self.unbind_compute();
        let result = (|| {
            let source = self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).ok_or("stale buffer")?.buf.clone();
            let desc = D3D11_BUFFER_DESC { ByteWidth: len.max(4) as u32, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() };
            let mut staging = None;
            unsafe { self.dev.device.CreateBuffer(&desc, None, Some(&mut staging)) }.map_err(|e| hr("buffer readback staging", e))?;
            let staging = staging.ok_or("no staging buffer")?;
            let region = D3D11_BOX { left: offset as u32, right: (offset + len) as u32, top: 0, bottom: 1, front: 0, back: 1 };
            if len != 0 { unsafe { self.dev.ctx.CopySubresourceRegion(staging.as_ID3D11Resource(), 0, 0, 0, 0, source.buf.as_ID3D11Resource(), 0, Some(&region)); } }
            let mut batch = Batch::default();
            batch.keep.push(Box::new(source));
            batch.tickets.push(Ticket::Buffer { token: Self::token(s), req, staging, len: len as usize });
            self.complete.submit(batch);
            Ok::<_, String>(())
        })();
        if let Err(e) = result { eprintln!("[hal-d3d11] buffer readback: {e}"); s.events.readback(req, readback::FAILED, [0; 4], &[]); }
    }
}
