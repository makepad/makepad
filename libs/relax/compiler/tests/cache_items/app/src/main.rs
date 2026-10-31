use cache_items_dep::*;

#[inline(never)]
fn use_leaf() -> u32 { leaf() + 1 }
#[inline(never)]
fn use_shape() -> usize { std::mem::size_of::<Shape>() }
#[inline(never)]
fn use_const() -> u32 { SCALE * 2 }
#[inline(never)]
fn use_dyn(s: &dyn Speak) -> u32 { s.speak() }
#[inline(never)]
fn use_loud<T: Speak>(s: &T) -> u32 { s.loud() }
#[inline(never)]
fn use_static() -> u32 { LEVEL + 1 }
struct Pad { a: u32 } // EDIT applayout
#[inline(never)]
fn use_pad() -> usize { std::mem::size_of::<Pad>() + Pad { a: 0 }.a as usize }
#[inline(never)]
fn unrelated() -> u32 { 42 }

fn main() {
    let d = Dog;
    println!("cache_items result {} {} {} {} {} {} {} {} {} {} {} {} {} {}", use_leaf(), use_shape(), use_const(), use_dyn(&d), use_loud(&d), unrelated(), untouched_user(), stable_after(), use_static(), local_size(), use_pad(), from_mac(), MAC_LEVEL + 1, mac_body());
}
