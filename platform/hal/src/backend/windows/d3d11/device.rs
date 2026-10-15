//! The D3D11 device: factory, adapter, device and immediate context, the fixed-function states
//! every draw picks from, the HAL format map and the worker threads. Moved from
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
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn b(v: bool) -> BOOL {
    BOOL(v as i32)
}

pub(crate) fn hr(what: &str, e: HRESULT) -> String {
    format!("{what}: {e}")
}

/// A long-lived thread running boxed jobs in order (compiles, retirement).
pub struct Worker {
    tx: Option<mpsc::Sender<Box<dyn FnOnce() + Send>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub fn new(name: &str) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Self { tx: Some(tx), thread: Some(thread) })
    }

    pub fn push(&self, job: impl FnOnce() + Send + 'static) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Box::new(job));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
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
        format::R32UINT => plain(DXGI_FORMAT_R32_UINT),
        format::BC7 => plain(DXGI_FORMAT_BC7_UNORM),
        format::BC5 => plain(DXGI_FORMAT_BC5_UNORM),
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
    /// [no depth test, test + write, test without write]
    pub depth: [ID3D11DepthStencilState; 3],
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
    /// device-local bytes allocated by this backend (textures and buffers it made)
    pub allocated: AtomicU64,
}

impl Device {
    pub fn new() -> Result<Arc<Device>, String> {
        unsafe {
            let factory = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0), &IDXGIFactory2::IID)
                .and_then(|raw| IDXGIFactory2::from_raw(raw))
                .map_err(|e| hr("CreateDXGIFactory2", e))?;
            // adapter 0: the one the desktop's primary output is on (the old backend's choice)
            let adapter = factory.EnumAdapters(0).map_err(|e| hr("IDXGIFactory2::EnumAdapters(0)", e))?;
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
            let depth = |enable: bool, write| -> Result<ID3D11DepthStencilState, String> {
                let op = D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilPassOp: D3D11_STENCIL_OP_KEEP,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                };
                let desc = D3D11_DEPTH_STENCIL_DESC {
                    DepthEnable: b(enable),
                    DepthWriteMask: write,
                    DepthFunc: D3D11_COMPARISON_LESS_EQUAL,
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
                depth: [depth(false, D3D11_DEPTH_WRITE_MASK_ZERO)?, depth(true, D3D11_DEPTH_WRITE_MASK_ALL)?, depth(true, D3D11_DEPTH_WRITE_MASK_ZERO)?],
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
                allocated: AtomicU64::new(0),
            }))
        }
    }

    /// The driver's budget and use of local video memory, when DXGI 1.4 reports it.
    pub fn budget(&self) -> Option<(u64, u64)> {
        let a = self.adapter3.as_ref()?;
        let info = unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL) }.ok()?;
        Some((info.Budget, info.CurrentUsage))
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
