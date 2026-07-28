//! Minimal Mapbox Vector Tile (MVT 2.1) encoder. Extent 4096, gzip applied by
//! the tiler so the map renderer's payload sniffing works unchanged.

use std::collections::HashMap;

pub const EXTENT: u32 = 4096;

#[derive(Debug, Clone, PartialEq)]
pub enum AttrVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeomType {
    Point = 1,
    Line = 2,
    Polygon = 3,
}

pub struct PreFeature {
    pub geom_type: GeomType,
    /// Fully encoded geometry command stream (zigzag deltas included).
    pub commands: Vec<u32>,
    pub attrs: Vec<(String, AttrVal)>,
}

struct LayerEnc {
    name: String,
    keys: Vec<String>,
    key_index: HashMap<String, u32>,
    values: Vec<AttrVal>,
    value_index: HashMap<String, u32>,
    features: Vec<Vec<u8>>,
}

impl LayerEnc {
    fn new(name: &str) -> Self {
        LayerEnc {
            name: name.to_string(),
            keys: Vec::new(),
            key_index: HashMap::new(),
            values: Vec::new(),
            value_index: HashMap::new(),
            features: Vec::new(),
        }
    }

    fn key_id(&mut self, key: &str) -> u32 {
        if let Some(&id) = self.key_index.get(key) {
            return id;
        }
        let id = self.keys.len() as u32;
        self.keys.push(key.to_string());
        self.key_index.insert(key.to_string(), id);
        id
    }

    fn value_id(&mut self, value: &AttrVal) -> u32 {
        let dedup_key = match value {
            AttrVal::Str(s) => format!("s\u{1}{s}"),
            AttrVal::Int(i) => format!("i\u{1}{i}"),
            AttrVal::Float(f) => format!("f\u{1}{:016x}", f.to_bits()),
            AttrVal::Bool(b) => format!("b\u{1}{b}"),
        };
        if let Some(&id) = self.value_index.get(&dedup_key) {
            return id;
        }
        let id = self.values.len() as u32;
        self.values.push(value.clone());
        self.value_index.insert(dedup_key, id);
        id
    }

    fn add_feature(&mut self, feature: &PreFeature) {
        let mut tags = Vec::with_capacity(feature.attrs.len() * 2);
        for (key, value) in &feature.attrs {
            tags.push(self.key_id(key));
            tags.push(self.value_id(value));
        }
        let mut buf = Vec::with_capacity(feature.commands.len() * 2 + tags.len() * 2 + 8);
        // tags (field 2, packed)
        if !tags.is_empty() {
            let mut packed = Vec::with_capacity(tags.len() * 2);
            for tag in &tags {
                write_varint(u64::from(*tag), &mut packed);
            }
            write_tag(2, 2, &mut buf);
            write_varint(packed.len() as u64, &mut buf);
            buf.extend_from_slice(&packed);
        }
        // type (field 3)
        write_tag(3, 0, &mut buf);
        write_varint(feature.geom_type as u64, &mut buf);
        // geometry (field 4, packed)
        let mut packed = Vec::with_capacity(feature.commands.len() * 2);
        for command in &feature.commands {
            write_varint(u64::from(*command), &mut packed);
        }
        write_tag(4, 2, &mut buf);
        write_varint(packed.len() as u64, &mut buf);
        buf.extend_from_slice(&packed);

        self.features.push(buf);
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let mut layer = Vec::new();
        // version (field 15)
        write_tag(15, 0, &mut layer);
        write_varint(2, &mut layer);
        // name (field 1)
        write_tag(1, 2, &mut layer);
        write_varint(self.name.len() as u64, &mut layer);
        layer.extend_from_slice(self.name.as_bytes());
        // features (field 2)
        for feature in &self.features {
            write_tag(2, 2, &mut layer);
            write_varint(feature.len() as u64, &mut layer);
            layer.extend_from_slice(feature);
        }
        // keys (field 3)
        for key in &self.keys {
            write_tag(3, 2, &mut layer);
            write_varint(key.len() as u64, &mut layer);
            layer.extend_from_slice(key.as_bytes());
        }
        // values (field 4)
        for value in &self.values {
            let mut vbuf = Vec::new();
            match value {
                AttrVal::Str(s) => {
                    write_tag(1, 2, &mut vbuf);
                    write_varint(s.len() as u64, &mut vbuf);
                    vbuf.extend_from_slice(s.as_bytes());
                }
                AttrVal::Float(f) => {
                    write_tag(3, 1, &mut vbuf);
                    vbuf.extend_from_slice(&f.to_le_bytes());
                }
                AttrVal::Int(i) => {
                    write_tag(4, 0, &mut vbuf);
                    write_varint(*i as u64, &mut vbuf);
                }
                AttrVal::Bool(b) => {
                    write_tag(7, 0, &mut vbuf);
                    write_varint(u64::from(*b), &mut vbuf);
                }
            }
            write_tag(4, 2, &mut layer);
            write_varint(vbuf.len() as u64, &mut layer);
            layer.extend_from_slice(&vbuf);
        }
        // extent (field 5)
        write_tag(5, 0, &mut layer);
        write_varint(u64::from(EXTENT), &mut layer);

        // Tile.layers is field 3
        write_tag(3, 2, out);
        write_varint(layer.len() as u64, out);
        out.extend_from_slice(&layer);
    }
}

/// One tile's worth of layers being assembled.
pub struct TileEnc {
    layers: Vec<LayerEnc>,
}

impl TileEnc {
    pub fn new() -> Self {
        TileEnc { layers: Vec::new() }
    }

    pub fn add_feature(&mut self, layer_name: &str, feature: &PreFeature) {
        let layer = match self.layers.iter_mut().find(|l| l.name == layer_name) {
            Some(l) => l,
            None => {
                self.layers.push(LayerEnc::new(layer_name));
                self.layers.last_mut().unwrap()
            }
        };
        layer.add_feature(feature);
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for layer in &self.layers {
            layer.encode(&mut out);
        }
        out
    }
}

pub fn zigzag(value: i64) -> u32 {
    ((value << 1) ^ (value >> 63)) as u32
}

pub fn command(id: u32, count: u32) -> u32 {
    (id & 0x7) | (count << 3)
}

fn write_tag(field: u32, wire_type: u32, out: &mut Vec<u8>) {
    write_varint(u64::from((field << 3) | wire_type), out);
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
