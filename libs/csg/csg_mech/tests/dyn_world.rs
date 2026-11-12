use makepad_csg_mech::dyn_world::DynWorld;
use makepad_csg_mech::geom::*;
use makepad_csg_mech::joint::Profile;
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner;
use makepad_csg_mech::sim::{AnchorKind, AnchorSpec, Assembly, BodyAction, BodySpec, Friction, JointKind, JointSpec, MechWorld, SolidField, GRAVITY, NEWTON, SLEEP_SPEED};
use makepad_csg_mech::weave::{self, Frame, SquareLink, Weave, WeaveProfile};
use makepad_dynamics::thread::TaskPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
}

#[test]
fn mail_runs_on_dynamics_and_stays_linked_hung_and_dragged() {
    let (wp, link, frame) = setup();
    let w = weave::mail(&wp, link, frame, [-12.0,-12.0], [12.0,12.0], 0.0, &|_| -1.0);
    assert!(w.pairs.len()>2, "mail must have multiple interlocks");
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    assert!(runner::on_dynamics(&asm), "mail runs on the dynamics engine");
    let top = w.links.iter().map(|l| dot(l.center(),frame.y)).fold(f64::NEG_INFINITY,f64::max);
    for (i,l) in w.links.iter().enumerate() {
        if dot(l.center(),frame.y)>top-1e-6 { fixed(&mut asm,i); }
    }
    let mut world = runner::dynamics_world(&asm, scale(frame.y,-GRAVITY), None).unwrap();
    for step in 0..480 { world.step(DT); linked(&w,&world,step); }
    let low = world.poses().iter().enumerate().map(|(i,p)| dot(sub(p.com,world.rest_com(i)),frame.y)).fold(0.0,f64::min);
    assert!(low < -1.0, "mail must hang: {low}");
    // a hand pulls the lowest link sideways at a fast hand's speed (the world caps the hand's step)
    let poses = world.poses();
    let body = (0..w.links.len()).min_by(|a,b| dot(poses[*a].com,frame.y).total_cmp(&dot(poses[*b].com,frame.y))).unwrap();
    let at = poses[body].com;
    world.grab(body,at,runner::POINTER_HZ);
    for step in 0..120 {
        world.drag(madd(at,frame.x,1000.0*(step+1) as f64*DT),DT);
        world.step(DT);
        linked(&w,&world,480+step);
    }
    assert!(dot(sub(world.poses()[body].com,at),frame.x)>5.0, "the hand must pull the mail");
}

/// A pool of 16 threads at the apps' workpool's cost per fan-out (the default) that counts its fan-outs and
/// runs their tasks on the caller.
struct Counting(AtomicUsize);

impl TaskPool for Counting {
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        self.0.fetch_add(1, Ordering::Relaxed);
        (0..n).for_each(f);
    }
    fn threads(&self) -> usize {
        16
    }
}

#[test]
fn big_mail_spreads_its_collision_over_the_pool() {
    let (wp, link, frame) = setup();
    let w = weave::mail(&wp, link, frame, [-24.0,-24.0], [24.0,24.0], 0.0, &|_| -1.0);
    let mut asm = weave::assembly(&w, 1.24e-3, 240.0);
    let top = w.links.iter().map(|l| dot(l.center(),frame.y)).fold(f64::NEG_INFINITY,f64::max);
    for (i,l) in w.links.iter().enumerate() {
        if dot(l.center(),frame.y)>top-1e-6 { fixed(&mut asm,i); }
    }
    let mut world = runner::dynamics_world(&asm, scale(frame.y,-GRAVITY), None).unwrap();
    let pool = Arc::new(Counting(AtomicUsize::new(0)));
    world.bind_pool(Some(pool.clone()));
    for _ in 0..4 { world.step(DT); }
    let fan_outs = pool.0.load(Ordering::Relaxed);
    assert!(fan_outs >= 4, "the big mail's midphase pays for a fan-out every step: {fan_outs} in 4 steps");
}

#[test]
fn chain_runs_on_dynamics_and_stays_linked_through_a_drag() {
    let (wp, link, frame) = setup();
    let w = weave::chain(&wp, link, frame, &[[0.0,0.0],[40.0,0.0]], false);
    let asm = weave::assembly(&w,1.24e-3,240.0);
    assert!(runner::on_dynamics(&asm), "a chain runs on the dynamics engine");
    let mut world = runner::dynamics_world(&asm,scale(frame.y,-GRAVITY),None).unwrap();
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
}

#[test]
fn dropped_chain_rests_on_floor() {
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
        friction:Friction::default(),collide:false,brake:None,motor:None}
}

#[test]
fn hinge_pendulum_period_matches_inertia() {
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
    println!("pendulum: period {period:.6}s, predicted {expected:.6}s");
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
}

// The four-bar that falls over (it runs on the current engine) runs here; limits set on its loop joint, then
// on every joint, hold. Its closure holds through the coupler's blow on the base (the current engine's
// joints give 0.33 mm there).
#[test]
fn loop_joint_limits_hold() {
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
}

// A spatial loop (crank and rocker on crossed hinges, a coupler on two balls) closed by a cone-limited ball.
// The crank falls fast, so the coupler strikes the cone hard: a hard hit carries a ball past its cone for a
// moment, a loop's as a tree's (a rod falling onto a 0.3 rad cone: 0.42 rad here, 0.69 on the current
// engine; this coupler, 0.48 there). The closure holds as a tree joint's does (the current engine's: 0.08 mm).
#[test]
fn spatial_loop_closed_by_a_limited_ball() {
    let o=[0.0,0.0,0.0]; let q=[0.0,40.0,0.0];
    let a=[20.0,0.0,0.0]; let b=[0.0,40.0,-25.0];
    let ball=|x: usize,y: usize,pivot: P3,cone: f64| JointSpec {name:format!("ball_{x}_{y}"),a:x,b:y,pivot,axis:norm(sub(b,a)),
        kind:JointKind::Spherical {cone,twist:None},friction:Friction::default(),collide:false,brake:None,motor:None};
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
    println!("spatial loop: closing ball at most {most:.4} rad, stretch {worst:.3e} mm");
}

#[test]
fn shaken_hanging_chain_stays_linked() {
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
}

#[test]
fn frozen_chain_holds_then_falls() {
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
}

#[test]
fn fixed_chain_drop_releases_anchor_and_reaches_floor() {
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
    println!("hull pendulum: period {period:.6}s, predicted {expected:.6}s");
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
    let asm=box_chain(6,2.0);
    // gravity off the chain's axis, so it swings
    let g=[GRAVITY*0.5,0.0,-GRAVITY*0.866];
    let mut world=DynWorld::new(&asm,g,None).unwrap();
    let mut worst=0.0_f64;
    for _ in 0..480 { world.step(DT); worst=worst.max(stretch(&asm,&world)); }
    assert!(worst<1e-6,"joint stretch {worst} mm");
    assert!(len(sub(world.poses()[5].com,world.rest_com(5)))>5.0,"chain must swing");
    println!("hull chain: worst stretch {worst:.3e}mm");
}

#[test]
fn tilted_hull_lands_flat_on_a_face() {
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
    println!("tilted hull: lowest corners {:?}", &z[..4]);
}

// The engine (as its reference) refuses flat and straight point clouds; DynWorld gives them the current
// engine's thickness, so a plate is a thin solid that lies flat on the floor.
#[test]
fn flat_and_straight_hulls_are_thin_solids() {
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
    println!("flat hulls: plate corners {z:?}");
}

// The hand pulls no harder than the current engine's: 200 times the part's weight, at least ten times
// everything's. A part held by a hinge and pulled away from it loads the hinge with exactly that.
#[test]
fn hand_pull_is_capped() {
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
    println!("hand cap: hinge load {force:.0} for a cap of {cap:.0}");
}

fn moved(poses: &[makepad_csg_mech::sim::Pose], rest: &[P3], body: usize) -> f64 { len(sub(poses[body].com,rest[body])) }

#[test]
fn drop_of_one_body_frees_the_same_bodies_as_the_current_engine() {
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
}

#[test]
fn body_let_go_from_the_moving_ground_keeps_its_velocity() {
    let asm=box_chain(3,2.0);
    let mut world=DynWorld::new(&asm,[0.0,0.0,0.0],None).unwrap();
    world.move_ground([0.0,0.0,3.0],[0.0,0.0,1.0],0.0,0.1);
    world.step(DT);
    assert!(world.max_speed()<1e-9,"a held body has no velocity of its own");
    world.apply(&[0,1,2],&BodyAction::Drop {lift:0.0});
    let after=world.max_speed();
    assert!((after-30.0).abs()<1.0,"the ground moves at 30 mm/s, speed after letting go {after}");
}

// A hinge chain hanging from a fixed anchor and a box on the floor fall asleep at rest; woken by a poke, a
// throw or an action, each moves as the same assembly that never slept. So do both when the anchor carries
// the chain into the box (two sleeping trees must never meet).
#[test]
fn woken_trees_move_as_if_they_never_slept() {
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
}

// A slow, small swing passes under the sleep speed at the top of each arc for longer than the engine's
// ten substeps; it must keep swinging, not sleep there.
#[test]
fn slow_swing_does_not_sleep_at_the_top() {
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
    println!("slow swing: {first:.4} then {last:.4} mm");
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

// Turning a part from where it printed can sink its proxy deeper before it comes clear. This arm's corner lies across
// the hinge from the block, 1.5 mm into it: turned one way it goes 0.5 mm deeper over 18 degrees and is clear by 55.
// A hand takes it past a right angle each way, and the block still stops it further round.
#[test]
fn a_part_turns_out_of_its_print_overlap_over_a_hump() {
    let turned=|sign: f64,degrees: f64| {
        let (mass,com)=(5.0,[12.0,0.0,0.0]);
        let inertia=[[mass*10.0,0.0,0.0],[0.0,mass*140.0,0.0],[0.0,0.0,mass*140.0]];
        let outline=[[-9.5,3.12],[30.0,3.12],[30.0,-3.0],[-7.0,-3.0]];
        let hull=outline.iter().flat_map(|c: &[f64;2]| [[c[0],-2.0,c[1]],[c[0],2.0,c[1]]]).collect();
        let arm=BodySpec {name:"arm".into(),mass:MassProps {volume:mass/1.24e-3,mass,com,inertia},
            rods:Vec::new(),tubes:Vec::new(),hulls:vec![hull],balls:Vec::new(),field:None};
        let mut asm=Assembly {bodies:vec![cuboid("block",[-18.0,0.0,0.0],[10.0,5.0,12.0],10.0),arm],
            joints:vec![JointSpec {collide:true,..hinge(0,1,[0.0;3])}],substeps:8,..Assembly::default()};
        fixed(&mut asm,0);
        let mut world=DynWorld::new(&asm,[0.0;3],None).unwrap();
        let far=[30.0,0.0,0.0];
        world.grab(1,far,makepad_csg_mech::runner::HAND_HZ);
        let mut most=0.0_f64;
        let frames=(degrees/40.0/DT) as usize;
        for k in 1..=frames {
            world.drag(rotate(far,[0.0,1.0,0.0],sign*(40.0*k as f64*DT).to_radians()),DT);
            world.step(DT);
            most=most.max(2.0*world.poses()[1].rot[3].abs().min(1.0).acos());
        }
        most.to_degrees()
    };
    for sign in [-1.0,1.0] {
        let past=turned(sign,100.0);
        assert!(past>90.0,"turned {sign} way, the hand got it only {past:.1} degrees");
        let stopped=turned(sign,170.0);
        assert!(stopped<130.0,"turned {sign} way, the block let it go {stopped:.1} degrees");
    }
}

// Hung on a pin, a piece's gravity runs from the pin through its centre of mass as it printed: it loads at rest, and a
// push swings it back and forth through where it loaded. On either engine.
#[test]
fn a_piece_on_a_pin_loads_at_rest_and_swings_once_pushed() {
    let pivot=[0.0,0.0,0.0];
    let mut asm=Assembly {bodies:vec![capsule("near",pivot,[20.0,0.0,0.0],1.0),capsule("far",[20.0,0.0,0.0],[40.0,0.0,0.0],1.0)],
        joints:vec![hinge(0,1,[20.0,0.0,0.0])],up:[0.0,1.0,0.0],substeps:8,..Assembly::default()};
    asm.anchors.push(AnchorSpec {body:0,pivot,kind:AnchorKind::Pin([0.0,1.0,0.0]),friction:Friction::default()});
    let gravity=scale(asm.hang_gravity(),GRAVITY);
    assert!(dot(norm(gravity),[1.0,0.0,0.0])>0.999999,"along the piece: {gravity:?}");
    let swing=|c: P3| c[2].atan2(c[0]).to_degrees();
    macro_rules! hang { ($world:expr,$name:expr) => {{
        let mut world=$world;
        let mut still=0.0_f64;
        for _ in 0..240 { world.step(DT); still=still.max(swing(world.poses()[1].com).abs()); }
        assert!(still<0.01,"{}: loaded, it swings {still:.4} degrees",$name);
        world.poke(1,[40.0,0.0,0.0],[0.0,0.0,100.0]);
        let (mut most,mut crossings,mut last)=(0.0_f64,0,0.0_f64);
        for _ in 0..240 {
            world.step(DT);
            let now=swing(world.poses()[1].com);
            most=most.max(now.abs());
            if now*last<0.0 { crossings+=1; }
            last=now;
        }
        println!("{}: loaded, swings {still:.6} degrees; pushed, {most:.2} degrees, through rest {crossings} times in 1 s",$name);
        assert!(most>2.0 && crossings>=3,"{}: pushed, it swings {most:.2} degrees, through rest {crossings} times",$name);
    }}}
    hang!(DynWorld::new(&asm,gravity,None).unwrap(),"dynamics");
    hang!(MechWorld::new(&asm,gravity,None),"box3d");
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

// Motors on one base: a servo lifts its arm and holds it there, a servo too weak for its arm's weight lets it
// sink, a drive spins a balanced wheel at its speed, and a slider's servo goes to its travel, its setting kept
// within the joint's range.
#[test]
fn motors_go_where_they_are_set_within_their_limits() {
    use makepad_csg_mech::sim::Motor;
    use makepad_csg_mech::MotorKind;
    let motor=|kind,limit: f64| Some(Motor {kind,limit:limit*1e6});
    let arm=|y: f64| capsule("arm",[0.0,y,0.0],[40.0,y,0.0],5.0);
    let mut asm=Assembly {bodies:vec![cuboid("base",[0.0,30.0,-30.0],[5.0,40.0,5.0],10.0),arm(0.0),arm(20.0),cuboid("wheel",[0.0,40.0,0.0],[10.0,2.0,10.0],2.0),cuboid("carriage",[0.0,60.0,0.0],[3.0,3.0,3.0],2.0)],
        joints:vec![JointSpec {motor:motor(MotorKind::Position,50.0),..hinge(0,1,[0.0;3])},JointSpec {motor:motor(MotorKind::Position,0.5),..hinge(0,2,[0.0,20.0,0.0])},
            JointSpec {motor:motor(MotorKind::Speed,10.0),..hinge(0,3,[0.0,40.0,0.0])},
            JointSpec {kind:JointKind::Prismatic {lower:-10.0,upper:10.0},axis:[1.0,0.0,0.0],motor:motor(MotorKind::Position,5.0),..hinge(0,4,[0.0,60.0,0.0])}],
        substeps:8,..Assembly::default()};
    fixed(&mut asm,0);
    let mut world=DynWorld::new(&asm,[0.0,0.0,-GRAVITY],None).unwrap();
    assert_eq!(world.motors(),[(0,0.0),(1,0.0),(2,0.0),(3,0.0)],"every motor starts where its joint printed, or still");
    world.set_motor(3,50.0);
    assert_eq!(world.motors()[3],(3,10.0),"a servo's setting stays within its joint's range");
    world.set_motor(0,-0.5);
    world.set_motor(2,2.0);
    world.set_motor(3,5.0);
    for _ in 0..90 { world.step(DT); }
    let before=world.angles(&asm);
    for _ in 0..30 { world.step(DT); }
    let after=world.angles(&asm);
    assert!((after[0]+0.5).abs()<1f64.to_radians(),"the servo lifts its arm to -0.5 rad and holds it: {}",after[0]);
    assert!(after[1]>30f64.to_radians(),"a servo of 0.5 N mm cannot hold an arm that weighs 1 N mm: it sinks to {} rad",after[1]);
    let speed=(after[2]-before[2])/(30.0*DT);
    assert!((speed/2.0-1.0).abs()<0.05,"the drive spins its wheel at 2 rad/s: {speed}");
    assert!((after[3]-5.0).abs()<0.05,"the slider's servo goes to 5 mm: {}",after[3]);
    assert!(world.resets().is_empty(),"no motor blows the state up");
    // what the panel reads: the held arm's servo pushes the arm's weight times its reach, the sinking one all it
    // has, and the held arm's hinge carries the arm's weight
    let (efforts,loads)=(world.efforts(),world.loads());
    let weight=asm.bodies[1].mass.mass*GRAVITY/NEWTON;
    let held=weight*asm.bodies[1].mass.com[0]*0.5f64.cos();
    assert!((efforts[0].abs()/held-1.0).abs()<0.05,"the servo holding its arm pushes {held} N mm: {} N mm",efforts[0]);
    assert!((efforts[1].abs()-0.5).abs()<0.005,"the sinking arm's servo pushes all of its 0.5 N mm: {} N mm",efforts[1]);
    assert!(loads[0].is_some_and(|f| (f/weight-1.0).abs()<0.03),"the held arm's hinge carries its weight, {weight} N: {:?} N",loads[0]);
}

/// A solid given by its distance (model frame): the distance at a point and the way it grows.
#[derive(Debug)]
struct Solid { sd: fn([f32; 3]) -> f32, lo: [f32; 3], hi: [f32; 3] }

impl makepad_box3d::field::DistanceField for Solid {
    fn distance(&self, p: makepad_box3d::math_functions::Vec3) -> (f32, makepad_box3d::math_functions::Vec3) {
        let (f, e) = (|x: f32, y: f32, z: f32| (self.sd)([x, y, z]), 0.01);
        let g = [f(p.x + e, p.y, p.z) - f(p.x - e, p.y, p.z), f(p.x, p.y + e, p.z) - f(p.x, p.y - e, p.z), f(p.x, p.y, p.z + e) - f(p.x, p.y, p.z - e)];
        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-6);
        (f(p.x, p.y, p.z), makepad_box3d::math_functions::vec3(g[0] / l, g[1] / l, g[2] / l))
    }
    fn bounds(&self) -> makepad_box3d::math_functions::AABB {
        let v = |p: [f32; 3]| makepad_box3d::math_functions::vec3(p[0], p[1], p[2]);
        makepad_box3d::math_functions::AABB { lower_bound: v(self.lo), upper_bound: v(self.hi) }
    }
}

// The cup: 40 mm across outside, 3 mm walls and floor, 30 mm tall, standing on the floor; the ball: 6 mm
// in radius, printed inside it 11 mm above the cup's floor.
const CUP_HALF: f32 = 20.0;
const CUP_WALL: f32 = 3.0;
const CUP_RIM: f32 = 30.0;
const BALL_R: f32 = 6.0;
const BALL_Z: f32 = 20.0;

fn sd_box(p: [f32; 3], half: f32, z: [f32; 2]) -> f32 {
    let q = [p[0].abs() - half, p[1].abs() - half, (p[2] - (z[0] + z[1]) / 2.0).abs() - (z[1] - z[0]) / 2.0];
    (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt() + q[0].max(q[1]).max(q[2]).min(0.0)
}

fn field_body(name: &str, solid: Solid, com: P3, mass: f64, inertia: f64, hull: Vec<P3>) -> BodySpec {
    BodySpec { name: name.into(), mass: MassProps { volume: mass / 1.24e-3, mass, com, inertia: std::array::from_fn(|i| std::array::from_fn(|j| if i == j { inertia } else { 0.0 })) },
        hulls: vec![hull], tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: Some(SolidField(std::sync::Arc::new(solid))) }
}

#[test]
fn a_ball_in_a_cup_of_fields_rests_on_its_floor_and_its_wall_holds() {
    let (h, w, rim) = (CUP_HALF as f64, CUP_WALL as f64, CUP_RIM as f64);
    let cup = Solid { sd: |p| sd_box(p, CUP_HALF, [0.0, CUP_RIM]).max(-sd_box(p, CUP_HALF - CUP_WALL, [CUP_WALL, CUP_RIM + 10.0])), lo: [-CUP_HALF, -CUP_HALF, 0.0], hi: [CUP_HALF, CUP_HALF, CUP_RIM] };
    let corners = (0..8).map(|k| [if k & 1 == 0 { -h } else { h }, if k & 2 == 0 { -h } else { h }, if k & 4 == 0 { 0.0 } else { rim }]).collect();
    let r = BALL_R as f64;
    let centre = [0.0, 0.0, BALL_Z as f64];
    // the ball as Fab builds a part that fits in another: its mesh's hulls (which meet the floor) and mass, and
    // its field; the hulls keep within GAP_TOL of the mesh's whole hull in every direction (those of its
    // corners and of the axes and diagonals), with few of its corners
    let (mesh, faces) = icosphere(4, r, centre);
    let mut ball = makepad_csg_mech::mesh_body("ball", &mesh, &faces, 1.24e-3, 1.0, &[]);
    let kept: Vec<P3> = ball.hulls.iter().flatten().copied().collect();
    let dirs: Vec<P3> = mesh.iter().map(|&p| norm(sub(p, centre))).chain((0..27).filter(|&k| k != 13).map(|k| norm([(k % 3) as f64 - 1.0, ((k / 3) % 3) as f64 - 1.0, (k / 9) as f64 - 1.0]))).collect();
    let short = short_of(&kept, &mesh, &dirs);
    println!("hulls: {} of the ball's {} corners in {} hulls, {short:.4} mm short of its whole hull", kept.len(), mesh.len(), ball.hulls.len());
    assert!(short <= makepad_csg_mech::GAP_TOL + 1e-9, "the hulls keep within GAP_TOL of the ball's: {short:.4} mm short");
    assert!(kept.len() < mesh.len() / 2, "the hulls keep few of its corners: {} of {}", kept.len(), mesh.len());
    ball.field = Some(SolidField(std::sync::Arc::new(Solid { sd: |p| (p[0] * p[0] + p[1] * p[1] + (p[2] - BALL_Z) * (p[2] - BALL_Z)).sqrt() - BALL_R, lo: [-BALL_R, -BALL_R, BALL_Z - BALL_R], hi: [BALL_R, BALL_R, BALL_Z + BALL_R] })));
    let mut asm = Assembly { bodies: vec![field_body("cup", cup, [0.0, 0.0, 12.0], 20.0, 5000.0, corners), ball], up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, ..Assembly::default() };
    // parts that meet through their fields run on the engine; beside balls (a support check's pieces) they stay on box3d
    assert!(runner::on_dynamics(&asm), "a piece of fields runs on the dynamics engine");
    let mut with_balls = asm.clone();
    with_balls.bodies.push(BodySpec { balls: vec![([30.0, 0.0, 5.0], 1.5)], ..cuboid("piece", [30.0, 0.0, 5.0], [2.0; 3], 0.1) });
    assert!(!runner::on_dynamics(&with_balls) && DynWorld::new(&with_balls, [0.0, 0.0, -GRAVITY], Some(0.0)).is_err(), "balls meet fields on box3d");
    let mut world = runner::dynamics_world(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    // dropped, the ball lands on the cup's floor and is asleep there within a second, with no kick once landed (its
    // speed never jumps), and stays asleep; it stays inside (their hulls meet only the floor: the cup's would fill
    // it) and the cup's hull stands on the floor
    let (mut floor_in, mut wall_in): (f64, f64) = (f64::MIN, f64::MIN);
    let (mut asleep_at, mut kick, mut last) = (None, 0.0f64, 0.0);
    for k in 0..360 {
        world.step(DT);
        let (t, v) = ((k + 1) as f64 * DT, world.max_speed());
        if t > 0.1 {
            kick = kick.max(v - last);
        }
        last = v;
        if asleep_at.is_none() && world.sleeping().contains(&1) {
            asleep_at = Some(t);
        }
        if k >= 120 {
            let (cup_at, ball_at) = (world.poses()[0].com, world.poses()[1].com);
            let at = sub(ball_at, sub(cup_at, [0.0, 0.0, 12.0]));
            floor_in = floor_in.max(w + r - at[2]);
            wall_in = wall_in.max(at[0].abs().max(at[1].abs()) + r - (h - w));
        }
    }
    let cup_at = world.poses()[0].com;
    println!("fields: landed, asleep at {asleep_at:?} s, speed jumps by {kick:.2} mm/s at most; the ball went {floor_in:.3} mm into the cup's floor and {wall_in:.3} into a wall; the cup at {cup_at:?}");
    assert!(asleep_at.is_some_and(|t| t < 1.0) && world.sleeping().contains(&1), "the ball is asleep within a second and stays so: {asleep_at:?}");
    assert!(kick < 10.0, "no kick once landed: its speed jumps by {kick:.2} mm/s");
    assert!(floor_in > -0.5 && floor_in < 0.35, "the ball rests on the cup's floor: {floor_in:.3} mm into it");
    assert!(wall_in < 0.3, "the cup's wall keeps the ball in: {wall_in:.3} mm into it");
    assert!(len(sub(cup_at, [0.0, 0.0, 12.0])) < 0.3, "the cup stands where it printed: {cup_at:?}");
    // pulled hard into the wall and held there two seconds (the cup held: the hand pulls with up to 200 times
    // the ball's weight), the ball reaches it and stays out of it
    fixed(&mut asm, 0);
    let mut world = runner::dynamics_world(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    for _ in 0..120 { world.step(DT); }
    let ball_at = world.poses()[1].com;
    world.grab(1, ball_at, runner::POINTER_HZ);
    let mut deepest: f64 = f64::MIN;
    for k in 0..480 {
        world.drag([ball_at[0] + 30.0 * (k as f64 / 60.0).min(1.0), ball_at[1], ball_at[2]], DT);
        world.step(DT);
        deepest = deepest.max(world.poses()[1].com[0] + r - (h - w));
    }
    println!("fields: pulled into the wall and held, the ball went {deepest:.3} mm into it");
    assert!(deepest > -0.5 && deepest < 0.5, "the ball reaches the cup's wall and stays out of it: {deepest:.3} mm into it");
}

#[test]
fn a_cup_of_fields_in_another_rests_asleep_and_their_walls_hold() {
    let (h, w, rim) = (CUP_HALF as f64, CUP_WALL as f64, CUP_RIM as f64);
    let outer = Solid { sd: |p| sd_box(p, CUP_HALF, [0.0, CUP_RIM]).max(-sd_box(p, CUP_HALF - CUP_WALL, [CUP_WALL, CUP_RIM + 10.0])), lo: [-CUP_HALF, -CUP_HALF, 0.0], hi: [CUP_HALF, CUP_HALF, CUP_RIM] };
    let corners = |half: f64, z: [f64; 2]| (0..8).map(|k| [if k & 1 == 0 { -half } else { half }, if k & 2 == 0 { -half } else { half }, if k & 4 == 0 { z[0] } else { z[1] }]).collect();
    // the inner cup: 24 mm across, 2 mm walls and floor, 15 mm tall, printed 0.05 mm over the outer one's floor
    let inner = Solid { sd: |p| sd_box(p, 12.0, [3.05, 18.05]).max(-sd_box(p, 10.0, [5.05, 30.0])), lo: [-12.0, -12.0, 3.05], hi: [12.0, 12.0, 18.05] };
    let mut asm = Assembly {
        bodies: vec![field_body("outer", outer, [0.0, 0.0, 12.0], 20.0, 5000.0, corners(h, [0.0, rim])), field_body("inner", inner, [0.0, 0.0, 9.04], 4.27, 285.0, corners(12.0, [3.05, 18.05]))],
        up: [0.0, 0.0, 1.0],
        sleep: SLEEP_SPEED,
        ..Assembly::default()
    };
    let mut world = runner::dynamics_world(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    // it settles on the outer one's floor and is asleep within a second, with no kick, and stays asleep
    let (mut asleep_at, mut kick, mut last, mut floor_in) = (None, 0.0f64, 0.0, f64::MIN);
    for k in 1..=480 {
        world.step(DT);
        let (t, v) = (k as f64 * DT, world.max_speed());
        kick = kick.max(v - last);
        last = v;
        floor_in = floor_in.max(w - (world.poses()[1].com[2] - (9.04 - 3.05)));
        if asleep_at.is_none() && world.sleeping().contains(&1) {
            asleep_at = Some(t);
        }
    }
    println!("cups of fields: asleep at {asleep_at:?} s, speed jumps by {kick:.2} mm/s at most, {floor_in:.3} mm into the outer one's floor");
    assert!(asleep_at.is_some_and(|t| t < 1.0) && world.sleeping().contains(&1), "the inner cup is asleep within a second and stays so: {asleep_at:?}");
    assert!(kick < 10.0, "no kick: its speed jumps by {kick:.2} mm/s");
    assert!(floor_in < 0.1, "it rests on the outer one's floor: {floor_in:.3} mm into it");
    // pulled hard into a wall and held there two seconds (the outer cup held), it reaches the wall and stays out
    fixed(&mut asm, 0);
    let mut world = runner::dynamics_world(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    for _ in 0..120 { world.step(DT); }
    let at = world.poses()[1].com;
    world.grab(1, at, runner::POINTER_HZ);
    let mut deepest: f64 = f64::MIN;
    for k in 0..480 {
        world.drag([at[0] + 30.0 * (k as f64 / 60.0).min(1.0), at[1], at[2]], DT);
        world.step(DT);
        deepest = deepest.max(world.poses()[1].com[0] + 12.0 - (h - w));
    }
    println!("cups of fields: pulled into the wall and held, the inner one went {deepest:.3} mm into it");
    assert!(deepest > -0.5 && deepest < 0.5, "the inner cup reaches the wall and stays out of it: {deepest:.3} mm into it");
}

/// A sphere of radius `r` about `c` as a closed mesh: an icosahedron's faces split `n` times, pushed out to it.
fn icosphere(n: usize, r: f64, c: P3) -> (Vec<P3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut v: Vec<P3> = [[-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0], [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t], [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0]]
        .iter()
        .map(|p| norm(*p))
        .collect();
    let mut f: Vec<[u32; 3]> = vec![[0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8], [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1]];
    for _ in 0..n {
        let mut mid = std::collections::HashMap::new();
        let mut half = |a: u32, b: u32, v: &mut Vec<P3>| *mid.entry((a.min(b), a.max(b))).or_insert_with(|| { v.push(norm(add(v[a as usize], v[b as usize]))); v.len() as u32 - 1 });
        f = f.iter().flat_map(|&[a, b, c]| { let (ab, bc, ca) = (half(a, b, &mut v), half(b, c, &mut v), half(c, a, &mut v)); [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]] }).collect();
    }
    (v.into_iter().map(|p| add(c, scale(p, r))).collect(), f)
}

/// How far inside `whole`'s hull `part`'s hull falls short along `dirs` at most (their support functions'
/// largest gap: for two convex hulls over all directions, the farthest a point of one lies from the other).
fn short_of(part: &[P3], whole: &[P3], dirs: &[P3]) -> f64 {
    let reach = |pts: &[P3], d: P3| pts.iter().map(|&p| dot(p, d)).fold(f64::MIN, f64::max);
    dirs.iter().map(|&d| reach(whole, d) - reach(part, d)).fold(0.0, f64::max)
}

// The block: 20 mm on a side, standing on the floor, with a hole 2.5 mm across down to 5 mm over its bottom;
// the pin: 2 mm across and 16 mm long, printed in the hole 1 mm over its bottom, 0.15 mm off its axis and
// leaning 0.3 degrees to that side.
const HOLE_R: f32 = 1.25;
const HOLE_FLOOR: f32 = 5.0;
const PIN_R: f64 = 1.0;
const PIN_LEN: f64 = 16.0;
const PIN_FOOT: f64 = HOLE_FLOOR as f64 + 1.0;
const PIN_OFF: f64 = 0.15;
const PIN_LEAN: f64 = 0.3;

/// The pin's distance: a cylinder about its leaning axis, its ends flat.
fn sd_pin(p: [f32; 3]) -> f32 {
    let (s, c) = PIN_LEAN.to_radians().sin_cos();
    let (u, w) = (p[0] as f64 - PIN_OFF, p[2] as f64 - PIN_FOOT);
    let q = [c * u - s * w, p[1] as f64, s * u + c * w];
    let d = [(q[0] * q[0] + q[1] * q[1]).sqrt() - PIN_R, (q[2] - PIN_LEN / 2.0).abs() - PIN_LEN / 2.0];
    (d[0].max(d[1]).min(0.0) + (d[0].max(0.0).powi(2) + d[1].max(0.0).powi(2)).sqrt()) as f32
}

#[test]
fn a_pin_of_a_field_rests_in_a_hole_of_a_field_and_sleeps() {
    // as Fab builds a part that fits in another: its mesh's hulls (which meet the floor) and mass, and its field
    let n = 48;
    // a closed cylinder: two rims of `n` corners, their caps fanned from their centres
    let mut pin: Vec<P3> = (0..2 * n).map(|i| { let a = (i % n) as f64 * std::f64::consts::TAU / n as f64; [PIN_R * a.cos(), PIN_R * a.sin(), PIN_FOOT + if i < n { 0.0 } else { PIN_LEN }] }).collect();
    pin.extend([[0.0, 0.0, PIN_FOOT], [0.0, 0.0, PIN_FOOT + PIN_LEN]]);
    let lean = PIN_LEAN.to_radians();
    let lean = |p: P3| [PIN_OFF + p[0] * lean.cos() + (p[2] - PIN_FOOT) * lean.sin(), p[1], PIN_FOOT - p[0] * lean.sin() + (p[2] - PIN_FOOT) * lean.cos()];
    let pin: Vec<P3> = pin.into_iter().map(lean).collect();
    let (foot, head) = (pin[2 * n], pin[2 * n + 1]);
    let (n32, low, high) = (n as u32, 2 * n as u32, 2 * n as u32 + 1);
    let tris: Vec<[u32; 3]> = (0..n32).flat_map(|i| { let j = (i + 1) % n32; [[i, j, n32 + j], [i, n32 + j, n32 + i], [low, j, i], [high, n32 + i, n32 + j]] }).collect();
    let mut body = makepad_csg_mech::mesh_body("pin", &pin, &tris, 1.24e-3, 1.0, &[]);
    let (lo, hi) = pin.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), v| (std::array::from_fn(|k| lo[k].min(v[k])), std::array::from_fn(|k| hi[k].max(v[k]))));
    body.field = Some(SolidField(std::sync::Arc::new(Solid { sd: sd_pin, lo: lo.map(|v| v as f32), hi: hi.map(|v| v as f32) })));
    let block = Solid {
        sd: |p| sd_box(p, 10.0, [0.0, 20.0]).max(-((p[0] * p[0] + p[1] * p[1]).sqrt() - HOLE_R).max(HOLE_FLOOR - p[2])),
        lo: [-10.0, -10.0, 0.0],
        hi: [10.0, 10.0, 20.0],
    };
    let corners = (0..8).map(|k| [if k & 1 == 0 { -10.0 } else { 10.0 }, if k & 2 == 0 { -10.0 } else { 10.0 }, if k & 4 == 0 { 0.0 } else { 20.0 }]).collect();
    let asm = Assembly { bodies: vec![field_body("block", block, [0.0, 0.0, 10.0], 10.0, 700.0, corners), body], up: [0.0, 0.0, 1.0], sleep: SLEEP_SPEED, ..Assembly::default() };
    let mut world = runner::dynamics_world(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    // it drops to the hole's bottom, stands on its end and is asleep within a second, with no kick once landed,
    // and stays asleep and in the hole
    let (mut asleep_at, mut kick, mut last, mut off, mut low_z) = (None, 0.0f64, 0.0, 0.0f64, f64::MAX);
    for k in 1..=480 {
        world.step(DT);
        let (t, v) = (k as f64 * DT, world.max_speed());
        let pose = world.poses()[1];
        let rest = world.rest_com(1);
        let (foot, head) = (pose.apply(rest, foot), pose.apply(rest, head));
        off = off.max(len([foot[0], foot[1], 0.0])).max(len([head[0], head[1], 0.0]));
        low_z = low_z.min(foot[2]);
        if t > 0.1 {
            kick = kick.max(v - last);
        }
        last = v;
        if asleep_at.is_none() && world.sleeping().contains(&1) {
            asleep_at = Some(t);
        }
    }
    println!("pin in a hole: asleep at {asleep_at:?} s, speed jumps by {kick:.2} mm/s at most once landed, axis up to {off:.3} mm off the hole's, foot down to {low_z:.3} mm");
    assert!(asleep_at.is_some_and(|t| t < 1.0) && world.sleeping().contains(&1), "the pin is asleep within a second and stays so: {asleep_at:?}");
    assert!(kick < 10.0, "no kick once landed: its speed jumps by {kick:.2} mm/s");
    assert!(off < (HOLE_R as f64 - PIN_R) + 0.05, "the pin stays in the hole: its axis {off:.3} mm off the hole's");
    assert!(low_z > HOLE_FLOOR as f64 - 0.1, "the pin rests on the hole's bottom, its contacts giving less than 0.1 mm: its foot at {low_z:.3} mm");
}
