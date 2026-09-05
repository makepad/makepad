//! Layout proof for compact vertex formats. No GPU.

use crate::makepad_platform::*;
use super::geometry_gen::{FillVertexTyped, RoadVertexTyped, RoofVertexTyped, VectorVertexPacked};
use std::mem::{align_of, offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy)]
struct TypedQuadVertex {
    pos: UNorm16x2,
    color: UNorm8x4,
}

#[repr(C)]
#[derive(Clone, Script, ScriptHook)]
struct ScriptTypedVertex {
    #[live]
    pos: F16x2,
    #[live]
    color: UNorm8x4,
}

#[test]
fn compact_fields_implement_script_traits() {
    fn assert_script_field<T: ScriptNew + ScriptApply>() {}
    assert_script_field::<F16x2>();
    assert_script_field::<F16x4>();
    assert_script_field::<U16x2>();
    assert_script_field::<I16x2>();
    assert_script_field::<UNorm16x2>();
    assert_script_field::<SNorm16x2>();
    assert_script_field::<UNorm8x4>();
    assert_script_field::<SNorm8x4>();
    assert_script_field::<ScriptTypedVertex>();
    assert_script_field::<FillVertexTyped>();
    assert_script_field::<RoadVertexTyped>();
    assert_script_field::<RoofVertexTyped>();
}

#[test]
fn typed_quad_pod_layout_matches_gpu_stride() {
    assert_eq!(size_of::<TypedQuadVertex>(), 8);
    assert_eq!(align_of::<TypedQuadVertex>(), 2);
    assert_eq!(offset_of!(TypedQuadVertex, pos), 0);
    assert_eq!(offset_of!(TypedQuadVertex, color), 4);

    let pos = UNorm16x2::from_f32(0.0, 1.0);
    let color = UNorm8x4::from_f32(1.0, 0.0, 0.0, 1.0);
    let v = TypedQuadVertex { pos, color };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            &v as *const TypedQuadVertex as *const u8,
            size_of::<TypedQuadVertex>(),
        )
    };
    assert_eq!(&bytes[0..2], &0u16.to_le_bytes());
    assert_eq!(&bytes[2..4], &65535u16.to_le_bytes());
    assert_eq!(bytes[4], 255);
    assert_eq!(bytes[5], 0);
    assert_eq!(bytes[6], 0);
    assert_eq!(bytes[7], 255);
}

#[test]
fn f32_quad_and_typed_quad_geometries_stage() {
    let f32_vertices = vec![
        0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0,
    ];
    let f32_upload_bytes = f32_vertices
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect::<Vec<_>>();
    let f32_indices: Vec<u32> = vec![0, 1, 2, 2, 3, 0];

    let mut typed_vertices = Vec::new();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let v = TypedQuadVertex {
            pos: UNorm16x2::from_f32(x, y),
            color: UNorm8x4::from_f32(1.0, 1.0, 1.0, 1.0),
        };
        typed_vertices.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                &v as *const TypedQuadVertex as *const u8,
                size_of::<TypedQuadVertex>(),
            )
        });
    }
    let typed_indices: Vec<u16> = vec![0, 1, 2, 2, 3, 0];

    assert_eq!(f32_vertices.len() * 4, 32);
    assert_eq!(typed_vertices.len(), 32);
    assert_eq!(typed_vertices.len() / size_of::<TypedQuadVertex>(), 4);
    assert_eq!(f32_indices.len(), 6);
    assert_eq!(typed_indices.len(), 6);

    let f32_geom = CxGeometry {
        indices: IndexData::U32(f32_indices),
        vertices: VertexData::F32(f32_vertices),
        vertex_stride: 8,
        index_count: 6,
        vertex_count: 4,
        ..Default::default()
    };
    let typed_geom = CxGeometry {
        indices: IndexData::U16(typed_indices),
        vertices: VertexData::Bytes(typed_vertices),
        vertex_stride: size_of::<TypedQuadVertex>(),
        index_count: 6,
        vertex_count: 4,
        ..Default::default()
    };
    assert_eq!(f32_geom.vertex_stride, 8);
    assert_eq!(typed_geom.vertex_stride, 8);
    assert!(f32_geom.vertices.is_f32());
    assert_eq!(f32_geom.vertices.as_bytes(), f32_upload_bytes);
    assert!(!f32_geom.indices.is_u16());
    assert!(!typed_geom.vertices.is_f32());
    assert!(typed_geom.indices.is_u16());
    assert_eq!(typed_geom.vertices.byte_len(), 32);
}

#[test]
fn attribute_packing_f32_stride_equals_slots_times_four() {
    let mut inputs = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    inputs.push(live_id!(pos), 2, DrawShaderAttrFormat::F32x2);
    inputs.push(live_id!(uv), 2, DrawShaderAttrFormat::F32x2);
    inputs.finalize();
    assert_eq!(inputs.total_slots, 4);
    assert_eq!(inputs.stride_bytes, 16);
    assert_eq!(inputs.inputs[0].byte_offset, 0);
    assert_eq!(inputs.inputs[1].byte_offset, 8);
}

#[test]
fn attribute_packing_compact_unorm_quad() {
    let mut inputs = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    inputs.push(live_id!(pos), 2, DrawShaderAttrFormat::U16x2Norm);
    inputs.push(live_id!(color), 4, DrawShaderAttrFormat::U8x4Norm);
    inputs.finalize();
    assert_eq!(inputs.inputs[0].byte_offset, 0);
    assert_eq!(inputs.inputs[0].byte_size, 4);
    assert_eq!(inputs.inputs[1].byte_offset, 4);
    assert_eq!(inputs.inputs[1].byte_size, 4);
    assert_eq!(inputs.stride_bytes, 8);
    assert!(inputs.has_compact());
}

#[test]
fn typed_map_vertex_layouts_match_repr_c_records() {
    let mut inputs = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    for (id, format) in [
        (live_id!(pos), DrawShaderAttrFormat::I16x2),
        (live_id!(off), DrawShaderAttrFormat::F16x2),
        (live_id!(color), DrawShaderAttrFormat::U8x4Norm),
        (live_id!(params), DrawShaderAttrFormat::F16x2),
        (live_id!(deck), DrawShaderAttrFormat::F32x1),
        (live_id!(depth), DrawShaderAttrFormat::F16x2),
        (live_id!(uv), DrawShaderAttrFormat::F16x2),
    ] {
        inputs.push(id, format.logical_slots(), format);
    }
    inputs.finalize();
    assert_eq!(
        inputs.inputs.iter().map(|input| input.byte_offset).collect::<Vec<_>>(),
        vec![0, 4, 8, 12, 16, 20, 24]
    );
    assert_eq!(inputs.stride_bytes, 28);
    assert_eq!(size_of::<RoadVertexTyped>(), 28);
    assert_eq!(align_of::<RoadVertexTyped>(), 4);
    assert_eq!(offset_of!(RoadVertexTyped, pos), 0);
    assert_eq!(offset_of!(RoadVertexTyped, off), 4);
    assert_eq!(offset_of!(RoadVertexTyped, color), 8);
    assert_eq!(offset_of!(RoadVertexTyped, params), 12);
    assert_eq!(offset_of!(RoadVertexTyped, deck), 16);
    assert_eq!(offset_of!(RoadVertexTyped, depth), 20);
    assert_eq!(offset_of!(RoadVertexTyped, uv), 24);

    assert_eq!(size_of::<FillVertexTyped>(), 16);
    assert_eq!(offset_of!(FillVertexTyped, pos), 0);
    assert_eq!(offset_of!(FillVertexTyped, color), 4);
    assert_eq!(offset_of!(FillVertexTyped, params), 8);
    assert_eq!(offset_of!(FillVertexTyped, zbias), 12);

    assert_eq!(size_of::<RoofVertexTyped>(), 16);
    assert_eq!(offset_of!(RoofVertexTyped, pos), 0);
    assert_eq!(offset_of!(RoofVertexTyped, color), 4);
    assert_eq!(offset_of!(RoofVertexTyped, height), 8);
    assert_eq!(offset_of!(RoofVertexTyped, params), 12);
}

#[test]
fn compact_then_f32_does_not_use_logical_slot_stride() {
    let mut inputs = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    inputs.push(live_id!(packed), 2, DrawShaderAttrFormat::U16x2Norm);
    inputs.push(live_id!(value), 1, DrawShaderAttrFormat::F32x1);
    inputs.finalize();
    assert_eq!(inputs.inputs[0].byte_offset, 0);
    assert_eq!(inputs.inputs[1].byte_offset, 4);
    assert_eq!(inputs.stride_bytes, 8);
}

#[test]
fn legacy_vector_vertex_packed_layout_stays_twelve_f32_lanes() {
    let mut inputs = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    for id in [
        live_id!(x),
        live_id!(y),
        live_id!(uv),
        live_id!(color),
        live_id!(stroke_mult),
        live_id!(stroke_dist),
        live_id!(p0s),
        live_id!(p12),
        live_id!(p3c),
        live_id!(param4),
        live_id!(param5),
        live_id!(zbias),
    ] {
        inputs.push(id, 1, DrawShaderAttrFormat::F32x1);
    }
    inputs.finalize();
    assert_eq!(size_of::<VectorVertexPacked>(), 48);
    assert_eq!(
        [
            offset_of!(VectorVertexPacked, x),
            offset_of!(VectorVertexPacked, y),
            offset_of!(VectorVertexPacked, uv),
            offset_of!(VectorVertexPacked, color),
            offset_of!(VectorVertexPacked, stroke_mult),
            offset_of!(VectorVertexPacked, stroke_dist),
            offset_of!(VectorVertexPacked, p0s),
            offset_of!(VectorVertexPacked, p12),
            offset_of!(VectorVertexPacked, p3c),
            offset_of!(VectorVertexPacked, param4),
            offset_of!(VectorVertexPacked, param5),
            offset_of!(VectorVertexPacked, zbias),
        ],
        [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44]
    );
    assert_eq!(inputs.total_slots, 12);
    assert_eq!(inputs.stride_bytes, 48);
    assert_eq!(
        inputs.inputs.iter().map(|input| input.byte_offset).collect::<Vec<_>>(),
        (0..12).map(|index| index * 4).collect::<Vec<_>>()
    );
    assert!(inputs.all_f32_lanes());
}

#[test]
fn signed_normalized_minima_match_webgl() {
    assert_eq!(SNorm16x2 { x: i16::MIN, y: -1 }.to_f32().0, -1.0);
    assert_eq!(SNorm8x4([i8::MIN, -1, 0, 1]).to_f32().0, -1.0);
    assert_eq!(
        DrawShaderAttrFormat::I16x2Norm.decode_to_f32(&i16::MIN.to_le_bytes())[0],
        -1.0
    );
    assert_eq!(
        DrawShaderAttrFormat::I8x4Norm.decode_to_f32(&[i8::MIN as u8, 0, 0, 0])[0],
        -1.0
    );
}
