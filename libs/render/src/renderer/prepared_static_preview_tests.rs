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
        prelit: false, anim_parts: Vec::new(), driven_parts: Vec::new(), sky: None,
        pbr: Default::default(),
    }
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
    assert!(!prepared.collider_parts.is_empty());
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
    states.clips.insert(ModelTarget::Attachment(0),ModelClipPlayback{name:Some("pulse".into()),time:1.,looping:false,weight:1.});
    states.clips.insert(ModelTarget::Attachment(1),ModelClipPlayback{name:Some("pulse".into()),time:0.5,looping:false,weight:1.});
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
    let instance=ModelInstance{model:"morph".into(),transform:Mat4f::identity(),tint:vec4(1.,1.,1.,1.),color_adjust:vec4(0.,1.,1.,0.),dynamic:false,depth_order:0.,part_poses:Vec::new(),custom_material:None};
    renderer.set_models(vec![instance.clone()]);
    renderer.set_world_attachments(vec![instance]);
    renderer.set_model_clip(ModelTarget::Instance(0),Some("pulse".into()),0.75,false).unwrap();
    renderer.set_model_clip_weighted(ModelTarget::Attachment(0),Some("pulse".into()),1.,false,0.5).unwrap();
    let movers=renderer.collect_lm_movers(&World::default(),vec3f(0.,0.,0.),None);
    assert_eq!(movers.len(),2);
    assert_eq!(movers[0].morph.as_ref().unwrap().weights[0].x,0.75);
    assert_eq!(movers[1].morph.as_ref().unwrap().weights[0].x,0.625);
    assert!(renderer.csm_static_casters.is_empty());
    assert!(renderer.collect_local_static_casters(&mut cx,&World::default()).is_empty());
}
