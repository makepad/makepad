//! Print what the Zelda-scale plan actually builds.
//!
//! Layout is pure, so the whole world can be inspected without a window —
//! which is the point of splitting `plan` from `realise`. Run with:
//!   cargo run -p makepad-arcade --example bigworld_probe --release

use makepad_arcade::bigworld::{self, Region};

fn main() {
    let root = std::path::Path::new("apps/arcade/resources");
    if !root.join("models/kenney").is_dir() {
        eprintln!("run apps/arcade/download_assets.sh first");
        return;
    }
    let t = std::time::Instant::now();
    let index = makepad_game_assets::AssetIndex::build(root);
    let index_ms = t.elapsed().as_millis();

    let plan = bigworld::plan(&index, 7);
    let s = &plan.stats;
    println!(
        "index {} entries in {index_ms} ms\nplan {} props ({} distinct models), {} tiles, {} npcs, {} pois, {} interactables in {} us",
        index.len(),
        s.props,
        s.distinct_models,
        s.tiles,
        s.npcs,
        plan.pois.len(),
        plan.interactables.len(),
        s.gen_us,
    );
    println!("\nper region:");
    for (r, n) in &s.per_region {
        println!("  {:<9} {n:>5}", r.name());
    }

    println!("\ncast:");
    let (cj, civ) = bigworld::civilian_cast(&index);
    let (hj, hero) = bigworld::hero_cast(&index);
    println!("  civilians {cj} joints, {} members", civ.len());
    println!("  heroes    {hj} joints, {} members", hero.len());

    println!("\nnpcs by region:");
    for r in [Region::Village, Region::Castle, Region::Dungeon] {
        let n: Vec<&str> = plan
            .npcs
            .iter()
            .filter(|n| n.region == r)
            .filter_map(|n| n.character.as_deref())
            .collect();
        println!("  {:<8} {}", r.name(), n.join(", "));
    }

    println!("\nsample models per region:");
    for (r, _) in &s.per_region {
        let mut ids: Vec<&str> = plan
            .placements
            .iter()
            .filter(|p| p.region == *r)
            .map(|p| p.model.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        println!("  {:<9} {}", r.name(), ids.iter().take(5).cloned().collect::<Vec<_>>().join(", "));
    }

    // Reachability: every region centre must be within a short walk of a road
    // tile, or a player cannot get there on foot.
    println!("\nreachability (nearest road tile to each region centre):");
    for (name, c) in [
        ("village", bigworld::VILLAGE),
        ("castle", bigworld::CASTLE),
        ("woods", bigworld::WOODS),
        ("dungeon", bigworld::DUNGEON),
        ("quarry", bigworld::QUARRY),
    ] {
        let best = plan
            .placements
            .iter()
            .filter(|p| p.region == Region::Roads)
            .map(|p| ((p.pos.x - c.x).powi(2) + (p.pos.z - c.y).powi(2)).sqrt())
            .fold(f32::INFINITY, f32::min);
        println!("  {name:<8} {best:.1} units");
    }
}
