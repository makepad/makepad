// A reference-to-raw cast keeps an existing trait object's data and vtable.
struct ErasedFn(*const (dyn Fn(usize) -> usize + Sync));

fn erase(f: &'static (dyn Fn(usize) -> usize + Sync)) -> ErasedFn {
    ErasedFn(f as *const _)
}

#[test]
fn inferred_dyn_raw_cast() {
    let f: &'static (dyn Fn(usize) -> usize + Sync) = &|x| x + 3;
    let p = erase(f);
    assert_eq!(unsafe { (*p.0)(4) }, 7);
    let explicit = f as *const (dyn Fn(usize) -> usize + Sync);
    assert_eq!(unsafe { (*explicit)(5) }, 8);
}

trait Value {
    fn value(&self) -> usize;
}
impl Value for usize {
    fn value(&self) -> usize { *self }
}

#[test]
fn inferred_thin_pointer_still_unsizes() {
    let value = 9usize;
    let ptr: *const dyn Value = &value as *const _;
    assert_eq!(unsafe { (*ptr).value() }, 9);
}

#[test]
fn mutable_dyn_pointer_and_auto_trait_erasure() {
    let mut value = 11usize;
    let object: &mut (dyn Value + Send) = &mut value;
    let ptr: *mut (dyn Value + Send) = object as *mut _;
    assert_eq!(unsafe { (*ptr).value() }, 11);
    let object: &(dyn Value + Send) = &value;
    let ptr: *const dyn Value = object as *const _;
    assert_eq!(unsafe { (*ptr).value() }, 11);
}
