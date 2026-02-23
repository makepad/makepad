use crate::{
    document::{GltfAccessor, GltfDocument, GLTF_MODE_TRIANGLES},
    error::GltfError,
    loader::LoadedGltf,
};

pub const GLTF_COMPONENT_TYPE_BYTE: u32 = 5120;
pub const GLTF_COMPONENT_TYPE_UNSIGNED_BYTE: u32 = 5121;
pub const GLTF_COMPONENT_TYPE_SHORT: u32 = 5122;
pub const GLTF_COMPONENT_TYPE_UNSIGNED_SHORT: u32 = 5123;
pub const GLTF_COMPONENT_TYPE_UNSIGNED_INT: u32 = 5125;
pub const GLTF_COMPONENT_TYPE_FLOAT: u32 = 5126;

#[derive(Clone, Debug)]
pub struct DecodedPrimitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub texcoords0: Option<Vec<[f32; 2]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

pub fn decode_mesh_primitive(
    loaded: &LoadedGltf,
    mesh_index: usize,
    primitive_index: usize,
) -> Result<DecodedPrimitive, GltfError> {
    let meshes = loaded.document.meshes_slice();
    let mesh = meshes.get(mesh_index).ok_or_else(|| {
        GltfError::Validation(format!(
            "mesh index {mesh_index} is out of bounds (len {})",
            meshes.len()
        ))
    })?;
    let primitive = mesh.primitives.get(primitive_index).ok_or_else(|| {
        GltfError::Validation(format!(
            "primitive index {primitive_index} is out of bounds for mesh {mesh_index} (len {})",
            mesh.primitives.len()
        ))
    })?;
    if primitive.mode() != GLTF_MODE_TRIANGLES {
        return Err(GltfError::Unsupported(format!(
            "decode_mesh_primitive only supports TRIANGLES mode for now (mesh {mesh_index} primitive {primitive_index} mode {})",
            primitive.mode()
        )));
    }

    let position_accessor_index = primitive
        .attributes
        .get("POSITION")
        .copied()
        .ok_or_else(|| {
            GltfError::Validation(format!(
                "meshes[{mesh_index}].primitives[{primitive_index}] is missing required POSITION attribute"
            ))
        })?;
    let positions = read_accessor_f32x3(loaded, position_accessor_index)?;

    let normals = if let Some(normal_accessor_index) = primitive.attributes.get("NORMAL") {
        Some(read_accessor_f32x3(loaded, *normal_accessor_index)?)
    } else {
        None
    };
    let tangents = if let Some(tangent_accessor_index) = primitive.attributes.get("TANGENT") {
        Some(read_accessor_f32x4(loaded, *tangent_accessor_index)?)
    } else {
        None
    };

    let texcoords0 = if let Some(uv_accessor_index) = primitive.attributes.get("TEXCOORD_0") {
        Some(read_accessor_f32x2(loaded, *uv_accessor_index)?)
    } else {
        None
    };

    if let Some(normals) = &normals {
        if normals.len() != positions.len() {
            return Err(GltfError::Validation(format!(
                "NORMAL vertex count {} does not match POSITION count {} for mesh {mesh_index} primitive {primitive_index}",
                normals.len(),
                positions.len()
            )));
        }
    }
    if let Some(tangents) = &tangents {
        if tangents.len() != positions.len() {
            return Err(GltfError::Validation(format!(
                "TANGENT vertex count {} does not match POSITION count {} for mesh {mesh_index} primitive {primitive_index}",
                tangents.len(),
                positions.len()
            )));
        }
    }
    if let Some(texcoords0) = &texcoords0 {
        if texcoords0.len() != positions.len() {
            return Err(GltfError::Validation(format!(
                "TEXCOORD_0 vertex count {} does not match POSITION count {} for mesh {mesh_index} primitive {primitive_index}",
                texcoords0.len(),
                positions.len()
            )));
        }
    }

    let indices = if let Some(indices_accessor_index) = primitive.indices {
        read_accessor_indices_u32(loaded, indices_accessor_index)?
    } else {
        (0_u32..positions.len() as u32).collect()
    };

    Ok(DecodedPrimitive {
        positions,
        normals,
        tangents,
        texcoords0,
        indices,
        material: primitive.material,
    })
}

pub fn read_accessor_f32x2(
    loaded: &LoadedGltf,
    accessor_index: usize,
) -> Result<Vec<[f32; 2]>, GltfError> {
    let view = AccessorView::new(&loaded.document, &loaded.buffers, accessor_index)?;
    if view.accessor.component_type != GLTF_COMPONENT_TYPE_FLOAT {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x2 requires FLOAT(5126), got {}",
            view.accessor.component_type
        )));
    }
    if view.component_count != 2 {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x2 requires VEC2, got {}",
            view.accessor.accessor_type
        )));
    }

    let mut out = Vec::with_capacity(view.accessor.count);
    for i in 0..view.accessor.count {
        let item = view.item_bytes(i)?;
        out.push([read_f32_le(item, 0)?, read_f32_le(item, 4)?]);
    }
    Ok(out)
}

pub fn read_accessor_f32x3(
    loaded: &LoadedGltf,
    accessor_index: usize,
) -> Result<Vec<[f32; 3]>, GltfError> {
    let view = AccessorView::new(&loaded.document, &loaded.buffers, accessor_index)?;
    if view.accessor.component_type != GLTF_COMPONENT_TYPE_FLOAT {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x3 requires FLOAT(5126), got {}",
            view.accessor.component_type
        )));
    }
    if view.component_count != 3 {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x3 requires VEC3, got {}",
            view.accessor.accessor_type
        )));
    }

    let mut out = Vec::with_capacity(view.accessor.count);
    for i in 0..view.accessor.count {
        let item = view.item_bytes(i)?;
        out.push([
            read_f32_le(item, 0)?,
            read_f32_le(item, 4)?,
            read_f32_le(item, 8)?,
        ]);
    }
    Ok(out)
}

pub fn read_accessor_f32x4(
    loaded: &LoadedGltf,
    accessor_index: usize,
) -> Result<Vec<[f32; 4]>, GltfError> {
    let view = AccessorView::new(&loaded.document, &loaded.buffers, accessor_index)?;
    if view.accessor.component_type != GLTF_COMPONENT_TYPE_FLOAT {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x4 requires FLOAT(5126), got {}",
            view.accessor.component_type
        )));
    }
    if view.component_count != 4 {
        return Err(GltfError::Unsupported(format!(
            "accessor[{accessor_index}] f32x4 requires VEC4, got {}",
            view.accessor.accessor_type
        )));
    }

    let mut out = Vec::with_capacity(view.accessor.count);
    for i in 0..view.accessor.count {
        let item = view.item_bytes(i)?;
        out.push([
            read_f32_le(item, 0)?,
            read_f32_le(item, 4)?,
            read_f32_le(item, 8)?,
            read_f32_le(item, 12)?,
        ]);
    }
    Ok(out)
}

pub fn read_accessor_indices_u32(
    loaded: &LoadedGltf,
    accessor_index: usize,
) -> Result<Vec<u32>, GltfError> {
    let view = AccessorView::new(&loaded.document, &loaded.buffers, accessor_index)?;
    if view.component_count != 1 {
        return Err(GltfError::Unsupported(format!(
            "indices accessor[{accessor_index}] must be SCALAR, got {}",
            view.accessor.accessor_type
        )));
    }

    let mut out = Vec::with_capacity(view.accessor.count);
    match view.accessor.component_type {
        GLTF_COMPONENT_TYPE_UNSIGNED_BYTE => {
            for i in 0..view.accessor.count {
                let item = view.item_bytes(i)?;
                out.push(item[0] as u32);
            }
        }
        GLTF_COMPONENT_TYPE_UNSIGNED_SHORT => {
            for i in 0..view.accessor.count {
                let item = view.item_bytes(i)?;
                out.push(read_u16_le(item, 0)? as u32);
            }
        }
        GLTF_COMPONENT_TYPE_UNSIGNED_INT => {
            for i in 0..view.accessor.count {
                let item = view.item_bytes(i)?;
                out.push(read_u32_le(item, 0)?);
            }
        }
        other => {
            return Err(GltfError::Unsupported(format!(
                "indices accessor[{accessor_index}] component type {other} is unsupported"
            )));
        }
    }
    Ok(out)
}

struct AccessorView<'a> {
    accessor: &'a GltfAccessor,
    bytes: &'a [u8],
    base_offset: usize,
    stride: usize,
    element_size: usize,
    component_count: usize,
}

impl<'a> AccessorView<'a> {
    fn new(
        document: &'a GltfDocument,
        buffers: &'a [Vec<u8>],
        accessor_index: usize,
    ) -> Result<Self, GltfError> {
        let accessors = document.accessors_slice();
        let accessor = accessors.get(accessor_index).ok_or_else(|| {
            GltfError::Validation(format!(
                "accessor index {accessor_index} is out of bounds (len {})",
                accessors.len()
            ))
        })?;

        if accessor.sparse.is_some() {
            return Err(GltfError::Unsupported(format!(
                "sparse accessors are not yet supported (accessor[{accessor_index}])"
            )));
        }

        let buffer_view_index = accessor.buffer_view.ok_or_else(|| {
            GltfError::Unsupported(format!(
                "accessor[{accessor_index}] has no bufferView (sparse-only accessors are not supported yet)"
            ))
        })?;
        let buffer_views = document.buffer_views_slice();
        let buffer_view = buffer_views.get(buffer_view_index).ok_or_else(|| {
            GltfError::Validation(format!(
                "accessor[{accessor_index}] references bufferView {buffer_view_index} out of bounds (len {})",
                buffer_views.len()
            ))
        })?;

        let buffer = buffers
            .get(buffer_view.buffer)
            .ok_or(GltfError::MissingBuffer {
                index: buffer_view.buffer,
            })?;

        let component_size = component_type_size(accessor.component_type).ok_or_else(|| {
            GltfError::Unsupported(format!(
                "accessor[{accessor_index}] uses unsupported component type {}",
                accessor.component_type
            ))
        })?;
        let component_count =
            accessor_component_count(&accessor.accessor_type).ok_or_else(|| {
                GltfError::Unsupported(format!(
                    "accessor[{accessor_index}] uses unsupported accessor type {}",
                    accessor.accessor_type
                ))
            })?;

        let element_size = component_size * component_count;
        let stride = buffer_view.byte_stride.unwrap_or(element_size);
        if stride < element_size {
            return Err(GltfError::Validation(format!(
                "accessor[{accessor_index}] stride {stride} is smaller than element size {element_size}"
            )));
        }
        if stride > 255 {
            return Err(GltfError::Validation(format!(
                "accessor[{accessor_index}] stride {stride} exceeds glTF maximum of 255"
            )));
        }

        let base_offset = buffer_view.byte_offset.unwrap_or(0) + accessor.byte_offset.unwrap_or(0);
        let last_offset = if accessor.count == 0 {
            base_offset
        } else {
            base_offset + stride * (accessor.count - 1) + element_size
        };
        let view_end = buffer_view.byte_offset.unwrap_or(0) + buffer_view.byte_length;
        if last_offset > buffer.len() {
            return Err(GltfError::Validation(format!(
                "accessor[{accessor_index}] data overruns backing buffer (need {last_offset}, got {})",
                buffer.len()
            )));
        }
        if last_offset > view_end {
            return Err(GltfError::Validation(format!(
                "accessor[{accessor_index}] data overruns bufferView bounds (need {last_offset}, view ends at {view_end})"
            )));
        }

        Ok(Self {
            accessor,
            bytes: buffer,
            base_offset,
            stride,
            element_size,
            component_count,
        })
    }

    fn item_bytes(&self, index: usize) -> Result<&'a [u8], GltfError> {
        let offset = self.base_offset + index * self.stride;
        let end = offset + self.element_size;
        if end > self.bytes.len() {
            return Err(GltfError::Validation(format!(
                "item read out of bounds at index {index}: [{offset}..{end}) > {}",
                self.bytes.len()
            )));
        }
        Ok(&self.bytes[offset..end])
    }
}

fn accessor_component_count(accessor_type: &str) -> Option<usize> {
    match accessor_type {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" => Some(4),
        "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

fn component_type_size(component_type: u32) -> Option<usize> {
    match component_type {
        GLTF_COMPONENT_TYPE_BYTE | GLTF_COMPONENT_TYPE_UNSIGNED_BYTE => Some(1),
        GLTF_COMPONENT_TYPE_SHORT | GLTF_COMPONENT_TYPE_UNSIGNED_SHORT => Some(2),
        GLTF_COMPONENT_TYPE_UNSIGNED_INT | GLTF_COMPONENT_TYPE_FLOAT => Some(4),
        _ => None,
    }
}

fn read_f32_le(data: &[u8], offset: usize) -> Result<f32, GltfError> {
    if offset + 4 > data.len() {
        return Err(GltfError::Validation(format!(
            "f32 read out of bounds at offset {offset}"
        )));
    }
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(&data[offset..offset + 4]);
    Ok(f32::from_le_bytes(bytes))
}

fn read_u16_le(data: &[u8], offset: usize) -> Result<u16, GltfError> {
    if offset + 2 > data.len() {
        return Err(GltfError::Validation(format!(
            "u16 read out of bounds at offset {offset}"
        )));
    }
    let mut bytes = [0_u8; 2];
    bytes.copy_from_slice(&data[offset..offset + 2]);
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32, GltfError> {
    if offset + 4 > data.len() {
        return Err(GltfError::Validation(format!(
            "u32 read out of bounds at offset {offset}"
        )));
    }
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(&data[offset..offset + 4]);
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use crate::{
        buffer::{decode_mesh_primitive, read_accessor_indices_u32},
        loader::load_gltf_from_path,
    };
    use std::path::PathBuf;

    fn damaged_helmet_path(subpath: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../examples/gltf/resources/glTF-Sample-Models/2.0/DamagedHelmet/{subpath}"
        ))
    }

    #[test]
    fn decodes_damaged_helmet_primitive_if_available() {
        let path = damaged_helmet_path("glTF/DamagedHelmet.gltf");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(path).expect("must load");
        let primitive = decode_mesh_primitive(&loaded, 0, 0).expect("must decode");

        assert_eq!(primitive.positions.len(), 14556);
        assert_eq!(primitive.normals.as_ref().expect("normals").len(), 14556);
        assert_eq!(primitive.texcoords0.as_ref().expect("uvs").len(), 14556);
        assert_eq!(primitive.indices.len(), 46356);
    }

    #[test]
    fn decodes_damaged_helmet_glb_indices_if_available() {
        let path = damaged_helmet_path("glTF-Binary/DamagedHelmet.glb");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(path).expect("must load");
        // Index accessor is 0 for DamagedHelmet.
        let indices = read_accessor_indices_u32(&loaded, 0).expect("must decode indices");
        assert_eq!(indices.len(), 46356);
    }
}
