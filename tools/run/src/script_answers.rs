//! Makepad's answers to its build scripts (relax-front unitgraph.rs `ScriptAnswer`), registered by
//! the makepad host (lib.rs HOST): the bare `relax` runs these scripts as programs.
#[path = "../../../platform/build_gen.rs"]
mod platform_gen;

use relax_compiler::unitgraph::{ScriptAnswer, ScriptEnv};

pub static PLATFORM: ScriptAnswer = ScriptAnswer { package: "makepad-platform", answer: platform };

/// What makepad-platform's build.rs (platform/build.rs) gives the build, from the code build.rs
/// includes (platform/build_gen.rs), without running the script: its cfgs (`apple_sim` from the
/// target, the `lines` / `apple_bundle` words of the MAKEPAD_BUILD variable) and its two OUT_DIR
/// files, app_meta_gen.rs (the default bundle name and identifier, from the folder that holds the
/// target folder) and app_icon_gen.rs (the icons in that folder's resources/). Its link lines are
/// the host binary's: in this mode nothing is linked at run time. Its side files
/// (makepad-platform.path, a target-folder Info.plist) are for cargo runs only.
fn platform(e: &ScriptEnv) -> Result<Vec<String>, String> {
    let target_os = e.env.iter().find(|(k, _)| k == "CARGO_CFG_TARGET_OS").map_or("", |(_, v)| v.as_str());
    let makepad = std::env::var("MAKEPAD_BUILD").ok();
    let lines = platform_gen::cfgs(makepad.as_deref(), target_os, e.target).into_iter().map(|c| format!("cargo:rustc-cfg={}", c)).collect();
    for (f, text) in [("app_meta_gen.rs", platform_gen::app_meta_gen(e.out_dir)), ("app_icon_gen.rs", platform_gen::app_icon_gen(e.out_dir).source)] {
        let path = e.out_dir.join(f);
        // unchanged files stay as they are (their stamps decide what is reparsed)
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).map_err(|err| format!("{}: {}", path.display(), err))?;
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::platform_gen;
    use std::process::Command;

    /// platform/build.rs, compiled and run for real, writes the bytes the makepad host answers with
    /// natively (`PLATFORM`; both call platform/build_gen.rs): app_meta_gen.rs, app_icon_gen.rs and
    /// the cfg lines.
    /// The script is this crate's build input, not a file read beside its sources: under the JIT
    /// those are a virtual tree, and rustc, another process, reads only files on disk.
    #[test]
    fn native_answer_is_byte_identical_to_the_build_script() {
        let tmp = std::env::temp_dir().join(format!("relax-platform-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let platform = tmp.join("platform");
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(platform.join("build.rs"), include_str!("../../../platform/build.rs")).unwrap();
        std::fs::write(platform.join("build_gen.rs"), include_str!("../../../platform/build_gen.rs")).unwrap();
        let root = tmp.join("some app");
        std::fs::create_dir_all(root.join("resources")).unwrap();
        std::fs::write(root.join("resources/icon_64.png"), b"png").unwrap();
        std::fs::write(root.join("resources/icon.ico"), b"ico").unwrap();
        let out = root.join("target/release/build/makepad-platform-0123456789abcdef/out");
        std::fs::create_dir_all(&out).unwrap();
        let exe = tmp.join("build-script");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let built = Command::new(rustc).args(["--edition", "2021", "-o"]).arg(&exe).arg(platform.join("build.rs")).output().expect("rustc");
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        for (os, target, makepad_build) in [("linux", "x86_64-unknown-linux-gnu", None), ("ios", "aarch64-apple-ios-sim", Some("lines+apple_bundle,other"))] {
            let mut cmd = Command::new(&exe);
            cmd.current_dir(&platform).env_clear().env("OUT_DIR", &out).env("CARGO_CFG_TARGET_OS", os).env("TARGET", target);
            if let Some(b) = makepad_build { cmd.env("MAKEPAD_BUILD", b); }
            let ran = cmd.output().expect("build script");
            assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
            let said: Vec<String> = String::from_utf8_lossy(&ran.stdout).lines().filter_map(|l| l.strip_prefix("cargo:rustc-cfg=")).map(str::to_string).collect();
            let native: Vec<String> = platform_gen::cfgs(makepad_build, os, target).into_iter().map(str::to_string).collect();
            assert_eq!(said, native, "{os}");
            assert_eq!(std::fs::read_to_string(out.join("app_meta_gen.rs")).unwrap(), platform_gen::app_meta_gen(&out));
            assert_eq!(std::fs::read_to_string(out.join("app_icon_gen.rs")).unwrap(), platform_gen::app_icon_gen(&out).source);
        }
        assert!(platform_gen::app_meta_gen(&out).contains("\"Some app\""));
        assert!(platform_gen::app_icon_gen(&out).source.contains("icon_64.png"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
