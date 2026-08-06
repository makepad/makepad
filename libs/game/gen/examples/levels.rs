//! Cost and placement counts for the kit composition generators.
//!
//! Run: cargo run -p makepad-game-gen --release --example levels

use makepad_game_gen::*;
use makepad_math::*;
use std::time::Instant;

fn road_kit() -> Kit {
    Kit::new(
        "kenney/city-kit-roads",
        4.0,
        vec![
            TileDef::new("road-end", TileRole::End, 0.1),
            TileDef::new("road-straight", TileRole::Straight, 0.1),
            TileDef::new("road-straight-barrier", TileRole::Straight, 0.6),
            TileDef::new("road-bend", TileRole::Corner, 0.1),
            TileDef::new("road-curve", TileRole::Corner, 0.1),
            TileDef::new("road-intersection", TileRole::TJunction, 0.1),
            TileDef::new("road-crossroad", TileRole::Cross, 0.1),
        ],
    )
}

fn dungeon_kit() -> Kit {
    Kit::new(
        "kenney/modular-dungeon-kit",
        4.0,
        vec![
            TileDef::new("corridor-end", TileRole::End, 3.0),
            TileDef::new("corridor", TileRole::Straight, 3.0),
            TileDef::new("corridor-corner", TileRole::Corner, 3.0),
            TileDef::new("corridor-junction", TileRole::TJunction, 3.0),
            TileDef::new("corridor-intersection", TileRole::Cross, 3.0),
            TileDef::new("template-floor-big", TileRole::Floor, 0.2),
            TileDef::new("template-floor-detail", TileRole::Floor, 0.2),
        ],
    )
}

fn building_kit() -> Kit {
    Kit::new(
        "kenney/city-kit-commercial",
        4.0,
        vec![
            TileDef::new("building-a", TileRole::Building, 8.0),
            TileDef::new("building-b", TileRole::Building, 10.0),
            TileDef::new("building-c", TileRole::Building, 12.0),
            TileDef::new("building-skyscraper-a", TileRole::Building, 20.0),
        ],
    )
}

fn prop_kit() -> Kit {
    Kit::new(
        "kenney/city-kit-props",
        4.0,
        vec![
            TileDef::new("light-square", TileRole::Prop, 4.0),
            TileDef::new("construction-cone", TileRole::Prop, 0.5),
        ],
    )
}

fn bench<T>(label: &str, runs: u32, f: impl Fn() -> T) -> T {
    // Warm once so the first allocation is not counted as generation cost.
    let mut out = f();
    let t = Instant::now();
    for _ in 0..runs {
        out = f();
    }
    let us = t.elapsed().as_secs_f64() * 1.0e6 / runs as f64;
    println!("{label:<38} {us:>9.1} us");
    out
}

fn main() {
    let rk = road_kit();
    let dk = dungeon_kit();
    let bk = building_kit();
    let pk = prop_kit();

    println!("--- generation cost (release, mean of N) ---");

    // A racing circuit from a closed spline.
    let circuit = Spline::new(
        vec![
            vec3f(-60.0, 0.0, -32.0),
            vec3f(60.0, 0.0, -32.0),
            vec3f(84.0, 0.0, 0.0),
            vec3f(60.0, 0.0, 32.0),
            vec3f(-60.0, 0.0, 32.0),
            vec3f(-84.0, 0.0, 0.0),
        ],
        true,
    );
    let track = bench("track from closed spline", 200, || {
        road_from_spline(&rk, &circuit, 16, 7)
    });

    // A city street network.
    let grid_paths: Vec<Vec<Vec3f>> = (0..6)
        .map(|i| {
            let z = -60.0 + i as f32 * 24.0;
            vec![vec3f(-72.0, 0.0, z), vec3f(72.0, 0.0, z)]
        })
        .chain((0..7).map(|i| {
            let x = -72.0 + i as f32 * 24.0;
            vec![vec3f(x, 0.0, -60.0), vec3f(x, 0.0, 60.0)]
        }))
        .collect();
    let net = bench("road network (13 paths)", 200, || {
        road_network(
            &rk,
            &RoadParams {
                seed: 3,
                paths: &grid_paths,
            },
        )
    });

    let small_town = bench("town 24x24 block 4", 100, || {
        town(
            &rk,
            &TownParams {
                seed: 1,
                extent: (24, 24),
                block: 4,
                density: 0.75,
                buildings: Some(&bk),
                props: Some(&pk),
            },
        )
    });

    let big_town = bench("town 60x60 block 5", 50, || {
        town(
            &rk,
            &TownParams {
                seed: 1,
                extent: (60, 60),
                block: 5,
                density: 0.7,
                buildings: Some(&bk),
                props: Some(&pk),
            },
        )
    });

    let dun = bench("dungeon 48x48 depth 5", 100, || {
        dungeon(
            &dk,
            &DungeonParams {
                seed: 2,
                extent: (48, 48),
                min_room: 5,
                depth: 5,
            },
        )
    });

    let big_dun = bench("dungeon 96x96 depth 6", 50, || {
        dungeon(
            &dk,
            &DungeonParams {
                seed: 2,
                extent: (96, 96),
                min_room: 6,
                depth: 6,
            },
        )
    });

    println!("\n--- placements (and layers = draw batches) ---");
    for (label, lvl) in [
        ("track (closed spline)", &track),
        ("road network", &net),
        ("town 24x24", &small_town),
        ("town 60x60", &big_town),
        ("dungeon 48x48", &dun),
        ("dungeon 96x96", &big_dun),
    ] {
        let per_layer: Vec<String> = lvl
            .layers
            .iter()
            .map(|l| format!("{}={}", short(&l.kit_id), l.placements.len()))
            .collect();
        println!(
            "{label:<24} {:>5} tiles  {} layer(s)  [{}]",
            lvl.placement_count(),
            lvl.layers.len(),
            per_layer.join(" ")
        );
    }

    println!("\n--- role histogram, town 60x60 roads ---");
    let roads_layer = big_town
        .layers
        .iter()
        .find(|l| l.kit_id == rk.id)
        .expect("road layer");
    let mut counts = [0usize; 5];
    for p in &roads_layer.placements {
        let idx = match rk.tiles[p.tile as usize].role {
            TileRole::End => 0,
            TileRole::Straight => 1,
            TileRole::Corner => 2,
            TileRole::TJunction => 3,
            _ => 4,
        };
        counts[idx] += 1;
    }
    println!(
        "end={} straight={} corner={} tee={} cross={}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    );

    // The invariant, checked on the biggest generated things rather than only
    // in unit tests.
    println!("\n--- adjacency check ---");
    for (label, lvl, kit) in [
        ("town 60x60", &big_town, &rk),
        ("dungeon 96x96", &big_dun, &dk),
    ] {
        let layer = lvl.layers.iter().find(|l| l.kit_id == kit.id).unwrap();
        let layout = TileLayout {
            kit_id: layer.kit_id.clone(),
            tile_size: layer.tile_size,
            placements: layer.placements.clone(),
            open_cells: vec![],
            entrance: None,
            exit: None,
        };
        let bad = adjacency_errors(&layout);
        println!("{label:<24} {} mismatched edges", bad.len());
    }
}

fn short(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}
