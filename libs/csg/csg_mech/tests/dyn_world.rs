use makepad_csg_mech::dyn_world::DynWorld;
use makepad_csg_mech::geom::*;
use makepad_csg_mech::joint::Profile;
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::sim::{AnchorKind, AnchorSpec, Assembly, BodyAction, BodySpec, Friction, JointKind, JointSpec, MechWorld, GRAVITY};
use makepad_csg_mech::weave::{self, Frame, SquareLink, Weave, WeaveProfile};
use std::time::Instant;

const DT: f64 = 1.0 / 240.0;

fn setup() -> (WeaveProfile, SquareLink, Frame) {
    let p = Profile::default();
    let wp = WeaveProfile::new(&p, None);
    (wp, SquareLink::new(&wp, None), Frame::new([0.0; 3], p.up))
}

fn linked(w: &Weave, world: &DynWorld, step: usize) {
    let poses = world.poses();
    let points: Vec<Vec<P3>> = w.links.iter().enumerate().map(|(i,l)| {
        l.pts.iter().map(|p| poses[i].apply(world.rest_com(i), *p)).collect()
    }).collect();
    for &(a,b) in &w.pairs {
        assert_eq!(weave::linking_number(&points[a as usize], &points[b as usize]).abs(), 1,
            "interlock {a}/{b} at step {step}");
    }
}

fn fixed(asm: &mut Assembly, body: usize) {
    asm.anchors.push(AnchorSpec { body, pivot: asm.bodies[body].mass.com, kind: AnchorKind::Fixed, friction: Friction::default() });
}

#[test]
fn hanging_chain_stays_linked() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0, 0.0], [40.0, 0.0]], false);
    assert!(!w.pairs.is_empty(), "chain must have interlocks");
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    fixed(&mut asm, 0);
    let mut world = DynWorld::new(&asm, scale(frame.y, -GRAVITY), None).unwrap();
    for step in 0..480 { world.step(DT); linked(&w, &world, step); }
    let last = w.links.len()-1;
    assert!(dot(sub(world.poses()[last].com, world.rest_com(last)), frame.y) < -1.0, "chain must hang");
    assert!((world.time()-2.0).abs()<1e-10, "time uses all substeps");
    println!("hanging chain: {:.3}s", start.elapsed().as_secs_f64());
}

#[test]
fn hanging_mail_stays_linked() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::mail(&wp, link, frame, [-12.0,-12.0], [12.0,12.0], 0.0, &|_| -1.0);
    assert!(w.pairs.len()>2, "mail must have multiple interlocks");
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    let top = w.links.iter().map(|l| dot(l.center(),frame.y)).fold(f64::NEG_INFINITY,f64::max);
    for (i,l) in w.links.iter().enumerate() {
        if dot(l.center(),frame.y)>top-1e-6 { fixed(&mut asm,i); }
    }
    let mut world = DynWorld::new(&asm, scale(frame.y,-GRAVITY), None).unwrap();
    for step in 0..480 { world.step(DT); linked(&w,&world,step); }
    let low = world.poses().iter().enumerate().map(|(i,p)| dot(sub(p.com,world.rest_com(i)),frame.y)).fold(0.0,f64::min);
    assert!(low < -1.0, "mail must hang: {low}");
    println!("hanging mail: {:.3}s", start.elapsed().as_secs_f64());
}

#[test]
fn chain_drag_at_three_hundred_stays_linked() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0,0.0],[40.0,0.0]], false);
    let asm = weave::assembly(&w,1.24e-3,240.0);
    let mut world = DynWorld::new(&asm,scale(frame.y,-GRAVITY),None).unwrap();
    let body = w.links.len()-1;
    let at = world.rest_com(body);
    world.grab(body,at,makepad_csg_mech::runner::POINTER_HZ);
    for step in 0..240 {
        world.drag(madd(at,frame.x,300.0*(step+1) as f64*DT),DT);
        world.step(DT);
        linked(&w,&world,step);
    }
    assert!(dot(sub(world.poses()[body].com,at),frame.x)>250.0,"grab must move the chain");
    world.throw(scale(frame.x,300.0));
    world.release();
    assert_eq!(world.grabbed(),None);
    println!("chain drag: {:.3}s",start.elapsed().as_secs_f64());
}

#[test]
fn dropped_chain_rests_on_floor() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp,link,frame,&[[0.0,0.0],[40.0,0.0]],false);
    let asm = weave::assembly(&w,1.24e-3,240.0);
    let floor = -15.0;
    let mut world = DynWorld::new(&asm,scale(frame.up,-GRAVITY),Some(floor)).unwrap();
    let mut began = false;
    for step in 0..480 {
        world.step(DT);
        linked(&w,&world,step);
        began |= world.contact_begins().iter().any(|e|e.1.is_none());
    }
    assert!(began,"floor contact must begin");
    assert!(world.max_speed()<2.0,"resting speed {}",world.max_speed());
    let poses = world.poses();
    for (i,l) in w.links.iter().enumerate() {
        let bottom=l.pts.iter().map(|p| dot(poses[i].apply(world.rest_com(i),*p),frame.up)-l.r).fold(f64::INFINITY,f64::min);
        assert!(bottom>floor-0.1,"link {i} penetrated floor: {bottom}");
        assert!(dot(sub(poses[i].com,world.rest_com(i)),frame.up)< -5.0,"link {i} must drop");
    }
    println!("dropped chain: {:.3}s",start.elapsed().as_secs_f64());
}

fn capsule(name: &str, a: P3, b: P3, mass: f64) -> BodySpec {
    let com=scale(add(a,b),0.5);
    let length=len(sub(a,b));
    let axis=norm(sub(b,a));
    let transverse=mass*(length*length/12.0+0.25);
    let axial=mass*0.5;
    let inertia=std::array::from_fn(|i| std::array::from_fn(|j| {
        (if i==j { transverse } else { 0.0 })+(axial-transverse)*axis[i]*axis[j]
    }));
    BodySpec { name:name.into(),mass:MassProps {volume:mass/1.24e-3,mass,com,inertia},
        rods:vec![(a,b,1.0)],tubes:Vec::new(),hulls:Vec::new(),balls:Vec::new(),field:None }
}

fn hinge(a: usize,b: usize,pivot: P3) -> JointSpec {
    JointSpec {name:format!("hinge_{a}_{b}"),a,b,pivot,axis:[0.0,1.0,0.0],
        kind:JointKind::Revolute {lower:-std::f64::consts::PI,upper:std::f64::consts::PI},
        friction:Friction::default(),collide:false,brake:None}
}

#[test]
fn hinge_pendulum_period_matches_inertia() {
    let start=Instant::now();
    let pivot=[0.0,0.0,50.0];
    let angle=0.08_f64;
    let end=madd(pivot,[angle.sin(),0.0,-angle.cos()],40.0);
    let mut asm=Assembly {bodies:vec![capsule("support",sub(pivot,[0.0,2.0,0.0]),add(pivot,[0.0,2.0,0.0]),1.0),capsule("pendulum",pivot,end,1.0)],joints:vec![hinge(0,1,pivot)],substeps:4,..Assembly::default()};
    asm.anchors.push(AnchorSpec {body:0,pivot,kind:AnchorKind::Pin([0.0,1.0,0.0]),friction:Friction::default()});
    let m=&asm.bodies[1].mass;
    let lever=len(sub(m.com,pivot));
    let expected=std::f64::consts::TAU*((m.inertia[1][1]+m.mass*lever*lever)/(m.mass*GRAVITY*lever)).sqrt();
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
    let mut old=world.poses()[1].com[0];
    let mut crossings=Vec::new();
    for step in 0..480 {
        world.step(DT);
        let x=world.poses()[1].com[0];
        if old>0.0 && x<=0.0 { crossings.push(step as f64*DT+DT*old/(old-x)); }
        old=x;
    }
    assert!(crossings.len()>=3,"pendulum must swing repeatedly");
    let period=(crossings[crossings.len()-1]-crossings[0])/(crossings.len()-1) as f64;
    assert!((period/expected-1.0).abs()<0.02,"period {period}, inertia prediction {expected}");
    println!("pendulum: {:.3}s, period {period:.6}s, predicted {expected:.6}s",start.elapsed().as_secs_f64());
}

// A parallelogram four-bar: base (fixed) a-b, left a-c, coupler c-d, right b-d, hinges about y. Its last
// joint in assembly order closes the loop; `rotate` moves the joint list round so another one does.
fn four_bar(offset: P3, rotate: usize) -> Assembly {
    let a=[0.0,0.0,0.0]; let b=[40.0,0.0,0.0];
    let c=add(a,offset); let d=add(b,offset);
    let mut joints=vec![hinge(0,1,a),hinge(1,2,c),hinge(2,3,d),hinge(3,0,b)];
    joints.rotate_left(rotate);
    let mut asm=Assembly {bodies:vec![capsule("base",a,b,1.0),capsule("left",a,c,1.0),capsule("coupler",c,d,1.0),capsule("right",b,d,1.0)],
        joints,substeps:4,..Assembly::default()};
    fixed(&mut asm,0);
    asm
}

fn p95(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() as f64*0.95).ceil() as usize).saturating_sub(1)]
}

// The loop joint is a real joint on a body that shares part b's mass: the hanging four-bar swings with the
// period of its inertia whichever joint closes the loop, keeps its swing (the closure takes no energy), and
// its closure stretches no more than a tree joint does.
#[test]
fn four_bar_swings_with_its_period_and_its_loop_closed() {
    let start=Instant::now();
    let length=40.0_f64;
    let angle=0.1_f64;
    let offset=[length*angle.sin(),0.0,-length*angle.cos()];
    // links about their pivots m (L^2/3 + 0.25) each, the coupler translating on a circle of L; weight 2 m g L
    let inertia=2.0*(length*length/3.0+0.25)+length*length;
    let expected=std::f64::consts::TAU*(inertia/(2.0*GRAVITY*length)).sqrt();
    for rotate in [0,1] {
        let asm=four_bar(offset,rotate);
        let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
        let mut old=world.poses()[2].com[0]-20.0;
        let mut crossings=Vec::new();
        let mut stretches=Vec::new();
        let (mut first,mut last)=(0.0_f64,0.0_f64);
        for step in 0..720 {
            world.step(DT);
            stretches.push(stretch(&asm,&world));
            let x=world.poses()[2].com[0]-20.0;
            if old>0.0 && x<=0.0 { crossings.push(step as f64*DT+DT*old/(old-x)); }
            old=x;
            if step<120 { first=first.max(x.abs()); } else if step>=600 { last=last.max(x.abs()); }
        }
        assert!(crossings.len()>=3,"the four-bar must swing");
        let period=(crossings[crossings.len()-1]-crossings[0])/(crossings.len()-1) as f64;
        let worst=stretches.iter().copied().fold(0.0,f64::max);
        assert!((period/expected-1.0).abs()<0.01,"loop closed at joint {}: period {period}, predicted {expected}",3-rotate);
        assert!(worst<1e-9,"loop closed at joint {}: closure stretch {worst} mm",3-rotate);
        assert!(last>0.99*first,"loop closed at joint {}: swing {first} mm became {last} mm",3-rotate);
        println!("four-bar swing, loop at joint {}: period {period:.6}s predicted {expected:.6}s, swing {first:.4} then {last:.4} mm, stretch max {worst:.3e} p95 {:.3e} mm",3-rotate,p95(stretches));
    }
    println!("four-bar swing: {:.3}s",start.elapsed().as_secs_f64());
}

// The four-bar that falls over (it runs on the current engine) runs here; limits set on its loop joint, then
// on every joint, hold. Its closure holds through the coupler's blow on the base (the current engine's
// joints give 0.33 mm there).
#[test]
fn loop_joint_limits_hold() {
    let start=Instant::now();
    let offset=[12.0,0.0,38.0];
    let limit=JointKind::Revolute {lower:-0.1,upper:0.1};
    for limited in [vec![],vec![3],vec![0,1,2,3]] {
        let mut asm=four_bar(offset,0);
        let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
        for &j in &limited { world.set_limits(j,&limit); asm.joints[j].kind=limit.clone(); }
        let mut most=0.0_f64;
        let mut stretches=Vec::new();
        for _ in 0..480 {
            world.step(DT);
            most=most.max(world.angles(&asm).iter().map(|a| a.abs()).fold(0.0,f64::max));
            stretches.push(stretch(&asm,&world));
        }
        let worst=stretches.iter().copied().fold(0.0,f64::max);
        let typical=p95(stretches);
        assert!(len(sub(world.poses()[2].com,world.rest_com(2)))>1.0,"the four-bar must move");
        if !limited.is_empty() { assert!(most<0.105,"joints {limited:?} limited to 0.1 rad: {most} rad"); }
        assert!(worst<1e-6,"joints {limited:?} limited: closure stretch max {worst} mm");
        println!("four-bar limited at {limited:?}: largest angle {most:.5} rad, stretch max {worst:.3e} p95 {typical:.3e} mm");
    }
    println!("loop limits: {:.3}s",start.elapsed().as_secs_f64());
}

// A spatial loop (crank and rocker on crossed hinges, a coupler on two balls) closed by a cone-limited ball.
// The crank falls fast, so the coupler strikes the cone hard: a hard hit carries a ball past its cone for a
// moment, a loop's as a tree's (a rod falling onto a 0.3 rad cone: 0.42 rad here, 0.69 on the current
// engine; this coupler, 0.48 there). The closure holds as a tree joint's does (the current engine's: 0.08 mm).
#[test]
fn spatial_loop_closed_by_a_limited_ball() {
    let start=Instant::now();
    let o=[0.0,0.0,0.0]; let q=[0.0,40.0,0.0];
    let a=[20.0,0.0,0.0]; let b=[0.0,40.0,-25.0];
    let ball=|x: usize,y: usize,pivot: P3,cone: f64| JointSpec {name:format!("ball_{x}_{y}"),a:x,b:y,pivot,axis:norm(sub(b,a)),
        kind:JointKind::Spherical {cone,twist:None},friction:Friction::default(),collide:false,brake:None};
    let mut rocker=hinge(3,0,q);
    rocker.axis=[1.0,0.0,0.0];
    let cone=0.3;
    let asm=Assembly {bodies:vec![capsule("support",o,q,1.0),capsule("crank",o,a,1.0),capsule("coupler",a,b,1.0),capsule("rocker",q,b,1.0)],
        joints:vec![hinge(0,1,o),rocker,ball(1,2,a,std::f64::consts::PI),ball(2,3,b,cone)],substeps:4,
        anchors:vec![AnchorSpec {body:0,pivot:o,kind:AnchorKind::Fixed,friction:Friction::default()}],..Assembly::default()};
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
    let (mut most,mut worst)=(0.0_f64,0.0_f64);
    for _ in 0..480 {
        world.step(DT);
        most=most.max(world.angles(&asm)[3]);
        worst=worst.max(stretch(&asm,&world));
    }
    assert!(len(sub(world.poses()[1].com,world.rest_com(1)))>1.0,"the crank must turn");
    assert!(most<cone+0.1,"the closing ball turned {most} rad past its cone {cone}");
    assert!(worst<1e-6,"spatial loop closure stretch {worst} mm");
    println!("spatial loop: {:.3}s, closing ball at most {most:.4} rad, stretch {worst:.3e} mm",start.elapsed().as_secs_f64());
}

#[test]
fn shaken_hanging_chain_stays_linked() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0, 0.0], [40.0, 0.0]], false);
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    fixed(&mut asm, 0);
    let mut world = DynWorld::new(&asm, scale(frame.y, -GRAVITY), None).unwrap();
    for step in 0..240 {
        let at = scale(frame.x, 20.0 * (std::f64::consts::TAU * 2.0 * (step+1) as f64 * DT).sin());
        world.move_ground(at, frame.up, 0.0, DT);
        world.step(DT);
        linked(&w, &world, step);
        assert!(len(sub(world.poses()[0].com, add(world.rest_com(0), at))) < 1e-8, "ground must carry its fixed anchor");
    }
    println!("shaken chain: {:.3}s", start.elapsed().as_secs_f64());
}

#[test]
fn frozen_chain_holds_then_falls() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0, 0.0], [40.0, 0.0]], false);
    let asm = weave::assembly(&w, 1.24e-3, 240.0);
    let mut world = DynWorld::new(&asm, scale(frame.up, -GRAVITY), None).unwrap();
    let bodies: Vec<_> = (0..w.links.len()).collect();
    world.apply(&bodies, &BodyAction::Freeze(true));
    assert_eq!(world.frozen(), bodies);
    for step in 0..240 {
        world.step(DT);
        linked(&w, &world, step);
        for (i,p) in world.poses().iter().enumerate() {
            assert!(world.is_frozen(i));
            assert!(len(sub(p.com, world.rest_com(i))) < 0.01, "frozen body {i} moved");
        }
    }
    world.apply(&bodies, &BodyAction::Freeze(false));
    assert!(world.frozen().is_empty());
    for _ in 0..24 { world.step(DT); }
    for (i,p) in world.poses().iter().enumerate() {
        assert!(dot(sub(p.com, world.rest_com(i)), frame.up) < -1.0, "thawed body {i} must fall");
    }
    println!("frozen chain: {:.3}s", start.elapsed().as_secs_f64());
}

#[test]
fn fixed_chain_drop_releases_anchor_and_reaches_floor() {
    let start = Instant::now();
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0, 0.0], [40.0, 0.0]], false);
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    fixed(&mut asm, 0);
    let mut world = DynWorld::new(&asm, scale(frame.up, -GRAVITY), Some(-15.0)).unwrap();
    world.move_ground(scale(frame.up, 3.0), frame.up, 0.0, DT);
    world.step(DT);
    let before = world.poses();
    let bodies: Vec<_> = (0..w.links.len()).collect();
    world.apply(&bodies, &BodyAction::Drop { lift: 0.0 });
    for (a,b) in world.poses().iter().zip(&before) {
        assert!(len(sub(a.com, b.com)) < 1e-8, "detaching must preserve world pose");
    }
    let mut began = false;
    for step in 0..480 {
        world.step(DT);
        linked(&w, &world, step);
        began |= world.contact_begins().iter().any(|e|e.1.is_none());
    }
    assert!(began, "released chain must contact floor");
    assert!(world.max_speed() < 2.0, "released chain must rest: {}", world.max_speed());
    assert!(dot(sub(world.poses()[0].com, before[0].com), frame.up) < -5.0, "fixed anchor must have become free");
    println!("fixed chain drop: {:.3}s", start.elapsed().as_secs_f64());
}

fn cuboid(name: &str, center: P3, half: P3, mass: f64) -> BodySpec {
    let corners=(0..8).map(|k| std::array::from_fn(|i| center[i]+if k>>i&1==0 {-half[i]} else {half[i]})).collect();
    let inertia=std::array::from_fn(|i| std::array::from_fn(|j| if i==j {
        let (u,v)=((i+1)%3,(i+2)%3); mass/3.0*(half[u]*half[u]+half[v]*half[v])
    } else {0.0}));
    BodySpec {name:name.into(),mass:MassProps {volume:mass/1.24e-3,mass,com:center,inertia},
        rods:Vec::new(),tubes:Vec::new(),hulls:vec![corners],balls:Vec::new(),field:None}
}

// Pose::apply rounds through f32 (about 1e-5 mm at these sizes): the stretch is measured in f64.
fn seen_from(pose: &makepad_csg_mech::sim::Pose, rest_com: P3, p: P3) -> P3 {
    let (v,w)=([pose.rot[0],pose.rot[1],pose.rot[2]],pose.rot[3]);
    let d=sub(p,rest_com);
    let t=scale(cross(v,d),2.0);
    add(pose.com,add(add(d,scale(t,w)),cross(v,t)))
}

fn stretch(asm: &Assembly, world: &DynWorld) -> f64 {
    let poses=world.poses();
    asm.joints.iter().map(|j| len(sub(seen_from(&poses[j.a],world.rest_com(j.a),j.pivot),seen_from(&poses[j.b],world.rest_com(j.b),j.pivot)))).fold(0.0,f64::max)
}

#[test]
fn hull_hinge_pendulum_period_matches_inertia() {
    let start=Instant::now();
    let pivot=[0.0,0.0,50.0];
    let angle=0.08_f64;
    let centre=madd(pivot,[angle.sin(),0.0,-angle.cos()],20.0);
    let mut asm=Assembly {bodies:vec![cuboid("support",pivot,[3.0,3.0,3.0],1.0),cuboid("pendulum",centre,[4.0,4.0,20.0],1.0)],
        joints:vec![hinge(0,1,pivot)],substeps:4,..Assembly::default()};
    asm.anchors.push(AnchorSpec {body:0,pivot,kind:AnchorKind::Pin([0.0,1.0,0.0]),friction:Friction::default()});
    let m=&asm.bodies[1].mass;
    let lever=len(sub(m.com,pivot));
    let expected=std::f64::consts::TAU*((m.inertia[1][1]+m.mass*lever*lever)/(m.mass*GRAVITY*lever)).sqrt();
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
    let mut old=world.poses()[1].com[0];
    let mut crossings=Vec::new();
    for step in 0..480 {
        world.step(DT);
        let x=world.poses()[1].com[0];
        if old>0.0 && x<=0.0 { crossings.push(step as f64*DT+DT*old/(old-x)); }
        old=x;
    }
    assert!(crossings.len()>=3,"pendulum must swing repeatedly");
    let period=(crossings[crossings.len()-1]-crossings[0])/(crossings.len()-1) as f64;
    assert!((period/expected-1.0).abs()<0.02,"period {period}, inertia prediction {expected}");
    println!("hull pendulum: {:.3}s, period {period:.6}s, predicted {expected:.6}s",start.elapsed().as_secs_f64());
}

fn box_chain(count: usize, gap: f64) -> Assembly {
    let pitch=20.0+gap;
    let mut asm=Assembly {substeps:4,..Assembly::default()};
    for i in 0..count { asm.bodies.push(cuboid(&format!("box_{i}"),[0.0,0.0,-pitch*i as f64],[5.0,5.0,10.0],1.0)); }
    for i in 1..count { asm.joints.push(hinge(i-1,i,[0.0,0.0,-pitch*(i as f64-0.5)])); }
    fixed(&mut asm,0);
    asm
}

#[test]
fn hull_chain_keeps_joints_closed() {
    let start=Instant::now();
    let asm=box_chain(6,2.0);
    // gravity off the chain's axis, so it swings
    let g=[GRAVITY*0.5,0.0,-GRAVITY*0.866];
    let mut world=DynWorld::new(&asm,g,None).unwrap();
    let mut worst=0.0_f64;
    for _ in 0..480 { world.step(DT); worst=worst.max(stretch(&asm,&world)); }
    assert!(worst<1e-6,"joint stretch {worst} mm");
    assert!(len(sub(world.poses()[5].com,world.rest_com(5)))>5.0,"chain must swing");
    println!("hull chain: {:.3}s, worst stretch {worst:.3e}mm",start.elapsed().as_secs_f64());
}

#[test]
fn tilted_hull_lands_flat_on_a_face() {
    let start=Instant::now();
    let side=10.0;
    let (sa,ca)=(0.5_f64.sin(),0.5_f64.cos());
    let (sb,cb)=(0.3_f64.sin(),0.3_f64.cos());
    let centre=[0.0,0.0,18.0];
    let corners: Vec<P3>=(0..8).map(|k| {
        let p: P3=std::array::from_fn(|i| if k>>i&1==0 {-side} else {side});
        let q=[p[0],ca*p[1]-sa*p[2],sa*p[1]+ca*p[2]];
        add(centre,[cb*q[0]+sb*q[2],q[1],-sb*q[0]+cb*q[2]])
    }).collect();
    let mass=8.0;
    let k=mass*2.0*side*side/3.0;
    let mut body=cuboid("cube",centre,[side;3],mass);
    body.hulls=vec![corners.clone()];
    body.mass.inertia=std::array::from_fn(|i| std::array::from_fn(|j| if i==j {k} else {0.0}));
    let asm=Assembly {bodies:vec![body],substeps:4,..Assembly::default()};
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],Some(0.0)).unwrap();
    for _ in 0..480 { world.step(DT); }
    let pose=world.poses()[0];
    let mut z: Vec<f64>=corners.iter().map(|c| pose.apply(world.rest_com(0),*c)[2]).collect();
    z.sort_by(f64::total_cmp);
    assert!(z[0]>-0.5,"cube sank into the floor: {}",z[0]);
    assert!(z[3]-z[0]<0.2,"four lowest corners must share a face: {z:?}");
    assert!(world.max_speed()<0.5 && world.max_angular_speed()<0.05,"cube must rest: {} {}",world.max_speed(),world.max_angular_speed());
    println!("tilted hull: {:.3}s, lowest corners {:?}",start.elapsed().as_secs_f64(),&z[..4]);
}

// The engine (as its reference) refuses flat and straight point clouds; DynWorld gives them the current
// engine's thickness, so a plate is a thin solid that lies flat on the floor.
#[test]
fn flat_and_straight_hulls_are_thin_solids() {
    let start=Instant::now();
    let (s,c)=(0.4_f64.sin(),0.4_f64.cos());
    let centre=[0.0,0.0,15.0];
    let corners: Vec<P3>=[[-10.0,-10.0],[10.0,-10.0],[10.0,10.0],[-10.0,10.0]].iter().map(|&[x,y]| add(centre,[c*x,y,s*x])).collect();
    let mut body=cuboid("plate",centre,[10.0,10.0,0.5],2.0);
    body.hulls=vec![corners.clone()];
    let asm=Assembly {bodies:vec![body],substeps:4,..Assembly::default()};
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],Some(0.0)).unwrap();
    for _ in 0..480 { world.step(DT); }
    let pose=world.poses()[0];
    let z: Vec<f64>=corners.iter().map(|p| pose.apply(world.rest_com(0),*p)[2]).collect();
    let (lo,hi)=z.iter().fold((f64::INFINITY,f64::NEG_INFINITY),|(lo,hi),&z| (lo.min(z),hi.max(z)));
    assert!(lo>-0.5 && hi-lo<0.2,"the plate must lie flat on the floor: {z:?}");
    let mut asm=box_chain(2,2.0);
    asm.bodies[1].hulls=vec![vec![[0.0,0.0,-15.0],[1.0,0.0,-15.0],[2.0,0.0,-15.0],[3.0,0.0,-15.0]]];
    DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).expect("a straight cloud is a thin bar");
    println!("flat hulls: {:.3}s, plate corners {z:?}",start.elapsed().as_secs_f64());
}

// The hand pulls no harder than the current engine's: 200 times the part's weight, at least ten times
// everything's. A part held by a hinge and pulled away from it loads the hinge with exactly that.
#[test]
fn hand_pull_is_capped() {
    let start=Instant::now();
    let pivot=[0.0,0.0,0.0];
    let mut asm=Assembly {bodies:vec![cuboid("support",pivot,[3.0,3.0,3.0],1.0),cuboid("arm",[0.0,0.0,-20.0],[4.0,4.0,10.0],1.0)],
        joints:vec![hinge(0,1,pivot)],substeps:4,..Assembly::default()};
    fixed(&mut asm,0);
    let mut world=DynWorld::new(&asm,[0.0;3],None).unwrap();
    let at=world.rest_com(1);
    world.grab(1,at,makepad_csg_mech::runner::HAND_HZ);
    world.drag(madd(at,[0.0,0.0,-1.0],50.0),DT);
    for _ in 0..120 { world.step(DT); }
    let cap=(1.0_f64*200.0).max(2.0*10.0)*GRAVITY;
    let force=world.joint_force(0);
    assert!((force/cap-1.0).abs()<0.02,"hinge load {force} against the cap {cap}");
    println!("hand cap: {:.3}s, hinge load {force:.0} for a cap of {cap:.0}",start.elapsed().as_secs_f64());
}

fn moved(poses: &[makepad_csg_mech::sim::Pose], rest: &[P3], body: usize) -> f64 { len(sub(poses[body].com,rest[body])) }

#[test]
fn drop_of_one_body_frees_the_same_bodies_as_the_current_engine() {
    let start=Instant::now();
    let asm=box_chain(3,2.0);
    let g=[GRAVITY*0.5,0.0,-GRAVITY*0.866];
    for (set,anchor_let_go) in [(vec![1usize],false),(vec![0usize],true)] {
        let mut dynamics=DynWorld::new(&asm,g,None).unwrap();
        let mut current=MechWorld::new(&asm,g,None);
        let rest: Vec<P3>=(0..3).map(|i| dynamics.rest_com(i)).collect();
        for lift in [0.0,5.0] {
            dynamics.apply(&set,&BodyAction::Drop {lift});
            current.apply(&set,&BodyAction::Drop {lift});
        }
        for _ in 0..60 { dynamics.step(DT); current.step(DT); }
        let (d,c)=(dynamics.poses(),current.poses());
        for body in 0..3 {
            // a body is free when it has fallen well past what a swing from the anchor reaches
            let free=|poses: &[makepad_csg_mech::sim::Pose]| moved(poses,&rest,body)>100.0;
            assert_eq!(free(&d),free(&c),"body {body} of set {set:?}: dynamics {} mm, current {} mm",moved(&d,&rest,body),moved(&c,&rest,body));
        }
        assert_eq!(moved(&d,&rest,0)>100.0,anchor_let_go,"anchor body {set:?}");
    }
    println!("partial drop: {:.3}s",start.elapsed().as_secs_f64());
}

#[test]
fn body_let_go_from_the_moving_ground_keeps_its_velocity() {
    let start=Instant::now();
    let asm=box_chain(3,2.0);
    let mut world=DynWorld::new(&asm,[0.0,0.0,0.0],None).unwrap();
    world.move_ground([0.0,0.0,3.0],[0.0,0.0,1.0],0.0,0.1);
    world.step(DT);
    assert!(world.max_speed()<1e-9,"a held body has no velocity of its own");
    world.apply(&[0,1,2],&BodyAction::Drop {lift:0.0});
    let after=world.max_speed();
    assert!((after-30.0).abs()<1.0,"the ground moves at 30 mm/s, speed after letting go {after}");
    println!("ground velocity: {:.3}s",start.elapsed().as_secs_f64());
}

// A hinge chain hanging from a fixed anchor and a box on the floor fall asleep at rest; woken by a poke, a
// throw or an action, each moves as the same assembly that never slept. So do both when the anchor carries
// the chain into the box (two sleeping trees must never meet).
#[test]
fn woken_trees_move_as_if_they_never_slept() {
    let start=Instant::now();
    let make=|sleep: f64| {
        let pivot=[0.0,0.0,60.0];
        let mut asm=Assembly {bodies:vec![capsule("support",sub(pivot,[0.0,3.0,0.0]),add(pivot,[0.0,3.0,0.0]),1.0)],substeps:4,sleep,..Assembly::default()};
        for k in 0..3 {
            let top=sub(pivot,[0.0,0.0,12.0*k as f64]);
            asm.bodies.push(capsule(&format!("link{k}"),top,sub(top,[0.0,0.0,12.0]),1.0));
            asm.joints.push(hinge(k,k+1,top));
        }
        asm.bodies.push(cuboid("box",[40.0,0.0,5.0],[5.0,5.0,5.0],1.0));
        fixed(&mut asm,0);
        asm
    };
    let (chain,boxed)=(vec![1,2,3],vec![4]);
    for wake in ["poke","throw","action","ground"] {
        let (asleep_asm,awake_asm)=(make(makepad_csg_mech::sim::SLEEP_SPEED),make(0.0));
        let mut asleep=DynWorld::new(&asleep_asm,[0.0,0.0,-GRAVITY],Some(0.0)).unwrap();
        let mut awake=DynWorld::new(&awake_asm,[0.0,0.0,-GRAVITY],Some(0.0)).unwrap();
        for _ in 0..240 { asleep.step(DT); awake.step(DT); }
        assert_eq!(asleep.sleeping(),[chain.clone(),boxed.clone()].concat(),"the chain and the box must be asleep");
        assert!(awake.sleeping().is_empty());
        let both=[chain.clone(),boxed.clone()].concat();
        let woken=match wake { "poke" => &chain, "ground" => &both, _ => &boxed };
        for world in [&mut asleep,&mut awake] {
            let centre=world.poses()[4].com;
            match wake {
                "poke" => { let tip=world.poses()[3].com; world.poke(3,tip,[60.0,0.0,0.0]); }
                "throw" => { world.grab(4,centre,makepad_csg_mech::runner::HAND_HZ); world.throw([0.0,80.0,150.0]); world.release(); }
                "ground" => world.move_ground([40.0,0.0,-20.0],[0.0,0.0,1.0],0.0,0.5),
                _ => world.apply(&[4],&BodyAction::Drop {lift:4.0}),
            }
        }
        let (mut apart,mut moved)=(0.0_f64,0.0_f64);
        for _ in 0..240 {
            asleep.step(DT); awake.step(DT);
            for &b in woken {
                apart=apart.max(len(sub(asleep.poses()[b].com,awake.poses()[b].com)));
                moved=moved.max(len(sub(asleep.poses()[b].com,asleep_asm.bodies[b].mass.com)));
            }
        }
        assert!(moved>0.5,"woken by {wake}: the parts must move ({moved} mm)");
        assert!(apart<1e-6,"woken by {wake}: {apart} mm from the assembly that never slept");
        println!("woken by {wake}: moved {moved:.2} mm, {apart:.3e} mm from the assembly that never slept");
    }
    println!("woken trees: {:.3}s",start.elapsed().as_secs_f64());
}

// A slow, small swing passes under the sleep speed at the top of each arc for longer than the engine's
// ten substeps; it must keep swinging, not sleep there.
#[test]
fn slow_swing_does_not_sleep_at_the_top() {
    let start=Instant::now();
    let pivot=[0.0,0.0,100.0];
    let angle=0.0125_f64;
    let end=madd(pivot,[angle.sin(),0.0,-angle.cos()],80.0);
    let mut asm=Assembly {bodies:vec![capsule("support",sub(pivot,[0.0,2.0,0.0]),add(pivot,[0.0,2.0,0.0]),1.0),capsule("pendulum",pivot,end,1.0)],
        joints:vec![hinge(0,1,pivot)],substeps:4,sleep:makepad_csg_mech::sim::SLEEP_SPEED,..Assembly::default()};
    fixed(&mut asm,0);
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
    let (mut first,mut last)=(0.0_f64,0.0_f64);
    for step in 0..480 {
        world.step(DT);
        assert!(world.sleeping().is_empty(),"the swing fell asleep at {:.3}s",(step+1) as f64*DT);
        let x=world.poses()[1].com[0];
        if step<120 { first=first.max(x.abs()); } else if step>=360 { last=last.max(x.abs()); }
    }
    assert!(last>0.99*first,"the swing must go on: {first} mm then {last} mm");
    println!("slow swing: {first:.4} then {last:.4} mm, {:.3}s",start.elapsed().as_secs_f64());
}

// A joint's collide says whether its two parts touch away from it, whichever of them the tree makes the
// other's parent: a lid hinged along a free box's back edge falls shut on the box when they do, and on
// through it when they do not.
#[test]
fn a_joints_collide_says_whether_its_parts_touch() {
    let fall=|collide: bool| {
        let pivot=[-20.0,0.0,1.0];
        let asm=Assembly {bodies:vec![cuboid("box",[0.0,0.0,-5.0],[20.0,20.0,5.0],10.0),cuboid("lid",[0.0,0.0,3.0],[20.0,20.0,2.0],1.0)],
            joints:vec![JointSpec {collide,..hinge(0,1,pivot)}],substeps:4,up:[0.0,0.0,1.0],..Assembly::default()};
        let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],Some(-10.0)).unwrap();
        for _ in 0..240 { world.step(DT); }
        world.poses()[1].com[2]
    };
    let (shut,through)=(fall(true),fall(false));
    assert!(shut>2.0,"the lid stops on the box: its middle at {shut} mm");
    assert!(through<0.0,"with no contact the lid falls through the box: its middle at {through} mm");
}

// Two parts' hulls can overlap where they print, though the real shapes keep their clearance: contact pushes only
// past that depth. With nothing on it the lid stays where it printed, and its weight does not take it through
// the box.
#[test]
fn a_pair_that_overlaps_where_it_prints_stays_put_and_still_stops() {
    let middle=1.0;
    let after_a_second=|gravity: P3| {
        let mut asm=Assembly {bodies:vec![cuboid("box",[0.0,0.0,-5.0],[20.0,20.0,5.0],10.0),cuboid("lid",[0.0,0.0,middle],[20.0,20.0,2.0],1.0)],
            joints:vec![JointSpec {collide:true,..hinge(0,1,[-20.0,0.0,0.0])}],substeps:4,up:[0.0,0.0,1.0],..Assembly::default()};
        fixed(&mut asm,0);
        let mut world=DynWorld::new(&asm,gravity,None).unwrap();
        for _ in 0..240 { world.step(DT); }
        world.poses()[1].com[2]
    };
    let free=after_a_second([0.0;3]);
    assert!((free-middle).abs()<0.01,"nothing on it, the lid stays where it printed: its middle at {free} mm");
    let weighed=after_a_second([0.0,0.0,-GRAVITY]);
    assert!((weighed-middle).abs()<0.5,"its weight does not take it through: its middle at {weighed} mm");
}

// The engine puts a blown-up state back at the print pose by itself: the world keeps a record of it, with the
// part it saw it on. A spring far stiffer than the substep can hold blows up; a soft one never does.
#[test]
fn a_state_that_blows_up_is_on_record() {
    let run=|stiffness: f64| {
        let pivot=[0.0,0.0,0.0];
        let mut asm=Assembly {bodies:vec![cuboid("base",[-5.0,0.0,0.0],[5.0,5.0,5.0],1.0),cuboid("bead",[6.0,0.0,0.0],[5.0,5.0,5.0],1.0)],
            joints:vec![JointSpec {kind:JointKind::Flex {stiffness,damping:0.0,one_way:true},..hinge(0,1,pivot)}],substeps:1,..Assembly::default()};
        fixed(&mut asm,0);
        let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
        for _ in 0..240 { world.step(DT); }
        world.resets().to_vec()
    };
    assert!(run(0.001).is_empty(),"a soft spring holds");
    let resets=run(1e6);
    let first=resets.first().expect("a spring far too stiff for its substep blows up");
    assert_eq!(first.part,Some(1),"the part on the spring");
    assert!(first.time>0.0 && first.time<1.0,"when it did: {}",first.time);
}
