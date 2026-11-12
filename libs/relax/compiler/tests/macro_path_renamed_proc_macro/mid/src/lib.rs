//! One macro under its own name and the other under that same name in another module, as time
//! has `time::macros::format_description` and
//! `pub use time_macros::serde_format_description as format_description;` in `time::serde`.
pub mod plain {
    pub use rename_mac::describe;
}
pub mod tagged {
    pub use rename_mac::tagged_describe as describe;
}
