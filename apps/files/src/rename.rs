//! Renaming: what a new name is allowed to be, and what a pattern does to a
//! whole selection at once.
//!
//! Both halves are pure string work with no filesystem in them, which is what
//! lets the batch dialog show a live preview of exactly what pressing Rename
//! will do — the preview and the operation run the same function.

/// Characters a filename may not contain. The separator would silently move
/// the file somewhere else, and NUL cannot survive the syscall.
pub const FORBIDDEN: [char; 2] = ['/', '\0'];

/// Why a name was refused, in the words the dialog shows.
pub fn name_error(name: &str) -> Option<&'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Some("A name cannot be empty");
    }
    if trimmed == "." || trimmed == ".." {
        return Some("That name belongs to the folder itself");
    }
    if name.contains(FORBIDDEN) {
        return Some("A name cannot contain a slash");
    }
    None
}

/// True when `name` can be handed to the filesystem as-is.
pub fn is_valid_name(name: &str) -> bool {
    name_error(name).is_none()
}

/// Split a filename into (stem, extension-with-dot). A dotfile with no second
/// dot is all stem — `.zshrc` has no extension, it *is* a name that starts
/// with one, and renaming it must not eat the leading dot.
pub fn split_extension(name: &str) -> (&str, &str) {
    let body = name.strip_prefix('.').unwrap_or(name);
    match body.rfind('.') {
        Some(at) if at > 0 => {
            let cut = at + (name.len() - body.len());
            (&name[..cut], &name[cut..])
        }
        _ => (name, ""),
    }
}

/// How a batch rename builds each new name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchMode {
    /// Replace every occurrence of `find` in the name with `replace`. An
    /// empty `find` changes nothing, which is what an untouched dialog shows.
    FindReplace { find: String, replace: String },
    /// Build the name from a pattern: `###` becomes the running number, zero
    /// padded to the run's length, and `{name}` the original stem. The
    /// original extension is always kept, so a template cannot make a folder
    /// out of a picture.
    Template { pattern: String },
}

impl BatchMode {
    /// The mode a dialog with these two fields means: a pattern wins when the
    /// user typed one, because it is the more specific instruction.
    pub fn from_fields(find: &str, replace: &str, pattern: &str) -> BatchMode {
        if !pattern.trim().is_empty() {
            BatchMode::Template {
                pattern: pattern.to_string(),
            }
        } else {
            BatchMode::FindReplace {
                find: find.to_string(),
                replace: replace.to_string(),
            }
        }
    }
}

/// The new name for every input, in input order, with collisions inside the
/// batch broken apart — two files that would land on one name would leave the
/// user with one file, so the second gets a " (2)".
///
/// A name the filesystem would refuse comes back unchanged: the batch renames
/// what it can and leaves the rest alone rather than failing whole.
pub fn batch_rename(names: &[String], mode: &BatchMode, start: u32) -> Vec<String> {
    let width = number_width(names.len(), start);
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let number = start + index as u32;
        let candidate = match mode {
            BatchMode::FindReplace { find, replace } => {
                if find.is_empty() {
                    name.clone()
                } else {
                    name.replace(find.as_str(), replace)
                }
            }
            BatchMode::Template { pattern } => {
                let (stem, extension) = split_extension(name);
                format!("{}{}", expand(pattern, stem, number, width), extension)
            }
        };
        let candidate = if is_valid_name(&candidate) {
            candidate.trim().to_string()
        } else {
            name.clone()
        };
        out.push(deduplicate(candidate, &out));
    }
    out
}

/// True when the batch would change nothing — the dialog's Rename button has
/// no work to do and says so instead of running an empty operation.
pub fn is_noop(names: &[String], renamed: &[String]) -> bool {
    names.len() == renamed.len() && names.iter().zip(renamed).all(|(a, b)| a == b)
}

/// Substitute the pattern's tokens for one item.
fn expand(pattern: &str, stem: &str, number: u32, width: usize) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("{name}") {
            out.push_str(stem);
            rest = tail;
            continue;
        }
        if rest.starts_with('#') {
            // A run of hashes is one placeholder, and its length is the
            // padding the user asked for — "###" is 001, "#" is 1 (widened
            // to whatever the run's largest number needs).
            let run = rest.chars().take_while(|c| *c == '#').count();
            out.push_str(&format!("{:0>width$}", number, width = run.max(width)));
            rest = &rest[run..];
            continue;
        }
        let c = rest.chars().next().unwrap_or('\0');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Digits needed for the largest number in the run, so a pattern with a bare
/// `#` still lines up when the selection runs past nine.
fn number_width(count: usize, start: u32) -> usize {
    let last = start as u64 + count.max(1) as u64 - 1;
    last.to_string().len()
}

/// " (2)" a name that another item in the same batch already took.
fn deduplicate(candidate: String, taken: &[String]) -> String {
    if !taken.contains(&candidate) {
        return candidate;
    }
    let (stem, extension) = split_extension(&candidate);
    for n in 2..1000 {
        let next = format!("{stem} ({n}){extension}");
        if !taken.contains(&next) {
            return next;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn refuses_names_the_filesystem_would_not_take() {
        assert!(is_valid_name("report.txt"));
        assert!(is_valid_name(".zshrc"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("   "));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name(".."));
        assert_eq!(name_error("a/b"), Some("A name cannot contain a slash"));
    }

    #[test]
    fn splits_the_extension_without_eating_the_leading_dot() {
        assert_eq!(split_extension("report.txt"), ("report", ".txt"));
        assert_eq!(split_extension("archive.tar.gz"), ("archive.tar", ".gz"));
        assert_eq!(split_extension("README"), ("README", ""));
        assert_eq!(split_extension(".zshrc"), (".zshrc", ""));
        assert_eq!(split_extension(".config.json"), (".config", ".json"));
    }

    #[test]
    fn find_and_replace_touches_only_the_match() {
        let mode = BatchMode::FindReplace {
            find: "IMG".to_string(),
            replace: "Holiday".to_string(),
        };
        let out = batch_rename(&names(&["IMG_1.png", "IMG_2.png", "other.png"]), &mode, 1);
        assert_eq!(out, ["Holiday_1.png", "Holiday_2.png", "other.png"]);
    }

    #[test]
    fn an_empty_find_changes_nothing() {
        let mode = BatchMode::FindReplace {
            find: String::new(),
            replace: "x".to_string(),
        };
        let input = names(&["a.txt", "b.txt"]);
        let out = batch_rename(&input, &mode, 1);
        assert!(is_noop(&input, &out));
    }

    #[test]
    fn a_template_numbers_the_run_and_keeps_the_extension() {
        let mode = BatchMode::Template {
            pattern: "shot-###".to_string(),
        };
        let out = batch_rename(&names(&["a.png", "b.jpg", "c"]), &mode, 1);
        assert_eq!(out, ["shot-001.png", "shot-002.jpg", "shot-003"]);
    }

    #[test]
    fn a_bare_hash_still_lines_up_past_nine() {
        let mode = BatchMode::Template {
            pattern: "f#".to_string(),
        };
        let input: Vec<String> = (0..12).map(|i| format!("x{i}.txt")).collect();
        let out = batch_rename(&input, &mode, 1);
        assert_eq!(out[0], "f01.txt");
        assert_eq!(out[11], "f12.txt");
    }

    #[test]
    fn a_template_can_keep_the_original_name() {
        let mode = BatchMode::Template {
            pattern: "2026 {name} (#)".to_string(),
        };
        let out = batch_rename(&names(&["trip.png", "beach.png"]), &mode, 1);
        assert_eq!(out, ["2026 trip (1).png", "2026 beach (2).png"]);
    }

    #[test]
    fn collisions_inside_one_batch_are_broken_apart() {
        let mode = BatchMode::Template {
            pattern: "same".to_string(),
        };
        let out = batch_rename(&names(&["a.txt", "b.txt", "c.txt"]), &mode, 1);
        assert_eq!(out, ["same.txt", "same (2).txt", "same (3).txt"]);
    }

    #[test]
    fn a_pattern_that_yields_an_illegal_name_leaves_the_file_alone() {
        let mode = BatchMode::Template {
            pattern: "a/b".to_string(),
        };
        let out = batch_rename(&names(&["keep.txt"]), &mode, 1);
        assert_eq!(out, ["keep.txt"]);
    }

    #[test]
    fn the_dialog_fields_pick_the_mode() {
        assert_eq!(
            BatchMode::from_fields("a", "b", ""),
            BatchMode::FindReplace {
                find: "a".to_string(),
                replace: "b".to_string()
            }
        );
        assert!(matches!(
            BatchMode::from_fields("a", "b", "p-###"),
            BatchMode::Template { .. }
        ));
    }
}
