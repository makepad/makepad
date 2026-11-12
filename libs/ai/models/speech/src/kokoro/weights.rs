//! Reader for the flat `.mktts` weight file produced by `tools/convert_kokoro.py`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use makepad_ai_loader::{F32Weight, MappedRegion, WeightOwner};

use crate::TtsError;

const MAGIC: &[u8] = b"MKTTS\0\0\0";
const VERSION: u32 = 1;

pub struct TensorInfo {
    pub shape: Vec<usize>,
    byte_offset: usize,
    byte_len: usize,
}

impl TensorInfo {
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }
}

/// An `f32` weight array: a view into the read-only mapping of the model file
/// (the converter pads every tensor to a 32-byte boundary and the mapping is
/// page aligned, so the view is aligned), or an owned buffer for weights that
/// are computed at load time.
pub enum WeightData {
    Owned(Vec<f32>),
    /// `offset` and `len` are in `f32` units.
    Mapped { map: Arc<MappedRegion>, offset: usize, len: usize, resident: Box<Resident> },
    /// A weight-normed kernel `g * v / ‖v‖` over mapped `g` and `v`. The folded
    /// array is computed on first access and cached; [`WeightData::drop_host_copy`]
    /// frees it (a device holds its own copy) and the next access recomputes it.
    Normed(Box<Normed>),
}

/// The GPU-resident f32 form of a weight, built on first GPU use (Metal shares its host pages).
pub struct Resident {
    owner: Arc<WeightOwner>,
    name: String,
    weight: OnceLock<F32Weight>,
    taps: OnceLock<F32Weight>,
}

impl Resident {
    fn new(owner: &Arc<WeightOwner>, name: &str) -> Self {
        Self { owner: owner.clone(), name: name.to_owned(), weight: OnceLock::new(), taps: OnceLock::new() }
    }
}

pub struct Normed {
    g: WeightData,
    v: WeightData,
    out_channels: usize,
    folded: OnceLock<Vec<f32>>,
    resident: Resident,
}

impl WeightData {
    /// Element count, without materializing a folded kernel.
    pub fn len(&self) -> usize {
        match self {
            WeightData::Owned(v) => v.len(),
            WeightData::Mapped { len, .. } => *len,
            WeightData::Normed(n) => n.v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Materialize page-aligned f32 storage only when a GPU gateway requests it.
    pub fn resident(&self) -> Option<&F32Weight> {
        let resident = match self {
            WeightData::Owned(_) => return None,
            WeightData::Mapped { resident, .. } => &**resident,
            WeightData::Normed(n) => &n.resident,
        };
        Some(resident.weight.get_or_init(|| {
            match F32Weight::new(&resident.owner, &resident.name, self.len(), |out| {
                match self {
                    WeightData::Normed(n) => fold_weight_norm(&n.g, &n.v, n.out_channels, out),
                    _ => out.copy_from_slice(self),
                }
                Ok::<(), std::convert::Infallible>(())
            }) {
                Ok(weight) => weight,
                Err(never) => match never {},
            }
        }))
    }

    /// Cache the `[out * kernel, in]` tap matrix for transposed convolution.
    pub fn resident_taps(&self, in_channels: usize, out_channels: usize, kernel: usize) -> Option<&F32Weight> {
        let resident = match self {
            WeightData::Owned(_) => return None,
            WeightData::Mapped { resident, .. } => &**resident,
            WeightData::Normed(n) => &n.resident,
        };
        Some(resident.taps.get_or_init(|| {
            // A weight-normed kernel nothing has folded yet is folded into a scratch copy, so neither the
            // resident weight nor the folded cache is kept beside the taps.
            let scratch;
            let values: &[f32] = match self {
                WeightData::Normed(n) if n.resident.weight.get().is_none() && n.folded.get().is_none() => {
                    let mut folded = vec![0.0; n.v.len()];
                    fold_weight_norm(&n.g, &n.v, n.out_channels, &mut folded);
                    scratch = folded;
                    &scratch[..]
                }
                _ => &**self,
            };
            match F32Weight::new(&resident.owner, &format!("{}.taps", resident.name), self.len(), |out| {
                for ic in 0..in_channels {
                    for oc in 0..out_channels {
                        for k in 0..kernel {
                            out[(oc * kernel + k) * in_channels + ic] =
                                values[(ic * out_channels + oc) * kernel + k];
                        }
                    }
                }
                Ok::<(), std::convert::Infallible>(())
            }) {
                Ok(weight) => weight,
                Err(never) => match never {},
            }
        }))
    }

    /// A stable identity for device-side caches: it does not change when a
    /// folded kernel is dropped and recomputed.
    pub fn key(&self) -> usize {
        match self {
            WeightData::Owned(v) => v.as_ptr() as usize,
            WeightData::Mapped { .. } => self.as_ptr() as usize,
            WeightData::Normed(n) => n.v.key(),
        }
    }

    /// Frees a cached folded kernel; later reads recompute it from the mapped
    /// `g` and `v`. Needs `&mut` so no borrow of the cache can be outstanding.
    pub fn drop_host_copy(&mut self) {
        if let WeightData::Normed(n) = self {
            n.folded.take();
        }
    }
}

fn fold_weight_norm(g: &[f32], v: &[f32], out_channels: usize, weight: &mut [f32]) {
    let per_channel = v.len() / out_channels;
    for channel in 0..out_channels {
        let row = &v[channel * per_channel..(channel + 1) * per_channel];
        let norm = row.iter().map(|x| x * x).sum::<f32>().sqrt();
        // A zero row would divide by zero; upstream never has one, but a
        // silent NaN here would be very hard to trace later.
        let scale = if norm > 0.0 { g[channel] / norm } else { 0.0 };
        for (out, x) in weight[channel * per_channel..(channel + 1) * per_channel].iter_mut().zip(row) {
            *out = x * scale;
        }
    }
}

/// Hands freed heap pages back to the OS. glibc keeps freed blocks of this size
/// in its arenas, so dropping the folded kernels would not lower the process
/// footprint otherwise; other allocators return them on their own.
pub fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // Safety: plain libc call with no pointers.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// What the Vulkan graph needs from a host weight: a stable identity and size
/// for its resident-weight cache, and the values only on a cache miss.
pub trait HostWeight {
    fn key(&self) -> usize;
    fn count(&self) -> usize;
    fn host(&self) -> &[f32];
}

impl HostWeight for [f32] {
    fn key(&self) -> usize { self.as_ptr() as usize }
    fn count(&self) -> usize { self.len() }
    fn host(&self) -> &[f32] { self }
}

impl HostWeight for Vec<f32> {
    fn key(&self) -> usize { self.as_ptr() as usize }
    fn count(&self) -> usize { self.len() }
    fn host(&self) -> &[f32] { self }
}

impl HostWeight for WeightData {
    fn key(&self) -> usize { WeightData::key(self) }
    fn count(&self) -> usize { self.len() }
    fn host(&self) -> &[f32] { self }
}

impl std::ops::Deref for WeightData {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        match self {
            WeightData::Owned(v) => v,
            WeightData::Mapped { map, offset, len, .. } => mapped_f32(map, *offset, *len),
            WeightData::Normed(n) => {
                if let Some(weight) = n.resident.weight.get() {
                    return weight.as_slice();
                }
                n.folded.get_or_init(|| {
                    let mut weight = vec![0.0; n.v.len()];
                    fold_weight_norm(&n.g, &n.v, n.out_channels, &mut weight);
                    weight
                })
            }
        }
    }
}

impl From<Vec<f32>> for WeightData {
    fn from(v: Vec<f32>) -> Self {
        WeightData::Owned(v)
    }
}

fn mapped_f32(map: &MappedRegion, offset: usize, len: usize) -> &[f32] {
    let bytes = map.as_slice();
    assert!((offset + len) * 4 <= bytes.len() && bytes.as_ptr() as usize % 4 == 0);
    // Safety: the extent was checked against the mapping when the index was
    // parsed; the mapping is page aligned and `offset` counts whole `f32`s; any
    // bit pattern is a valid `f32`.
    unsafe { std::slice::from_raw_parts((bytes.as_ptr() as *const f32).add(offset), len) }
}

pub struct Weights {
    /// The whole file, mapped read-only.
    map: Arc<MappedRegion>,
    tensors: HashMap<String, TensorInfo>,
    owner: Arc<WeightOwner>,
}

fn bad(message: impl Into<String>) -> TtsError {
    TtsError::Backend(message.into())
}

impl Weights {
    pub fn load(path: &str) -> Result<Self, TtsError> {
        let map = Arc::new(MappedRegion::map(path).map_err(|err| bad(format!("{path}: {err}")))?);
        let tensors = parse_index(map.as_slice())?;
        Ok(Self { map, tensors, owner: Arc::new(WeightOwner::new("kokoro-f32")) })
    }

    /// The mapping of the whole file, to drop its pages once a device holds
    /// its own copy of the weights.
    pub fn mapped(&self) -> &Arc<MappedRegion> {
        &self.map
    }

    pub fn len(&self) -> usize {
        self.tensors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tensors.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tensors.keys().map(String::as_str)
    }

    pub fn info(&self, name: &str) -> Option<&TensorInfo> {
        self.tensors.get(name)
    }

    /// Zero-copy view of a tensor.
    pub fn get(&self, name: &str) -> Option<&[f32]> {
        let info = self.tensors.get(name)?;
        Some(mapped_f32(&self.map, info.byte_offset / 4, info.byte_len / 4))
    }

    /// A tensor that keeps pointing into the mapping (no copy).
    pub fn take(&self, name: &str) -> Option<WeightData> {
        let info = self.tensors.get(name)?;
        Some(WeightData::Mapped {
            map: self.map.clone(),
            offset: info.byte_offset / 4,
            len: info.byte_len / 4,
            resident: Box::new(Resident::new(&self.owner, name)),
        })
    }

    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.tensors.get(name).map(|info| info.shape.as_slice())
    }

    /// Rebuild a PyTorch weight-normed kernel: `W = g * v / ‖v‖`, with the norm
    /// taken over everything but the output channel.
    ///
    /// Kokoro's decoder stores `weight_g` and `weight_v` rather than `weight`.
    /// Doing this in Rust rather than the converter keeps the Python side pure
    /// standard library — a norm over 40M floats in interpreted Python is a
    /// minute of nothing.
    pub fn weight_norm(&self, prefix: &str) -> Result<WeightData, TtsError> {
        let g_name = format!("{prefix}.weight_g");
        let v_name = format!("{prefix}.weight_v");

        let g = self
            .get(&g_name)
            .ok_or_else(|| bad(format!("missing {g_name}")))?;
        self.get(&v_name)
            .ok_or_else(|| bad(format!("missing {v_name}")))?;
        let shape = self.shape(&v_name).unwrap();

        let out_channels = *shape.first().ok_or_else(|| bad("weight_v is scalar"))?;
        if g.len() != out_channels {
            return Err(bad(format!(
                "{prefix}: weight_g has {} entries for {out_channels} channels",
                g.len()
            )));
        }
        Ok(WeightData::Normed(Box::new(Normed {
            g: self.take(&g_name).unwrap(),
            v: self.take(&v_name).unwrap(),
            out_channels,
            folded: OnceLock::new(),
            resident: Resident::new(&self.owner, prefix),
        })))
    }
}

fn parse_index(bytes: &[u8]) -> Result<HashMap<String, TensorInfo>, TtsError> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err(bad("not a .mktts file"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != VERSION {
        return Err(bad(format!("unsupported .mktts version {version}")));
    }
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;

    let mut at = 16;
    let read_u32 = |at: &mut usize| -> u32 {
        let value = u32::from_le_bytes(bytes[*at..*at + 4].try_into().unwrap());
        *at += 4;
        value
    };

    let mut tensors = HashMap::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u32(&mut at) as usize;
        let name = std::str::from_utf8(&bytes[at..at + name_len])
            .map_err(|_| bad("tensor name is not utf8"))?
            .to_string();
        at += name_len;

        let dtype = bytes[at];
        let ndim = bytes[at + 1] as usize;
        at += 2;
        if dtype != 0 {
            return Err(bad(format!("{name}: only f32 is supported")));
        }

        let shape: Vec<usize> = (0..ndim).map(|_| read_u32(&mut at) as usize).collect();
        let byte_offset = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        let byte_len = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
        at += 16;

        if byte_offset % 4 != 0 || byte_offset + byte_len > bytes.len() {
            return Err(bad(format!("{name}: bad tensor extent")));
        }
        let expected: usize = shape.iter().product::<usize>() * 4;
        if expected != byte_len {
            return Err(bad(format!(
                "{name}: shape {shape:?} implies {expected} bytes, index says {byte_len}"
            )));
        }

        tensors.insert(
            name,
            TensorInfo {
                shape,
                byte_offset,
                byte_len,
            },
        );
    }
    Ok(tensors)
}
