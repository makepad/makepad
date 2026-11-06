use makepad_csg_math::fast_hash::FastMap;
use makepad_csg_mesh::mesh::TriMesh;

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn join(parent: &mut [usize], a: usize, b: usize) {
    let a = root(parent, a);
    let b = root(parent, b);
    parent[a] = b;
}

/// Write the boundary of a pinched regularised set as separate manifold fans.
/// Coincident positions are retained: this changes incidence, never geometry.
/// Around a contact edge, alternating oriented faces bound disjoint material
/// wedges. Glue each wedge's two faces, then give each connected vertex fan
/// its own index. Coincident rays and nonalternating faces are not pinches:
/// their extra incidence remains visible to validation, rather than disguising
/// overlapping walls or zero-thickness sheets as separate shells.
///
/// Only active vertices can acquire new fans: untouched operand regions were
/// manifold, while selection seams, weld survivors (including earlier contact
/// copies), open edges and finishing edits are active. Finishing preserves the
/// welded vertex indices; T-junctions insert existing active vertices. A single
/// triangle scan finds their complete stars. Edge storage and fan traversal are
/// local to those stars, and inactive corners keep their original indices.
pub(crate) fn separate_contact_fans(mesh: &mut TriMesh, active: &[bool]) {
    debug_assert_eq!(active.len(), mesh.vertex_count());
    let triangles: Vec<usize> = mesh.triangles.iter().enumerate()
        .filter(|(_, tri)| tri.iter().any(|&v| active[v as usize]))
        .map(|(ti, _)| ti).collect();
    let mut edges: FastMap<(u32, u32), Vec<usize>> = FastMap::default();
    for (ti, &original) in triangles.iter().enumerate() {
        let tri = mesh.triangles[original];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            if !active[a as usize] && !active[b as usize] { continue; }
            edges.entry((a.min(b), a.max(b))).or_default().push(3 * ti + k);
        }
    }
    let mut wedges = Vec::new();
    let mut parent: Vec<usize> = (0..triangles.len() * 3).collect();
    let corner = |h: usize, v: u32| {
        let ti = h / 3;
        3 * ti + mesh.triangles[triangles[ti]].iter().position(|&x| x == v).unwrap()
    };
    for (&(a, b), group) in &edges {
        let mut pairs = Vec::new();
        if group.len() > 2 && group.len() % 2 == 0 {
            let origin = mesh.vertices[a as usize];
            let axis = (mesh.vertices[b as usize] - origin).normalize();
            let radial = |h: usize| {
                let p = mesh.vertices[mesh.triangles[triangles[h / 3]][(h % 3 + 2) % 3] as usize] - origin;
                (p - axis * p.dot(axis)).normalize()
            };
            let u = radial(group[0]);
            let v = axis.cross(u);
            let mut rays: Vec<_> = group.iter().map(|&h| {
                let r = radial(h);
                (r.dot(v).atan2(r.dot(u)), h, r, mesh.triangles[triangles[h / 3]][h % 3] == a)
            }).collect();
            rays.sort_by(|a, b| a.0.total_cmp(&b.0));
            let separate = (0..rays.len()).all(|i| {
                let x = rays[i];
                let y = rays[(i + 1) % rays.len()];
                x.3 != y.3 && x.2.is_finite() && y.2.is_finite()
                    && (x.2.dot(y.2) < 0.0 || x.2.cross(y.2).length() > 64.0 * f64::EPSILON)
            });
            if separate {
                for i in 0..rays.len() {
                    // Looking along the positive axis, a reverse edge has
                    // material counterclockwise from its ray; the next forward
                    // edge closes that material wedge.
                    if !rays[i].3 {
                        let pair = (rays[i].1, rays[(i + 1) % rays.len()].1);
                        pairs.push(pair);
                        wedges.push((pair, origin + (mesh.vertices[b as usize] - origin) * 0.5));
                    }
                }
            }
        }
        if pairs.is_empty() {
            // Ordinary edges glue both corners. Invalid extra incidence is
            // deliberately kept together so that it cannot become a false pass.
            pairs.extend(group.windows(2).map(|w| (w[0], w[1])));
        }
        for (x, y) in pairs {
            if active[a as usize] { join(&mut parent, corner(x, a), corner(y, a)); }
            if active[b as usize] { join(&mut parent, corner(x, b), corner(y, b)); }
        }
    }
    let mut ids = vec![u32::MAX; parent.len()];
    let mut used = FastMap::default();
    for (ti, &original) in triangles.iter().enumerate() {
        for k in 0..3 {
            let old = mesh.triangles[original][k];
            if !active[old as usize] { continue; }
            let r = root(&mut parent, 3 * ti + k);
            if ids[r] == u32::MAX {
                ids[r] = if used.insert(old, ()).is_some() {
                    mesh.add_vertex(mesh.vertices[old as usize])
                } else {
                    old
                };
            }
            mesh.triangles[original][k] = ids[r];
        }
    }
    // A vertex fan can reconnect around the end of a finite contact line.
    // Its material wedges then have distinct abstract edges with the SAME
    // endpoint indices. An indexed triangle mesh cannot encode parallel edges,
    // so each wedge gets an interior vertex. This is part of writing the
    // regularised boundary as a simplicial mesh, not a validation-driven repair.
    if !wedges.is_empty() {
        let mut splits = FastMap::default();
        for ((x, y), position) in wedges {
            let mid = mesh.add_vertex(position);
            splits.entry(triangles[x / 3]).or_insert([None; 3])[x % 3] = Some(mid);
            splits.entry(triangles[y / 3]).or_insert([None; 3])[y % 3] = Some(mid);
        }
        let mut splits: Vec<_> = splits.into_iter().collect();
        splits.sort_unstable_by_key(|s| s.0);
        let old = std::mem::take(&mut mesh.triangles);
        mesh.clear_normals();
        mesh.triangles.reserve(old.len() + splits.len());
        let mut from = 0;
        for (ti, split) in splits {
            mesh.triangles.extend_from_slice(&old[from..ti]);
            from = ti + 1;
            let tri = old[ti];
            match split.iter().filter(|s| s.is_some()).count() {
                0 => mesh.triangles.push(tri),
                1 => {
                    let k = split.iter().position(|s| s.is_some()).unwrap();
                    let (a, b, c) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
                    let mid = split[k].unwrap();
                    mesh.triangles.extend([[a, mid, c], [mid, b, c]]);
                }
                _ => {
                    let p = tri.map(|v| mesh.vertices[v as usize]);
                    let center = mesh.add_vertex(p[0] + ((p[1] - p[0]) + (p[2] - p[0])) / 3.0);
                    for k in 0..3 {
                        let (a, b) = (tri[k], tri[(k + 1) % 3]);
                        if let Some(mid) = split[k] {
                            mesh.triangles.extend([[a, mid, center], [mid, b, center]]);
                        } else {
                            mesh.triangles.push([a, b, center]);
                        }
                    }
                }
            }
        }
        mesh.triangles.extend_from_slice(&old[from..]);
    }
}
