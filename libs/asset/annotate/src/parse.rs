//! Parse the vision model's reply into a [`Record`].
//!
//! The reply shape is fixed by `prompt.txt`: ten `key: value` lines. Parsing
//! is deliberately lenient about everything except the vocabularies — small
//! models wrap replies in code fences, bold the keys, add a preamble, or drop
//! a line entirely, and none of that should cost an asset its annotation. It
//! is deliberately strict about vocabularies: an out-of-list category becomes
//! `None` rather than a new tag, so a rambling reply cannot invent facets that
//! nothing queries.

/// Closed vocabularies. A value outside its list is dropped, never coined.
pub const CATEGORIES: &[&str] = &[
    "building", "road", "wall", "roof", "floor", "fence", "prop", "vehicle", "tree", "plant",
    "rock", "water", "furniture", "character", "weapon", "food", "sign", "stairs", "light",
    "container", "terrain",
];
pub const ROLES: &[&str] =
    &["tile", "wall", "roof", "corner", "edge", "door", "window", "prop", "standalone"];
pub const CONNECTIONS: &[&str] = &["straight", "corner", "tee", "cross", "end", "none"];
pub const SIZES: &[&str] = &["1x1", "1x2", "2x2", "tall", "flat", "long"];
pub const STYLES: &[&str] = &[
    "low-poly", "cartoon", "realistic", "toy", "medieval", "modern", "sci-fi", "rustic",
];
/// Colour words kept as tags. Anything else still reaches the description
/// line, it just does not become a queryable facet.
pub const COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "blue", "purple", "pink", "brown", "grey", "black",
    "white", "beige", "tan", "gold", "silver", "teal", "cyan", "navy", "maroon", "olive",
];

/// One asset's parsed annotation facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub what: String,
    pub cat: Option<String>,
    pub role: Option<String>,
    pub conn: Option<String>,
    pub size: Option<String>,
    pub colors: Vec<String>,
    pub style: Vec<String>,
    pub desc: String,
}

impl Record {
    /// Whether enough came back to be worth publishing. A reply with no name
    /// and no category told us nothing and is treated as a failure, so the
    /// asset stays unannotated and a later run retries it.
    pub fn is_useful(&self) -> bool {
        !self.what.is_empty() || self.cat.is_some()
    }
}

fn strip_markup(line: &str) -> String {
    line.replace("**", "").replace('`', "").trim().to_string()
}

fn pick(value: &str, vocab: &[&str]) -> Option<String> {
    let v = value.trim().trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
    if v.is_empty() || v == "-" {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    if let Some(hit) = vocab.iter().find(|c| **c == lower) {
        return Some((*hit).to_string());
    }
    // Tolerate "low poly" for "low-poly" and "sci fi" for "sci-fi".
    let dashed = lower.replace(' ', "-");
    vocab.iter().find(|c| **c == dashed).map(|c| (*c).to_string())
}

fn pick_many(value: &str, vocab: &[&str], max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in value.split([',', '/', ';']) {
        // A phrase like "dark green" still yields "green".
        let hit = pick(part, vocab).or_else(|| {
            part.split_whitespace().rev().find_map(|w| pick(w, vocab))
        });
        if let Some(h) = hit {
            if !out.contains(&h) {
                out.push(h);
                if out.len() == max {
                    break;
                }
            }
        }
    }
    out
}

fn clamp_words(text: &str, max_words: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.is_empty() || (words.len() == 1 && words[0] == "-") {
        return String::new();
    }
    words[..words.len().min(max_words)].join(" ")
}

/// Parse a model reply. Unknown keys and stray prose are ignored; the first
/// occurrence of each known key wins, so a model that restates itself does not
/// overwrite its own better first answer.
pub fn parse_record(reply: &str) -> Record {
    let mut rec = Record::default();
    let mut seen: Vec<&str> = Vec::new();
    for raw in reply.lines() {
        let line = strip_markup(raw);
        let line = line.trim_start_matches(['-', '*', '#', ' ']).trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let key = key.trim_start_matches(['-', '*', ' ']).trim();
        let value = value.trim();
        if seen.iter().any(|k| *k == key) {
            continue;
        }
        match key {
            "what" => {
                rec.what = clamp_words(value, 3).to_ascii_lowercase();
                seen.push("what");
            }
            "cat" | "category" => {
                rec.cat = pick(value, CATEGORIES);
                seen.push("cat");
            }
            "role" => {
                rec.role = pick(value, ROLES);
                seen.push("role");
            }
            "conn" | "connection" | "connects" => {
                rec.conn = pick(value, CONNECTIONS);
                seen.push("conn");
            }
            "size" | "footprint" => {
                rec.size = pick(value, SIZES);
                seen.push("size");
            }
            "colors" | "colours" | "color" | "colour" => {
                rec.colors = pick_many(value, COLORS, 3);
                seen.push("colors");
            }
            "style" => {
                rec.style = pick_many(value, STYLES, 2);
                seen.push("style");
            }
            "desc" | "description" => {
                rec.desc = clamp_words(value, 10);
                seen.push("desc");
            }
            _ => {}
        }
    }
    rec
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: &str = "what: wooden cart\n\
        cat: vehicle\n\
        role: standalone\n\
        conn: none\n\
        size: 1x2\n\
        colors: brown, grey\n\
        style: low-poly, medieval\n\
        desc: open wooden cart with one large spoked wheel";

    #[test]
    fn parses_a_clean_reply() {
        let r = parse_record(CLEAN);
        assert_eq!(r.what, "wooden cart");
        assert_eq!(r.cat.as_deref(), Some("vehicle"));
        assert_eq!(r.role.as_deref(), Some("standalone"));
        assert_eq!(r.conn.as_deref(), Some("none"));
        assert_eq!(r.size.as_deref(), Some("1x2"));
        assert_eq!(r.colors, vec!["brown", "grey"]);
        assert_eq!(r.style, vec!["low-poly", "medieval"]);
        assert_eq!(r.desc, "open wooden cart with one large spoked wheel");
        assert!(r.is_useful());
    }

    #[test]
    fn survives_fences_bold_and_preamble() {
        let messy = "Sure! Here is the analysis:\n```\n\
            **what:** road tile\n\
            - **cat**: road\n\
            role: tile\n\
            conn: corner\n\
            size: 1x1\n\
            colors: dark grey / white\n\
            style: low poly\n\
            desc: grey asphalt corner with white lane markings\n```\nHope that helps!";
        let r = parse_record(messy);
        assert_eq!(r.what, "road tile");
        assert_eq!(r.cat.as_deref(), Some("road"));
        assert_eq!(r.conn.as_deref(), Some("corner"));
        // "dark grey" still yields the colour word, "low poly" the dashed style
        assert_eq!(r.colors, vec!["grey", "white"]);
        assert_eq!(r.style, vec!["low-poly"]);
    }

    #[test]
    fn refuses_to_coin_vocabulary() {
        let r = parse_record(
            "what: thing\ncat: spaceship-hangar\nrole: gizmo\nconn: diagonal\nstyle: baroque\ncolors: chartreuse",
        );
        assert_eq!(r.cat, None);
        assert_eq!(r.role, None);
        assert_eq!(r.conn, None);
        assert!(r.style.is_empty());
        assert!(r.colors.is_empty());
    }

    #[test]
    fn dashes_and_missing_lines_are_empty_not_errors() {
        let r = parse_record("what: -\ncat: prop\ndesc: -");
        assert_eq!(r.what, "");
        assert_eq!(r.desc, "");
        // a category alone is still worth publishing
        assert!(r.is_useful());
        assert!(!parse_record("I cannot tell what this is.").is_useful());
    }

    #[test]
    fn first_answer_wins_over_restatement() {
        let r = parse_record("what: house\ncat: building\nwhat: barn\ncat: prop");
        assert_eq!(r.what, "house");
        assert_eq!(r.cat.as_deref(), Some("building"));
    }

    #[test]
    fn clamps_runaway_fields() {
        let long = "desc: ".to_string() + &vec!["word"; 40].join(" ");
        assert_eq!(parse_record(&long).desc.split_whitespace().count(), 10);
        let name = "what: ".to_string() + &vec!["name"; 9].join(" ");
        assert_eq!(parse_record(&name).what.split_whitespace().count(), 3);
    }

}
