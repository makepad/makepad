//! Small, dependency-free ONNX protobuf reader.
//!
//! This intentionally implements only the protobuf messages needed by native
//! model loaders: model/graph metadata, nodes and their attributes, and tensor
//! initializers. Unknown fields are skipped, so newer ONNX producer versions
//! remain readable without pulling a protobuf runtime into every model crate.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct OnnxModel {
    pub producer_name: String,
    pub producer_version: String,
    pub graph: OnnxGraph,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnnxGraph {
    pub name: String,
    pub nodes: Vec<OnnxNode>,
    pub initializers: BTreeMap<String, OnnxTensor>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnnxNode {
    pub name: String,
    pub op_type: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attributes: BTreeMap<String, OnnxAttribute>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OnnxAttribute {
    Float(f32),
    Int(i64),
    String(Vec<u8>),
    Tensor(OnnxTensor),
    Floats(Vec<f32>),
    Ints(Vec<i64>),
    Strings(Vec<Vec<u8>>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnnxTensor {
    pub name: String,
    pub dims: Vec<i64>,
    /// ONNX TensorProto.DataType numeric value (FLOAT is 1, INT64 is 7).
    pub data_type: i32,
    pub raw_data: Vec<u8>,
    pub float_data: Vec<f32>,
    pub int32_data: Vec<i32>,
    pub int64_data: Vec<i64>,
    pub double_data: Vec<f64>,
    pub uint64_data: Vec<u64>,
}

impl OnnxTensor {
    pub fn element_count(&self) -> Result<usize, String> {
        self.dims.iter().try_fold(1usize, |n, &d| {
            let d = usize::try_from(d).map_err(|_| {
                format!("ONNX tensor '{}' has negative dimension {d}", self.name)
            })?;
            n.checked_mul(d).ok_or_else(|| {
                format!("ONNX tensor '{}' element count overflows usize", self.name)
            })
        })
    }

    pub fn f32_values(&self) -> Result<Vec<f32>, String> {
        if self.data_type != 1 {
            return Err(format!(
                "ONNX tensor '{}' has data_type {}, expected FLOAT (1)",
                self.name, self.data_type
            ));
        }
        let values = if !self.raw_data.is_empty() {
            if self.raw_data.len() % 4 != 0 {
                return Err(format!(
                    "ONNX tensor '{}' has {} raw bytes, not a multiple of four",
                    self.name,
                    self.raw_data.len()
                ));
            }
            self.raw_data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        } else {
            self.float_data.clone()
        };
        let expected = self.element_count()?;
        if values.len() != expected {
            return Err(format!(
                "ONNX tensor '{}' has {} FLOAT values, shape {:?} requires {expected}",
                self.name,
                values.len(),
                self.dims
            ));
        }
        Ok(values)
    }

    pub fn i64_values(&self) -> Result<Vec<i64>, String> {
        if self.data_type != 7 {
            return Err(format!(
                "ONNX tensor '{}' has data_type {}, expected INT64 (7)",
                self.name, self.data_type
            ));
        }
        let values = if !self.raw_data.is_empty() {
            if self.raw_data.len() % 8 != 0 {
                return Err(format!(
                    "ONNX tensor '{}' has {} raw bytes, not a multiple of eight",
                    self.name,
                    self.raw_data.len()
                ));
            }
            self.raw_data
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes(c.try_into().expect("eight-byte chunk")))
                .collect()
        } else {
            self.int64_data.clone()
        };
        let expected = self.element_count()?;
        if values.len() != expected {
            return Err(format!(
                "ONNX tensor '{}' has {} INT64 values, shape {:?} requires {expected}",
                self.name,
                values.len(),
                self.dims
            ));
        }
        Ok(values)
    }
}

impl OnnxModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .map_err(|e| format!("read ONNX model {}: {e}", path.display()))?;
        Self::parse(&bytes).map_err(|e| format!("parse ONNX model {}: {e}", path.display()))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut pb = Pb::new(bytes);
        let mut producer_name = String::new();
        let mut producer_version = String::new();
        let mut graph = None;
        while let Some((field, wire)) = pb.key()? {
            match (field, wire) {
                (2, 2) => producer_name = text(pb.bytes()?)?,
                (3, 2) => producer_version = text(pb.bytes()?)?,
                (7, 2) => graph = Some(parse_graph(pb.bytes()?)?),
                _ => pb.skip(wire)?,
            }
        }
        Ok(Self {
            producer_name,
            producer_version,
            graph: graph.ok_or_else(|| "ModelProto has no graph".to_string())?,
        })
    }
}

fn parse_graph(bytes: &[u8]) -> Result<OnnxGraph, String> {
    let mut pb = Pb::new(bytes);
    let mut graph = OnnxGraph::default();
    while let Some((field, wire)) = pb.key()? {
        match (field, wire) {
            (1, 2) => graph.nodes.push(parse_node(pb.bytes()?)?),
            (2, 2) => graph.name = text(pb.bytes()?)?,
            (5, 2) => {
                let tensor = parse_tensor(pb.bytes()?)?;
                if graph.initializers.insert(tensor.name.clone(), tensor).is_some() {
                    return Err("GraphProto contains duplicate initializer name".to_string());
                }
            }
            (11, 2) => graph.inputs.push(parse_value_info_name(pb.bytes()?)?),
            (12, 2) => graph.outputs.push(parse_value_info_name(pb.bytes()?)?),
            _ => pb.skip(wire)?,
        }
    }
    Ok(graph)
}

fn parse_value_info_name(bytes: &[u8]) -> Result<String, String> {
    let mut pb = Pb::new(bytes);
    while let Some((field, wire)) = pb.key()? {
        if (field, wire) == (1, 2) {
            return text(pb.bytes()?);
        }
        pb.skip(wire)?;
    }
    Ok(String::new())
}

fn parse_node(bytes: &[u8]) -> Result<OnnxNode, String> {
    let mut pb = Pb::new(bytes);
    let mut node = OnnxNode::default();
    while let Some((field, wire)) = pb.key()? {
        match (field, wire) {
            (1, 2) => node.inputs.push(text(pb.bytes()?)?),
            (2, 2) => node.outputs.push(text(pb.bytes()?)?),
            (3, 2) => node.name = text(pb.bytes()?)?,
            (4, 2) => node.op_type = text(pb.bytes()?)?,
            (5, 2) => {
                let (name, value) = parse_attribute(pb.bytes()?)?;
                if let Some(value) = value {
                    node.attributes.insert(name, value);
                }
            }
            _ => pb.skip(wire)?,
        }
    }
    Ok(node)
}

fn parse_attribute(bytes: &[u8]) -> Result<(String, Option<OnnxAttribute>), String> {
    let mut pb = Pb::new(bytes);
    let mut name = String::new();
    let mut value = None;
    let mut floats = Vec::new();
    let mut ints = Vec::new();
    let mut strings = Vec::new();
    while let Some((field, wire)) = pb.key()? {
        match (field, wire) {
            (1, 2) => name = text(pb.bytes()?)?,
            (2, 5) => value = Some(OnnxAttribute::Float(pb.fixed32_f32()?)),
            (3, 0) => value = Some(OnnxAttribute::Int(pb.varint()? as i64)),
            (4, 2) => value = Some(OnnxAttribute::String(pb.bytes()?.to_vec())),
            (5, 2) => value = Some(OnnxAttribute::Tensor(parse_tensor(pb.bytes()?)?)),
            (7, 5) => floats.push(pb.fixed32_f32()?),
            (7, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    floats.push(packed.fixed32_f32()?);
                }
            }
            (8, 0) => ints.push(pb.varint()? as i64),
            (8, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    ints.push(packed.varint()? as i64);
                }
            }
            (9, 2) => strings.push(pb.bytes()?.to_vec()),
            _ => pb.skip(wire)?,
        }
    }
    if value.is_none() {
        value = if !floats.is_empty() {
            Some(OnnxAttribute::Floats(floats))
        } else if !ints.is_empty() {
            Some(OnnxAttribute::Ints(ints))
        } else if !strings.is_empty() {
            Some(OnnxAttribute::Strings(strings))
        } else {
            None
        };
    }
    Ok((name, value))
}

fn parse_tensor(bytes: &[u8]) -> Result<OnnxTensor, String> {
    let mut pb = Pb::new(bytes);
    let mut tensor = OnnxTensor::default();
    while let Some((field, wire)) = pb.key()? {
        match (field, wire) {
            (1, 0) => tensor.dims.push(pb.varint()? as i64),
            (1, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.dims.push(packed.varint()? as i64);
                }
            }
            (2, 0) => tensor.data_type = pb.varint()? as i32,
            (4, 5) => tensor.float_data.push(pb.fixed32_f32()?),
            (4, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.float_data.push(packed.fixed32_f32()?);
                }
            }
            (5, 0) => tensor.int32_data.push(pb.varint()? as i32),
            (5, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.int32_data.push(packed.varint()? as i32);
                }
            }
            (7, 0) => tensor.int64_data.push(pb.varint()? as i64),
            (7, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.int64_data.push(packed.varint()? as i64);
                }
            }
            (8, 2) => tensor.name = text(pb.bytes()?)?,
            (9, 2) => tensor.raw_data = pb.bytes()?.to_vec(),
            (10, 1) => tensor.double_data.push(pb.fixed64_f64()?),
            (10, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.double_data.push(packed.fixed64_f64()?);
                }
            }
            (11, 0) => tensor.uint64_data.push(pb.varint()?),
            (11, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    tensor.uint64_data.push(packed.varint()?);
                }
            }
            _ => pb.skip(wire)?,
        }
    }
    Ok(tensor)
}

fn text(bytes: &[u8]) -> Result<String, String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|e| format!("invalid UTF-8 in ONNX protobuf: {e}"))
}

struct Pb<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Pb<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or_else(|| "truncated protobuf varint".to_string())?;
            self.pos += 1;
            if shift < 64 {
                value |= u64::from(byte & 0x7f) << shift;
            } else if byte & 0x7e != 0 {
                return Err("protobuf varint overflows u64".to_string());
            }
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("protobuf varint exceeds ten bytes".to_string())
    }

    fn key(&mut self) -> Result<Option<(u64, u8)>, String> {
        if self.done() {
            return Ok(None);
        }
        let key = self.varint()?;
        if key == 0 {
            return Err("protobuf field key is zero".to_string());
        }
        Ok(Some((key >> 3, (key & 7) as u8)))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = usize::try_from(self.varint()?)
            .map_err(|_| "protobuf length does not fit usize".to_string())?;
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "protobuf length overflows usize".to_string())?;
        if end > self.bytes.len() {
            return Err("truncated length-delimited protobuf field".to_string());
        }
        let value = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(value)
    }

    fn fixed32_f32(&mut self) -> Result<f32, String> {
        let bytes: [u8; 4] = self.take_fixed::<4>()?.try_into().expect("fixed width");
        Ok(f32::from_le_bytes(bytes))
    }

    fn fixed64_f64(&mut self) -> Result<f64, String> {
        let bytes: [u8; 8] = self.take_fixed::<8>()?.try_into().expect("fixed width");
        Ok(f64::from_le_bytes(bytes))
    }

    fn take_fixed<const N: usize>(&mut self) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(N)
            .ok_or_else(|| "protobuf fixed field overflows usize".to_string())?;
        if end > self.bytes.len() {
            return Err("truncated fixed-width protobuf field".to_string());
        }
        let value = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(value)
    }

    fn skip(&mut self, wire: u8) -> Result<(), String> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.take_fixed::<8>()?;
            }
            2 => {
                self.bytes()?;
            }
            5 => {
                self.take_fixed::<4>()?;
            }
            _ => return Err(format!("unsupported protobuf wire type {wire}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_messages() {
        assert!(OnnxModel::parse(&[0x3a, 0x04, 0x08]).is_err());
    }
}
