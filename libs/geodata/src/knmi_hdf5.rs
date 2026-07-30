//! Minimal pure-Rust HDF5 reader for KNMI radar files.
//!
//! This is NOT a general HDF5 implementation. KNMI's radar products are
//! written by a fixed, old HDF5 writer and always look the same:
//! superblock v0, symbol-table groups (B-tree v1 + local heap), datasets
//! with v1 object headers, contiguous-or-chunked layout v3, and a deflate
//! filter with ONE chunk spanning the whole image. We walk exactly that
//! shape and nothing more; anything unexpected returns an error instead of
//! guessing. Times and geo constants come from the filename and the
//! documented RAD_NL25 grid, so no attribute parsing is needed.

use makepad_fast_inflate::DecompressError;

pub struct Hdf5File<'a> {
    data: &'a [u8],
}

/// Decoded HDF5 attribute value (KNMI files use f32/i32 arrays and strings).
#[derive(Debug, Clone)]
pub enum AttrValue {
    Floats(Vec<f64>),
    Ints(Vec<i64>),
    Text(String),
}

impl AttrValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            AttrValue::Floats(v) => v.first().copied(),
            AttrValue::Ints(v) => v.first().map(|&i| i as f64),
            AttrValue::Text(_) => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            AttrValue::Ints(v) => v.first().copied(),
            AttrValue::Floats(v) => v.first().map(|&f| f as i64),
            AttrValue::Text(_) => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            AttrValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct DatasetInfo {
    /// (rows, cols) from the dataspace message.
    pub dims: (u64, u64),
    /// Raw (still-compressed if filtered) chunk bytes.
    chunk_offset: u64,
    chunk_size: u64,
    /// Deflate filter present.
    deflated: bool,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}

impl<'a> Hdf5File<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self, String> {
        if data.len() < 96 || &data[0..8] != b"\x89HDF\r\n\x1a\n" {
            return Err("not an HDF5 file".into());
        }
        // Superblock v0 with 8-byte offsets/lengths is the only layout the
        // KNMI writer produces.
        if data[8] != 0 {
            return Err(format!("unsupported superblock version {}", data[8]));
        }
        if data[13] != 8 || data[14] != 8 {
            return Err("unsupported offset/length size".into());
        }
        Ok(Self { data })
    }

    /// Object header address of the root group.
    fn root_object_header(&self) -> u64 {
        // Superblock v0: root group symbol table entry at offset 24 + 4*8.
        // Symbol table entry: link_name_offset(8) object_header_addr(8) ...
        u64le(self.data, 24 + 32 + 8)
    }

    /// Find a child object (group or dataset) by name inside the group whose
    /// object header sits at `header_addr`.
    pub fn find_child(&self, header_addr: u64, name: &str) -> Result<Option<u64>, String> {
        let (btree, heap) = self.group_symbol_table(header_addr)?;
        let mut found = None;
        self.walk_group_btree(btree, heap, &mut |child_name, child_addr| {
            if child_name == name {
                found = Some(child_addr);
            }
        })?;
        Ok(found)
    }

    pub fn find_path(&self, path: &[&str]) -> Result<Option<u64>, String> {
        let mut at = self.root_object_header();
        for part in path {
            match self.find_child(at, part)? {
                Some(next) => at = next,
                None => return Ok(None),
            }
        }
        Ok(Some(at))
    }

    /// Parse the SYMBOL_TABLE message (type 0x11) of a group object header.
    fn group_symbol_table(&self, header_addr: u64) -> Result<(u64, u64), String> {
        let mut result = None;
        self.walk_object_header(header_addr, &mut |msg_type, body| {
            if msg_type == 0x0011 && body.len() >= 16 {
                result = Some((u64le(body, 0), u64le(body, 8)));
            }
        })?;
        result.ok_or_else(|| "object is not a symbol-table group".into())
    }

    /// Iterate a group B-tree (v1, node type 0) yielding (name, header_addr).
    fn walk_group_btree(
        &self,
        btree_addr: u64,
        heap_addr: u64,
        visit: &mut impl FnMut(&str, u64),
    ) -> Result<(), String> {
        let d = self.data;
        let o = btree_addr as usize;
        if d.len() < o + 24 || &d[o..o + 4] != b"TREE" {
            return Err("bad group btree node".into());
        }
        let node_type = d[o + 4];
        let node_level = d[o + 5];
        let entries = u16le(d, o + 6) as usize;
        if node_type != 0 {
            return Err("unexpected btree node type".into());
        }
        // keys/children start after: sig(4) type(1) level(1) entries(2)
        // left(8) right(8) = 24; layout: key0 child0 key1 child1 ... keyN
        let mut pos = o + 24 + 8; // skip key0 (length-size offset into heap)
        for _ in 0..entries {
            let child = u64le(d, pos);
            pos += 8 + 8; // child + next key
            if node_level > 0 {
                self.walk_group_btree(child, heap_addr, visit)?;
            } else {
                self.walk_snod(child, heap_addr, visit)?;
            }
        }
        Ok(())
    }

    /// Symbol node: list of symbol table entries.
    fn walk_snod(
        &self,
        snod_addr: u64,
        heap_addr: u64,
        visit: &mut impl FnMut(&str, u64),
    ) -> Result<(), String> {
        let d = self.data;
        let o = snod_addr as usize;
        if d.len() < o + 8 || &d[o..o + 4] != b"SNOD" {
            return Err("bad symbol node".into());
        }
        let count = u16le(d, o + 6) as usize;
        // Local heap: signature HEAP, version, data segment address at +24.
        let h = heap_addr as usize;
        if d.len() < h + 32 || &d[h..h + 4] != b"HEAP" {
            return Err("bad local heap".into());
        }
        let heap_data = u64le(d, h + 24) as usize;
        let mut pos = o + 8;
        for _ in 0..count {
            let name_off = u64le(d, pos) as usize;
            let header = u64le(d, pos + 8);
            pos += 40; // symbol table entry is 40 bytes with 8-byte offsets
            let name_start = heap_data + name_off;
            let mut end = name_start;
            while end < d.len() && d[end] != 0 {
                end += 1;
            }
            if let Ok(name) = std::str::from_utf8(&d[name_start..end]) {
                visit(name, header);
            }
        }
        Ok(())
    }

    /// Iterate the messages of a v1 object header, following continuation
    /// blocks (message type 0x0010).
    fn walk_object_header(
        &self,
        header_addr: u64,
        visit: &mut impl FnMut(u16, &[u8]),
    ) -> Result<(), String> {
        let d = self.data;
        let o = header_addr as usize;
        if d.len() < o + 16 || d[o] != 1 {
            return Err("unsupported object header version".into());
        }
        let mut remaining_msgs = u16le(d, o + 2) as usize;
        // v1 header: version(1) pad(1) nmsgs(2) refcount(4) header_size(4)
        // then padding to 8-byte alignment: messages start at +16.
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        let first_size = u32le(d, o + 8) as usize;
        blocks.push((o + 16, first_size));
        let mut block_index = 0;
        while block_index < blocks.len() {
            let (mut pos, size) = blocks[block_index];
            let block_end = pos + size;
            while pos + 8 <= block_end && remaining_msgs > 0 {
                let msg_type = u16le(d, pos);
                let msg_size = u16le(d, pos + 2) as usize;
                let body = &d[pos + 8..(pos + 8 + msg_size).min(d.len())];
                if msg_type == 0x0010 && body.len() >= 16 {
                    blocks.push((u64le(body, 0) as usize, u64le(body, 8) as usize));
                } else {
                    visit(msg_type, body);
                }
                remaining_msgs -= 1;
                pos += 8 + msg_size;
            }
            block_index += 1;
        }
        Ok(())
    }

    /// Layout + dataspace + filters of a dataset object header.
    pub fn dataset_info(&self, header_addr: u64) -> Result<DatasetInfo, String> {
        let mut dims: Option<(u64, u64)> = None;
        let mut deflated = false;
        let mut chunk: Option<(u64, (u64, u64))> = None; // btree addr, chunk dims
        let mut contiguous: Option<(u64, u64)> = None;
        self.walk_object_header(header_addr, &mut |msg_type, body| match msg_type {
            0x0001 => {
                // Dataspace v1: version(1) rank(1) flags(1) reserved(5) dims...
                if body.len() >= 8 && body[0] == 1 {
                    let rank = body[1] as usize;
                    if rank == 2 && body.len() >= 8 + 16 {
                        dims = Some((u64le(body, 8), u64le(body, 16)));
                    }
                }
            }
            0x0008 => {
                // Layout v3.
                if body.len() >= 2 && body[0] == 3 {
                    match body[1] {
                        1 if body.len() >= 18 => {
                            contiguous = Some((u64le(body, 2), u64le(body, 10)));
                        }
                        2 => {
                            // chunked: dimensionality(1) btree(8) dims(4*each)
                            let rank = body[2] as usize;
                            if body.len() >= 3 + 8 + rank * 4 {
                                let btree = u64le(body, 3);
                                let d0 = u32le(body, 11) as u64;
                                let d1 = u32le(body, 15) as u64;
                                chunk = Some((btree, (d0, d1)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            0x000B => {
                // Filter pipeline: any deflate (filter id 1) counts.
                if body.len() >= 2 {
                    deflated = true;
                }
            }
            _ => {}
        })?;
        let dims = dims.ok_or("dataset without 2D dataspace")?;
        if let Some((btree_addr, _chunk_dims)) = chunk {
            // Chunk B-tree v1 (node type 1). KNMI writes ONE chunk per
            // dataset, so the root node is a leaf with a single entry.
            let d = self.data;
            let o = btree_addr as usize;
            if d.len() < o + 24 || &d[o..o + 4] != b"TREE" || d[o + 4] != 1 {
                return Err("bad chunk btree".into());
            }
            let level = d[o + 5];
            let entries = u16le(d, o + 6) as usize;
            if level != 0 || entries != 1 {
                return Err(format!(
                    "unexpected chunk btree shape (level {level}, {entries} entries)"
                ));
            }
            // key: chunk_size(4) filter_mask(4) offsets((rank+1)*8) then child(8).
            // rank for a 2D dataset's chunk key is 3 (row, col, element).
            let key = o + 24;
            let chunk_size = u32le(d, key) as u64;
            let child = u64le(d, key + 8 + 3 * 8);
            Ok(DatasetInfo {
                dims,
                chunk_offset: child,
                chunk_size,
                deflated,
            })
        } else if let Some((addr, size)) = contiguous {
            Ok(DatasetInfo {
                dims,
                chunk_offset: addr,
                chunk_size: size,
                deflated: false,
            })
        } else {
            Err("dataset without layout".into())
        }
    }

    /// Parse one v1 attribute message body into (name, value).
    fn parse_attribute(body: &[u8]) -> Option<(String, AttrValue)> {
        if body.len() < 8 || body[0] != 1 {
            return None;
        }
        let name_size = u16le(body, 2) as usize;
        let datatype_size = u16le(body, 4) as usize;
        let dataspace_size = u16le(body, 6) as usize;
        let pad8 = |n: usize| (n + 7) & !7;
        let name_start = 8;
        let dt_start = name_start + pad8(name_size);
        let ds_start = dt_start + pad8(datatype_size);
        let data_start = ds_start + pad8(dataspace_size);
        if data_start > body.len() {
            return None;
        }
        let name_bytes = &body[name_start..name_start + name_size.min(body.len() - name_start)];
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
        let name = std::str::from_utf8(&name_bytes[..name_end]).ok()?.to_string();
        // Datatype: byte0 = (version<<4)|class, bytes 4..8 = element size.
        let dt = &body[dt_start..dt_start + datatype_size.min(body.len() - dt_start)];
        if dt.len() < 8 {
            return None;
        }
        let class = dt[0] & 0x0f;
        let elem_size = u32le(dt, 4) as usize;
        // Dataspace v1: version(1) rank(1) flags(1) reserved(5) dims[rank]*8.
        let ds = &body[ds_start..ds_start + dataspace_size.min(body.len() - ds_start)];
        let mut count = 1usize;
        if ds.len() >= 8 && ds[0] == 1 {
            let rank = ds[1] as usize;
            if ds.len() >= 8 + rank * 8 {
                for dim in 0..rank {
                    count *= u64le(ds, 8 + dim * 8) as usize;
                }
            }
        }
        let data = &body[data_start..];
        if data.len() < count * elem_size {
            return None;
        }
        match class {
            0 => {
                // Fixed-point (KNMI writes i32).
                let mut values = Vec::with_capacity(count);
                for index in 0..count {
                    let o = index * elem_size;
                    values.push(match elem_size {
                        4 => u32le(data, o) as i32 as i64,
                        8 => u64le(data, o) as i64,
                        2 => u16le(data, o) as i16 as i64,
                        1 => data[o] as i8 as i64,
                        _ => return None,
                    });
                }
                Some((name, AttrValue::Ints(values)))
            }
            1 => {
                let mut values = Vec::with_capacity(count);
                for index in 0..count {
                    let o = index * elem_size;
                    values.push(match elem_size {
                        4 => f32::from_bits(u32le(data, o)) as f64,
                        8 => f64::from_bits(u64le(data, o)),
                        _ => return None,
                    });
                }
                Some((name, AttrValue::Floats(values)))
            }
            3 => {
                let text = &data[..count * elem_size];
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                Some((name, AttrValue::Text(String::from_utf8_lossy(&text[..end]).into_owned())))
            }
            _ => None,
        }
    }

    /// Look up one attribute of the object at `header_addr` by name.
    pub fn attr(&self, header_addr: u64, name: &str) -> Result<Option<AttrValue>, String> {
        let mut found = None;
        self.walk_object_header(header_addr, &mut |msg_type, body| {
            if msg_type == 0x000C && found.is_none() {
                if let Some((attr_name, value)) = Self::parse_attribute(body) {
                    if attr_name == name {
                        found = Some(value);
                    }
                }
            }
        })?;
        Ok(found)
    }

    /// Read + (if needed) inflate the dataset's single chunk.
    pub fn read_dataset(&self, info: &DatasetInfo) -> Result<Vec<u8>, String> {
        let start = info.chunk_offset as usize;
        let end = start + info.chunk_size as usize;
        if end > self.data.len() {
            return Err("chunk out of bounds".into());
        }
        let raw = &self.data[start..end];
        if info.deflated {
            makepad_fast_inflate::zlib_decompress_vec(raw)
                .map_err(|err: DecompressError| format!("inflate: {err:?}"))
        } else {
            Ok(raw.to_vec())
        }
    }

    /// Read a dataset of little-endian u16 values (radar volume scan data).
    pub fn read_dataset_u16(&self, info: &DatasetInfo) -> Result<Vec<u16>, String> {
        let bytes = self.read_dataset(info)?;
        if bytes.len() % 2 != 0 {
            return Err(format!("odd byte count {} for u16 dataset", bytes.len()));
        }
        Ok(bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect())
    }
}

/// One decoded nowcast frame: 765x700 raw pixel values (0.5*PV-32 dBZ).
#[derive(Clone)]
pub struct KnmiFrame {
    pub minutes_offset: u32,
    pub rows: usize,
    pub cols: usize,
    pub values: Vec<u8>,
}

/// Decode every `imageN/image_data` frame of a KNMI radar file, in frame
/// order (image1 = +0 min, each subsequent +5 min).
pub fn decode_frames(data: &[u8]) -> Result<Vec<KnmiFrame>, String> {
    let file = Hdf5File::open(data)?;
    let mut frames = Vec::new();
    for index in 1..=64 {
        let group = format!("image{index}");
        let Some(ds) = file.find_path(&[&group, "image_data"])? else {
            break;
        };
        let info = file.dataset_info(ds)?;
        let values = file.read_dataset(&info)?;
        let (rows, cols) = (info.dims.0 as usize, info.dims.1 as usize);
        if values.len() != rows * cols {
            return Err(format!(
                "frame {index}: {} bytes for {}x{}",
                values.len(),
                rows,
                cols
            ));
        }
        frames.push(KnmiFrame {
            minutes_offset: (index as u32 - 1) * 5,
            rows,
            cols,
            values,
        });
    }
    if frames.is_empty() {
        return Err("no image groups found".into());
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_volume_attributes() {
        let path =
            "../../local/overlays/radar_test/RAD_NL62_VOL_NA_202607301810.h5";
        let Ok(data) = std::fs::read(path) else {
            return;
        };
        let file = Hdf5File::open(&data).unwrap();
        let radar = file.find_path(&["radar1"]).unwrap().unwrap();
        // Reference values from h5py over the same file.
        let loc = file.attr(radar, "radar_location").unwrap().unwrap();
        let AttrValue::Floats(loc) = loc else {
            panic!("radar_location not floats")
        };
        assert!((loc[0] - 5.1381).abs() < 1e-3 && (loc[1] - 51.8369).abs() < 1e-3);
        let name = file.attr(radar, "radar_name").unwrap().unwrap();
        assert_eq!(name.as_text(), Some("Herwijnen"));
        let scan = file.find_path(&["scan6"]).unwrap().unwrap();
        assert!((file.attr(scan, "scan_elevation").unwrap().unwrap().as_f64().unwrap() - 0.8).abs() < 1e-4);
        assert!((file.attr(scan, "scan_range_bin").unwrap().unwrap().as_f64().unwrap() - 0.2235).abs() < 1e-5);
        assert_eq!(file.attr(scan, "scan_number_range").unwrap().unwrap().as_i64(), Some(838));
        assert_eq!(file.attr(scan, "scan_number_azim").unwrap().unwrap().as_i64(), Some(360));
        let cal = file.find_path(&["scan6", "calibration"]).unwrap().unwrap();
        let formula = file.attr(cal, "calibration_Z_formulas").unwrap().unwrap();
        assert_eq!(formula.as_text(), Some("GEO=0.00193793*PV+-31.5019"));
        let ds = file.find_path(&["scan6", "scan_Z_data"]).unwrap().unwrap();
        let info = file.dataset_info(ds).unwrap();
        assert_eq!(info.dims, (360, 838));
        let values = file.read_dataset_u16(&info).unwrap();
        assert_eq!(values.len(), 360 * 838);
    }

    #[test]
    fn decodes_cached_forecast_file() {
        let path = "../../local/overlays/radar/forecast/RAD_NL25_PCP_FM_202607280900.h5";
        let Ok(data) = std::fs::read(path) else {
            return;
        };
        let frames = decode_frames(&data).unwrap();
        assert_eq!(frames.len(), 25);
        let f1 = &frames[0];
        assert_eq!((f1.rows, f1.cols), (765, 700));
        let sum: u64 = f1.values.iter().map(|&v| v as u64).sum();
        let nonzero = f1.values.iter().filter(|&&v| v > 0).count();
        // Reference values from h5py over the same file.
        assert_eq!(sum, 367924);
        assert_eq!(nonzero, 5068);
        let f25 = &frames[24];
        let sum25: u64 = f25.values.iter().map(|&v| v as u64).sum();
        assert_eq!(sum25, 522342);
    }
}
