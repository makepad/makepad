//! Projected mesh decals: the part of a target surface inside a projection box
//! becomes a thin new object floating just above it, with 0..1 UVs across the
//! decal. Badges, vents, text, panel lines and wear marks are materials on
//! such decals (usually alpha-masked), so they never disturb the target's own
//! UVs or tiling textures.
use super::*;
use crate::{mesh::TriangleMesh, transform::Matrix4, Operation, SceneOperation};

#[derive(Clone, Debug, PartialEq)]
pub struct Decal { pub target: String, pub center: [f64; 3], pub normal: [f64; 3], pub up: [f64; 3], pub size: [f64; 2], pub depth: f64, pub offset: f64 }

#[derive(Default)]
pub(super) struct DecalTarget {
    next_use: Vec<NextUse>,
    world: Option<([[u64; 4]; 4], TriangleMesh)>,
    world_bytes: usize,
}

enum NextUse { None, Adjacent, AfterOtherWork }

impl DecalTarget {
    pub(super) fn plan(ops: &[Operation]) -> BTreeMap<String, Self> {
        let mut targets = BTreeMap::<String, Self>::new();
        let mut needed = std::collections::BTreeSet::new();
        let mut next_target = None;
        // A projection keeps its triangles only if another projection can read
        // the same mesh before any possible edit. New operation kinds end reuse.
        for op in ops.iter().rev() {
            match op {
                Operation::Construction(ConstructionOperation { kind: Construction::Decal { decal, .. }, .. }) => {
                    let name = decal.target.as_str();
                    let next_use = if needed.insert(name) {
                        NextUse::None
                    } else if next_target == Some(name) {
                        NextUse::Adjacent
                    } else {
                        NextUse::AfterOtherWork
                    };
                    targets.entry(decal.target.clone()).or_default().next_use.push(next_use);
                    next_target = Some(name);
                    continue;
                }
                // Factories only insert a new object. Parenting can change a
                // world frame, so project compares the full matrix at each use.
                Operation::Scene(SceneOperation::Node { .. }) => continue,
                Operation::Construction(_) | Operation::SetMaterial { .. } | Operation::Surface(_) => {}
                Operation::Transform { object, .. } | Operation::AssignMaterial { object, .. } => {
                    needed.remove(object.as_str());
                }
                _ => needed.clear(),
            }
            next_target = None;
        }
        targets
    }

    pub(super) fn project(
        &mut self, source: &Mesh, frame: Matrix4, decal: &Decal,
        material: u32, limits: &Limits, ctx: &mut Context<'_>,
    ) -> Result<Mesh> {
        let bounded = limits.mesh.max_bytes != usize::MAX;
        // With a host byte budget, keep only the next projection's own input.
        // An intervening operation needs its entire admitted working set.
        let keep = match self.next_use.pop() {
            Some(NextUse::Adjacent) => true,
            Some(NextUse::AfterOtherWork) => !bounded,
            _ => false,
        };
        let frame_bits = frame.map(|row| row.map(f64::to_bits));
        let tri = match self.world.take().filter(|(old, _)| *old == frame_bits) {
            Some((_, tri)) => {
                // The per-operation allowance can shrink as the state grows.
                ctx.bytes(self.world_bytes)?;
                tri
            }
            None => {
                let tri = bake_source(source, frame, ctx)?.triangulate(ctx)?;
                if keep && bounded {
                    self.world_bytes = std::mem::size_of_val(tri.vertices.as_slice())
                        + std::mem::size_of_val(tri.triangles.as_slice())
                        + tri.vertices.iter().map(|v| std::mem::size_of_val(v.weights.as_slice())).sum::<usize>();
                }
                tri
            }
        };
        let mesh = decal.generate(&tri, material, ctx)?;
        if keep {
            self.world = Some((frame_bits, tri));
        }
        Ok(mesh)
    }
}

impl Decal {
    fn generate(&self, tri: &TriangleMesh, material: u32, ctx: &mut Context<'_>) -> Result<Mesh> {
        let n = self.normal;
        let right = unit(cross(self.up, n)).or_else(|_| unit(cross([1., 0., 0.], n)))?;
        let upv = cross(n, right);
        let (hw, hh) = (self.size[0] * 0.5, self.size[1] * 0.5);
        // Clip planes: (normal, offset) keep dot(p, normal) <= offset.
        let local = |p: [f64; 3]| { let d = sub(p, self.center); [dot(d, right), dot(d, upv), dot(d, n)] };
        let planes: [([f64; 3], f64); 6] = [([1., 0., 0.], hw), ([-1., 0., 0.], hw), ([0., 1., 0.], hh), ([0., -1., 0.], hh), ([0., 0., 1.], self.depth), ([0., 0., -1.], self.depth)];
        let mut positions: Vec<[f64; 3]> = Vec::new();
        let mut polygons = Vec::new();
        let mut normals: Vec<[f64; 3]> = Vec::new();
        let mut weld: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
        // Clipping a convex triangle against six planes adds at most six corners.
        let mut poly = Vec::with_capacity(9);
        let mut next = Vec::with_capacity(9);
        let half_size = [hw, hh, self.depth];
        for t in &tri.triangles {
            ctx.checkpoint(1)?;
            let v = t.indices.map(|i| &tri.vertices[i as usize]);
            let p = v.map(|x| x.position);
            let box_points = p.map(local);
            if (0..3).any(|d| box_points.iter().all(|q| q[d] > half_size[d])
                || box_points.iter().all(|q| q[d] < -half_size[d])) {
                continue;
            }
            let face_n = match unit(cross(sub(p[1], p[0]), sub(p[2], p[0]))) { Ok(f) => f, Err(_) => continue };
            if dot(face_n, n) < 0.2 { continue; }
            // Each clipped point carries (local position, world position, normal).
            poly.clear();
            poly.extend((0..3).map(|i| (box_points[i], p[i], v[i].normal)));
            for (pn, po) in planes {
                next.clear();
                for i in 0..poly.len() {
                    let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                    let (da, db) = (dot(a.0, pn) - po, dot(b.0, pn) - po);
                    if da <= 0. { next.push(a); }
                    if (da <= 0.) != (db <= 0.) {
                        let s = da / (da - db);
                        let lerp = |x: [f64; 3], y: [f64; 3]| add(x, mul(sub(y, x), s));
                        next.push((lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2)));
                    }
                }
                std::mem::swap(&mut poly, &mut next);
                if poly.len() < 3 { break; }
            }
            if poly.len() < 3 { continue; }
            let mut ids = Vec::with_capacity(poly.len()); let mut uvs = Vec::with_capacity(poly.len());
            for (l, w, nn) in &poly {
                let nn = unit(*nn).unwrap_or(face_n);
                let q = add(*w, mul(nn, self.offset));
                let k = q.map(|x| (x * 1e5).round() as i64);
                let id = *weld.entry(k).or_insert_with(|| { positions.push(q); normals.push(nn); (positions.len() - 1) as u32 });
                if ids.last() != Some(&id) && ids.first() != Some(&id) { ids.push(id); uvs.push([0.5 + l[0] / self.size[0], 0.5 - l[1] / self.size[1]]); }
            }
            let area = (1..ids.len().saturating_sub(1)).map(|i| length(cross(sub(positions[ids[i] as usize], positions[ids[0] as usize]), sub(positions[ids[i + 1] as usize], positions[ids[0] as usize])))).sum::<f64>();
            if ids.len() >= 3 && area > 1e-10 { polygons.push(Polygon { vertices: ids, uvs, material }); }
        }
        if polygons.is_empty() { return Err(Error::Invalid("decal projection box does not touch any front-facing surface of its target")); }
        admit(positions.len(), polygons.len(), polygons.iter().map(|p| p.vertices.len()).sum(), ctx)?;
        let mut mesh = Mesh::from_polygons(&positions, &polygons, ctx)?;
        let index: std::collections::HashMap<_, usize> = mesh.vertices().iter().enumerate().map(|(i, v)| (v.id, i)).collect();
        let values: Vec<_> = mesh.corners().iter().map(|c| (c.id, Some(normals[index[&c.vertex]]))).collect();
        mesh.set_corner_normals_bulk(&values, ctx)?;
        Ok(mesh)
    }
}

pub(super) fn write_decal(w:&mut Writer,value:&Decal)->Result<()>{w.string(&value.target)?;for item in &value.center{w.f64(*item)?;}for item in &value.normal{w.f64(*item)?;}for item in &value.up{w.f64(*item)?;}for item in &value.size{w.f64(*item)?;}w.f64(value.depth)?;w.f64(value.offset)?;Ok(())}
pub(super) fn read_decal(r:&mut Reader,limits:&Limits)->Result<Decal>{let _=limits;Ok(Decal{target:r.string()?,center:[r.f64()?,r.f64()?,r.f64()?],normal:[r.f64()?,r.f64()?,r.f64()?],up:[r.f64()?,r.f64()?,r.f64()?],size:[r.f64()?,r.f64()?],depth:r.f64()?,offset:r.f64()?})}

impl Decal {
    pub fn check(&self,_limits:&Limits)->Result<()>{
        // The target is resolved by execution.
        finite(self.center)?;finite(self.normal)?;finite(self.up)?;finite(self.size)?;finite([self.depth,self.offset])?;
        unit(self.normal)?;
        if (length(self.normal)-1.).abs()>1e-12{return Err(Error::Invalid("degenerate construction direction"));}
        if self.size.iter().any(|s|*s<=0.)||self.depth<=0.||self.offset<0.||self.offset>=0.1{return Err(Error::Invalid("decal size/depth/offset"));}Ok(())
    }
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    #[test]
    fn decal_normalization_and_binary_roundtrip(){
        use crate::{SceneNode, Transform, Workspace};

        let limits=Limits::default();let literal=Decal{target:"target".into(),center:[0.;3],normal:[0.,0.,1.],up:[0.,1.,0.],size:[1.;2],depth:0.5,offset:0.0015};
        literal.check(&limits).unwrap();let mut w=Writer::new();write_decal(&mut w,&literal).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(read_decal(&mut r,&limits).unwrap(),literal);r.end().unwrap();
        let mut bad=literal.clone();bad.normal=[0.,0.,8.];assert!(bad.check(&limits).is_err());bad=literal.clone();bad.offset=0.1;assert!(bad.check(&limits).is_err());bad=literal;bad.center[0]=f64::NAN;assert!(bad.check(&limits).is_err());

        // Each box cuts a vertical strip out of just one quad, across its diagonal.
        let cells = 24u32;
        let points: Vec<_> = (0..=cells).flat_map(|x| [[x as f64, 0., 0.], [x as f64, 1., 0.]]).collect();
        let quads: Vec<_> = (0..cells).map(|i| Polygon {
            vertices: vec![2*i, 2*i + 2, 2*i + 3, 2*i + 1], uvs: vec![], material: 0,
        }).collect();
        let ctx = &mut Context::default();
        let mut target = Mesh::from_polygons(&points, &quads, ctx).unwrap();
        // Left corners inherit the face normal. Right corners have an authored tilt.
        let split_normals: Vec<_> = target.corners().chunks_exact(4).flat_map(|quad|
            [quad[1].id, quad[2].id].map(|id| (id, Some([0.6, 0., 0.8])))
        ).collect();
        target.set_corner_normals_bulk(&split_normals, ctx).unwrap();
        let ids = target.vertices().iter().map(|v| v.id).collect();
        let frame = |object: &str, parent: Option<&str>, translation, scale| Operation::Scene(SceneOperation::Node {
            object: object.into(),
            node: SceneNode {
                parent: parent.map(str::to_owned),
                transform: Transform { translation, scale, ..Default::default() },
                ..Default::default()
            },
        });
        let decal = |name: &str, cell: u32, z| Operation::Construction(ConstructionOperation {
            object: name.into(),
            kind: Construction::Decal {
                material: 0,
                decal: Decal {
                    target: "target".into(), center: [4. + 2. * f64::from(cell), 1., z],
                    normal: [0., 0., 1.], up: [0., 1., 0.], size: [1., 4.], depth: 0.125, offset: 0.01,
                },
            },
        });
        let mut workspace = Workspace::new(limits).unwrap();
        workspace.insert_mesh("target", target).unwrap();
        workspace.execute(&[
            Operation::Plane { object: "parent".into(), size: [1.; 2] },
            frame("target", Some("parent"), [3.,-1.,1.], [2.,4.,4.]),
        ], None).unwrap();
        let mut translate = crate::transform::IDENTITY_MATRIX;
        translate[2][3] = 0.5;
        workspace.execute(&[
            decal("first", 3, 1.), decal("second", 18, 1.),
            frame("parent", None, [0.,0.,2.], [1.;3]),
            decal("reframed", 3, 3.),
            Operation::Transform { object: "target".into(), vertices: ids, matrix: translate },
            decal("edited", 3, 5.),
        ], None).unwrap();
        for (name, cell, z) in [("first", 3., 1.), ("second", 18., 1.), ("reframed", 3., 3.), ("edited", 3., 5.)] {
            let mesh = workspace.object(name).unwrap();
            assert_eq!(mesh.faces().len(), 2, "{name} should touch only the two triangles of one quad");
            assert_eq!(mesh.vertices().len(), 6, "{name} should weld both ends of the shared diagonal");
            // In unit-cell coordinates, the diagonal is y = 1 - x.
            let strips = [
                [(0.25, 0.), (0.75, 0.), (0.75, 0.25), (0.25, 0.75)],
                [(0.75, 1.), (0.25, 1.), (0.25, 0.75), (0.75, 0.25)],
            ];
            for (face, expected) in mesh.faces().iter().zip(strips) {
                let corners = mesh.face_corners(face.id).unwrap();
                assert_eq!(corners.len(), expected.len(), "{name} clipped polygon");
                for (corner, (x, y)) in corners.iter().zip(expected) {
                    // The inverse scale takes the right normal to (0.6/2, 0, 0.8/4).
                    let tilted = [3. / 13_f64.sqrt(), 0., 2. / 13_f64.sqrt()];
                    let normal = unit([x * tilted[0], 0., (1. - x) + x * tilted[2]]).unwrap();
                    let position = add([3. + 2. * (cell + x), -1. + 4. * y, z], mul(normal, 0.01));
                    assert!(length(sub(mesh.vertex(corner.vertex).unwrap().position, position)) < 1e-12, "{name} corner position and order");
                    assert_eq!(corner.uv, [(x - 0.25) * 2., 1. - y], "{name} corner UV");
                    assert!(length(sub(corner.normal.unwrap(), normal)) < 1e-12, "{name} interpolated world normal");
                }
            }
        }
    }
}
