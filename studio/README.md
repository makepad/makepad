# makepad-studio

This is a prototype of an IDE written in Makepad Framework. For an explanation of what Makepad Framework is, please see the README for the [makepad-widgets](https://crates.io/crates/makepad-widgets) crate.

Our eventual goal is to develop this into an IDE that is live design aware. Such an IDE can detect when changes are made to DSL code describing the styling of an application, rather than native Rust code, so that instead of a recompiling, it can send the changes to the DSL code over to the application, allowing the latter to update itself.

That said, this crate is currently still under heavy development. At the moment of this writing, it has a working dock system with tabs, a file tree with folding, and a basic code editor with syntax highlighting. Our main challenge at the moment is to redesign the IDE so that it has a proper extension model with sandboxing.

## Contact

If you have any questions/suggestions, feel free to reach out to us on our discord channel:
https://discord.com/invite/urEMqtMcSd=