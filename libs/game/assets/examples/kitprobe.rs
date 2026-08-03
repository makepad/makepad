fn main() {
    let root = std::path::PathBuf::from("apps/arcade/resources");
    let idx = makepad_game_assets::AssetIndex::build(&root);
    let kits = idx.kits();
    println!("{} kits, {} entries total", kits.len(), idx.len());
    println!("{:<24} {:>6} {:>8}  roles", "kit", "tiles", "size");
    for k in &kits {
        let roles: Vec<String> = k.roles.iter().map(|(r, n)| format!("{r}:{n}")).collect();
        let unclassified = k.tiles - k.roles.iter().map(|(_, n)| n).sum::<u32>();
        println!("{:<24} {:>6} {:>8}  {} (unclassified {})",
            k.pack, k.tiles,
            k.tile_size.map(|s| format!("{s:.2}")).unwrap_or("-".into()),
            roles.join(" "), unclassified);
    }
}
