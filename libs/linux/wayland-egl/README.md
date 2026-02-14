[![crates.io](https://img.shields.io/crates/v/wayland-egl.svg)](https://crates.io/crates/wayland-egl)
[![docs.rs](https://docs.rs/wayland-egl/badge.svg)](https://docs.rs/wayland-egl)
[![Continuous Integration](https://github.com/Smithay/wayland-rs/workflows/Continuous%20Integration/badge.svg)](https://github.com/Smithay/wayland-rs/actions?query=workflow%3A%22Continuous+Integration%22)
[![codecov](https://codecov.io/gh/Smithay/wayland-rs/branch/master/graph/badge.svg)](https://codecov.io/gh/Smithay/wayland-rs)

# wayland-egl

This crate provides bindings for OpenGL/Vulkan support for Wayland client apps. It allows to
create an `EGLSurface` from any `WlSurface`, which can then play the role of the base surface
for initializing an OpenGL or Vulkan context.