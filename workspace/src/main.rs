// makepad uses these workspaces essentially as make files, but networked and persistent and also being a fileserver.
use hub::*;

pub fn main() {
    let key = std::fs::read("./key.bin").unwrap();
    
    HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, HttpServe::Disabled, & | ws, htc | match htc.msg {
        HubMsg::PackagesRequest {uid} => {
            let builds = &["check", "debug", "release"];
            ws.packages_response(htc.from, uid, vec![
                HubPackage::new("workspace", builds),
                HubPackage::new("makepad", builds),
                HubPackage::new("csvproc", builds),
                HubPackage::new("ui_example", builds),
                HubPackage::new("makepad_webgl", builds),
            ]);
            Ok(())
        },
        HubMsg::Build {uid, package, config} => {
            let mut args = Vec::new();
            let env = Vec::new();
            match config.as_ref() {
                "release" => args.extend_from_slice(&["build", "--release", "-p", &package]),
                "debug" => args.extend_from_slice(&["build", "-p", &package]),
                "check" => args.extend_from_slice(&["check", "-p", &package]),
                _ => return ws.cannot_find_build(uid, &package, &config)
            }
            
            match package.as_ref() {
                "makepad_webgl" => args.push("--target=wasm32-unknown-unknown"),
                _ => ()
            }
            
            if let BuildResult::Wasm {path} = ws.cargo(uid, &args, &env) ? {
                if config == "release" { // strip the build
                    ws.wasm_strip_debug(&path);
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
