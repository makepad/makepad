pub(crate) trait DowncastRef<T> {
    fn downcast_ref(from: &T) -> Option<&Self>;
}

pub(crate) trait DowncastMut<T> {
    fn downcast_mut(from: &mut T) -> Option<&mut Self>;
}
