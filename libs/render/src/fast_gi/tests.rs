use super::*;

#[test]
fn startup_waits_for_three_complete_sweeps_not_three_batches() {
    let count = 11;
    let budget = 4;
    let mut cursor = 0;
    let mut startup = GiStartup::default();
    for frame in 0..9 {
        let batch = budget.min(count - cursor);
        cursor = (cursor + batch) % count;
        assert_eq!(startup.advance(cursor == 0, frame as f64), 0.0);
        assert_eq!(startup.sweeps, (frame + 1) / 3);
        assert_eq!(startup.fade_started.is_some(), frame == 8);
    }
    assert_eq!(startup.advance(false, 8.0 + STARTUP_FADE_SECONDS), 1.0);
    for _ in 0..10 { assert_eq!(startup.advance(true, 20.0), 1.0); }
    assert_eq!(startup.sweeps, STARTUP_SWEEPS, "steady state does not restart the fade");
}

#[test]
fn startup_fade_is_time_based_monotonic_and_has_soft_endpoints() {
    let mut startup = GiStartup::default();
    for _ in 0..STARTUP_SWEEPS { startup.advance(true, 10.0); }
    let mut previous = 0.0;
    for step in 0..=100 {
        let now = 10.0 + STARTUP_FADE_SECONDS * step as f64 / 100.0;
        let blend = startup.advance(false, now);
        assert!(blend >= previous && blend <= 1.0);
        previous = blend;
    }
    assert!((startup.advance(false, 10.0 + STARTUP_FADE_SECONDS * 0.5) - 0.5).abs() < 1e-6);
    assert!(startup.advance(false, 10.0 + STARTUP_FADE_SECONDS * 0.01) < 0.001);
    assert!(startup.advance(false, 10.0 + STARTUP_FADE_SECONDS * 0.99) > 0.999);
    let mut sparse = GiStartup::default();
    for _ in 0..STARTUP_SWEEPS { sparse.advance(true, 10.0); }
    assert_eq!(sparse.advance(false, 10.6), startup.advance(false, 10.6), "frame count does not set fade duration");
    assert_eq!(sparse.advance(false, 100.0), 1.0);
}

#[test]
fn startup_display_fade_does_not_change_transport_and_resets_with_history() {
    let mut gi = FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.traced = 1;
    gi.stats.ready_probes = 1;
    assert_eq!(gi.display_strength(), 0.0, "first batch is not presented");
    assert_eq!(gi.config().feedback, 0.85, "transport stays enabled while hidden");
    for _ in 0..STARTUP_SWEEPS { gi.startup.advance(true, 0.0); }
    gi.stats.display_blend = gi.startup.advance(false, STARTUP_FADE_SECONDS * 0.5);
    gi.stats.startup_sweeps = gi.startup.sweeps;
    assert_eq!(gi.display_strength(), 0.5);
    gi.set_debug(GiDebug::ProbeState);
    assert_eq!(gi.display_strength(), 1.0, "diagnostics may inspect unfinished fields");
    gi.set_debug(GiDebug::Off);
    gi.set_config(GiConfig { strength: 0.5, feedback: 0.0, ..gi.config() });
    assert_eq!(gi.display_strength(), 0.25, "lighting controls preserve startup progress");
    gi.reset_progress(); // Scene submission and origin shifts use the same reset.
    assert_eq!(gi.traced, 0);
    assert_eq!(gi.stats.startup_sweeps, 0);
    assert_eq!(gi.stats.ready_probes, 0);
    assert_eq!(gi.display_strength(), 0.0);
    assert!(gi.startup.fade_started.is_none());
    gi.set_mode(GiMode::Off);
    assert_eq!(gi.stats.display_blend, 0.0);
    assert_eq!(gi.stats.resident_bytes, 0);
}

fn oct_cell(mut x:i32,mut y:i32)->(i32,i32) {
    if x<0 {x=-1-x;y=7-y;}
    if x>7 {x=15-x;y=7-y;}
    if y<0 {x=7-x;y=-1-y;}
    if y>7 {x=7-x;y=15-y;}
    (x,y)
}

fn oct_ray(r:usize)->(Vec3f,f32) {
    let x=(r%8)as f32*0.25-0.875;
    let y=(r/8)as f32*0.25-0.875;
    let z=1.0-x.abs()-y.abs();
    let d=if z<0.0 {vec3f((1.0-y.abs())*x.signum(),(1.0-x.abs())*y.signum(),z)}else{vec3f(x,y,z)};
    let len=d.length();
    (d/len,1.0/(len*len*len))
}

// CPU oracle for the small GPU update filter, not a runtime ray path.
fn filtered_moments(distances:&[f32;64],r:usize,spacing:f32)->[f32;2] {
    let center=oct_ray(r).0;
    let mut moments=[0.0;2];
    let mut total=0.0;
    for y in -1..=1 {for x in -1..=1 {
        let (u,v)=oct_cell((r%8)as i32+x,(r/8)as i32+y);
        let s=(u+v*8)as usize;
        let (direction,solid_angle)=oct_ray(s);
        if distances[s]<0.0{continue;}
        let weight=center.dot(direction).max(0.0).powi(16)*solid_angle;
        let d=distances[s].min(spacing*2.0);
        moments[0]+=d*weight;moments[1]+=d*d*weight;total+=weight;
    }}
    if total<=0.000001 {return [-1.0,0.0];}
    [moments[0]/total,moments[1]/total]
}

#[test]
fn invalid_rays_are_excluded_not_clamped_to_occluders() {
    let mut d=[1.25;64];
    for invalid in 0..6 {
        d[invalid]=-1.0;
        for r in 0..64 {
            let m=filtered_moments(&d,r,1.5);
            if m[0]>=0.0 {assert!((m[0]-1.25).abs()<1e-5);assert!((m[1]-1.5625).abs()<1e-5);}
        }
    }
    for r in 0..64 {assert_eq!(filtered_moments(&[-2.0;64],r,1.5),[-1.0,0.0]);}
    // DC projection normalizes only accepted solid angles.
    let mut sum=0.0;let mut weight=0.0;
    for r in 0..64 {if d[r]>=0.0 {let w=oct_ray(r).1;sum+=0.75*w;weight+=w;}}
    assert!((sum/weight-0.75_f32).abs()<1e-6);
}

#[test]
fn distance_filter_preserves_constants_and_bounds_sky_variance() {
    for spacing in [0.5,1.5,8.0] {
        for r in 0..64 {
            let m=filtered_moments(&[spacing;64],r,spacing);
            assert!((m[0]-spacing).abs()<1e-5);
            assert!((m[1]-spacing*spacing).abs()<1e-4);
            let sky=filtered_moments(&[100.0;64],r,spacing);
            assert!((sky[0]-spacing*2.0).abs()<1e-4);
            assert!((sky[1]-spacing*spacing*4.0).abs()<1e-3);
        }
    }
    let mut mixed=[0.02;64];mixed[36]=32.0;
    let m=filtered_moments(&mixed,36,1.5);
    assert!(m[1]-m[0]*m[0]>0.01,"store E[d²], not the square of filtered distance");
}

#[test]
fn filtered_near_wall_moments_still_reject_receivers_behind_it() {
    // A probe 2.85 cm in front of a plane, as at the fixture's back wall.
    // Rays parallel to/away from the wall miss; they must not wash out its
    // visibility when the distance field is filtered around the -Z pole.
    let mut distances=[32.0;64];
    for (r,d) in distances.iter_mut().enumerate() {
        let z=oct_ray(r).0.z;
        if z<0.0 {*d=0.0285/(-z);}
    }
    for r in [0,7,56,63] {
        let m=filtered_moments(&distances,r,1.5);
        let variance=(m[1]-m[0]*m[0]).max(0.0001);
        let delta=(0.4_f32-m[0]-0.075).max(0.0);
        let visibility=(variance/(variance+delta*delta)).powi(2);
        assert!(visibility<0.001,"filtered wall leaks at bin {r}: {visibility}");
    }
}

#[test]
fn facing_weight_has_no_pinprick_at_a_near_surface_probe() {
    let facing=|p:Vec3f,spacing:f32| {
        ((p.z/p.length().max(spacing*0.25))*0.5+0.5).clamp(0.0,1.0).powi(2)
    };
    for spacing in [0.5,1.5,8.0] {
        let z=spacing*0.019; // Fixture: probe 0.0285 m in front of the wall.
        let center=facing(vec3f(0.0,0.0,z),spacing);
        for i in -10..=10 {
            let side=facing(vec3f(spacing*i as f32*0.01,0.0,z),spacing);
            assert!((side-center).abs()<1e-6,"near-probe angle must not stamp a dark point");
        }
        assert_eq!(facing(Vec3f::default(),spacing),0.25);
        assert_eq!(facing(vec3f(0.0,0.0,spacing),spacing),1.0);
        assert_eq!(facing(vec3f(0.0,0.0,-spacing),spacing),0.0);
    }
}

#[test]
fn config_bounds_work_and_memory() {
    let c=GiConfig{grid:[usize::MAX,0,3],spacing:f32::NAN,probes_per_frame:usize::MAX,ray_distance:f32::INFINITY,strength:-1.0,anchor:Some(vec3f(f32::NAN,0.0,0.0)),feedback:2.0}.clamped();
    assert_eq!(c.grid,[16,2,3]);assert_eq!(c.probes_per_frame,256);assert_eq!(c.spacing,2.0);assert_eq!(c.strength,0.0);
    assert_eq!(GiConfig::default().probe_count(),1152);
    assert_eq!(GiConfig::default().probes_per_frame * RAYS, 2048);
    assert_eq!(PROBE_TEXELS, 8 + RAYS / 2);
    assert_eq!(c.anchor,None);assert_eq!(c.feedback,0.95);
    assert!(MAX_TRIANGLES * 5 <= DATA_WIDTH * 2048);
}

#[test]
fn octahedral_visibility_neighbors_fold_at_every_edge() {
    // Mirror the shader's integer texel mapping, including corner crossings.
    let wrap=oct_cell;
    for y in -1..=8 {for x in -1..=8 {
        let (u,v)=wrap(x,y);
        assert!((0..8).contains(&u) && (0..8).contains(&v));
        if (0..8).contains(&x) && (0..8).contains(&y) {assert_eq!((u,v),(x,y));}
        // Both packed moment pairs always address a real probe texel.
        assert!((4+(u+8*v)/2) < PROBE_TEXELS as i32);
    }}
    for i in 0..8 {
        assert_eq!(wrap(-1,i),(0,7-i));
        assert_eq!(wrap(8,i),(7,7-i));
        assert_eq!(wrap(i,-1),(7-i,0));
        assert_eq!(wrap(i,8),(7-i,7));
    }
    assert_eq!(wrap(-1,-1),(7,7));
    assert_eq!(wrap(8,8),(0,0));
}

#[test]
fn trilinear_sampling_preserves_constant_irradiance_across_cell_faces() {
    // Visibility/facing reweight all eight corners; normalization must keep
    // constant light constant, including where one corner becomes invalid.
    for step in 0..=100 {
        let f=[step as f32/100.0,0.37,0.61];
        let mut sum=0.0;
        let mut reweighted=0.0;
        let mut irradiance=0.0;
        for i in 0..8 {
            let w=(0..3).map(|a|if i&(1<<a)==0{1.0-f[a]}else{f[a]}).product::<f32>();
            sum+=w;
            let visible=if i==2{0.0}else{(i+1)as f32/8.0};
            reweighted+=w*visible;
            irradiance+=w*visible*0.7;
        }
        assert!((sum-1.0).abs()<1e-6);
        assert!((irradiance/reweighted-0.7).abs()<1e-6);
    }
}
#[test]
fn inactive_nearest_probe_does_not_create_a_black_irradiance_streak() {
    let smooth=|x:f32| {let t=(x/0.025).clamp(0.0,1.0);t*t*(3.0-2.0*t)};
    // GI fixture: x=1 is the cube face; its nearest probe is x=1.0095,
    // INSIDE the cube and correctly invalid. Remaining support is 0.00633.
    let available=0.0095_f32/1.5;
    assert!(smooth(available)<0.2,"old absolute confidence crushes visible GI");
    for available in [available,0.0001,0.01,0.1,1.0] {
        assert_eq!(smooth(available/available),1.0);
        // Renormalization does not unshadow genuinely occluded probes.
        assert!(smooth((available*0.00001)/available)<0.000001);
    }
    assert_eq!(smooth(0.0/0.00000001),0.0,"an empty cell still uses fallback");
    let shifted_available=(0.0095+1.5*0.1)/1.5;
    assert!(shifted_available>0.1,"air-side interpolation avoids near-zero support");
}
#[test]
fn display_dither_preserves_mean_and_stays_within_one_quantization_step() {
    for code in [0.0_f32,0.2,8.25,42.4,80.75,128.3,254.8,255.0] {
        let value=code/255.0;
        let mut mean=0.0;
        for i in 0..1024 {
            let noise=((i as f32+0.5)/1024.0-0.5)/255.0;
            let quantized=((value+noise).clamp(0.0,1.0)*255.0).round()/255.0;
            assert!((quantized-value).abs()<=1.0/255.0+1e-6);
            mean+=quantized/1024.0;
        }
        assert!((mean-value).abs()<0.00001);
    }
}
#[test]
fn off_allocates_nothing_and_transitions_discard_history() {
    let mut gi=FastGi::default();assert!(!gi.needs_scene((0,0,0)));assert!(gi.gpu.is_none());assert!(gi.job.is_none());assert_eq!(gi.stats.resident_bytes,0);
    gi.set_mode(GiMode::Fast);assert!(gi.needs_scene((0,0,0)));
    gi.reject((0,0,0),"test rejection");assert!(!gi.needs_scene((0,0,0)));assert!(gi.needs_scene((1,0,0)));
    gi.set_mode(GiMode::Off);assert!(!gi.stats.rejected_scene);assert!(gi.revision.is_none());assert!(gi.pending.is_none());
}
#[test]
fn feedback_control_preserves_geometry_but_anchor_changes_invalidate_it() {
    let mut gi=FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.revision=Some((1,2,3));gi.traced=1152;gi.cursor=64;
    gi.set_config(GiConfig{feedback:0.0,strength:0.5,..gi.config()});
    assert_eq!(gi.revision,Some((1,2,3)));assert_eq!(gi.traced,1152);assert_eq!(gi.cursor,64);
    gi.set_config(GiConfig{anchor:Some(vec3f(0.0,2.0,0.0)),..gi.config()});
    assert!(gi.revision.is_none());assert_eq!(gi.traced,0);
}
#[test]
fn diffuse_feedback_converges_and_decays_without_new_rays() {
    // Constant-radiance enclosure: normalized SH sampling is identity.
    // This checks transport gain/decay, not general-scene leak freedom.
    let gain=0.9*GiConfig::default().feedback;
    assert!(gain<1.0);
    let mut radiance=0.0_f32;
    for _ in 0..100 {radiance=0.2+gain*radiance;}
    assert!((radiance-0.2/(1.0-gain)).abs()<1e-5);
    for _ in 0..100 {radiance*=gain;}
    assert!(radiance<1e-6);
    assert_eq!(GiConfig::default().probes_per_frame*RAYS,2048);
}
#[test]
fn receiver_bias_never_crosses_a_nearby_visible_probe() {
    let n=vec3f(0.0,0.0,1.0);
    for spacing in [0.5_f32,1.5,8.0] {
        for d in [0.0001_f32,0.0095,0.0285,0.04,0.1,1.0] {
            let probe=n*d;
            let bias=(spacing*0.03).min(probe.length()*0.25);
            let delta=n*bias-probe;
            assert!(delta.length()>0.0);
            assert!((-delta.normalize()).dot(n)>0.9999);
        }
    }
    // The fixture's back-wall probe was 0.0285m from its receiver;
    // the former 0.045m bias crossed it, flipping its normal-facing weight.
    assert!(1.5*0.03>0.0285);
}
pub(super) fn cube() -> Instance {
    let(vertices,indices)=crate::geometry::shape_geometry_data(makepad_scene::Shape::Box);
    Instance{mesh:Arc::new(Mesh{vertices,indices,stride:12,color_lane:-1,image:None}),transform:Mat4f::identity(),tint:vec3f(0.8,0.1,0.2),emission:vec3f(0.5,0.0,0.0),diffuse:1.0,exact_box:true}
}

#[test]
fn traversal_headroom_keeps_default_worst_case_budget() {
    for nodes in [0,32,256,257,511,1024,2048,100000] {
        for probes in [1,4,8,32,256] {
            let (batch,limit)=trace_budget(probes,nodes);
            assert!((256..=2048).contains(&limit));assert!(batch>=1);
            assert!(batch*limit<=(probes*256).max(2048));
        }
    }
    assert_eq!(trace_budget(32,100000),(4,2048));
    assert_eq!(trace_budget(32,32),(32,256));
}

#[test]
fn targeted_emitter_rays_share_the_existing_frame_budget() {
    for emitters in 0..=emitters::MAX_EMITTERS {
        let width=RAYS+emitters*emitters::SAMPLES_PER_EMITTER;
        for probes in [1,4,8,32,256] {
            let relight=emitter_probe_budget(probes,width);
            assert!(relight*width<=(probes*RAYS).max(width));
            for nodes in [0,256,257,1024,2048,100000] {
                let (batch,limit)=trace_budget(relight,nodes);
                assert!(batch*width*limit<=(probes*RAYS*256).max(width*limit));
            }
        }
    }
    assert_eq!(emitter_probe_budget(32,64),32);
    assert_eq!(emitter_probe_budget(32,76),26);
    assert_eq!(emitter_probe_budget(32,88),23);
}

#[test]
fn emissive_source_ids_survive_bvh_reordering_without_tagging_mesh_bounds() {
    let first=cube();
    let mut second=cube();second.transform.v[12]=10.0;second.emission=vec3f(0.0,2.0,0.0);
    let mut arbitrary=cube();arbitrary.transform.v[12]=20.0;arbitrary.exact_box=false;
    let mut dark=cube();dark.transform.v[12]=30.0;dark.emission=Vec3f::default();
    let p=Prepared::build(vec![first,second,arbitrary,dark]).unwrap();
    assert_eq!(p.emitters.len(),2);
    assert_eq!(p.static_boxes.len(),3);
    assert_eq!(p.count,48,"all triangles still occlude");
    for t in p.triangles[..p.count*20].chunks_exact(20) {
        let x=(t[0]+t[4]+t[8])/3.0;
        let expected=if x<5.0{1.0}else if x<15.0{2.0}else{0.0};
        assert_eq!(t[19],expected);
        if expected==2.0 {assert_eq!(&t[16..19],&[0.0,2.0,0.0]);}
    }
}

#[test]
fn targeted_emission_is_separate_from_visibility_moments_and_reflection() {
    let shader=include_str!("shaders.rs");
    assert!(shader.contains("if self.emitter_sampled(index,source.w) {emission=vec3(0.0,0.0,0.0)}"));
    assert!(shader.contains("albedo=self.tri(at+3.0).xyz"));
    assert!(shader.contains("if id<=0.0 {return false}"));
    assert!(shader.contains("hit.y!=0.0 || hit.w<0.5"),"blocked or exhausted target is not light");
    assert!(shader.contains("result=result+self.emitter_sh(index,target)*value"));
    assert!(shader.contains("var result=coefficients/max(weight,0.0001)"));
    assert!(shader.contains("while r<64.0"),"classification and moments keep their 64-ray basis");
    assert_eq!(PROBE_TEXELS,40,"no forward-field sampler/layout growth");
}

#[test]
fn cell_blockers_cover_relocated_endpoints_and_never_drop_overflow() {
    let point=vec3f(-0.44,0.5,0.5);
    let bounds=[(point,point);5];
    assert_eq!(cell_blockers(Vec3f::default(),1.0,&bounds[..4]),[0.0,1.0,2.0,3.0]);
    assert_eq!(cell_blockers(Vec3f::default(),1.0,&bounds),[-2.0,-1.0,-1.0,-1.0]);
    assert_eq!(cell_blockers(vec3f(10.0,0.0,0.0),1.0,&bounds),[-1.0;4]);
    // OBB broad phase must include rotated/translated corners, not just two
    // transformed extrema (which can invert/shrink under rotation).
    let mut m=Mover{transform:Mat4f::identity(),min:vec3f(-1.0,-2.0,-3.0),max:vec3f(1.0,2.0,3.0),color:Vec3f::default(),emission:Vec3f::default(),exact_box:true};
    m.transform.v[0]=0.0;m.transform.v[2]=1.0;m.transform.v[8]=-1.0;m.transform.v[10]=0.0;m.transform.v[12]=5.0;
    let (lo,hi)=mover_bounds(&m);assert_eq!(lo,vec3f(2.0,-2.0,-1.0));assert_eq!(hi,vec3f(8.0,2.0,1.0));
}

fn segment_hits_box(p:Vec3f,q:Vec3f,lo:Vec3f,hi:Vec3f)->bool {
    let mut near=0.00001_f32;let mut far=0.99999_f32;
    for (p,q,lo,hi) in [(p.x,q.x,lo.x,hi.x),(p.y,q.y,lo.y,hi.y),(p.z,q.z,lo.z,hi.z)] {
        let d=q-p;let inv=if d<0.0{-1.0}else{1.0}/d.abs().max(0.0000001);
        let a=(lo-p)*inv;let b=(hi-p)*inv;
        near=near.max(a.min(b));far=far.min(a.max(b));
    }
    near<far
}

fn record_point(record:&[f32;24],p:Vec3f)->Vec3f {
    let row=|i:usize|record[i]*p.x+record[i+1]*p.y+record[i+2]*p.z+record[i+3];
    vec3f(row(0),row(4),row(8))
}

fn record_blocks_receiver(record:&[f32;24],probe:Vec3f,receiver:Vec3f)->bool {
    // CPU oracle for the packed gather -> forward visibility contract.
    // Zero rows replace only the visibility copy of an approximate mover.
    let mut field=*record;
    if field[15]<=0.5 {field[..12].fill(0.0);}
    if field[..3].iter().all(|v|*v==0.0){return false;}
    segment_hits_box(record_point(&field,probe),record_point(&field,receiver),vec3f(-1.0,-1.0,-1.0),vec3f(1.0,1.0,1.0))
}

#[test]
fn actor_bounds_cannot_blacken_the_ground_or_face_but_solid_boxes_still_block() {
    // A whole posed-character AABB spans the empty space between its feet,
    // hands and face. Ground and eye receivers are inside that box, despite
    // being exposed to air. Treating the AABB as solid rejects every probe.
    let actor=Mover{transform:Mat4f::identity(),min:vec3f(-1.2,-0.04,-0.7),max:vec3f(1.2,2.3,0.7),color:vec3f(0.5,0.5,0.5),emission:Vec3f::default(),exact_box:false};
    let bounds=mover_record(&actor);
    let solid=mover_record(&Mover{exact_box:true,..actor});
    assert_eq!(&bounds[..15],&solid[..15],"approximate transport geometry/albedo is retained");
    assert_eq!(&bounds[16..],&solid[16..]);
    assert_eq!(bounds[15],0.0);assert_eq!(solid[15],1.0);
    for receiver in [vec3f(0.6,0.0015,0.3),vec3f(0.3,1.7,0.55)] {
        for x in [-1.5,1.5] {for y in [0.5,3.0] {for z in [-1.5,1.5] {
            let probe=vec3f(x,y,z);
            assert!(record_blocks_receiver(&solid,probe,receiver),"the old solid-bounds path reproduces complete darkness");
            assert!(!record_blocks_receiver(&bounds,probe,receiver),"empty bounds are not geometric occlusion");
        }}}
    }
    let floor=mover_record(&Mover{min:vec3f(-6.0,-0.16,-6.0),max:vec3f(6.0,0.0,6.0),exact_box:true,..actor});
    let receiver=vec3f(0.6,0.0015,0.3);
    assert!(record_blocks_receiver(&floor,vec3f(0.6,-1.0,0.3),receiver),"below-floor probes remain blocked");
    assert!(!record_blocks_receiver(&floor,vec3f(0.6,1.0,0.3),receiver));
    // A transformed thin solid door keeps its exact hard gate, even when
    // the same cell includes an approximate character record.
    let mut door=Mover{min:vec3f(-0.08,-1.0,-0.5),max:vec3f(0.08,1.0,0.5),exact_box:true,..actor};
    door.transform.v[0]=0.0;door.transform.v[2]=1.0;
    door.transform.v[8]=-1.0;door.transform.v[10]=0.0;door.transform.v[13]=1.0;
    let door=mover_record(&door);
    assert!(record_blocks_receiver(&door,vec3f(0.0,1.0,-0.5),vec3f(0.0,1.0,0.5)));
    assert!(!record_blocks_receiver(&door,vec3f(0.0,1.0,0.3),vec3f(0.0,1.0,0.5)));
    let sampling=include_str!("shaders.rs");
    assert!(sampling.contains("if dot(a.xyz,a.xyz)>0.0"),"forward gate must honor the zero-row marker");
    assert!(sampling.contains("index<self.gi_blocker_count && self.mover(index*6.0+3.0).w>0.5"),"gather must keep approximate bounds out of hard visibility");
}

#[test]
fn air_probe_inside_actor_bounds_is_not_classified_as_inside_solid_geometry() {
    let actor=Mover{transform:Mat4f::identity(),min:vec3f(-1.2,-0.04,-0.7),max:vec3f(1.2,2.3,0.7),color:vec3f(0.5,0.5,0.5),emission:Vec3f::default(),exact_box:false};
    let probe=vec3f(0.8,0.3,0.3); // Air between the character's hand and foot.
    for exact_box in [false,true] {
        let record=mover_record(&Mover{exact_box,..actor});
        let p=record_point(&record,probe);
        let inside=p.x.abs().max(p.y.abs()).max(p.z.abs())<0.999;
        assert!(inside);
        assert_eq!(inside&&record[15]>0.5,exact_box,"only exact solids invalidate a probe");
        assert_eq!(!inside||record[15]>0.5,exact_box,"emitter occlusion must not start inside an approximate proxy");
    }
    let shader=include_str!("shaders.rs");
    assert!(shader.contains("max(max(abs(p.x),abs(p.y)),abs(p.z))<0.999 && self.mover(i*6.0+3.0).w>0.5"));
    assert!(shader.contains("max(max(abs(p.x),abs(p.y)),abs(p.z))>=0.999 || self.mover(i*6.0+3.0).w>0.5"));
}

#[test]
fn thin_door_blocks_cross_wall_probe_segments_but_not_same_side_light() {
    let lo=vec3f(-3.175,0.0,-3.75);let hi=vec3f(-2.825,4.0,-0.25);
    let receiver=vec3f(-2.75,0.0015,-2.0);
    assert!(segment_hits_box(vec3f(-3.4905,1.1055,-1.7715),receiver,lo,hi));
    assert!(!segment_hits_box(vec3f(-1.9905,1.1055,-1.7715),receiver,lo,hi));
    let open=vec3f(0.0,0.0,5.0);
    assert!(!segment_hits_box(vec3f(-3.4905,1.1055,-1.7715),receiver,lo+open,hi+open));
    assert!(!segment_hits_box(vec3f(-4.0,4.5,-2.0),vec3f(-2.0,4.5,-2.0),lo,hi));
}

#[test]
fn static_room_wall_rejects_dark_exterior_probes_at_every_height() {
    let mut wall=cube();wall.transform.v[0]=0.4;wall.transform.v[5]=6.0;wall.transform.v[10]=10.0;
    wall.transform.v[12]=-6.0;wall.transform.v[13]=3.0;
    let prepared=Prepared::build(vec![wall]).unwrap();
    assert_eq!(prepared.static_boxes.len(),1);
    let (lo,hi)=mover_bounds(&prepared.static_boxes[0]);
    for y in [1.0,1.75,2.5,3.25,4.0,4.75,5.5] {
        let receiver=vec3f(-5.7,y,-4.8+0.0015);
        assert!(segment_hits_box(vec3f(-6.501,y,-4.499),receiver,lo,hi));
        assert!(!segment_hits_box(vec3f(-4.9905,y,-4.499),receiver,lo,hi));
        // The actual shaded red wall, biased toward room air, also remains
        // reachable from interior probes: no false self-occluding black edge.
        assert!(!segment_hits_box(vec3f(-4.9905,y,-4.499),vec3f(-5.8+0.0015,y,-4.0),lo,hi));
    }
}

#[test]
fn receiver_lists_offset_static_ids_without_changing_dynamic_ray_count() {
    assert_eq!(merge_blockers([0.0,-1.0,-1.0,-1.0],[0.0,1.0,-1.0,-1.0],1),[0.0,1.0,2.0,-1.0]);
    assert_eq!(merge_blockers([-1.0;4],[0.0,1.0,-1.0,-1.0],0),[0.0,1.0,-1.0,-1.0]);
    assert_eq!(merge_blockers([0.0,1.0,-1.0,-1.0],[0.0,1.0,2.0,-1.0],2),[-2.0,-1.0,-1.0,-1.0]);
    assert_eq!(merge_blockers([-1.0;4],[-2.0,-1.0,-1.0,-1.0],0),[-2.0,-1.0,-1.0,-1.0]);
    assert_eq!(MAX_BLOCKERS,64);assert_eq!(MAX_MOVERS,32);
}

#[test]
fn actor_count_changes_do_not_make_history_sample_the_current_blocker_bank() {
    for (previous,current) in [(0,8),(8,9),(9,8),(8,8),(64,32),(32,64)] {
        assert_eq!(producer_blocker_count(1,previous,current),previous,"relight reads last frame");
        assert_eq!(producer_blocker_count(2,previous,current),current,"gather writes this frame");
    }
}

#[test]
fn superseding_scene_submission_discards_an_unconsumed_preparation() {
    let mut gi=FastGi::default();gi.set_mode(GiMode::Fast);
    gi.revision=Some((1,0,0));gi.pending=Some(Arc::new(Prepared::build(vec![cube()]).unwrap()));
    gi.traced=1152;gi.stats.ready_probes=1152;gi.stats.display_blend=1.0;
    let old_generation=gi.generation;
    let generation=gi.begin_submission((2,0,0));
    assert_eq!(generation,old_generation+1);assert_eq!(gi.revision,Some((2,0,0)));
    assert!(gi.pending.is_none(),"ensure must not create the obsolete GPU scene");
    assert!(gi.gpu.is_none());assert_eq!(gi.stats.ready_probes,0);
    assert_eq!(gi.stats.display_blend,0.0);assert!(gi.stats.building);
}

#[test]
fn fixture_roof_really_casts_the_reported_floor_line_from_the_old_sun() {
    let mut roof=cube();
    roof.transform.v[0]=12.0;roof.transform.v[5]=0.4;roof.transform.v[10]=6.0;
    roof.transform.v[13]=6.0;roof.transform.v[14]=-2.0;
    let scene=Prepared::build(vec![roof]).unwrap();
    let sun=vec3f(0.4,0.8,0.2).normalize();
    // Roof underside y=5.8, front edge z=1: projection reaches z=-0.45.
    let shadowed=trace(&scene,vec3f(0.0,0.001,-0.7),sun,256);
    let lit=trace(&scene,vec3f(0.0,0.001,-0.2),sun,256);
    assert!(shadowed.1>0);assert_eq!(lit.1,0);
}

#[test]
fn diagnostics_do_not_reset_transport_or_allocate_off_resources() {
    let mut gi=FastGi::default();let generation=gi.generation;
    let mut debug=GiDebug::Off;
    for _ in 0..7 {debug=debug.next();gi.set_debug(debug);}
    assert_eq!(debug,GiDebug::Off);assert_eq!(gi.generation,generation);
    assert!(gi.gpu.is_none());assert!(gi.placement.is_none());
}
#[test]
fn raw_probe_debug_selects_the_largest_nominal_weight() {
    for x in [0.01_f32,0.49,0.51,0.99] {for y in [0.2_f32,0.8] {for z in [0.1_f32,0.9] {
        let f=[x,y,z];
        let chosen=(0..3).fold(0,|id,a|id|((f[a]>=0.5)as usize)<<a);
        let weights:Vec<f32>=(0..8).map(|id|(0..3).map(|a|if id&(1<<a)==0{1.0-f[a]}else{f[a]}).product()).collect();
        assert!(weights.iter().all(|&w|w<=weights[chosen]));
    }}}
    assert_eq!(GiDebug::ProbeState.next(),GiDebug::RawProbe);
    assert_eq!(GiDebug::RawProbe.next(),GiDebug::Off);
}
// Independent CPU transcription of the *new* bounded shader loop, compared
// below with analytic box intersections, not with the old tracer as oracle.
fn trace(p:&Prepared,ro:Vec3f,rd:Vec3f,steps_limit:usize)->(f32,i32,bool) {
    let read=|data:&[f32],at:usize|vec3f(data[at],data[at+1],data[at+2]);
    let inv=vec3f(1.0/rd.x.abs().max(1e-7)*if rd.x<0.0{-1.0}else{1.0},1.0/rd.y.abs().max(1e-7)*if rd.y<0.0{-1.0}else{1.0},1.0/rd.z.abs().max(1e-7)*if rd.z<0.0{-1.0}else{1.0});
    let mut out=(32.0,0,false);let mut node=0;let mut steps=0;
    while node<p.node_count && steps<steps_limit {
        steps+=1;
        let at=node*8;let code=p.nodes[at+3];let next=p.nodes[at+7] as usize;
        let a=mul(read(&p.nodes,at)-ro,inv);let b=mul(read(&p.nodes,at+4)-ro,inv);
        let near=a.x.min(b.x).max(a.y.min(b.y)).max(a.z.min(b.z)).max(0.0001);
        let far=a.x.max(b.x).min(a.y.max(b.y)).min(a.z.max(b.z)).min(out.0)*1.000001;
        if near<=far {
            if code<0.0 {for j in next..next+((-code) as usize).min(8) {
                let p0=read(&p.triangles,j*20);let e1=read(&p.triangles,j*20+4)-p0;let e2=read(&p.triangles,j*20+8)-p0;
                let h=Vec3f::cross(rd,e2);let det=e1.dot(h);
                if det.abs()>1e-8 {
                    let q=ro-p0;let u=q.dot(h)/det;let r=Vec3f::cross(q,e1);let v=rd.dot(r)/det;let t=e2.dot(r)/det;
                    if u>=-1e-6 && v>=-1e-6 && u+v<=1.000001 && t>0.001 && t<out.0{out=(t,j as i32+1,det<=0.0);}
                }
            }}
            node+=1;
        }else{node=if code<0.0{node+1}else{next};}
    }
    if node<p.node_count{(0.0,-1,false)}else{out}
}
#[test]
fn actual_game_cube_winding_hits_and_seams_match_analytic_box() {
    let p=Prepared::build(vec![cube()]).unwrap();assert_eq!(p.count,12);
    for direction in [vec3f(1.0,0.0,0.0),vec3f(-1.0,0.0,0.0),vec3f(0.0,1.0,0.0),vec3f(0.0,-1.0,0.0),vec3f(0.0,0.0,1.0),vec3f(0.0,0.0,-1.0),vec3f(1.0,1.0,1.0).normalize()] {
        let extent=direction.x.abs().max(direction.y.abs()).max(direction.z.abs());
        let hit=trace(&p,direction*3.0,direction*(-1.0),256);
        assert!(hit.1>0,"miss at {direction:?}");assert!((hit.0-(3.0-0.5/extent)).abs()<1e-4,"{hit:?}");assert!(!hit.2,"inverted actual mesh winding");
        let inside=trace(&p,Vec3f::default(),direction,256);assert!(inside.1>0 && inside.2,"inside probes must reject backfaces");
    }
    assert_eq!(trace(&p,vec3f(3.0,3.0,0.0),vec3f(1.0,0.0,0.0),256).1,0);
    assert_eq!(trace(&p,vec3f(3.0,0.0,0.0),vec3f(-1.0,0.0,0.0),0).1,-1,"exhaustion must never be a miss");
}
#[test]
fn packing_retains_materials_and_rejects_bad_inputs() {
    let p=Prepared::build(vec![cube()]).unwrap();
    for tri in p.triangles[..p.count*20].chunks_exact(20){assert!((tri[12]-0.8).abs()<1e-6);assert_eq!(tri[16],0.5);}
    let mut bad=cube();bad.transform.v[0]=f32::NAN;assert!(Prepared::build(vec![bad]).is_err());
    let mut bad=cube();Arc::make_mut(&mut bad.mesh).stride=1;assert!(Prepared::build(vec![bad]).is_err());
    let mut bad=cube();Arc::make_mut(&mut bad.mesh).indices[0]=u32::MAX;assert!(Prepared::build(vec![bad]).is_err());
    let empty=Prepared::build(Vec::new()).unwrap();assert_eq!(empty.node_count,0);assert_eq!(trace(&empty,Vec3f::default(),vec3f(0.0,1.0,0.0),256).1,0);
}

// CPU reference for receiver support accounting, not a second runtime path.
// Entry = (nominal trilinear weight, eligible, visibility^2*facing, light).
fn eligible_support_sample(probes:&[(f32,bool,f32,f32)])->(f32,f32,f32) {
    let(mut available,mut weight,mut light)=(0.0_f32,0.0_f32,0.0_f32);
    for &(nominal,eligible,attenuation,radiance) in probes {
        if !eligible || nominal<=0.0 {continue;}
        available+=nominal;
        let w=nominal*attenuation;weight+=w;light+=w*radiance;
    }
    let t=(weight/available.max(1e-8)/0.025).clamp(0.0,1.0);
    let confidence=t*t*(3.0-2.0*t);
    (available,confidence,light/weight.max(1e-8)*confidence)
}

#[test]
fn trihedral_corner_support_excludes_valid_but_unreachable_probes() {
    // Actual fixture: wall receiver 1 mm from the floor/red-wall corner.
    // Relocation leaves six valid probes outside the room or below its floor.
    let origin=vec3f(-7.9905,-1.8945,-7.7715);
    let receiver=vec3f(-5.799,0.001,-4.8);
    let endpoint=receiver+vec3f(0.0,0.0,0.0015);
    let local=(receiver+vec3f(0.0,0.0,0.15)-origin)/1.5;
    let f=[local.x.fract(),local.y.fract(),local.z.fract()];
    let positions=[
        vec3f(-6.4905,-0.9,-4.7715),vec3f(-4.9905,-0.9,-4.7715),
        vec3f(-6.501,1.1055,-4.7715),vec3f(-4.9905,1.1055,-4.499),
        vec3f(-6.4905,-0.9,-3.2715),vec3f(-4.9905,-0.9,-3.2715),
        vec3f(-6.501,1.1055,-3.2715),vec3f(-4.9905,1.1055,-3.2715),
    ];
    let boxes=[
        (vec3f(-7.0,-0.6,-7.0),vec3f(7.0,0.0,7.0)),
        (vec3f(-6.0,0.0,-5.2),vec3f(6.0,6.0,-4.8)),
        (vec3f(-6.2,0.0,-5.0),vec3f(-5.8,6.0,5.0)),
    ];
    let mut probes=Vec::new();
    for (i,p) in positions.into_iter().enumerate() {
        let nominal=(0..3).map(|a|if i&(1<<a)==0{1.0-f[a]}else{f[a]}).product::<f32>();
        let eligible=!boxes.iter().any(|&(lo,hi)|segment_hits_box(p,endpoint,lo,hi));
        assert_eq!(eligible,i==3||i==7);
        // Independent fixed-64 moment/SH oracle at these exact probes.
        let(attenuation,light)=match i {3=>(0.05100073,0.13071358),7=>(0.08838945,0.21130915),_=>(1.0,1000.0)};
        probes.push((nominal,eligible,attenuation,light));
    }
    assert!((probes.iter().map(|p|p.0).sum::<f32>()-1.0).abs()<1e-6);
    let(available,confidence,light)=eligible_support_sample(&probes);
    assert!((available-0.12155033).abs()<2e-6,"reachable support is not all valid support");
    assert_eq!(confidence,1.0);
    assert!((light-0.14139350).abs()<2e-6,"blocked bright probes must contribute nothing");
    let old_weight=probes.iter().filter(|p|p.1).map(|p|p.0*p.2).sum::<f32>();
    let t=(old_weight/0.025).clamp(0.0,1.0);
    assert!((t*t*(3.0-2.0*t)-0.17076463).abs()<2e-5,"old denominator reproduces the dark wedge");
}

#[test]
fn eligible_support_keeps_full_occlusion_and_partial_moment_uncertainty_dark() {
    assert_eq!(eligible_support_sample(&[(1.0,false,1.0,1000.0)]),(0.0,0.0,0.0));
    assert_eq!(eligible_support_sample(&[(0.0,true,1.0,1000.0)]),(0.0,0.0,0.0));
    for available in [0.0001_f32,0.12155033,0.5,1.0] {
        // Eligible is a geometric gate, not proof of moment visibility.
        let visibility=0.1_f32;let facing=0.5_f32;
        let probes=[(available,true,visibility*visibility*facing,0.8),(1.0-available,false,1.0,1000.0)];
        let(support,confidence,light)=eligible_support_sample(&probes);
        assert!((support-available).abs()<1e-7);
        assert!((confidence-0.104).abs()<1e-6);
        assert!((light-0.0832).abs()<1e-6,"uncertainty must still attenuate lighting");
        let(_,confidence,light)=eligible_support_sample(&[(available,true,0.00001,0.8)]);
        assert!(confidence<1e-6 && light<1e-6);
    }
}

#[test]
fn shader_reuses_one_exact_gate_for_irradiance_and_nominal_support() {
    let sampling=include_str!("shaders.rs").split("    let ProbeLayout = {").next().unwrap();
    assert_eq!(sampling.matches("self.gi_segment_clear(").count(),1,"one shared eligibility gate");
    assert_eq!(sampling.matches("self.gi_probe_eligible(").count(),8,"each corner is gated once");
    let probe=sampling.split("        gi_probe: fn").nth(1).unwrap().split("        gi_ambient: fn").next().unwrap();
    assert!(!probe.contains("gi_segment_clear"),"do not retrace every blocker for the numerator");
    assert!(probe.contains("if probe.w<0.5 || w<=0.0"));
    assert!(probe.contains("w*visibility*visibility*max(facing,0.005)"));
    assert!(sampling.contains("smoothstep(0.0,0.025,sum.w/max(available,0.00000001))"));
    for i in 0..8 {assert!(sampling.contains(&format!("step(0.5,p{i}.w)*w{i}")));}
    assert!(sampling.contains("let state=self.gi_probe_info(cell).w"),"raw diagnostics stay unfiltered");
}
