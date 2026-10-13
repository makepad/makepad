fn with_app(f: impl FnOnce()) { f(); }

fn shared_cast<T>(this: &mut T, replacement: T) {
    with_app(|| { let _address = this as *const T as usize; });
    *this = replacement;
}

fn mutable_cast<T>(this: &mut T, replacement: T) {
    with_app(|| { let _address = this as *mut T as usize; });
    *this = replacement;
}

fn unsize_cast(this: &mut [u32; 2]) {
    with_app(|| { let _pointer = this as *const [u32]; });
    this[0] += 1;
}

fn inferred_cast(this: &mut u32) {
    with_app(|| { let _pointer: *const u32 = this as *const _; });
    with_app(|| { let _pointer = this as *mut _ as *mut (); });
    *this += 1;
}

fn array_element_cast(this: &mut [u32; 2]) {
    with_app(|| { let _pointer = this as *const u32; });
    this[0] += 1;
}

fn implicit_coercion(this: &mut u32) {
    with_app(|| { let _pointer: *const u32 = this; });
    *this += 1;
}

fn main() {
    let mut value = 1;
    shared_cast(&mut value, 2);
    assert_eq!(value, 2);
    mutable_cast(&mut value, 3);
    assert_eq!(value, 3);
    let mut array = [1, 2];
    unsize_cast(&mut array);
    assert_eq!(array[0], 2);
    array_element_cast(&mut array);
    assert_eq!(array[0], 3);
    let mut number = 0;
    inferred_cast(&mut number);
    implicit_coercion(&mut number);
    assert_eq!(number, 2);
}

#[test]
fn raw_pointer_capture_casts() { main(); }
