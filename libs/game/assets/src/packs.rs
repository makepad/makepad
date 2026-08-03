//! Per-pack theme curation — the cheap half of making 4700 models findable.
//!
//! Hand-curating every model does not survive a catalogue this size, so the
//! work is split three ways:
//!
//! 1. **this file** — ~55 rows, one per pack, giving every model in that pack
//!    its setting and theme keywords for free. A model in `castle-kit` is
//!    findable as "medieval" without anyone writing a row for it.
//! 2. **filename tokens** (`lib.rs`) — Kenney's names are systematic
//!    (`tree_pineDefaultA`, `boat-sail-a`), so splitting on separators and
//!    camelCase yields real nouns at zero curation cost.
//! 3. **query-time synonyms** (`aliases.rs`) — one table that applies to the
//!    whole catalogue regardless of its size, which is why that is where
//!    curation effort pays best.
//!
//! Item-level rows in `aliases.rs` are then spent only on the few hundred
//! things people actually ask for by name.

/// Theme keywords contributed to every model in a pack, plus the pack's
/// display name. Keyed by directory name.
pub struct PackTheme {
    pub pack: &'static str,
    pub name: &'static str,
    pub themes: &'static [&'static str],
}

pub const PACK_THEMES: &[PackTheme] = &[
    // ---- starter kits (pinned from the KenneyNL GitHub repos) -----------
    PackTheme { pack: "arena", name: "Mini Arena", themes: &["arena", "battle", "roman", "medieval", "fight", "combat", "colosseum"] },
    PackTheme { pack: "city", name: "City Builder", themes: &["city", "town", "urban", "street", "modern", "builder"] },
    PackTheme { pack: "fps", name: "FPS Kit", themes: &["shooter", "fps", "sci-fi", "combat", "arena", "first person"] },
    PackTheme { pack: "platformer", name: "Platformer Starter", themes: &["platformer", "jumping", "level", "arcade", "side scroller"] },
    PackTheme { pack: "racing", name: "Racing Starter", themes: &["racing", "race", "speed", "track", "driving"] },
    // ---- full catalogue -------------------------------------------------
    PackTheme { pack: "3d-road-tiles", name: "3D Road Tiles", themes: &["road", "street", "tile", "modular", "driving", "city"] },
    PackTheme { pack: "blaster-kit", name: "Blaster Kit", themes: &["weapon", "blaster", "gun", "sci-fi", "shooter", "target", "shooting"] },
    PackTheme { pack: "blocky-characters", name: "Blocky Characters", themes: &["character", "person", "people", "blocky", "player", "avatar", "someone to play as"] },
    PackTheme { pack: "brick-kit", name: "Brick Kit", themes: &["brick", "toy", "plastic", "building block", "lego", "construction", "build"] },
    PackTheme { pack: "building-kit", name: "Building Kit", themes: &["building", "house", "structure", "modular", "architecture"] },
    PackTheme { pack: "car-kit", name: "Car Kit", themes: &["car", "vehicle", "driving", "transport", "road", "something to drive"] },
    PackTheme { pack: "castle-kit", name: "Castle Kit", themes: &["castle", "medieval", "fantasy", "fortress", "knight", "king", "kingdom"] },
    PackTheme { pack: "city-kit-commercial", name: "City Kit: Commercial", themes: &["city", "building", "skyscraper", "commercial", "shop", "urban", "downtown", "town"] },
    PackTheme { pack: "city-kit-industrial", name: "City Kit: Industrial", themes: &["city", "factory", "warehouse", "industrial", "urban", "works"] },
    PackTheme { pack: "city-kit-roads", name: "City Kit: Roads", themes: &["road", "street", "city", "town", "driving", "traffic", "something to drive on"] },
    PackTheme { pack: "city-kit-suburban", name: "City Kit: Suburban", themes: &["city", "suburban", "house", "home", "neighbourhood", "neighborhood", "town", "somewhere to live"] },
    PackTheme { pack: "coaster-kit", name: "Coaster Kit", themes: &["rollercoaster", "coaster", "theme park", "ride", "fairground", "attraction", "amusement"] },
    PackTheme { pack: "cube-pets", name: "Cube Pets", themes: &["animal", "pet", "cute", "creature", "cube", "blocky"] },
    PackTheme { pack: "factory-kit", name: "Factory Kit", themes: &["factory", "industrial", "conveyor", "warehouse", "machine", "production", "belt"] },
    PackTheme { pack: "fantasy-town-kit", name: "Fantasy Town Kit", themes: &["fantasy", "medieval", "town", "village", "building", "rpg"] },
    PackTheme { pack: "food-kit", name: "Food Kit", themes: &["food", "eat", "kitchen", "cooking", "meal", "snack", "something to eat"] },
    PackTheme { pack: "furniture-kit", name: "Furniture Kit", themes: &["furniture", "interior", "house", "home", "room", "indoor", "decor"] },
    PackTheme { pack: "graveyard-kit", name: "Graveyard Kit", themes: &["graveyard", "halloween", "spooky", "horror", "scary", "monster", "creepy", "ghost"] },
    PackTheme { pack: "hexagon-kit", name: "Hexagon Kit", themes: &["hexagon", "hex", "tile", "terrain", "strategy", "board", "modular"] },
    PackTheme { pack: "holiday-kit", name: "Holiday Kit", themes: &["christmas", "holiday", "winter", "snow", "festive", "xmas", "cabin"] },
    PackTheme { pack: "marble-kit", name: "Marble Kit", themes: &["marble", "track", "ball", "run", "puzzle", "rolling"] },
    PackTheme { pack: "mini-arcade", name: "Mini Arcade", themes: &["arcade", "game", "machine", "retro", "play", "cabinet"] },
    PackTheme { pack: "mini-arena", name: "Mini Arena", themes: &["arena", "battle", "roman", "fight", "combat", "colosseum"] },
    PackTheme { pack: "mini-characters", name: "Mini Characters", themes: &["character", "person", "people", "player", "avatar", "someone to play as"] },
    PackTheme { pack: "mini-dungeon", name: "Mini Dungeon", themes: &["dungeon", "rpg", "roguelike", "medieval", "cave", "adventure", "crawl"] },
    PackTheme { pack: "mini-forest", name: "Mini Forest", themes: &["forest", "nature", "woods", "camp", "outdoors", "archer", "tent"] },
    PackTheme { pack: "mini-market", name: "Mini Market", themes: &["market", "shop", "store", "supermarket", "shopping", "grocery"] },
    PackTheme { pack: "mini-skate", name: "Mini Skate", themes: &["skate", "skateboard", "park", "ramp", "street", "trick"] },
    PackTheme { pack: "minigolf-kit", name: "Minigolf Kit", themes: &["golf", "minigolf", "course", "putting", "level", "sport"] },
    PackTheme { pack: "modular-buildings", name: "Modular Buildings", themes: &["building", "modular", "house", "city", "town", "architecture"] },
    PackTheme { pack: "modular-cave-kit", name: "Modular Cave Kit", themes: &["cave", "underground", "modular", "tunnel", "rock", "dungeon"] },
    PackTheme { pack: "modular-dungeon-kit", name: "Modular Dungeon Kit", themes: &["dungeon", "modular", "underground", "rpg", "medieval", "tunnel"] },
    PackTheme { pack: "modular-space-kit", name: "Modular Space Kit", themes: &["space", "sci-fi", "station", "modular", "future", "spaceship", "corridor"] },
    PackTheme { pack: "nature-kit", name: "Nature Kit", themes: &["nature", "outdoors", "forest", "tree", "plant", "scenery", "landscape", "countryside"] },
    PackTheme { pack: "pirate-kit", name: "Pirate Kit", themes: &["pirate", "ship", "boat", "island", "sea", "ocean", "treasure", "sailing"] },
    PackTheme { pack: "platformer-kit", name: "Platformer Kit", themes: &["platformer", "level", "jumping", "arcade", "obstacle"] },
    PackTheme { pack: "prototype-kit", name: "Prototype Kit", themes: &["prototype", "placeholder", "blockout", "greybox", "test", "simple"] },
    PackTheme { pack: "racing-kit", name: "Racing Kit", themes: &["racing", "race", "track", "car", "speed", "driving", "circuit"] },
    PackTheme { pack: "retro-fantasy-kit", name: "Retro Fantasy Kit", themes: &["retro", "fantasy", "medieval", "town", "castle", "pixel", "old school"] },
    PackTheme { pack: "retro-urban-kit", name: "Retro Urban Kit", themes: &["retro", "urban", "city", "street", "old school", "town"] },
    PackTheme { pack: "space-kit", name: "Space Kit", themes: &["space", "sci-fi", "future", "planet", "rocket", "spaceship", "astronaut", "alien"] },
    PackTheme { pack: "space-station-kit", name: "Space Station Kit", themes: &["space", "station", "sci-fi", "interior", "future", "corridor", "spaceship"] },
    PackTheme { pack: "survival-kit", name: "Survival Kit", themes: &["survival", "nature", "camp", "outdoors", "wilderness", "craft"] },
    PackTheme { pack: "tower-defense-kit", name: "Tower Defense Kit", themes: &["tower defense", "defense", "castle", "medieval", "strategy", "tower"] },
    PackTheme { pack: "toy-car-kit", name: "Toy Car Kit", themes: &["toy", "car", "vehicle", "track", "play", "cute", "something to drive"] },
    PackTheme { pack: "train-kit", name: "Train Kit", themes: &["train", "railway", "railroad", "rail", "track", "locomotive", "tram"] },
    PackTheme { pack: "watercraft-kit", name: "Watercraft Kit", themes: &["boat", "ship", "water", "sea", "sailing", "vehicle", "ocean", "river"] },
];

pub fn theme_of(pack: &str) -> Option<&'static PackTheme> {
    PACK_THEMES.iter().find(|p| p.pack == pack)
}

/// Fallback category for a pack, so the 4400 uncurated models still land
/// somewhere sensible in the category tree (and so the prompt summary counts
/// mean something). Item-level curation overrides this when present.
pub fn default_category(pack: &str) -> Option<&'static str> {
    Some(match pack {
        "car-kit" | "toy-car-kit" | "racing-kit" | "racing" => "vehicle/ground",
        "watercraft-kit" | "pirate-kit" => "vehicle/water",
        "train-kit" => "vehicle/rail",
        "blocky-characters" | "mini-characters" => "character/player",
        "cube-pets" => "character/animal",
        "nature-kit" | "mini-forest" | "survival-kit" => "nature/plant",
        "food-kit" => "prop/food",
        "furniture-kit" => "prop/furniture",
        "blaster-kit" => "prop/weapon",
        "castle-kit" | "fantasy-town-kit" | "retro-fantasy-kit" | "tower-defense-kit" => {
            "building/medieval"
        }
        "city-kit-commercial" | "city-kit-industrial" | "city-kit-suburban" | "modular-buildings"
        | "building-kit" | "retro-urban-kit" | "mini-market" => "building/urban",
        "city-kit-roads" | "3d-road-tiles" => "road/street",
        "space-kit" | "space-station-kit" | "modular-space-kit" => "sci-fi/space",
        "mini-dungeon" | "modular-dungeon-kit" | "modular-cave-kit" => "building/dungeon",
        "graveyard-kit" | "holiday-kit" => "prop/decoration",
        "platformer-kit" | "platformer" | "marble-kit" | "minigolf-kit" | "coaster-kit"
        | "mini-skate" | "hexagon-kit" | "brick-kit" | "prototype-kit" | "factory-kit"
        | "mini-arcade" | "mini-arena" | "arena" | "city" | "fps" => "terrain/platform",
        _ => return None,
    })
}

/// Tokens that carry no meaning in a Kenney filename: variant markers and
/// filler. Stripped so `tree_pineDefaultA` indexes as "tree pine", not as a
/// thing called "default a".
pub fn is_noise_token(t: &str) -> bool {
    if t.len() == 1 {
        return true; // trailing A/B/C variant letters
    }
    matches!(
        t,
        "default" | "type" | "variant" | "alt" | "version" | "new" | "old2" | "obj" | "mesh"
    ) || (t.starts_with("type") && t[4..].chars().all(|c| c.is_ascii_digit()))
        || t.chars().all(|c| c.is_ascii_digit())
}
