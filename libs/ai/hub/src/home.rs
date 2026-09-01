use std::fs;
use std::path::PathBuf;

/// The shared per-user home for Makepad AI state.
pub fn makepad_home() -> PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
        return PathBuf::from(home);
    }
    // USERPROFILE on Windows, HOME elsewhere; temp dir as a last resort.
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}

fn create_home_subdir(name: &str) -> PathBuf {
    let path = makepad_home().join(name);
    let _ = fs::create_dir_all(&path);
    path
}

/// The shared model weights directory, created on demand.
pub fn weights_dir() -> PathBuf {
    create_home_subdir("weights")
}

/// The private runtime-state directory, created on demand.
pub fn run_dir() -> PathBuf {
    let path = create_home_subdir("run");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
    }
    path
}

/// The shared cache directory, created on demand.
pub fn cache_dir() -> PathBuf {
    create_home_subdir("cache")
}

/// The shared logs directory, created on demand.
pub fn logs_dir() -> PathBuf {
    create_home_subdir("logs")
}

/// Returns the default weights directory, migrating the legacy directory once.
pub fn default_weights_dir_with_migration(log: &mut dyn FnMut(&str)) -> PathBuf {
    let home = makepad_home();
    let legacy = home.join("ai_content");
    let weights = home.join("weights");

    if legacy.exists() {
        if weights.exists() {
            log("legacy ai_content and weights directories both exist; leaving ai_content alone");
            return weights;
        }
        return match fs::rename(&legacy, &weights) {
            Ok(()) => weights,
            Err(err) => {
                log(&format!(
                    "could not migrate {} to {}: {err}; continuing to use the legacy directory",
                    legacy.display(),
                    weights.display()
                ));
                legacy
            }
        };
    }

    weights_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvRestore {
        name: &'static str,
        value: Option<OsString>,
    }

    impl EnvRestore {
        fn set(name: &'static str, value: &OsStr) -> Self {
            let restore = Self {
                name,
                value: std::env::var_os(name),
            };
            std::env::set_var(name, value);
            restore
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "makepad-ai-hub-home-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn home_directories_and_migration() {
        let temp = TempDir::new();
        let _makepad_home = EnvRestore::set("MAKEPAD_HOME", temp.0.join("fresh").as_os_str());

        let weights = weights_dir();
        assert_eq!(weights, temp.0.join("fresh/weights"));
        assert!(weights.is_dir());

        let run = run_dir();
        assert!(run.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(run).unwrap().permissions().mode() & 0o777, 0o700);
        }

        let legacy_home = temp.0.join("legacy");
        std::env::set_var("MAKEPAD_HOME", &legacy_home);
        let legacy = legacy_home.join("ai_content");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("sentinel"), b"legacy").unwrap();
        let mut logs = Vec::new();
        let migrated = default_weights_dir_with_migration(&mut |line| logs.push(line.to_owned()));
        assert_eq!(migrated, legacy_home.join("weights"));
        assert!(!legacy.exists());
        assert!(migrated.join("sentinel").is_file());

        logs.clear();
        assert_eq!(
            default_weights_dir_with_migration(&mut |line| logs.push(line.to_owned())),
            migrated
        );
        assert!(logs.is_empty());

        let both_home = temp.0.join("both");
        std::env::set_var("MAKEPAD_HOME", &both_home);
        let old = both_home.join("ai_content");
        let new = both_home.join("weights");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        logs.clear();
        assert_eq!(
            default_weights_dir_with_migration(&mut |line| logs.push(line.to_owned())),
            new
        );
        assert!(old.is_dir());
        assert!(!logs.is_empty());

        let override_home = temp.0.join("override");
        let _home = EnvRestore::set("HOME", temp.0.join("ignored-home").as_os_str());
        std::env::set_var("MAKEPAD_HOME", &override_home);
        assert_eq!(makepad_home(), override_home);
    }
}
