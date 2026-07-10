fn main() {
    // Mirror platform/build.rs: MAKEPAD=headless builds get cfg(headless), so
    // the GameView can stub the gamepad path (the headless platform backend
    // has no game-input implementation).
    println!("cargo:rustc-check-cfg=cfg(headless)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if let Ok(configs) = std::env::var("MAKEPAD") {
        if configs.split(['+', ',']).any(|c| c == "headless") {
            println!("cargo:rustc-cfg=headless");
        }
    }
}
