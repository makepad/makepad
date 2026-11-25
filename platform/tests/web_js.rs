//! The web backend's JavaScript (src/os/web/*.js) under its tests (src/os/web/*.test.mjs), each
//! file a page in our own CEF (libs/web_test).

fn main() -> std::process::ExitCode {
    makepad_web_test::main(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), &["src/os/web"])
}
