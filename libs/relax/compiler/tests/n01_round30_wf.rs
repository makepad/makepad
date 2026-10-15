use std::marker::PhantomData;
use std::pin::pin;

struct CachePadded<T> { value: T }
impl<T> CachePadded<T> {
    fn new(value: T) -> Self { CachePadded::<T> { value } }
}

struct QueueHandle<State> { state: PhantomData<State> }
struct QueueProxyData<I, U, State> {
    handle: QueueHandle<State>,
    udata: U,
    interface: PhantomData<I>,
}
impl<State> QueueHandle<State> {
    fn make_data<I, U>(&self, user_data: U) -> QueueProxyData<I, U, State> {
        QueueProxyData::<I, U, State> {
            handle: QueueHandle { state: PhantomData },
            udata: user_data,
            interface: PhantomData,
        }
    }
}

// std/sync/mpmc/utils.rs:79: explicit arguments on a struct initializer.
#[test]
fn cache_padded_explicit_argument() {
    assert_eq!(CachePadded::new(13u32).value, 13);
}

// wayland-client/event_queue.rs:619: mixed inferred and explicit generic inputs.
#[test]
fn queue_proxy_data_explicit_arguments() {
    let queue: QueueHandle<u8> = QueueHandle { state: PhantomData };
    let data = queue.make_data::<u16, _>(17u32);
    assert_eq!(data.udata, 17);
}

// async_closures.rs:26: pin! expands to a qualified struct initializer.
#[test]
fn pin_macro_in_generic_body() {
    fn pinned<T>(value: T) -> T where T: Copy {
        let value = pin!(value);
        *value
    }
    assert_eq!(pinned(19u32), 19);
}
