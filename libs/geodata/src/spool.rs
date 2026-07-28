//! Disk-spooling tiler for layers too large for the in-memory `Tileset`
//! (BAG: ~11M polygons). One pass clips features and appends compact records
//! to one spool file per (zoom, 256x256 block); NL covers only a handful of
//! blocks per zoom, so the second pass loads one block at a time — in the
//! mbtiles writer's required order — and encodes its tiles. Peak memory is
//! one block, not the whole country.

use crate::mvt::{AttrVal, GeomType, PreFeature, TileEnc};
use crate::tiler::{
    create_writer, empty_lonlat_bounds, geometry_to_tiles, gzip_tile, note_fields,
    TilesetConfig, TilesetStats,
};
use crate::wkb::Geometry;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub struct SpoolTiler {
    dir: PathBuf,
    zmin: u8,
    zmax: u8,
    writers: HashMap<(u8, u32, u32), BufWriter<std::fs::File>>,
    strings: Vec<String>,
    string_index: HashMap<String, u32>,
    fields: HashMap<String, HashMap<String, &'static str>>,
    bounds: (f64, f64, f64, f64),
    features_in: u64,
    tile_features: u64,
    sidecar: crate::sidecar::SidecarBuilder,
    ring_layers: Vec<String>,
}

impl SpoolTiler {
    pub fn new(dir: &Path, zmin: u8, zmax: u8) -> Result<Self, String> {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("clear spool dir: {e}"))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("create spool dir: {e}"))?;
        Ok(SpoolTiler {
            dir: dir.to_path_buf(),
            zmin,
            zmax,
            writers: HashMap::new(),
            strings: Vec::new(),
            string_index: HashMap::new(),
            fields: HashMap::new(),
            bounds: empty_lonlat_bounds(),
            features_in: 0,
            tile_features: 0,
            sidecar: crate::sidecar::SidecarBuilder::new(),
            ring_layers: Vec::new(),
        })
    }

    /// See [`crate::tiler::Tileset::query_rings`].
    pub fn query_rings(&mut self, layers: &[&str]) {
        self.ring_layers = layers.iter().map(|s| s.to_string()).collect();
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.string_index.get(s) {
            return id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.string_index.insert(s.to_string(), id);
        id
    }

    pub fn add(
        &mut self,
        layer: &str,
        geometry: &Geometry,
        attrs: &[(String, AttrVal)],
    ) -> Result<(), String> {
        self.features_in += 1;
        note_fields(&mut self.fields, layer, attrs);
        let want_ring = self.ring_layers.iter().any(|l| l == layer);
        self.sidecar.add(layer, geometry, attrs, want_ring);
        let layer_id = self.intern(layer);

        // Pre-encode attrs once per source feature (shared by all its tiles).
        let mut attr_buf = Vec::new();
        write_varint(attrs.len() as u64, &mut attr_buf);
        for (key, value) in attrs {
            let key_id = self.intern(key);
            write_varint(u64::from(key_id), &mut attr_buf);
            match value {
                AttrVal::Int(i) => {
                    attr_buf.push(0);
                    write_varint(zigzag64(*i), &mut attr_buf);
                }
                AttrVal::Float(f) => {
                    attr_buf.push(1);
                    attr_buf.extend_from_slice(&f.to_le_bytes());
                }
                AttrVal::Str(s) => {
                    attr_buf.push(2);
                    let sid = self.intern(s);
                    write_varint(u64::from(sid), &mut attr_buf);
                }
                AttrVal::Bool(b) => {
                    attr_buf.push(3);
                    attr_buf.push(u8::from(*b));
                }
            }
        }

        let zmin = self.zmin;
        let zmax = self.zmax;
        let mut emitted: Vec<(u8, u32, u32, PreFeature)> = Vec::new();
        geometry_to_tiles(geometry, zmin, zmax, &[], &mut self.bounds, &mut |z, x, y, f| {
            emitted.push((z, x, y, f));
        });
        for (zoom, x, y, feature) in emitted {
            self.tile_features += 1;
            let block = (zoom, x >> 8, y >> 8);
            let writer = match self.writers.entry(block) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let path = self
                        .dir
                        .join(format!("z{}-bx{}-by{}.spool", block.0, block.1, block.2));
                    let file = std::fs::File::create(&path)
                        .map_err(|e| format!("create {}: {e}", path.display()))?;
                    e.insert(BufWriter::with_capacity(1 << 20, file))
                }
            };
            let local = (((y & 255) << 8) | (x & 255)) as u16;
            let mut rec = Vec::with_capacity(feature.commands.len() * 3 + attr_buf.len() + 8);
            rec.extend_from_slice(&local.to_le_bytes());
            write_varint(u64::from(layer_id), &mut rec);
            rec.push(feature.geom_type as u8);
            write_varint(feature.commands.len() as u64, &mut rec);
            for c in &feature.commands {
                write_varint(u64::from(*c), &mut rec);
            }
            rec.extend_from_slice(&attr_buf);
            let mut framed = Vec::with_capacity(rec.len() + 4);
            write_varint(rec.len() as u64, &mut framed);
            framed.extend_from_slice(&rec);
            writer
                .write_all(&framed)
                .map_err(|e| format!("spool write: {e}"))?;
        }
        Ok(())
    }

    pub fn finish(mut self, out_path: &Path, config: &TilesetConfig) -> Result<TilesetStats, String> {
        let mut blocks: Vec<(u8, u32, u32)> = self.writers.keys().copied().collect();
        for (_, writer) in self.writers.iter_mut() {
            writer.flush().map_err(|e| format!("spool flush: {e}"))?;
        }
        self.writers.clear();
        // Writer rowid order: zoom asc, then block row-major, then local
        // row-major (handled by the BTreeMap below).
        blocks.sort_by_key(|&(z, bx, by)| (z, by, bx));
        eprintln!("  spool: {} blocks, {} tile-features", blocks.len(), self.tile_features);

        let mut writer = create_writer(out_path, config, &self.fields, self.bounds)?;
        let sidecar = std::mem::take(&mut self.sidecar);
        let feature_rows = sidecar.write(&mut writer)?;
        eprintln!("  sidecar: {feature_rows} queryable features");
        let mut stats = TilesetStats {
            features_in: self.features_in,
            tile_features: self.tile_features,
            ..Default::default()
        };
        for (zoom, bx, by) in blocks {
            let path = self
                .dir
                .join(format!("z{zoom}-bx{bx}-by{by}.spool"));
            let mut data = Vec::new();
            std::fs::File::open(&path)
                .and_then(|mut f| f.read_to_end(&mut data))
                .map_err(|e| format!("read {}: {e}", path.display()))?;

            let mut tiles: BTreeMap<u16, Vec<(u32, PreFeature)>> = BTreeMap::new();
            let mut pos = 0usize;
            while pos < data.len() {
                let (rec_len, n) = read_varint(&data[pos..]).ok_or("corrupt spool")?;
                pos += n;
                let rec = &data[pos..pos + rec_len as usize];
                pos += rec_len as usize;

                let local = u16::from_le_bytes([rec[0], rec[1]]);
                let mut p = 2usize;
                let (layer_id, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                p += n;
                let geom_type = match rec[p] {
                    1 => GeomType::Point,
                    2 => GeomType::Line,
                    3 => GeomType::Polygon,
                    _ => return Err("corrupt spool geom type".into()),
                };
                p += 1;
                let (n_cmds, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                p += n;
                let mut commands = Vec::with_capacity(n_cmds as usize);
                for _ in 0..n_cmds {
                    let (c, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                    p += n;
                    commands.push(c as u32);
                }
                let (n_attrs, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                p += n;
                let mut attrs = Vec::with_capacity(n_attrs as usize);
                for _ in 0..n_attrs {
                    let (key_id, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                    p += n;
                    let key = self.strings[key_id as usize].clone();
                    let tag = rec[p];
                    p += 1;
                    let value = match tag {
                        0 => {
                            let (v, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                            p += n;
                            AttrVal::Int(unzigzag64(v))
                        }
                        1 => {
                            let bytes: [u8; 8] =
                                rec[p..p + 8].try_into().map_err(|_| "corrupt spool")?;
                            p += 8;
                            AttrVal::Float(f64::from_le_bytes(bytes))
                        }
                        2 => {
                            let (sid, n) = read_varint(&rec[p..]).ok_or("corrupt spool")?;
                            p += n;
                            AttrVal::Str(self.strings[sid as usize].clone())
                        }
                        3 => {
                            let v = rec[p] != 0;
                            p += 1;
                            AttrVal::Bool(v)
                        }
                        _ => return Err("corrupt spool attr tag".into()),
                    };
                    attrs.push((key, value));
                }
                tiles.entry(local).or_default().push((
                    layer_id as u32,
                    PreFeature {
                        geom_type,
                        commands,
                        attrs,
                    },
                ));
            }
            drop(data);

            for (local, features) in tiles {
                let lx = u32::from(local & 255);
                let ly = u32::from(local >> 8);
                let mut enc = TileEnc::new();
                for (layer_id, feature) in &features {
                    enc.add_feature(&self.strings[*layer_id as usize], feature);
                }
                let tile_data = gzip_tile(&enc.encode())?;
                let x = (bx << 8) | lx;
                let y = (by << 8) | ly;
                writer
                    .write_tile_xyz(zoom, x, y, &tile_data)
                    .map_err(|e| format!("write tile z{zoom}/{x}/{y}: {e:?}"))?;
                stats.tiles += 1;
                stats.bytes += tile_data.len() as u64;
            }
            let _ = std::fs::remove_file(&path);
        }
        writer.finish().map_err(|e| format!("finish mbtiles: {e:?}"))?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(stats)
    }
}

fn zigzag64(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag64(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (i, &byte) in data.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}
