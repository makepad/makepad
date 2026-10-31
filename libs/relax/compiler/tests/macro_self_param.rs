// rustc_parse/src/parser/function.rs::parse_self_param preserves the receiver ident.
// rustc_resolve/src/late.rs::fresh_binding binds its macro_rules syntax context.
macro_rules! choose {
    (true $then:tt $($else:tt)*) => { $then };
}

macro_rules! iterator_shared {
    (
        is_forward = $forward:ident, item = $item:ty, iter_forward = $iter:ty,
        next($self:ident) $body:block,
    ) => {
        iterator_shared!(@mut $forward, $item, next($self) $body);
    };
    (@mut $forward:ident, $item:ty, next($self:ident) $body:block) => {
        pub const fn next(&mut $self) -> Option<$item> {
            choose! { $forward $body }
        }
    };
    (value, item = $item:ty, iter_forward = $iter:ty, next($self:ident) $body:block) => {
        pub const fn next(mut $self) -> Option<($item, $iter)> $body
    };
}

struct Split<'a> { this: &'a str, finished: bool }

impl<'a> Split<'a> {
    iterator_shared! {
        is_forward = true, item = &'a str, iter_forward = Split<'a>,
        next(self) {
            let Self { this, finished } = *self;
            if finished { return None; }
            *self = Self { this: "", finished: true };
            Some(this)
        },
    }
}

#[test]
fn captured_mut_receiver_through_nested_macros() {
    let mut iter = Split { this: "borrowed", finished: false };
    assert_eq!(iter.next(), Some("borrowed"));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.this, "");
}

struct ValueSplit<'a> { this: &'a str, finished: bool }

impl<'a> ValueSplit<'a> {
    iterator_shared! {
        value, item = &'a str, iter_forward = ValueSplit<'a>,
        next(self) {
            let Self { this, finished } = self;
            if finished { return None; }
            self.finished = true;
            Some((this, self))
        }
    }
}

#[test]
fn captured_value_receiver() {
    let iter = ValueSplit { this: "owned", finished: false };
    let (item, iter) = iter.next().unwrap();
    assert_eq!(item, "owned");
    assert!(iter.next().is_none());
}

macro_rules! defined_receivers {
    () => {
        fn next(&mut self) -> Option<u32> {
            let Self { value } = *self;
            if value == 0 { return None; }
            self.value = 0;
            Some(value)
        }
        fn into_value(self) -> u32 { self.value }
    };
}

struct Defined { value: u32 }
impl Defined { defined_receivers!(); }

#[test]
fn receiver_and_body_both_from_definition() {
    let mut iter = Defined { value: 7 };
    assert_eq!(iter.next(), Some(7));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.into_value(), 0);
}

macro_rules! restricted_receivers {
    ($self:ident, $read:block, $take:block) => {
        pub(self) fn read<'s>(&'s $self) -> u32 $read
        pub(self) fn take(mut $self: Self) -> u32 $take
    };
}

#[test]
fn local_impl_with_restricted_visibility() {
    struct Local { value: u32 }
    impl Local {
        restricted_receivers!(self, { self.value }, {
            self.value += 1;
            self.value
        });
    }
    let value = Local { value: 11 };
    assert_eq!(value.read(), 11);
    assert_eq!(value.take(), 12);
}
