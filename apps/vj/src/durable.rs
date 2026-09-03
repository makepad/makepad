//! Writing the operator's own work to disk so that losing power cannot cost
//! it.
//!
//! Every derived cache in this app already writes to a temporary and renames
//! it into place; the files that hold work nobody can regenerate — the cue
//! and loop marks, the controller map, the panel settings — were written
//! straight over the previous good copy, and the result was discarded. A
//! crash in that window left a half-written file, and the next session read
//! whatever survived. This is the derived cache's discipline, applied to the
//! things that actually matter.

use std::io;
use std::path::Path;

/// Write `body` to `path` so that the file is either its old contents or its
/// new ones, never a mixture.
///
/// The bytes go to a temporary beside the destination and are renamed over
/// it, which is one indivisible step as far as any later reader is
/// concerned. A failure anywhere before that step leaves the previous good
/// file exactly as it was — and, unlike the in-place writes this replaces,
/// says so to its caller instead of returning nothing.
///
/// A temporary left behind by an earlier crash is simply overwritten: it is
/// named for its destination, so there is only ever one, and it belongs to
/// whoever is writing now.
/// A position along a track, read back from a file the operator's work
/// lives in.
///
/// `"NaN"` and `"inf"` both parse as numbers, so a torn write hands back
/// something that looks like a position and is not one; so does a negative
/// number, which no point in a track can be. Either would go on to poison
/// every span computed from it, so a line that cannot mean a position is
/// dropped instead.
pub fn seconds(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|secs| secs.is_finite() && *secs >= 0.0)
}

pub fn write_file(path: &Path, body: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, body)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("vj-durable").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_position_read_back_from_disk_is_one_the_engine_can_use() {
        // Text that parses is not the same as text that means something:
        // "NaN" and "inf" both parse as numbers, and a truncated write can
        // leave either. A position like that poisons every span the deck
        // computes from it.
        assert_eq!(seconds("12.5"), Some(12.5));
        assert_eq!(seconds("0"), Some(0.0));
        assert_eq!(seconds("NaN"), None);
        assert_eq!(seconds("inf"), None);
        assert_eq!(seconds("-inf"), None);
        assert_eq!(seconds("-1.0"), None, "a position before the start of a track");
        assert_eq!(seconds(""), None);
        assert_eq!(seconds("half"), None);
    }

    #[test]
    fn a_write_makes_its_directory_and_leaves_no_leftovers() {
        let dir = scratch("plain");
        let path = dir.join("deep").join("marks");
        write_file(&path, "cue 12.5\n").expect("write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "cue 12.5\n");
        let strays: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "marks")
            .collect();
        assert!(strays.is_empty(), "nothing beside the file itself: {strays:?}");
    }

    #[test]
    fn a_write_that_fails_leaves_the_previous_good_file_alone() {
        // The whole point. A torn write used to mean the operator's marks
        // came back as whatever bytes happened to land.
        let dir = scratch("failing");
        let path = dir.join("marks");
        write_file(&path, "cue 1.0\n").expect("the first write");
        // Make the temporary name unusable, the way a full disk or a lost
        // handle would: the write cannot complete, and must not have touched
        // the destination on its way to finding that out.
        std::fs::create_dir_all(path.with_extension("tmp")).expect("block the temp");
        assert!(write_file(&path, "cue 2.0\n").is_err(), "the write reports its failure");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "cue 1.0\n",
            "the previous good file survived"
        );
    }

    #[test]
    fn a_leftover_temporary_from_an_earlier_crash_does_not_block_the_next_write() {
        let dir = scratch("stale");
        let path = dir.join("marks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path.with_extension("tmp"), "half a file").unwrap();
        write_file(&path, "cue 3.0\n").expect("write over the leftover");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "cue 3.0\n");
        assert!(!path.with_extension("tmp").exists(), "and the leftover is gone");
    }

    #[test]
    fn a_rewrite_replaces_the_whole_file_rather_than_the_start_of_it() {
        let dir = scratch("shorter");
        let path = dir.join("marks");
        write_file(&path, "cue 10.0\n1.0 2.0\n3.0 4.0\n").expect("long");
        write_file(&path, "cue 0.0\n").expect("short");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "cue 0.0\n");
    }
}
