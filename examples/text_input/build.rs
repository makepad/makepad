//! Sync Makepad web assets into `public/` for Dioxus `dx serve --web` (same layout as `cargo makepad` wasm output):
//! - `wasm_bridge.js`, platform `web.js` / `web_gl.js` / `full_canvas.css`
//! - Dependency crate resources under `public/<crate_snake>/resources/` (see `tools/cargo_makepad/src/wasm/compile.rs`)
use std::fs;
use std::path::{Path, PathBuf};

fn copy_if_newer(src: &Path, dst: &Path) {
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.is_file() {
        return;
    }
    let _ = fs::create_dir_all(dst.parent().unwrap());
    let need_copy = match (fs::metadata(src), fs::metadata(dst)) {
        (Ok(sm), Ok(dm)) => sm.modified().unwrap() > dm.modified().unwrap(),
        _ => true,
    };
    if need_copy {
        fs::copy(src, dst).unwrap_or_else(|e| panic!("copy {} -> {}: {e}", src.display(), dst.display()));
    }
}

fn rerun_if_changed_tree(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            println!("cargo:rerun-if-changed={}", p.display());
            if p.is_dir() {
                rerun_if_changed_tree(&p);
            }
        }
    }
}

fn copy_tree_if_newer(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    let _ = fs::create_dir_all(dst);
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let out = dst.join(e.file_name());
        if p.is_dir() {
            copy_tree_if_newer(&p, &out);
        } else {
            let need_copy = match (fs::metadata(&p), fs::metadata(&out)) {
                (Ok(sm), Ok(dm)) => sm.modified().unwrap() > dm.modified().unwrap(),
                _ => true,
            };
            if need_copy {
                let _ = fs::create_dir_all(out.parent().unwrap());
                fs::copy(&p, &out).unwrap_or_else(|e| {
                    panic!("copy {} -> {}: {e}", p.display(), out.display())
                });
            }
        }
    }
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_platform = dir.join("../../platform/src/os/web");

    let bridge_src = dir.join("../../libs/wasm_bridge/src/wasm_bridge.js");
    let bridge_dst = dir.join("public/makepad_wasm_bridge/wasm_bridge.js");
    copy_if_newer(&bridge_src, &bridge_dst);

    let platform_dst = dir.join("public/makepad_platform");
    copy_if_newer(&repo_platform.join("web.js"), &platform_dst.join("web.js"));
    copy_if_newer(&repo_platform.join("web_gl.js"), &platform_dst.join("web_gl.js"));
    copy_if_newer(&repo_platform.join("full_canvas.css"), &platform_dst.join("full_canvas.css"));

    // `cargo makepad` wasm places each dependency's `resources/` at `public/<name_underscore>/resources/`.
    let widgets_res = dir.join("../../widgets/resources");
    if widgets_res.is_dir() {
        rerun_if_changed_tree(&widgets_res);
        copy_tree_if_newer(
            &widgets_res,
            &dir.join("public/makepad_widgets/resources"),
        );
    }

    println!("cargo:rerun-if-changed={}", dir.join("public/index.html").display());
}
