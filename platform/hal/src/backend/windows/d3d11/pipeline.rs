//! Pipelines: the HLSL of a `PipelineCreate` compiled with D3DCompile on the compile worker
//! (DXBC cached on disk), its shaders, input layout and sampler states. Moved from
//! os/windows/d3d11.rs (`CxOsDrawShader::new`, `get_or_compile_shader_bytes`,
//! `d3d_compile_hlsl`, the DXBC cache); Cx's shader compiler made the HLSL, nothing is composed
//! or parsed here: the texture shapes and sampler states come with the payload.

use super::device::{hr, Device};
use crate::hlsl::HlslPipeline;
use crate::types::*;
use makepad_windows_sys::core::PCSTR;
use makepad_windows_sys::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// A compiled pipeline: everything a draw binds besides its resources.
pub struct Program {
    pub vs: ID3D11VertexShader,
    pub ps: ID3D11PixelShader,
    /// none for a shader without geometry inputs
    pub layout: Option<ID3D11InputLayout>,
    pub samplers: Vec<Option<ID3D11SamplerState>>,
    /// texture shapes in register order
    pub textures: Vec<u32>,
    /// `PipelineDesc::bindings`
    pub bindings: Vec<u32>,
    /// geometry vertex stride in bytes
    pub stride: u32,
    pub desc: PipelineDesc,
}

impl Program {
    pub fn slot(&self, which: usize) -> Option<u32> {
        self.bindings.get(which).copied().filter(|v| *v != pipeline_binding::NONE)
    }
    /// The t register of the instance words: after the textures (shader_ir_hlsl).
    pub fn instance_slot(&self) -> Option<u32> {
        (self.desc.instance_stride > 0).then_some(self.textures.len() as u32)
    }
}

/// What a program is compiled from, kept so a device reset can build it again on the new device.
pub struct PipeSrc {
    pub code: HlslPipeline,
    pub desc: PipelineDesc,
    pub bindings: Vec<u32>,
    pub layout: Vec<u32>,
    pub name: String,
}

impl PipeSrc {
    /// Pipelines that compile to the same program share it (the desc fields the program reads).
    pub fn same_program(&self, o: &PipeSrc) -> bool {
        self.code == o.code
            && self.bindings == o.bindings
            && self.layout == o.layout
            && (self.desc.blend, self.desc.instance_stride, self.desc.outputs) == (o.desc.blend, o.desc.instance_stride, o.desc.outputs)
    }
    /// The program map's key (collisions are told apart by `same_program`).
    pub fn hash_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.code.hash(&mut h);
        self.bindings.hash(&mut h);
        self.layout.hash(&mut h);
        (self.desc.blend, self.desc.instance_stride, self.desc.outputs).hash(&mut h);
        h.finish()
    }
}

/// Filled once by the compile worker; the backend reads it without waiting.
pub struct PipeState {
    pub src: Arc<PipeSrc>,
    pub program: OnceLock<Result<Arc<Program>, String>>,
    /// bumped when `program` is set (the backend announces Ready / Failed once per change)
    pub epoch: AtomicU64,
}

impl PipeState {
    pub fn new(src: Arc<PipeSrc>) -> Arc<Self> {
        Arc::new(Self { src, program: OnceLock::new(), epoch: AtomicU64::new(0) })
    }
    pub fn ready(&self) -> Option<&Arc<Program>> {
        self.program.get().and_then(|r| r.as_ref().ok())
    }
    pub fn failure(&self) -> Option<&str> {
        self.program.get().and_then(|r| r.as_ref().err()).map(|s| s.as_str())
    }
}

/// D3D11's input-layout semantic suffix of geometry input `index`: `A`..`Z`, then `AA`, `AB`..
/// The same scheme as the HLSL emitter's `index_to_semantic` (platform/shader shader_ir_draw.rs),
/// so the layout's names match what the vertex shader declares.
fn semantic_suffix(index: usize) -> String {
    const LETTERS: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut value = index;
    let mut out = Vec::new();
    loop {
        out.push(LETTERS[value % 26] as char);
        if value < 26 {
            break;
        }
        value = value / 26 - 1;
    }
    out.iter().rev().collect()
}

/// Chunks of an input's float lanes as HLSL expands it into semantics (matrices by column).
fn slot_chunks(slots: usize) -> Vec<usize> {
    match slots {
        0 => Vec::new(),
        9 => vec![3, 3, 3],
        16 => vec![4, 4, 4, 4],
        _ => {
            let mut rem = slots;
            let mut chunks = Vec::new();
            while rem > 0 {
                let chunk = rem.min(4);
                chunks.push(chunk);
                rem -= chunk;
            }
            chunks
        }
    }
}

/// The DXGI format of `lanes` of a geometry input in Cx's `DrawShaderAttrFormat` (by its
/// declaration order: F32x1..4, F16x2, F16x4, U16x2, I16x2, U16x2Norm, I16x2Norm, U8x4Norm,
/// I8x4Norm, U32x1, I32x1).
fn attr_format(format: u32, lanes: usize) -> Option<DXGI_FORMAT> {
    let lanes = lanes.clamp(1, 4);
    Some(match format {
        0..=3 => [DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT][lanes - 1],
        4 => DXGI_FORMAT_R16G16_FLOAT,
        5 => DXGI_FORMAT_R16G16B16A16_FLOAT,
        6 => DXGI_FORMAT_R16G16_UINT,
        7 => DXGI_FORMAT_R16G16_SINT,
        8 => DXGI_FORMAT_R16G16_UNORM,
        9 => DXGI_FORMAT_R16G16_SNORM,
        10 => DXGI_FORMAT_R8G8B8A8_UNORM,
        11 => DXGI_FORMAT_R8G8B8A8_SNORM,
        12 => [DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_UINT, DXGI_FORMAT_R32G32B32_UINT, DXGI_FORMAT_R32G32B32A32_UINT][lanes - 1],
        13 => [DXGI_FORMAT_R32_SINT, DXGI_FORMAT_R32G32_SINT, DXGI_FORMAT_R32G32B32_SINT, DXGI_FORMAT_R32G32B32A32_SINT][lanes - 1],
        _ => return None,
    })
}

fn cache_dir() -> Option<&'static std::path::Path> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("makepad").join("d3d11_shader_cache");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    })
    .as_deref()
}

/// FNV-1a of the source, seeded with a version that changes with the compile flags/entries.
fn cache_key(hlsl: &str) -> u64 {
    const VERSION: u8 = 3;
    let mut hash: u64 = 0xcbf29ce484222325;
    hash ^= VERSION as u64;
    hash = hash.wrapping_mul(0x100000001b3);
    for byte in hlsl.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A complete DXBC container: `DXBC`, digest, version, its own total size. The cache is shared
/// by processes, so a file can be another's partial write.
fn is_complete_dxbc(bytes: &[u8]) -> bool {
    if bytes.len() < 32 || !bytes.starts_with(b"DXBC") {
        return false;
    }
    u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize == bytes.len()
}

/// Writes a temporary file and renames it into place: readers see all of an entry or none.
fn publish(path: &std::path::Path, bytes: &[u8]) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// One stage through FXC. Optimisation is skipped: FXC's optimiser makes UI shaders take
/// seconds; the gain at run time is small (the old backend's measured choice).
fn compile_stage(target: &str, entry: &str, hlsl: &str) -> Result<Vec<u8>, String> {
    const SKIP_OPTIMIZATION: u32 = 1 << 2;
    const BACKWARDS_COMPATIBILITY: u32 = 1 << 12;
    let target = format!("{target}\0");
    let entry = format!("{entry}\0");
    unsafe {
        let mut blob = None;
        let mut errors = None;
        let result = D3DCompile(
            hlsl.as_ptr() as *const _,
            hlsl.len(),
            Some(PCSTR(b"makepad_shader\0".as_ptr())),
            None,
            None,
            Some(PCSTR(entry.as_ptr())),
            PCSTR(target.as_ptr()),
            SKIP_OPTIMIZATION | BACKWARDS_COMPATIBILITY,
            0,
            &mut blob,
            Some(&mut errors),
        );
        if result.is_ok() {
            if let Some(blob) = blob {
                return Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec());
            }
        }
        let log = errors.map(|e| String::from_utf8_lossy(std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize())).into_owned());
        Err(log.unwrap_or_else(|| format!("D3DCompile {}: {:?}", target.trim_end_matches('\0'), result.err())))
    }
}

fn stage_bytes(key: u64, suffix: &str, target: &str, entry: &str, hlsl: &str) -> Result<Vec<u8>, String> {
    if let Some(dir) = cache_dir() {
        let path = dir.join(format!("{key:016x}{suffix}.dxbc"));
        if let Ok(bytes) = std::fs::read(&path) {
            if is_complete_dxbc(&bytes) {
                return Ok(bytes);
            }
        }
        let bytes = compile_stage(target, entry, hlsl)?;
        publish(&path, &bytes);
        return Ok(bytes);
    }
    compile_stage(target, entry, hlsl)
}

/// Line-numbered source for a compile error.
fn numbered(src: &str) -> String {
    src.lines().enumerate().map(|(i, l)| format!("{}:{l}\n", i + 1)).collect()
}

/// E_OUTOFMEMORY
const E_OUTOFMEMORY: makepad_windows_sys::core::HRESULT = makepad_windows_sys::core::HRESULT(0x8007000Eu32 as i32);

/// Compiles and creates a pipeline (any thread: no immediate-context work).
pub fn build(dev: &Device, code: &HlslPipeline, desc: PipelineDesc, bindings: Vec<u32>, vertex_layout: Vec<u32>, name: &str) -> Result<Arc<Program>, String> {
    let hlsl = &code.source;
    let key = cache_key(hlsl);
    // `MAKEPAD_HLSL_DUMP=<dir>`: every pipeline's source, by its cache key (compile diagnosis)
    if let Some(dir) = std::env::var_os("MAKEPAD_HLSL_DUMP") {
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{key:016x}.hlsl")), hlsl);
    }
    let mut vs_bytes = stage_bytes(key, "_vs", "vs_5_0", "vertex_main", hlsl).map_err(|e| format!("vertex shader `{name}`:\n{e}\n{}", numbered(hlsl)))?;
    let mut ps_bytes = stage_bytes(key, "_ps", "ps_5_0", "pixel_main", hlsl).map_err(|e| format!("pixel shader `{name}`:\n{e}\n{}", numbered(hlsl)))?;
    let create = |vs_bytes: &[u8], ps_bytes: &[u8]| unsafe {
        let (mut vs, mut ps) = (None, None);
        dev.device.CreateVertexShader(vs_bytes, None, Some(&mut vs)).and_then(|_| dev.device.CreatePixelShader(ps_bytes, None, Some(&mut ps))).map(|_| (vs, ps))
    };
    let (vs, ps) = match create(&vs_bytes, &ps_bytes) {
        Ok(made) => made,
        // a removed device or exhausted memory is not the bytecode's fault
        Err(e) if dev.check_lost() || e == E_OUTOFMEMORY => return Err(hr("CreateVertexShader/CreatePixelShader", e)),
        Err(e) => {
            // a live device rejected the bytecode itself (a stale or foreign cache entry):
            // drop the entries and compile once more without the cache
            eprintln!("[hal-d3d11] the device rejected cached bytecode of `{name}` ({e}); recompiling");
            if let Some(dir) = cache_dir() {
                for suffix in ["_vs", "_ps"] {
                    let _ = std::fs::remove_file(dir.join(format!("{key:016x}{suffix}.dxbc")));
                }
            }
            vs_bytes = compile_stage("vs_5_0", "vertex_main", hlsl).map_err(|e| format!("vertex shader `{name}`:\n{e}\n{}", numbered(hlsl)))?;
            ps_bytes = compile_stage("ps_5_0", "pixel_main", hlsl).map_err(|e| format!("pixel shader `{name}`:\n{e}\n{}", numbered(hlsl)))?;
            create(&vs_bytes, &ps_bytes).map_err(|e| hr("CreateVertexShader/CreatePixelShader of freshly compiled bytecode", e))?
        }
    };
    // geometry inputs: stride in bytes, then (byte offset, format, lane count) each
    let stride = vertex_layout.first().copied().unwrap_or(0);
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut elements: Vec<(usize, u32, DXGI_FORMAT, u32)> = Vec::new();
    for (i, input) in vertex_layout.get(1..).unwrap_or(&[]).chunks_exact(3).enumerate() {
        let (offset, fmt, lanes) = (input[0], input[1], input[2] as usize);
        names.push(format!("GEOM{}\0", semantic_suffix(i)).into_bytes());
        let mut lane = 0usize;
        for (chunk_index, chunk) in slot_chunks(lanes).into_iter().enumerate() {
            let f = attr_format(fmt, chunk).ok_or_else(|| format!("geometry input {i}: unknown attribute format {fmt}"))?;
            elements.push((names.len() - 1, chunk_index as u32, f, offset + lane as u32 * 4));
            lane += chunk;
        }
    }
    let layout_desc: Vec<D3D11_INPUT_ELEMENT_DESC> = elements
        .iter()
        .map(|(n, index, f, offset)| D3D11_INPUT_ELEMENT_DESC {
            SemanticName: PCSTR(names[*n].as_ptr()),
            SemanticIndex: *index,
            Format: *f,
            InputSlot: 0,
            AlignedByteOffset: *offset,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        })
        .collect();
    let layout = if layout_desc.is_empty() {
        None
    } else {
        let mut out = None;
        unsafe { dev.device.CreateInputLayout(&layout_desc, &vs_bytes, Some(&mut out)) }
            .map_err(|e| format!("CreateInputLayout `{name}` ({} elements, stride {stride}): {e}", layout_desc.len()))?;
        out
    };
    let mut samplers = Vec::new();
    for s in &code.samplers {
        let address = match s.address {
            0 => D3D11_TEXTURE_ADDRESS_WRAP,
            1 => D3D11_TEXTURE_ADDRESS_CLAMP,
            2 => D3D11_TEXTURE_ADDRESS_BORDER,
            _ => D3D11_TEXTURE_ADDRESS_MIRROR,
        };
        let filter = match (s.filter, s.compare) {
            (0, false) => D3D11_FILTER_MIN_MAG_MIP_POINT,
            (_, false) => D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            (0, true) => D3D11_FILTER_COMPARISON_MIN_MAG_MIP_POINT,
            (_, true) => D3D11_FILTER_COMPARISON_MIN_MAG_MIP_LINEAR,
        };
        let d = D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: address,
            AddressV: address,
            AddressW: address,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: if s.compare { D3D11_COMPARISON_LESS_EQUAL } else { D3D11_COMPARISON_NEVER },
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        };
        let mut out = None;
        unsafe { dev.device.CreateSamplerState(&d, Some(&mut out)) }.map_err(|e| hr("CreateSamplerState", e))?;
        samplers.push(out);
    }
    if code.textures.len() + desc.instance_stride.min(1) as usize > D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT as usize {
        return Err(format!("`{name}` binds {} textures; D3D11 has {} shader-resource slots", code.textures.len(), D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT));
    }
    if samplers.len() > D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT as usize {
        return Err(format!("`{name}` declares {} samplers; D3D11 has {}", samplers.len(), D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT));
    }
    Ok(Arc::new(Program {
        vs: vs.ok_or("CreateVertexShader returned nothing")?,
        ps: ps.ok_or("CreatePixelShader returned nothing")?,
        layout,
        samplers,
        textures: code.textures.clone(),
        bindings,
        stride,
        desc,
    }))
}
