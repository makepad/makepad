// A derive repeats one field type node under several where-predicates, each with its own
// `for<..>` binder (T::Shim<dyn for<'t> Fn(&'s mut T::Shim<..>)> and the inner T::Shim<..>):
// the lifetime uses are resolved per predicate.
trait CallWithShim2: Sized { type Shim<T: ?Sized>; }
trait Trait<'s, 't, 'u> {}

#[derive(Clone)]
struct ShimMethod3<T: CallWithShim2 + 'static>(
    pub &'static dyn for<'s> Fn(
        &'s mut T::Shim<dyn for<'t> Fn(&'s mut T::Shim<dyn for<'u> Trait<'s, 't, 'u>>)>,
    ),
);

#[test]
fn compiles() {}
