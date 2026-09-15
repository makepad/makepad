//! OpenStreetMap PBF reader: the `fileformat.proto` framing and the
//! `osmformat.proto` blocks, written from the spec on the repo's own
//! protobuf varint helpers (`makepad_mbtile_reader`) and inflate
//! (`makepad_fast_inflate`), so a bake needs no crates.io parser.
//!
//! Surface: [`BlobReader`] walks a file blob by blob; [`Blob::decode`]
//! turns one into a [`HeaderBlock`] or a [`PrimitiveBlock`];
//! [`PrimitiveBlock::elements`] yields [`Element`]s (dense nodes, nodes,
//! ways, relations — in that order per primitive group); [`ElementReader`]
//! runs a serial [`for_each`](ElementReader::for_each) or a
//! [`par_map_reduce`](ElementReader::par_map_reduce) over a bounded pool
//! of decode workers. Coordinates follow the spec exactly:
//! `degrees = 1e-9 * (offset + granularity * value)`.

use makepad_fast_inflate::{zlib_decompress, DecompressError};
use makepad_mbtile_reader::{read_pb_len_slice, read_pb_varint, skip_pb_field};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};

/// Spec limit on one BlobHeader message (64 KiB).
pub const MAX_BLOB_HEADER_SIZE: u64 = 64 * 1024;
/// Spec limit on one blob's contents, compressed or inflated (32 MiB).
pub const MAX_BLOB_MESSAGE_SIZE: u64 = 32 * 1024 * 1024;

// --- protobuf wire helpers ---------------------------------------------

fn read_key(bytes: &[u8], pos: &mut usize) -> Result<(u32, u8), String> {
    let key = read_pb_varint(bytes, pos)?;
    let field = u32::try_from(key >> 3)
        .map_err(|_| "protobuf field number out of range".to_string())?;
    Ok((field, (key & 7) as u8))
}

fn zigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn as_u32(value: u64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "uint32 field out of range".to_string())
}

/// `int32`: protobuf sign-extends negatives to ten-byte varints, so the
/// value is read back through `i64` and then range-checked.
fn as_i32(value: u64) -> Result<i32, String> {
    i32::try_from(value as i64).map_err(|_| "int32 field out of range".to_string())
}

fn as_sint64(value: u64) -> Result<i64, String> {
    Ok(zigzag(value))
}

/// Read a `repeated` varint field in either encoding: packed (wire type
/// 2, one length-delimited run of varints) or one value per key (wire
/// type 0). `decode` maps each raw varint to the field's scalar type.
fn read_repeated<T>(
    bytes: &[u8],
    pos: &mut usize,
    wire: u8,
    out: &mut Vec<T>,
    decode: impl Fn(u64) -> Result<T, String>,
) -> Result<(), String> {
    match wire {
        2 => {
            let packed = read_pb_len_slice(bytes, pos)?;
            let mut at = 0;
            while at < packed.len() {
                out.push(decode(read_pb_varint(packed, &mut at)?)?);
            }
            Ok(())
        }
        0 => {
            out.push(decode(read_pb_varint(bytes, pos)?)?);
            Ok(())
        }
        _ => Err(format!("unexpected wire type {wire} for a repeated varint field")),
    }
}

fn read_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    std::str::from_utf8(read_pb_len_slice(bytes, pos)?)
        .map(str::to_owned)
        .map_err(|_| "string field is not valid UTF-8".to_string())
}

// --- fileformat.proto ----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Empty,
    Raw,
    Zlib,
    Lzma,
    Bzip2,
    Lz4,
    Zstd,
}

/// The kind of a blob, from its header's `type` string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobType<'a> {
    OsmHeader,
    OsmData,
    Unknown(&'a str),
}

/// One decoded blob.
#[derive(Clone, Debug)]
pub enum BlobDecode<'a> {
    OsmHeader(Box<HeaderBlock>),
    OsmData(PrimitiveBlock),
    Unknown(&'a str),
}

/// One file block: its header's type plus the still-compressed contents.
#[derive(Clone, Debug)]
pub struct Blob {
    blob_type: String,
    raw_size: Option<i32>,
    encoding: Encoding,
    message: Vec<u8>,
    payload: (usize, usize),
}

impl Blob {
    fn parse(blob_type: String, message: Vec<u8>) -> Result<Blob, String> {
        let mut blob = Blob {
            blob_type,
            raw_size: None,
            encoding: Encoding::Empty,
            message: Vec::new(),
            payload: (0, 0),
        };
        let mut pos = 0;
        while pos < message.len() {
            let (field, wire) = read_key(&message, &mut pos)?;
            let encoding = match (field, wire) {
                (2, 0) => {
                    blob.raw_size = Some(as_i32(read_pb_varint(&message, &mut pos)?)?);
                    continue;
                }
                (1, 2) => Encoding::Raw,
                (3, 2) => Encoding::Zlib,
                (4, 2) => Encoding::Lzma,
                (5, 2) => Encoding::Bzip2,
                (6, 2) => Encoding::Lz4,
                (7, 2) => Encoding::Zstd,
                (_, wire) => {
                    skip_pb_field(&message, &mut pos, wire)?;
                    continue;
                }
            };
            let len = read_pb_len_slice(&message, &mut pos)?.len();
            blob.encoding = encoding;
            blob.payload = (pos - len, pos);
        }
        blob.message = message;
        Ok(blob)
    }

    pub fn get_type(&self) -> BlobType<'_> {
        match self.blob_type.as_str() {
            "OSMHeader" => BlobType::OsmHeader,
            "OSMData" => BlobType::OsmData,
            other => BlobType::Unknown(other),
        }
    }

    pub fn decode(&self) -> Result<BlobDecode<'_>, String> {
        match self.get_type() {
            BlobType::OsmHeader => Ok(BlobDecode::OsmHeader(Box::new(self.to_headerblock()?))),
            BlobType::OsmData => Ok(BlobDecode::OsmData(self.to_primitiveblock()?)),
            BlobType::Unknown(other) => Ok(BlobDecode::Unknown(other)),
        }
    }

    pub fn to_headerblock(&self) -> Result<HeaderBlock, String> {
        HeaderBlock::parse(&self.contents()?)
    }

    pub fn to_primitiveblock(&self) -> Result<PrimitiveBlock, String> {
        PrimitiveBlock::parse(&self.contents()?)
    }

    /// The inflated block bytes. A compressed blob must declare its
    /// `raw_size`, and that declared size — checked against the spec limit
    /// before anything is allocated — bounds the inflate: the stream has to
    /// fill exactly that many bytes, so neither a zlib bomb nor a header
    /// claiming gigabytes gets past the 32 MiB line.
    fn contents(&self) -> Result<Vec<u8>, String> {
        let payload = &self.message[self.payload.0..self.payload.1];
        let contents = match self.encoding {
            Encoding::Raw => {
                if payload.len() as u64 >= MAX_BLOB_MESSAGE_SIZE {
                    return Err(format!("blob contents of {} bytes exceed the 32 MiB limit", payload.len()));
                }
                payload.to_vec()
            }
            Encoding::Zlib => {
                let raw_size = self
                    .raw_size
                    .ok_or_else(|| "blob zlib data without a raw_size".to_string())?;
                if raw_size < 0 {
                    return Err(format!("blob raw_size {raw_size} is negative"));
                }
                if u64::from(raw_size.unsigned_abs()) >= MAX_BLOB_MESSAGE_SIZE {
                    return Err(format!("blob raw_size {raw_size} exceeds the 32 MiB limit"));
                }
                let raw_size = raw_size as usize;
                let mut output = vec![0u8; raw_size];
                let written = match zlib_decompress(payload, &mut output) {
                    Ok((_consumed, written)) => written,
                    Err(DecompressError::InsufficientSpace) => {
                        return Err(format!("blob zlib data inflates past its raw_size of {raw_size} bytes"))
                    }
                    Err(err) => return Err(format!("blob zlib data: {err}")),
                };
                if written != raw_size {
                    return Err(format!(
                        "blob raw_size {raw_size} does not match its {written} inflated bytes"
                    ));
                }
                output
            }
            Encoding::Lzma => return Err("blob uses lzma compression, which is not supported".into()),
            Encoding::Bzip2 => return Err("blob uses bzip2 compression, which is not supported".into()),
            Encoding::Lz4 => return Err("blob uses lz4 compression, which is not supported".into()),
            Encoding::Zstd => return Err("blob uses zstd compression, which is not supported".into()),
            Encoding::Empty => return Err("blob has no data".into()),
        };
        if let Some(raw_size) = self.raw_size {
            if usize::try_from(raw_size).ok() != Some(contents.len()) {
                return Err(format!(
                    "blob raw_size {raw_size} does not match its {} content bytes",
                    contents.len()
                ));
            }
        }
        Ok(contents)
    }
}

/// Walks a PBF file blob by blob: a 4-byte big-endian header length, the
/// BlobHeader, then `datasize` bytes of Blob.
pub struct BlobReader<R: Read> {
    reader: R,
    done: bool,
}

impl BlobReader<BufReader<File>> {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        File::open(path)
            .map(|file| Self::new(BufReader::new(file)))
            .map_err(|err| err.to_string())
    }
}

impl<R: Read> BlobReader<R> {
    pub fn new(reader: R) -> Self {
        BlobReader { reader, done: false }
    }

    /// Fill `buf` unless the stream ends first; returns the bytes read.
    fn read_fully(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.reader.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err.to_string()),
            }
        }
        Ok(filled)
    }

    fn read_exact_vec(&mut self, len: usize, what: &str) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; len];
        if self.read_fully(&mut bytes)? != len {
            return Err(format!("file ends inside a {what}"));
        }
        Ok(bytes)
    }

    fn read_blob(&mut self) -> Result<Option<Blob>, String> {
        let mut len = [0u8; 4];
        match self.read_fully(&mut len)? {
            0 => return Ok(None),
            4 => {}
            _ => return Err("file ends inside a blob header length".into()),
        }
        let header_size = u64::from(u32::from_be_bytes(len));
        if header_size >= MAX_BLOB_HEADER_SIZE {
            return Err(format!("blob header of {header_size} bytes exceeds the 64 KiB limit"));
        }
        let header = self.read_exact_vec(header_size as usize, "blob header")?;
        let mut blob_type = None;
        let mut datasize = None;
        let mut pos = 0;
        while pos < header.len() {
            let (field, wire) = read_key(&header, &mut pos)?;
            match (field, wire) {
                (1, 2) => blob_type = Some(read_string(&header, &mut pos)?),
                (3, 0) => datasize = Some(as_i32(read_pb_varint(&header, &mut pos)?)?),
                (_, wire) => skip_pb_field(&header, &mut pos, wire)?,
            }
        }
        let blob_type = blob_type.ok_or_else(|| "blob header without a type".to_string())?;
        let datasize = datasize.ok_or_else(|| "blob header without a datasize".to_string())?;
        if datasize < 0 || u64::from(datasize.unsigned_abs()) >= MAX_BLOB_MESSAGE_SIZE {
            return Err(format!("blob of {datasize} bytes exceeds the 32 MiB limit"));
        }
        let message = self.read_exact_vec(datasize as usize, "blob")?;
        Blob::parse(blob_type, message).map(Some)
    }
}

impl<R: Read> Iterator for BlobReader<R> {
    type Item = Result<Blob, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.read_blob() {
            Ok(Some(blob)) => Some(Ok(blob)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(err) => {
                self.done = true;
                Some(Err(err))
            }
        }
    }
}

// --- osmformat.proto: HeaderBlock ------------------------------------------

/// The header's bounding box, in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderBBox {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl HeaderBBox {
    fn parse(bytes: &[u8]) -> Result<HeaderBBox, String> {
        let mut nano = [0i64; 4];
        let mut pos = 0;
        while pos < bytes.len() {
            let (field, wire) = read_key(bytes, &mut pos)?;
            match (field, wire) {
                (1..=4, 0) => nano[field as usize - 1] = zigzag(read_pb_varint(bytes, &mut pos)?),
                (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
            }
        }
        Ok(HeaderBBox {
            left: (nano[0] as f64) * 1.0e-9,
            right: (nano[1] as f64) * 1.0e-9,
            top: (nano[2] as f64) * 1.0e-9,
            bottom: (nano[3] as f64) * 1.0e-9,
        })
    }
}

/// The file's OSMHeader block.
#[derive(Clone, Debug, Default)]
pub struct HeaderBlock {
    bbox: Option<HeaderBBox>,
    required_features: Vec<String>,
    optional_features: Vec<String>,
    writing_program: Option<String>,
}

impl HeaderBlock {
    fn parse(bytes: &[u8]) -> Result<HeaderBlock, String> {
        let mut header = HeaderBlock::default();
        let mut pos = 0;
        while pos < bytes.len() {
            let (field, wire) = read_key(bytes, &mut pos)?;
            match (field, wire) {
                (1, 2) => header.bbox = Some(HeaderBBox::parse(read_pb_len_slice(bytes, &mut pos)?)?),
                (4, 2) => header.required_features.push(read_string(bytes, &mut pos)?),
                (5, 2) => header.optional_features.push(read_string(bytes, &mut pos)?),
                (16, 2) => header.writing_program = Some(read_string(bytes, &mut pos)?),
                // source (17) and the osmosis replication fields (32–34)
                // have no reader in the repo; they skip like any unknown.
                (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
            }
        }
        Ok(header)
    }

    pub fn bbox(&self) -> Option<HeaderBBox> {
        self.bbox.clone()
    }

    pub fn required_features(&self) -> &[String] {
        &self.required_features
    }

    pub fn optional_features(&self) -> &[String] {
        &self.optional_features
    }

    pub fn writing_program(&self) -> Option<&str> {
        self.writing_program.as_deref()
    }
}

// --- osmformat.proto: PrimitiveBlock ---------------------------------------

/// Marks a string-table entry that is not valid UTF-8; such an entry ends
/// tag iteration, as it did with the crate this reader replaces.
const INVALID_STRING: u32 = u32::MAX;

#[derive(Clone, Debug, Default)]
struct RawNode {
    id: i64,
    lat: i64,
    lon: i64,
    keys: Vec<u32>,
    vals: Vec<u32>,
}

/// Column-wise nodes; ids and coordinates are delta coded.
#[derive(Clone, Debug, Default)]
struct DenseNodes {
    ids: Vec<i64>,
    lats: Vec<i64>,
    lons: Vec<i64>,
    keys_vals: Vec<i32>,
}

#[derive(Clone, Debug, Default)]
struct RawWay {
    id: i64,
    keys: Vec<u32>,
    vals: Vec<u32>,
    refs: Vec<i64>,
}

#[derive(Clone, Debug, Default)]
struct RawRelation {
    id: i64,
    keys: Vec<u32>,
    vals: Vec<u32>,
    roles_sid: Vec<i32>,
    memids: Vec<i64>,
    types: Vec<RelMemberType>,
}

#[derive(Clone, Debug, Default)]
struct PrimitiveGroup {
    nodes: Vec<RawNode>,
    dense: DenseNodes,
    ways: Vec<RawWay>,
    relations: Vec<RawRelation>,
}

/// One decoded data block: its string table and primitive groups.
#[derive(Clone, Debug)]
pub struct PrimitiveBlock {
    /// Every valid string-table entry, concatenated.
    text: String,
    /// (start, len) into `text`, or len == INVALID_STRING.
    strings: Vec<(u32, u32)>,
    groups: Vec<PrimitiveGroup>,
    granularity: i32,
    lat_offset: i64,
    lon_offset: i64,
}

impl PrimitiveBlock {
    fn parse(bytes: &[u8]) -> Result<PrimitiveBlock, String> {
        let mut block = PrimitiveBlock {
            text: String::new(),
            strings: Vec::new(),
            groups: Vec::new(),
            granularity: 100,
            lat_offset: 0,
            lon_offset: 0,
        };
        let mut pos = 0;
        while pos < bytes.len() {
            let (field, wire) = read_key(bytes, &mut pos)?;
            match (field, wire) {
                (1, 2) => block.parse_stringtable(read_pb_len_slice(bytes, &mut pos)?)?,
                (2, 2) => block.groups.push(parse_group(read_pb_len_slice(bytes, &mut pos)?)?),
                (17, 0) => block.granularity = as_i32(read_pb_varint(bytes, &mut pos)?)?,
                (19, 0) => block.lat_offset = read_pb_varint(bytes, &mut pos)? as i64,
                (20, 0) => block.lon_offset = read_pb_varint(bytes, &mut pos)? as i64,
                // date_granularity (18) only scales Info timestamps, which
                // this reader does not decode.
                (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
            }
        }
        Ok(block)
    }

    fn parse_stringtable(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut pos = 0;
        while pos < bytes.len() {
            let (field, wire) = read_key(bytes, &mut pos)?;
            match (field, wire) {
                (1, 2) => {
                    let entry = read_pb_len_slice(bytes, &mut pos)?;
                    match std::str::from_utf8(entry) {
                        Ok(text) => {
                            let start = u32::try_from(self.text.len())
                                .map_err(|_| "string table too large".to_string())?;
                            self.text.push_str(text);
                            self.strings.push((start, text.len() as u32));
                        }
                        Err(_) => self.strings.push((0, INVALID_STRING)),
                    }
                }
                (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
            }
        }
        Ok(())
    }

    /// Every element of the block: per group, dense nodes, then nodes,
    /// ways and relations.
    pub fn elements(&self) -> BlockElementsIter<'_> {
        BlockElementsIter {
            block: self,
            groups: self.groups.iter(),
            dense: DenseNodeIter::new(self, &EMPTY_DENSE),
            nodes: [].iter(),
            ways: [].iter(),
            relations: [].iter(),
        }
    }

    fn string(&self, index: usize) -> Result<&str, String> {
        let &(start, len) = self
            .strings
            .get(index)
            .ok_or_else(|| format!("stringtable index {index} out of bounds"))?;
        if len == INVALID_STRING {
            return Err(format!("stringtable entry {index} is not valid UTF-8"));
        }
        Ok(&self.text[start as usize..start as usize + len as usize])
    }

    fn key_value(&self, key: Option<usize>, value: Option<usize>) -> Option<(&str, &str)> {
        match (key, value) {
            (Some(key), Some(value)) => match (self.string(key), self.string(value)) {
                (Ok(key), Ok(value)) => Some((key, value)),
                _ => None,
            },
            _ => None,
        }
    }

    fn nano_lat(&self, lat: i64) -> i64 {
        self.lat_offset.wrapping_add(i64::from(self.granularity).wrapping_mul(lat))
    }

    fn nano_lon(&self, lon: i64) -> i64 {
        self.lon_offset.wrapping_add(i64::from(self.granularity).wrapping_mul(lon))
    }
}

static EMPTY_DENSE: DenseNodes = DenseNodes {
    ids: Vec::new(),
    lats: Vec::new(),
    lons: Vec::new(),
    keys_vals: Vec::new(),
};

fn parse_group(bytes: &[u8]) -> Result<PrimitiveGroup, String> {
    let mut group = PrimitiveGroup::default();
    let mut pos = 0;
    while pos < bytes.len() {
        let (field, wire) = read_key(bytes, &mut pos)?;
        match (field, wire) {
            (1, 2) => group.nodes.push(parse_node(read_pb_len_slice(bytes, &mut pos)?)?),
            (2, 2) => parse_dense(read_pb_len_slice(bytes, &mut pos)?, &mut group.dense)?,
            (3, 2) => group.ways.push(parse_way(read_pb_len_slice(bytes, &mut pos)?)?),
            (4, 2) => group.relations.push(parse_relation(read_pb_len_slice(bytes, &mut pos)?)?),
            // Changesets (5) and anything newer are skipped.
            (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(group)
}

fn parse_node(bytes: &[u8]) -> Result<RawNode, String> {
    let mut node = RawNode::default();
    let mut pos = 0;
    while pos < bytes.len() {
        let (field, wire) = read_key(bytes, &mut pos)?;
        match (field, wire) {
            (1, 0) => node.id = zigzag(read_pb_varint(bytes, &mut pos)?),
            (2, wire) => read_repeated(bytes, &mut pos, wire, &mut node.keys, as_u32)?,
            (3, wire) => read_repeated(bytes, &mut pos, wire, &mut node.vals, as_u32)?,
            (8, 0) => node.lat = zigzag(read_pb_varint(bytes, &mut pos)?),
            (9, 0) => node.lon = zigzag(read_pb_varint(bytes, &mut pos)?),
            // Info (4) is metadata the bakes never read.
            (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(node)
}

fn parse_dense(bytes: &[u8], dense: &mut DenseNodes) -> Result<(), String> {
    let mut pos = 0;
    while pos < bytes.len() {
        let (field, wire) = read_key(bytes, &mut pos)?;
        match (field, wire) {
            (1, wire) => read_repeated(bytes, &mut pos, wire, &mut dense.ids, as_sint64)?,
            (8, wire) => read_repeated(bytes, &mut pos, wire, &mut dense.lats, as_sint64)?,
            (9, wire) => read_repeated(bytes, &mut pos, wire, &mut dense.lons, as_sint64)?,
            (10, wire) => read_repeated(bytes, &mut pos, wire, &mut dense.keys_vals, as_i32)?,
            // DenseInfo (5) is metadata the bakes never read.
            (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(())
}

fn parse_way(bytes: &[u8]) -> Result<RawWay, String> {
    let mut way = RawWay::default();
    let mut pos = 0;
    while pos < bytes.len() {
        let (field, wire) = read_key(bytes, &mut pos)?;
        match (field, wire) {
            (1, 0) => way.id = read_pb_varint(bytes, &mut pos)? as i64,
            (2, wire) => read_repeated(bytes, &mut pos, wire, &mut way.keys, as_u32)?,
            (3, wire) => read_repeated(bytes, &mut pos, wire, &mut way.vals, as_u32)?,
            (8, wire) => read_repeated(bytes, &mut pos, wire, &mut way.refs, as_sint64)?,
            // Info (4) and the LocationsOnWays columns (9, 10) are skipped.
            (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(way)
}

fn parse_relation(bytes: &[u8]) -> Result<RawRelation, String> {
    let mut relation = RawRelation::default();
    let mut pos = 0;
    while pos < bytes.len() {
        let (field, wire) = read_key(bytes, &mut pos)?;
        match (field, wire) {
            (1, 0) => relation.id = read_pb_varint(bytes, &mut pos)? as i64,
            (2, wire) => read_repeated(bytes, &mut pos, wire, &mut relation.keys, as_u32)?,
            (3, wire) => read_repeated(bytes, &mut pos, wire, &mut relation.vals, as_u32)?,
            (8, wire) => read_repeated(bytes, &mut pos, wire, &mut relation.roles_sid, as_i32)?,
            (9, wire) => read_repeated(bytes, &mut pos, wire, &mut relation.memids, as_sint64)?,
            (10, wire) => read_repeated(bytes, &mut pos, wire, &mut relation.types, |value| {
                match value {
                    0 => Ok(RelMemberType::Node),
                    1 => Ok(RelMemberType::Way),
                    2 => Ok(RelMemberType::Relation),
                    other => Err(format!("unknown relation member type {other}")),
                }
            })?,
            (_, wire) => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(relation)
}

// --- elements ------------------------------------------------------------

/// One OSM element borrowed from its block.
#[derive(Clone, Debug)]
pub enum Element<'a> {
    Node(Node<'a>),
    /// A node from a `DenseNodes` column group; same data as `Node`, kept
    /// apart so the column representation is never copied out.
    DenseNode(DenseNode<'a>),
    Way(Way<'a>),
    Relation(Relation<'a>),
}

#[derive(Clone, Debug)]
pub struct Node<'a> {
    block: &'a PrimitiveBlock,
    node: &'a RawNode,
}

impl<'a> Node<'a> {
    pub fn id(&self) -> i64 {
        self.node.id
    }

    pub fn tags(&self) -> TagIter<'a> {
        TagIter { block: self.block, keys: self.node.keys.iter(), vals: self.node.vals.iter() }
    }

    pub fn lat(&self) -> f64 {
        1e-9 * self.block.nano_lat(self.node.lat) as f64
    }

    pub fn lon(&self) -> f64 {
        1e-9 * self.block.nano_lon(self.node.lon) as f64
    }

    /// Latitude in 1e-7 degree units, the store's fixed-point coordinate.
    pub fn decimicro_lat(&self) -> i32 {
        (self.block.nano_lat(self.node.lat) / 100) as i32
    }

    pub fn decimicro_lon(&self) -> i32 {
        (self.block.nano_lon(self.node.lon) / 100) as i32
    }
}

#[derive(Clone, Debug)]
pub struct DenseNode<'a> {
    block: &'a PrimitiveBlock,
    id: i64,
    lat: i64,
    lon: i64,
    keys_vals: &'a [i32],
}

impl<'a> DenseNode<'a> {
    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn tags(&self) -> DenseTagIter<'a> {
        DenseTagIter { block: self.block, keys_vals: self.keys_vals.iter() }
    }

    pub fn lat(&self) -> f64 {
        1e-9 * self.block.nano_lat(self.lat) as f64
    }

    pub fn lon(&self) -> f64 {
        1e-9 * self.block.nano_lon(self.lon) as f64
    }

    pub fn decimicro_lat(&self) -> i32 {
        (self.block.nano_lat(self.lat) / 100) as i32
    }

    pub fn decimicro_lon(&self) -> i32 {
        (self.block.nano_lon(self.lon) / 100) as i32
    }
}

#[derive(Clone, Debug)]
pub struct Way<'a> {
    block: &'a PrimitiveBlock,
    way: &'a RawWay,
}

impl<'a> Way<'a> {
    pub fn id(&self) -> i64 {
        self.way.id
    }

    pub fn tags(&self) -> TagIter<'a> {
        TagIter { block: self.block, keys: self.way.keys.iter(), vals: self.way.vals.iter() }
    }

    /// The node ids, in way order.
    pub fn refs(&self) -> WayRefIter<'a> {
        WayRefIter { deltas: self.way.refs.iter(), current: 0 }
    }
}

#[derive(Clone, Debug)]
pub struct Relation<'a> {
    block: &'a PrimitiveBlock,
    relation: &'a RawRelation,
}

impl<'a> Relation<'a> {
    pub fn id(&self) -> i64 {
        self.relation.id
    }

    pub fn tags(&self) -> TagIter<'a> {
        TagIter {
            block: self.block,
            keys: self.relation.keys.iter(),
            vals: self.relation.vals.iter(),
        }
    }

    pub fn members(&self) -> RelMemberIter<'a> {
        RelMemberIter {
            block: self.block,
            roles: self.relation.roles_sid.iter(),
            deltas: self.relation.memids.iter(),
            types: self.relation.types.iter(),
            current: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelMemberType {
    Node,
    Way,
    Relation,
}

#[derive(Clone, Debug)]
pub struct RelMember<'a> {
    block: &'a PrimitiveBlock,
    role_sid: i32,
    pub member_id: i64,
    pub member_type: RelMemberType,
}

impl<'a> RelMember<'a> {
    pub fn role(&self) -> Result<&'a str, String> {
        let index = usize::try_from(self.role_sid)
            .map_err(|_| format!("stringtable index {} out of bounds", self.role_sid))?;
        self.block.string(index)
    }
}

/// `(key, value)` tags of a node, way or relation. Ends early at a
/// string-table index that is out of range or not UTF-8.
#[derive(Clone, Debug)]
pub struct TagIter<'a> {
    block: &'a PrimitiveBlock,
    keys: std::slice::Iter<'a, u32>,
    vals: std::slice::Iter<'a, u32>,
}

impl<'a> Iterator for TagIter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        self.block.key_value(
            self.keys.next().map(|key| *key as usize),
            self.vals.next().map(|value| *value as usize),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.keys.len().min(self.vals.len())))
    }
}

/// `(key, value)` tags of a dense node.
#[derive(Clone, Debug)]
pub struct DenseTagIter<'a> {
    block: &'a PrimitiveBlock,
    keys_vals: std::slice::Iter<'a, i32>,
}

impl<'a> Iterator for DenseTagIter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let key = *self.keys_vals.next()?;
        let value = *self.keys_vals.next()?;
        self.block.key_value(usize::try_from(key).ok(), usize::try_from(value).ok())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.keys_vals.len() / 2))
    }
}

/// The node ids of a way, delta decoded.
#[derive(Clone, Debug)]
pub struct WayRefIter<'a> {
    deltas: std::slice::Iter<'a, i64>,
    current: i64,
}

impl Iterator for WayRefIter<'_> {
    type Item = i64;

    fn next(&mut self) -> Option<Self::Item> {
        let delta = *self.deltas.next()?;
        self.current = self.current.wrapping_add(delta);
        Some(self.current)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.deltas.size_hint()
    }
}

impl ExactSizeIterator for WayRefIter<'_> {}

/// The members of a relation, member ids delta decoded.
#[derive(Clone, Debug)]
pub struct RelMemberIter<'a> {
    block: &'a PrimitiveBlock,
    roles: std::slice::Iter<'a, i32>,
    deltas: std::slice::Iter<'a, i64>,
    types: std::slice::Iter<'a, RelMemberType>,
    current: i64,
}

impl<'a> Iterator for RelMemberIter<'a> {
    type Item = RelMember<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (Some(&role_sid), Some(&delta), Some(&member_type)) =
            (self.roles.next(), self.deltas.next(), self.types.next())
        else {
            return None;
        };
        self.current = self.current.wrapping_add(delta);
        Some(RelMember { block: self.block, role_sid, member_id: self.current, member_type })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.roles.len().min(self.deltas.len()).min(self.types.len());
        (len, Some(len))
    }
}

impl ExactSizeIterator for RelMemberIter<'_> {}

/// Dense nodes of one group, delta decoded, each with its `keys_vals` run.
#[derive(Clone, Debug)]
struct DenseNodeIter<'a> {
    block: &'a PrimitiveBlock,
    ids: std::slice::Iter<'a, i64>,
    lats: std::slice::Iter<'a, i64>,
    lons: std::slice::Iter<'a, i64>,
    current_id: i64,
    current_lat: i64,
    current_lon: i64,
    keys_vals: &'a [i32],
    keys_vals_index: usize,
}

impl<'a> DenseNodeIter<'a> {
    fn new(block: &'a PrimitiveBlock, dense: &'a DenseNodes) -> DenseNodeIter<'a> {
        DenseNodeIter {
            block,
            ids: dense.ids.iter(),
            lats: dense.lats.iter(),
            lons: dense.lons.iter(),
            current_id: 0,
            current_lat: 0,
            current_lon: 0,
            keys_vals: &dense.keys_vals,
            keys_vals_index: 0,
        }
    }
}

impl<'a> Iterator for DenseNodeIter<'a> {
    type Item = DenseNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (Some(&delta_id), Some(&delta_lat), Some(&delta_lon)) =
            (self.ids.next(), self.lats.next(), self.lons.next())
        else {
            return None;
        };
        self.current_id = self.current_id.wrapping_add(delta_id);
        self.current_lat = self.current_lat.wrapping_add(delta_lat);
        self.current_lon = self.current_lon.wrapping_add(delta_lon);
        // This node's (key, value) run ends at a 0 key; a block with no
        // tagged dense nodes at all may omit the column entirely.
        let keys_vals = self.keys_vals;
        let start = self.keys_vals_index;
        let mut end = start;
        for chunk in keys_vals[start..].chunks(2) {
            if chunk[0] != 0 && chunk.len() == 2 {
                end += 2;
                self.keys_vals_index += 2;
            } else {
                self.keys_vals_index += 1;
                break;
            }
        }
        Some(DenseNode {
            block: self.block,
            id: self.current_id,
            lat: self.current_lat,
            lon: self.current_lon,
            keys_vals: &keys_vals[start..end],
        })
    }
}

/// All elements of a block, group by group.
#[derive(Clone, Debug)]
pub struct BlockElementsIter<'a> {
    block: &'a PrimitiveBlock,
    groups: std::slice::Iter<'a, PrimitiveGroup>,
    dense: DenseNodeIter<'a>,
    nodes: std::slice::Iter<'a, RawNode>,
    ways: std::slice::Iter<'a, RawWay>,
    relations: std::slice::Iter<'a, RawRelation>,
}

impl<'a> Iterator for BlockElementsIter<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(node) = self.dense.next() {
                return Some(Element::DenseNode(node));
            }
            if let Some(node) = self.nodes.next() {
                return Some(Element::Node(Node { block: self.block, node }));
            }
            if let Some(way) = self.ways.next() {
                return Some(Element::Way(Way { block: self.block, way }));
            }
            if let Some(relation) = self.relations.next() {
                return Some(Element::Relation(Relation { block: self.block, relation }));
            }
            let group = self.groups.next()?;
            self.dense = DenseNodeIter::new(self.block, &group.dense);
            self.nodes = group.nodes.iter();
            self.ways = group.ways.iter();
            self.relations = group.relations.iter();
        }
    }
}

// --- element reader ------------------------------------------------------

/// Decode workers for a parallel pass: the machine minus two cores for the
/// reader and whoever else is running, never fewer than two.
pub fn decode_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(2))
        .unwrap_or(4)
}

/// Element-level access to a whole file.
pub struct ElementReader<R: Read> {
    blobs: BlobReader<R>,
}

impl ElementReader<BufReader<File>> {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        Ok(ElementReader { blobs: BlobReader::from_path(path)? })
    }
}

impl<R: Read> ElementReader<R> {
    pub fn new(reader: R) -> Self {
        ElementReader { blobs: BlobReader::new(reader) }
    }

    /// Visit every element in file order on the calling thread.
    pub fn for_each<F>(self, mut visit: F) -> Result<(), String>
    where
        F: for<'a> FnMut(Element<'a>),
    {
        for blob in self.blobs {
            if let BlobDecode::OsmData(block) = blob?.decode()? {
                for element in block.elements() {
                    visit(element);
                }
            }
        }
        Ok(())
    }

    /// Map every element and fold the results: blobs stream from the
    /// calling thread into a bounded queue, a fixed pool of workers
    /// inflates, decodes and folds each block from `identity()`, and the
    /// per-worker results are folded in worker order at the end. `reduce`
    /// runs on worker threads, so it must be `Sync`; the element order it
    /// sees is not the file order.
    pub fn par_map_reduce<MP, RD, ID, T>(self, map: MP, identity: ID, reduce: RD) -> Result<T, String>
    where
        MP: for<'a> Fn(Element<'a>) -> T + Sync + Send,
        RD: Fn(T, T) -> T + Sync + Send,
        ID: Fn() -> T + Sync + Send,
        T: Send,
    {
        let workers = decode_workers();
        let (blob_tx, blob_rx) = sync_channel::<Blob>(workers * 4);
        let blob_rx = Arc::new(Mutex::new(blob_rx));
        let failed = AtomicBool::new(false);
        let mut blobs = self.blobs;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let blob_rx = Arc::clone(&blob_rx);
                    let (map, identity, reduce, failed) = (&map, &identity, &reduce, &failed);
                    scope.spawn(move || -> Result<T, String> {
                        let mut acc = identity();
                        loop {
                            let received = blob_rx
                                .lock()
                                .map_err(|_| "pbf decode queue poisoned".to_string())?
                                .recv();
                            let Ok(blob) = received else {
                                break;
                            };
                            let block = match blob.decode() {
                                Ok(BlobDecode::OsmData(block)) => block,
                                Ok(_) => continue,
                                Err(err) => {
                                    failed.store(true, Ordering::Relaxed);
                                    return Err(err);
                                }
                            };
                            let mut block_acc = identity();
                            for element in block.elements() {
                                block_acc = reduce(block_acc, map(element));
                            }
                            acc = reduce(acc, block_acc);
                        }
                        Ok(acc)
                    })
                })
                .collect();
            // Only the workers hold the queue now: once they all exit, a
            // blocked send fails instead of waiting forever.
            drop(blob_rx);
            let mut first_error = None;
            for blob in blobs.by_ref() {
                if failed.load(Ordering::Relaxed) {
                    break;
                }
                match blob {
                    Ok(blob) => {
                        if blob_tx.send(blob).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        first_error = Some(err);
                        break;
                    }
                }
            }
            drop(blob_tx);
            let mut result = identity();
            for handle in handles {
                match handle.join() {
                    Ok(Ok(acc)) => result = reduce(result, acc),
                    Ok(Err(err)) => {
                        first_error.get_or_insert(err);
                    }
                    Err(_) => {
                        first_error.get_or_insert_with(|| "pbf decode worker panicked".to_string());
                    }
                }
            }
            match first_error {
                Some(err) => Err(err),
                None => Ok(result),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn zig(value: i64) -> u64 {
        ((value << 1) ^ (value >> 63)) as u64
    }

    fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
        varint(out, (u64::from(field) << 3) | u64::from(wire));
    }

    fn field_varint(out: &mut Vec<u8>, field: u32, value: u64) {
        key(out, field, 0);
        varint(out, value);
    }

    fn field_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
        key(out, field, 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn field_packed(out: &mut Vec<u8>, field: u32, values: &[u64]) {
        let mut packed = Vec::new();
        for &value in values {
            varint(&mut packed, value);
        }
        field_bytes(out, field, &packed);
    }

    const STRINGS: [&str; 11] = [
        "", "amenity", "bench", "highway", "residential", "name", "Main", "outer", "type",
        "multipolygon", "inner",
    ];

    fn header_block() -> Vec<u8> {
        let mut bbox = Vec::new();
        field_varint(&mut bbox, 1, zig(4_500_000_000));
        field_varint(&mut bbox, 2, zig(5_200_000_000));
        field_varint(&mut bbox, 3, zig(52_500_000_000));
        field_varint(&mut bbox, 4, zig(52_200_000_000));
        let mut out = Vec::new();
        field_bytes(&mut out, 1, &bbox);
        field_bytes(&mut out, 4, b"OsmSchema-V0.6");
        field_bytes(&mut out, 4, b"DenseNodes");
        field_bytes(&mut out, 5, b"Sort.Type_then_ID");
        field_bytes(&mut out, 16, b"osm_pbf test");
        field_varint(&mut out, 33, 42);
        out
    }

    /// Granularity 200, lat_offset 1000, lon_offset -1000; dense nodes
    /// 10 (amenity=bench), 11 (untagged), 12 (name, highway); plain node
    /// 20 with its keys in the unpacked encoding; closed way 100; relation
    /// 200 with an outer way and an inner node.
    fn data_block() -> Vec<u8> {
        let mut stringtable = Vec::new();
        for s in STRINGS {
            field_bytes(&mut stringtable, 1, s.as_bytes());
        }
        let mut dense = Vec::new();
        field_packed(&mut dense, 1, &[zig(10), zig(1), zig(1)]);
        field_packed(&mut dense, 8, &[zig(262_500_000), zig(-5), zig(10)]);
        field_packed(&mut dense, 9, &[zig(24_500_000), zig(7), zig(-3)]);
        field_packed(&mut dense, 10, &[1, 2, 0, 0, 5, 6, 3, 4, 0]);
        let mut node = Vec::new();
        field_varint(&mut node, 1, zig(20));
        field_varint(&mut node, 2, 5);
        field_varint(&mut node, 3, 6);
        field_varint(&mut node, 8, zig(-262_500_000));
        field_varint(&mut node, 9, zig(24_000_000));
        let mut way = Vec::new();
        field_varint(&mut way, 1, 100);
        field_packed(&mut way, 2, &[3]);
        field_packed(&mut way, 3, &[4]);
        field_packed(&mut way, 8, &[zig(10), zig(1), zig(1), zig(-2)]);
        let mut relation = Vec::new();
        field_varint(&mut relation, 1, 200);
        field_packed(&mut relation, 2, &[8]);
        field_packed(&mut relation, 3, &[9]);
        field_packed(&mut relation, 8, &[7, 10]);
        field_packed(&mut relation, 9, &[zig(100), zig(-80)]);
        field_packed(&mut relation, 10, &[1, 0]);
        let mut group = Vec::new();
        field_bytes(&mut group, 2, &dense);
        field_bytes(&mut group, 1, &node);
        field_bytes(&mut group, 3, &way);
        field_bytes(&mut group, 4, &relation);
        let mut out = Vec::new();
        field_bytes(&mut out, 1, &stringtable);
        field_bytes(&mut out, 2, &group);
        field_varint(&mut out, 17, 200);
        field_varint(&mut out, 19, 1000);
        field_varint(&mut out, 20, (-1000i64) as u64);
        out
    }

    fn frame(blob_type: &str, blob: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        field_bytes(&mut header, 1, blob_type.as_bytes());
        field_varint(&mut header, 3, blob.len() as u64);
        let mut out = Vec::new();
        out.extend_from_slice(&(header.len() as u32).to_be_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(blob);
        out
    }

    fn raw_blob(contents: &[u8]) -> Vec<u8> {
        let mut blob = Vec::new();
        field_bytes(&mut blob, 1, contents);
        blob
    }

    fn zlib_blob(contents: &[u8]) -> Vec<u8> {
        let mut blob = Vec::new();
        field_varint(&mut blob, 2, contents.len() as u64);
        field_bytes(&mut blob, 3, &makepad_fast_inflate::zlib_compress(contents, 6));
        blob
    }

    fn test_file() -> Vec<u8> {
        let mut file = frame("OSMHeader", &raw_blob(&header_block()));
        file.extend(frame("OSMData", &zlib_blob(&data_block())));
        file
    }

    fn degrees(offset: i64, granularity: i64, value: i64) -> f64 {
        1e-9 * (offset + granularity * value) as f64
    }

    fn element_id(element: &Element<'_>) -> i64 {
        match element {
            Element::Node(node) => node.id(),
            Element::DenseNode(node) => node.id(),
            Element::Way(way) => way.id(),
            Element::Relation(relation) => relation.id(),
        }
    }

    #[test]
    fn reads_header_and_every_element_accessor() {
        let mut blobs = BlobReader::new(Cursor::new(test_file()));
        let header = blobs.next().unwrap().unwrap();
        assert_eq!(header.get_type(), BlobType::OsmHeader);
        let header = header.to_headerblock().unwrap();
        assert_eq!(header.required_features(), ["OsmSchema-V0.6".to_string(), "DenseNodes".to_string()]);
        assert_eq!(header.optional_features(), ["Sort.Type_then_ID".to_string()]);
        let bbox = header.bbox().unwrap();
        assert!((bbox.left - 4.5).abs() < 1e-12 && (bbox.right - 5.2).abs() < 1e-12);
        assert!((bbox.top - 52.5).abs() < 1e-12 && (bbox.bottom - 52.2).abs() < 1e-12);
        assert_eq!(header.writing_program(), Some("osm_pbf test"));

        let data = blobs.next().unwrap().unwrap();
        assert_eq!(data.get_type(), BlobType::OsmData);
        let BlobDecode::OsmData(block) = data.decode().unwrap() else {
            panic!("expected OSMData");
        };
        assert!(blobs.next().is_none());

        let elements: Vec<Element<'_>> = block.elements().collect();
        assert_eq!(elements.iter().map(element_id).collect::<Vec<_>>(), [10, 11, 12, 20, 100, 200]);
        let Element::DenseNode(node) = &elements[0] else { panic!("dense node first") };
        assert_eq!(node.lat(), degrees(1000, 200, 262_500_000));
        assert_eq!(node.lon(), degrees(-1000, 200, 24_500_000));
        assert_eq!(node.tags().collect::<Vec<_>>(), [("amenity", "bench")]);
        let Element::DenseNode(node) = &elements[1] else { panic!("dense node") };
        assert_eq!(node.lat(), degrees(1000, 200, 262_499_995));
        assert_eq!(node.lon(), degrees(-1000, 200, 24_500_007));
        assert_eq!(node.tags().count(), 0);
        let Element::DenseNode(node) = &elements[2] else { panic!("dense node") };
        assert_eq!(node.lat(), degrees(1000, 200, 262_500_005));
        assert_eq!(node.lon(), degrees(-1000, 200, 24_500_004));
        assert_eq!(node.tags().collect::<Vec<_>>(), [("name", "Main"), ("highway", "residential")]);
        let Element::Node(node) = &elements[3] else { panic!("node") };
        assert_eq!(node.lat(), degrees(1000, 200, -262_500_000));
        assert_eq!(node.lon(), degrees(-1000, 200, 24_000_000));
        assert_eq!(node.tags().collect::<Vec<_>>(), [("name", "Main")]);
        let Element::Way(way) = &elements[4] else { panic!("way") };
        assert_eq!(way.tags().collect::<Vec<_>>(), [("highway", "residential")]);
        assert_eq!(way.refs().len(), 4);
        assert_eq!(way.refs().collect::<Vec<_>>(), [10, 11, 12, 10]);
        let Element::Relation(relation) = &elements[5] else { panic!("relation") };
        assert_eq!(relation.tags().collect::<Vec<_>>(), [("type", "multipolygon")]);
        let members: Vec<_> = relation.members().collect();
        assert_eq!(relation.members().len(), 2);
        assert_eq!(
            (members[0].member_id, members[0].member_type, members[0].role().unwrap()),
            (100, RelMemberType::Way, "outer")
        );
        assert_eq!(
            (members[1].member_id, members[1].member_type, members[1].role().unwrap()),
            (20, RelMemberType::Node, "inner")
        );
    }

    #[test]
    fn element_reader_walks_serially_and_in_parallel() {
        let mut ids = Vec::new();
        ElementReader::new(Cursor::new(test_file()))
            .for_each(|element| ids.push(element_id(&element)))
            .unwrap();
        assert_eq!(ids, [10, 11, 12, 20, 100, 200]);
        // Two data blobs, so more than one worker has a block to fold.
        let mut file = test_file();
        file.extend(frame("OSMData", &zlib_blob(&data_block())));
        let (count, id_sum) = ElementReader::new(Cursor::new(file))
            .par_map_reduce(
                |element| (1u64, element_id(&element)),
                || (0u64, 0i64),
                |a, b| (a.0 + b.0, a.1 + b.1),
            )
            .unwrap();
        assert_eq!((count, id_sum), (12, 2 * (10 + 11 + 12 + 20 + 100 + 200)));
    }

    #[test]
    fn rejects_unsupported_compression_and_bad_framing() {
        let mut lzma = Vec::new();
        field_bytes(&mut lzma, 4, b"not lzma");
        let blob = BlobReader::new(Cursor::new(frame("OSMData", &lzma))).next().unwrap().unwrap();
        assert!(blob.decode().unwrap_err().contains("lzma"));

        let compressed = makepad_fast_inflate::zlib_compress(&data_block(), 6);
        let mut wrong_size = Vec::new();
        field_varint(&mut wrong_size, 2, 3);
        field_bytes(&mut wrong_size, 3, &compressed);
        let blob = BlobReader::new(Cursor::new(frame("OSMData", &wrong_size))).next().unwrap().unwrap();
        assert!(blob.decode().unwrap_err().contains("raw_size"));

        // A claimed size past the limit is refused before any inflate.
        let mut oversized_claim = Vec::new();
        field_varint(&mut oversized_claim, 2, 40 * 1024 * 1024);
        field_bytes(&mut oversized_claim, 3, &compressed);
        let blob = BlobReader::new(Cursor::new(frame("OSMData", &oversized_claim))).next().unwrap().unwrap();
        assert!(blob.decode().unwrap_err().contains("32 MiB"));

        let mut no_size = Vec::new();
        field_bytes(&mut no_size, 3, &compressed);
        let blob = BlobReader::new(Cursor::new(frame("OSMData", &no_size))).next().unwrap().unwrap();
        assert!(blob.decode().unwrap_err().contains("raw_size"));

        let mut huge_datasize = Vec::new();
        field_bytes(&mut huge_datasize, 1, b"OSMData");
        field_varint(&mut huge_datasize, 3, 1 << 40);
        let mut file = (huge_datasize.len() as u32).to_be_bytes().to_vec();
        file.extend_from_slice(&huge_datasize);
        let err = BlobReader::new(Cursor::new(file)).next().unwrap().unwrap_err();
        assert!(err.contains("int32"));

        let mut truncated = test_file();
        truncated.truncate(truncated.len() - 10);
        let mut blobs = BlobReader::new(Cursor::new(truncated));
        assert!(blobs.next().unwrap().is_ok());
        assert!(blobs.next().unwrap().is_err());
        assert!(blobs.next().is_none());

        let oversized = 70_000u32.to_be_bytes().to_vec();
        let err = BlobReader::new(Cursor::new(oversized)).next().unwrap().unwrap_err();
        assert!(err.contains("64 KiB"));

        let err = ElementReader::new(Cursor::new(frame("OSMData", &lzma)))
            .par_map_reduce(|_| 1u64, || 0, |a, b| a + b)
            .unwrap_err();
        assert!(err.contains("lzma"));
    }
}
