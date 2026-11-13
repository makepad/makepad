// An edit removes `Held`, its `new` and the `margin` it calls (walls.rs); a generic fn of an
// untouched file (`Grid::of`, grid.rs) was instantiated at the closure of `Held::new`. The
// removed names reach `Grid::of` (it calls `Self::new`), so its instances are built again with
// the edit, the removed fn's one too: its closure's environment is the one the removed version
// gave it, not looked up in a check of the removed fn against the names the edit took away.
// The edit also takes a field out of a struct, so every layout is laid out again (the closure
// environments derived so far too): "side table: no entry" ICE in an app's walls edit.
mod grid;
mod walls;

#[test]
fn run() {
    println!("out {} {} {}", walls::count(3), walls::held(4), walls::walls(2));
}
