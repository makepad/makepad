use crate::{Splat, SplatError, SplatFileFormat, SplatScene};
use std::io::{BufRead, BufReader, Cursor, Read, Seek};

const SH_C0: f32 = 0.2820948;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlyFormat {
    Ascii,
    BinaryLittleEndian,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlyScalarType {
    Char,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Float,
    Double,
}

#[derive(Clone, Debug)]
struct PlyProperty {
    name: String,
    scalar_type: PlyScalarType,
}

#[derive(Clone, Debug)]
struct PlyHeader {
    format: PlyFormat,
    vertex_count: usize,
    properties: Vec<PlyProperty>,
    data_offset: usize,
}

pub fn load_ply_from_bytes(bytes: &[u8]) -> Result<SplatScene, SplatError> {
    let header = parse_header(bytes)?;

    let mut scene = SplatScene::empty(SplatFileFormat::Ply);
    scene.splats.reserve(header.vertex_count);

    match header.format {
        PlyFormat::Ascii => parse_ascii_vertices(bytes, &header, &mut scene.splats)?,
        PlyFormat::BinaryLittleEndian => parse_binary_vertices(bytes, &header, &mut scene.splats)?,
    }

    scene.recompute_bounds();
    Ok(scene)
}

fn parse_header(bytes: &[u8]) -> Result<PlyHeader, SplatError> {
    let mut reader = BufReader::new(Cursor::new(bytes));
    let mut line = String::new();

    reader.read_line(&mut line)?;
    if line.trim_end_matches(['\r', '\n']) != "ply" {
        return Err(SplatError::InvalidData(
            "missing PLY signature on first line".to_string(),
        ));
    }

    let mut format: Option<PlyFormat> = None;
    let mut vertex_count: Option<usize> = None;
    let mut properties = Vec::new();
    let mut in_vertex_element = false;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(SplatError::InvalidData(
                "unexpected EOF while reading PLY header".to_string(),
            ));
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "end_header" {
            break;
        }

        if trimmed.is_empty() || trimmed.starts_with("comment") {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "format" => {
                if parts.len() < 2 {
                    return Err(SplatError::InvalidData(
                        "format line missing encoding".to_string(),
                    ));
                }
                format = Some(match parts[1] {
                    "ascii" => PlyFormat::Ascii,
                    "binary_little_endian" => PlyFormat::BinaryLittleEndian,
                    other => {
                        return Err(SplatError::Unsupported(format!(
                            "unsupported PLY format '{other}'"
                        )))
                    }
                });
            }
            "element" => {
                if parts.len() < 3 {
                    return Err(SplatError::InvalidData(
                        "element line missing payload".to_string(),
                    ));
                }
                in_vertex_element = parts[1] == "vertex";
                if in_vertex_element {
                    vertex_count = Some(parts[2].parse::<usize>().map_err(|_| {
                        SplatError::InvalidData("invalid vertex count in header".to_string())
                    })?);
                    properties.clear();
                }
            }
            "property" if in_vertex_element => {
                if parts.len() < 3 {
                    return Err(SplatError::InvalidData(
                        "vertex property line missing payload".to_string(),
                    ));
                }
                if parts[1] == "list" {
                    return Err(SplatError::Unsupported(
                        "list vertex properties are not supported for splat PLY".to_string(),
                    ));
                }
                let scalar_type = parse_scalar_type(parts[1])?;
                properties.push(PlyProperty {
                    name: parts[2].to_string(),
                    scalar_type,
                });
            }
            _ => {}
        }
    }

    let format = format.ok_or_else(|| SplatError::MissingField("format".to_string()))?;
    let vertex_count =
        vertex_count.ok_or_else(|| SplatError::MissingField("element vertex".to_string()))?;

    let data_offset = reader.stream_position()? as usize;

    Ok(PlyHeader {
        format,
        vertex_count,
        properties,
        data_offset,
    })
}

fn parse_scalar_type(value: &str) -> Result<PlyScalarType, SplatError> {
    match value {
        "char" | "int8" => Ok(PlyScalarType::Char),
        "uchar" | "uint8" => Ok(PlyScalarType::UChar),
        "short" | "int16" => Ok(PlyScalarType::Short),
        "ushort" | "uint16" => Ok(PlyScalarType::UShort),
        "int" | "int32" => Ok(PlyScalarType::Int),
        "uint" | "uint32" => Ok(PlyScalarType::UInt),
        "float" | "float32" => Ok(PlyScalarType::Float),
        "double" | "float64" => Ok(PlyScalarType::Double),
        other => Err(SplatError::Unsupported(format!(
            "unsupported PLY scalar type '{other}'"
        ))),
    }
}

fn parse_ascii_vertices(
    bytes: &[u8],
    header: &PlyHeader,
    out: &mut Vec<Splat>,
) -> Result<(), SplatError> {
    let mut reader = BufReader::new(Cursor::new(&bytes[header.data_offset..]));
    let mut line = String::new();

    let indices = PropertyIndices::new(&header.properties);

    for _ in 0..header.vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(SplatError::InvalidData(
                "unexpected EOF while reading ascii vertex payload".to_string(),
            ));
        }
        if line.trim().is_empty() {
            continue;
        }

        let mut values = Vec::with_capacity(header.properties.len());
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < header.properties.len() {
            return Err(SplatError::InvalidData(
                "ascii PLY vertex has fewer properties than header declares".to_string(),
            ));
        }

        for (property, token) in header.properties.iter().zip(tokens.iter()) {
            values.push(parse_ascii_scalar(*token, property.scalar_type)?);
        }

        out.push(build_splat_from_values(&values, &indices));
    }

    Ok(())
}

fn parse_binary_vertices(
    bytes: &[u8],
    header: &PlyHeader,
    out: &mut Vec<Splat>,
) -> Result<(), SplatError> {
    let mut cursor = Cursor::new(&bytes[header.data_offset..]);
    let indices = PropertyIndices::new(&header.properties);

    for _ in 0..header.vertex_count {
        let mut values = Vec::with_capacity(header.properties.len());
        for property in &header.properties {
            values.push(read_binary_scalar(&mut cursor, property.scalar_type)?);
        }
        out.push(build_splat_from_values(&values, &indices));
    }

    Ok(())
}

fn parse_ascii_scalar(token: &str, scalar_type: PlyScalarType) -> Result<f32, SplatError> {
    match scalar_type {
        PlyScalarType::Char => token
            .parse::<i8>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int8 ascii value".to_string())),
        PlyScalarType::UChar => token
            .parse::<u8>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint8 ascii value".to_string())),
        PlyScalarType::Short => token
            .parse::<i16>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int16 ascii value".to_string())),
        PlyScalarType::UShort => token
            .parse::<u16>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint16 ascii value".to_string())),
        PlyScalarType::Int => token
            .parse::<i32>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid int32 ascii value".to_string())),
        PlyScalarType::UInt => token
            .parse::<u32>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid uint32 ascii value".to_string())),
        PlyScalarType::Float => token
            .parse::<f32>()
            .map_err(|_| SplatError::InvalidData("invalid float ascii value".to_string())),
        PlyScalarType::Double => token
            .parse::<f64>()
            .map(|v| v as f32)
            .map_err(|_| SplatError::InvalidData("invalid double ascii value".to_string())),
    }
}

fn read_binary_scalar(
    reader: &mut impl Read,
    scalar_type: PlyScalarType,
) -> Result<f32, SplatError> {
    macro_rules! read_t {
        ($t:ty) => {{
            let mut bytes = [0u8; std::mem::size_of::<$t>()];
            reader.read_exact(&mut bytes)?;
            <$t>::from_le_bytes(bytes) as f32
        }};
    }

    let value = match scalar_type {
        PlyScalarType::Char => read_t!(i8),
        PlyScalarType::UChar => read_t!(u8),
        PlyScalarType::Short => read_t!(i16),
        PlyScalarType::UShort => read_t!(u16),
        PlyScalarType::Int => read_t!(i32),
        PlyScalarType::UInt => read_t!(u32),
        PlyScalarType::Float => {
            let mut bytes = [0u8; 4];
            reader.read_exact(&mut bytes)?;
            f32::from_le_bytes(bytes)
        }
        PlyScalarType::Double => {
            let mut bytes = [0u8; 8];
            reader.read_exact(&mut bytes)?;
            f64::from_le_bytes(bytes) as f32
        }
    };
    Ok(value)
}

#[derive(Clone, Debug)]
struct PropertyIndices {
    x: Option<usize>,
    y: Option<usize>,
    z: Option<usize>,
    dc0: Option<usize>,
    dc1: Option<usize>,
    dc2: Option<usize>,
    opacity: Option<usize>,
    scale0: Option<usize>,
    scale1: Option<usize>,
    scale2: Option<usize>,
    rot0: Option<usize>,
    rot1: Option<usize>,
    rot2: Option<usize>,
    rot3: Option<usize>,
}

impl PropertyIndices {
    fn new(properties: &[PlyProperty]) -> Self {
        let mut out = Self {
            x: None,
            y: None,
            z: None,
            dc0: None,
            dc1: None,
            dc2: None,
            opacity: None,
            scale0: None,
            scale1: None,
            scale2: None,
            rot0: None,
            rot1: None,
            rot2: None,
            rot3: None,
        };

        for (index, property) in properties.iter().enumerate() {
            match property.name.as_str() {
                "x" => out.x = Some(index),
                "y" => out.y = Some(index),
                "z" => out.z = Some(index),
                "f_dc_0" => out.dc0 = Some(index),
                "f_dc_1" => out.dc1 = Some(index),
                "f_dc_2" => out.dc2 = Some(index),
                "opacity" => out.opacity = Some(index),
                "scale_0" => out.scale0 = Some(index),
                "scale_1" => out.scale1 = Some(index),
                "scale_2" => out.scale2 = Some(index),
                "rot_0" => out.rot0 = Some(index),
                "rot_1" => out.rot1 = Some(index),
                "rot_2" => out.rot2 = Some(index),
                "rot_3" => out.rot3 = Some(index),
                _ => {}
            }
        }

        out
    }
}

fn build_splat_from_values(values: &[f32], idx: &PropertyIndices) -> Splat {
    let x = idx.x.and_then(|i| values.get(i)).copied().unwrap_or(0.0);
    let y = idx.y.and_then(|i| values.get(i)).copied().unwrap_or(0.0);
    let z = idx.z.and_then(|i| values.get(i)).copied().unwrap_or(0.0);

    let dc0 = idx.dc0.and_then(|i| values.get(i)).copied().unwrap_or(0.0);
    let dc1 = idx.dc1.and_then(|i| values.get(i)).copied().unwrap_or(0.0);
    let dc2 = idx.dc2.and_then(|i| values.get(i)).copied().unwrap_or(0.0);

    let opacity = idx
        .opacity
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(1.0);

    let s0 = idx
        .scale0
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(-7.0)
        .exp();
    let s1 = idx
        .scale1
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(-7.0)
        .exp();
    let s2 = idx
        .scale2
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(-7.0)
        .exp();

    let (rotation, _had_rot) = {
        let r0 = idx.rot0.and_then(|i| values.get(i)).copied();
        let r1 = idx.rot1.and_then(|i| values.get(i)).copied();
        let r2 = idx.rot2.and_then(|i| values.get(i)).copied();
        let r3 = idx.rot3.and_then(|i| values.get(i)).copied();
        if let (Some(w), Some(x), Some(y), Some(z)) = (r0, r1, r2, r3) {
            (normalize_quaternion([x, y, z, w]), true)
        } else {
            ([0.0, 0.0, 0.0, 1.0], false)
        }
    };

    let color = [
        (0.5 + SH_C0 * dc0).clamp(0.0, 1.0),
        (0.5 + SH_C0 * dc1).clamp(0.0, 1.0),
        (0.5 + SH_C0 * dc2).clamp(0.0, 1.0),
        sigmoid(opacity).clamp(0.0, 1.0),
    ];

    Splat {
        position: [x, y, z],
        scale: [s0, s1, s2],
        rotation,
        color,
    }
}

fn normalize_quaternion(q: [f32; 4]) -> [f32; 4] {
    let len2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len2 <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv_len = len2.sqrt().recip();
    [
        q[0] * inv_len,
        q[1] * inv_len,
        q[2] * inv_len,
        q[3] * inv_len,
    ]
}

fn sigmoid(v: f32) -> f32 {
    if v >= 0.0 {
        1.0 / (1.0 + (-v).exp())
    } else {
        let exp_v = v.exp();
        exp_v / (1.0 + exp_v)
    }
}
