//! The title block: measured text, fitted sizes, and two-line wrapping.
//!
//! Engraved title pages have conventional proportions — a dominant title, a
//! smaller subtitle under it, both centred inside the printable width — and a
//! title that runs off the page breaks all of them. So the block is *fitted*:
//! the text is measured, the size is reduced only as far as it must be, and a
//! title too long to shrink politely wraps onto a second line instead.
//!
//! # Units
//!
//! Everything is in staff spaces, page-local, y down, like the rest of the
//! engraver. A [`makepad_score_render::TextRun`]'s `size` is *not* its em: the
//! renderer hands `size` to Makepad's text stack as a **point** size, and that
//! stack converts points to layout pixels at 96/72. One unit of `size` is
//! therefore [`EM_PER_TEXT_SIZE`] staff spaces of drawn em, and a run measured
//! any other way is a third too narrow — which is exactly how a title that
//! "fit" on paper ran off both edges of the page.

use crate::document::PAGE_WIDTH_SP;
use crate::engrave::{MARGIN_LEFT, MARGIN_RIGHT};

/// Staff spaces of drawn em per unit of a text run's `size`.
///
/// The renderer treats `size` as points and Makepad lays text out in logical
/// pixels at 96 per inch against 72 points per inch.
pub(crate) const EM_PER_TEXT_SIZE: f64 = 96.0 / 72.0;

/// The width a page may print into.
pub(crate) const PRINTABLE_WIDTH_SP: f64 = PAGE_WIDTH_SP - MARGIN_LEFT - MARGIN_RIGHT;

/// Page y the title block starts at.
const BLOCK_TOP: f64 = 7.5;
/// Page y the title block must stay above, clear of the first system, which
/// the page planner starts at 28.
const BLOCK_BOTTOM: f64 = 27.0;

const TITLE_SIZE_MAX: f64 = 5.4;
/// Below this the title is wrapped rather than shrunk further: two readable
/// lines beat one small one.
const TITLE_SIZE_WRAP_AT: f64 = 4.2;
const TITLE_SIZE_MIN: f64 = 2.6;
const SUBTITLE_SIZE_MAX: f64 = 2.2;
const SUBTITLE_SIZE_MIN: f64 = 1.3;
/// The subtitle never grows past this fraction of the title: the title has to
/// stay the dominant line however far it was shrunk.
const SUBTITLE_OF_TITLE: f64 = 0.5;

/// Height of one line box as a fraction of the em: the shipped face's own
/// ascender and descender, as Makepad's layouter adjusts them
/// (`asc: -0.1 desc: 0.0` in every theme's `font_regular`).
const LINE_BOX: f64 = (1.025 - 0.1) + 0.275;
/// Line boxes of a wrapped title overlap slightly; a title set solid reads
/// better than one with a body-text leading.
const LINE_ADVANCE: f64 = LINE_BOX * 0.96;
/// Staff spaces between the title's line box and the subtitle's.
const TITLE_TO_SUBTITLE: f64 = 0.6;

/// One laid-out line of the block: what to draw, how big, and where its line
/// box starts. The x is always the page centre; the engraver centres the run.
///
/// `top` is the page y a [`makepad_score_render::TextRun`] takes: Makepad's
/// text layouter treats a run's origin as the **top** of the line box and puts
/// the baseline an ascender below it, which is why the old fixed title and
/// subtitle sat on top of each other.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TitleLine {
    pub(crate) text: String,
    pub(crate) size: f64,
    pub(crate) top: f64,
}

impl TitleLine {
    /// Page y the line box ends at.
    pub(crate) fn bottom(&self) -> f64 {
        self.top + self.size * EM_PER_TEXT_SIZE * LINE_BOX
    }

    #[cfg(test)]
    pub(crate) fn width(&self) -> f64 {
        text_width_sp(&self.text, self.size)
    }
}

/// The fitted title block of page one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TitleBlock {
    pub(crate) title: Vec<TitleLine>,
    pub(crate) subtitle: Option<TitleLine>,
}

impl TitleBlock {
    /// Page y the block ends at.
    pub(crate) fn bottom(&self) -> f64 {
        self.subtitle
            .as_ref()
            .or_else(|| self.title.last())
            .map_or(BLOCK_TOP, TitleLine::bottom)
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = &TitleLine> {
        self.title.iter().chain(self.subtitle.iter())
    }

    /// The widest line, which is what has to fit the printable width.
    #[cfg(test)]
    pub(crate) fn width(&self) -> f64 {
        self.lines().map(TitleLine::width).fold(0.0, f64::max)
    }
}

/// The height of one run's line box, in staff spaces. A run's page y is the
/// top of this box.
pub(crate) fn line_box_sp(size: f64) -> f64 {
    size * EM_PER_TEXT_SIZE * LINE_BOX
}

/// The drawn width of a run, in staff spaces.
pub(crate) fn text_width_sp(text: &str, size: f64) -> f64 {
    advance_em(text) * size * EM_PER_TEXT_SIZE
}

/// The advance width of a string in ems of the UI text face.
fn advance_em(text: &str) -> f64 {
    text.chars()
        .map(|character| {
            let index = character as u32;
            if (32..127).contains(&index) {
                f64::from(ASCII_ADVANCE_EM[(index - 32) as usize])
            } else {
                // CJK and the rest are wider than Latin; a generous stand-in
                // keeps the fit conservative rather than overflowing.
                FALLBACK_ADVANCE_EM
            }
        })
        .sum()
}

const FALLBACK_ADVANCE_EM: f64 = 0.9;

/// Advance widths of printable ASCII in `IBMPlexSans-Text.ttf`, the face every
/// Makepad theme binds to `font_regular` and therefore the face this app's
/// score text is drawn in. Generated from the font's `hmtx` table at 1000
/// units per em; `advance_table_matches_the_shipped_face` re-checks them
/// against the file whenever the development checkout has it.
#[rustfmt::skip]
const ASCII_ADVANCE_EM: [f32; 95] = [
    0.236, 0.290, 0.432, 0.698, 0.599, 0.938, 0.699, 0.247,
    0.338, 0.338, 0.479, 0.600, 0.278, 0.400, 0.278, 0.399,
    0.600, 0.600, 0.600, 0.600, 0.600, 0.600, 0.600, 0.600,
    0.600, 0.600, 0.298, 0.298, 0.600, 0.600, 0.600, 0.481,
    0.893, 0.649, 0.657, 0.627, 0.676, 0.588, 0.564, 0.699,
    0.710, 0.407, 0.519, 0.646, 0.507, 0.812, 0.710, 0.708,
    0.616, 0.708, 0.647, 0.588, 0.575, 0.682, 0.617, 0.907,
    0.624, 0.603, 0.586, 0.320, 0.399, 0.320, 0.600, 0.563,
    0.600, 0.541, 0.585, 0.505, 0.585, 0.552, 0.331, 0.533,
    0.573, 0.257, 0.257, 0.537, 0.278, 0.877, 0.573, 0.562,
    0.585, 0.585, 0.375, 0.490, 0.357, 0.574, 0.500, 0.782,
    0.518, 0.506, 0.474, 0.349, 0.332, 0.349, 0.600,
];

/// Lays out the title and subtitle of page one so that both fit the printable
/// width and neither reaches the first system.
///
/// The search is deliberately ordered: keep one line while the size stays
/// respectable, wrap to two before shrinking a long title into illegibility,
/// and only truncate a title no two lines at the minimum size could hold.
pub(crate) fn title_block(title: &str, subtitle: &str) -> TitleBlock {
    let title = collapse_whitespace(title);
    let subtitle = collapse_whitespace(subtitle);
    let width = PRINTABLE_WIDTH_SP;

    for size in sizes(TITLE_SIZE_MAX, TITLE_SIZE_WRAP_AT) {
        if text_width_sp(&title, size) <= width {
            if let Some(block) = stack(&[title.clone()], size, &subtitle, width) {
                return block;
            }
        }
    }
    for size in sizes(TITLE_SIZE_MAX, TITLE_SIZE_MIN) {
        let Some(lines) = wrap_two(&title, size, width) else {
            continue;
        };
        if let Some(block) = stack(&lines, size, &subtitle, width) {
            return block;
        }
    }
    // Nothing fits: draw at the minimum and truncate, so the page is still a
    // page rather than a title running into the margins.
    let size = TITLE_SIZE_MIN;
    let lines = wrap_two(&title, size, width).unwrap_or_else(|| {
        let mut halves = split_in_two(&title, size, width)
            .map(|(left, right)| vec![left, right])
            .unwrap_or_else(|| vec![title.clone()]);
        for line in &mut halves {
            *line = truncate_to(line, size, width);
        }
        halves
    });
    stack(&lines, size, &subtitle, width).unwrap_or_else(|| TitleBlock {
        title: vec![TitleLine {
            text: truncate_to(&title, size, width),
            size,
            top: BLOCK_TOP,
        }],
        subtitle: None,
    })
}

/// Candidate sizes, largest first, in tenths of a staff space.
fn sizes(from: f64, to: f64) -> impl Iterator<Item = f64> {
    let steps = (((from - to) / 0.1).round() as i32).max(0);
    (0..=steps).map(move |step| from - f64::from(step) * 0.1)
}

/// Stacks laid-out title lines and the subtitle into a block, or `None` when
/// the result would reach into the first system.
fn stack(lines: &[String], size: f64, subtitle: &str, width: f64) -> Option<TitleBlock> {
    let em = size * EM_PER_TEXT_SIZE;
    let title: Vec<TitleLine> = lines
        .iter()
        .enumerate()
        .map(|(index, text)| TitleLine {
            text: text.clone(),
            size,
            top: BLOCK_TOP + index as f64 * em * LINE_ADVANCE,
        })
        .collect();
    let title_bottom = title.last().map_or(BLOCK_TOP, TitleLine::bottom);

    let subtitle = fit_subtitle(subtitle, size, width).map(|subtitle_size| TitleLine {
        text: subtitle.to_string(),
        size: subtitle_size,
        top: title_bottom + TITLE_TO_SUBTITLE,
    });
    let block = TitleBlock { title, subtitle };
    (block.bottom() <= BLOCK_BOTTOM).then_some(block)
}

/// The largest subtitle size that fits the width and stays under the title.
fn fit_subtitle(subtitle: &str, title_size: f64, width: f64) -> Option<f64> {
    if subtitle.is_empty() {
        return None;
    }
    let ceiling = SUBTITLE_SIZE_MAX.min(title_size * SUBTITLE_OF_TITLE);
    if ceiling < SUBTITLE_SIZE_MIN {
        return Some(SUBTITLE_SIZE_MIN);
    }
    sizes(ceiling, SUBTITLE_SIZE_MIN)
        .find(|size| text_width_sp(subtitle, *size) <= width)
        .or(Some(SUBTITLE_SIZE_MIN))
}

/// The title as one line if it fits, else as two balanced lines if *they* fit,
/// else `None`.
fn wrap_two(title: &str, size: f64, width: f64) -> Option<Vec<String>> {
    if text_width_sp(title, size) <= width {
        return Some(vec![title.to_string()]);
    }
    split_in_two(title, size, width).map(|(left, right)| vec![left, right])
}

/// Splits at the word boundary that makes the two lines as even as possible,
/// provided both fit.
fn split_in_two(title: &str, size: f64, width: f64) -> Option<(String, String)> {
    let words: Vec<&str> = title.split(' ').filter(|word| !word.is_empty()).collect();
    if words.len() < 2 {
        return None;
    }
    let mut best: Option<(f64, usize)> = None;
    for split in 1..words.len() {
        let left = words[..split].join(" ");
        let right = words[split..].join(" ");
        let (left_width, right_width) = (
            text_width_sp(&left, size),
            text_width_sp(&right, size),
        );
        if left_width > width || right_width > width {
            continue;
        }
        let imbalance = (left_width - right_width).abs();
        if best.is_none_or(|(previous, _)| imbalance < previous) {
            best = Some((imbalance, split));
        }
    }
    let (_, split) = best?;
    Some((words[..split].join(" "), words[split..].join(" ")))
}

/// Shortens a line with an ellipsis until it fits. The last resort.
fn truncate_to(line: &str, size: f64, width: f64) -> String {
    if text_width_sp(line, size) <= width {
        return line.to_string();
    }
    let characters: Vec<char> = line.chars().collect();
    for keep in (1..characters.len()).rev() {
        let candidate: String = characters[..keep]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string()
            + "...";
        if text_width_sp(&candidate, size) <= width {
            return candidate;
        }
    }
    String::from("...")
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACH: &str =
        "Das wohltemperierte Klavier I - Praeludium und Fuge 1 in C-Dur BWV 846";

    /// The reported bug, as an invariant: whatever the title, every line of the
    /// block is inside the printable width and clear of the first system.
    #[test]
    fn every_title_length_fits_the_page() {
        let titles = [
            "",
            "Prelude",
            "Nocturne in Makepad",
            "Sonata quasi una Fantasia in C sharp minor",
            BACH,
            // A title with no spaces at all cannot be wrapped: it is truncated.
            "Wolfgangamadeusmozartklaviersonatennummerelfinadurkoechelverzeichnis331",
            // Absurd, and still not allowed off the page.
            &"Extremely Long Title ".repeat(20),
        ];
        for title in titles {
            let block = title_block(title, "for piano");
            assert!(
                block.width() <= PRINTABLE_WIDTH_SP + 1e-9,
                "{title:?} lays out {:.2} sp wide, past the {PRINTABLE_WIDTH_SP:.2} sp printable width",
                block.width()
            );
            assert!(
                block.bottom() <= BLOCK_BOTTOM + 1e-9,
                "{title:?} reaches page y {:.2}, into the first system",
                block.bottom()
            );
            assert!(block.title.len() <= 2, "a title block is at most two lines");
        }
    }

    #[test]
    fn a_short_title_keeps_the_full_size_on_one_line() {
        let block = title_block("Nocturne in Makepad", "for piano");
        assert_eq!(block.title.len(), 1);
        assert_eq!(block.title[0].size, TITLE_SIZE_MAX);
    }

    /// The Bach title is the one the user reported. It must wrap rather than
    /// shrink to nothing, and it must stay much bigger than its subtitle.
    #[test]
    fn a_long_title_wraps_instead_of_shrinking_away() {
        let block = title_block(BACH, "for piano");
        assert_eq!(block.title.len(), 2, "{:#?}", block.title);
        assert!(
            block.title[0].size >= TITLE_SIZE_WRAP_AT,
            "wrapped lines stay readable, got {}",
            block.title[0].size
        );
        let subtitle = block.subtitle.expect("the subtitle survives");
        assert!(
            block.title[0].size >= subtitle.size * 1.8,
            "the title must dominate: {} vs {}",
            block.title[0].size,
            subtitle.size
        );
        // Both halves carry real words, and together they are the whole title.
        assert_eq!(
            format!("{} {}", block.title[0].text, block.title[1].text),
            BACH
        );
    }

    #[test]
    fn a_medium_title_shrinks_before_it_wraps() {
        let title = "Sonata quasi una Fantasia in C sharp minor";
        let block = title_block(title, "for piano");
        assert_eq!(block.title.len(), 1);
        assert!(block.title[0].size < TITLE_SIZE_MAX);
        assert!(block.title[0].size >= TITLE_SIZE_WRAP_AT);
    }

    /// The reported title also had "for piano" sitting inside it. Lines
    /// descend the page and no line box starts before the one above it ends —
    /// bar the deliberate solid setting between two title lines.
    #[test]
    fn lines_descend_and_never_collide() {
        for title in ["Prelude", BACH] {
            let block = title_block(title, "for piano");
            let mut previous: Option<&TitleLine> = None;
            for line in block.lines() {
                if let Some(previous) = previous {
                    let overlap = previous.bottom() - line.top;
                    assert!(
                        overlap
                            <= previous.size * EM_PER_TEXT_SIZE * (LINE_BOX - LINE_ADVANCE) + 1e-9,
                        "line {:?} of {title:?} runs into the one above it",
                        line.text
                    );
                }
                previous = Some(line);
            }
            let subtitle = block.subtitle.as_ref().expect("the subtitle survives");
            let last = block.title.last().unwrap();
            assert!(
                subtitle.top >= last.bottom(),
                "the subtitle of {title:?} sits inside the title"
            );
        }
    }

    /// The renderer hands a run's `size` to Makepad as a point size, and
    /// Makepad lays points out at 96 logical pixels to the inch. Measuring in
    /// `size` rather than in ems is what let the title overflow by a third.
    #[test]
    fn a_run_draws_four_thirds_of_its_nominal_size() {
        assert!((EM_PER_TEXT_SIZE - 4.0 / 3.0).abs() < 1e-12);
        // The reported overflow, in numbers: the old estimate said the Bach
        // title was narrower than the page; measured, it is far wider.
        let old_estimate = BACH.chars().count() as f64 * TITLE_SIZE_MAX * 0.52;
        assert!(old_estimate < PRINTABLE_WIDTH_SP * 1.5);
        assert!(text_width_sp(BACH, TITLE_SIZE_MAX) > PRINTABLE_WIDTH_SP * 1.7);
    }

    /// The baked table is a copy of the shipped face's own metrics. Where the
    /// development checkout has the file, prove it still is.
    #[test]
    fn advance_table_matches_the_shipped_face() {
        let Some(bytes) = std::env::current_dir()
            .ok()
            .into_iter()
            .flat_map(|current| {
                current
                    .ancestors()
                    .take(6)
                    .map(|root| root.join("widgets/resources/IBMPlexSans-Text.ttf"))
                    .collect::<Vec<_>>()
            })
            .find_map(|path| std::fs::read(path).ok())
        else {
            return;
        };
        let face = ttf_parser::Face::parse(&bytes, 0).expect("the shipped face parses");
        let units = f64::from(face.units_per_em());
        for (index, expected) in ASCII_ADVANCE_EM.iter().enumerate() {
            let character = char::from_u32(index as u32 + 32).unwrap();
            let advance = face
                .glyph_index(character)
                .and_then(|glyph| face.glyph_hor_advance(glyph))
                .map(|advance| f64::from(advance) / units)
                .expect("every printable ASCII character is in the face");
            assert!(
                (advance - f64::from(*expected)).abs() < 5e-4,
                "{character:?} advances {advance}, table says {expected}"
            );
        }
    }
}
