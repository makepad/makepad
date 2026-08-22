//! UV chart projection + texture bake, ported from trellis.cpp
//! uv_chart_project (uv_bake.cpp): normal-clustered planar charts grown on
//! the face graph (gate: dot(seed normal, face normal) >= 0.7), per-chart
//! planar basis + projected bbox, shelf packing with a bisected global
//! scale, depth-tested rasterization along each chart's seed normal, texels
//! shaded by a caller-provided sampler (e.g. trilinear over a sparse voxel
//! PBR volume), then multi-source BFS dilation into the gutters.
//!
//! Deterministic and O(F log F) — no xatlas. Charts split vertices per
//! chart (a vertex on a chart border appears once per chart), so the output
//! has its own vertex/uv/index buffers.

/// One baked mesh: positions (input space), UVs in [0,1], triangle indices,
/// and two RGBA8 atlases (glTF convention: base color, and
/// metallic-roughness with G=roughness, B=metallic).
pub struct BakedMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// For every output vertex, the input vertex it was split from — lets
    /// callers carry pre-split attributes (e.g. smooth normals) through the
    /// per-chart vertex duplication.
    pub source_vertex: Vec<u32>,
    pub base_rgba: Vec<u8>,
    pub mr_rgba: Vec<u8>,
    pub size: usize,
}

impl BakedMesh {
    pub fn ok(&self) -> bool {
        self.size > 0 && !self.indices.is_empty()
    }
}

/// Sample at a surface position: returns 6 channels in 0..=1
/// (base RGB, metallic, roughness, alpha) or None where no data exists
/// (left for dilation).
pub type BakeSampler<'a> = &'a (dyn Fn([f32; 3]) -> Option<[f32; 6]> + Sync);

#[inline]
fn ekey(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

/// Multi-source BFS dilation: every unwritten texel takes the color of its
/// nearest written one (4-neighborhood flood).
fn dilate_full(base: &mut [u8], mr: &mut [u8], mask: &mut [u8], t: usize) {
    let mut queue: Vec<u32> = Vec::with_capacity(t * t / 4);
    for i in 0..t * t {
        if mask[i] != 0 {
            queue.push(i as u32);
        }
    }
    let mut qi = 0usize;
    while qi < queue.len() {
        let cur = queue[qi] as usize;
        qi += 1;
        let (x, y) = (cur % t, cur / t);
        let neighbors = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ];
        for (xx, yy) in neighbors {
            if xx >= t || yy >= t {
                continue;
            }
            let s = yy * t + xx;
            if mask[s] != 0 {
                continue;
            }
            for c in 0..4 {
                base[4 * s + c] = base[4 * cur + c];
                mr[4 * s + c] = mr[4 * cur + c];
            }
            mask[s] = 1;
            queue.push(s as u32);
        }
    }
}

/// Chart-project + bake. `sample` shades texels at interpolated surface
/// positions; texels whose sampler returns None stay empty and are filled by
/// dilation from their chart neighbors.
pub fn uv_chart_bake(
    positions: &[[f32; 3]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
) -> BakedMesh {
    let t = texsize;
    let f = indices.len() / 3;
    let v = positions.len();
    let mut out = BakedMesh {
        positions: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        source_vertex: Vec::new(),
        base_rgba: Vec::new(),
        mr_rgba: Vec::new(),
        size: 0,
    };
    if f == 0 || t == 0 {
        return out;
    }

    // face normals
    let mut fnorm = vec![[0f32; 3]; f];
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if l > 1e-20 {
            fnorm[fi] = [n[0] / l, n[1] / l, n[2] / l];
        }
    }

    // undirected edge -> up to 2 incident faces (sort-based)
    let mut ef: Vec<(u64, u32)> = Vec::with_capacity(f * 3);
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        ef.push((ekey(tri[0], tri[1]), fi as u32));
        ef.push((ekey(tri[1], tri[2]), fi as u32));
        ef.push((ekey(tri[2], tri[0]), fi as u32));
    }
    ef.sort_unstable();
    // face adjacency via shared edges (first two faces per edge)
    let mut fadj: Vec<Vec<u32>> = vec![Vec::new(); f];
    {
        let mut i = 0;
        while i < ef.len() {
            let k = ef[i].0;
            let mut j = i + 1;
            while j < ef.len() && ef[j].0 == k {
                j += 1;
            }
            if j - i >= 2 {
                let f0 = ef[i].1;
                let f1 = ef[i + 1].1;
                fadj[f0 as usize].push(f1);
                fadj[f1 as usize].push(f0);
            }
            i = j;
        }
    }

    // region growing gated on deviation from the SEED normal
    let mut fchart = vec![u32::MAX; f];
    let mut charts: Vec<Vec<u32>> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for f0 in 0..f {
        if fchart[f0] != u32::MAX {
            continue;
        }
        let cid = charts.len() as u32;
        charts.push(Vec::new());
        let ns = fnorm[f0];
        stack.push(f0 as u32);
        fchart[f0] = cid;
        while let Some(fi) = stack.pop() {
            charts[cid as usize].push(fi);
            for &g in &fadj[fi as usize] {
                if fchart[g as usize] != u32::MAX {
                    continue;
                }
                let ng = fnorm[g as usize];
                if ns[0] * ng[0] + ns[1] * ng[1] + ns[2] * ng[2] < 0.7 {
                    continue;
                }
                fchart[g as usize] = cid;
                stack.push(g);
            }
        }
    }

    // per-chart planar basis from the seed normal + projected bbox
    struct Chart {
        bu: [f32; 3],
        bv: [f32; 3],
        umin: f32,
        vmin: f32,
        umax: f32,
        vmax: f32,
        ox: f32,
        oy: f32,
    }
    let mut cs: Vec<Chart> = Vec::with_capacity(charts.len());
    for chart in &charts {
        let n = fnorm[chart[0] as usize];
        let up = if n[2].abs() > 0.9 {
            [1.0f32, 0.0, 0.0]
        } else {
            [0.0f32, 0.0, 1.0]
        };
        let mut bu = [
            up[1] * n[2] - up[2] * n[1],
            up[2] * n[0] - up[0] * n[2],
            up[0] * n[1] - up[1] * n[0],
        ];
        let l = (bu[0] * bu[0] + bu[1] * bu[1] + bu[2] * bu[2]).sqrt();
        if l > 1e-20 {
            bu = [bu[0] / l, bu[1] / l, bu[2] / l];
        } else {
            bu = [1.0, 0.0, 0.0];
        }
        let bv = [
            n[1] * bu[2] - n[2] * bu[1],
            n[2] * bu[0] - n[0] * bu[2],
            n[0] * bu[1] - n[1] * bu[0],
        ];
        let mut c = Chart {
            bu,
            bv,
            umin: f32::MAX,
            vmin: f32::MAX,
            umax: f32::MIN,
            vmax: f32::MIN,
            ox: 0.0,
            oy: 0.0,
        };
        for &fi in chart {
            for &vi in &indices[fi as usize * 3..fi as usize * 3 + 3] {
                let p = positions[vi as usize];
                let u = p[0] * bu[0] + p[1] * bu[1] + p[2] * bu[2];
                let w = p[0] * bv[0] + p[1] * bv[1] + p[2] * bv[2];
                c.umin = c.umin.min(u);
                c.umax = c.umax.max(u);
                c.vmin = c.vmin.min(w);
                c.vmax = c.vmax.max(w);
            }
        }
        cs.push(c);
    }

    // shelf packing at a bisected global scale
    let pad = 2.0f32;
    let mut order: Vec<usize> = (0..cs.len()).collect();
    let total: f32 = cs
        .iter()
        .map(|c| (c.umax - c.umin) * (c.vmax - c.vmin))
        .sum();
    let mut scale = if total > 0.0 {
        ((t * t) as f32 * 0.72 / total).sqrt()
    } else {
        1.0
    };
    order.sort_by(|&a, &b| {
        let ha = cs[a].vmax - cs[a].vmin;
        let hb = cs[b].vmax - cs[b].vmin;
        hb.partial_cmp(&ha).unwrap_or(std::cmp::Ordering::Equal)
    });
    let tf = t as f32;
    loop {
        let mut fits = true;
        let (mut x, mut y, mut shelf) = (pad, pad, 0.0f32);
        for &ci in &order {
            let w = (cs[ci].umax - cs[ci].umin) * scale + 2.0 * pad;
            let h = (cs[ci].vmax - cs[ci].vmin) * scale + 2.0 * pad;
            if w > tf || h > tf {
                fits = false;
                break;
            }
            if x + w > tf {
                x = pad;
                y += shelf;
                shelf = 0.0;
            }
            if y + h > tf {
                fits = false;
                break;
            }
            cs[ci].ox = x + pad;
            cs[ci].oy = y + pad;
            x += w;
            shelf = shelf.max(h);
        }
        if fits || scale <= 1e-3 {
            break;
        }
        scale *= 0.92;
    }

    // emit per-chart vertices/uvs/faces
    let mut vstamp = vec![u32::MAX; v];
    let mut vlocal = vec![0u32; v];
    let mut fchart_out: Vec<u32> = Vec::with_capacity(f);
    for (cid, chart) in charts.iter().enumerate() {
        let c = &cs[cid];
        for &fi in chart {
            let mut lidx = [0u32; 3];
            for (j, &vi) in indices[fi as usize * 3..fi as usize * 3 + 3].iter().enumerate() {
                if vstamp[vi as usize] != cid as u32 {
                    vstamp[vi as usize] = cid as u32;
                    vlocal[vi as usize] = (out.positions.len()) as u32;
                    let p = positions[vi as usize];
                    out.positions.push(p);
                    out.source_vertex.push(vi);
                    let u = p[0] * c.bu[0] + p[1] * c.bu[1] + p[2] * c.bu[2];
                    let w = p[0] * c.bv[0] + p[1] * c.bv[1] + p[2] * c.bv[2];
                    out.uvs.push([
                        (c.ox + (u - c.umin) * scale) / tf,
                        (c.oy + (w - c.vmin) * scale) / tf,
                    ]);
                }
                lidx[j] = vlocal[vi as usize];
            }
            out.indices.extend_from_slice(&lidx);
            fchart_out.push(cid as u32);
        }
    }

    // depth-tested raster along each chart's seed normal (threaded by rows
    // of faces would race the z-buffer; the crib is serial and fast enough)
    out.size = t;
    out.base_rgba = vec![0u8; t * t * 4];
    out.mr_rgba = vec![0u8; t * t * 4];
    let mut mask = vec![0u8; t * t];
    let mut zbuf = vec![f32::MIN; t * t];
    let u8c = |x: f32| -> u8 { (x * 255.0).clamp(0.0, 255.0) as u8 };
    let fo = out.indices.len() / 3;
    for fi in 0..fo {
        let idx = [
            out.indices[fi * 3] as usize,
            out.indices[fi * 3 + 1] as usize,
            out.indices[fi * 3 + 2] as usize,
        ];
        let n = fnorm[charts[fchart_out[fi] as usize][0] as usize];
        let mut px = [0f32; 3];
        let mut py = [0f32; 3];
        let mut pz = [0f32; 3];
        for j in 0..3 {
            px[j] = out.uvs[idx[j]][0] * tf;
            py[j] = out.uvs[idx[j]][1] * tf;
            let p = out.positions[idx[j]];
            pz[j] = p[0] * n[0] + p[1] * n[1] + p[2] * n[2];
        }
        let x0 = (px[0].min(px[1]).min(px[2]).floor() as i64).max(0) as usize;
        let x1 = (px[0].max(px[1]).max(px[2]).ceil() as i64).min(t as i64 - 1) as usize;
        let y0 = (py[0].min(py[1]).min(py[2]).floor() as i64).max(0) as usize;
        let y1 = (py[0].max(py[1]).max(py[2]).ceil() as i64).min(t as i64 - 1) as usize;
        let d = (py[1] - py[2]) * (px[0] - px[2]) + (px[2] - px[1]) * (py[0] - py[2]);
        if d.abs() < 1e-9 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let w0 = ((py[1] - py[2]) * (fx - px[2]) + (px[2] - px[1]) * (fy - py[2])) / d;
                let w1 = ((py[2] - py[0]) * (fx - px[2]) + (px[0] - px[2]) * (fy - py[2])) / d;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let ti = y * t + x;
                let dep = w0 * pz[0] + w1 * pz[1] + w2 * pz[2];
                if mask[ti] != 0 && dep < zbuf[ti] {
                    continue;
                }
                let mut p = [0f32; 3];
                for a in 0..3 {
                    p[a] = w0 * out.positions[idx[0]][a]
                        + w1 * out.positions[idx[1]][a]
                        + w2 * out.positions[idx[2]][a];
                }
                let Some(s6) = sample(p) else { continue };
                zbuf[ti] = dep;
                out.base_rgba[4 * ti] = u8c(s6[0]);
                out.base_rgba[4 * ti + 1] = u8c(s6[1]);
                out.base_rgba[4 * ti + 2] = u8c(s6[2]);
                out.base_rgba[4 * ti + 3] = u8c(s6[5]);
                out.mr_rgba[4 * ti] = 0;
                out.mr_rgba[4 * ti + 1] = u8c(s6[4]); // G = roughness
                out.mr_rgba[4 * ti + 2] = u8c(s6[3]); // B = metallic
                out.mr_rgba[4 * ti + 3] = 255;
                mask[ti] = 1;
            }
        }
    }
    dilate_full(&mut out.base_rgba, &mut out.mr_rgba, &mut mask, t);
    out
}

/// Box-projection bake, port of trellis.cpp uv_box_project — the reference
/// port's DEFAULT unwrap: occlusion-aware 6-plane bucket assignment (per-axis
/// min/max depth maps decide along which axis a face is actually the visible
/// surface; faces hidden along every usable axis get a second-layer bucket of
/// their dominant axis), 4x3 atlas grid, depth-tested raster, seam dilation.
/// Unlike the normal-cone chart growth, buckets cannot fold over themselves:
/// residual within-bucket overlap resolves outermost-wins instead of
/// z-fighting texel noise.
pub fn uv_box_bake(
    positions: &[[f32; 3]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
) -> BakedMesh {
    let t = texsize;
    let f = indices.len() / 3;
    let v = positions.len();
    let mut out = BakedMesh {
        positions: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        source_vertex: Vec::new(),
        base_rgba: Vec::new(),
        mr_rgba: Vec::new(),
        size: 0,
    };
    if f == 0 || t == 0 {
        return out;
    }
    // in-plane axes per dominant axis: X->(Y,Z), Y->(X,Z), Z->(X,Y)
    const UAX: [usize; 3] = [1, 0, 0];
    const VAX: [usize; 3] = [2, 2, 1];

    // face normals (unnormalized) + mean edge for the visibility epsilon
    let mut fnorm = vec![[0f32; 3]; f];
    let mut mean_edge = 0f32;
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        fnorm[fi] = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        mean_edge += (e1[0] * e1[0] + e1[1] * e1[1] + e1[2] * e1[2]).sqrt();
    }
    mean_edge /= f as f32;
    let eps = 2.0 * mean_edge;

    // global projected bbox per axis
    let mut gmin = [[f32::MAX; 2]; 3];
    let mut gmax = [[f32::MIN; 2]; 3];
    for p in positions {
        for ax in 0..3 {
            let (uu, vv) = (p[UAX[ax]], p[VAX[ax]]);
            gmin[ax][0] = gmin[ax][0].min(uu);
            gmax[ax][0] = gmax[ax][0].max(uu);
            gmin[ax][1] = gmin[ax][1].min(vv);
            gmax[ax][1] = gmax[ax][1].max(vv);
        }
    }

    // per-axis min/max depth maps from ALL faces
    const D: usize = 1024;
    let mut dmin = vec![f32::MAX; 3 * D * D];
    let mut dmax = vec![f32::MIN; 3 * D * D];
    let dtex = |ax: usize, uu: f32, vv: f32| -> (f32, f32) {
        (
            (uu - gmin[ax][0]) / (gmax[ax][0] - gmin[ax][0]).max(1e-6) * (D - 1) as f32,
            (vv - gmin[ax][1]) / (gmax[ax][1] - gmin[ax][1]).max(1e-6) * (D - 1) as f32,
        )
    };
    for tri in indices.chunks_exact(3) {
        for ax in 0..3 {
            let mut px = [0f32; 3];
            let mut py = [0f32; 3];
            let mut pd = [0f32; 3];
            for j in 0..3 {
                let p = positions[tri[j] as usize];
                let (fx, fy) = dtex(ax, p[UAX[ax]], p[VAX[ax]]);
                px[j] = fx;
                py[j] = fy;
                pd[j] = p[ax];
            }
            let x0 = (px[0].min(px[1]).min(px[2]).floor() as i64).max(0) as usize;
            let x1 = (px[0].max(px[1]).max(px[2]).ceil() as i64).min(D as i64 - 1) as usize;
            let y0 = (py[0].min(py[1]).min(py[2]).floor() as i64).max(0) as usize;
            let y1 = (py[0].max(py[1]).max(py[2]).ceil() as i64).min(D as i64 - 1) as usize;
            let d = (py[1] - py[2]) * (px[0] - px[2]) + (px[2] - px[1]) * (py[0] - py[2]);
            if d.abs() < 1e-9 {
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let fx = x as f32 + 0.5;
                    let fy = y as f32 + 0.5;
                    let w0 =
                        ((py[1] - py[2]) * (fx - px[2]) + (px[2] - px[1]) * (fy - py[2])) / d;
                    let w1 =
                        ((py[2] - py[0]) * (fx - px[2]) + (px[0] - px[2]) * (fy - py[2])) / d;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -0.05 || w1 < -0.05 || w2 < -0.05 {
                        continue;
                    }
                    let dep = w0 * pd[0] + w1 * pd[1] + w2 * pd[2];
                    let ti = ax * D * D + y * D + x;
                    dmin[ti] = dmin[ti].min(dep);
                    dmax[ti] = dmax[ti].max(dep);
                }
            }
        }
    }

    let visible = |tri: &[u32], ax: usize, dir: usize| -> bool {
        let probe = |uu: f32, vv: f32, dep: f32| -> bool {
            let (fx, fy) = dtex(ax, uu, vv);
            let x = (fx as i64).clamp(0, D as i64 - 1) as usize;
            let y = (fy as i64).clamp(0, D as i64 - 1) as usize;
            let ti = ax * D * D + y * D + x;
            if dir == 0 {
                dep >= dmax[ti] - eps
            } else {
                dep <= dmin[ti] + eps
            }
        };
        let (mut cu, mut cv, mut cd) = (0f32, 0f32, 0f32);
        let mut pass = 0;
        for &vi in tri.iter().take(3) {
            let p = positions[vi as usize];
            let (uu, vv, dep) = (p[UAX[ax]], p[VAX[ax]], p[ax]);
            cu += uu / 3.0;
            cv += vv / 3.0;
            cd += dep / 3.0;
            if probe(uu, vv, dep) {
                pass += 1;
            }
        }
        probe(cu, cv, cd) || pass >= 2
    };

    // occlusion-aware bucket assignment: 0..5 visible, 6..11 occluded layer
    const NB: usize = 12;
    let mut fbucket = vec![0usize; f];
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        let n = fnorm[fi];
        let nlen = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| {
            n[b].abs()
                .partial_cmp(&n[a].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let primary = order[0] * 2 + if n[order[0]] >= 0.0 { 0 } else { 1 };
        fbucket[fi] = 6 + primary;
        for (k, &ax) in order.iter().enumerate() {
            let dir = if n[ax] >= 0.0 { 0 } else { 1 };
            if k > 0 && n[ax].abs() < 0.35 * nlen {
                break;
            }
            if visible(tri, ax, dir) {
                fbucket[fi] = ax * 2 + dir;
                break;
            }
        }
    }

    // per-bucket vertex dedup + projected bbox
    let mut vmap = vec![i64::MIN; v * NB];
    let mut bxref: Vec<Vec<u32>> = vec![Vec::new(); NB];
    let mut umin = [f32::MAX; NB];
    let mut umax = [f32::MIN; NB];
    let mut vmin = [f32::MAX; NB];
    let mut vmax = [f32::MIN; NB];
    let mut bfaces: Vec<Vec<[u32; 3]>> = vec![Vec::new(); NB];
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        let g = fbucket[fi];
        let ax = (g % 6) / 2;
        let mut lidx = [0u32; 3];
        for (j, &vi) in tri.iter().enumerate() {
            let slot = vi as usize * NB + g;
            if vmap[slot] < 0 {
                vmap[slot] = bxref[g].len() as i64;
                bxref[g].push(vi);
                let p = positions[vi as usize];
                let (uu, vv) = (p[UAX[ax]], p[VAX[ax]]);
                umin[g] = umin[g].min(uu);
                umax[g] = umax[g].max(uu);
                vmin[g] = vmin[g].min(vv);
                vmax[g] = vmax[g].max(vv);
            }
            lidx[j] = vmap[slot] as u32;
        }
        bfaces[g].push(lidx);
    }

    // atlas layout: 4x3 grid, aspect-preserving fit + 2px pad
    let (cw, ch, pad) = (t / 4, t / 3, 2.0f32);
    let mut base_local = [0usize; NB];
    let mut vo = 0usize;
    for g in 0..NB {
        base_local[g] = vo;
        vo += bxref[g].len();
    }
    out.positions = vec![[0f32; 3]; vo];
    out.uvs = vec![[0f32; 2]; vo];
    out.source_vertex = vec![0u32; vo];
    let tf = t as f32;
    for g in 0..NB {
        let ax = (g % 6) / 2;
        let (col, row) = (g % 4, g / 4);
        let ox = (col * cw) as f32 + pad;
        let oy = (row * ch) as f32 + pad;
        let bw = (umax[g] - umin[g]).max(1e-6);
        let bh = (vmax[g] - vmin[g]).max(1e-6);
        let s = ((cw as f32 - 2.0 * pad) / bw).min((ch as f32 - 2.0 * pad) / bh);
        for (i, &vi) in bxref[g].iter().enumerate() {
            let o = base_local[g] + i;
            let p = positions[vi as usize];
            out.positions[o] = p;
            out.source_vertex[o] = vi;
            let (uu, vv) = (p[UAX[ax]], p[VAX[ax]]);
            out.uvs[o] = [
                (ox + (uu - umin[g]) * s) / tf,
                (oy + (vv - vmin[g]) * s) / tf,
            ];
        }
    }
    let mut fgroup: Vec<usize> = Vec::with_capacity(f);
    for g in 0..NB {
        for tri in &bfaces[g] {
            for &l in tri {
                out.indices.push((base_local[g] + l as usize) as u32);
            }
            fgroup.push(g);
        }
    }

    // depth-tested raster (outermost along the bucket's projection wins)
    out.size = t;
    out.base_rgba = vec![0u8; t * t * 4];
    out.mr_rgba = vec![0u8; t * t * 4];
    let mut mask = vec![0u8; t * t];
    let mut zbuf = vec![f32::MIN; t * t];
    let u8c = |x: f32| -> u8 { (x * 255.0).clamp(0.0, 255.0) as u8 };
    let fo = out.indices.len() / 3;
    for fi in 0..fo {
        let idx = [
            out.indices[fi * 3] as usize,
            out.indices[fi * 3 + 1] as usize,
            out.indices[fi * 3 + 2] as usize,
        ];
        let gax = (fgroup[fi] % 6) / 2;
        let gsign = if fgroup[fi] % 2 == 0 { 1.0f32 } else { -1.0 };
        let mut px = [0f32; 3];
        let mut py = [0f32; 3];
        let mut pz = [0f32; 3];
        for j in 0..3 {
            px[j] = out.uvs[idx[j]][0] * tf;
            py[j] = out.uvs[idx[j]][1] * tf;
            pz[j] = gsign * out.positions[idx[j]][gax];
        }
        let x0 = (px[0].min(px[1]).min(px[2]).floor() as i64).max(0) as usize;
        let x1 = (px[0].max(px[1]).max(px[2]).ceil() as i64).min(t as i64 - 1) as usize;
        let y0 = (py[0].min(py[1]).min(py[2]).floor() as i64).max(0) as usize;
        let y1 = (py[0].max(py[1]).max(py[2]).ceil() as i64).min(t as i64 - 1) as usize;
        let d = (py[1] - py[2]) * (px[0] - px[2]) + (px[2] - px[1]) * (py[0] - py[2]);
        if d.abs() < 1e-9 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let w0 = ((py[1] - py[2]) * (fx - px[2]) + (px[2] - px[1]) * (fy - py[2])) / d;
                let w1 = ((py[2] - py[0]) * (fx - px[2]) + (px[0] - px[2]) * (fy - py[2])) / d;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let ti = y * t + x;
                let dep = w0 * pz[0] + w1 * pz[1] + w2 * pz[2];
                if mask[ti] != 0 && dep < zbuf[ti] {
                    continue;
                }
                let mut p = [0f32; 3];
                for a in 0..3 {
                    p[a] = w0 * out.positions[idx[0]][a]
                        + w1 * out.positions[idx[1]][a]
                        + w2 * out.positions[idx[2]][a];
                }
                let Some(s6) = sample(p) else { continue };
                zbuf[ti] = dep;
                out.base_rgba[4 * ti] = u8c(s6[0]);
                out.base_rgba[4 * ti + 1] = u8c(s6[1]);
                out.base_rgba[4 * ti + 2] = u8c(s6[2]);
                out.base_rgba[4 * ti + 3] = u8c(s6[5]);
                out.mr_rgba[4 * ti] = 0;
                out.mr_rgba[4 * ti + 1] = u8c(s6[4]);
                out.mr_rgba[4 * ti + 2] = u8c(s6[3]);
                out.mr_rgba[4 * ti + 3] = 255;
                mask[ti] = 1;
            }
        }
    }
    dilate_full(&mut out.base_rgba, &mut out.mr_rgba, &mut mask, t);
    out
}

/// Official Hunyuan-Paint unwrap: `xatlas.parametrize` defaults, then
/// `uv / (width, height)`. Vertices on chart seams are split (`xref`).
pub fn uv_xatlas_unwrap(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>, Vec<u32>), String> {
    uv_xatlas_unwrap_ctl(positions, indices, &mut |_| true)
}

/// Same as [`uv_xatlas_unwrap`]; `ctl(frac)` reports xatlas's phase
/// boundaries (add mesh / charts / pack / output). Returning false aborts
/// with an error.
pub fn uv_xatlas_unwrap_ctl(
    positions: &[[f32; 3]],
    indices: &[u32],
    ctl: &mut impl FnMut(f64) -> bool,
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>, Vec<u32>), String> {
    if positions.is_empty() || indices.len() < 3 {
        return Err("xatlas unwrap: empty mesh".into());
    }
    if indices.len() % 3 != 0 {
        return Err("xatlas unwrap: indices are not triangles".into());
    }
    let faces: Vec<[u32; 3]> = indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let atlas = makepad_xatlas::parametrize_with_progress(positions, &faces, ctl).map_err(|err| {
        format!("xatlas parametrize failed: {err}")
    })?;
    if atlas.width == 0 || atlas.height == 0 || atlas.vertices.is_empty() {
        return Err("xatlas produced an empty atlas".into());
    }
    let uvs: Vec<[f32; 2]> = atlas
        .normalized_uvs()
        .into_iter()
        .map(|uv| [uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)])
        .collect();
    let out_positions: Vec<[f32; 3]> = atlas
        .vertices
        .iter()
        .map(|v| positions[v.xref as usize])
        .collect();
    let source_vertex: Vec<u32> = atlas.vertices.iter().map(|v| v.xref).collect();
    let out_indices: Vec<u32> = atlas
        .indices
        .iter()
        .flat_map(|f| [f[0], f[1], f[2]])
        .collect();
    Ok((out_positions, uvs, out_indices, source_vertex))
}

/// Raster an already-unwrapped mesh into a PBR atlas. Used after
/// [`uv_xatlas_unwrap`] so Hunyuan and TRELLIS share one UV set.
pub fn uv_atlas_bake(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
) -> BakedMesh {
    uv_atlas_bake_ctl(positions, uvs, indices, texsize, sample, &mut |_, _| true)
}

/// Same as [`uv_atlas_bake`]; `ctl(done_tris, total_tris)` is called every
/// 512 triangles. Returning false stops rasterizing (partial atlas).
pub fn uv_atlas_bake_ctl(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
    ctl: &mut impl FnMut(usize, usize) -> bool,
) -> BakedMesh {
    let t = texsize;
    let mut out = BakedMesh {
        positions: positions.to_vec(),
        uvs: uvs.to_vec(),
        indices: indices.to_vec(),
        source_vertex: (0..positions.len() as u32).collect(),
        base_rgba: vec![0; t * t * 4],
        mr_rgba: vec![0; t * t * 4],
        size: t,
    };
    if t == 0 || positions.is_empty() || uvs.len() != positions.len() || indices.len() < 3 {
        out.size = 0;
        return out;
    }
    let mut mask = vec![0u8; t * t];
    let mut zbuf = vec![f32::INFINITY; t * t];
    let ts = t as f32;
    let total_tris = indices.len() / 3;
    for (tri_i, tri) in indices.chunks_exact(3).enumerate() {
        if tri_i % 512 == 0 && !ctl(tri_i, total_tris) {
            break;
        }
        let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if idx.iter().any(|&i| i >= positions.len() || i >= uvs.len()) {
            continue;
        }
        let uv0 = [uvs[idx[0]][0] * ts, uvs[idx[0]][1] * ts];
        let uv1 = [uvs[idx[1]][0] * ts, uvs[idx[1]][1] * ts];
        let uv2 = [uvs[idx[2]][0] * ts, uvs[idx[2]][1] * ts];
        let min_x = uv0[0].min(uv1[0]).min(uv2[0]).floor().max(0.0) as i32;
        let max_x = uv0[0].max(uv1[0]).max(uv2[0]).ceil().min(ts - 1.0) as i32;
        let min_y = uv0[1].min(uv1[1]).min(uv2[1]).floor().max(0.0) as i32;
        let max_y = uv0[1].max(uv1[1]).max(uv2[1]).ceil().min(ts - 1.0) as i32;
        let area = (uv1[0] - uv0[0]) * (uv2[1] - uv0[1]) - (uv1[1] - uv0[1]) * (uv2[0] - uv0[0]);
        if area.abs() < 1e-8 {
            continue;
        }
        let inv = 1.0 / area;
        for y in min_y..=max_y {
            let sy = y as f32 + 0.5;
            for x in min_x..=max_x {
                let sx = x as f32 + 0.5;
                let w0 = ((uv1[0] - sx) * (uv2[1] - sy) - (uv1[1] - sy) * (uv2[0] - sx)) * inv;
                let w1 = ((uv2[0] - sx) * (uv0[1] - sy) - (uv2[1] - sy) * (uv0[0] - sx)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let ti = y as usize * t + x as usize;
                let mut p = [0f32; 3];
                for a in 0..3 {
                    p[a] = w0 * positions[idx[0]][a]
                        + w1 * positions[idx[1]][a]
                        + w2 * positions[idx[2]][a];
                }
                let Some(s6) = sample(p) else { continue };
                if 0.0 < zbuf[ti] && zbuf[ti] < f32::INFINITY {
                    continue;
                }
                zbuf[ti] = 0.0;
                out.base_rgba[4 * ti] = u8c(s6[0]);
                out.base_rgba[4 * ti + 1] = u8c(s6[1]);
                out.base_rgba[4 * ti + 2] = u8c(s6[2]);
                out.base_rgba[4 * ti + 3] = u8c(s6[5]);
                out.mr_rgba[4 * ti] = 0;
                out.mr_rgba[4 * ti + 1] = u8c(s6[4]);
                out.mr_rgba[4 * ti + 2] = u8c(s6[3]);
                out.mr_rgba[4 * ti + 3] = 255;
                mask[ti] = 1;
            }
        }
    }
    dilate_full(&mut out.base_rgba, &mut out.mr_rgba, &mut mask, t);
    out
}

/// Hunyuan-compatible unwrap then TRELLIS volume bake onto that atlas.
pub fn uv_xatlas_bake(
    positions: &[[f32; 3]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
) -> BakedMesh {
    uv_xatlas_bake_ctl(positions, indices, texsize, sample, &mut |_, _| true)
}

/// Same as [`uv_xatlas_bake`]; `ctl(stage, frac)` reports `"unwrap"` (xatlas
/// phases, coarse) then `"bake"` (per triangle). Returning false stops.
pub fn uv_xatlas_bake_ctl(
    positions: &[[f32; 3]],
    indices: &[u32],
    texsize: usize,
    sample: BakeSampler,
    ctl: &mut impl FnMut(&str, f64) -> bool,
) -> BakedMesh {
    let unwrapped = uv_xatlas_unwrap_ctl(positions, indices, &mut |frac| ctl("unwrap", frac));
    match unwrapped {
        Ok((pos, uvs, idx, source)) => {
            let mut baked = uv_atlas_bake_ctl(&pos, &uvs, &idx, texsize, sample, &mut |done, total| {
                ctl("bake", done as f64 / total.max(1) as f64)
            });
            baked.source_vertex = source;
            baked
        }
        Err(_) => BakedMesh {
            positions: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            source_vertex: Vec::new(),
            base_rgba: Vec::new(),
            mr_rgba: Vec::new(),
            size: 0,
        },
    }
}

fn u8c(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_bake_covers_and_separates_planes() {
        // Same L geometry through the box projection: two buckets (+Z and
        // +X), full dilation coverage, valid UVs, indices preserved.
        let positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        let indices = vec![0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2];
        let sampler =
            |p: [f32; 3]| -> Option<[f32; 6]> { Some([p[0], p[1], p[2], 0.0, 1.0, 1.0]) };
        let baked = uv_box_bake(&positions, &indices, 64, &sampler);
        assert!(baked.ok());
        assert_eq!(baked.indices.len(), indices.len());
        for uv in &baked.uvs {
            assert!(uv[0] >= 0.0 && uv[0] <= 1.0, "u {}", uv[0]);
            assert!(uv[1] >= 0.0 && uv[1] <= 1.0, "v {}", uv[1]);
        }
        assert!(baked.base_rgba.chunks_exact(4).all(|px| px[3] > 0));
        assert_eq!(baked.source_vertex.len(), baked.positions.len());
        // source_vertex maps back to identical positions
        for (o, &src) in baked.source_vertex.iter().enumerate() {
            assert_eq!(baked.positions[o], positions[src as usize]);
        }
    }

    #[test]
    fn bakes_a_two_plane_box_corner() {
        // Two perpendicular quads (an L): must become >= 2 charts, all
        // texels inside the charts shaded, UVs in range.
        let positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        let indices = vec![
            0, 1, 2, 0, 2, 3, // z=0 plane
            1, 4, 5, 1, 5, 2, // x=1 plane
        ];
        let sampler = |p: [f32; 3]| -> Option<[f32; 6]> {
            // color encodes position for a checkable bake
            Some([p[0], p[1], p[2], 0.0, 1.0, 1.0])
        };
        let baked = uv_chart_bake(&positions, &indices, 64, &sampler);
        assert!(baked.ok());
        assert_eq!(baked.indices.len(), indices.len());
        for uv in &baked.uvs {
            assert!(uv[0] >= 0.0 && uv[0] <= 1.0, "u {}", uv[0]);
            assert!(uv[1] >= 0.0 && uv[1] <= 1.0, "v {}", uv[1]);
        }
        // Dilation fills the whole atlas.
        assert!(baked.base_rgba.chunks_exact(4).all(|px| px[3] > 0));
        // A texel at the center of the z=0 quad should be greenish-red
        // (x,y in 0..1, z=0 -> blue 0). Find it via a vertex uv.
        let uv = baked.uvs[0];
        let x = (uv[0] * 64.0) as usize + 2;
        let y = (uv[1] * 64.0) as usize + 2;
        let px = &baked.base_rgba[4 * (y * 64 + x)..4 * (y * 64 + x) + 4];
        assert!(px[2] < 60, "z=0 plane should have low blue, got {px:?}");
    }

    #[test]
    fn xatlas_unwrap_covers_an_l_mesh() {
        let positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        let indices = vec![0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2];
        let (pos, uvs, idx, src) = uv_xatlas_unwrap(&positions, &indices).expect("unwrap");
        assert_eq!(pos.len(), uvs.len());
        assert_eq!(pos.len(), src.len());
        assert_eq!(idx.len(), indices.len());
        for uv in &uvs {
            assert!((0.0..=1.0).contains(&uv[0]));
            assert!((0.0..=1.0).contains(&uv[1]));
        }
        for &s in &src {
            assert!((s as usize) < positions.len());
        }
        let sampler = |p: [f32; 3]| -> Option<[f32; 6]> { Some([p[0], p[1], p[2], 0.0, 1.0, 1.0]) };
        let baked = uv_xatlas_bake(&positions, &indices, 64, &sampler);
        assert!(baked.ok());
        assert_eq!(baked.source_vertex.len(), baked.positions.len());
    }
}
