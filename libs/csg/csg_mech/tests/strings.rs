//! Strings on the dynamics engine (millimetres, grams, seconds): a hung part barely stretches its string,
//! a slack string pulls nothing, an eyelet slides along its string, the builder refuses a string that
//! starts outside its part or stretched, and a held bar carries its strings under the cursor.
use makepad_csg_mech::dyn_world::{check_strings, DynWorld};
use makepad_csg_mech::geom::*;
use makepad_csg_mech::mass::MassProps;
use makepad_csg_mech::runner::on_dynamics;
use makepad_csg_mech::sim::{AnchorKind, AnchorSpec, Assembly, BodySpec, Friction, StringFault, StringSpec, GRAVITY, NEWTON};

const DT: f64 = 1.0 / 240.0;

fn cuboid(name: &str, center: P3, half: P3, mass: f64) -> BodySpec {
    let corners = (0..8).map(|k| std::array::from_fn(|i| center[i] + if k >> i & 1 == 0 { -half[i] } else { half[i] })).collect();
    let inertia = std::array::from_fn(|i| std::array::from_fn(|j| if i == j {
        let (u, v) = ((i + 1) % 3, (i + 2) % 3);
        mass / 3.0 * (half[u] * half[u] + half[v] * half[v])
    } else { 0.0 }));
    BodySpec { name: name.into(), mass: MassProps { volume: mass / 1.24e-3, mass, com: center, inertia },
        rods: Vec::new(), tubes: Vec::new(), hulls: vec![corners], balls: Vec::new(), field: None }
}

fn path(points: &[P3]) -> f64 { points.windows(2).map(|w| len(sub(w[1], w[0]))).sum() }

// A bar held fixed. Hung from it on a string that starts taut: a block. On a string from the bar's middle
// to its end through its eyelet: a bead, printed off the middle. On the floor: a box, with a slack string
// up to the bar when `slack` is set.
fn strung(slack: bool) -> Assembly {
    let mut asm = Assembly {
        bodies: vec![
            cuboid("bar", [0.0, 0.0, 200.0], [110.0, 5.0, 5.0], 50.0),
            cuboid("hung", [-60.0, 0.0, 120.0], [10.0, 10.0, 10.0], 10.0),
            cuboid("bead", [40.0, 0.0, 150.0], [5.0, 5.0, 5.0], 2.0),
            cuboid("box", [0.0, 0.0, 10.0], [10.0, 10.0, 10.0], 10.0),
        ],
        substeps: 8,
        ..Assembly::default()
    };
    asm.anchors.push(AnchorSpec { body: 0, pivot: [0.0, 0.0, 200.0], kind: AnchorKind::Fixed, friction: Friction::default() });
    asm.strings.push(StringSpec { a: 0, from: [-60.0, 0.0, 195.0], b: 1, to: [-60.0, 0.0, 130.0], length: 65.0, ..StringSpec::default() });
    asm.strings.push(StringSpec { a: 0, from: [20.0, 0.0, 195.0], through: vec![(2, [40.0, 0.0, 150.0])], b: 0, to: [100.0, 0.0, 195.0], length: 140.0 });
    if slack {
        asm.strings.push(StringSpec { a: 0, from: [0.0, 0.0, 195.0], b: 3, to: [0.0, 0.0, 20.0], length: 300.0, ..StringSpec::default() });
    }
    asm
}

#[test]
fn strings_hang_slacken_and_slide() {
    let asm = strung(true);
    assert!(on_dynamics(&asm), "a strung piece runs on the dynamics engine, joints or not");
    let gravity = [0.0, 0.0, -GRAVITY];
    let mut world = DynWorld::new(&asm, gravity, Some(0.0)).unwrap();
    let mut without = DynWorld::new(&strung(false), gravity, Some(0.0)).unwrap();
    let mut longest_first = 0.0_f64;
    for _ in 0..480 {
        world.step(DT);
        without.step(DT);
        let bead = &world.strings()[1].points;
        longest_first = longest_first.max(len(sub(bead[1], bead[0])));
    }
    let strings = world.strings();
    assert_eq!(strings.iter().map(|s| s.points.len()).collect::<Vec<_>>(), [2, 3, 2], "a path point per string point");
    let stretch = path(&strings[0].points) - asm.strings[0].length;
    assert!(stretch.abs() < 0.1, "a hung part stretches its string under 0.1 mm: {stretch} mm");
    assert_eq!(strings[0].slack, 0.0, "a hung string is taut");
    let weight = asm.bodies[1].mass.mass * GRAVITY / NEWTON;
    assert!((strings[0].tension / weight - 1.0).abs() < 0.02, "a hung part's string pulls with its weight, {weight} N: {} N", strings[0].tension);
    assert!(strings[2].slack > 100.0, "the box's string stays slack: {} mm", strings[2].slack);
    assert_eq!(strings[2].tension, 0.0, "a slack string pulls with nothing");
    assert_eq!(world.poses(), without.poses(), "a slack string pulls nothing");
    // At a fixed eyelet the first stretch would stay 49 mm; slid to the middle it is 70 mm.
    assert!(longest_first > 65.0, "the bead slides along its string: first stretch at most {longest_first} mm");
    assert!(strings[1].slack < 1.0, "the bead hangs on its string: slack {} mm", strings[1].slack);

    let refused = |edit: &dyn Fn(&mut Assembly)| {
        let mut a = strung(false);
        edit(&mut a);
        let fault = check_strings(&a).expect_err("a refusal");
        let built = DynWorld::new(&a, gravity, Some(0.0)).err().expect("the world refuses it too");
        assert_eq!(built, fault.to_string(), "the world's refusal is the check's");
        fault
    };
    let fault = refused(&|a| a.strings[0].to = [-60.0, 0.0, 140.0]);
    assert_eq!(fault, StringFault::Outside { string: 0, point: 1, points: 2, part: "hung".into() }, "an end outside its part");
    assert_eq!(fault.to_string(), "string 0: its end is not inside part 'hung'", "the refusal names the string and the part");
    let fault = refused(&|a| a.strings[1].through[0].1 = [40.0, 20.0, 150.0]);
    assert_eq!(fault.to_string(), "string 1: its eyelet 1 is not inside part 'bead'", "an eyelet outside its part");
    assert_eq!(refused(&|a| a.strings[0].length = 60.0), StringFault::TooShort { string: 0, path: 65.0, length: 60.0 }, "a string that starts stretched");
    assert_eq!(refused(&|a| a.strings[1].b = 9), StringFault::NoPart { string: 1, part: 9 }, "a part the piece does not have");
}

#[test]
fn a_held_bar_carries_its_strings() {
    let mut asm = strung(false);
    asm.anchors[0].kind = AnchorKind::Held;
    assert!(on_dynamics(&Assembly { strings: Vec::new(), ..asm.clone() }), "a held part runs on the dynamics engine");
    let mut world = DynWorld::new(&asm, [0.0, 0.0, -GRAVITY], Some(0.0)).unwrap();
    let rest = world.rest_com(0);
    let (hold, eye) = ([100.0, 0.0, 200.0], asm.strings[0].from);
    world.grab(0, hold, 10.0);
    assert_eq!(world.grabbed(), Some(0), "the hand holds the bar");
    let (mut off_cursor, mut string_lag, mut hung_x) = (0.0_f64, 0.0_f64, f64::NEG_INFINITY);
    for k in 1..=120 {
        // half a second at 120 mm/s across and 60 mm/s up
        let cursor = add(hold, [0.5 * k as f64, 0.0, 0.25 * k as f64]);
        world.drag(cursor, DT);
        world.step(DT);
        let bar = world.poses()[0];
        off_cursor = off_cursor.max(len(sub(bar.apply(rest, hold), cursor)));
        string_lag = string_lag.max(len(sub(world.strings()[0].points[0], bar.apply(rest, eye))));
        hung_x = hung_x.max(world.poses()[1].com[0]);
    }
    // Pose::apply rounds through f32: about 1e-5 mm here.
    assert!(off_cursor < 1e-3, "the held point stays under the cursor: {off_cursor} mm off");
    assert!(string_lag < 1e-3, "the string leaves the bar where the bar is, on the same step: {string_lag} mm behind");
    world.release();
    assert_eq!(world.grabbed(), None, "let go");
    let let_go = world.poses()[0];
    for _ in 0..240 {
        world.step(DT);
        hung_x = hung_x.max(world.poses()[1].com[0]);
    }
    let now = world.poses()[0];
    assert!(len(sub(now.com, let_go.com)) < 1e-6 && now.rot == let_go.rot, "the bar stays where it was let go");
    assert!(hung_x > -10.0, "the hung part came along under the bar, from x -60 to x 0: at most x {hung_x} mm");
}
