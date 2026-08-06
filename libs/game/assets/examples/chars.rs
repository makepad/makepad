//! What rigged characters the library holds, and what the AI is told about them.
fn main() {
    use makepad_game_assets::agent;
    let idx = makepad_game_assets::AssetIndex::build(std::path::Path::new(
        "apps/arcade/resources",
    ));
    for c in idx.casts() {
        println!(
            "rig {:>2} joints — {} members, up to {} clips",
            c.joints, c.members.len(), c.max_clips
        );
        println!("  states: {}", c.shared_states.join(", "));
        println!("  members: {}", c.members.join(", "));
        if c.richest.len() < c.members.len() {
            println!("  richest ({} clips): {}", c.max_clips, c.richest.join(", "));
        }
        println!();
    }
    let json = agent::casts_to_json(&agent::execute_cast(&idx, None), 6);
    println!("find_cast JSON ({} bytes):\n{}", json.len(), json);
    println!();
    for q in ["a character that can attack", "a skeleton enemy", "someone who can run and jump"] {
        let hits = idx.find(q);
        let top: Vec<&str> = hits.iter().filter(|h| h.entry.rigged).take(3)
            .map(|h| h.entry.id.as_str()).collect();
        println!("{q:<34} -> {}", top.join(", "));
    }
}
