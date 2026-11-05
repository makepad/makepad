//! The D3D11 device: factory, adapter, device and immediate context, the fixed-function states
//! every draw picks from, the HAL format map and the HLSL compile threads. Moved from
//! os/windows/d3d11.rs (`D3d11Cx::create_device_tier`, `CxOsPass::set_states`,
//! `texture_pixel_to_dx11_pixel`); the device is shared by every session of the process.
//!
//! The immediate context is multithread-protected: the HAL's submit path (under the HAL state
//! lock) records, and the completion worker polls queries and maps finished readbacks. Neither
//! waits for the other beyond the context's own per-call lock.

use crate::types::*;
use makepad_windows_sys::core::{BOOL, HRESULT};
use makepad_windows_sys::Win32::Graphics::Direct3D::*;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::*;
use std::collections::{HashMap, VecDeque};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn b(v: bool) -> BOOL {
    BOOL(v as i32)
}

pub(crate) fn hr(what: &str, e: HRESULT) -> String {
    format!("{what}: {e}")
}

/// The HLSL compile threads. They take jobs from one queue, so a long compile holds up only
/// the thread running it, never the jobs queued behind it. A job a frame draws goes ahead of
/// those no frame draws ([`CompilePool::hurry`]).
pub struct CompilePool {
    queue: Arc<(Mutex<CompileQueue>, Condvar)>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

type CompileJob = (usize, Box<dyn FnOnce() + Send>);

struct CompileQueue {
    /// jobs a frame draws, in the order they were wanted
    wanted: VecDeque<CompileJob>,
    jobs: VecDeque<CompileJob>,
    /// false once `stop` ran: later pushes are dropped
    open: bool,
}

impl CompilePool {
    pub fn new(name: &str, threads: usize) -> Result<Self, String> {
        let queue = Arc::new((Mutex::new(CompileQueue { wanted: VecDeque::new(), jobs: VecDeque::new(), open: true }), Condvar::new()));
        // a failed spawn drops `pool`: the threads already running end
        let mut pool = Self { queue, threads: Vec::new() };
        for i in 0..threads {
            let queue = pool.queue.clone();
            let thread = std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    let (jobs, wake) = &*queue;
                    loop {
                        let job = {
                            let mut q = lock(jobs);
                            loop {
                                let next = match q.wanted.pop_front() {
                                    Some(job) => Some(job),
                                    None => q.jobs.pop_front(),
                                };
                                if let Some((_, job)) = next { break job; }
                                if !q.open { return; }
                                q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        job();
                    }
                })
                .map_err(|e| format!("{name}-{i}: {e}"))?;
            pool.threads.push(thread);
        }
        Ok(pool)
    }

    /// Queues `job` under `key` (what [`CompilePool::hurry`] names it by).
    pub fn push(&self, key: usize, job: impl FnOnce() + Send + 'static) {
        let (jobs, wake) = &*self.queue;
        let mut q = lock(jobs);
        if q.open {
            q.jobs.push_back((key, Box::new(job)));
            wake.notify_one();
        }
    }

    /// A frame draws what the job queued under `key` compiles: it goes ahead of every job no
    /// frame asked for. Nothing when it is running or done.
    pub fn hurry(&self, key: usize) {
        let (jobs, _) = &*self.queue;
        let mut q = lock(jobs);
        if let Some(job) = q.jobs.iter().position(|(k, _)| *k == key).and_then(|at| q.jobs.remove(at)) {
            q.wanted.push_back(job);
        }
    }

    /// The threads run the jobs queued so far and end; this returns once they have. Later
    /// pushes are dropped. The end of the process (`D3d11::before_exit`) and `Drop`.
    pub fn stop(&mut self) {
        let (jobs, wake) = &*self.queue;
        lock(jobs).open = false;
        wake.notify_all();
        for thread in self.threads.drain(..) {
            if thread.join().is_err() { eprintln!("[hal-d3d11] a compile thread panicked"); }
        }
    }
}

impl Drop for CompilePool {
    fn drop(&mut self) { self.stop(); }
}

/// How a HAL format is stored, viewed and sampled.
#[derive(Clone, Copy, Debug)]
pub struct Fmt {
    /// the resource format (typeless for depth, so it can be both a depth target and sampled)
    pub resource: DXGI_FORMAT,
    pub srv: DXGI_FORMAT,
    pub rtv: DXGI_FORMAT,
    pub dsv: DXGI_FORMAT,
}

pub fn dxgi(format: u32) -> Option<Fmt> {
    let plain = |f| Some(Fmt { resource: f, srv: f, rtv: f, dsv: DXGI_FORMAT_UNKNOWN });
    match format {
        format::RGBA8 => plain(DXGI_FORMAT_R8G8B8A8_UNORM),
        format::BGRA8 => plain(DXGI_FORMAT_B8G8R8A8_UNORM),
        format::R8 => plain(DXGI_FORMAT_R8_UNORM),
        format::RG8 => plain(DXGI_FORMAT_R8G8_UNORM),
        format::RGBA16F => plain(DXGI_FORMAT_R16G16B16A16_FLOAT),
        format::RGBA32F => plain(DXGI_FORMAT_R32G32B32A32_FLOAT),
        format::R32F => plain(DXGI_FORMAT_R32_FLOAT),
        format::RG16F => plain(DXGI_FORMAT_R16G16_FLOAT),
        format::R16F => plain(DXGI_FORMAT_R16_FLOAT),
        format::R32UINT => plain(DXGI_FORMAT_R32_UINT),
        format::BC7 => plain(DXGI_FORMAT_BC7_UNORM),
        format::BC5 => plain(DXGI_FORMAT_BC5_UNORM),
        // sampled and rendered by plane: R8 views are its luma, RG8 views its chroma
        format::NV12 => Some(Fmt { resource: DXGI_FORMAT_NV12, srv: DXGI_FORMAT_R8_UNORM, rtv: DXGI_FORMAT_R8_UNORM, dsv: DXGI_FORMAT_UNKNOWN }),
        format::DEPTH32F => Some(Fmt { resource: DXGI_FORMAT_R32_TYPELESS, srv: DXGI_FORMAT_R32_FLOAT, rtv: DXGI_FORMAT_UNKNOWN, dsv: DXGI_FORMAT_D32_FLOAT }),
        format::DEPTH24_STENCIL8 => Some(Fmt {
            resource: DXGI_FORMAT_R24G8_TYPELESS,
            srv: DXGI_FORMAT_R24_UNORM_X8_TYPELESS,
            rtv: DXGI_FORMAT_UNKNOWN,
            dsv: DXGI_FORMAT_D24_UNORM_S8_UINT,
        }),
        // ASTC has no D3D11 format
        _ => None,
    }
}

/// Fixed-function states, made once per device.
pub struct States {
    pub raster: [ID3D11RasterizerState; 2],
    /// No test; LessEqual with/without writes; Greater with/without writes.
    pub depth: [ID3D11DepthStencilState; 5],
    /// blend states by (per-attachment mode bits, max): made on first use
    pub blend: Mutex<HashMap<u32, ID3D11BlendState>>,
}

pub struct Device {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    /// D3D11.1: constant-buffer windows into the per-frame arena
    pub ctx1: ID3D11DeviceContext1,
    pub factory: IDXGIFactory2,
    pub adapter3: Option<IDXGIAdapter3>,
    pub adapter_name: String,
    pub states: States,
    /// bit per HAL format the device can sample
    pub formats: u64,
    /// bit per HAL format the device can render to
    pub renderable: u64,
    /// bit per HAL format the device blends
    pub blendable: u64,
    pub lost: AtomicBool,
    /// what the backend allocates for itself (arenas, staging, leases, window copies), on the
    /// HAL's ledger as backend rows: one for the process, as the device counter counts it (every
    /// device of it: the HAL's, a replaced one's leftovers, the shared-texture copier's)
    pub pools: Arc<crate::ledger::Pools>,
}

fn process_pools() -> Arc<crate::ledger::Pools> {
    static POOLS: std::sync::OnceLock<Arc<crate::ledger::Pools>> = std::sync::OnceLock::new();
    POOLS.get_or_init(Default::default).clone()
}

impl Device {
    pub fn new() -> Result<Arc<Device>, String> {
        unsafe {
            let factory = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0), &IDXGIFactory2::IID)
                .and_then(|raw| IDXGIFactory2::from_raw(raw))
                .map_err(|e| hr("CreateDXGIFactory2", e))?;
            // the GPU with the most memory of its own (with a discrete GPU next to an iGPU the
            // discrete one renders, the iGPU only scans out; DWM moves the frames across);
            // adapter 0 (the primary output's) when none says more. The software adapter never.
            let mut adapter = factory.EnumAdapters(0).map_err(|e| hr("IDXGIFactory2::EnumAdapters(0)", e))?;
            let mut best = adapter.GetDesc().map(|d| if d.VendorId == 0x1414 { 0 } else { d.DedicatedVideoMemory }).unwrap_or(0);
            let mut i = 1;
            while let Ok(a) = factory.EnumAdapters(i) {
                let memory = a.GetDesc().map(|d| if d.VendorId == 0x1414 { 0 } else { d.DedicatedVideoMemory }).unwrap_or(0);
                if memory > best {
                    best = memory;
                    adapter = a;
                }
                i += 1;
            }
            let adapter_name = adapter
                .GetDesc()
                .map(|d| {
                    let len = d.Description.iter().position(|c| *c == 0).unwrap_or(d.Description.len());
                    String::from_utf16_lossy(&d.Description[..len])
                })
                .unwrap_or_default();
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 | D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0),
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .map_err(|e| hr("D3D11CreateDevice (feature level 11)", e))?;
            let device = device.ok_or("D3D11CreateDevice returned no device")?;
            let ctx = ctx.ok_or("D3D11CreateDevice returned no context")?;
            let ctx1 = ID3D11DeviceContext1::query(ctx.as_raw()).map_err(|e| hr("ID3D11DeviceContext1 (D3D11.1 runtime)", e))?;
            if let Ok(mt) = ID3D11Multithread::query(ctx.as_raw()) {
                let _ = mt.SetMultithreadProtected(true);
            }
            let adapter3 = IDXGIAdapter3::query(adapter.as_raw()).ok();

            let raster = |cull| -> Result<ID3D11RasterizerState, String> {
                let desc = D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: cull,
                    FrontCounterClockwise: b(true),
                    DepthClipEnable: b(true),
                    ScissorEnable: b(true),
                    ..Default::default()
                };
                let mut out = None;
                device.CreateRasterizerState(&desc, Some(&mut out)).map_err(|e| hr("CreateRasterizerState", e))?;
                out.ok_or_else(|| "CreateRasterizerState returned nothing".into())
            };
            let depth = |enable: bool, write, compare| -> Result<ID3D11DepthStencilState, String> {
                let op = D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilPassOp: D3D11_STENCIL_OP_KEEP,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                };
                let desc = D3D11_DEPTH_STENCIL_DESC {
                    DepthEnable: b(enable),
                    DepthWriteMask: write,
                    DepthFunc: compare,
                    StencilEnable: b(false),
                    StencilReadMask: 0xff,
                    StencilWriteMask: 0xff,
                    FrontFace: op,
                    BackFace: op,
                };
                let mut out = None;
                device.CreateDepthStencilState(&desc, Some(&mut out)).map_err(|e| hr("CreateDepthStencilState", e))?;
                out.ok_or_else(|| "CreateDepthStencilState returned nothing".into())
            };
            let states = States {
                raster: [raster(D3D11_CULL_NONE)?, raster(D3D11_CULL_BACK)?],
                depth: [
                    depth(false, D3D11_DEPTH_WRITE_MASK_ZERO, D3D11_COMPARISON_LESS_EQUAL)?,
                    depth(true, D3D11_DEPTH_WRITE_MASK_ALL, D3D11_COMPARISON_LESS_EQUAL)?,
                    depth(true, D3D11_DEPTH_WRITE_MASK_ZERO, D3D11_COMPARISON_LESS_EQUAL)?,
                    depth(true, D3D11_DEPTH_WRITE_MASK_ALL, D3D11_COMPARISON_GREATER)?,
                    depth(true, D3D11_DEPTH_WRITE_MASK_ZERO, D3D11_COMPARISON_GREATER)?,
                ],
                blend: Mutex::new(HashMap::new()),
            };

            let (mut formats, mut renderable, mut blendable) = (0u64, 0u64, 0u64);
            for f in 0..format::COUNT {
                let Some(fmt) = dxgi(f) else { continue };
                let support = device.CheckFormatSupport(fmt.resource).unwrap_or(0);
                let srv = if fmt.srv == fmt.resource { support } else { device.CheckFormatSupport(fmt.srv).unwrap_or(0) };
                if srv & D3D11_FORMAT_SUPPORT_SHADER_SAMPLE.0 as u32 != 0 || (format::is_depth(f) && srv & D3D11_FORMAT_SUPPORT_TEXTURE2D.0 as u32 != 0) || f == format::R32UINT {
                    formats |= 1 << f;
                }
                let target = if format::is_depth(f) { device.CheckFormatSupport(fmt.dsv).unwrap_or(0) & D3D11_FORMAT_SUPPORT_DEPTH_STENCIL.0 as u32 }
                    else { support & D3D11_FORMAT_SUPPORT_RENDER_TARGET.0 as u32 };
                if target != 0 {
                    renderable |= 1 << f;
                }
                if support & D3D11_FORMAT_SUPPORT_BLENDABLE.0 as u32 != 0 {
                    blendable |= 1 << f;
                }
            }
            Ok(Arc::new(Device {
                device,
                ctx,
                ctx1,
                factory,
                adapter3,
                adapter_name,
                states,
                formats,
                renderable,
                blendable,
                lost: AtomicBool::new(false),
                pools: process_pools(),
            }))
        }
    }

    /// The driver's budget and use of local video memory, when DXGI 1.4 reports it.
    pub fn budget(&self) -> Option<(u64, u64)> {
        let a = self.adapter3.as_ref()?;
        let info = unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL) }.ok()?;
        Some((info.Budget, info.CurrentUsage))
    }

    /// The device's own count of this process's video memory (local plus non-local), when
    /// DXGI 1.4 reports it: what the ledger is checked against.
    pub fn usage(&self) -> Option<u64> {
        let a = self.adapter3.as_ref()?;
        let local = unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL) }.ok()?;
        let non_local = unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL) }.ok()?;
        Some(local.CurrentUsage + non_local.CurrentUsage)
    }

    /// Whether the device was removed or reset (asked of the device itself).
    pub fn check_lost(&self) -> bool {
        if self.lost.load(Ordering::Acquire) {
            return true;
        }
        let reason = unsafe { (self.device.vtable().GetDeviceRemovedReason)(self.device.as_raw()) };
        if reason.is_err() {
            if !self.lost.swap(true, Ordering::AcqRel) {
                eprintln!("[hal-d3d11] DEVICE LOST: GetDeviceRemovedReason {reason}");
            }
            return true;
        }
        false
    }

    /// The blend state of a draw: bit `2i` of `modes` enables the over-blend on attachment `i`,
    /// bit `2i+1` writes it at all (an output the shader does not write keeps its pixels).
    pub fn blend_state(&self, modes: u32, max: bool) -> Option<ID3D11BlendState> {
        let key = modes | (max as u32) << 31;
        let mut cache = lock(&self.states.blend);
        if let Some(b) = cache.get(&key) {
            return Some(b.clone());
        }
        let mut desc = D3D11_BLEND_DESC { AlphaToCoverageEnable: b(false), IndependentBlendEnable: b(true), ..Default::default() };
        let op = if max { D3D11_BLEND_OP_MAX } else { D3D11_BLEND_OP_ADD };
        let dest = if max { D3D11_BLEND_ONE } else { D3D11_BLEND_INV_SRC_ALPHA };
        for (i, rt) in desc.RenderTarget.iter_mut().enumerate() {
            let blend = modes & (1 << (2 * i)) != 0;
            let write = modes & (1 << (2 * i + 1)) != 0;
            *rt = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: b(blend),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: dest,
                BlendOp: op,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: dest,
                BlendOpAlpha: op,
                RenderTargetWriteMask: if write { D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8 } else { 0 },
            };
        }
        let mut out = None;
        if let Err(e) = unsafe { self.device.CreateBlendState(&desc, Some(&mut out)) } {
            eprintln!("[hal-d3d11] CreateBlendState: {e}");
            return None;
        }
        let out = out?;
        cache.insert(key, out.clone());
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::CompilePool;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A long compile holds up only the thread running it: the jobs queued behind it run on the
    /// other threads (a queue per thread, filled round robin, left every other job behind it).
    #[test]
    fn a_long_compile_holds_up_only_its_own_thread() {
        let pool = CompilePool::new("test-compile", 2).unwrap();
        let (release, released) = mpsc::channel::<()>();
        let (done, finished) = mpsc::channel::<usize>();
        // the long one runs until the jobs queued after it are done
        pool.push(0, move || { let _ = released.recv_timeout(Duration::from_secs(10)); });
        for i in 1..=4 {
            let done = done.clone();
            pool.push(i, move || { let _ = done.send(i); });
        }
        for _ in 0..4 {
            finished.recv_timeout(Duration::from_secs(10)).expect("a job queued behind the long compile waited for it");
        }
        let _ = release.send(());
    }

    /// A compile a frame draws runs before the ones queued ahead of it that no frame draws.
    #[test]
    fn a_wanted_compile_goes_ahead_of_the_queue() {
        let pool = CompilePool::new("test-compile", 1).unwrap();
        let (release, released) = mpsc::channel::<()>();
        let (done, finished) = mpsc::channel::<usize>();
        // the one thread is busy while the queue fills
        pool.push(0, move || { let _ = released.recv_timeout(Duration::from_secs(10)); });
        for key in 1..=3 {
            let done = done.clone();
            pool.push(key, move || { let _ = done.send(key); });
        }
        pool.hurry(3);
        let _ = release.send(());
        let order: Vec<usize> = (0..3).map(|_| finished.recv_timeout(Duration::from_secs(10)).expect("every job runs")).collect();
        assert_eq!(order, [3, 1, 2]);
    }
}
