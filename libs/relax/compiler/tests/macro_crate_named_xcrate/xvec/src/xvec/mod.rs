pub struct XSlice<T: 'static>(&'static [T]);
pub struct XVec<T: 'static>(&'static [T]);
impl<T: Copy> XSlice<T> {
    pub const fn from_ule_slice(s: &'static [T]) -> XSlice<T> {
        XSlice(s)
    }
    pub const fn as_xvec(self) -> XVec<T> {
        XVec(self.0)
    }
}
impl<T: Copy> XVec<T> {
    pub fn get(&self, i: usize) -> Option<T> {
        self.0.get(i).copied()
    }
    pub const fn len(&self) -> usize {
        self.0.len()
    }
}
#[macro_export]
macro_rules! xslice {
    () => { $crate::XSlice::from_ule_slice(&[]) };
    ($aligned:ty; $convert:expr; [$($x:expr),+ $(,)?]) => {
        $crate::XSlice::<$aligned>::from_ule_slice(const { &[$($convert($x)),*] })
    };
}
#[macro_export]
macro_rules! xvec {
    () => { $crate::XSlice::from_ule_slice(&[]).as_xvec() };
    ($aligned:ty; $convert:expr; [$($x:expr),+ $(,)?]) => {
        $crate::xslice![$aligned; $convert; [$($x),+]].as_xvec()
    };
}
