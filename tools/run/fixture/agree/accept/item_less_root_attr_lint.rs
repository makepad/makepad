// A crate root without items whose own attribute draws a lint (rustc: unused attribute, a
// warning). A lint takes the attributes around its node from the body of the definition it is
// reported in; a crate root has no body, and its definition names no item of its file, so with
// no items there the lookup found none and `makepad check` / `makepad test` panicked after the
// tests had passed ("item 0 is outside the parsed tree").
#![allow()]
