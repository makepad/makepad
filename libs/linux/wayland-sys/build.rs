use pkg_config::Config;

fn main() {
    if std::env::var_os("CARGO_FEATURE_DLOPEN").is_some() {
        // Do not link to anything
        return;
    }

    let host = std::env::var("HOST").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();
    if host != target && std::env::var_os("WAYLAND_SYS_FORCE_PKG_CONFIG").is_none() {
        // Allow cross `cargo check` without a sysroot/pkg-config setup.
        return;
    }

    if std::env::var_os("CARGO_FEATURE_CLIENT").is_some() {
        Config::new().probe("wayland-client").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_CURSOR").is_some() {
        Config::new().probe("wayland-cursor").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_EGL").is_some() {
        Config::new().probe("wayland-egl").unwrap();
    }
    if std::env::var_os("CARGO_FEATURE_SERVER").is_some() {
        Config::new().probe("wayland-server").unwrap();
    }
}
