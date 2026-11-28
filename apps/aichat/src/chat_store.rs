//! The panel's saved transcript and its sibling pictures folder.

use makepad_ai_services::engine::{EngineCore, NoModel, ServiceRegistry};
use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// A second panel, or a host switching back to this key, must also leave its save alone.
static UNSAVED: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Default::default);

pub(super) fn keep(path: &Path, engine: &mut EngineCore, now: SystemTime) {
    if let Some(note) = UNSAVED.lock().unwrap().get(path).cloned() {
        engine.announce(&note);
        return;
    }
    let Err(mut reason) = restore(path, engine) else { return };
    let recovery = (|| -> io::Result<()> {
        // Two builds can open the same damaged save. Recheck it under the lock before
        // moving either name. The OS releases the lock even if the app crashes.
        let lock = fs::OpenOptions::new().write(true).create(true).truncate(false).open(path.with_extension("recovery.lock"))?;
        lock.try_lock().map_err(io::Error::from)?;
        match restore(path, engine) {
            Ok(()) => return Ok(()),
            Err(current) => reason = current,
        }
        let name = set_aside(path, &utc_stamp(now))?;
        engine.keep_chat(path.with_extension("pictures"), None).map_err(io::Error::other)?;
        engine.announce(&format!("The old chat is kept as {name}. This one starts fresh because the saved chat could not be read ({}).", one_line(&reason)));
        Ok(())
    })();
    if let Err(error) = recovery {
        let note = format!("The saved chat could not be set aside ({error}), so this conversation is not saved until the app restarts. The save could not be read ({}).", one_line(&reason));
        UNSAVED.lock().unwrap().insert(path.to_owned(), note.clone());
        engine.announce(&note);
    }
}

fn one_line(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn restore(path: &Path, engine: &mut EngineCore) -> Result<(), String> {
    let pictures = path.with_extension("pictures");
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return engine.keep_chat(pictures, None),
        Err(e) => return Err(e.to_string()),
        Ok(meta) if !meta.is_file() => {
            // A dangling link exists, and trying to read a pipe can block the panel.
            if !meta.is_symlink() || !fs::metadata(path).is_ok_and(|m| m.is_file()) {
                return Err("the chat path does not point to a regular file".into());
            }
        }
        Ok(_) => {}
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    // keep_chat binds its pictures folder even when it rejects the text. Validate with
    // the same reader before a later Clear can sweep the old pictures. The JSON file
    // itself cannot contain pictures, so this check touches no picture directory.
    EngineCore::new(ServiceRegistry::new(), Box::new(NoModel::default()), None, 0)
        .keep_chat(path.to_owned(), Some(&text))?;
    // A host can name the chat after the person has already started talking.
    let saved = engine.state().entries.is_empty().then_some(text.as_str());
    engine.keep_chat(pictures, saved)
}

fn set_aside(path: &Path, stamp: &str) -> io::Result<String> {
    let key = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("missing chat key"))?;
    let source = fs::symlink_metadata(path)?;
    if !source.is_file() && !source.is_symlink() {
        return Err(io::Error::other("the chat path is not a file"));
    }
    let pictures = path.with_extension("pictures");
    let has_pictures = match fs::symlink_metadata(&pictures) {
        Ok(meta) if meta.is_dir() => true,
        Ok(_) => return Err(io::Error::other("the chat's pictures path is not a folder")),
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    for n in 1u64.. {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let name = format!("{key}.unreadable-{stamp}{suffix}.json");
        let archive = path.with_file_name(&name);
        let archive_pictures = archive.with_extension("pictures");
        match fs::symlink_metadata(&archive_pictures) {
            Ok(_) => continue,
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // Link then unlink is a no-clobber move in the same folder. Keep both names
        // until the pictures are safe, so a failure or crash cannot orphan their chat.
        match keep_file(path, &archive, source.is_file()) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        if has_pictures {
            if let Err(e) = move_pictures(&pictures, &archive_pictures) {
                fs::remove_file(&archive)?;
                if e.kind() == ErrorKind::AlreadyExists { continue }
                return Err(e);
            }
        }
        fs::remove_file(path).map_err(|e| io::Error::new(e.kind(), format!("kept as {name}, but the original name could not be removed: {e}")))?;
        return Ok(name);
    }
    unreachable!()
}

fn keep_file(path: &Path, archive: &Path, regular_file: bool) -> io::Result<()> {
    match fs::hard_link(path, archive) {
        Ok(()) => return Ok(()),
        // Moving a symbolic link keeps the link itself, including a dangling one.
        Err(e) if !regular_file || e.kind() == ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    // Some volumes have no hard links. Only release the original after all its bytes
    // have reached a newly created file and the pictures have moved.
    let mut original = fs::File::open(path)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(original.metadata()?.permissions().mode() & 0o777);
    }
    let mut backup = options.open(archive)?;
    if let Err(error) = io::copy(&mut original, &mut backup).and_then(|_| backup.sync_all()) {
        drop(backup);
        fs::remove_file(archive)?;
        return Err(error);
    }
    Ok(())
}

fn move_pictures(from: &Path, to: &Path) -> io::Result<()> {
    // Unix rename can replace an existing empty directory. Reserve our own empty
    // directory first. Windows already refuses to replace a destination directory.
    #[cfg(unix)]
    fs::create_dir(to)?;
    let moved = fs::rename(from, to);
    #[cfg(unix)]
    if moved.is_err() {
        fs::remove_dir(to)?;
    }
    moved
}

pub(super) fn write(path: &Path, engine: &EngineCore) -> io::Result<()> {
    if UNSAVED.lock().unwrap().contains_key(path) {
        return Ok(());
    }
    if engine.state().entries.is_empty() {
        return match fs::remove_file(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, engine.saved_chat())?;
    fs::rename(tmp, path)
}

fn utc_stamp(now: SystemTime) -> String {
    let seconds = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64) - i64::from(e.duration().subsec_nanos() != 0),
    };
    // Hinnant's civil_from_days, with March as the start of the year.
    let days = seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day = days.rem_euclid(146_097);
    let year_of_era = (day - day / 1_460 + day / 36_524 - day / 146_096) / 365;
    let day_of_year = day - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 { march_month + 3 } else { march_month - 9 };
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    let time = seconds.rem_euclid(86_400);
    format!("{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}", time / 3600, time / 60 % 60, time % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::state::Entry;
    use std::time::Duration;

    fn engine() -> EngineCore {
        EngineCore::new(ServiceRegistry::new(), Box::new(NoModel::default()), None, 1)
    }

    #[test]
    fn unreadable_chats_and_their_pictures_survive_the_next_write() {
        let root = std::env::temp_dir().join(format!("aichat-recovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create the isolated chat folder");
        let path = root.join("chat.json");
        let pictures = path.with_extension("pictures");
        let now = UNIX_EPOCH + Duration::from_secs(1_795_774_503);
        let picture = format!("{}.png", "ab".repeat(32)); // A name the engine's sweep recognizes.
        let mut fresh = engine();
        keep(&path, &mut fresh, now);
        assert!(fresh.state().entries.is_empty(), "a missing save must start without a warning");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "a first start must create no archive");

        let cases: &[(&[u8], &str, &str)] = &[
            (b"\xff\xfe\x00", "", "UTF-8"),
            (b"\xf0\x80broken", "-2", "UTF-8"),
            (br#"{"version":2,"rows":[]}"#, "-3", "version 2"),
        ];
        for &(bytes, suffix, why) in cases {
            fs::write(&path, bytes).unwrap();
            // The first collision has only a JSON file; the next also has pictures.
            if !suffix.is_empty() {
                fs::create_dir(&pictures).unwrap();
                fs::write(pictures.join(&picture), bytes).unwrap();
            }
            let mut core = engine();
            keep(&path, &mut core, now);
            let name = format!("chat.unreadable-2026-11-27-101503{suffix}.json");
            assert!(matches!(core.state().entries.as_slice(), [Entry::System { text }] if text.contains(&name) && text.contains(why)),
                "the fresh transcript must explain the read error and name {name} in one line");
            assert!(!pictures.exists(), "the old pictures folder must move before the fresh chat is bound");
            core.send("the fresh conversation", 0.0);
            core.pump(0.1);
            write(&path, &core).expect("write the fresh conversation");
            assert_eq!(fs::read(root.join(&name)).unwrap(), bytes, "the next write must preserve the exact bytes of {name}");
            if !suffix.is_empty() {
                assert_eq!(fs::read(root.join(&name).with_extension("pictures").join(&picture)).unwrap(), bytes,
                    "the old picture must stay with its own dated chat");
            }
        }
        for &(bytes, suffix, _) in cases {
            let archive = root.join(format!("chat.unreadable-2026-11-27-101503{suffix}.json"));
            assert_eq!(fs::read(archive).unwrap(), bytes, "a later recovery in the same second must not replace an earlier backup");
        }
        let listing = || {
            let mut names: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
            names.sort();
            names
        };
        let before = listing();
        let mut restored = engine();
        keep(&path, &mut restored, now);
        assert!(restored.state().entries.iter().any(|e| matches!(e, Entry::User { text, .. } if text == "the fresh conversation")),
            "the active file must restore the fresh conversation");
        assert_eq!(listing(), before, "opening a readable save must set nothing aside");

        // A directory at the recovery lock's path forces a real I/O error even when
        // the test runs with permission to write read-only files.
        let blocked = root.join("blocked.json");
        let old = br#"{"version":2,"rows":[]}"#;
        fs::write(&blocked, old).unwrap();
        let blocked_pictures = blocked.with_extension("pictures");
        fs::create_dir(&blocked_pictures).unwrap();
        fs::write(blocked_pictures.join(&picture), old).unwrap();
        let obstruction = blocked.with_extension("recovery.lock");
        fs::create_dir(&obstruction).unwrap();
        let mut core = engine();
        keep(&blocked, &mut core, now);
        assert!(matches!(core.state().entries.as_slice(), [Entry::System { text }] if text.contains("could not be set aside (")
            && text.contains("not saved until the app restarts") && text.contains("version 2")),
            "the transcript must explain the failure and say how long saving is disabled");
        fs::remove_dir(&obstruction).unwrap();
        core.send("in memory", 0.0);
        core.pump(0.1);
        write(&blocked, &core).expect("skip replacement of the protected save");
        assert_eq!(fs::read(&blocked).unwrap(), old, "a later write must leave a protected file byte for byte");
        core.clear(1.0);
        write(&blocked, &core).expect("skip removal of the protected save");
        assert_eq!(fs::read(&blocked).unwrap(), old, "clearing an unsaved chat must not delete the protected file");
        assert_eq!(fs::read(blocked_pictures.join(&picture)).unwrap(), old, "clearing an unsaved chat must not sweep its old pictures");
        restored.send("another chat can still be saved", 1.1);
        restored.pump(1.2);
        write(&path, &restored).expect("write the other key after one key was protected");
        assert!(fs::read_to_string(&path).unwrap().contains("another chat can still be saved"), "a failed recovery must disable only its own key");
        fs::remove_dir_all(root).expect("clean up the isolated chat folder");
    }
}
