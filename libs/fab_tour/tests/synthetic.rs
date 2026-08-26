//! Property tests on procedurally generated buildings.
//!
//! The generator builds rooms by BSP-splitting a rectangle and putting exactly
//! one doorway in every split wall, so the room graph is a tree and "every
//! room is reachable" holds by construction. Anything the planner fails to
//! visit is therefore the planner's fault.

use makepad_fab_tour::*;

fn analyse(scene: &TourScene) -> SiteAnalysis {
    SiteAnalysis::analyse(scene, &AnalysisConfig::default())
}

#[test]
fn every_generated_track_passes_qa() {
    let limits = QaLimits::default();
    let mut checked = 0;
    for seed in 1..=6u64 {
        let scene = synthetic::building(&synthetic::Plan {
            seed,
            ..Default::default()
        });
        let site = analyse(&scene);
        assert!(
            !site.rooms.is_empty(),
            "seed {seed}: analysis found no rooms at all"
        );
        let tracks = all_shots(&site, &ShotOptions::default());
        assert!(!tracks.is_empty(), "seed {seed}: no shots generated");
        for t in &tracks {
            let r = qa::check(&site, t, &limits);
            assert!(
                r.passed(),
                "seed {seed}: {}\n  {:#?}",
                r.summary(),
                r.failures
            );
            checked += 1;
        }
    }
    assert!(checked >= 30, "only {checked} tracks checked");
}

#[test]
fn walkthrough_visits_every_reachable_room() {
    for seed in 1..=5u64 {
        let scene = synthetic::building(&synthetic::Plan {
            seed,
            storeys: 1,
            ..Default::default()
        });
        let site = analyse(&scene);
        // No cap: the point of this test is completeness.
        let opt = ShotOptions {
            max_rooms: 0,
            ..Default::default()
        };
        let track = shots::walkthrough(&site, &opt);
        assert!(!track.keys.is_empty(), "seed {seed}: empty walkthrough");
        let report = qa::check(&site, &track, &QaLimits::default());

        let reachable: Vec<usize> = (0..site.rooms.len())
            .filter(|i| site.rooms[*i].interior && !site.unreachable.contains(i))
            .collect();
        let missed: Vec<&str> = reachable
            .iter()
            .filter(|i| !report.rooms_visited.contains(i))
            .map(|i| site.rooms[*i].name.as_str())
            .collect();
        assert!(
            missed.is_empty(),
            "seed {seed}: {} reachable rooms, missed {:?} (visited {:?})",
            reachable.len(),
            missed,
            report.rooms_visited.len()
        );
    }
}

#[test]
fn unreachable_rooms_are_reported_not_hidden() {
    // Brick up doorways until the analysis genuinely isolates something.
    let mut found = false;
    for seed in 1..=12u64 {
        let scene = synthetic::with_unreachable(seed, 2);
        let site = analyse(&scene);
        if site.unreachable.is_empty() {
            continue;
        }
        found = true;
        // Reported...
        for u in &site.unreachable {
            assert!(
                site.rooms[*u].interior,
                "seed {seed}: reported an exterior region as unreachable"
            );
        }
        // ...and genuinely not visited, rather than silently walked into.
        let opt = ShotOptions {
            max_rooms: 0,
            ..Default::default()
        };
        let track = shots::walkthrough(&site, &opt);
        let report = qa::check(&site, &track, &QaLimits::default());
        for u in &site.unreachable {
            assert!(
                !report.rooms_visited.contains(u),
                "seed {seed}: room {} is reported unreachable but the track went there",
                site.rooms[*u].name
            );
        }
        assert!(report.passed(), "seed {seed}: {}", report.summary());
        break;
    }
    assert!(
        found,
        "no seed produced an isolated room; the fixture can no longer test this"
    );
}

#[test]
fn rooms_are_named_from_zones_and_ranked() {
    let scene = synthetic::villa();
    let site = analyse(&scene);
    let named = site
        .rooms
        .iter()
        .filter(|r| r.name.starts_with("Room ") && r.name.contains('-'))
        .count();
    assert!(
        named > 0,
        "no room picked up its zone name; got {:?}",
        site.rooms.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    let rank = site.rooms_by_rank();
    assert!(!rank.is_empty());
    for w in rank.windows(2) {
        assert!(
            site.rooms[w[0]].score >= site.rooms[w[1]].score,
            "ranking is not sorted"
        );
    }
}

#[test]
fn analysis_finds_storeys_entrance_and_stairs() {
    let scene = synthetic::villa();
    let site = analyse(&scene);
    assert_eq!(site.storeys.len(), 2, "expected two habitable storeys");
    assert!(site.entrance.is_some(), "front door not found");
    assert!(!site.stairs.is_empty(), "stair link not found");
    assert!(!site.facades.is_empty(), "no facades ranked");
    assert!(
        site.facades[0].score >= site.facades[site.facades.len() - 1].score,
        "facades not ranked"
    );
}

#[test]
fn full_tour_is_one_continuous_track() {
    let scene = synthetic::villa();
    let site = analyse(&scene);
    let track = shots::full_tour(&site, &ShotOptions::default());
    assert!(track.keys.len() > 100, "full tour is suspiciously short");
    // Times strictly increase.
    for w in track.keys.windows(2) {
        assert!(
            w[1].t >= w[0].t,
            "track time went backwards: {} then {}",
            w[0].t,
            w[1].t
        );
    }
    // No teleports between consecutive keys.
    let max_jump = track
        .keys
        .windows(2)
        .map(|w| (w[1].pos - w[0].pos).length())
        .fold(0.0f32, f32::max);
    assert!(max_jump < 2.0, "track teleports {max_jump:.2} m between keys");
    let r = qa::check(&site, &track, &QaLimits::default());
    assert!(r.passed(), "{}\n{:#?}", r.summary(), r.failures);
    assert!(!track.notes.is_empty(), "full tour has no leg markers");
}

#[test]
fn tracks_are_constant_speed_and_eased() {
    let scene = synthetic::villa();
    let site = analyse(&scene);
    let track = shots::drone_reveal(&site, &ShotOptions::default());
    let n = track.keys.len();
    assert!(n > 30);
    let speed = |i: usize| -> f32 {
        (track.keys[i + 1].pos - track.keys[i].pos).length()
            / (track.keys[i + 1].t - track.keys[i].t).max(1e-6)
    };
    // Ends slow, middle fast: that is the ease.
    let start = speed(1);
    let mid = speed(n / 2);
    let end = speed(n - 3);
    assert!(
        start < mid * 0.8 && end < mid * 0.8,
        "not eased: {start:.2} / {mid:.2} / {end:.2}"
    );
    // The middle is close to constant speed.
    let a = speed(n * 4 / 10);
    let b = speed(n * 6 / 10);
    assert!(
        (a - b).abs() < mid * 0.35,
        "cruise is not constant: {a:.2} vs {b:.2}"
    );
}

#[test]
fn generation_is_fast_enough_to_feel_instant() {
    let scene = synthetic::villa();
    let t0 = std::time::Instant::now();
    let site = analyse(&scene);
    let tracks = all_shots(&site, &ShotOptions::default());
    let ms = t0.elapsed().as_secs_f32() * 1000.0;
    assert!(!tracks.is_empty());
    assert!(ms < 2000.0, "analysis + generation took {ms:.0} ms");
}

/// Two rooms, one 1 m gap in the partition, no typed Door. The gap must
/// become an opening and the watershed must keep the rooms apart.
#[test]
fn wall_gap_becomes_an_opening() {
    use makepad_math::vec3;
    let mut b = TourSceneBuilder::new("gap-house");
    b.storey("L0", 0.0, 3.0);
    b.element("concrete slab", TourClass::Slab, 0);
    b.box_solid(vec3(0.0, 0.0, -0.30), vec3(10.0, 6.0, 0.0));
    // Perimeter.
    b.element("exterior wall", TourClass::Wall, 0);
    b.box_solid(vec3(0.0, 0.0, 0.0), vec3(10.0, 0.20, 2.7));
    b.element("exterior wall", TourClass::Wall, 0);
    b.box_solid(vec3(0.0, 5.80, 0.0), vec3(10.0, 6.0, 2.7));
    b.element("exterior wall", TourClass::Wall, 0);
    b.box_solid(vec3(0.0, 0.0, 0.0), vec3(0.20, 6.0, 2.7));
    b.element("exterior wall", TourClass::Wall, 0);
    b.box_solid(vec3(9.80, 0.0, 0.0), vec3(10.0, 6.0, 2.7));
    // Partition with a 1.0 m door gap around y = 3.
    b.element("interior wall", TourClass::Wall, 0);
    b.box_solid(vec3(4.90, 0.0, 0.0), vec3(5.10, 2.50, 2.7));
    b.element("interior wall", TourClass::Wall, 0);
    b.box_solid(vec3(4.90, 3.50, 0.0), vec3(5.10, 6.0, 2.7));
    b.element("roof", TourClass::Roof, 0);
    b.box_solid(vec3(0.0, 0.0, 2.70), vec3(10.0, 6.0, 3.0));
    let scene = b.finish();
    let site = analyse(&scene);
    assert!(
        !site.openings.is_empty(),
        "expected a derived opening through the partition, got 0"
    );
    assert!(
        site.openings.iter().any(|o| (0.7..=1.5).contains(&o.width)),
        "openings had widths {:?}",
        site.openings.iter().map(|o| o.width).collect::<Vec<_>>()
    );
    let interior = site.rooms.iter().filter(|r| r.interior).count();
    assert!(
        interior >= 2,
        "expected two interior rooms across the opening, got {interior} (rooms {:?})",
        site.rooms
            .iter()
            .map(|r| (r.interior, r.area))
            .collect::<Vec<_>>()
    );
}
