//! Official audio.cpp MiniMax-Music3 GGUF pack.
//!
//! These files are `general.architecture = audiocpp` with native HF tensor
//! names (`model.layers.N.self_attn.q_proj`, `transformer_blocks.N.attn.to_q`).
//! Do not remap them through Flux or H3 GGUF maps.
//!
//! Default mix (audio.cpp README): Q4_0 LM + Q4_0 DiT + BF16 RVQ + F32
//! condition encoder + F32 vocoder. Sidecar `tokenizer/tokenizer.json` is
//! the vocab — the GGUFs have no `tokenizer.ggml.*`.

use crate::music3::{
    expected_shape_canaries, Music3ConditionEncoder, MUSIC3_COND_HIDDEN, MUSIC3_COND_LAYERS,
    MUSIC3_COND_OUT, MUSIC3_DIT_LAYERS, MUSIC3_LM_LAYERS,
};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::{
    block_size, f32_to_f16, get_rows_ggml_bytes_cpu, vec_dot_q4_0_f32, QK,
};
use makepad_ggml::tensor::{ggml_row_size_for_type, TensorType};
use makepad_llama::GgufFile;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Per-tensor packed-byte cache cap. Layer Q4/BF16 weights fit; embed/lm_head do not.
const BYTE_CACHE_MAX: usize = 64 * 1024 * 1024;

fn music3_trace(f: impl FnOnce()) {
    if std::env::var_os("MAKEPAD_MUSIC3_TRACE").is_some() {
        f();
    }
}

fn music3_log_first_linear(path: &str, name: &str, ty: TensorType, m: usize, k: usize, n: usize) {
    use std::sync::OnceLock;
    static METAL: OnceLock<()> = OnceLock::new();
    static METAL_MULTI: OnceLock<()> = OnceLock::new();
    static GEMV: OnceLock<()> = OnceLock::new();
    let slot = if path == "metal" {
        &METAL
    } else if path == "metal_keyed_multi" {
        &METAL_MULTI
    } else {
        &GEMV
    };
    slot.get_or_init(|| {
        eprintln!("[music3] first linear {path} {ty:?} {name} m={m} k={k} n={n}");
    });
}

pub const MUSIC3_GGUF_ARCH: &str = "audiocpp";
pub const MUSIC3_GGUF_LM: &str = "language_model_q4_0.gguf";
pub const MUSIC3_GGUF_RVQ: &str = "rvq_depth_decoder_bf16.gguf";
pub const MUSIC3_GGUF_DIT: &str = "transformer_q4_0.gguf";
pub const MUSIC3_GGUF_COND: &str = "condition_encoder.gguf";
pub const MUSIC3_GGUF_VOC: &str = "vocoder.gguf";

const EXPECTED_TENSORS: &[(&str, usize)] = &[
    ("condition", 4),
    ("vocoder", 121),
    ("transformer", 441),
    ("rvq", 47),
    ("language_model", 399),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Music3GgufRole {
    LanguageModel,
    Rvq,
    Transformer,
    Condition,
    Vocoder,
}

impl Music3GgufRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LanguageModel => "language_model",
            Self::Rvq => "rvq",
            Self::Transformer => "transformer",
            Self::Condition => "condition",
            Self::Vocoder => "vocoder",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Music3GgufPaths {
    pub language_model: PathBuf,
    pub rvq: PathBuf,
    pub transformer: PathBuf,
    pub condition: PathBuf,
    pub vocoder: PathBuf,
    pub tokenizer: PathBuf,
}

impl Music3GgufPaths {
    /// audio.cpp default Q4 mix under `dir`.
    pub fn default_mix(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let lm = std::env::var("MAKEPAD_MUSIC3_LM")
            .map(|n| dir.join(n))
            .ok()
            .filter(|p| p.exists())
            .or_else(|| {
                for name in [
                    "lm_q8_0.gguf",
                    "language_model_q8_0.gguf",
                    "language_model_q4_k.gguf",
                ] {
                    let p = dir.join(name);
                    if p.exists() {
                        return Some(p);
                    }
                }
                None
            })
            .unwrap_or_else(|| dir.join(MUSIC3_GGUF_LM));
        Self {
            language_model: lm,
            rvq: dir.join(MUSIC3_GGUF_RVQ),
            transformer: dir.join(MUSIC3_GGUF_DIT),
            condition: dir.join(MUSIC3_GGUF_COND),
            vocoder: dir.join(MUSIC3_GGUF_VOC),
            tokenizer: dir.join("tokenizer"),
        }
    }
}

pub struct Music3GgufFile {
    pub role: Music3GgufRole,
    pub path: PathBuf,
    pub architecture: String,
    pub family: String,
    pub name_format: String,
    pub file: GgufFile,
    byte_cache: RefCell<HashMap<String, Arc<Vec<u8>>>>,
    bf16_as_f16: RefCell<HashMap<String, Arc<Vec<u8>>>>,
    io: RefCell<File>,
}

impl Music3GgufFile {
    pub fn open(role: Music3GgufRole, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = GgufFile::open(&path).map_err(|err| {
            DiffusionError::model(format!("music3 gguf {}: {err}", path.display()))
        })?;
        let architecture = kv_string(&file, "general.architecture");
        if architecture != MUSIC3_GGUF_ARCH {
            return Err(DiffusionError::model(format!(
                "music3 gguf {}: architecture {architecture:?}, expected {MUSIC3_GGUF_ARCH}",
                path.display()
            )));
        }
        let family = kv_string(&file, "audiocpp.model_spec.family");
        let name_format = kv_string(&file, "audiocpp.tensor_name_format");
        if !name_format.is_empty() && name_format != "native" {
            return Err(DiffusionError::model(format!(
                "music3 gguf {}: tensor_name_format {name_format:?}, expected native",
                path.display()
            )));
        }
        let io = File::open(&path).map_err(|err| {
            DiffusionError::model(format!("music3 gguf {}: {err}", path.display()))
        })?;
        Ok(Self {
            role,
            path,
            architecture,
            family,
            name_format,
            file,
            byte_cache: RefCell::new(HashMap::new()),
            bf16_as_f16: RefCell::new(HashMap::new()),
            io: RefCell::new(io),
        })
    }

    pub fn tensor_count(&self) -> usize {
        self.file.tensors.len()
    }

    pub fn dtype_counts(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(TensorType, usize)> = Vec::new();
        for tensor in &self.file.tensors {
            if let Some(slot) = counts.iter_mut().find(|(ty, _)| *ty == tensor.tensor_type) {
                slot.1 += 1;
            } else {
                counts.push((tensor.tensor_type, 1));
            }
        }
        counts
            .into_iter()
            .map(|(ty, n)| (format!("{ty:?}"), n))
            .collect()
    }

    pub fn require(&self, name: &str) -> Result<&makepad_llama::GgufTensorInfo> {
        self.file.get_tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "music3 {} missing tensor {name}",
                self.role.as_str()
            ))
        })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.file.get_tensor(name).is_some()
    }

    pub fn read_bytes(&self, name: &str) -> Result<Vec<u8>> {
        Ok((*self.bytes_arc(name)?).clone())
    }

    /// Drop packed-weight copies so a later stage can reuse the 16 GB Mac.
    pub fn clear_byte_cache(&self) {
        self.byte_cache.borrow_mut().clear();
        self.bf16_as_f16.borrow_mut().clear();
    }

    fn byte_cache_limit() -> usize {
        std::env::var("MAKEPAD_MUSIC3_BYTE_CACHE_MAX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(BYTE_CACHE_MAX)
    }

    fn bytes_arc(&self, name: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.byte_cache.borrow().get(name) {
            return Ok(Arc::clone(hit));
        }
        let arc = Arc::new(self.read_bytes_uncached(name)?);
        if arc.len() <= Self::byte_cache_limit() {
            self.byte_cache
                .borrow_mut()
                .insert(name.to_string(), Arc::clone(&arc));
        }
        Ok(arc)
    }

    /// One-shot read that never lands in the heap byte cache. Use for
    /// GPU-named-buffer uploads and decode-once reads: dead heap twins of the
    /// 8.4 GB weight set were committing ~16 GB on the 16 GB M4 and the
    /// compressor churn throttled prefill pass 2 and early decode
    /// (q8_8fable5 pf-l0: pass1 ops at GEMM floor, pass2 2.5x slower).
    /// File-backed page-cache reads drop for free; heap copies do not.
    pub fn read_bytes_uncached(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(hit) = self.byte_cache.borrow().get(name) {
            return Ok((**hit).clone());
        }
        self.require(name)?;
        self.file
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("music3 read {name}: {err}")))
    }

    /// BF16 → F16 conversion without retaining either copy (upload-once path).
    fn bf16_as_f16_uncached(&self, name: &str) -> Option<Vec<u8>> {
        if let Some(hit) = self.bf16_as_f16.borrow().get(name) {
            return Some((**hit).clone());
        }
        let bf = self.read_bytes_uncached(name).ok()?;
        if bf.len() % 2 != 0 {
            return None;
        }
        let mut f16 = vec![0u8; bf.len()];
        for (i, chunk) in bf.chunks_exact(2).enumerate() {
            let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
            let f = f32::from_bits((bits as u32) << 16);
            let h = f32_to_f16(f);
            f16[i * 2..i * 2 + 2].copy_from_slice(&h.to_le_bytes());
        }
        Some(f16)
    }

    pub fn read_f32(&self, name: &str) -> Result<Vec<f32>> {
        let info = self.require(name)?;
        if info.tensor_type != TensorType::F32 {
            return Err(DiffusionError::model(format!(
                "music3 {name}: type {:?}, expected F32",
                info.tensor_type
            )));
        }
        let bytes = self.read_bytes_uncached(name)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    /// Decode F32 / F16 / BF16 / Q4_0 to host f32 (ggml row-major).
    /// Refuses tensors whose storage is larger than 256 MiB so the 16 GB Mac
    /// never materializes `embed_tokens` / `lm_head`.
    pub fn read_f32_any(&self, name: &str) -> Result<Vec<f32>> {
        let info = self.require(name)?;
        if info.size_bytes > 256 * 1024 * 1024 {
            return Err(DiffusionError::model(format!(
                "music3 {name}: {} bytes is too large for read_f32_any (use gather_rows)",
                info.size_bytes
            )));
        }
        decode_ggml_f32(
            &self.read_bytes_uncached(name)?,
            info.tensor_type,
            &info.dimensions,
        )
        .map_err(|err| DiffusionError::model(format!("music3 {name}: {err}")))
    }

    /// Byte length of one ggml row (ne0).
    pub fn row_bytes(&self, name: &str) -> Result<usize> {
        let info = self.require(name)?;
        let n_cols = *info.dimensions.first().unwrap_or(&1);
        ggml_row_size_for_type(info.tensor_type, n_cols as i64)
            .map_err(|err| DiffusionError::model(format!("music3 {name}: {err}")))
    }

    /// Seek-read one ggml row without mapping the whole tensor.
    pub fn read_row_bytes(&self, name: &str, row: u64) -> Result<Vec<u8>> {
        let info = self.require(name)?;
        let n_rows = info
            .dimensions
            .iter()
            .skip(1)
            .map(|&d| d as u64)
            .product::<u64>()
            .max(1);
        if row >= n_rows {
            return Err(DiffusionError::model(format!(
                "music3 {name}: row {row} >= {n_rows}"
            )));
        }
        let row_bytes = self.row_bytes(name)?;
        let start = info
            .absolute_offset(self.file.data_offset)
            .map_err(|err| DiffusionError::model(format!("music3 {name}: {err}")))?
            + row * row_bytes as u64;
        let mut file = self.io.borrow_mut();
        file.seek(SeekFrom::Start(start)).map_err(|err| {
            DiffusionError::model(format!("music3 {name} seek {row}: {err}"))
        })?;
        let mut buf = vec![0u8; row_bytes];
        file.read_exact(&mut buf).map_err(|err| {
            DiffusionError::model(format!("music3 {name} row {row}: {err}"))
        })?;
        Ok(buf)
    }

    /// Gather selected rows as f32. Works for F32 / F16 / BF16 / Q4_0.
    /// Tables under the byte-cache cap are sliced from RAM (the RVQ decode
    /// loop gathers 13 embedding rows per frame — seek+read per row was
    /// ~0.7s per generate); bigger tables (LM embed) stay seek-per-row.
    pub fn gather_rows(&self, name: &str, ids: &[u32]) -> Result<Vec<f32>> {
        let info = self.require(name)?;
        let n_cols = *info.dimensions.first().unwrap_or(&1) as usize;
        let n_rows = info
            .dimensions
            .iter()
            .skip(1)
            .map(|&d| d as usize)
            .product::<usize>()
            .max(1);
        let row_bytes = self.row_bytes(name)?;
        for &id in ids {
            if id as usize >= n_rows {
                return Err(DiffusionError::model(format!(
                    "music3 {name}: token {id} >= rows {n_rows}"
                )));
            }
        }
        let mut packed = vec![0u8; ids.len() * row_bytes];
        if info.size_bytes as usize <= Self::byte_cache_limit() {
            let bytes = self.bytes_arc(name)?;
            for (i, &id) in ids.iter().enumerate() {
                let off = id as usize * row_bytes;
                packed[i * row_bytes..(i + 1) * row_bytes]
                    .copy_from_slice(&bytes[off..off + row_bytes]);
            }
        } else {
            let start = info
                .absolute_offset(self.file.data_offset)
                .map_err(|err| DiffusionError::model(format!("music3 {name}: {err}")))?;
            let mut file = self.io.borrow_mut();
            for (i, &id) in ids.iter().enumerate() {
                file.seek(SeekFrom::Start(start + id as u64 * row_bytes as u64))
                    .map_err(|err| {
                        DiffusionError::model(format!("music3 {name} seek {id}: {err}"))
                    })?;
                file.read_exact(&mut packed[i * row_bytes..(i + 1) * row_bytes])
                    .map_err(|err| {
                        DiffusionError::model(format!("music3 {name} row {id}: {err}"))
                    })?;
            }
        }
        let indices: Vec<i32> = (0..ids.len() as i32).collect();
        get_rows_ggml_bytes_cpu(
            &packed,
            info.tensor_type.ggml_type(),
            n_cols,
            ids.len(),
            &indices,
        )
        .ok_or_else(|| {
            DiffusionError::model(format!(
                "music3 {name}: decode {:?} rows failed (cols={n_cols})",
                info.tensor_type
            ))
        })
    }

    /// GGUF ggml order: `[k, n]` (ne0 quantized / innermost).
    pub fn linear_kn(&self, name: &str) -> Result<(usize, usize, TensorType)> {
        let info = self.require(name)?;
        let dims: Vec<usize> = info
            .dimensions
            .iter()
            .map(|&d| d as usize)
            .filter(|&d| d > 1)
            .collect();
        if dims.len() != 2 {
            return Err(DiffusionError::model(format!(
                "music3 {name}: rank {:?} (squeezed {:?}), expected 2",
                info.dimensions, dims
            )));
        }
        Ok((dims[0], dims[1], info.tensor_type))
    }

    /// `C[m, n] = A[m, k] * W[n, k]` via official Metal `mul_mm` / `mul_mv`.
    ///
    /// Decode uses native `m=1` so Q4_0 hits `kernel_mul_mv_q4_0_f32`. Do not
    /// pad to 32: that selected `mul_mm` and produced all-zero outputs.
    /// If Metal returns zeros, Q4_0 falls back to packed `vec_dot` GEMV.
    pub fn linear_nt(&self, name: &str, a: &[f32], m: usize) -> Result<Vec<f32>> {
        let info = self.require(name)?;
        let (k, n, ty) = self.linear_kn(name)?;
        if a.len() != m.saturating_mul(k) {
            return Err(DiffusionError::model(format!(
                "music3 {name}: lhs {} values, expected {} (m={m} k={k})",
                a.len(),
                m * k
            )));
        }
        if info.dimensions.len() != 2 {
            return self.linear_nt_host_f32(name, a, m, k, n);
        }
        // Native `m` — do not pad decode (m=1) to 32. Padding forced
        // `kernel_mul_mm_q4_0_f32`, which wrote zeros on the older metallib.
        // m=1 hits official `kernel_mul_mv_q4_0_f32`.
        // Skip native BF16: this metallib has no kernel_mul_mv_bf16_*.
        let skip_metal = std::env::var_os("MAKEPAD_MUSIC3_CPU_LINEAR").is_some();
        // Q4 mul_mm on the 104-token LM prefill accumulates enough error that
        // RVQ head-0 ranks 996 instead of official 271. Prefill dequants once
        // and uses F32 Metal GEMM. Decode (m=1) stays on Q4 mul_mv.
        let skip_q4_mm_prefill = self.role == Music3GgufRole::LanguageModel
            && ty == TensorType::Q4_0
            && m >= 8
            && std::env::var_os("MAKEPAD_MUSIC3_F32_PREFILL").is_some();
        if !skip_metal && ty != TensorType::BF16 && !skip_q4_mm_prefill {
            if let Some(out) = self.try_metal_quant_linear(name, a, m, k, n, ty) {
                if out.iter().any(|v| v.is_finite() && *v != 0.0) {
                    music3_log_first_linear("metal", name, ty, m, k, n);
                    return Ok(out);
                }
                music3_trace(|| {
                    eprintln!("[music3] metal {name} {ty:?} m={m} returned zeros; fallback")
                });
            }
        }
        // Compiled metallib has no BF16 mul_mv/mul_mm. F16 kernels exist.
        if ty == TensorType::BF16 {
            if let Some(out) = self.try_metal_bf16_as_f16(name, a, m, k, n) {
                if out.iter().any(|v| v.is_finite() && *v != 0.0) {
                    music3_log_first_linear("metal_f16", name, ty, m, k, n);
                    return Ok(out);
                }
            }
        }
        // GEMV is for decode (m=1). Prefill (m=104) dequants the weight once
        // and reuses it; 104 serial GEMVs re-read the packed rows each time.
        if ty == TensorType::Q4_0 && m <= 4 {
            if let Some(out) = self.q4_0_gemv(name, a, m, k, n) {
                return Ok(out);
            }
        }
        self.linear_nt_dequant(name, a, m, k, n, ty)
    }

    /// One activation upload, several named-weight GEMMs, one wait. Same kernels
    /// as [`Self::linear_nt`]. Falls back to sequential `linear_nt` on miss.
    pub fn linear_nt_multi(&self, names: &[&str], a: &[f32], m: usize) -> Result<Vec<Vec<f32>>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        if names.len() == 1 {
            return Ok(vec![self.linear_nt(names[0], a, m)?]);
        }
        if let Some(out) = self.try_metal_quant_linear_multi(names, a, m) {
            if out
                .iter()
                .all(|y| y.iter().any(|v| v.is_finite() && *v != 0.0))
            {
                if let Ok((k, n, ty)) = self.linear_kn(names[0]) {
                    music3_log_first_linear("metal_keyed_multi", names[0], ty, m, k, n);
                }
                return Ok(out);
            }
        }
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            out.push(self.linear_nt(name, a, m)?);
        }
        Ok(out)
    }

    pub fn linear_nt_qkv(
        &self,
        q: &str,
        k: &str,
        v: &str,
        a: &[f32],
        m: usize,
    ) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let mut out = self.linear_nt_multi(&[q, k, v], a, m)?;
        let v = out.pop().expect("qkv v");
        let k = out.pop().expect("qkv k");
        let q = out.pop().expect("qkv q");
        Ok((q, k, v))
    }

    pub fn linear_nt_up_gate(
        &self,
        up: &str,
        gate: &str,
        a: &[f32],
        m: usize,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let mut out = self.linear_nt_multi(&[up, gate], a, m)?;
        let gate = out.pop().expect("up_gate gate");
        let up = out.pop().expect("up_gate up");
        Ok((up, gate))
    }

    pub fn keyed_mat<'a>(
        &'a self,
        ns: &'a str,
        name: &'a str,
    ) -> Option<makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix<'a>> {
        let (_k, n, ty) = self.linear_kn(name).ok()?;
        if ty == TensorType::BF16 {
            return None;
        }
        Some(
            makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix {
                bt_ggml_type: ty.ggml_type(),
                n,
                namespace: ns,
                cache_key: name,
            },
        )
    }

    /// Keyed matrix that also serves BF16 weights through the F16 sidecar
    /// namespace (`load_keyed_bytes` converts on first touch).
    pub fn keyed_mat_any<'a>(
        &self,
        ns: &'a str,
        ns_f16: &'a str,
        name: &'a str,
    ) -> Option<makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix<'a>> {
        let (_k, n, ty) = self.linear_kn(name).ok()?;
        Some(if ty == TensorType::BF16 {
            makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix {
                bt_ggml_type: TensorType::F16.ggml_type(),
                n,
                namespace: ns_f16,
                cache_key: name,
            }
        } else {
            makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix {
                bt_ggml_type: ty.ggml_type(),
                n,
                namespace: ns,
                cache_key: name,
            }
        })
    }

    pub fn load_keyed_bytes(&self, namespace: &str, key: &str) -> std::result::Result<Vec<u8>, String> {
        if namespace.ends_with("-f16") {
            self.bf16_as_f16_uncached(key)
                .ok_or_else(|| format!("music3 bf16-as-f16 {key}"))
        } else {
            self.read_bytes_uncached(key).map_err(|err| err.to_string())
        }
    }

    fn try_metal_quant_linear_multi(
        &self,
        names: &[&str],
        a: &[f32],
        m: usize,
    ) -> Option<Vec<Vec<f32>>> {
        if std::env::var_os("MAKEPAD_MUSIC3_CPU_LINEAR").is_some() {
            return None;
        }
        let ns = format!("music3-{}", self.role.as_str());
        let ns_f16 = format!("music3-{}-f16", self.role.as_str());
        let mut specs: Vec<(&str, u32, usize, bool)> = Vec::with_capacity(names.len());
        let mut k0 = 0usize;
        for name in names {
            let (k, n, ty) = self.linear_kn(name).ok()?;
            if a.len() != m.saturating_mul(k) {
                return None;
            }
            if k0 == 0 {
                k0 = k;
            } else if k != k0 {
                return None;
            }
            if ty == TensorType::BF16 {
                specs.push((*name, TensorType::F16.ggml_type(), n, true));
            } else {
                specs.push((*name, ty.ggml_type(), n, false));
            }
        }
        let mats: Vec<makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix<'_>> = specs
            .iter()
            .map(|(name, ty, n, is_f16)| makepad_ggml::backend::metal::MatmulNtGgmlBytesKeyedMatrix {
                bt_ggml_type: *ty,
                n: *n,
                namespace: if *is_f16 { ns_f16.as_str() } else { ns.as_str() },
                cache_key: name,
            })
            .collect();
        makepad_ggml::backend::metal::try_matmul_nt_ggml_bytes_keyed_multi(
            a,
            m,
            k0,
            &mats,
            |namespace, key| self.load_keyed_bytes(namespace, key),
        )
    }

    fn linear_nt_host_f32(
        &self,
        name: &str,
        a: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Result<Vec<f32>> {
        let w = self.read_f32_any(name)?;
        if w.len() != n.saturating_mul(k) {
            return Err(DiffusionError::model(format!(
                "music3 {name}: dense weight {} values, expected {}",
                w.len(),
                n * k
            )));
        }
        match makepad_ggml::backend::metal::try_matmul_nt_f32(a, &w, m, k, n) {
            Some(y) if y.iter().any(|v| v.is_finite() && *v != 0.0) => Ok(y),
            _ => matmul_nt_f32_cpu(a, &w, m, k, n),
        }
    }

    fn try_metal_quant_linear(
        &self,
        name: &str,
        a: &[f32],
        m: usize,
        k: usize,
        n: usize,
        ty: TensorType,
    ) -> Option<Vec<f32>> {
        let ggml_type = ty.ggml_type();
        let ns = format!("music3-{}", self.role.as_str());
        let load = || self.read_bytes_uncached(name).map_err(|err| err.to_string());
        makepad_ggml::backend::metal::try_matmul_nt_ggml_bytes_keyed(
            a, ggml_type, m, k, n, &ns, name, load,
        )
    }

    fn try_metal_bf16_as_f16(
        &self,
        name: &str,
        a: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        let ns = format!("music3-{}-f16", self.role.as_str());
        let load = || {
            self.bf16_as_f16_uncached(name)
                .ok_or_else(|| format!("music3 bf16-as-f16 {name}"))
        };
        makepad_ggml::backend::metal::try_matmul_nt_ggml_bytes_keyed(
            a,
            TensorType::F16.ggml_type(),
            m,
            k,
            n,
            &ns,
            name,
            load,
        )
    }

    /// Packed Q4_0 × f32 without materializing the f32 matrix. Used when Metal
    /// Q4 decode is unavailable. Parallel over output rows.
    fn q4_0_gemv(
        &self,
        name: &str,
        a: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if k == 0 || k % QK != 0 {
            return None;
        }
        let packed = self.bytes_arc(name).ok()?;
        let row_bytes = ggml_row_size_for_type(TensorType::Q4_0, k as i64).ok()?;
        if packed.len() != n.saturating_mul(row_bytes) {
            return None;
        }
        let bs = block_size(TensorType::Q4_0.ggml_type());
        let nb = k / QK;
        let mut out = vec![0f32; m.saturating_mul(n)];
        for r in 0..m {
            let x = &a[r * k..(r + 1) * k];
            let dst = &mut out[r * n..(r + 1) * n];
            q4_0_gemv_rows(&packed, x, n, row_bytes, nb, bs, dst);
        }
        if out.iter().any(|v| v.is_finite() && *v != 0.0) {
            music3_log_first_linear("q4_gemv", name, TensorType::Q4_0, m, k, n);
            Some(out)
        } else {
            None
        }
    }

    fn linear_nt_dequant(
        &self,
        name: &str,
        a: &[f32],
        m: usize,
        k: usize,
        n: usize,
        ty: TensorType,
    ) -> Result<Vec<f32>> {
        const MAX_F32: usize = 8 * 1024 * 1024;
        let chunk_n = (MAX_F32 / k.max(1)).max(1).min(n);
        let row_bytes = ggml_row_size_for_type(ty, k as i64).map_err(|err| {
            DiffusionError::model(format!("music3 {name}: row size k={k}: {err}"))
        })?;
        let cached = self.bytes_arc(name).ok().filter(|b| b.len() == n * row_bytes);
        let mut file_guard;
        let mut disk: Option<&mut File> = None;
        let mut start = 0u64;
        if cached.is_none() {
            let info = self.require(name)?;
            start = info
                .absolute_offset(self.file.data_offset)
                .map_err(|err| DiffusionError::model(format!("music3 {name}: {err}")))?;
            file_guard = self.io.borrow_mut();
            disk = Some(&mut *file_guard);
        }
        let mut out = vec![0f32; m * n];
        let mut packed = vec![0u8; chunk_n * row_bytes];
        for row0 in (0..n).step_by(chunk_n) {
            let take = (n - row0).min(chunk_n);
            let nbytes = take * row_bytes;
            let src = if let Some(bytes) = cached.as_ref() {
                &bytes[row0 * row_bytes..row0 * row_bytes + nbytes]
            } else {
                let file = disk.as_mut().unwrap();
                file.seek(SeekFrom::Start(start + row0 as u64 * row_bytes as u64))
                    .map_err(|err| DiffusionError::model(format!("music3 {name} seek: {err}")))?;
                file.read_exact(&mut packed[..nbytes])
                    .map_err(|err| DiffusionError::model(format!("music3 {name} rows: {err}")))?;
                &packed[..nbytes]
            };
            let indices: Vec<i32> = (0..take as i32).collect();
            let w = get_rows_ggml_bytes_cpu(src, ty.ggml_type(), k, take, &indices).ok_or_else(
                || {
                    DiffusionError::model(format!(
                        "music3 {name}: dequant {:?} strip {row0}+{take}",
                        ty
                    ))
                },
            )?;
            let y = match makepad_ggml::backend::metal::try_matmul_nt_f32(a, &w, m, k, take) {
                Some(y) if y.iter().any(|v| v.is_finite() && *v != 0.0) => y,
                _ => matmul_nt_f32_cpu(a, &w, m, k, take)?,
            };
            for r in 0..m {
                out[r * n + row0..r * n + row0 + take]
                    .copy_from_slice(&y[r * take..(r + 1) * take]);
            }
        }
        if !out.iter().any(|v| v.is_finite() && *v != 0.0) && a.iter().any(|v| *v != 0.0) {
            return Err(DiffusionError::model(format!(
                "music3 {name}: dequant gemm still all-zero (m={m} k={k} n={n} {:?})",
                ty
            )));
        }
        Ok(out)
    }

    /// Gather rows of an F16 embedding table stored `[hidden, vocab]`.
    /// Seeks per id — does not map the 1.6 GiB table.
    pub fn gather_f16_rows(&self, name: &str, ids: &[u32]) -> Result<Vec<f32>> {
        let info = self.require(name)?;
        if info.tensor_type != TensorType::F16 || info.dimensions.len() != 2 {
            return Err(DiffusionError::model(format!(
                "music3 {name}: expected F16 [hidden, vocab], got {:?} {:?}",
                info.tensor_type, info.dimensions
            )));
        }
        self.gather_rows(name, ids)
    }
}

fn q4_0_gemv_rows(
    packed: &[u8],
    x: &[f32],
    n: usize,
    row_bytes: usize,
    nb: usize,
    bs: usize,
    dst: &mut [f32],
) {
    debug_assert_eq!(dst.len(), n);
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .clamp(1, 8);
    if n < 256 || threads == 1 {
        for row in 0..n {
            dst[row] = q4_0_dot_row(&packed[row * row_bytes..], x, nb, bs);
        }
        return;
    }
    let chunk = n.div_ceil(threads);
    std::thread::scope(|scope| {
        for (t, slot) in dst.chunks_mut(chunk).enumerate() {
            let start = t * chunk;
            scope.spawn(move || {
                for (i, out) in slot.iter_mut().enumerate() {
                    *out = q4_0_dot_row(&packed[(start + i) * row_bytes..], x, nb, bs);
                }
            });
        }
    });
}

fn q4_0_dot_row(row: &[u8], x: &[f32], nb: usize, bs: usize) -> f32 {
    let mut sum = 0f32;
    for blk in 0..nb {
        sum += vec_dot_q4_0_f32(&row[blk * bs..], &x[blk * QK..]);
    }
    sum
}

pub struct Music3GgufPack {
    pub paths: Music3GgufPaths,
    pub language_model: Music3GgufFile,
    pub rvq: Music3GgufFile,
    pub transformer: Music3GgufFile,
    pub condition: Music3GgufFile,
    pub vocoder: Music3GgufFile,
}

impl Music3GgufPack {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_paths(Music3GgufPaths::default_mix(dir))
    }

    pub fn open_paths(paths: Music3GgufPaths) -> Result<Self> {
        Ok(Self {
            language_model: Music3GgufFile::open(
                Music3GgufRole::LanguageModel,
                &paths.language_model,
            )?,
            rvq: Music3GgufFile::open(Music3GgufRole::Rvq, &paths.rvq)?,
            transformer: Music3GgufFile::open(Music3GgufRole::Transformer, &paths.transformer)?,
            condition: Music3GgufFile::open(Music3GgufRole::Condition, &paths.condition)?,
            vocoder: Music3GgufFile::open(Music3GgufRole::Vocoder, &paths.vocoder)?,
            paths,
        })
    }

    pub fn file(&self, role: Music3GgufRole) -> &Music3GgufFile {
        match role {
            Music3GgufRole::LanguageModel => &self.language_model,
            Music3GgufRole::Rvq => &self.rvq,
            Music3GgufRole::Transformer => &self.transformer,
            Music3GgufRole::Condition => &self.condition,
            Music3GgufRole::Vocoder => &self.vocoder,
        }
    }

    pub fn find_tensor(&self, name: &str) -> Option<(&Music3GgufFile, &makepad_llama::GgufTensorInfo)> {
        for role in [
            Music3GgufRole::LanguageModel,
            Music3GgufRole::Rvq,
            Music3GgufRole::Transformer,
            Music3GgufRole::Condition,
            Music3GgufRole::Vocoder,
        ] {
            let file = self.file(role);
            if let Some(info) = file.file.get_tensor(name) {
                return Some((file, info));
            }
        }
        None
    }

    /// Official config shapes vs GGUF native names. GGUF stores ggml order
    /// (ne0 innermost), so a reversed match against the torch canary is ok.
    pub fn validate_python_shapes(&self) -> Result<Vec<String>> {
        for &(role, count) in EXPECTED_TENSORS {
            let got = match role {
                "condition" => self.condition.tensor_count(),
                "vocoder" => self.vocoder.tensor_count(),
                "transformer" => self.transformer.tensor_count(),
                "rvq" => self.rvq.tensor_count(),
                "language_model" => self.language_model.tensor_count(),
                _ => 0,
            };
            if got != count {
                return Err(DiffusionError::model(format!(
                    "music3 {role} tensors {got}, expected {count} (audio.cpp default mix)"
                )));
            }
        }

        let mut ok = Vec::new();
        for &(name, canary) in expected_shape_canaries() {
            let Some((file, info)) = self.find_tensor(name) else {
                return Err(DiffusionError::model(format!(
                    "music3 gguf missing tensor {name}"
                )));
            };
            let dims: Vec<usize> = info
                .dimensions
                .iter()
                .map(|&d| d as usize)
                .collect();
            if !dims_match_torch(&dims, canary) {
                return Err(DiffusionError::model(format!(
                    "music3 {name}: gguf {dims:?} does not match python {canary:?}"
                )));
            }
            ok.push(format!(
                "{} {} {:?} {:?} (python {:?})",
                file.role.as_str(),
                name,
                info.tensor_type,
                dims,
                canary
            ));
        }

        let lm_layers = self
            .language_model
            .file
            .tensors
            .iter()
            .filter(|t| {
                t.name.starts_with("model.layers.")
                    && t.name.ends_with(".input_layernorm.weight")
            })
            .count();
        if lm_layers != MUSIC3_LM_LAYERS {
            return Err(DiffusionError::model(format!(
                "music3 LM layers {lm_layers}, expected {MUSIC3_LM_LAYERS}"
            )));
        }
        let dit_blocks = self
            .transformer
            .file
            .tensors
            .iter()
            .filter(|t| {
                t.name.starts_with("transformer_blocks.")
                    && t.name.ends_with(".attn.to_q.weight")
            })
            .count();
        if dit_blocks != MUSIC3_DIT_LAYERS {
            return Err(DiffusionError::model(format!(
                "music3 DiT blocks {dit_blocks}, expected {MUSIC3_DIT_LAYERS}"
            )));
        }
        Ok(ok)
    }

    /// CPU F32 condition encoder from the pack (same math as the Python
    /// ModularPipeline encoder).
    pub fn load_condition_encoder(&self) -> Result<Music3ConditionEncoder> {
        load_condition_encoder_from(&self.condition)
    }

    pub fn load_vocoder(&self) -> Result<crate::music3_vocoder::Music3Vocoder> {
        load_vocoder_from(&self.vocoder)
    }
}

/// [`Music3GgufPack::load_condition_encoder`] on a lone `condition_encoder.gguf`
/// member (the CUDA pipeline destructures the pack and loads stages lazily).
pub fn load_condition_encoder_from(condition: &Music3GgufFile) -> Result<Music3ConditionEncoder> {
    let logits = condition.read_f32("layer_weight_logits")?;
    let scale = condition.read_f32("layer_scale")?;
    let weight = condition.read_f32("proj.weight")?;
    let bias = condition.read_f32("proj.bias")?;
    if weight.len() != MUSIC3_COND_OUT * MUSIC3_COND_HIDDEN * 3 {
        return Err(DiffusionError::model(format!(
            "cond proj.weight {} values, expected {}",
            weight.len(),
            MUSIC3_COND_OUT * MUSIC3_COND_HIDDEN * 3
        )));
    }
    if logits.len() != MUSIC3_COND_LAYERS {
        return Err(DiffusionError::model(format!(
            "cond layer_weight_logits {}, expected {MUSIC3_COND_LAYERS}",
            logits.len()
        )));
    }
    Music3ConditionEncoder::from_parts(
        logits,
        scale.first().copied().unwrap_or(1.0),
        weight,
        bias,
    )
}

pub fn load_vocoder_from(vocoder: &Music3GgufFile) -> Result<crate::music3_vocoder::Music3Vocoder> {
    crate::music3_vocoder::Music3Vocoder::load_with(|name| vocoder.read_f32(name))
}

fn decode_ggml_f32(
    bytes: &[u8],
    ty: TensorType,
    dims: &[u64],
) -> std::result::Result<Vec<f32>, String> {
    if dims.is_empty() {
        return Err("empty dims".to_string());
    }
    let n_cols = dims[0] as usize;
    let n_rows = dims[1..]
        .iter()
        .map(|&d| d as usize)
        .product::<usize>()
        .max(1);
    match ty {
        TensorType::F32 => {
            if bytes.len() != n_cols.saturating_mul(n_rows).saturating_mul(4) {
                return Err(format!(
                    "F32 byte length {} for {n_rows}x{n_cols}",
                    bytes.len()
                ));
            }
            Ok(bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        }
        TensorType::F16 | TensorType::BF16 | TensorType::Q4_0 | TensorType::Q4K | TensorType::Q8_0 => {
            let indices: Vec<i32> = (0..n_rows as i32).collect();
            get_rows_ggml_bytes_cpu(bytes, ty.ggml_type(), n_cols, n_rows, &indices).ok_or_else(
                || format!("decode {ty:?} {n_rows}x{n_cols} failed ({} bytes)", bytes.len()),
            )
        }
        other => Err(format!("unsupported tensor type {other:?}")),
    }
}

fn kv_string(file: &GgufFile, key: &str) -> String {
    file.get_value(key)
        .and_then(|v| v.as_string())
        .and_then(|s| s.try_utf8().ok())
        .unwrap_or("")
        .to_string()
}

fn matmul_nt_f32_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len() != m * k || bt.len() != n * k {
        return Err(DiffusionError::model(format!(
            "cpu gemm shape a={} bt={} m={m} k={k} n={n}",
            a.len(),
            bt.len()
        )));
    }
    let mut out = vec![0f32; m * n];
    makepad_ai_sfx::sa3::par_rows(&mut out, n, &|i, row| {
        let ai = &a[i * k..(i + 1) * k];
        for (j, dst) in row.iter_mut().enumerate() {
            let bj = &bt[j * k..(j + 1) * k];
            let mut acc = 0f32;
            for t in 0..k {
                acc += ai[t] * bj[t];
            }
            *dst = acc;
        }
    });
    Ok(out)
}

fn dims_match_torch(gguf: &[usize], torch: &[usize]) -> bool {
    if gguf == torch {
        return true;
    }
    if gguf.len() == torch.len() {
        let mut rev = gguf.to_vec();
        rev.reverse();
        return rev == torch;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mix_names_match_audiocpp_readme() {
        let paths = Music3GgufPaths::default_mix("/tmp/music3");
        assert!(paths.language_model.ends_with(MUSIC3_GGUF_LM));
        assert!(paths.transformer.ends_with(MUSIC3_GGUF_DIT));
        assert!(paths.rvq.ends_with(MUSIC3_GGUF_RVQ));
    }

    #[test]
    fn ggml_dims_accept_reversed_torch_canary() {
        assert!(dims_match_torch(&[2048, 2304], &[2048, 2304]));
        assert!(dims_match_torch(&[2304, 2048], &[2048, 2304]));
        assert!(!dims_match_torch(&[2048, 2048], &[2048, 2304]));
    }

    #[test]
    fn decode_f32_bf16_q4_rows() {
        let f32_bytes: Vec<u8> = [1.0f32, -2.0, 3.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let f32 = decode_ggml_f32(&f32_bytes, TensorType::F32, &[3]).unwrap();
        assert_eq!(f32, vec![1.0, -2.0, 3.5]);

        // BF16 1.0 (f32 0x3f800000) / -2.0 (f32 0xc0000000)
        let bf = decode_ggml_f32(&[0x80, 0x3f, 0x00, 0xc0], TensorType::BF16, &[2]).unwrap();
        assert_eq!(bf, vec![1.0, -2.0]);

        // Q4_0 scale=1.0 (f16 0x3c00), qs all 0 → values -8.
        let mut q4 = vec![0u8; 18];
        q4[0] = 0x00;
        q4[1] = 0x3c;
        let q = decode_ggml_f32(&q4, TensorType::Q4_0, &[32]).unwrap();
        assert_eq!(q.len(), 32);
        assert!(q.iter().all(|v| (*v + 8.0).abs() < 1e-5));
    }
}
