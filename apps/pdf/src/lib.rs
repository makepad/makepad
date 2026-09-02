//! pdf: a PDF viewer for the Makepad app family. See Cargo.toml for scope.
//!
//! The parsing is `makepad-pdf-parse` and the page drawing is the `PdfView` /
//! `PdfPageView` pair in widgets/src/pdf_view.rs; this crate is the viewer
//! around them.

pub mod loader;
pub mod preview;
pub mod theme;
pub mod thumbs;
pub mod widget;

use std::path::PathBuf;

/// What the command line asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    /// The document to open, if one was named.
    pub path: Option<PathBuf>,
    /// `--preview`: a popup-sized Quick Look window rather than a full one.
    pub preview: bool,
}

/// Parse pdf's own arguments out of `argv` (already skipping argv[0]).
///
/// Every other flag belongs to somebody else — `--remote`, `--stdin-loop`,
/// `MAKEPAD_*` switches — and is left alone, exactly as the other Makepad apps
/// treat them.
pub fn parse_args<I: IntoIterator<Item = String>>(argv: I) -> Args {
    let mut args = Args::default();
    for arg in argv {
        if arg == "--preview" {
            args.preview = true;
        } else if arg.starts_with('-') {
            // Not ours to parse.
        } else if args.path.is_none() {
            args.path = Some(PathBuf::from(arg));
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_path_is_the_document() {
        let args = parse_args(argv(&["paper.pdf"]));
        assert_eq!(args.path, Some(PathBuf::from("paper.pdf")));
        assert!(!args.preview);
    }

    #[test]
    fn preview_takes_the_path_after_it() {
        let args = parse_args(argv(&["--preview", "/docs/paper.pdf"]));
        assert!(args.preview);
        assert_eq!(args.path, Some(PathBuf::from("/docs/paper.pdf")));
    }

    #[test]
    fn other_flags_are_left_to_their_owners() {
        // --remote and its port form belong to the platform, not to us, and
        // neither may be mistaken for a filename.
        let args = parse_args(argv(&["--remote=5000", "--stdin-loop", "a.pdf", "b.pdf"]));
        assert_eq!(args.path, Some(PathBuf::from("a.pdf")));
        assert!(!args.preview);
    }

    #[test]
    fn no_document_is_a_valid_start() {
        assert_eq!(parse_args(argv(&[])), Args::default());
    }
}
