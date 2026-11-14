// `--empty`: started with a cleared environment, the program has none (its own reads, a list of
// them, a shell's; `--via-shell`: but PATH, which the shell that started it may set).
// `--token`: prints the PREPARED_TOKEN it was given. `--nested`: starts itself (current_exe) with
// a cleared environment. `--shell-nested`: a shell it starts with a cleared environment execs it;
// `--shell-changed`: a shell it starts with another PREPARED_TOKEN (the build script's input)
// execs it, which is still the run's build (relax issue 172, PR 175's shell case).
// `--env-off-disk`: no file in the run's own folder (its executable's) holds PREPARED_MARKER's
// value, which the run's build environment has: the run hands that to a launch of its executable
// by a descriptor, never in a file a killed run would leave.
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |s: &str| args.iter().any(|a| a == s);
    if has("--empty") {
        for name in ["HOME", "PATH", "PREPARED_TOKEN", "CARGO_HOME", "RUSTUP_HOME"] {
            if name == "PATH" && has("--via-shell") {
                continue;
            }
            assert!(std::env::var_os(name).is_none(), "the program reads {name}");
            assert!(!std::env::vars_os().any(|(k, _)| k == name), "the program lists {name}");
        }
        #[cfg(unix)]
        {
            let out = Command::new("/bin/sh").args(["-c", "printf '%s:%s' \"${HOME-unset}\" \"${PREPARED_TOKEN-unset}\""]).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), "unset:unset", "a shell the program starts has variables it was not given");
        }
    }
    if has("--token") {
        println!("token {}", std::env::var("PREPARED_TOKEN").unwrap_or_default());
    }
    let built = format!("{} 5\n", env!("PREPARED_BUILD"));
    let exe = std::env::current_exe().unwrap();
    if has("--nested") {
        let child = Command::new(&exe).arg("--empty").env_clear().output().unwrap();
        assert!(child.status.success(), "its own executable, started with a cleared environment: {}", String::from_utf8_lossy(&child.stderr));
        assert_eq!(String::from_utf8_lossy(&child.stdout), built);
    }
    #[cfg(unix)]
    {
        if has("--env-off-disk") {
            use std::os::unix::fs::MetadataExt;
            let marker = std::env::var("PREPARED_MARKER").expect("the fixture sets PREPARED_MARKER");
            let me = std::fs::metadata(&exe).unwrap();
            let mut dirs = vec![exe.parent().unwrap().to_path_buf()];
            let mut files = 0;
            while let Some(d) = dirs.pop() {
                for e in std::fs::read_dir(&d).unwrap().flatten() {
                    let m = e.metadata().unwrap();
                    if m.is_dir() {
                        dirs.push(e.path());
                    } else if !(m.dev() == me.dev() && m.ino() == me.ino()) {
                        // (the links of the loader are the program's executable itself)
                        files += 1;
                        let bytes = std::fs::read(e.path()).unwrap();
                        assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()), "{} holds the run's environment", e.path().display());
                    }
                }
            }
            assert!(files > 0, "the run's folder has its command line beside its executable");
        }
        if has("--shell-nested") {
            let child = Command::new("/bin/sh").args(["-c", "exec \"$0\" --empty --via-shell"]).arg(&exe).env_clear().output().unwrap();
            assert!(child.status.success(), "its own executable, exec'd by a shell with a cleared environment: {}", String::from_utf8_lossy(&child.stderr));
            assert_eq!(String::from_utf8_lossy(&child.stdout), built);
        }
        if has("--shell-changed") {
            let child = Command::new("/bin/sh").args(["-c", "exec \"$0\" --token"]).arg(&exe).env("PREPARED_TOKEN", "other").output().unwrap();
            assert!(child.status.success(), "its own executable, exec'd by a shell with another build input: {}", String::from_utf8_lossy(&child.stderr));
            assert_eq!(String::from_utf8_lossy(&child.stdout), format!("token other\n{built}"));
        }
    }
    println!("{} {}", env!("PREPARED_BUILD"), pc_native::add(2, 3));
}
