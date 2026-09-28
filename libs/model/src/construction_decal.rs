//! Projected mesh decals: the part of a target surface inside a projection box
//! becomes a thin new object floating just above it, with 0..1 UVs across the
//! decal. Badges, vents, text, panel lines and wear marks are materials on
//! such decals (usually alpha-masked), so they never disturb the target's own
//! UVs or tiling textures.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Decal { pub target: String, pub center: [f64; 3], pub normal: [f64; 3], pub up: [f64; 3], pub size: [f64; 2], pub depth: f64, pub offset: f64 }

impl Decal {
    pub(super) fn parse(v: &Value) -> Result<Self> {
        fields(v, &["op", "object", "target", "center", "normal", "up", "size", "depth", "offset", "material"])?;
        let size: [f64; 2] = array(need(v, "size")?)?;
        let normal = unit(array(need(v, "normal")?)?)?;
        let up = v.get("up").map(array).transpose()?.unwrap_or([0., 1., 0.]);
        let depth = v.get("depth").map(crate::service::float).transpose()?.unwrap_or(size[0].max(size[1]) * 0.5);
        let offset = v.get("offset").map(crate::service::float).transpose()?.unwrap_or(0.0015);
        if size.iter().any(|s| !(s.is_finite() && *s > 0.)) || !(depth.is_finite() && depth > 0.) || !(offset.is_finite() && offset >= 0. && offset < 0.1) {
            return Err(Error::Invalid("decal size/depth/offset"));
        }
        Ok(Decal { target: text(v, "target")?.into(), center: array(need(v, "center")?)?, normal, up, size, depth, offset })
    }
    pub(super) fn value(&self) -> Vec<(&'static str, Value)> {
        let v3 = |p: [f64; 3]| Value::Arr(p.iter().copied().map(Value::F64).collect());
        vec![("target", json::s(&self.target)), ("center", v3(self.center)), ("normal", v3(self.normal)), ("up", v3(self.up)),
            ("size", Value::Arr(vec![Value::F64(self.size[0]), Value::F64(self.size[1])])), ("depth", Value::F64(self.depth)), ("offset", Value::F64(self.offset))]
    }
    pub(super) fn generate(&self, target: &Mesh, material: u32, ctx: &mut Context<'_>) -> Result<Mesh> {
        let n = self.normal;
        let right = unit(cross(self.up, n)).or_else(|_| unit(cross([1., 0., 0.], n)))?;
        let upv = cross(n, right);
        let (hw, hh) = (self.size[0] * 0.5, self.size[1] * 0.5);
        let tri = target.triangulate(ctx)?;
        // Clip planes: (normal, offset) keep dot(p, normal) <= offset.
        let local = |p: [f64; 3]| { let d = sub(p, self.center); [dot(d, right), dot(d, upv), dot(d, n)] };
        let planes: [([f64; 3], f64); 6] = [([1., 0., 0.], hw), ([-1., 0., 0.], hw), ([0., 1., 0.], hh), ([0., -1., 0.], hh), ([0., 0., 1.], self.depth), ([0., 0., -1.], self.depth)];
        let mut positions: Vec<[f64; 3]> = Vec::new();
        let mut polygons = Vec::new();
        let mut normals: Vec<[f64; 3]> = Vec::new();
        let mut weld: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
        for t in &tri.triangles {
            ctx.checkpoint(1)?;
            let v = t.indices.map(|i| &tri.vertices[i as usize]);
            let p = v.map(|x| x.position);
            let face_n = match unit(cross(sub(p[1], p[0]), sub(p[2], p[0]))) { Ok(f) => f, Err(_) => continue };
            if dot(face_n, n) < 0.2 { continue; }
            // Each clipped point carries (local position, world position, normal).
            let mut poly: Vec<([f64; 3], [f64; 3], [f64; 3])> = (0..3).map(|i| (local(p[i]), p[i], v[i].normal)).collect();
            for (pn, po) in planes {
                let mut next = Vec::new();
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
                poly = next;
                if poly.len() < 3 { break; }
            }
            if poly.len() < 3 { continue; }
            let mut ids = Vec::new(); let mut uvs = Vec::new();
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
