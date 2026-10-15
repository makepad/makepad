// The files of a crate parse in parallel, each against the macro table as the crate starts plus
// its own definitions; the loader keeps a file's parse only when its macro lookups give what the
// sequential parse gives at that point of the load order. Here the prescan's latest `m!` is a's,
// but in load order b's definition comes later, so c sees b's (rustc: textual scope, the latest
// `#[macro_use]` definition): c must be parsed again by the loader.
#[macro_use]
#[path = "par_parse/a.rs"]
mod a;
#[macro_use]
#[path = "par_parse/b.rs"]
mod b;
#[path = "par_parse/c.rs"]
mod c;
#[test] fn latest_textual_definition() { assert_eq!(c::which(), 2); }
#[test] fn own_definitions() { assert_eq!(c::own(), 3); assert_eq!(a::which_a(), 1); }
#[test] fn module_path_of_files() { assert_eq!(c::path(), "test_crate::c"); }

