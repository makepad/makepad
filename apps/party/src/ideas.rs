//! Ideas for a guest's next redraw: the box's vision model looks at their
//! loop and suggests a few ways to redraw it, which the phone shows after its
//! own ready-made prompts (a tap sends one like a typed wish). A source is
//! looked at once, when its phone first asks (`GET ideas`), and only on a
//! box whose hub keeps the vision model pinned beside the image model, as
//! for the takes' instructions (`worker::prompt_for`): otherwise each look
//! would swap the image model out and back. The picture and the ideas stay
//! in memory, here and on the hub, and only their count is logged.

use crate::hub::{BoxHub, VISION_MODEL};
use crate::image;

/// How many ideas the vision model is asked for.
pub const IDEAS: usize = 6;
/// The longest idea that makes a chip: a longer line is the model talking,
/// not an idea.
const MAX_WORDS: usize = 10;
/// The side of the picture the vision model sees: enough to tell what is in
/// it, and its answer comes back in about a second.
const SIDE: usize = 512;

/// What the vision model is asked.
pub fn cue() -> String {
    format!(
        "This is a picture from a party. Look at what it shows, then suggest {IDEAS} different, playful ways to redraw this \
         very picture: each an art style, a world to put it in, or a mood, and each naming something you can see in it. \
         Write each as a short phrase of at most eight words, one per line, without numbers. Keep them kind, and name no \
         real brands, films, games, characters or people."
    )
}

/// The ideas in the vision model's `answer`: one a line, without list marks,
/// numbers or quotes; each once, kind and short, at most [`IDEAS`]. A line
/// that leads into a list ("Here are some:") is none.
pub fn parse(answer: &str) -> Vec<String> {
    let mut ideas: Vec<String> = Vec::new();
    for line in answer.lines().map(str::trim).filter(|line| !line.ends_with(':')) {
        let idea = unlisted(line).trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '*')).trim_end_matches(['.', '!', ',']).trim();
        let words = idea.split_whitespace().count();
        if words == 0 || words > MAX_WORDS || !crate::worker::kind_wish(idea) {
            continue;
        }
        let idea = idea.split_whitespace().collect::<Vec<_>>().join(" ");
        if ideas.iter().any(|seen| seen.eq_ignore_ascii_case(&idea)) {
            continue;
        }
        ideas.push(idea);
        if ideas.len() == IDEAS {
            break;
        }
    }
    ideas
}

/// `line` without its list mark: a bullet, or a number with `.`, `)` or `:`
/// (a number that starts the idea itself stays).
fn unlisted(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '*', '•']).trim_start();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
    match rest.strip_prefix(['.', ')', ':']) {
        Some(after) if rest.len() < line.len() => after.trim_start(),
        _ => line,
    }
}

/// Whether this box's hub can look at pictures for ideas without a model
/// swap: it keeps the vision model pinned.
pub fn available(hub: &BoxHub) -> bool {
    hub.pinned(VISION_MODEL)
}

/// The vision model's ideas for `picture` (a JPEG): a call to the hub, so
/// for a connection's own thread, never the screen's or the worker's.
pub fn ask(hub: &BoxHub, picture: &[u8], cancelled: &dyn Fn() -> bool) -> Result<Vec<String>, String> {
    let rgb = image::decode_jpeg(picture)?;
    let (w, h) = crate::wall_plan::fit_within(rgb.width, rgb.height, SIDE, SIDE);
    let small = image::encode_jpeg(&image::resize(&rgb, w, h), 86)?;
    let (answer, _) = hub.describe(VISION_MODEL, &small, &cue(), 120, cancelled)?;
    Ok(parse(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_makes_short_kind_ideas_each_once() {
        let answer = "Here are some ideas:\n1. A watercolour painting at dusk.\n2) \"Underwater, among glowing jellyfish\"\n- a watercolour painting at dusk\n\
                      * knights with a bloody sword\n\n• a cosy cabin in falling snow!\n3. this one goes on and on about everything in the picture and never stops talking at all\n\
                      4. paper   lanterns over a night market\n5. **1920s jazz club**";
        assert_eq!(
            parse(answer),
            vec!["A watercolour painting at dusk", "Underwater, among glowing jellyfish", "a cosy cabin in falling snow", "paper lanterns over a night market", "1920s jazz club"],
            "list marks, numbers, quotes and end marks go (a number in the idea stays); the lead-in, a repeat, an unkind line and a ramble are left out"
        );
        let many: String = (0..IDEAS + 3).map(|i| format!("idea number {i}\n")).collect();
        assert_eq!(parse(&many).len(), IDEAS, "the ideas asked for, no more");
    }
}
