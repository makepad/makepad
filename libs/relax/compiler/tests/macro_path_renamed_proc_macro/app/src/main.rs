// A path names a proc macro through the imports it crosses, a rename among them (rustc resolves
// a macro path as any other path). `time::serde::format_description!(mod_name, Date, "..")` is
// time_macros' `serde_format_description` -- before: the macro was looked up by the path's last
// name, `format_description` ran, and refused the arguments ("unexpected token: ,").
fn main() {
    println!("renamed proc macro ok {} {}", rename_mid::plain::describe!(), rename_mid::tagged::describe!(name, "x"));
}
