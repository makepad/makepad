//! Minimal reader for torch.save() `.pth` archives holding a plain
//! `{"state_dict": OrderedDict[str, Tensor], ...}` dict (the audiotools
//! BaseModel format used by the MOSS-SoundEffect DAC VAE weights).
//!
//! Scope (checked, everything else errors loudly):
//! - the archive is a ZIP with STORED (uncompressed) entries — torch.save
//!   always writes ZIP_STORED;
//! - the pickle uses only the opcode subset torch emits for nested
//!   dict/str/int/tuple structures plus `torch._utils._rebuild_tensor_v2`
//!   with `torch.FloatStorage` / `torch.HalfStorage` / `torch.BFloat16Storage`
//!   persistent ids;
//! - tensors can be materialized as contiguous f32 or copied in their saved
//!   scalar dtype via their saved stride.
//!
//! This is NOT a general unpickler: REDUCE of unknown callables, classes with
//! state (BUILD of non-dict state), and object graphs are rejected.
//!
//! Staged copy of libs/diffusion/src/torch_pth.rs (lane T2, /aiarch.md §1) —
//! coexists here with formats/torch.rs (the torch_bin.rs copy) as a separate
//! module rather than a merge, since both are mechanical, working parsers.
//! Pure staging: the original stays in place and keeps depending on
//! `crate::error::DiffusionError`; consumers are re-pointed in a later lane.
//! Here that diffusion-specific error type is swapped for a small local
//! `TorchPthError` so this module has no dependency on the diffusion crate.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub type Result<T> = std::result::Result<T, TorchPthError>;

#[derive(Debug)]
pub struct TorchPthError(String);

impl std::fmt::Display for TorchPthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "torch_pth: {}", self.0)
    }
}

impl std::error::Error for TorchPthError {}

fn err(msg: impl std::fmt::Display) -> TorchPthError {
    TorchPthError(format!("{msg}"))
}

// ---------------------------------------------------------------------------
// ZIP (stored entries only)
// ---------------------------------------------------------------------------

struct ZipEntry {
    /// Absolute byte offset of the entry's data.
    data_offset: u64,
    size: u64,
}

struct ZipIndex {
    file: File,
    entries: HashMap<String, ZipEntry>,
}

impl ZipIndex {
    fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        let file_len = file.seek(SeekFrom::End(0)).map_err(|e| err(e))?;
        // Find EOCD (PK\x05\x06) in the trailing 66KB.
        let tail_len = file_len.min(66 * 1024);
        file.seek(SeekFrom::Start(file_len - tail_len)).map_err(|e| err(e))?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail).map_err(|e| err(e))?;
        let eocd = tail
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .ok_or_else(|| err("no ZIP end-of-central-directory record"))?;
        let rec = &tail[eocd..];
        if rec.len() < 22 {
            return Err(err("truncated EOCD"));
        }
        let count = u16::from_le_bytes([rec[10], rec[11]]) as usize;
        let cd_size = u32::from_le_bytes([rec[12], rec[13], rec[14], rec[15]]) as u64;
        let cd_offset = u32::from_le_bytes([rec[16], rec[17], rec[18], rec[19]]) as u64;
        if cd_offset == 0xFFFF_FFFF {
            return Err(err("zip64 central directory not supported"));
        }
        file.seek(SeekFrom::Start(cd_offset)).map_err(|e| err(e))?;
        let mut cd = vec![0u8; cd_size as usize];
        file.read_exact(&mut cd).map_err(|e| err(e))?;

        let mut headers: Vec<(String, u64, u64, u16)> = Vec::with_capacity(count);
        let mut pos = 0usize;
        while pos + 46 <= cd.len() {
            if cd[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
                break;
            }
            let method = u16::from_le_bytes([cd[pos + 10], cd[pos + 11]]);
            let comp_size = u32::from_le_bytes([
                cd[pos + 20], cd[pos + 21], cd[pos + 22], cd[pos + 23],
            ]) as u64;
            let name_len = u16::from_le_bytes([cd[pos + 28], cd[pos + 29]]) as usize;
            let extra_len = u16::from_le_bytes([cd[pos + 30], cd[pos + 31]]) as usize;
            let comment_len = u16::from_le_bytes([cd[pos + 32], cd[pos + 33]]) as usize;
            let local_offset = u32::from_le_bytes([
                cd[pos + 42], cd[pos + 43], cd[pos + 44], cd[pos + 45],
            ]) as u64;
            let name = String::from_utf8_lossy(&cd[pos + 46..pos + 46 + name_len]).into_owned();
            if method != 0 {
                return Err(err(format!("entry {name}: compressed ({method}), expected stored")));
            }
            headers.push((name, local_offset, comp_size, 0));
            pos += 46 + name_len + extra_len + comment_len;
        }
        // Resolve data offsets through each local header (its extra field length
        // differs from the central one — torch pads storages for alignment).
        let mut entries = HashMap::with_capacity(headers.len());
        for (name, local_offset, size, _) in headers {
            let mut hdr = [0u8; 30];
            file.seek(SeekFrom::Start(local_offset)).map_err(|e| err(e))?;
            file.read_exact(&mut hdr).map_err(|e| err(e))?;
            if hdr[0..4] != [0x50, 0x4b, 0x03, 0x04] {
                return Err(err(format!("entry {name}: bad local header")));
            }
            let name_len = u16::from_le_bytes([hdr[26], hdr[27]]) as u64;
            let extra_len = u16::from_le_bytes([hdr[28], hdr[29]]) as u64;
            entries.insert(
                name,
                ZipEntry {
                    data_offset: local_offset + 30 + name_len + extra_len,
                    size,
                },
            );
        }
        Ok(Self { file, entries })
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| err(format!("zip entry missing: {name}")))?;
        let mut buf = vec![0u8; entry.size as usize];
        self.file
            .seek(SeekFrom::Start(entry.data_offset))
            .map_err(|e| err(e))?;
        self.file.read_exact(&mut buf).map_err(|e| err(e))?;
        Ok(buf)
    }

    /// The archive-internal prefix (torch names the root after the file, e.g.
    /// `weights/data.pkl`, `archive/data.pkl`).
    fn root(&self) -> Result<String> {
        for name in self.entries.keys() {
            if let Some(prefix) = name.strip_suffix("/data.pkl") {
                return Ok(prefix.to_string());
            }
        }
        Err(err("no data.pkl in archive"))
    }
}

// ---------------------------------------------------------------------------
// Pickle VM (constrained subset)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Global(String, String),
    /// (storage_dtype, storage_key, numel)
    Storage(StorageRef),
    Tensor(RawTensor),
    Mark,
}

#[derive(Clone, Debug)]
struct StorageRef {
    dtype: String,
    key: String,
    numel: usize,
}

#[derive(Clone, Debug)]
pub struct RawTensor {
    storage_dtype: String,
    storage_key: String,
    offset: usize,
    pub shape: Vec<usize>,
    stride: Vec<usize>,
}

fn as_usize_vec(v: &Value) -> Result<Vec<usize>> {
    match v {
        Value::Tuple(items) | Value::List(items) => items
            .iter()
            .map(|it| match it {
                Value::Int(i) if *i >= 0 => Ok(*i as usize),
                other => Err(err(format!("expected non-negative int, got {other:?}"))),
            })
            .collect(),
        other => Err(err(format!("expected tuple of ints, got {other:?}"))),
    }
}

fn rebuild_tensor_v2(args: &[Value]) -> Result<Value> {
    if args.len() < 4 {
        return Err(err("_rebuild_tensor_v2 arity"));
    }
    let storage = match &args[0] {
        Value::Storage(s) => s.clone(),
        other => return Err(err(format!("rebuild storage {other:?}"))),
    };
    let offset = match &args[1] {
        Value::Int(i) if *i >= 0 => *i as usize,
        other => return Err(err(format!("rebuild offset {other:?}"))),
    };
    let shape = as_usize_vec(&args[2])?;
    let stride = as_usize_vec(&args[3])?;
    Ok(Value::Tensor(RawTensor {
        storage_dtype: storage.dtype,
        storage_key: storage.key,
        offset,
        shape,
        stride,
    }))
}

fn unpickle(data: &[u8]) -> Result<Value> {
    let mut pos = 0usize;
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<u32, Value> = HashMap::new();

    macro_rules! need {
        ($n:expr) => {{
            if pos + $n > data.len() {
                return Err(err("pickle truncated"));
            }
        }};
    }
    macro_rules! pop {
        () => {
            stack.pop().ok_or_else(|| err("pickle stack underflow"))?
        };
    }

    loop {
        need!(1);
        let op = data[pos];
        pos += 1;
        match op {
            0x80 => {
                need!(1);
                pos += 1; // PROTO
            }
            0x95 => {
                need!(8);
                pos += 8; // FRAME
            }
            b'}' => stack.push(Value::Dict(Vec::new())),
            b')' => stack.push(Value::Tuple(Vec::new())),
            b']' => stack.push(Value::List(Vec::new())),
            b'(' => stack.push(Value::Mark),
            b'N' => stack.push(Value::None),
            0x88 => stack.push(Value::Bool(true)),
            0x89 => stack.push(Value::Bool(false)),
            b'K' => {
                need!(1);
                stack.push(Value::Int(data[pos] as i64));
                pos += 1;
            }
            b'M' => {
                need!(2);
                stack.push(Value::Int(u16::from_le_bytes([data[pos], data[pos + 1]]) as i64));
                pos += 2;
            }
            b'J' => {
                need!(4);
                stack.push(Value::Int(i32::from_le_bytes([
                    data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                ]) as i64));
                pos += 4;
            }
            b'G' => {
                // BINFLOAT: 8-byte big-endian double
                need!(8);
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[pos..pos + 8]);
                stack.push(Value::Float(f64::from_be_bytes(bytes)));
                pos += 8;
            }
            0x8a => {
                // LONG1
                need!(1);
                let n = data[pos] as usize;
                pos += 1;
                need!(n);
                let mut v: i64 = 0;
                for (i, &b) in data[pos..pos + n].iter().enumerate() {
                    v |= (b as i64) << (8 * i);
                }
                if n > 0 && data[pos + n - 1] & 0x80 != 0 && n < 8 {
                    v -= 1i64 << (8 * n);
                }
                pos += n;
                stack.push(Value::Int(v));
            }
            0x8c => {
                // SHORT_BINUNICODE
                need!(1);
                let n = data[pos] as usize;
                pos += 1;
                need!(n);
                stack.push(Value::Str(
                    String::from_utf8_lossy(&data[pos..pos + n]).into_owned(),
                ));
                pos += n;
            }
            b'X' => {
                need!(4);
                let n = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
                pos += 4;
                need!(n);
                stack.push(Value::Str(
                    String::from_utf8_lossy(&data[pos..pos + n]).into_owned(),
                ));
                pos += n;
            }
            b'c' => {
                // GLOBAL: two newline-terminated lines
                let mut read_line = || -> Result<String> {
                    let start = pos;
                    while pos < data.len() && data[pos] != b'\n' {
                        pos += 1;
                    }
                    if pos >= data.len() {
                        return Err(err("GLOBAL truncated"));
                    }
                    let s = String::from_utf8_lossy(&data[start..pos]).into_owned();
                    pos += 1;
                    Ok(s)
                };
                let module = read_line()?;
                let name = read_line()?;
                stack.push(Value::Global(module, name));
            }
            0x93 => {
                // STACK_GLOBAL
                let name = pop!();
                let module = pop!();
                match (module, name) {
                    (Value::Str(m), Value::Str(n)) => stack.push(Value::Global(m, n)),
                    _ => return Err(err("STACK_GLOBAL non-string args")),
                }
            }
            b'q' => {
                need!(1);
                let k = data[pos] as u32;
                pos += 1;
                let top = stack.last().ok_or_else(|| err("BINPUT empty stack"))?.clone();
                memo.insert(k, top);
            }
            b'r' => {
                need!(4);
                let k = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                pos += 4;
                let top = stack.last().ok_or_else(|| err("LONG_BINPUT empty stack"))?.clone();
                memo.insert(k, top);
            }
            0x94 => {
                let k = memo.len() as u32;
                let top = stack.last().ok_or_else(|| err("MEMOIZE empty stack"))?.clone();
                memo.insert(k, top);
            }
            b'h' => {
                need!(1);
                let k = data[pos] as u32;
                pos += 1;
                stack.push(memo.get(&k).ok_or_else(|| err("BINGET missing"))?.clone());
            }
            b'j' => {
                need!(4);
                let k = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                pos += 4;
                stack.push(memo.get(&k).ok_or_else(|| err("LONG_BINGET missing"))?.clone());
            }
            0x85 => {
                let a = pop!();
                stack.push(Value::Tuple(vec![a]));
            }
            0x86 => {
                let b = pop!();
                let a = pop!();
                stack.push(Value::Tuple(vec![a, b]));
            }
            0x87 => {
                let c = pop!();
                let b = pop!();
                let a = pop!();
                stack.push(Value::Tuple(vec![a, b, c]));
            }
            b't' => {
                let mut items = Vec::new();
                loop {
                    match pop!() {
                        Value::Mark => break,
                        v => items.push(v),
                    }
                }
                items.reverse();
                stack.push(Value::Tuple(items));
            }
            b'Q' => {
                // BINPERSID: persistent id tuple ('storage', Global, key, device, numel)
                let pid = pop!();
                let items = match pid {
                    Value::Tuple(items) => items,
                    other => return Err(err(format!("BINPERSID non-tuple: {other:?}"))),
                };
                if items.len() != 5 {
                    return Err(err(format!("persistent id arity {} != 5", items.len())));
                }
                let tag = match &items[0] {
                    Value::Str(s) => s.as_str(),
                    _ => "",
                };
                if tag != "storage" {
                    return Err(err(format!("persistent id tag {tag:?}")));
                }
                let dtype = match &items[1] {
                    Value::Global(_, n) => n.clone(),
                    other => return Err(err(format!("storage class {other:?}"))),
                };
                let key = match &items[2] {
                    Value::Str(s) => s.clone(),
                    other => return Err(err(format!("storage key {other:?}"))),
                };
                let numel = match &items[4] {
                    Value::Int(i) if *i >= 0 => *i as usize,
                    other => return Err(err(format!("storage numel {other:?}"))),
                };
                stack.push(Value::Storage(StorageRef { dtype, key, numel }));
            }
            b'R' => {
                // REDUCE
                let args = pop!();
                let callable = pop!();
                let value = match (&callable, args) {
                    (Value::Global(m, n), Value::Tuple(args))
                        if m == "torch._utils" && n == "_rebuild_tensor_v2" =>
                    {
                        rebuild_tensor_v2(&args)?
                    }
                    // torch._tensor._rebuild_from_type_v2(func, type, args, state):
                    // the plain-Tensor wrapper newer torch emits in state dicts.
                    (Value::Global(m, n), Value::Tuple(args))
                        if m == "torch._tensor" && n == "_rebuild_from_type_v2" =>
                    {
                        if args.len() < 3 {
                            return Err(err("_rebuild_from_type_v2 arity"));
                        }
                        match (&args[0], &args[2]) {
                            (Value::Global(fm, fnn), Value::Tuple(inner))
                                if fm == "torch._utils" && fnn == "_rebuild_tensor_v2" =>
                            {
                                rebuild_tensor_v2(inner)?
                            }
                            (func, _) => {
                                return Err(err(format!(
                                    "_rebuild_from_type_v2 of unsupported func {func:?}"
                                )));
                            }
                        }
                    }
                    (Value::Global(m, n), Value::Tuple(_))
                        if m == "collections" && n == "OrderedDict" =>
                    {
                        Value::Dict(Vec::new())
                    }
                    (Value::Global(m, n), _) => {
                        return Err(err(format!("REDUCE of unsupported callable {m}.{n}")));
                    }
                    (other, _) => return Err(err(format!("REDUCE of non-global {other:?}"))),
                };
                stack.push(value);
            }
            b's' => {
                // SETITEM
                let v = pop!();
                let k = pop!();
                match stack.last_mut() {
                    Some(Value::Dict(items)) => items.push((k, v)),
                    other => return Err(err(format!("SETITEM into {other:?}"))),
                }
            }
            b'u' => {
                // SETITEMS (to mark)
                let mut kvs = Vec::new();
                loop {
                    match pop!() {
                        Value::Mark => break,
                        v => kvs.push(v),
                    }
                }
                kvs.reverse();
                if kvs.len() % 2 != 0 {
                    return Err(err("SETITEMS odd count"));
                }
                match stack.last_mut() {
                    Some(Value::Dict(items)) => {
                        let mut it = kvs.into_iter();
                        while let (Some(k), Some(v)) = (it.next(), it.next()) {
                            items.push((k, v));
                        }
                    }
                    other => return Err(err(format!("SETITEMS into {other:?}"))),
                }
            }
            b'e' => {
                // APPENDS (to mark)
                let mut vs = Vec::new();
                loop {
                    match pop!() {
                        Value::Mark => break,
                        v => vs.push(v),
                    }
                }
                vs.reverse();
                match stack.last_mut() {
                    Some(Value::List(items)) => items.extend(vs),
                    other => return Err(err(format!("APPENDS into {other:?}"))),
                }
            }
            b'a' => {
                // APPEND
                let v = pop!();
                match stack.last_mut() {
                    Some(Value::List(items)) => items.push(v),
                    other => return Err(err(format!("APPEND into {other:?}"))),
                }
            }
            b'b' => {
                // BUILD: obj.__setstate__(state). Only dict-state onto Dict is
                // meaningful for our subset; merge it.
                let state = pop!();
                match (stack.last_mut(), state) {
                    (Some(Value::Dict(items)), Value::Dict(state_items)) => {
                        items.extend(state_items);
                    }
                    (Some(_), Value::None) => {}
                    (top, state) => {
                        return Err(err(format!("BUILD {state:?} onto {top:?}")));
                    }
                }
            }
            b'.' => {
                return pop!().pipe_ok();
            }
            other => {
                return Err(err(format!(
                    "unsupported pickle opcode 0x{other:02x} at {}",
                    pos - 1
                )));
            }
        }
    }
}

trait PipeOk {
    fn pipe_ok(self) -> Result<Value>;
}
impl PipeOk for Value {
    fn pipe_ok(self) -> Result<Value> {
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A `.pth` state dict: tensor name -> (shape, contiguous f32 data), loaded
/// lazily per tensor (storages are read from the zip on demand).
pub struct PthStateDict {
    zip: ZipIndex,
    root: String,
    tensors: HashMap<String, RawTensor>,
}

/// Scalar types supported by the constrained torch checkpoint reader.  The
/// raw path preserves these bits for deterministic safetensors conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PthDType {
    F32,
    F16,
    BF16,
}

impl PthDType {
    pub fn element_size(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F16 | Self::BF16 => 2,
        }
    }

    fn from_storage(storage: &str) -> Result<Self> {
        match storage {
            "FloatStorage" => Ok(Self::F32),
            "HalfStorage" => Ok(Self::F16),
            "BFloat16Storage" => Ok(Self::BF16),
            other => Err(err(format!("unsupported storage dtype {other}"))),
        }
    }
}

impl PthStateDict {
    /// Opens the archive and locates the `state_dict` map inside the pickled
    /// root object (either the root itself is the state dict, or it is a dict
    /// with a `state_dict` entry — the audiotools BaseModel layout).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let mut zip = ZipIndex::open(path.as_ref())?;
        let root = zip.root()?;
        let pkl = zip.read(&format!("{root}/data.pkl"))?;
        let value = unpickle(&pkl)?;
        let dict = match &value {
            Value::Dict(items) => items,
            other => return Err(err(format!("pickle root is {other:?}, expected dict"))),
        };
        // Prefer an inner "state_dict" entry; else use the root dict.
        let mut state_items = dict;
        for (k, v) in dict {
            if matches!(k, Value::Str(s) if s == "state_dict") {
                match v {
                    Value::Dict(items) => state_items = items,
                    other => return Err(err(format!("state_dict is {other:?}"))),
                }
            }
        }
        let mut tensors = HashMap::with_capacity(state_items.len());
        for (k, v) in state_items {
            if let (Value::Str(name), Value::Tensor(t)) = (k, v) {
                tensors.insert(name.clone(), t.clone());
            }
        }
        if tensors.is_empty() {
            return Err(err("no tensors found in state dict"));
        }
        Ok(Self { zip, root, tensors })
    }

    /// Opens an archive whose pickled root is an ARBITRARILY NESTED structure
    /// of dicts holding tensors (the IndexTTS checkpoints: `gpt.pth` is a
    /// flat state dict, `s2mel.pth` is `{"net": {"cfm": {...}, ...}, ...}`,
    /// `codec.pth` is `{"model": {...}}`, `bigvgan_generator.pt` is
    /// `{"generator": {...}}`, and `feat1.pt`/`feat2.pt` are a bare tensor).
    ///
    /// Tensor names become dot-joined paths (`net.cfm.estimator...`); a bare
    /// tensor root is stored under `""`. Non-tensor leaves (epoch counters,
    /// config strings, `num_batches_tracked` ints, ...) are skipped silently —
    /// they only error if actually read.
    pub fn load_nested(path: impl AsRef<Path>) -> Result<Self> {
        let mut zip = ZipIndex::open(path.as_ref())?;
        let root = zip.root()?;
        let pkl = zip.read(&format!("{root}/data.pkl"))?;
        let value = unpickle(&pkl)?;
        let mut tensors = HashMap::new();
        fn walk(value: &Value, prefix: &str, out: &mut HashMap<String, RawTensor>) {
            match value {
                Value::Tensor(t) => {
                    out.insert(prefix.to_string(), t.clone());
                }
                Value::Dict(items) => {
                    for (k, v) in items {
                        if let Value::Str(name) = k {
                            let joined = if prefix.is_empty() {
                                name.clone()
                            } else {
                                format!("{prefix}.{name}")
                            };
                            walk(v, &joined, out);
                        }
                    }
                }
                _ => {}
            }
        }
        walk(&value, "", &mut tensors);
        if tensors.is_empty() {
            return Err(err("no tensors found in nested checkpoint"));
        }
        Ok(Self { zip, root, tensors })
    }

    /// Convenience for single-tensor checkpoints (`feat1.pt`/`feat2.pt`):
    /// returns (shape, contiguous f32 data) of the bare pickled tensor.
    pub fn load_single_tensor(path: impl AsRef<Path>) -> Result<(Vec<usize>, Vec<f32>)> {
        let mut sd = Self::load_nested(path)?;
        let name = sd
            .tensors
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| err("empty checkpoint"))?;
        if sd.tensors.len() != 1 {
            return Err(err(format!(
                "expected a single tensor, found {}",
                sd.tensors.len()
            )));
        }
        let shape = sd.shape(&name)?;
        let data = sd.f32(&name)?;
        Ok((shape, data))
    }

    pub fn has(&self, name: &str) -> bool {
        self.tensors.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.tensors.keys()
    }

    pub fn shape(&self, name: &str) -> Result<Vec<usize>> {
        Ok(self
            .tensors
            .get(name)
            .ok_or_else(|| err(format!("tensor missing: {name}")))?
            .shape
            .clone())
    }

    pub fn dtype(&self, name: &str) -> Result<PthDType> {
        let tensor = self
            .tensors
            .get(name)
            .ok_or_else(|| err(format!("tensor missing: {name}")))?;
        PthDType::from_storage(&tensor.storage_dtype)
    }

    /// Reads one tensor in its original scalar dtype and materializes its
    /// logical shape contiguously.  Returned bytes are little-endian, exactly
    /// the representation safetensors uses for F32/F16/BF16.
    pub fn raw_contiguous(&mut self, name: &str) -> Result<Vec<u8>> {
        let tensor = self
            .tensors
            .get(name)
            .ok_or_else(|| err(format!("tensor missing: {name}")))?
            .clone();
        let dtype = PthDType::from_storage(&tensor.storage_dtype)?;
        let element_size = dtype.element_size();
        let bytes = self
            .zip
            .read(&format!("{}/data/{}", self.root, tensor.storage_key))?;
        let numel = tensor.shape.iter().product::<usize>();
        let byte_offset = tensor
            .offset
            .checked_mul(element_size)
            .ok_or_else(|| err(format!("tensor {name}: byte offset overflow")))?;
        let byte_len = numel
            .checked_mul(element_size)
            .ok_or_else(|| err(format!("tensor {name}: byte length overflow")))?;

        let mut expected_stride = 1usize;
        let contiguous = tensor
            .shape
            .iter()
            .zip(&tensor.stride)
            .rev()
            .all(|(&dimension, &stride)| {
                let matches = dimension <= 1 || stride == expected_stride;
                expected_stride = expected_stride.saturating_mul(dimension);
                matches
            });
        if contiguous {
            let end = byte_offset
                .checked_add(byte_len)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| err(format!("tensor {name}: contiguous bytes exceed storage")))?;
            return Ok(bytes[byte_offset..end].to_vec());
        }

        let mut out = Vec::with_capacity(byte_len);
        if tensor.shape.is_empty() {
            let end = byte_offset + element_size;
            let value = bytes
                .get(byte_offset..end)
                .ok_or_else(|| err(format!("tensor {name}: scalar offset out of range")))?;
            out.extend_from_slice(value);
            return Ok(out);
        }
        let dimensions = tensor.shape.len();
        let mut index = vec![0usize; dimensions];
        'walk: loop {
            let mut element_offset = tensor.offset;
            for dimension in 0..dimensions {
                element_offset += index[dimension] * tensor.stride[dimension];
            }
            let start = element_offset
                .checked_mul(element_size)
                .ok_or_else(|| err(format!("tensor {name}: element offset overflow")))?;
            let end = start + element_size;
            out.extend_from_slice(
                bytes
                    .get(start..end)
                    .ok_or_else(|| err(format!("tensor {name}: element exceeds storage")))?,
            );
            for dimension in (0..dimensions).rev() {
                index[dimension] += 1;
                if index[dimension] < tensor.shape[dimension] {
                    continue 'walk;
                }
                index[dimension] = 0;
            }
            break;
        }
        Ok(out)
    }

    /// Reads a tensor as contiguous f32 (row-major over its shape).
    pub fn f32(&mut self, name: &str) -> Result<Vec<f32>> {
        let t = self
            .tensors
            .get(name)
            .ok_or_else(|| err(format!("tensor missing: {name}")))?
            .clone();
        let bytes = self.zip.read(&format!("{}/data/{}", self.root, t.storage_key))?;
        let elems: Vec<f32> = match t.storage_dtype.as_str() {
            "FloatStorage" => bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            "HalfStorage" => bytes
                .chunks_exact(2)
                .map(|c| half_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect(),
            "BFloat16Storage" => bytes
                .chunks_exact(2)
                .map(|c| f32::from_bits(u32::from(u16::from_le_bytes([c[0], c[1]])) << 16))
                .collect(),
            other => return Err(err(format!("tensor {name}: storage dtype {other}"))),
        };
        let numel: usize = t.shape.iter().product();
        if t.shape.is_empty() {
            return Ok(vec![*elems
                .get(t.offset)
                .ok_or_else(|| err(format!("tensor {name}: scalar offset out of range")))?]);
        }
        // Materialize contiguous via stride walk.
        let mut out = Vec::with_capacity(numel);
        let dims = t.shape.len();
        let mut idx = vec![0usize; dims];
        'walk: loop {
            let mut off = t.offset;
            for d in 0..dims {
                off += idx[d] * t.stride[d];
            }
            out.push(*elems.get(off).ok_or_else(|| {
                err(format!("tensor {name}: element offset {off} out of storage"))
            })?);
            // increment multi-index
            for d in (0..dims).rev() {
                idx[d] += 1;
                if idx[d] < t.shape[d] {
                    continue 'walk;
                }
                idx[d] = 0;
            }
            break;
        }
        Ok(out)
    }

    /// f32 tensor with an exact-shape check.
    pub fn f32_shaped(&mut self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let actual = self.shape(name)?;
        if actual != shape {
            return Err(err(format!(
                "tensor {name}: shape {actual:?}, expected {shape:?}"
            )));
        }
        self.f32(name)
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = (h >> 15) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // subnormal
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}
