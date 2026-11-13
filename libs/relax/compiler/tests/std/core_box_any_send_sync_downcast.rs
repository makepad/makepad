// Box<dyn Any + Send + Sync>::downcast (stable since 1.51), beside the ones for Box<dyn Any> and
// Box<dyn Any + Send>. salvo_core 1.0's Depot keeps `Box<dyn Any + Send + Sync>` values and takes
// one out with `.downcast::<V>()` (src/depot.rs), and the std here had no such method for that
// type: "no method named `downcast` found for `Box<dyn Any + Send + Sync>`".
use std::any::Any;
use std::collections::HashMap;

// salvo's shape
#[derive(Default)]
struct Depot {
    map: HashMap<String, Box<dyn Any + Send + Sync>>,
}
impl Depot {
    fn insert<V: Any + Send + Sync>(&mut self, key: &str, value: V) {
        self.map.insert(key.to_owned(), Box::new(value));
    }
    fn remove<V: Any + Send + Sync>(&mut self, key: &str) -> Result<V, Option<Box<dyn Any + Send + Sync>>> {
        if let Some(boxed) = self.map.remove(key) { boxed.downcast::<V>().map(|b| *b).map_err(Some) } else { Err(None) }
    }
}

#[test]
fn the_value_comes_back_out() {
    let mut depot = Depot::default();
    depot.insert("n", 7u32);
    depot.insert("s", String::from("text"));
    assert_eq!(depot.remove::<u32>("n").ok(), Some(7));
    assert_eq!(depot.remove::<String>("s").ok().as_deref(), Some("text"));
    assert!(matches!(depot.remove::<u32>("n"), Err(None)));
}

#[test]
fn the_wrong_type_gives_the_box_back() {
    let b: Box<dyn Any + Send + Sync> = Box::new(5u8);
    let b = b.downcast::<u16>().unwrap_err();
    assert!(b.is::<u8>());
    let b: Box<u8> = b.downcast().unwrap();
    assert_eq!(*b, 5);
    // the other two forms stay as they were
    let plain: Box<dyn Any> = Box::new('c');
    assert_eq!(*plain.downcast::<char>().unwrap(), 'c');
    let send: Box<dyn Any + Send> = Box::new(1.5f32);
    assert_eq!(*send.downcast::<f32>().unwrap(), 1.5);
}

#[test]
fn a_value_with_a_destructor_is_dropped_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    struct Counted(Arc<AtomicUsize>);
    impl Drop for Counted {
        fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let b: Box<dyn Any + Send + Sync> = Box::new(Counted(drops.clone()));
    let b = b.downcast::<String>().unwrap_err();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let c = b.downcast::<Counted>().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(c);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
