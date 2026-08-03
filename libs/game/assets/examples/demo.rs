use makepad_game_assets::{agent, AssetIndex};
fn main() {
    let idx = AssetIndex::build(std::path::Path::new("apps/arcade/resources"));
    println!("index: {} entries ({} models, {} sounds, {} music)\n",
        idx.len(),
        idx.count_of(makepad_game_assets::AssetKind::Model),
        idx.count_of(makepad_game_assets::AssetKind::Sound),
        idx.count_of(makepad_game_assets::AssetKind::Music));
    println!("--- prompt summary ({} chars) ---\n{}\n", agent::library_summary(&idx).len(), agent::library_summary(&idx));
    for q in ["i want a lorry", "a big scary monster", "something to hide behind",
              "sound when you crash into a wall", "happy win music", "a digger like at the roadworks"] {
        let r = agent::execute(&idx, &agent::FindParams::new(q));
        let top: Vec<String> = r.iter().take(3).map(|x| format!("{} ({})", x.id, x.name)).collect();
        println!("{:38} -> {}", format!("\"{q}\""), if top.is_empty() { "(no hits)".into() } else { top.join("\n                                          ") });
    }
    println!("\n--- compact JSON handed to the model ---\n{}", agent::results_to_json(&agent::execute(&idx, &agent::FindParams::new("red truck"))));
}
