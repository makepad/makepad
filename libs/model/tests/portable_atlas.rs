use makepad_model::*;

fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) {
    doc.apply(Transaction { request_id: id.into(), expected: doc.head(), operations }, None).unwrap();
}
fn texture(limits: &Limits) -> Vec<u8> {
    Texture { width: 2, height: 2, rgb: vec![128,160,200, 240,40,80, 20,220,90, 80,100,250] }.to_png(limits).unwrap()
}
fn document(two_materials: bool, uv_scale: f64, uv_offset: f64) -> Document {
    let mut doc = Document::new(Limits::default()).unwrap();
    let mut ops = vec![Operation::Plane { object: "a".into(), size: [2.0;2] },
        Operation::SetMaterial { material: 0, value: Material { color: [0.25,0.5,0.75], base_color_png: texture(doc.limits()) } }];
    if two_materials {
        ops.push(Operation::Plane { object: "b".into(), size: [2.0;2] });
        ops.push(Operation::SetMaterial { material: 7, value: Material { color: [0.8,0.2,0.4], base_color_png: texture(doc.limits()) } });
    }
    apply(&mut doc, "planes", ops);
    let mut ops = Vec::new();
    for (name, mesh) in doc.objects() {
        for corner in mesh.corners() {
            let p = mesh.vertex(corner.vertex).unwrap().position;
            ops.push(Operation::SetUv { object: name.into(), corner: corner.id,
                uv: [(p[0]+1.0)*0.5*uv_scale+uv_offset, (p[2]+1.0)*0.5*uv_scale+uv_offset] });
        }
        if name == "b" { ops.push(Operation::AssignMaterial { object: name.into(), faces: mesh.faces().iter().map(|f|f.id).collect(), material: 7 }); }
    }
    ops.push(Operation::SetSkeleton { skeleton: Skeleton { joints: vec![Joint { name: "root".into(), parent: None, translation: [0.;3] }] } });
    for name in ["a", "b"].into_iter().take(if two_materials {2} else {1}) { ops.push(Operation::AutoWeights { object: name.into() }); }
    for name in ["idle", "walk"] { ops.push(Operation::SetClip { clip: AnimationClip { name: name.into(), channels: vec![AnimationChannel {
        joint: 0, path: AnimationPath::Translation, keys: vec![Keyframe { time: 0., value: [0.;4] }, Keyframe { time: 1., value: [0.,0.1,0.,0.] }],
    }] } }); }
    apply(&mut doc, "skin", ops);
    doc
}
fn linear(byte: u8) -> f64 {
    let value=byte as f64/255.; if value <=0.04045 {value/12.92} else {((value+0.055)/1.055).powf(2.4)}
}
// This consumer reads only the compiled PNG and UV accessor. Like the old
// character loader, it never consults source materials or the authoring engine.
fn sample(image: &Texture, uv: [f64;2]) -> [f64;3] {
    let at=[uv[0]*image.width as f64-0.5, uv[1]*image.height as f64-0.5];
    let low=[at[0].floor() as i64,at[1].floor() as i64];
    let fraction=[at[0]-at[0].floor(),at[1]-at[1].floor()];
    let mut color=[0.;3];
    for y in 0..2 {for x in 0..2 {
        let sx=(low[0]+x).rem_euclid(image.width as i64) as usize;
        let sy=(low[1]+y).rem_euclid(image.height as i64) as usize;
        let weight=if x==0 {1.-fraction[0]}else{fraction[0]} * if y==0 {1.-fraction[1]}else{fraction[1]};
        for lane in 0..3 {color[lane]+=linear(image.rgb[(sy*image.width as usize+sx)*3+lane])*weight;}
    }}
    color
}
fn original_uvs(doc: &Document, object: &str) -> Vec<[f64;2]> {
    let mut ctx=mesh::Context::new(doc.limits().mesh.clone(),None);
    let triangles=doc.object(object).unwrap().triangulate(&mut ctx).unwrap();
    triangles.triangles.iter().flat_map(|triangle|triangle.indices.iter().map(|&index|triangles.vertices[index as usize].uv)).collect()
}
fn compare_first_image(doc: &Document, compiled: &CompiledModel) {
    let loaded=makepad_gltf::load_gltf_from_bytes(&compiled.glb,None).unwrap();
    assert_eq!(loaded.document.materials_slice().len(),1);
    assert_eq!(loaded.document.images_slice().len(),1);
    let png=makepad_gltf::load_image_bytes(&loaded,0).unwrap();
    let image=Texture::from_png(&png,doc.limits()).unwrap();
    let factor=loaded.document.materials_slice()[0].pbr_metallic_roughness.as_ref().unwrap().base_color_factor.unwrap_or([1.;4]);
    assert_eq!(factor,[1.;4]);
    assert_eq!(loaded.document.skins.as_ref().unwrap().len(),1);
    assert_eq!(loaded.document.animations.as_ref().unwrap().len(),2);
    let json=String::from_utf8_lossy(&compiled.glb);
    assert!(json.contains("\"minFilter\":9729"),"shared atlas must avoid cross-tile mip bleeding");
    for (index, source) in compiled.primitives.iter().enumerate() {
        let primitive=&loaded.document.meshes_slice()[0].primitives[index];
        assert_eq!(primitive.material,Some(0));
        assert!(primitive.attributes.contains_key("JOINTS_0") && primitive.attributes.contains_key("WEIGHTS_0"));
        let decoded=makepad_gltf::decode_mesh_primitive(&loaded,0,index).unwrap();
        let exported=decoded.texcoords0.unwrap();
        let original=original_uvs(doc,&source.object);
        let material=&doc.materials()[&source.material];
        let source_image=Texture::from_png(&material.base_color_png,doc.limits()).unwrap();
        assert_eq!(original.len(),exported.len());
        for (before,after) in original.chunks_exact(3).zip(exported.chunks_exact(3)) {
            for weights in [[1.,0.,0.],[0.,1.,0.],[0.,0.,1.],[0.2,0.3,0.5],[0.33,0.34,0.33]] {
                let original_uv=std::array::from_fn(|axis| (0..3).map(|i|before[i][axis]*weights[i]).sum());
                let compiled_uv=std::array::from_fn(|axis| (0..3).map(|i|after[i][axis] as f64*weights[i]).sum());
                let original_color=sample(&source_image,original_uv);
                let actual=sample(&image,compiled_uv);
                for lane in 0..3 {
                    let expected=original_color[lane]*material.color[lane] as f64;
                    assert!((actual[lane]-expected).abs()<0.0045,"primitive{index} lane{lane}: expected{expected}, got{} (UV{original_uv:?})",actual[lane]);
                }
            }
        }
    }
}

#[test]
fn portable_first_image_preserves_all_material_colors_repeat_seams_skin_and_source() {
    let doc=document(true,2.0,-0.5);
    let source=doc.to_bytes(None).unwrap();
    let compiled=doc.compile(None).unwrap();
    assert_eq!(doc.materials().len(),2);
    assert_eq!(compiled.primitives.len(),2);
    assert_eq!(doc.to_bytes(None).unwrap(),source,"export must not rewrite editable UVs or materials");
    compare_first_image(&doc,&compiled);
    let reopened=Document::from_bytes(&source,Limits::default(),None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(),source);
    assert_eq!(reopened.materials(),doc.materials());
    assert_eq!(reopened.compile(None).unwrap().glb,compiled.glb);
}

#[test]
fn single_material_keeps_arbitrary_repeat_uvs_and_bakes_linear_color_factor() {
    let doc=document(false,2.0,40.0);
    let compiled=doc.compile(None).unwrap();
    compare_first_image(&doc,&compiled);
    let loaded=makepad_gltf::load_gltf_from_bytes(&compiled.glb,None).unwrap();
    let actual=makepad_gltf::decode_mesh_primitive(&loaded,0,0).unwrap().texcoords0.unwrap();
    assert_eq!(actual,original_uvs(&doc,"a").iter().map(|uv|[uv[0] as f32,uv[1] as f32]).collect::<Vec<_>>());
    let image=Texture::from_png(&makepad_gltf::load_image_bytes(&loaded,0).unwrap(),doc.limits()).unwrap();
    assert_eq!((image.width,image.height),(2,2));
    assert!(image.rgb[0]>60 && image.rgb[0]<70,"linear factor is not naive sRGB byte multiplication");
}

#[test]
fn unsafe_repeat_domains_and_atlas_budgets_refuse_without_source_mutation() {
    for doc in [document(true,2.0,40.0), document(true,9.0,0.0)] {
        let before=doc.to_bytes(None).unwrap();
        assert!(doc.compile(None).is_err());
        assert_eq!(doc.to_bytes(None).unwrap(),before);
    }
    let mut doc=document(true,1.0,0.0);
    let png=Texture::solid(1024,1,[120,30,90],doc.limits()).unwrap().to_png(doc.limits()).unwrap();
    apply(&mut doc,"large",vec![Operation::SetMaterial {material:7,value:Material {color:[1.;3],base_color_png:png}}]);
    let before=doc.to_bytes(None).unwrap();
    assert!(matches!(doc.compile(None),Err(Error::Budget("skin atlas tile dimensions"))));
    assert_eq!(doc.to_bytes(None).unwrap(),before);
    assert!(matches!(document(true,1.,0.).compile(Some(&||true)),Err(Error::Mesh(mesh::MeshError::Cancelled))));
}
