// An edit adding a type with an inherent impl, and a method to a type that had none, whose names
// are common words (`new`, `push`, `clear`, `walk`): only the code that can reach them is checked
// and rebuilt again. The others' names never show in the patch's bodies (forbid, with
// RELAX_RELOAD_TRACE):
//  - `vec_user` calls Vec's `new` and `push`: no value it has is of the new type;
//  - `walk_user` has a local named `walk` and reads the field `walk`, and calls no method `walk`.
//  v1: `Blocks` (new) with `new`/`push`/`clear`, and `Lane::walk`, called from `kernel`.
struct Lane {
    walk: u32,
}

fn vec_user() -> usize {
    let mut v = Vec::new();
    v.push(1u8);
    v.push(2);
    v.len()
}

fn walk_user(l: &Lane) -> u32 {
    let walk = l.walk;
    walk * 2
}

fn kernel(l: &Lane) -> u32 {
    l.walk + 1
}

#[test]
fn run() {
    let l = Lane { walk: 3 };
    println!("out {} {} {}", kernel(&l), vec_user(), walk_user(&l));
}
