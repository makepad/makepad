use super::*;

fn triangle() -> StaticModel {
    let mut vertices = Vec::new();
    for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
        vertices.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, f32::from_bits(0xffff_ffff), 0.0]);
    }
    StaticModel {
        vertices, indices: vec![0, 1, 2], texture_uri: None, texture_png: None,
        min: vec3f(0.0, 0.0, 0.0), max: vec3f(1.0, 0.0, 1.0),
        parts: vec![(vec3f(0.0, 0.0, 0.0), vec3f(1.0, 0.0, 1.0))],
        ground_ao: None, draw_layers: Vec::new(), detail_png: None, detail_scale: [1.0, 1.0],
        prelit: false, anim_parts: Vec::new(), driven_parts: Vec::new(), sky: None, liquids: Vec::new(), liquid_ranges: Vec::new(),
        pbr: Default::default(),
    }
}

#[test]
fn texture_book_separates_shared_lod_atlas_from_geometry() {
    let png = Cx::encode_rgba_as_png(2, 2, &[180; 16]).unwrap();
    let prepare = || {
        let mut model = triangle();
        model.texture_png = Some(png.clone());
        let mut prepared = PreparedStaticPreview::prepare(model).unwrap();
        // Distinct allocations prove that content keys, rather than Arc identity, deduplicate levels.
        prepared.main.texture = std::sync::Arc::new((*prepared.main.texture).clone());
        prepared
    };
    let mut prepared = prepare();
    let atlas = prepared.main.texture.clone();
    let lods = vec![(10.0, prepare()), (20.0, prepare())];
    prepared = prepared.with_lods(lods).unwrap();
    // A second binding of this key must retain the first slot in the census.
    prepared.main.detail = atlas.clone();
    let geometry = (3 * crate::model::MODEL_VERTEX_FLOATS + 3) * 4;
    assert_eq!(prepared.level_geometry_bytes(), vec![geometry; 3], "LOD geometry contains no texture bytes");
    let charges = prepared.texture_charges();
    let atlas_rows: Vec<_> = charges.iter().filter(|t| t.key == atlas.hash).collect();
    assert_eq!(atlas_rows.len(), 1, "all three levels charge the atlas once");
    assert_eq!((atlas_rows[0].slot, atlas_rows[0].width, atlas_rows[0].height, atlas_rows[0].layers, atlas_rows[0].mips, atlas_rows[0].bytes),
        (TextureSlot::Base, 2, 2, 1, 2, 20), "the first binding and whole mip chain are reported");
    let mut classification = (*atlas_rows[0]).clone();
    classification.width = crate::material_surface::COMPRESS_MIN_TEXELS;
    classification.height = 1;
    assert!(classification.rgba_compressible(), "the upload threshold includes its boundary");
    classification.width -= 1;
    assert!(!classification.rgba_compressible(), "smaller images stay out of compressible RGBA");
    classification.width += 1;
    classification.compressed = Some(crate::material_surface::GpuCompressedFormat::Bc7);
    assert!(!classification.rgba_compressible(), "compressed bytes cannot also be compressible RGBA");
    let keys = prepared.level_texture_keys();
    assert_eq!(keys.len(), 3, "one key list per distance level");
    for level in keys {
        assert_eq!(level.iter().filter(|&&key| key == atlas.hash).count(), 1, "each level binds the atlas exactly once");
    }
    assert_eq!(prepared.upload_bytes(), 3 * geometry + 20 + 4 + 4, "atlas and two default maps each upload once");
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    let _uploaded = renderer.upload_static_preview(&mut cx, prepared);
    assert_eq!(renderer.cached_texture_keys().collect::<std::collections::HashSet<_>>(),
        charges.iter().map(|t| t.key).collect(), "charge keys match upload cache keys");
}

#[test]
fn splat_charges_follow_array_order_and_upload_format() {
    use crate::material_surface::{PreparedSurface, SplatLayer, SplatSurface, splat_array_key};
    let layer = |color| SplatLayer {
        albedo_png: None, color, normal_png: None, scale: 1.0, roughness: 0.5,
        metallic: 0.0, bump: 0.0, relief: 0.0, wet: 0.0, puddles: 0.0,
        macro_variation: [0.0; 4], stochastic: false,
    };
    let mut surface = PreparedSurface::prepare(crate::material_surface::MaterialSurface {
        splat: Some(std::sync::Arc::new(SplatSurface {
            layers: vec![layer([1.0, 0.0, 0.0]), layer([0.0, 1.0, 0.0])],
            weights_png: Cx::encode_rgba_as_png(1, 1, &[255, 0, 0, 0]).unwrap(),
            origin: [0.0; 2], cell: 1.0, width: 1.0, rows: 1.0, band: None,
        })), ..Default::default()
    }, &mut 4096).unwrap();
    // Compressed layers need no transcoding here: the test checks the array upload contract.
    let splat = surface.splat.as_mut().unwrap();
    for layer in &mut splat.albedo {
        let layer = std::sync::Arc::make_mut(layer);
        layer.data.clear();
        layer.compressed = Some((crate::material_surface::GpuCompressedFormat::Bc7, vec![0; 16]));
    }
    let first_key = splat_array_key(&splat.albedo);
    let mut reversed = surface.clone();
    reversed.splat.as_mut().unwrap().albedo.reverse();
    let second_key = splat_array_key(&reversed.splat.as_ref().unwrap().albedo);
    assert_ne!(first_key, second_key, "layer order identifies a distinct array");
    let mut prepared = PreparedStaticPreview::prepare(triangle()).unwrap();
    prepared.main.surface = Some(surface);
    let mut other = prepared.main.clone();
    other.surface = Some(reversed);
    prepared.extra.push(other);
    let charges = prepared.texture_charges();
    let arrays: Vec<_> = charges.iter().filter(|t| t.slot == TextureSlot::SplatAlbedo).collect();
    assert_eq!(arrays.len(), 2, "the two bindings hold two arrays");
    for array in arrays {
        assert_eq!((array.bytes, array.layers, array.mips, array.compressed),
            (32, 2, 1, Some(crate::material_surface::GpuCompressedFormat::Bc7)), "array metadata describes the uploaded blocks");
    }
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    let _uploaded = renderer.upload_static_preview(&mut cx, prepared);
    assert_eq!(renderer.cached_texture_keys().collect::<std::collections::HashSet<_>>(),
        charges.iter().map(|t| t.key).collect(), "splat layers charge only their arrays and weights");
}

#[test]
fn generated_png_limits_are_checked_before_pixel_decode() {
    let png = Cx::encode_rgba_as_png(4, 4, &[255; 64]).unwrap();
    assert!(decode_generated_png(&png, 2, 1024).is_err());
    assert!(decode_generated_png(&png, 8, 32).is_err());
    assert_eq!(decode_generated_png(&png, 8, 64).unwrap().data.len(), 16);
    let mut model = triangle();
    model.texture_png = Some(png);
    let prepared = PreparedStaticPreview::prepare(model).unwrap();
    assert_eq!(prepared.main.texture.data.len(), 16+4+1);
}

#[test]
fn generated_atlas_upload_preserves_repeat_without_building_cpu_mips() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut image = ImageBuffer::default();
    image.width = 2; image.height = 2; image.data = vec![0xff00_ff00; 4];
    let texture = upload_generated_texture(&mut cx, image);
    match texture.get_format(&mut cx) {
        TextureFormat::VecMipBGRAu8_32 { max_level: Some(0), wrap: TextureWrap::Repeat, data: Some(data), .. } => assert_eq!(data.len(), 4),
        _ => panic!("generated atlas upload must use its prepared single level"),
    }
}

#[test]
fn preparation_retains_geometry_and_derives_collision_off_ui() {
    let model = triangle();
    let prepared = PreparedStaticPreview::prepare(model).unwrap();
    assert_eq!(prepared.main.indices, [0, 1, 2]);
    assert_eq!(prepared.mesh_indices.as_ref(), &prepared.main.indices);
    assert_eq!(prepared.positions.len(), 3);
    assert!(!prepared.collider_parts.boxes().is_empty());
    assert_eq!(prepared.upload_bytes(), (3 * crate::model::MODEL_VERTEX_FLOATS + 3) * 4 + 12);
    let other_pane = prepared.clone();
    assert_eq!(other_pane.main.indices, prepared.main.indices);
    assert_eq!(other_pane.main.vertices.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), prepared.main.vertices.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
}

#[test]
fn preparation_rejects_bad_indices_positions_and_invalid_png() {
    let mut model = triangle();
    model.indices[2] = 999;
    assert!(PreparedStaticPreview::prepare(model).is_err());
    let mut model = triangle();
    model.vertices[0] = f32::NAN;
    assert!(PreparedStaticPreview::prepare(model).is_err());
    let mut model = triangle();
    model.texture_png = Some(vec![1, 2, 3]);
    assert!(PreparedStaticPreview::prepare(model).is_err());
}

#[test]
fn attachment_clips_are_independent_and_aggregate_light_admission_is_atomic() {
    let model=crate::StaticModel::parse_glb(&crate::asset_morph::tests::fixture(false,true)).unwrap();
    let part=&model.anim_parts[0];
    let mut states=ModelStates::default();
    states.clips.insert(StateTarget::Attachment(0),ModelClipPlayback{name:Some("pulse".into()),time:1.,looping:false,weight:1.});
    states.clips.insert(StateTarget::Attachment(1),ModelClipPlayback{name:Some("pulse".into()),time:0.5,looping:false,weight:1.});
    assert_eq!(states.transform(&ModelTarget::Attachment(0),"fixture",part).v[12],2.);
    assert_eq!(states.transform(&ModelTarget::Attachment(1),"fixture",part).v[12],0.);
    let mut renderer=Renderer::default();renderer.set_clustered_lighting(true);
    let light=crate::lightmap::LmLight::omni(vec3f(0.,0.,0.),vec3f(1.,1.,1.),10.);
    // Over the authored budget nothing is refused: the frame keeps the
    // lights nearest the eye and fades the tail.
    renderer.add_asset_frame_lights(vec![light.clone();crate::asset_lights::MAX_ASSET_FRAME_LIGHTS]).unwrap();
    renderer.add_asset_frame_lights(vec![light]).unwrap();
    assert_eq!(renderer.host_asset_lights.len(),crate::asset_lights::MAX_ASSET_FRAME_LIGHTS+1);
    let mut lights:Vec<_>=(0..400).map(|i|crate::lightmap::LmLight::omni(vec3f(i as f32,0.,0.),vec3f(1.,1.,1.),10.)).collect();
    super::lights::budget_authored_lights(&mut lights,0,vec3f(0.,0.,0.),crate::asset_lights::MAX_ASSET_FRAME_LIGHTS);
    assert_eq!(lights.len(),crate::asset_lights::MAX_ASSET_FRAME_LIGHTS);
    assert!(lights.iter().all(|l|l.pos.x<256.0),"the nearest lights win");
    assert!(lights[0].color.x==1.0&&lights[255].color.x<0.05,"the tail fades to the cut");
}

#[test]
fn morph_shadow_casters_share_visible_weights_and_exclude_rest_duplicates() {
    let mut cx=Cx::new(Box::new(|_,_|{}));
    let bytes=crate::asset_morph::tests::fixture(false,false);
    let model=StaticModel::parse_glb(&bytes).unwrap();
    let morph=crate::asset_morph::AssetMorph::parse(&bytes,false).unwrap();
    let prepared=PreparedStaticPreview::prepare(model).unwrap().with_morph(morph);
    let mut renderer=Renderer::default();
    let uploaded=renderer.upload_static_preview(&mut cx,prepared);
    renderer.install_uploaded_static("morph",uploaded);
    let instance=ModelInstance{model:"morph".into(),transform:Mat4f::identity(),tint:vec4(1.,1.,1.,1.),color_adjust:vec4(0.,1.,1.,0.),dynamic:false,depth_order:0.,part_poses:Vec::new(),custom_material:None,lamps:Default::default()};
    renderer.set_models(vec![instance.clone()]);
    renderer.set_world_attachments(vec![instance]);
    renderer.set_model_clip(ModelTarget::Instance(0),Some("pulse".into()),0.75,false).unwrap();
    renderer.set_model_clip_weighted(ModelTarget::Attachment(0),Some("pulse".into()),1.,false,0.5).unwrap();
    let movers=renderer.collect_lm_movers(&World::default(),vec3f(0.,0.,0.),1.,None);
    assert_eq!(movers.len(),2);
    assert_eq!(movers[0].morph.as_ref().unwrap().weights[0].x,0.75);
    assert_eq!(movers[1].morph.as_ref().unwrap().weights[0].x,0.625);
    assert!(renderer.csm_static_casters.is_empty());
    assert!(renderer.collect_local_static_casters(&mut cx,&World::default()).is_empty());
}

/// Rigid and mover ENTITIES cast in their own shape: a marble's bake/CSM
/// caster is the sphere mesh, not the unit box every body used to get
/// (USER_PLAYTEST_1, "the ball has a CUBE as its shadow").
#[test]
fn entity_casters_keep_their_primitive_shape(){
    let mut cx=Cx::new(Box::new(|_,_|{}));
    let mut renderer=Renderer::default();
    let mut world=World::default();
    let body=|id:u64,shape:Shape|Entity{id,shape,kind:BodyKind::Rigid,half:vec3f(0.5,0.5,0.5),scale:vec3f(1.,1.,1.),..Default::default()};
    world.entities=vec![body(1,Shape::Sphere),body(2,Shape::Box),body(3,Shape::Cylinder)];
    // No geometry resident yet: nothing is drawn as the wrong shape.
    assert!(renderer.collect_lm_movers(&world,vec3f(0.,0.,0.),1.,None).is_empty());
    renderer.ensure_entity_caster_geometries(&mut cx);
    let movers=renderer.collect_lm_movers(&world,vec3f(0.,0.,0.),1.,None);
    assert_eq!(movers.len(),3);
    let id=|shape:Shape|renderer.shadow_shape_geometries[shape.index()].as_ref().unwrap().geometry_id();
    assert_eq!(movers[0].geometry,id(Shape::Sphere));
    assert_eq!(movers[1].geometry,id(Shape::Box));
    assert_eq!(movers[2].geometry,id(Shape::Cylinder));
    assert_ne!(id(Shape::Sphere),id(Shape::Box));
}
