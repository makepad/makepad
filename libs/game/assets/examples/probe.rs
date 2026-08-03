//! Ad-hoc query probe: prints the top hits and their kind for a set of
//! queries, so a ranking change can be judged against the queries it might
//! regress rather than only the ones it fixes.

use makepad_game_assets::AssetIndex;

fn main() {
    let idx = AssetIndex::build(std::path::Path::new("apps/arcade/resources"));
    let queries: Vec<String> = std::env::args().skip(1).collect();
    let queries: Vec<&str> = if queries.is_empty() {
        vec![
            "spaceship",
            "glass smashing",
            "metal clang",
            "laser gun",
            "explosion",
            "footsteps on wood",
            "coins",
            "somewhere for my guy to stand",
            "spaceship engine",
            "a boat",
            "police car",
        ]
    } else {
        queries.iter().map(|s| s.as_str()).collect()
    };
    for q in queries {
        println!("\"{q}\"");
        for h in idx.find(q).iter().take(4) {
            println!(
                "    {:6} {:5} {}",
                h.score,
                format!("{:?}", h.entry.kind),
                h.entry.id
            );
        }
    }
}
