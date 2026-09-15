//! Release-only probe-ray baseline, not an end-to-end GI or Quest benchmark.
use makepad_draw::*;
use makepad_raytrace::{building::building, bvh::{Bvh, Tri}};

fn main() {
    for (floors, bays) in [(1, 2), (4, 6), (8, 10)] {
        let scene = building(floors, bays);
        let tris: Vec<_> = scene.indices.chunks_exact(3).map(|i| {
            let p = |i:u32| { let p=scene.positions[i as usize]; vec3f(p[0],p[1],p[2]) };
            Tri { v0:p(i[0]), v1:p(i[1]), v2:p(i[2]) }
        }).collect();
        let build = std::time::Instant::now();
        let bvh = Bvh::build(&tris);
        let build_ms = build.elapsed().as_secs_f64()*1e3;
        let rays:Vec<_>=(0..2048).map(|i| {
            let origin=vec3f((i%17)as f32*0.6-5.0,1.2+(i%floors)as f32*3.6,(i%11)as f32-5.0);
            let y=1.0-2.0*((i%64)as f32+0.5)/64.0;
            let r=(1.0-y*y).sqrt(); let phi=(i%64)as f32*2.3999632;
            (origin,vec3f(phi.cos()*r,y,phi.sin()*r))
        }).collect();
        let start=std::time::Instant::now(); let mut hits=0; let mut invalid=0;
        for _ in 0..32 { for &(p,d) in &rays {
            let h=bvh.trace(std::hint::black_box(p),std::hint::black_box(d),40.0,false);
            hits+=h.is_hit()as usize; invalid+=h.truncated as usize;
        }}
        println!("triangles={} build_ms={build_ms:.2} 2048_probe_rays_ms={:.3} hits={hits} invalid={invalid}; CPU intersections only, no lighting/shadow queries/upload",tris.len(),start.elapsed().as_secs_f64()*1e3/32.0);
    }
}
