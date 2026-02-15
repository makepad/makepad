# CHANGELOG: wayland-egl

## Unreleased

## 0.32.0 -- 2023-09-02

#### Breaking changes

- Update wayland-backend to 0.3 & wayland-client 0.31

## 0.31.0 -- 2023-07-13

#### Breaking changes

- Update wayland-sys to 0.31

## 0.30.0 -- 2022-12-27

## 0.30.0-alpha9

#### Breaking changes

- `WlEglSurface` creation API now correcly handles all error conditions, as a result
  `wayland-egl` now has its own error type.

## 0.30.0-alpha6

#### Breaking changes

- `WlEglSurface` is now `!Sync`, as it should have been from the start

## 0.30.0-alpha1

Rework of the crate as a consequence of the rework of `wayland-client`.
