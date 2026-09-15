# ohos-sys

FFI-bindings for the native API of [OpenHarmony OS]. See the [documentation] for a list of supported components.
This crate is under active development, and not officially affiliated with OpenHarmony OS.

## Development

The current bindings are generated with `bindgen` using `scripts/generate_bindings.sh`.
A separate file is generated for each API version, and (assuming no breaking changes) the new additions are
manually copied to an `apiXX_additions.rs` file, which is included as a module if `feature = api-XX` is selected.
The generated file `drawing_apiXX.rs` should be committed to version control, so we can easily rerun the script on a 
patch-release for a given API-level and see what changed.
The file itself is however not needed, and will be excluded from `crates.io` releases.

# Contributing

There are still quite a few OpenHarmony APIs missing. Feel free to contribute missing APIs, but be sure to adapt
the script, so your bindings are reproducible! 
Please also check the following:

- Ensure that opaque struct definitions do not derive `Copy`, `Clone` and `Debug`.
- Blocklist all unnecessary type definitions, e.g. from the C standard library.
- Preferably generate the bindings with libclang in `C` mode. However, if a header file is not C-compliant
  due to an issue of the OpenHarmony SDK, then setting `libclang` to C++ mode is fine.
- Be sure to guard the new component behind a cargo feature and document the feature in Cargo.toml.
- If you did not generate the bindings with API-level 10, specify which API-level you generated the bindings with
  and guard the generated module behind the corresponding api-level feature flag.
- Installing `bindgen`: We require at least bindgen 0.70.0, with the `prettyplease` feature enabled. 
  You can install it by running `cargo install bindgen-cli --features prettyplease`

## License

This crate is licensed under the Apache-2.0 license, matching the OpenHarmony OS SDK.

[OpenHarmony OS]: https://docs.openharmony.cn/pages/v5.0/en/OpenHarmony-Overview.md
[documentation]: https://docs.rs/ohos-sys/latest/ohos_sys/
