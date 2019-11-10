// workspaces are networked build and file servers. This 'main' one is also compiled into makepad
use hub::*;

pub fn workspace(ws: &mut HubWorkspace, htc: FromHubMsg) -> Result<(), HubWsError> {
    match htc.msg {
        HubMsg::ListPackagesRequest {uid} => {
            // lets read our Cargo.toml in the root
            let packages = ws.read_packages(uid);
            let builds = &["check", "debug", "release", "small"];
            ws.packages_response(
                htc.from,
                uid,
                packages.iter().map( | (project, v) | HubPackage::new(project, v, builds)).collect()
            );
            Ok(())
        },
        HubMsg::Build {uid, project, package, config} => {
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
            
            if let BuildResult::Wasm {path} = ws.cargo(uid, &project, &args, &env) ? {
                if config == "small" { // strip the build
                    ws.wasm_strip_debug(uid, &path) ?;
                }
            }
            Ok(())
        },
        _ => ws.default(htc)
    }
}

#[allow(dead_code)]
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    HubWorkspace::run_workspace_commandline(args, | ws, htc | {workspace(ws, htc)});
}
