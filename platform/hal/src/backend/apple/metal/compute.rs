use super::{device::Done, Metal};
use crate::{backend::exec::PipeStatus, buffers::{CmdView, PipelineBlob}, compute::{self, KernelLayout}, session::Session, types::*};
use makepad_apple_sys::*;
use std::sync::{Arc, Mutex, OnceLock};

pub(super) struct State(pub OnceLock<Result<RcObjcId, String>>);
// Metal pipeline objects are immutable and usable from any thread.
unsafe impl Send for State {}
unsafe impl Sync for State {}
pub(super) struct Pipe {
    pub gen: u32,
    pub layout: KernelLayout,
    pub state: Arc<State>,
    pub announced: bool,
}

impl Metal {
    pub(super) fn compute_ready(&self, sid: u32, passes: &[Pass], view: &CmdView) -> bool {
        for pass in passes.iter().filter(|p| p.kind == pass_kind::COMPUTE) {
            let Some(dispatches) = compute::records::<Dispatch>(view.arena, pass.dispatches) else { return false; };
            for d in dispatches {
                let Some(p) = self.compute.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { return false; };
                if !matches!(p.state.0.get(), Some(Ok(_))) { return false; }
            }
        }
        true
    }

    pub(super) fn create_compute(&mut self, s: &mut Session, id: Id, blob: &PipelineBlob) -> PipeStatus {
        let layout = match KernelLayout::parse(blob) { Ok(l) => l, Err(e) => return PipeStatus::Failed(e) };
        if let Err(e) = layout.validate_device(&self.info) { return PipeStatus::Failed(e); }
        let source = match std::str::from_utf8(blob.code) { Ok(s) => s.to_owned(), Err(e) => return PipeStatus::Failed(e.to_string()) };
        let precise = layout.desc.precise_math != 0;
        let invocations = layout.desc.workgroup.iter().map(|n| *n as u64).product::<u64>();
        let state = Arc::new(State(OnceLock::new()));
        self.compute.insert((s.id, id.index), Pipe { gen: id.gen, layout, state: state.clone(), announced: false });
        let device = RcObjcId::from_unowned(NonNull::new(self.dev.device).unwrap());
        let mailbox = self.dev.mailbox.clone();
        let sid = s.id;
        let incarnation = s.font_frames.clone();
        // the answer, from the completion handler that ends the compile
        let answer: Arc<dyn Fn(Result<RcObjcId, String>) + Send + Sync> = Arc::new(move |result| {
            let error = result.as_ref().err().cloned();
            let _ = state.0.set(result);
            mailbox.push(match error {
                Some(log) => Done::PipelineFailed { sid, incarnation: incarnation.clone(), id, log },
                None => Done::PipelineReady { sid, incarnation: incarnation.clone(), id },
            });
        });
        // Asynchronous, as the render pipelines are: Metal compiles the libraries and pipelines
        // of every pending kernel at once (its compiler service), and the allocator thread is
        // not held for seconds per kernel (it queued the kernels one after another, behind the
        // allocations).
        self.dev.compile(Box::new(move || {
            let _pool = super::Pool::new();
            let options = super::pipeline::compile_options(precise);
            let callback_device = device.clone();
            let completion = objc_block!(move |library: ObjcId, error: ObjcId| {
                let Some(library) = NonNull::new(library) else { return answer(Err(super::pipeline::describe_error(error, "no compute library"))) };
                let _pool = super::Pool::new();
                let function: ObjcId = unsafe { msg_send![library.as_ptr(), newFunctionWithName: str_to_nsstring("kernel_main")] };
                let Some(function) = NonNull::new(function).map(RcObjcId::from_owned) else { return answer(Err("missing kernel_main".into())) };
                let answer = answer.clone();
                // the function, held until its pipeline is made
                let kept = function.clone();
                let made = objc_block!(move |pipe: ObjcId, error: ObjcId| {
                    let _ = &kept;
                    let result = match NonNull::new(pipe) {
                        None => Err(super::pipeline::describe_error(error, "no compute pipeline")),
                        Some(pipe) => {
                            let pipe = RcObjcId::from_unowned(pipe);
                            let limit: u64 = unsafe { msg_send![pipe.as_id(), maxTotalThreadsPerThreadgroup] };
                            if invocations > limit { Err(format!("compute workgroup has {invocations} threads; pipeline supports {limit}")) } else { Ok(pipe) }
                        }
                    };
                    answer(result);
                });
                unsafe { let _: () = msg_send![callback_device.as_id(), newComputePipelineStateWithFunction: function.as_id() completionHandler: &made]; }
            });
            unsafe { let _: () = msg_send![device.as_id(), newLibraryWithSource: str_to_nsstring(&source) options: options.as_id() completionHandler: &completion]; }
        }));
        PipeStatus::Pending
    }

    pub(super) fn encode_compute(&mut self, s: &mut Session, pass: &Pass, view: &CmdView, cb: ObjcId, seq: u64) -> Result<Vec<usize>, String> {
        let dispatches = compute::records::<Dispatch>(view.arena, pass.dispatches).ok_or("invalid dispatch records")?;
        // Preflight the whole pass: never run only a prefix while a later kernel compiles.
        for d in &dispatches {
            let p = self.compute.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).ok_or("stale compute pipeline")?;
            match p.state.0.get() { Some(Ok(_)) => {}, Some(Err(e)) => return Err(e.clone()), None => { self.pending_draws += 1; return Err("compute pipeline pending".into()); } }
        }
        let mut indices = Vec::new();
        // Stage-counter devices require encoder boundaries for timestamps. The
        // encoders still belong to this one pass and command-buffer submission.
        for d in &dispatches {
            let index = self.encode_dispatch(s, pass, view, cb, seq, d)?;
            if let Some(i) = index { indices.push(i); }
            if d.timer != 0 {
                if let (Some(samples), Some(index)) = (self.batch.as_ref().and_then(|b| b.samples.clone()), index) {
                    let time = super::timing::PassTime::Counters(samples, vec![index]);
                    let name = s.string(d.timer).unwrap_or("compute").to_string();
                    let sid = s.id;
                    let incarnation = s.font_frames.clone();
                    let mailbox = self.dev.mailbox.clone();
                    unsafe { let _: () = msg_send![cb, addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                        let status: u64 = unsafe { msg_send![cb, status] };
                        if status == 4 {
                            if let Some((busy, _, _)) = time.span(cb) {
                                mailbox.push(Done::DispatchTime { sid, incarnation: incarnation.clone(), name: name.clone(), gpu_ns: (busy.max(0.0) * 1e9) as u64 });
                            }
                        }
                    })]; }
                }
            }
        }
        Ok(indices)
    }

    fn encode_dispatch(&mut self, s: &mut Session, pass: &Pass, view: &CmdView, cb: ObjcId, seq: u64, dispatch: &Dispatch) -> Result<Option<usize>, String> {
        let dispatch_sampling: bool = self.batch.as_ref().is_some_and(|b| b.samples.is_some()) && unsafe { msg_send![self.dev.device, supportsCounterSampling: 2u64] };
        let samples = self.batch.as_ref().and_then(|b| b.samples.clone()).filter(|x| x.clock.stage || dispatch_sampling);
        let sample = samples.as_ref().and_then(|x| x.reserve());
        if let (Some(samples), Some(i)) = (&samples, sample) { samples.simple_encoder(i); }
        let encoder: ObjcId = unsafe {
            if let (Some(samples), Some(i)) = (&samples.as_ref().filter(|x| x.clock.stage), sample) {
                let desc: ObjcId = msg_send![class!(MTLComputePassDescriptor), computePassDescriptor];
                let attachments: ObjcId = msg_send![desc, sampleBufferAttachments];
                let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
                let _: () = msg_send![a, setSampleBuffer: samples.buffer.as_ref().map_or(nil, |b| b.as_id())];
                let _: () = msg_send![a, setStartOfEncoderSampleIndex: i as u64];
                let _: () = msg_send![a, setEndOfEncoderSampleIndex: (i + 1) as u64];
                msg_send![cb, computeCommandEncoderWithDescriptor: desc]
            } else { msg_send![cb, computeCommandEncoder] }
        };
        if let Some(key) = self.prof_key {
            if let Some(index) = crate::gpu_prof::with_frame(key, |b| {
                if sample.is_some() { b.expect(1); } else { b.untimed(1); }
                b.pass(s.string(pass.name).unwrap_or("compute"), makepad_gpu_trace::kind::COMPUTE, 0, 0, "", 0)
            }) {
                self.prof.push(super::frame::ProfPass { key, pass: index, kind: makepad_gpu_trace::kind::COMPUTE, sample, split: None });
            }
        }
        if encoder == nil { return Err("no compute encoder".into()); }
        if let (Some(samples), Some(i)) = (&samples.as_ref().filter(|x| !x.clock.stage), sample) {
            unsafe { let _: () = msg_send![encoder, sampleCountersInBuffer: samples.buffer.as_ref().unwrap().as_id() atSampleIndex: i as u64 withBarrier: true]; }
        }
        let result = (|| {
            unsafe { let _: () = msg_send![encoder, setLabel: str_to_nsstring(s.string(pass.name).unwrap_or("compute"))]; }
            for d in std::iter::once(dispatch) {
                let p = &self.compute[&(s.id, d.pipeline.index)];
                let pipeline = p.state.0.get().unwrap().as_ref().unwrap();
                p.layout.validate_dispatch(s, d, view.arena)?;
                let params = compute::bytes(view.arena, d.params).ok_or("invalid compute params")?;
                let buffers = compute::ids(view.arena, d.buffers).ok_or("invalid compute buffers")?;
                let textures = compute::ids(view.arena, d.textures).ok_or("invalid compute textures")?;
                if params.len() != p.layout.desc.push_size as usize || buffers.len() != p.layout.buffers.len() || textures.len() != p.layout.textures.len() { return Err("compute binding count mismatch".into()); }
                unsafe { let _: () = msg_send![encoder, setComputePipelineState: pipeline.as_id()]; }
                for (id, binding) in buffers.iter().zip(&p.layout.buffers) {
                    let b = self.buf.get_mut(&(s.id, id.index)).filter(|b| b.gen == id.gen).ok_or("stale compute buffer")?;
                    b.last_bound = seq;
                    unsafe { let _: () = msg_send![encoder, setBuffer: b.buffer.as_id() offset: 0u64 atIndex: binding.index as u64]; }
                }
                for (id, binding) in textures.iter().zip(&p.layout.textures) {
                    let desc = &s.textures.get(s.id, *id).map_err(|_| "stale storage texture")?.val;
                    if desc.usage & usage::STORAGE == 0 || desc.format != binding.format { return Err("storage image format/usage mismatch".into()); }
                    let t = self.tex.get_mut(&(s.id, id.index)).filter(|t| t.gen == id.gen).ok_or("missing storage image")?;
                    t.last_bound = seq;
                    unsafe { let _: () = msg_send![encoder, setTexture: t.obj.as_id() atIndex: binding.index as u64]; }
                }
                if !params.is_empty() { unsafe { let _: () = msg_send![encoder, setBytes: params.as_ptr() as *const std::ffi::c_void length: params.len() as u64 atIndex: p.layout.desc.param_binding as u64]; } }
                let [x, y, z] = p.layout.desc.workgroup;
                let threads = MTLSize { width: x as u64, height: y as u64, depth: z as u64 };
                if d.indirect_buffer.is_none() {
                    let [x, y, z] = d.groups;
                    let groups = MTLSize { width: x as u64, height: y as u64, depth: z as u64 };
                    unsafe { let _: () = msg_send![encoder, dispatchThreadgroups: groups threadsPerThreadgroup: threads]; }
                } else {
                    let b = self.buf.get_mut(&(s.id, d.indirect_buffer.index)).filter(|b| b.gen == d.indirect_buffer.gen).ok_or("stale indirect buffer")?;
                    if d.indirect_offset % 4 != 0 || d.indirect_offset.checked_add(12).is_none_or(|end| end > b.size) { return Err("invalid indirect offset".into()); }
                    b.last_bound = seq;
                    unsafe { let _: () = msg_send![encoder, dispatchThreadgroupsWithIndirectBuffer: b.buffer.as_id() indirectBufferOffset: d.indirect_offset threadsPerThreadgroup: threads]; }
                }
                // Serial encoder plus a buffers|textures memory barrier orders every dispatch.
                // All resources use tracked hazards (no heaps/untracked allocations), so ending
                // this encoder also orders later render/blit/indirect reads on the same queue.
                unsafe { let _: () = msg_send![encoder, memoryBarrierWithScope: 3u64]; }
            }
            Ok(sample)
        })();
        if let (Some(samples), Some(i)) = (&samples.as_ref().filter(|x| !x.clock.stage), sample) {
            unsafe { let _: () = msg_send![encoder, sampleCountersInBuffer: samples.buffer.as_ref().unwrap().as_id() atSampleIndex: (i + 1) as u64 withBarrier: true]; }
        }
        unsafe { let _: () = msg_send![encoder, endEncoding]; }
        result
    }

    pub(super) fn read_compute_buffer(&mut self, s: &mut Session, id: Id, req: u32, offset: u64, len: u64) {
        let Some(source) = self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).map(|b| b.buffer.clone()) else { s.events.readback(req, readback::STALE, [0; 4], &[]); return; };
        let staging: ObjcId = unsafe { msg_send![self.dev.device, newBufferWithLength: len.max(4) options: MTLResourceOptions::StorageModeShared] };
        let Some(staging) = NonNull::new(staging).map(RcObjcId::from_owned) else { s.events.readback(req, readback::FAILED, [0; 4], &[]); return; };
        let (cb, seq) = self.ensure_batch();
        self.buf.get_mut(&(s.id, id.index)).unwrap().last_bound = seq;
        self.flush_uploads();
        unsafe {
            let encoder: ObjcId = msg_send![cb, blitCommandEncoder];
            if len != 0 { let _: () = msg_send![encoder, copyFromBuffer: source.as_id() sourceOffset: offset toBuffer: staging.as_id() destinationOffset: 0u64 size: len]; }
            let _: () = msg_send![encoder, endEncoding];
        }
        let resources = Mutex::new(Some((source, staging)));
        let mailbox = self.dev.mailbox.clone();
        let sid = s.id;
        let incarnation = s.font_frames.clone();
        unsafe { let _: () = msg_send![cb, addCompletedHandler: &objc_block!(move |cb: ObjcId| {
            if let Some((_source, staging)) = resources.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let status: u64 = unsafe { msg_send![cb, status] };
                let pixels = if status == 4 {
                    let ptr: *const u8 = unsafe { msg_send![staging.as_id(), contents] };
                    unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() }
                } else { Vec::new() };
                mailbox.push(Done::Readback { sid, incarnation: incarnation.clone(), req, status: if status == 4 { readback::OK } else { readback::FAILED }, rect: [0; 4], pixels });
            }
        })]; }
    }
}
