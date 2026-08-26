//! A small procedural house so the viewer, the tests and every lane have a
//! model before the parser lands. Two stories, slab, walls with door and
//! window openings (as separate elements), a hipped roof, a few pieces of
//! furniture. Meters, Z up, right-handed — exactly what the parser must
//! produce (`ModelData` conventions).

use crate::model::ids::{ElementId, LayerId, MaterialId, MeshId, StoryId};
use crate::model::model::*;
use crate::model::units::{LengthUnit, Units};
use makepad_math::Mat4f;

struct Builder {
    model: ModelData,
}

impl Builder {
    fn material(&mut self, name: &str, rgba: [f32; 4], rough: f32, metal: f32) -> MaterialId {
        self.model.materials.push(MaterialData {
            name: name.into(),
            base_color: rgba,
            roughness: rough,
            metallic: metal,
            ..Default::default()
        });
        MaterialId::from_index(self.model.materials.len() - 1)
    }

    fn glass(&mut self) -> MaterialId {
        self.model.materials.push(MaterialData {
            name: "Glass".into(),
            base_color: [0.7, 0.85, 0.95, 0.35],
            roughness: 0.05,
            metallic: 0.0,
            ior: 1.5,
            transmission: 0.9,
            double_sided: true,
            ..Default::default()
        });
        MaterialId::from_index(self.model.materials.len() - 1)
    }

    /// Axis-aligned box mesh from `min` to `max`, flat normals, one submesh.
    fn box_mesh(&mut self, min: [f32; 3], max: [f32; 3], material: MaterialId) -> MeshId {
        let mut positions = Vec::with_capacity(24);
        let mut normals = Vec::with_capacity(24);
        let mut uvs = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        // (normal, u axis, v axis)
        let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let c = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let h = [
            (max[0] - min[0]) * 0.5,
            (max[1] - min[1]) * 0.5,
            (max[2] - min[2]) * 0.5,
        ];
        for (n, u, v) in faces {
            let base = positions.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = [
                    c[0] + (n[0] + u[0] * su + v[0] * sv) * h[0],
                    c[1] + (n[1] + u[1] * su + v[1] * sv) * h[1],
                    c[2] + (n[2] + u[2] * su + v[2] * sv) * h[2],
                ];
                positions.push(p);
                normals.push(n);
                uvs.push([(su + 1.0) * 0.5, (sv + 1.0) * 0.5]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let count = indices.len() as u32;
        self.model.meshes.push(MeshData {
            positions,
            normals,
            uvs,
            indices,
            contour_edges: Vec::new(),
            submeshes: vec![SubMesh {
                material,
                first_index: 0,
                index_count: count,
            }],
        });
        MeshId::from_index(self.model.meshes.len() - 1)
    }

    /// Triangle prism along Y (a gable roof half) — used for the roof.
    fn roof_mesh(
        &mut self,
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        z_eave: f32,
        z_ridge: f32,
        material: MaterialId,
    ) -> MeshId {
        let xm = (x0 + x1) * 0.5;
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        let mut quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]| {
            let base = positions.len() as u32;
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            let n = [n[0] / l, n[1] / l, n[2] / l];
            for p in [a, b, c, d] {
                positions.push(p);
                normals.push(n);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        // south slope (faces -X side up)
        quad(
            [x0, y0, z_eave],
            [xm, y0, z_ridge],
            [xm, y1, z_ridge],
            [x0, y1, z_eave],
        );
        // north slope
        quad(
            [xm, y0, z_ridge],
            [x1, y0, z_eave],
            [x1, y1, z_eave],
            [xm, y1, z_ridge],
        );
        // gable triangles (as degenerate quads)
        quad(
            [x0, y0, z_eave],
            [x1, y0, z_eave],
            [xm, y0, z_ridge],
            [xm, y0, z_ridge],
        );
        quad(
            [x1, y1, z_eave],
            [x0, y1, z_eave],
            [xm, y1, z_ridge],
            [xm, y1, z_ridge],
        );
        let count = indices.len() as u32;
        let n = positions.len();
        self.model.meshes.push(MeshData {
            positions,
            normals,
            uvs: vec![[0.0, 0.0]; n],
            indices,
            contour_edges: Vec::new(),
            submeshes: vec![SubMesh {
                material,
                first_index: 0,
                index_count: count,
            }],
        });
        MeshId::from_index(self.model.meshes.len() - 1)
    }

    fn element(
        &mut self,
        name: &str,
        class: ElementClass,
        story: Option<StoryId>,
        layer: LayerId,
        parent: Option<ElementId>,
        meshes: Vec<MeshId>,
    ) -> ElementId {
        let id = ElementId::from_index(self.model.elements.len());
        let class_label = class.label().to_string();
        self.model.elements.push(ElementData {
            id,
            guid: format!("DEMO-{:04}", id.0),
            name: name.into(),
            class,
            story,
            layer: Some(layer),
            parent,
            transform: Mat4f::default(),
            meshes: meshes.into_iter().map(crate::model::model::MeshRef::identity).collect(),
            properties: vec![
                Property {
                    group: "General".into(),
                    name: "Type".into(),
                    value: PropertyValue::Text(class_label),
                },
                Property {
                    group: "General".into(),
                    name: "Source".into(),
                    value: PropertyValue::Text("Demo house".into()),
                },
            ],
            quantities: Vec::new(),
        });
        id
    }
}

/// The demo house. Footprint 10 m × 7 m, two stories of 3 m, gable roof.
pub fn demo_house() -> ModelData {
    let mut b = Builder {
        model: ModelData {
            name: "Demo House".into(),
            units: Units {
                source_to_meters: 1.0,
                display: LengthUnit::Meter,
                precision: 2,
            },
            up_axis: UpAxis::Z,
            handedness: Handedness::Right,
            ..Default::default()
        },
    };
    b.model
        .metadata
        .push(("Project".into(), "Fab viewer demo house".into()));

    let story0 = StoryId::from_index(0);
    let story1 = StoryId::from_index(1);
    b.model.stories.push(StoryData {
        id: story0,
        name: "Ground Floor".into(),
        elevation: 0.0,
        height: 3.0,
    });
    b.model.stories.push(StoryData {
        id: story1,
        name: "1st Floor".into(),
        elevation: 3.0,
        height: 3.0,
    });
    let l_structure = LayerId::from_index(0);
    let l_openings = LayerId::from_index(1);
    let l_furniture = LayerId::from_index(2);
    let l_site = LayerId::from_index(3);
    for (id, name) in [
        (l_structure, "Structure"),
        (l_openings, "Openings"),
        (l_furniture, "Furniture"),
        (l_site, "Site"),
    ] {
        b.model.layers.push(LayerData {
            id,
            name: name.into(),
            visible: true,
        });
    }

    let m_plaster = b.material("Plaster White", [0.86, 0.85, 0.82, 1.0], 0.85, 0.0);
    let m_concrete = b.material("Concrete", [0.55, 0.55, 0.54, 1.0], 0.9, 0.0);
    let m_roof = b.material("Roof Tile", [0.45, 0.22, 0.16, 1.0], 0.8, 0.0);
    let m_wood = b.material("Oak", [0.55, 0.38, 0.22, 1.0], 0.55, 0.0);
    let m_metal = b.material("Brushed Steel", [0.7, 0.7, 0.72, 1.0], 0.35, 0.9);
    let m_grass = b.material("Grass", [0.28, 0.42, 0.2, 1.0], 0.95, 0.0);
    let m_glass = b.glass();

    let (x0, x1, y0, y1) = (0.0f32, 10.0f32, 0.0f32, 7.0f32);
    let t = 0.25f32; // wall thickness

    // Site
    let site = b.element("Site", ElementClass::Site, None, l_site, None, vec![]);
    let terrain = b.box_mesh([x0 - 6.0, y0 - 6.0, -0.3], [x1 + 6.0, y1 + 6.0, 0.0], m_grass);
    b.element(
        "Terrain",
        ElementClass::Mesh,
        None,
        l_site,
        Some(site),
        vec![terrain],
    );

    let building = b.element(
        "Building",
        ElementClass::Group,
        None,
        l_structure,
        Some(site),
        vec![],
    );

    for (si, story, z) in [(0, story0, 0.0f32), (1, story1, 3.0f32)] {
        let group = b.element(
            if si == 0 { "Ground Floor" } else { "1st Floor" },
            ElementClass::Group,
            Some(story),
            l_structure,
            Some(building),
            vec![],
        );
        // slab
        let slab = b.box_mesh([x0, y0, z - 0.25], [x1, y1, z], m_concrete);
        b.element(
            &format!("Slab {:02}", si + 1),
            ElementClass::Slab,
            Some(story),
            l_structure,
            Some(group),
            vec![slab],
        );
        // four walls (with a door gap on the south wall of the ground floor)
        let h = z + 3.0;
        let south = if si == 0 {
            vec![
                b.box_mesh([x0, y0, z], [4.0, y0 + t, h], m_plaster),
                b.box_mesh([5.0, y0, z], [x1, y0 + t, h], m_plaster),
                b.box_mesh([4.0, y0, z + 2.2], [5.0, y0 + t, h], m_plaster),
            ]
        } else {
            vec![b.box_mesh([x0, y0, z], [x1, y0 + t, h], m_plaster)]
        };
        b.element(
            &format!("Wall S{:02}", si + 1),
            ElementClass::Wall,
            Some(story),
            l_structure,
            Some(group),
            south,
        );
        let north = b.box_mesh([x0, y1 - t, z], [x1, y1, h], m_plaster);
        b.element(
            &format!("Wall N{:02}", si + 1),
            ElementClass::Wall,
            Some(story),
            l_structure,
            Some(group),
            vec![north],
        );
        let west = b.box_mesh([x0, y0, z], [x0 + t, y1, h], m_plaster);
        b.element(
            &format!("Wall W{:02}", si + 1),
            ElementClass::Wall,
            Some(story),
            l_structure,
            Some(group),
            vec![west],
        );
        let east = b.box_mesh([x1 - t, y0, z], [x1, y1, h], m_plaster);
        b.element(
            &format!("Wall E{:02}", si + 1),
            ElementClass::Wall,
            Some(story),
            l_structure,
            Some(group),
            vec![east],
        );
        // interior wall
        let inner = b.box_mesh([5.5, y0 + t, z], [5.5 + 0.12, y1 - t, h], m_plaster);
        b.element(
            &format!("Wall I{:02}", si + 1),
            ElementClass::Wall,
            Some(story),
            l_structure,
            Some(group),
            vec![inner],
        );
        // windows: glass panes set into the north wall
        for (wi, wx) in [1.5f32, 4.0, 7.0].iter().enumerate() {
            let pane = b.box_mesh(
                [*wx, y1 - t * 0.5 - 0.02, z + 0.9],
                [*wx + 1.4, y1 - t * 0.5 + 0.02, z + 2.3],
                m_glass,
            );
            let frame = b.box_mesh(
                [*wx - 0.05, y1 - t * 0.5 - 0.05, z + 0.85],
                [*wx + 1.45, y1 - t * 0.5 - 0.02, z + 0.9],
                m_metal,
            );
            b.element(
                &format!("Window {}{:02}", si + 1, wi + 1),
                ElementClass::Window,
                Some(story),
                l_openings,
                Some(group),
                vec![pane, frame],
            );
        }
        if si == 0 {
            let door = b.box_mesh([4.05, y0 + 0.05, z], [4.95, y0 + 0.1, z + 2.15], m_wood);
            b.element(
                "Door 001",
                ElementClass::Door,
                Some(story),
                l_openings,
                Some(group),
                vec![door],
            );
            // furniture: a table and two chairs
            let top = b.box_mesh([1.5, 2.5, 0.72], [3.1, 3.4, 0.76], m_wood);
            let legs: Vec<MeshId> = [(1.55, 2.55), (3.0, 2.55), (1.55, 3.3), (3.0, 3.3)]
                .iter()
                .map(|(lx, ly)| b.box_mesh([*lx, *ly, 0.0], [*lx + 0.05, *ly + 0.05, 0.72], m_metal))
                .collect();
            let mut table = vec![top];
            table.extend(legs);
            b.element(
                "Table",
                ElementClass::Furniture,
                Some(story),
                l_furniture,
                Some(group),
                table,
            );
            for (ci, cy) in [1.9f32, 3.6].iter().enumerate() {
                let seat = b.box_mesh([2.1, *cy, 0.42], [2.5, *cy + 0.4, 0.46], m_wood);
                let back = b.box_mesh([2.1, *cy + 0.36, 0.46], [2.5, *cy + 0.4, 0.9], m_wood);
                b.element(
                    &format!("Chair {:02}", ci + 1),
                    ElementClass::Furniture,
                    Some(story),
                    l_furniture,
                    Some(group),
                    vec![seat, back],
                );
            }
            // a column
            let col = b.box_mesh([7.5, 3.3, 0.0], [7.8, 3.6, 3.0], m_concrete);
            b.element(
                "Column 001",
                ElementClass::Column,
                Some(story),
                l_structure,
                Some(group),
                vec![col],
            );
        } else {
            let bed = b.box_mesh([6.5, 1.0, z], [8.5, 3.0, z + 0.5], m_wood);
            b.element(
                "Bed",
                ElementClass::Furniture,
                Some(story),
                l_furniture,
                Some(group),
                vec![bed],
            );
        }
    }

    // roof on top of the 1st floor
    let roof = b.roof_mesh(x0 - 0.4, x1 + 0.4, y0 - 0.4, y1 + 0.4, 6.0, 8.2, m_roof);
    b.element(
        "Roof",
        ElementClass::Roof,
        Some(story1),
        l_structure,
        Some(building),
        vec![roof],
    );

    b.model
}

/// A synthetic model of a given size — the stress fixture the performance
/// gates run against (5 M triangles, interactive, sub-ms
/// pick"). It is deliberately shaped like a real BIM export rather than one
/// giant mesh: a few thousand elements spread over a city block and several
/// storeys, sharing a handful of meshes through instance transforms, with
/// contour edges and per-element metadata.
///
/// `target_triangles` is met to within one mesh; `elements` is exact.
pub fn synthetic_model(target_triangles: usize, elements: usize) -> ModelData {
    let elements = elements.max(1);
    let per_element = (target_triangles / elements).max(2);
    // A sphere of `seg × seg` quads is `2 · seg²` triangles.
    let seg = ((per_element as f32 / 2.0).sqrt().round() as usize).max(1);
    const MESHES: usize = 8;
    const STORIES: usize = 6;

    let mut model = ModelData {
        name: format!("Synthetic {elements} × {}", 2 * seg * seg),
        units: Units {
            source_to_meters: 1.0,
            display: LengthUnit::Meter,
            precision: 2,
        },
        ..Default::default()
    };
    for s in 0..STORIES {
        model.stories.push(StoryData {
            id: StoryId::from_index(s),
            name: format!("Level {s}"),
            elevation: s as f32 * 4.0,
            height: 4.0,
        });
    }
    model.layers.push(LayerData {
        id: LayerId::from_index(0),
        name: "Synthetic".into(),
        visible: true,
    });
    for m in 0..MESHES {
        model
            .meshes
            .push(sphere_mesh(seg, 0.8 + 0.05 * m as f32));
    }

    let side = (elements as f32).sqrt().ceil().max(1.0) as usize;
    let classes = [
        ElementClass::Wall,
        ElementClass::Slab,
        ElementClass::Column,
        ElementClass::Window,
        ElementClass::Object,
    ];
    for i in 0..elements {
        let (gx, gy) = ((i % side) as f32 * 3.0, (i / side) as f32 * 3.0);
        let story = i % STORIES;
        let mut t = Mat4f::default();
        t.v[12] = gx;
        t.v[13] = gy;
        t.v[14] = story as f32 * 4.0;
        model.elements.push(ElementData {
            id: ElementId::from_index(i),
            guid: format!("SYNTH-{i:06}"),
            name: format!("Part {i}"),
            class: classes[i % classes.len()].clone(),
            story: Some(StoryId::from_index(story)),
            layer: Some(LayerId::from_index(0)),
            parent: None,
            transform: t,
            meshes: vec![MeshRef::identity(MeshId::from_index(i % MESHES))],
            properties: Vec::new(),
            quantities: Vec::new(),
        });
    }
    model
}

/// UV sphere with `seg × seg` quads, per-vertex normals and the polar contour
/// rings as edges.
fn sphere_mesh(seg: usize, radius: f32) -> MeshData {
    let mut m = MeshData::default();
    let n = seg.max(1);
    for iv in 0..=n {
        let v = iv as f32 / n as f32;
        let phi = v * std::f32::consts::PI;
        for iu in 0..=n {
            let u = iu as f32 / n as f32;
            let theta = u * std::f32::consts::TAU;
            let d = [
                phi.sin() * theta.cos(),
                phi.sin() * theta.sin(),
                phi.cos(),
            ];
            m.positions.push([d[0] * radius, d[1] * radius, d[2] * radius]);
            m.normals.push(d);
            m.uvs.push([u, v]);
        }
    }
    let row = n + 1;
    for iv in 0..n {
        for iu in 0..n {
            let a = (iv * row + iu) as u32;
            let (b, c, d) = (a + 1, a + row as u32, a + row as u32 + 1);
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
            if iu == 0 || iv == 0 {
                m.contour_edges.extend_from_slice(&[a, b]);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_model_hits_its_size() {
        let m = synthetic_model(120_000, 60);
        assert_eq!(m.elements.len(), 60);
        let tris: usize = m
            .elements
            .iter()
            .flat_map(|e| e.meshes.iter())
            .map(|r| m.meshes[r.mesh.index()].indices.len() / 3)
            .sum();
        // Within one mesh of the target.
        assert!(tris > 100_000 && tris < 140_000, "{tris}");
        for mesh in &m.meshes {
            assert_eq!(mesh.normals.len(), mesh.positions.len());
            assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
        }
    }

    #[test]
    fn demo_house_is_well_formed() {
        let m = demo_house();
        assert!(m.elements.len() > 20);
        for e in &m.elements {
            for mref in &e.meshes {
                let mesh = &m.meshes[mref.mesh.index()];
                assert_eq!(mesh.indices.len() % 3, 0);
                assert_eq!(mesh.normals.len(), mesh.positions.len());
                assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
            }
        }
    }
}
