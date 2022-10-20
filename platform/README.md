# makepad-platform

This is a lower-level crate for Makepad Framework. For an explanation of what Makepad Framework is, please see the README for the [makepad-widgets](https://crates.io/crates/makepad-widgets) crate.

This crate contains all platform specific code, including Rust bindings to native APIs such a X11 and GLX, code that uses these APIs to interact with both the window system and GPU, etc. In addition, this crate contains both the compiler and the runtime for the DSL, since the DSL interacts with almost every other part of the system.

This crate is re-exported by the [makepad-widgets](https://crates.io/crates/makepad-widgets) crate. In a typical application, you would depend on that crate instead of this one.

## Contact

If you have any questions/suggestions, feel free to reach out to us on our discord channel:
https://discord.com/invite/urEMqtMcSd=