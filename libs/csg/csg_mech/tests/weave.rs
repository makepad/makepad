use makepad_csg_mech::joint::Profile;
use makepad_csg_mech::sim::{AnchorKind, AnchorSpec, Friction, MechWorld};
use makepad_csg_mech::weave::*;

fn setup() -> (WeaveProfile, SquareLink, Frame) {
    let p = Profile::default();
    let wp = WeaveProfile::new(&p, None);
    (wp, SquareLink::new(&wp, None), Frame::new([0.0; 3], p.up))
}

#[test]
fn chain_links_interlock_and_print_free() {
    let (wp, link, frame) = setup();
    let w = chain(&wp, link, frame, &[[0.0, 0.0], [100.0, 0.0]], false);
    assert_eq!(w.links.len(), 12);
    for (a, b) in &w.pairs {
        assert_eq!(linking_number(&w.links[*a as usize].pts, &w.links[*b as usize].pts).abs(), 1);
    }
    let c = check(&w, &wp);
    assert_eq!(c.verdict, GapClass::Ok, "{c:?}");
    assert!((c.min_gap_z.0 - wp.c_z).abs() < 1e-9);
    assert!(c.bridge <= wp.bridge_max && c.leg_deg >= 44.99);
}

#[test]
fn mail_is_four_in_one_and_clipped() {
    let (wp, link, frame) = setup();
    let w = mail(&wp, link, frame, [-40.0, -40.0], [40.0, 40.0], 0.0, &|_| -1.0);
    let mut degree = vec![0; w.links.len()];
    for (a, b) in &w.pairs {
        degree[*a as usize] += 1;
        degree[*b as usize] += 1;
    }
    assert_eq!(degree.iter().copied().max(), Some(4));
    let c = check(&w, &wp);
    assert_eq!(c.verdict, GapClass::Ok, "{c:?}");
    assert!(c.tangled.is_empty() && c.unlinked.is_empty());
    let disc = mail(&wp, link, frame, [-40.0, -40.0], [40.0, 40.0], 0.0, &|p| (p[0] * p[0] + p[1] * p[1]).sqrt() - 30.0);
    assert!(disc.links.len() < w.links.len() && !disc.links.is_empty());
    for l in &disc.links {
        let c = frame.to_local(l.center());
        assert!((c[0] * c[0] + c[1] * c[1]).sqrt() < 30.0);
    }
}

#[test]
fn tight_bracelet_is_flagged() {
    let (wp, link, frame) = setup();
    let circle = |r: f64| -> Vec<[f64; 2]> { (0..128).map(|k| { let t = k as f64 / 128.0 * std::f64::consts::TAU; [r * t.cos(), r * t.sin()] }).collect() };
    assert_eq!(check(&chain(&wp, link, frame, &circle(30.0), true), &wp).verdict, GapClass::Ok);
    assert_ne!(check(&chain(&wp, link, frame, &circle(15.0), true), &wp).verdict, GapClass::Ok);
}

#[test]
fn hanging_mail_stays_linked() {
    let (wp, link, frame) = setup();
    let w = mail(&wp, link, frame, [-20.0, -20.0], [20.0, 20.0], 0.0, &|_| -1.0);
    let mut asm = assembly(&w, 1.24e-3, 240.0);
    let ys: Vec<f64> = w.links.iter().map(|l| frame.to_local(l.center())[1]).collect();
    let top = ys.iter().cloned().fold(f64::MIN, f64::max);
    for (i, y) in ys.iter().enumerate() {
        if *y > top - 1e-6 {
            asm.anchors.push(AnchorSpec { body: i, pivot: asm.bodies[i].mass.com, kind: AnchorKind::Fixed, friction: Friction::default() });
        }
    }
    let rest: Vec<_> = asm.bodies.iter().map(|b| b.mass.com).collect();
    let mut mw = MechWorld::new(&asm, makepad_csg_mech::geom::scale(frame.y, -9810.0), None);
    for _ in 0..480 {
        mw.step(1.0 / 240.0);
    }
    let poses = mw.poses();
    let posed: Vec<Vec<_>> = w.links.iter().enumerate().map(|(i, l)| l.pts.iter().map(|p| poses[i].apply(rest[i], *p)).collect()).collect();
    let unlinked = w.pairs.iter().filter(|(a, b)| linking_number(&posed[*a as usize], &posed[*b as usize]) == 0).count();
    assert_eq!(unlinked, 0);
    // it hangs: the bottom row fell below where it was printed
    let low = (0..w.links.len()).map(|i| frame.to_local(poses[i].com)[1] - frame.to_local(rest[i])[1]).fold(0.0, f64::min);
    assert!(low < -1.0, "moved {low}");
}

#[test]
fn beads_of_a_clean_weave_stay_apart() {
    let (wp, link, frame) = setup();
    let w = chain(&wp, link, frame, &[[0.0, 0.0], [40.0, 0.0]], false);
    let links = w.links();
    // ideal beads: each link's outer wall traced at every layer as a loop of short beads along its centreline
    // (width 0.42, height 0.2): the gap to the neighbours is the design clearance less the bead's reach
    let mut beads = Vec::new();
    for l in &w.links {
        for (a, b) in l.segments() {
            beads.push(Bead { a, b, width: 0.42, height: 0.2, over_air: false });
        }
    }
    let c = check_beads(&links, &beads, frame.up, 0.15);
    assert_eq!(c.stray, 0);
    assert!(c.fused.is_empty(), "{:?}", c.fused.first());
    // a bead laid across two links bridges them
    let (p, q) = (w.links[0].center(), w.links[1].center());
    let c2 = check_beads(&links, &[Bead { a: madd_(p, frame.up, 0.6), b: madd_(q, frame.up, 0.6), width: 0.42, height: 0.2, over_air: true }], frame.up, 0.15);
    assert!(!c2.fused.is_empty());
}

fn madd_(a: [f64; 3], b: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

#[test]
fn scenes_keep_a_mail_together() {
    let (wp, link, frame) = setup();
    let w = mail(&wp, link, frame, [-20.0, -20.0], [20.0, 20.0], 0.0, &|_| -1.0);
    for scene in [WeaveScene::Drop, WeaveScene::Drape, WeaveScene::Hang] {
        let r = simulate(&w, 1.24e-3, scene, 2.0, &|| false);
        println!("{r:?}");
        assert_eq!(r.unlinked, 0, "{scene:?}");
        assert!(r.deepest > -0.5, "{scene:?} {}", r.deepest);
        assert!(r.drop > 1.0, "{scene:?} fell {}", r.drop);
    }
    let c = chain(&wp, link, frame, &[[0.0, 0.0], [80.0, 0.0]], false);
    let r = simulate(&c, 1.24e-3, WeaveScene::Hang, 3.0, &|| false);
    assert_eq!(r.unlinked, 0);
}

#[test]
fn graded_mail_interlocks_at_every_size() {
    let (wp, link, frame) = setup();
    // links grow from the smallest at the centre to 1.5 mm larger at the edges (bridges stay under 10 mm), per column and per row
    let grow = |u: f64| link.a + 1.5 * (u.abs() / 40.0).min(1.0);
    let w = mail_graded(&wp, link, frame, [-40.0, -40.0], [40.0, 40.0], 0.0, &|_| -1.0, &grow, &grow);
    let sizes: Vec<f64> = w.links.iter().map(|l| l.bounds()).map(|(lo, hi)| hi[0] - lo[0]).collect();
    let (small, big) = sizes.iter().fold((f64::MAX, 0.0f64), |(a, b), s| (a.min(*s), b.max(*s)));
    assert!(big - small > 2.0, "{small} {big}");
    let mut degree = vec![0; w.links.len()];
    for (a, b) in &w.pairs {
        degree[*a as usize] += 1;
        degree[*b as usize] += 1;
    }
    assert_eq!(degree.iter().copied().max(), Some(4));
    let c = check(&w, &wp);
    assert_eq!(c.verdict, GapClass::Ok, "{c:?}");
    assert!(c.tangled.is_empty() && c.unlinked.is_empty());
    assert!(c.bridge <= wp.bridge_max, "{}", c.bridge);
    assert!((c.min_gap_z.0 - wp.c_z).abs() < 1e-6, "{:?}", c.min_gap_z);
    assert!(c.min_gap_xy.0 >= wp.c_xy - 1e-6, "{:?}", c.min_gap_xy);
    let r = simulate(&w, 1.24e-3, WeaveScene::Drop, 1.5, &|| false);
    assert_eq!(r.unlinked, 0, "{r:?}");
}

#[test]
fn graded_chain_and_bracelet_interlock() {
    let (wp, link, frame) = setup();
    let w = chain_graded(&wp, link, frame, &[[0.0, 0.0], [150.0, 0.0]], false, &|t| link.a + 2.0 * t / 150.0);
    let c = check(&w, &wp);
    assert_eq!(c.verdict, GapClass::Ok, "{c:?}");
    assert!(w.links.len() > 8 && c.unlinked.is_empty() && c.tangled.is_empty());
    let circle: Vec<[f64; 2]> = (0..128).map(|k| { let t = k as f64 / 128.0 * std::f64::consts::TAU; [40.0 * t.cos(), 40.0 * t.sin()] }).collect();
    let b = chain_graded(&wp, link, frame, &circle, true, &|t| link.a + 1.5 * (t / 40.0).sin().abs());
    let c = check(&b, &wp);
    assert_eq!(c.verdict, GapClass::Ok, "{c:?}");
    assert!(c.unlinked.is_empty() && c.tangled.is_empty());
}

/// A box filled with leaning links: every link stands on the bed, interlocks with two links in every row of the
/// other family it crosses, and keeps its clearances (in the layer and across layers) at every lean tried.
#[test]
fn trellis_fills_a_box_standing_on_the_bed() {
    let (wp, _, frame) = setup();
    let mut fails = Vec::new();
    for deg in [45.0f64, 55.0, 60.0, 65.0] {
        let link = LeanLink::new(&wp, deg.to_radians());
        let w = trellis(&wp, link, frame, [-15.0, -15.0, 0.0], [15.0, 15.0, 20.0], 0.0, 0.0, &|_| -1.0);
        let c = check(&w, &wp);
        let mut degree = vec![0; w.links.len()];
        for (a, b) in &w.pairs {
            degree[*a as usize] += 1;
            degree[*b as usize] += 1;
        }
        let (pieces, loose) = w.pieces();
        let top = w.links.iter().flat_map(|l| l.pts.iter()).map(|p| frame.to_local(*p)[2]).fold(f64::MIN, f64::max);
        let low = w.links.iter().map(|l| l.pts.iter().map(|p| frame.to_local(*p)[2]).fold(f64::MAX, f64::min)).fold(f64::MIN, f64::max);
        println!(
            "lean {deg}: d {:.2} q {:.2} links {} pairs {} max degree {:?} pieces {pieces} loose {loose} gap z {:.3} xy {:.3} bridge {:.2} leg {:.1} top {:.1} highest bottom {:.2} close {:?} unlinked {} tangled {}",
            link.d, link.q, w.links.len(), w.pairs.len(), degree.iter().max(), c.min_gap_z.0, c.min_gap_xy.0, c.bridge, c.leg_deg, top, low, c.close.iter().take(3).collect::<Vec<_>>(), c.unlinked.len(), c.tangled.len()
        );
        if c.verdict != GapClass::Ok || c.min_gap_xy.0 < wp.c_xy - 1e-6 || c.min_gap_z.0 < wp.c_z - 1e-6 {
            fails.push((deg, c.min_gap_xy.0, c.min_gap_z.0));
            continue;
        }
        assert!(c.unlinked.is_empty() && c.tangled.is_empty(), "lean {deg}");
        assert_eq!((pieces, loose), (1, 0), "lean {deg}");
        assert!(c.bridge <= wp.bridge_max && c.leg_deg >= wp.leg.to_degrees() - 1.0, "lean {deg}: bridge {} leg {}", c.bridge, c.leg_deg);
        // every link's lowest point is on the bed (its bottom bar)
        assert!(low <= link.lo + 1e-6, "lean {deg}: a link starts {low} mm up");
        assert!(top <= 20.0 + 1e-6 && top > 12.0, "lean {deg}: top {top}");
    }
    assert!(fails.is_empty(), "short of the clearances (lean, in layer, across): {fails:?}");
    // dropped, the fill holds together and its links move against each other
    let w = trellis(&wp, LeanLink::new(&wp, 60f64.to_radians()), frame, [-15.0, -15.0, 0.0], [15.0, 15.0, 20.0], 0.0, 0.0, &|_| -1.0);
    let r = simulate(&w, 1.24e-3, WeaveScene::Drop, 2.0, &|| false);
    println!("{r:?}");
    assert_eq!(r.unlinked, 0);
    assert!(r.pair_motion > wp.c_xy, "pairs moved {}", r.pair_motion);
}
