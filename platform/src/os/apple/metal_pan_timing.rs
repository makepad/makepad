//! Diagnostic-only shader timing. Apple GPUs sample stage boundaries, so a
//! sampled frame stores/loads attachments between consecutive shader groups.
//! Such frames MUST NOT enter the ordinary command-buffer latency population.
use super::*;
use crate::draw_pass::GpuTimeRecorder;
use std::sync::atomic::AtomicBool;

/// Every application pass, including glyph/texture producers. Only Metal
/// completion callbacks lock the accumulator; the UI only installs a callback.
/// Duration sums describe work, not utilization: command buffers can overlap.
/// The one-second accumulation window of `record_pass`, owned by the
/// context and shared with its completion handlers (no static).
pub(super) type PassWindowShared = Arc<Mutex<Option<PassWindow>>>;

pub(super) fn record_pass(
    window: PassWindowShared,
    command: ObjcId, query: Option<GpuTimeRecorder>, label: String,
    repaint: u64, serial: u64, size: (f64, f64), containing: bool,
    intrusive: bool, upload_cpu_ms: f64, counters: GpuSampleCounters,
) {
    let tag = query.as_ref().map_or(0, GpuTimeRecorder::current_tag);
    unsafe {let _: () = msg_send![command, addCompletedHandler:&objc_block!(move |command: ObjcId| {
        let start: f64 = unsafe {msg_send![command, GPUStartTime]};
        let end: f64 = unsafe {msg_send![command, GPUEndTime]};
        let status: u64 = unsafe {msg_send![command, status]};
        crate::trace!("gpu.pass", "repaint={repaint} cb={serial} pass={label} pixels={:.0}x{:.0} start={start:.9} end={end:.9} ms={:.3} containing={containing} intrusive={intrusive} instances={} instance_bytes={} texture_bytes={} status={status}",size.0,size.1,(end-start)*1000.0,counters.instances,counters.instance_bytes,counters.texture_bytes);
        if intrusive || status != 4 || !start.is_finite() || start <= 0.0 || !end.is_finite() || end < start {return;}
        let Ok(mut state) = window.lock() else {return;};
        let now = Instant::now();
        let window = state.get_or_insert_with(||PassWindow {since:now,..Default::default()});
        let ms = (end-start)*1000.0;
        window.command_buffers += 1;
        window.total += ms;
        window.outside += if containing {0.0} else {ms};
        window.upload_cpu += upload_cpu_ms;
        window.instance_bytes += counters.instance_bytes;
        window.texture_bytes += counters.texture_bytes;
        if let Some(bin) = window.passes.iter_mut().find(|b|b.0==label) {
            bin.1 += 1; bin.2 += ms; bin.3 = bin.3.max(ms);
        } else if window.passes.len() < GROUPS {
            window.passes.push((label.clone(),1,ms,ms));
        } else {window.dropped += 1;}
        if now.duration_since(window.since).as_secs_f64() < 1.0 {return;}
        window.passes.sort_unstable_by(|a,b|b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        let mut line = format!("gpu: scope=all_app_passes window_ms={:.3} cb_count={} cb_ms_sum={:.3} outside_containing_ms_sum={:.3} upload_cpu_ms={:.3} instance_bytes={} texture_bytes={} passes_dropped={} pass_ms=[",now.duration_since(window.since).as_secs_f64()*1000.0,window.command_buffers,window.total,window.outside,window.upload_cpu,window.instance_bytes,window.texture_bytes,window.dropped);
        for (i,(name,count,sum,max)) in window.passes.iter().enumerate() {
            let _ = write!(line,"{}{name}:buffers={count},sum={sum:.3},mean={:.3},max={max:.3}",if i==0 {""}else{";"},sum / *count as f64);
        }
        line.push(']');
        crate::log!("{line}");
        if let Some(query) = &query {query.record_breakdown(tag,line);}
        window.since=now; window.passes.clear(); window.command_buffers=0;
        window.total=0.0; window.outside=0.0; window.upload_cpu=0.0;
        window.instance_bytes=0; window.texture_bytes=0; window.dropped=0;
    })];}
}
pub(super) struct PassWindow {
    since: Instant, passes: Vec<(String,u64,f64,f64)>, command_buffers:u64,
    total:f64, outside:f64, upload_cpu:f64, instance_bytes:u64, texture_bytes:u64, dropped:u64,
}
impl Default for PassWindow {
    fn default()->Self {
        Self {since:Instant::now(),passes:Vec::new(),command_buffers:0,total:0.0,outside:0.0,upload_cpu:0.0,instance_bytes:0,texture_bytes:0,dropped:0}
    }
}

const GROUPS: usize = 1024;
const SAMPLES: usize = 2 + GROUPS * 2;
extern "C" {
    static MTLCommonCounterSetTimestamp: ObjcId;
}
struct CounterSlot {
    buffer: RcObjcId,
    busy: AtomicBool,
}
pub(super) struct PanCounters {
    device: RcObjcId,
    slots: [CounterSlot; 3],
    stage: bool,
    blit_boundary: bool,
    epoch: Instant,
    next_ms: AtomicU64,
    repaint: AtomicU64,
    pass_calls: AtomicU64,
    pass_count: AtomicU64,
    sample_turn: AtomicU64,
}
impl PanCounters {
    // Called only by the existing Metal allocation worker.
    pub(super) fn new(device: &RcObjcId) -> Option<Arc<Self>> {
        if std::env::var_os("MAKEPAD_ATLAS_DIAGNOSTICS").is_none() { return None; }
        unsafe {
            let available: bool = msg_send![device.as_id(), respondsToSelector:sel!(supportsCounterSampling:)];
            if !available { crate::log!("gpu: counters=unavailable reason=unsupported_api"); return None; }
            let stage: bool = msg_send![device.as_id(), supportsCounterSampling:0u64];
            let draw: bool = msg_send![device.as_id(), supportsCounterSampling:1u64];
            let blit_boundary: bool = msg_send![device.as_id(), supportsCounterSampling:4u64];
            if !stage && !draw { crate::log!("gpu: counters=unavailable reason=unsupported_boundary"); return None; }
            let sets: ObjcId = msg_send![device.as_id(), counterSets];
            let count: u64 = msg_send![sets, count];
            let mut set = nil;
            for i in 0..count {
                let candidate: ObjcId = msg_send![sets, objectAtIndex:i];
                let name: ObjcId = msg_send![candidate, name];
                let matches: bool = msg_send![name, isEqualToString:MTLCommonCounterSetTimestamp];
                if matches { set = candidate; break; }
            }
            if set == nil { crate::log!("gpu: counters=unavailable reason=no_timestamps"); return None; }
            let descriptor = RcObjcId::from_owned(NonNull::new(msg_send![class!(MTLCounterSampleBufferDescriptor), new])?);
            let _: () = msg_send![descriptor.as_id(), setCounterSet:set];
            let _: () = msg_send![descriptor.as_id(), setStorageMode:0u64];
            let _: () = msg_send![descriptor.as_id(), setSampleCount:SAMPLES as u64];
            let create = || {
                let mut error: ObjcId = nil;
                let buffer = NonNull::new(msg_send![device.as_id(), newCounterSampleBufferWithDescriptor:descriptor.as_id() error:&mut error]);
                if buffer.is_none() { crate::log!("gpu: counters=unavailable reason=allocation_failed"); }
                buffer.map(|buffer| CounterSlot { buffer:RcObjcId::from_owned(buffer), busy:AtomicBool::new(false) })
            };
            Some(Arc::new(Self {device:device.clone(), slots:[create()?,create()?,create()?], stage:stage && !draw, blit_boundary, epoch:Instant::now(),next_ms:AtomicU64::new(0),repaint:AtomicU64::new(u64::MAX),pass_calls:AtomicU64::new(0),pass_count:AtomicU64::new(1),sample_turn:AtomicU64::new(0)}))
        }
    }
    pub(super) fn begin(self: &Arc<Self>, query: Option<GpuTimeRecorder>, label: String, upload_cpu_ms:f64, repaint:u64, containing:bool) -> Option<PanFrame> {
        // Rotate across the pass order. Always taking the first pass each
        // second would permanently starve the containing pass when an
        // offscreen producer renders first. No repaint is requested to sample.
        if self.repaint.swap(repaint,Ordering::Relaxed)!=repaint {
            let count=self.pass_calls.swap(0,Ordering::Relaxed);
            self.pass_count.store(count.max(1),Ordering::Relaxed);
        }
        let ordinal=self.pass_calls.fetch_add(1,Ordering::Relaxed);
        let turn=self.sample_turn.load(Ordering::Relaxed);
        let target=(turn/2)%self.pass_count.load(Ordering::Relaxed);
        // Reserve alternate samples for the containing pass, even when many
        // texture producers precede it. The other samples rotate through all
        // producers; all-pass completion timings still cover every buffer.
        if turn%2==0 && query.is_some() {
            if !containing {return None;}
        } else if ordinal!=target {return None;}
        let now = self.epoch.elapsed().as_millis() as u64;
        let next = self.next_ms.load(Ordering::Relaxed);
        if now < next || self.next_ms.compare_exchange(next,now+1000,Ordering::Relaxed,Ordering::Relaxed).is_err() {return None;}
        let slot = self.slots.iter().position(|s| s.busy.compare_exchange(false,true,Ordering::Acquire,Ordering::Relaxed).is_ok())?;
        self.sample_turn.fetch_add(1,Ordering::Relaxed);
        let mut cpu = 0u64; let mut gpu = 0u64;
        unsafe { let _: () = msg_send![self.device.as_id(), sampleTimestamps:&mut cpu gpuTimestamp:&mut gpu]; }
        let tag = query.as_ref().map_or(0, GpuTimeRecorder::current_tag);
        Some(PanFrame { counters:self.clone(),slot,query,tag,label,groups:[Group::EMPTY;GROUPS],len:0,truncated:false,blit:false,cpu,gpu,upload_cpu_ms })
    }
}
#[derive(Clone, Copy)]
struct Group { shader:usize,name:&'static str }
impl Group { const EMPTY:Self=Self {shader:usize::MAX,name:"clear"}; }
pub(super) struct PanFrame {
    counters:Arc<PanCounters>,slot:usize,query:Option<GpuTimeRecorder>,tag:u64,label:String,
    groups:[Group;GROUPS],len:usize,truncated:bool,blit:bool,
    cpu:u64,gpu:u64,upload_cpu_ms:f64,
}
impl PanFrame {
    fn buffer(&self)->ObjcId {self.counters.slots[self.slot].buffer.as_id()}
    fn stage(&self)->bool {self.counters.stage}
    pub(super) fn blit_encoder(&mut self, command:ObjcId)->ObjcId {
        self.blit=true;
        unsafe {
            if self.stage() {
                let descriptor:ObjcId=msg_send![class!(MTLBlitPassDescriptor), blitPassDescriptor];
                let attachments:ObjcId=msg_send![descriptor,sampleBufferAttachments];
                let attachment:ObjcId=msg_send![attachments,objectAtIndexedSubscript:0u64];
                let _:()=msg_send![attachment,setSampleBuffer:self.buffer()];
                let _:()=msg_send![attachment,setStartOfEncoderSampleIndex:0u64];
                let _:()=msg_send![attachment,setEndOfEncoderSampleIndex:1u64];
                msg_send![command,blitCommandEncoderWithDescriptor:descriptor]
            } else {
                let encoder:ObjcId=msg_send![command,blitCommandEncoder];
                if self.counters.blit_boundary {self.sample(encoder,0);}
                encoder
            }
        }
    }
    pub(super) fn end_blit(&self,encoder:ObjcId) {
        if !self.stage() && self.counters.blit_boundary {self.sample(encoder,1);}
    }
    fn sample(&self,encoder:ObjcId,index:usize) {
        unsafe { let _:()=msg_send![encoder,sampleCountersInBuffer:self.buffer() atSampleIndex:index as u64 withBarrier:true]; }
    }
    fn attach(&self,descriptor:ObjcId,index:usize) {
        unsafe {
            let attachments:ObjcId=msg_send![descriptor,sampleBufferAttachments];
            let a:ObjcId=msg_send![attachments,objectAtIndexedSubscript:0u64];
            let _:()=msg_send![a,setSampleBuffer:self.buffer()];
            let _:()=msg_send![a,setStartOfVertexSampleIndex:(2+index*2) as u64];
            let _:()=msg_send![a,setEndOfVertexSampleIndex:u64::MAX];
            let _:()=msg_send![a,setStartOfFragmentSampleIndex:u64::MAX];
            let _:()=msg_send![a,setEndOfFragmentSampleIndex:(3+index*2) as u64];
        }
    }
    pub(super) fn complete(self,command:ObjcId) {
        let tag=self.tag;
        unsafe { let _:()=msg_send![command,addCompletedHandler:&objc_block!(move |command:ObjcId| {
            let mut cpu=0u64;let mut gpu=0u64;
            unsafe {let _:()=msg_send![self.counters.device.as_id(),sampleTimestamps:&mut cpu gpuTimestamp:&mut gpu];}
            let scale=if self.stage() {1e-6} else if gpu>self.gpu && cpu>self.cpu {(cpu-self.cpu) as f64/(gpu-self.gpu) as f64*1e-6} else {f64::NAN};
            let data:ObjcId=unsafe {msg_send![self.buffer(),resolveCounterRange:NSRange {location:0,length:(2+self.len*2) as _}]};
            let bytes:u64=unsafe {msg_send![data,length]};
            let ptr:*const u64=unsafe {msg_send![data,bytes]};
            let mut invalid=0;
            let mut span=|i:usize|->Option<f64> {
                if ptr.is_null() || bytes < ((i+2)*8) as u64 {invalid+=1;return None;}
                let (a,b)=unsafe {(*ptr.add(i),*ptr.add(i+1))};
                if a==u64::MAX || b==u64::MAX || a==0 || b<a || !scale.is_finite() {invalid+=1;return None;}
                Some((b-a) as f64*scale)
            };
            let blit=if !self.blit {Some(0.0)}else if self.stage()||self.counters.blit_boundary {span(0)}else{None};
            let mut bins:Vec<(usize,&str,f64)>=Vec::new();
            let mut render=0.0;
            for i in 0..self.len {
                let g=self.groups[i];
                if let Some(ms)=span(2+i*2) {
                    render+=ms;
                    if let Some(bin)=bins.iter_mut().find(|b|b.0==g.shader) {bin.2+=ms;}else{bins.push((g.shader,g.name,ms));}
                }
            }
            bins.sort_unstable_by(|a,b|b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
            let start:f64=unsafe {msg_send![command,GPUStartTime]};
            let end:f64=unsafe {msg_send![command,GPUEndTime]};
            let mut line=format!("gpu: pass={} total={:.3}ms render={render:.3}ms blit={} upload_cpu={:.3}ms mode={} groups={} truncated={} invalid={} shader_ms=[",self.label,(end-start)*1000.0,blit.map_or("unavailable".into(),|ms|format!("{ms:.3}ms")),self.upload_cpu_ms,if self.stage(){"stage_split"}else{"draw_boundary"},self.len,self.truncated,invalid);
            for (i,(shader,name,ms)) in bins.iter().enumerate() {let _=write!(line,"{}{name}#{shader}={ms:.3}ms",if i==0{""}else{";"});}
            line.push(']');
            crate::log!("{line}");
            if let Some(query) = &self.query {query.record_breakdown(tag,line);}
            self.counters.slots[self.slot].busy.store(false,Ordering::Release);
        })]; }
    }
}

pub(super) struct PanEncoder {
    pub(super) encoder:ObjcId,
    command:ObjcId,descriptor:ObjcId,viewport:MTLViewport,
    pub(super) frame:Option<PanFrame>,
}
impl PanEncoder {
    pub(super) fn new(command:ObjcId,descriptor:ObjcId,viewport:MTLViewport,frame:Option<PanFrame>)->Self {
        unsafe {
            // The descriptor is reused: never leave a previous frame's buffer attached.
            let available:bool=msg_send![descriptor,respondsToSelector:sel!(sampleBufferAttachments)];
            if available {
                let attachments:ObjcId=msg_send![descriptor,sampleBufferAttachments];
                let a:ObjcId=msg_send![attachments,objectAtIndexedSubscript:0u64];
                let _:()=msg_send![a,setSampleBuffer:nil];
            }
        }
        if let Some(f)=&frame {if f.stage(){f.attach(descriptor,0);}}
        let encoder:ObjcId=unsafe {msg_send![command,renderCommandEncoderWithDescriptor:descriptor]};
        unsafe {let _:()=msg_send![encoder,setViewport:viewport];}
        Self {encoder,command,descriptor,viewport,frame}
    }
    pub(super) fn group(&mut self,shader:usize,sh:&CxDrawShader)->ObjcId {
        let Some(f)=&mut self.frame else{return self.encoder;};
        if f.len>0 && f.groups[f.len-1].shader==shader{return self.encoder;}
        if f.len==GROUPS {f.truncated=true;return self.encoder;}
        if f.len>0 {
            if f.stage() {
                unsafe {
                    let _:()=msg_send![self.encoder,endEncoding];
                    let colors:ObjcId=msg_send![self.descriptor,colorAttachments];
                    for i in 0..8u64 {
                        let a:ObjcId=msg_send![colors,objectAtIndexedSubscript:i];
                        let texture:ObjcId=msg_send![a,texture];
                        if texture!=nil {let _:()=msg_send![a,setLoadAction:MTLLoadAction::Load];}
                    }
                    let depth:ObjcId=msg_send![self.descriptor,depthAttachment];
                    let texture:ObjcId=msg_send![depth,texture];
                    if texture!=nil {let _:()=msg_send![depth,setLoadAction:MTLLoadAction::Load];}
                }
                f.attach(self.descriptor,f.len);
                self.encoder=unsafe {msg_send![self.command,renderCommandEncoderWithDescriptor:self.descriptor]};
                unsafe {let _:()=msg_send![self.encoder,setViewport:self.viewport];}
            }else {f.sample(self.encoder,3+(f.len-1)*2);}
        }
        if !f.stage(){f.sample(self.encoder,2+f.len*2);}
        let has=|id|sh.mapping.instances.inputs.iter().any(|v|v.id==id);
        let name=if has(id!(shadow_radius)){"DrawAtlasNode"}else if has(id!(selection_base))||has(id!(packed_rect)){"DrawAtlasStructureBatch"}else if has(id!(char_index)){"DrawText"}else if has(id!(cell_length)){"DrawCodeTokens"}else if has(id!(mean_4)){"DrawCodeMergedLines"}else if has(id!(width_coverage)){"DrawCodeLines"}else if has(id!(glyph))&&has(id!(cell_row)){"DrawCodeView"}else{"Shader"};
        f.groups[f.len]=Group {shader,name};f.len+=1;
        self.encoder
    }
    pub(super) fn end(self) {
        if let Some(f)=&self.frame {if !f.stage() && f.len>0 {f.sample(self.encoder,3+(f.len-1)*2);}}
        unsafe {let _:()=msg_send![self.encoder,endEncoding];}
        if let Some(f)=self.frame {f.complete(self.command);}
    }
}
