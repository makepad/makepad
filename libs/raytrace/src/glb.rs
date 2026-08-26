//! GLB/glTF → `SceneInput`: every mesh primitive under every node, with node
//! transforms flattened, PBR metallic-roughness materials, embedded
//! base-colour images decoded into the atlas list. Y-up (glTF's law).

use crate::scene::{Camera, Image, Material, SceneInput, Sun};
use makepad_draw::image_cache::ImageBuffer;
use makepad_draw::*;
use makepad_gltf::{decode_mesh_primitive, load_image_bytes, GltfNode, JsonValue, LoadedGltf};

pub fn load_glb(path: &std::path::Path) -> Result<SceneInput, String> {
    let loaded = makepad_gltf::load_gltf_from_path(path).map_err(|e| format!("{e:?}"))?;
    scene_from_gltf(&loaded)
}

pub fn scene_from_gltf(loaded: &LoadedGltf) -> Result<SceneInput, String> {
    let doc = &loaded.document;
    let mut scene = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };

    // Images (decode lazily: only the ones a material references).
    let mut image_slot: Vec<Option<usize>> = vec![None; doc.images_slice().len()];
    let mut image_of_texture = |scene: &mut SceneInput, tex: usize| -> Option<usize> {
        let src = doc.textures_slice().get(tex)?.source?;
        if let Some(slot) = image_slot.get(src).copied().flatten() {
            return Some(slot);
        }
        let bytes = load_image_bytes(loaded, src).ok()?;
        let buf = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            ImageBuffer::from_png(&bytes).ok()?
        } else {
            ImageBuffer::from_jpg(&bytes).ok()?
        };
        scene.images.push(Image { width: buf.width, height: buf.height, data: buf.data });
        let slot = scene.images.len() - 1;
        image_slot[src] = Some(slot);
        Some(slot)
    };

    // Materials.
    for m in doc.materials_slice() {
        let pbr = m.pbr_metallic_roughness.as_ref();
        let base = pbr.and_then(|p| p.base_color_factor).unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let mut mat = Material {
            albedo: [base[0], base[1], base[2]],
            roughness: pbr.and_then(|p| p.roughness_factor).unwrap_or(1.0),
            metal: pbr.and_then(|p| p.metallic_factor).unwrap_or(1.0),
            emission: m.emissive_factor.unwrap_or([0.0; 3]),
            two_sided: m.double_sided.unwrap_or(false),
            ..Default::default()
        };
        // glTF's metallic default is 1.0, which turns untextured Kenney
        // props into chrome; only trust it when a factor is given.
        if pbr.map_or(true, |p| p.metallic_factor.is_none()) {
            mat.metal = 0.0;
        }
        if m.alpha_mode.as_deref() == Some("BLEND") && base[3] < 0.99 {
            mat.transmission = 1.0 - base[3];
            mat.two_sided = true;
        }
        let extension_number = |extension: &str, field: &str| {
            let value = m.extensions.as_ref()?.key(extension)?.key(field)?;
            match value {
                JsonValue::U64(value) => Some(*value as f32),
                JsonValue::U128(value) => Some(*value as f32),
                JsonValue::I64(value) => Some(*value as f32),
                JsonValue::I128(value) => Some(*value as f32),
                JsonValue::F64(value) => Some(*value as f32),
                _ => None,
            }
        };
        if let Some(transmission) =
            extension_number("KHR_materials_transmission", "transmissionFactor")
        {
            mat.transmission = transmission.clamp(0.0, 1.0);
        }
        if let Some(ior) = extension_number("KHR_materials_ior", "ior") {
            mat.ior = ior.max(1.0);
        }
        if mat.transmission > 0.0 {
            // The tracer models architectural glazing as a thin sheet.
            mat.two_sided = true;
        }
        if let Some(t) = pbr.and_then(|p| p.base_color_texture.as_ref()) {
            mat.texture = image_of_texture(&mut scene, t.index);
        }
        scene.materials.push(mat);
    }
    if scene.materials.is_empty() {
        scene.materials.push(Material::default());
    }
    let default_mat = scene.materials.len() as u32 - 1;

    // Nodes.
    let roots: Vec<usize> = match doc.scenes_slice().first().and_then(|s| s.nodes.clone()) {
        Some(n) => n,
        None => (0..doc.nodes_slice().len()).collect(),
    };
    fn node_matrix(n: &GltfNode) -> Mat4f {
        if let Some(m) = n.matrix {
            return Mat4f { v: m };
        }
        let t = n.translation.unwrap_or([0.0; 3]);
        let r = n.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let s = n.scale.unwrap_or([1.0; 3]);
        let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
        // Column-major TRS.
        let m = [
            (1.0 - 2.0 * (y * y + z * z)) * s[0], (2.0 * (x * y + z * w)) * s[0], (2.0 * (x * z - y * w)) * s[0], 0.0,
            (2.0 * (x * y - z * w)) * s[1], (1.0 - 2.0 * (x * x + z * z)) * s[1], (2.0 * (y * z + x * w)) * s[1], 0.0,
            (2.0 * (x * z + y * w)) * s[2], (2.0 * (y * z - x * w)) * s[2], (1.0 - 2.0 * (x * x + y * y)) * s[2], 0.0,
            t[0], t[1], t[2], 1.0,
        ];
        Mat4f { v: m }
    }
    let mut stack: Vec<(usize, Mat4f)> = roots.iter().map(|&n| (n, Mat4f::identity())).collect();
    let mut visited = 0usize;
    while let Some((ni, parent)) = stack.pop() {
        visited += 1;
        if visited > 100_000 {
            break;
        }
        let Some(node) = doc.nodes_slice().get(ni) else { continue };
        let world = Mat4f::mul(&parent, &node_matrix(node));
        if let Some(children) = &node.children {
            for &c in children {
                stack.push((c, world));
            }
        }
        let Some(mi) = node.mesh else { continue };
        let Some(mesh) = doc.meshes_slice().get(mi) else { continue };
        for pi in 0..mesh.primitives.len() {
            let Ok(prim) = decode_mesh_primitive(loaded, mi, pi) else { continue };
            let xf = |p: [f32; 3]| {
                let v = world.transform_vec4(vec4(p[0], p[1], p[2], 1.0));
                [v.x, v.y, v.z]
            };
            let positions: Vec<[f32; 3]> = prim.positions.iter().map(|&p| xf(p)).collect();
            let normals: Option<Vec<[f32; 3]>> = prim.normals.as_ref().map(|ns| {
                ns.iter()
                    .map(|&n| {
                        let v = world.transform_vec4(vec4(n[0], n[1], n[2], 0.0));
                        let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1.0e-9);
                        [v.x / l, v.y / l, v.z / l]
                    })
                    .collect()
            });
            let mat = prim.material.map(|m| m as u32).unwrap_or(default_mat);
            scene.push_mesh(&positions, normals.as_deref(), prim.texcoords0.as_deref(), &prim.indices, mat);
        }
    }
    scene.ensure_normals();

    // A camera that frames the whole thing from the front-right, sun high.
    let (lo, hi) = scene.bounds();
    let center = (lo + hi) * 0.5;
    let radius = (hi - lo).length() * 0.5;
    let fov = 40.0f32.to_radians();
    let dist = radius / (fov * 0.5).tan() * 1.1;
    scene.camera = Camera {
        pos: center + vec3f(0.6, 0.35, 0.75).normalize() * dist,
        target: center,
        up: vec3f(0.0, 1.0, 0.0),
        fov_y: fov,
        focus_dist: dist,
        f_stop: 8.0,
        ..Default::default()
    };
    scene.sun = Sun::default();
    Ok(scene)
}
