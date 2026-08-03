use makepad_game_gen::*;
use makepad_game_script::compose::{kit_from_index, level_placements, KitUse};

fn main() {
    let idx = makepad_game_assets::AssetIndex::build(&std::path::PathBuf::from("apps/arcade/resources"));
    println!("library: {} entries", idx.len());

    let roads = kit_from_index(&idx, "city-kit-roads", KitUse::Structure).unwrap();
    let houses = kit_from_index(&idx, "city-kit-suburban", KitUse::Buildings).unwrap();
    println!(
        "roads {} tiles, houses {} tiles ({} classed Building)",
        roads.tiles.len(),
        houses.tiles.len(),
        houses.by_role(TileRole::Building).len()
    );

    let t0 = std::time::Instant::now();
    let level = town(&roads, &TownParams {
        seed: 11, extent: (18, 18), block: 6, density: 0.7,
        buildings: Some(&houses), props: None,
    });
    let us = t0.elapsed().as_micros();
    let kits = [roads.clone(), houses.clone()];
    let placed = level_placements(&level, &kits);
    let buildings = placed.iter().filter(|p| p.model.contains("suburban")).count();
    println!("town: {} tiles in {us}us — {} buildings, {} road", placed.len(), buildings, placed.len() - buildings);

    let d = kit_from_index(&idx, "modular-dungeon-kit", KitUse::Structure).unwrap();
    let dl = dungeon(&d, &DungeonParams { seed: 11, extent: (20, 20), min_room: 5, depth: 4 });
    let dp = level_placements(&dl, std::slice::from_ref(&d));
    println!("dungeon: {} tiles, entrance {:?}", dp.len(), dl.entrance.is_some());
    println!("adjacency errors: {}", level.layers.iter().map(|l| {
        adjacency_errors(&TileLayout { kit_id: l.kit_id.clone(), tile_size: l.tile_size,
            placements: l.placements.clone(), open_cells: vec![], entrance: None, exit: None }).len()
    }).sum::<usize>());
}
