//! The prompt suite: what kids actually ask for, plus deliberately vague ones.
//!
//! Expectations are intentionally CRUDE. They answer "did it build the thing it
//! was asked for", not "is it fun" — a stricter rubric would just measure how
//! well the harness author guessed, and a looser one would pass an empty world.

/// What a generated game must contain to count as having done the job.
pub struct Expect {
    /// Every one of these verbs must appear.
    pub needs: &'static [&'static str],
    /// At least one verb from each group must appear. Groups let a prompt
    /// accept more than one reasonable reading ("a boat" is a mover OR a car).
    pub any_of: &'static [&'static [&'static str]],
    /// The world must end up with at least this many entities after eval.
    pub min_entities: usize,
}

pub struct Case {
    pub slug: &'static str,
    pub prompt: &'static str,
    pub expect: Expect,
    /// Needs the asset index's `find_model` tool to be meaningful.
    pub needs_assets: bool,
}

/// A game is playable if it has *something* to look at and a way to drive it.
const BASE: &[&[&str]] = &[&["on_tick", "race", "chase", "wander", "patrol"]];

pub const CASES: &[Case] = &[
    Case {
        slug: "racing",
        prompt: "make a racing game with cars going round a track",
        expect: Expect {
            needs: &["car"],
            any_of: &[&["checkpoint", "race"], &["camera", "character", "car"]],
            min_entities: 4,
        },
        needs_assets: false,
    },
    Case {
        slug: "dogfight",
        prompt: "make a dogfighting game with planes shooting each other",
        expect: Expect {
            needs: &["plane"],
            any_of: &[&["spawn", "on_touch"]],
            min_entities: 2,
        },
        needs_assets: false,
    },
    Case {
        slug: "explore",
        prompt: "a first person game where you walk around and explore a landscape",
        expect: Expect {
            needs: &["character"],
            any_of: &[&["terrain"], BASE[0]],
            min_entities: 2,
        },
        needs_assets: false,
    },
    Case {
        slug: "platformer",
        prompt: "a platformer where you jump between floating platforms",
        expect: Expect {
            needs: &["character"],
            any_of: &[&["box"]],
            min_entities: 5,
        },
        needs_assets: false,
    },
    Case {
        slug: "chase-animals",
        prompt: "a game where you chase animals around a field and catch them",
        expect: Expect {
            needs: &["character"],
            any_of: &[&["wander", "chase", "patrol"], &["mover", "character"]],
            min_entities: 4,
        },
        needs_assets: false,
    },
    Case {
        slug: "boat",
        prompt: "a game with a boat on water",
        expect: Expect {
            needs: &[],
            any_of: &[&["terrain", "box"], BASE[0]],
            min_entities: 2,
        },
        needs_assets: false,
    },
    Case {
        slug: "tower",
        prompt: "a game where you stack blocks to build a tower",
        expect: Expect {
            needs: &["box"],
            any_of: &[BASE[0]],
            min_entities: 4,
        },
        needs_assets: false,
    },
    Case {
        slug: "maze",
        prompt: "a maze game where you have to find the exit",
        expect: Expect {
            needs: &["box"],
            any_of: &[&["character", "mover"], BASE[0]],
            min_entities: 8,
        },
        needs_assets: false,
    },
    Case {
        slug: "shooting-gallery",
        prompt: "a shooting gallery where you shoot targets for points",
        expect: Expect {
            needs: &[],
            any_of: &[&["spawn"], &["on_touch"], &["score", "text"]],
            min_entities: 4,
        },
        needs_assets: false,
    },
    Case {
        slug: "football",
        prompt: "a football game where you kick a ball into a goal",
        expect: Expect {
            needs: &[],
            any_of: &[&["box"], &["character", "mover"]],
            min_entities: 3,
        },
        needs_assets: false,
    },
    Case {
        slug: "vague-fun",
        prompt: "make something fun",
        expect: Expect {
            needs: &[],
            any_of: &[BASE[0]],
            min_entities: 2,
        },
        needs_assets: false,
    },
    Case {
        slug: "vague-dinosaurs",
        prompt: "a game with dinosaurs",
        expect: Expect {
            needs: &[],
            any_of: &[&["mover", "character", "box"]],
            min_entities: 3,
        },
        needs_assets: false,
    },
    Case {
        slug: "assets-trucks",
        prompt: "make a game with trucks driving around",
        expect: Expect {
            needs: &[],
            any_of: &[&["car", "mover", "box"]],
            min_entities: 3,
        },
        needs_assets: true,
    },
    Case {
        slug: "assets-trees",
        prompt: "put some trees and rocks around a field to explore",
        expect: Expect {
            needs: &[],
            any_of: &[&["box", "mover", "character"]],
            min_entities: 5,
        },
        needs_assets: true,
    },
];
