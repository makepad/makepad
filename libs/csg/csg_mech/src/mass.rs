//! Mass, centre of mass and inertia of a closed triangle mesh, exactly (signed
//! tetrahedra from the origin; covariance of each canonical tetrahedron).

use crate::geom::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProps {
    /// mm^3.
    pub volume: f64,
    /// g.
    pub mass: f64,
    pub com: P3,
    /// About the centre of mass, model axes, g mm^2.
    pub inertia: [[f64; 3]; 3],
}

/// `fill` scales the density (1: solid; a slicer estimate of walls + infill for a printed part).
pub fn mesh_mass(vertices: &[P3], triangles: &[[u32; 3]], density: f64, fill: f64) -> MassProps {
    let canon = [[2.0, 1.0, 1.0], [1.0, 2.0, 1.0], [1.0, 1.0, 2.0]];
    let mut vol = 0.0;
    let mut first = [0.0; 3];
    let mut cov = [[0.0f64; 3]; 3];
    for t in triangles {
        let a = vertices[t[0] as usize];
        let b = vertices[t[1] as usize];
        let c = vertices[t[2] as usize];
        let det = dot(a, cross(b, c));
        let v = det / 6.0;
        vol += v;
        for k in 0..3 {
            first[k] += v * (a[k] + b[k] + c[k]) / 4.0;
        }
        // covariance = det/120 * A * canon * A^T with A's columns a, b, c
        let m = [a, b, c];
        for i in 0..3 {
            for j in 0..3 {
                let mut s = 0.0;
                for p in 0..3 {
                    for q in 0..3 {
                        s += m[p][i] * canon[p][q] * m[q][j];
                    }
                }
                cov[i][j] += det / 120.0 * s;
            }
        }
    }
    let rho = density * fill;
    let mass = vol * rho;
    let com = if vol.abs() > 1e-12 { scale(first, 1.0 / vol) } else { [0.0; 3] };
    // shift the covariance to the centre of mass, then inertia = tr(C) I - C
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = rho * (cov[i][j] - vol * com[i] * com[j]);
        }
    }
    let tr = c[0][0] + c[1][1] + c[2][2];
    let inertia = std::array::from_fn(|i| std::array::from_fn(|j| if i == j { tr - c[i][j] } else { -c[i][j] }));
    MassProps { volume: vol, mass, com, inertia }
}

/// Mass properties of a solid given as equal cubic cells (their centres, each of volume `voxel`).
pub fn voxel_mass(centres: &[P3], voxel: f64, density: f64, fill: f64) -> MassProps {
    let vol = voxel * centres.len() as f64;
    let mut first = [0.0; 3];
    for c in centres {
        for k in 0..3 {
            first[k] += voxel * c[k];
        }
    }
    let com = if vol.abs() > 1e-12 { scale(first, 1.0 / vol) } else { [0.0; 3] };
    // second moments about the centre of mass, with each cell's own extent
    let edge2 = voxel.cbrt().powi(2);
    let mut cov = [[0.0f64; 3]; 3];
    for c in centres {
        let d = sub(*c, com);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += voxel * d[i] * d[j];
            }
            cov[i][i] += voxel * edge2 / 12.0;
        }
    }
    let rho = density * fill;
    let tr = rho * (cov[0][0] + cov[1][1] + cov[2][2]);
    let inertia = std::array::from_fn(|i| std::array::from_fn(|j| if i == j { tr - rho * cov[i][j] } else { -rho * cov[i][j] }));
    MassProps { volume: vol, mass: vol * rho, com, inertia }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_mass() {
        // a 2 x 4 x 6 box at (1, 2, 3)
        let (sx, sy, sz) = (2.0, 4.0, 6.0);
        let c = [1.0, 2.0, 3.0];
        let v: Vec<P3> = (0..8).map(|i| [c[0] + if i & 1 == 0 { -sx / 2.0 } else { sx / 2.0 }, c[1] + if i & 2 == 0 { -sy / 2.0 } else { sy / 2.0 }, c[2] + if i & 4 == 0 { -sz / 2.0 } else { sz / 2.0 }]).collect();
        let t: Vec<[u32; 3]> = vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        let m = mesh_mass(&v, &t, 1.0, 1.0);
        assert!((m.volume - 48.0).abs() < 1e-9, "{}", m.volume);
        assert!(len(sub(m.com, c)) < 1e-9);
        let ix = 48.0 * (sy * sy + sz * sz) / 12.0;
        let iy = 48.0 * (sx * sx + sz * sz) / 12.0;
        let iz = 48.0 * (sx * sx + sy * sy) / 12.0;
        assert!((m.inertia[0][0] - ix).abs() < 1e-6 && (m.inertia[1][1] - iy).abs() < 1e-6 && (m.inertia[2][2] - iz).abs() < 1e-6, "{:?}", m.inertia);
        assert!(m.inertia[0][1].abs() < 1e-6);
    }
}
