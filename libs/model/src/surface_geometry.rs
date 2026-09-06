use super::*;
type V3 = [f64; 3];
fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}
fn mul(a: V3, b: f64) -> V3 {
    a.map(|x| x * b)
}
fn dot(a: V3, b: V3) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(super) fn normalize(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    if n > 1e-20 {
        mul(a, 1. / n)
    } else {
        [0., 0., 1.]
    }
}
#[derive(Clone)]
struct Tri {
    p: [V3; 3],
    n: [V3; 3],
    uv: [[f64; 2]; 3],
    ids: [mesh::VertexId; 3],
    material: u32,
}
impl Tri {
    fn point(&self, b: V3) -> V3 {
        add(
            add(mul(self.p[0], b[0]), mul(self.p[1], b[1])),
            mul(self.p[2], b[2]),
        )
    }
    fn normal(&self, b: V3) -> V3 {
        normalize(add(
            add(mul(self.n[0], b[0]), mul(self.n[1], b[1])),
            mul(self.n[2], b[2]),
        ))
    }
    fn uv(&self, b: V3) -> [f64; 2] {
        std::array::from_fn(|i| (0..3).map(|j| self.uv[j][i] * b[j]).sum())
    }
}
struct Node {
    min: V3,
    max: V3,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}
struct Tree {
    triangles: Vec<Tri>,
    nodes: Vec<Node>,
    order: Vec<usize>,
}
impl Tree {
    fn new(mesh: &mesh::Mesh, limits: &Limits, ctx: &mut mesh::Context<'_>) -> Result<Self> {
        let t = mesh.triangulate(ctx)?;
        if t.triangles.len().saturating_mul(1024) > limits.mesh.max_bytes {
            return Err(Error::Budget("surface BVH bytes"));
        }
        let triangles = t
            .triangles
            .iter()
            .map(|tri| {
                let v = tri.indices.map(|i| &t.vertices[i as usize]);
                Tri {
                    p: v.map(|v| v.position),
                    n: v.map(|v| v.normal),
                    uv: v.map(|v| v.uv),
                    ids: v.map(|v| v.source_vertex),
                    material: tri.material,
                }
            })
            .collect::<Vec<_>>();
        if triangles.is_empty() {
            return Err(Error::Invalid("surface requires triangles"));
        }
        let mut tree = Self {
            order: (0..triangles.len()).collect(),
            triangles,
            nodes: Vec::new(),
        };
        tree.build(0, tree.order.len(), ctx)?;
        Ok(tree)
    }
    fn build(&mut self, start: usize, end: usize, ctx: &mut mesh::Context<'_>) -> Result<usize> {
        ctx.checkpoint((end - start) as u64)?;
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for index in &self.order[start..end] {
            for p in self.triangles[*index].p {
                for c in 0..3 {
                    min[c] = min[c].min(p[c]);
                    max[c] = max[c].max(p[c]);
                }
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|a, b| (max[*a] - min[*a]).total_cmp(&(max[*b] - min[*b])))
                .unwrap();
            let tris = &self.triangles;
            self.order[start..end].sort_by(|a, b| {
                let ca = tris[*a].p.iter().map(|p| p[axis]).sum::<f64>();
                let cb = tris[*b].p.iter().map(|p| p[axis]).sum::<f64>();
                ca.total_cmp(&cb).then(a.cmp(b))
            });
            let mid = (start + end) / 2;
            let a = self.build(start, mid, ctx)?;
            let b = self.build(mid, end, ctx)?;
            self.nodes[index].children = Some((a, b));
        }
        Ok(index)
    }
    fn closest(
        &self,
        p: V3,
        max: f64,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<Option<(usize, V3, f64)>> {
        let mut best = max * max;
        let mut found = None;
        let mut stack = vec![0];
        while let Some(n) = stack.pop() {
            ctx.checkpoint(1)?;
            let node = &self.nodes[n];
            let bound = (0..3)
                .map(|i| {
                    let d = (node.min[i] - p[i]).max(0.).max(p[i] - node.max[i]);
                    d * d
                })
                .sum::<f64>();
            if bound > best {
                continue;
            }
            if let Some((a, b)) = node.children {
                stack.push(b);
                stack.push(a);
            } else {
                for id in &self.order[node.start..node.end] {
                    ctx.checkpoint(1)?;
                    let tri = &self.triangles[*id];
                    let bary = closest_bary(p, tri.p);
                    let delta = sub(tri.point(bary), p);
                    let d = dot(delta, delta);
                    if d <= best {
                        best = d;
                        found = Some((*id, bary, d.sqrt()));
                    }
                }
            }
        }
        Ok(found)
    }
    fn ray(
        &self,
        origin: V3,
        direction: V3,
        max: f64,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<Option<f64>> {
        let mut best = max;
        let mut hit = false;
        let mut stack = vec![0];
        while let Some(n) = stack.pop() {
            ctx.checkpoint(1)?;
            let node = &self.nodes[n];
            let (mut near, mut far) = (0f64, best);
            for c in 0..3 {
                if direction[c].abs() < 1e-20 {
                    if origin[c] < node.min[c] || origin[c] > node.max[c] {
                        far = -1.;
                        break;
                    }
                } else {
                    let a = (node.min[c] - origin[c]) / direction[c];
                    let b = (node.max[c] - origin[c]) / direction[c];
                    near = near.max(a.min(b));
                    far = far.min(a.max(b));
                }
            }
            if far < near {
                continue;
            }
            if let Some((a, b)) = node.children {
                stack.push(b);
                stack.push(a)
            } else {
                for id in &self.order[node.start..node.end] {
                    ctx.checkpoint(1)?;
                    if let Some(t) = ray_tri(origin, direction, self.triangles[*id].p) {
                        if t > 1e-7 && t < best {
                            best = t;
                            hit = true;
                        }
                    }
                }
            }
        }
        Ok(hit.then_some(best))
    }
}
fn ray_tri(o: V3, d: V3, p: [V3; 3]) -> Option<f64> {
    let e1 = sub(p[1], p[0]);
    let e2 = sub(p[2], p[0]);
    let h = cross(d, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-14 {
        return None;
    }
    let s = sub(o, p[0]);
    let u = dot(s, h) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) / det;
    if v < 0. || u + v > 1. {
        return None;
    }
    let t = dot(e2, q) / det;
    (t >= 0.).then_some(t)
}
fn closest_bary(p: V3, t: [V3; 3]) -> V3 {
    let [a, b, c] = t;
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0. && d2 <= 0. {
        return [1., 0., 0.];
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0. && d4 <= d3 {
        return [0., 1., 0.];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0. && d1 >= 0. && d3 <= 0. {
        let v = d1 / (d1 - d3);
        return [1. - v, v, 0.];
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0. && d5 <= d6 {
        return [0., 0., 1.];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0. && d2 >= 0. && d6 <= 0. {
        let w = d2 / (d2 - d6);
        return [1. - w, 0., w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0. && (d4 - d3) >= 0. && (d5 - d6) >= 0. {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0., 1. - w, w];
    }
    let denom = va + vb + vc;
    if denom.abs() < 1e-30 {
        return [1., 0., 0.];
    }
    let v = vb / denom;
    let w = vc / denom;
    [1. - v - w, v, w]
}
fn raster(
    tri: &Tri,
    width: u32,
    height: u32,
    ctx: &mut mesh::Context<'_>,
    mut visit: impl FnMut(usize, V3, &mut mesh::Context<'_>) -> Result<()>,
) -> Result<()> {
    if tri
        .uv
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || *v < -1e-9 || *v > 1. + 1e-9)
    {
        return Err(Error::Invalid("surface raster requires UVs in 0..1"));
    }
    let a = tri.uv[0];
    let b = tri.uv[1];
    let c = tri.uv[2];
    let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if det.abs() < 1e-16 {
        return Ok(());
    }
    let minx = (tri.uv.iter().map(|p| p[0]).fold(1., f64::min) * width as f64)
        .floor()
        .max(0.) as u32;
    let maxx = (tri.uv.iter().map(|p| p[0]).fold(0., f64::max) * width as f64)
        .ceil()
        .min(width as f64) as u32;
    let miny = (tri.uv.iter().map(|p| p[1]).fold(1., f64::min) * height as f64)
        .floor()
        .max(0.) as u32;
    let maxy = (tri.uv.iter().map(|p| p[1]).fold(0., f64::max) * height as f64)
        .ceil()
        .min(height as f64) as u32;
    for y in miny..maxy {
        ctx.checkpoint((maxx - minx) as u64)?;
        for x in minx..maxx {
            let u = (x as f64 + 0.5) / width as f64 - a[0];
            let v = (y as f64 + 0.5) / height as f64 - a[1];
            let w1 = (u * (c[1] - a[1]) - v * (c[0] - a[0])) / det;
            let w2 = ((b[0] - a[0]) * v - (b[1] - a[1]) * u) / det;
            let w0 = 1. - w1 - w2;
            if w0 >= -1e-10 && w1 >= -1e-10 && w2 >= -1e-10 {
                visit((y * width + x) as usize, [w0, w1, w2], ctx)?;
            }
        }
    }
    Ok(())
}
fn frame(tri: &Tri, n: V3) -> (V3, V3) {
    let dp1 = sub(tri.p[1], tri.p[0]);
    let dp2 = sub(tri.p[2], tri.p[0]);
    let du1 = tri.uv[1][0] - tri.uv[0][0];
    let du2 = tri.uv[2][0] - tri.uv[0][0];
    let dv1 = tri.uv[1][1] - tri.uv[0][1];
    let dv2 = tri.uv[2][1] - tri.uv[0][1];
    let d = du1 * dv2 - du2 * dv1;
    if d.abs() < 1e-16 {
        let axis = if n[2].abs() < 0.9 {
            [0., 0., 1.]
        } else {
            [0., 1., 0.]
        };
        let t = normalize(cross(axis, n));
        return (t, cross(n, t));
    }
    let raw = mul(sub(mul(dp1, dv2), mul(dp2, dv1)), 1. / d);
    let t = normalize(sub(raw, mul(n, dot(n, raw))));
    let rawb = mul(sub(mul(dp2, du1), mul(dp1, du2)), 1. / d);
    let sign = if dot(cross(n, t), rawb) < 0. { -1. } else { 1. };
    (t, mul(cross(n, t), sign))
}
fn sample(image: &RgbaImage, uv: [f64; 2], ch: SurfaceChannel) -> [f64; 4] {
    let x = (uv[0].rem_euclid(1.) * image.width as f64).floor() as usize;
    let y = (uv[1].rem_euclid(1.) * image.height as f64).floor() as usize;
    let i = (y * image.width as usize + x) * 4;
    decode(ch, image.pixels[i..i + 4].try_into().unwrap())
}

pub(super) fn project(
    state: &mut SurfaceState,
    op: &SurfaceOperation,
    objects: &BTreeMap<String, mesh::Mesh>,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<()> {
    let SurfaceOperation::ProjectedStroke {
        object,
        material,
        channel,
        layer,
        origin,
        direction,
        radius,
        depth,
        hardness,
        opacity,
        color,
        mask,
    } = op
    else {
        unreachable!()
    };
    brush_validate(*radius, *hardness, *opacity, color)?;
    if origin.iter().chain(direction).any(|v| !v.is_finite())
        || dot(*direction, *direction) < 1e-20
        || !depth.is_finite()
        || *depth <= 0.
    {
        return Err(Error::Invalid("projected brush axis/depth"));
    }
    let mesh = objects
        .get(object)
        .ok_or(Error::Invalid("projected brush object"))?;
    let tree = Tree::new(mesh, limits, ctx)?;
    let d = normalize(*direction);
    let target = state.layer(*material, *channel, layer)?;
    let (w, h) = (target.image.width, target.image.height);
    if target
        .image
        .pixels
        .len()
        .saturating_mul(4)
        .saturating_add(tree.triangles.len() * 1024)
        > limits.mesh.max_bytes
    {
        return Err(Error::Budget("projected brush bytes"));
    }
    let mut coverage = vec![0f64; w as usize * h as usize];
    for tri in tree.triangles.iter().filter(|t| t.material == *material) {
        raster(tri, w, h, ctx, |i, b, ctx| {
            let p = tri.point(b);
            let from = sub(p, *origin);
            let along = dot(from, d);
            if along < 0. || along > *depth || dot(tri.normal(b), d) >= 0. {
                return Ok(());
            }
            let radial = dot(sub(from, mul(d, along)), sub(from, mul(d, along))).sqrt();
            let alpha = falloff(radial, *radius, *hardness);
            if alpha > 0. {
                let ray_origin = sub(p, mul(d, along + 1e-5));
                if tree.ray(ray_origin, d, along, ctx)?.is_none() {
                    coverage[i] = coverage[i].max(alpha);
                }
            }
            Ok(())
        })?;
    }
    for (i, a) in coverage.into_iter().enumerate() {
        ctx.checkpoint(1)?;
        paint_pixel(target, *channel, i, *color, a * opacity, *mask);
    }
    target.mips.clear();
    target.pattern = None;
    target.derived = None;
    Ok(())
}
pub(super) fn bake(
    state: &mut SurfaceState,
    op: &SurfaceOperation,
    objects: &BTreeMap<String, mesh::Mesh>,
    legacy: &BTreeMap<u32, Material>,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<()> {
    let SurfaceOperation::Bake {
        source,
        target,
        material,
        channel,
        layer,
        width,
        height,
        max_distance,
        ao_samples,
        ao_distance,
        dilation,
    } = op
    else {
        unreachable!()
    };
    if !max_distance.is_finite()
        || *max_distance <= 0.
        || !ao_distance.is_finite()
        || *ao_distance <= 0.
        || *ao_samples == 0
        || *ao_samples > 64
        || *dilation > 64
    {
        return Err(Error::Invalid("surface bake settings"));
    }
    let mut image = RgbaImage::new(*width, *height, [0; 4], limits)?;
    let high = Tree::new(
        objects.get(source).ok_or(Error::Invalid("bake source"))?,
        limits,
        ctx,
    )?;
    let low = Tree::new(
        objects.get(target).ok_or(Error::Invalid("bake target"))?,
        limits,
        ctx,
    )?;
    if (high.triangles.len() + low.triangles.len())
        .saturating_mul(1024)
        .saturating_add(image.memory_bytes() * 8)
        .saturating_add(state.memory_bytes())
        > limits.mesh.max_bytes
    {
        return Err(Error::Budget("surface bake bytes"));
    }
    let mut materials = BTreeMap::new();
    let mut images = BTreeMap::new();
    for tri in &high.triangles {
        if materials.contains_key(&tri.material) {
            continue;
        }
        let mat = if let Some(m) = state.materials.get(&tri.material) {
            m.clone()
        } else {
            SurfaceMaterial::from_legacy(
                legacy
                    .get(&tri.material)
                    .ok_or(Error::Invalid("bake source material"))?,
                limits,
            )?
        };
        let map = mat.flatten(*channel, limits, ctx)?;
        if let Some(map) = map {
            images.insert(tri.material, map);
        }
        materials.insert(tri.material, mat);
        if images
            .values()
            .map(RgbaImage::memory_bytes)
            .sum::<usize>()
            .saturating_add(
                materials
                    .values()
                    .map(SurfaceMaterial::memory_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(state.memory_bytes())
            .saturating_add(image.memory_bytes() * 8)
            .saturating_add((high.triangles.len() + low.triangles.len()) * 1024)
            > limits.mesh.max_bytes
        {
            return Err(Error::Budget("bake material working bytes"));
        }
    }
    let emissive_bake_strength = materials
        .values()
        .map(|m| m.emissive.iter().copied().fold(0f64, f64::max) * m.emissive_strength)
        .fold(1f64, f64::max);
    let mut covered = vec![false; *width as usize * *height as usize];
    let mut hits = 0usize;
    for tri in low.triangles.iter().filter(|t| t.material == *material) {
        raster(tri, *width, *height, ctx, |i, b, ctx| {
            // Shared triangle edges visit the same texel; matching hits are harmless.
            let p = tri.point(b);
            let Some((index, hb, _)) = high.closest(p, *max_distance, ctx)? else {
                return Ok(());
            };
            let ht = &high.triangles[index];
            let mat = &materials[&ht.material];
            let hn = ht.normal(hb);
            let uv = ht.uv(hb);
            let mut value = images
                .get(&ht.material)
                .map(|im| sample(im, uv, *channel))
                .unwrap_or_else(|| decode(*channel, channel.neutral()));
            match channel {
                SurfaceChannel::Normal => {
                    let mut world = hn;
                    if images.contains_key(&ht.material) {
                        let (t, b) = frame(ht, hn);
                        let mapped = normalize([
                            (value[0] * 2. - 1.) * mat.normal_scale,
                            (value[1] * 2. - 1.) * mat.normal_scale,
                            value[2] * 2. - 1.,
                        ]);
                        world = normalize(add(
                            add(mul(t, mapped[0]), mul(b, mapped[1])),
                            mul(hn, mapped[2]),
                        ));
                    }
                    let ln = tri.normal(b);
                    let (t, bt) = frame(tri, ln);
                    value = [
                        dot(world, t) * 0.5 + 0.5,
                        dot(world, bt) * 0.5 + 0.5,
                        dot(world, ln) * 0.5 + 0.5,
                        1.,
                    ];
                }
                SurfaceChannel::Occlusion => {
                    let hp = ht.point(hb);
                    let (t, bt) = frame(ht, hn);
                    let origin = add(hp, mul(hn, 1e-5));
                    let mut blocked = 0u32;
                    for k in 0..*ao_samples {
                        let u = (k as f64 + 0.5) / *ao_samples as f64;
                        let angle = k as f64 * 2.399963229728653;
                        let r = u.sqrt();
                        let ray = add(
                            add(mul(t, r * angle.cos()), mul(bt, r * angle.sin())),
                            mul(hn, (1. - u).sqrt()),
                        );
                        if high.ray(origin, ray, *ao_distance, ctx)?.is_some() {
                            blocked += 1;
                        }
                    }
                    let ao = 1. - blocked as f64 / *ao_samples as f64;
                    value = [ao, ao, ao, 1.];
                }
                SurfaceChannel::BaseColor => {
                    for c in 0..4 {
                        let vc = (0..3)
                            .map(|j| {
                                state
                                    .vertex_colors
                                    .get(&(source.clone(), ht.ids[j]))
                                    .map_or(1., |color| color[c])
                                    * hb[j]
                            })
                            .sum::<f64>();
                        value[c] *= mat.base_color[c] * vc;
                    }
                }
                SurfaceChannel::MetallicRoughness => {
                    value[1] *= mat.roughness;
                    value[2] *= mat.metallic;
                    value[3] = 1.;
                }
                SurfaceChannel::Emissive => {
                    for c in 0..3 {
                        let e = if images.contains_key(&ht.material) {
                            value[c]
                        } else {
                            1.
                        };
                        value[c] =
                            e * mat.emissive[c] * mat.emissive_strength / emissive_bake_strength;
                    }
                    value[3] = 1.;
                }
            }
            let encoded = encode(*channel, value);
            if covered[i] && image.pixels[i * 4..i * 4 + 4] != encoded {
                return Err(Error::Invalid(
                    "overlapping target UVs bake conflicting attributes",
                ));
            }
            if !covered[i] {
                hits += 1;
            }
            covered[i] = true;
            image.pixels[i * 4..i * 4 + 4].copy_from_slice(&encoded);
            Ok(())
        })?;
    }
    if hits == 0 {
        return Err(Error::Invalid(
            "bake found no source surface within distance",
        ));
    }
    image.dilate(*dilation, false, limits, ctx)?;
    let m = state.ensure(*material, legacy, limits)?;
    match channel {
        SurfaceChannel::BaseColor => m.base_color = [1.; 4],
        SurfaceChannel::MetallicRoughness => {
            m.metallic = 1.;
            m.roughness = 1.;
        }
        SurfaceChannel::Normal => m.normal_scale = 1.,
        SurfaceChannel::Occlusion => m.occlusion_strength = 1.,
        SurfaceChannel::Emissive => {
            m.emissive = [1.; 3];
            m.emissive_strength = emissive_bake_strength;
        }
    }
    let layers = m.channels.entry(*channel).or_default();
    let value = SurfaceLayer::new(layer.clone(), image);
    if let Some(old) = layers.iter_mut().find(|l| l.id == *layer) {
        *old = value
    } else {
        layers.push(value)
    }
    Ok(())
}
fn triangle_color(state: &SurfaceState, name: &str, triangle: &Tri, bary: V3) -> [f64; 4] {
    std::array::from_fn(|channel| {
        (0..3).map(|corner| {
            state.vertex_colors.get(&(name.to_owned(), triangle.ids[corner]))
                .map_or(1., |color| color[channel]) * bary[corner]
        }).sum::<f64>().clamp(0., 1.)
    })
}

pub(super) fn transfer_vertex_colors(
    state: &mut SurfaceState,
    source_name: &str,
    source: &mesh::Mesh,
    target_name: &str,
    target: &mesh::Mesh,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<()> {
    ctx.checkpoint(state.vertex_colors.len() as u64)?;
    if !state.vertex_colors.keys().any(|(name, _)| name == source_name) {
        return Ok(());
    }
    // Include the pending color map and a conservative triangulation/BVH
    // allowance before allocating. Retained material bitmaps are not cloned.
    let color_bytes = target.vertices().len().saturating_mul(limits.max_name_bytes + 96);
    if state.vertex_colors.len().saturating_add(target.vertices().len()) > limits.mesh.max_vertices
        || state.memory_bytes().saturating_add(color_bytes) > limits.max_source_bytes {
        return Err(Error::Budget("derived vertex colors"));
    }
    let scratch = color_bytes
        .saturating_add(source.corners().len().saturating_mul(1024));
    if state.memory_bytes().saturating_add(scratch) > limits.mesh.max_bytes {
        return Err(Error::Budget("derived vertex color transfer bytes"));
    }
    let mut colors = BTreeMap::new();
    let mut tree = None;
    for vertex in target.vertices() {
        ctx.checkpoint(1)?;
        // Decimation may keep an ID while moving its position. Only an
        // unchanged vertex keeps its exact paint; moved vertices sample the
        // original surface so a collapse also blends the color field.
        let color = if source.vertex(vertex.id).is_some_and(|old| old.position == vertex.position) {
            state.vertex_colors.get(&(source_name.to_owned(), vertex.id)).copied().unwrap_or([1.; 4])
        } else {
            if tree.is_none() { tree = Some(Tree::new(source, limits, ctx)?); }
            let tree = tree.as_ref().unwrap();
            let (triangle, bary, _) = tree.closest(vertex.position, f64::MAX.sqrt(), ctx)?
                .ok_or(Error::Invalid("derived paint has no source surface"))?;
            triangle_color(state, source_name, &tree.triangles[triangle], bary)
        };
        colors.insert((target_name.to_owned(), vertex.id), color);
    }
    ctx.checkpoint(0)?;
    state.vertex_colors.extend(colors);
    Ok(())
}

pub(super) fn reconcile(
    state: &mut SurfaceState,
    before: &BTreeMap<String, mesh::Mesh>,
    after: &BTreeMap<String, mesh::Mesh>,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<()> {
    state.validate(limits, ctx)?;
    if state.vertex_colors.is_empty() {
        return Ok(());
    }
    let mut colors = BTreeMap::new();
    for (name, mesh) in after {
        if !state.vertex_colors.keys().any(|(n, _)| n == name) {
            continue;
        }
        let mut tree = None;
        for vertex in mesh.vertices() {
            ctx.checkpoint(1)?;
            let key = (name.clone(), vertex.id);
            if let Some(c) = state.vertex_colors.get(&key) {
                colors.insert(key, *c);
                continue;
            }
            let Some(old) = before.get(name) else {
                continue;
            };
            if old.faces().is_empty() {
                continue;
            }
            if tree.is_none() {
                tree = Some(Tree::new(old, limits, ctx)?)
            }
            if let Some((id, b, _)) =
                tree.as_ref()
                    .unwrap()
                    .closest(vertex.position, f64::MAX.sqrt(), ctx)?
            {
                let tri = &tree.as_ref().unwrap().triangles[id];
                let color = triangle_color(state, name, tri, b);
                colors.insert(key, color);
            }
        }
    }
    if colors
        .len()
        .saturating_mul(limits.max_name_bytes + 96)
        .saturating_add(state.memory_bytes())
        > limits.mesh.max_bytes
    {
        return Err(Error::Budget("vertex color transfer bytes"));
    }
    ctx.checkpoint(0)?;
    state.vertex_colors = colors;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_paint_interpolates_moved_stable_vertices_without_changing_source() {
        let limits = Limits::default();
        let mut ctx = mesh::Context::default();
        let source = mesh::Mesh::from_polygons(&[[0.,0.,0.],[1.,0.,0.],[1.,1.,0.],[0.,1.,0.]],
            &[mesh::Polygon { vertices: vec![0,1,2,3], uvs: vec![[0.,0.],[1.,0.],[1.,1.],[0.,1.]], material: 0 }], &mut ctx).unwrap();
        let mut target = source.clone();
        let ids = target.vertices().iter().map(|vertex| vertex.id).collect::<Vec<_>>();
        let frame = crate::Transform { translation: [0.25,0.25,0.], scale: [0.5,0.5,1.], ..Default::default() }.matrix().unwrap();
        target.transform(&ids, frame, &mut ctx).unwrap();
        let mut state = SurfaceState::default();
        for vertex in source.vertices() {
            state.vertex_colors.insert(("source".into(),vertex.id),[vertex.position[0],vertex.position[1],0.,1.]);
        }
        let before = state.clone();
        transfer_vertex_colors(&mut state,"source",&source,"derived",&target,&limits,&mut ctx).unwrap();
        for vertex in target.vertices() {
            let color = state.vertex_colors[&("derived".into(),vertex.id)];
            assert!((color[0]-vertex.position[0]).abs()<1e-9);
            assert!((color[1]-vertex.position[1]).abs()<1e-9);
        }
        for (key,color) in &before.vertex_colors { assert_eq!(state.vertex_colors[key],*color); }
        let successful = state.clone();
        let mut cancelled = mesh::Context::new(limits.mesh.clone(),Some(&||true));
        assert!(transfer_vertex_colors(&mut state,"source",&source,"cancelled",&target,&limits,&mut cancelled).is_err());
        assert_eq!(state,successful);
        let mut tiny = limits.clone(); tiny.mesh.max_bytes = state.memory_bytes();
        assert!(transfer_vertex_colors(&mut state,"source",&source,"budget",&target,&tiny,&mut ctx).is_err());
        assert_eq!(state,successful);
    }
}
