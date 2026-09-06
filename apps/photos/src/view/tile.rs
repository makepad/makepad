//! The wall's compact face: what the home-screen tile shows, decided from
//! the same state the full view has — the open library, the filter, the
//! picture the person last picked. No second grid, no second store: the
//! tile is the resident `TileGrid` drawn small with a caption over it (or
//! an explicit empty state when nothing is baked), and the numbers here
//! are pure so the rules are tested without a window.

use makepad_image_tiles::library::ItemId;
use makepad_widgets::HostedViewMode;

/// The picture the person last clicked on the wall.
#[derive(Clone, Debug, PartialEq)]
pub struct Selected {
    pub item: ItemId,
    pub title: String,
}

/// How long after a face or viewport change before the camera returns to
/// the selected picture: the grid re-cuts its packing a beat after any
/// resize, and the picture's place on the wall only exists after that.
pub const REFOCUS_DELAY: f64 = 0.6;

/// The most characters a tile caption carries before eliding.
pub const CAPTION_CHARS: usize = 56;

/// Everything the widgets need to be told for one face.
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    pub mode: HostedViewMode,
    /// The search row and the status line — the full face's chrome.
    pub chrome: bool,
    /// The caption pill over the compact wall.
    pub caption: bool,
    pub caption_app: String,
    pub caption_title: String,
    /// The compact empty state instead of a wall with nothing on it.
    pub empty: bool,
}

/// Decide the face. `pictures` is what the library reported, `visible`
/// how many the filter keeps, `query` the filter itself.
pub fn presentation(mode: HostedViewMode, selected: Option<&Selected>, pictures: usize, visible: usize, query: &str) -> Presentation {
    let tile = mode == HostedViewMode::Tile;
    let empty = tile && pictures == 0;
    let caption_title = match selected {
        Some(s) => short_title(&s.title, CAPTION_CHARS),
        None if !query.trim().is_empty() => format!("{visible} of {pictures} · {}", query.trim()),
        None => format!("{pictures} pictures"),
    };
    Presentation {
        mode,
        chrome: !tile,
        caption: tile && !empty,
        caption_app: "Photos".to_string(),
        caption_title,
        empty,
    }
}

/// One line for the caption: the first line of the title, elided.
pub fn short_title(title: &str, max: usize) -> String {
    let line = title.lines().next().unwrap_or("").trim();
    let count = line.chars().count();
    if count <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tile_captions_the_selected_picture_and_the_full_face_keeps_its_chrome() {
        let picked = Selected { item: 7, title: "2024-01-02: A robot learns to love".into() };
        let tile = presentation(HostedViewMode::Tile, Some(&picked), 293, 293, "");
        assert!(tile.caption && !tile.chrome && !tile.empty);
        assert_eq!(tile.caption_title, "2024-01-02: A robot learns to love");
        let full = presentation(HostedViewMode::Full, Some(&picked), 293, 293, "");
        assert!(full.chrome && !full.caption && !full.empty);
    }

    #[test]
    fn without_a_pick_the_caption_describes_the_wall_or_its_filter() {
        assert_eq!(presentation(HostedViewMode::Tile, None, 293, 293, "").caption_title, "293 pictures");
        assert_eq!(presentation(HostedViewMode::Tile, None, 293, 12, " robots ").caption_title, "12 of 293 · robots");
    }

    #[test]
    fn an_empty_library_gets_the_explicit_empty_face_only_when_compact() {
        let tile = presentation(HostedViewMode::Tile, None, 0, 0, "");
        assert!(tile.empty && !tile.caption && !tile.chrome);
        let full = presentation(HostedViewMode::Full, None, 0, 0, "");
        assert!(!full.empty && full.chrome, "the full face keeps its own status line");
    }

    #[test]
    fn long_and_multiline_titles_become_one_short_line() {
        let long = "x".repeat(200);
        let short = short_title(&long, CAPTION_CHARS);
        assert_eq!(short.chars().count(), CAPTION_CHARS);
        assert!(short.ends_with('…'));
        assert_eq!(short_title("first line\nsecond line", 20), "first line");
        assert_eq!(short_title("  padded  ", 20), "padded");
    }
}
