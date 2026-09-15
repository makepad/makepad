//! Convert upstream PyTorch Kokoro weights (`kokoro-v1_0.pth` model,
//! `voices/*.pt` voice packs) into the flat container `kokoro::weights`
//! reads (`.mktts` / `.mkvoice` — same format, one `style` tensor for a
//! voice). Byte-for-byte the file `tools/convert_kokoro.py` writes, so
//! either converter can produce the cache; keep the two in sync.
//!
//! This lets a node download the upstream weights from HuggingFace and
//! convert them itself — no Python on the box, no hand-carried files.
//!
//! A `.pth` is a ZIP of STORED (uncompressed) entries around a pickle whose
//! tensors are `persistent_id` references into raw little-endian storage
//! records. The pickle reader below is a constrained VM over exactly the
//! opcode subset torch emits for nested dicts of tensors: REDUCE of anything
//! but `_rebuild_tensor_v2` / `_rebuild_parameter` / `OrderedDict` is refused
//! loudly and nothing is ever executed. Tensor bytes are copied verbatim
//! (dtype stays f32; weight-norm `weight_g`/`weight_v` pairs are left intact
//! — the Rust loader reconstructs `W = g*v/||v||`).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::TtsError;

const MAGIC: &[u8] = b"MKTTS\0\0\0";
const VERSION: u32 = 1;
const ALIGN: u64 = 32;

fn bad(message: impl Into<String>) -> TtsError {
    TtsError::Backend(message.into())
}

/// What a conversion produced, for logs/progress.
#[derive(Clone, Copy, Debug)]
pub struct ConvertReport {
    pub tensors: usize,
    /// Total parameter count (sum of tensor element counts).
    pub params: usize,
    /// Size of the written file in bytes.
    pub bytes: u64,
}

/// Convert a model checkpoint (`kokoro-v1_0.pth`) to `.mktts`.
pub fn convert_pth_to_mktts(src: &Path, dest: &Path) -> Result<ConvertReport, TtsError> {
    convert_torch_weights(src, dest, &mut |_, _| {})
}

/// Convert a single-voice pack (`voices/<name>.pt`, a bare `[510,1,256]`
/// tensor) to `.mkvoice`. The container is identical to `.mktts`; the one
/// tensor is named `style`.
pub fn convert_pt_to_mkvoice(src: &Path, dest: &Path) -> Result<ConvertReport, TtsError> {
    convert_torch_weights(src, dest, &mut |_, _| {})
}

/// [`convert_pth_to_mktts`] with a per-tensor `progress(done, total)`
/// callback (548 tensors for the model — a service can show real progress).
/// Handles both layouts: a nested state dict (model) or a bare tensor
/// (voice pack). Writes `<dest>.tmp` then renames, so a crash never leaves
/// a truncated file at the final path.
pub fn convert_torch_weights(
    src: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<ConvertReport, TtsError> {
    let context = |e: TtsError| match e {
        TtsError::Backend(m) => bad(format!("{}: {m}", src.display())),
        other => other,
    };
    convert_inner(src, dest, progress).map_err(context)
}

fn convert_inner(
    src: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<ConvertReport, TtsError> {
    let mut zip = ZipArchive::open(src)?;
    let prefix = zip.pkl_prefix()?;
    let pkl = zip.read(&format!("{prefix}data.pkl"))?;
    let root = unpickle(&pkl)?;
    let tensors = collect(&root)?;
    if tensors.is_empty() {
        return Err(bad("no tensors found"));
    }

    // Validate every tensor against its storage before writing anything.
    for (name, tensor) in &tensors {
        if tensor.storage_dtype != "FloatStorage" {
            return Err(bad(format!(
                "{name}: unsupported dtype {}; expected all-f32 weights",
                tensor.storage_dtype
            )));
        }
        if !tensor.is_contiguous() {
            return Err(bad(format!(
                "{name}: non-contiguous tensor (shape {:?}, stride {:?})",
                tensor.shape, tensor.stride
            )));
        }
        if tensor.shape.len() > 255 {
            return Err(bad(format!("{name}: {} dims", tensor.shape.len())));
        }
        if tensor.shape.iter().any(|&d| d > u32::MAX as usize) {
            return Err(bad(format!("{name}: dimension exceeds u32")));
        }
        let entry = zip.entry_size(&format!("{prefix}data/{}", tensor.storage_key))?;
        let end = (tensor.offset + tensor.numel()) as u64 * 4;
        if end > entry {
            return Err(bad(format!(
                "{name}: tensor overruns its storage ({end} > {entry})"
            )));
        }
    }

    // Lay out the index first so offsets are known before anything is
    // written — the exact algorithm of convert_kokoro.py, for byte-identical
    // output.
    let mut index_size: u64 = 4 + 4; // version + count (magic written separately)
    for (name, tensor) in &tensors {
        index_size += 4 + name.len() as u64 + 1 + 1 + 4 * tensor.shape.len() as u64 + 8 + 8;
    }
    let mut at = MAGIC.len() as u64 + index_size;
    let mut offsets = Vec::with_capacity(tensors.len());
    for (_, tensor) in &tensors {
        at = at.div_ceil(ALIGN) * ALIGN;
        offsets.push(at);
        at += tensor.numel() as u64 * 4;
    }
    let total_bytes = at;

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| bad(format!("mkdir {}: {e}", parent.display())))?;
        }
    }
    let mut tmp_os = dest.as_os_str().to_os_string();
    tmp_os.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_os);
    let file =
        File::create(&tmp).map_err(|e| bad(format!("create {}: {e}", tmp.display())))?;
    let mut out = BufWriter::new(file);
    let write = |out: &mut BufWriter<File>, bytes: &[u8]| {
        out.write_all(bytes)
            .map_err(|e| bad(format!("write {}: {e}", tmp.display())))
    };

    write(&mut out, MAGIC)?;
    write(&mut out, &VERSION.to_le_bytes())?;
    write(&mut out, &(tensors.len() as u32).to_le_bytes())?;
    for ((name, tensor), offset) in tensors.iter().zip(&offsets) {
        write(&mut out, &(name.len() as u32).to_le_bytes())?;
        write(&mut out, name.as_bytes())?;
        write(&mut out, &[0u8, tensor.shape.len() as u8])?;
        for &dim in &tensor.shape {
            write(&mut out, &(dim as u32).to_le_bytes())?;
        }
        write(&mut out, &offset.to_le_bytes())?;
        write(&mut out, &(tensor.numel() as u64 * 4).to_le_bytes())?;
    }

    let total = tensors.len();
    let mut params = 0usize;
    let mut pos = MAGIC.len() as u64 + index_size;
    progress(0, total);
    for (index, ((name, tensor), offset)) in tensors.iter().zip(&offsets).enumerate() {
        write(&mut out, &vec![0u8; (offset - pos) as usize])?;
        let payload = zip
            .read_range(
                &format!("{prefix}data/{}", tensor.storage_key),
                tensor.offset as u64 * 4,
                tensor.numel() as u64 * 4,
            )
            .map_err(|e| match e {
                TtsError::Backend(m) => bad(format!("{name}: {m}")),
                other => other,
            })?;
        write(&mut out, &payload)?;
        pos = offset + payload.len() as u64;
        params += tensor.numel();
        progress(index + 1, total);
    }
    out.flush()
        .map_err(|e| bad(format!("flush {}: {e}", tmp.display())))?;
    drop(out);

    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(&tmp, dest).map_err(|e| {
        bad(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            dest.display()
        ))
    })?;
    Ok(ConvertReport {
        tensors: total,
        params,
        bytes: total_bytes,
    })
}

// ---------------------------------------------------------------------------
// ZIP reading (stored entries only — torch.save always writes ZIP_STORED)
// ---------------------------------------------------------------------------

struct ZipEntry {
    /// Absolute byte offset of the entry's data.
    data_offset: u64,
    size: u64,
}

struct ZipArchive {
    file: File,
    entries: HashMap<String, ZipEntry>,
}

impl ZipArchive {
    fn open(path: &Path) -> Result<Self, TtsError> {
        let mut file = File::open(path).map_err(|e| bad(format!("open: {e}")))?;
        let file_len = file
            .seek(SeekFrom::End(0))
            .map_err(|e| bad(format!("seek: {e}")))?;
        // Find the end-of-central-directory record (PK\x05\x06) in the
        // trailing 66KB (the comment field caps at 64KB).
        let tail_len = file_len.min(66 * 1024);
        file.seek(SeekFrom::Start(file_len - tail_len))
            .map_err(|e| bad(format!("seek: {e}")))?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail)
            .map_err(|e| bad(format!("read: {e}")))?;
        let eocd = tail
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .ok_or_else(|| bad("not a zip archive (no end-of-central-directory)"))?;
        let record = &tail[eocd..];
        if record.len() < 22 {
            return Err(bad("truncated zip end-of-central-directory"));
        }
        let cd_size = u32::from_le_bytes(record[12..16].try_into().unwrap()) as u64;
        let cd_offset = u32::from_le_bytes(record[16..20].try_into().unwrap()) as u64;
        if cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF {
            return Err(bad("zip64 archives are not supported"));
        }
        file.seek(SeekFrom::Start(cd_offset))
            .map_err(|e| bad(format!("seek: {e}")))?;
        let mut cd = vec![0u8; cd_size as usize];
        file.read_exact(&mut cd)
            .map_err(|e| bad(format!("central directory read: {e}")))?;

        // Central directory names + sizes, then each entry's data offset via
        // its local header (whose extra-field length differs from the central
        // one — torch pads storages to 64-byte alignment there).
        let mut headers = Vec::new();
        let mut pos = 0usize;
        while pos + 46 <= cd.len() {
            if cd[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
                break;
            }
            let method = u16::from_le_bytes([cd[pos + 10], cd[pos + 11]]);
            let comp_size =
                u32::from_le_bytes(cd[pos + 20..pos + 24].try_into().unwrap()) as u64;
            let name_len = u16::from_le_bytes([cd[pos + 28], cd[pos + 29]]) as usize;
            let extra_len = u16::from_le_bytes([cd[pos + 30], cd[pos + 31]]) as usize;
            let comment_len = u16::from_le_bytes([cd[pos + 32], cd[pos + 33]]) as usize;
            let local_offset =
                u32::from_le_bytes(cd[pos + 42..pos + 46].try_into().unwrap()) as u64;
            if pos + 46 + name_len > cd.len() {
                return Err(bad("truncated central directory entry"));
            }
            let name = std::str::from_utf8(&cd[pos + 46..pos + 46 + name_len])
                .map_err(|_| bad("zip entry name is not utf8"))?
                .to_string();
            if method != 0 {
                return Err(bad(format!(
                    "entry {name}: compressed (method {method}), expected stored — not a torch archive?"
                )));
            }
            headers.push((name, local_offset, comp_size));
            pos += 46 + name_len + extra_len + comment_len;
        }
        let mut entries = HashMap::with_capacity(headers.len());
        for (name, local_offset, size) in headers {
            let mut header = [0u8; 30];
            file.seek(SeekFrom::Start(local_offset))
                .map_err(|e| bad(format!("seek: {e}")))?;
            file.read_exact(&mut header)
                .map_err(|e| bad(format!("local header read: {e}")))?;
            if header[0..4] != [0x50, 0x4b, 0x03, 0x04] {
                return Err(bad(format!("entry {name}: bad local header")));
            }
            let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
            let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
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

    /// The archive-internal prefix before `data.pkl` (torch names the root
    /// after the file: `model/data.pkl`, `af_heart/data.pkl`, ...).
    fn pkl_prefix(&self) -> Result<String, TtsError> {
        for name in self.entries.keys() {
            if let Some(prefix) = name.strip_suffix("data.pkl") {
                return Ok(prefix.to_string());
            }
        }
        Err(bad("no data.pkl in archive — not a torch checkpoint"))
    }

    fn entry_size(&self, name: &str) -> Result<u64, TtsError> {
        Ok(self
            .entries
            .get(name)
            .ok_or_else(|| bad(format!("zip entry missing: {name}")))?
            .size)
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>, TtsError> {
        let size = self.entry_size(name)?;
        self.read_range(name, 0, size)
    }

    /// Read `len` bytes at `start` within an entry's data — storage records
    /// are uncompressed, so a tensor's bytes can be pulled without loading
    /// the whole storage.
    fn read_range(&mut self, name: &str, start: u64, len: u64) -> Result<Vec<u8>, TtsError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| bad(format!("zip entry missing: {name}")))?;
        if start + len > entry.size {
            return Err(bad(format!(
                "zip entry {name}: range {start}+{len} beyond size {}",
                entry.size
            )));
        }
        self.file
            .seek(SeekFrom::Start(entry.data_offset + start))
            .map_err(|e| bad(format!("seek: {e}")))?;
        let mut buf = vec![0u8; len as usize];
        self.file
            .read_exact(&mut buf)
            .map_err(|e| bad(format!("read {name}: {e}")))?;
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// Pickle VM (constrained torch-checkpoint subset; nothing is executed)
// ---------------------------------------------------------------------------

/// A tensor as pickled: a view into a named storage record.
#[derive(Clone, Debug)]
struct PthTensor {
    storage_dtype: String,
    storage_key: String,
    /// Element (not byte) offset into the storage.
    offset: usize,
    shape: Vec<usize>,
    stride: Vec<usize>,
}

impl PthTensor {
    fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// C-contiguous per PyTorch's definition (size-1 dims have don't-care
    /// strides). The verbatim byte copy is only correct for contiguous
    /// tensors; upstream Kokoro has no other kind.
    fn is_contiguous(&self) -> bool {
        let mut expected = 1usize;
        for (dim, stride) in self.shape.iter().zip(&self.stride).rev() {
            if *dim != 1 && *stride != expected {
                return false;
            }
            expected *= *dim;
        }
        true
    }
}

#[derive(Clone, Debug)]
enum Value {
    None,
    Bool(#[allow(dead_code)] bool),
    Int(i64),
    Str(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    /// Insertion-ordered — tensor order in the output must match the pickle
    /// stream (the Python converter walks Python dicts, which preserve
    /// insertion order).
    Dict(Vec<(Value, Value)>),
    Global(String, String),
    Tensor(PthTensor),
    Mark,
}

struct PickleCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PickleCursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], TtsError> {
        if self.pos + n > self.data.len() {
            return Err(bad("pickle truncated"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, TtsError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, TtsError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, TtsError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, TtsError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// A newline-terminated line (GLOBAL's module/name arguments).
    fn line(&mut self) -> Result<String, TtsError> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(bad("pickle GLOBAL truncated"));
        }
        let text = std::str::from_utf8(&self.data[start..self.pos])
            .map_err(|_| bad("pickle GLOBAL is not utf8"))?
            .to_string();
        self.pos += 1;
        Ok(text)
    }

    fn str(&mut self, n: usize) -> Result<Value, TtsError> {
        Ok(Value::Str(
            std::str::from_utf8(self.take(n)?)
                .map_err(|_| bad("pickle string is not utf8"))?
                .to_string(),
        ))
    }
}

fn pop(stack: &mut Vec<Value>) -> Result<Value, TtsError> {
    stack.pop().ok_or_else(|| bad("pickle stack underflow"))
}

/// Pop values down to the innermost MARK.
fn pop_to_mark(stack: &mut Vec<Value>) -> Result<Vec<Value>, TtsError> {
    let mut items = Vec::new();
    loop {
        match pop(stack)? {
            Value::Mark => break,
            value => items.push(value),
        }
    }
    items.reverse();
    Ok(items)
}

/// The persistent-id tuple torch writes:
/// `('storage', <StorageClass>, key, location, numel)`.
fn persistent_load(pid: Value) -> Result<Value, TtsError> {
    let items = match pid {
        Value::Tuple(items) => items,
        other => return Err(bad(format!("persistent id is not a tuple: {other:?}"))),
    };
    if items.len() < 5 {
        return Err(bad(format!("persistent id arity {} < 5", items.len())));
    }
    match &items[0] {
        Value::Str(tag) if tag == "storage" => {}
        other => return Err(bad(format!("persistent id tag {other:?}"))),
    }
    let dtype = match &items[1] {
        Value::Global(_, name) if name.ends_with("Storage") => name.clone(),
        other => return Err(bad(format!("persistent id storage class {other:?}"))),
    };
    let key = match &items[2] {
        Value::Str(key) => key.clone(),
        other => return Err(bad(format!("persistent id storage key {other:?}"))),
    };
    // items[3] is the device location ("cpu"), irrelevant here; items[4] is
    // the element count, carried on the tensor's storage bounds check side
    // via the zip entry size instead.
    let _ = &items[3];
    Ok(Value::Tuple(vec![
        Value::Str(dtype),
        Value::Str(key),
    ]))
}

fn as_usize_vec(value: &Value) -> Result<Vec<usize>, TtsError> {
    match value {
        Value::Tuple(items) | Value::List(items) => items
            .iter()
            .map(|item| match item {
                Value::Int(i) if *i >= 0 => Ok(*i as usize),
                other => Err(bad(format!("expected non-negative int, got {other:?}"))),
            })
            .collect(),
        other => Err(bad(format!("expected tuple of ints, got {other:?}"))),
    }
}

/// REDUCE — the only three callables a torch weight checkpoint names.
fn reduce(callable: Value, args: Value) -> Result<Value, TtsError> {
    let (module, name) = match &callable {
        Value::Global(module, name) => (module.as_str(), name.as_str()),
        other => return Err(bad(format!("REDUCE of non-global {other:?}"))),
    };
    let args = match args {
        Value::Tuple(items) => items,
        other => return Err(bad(format!("REDUCE args not a tuple: {other:?}"))),
    };
    match (module, name) {
        ("torch._utils", "_rebuild_tensor_v2") => {
            if args.len() < 4 {
                return Err(bad("_rebuild_tensor_v2 arity < 4"));
            }
            let (dtype, key) = match &args[0] {
                Value::Tuple(items) if items.len() == 2 => match (&items[0], &items[1]) {
                    (Value::Str(dtype), Value::Str(key)) => (dtype.clone(), key.clone()),
                    other => return Err(bad(format!("rebuild storage {other:?}"))),
                },
                other => return Err(bad(format!("rebuild storage {other:?}"))),
            };
            let offset = match &args[1] {
                Value::Int(i) if *i >= 0 => *i as usize,
                other => return Err(bad(format!("rebuild offset {other:?}"))),
            };
            Ok(Value::Tensor(PthTensor {
                storage_dtype: dtype,
                storage_key: key,
                offset,
                shape: as_usize_vec(&args[2])?,
                stride: as_usize_vec(&args[3])?,
            }))
        }
        // Parameter wraps a tensor; unwrap it.
        ("torch._utils", "_rebuild_parameter") => match args.into_iter().next() {
            Some(data @ Value::Tensor(_)) => Ok(data),
            other => Err(bad(format!("_rebuild_parameter of {other:?}"))),
        },
        ("collections", "OrderedDict") => {
            if !args.is_empty() {
                return Err(bad("OrderedDict with constructor args"));
            }
            Ok(Value::Dict(Vec::new()))
        }
        _ => Err(bad(format!("REDUCE of unsupported callable {module}.{name}"))),
    }
}

fn unpickle(data: &[u8]) -> Result<Value, TtsError> {
    let mut cursor = PickleCursor { data, pos: 0 };
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<u32, Value> = HashMap::new();

    let memo_put = |memo: &mut HashMap<u32, Value>,
                    stack: &Vec<Value>,
                    key: u32|
     -> Result<(), TtsError> {
        let top = stack.last().ok_or_else(|| bad("memo of empty stack"))?;
        memo.insert(key, top.clone());
        Ok(())
    };

    loop {
        let op = cursor.u8()?;
        match op {
            0x80 => {
                cursor.u8()?; // PROTO
            }
            0x95 => {
                cursor.take(8)?; // FRAME (protocol 4)
            }
            b'.' => return pop(&mut stack), // STOP
            b'(' => stack.push(Value::Mark),
            b'}' => stack.push(Value::Dict(Vec::new())),
            b']' => stack.push(Value::List(Vec::new())),
            b')' => stack.push(Value::Tuple(Vec::new())),
            b'N' => stack.push(Value::None),
            0x88 => stack.push(Value::Bool(true)),
            0x89 => stack.push(Value::Bool(false)),
            b'K' => {
                let v = cursor.u8()?;
                stack.push(Value::Int(v as i64));
            }
            b'M' => {
                let v = cursor.u16()?;
                stack.push(Value::Int(v as i64));
            }
            b'J' => {
                let v = cursor.i32()?;
                stack.push(Value::Int(v as i64));
            }
            0x8a => {
                // LONG1: little-endian two's-complement of n bytes
                let n = cursor.u8()? as usize;
                let bytes = cursor.take(n)?;
                let mut v: i64 = 0;
                if n > 8 {
                    return Err(bad("LONG1 wider than 64 bits"));
                }
                for (i, &b) in bytes.iter().enumerate() {
                    v |= (b as i64) << (8 * i);
                }
                if n > 0 && n < 8 && bytes[n - 1] & 0x80 != 0 {
                    v -= 1i64 << (8 * n);
                }
                stack.push(Value::Int(v));
            }
            b'X' => {
                // BINUNICODE
                let n = cursor.u32()? as usize;
                let s = cursor.str(n)?;
                stack.push(s);
            }
            0x8c => {
                // SHORT_BINUNICODE (protocol 4)
                let n = cursor.u8()? as usize;
                let s = cursor.str(n)?;
                stack.push(s);
            }
            b'c' => {
                // GLOBAL: two newline-terminated lines
                let module = cursor.line()?;
                let name = cursor.line()?;
                stack.push(Value::Global(module, name));
            }
            0x93 => {
                // STACK_GLOBAL (protocol 4)
                let name = pop(&mut stack)?;
                let module = pop(&mut stack)?;
                match (module, name) {
                    (Value::Str(m), Value::Str(n)) => stack.push(Value::Global(m, n)),
                    _ => return Err(bad("STACK_GLOBAL of non-strings")),
                }
            }
            b'q' => {
                let key = cursor.u8()? as u32;
                memo_put(&mut memo, &stack, key)?;
            }
            b'r' => {
                let key = cursor.u32()?;
                memo_put(&mut memo, &stack, key)?;
            }
            0x94 => {
                // MEMOIZE (protocol 4): next sequential key
                let key = memo.len() as u32;
                memo_put(&mut memo, &stack, key)?;
            }
            b'h' => {
                let key = cursor.u8()? as u32;
                let value = memo
                    .get(&key)
                    .ok_or_else(|| bad("BINGET of missing memo"))?
                    .clone();
                stack.push(value);
            }
            b'j' => {
                let key = cursor.u32()?;
                let value = memo
                    .get(&key)
                    .ok_or_else(|| bad("LONG_BINGET of missing memo"))?
                    .clone();
                stack.push(value);
            }
            0x85 => {
                let a = pop(&mut stack)?;
                stack.push(Value::Tuple(vec![a]));
            }
            0x86 => {
                let b = pop(&mut stack)?;
                let a = pop(&mut stack)?;
                stack.push(Value::Tuple(vec![a, b]));
            }
            0x87 => {
                let c = pop(&mut stack)?;
                let b = pop(&mut stack)?;
                let a = pop(&mut stack)?;
                stack.push(Value::Tuple(vec![a, b, c]));
            }
            b't' => {
                let items = pop_to_mark(&mut stack)?;
                stack.push(Value::Tuple(items));
            }
            b'Q' => {
                // BINPERSID
                let pid = pop(&mut stack)?;
                stack.push(persistent_load(pid)?);
            }
            b'R' => {
                let args = pop(&mut stack)?;
                let callable = pop(&mut stack)?;
                stack.push(reduce(callable, args)?);
            }
            b's' => {
                // SETITEM
                let value = pop(&mut stack)?;
                let key = pop(&mut stack)?;
                match stack.last_mut() {
                    Some(Value::Dict(items)) => items.push((key, value)),
                    other => return Err(bad(format!("SETITEM into {other:?}"))),
                }
            }
            b'u' => {
                // SETITEMS
                let kvs = pop_to_mark(&mut stack)?;
                if kvs.len() % 2 != 0 {
                    return Err(bad("SETITEMS with odd item count"));
                }
                match stack.last_mut() {
                    Some(Value::Dict(items)) => {
                        let mut it = kvs.into_iter();
                        while let (Some(k), Some(v)) = (it.next(), it.next()) {
                            items.push((k, v));
                        }
                    }
                    other => return Err(bad(format!("SETITEMS into {other:?}"))),
                }
            }
            b'a' => {
                // APPEND
                let value = pop(&mut stack)?;
                match stack.last_mut() {
                    Some(Value::List(items)) => items.push(value),
                    other => return Err(bad(format!("APPEND into {other:?}"))),
                }
            }
            b'e' => {
                // APPENDS
                let values = pop_to_mark(&mut stack)?;
                match stack.last_mut() {
                    Some(Value::List(items)) => items.extend(values),
                    other => return Err(bad(format!("APPENDS into {other:?}"))),
                }
            }
            b'b' => {
                // BUILD: obj.__setstate__(state). For the OrderedDicts in a
                // torch checkpoint the state is `{'_metadata': ...}`, which
                // Python puts on the instance __dict__ — invisible to a dict
                // items walk. Discard it (do NOT merge into the dict items;
                // the Python converter never sees it either).
                let state = pop(&mut stack)?;
                match (&state, stack.last()) {
                    (Value::Dict(_) | Value::None, Some(_)) => {}
                    (state, top) => {
                        return Err(bad(format!("BUILD of {state:?} onto {top:?}")));
                    }
                }
            }
            other => {
                return Err(bad(format!(
                    "unsupported pickle opcode 0x{other:02x} at {}",
                    cursor.pos - 1
                )));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Walk: (dotted_name, tensor) in pickle-stream order
// ---------------------------------------------------------------------------

fn collect(root: &Value) -> Result<Vec<(String, PthTensor)>, TtsError> {
    // A voice pack is a bare tensor; name it `style` (what the loader reads).
    if let Value::Tensor(tensor) = root {
        return Ok(vec![("style".to_string(), tensor.clone())]);
    }
    let mut out = Vec::new();
    walk(root, "", &mut out)?;
    Ok(out)
}

fn walk(
    node: &Value,
    prefix: &str,
    out: &mut Vec<(String, PthTensor)>,
) -> Result<(), TtsError> {
    match node {
        Value::Tensor(tensor) => out.push((prefix.to_string(), tensor.clone())),
        Value::Dict(items) => {
            for (key, value) in items {
                let key = match key {
                    Value::Str(s) => s.clone(),
                    Value::Int(i) => i.to_string(),
                    other => return Err(bad(format!("unsupported dict key {other:?}"))),
                };
                let child = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                walk(value, &child, out)?;
            }
        }
        // Scalars, lists, globals: no weights beneath (matches the Python
        // converter's walk, which only descends dicts).
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests: crafted archives — the zip and pickle bytes are hand-assembled so
// no torch, network, or checked-in binary is involved.
// ---------------------------------------------------------------------------

// The round trip reads the result back through Kokoro's loader, so these
// tests need that engine compiled in.
#[cfg(all(test, feature = "kokoro"))]
mod tests {
    use super::*;
    use crate::kokoro::weights::Weights;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "makepad-tts-convert-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- minimal stored-zip writer ------------------------------------------

    fn stored_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let local_offset = out.len() as u32;
            // Local file header.
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
            out.extend_from_slice(&0u32.to_le_bytes()); // time+date
            out.extend_from_slice(&0u32.to_le_bytes()); // crc (unchecked)
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            // Central directory record.
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes()); // made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // method
            central.extend_from_slice(&0u32.to_le_bytes()); // time+date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&local_offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        // End of central directory.
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out
    }

    // -- minimal pickle emitter ----------------------------------------------

    struct Pickle(Vec<u8>);

    impl Pickle {
        fn new() -> Self {
            Self(vec![0x80, 0x02]) // PROTO 2
        }

        fn global(mut self, module: &str, name: &str) -> Self {
            self.0.push(b'c');
            self.0.extend_from_slice(module.as_bytes());
            self.0.push(b'\n');
            self.0.extend_from_slice(name.as_bytes());
            self.0.push(b'\n');
            self
        }

        fn string(mut self, s: &str) -> Self {
            self.0.push(b'X');
            self.0
                .extend_from_slice(&(s.len() as u32).to_le_bytes());
            self.0.extend_from_slice(s.as_bytes());
            self
        }

        fn int(mut self, v: u32) -> Self {
            self.0.push(b'J');
            self.0.extend_from_slice(&(v as i32).to_le_bytes());
            self
        }

        fn op(mut self, op: u8) -> Self {
            self.0.push(op);
            self
        }

        /// A `_rebuild_tensor_v2` call for `storage_key` with dtype class
        /// `storage_class`, element offset, shape and stride.
        fn tensor(
            self,
            storage_class: &str,
            storage_key: &str,
            numel: u32,
            offset: u32,
            shape: &[u32],
            stride: &[u32],
        ) -> Self {
            let mut p = self
                .global("torch._utils", "_rebuild_tensor_v2")
                .op(b'(') // args mark
                .op(b'(') // pid mark
                .string("storage")
                .global("torch", storage_class)
                .string(storage_key)
                .string("cpu")
                .int(numel)
                .op(b't') // pid tuple
                .op(b'Q') // BINPERSID
                .int(offset);
            for &d in shape {
                p = p.int(d);
            }
            p = p.op(match shape.len() {
                1 => 0x85,
                2 => 0x86,
                3 => 0x87,
                _ => panic!("test helper supports 1-3 dims"),
            });
            for &s in stride {
                p = p.int(s);
            }
            p = p.op(match stride.len() {
                1 => 0x85,
                2 => 0x86,
                3 => 0x87,
                _ => panic!("test helper supports 1-3 dims"),
            });
            p.op(0x89) // requires_grad = False
                .global("collections", "OrderedDict")
                .op(b')')
                .op(b'R') // backward hooks
                .op(b't') // close args tuple
                .op(b'R') // REDUCE _rebuild_tensor_v2
        }

        fn stop(mut self) -> Vec<u8> {
            self.0.push(b'.');
            self.0
        }
    }

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn convert_bytes(dir: &Path, archive: &[u8], name: &str) -> Result<PathBuf, TtsError> {
        let src = dir.join(name);
        std::fs::write(&src, archive).unwrap();
        let dest = dir.join(format!("{name}.mktts"));
        convert_pth_to_mktts(&src, &dest)?;
        Ok(dest)
    }

    /// Read back the index names in FILE order (Weights uses a HashMap, so
    /// order must be checked on the raw bytes).
    fn index_names(path: &Path) -> Vec<String> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 1);
        let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let mut names = Vec::new();
        let mut at = 16;
        for _ in 0..count {
            let name_len =
                u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            names.push(String::from_utf8(bytes[at..at + name_len].to_vec()).unwrap());
            at += name_len;
            let ndim = bytes[at + 1] as usize;
            at += 2 + 4 * ndim + 16;
        }
        names
    }

    #[test]
    fn nested_dict_with_shared_storage_round_trips() {
        let dir = test_dir("nested");
        // Root {"a": {"weight": [2,2] at offset 2}, "b": [2] at offset 0},
        // both views of one 6-element storage — like torch's shared storages.
        let pickle = Pickle::new()
            .op(b'}') // root dict
            .string("a")
            .op(b'}') // inner dict
            .string("weight")
            .tensor("FloatStorage", "0", 6, 2, &[2, 2], &[2, 1])
            .op(b's') // inner["weight"] = tensor
            .op(b's') // root["a"] = inner
            .string("b")
            .tensor("FloatStorage", "0", 6, 0, &[2], &[1])
            .op(b's')
            .stop();
        let archive = stored_zip(&[
            ("model/data.pkl", pickle),
            ("model/data/0", f32_bytes(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
        ]);
        let dest = convert_bytes(&dir, &archive, "nested.pth").unwrap();

        // Pickle-stream order, dotted names.
        assert_eq!(index_names(&dest), vec!["a.weight", "b"]);

        let weights = Weights::load(dest.to_str().unwrap()).unwrap();
        assert_eq!(weights.len(), 2);
        assert_eq!(weights.shape("a.weight").unwrap(), &[2, 2]);
        assert_eq!(weights.get("a.weight").unwrap(), &[3.0, 4.0, 5.0, 6.0]);
        assert_eq!(weights.get("b").unwrap(), &[1.0, 2.0]);
    }

    #[test]
    fn bare_tensor_root_becomes_style() {
        let dir = test_dir("voice");
        // A voice pack: the pickle root IS the tensor (shape [2,1,3]).
        let pickle = Pickle::new()
            .tensor("FloatStorage", "0", 6, 0, &[2, 1, 3], &[3, 3, 1])
            .stop();
        let archive = stored_zip(&[
            ("af_test/data.pkl", pickle),
            ("af_test/data/0", f32_bytes(&[0.5, 1.5, 2.5, 3.5, 4.5, 5.5])),
        ]);
        let src = dir.join("af_test.pt");
        std::fs::write(&src, &archive).unwrap();
        let dest = dir.join("af_test.mkvoice");
        let report = convert_pt_to_mkvoice(&src, &dest).unwrap();
        assert_eq!(report.tensors, 1);
        assert_eq!(report.params, 6);

        let weights = Weights::load(dest.to_str().unwrap()).unwrap();
        assert_eq!(weights.shape("style").unwrap(), &[2, 1, 3]);
        assert_eq!(
            weights.get("style").unwrap(),
            &[0.5, 1.5, 2.5, 3.5, 4.5, 5.5]
        );
    }

    #[test]
    fn payloads_are_32_byte_aligned() {
        let dir = test_dir("align");
        let pickle = Pickle::new()
            .op(b'}')
            .string("x")
            .tensor("FloatStorage", "0", 3, 0, &[3], &[1])
            .op(b's')
            .string("y")
            .tensor("FloatStorage", "1", 2, 0, &[2], &[1])
            .op(b's')
            .stop();
        let archive = stored_zip(&[
            ("m/data.pkl", pickle),
            ("m/data/0", f32_bytes(&[1.0, 2.0, 3.0])),
            ("m/data/1", f32_bytes(&[7.0, 8.0])),
        ]);
        let dest = convert_bytes(&dir, &archive, "align.pth").unwrap();
        let bytes = std::fs::read(&dest).unwrap();
        // Walk the index and check every offset.
        let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let mut at = 16;
        for _ in 0..count {
            let name_len =
                u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            at += 4 + name_len;
            let ndim = bytes[at + 1] as usize;
            at += 2 + 4 * ndim;
            let offset = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            assert_eq!(offset % 32, 0, "tensor payload not 32-byte aligned");
            at += 16;
        }
    }

    #[test]
    fn non_f32_storage_is_rejected() {
        let dir = test_dir("dtype");
        let pickle = Pickle::new()
            .op(b'}')
            .string("x")
            .tensor("DoubleStorage", "0", 2, 0, &[2], &[1])
            .op(b's')
            .stop();
        let archive = stored_zip(&[
            ("m/data.pkl", pickle),
            ("m/data/0", vec![0u8; 16]),
        ]);
        let err = convert_bytes(&dir, &archive, "f64.pth").unwrap_err();
        assert!(format!("{err:?}").contains("dtype"), "{err:?}");
    }

    #[test]
    fn non_contiguous_tensor_is_rejected() {
        let dir = test_dir("stride");
        let pickle = Pickle::new()
            .op(b'}')
            .string("x")
            // Transposed view: shape [2,2], stride [1,2].
            .tensor("FloatStorage", "0", 4, 0, &[2, 2], &[1, 2])
            .op(b's')
            .stop();
        let archive = stored_zip(&[
            ("m/data.pkl", pickle),
            ("m/data/0", f32_bytes(&[1.0, 2.0, 3.0, 4.0])),
        ]);
        let err = convert_bytes(&dir, &archive, "strided.pth").unwrap_err();
        assert!(format!("{err:?}").contains("contiguous"), "{err:?}");
    }

    #[test]
    fn tensor_overrunning_storage_is_rejected() {
        let dir = test_dir("overrun");
        let pickle = Pickle::new()
            .op(b'}')
            .string("x")
            .tensor("FloatStorage", "0", 8, 0, &[8], &[1])
            .op(b's')
            .stop();
        // Storage only holds 4 floats.
        let archive = stored_zip(&[
            ("m/data.pkl", pickle),
            ("m/data/0", f32_bytes(&[1.0, 2.0, 3.0, 4.0])),
        ]);
        let err = convert_bytes(&dir, &archive, "overrun.pth").unwrap_err();
        assert!(format!("{err:?}").contains("overruns"), "{err:?}");
    }

    #[test]
    fn unsupported_callable_is_rejected() {
        let dir = test_dir("callable");
        let pickle = Pickle::new()
            .global("os", "system")
            .op(b'(')
            .string("echo pwned")
            .op(b't')
            .op(b'R')
            .stop();
        let archive = stored_zip(&[("m/data.pkl", pickle)]);
        let err = convert_bytes(&dir, &archive, "evil.pth").unwrap_err();
        assert!(
            format!("{err:?}").contains("unsupported callable"),
            "{err:?}"
        );
    }

    #[test]
    fn compressed_zip_entries_are_rejected() {
        let dir = test_dir("deflate");
        let mut archive = stored_zip(&[("m/data.pkl", vec![b'N', b'.'])]);
        // Flip the method field in both the local (offset 8) and central
        // directory headers to 8 (deflate).
        archive[8] = 8;
        let central = archive
            .windows(4)
            .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
            .unwrap();
        archive[central + 10] = 8;
        let err = convert_bytes(&dir, &archive, "deflate.pth").unwrap_err();
        assert!(format!("{err:?}").contains("stored"), "{err:?}");
    }

    #[test]
    fn progress_reports_every_tensor() {
        let dir = test_dir("progress");
        let pickle = Pickle::new()
            .op(b'}')
            .string("x")
            .tensor("FloatStorage", "0", 2, 0, &[2], &[1])
            .op(b's')
            .string("y")
            .tensor("FloatStorage", "0", 2, 0, &[2], &[1])
            .op(b's')
            .stop();
        let archive = stored_zip(&[
            ("m/data.pkl", pickle),
            ("m/data/0", f32_bytes(&[1.0, 2.0])),
        ]);
        let src = dir.join("p.pth");
        std::fs::write(&src, &archive).unwrap();
        let dest = dir.join("p.mktts");
        let mut seen = Vec::new();
        convert_torch_weights(&src, &dest, &mut |done, total| seen.push((done, total)))
            .unwrap();
        assert_eq!(seen, vec![(0, 2), (1, 2), (2, 2)]);
        // No .tmp left behind.
        assert!(!dir.join("p.mktts.tmp").exists());
    }
}
