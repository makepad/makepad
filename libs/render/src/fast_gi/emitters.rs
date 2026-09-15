//! Bounded static-box emission quadrature. This runs with probe placement,
//! not per frame: each tile carries its exact full-sphere L1 integral and a
//! center ray whose visibility is evaluated later on the GPU. Occlusion
//! within a tile remains an approximation; no CPU visibility rays are used.
use super::{GiConfig, Mover, Vec3f};

pub(super) const MAX_EMITTERS: usize = 2;
pub(super) const SAMPLES_PER_EMITTER: usize = 12;

#[derive(Clone, Copy)]
pub(super) struct Emitter {
    pub box_: Mover,
    /// Positive integer also stored in the source triangles' emission.w.
    pub id: f32,
}

pub(super) struct Samples {
    /// Rows: position/state, `count` emission/ID headers, then 12 pairs per
    /// source. Pair = (unit direction, distance), (DC, L1x, L1y, L1z).
    pub data: Vec<f32>,
    pub width: usize,
    pub count: usize,
    /// Relevant sources outside the selection budget or with invalid data.
    /// Their ordinary 64-ray emission must remain enabled.
    pub fallbacks: usize,
    /// Selected source/probe pairs whose header ID is zero: use ordinary
    /// emission at these probes too, without double counting at other probes.
    pub probe_fallbacks: usize,
}

type D3 = [f64; 3];
fn add(a: D3, b: D3) -> D3 { std::array::from_fn(|i| a[i] + b[i]) }
fn sub(a: D3, b: D3) -> D3 { std::array::from_fn(|i| a[i] - b[i]) }
fn scale(a: D3, s: f64) -> D3 { a.map(|x| x * s) }
fn dot(a: D3, b: D3) -> f64 { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
fn cross(a: D3, b: D3) -> D3 {
    [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]
}
fn length(a: D3) -> f64 { dot(a, a).sqrt() }
fn finite(a: D3) -> bool { a.into_iter().all(f64::is_finite) }
fn d3(v: Vec3f) -> D3 { [v.x as f64, v.y as f64, v.z as f64] }

fn world_corners(m: &Mover) -> Option<[D3; 8]> {
    let lo = d3(m.min);
    let hi = d3(m.max);
    let t = m.transform.v.map(f64::from);
    if !finite(lo) || !finite(hi) || !t.iter().all(|v| v.is_finite())
        || t[3] != 0.0 || t[7] != 0.0 || t[11] != 0.0 || t[15] != 1.0
        || (0..3).any(|i| lo[i] > hi[i]) { return None; }
    let corners = std::array::from_fn(|i| {
        let p: D3 = std::array::from_fn(|a| if i & (1 << a) == 0 { lo[a] } else { hi[a] });
        std::array::from_fn(|a| t[a]*p[0] + t[4+a]*p[1] + t[8+a]*p[2] + t[12+a])
    });
    corners.iter().all(|&p| finite(p)).then_some(corners)
}

fn bounds(corners: &[D3; 8]) -> (D3, D3) {
    let lo = std::array::from_fn(|a| corners.iter().map(|p| p[a]).fold(f64::INFINITY, f64::min));
    let hi = std::array::from_fn(|a| corners.iter().map(|p| p[a]).fold(f64::NEG_INFINITY, f64::max));
    (lo, hi)
}

#[derive(Clone, Copy)]
struct Source {
    corners: [D3; 8],
    edges: [D3; 3],
    inverse: [D3; 3],
    emission: [f32; 3],
    id: f32,
    score: f64,
}

impl Source {
    fn new(e: &Emitter, corners: [D3; 8], center: D3, spacing: f64) -> Option<Self> {
        let emission = [e.box_.emission.x, e.box_.emission.y, e.box_.emission.z];
        if !emission.iter().all(|v| v.is_finite()) || !e.id.is_finite()
            || e.id <= 0.0 || e.id > 16_777_216.0 || e.id.fract() != 0.0 { return None; }
        let emission = emission.map(|v| v.clamp(0.0, 8.0));
        let edges = [sub(corners[1], corners[0]), sub(corners[2], corners[0]), sub(corners[4], corners[0])];
        let cofactors = [cross(edges[1], edges[2]), cross(edges[2], edges[0]), cross(edges[0], edges[1])];
        let determinant = dot(edges[0], cofactors[0]);
        let size = edges.iter().map(|&v| length(v)).product::<f64>();
        if !size.is_finite() || size <= 0.0 || !determinant.is_finite()
            || determinant.abs() <= size * 1e-12 { return None; }
        let inverse = cofactors.map(|v| scale(v, 1.0/determinant));
        if !inverse.iter().all(|&v| finite(v)) { return None; }
        let area = 2.0 * cofactors.iter().map(|&v| length(v)).sum::<f64>();
        let source_center = scale(add(corners[0], corners[7]), 0.5);
        let delta = sub(source_center, center);
        let brightness = emission.into_iter().fold(0.0_f32, f32::max) as f64;
        let score = brightness * area / dot(delta, delta).max(spacing * spacing);
        (score.is_finite() && score > 0.0).then_some(Self { corners, edges, inverse, emission, id: e.id, score })
    }

    fn local(&self, p: D3) -> D3 {
        let p = sub(p, self.corners[0]);
        self.inverse.map(|row| dot(row, p))
    }

    fn samples(&self, probe: D3, range: f64) -> Option<[[f32; 8]; SAMPLES_PER_EMITTER]> {
        let local = self.local(probe);
        if !finite(probe) || !finite(local)
            || local.iter().all(|&v| v >= -1e-10 && v <= 1.0+1e-10) { return None; }
        // All-or-nothing eligibility: partial range clipping cannot silently
        // replace the ordinary estimator with an incomplete emitter integral.
        if self.corners.iter().any(|&q| length(sub(q, probe)) > range) { return None; }
        let mut out = [[0.0_f32; 8]; SAMPLES_PER_EMITTER];
        let mut slot = 0;
        for axis in 0..3 {
            let side = if local[axis] < -1e-10 { 0.0 } else if local[axis] > 1.0+1e-10 { 1.0 } else { continue; };
            let axes = match axis { 0 => [1, 2], 1 => [0, 2], _ => [0, 1] };
            let u = self.edges[axes[0]];
            let v = self.edges[axes[1]];
            let face = add(self.corners[0], scale(self.edges[axis], side));
            for y in 0..2 { for x in 0..2 {
                let q = add(face, add(scale(u, x as f64*0.5), scale(v, y as f64*0.5)));
                let quad = [q, add(q, scale(u, 0.5)), add(q, scale(add(u, v), 0.5)), add(q, scale(v, 0.5))];
                let integrated = integrate_quad(probe, quad)?;
                let delta = sub(add(q, scale(add(u, v), 0.25)), probe);
                let distance = length(delta);
                // Match the GPU endpoint exclusion: never suppress ordinary
                // emission for a target the visibility pass will skip.
                if !distance.is_finite() || distance <= 0.002 { return None; }
                let direction = scale(delta, 1.0/distance);
                out[slot] = [direction[0] as f32, direction[1] as f32, direction[2] as f32, distance as f32,
                    integrated[0] as f32, integrated[1] as f32, integrated[2] as f32, integrated[3] as f32];
                if !out[slot].iter().all(|v| v.is_finite()) { return None; }
                slot += 1;
            }}
        }
        (slot > 0).then_some(out)
    }
}

/// Exact projection of a convex planar quad onto basis (1, 2*direction),
/// normalized over the full sphere. This is not clamped-cosine irradiance.
fn integrate_quad(probe: D3, corners: [D3; 4]) -> Option<[f64; 4]> {
    let mut rays = [[0.0; 3]; 4];
    for (i, q) in corners.into_iter().enumerate() {
        let delta = sub(q, probe);
        let distance = length(delta);
        if !distance.is_finite() || distance <= 1e-12 { return None; }
        rays[i] = scale(delta, 1.0/distance);
    }
    let triangle = |a: D3, b: D3, c: D3| {
        2.0 * dot(a, cross(b, c)).atan2(1.0 + dot(a, b) + dot(b, c) + dot(c, a))
    };
    let mut omega = triangle(rays[0], rays[1], rays[2]) + triangle(rays[0], rays[2], rays[3]);
    let mut vector = [0.0; 3];
    for i in 0..4 {
        let a = rays[i];
        let b = rays[(i+1)%4];
        let edge = cross(a, b);
        let size = length(edge);
        if !size.is_finite() || size <= 1e-15 { return None; }
        let angle = size.atan2(dot(a, b));
        vector = add(vector, scale(edge, 0.5*angle/size));
    }
    // The face's vertex winding changes under mirrored transforms. Correct
    // the solid angle and first moment together, not by clamping components.
    if omega < 0.0 { omega = -omega; vector = scale(vector, -1.0); }
    let pi = std::f64::consts::PI;
    if !omega.is_finite() || !finite(vector) || omega <= 1e-14 || omega > 2.0*pi+1e-10
        || length(vector) > omega*(1.0+1e-7)+1e-12 { return None; }
    Some([omega/(4.0*pi), vector[0]/(2.0*pi), vector[1]/(2.0*pi), vector[2]/(2.0*pi)])
}

impl Samples {
    pub(super) fn build(emitters: &[Emitter], positions: &[f32], origin: Vec3f, c: GiConfig) -> Self {
        let c = c.clamped();
        let low = d3(origin);
        let high = std::array::from_fn(|i| low[i] + (c.grid[i]-1) as f64*c.spacing as f64);
        let center = scale(add(low, high), 0.5);
        let range = c.ray_distance as f64;
        let mut selected = Vec::<Source>::with_capacity(MAX_EMITTERS);
        let mut relevant = 0usize;
        for e in emitters {
            let emission = [e.box_.emission.x, e.box_.emission.y, e.box_.emission.z];
            if emission.iter().all(|v| v.is_finite() && *v <= 0.0) { continue; }
            let Some(corners) = world_corners(&e.box_) else { relevant += 1; continue; };
            let (lo, hi) = bounds(&corners);
            let gap: D3 = std::array::from_fn(|a| (low[a]-hi[a]).max(lo[a]-high[a]).max(0.0));
            if finite(low) && length(gap) > range { continue; }
            relevant += 1;
            let Some(source) = Source::new(e, corners, center, c.spacing as f64) else { continue; };
            // Bounded insertion, stable on equal scores; memory does not grow
            // with the scene's emitter population.
            let at = selected.iter().position(|s| source.score > s.score).unwrap_or(selected.len());
            if at < MAX_EMITTERS {
                if selected.len() == MAX_EMITTERS { selected.pop(); }
                selected.insert(at, source);
            }
        }
        let count = selected.len();
        let width = 1 + count + count*SAMPLES_PER_EMITTER*2;
        let rows = positions.len()/4;
        let mut out = Self { data: vec![0.0; rows*width*4], width, count,
            fallbacks: relevant-count, probe_fallbacks: 0 };
        for (row, position) in positions.chunks_exact(4).enumerate() {
            let row_start = row*width*4;
            out.data[row_start..row_start+4].copy_from_slice(position);
            for (emitter, source) in selected.iter().enumerate() {
                let probe = [position[0] as f64, position[1] as f64, position[2] as f64];
                let samples = if position[3].is_finite() && position[3] >= 0.5 {
                    source.samples(probe, range)
                } else { None };
                let Some(samples) = samples else { out.probe_fallbacks += 1; continue; };
                let header = row_start+(1+emitter)*4;
                out.data[header..header+4].copy_from_slice(&[source.emission[0], source.emission[1], source.emission[2], source.id]);
                let start = row_start+(1+count+emitter*SAMPLES_PER_EMITTER*2)*4;
                for (slot, sample) in samples.into_iter().enumerate() {
                    out.data[start+slot*8..start+(slot+1)*8].copy_from_slice(&sample);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_gi::{Mat4f, vec3f};

    fn emitter(id: f32) -> Emitter {
        Emitter { id, box_: Mover { transform: Mat4f::identity(), min: vec3f(-0.5,-0.5,-0.5), max: vec3f(0.5,0.5,0.5),
            color: Vec3f::default(), emission: vec3f(2.0,3.0,4.0), exact_box: true } }
    }
    fn position(p: D3) -> [f32; 4] { [p[0] as f32,p[1] as f32,p[2] as f32,1.0] }
    fn config() -> GiConfig { GiConfig { grid: [2,2,2], ..GiConfig::default() } }
    fn packed_coefficients(s: &Samples, row: usize, emitter: usize) -> [f64; 4] {
        let start = (row*s.width+1+s.count+emitter*SAMPLES_PER_EMITTER*2)*4;
        let mut out = [0.0; 4];
        for slot in 0..SAMPLES_PER_EMITTER {
            for i in 0..4 { out[i] += s.data[start+slot*8+4+i] as f64; }
        }
        out
    }
    fn assert_close(a: [f64; 4], b: [f64; 4], tolerance: f64) {
        for i in 0..4 { assert!((a[i]-b[i]).abs()<tolerance,"coefficient {i}: {} vs {}",a[i],b[i]); }
    }

    // Independent uniform-area integration with dOmega=cos(theta)*dA/r^2.
    // No spherical triangles or boundary-vector formula from production.
    fn dense_reference(e: Emitter, p: D3, divisions: usize) -> [f64; 4] {
        let corners = world_corners(&e.box_).unwrap();
        let source = Source::new(&e,corners,[0.0;3],1.5).unwrap();
        let local = source.local(p);
        let mut result = [0.0;4];
        for axis in 0..3 {
            let side = if local[axis]<0.0 {0.0} else if local[axis]>1.0 {1.0} else {continue;};
            let axes: Vec<usize>=(0..3).filter(|&i|i!=axis).collect();
            let u=source.edges[axes[0]];let v=source.edges[axes[1]];
            let area=length(cross(u,v));let normal=scale(cross(u,v),1.0/area);
            let face=add(corners[0],scale(source.edges[axis],side));
            for y in 0..divisions {for x in 0..divisions {
                let q=add(face,add(scale(u,(x as f64+0.5)/divisions as f64),scale(v,(y as f64+0.5)/divisions as f64)));
                let delta=sub(q,p);let r=length(delta);let direction=scale(delta,1.0/r);
                let weight=dot(normal,direction).abs()*area/(divisions*divisions)as f64/(r*r)/(4.0*std::f64::consts::PI);
                result[0]+=weight;for i in 0..3 {result[i+1]+=2.0*direction[i]*weight;}
            }}
        }
        result
    }

    #[test]
    fn exact_tiles_match_dense_area_projection_for_one_two_and_three_visible_faces() {
        let e=emitter(7.0);
        for (p,faces) in [([3.0,0.0,0.0],1),([3.0,3.0,0.0],2),([3.0,3.0,3.0],3)] {
            let pos=position(p);let s=Samples::build(&[e],&pos,Vec3f::default(),config());
            assert_eq!((s.count,s.width,s.fallbacks,s.probe_fallbacks),(1,26,0,0));
            assert_eq!(&s.data[..4],&pos);assert_eq!(&s.data[4..8],&[2.0,3.0,4.0,7.0]);
            let mut active=0;
            for sample in s.data[8..].chunks_exact(8) {
                if sample[3]>0.0 {
                    active+=1;assert!((length([sample[0]as f64,sample[1]as f64,sample[2]as f64])-1.0).abs()<1e-6);
                    assert!(sample[4]>0.0);
                } else { assert!(sample.iter().all(|&v|v==0.0)); }
            }
            assert_eq!(active,faces*4);
            assert_close(packed_coefficients(&s,0,0),dense_reference(e,p,192),2e-7);
        }
    }

    #[test]
    fn mirrored_rotated_nonuniform_box_keeps_signed_directional_integrals() {
        let mut e=emitter(1.0);let angle=0.63_f32;let(co,si)=(angle.cos(),angle.sin());
        e.box_.transform.v=[-2.0*co,-2.0*si,0.0,0.0,-1.5*si,1.5*co,0.0,0.0,0.0,0.0,0.3,0.0,1.2,-0.7,2.0,1.0];
        let p=[4.0,3.0,5.0];let s=Samples::build(&[e],&position(p),Vec3f::default(),config());
        assert_eq!(s.probe_fallbacks,0);
        let coefficients=packed_coefficients(&s,0,0);
        assert!(coefficients[0]>0.0);assert!(coefficients[1]<0.0 && coefficients[3]<0.0);
        assert_close(coefficients,dense_reference(e,p,256),3e-7);
    }

    #[test]
    fn thin_fixture_panel_side_is_integrated_without_oct_ray_aliasing() {
        let mut e=emitter(3.0);e.box_.min=vec3f(-0.05,-1.5,-1.5);e.box_.max=vec3f(0.05,1.5,1.5);
        e.box_.transform.v[12]=-5.7;e.box_.transform.v[13]=2.5;e.box_.transform.v[14]=-2.0;
        let p=[-4.9905,2.6055,-4.499];let s=Samples::build(&[e],&position(p),vec3f(-7.9905,-1.8945,-7.7715),config());
        assert_eq!(s.probe_fallbacks,0);
        let side_dc=(4..8).map(|i|s.data[8+i*8+4]as f64).sum::<f64>();
        assert!(side_dc>0.001,"the thin side must not disappear between fixed rays");
        assert_close(packed_coefficients(&s,0,0),dense_reference(e,p,384),2e-6);
    }

    #[test]
    fn invalid_inside_on_face_and_partly_out_of_range_probe_pairs_fall_back_wholly() {
        let e=emitter(2.0);let mut c=config();c.ray_distance=4.0;
        let positions=[0.0,0.0,0.0,1.0, 0.5,0.0,0.0,1.0, 0.5,0.5,0.5,1.0,
            0.0,0.0,3.9,1.0, 0.0,0.0,3.0,0.0, 0.0,0.0,3.0,-1.0, 0.0,0.0,3.0,1.0];
        let s=Samples::build(&[e],&positions,Vec3f::default(),c);
        assert_eq!(s.count,1);assert_eq!(s.fallbacks,0);assert_eq!(s.probe_fallbacks,6);
        for row in 0..7 {
            let start=row*s.width*4;
            assert_eq!(&s.data[start..start+4],&positions[row*4..row*4+4]);
            if row<6 {assert!(s.data[start+4..start+s.width*4].iter().all(|&v|v==0.0));}
            else {assert_eq!(s.data[start+7],2.0);assert!(packed_coefficients(&s,row,0)[0]>0.0);}
        }
        for invalid in [[f32::NAN,0.0,3.0,1.0],[0.0,0.0,3.0,f32::INFINITY],[0.5005,-0.25,-0.25,1.0]] {
            let s=Samples::build(&[e],&invalid,Vec3f::default(),c);
            assert_eq!(s.probe_fallbacks,1);assert!(s.data[4..].iter().all(|&v|v==0.0));
        }
    }

    #[test]
    fn selection_is_capped_stable_range_culled_and_separate_from_probe_fallbacks() {
        let a=emitter(9.0);let mut b=emitter(2.0);b.box_.emission=vec3f(20.0,-3.0,5.0);
        let mut c=b;c.id=1.0;let mut distant=b;distant.id=99.0;distant.box_.transform.v[12]=1000.0;
        let mut dark=b;dark.id=100.0;dark.box_.emission=Vec3f::default();
        let positions=[3.0,3.0,3.0,1.0,3.0,3.0,3.0,-1.0];
        let s=Samples::build(&[a,b,c,distant,dark],&positions,vec3f(-0.75,-0.75,-0.75),config());
        assert_eq!((s.count,s.width,s.fallbacks,s.probe_fallbacks),(2,51,1,2));
        assert_eq!(s.data.len(),2*51*4);
        assert_eq!(&s.data[4..8],&[8.0,0.0,5.0,2.0]);assert_eq!(&s.data[8..12],&[8.0,0.0,5.0,1.0]);
        assert_close(packed_coefficients(&s,0,0),packed_coefficients(&s,0,1),1e-10);
        assert!(s.data[51*4+4..].iter().all(|&v|v==0.0));
        let empty=Samples::build(&[],&positions,Vec3f::default(),config());
        assert_eq!((empty.count,empty.width,empty.fallbacks,empty.probe_fallbacks),(0,1,0,0));
        assert_eq!(empty.data,positions);
    }

    #[test]
    fn invalid_source_geometry_or_ids_preserve_the_ordinary_estimator() {
        let base=emitter(1.0);let mut invalid=Vec::new();
        let mut e=base;e.box_.min.x=e.box_.max.x;invalid.push(e);
        let mut e=base;e.box_.transform.v[0]=0.0;invalid.push(e);
        let mut e=base;e.box_.transform.v[12]=f32::NAN;invalid.push(e);
        let mut e=base;e.box_.emission.x=f32::INFINITY;invalid.push(e);
        for id in [-1.0,0.0,0.5,f32::NAN] {let mut e=base;e.id=id;invalid.push(e);}
        for e in invalid {
            let s=Samples::build(&[e],&position([3.0,0.0,0.0]),Vec3f::default(),config());
            assert_eq!((s.count,s.width,s.fallbacks,s.probe_fallbacks),(0,1,1,0));
        }
        assert!(integrate_quad([0.0;3],[[1.0,0.0,0.0];4]).is_none());
    }
}
