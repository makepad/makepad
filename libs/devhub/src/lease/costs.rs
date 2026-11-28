//! Restart checkpoints for the append-only cost and lease ledgers.
use super::CostClass;
use makepad_network::digest::sha256_hash;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::hash::Hash;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const COSTS_SNAP_FILE: &str = "costs.snap";
pub(crate) const NEXT_ID_SNAP_FILE: &str = "leases.id.snap";
pub(crate) const ANCHOR_WINDOW: u64 = 4096;
pub(crate) const CHECKPOINT_DELTA_BYTES: u64 = 4 << 20;

const COSTS_MAGIC: [u8; 8] = *b"DHC1\0\0\0\0";
const COSTS_VERSION: u32 = 1;
const COSTS_HEADER_LEN: usize = 120;
const NEXT_ID_MAGIC: [u8; 8] = *b"DHL1\0\0\0\0";
const NEXT_ID_VERSION: u32 = 1;
const NEXT_ID_SNAP_LEN: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileAnchor {
    pub offset: u64,
    pub head_hash: [u8; 32],
    pub tail_hash: [u8; 32],
}

impl FileAnchor {
    pub fn compute(file: &mut File, offset: u64) -> io::Result<Self> {
        if file.metadata()?.len() < offset {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ledger shorter than checkpoint"));
        }
        let mut buf = [0; ANCHOR_WINDOW as usize];
        let buf = &mut buf[..offset.min(ANCHOR_WINDOW) as usize];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(buf)?;
        let head_hash = sha256_hash(buf);
        let start = offset.saturating_sub(ANCHOR_WINDOW);
        let tail_hash = if start == 0 { head_hash } else {
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(buf)?;
            sha256_hash(buf)
        };
        Ok(Self { offset, head_hash, tail_hash })
    }

    pub fn matches(&self, file: &mut File) -> io::Result<bool> {
        Ok(file.metadata()?.len() >= self.offset && *self == Self::compute(file, self.offset)?)
    }
}

// The v1 wire order. Each row names the scheduler field, its key, value, and value reader.
macro_rules! cost_sections {
    ($visitor:ident) => {
        $visitor! {
            walls: (String, String) => VecDeque<u64>, walls(9);
            per_package: String => VecDeque<u64>, walls(9);
            runs: (String, String) => VecDeque<u64>, walls(9);
            node_walls: (String, String, String) => VecDeque<u64>, walls(9);
            lane_walls: (String, String) => VecDeque<u64>, walls(9);
            native_pkgs: (String, String) => String, string();
            builds: (String, bool) => VecDeque<u64>, walls(9);
            node_builds: (String, String, bool) => VecDeque<u64>, walls(9);
            peaks: (String, String) => VecDeque<f64>, measures(20, false);
            kind_runs: (String, &'static str) => VecDeque<u64>, walls(20);
            family_peaks: (String, String, String) => VecDeque<f64>, measures(20, false);
            lane_peaks: (String, String, String, String) => VecDeque<f64>, measures(20, false);
            family_state_peaks: (String, String, String, bool) => VecDeque<f64>, measures(20, false);
            lane_state_peaks: (String, String, String, String, bool) => VecDeque<f64>, measures(20, false);
            cpu: (String, String) => VecDeque<f64>, measures(9, false);
            cpu_peaks: (String, String) => VecDeque<f64>, measures(9, false);
            past_told: (String, String) => bool, boolean();
            disks: (String, String, bool) => VecDeque<f64>, measures(9, false);
            node_disks: (String, String, String, bool) => VecDeque<f64>, measures(9, false);
            kind_disks: (String, bool, bool) => VecDeque<f64>, measures(9, false);
            jit_cores: String => VecDeque<f64>, measures(30, true);
            tree_jits: (String, String) => String, string();
            tree_rebuilds: String => bool, boolean();
            calls: (String, String, bool) => VecDeque<f64>, measures(9, false);
        }
    };
}
pub(super) use cost_sections;

macro_rules! snapshot {
    ($($field:ident: $key:ty => $val:ty, $read:ident($($arg:expr),*);)*) => {
        #[derive(Debug, Default, PartialEq)]
        pub(super) struct CostSnap { $(pub $field: HashMap<$key, $val>,)* }

        pub(super) struct CostMapsRef<'a> { $(pub $field: &'a HashMap<$key, $val>,)* }

        pub(super) fn encode_body(maps: CostMapsRef<'_>) -> Vec<u8> {
            let mut out = Encoder::default();
            $(out.map(maps.$field);)*
            out.finish()
        }

        fn decode_body(body: &[u8]) -> Result<CostSnap, String> {
            let mut d = Decoder::new(body)?;
            let snap = CostSnap { $($field: d.map(|d| d.$read($($arg),*))?,)* };
            if !d.bytes.is_empty() { return Err("trailing bytes in cost snapshot".into()); }
            Ok(snap)
        }
    };
}
cost_sections!(snapshot);

#[derive(Default)]
struct Encoder<'a> {
    indices: HashMap<&'a str, u32>,
    table: Vec<u8>,
    sections: Vec<u8>,
}

impl<'a> Encoder<'a> {
    fn map<K: Encode, V: Encode>(&mut self, values: &'a HashMap<K, V>) {
        self.sections.extend_from_slice(&count(values.len()));
        for (key, value) in values {
            key.encode(self);
            value.encode(self);
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + self.table.len() + self.sections.len());
        body.extend_from_slice(&count(self.indices.len()));
        body.extend(self.table);
        body.extend(self.sections);
        body
    }
}

fn count(n: usize) -> [u8; 4] { u32::try_from(n).expect("cost checkpoint count exceeds u32").to_le_bytes() }

trait Encode {
    fn encode<'a>(&'a self, out: &mut Encoder<'a>);
}

impl Encode for String {
    fn encode<'a>(&'a self, out: &mut Encoder<'a>) {
        let next = out.indices.len();
        let index = out.indices.entry(self.as_str()).or_insert_with(|| {
            out.table.extend_from_slice(&count(self.len()));
            out.table.extend_from_slice(self.as_bytes());
            u32::try_from(next).expect("cost checkpoint index exceeds u32")
        });
        out.sections.extend_from_slice(&index.to_le_bytes());
    }
}

impl Encode for bool {
    fn encode(&self, out: &mut Encoder<'_>) { out.sections.push(u8::from(*self)); }
}

impl Encode for &'static str {
    fn encode(&self, out: &mut Encoder<'_>) {
        out.sections.push(match CostClass::parse(self).expect("scheduler cost class") {
            CostClass::Small => 0, CostClass::Medium => 1, CostClass::Large => 2,
        });
    }
}

macro_rules! encode_deque {
    ($t:ty) => {
        impl Encode for VecDeque<$t> {
            fn encode(&self, out: &mut Encoder<'_>) {
                out.sections.push(u8::try_from(self.len()).expect("scheduler history length"));
                for n in self { out.sections.extend_from_slice(&n.to_le_bytes()); }
            }
        }
    };
}
encode_deque!(u64);
encode_deque!(f64);

struct Decoder<'a> {
    bytes: &'a [u8],
    strings: Vec<&'a str>,
}

impl<'a> Decoder<'a> {
    fn new(body: &'a [u8]) -> Result<Self, String> {
        let mut d = Self { bytes: body, strings: Vec::new() };
        let count = d.count(4)?;
        for _ in 0..count {
            let len = d.u32()? as usize;
            let bytes = d.take(len)?;
            d.strings.push(std::str::from_utf8(bytes).map_err(|_| "invalid cost string")?);
        }
        Ok(d)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let (value, rest) = self.bytes.split_at_checked(n).ok_or("short cost snapshot")?;
        self.bytes = rest;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, String> { Ok(self.take(1)?[0]) }
    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }

    fn count(&mut self, min_bytes: usize) -> Result<usize, String> {
        let count = self.u32()? as usize;
        if count > self.bytes.len() / min_bytes { return Err("impossible cost snapshot count".into()); }
        Ok(count)
    }

    fn string(&mut self) -> Result<String, String> {
        let index = self.u32()? as usize;
        Ok(self.strings.get(index).ok_or("invalid cost string index")?.to_string())
    }

    fn boolean(&mut self) -> Result<bool, String> {
        match self.byte()? { 0 => Ok(false), 1 => Ok(true), _ => Err("invalid cost boolean".into()) }
    }

    fn samples(&mut self, cap: u8) -> Result<impl Iterator<Item = u64> + 'a, String> {
        let len = self.byte()?;
        if len > cap { return Err("cost history exceeds its window".into()); }
        Ok(self.take(usize::from(len) * 8)?.chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())))
    }

    fn walls(&mut self, cap: u8) -> Result<VecDeque<u64>, String> { Ok(self.samples(cap)?.collect()) }

    fn measures(&mut self, cap: u8, positive: bool) -> Result<VecDeque<f64>, String> {
        self.samples(cap)?.map(|bits| {
            let n = f64::from_bits(bits);
            if !n.is_finite() || n < 0.0 || (positive && n == 0.0) { Err("invalid cost measurement".into()) } else { Ok(n) }
        }).collect()
    }

    fn map<K: Key, V>(&mut self, read: impl Fn(&mut Self) -> Result<V, String>) -> Result<HashMap<K, V>, String> {
        // All values occupy at least one byte. Grow only as complete entries are decoded.
        let count = self.count(K::BYTES + 1)?;
        let mut map = HashMap::new();
        for _ in 0..count {
            let key = K::decode(self)?;
            if map.insert(key, read(self)?).is_some() { return Err("duplicate cost key".into()); }
        }
        Ok(map)
    }
}

trait Key: Eq + Hash + Sized {
    const BYTES: usize;
    fn decode(d: &mut Decoder<'_>) -> Result<Self, String>;
}

impl Key for String {
    const BYTES: usize = 4;
    fn decode(d: &mut Decoder<'_>) -> Result<Self, String> { d.string() }
}

impl Key for bool {
    const BYTES: usize = 1;
    fn decode(d: &mut Decoder<'_>) -> Result<Self, String> { d.boolean() }
}

impl Key for &'static str {
    const BYTES: usize = 1;
    fn decode(d: &mut Decoder<'_>) -> Result<Self, String> {
        Ok(match d.byte()? {
            0 => CostClass::Small, 1 => CostClass::Medium, 2 => CostClass::Large,
            _ => return Err("invalid cost class".into()),
        }.as_str())
    }
}

macro_rules! tuple_key {
    ($($t:ident $i:tt),*) => {
        impl<$($t: Key),*> Key for ($($t,)*) {
            const BYTES: usize = 0 $(+ $t::BYTES)*;
            fn decode(d: &mut Decoder<'_>) -> Result<Self, String> { Ok(($($t::decode(d)?,)*)) }
        }
        impl<$($t: Encode),*> Encode for ($($t,)*) {
            fn encode<'a>(&'a self, out: &mut Encoder<'a>) { $(self.$i.encode(out);)* }
        }
    };
}
tuple_key!(A 0, B 1);
tuple_key!(A 0, B 1, C 2);
tuple_key!(A 0, B 1, C 2, D 3);
tuple_key!(A 0, B 1, C 2, D 3, E 4);

fn header(bytes: &[u8], magic: [u8; 8], version: u32, hashes: usize, ledger: &mut File) -> Result<FileAnchor, String> {
    if bytes[..8] != magic || bytes[8..12] != version.to_le_bytes() || bytes[12..16] != [0; 4] {
        return Err("unrecognized checkpoint header".into());
    }
    let anchor = FileAnchor {
        offset: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        head_hash: bytes[hashes..hashes + 32].try_into().unwrap(),
        tail_hash: bytes[hashes + 32..hashes + 64].try_into().unwrap(),
    };
    if !anchor.matches(ledger).map_err(|e| e.to_string())? { return Err("checkpoint does not match ledger".into()); }
    Ok(anchor)
}

fn open_snapshot(path: &Path) -> Result<Option<File>, String> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn load_costs_snap(path: &Path, ledger: &mut File) -> Result<Option<(FileAnchor, CostSnap)>, String> {
    let Some(mut file) = open_snapshot(path)? else { return Ok(None) };
    let mut bytes = [0; COSTS_HEADER_LEN];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let anchor = header(&bytes, COSTS_MAGIC, COSTS_VERSION, 24, ledger)?;
    let mut body = Vec::new();
    file.read_to_end(&mut body).map_err(|e| e.to_string())?;
    if sha256_hash(&body) != bytes[88..120] { return Err("cost snapshot body hash mismatch".into()); }
    Ok(Some((anchor, decode_body(&body)?)))
}

fn write_snapshot(path: &Path, header: &[u8], body: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".new");
    let mut file = File::create(&tmp)?;
    file.write_all(header)?;
    file.write_all(body)?;
    file.sync_data()?;
    drop(file);
    fs::rename(tmp, path)
}

pub(super) fn write_costs_snap(path: &Path, anchor: FileAnchor, body: &[u8]) -> io::Result<()> {
    let mut h = [0; COSTS_HEADER_LEN];
    h[..8].copy_from_slice(&COSTS_MAGIC);
    h[8..12].copy_from_slice(&COSTS_VERSION.to_le_bytes());
    h[16..24].copy_from_slice(&anchor.offset.to_le_bytes());
    h[24..56].copy_from_slice(&anchor.head_hash);
    h[56..88].copy_from_slice(&anchor.tail_hash);
    h[88..].copy_from_slice(&sha256_hash(body));
    write_snapshot(path, &h, body)
}

pub(super) fn load_next_id_snap(path: &Path, ledger: &mut File) -> Result<Option<(FileAnchor, u64)>, String> {
    let Some(mut file) = open_snapshot(path)? else { return Ok(None) };
    if file.metadata().map_err(|e| e.to_string())?.len() != NEXT_ID_SNAP_LEN as u64 { return Err("wrong lease ID snapshot size".into()); }
    let mut h = [0; NEXT_ID_SNAP_LEN];
    file.read_exact(&mut h).map_err(|e| e.to_string())?;
    let anchor = header(&h, NEXT_ID_MAGIC, NEXT_ID_VERSION, 32, ledger)?;
    Ok(Some((anchor, u64::from_le_bytes(h[24..32].try_into().unwrap()))))
}

pub(super) fn write_next_id_snap(path: &Path, anchor: FileAnchor, max_id: u64) -> io::Result<()> {
    let mut h = [0; NEXT_ID_SNAP_LEN];
    h[..8].copy_from_slice(&NEXT_ID_MAGIC);
    h[8..12].copy_from_slice(&NEXT_ID_VERSION.to_le_bytes());
    h[16..24].copy_from_slice(&anchor.offset.to_le_bytes());
    h[24..32].copy_from_slice(&max_id.to_le_bytes());
    h[32..64].copy_from_slice(&anchor.head_hash);
    h[64..].copy_from_slice(&anchor.tail_hash);
    write_snapshot(path, &h, &[])
}
