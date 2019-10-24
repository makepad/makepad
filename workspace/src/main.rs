// makepad uses these workspaces essentially as make files, but networked and persistent and also being a fileserver.
use hub::*;

pub fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    
    HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, HttpServe::Disabled, & | ws, htc | match htc.msg {
        HubMsg::PackagesRequest {uid} => {
            // lets read our Cargo.toml in the root
            let packages = ws.read_packages(uid) ?;
            let builds = &["check", "debug", "release", "small"];
            ws.packages_response(
                htc.from,
                uid,
                packages.iter().map( | v | HubPackage::new(v, builds)).collect()
            );
            Ok(())
        },
        HubMsg::Build {uid, package, config} => {
            let mut args = Vec::new();
            let mut env = Vec::new();
            match config.as_ref() {
                "small" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                "release" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                "debug" => args.extend_from_slice(&["build", "-p", &package]),
                "check" => args.extend_from_slice(&["check", "-p", &package]),
                _ => return ws.cannot_find_build(uid, &package, &config)
            }
            
            if config == "small" {
                env.push(("RUSTFLAGS", "-C opt-level=z -C panic=abort -C codegen-units=1"))
            }
            
            if package.ends_with("wasm") {
                args.push("--target=wasm32-unknown-unknown");
            }
            
            if let BuildResult::Wasm {path} = ws.cargo(uid, &args, &env) ? {
                if config == "small" { // strip the build
                    ws.wasm_strip_debug(uid, &path) ?;
                }
            }
            Ok(())
        },
        HubMsg::WorkspaceFileTreeRequest {uid} => {
            ws.workspace_file_tree(
                htc.from,
                uid,
                &[".json", ".toml", ".js", ".rs", ".txt", ".text", ".ron", ".html"],
                Some("index.ron")
            );
            Ok(())
        },
        _ => ws.default(htc)
    });
}
