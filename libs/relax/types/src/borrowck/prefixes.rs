// ported from N:rustc_borrowck/src/prefixes.rs @8d1a764
// STOP: M0 lacks UnwrapUnsafeBinder and PhantomDeref projection variants.
use crate::mir::{Place, PlaceElem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixSet {
    All,
    Shallow,
}

pub struct Prefixes<'a> {
    place: &'a Place,
    next: Option<usize>,
    kind: PrefixSet,
}

pub fn prefixes(place: &Place, kind: PrefixSet) -> Prefixes<'_> {
    Prefixes { place, next: Some(place.projection.len()), kind }
}

impl Iterator for Prefixes<'_> {
    type Item = Place;
    fn next(&mut self) -> Option<Place> {
        let mut len = self.next?;
        loop {
            self.next = len.checked_sub(1);
            match len.checked_sub(1).map(|i| &self.place.projection[i]) {
                None | Some(PlaceElem::Field(..)) => {}
                Some(PlaceElem::Deref) => {
                    if self.kind == PrefixSet::Shallow {
                        self.next = None;
                    }
                }
                Some(PlaceElem::Subtype(_)) => panic!("Subtype is not supported in borrowck MIR"),
                Some(
                    PlaceElem::Index(_)
                    | PlaceElem::ConstantIndex { .. }
                    | PlaceElem::Subslice { .. }
                    | PlaceElem::Downcast(..)
                    | PlaceElem::OpaqueCast(_),
                ) => {
                    len -= 1;
                    continue;
                }
            }
            return Some(Place { local: self.place.local, projection: self.place.projection[..len].to_vec() });
        }
    }
}
