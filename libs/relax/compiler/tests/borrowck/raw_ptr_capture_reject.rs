fn with_app(f: impl FnOnce()) { f(); }

fn move_cast<T>(this: &mut T, replacement: T) {
    with_app(move || { let _address = this as *const T as usize; });
    *this = replacement; // E0382: a move closure still captures the reference by value.
}

fn main() {
    let mut value = 1;
    move_cast(&mut value, 2);
}
