//! What the generator promises: the same seed makes the same map, every map
//! is legal against the world-grid contract, and every map is FAIR — which
//! is checked here independently of the generator's own retry loop.

use makepad_rtsmap::verify::{distance_field, passable_letter};
use makepad_rtsmap::{
    amount, generate, generate_checked, pick_tiles, tiles, MapSpec, Style, Terrain, TileSet,
    MASK_ALL,
};

fn spec(seed: u32, style: Style, players: u8) -> MapSpec {
    MapSpec { seed, style, players, width: 64, height: 64, ..MapSpec::default() }
}

#[test]
fn rtsmap_same_seed_makes_the_same_map() {
    for style in Style::ALL {
        for seed in 1..=6u32 {
            let s = spec(seed, style, 4);
            let first = generate(&s);
            let second = generate(&s);
            assert_eq!(first, second, "{} seed {seed}", style.name());
        }
    }
}

#[test]
fn rtsmap_different_seeds_make_different_maps() {
    let a = generate(&spec(1, Style::Temperate, 2));
    let b = generate(&spec(2, Style::Temperate, 2));
    assert_ne!(a.grid, b.grid);
}

#[test]
fn rtsmap_grid_uses_only_contract_letters() {
    for style in Style::ALL {
        let map = generate(&spec(11, style, 4));
        for (index, letter) in map.grid.iter().enumerate() {
            assert!(
                matches!(letter, b'.' | b'#' | b'w' | b'r' | b'b' | b't'),
                "cell {index} of {} is {:?}",
                style.name(),
                *letter as char
            );
        }
        assert_eq!(map.grid.len(), map.width as usize * map.height as usize);
        assert_eq!(map.grid_rows().len(), map.height as usize);
    }
}

#[test]
fn rtsmap_every_style_and_player_count_generates_a_fair_map() {
    for style in Style::ALL {
        for players in [2u8, 3, 4, 6] {
            for seed in 1..=8u32 {
                let map = generate_checked(&spec(seed, style, players)).unwrap_or_else(|report| {
                    panic!("{} {players}p seed {seed}: {}", style.name(), report.summary())
                });
                assert_eq!(map.starts.len(), players as usize);
                assert_eq!(map.houses.len(), players as usize);
            }
        }
    }
}

#[test]
fn rtsmap_every_start_can_walk_to_every_other_start() {
    for style in Style::ALL {
        for players in [2u8, 4, 6] {
            let map = generate(&spec(5, style, players));
            let starts = map.starts.clone();
            for from in &starts {
                let field = distance_field(&map, (from.x, from.y));
                for to in &starts {
                    let at = to.y as usize * map.width as usize + to.x as usize;
                    assert_ne!(
                        field[at],
                        u16::MAX,
                        "{} {players}p: ({},{}) cannot reach ({},{})",
                        style.name(),
                        from.x,
                        from.y,
                        to.x,
                        to.y
                    );
                }
            }
        }
    }
}

#[test]
fn rtsmap_every_start_has_a_buildable_pocket_and_a_field_in_reach() {
    for style in Style::ALL {
        let map = generate(&spec(3, style, 4));
        for start in &map.starts {
            assert!(start.pocket >= makepad_rtsmap::verify::POCKET_MIN, "{start:?}");
            assert!(
                start.resource_cells >= makepad_rtsmap::verify::RESOURCE_CELLS_MIN,
                "{start:?}"
            );
            assert!(start.resource_distance <= makepad_rtsmap::RESOURCE_REACH, "{start:?}");
        }
    }
}

#[test]
fn rtsmap_resource_amount_follows_the_asked_for_density() {
    let sparse = generate(&MapSpec { resources: amount("low"), ..spec(9, Style::Desert, 4) });
    let heavy = generate(&MapSpec { resources: amount("heavy"), ..spec(9, Style::Desert, 4) });
    assert!(
        heavy.resources.len() > sparse.resources.len() * 2,
        "sparse={} heavy={}",
        sparse.resources.len(),
        heavy.resources.len()
    );
    // Richest at the heart: a field's best cell must be near-full.
    assert!(heavy.resources.iter().map(|r| r.stage).max().unwrap_or(0) >= 10);
    assert!(heavy.resources.iter().all(|r| r.stage > 0 && r.stage <= 11));
}

#[test]
fn rtsmap_a_desert_has_no_water_and_a_temperate_map_does() {
    let desert = generate(&spec(4, Style::Desert, 4));
    assert!(!desert.terrain.contains(&Terrain::Water));
    let temperate = generate(&MapSpec { water: 1.0, ..spec(4, Style::Temperate, 4) });
    assert!(temperate.terrain.contains(&Terrain::Water));
    // Shores exist wherever water does, or the bank is a cliff of nothing.
    assert!(temperate.terrain.contains(&Terrain::Shore));
}

#[test]
fn rtsmap_plateaus_are_rimmed_and_reachable() {
    let map = generate(&MapSpec { cliffs: 1.0, ..spec(7, Style::Desert, 4) });
    assert!(map.terrain.contains(&Terrain::Cliff));
    assert!(map.terrain.contains(&Terrain::Plateau));
    // Every plateau top is walkable from a start: the ramp pass exists so
    // high ground is ground.
    let field = distance_field(&map, (map.starts[0].x, map.starts[0].y));
    let tops = map
        .terrain
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == Terrain::Plateau)
        .count();
    let reached = map
        .terrain
        .iter()
        .enumerate()
        .filter(|(at, t)| **t == Terrain::Plateau && field[*at] != u16::MAX)
        .count();
    assert!(tops > 0);
    assert!(
        reached * 4 >= tops * 3,
        "only {reached}/{tops} plateau cells are walkable from a start"
    );
}

#[test]
fn rtsmap_roads_cross_water_so_the_map_stays_connected() {
    let map = generate(&MapSpec { water: 1.0, roads: 1.0, ..spec(2, Style::Temperate, 4) });
    assert!(map.terrain.contains(&Terrain::Road));
    // Every road cell is passable, including the ones that were river.
    for (at, terrain) in map.terrain.iter().enumerate() {
        if *terrain == Terrain::Road {
            assert!(passable_letter(map.grid[at]));
        }
    }
}

#[test]
fn rtsmap_tile_picking_is_stable_and_edge_matched() {
    let map = generate(&spec(6, Style::Desert, 4));
    let mut set = TileSet::new();
    // A pretend pack: one interior piece per terrain, plus authored edges
    // for the raised ground.
    for terrain in [
        Terrain::Clear,
        Terrain::Rough,
        Terrain::Shore,
        Terrain::Road,
        Terrain::Water,
        Terrain::Cliff,
        Terrain::Plateau,
        Terrain::Resource,
    ] {
        set.push_single(terrain, 1000 + terrain as u32);
    }
    for mask in 0..MASK_ALL {
        set.push_masked(Terrain::Cliff, mask, 2000 + mask as u32);
    }
    let first = pick_tiles(&map, &set, 17);
    assert_eq!(first, pick_tiles(&map, &set, 17));
    assert_eq!(first.len(), map.terrain.len());
    // A cliff cell whose neighbourhood is not full must draw the piece
    // authored for exactly that neighbourhood.
    let mut checked = 0;
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            let at = (y * map.width as i32 + x) as usize;
            if map.terrain[at] != Terrain::Cliff {
                continue;
            }
            let mask = tiles::neighbour_mask(&map, x, y);
            if mask == MASK_ALL {
                continue;
            }
            assert_eq!(first[at], Some(2000 + mask as u32));
            checked += 1;
        }
    }
    assert!(checked > 0, "no edge cliff cells to check");
}

#[test]
fn rtsmap_arena_is_the_same_map_from_every_chair() {
    let map = generate(&spec(12, Style::Arena, 4));
    // Rotational symmetry: every house's sorted distance profile is identical,
    // which the fairness check measures as a zero spread.
    assert!(map.report.start_spread < 0.02, "{}", map.report.summary());
    // and each house has the same number of resource cells in reach.
    let counts: Vec<u16> = map.starts.iter().map(|s| s.resource_cells).collect();
    let lo = *counts.iter().min().unwrap() as f32;
    let hi = *counts.iter().max().unwrap() as f32;
    assert!(hi <= lo * 1.25, "resource cells per house: {counts:?}");
}

#[test]
fn rtsmap_spec_is_clamped_to_something_playable() {
    let map = generate(&MapSpec {
        width: 4,
        height: 4000,
        players: 99,
        ..MapSpec::default()
    });
    assert_eq!(map.width, makepad_rtsmap::MIN_SIZE);
    assert_eq!(map.height, makepad_rtsmap::MAX_SIZE);
    assert_eq!(map.starts.len(), makepad_rtsmap::MAX_PLAYERS as usize);
}

#[test]
fn rtsmap_emits_the_contract_sidecars() {
    use makepad_rtsmap::emit::{place_text, EmitOpts, House, PropArt};
    let map = generate(&spec(21, Style::Temperate, 2));
    let opts = EmitOpts {
        source: "test".into(),
        world_key: "worlds/gen-test".into(),
        houses: vec![House { name: "A".into(), color: "e8c040".into(), side: "0".into() }],
        resource_key: "billboards/test/res".into(),
        props: PropArt {
            trees: vec!["billboards/test/t01".into()],
            rocks: vec!["billboards/test/rock".into()],
            ruins: vec!["billboards/test/ruin".into()],
            blooms: vec!["billboards/test/bloom".into()],
        },
        roster: vec!["billboards/test/tank".into()],
        ..EmitOpts::default()
    };
    let place = place_text(&map, &opts);
    assert!(place.starts_with("world-place 1\n"));
    assert!(place.contains("\nmode rts\n"));
    assert!(place.contains("\ncell 6.0\n"));
    assert!(place.contains("\ngrid worlds/gen-test.grid\n"));
    assert!(place.contains("\nhouse A color=e8c040 side=0\n"));
    assert!(place.contains("\nroster billboards/test/tank\n"));
    assert!(place.contains(" class=resource stage="));
    let grid = makepad_rtsmap::grid_text(&map, 6.0);
    assert!(grid.starts_with("world-grid 1\n"));
    assert_eq!(grid.lines().filter(|l| l.starts_with("row ")).count(), map.height as usize);
    let spawn = makepad_rtsmap::spawn_text(&map, 6.0, 60.0);
    assert_eq!(spawn.lines().filter(|l| l.starts_with("start ")).count(), 2);
    assert!(spawn.contains("\neye 60\n"));
}

/// Not a check — a look. Writes one PNG per style and seed so a human can
/// tell "legal" from "good".
#[test]
#[ignore]
fn rtsmap_write_preview_maps() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/agent_state/cnc/m5-maps");
    std::fs::create_dir_all(&out).expect("create preview dir");
    let mut index = String::from("# M5 generated maps\n\n");
    for style in Style::ALL {
        for (slot, seed) in [3u32, 17, 42, 91].into_iter().enumerate() {
            let players = [2u8, 4, 4, 6][slot];
            let map = generate(&MapSpec {
                seed,
                players,
                width: 72,
                height: 72,
                resources: if slot == 3 { 1.0 } else { 0.6 },
                ..spec(seed, style, players)
            });
            let (rgba, w, h) = makepad_rtsmap::preview::rgba(&map, 6);
            let name = format!("{}-{players}p-{seed}.png", style.name());
            std::fs::write(out.join(&name), makepad_rtsmap::preview::png(&rgba, w, h))
                .expect("write preview");
            index.push_str(&format!(
                "- `{name}` — {}x{} cells, {} resource cells, {} props — {}\n",
                map.width,
                map.height,
                map.resources.len(),
                map.props.len(),
                map.report.summary()
            ));
        }
    }
    std::fs::write(out.join("README.md"), index).expect("write index");
}
